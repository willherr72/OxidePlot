use crate::state::data_series::DataSeries;

/// View state for a GPU-rendered plot. Tracks current view bounds
/// and handles pan/zoom interaction.
#[derive(Debug, Clone)]
pub struct PlotViewState {
    /// Current view bounds in data coordinates
    pub x_min: f64,
    pub x_max: f64,
    pub y_min: f64,
    pub y_max: f64,
    /// Whether to auto-fit view to data on next frame
    pub auto_fit: bool,
    /// Track if we've ever been initialized
    pub initialized: bool,
    /// Previous frame's X range for change detection (used by sync).
    pub prev_x_min: f64,
    pub prev_x_max: f64,
}

impl Default for PlotViewState {
    fn default() -> Self {
        Self {
            x_min: 0.0,
            x_max: 1.0,
            y_min: 0.0,
            y_max: 1.0,
            auto_fit: true,
            initialized: false,
            prev_x_min: 0.0,
            prev_x_max: 1.0,
        }
    }
}

impl PlotViewState {
    /// Returns true if the X range changed since the last snapshot.
    pub fn x_range_changed(&self) -> bool {
        (self.x_min - self.prev_x_min).abs() > 1e-15
            || (self.x_max - self.prev_x_max).abs() > 1e-15
    }

    /// Snapshot the current X range as the "previous" state.
    pub fn snapshot_x_range(&mut self) {
        self.prev_x_min = self.x_min;
        self.prev_x_max = self.x_max;
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// Auto-scale only the Y axis to fit data visible in the current X range.
    /// Called every frame when auto_scale_y is enabled.
    pub fn auto_scale_y_to_visible(&mut self, series: &[DataSeries]) {
        let mut y_min = f64::INFINITY;
        let mut y_max = f64::NEG_INFINITY;

        for s in series {
            if !s.visible || s.x.is_empty() {
                continue;
            }
            for (&xv, &yv) in s.x.iter().zip(s.y.iter()) {
                if xv.is_finite() && yv.is_finite() && xv >= self.x_min && xv <= self.x_max {
                    y_min = y_min.min(yv);
                    y_max = y_max.max(yv);
                }
            }
        }

        if !y_min.is_finite() || !y_max.is_finite() {
            return;
        }

        let y_pad = (y_max - y_min) * 0.05;
        let y_pad = if y_pad.abs() < 1e-15 { 0.5 } else { y_pad };

        self.y_min = y_min - y_pad;
        self.y_max = y_max + y_pad;
    }

    /// Auto-scale only the Y axis for normalized multi-unit mode.
    pub fn auto_scale_y_normalized(&mut self) {
        self.y_min = -0.05;
        self.y_max = 1.05;
    }

    /// Auto-fit the view bounds to encompass all visible series data.
    /// Adds 5% padding on each side.
    pub fn fit_to_data(&mut self, series: &[DataSeries]) {
        let mut x_min = f64::INFINITY;
        let mut x_max = f64::NEG_INFINITY;
        let mut y_min = f64::INFINITY;
        let mut y_max = f64::NEG_INFINITY;

        for s in series {
            if !s.visible || s.x.is_empty() {
                continue;
            }
            for &xv in &s.x {
                if xv.is_finite() {
                    x_min = x_min.min(xv);
                    x_max = x_max.max(xv);
                }
            }
            for &yv in &s.y {
                if yv.is_finite() {
                    y_min = y_min.min(yv);
                    y_max = y_max.max(yv);
                }
            }
        }

        if !x_min.is_finite() || !x_max.is_finite() || !y_min.is_finite() || !y_max.is_finite() {
            return;
        }

        let x_pad = (x_max - x_min) * 0.05;
        let y_pad = (y_max - y_min) * 0.05;
        let x_pad = if x_pad.abs() < 1e-15 { 0.5 } else { x_pad };
        let y_pad = if y_pad.abs() < 1e-15 { 0.5 } else { y_pad };

        self.x_min = x_min - x_pad;
        self.x_max = x_max + x_pad;
        self.y_min = y_min - y_pad;
        self.y_max = y_max + y_pad;
        self.initialized = true;
    }

    /// Auto-fit the view bounds for normalized multi-unit data (Y in [0, 1]).
    pub fn fit_to_data_normalized(&mut self, series: &[DataSeries]) {
        let mut x_min = f64::INFINITY;
        let mut x_max = f64::NEG_INFINITY;

        for s in series {
            if !s.visible || s.x.is_empty() {
                continue;
            }
            for &xv in &s.x {
                if xv.is_finite() {
                    x_min = x_min.min(xv);
                    x_max = x_max.max(xv);
                }
            }
        }

        if !x_min.is_finite() || !x_max.is_finite() {
            return;
        }

        let x_pad = (x_max - x_min) * 0.05;
        let x_pad = if x_pad.abs() < 1e-15 { 0.5 } else { x_pad };

        self.x_min = x_min - x_pad;
        self.x_max = x_max + x_pad;
        self.y_min = -0.05;
        self.y_max = 1.05;
        self.initialized = true;
    }

    /// Handle mouse input on the plot area for pan/zoom.
    pub fn handle_input(&mut self, response: &egui::Response, rect: egui::Rect) {
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
    pub fn screen_to_data(&self, pos: egui::Pos2, rect: egui::Rect) -> (f64, f64) {
        let t_x = (pos.x - rect.left()) as f64 / rect.width() as f64;
        let t_y = 1.0 - (pos.y - rect.top()) as f64 / rect.height() as f64;
        let data_x = self.x_min + t_x * (self.x_max - self.x_min);
        let data_y = self.y_min + t_y * (self.y_max - self.y_min);
        (data_x, data_y)
    }

    /// Convert data coordinates to screen position.
    pub fn data_to_screen(&self, x: f64, y: f64, rect: egui::Rect) -> egui::Pos2 {
        let t_x = (x - self.x_min) / (self.x_max - self.x_min);
        let t_y = 1.0 - (y - self.y_min) / (self.y_max - self.y_min);
        egui::Pos2::new(
            rect.left() + (t_x as f32) * rect.width(),
            rect.top() + (t_y as f32) * rect.height(),
        )
    }

    /// Set the x range (used for sync).
    pub fn set_x_range(&mut self, min: f64, max: f64) {
        self.x_min = min;
        self.x_max = max;
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
