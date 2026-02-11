use crate::data::loader::LoadedData;

/// State for the data selection dialog, created when the user loads a file
/// and needs to choose which columns to plot.
pub struct DataSelectionState {
    pub loaded_data: LoadedData,
    pub target_graph_id: u64,
    pub selected_x: usize,
    pub selected_y: Vec<bool>,
    /// Optional Z column index (into usable_columns). None = 2D, Some = 3D.
    pub selected_z: Option<usize>,
    pub usable_columns: Vec<usize>,
}

impl DataSelectionState {
    pub fn new(loaded_data: LoadedData, target_graph_id: u64) -> Self {
        // Determine which columns are usable (numeric or datetime-like).
        let usable_columns: Vec<usize> = (0..loaded_data.columns.len())
            .filter(|&i| {
                let col_name = &loaded_data.columns[i];
                // Columns with "time" in the name are always usable.
                if col_name.to_lowercase().contains("time") {
                    return true;
                }
                // Check if column data is mostly numeric by sampling up to 100 rows.
                let data = &loaded_data.column_data[i];
                let sample: Vec<&str> = data.iter().take(100).map(|s| s.as_str()).collect();
                let numeric_count = sample
                    .iter()
                    .filter(|s| s.trim().parse::<f64>().is_ok())
                    .count();
                let ratio = if sample.is_empty() {
                    0.0
                } else {
                    numeric_count as f64 / sample.len() as f64
                };
                ratio >= 0.5
            })
            .collect();

        // Fall back to all columns if none pass the filter.
        let usable = if usable_columns.is_empty() {
            (0..loaded_data.columns.len()).collect()
        } else {
            usable_columns
        };

        let num_usable = usable.len();
        Self {
            loaded_data,
            target_graph_id,
            selected_x: 0,
            selected_y: vec![false; num_usable],
            selected_z: None,
            usable_columns: usable,
        }
    }
}

/// The columns the user selected from the dialog.
pub struct ColumnSelection {
    pub graph_id: u64,
    pub x_col_index: usize,
    pub y_col_indices: Vec<usize>,
    /// Optional Z column for 3D data.
    pub z_col_index: Option<usize>,
}

/// Result of the data selection dialog interaction each frame.
pub enum DialogResult {
    Ok(ColumnSelection),
    Cancel,
}

/// Show the data selection dialog as an egui window.
///
/// Returns `Some(DialogResult)` when the user presses OK or Cancel,
/// or `None` while the dialog is still open.
pub fn show_data_selection_dialog(
    ctx: &egui::Context,
    state: &mut DataSelectionState,
) -> Option<DialogResult> {
    let mut result = None;

    egui::Window::new("Select Data Columns")
        .collapsible(false)
        .resizable(true)
        .default_width(480.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            // File info
            ui.label(egui::RichText::new(format!(
                "File contains {} columns and {} rows.",
                state.loaded_data.columns.len(),
                state.loaded_data.row_count,
            )).weak());

            ui.add_space(12.0);

            // --- X axis selector ---
            ui.label(egui::RichText::new("X Axis").strong());
            ui.add_space(2.0);
            if !state.usable_columns.is_empty() {
                let current_x_name =
                    &state.loaded_data.columns[state.usable_columns[state.selected_x]];
                egui::ComboBox::from_id_salt("x_axis_selector")
                    .selected_text(current_x_name)
                    .width(300.0)
                    .show_ui(ui, |ui| {
                        for (i, &col_idx) in state.usable_columns.iter().enumerate() {
                            ui.selectable_value(
                                &mut state.selected_x,
                                i,
                                &state.loaded_data.columns[col_idx],
                            );
                        }
                    });
            }

            ui.add_space(12.0);

            // --- Y axis multi-selector ---
            ui.label(egui::RichText::new("Y Axis (select one or more)").strong());
            ui.add_space(2.0);

            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(200.0)
                        .show(ui, |ui| {
                            for (i, &col_idx) in state.usable_columns.iter().enumerate() {
                                ui.checkbox(
                                    &mut state.selected_y[i],
                                    &state.loaded_data.columns[col_idx],
                                );
                            }
                        });
                });

            ui.add_space(12.0);

            // --- Z axis selector (optional, for 3D) ---
            ui.label(egui::RichText::new("Z Axis (optional, enables 3D)").strong());
            ui.add_space(2.0);
            {
                let z_label = match state.selected_z {
                    Some(idx) => state.loaded_data.columns[state.usable_columns[idx]].clone(),
                    None => "None (2D)".to_string(),
                };
                egui::ComboBox::from_id_salt("z_axis_selector")
                    .selected_text(z_label)
                    .width(300.0)
                    .show_ui(ui, |ui| {
                        let mut z_val = state.selected_z;
                        if ui.selectable_value(&mut z_val, None, "None (2D)").clicked() {
                            state.selected_z = None;
                        }
                        for (i, &col_idx) in state.usable_columns.iter().enumerate() {
                            if ui.selectable_value(
                                &mut z_val,
                                Some(i),
                                &state.loaded_data.columns[col_idx],
                            ).clicked() {
                                state.selected_z = Some(i);
                            }
                        }
                    });
            }

            ui.add_space(16.0);

            // --- OK / Cancel buttons ---
            let any_y_selected = state.selected_y.iter().any(|&s| s);
            ui.horizontal(|ui| {
                let ok_btn = ui.add_enabled(
                    any_y_selected,
                    egui::Button::new(egui::RichText::new("OK").strong())
                        .min_size(egui::vec2(100.0, 32.0)),
                );
                if ok_btn.clicked() {
                    let x_col = state.usable_columns[state.selected_x];
                    let y_cols: Vec<usize> = state
                        .selected_y
                        .iter()
                        .enumerate()
                        .filter(|(_, &selected)| selected)
                        .map(|(i, _)| state.usable_columns[i])
                        .collect();
                    let z_col = state.selected_z.map(|i| state.usable_columns[i]);

                    if !y_cols.is_empty() {
                        result = Some(DialogResult::Ok(ColumnSelection {
                            graph_id: state.target_graph_id,
                            x_col_index: x_col,
                            y_col_indices: y_cols,
                            z_col_index: z_col,
                        }));
                    }
                }

                if ui.add(egui::Button::new("Cancel").min_size(egui::vec2(100.0, 32.0))).clicked() {
                    result = Some(DialogResult::Cancel);
                }

                if !any_y_selected {
                    ui.label(egui::RichText::new("Select at least one Y column").weak());
                }
            });
        });

    result
}
