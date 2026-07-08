# Spectrum + Spectrogram Tabs Slice — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add two more per-graph view tabs — **Spectrum** (overlaid PSD of the plotted series) and **Spectrogram** (freq-vs-time heatmap of the selected series) — reusing the tab framework from the Distribution slice. The FFT compute (`compute_psd`, `compute_spectrogram`) already lives in `oxideplot-core`.

**Architecture:** Two thin wasm bindings compute a series' PSD / spectrogram from its `ys` (sample rate inferred from its `xs` = `1/median(dt)`, or an override). `SpectrumView.svelte` is an SVG line chart (like DistView, but lines + log-Y) overlaying each plotted series' PSD in its color. `SpectrogramView.svelte` is a 2D-canvas magma heatmap of the *selected* series. The graph's view mode widens to `'plot'|'table'|'dist'|'spectrum'|'spectrogram'`, the header tab strip gains two tabs, and a small sample-rate field lets the user override the inferred rate.

**Tech Stack:** Rust (`oxideplot-wasm`, `serde_wasm_bindgen`), Svelte 5, SVG + 2D canvas. `npm run build:wasm`.

## Global Constraints

- Live render path is `crates/oxideplot-wasm/src/lib.rs`'s `OxidePlot`. **Real wasm gate: `cargo build -p oxideplot-wasm --target wasm32-unknown-unknown`** (native build is a cfg-stub).
- Copy established patterns: the `series_histogram` binding (bounds-check + serde return) for the new bindings; `DistView.svelte` (container-measured SVG, exported `refresh()`, per-series colors from `seriesInfo()`, 0..1 color floats × 255) for the view components; the Distribution slice's tab framework (`setViewMode`, per-graph header tabs, view mounts as `{:else if}` siblings of `.canvas-wrap`, `class:hidden={viewMode !== 'plot'}`, App mirror + `on:viewmode`).
- Sample rate: computed in wasm from the source's `xs` as `1 / median(positive consecutive diffs)`; `1.0` fallback if not derivable. An optional override is passed from the UI.
- Spectrum overlays ALL plotted (visible) series; Spectrogram shows the SELECTED series (`selectedSeriesIndex`, already wired). The SeriesList (visible in all non-table modes) is the channel picker for Spectrogram.
- Frontend type check: `npm run check` — NO NEW errors vs. the baseline (generated wasm `.js` + 2 pre-existing `Graph.svelte` errors).
- Magma colormap for the spectrogram: port the 5-stop gradient the MCP uses (`crates/oxideplot-mcp/src/main.rs` `heat_color`) into a small JS function in `SpectrogramView.svelte`.

## File Structure

**Modify:** `crates/oxideplot-wasm/src/lib.rs` (2 bindings), `src/lib/renderer.ts` (2 wrappers + types), `src/lib/components/Graph.svelte` (viewMode widen + tabs + mounts + sample-rate state), `src/App.svelte` (viewMode mirror widen).
**Create:** `src/lib/components/SpectrumView.svelte`, `src/lib/components/SpectrogramView.svelte`.

Branch `tauri-migration`; commit each task.

---

## Task 1: WASM — `series_spectrum` + `series_spectrogram`

**Files:** Modify `crates/oxideplot-wasm/src/lib.rs`

**Interfaces:**
- Consumes: `oxideplot_core::processing::spectral::{compute_psd, compute_spectrogram}`, `self.sources[i].{xs, ys}`.
- Produces (JS-callable):
  - `series_spectrum(&self, source_index: usize, sample_rate: Option<f64>) -> Result<JsValue, JsValue>` → `{ freqs: number[], power: number[], sample_rate: number }`.
  - `series_spectrogram(&self, source_index: usize, window: usize, sample_rate: Option<f64>) -> Result<JsValue, JsValue>` → `{ frames: number[][], bins: number, n_frames: number, sample_rate: number, nyquist: number, duration_s: number }` (`frames[frame][bin]` = magnitude; `nyquist = sample_rate/2`; `duration_s = n_frames * (window/2) / sample_rate`).

- [ ] **Step 1: Read the precedent** — read `series_histogram` (~line 826, added in the Distribution slice) for the bounds-check + `serde_wasm_bindgen::to_value` return shape; and confirm `SourceSeries { xs, ys, .. }`.

- [ ] **Step 2: Add a sample-rate helper (private, in `mod wasm_impl`)**:

```rust
/// Sample rate (Hz) from a series' X values: 1/median positive dt. 1.0 fallback.
fn sample_rate_from_xs(xs: &[f64]) -> f64 {
    let mut dts: Vec<f64> = xs
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|d| d.is_finite() && *d > 0.0)
        .collect();
    if dts.is_empty() {
        return 1.0;
    }
    dts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let md = dts[dts.len() / 2];
    if md > 0.0 {
        1.0 / md
    } else {
        1.0
    }
}
```

- [ ] **Step 3: Implement the two bindings** (inside `mod wasm_impl`, near `series_histogram`):

```rust
#[derive(serde::Serialize)]
struct SpectrumData {
    freqs: Vec<f64>,
    power: Vec<f64>,
    sample_rate: f64,
}

#[wasm_bindgen]
pub fn series_spectrum(&self, source_index: usize, sample_rate: Option<f64>) -> Result<JsValue, JsValue> {
    let src = self
        .sources
        .get(source_index)
        .ok_or_else(|| JsValue::from_str("source index out of range"))?;
    let fs = sample_rate.unwrap_or_else(|| sample_rate_from_xs(&src.xs));
    let (freqs, power) = oxideplot_core::processing::spectral::compute_psd(&src.ys, fs);
    if freqs.is_empty() {
        return Err(JsValue::from_str("not enough samples for a spectrum"));
    }
    serde_wasm_bindgen::to_value(&SpectrumData { freqs, power, sample_rate: fs })
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

#[derive(serde::Serialize)]
struct SpectrogramData {
    frames: Vec<Vec<f64>>,
    bins: usize,
    n_frames: usize,
    sample_rate: f64,
    nyquist: f64,
    duration_s: f64,
}

#[wasm_bindgen]
pub fn series_spectrogram(&self, source_index: usize, window: usize, sample_rate: Option<f64>) -> Result<JsValue, JsValue> {
    let src = self
        .sources
        .get(source_index)
        .ok_or_else(|| JsValue::from_str("source index out of range"))?;
    let fs = sample_rate.unwrap_or_else(|| sample_rate_from_xs(&src.xs));
    let win = window.clamp(16, 4096);
    let (frames, bins) = oxideplot_core::processing::spectral::compute_spectrogram(&src.ys, win);
    if frames.is_empty() || bins == 0 {
        return Err(JsValue::from_str("not enough samples for a spectrogram"));
    }
    let n_frames = frames.len();
    let duration_s = n_frames as f64 * (win as f64 / 2.0) / fs;
    serde_wasm_bindgen::to_value(&SpectrogramData {
        frames, bins, n_frames, sample_rate: fs, nyquist: fs / 2.0, duration_s,
    })
    .map_err(|e| JsValue::from_str(&e.to_string()))
}
```

- [ ] **Step 4: Build** — `cargo build -p oxideplot-wasm --target wasm32-unknown-unknown` succeeds, no new warnings.

- [ ] **Step 5: Commit** — `git add crates/oxideplot-wasm/src/lib.rs && git commit -m "feat(wasm): series_spectrum + series_spectrogram bindings"`

---

## Task 2: renderer.ts — wrappers + types

**Files:** Modify `src/lib/renderer.ts`

**Interfaces:**
- Produces: `interface SpectrumData { freqs: number[]; power: number[]; sample_rate: number }`; `interface SpectrogramData { frames: number[][]; bins: number; n_frames: number; sample_rate: number; nyquist: number; duration_s: number }`; and methods `seriesSpectrum(sourceIndex: number, sampleRate?: number): SpectrumData`, `seriesSpectrogram(sourceIndex: number, window: number, sampleRate?: number): SpectrogramData`.

- [ ] **Step 1: Implement** — copy the `seriesHistogram()` wrapper shape:

```ts
seriesSpectrum(sourceIndex: number, sampleRate?: number): SpectrumData {
  this.assertPlot();
  return (this.plot as any).series_spectrum(sourceIndex, sampleRate ?? undefined) as SpectrumData;
}
seriesSpectrogram(sourceIndex: number, window: number, sampleRate?: number): SpectrogramData {
  this.assertPlot();
  return (this.plot as any).series_spectrogram(sourceIndex, window, sampleRate ?? undefined) as SpectrogramData;
}
```
(Pass `undefined` for an omitted rate so wasm's `Option<f64>` is `None`.) Add the two interfaces near `HistogramData`.

- [ ] **Step 2: Type check** — `npm run check`, no new errors.
- [ ] **Step 3: Commit** — `feat(app): renderer.ts spectrum/spectrogram wrappers + types`

---

## Task 3: `SpectrumView.svelte` — overlaid PSD line chart

**Files:** Create `src/lib/components/SpectrumView.svelte`

**Interfaces:** `<SpectrumView {renderer} sampleRate={number|null} />` with exported `refresh()`. Overlays each plotted series' PSD as a colored line; log-Y; X = frequency.

- [ ] **Step 1: Read** `DistView.svelte` (container-measured SVG, `refresh()` pulling `seriesInfo()` + per-series data, 0..1 color × 255, hidden-series dimming).

- [ ] **Step 2: Implement** — like DistView but a LINE chart. In `refresh()`: pull `renderer.seriesInfo()`; for each series `i`, `renderer.seriesSpectrum(i, sampleRate ?? undefined)`. Build one `<polyline>`/`<path>` per series in its color. Axes: **X = frequency** (0 … max freq across series), linear; **Y = power, log10** (compute `log10(power)` for `power > 0`, drop ≤0; map to the plot height between the min/max log-power across all series). Labeled ticks: a few X freq labels (0, mid, max Hz) and Y labels as `10^n` powers. A small caption showing the sample rate (from the returned `sample_rate`, e.g. "fs = 100 Hz"). Container-measured SVG (bind:clientWidth/Height), reactive layout, guard before measurement; per-series try/catch so one failing series doesn't break others; empty/error state message.

- [ ] **Step 3: Type check** — `npm run check`, no new errors.
- [ ] **Step 4: Commit** — `feat(app): SpectrumView overlaid PSD line chart`

---

## Task 4: `SpectrogramView.svelte` — magma heatmap

**Files:** Create `src/lib/components/SpectrogramView.svelte`

**Interfaces:** `<SpectrogramView {renderer} seriesIndex={number} sampleRate={number|null} />` with exported `refresh()`. Renders a freq-vs-time heatmap of the selected series onto a 2D canvas.

- [ ] **Step 1: Read** `crates/oxideplot-mcp/src/main.rs` `heat_color` (the 5-stop magma gradient) to port the colormap to JS.

- [ ] **Step 2: Implement** — a `<canvas>` sized to its container (bind:clientWidth/Height → set `canvas.width/height`). In `refresh()`: `const d = renderer.seriesSpectrogram(seriesIndex, 256, sampleRate ?? undefined)` (window 256). Draw: compute a robust color range (5th–99.5th percentile of `log10(magnitude + 1e-12)` over all cells), then for each pixel map `(px → frame, py → freq bin with freq increasing upward)`, look up the magnitude, normalize to `[0,1]`, and `putImageData`/`fillRect` the magma color. Port `heat_color` to a JS `magma(t: number): [r,g,b]` (0..255). Add baked axis labels via HTML/SVG overlay OR canvas text: Y = frequency `0 … nyquist` Hz (a few ticks), X = time `0 … duration_s` s; a caption "fs = N Hz". Re-pull reactively on `seriesIndex` / sampleRate change (guarded). Error/empty state (e.g. "select a series" or "not enough samples").

- [ ] **Step 3: Type check** — `npm run check`, no new errors.
- [ ] **Step 4: Commit** — `feat(app): SpectrogramView magma heatmap of selected series`

---

## Task 5: View-tab framework — Spectrum + Spectrogram tabs + sample-rate control

**Files:** Modify `src/lib/components/Graph.svelte`, `src/App.svelte`

- [ ] **Step 1: Widen view mode** — in `Graph.svelte`, `viewMode: 'plot'|'table'|'dist'` → add `'spectrum'|'spectrogram'` (declaration + `getViewMode` type + `setViewMode` param type). In `App.svelte`, widen the mirrored `viewMode` type identically.

- [ ] **Step 2: Sample-rate state** — in `Graph.svelte` add `let sampleRate: number | null = null;` (null = infer). Add `let spectrumView: SpectrumView; let spectrogramView: SpectrogramView;` and imports. Extend `setViewMode` so `'spectrum'` refreshes `spectrumView` and `'spectrogram'` refreshes `spectrogramView` (after `tick()`), like table/dist. Extend `setSeries()`'s post-load refresh block to also refresh whichever of spectrum/spectrogram is active (mirroring the dist branch added earlier). When `selectedSeriesIndex` changes and `viewMode === 'spectrogram'`, refresh `spectrogramView`.

- [ ] **Step 3: Tabs + mounts + sample-rate field** — in `Graph.svelte`'s header tab strip, add two buttons `Spectrum` and `Spectrogram` (`class:active` + `on:click={() => setViewMode('spectrum'|'spectrogram')}`). Generalize the view mounts to include:
  ```svelte
  {:else if hasData && viewMode === 'spectrum'}
    <SpectrumView bind:this={spectrumView} {renderer} {sampleRate} />
  {:else if hasData && viewMode === 'spectrogram'}
    <SpectrogramView bind:this={spectrogramView} {renderer} seriesIndex={selectedSeriesIndex} {sampleRate} />
  {/if}
  ```
  The `.canvas-wrap` hide condition is already `viewMode !== 'plot'`, so it covers the new modes. When `viewMode` is `spectrum` or `spectrogram`, show a small numeric input in the header (next to the tabs) bound to `sampleRate` (placeholder "auto (from X)"; on change, refresh the active spectral view). Empty input → `null` (infer).

- [ ] **Step 4: Type check** — `npm run check`, no new errors. Confirm no dangling refs.
- [ ] **Step 5: Commit** — `feat(app): Spectrum + Spectrogram tabs + sample-rate control`

---

## Task 6: Build + visual verification (controller step)

- [ ] **Step 1:** `npm run build:wasm` (regenerates `series_spectrum`/`series_spectrogram` into the `.d.ts`; confirm they appear). `npm run build` (vite) succeeds.
- [ ] **Step 2:** Launch (`npx tauri dev`), load a CSV with a datetime/time X and a vibration-like channel, plot 1–3 series.
- [ ] **Step 3: Verify:**
  - The header tab strip now reads `Plot · Table · Dist · Spectrum · Spectrogram`.
  - **Spectrum**: overlaid PSD lines (one per plotted series, colored), log-Y, X in Hz; peaks visible for a periodic channel; the `fs = …` caption reads a sensible rate. The sample-rate field overrides it.
  - **Spectrogram**: a magma heatmap of the *selected* series (freq up the Y, time along X); selecting a different series in the list changes it; a clear band/structure for a real vibration channel.
  - Switching back to **Plot** restores the WebGPU plot with pan/zoom intact.
- [ ] **Step 4:** Record the outcome; fix any finding.

## Definition of done

- `cargo build -p oxideplot-wasm --target wasm32-unknown-unknown`, `npm run build:wasm`, `npm run check` (no new errors), `npm run build` all pass.
- In the app: `Spectrum` shows overlaid PSDs (log-Y, colored, sample-rate caption + override); `Spectrogram` shows a magma freq-vs-time heatmap of the selected series; both slot into the existing tab strip; Plot/Table/Dist unaffected.
