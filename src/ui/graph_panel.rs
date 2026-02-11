use crate::data::datetime;
use crate::processing::downsampling;
use crate::state::data_series::{InterpolationMode, PlotMode};
use crate::state::graph_state::{CursorMode, GraphState, SortDirection};
use crate::state::theme::Theme;
use crate::render::gpu_plot::create_plot_paint_callback;
use crate::render::gpu_types::{DrawMode, GridGpuData, Line3DData, PlotUniforms, Scatter3DData, SeriesGpuData};
use crate::render::plot_interaction;
use crate::plot3d::renderer::create_3d_paint_callback;

/// Actions that the graph panel can request from the parent.
pub enum GraphAction {
    None,
    Close,
    AddData,
    OpenSettings,
    ToggleSync,
    ToggleTableView,
    CenterView,
    ExportCsv,
    ExportImageSave,
    ExportImageClipboard,
}

/// Helper to create a toolbar button with consistent min size.
fn toolbar_btn(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(label).min_size(egui::vec2(0.0, 26.0)))
}

/// Helper to create a selected/toggled toolbar button.
fn toolbar_toggle_btn(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let btn = if active {
        egui::Button::new(egui::RichText::new(label).strong())
            .fill(ui.visuals().selection.bg_fill)
            .min_size(egui::vec2(0.0, 26.0))
    } else {
        egui::Button::new(label).min_size(egui::vec2(0.0, 26.0))
    };
    ui.add(btn)
}

/// Render a single graph panel. Returns an action if the user clicked a button.
/// `panel_height` is the target total height for this graph panel.
pub fn show_graph_panel(
    graph: &mut GraphState,
    ui: &mut egui::Ui,
    graph_index: usize,
    theme: &Theme,
    panel_height: f32,
) -> GraphAction {
    let mut action = GraphAction::None;

    let frame_resp = egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(10))
        .corner_radius(egui::CornerRadius::same(8))
        .show(ui, |ui| {
        // --- Title row ---
        ui.horizontal(|ui| {
            // Drag handle for reordering graphs — painted grip lines
            let (handle_rect, handle_resp) = ui.allocate_exact_size(
                egui::vec2(14.0, 26.0),
                egui::Sense::drag(),
            );
            let grip_color = if handle_resp.hovered() || handle_resp.dragged() {
                ui.visuals().text_color().gamma_multiply(0.7)
            } else {
                ui.visuals().text_color().gamma_multiply(0.3)
            };
            let cx = handle_rect.center().x;
            for dy in [-4.0_f32, 0.0, 4.0] {
                let y = handle_rect.center().y + dy;
                ui.painter().hline(
                    (cx - 5.0)..=(cx + 5.0),
                    y,
                    egui::Stroke::new(1.5, grip_color),
                );
            }
            handle_resp.dnd_set_drag_payload(graph_index);

            ui.add(
                egui::TextEdit::singleline(&mut graph.title)
                    .font(egui::TextStyle::Heading)
                    .desired_width(ui.available_width() - 90.0)
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Close button - visually distinct in red
                let close_btn = egui::Button::new(
                    egui::RichText::new("Close").color(egui::Color32::from_rgb(220, 60, 60))
                ).min_size(egui::vec2(0.0, 26.0));
                if ui.add(close_btn).on_hover_text("Remove this graph").clicked() {
                    action = GraphAction::Close;
                }
            });
        });

        ui.add_space(2.0);

        // --- Toolbar row ---
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;

            // == View group ==
            if toolbar_btn(ui, "Settings").on_hover_text("Open graph settings").clicked() {
                action = GraphAction::OpenSettings;
            }
            let view_active = graph.show_data_table;
            if toolbar_toggle_btn(ui, if view_active { "Graph View" } else { "Table View" }, view_active)
                .on_hover_text("Toggle between chart and data table")
                .clicked()
            {
                action = GraphAction::ToggleTableView;
            }

            ui.separator();

            // == Data group ==
            if toolbar_btn(ui, "Add Data").on_hover_text("Import CSV or Excel file").clicked() {
                action = GraphAction::AddData;
            }
            let remove_popup_id = ui.make_persistent_id(format!("remove_popup_{}", graph.id));
            let remove_btn_resp = toolbar_btn(ui, "Remove Data").on_hover_text("Remove series from graph");
            if remove_btn_resp.clicked() {
                graph.remove_series_selected = vec![false; graph.series.len()];
                ui.memory_mut(|m| m.toggle_popup(remove_popup_id));
            }
            egui::popup_below_widget(ui, remove_popup_id, &remove_btn_resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                ui.set_min_width(220.0);
                if graph.series.is_empty() {
                    ui.label(egui::RichText::new("No series to remove.").weak());
                } else {
                    ui.label(egui::RichText::new("Select series to remove:").strong());
                    ui.add_space(4.0);
                    // Ensure vec is correct length
                    if graph.remove_series_selected.len() != graph.series.len() {
                        graph.remove_series_selected.resize(graph.series.len(), false);
                    }
                    for (i, series) in graph.series.iter().enumerate() {
                        let color = series.color32();
                        ui.checkbox(
                            &mut graph.remove_series_selected[i],
                            egui::RichText::new(&series.label).color(color),
                        );
                    }
                    ui.add_space(4.0);
                    let any_selected = graph.remove_series_selected.iter().any(|&s| s);
                    let del_btn = ui.add_enabled(
                        any_selected,
                        egui::Button::new(
                            egui::RichText::new("Delete Selected")
                                .color(egui::Color32::from_rgb(220, 60, 60)),
                        ).min_size(egui::vec2(0.0, 28.0)),
                    );
                    if del_btn.clicked() {
                        let ids_to_remove: Vec<u64> = graph
                            .remove_series_selected
                            .iter()
                            .enumerate()
                            .filter(|(_, &sel)| sel)
                            .filter_map(|(i, _)| graph.series.get(i).map(|s| s.id))
                            .collect();
                        for id in &ids_to_remove {
                            graph.remove_series_by_id(*id);
                        }
                        graph.remove_series_selected = vec![false; graph.series.len()];
                        ui.memory_mut(|m| m.toggle_popup(remove_popup_id));
                    }
                }
            });
            ui.separator();

            // == View controls group ==
            if toolbar_btn(ui, "Fit View").on_hover_text("Auto-fit to data bounds").clicked() {
                action = GraphAction::CenterView;
            }

            if toolbar_toggle_btn(ui, "Auto Y", graph.auto_scale_y)
                .on_hover_text("Auto-scale Y axis to visible data")
                .clicked()
            {
                graph.auto_scale_y = !graph.auto_scale_y;
            }

            // 2D/3D toggle - clear labels
            let is_3d = graph.plot_mode == PlotMode::Plot3D;
            if toolbar_toggle_btn(ui, "2D", !is_3d).on_hover_text("2D plot mode").clicked() {
                graph.plot_mode = PlotMode::Plot2D;
            }
            if toolbar_toggle_btn(ui, "3D", is_3d).on_hover_text("3D plot mode").clicked() {
                graph.plot_mode = PlotMode::Plot3D;
            }

            ui.separator();

            // == Measurement group - cursor controls with clear labels ==
            let _cursor_off = graph.cursor_state.mode == CursorMode::Off;
            let cursor_v = graph.cursor_state.mode == CursorMode::Vertical;
            let cursor_h = graph.cursor_state.mode == CursorMode::Horizontal;

            if toolbar_toggle_btn(ui, "V-Cursor", cursor_v)
                .on_hover_text("Vertical cursor measurement (right-click to place)")
                .clicked()
            {
                if cursor_v {
                    // Turn off
                    graph.cursor_state.mode = CursorMode::Off;
                    graph.cursor_state.cursor1 = None;
                    graph.cursor_state.cursor2 = None;
                } else {
                    graph.cursor_state.mode = CursorMode::Vertical;
                    graph.cursor_state.cursor1 = None;
                    graph.cursor_state.cursor2 = None;
                }
            }
            if toolbar_toggle_btn(ui, "H-Cursor", cursor_h)
                .on_hover_text("Horizontal cursor measurement (right-click to place)")
                .clicked()
            {
                if cursor_h {
                    // Turn off
                    graph.cursor_state.mode = CursorMode::Off;
                    graph.cursor_state.cursor1 = None;
                    graph.cursor_state.cursor2 = None;
                } else {
                    graph.cursor_state.mode = CursorMode::Horizontal;
                    graph.cursor_state.cursor1 = None;
                    graph.cursor_state.cursor2 = None;
                }
            }

            ui.separator();

            // == Sync ==
            let is_synced = !graph.sync_partner_ids.is_empty();
            let sync_label = if is_synced { "Synced" } else { "Sync X-Axis" };
            if toolbar_toggle_btn(ui, sync_label, is_synced)
                .on_hover_text(if is_synced { "Click to unsync" } else { "Sync X-axis with other graphs" })
                .clicked()
            {
                action = GraphAction::ToggleSync;
            }

            ui.separator();

            // == Export (right side) ==
            let export_popup_id = ui.make_persistent_id(format!("export_popup_{}", graph.id));
            let export_btn_resp = toolbar_btn(ui, "Export").on_hover_text("Export data or image");
            if export_btn_resp.clicked() {
                ui.memory_mut(|m| m.toggle_popup(export_popup_id));
            }
            egui::popup_below_widget(ui, export_popup_id, &export_btn_resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                ui.set_min_width(160.0);
                if ui.button("Save as CSV").clicked() {
                    action = GraphAction::ExportCsv;
                    ui.memory_mut(|m| m.toggle_popup(export_popup_id));
                }
                if ui.button("Save as Image").clicked() {
                    action = GraphAction::ExportImageSave;
                    ui.memory_mut(|m| m.toggle_popup(export_popup_id));
                }
                if ui.button("Copy Image").clicked() {
                    action = GraphAction::ExportImageClipboard;
                    ui.memory_mut(|m| m.toggle_popup(export_popup_id));
                }
            });
        });

        ui.add_space(4.0);

        // Compute remaining height for the plot area.
        // Overhead: frame inner margin (20) + title row (~32) + toolbar (~32)
        // + spacing (2+4) + item spacing (~18) + frame border (~2) ≈ 110
        let plot_area_height = (panel_height - 110.0).max(150.0);

        // --- Main content ---
        if graph.show_data_table {
            show_table_view(graph, ui);
        } else {
            match graph.plot_mode {
                PlotMode::Plot2D => show_gpu_plot(graph, ui, theme, plot_area_height),
                PlotMode::Plot3D => show_3d_plot(graph, ui, theme, plot_area_height),
            }
        }
    });
    graph.last_frame_rect = Some(frame_resp.response.rect);

    action
}

// ---------------------------------------------------------------------------
// Multi-unit normalization helpers (carried over from Phase 2)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct UnitRange {
    y_min: f64,
    y_max: f64,
}

impl UnitRange {
    fn normalize(&self, y: f64) -> f64 {
        let span = self.y_max - self.y_min;
        if span.abs() < 1e-15 {
            0.5
        } else {
            (y - self.y_min) / span
        }
    }

    fn denormalize(&self, n: f64) -> f64 {
        let span = self.y_max - self.y_min;
        n * span + self.y_min
    }
}

fn collect_unit_order(graph: &GraphState) -> Vec<String> {
    let mut unit_order: Vec<String> = Vec::new();
    for series in &graph.series {
        if series.visible && !unit_order.contains(&series.unit) {
            unit_order.push(series.unit.clone());
        }
    }
    unit_order
}

fn compute_unit_ranges(graph: &GraphState, unit_order: &[String]) -> Vec<UnitRange> {
    unit_order
        .iter()
        .map(|unit| {
            let mut y_min = f64::INFINITY;
            let mut y_max = f64::NEG_INFINITY;
            for series in &graph.series {
                if !series.visible || series.unit != *unit {
                    continue;
                }
                for &yv in &series.y {
                    if yv.is_finite() {
                        y_min = y_min.min(yv);
                        y_max = y_max.max(yv);
                    }
                }
            }
            let span = y_max - y_min;
            let padding = if span.abs() < 1e-15 { 1.0 } else { span * 0.05 };
            UnitRange {
                y_min: y_min - padding,
                y_max: y_max + padding,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// GPU-accelerated plot rendering
// ---------------------------------------------------------------------------

fn show_gpu_plot(graph: &mut GraphState, ui: &mut egui::Ui, theme: &Theme, plot_area_height: f32) {
    if graph.series.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("No data loaded").strong().size(16.0));
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Click \"Add Data\" above to import a CSV or Excel file, or drag-and-drop a file.").weak());
        });
        ui.add_space(40.0);
        return;
    }

    let unit_order = collect_unit_order(graph);
    let multi_unit = unit_order.len() > 1;
    let unit_ranges = if multi_unit {
        compute_unit_ranges(graph, &unit_order)
    } else {
        Vec::new()
    };
    let is_datetime = graph.x_axis_is_datetime == Some(true);

    // --- Auto-fit on first display or when requested ---
    if graph.plot_view.auto_fit || !graph.plot_view.initialized {
        if multi_unit {
            graph.plot_view.fit_to_data_normalized(&graph.series);
        } else {
            graph.plot_view.fit_to_data(&graph.series);
        }
        graph.plot_view.auto_fit = false;
    } else if graph.auto_scale_y {
        // Continuously fit Y axis to data visible within current X range
        if multi_unit {
            graph.plot_view.auto_scale_y_normalized();
        } else {
            graph.plot_view.auto_scale_y_to_visible(&graph.series);
        }
    }

    // --- Layout: left margin for Y axis, main plot area, right margin for labels ---
    let left_margin = 70.0_f32;
    let right_margin = if multi_unit { 70.0_f32 * (unit_order.len() as f32 - 1.0).max(0.0) + 70.0 } else { 20.0 };
    let bottom_margin = 40.0_f32;
    let top_margin = 10.0_f32;
    let plot_height = (plot_area_height - bottom_margin - top_margin).max(100.0);

    let total_height = plot_height + bottom_margin + top_margin;
    let available_width = ui.available_width();
    let total_rect = ui.allocate_space(egui::Vec2::new(available_width, total_height)).1;

    let plot_rect = egui::Rect::from_min_max(
        egui::Pos2::new(total_rect.left() + left_margin, total_rect.top() + top_margin),
        egui::Pos2::new(total_rect.right() - right_margin, total_rect.bottom() - bottom_margin),
    );

    // --- Handle mouse interaction (use interact to avoid double layout allocation) ---
    let plot_id = egui::Id::new("gpu_plot").with(graph.id);
    let response = ui.interact(plot_rect, plot_id, egui::Sense::click_and_drag());
    graph.plot_view.handle_input(&response, plot_rect);

    let painter = ui.painter_at(total_rect);

    // --- Draw plot background ---
    painter.rect_filled(plot_rect, 0.0, theme.plot_bg());

    // --- Build grid data ---
    let grid_color_c32 = theme.grid_color();
    let grid_color = [
        grid_color_c32.r() as f32 / 255.0,
        grid_color_c32.g() as f32 / 255.0,
        grid_color_c32.b() as f32 / 255.0,
        grid_color_c32.a() as f32 / 255.0,
    ];

    let pv = &graph.plot_view;
    let x_grid = plot_interaction::compute_grid_lines(pv.x_min, pv.x_max);
    let y_grid = plot_interaction::compute_grid_lines(pv.y_min, pv.y_max);

    // Offset all coordinates by the view origin before f32 conversion.
    // Large absolute values (e.g. Unix timestamps ~1.77e9) lose precision
    // in f32 (only ~7 significant digits). By subtracting the view origin
    // first, all f32 values are small relative offsets with full precision.
    let x_off = pv.x_min;
    let y_off = pv.y_min;

    let mut grid_segments: Vec<[f32; 2]> = Vec::new();
    // Vertical grid lines
    for &(xval, is_major) in &x_grid {
        if is_major {
            let x = (xval - x_off) as f32;
            grid_segments.push([x, (pv.y_min - y_off) as f32]);
            grid_segments.push([x, (pv.y_max - y_off) as f32]);
        }
    }
    // Horizontal grid lines
    for &(yval, is_major) in &y_grid {
        if is_major {
            let y = (yval - y_off) as f32;
            grid_segments.push([(pv.x_min - x_off) as f32, y]);
            grid_segments.push([(pv.x_max - x_off) as f32, y]);
        }
    }

    let grid_data = GridGpuData {
        segments: grid_segments,
        color: grid_color,
        line_width: 1.0,
    };

    // --- Build series GPU data ---
    let mut series_data: Vec<SeriesGpuData> = Vec::new();

    for series in &graph.series {
        if !series.visible || series.x.is_empty() {
            continue;
        }

        // Downsample for view (skip if small enough)
        let (plot_x, plot_y) = if series.x.len() > 10000 {
            downsampling::downsample_for_view(
                &series.x, &series.y, pv.x_min, pv.x_max, 10000,
            )
        } else {
            // Borrow directly via slices rather than cloning
            (series.x.clone(), series.y.clone())
        };

        // Apply multi-unit normalization if needed
        let plot_y = if multi_unit {
            if let Some(idx) = unit_order.iter().position(|u| u == &series.unit) {
                let range = &unit_ranges[idx];
                plot_y.iter().map(|&y| range.normalize(y)).collect::<Vec<_>>()
            } else {
                plot_y
            }
        } else {
            plot_y
        };

        // Build points array (offset by view origin for f32 precision)
        let points: Vec<[f32; 2]> = plot_x
            .iter()
            .zip(plot_y.iter())
            .filter(|(x, y)| x.is_finite() && y.is_finite())
            .map(|(&x, &y)| [(x - x_off) as f32, (y - y_off) as f32])
            .collect();

        let c = series.color;
        let color = [
            c[0] as f32 / 255.0,
            c[1] as f32 / 255.0,
            c[2] as f32 / 255.0,
            c[3] as f32 / 255.0,
        ];

        let draw_mode = match series.interpolation {
            InterpolationMode::Linear => DrawMode::Lines,
            InterpolationMode::Step => DrawMode::Step,
            InterpolationMode::Points => DrawMode::Points,
        };

        series_data.push(SeriesGpuData {
            points: points.clone(),
            color,
            line_width: series.line_width,
            point_radius: series.line_width + 1.0,
            draw_mode,
        });

        // Additional dots layer when show_dots is enabled (and not already Points mode)
        if series.show_dots
            && series.interpolation != InterpolationMode::Points
            && points.len() < 10000
        {
            series_data.push(SeriesGpuData {
                points,
                color,
                line_width: series.line_width,
                point_radius: series.line_width + 0.5,
                draw_mode: DrawMode::Points,
            });
        }
    }

    // --- Build uniforms (offset by view origin, matching data points) ---
    let uniforms_base = PlotUniforms {
        view_min: [(pv.x_min - x_off) as f32, (pv.y_min - y_off) as f32],
        view_max: [(pv.x_max - x_off) as f32, (pv.y_max - y_off) as f32],
        resolution: [plot_rect.width(), plot_rect.height()],
        line_width: 2.0,
        point_radius: 3.0,
        color: [1.0, 1.0, 1.0, 1.0],
        _padding: [0.0; 4],
    };

    // --- Issue GPU paint callback ---
    let paint_cb = create_plot_paint_callback(plot_rect, series_data, grid_data, uniforms_base);
    painter.add(paint_cb);

    // --- Draw axis labels, ticks, and legend with egui overlays ---
    draw_axes_and_labels(
        &painter, graph, plot_rect, total_rect,
        &x_grid, &y_grid, is_datetime,
        multi_unit, &unit_order, &unit_ranges,
    );

    draw_legend(&painter, graph, plot_rect);

    // --- Cursor interaction and rendering ---
    if graph.cursor_state.mode != CursorMode::Off {
        handle_cursor_click(graph, &response, plot_rect);
        draw_cursors(
            &painter, graph, plot_rect, is_datetime,
            multi_unit, &unit_order, &unit_ranges,
        );
    }

    // --- Hover tooltip ---
    if response.hovered() && graph.cursor_state.mode == CursorMode::Off {
        if let Some(mouse_pos) = response.hover_pos() {
            draw_hover_tooltip(
                &painter, graph, plot_rect, mouse_pos,
                multi_unit, &unit_order, &unit_ranges,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 3D GPU-accelerated plot rendering
// ---------------------------------------------------------------------------

fn show_3d_plot(graph: &mut GraphState, ui: &mut egui::Ui, theme: &Theme, plot_area_height: f32) {
    if graph.series.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("No data loaded").strong().size(16.0));
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Click \"Add Data\" above to import a CSV or Excel file, or drag-and-drop a file.").weak());
        });
        ui.add_space(40.0);
        return;
    }

    let plot_height = plot_area_height.max(150.0);
    let available_width = ui.available_width();
    let total_rect = ui.allocate_space(egui::Vec2::new(available_width, plot_height)).1;

    // Handle mouse interaction for camera
    let plot_id = egui::Id::new("gpu_plot_3d").with(graph.id);
    let response = ui.interact(total_rect, plot_id, egui::Sense::click_and_drag());
    graph.camera.handle_input(&response);

    let painter = ui.painter_at(total_rect);
    let aspect = total_rect.width() / total_rect.height();

    // --- Normalize data to [-1, 1]^3 to avoid float precision issues ---
    let (data_min, data_max) = compute_data_bounds_3d(&graph.series);

    // Build camera uniforms
    let uniforms_base = graph.camera.uniforms(aspect);

    // --- Build 3D grid (box wireframe + axis lines) ---
    let grid_color_c32 = theme.grid_color();
    let grid_color = [
        grid_color_c32.r() as f32 / 255.0,
        grid_color_c32.g() as f32 / 255.0,
        grid_color_c32.b() as f32 / 255.0,
        grid_color_c32.a() as f32 / 255.0 * 0.5,
    ];

    let grid_lines = build_3d_grid();
    let grid_data = Line3DData {
        segments: grid_lines,
        color: grid_color,
        line_width: 1.0,
    };

    // --- Build scatter and line data ---
    let mut scatter_data: Vec<Scatter3DData> = Vec::new();
    let mut line_data: Vec<Line3DData> = vec![grid_data]; // grid first

    for series in &graph.series {
        if !series.visible {
            continue;
        }

        let c = series.color;
        let color = [
            c[0] as f32 / 255.0,
            c[1] as f32 / 255.0,
            c[2] as f32 / 255.0,
            c[3] as f32 / 255.0,
        ];

        if series.has_z() {
            // Full 3D series with X, Y, Z data
            let positions: Vec<[f32; 4]> = series.x.iter()
                .zip(series.y.iter())
                .zip(series.z.iter())
                .filter(|((x, y), z)| x.is_finite() && y.is_finite() && z.is_finite())
                .map(|((x, y), z)| normalize_point(*x, *y, *z, data_min, data_max))
                .collect();

            if positions.is_empty() {
                continue;
            }

            // Add scatter points
            scatter_data.push(Scatter3DData {
                positions: positions.clone(),
                color,
                point_size: 4.0,
            });

            // Add connecting lines if using linear interpolation
            if series.interpolation == InterpolationMode::Linear && positions.len() >= 2 {
                let mut segments: Vec<[f32; 4]> = Vec::with_capacity((positions.len() - 1) * 2);
                for i in 0..positions.len() - 1 {
                    segments.push(positions[i]);
                    segments.push(positions[i + 1]);
                }
                line_data.push(Line3DData {
                    segments,
                    color,
                    line_width: 1.5,
                });
            }
        } else {
            // 2D series in 3D space (Z=0)
            let positions: Vec<[f32; 4]> = series.x.iter()
                .zip(series.y.iter())
                .filter(|(x, y)| x.is_finite() && y.is_finite())
                .map(|(x, y)| normalize_point(*x, *y, 0.0, data_min, data_max))
                .collect();

            if positions.is_empty() {
                continue;
            }

            scatter_data.push(Scatter3DData {
                positions: positions.clone(),
                color,
                point_size: 3.0,
            });

            if series.interpolation == InterpolationMode::Linear && positions.len() >= 2 {
                let mut segments: Vec<[f32; 4]> = Vec::with_capacity((positions.len() - 1) * 2);
                for i in 0..positions.len() - 1 {
                    segments.push(positions[i]);
                    segments.push(positions[i + 1]);
                }
                line_data.push(Line3DData {
                    segments,
                    color,
                    line_width: 1.5,
                });
            }
        }
    }

    // --- Background color ---
    let bg_c = theme.plot_bg();
    let bg_color = [
        bg_c.r() as f32 / 255.0,
        bg_c.g() as f32 / 255.0,
        bg_c.b() as f32 / 255.0,
        1.0,
    ];

    // --- Viewport size in physical pixels ---
    let ppp = ui.ctx().pixels_per_point();
    let viewport_size = [
        (total_rect.width() * ppp) as u32,
        (total_rect.height() * ppp) as u32,
    ];

    // --- Issue 3D GPU paint callback ---
    let paint_cb = create_3d_paint_callback(
        total_rect,
        scatter_data,
        line_data,
        uniforms_base,
        bg_color,
        viewport_size,
    );
    painter.add(paint_cb);

    // --- Draw 3D axis labels as egui overlay ---
    draw_3d_axis_labels(&painter, graph, total_rect, data_min, data_max, aspect);

    // --- Draw legend ---
    draw_legend(&painter, graph, total_rect);
}

/// Compute axis-aligned bounding box of all visible 3D series data.
fn compute_data_bounds_3d(series: &[crate::state::data_series::DataSeries]) -> ([f64; 3], [f64; 3]) {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];

    for s in series {
        if !s.visible {
            continue;
        }
        for &xv in &s.x {
            if xv.is_finite() {
                min[0] = min[0].min(xv);
                max[0] = max[0].max(xv);
            }
        }
        for &yv in &s.y {
            if yv.is_finite() {
                min[1] = min[1].min(yv);
                max[1] = max[1].max(yv);
            }
        }
        if s.has_z() {
            for &zv in &s.z {
                if zv.is_finite() {
                    min[2] = min[2].min(zv);
                    max[2] = max[2].max(zv);
                }
            }
        }
    }

    // If no data found, default to unit cube
    for i in 0..3 {
        if !min[i].is_finite() || !max[i].is_finite() {
            min[i] = -1.0;
            max[i] = 1.0;
        }
        // Avoid zero-range axes
        if (max[i] - min[i]).abs() < 1e-12 {
            min[i] -= 0.5;
            max[i] += 0.5;
        }
    }

    (min, max)
}

/// Map a data point to normalized [-1, 1]^3 space.
fn normalize_point(x: f64, y: f64, z: f64, data_min: [f64; 3], data_max: [f64; 3]) -> [f32; 4] {
    let nx = ((x - data_min[0]) / (data_max[0] - data_min[0]) * 2.0 - 1.0) as f32;
    let ny = ((y - data_min[1]) / (data_max[1] - data_min[1]) * 2.0 - 1.0) as f32;
    let nz = ((z - data_min[2]) / (data_max[2] - data_min[2]) * 2.0 - 1.0) as f32;
    [nx, ny, nz, 1.0]
}

/// Build a wireframe grid box in normalized [-1, 1]^3 space.
fn build_3d_grid() -> Vec<[f32; 4]> {
    let mut segments: Vec<[f32; 4]> = Vec::new();

    // 12 edges of the unit cube
    let corners: [[f32; 3]; 8] = [
        [-1.0, -1.0, -1.0], [1.0, -1.0, -1.0],
        [1.0, 1.0, -1.0],   [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],  [1.0, -1.0, 1.0],
        [1.0, 1.0, 1.0],    [-1.0, 1.0, 1.0],
    ];
    let edges: [(usize, usize); 12] = [
        (0,1),(1,2),(2,3),(3,0), // back face
        (4,5),(5,6),(6,7),(7,4), // front face
        (0,4),(1,5),(2,6),(3,7), // connecting edges
    ];

    for (a, b) in edges {
        segments.push([corners[a][0], corners[a][1], corners[a][2], 1.0]);
        segments.push([corners[b][0], corners[b][1], corners[b][2], 1.0]);
    }

    // Interior grid lines (5 divisions per axis)
    for i in 1..5 {
        let t = -1.0 + (i as f32) / 5.0 * 2.0;

        // X-parallel lines on bottom face (Y=-1)
        segments.push([t, -1.0, -1.0, 1.0]);
        segments.push([t, -1.0, 1.0, 1.0]);

        // Z-parallel lines on bottom face
        segments.push([-1.0, -1.0, t, 1.0]);
        segments.push([1.0, -1.0, t, 1.0]);

        // Y-parallel lines on back face (Z=-1)
        segments.push([t, -1.0, -1.0, 1.0]);
        segments.push([t, 1.0, -1.0, 1.0]);

        // Y ticks on left edge
        segments.push([-1.0, t, -1.0, 1.0]);
        segments.push([-1.0, t, 1.0, 1.0]);
    }

    segments
}

/// Draw 3D axis labels by projecting corner positions to screen space.
fn draw_3d_axis_labels(
    painter: &egui::Painter,
    graph: &GraphState,
    plot_rect: egui::Rect,
    data_min: [f64; 3],
    data_max: [f64; 3],
    aspect: f32,
) {
    let text_color = painter.ctx().style().visuals.text_color();
    let dim_color = text_color.gamma_multiply(0.7);
    let font = egui::FontId::proportional(10.0);
    let label_font = egui::FontId::proportional(12.0);

    let vp = graph.camera.view_projection(aspect);

    // Helper: project 3D point to screen pos
    let project = |x: f32, y: f32, z: f32| -> Option<egui::Pos2> {
        let clip = vp * glam::Vec4::new(x, y, z, 1.0);
        if clip.w <= 0.0 {
            return None; // behind camera
        }
        let ndc = glam::Vec2::new(clip.x / clip.w, clip.y / clip.w);
        let screen_x = plot_rect.left() + (ndc.x * 0.5 + 0.5) * plot_rect.width();
        let screen_y = plot_rect.top() + (-ndc.y * 0.5 + 0.5) * plot_rect.height();
        Some(egui::Pos2::new(screen_x, screen_y))
    };

    // Axis labels at midpoints of edges
    let axis_labels = [
        ("X", [0.0, -1.15, -1.15]),
        ("Y", [-1.15, 0.0, -1.15]),
        ("Z", [-1.15, -1.15, 0.0]),
    ];
    for (label, pos) in axis_labels {
        if let Some(screen_pos) = project(pos[0], pos[1], pos[2]) {
            if plot_rect.contains(screen_pos) {
                painter.text(screen_pos, egui::Align2::CENTER_CENTER, label, label_font.clone(), text_color);
            }
        }
    }

    // Tick labels along each axis
    for i in 0..=4 {
        let t = i as f32 / 4.0;
        let ndc_val = -1.0 + t * 2.0;

        // X ticks along bottom-back edge
        let x_data = data_min[0] + t as f64 * (data_max[0] - data_min[0]);
        if let Some(pos) = project(ndc_val, -1.0, -1.2) {
            if plot_rect.contains(pos) {
                painter.text(pos, egui::Align2::CENTER_TOP, plot_interaction::format_tick_value(x_data), font.clone(), dim_color);
            }
        }

        // Y ticks along left-back edge
        let y_data = data_min[1] + t as f64 * (data_max[1] - data_min[1]);
        if let Some(pos) = project(-1.2, ndc_val, -1.0) {
            if plot_rect.contains(pos) {
                painter.text(pos, egui::Align2::RIGHT_CENTER, plot_interaction::format_tick_value(y_data), font.clone(), dim_color);
            }
        }

        // Z ticks along bottom-left edge
        let z_data = data_min[2] + t as f64 * (data_max[2] - data_min[2]);
        if let Some(pos) = project(-1.2, -1.0, ndc_val) {
            if plot_rect.contains(pos) {
                painter.text(pos, egui::Align2::RIGHT_CENTER, plot_interaction::format_tick_value(z_data), font.clone(), dim_color);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Axis labels, tick marks, grid labels
// ---------------------------------------------------------------------------

fn draw_axes_and_labels(
    painter: &egui::Painter,
    graph: &GraphState,
    plot_rect: egui::Rect,
    total_rect: egui::Rect,
    x_grid: &[(f64, bool)],
    y_grid: &[(f64, bool)],
    is_datetime: bool,
    multi_unit: bool,
    unit_order: &[String],
    unit_ranges: &[UnitRange],
) {
    let pv = &graph.plot_view;
    let text_color = painter.ctx().style().visuals.text_color();
    let dim_color = text_color.gamma_multiply(0.6);

    // --- Plot border ---
    painter.rect_stroke(plot_rect, 0.0, egui::Stroke::new(1.0, dim_color), egui::StrokeKind::Outside);

    // --- X-axis tick labels ---
    for &(xval, is_major) in x_grid {
        if !is_major {
            continue;
        }
        let screen_x = pv.data_to_screen(xval, pv.y_min, plot_rect).x;
        if screen_x < plot_rect.left() || screen_x > plot_rect.right() {
            continue;
        }

        let label = if is_datetime {
            datetime::format_timestamp(xval)
        } else {
            plot_interaction::format_tick_value(xval)
        };

        painter.text(
            egui::Pos2::new(screen_x, plot_rect.bottom() + 4.0),
            egui::Align2::CENTER_TOP,
            label,
            egui::FontId::proportional(10.0),
            dim_color,
        );
    }

    // --- X-axis label ---
    let x_label = if is_datetime {
        "Date and Time".to_string()
    } else {
        match (&graph.x_axis_name, &graph.x_axis_unit) {
            (Some(name), Some(unit)) => format!("{name} ({unit})"),
            (Some(name), None) => name.clone(),
            (None, Some(unit)) => format!("X Axis ({unit})"),
            (None, None) => "X Axis".to_string(),
        }
    };
    painter.text(
        egui::Pos2::new(plot_rect.center().x, total_rect.bottom() - 4.0),
        egui::Align2::CENTER_BOTTOM,
        x_label,
        egui::FontId::proportional(12.0),
        text_color,
    );

    // --- Y-axis tick labels ---
    if multi_unit {
        // In multi-unit mode, draw labels for each unit on alternating sides
        for (i, unit) in unit_order.iter().enumerate() {
            let range = &unit_ranges[i];
            for &(yval, is_major) in y_grid {
                if !is_major {
                    continue;
                }
                let screen_y = pv.data_to_screen(pv.x_min, yval, plot_rect).y;
                if screen_y < plot_rect.top() || screen_y > plot_rect.bottom() {
                    continue;
                }

                let real_val = range.denormalize(yval);
                let label = plot_interaction::format_tick_value(real_val);

                if i == 0 {
                    painter.text(
                        egui::Pos2::new(plot_rect.left() - 4.0, screen_y),
                        egui::Align2::RIGHT_CENTER,
                        label,
                        egui::FontId::proportional(10.0),
                        dim_color,
                    );
                } else {
                    let x_offset = plot_rect.right() + 4.0 + (i as f32 - 1.0) * 70.0;
                    painter.text(
                        egui::Pos2::new(x_offset, screen_y),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::proportional(10.0),
                        dim_color,
                    );
                }
            }

            // Unit axis label
            let label_text = format!("Y ({unit})");
            if i == 0 {
                // Left side - rotated text not easily available, use vertical position
                painter.text(
                    egui::Pos2::new(total_rect.left() + 2.0, plot_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &label_text,
                    egui::FontId::proportional(11.0),
                    text_color,
                );
            } else {
                let x_offset = plot_rect.right() + 4.0 + (i as f32 - 1.0) * 70.0 + 50.0;
                painter.text(
                    egui::Pos2::new(x_offset, plot_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &label_text,
                    egui::FontId::proportional(11.0),
                    text_color,
                );
            }
        }
    } else {
        // Single unit Y-axis
        for &(yval, is_major) in y_grid {
            if !is_major {
                continue;
            }
            let screen_y = pv.data_to_screen(pv.x_min, yval, plot_rect).y;
            if screen_y < plot_rect.top() || screen_y > plot_rect.bottom() {
                continue;
            }
            let label = plot_interaction::format_tick_value(yval);
            painter.text(
                egui::Pos2::new(plot_rect.left() - 4.0, screen_y),
                egui::Align2::RIGHT_CENTER,
                label,
                egui::FontId::proportional(10.0),
                dim_color,
            );
        }

        let y_label = unit_order
            .first()
            .map(|u| format!("Y Axis ({u})"))
            .unwrap_or_else(|| "Y Axis".to_string());
        painter.text(
            egui::Pos2::new(total_rect.left() + 2.0, plot_rect.center().y),
            egui::Align2::LEFT_CENTER,
            y_label,
            egui::FontId::proportional(11.0),
            text_color,
        );
    }
}

// ---------------------------------------------------------------------------
// Legend
// ---------------------------------------------------------------------------

fn draw_legend(painter: &egui::Painter, graph: &GraphState, plot_rect: egui::Rect) {
    if graph.series.is_empty() {
        return;
    }

    let text_color = painter.ctx().style().visuals.text_color();
    let bg_color = painter.ctx().style().visuals.window_fill;
    let mut y = plot_rect.top() + 8.0;
    let x = plot_rect.right() - 8.0;

    // Compute legend width for background
    let font = egui::FontId::proportional(11.0);
    let max_width = graph
        .series
        .iter()
        .filter(|s| s.visible)
        .map(|s| {
            painter
                .layout_no_wrap(s.label.clone(), font.clone(), text_color)
                .rect
                .width()
        })
        .fold(0.0_f32, f32::max);

    let legend_width = max_width + 24.0; // color swatch + padding
    let visible_count = graph.series.iter().filter(|s| s.visible).count();
    let legend_height = visible_count as f32 * 16.0 + 8.0;

    let legend_rect = egui::Rect::from_min_size(
        egui::Pos2::new(x - legend_width - 4.0, y - 4.0),
        egui::Vec2::new(legend_width + 8.0, legend_height),
    );
    painter.rect_filled(legend_rect, 4.0, bg_color.gamma_multiply(0.85));
    painter.rect_stroke(legend_rect, 4.0, egui::Stroke::new(0.5, text_color.gamma_multiply(0.3)), egui::StrokeKind::Outside);

    for series in &graph.series {
        if !series.visible {
            continue;
        }
        let color = series.color32();

        // Color swatch
        let swatch_rect = egui::Rect::from_min_size(
            egui::Pos2::new(x - legend_width, y),
            egui::Vec2::new(12.0, 12.0),
        );
        painter.rect_filled(swatch_rect, 2.0, color);

        // Label
        painter.text(
            egui::Pos2::new(x - legend_width + 16.0, y + 6.0),
            egui::Align2::LEFT_CENTER,
            &series.label,
            font.clone(),
            text_color,
        );

        y += 16.0;
    }
}

// ---------------------------------------------------------------------------
// Hover tooltip
// ---------------------------------------------------------------------------

fn draw_hover_tooltip(
    painter: &egui::Painter,
    graph: &GraphState,
    plot_rect: egui::Rect,
    mouse_pos: egui::Pos2,
    multi_unit: bool,
    unit_order: &[String],
    unit_ranges: &[UnitRange],
) {
    let pv = &graph.plot_view;
    let (mouse_data_x, mouse_data_y) = pv.screen_to_data(mouse_pos, plot_rect);

    // Normalize mouse coordinates by axis range so distance is comparable
    let x_span = (pv.x_max - pv.x_min).max(1e-15);
    let y_span = (pv.y_max - pv.y_min).max(1e-15);

    let mut best_dist = f64::INFINITY;
    let mut best_label = String::new();
    let mut best_x = 0.0;
    let mut best_y_display = 0.0;
    let mut best_screen_pos = egui::Pos2::ZERO;
    let mut best_color = egui::Color32::WHITE;

    for series in &graph.series {
        if !series.visible || series.x.is_empty() {
            continue;
        }

        // Limit search to visible X range using binary search when sorted.
        let (search_start, search_end) = {
            let is_sorted = series.x.windows(2).take(20).all(|w| w[0] <= w[1]);
            if is_sorted && series.x.len() > 1000 {
                let s = series.x.partition_point(|&v| v < pv.x_min).saturating_sub(1);
                let e = (series.x.partition_point(|&v| v <= pv.x_max) + 1).min(series.x.len());
                (s, e)
            } else {
                (0, series.x.len())
            }
        };

        if multi_unit {
            let range_opt = unit_order
                .iter()
                .position(|u| u == &series.unit)
                .map(|idx| &unit_ranges[idx]);

            if let Some(range) = range_opt {
                for i in search_start..search_end {
                    let xv = series.x[i];
                    let yv_norm = range.normalize(series.y[i]);
                    if !xv.is_finite() || !yv_norm.is_finite() {
                        continue;
                    }
                    let dx = (xv - mouse_data_x) / x_span;
                    let dy = (yv_norm - mouse_data_y) / y_span;
                    let dist = dx * dx + dy * dy;
                    if dist < best_dist {
                        best_dist = dist;
                        best_label = series.label.clone();
                        best_x = xv;
                        best_y_display = series.y[i]; // real value for display
                        best_screen_pos = pv.data_to_screen(xv, yv_norm, plot_rect);
                        best_color = series.color32();
                    }
                }
            }
        } else {
            for i in search_start..search_end {
                let xv = series.x[i];
                let yv = series.y[i];
                if !xv.is_finite() || !yv.is_finite() {
                    continue;
                }
                let dx = (xv - mouse_data_x) / x_span;
                let dy = (yv - mouse_data_y) / y_span;
                let dist = dx * dx + dy * dy;
                if dist < best_dist {
                    best_dist = dist;
                    best_label = series.label.clone();
                    best_x = xv;
                    best_y_display = yv;
                    best_screen_pos = pv.data_to_screen(xv, yv, plot_rect);
                    best_color = series.color32();
                }
            }
        }
    }

    // Threshold: distance is already normalized by axis ranges, so use a fixed fraction.
    // 0.02^2 = 0.0004 means ~2% of the visible range in each axis.
    let threshold = 0.001;

    if best_dist < threshold && !best_label.is_empty() {
        let x_str = if graph.x_axis_is_datetime == Some(true) {
            datetime::format_timestamp(best_x)
        } else {
            format!("{best_x:.3}")
        };
        let text = format!("{best_label}: X={x_str}, Y={best_y_display:.3}");

        // Draw highlight dot
        painter.circle_filled(best_screen_pos, 5.0, best_color);
        painter.circle_stroke(best_screen_pos, 5.0, egui::Stroke::new(1.0, egui::Color32::WHITE));

        // Draw tooltip background + text
        let font = egui::FontId::proportional(11.0);
        let text_color = painter.ctx().style().visuals.text_color();
        let galley = painter.layout_no_wrap(text.clone(), font.clone(), text_color);
        let text_rect = galley.rect;
        let tooltip_pos = egui::Pos2::new(
            best_screen_pos.x + 10.0,
            best_screen_pos.y - text_rect.height() - 8.0,
        );
        let bg_rect = egui::Rect::from_min_size(
            egui::Pos2::new(tooltip_pos.x - 4.0, tooltip_pos.y - 2.0),
            egui::Vec2::new(text_rect.width() + 8.0, text_rect.height() + 4.0),
        );

        let bg_color = painter.ctx().style().visuals.window_fill;
        painter.rect_filled(bg_rect, 3.0, bg_color.gamma_multiply(0.9));
        painter.rect_stroke(bg_rect, 3.0, egui::Stroke::new(0.5, best_color), egui::StrokeKind::Outside);
        painter.text(tooltip_pos, egui::Align2::LEFT_TOP, text, font, best_color);
    }
}

// ---------------------------------------------------------------------------
// Measurement Cursors
// ---------------------------------------------------------------------------

fn handle_cursor_click(graph: &mut GraphState, response: &egui::Response, plot_rect: egui::Rect) {
    // Right-click places cursors when in cursor mode (left-click is pan)
    if response.secondary_clicked() {
        if let Some(mouse_pos) = response.interact_pointer_pos() {
            let pv = &graph.plot_view;
            let (data_x, data_y) = pv.screen_to_data(mouse_pos, plot_rect);

            let val = match graph.cursor_state.mode {
                CursorMode::Vertical => data_x,
                CursorMode::Horizontal => data_y,
                CursorMode::Off => return,
            };

            if graph.cursor_state.cursor1.is_none() {
                graph.cursor_state.cursor1 = Some(val);
            } else if graph.cursor_state.cursor2.is_none() {
                graph.cursor_state.cursor2 = Some(val);
            } else {
                // Both set: reset and place new first cursor
                graph.cursor_state.cursor1 = Some(val);
                graph.cursor_state.cursor2 = None;
            }
        }
    }
}

fn draw_cursors(
    painter: &egui::Painter,
    graph: &GraphState,
    plot_rect: egui::Rect,
    is_datetime: bool,
    multi_unit: bool,
    unit_order: &[String],
    unit_ranges: &[UnitRange],
) {
    let pv = &graph.plot_view;
    let cursor_color = egui::Color32::from_rgb(255, 200, 0); // Yellow
    let stroke = egui::Stroke::new(1.5, cursor_color);
    let font = egui::FontId::proportional(11.0);
    let text_color = painter.ctx().style().visuals.text_color();

    match graph.cursor_state.mode {
        CursorMode::Vertical => {
            // Draw vertical cursor lines
            if let Some(x1) = graph.cursor_state.cursor1 {
                let screen_x = pv.data_to_screen(x1, pv.y_min, plot_rect).x;
                if screen_x >= plot_rect.left() && screen_x <= plot_rect.right() {
                    painter.line_segment(
                        [egui::Pos2::new(screen_x, plot_rect.top()),
                         egui::Pos2::new(screen_x, plot_rect.bottom())],
                        stroke,
                    );
                    let label = if is_datetime {
                        format!("C1: {}", crate::data::datetime::format_timestamp(x1))
                    } else {
                        format!("C1: {:.4}", x1)
                    };
                    painter.text(
                        egui::Pos2::new(screen_x + 4.0, plot_rect.top() + 4.0),
                        egui::Align2::LEFT_TOP, label, font.clone(), cursor_color,
                    );
                }
            }
            if let Some(x2) = graph.cursor_state.cursor2 {
                let screen_x = pv.data_to_screen(x2, pv.y_min, plot_rect).x;
                if screen_x >= plot_rect.left() && screen_x <= plot_rect.right() {
                    painter.line_segment(
                        [egui::Pos2::new(screen_x, plot_rect.top()),
                         egui::Pos2::new(screen_x, plot_rect.bottom())],
                        stroke,
                    );
                    let label = if is_datetime {
                        format!("C2: {}", crate::data::datetime::format_timestamp(x2))
                    } else {
                        format!("C2: {:.4}", x2)
                    };
                    painter.text(
                        egui::Pos2::new(screen_x + 4.0, plot_rect.top() + 18.0),
                        egui::Align2::LEFT_TOP, label, font.clone(), cursor_color,
                    );
                }
            }
            // Delta display
            if let (Some(x1), Some(x2)) = (graph.cursor_state.cursor1, graph.cursor_state.cursor2) {
                let delta = (x2 - x1).abs();
                let delta_label = if is_datetime {
                    format!("dX: {:.3}s", delta)
                } else {
                    format!("dX: {:.4}", delta)
                };
                painter.text(
                    egui::Pos2::new(plot_rect.left() + 8.0, plot_rect.bottom() - 20.0),
                    egui::Align2::LEFT_BOTTOM, delta_label, font, text_color,
                );
            }
        }
        CursorMode::Horizontal => {
            // Draw horizontal cursor lines with per-unit labels
            if let Some(y1) = graph.cursor_state.cursor1 {
                let screen_y = pv.data_to_screen(pv.x_min, y1, plot_rect).y;
                if screen_y >= plot_rect.top() && screen_y <= plot_rect.bottom() {
                    painter.line_segment(
                        [egui::Pos2::new(plot_rect.left(), screen_y),
                         egui::Pos2::new(plot_rect.right(), screen_y)],
                        stroke,
                    );
                    let label = if multi_unit {
                        let parts: Vec<String> = unit_order.iter().zip(unit_ranges.iter())
                            .map(|(u, r)| format!("{:.4} {u}", r.denormalize(y1)))
                            .collect();
                        format!("C1: {}", parts.join("  |  "))
                    } else {
                        format!("C1: {:.4}", y1)
                    };
                    painter.text(
                        egui::Pos2::new(plot_rect.left() + 4.0, screen_y - 14.0),
                        egui::Align2::LEFT_BOTTOM, label, font.clone(), cursor_color,
                    );
                }
            }
            if let Some(y2) = graph.cursor_state.cursor2 {
                let screen_y = pv.data_to_screen(pv.x_min, y2, plot_rect).y;
                if screen_y >= plot_rect.top() && screen_y <= plot_rect.bottom() {
                    painter.line_segment(
                        [egui::Pos2::new(plot_rect.left(), screen_y),
                         egui::Pos2::new(plot_rect.right(), screen_y)],
                        stroke,
                    );
                    let label = if multi_unit {
                        let parts: Vec<String> = unit_order.iter().zip(unit_ranges.iter())
                            .map(|(u, r)| format!("{:.4} {u}", r.denormalize(y2)))
                            .collect();
                        format!("C2: {}", parts.join("  |  "))
                    } else {
                        format!("C2: {:.4}", y2)
                    };
                    painter.text(
                        egui::Pos2::new(plot_rect.left() + 4.0, screen_y + 2.0),
                        egui::Align2::LEFT_TOP, label, font.clone(), cursor_color,
                    );
                }
            }
            // Delta display (per-unit when multi-unit)
            if let (Some(y1), Some(y2)) = (graph.cursor_state.cursor1, graph.cursor_state.cursor2) {
                if multi_unit {
                    let mut y_pos = plot_rect.bottom() - 20.0;
                    for (unit, range) in unit_order.iter().zip(unit_ranges.iter()) {
                        let real_y1 = range.denormalize(y1);
                        let real_y2 = range.denormalize(y2);
                        let delta = (real_y2 - real_y1).abs();
                        painter.text(
                            egui::Pos2::new(plot_rect.left() + 8.0, y_pos),
                            egui::Align2::LEFT_BOTTOM,
                            format!("dY ({unit}): {delta:.4}"), font.clone(), text_color,
                        );
                        y_pos -= 16.0;
                    }
                } else {
                    let delta = (y2 - y1).abs();
                    painter.text(
                        egui::Pos2::new(plot_rect.left() + 8.0, plot_rect.bottom() - 20.0),
                        egui::Align2::LEFT_BOTTOM,
                        format!("dY: {:.4}", delta), font, text_color,
                    );
                }
            }
        }
        CursorMode::Off => {}
    }
}

// ---------------------------------------------------------------------------
// Table view (unchanged from Phase 2)
// ---------------------------------------------------------------------------

fn cycle_sort(graph: &mut GraphState, col: usize) {
    graph.table_sort = match graph.table_sort {
        Some((c, SortDirection::Ascending)) if c == col => Some((col, SortDirection::Descending)),
        Some((c, SortDirection::Descending)) if c == col => None,
        _ => Some((col, SortDirection::Ascending)),
    };
}

fn get_table_value(graph: &GraphState, col: usize, row: usize) -> f64 {
    if col == 0 {
        graph.series.first()
            .and_then(|s| s.x.get(row))
            .copied()
            .unwrap_or(f64::NAN)
    } else {
        graph.series.get(col - 1)
            .and_then(|s| s.y.get(row))
            .copied()
            .unwrap_or(f64::NAN)
    }
}

fn show_table_view(graph: &mut GraphState, ui: &mut egui::Ui) {
    if graph.series.is_empty() {
        ui.label("No data loaded.");
        return;
    }

    use egui_extras::{Column, TableBuilder};

    let num_cols = graph.series.len() + 1;
    let row_count = graph.series.iter().map(|s| s.x.len()).max().unwrap_or(0);
    let is_datetime = graph.x_axis_is_datetime == Some(true);

    let sorted_indices: Vec<usize> = match graph.table_sort {
        Some((col, dir)) => {
            let mut indices: Vec<usize> = (0..row_count).collect();
            indices.sort_by(|&a, &b| {
                let val_a = get_table_value(graph, col, a);
                let val_b = get_table_value(graph, col, b);
                let cmp = val_a.partial_cmp(&val_b).unwrap_or(std::cmp::Ordering::Equal);
                match dir {
                    SortDirection::Ascending => cmp,
                    SortDirection::Descending => cmp.reverse(),
                }
            });
            indices
        }
        None => (0..row_count).collect(),
    };

    let x_header = graph
        .x_axis_name
        .clone()
        .unwrap_or_else(|| "X Axis".to_string());

    let series_labels: Vec<String> = graph.series.iter().map(|s| s.label.clone()).collect();
    let current_sort = graph.table_sort;

    let table = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .columns(Column::auto().at_least(100.0), num_cols)
        .min_scrolled_height(300.0);

    let clicked_col: std::cell::Cell<Option<usize>> = std::cell::Cell::new(None);

    table
        .header(20.0, |mut header| {
            header.col(|ui| {
                let arrow = match current_sort {
                    Some((0, SortDirection::Ascending)) => " ^",
                    Some((0, SortDirection::Descending)) => " v",
                    _ => "",
                };
                if ui.button(format!("{x_header}{arrow}")).clicked() {
                    clicked_col.set(Some(0));
                }
            });

            for (i, label) in series_labels.iter().enumerate() {
                let col_idx = i + 1;
                header.col(|ui| {
                    let arrow = match current_sort {
                        Some((c, SortDirection::Ascending)) if c == col_idx => " ^",
                        Some((c, SortDirection::Descending)) if c == col_idx => " v",
                        _ => "",
                    };
                    if ui.button(format!("{label}{arrow}")).clicked() {
                        clicked_col.set(Some(col_idx));
                    }
                });
            }
        })
        .body(|body| {
            body.rows(18.0, row_count, |mut row| {
                let sorted_row_idx = sorted_indices[row.index()];

                row.col(|ui| {
                    if let Some(first_series) = graph.series.first() {
                        if sorted_row_idx < first_series.x.len() {
                            let xv = first_series.x[sorted_row_idx];
                            if is_datetime {
                                ui.label(datetime::format_timestamp(xv));
                            } else {
                                ui.label(format!("{xv:.3}"));
                            }
                        }
                    }
                });

                for series in &graph.series {
                    row.col(|ui| {
                        if sorted_row_idx < series.y.len() {
                            let yv = series.y[sorted_row_idx];
                            if yv.is_finite() {
                                ui.label(format!("{yv:.3}"));
                            } else {
                                ui.label("-");
                            }
                        }
                    });
                }
            });
        });

    if let Some(col) = clicked_col.get() {
        cycle_sort(graph, col);
    }
}
