# Phase 9 — Interpolation / Resampling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`).

**Goal:** Resample a series onto a uniform N-point x-grid via linear / nearest / natural-cubic-spline interpolation, producing a derived series — added through the existing fx picker as a `resample` transform.

**Architecture:** New tested `oxideplot-core::processing::interpolation` (kernels + `resample`); `add_transform` gains a `resample` kind (params method + points); the fx picker gains a "Resample" option. Reuses Phase 8's derived-series machinery.

**Tech Stack:** Rust (oxideplot-core + oxideplot-wasm), Svelte 5 + TS.

## Global Constraints
- Branch `tauri-migration`. Interp math in core (native-tested); wasm wraps. Core builds native+wasm. No egui/polars in core.
- Source xs assumed ascending; non-finite points dropped before fitting; degenerate (<2 finite pts or n<2) returns input unchanged, no panic.
- `src/lib/wasm/` generated/gitignored — `build:wasm` before `build`/`tauri dev`. Theme via CSS vars. One commit per task.

---

## File structure
- `crates/oxideplot-core/src/processing/interpolation.rs` — NEW: `Method`, `resample`, kernels + tests.
- `crates/oxideplot-core/src/processing/mod.rs` — MODIFY: `pub mod interpolation;`.
- `crates/oxideplot-wasm/src/lib.rs` — MODIFY: `TransformParams` += `method`/`points`; `add_transform` `resample` arm.
- `src/lib/components/SeriesList.svelte` — MODIFY: "Resample" option + method/points params.

---

## Task 1: Core interpolation + resample + tests

**Files:** Create `crates/oxideplot-core/src/processing/interpolation.rs`; modify `processing/mod.rs` (`pub mod interpolation;`).

**Interfaces — Produces (Task 2):** `pub enum Method { Linear, Nearest, Cubic }` and `pub fn resample(xs: &[f64], ys: &[f64], n: usize, method: Method) -> (Vec<f64>, Vec<f64>)`.

- [ ] **Step 1: Write failing tests** (top of `interpolation.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linear_resample_of_line() {
        // y = 2x sampled at 0,2,4 → resampled to 5 pts over [0,4] = 0,1,2,3,4
        let (gx, gy) = resample(&[0.0,2.0,4.0], &[0.0,4.0,8.0], 5, Method::Linear);
        assert_eq!(gx, vec![0.0,1.0,2.0,3.0,4.0]);
        for (x,y) in gx.iter().zip(gy.iter()) { assert!((y - 2.0*x).abs() < 1e-9, "x={x} y={y}"); }
    }
    #[test]
    fn nearest_picks_closest_sample() {
        // samples at x=0(y=10), x=10(y=20); grid point near 0 → 10, near 10 → 20
        let (gx, gy) = resample(&[0.0,10.0], &[10.0,20.0], 11, Method::Nearest);
        assert_eq!(gx.len(), 11);
        assert_eq!(gy[0], 10.0);            // x=0
        assert_eq!(*gy.last().unwrap(), 20.0); // x=10
        assert_eq!(gy[1], 10.0);            // x=1 nearer 0
        assert_eq!(gy[9], 20.0);            // x=9 nearer 10
    }
    #[test]
    fn cubic_passes_through_sample_points() {
        let sx = vec![0.0, 1.0, 2.0, 3.0];
        let sy = vec![0.0, 1.0, 8.0, 27.0]; // y=x^3 samples
        // resample to 4 pts == the original x grid → values match the samples
        let (gx, gy) = resample(&sx, &sy, 4, Method::Cubic);
        assert_eq!(gx, sx);
        for (got, want) in gy.iter().zip(sy.iter()) { assert!((got - want).abs() < 1e-9, "got {got} want {want}"); }
    }
    #[test]
    fn resample_count_and_endpoints() {
        let (gx, _) = resample(&[1.0, 5.0, 9.0], &[0.0, 1.0, 0.0], 100, Method::Linear);
        assert_eq!(gx.len(), 100);
        assert_eq!(gx[0], 1.0);
        assert_eq!(*gx.last().unwrap(), 9.0);
    }
    #[test]
    fn degenerate_returns_input() {
        let (gx, gy) = resample(&[1.0], &[2.0], 50, Method::Linear);
        assert_eq!((gx, gy), (vec![1.0], vec![2.0]));
    }
}
```

- [ ] **Step 2: Run → FAIL** — `cargo test -p oxideplot-core interpolation::`.

- [ ] **Step 3: Implement** (prepend to `interpolation.rs`):

```rust
//! Resample a series onto a uniform x-grid by linear / nearest / natural-cubic
//! interpolation. Pure + native-tested; the wasm layer wraps `resample`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method { Linear, Nearest, Cubic }

/// Index `i` with `sx[i] <= x <= sx[i+1]`, clamped to `[0, len-2]`.
fn bracket(sx: &[f64], x: f64) -> usize {
    match sx.binary_search_by(|v| v.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Less)) {
        Ok(i) => i.min(sx.len() - 2),
        Err(i) => i.saturating_sub(1).min(sx.len() - 2),
    }
}

fn interp_linear(sx: &[f64], sy: &[f64], x: f64) -> f64 {
    let i = bracket(sx, x);
    let (x0, x1, y0, y1) = (sx[i], sx[i + 1], sy[i], sy[i + 1]);
    if x1 == x0 { y0 } else { y0 + (y1 - y0) * (x - x0) / (x1 - x0) }
}

fn interp_nearest(sx: &[f64], sy: &[f64], x: f64) -> f64 {
    let i = bracket(sx, x);
    if (x - sx[i]).abs() <= (sx[i + 1] - x).abs() { sy[i] } else { sy[i + 1] }
}

/// Natural cubic spline second derivatives (m[0]=m[n-1]=0) via the Thomas algorithm.
fn cubic_second_derivs(sx: &[f64], sy: &[f64]) -> Vec<f64> {
    let n = sx.len();
    let mut m = vec![0.0; n];
    if n < 3 { return m; }
    let mut c = vec![0.0; n]; // modified superdiagonal
    let mut d = vec![0.0; n]; // modified rhs
    for i in 1..n - 1 {
        let h0 = sx[i] - sx[i - 1];
        let h1 = sx[i + 1] - sx[i];
        let a = h0;
        let b = 2.0 * (h0 + h1);
        let cc = h1;
        let dd = 6.0 * ((sy[i + 1] - sy[i]) / h1 - (sy[i] - sy[i - 1]) / h0);
        let denom = b - a * c[i - 1];
        c[i] = cc / denom;
        d[i] = (dd - a * d[i - 1]) / denom;
    }
    for i in (1..n - 1).rev() {
        m[i] = d[i] - c[i] * m[i + 1];
    }
    m
}

fn interp_cubic(sx: &[f64], sy: &[f64], m: &[f64], x: f64) -> f64 {
    let i = bracket(sx, x);
    let h = sx[i + 1] - sx[i];
    if h == 0.0 { return sy[i]; }
    let a = (sx[i + 1] - x) / h;
    let b = (x - sx[i]) / h;
    a * sy[i] + b * sy[i + 1] + ((a * a * a - a) * m[i] + (b * b * b - b) * m[i + 1]) * (h * h) / 6.0
}

/// Resample `(xs,ys)` onto `n` evenly-spaced x over `[min,max]` of the finite,
/// ascending source points. Returns `(grid_xs, interp_ys)`.
pub fn resample(xs: &[f64], ys: &[f64], n: usize, method: Method) -> (Vec<f64>, Vec<f64>) {
    let (sx, sy): (Vec<f64>, Vec<f64>) = xs.iter().zip(ys.iter())
        .filter(|(x, y)| x.is_finite() && y.is_finite())
        .map(|(&x, &y)| (x, y))
        .unzip();
    if sx.len() < 2 || n < 2 {
        return (xs.to_vec(), ys.to_vec());
    }
    let x0 = sx[0];
    let x1 = sx[sx.len() - 1];
    let m = if method == Method::Cubic { Some(cubic_second_derivs(&sx, &sy)) } else { None };
    let grid: Vec<f64> = (0..n).map(|i| x0 + (x1 - x0) * (i as f64) / ((n - 1) as f64)).collect();
    let out: Vec<f64> = grid.iter().map(|&x| match method {
        Method::Linear => interp_linear(&sx, &sy, x),
        Method::Nearest => interp_nearest(&sx, &sy, x),
        Method::Cubic => interp_cubic(&sx, &sy, m.as_ref().unwrap(), x),
    }).collect();
    (grid, out)
}
```

Add `pub mod interpolation;` to `processing/mod.rs`.

- [ ] **Step 4: Run → PASS** — `cargo test -p oxideplot-core interpolation::` (5 tests) + whole suite `cargo test -p oxideplot-core` green + `cargo build -p oxideplot-core --target wasm32-unknown-unknown`.

- [ ] **Step 5: Commit** — `git add crates/oxideplot-core/src/processing/interpolation.rs crates/oxideplot-core/src/processing/mod.rs && git commit -m "feat(core): resample/interpolation (linear/nearest/cubic) + tests"`

---

## Task 2: WASM `resample` transform kind

**Files:** Modify `crates/oxideplot-wasm/src/lib.rs`.

**Interfaces — Consumes:** `oxideplot_core::processing::interpolation::{Method, resample}`; the existing `add_transform` structure (TransformParams, derived-series build with `palette_color`/`compute_y_bounds`, `auto_fit`).

- [ ] **Step 1:** Extend `TransformParams` with `method: Option<String>` and `points: Option<usize>`. In `add_transform`'s match, add:

```rust
"resample" => {
    let method = match p.method.as_deref() {
        Some("nearest") => interpolation::Method::Nearest,
        Some("cubic")   => interpolation::Method::Cubic,
        _ => interpolation::Method::Linear,
    };
    let n = p.points.unwrap_or(500).max(2);
    let (new_xs, new_ys) = interpolation::resample(&src.xs, &src.ys, n, method);
    let mlabel = p.method.as_deref().unwrap_or("linear");
    // NOTE: resample produces NEW xs (not the source xs) — use new_xs for the derived series.
    (new_xs, new_ys, format!("{base} · resample({mlabel}, {n})"))
}
```
Because resample returns new xs, the `add_transform` arm must yield the new xs too. If the existing arms return only `(new_ys, label)` and reuse `src.xs` for the derived series, **refactor the match to yield `(new_xs, new_ys, label)`** where all the Phase-8 arms return `src.xs.clone()` as `new_xs` and `resample` returns its grid — then build the `SourceSeries` from the arm's `new_xs`/`new_ys`. Import `use oxideplot_core::processing::interpolation;`.

- [ ] **Step 2: Verify** — `npm run build:wasm` → `npm run build` → `cargo build -p oxideplot-core --target wasm32-unknown-unknown`, all succeed. (No new renderer method — `addTransform` already passes an arbitrary params object.)

- [ ] **Step 3: Commit** — `git add crates/oxideplot-wasm/src/lib.rs && git commit -m "feat: resample transform kind in add_transform (new x-grid)"`

---

## Task 3: "Resample" in the fx picker

**Files:** Modify `src/lib/components/SeriesList.svelte`.

- [ ] **Step 1:** Add `<option value="resample">Resample</option>` to the fx transform `<select>`. Add picker state `fxMethod: string = 'linear'` and `fxPoints: number = 500` (reset on picker open alongside the others). When `fxKind === 'resample'`, show: a **method** `<select>` (Linear/Nearest/Cubic spline → `linear`/`nearest`/`cubic`, bound to `fxMethod`) and a **points** number input (`min=2`, default 500, bound to `fxPoints`). On Apply with `resample`: `params = { method: fxMethod, points: fxPoints }` → `renderer.addTransform(i, 'resample', params)`. Style consistent with the existing picker (CSS vars).

- [ ] **Step 2: Verify** — `npm run build:wasm` → `npm run build` (no errors) → `cargo build` (workspace). Visual (human): fx → Resample → pick method + points → Apply → a resampled derived series appears (e.g. cubic produces a smooth curve through the points; nearest a step-like one); chaining + remove work.

- [ ] **Step 3: Commit** — `git add src/lib/components/SeriesList.svelte && git commit -m "feat: resample option in fx picker (method + points)"`

---

## Self-Review
- Spec coverage: linear/nearest/cubic + resample-to-N (Task 1) ✓; resample kind in add_transform with new x-grid (Task 2) ✓; fx picker Resample option + method/points (Task 3) ✓; reuses derived-series infra ✓; degenerate-safe ✓.
- Placeholders: Task 1 complete code + real tests (incl. cubic-through-points). Task 2 calls out the (xs,ys,label) match refactor explicitly. Task 3 gives exact option/param wiring. No "TBD".
- Type consistency: `Method`/`resample` (Task 1) used verbatim in Task 2; `TransformParams { window, mode, method, points }` ↔ JS `{ method, points }` (Task 3); `resample` kind string consistent (Task 2 arm ↔ Task 3 option); the match now yields `(new_xs, new_ys, label)` consistently across all arms.
