# OxidePlot Post-MVP Feature Roadmap

**Date:** 2026-06-18
**Status:** Approved roadmap (decomposition); each near-term phase gets its own spec → plan → implementation cycle
**Author:** Will (with Claude)

## Context

The egui → Tauri + Svelte + WebGPU migration is complete (see
`2026-06-17-oxideplot-tauri-migration-design.md` and its plan). The result is a
working but deliberately **basic single-graph 2D plotter**: open CSV/Excel,
plot multi-series, datetime axes, pan/zoom/fit, cursors, viewport LTTB
downsampling, draw modes, series list, settings, PNG/CSV export, themes,
persisted prefs.

The original app (and the comprehensive `ROADMAP.md`) envisions much more. This
document organizes the remaining work into **near-term phases** (the five
features Will named) and a **themed long-term backlog** (the rest of
`ROADMAP.md`), with sequencing.

This is a **roadmap / decomposition**, not a single implementation spec. Each
near-term phase is a sub-project that gets its own brainstorm → design spec →
implementation plan → execution, in order.

## Architectural starting point (what's already in place)

- **`oxideplot-core`** (Rust, native + wasm32): data parsing (CSV/Excel/datetime/
  unit inference), `processing/` (LTTB downsampling, `math_ops.rs`, statistics,
  kd_tree), the wgpu `PlotRenderer` + WGSL shaders, `PlotViewState`, and —
  importantly — a **multi-graph + axis-sync state model already exists**:
  `GraphState`, sync-group IDs, `AxisState` (carried forward during the
  migration specifically for this roadmap).
- **`oxideplot-wasm`**: `OxidePlot` wasm-bindgen wrapper driving one canvas.
- **`src-tauri`**: OS integration (file dialogs, read/save, prefs).
- **`src/`**: Svelte 5 frontend — toolbar, column dialog, series list, settings,
  SVG axis + cursor overlays, single `<canvas>` + `Renderer` wrapper.

The data/processing engine and renderer are reusable per-series and per-graph,
so most phases build on the existing pieces rather than replacing them.

## Sequencing decision

**Self-contained value-adds first, the big architectural rework last.** The
three additive features (table view, math, interpolation) enrich the current
single-graph app, are lower-risk, and don't depend on multi-graph. Multi-graph
+ synced axes is the large architectural change, so it comes last of the
near-term set. (Synced X-axis *requires* multi-graph, so they ship together.)

---

## Near-term phases

> **Status (2026-06-18): all four near-term phases COMPLETE and visually confirmed.**
> Phase 7 (Table view), Phase 8 (Math transforms), Phase 9 (Interpolation/resampling),
> and Phase 10 (Multi-graph + synced X) are all built, reviewed, and working on the
> `tauri-migration` branch. Each has its own design spec + plan under
> `docs/superpowers/`. Remaining work is the long-term backlog (B1–B11) below.

### Phase 7 — Table view

**Intent:** inspect the loaded data as a table alongside/over the plot.

**Scope (MVP for this phase):**
- A **virtualized** read-only data table (renders only visible rows → handles
  large row counts without lag).
- Column headers, scrolling, basic search/filter; column sort.
- Toggle to show/hide (split pane or overlay panel).
- Reuses the parsed `LoadedData` already held in the WASM module (expose rows to
  JS in a windowed/paged way to avoid copying the whole dataset at once).

**Deferred to backlog:** inline cell editing (B5), conditional formatting,
column reordering/resizing polish.

**Depends on:** nothing new (current single-graph app).

### Phase 8 — Math functions / transforms

**Intent:** apply a transform to a series, producing a **new derived series**
that appears in the series list (own color, removable).

**Scope (curated core set):** moving average (simple/EMA), derivative (dy/dx),
integral (cumulative/trapezoidal), normalize (0–1 / z-score), and elementwise
abs / log / sqrt. UI: pick a source series + transform (+ params, e.g. window
size) → adds the result series.

**Notes:** `processing/math_ops.rs` is the starting point; transforms operate on
the full source data (not the downsampled GPU points) and feed the existing
rebuild/downsample path.

**Deferred to backlog (B3):** FFT, Butterworth/band-pass filters, detrend,
curve fitting/regression.

**Depends on:** nothing new.

### Phase 9 — Interpolation / resampling

**Intent:** resample a series to a target point count or fill gaps; same UX
pattern as transforms (produces a resampled series).

**Scope:** linear, nearest-neighbor, and cubic-spline interpolation; decimate /
resample to N points or a target sample rate.

**Depends on:** Phase 8's transform/derived-series UX pattern (reuse it).

### Phase 10 — Multi-graph + synced X-axis

**Intent:** the big one — multiple plot panels with linked axes.

**Scope:** multiple **stacked vertical plot panels**, each with its own
series set and Y-axis, sharing a **linkable X-axis** (pan/zoom in one linked
panel updates the others' X range — "sync groups"). Add / remove / reorder
graphs. Per-graph series assignment (drag/assign series to a graph).

**Architecture work:** this is primarily frontend + wasm structural change —
move from one `OxidePlot`/canvas to **N graphs** (either N canvases each with a
renderer, or one renderer drawing N viewports). The **core already models the
data side** (`GraphState`, `AxisState`, sync-group IDs), so the effort is the
multi-canvas/layout frontend, the per-graph wasm API, and the X-sync wiring.
Largest phase; gets a dedicated deep brainstorm (incl. the N-canvas vs.
N-viewport rendering decision) before planning.

**Depends on:** nothing strictly, but sequenced last by choice; synced X-axis is
part of this phase (can't link axes without multiple graphs).

---

## Long-term backlog (themed, roughly value-ordered)

Coarse groupings of the rest of `ROADMAP.md`; re-prioritize freely. Each becomes
its own spec→plan cycle when promoted to near-term.

- **B1 · Axis & series richness** — log / dual-independent-Y / inverted axes;
  SI / scientific / engineering tick formatting; custom axis labels & titles;
  marker shapes, dashed/dotted line styles, variable width; error bars,
  confidence bands, gradient-colored lines; custom + colorblind-safe palettes.
  *(High value — publication-quality plots.)*
- **B2 · More plot types** — bar/histogram, box-and-whisker, heatmap/colormap,
  polar, FFT/spectrum view, XY/Lissajous, waterfall/stacked, fill-between,
  candlestick/OHLC.
- **B3 · Heavy analysis** — FFT; low/high/band-pass filters; detrend; curve
  fitting (linear/polynomial/exponential/custom + R² + residuals); percentiles,
  RMS, skew/kurtosis, correlation matrix, value histograms, outlier detection;
  summary-stats export.
- **B4 · Annotations & legend** — draggable legend, legend outside plot area,
  text & arrow annotations, horizontal/vertical reference lines, region
  highlighting.
- **B5 · Data editing** — manual point edit, lasso/rectangle delete,
  crop-to-visible, merge/split series, rename columns, **formula columns**
  (spreadsheet-style), inline table cell editing (extends Phase 7).
- **B6 · File formats & I/O** — import: TSV, Parquet, JSON/JSONL, HDF5/NetCDF,
  SQLite, clipboard paste, URL fetch, live file watching (tail -f), multi-sheet
  Excel, encoding detection, custom delimiters, skip/comment rows. Export: SVG,
  PDF, xlsx-with-charts, Parquet, LaTeX/TikZ, high-DPI PNG, export-region, batch.
- **B7 · Projects & UX/QoL** — project save/load (`.oxideplot`), recent projects,
  keyboard shortcuts, **undo/redo** (command pattern), drag-series-between-graphs,
  right-click context menus, crosshair/snap-to-data cursor, status bar, welcome
  screen, grid / tabbed / detachable graph layouts (extends Phase 10).
- **B8 · Performance & scale** — streaming/memory-mapped parsing, level-of-detail
  rendering, compute-shader GPU downsampling, dirty-rect / frame-budget
  optimization, pre-allocated GPU buffers, perf overlay (FPS/frame time/GPU mem),
  benchmark suite (10K–10M points).
- **B9 · CLI / headless** — `oxideplot render data.csv -o plot.png`, `info`,
  `stats`, `convert`, piping/streaming (`tail -f | oxideplot --live`), open/export
  project files. *(Shares the native offscreen-render path with the MCP server.)*
- **B10 · Platform & distribution** — Linux & macOS builds, Vulkan/GL fallback
  verification, file associations (`.csv`/`.oxideplot` double-click), CI/CD
  (build+test on push, release binaries on tag), GitHub Releases,
  winget/scoop/brew/Flatpak/AppImage.
- **B11 · Stretch** — real-time data feeds (serial/COM, MQTT/WebSocket) + rolling
  window + trigger capture; embedded scripting (Rhai/Lua) + macro recorder +
  templates; collaboration (shareable self-contained HTML, embed); the **3D
  plotting port** (renderer + orbit camera already exist in core); and the
  **Claude / MCP server** (already designed in the migration spec: load_data →
  describe_data → create_graph → render_png, driven by Claude).

---

## Process

Per the brainstorming/decomposition workflow: this roadmap is the decomposition.
We execute **one phase at a time**, in order (7 → 8 → 9 → 10, then promote
backlog items). Each phase begins with its own focused brainstorm (clarifying
that phase's specifics — e.g., exact transforms in Phase 8, the N-canvas-vs-
N-viewport decision in Phase 10), produces its own design spec and
implementation plan, and is built + reviewed before starting the next.

**Status:** Phases 7–10 are complete (see the status note above). The next step
is to either finish/merge the `tauri-migration` branch or promote a backlog item
(B1–B11) to its own brainstorm → spec → plan → implement cycle.

## Non-goals (for the roadmap as a whole)

- Not committing to *all* backlog items — the backlog is a prioritized menu, not
  a contract. Items are promoted to near-term deliberately.
- Distant backlog phases are intentionally coarse; they get real scope only when
  promoted.
