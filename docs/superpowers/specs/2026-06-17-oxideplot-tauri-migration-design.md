# OxidePlot → Tauri Migration — Design

**Date:** 2026-06-17
**Status:** Draft for review
**Author:** Will (with Claude)

## Goal

Migrate OxidePlot's frontend from `egui`/`eframe` to **Tauri 2 + Svelte 5**, while
**preserving the bespoke wgpu GPU rendering engine** by compiling it to WebAssembly
and running it on an HTML `<canvas>` inside the webview. The result is a more
user-friendly desktop data-visualization app consistent with the rest of the
author's Tauri stack (DoppelArm, Klaxon, RustCOM), suitable for the herrlabs
portfolio.

This spec covers a **2D-first MVP** scoped to roughly a few days. 3D plotting is
deferred to a clean follow-up.

A second planned follow-up (after the migration MVP) exposes `oxideplot-core` as an
**MCP server** so Claude can drive OxidePlot as a tool — load data, get statistical
summaries, build graph specs, render PNGs, and iterate ("make good graphs and learn
from the data"). It is **not** part of the few-days MVP, but the core is designed so
it drops in without rework. See "Follow-up Phase: Claude / MCP Integration".

## Motivation

- `egui` immediate-mode UI is unpleasant to work in, especially for text-heavy and
  interaction-heavy chrome (axes, tick labels, cursors, dialogs, settings).
- The genuinely valuable, hard-won code is **not** the UI — it's the data engine
  (CSV/Excel parsing, datetime detection, unit inference), the processing
  (LTTB downsampling, statistics), and the **custom WGSL shaders + wgpu renderer**.
  None of that is coupled to egui in a way that forces a rewrite.
- A web frontend makes the painful parts (crisp text axes, cursors, legends,
  settings panels) *easier*, not harder.

## Non-Goals / Deferred

Explicitly **out of scope** for this MVP:

- **3D plotting** (`plot3d/`) — ported as a follow-up. Same pattern, ~2x the work.
- The large `ROADMAP.md` backlog: FFT, curve fitting, new file formats (Parquet,
  HDF5, JSON), CLI, real-time/serial feeds, scripting, AI features.
- New analysis features beyond what the current 2D app already does.

The MVP target is **feature parity with today's 2D app, on Tauri, with nicer UX** —
not new capabilities.

## Architecture

Three layers. The guiding principle: **salvage the engine, rebuild the shell.**

```
┌─────────────────────────────────────────────────────────────┐
│  Tauri app                                                    │
│                                                               │
│  ┌─────────────────────────┐   IPC    ┌────────────────────┐ │
│  │ Svelte 5 frontend        │ ───────► │ Rust backend       │ │
│  │  - toolbar, series list  │ commands │  (OS integration)  │ │
│  │  - settings, dialogs     │ ◄─────── │  - file dialog     │ │
│  │  - SVG overlay:          │          │  - read bytes      │ │
│  │    axes/ticks/cursors    │          │  - save PNG/CSV    │ │
│  │  - hosts <canvas>        │          │  - persist prefs   │ │
│  └───────────┬─────────────┘          └────────────────────┘ │
│              │ JS calls (wasm-bindgen)                         │
│  ┌───────────▼─────────────────────────────────────────────┐ │
│  │ WASM render module  (oxideplot-core compiled to wasm32)  │ │
│  │  - owns wgpu surface on <canvas>                         │ │
│  │  - owns loaded data, runs LTTB locally                   │ │
│  │  - pan/zoom = update uniforms, redraw (no IPC per frame) │ │
│  └──────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘

         oxideplot-core (Rust lib crate) — compiles to native AND wasm32
         ┌──────────────────────────────────────────────────────┐
         │ data/        CSV/Excel/datetime/unit inference (as-is) │
         │ processing/  LTTB, statistics, math (as-is)            │
         │ state/       data models (egui types stripped)         │
         │ render/      2D wgpu renderer (egui coupling removed)   │
         │ plot3d/      3D wgpu renderer (DEFERRED, kept in crate) │
         └──────────────────────────────────────────────────────┘
```

### Layer 1 — `oxideplot-core` (Rust library crate)

A new library crate holding everything that is not egui. Compiles to both native
(`x86_64-pc-windows-msvc`) and `wasm32-unknown-unknown`.

| Module | Source today | Change |
|--------|--------------|--------|
| `data/` | `src/data/*` | Move as-is. Verify `calamine`/`csv`/`chrono` build on wasm32. |
| `processing/` | `src/processing/*` | Move as-is (pure compute). |
| `state/` data models | `src/state/data_series.rs`, `graph_state.rs` | Move; strip egui-derived types (colors → `[f32;4]`, etc.). |
| `render/` | `src/render/*` | Move; **remove egui coupling** (see below). |
| `plot3d/` | `src/plot3d/*` | Move; deferred but kept compiling. |

**Removing egui coupling from `render/`:** the WGSL shader (`PLOT_SHADER_SRC`) and
the buffer/bind-group/uniform building logic are already egui-independent. The only
egui-specific surface area is:

1. `init_gpu_resources` takes an `egui_wgpu::RenderState` to get `device` +
   `target_format`, then stores pipelines in egui's `callback_resources` TypeMap.
   → Replace with a plain `PlotRenderer` struct that owns its own `device`, `queue`,
   `surface`, `config`, and pipelines.
2. `GpuPlotCallback` implements `egui_wgpu::CallbackTrait` (`prepare`/`paint`) and
   renders into egui's render pass, reading viewport/scissor from
   `egui::PaintCallbackInfo`.
   → Replace with a `render()` method that acquires the surface texture, opens its
   own render pass over the full canvas, and runs the same draw calls
   (`set_pipeline` / `set_bind_group` / `draw(0..6, 0..instance_count)`).

Everything else — the shader, `PlotUniforms`, `SeriesGpuData`, `GridGpuData`, the
line/step/point buffer construction — is reused verbatim.

**Design constraint for the MCP follow-up:** make the renderer's *target* an
abstraction, not a hard-coded surface. The MVP only needs the **surface target**
(canvas via WASM), but structuring `render()` to accept either a surface texture or
an **offscreen texture** means the same renderer can later produce PNGs natively
(render → texture → copy to buffer → encode) with no engine changes. This is the
one place the MVP should look ahead; everywhere else, build only what the MVP needs.

### Layer 2 — WASM render module

`oxideplot-core` compiled to `wasm32-unknown-unknown` plus a thin `wasm-bindgen`
wrapper that exposes an imperative API to JavaScript:

```
Renderer.init(canvas: HTMLCanvasElement) -> Promise<Renderer>
  .load_columns(columns)        // hand parsed data into the engine
  .set_series(specs)            // which X/Y, colors, draw mode
  .set_view(min, max)           // view window in data coords
  .pan(dx_px, dy_px)
  .zoom(factor, anchor_px)
  .auto_fit()
  .resize(w, h, dpr)
  .render()
  .screenshot() -> Uint8Array   // PNG bytes for export
  .view_state() -> {min,max}    // so the SVG overlay can draw matching ticks
```

Responsibilities: owns the wgpu surface bound to the `<canvas>`, owns the loaded
dataset, runs LTTB downsampling to the current viewport locally, draws. Because
interaction stays inside WASM, **pan/zoom never round-trip through Tauri IPC** —
it feels as responsive as the current native app.

### Layer 3 — Tauri app

**Rust backend (`src-tauri`)** — OS integration only, exposed as Tauri commands:

| Command | Purpose |
|---------|---------|
| `open_file_dialog()` | Native file picker (`rfd`), returns path. |
| `read_file(path)` | Read bytes off disk, return to frontend (webview can't read arbitrary paths). |
| `save_png(bytes, path)` / `export_csv(...)` | Write exports to disk via save dialog. |
| `load_prefs()` / `save_prefs(...)` | Persist recent files, theme, window state. |

May depend on `oxideplot-core` natively (e.g. to parse server-side or for a future
CLI), but the MVP keeps parsing in WASM to avoid duplicating the data path.

**Svelte 5 frontend** — all UI chrome plus the canvas host:

- Toolbar (open, export, fit, draw-mode toggle, theme)
- Series list / legend (add/remove, color, z-order, show/hide)
- Settings panel and column-selection dialog (replacing
  `settings_dialog.rs` / `data_selection_dialog.rs`)
- Hosts the `<canvas>` and drives the WASM `Renderer`
- **SVG overlay** drawn over the canvas for axes, tick labels, measurement cursors,
  and crosshair — reading the view transform from `Renderer.view_state()`. This is
  the chief UX upgrade: crisp DOM text and native pointer interaction instead of
  egui-painted text.
- Light/dark theme via CSS variables.

## Data Flow

1. **Open:** user clicks Open → `open_file_dialog` → `read_file` returns bytes →
   frontend passes bytes to WASM → WASM parses (reusing `data/`) and reports columns.
2. **Plot:** user selects X + Y column(s) in the dialog → `set_series` → WASM builds
   GPU buffers and renders; `auto_fit` sets the initial view.
3. **Interact:** canvas captures mouse/wheel → `pan`/`zoom` on WASM → WASM
   re-downsamples the viewport (LTTB) and redraws. The Svelte SVG overlay re-reads
   `view_state()` and redraws ticks/labels/cursors to match.
4. **Export:** PNG → WASM `screenshot()` returns bytes → `save_png`. CSV → backend
   `export_csv` from the current series.

## Error Handling

- WASM API returns `Result`-mapped JS errors (via `wasm-bindgen`); the frontend
  surfaces them as toasts, never silent failures.
- File read / parse errors flow back as user-facing messages ("Couldn't parse
  column 3 as a number on row 812").
- The renderer must not panic the webview: replace `unwrap()` on the rendering hot
  path with error returns (this also clears several `ROADMAP.md` known-bug items).
- WebGPU init failure triggers the documented fallback path (see Risks).

## Testing Strategy

- **`oxideplot-core` unit tests** run native (`cargo test`): datetime parsing, CSV
  edge cases, LTTB correctness (downsampled data preserves min/max), unit inference,
  statistics. Several of these are roadmap items we get to bank now.
- **Round-trip tests:** load → series → export CSV → reload matches.
- **Manual/visual acceptance** against a parity checklist (below), comparing the
  Tauri build to the current egui build on the same sample files.
- WASM smoke test: the spike (Phase 0) is itself the first integration test.

## MVP Feature Parity Checklist

Target = today's 2D `README.md` features, on Tauri:

- [ ] GPU 2D plotting: lines, step, scatter
- [ ] CSV + Excel import (drag-drop and dialog), header auto-detect
- [ ] Multi-series against shared X; multi-unit normalized overlay
- [ ] ISO 8601 / RFC 3339 timestamp detection on X axis
- [ ] Interactive pan / zoom / auto-fit
- [ ] Measurement cursors (vertical + horizontal) with delta readout
- [ ] LTTB downsampling for large datasets (100k+ points smooth)
- [ ] Export: PNG, CSV, copy image to clipboard
- [ ] Series z-order / reorder
- [ ] Light + dark theme
- [ ] SVG axis ticks + labels overlay (UX upgrade over egui text)

## Risks & Mitigations

| Risk | Mitigation |
|------|-----------|
| **wgpu-on-wasm + WebGPU inside WebView2 may not init cleanly** (the flagged time-sink) | **Phase 0 spike** before any porting: render a single triangle on a `<canvas>` in the Tauri webview. Confirm WebGPU, with WebGL2 as fallback backend. |
| Spike fails entirely | Fallback: reuse the WGSL **verbatim** but drive WebGPU from TypeScript for just the draw loop; keep data/processing in the backend. |
| `calamine`/`csv`/`chrono` wasm32 build issues | Verify in Phase 0; if a crate won't build for wasm, move that parsing to the Tauri backend and pass parsed columns to WASM. |
| Canvas resize / HiDPI / devicePixelRatio handling | Centralize in `resize(w,h,dpr)`; test on a HiDPI display early. |
| Scope creep from `ROADMAP.md` | This spec's checklist is the line. New features are post-MVP. |

## Rough Sequence (milestones)

0. **Spike:** triangle on canvas in Tauri webview (WebGPU + WebGL2 fallback). Decide go/no-go on the WASM-renderer path.
1. **Extract `oxideplot-core`:** move data/processing/state, strip egui types, `cargo test` green native + `cargo build --target wasm32` green.
2. **Standalone 2D renderer:** remove egui coupling, own surface/device/render pass; render a hard-coded series on the canvas.
3. **Wire data path:** Tauri file dialog + read → WASM parse → column dialog → `set_series` → real plot.
4. **Interaction + overlay:** pan/zoom/auto-fit; SVG axes/ticks/cursors overlay.
5. **Chrome + export:** toolbar, series list, settings, theme, PNG/CSV export, persistence.
6. **Parity pass + polish:** walk the checklist against the egui build; portfolio screenshots / short demo clip.

**Follow-up phases (after the MVP ships):**

7. **3D port:** standalone 3D renderer + orbit camera + 3D axis overlay.
8. **MCP server:** native binary on `oxideplot-core` with offscreen render; `load_data` / `describe_data` / `create_graph` / `render_png`; Claude-driven graphing loop.

## Follow-up Phase: Claude / MCP Integration

**Built after the migration MVP. Designed-for now, not built now.**

Goal: let Claude drive OxidePlot as a tool to "make good graphs and learn from the
data." Implemented as an **MCP server** — a small native Rust binary that depends on
`oxideplot-core` and renders **offscreen** (no webview, no canvas), returning PNGs
and structured summaries.

Proposed MCP tools:

| Tool | Returns | Purpose |
|------|---------|---------|
| `load_data(path)` | columns, row count, inferred types | Ingest a file into a session. |
| `describe_data()` | per-column stats, ranges, missing counts, correlations, outliers | The "learn from the data" surface — what Claude reasons over. |
| `suggest_views()` | candidate graph specs | Heuristic starting points (time series vs scatter vs distribution). |
| `create_graph(spec)` | graph id | Build/configure a graph (X/Y, draw mode, axes, range). |
| `render_png(graph_id)` | PNG bytes | Image Claude can *see* and critique, then refine the spec. |
| `export(graph_id, fmt)` | path | Save PNG/CSV to disk. |

Loop: Claude calls `load_data` → `describe_data` (reads the data) → `create_graph` →
`render_png` (looks at it) → adjusts spec → re-renders until the graph is good.

This reuses `oxideplot-core` wholesale: `data/` for ingest, `processing/` for the
`describe_data` statistics, and the renderer with an **offscreen target** for
`render_png`. The only genuinely new code is the MCP server glue and the
`describe_data` / `suggest_views` summarization logic. It also satisfies the
roadmap's headless-render and AI-insight items.

Out of scope even for this follow-up: fine-tuning / model training (the "learn"
here means Claude analyzing data at inference time, not training a model).

## Portfolio Framing

The story: *"Rewrote my Rust GPU plotting app from egui to a Tauri + Svelte
desktop app, keeping the custom WGSL/wgpu rendering engine alive by compiling it to
WebAssembly and running it on a WebGPU canvas — one Rust core shared across web
(canvas), native (offscreen), and an MCP server that lets Claude analyze data and
generate graphs on its own."* A distinctive, technically meaty narrative — GPU
rendering + WASM + an AI-controllable tool — that doesn't overlap with RustCOM's
plotting.
