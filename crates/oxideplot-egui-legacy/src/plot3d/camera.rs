use eframe::egui;
use glam::{Mat4, Vec3};
use std::f32::consts::PI;

/// Orbital camera that revolves around a target point.
///
/// The camera position is derived from spherical coordinates (azimuth,
/// elevation, distance) relative to `target`. It produces right-handed
/// view and perspective projection matrices suitable for wgpu/NDC.
#[derive(Debug, Clone)]
pub struct OrbitalCamera {
    /// The world-space point the camera orbits around.
    pub target: Vec3,
    /// Distance from the target along the viewing ray.
    pub distance: f32,
    /// Horizontal angle in radians (rotation around the world Y axis).
    pub azimuth: f32,
    /// Vertical angle in radians, clamped to (-PI/2 + 0.01, PI/2 - 0.01).
    pub elevation: f32,
    /// Vertical field of view in radians.
    pub fov_y: f32,
}

impl Default for OrbitalCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            distance: 3.0,
            azimuth: PI / 4.0,
            elevation: PI / 6.0,
            fov_y: PI / 4.0,
        }
    }
}

/// Minimum elevation (just above looking straight down from below).
const ELEVATION_MIN: f32 = -PI / 2.0 + 0.01;
/// Maximum elevation (just below looking straight down from above).
const ELEVATION_MAX: f32 = PI / 2.0 - 0.01;

/// Minimum orbit distance.
const DISTANCE_MIN: f32 = 0.1;
/// Maximum orbit distance.
const DISTANCE_MAX: f32 = 50.0;

impl OrbitalCamera {
    /// Compute the camera's world-space position from the spherical
    /// coordinates (azimuth, elevation, distance) relative to `target`.
    pub fn position(&self) -> Vec3 {
        let cos_elev = self.elevation.cos();
        let sin_elev = self.elevation.sin();
        let cos_az = self.azimuth.cos();
        let sin_az = self.azimuth.sin();

        let offset = Vec3::new(
            cos_elev * sin_az,
            sin_elev,
            cos_elev * cos_az,
        ) * self.distance;

        self.target + offset
    }

    /// Build a right-handed view matrix looking from `position()` toward
    /// `target` with world-up = +Y.
    pub fn view_matrix(&self) -> Mat4 {
        let eye = self.position();
        Mat4::look_at_rh(eye, self.target, Vec3::Y)
    }

    /// Build a right-handed perspective projection matrix.
    ///
    /// * `aspect` - viewport width / height.
    /// * near = 0.01, far = 100.0.
    pub fn projection_matrix(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, aspect, 0.01, 100.0)
    }

    /// Combined view-projection matrix (projection * view).
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection_matrix(aspect) * self.view_matrix()
    }

    /// Rotate the camera by the given angle deltas (radians).
    ///
    /// Elevation is clamped to avoid gimbal-lock singularities at the poles.
    pub fn rotate(&mut self, delta_azimuth: f32, delta_elevation: f32) {
        self.azimuth += delta_azimuth;
        self.elevation = (self.elevation + delta_elevation).clamp(ELEVATION_MIN, ELEVATION_MAX);
    }

    /// Zoom by multiplying the current distance by `factor`.
    ///
    /// The resulting distance is clamped to [0.1, 50.0].
    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(DISTANCE_MIN, DISTANCE_MAX);
    }

    /// Pan the target position in the camera-local right/up plane.
    ///
    /// `dx` moves along the camera's right vector and `dy` moves along the
    /// camera's up vector. Both are scaled by the current distance so that
    /// panning feels consistent regardless of zoom level.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let view = self.view_matrix();

        // The view matrix rows (in column-major storage) give us the camera
        // basis vectors expressed in world space.
        let right = Vec3::new(view.x_axis.x, view.y_axis.x, view.z_axis.x);
        let up = Vec3::new(view.x_axis.y, view.y_axis.y, view.z_axis.y);

        let scale = self.distance * 0.002;
        self.target += right * (-dx * scale) + up * (dy * scale);
    }

    /// Reset the camera to its default state.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Process egui mouse / scroll input on the given `Response` area.
    ///
    /// * **Left-drag** rotates the camera.
    /// * **Right-drag** pans the target.
    /// * **Scroll wheel** zooms in/out.
    /// * **Double-click** resets to defaults.
    pub fn handle_input(&mut self, response: &egui::Response) {
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
    pub fn uniforms(&self, aspect: f32) -> crate::render::gpu_types::Plot3DUniforms {
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
