# MCP Foundation M1 — Headless Offscreen Rendering Design

**Date:** 2026-06-18
**Status:** Approved design → plan next
**Part of:** OxidePlot MCP server (M1 of M1→M2→M3). See the MCP design discussion.

## Context

The MCP server must render OxidePlot's GPU plots **without a window** so Claude
can look at the result. Today `PlotRenderer` only draws to a `RenderTarget::Surface`
(a canvas/window) — `renderer.rs` even carries the marker
`// Offscreen { texture: Texture, ... } // MVP: not implemented (MCP follow-up)`.

M1 builds that offscreen path. It is the **foundation** everything else (MCP
`render_graph`, the future CLI `oxideplot render`) stands on, so correctness and
tests come first.

## Goal

Headlessly render a set of draw calls to an offscreen texture and read the pixels
back as a tight **RGBA8** buffer, entirely in `oxideplot-core` (native), with no
new runtime dependencies and a strong, visually-verifiable test.

## Scope decision: RGBA in core, PNG downstream

Core exposes raw RGBA pixels — NOT PNG. Rationale:
- `oxideplot-core` also compiles to `wasm32`; adding an image-encoder there
  risks the wasm build. RGBA read-back uses only `wgpu`, which is already a dep.
- Raw pixels are the fundamental primitive; PNG/JPEG/base64 encoding is a
  one-liner in the consuming native crate (the MCP server / CLI), which can
  freely depend on the `image` crate.
- The test can still encode a PNG via an `image` **dev-dependency** (test-only)
  to dump a file for eyeballing, without polluting core's runtime deps.

## Architecture

### `RenderTarget::Offscreen`
Extend the enum (`crates/oxideplot-core/src/render/renderer.rs`):
```rust
pub enum RenderTarget {
    Surface { surface: Surface<'static>, config: SurfaceConfiguration },
    Offscreen { texture: wgpu::Texture, width: u32, height: u32 },
}
```
(`texture` created with usage `RENDER_ATTACHMENT | COPY_SRC`.)

### `PlotRenderer::new_offscreen`
```rust
pub async fn new_offscreen(width: u32, height: u32) -> Self
```
Headless init mirroring `new_for_surface`, but:
- `Instance::default()`, `request_adapter` with `compatible_surface: None`
  (`force_fallback_adapter: false`); `request_device` as today.
- **Format:** `wgpu::TextureFormat::Rgba8UnormSrgb` (PNG is sRGB; RGBA channel
  order matches read-back → no B/R swap). Build pipelines with the existing
  `create_pipelines(&device, format)`.
- Create the offscreen `texture` (size `width.max(1) x height.max(1)`, the chosen
  format, `RENDER_ATTACHMENT | COPY_SRC`).
- Store `target: RenderTarget::Offscreen { texture, width, height }`.

### `PlotRenderer::render_to_rgba`
```rust
pub fn render_to_rgba(&self, draw_calls: &[DrawCall], clear: [f64; 4]) -> Vec<u8>
```
1. Create a view of the offscreen texture; run the **same** render pass +
   pipeline/bind-group/`draw(0..6, 0..n)` loop as `render()`, clearing to `clear`.
2. `copy_texture_to_buffer` into a read-back buffer. **Row padding:** wgpu
   requires `bytes_per_row` aligned to `COPY_BYTES_PER_ROW_ALIGNMENT` (256).
   `unpadded = width*4`; `padded = ceil(unpadded/256)*256`. Allocate
   `padded * height` bytes (`MAP_READ | COPY_DST`).
3. Submit. `buffer.slice(..).map_async(Read, cb)`; `device.poll(wgpu::Maintain::Wait)`.
4. Copy the mapped bytes into a tight `Vec<u8>` of `width*height*4`, **dropping the
   per-row padding** (`for each row: copy padded[row*padded .. +unpadded]`).
   Unmap. Return.

Returned buffer: `width*height*4` bytes, row-major, top-to-bottom, RGBA8.

### Update the existing matches (they become refutable)
- `render()` currently does `let RenderTarget::Surface { .. } = &self.target;`
  (irrefutable). Change to a `match`: `Surface` arm as today; `Offscreen` arm →
  return `Ok(())` is wrong (present makes no sense) — instead have `render()`
  `match` and for `Offscreen` do nothing / `unreachable`-style no-op (a surface
  renderer is never Offscreen and vice-versa; document the invariant).
- `resize()`'s `match &mut self.target` gains an `Offscreen` arm: recreate the
  texture at the new size (or no-op for M1 — the MCP server constructs a fresh
  renderer per size, so a no-op with a doc note is acceptable for M1).

No `image`/`png` crate in `[dependencies]`. `image` may be added under
`[dev-dependencies]` for the test's PNG dump only.

## Color / correctness notes (the gotchas)
- **Row padding** (256-byte alignment) is the classic read-back bug — the test
  MUST use a width whose `width*4` is NOT already 256-aligned (e.g. 400 → 1600,
  aligned; use 401 or 300 → 1200 not aligned) so the un-padding logic is exercised.
- **sRGB:** with an `*Srgb` target the pipeline writes are sRGB-encoded, matching
  PNG expectations. Eyeball the dumped PNG against the on-screen app for parity;
  if the clear color looks off, reconcile the clear-color space (the app feeds
  sRGB-fraction clear values).
- **Alpha:** clear alpha = 1.0; output is opaque. Fine for PNG.

## Testing (foundation strength)
Native unit tests in `renderer.rs` (or a `render/offscreen_tests.rs`), using
`pollster` to drive the async constructor (already a workspace-available executor
pattern; if absent, use `pollster` dev-dep):
1. **Dimensions + non-blank:** `new_offscreen(300, 200)`, build draw calls for a
   simple 2-point line series over a known viewport, `render_to_rgba(clear=dark)`.
   Assert: `buf.len() == 300*200*4`; and **not every pixel equals the clear color**
   (something was drawn) — scan for ≥1 pixel differing from `clear`.
2. **Empty draw calls → solid clear:** `render_to_rgba(&[], clear)` → every pixel
   equals the clamped clear color (validates the clear path + read-back with no
   geometry).
3. **Padding correctness:** use a width like `300` (1200 bytes/row, not 256-aligned)
   so the un-padding path is covered; assert the last row's pixels are valid (not
   zeroed/garbage), e.g. the clear color for an empty render.
4. **PNG dump (visual):** with the `image` dev-dep, encode the case-1 buffer to
   `target/oxideplot-offscreen-m1.png` for manual eyeballing (assert the write
   succeeds; the human confirms it looks like a plot).

## Non-goals (M1)
PNG/base64 encoding in core, the MCP tools/crate, anti-aliasing/MSAA, DPI scaling,
axis/label rendering (labels are SVG overlays in the app — the MCP server will
composite or draw them separately in a later milestone; M1 renders the GPU plot
layer only), offscreen `resize` reuse.

## Next
M2: the `oxideplot-mcp` crate — tools `load_csv`, `describe_data`, `query_data`
(reusing Phase 7 `data::table`), `create_graph`, `render_graph` (RGBA → PNG →
image content), over stdio via `rmcp`.
