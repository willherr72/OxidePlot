use egui::{Color32, Visuals};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    Dark,
    Light,
}

impl Theme {
    pub fn toggle(&self) -> Self {
        match self {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        }
    }

    pub fn visuals(&self) -> Visuals {
        match self {
            Theme::Dark => Visuals::dark(),
            Theme::Light => Visuals::light(),
        }
    }

    pub fn plot_bg(&self) -> Color32 {
        match self {
            Theme::Dark => Color32::from_rgb(20, 20, 20),
            Theme::Light => Color32::from_rgb(255, 255, 255),
        }
    }

    pub fn grid_color(&self) -> Color32 {
        match self {
            Theme::Dark => Color32::from_rgba_premultiplied(100, 100, 100, 60),
            Theme::Light => Color32::from_rgba_premultiplied(180, 180, 180, 80),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Theme::Dark => "Dark",
            Theme::Light => "Light",
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::Dark
    }
}
