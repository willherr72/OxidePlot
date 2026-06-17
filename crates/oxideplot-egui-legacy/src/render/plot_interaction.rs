pub use oxideplot_core::state::plot_view::PlotViewState;

/// Extension trait providing egui-dependent methods for PlotViewState.
pub trait PlotViewStateExt {
    fn handle_input(&mut self, response: &egui::Response, rect: egui::Rect);
    fn screen_to_data(&self, pos: egui::Pos2, rect: egui::Rect) -> (f64, f64);
    fn data_to_screen(&self, x: f64, y: f64, rect: egui::Rect) -> egui::Pos2;
}

impl PlotViewStateExt for PlotViewState {
    /// Handle mouse input on the plot area for pan/zoom.
    fn handle_input(&mut self, response: &egui::Response, rect: egui::Rect) {
        // Pan: drag with primary mouse button
        if response.dragged_by(egui::PointerButton::Primary) {
            let delta = response.drag_delta();
            let dx = -(delta.x as f64) * (self.x_max - self.x_min) / rect.width() as f64;
            let dy = (delta.y as f64) * (self.y_max - self.y_min) / rect.height() as f64;
            self.x_min += dx;
            self.x_max += dx;
            self.y_min += dy;
            self.y_max += dy;
            self.auto_fit = false;
        }

        // Zoom: scroll wheel, centered on mouse position
        let scroll_delta = response.ctx.input(|i| {
            if response.hovered() {
                i.smooth_scroll_delta.y
            } else {
                0.0
            }
        });

        if scroll_delta.abs() > 0.0 {
            let zoom_factor = 1.0 - (scroll_delta as f64) * 0.001;
            let zoom_factor = zoom_factor.clamp(0.5, 2.0);

            if let Some(mouse_pos) = response.hover_pos() {
                let (cx, cy) = self.screen_to_data(mouse_pos, rect);
                self.x_min = cx + (self.x_min - cx) * zoom_factor;
                self.x_max = cx + (self.x_max - cx) * zoom_factor;
                self.y_min = cy + (self.y_min - cy) * zoom_factor;
                self.y_max = cy + (self.y_max - cy) * zoom_factor;
            }
            self.auto_fit = false;
        }

        // Double-click to auto-fit
        if response.double_clicked() {
            self.auto_fit = true;
        }
    }

    /// Convert screen position to data coordinates.
    fn screen_to_data(&self, pos: egui::Pos2, rect: egui::Rect) -> (f64, f64) {
        let t_x = (pos.x - rect.left()) as f64 / rect.width() as f64;
        let t_y = 1.0 - (pos.y - rect.top()) as f64 / rect.height() as f64;
        let data_x = self.x_min + t_x * (self.x_max - self.x_min);
        let data_y = self.y_min + t_y * (self.y_max - self.y_min);
        (data_x, data_y)
    }

    /// Convert data coordinates to screen position.
    fn data_to_screen(&self, x: f64, y: f64, rect: egui::Rect) -> egui::Pos2 {
        let t_x = (x - self.x_min) / (self.x_max - self.x_min);
        let t_y = 1.0 - (y - self.y_min) / (self.y_max - self.y_min);
        egui::Pos2::new(
            rect.left() + (t_x as f32) * rect.width(),
            rect.top() + (t_y as f32) * rect.height(),
        )
    }
}

/// Compute nice grid line positions for an axis range.
/// Returns (value, is_major) pairs.
pub fn compute_grid_lines(min: f64, max: f64) -> Vec<(f64, bool)> {
    let range = max - min;
    if range <= 0.0 || !range.is_finite() {
        return Vec::new();
    }

    let raw_step = range / 8.0;
    let order = 10f64.powf(raw_step.log10().floor());
    let normalized = raw_step / order;

    let nice_step = if normalized <= 1.0 {
        order
    } else if normalized <= 2.0 {
        2.0 * order
    } else if normalized <= 5.0 {
        5.0 * order
    } else {
        10.0 * order
    };

    let minor_step = nice_step / 5.0;

    let start = (min / minor_step).floor() as i64;
    let end = (max / minor_step).ceil() as i64;

    let mut lines = Vec::new();
    for i in start..=end {
        let val = i as f64 * minor_step;
        if val >= min && val <= max {
            let is_major = ((val / nice_step).round() * nice_step - val).abs() < nice_step * 0.01;
            lines.push((val, is_major));
        }
    }
    lines
}

/// Format a numeric value for axis tick labels.
pub fn format_tick_value(val: f64) -> String {
    if val.abs() >= 1e6 || (val != 0.0 && val.abs() < 1e-3) {
        format!("{val:.2e}")
    } else if val == 0.0 {
        "0".to_string()
    } else {
        let s = format!("{val:.6}");
        let s = s.trim_end_matches('0');
        let s = s.trim_end_matches('.');
        s.to_string()
    }
}
