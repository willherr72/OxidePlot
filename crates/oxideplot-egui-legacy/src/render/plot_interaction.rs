pub use oxideplot_core::state::plot_view::PlotViewState;
pub use oxideplot_core::render::axis::{compute_grid_lines, format_tick_value};

// Helpers to convert between egui and core geom types at the egui boundary.
fn to_geom_rect(r: egui::Rect) -> oxideplot_core::geom::Rect {
    oxideplot_core::geom::Rect { left: r.min.x, top: r.min.y, width: r.width(), height: r.height() }
}
fn to_geom_pos(p: egui::Pos2) -> oxideplot_core::geom::Pos2 {
    oxideplot_core::geom::Pos2 { x: p.x, y: p.y }
}

/// Extension trait providing egui-dependent methods for PlotViewState.
pub trait PlotViewStateExt {
    fn handle_input(&mut self, response: &egui::Response, rect: egui::Rect);
}

impl PlotViewStateExt for PlotViewState {
    /// Handle mouse input on the plot area for pan/zoom.
    fn handle_input(&mut self, response: &egui::Response, rect: egui::Rect) {
        // Pan: drag with primary mouse button
        if response.dragged_by(egui::PointerButton::Primary) {
            let delta = response.drag_delta();
            self.pan(delta.x, delta.y, to_geom_rect(rect));
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
            if let Some(mouse_pos) = response.hover_pos() {
                self.zoom(scroll_delta, to_geom_pos(mouse_pos), to_geom_rect(rect));
            } else {
                // Zoom without anchor — use center
                let zoom_factor = (1.0 - (scroll_delta as f64) * 0.001).clamp(0.5, 2.0);
                let cx = (self.x_min + self.x_max) / 2.0;
                let cy = (self.y_min + self.y_max) / 2.0;
                self.x_min = cx + (self.x_min - cx) * zoom_factor;
                self.x_max = cx + (self.x_max - cx) * zoom_factor;
                self.y_min = cy + (self.y_min - cy) * zoom_factor;
                self.y_max = cy + (self.y_max - cy) * zoom_factor;
                self.auto_fit = false;
            }
        }

        // Double-click to auto-fit
        if response.double_clicked() {
            self.auto_fit = true;
        }
    }
}
