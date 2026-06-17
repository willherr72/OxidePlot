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
//
// Task 4.2 — viewport-aware downsampling.  Full source data is stored in
// `sources: Vec<SourceSeries>` and `rebuild_visible()` re-runs LTTB over the
// visible X-range on every pan/zoom/auto_fit, giving ~1 point per pixel.
//
// Task 4.3 — adds `x_is_time` field and `axis_ticks()` method for SVG tick
// label overlay.  `set_series` now detects datetime X columns.

use wasm_bindgen::prelude::*;

// ─── WASM-only implementation ────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod wasm_impl {
    use super::*;
    use serde::{Deserialize, Serialize};
    use oxideplot_core::render::gpu_types::{DrawMode, GridGpuData, PlotUniforms, SeriesGpuData};
    use oxideplot_core::render::renderer::PlotRenderer;
    use oxideplot_core::data::loader::{LoadedData, FileMeta, load_from_bytes, column_to_f64, column_to_timestamps};
    use oxideplot_core::processing::downsampling::downsample_for_view;
    use oxideplot_core::state::plot_view::PlotViewState;
    use oxideplot_core::geom::{Pos2, Rect};
    use oxideplot_core::render::axis::{compute_grid_lines, format_tick_value};
    use oxideplot_core::data::datetime::format_timestamp;

    /// Minimum target point count when width is very small.
    const MIN_TARGET_POINTS: usize = 800;

    /// Full source data for one series, stored before any downsampling.
    /// xs must be in ascending order (standard time-series assumption).
    struct SourceSeries {
        name: String,
        visible: bool,
        xs: Vec<f64>,
        ys: Vec<f64>,
        color: [f32; 4],
        draw_mode: DrawMode,
    }

    /// Serialisable info about one series, returned by `series_info`.
    #[derive(Serialize)]
    struct SeriesInfo {
        name: String,
        color: [f32; 4],
        visible: bool,
    }

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

    #[derive(Serialize)]
    struct TickEntry {
        value: f64,
        label: String,
        major: bool,
    }

    #[derive(Serialize)]
    struct AxisTicks {
        x: Vec<TickEntry>,
        y: Vec<TickEntry>,
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
        /// Full source data per series (stored un-downsampled).
        sources: Vec<SourceSeries>,
        /// Viewport-downsampled GPU series, rebuilt by rebuild_visible().
        series: Vec<SeriesGpuData>,
        grid: GridGpuData,
        width: u32,
        height: u32,
        /// Parsed data stored here for set_series / series building.
        loaded: Option<LoadedData>,
        /// Current view state (bounds + pan/zoom logic).
        view: PlotViewState,
        /// True when the X axis contains datetime (Unix timestamp) data.
        x_is_time: bool,
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
                sources: vec![],
                series: vec![],
                grid,
                width,
                height,
                loaded: None,
                view: PlotViewState::default(),
                x_is_time: false,
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
            self.sources.clear();
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
            let mut new_sources: Vec<SourceSeries> = Vec::with_capacity(specs.len());
            let mut x_is_time_any = false;

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
                    x_is_time_any = true;
                    ts
                } else {
                    let (vals, _) = column_to_f64(x_col_data);
                    vals
                };

                // Convert Y: always f64.
                let (y_vals, _) = column_to_f64(y_col_data);

                // Zip and filter: keep only finite pairs.
                let (xs, ys): (Vec<f64>, Vec<f64>) = x_vals
                    .iter()
                    .zip(y_vals.iter())
                    .filter(|(&x, &y)| x.is_finite() && y.is_finite())
                    .map(|(&x, &y)| (x, y))
                    .unzip();

                if xs.is_empty() {
                    continue;
                }

                let draw_mode = match spec.draw_mode.as_str() {
                    "step" => DrawMode::Step,
                    "points" => DrawMode::Points,
                    _ => DrawMode::Lines,
                };

                // Store FULL source data — no downsampling here.
                // rebuild_visible() will LTTB-downsample to the visible range.
                let name = data.columns[spec.y_col].clone();
                new_sources.push(SourceSeries {
                    name,
                    visible: true,
                    xs,
                    ys,
                    color: spec.color,
                    draw_mode,
                });
            }

            self.sources = new_sources;
            self.x_is_time = x_is_time_any;
            // auto_fit computes bounds from source data, calls rebuild_visible + render.
            self.auto_fit();
            Ok(())
        }

        /// Auto-fit the view bounds to encompass all stored series with 5% padding,
        /// then rebuild visible downsampled series and re-render.
        #[wasm_bindgen]
        pub fn auto_fit(&mut self) {
            if self.sources.is_empty() {
                return;
            }

            let mut x_min = f64::INFINITY;
            let mut x_max = f64::NEG_INFINITY;
            let mut y_min = f64::INFINITY;
            let mut y_max = f64::NEG_INFINITY;

            for s in &self.sources {
                for (&x, &y) in s.xs.iter().zip(s.ys.iter()) {
                    if x.is_finite() {
                        x_min = x_min.min(x);
                        x_max = x_max.max(x);
                    }
                    if y.is_finite() {
                        y_min = y_min.min(y);
                        y_max = y_max.max(y);
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

            self.rebuild_visible();
            self.render();
        }

        /// Pan the view by a pixel drag delta (backing-store pixels) and re-render.
        #[wasm_bindgen]
        pub fn pan(&mut self, dx_px: f32, dy_px: f32) {
            let rect = self.canvas_rect();
            self.view.pan(dx_px, dy_px, rect);
            self.rebuild_visible();
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
            self.rebuild_visible();
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

        /// Return tick data for both axes as a JS object:
        /// `{ x: [{value, label, major}], y: [{value, label, major}] }`
        /// X labels use datetime format when x_is_time is true.
        #[wasm_bindgen]
        pub fn axis_ticks(&self) -> JsValue {
            let x_lines = compute_grid_lines(self.view.x_min, self.view.x_max);
            let y_lines = compute_grid_lines(self.view.y_min, self.view.y_max);

            let x_span = self.view.x_max - self.view.x_min;

            let x_ticks: Vec<TickEntry> = x_lines
                .into_iter()
                .map(|(val, major)| {
                    let label = if self.x_is_time {
                        // Shorten label based on visible span:
                        // < 1 day (86400s): show only time portion
                        // >= 1 day: show full datetime
                        if x_span < 86400.0 {
                            let full = format_timestamp(val);
                            // Extract HH:MM:SS (chars 11..19)
                            if full.len() >= 19 {
                                full[11..19].to_string()
                            } else {
                                full
                            }
                        } else {
                            format_timestamp(val)
                        }
                    } else {
                        format_tick_value(val)
                    };
                    TickEntry { value: val, label, major }
                })
                .collect();

            let y_ticks: Vec<TickEntry> = y_lines
                .into_iter()
                .map(|(val, major)| TickEntry {
                    value: val,
                    label: format_tick_value(val),
                    major,
                })
                .collect();

            let ticks = AxisTicks { x: x_ticks, y: y_ticks };
            serde_wasm_bindgen::to_value(&ticks).unwrap_or(JsValue::NULL)
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

        /// Set the draw mode for all existing series and re-render.
        ///
        /// `mode` is one of `"lines"`, `"step"`, or `"points"`.
        /// Unrecognised values fall back to `"lines"`.
        ///
        /// After updating every `SourceSeries`, `rebuild_visible()` is called
        /// (re-applying viewport LTTB at the current zoom level) and then `render()`.
        #[wasm_bindgen]
        pub fn set_draw_mode(&mut self, mode: String) {
            let draw_mode = match mode.as_str() {
                "step" => DrawMode::Step,
                "points" => DrawMode::Points,
                _ => DrawMode::Lines,
            };
            for src in &mut self.sources {
                src.draw_mode = draw_mode;
            }
            self.rebuild_visible();
            self.render();
        }

        /// Resize the renderer surface.  Call this from a ResizeObserver.
        pub fn resize(&mut self, w: u32, h: u32) {
            self.width = w;
            self.height = h;
            self.renderer.resize(w, h);
            self.rebuild_visible();
            self.render();
        }

        // ── Series management ─────────────────────────────────────────────────

        /// Return an array of `{ name, color, visible }` objects (one per source
        /// series, in render order) so the frontend can build a series legend.
        #[wasm_bindgen]
        pub fn series_info(&self) -> JsValue {
            let info: Vec<SeriesInfo> = self
                .sources
                .iter()
                .map(|src| SeriesInfo {
                    name: src.name.clone(),
                    color: src.color,
                    visible: src.visible,
                })
                .collect();
            serde_wasm_bindgen::to_value(&info).unwrap_or(JsValue::NULL)
        }

        /// Toggle the visibility of the series at `index` and re-render.
        #[wasm_bindgen]
        pub fn set_series_visible(&mut self, index: usize, visible: bool) {
            if index >= self.sources.len() {
                return;
            }
            self.sources[index].visible = visible;
            self.rebuild_visible();
            self.render();
        }

        /// Remove the series at `index` and re-render.
        #[wasm_bindgen]
        pub fn remove_series(&mut self, index: usize) {
            if index >= self.sources.len() {
                return;
            }
            self.sources.remove(index);
            self.rebuild_visible();
            self.render();
        }

        /// Move the series at `from` to position `to`, shifting others, and re-render.
        /// This changes z-order: later indices render on top.
        #[wasm_bindgen]
        pub fn move_series(&mut self, from: usize, to: usize) {
            let n = self.sources.len();
            if from >= n || to >= n {
                return;
            }
            let src = self.sources.remove(from);
            self.sources.insert(to, src);
            self.rebuild_visible();
            self.render();
        }

        // ── Private helpers ───────────────────────────────────────────────────

        /// Rebuild `self.series` by LTTB-downsampling each source series to the
        /// visible X-range.  Target point count = max(width, MIN_TARGET_POINTS),
        /// giving roughly one point per horizontal pixel.
        ///
        /// Uses `downsample_for_view` from oxideplot-core, which uses binary
        /// search on sorted X data and extends the window by one extra point on
        /// each edge so lines don't clip while panning.
        ///
        /// Performance note: this runs on every pan/zoom event (per pointermove
        /// during a drag).  For very large datasets (~1M points) the O(n) visible
        /// scan + LTTB may be noticeable.  For this MVP the straightforward
        /// implementation is acceptable; debouncing or spatial indices can be
        /// added in a future task if profiling warrants it.
        fn rebuild_visible(&mut self) {
            let target = (self.width as usize).max(MIN_TARGET_POINTS);
            let x_min = self.view.x_min;
            let x_max = self.view.x_max;

            self.series = self
                .sources
                .iter()
                .map(|src| {
                    // Invisible series get an empty SeriesGpuData so self.series
                    // stays index-aligned with self.sources; build_draw_calls
                    // skips empty point buffers.
                    if !src.visible {
                        return SeriesGpuData {
                            points: vec![],
                            color: src.color,
                            line_width: 2.0,
                            point_radius: 3.0,
                            draw_mode: src.draw_mode,
                        };
                    }

                    let (vis_x, vis_y) =
                        downsample_for_view(&src.xs, &src.ys, x_min, x_max, target);

                    let points: Vec<[f32; 2]> = vis_x
                        .iter()
                        .zip(vis_y.iter())
                        .map(|(&x, &y)| [x as f32, y as f32])
                        .collect();

                    SeriesGpuData {
                        points,
                        color: src.color,
                        line_width: 2.0,
                        point_radius: 3.0,
                        draw_mode: src.draw_mode,
                    }
                })
                .collect();
        }

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
