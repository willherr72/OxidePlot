use bytemuck::{Pod, Zeroable};

/// GPU uniform buffer for the plot transform and rendering parameters.
/// Layout matches the WGSL struct exactly (64 bytes, aligned to 16).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct PlotUniforms {
    /// Min x,y of view window in data coords.
    pub view_min: [f32; 2],
    /// Max x,y of view window in data coords.
    pub view_max: [f32; 2],
    /// Pixel size of the plot area.
    pub resolution: [f32; 2],
    /// Line width in pixels.
    pub line_width: f32,
    /// Point radius in pixels.
    pub point_radius: f32,
    /// RGBA color for current draw call.
    pub color: [f32; 4],
    /// Pad to 64 bytes (multiple of 16).
    pub _padding: [f32; 4],
}

/// How a data series should be drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawMode {
    /// Connected line segments (point[i] -> point[i+1]).
    Lines,
    /// Step function (horizontal then vertical segments).
    Step,
    /// Individual scatter points.
    Points,
}

/// GPU-ready data for a single data series.
#[derive(Debug, Clone)]
pub struct SeriesGpuData {
    /// Point positions in data coordinates, stored as [x, y] pairs.
    pub points: Vec<[f32; 2]>,
    /// RGBA color (0.0..1.0 per channel).
    pub color: [f32; 4],
    /// Line width in pixels.
    pub line_width: f32,
    /// Point radius in pixels.
    pub point_radius: f32,
    /// How to draw this series.
    pub draw_mode: DrawMode,
}

/// GPU-ready data for grid lines.
#[derive(Debug, Clone)]
pub struct GridGpuData {
    /// Line segment endpoint pairs: [p0, p1, p0, p1, ...].
    /// Each consecutive pair of `[f32; 2]` values defines one line segment.
    pub segments: Vec<[f32; 2]>,
    /// RGBA color (0.0..1.0 per channel).
    pub color: [f32; 4],
    /// Line width in pixels.
    pub line_width: f32,
}

// ---------------------------------------------------------------------------
// 3D rendering types
// ---------------------------------------------------------------------------

/// GPU uniform buffer for 3D plot rendering.
/// Contains camera transform + per-draw-call parameters.
/// 112 bytes = 7 * 16, properly 16-byte aligned.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Plot3DUniforms {
    /// View-projection matrix (column-major).
    pub view_proj: [[f32; 4]; 4],
    /// Camera world position (w unused).
    pub camera_pos: [f32; 4],
    /// RGBA color for current draw call.
    pub color: [f32; 4],
    /// Viewport resolution in pixels.
    pub resolution: [f32; 2],
    /// Point radius in pixels.
    pub point_size: f32,
    /// Line width in pixels.
    pub line_width: f32,
}

/// GPU-ready 3D scatter data for a single series.
#[derive(Debug, Clone)]
pub struct Scatter3DData {
    /// Positions as [x, y, z, _pad] (w unused, set to 1.0).
    pub positions: Vec<[f32; 4]>,
    /// RGBA color.
    pub color: [f32; 4],
    /// Point radius in pixels.
    pub point_size: f32,
}

/// GPU-ready 3D line data for a single series.
#[derive(Debug, Clone)]
pub struct Line3DData {
    /// Segment endpoint pairs: [start, end, start, end, ...].
    /// Each position is [x, y, z, _pad].
    pub segments: Vec<[f32; 4]>,
    /// RGBA color.
    pub color: [f32; 4],
    /// Line width in pixels.
    pub line_width: f32,
}
