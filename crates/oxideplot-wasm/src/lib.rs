// Task 0.3 — Throwaway spike: prove wgpu-on-wasm renders in WebView2.
// This crate compiles to essentially nothing on native targets (all wasm32-only
// code is cfg-gated). The native `cargo build` of the workspace must succeed.

use wasm_bindgen::prelude::*;

/// Called from JS/Svelte. Draws a cyan triangle on `canvas` using wgpu→wasm.
/// On success returns "WASM_WEBGPU_OK"; on any failure returns an error string.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub async fn run_triangle(canvas: web_sys::HtmlCanvasElement) -> String {
    console_error_panic_hook::set_once();

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::BROWSER_WEBGPU,
        ..Default::default()
    });

    let surface_target = wgpu::SurfaceTarget::Canvas(canvas);
    let surface = match instance.create_surface(surface_target) {
        Ok(s) => s,
        Err(e) => return format!("WASM_SURFACE_ERR: {e}"),
    };

    let adapter = match instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        })
        .await
    {
        Some(a) => a,
        None => return "WASM_NO_ADAPTER".to_string(),
    };

    let (device, queue) = match adapter
        .request_device(&wgpu::DeviceDescriptor::default(), None)
        .await
    {
        Ok(dq) => dq,
        Err(e) => return format!("WASM_DEVICE_ERR: {e}"),
    };

    let caps = surface.get_capabilities(&adapter);
    let format = caps.formats[0];

    let width = 640u32;
    let height = 480u32;
    surface.configure(
        &device,
        &wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        },
    );

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("spike_shader"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var p = array(vec2(0.0,0.5), vec2(-0.5,-0.5), vec2(0.5,-0.5));
    return vec4(p[i], 0.0, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> { return vec4(0.2,0.8,1.0,1.0); }
"#
            .into(),
        ),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("spike_pipeline"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            targets: &[Some(format.into())],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    let frame = match surface.get_current_texture() {
        Ok(f) => f,
        Err(e) => return format!("WASM_FRAME_ERR: {e}"),
    };
    let view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("spike_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.draw(0..3, 0..1);
    }
    queue.submit(std::iter::once(enc.finish()));
    frame.present();

    "WASM_WEBGPU_OK".to_string()
}

/// Stub exported when building for native targets so the crate compiles cleanly.
#[cfg(not(target_arch = "wasm32"))]
#[wasm_bindgen]
pub fn run_triangle_stub() -> String {
    "NATIVE_STUB".to_string()
}
