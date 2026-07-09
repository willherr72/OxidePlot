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
use oxideplot_core::data::loader::resolve_col;
use oxideplot_core::processing::downsampling::minmax_envelope;
use oxideplot_core::processing::expr::{apply_filter, collect_expr_cols, eval_expr, parse_expr, rolling_compute};
use oxideplot_core::processing::histogram::histogram as core_histogram;
use oxideplot_core::processing::qc::{health_check as core_health_check, longest_constant_run, Finding, Severity};
use oxideplot_core::processing::spectral::{compute_psd, compute_spectrogram, infer_sample_rate};
use oxideplot_core::processing::statistics::pearson;

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

/// Magma-like colormap: t in 0..1 -> RGB. For spectrogram intensity.
fn heat_color(t: f64) -> [u8; 3] {
    const STOPS: [(f64, [f64; 3]); 5] = [
        (0.0, [0.0, 0.0, 0.02]),
        (0.25, [0.28, 0.05, 0.35]),
        (0.5, [0.65, 0.18, 0.42]),
        (0.75, [0.95, 0.45, 0.28]),
        (1.0, [0.99, 0.87, 0.55]),
    ];
    let t = t.clamp(0.0, 1.0);
    let mut i = 0;
    while i + 1 < STOPS.len() && t > STOPS[i + 1].0 {
        i += 1;
    }
    let (t0, c0) = STOPS[i];
    let (t1, c1) = STOPS[(i + 1).min(STOPS.len() - 1)];
    let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
    [
        ((c0[0] + (c1[0] - c0[0]) * f) * 255.0) as u8,
        ((c0[1] + (c1[1] - c0[1]) * f) * 255.0) as u8,
        ((c0[2] + (c1[2] - c0[2]) * f) * 255.0) as u8,
    ]
}

/// Escape a CSV field (quote if it contains a comma, quote, or newline).
fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
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
struct CorrelateParams {
    /// Dataset id returned by load_csv.
    dataset_id: String,
    /// Numeric columns to correlate (names or indices). Omit for all numeric columns.
    #[serde(default)]
    columns: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct HealthParams {
    /// Dataset id returned by load_csv.
    dataset_id: String,
    /// Optional channel-lineage hint: maps a derived/calibrated column name to the
    /// raw source columns it is computed from (e.g. {"calibrated_az": ["raw_ax2",
    /// "raw_ay2","raw_az2"]}). When a fault appears in a derived column, it is
    /// traced back to these sources. Naming (raw…/calibrated…) is used when omitted.
    #[serde(default)]
    lineage: Option<std::collections::HashMap<String, Vec<String>>>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SegmentParams {
    /// Dataset id returned by load_csv.
    dataset_id: String,
    /// Columns (names or indices) to segment. Omit for all numeric columns.
    #[serde(default)]
    columns: Option<Vec<String>>,
    /// Number of equal row-windows to split into (default 10).
    #[serde(default)]
    segments: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct DeriveParams {
    /// Dataset id returned by load_csv.
    dataset_id: String,
    /// Operation: "magnitude" (sqrt of sum of squares), "add", "mean", "subtract"
    /// (A−B), "ratio" (A/B), or "scale" (scale·col + offset).
    op: String,
    /// Source columns (names or indices). magnitude/add/mean take one or more;
    /// subtract/ratio/rolling_corr take exactly two; scale/rolling_* take one.
    /// Omit for op "expr" (columns come from the expression).
    #[serde(default)]
    columns: Vec<String>,
    /// Name for the new column (defaults to op + source names).
    #[serde(default)]
    new_name: Option<String>,
    /// For "scale": the multiplier (default 1).
    #[serde(default)]
    scale: Option<f64>,
    /// For "scale": the offset (default 0).
    #[serde(default)]
    offset: Option<f64>,
    /// For op "expr": a formula over column names, e.g.
    /// "deg(acos(calibrated_az / total_gravity))" or "raw_ax2 - raw_ax1". Supports
    /// + - * / ^, parentheses, and sqrt/abs/sin/cos/tan/asin/acos/atan/atan2/hypot/
    /// pow/exp/ln/log10/floor/ceil/round/sign/deg/rad/min/max.
    #[serde(default)]
    expression: Option<String>,
    /// For rolling ops (rolling_mean/std/min/max, rolling_corr): the window in rows.
    #[serde(default)]
    window: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SpectrumParams {
    /// Dataset id returned by load_csv.
    dataset_id: String,
    /// Numeric column (name or index) to compute the spectrum of.
    column: String,
    /// Sample rate in Hz. Omit to infer from a datetime column (1/median dt).
    #[serde(default)]
    sample_rate: Option<f64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct HistogramParams {
    /// Dataset id returned by load_csv.
    dataset_id: String,
    /// Numeric column (name or index) to bin.
    column: String,
    /// Number of bins (default 40).
    #[serde(default)]
    bins: Option<usize>,
    /// Image width/height in px (defaults 640×360).
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SpectrogramParams {
    /// Dataset id returned by load_csv.
    dataset_id: String,
    /// Numeric column (name or index) to analyse.
    column: String,
    /// Sample rate in Hz. Omit to infer from a datetime column.
    #[serde(default)]
    sample_rate: Option<f64>,
    /// FFT window size in samples (default 256; frequency resolution = fs/window,
    /// so a larger window gives finer frequency but coarser time).
    #[serde(default)]
    window: Option<usize>,
    /// Image width/height in px (defaults 760×380).
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
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
    /// Boolean filter predicate over column names, e.g. "raw_ax2 > 1e6" or
    /// "total_gravity < 0.5 and vibe_x > 3". Keeps only rows where it is true.
    /// Operators: > < >= <= == != and/or (&&/||), + - * / ^, plus the math
    /// functions from derive_column expr.
    #[serde(default)]
    filter: Option<String>,
    /// Row offset into the (sorted/filtered) result (default 0).
    #[serde(default)]
    offset: Option<usize>,
    /// Max rows to return (default 20, capped at 200).
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ExportParams {
    /// Dataset id returned by load_csv.
    dataset_id: String,
    /// File path to write the CSV to.
    path: String,
    /// Columns to include (names or indices). Omit for all columns.
    #[serde(default)]
    columns: Option<Vec<String>>,
    /// Optional boolean filter predicate (same syntax as query_data's 'filter') —
    /// export only the matching rows.
    #[serde(default)]
    filter: Option<String>,
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
    /// Y-axis autoscale: "minmax" (default) or "robust" (clip to the 1st–99th
    /// percentile so a lone extreme outlier doesn't flatten the signal).
    #[serde(default)]
    autoscale: Option<String>,
    /// Y-axis scale: "linear" (default) or "log" (log10; non-positive values are
    /// dropped). Good for series spanning orders of magnitude.
    #[serde(default)]
    y_scale: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ReportParams {
    /// Dataset id returned by load_csv.
    dataset_id: String,
    /// Report title. Omit to default to the dataset id.
    #[serde(default)]
    title: Option<String>,
    /// Columns (names or indices) to feature in the overview plot. Omit for all
    /// numeric columns (capped at 8).
    #[serde(default)]
    columns: Option<Vec<String>>,
    /// Max number of plots to include: an overview plus one per significant
    /// finding (default 6).
    #[serde(default)]
    max_plots: Option<usize>,
    /// File path to write the HTML report to. Omit for a generated path in the
    /// system temp directory (oxideplot_report_<dataset>_<timestamp>.html).
    #[serde(default)]
    output_path: Option<String>,
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
        description = "Pearson correlation across a dataset's numeric columns. Returns the correlation matrix (when <=20 columns) plus every pair sorted by |correlation| — use it to spot a decorrelated/damaged axis (redundant sensors should be near +/-1; an unexpectedly low value flags a problem)."
    )]
    async fn correlate(
        &self,
        Parameters(CorrelateParams {
            dataset_id,
            columns,
        }): Parameters<CorrelateParams>,
    ) -> Result<CallToolResult, McpError> {
        let s = self.session.lock().await;
        let ds = s.datasets.get(&dataset_id).ok_or_else(|| {
            McpError::invalid_params(format!("unknown dataset_id '{dataset_id}'"), None)
        })?;

        let idxs: Vec<usize> = match columns {
            Some(names) => names
                .iter()
                .filter_map(|n| resolve_col(&ds.data, n))
                .filter(|&c| ds.numeric_cols[c])
                .collect(),
            None => (0..ds.data.columns.len())
                .filter(|&c| ds.numeric_cols[c])
                .collect(),
        };
        if idxs.len() < 2 {
            return Err(McpError::invalid_params(
                "need at least 2 numeric columns to correlate".to_string(),
                None,
            ));
        }

        let cols: Vec<(String, Vec<f64>)> = idxs
            .iter()
            .map(|&c| {
                (
                    ds.data.columns[c].clone(),
                    column_to_f64(&ds.data.column_data[c]).0,
                )
            })
            .collect();
        let n = cols.len();
        let round3 = |x: f64| (x * 1000.0).round() / 1000.0;

        let mut matrix = vec![vec![serde_json::Value::Null; n]; n];
        let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
        for i in 0..n {
            matrix[i][i] = json!(1.0);
            for j in (i + 1)..n {
                let r = pearson(&cols[i].1, &cols[j].1);
                let rv = r.map(round3);
                matrix[i][j] = json!(rv);
                matrix[j][i] = json!(rv);
                if let Some(x) = r {
                    pairs.push((x, i, j));
                }
            }
        }
        pairs.sort_by(|a, b| {
            b.0.abs()
                .partial_cmp(&a.0.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let pairs_json: Vec<_> = pairs
            .iter()
            .take(60)
            .map(|(r, i, j)| json!({ "a": cols[*i].0, "b": cols[*j].0, "r": round3(*r) }))
            .collect();

        let names: Vec<&String> = cols.iter().map(|c| &c.0).collect();
        let mut out = json!({ "columns": names, "pairs_by_abs_correlation": pairs_json });
        if n <= 20 {
            out["matrix"] = json!(matrix);
        }
        Ok(Self::text_result(out))
    }

    #[tool(
        description = "Per-segment statistics: split the dataset into N equal row-windows and report mean/std/min/max per column per segment, so a mid-run level shift, drift, or a channel railing out becomes visible (whole-dataset stats hide these). Omit 'columns' for all numeric."
    )]
    async fn segment_stats(
        &self,
        Parameters(SegmentParams {
            dataset_id,
            columns,
            segments,
        }): Parameters<SegmentParams>,
    ) -> Result<CallToolResult, McpError> {
        let s = self.session.lock().await;
        let ds = s.datasets.get(&dataset_id).ok_or_else(|| {
            McpError::invalid_params(format!("unknown dataset_id '{dataset_id}'"), None)
        })?;
        let n_seg = segments.unwrap_or(10).clamp(2, 500);
        let idxs: Vec<usize> = match columns {
            Some(names) => names
                .iter()
                .filter_map(|n| resolve_col(&ds.data, n))
                .filter(|&c| ds.numeric_cols[c])
                .collect(),
            None => (0..ds.data.columns.len())
                .filter(|&c| ds.numeric_cols[c])
                .collect(),
        };
        let n_rows = ds.data.row_count;
        let r4 = |x: f64| (x * 10000.0).round() / 10000.0;

        let mut out = Vec::new();
        for &c in &idxs {
            let (vals, _) = column_to_f64(&ds.data.column_data[c]);
            let mut segs = Vec::new();
            for seg in 0..n_seg {
                let lo = seg * n_rows / n_seg;
                let hi = ((seg + 1) * n_rows / n_seg).min(n_rows);
                let slice: Vec<f64> = vals
                    .get(lo..hi)
                    .unwrap_or(&[])
                    .iter()
                    .copied()
                    .filter(|v| v.is_finite())
                    .collect();
                match SeriesStats::compute(&slice) {
                    Some(st) => segs.push(json!({
                        "rows": [lo, hi],
                        "mean": r4(st.mean),
                        "std": r4(st.std_dev),
                        "min": r4(st.min),
                        "max": r4(st.max),
                    })),
                    None => segs.push(json!({ "rows": [lo, hi], "note": "no finite values" })),
                }
            }
            out.push(json!({ "column": ds.data.columns[c], "segments": segs }));
        }
        Ok(Self::text_result(json!({
            "n_segments": n_seg,
            "n_rows": n_rows,
            "columns": out,
        })))
    }

    #[tool(
        description = "One-call QC health scan of a dataset: returns a severity-ranked list of issues — dead/frozen channels, single-sample glitches (robust z-score), mid-run level shifts (changepoints, grouped into regime_change_events and traced to the likely raw source channel), time-index gaps, and missing-data clusters. Pass 'lineage' to map derived columns to their raw sources for precise attribution. The report a field engineer runs first."
    )]
    async fn health_check(
        &self,
        Parameters(HealthParams {
            dataset_id,
            lineage,
        }): Parameters<HealthParams>,
    ) -> Result<CallToolResult, McpError> {
        let s = self.session.lock().await;
        let ds = s.datasets.get(&dataset_id).ok_or_else(|| {
            McpError::invalid_params(format!("unknown dataset_id '{dataset_id}'"), None)
        })?;
        let findings = core_health_check(&ds.data, &ds.numeric_cols, lineage.as_ref());
        Ok(Self::text_result(json!({
            "dataset_id": dataset_id,
            "n_rows": ds.data.row_count,
            "n_findings": findings.len(),
            "findings": serde_json::to_value(&findings).unwrap(),
        })))
    }

    #[tool(
        description = "Add a computed column to a dataset. op: 'magnitude' (sqrt of sum of squares), 'add'/'mean' (>=1 cols), 'subtract'(A-B)/'ratio'(A/B) (2 cols), 'scale' (scale*col+offset, 1 col), 'rolling_mean'/'rolling_std'/'rolling_min'/'rolling_max' (1 col + 'window'), 'rolling_corr' (2 cols + 'window'), or 'expr' (a free-form 'expression' over column names, e.g. deg(acos(calibrated_az/total_gravity)) — for recomputing survey math / independent checks). The new column is usable by describe_data/query_data/create_graph/correlate."
    )]
    async fn derive_column(
        &self,
        Parameters(DeriveParams {
            dataset_id,
            op,
            columns,
            new_name,
            scale,
            offset,
            expression,
            window,
        }): Parameters<DeriveParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut s = self.session.lock().await;
        let ds = s.datasets.get_mut(&dataset_id).ok_or_else(|| {
            McpError::invalid_params(format!("unknown dataset_id '{dataset_id}'"), None)
        })?;
        let n_rows = ds.data.row_count;

        let vals: Vec<f64> = if op == "expr" {
            // Free-form formula over column names.
            let formula = expression.as_deref().ok_or_else(|| {
                McpError::invalid_params("op 'expr' needs an 'expression'".to_string(), None)
            })?;
            let ast = parse_expr(&ds.data, formula)
                .map_err(|e| McpError::invalid_params(format!("expression error: {e}"), None))?;
            let mut refs = std::collections::HashSet::new();
            collect_expr_cols(&ast, &mut refs);
            let colvals: std::collections::HashMap<usize, Vec<f64>> = refs
                .iter()
                .map(|&ci| (ci, column_to_f64(&ds.data.column_data[ci]).0))
                .collect();
            (0..n_rows).map(|r| eval_expr(&ast, &colvals, r)).collect()
        } else {
            let mut idxs = Vec::new();
            for c in &columns {
                idxs.push(resolve_col(&ds.data, c).ok_or_else(|| {
                    McpError::invalid_params(format!("unknown column '{c}'"), None)
                })?);
            }
            let one = ["scale", "rolling_mean", "rolling_std", "rolling_min", "rolling_max"];
            match op.as_str() {
                "subtract" | "ratio" | "rolling_corr" if idxs.len() != 2 => {
                    return Err(McpError::invalid_params(
                        format!("op '{op}' needs exactly 2 columns"),
                        None,
                    ))
                }
                o if one.contains(&o) && idxs.len() != 1 => {
                    return Err(McpError::invalid_params(
                        format!("op '{op}' needs exactly 1 column"),
                        None,
                    ))
                }
                "magnitude" | "add" | "mean" if idxs.is_empty() => {
                    return Err(McpError::invalid_params(
                        format!("op '{op}' needs at least 1 column"),
                        None,
                    ))
                }
                "magnitude" | "add" | "mean" | "subtract" | "ratio" | "scale" | "rolling_mean"
                | "rolling_std" | "rolling_min" | "rolling_max" | "rolling_corr" => {}
                other => {
                    return Err(McpError::invalid_params(
                        format!("unknown op '{other}'"),
                        None,
                    ))
                }
            }
            let cols: Vec<Vec<f64>> = idxs
                .iter()
                .map(|&c| column_to_f64(&ds.data.column_data[c]).0)
                .collect();

            if op.starts_with("rolling_") {
                let win = window.unwrap_or(0);
                if win < 2 {
                    return Err(McpError::invalid_params(
                        format!("op '{op}' needs a 'window' of at least 2"),
                        None,
                    ));
                }
                rolling_compute(&op, &cols, win, n_rows)
            } else {
                let a = scale.unwrap_or(1.0);
                let b = offset.unwrap_or(0.0);
                (0..n_rows)
                    .map(|r| {
                        let g = |k: usize| cols[k].get(r).copied().unwrap_or(f64::NAN);
                        match op.as_str() {
                            "magnitude" => (0..cols.len()).map(|k| g(k) * g(k)).sum::<f64>().sqrt(),
                            "add" => (0..cols.len()).map(g).sum(),
                            "mean" => (0..cols.len()).map(g).sum::<f64>() / cols.len() as f64,
                            "subtract" => g(0) - g(1),
                            "ratio" => g(0) / g(1),
                            "scale" => a * g(0) + b,
                            _ => f64::NAN,
                        }
                    })
                    .collect()
            }
        };

        let name = new_name.unwrap_or_else(|| {
            if op == "expr" {
                format!("expr_{}", ds.data.columns.len())
            } else {
                format!("{op}_{}", columns.join("_"))
            }
        });
        let cells: Vec<String> = vals
            .iter()
            .map(|v| if v.is_finite() { format!("{v}") } else { String::new() })
            .collect();
        ds.data.columns.push(name.clone());
        ds.data.column_data.push(cells);
        ds.numeric_cols.push(true);
        let new_index = ds.data.columns.len() - 1;
        let st = SeriesStats::compute(&vals);

        Ok(Self::text_result(json!({
            "dataset_id": dataset_id,
            "new_column": name,
            "index": new_index,
            "op": op,
            "from": if op == "expr" { json!(expression) } else { json!(columns) },
            "n_rows": n_rows,
            "min": st.as_ref().map(|s| s.min),
            "max": st.as_ref().map(|s| s.max),
            "mean": st.as_ref().map(|s| s.mean),
        })))
    }

    #[tool(
        description = "Power spectral density (FFT) of a numeric column — for vibration/shock data whose signatures (stick-slip, whirl, bit-bounce) live in the frequency domain. Stores the PSD as a new dataset (columns: frequency, power) and returns the dominant peak frequencies. Sample rate is inferred from a datetime column unless given."
    )]
    async fn spectrum(
        &self,
        Parameters(SpectrumParams {
            dataset_id,
            column,
            sample_rate,
        }): Parameters<SpectrumParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut s = self.session.lock().await;
        let (freqs, power, fs, colname) = {
            let ds = s.datasets.get(&dataset_id).ok_or_else(|| {
                McpError::invalid_params(format!("unknown dataset_id '{dataset_id}'"), None)
            })?;
            let ci = resolve_col(&ds.data, &column).ok_or_else(|| {
                McpError::invalid_params(format!("unknown column '{column}'"), None)
            })?;
            if !ds.numeric_cols[ci] {
                return Err(McpError::invalid_params(
                    format!("column '{}' is not numeric", ds.data.columns[ci]),
                    None,
                ));
            }
            let fs = sample_rate.unwrap_or_else(|| infer_sample_rate(&ds.data));
            let (vals, _) = column_to_f64(&ds.data.column_data[ci]);
            let (freqs, power) = compute_psd(&vals, fs);
            (freqs, power, fs, ds.data.columns[ci].clone())
        };
        if freqs.is_empty() {
            return Err(McpError::internal_error(
                "too few finite samples for a spectrum".to_string(),
                None,
            ));
        }

        let mut order: Vec<usize> = (0..power.len()).collect();
        order.sort_by(|&a, &b| {
            power[b]
                .partial_cmp(&power[a])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let peaks: Vec<serde_json::Value> = order
            .iter()
            .take(6)
            .map(|&i| json!({ "frequency_hz": (freqs[i]*1000.0).round()/1000.0, "power": power[i] }))
            .collect();

        let n = freqs.len();
        let ld = LoadedData {
            columns: vec!["frequency".to_string(), "power".to_string()],
            column_data: vec![
                freqs.iter().map(|v| format!("{v}")).collect(),
                power.iter().map(|v| format!("{v}")).collect(),
            ],
            row_count: n,
        };
        let id = s.new_id("ds");
        s.datasets.insert(
            id.clone(),
            Dataset {
                data: ld,
                numeric_cols: vec![true, true],
            },
        );
        Ok(Self::text_result(json!({
            "dataset_id": id,
            "of_column": colname,
            "sample_rate_hz": fs,
            "n_bins": n,
            "freq_max_hz": freqs.last().copied(),
            "peaks": peaks,
            "note": "PSD stored as a new dataset (columns: frequency, power). Render with create_graph x=frequency y=power, then render_graph y_scale=log. 'peaks' are the dominant frequencies.",
        })))
    }

    #[tool(
        description = "Spectrogram (STFT): a frequency-vs-time heatmap PNG of a numeric column (Y=frequency 0..Nyquist, X=time, colour=intensity). Shows how the frequency content evolves over the run — a band that shifts vertically means changing resonance. Sample rate inferred from a datetime column unless given."
    )]
    async fn spectrogram(
        &self,
        Parameters(SpectrogramParams {
            dataset_id,
            column,
            sample_rate,
            window,
            width,
            height,
        }): Parameters<SpectrogramParams>,
    ) -> Result<CallToolResult, McpError> {
        let (vals, fs, colname) = {
            let s = self.session.lock().await;
            let ds = s.datasets.get(&dataset_id).ok_or_else(|| {
                McpError::invalid_params(format!("unknown dataset_id '{dataset_id}'"), None)
            })?;
            let ci = resolve_col(&ds.data, &column).ok_or_else(|| {
                McpError::invalid_params(format!("unknown column '{column}'"), None)
            })?;
            if !ds.numeric_cols[ci] {
                return Err(McpError::invalid_params(
                    format!("column '{}' is not numeric", ds.data.columns[ci]),
                    None,
                ));
            }
            let fs = sample_rate.unwrap_or_else(|| infer_sample_rate(&ds.data));
            (
                column_to_f64(&ds.data.column_data[ci]).0,
                fs,
                ds.data.columns[ci].clone(),
            )
        };

        let win = window.unwrap_or(256).clamp(16, 4096);
        let w = width.unwrap_or(760).clamp(200, 2000);
        let h = height.unwrap_or(380).clamp(150, 1200);

        let (frames, bins) =
            tokio::task::spawn_blocking(move || compute_spectrogram(&vals, win))
                .await
                .map_err(|e| McpError::internal_error(format!("stft task failed: {e}"), None))?;
        if frames.is_empty() || bins == 0 {
            return Err(McpError::internal_error(
                format!("need at least {win} finite samples for a spectrogram"),
                None,
            ));
        }
        let n_frames = frames.len();

        // Colour range from the 5th–99.5th percentile of log-magnitude (contrast).
        let mut logs: Vec<f64> = frames
            .iter()
            .flat_map(|f| f.iter().map(|&m| (m + 1e-12).log10()))
            .collect();
        logs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let lo = logs[(logs.len() as f64 * 0.05) as usize];
        let hi = logs[((logs.len() - 1) as f64 * 0.995) as usize];
        let span = (hi - lo).max(1e-9);

        let mut buf = vec![0u8; (w * h * 4) as usize];
        for py in 0..h {
            let bin = (((h - 1 - py) as usize) * bins / h as usize).min(bins - 1);
            for px in 0..w {
                let fr = (px as usize * n_frames / w as usize).min(n_frames - 1);
                let t = ((frames[fr][bin] + 1e-12).log10() - lo) / span;
                let [r, g, b] = heat_color(t);
                let o = ((py * w + px) * 4) as usize;
                buf[o] = r;
                buf[o + 1] = g;
                buf[o + 2] = b;
                buf[o + 3] = 255;
            }
        }

        // Baked axes: frequency (Y, 0..Nyquist) and time (X, seconds).
        let nyq = fs / 2.0;
        let total_time = n_frames as f64 * (win as f64 / 2.0) / fs;
        let lc = [235u8, 235, 240, 255];
        for k in 0..=4 {
            let f = nyq * k as f64 / 4.0;
            let py = ((1.0 - f / nyq.max(1e-9)) * h as f64) as i32 - 7;
            draw_text(&mut buf, w, h, &format!("{f:.0}"), 3, py.clamp(1, h as i32 - 9), lc, 1);
            let tt = total_time * k as f64 / 4.0;
            let px = (w as f64 * k as f64 / 4.0) as i32;
            draw_text(
                &mut buf,
                w,
                h,
                &format!("{tt:.0}"),
                (px + 2).min(w as i32 - 26),
                h as i32 - 11,
                lc,
                1,
            );
        }

        let img = image::RgbaImage::from_raw(w, h, buf).ok_or_else(|| {
            McpError::internal_error("spectrogram buffer mis-sized".to_string(), None)
        })?;
        let mut png: Vec<u8> = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| McpError::internal_error(format!("PNG encode failed: {e}"), None))?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);

        let text = json!({
            "of_column": colname,
            "sample_rate_hz": fs,
            "window_samples": win,
            "n_frames": n_frames,
            "freq_bins": bins,
            "freq_range_hz": [0.0, nyq],
            "time_range_s": [0.0, total_time],
            "note": "Heatmap: Y=frequency (0 bottom → Nyquist top), X=time (s), colour=intensity (dark=low, bright=high). A band shifting vertically over time = changing resonance.",
        })
        .to_string();
        Ok(CallToolResult::success(vec![
            Content::image(b64, "image/png".to_string()),
            Content::text(text),
        ]))
    }

    #[tool(
        description = "Histogram of a numeric column — a distribution bar-chart PNG plus the bin counts. Reveals bimodality, rail-pinning/saturation, and clustering that a time-series line plot buries."
    )]
    async fn histogram(
        &self,
        Parameters(HistogramParams {
            dataset_id,
            column,
            bins,
            width,
            height,
        }): Parameters<HistogramParams>,
    ) -> Result<CallToolResult, McpError> {
        let (vals, colname) = {
            let s = self.session.lock().await;
            let ds = s.datasets.get(&dataset_id).ok_or_else(|| {
                McpError::invalid_params(format!("unknown dataset_id '{dataset_id}'"), None)
            })?;
            let ci = resolve_col(&ds.data, &column).ok_or_else(|| {
                McpError::invalid_params(format!("unknown column '{column}'"), None)
            })?;
            if !ds.numeric_cols[ci] {
                return Err(McpError::invalid_params(
                    format!("column '{}' is not numeric", ds.data.columns[ci]),
                    None,
                ));
            }
            (
                column_to_f64(&ds.data.column_data[ci]).0,
                ds.data.columns[ci].clone(),
            )
        };
        let nbins = bins.unwrap_or(40).clamp(2, 200);
        let w = width.unwrap_or(640).clamp(200, 2000);
        let h = height.unwrap_or(360).clamp(150, 1200);
        let hist = core_histogram(&vals, nbins).ok_or_else(|| {
            McpError::internal_error(
                "need at least 2 finite values for a histogram".to_string(),
                None,
            )
        })?;
        let counts = hist.counts;
        let vmin = hist.min;
        let vmax = hist.max;
        let n = hist.n;
        let maxc = (*counts.iter().max().unwrap_or(&1)).max(1);

        let bg = [14u8, 15, 19];
        let barc = [235u8, 170, 70];
        let lc = [200u8, 205, 215, 255];
        let mut buf = vec![0u8; (w * h * 4) as usize];
        for px in 0..(w * h) as usize {
            let o = px * 4;
            buf[o] = bg[0];
            buf[o + 1] = bg[1];
            buf[o + 2] = bg[2];
            buf[o + 3] = 255;
        }
        let (ml, mr, mt, mb) = (46u32, 10u32, 12u32, 22u32);
        let plot_w = w.saturating_sub(ml + mr).max(1);
        let plot_h = h.saturating_sub(mt + mb).max(1);
        for bi in 0..nbins {
            let bar_h = (counts[bi] as f64 / maxc as f64 * plot_h as f64) as u32;
            let x0 = ml + (bi as u32 * plot_w / nbins as u32);
            let x1 = ml + ((bi as u32 + 1) * plot_w / nbins as u32);
            let ytop = mt + plot_h - bar_h;
            for py in ytop..(mt + plot_h) {
                for px in x0..x1.min(w) {
                    let o = ((py * w + px) * 4) as usize;
                    buf[o] = barc[0];
                    buf[o + 1] = barc[1];
                    buf[o + 2] = barc[2];
                    buf[o + 3] = 255;
                }
            }
        }
        // Baked labels: x at min/mid/max, y at 0/maxcount.
        draw_text(&mut buf, w, h, &format_tick_value(vmin), ml as i32, (h - mb + 4) as i32, lc, 1);
        draw_text(
            &mut buf,
            w,
            h,
            &format_tick_value((vmin + vmax) / 2.0),
            (ml + plot_w / 2) as i32 - 12,
            (h - mb + 4) as i32,
            lc,
            1,
        );
        draw_text(
            &mut buf,
            w,
            h,
            &format_tick_value(vmax),
            (w - mr) as i32 - 30,
            (h - mb + 4) as i32,
            lc,
            1,
        );
        draw_text(&mut buf, w, h, &format!("{maxc}"), 2, mt as i32, lc, 1);
        draw_text(&mut buf, w, h, "0", 2, (mt + plot_h) as i32 - 6, lc, 1);

        let img = image::RgbaImage::from_raw(w, h, buf).ok_or_else(|| {
            McpError::internal_error("histogram buffer mis-sized".to_string(), None)
        })?;
        let mut png: Vec<u8> = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| McpError::internal_error(format!("PNG encode failed: {e}"), None))?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);

        let centers: Vec<f64> = hist
            .bin_centers
            .iter()
            .map(|v| (v * 1000.0).round() / 1000.0)
            .collect();
        let modal = counts
            .iter()
            .enumerate()
            .max_by_key(|(_, c)| **c)
            .map(|(i, _)| centers[i]);
        let text = json!({
            "of_column": colname,
            "n": n,
            "min": vmin,
            "max": vmax,
            "bins": nbins,
            "counts": counts,
            "bin_centers": centers,
            "modal_bin_center": modal,
            "note": "Value-distribution bars. Multiple separated peaks = multimodal; a tall spike at the min or max edge = rail-pinning/saturation.",
        })
        .to_string();
        Ok(CallToolResult::success(vec![
            Content::image(b64, "image/png".to_string()),
            Content::text(text),
        ]))
    }

    #[tool(
        description = "Return a page of raw rows from a dataset, with optional sort (by column name), case-insensitive search, a boolean 'filter' predicate (e.g. \"raw_ax2 > 1e6 and total_gravity < 0.5\") to pull just the matching rows, and paging (offset/limit)."
    )]
    async fn query_data(
        &self,
        Parameters(QueryParams {
            dataset_id,
            sort_col,
            sort_desc,
            search,
            filter,
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

        let mut idx = compute_view_index(&ds.data, &q);
        if let Some(pred) = filter.as_deref() {
            idx = apply_filter(&ds.data, &idx, pred)
                .map_err(|e| McpError::invalid_params(format!("filter error: {e}"), None))?;
        }
        let total = idx.len();
        let start = offset.unwrap_or(0);
        let count = limit.unwrap_or(20).min(200);
        let rows = window_rows(&ds.data, &idx, start, count, None);

        Ok(Self::text_result(json!({
            "total": total,
            "offset": start,
            "returned": rows.len(),
            "columns": ds.data.columns,
            "rows": rows,
        })))
    }

    #[tool(
        description = "Export a dataset to a CSV file on disk, optionally a column subset and/or only rows matching a boolean 'filter' predicate (same syntax as query_data). Use to hand off flagged rows or a derived/filtered table as a file. Returns the path and rows written."
    )]
    async fn export_csv(
        &self,
        Parameters(ExportParams {
            dataset_id,
            path,
            columns,
            filter,
        }): Parameters<ExportParams>,
    ) -> Result<CallToolResult, McpError> {
        let s = self.session.lock().await;
        let ds = s.datasets.get(&dataset_id).ok_or_else(|| {
            McpError::invalid_params(format!("unknown dataset_id '{dataset_id}'"), None)
        })?;

        let cidx: Vec<usize> = match &columns {
            Some(names) => {
                let mut v = Vec::new();
                for n in names {
                    v.push(resolve_col(&ds.data, n).ok_or_else(|| {
                        McpError::invalid_params(format!("unknown column '{n}'"), None)
                    })?);
                }
                v
            }
            None => (0..ds.data.columns.len()).collect(),
        };

        let all: Vec<usize> = (0..ds.data.row_count).collect();
        let rows = match filter.as_deref() {
            Some(pred) => apply_filter(&ds.data, &all, pred)
                .map_err(|e| McpError::invalid_params(format!("filter error: {e}"), None))?,
            None => all,
        };

        let mut out = String::new();
        out.push_str(
            &cidx
                .iter()
                .map(|&c| csv_escape(&ds.data.columns[c]))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
        for &r in &rows {
            let line: Vec<String> = cidx
                .iter()
                .map(|&c| csv_escape(ds.data.column_data[c].get(r).map(|s| s.as_str()).unwrap_or("")))
                .collect();
            out.push_str(&line.join(","));
            out.push('\n');
        }
        std::fs::write(&path, out)
            .map_err(|e| McpError::internal_error(format!("failed to write '{path}': {e}"), None))?;

        Ok(Self::text_result(json!({
            "path": path,
            "rows_written": rows.len(),
            "columns": cidx.iter().map(|&c| &ds.data.columns[c]).collect::<Vec<_>>(),
            "filtered": filter.is_some(),
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
            autoscale,
            y_scale,
        }): Parameters<RenderGraphParams>,
    ) -> Result<CallToolResult, McpError> {
        // Build per-panel render inputs under the lock; render without holding it.
        let (prep, text) = {
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
            // Effective spec: the stored graph with this render's layout/transform
            // overrides folded in (GraphSpec itself carries no window/downsample
            // state — that lives in RenderWindow below).
            let eff_spec = GraphSpec {
                dataset_id: g.dataset_id.clone(),
                x_col: g.x_col,
                y_cols: g.y_cols.clone(),
                draw_mode: g.draw_mode,
                layout: lay,
                transform: tf,
                title: g.title.clone(),
            };
            let win = RenderWindow {
                row_start,
                row_end,
                x_min,
                x_max,
                downsample: downsample.clone(),
                robust: autoscale.as_deref() == Some("robust"),
                y_log: y_scale.as_deref() == Some("log"),
            };

            let prep = prepare_render(&ds.data, &eff_spec, w, h, &win)
                .map_err(|e| McpError::internal_error(e, None))?;

            // Text companion.
            let x_ticks: Vec<String> = compute_grid_lines(prep.x_view.0, prep.x_view.1)
                .iter()
                .filter(|(_, m)| *m)
                .map(|(v, _)| {
                    if prep.x_is_time {
                        format_timestamp(*v)
                    } else {
                        format_tick_value(*v)
                    }
                })
                .collect();
            let x_range_json = if prep.x_is_time {
                json!([format_timestamp(prep.xmin), format_timestamp(prep.xmax)])
            } else {
                json!([prep.xmin, prep.xmax])
            };
            let legend: Vec<serde_json::Value> = prep
                .legend
                .iter()
                .map(|(name, c, ymin, ymax)| {
                    json!({
                        "series": name,
                        "y_range": [ymin, ymax],
                        "color_rgb": [(c[0]*255.0) as u8, (c[1]*255.0) as u8, (c[2]*255.0) as u8],
                    })
                })
                .collect();
            let note = match lay {
                Layout::Stacked => "Stacked: each series in its own panel (top→bottom) with its own Y axis, sharing X.",
                Layout::Normalized => "Normalized: each series rescaled to 0..1 so shapes are comparable regardless of scale.",
                Layout::Overlay => "Overlay: all series share one Y axis (a large-scale series can dwarf a small one).",
            };
            let ds_mode = downsample.as_deref().unwrap_or("minmax");
            let robust = autoscale.as_deref() == Some("robust");
            let text = json!({
                "title": eff_spec.title.clone(),
                "layout": lay.as_str(),
                "transform": tf.label(),
                "x_axis": ds.data.columns[g.x_col].clone(),
                "x_is_time": prep.x_is_time,
                "x_range": x_range_json,
                "x_ticks": x_ticks,
                "series": legend,
                "size": [prep.panel_w, prep.out_h],
                "points_per_series": prep.max_raw_points,
                "downsampled_for_render": prep.any_downsampled,
                "downsample": ds_mode,
                "autoscale": if robust { "robust" } else { "minmax" },
                "y_scale": if prep.y_is_log { "log" } else { "linear" },
                "window": { "row_start": row_start, "row_end": row_end, "x_min": x_min, "x_max": x_max },
                "note": note,
            })
            .to_string();

            (prep, text)
        };

        let (panel_w, out_h) = (prep.panel_w, prep.out_h);
        let rgba = render_rgba(prep)
            .await
            .map_err(|e| McpError::internal_error(e, None))?;

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

    #[tool(
        description = "Generate the deliverable of a tool-test: a single self-contained HTML QC report for a dataset — health_check findings (with a verdict banner), auto-selected plots (an overview of the numeric columns plus one focused plot per significant finding), and a flagged-rows table — written to disk ready to hand off. Fully automatic by default; 'columns'/'max_plots'/'output_path' are optional overrides."
    )]
    async fn report(
        &self,
        Parameters(ReportParams {
            dataset_id,
            title,
            columns,
            max_plots,
            output_path,
        }): Parameters<ReportParams>,
    ) -> Result<CallToolResult, McpError> {
        let max_plots = max_plots.unwrap_or(6).clamp(1, 20);

        let (findings, data_summary, plot_specs, flagged_cols, flagged_rows, total_flagged) = {
            let s = self.session.lock().await;
            let ds = s.datasets.get(&dataset_id).ok_or_else(|| {
                McpError::invalid_params(format!("unknown dataset_id '{dataset_id}'"), None)
            })?;

            let findings = core_health_check(&ds.data, &ds.numeric_cols, None);

            // --- Dataset summary ---
            let n_rows = ds.data.row_count;
            let n_cols = ds.data.columns.len();
            let n_numeric = ds.numeric_cols.iter().filter(|&&b| b).count();
            let dt_col = (0..ds.data.columns.len()).find(|&c| {
                column_to_timestamps(&ds.data.column_data[c])
                    .map(|(_, f)| f >= 0.5)
                    .unwrap_or(false)
            });
            let time_range = dt_col.and_then(|c| {
                let (ts, _) = column_to_timestamps(&ds.data.column_data[c])?;
                let finite: Vec<f64> = ts.iter().copied().filter(|v| v.is_finite()).collect();
                if finite.is_empty() {
                    return None;
                }
                let mn = finite.iter().cloned().fold(f64::INFINITY, f64::min);
                let mx = finite.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                Some((format_timestamp(mn), format_timestamp(mx)))
            });
            let data_summary = json!({
                "n_rows": n_rows,
                "n_columns": n_cols,
                "n_numeric_columns": n_numeric,
                "time_range": time_range.as_ref().map(|(a, b)| json!({ "start": a, "end": b })),
            });

            // --- Auto-select plots (capped at max_plots) ---
            // X: first datetime column if any, else the first numeric column.
            let x_col = dt_col.or_else(|| (0..ds.data.columns.len()).find(|&c| ds.numeric_cols[c]));
            let mut plot_specs: Vec<(String, GraphSpec)> = Vec::new();
            if let Some(x) = x_col {
                let overview_y: Vec<usize> = match &columns {
                    Some(names) => names
                        .iter()
                        .filter_map(|n| resolve_col(&ds.data, n))
                        .filter(|&c| ds.numeric_cols[c])
                        .collect(),
                    None => (0..ds.data.columns.len())
                        .filter(|&c| ds.numeric_cols[c] && Some(c) != dt_col)
                        .take(8)
                        .collect(),
                };
                if !overview_y.is_empty() {
                    let lay = if overview_y.len() > 3 { Layout::Stacked } else { Layout::Overlay };
                    plot_specs.push((
                        "Overview".to_string(),
                        GraphSpec {
                            dataset_id: dataset_id.clone(),
                            x_col: x,
                            y_cols: overview_y,
                            draw_mode: DrawMode::Lines,
                            layout: lay,
                            transform: Transform::None,
                            title: Some("Overview".to_string()),
                        },
                    ));
                }

                // One focused plot per significant finding that names a column
                // (changepoint / regime_change_event / glitch / outlier_regime),
                // de-duplicated by column, most severe first (findings are
                // already severity-sorted).
                let mut seen_cols: std::collections::HashSet<String> = std::collections::HashSet::new();
                for f in &findings {
                    if plot_specs.len() >= max_plots {
                        break;
                    }
                    if !matches!(
                        f.kind.as_str(),
                        "changepoint" | "regime_change_event" | "glitch" | "outlier_regime"
                    ) {
                        continue;
                    }
                    let Some(col_name) = f
                        .column
                        .clone()
                        .or_else(|| f.affected.as_ref().and_then(|a| a.first().cloned()))
                    else {
                        continue;
                    };
                    if !seen_cols.insert(col_name.clone()) {
                        continue;
                    }
                    let Some(ci) = resolve_col(&ds.data, &col_name) else {
                        continue;
                    };
                    if !ds.numeric_cols[ci] || Some(ci) == Some(x) {
                        continue;
                    }
                    let row_ref = f
                        .row
                        .or(f.onset_row)
                        .or_else(|| f.rows.as_ref().and_then(|r| r.first()).map(|r| r[0]));
                    let caption = match row_ref {
                        Some(r) => format!("{} — {} (row ~{r})", f.kind, col_name),
                        None => format!("{} — {}", f.kind, col_name),
                    };
                    plot_specs.push((
                        caption,
                        GraphSpec {
                            dataset_id: dataset_id.clone(),
                            x_col: x,
                            y_cols: vec![ci],
                            draw_mode: DrawMode::Lines,
                            layout: Layout::Overlay,
                            transform: Transform::None,
                            title: Some(col_name),
                        },
                    ));
                }
                plot_specs.truncate(max_plots);
            }

            // --- Flagged rows: collect from findings' row/rows/onset_row ---
            const MAX_TRACKED_FLAGGED: usize = 5000;
            let mut flagged_row_set: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
            let mut flagged_col_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            for f in &findings {
                if let Some(col) = &f.column {
                    flagged_col_set.insert(col.clone());
                }
                if let Some(affected) = &f.affected {
                    for a in affected {
                        flagged_col_set.insert(a.clone());
                    }
                }
                if let Some(r) = f.row {
                    flagged_row_set.insert(r);
                }
                if let Some(r) = f.onset_row {
                    flagged_row_set.insert(r);
                }
                if let Some(r) = f.first_gap_row {
                    flagged_row_set.insert(r);
                }
                if let Some(ranges) = &f.rows {
                    'ranges: for rg in ranges {
                        for r in rg[0]..=rg[1] {
                            if flagged_row_set.len() >= MAX_TRACKED_FLAGGED {
                                break 'ranges;
                            }
                            flagged_row_set.insert(r);
                        }
                    }
                }
            }
            let total_flagged = flagged_row_set.len();
            let mut display_cols: Vec<usize> = if flagged_col_set.is_empty() {
                (0..ds.data.columns.len()).filter(|&c| ds.numeric_cols[c]).take(6).collect()
            } else {
                flagged_col_set.iter().filter_map(|n| resolve_col(&ds.data, n)).collect()
            };
            display_cols.truncate(8);
            let flagged_col_names: Vec<String> =
                display_cols.iter().map(|&c| ds.data.columns[c].clone()).collect();
            let flagged_rows: Vec<(usize, Vec<String>)> = flagged_row_set
                .iter()
                .take(50)
                .map(|&r| {
                    let vals = display_cols
                        .iter()
                        .map(|&c| ds.data.column_data[c].get(r).cloned().unwrap_or_default())
                        .collect();
                    (r, vals)
                })
                .collect();

            (findings, data_summary, plot_specs, flagged_col_names, flagged_rows, total_flagged)
        };

        // --- Render the auto-selected plots (off the lock; re-lock briefly per
        // plot only for the CPU-side prepare step — GPU work never holds it). ---
        let mut plots: Vec<(String, String)> = Vec::new();
        let mut plot_errors: Vec<String> = Vec::new();
        for (caption, spec) in &plot_specs {
            let h: u32 = if spec.layout == Layout::Stacked {
                (spec.y_cols.len() as u32 * 130).clamp(280, 900)
            } else {
                340
            };
            let png = {
                let s = self.session.lock().await;
                let ds = s.datasets.get(&dataset_id).ok_or_else(|| {
                    McpError::invalid_params(format!("unknown dataset_id '{dataset_id}'"), None)
                })?;
                render_spec_to_png(&ds.data, spec, 880, h).await
            };
            match png {
                Ok(bytes) => plots.push((caption.clone(), base64::engine::general_purpose::STANDARD.encode(&bytes))),
                Err(e) => plot_errors.push(format!("{caption}: {e}")),
            }
        }

        // --- Assemble the self-contained HTML report ---
        let report_title = title.unwrap_or_else(|| dataset_id.clone());
        let generated_at = format_timestamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
        );
        let html = build_report_html(
            &report_title,
            &generated_at,
            &data_summary,
            &findings,
            &plots,
            &flagged_cols,
            &flagged_rows,
            total_flagged,
        );

        // --- Write to disk ---
        let out_path = match output_path {
            Some(p) => std::path::PathBuf::from(p),
            None => {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let safe_id: String = dataset_id
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect();
                std::env::temp_dir().join(format!("oxideplot_report_{safe_id}_{ts}.html"))
            }
        };
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    McpError::internal_error(
                        format!("failed to create directory '{}': {e}", parent.display()),
                        None,
                    )
                })?;
            }
        }
        std::fs::write(&out_path, &html).map_err(|e| {
            McpError::internal_error(format!("failed to write '{}': {e}", out_path.display()), None)
        })?;
        let abs_path = if out_path.is_absolute() {
            out_path.clone()
        } else {
            std::env::current_dir().map(|cwd| cwd.join(&out_path)).unwrap_or(out_path.clone())
        };

        let n_high = findings.iter().filter(|f| matches!(f.severity, Severity::High)).count();
        let n_medium = findings.iter().filter(|f| matches!(f.severity, Severity::Medium)).count();
        let n_low = findings.iter().filter(|f| matches!(f.severity, Severity::Low)).count();

        Ok(Self::text_result(json!({
            "report_path": abs_path.display().to_string(),
            "n_findings": findings.len(),
            "n_plots": plots.len(),
            "severities": { "high": n_high, "medium": n_medium, "low": n_low },
            "flagged_rows_total": total_flagged,
            "plot_errors": plot_errors,
            "note": "Open this HTML file to view/hand off the report.",
        })))
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
    x_origin: f64,
    w: u32,
    h: u32,
    draw_mode: DrawMode,
    normalize: bool,
    robust: bool,
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
        // Min/max over the panel, plus (if robust) a 1st–99th-percentile view
        // so a lone extreme outlier doesn't flatten the signal.
        let (mut mn, mut mx) = (f64::INFINITY, f64::NEG_INFINITY);
        for sd in series {
            if sd.ymin.is_finite() {
                mn = mn.min(sd.ymin);
            }
            if sd.ymax.is_finite() {
                mx = mx.max(sd.ymax);
            }
        }
        let (mut ymn, mut ymx) = if robust {
            let mut ys: Vec<f32> = series
                .iter()
                .flat_map(|sd| sd.points.iter().map(|p| p[1]))
                .collect();
            if ys.len() >= 20 {
                ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let q = |f: f64| ys[(((ys.len() - 1) as f64) * f).round() as usize] as f64;
                (q(0.01), q(0.99))
            } else {
                (mn, mx)
            }
        } else {
            (mn, mx)
        };
        // Degenerate guard: fall back to min/max, then to a unit range.
        if !ymn.is_finite() || !ymx.is_finite() || ymx <= ymn {
            if mn.is_finite() && mx.is_finite() && mx > mn {
                ymn = mn;
                ymx = mx;
            } else {
                ymn = 0.0;
                ymx = 1.0;
            }
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
    // Grid vertices share the render-space X offset (x_origin) with the series
    // points and the uniforms below, so the grid aligns with the data.
    let mut segs: Vec<[f32; 2]> = Vec::new();
    for (xv, _) in &x_ticks {
        segs.push([(*xv - x_origin) as f32, y_view.0 as f32]);
        segs.push([(*xv - x_origin) as f32, y_view.1 as f32]);
    }
    for (yv, _) in &y_ticks {
        segs.push([(x_view.0 - x_origin) as f32, *yv as f32]);
        segs.push([(x_view.1 - x_origin) as f32, *yv as f32]);
    }
    let grid = GridGpuData {
        segments: segs,
        color: [0.45, 0.47, 0.55, 0.22],
        line_width: 1.0,
    };
    let uniforms = PlotUniforms {
        view_min: [(x_view.0 - x_origin) as f32, y_view.0 as f32],
        view_max: [(x_view.1 - x_origin) as f32, y_view.1 as f32],
        resolution: [w as f32, h as f32],
        line_width: 2.0,
        point_radius: 3.0,
        color: [0.0, 0.0, 0.0, 0.0],
        _padding: [0.0; 4],
    };
    (gpu_series, grid, uniforms)
}

/// Windowing/downsample/scale options for a single render, layered on top of a
/// `GraphSpec`'s own layout/transform. `RenderWindow::default()` renders the
/// full series with the standard minmax downsample/autoscale and linear Y —
/// what `report`'s auto-selected plots use; `render_graph` threads its
/// override params through here.
#[derive(Default)]
struct RenderWindow {
    row_start: Option<usize>,
    row_end: Option<usize>,
    x_min: Option<f64>,
    x_max: Option<f64>,
    downsample: Option<String>,
    robust: bool,
    y_log: bool,
}

/// Prepared per-panel render inputs for a `GraphSpec`, built while the caller
/// still holds whatever lock guards the source data — nothing here borrows
/// `LoadedData`, so it safely outlives the lock. Also carries the raw
/// ingredients `render_graph` needs for its text companion (JSON assembly
/// stays in the tool, per its existing contract).
struct RenderPrep {
    panels: Vec<(Vec<SeriesGpuData>, GridGpuData, PlotUniforms)>,
    panel_w: u32,
    panel_h: u32,
    out_h: u32,
    x_is_time: bool,
    y_is_log: bool,
    x_view: (f64, f64),
    xmin: f64,
    xmax: f64,
    /// Per-series (name, color, y_min, y_max) in draw order.
    legend: Vec<(String, [f32; 4], f64, f64)>,
    max_raw_points: usize,
    any_downsampled: bool,
}

/// Build everything needed to GPU-render a `GraphSpec` over `data` — series
/// points (windowed/downsampled/scaled per `win`), panel layout, and the
/// legend/axis facts `render_graph` folds into its text companion. Pure CPU
/// work; no GPU/lock involved, so callers can run this under a lock and then
/// drop it before the actual (blocking) render. This is the "GraphSpec +
/// LoadedData + size → renderable panels" core both `render_graph` and
/// `render_spec_to_png` share.
fn prepare_render(
    data: &LoadedData,
    spec: &GraphSpec,
    width: u32,
    height: u32,
    win: &RenderWindow,
) -> Result<RenderPrep, String> {
    let w = width.clamp(200, 2000);
    let h = height.clamp(150, 1400);
    let lay = spec.layout;
    let tf = spec.transform;

    // X values: datetime → epoch-second timestamps, else numeric.
    let xcol = &data.column_data[spec.x_col];
    let (xs, x_is_time): (Vec<f64>, bool) = match column_to_timestamps(xcol) {
        Some((v, _)) => (v, true),
        None => (column_to_f64(xcol).0, false),
    };
    // Large-coordinate render offset: datetime X is epoch seconds (~1.8e9),
    // beyond f32 precision, so every X vertex (and the view bounds / grid) is
    // shifted by a stable in-window origin before the f32 cast. The origin
    // cancels in the shader's (p - view_min)/(view_max - view_min), so lines
    // render precisely instead of collapsing. Label/tick text stays in the
    // original epoch space (see the text companion below).
    let x_origin = xs.iter().copied().find(|x| x.is_finite()).unwrap_or(0.0);

    // Per-series finite points + colour + own y-range. Very large series
    // are downsampled to ~2×width for rendering (keeps the shape, renders
    // fast — this is the win for files too big to read directly).
    let render_cap = (w as usize) * 2;
    let ds_mode = win.downsample.as_deref().unwrap_or("minmax");
    let robust = win.robust;
    let y_is_log = win.y_log;
    let mut sdata: Vec<PanelSeries> = Vec::new();
    let (mut xmin, mut xmax) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut max_raw_points = 0usize;
    let mut any_downsampled = false;
    for (k, &yc) in spec.y_cols.iter().enumerate() {
        let (ysv, _) = column_to_f64(&data.column_data[yc]);
        let ysv = tf.apply(&xs, &ysv); // optional transform (smooth/derivative/integral)

        // Finite (x, y) pairs + y-range over the FULL series.
        let mut fx: Vec<f64> = Vec::new();
        let mut fy: Vec<f64> = Vec::new();
        let (mut ymn, mut ymx) = (f64::INFINITY, f64::NEG_INFINITY);
        for (row, (&x, &y)) in xs.iter().zip(ysv.iter()).enumerate() {
            if win.row_start.is_some_and(|rs| row < rs) || win.row_end.is_some_and(|re| row >= re) {
                continue;
            }
            if !x.is_finite() || !y.is_finite() {
                continue;
            }
            if win.x_min.is_some_and(|xm| x < xm) || win.x_max.is_some_and(|xx| x > xx) {
                continue;
            }
            fx.push(x);
            fy.push(y);
            xmin = xmin.min(x);
            xmax = xmax.max(x);
            ymn = ymn.min(y);
            ymx = ymx.max(y);
        }

        // Log-Y: keep positive values, map to log10, and re-fit the y-range.
        if y_is_log {
            let mut lx = Vec::with_capacity(fx.len());
            let mut ly = Vec::with_capacity(fy.len());
            ymn = f64::INFINITY;
            ymx = f64::NEG_INFINITY;
            for (&x, &y) in fx.iter().zip(fy.iter()) {
                if y > 0.0 {
                    let l = y.log10();
                    lx.push(x);
                    ly.push(l);
                    ymn = ymn.min(l);
                    ymx = ymx.max(l);
                }
            }
            fx = lx;
            fy = ly;
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
            .map(|(&x, &y)| [(x - x_origin) as f32, y as f32])
            .collect();
        sdata.push(PanelSeries {
            name: data.columns[yc].clone(),
            color: PALETTE[k % PALETTE.len()],
            points: pts,
            ymin: ymn,
            ymax: ymx,
        });
    }
    if !xmin.is_finite() || (xmax - xmin) <= 0.0 {
        return Err("no finite data to plot for this graph".to_string());
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
                x_origin,
                w,
                panel_h,
                spec.draw_mode,
                false,
                robust,
            ));
        }
    } else {
        panels.push(build_panel(
            &sdata, x_view, x_origin, w, panel_h, spec.draw_mode, normalize, robust,
        ));
    }
    let out_h = panel_h * n;
    let legend = sdata
        .iter()
        .map(|sd| (sd.name.clone(), sd.color, sd.ymin, sd.ymax))
        .collect();

    Ok(RenderPrep {
        panels,
        panel_w: w,
        panel_h,
        out_h,
        x_is_time,
        y_is_log,
        x_view,
        xmin,
        xmax,
        legend,
        max_raw_points,
        any_downsampled,
    })
}

/// Run the GPU render + numeric tick-label baking (+ vertical compositing for
/// a stacked layout) for a prepared render, off the tokio executor (GPU
/// read-back blocks).
async fn render_rgba(prep: RenderPrep) -> Result<Vec<u8>, String> {
    let RenderPrep { panels, panel_w, panel_h, out_h, x_is_time, y_is_log, .. } = prep;
    tokio::task::spawn_blocking(move || {
        pollster::block_on(async move {
            let clear = [0.055_f64, 0.059, 0.075, 1.0];
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
                        composite[dst..dst + row_bytes].copy_from_slice(&prgba[src..src + row_bytes]);
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
                    // Log-Y: labels show the real value (10^tick), not the log.
                    let ylabel = if y_is_log {
                        format_tick_value(10f64.powf(v))
                    } else {
                        format_tick_value(v)
                    };
                    draw_text(&mut buf, panel_w, out_h, &ylabel, 3, py - 5, lc, 2);
                }
            }
            buf
        })
    })
    .await
    .map_err(|e| format!("render task failed: {e}"))
}

/// Render a `GraphSpec` over `data` to PNG bytes at the given size, using the
/// standard full-range minmax downsample/autoscale and linear Y (no windowing
/// overrides — that's `render_graph`'s job via `RenderWindow`). This is the
/// one-shot render path used by `report`, which plots without creating
/// persistent session graph state.
async fn render_spec_to_png(
    data: &LoadedData,
    spec: &GraphSpec,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let prep = prepare_render(data, spec, width, height, &RenderWindow::default())?;
    let (panel_w, out_h) = (prep.panel_w, prep.out_h);
    let rgba = render_rgba(prep).await?;
    let img = image::RgbaImage::from_raw(panel_w, out_h, rgba)
        .ok_or_else(|| "render produced a mis-sized buffer".to_string())?;
    let mut png: Vec<u8> = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("PNG encode failed: {e}"))?;
    Ok(png)
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct PingParams {
    /// Optional text; the server echoes it back. Omit for a bare liveness check.
    #[serde(default)]
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

// ─── report: self-contained HTML QC deliverable ────────────────────────────────

/// Escape text for safe interpolation into HTML (column names / detail
/// strings are user data and may contain markup-sensitive characters).
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Best available row/range locator for a finding, formatted for display.
fn finding_location(f: &Finding) -> Option<String> {
    if let Some(r) = f.row {
        return Some(format!("row {r}"));
    }
    if let Some(r) = f.onset_row {
        return Some(format!("onset row {r}"));
    }
    if let Some(r) = f.first_gap_row {
        return Some(format!("first gap at row {r}"));
    }
    if let Some(ranges) = &f.rows {
        if ranges.is_empty() {
            return None;
        }
        let shown = ranges.iter().take(5).map(|r| format!("{}\u{2013}{}", r[0], r[1])).collect::<Vec<_>>().join(", ");
        return Some(if ranges.len() > 5 {
            format!("rows {shown} (+{} more)", ranges.len() - 5)
        } else {
            format!("rows {shown}")
        });
    }
    None
}

/// Assemble the self-contained report HTML: title + summary, verdict banner,
/// findings, auto-selected plots (inline base64 PNGs), and a flagged-rows
/// table. No external resources — everything is inlined so the file is a
/// standalone document that can be handed off or opened offline.
#[allow(clippy::too_many_arguments)]
fn build_report_html(
    title: &str,
    generated_at: &str,
    data_summary: &serde_json::Value,
    findings: &[Finding],
    plots: &[(String, String)],
    flagged_cols: &[String],
    flagged_rows: &[(usize, Vec<String>)],
    total_flagged: usize,
) -> String {
    let n_high = findings.iter().filter(|f| matches!(f.severity, Severity::High)).count();
    let n_medium = findings.iter().filter(|f| matches!(f.severity, Severity::Medium)).count();
    let n_low = findings.iter().filter(|f| matches!(f.severity, Severity::Low)).count();

    let (verdict_class, verdict_text) = if findings.is_empty() {
        ("clean".to_string(), "No issues found — clean.".to_string())
    } else {
        let mut parts = Vec::new();
        if n_high > 0 {
            parts.push(format!("{n_high} High"));
        }
        if n_medium > 0 {
            parts.push(format!("{n_medium} Medium"));
        }
        if n_low > 0 {
            parts.push(format!("{n_low} Low"));
        }
        let class = if n_high > 0 {
            "high"
        } else if n_medium > 0 {
            "medium"
        } else {
            "low"
        };
        (
            class.to_string(),
            format!(
                "{} finding{}: {}",
                findings.len(),
                if findings.len() == 1 { "" } else { "s" },
                parts.join(", ")
            ),
        )
    };

    let n_rows = data_summary.get("n_rows").and_then(|v| v.as_u64()).unwrap_or(0);
    let n_cols = data_summary.get("n_columns").and_then(|v| v.as_u64()).unwrap_or(0);
    let n_numeric = data_summary.get("n_numeric_columns").and_then(|v| v.as_u64()).unwrap_or(0);
    let time_range_row = match data_summary.get("time_range").and_then(|v| v.as_object()) {
        Some(tr) => {
            let start = tr.get("start").and_then(|v| v.as_str()).unwrap_or("?");
            let end = tr.get("end").and_then(|v| v.as_str()).unwrap_or("?");
            format!(
                "<div class=\"stat\"><span class=\"stat-label\">Time range</span><span class=\"stat-value\">{} &rarr; {}</span></div>",
                html_escape(start),
                html_escape(end)
            )
        }
        None => String::new(),
    };

    let mut findings_html = String::new();
    if findings.is_empty() {
        findings_html.push_str("<p class=\"muted\">No QC findings — every check passed.</p>");
    } else {
        findings_html.push_str("<div class=\"findings\">");
        for f in findings {
            let sev_class = match f.severity {
                Severity::High => "high",
                Severity::Medium => "medium",
                Severity::Low => "low",
            };
            let sev_label = match f.severity {
                Severity::High => "High",
                Severity::Medium => "Medium",
                Severity::Low => "Low",
            };
            let loc = finding_location(f).map(|l| format!("<span class=\"finding-loc\">{}</span>", html_escape(&l))).unwrap_or_default();
            let col = f.column.as_ref().map(|c| format!("<span class=\"finding-col\">{}</span>", html_escape(c))).unwrap_or_default();
            let affected = f.affected.as_ref().filter(|a| !a.is_empty()).map(|a| {
                format!(
                    "<div class=\"finding-extra\">affected: {}</div>",
                    html_escape(&a.join(", "))
                )
            }).unwrap_or_default();
            let culprit = f.culprit.as_ref().filter(|c| !c.is_empty()).map(|c| {
                format!(
                    "<div class=\"finding-extra\">likely source: {}</div>",
                    html_escape(&c.join(", "))
                )
            }).unwrap_or_default();
            findings_html.push_str(&format!(
                "<div class=\"finding finding-{sev_class}\">\
                    <div class=\"finding-head\"><span class=\"badge badge-{sev_class}\">{sev_label}</span>\
                    <span class=\"finding-kind\">{kind}</span>{col}{loc}</div>\
                    <div class=\"finding-detail\">{detail}</div>{affected}{culprit}\
                </div>",
                sev_class = sev_class,
                sev_label = sev_label,
                kind = html_escape(&f.kind),
                col = col,
                loc = loc,
                detail = html_escape(&f.detail),
                affected = affected,
                culprit = culprit,
            ));
        }
        findings_html.push_str("</div>");
    }

    let mut plots_html = String::new();
    if plots.is_empty() {
        plots_html.push_str("<p class=\"muted\">No plots were generated for this dataset.</p>");
    } else {
        for (caption, b64) in plots {
            plots_html.push_str(&format!(
                "<figure class=\"plot\"><img src=\"data:image/png;base64,{b64}\" alt=\"{alt}\">\
                <figcaption>{caption}</figcaption></figure>",
                b64 = b64,
                alt = html_escape(caption),
                caption = html_escape(caption),
            ));
        }
    }

    let mut table_html = String::new();
    if flagged_rows.is_empty() {
        table_html.push_str("<p class=\"muted\">No specific rows were flagged.</p>");
    } else {
        table_html.push_str("<div class=\"table-wrap\"><table><thead><tr><th>Row</th>");
        for c in flagged_cols {
            table_html.push_str(&format!("<th>{}</th>", html_escape(c)));
        }
        table_html.push_str("</tr></thead><tbody>");
        for (r, vals) in flagged_rows {
            table_html.push_str(&format!("<tr><td class=\"row-idx\">{r}</td>"));
            for v in vals {
                table_html.push_str(&format!("<td>{}</td>", html_escape(v)));
            }
            table_html.push_str("</tr>");
        }
        table_html.push_str("</tbody></table></div>");
        let shown = flagged_rows.len();
        let suffix = if total_flagged > shown { "+" } else { "" };
        table_html.push_str(&format!(
            "<p class=\"muted\">Showing {shown} of {total_flagged}{suffix} flagged row(s).</p>"
        ));
    }

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{title_esc} — OxidePlot QC Report</title>
<style>
  :root {{
    --bg: #ffffff; --fg: #1a1d23; --muted: #5b6270; --border: #dde1e7;
    --card: #f7f8fa; --high: #b3261e; --high-bg: #fdecea;
    --medium: #96660a; --medium-bg: #fdf3e0; --low: #2f6f4e; --low-bg: #e9f6ee;
    --clean: #2f6f4e; --clean-bg: #e9f6ee; --accent: #2a5db0;
  }}
  * {{ box-sizing: border-box; }}
  body {{
    margin: 0; padding: 2.5rem 3rem 4rem; background: var(--bg); color: var(--fg);
    font-family: -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
    line-height: 1.5; max-width: 980px; margin-inline: auto;
  }}
  h1 {{ font-size: 1.6rem; margin: 0 0 0.15rem; }}
  h2 {{ font-size: 1.1rem; margin: 2.2rem 0 0.8rem; border-bottom: 1px solid var(--border); padding-bottom: 0.35rem; }}
  .generated {{ color: var(--muted); font-size: 0.85rem; margin-bottom: 1.4rem; }}
  .summary {{ display: flex; flex-wrap: wrap; gap: 0.75rem; margin-bottom: 1.2rem; }}
  .stat {{ background: var(--card); border: 1px solid var(--border); border-radius: 8px; padding: 0.6rem 0.9rem; min-width: 9rem; }}
  .stat-label {{ display: block; font-size: 0.72rem; color: var(--muted); text-transform: uppercase; letter-spacing: 0.03em; }}
  .stat-value {{ display: block; font-size: 1.05rem; font-weight: 600; margin-top: 0.15rem; }}
  .verdict {{ border-radius: 10px; padding: 0.9rem 1.1rem; font-weight: 600; font-size: 1.02rem; border: 1px solid; }}
  .verdict.high {{ background: var(--high-bg); color: var(--high); border-color: var(--high); }}
  .verdict.medium {{ background: var(--medium-bg); color: var(--medium); border-color: var(--medium); }}
  .verdict.low {{ background: var(--low-bg); color: var(--low); border-color: var(--low); }}
  .verdict.clean {{ background: var(--clean-bg); color: var(--clean); border-color: var(--clean); }}
  .findings {{ display: flex; flex-direction: column; gap: 0.6rem; }}
  .finding {{ border: 1px solid var(--border); border-left-width: 4px; border-radius: 6px; padding: 0.6rem 0.85rem; background: var(--card); }}
  .finding-high {{ border-left-color: var(--high); }}
  .finding-medium {{ border-left-color: var(--medium); }}
  .finding-low {{ border-left-color: var(--low); }}
  .finding-head {{ display: flex; flex-wrap: wrap; align-items: center; gap: 0.5rem; font-size: 0.92rem; }}
  .badge {{ display: inline-block; font-size: 0.7rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.03em; border-radius: 4px; padding: 0.15rem 0.45rem; color: #fff; }}
  .badge-high {{ background: var(--high); }}
  .badge-medium {{ background: var(--medium); }}
  .badge-low {{ background: var(--low); }}
  .finding-kind {{ font-weight: 600; }}
  .finding-col {{ color: var(--accent); font-family: ui-monospace, Consolas, monospace; font-size: 0.85rem; }}
  .finding-loc {{ color: var(--muted); font-size: 0.82rem; margin-left: auto; }}
  .finding-detail {{ margin-top: 0.3rem; color: var(--fg); font-size: 0.92rem; }}
  .finding-extra {{ margin-top: 0.2rem; color: var(--muted); font-size: 0.82rem; }}
  .plot {{ margin: 0 0 1.6rem; }}
  .plot img {{ max-width: 100%; height: auto; border: 1px solid var(--border); border-radius: 8px; display: block; }}
  .plot figcaption {{ margin-top: 0.4rem; font-size: 0.85rem; color: var(--muted); }}
  .table-wrap {{ overflow-x: auto; border: 1px solid var(--border); border-radius: 8px; }}
  table {{ border-collapse: collapse; width: 100%; font-size: 0.82rem; }}
  th, td {{ padding: 0.35rem 0.6rem; border-bottom: 1px solid var(--border); text-align: right; white-space: nowrap; }}
  th:first-child, td:first-child {{ text-align: left; }}
  thead th {{ background: var(--card); position: sticky; top: 0; }}
  td.row-idx {{ font-family: ui-monospace, Consolas, monospace; color: var(--muted); text-align: left; }}
  .muted {{ color: var(--muted); font-size: 0.9rem; }}
  footer {{ margin-top: 3rem; color: var(--muted); font-size: 0.78rem; border-top: 1px solid var(--border); padding-top: 0.8rem; }}
  @media print {{
    body {{ padding: 0.5in; max-width: none; }}
    .plot {{ break-inside: avoid; }}
    .finding {{ break-inside: avoid; }}
    * {{ -webkit-print-color-adjust: exact; print-color-adjust: exact; }}
  }}
</style>
</head>
<body>
<h1>{title_esc}</h1>
<div class="generated">Generated {generated_esc} &middot; OxidePlot QC report</div>

<div class="summary">
  <div class="stat"><span class="stat-label">Rows</span><span class="stat-value">{n_rows}</span></div>
  <div class="stat"><span class="stat-label">Columns</span><span class="stat-value">{n_cols} ({n_numeric} numeric)</span></div>
  {time_range_row}
</div>

<div class="verdict {verdict_class}">{verdict_text_esc}</div>

<h2>Findings</h2>
{findings_html}

<h2>Plots</h2>
{plots_html}

<h2>Flagged rows</h2>
{table_html}

<footer>Self-contained QC report &mdash; produced by OxidePlot's <code>report</code> tool.</footer>
</body>
</html>
"#,
        title_esc = html_escape(title),
        generated_esc = html_escape(generated_at),
        n_rows = n_rows,
        n_cols = n_cols,
        n_numeric = n_numeric,
        time_range_row = time_range_row,
        verdict_class = verdict_class,
        verdict_text_esc = html_escape(&verdict_text),
        findings_html = findings_html,
        plots_html = plots_html,
        table_html = table_html,
    )
}
