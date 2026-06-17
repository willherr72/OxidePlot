// Task 2.3 — WASM wrapper for the standalone PlotRenderer.
// Exposes an `OxidePlot` JS class that creates a wgpu surface from an
// HtmlCanvasElement and drives `oxideplot_core::render::renderer::PlotRenderer`
// to render hard-coded plot data.
//
// Task 3.2 — adds `load_file_bytes` which parses CSV/Excel bytes via the core
// loader and returns column metadata as a JS object.  The parsed `LoadedData`
// is stored in `self.loaded` for Task 3.3 (series construction).
//
// Task 3.3 — adds `set_series(specs_json)` and `auto_fit()`.  `set_series`
// replaces the hard-coded demo with GPU series built from the loaded data.
// `render()` now uses the stored view state instead of hard-coded bounds.
//
// Task 4.1 — adopts `PlotViewState` for the view, exposes `pan`, `zoom`,
// and `view_state` for interactive canvas-driven interaction.

use wasm_bindgen::prelude::*;

// ─── WASM-only implementation ────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod wasm_impl {
    use super::*;
    use serde::{Deserialize, Serialize};
    use oxideplot_core::render::gpu_types::{DrawMode, GridGpuData, PlotUniforms, SeriesGpuData};
    use oxideplot_core::render::renderer::PlotRenderer;
    use oxideplot_core::data::loader::{LoadedData, FileMeta, load_from_bytes, column_to_f64, column_to_timestamps};
    use oxideplot_core::processing::downsampling::lttb_downsample;
    use oxideplot_core::state::plot_view::PlotViewState;
    use oxideplot_core::geom::{Pos2, Rect};

    /// Maximum points per series before LTTB downsampling kicks in.
    const MAX_SERIES_POINTS: usize = 2000;

    /// JSON spec for one series passed in from JS via `set_series`.
    #[derive(Deserialize)]
    struct SeriesSpec {
        x_col: usize,
        y_col: usize,
        color: [f32; 4],
        draw_mode: String,
    }

    /// Serialisable snapshot of the current view bounds, returned by `view_state`.
    #[derive(Serialize)]
    struct ViewSnapshot {
        x_min: f64,
        x_max: f64,
        y_min: f64,
        y_max: f64,
    }

    /// A GPU-accelerated 2D plot bound to an HTML canvas.
    ///
    /// Usage from JavaScript/TypeScript:
    /// ```js
    /// const plot = await OxidePlot.create(canvas);
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
        /// Parsed data stored here for set_series / series building.
        loaded: Option<LoadedData>,
        /// Current view state (bounds + pan/zoom logic).
        view: PlotViewState,
    }

    #[wasm_bindgen]
    impl OxidePlot {
        /// Construct an OxidePlot attached to `canvas`.
        ///
        /// Creates a wgpu instance + WebGPU surface from the canvas element,
        /// then initialises the core PlotRenderer.  No data is plotted until
        /// `set_series` is called after `load_file_bytes`.
        ///
        /// Call as: `const plot = await OxidePlot.create(canvas)`
        #[wasm_bindgen(js_name = "create")]
        pub async fn create(canvas: web_sys::HtmlCanvasElement) -> OxidePlot {
            console_error_panic_hook::set_once();

            let width = canvas.width();
            let height = canvas.height();

            // Create wgpu instance with WebGPU backend.
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends: wgpu::Backends::BROWSER_WEBGPU,
                ..Default::default()
            });

            // Build surface from the canvas element.
            let surface_target = wgpu::SurfaceTarget::Canvas(canvas);
            let surface = instance
                .create_surface(surface_target)
                .expect("failed to create wgpu surface from canvas");

            // Initialise the core renderer.
            let renderer =
                PlotRenderer::new_for_surface(&instance, surface, width.max(1), height.max(1))
                    .await;

            // Empty grid (no segments).
            let grid = GridGpuData {
                segments: vec![],
                color: [0.3, 0.3, 0.3, 1.0],
                line_width: 1.0,
            };

            OxidePlot {
                renderer,
                series: vec![],
                grid,
                width,
                height,
                loaded: None,
                view: PlotViewState::default(),
            }
        }

        /// Parse file bytes and return column metadata as a JS object.
        ///
        /// `bytes`    — raw file contents (passed from the Tauri `read_file` command).
        /// `filename` — original filename (used for extension-based dispatch: .csv / .xlsx / .xls).
        ///
        /// Returns `{ columns: [{ name: string, kind: string }], rows: number }` on success,
        /// or a JS string error on failure.
        ///
        /// The parsed data is stored internally in `self.loaded` so that
        /// `set_series` can build GPU series from the chosen column indices.
        #[wasm_bindgen]
        pub fn load_file_bytes(
            &mut self,
            bytes: Vec<u8>,
            filename: String,
        ) -> Result<JsValue, JsValue> {
            let data = load_from_bytes(&bytes, &filename)
                .map_err(|e| JsValue::from_str(&e))?;

            let meta = FileMeta::from_loaded(&data);

            // Store parsed data for series construction.
            self.loaded = Some(data);
            // Clear any previous series until the user picks new columns.
            self.series.clear();

            serde_wasm_bindgen::to_value(&meta)
                .map_err(|e| JsValue::from_str(&e.to_string()))
        }

        /// Build GPU series from column specs and render.
        ///
        /// `specs_json` is a JSON array of objects:
        /// ```json
        /// [{ "x_col": 0, "y_col": 1, "color": [r, g, b, a], "draw_mode": "lines" }]
        /// ```
        /// `draw_mode` is one of `"lines"`, `"step"`, or `"points"`.
        ///
        /// After building all series, `auto_fit` is called (which renders).
        #[wasm_bindgen]
        pub fn set_series(&mut self, specs_json: String) -> Result<(), JsValue> {
            let data = self
                .loaded
                .as_ref()
                .ok_or_else(|| JsValue::from_str("No file loaded — call load_file_bytes first"))?;

            let specs: Vec<SeriesSpec> = serde_json::from_str(&specs_json)
                .map_err(|e| JsValue::from_str(&format!("Invalid series spec JSON: {e}")))?;

            let num_cols = data.columns.len();
            let mut new_series: Vec<SeriesGpuData> = Vec::with_capacity(specs.len());

            for spec in &specs {
                if spec.x_col >= num_cols || spec.y_col >= num_cols {
                    return Err(JsValue::from_str(&format!(
                        "Column index out of range: x_col={}, y_col={}, num_cols={}",
                        spec.x_col, spec.y_col, num_cols
                    )));
                }

                let x_col_data = &data.column_data[spec.x_col];
                let y_col_data = &data.column_data[spec.y_col];

                // Convert X: try datetime first, fall back to f64.
                let x_vals: Vec<f64> = if let Some((ts, _)) = column_to_timestamps(x_col_data) {
                    ts
                } else {
                    let (vals, _) = column_to_f64(x_col_data);
                    vals
                };

                // Convert Y: always f64.
                let (y_vals, _) = column_to_f64(y_col_data);

                // Zip and filter: keep only finite pairs.
                let (mut xs, mut ys): (Vec<f64>, Vec<f64>) = x_vals
                    .iter()
                    .zip(y_vals.iter())
                    .filter(|(&x, &y)| x.is_finite() && y.is_finite())
                    .map(|(&x, &y)| (x, y))
                    .unzip();

                if xs.is_empty() {
                    continue;
                }

                // Downsample if the series is large.
                if xs.len() > MAX_SERIES_POINTS {
                    let (ds_x, ds_y) = lttb_downsample(&xs, &ys, MAX_SERIES_POINTS);
                    xs = ds_x;
                    ys = ds_y;
                }

                // Build GPU points as [f32; 2] pairs.
                let points: Vec<[f32; 2]> = xs
                    .iter()
                    .zip(ys.iter())
                    .map(|(&x, &y)| [x as f32, y as f32])
                    .collect();

                let draw_mode = match spec.draw_mode.as_str() {
                    "step" => DrawMode::Step,
                    "points" => DrawMode::Points,
                    _ => DrawMode::Lines,
                };

                new_series.push(SeriesGpuData {
                    points,
                    color: spec.color,
                    line_width: 2.0,
                    point_radius: 3.0,
                    draw_mode,
                });
            }

            self.series = new_series;
            // auto_fit now calls render() internally.
            self.auto_fit();
            Ok(())
        }

        /// Auto-fit the view bounds to encompass all stored series with 5% padding,
        /// then re-render.
        #[wasm_bindgen]
        pub fn auto_fit(&mut self) {
            if self.series.is_empty() {
                return;
            }

            let mut x_min = f64::INFINITY;
            let mut x_max = f64::NEG_INFINITY;
            let mut y_min = f64::INFINITY;
            let mut y_max = f64::NEG_INFINITY;

            for s in &self.series {
                for &[x, y] in &s.points {
                    let xd = x as f64;
                    let yd = y as f64;
                    if xd.is_finite() {
                        x_min = x_min.min(xd);
                        x_max = x_max.max(xd);
                    }
                    if yd.is_finite() {
                        y_min = y_min.min(yd);
                        y_max = y_max.max(yd);
                    }
                }
            }

            if !x_min.is_finite() || !x_max.is_finite() || !y_min.is_finite() || !y_max.is_finite() {
                return;
            }

            let x_pad = ((x_max - x_min) * 0.05).max(1e-9);
            let y_pad = ((y_max - y_min) * 0.05).max(1e-9);

            self.view.x_min = x_min - x_pad;
            self.view.x_max = x_max + x_pad;
            self.view.y_min = y_min - y_pad;
            self.view.y_max = y_max + y_pad;
            self.view.auto_fit = false;
            self.view.initialized = true;

            self.render();
        }

        /// Pan the view by a pixel drag delta (backing-store pixels) and re-render.
        #[wasm_bindgen]
        pub fn pan(&mut self, dx_px: f32, dy_px: f32) {
            let rect = self.canvas_rect();
            self.view.pan(dx_px, dy_px, rect);
            self.render();
        }

        /// Zoom around a screen-space anchor (backing-store pixels) and re-render.
        ///
        /// `scroll_y` follows the sign convention: positive = zoom in (scroll up).
        /// Pass `-event.deltaY` from the browser `wheel` event.
        #[wasm_bindgen]
        pub fn zoom(&mut self, scroll_y: f32, anchor_x: f32, anchor_y: f32) {
            let anchor = Pos2 { x: anchor_x, y: anchor_y };
            let rect = self.canvas_rect();
            self.view.zoom(scroll_y, anchor, rect);
            self.render();
        }

        /// Return current view bounds as a JS object `{ x_min, x_max, y_min, y_max }`.
        #[wasm_bindgen]
        pub fn view_state(&self) -> JsValue {
            let snapshot = ViewSnapshot {
                x_min: self.view.x_min,
                x_max: self.view.x_max,
                y_min: self.view.y_min,
                y_max: self.view.y_max,
            };
            serde_wasm_bindgen::to_value(&snapshot).unwrap_or(JsValue::NULL)
        }

        /// Render one frame: build draw calls from stored series, then present.
        pub fn render(&self) {
            // If no series yet, draw a blank dark frame.
            if self.series.is_empty() {
                // Attempt a blank render — just clear to background.
                let uniforms = PlotUniforms {
                    view_min: [0.0, 0.0],
                    view_max: [1.0, 1.0],
                    resolution: [self.width as f32, self.height as f32],
                    line_width: 2.0,
                    point_radius: 4.0,
                    color: [0.0, 0.0, 0.0, 0.0],
                    _padding: [0.0; 4],
                };
                let calls = self.renderer.build_draw_calls(&[], &self.grid, uniforms);
                if let Err(e) = self.renderer.render(&calls, [0.10, 0.10, 0.12, 1.0]) {
                    web_sys::console::error_1(&format!("OxidePlot render error: {e:?}").into());
                }
                return;
            }

            let uniforms = PlotUniforms {
                view_min: [self.view.x_min as f32, self.view.y_min as f32],
                view_max: [self.view.x_max as f32, self.view.y_max as f32],
                resolution: [self.width as f32, self.height as f32],
                line_width: 2.0,
                point_radius: 3.0,
                color: [0.0, 0.0, 0.0, 0.0], // overridden per-series inside build_draw_calls
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

        // ── Private helpers ───────────────────────────────────────────────────

        /// Build a canvas-sized Rect (backing-store pixels, origin at top-left).
        fn canvas_rect(&self) -> Rect {
            Rect {
                left: 0.0,
                top: 0.0,
                width: self.width as f32,
                height: self.height as f32,
            }
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
