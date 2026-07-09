# OxidePlot

A **GPU-accelerated data-visualization desktop app** — and an **MCP analysis server** that lets an AI agent (Claude) work with the same data. The rendering and data engine is written in Rust and compiled twice from one shared crate: to **WebAssembly** for the desktop app's WebGPU canvas, and to **native** for the MCP server and tests. Both share exactly the same plotting, parsing, and analysis code.

Built with **Tauri 2 + Svelte 5 + WebGPU**. Native-feeling interactivity — GPU line rendering, viewport-aware downsampling, zero-copy data paths — in a single installable desktop binary.

## Two ways to use it

- **As a desktop graphing app.** Open a CSV / Excel / `.dat`, plot it, and explore — fast GPU line plots, distribution / spectrum / spectrogram / scatter views, formula columns, and a rubber-band zoom. If you just want a great plotting tool, grab the installer (see [Releases](#install)) and go.
- **As an AI analysis backend (MCP).** Point Claude (or any MCP client) at the `oxideplot-mcp` server and ask it to load a log, run a QC pass, render plots, and hand back a single self-contained HTML report. Originally built for MWD / sensor-log QC, but works on any tabular time-series.

## Screenshots

<!-- add screenshots of the plot / spectrogram / scatter / report here -->

---

# The desktop app

## Loading data

- **Formats:** CSV, Excel (`.xlsx` / `.xls`), and **generic delimited text** (`.dat` / `.txt` / `.tsv`) with **automatic delimiter detection** (tab, comma, semicolon, pipe) and **metadata-preamble skipping** — instrument dumps with header blocks (e.g. spectrum-analyzer `.dat` files) load without any special handling.
- **Datetime handling:** ISO 8601 / RFC 3339, common `MM/DD/YYYY`-style formats, **12-hour `hh:mm:ss AM/PM`** times, and **separate `Date` + `Time` columns auto-merged** into a single timestamp axis. Large epoch timestamps render precisely (a large-coordinate offset keeps GPU vertices inside f32 precision).
- **How:** the toolbar **Open** button, **drag-and-drop a file onto any graph**, or the recent-files list. Blank/unnamed columns are auto-labeled so they're always selectable.

## Views — per-graph tabs

Each graph has its own tab strip; switch a graph between:

| View | What it shows |
|---|---|
| **Plot** | GPU-rendered line / step / points over the X axis |
| **Table** | The raw rows, scoped to the columns you plotted, sortable + filterable |
| **Dist** | A small-multiple histogram per plotted series, in its color |
| **Spectrum** | Overlaid power-spectral-density (FFT) of the plotted series, log-Y |
| **Spectrogram** | A frequency-vs-time magma heatmap of the selected series (STFT) |
| **Scatter** | One column against another, points colored by time (early → late) — for hysteresis, saturation, cluster shapes |

## Navigation

- **Left-drag — rubber-band zoom box** with axis snapping: a mostly-horizontal drag snaps to a **full-height X band** (zoom X only), mostly-vertical to a **full-width Y band** (zoom Y only), and a diagonal drag draws an **XY box**.
- **Right-drag — pan.**
- **Wheel — scroll** the graph stack; **Ctrl/Cmd + wheel — zoom** under the cursor.
- **Double-click — fit** to data.

## Formula columns

The **`+ƒ` editor** derives a new column from an expression over the existing columns and plots it immediately. Multi-word column names are referenced with quotes.

```
sqrt("raw_ax"^2 + "raw_ay"^2 + "raw_az"^2)      # vector magnitude
deg(atan2("accel_x", "accel_z"))                # recompute inclination and overlay it
"Temp PV °C" - "Temp Setpoint °C"               # error signal
```

Supported: `+ - * / ^`, parentheses, comparisons + `and`/`or`, and functions `sqrt sin cos tan asin acos atan atan2 hypot deg rad abs exp ln log10 floor ceil round sign min max`.

## Render options, workspace, and more

- **Render options** (Settings): robust autoscale (clip outliers to the 1st–99th percentile), log-Y, min/max-envelope downsampling, normalized multi-unit overlay, line width, point radius, grid.
- **Multi-graph workspace:** a vertical stack of graphs, each with its own file; scroll a tall stack, cross-graph X-sync, add/remove graphs.
- **Measurement cursors:** vertical/horizontal cursor pairs with ΔX / ΔY readout.
- **Export:** PNG (composited **with axes + a series legend**), CSV, and copy-to-clipboard.
- **Light / dark theme**, persisted across sessions along with all preferences.
- **Viewport-aware downsampling** keeps 100k+ point datasets fluid — the rendered sample count tracks canvas resolution.

---

# The MCP analysis server

`oxideplot-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server that exposes OxidePlot's data engine as tools an LLM agent can call — load data, understand it, run QC, render plots, and produce a deliverable. It shares `oxideplot-core` with the desktop app, so the analysis and the pictures match what you'd see in the UI.

**Tools:**

- **Data:** `load_csv`, `describe_data` (per-column QC stats), `query_data` (filter + window rows), `export_csv`, `derive_column` (magnitude / A−B / rolling ops / free-form `expr`).
- **QC:** `health_check` — ranked findings (dead/frozen channels, out-of-range regimes vs. glitches, robust change-points, time gaps) with raw-source lineage.
- **Frequency:** `spectrum` (PSD peaks), `spectrogram` (STFT), `segment_stats`, `correlate`.
- **Plots:** `create_graph` + `render_graph` (returns a PNG), `histogram`.
- **Deliverable:** **`report`** — one call bundles `health_check` + auto-selected plots + the flagged-row export into a **single self-contained HTML document** (base64-embedded images, inline styles) that a field engineer or customer can be handed directly.

### Using it with Claude

Build the server and register it in your MCP client config (e.g. Claude Desktop / Claude Code):

```jsonc
{
  "mcpServers": {
    "oxideplot": {
      "command": "C:/path/to/OxidePlot/target/release/oxideplot-mcp.exe"
    }
  }
}
```

Then: *"Load `pump-run.csv`, run a health check, and give me a report."*

---

# Architecture

The core idea is a **single shared engine compiled to two targets**.

```
crates/oxideplot-core   ← data engine: parsing (CSV/Excel/delimited), datetime,
        │                 downsampling, statistics, QC, spectral (FFT/STFT),
        │                 expression evaluator, the wgpu PlotRenderer + WGSL shaders
        │
        ├── crates/oxideplot-wasm  ← wasm-bindgen wrapper → WebGPU canvas in the Tauri webview
        │        └── paired with src/ (Svelte 5 frontend + SVG axis/cursor overlays)
        │        └── src-tauri/ (Tauri 2 backend: file I/O, dialogs, prefs)
        │
        └── crates/oxideplot-mcp   ← native MCP server (rmcp) — the same core, offscreen
```

`oxideplot-core` is target-agnostic Rust. It compiles to `wasm32-unknown-unknown` for the app (WebGPU canvas) and to native for the MCP server, tests, and offscreen PNG rendering. All compute — parsing, downsampling, QC heuristics, FFT/STFT, the expression evaluator, the renderer — lives here once.

> OxidePlot was migrated from an egui/eframe desktop app. The migration preserved the bespoke wgpu rendering engine wholesale — extracted into `oxideplot-core` and recompiled to WASM — while the egui UI was replaced with Svelte + Tauri, and the shared core later grew the MCP server.

## Project layout

```
crates/
  oxideplot-core/   Rust engine: parsing, datetime, downsampling, statistics,
                    QC, spectral, expr evaluator, wgpu PlotRenderer, WGSL shaders
  oxideplot-wasm/   wasm-bindgen wrapper — bridges core to JS/TS (cdylib + rlib)
  oxideplot-mcp/    native MCP server exposing core as analysis tools

src-tauri/          Tauri 2 backend: file dialogs, read/write, prefs store
src/                Svelte 5 frontend (canvas host, axis/cursor overlays, panels)
  lib/wasm/         Generated WASM output — gitignored; build before running
```

---

# <a name="install"></a>Install (end users)

Download the latest **Windows installer** (`.exe` / `.msi`) from the [Releases](https://github.com/willherr72/OxidePlot/releases) page and run it. WebView2 (already present on Windows 10/11) is the only runtime dependency.

# Build from source

## Prerequisites

- [Rust](https://rustup.rs/) with the wasm target: `rustup target add wasm32-unknown-unknown`
- [wasm-pack](https://rustwasm.github.io/wasm-pack/installer/)
- [Node.js](https://nodejs.org/) 18+ and npm
- [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform

## Desktop app

```sh
npm install

# Build the WASM module (src/lib/wasm/ is gitignored — required on a fresh checkout)
npm run build:wasm            # dev profile (fast) — for `tauri dev`

npx tauri dev                 # run in development (hot-reload frontend)

npx tauri build               # package installers → src-tauri/target/release/bundle/
                              # (auto-rebuilds the optimized WASM first)
```

> `src/lib/wasm/` is generated. Run `npm run build:wasm` before `tauri dev` / `npm run build` on any fresh checkout, or you'll get a "module not found" at startup.

## MCP server

```sh
cargo build -p oxideplot-mcp --release
# → target/release/oxideplot-mcp.exe  (register it in your MCP client, see above)
```

## Workspace Rust build / tests

```sh
cargo build            # or --release
cargo test             # core + native tests
cargo build -p oxideplot-wasm --target wasm32-unknown-unknown   # the real WASM build gate
```

# Roadmap

- **3D plotting** — port the dormant 3D renderer (camera, mesh pipeline) to the same WASM/WebGPU canvas.
- **Multi-file overlay / diff** in the desktop app.
- **Plot annotations** and survey-station auto-detect for MWD workflows.

# License

All rights reserved.
