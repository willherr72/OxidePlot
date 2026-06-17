//! Egui-free GPU plot primitives for the standalone [`crate::render::renderer::PlotRenderer`].
//!
//! This module contains the reusable, egui-independent pieces copied and adapted
//! from the legacy `oxideplot-egui-legacy` renderer: the WGSL shader source, the
//! draw-call types, the storage-buffer helper, and the 2D line/point pipeline
//! creation. The legacy crate still carries its own `egui_wgpu::CallbackTrait`
//! variant of this code (the Phase 6 parity reference); the WGSL string is
//! intentionally duplicated until the legacy crate is removed in Task 6.3.

use wgpu::util::DeviceExt;

use super::gpu_types::PlotUniforms;

// ---------------------------------------------------------------------------
// WGSL shader source (embedded)
//
// NOTE: This string is byte-for-byte identical to the one in the legacy crate's
// `render/gpu_plot.rs`. The duplication is deliberate (see module docs) and
// disappears when legacy is deleted in Task 6.3.
// ---------------------------------------------------------------------------

pub const PLOT_SHADER_SRC: &str = r#"
struct Uniforms {
    view_min: vec2<f32>,
    view_max: vec2<f32>,
    resolution: vec2<f32>,
    line_width: f32,
    point_radius: f32,
    color: vec4<f32>,
    _padding: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var<storage, read> points: array<vec2<f32>>;

struct LineOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

fn data_to_ndc(p: vec2<f32>) -> vec2<f32> {
    let range = u.view_max - u.view_min;
    let safe_range = select(range, vec2<f32>(1.0), abs(range) < vec2<f32>(1e-10));
    return (p - u.view_min) / safe_range * 2.0 - vec2<f32>(1.0, 1.0);
}

fn ndc_to_px(ndc: vec2<f32>) -> vec2<f32> {
    return (ndc * 0.5 + 0.5) * u.resolution;
}

fn px_to_ndc(px: vec2<f32>) -> vec2<f32> {
    return (px / u.resolution) * 2.0 - vec2<f32>(1.0, 1.0);
}

@vertex
fn vs_line(
    @builtin(instance_index) inst: u32,
    @builtin(vertex_index) vert: u32,
) -> LineOutput {
    // Each instance is one line segment; storage holds pairs: [start, end, start, end, ...]
    let p0 = data_to_ndc(points[inst * 2u]);
    let p1 = data_to_ndc(points[inst * 2u + 1u]);

    let px0 = ndc_to_px(p0);
    let px1 = ndc_to_px(p1);

    let dir = px1 - px0;
    let len = length(dir);
    var perp: vec2<f32>;
    if len > 0.001 {
        perp = vec2<f32>(-dir.y, dir.x) / len * u.line_width * 0.5;
    } else {
        perp = vec2<f32>(0.0, u.line_width * 0.5);
    }

    var base: vec2<f32>;
    var side: f32;
    switch vert {
        case 0u: { base = px0; side = 1.0; }
        case 1u: { base = px0; side = -1.0; }
        case 2u: { base = px1; side = 1.0; }
        case 3u: { base = px0; side = -1.0; }
        case 4u: { base = px1; side = -1.0; }
        case 5u: { base = px1; side = 1.0; }
        default: { base = px0; side = 1.0; }
    }

    let final_px = base + perp * side;
    let final_ndc = px_to_ndc(final_px);

    var out: LineOutput;
    out.pos = vec4<f32>(final_ndc, 0.0, 1.0);
    out.color = u.color;
    return out;
}

@fragment
fn fs_solid(@location(0) color: vec4<f32>) -> @location(0) vec4<f32> {
    return color;
}

struct PointOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_point(
    @builtin(instance_index) inst: u32,
    @builtin(vertex_index) vert: u32,
) -> PointOutput {
    let center = data_to_ndc(points[inst]);
    let center_px = ndc_to_px(center);

    let r = u.point_radius;
    var uv: vec2<f32>;
    switch vert {
        case 0u: { uv = vec2<f32>(-1.0, -1.0); }
        case 1u: { uv = vec2<f32>(1.0, -1.0); }
        case 2u: { uv = vec2<f32>(-1.0, 1.0); }
        case 3u: { uv = vec2<f32>(1.0, -1.0); }
        case 4u: { uv = vec2<f32>(1.0, 1.0); }
        case 5u: { uv = vec2<f32>(-1.0, 1.0); }
        default: { uv = vec2<f32>(0.0, 0.0); }
    }
    let offset = uv * r;

    let pos_px = center_px + offset;
    let pos_ndc = px_to_ndc(pos_px);

    var out: PointOutput;
    out.pos = vec4<f32>(pos_ndc, 0.0, 1.0);
    out.color = u.color;
    out.uv = uv;
    return out;
}

@fragment
fn fs_point(
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
) -> @location(0) vec4<f32> {
    let dist = length(uv);
    if dist > 1.0 {
        discard;
    }
    let alpha = 1.0 - smoothstep(0.8, 1.0, dist);
    return vec4<f32>(color.rgb, color.a * alpha);
}
"#;

// ---------------------------------------------------------------------------
// Per-draw-call types
// ---------------------------------------------------------------------------

/// Which pipeline a [`DrawCall`] should be issued against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineType {
    Line,
    Point,
}

/// A single GPU draw call: a bind group (uniform + storage buffer) plus the
/// instance count and which pipeline to bind. Six vertices per instance.
pub struct DrawCall {
    pub bind_group: wgpu::BindGroup,
    pub instance_count: u32,
    pub pipeline_type: PipelineType,
}

// ---------------------------------------------------------------------------
// Pipeline creation
// ---------------------------------------------------------------------------

/// Create the 2D line and point render pipelines plus the shared bind-group
/// layout, targeting `format`.
///
/// Adapted from legacy `init_gpu_resources`: identical pipeline/layout setup,
/// but takes a plain `&wgpu::Device` + `wgpu::TextureFormat` instead of an
/// `egui_wgpu::RenderState`, and returns the resources instead of stuffing them
/// into egui's callback `TypeMap`.
pub fn create_pipelines(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> (
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::BindGroupLayout,
) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("plot_shader"),
        source: wgpu::ShaderSource::Wgsl(PLOT_SHADER_SRC.into()),
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("plot_bind_group_layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(
                        std::mem::size_of::<PlotUniforms>() as u64,
                    ),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(8), // at least one vec2<f32>
                },
                count: None,
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("plot_pipeline_layout"),
        bind_group_layouts: &[&bind_group_layout],
        push_constant_ranges: &[],
    });

    let primitive = wgpu::PrimitiveState {
        topology: wgpu::PrimitiveTopology::TriangleList,
        strip_index_format: None,
        front_face: wgpu::FrontFace::Ccw,
        cull_mode: None,
        unclipped_depth: false,
        polygon_mode: wgpu::PolygonMode::Fill,
        conservative: false,
    };

    let multisample = wgpu::MultisampleState {
        count: 1,
        mask: !0,
        alpha_to_coverage_enabled: false,
    };

    let blend_state = wgpu::BlendState::ALPHA_BLENDING;

    let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("plot_line_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_line"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive,
        depth_stencil: None,
        multisample,
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_solid"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend_state),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview: None,
        cache: None,
    });

    let point_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("plot_point_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_point"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive,
        depth_stencil: None,
        multisample,
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_point"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend_state),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview: None,
        cache: None,
    });

    (line_pipeline, point_pipeline, bind_group_layout)
}

// ---------------------------------------------------------------------------
// Helper: create a storage buffer with at least min_binding_size bytes
// ---------------------------------------------------------------------------

/// Create a storage buffer, padding to the 8-byte minimum binding size when the
/// supplied data is too small. Copied verbatim from the legacy implementation.
pub(crate) fn create_storage_buffer(
    device: &wgpu::Device,
    label: &str,
    data: &[u8],
) -> wgpu::Buffer {
    // wgpu requires storage buffers to meet min_binding_size (8 bytes).
    // Pad with zeros if the data is too small.
    if data.len() < 8 {
        let mut padded = vec![0u8; 8];
        padded[..data.len()].copy_from_slice(data);
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: &padded,
            usage: wgpu::BufferUsages::STORAGE,
        })
    } else {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: data,
            usage: wgpu::BufferUsages::STORAGE,
        })
    }
}
