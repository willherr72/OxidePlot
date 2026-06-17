use std::sync::Mutex;

use eframe::egui;
use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;
use eframe::egui_wgpu;

use super::gpu_types::{DrawMode, GridGpuData, PlotUniforms, SeriesGpuData};

// ---------------------------------------------------------------------------
// WGSL shader source (embedded)
// ---------------------------------------------------------------------------

const PLOT_SHADER_SRC: &str = r#"
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
// Persistent GPU resources (stored once in CallbackResources)
// ---------------------------------------------------------------------------

pub struct GpuPlotResources {
    pub line_pipeline: wgpu::RenderPipeline,
    pub point_pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
}

// ---------------------------------------------------------------------------
// Per-callback draw data
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineType {
    Line,
    Point,
}

pub struct DrawCall {
    pub bind_group: wgpu::BindGroup,
    pub instance_count: u32,
    pub pipeline_type: PipelineType,
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

pub fn init_gpu_resources(render_state: &egui_wgpu::RenderState) {
    let device = &render_state.device;
    let target_format = render_state.target_format;

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
                format: target_format,
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
                format: target_format,
                blend: Some(blend_state),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview: None,
        cache: None,
    });

    let resources = GpuPlotResources {
        line_pipeline,
        point_pipeline,
        bind_group_layout,
    };

    render_state
        .renderer
        .write()
        .callback_resources
        .insert(resources);
}

// ---------------------------------------------------------------------------
// Helper: create a storage buffer with at least min_binding_size bytes
// ---------------------------------------------------------------------------

fn create_storage_buffer(device: &wgpu::Device, label: &str, data: &[u8]) -> wgpu::Buffer {
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

// ---------------------------------------------------------------------------
// Paint callback
// ---------------------------------------------------------------------------

/// Per-frame callback. Stores draw calls in a Mutex so each plot instance
/// is self-contained (no shared TypeMap collisions between multiple plots).
pub struct GpuPlotCallback {
    pub series_data: Vec<SeriesGpuData>,
    pub grid_data: GridGpuData,
    pub uniforms_base: PlotUniforms,
    /// Filled by prepare(), consumed by paint(). Per-callback, not shared.
    draw_calls: Mutex<Vec<DrawCall>>,
}

impl egui_wgpu::CallbackTrait for GpuPlotCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(resources) = callback_resources.get::<GpuPlotResources>() else {
            return Vec::new();
        };

        let mut draw_calls: Vec<DrawCall> = Vec::new();

        // -- Grid lines --------------------------------------------------
        if self.grid_data.segments.len() >= 2 {
            let storage_data: &[u8] = bytemuck::cast_slice(&self.grid_data.segments);
            let storage_buf = create_storage_buffer(device, "grid_storage", storage_data);

            let mut uniforms = self.uniforms_base;
            uniforms.color = self.grid_data.color;
            uniforms.line_width = self.grid_data.line_width;

            let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("grid_uniform"),
                contents: bytemuck::bytes_of(&uniforms),
                usage: wgpu::BufferUsages::UNIFORM,
            });

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("grid_bind_group"),
                layout: &resources.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: storage_buf.as_entire_binding(),
                    },
                ],
            });

            let instance_count = (self.grid_data.segments.len() / 2) as u32;
            draw_calls.push(DrawCall {
                bind_group,
                instance_count,
                pipeline_type: PipelineType::Line,
            });
        }

        // -- Data series -------------------------------------------------
        for series in &self.series_data {
            if series.points.is_empty() {
                continue;
            }
            if series.points.len() < 2 && series.draw_mode != DrawMode::Points {
                continue;
            }

            match series.draw_mode {
                DrawMode::Lines => {
                    let mut pairs: Vec<[f32; 2]> =
                        Vec::with_capacity((series.points.len() - 1) * 2);
                    for i in 0..series.points.len() - 1 {
                        pairs.push(series.points[i]);
                        pairs.push(series.points[i + 1]);
                    }
                    if pairs.is_empty() {
                        continue;
                    }

                    let storage_data: &[u8] = bytemuck::cast_slice(&pairs);
                    let storage_buf =
                        create_storage_buffer(device, "series_line_storage", storage_data);

                    let mut uniforms = self.uniforms_base;
                    uniforms.color = series.color;
                    uniforms.line_width = series.line_width;

                    let uniform_buf =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("series_line_uniform"),
                            contents: bytemuck::bytes_of(&uniforms),
                            usage: wgpu::BufferUsages::UNIFORM,
                        });

                    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("series_line_bg"),
                        layout: &resources.bind_group_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: uniform_buf.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: storage_buf.as_entire_binding(),
                            },
                        ],
                    });

                    draw_calls.push(DrawCall {
                        bind_group,
                        instance_count: (pairs.len() / 2) as u32,
                        pipeline_type: PipelineType::Line,
                    });
                }

                DrawMode::Step => {
                    let mut step_points: Vec<[f32; 2]> = Vec::new();
                    for i in 0..series.points.len() - 1 {
                        let p0 = series.points[i];
                        let p1 = series.points[i + 1];
                        let mid = [p1[0], p0[1]];
                        step_points.push(p0);
                        step_points.push(mid);
                        step_points.push(mid);
                        step_points.push(p1);
                    }
                    if step_points.len() < 2 {
                        continue;
                    }

                    let storage_data: &[u8] = bytemuck::cast_slice(&step_points);
                    let storage_buf =
                        create_storage_buffer(device, "series_step_storage", storage_data);

                    let mut uniforms = self.uniforms_base;
                    uniforms.color = series.color;
                    uniforms.line_width = series.line_width;

                    let uniform_buf =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("series_step_uniform"),
                            contents: bytemuck::bytes_of(&uniforms),
                            usage: wgpu::BufferUsages::UNIFORM,
                        });

                    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("series_step_bg"),
                        layout: &resources.bind_group_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: uniform_buf.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: storage_buf.as_entire_binding(),
                            },
                        ],
                    });

                    draw_calls.push(DrawCall {
                        bind_group,
                        instance_count: (step_points.len() / 2) as u32,
                        pipeline_type: PipelineType::Line,
                    });
                }

                DrawMode::Points => {
                    let storage_data: &[u8] = bytemuck::cast_slice(&series.points);
                    let storage_buf =
                        create_storage_buffer(device, "series_point_storage", storage_data);

                    let mut uniforms = self.uniforms_base;
                    uniforms.color = series.color;
                    uniforms.point_radius = series.point_radius;

                    let uniform_buf =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("series_point_uniform"),
                            contents: bytemuck::bytes_of(&uniforms),
                            usage: wgpu::BufferUsages::UNIFORM,
                        });

                    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("series_point_bg"),
                        layout: &resources.bind_group_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: uniform_buf.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: storage_buf.as_entire_binding(),
                            },
                        ],
                    });

                    draw_calls.push(DrawCall {
                        bind_group,
                        instance_count: series.points.len() as u32,
                        pipeline_type: PipelineType::Point,
                    });
                }
            }
        }

        // Store in our own Mutex (not the shared TypeMap).
        *self.draw_calls.lock().unwrap() = draw_calls;

        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(resources) = callback_resources.get::<GpuPlotResources>() else {
            return;
        };

        let calls = self.draw_calls.lock().unwrap();
        if calls.is_empty() {
            return;
        }

        let viewport = info.viewport_in_pixels();
        if viewport.width_px <= 0 || viewport.height_px <= 0 {
            return;
        }
        render_pass.set_viewport(
            viewport.left_px as f32,
            viewport.top_px as f32,
            viewport.width_px as f32,
            viewport.height_px as f32,
            0.0,
            1.0,
        );

        let clip = info.clip_rect_in_pixels();
        if clip.width_px > 0 && clip.height_px > 0 {
            render_pass.set_scissor_rect(
                clip.left_px as u32,
                clip.top_px as u32,
                clip.width_px as u32,
                clip.height_px as u32,
            );
        }

        for call in calls.iter() {
            match call.pipeline_type {
                PipelineType::Line => render_pass.set_pipeline(&resources.line_pipeline),
                PipelineType::Point => render_pass.set_pipeline(&resources.point_pipeline),
            }
            render_pass.set_bind_group(0, &call.bind_group, &[]);
            render_pass.draw(0..6, 0..call.instance_count);
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience helper
// ---------------------------------------------------------------------------

pub fn create_plot_paint_callback(
    rect: egui::Rect,
    series_data: Vec<SeriesGpuData>,
    grid_data: GridGpuData,
    uniforms_base: PlotUniforms,
) -> egui::PaintCallback {
    egui_wgpu::Callback::new_paint_callback(
        rect,
        GpuPlotCallback {
            series_data,
            grid_data,
            uniforms_base,
            draw_calls: Mutex::new(Vec::new()),
        },
    )
}
