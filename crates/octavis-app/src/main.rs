mod app;
mod camera;
mod editor;
mod gizmo;
mod picking;
mod render;
mod scene;

use eframe::egui_wgpu::{self, wgpu};

fn main() -> eframe::Result {
    // Vulkan crashes at startup on some Windows drivers, so default to DX12
    // there. WGPU_BACKEND still overrides.
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    if cfg!(windows) && wgpu::Backends::from_env().is_none() {
        setup.instance_descriptor.backends = wgpu::Backends::DX12;
    }

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        depth_buffer: 32,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            wgpu_setup: setup.into(),
            ..Default::default()
        },
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Octavis")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native("Octavis", options, Box::new(|cc| Ok(Box::new(app::OctavisApp::new(cc)))))
}
