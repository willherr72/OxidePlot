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
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use serde_json::json;
use std::f64::consts::PI;
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

/// Pearson correlation over rows where both series are finite. None if < 2 pairs
/// or a series has zero variance.
fn pearson(a: &[f64], b: &[f64]) -> Option<f64> {
    let mut n = 0usize;
    let (mut sx, mut sy) = (0.0, 0.0);
    for (&x, &y) in a.iter().zip(b.iter()) {
        if x.is_finite() && y.is_finite() {
            n += 1;
            sx += x;
            sy += y;
        }
    }
    if n < 2 {
        return None;
    }
    let (mx, my) = (sx / n as f64, sy / n as f64);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (&x, &y) in a.iter().zip(b.iter()) {
        if x.is_finite() && y.is_finite() {
            let (dx, dy) = (x - mx, y - my);
            sxy += dx * dy;
            sxx += dx * dx;
            syy += dy * dy;
        }
    }
    let denom = (sxx * syy).sqrt();
    if denom == 0.0 {
        None
    } else {
        Some(sxy / denom)
    }
}

/// Infer the sample rate (Hz) from the dataset's first datetime column (1/median
/// dt between rows). Falls back to 1.0 (freq then reads in cycles/sample).
fn infer_sample_rate(ds: &Dataset) -> f64 {
    for c in 0..ds.data.columns.len() {
        if let Some((ts, frac)) = column_to_timestamps(&ds.data.column_data[c]) {
            if frac >= 0.5 && ts.len() >= 2 {
                let mut dts: Vec<f64> = ts
                    .windows(2)
                    .map(|w| w[1] - w[0])
                    .filter(|d| d.is_finite() && *d > 0.0)
                    .collect();
                if !dts.is_empty() {
                    dts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    let md = dts[dts.len() / 2];
                    if md > 0.0 {
                        return 1.0 / md;
                    }
                }
            }
        }
    }
    1.0
}

/// Hann-windowed, mean-removed FFT of `vals`, returning one-sided (frequency,
/// power) arrays (DC bin dropped). `fs` is the sample rate in Hz.
fn compute_psd(vals: &[f64], fs: f64) -> (Vec<f64>, Vec<f64>) {
    let y: Vec<f64> = vals.iter().copied().filter(|v| v.is_finite()).collect();
    let n = y.len();
    if n < 4 {
        return (vec![], vec![]);
    }
    let mean = y.iter().sum::<f64>() / n as f64;
    let mut buf: Vec<Complex<f64>> = (0..n)
        .map(|i| {
            let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / (n as f64 - 1.0)).cos();
            Complex::new((y[i] - mean) * w, 0.0)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    let half = n / 2;
    let mut freqs = Vec::with_capacity(half);
    let mut power = Vec::with_capacity(half);
    for k in 1..half {
        freqs.push(k as f64 * fs / n as f64);
        power.push(buf[k].norm_sqr() / n as f64);
    }
    (freqs, power)
}

/// Short-time FFT magnitude matrix (`frames[frame][bin]`, bins = window/2, hop =
/// window/2). Returns the matrix and the bin count.
fn compute_spectrogram(vals: &[f64], window: usize) -> (Vec<Vec<f64>>, usize) {
    let y: Vec<f64> = vals.iter().copied().filter(|v| v.is_finite()).collect();
    let w = window.clamp(16, 4096);
    let n = y.len();
    if n < w {
        return (vec![], 0);
    }
    let hop = (w / 2).max(1);
    let bins = w / 2;
    let fft = FftPlanner::new().plan_fft_forward(w);
    let hann: Vec<f64> = (0..w)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / (w as f64 - 1.0)).cos())
        .collect();
    let mut frames: Vec<Vec<f64>> = Vec::new();
    let mut start = 0;
    while start + w <= n {
        let mut buf: Vec<Complex<f64>> = (0..w)
            .map(|i| Complex::new(y[start + i] * hann[i], 0.0))
            .collect();
        fft.process(&mut buf);
        frames.push((0..bins).map(|k| buf[k].norm_sqr().sqrt()).collect());
        start += hop;
    }
    (frames, bins)
}

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

/// Group a sorted list of row indices into runs (start, end, count), joining
/// indices no more than `max_gap` apart.
fn group_runs(rows: &[usize], max_gap: usize) -> Vec<(usize, usize, usize)> {
    let mut runs = Vec::new();
    if rows.is_empty() {
        return runs;
    }
    let (mut start, mut prev, mut count) = (rows[0], rows[0], 1usize);
    for &r in &rows[1..] {
        if r <= prev + max_gap + 1 {
            prev = r;
            count += 1;
        } else {
            runs.push((start, prev, count));
            start = r;
            prev = r;
            count = 1;
        }
    }
    runs.push((start, prev, count));
    runs
}

/// Median and MAD (median absolute deviation) of the finite values.
fn median_mad(vals: &[f64]) -> Option<(f64, f64)> {
    let mut v: Vec<f64> = vals.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let med = v[v.len() / 2];
    let mut d: Vec<f64> = v.iter().map(|x| (x - med).abs()).collect();
    d.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some((med, d[d.len() / 2]))
}

/// Narrow a changepoint (level `m0` → `m1`) to the transition row within
/// `[lo, hi)` — the first row where a trailing-window mean crosses the midpoint.
fn localize_changepoint(vals: &[f64], lo: usize, hi: usize, m0: f64, m1: f64) -> usize {
    let mid = (m0 + m1) * 0.5;
    let ascending = m1 >= m0;
    let win = ((hi - lo) / 6).clamp(5, 300);
    for r in lo..hi {
        let a = r.saturating_sub(win).max(lo);
        let s: Vec<f64> = vals[a..=r].iter().copied().filter(|v| v.is_finite()).collect();
        if s.is_empty() {
            continue;
        }
        let m = s.iter().sum::<f64>() / s.len() as f64;
        if (ascending && m >= mid) || (!ascending && m <= mid) {
            return r;
        }
    }
    (lo + hi) / 2
}

/// Rough channel role from its name: 0 = raw source (`raw…`), 2 = derived output
/// (calibrated/calculated/…), 1 = neutral. Used to trace a fault to its source.
fn channel_role(name: &str) -> u8 {
    let n = name.to_ascii_lowercase();
    if n.starts_with("raw") {
        return 0;
    }
    const DERIVED: &[&str] = &[
        "calibrated",
        "calculated",
        "computed",
        "corrected",
        "derived",
        "adjusted",
    ];
    if DERIVED.iter().any(|d| n.contains(d)) {
        return 2;
    }
    1
}

/// Robust level shift at `onset`: (median_after − median_before, |shift| / MAD-noise)
/// over a window `w` either side. Used to spot a co-occurring shift in a raw
/// channel that didn't independently cross the changepoint threshold.
fn shift_ratio_at(vals: &[f64], onset: usize, w: usize) -> Option<(f64, f64)> {
    let n = vals.len();
    if onset == 0 || onset >= n {
        return None;
    }
    let lo = onset.saturating_sub(w);
    let hi = (onset + w).min(n);
    let before: Vec<f64> = vals[lo..onset].iter().copied().filter(|v| v.is_finite()).collect();
    let after: Vec<f64> = vals[onset..hi].iter().copied().filter(|v| v.is_finite()).collect();
    let (mb, db) = median_mad(&before)?;
    let (ma, da) = median_mad(&after)?;
    let noise = (db.max(da) * 1.4826).max(1e-9);
    let shift = ma - mb;
    Some((shift, shift.abs() / noise))
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
    /// subtract/ratio take exactly two; scale takes exactly one.
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
    /// Y-axis autoscale: "minmax" (default) or "robust" (clip to the 1st–99th
    /// percentile so a lone extreme outlier doesn't flatten the signal).
    #[serde(default)]
    autoscale: Option<String>,
    /// Y-axis scale: "linear" (default) or "log" (log10; non-positive values are
    /// dropped). Good for series spanning orders of magnitude.
    #[serde(default)]
    y_scale: Option<String>,
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
        let n_rows = ds.data.row_count;
        // (severity_rank, finding); 0 = high, 1 = medium, 2 = low.
        let mut findings: Vec<(u8, serde_json::Value)> = Vec::new();
        // Detected changepoints (col idx, onset row, shift) — grouped + traced later.
        let mut changepoints: Vec<(usize, usize, f64)> = Vec::new();

        // Dataset-level: time-index gaps (first datetime column).
        for c in 0..ds.data.columns.len() {
            if let Some((ts, frac)) = column_to_timestamps(&ds.data.column_data[c]) {
                if frac >= 0.5 && ts.len() >= 3 {
                    let dts: Vec<f64> = ts.windows(2).map(|w| w[1] - w[0]).collect();
                    let mut sorted: Vec<f64> =
                        dts.iter().copied().filter(|d| d.is_finite() && *d > 0.0).collect();
                    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    if !sorted.is_empty() {
                        let median = sorted[sorted.len() / 2];
                        let (mut n_gaps, mut lost, mut first) = (0usize, 0.0, None);
                        for (i, &d) in dts.iter().enumerate() {
                            if d.is_finite() && d > median * 3.0 && d > median + 1e-6 {
                                n_gaps += 1;
                                lost += d - median;
                                if first.is_none() {
                                    first = Some(i);
                                }
                            }
                        }
                        if n_gaps > 0 {
                            findings.push((1, json!({
                                "severity": "medium", "kind": "time_gaps",
                                "column": ds.data.columns[c],
                                "detail": format!("{n_gaps} gap(s), ~{:.0}s of samples missing", lost),
                                "first_gap_row": first,
                            })));
                        }
                    }
                }
                break;
            }
        }

        // Per numeric column checks.
        for c in 0..ds.data.columns.len() {
            if !ds.numeric_cols[c] {
                continue;
            }
            let name = &ds.data.columns[c];
            let cells = &ds.data.column_data[c];
            // Skip a datetime index column (it's the time axis, not a data channel).
            if column_to_timestamps(cells)
                .map(|(_, f)| f >= 0.5)
                .unwrap_or(false)
            {
                continue;
            }
            let (vals, _) = column_to_f64(cells);
            let finite: Vec<f64> = vals.iter().copied().filter(|v| v.is_finite()).collect();
            let n_finite = finite.len();
            let n_missing = n_rows.saturating_sub(n_finite);
            let distinct = cells.iter().collect::<std::collections::HashSet<_>>().len();
            let const_run = longest_constant_run(cells);

            // Dead / constant.
            if n_finite > 0 && distinct <= 1 {
                findings.push((0, json!({ "severity": "high", "kind": "dead", "column": name, "detail": "single distinct value (dead/constant)" })));
            } else if n_finite > 0 {
                let n_zero = finite.iter().filter(|&&v| v == 0.0).count();
                if n_zero as f64 / n_finite as f64 > 0.95 {
                    findings.push((0, json!({ "severity": "high", "kind": "dead", "column": name, "detail": format!("{:.0}% zero", 100.0 * n_zero as f64 / n_finite as f64) })));
                }
            }
            // Frozen (long constant run but not fully dead).
            if distinct > 1 && (const_run as f64) > (n_rows as f64 * 0.05).max(30.0) {
                findings.push((1, json!({ "severity": "medium", "kind": "frozen", "column": name, "detail": format!("stuck for {const_run} consecutive rows") })));
            }
            // Missing.
            if n_rows > 0 && n_missing as f64 / n_rows as f64 > 0.05 {
                findings.push((1, json!({ "severity": "medium", "kind": "missing", "column": name, "detail": format!("{:.1}% missing ({n_missing} rows)", 100.0 * n_missing as f64 / n_rows as f64) })));
            }
            // Outliers via robust z-score (median / MAD): isolated short runs are
            // glitches; a long contiguous run is a regime (e.g. the stationary
            // survey start), not a glitch — reported separately at lower severity.
            if n_finite >= 20 {
                if let Some((median, mad)) = median_mad(&finite) {
                    if mad > 0.0 {
                        let rows: Vec<usize> = vals
                            .iter()
                            .enumerate()
                            .filter(|(_, &v)| {
                                v.is_finite() && (v - median).abs() / (1.4826 * mad) > 10.0
                            })
                            .map(|(r, _)| r)
                            .collect();
                        let mut glitch_ranges: Vec<serde_json::Value> = Vec::new();
                        let mut glitch_count = 0usize;
                        for (rs, re, count) in group_runs(&rows, 2) {
                            if count <= 4 {
                                glitch_count += count;
                                if glitch_ranges.len() < 10 {
                                    glitch_ranges.push(json!([rs, re]));
                                }
                            } else {
                                findings.push((1, json!({ "severity": "medium", "kind": "outlier_regime", "column": name, "detail": format!("{count} consecutive out-of-range samples (a regime, not a glitch)"), "rows": [rs, re] })));
                            }
                        }
                        if glitch_count > 0 {
                            findings.push((0, json!({ "severity": "high", "kind": "glitch", "column": name, "detail": format!("{glitch_count} isolated out-of-range sample(s)"), "rows": glitch_ranges })));
                        }
                    }
                }
            }
            // Changepoint: an adjacent-segment MEDIAN jump large vs the local
            // MAD-noise (robust — a lone spike can't move the median), then
            // localised to the actual transition row (finer than the segment grid).
            if n_rows >= 60 {
                let n_seg = (n_rows / 200).clamp(8, 40);
                let mut meds: Vec<Option<f64>> = Vec::new();
                let mut mads: Vec<f64> = Vec::new();
                let mut bounds: Vec<(usize, usize)> = Vec::new();
                for seg in 0..n_seg {
                    let lo = seg * n_rows / n_seg;
                    let hi = ((seg + 1) * n_rows / n_seg).min(n_rows);
                    let sl: Vec<f64> = vals
                        .get(lo..hi)
                        .unwrap_or(&[])
                        .iter()
                        .copied()
                        .filter(|v| v.is_finite())
                        .collect();
                    match median_mad(&sl) {
                        Some((m, d)) => {
                            meds.push(Some(m));
                            mads.push(d);
                        }
                        None => meds.push(None),
                    }
                    bounds.push((lo, hi));
                }
                mads.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let scale = (if mads.is_empty() {
                    0.0
                } else {
                    mads[mads.len() / 2] * 1.4826
                })
                .max(1e-9);
                for seg in 1..n_seg {
                    if let (Some(a), Some(b)) = (meds[seg - 1], meds[seg]) {
                        if (b - a).abs() > 6.0 * scale {
                            let onset =
                                localize_changepoint(&vals, bounds[seg - 1].0, bounds[seg].1, a, b);
                            changepoints.push((c, onset, b - a));
                            break;
                        }
                    }
                }
            }
        }

        // --- Channel-lineage tracing: cluster co-occurring changepoints into one
        // event and attribute it to the likely raw source (naming → explicit
        // lineage hint → co-occurrence scan of raw channels at the onset). ---
        changepoints.sort_by_key(|cp| cp.1);
        let tol = (n_rows / 50).max(20);
        let mut clusters: Vec<Vec<(usize, usize, f64)>> = Vec::new();
        for cp in changepoints {
            match clusters.last_mut() {
                Some(last) if cp.1 <= last.last().unwrap().1 + tol => last.push(cp),
                _ => clusters.push(vec![cp]),
            }
        }
        let raw_cols: Vec<usize> = (0..ds.data.columns.len())
            .filter(|&c| ds.numeric_cols[c] && channel_role(&ds.data.columns[c]) == 0)
            .collect();

        for cl in &clusters {
            let onset = cl.iter().map(|c| c.1).min().unwrap();
            let cols_in: Vec<usize> = cl.iter().map(|c| c.0).collect();
            let mut culprit: Vec<usize> = Vec::new();
            // (a) raw-named columns already in the cluster.
            for &c in &cols_in {
                if channel_role(&ds.data.columns[c]) == 0 && !culprit.contains(&c) {
                    culprit.push(c);
                }
            }
            // (b) explicit lineage hint on any derived column in the cluster.
            if let Some(map) = &lineage {
                for &c in &cols_in {
                    if let Some(sources) = map.get(&ds.data.columns[c]) {
                        for src in sources {
                            if let Some(si) = resolve_col(&ds.data, src) {
                                if !culprit.contains(&si) {
                                    culprit.push(si);
                                }
                            }
                        }
                    }
                }
            }
            // (c) co-occurrence scan: a raw channel that shifted at the onset.
            if culprit.is_empty() {
                let w = tol.max(30);
                let mut best: Option<(usize, f64)> = None;
                for &rc in &raw_cols {
                    if cols_in.contains(&rc) {
                        continue;
                    }
                    let (rvals, _) = column_to_f64(&ds.data.column_data[rc]);
                    if let Some((_, ratio)) = shift_ratio_at(&rvals, onset, w) {
                        if ratio > 5.0 && best.map_or(true, |b| ratio > b.1) {
                            best = Some((rc, ratio));
                        }
                    }
                }
                if let Some((rc, _)) = best {
                    culprit.push(rc);
                }
            }

            let source_traced = culprit.iter().any(|c| !cols_in.contains(c));
            if cols_in.len() == 1 && !source_traced {
                let name = &ds.data.columns[cols_in[0]];
                findings.push((1, json!({ "severity": "medium", "kind": "changepoint", "column": name, "detail": format!("median shifts {:.4} at ~row {onset}", cl[0].2), "row": onset })));
            } else {
                let affected: Vec<&String> = cols_in.iter().map(|&c| &ds.data.columns[c]).collect();
                let culprit_names: Vec<&String> =
                    culprit.iter().map(|&c| &ds.data.columns[c]).collect();
                let detail = if culprit_names.is_empty() {
                    format!(
                        "coincident level shift across {} channels at ~row {onset} (source unclear)",
                        affected.len()
                    )
                } else {
                    format!(
                        "level shift at ~row {onset}; likely source: {}; affects {} channel(s)",
                        culprit_names
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                        affected.len()
                    )
                };
                let sev = if culprit_names.is_empty() { 1 } else { 0 };
                findings.push((sev, json!({
                    "severity": if sev == 0 { "high" } else { "medium" },
                    "kind": "regime_change_event",
                    "onset_row": onset,
                    "culprit": culprit_names,
                    "affected": affected,
                    "detail": detail,
                })));
            }
        }

        findings.sort_by_key(|f| f.0);
        let list: Vec<_> = findings.into_iter().map(|f| f.1).collect();
        Ok(Self::text_result(json!({
            "dataset_id": dataset_id,
            "n_rows": n_rows,
            "n_findings": list.len(),
            "findings": list,
        })))
    }

    #[tool(
        description = "Add a computed column to a dataset. op: 'magnitude' (sqrt of sum of squares, e.g. accel magnitude from x/y/z), 'add' or 'mean' (of >=1 columns), 'subtract' (A-B) or 'ratio' (A/B) (exactly 2), or 'scale' (scale*col+offset, exactly 1). The new column is usable by describe_data / query_data / create_graph / correlate."
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
        }): Parameters<DeriveParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut s = self.session.lock().await;
        let ds = s.datasets.get_mut(&dataset_id).ok_or_else(|| {
            McpError::invalid_params(format!("unknown dataset_id '{dataset_id}'"), None)
        })?;

        let mut idxs = Vec::new();
        for c in &columns {
            idxs.push(resolve_col(&ds.data, c).ok_or_else(|| {
                McpError::invalid_params(format!("unknown column '{c}'"), None)
            })?);
        }
        match op.as_str() {
            "subtract" | "ratio" if idxs.len() != 2 => {
                return Err(McpError::invalid_params(
                    format!("op '{op}' needs exactly 2 columns"),
                    None,
                ))
            }
            "scale" if idxs.len() != 1 => {
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
            "magnitude" | "add" | "mean" | "subtract" | "ratio" | "scale" => {}
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
        let n_rows = ds.data.row_count;
        let a = scale.unwrap_or(1.0);
        let b = offset.unwrap_or(0.0);
        let mut vals: Vec<f64> = Vec::with_capacity(n_rows);
        for r in 0..n_rows {
            let g = |k: usize| cols[k].get(r).copied().unwrap_or(f64::NAN);
            let v = match op.as_str() {
                "magnitude" => (0..cols.len()).map(|k| g(k) * g(k)).sum::<f64>().sqrt(),
                "add" => (0..cols.len()).map(g).sum(),
                "mean" => (0..cols.len()).map(g).sum::<f64>() / cols.len() as f64,
                "subtract" => g(0) - g(1),
                "ratio" => g(0) / g(1),
                "scale" => a * g(0) + b,
                _ => f64::NAN,
            };
            vals.push(v);
        }

        let name = new_name.unwrap_or_else(|| format!("{op}_{}", columns.join("_")));
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
            "from": columns,
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
            let fs = sample_rate.unwrap_or_else(|| infer_sample_rate(ds));
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
            let fs = sample_rate.unwrap_or_else(|| infer_sample_rate(ds));
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
            autoscale,
            y_scale,
        }): Parameters<RenderGraphParams>,
    ) -> Result<CallToolResult, McpError> {
        // Build per-panel render inputs under the lock; render without holding it.
        let (panels, panel_w, panel_h, out_h, clear, x_is_time, y_is_log, text) = {
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
            let robust = autoscale.as_deref() == Some("robust");
            let y_is_log = y_scale.as_deref() == Some("log");
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
                        robust,
                    ));
                }
            } else {
                panels.push(build_panel(
                    &sdata, x_view, w, panel_h, g.draw_mode, normalize, robust,
                ));
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
                "autoscale": if robust { "robust" } else { "minmax" },
                "y_scale": if y_is_log { "log" } else { "linear" },
                "window": { "row_start": row_start, "row_end": row_end, "x_min": x_min, "x_max": x_max },
                "note": note,
            })
            .to_string();

            let clear = [0.055_f64, 0.059, 0.075, 1.0];
            (panels, w, panel_h, out_h, clear, x_is_time, y_is_log, text)
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
