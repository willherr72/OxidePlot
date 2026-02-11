use crate::processing::math_ops::{MathOp, perform_math};
use crate::processing::statistics::SeriesStats;
use crate::state::data_series::{color_for_index, DataSeries, InterpolationMode};
use crate::state::graph_state::GraphState;

/// Persistent state for the settings dialog, created when the user opens
/// settings for a particular graph.
pub struct SettingsDialogState {
    pub graph_id: u64,
    pub selected_series: Vec<bool>,
    pub math_op: MathOp,
    pub math_error: String,
    pub unit_new_name: String,
    pub unit_factor: String,
    pub unit_bias: String,
    /// Cached stats report text. Recomputed only when selection changes.
    pub stats_cache: Option<(Vec<u64>, String)>,
}

impl SettingsDialogState {
    pub fn new(graph_id: u64, series_count: usize) -> Self {
        Self {
            graph_id,
            selected_series: vec![false; series_count],
            math_op: MathOp::Add,
            math_error: String::new(),
            unit_new_name: String::new(),
            unit_factor: "1.0".to_string(),
            unit_bias: "0.0".to_string(),
            stats_cache: None,
        }
    }
}

/// Show the settings dialog window. Returns `true` while it should stay open,
/// `false` when the user closes it.
pub fn show_settings_dialog(
    ctx: &egui::Context,
    state: &mut SettingsDialogState,
    graph: &mut GraphState,
) -> bool {
    let mut open = true;

    egui::Window::new("Graph Settings")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(600.0)
        .default_height(500.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                // ============================================================
                // SECTION 1: Series list with inline controls
                // ============================================================
                ui.label(egui::RichText::new("Data Series").strong().size(15.0));
                ui.add_space(4.0);

                if graph.series.is_empty() {
                    ui.label(egui::RichText::new("No data series loaded.").weak());
                } else {
                    // Compute name field width from longest label
                    let max_name_width = graph
                        .series
                        .iter()
                        .map(|s| {
                            let galley = ui.painter().layout_no_wrap(
                                s.label.clone(),
                                egui::FontId::proportional(14.0),
                                egui::Color32::WHITE,
                            );
                            galley.rect.width()
                        })
                        .fold(60.0f32, f32::max);
                    let name_width = (max_name_width + 24.0).max(100.0).min(300.0);

                    let mut series_swap: Option<(usize, usize)> = None;

                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("series_scroll")
                                .max_height(300.0)
                                .show(ui, |ui| {
                                    let series_count = graph.series.len();
                                    for (i, series) in graph.series.iter_mut().enumerate() {
                                        ui.horizontal(|ui| {
                                            // Z-order up/down buttons
                                            ui.vertical(|ui| {
                                                ui.spacing_mut().item_spacing.y = 0.0;
                                                let up_btn = ui.add_enabled(
                                                    i > 0,
                                                    egui::Button::new(
                                                        egui::RichText::new("^").size(9.0)
                                                    ).frame(false).min_size(egui::vec2(16.0, 12.0)),
                                                ).on_hover_text("Move up (draw earlier)");
                                                if up_btn.clicked() {
                                                    series_swap = Some((i, i - 1));
                                                }
                                                let down_btn = ui.add_enabled(
                                                    i < series_count - 1,
                                                    egui::Button::new(
                                                        egui::RichText::new("v").size(9.0)
                                                    ).frame(false).min_size(egui::vec2(16.0, 12.0)),
                                                ).on_hover_text("Move down (draw later)");
                                                if down_btn.clicked() {
                                                    series_swap = Some((i, i + 1));
                                                }
                                            });

                                            // Color picker swatch
                                            ui.color_edit_button_srgba_unmultiplied(
                                                &mut series.color,
                                            );

                                            // Visibility toggle
                                            ui.checkbox(&mut series.visible, "")
                                                .on_hover_text("Show/hide series");

                                            // Editable series name (auto-sized)
                                            ui.add(
                                                egui::TextEdit::singleline(&mut series.label)
                                                    .desired_width(name_width),
                                            );

                                            ui.separator();

                                            // Dots toggle
                                            ui.checkbox(&mut series.show_dots, "Dots")
                                                .on_hover_text("Show data point markers");

                                            // Interpolation mode combo
                                            egui::ComboBox::from_id_salt(format!(
                                                "interp_{}",
                                                series.id
                                            ))
                                            .selected_text(series.interpolation.label())
                                            .width(80.0)
                                            .show_ui(ui, |ui| {
                                                for mode in [
                                                    InterpolationMode::Linear,
                                                    InterpolationMode::Step,
                                                    InterpolationMode::Points,
                                                ] {
                                                    if ui
                                                        .selectable_value(
                                                            &mut series.interpolation,
                                                            mode,
                                                            mode.label(),
                                                        )
                                                        .clicked()
                                                    {
                                                        if mode == InterpolationMode::Points {
                                                            series.show_dots = true;
                                                        }
                                                    }
                                                }
                                            });

                                            // Line width drag
                                            ui.add(
                                                egui::DragValue::new(&mut series.line_width)
                                                    .range(0.5..=8.0)
                                                    .speed(0.1)
                                                    .suffix(" px"),
                                            );
                                        });

                                        // Subtle separator between series
                                        if i < series_count - 1 {
                                            ui.separator();
                                        }
                                    }
                                });
                        });

                    // Apply series reorder after the UI is done
                    if let Some((a, b)) = series_swap {
                        graph.series.swap(a, b);
                        // Keep the selection checkboxes in sync
                        if a < state.selected_series.len() && b < state.selected_series.len() {
                            state.selected_series.swap(a, b);
                        }
                    }
                }

                ui.add_space(12.0);

                // ============================================================
                // SECTION 2: Advanced Tools (selection-based)
                // ============================================================
                ui.label(egui::RichText::new("Advanced Tools").strong().size(15.0));
                ui.add_space(4.0);

                if graph.series.is_empty() {
                    ui.label(egui::RichText::new("No series to operate on.").weak());
                } else {
                    // Keep selection vector in sync
                    if state.selected_series.len() != graph.series.len() {
                        state.selected_series.resize(graph.series.len(), false);
                    }

                    // Inline selection bar
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(6))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new("Select series for tools below:").weak(),
                            );
                            ui.add_space(2.0);
                            ui.horizontal_wrapped(|ui| {
                                for (i, series) in graph.series.iter().enumerate() {
                                    let color = series.color32();
                                    ui.checkbox(
                                        &mut state.selected_series[i],
                                        egui::RichText::new(&series.label).color(color),
                                    );
                                }
                            });
                        });

                    ui.add_space(4.0);

                    // --- Unit Conversion ---
                    egui::CollapsingHeader::new("Unit Conversion")
                        .id_salt("unit_section")
                        .default_open(false)
                        .show(ui, |ui| {
                            show_unit_panel(ui, state, graph);
                        });

                    // --- Statistics ---
                    egui::CollapsingHeader::new("Statistics")
                        .id_salt("stats_section")
                        .default_open(false)
                        .show(ui, |ui| {
                            show_stats_panel(ui, state, graph);
                        });

                    // --- Math Operations ---
                    egui::CollapsingHeader::new("Math Operations")
                        .id_salt("math_section")
                        .default_open(false)
                        .show(ui, |ui| {
                            show_math_panel(ui, state, graph);
                        });

                }
            });
        });

    open
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn get_selected_ids(state: &SettingsDialogState, graph: &GraphState) -> Vec<u64> {
    state
        .selected_series
        .iter()
        .enumerate()
        .filter(|(_, &selected)| selected)
        .filter_map(|(i, _)| graph.series.get(i).map(|s| s.id))
        .collect()
}

// ---------------------------------------------------------------------------
// Tool panels (shown inside collapsible headers)
// ---------------------------------------------------------------------------

fn show_stats_panel(
    ui: &mut egui::Ui,
    state: &mut SettingsDialogState,
    graph: &GraphState,
) {
    let ids = get_selected_ids(state, graph);
    if ids.is_empty() {
        ui.label(egui::RichText::new("Select series above first.").weak());
        state.stats_cache = None;
        return;
    }

    // Only recompute when the selected series change
    let need_recompute = match &state.stats_cache {
        Some((cached_ids, _)) => *cached_ids != ids,
        None => true,
    };

    if need_recompute {
        let mut report = String::new();
        for series in &graph.series {
            if ids.contains(&series.id) {
                if let Some(stats) = SeriesStats::compute(&series.y) {
                    report.push_str(&stats.report(&series.label));
                    report.push('\n');
                } else {
                    report.push_str(&format!(
                        "{}:\n  No valid numeric data.\n\n",
                        series.label
                    ));
                }
            }
        }
        state.stats_cache = Some((ids, report));
    }

    let report = &state.stats_cache.as_ref().unwrap().1;

    if report.is_empty() {
        ui.label(egui::RichText::new("No statistics available.").weak());
    } else {
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.monospace(report);
            });
    }
}

fn show_math_panel(
    ui: &mut egui::Ui,
    state: &mut SettingsDialogState,
    graph: &mut GraphState,
) {
    let selected_ids = get_selected_ids(state, graph);
    if selected_ids.len() != 2 {
        ui.label(
            egui::RichText::new("Select exactly 2 series above to perform math operations.")
                .weak(),
        );
        return;
    }

    ui.label("Choose operation:");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        for op in [MathOp::Add, MathOp::Subtract, MathOp::Multiply, MathOp::Divide] {
            ui.radio_value(&mut state.math_op, op, op.label());
        }
    });

    ui.add_space(4.0);

    if !state.math_error.is_empty() {
        ui.colored_label(egui::Color32::from_rgb(255, 80, 80), &state.math_error);
        ui.add_space(4.0);
    }

    if ui
        .add(egui::Button::new("Apply Math").min_size(egui::vec2(120.0, 28.0)))
        .clicked()
    {
        let s1 = graph
            .series
            .iter()
            .find(|s| s.id == selected_ids[0])
            .cloned();
        let s2 = graph
            .series
            .iter()
            .find(|s| s.id == selected_ids[1])
            .cloned();

        if let (Some(s1), Some(s2)) = (s1, s2) {
            match perform_math(&s1.x, &s1.y, &s2.x, &s2.y, state.math_op, 1e-5) {
                Ok(math_result) => {
                    let unit = if s1.unit == s2.unit {
                        s1.unit.clone()
                    } else {
                        "units".to_string()
                    };
                    let title = format!(
                        "{} {} {}",
                        s1.label,
                        state.math_op.symbol(),
                        s2.label
                    );
                    let color = color_for_index(graph.series_counter);
                    let new_series =
                        DataSeries::new(title, math_result.x, math_result.y, color, unit);
                    graph.add_series(new_series);
                    state.math_error.clear();
                    state.selected_series.push(false);
                }
                Err(e) => {
                    state.math_error = e;
                }
            }
        }
    }
}

fn show_unit_panel(
    ui: &mut egui::Ui,
    state: &mut SettingsDialogState,
    graph: &mut GraphState,
) {
    let ids = get_selected_ids(state, graph);
    if ids.is_empty() {
        ui.label(egui::RichText::new("Select series above first.").weak());
        return;
    }

    ui.add_space(2.0);

    egui::Grid::new("unit_grid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("New unit:");
            ui.add(egui::TextEdit::singleline(&mut state.unit_new_name).desired_width(150.0));
            ui.end_row();

            ui.label("Factor:");
            ui.add(egui::TextEdit::singleline(&mut state.unit_factor).desired_width(100.0));
            ui.end_row();

            ui.label("Bias:");
            ui.add(egui::TextEdit::singleline(&mut state.unit_bias).desired_width(100.0));
            ui.end_row();
        });

    ui.add_space(2.0);
    ui.label(egui::RichText::new("Formula: new = (old + bias) \u{00D7} factor").weak());

    ui.add_space(8.0);
    ui.label(egui::RichText::new("Presets:").strong());
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        if ui
            .add(
                egui::Button::new("\u{00B0}F \u{2192} \u{00B0}C")
                    .min_size(egui::vec2(0.0, 26.0)),
            )
            .clicked()
        {
            state.unit_new_name = "\u{00B0}C".to_string();
            state.unit_factor = "0.555556".to_string();
            state.unit_bias = "-32".to_string();
        }
        if ui
            .add(
                egui::Button::new("\u{00B0}C \u{2192} \u{00B0}F")
                    .min_size(egui::vec2(0.0, 26.0)),
            )
            .clicked()
        {
            state.unit_new_name = "\u{00B0}F".to_string();
            state.unit_factor = "1.8".to_string();
            state.unit_bias = "32".to_string();
        }
        if ui
            .add(egui::Button::new("psi \u{2192} kPa").min_size(egui::vec2(0.0, 26.0)))
            .clicked()
        {
            state.unit_new_name = "kPa".to_string();
            state.unit_factor = "6.89476".to_string();
            state.unit_bias = "0".to_string();
        }
        if ui
            .add(egui::Button::new("kPa \u{2192} psi").min_size(egui::vec2(0.0, 26.0)))
            .clicked()
        {
            state.unit_new_name = "psi".to_string();
            state.unit_factor = "0.145038".to_string();
            state.unit_bias = "0".to_string();
        }
    });

    ui.add_space(8.0);

    if ui
        .add(egui::Button::new("Apply Conversion").min_size(egui::vec2(140.0, 28.0)))
        .clicked()
    {
        if let (Ok(factor), Ok(bias)) = (
            state.unit_factor.parse::<f64>(),
            state.unit_bias.parse::<f64>(),
        ) {
            if !state.unit_new_name.is_empty() && factor != 0.0 {
                let selected_ids = get_selected_ids(state, graph);
                let new_unit = state.unit_new_name.clone();
                for series in &mut graph.series {
                    if selected_ids.contains(&series.id) {
                        series.y = series.y.iter().map(|&v| (v + bias) * factor).collect();
                        let old_name = series
                            .label
                            .split(" (")
                            .next()
                            .unwrap_or(&series.label)
                            .to_string();
                        series.label = format!("{} ({})", old_name, new_unit);
                        series.unit = new_unit.clone();
                        series.needs_resample = true;
                    }
                }
            }
        }
    }
}
