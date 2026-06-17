mod state;
mod data;
mod processing;
mod ui;
mod render;
mod plot3d;
mod app;

use app::OxidePlotApp;
use eframe::egui;
use eframe::egui_wgpu;

fn main() -> eframe::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("OxidePlot")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([800.0, 600.0])
            .with_drag_and_drop(true),
        // Configure wgpu for driver stability on Windows.
        wgpu_options: egui_wgpu::WgpuConfiguration {
            present_mode: eframe::wgpu::PresentMode::AutoVsync,
            wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew {
                instance_descriptor: eframe::wgpu::InstanceDescriptor {
                    // Prefer DX12 on Windows for stability; include Vulkan and GL as fallbacks.
                    backends: eframe::wgpu::Backends::DX12
                        | eframe::wgpu::Backends::VULKAN
                        | eframe::wgpu::Backends::GL,
                    ..Default::default()
                },
                power_preference: eframe::wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::run_native(
        "OxidePlot",
        options,
        Box::new(|cc| Ok(Box::new(OxidePlotApp::new(cc)))),
    )
}
