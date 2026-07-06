//! OxidePlot MCP server — lets Claude drive OxidePlot over stdio: load a
//! dataset, understand it (stats / raw rows / a rendered image), and iterate.
//!
//! Reuses `oxideplot-core` directly (parsing, `data::table`, `statistics`, and
//! the M1 offscreen renderer). In-memory stateful sessions, no IPC.

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::{
    handler::server::tool::ToolRouter, handler::server::wrapper::Parameters, model::*, schemars,
    tool, tool_handler, tool_router, transport::stdio, ErrorData as McpError, ServerHandler,
    ServiceExt,
};
use serde_json::json;
use tokio::sync::Mutex;

use oxideplot_core::data::loader::{
    column_to_f64, column_to_timestamps, load_from_bytes, FileMeta, LoadedData,
};
use oxideplot_core::data::table::{compute_view_index, window_rows, TableQuery};
use oxideplot_core::processing::downsampling::lttb_downsample;
use oxideplot_core::processing::math_ops;
use oxideplot_core::processing::statistics::SeriesStats;
use oxideplot_core::render::axis::{compute_grid_lines, format_tick_value};
use oxideplot_core::render::gpu_types::{DrawMode, GridGpuData, PlotUniforms, SeriesGpuData};
use oxideplot_core::render::renderer::PlotRenderer;

use base64::Engine as _;

/// Series colour palette (matches the app / ColumnDialog).
const PALETTE: [[f32; 4]; 8] = [
    [0.20, 0.85, 1.00, 1.0],
    [1.00, 0.60, 0.10, 1.0],
    [0.40, 1.00, 0.40, 1.0],
    [1.00, 0.30, 0.30, 1.0],
    [0.80, 0.40, 1.00, 1.0],
    [1.00, 0.90, 0.10, 1.0],
    [0.10, 0.90, 0.70, 1.0],
    [1.00, 0.55, 0.80, 1.0],
];

/// Resolve a column reference (name or numeric index string) to a column index.
fn resolve_col(data: &LoadedData, spec: &str) -> Option<usize> {
    if let Ok(i) = spec.parse::<usize>() {
        if i < data.columns.len() {
            return Some(i);
        }
    }
    data.columns.iter().position(|c| c == spec)
}

// ─── Session state ────────────────────────────────────────────────────────────

/// A parsed dataset held in the session.
struct Dataset {
    data: LoadedData,
    /// Per-column: true if the column sorts/filters/describes numerically
    /// (numeric or datetime), matching the app's `ColumnMeta.kind` rule.
    numeric_cols: Vec<bool>,
}

/// How multiple Y series share vertical space in the render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    /// All series on one shared Y axis (raw values).
    Overlay,
    /// All series overlaid, each rescaled to its own 0..1 range (compare shapes).
    Normalized,
    /// One stacked panel per series, each with its own Y axis, sharing X.
    Stacked,
}

impl Layout {
    fn parse(s: Option<&str>) -> Self {
        match s {
            Some("normalized") => Layout::Normalized,
            Some("stacked") => Layout::Stacked,
            _ => Layout::Overlay,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Layout::Overlay => "overlay",
            Layout::Normalized => "normalized",
            Layout::Stacked => "stacked",
        }
    }
}

/// An optional per-series transform applied before plotting (reuses core math).
#[derive(Clone, Copy, Debug)]
enum Transform {
    None,
    MovingAverage(usize),
    Derivative,
    Integral,
}

impl Transform {
    fn parse(kind: Option<&str>, window: Option<usize>) -> Self {
        match kind {
            Some("moving_average") | Some("smooth") => {
                Transform::MovingAverage(window.unwrap_or(5).max(1))
            }
            Some("derivative") => Transform::Derivative,
            Some("integral") => Transform::Integral,
            _ => Transform::None,
        }
    }
    /// Short suffix for legends/titles, or None if no transform.
    fn label(self) -> Option<String> {
        match self {
            Transform::None => None,
            Transform::MovingAverage(w) => Some(format!("MA({w})")),
            Transform::Derivative => Some("d/dx".to_string()),
            Transform::Integral => Some("∫".to_string()),
        }
    }
    /// Apply to a series' `(xs, ys)`, returning the transformed y values.
    fn apply(self, xs: &[f64], ys: &[f64]) -> Vec<f64> {
        match self {
            Transform::None => ys.to_vec(),
            Transform::MovingAverage(w) => math_ops::moving_average(ys, w),
            Transform::Derivative => math_ops::derivative(xs, ys),
            Transform::Integral => math_ops::integral(xs, ys),
        }
    }
}

/// A graph definition (which dataset + columns to plot).
struct GraphSpec {
    dataset_id: String,
    x_col: usize,
    y_cols: Vec<usize>,
    draw_mode: DrawMode,
    layout: Layout,
    transform: Transform,
    title: Option<String>,
}

/// In-memory session: datasets + graph specs by id.
#[derive(Default)]
struct Session {
    datasets: HashMap<String, Dataset>,
    graphs: HashMap<String, GraphSpec>,
    next: u64,
}

impl Session {
    fn new_id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}-{}", self.next)
    }
}

// ─── Tool parameter structs ───────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct LoadParams {
    /// Absolute path to a CSV or Excel (.xlsx) file on disk.
    path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct DescribeParams {
    /// Dataset id returned by load_csv.
    dataset_id: String,
    /// Column names to describe. Omit to describe all numeric columns.
    #[serde(default)]
    columns: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct QueryParams {
    /// Dataset id returned by load_csv.
    dataset_id: String,
    /// Column name to sort by (optional).
    #[serde(default)]
    sort_col: Option<String>,
    /// Sort descending (default false = ascending).
    #[serde(default)]
    sort_desc: Option<bool>,
    /// Case-insensitive substring filter across all columns (optional).
    #[serde(default)]
    search: Option<String>,
    /// Row offset into the (sorted/filtered) result (default 0).
    #[serde(default)]
    offset: Option<usize>,
    /// Max rows to return (default 20, capped at 200).
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct CreateGraphParams {
    /// Dataset id returned by load_csv.
    dataset_id: String,
    /// X-axis column: name or numeric index.
    x_col: String,
    /// Y-axis columns: names or numeric indices (one or more).
    y_cols: Vec<String>,
    /// Draw mode: "lines" (default), "step", or "points".
    #[serde(default)]
    draw_mode: Option<String>,
    /// Vertical layout for multiple Y series: "overlay" (default, shared Y axis),
    /// "normalized" (overlaid, each rescaled to 0..1 to compare shapes), or
    /// "stacked" (one panel per series with its own Y axis, sharing X).
    #[serde(default)]
    layout: Option<String>,
    /// Transform applied to every Y series before plotting: "moving_average"
    /// (smoothing), "derivative" (dy/dx), or "integral" (cumulative). Omit for none.
    #[serde(default)]
    transform: Option<String>,
    /// Window size for moving_average (default 5).
    #[serde(default)]
    transform_window: Option<usize>,
    /// Optional title (echoed in the render text).
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RenderGraphParams {
    /// Graph id returned by create_graph.
    graph_id: String,
    /// Image width in px (default 900, clamped 200..2000).
    #[serde(default)]
    width: Option<u32>,
    /// Image height in px (default 560, clamped 150..1400).
    #[serde(default)]
    height: Option<u32>,
    /// Override the graph's layout for this render: "overlay", "normalized", or
    /// "stacked". Omit to use the layout set at create_graph time.
    #[serde(default)]
    layout: Option<String>,
    /// Override the graph's transform for this render: "moving_average",
    /// "derivative", "integral", or "none". Omit to use the create_graph value.
    #[serde(default)]
    transform: Option<String>,
    /// Window size for a moving_average override (default 5).
    #[serde(default)]
    transform_window: Option<usize>,
}

// ─── Server ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct OxidePlot {
    session: Arc<Mutex<Session>>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl OxidePlot {
    fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(Session::default())),
            tool_router: Self::tool_router(),
        }
    }

    fn text_result(value: serde_json::Value) -> CallToolResult {
        CallToolResult::success(vec![Content::text(value.to_string())])
    }

    #[tool(description = "Connectivity check — echoes the message back.")]
    async fn ping(
        &self,
        Parameters(PingParams { message }): Parameters<PingParams>,
    ) -> Result<CallToolResult, McpError> {
        Ok(Self::text_result(json!({ "pong": message })))
    }

    #[tool(
        description = "Load a CSV or Excel (.xlsx) file from an absolute disk path. Returns a dataset_id plus the columns (index, name, kind) and row count."
    )]
    async fn load_csv(
        &self,
        Parameters(LoadParams { path }): Parameters<LoadParams>,
    ) -> Result<CallToolResult, McpError> {
        let bytes = std::fs::read(&path)
            .map_err(|e| McpError::internal_error(format!("cannot read '{path}': {e}"), None))?;
        let fname = std::path::Path::new(&path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&path)
            .to_string();
        let data = load_from_bytes(&bytes, &fname)
            .map_err(|e| McpError::internal_error(format!("parse failed: {e}"), None))?;
        let meta = FileMeta::from_loaded(&data);
        let numeric_cols: Vec<bool> = (0..data.columns.len())
            .map(|c| {
                let col = &data.column_data[c];
                column_to_f64(col).1 >= 0.5 || column_to_timestamps(col).is_some()
            })
            .collect();

        let mut s = self.session.lock().await;
        let id = s.new_id("ds");
        let cols_json: Vec<_> = meta
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| json!({ "index": i, "name": c.name, "kind": c.kind }))
            .collect();
        let summary = json!({
            "dataset_id": id,
            "name": fname,
            "rows": data.row_count,
            "columns": cols_json,
        });
        s.datasets.insert(id, Dataset { data, numeric_cols });
        Ok(Self::text_result(summary))
    }

    #[tool(
        description = "Summary statistics (count, min, max, mean, median, std_dev, peak_to_peak) for a dataset's numeric columns. Pass column names to limit; omit for all numeric columns."
    )]
    async fn describe_data(
        &self,
        Parameters(DescribeParams {
            dataset_id,
            columns,
        }): Parameters<DescribeParams>,
    ) -> Result<CallToolResult, McpError> {
        let s = self.session.lock().await;
        let ds = s
            .datasets
            .get(&dataset_id)
            .ok_or_else(|| McpError::invalid_params(format!("unknown dataset_id '{dataset_id}'"), None))?;

        let indices: Vec<usize> = match columns {
            Some(names) => names
                .iter()
                .filter_map(|n| ds.data.columns.iter().position(|c| c == n))
                .collect(),
            None => (0..ds.data.columns.len())
                .filter(|&c| ds.numeric_cols[c])
                .collect(),
        };

        let mut out = Vec::new();
        for c in indices {
            let (vals, _) = column_to_f64(&ds.data.column_data[c]);
            match SeriesStats::compute(&vals) {
                Some(st) => out.push(json!({
                    "column": ds.data.columns[c],
                    "count": st.count,
                    "min": st.min,
                    "max": st.max,
                    "mean": st.mean,
                    "median": st.median,
                    "std_dev": st.std_dev,
                    "peak_to_peak": st.peak_to_peak,
                })),
                None => out.push(json!({
                    "column": ds.data.columns[c],
                    "note": "no finite numeric values",
                })),
            }
        }
        Ok(Self::text_result(json!({ "stats": out })))
    }

    #[tool(
        description = "Return a page of raw rows from a dataset, with optional sort (by column name), case-insensitive search, and paging (offset/limit). Use this to inspect actual values."
    )]
    async fn query_data(
        &self,
        Parameters(QueryParams {
            dataset_id,
            sort_col,
            sort_desc,
            search,
            offset,
            limit,
        }): Parameters<QueryParams>,
    ) -> Result<CallToolResult, McpError> {
        let s = self.session.lock().await;
        let ds = s
            .datasets
            .get(&dataset_id)
            .ok_or_else(|| McpError::invalid_params(format!("unknown dataset_id '{dataset_id}'"), None))?;

        let mut q = TableQuery {
            numeric_cols: ds.numeric_cols.clone(),
            ..Default::default()
        };
        if let Some(name) = sort_col {
            if let Some(ci) = ds.data.columns.iter().position(|c| *c == name) {
                // TableQuery.sort = (col, ascending); sort_desc flips it.
                q.sort = Some((ci, !sort_desc.unwrap_or(false)));
            }
        }
        if let Some(term) = search {
            q.search = term;
        }

        let idx = compute_view_index(&ds.data, &q);
        let total = idx.len();
        let start = offset.unwrap_or(0);
        let count = limit.unwrap_or(20).min(200);
        let rows = window_rows(&ds.data, &idx, start, count);

        Ok(Self::text_result(json!({
            "total": total,
            "offset": start,
            "returned": rows.len(),
            "columns": ds.data.columns,
            "rows": rows,
        })))
    }

    #[tool(
        description = "Define a graph: pick the X column and one or more Y columns (by name or index) from a dataset. Returns a graph_id to render with render_graph."
    )]
    async fn create_graph(
        &self,
        Parameters(CreateGraphParams {
            dataset_id,
            x_col,
            y_cols,
            draw_mode,
            layout,
            transform,
            transform_window,
            title,
        }): Parameters<CreateGraphParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut s = self.session.lock().await;
        let (x, ys, x_name, y_names) = {
            let ds = s.datasets.get(&dataset_id).ok_or_else(|| {
                McpError::invalid_params(format!("unknown dataset_id '{dataset_id}'"), None)
            })?;
            let x = resolve_col(&ds.data, &x_col)
                .ok_or_else(|| McpError::invalid_params(format!("unknown x_col '{x_col}'"), None))?;
            let mut ys = Vec::new();
            for name in &y_cols {
                ys.push(resolve_col(&ds.data, name).ok_or_else(|| {
                    McpError::invalid_params(format!("unknown y_col '{name}'"), None)
                })?);
            }
            if ys.is_empty() {
                return Err(McpError::invalid_params(
                    "y_cols must have at least one column".to_string(),
                    None,
                ));
            }
            let x_name = ds.data.columns[x].clone();
            let y_names: Vec<String> = ys.iter().map(|&i| ds.data.columns[i].clone()).collect();
            (x, ys, x_name, y_names)
        };
        let dm = match draw_mode.as_deref() {
            Some("step") => DrawMode::Step,
            Some("points") => DrawMode::Points,
            _ => DrawMode::Lines,
        };
        let lay = Layout::parse(layout.as_deref());
        let tf = Transform::parse(transform.as_deref(), transform_window);

        let id = s.new_id("gr");
        s.graphs.insert(
            id.clone(),
            GraphSpec {
                dataset_id: dataset_id.clone(),
                x_col: x,
                y_cols: ys,
                draw_mode: dm,
                layout: lay,
                transform: tf,
                title,
            },
        );
        Ok(Self::text_result(json!({
            "graph_id": id,
            "dataset_id": dataset_id,
            "x": x_name,
            "ys": y_names,
            "draw_mode": format!("{dm:?}").to_lowercase(),
            "layout": lay.as_str(),
            "transform": tf.label(),
        })))
    }

    #[tool(
        description = "Render a graph to a PNG image (returned as image content) plus a text block with the axis ranges, tick values, and legend. Use this to SEE the plot."
    )]
    async fn render_graph(
        &self,
        Parameters(RenderGraphParams {
            graph_id,
            width,
            height,
            layout,
            transform,
            transform_window,
        }): Parameters<RenderGraphParams>,
    ) -> Result<CallToolResult, McpError> {
        // Build per-panel render inputs under the lock; render without holding it.
        let (panels, panel_w, panel_h, out_h, clear, text) = {
            let s = self.session.lock().await;
            let g = s.graphs.get(&graph_id).ok_or_else(|| {
                McpError::invalid_params(format!("unknown graph_id '{graph_id}'"), None)
            })?;
            let ds = s.datasets.get(&g.dataset_id).ok_or_else(|| {
                McpError::internal_error(
                    format!("graph references missing dataset '{}'", g.dataset_id),
                    None,
                )
            })?;

            let w = width.unwrap_or(900).clamp(200, 2000);
            let h = height.unwrap_or(560).clamp(150, 1400);
            let lay = match layout.as_deref() {
                Some(v) => Layout::parse(Some(v)),
                None => g.layout,
            };
            let tf = match transform.as_deref() {
                Some("none") => Transform::None,
                Some(k) => Transform::parse(Some(k), transform_window),
                None => g.transform,
            };

            // X values: datetime → timestamps, else numeric.
            let xcol = &ds.data.column_data[g.x_col];
            let xs: Vec<f64> = match column_to_timestamps(xcol) {
                Some((v, _)) => v,
                None => column_to_f64(xcol).0,
            };

            // Per-series finite points + colour + own y-range. Very large series
            // are LTTB-downsampled to ~2×width for rendering (keeps the shape,
            // renders fast — this is the win for files too big to read directly).
            let render_cap = (w as usize) * 2;
            let mut sdata: Vec<PanelSeries> = Vec::new();
            let (mut xmin, mut xmax) = (f64::INFINITY, f64::NEG_INFINITY);
            let mut max_raw_points = 0usize;
            let mut any_downsampled = false;
            for (k, &yc) in g.y_cols.iter().enumerate() {
                let (ysv, _) = column_to_f64(&ds.data.column_data[yc]);
                let ysv = tf.apply(&xs, &ysv); // optional transform (smooth/derivative/integral)

                // Finite (x, y) pairs + y-range over the FULL series.
                let mut fx: Vec<f64> = Vec::new();
                let mut fy: Vec<f64> = Vec::new();
                let (mut ymn, mut ymx) = (f64::INFINITY, f64::NEG_INFINITY);
                for (&x, &y) in xs.iter().zip(ysv.iter()) {
                    if x.is_finite() && y.is_finite() {
                        fx.push(x);
                        fy.push(y);
                        xmin = xmin.min(x);
                        xmax = xmax.max(x);
                        ymn = ymn.min(y);
                        ymx = ymx.max(y);
                    }
                }
                max_raw_points = max_raw_points.max(fx.len());

                // Downsample for rendering if there are far more points than pixels.
                let (fx, fy) = if fx.len() > render_cap * 2 {
                    any_downsampled = true;
                    lttb_downsample(&fx, &fy, render_cap)
                } else {
                    (fx, fy)
                };

                let pts: Vec<[f32; 2]> = fx
                    .iter()
                    .zip(fy.iter())
                    .map(|(&x, &y)| [x as f32, y as f32])
                    .collect();
                sdata.push(PanelSeries {
                    name: ds.data.columns[yc].clone(),
                    color: PALETTE[k % PALETTE.len()],
                    points: pts,
                    ymin: ymn,
                    ymax: ymx,
                });
            }
            if !xmin.is_finite() || (xmax - xmin) <= 0.0 {
                return Err(McpError::internal_error(
                    "no finite data to plot for this graph".to_string(),
                    None,
                ));
            }
            let xpad = ((xmax - xmin) * 0.03).max(1e-9);
            let x_view = (xmin - xpad, xmax + xpad);

            // Panels: overlay/normalized → 1 panel (all series); stacked → 1 per series.
            let (n, panel_h) = match lay {
                Layout::Stacked => {
                    let n = sdata.len().max(1) as u32;
                    (n, (h / n).max(60))
                }
                _ => (1u32, h),
            };
            let normalize = lay == Layout::Normalized;
            let mut panels: Vec<(Vec<SeriesGpuData>, GridGpuData, PlotUniforms)> = Vec::new();
            if lay == Layout::Stacked {
                for sd in &sdata {
                    panels.push(build_panel(
                        std::slice::from_ref(sd),
                        x_view,
                        w,
                        panel_h,
                        g.draw_mode,
                        false,
                    ));
                }
            } else {
                panels.push(build_panel(&sdata, x_view, w, panel_h, g.draw_mode, normalize));
            }
            let out_h = panel_h * n;

            // Text companion.
            let x_ticks: Vec<String> = compute_grid_lines(x_view.0, x_view.1)
                .iter()
                .filter(|(_, m)| *m)
                .map(|(v, _)| format_tick_value(*v))
                .collect();
            let legend: Vec<serde_json::Value> = sdata
                .iter()
                .map(|sd| {
                    let c = sd.color;
                    json!({
                        "series": sd.name,
                        "y_range": [sd.ymin, sd.ymax],
                        "color_rgb": [(c[0]*255.0) as u8, (c[1]*255.0) as u8, (c[2]*255.0) as u8],
                    })
                })
                .collect();
            let note = match lay {
                Layout::Stacked => "Stacked: each series in its own panel (top→bottom) with its own Y axis, sharing X.",
                Layout::Normalized => "Normalized: each series rescaled to 0..1 so shapes are comparable regardless of scale.",
                Layout::Overlay => "Overlay: all series share one Y axis (a large-scale series can dwarf a small one).",
            };
            let text = json!({
                "title": g.title.clone(),
                "layout": lay.as_str(),
                "transform": tf.label(),
                "x_axis": ds.data.columns[g.x_col].clone(),
                "x_range": [xmin, xmax],
                "x_ticks": x_ticks,
                "series": legend,
                "size": [w, out_h],
                "points_per_series": max_raw_points,
                "downsampled_for_render": any_downsampled,
                "note": note,
            })
            .to_string();

            let clear = [0.055_f64, 0.059, 0.075, 1.0];
            (panels, w, panel_h, out_h, clear, text)
        };

        // Render off the lock, on a blocking thread (GPU read-back blocks).
        let rgba = tokio::task::spawn_blocking(move || {
            pollster::block_on(async move {
                let r = PlotRenderer::new_offscreen(panel_w, panel_h).await;
                if panels.len() == 1 {
                    let (series, grid, uniforms) = &panels[0];
                    let calls = r.build_draw_calls(series, grid, *uniforms);
                    r.render_to_rgba(&calls, clear)
                } else {
                    // Stacked: render each panel, composite vertically into one image.
                    let cb = [
                        (clear[0] * 255.0) as u8,
                        (clear[1] * 255.0) as u8,
                        (clear[2] * 255.0) as u8,
                        255u8,
                    ];
                    let row_bytes = (panel_w * 4) as usize;
                    let mut composite: Vec<u8> = Vec::with_capacity((panel_w * out_h * 4) as usize);
                    for _ in 0..(panel_w * out_h) {
                        composite.extend_from_slice(&cb);
                    }
                    for (i, (series, grid, uniforms)) in panels.iter().enumerate() {
                        let calls = r.build_draw_calls(series, grid, *uniforms);
                        let prgba = r.render_to_rgba(&calls, clear);
                        let y0 = i as u32 * panel_h;
                        for row in 0..panel_h {
                            let src = (row * panel_w * 4) as usize;
                            let dst = ((y0 + row) * panel_w * 4) as usize;
                            composite[dst..dst + row_bytes]
                                .copy_from_slice(&prgba[src..src + row_bytes]);
                        }
                        // Thin separator between panels (not after the last).
                        if i + 1 < panels.len() {
                            let sy = y0 + panel_h - 1;
                            let base = (sy * panel_w * 4) as usize;
                            for px in 0..panel_w as usize {
                                let o = base + px * 4;
                                composite[o..o + 4].copy_from_slice(&[90, 94, 104, 255]);
                            }
                        }
                    }
                    composite
                }
            })
        })
        .await
        .map_err(|e| McpError::internal_error(format!("render task failed: {e}"), None))?;

        // Encode PNG → base64 → image content.
        let img = image::RgbaImage::from_raw(panel_w, out_h, rgba).ok_or_else(|| {
            McpError::internal_error("render produced a mis-sized buffer".to_string(), None)
        })?;
        let mut png: Vec<u8> = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| McpError::internal_error(format!("PNG encode failed: {e}"), None))?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);

        Ok(CallToolResult::success(vec![
            Content::image(b64, "image/png".to_string()),
            Content::text(text),
        ]))
    }
}

/// One series' finite points + colour + its own y-range (input to `build_panel`).
struct PanelSeries {
    name: String,
    color: [f32; 4],
    points: Vec<[f32; 2]>,
    ymin: f64,
    ymax: f64,
}

/// Build the (series, grid, uniforms) for one panel over the shared `x_view`.
/// When `normalize`, each series' y is rescaled to 0..1 (its own range) and the
/// y-view is fixed to [-0.05, 1.05]; otherwise the y-view fits the panel's data.
fn build_panel(
    series: &[PanelSeries],
    x_view: (f64, f64),
    w: u32,
    h: u32,
    draw_mode: DrawMode,
    normalize: bool,
) -> (Vec<SeriesGpuData>, GridGpuData, PlotUniforms) {
    let (y_view, gpu_series): ((f64, f64), Vec<SeriesGpuData>) = if normalize {
        let s = series
            .iter()
            .map(|sd| {
                let range = (sd.ymax - sd.ymin).max(1e-12);
                let pts: Vec<[f32; 2]> = sd
                    .points
                    .iter()
                    .map(|[x, y]| [*x, ((*y as f64 - sd.ymin) / range) as f32])
                    .collect();
                SeriesGpuData {
                    points: pts,
                    color: sd.color,
                    line_width: 2.0,
                    point_radius: 3.0,
                    draw_mode,
                }
            })
            .collect();
        ((-0.05, 1.05), s)
    } else {
        let mut ymn = f64::INFINITY;
        let mut ymx = f64::NEG_INFINITY;
        for sd in series {
            if sd.ymin.is_finite() {
                ymn = ymn.min(sd.ymin);
            }
            if sd.ymax.is_finite() {
                ymx = ymx.max(sd.ymax);
            }
        }
        if !ymn.is_finite() || !ymx.is_finite() {
            ymn = 0.0;
            ymx = 1.0;
        }
        let ypad = ((ymx - ymn) * 0.05).max(1e-9);
        let s = series
            .iter()
            .map(|sd| SeriesGpuData {
                points: sd.points.clone(),
                color: sd.color,
                line_width: 2.0,
                point_radius: 3.0,
                draw_mode,
            })
            .collect();
        ((ymn - ypad, ymx + ypad), s)
    };

    let x_ticks = compute_grid_lines(x_view.0, x_view.1);
    let y_ticks = compute_grid_lines(y_view.0, y_view.1);
    let mut segs: Vec<[f32; 2]> = Vec::new();
    for (xv, _) in &x_ticks {
        segs.push([*xv as f32, y_view.0 as f32]);
        segs.push([*xv as f32, y_view.1 as f32]);
    }
    for (yv, _) in &y_ticks {
        segs.push([x_view.0 as f32, *yv as f32]);
        segs.push([x_view.1 as f32, *yv as f32]);
    }
    let grid = GridGpuData {
        segments: segs,
        color: [0.45, 0.47, 0.55, 0.22],
        line_width: 1.0,
    };
    let uniforms = PlotUniforms {
        view_min: [x_view.0 as f32, y_view.0 as f32],
        view_max: [x_view.1 as f32, y_view.1 as f32],
        resolution: [w as f32, h as f32],
        line_width: 2.0,
        point_radius: 3.0,
        color: [0.0, 0.0, 0.0, 0.0],
        _padding: [0.0; 4],
    };
    (gpu_series, grid, uniforms)
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct PingParams {
    /// Any text; the server echoes it back.
    message: String,
}

#[tool_handler]
impl ServerHandler for OxidePlot {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "OxidePlot: load CSV/Excel data (load_csv), understand it via statistics \
                 (describe_data) and raw rows (query_data), then build and render plots to \
                 images (create_graph + render_graph)."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = OxidePlot::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
