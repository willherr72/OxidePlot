# OxidePlot

A high-performance desktop data-visualization app built with **Tauri 2 + Svelte 5**, where the entire rendering and data engine is written in Rust, compiled to **WebAssembly**, and driven over a **WebGPU canvas** inside the webview. The result is native-feeling interactivity — GPU-accelerated line rendering, viewport-aware downsampling, and zero-copy data paths — packaged as a single installable desktop binary.

## Screenshots

<!-- add screenshots of the dark/light plot here -->

## Architecture

The interesting part of OxidePlot is the shared-core model.

```
crates/oxideplot-core   ← data engine + wgpu PlotRenderer + WGSL shaders
        │
        ├── crates/oxideplot-wasm  ← wasm-bindgen wrapper, exposes core to JS
        │        └── compiled to WebGPU canvas inside Tauri webview
        │
        └── src-tauri              ← Tauri backend: file I/O, dialogs, prefs
                 └── paired with src/ (Svelte 5 frontend + SVG overlays)
```

`oxideplot-core` is target-agnostic Rust: it owns the data parsing pipeline (CSV, Excel), LTTB downsampling, statistics, view/interaction state, the `wgpu` `PlotRenderer`, and all WGSL shaders. Today it runs in the webview via WASM on a WebGPU canvas; the same crate is designed to also drive a native offscreen renderer (for a planned Claude/MCP server that can load data, render a PNG, and return it).

OxidePlot was migrated from an egui/eframe desktop app. The migration preserved the bespoke wgpu rendering engine wholesale — it was extracted into `oxideplot-core` and recompiled to WASM, while the egui UI layer was replaced with Svelte + Tauri.

## Features

- **GPU-accelerated rendering** — Custom WGSL shaders for line, step, and scatter modes via `wgpu` on a WebGPU canvas
- **CSV and Excel import** — File dialog, drag-and-drop, and recent-file list with automatic header detection
- **Multi-series plotting** — Multiple Y columns against a shared X axis; ISO 8601 / RFC 3339 datetime X axis supported
- **Normalized multi-unit overlay** — Series with different units plotted on a shared normalized Y axis
- **Interactive pan and zoom** — Click-drag to pan, scroll to zoom, auto-fit to data
- **Viewport-aware LTTB downsampling** — Handles 100k+ point datasets without lag; sample count tracks canvas resolution
- **SVG axes and tick labels** — Crisp vector overlays rendered by Svelte on top of the GPU canvas
- **Measurement cursors** — Vertical/horizontal cursor pairs with ΔX / ΔY readout (per-unit for multi-axis)
- **Series list** — Toggle visibility, reorder series, adjust rendering priority
- **Settings** — Line width, point radius, grid on/off
- **Export** — Save as PNG, save as CSV, copy image to clipboard
- **Light / dark theme** — Persisted across sessions alongside all other user preferences

## Project Layout

```
crates/
  oxideplot-core/      Rust engine: data parsing, downsampling, statistics,
                       wgpu PlotRenderer, WGSL shaders, view/interaction state
  oxideplot-wasm/      wasm-bindgen wrapper — thin bridge from core to JS/TS

src-tauri/             Tauri 2 backend: file dialogs, read/write, prefs store

src/                   Svelte 5 frontend
  lib/wasm/            Generated WASM output (gitignored — build before running)
  components/          Svelte components: canvas host, axis overlays, panels
```

## Build and Run

### Prerequisites

- [Rust](https://rustup.rs/) (edition 2021) with the `wasm32-unknown-unknown` target:
  ```
  rustup target add wasm32-unknown-unknown
  ```
- [wasm-pack](https://rustwasm.github.io/wasm-pack/installer/)
- [Node.js](https://nodejs.org/) (v18+) and npm
- [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform (WebView2 on Windows is usually already present)

### Steps

```sh
# 1. Install JS dependencies
npm install

# 2. Build the WASM module (must be run on a fresh checkout — src/lib/wasm/ is gitignored)
npm run build:wasm
# Equivalent to: wasm-pack build crates/oxideplot-wasm --target web --out-dir ../../src/lib/wasm
# For a dev (unoptimized) build the dev script adds --dev; omit it for release.

# 3. Run in development mode (hot-reload frontend, debug Tauri backend)
npx tauri dev

# 4. Package for distribution (produces MSI + NSIS installers)
npx tauri build
# Installers land in target/release/bundle/
```

> **Note:** `src/lib/wasm/` is generated and gitignored. `npm run build:wasm` **must** be run before `npm run dev`, `npm run build`, or `npx tauri dev` on any fresh checkout. Skipping this step will produce a "module not found" error at startup.

### Workspace-only Rust build (no frontend)

```sh
cargo build        # debug
cargo build --release
```

## Roadmap

- **3D plotting** — Port the dormant 3D renderer (camera, mesh pipeline) to the same WASM/WebGPU canvas
- **Claude / MCP server** — Expose `oxideplot-core`'s offscreen renderer as an MCP tool so an LLM agent can load data, describe it, render a PNG, and iterate without a UI

## License

All rights reserved.
