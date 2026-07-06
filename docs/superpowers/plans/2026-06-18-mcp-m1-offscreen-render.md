# MCP M1 — Headless Offscreen Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development (or inline). Checkbox steps.

**Goal:** Render draw calls to an offscreen texture headlessly and read the pixels back as a tight RGBA8 buffer, in `oxideplot-core` (native), with strong tests + a PNG dump for visual verification.

**Architecture:** Add `RenderTarget::Offscreen` + `PlotRenderer::new_offscreen(w,h)` (headless wgpu) + `render_to_rgba(draw_calls, clear)` (render pass → copy-to-buffer with 256-byte row padding → map/poll → un-pad). No `image` in core runtime deps (dev-dep only, for the test PNG).

**Tech Stack:** Rust, wgpu 24 (mirror the existing `new_for_surface`/`render` patterns exactly for adapter/device/pass setup).

## Global Constraints
- Branch `tauri-migration`. `oxideplot-core` must keep building for native AND `wasm32-unknown-unknown`. No `image`/`png` in core `[dependencies]` — dev-dep only.
- Mirror wgpu API usage from the existing `renderer.rs` (`request_adapter(...).await.expect`, `request_device(&desc, None).await.expect`, the render-pass loop) so versions match — do NOT invent wgpu-24 signatures; resolve exact calls (poll/map) against the compiler.
- One commit for the task; never commit a non-compiling tree.

## File structure
- `crates/oxideplot-core/src/render/renderer.rs` — MODIFY: `RenderTarget::Offscreen`, `new_offscreen`, `render_to_rgba`, update `render()`/`resize()` matches, `#[cfg(test)]` tests.
- `crates/oxideplot-core/Cargo.toml` — MODIFY: add `image` + `pollster` under `[dev-dependencies]` (test-only).

---

## Task 1: Offscreen render-to-RGBA + tests

**Files:** Modify `crates/oxideplot-core/src/render/renderer.rs`, `crates/oxideplot-core/Cargo.toml`.

**Interfaces — Produces (M2):**
- `RenderTarget::Offscreen { texture: wgpu::Texture, width: u32, height: u32 }`
- `pub async fn PlotRenderer::new_offscreen(width: u32, height: u32) -> Self`
- `pub fn PlotRenderer::render_to_rgba(&self, draw_calls: &[DrawCall], clear: [f64; 4]) -> Vec<u8>` (returns `width*height*4` bytes, row-major, RGBA8)

- [ ] **Step 1: Extend `RenderTarget`.** Add the `Offscreen { texture: wgpu::Texture, width: u32, height: u32 }` variant to the enum.

- [ ] **Step 2: `new_offscreen`.** Add:
```rust
/// Headless renderer that draws to an offscreen RGBA8 texture (no surface).
/// Used by the MCP server / CLI to render plots without a window.
pub async fn new_offscreen(width: u32, height: u32) -> Self {
    let (w, h) = (width.max(1), height.max(1));
    let instance = Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: None,
        })
        .await
        .expect("no wgpu adapter found for offscreen rendering");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default(), None)
        .await
        .expect("failed to request wgpu device");

    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen_plot_target"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });

    let (line_pipeline, point_pipeline, bind_group_layout) = create_pipelines(&device, format);

    Self {
        device,
        queue,
        target: RenderTarget::Offscreen { texture, width: w, height: h },
        format,
        line_pipeline,
        point_pipeline,
        bind_group_layout,
    }
}
```
(Import `wgpu::Instance` is already in scope. If `Instance::default()` isn't the right constructor for wgpu 24, mirror how `oxideplot-wasm` builds its `Instance` — use the same call.)

- [ ] **Step 3: `render_to_rgba`.** Add (render pass mirrors `render()`; then read-back with padding):
```rust
/// Render `draw_calls` to the offscreen target (clearing to `clear` RGBA) and
/// return the pixels as a tight `width*height*4` RGBA8 buffer (row-major).
/// Panics if called on a Surface-target renderer.
pub fn render_to_rgba(&self, draw_calls: &[DrawCall], clear: [f64; 4]) -> Vec<u8> {
    let (texture, width, height) = match &self.target {
        RenderTarget::Offscreen { texture, width, height } => (texture, *width, *height),
        RenderTarget::Surface { .. } => panic!("render_to_rgba called on a surface renderer"),
    };
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    // Row-padding: bytes_per_row must be a multiple of 256.
    let unpadded_bpr = width * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT; // 256
    let padded_bpr = ((unpadded_bpr + align - 1) / align) * align;
    let buffer_size = (padded_bpr * height) as u64;

    let out_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("offscreen_readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("offscreen_encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("offscreen_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear[0], g: clear[1], b: clear[2], a: clear[3],
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        for call in draw_calls {
            match call.pipeline_type {
                PipelineType::Line => pass.set_pipeline(&self.line_pipeline),
                PipelineType::Point => pass.set_pipeline(&self.point_pipeline),
            }
            pass.set_bind_group(0, &call.bind_group, &[]);
            pass.draw(0..6, 0..call.instance_count);
        }
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { // (wgpu 24 name; older: ImageCopyTexture) — use what compiles
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &out_buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bpr),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    self.queue.submit(std::iter::once(encoder.finish()));

    // Map + wait.
    let slice = out_buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    self.device.poll(wgpu::Maintain::Wait); // wgpu 24: adjust to PollType::Wait if required

    let mapped = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((unpadded_bpr * height) as usize);
    for row in 0..height {
        let start = (row * padded_bpr) as usize;
        pixels.extend_from_slice(&mapped[start..start + unpadded_bpr as usize]);
    }
    drop(mapped);
    out_buf.unmap();
    pixels
}
```
**Note:** the exact wgpu-24 type names for `copy_texture_to_buffer` args (`TexelCopyTextureInfo`/`ImageCopyTexture`, `TexelCopyBufferInfo`/`ImageCopyBuffer`) and `poll` (`Maintain::Wait`/`PollType::Wait`) vary — use whatever the crate exposes (check `oxideplot-wasm`/compiler). The LOGIC (padding math, per-row un-pad) is the contract; keep it exact.

- [ ] **Step 4: Update the now-refutable matches.**
- `render()`: change the irrefutable `let RenderTarget::Surface { surface, .. } = &self.target;` to a `match`; `Surface` arm = today's body; `Offscreen` arm = `return Ok(())` (documented: a surface present is never issued on an offscreen renderer; use `render_to_rgba` instead).
- `resize()`: add an `Offscreen { .. }` arm = no-op for M1 (doc: the MCP server builds a fresh renderer per output size).

- [ ] **Step 5: dev-deps.** In `crates/oxideplot-core/Cargo.toml` add:
```toml
[dev-dependencies]
pollster = "0.4"
image = { version = "0.25", default-features = false, features = ["png"] }
```
(match versions to what resolves; `pollster` drives the async `new_offscreen` in tests.)

- [ ] **Step 6: Write tests** (`#[cfg(test)] mod offscreen_tests` in `renderer.rs`). Build a minimal draw-call set via `build_draw_calls` with one 2-point line series over a known viewport (construct `SeriesGpuData`/`GridGpuData`/`PlotUniforms` directly — see `gpu_types` + how `oxideplot-wasm` builds them). Then:
```rust
#[test]
fn offscreen_dims_and_nonblank() {
    let r = pollster::block_on(PlotRenderer::new_offscreen(300, 200));
    let calls = /* build_draw_calls for a simple line spanning the viewport */;
    let clear = [0.05, 0.06, 0.08, 1.0];
    let buf = r.render_to_rgba(&calls, clear);
    assert_eq!(buf.len(), 300 * 200 * 4);
    // Something was drawn: at least one pixel differs from the clear color.
    let clr = [(clear[0]*255.0) as u8, (clear[1]*255.0) as u8, (clear[2]*255.0) as u8, 255];
    let drew = buf.chunks_exact(4).any(|px| px != clr);
    assert!(drew, "offscreen render produced a blank (clear-only) image");
}

#[test]
fn offscreen_empty_is_solid_clear() {
    let r = pollster::block_on(PlotRenderer::new_offscreen(300, 200)); // 300*4=1200, NOT 256-aligned → exercises padding
    let clear = [0.1, 0.2, 0.3, 1.0];
    let buf = r.render_to_rgba(&[], clear);
    assert_eq!(buf.len(), 300 * 200 * 4);
    let clr = [(0.1*255.0).round() as u8, (0.2*255.0).round() as u8, (0.3*255.0).round() as u8, 255];
    // Every pixel ~= clear (allow ±1 for sRGB rounding). Also checks the LAST row
    // (padding un-pad correctness).
    for px in buf.chunks_exact(4) {
        for c in 0..3 { assert!((px[c] as i16 - clr[c] as i16).abs() <= 2, "pixel {px:?} != clear {clr:?}"); }
    }
}

#[test]
fn offscreen_png_dump() {
    // Visual artifact for eyeballing — not a hard assertion beyond a successful write.
    let r = pollster::block_on(PlotRenderer::new_offscreen(640, 400));
    let calls = /* same simple plot as case 1, scaled */;
    let buf = r.render_to_rgba(&calls, [0.05, 0.06, 0.08, 1.0]);
    let img = image::RgbaImage::from_raw(640, 400, buf).expect("buffer sized for image");
    let _ = std::fs::create_dir_all("target");
    img.save("target/oxideplot-offscreen-m1.png").expect("write PNG");
}
```
(If constructing `build_draw_calls` inputs by hand is heavy, add a tiny test helper that mirrors how `oxideplot-wasm::rebuild_visible` builds a `SeriesGpuData` from xs/ys — keep it minimal: 2–5 points is enough.)

- [ ] **Step 7: Verify.**
- `cargo test -p oxideplot-core offscreen` → 3 tests PASS.
- `cargo test -p oxideplot-core` → whole suite still green.
- `cargo build -p oxideplot-core --target wasm32-unknown-unknown` → succeeds (no `image` leaked into non-test build; the offscreen code compiles for wasm too, or is behind `#[cfg(not(target_arch="wasm32"))]` if any native-only API is used — prefer keeping it un-gated if it compiles for wasm).
- `npm run build:wasm` → succeeds.
- Open `target/oxideplot-offscreen-m1.png` and confirm it shows a plotted line on the dark background (human check).

- [ ] **Step 8: Commit.** `feat(core): headless offscreen render-to-RGBA (MCP M1 foundation) + tests`

## Self-Review
- Spec coverage: Offscreen target + new_offscreen + render_to_rgba (Steps 1–3) ✓; match updates (4) ✓; RGBA-in-core / no image runtime dep (5) ✓; padding + non-blank + empty + last-row tests (6) ✓; PNG dump for visual (6) ✓; wasm-clean (7) ✓.
- Placeholders: the two `/* build_draw_calls ... */` spots are the one thing the implementer fleshes out from `gpu_types` — flagged with guidance; everything else is concrete. The wgpu-24 type-name/poll caveats are called out as "use what compiles," with the invariant logic given exactly.
- Type consistency: `render_to_rgba(&[DrawCall], [f64;4]) -> Vec<u8>` and `new_offscreen(u32,u32)` match the spec + M2's consumption; `RenderTarget::Offscreen` fields consistent across new_offscreen/render_to_rgba/resize.
