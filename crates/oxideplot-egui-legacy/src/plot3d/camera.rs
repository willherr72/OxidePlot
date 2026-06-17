pub use oxideplot_core::state::orbital_camera::OrbitalCamera;
use eframe::egui;

/// Extension trait providing egui-dependent methods for OrbitalCamera.
pub trait OrbitalCameraExt {
    fn handle_input(&mut self, response: &egui::Response);
    fn uniforms(&self, aspect: f32) -> crate::render::gpu_types::Plot3DUniforms;
}

impl OrbitalCameraExt for OrbitalCamera {
    /// Process egui mouse / scroll input on the given `Response` area.
    ///
    /// * **Left-drag** rotates the camera.
    /// * **Right-drag** pans the target.
    /// * **Scroll wheel** zooms in/out.
    /// * **Double-click** resets to defaults.
    fn handle_input(&mut self, response: &egui::Response) {
        // --- Rotation (left drag) ---
        if response.dragged_by(egui::PointerButton::Primary) {
            let delta = response.drag_delta();
            self.rotate(delta.x * -0.005, delta.y * -0.005);
        }

        // --- Pan (right drag) ---
        if response.dragged_by(egui::PointerButton::Secondary) {
            let delta = response.drag_delta();
            self.pan(delta.x, delta.y);
        }

        // --- Zoom (scroll wheel) ---
        if response.hovered() {
            let scroll = response.ctx.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                let factor = (1.0_f32 - scroll * 0.001).clamp(0.5, 2.0);
                self.zoom(factor);
            }
        }

        // --- Reset (double-click) ---
        if response.double_clicked() {
            self.reset();
        }
    }

    /// Build a `Plot3DUniforms` value from the current camera state.
    ///
    /// The `view_proj` and `camera_pos` fields are filled from the camera;
    /// `color`, `resolution`, `point_size`, and `line_width` are set to
    /// zeros/defaults and should be overwritten per draw call.
    fn uniforms(&self, aspect: f32) -> crate::render::gpu_types::Plot3DUniforms {
        let vp = self.view_projection(aspect);
        let pos = self.position();

        crate::render::gpu_types::Plot3DUniforms {
            view_proj: vp.to_cols_array_2d(),
            camera_pos: [pos.x, pos.y, pos.z, 1.0],
            color: [0.0; 4],
            resolution: [0.0; 2],
            point_size: 0.0,
            line_width: 0.0,
        }
    }
}
