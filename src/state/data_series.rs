use serde::{Deserialize, Serialize};

/// Color palette matching the Python version's 12 colors
pub const COLOR_PALETTE: [[u8; 4]; 12] = [
    [255, 0, 0, 255],     // Red
    [0, 255, 0, 255],     // Green
    [0, 0, 255, 255],     // Blue
    [255, 255, 0, 255],   // Yellow
    [255, 0, 255, 255],   // Magenta
    [0, 255, 255, 255],   // Cyan
    [255, 165, 0, 255],   // Orange
    [128, 0, 128, 255],   // Purple
    [0, 128, 0, 255],     // Dark Green
    [0, 0, 128, 255],     // Navy
    [255, 192, 203, 255], // Pink
    [165, 42, 42, 255],   // Brown
];

pub fn color_for_index(index: usize) -> [u8; 4] {
    COLOR_PALETTE[index % COLOR_PALETTE.len()]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterpolationMode {
    Linear,
    Step,
    Points,
}

impl Default for InterpolationMode {
    fn default() -> Self {
        InterpolationMode::Linear
    }
}

impl InterpolationMode {
    pub fn label(&self) -> &'static str {
        match self {
            InterpolationMode::Linear => "Linear",
            InterpolationMode::Step => "Step",
            InterpolationMode::Points => "Points Only",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlotMode {
    Plot2D,
    Plot3D,
}

impl Default for PlotMode {
    fn default() -> Self {
        PlotMode::Plot2D
    }
}

static NEXT_SERIES_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_series_id() -> u64 {
    NEXT_SERIES_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSeries {
    pub id: u64,
    pub label: String,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    /// Optional Z data for 3D plots. Empty means 2D-only series.
    pub z: Vec<f64>,
    pub color: [u8; 4],
    pub unit: String,
    pub interpolation: InterpolationMode,
    pub show_dots: bool,
    pub visible: bool,
    /// Line width for this series (pixels).
    pub line_width: f32,
    #[serde(skip)]
    pub downsampled_x: Vec<f64>,
    #[serde(skip)]
    pub downsampled_y: Vec<f64>,
    #[serde(skip)]
    pub needs_resample: bool,
}

impl DataSeries {
    pub fn new(
        label: String,
        x: Vec<f64>,
        y: Vec<f64>,
        color: [u8; 4],
        unit: String,
    ) -> Self {
        Self {
            id: next_series_id(),
            label,
            x,
            y,
            z: Vec::new(),
            color,
            unit,
            interpolation: InterpolationMode::default(),
            show_dots: true,
            visible: true,
            line_width: 2.0,
            downsampled_x: Vec::new(),
            downsampled_y: Vec::new(),
            needs_resample: true,
        }
    }

    /// Create a 3D data series with X, Y, and Z data.
    pub fn new_3d(
        label: String,
        x: Vec<f64>,
        y: Vec<f64>,
        z: Vec<f64>,
        color: [u8; 4],
        unit: String,
    ) -> Self {
        Self {
            id: next_series_id(),
            label,
            x,
            y,
            z,
            color,
            unit,
            interpolation: InterpolationMode::default(),
            show_dots: true,
            visible: true,
            line_width: 2.0,
            downsampled_x: Vec::new(),
            downsampled_y: Vec::new(),
            needs_resample: true,
        }
    }

    /// Whether this series has 3D data.
    pub fn has_z(&self) -> bool {
        !self.z.is_empty()
    }

    pub fn color32(&self) -> egui::Color32 {
        egui::Color32::from_rgba_unmultiplied(self.color[0], self.color[1], self.color[2], self.color[3])
    }

    pub fn point_count(&self) -> usize {
        self.x.len()
    }
}
