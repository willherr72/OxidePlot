use eframe::egui;
use crate::state::app_state::{AppState, VERSION};
use crate::ui::data_selection_dialog::{DataSelectionState, DialogResult};
use crate::ui::settings_dialog::SettingsDialogState;
use crate::ui::graph_panel::{self, GraphAction};
use crate::data::loader;
use crate::data::unit_inference;
use crate::data::datetime;
use crate::state::data_series::{DataSeries, color_for_index};
use crate::render::gpu_plot;
use crate::plot3d::renderer as plot3d_renderer;
use std::io::Write;
use std::sync::{Arc, Mutex};

/// State for the X-axis sync selection dialog.
pub struct SyncDialogState {
    /// The graph that initiated the sync request.
    pub source_graph_id: u64,
    /// Other graphs available for syncing: (id, title).
    pub available_graphs: Vec<(u64, String)>,
    /// Parallel bool vec -- which of the available graphs are selected.
    pub selected: Vec<bool>,
}

/// What to do when a screenshot arrives.
enum PendingScreenshot {
    SaveFile { graph_id: u64 },
    Clipboard { graph_id: u64 },
}

/// Pending async file load result.
struct PendingLoad {
    graph_id: u64,
    result: Arc<Mutex<Option<Result<loader::LoadedData, String>>>>,
}

/// The main OxidePlot application.
pub struct OxidePlotApp {
    pub state: AppState,
    /// Active data-column selection dialog (shown after a file is loaded).
    pub data_selection: Option<DataSelectionState>,
    /// Active settings dialog for a particular graph.
    pub settings_dialog: Option<SettingsDialogState>,
    /// Active X-axis sync selection dialog.
    pub sync_dialog: Option<SyncDialogState>,
    /// An error message to display briefly (could be extended to a toast).
    pub error_message: Option<String>,
    /// Whether to show the About window (hidden menu).
    pub show_about: bool,
    /// Whether to show the Debug Info window (hidden menu).
    pub show_debug: bool,
    /// Async file load in progress.
    pending_load: Option<PendingLoad>,
    /// Pending screenshot action (save file or clipboard).
    pending_screenshot: Option<PendingScreenshot>,
}

impl OxidePlotApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let state = AppState::new();

        // --- Global UI style improvements ---
        let ctx = &cc.egui_ctx;
        let mut style = (*ctx.style()).clone();

        // Larger text across the board
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::proportional(15.0),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::proportional(14.5),
        );
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::proportional(22.0),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::proportional(12.0),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::monospace(13.5),
        );

        // Larger buttons with more padding
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.window_margin = egui::Margin::same(12);
        style.spacing.indent = 20.0;

        // Rounder corners for a modern look
        style.visuals.window_corner_radius = egui::CornerRadius::same(8);
        style.visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.open.corner_radius = egui::CornerRadius::same(6);

        // Wider widget stroke on hover for better affordance
        style.visuals.widgets.hovered.bg_stroke =
            egui::Stroke::new(1.5, egui::Color32::from_gray(160));
        style.visuals.widgets.active.bg_stroke =
            egui::Stroke::new(2.0, egui::Color32::from_gray(200));

        ctx.set_style(style);
        ctx.set_visuals(state.theme.visuals());

        // Initialize GPU plot rendering resources (2D and 3D).
        if let Some(render_state) = cc.wgpu_render_state.as_ref() {
            gpu_plot::init_gpu_resources(render_state);
            plot3d_renderer::init_3d_resources(render_state);
        }

        Self {
            state,
            data_selection: None,
            settings_dialog: None,
            sync_dialog: None,
            error_message: None,
            show_about: false,
            show_debug: false,
            pending_load: None,
            pending_screenshot: None,
        }
    }

    /// Open a native file dialog and, on success, parse the file and open the
    /// column-selection dialog targeting the given graph.
    fn open_file_dialog(&mut self, graph_id: u64) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Data Files", &["csv", "xls", "xlsx"])
            .add_filter("All Files", &["*"])
            .pick_file()
        {
            self.load_file(graph_id, &path);
        }
    }

    /// Parse a data file asynchronously so the UI stays responsive.
    fn load_file(&mut self, graph_id: u64, path: &std::path::Path) {
        let path_buf = path.to_path_buf();
        let result: Arc<Mutex<Option<Result<loader::LoadedData, String>>>> =
            Arc::new(Mutex::new(None));
        let result_clone = Arc::clone(&result);

        std::thread::spawn(move || {
            let loaded = loader::load_file(&path_buf);
            *result_clone.lock().unwrap() = Some(loaded);
        });

        self.pending_load = Some(PendingLoad { graph_id, result });
    }

    /// Export the data of a graph to a CSV file via a save dialog.
    fn export_csv(&self, graph_id: u64) {
        let graph = match self.state.graph_by_id(graph_id) {
            Some(g) => g,
            None => return,
        };
        if graph.series.is_empty() {
            return;
        }

        let filename = format!("{}.csv", graph.title.replace(' ', "_"));
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(&filename)
            .add_filter("CSV Files", &["csv"])
            .save_file()
        {
            if let Ok(mut file) = std::fs::File::create(&path) {
                // Header row: X column + all Y series labels
                let is_datetime = graph.x_axis_is_datetime == Some(true);
                let x_name = graph.x_axis_name.as_deref().unwrap_or("X");
                let mut header = String::from(x_name);
                for s in &graph.series {
                    header.push(',');
                    header.push_str(&s.label);
                }
                let _ = writeln!(file, "{header}");

                // Data rows
                let max_len = graph.series.iter().map(|s| s.x.len()).max().unwrap_or(0);
                for i in 0..max_len {
                    let x_val = graph.series.first()
                        .and_then(|s| s.x.get(i))
                        .copied()
                        .unwrap_or(f64::NAN);
                    let x_str = if is_datetime {
                        datetime::format_timestamp(x_val)
                    } else {
                        format!("{x_val}")
                    };
                    let mut row = x_str;
                    for s in &graph.series {
                        let yv = s.y.get(i).copied().unwrap_or(f64::NAN);
                        row.push(',');
                        if yv.is_finite() {
                            row.push_str(&format!("{yv}"));
                        }
                    }
                    let _ = writeln!(file, "{row}");
                }
                tracing::info!("Exported CSV to {:?}", path);
            }
        }
    }

    /// Save the current project state to a JSON file.
    fn save_project(&self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name("project.oxideplot")
            .add_filter("OxidePlot Project", &["oxideplot", "json"])
            .save_file()
        {
            match serde_json::to_string_pretty(&self.state) {
                Ok(json) => {
                    if let Err(e) = std::fs::write(&path, json) {
                        tracing::error!("Failed to save project: {e}");
                    } else {
                        tracing::info!("Project saved to {:?}", path);
                    }
                }
                Err(e) => tracing::error!("Failed to serialize project: {e}"),
            }
        }
    }

    /// Load a project from a JSON file.
    fn load_project(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("OxidePlot Project", &["oxideplot", "json"])
            .pick_file()
        {
            match std::fs::read_to_string(&path) {
                Ok(json) => match serde_json::from_str::<AppState>(&json) {
                    Ok(loaded_state) => {
                        self.state = loaded_state;
                        tracing::info!("Project loaded from {:?}", path);
                    }
                    Err(e) => {
                        self.error_message = Some(format!("Failed to parse project: {e}"));
                    }
                },
                Err(e) => {
                    self.error_message = Some(format!("Failed to read file: {e}"));
                }
            }
        }
    }

    /// Called when the user presses OK in the column-selection dialog.
    /// Reads the selected X/Y columns from the loaded data and creates
    /// `DataSeries` entries on the target graph.
    fn process_column_selection(
        &mut self,
        selection: crate::ui::data_selection_dialog::ColumnSelection,
    ) {
        // We still have self.data_selection at this point.  Extract what we
        // need before we clear it.
        let loaded = match self.data_selection.as_ref() {
            Some(ds) => &ds.loaded_data,
            None => return,
        };

        let x_col_name = loaded.columns[selection.x_col_index].clone();
        let x_data: Vec<String> = loaded.column_data[selection.x_col_index].clone();

        // Collect Y column info before we drop the borrow.
        let y_info: Vec<(String, Vec<String>)> = selection
            .y_col_indices
            .iter()
            .map(|&idx| {
                (
                    loaded.columns[idx].clone(),
                    loaded.column_data[idx].clone(),
                )
            })
            .collect();

        // Optional Z column for 3D.
        let z_info: Option<(String, Vec<String>)> = selection.z_col_index.map(|idx| {
            (
                loaded.columns[idx].clone(),
                loaded.column_data[idx].clone(),
            )
        });

        // Done reading loaded data -- now clear the dialog state.
        self.data_selection = None;

        // --- Resolve X values --------------------------------------------------
        let is_time_col = {
            let lc = x_col_name.to_lowercase();
            lc.contains("time") || lc.contains("date") || x_col_name == "RTC-TIME"
        };

        let (x_values, is_datetime) = if is_time_col {
            // Prefer numeric timestamps if the column parses well.
            let (numeric, frac) = loader::column_to_f64(&x_data);
            if frac > 0.7 {
                (numeric, true)
            } else if let Some((timestamps, _)) =
                loader::column_to_timestamps(&x_data)
            {
                let fixed =
                    datetime::fix_error_timestamps(&timestamps, 0.0, 978307199.0, 10.0);
                (fixed, true)
            } else {
                let (numeric, _) = loader::column_to_f64(&x_data);
                (numeric, false)
            }
        } else {
            let (numeric, frac) = loader::column_to_f64(&x_data);
            if frac > 0.7 {
                (numeric, false)
            } else if let Some((timestamps, _)) =
                loader::column_to_timestamps(&x_data)
            {
                let fixed =
                    datetime::fix_error_timestamps(&timestamps, 0.0, 978307199.0, 10.0);
                (fixed, true)
            } else {
                // Fall back to row indices.
                let indices: Vec<f64> = (0..x_data.len()).map(|i| i as f64).collect();
                (indices, false)
            }
        };

        // --- Write to target graph ---------------------------------------------
        let graph = match self.state.graph_by_id_mut(selection.graph_id) {
            Some(g) => g,
            None => return,
        };

        if graph.series.is_empty() {
            graph.x_axis_is_datetime = Some(is_datetime);
            graph.x_axis_name = Some(x_col_name.clone());
            if y_info.len() == 1 {
                graph.title = format!("{} vs. {}", x_col_name, y_info[0].0);
            } else {
                graph.title = format!("{x_col_name} vs. Multiple Data");
            }
        }

        // Pre-parse Z values if 3D.
        let z_values: Option<Vec<f64>> = z_info.as_ref().map(|(_, z_data)| {
            let (vals, _) = loader::column_to_f64(z_data);
            vals
        });

        // If Z column provided, switch graph to 3D mode.
        if z_values.is_some() {
            graph.plot_mode = crate::state::data_series::PlotMode::Plot3D;
        }

        for (y_col_name, y_data) in &y_info {
            let (y_values, _) = loader::column_to_f64(y_data);

            if let Some(ref z_vals) = z_values {
                // 3D: filter NaN/Inf from x, y, and z together.
                let mut filtered_x = Vec::new();
                let mut filtered_y = Vec::new();
                let mut filtered_z = Vec::new();
                for i in 0..x_values.len().min(y_values.len()).min(z_vals.len()) {
                    let xv = x_values[i];
                    let yv = y_values[i];
                    let zv = z_vals[i];
                    if xv.is_finite() && yv.is_finite() && zv.is_finite() {
                        filtered_x.push(xv);
                        filtered_y.push(yv);
                        filtered_z.push(zv);
                    }
                }

                if filtered_x.is_empty() {
                    continue;
                }

                let unit = unit_inference::infer_unit(y_col_name);
                let color = color_for_index(graph.series_counter);
                let label = format!("{y_col_name} ({unit})");
                let series = DataSeries::new_3d(
                    label, filtered_x, filtered_y, filtered_z, color, unit,
                );
                graph.add_series(series);
            } else {
                // 2D: filter NaN/Inf from x and y.
                let mut filtered_x = Vec::new();
                let mut filtered_y = Vec::new();
                for (xv, yv) in x_values.iter().zip(y_values.iter()) {
                    if xv.is_finite() && yv.is_finite() {
                        filtered_x.push(*xv);
                        filtered_y.push(*yv);
                    }
                }

                if filtered_x.is_empty() {
                    continue;
                }

                let unit = unit_inference::infer_unit(y_col_name);
                let color = color_for_index(graph.series_counter);
                let label = format!("{y_col_name} ({unit})");
                let series = DataSeries::new(label, filtered_x, filtered_y, color, unit);
                graph.add_series(series);
            }
        }
    }
}

impl eframe::App for OxidePlotApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply theme visuals every frame (cheap) while preserving our custom rounding.
        let mut vis = self.state.theme.visuals();
        vis.window_corner_radius = egui::CornerRadius::same(8);
        vis.widgets.noninteractive.corner_radius = egui::CornerRadius::same(6);
        vis.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
        vis.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
        vis.widgets.active.corner_radius = egui::CornerRadius::same(6);
        vis.widgets.open.corner_radius = egui::CornerRadius::same(6);
        vis.widgets.hovered.bg_stroke =
            egui::Stroke::new(1.5, egui::Color32::from_gray(160));
        vis.widgets.active.bg_stroke =
            egui::Stroke::new(2.0, egui::Color32::from_gray(200));
        ctx.set_visuals(vis);

        // ------------------------------------------------------------------
        // 0. Handle screenshot events from previous frame
        // ------------------------------------------------------------------
        if self.pending_screenshot.is_some() {
            let mut screenshot_image: Option<Arc<egui::ColorImage>> = None;
            ctx.input(|i| {
                for event in &i.raw.events {
                    if let egui::Event::Screenshot { image, .. } = event {
                        screenshot_image = Some(image.clone());
                    }
                }
            });

            if let Some(color_image) = screenshot_image {
                let action = self.pending_screenshot.take().unwrap();
                let graph_id = match &action {
                    PendingScreenshot::SaveFile { graph_id } => *graph_id,
                    PendingScreenshot::Clipboard { graph_id } => *graph_id,
                };

                // Crop to just the graph panel rect
                let ppp = ctx.pixels_per_point();
                let full_w = color_image.width();
                let crop_rect = self
                    .state
                    .graph_by_id(graph_id)
                    .and_then(|g| g.last_frame_rect);

                let (rgba, width, height) = if let Some(rect) = crop_rect {
                    let x0 = ((rect.left() * ppp) as usize).min(full_w);
                    let y0 = ((rect.top() * ppp) as usize).min(color_image.height());
                    let x1 = ((rect.right() * ppp).ceil() as usize).min(full_w);
                    let y1 = ((rect.bottom() * ppp).ceil() as usize).min(color_image.height());
                    let cw = x1.saturating_sub(x0);
                    let ch = y1.saturating_sub(y0);
                    let mut cropped = Vec::with_capacity(cw * ch * 4);
                    for row in y0..y1 {
                        for col in x0..x1 {
                            let c = color_image.pixels[row * full_w + col];
                            cropped.extend_from_slice(&[c.r(), c.g(), c.b(), c.a()]);
                        }
                    }
                    (cropped, cw, ch)
                } else {
                    let w = color_image.width();
                    let h = color_image.height();
                    let rgba: Vec<u8> = color_image
                        .pixels
                        .iter()
                        .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
                        .collect();
                    (rgba, w, h)
                };

                match action {
                    PendingScreenshot::SaveFile { .. } => {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name("plot.png")
                            .add_filter("PNG Image", &["png"])
                            .save_file()
                        {
                            if let Some(img) = image::RgbaImage::from_raw(
                                width as u32,
                                height as u32,
                                rgba,
                            ) {
                                if let Err(e) = img.save(&path) {
                                    self.error_message =
                                        Some(format!("Failed to save image: {e}"));
                                } else {
                                    tracing::info!("Saved screenshot to {:?}", path);
                                }
                            }
                        }
                    }
                    PendingScreenshot::Clipboard { .. } => {
                        match arboard::Clipboard::new() {
                            Ok(mut clipboard) => {
                                let img_data = arboard::ImageData {
                                    width,
                                    height,
                                    bytes: std::borrow::Cow::Owned(rgba),
                                };
                                if let Err(e) = clipboard.set_image(img_data) {
                                    self.error_message =
                                        Some(format!("Failed to copy to clipboard: {e}"));
                                } else {
                                    tracing::info!("Copied screenshot to clipboard");
                                }
                            }
                            Err(e) => {
                                self.error_message =
                                    Some(format!("Failed to access clipboard: {e}"));
                            }
                        }
                    }
                }
            }
        }

        // ------------------------------------------------------------------
        // 1. Handle dropped files (collect paths first to avoid borrow issues)
        // ------------------------------------------------------------------
        let mut dropped_paths: Vec<std::path::PathBuf> = Vec::new();
        ctx.input(|i| {
            for file in &i.raw.dropped_files {
                if let Some(path) = &file.path {
                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.to_lowercase())
                        .unwrap_or_default();
                    if ext == "csv" || ext == "xls" || ext == "xlsx" {
                        dropped_paths.push(path.clone());
                    }
                }
            }
        });

        for path in dropped_paths {
            // Drop onto the first graph, or create one if there are none.
            let graph_id = self
                .state
                .graphs
                .first()
                .map(|g| g.id)
                .unwrap_or_else(|| self.state.add_graph().id);
            self.load_file(graph_id, &path);
        }

        // ------------------------------------------------------------------
        // 2. Collect graph-panel actions (we render everything in here)
        // ------------------------------------------------------------------
        let mut actions: Vec<(u64, GraphAction)> = Vec::new();

        // --- Header panel ---
        let mut save_project = false;
        let mut load_project = false;
        egui::TopBottomPanel::top("header")
            .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(egui::Margin::symmetric(16, 8)))
            .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.visuals_mut().override_text_color = Some(ui.visuals().strong_text_color());
                let heading_response = ui.heading("OxidePlot");
                ui.visuals_mut().override_text_color = None;
                heading_response.context_menu(|ui| {
                    if ui.button("About OxidePlot").clicked() {
                        self.show_about = true;
                        ui.close_menu();
                    }
                    if ui.button("Debug Info").clicked() {
                        self.show_debug = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Reset All Graphs").clicked() {
                        self.state.graphs.clear();
                        self.state.add_graph();
                        ui.close_menu();
                    }
                });

                ui.separator();

                if ui.button("Save Project").clicked() {
                    save_project = true;
                }
                if ui.button("Load Project").clicked() {
                    load_project = true;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Theme toggle with sun/moon icon
                    let theme_icon = match self.state.theme {
                        crate::state::theme::Theme::Dark => "Light Mode",
                        crate::state::theme::Theme::Light => "Dark Mode",
                    };
                    if ui.button(theme_icon).clicked() {
                        self.state.theme = self.state.theme.toggle();
                    }

                    ui.separator();
                    ui.small(format!("v{VERSION}"));
                });
            });
        });

        if save_project {
            self.save_project();
        }
        if load_project {
            self.load_project();
        }

        // --- Footer panel ---
        egui::TopBottomPanel::bottom("footer")
            .frame(egui::Frame::side_top_panel(&ctx.style()).inner_margin(egui::Margin::symmetric(16, 6)))
            .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let add_btn = egui::Button::new(
                    egui::RichText::new("+ New Graph").strong()
                ).min_size(egui::vec2(120.0, 28.0));
                if ui.add(add_btn).clicked() {
                    self.state.add_graph();
                }

                ui.separator();

                let count = self.state.graphs.len();
                let label = if count == 1 { "1 graph" } else { &format!("{count} graphs") };
                ui.label(egui::RichText::new(label).weak());

                // Show transient error if any.
                if let Some(msg) = &self.error_message {
                    ui.separator();
                    ui.colored_label(egui::Color32::from_rgb(255, 80, 80), msg);
                    if ui.small_button("dismiss").clicked() {
                        self.error_message = None;
                    }
                }
            });
        });

        // --- Central panel with graph panels ---
        egui::CentralPanel::default().show(ctx, |ui| {
            let viewport_height = ui.available_height();
            let graph_count = self.state.graphs.len();
            let spacing = 10.0_f32;
            let panel_height = if graph_count <= 1 {
                viewport_height
            } else {
                ((viewport_height - spacing) / 2.0).max(300.0)
            };

            egui::ScrollArea::vertical().show(ui, |ui| {
                let graph_ids: Vec<u64> =
                    self.state.graphs.iter().map(|g| g.id).collect();

                if graph_ids.is_empty() {
                    ui.add_space(80.0);
                    ui.vertical_centered(|ui| {
                        ui.heading("Welcome to OxidePlot");
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new(
                                "Click \"+ New Graph\" below, or drag-and-drop a CSV / Excel file to get started."
                            ).weak()
                        );
                    });
                }

                for (idx, &gid) in graph_ids.iter().enumerate() {
                    let theme = self.state.theme;
                    if let Some(graph) = self.state.graph_by_id_mut(gid) {
                        ui.push_id(gid, |ui| {
                            let action =
                                graph_panel::show_graph_panel(graph, ui, idx, &theme, panel_height);
                            match action {
                                GraphAction::None => {}
                                other => actions.push((gid, other)),
                            }
                        });
                        ui.add_space(spacing);
                    }
                }

                // DnD drop detection: overlay each graph panel rect as a drop target
                let mut graph_reorder: Option<(usize, usize)> = None;
                if egui::DragAndDrop::has_any_payload(ui.ctx()) {
                    // Collect rects first to avoid borrow conflicts
                    let rects: Vec<(usize, Option<egui::Rect>)> = graph_ids
                        .iter()
                        .enumerate()
                        .map(|(idx, &gid)| {
                            let rect = self.state.graph_by_id(gid).and_then(|g| g.last_frame_rect);
                            (idx, rect)
                        })
                        .collect();

                    for (idx, rect) in &rects {
                        if let Some(rect) = rect {
                            let drop_resp = ui.interact(
                                *rect,
                                egui::Id::new("graph_drop").with(idx),
                                egui::Sense::hover(),
                            );

                            // Shared logic for hover indicator and drop
                            let compute_target = |resp: &egui::Response, from: usize| -> Option<usize> {
                                let top_half = resp.hover_pos()
                                    .map_or(false, |p| p.y < rect.center().y);
                                let to = if top_half { *idx } else { idx + 1 };
                                // Skip no-ops: inserting at `from` or `from+1` results in same position
                                if to == from || to == from + 1 { None } else { Some(to) }
                            };

                            if let Some(payload) = drop_resp.dnd_hover_payload::<usize>() {
                                if let Some(to) = compute_target(&drop_resp, *payload) {
                                    let y = if to <= *idx { rect.top() } else { rect.bottom() };
                                    ui.painter().hline(
                                        rect.x_range(),
                                        y,
                                        egui::Stroke::new(3.0, egui::Color32::from_rgb(80, 140, 255)),
                                    );
                                }
                            }
                            if let Some(payload) = drop_resp.dnd_release_payload::<usize>() {
                                if let Some(to) = compute_target(&drop_resp, *payload) {
                                    graph_reorder = Some((*payload, to));
                                }
                            }
                        }
                    }
                }

                // Apply graph reorder
                if let Some((from, to)) = graph_reorder {
                    let len = self.state.graphs.len();
                    if from < len {
                        let graph = self.state.graphs.remove(from);
                        let insert_at = if to > from { to - 1 } else { to };
                        let insert_at = insert_at.min(self.state.graphs.len());
                        self.state.graphs.insert(insert_at, graph);
                    }
                }
            });
        });

        // ------------------------------------------------------------------
        // 2b. Synchronize X-axis across synced graphs
        // ------------------------------------------------------------------
        // Find which synced graph(s) had their X range change this frame,
        // then propagate to their partners.
        {
            // First pass: find changed graphs and what they want to set.
            let mut propagations: Vec<(u64, f64, f64)> = Vec::new();
            for graph in &self.state.graphs {
                if !graph.sync_partner_ids.is_empty() && graph.plot_view.x_range_changed() {
                    let x_min = graph.plot_view.x_min;
                    let x_max = graph.plot_view.x_max;
                    for &partner_id in &graph.sync_partner_ids {
                        propagations.push((partner_id, x_min, x_max));
                    }
                }
            }

            // Second pass: apply propagations.
            for (target_id, x_min, x_max) in &propagations {
                if let Some(target) = self.state.graph_by_id_mut(*target_id) {
                    target.plot_view.x_min = *x_min;
                    target.plot_view.x_max = *x_max;
                }
            }

            // Third pass: snapshot current X ranges for next frame comparison.
            for graph in &mut self.state.graphs {
                graph.plot_view.snapshot_x_range();
            }
        }

        // ------------------------------------------------------------------
        // 3. Process collected graph-panel actions
        // ------------------------------------------------------------------
        for (gid, action) in actions {
            match action {
                GraphAction::Close => {
                    self.state.remove_graph(gid);
                }
                GraphAction::AddData => {
                    self.open_file_dialog(gid);
                }
                GraphAction::OpenSettings => {
                    let series_count = self
                        .state
                        .graph_by_id(gid)
                        .map(|g| g.series.len())
                        .unwrap_or(0);
                    self.settings_dialog =
                        Some(SettingsDialogState::new(gid, series_count));
                }
                GraphAction::ToggleTableView => {
                    if let Some(g) = self.state.graph_by_id_mut(gid) {
                        g.show_data_table = !g.show_data_table;
                    }
                }
                GraphAction::CenterView => {
                    if let Some(g) = self.state.graph_by_id_mut(gid) {
                        g.plot_view.auto_fit = true;
                        g.camera.reset();
                    }
                }
                GraphAction::ExportCsv => {
                    self.export_csv(gid);
                }
                GraphAction::ExportImageSave => {
                    self.pending_screenshot = Some(PendingScreenshot::SaveFile { graph_id: gid });
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                }
                GraphAction::ExportImageClipboard => {
                    self.pending_screenshot = Some(PendingScreenshot::Clipboard { graph_id: gid });
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                }
                GraphAction::ToggleSync => {
                    if let Some(g) = self.state.graph_by_id(gid) {
                        if !g.sync_partner_ids.is_empty() {
                            // Already synced -- unsync this graph from its group.
                            let partners = g.sync_partner_ids.clone();
                            // Remove this graph's id from every partner's list.
                            for &pid in &partners {
                                if let Some(pg) = self.state.graph_by_id_mut(pid) {
                                    pg.sync_partner_ids.retain(|id| *id != gid);
                                }
                            }
                            // Clear this graph's own list.
                            if let Some(g) = self.state.graph_by_id_mut(gid) {
                                g.sync_partner_ids.clear();
                            }
                            self.state.recompute_sync_groups();
                        } else {
                            // Not synced yet -- open sync dialog if there are
                            // other graphs to sync with.
                            let available: Vec<(u64, String)> = self
                                .state
                                .graphs
                                .iter()
                                .filter(|g| g.id != gid)
                                .map(|g| (g.id, g.title.clone()))
                                .collect();
                            if !available.is_empty() {
                                let count = available.len();
                                self.sync_dialog = Some(SyncDialogState {
                                    source_graph_id: gid,
                                    available_graphs: available,
                                    selected: vec![false; count],
                                });
                            }
                        }
                    }
                }
                GraphAction::None => {}
            }
        }

        // ------------------------------------------------------------------
        // 3b. Poll async file load
        // ------------------------------------------------------------------
        if let Some(ref pending) = self.pending_load {
            let mut lock = pending.result.lock().unwrap();
            if let Some(result) = lock.take() {
                let graph_id = pending.graph_id;
                match result {
                    Ok(loaded_data) => {
                        self.data_selection = Some(DataSelectionState::new(loaded_data, graph_id));
                    }
                    Err(e) => {
                        tracing::error!("Failed to load file: {e}");
                        self.error_message = Some(format!("Failed to load file: {e}"));
                    }
                }
                drop(lock);
                self.pending_load = None;
            }
        }

        // Show loading indicator
        if self.pending_load.is_some() {
            egui::Window::new("Loading")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading file...");
                    });
                });
            ctx.request_repaint();
        }

        // ------------------------------------------------------------------
        // 4. Data-selection dialog
        // ------------------------------------------------------------------
        let mut selection_to_process = None;
        if let Some(ref mut ds) = self.data_selection {
            match crate::ui::data_selection_dialog::show_data_selection_dialog(ctx, ds)
            {
                Some(DialogResult::Ok(selection)) => {
                    selection_to_process = Some(selection);
                }
                Some(DialogResult::Cancel) => {
                    self.data_selection = None;
                }
                None => {} // dialog still open
            }
        }
        if let Some(selection) = selection_to_process {
            self.process_column_selection(selection);
        }

        // ------------------------------------------------------------------
        // 5. Settings dialog
        // ------------------------------------------------------------------
        let mut close_settings = false;
        if let Some(ref mut sd) = self.settings_dialog {
            let gid = sd.graph_id;
            if let Some(graph) = self.state.graph_by_id_mut(gid) {
                let keep = crate::ui::settings_dialog::show_settings_dialog(
                    ctx, sd, graph,
                );
                if !keep {
                    close_settings = true;
                }
            } else {
                close_settings = true;
            }
        }
        if close_settings {
            self.settings_dialog = None;
        }

        // ------------------------------------------------------------------
        // 5b. About window (hidden menu)
        // ------------------------------------------------------------------
        if self.show_about {
            egui::Window::new("About OxidePlot")
                .open(&mut self.show_about)
                .collapsible(false)
                .resizable(false)
                .default_width(320.0)
                .show(ctx, |ui| {
                    ui.heading("OxidePlot");
                    ui.label(format!("Version: {VERSION}"));
                    ui.add_space(4.0);
                    ui.label("A high-performance data visualization tool.");
                    ui.add_space(10.0);
                    ui.label("Features:");
                    ui.label("  \u{2022} Multi-series plotting");
                    ui.label("  \u{2022} CSV and Excel file support");
                    ui.label("  \u{2022} Drag and drop");
                    ui.label("  \u{2022} Interactive data analysis");
                    ui.label("  \u{2022} Unit conversion");
                    ui.label("  \u{2022} Mathematical operations");
                    ui.add_space(10.0);
                    ui.label("Right-click the title for this menu.");
                });
        }

        // ------------------------------------------------------------------
        // 5c. Debug Info window (hidden menu)
        // ------------------------------------------------------------------
        if self.show_debug {
            egui::Window::new("Debug Info")
                .open(&mut self.show_debug)
                .collapsible(false)
                .resizable(false)
                .default_width(300.0)
                .show(ctx, |ui| {
                    ui.label(format!("Active Graphs: {}", self.state.graphs.len()));
                    let total_series: usize =
                        self.state.graphs.iter().map(|g| g.series.len()).sum();
                    ui.label(format!("Total Data Series: {total_series}"));
                    let total_points: usize = self
                        .state
                        .graphs
                        .iter()
                        .flat_map(|g| g.series.iter())
                        .map(|s| s.x.len())
                        .sum();
                    ui.label(format!("Total Data Points: {total_points}"));
                    ui.label(format!("Theme: {:?}", self.state.theme));
                });
        }

        // ------------------------------------------------------------------
        // 6. Sync dialog
        // ------------------------------------------------------------------
        let mut close_sync = false;
        let mut sync_apply: Option<(u64, Vec<u64>)> = None;
        if let Some(ref mut sd) = self.sync_dialog {
            let mut open = true;
            egui::Window::new("Sync X-Axis")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_width(400.0)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Select graphs to synchronize X-axis with:");
                    ui.add_space(8.0);

                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            for (i, (_gid, title)) in sd.available_graphs.iter().enumerate() {
                                ui.checkbox(&mut sd.selected[i], title.as_str());
                            }
                        });

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let any_selected = sd.selected.iter().any(|&s| s);
                        let apply_btn = ui.add_enabled(
                            any_selected,
                            egui::Button::new("Apply").min_size(egui::vec2(80.0, 28.0)),
                        );
                        if apply_btn.clicked() {
                            let chosen: Vec<u64> = sd
                                .available_graphs
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| sd.selected[*i])
                                .map(|(_, (id, _))| *id)
                                .collect();
                            if !chosen.is_empty() {
                                sync_apply = Some((sd.source_graph_id, chosen));
                            }
                            close_sync = true;
                        }
                        if ui.add(egui::Button::new("Cancel").min_size(egui::vec2(80.0, 28.0))).clicked() {
                            close_sync = true;
                        }
                    });
                });
            if !open {
                close_sync = true;
            }
        }

        // Apply sync selections after the dialog UI has been drawn.
        if let Some((source_id, partner_ids)) = sync_apply {
            // Build the full group: source + all chosen partners.
            let mut group: Vec<u64> = vec![source_id];
            group.extend_from_slice(&partner_ids);

            // For each member of the group, ensure every *other* member is in
            // its sync_partner_ids list.
            for &member_id in &group {
                if let Some(g) = self.state.graph_by_id_mut(member_id) {
                    for &other in &group {
                        if other != member_id && !g.sync_partner_ids.contains(&other) {
                            g.sync_partner_ids.push(other);
                        }
                    }
                }
            }
            self.state.recompute_sync_groups();
        }

        if close_sync {
            self.sync_dialog = None;
        }
    }
}
