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

    /// Set the x range (used for sync).
    pub fn set_x_range(&mut self, min: f64, max: f64) {
        self.x_min = min;
        self.x_max = max;
    }

    /// Pan the view by a pixel drag delta.
    pub fn pan(&mut self, dx_px: f32, dy_px: f32, rect: crate::geom::Rect) {
        let dx = -(dx_px as f64) * (self.x_max - self.x_min) / rect.width as f64;
        let dy = (dy_px as f64) * (self.y_max - self.y_min) / rect.height as f64;
        self.x_min += dx; self.x_max += dx; self.y_min += dy; self.y_max += dy;
        self.auto_fit = false;
    }

    /// Zoom about a screen-space anchor using a scroll delta.
    pub fn zoom(&mut self, scroll_y: f32, anchor: crate::geom::Pos2, rect: crate::geom::Rect) {
        let zoom_factor = (1.0 - (scroll_y as f64) * 0.001).clamp(0.5, 2.0);
        let (cx, cy) = self.screen_to_data(anchor, rect);
        self.x_min = cx + (self.x_min - cx) * zoom_factor;
        self.x_max = cx + (self.x_max - cx) * zoom_factor;
        self.y_min = cy + (self.y_min - cy) * zoom_factor;
        self.y_max = cy + (self.y_max - cy) * zoom_factor;
        self.auto_fit = false;
    }

    /// Screen pixel position within `rect` -> data coordinates.
    pub fn screen_to_data(&self, pos: crate::geom::Pos2, rect: crate::geom::Rect) -> (f64, f64) {
        let t_x = (pos.x - rect.left) as f64 / rect.width as f64;
        let t_y = 1.0 - (pos.y - rect.top) as f64 / rect.height as f64;
        (self.x_min + t_x * (self.x_max - self.x_min), self.y_min + t_y * (self.y_max - self.y_min))
    }

    /// Data coordinates -> screen pixel position within `rect`.
    pub fn data_to_screen(&self, x: f64, y: f64, rect: crate::geom::Rect) -> crate::geom::Pos2 {
        let t_x = (x - self.x_min) / (self.x_max - self.x_min);
        let t_y = 1.0 - (y - self.y_min) / (self.y_max - self.y_min);
        crate::geom::Pos2 { x: rect.left + (t_x as f32) * rect.width, y: rect.top + (t_y as f32) * rect.height }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rect;

    #[test]
    fn pan_shifts_view_left() {
        let mut v = PlotViewState { x_min: 0.0, x_max: 10.0, y_min: 0.0, y_max: 10.0, ..Default::default() };
        v.pan(10.0, 0.0, Rect { left: 0.0, top: 0.0, width: 100.0, height: 100.0 });
        assert!(v.x_min < 0.0 && v.x_max < 10.0, "drag-right should move the view left");
    }

    #[test]
    fn screen_data_roundtrip() {
        let v = PlotViewState { x_min: 0.0, x_max: 10.0, y_min: 0.0, y_max: 10.0, ..Default::default() };
        let r = Rect { left: 0.0, top: 0.0, width: 100.0, height: 100.0 };
        let p = v.data_to_screen(5.0, 5.0, r);
        let (dx, dy) = v.screen_to_data(p, r);
        assert!((dx - 5.0).abs() < 1e-9 && (dy - 5.0).abs() < 1e-9);
    }
}
