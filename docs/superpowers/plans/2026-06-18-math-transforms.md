# Phase 8 — Math Transforms Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Per-series math transforms (moving average, derivative, integral, normalize, abs/log/sqrt) that each produce a new derived series, applied via a per-series "fx" picker.

**Architecture:** Pure transform functions in `oxideplot-core::processing::math_ops` (native-tested); a wasm `add_transform` builds a derived `SourceSeries` from a source series' data via those functions and re-renders; a per-series "fx" picker in `SeriesList.svelte` drives it.

**Tech Stack:** Rust (oxideplot-core + oxideplot-wasm), Svelte 5 + TypeScript.

## Global Constraints

- Branch: `tauri-migration`.
- Transform math lives in `oxideplot-core` (native `cargo test`); wasm wraps it.
- `oxideplot-core` builds native AND wasm32. No egui in core. No polars.
- Derived series are first-class `SourceSeries` (visible/remove/reorder/transform-again); computed once at add time; plot-only (not added to the table's file columns).
- `src/lib/wasm/` is generated/gitignored — `npm run build:wasm` before `npm run build`/`tauri dev`.
- Theme via existing CSS vars. Commit per task; never commit a non-compiling tree.

---

## File structure

- `crates/oxideplot-core/src/processing/math_ops.rs` — MODIFY: add transform fns + tests.
- `crates/oxideplot-wasm/src/lib.rs` — MODIFY: `add_transform` method.
- `src/lib/renderer.ts` — MODIFY: `addTransform` wrapper.
- `src/lib/components/SeriesList.svelte` — MODIFY: per-series "fx" picker.

---

## Task 1: Core transform functions + tests

**Files:** Modify `crates/oxideplot-core/src/processing/math_ops.rs` (read it first — reuse anything already present; add what's missing).

**Interfaces — Produces (Task 2):**
- `pub fn moving_average(ys: &[f64], window: usize) -> Vec<f64>`
- `pub fn derivative(xs: &[f64], ys: &[f64]) -> Vec<f64>`
- `pub fn integral(xs: &[f64], ys: &[f64]) -> Vec<f64>`
- `pub fn normalize(ys: &[f64], zscore: bool) -> Vec<f64>`
- `pub fn map_abs(ys: &[f64]) -> Vec<f64>`, `pub fn map_ln(ys: &[f64]) -> Vec<f64>`, `pub fn map_sqrt(ys: &[f64]) -> Vec<f64>`

- [ ] **Step 1: Write failing tests** (append an inline `#[cfg(test)] mod transform_tests` to `math_ops.rs`):

```rust
#[cfg(test)]
mod transform_tests {
    use super::*;

    #[test]
    fn moving_average_centered_window3() {
        let ys = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        // centered window 3 (half=1), clamped at edges:
        // i0:{1,2}=1.5  i1:{1,2,3}=2  i2:{2,3,4}=3  i3:{3,4,5}=4  i4:{4,5}=4.5
        assert_eq!(moving_average(&ys, 3), vec![1.5, 2.0, 3.0, 4.0, 4.5]);
    }
    #[test]
    fn moving_average_window1_is_identity() {
        let ys = vec![1.0, 9.0, 4.0];
        assert_eq!(moving_average(&ys, 1), ys);
    }
    #[test]
    fn derivative_of_linear_is_constant_slope() {
        let xs = vec![0.0, 1.0, 2.0, 3.0];
        let ys = vec![0.0, 2.0, 4.0, 6.0]; // slope 2
        for v in derivative(&xs, &ys) { assert!((v - 2.0).abs() < 1e-9, "got {v}"); }
    }
    #[test]
    fn integral_of_constant_is_cumulative() {
        let xs = vec![0.0, 1.0, 2.0, 3.0];
        let ys = vec![1.0, 1.0, 1.0, 1.0];
        assert_eq!(integral(&xs, &ys), vec![0.0, 1.0, 2.0, 3.0]);
    }
    #[test]
    fn normalize_minmax_maps_to_unit() {
        assert_eq!(normalize(&[10.0, 20.0, 30.0], false), vec![0.0, 0.5, 1.0]);
    }
    #[test]
    fn normalize_zscore_has_zero_mean() {
        let z = normalize(&[1.0, 2.0, 3.0], true);
        let mean: f64 = z.iter().sum::<f64>() / z.len() as f64;
        assert!(mean.abs() < 1e-9);
    }
    #[test]
    fn unary_ops() {
        assert_eq!(map_abs(&[-1.0, 2.0]), vec![1.0, 2.0]);
        assert!(map_ln(&[-1.0])[0].is_nan());
        assert_eq!(map_sqrt(&[4.0, 9.0]), vec![2.0, 3.0]);
    }
}
```

- [ ] **Step 2: Run to verify they fail** — `cargo test -p oxideplot-core transform_tests` → FAIL (functions not defined / mismatch).

- [ ] **Step 3: Implement** (add to `math_ops.rs`):

```rust
/// Centered simple moving average, window >= 1, clamped at the edges.
pub fn moving_average(ys: &[f64], window: usize) -> Vec<f64> {
    let n = ys.len();
    let w = window.max(1);
    let half = w / 2;
    (0..n).map(|i| {
        let lo = i.saturating_sub(half);
        let hi = (i + half + 1).min(n);
        let slice = &ys[lo..hi];
        slice.iter().sum::<f64>() / slice.len() as f64
    }).collect()
}

/// Numerical dy/dx: central difference interior, forward/backward at the ends.
pub fn derivative(xs: &[f64], ys: &[f64]) -> Vec<f64> {
    let n = ys.len().min(xs.len());
    if n == 0 { return vec![]; }
    if n == 1 { return vec![0.0]; }
    (0..n).map(|i| {
        if i == 0 {
            (ys[1] - ys[0]) / (xs[1] - xs[0])
        } else if i == n - 1 {
            (ys[n - 1] - ys[n - 2]) / (xs[n - 1] - xs[n - 2])
        } else {
            (ys[i + 1] - ys[i - 1]) / (xs[i + 1] - xs[i - 1])
        }
    }).collect()
}

/// Cumulative trapezoidal integral; Y[0] = 0.
pub fn integral(xs: &[f64], ys: &[f64]) -> Vec<f64> {
    let n = ys.len().min(xs.len());
    let mut out = Vec::with_capacity(n);
    let mut acc = 0.0;
    for i in 0..n {
        if i > 0 {
            acc += (xs[i] - xs[i - 1]) * (ys[i] + ys[i - 1]) / 2.0;
        }
        out.push(acc);
    }
    out
}

/// Normalize to [0,1] (min-max) or zero-mean/unit-std (z-score).
pub fn normalize(ys: &[f64], zscore: bool) -> Vec<f64> {
    let finite: Vec<f64> = ys.iter().copied().filter(|v| v.is_finite()).collect();
    if finite.is_empty() { return ys.to_vec(); }
    if zscore {
        let mean = finite.iter().sum::<f64>() / finite.len() as f64;
        let var = finite.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / finite.len() as f64;
        let std = var.sqrt();
        if std == 0.0 { return ys.iter().map(|_| 0.0).collect(); }
        ys.iter().map(|&v| (v - mean) / std).collect()
    } else {
        let min = finite.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = finite.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let range = max - min;
        if range == 0.0 { return ys.iter().map(|_| 0.5).collect(); }
        ys.iter().map(|&v| (v - min) / range).collect()
    }
}

pub fn map_abs(ys: &[f64]) -> Vec<f64> { ys.iter().map(|v| v.abs()).collect() }
pub fn map_ln(ys: &[f64]) -> Vec<f64> { ys.iter().map(|v| v.ln()).collect() }
pub fn map_sqrt(ys: &[f64]) -> Vec<f64> { ys.iter().map(|v| v.sqrt()).collect() }
```

- [ ] **Step 4: Run to verify pass** — `cargo test -p oxideplot-core transform_tests` → PASS (7 tests). Also `cargo build -p oxideplot-core --target wasm32-unknown-unknown` → succeeds.

- [ ] **Step 5: Commit** — `git add crates/oxideplot-core/src/processing/math_ops.rs && git commit -m "feat(core): math transforms (MA/derivative/integral/normalize/abs/log/sqrt) + tests"`

---

## Task 2: WASM `add_transform` + renderer wrapper

**Files:** Modify `crates/oxideplot-wasm/src/lib.rs`, `src/lib/renderer.ts`.

**Interfaces — Consumes (Task 1):** the `math_ops` fns above; `OxidePlot.sources: Vec<SourceSeries { name, x_name, visible, xs, ys, color, draw_mode, y_min, y_max }>`, the palette/color logic used by `set_series`, `rebuild_visible()`, `auto_fit()`, and the per-source `y_min/y_max` computation used in `set_series`.
**Produces (Task 3):** `renderer.addTransform`.

- [ ] **Step 1: Add `add_transform`** to `OxidePlot` (`mod wasm_impl`). Import `use oxideplot_core::processing::math_ops;`.

```rust
#[derive(serde::Deserialize, Default)]
struct TransformParams { window: Option<usize>, mode: Option<String> }

#[wasm_bindgen]
pub fn add_transform(&mut self, source_index: usize, kind: String, params: JsValue) -> Result<(), JsValue> {
    let src = self.sources.get(source_index)
        .ok_or_else(|| JsValue::from_str("source index out of range"))?;
    let p: TransformParams = if params.is_null() || params.is_undefined() {
        TransformParams::default()
    } else {
        serde_wasm_bindgen::from_value(params).map_err(|e| JsValue::from_str(&e.to_string()))?
    };
    let xs = src.xs.clone();
    let base = src.name.clone();
    let (new_ys, label) = match kind.as_str() {
        "moving_average" => { let w = p.window.unwrap_or(5).max(1); (math_ops::moving_average(&src.ys, w), format!("{base} · MA({w})")) }
        "derivative" => (math_ops::derivative(&src.xs, &src.ys), format!("d/dx({base})")),
        "integral"   => (math_ops::integral(&src.xs, &src.ys), format!("∫({base})")),
        "normalize"  => { let z = p.mode.as_deref() == Some("zscore"); (math_ops::normalize(&src.ys, z), if z { format!("z({base})") } else { format!("norm({base})") }) }
        "abs"  => (math_ops::map_abs(&src.ys),  format!("|{base}|")),
        "log"  => (math_ops::map_ln(&src.ys),   format!("log({base})")),
        "sqrt" => (math_ops::map_sqrt(&src.ys), format!("√({base})")),
        other => return Err(JsValue::from_str(&format!("unknown transform: {other}"))),
    };
    // color: cycle the same palette set_series uses, by current series count
    let color = /* same palette indexing helper used in set_series, index = self.sources.len() */;
    // y_min/y_max over finite new_ys (same rule used in set_series)
    let (y_min, y_max) = /* compute as in set_series for a column of values */;
    self.sources.push(SourceSeries {
        name: label, x_name: src.x_name.clone(), visible: true,
        xs, ys: new_ys, color, draw_mode: DrawMode::Lines, y_min, y_max,
    });
    self.rebuild_visible();
    self.auto_fit(); // re-fit + render so the new series is visible
    Ok(())
}
```
Fill the `color` and `y_min/y_max` blanks by reusing the exact palette + min/max logic already in `set_series` (factor a tiny helper if cleaner). `auto_fit()` already calls `rebuild_visible()` + `render()`, so the explicit `rebuild_visible()` before it is harmless but you may drop it.

- [ ] **Step 2: renderer wrapper** (`src/lib/renderer.ts`):

```ts
addTransform(sourceIndex: number, kind: string, params: { window?: number; mode?: string } | null): void {
  this.plot!.add_transform(sourceIndex, kind, params);
}
```

- [ ] **Step 3: Verify** — `npm run build:wasm` (succeeds) → `npm run build` (succeeds) → `cargo build -p oxideplot-core --target wasm32-unknown-unknown` (succeeds). (Logic is tested in Task 1; this is wiring.)

- [ ] **Step 4: Commit** — `git add crates/oxideplot-wasm/src/lib.rs src/lib/renderer.ts && git commit -m "feat: wasm add_transform building derived series from core math_ops"`

---

## Task 3: Per-series "fx" picker in SeriesList

**Files:** Modify `src/lib/components/SeriesList.svelte` (+ App.svelte only if a refresh hook is needed).

**Interfaces — Consumes (Task 2):** `renderer.addTransform(sourceIndex, kind, params)`. The component already has `series: SeriesInfoEntry[]`, the `renderer`, and dispatches a `change` event the parent handles by refreshing the series list + view.

- [ ] **Step 1: Add the fx picker.** For each series row, add a small **"fx"** button. Clicking toggles an inline picker for that row (track `openFxIndex: number | null`). The picker has:
  - a transform `<select>` with options: Moving average, Derivative, Integral, Normalize, Abs, Log, Sqrt (values `moving_average`/`derivative`/`integral`/`normalize`/`abs`/`log`/`sqrt`).
  - a contextual param control: when `moving_average` is selected, a number input `window` (default 5, min 1); when `normalize` is selected, a `<select>` mode (`minmax`/`zscore`); otherwise none.
  - an **Apply** button → `renderer.addTransform(i, kind, params)` where `params = { window }` for MA, `{ mode }` for normalize, else `null`; then close the picker and `dispatch('change')` (parent refreshes the list — the derived series appears).
  - Wrap the `addTransform` call in try/catch; on error, surface it (reuse the existing error pattern or `console.error`).
- [ ] **Step 2: Style** the fx button + picker with the existing CSS vars (compact, consistent with the panel). The picker can be an inline expanding block under the row or a small popover; keep it simple and themed.
- [ ] **Step 3: Verify** — `npm run build:wasm` (run once) → `npm run build` (no errors) → `cargo build` (workspace) succeeds. Visual (human): click fx on a series → pick a transform (+param) → Apply → a new derived series appears in the list and on the plot (view re-fits); it's removable/toggleable; chaining (transform a derived series) works.
- [ ] **Step 4: Commit** — `git add src/lib/components/SeriesList.svelte src/App.svelte && git commit -m "feat: per-series fx transform picker"`

---

## Self-Review

**Spec coverage:** all 7 transforms (Task 1) ✓; per-series fx UX (Task 3) ✓; derived series first-class + named + colored + auto-fit (Task 2) ✓; core-tested logic, wasm wrapper (Tasks 1–2) ✓; chaining works because a derived series is a normal `SourceSeries` (Task 3 visual) ✓.

**Placeholder scan:** Task 1 has complete code + real tests. Task 2 leaves two clearly-scoped blanks (`color`, `y_min/y_max`) with the explicit instruction to reuse `set_series`'s exact existing logic — appropriate (don't duplicate that logic verbatim here; reuse it). Task 3 specifies exact option values, param rules, and the params object shape. No "TBD"/"handle edge cases".

**Type consistency:** `math_ops` fn names/signatures (Task 1) are called verbatim in Task 2; `add_transform(source_index, kind, params)` / `addTransform(sourceIndex, kind, params)` match across Tasks 2–3; transform `kind` string values are consistent (Task 2 match arms ↔ Task 3 select values); `TransformParams { window, mode }` ↔ the JS `{ window?, mode? }`.
