# Render Options Slice — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add three per-graph render options to the OxidePlot desktop app's Settings panel — **Autoscale** (Min/Max · Robust), **Y-scale** (Linear · Log), **Downsample** (Min/Max · LTTB · None) — that change how the live WebGPU plot fits, transforms, and decimates.

**Architecture:** Pure, testable compute goes in `oxideplot-core` (a `percentile` helper + a downsample-mode dispatch). The live render struct `oxideplot-wasm::OxidePlot` gains three fields + three `#[wasm_bindgen]` setters and applies them in `auto_fit` (Y-bounds: robust percentile + log10), `rebuild_visible` (downsample mode + log10 of points, *after* decimation), and `axis_ticks` (log-aware ticks). The Svelte layers copy the existing `normalized` toggle's end-to-end pattern. The wasm/Svelte layers aren't unit-testable (GPU glue), so they're verified by build/typecheck plus a final visual check in the running app.

**Tech Stack:** Rust (`oxideplot-core`, `oxideplot-wasm` via wasm-bindgen/wasm-pack), Svelte 5, WebGPU. Build wasm with `npm run build:wasm`.

## Global Constraints

- The live render path is `crates/oxideplot-wasm/src/lib.rs`'s `OxidePlot` struct — NOT the dead `state::graph_state`/`state::plot_view::{fit_to_data,auto_scale_y_to_visible}` methods (called nowhere in the app). All three toggles hook into `OxidePlot::{auto_fit, rebuild_visible, axis_ticks}`.
- Defaults per the approved design: Autoscale=Min/Max, Y-scale=Linear, Downsample=**Min/Max**. Note the Downsample default is a deliberate behavior change — today the app hardcodes LTTB; the new default is Min/Max (spike-safe), matching the MCP.
- Copy the `normalized` toggle's full path as the template: `Settings.svelte` control+dispatch → `App.svelte` mirror-var+handler+prop → `Graph.svelte` local-var+setter+getter → `renderer.ts` wrapper → `oxideplot-wasm` `set_*`.
- Log-Y rule (from the render-path map): decimate on the RAW signal first (whichever downsample mode), THEN map surviving points through `log10`, dropping non-positive survivors. The Y-bounds and axis ticks are computed in log10-space; `Axes.svelte` is scale-agnostic and needs no change.
- Never hand-edit the generated `src/lib/wasm/oxideplot_wasm.{js,d.ts}` — they regenerate from `npm run build:wasm`.
- Core tests: `cargo test -p oxideplot-core`. Native build check: `cargo build -p oxideplot-wasm`. Wasm build: `npm run build:wasm`. Type check: `npm run check` (svelte-check) if defined, else `npx tsc --noEmit`.

---

## File Structure

**Modify (core — testable):**
- `crates/oxideplot-core/src/processing/statistics.rs` — add `percentile`.
- `crates/oxideplot-core/src/processing/downsampling.rs` — add `DownsampleMode` + `downsample_for_view_mode`.

**Modify (wasm — build-verified):**
- `crates/oxideplot-wasm/src/lib.rs` — 2 new enums, 3 new `OxidePlot` fields + defaults, 3 new setters, and logic in `auto_fit` / `rebuild_visible` / `axis_ticks`.

**Modify (frontend — build/typecheck + visual):**
- `src/lib/renderer.ts` — 3 wrapper methods.
- `src/lib/components/Graph.svelte` — 3 local vars + setter/getter pairs.
- `src/App.svelte` — 3 mirror vars in `syncFromGraph`, 3 handlers, 3 `<Settings>` props/events.
- `src/lib/components/Settings.svelte` — 3 `<select>` controls + dispatched events.

Work on branch `tauri-migration`; commit each task.

---

## Task 1: Core — `percentile` helper

**Files:** Modify `crates/oxideplot-core/src/processing/statistics.rs`

**Interfaces:**
- Produces: `oxideplot_core::processing::statistics::percentile(sorted: &[f64], f: f64) -> f64` — the value at fraction `f∈[0,1]` of an already-ascending-sorted finite slice, by nearest-rank on `(len-1)*f`. Returns `f64::NAN` for an empty slice.

- [ ] **Step 1: Write the failing test** — append to `statistics.rs`:

```rust
#[cfg(test)]
mod percentile_tests {
    use super::*;

    #[test]
    fn percentile_basic() {
        let v = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]; // sorted, len 10
        assert_eq!(percentile(&v, 0.0), 0.0);
        assert_eq!(percentile(&v, 1.0), 9.0);
        // (len-1)*0.5 = 4.5 → round → index 5 → 5.0 (nearest-rank)
        assert_eq!(percentile(&v, 0.5), 5.0);
    }

    #[test]
    fn percentile_clips_outlier() {
        // 99 values in [0,1] plus one huge outlier; p99 must stay near 1, not the outlier.
        let mut v: Vec<f64> = (0..99).map(|i| i as f64 / 98.0).collect();
        v.push(1e9);
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(percentile(&v, 0.99) < 2.0, "p99 should ignore the lone 1e9 outlier");
    }

    #[test]
    fn percentile_empty_is_nan() {
        assert!(percentile(&[], 0.5).is_nan());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p oxideplot-core percentile_basic`
Expected: FAIL — `cannot find function 'percentile'`.

- [ ] **Step 3: Implement** — add above the test module:

```rust
/// Value at fraction `f` (0..1) of an ascending-sorted finite slice, nearest-rank.
/// Returns NaN for an empty slice.
pub fn percentile(sorted: &[f64], f: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = (((sorted.len() - 1) as f64) * f.clamp(0.0, 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p oxideplot-core percentile_tests`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/oxideplot-core/src/processing/statistics.rs
git commit -m "feat(core): add percentile helper to statistics"
```

---

## Task 2: Core — `DownsampleMode` + mode-aware view downsampling

**Files:** Modify `crates/oxideplot-core/src/processing/downsampling.rs`

**Interfaces:**
- Consumes: existing `lttb_downsample`, `minmax_envelope`, and the existing `downsample_for_view` (for its window-slicing logic — read it first).
- Produces:
  - `pub enum DownsampleMode { MinMax, Lttb, None }` with `pub fn parse(s: &str) -> DownsampleMode` (`"lttb"→Lttb`, `"none"→None`, anything else → `MinMax`).
  - `pub fn downsample_for_view_mode(x: &[f64], y: &[f64], view_min: f64, view_max: f64, max_points: usize, mode: DownsampleMode) -> (Vec<f64>, Vec<f64>)` — same visible-X windowing as `downsample_for_view`, then: `Lttb` → `lttb_downsample(.., max_points)`; `MinMax` → `minmax_envelope(.., max_points / 2)` (buckets = max_points/2 so output ≤ max_points, matching the LTTB point budget); `None` → the windowed slice unchanged. Leave the existing `downsample_for_view` untouched.

- [ ] **Step 1: Read the existing windowing** — open `crates/oxideplot-core/src/processing/downsampling.rs` and read `downsample_for_view` (~line 74): it binary-searches `[view_min, view_max]` to a visible slice, then LTTBs if the slice exceeds `max_points`. Reuse that windowing shape in the new function.

- [ ] **Step 2: Write the failing tests** — append to `downsampling.rs`:

```rust
#[cfg(test)]
mod ds_mode_tests {
    use super::*;

    fn ramp(n: usize) -> (Vec<f64>, Vec<f64>) {
        let x: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let y = x.clone();
        (x, y)
    }

    #[test]
    fn none_returns_windowed_slice_untouched() {
        let (x, y) = ramp(1000);
        // window to x in [100, 200] → 101 points, no decimation
        let (ox, _) = downsample_for_view_mode(&x, &y, 100.0, 200.0, 50, DownsampleMode::None);
        assert_eq!(ox.first().copied(), Some(100.0));
        assert_eq!(ox.last().copied(), Some(200.0));
        assert_eq!(ox.len(), 101);
    }

    #[test]
    fn minmax_keeps_spike_within_budget() {
        let n = 5000;
        let x: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let mut y = vec![0.0f64; n];
        y[2500] = 1e6;
        let (_, oy) = downsample_for_view_mode(&x, &y, 0.0, n as f64, 400, DownsampleMode::MinMax);
        assert!(oy.iter().cloned().fold(0.0, f64::max) >= 1e6, "spike must survive minmax");
        assert!(oy.len() <= 400, "output within the point budget");
    }

    #[test]
    fn lttb_decimates_to_budget() {
        let (x, y) = ramp(5000);
        let (ox, _) = downsample_for_view_mode(&x, &y, 0.0, 5000.0, 400, DownsampleMode::Lttb);
        assert!(ox.len() <= 400 && ox.len() > 3);
    }

    #[test]
    fn parse_modes() {
        assert!(matches!(DownsampleMode::parse("lttb"), DownsampleMode::Lttb));
        assert!(matches!(DownsampleMode::parse("none"), DownsampleMode::None));
        assert!(matches!(DownsampleMode::parse("minmax"), DownsampleMode::MinMax));
        assert!(matches!(DownsampleMode::parse("garbage"), DownsampleMode::MinMax));
    }
}
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p oxideplot-core parse_modes`
Expected: FAIL — `DownsampleMode` / `downsample_for_view_mode` not found.

- [ ] **Step 4: Implement** — add to `downsampling.rs`:

```rust
/// Which decimation to use when building the visible render series.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DownsampleMode {
    MinMax,
    Lttb,
    None,
}

impl DownsampleMode {
    pub fn parse(s: &str) -> DownsampleMode {
        match s {
            "lttb" => DownsampleMode::Lttb,
            "none" => DownsampleMode::None,
            _ => DownsampleMode::MinMax,
        }
    }
}

/// Like `downsample_for_view`, but selects the decimation strategy.
pub fn downsample_for_view_mode(
    x: &[f64],
    y: &[f64],
    view_min: f64,
    view_max: f64,
    max_points: usize,
    mode: DownsampleMode,
) -> (Vec<f64>, Vec<f64>) {
    // Visible-X window: partition_point mirrors downsample_for_view's slicing.
    let lo = x.partition_point(|&v| v < view_min);
    let hi = x.partition_point(|&v| v <= view_max);
    let (xw, yw) = (&x[lo..hi], &y[lo..hi]);
    if xw.len() <= max_points || max_points < 3 {
        return (xw.to_vec(), yw.to_vec());
    }
    match mode {
        DownsampleMode::None => (xw.to_vec(), yw.to_vec()),
        DownsampleMode::Lttb => lttb_downsample(xw, yw, max_points),
        DownsampleMode::MinMax => minmax_envelope(xw, yw, max_points / 2),
    }
}
```

NOTE: confirm `downsample_for_view`'s actual windowing (it may use `binary_search`/a loop rather than `partition_point`) and MATCH it so the visible window is identical to today's behavior; adjust the `lo`/`hi` computation to match the existing function exactly.

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -p oxideplot-core ds_mode_tests`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/oxideplot-core/src/processing/downsampling.rs
git commit -m "feat(core): DownsampleMode + downsample_for_view_mode"
```

---

## Task 3: WASM — fields, setters, and render logic in `OxidePlot`

**Files:** Modify `crates/oxideplot-wasm/src/lib.rs`

**Interfaces:**
- Consumes: `oxideplot_core::processing::statistics::percentile` (Task 1), `oxideplot_core::processing::downsampling::{DownsampleMode, downsample_for_view_mode}` (Task 2), existing `compute_grid_lines`, `format_tick_value`.
- Produces (JS-callable): `OxidePlot::set_autoscale_mode(&mut self, mode: String)` (`"minmax"|"robust"`), `set_y_scale(&mut self, mode: String)` (`"linear"|"log"`), `set_downsample_mode(&mut self, mode: String)` (`"minmax"|"lttb"|"none"`). Plus getters if the existing setters have them — check the pattern; `normalized` has no getter on the wasm side (state mirrors live in Graph.svelte), so NO wasm getters are needed.

This task is not unit-testable (GPU glue); it is verified by native + wasm builds here and the visual check in Task 6.

- [ ] **Step 1: Add enums + fields + defaults**
  - Add near the top of `lib.rs` (module scope):
    ```rust
    #[derive(Clone, Copy, PartialEq)]
    enum AutoscaleMode { MinMax, Robust }
    #[derive(Clone, Copy, PartialEq)]
    enum YScale { Linear, Log }
    ```
  - Import at top: `use oxideplot_core::processing::statistics::percentile;` and `use oxideplot_core::processing::downsampling::{DownsampleMode, downsample_for_view_mode};` (keep the existing `downsample_for_view` import only if still used elsewhere; if the only call site moves in Step 3, remove it).
  - On the `OxidePlot` struct (next to `normalized: bool`): `autoscale_mode: AutoscaleMode`, `y_scale: YScale`, `downsample_mode: DownsampleMode`.
  - In `OxidePlot::create()` (next to `normalized: false,`): `autoscale_mode: AutoscaleMode::MinMax, y_scale: YScale::Linear, downsample_mode: DownsampleMode::MinMax,`.

- [ ] **Step 2: Y-bounds in `auto_fit()`** — in the `if !self.normalized { … }` branch that currently does the min/max loop over visible `ys`, compute a value stream `v` per y that is `y.log10()` when `self.y_scale == YScale::Log && y > 0.0` (skip non-positive when log), else `y`. Then:
  - `AutoscaleMode::MinMax` → keep min/max over the `v` stream (existing behavior for linear).
  - `AutoscaleMode::Robust` → collect all finite `v` into a `Vec`, sort ascending, and set `y_min = percentile(&sorted, 0.01)`, `y_max = percentile(&sorted, 0.99)`; if that range is degenerate (`!finite || y_max <= y_min`), fall back to the min/max of `v`, then to `(0.0, 1.0)`.
  - Keep the existing `y_pad = ((y_max - y_min)*0.05).max(1e-9)` padding. The normalized early-branch (`[-0.05, 1.05]`) is unchanged and still wins when normalized.

- [ ] **Step 3: Points in `rebuild_visible()`** — at the `downsample_for_view(&src.xs, &src.ys, x_min, x_max, target)` call site, switch to `downsample_for_view_mode(&src.xs, &src.ys, x_min, x_max, target, self.downsample_mode)`. THEN, when building the GPU `points` from `(vis_x, vis_y)`: if `self.y_scale == YScale::Log`, map each point to `[x as f32, (y.log10()) as f32]` and SKIP any point with `y <= 0.0` (non-finite already skipped). If Linear, unchanged. (Decimate first, log-transform the survivors second, per the Global Constraints.)

- [ ] **Step 4: Ticks in `axis_ticks()`** — `compute_grid_lines(self.view.y_min, self.view.y_max)` already runs on the (now possibly log10-space) `view.y_min/y_max`. When `self.y_scale == YScale::Log`, keep each tick's `value` in log-space (so it maps correctly against the log-space view in `Axes.svelte`) but set its `label` to `format_tick_value(10f64.powf(val))` instead of `format_tick_value(val)`. X ticks unchanged.

- [ ] **Step 5: Add the three setters** — following `set_normalized`/`set_draw_mode`:

```rust
#[wasm_bindgen]
pub fn set_autoscale_mode(&mut self, mode: String) {
    self.autoscale_mode = if mode == "robust" { AutoscaleMode::Robust } else { AutoscaleMode::MinMax };
    self.auto_fit();
}
#[wasm_bindgen]
pub fn set_y_scale(&mut self, mode: String) {
    self.y_scale = if mode == "log" { YScale::Log } else { YScale::Linear };
    self.auto_fit();
}
#[wasm_bindgen]
pub fn set_downsample_mode(&mut self, mode: String) {
    self.downsample_mode = DownsampleMode::parse(&mode);
    self.rebuild_visible();
    self.render();
}
```

- [ ] **Step 6: Build (native + wasm)**

Run: `cargo build -p oxideplot-wasm`
Expected: SUCCESS (fix any dangling imports — e.g. remove the old `downsample_for_view` import if unused).

Run: `cargo build -p oxideplot-core --target wasm32-unknown-unknown`
Expected: SUCCESS (core unchanged shape; confirms nothing regressed for wasm).

- [ ] **Step 7: Commit**

```bash
git add crates/oxideplot-wasm/src/lib.rs
git commit -m "feat(wasm): autoscale/y-scale/downsample modes in OxidePlot render path"
```

---

## Task 4: Frontend wiring — renderer.ts + Graph.svelte

**Files:** Modify `src/lib/renderer.ts`, `src/lib/components/Graph.svelte`

**Interfaces:**
- Consumes: wasm `set_autoscale_mode`/`set_y_scale`/`set_downsample_mode` (Task 3, available after `npm run build:wasm`).
- Produces: `Renderer.setAutoscaleMode(mode)`, `setYScale(mode)`, `setDownsampleMode(mode)`; and `Graph.svelte` exported `setAutoscaleMode/getAutoscaleMode`, `setYScale/getYScale`, `setDownsampleMode/getDownsampleMode` (string-valued: `'minmax'|'robust'`, `'linear'|'log'`, `'minmax'|'lttb'|'none'`).

- [ ] **Step 1: renderer.ts wrappers** — copy the `setNormalized` shape (`this.assertPlot(); (this.plot as any).set_X(mode);`):

```ts
setAutoscaleMode(mode: string): void { this.assertPlot(); (this.plot as any).set_autoscale_mode(mode); }
setYScale(mode: string): void { this.assertPlot(); (this.plot as any).set_y_scale(mode); }
setDownsampleMode(mode: string): void { this.assertPlot(); (this.plot as any).set_downsample_mode(mode); }
```

- [ ] **Step 2: Graph.svelte state + setters/getters** — copy the `normalized`/`setNormalized`/`getNormalized` shape. Add local vars `let autoscaleMode = 'minmax'; let yScale = 'linear'; let downsampleMode = 'minmax';` and:

```ts
export function setAutoscaleMode(v: string): void { autoscaleMode = v; try { renderer.setAutoscaleMode(v); } catch (_) {} refreshView(); }
export function getAutoscaleMode(): string { return autoscaleMode; }
export function setYScale(v: string): void { yScale = v; try { renderer.setYScale(v); } catch (_) {} refreshView(); }
export function getYScale(): string { return yScale; }
export function setDownsampleMode(v: string): void { downsampleMode = v; try { renderer.setDownsampleMode(v); } catch (_) {} refreshView(); }
export function getDownsampleMode(): string { return downsampleMode; }
```

(`refreshView()` re-pulls view state + ticks so the axis overlay updates — same as `setNormalized`.)

- [ ] **Step 3: Type check**

Run: `npm run check` (or `npx tsc --noEmit` if `check` is not a script)
Expected: no new type errors from the added methods.

- [ ] **Step 4: Commit**

```bash
git add src/lib/renderer.ts src/lib/components/Graph.svelte
git commit -m "feat(app): renderer + Graph wiring for render-option toggles"
```

---

## Task 5: Settings UI — Settings.svelte + App.svelte

**Files:** Modify `src/lib/components/Settings.svelte`, `src/App.svelte`

**Interfaces:**
- Consumes: `Graph.svelte`'s `setAutoscaleMode/getAutoscaleMode` etc. (Task 4).
- Produces: three `<select>` controls in Settings, wired through App to the focused graph.

- [ ] **Step 1: Settings.svelte controls** — add three exported props `export let autoscaleMode: string; export let yScale: string; export let downsampleMode: string;`, add them to the `createEventDispatcher` event map, and add three `<select>` controls (these are 2–3-way choices, not checkboxes) each dispatching `{ value }` on change, e.g.:

```svelte
<label class="setting">
  <span>Autoscale</span>
  <select value={autoscaleMode} on:change={(e) => dispatch('autoscalemode', { value: e.currentTarget.value })}>
    <option value="minmax">Min / Max</option>
    <option value="robust">Robust</option>
  </select>
</label>
```
Repeat for `yScale` (`linear`/`log`, event `yscale`) and `downsampleMode` (`minmax`/`lttb`/`none`, event `downsamplemode`). Match the existing Settings styling/markup conventions.

- [ ] **Step 2: App.svelte mirror vars + handlers + wiring** —
  - Add mirror vars: `let autoscaleMode = 'minmax'; let yScale = 'linear'; let downsampleMode = 'minmax';`.
  - In `syncFromGraph()` add: `autoscaleMode = g.getAutoscaleMode(); yScale = g.getYScale(); downsampleMode = g.getDownsampleMode();`.
  - Add handlers copying `handleNormalized`:
    ```ts
    function handleAutoscaleMode(e: CustomEvent<{ value: string }>) { focusedGraph?.setAutoscaleMode(e.detail.value); syncFromGraph(); }
    function handleYScale(e: CustomEvent<{ value: string }>) { focusedGraph?.setYScale(e.detail.value); syncFromGraph(); }
    function handleDownsampleMode(e: CustomEvent<{ value: string }>) { focusedGraph?.setDownsampleMode(e.detail.value); syncFromGraph(); }
    ```
  - Pass props + events to `<Settings>`: add `{autoscaleMode} {yScale} {downsampleMode}` and `on:autoscalemode={handleAutoscaleMode} on:yscale={handleYScale} on:downsamplemode={handleDownsampleMode}`.

- [ ] **Step 2b: Verify props are exhaustive** — confirm the `<Settings ... />` invocation passes ALL of Settings.svelte's required props (the added three plus the existing lineWidth/pointRadius/showGrid/normalized), so Svelte doesn't warn about missing props.

- [ ] **Step 3: Type check**

Run: `npm run check` (or `npx tsc --noEmit`)
Expected: no new type errors.

- [ ] **Step 4: Commit**

```bash
git add src/lib/components/Settings.svelte src/App.svelte
git commit -m "feat(app): Settings controls for autoscale/y-scale/downsample"
```

---

## Task 6: Build wasm + visual verification (controller step)

**Files:** none (build + manual verification). This task is performed by the controller (not a subagent) because it requires running the desktop app and observing the plot.

- [ ] **Step 1: Rebuild the wasm bindings**

Run: `npm run build:wasm`
Expected: SUCCESS; `src/lib/wasm/oxideplot_wasm.{js,d.ts}` now expose `set_autoscale_mode`, `set_y_scale`, `set_downsample_mode`.

- [ ] **Step 2: Launch the app and load a dataset** — use the `run` skill (or `npm run tauri dev`). Open a CSV with (a) a channel that has a lone large spike, and (b) a channel spanning several orders of magnitude (e.g. a synthetic file, or one of the MWD logs).

- [ ] **Step 3: Verify each toggle in the Settings panel**
  - **Autoscale → Robust:** the Y view clips the lone spike out of frame and the main signal fills the panel (vs Min/Max where the spike flattens it). Switching back to Min/Max restores the full range.
  - **Y-scale → Log:** the order-of-magnitude channel becomes legible; Y-axis labels read as powers of ten (1, 10, 100, …); non-positive samples drop. Switching back to Linear restores.
  - **Downsample → None / LTTB / Min/Max:** the line visibly changes density/shape; Min/Max preserves the spike, LTTB is smoother, None draws every visible point. Confirm no crash and pan/zoom still work under each.
  - Confirm the selected values persist correctly when switching focus between stacked graphs (the `syncFromGraph` mirrors).

- [ ] **Step 4: Record the outcome** — capture a screenshot or note per toggle in the progress ledger. If any toggle misbehaves, file it as a finding and dispatch a fix; otherwise the slice is done.

---

## Definition of done

- `cargo test -p oxideplot-core` passes (Tasks 1–2 add `percentile_tests`, `ds_mode_tests`).
- `cargo build -p oxideplot-wasm` and `cargo build -p oxideplot-core --target wasm32-unknown-unknown` succeed; `npm run build:wasm` and type-check pass.
- In the running app, all three Settings toggles change the plot as specified (robust clips outliers; log-Y shows decades; downsample modes change decimation), with no crash and working pan/zoom.
- Defaults: Autoscale=Min/Max, Y-scale=Linear, Downsample=Min/Max.
