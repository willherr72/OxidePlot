use std::sync::Mutex;

use eframe::egui;
use eframe::egui_wgpu;
use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;

use crate::render::gpu_types::{Line3DData, Plot3DUniforms, Scatter3DData};

// ---------------------------------------------------------------------------
// WGSL shader sources (embedded)
// ---------------------------------------------------------------------------

const SCENE_SHADER_SRC: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    color: vec4<f32>,
    resolution: vec2<f32>,
    point_size: f32,
    line_width: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var<storage, read> positions: array<vec4<f32>>;

// ---- Scatter points (instanced billboard quads) ----

struct ScatterOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_scatter(
    @builtin(instance_index) inst: u32,
    @builtin(vertex_index) vert: u32,
) -> ScatterOutput {
    let world_pos = vec4<f32>(positions[inst].xyz, 1.0);
    let clip = u.view_proj * world_pos;

    // Convert clip space to screen pixels (NDC -> pixels).
    let ndc = clip.xy / clip.w;
    let screen = (ndc * 0.5 + 0.5) * u.resolution;

    // Build billboard quad corners.
    var uv: vec2<f32>;
    switch vert {
        case 0u: { uv = vec2<f32>(-1.0, -1.0); }
        case 1u: { uv = vec2<f32>( 1.0, -1.0); }
        case 2u: { uv = vec2<f32>(-1.0,  1.0); }
        case 3u: { uv = vec2<f32>( 1.0, -1.0); }
        case 4u: { uv = vec2<f32>( 1.0,  1.0); }
        case 5u: { uv = vec2<f32>(-1.0,  1.0); }
        default: { uv = vec2<f32>(0.0, 0.0); }
    }

    let offset_px = uv * u.point_size;
    let final_screen = screen + offset_px;

    // Convert back from screen pixels to NDC.
    let final_ndc = (final_screen / u.resolution) * 2.0 - vec2<f32>(1.0, 1.0);

    // Reconstruct clip space preserving original z and w for depth.
    let out_clip = vec4<f32>(final_ndc * clip.w, clip.z, clip.w);

    var out: ScatterOutput;
    out.pos = out_clip;
    out.color = u.color;
    out.uv = uv;
    return out;
}

@fragment
fn fs_scatter(
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

// ---- Line segments (instanced camera-facing ribbon) ----

struct LineOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(
    @builtin(instance_index) inst: u32,
    @builtin(vertex_index) vert: u32,
) -> LineOutput {
    // Storage holds pairs: [start, end, start, end, ...]
    let p0_world = vec4<f32>(positions[inst * 2u].xyz, 1.0);
    let p1_world = vec4<f32>(positions[inst * 2u + 1u].xyz, 1.0);

    let clip0 = u.view_proj * p0_world;
    let clip1 = u.view_proj * p1_world;

    // Convert to screen pixels.
    let ndc0 = clip0.xy / clip0.w;
    let ndc1 = clip1.xy / clip1.w;
    let screen0 = (ndc0 * 0.5 + 0.5) * u.resolution;
    let screen1 = (ndc1 * 0.5 + 0.5) * u.resolution;

    // Direction along the segment in screen space.
    let dir = screen1 - screen0;
    let len = length(dir);

    // Perpendicular direction for ribbon width.
    var perp: vec2<f32>;
    if len > 0.001 {
        perp = vec2<f32>(-dir.y, dir.x) / len * u.line_width * 0.5;
    } else {
        perp = vec2<f32>(0.0, u.line_width * 0.5);
    }

    // Build the quad: 6 vertices per segment (two triangles).
    // t=0 means endpoint 0, t=1 means endpoint 1. side = +/- 1.
    var t: f32;
    var side: f32;
    switch vert {
        case 0u: { t = 0.0; side =  1.0; }
        case 1u: { t = 0.0; side = -1.0; }
        case 2u: { t = 1.0; side =  1.0; }
        case 3u: { t = 0.0; side = -1.0; }
        case 4u: { t = 1.0; side = -1.0; }
        case 5u: { t = 1.0; side =  1.0; }
        default: { t = 0.0; side = 1.0; }
    }

    // Interpolate screen position along the segment.
    let base_screen = mix(screen0, screen1, t);
    let final_screen = base_screen + perp * side;

    // Convert back to NDC.
    let final_ndc = (final_screen / u.resolution) * 2.0 - vec2<f32>(1.0, 1.0);

    // Perspective-correct depth interpolation.
    // Interpolate 1/w linearly, then recover w and z.
    let inv_w0 = 1.0 / clip0.w;
    let inv_w1 = 1.0 / clip1.w;
    let inv_w = mix(inv_w0, inv_w1, t);
    let w = 1.0 / inv_w;

    // z/w also interpolates linearly in screen space, so interpolate z/w.
    let z_over_w_0 = clip0.z / clip0.w;
    let z_over_w_1 = clip1.z / clip1.w;
    let z_over_w = mix(z_over_w_0, z_over_w_1, t);
    let z = z_over_w * w;

    let out_clip = vec4<f32>(final_ndc * w, z, w);

    var out: LineOutput;
    out.pos = out_clip;
    out.color = u.color;
    return out;
}

@fragment
fn fs_line(
    @location(0) color: vec4<f32>,
) -> @location(0) vec4<f32> {
    return color;
}
"#;

const BLIT_SHADER_SRC: &str = r#"
struct BlitOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_blit(
    @builtin(vertex_index) vert: u32,
) -> BlitOutput {
    // Fullscreen quad: 6 vertices, two triangles.
    var positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
    );
    var uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 0.0),
    );

    var out: BlitOutput;
    out.pos = vec4<f32>(positions[vert], 0.0, 1.0);
    out.uv = uvs[vert];
    return out;
}

@group(0) @binding(0) var t_color: texture_2d<f32>;
@group(0) @binding(1) var s_color: sampler;

@fragment
fn fs_blit(
    @location(0) uv: vec2<f32>,
) -> @location(0) vec4<f32> {
    return textureSample(t_color, s_color, uv);
}
"#;

// ---------------------------------------------------------------------------
// Persistent GPU resources (stored once in CallbackResources)
// ---------------------------------------------------------------------------

pub struct Plot3DResources {
    pub scatter_pipeline: wgpu::RenderPipeline,
    pub line_pipeline: wgpu::RenderPipeline,
    pub blit_pipeline: wgpu::RenderPipeline,
    pub scene_bind_group_layout: wgpu::BindGroupLayout,
    pub blit_bind_group_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    /// The texture format used by the render target. The offscreen color
    /// texture must match this so it's compatible with the scene pipelines.
    pub target_format: wgpu::TextureFormat,
}

// ---------------------------------------------------------------------------
// Blit state: created during prepare(), consumed during paint()
// ---------------------------------------------------------------------------

struct BlitState {
    blit_bind_group: wgpu::BindGroup,
}

// ---------------------------------------------------------------------------
// Cached offscreen textures (avoid recreating every frame)
// ---------------------------------------------------------------------------

pub struct CachedOffscreenTextures {
    pub color_view: wgpu::TextureView,
    pub depth_view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

pub fn init_3d_resources(render_state: &egui_wgpu::RenderState) {
    let device = &render_state.device;
    let target_format = render_state.target_format;

    // -- Scene shader module --
    let scene_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("plot3d_scene_shader"),
        source: wgpu::ShaderSource::Wgsl(SCENE_SHADER_SRC.into()),
    });

    // -- Blit shader module --
    let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("plot3d_blit_shader"),
        source: wgpu::ShaderSource::Wgsl(BLIT_SHADER_SRC.into()),
    });

    // -- Scene bind group layout (uniform + storage) --
    let scene_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("plot3d_scene_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(
                            std::mem::size_of::<Plot3DUniforms>() as u64,
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
                        min_binding_size: std::num::NonZeroU64::new(16), // at least one vec4<f32>
                    },
                    count: None,
                },
            ],
        });

    // -- Blit bind group layout (texture + sampler) --
    let blit_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("plot3d_blit_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

    // -- Pipeline layouts --
    let scene_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("plot3d_scene_pipeline_layout"),
        bind_group_layouts: &[&scene_bind_group_layout],
        push_constant_ranges: &[],
    });

    let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("plot3d_blit_pipeline_layout"),
        bind_group_layouts: &[&blit_bind_group_layout],
        push_constant_ranges: &[],
    });

    // -- Shared state for scene pipelines --
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

    // -- Scatter pipeline (depth test Less, depth write enabled) --
    let scatter_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("plot3d_scatter_pipeline"),
        layout: Some(&scene_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &scene_shader,
            entry_point: Some("vs_scatter"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive,
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample,
        fragment: Some(wgpu::FragmentState {
            module: &scene_shader,
            entry_point: Some("fs_scatter"),
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

    // -- Line pipeline (depth test Less, depth write enabled) --
    let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("plot3d_line_pipeline"),
        layout: Some(&scene_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &scene_shader,
            entry_point: Some("vs_line"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive,
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample,
        fragment: Some(wgpu::FragmentState {
            module: &scene_shader,
            entry_point: Some("fs_line"),
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

    // -- Blit pipeline (no depth, renders to target_format) --
    let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("plot3d_blit_pipeline"),
        layout: Some(&blit_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &blit_shader,
            entry_point: Some("vs_blit"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: None,
        multisample,
        fragment: Some(wgpu::FragmentState {
            module: &blit_shader,
            entry_point: Some("fs_blit"),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview: None,
        cache: None,
    });

    // -- Sampler for blit --
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("plot3d_blit_sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

    let resources = Plot3DResources {
        scatter_pipeline,
        line_pipeline,
        blit_pipeline,
        scene_bind_group_layout,
        blit_bind_group_layout,
        sampler,
        target_format,
    };

    render_state
        .renderer
        .write()
        .callback_resources
        .insert(resources);
}

// ---------------------------------------------------------------------------
// Helper: create a storage buffer with at least 16 bytes for vec4 alignment
// ---------------------------------------------------------------------------

fn create_storage_buffer_3d(device: &wgpu::Device, label: &str, data: &[u8]) -> wgpu::Buffer {
    if data.len() < 16 {
        let mut padded = vec![0u8; 16];
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

pub struct Plot3DCallback {
    pub scatter_data: Vec<Scatter3DData>,
    pub line_data: Vec<Line3DData>,
    pub uniforms_base: Plot3DUniforms,
    pub bg_color: [f32; 4],
    pub viewport_size: [u32; 2],
    blit_state: Mutex<Option<BlitState>>,
}

impl egui_wgpu::CallbackTrait for Plot3DCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        // Extract target_format without holding a long-lived borrow on
        // callback_resources, so we can call insert() below if textures
        // need recreating.
        let target_format = match callback_resources.get::<Plot3DResources>() {
            Some(r) => r.target_format,
            None => return Vec::new(),
        };

        let width = self.viewport_size[0].max(1);
        let height = self.viewport_size[1].max(1);

        // -- Get or create cached offscreen textures --
        // Only recreate when size changes to avoid exhausting VRAM.
        let needs_recreate = match callback_resources.get::<CachedOffscreenTextures>() {
            Some(cached) => cached.width != width || cached.height != height || cached.format != target_format,
            None => true,
        };

        if needs_recreate {
            let color_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("plot3d_offscreen_color"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: target_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let color_view = color_texture.create_view(&wgpu::TextureViewDescriptor::default());

            let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("plot3d_offscreen_depth"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

            callback_resources.insert(CachedOffscreenTextures {
                color_view,
                depth_view,
                width,
                height,
                format: target_format,
            });
        }

        // Now safe to hold immutable borrows — no more inserts after this point.
        let resources = callback_resources.get::<Plot3DResources>().unwrap();
        let cached = callback_resources.get::<CachedOffscreenTextures>().unwrap();
        let color_view = &cached.color_view;
        let depth_view = &cached.depth_view;

        // -- Begin offscreen render pass --
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("plot3d_offscreen_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: self.bg_color[0] as f64,
                            g: self.bg_color[1] as f64,
                            b: self.bg_color[2] as f64,
                            a: self.bg_color[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);

            // -- Draw line segments (grid lines first, then series lines) --
            for line in &self.line_data {
                if line.segments.len() < 2 {
                    continue;
                }

                let storage_data: &[u8] = bytemuck::cast_slice(&line.segments);
                let storage_buf =
                    create_storage_buffer_3d(device, "plot3d_line_storage", storage_data);

                let mut uniforms = self.uniforms_base;
                uniforms.color = line.color;
                uniforms.line_width = line.line_width;
                uniforms.resolution = [width as f32, height as f32];

                let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("plot3d_line_uniform"),
                    contents: bytemuck::bytes_of(&uniforms),
                    usage: wgpu::BufferUsages::UNIFORM,
                });

                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("plot3d_line_bind_group"),
                    layout: &resources.scene_bind_group_layout,
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

                let instance_count = (line.segments.len() / 2) as u32;
                render_pass.set_pipeline(&resources.line_pipeline);
                render_pass.set_bind_group(0, &bind_group, &[]);
                render_pass.draw(0..6, 0..instance_count);
            }

            // -- Draw scatter points --
            for scatter in &self.scatter_data {
                if scatter.positions.is_empty() {
                    continue;
                }

                let storage_data: &[u8] = bytemuck::cast_slice(&scatter.positions);
                let storage_buf =
                    create_storage_buffer_3d(device, "plot3d_scatter_storage", storage_data);

                let mut uniforms = self.uniforms_base;
                uniforms.color = scatter.color;
                uniforms.point_size = scatter.point_size;
                uniforms.resolution = [width as f32, height as f32];

                let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("plot3d_scatter_uniform"),
                    contents: bytemuck::bytes_of(&uniforms),
                    usage: wgpu::BufferUsages::UNIFORM,
                });

                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("plot3d_scatter_bind_group"),
                    layout: &resources.scene_bind_group_layout,
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

                let instance_count = scatter.positions.len() as u32;
                render_pass.set_pipeline(&resources.scatter_pipeline);
                render_pass.set_bind_group(0, &bind_group, &[]);
                render_pass.draw(0..6, 0..instance_count);
            }
        }
        // render_pass is dropped here, ending the offscreen pass.

        // -- Create blit bind group --
        let blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("plot3d_blit_bind_group"),
            layout: &resources.blit_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&resources.sampler),
                },
            ],
        });

        *self.blit_state.lock().unwrap() = Some(BlitState { blit_bind_group });

        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(resources) = callback_resources.get::<Plot3DResources>() else {
            return;
        };

        let state_guard = self.blit_state.lock().unwrap();
        let Some(state) = state_guard.as_ref() else {
            return;
        };

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

        render_pass.set_pipeline(&resources.blit_pipeline);
        render_pass.set_bind_group(0, &state.blit_bind_group, &[]);
        render_pass.draw(0..6, 0..1);
    }
}

// ---------------------------------------------------------------------------
// Convenience helper
// ---------------------------------------------------------------------------

pub fn create_3d_paint_callback(
    rect: egui::Rect,
    scatter_data: Vec<Scatter3DData>,
    line_data: Vec<Line3DData>,
    uniforms_base: Plot3DUniforms,
    bg_color: [f32; 4],
    viewport_size: [u32; 2],
) -> egui::PaintCallback {
    egui_wgpu::Callback::new_paint_callback(
        rect,
        Plot3DCallback {
            scatter_data,
            line_data,
            uniforms_base,
            bg_color,
            viewport_size,
            blit_state: Mutex::new(None),
        },
    )
}
