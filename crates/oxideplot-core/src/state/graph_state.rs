//! Multi-series graph model (series, axes, cursors, axis-sync groups).
//! Retained for the planned 3D-plotting / MCP-server roadmap — the current 2D
//! MVP builds GPU series directly and does not yet consume `GraphState`.
//! Exposed as `pub` API rather than removed.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use crate::state::data_series::{DataSeries, PlotMode, color_for_index};
use crate::state::plot_view::PlotViewState;
use crate::state::orbital_camera::OrbitalCamera;

/// Direction for table column sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// Cursor measurement mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMode {
    Off,
    Vertical,
    Horizontal,
}

/// Maximum number of measurement cursors per plot.
pub const MAX_CURSORS: usize = 4;

/// State for measurement cursors on a 2D plot.
#[derive(Debug, Clone)]
pub struct CursorState {
    pub mode: CursorMode,
    /// Cursor positions C1..C4 (x for vertical, y for horizontal).
    pub positions: [Option<f64>; MAX_CURSORS],
    /// Which slot the next right-click fills (cycles 0..MAX_CURSORS-1).
    pub next_index: usize,
}

impl CursorState {
    /// Reset all cursor positions and the placement index.
    pub fn clear(&mut self) {
        self.positions = [None; MAX_CURSORS];
        self.next_index = 0;
    }

    /// Number of cursors currently placed.
    pub fn placed_count(&self) -> usize {
        self.positions.iter().filter(|p| p.is_some()).count()
    }
}

impl Default for CursorState {
    fn default() -> Self {
        Self {
            mode: CursorMode::Off,
            positions: [None; MAX_CURSORS],
            next_index: 0,
        }
    }
}

static NEXT_GRAPH_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_graph_id() -> u64 {
    NEXT_GRAPH_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxisState {
    pub label: String,
    pub auto_range: bool,
    pub min: f64,
    pub max: f64,
}

impl Default for AxisState {
    fn default() -> Self {
        Self {
            label: String::new(),
            auto_range: true,
            min: 0.0,
            max: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphState {
    pub id: u64,
    pub title: String,
    pub series: Vec<DataSeries>,
    pub series_counter: usize,
    pub x_axis_is_datetime: Option<bool>,
    pub x_axis_name: Option<String>,
    pub x_axis_unit: Option<String>,
    pub y_axis_name: Option<String>,
    pub z_axis_name: Option<String>,
    pub auto_scale_y: bool,
    pub y_axes: HashMap<String, AxisState>,
    pub sync_partner_ids: Vec<u64>,
    /// Shared axis-link group identifier. All graphs in the same sync group
    /// share the same value so their axes can be linked together. Computed as
    /// the minimum graph-id in the group.
    #[serde(skip)]
    pub sync_group_id: Option<u64>,
    pub show_data_table: bool,
    pub plot_mode: PlotMode,
    pub is_updating_range: bool,
    /// Table sort state: (column_index, sort_direction).
    /// None = original order, Some((col, direction)) = sorted.
    #[serde(skip)]
    pub table_sort: Option<(usize, SortDirection)>,
    /// GPU plot view state (pan/zoom bounds).
    #[serde(skip)]
    pub plot_view: PlotViewState,
    /// 3D orbital camera state.
    #[serde(skip)]
    pub camera: OrbitalCamera,
    /// Measurement cursor state.
    #[serde(skip)]
    pub cursor_state: CursorState,
    /// Checkbox state for the "Remove Data" popup.
    #[serde(skip)]
    pub remove_series_selected: Vec<bool>,
    /// Screen rect of the graph panel (set each frame for screenshot cropping).
    #[serde(skip)]
    pub last_frame_rect: Option<crate::geom::Rect>,
}

impl GraphState {
    pub fn new() -> Self {
        Self {
            id: next_graph_id(),
            title: "Title".to_string(),
            series: Vec::new(),
            series_counter: 0,
            x_axis_is_datetime: None,
            x_axis_name: None,
            x_axis_unit: None,
            y_axis_name: None,
            z_axis_name: None,
            auto_scale_y: true,
            y_axes: HashMap::new(),
            sync_partner_ids: Vec::new(),
            sync_group_id: None,
            show_data_table: false,
            plot_mode: PlotMode::default(),
            is_updating_range: false,
            table_sort: None,
            plot_view: PlotViewState::new(),
            camera: OrbitalCamera::default(),
            cursor_state: CursorState::default(),
            remove_series_selected: Vec::new(),
            last_frame_rect: None,
        }
    }

    pub fn add_series(&mut self, mut series: DataSeries) {
        // Assign color if not already set meaningfully (all zeros means unset)
        if series.color == [0, 0, 0, 0] {
            series.color = color_for_index(self.series_counter);
        }
        self.series_counter += 1;

        // Ensure y-axis entry exists for this unit
        if !self.y_axes.contains_key(&series.unit) {
            self.y_axes.insert(
                series.unit.clone(),
                AxisState {
                    label: format!("Y Axis ({})", series.unit),
                    ..Default::default()
                },
            );
        }

        self.series.push(series);
    }

    pub fn remove_series_by_id(&mut self, series_id: u64) {
        if let Some(pos) = self.series.iter().position(|s| s.id == series_id) {
            let removed = self.series.remove(pos);
            // Clean up y-axis if no more series use this unit
            let unit_still_used = self.series.iter().any(|s| s.unit == removed.unit);
            if !unit_still_used {
                self.y_axes.remove(&removed.unit);
            }
        }
    }

    pub fn series_labels(&self) -> Vec<String> {
        self.series.iter().map(|s| s.label.clone()).collect()
    }

    /// Get the global x range across all visible series
    pub fn x_range(&self) -> Option<(f64, f64)> {
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for s in &self.series {
            if !s.visible || s.x.is_empty() {
                continue;
            }
            for &v in &s.x {
                if v.is_finite() {
                    min = min.min(v);
                    max = max.max(v);
                }
            }
        }
        if min.is_finite() && max.is_finite() {
            Some((min, max))
        } else {
            None
        }
    }

    /// Get the y range for a given unit within the given x range
    pub fn y_range_for_unit(&self, unit: &str, x_min: f64, x_max: f64) -> Option<(f64, f64)> {
        let mut y_min = f64::INFINITY;
        let mut y_max = f64::NEG_INFINITY;
        for s in &self.series {
            if !s.visible || s.unit != unit {
                continue;
            }
            for (xv, yv) in s.x.iter().zip(s.y.iter()) {
                if *xv >= x_min && *xv <= x_max && yv.is_finite() {
                    y_min = y_min.min(*yv);
                    y_max = y_max.max(*yv);
                }
            }
        }
        if y_min.is_finite() && y_max.is_finite() {
            let padding = (y_max - y_min) * 0.05;
            Some((y_min - padding, y_max + padding))
        } else {
            None
        }
    }
}

impl Default for GraphState {
    fn default() -> Self {
        Self::new()
    }
}
