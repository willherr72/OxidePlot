# Distribution Tab Slice — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce the per-graph **view-tab framework** (`Plot · Table · Dist`) in the OxidePlot app and add the **Distribution** view — an SVG histogram (bar chart) of the *selected* series — establishing the tab pattern that Spectrum/Spectrogram will later extend.

**Architecture:** `histogram()` already lives in `oxideplot-core` (from the Foundation slice), so the compute is done. This slice is frontend: a thin WASM binding `series_histogram(source_index, nbins)` returning the histogram of a source's `ys`; a `renderer.ts` wrapper; a new SVG `DistView.svelte` (no WebGPU — like the existing `TableView`); a `selectedSeriesIndex` concept (click a series row to select it); and generalizing each graph's `viewMode` from `'plot'|'table'` to `'plot'|'table'|'dist'` with a per-graph header tab row (replacing the single toolbar "Table" button).

**Tech Stack:** Rust (`oxideplot-wasm` via wasm-bindgen, `serde_wasm_bindgen`), Svelte 5, SVG. Build wasm with `npm run build:wasm`.

## Global Constraints

- The live render path is `crates/oxideplot-wasm/src/lib.rs`'s `OxidePlot`. **The real wasm build gate is `cargo build -p oxideplot-wasm --target wasm32-unknown-unknown`** — the plain native `cargo build -p oxideplot-wasm` only compiles a cfg-stubbed no-op (`mod wasm_impl` is `#[cfg(target_arch = "wasm32")]`), so it does NOT verify the edited code.
- Copy existing precedents exactly: `series_histogram` copies `add_transform`'s bounds-checked source lookup + `series_info`/`axis_ticks`'s `serde_wasm_bindgen::to_value` return; `renderer.ts.seriesHistogram` copies `axisTicks()`; `DistView.svelte` copies `TableView.svelte`'s prop + `refresh()` shape; the view-tab mechanism copies the current `viewMode`/`toggleViewMode`/`TableView`-sibling pattern.
- Series are identified by array index into `sources` (Rust) / the `seriesInfo()` array (JS) — the same index every per-series action already uses.
- Frontend type check: `npm run check`. There is a large PRE-EXISTING baseline of type errors (in the generated `src/lib/wasm/oxideplot_wasm.js`, plus 2 in `Graph.svelte`); the gate is NO NEW errors versus that baseline (confirm before/after if unsure).
- Theme: SVG bars use the app's CSS custom properties (e.g. `--accent` amber, `--bg`/`--panel-bg`, `--axis-text`), consistent with `Axes.svelte`, so light/dark themes both work.

## File Structure

**Modify:**
- `crates/oxideplot-wasm/src/lib.rs` — add `HistogramData` struct + `series_histogram` method.
- `src/lib/renderer.ts` — add `HistogramData` interface + `seriesHistogram` method.
- `src/lib/components/SeriesList.svelte` — click-to-select a series row (`dispatch('select', i)`) + selected-row highlight.
- `src/lib/components/Graph.svelte` — `selectedSeriesIndex` state + accessors; widen `viewMode` to `'plot'|'table'|'dist'`; a per-graph header tab row; mount `DistView`; canvas-hide condition `!== 'plot'`.
- `src/App.svelte` — mirror `selectedSeriesIndex` + `viewMode`; wire `SeriesList` `on:select`; remove the toolbar Table button; keep panel-hiding correct for non-plot modes.

**Create:**
- `src/lib/components/DistView.svelte` — SVG histogram bar chart.

Work on branch `tauri-migration`; commit each task.

---

## Task 1: WASM — `series_histogram` binding

**Files:** Modify `crates/oxideplot-wasm/src/lib.rs`

**Interfaces:**
- Consumes: `oxideplot_core::processing::histogram::histogram` (exists), `self.sources[i].ys`.
- Produces (JS-callable): `OxidePlot::series_histogram(&self, source_index: usize, nbins: usize) -> Result<JsValue, JsValue>` returning a JS object `{ counts: number[], bin_centers: number[], min: number, max: number, n: number }`. Throws (JS exception) on out-of-range index or too-few-finite-values.

Not unit-testable (wasm). Gate: `cargo build -p oxideplot-wasm --target wasm32-unknown-unknown` succeeds, no new warnings.

- [ ] **Step 1: Read the precedents** — in `crates/oxideplot-wasm/src/lib.rs`, read `add_transform` (~line 1054, for the `self.sources.get(i).ok_or_else(...)` bounds-check pattern) and `series_info`/`axis_ticks` (~lines 594-658/741-753, for the `#[derive(Serialize)] struct` + `serde_wasm_bindgen::to_value(&x).map_err(...)` return). Note whether there's a "Series management" section to place this near.

- [ ] **Step 2: Implement** — add inside `mod wasm_impl` (so it's `#[cfg(target_arch="wasm32")]`-gated), next to the other `#[wasm_bindgen]` methods:

```rust
#[derive(serde::Serialize)]
struct HistogramData {
    counts: Vec<usize>,
    bin_centers: Vec<f64>,
    min: f64,
    max: f64,
    n: usize,
}

#[wasm_bindgen]
pub fn series_histogram(&self, source_index: usize, nbins: usize) -> Result<JsValue, JsValue> {
    let src = self
        .sources
        .get(source_index)
        .ok_or_else(|| JsValue::from_str("source index out of range"))?;
    let h = oxideplot_core::processing::histogram::histogram(&src.ys, nbins)
        .ok_or_else(|| JsValue::from_str("not enough finite values for a histogram"))?;
    let data = HistogramData {
        counts: h.counts,
        bin_centers: h.bin_centers,
        min: h.min,
        max: h.max,
        n: h.n,
    };
    serde_wasm_bindgen::to_value(&data).map_err(|e| JsValue::from_str(&e.to_string()))
}
```

If `HistogramData`'s field names collide with an existing struct, name it `SeriesHistogramData`. Match the `serde`/`serde_wasm_bindgen` import style already used by the neighboring return-structs (they're already imported — confirm).

- [ ] **Step 3: Build (the real gate)**

Run: `cargo build -p oxideplot-wasm --target wasm32-unknown-unknown`
Expected: SUCCESS, no new warnings (a pre-existing `unused import: wasm_bindgen::prelude::*` at lib.rs:24 is acceptable — it predates this task).

- [ ] **Step 4: Commit**

```bash
git add crates/oxideplot-wasm/src/lib.rs
git commit -m "feat(wasm): series_histogram binding (core histogram of a source)"
```

---

## Task 2: renderer.ts — `seriesHistogram` wrapper

**Files:** Modify `src/lib/renderer.ts`

**Interfaces:**
- Consumes: wasm `series_histogram` (available after `npm run build:wasm` in Task 6; the `(this.plot as any)` cast means type-check passes before then).
- Produces: `interface HistogramData { counts: number[]; bin_centers: number[]; min: number; max: number; n: number }` and `Renderer.seriesHistogram(sourceIndex: number, nbins: number): HistogramData`.

- [ ] **Step 1: Read the precedent** — read `axisTicks()` (~line 159) and the `AxisTicksData`/`TickEntry` interfaces (~lines 49-58) in `src/lib/renderer.ts`.

- [ ] **Step 2: Implement** — add the interface near `AxisTicksData`, and the method following `axisTicks()`'s shape:

```ts
export interface HistogramData {
  counts: number[];
  bin_centers: number[];
  min: number;
  max: number;
  n: number;
}
```

```ts
seriesHistogram(sourceIndex: number, nbins: number): HistogramData {
  this.assertPlot();
  return (this.plot as any).series_histogram(sourceIndex, nbins) as HistogramData;
}
```

- [ ] **Step 3: Type check**

Run: `npm run check`
Expected: no NEW errors vs. the known baseline.

- [ ] **Step 4: Commit**

```bash
git add src/lib/renderer.ts
git commit -m "feat(app): renderer.ts seriesHistogram wrapper + HistogramData"
```

---

## Task 3: Series selection

**Files:** Modify `src/lib/components/SeriesList.svelte`, `src/lib/components/Graph.svelte`, `src/App.svelte`

**Interfaces:**
- Produces: `SeriesList` dispatches `select` with the row index; `Graph` holds `selectedSeriesIndex` with `getSelectedSeriesIndex()`/`setSelectedSeriesIndex(i)`; `App` mirrors it and forwards `SeriesList` `select` → focused graph.

- [ ] **Step 1: SeriesList — emit selection** — in `src/lib/components/SeriesList.svelte`: add `select: number` to the `createEventDispatcher` type map; add a `export let selectedIndex: number | null = null;` prop; make each series row (the `.series-name` or the row container) clickable → `dispatch('select', i)` (do NOT swallow the existing per-row control clicks — put the handler on the name/label, or `stopPropagation` on the control buttons). Add a `class:selected={i === selectedIndex}` on the row and a matching CSS rule (use `--col-row-selected` or `--series-row-hover`-style token for a subtle highlight).

- [ ] **Step 2: Graph — hold the selection** — in `src/lib/components/Graph.svelte`:
  - Add `let selectedSeriesIndex = 0;` near the `viewMode` state (~line 65).
  - Add `export function getSelectedSeriesIndex(): number { return selectedSeriesIndex; }` and `export function setSelectedSeriesIndex(i: number): void { selectedSeriesIndex = i; }` near the other getters (~line 435-447). The setter should also `refreshView()` is NOT needed (selection alone doesn't change the plot), but if the Dist view is active it must refresh — safest: after setting, if `viewMode === 'dist'`, call the dist refresh (added in Task 5). For THIS task just store it; Task 5 wires the dist refresh.
  - Clamp/reset on data changes: in `setSeries()` set `selectedSeriesIndex = 0`; in `removeSeries`/`moveSeries`/`clear()` paths, clamp `selectedSeriesIndex` to a valid range (`Math.min(selectedSeriesIndex, seriesCount-1)`, or 0) so it never dangles.

- [ ] **Step 3: App — forward selection** — in `src/App.svelte`:
  - Add `let selectedSeriesIndex = 0;` mirror var; in `syncFromGraph()` add `selectedSeriesIndex = g.getSelectedSeriesIndex();`.
  - Where `<SeriesList ... on:change={handleSeriesChange} />` is rendered (~line 624-628), add `selectedIndex={selectedSeriesIndex}` and `on:select={(e) => { focusedGraph?.setSelectedSeriesIndex(e.detail); syncFromGraph(); }}`.

- [ ] **Step 4: Type check**

Run: `npm run check`
Expected: no NEW errors.

- [ ] **Step 5: Commit**

```bash
git add src/lib/components/SeriesList.svelte src/lib/components/Graph.svelte src/App.svelte
git commit -m "feat(app): selectable series (click a row) for single-series views"
```

---

## Task 4: `DistView.svelte` — SVG histogram

**Files:** Create `src/lib/components/DistView.svelte`

**Interfaces:**
- Consumes: `Renderer.seriesHistogram` (Task 2), `HistogramData`.
- Produces: `<DistView {renderer} seriesIndex={n} />` with an exported `refresh(): void`. Renders an SVG bar chart of `renderer.seriesHistogram(seriesIndex, nbins)`.

- [ ] **Step 1: Read the precedent** — read `src/lib/components/TableView.svelte` for the prop shape (`export let renderer: Renderer;`), the `refresh()` export, and how it pulls data on demand (no reactive WASM push).

- [ ] **Step 2: Implement** — create `src/lib/components/DistView.svelte`. Props: `export let renderer: Renderer; export let seriesIndex: number;`. Local `let data: HistogramData | null = null; let error = ''; const NBINS = 40;`. An exported `refresh()` that calls `renderer.seriesHistogram(seriesIndex, NBINS)` in a try/catch (set `error` on throw, e.g. "not enough finite values"), stores `data`, and clears `error`. Call `refresh()` in `onMount` and in a reactive `$: seriesIndex, refresh()` (re-pull when the selected series changes). Render:
  - A responsive `<svg viewBox="0 0 W H" preserveAspectRatio="none">`-style chart (or measure the container) with left/bottom margins for labels.
  - `<rect>` per bin: x from bin index across the plot width, height ∝ `count / max(counts)` of the plot height, filled `var(--accent)`.
  - Baked axis text (SVG `<text>`, `fill: var(--axis-text)`): X labels at min / midpoint / max (`data.min`, `(min+max)/2`, `data.max`, formatted to a few sig figs); Y labels at 0 and `max(counts)`.
  - When `error` is set or `data` is null, show a centered message instead of bars.
  - Style to fill its parent (like TableView) so it occupies the plot area; background `var(--bg)`/`--panel-bg`.

  Keep it self-contained and simple — this is a static ≤40-bar chart, not an interactive plot.

- [ ] **Step 3: Type check**

Run: `npm run check`
Expected: no NEW errors (DistView is not yet mounted anywhere; this verifies it compiles standalone).

- [ ] **Step 4: Commit**

```bash
git add src/lib/components/DistView.svelte
git commit -m "feat(app): DistView SVG histogram component"
```

---

## Task 5: View-tab framework + mount DistView

**Files:** Modify `src/lib/components/Graph.svelte`, `src/App.svelte`

**Interfaces:**
- Consumes: `DistView` (Task 4), `selectedSeriesIndex` (Task 3).
- Produces: per-graph header tabs `Plot · Table · Dist`; `Graph` `viewMode: 'plot'|'table'|'dist'` with `setViewMode(mode)` + existing `getViewMode`; the toolbar Table button removed.

- [ ] **Step 1: Widen the view mode** — in `src/lib/components/Graph.svelte`:
  - Change `viewMode: 'plot' | 'table'` → `'plot' | 'table' | 'dist'` (declaration ~line 65 and the `getViewMode` return type ~line 438).
  - Replace `toggleViewMode()` with `export async function setViewMode(mode: 'plot'|'table'|'dist'): Promise<void>` that sets `viewMode = mode`, then `await tick()` and: if `'table'` refresh the table (`tableView?.refresh()`), if `'dist'` refresh the dist view (`distView?.refresh()`). Keep it robust if the target component isn't mounted yet.
  - Add `let distView: DistView;` next to `let tableView: TableView;` and import `DistView`.

- [ ] **Step 2: Per-graph header tabs + view mounts** — in `Graph.svelte`'s template:
  - Above `.canvas-wrap`, add a small header tab row (only when `hasData`): three buttons `Plot`/`Table`/`Dist`, each `class:active={viewMode === '<m>'}` and `on:click={() => setViewMode('<m>')}`. Style with a compact tab look (reuse `.tbtn`-like tokens: `--btn-bg`, `--btn-active-bg`, `--accent`, `--radius-sm`) so it reads as an instrument tab strip, not clashing with the toolbar.
  - Generalize the view mounts:
    ```svelte
    {#if hasData && viewMode === 'table'}
      <TableView bind:this={tableView} {renderer} />
    {:else if hasData && viewMode === 'dist'}
      <DistView bind:this={distView} {renderer} seriesIndex={selectedSeriesIndex} />
    {/if}
    ```
  - Change the canvas-hide condition from `class:hidden={viewMode === 'table'}` to `class:hidden={viewMode !== 'plot'}` so Dist also hides (but does NOT unmount) the WebGPU canvas, preserving the surface.
  - In `setSelectedSeriesIndex` (Task 3), after storing, if `viewMode === 'dist'` call `distView?.refresh()` so switching series updates the histogram.

- [ ] **Step 3: App — remove the toolbar Table button; keep panel logic** — in `src/App.svelte`:
  - Change the mirrored `viewMode: 'plot' | 'table'` → `'plot' | 'table' | 'dist'` (~line 36) and `syncFromGraph()` already reads `g.getViewMode()`.
  - REMOVE the toolbar Table `<button>` (~lines 541-544) and its `toggleViewMode` wrapper (~226-229) — view switching now lives in the per-graph tabs. (If `toggleViewMode` is referenced elsewhere, remove those references.)
  - The panels block `{#if viewMode === 'plot' && focusedGraph}` (~line 622) already hides SeriesList/Settings for any non-plot mode — leave it (it now also covers `'dist'`, which is correct: Dist shows the selected-series histogram; SeriesList is hidden, matching Table's behavior). CONFIRM there are no other `viewMode === 'table'` checks in App that need to become `!== 'plot'`.

- [ ] **Step 4: Type check**

Run: `npm run check`
Expected: no NEW errors.

- [ ] **Step 5: Commit**

```bash
git add src/lib/components/Graph.svelte src/App.svelte
git commit -m "feat(app): per-graph view tabs (Plot/Table/Dist) + mount DistView"
```

---

## Task 6: Build wasm + visual verification (controller step)

Performed by the controller (requires the running app).

- [ ] **Step 1: Rebuild bindings** — `npm run build:wasm` (regenerates `series_histogram` into `src/lib/wasm/oxideplot_wasm.{js,d.ts}`). Confirm `series_histogram` appears in the `.d.ts`.
- [ ] **Step 2: Bundle check** — `npm run build` (vite) succeeds.
- [ ] **Step 3: Launch** — `npx tauri dev`; load a CSV, plot 2-3 channels.
- [ ] **Step 4: Verify**
  - Each graph shows a `Plot · Table · Dist` tab strip in its header; the old toolbar Table button is gone.
  - Clicking **Dist** shows a histogram (bars) of the selected series; the WebGPU plot is hidden but pan/zoom still work when switching back to Plot.
  - Clicking a different series row in the list (Plot mode) selects it (highlight); switching to Dist shows THAT series' distribution. On a bimodal channel, two separated bar clusters appear.
  - **Table** tab still works as before.
  - In a multi-graph stack, each graph's tabs are independent (graph 1 can be Plot while graph 2 is Dist).
- [ ] **Step 5: Record** the outcome in the ledger; file findings + fix if any toggle misbehaves.

---

## Definition of done

- `cargo build -p oxideplot-wasm --target wasm32-unknown-unknown` succeeds; `npm run build:wasm`, `npm run check` (no new errors), and `npm run build` pass.
- In the running app: each graph has a `Plot · Table · Dist` header tab strip (toolbar Table button removed); Dist shows the selected series' histogram; clicking a series row changes the selection and the Dist view follows; Table unchanged.
- No core changes were needed (histogram already in core); the only Rust change is the one `series_histogram` binding.
