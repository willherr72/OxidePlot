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

/// A graph definition (which dataset + columns to plot).
struct GraphSpec {
    dataset_id: String,
    x_col: usize,
    y_cols: Vec<usize>,
    draw_mode: DrawMode,
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

        let id = s.new_id("gr");
        s.graphs.insert(
            id.clone(),
            GraphSpec {
                dataset_id: dataset_id.clone(),
                x_col: x,
                y_cols: ys,
                draw_mode: dm,
                title,
            },
        );
        Ok(Self::text_result(json!({
            "graph_id": id,
            "dataset_id": dataset_id,
            "x": x_name,
            "ys": y_names,
            "draw_mode": format!("{dm:?}").to_lowercase(),
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
        }): Parameters<RenderGraphParams>,
    ) -> Result<CallToolResult, McpError> {
        // Build all render inputs under the lock, then render without holding it.
        let (series, grid, uniforms, w, h, text) = {
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

            // X values: datetime → timestamps, else numeric.
            let xcol = &ds.data.column_data[g.x_col];
            let xs: Vec<f64> = match column_to_timestamps(xcol) {
                Some((v, _)) => v,
                None => column_to_f64(xcol).0,
            };

            let mut series = Vec::new();
            let (mut xmin, mut xmax, mut ymin, mut ymax) =
                (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
            for (k, &yc) in g.y_cols.iter().enumerate() {
                let (ysv, _) = column_to_f64(&ds.data.column_data[yc]);
                let mut pts: Vec<[f32; 2]> = Vec::new();
                for (&x, &y) in xs.iter().zip(ysv.iter()) {
                    if x.is_finite() && y.is_finite() {
                        pts.push([x as f32, y as f32]);
                        xmin = xmin.min(x);
                        xmax = xmax.max(x);
                        ymin = ymin.min(y);
                        ymax = ymax.max(y);
                    }
                }
                series.push(SeriesGpuData {
                    points: pts,
                    color: PALETTE[k % PALETTE.len()],
                    line_width: 2.0,
                    point_radius: 3.0,
                    draw_mode: g.draw_mode,
                });
            }
            if !xmin.is_finite() || !ymin.is_finite() || (xmax - xmin) <= 0.0 {
                return Err(McpError::internal_error(
                    "no finite data to plot for this graph".to_string(),
                    None,
                ));
            }

            let xpad = ((xmax - xmin) * 0.03).max(1e-9);
            let ypad = ((ymax - ymin) * 0.05).max(1e-9);
            let (vxmin, vxmax, vymin, vymax) =
                (xmin - xpad, xmax + xpad, ymin - ypad, ymax + ypad);

            // Build GPU grid segments (the app draws grid via SVG; offscreen needs its own).
            let x_ticks = compute_grid_lines(vxmin, vxmax);
            let y_ticks = compute_grid_lines(vymin, vymax);
            let mut segs: Vec<[f32; 2]> = Vec::new();
            for (xv, _) in &x_ticks {
                segs.push([*xv as f32, vymin as f32]);
                segs.push([*xv as f32, vymax as f32]);
            }
            for (yv, _) in &y_ticks {
                segs.push([vxmin as f32, *yv as f32]);
                segs.push([vxmax as f32, *yv as f32]);
            }
            let grid = GridGpuData {
                segments: segs,
                color: [0.45, 0.47, 0.55, 0.22],
                line_width: 1.0,
            };

            let uniforms = PlotUniforms {
                view_min: [vxmin as f32, vymin as f32],
                view_max: [vxmax as f32, vymax as f32],
                resolution: [w as f32, h as f32],
                line_width: 2.0,
                point_radius: 3.0,
                color: [0.0, 0.0, 0.0, 0.0],
                _padding: [0.0; 4],
            };

            // Text companion: ranges, major tick labels, and the legend (so the
            // scales are readable even though the GPU layer draws no tick text).
            let x_tick_labels: Vec<String> = x_ticks
                .iter()
                .filter(|(_, major)| *major)
                .map(|(v, _)| format_tick_value(*v))
                .collect();
            let y_tick_labels: Vec<String> = y_ticks
                .iter()
                .filter(|(_, major)| *major)
                .map(|(v, _)| format_tick_value(*v))
                .collect();
            let legend: Vec<serde_json::Value> = g
                .y_cols
                .iter()
                .enumerate()
                .map(|(k, &yc)| {
                    let c = PALETTE[k % PALETTE.len()];
                    json!({
                        "series": ds.data.columns[yc],
                        "color_rgb": [(c[0]*255.0) as u8, (c[1]*255.0) as u8, (c[2]*255.0) as u8],
                    })
                })
                .collect();
            let text = json!({
                "title": g.title.clone(),
                "x_axis": ds.data.columns[g.x_col].clone(),
                "x_range": [xmin, xmax],
                "x_ticks": x_tick_labels,
                "y_range": [ymin, ymax],
                "y_ticks": y_tick_labels,
                "legend": legend,
                "size": [w, h],
                "note": "PNG shows the plotted series + grid; axis tick labels are in x_ticks/y_ticks.",
            })
            .to_string();

            (series, grid, uniforms, w, h, text)
        };

        // Render off the lock, on a blocking thread (GPU read-back blocks).
        let clear = [0.055_f64, 0.059, 0.075, 1.0];
        let rgba = tokio::task::spawn_blocking(move || {
            pollster::block_on(async {
                let r = PlotRenderer::new_offscreen(w, h).await;
                let calls = r.build_draw_calls(&series, &grid, uniforms);
                r.render_to_rgba(&calls, clear)
            })
        })
        .await
        .map_err(|e| McpError::internal_error(format!("render task failed: {e}"), None))?;

        // Encode PNG → base64 → image content.
        let img = image::RgbaImage::from_raw(w, h, rgba)
            .ok_or_else(|| McpError::internal_error("render produced a mis-sized buffer".to_string(), None))?;
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
