# Foundation: Analysis Compute → oxideplot-core — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the analysis compute (spectral, QC, histogram, expression evaluator, correlation, min/max downsampling) out of `oxideplot-mcp/src/main.rs` into `oxideplot-core` so the desktop app (WASM) and the MCP (native) share one implementation, leaving the MCP fully working.

**Architecture:** Each group of functions becomes a focused `oxideplot-core` module with a small public API and native unit tests that assert the behaviour already verified through the MCP. Structured results (`Finding`, `Histogram`) replace the MCP's ad-hoc `serde_json` construction so both consumers can use them. The MCP's tool methods then call core and serialize the results, preserving their current JSON shape. `rustfft` moves to core and MUST build for `wasm32`.

**Tech Stack:** Rust (edition 2021), `rustfft` 6, `serde`. `oxideplot-core` is a dependency of both `oxideplot-mcp` (native) and `oxideplot-wasm` (wasm32).

## Global Constraints

- The MCP must build and behave identically after this slice: same tool outputs (JSON shape and values) for `health_check`, `histogram`, `spectrum`, `spectrogram`, `correlate`, `derive_column`, `query_data` (filter), `render_graph` (min/max downsampling). Verified by the parity tests in each task and the MCP smoke in Task 8.
- Moved function **bodies are copied verbatim** from `oxideplot-mcp/src/main.rs` unless a step says otherwise; only signatures/imports change where noted.
- Core types in scope: `oxideplot_core::data::loader::{LoadedData, column_to_f64, column_to_timestamps}`. `LoadedData { columns: Vec<String>, column_data: Vec<Vec<String>> /* column-major */, row_count: usize }`. `column_to_f64(&[String]) -> (Vec<f64>, f64)`, `column_to_timestamps(&[String]) -> Option<(Vec<f64>, f64)>`.
- `oxideplot-core` has no `serde_json` dependency; core returns typed structs, and the MCP serializes them.
- Run all core tests with `cargo test -p oxideplot-core`. Rebuild the MCP with `cargo build -p oxideplot-mcp`.

---

## File Structure

**Create:**
- `crates/oxideplot-core/src/processing/expr.rs` — expression evaluator (parse/eval), `apply_filter`, `rolling_compute`.
- `crates/oxideplot-core/src/processing/spectral.rs` — `compute_psd`, `compute_spectrogram`, `infer_sample_rate`.
- `crates/oxideplot-core/src/processing/histogram.rs` — `Histogram`, `histogram()`.
- `crates/oxideplot-core/src/processing/qc.rs` — `Severity`, `Finding`, `health_check()`, and QC helpers.

**Modify:**
- `crates/oxideplot-core/src/processing/statistics.rs` — add `pearson`, `median_mad`.
- `crates/oxideplot-core/src/processing/downsampling.rs` — add `minmax_envelope`.
- `crates/oxideplot-core/src/data/loader.rs` — add `resolve_col`.
- `crates/oxideplot-core/src/processing/mod.rs` — declare the new modules.
- `crates/oxideplot-core/Cargo.toml` — add `rustfft = "6"`.
- `crates/oxideplot-mcp/src/main.rs` — delete the moved functions; import from core; adapt call sites.
- `crates/oxideplot-mcp/Cargo.toml` — remove `rustfft` (now transitive via core).

Every task is committed on the `tauri-migration` branch.

---

## Task 1: statistics — `pearson` + `median_mad`

**Files:**
- Modify: `crates/oxideplot-core/src/processing/statistics.rs`

**Interfaces:**
- Produces: `oxideplot_core::processing::statistics::pearson(a: &[f64], b: &[f64]) -> Option<f64>`; `median_mad(vals: &[f64]) -> Option<(f64, f64)>` (returns `(median, MAD)`).

- [ ] **Step 1: Write the failing tests** — append to `statistics.rs`:

```rust
#[cfg(test)]
mod moved_tests {
    use super::*;

    #[test]
    fn pearson_correlated_and_anti() {
        let a = [1.0, 2.0, 3.0, 4.0, 5.0];
        let b = [2.0, 4.0, 6.0, 8.0, 10.0]; // 2a → r = +1
        let c = [5.0, 4.0, 3.0, 2.0, 1.0];  // −a → r = −1
        assert!((pearson(&a, &b).unwrap() - 1.0).abs() < 1e-9);
        assert!((pearson(&a, &c).unwrap() + 1.0).abs() < 1e-9);
    }

    #[test]
    fn pearson_zero_variance_is_none() {
        let a = [1.0, 1.0, 1.0];
        let b = [1.0, 2.0, 3.0];
        assert!(pearson(&a, &b).is_none());
    }

    #[test]
    fn median_mad_basic() {
        // sorted 1,2,3,4,100 → median 3; abs devs 2,1,0,1,97 → sorted 0,1,1,2,97 → MAD 1
        let (m, d) = median_mad(&[3.0, 1.0, 4.0, 2.0, 100.0]).unwrap();
        assert_eq!(m, 3.0);
        assert_eq!(d, 1.0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p oxideplot-core pearson_correlated_and_anti`
Expected: FAIL — `cannot find function 'pearson'`.

- [ ] **Step 3: Implement** — add `pearson` and `median_mad` to `statistics.rs` (above the test module). Copy the bodies **verbatim** from `oxideplot-mcp/src/main.rs` — `pearson` at line 59, `median_mad` at line 662 — prefixing each with `pub`:

```rust
/// Pearson correlation over rows where both series are finite. None if < 2 pairs
/// or a series has zero variance.
pub fn pearson(a: &[f64], b: &[f64]) -> Option<f64> {
    // ... body verbatim from main.rs:59 ...
}

/// Median and MAD (median absolute deviation) of the finite values.
pub fn median_mad(vals: &[f64]) -> Option<(f64, f64)> {
    // ... body verbatim from main.rs:662 ...
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p oxideplot-core moved_tests`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/oxideplot-core/src/processing/statistics.rs
git commit -m "feat(core): add pearson + median_mad to statistics"
```

---

## Task 2: downsampling — `minmax_envelope`

**Files:**
- Modify: `crates/oxideplot-core/src/processing/downsampling.rs`

**Interfaces:**
- Produces: `oxideplot_core::processing::downsampling::minmax_envelope(fx: &[f64], fy: &[f64], buckets: usize) -> (Vec<f64>, Vec<f64>)`.

- [ ] **Step 1: Write the failing test** — append to `downsampling.rs`:

```rust
#[cfg(test)]
mod envelope_tests {
    use super::*;

    #[test]
    fn minmax_keeps_a_lone_spike() {
        // 1000 samples of 0.0 with a single 1e6 spike at index 640.
        let n = 1000;
        let fx: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let mut fy = vec![0.0f64; n];
        fy[640] = 1e6;
        let (_, oy) = minmax_envelope(&fx, &fy, 100); // 100 buckets → ≤200 points
        assert!(oy.iter().cloned().fold(0.0, f64::max) >= 1e6,
            "min/max envelope must preserve the spike");
        assert!(oy.len() < n, "should have downsampled");
    }

    #[test]
    fn minmax_passthrough_when_small() {
        let fx = [0.0, 1.0, 2.0];
        let fy = [0.0, 1.0, 2.0];
        let (ox, _) = minmax_envelope(&fx, &fy, 100);
        assert_eq!(ox.len(), 3);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p oxideplot-core minmax_keeps_a_lone_spike`
Expected: FAIL — `cannot find function 'minmax_envelope'`.

- [ ] **Step 3: Implement** — add `minmax_envelope` to `downsampling.rs`, body **verbatim** from `oxideplot-mcp/src/main.rs:755`, prefixed `pub`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p oxideplot-core envelope_tests`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/oxideplot-core/src/processing/downsampling.rs
git commit -m "feat(core): add minmax_envelope (spike-safe decimation)"
```

---

## Task 3: loader — `resolve_col`

**Files:**
- Modify: `crates/oxideplot-core/src/data/loader.rs`

**Interfaces:**
- Produces: `oxideplot_core::data::loader::resolve_col(data: &LoadedData, spec: &str) -> Option<usize>` — resolves a column by numeric-index string or exact name.

- [ ] **Step 1: Write the failing test** — append to `loader.rs`:

```rust
#[cfg(test)]
mod resolve_tests {
    use super::*;

    fn ld() -> LoadedData {
        LoadedData {
            columns: vec!["time".into(), "temp".into(), "pressure".into()],
            column_data: vec![vec![], vec![], vec![]],
            row_count: 0,
        }
    }

    #[test]
    fn resolve_by_name_and_index() {
        let d = ld();
        assert_eq!(resolve_col(&d, "temp"), Some(1));
        assert_eq!(resolve_col(&d, "2"), Some(2));
        assert_eq!(resolve_col(&d, "nope"), None);
        assert_eq!(resolve_col(&d, "9"), None); // out of range
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p oxideplot-core resolve_by_name_and_index`
Expected: FAIL — `cannot find function 'resolve_col'`.

- [ ] **Step 3: Implement** — add to `loader.rs`, body **verbatim** from `oxideplot-mcp/src/main.rs:48`, prefixed `pub`:

```rust
/// Resolve a column reference (name or numeric index string) to a column index.
pub fn resolve_col(data: &LoadedData, spec: &str) -> Option<usize> {
    if let Ok(i) = spec.parse::<usize>() {
        if i < data.columns.len() {
            return Some(i);
        }
    }
    data.columns.iter().position(|c| c == spec)
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p oxideplot-core resolve_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/oxideplot-core/src/data/loader.rs
git commit -m "feat(core): add resolve_col to loader"
```

---

## Task 4: `processing::expr` — expression evaluator, filter, rolling

**Files:**
- Create: `crates/oxideplot-core/src/processing/expr.rs`
- Modify: `crates/oxideplot-core/src/processing/mod.rs`

**Interfaces:**
- Consumes: `resolve_col` (Task 3), `column_to_f64`.
- Produces (all in `oxideplot_core::processing::expr`):
  - `pub enum Ast` (opaque to callers)
  - `pub fn parse_expr(data: &LoadedData, s: &str) -> Result<Ast, String>`
  - `pub fn collect_expr_cols(a: &Ast, out: &mut std::collections::HashSet<usize>)`
  - `pub fn eval_expr(a: &Ast, cols: &std::collections::HashMap<usize, Vec<f64>>, row: usize) -> f64`
  - `pub fn apply_filter(data: &LoadedData, rows: &[usize], filter: &str) -> Result<Vec<usize>, String>`
  - `pub fn rolling_compute(op: &str, cols: &[Vec<f64>], win: usize, n_rows: usize) -> Vec<f64>`

- [ ] **Step 1: Declare the module** — add to `crates/oxideplot-core/src/processing/mod.rs`:

```rust
pub mod expr;
```

- [ ] **Step 2: Write the module by moving code** — create `expr.rs`. At the top:

```rust
use crate::data::loader::{column_to_f64, resolve_col, LoadedData};
use std::collections::{HashMap, HashSet};
```

Then copy **verbatim** from `oxideplot-mcp/src/main.rs`, marking the listed items `pub`:
- `enum Rel` (main.rs:198) — mark **`pub enum Rel`** (it appears in the public `Ast::Cmp` variant; leaving it private triggers E0446 "private type in public interface").
- `enum Ast` (main.rs:208) — mark **`pub enum Ast`**.
- `enum Tok` (main.rs:220) — keep private.
- `fn tokenize_expr` (main.rs:232) — keep private.
- `struct ExprParser<'a>` + `impl ExprParser<'_>` (main.rs:328–456) — keep private.
- `fn parse_expr` (main.rs:457) — mark **`pub`**.
- `fn collect_expr_cols` (main.rs:470) — mark **`pub`**; change its signature's `std::collections::HashSet<usize>` to `HashSet<usize>` (import already added).
- `fn eval_expr` (main.rs:485) — mark **`pub`**; likewise use imported `HashMap`.
- `fn apply_filter` (main.rs:581) — mark **`pub`**.
- `fn rolling_compute` (main.rs:601) — mark **`pub`**; note it calls `crate::processing::statistics::pearson` for `rolling_corr` — change the `pearson(...)` call to `crate::processing::statistics::pearson(...)` and add `use crate::processing::statistics::pearson;` at the top.

Do NOT move `csv_escape` (main.rs:571) — it stays in the MCP for `export_csv`.

- [ ] **Step 3: Write the failing tests** — append to `expr.rs`:

```rust
#[cfg(test)]
mod expr_tests {
    use super::*;
    use crate::data::loader::LoadedData;

    fn dataset() -> LoadedData {
        // ax=3, ay=4, az=0 for all 5 rows.
        let col = |v: &str| vec![v.to_string(); 5];
        LoadedData {
            columns: vec!["ax".into(), "ay".into(), "az".into()],
            column_data: vec![col("3"), col("4"), col("0")],
            row_count: 5,
        }
    }

    fn eval_all(d: &LoadedData, s: &str) -> Vec<f64> {
        let ast = parse_expr(d, s).unwrap();
        let mut refs = HashSet::new();
        collect_expr_cols(&ast, &mut refs);
        let cols: HashMap<usize, Vec<f64>> = refs
            .iter()
            .map(|&ci| (ci, column_to_f64(&d.column_data[ci]).0))
            .collect();
        (0..d.row_count).map(|r| eval_expr(&ast, &cols, r)).collect()
    }

    #[test]
    fn magnitude_via_expr() {
        let d = dataset();
        assert!((eval_all(&d, "sqrt(ax^2 + ay^2 + az^2)")[0] - 5.0).abs() < 1e-9);
    }

    #[test]
    fn trig_survey_math() {
        let d = dataset();
        assert!((eval_all(&d, "deg(atan2(ay, ax))")[0] - 53.13010235).abs() < 1e-6);
    }

    #[test]
    fn comparisons_and_logic() {
        let d = dataset();
        assert_eq!(eval_all(&d, "ax > 2 and ay < 5")[0], 1.0);
        assert_eq!(eval_all(&d, "ax > 5 or az == 0")[0], 1.0);
        assert_eq!(eval_all(&d, "ax == ay")[0], 0.0);
    }

    #[test]
    fn unknown_column_errors() {
        let d = dataset();
        assert!(parse_expr(&d, "nope * 2").is_err());
    }

    #[test]
    fn filter_selects_rows() {
        // one row where ax=99, rest ax=3
        let mut d = dataset();
        d.column_data[0][2] = "99".into();
        let kept = apply_filter(&d, &(0..5).collect::<Vec<_>>(), "ax > 50").unwrap();
        assert_eq!(kept, vec![2]);
    }

    #[test]
    fn rolling_mean_trailing_window() {
        let cols = vec![vec![0.0, 2.0, 4.0, 6.0]];
        let out = rolling_compute("rolling_mean", &cols, 2, 4);
        // trailing window of 2: [0], [0,2]→1, [2,4]→3, [4,6]→5
        assert_eq!(out, vec![0.0, 1.0, 3.0, 5.0]);
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p oxideplot-core expr_tests`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/oxideplot-core/src/processing/expr.rs crates/oxideplot-core/src/processing/mod.rs
git commit -m "feat(core): processing::expr — expression evaluator, filter, rolling ops"
```

---

## Task 5: `processing::spectral` — PSD, spectrogram, sample-rate + `rustfft` on wasm

**Files:**
- Create: `crates/oxideplot-core/src/processing/spectral.rs`
- Modify: `crates/oxideplot-core/src/processing/mod.rs`, `crates/oxideplot-core/Cargo.toml`

**Interfaces:**
- Produces (in `oxideplot_core::processing::spectral`):
  - `pub fn compute_psd(vals: &[f64], fs: f64) -> (Vec<f64>, Vec<f64>)` — one-sided (frequency, power), DC bin dropped.
  - `pub fn compute_spectrogram(vals: &[f64], window: usize) -> (Vec<Vec<f64>>, usize)` — `(frames[frame][bin], bin_count)`.
  - `pub fn infer_sample_rate(data: &LoadedData) -> f64` — 1/median dt from the first datetime column, else 1.0.

- [ ] **Step 1: Add the dependency** — in `crates/oxideplot-core/Cargo.toml`, under `[dependencies]`, add:

```toml
rustfft = "6"
```

- [ ] **Step 2: Verify rustfft builds for wasm (the key risk)** —

Run: `rustup target add wasm32-unknown-unknown && cargo build -p oxideplot-core --target wasm32-unknown-unknown`
Expected: SUCCESS (compiles). If it FAILS on `rustfft`, STOP and escalate — the fallback is to replace `rustfft` with a pure-scalar FFT (`realfft`/`microfft`) and adjust `compute_psd`/`compute_spectrogram`; do not proceed until core builds for wasm.

- [ ] **Step 3: Declare the module** — add to `processing/mod.rs`:

```rust
pub mod spectral;
```

- [ ] **Step 4: Write the module** — create `spectral.rs`. Top imports:

```rust
use crate::data::loader::{column_to_timestamps, LoadedData};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use std::f64::consts::PI;
```

Copy **verbatim** from `oxideplot-mcp/src/main.rs`, `pub`-marking each:
- `fn compute_psd` (main.rs:116) → `pub fn compute_psd`.
- `fn compute_spectrogram` (main.rs:142) → `pub fn compute_spectrogram`.
- `fn infer_sample_rate` (main.rs:92) → `pub fn infer_sample_rate`, **changing the signature** from `(ds: &Dataset)` to `(data: &LoadedData)` and replacing every `ds.data` in the body with `data` (it only touches `ds.data.columns` and `ds.data.column_data`).

Do NOT move `heat_color` (main.rs:169) — colormap/rendering stays in the MCP for now.

- [ ] **Step 5: Write the failing tests** — append to `spectral.rs`:

```rust
#[cfg(test)]
mod spectral_tests {
    use super::*;

    fn argmax(v: &[f64]) -> usize {
        v.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).map(|(i, _)| i).unwrap()
    }

    #[test]
    fn psd_peaks_at_signal_frequency() {
        let fs = 100.0;
        let n = 4000;
        // 5 Hz sine.
        let sig: Vec<f64> = (0..n).map(|i| (2.0 * PI * 5.0 * i as f64 / fs).sin()).collect();
        let (freqs, power) = compute_psd(&sig, fs);
        let peak = freqs[argmax(&power)];
        assert!((peak - 5.0).abs() < 0.5, "peak at {peak}, expected ~5 Hz");
    }

    #[test]
    fn spectrogram_shape() {
        let sig: Vec<f64> = (0..2000).map(|i| (i as f64 / 10.0).sin()).collect();
        let (frames, bins) = compute_spectrogram(&sig, 256);
        assert_eq!(bins, 128);
        assert!(frames.len() > 5);
        assert!(frames.iter().all(|f| f.len() == 128));
    }

    #[test]
    fn sample_rate_from_datetime() {
        // ISO-8601 whole-second stamps at 2-second steps → 0.5 Hz. (Non-1.0 so it
        // is distinguishable from the no-datetime fallback of 1.0.) Uses the same
        // ISO format the MCP verified via column_to_timestamps.
        let stamps: Vec<String> = (0..20)
            .map(|i| format!("2026-07-02T16:00:{:02}Z", i * 2))
            .collect();
        let d = LoadedData {
            columns: vec!["t".into(), "v".into()],
            column_data: vec![stamps, vec!["0".into(); 20]],
            row_count: 20,
        };
        assert!((infer_sample_rate(&d) - 0.5).abs() < 0.05);
    }
}
```

- [ ] **Step 6: Run to verify pass**

Run: `cargo test -p oxideplot-core spectral_tests`
Expected: PASS (3 tests).

- [ ] **Step 7: Commit**

```bash
git add crates/oxideplot-core/src/processing/spectral.rs crates/oxideplot-core/src/processing/mod.rs crates/oxideplot-core/Cargo.toml Cargo.lock
git commit -m "feat(core): processing::spectral (PSD/spectrogram) + rustfft, wasm-verified"
```

---

## Task 6: `processing::histogram` — binning compute

**Files:**
- Create: `crates/oxideplot-core/src/processing/histogram.rs`
- Modify: `crates/oxideplot-core/src/processing/mod.rs`

**Interfaces:**
- Produces (in `oxideplot_core::processing::histogram`):
  - `pub struct Histogram { pub counts: Vec<usize>, pub bin_centers: Vec<f64>, pub min: f64, pub max: f64, pub n: usize }`
  - `pub fn histogram(vals: &[f64], nbins: usize) -> Option<Histogram>` — `None` if fewer than 2 finite values.

- [ ] **Step 1: Declare the module** — add `pub mod histogram;` to `processing/mod.rs`.

- [ ] **Step 2: Write the failing test** — create `histogram.rs`:

```rust
#[cfg(test)]
mod histogram_tests {
    use super::*;

    #[test]
    fn bimodal_two_peaks_empty_middle() {
        // 400 samples near 0, 600 near 25, nothing between.
        let mut v = vec![0.0f64; 400];
        v.extend(vec![25.0f64; 600]);
        let h = histogram(&v, 30).unwrap();
        assert_eq!(h.n, 1000);
        // the middle third of bins should be empty
        let mid = &h.counts[10..20];
        assert!(mid.iter().all(|&c| c == 0), "expected empty middle, got {mid:?}");
        assert!(h.counts[0] >= 400);
        assert!(h.counts[29] >= 600);
    }

    #[test]
    fn too_few_values_is_none() {
        assert!(histogram(&[1.0], 10).is_none());
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p oxideplot-core bimodal_two_peaks`
Expected: FAIL — module/function missing.

- [ ] **Step 4: Implement** — add above the test module in `histogram.rs`. Port the binning logic from the MCP `histogram` tool (`oxideplot-mcp/src/main.rs:2035`) — the finite-filter, `vmin`/`vmax`/`span`, the `counts` loop, and the `bin_centers` computation — into this pure function. Do NOT move the PNG rendering (that stays in the MCP tool):

```rust
/// Distribution of the finite values into `nbins` equal-width bins.
pub struct Histogram {
    pub counts: Vec<usize>,
    pub bin_centers: Vec<f64>,
    pub min: f64,
    pub max: f64,
    pub n: usize,
}

pub fn histogram(vals: &[f64], nbins: usize) -> Option<Histogram> {
    let finite: Vec<f64> = vals.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.len() < 2 {
        return None;
    }
    let nbins = nbins.clamp(2, 200);
    let min = finite.iter().copied().fold(f64::INFINITY, f64::min);
    let max = finite.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (max - min).max(1e-12);
    let mut counts = vec![0usize; nbins];
    for &v in &finite {
        let bi = (((v - min) / span * nbins as f64) as usize).min(nbins - 1);
        counts[bi] += 1;
    }
    let bin_centers = (0..nbins)
        .map(|i| min + span * (i as f64 + 0.5) / nbins as f64)
        .collect();
    Some(Histogram { counts, bin_centers, min, max, n: finite.len() })
}
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p oxideplot-core histogram_tests`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/oxideplot-core/src/processing/histogram.rs crates/oxideplot-core/src/processing/mod.rs
git commit -m "feat(core): processing::histogram (binning compute)"
```

---

## Task 7: `processing::qc` — Finding, health_check + helpers

**Files:**
- Create: `crates/oxideplot-core/src/processing/qc.rs`
- Modify: `crates/oxideplot-core/src/processing/mod.rs`

**Interfaces:**
- Consumes: `median_mad` (Task 1), `column_to_f64`, `column_to_timestamps`, `resolve_col` (Task 3).
- Produces (in `oxideplot_core::processing::qc`):
  - `pub enum Severity { High, Medium, Low }` with `pub fn rank(self) -> u8`, `#[derive(serde::Serialize)] #[serde(rename_all = "lowercase")]`.
  - `pub struct Finding` (serde-serializable; fields below).
  - `pub fn health_check(data: &LoadedData, numeric_cols: &[bool], lineage: Option<&std::collections::HashMap<String, Vec<String>>>) -> Vec<Finding>` (returned already sorted by severity rank).
  - Helper fns (`pub(crate)`): `longest_constant_run`, `group_runs`, `localize_changepoint`, `channel_role`, `shift_ratio_at`.

- [ ] **Step 1: Declare the module** — add `pub mod qc;` to `processing/mod.rs`.

- [ ] **Step 2: Write the types + helpers** — create `qc.rs`. Top:

```rust
use crate::data::loader::{column_to_f64, column_to_timestamps, resolve_col, LoadedData};
use crate::processing::statistics::median_mad;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    High,
    Medium,
    Low,
}
impl Severity {
    pub fn rank(self) -> u8 {
        match self {
            Severity::High => 0,
            Severity::Medium => 1,
            Severity::Low => 2,
        }
    }
}

/// A QC finding. Optional fields are omitted from JSON when None, so each
/// finding kind serializes to exactly the fields it uses (matching the MCP).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<[usize; 2]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_gap_row: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onset_row: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub culprit: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected: Option<Vec<String>>,
}

impl Finding {
    /// Minimal constructor; callers set optional fields with struct-update.
    fn new(severity: Severity, kind: &str, detail: String) -> Self {
        Finding {
            severity,
            kind: kind.to_string(),
            column: None,
            detail,
            row: None,
            rows: None,
            first_gap_row: None,
            onset_row: None,
            culprit: None,
            affected: None,
        }
    }
}
```

Then copy **verbatim** from `oxideplot-mcp/src/main.rs`, marking them `pub(crate)`:
- `fn longest_constant_run` (main.rs:735)
- `fn group_runs` (main.rs:640)
- `fn localize_changepoint` (main.rs:676)
- `fn channel_role` (main.rs:696)
- `fn shift_ratio_at` (main.rs:718)

(`median_mad` is now imported from `statistics`; do NOT redefine it here.)

- [ ] **Step 3: Port `health_check`** — port the body of the MCP `health_check` tool (`oxideplot-mcp/src/main.rs:1421`, the section from `let n_rows = ds.data.row_count;` through `findings.sort_by_key(...)`) into:

```rust
pub fn health_check(
    data: &LoadedData,
    numeric_cols: &[bool],
    lineage: Option<&HashMap<String, Vec<String>>>,
) -> Vec<Finding> {
    let n_rows = data.row_count;
    let mut findings: Vec<Finding> = Vec::new();
    let mut changepoints: Vec<(usize, usize, f64)> = Vec::new();
    // ... ported logic ...
    findings.sort_by_key(|f| f.severity.rank());
    findings
}
```

Mechanical replacements while porting:
- `ds.data` → `data`; `ds.numeric_cols` → `numeric_cols`.
- Each `findings.push((0/1, json!({ "severity": "high"/"medium", "kind": K, ...})))` → `findings.push(Finding { ..Finding::new(Severity::High/Medium, K, DETAIL) , /* optional fields */ })`. Field mapping:
  - `"column": name` → `column: Some(name.clone())`.
  - `"row": onset` → `row: Some(onset)`.
  - `"rows": [[rs, re], ...]` → `rows: Some(vec![[rs, re], ...])`.
  - `"first_gap_row": first` → `first_gap_row: first` (already `Option<usize>`).
  - `"onset_row": onset` → `onset_row: Some(onset)`.
  - `"culprit"/"affected": Vec<&String>` → collect to `Vec<String>`: `culprit: Some(culprit_names.iter().map(|s| s.to_string()).collect())`.
- The `lineage` lookup uses `lineage` (the param) instead of the tool's destructured field.
- The severity-rank sort replaces `findings.sort_by_key(|f| f.0)`.

- [ ] **Step 4: Write the failing tests** — append to `qc.rs`:

```rust
#[cfg(test)]
mod qc_tests {
    use super::*;

    // Build a dataset from parallel numeric columns of equal length.
    fn ds(cols: Vec<(&str, Vec<f64>)>) -> (LoadedData, Vec<bool>) {
        let n = cols[0].1.len();
        let columns: Vec<String> = cols.iter().map(|(nm, _)| nm.to_string()).collect();
        let column_data: Vec<Vec<String>> = cols
            .iter()
            .map(|(_, v)| v.iter().map(|x| x.to_string()).collect())
            .collect();
        let numeric = vec![true; columns.len()];
        (LoadedData { columns, column_data, row_count: n }, numeric)
    }

    #[test]
    fn flags_dead_glitch_and_regime() {
        let n = 600;
        let good: Vec<f64> = (0..n).map(|i| (i as f64 / 30.0).sin()).collect();
        let dead = vec![0.0f64; n];
        let mut glitchy: Vec<f64> = (0..n).map(|i| (i as f64 / 12.0).sin()).collect();
        glitchy[250] = 1e6; // single isolated spike
        let static_start: Vec<f64> =
            (0..n).map(|i| if i < 100 { 50.0 } else { 0.0 }).collect();
        let (d, nc) = ds(vec![
            ("good", good),
            ("dead", dead),
            ("glitchy", glitchy),
            ("static_start", static_start),
        ]);
        let f = health_check(&d, &nc, None);
        let has = |kind: &str, col: &str| {
            f.iter().any(|x| x.kind == kind && x.column.as_deref() == Some(col))
        };
        assert!(has("dead", "dead"));
        assert!(has("glitch", "glitchy"));
        // 100 contiguous outliers reclassify as a regime, NOT a glitch:
        assert!(has("outlier_regime", "static_start"));
        assert!(!has("glitch", "static_start"));
        // a lone spike must NOT manufacture a changepoint on 'glitchy':
        assert!(!f.iter().any(|x| x.kind == "changepoint" && x.column.as_deref() == Some("glitchy")));
    }

    #[test]
    fn traces_regime_to_raw_source() {
        let n = 1000;
        let carrier: Vec<f64> = (0..n).map(|i| (i as f64 / 25.0).sin()).collect();
        let raw_ax2: Vec<f64> =
            (0..n).map(|i| carrier[i] + if i > 500 { 10.0 } else { 0.0 }).collect();
        let cal_az: Vec<f64> = (0..n).map(|i| raw_ax2[i] * 0.7).collect();
        let (d, nc) = ds(vec![("raw_ax2", raw_ax2), ("calibrated_az", cal_az)]);
        let f = health_check(&d, &nc, None);
        let ev = f.iter().find(|x| x.kind == "regime_change_event").expect("an event");
        assert!(ev.culprit.as_ref().unwrap().iter().any(|c| c == "raw_ax2"));
    }
}
```

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p oxideplot-core qc_tests`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/oxideplot-core/src/processing/qc.rs crates/oxideplot-core/src/processing/mod.rs
git commit -m "feat(core): processing::qc — Finding + health_check (regime/lineage), tested"
```

---

## Task 8: Re-point `oxideplot-mcp` to core (remove private copies)

**Files:**
- Modify: `crates/oxideplot-mcp/src/main.rs`, `crates/oxideplot-mcp/Cargo.toml`

**Interfaces:**
- Consumes: everything produced in Tasks 1–7.

- [ ] **Step 1: Delete the moved functions from `main.rs`** — remove these definitions (now in core): `resolve_col` (48), `pearson` (59), `infer_sample_rate` (92), `compute_psd` (116), `compute_spectrogram` (142), `enum Rel`/`Ast`/`Tok`, `tokenize_expr`, `ExprParser` + impl, `parse_expr`, `collect_expr_cols`, `eval_expr`, `apply_filter`, `rolling_compute`, `group_runs`, `median_mad`, `localize_changepoint`, `channel_role`, `shift_ratio_at`, `longest_constant_run`, `minmax_envelope`. KEEP `heat_color`, `csv_escape`, `draw_text`, `glyph5x7`, `fmt_*`, and the render/tool code.

- [ ] **Step 2: Add imports** — near the top of `main.rs`, add:

```rust
use oxideplot_core::data::loader::resolve_col;
use oxideplot_core::processing::downsampling::minmax_envelope;
use oxideplot_core::processing::expr::{apply_filter, collect_expr_cols, eval_expr, parse_expr, rolling_compute};
use oxideplot_core::processing::histogram::histogram as core_histogram;
use oxideplot_core::processing::qc::health_check as core_health_check;
use oxideplot_core::processing::spectral::{compute_psd, compute_spectrogram, infer_sample_rate};
use oxideplot_core::processing::statistics::pearson;
```

Remove the now-unused `use rustfft::...;` and `use std::f64::consts::PI;` (if PI is unused after the move) and any other imports that dangle.

- [ ] **Step 3: Adapt call sites** —
  - `infer_sample_rate(ds)` → `infer_sample_rate(&ds.data)` (in `spectrum` and `spectrogram` tools).
  - The `health_check` tool: replace its inlined body with `let findings = core_health_check(&ds.data, &ds.numeric_cols, lineage.as_ref());` then serialize: `json!({ "dataset_id": dataset_id, "n_rows": ds.data.row_count, "n_findings": findings.len(), "findings": serde_json::to_value(&findings).unwrap() })`. The `Finding` serde shape matches the previous hand-built JSON.
  - The `histogram` tool: replace the inlined binning with `let h = core_histogram(&vals, nbins).ok_or_else(|| McpError::internal_error("need at least 2 finite values".into(), None))?;` and use `h.counts`, `h.bin_centers`, `h.min`, `h.max`, `h.n` in the render + text (rendering code unchanged).
  - `derive_column` (expr path) and `query_data`/`export_csv` (filter): the `parse_expr`/`collect_expr_cols`/`eval_expr`/`apply_filter`/`rolling_compute` calls now resolve to the imported core fns — no code change beyond the import.
  - `render_graph`: `minmax_envelope(...)` now resolves to the core import — no change.

- [ ] **Step 4: Remove the MCP's rustfft dep** — in `crates/oxideplot-mcp/Cargo.toml`, delete the `rustfft = "6"` line.

- [ ] **Step 5: Build the MCP**

Run: `cargo build -p oxideplot-mcp`
Expected: SUCCESS, no warnings about unused imports (fix any dangling imports revealed).

- [ ] **Step 6: Parity smoke-test the running MCP** — self-contained (generates its own data, drives the MCP over stdio, asserts core-backed tools still work). Write `scratch/mcp_smoke.py`:

```python
import subprocess, json, sys, os, math

BIN = sys.argv[1]
os.makedirs("scratch", exist_ok=True)
# tiny dataset: a dead column, a 5 Hz sine sampled at 100 Hz.
with open("scratch/smoke.csv", "w") as f:
    f.write("dead,sig\n")
    for i in range(4000):
        f.write("0,%.5f\n" % math.sin(2 * math.pi * 5 * i / 100.0))

p = subprocess.Popen([BIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL, text=True, bufsize=1)
_id = 0
def send(o): p.stdin.write(json.dumps(o) + "\n"); p.stdin.flush()
def rd():
    while True:
        l = p.stdout.readline()
        if l == "": return None
        if l.strip(): return json.loads(l)
def call(m, pa=None):
    global _id; _id += 1
    send({"jsonrpc": "2.0", "id": _id, "method": m, "params": pa or {}})
    while True:
        r = rd()
        if r is None or r.get("id") == _id: return r
def tool(n, a):
    r = call("tools/call", {"name": n, "arguments": a})
    assert "error" not in r, f"{n}: {r.get('error')}"
    return json.loads(r["result"]["content"][0]["text"])

call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "c", "version": "0"}})
send({"jsonrpc": "2.0", "method": "notifications/initialized"})
ds = tool("load_csv", {"path": os.path.abspath("scratch/smoke.csv")})["dataset_id"]

hc = tool("health_check", {"dataset_id": ds})
assert any(f["kind"] == "dead" and f.get("column") == "dead" for f in hc["findings"]), "health_check lost the dead finding"

sp = tool("spectrum", {"dataset_id": ds, "column": "sig", "sample_rate": 100.0})
top = sp["peaks"][0]["frequency_hz"]
assert abs(top - 5.0) < 0.5, f"spectrum peak {top}, expected ~5 Hz"

print("MCP smoke OK — health_check + spectrum still work via core")
p.stdin.close(); p.wait(timeout=5)
```

Run: `python scratch/mcp_smoke.py ./target/debug/oxideplot-mcp.exe`
Expected: prints `MCP smoke OK …` and exits 0 (asserts the dead finding and the 5 Hz spectrum peak, proving the core-backed tools are intact).

- [ ] **Step 7: Commit**

```bash
git add crates/oxideplot-mcp/src/main.rs crates/oxideplot-mcp/Cargo.toml Cargo.lock
git commit -m "refactor(mcp): call oxideplot-core for analysis compute (remove private copies)"
```

---

## Definition of done

- `cargo test -p oxideplot-core` passes (Tasks 1–7 suites).
- `cargo build -p oxideplot-core --target wasm32-unknown-unknown` succeeds (rustfft on wasm).
- `cargo build -p oxideplot-mcp` succeeds; the MCP's `health_check`, `spectrum`, `histogram`, `derive_column`, and filtered `query_data` produce identical results to before (Task 8 smoke).
- No analysis algorithm is defined in two places: `oxideplot-mcp/src/main.rs` contains only rendering/tool glue for these features; the compute lives in `oxideplot-core`.
