# OxidePlot Desktop — Analysis Rollforward Design

**Date:** 2026-07-07
**Status:** Approved (design) — pending implementation planning
**Branch:** `tauri-migration`

## Goal

Roll the MCP-proven analysis capabilities into the OxidePlot desktop app
(Tauri 2 + Svelte 5 + WebGPU) with a **cohesive UI**. The MCP server
(`oxideplot-mcp`) was the proving ground: its tools validated, on real MWD
sensor logs, which analyses matter. This project brings the valuable ones to the
human-facing app.

Four capability groups, equal priority:

1. **QC scan** — one-click `health_check` report.
2. **Spectral** — FFT/PSD (`spectrum`) + STFT heatmap (`spectrogram`).
3. **Distribution + formula columns** — `histogram` view + free-form derived columns.
4. **Render upgrades** — robust autoscale, log-Y, min/max (spike-safe) downsampling.

## Primary constraint

**UI cohesion.** Extend the surfaces the app already has instead of bolting on
four disconnected panels. The app today is plot-centric: a toolbar, a vertical
**stack of graphs** (multi-graph, one focused), overlay panels (SeriesList,
Settings), a ColumnDialog, and a single Plot↔Table view toggle.

## Architecture

### Foundation — promote compute to `oxideplot-core`

All the analysis logic currently lives in `oxideplot-mcp/src/main.rs` (native,
Claude-facing only). `oxideplot-core` compiles to **both** WASM (for the app) and
native (for the MCP + tests). The foundation moves the reusable compute into core
so there is **one** implementation, consumed by both:

- `processing::spectral` — `compute_psd`, `compute_spectrogram`, sample-rate
  inference. **Adds `rustfft` to `oxideplot-core`** (pure Rust; must be verified
  to build for `wasm32-unknown-unknown` early — it falls back to scalar on wasm).
- `processing::qc` — the `health_check` heuristics: dead/frozen, robust-z
  glitches vs. `outlier_regime`, changepoints (segment median + MAD) with
  onset localization, channel-lineage tracing, time-gaps, missing clusters.
- `processing::histogram` — binning + counts.
- `processing::expr` — the expression evaluator (tokenizer, Pratt parser, eval),
  including comparisons/logical ops for filter predicates.
- `processing::statistics` — add `pearson` correlation and `median_mad`.
- `processing::downsampling` — add `minmax_envelope` (already has `lttb`).
- Robust-percentile Y view → `state::plot_view` / `render::axis`.

After extraction, `oxideplot-mcp` calls `oxideplot-core` for these (removing its
private copies), so the MCP and app can never diverge. WASM exposure goes through
the existing `oxideplot-wasm` crate; native paths call core directly.

### UI surfaces (where each capability lives)

| Capability | Home surface | Why cohesive |
|---|---|---|
| Spectral, Histogram | **Per-graph view tabs** | Extends today's Plot↔Table toggle |
| QC report | **Bottom drawer** (toggle) | A report, not a plot; dataset-level |
| Render upgrades | **Settings panel** | They are just more render settings |
| Formula columns | **Column workflow** | Producing a column is a column action |

## Capability designs

### 1. Per-graph view tabs

Each graph's header gains a view selector: **Plot · Table · Dist · Spectrum ·
Spectrogram** (generalizes the existing `viewMode: 'plot' | 'table'`).

- **Plot / Table** — unchanged.
- **Spectrum** — overlaid PSD, one line per visible series, log-Y by default.
  Reuses the existing WebGPU line renderer (PSD is just an (x=freq, y=power) line).
- **Dist (histogram)** — acts on the **series selected in the SeriesList**;
  rendered as bars. A series picker (the existing list) switches the subject.
- **Spectrogram** — acts on the **selected series** (inherently single-channel);
  a color-mapped heatmap panel (freq × time). If several series are visible, uses
  the selected one and shows a one-line hint.
- **Sample rate** — inferred from the graph's datetime X column (1/median dt).
  When the X axis is not time, a small manual "sample rate (Hz)" field appears in
  the spectral view's controls.

Multi-graph still applies: to see a line plot *and* its spectrogram at once, add a
second graph and set its tab to Spectrogram (both stacked, per the user's
stacked-layout preference).

### 2. QC drawer

- A `QC` toolbar button toggles a **bottom drawer**.
- `health_check` **auto-runs on file open** (fast); the QC button shows a badge
  with the finding count and highest severity.
- Findings are severity-ranked rows: severity dot · kind · column · detail.
- **Click-to-jump:** clicking a row-scoped finding (glitch / regime / changepoint
  / gap) sets the focused graph's X-range to that row/time window and highlights
  the implicated channel. Dataset-level findings (dead/frozen/missing) just
  highlight the channel.
- `regime_change_event` findings show the traced culprit + affected channels.

### 3. Render options (Settings)

Three new per-graph controls appended to the existing Settings panel:

- **Autoscale** — `Min/Max` (default) · `Robust` (1st–99th percentile clip).
- **Y-scale** — `Linear` (default) · `Log` (log10; non-positive dropped).
- **Downsample** — `Min/Max` (default, spike-safe) · `LTTB` · `None`.

These map onto the existing per-graph render settings mirrored into `App.svelte`.

### 4. Formula / derived columns

- A `+ ƒ` action in the ColumnDialog (and/or SeriesList) opens a small formula
  editor: name + expression over column names, e.g.
  `deg(acos(calibrated_az / total_gravity))`, `sqrt(ax^2+ay^2+az^2)`.
- On confirm, the expression is evaluated (via `processing::expr`) into a new
  column appended to the in-memory dataset; it then behaves like any other column
  — selectable as a series, tabled, or analyzed.
- Errors (unknown column, bad syntax) surface inline in the editor.

## Build order (each slice: spec → plan → implement → verify)

1. **Foundation** — extract compute to `oxideplot-core`; verify `rustfft` builds
   for wasm; MCP re-points to core; core unit tests pass (native).
2. **Render options** — quick win that exercises the core→WASM→Svelte plumbing
   end to end on the smallest surface.
3. **Dist tab** — introduce the view-tab framework using the simplest new view.
4. **Spectrum + Spectrogram** — the FFT views (sample-rate handling, heatmap render).
5. **Formula columns** — the column-workflow extension.
6. **QC drawer** — the health scan surface with click-to-jump.

## Resolved design decisions

- Dist and Spectrogram operate on the **selected** series; Spectrum **overlays**
  all visible series.
- QC **auto-runs on file open** (with a manual re-run available in the drawer).
- Sample rate is **inferred** from a datetime X, with a manual override field
  when the X axis is not time.
- One implementation of each algorithm, in `oxideplot-core`, shared by app + MCP.

## Out of scope (deferred, not lost)

- **Correlation** view (proven in the MCP but not among the four rollforward
  buckets) — natural later addition as a second QC-drawer tab.
- Scatter colored-by-time, plot annotations, multi-file overlay/diff, and the
  MWD survey-station auto-detect — future work, tracked separately.

## Testing strategy

- **Foundation / core:** native unit tests per moved module, asserting parity
  with the MCP's known-good outputs (e.g. PSD peaks at the injected frequencies,
  `health_check` catches the planted dead/glitch/regime/gap defects). Verify the
  wasm target compiles.
- **Each UI slice:** drive the running app (via the `run` skill) on a sample MWD
  CSV and confirm the surface behaves — the view renders, the QC finding jumps,
  the formula column appears, the toggle changes the plot.
- Parity check: the app's output for a given file matches the MCP's for the same
  operation, since both call the same core.
