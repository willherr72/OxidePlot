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

use oxideplot_core::data::datetime::format_timestamp;
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

/// Longest run of consecutive identical raw cells (flags a frozen/stuck channel).
fn longest_constant_run(cells: &[String]) -> usize {
    let mut best = 0usize;
    let mut cur = 0usize;
    let mut prev: Option<&str> = None;
    for c in cells {
        if prev == Some(c.as_str()) {
            cur += 1;
        } else {
            cur = 1;
            prev = Some(c.as_str());
        }
        best = best.max(cur);
    }
    best
}

/// Min/max envelope decimation: split into `buckets` equal index ranges and keep
/// each bucket's min-y AND max-y point (in x order). Unlike LTTB this NEVER drops
/// a 1-sample spike or dropout — the extreme in each bucket is always kept.
/// Returns up to 2×buckets points.
fn minmax_envelope(fx: &[f64], fy: &[f64], buckets: usize) -> (Vec<f64>, Vec<f64>) {
    let n = fx.len();
    if buckets == 0 || n <= buckets * 2 {
        return (fx.to_vec(), fy.to_vec());
    }
    let mut ox = Vec::with_capacity(buckets * 2);
    let mut oy = Vec::with_capacity(buckets * 2);
    for b in 0..buckets {
        let lo = b * n / buckets;
        let hi = ((b + 1) * n / buckets).min(n);
        if lo >= hi {
            continue;
        }
        let mut imin = lo;
        let mut imax = lo;
        for i in lo..hi {
            if fy[i] < fy[imin] {
                imin = i;
            }
            if fy[i] > fy[imax] {
                imax = i;
            }
        }
        let (a, c) = if imin <= imax { (imin, imax) } else { (imax, imin) };
        ox.push(fx[a]);
        oy.push(fy[a]);
        if c != a {
            ox.push(fx[c]);
            oy.push(fy[c]);
        }
    }
    (ox, oy)
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
    /// Window to a row-index range (inclusive start, exclusive end) — e.g. rows
    /// 6700–6800 to inspect a glitch. Rows within the window are NOT downsampled
    /// unless they still exceed ~2×width, so single-sample spikes survive.
    #[serde(default)]
    row_start: Option<usize>,
    #[serde(default)]
    row_end: Option<usize>,
    /// Window to an X-value range. For a numeric X these are the values; for a
    /// datetime X they are epoch seconds. Applied in addition to row_start/end.
    #[serde(default)]
    x_min: Option<f64>,
    #[serde(default)]
    x_max: Option<f64>,
    /// Downsampling for large series: "minmax" (default — keeps the min & max of
    /// each bucket, so single-sample spikes/dropouts are NEVER lost; best for QC),
    /// "lttb" (smoother, may drop a lone spike), or "none" (plot every point).
    #[serde(default)]
    downsample: Option<String>,
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
        description = "Per-column QC + summary stats: n_total, n_missing, pct_zero, distinct, longest_constant_run (flag dead / frozen / duplicate channels), plus min/max/mean/median/std_dev/peak_to_peak for numeric columns. Omit 'columns' to describe ALL columns in one call."
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
            None => (0..ds.data.columns.len()).collect(), // all columns → one-call QC overview
        };

        let mut out = Vec::new();
        for c in indices {
            let cells = &ds.data.column_data[c];
            let n_total = cells.len();
            // QC stats (computed for every column, numeric or not).
            let distinct = cells.iter().collect::<std::collections::HashSet<_>>().len();
            let const_run = longest_constant_run(cells);
            let (vals, _) = column_to_f64(cells);
            let n_finite = vals.iter().filter(|v| v.is_finite()).count();
            let n_missing = n_total.saturating_sub(n_finite);
            let n_zero = vals.iter().filter(|v| v.is_finite() && **v == 0.0).count();
            let pct_zero = if n_finite == 0 {
                0.0
            } else {
                100.0 * n_zero as f64 / n_finite as f64
            };

            let mut obj = serde_json::Map::new();
            obj.insert("column".into(), json!(ds.data.columns[c]));
            obj.insert(
                "kind".into(),
                json!(if ds.numeric_cols[c] { "numeric" } else { "text" }),
            );
            obj.insert("n_total".into(), json!(n_total));
            obj.insert("n_missing".into(), json!(n_missing));
            obj.insert("pct_zero".into(), json!((pct_zero * 10.0).round() / 10.0));
            obj.insert("distinct".into(), json!(distinct));
            obj.insert("longest_constant_run".into(), json!(const_run));
            // Numeric summary (only for columns with finite values).
            if let Some(st) = SeriesStats::compute(&vals) {
                obj.insert("min".into(), json!(st.min));
                obj.insert("max".into(), json!(st.max));
                obj.insert("mean".into(), json!(st.mean));
                obj.insert("median".into(), json!(st.median));
                obj.insert("std_dev".into(), json!(st.std_dev));
                obj.insert("peak_to_peak".into(), json!(st.peak_to_peak));
            }
            out.push(serde_json::Value::Object(obj));
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
            if !ds.numeric_cols[x] {
                return Err(McpError::invalid_params(
                    format!(
                        "x_col '{}' is a text column; X must be numeric or datetime",
                        ds.data.columns[x]
                    ),
                    None,
                ));
            }
            let mut ys = Vec::new();
            for name in &y_cols {
                let ci = resolve_col(&ds.data, name).ok_or_else(|| {
                    McpError::invalid_params(format!("unknown y_col '{name}'"), None)
                })?;
                if !ds.numeric_cols[ci] {
                    return Err(McpError::invalid_params(
                        format!(
                            "y_col '{}' is a text column; Y columns must be numeric",
                            ds.data.columns[ci]
                        ),
                        None,
                    ));
                }
                ys.push(ci);
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
            row_start,
            row_end,
            x_min,
            x_max,
            downsample,
        }): Parameters<RenderGraphParams>,
    ) -> Result<CallToolResult, McpError> {
        // Build per-panel render inputs under the lock; render without holding it.
        let (panels, panel_w, panel_h, out_h, clear, x_is_time, text) = {
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

            // X values: datetime → epoch-second timestamps, else numeric.
            let xcol = &ds.data.column_data[g.x_col];
            let (xs, x_is_time): (Vec<f64>, bool) = match column_to_timestamps(xcol) {
                Some((v, _)) => (v, true),
                None => (column_to_f64(xcol).0, false),
            };

            // Per-series finite points + colour + own y-range. Very large series
            // are LTTB-downsampled to ~2×width for rendering (keeps the shape,
            // renders fast — this is the win for files too big to read directly).
            let render_cap = (w as usize) * 2;
            let ds_mode = downsample.as_deref().unwrap_or("minmax");
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
                for (row, (&x, &y)) in xs.iter().zip(ysv.iter()).enumerate() {
                    if row_start.is_some_and(|rs| row < rs) || row_end.is_some_and(|re| row >= re) {
                        continue;
                    }
                    if !x.is_finite() || !y.is_finite() {
                        continue;
                    }
                    if x_min.is_some_and(|xm| x < xm) || x_max.is_some_and(|xx| x > xx) {
                        continue;
                    }
                    fx.push(x);
                    fy.push(y);
                    xmin = xmin.min(x);
                    xmax = xmax.max(x);
                    ymn = ymn.min(y);
                    ymx = ymx.max(y);
                }
                max_raw_points = max_raw_points.max(fx.len());

                // Downsample for rendering if there are far more points than pixels.
                // Default "minmax" keeps each bucket's extremes so spikes survive.
                let (fx, fy) = if ds_mode != "none" && fx.len() > render_cap * 2 {
                    any_downsampled = true;
                    if ds_mode == "lttb" {
                        lttb_downsample(&fx, &fy, render_cap)
                    } else {
                        minmax_envelope(&fx, &fy, w as usize)
                    }
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
                .map(|(v, _)| {
                    if x_is_time {
                        format_timestamp(*v)
                    } else {
                        format_tick_value(*v)
                    }
                })
                .collect();
            let x_range_json = if x_is_time {
                json!([format_timestamp(xmin), format_timestamp(xmax)])
            } else {
                json!([xmin, xmax])
            };
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
                "x_is_time": x_is_time,
                "x_range": x_range_json,
                "x_ticks": x_ticks,
                "series": legend,
                "size": [w, out_h],
                "points_per_series": max_raw_points,
                "downsampled_for_render": any_downsampled,
                "downsample": ds_mode,
                "window": { "row_start": row_start, "row_end": row_end, "x_min": x_min, "x_max": x_max },
                "note": note,
            })
            .to_string();

            let clear = [0.055_f64, 0.059, 0.075, 1.0];
            (panels, w, panel_h, out_h, clear, x_is_time, text)
        };

        // Render off the lock, on a blocking thread (GPU read-back blocks).
        let rgba = tokio::task::spawn_blocking(move || {
            pollster::block_on(async move {
                let r = PlotRenderer::new_offscreen(panel_w, panel_h).await;
                let mut buf: Vec<u8> = if panels.len() == 1 {
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
                };

                // Bake numeric tick labels onto each panel (map tick value → pixel
                // via the panel's view). Title/legend stay in the text companion.
                let lc = [198u8, 204, 214, 255];
                for (i, (_, _, u)) in panels.iter().enumerate() {
                    let y_off = i as u32 * panel_h;
                    let (vxmin, vxmax) = (u.view_min[0] as f64, u.view_max[0] as f64);
                    let (vymin, vymax) = (u.view_min[1] as f64, u.view_max[1] as f64);
                    let xspan = (vxmax - vxmin).max(1e-12);
                    let yspan = (vymax - vymin).max(1e-12);
                    let x_scale = if x_is_time { 1 } else { 2 }; // datetime labels are longer
                    for (v, major) in compute_grid_lines(vxmin, vxmax) {
                        if !major {
                            continue;
                        }
                        let px = (((v - vxmin) / xspan) * panel_w as f64) as i32;
                        draw_text(
                            &mut buf,
                            panel_w,
                            out_h,
                            &fmt_x_tick(v, x_is_time, vxmax - vxmin),
                            px + 3,
                            (y_off + panel_h) as i32 - if x_is_time { 10 } else { 15 },
                            lc,
                            x_scale,
                        );
                    }
                    for (v, major) in compute_grid_lines(vymin, vymax) {
                        if !major {
                            continue;
                        }
                        let py = ((1.0 - (v - vymin) / yspan) * panel_h as f64) as i32 + y_off as i32;
                        draw_text(&mut buf, panel_w, out_h, &format_tick_value(v), 3, py - 5, lc, 2);
                    }
                }
                buf
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

// ─── Minimal 5×7 bitmap font for numeric tick labels (no font asset needed) ─────

/// 5×7 glyph rows (bit 4 = leftmost pixel) for the characters that appear in
/// numeric tick labels. Unknown chars render as blank (space).
fn glyph5x7(c: char) -> [u8; 7] {
    match c {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x0E, 0x11, 0x01, 0x06, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '+' => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
        'e' | 'E' => [0x00, 0x00, 0x0E, 0x11, 0x1E, 0x10, 0x0E],
        ':' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00],
        _ => [0; 7],
    }
}

/// Draw `text` onto an RGBA buffer at top-left (x, y), scaled `scale`×, clipped
/// to the image bounds. Used for baked numeric tick labels.
fn draw_text(buf: &mut [u8], w: u32, h: u32, text: &str, x: i32, y: i32, color: [u8; 4], scale: i32) {
    let mut cx = x;
    for ch in text.chars() {
        let g = glyph5x7(ch);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..5i32 {
                if (bits >> (4 - col)) & 1 == 1 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let px = cx + col * scale + dx;
                            let py = y + row as i32 * scale + dy;
                            if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                                let o = ((py as u32 * w + px as u32) * 4) as usize;
                                buf[o..o + 4].copy_from_slice(&color);
                            }
                        }
                    }
                }
            }
        }
        cx += 6 * scale; // 5px glyph + 1px gap
    }
}

/// Compact datetime tick label chosen by the visible span. Uses only digits /
/// '-' / ':' (renderable by the 5×7 font). Reuses core `format_timestamp`.
fn fmt_time_tick(ts: f64, span_secs: f64) -> String {
    let full = format_timestamp(ts);
    let base = full.split('.').next().unwrap_or(&full); // drop fractional seconds
    let mut it = base.splitn(2, ' ');
    let date = it.next().unwrap_or(base); // "YYYY-MM-DD"
    let time = it.next().unwrap_or(""); // "HH:MM:SS"
    if span_secs <= 2.0 * 86_400.0 && !time.is_empty() {
        time.to_string()
    } else if span_secs <= 400.0 * 86_400.0 {
        let md = date.get(5..).unwrap_or(date); // "MM-DD"
        let hm = time.get(0..5).unwrap_or(time); // "HH:MM"
        if hm.is_empty() {
            md.to_string()
        } else {
            format!("{md} {hm}")
        }
    } else {
        date.to_string()
    }
}

/// Format an x-axis tick: datetime-aware when `x_is_time`, else numeric.
fn fmt_x_tick(v: f64, x_is_time: bool, span: f64) -> String {
    if x_is_time {
        fmt_time_tick(v, span)
    } else {
        format_tick_value(v)
    }
}
