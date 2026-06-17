# OxidePlot Tauri Migration (MVP) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate OxidePlot's 2D plotting from an egui desktop app to a Tauri 2 + Svelte 5 app, reusing the existing WGSL/wgpu engine by compiling a new `oxideplot-core` crate to WebAssembly and running it on a WebGPU canvas.

**Architecture:** Extract all non-egui logic (data, processing, state, 2D renderer) into a reusable `oxideplot-core` library crate. A thin `oxideplot-wasm` wrapper compiles core to `wasm32` and drives a `<canvas>`. A Tauri Rust backend handles OS integration (file dialogs, disk I/O, persistence). A Svelte 5 frontend owns the UI chrome and draws axes/cursors as SVG overlays.

**Tech Stack:** Rust (edition 2021), wgpu 24.x (matched to eframe 0.31), `wasm-bindgen` + `wasm-pack`, Tauri 2, Svelte 5 + Vite + TypeScript, `csv` + `calamine` + `chrono` for parsing.

## Global Constraints

- **Branch:** all work on `tauri-migration` (already created).
- **Rust edition:** 2021.
- **wgpu version:** must match what `eframe 0.31` resolves (run `cargo tree -i wgpu` in the legacy crate; expected `24.x`). Pin that exact version in `oxideplot-core` so the existing WGSL and pipeline descriptors compile unchanged.
- **No polars:** `polars` is unused; it must not appear in any new crate's dependencies.
- **MVP scope = 2D parity only.** No 3D, no `ROADMAP.md` features (FFT, fitting, new formats, CLI, scripting, AI). `plot3d/` stays in the legacy crate for this MVP; it is ported in the 3D follow-up.
- **Render target abstraction:** the renderer's draw target is an abstraction (`Surface | Offscreen`); only `Surface` is implemented in the MVP. Do not implement offscreen/PNG-native rendering here (that is the MCP follow-up).
- **Frontend framework:** Svelte 5 (runes) to match DoppelArm / Klaxon / RustCOM.
- **License:** "All rights reserved" (unchanged).
- **Commit cadence:** one commit per completed step group as marked; never commit a non-compiling tree.

---

## File Structure (target state)

```
OxidePlot/
├── Cargo.toml                       # MODIFIED: becomes [workspace] root
├── package.json                     # NEW: Svelte/Vite frontend
├── vite.config.ts                   # NEW
├── svelte.config.js                 # NEW
├── index.html                       # NEW
├── crates/
│   ├── oxideplot-core/              # NEW lib crate (native + wasm32)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── geom.rs              # NEW: plain Rect/Pos to replace egui types
│   │       ├── data/               # MOVED from src/data (verbatim)
│   │       ├── processing/         # MOVED from src/processing (verbatim)
│   │       ├── state/              # MOVED from src/state (egui types stripped)
│   │       └── render/             # MOVED from src/render (egui coupling removed)
│   ├── oxideplot-wasm/              # NEW cdylib: wasm-bindgen wrapper
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   └── oxideplot-egui-legacy/       # MOVED: the entire current egui app
│       ├── Cargo.toml               #   kept building for parity reference,
│       └── src/                     #   deleted in Task 6.3
├── src-tauri/                       # NEW: Tauri backend
│   ├── Cargo.toml
│   ├── build.rs
│   ├── tauri.conf.json
│   └── src/
│       ├── main.rs
│       └── commands.rs
└── src/                             # NEW: Svelte frontend source
    ├── main.ts
    ├── App.svelte
    └── lib/
        ├── renderer.ts              # wraps the wasm module
        ├── api.ts                   # wraps Tauri commands
        ├── overlay/
        │   ├── Axes.svelte
        │   └── Cursors.svelte
        └── components/
            ├── Toolbar.svelte
            ├── SeriesList.svelte
            ├── ColumnDialog.svelte
            └── Settings.svelte
```

---

## PHASE 0 — De-risking spike (GO / NO-GO gate)

> Purpose: prove WebGPU works inside Tauri's WebView2, and that the existing wgpu engine runs as `wasm32` on a canvas, **before** porting real code. If 0.3 fails, fall back to driving WebGPU from TypeScript (reusing the WGSL verbatim) and revisit the plan.

### Task 0.1: Scaffold Tauri 2 + Svelte 5 project

**Files:**
- Create: `package.json`, `vite.config.ts`, `svelte.config.js`, `index.html`, `src/main.ts`, `src/App.svelte`
- Create: `src-tauri/` (via Tauri CLI)

- [ ] **Step 1: Scaffold the frontend** (run at repo root)

```bash
npm create vite@latest . -- --template svelte-ts
npm install
```

If prompted about the non-empty directory, choose "Ignore files and continue". Do NOT let it delete `crates/`, `docs/`, or `src/` from the legacy app — if the template wants to overwrite `src/`, scaffold into a temp dir and copy `package.json`, `vite.config.ts`, `index.html`, `src/main.ts`, `src/App.svelte` over manually.

- [ ] **Step 2: Add Tauri**

```bash
npm install --save-dev @tauri-apps/cli@^2
npx tauri init --app-name oxideplot --window-title OxidePlot --frontend-dist ../dist --dev-url http://localhost:5173 --before-dev-command "npm run dev" --before-build-command "npm run build"
```

- [ ] **Step 3: Run the dev shell**

Run: `npx tauri dev`
Expected: a native window opens showing the default Vite + Svelte page. Close it.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "chore: scaffold Tauri 2 + Svelte 5 shell"
```

### Task 0.2: Prove WebGPU exists in the webview (TypeScript triangle)

**Files:**
- Create: `src/lib/spike/webgpu_triangle.ts`
- Modify: `src/App.svelte`

- [ ] **Step 1: Write a minimal WebGPU triangle in TS**

Create `src/lib/spike/webgpu_triangle.ts` that: requests `navigator.gpu.requestAdapter()`, creates a device, configures a `<canvas>` context, and draws one solid triangle with an inline WGSL shader. On failure (no `navigator.gpu`), log `"WEBGPU_UNAVAILABLE"` and attempt a WebGL2 context as the fallback probe.

```ts
export async function runTriangle(canvas: HTMLCanvasElement): Promise<string> {
  if (!navigator.gpu) {
    const gl = canvas.getContext("webgl2");
    return gl ? "NO_WEBGPU_BUT_WEBGL2" : "NO_WEBGPU_NO_WEBGL2";
  }
  const adapter = await navigator.gpu.requestAdapter();
  if (!adapter) return "NO_ADAPTER";
  const device = await adapter.requestDevice();
  const ctx = canvas.getContext("webgpu")!;
  const format = navigator.gpu.getPreferredCanvasFormat();
  ctx.configure({ device, format, alphaMode: "opaque" });
  const shader = device.createShaderModule({ code: `
    @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
      var p = array(vec2(0.0,0.5), vec2(-0.5,-0.5), vec2(0.5,-0.5));
      return vec4(p[i], 0.0, 1.0);
    }
    @fragment fn fs() -> @location(0) vec4<f32> { return vec4(0.2,0.8,1.0,1.0); }`});
  const pipeline = device.createRenderPipeline({
    layout: "auto",
    vertex: { module: shader, entryPoint: "vs" },
    fragment: { module: shader, entryPoint: "fs", targets: [{ format }] },
    primitive: { topology: "triangle-list" },
  });
  const enc = device.createCommandEncoder();
  const pass = enc.beginRenderPass({ colorAttachments: [{
    view: ctx.getCurrentTexture().createView(),
    clearValue: { r: 0, g: 0, b: 0, a: 1 }, loadOp: "clear", storeOp: "store" }]});
  pass.setPipeline(pipeline); pass.draw(3); pass.end();
  device.queue.submit([enc.finish()]);
  return "WEBGPU_OK";
}
```

- [ ] **Step 2: Mount it in App.svelte and display the status string**

Add a `<canvas>` and an `onMount` that calls `runTriangle` and renders the returned status in a `<p>`.

- [ ] **Step 3: Verify in the Tauri window**

Run: `npx tauri dev`
Expected: a cyan triangle on the canvas and the text `WEBGPU_OK`.
**If the status is `NO_WEBGPU_*`:** record it; you may need to enable the WebView2 WebGPU flag (set env `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--enable-unsafe-webgpu --enable-features=Vulkan` in `src-tauri/src/main.rs` via `std::env::set_var` before `tauri::Builder`). Retry. If still unavailable, the project falls back to WebGL2 — note this and flag for re-planning.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "spike: confirm WebGPU available in WebView2 (TS triangle)"
```

### Task 0.3: Prove wgpu-on-wasm runs on the canvas (the chosen path)

**Files:**
- Create: `crates/oxideplot-wasm/Cargo.toml`, `crates/oxideplot-wasm/src/lib.rs`
- Modify: `Cargo.toml` (workspace), `src/App.svelte`, `package.json` (wasm build script)

- [ ] **Step 1: Make the root a Cargo workspace**

Replace root `Cargo.toml` `[package]`/`[dependencies]` with:

```toml
[workspace]
resolver = "2"
members = ["crates/oxideplot-core", "crates/oxideplot-wasm", "crates/oxideplot-egui-legacy", "src-tauri"]

[profile.release]
opt-level = 3
lto = true
```

(The legacy crate move happens in Task 1.1; for this spike, temporarily list only `crates/oxideplot-wasm`.)

- [ ] **Step 2: Create the wasm spike crate**

`crates/oxideplot-wasm/Cargo.toml`:

```toml
[package]
name = "oxideplot-wasm"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
wgpu = { version = "24", features = ["webgpu"] }
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"
web-sys = { version = "0.3", features = ["HtmlCanvasElement", "Window", "Document"] }
console_error_panic_hook = "0.1"
```

`crates/oxideplot-wasm/src/lib.rs`: a `#[wasm_bindgen]` async `run_triangle(canvas: HtmlCanvasElement)` that creates a `wgpu::Instance`, a surface from the canvas (`wgpu::SurfaceTarget::Canvas`), requests adapter+device, and draws one triangle with an inline WGSL string (reuse the same shader text as 0.2). Call `console_error_panic_hook::set_once()` first.

- [ ] **Step 3: Add a wasm build script**

In `package.json` add:

```json
"scripts": {
  "build:wasm": "wasm-pack build crates/oxideplot-wasm --target web --out-dir ../../src/lib/wasm --dev"
}
```

Install the toolchain if needed: `rustup target add wasm32-unknown-unknown` and `cargo install wasm-pack`.

- [ ] **Step 4: Build the wasm module**

Run: `npm run build:wasm`
Expected: `src/lib/wasm/oxideplot_wasm.js` + `.wasm` are produced with no errors.

- [ ] **Step 5: Call it from Svelte and verify**

In `App.svelte`, import the generated module, `await init()`, then call `run_triangle(canvas)`.
Run: `npx tauri dev`
Expected: the triangle renders, drawn by **Rust/wgpu compiled to wasm**.
**GO/NO-GO:** if this renders, the chosen path is proven — proceed to Phase 1. If it fails after enabling the WebView2 flag, stop and re-plan around the TS-WebGPU fallback (Phase 2 would then drive WebGPU from TS, still reusing the WGSL verbatim).

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "spike: render wgpu-on-wasm triangle in webview (GO/NO-GO passed)"
```

---

## PHASE 1 — Extract `oxideplot-core`

> Move all non-egui logic into a library crate that builds for both native and `wasm32`. No behavior changes; this is pure restructuring plus characterization tests.

### Task 1.1: Create the core crate and move the legacy app

**Files:**
- Create: `crates/oxideplot-core/Cargo.toml`, `crates/oxideplot-core/src/lib.rs`
- Move: `src/` (entire current app) → `crates/oxideplot-egui-legacy/src/`; current root deps → `crates/oxideplot-egui-legacy/Cargo.toml`
- Modify: root `Cargo.toml` workspace members

- [ ] **Step 1: Move the egui app into a legacy crate**

```bash
mkdir -p crates/oxideplot-egui-legacy
git mv src crates/oxideplot-egui-legacy/src
```

Create `crates/oxideplot-egui-legacy/Cargo.toml` = the ORIGINAL root `Cargo.toml` contents (package name `oxideplot-egui-legacy`, the full original `[dependencies]` block including `eframe`, `egui*`, `polars`, etc.), edition 2021. This keeps the egui build alive for parity comparison.

- [ ] **Step 2: Create the core crate skeleton**

`crates/oxideplot-core/Cargo.toml`:

```toml
[package]
name = "oxideplot-core"
version = "0.1.0"
edition = "2021"

[dependencies]
wgpu = { version = "24" }
bytemuck = { version = "1", features = ["derive"] }
glam = { version = "0.29", features = ["bytemuck"] }
chrono = { version = "0.4", default-features = false, features = ["clock", "std"] }
csv = "1.3"
calamine = "0.28"
serde = { version = "1", features = ["derive"] }

[dev-dependencies]
# none yet
```

`crates/oxideplot-core/src/lib.rs`:

```rust
pub mod geom;
pub mod data;
pub mod processing;
pub mod state;
pub mod render;
```

(Modules are added in subsequent tasks; comment out `mod` lines until each exists so it compiles.)

- [ ] **Step 3: Update workspace members**

Root `Cargo.toml` members: `["crates/oxideplot-core", "crates/oxideplot-wasm", "crates/oxideplot-egui-legacy", "src-tauri"]`. (Add `src-tauri` once Task 3.1 creates it; until then omit it.)

- [ ] **Step 4: Verify both crates build**

Run: `cargo build -p oxideplot-egui-legacy && cargo build -p oxideplot-core`
Expected: both succeed (core builds an empty lib).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor: move egui app to legacy crate, add empty oxideplot-core"
```

### Task 1.2: Move `data/` into core (verbatim) + characterization tests

**Files:**
- Move: `crates/oxideplot-egui-legacy/src/data/` → `crates/oxideplot-core/src/data/`
- Test: `crates/oxideplot-core/src/data/datetime.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Move the module**

```bash
git mv crates/oxideplot-egui-legacy/src/data crates/oxideplot-core/src/data
```

In legacy, replace its own `data` usages with `oxideplot_core::data` (add `oxideplot-core = { path = "../oxideplot-core" }` to the legacy `Cargo.toml`). Uncomment `pub mod data;` in core `lib.rs`.

- [ ] **Step 2: Write a characterization test for datetime parsing**

Append to `crates/oxideplot-core/src/data/datetime.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_rfc3339() {
        // Use the module's real entry point; assert a known timestamp maps
        // to the expected epoch seconds (adjust fn name to the actual API).
        let ts = parse_datetime("2024-02-10T14:30:00Z").expect("should parse");
        assert!(ts > 0.0);
    }
    #[test]
    fn rejects_non_datetime() {
        assert!(parse_datetime("hello").is_none());
    }
}
```

(Replace `parse_datetime` with the module's actual public function discovered when moving the file.)

- [ ] **Step 3: Run native tests**

Run: `cargo test -p oxideplot-core`
Expected: PASS.

- [ ] **Step 4: Verify the wasm target compiles**

Run: `cargo build -p oxideplot-core --target wasm32-unknown-unknown`
Expected: success. **If `calamine` fails to build for wasm:** gate Excel parsing behind `#[cfg(not(target_arch = "wasm32"))]`, keep CSV-only on wasm, and add a note that Excel import is parsed in the Tauri backend instead. Verify CSV path still builds for wasm.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor: move data/ into core with datetime tests; wasm builds"
```

### Task 1.3: Move `processing/` into core + LTTB/stats tests

**Files:**
- Move: `.../processing/` → `crates/oxideplot-core/src/processing/`
- Test: inline tests in `downsampling.rs` and `statistics.rs`

- [ ] **Step 1: Move the module** (`git mv`), repoint legacy imports to `oxideplot_core::processing`, uncomment `pub mod processing;`.

- [ ] **Step 2: Write LTTB correctness tests**

Append to `crates/oxideplot-core/src/processing/downsampling.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn downsample_keeps_endpoints_and_count() {
        let xs: Vec<f64> = (0..10_000).map(|i| i as f64).collect();
        let ys: Vec<f64> = xs.iter().map(|x| (x * 0.01).sin()).collect();
        let out = lttb(&xs, &ys, 500); // adjust to actual signature
        assert_eq!(out.len(), 500);
        assert_eq!(out.first().unwrap().0, xs[0]);
        assert_eq!(out.last().unwrap().0, *xs.last().unwrap());
    }
    #[test]
    fn downsample_noop_when_target_exceeds_len() {
        let xs = vec![0.0, 1.0, 2.0];
        let ys = vec![0.0, 1.0, 0.0];
        let out = lttb(&xs, &ys, 100);
        assert_eq!(out.len(), 3);
    }
}
```

(Adjust `lttb` name/signature/return type to the real API.)

- [ ] **Step 3: Run tests** — `cargo test -p oxideplot-core` → PASS.
- [ ] **Step 4: Verify wasm build** — `cargo build -p oxideplot-core --target wasm32-unknown-unknown` → success.
- [ ] **Step 5: Commit** — `git add -A && git commit -m "refactor: move processing/ into core with LTTB tests"`

### Task 1.4: Move `state/` data models into core, strip egui types

**Files:**
- Create: `crates/oxideplot-core/src/geom.rs`
- Move: `.../state/data_series.rs`, `.../state/graph_state.rs` → core; leave `theme.rs` color choices in the frontend (CSS) — port only what the renderer needs.

- [ ] **Step 1: Add plain geometry types**

`crates/oxideplot-core/src/geom.rs`:

```rust
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rect { pub left: f32, pub top: f32, pub width: f32, pub height: f32 }
impl Rect {
    pub fn right(&self) -> f32 { self.left + self.width }
    pub fn bottom(&self) -> f32 { self.top + self.height }
}
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Pos2 { pub x: f32, pub y: f32 }
```

- [ ] **Step 2: Move data models and replace egui color types**

`git mv` the two files into `crates/oxideplot-core/src/state/` (create `state/mod.rs` exposing them). Replace each `egui::Color32` field/usage with `[f32; 4]` (RGBA 0..1). Where the legacy used `Color32::from_rgb(..)`, store the normalized array directly. Remove the two `egui::` imports flagged by grep (`data_series.rs`, `graph_state.rs`).

- [ ] **Step 3: Verify build (native + wasm)**

Run: `cargo build -p oxideplot-core && cargo build -p oxideplot-core --target wasm32-unknown-unknown`
Expected: both succeed.

- [ ] **Step 4: Commit** — `git add -A && git commit -m "refactor: move state models to core, replace egui colors with [f32;4]"`

---

## PHASE 2 — Standalone 2D renderer

> Remove the egui_wgpu coupling from `render/` and stand up a `PlotRenderer` that owns its own device/queue/surface. Then draw a hard-coded plot via the wasm wrapper.

### Task 2.1: Move `render/` into core and decouple from egui

**Files:**
- Move: `.../render/gpu_types.rs`, `gpu_plot.rs`, `plot_interaction.rs` → `crates/oxideplot-core/src/render/`
- Create: `crates/oxideplot-core/src/render/renderer.rs` (the new `PlotRenderer`)

- [ ] **Step 1: Move `gpu_types.rs` verbatim** — it has no egui dependency. Uncomment `pub mod render;` with a `render/mod.rs` exposing submodules.

- [ ] **Step 2: Port `plot_interaction.rs` to plain geometry**

Move the file; then: replace `egui::Rect`→`crate::geom::Rect`, `egui::Pos2`→`crate::geom::Pos2`. Delete `handle_input(response, rect)` (egui-specific) and replace with pure methods:

```rust
impl PlotViewState {
    pub fn pan(&mut self, dx_px: f32, dy_px: f32, rect: crate::geom::Rect) {
        let dx = -(dx_px as f64) * (self.x_max - self.x_min) / rect.width as f64;
        let dy = (dy_px as f64) * (self.y_max - self.y_min) / rect.height as f64;
        self.x_min += dx; self.x_max += dx; self.y_min += dy; self.y_max += dy;
        self.auto_fit = false;
    }
    pub fn zoom(&mut self, scroll_y: f32, anchor: crate::geom::Pos2, rect: crate::geom::Rect) {
        let zoom_factor = (1.0 - (scroll_y as f64) * 0.001).clamp(0.5, 2.0);
        let (cx, cy) = self.screen_to_data(anchor, rect);
        self.x_min = cx + (self.x_min - cx) * zoom_factor;
        self.x_max = cx + (self.x_max - cx) * zoom_factor;
        self.y_min = cy + (self.y_min - cy) * zoom_factor;
        self.y_max = cy + (self.y_max - cy) * zoom_factor;
        self.auto_fit = false;
    }
}
```

Keep `screen_to_data`, `data_to_screen`, `fit_to_data`, `auto_scale_y_to_visible`, `compute_grid_lines`, `format_tick_value` — change their `egui::Rect/Pos2` params to `crate::geom::*`.

- [ ] **Step 3: Write a unit test for the pure interaction math**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Rect, Pos2};
    #[test]
    fn pan_shifts_view_left() {
        let mut v = PlotViewState { x_min: 0.0, x_max: 10.0, y_min: 0.0, y_max: 10.0, ..Default::default() };
        let r = Rect { left: 0.0, top: 0.0, width: 100.0, height: 100.0 };
        v.pan(10.0, 0.0, r); // drag right by 10px => view moves left
        assert!(v.x_min < 0.0 && v.x_max < 10.0);
    }
    #[test]
    fn grid_lines_within_range() {
        for (val, _major) in compute_grid_lines(0.0, 100.0) { assert!((0.0..=100.0).contains(&val)); }
    }
}
```

Run: `cargo test -p oxideplot-core` → PASS. Commit: `refactor: move render interaction to core, decouple from egui`.

### Task 2.2: `PlotRenderer` owning device/queue/target

**Files:**
- Create: `crates/oxideplot-core/src/render/renderer.rs`
- Modify: `crates/oxideplot-core/src/render/gpu_plot.rs` (extract reusable pipeline/buffer code; drop the `egui_wgpu::CallbackTrait` impl)

- [ ] **Step 1: Extract pipeline creation**

Copy `PLOT_SHADER_SRC` and the pipeline-building body of `init_gpu_resources` from `gpu_plot.rs` into a `PlotRenderer::create_pipelines(device, format)` associated fn returning `(line_pipeline, point_pipeline, bind_group_layout)`. Change `eframe::wgpu` → `wgpu`. Keep the WGSL string byte-for-byte.

- [ ] **Step 2: Define `PlotRenderer` and the target abstraction**

```rust
use wgpu::*;
pub enum RenderTarget {
    Surface { surface: Surface<'static>, config: SurfaceConfiguration },
    // Offscreen { texture: Texture, ... }  // MVP: not implemented (MCP follow-up)
}
pub struct PlotRenderer {
    pub device: Device,
    pub queue: Queue,
    pub target: RenderTarget,
    pub format: TextureFormat,
    line_pipeline: RenderPipeline,
    point_pipeline: RenderPipeline,
    bind_group_layout: BindGroupLayout,
}
impl PlotRenderer {
    pub async fn new_for_surface(
        instance: &Instance, surface: Surface<'static>, width: u32, height: u32,
    ) -> Self { /* request adapter+device, configure surface, create_pipelines */ }
    pub fn resize(&mut self, width: u32, height: u32) { /* reconfigure surface */ }
}
```

- [ ] **Step 3: Port the draw-call build + render**

Move the per-series buffer/bind-group construction out of `CallbackTrait::prepare` into `PlotRenderer::build_draw_calls(&self, series: &[SeriesGpuData], grid: &GridGpuData, uniforms_base: PlotUniforms) -> Vec<DrawCall>` (identical logic, `device` from `self.device`). Move the `paint()` body into:

```rust
pub fn render(&self, draw_calls: &[DrawCall], clear: [f64;4]) -> Result<(), SurfaceError> {
    let RenderTarget::Surface { surface, .. } = &self.target else { return Ok(()); };
    let frame = surface.get_current_texture()?;
    let view = frame.texture.create_view(&Default::default());
    let mut enc = self.device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&RenderPassDescriptor {
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &view, resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Color{r:clear[0],g:clear[1],b:clear[2],a:clear[3]}), store: StoreOp::Store },
            })],
            ..Default::default()
        });
        for call in draw_calls {
            pass.set_pipeline(match call.pipeline_type { PipelineType::Line => &self.line_pipeline, PipelineType::Point => &self.point_pipeline });
            pass.set_bind_group(0, &call.bind_group, &[]);
            pass.draw(0..6, 0..call.instance_count);
        }
    }
    self.queue.submit([enc.finish()]);
    frame.present();
    Ok(())
}
```

- [ ] **Step 4: Delete the egui callback path** — remove the `egui_wgpu::CallbackTrait` impl, `create_plot_paint_callback`, and the `eframe`/`egui` imports from `gpu_plot.rs`.

- [ ] **Step 5: Verify** — `cargo build -p oxideplot-core` and `cargo build -p oxideplot-core --target wasm32-unknown-unknown` → both succeed. Commit: `feat(core): standalone PlotRenderer owning device/queue/surface`.

### Task 2.3: Render a hard-coded plot through the wasm wrapper

**Files:**
- Modify: `crates/oxideplot-wasm/Cargo.toml` (depend on core), `crates/oxideplot-wasm/src/lib.rs`
- Modify: `src/App.svelte`

- [ ] **Step 1: Add core dependency to the wasm crate**

`crates/oxideplot-wasm/Cargo.toml`: add `oxideplot-core = { path = "../oxideplot-core" }`.

- [ ] **Step 2: Expose an `OxidePlot` object**

Replace the spike `run_triangle` with:

```rust
#[wasm_bindgen]
pub struct OxidePlot { renderer: oxideplot_core::render::PlotRenderer, view: PlotViewState, series: Vec<SeriesGpuData> }

#[wasm_bindgen]
impl OxidePlot {
    #[wasm_bindgen(constructor)]
    pub async fn new(canvas: web_sys::HtmlCanvasElement) -> OxidePlot { /* instance + surface from canvas + PlotRenderer::new_for_surface */ }
    pub fn render(&self) { let calls = self.renderer.build_draw_calls(&self.series, &grid, uniforms); let _ = self.renderer.render(&calls, [0.1,0.1,0.12,1.0]); }
    pub fn resize(&mut self, w: u32, h: u32) { self.renderer.resize(w, h); }
}
```

For this task, hard-code two `SeriesGpuData` (a sine and a line) in `new`.

- [ ] **Step 3: Build wasm + wire Svelte** — `npm run build:wasm`; in `App.svelte` construct `await new OxidePlot(canvas)` and call `.render()`.

- [ ] **Step 4: Verify** — `npx tauri dev`. Expected: two colored series drawn by the ported wgpu engine, GPU-accelerated, in the Tauri window.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat: render hard-coded plot via wasm PlotRenderer in Tauri"`

---

## PHASE 3 — Data path (file → parse → plot)

### Task 3.1: Tauri backend commands (file dialog + read)

**Files:**
- Create: `src-tauri/src/commands.rs`; Modify: `src-tauri/src/main.rs`, `src-tauri/Cargo.toml`, root `Cargo.toml` (add `src-tauri` to workspace)

- [ ] **Step 1: Add deps** — in `src-tauri/Cargo.toml` add `rfd = "0.15"` and `tauri-plugin-dialog` (or use `rfd` directly), `serde`, `serde_json`.

- [ ] **Step 2: Implement commands**

```rust
#[tauri::command]
fn pick_file() -> Option<String> {
    rfd::FileDialog::new().add_filter("data", &["csv","xlsx","xls"]).pick_file()
        .map(|p| p.to_string_lossy().into_owned())
}
#[tauri::command]
fn read_file(path: String) -> Result<Vec<u8>, String> {
    std::fs::read(&path).map_err(|e| e.to_string())
}
```

Register both in `main.rs` `invoke_handler`.

- [ ] **Step 3: Verify** — `cargo build -p oxideplot-app` (or whatever the tauri crate is named). Commit: `feat(tauri): pick_file + read_file commands`.

### Task 3.2: WASM parse + column metadata

**Files:** Modify `crates/oxideplot-wasm/src/lib.rs`

- [ ] **Step 1: Add `load_file_bytes`**

```rust
#[wasm_bindgen]
impl OxidePlot {
    pub fn load_file_bytes(&mut self, bytes: Vec<u8>, filename: String) -> Result<JsValue, JsValue> {
        let table = oxideplot_core::data::load_from_bytes(&bytes, &filename).map_err(|e| JsValue::from_str(&e.to_string()))?;
        // return { columns: [{name, kind}], rows: N } as JsValue via serde_wasm_bindgen
    }
}
```

Add a `data::load_from_bytes(bytes, filename)` thin wrapper in core that dispatches CSV vs Excel by extension, reusing the existing loader/parser (refactor the existing path-based loader to accept bytes; keep a path-based shim for the legacy crate). Add `serde-wasm-bindgen = "0.6"` to the wasm crate.

- [ ] **Step 2: Test the core wrapper natively**

```rust
#[test]
fn load_csv_from_bytes_detects_columns() {
    let csv = b"time,temp\n0,20.0\n1,21.5\n";
    let t = load_from_bytes(csv, "x.csv").unwrap();
    assert_eq!(t.columns.len(), 2);
    assert_eq!(t.rows, 2);
}
```

Run `cargo test -p oxideplot-core` → PASS.

- [ ] **Step 3: Build wasm + commit** — `npm run build:wasm`; commit `feat: parse file bytes in wasm, return column metadata`.

### Task 3.3: Column-selection dialog → real plot

**Files:** Create `src/lib/components/ColumnDialog.svelte`, `src/lib/api.ts`, `src/lib/renderer.ts`; Modify `App.svelte`

- [ ] **Step 1: `api.ts`** wraps `invoke('pick_file')` / `invoke('read_file', {path})` from `@tauri-apps/api/core`.
- [ ] **Step 2: `renderer.ts`** wraps the wasm `OxidePlot` (init, loadFileBytes, setSeries, render, resize, pan, zoom, autoFit, viewState).
- [ ] **Step 3: Add `set_series(specs)` + `auto_fit()`** to the wasm crate: `set_series` takes JSON `[{x_col, y_col, color, draw_mode}]`, builds `SeriesGpuData` from the loaded table (downsample via `processing::lttb`), stores them, calls `auto_fit`, `render`.
- [ ] **Step 4: `ColumnDialog.svelte`** lists columns from metadata; user picks X + one/more Y; emits a series spec.
- [ ] **Step 5: Wire Open flow** in `App.svelte`: Open button → `pick_file` → `read_file` → `loadFileBytes` → dialog → `setSeries` → plot.
- [ ] **Step 6: Verify** — `npx tauri dev`, open a real sample CSV, confirm the plotted curve matches the legacy app for the same file. Commit: `feat: open file, choose columns, render real data`.

---

## PHASE 4 — Interaction + SVG overlay

### Task 4.1: Pan / zoom / auto-fit from canvas events

**Files:** Modify `src/lib/renderer.ts`, `App.svelte`; wasm `OxidePlot` (`pan`, `zoom`, `auto_fit`, `view_state`)

- [ ] **Step 1: Expose `pan(dx,dy)`, `zoom(scroll_y,x,y)`, `auto_fit()`, `view_state()->JsValue`** on `OxidePlot`, delegating to `PlotViewState` methods (Task 2.1) and re-rendering.
- [ ] **Step 2: Canvas pointer/wheel handlers** in Svelte: pointer-drag → `pan(dx, dy)`; wheel → `zoom(deltaY, x, y)`; double-click → `auto_fit()`. Use the canvas bounding rect for pixel coords.
- [ ] **Step 3: Verify** — drag pans, wheel zooms toward cursor, double-click fits. Matches legacy feel. Commit: `feat: interactive pan/zoom/auto-fit on canvas`.

### Task 4.2: Viewport-aware downsampling

**Files:** Modify wasm `OxidePlot::render` / `set_view`

- [ ] **Step 1:** On view change, re-run `processing::lttb` against the visible X-range to ~2–4k points per series before building draw calls (target bucket count from canvas pixel width).
- [ ] **Step 2: Verify** with a generated 1M-point CSV (write a small helper or reuse one): pan/zoom stays smooth (~60fps), no full-data upload per frame. Commit: `perf: viewport LTTB downsampling before draw`.

### Task 4.3: SVG axes + tick labels overlay

**Files:** Create `src/lib/overlay/Axes.svelte`; Modify `App.svelte`; wasm exposes `tick_positions()` or reuse `view_state` + JS ticks

- [ ] **Step 1: Expose tick data** — add `OxidePlot::axis_ticks() -> JsValue` returning `compute_grid_lines(min,max)` + `format_tick_value` output for both axes (reuses the pure core fns).
- [ ] **Step 2: `Axes.svelte`** absolutely-positioned `<svg>` over the canvas; draws tick marks + labels at screen positions derived from `view_state`. Time-aware X labels when the X column is datetime (reuse `data::datetime` formatting).
- [ ] **Step 3: Verify** — crisp DOM-text axes that track pan/zoom; compare against legacy axis labels. Commit: `feat: SVG axis + tick label overlay`.

### Task 4.4: Measurement cursors overlay

**Files:** Create `src/lib/overlay/Cursors.svelte`

- [ ] **Step 1: Port cursor logic** — vertical/horizontal cursor pairs as SVG lines; click-to-place (port `handle_cursor_click` intent); delta readout box (per-unit, matching legacy `draw_cursors`).
- [ ] **Step 2: Verify** — place cursors, read deltas; matches legacy behavior. Commit: `feat: measurement cursors overlay`.

---

## PHASE 5 — Chrome + export

### Task 5.1: Toolbar
- [ ] `Toolbar.svelte`: Open, Fit, draw-mode toggle (Lines/Step/Points), theme toggle. Wire to renderer/api. Verify each control. Commit.

### Task 5.2: Series list / legend
- [ ] `SeriesList.svelte`: list series with color swatch, visibility toggle, remove, and z-order reorder (drag). Add wasm `set_series_visible`, `remove_series`, `reorder_series`. Verify. Commit.

### Task 5.3: Settings panel
- [ ] `Settings.svelte`: line width, point radius, normalized multi-unit toggle, grid on/off — porting the relevant controls from `settings_dialog.rs` (skip 3D settings). Wire to wasm uniforms/state. Verify. Commit.

### Task 5.4: Export (PNG, CSV, clipboard)

**Files:** wasm `screenshot()`, Tauri `save_png`/`export_csv`, Toolbar wiring

- [ ] **Step 1:** `OxidePlot::screenshot() -> Vec<u8>` — render the current frame to the canvas, then read pixels via `canvas.toBlob`/`toDataURL` on the JS side (MVP-simple, surface readback in Rust deferred). For PNG, prefer JS `canvas.toBlob('image/png')` and pass bytes to a Tauri `save_png(bytes)` command using `rfd` save dialog.
- [ ] **Step 2:** `export_csv` Tauri command writes the current series back to CSV (reuse core to serialize).
- [ ] **Step 3:** Clipboard image via the browser Clipboard API (or Tauri clipboard plugin). Verify all three. Commit.

### Task 5.5: Persistence
- [ ] `load_prefs`/`save_prefs` Tauri commands (JSON in app config dir): recent files, theme, window size. Apply on startup. Verify across restarts. Commit.

### Task 5.6: Light/dark theme
- [ ] CSS variables + theme toggle; pass background/grid colors into wasm uniforms so the plot matches the chrome. Verify both themes. Commit.

---

## PHASE 6 — Parity pass + polish

### Task 6.1: Walk the parity checklist
- [ ] Side-by-side vs `cargo run -p oxideplot-egui-legacy` on the same sample files. Verify every item in the spec's "MVP Feature Parity Checklist": lines/step/scatter, CSV+Excel, multi-series, multi-unit normalized, timestamp X, pan/zoom/fit, cursors, LTTB smoothness, PNG/CSV/clipboard export, z-order, light/dark, SVG axes. File a fix-it list for any gap and resolve. Commit per fix.

### Task 6.2: Packaging smoke test
- [ ] `npx tauri build` produces a Windows installer; launch it, open a file, plot, export. Verify. Commit.

### Task 6.3: Remove legacy crate + portfolio assets
- [ ] Once parity is signed off: `git rm -r crates/oxideplot-egui-legacy`, drop it from workspace members, remove now-unused root deps. Update `README.md` (new build/run instructions, screenshots). Capture portfolio screenshots + a short screen-capture clip. `cargo build` + `npx tauri build` still green. Commit: `chore: remove egui legacy crate; update README + portfolio assets`.

---

## Self-Review

**Spec coverage:**
- Layer 1 `oxideplot-core` (data/processing/state/render) → Phases 1–2. ✓
- Layer 2 WASM module → Tasks 0.3, 2.3, 3.2, 4.1. ✓
- Layer 3 Tauri backend → Tasks 3.1, 5.4, 5.5; Svelte frontend → Phases 3–5. ✓
- Render-target abstraction (Surface only, offscreen designed-for) → Task 2.2. ✓
- Data flow (open→parse→plot→interact→export) → Phases 3–5. ✓
- Error handling (Result-mapped JS errors, no panics on hot path) → Tasks 2.2/2.3/3.2 (replace `unwrap()` during port). ✓
- Testing (core unit tests native + wasm build check) → Tasks 1.2–1.4, 2.1, 3.2. ✓
- Parity checklist → Task 6.1. ✓
- De-risking spike (go/no-go + WebGL2 fallback) → Phase 0. ✓
- Deferred: 3D (legacy crate, follow-up), MCP (follow-up) — correctly excluded. ✓

**Placeholder scan:** Test bodies use real assertions; function names flagged as "adjust to actual API" point at concrete modules discovered during the move (datetime/downsampling). No "TBD/implement later". Ported-code steps reference exact existing files + the exact edits (import/type swaps), rather than reproducing hundreds of unchanged lines — intentional DRY for code already in the repo.

**Type consistency:** `PlotRenderer`, `RenderTarget`, `build_draw_calls`, `render`, `DrawCall`, `PipelineType`, `SeriesGpuData`, `GridGpuData`, `PlotUniforms`, `PlotViewState::{pan,zoom,screen_to_data,fit_to_data}`, `geom::{Rect,Pos2}`, wasm `OxidePlot::{new,load_file_bytes,set_series,pan,zoom,auto_fit,view_state,axis_ticks,render,resize,screenshot}`, Tauri `pick_file/read_file/save_png/export_csv/load_prefs/save_prefs` — names used consistently across tasks.

**Known follow-the-thread items for the implementer:** exact public fn names in `data::datetime` and `processing::downsampling` are confirmed when those files are moved (Tasks 1.2/1.3); the wgpu version is confirmed via `cargo tree -i wgpu` in Task 0.3/1.1.
