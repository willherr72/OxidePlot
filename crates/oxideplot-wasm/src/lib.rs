// Task 2.3 — WASM wrapper for the standalone PlotRenderer.
// Exposes an `OxidePlot` JS class that creates a wgpu surface from an
// HtmlCanvasElement and drives `oxideplot_core::render::renderer::PlotRenderer`
// to render hard-coded plot data.
//
// The Phase 0 `run_triangle` spike is fully replaced here.

use wasm_bindgen::prelude::*;

// ─── WASM-only implementation ────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod wasm_impl {
    use super::*;
    use oxideplot_core::render::gpu_types::{DrawMode, GridGpuData, PlotUniforms, SeriesGpuData};
    use oxideplot_core::render::renderer::PlotRenderer;

    /// A GPU-accelerated 2D plot bound to an HTML canvas.
    ///
    /// Usage from JavaScript/TypeScript:
    /// ```js
    /// const plot = await new OxidePlot(canvas);
    /// plot.render();
    /// plot.resize(newW, newH);
    /// ```
    #[wasm_bindgen]
    pub struct OxidePlot {
        renderer: PlotRenderer,
        series: Vec<SeriesGpuData>,
        grid: GridGpuData,
        width: u32,
        height: u32,
    }

    #[wasm_bindgen]
    impl OxidePlot {
        /// Construct an OxidePlot attached to `canvas`.
        ///
        /// Creates a wgpu instance + WebGPU surface from the canvas element,
        /// then initialises the core PlotRenderer and pre-builds two hard-coded
        /// data series (a sine wave and a diagonal line).
        ///
        /// Call as: `const plot = await OxidePlot.create(canvas)`
        #[wasm_bindgen(js_name = "create")]
        pub async fn create(canvas: web_sys::HtmlCanvasElement) -> OxidePlot {
            console_error_panic_hook::set_once();

            let width = canvas.width();
            let height = canvas.height();

            // Create wgpu instance with WebGPU backend (same as Phase 0 spike).
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends: wgpu::Backends::BROWSER_WEBGPU,
                ..Default::default()
            });

            // Build surface from the canvas element.
            let surface_target = wgpu::SurfaceTarget::Canvas(canvas);
            let surface = instance
                .create_surface(surface_target)
                .expect("failed to create wgpu surface from canvas");

            // Initialise the core renderer (requests adapter + device, configures surface).
            let renderer =
                PlotRenderer::new_for_surface(&instance, surface, width.max(1), height.max(1))
                    .await;

            // ── Hard-coded series ──────────────────────────────────────────────
            //
            // Data coordinate space: x ∈ [0, 10], y ∈ [-1.5, 1.5].
            // view_min = [0.0, -1.5], view_max = [10.0, 1.5] in `render()`.
            // The shader maps data → NDC linearly, so any point within those
            // bounds will be visible.

            // Series 0: sine wave  — cyan/teal, drawn as connected lines.
            let sine_points: Vec<[f32; 2]> = (0..=255)
                .map(|i| {
                    let x = i as f32 / 255.0 * 10.0;
                    [x, x.sin()]
                })
                .collect();

            let sine_series = SeriesGpuData {
                points: sine_points,
                color: [0.2, 0.85, 1.0, 1.0], // bright cyan
                line_width: 2.5,
                point_radius: 4.0,
                draw_mode: DrawMode::Lines,
            };

            // Series 1: diagonal line from (0, -1) to (10, 1) — amber/orange.
            let line_points: Vec<[f32; 2]> = (0..=64)
                .map(|i| {
                    let t = i as f32 / 64.0;
                    [t * 10.0, t * 2.0 - 1.0]
                })
                .collect();

            let line_series = SeriesGpuData {
                points: line_points,
                color: [1.0, 0.6, 0.1, 1.0], // amber/orange
                line_width: 2.0,
                point_radius: 4.0,
                draw_mode: DrawMode::Lines,
            };

            // Empty grid (no segments) — skipped by build_draw_calls.
            let grid = GridGpuData {
                segments: vec![],
                color: [0.3, 0.3, 0.3, 1.0],
                line_width: 1.0,
            };

            OxidePlot {
                renderer,
                series: vec![sine_series, line_series],
                grid,
                width,
                height,
            }
        }

        /// Render one frame: build draw calls then present to the canvas.
        pub fn render(&self) {
            // View covers the full data range so all points are visible.
            let uniforms = PlotUniforms {
                view_min: [0.0, -1.5],
                view_max: [10.0, 1.5],
                resolution: [self.width as f32, self.height as f32],
                line_width: 2.0,   // overridden per-series inside build_draw_calls
                point_radius: 4.0, // overridden per-series inside build_draw_calls
                color: [0.0, 0.0, 0.0, 0.0], // overridden per-series
                _padding: [0.0; 4],
            };

            let calls = self.renderer.build_draw_calls(&self.series, &self.grid, uniforms);

            // Dark background (#1a1a1f ≈ 0.10, 0.10, 0.12)
            if let Err(e) = self.renderer.render(&calls, [0.10, 0.10, 0.12, 1.0]) {
                web_sys::console::error_1(&format!("OxidePlot render error: {e:?}").into());
            }
        }

        /// Resize the renderer surface.  Call this from a ResizeObserver.
        pub fn resize(&mut self, w: u32, h: u32) {
            self.width = w;
            self.height = h;
            self.renderer.resize(w, h);
        }
    }
}

// ─── Re-export OxidePlot at crate root on wasm32 ─────────────────────────────

#[cfg(target_arch = "wasm32")]
pub use wasm_impl::OxidePlot;

// ─── Native stub so `cargo build` of the workspace succeeds ─────────────────
//
// wasm_bindgen cannot emit anything useful on native targets; a single
// do-nothing stub keeps the `[lib] crate-type = ["cdylib", "rlib"]` happy.

#[cfg(not(target_arch = "wasm32"))]
#[wasm_bindgen]
pub fn oxideplot_native_stub() -> &'static str {
    "NATIVE_STUB"
}
