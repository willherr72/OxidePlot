//! Standalone, egui-free 2D GPU plot renderer.
//!
//! [`PlotRenderer`] owns its own wgpu [`Device`]/[`Queue`], a [`RenderTarget`]
//! (a surface in the MVP), and the line/point pipelines. It replaces the egui
//! `CallbackTrait` integration used by the legacy crate: callers build draw
//! calls with [`PlotRenderer::build_draw_calls`] and submit a frame with
//! [`PlotRenderer::render`].
//!
//! Surface initialization mirrors the wgpu-24 spike in `oxideplot-wasm`
//! (`request_adapter` → `Option<Adapter>`, `request_device(&desc, None)`,
//! `SurfaceConfiguration { desired_maximum_frame_latency, .. }`), so the same
//! code path compiles for both native and `wasm32-unknown-unknown`.

use wgpu::util::DeviceExt;
use wgpu::{
    BindGroupLayout, Device, Instance, Queue, RenderPipeline, Surface, SurfaceConfiguration,
    SurfaceError, TextureFormat,
};

use super::gpu_plot::{create_pipelines, create_storage_buffer, DrawCall, PipelineType};
use super::gpu_types::{DrawMode, GridGpuData, PlotUniforms, SeriesGpuData};

/// Where a [`PlotRenderer`] draws its frames.
pub enum RenderTarget {
    /// A presentable swapchain surface plus its current configuration.
    Surface {
        surface: Surface<'static>,
        config: SurfaceConfiguration,
    },
    // Offscreen { texture: Texture, ... } // MVP: not implemented (MCP follow-up)
}

/// A self-contained 2D plot renderer that owns its GPU device/queue/target.
pub struct PlotRenderer {
    pub device: Device,
    pub queue: Queue,
    pub target: RenderTarget,
    pub format: TextureFormat,
    line_pipeline: RenderPipeline,
    point_pipeline: RenderPipeline,
    bind_group_layout: BindGroupLayout,
}

impl PlotRenderer {
    /// Create a renderer that presents to `surface`.
    ///
    /// Requests an adapter compatible with the surface, opens a device + queue,
    /// configures the surface at `width` x `height`, and builds the 2D
    /// pipelines. Async on both native and wasm.
    pub async fn new_for_surface(
        instance: &Instance,
        surface: Surface<'static>,
        width: u32,
        height: u32,
    ) -> Self {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await
            .expect("no compatible wgpu adapter found");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await
            .expect("failed to request wgpu device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats[0];

        let config = SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let (line_pipeline, point_pipeline, bind_group_layout) =
            create_pipelines(&device, format);

        Self {
            device,
            queue,
            target: RenderTarget::Surface { surface, config },
            format,
            line_pipeline,
            point_pipeline,
            bind_group_layout,
        }
    }

    /// Reconfigure the target surface to a new size. No-op on zero dimensions.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        match &mut self.target {
            RenderTarget::Surface { surface, config } => {
                config.width = width;
                config.height = height;
                surface.configure(&self.device, config);
            }
        }
    }

    /// Build the per-frame draw calls (uniform + storage buffers and their bind
    /// groups) for the grid and each data series.
    ///
    /// This is the egui-free port of the legacy `CallbackTrait::prepare` body;
    /// the per-series/grid buffer construction is identical, but uses
    /// `&self.device` and `self.bind_group_layout` instead of egui callback
    /// resources.
    pub fn build_draw_calls(
        &self,
        series: &[SeriesGpuData],
        grid: &GridGpuData,
        uniforms_base: PlotUniforms,
    ) -> Vec<DrawCall> {
        let device = &self.device;
        let mut draw_calls: Vec<DrawCall> = Vec::new();

        // -- Grid lines --------------------------------------------------
        if grid.segments.len() >= 2 {
            let storage_data: &[u8] = bytemuck::cast_slice(&grid.segments);
            let storage_buf = create_storage_buffer(device, "grid_storage", storage_data);

            let mut uniforms = uniforms_base;
            uniforms.color = grid.color;
            uniforms.line_width = grid.line_width;

            let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("grid_uniform"),
                contents: bytemuck::bytes_of(&uniforms),
                usage: wgpu::BufferUsages::UNIFORM,
            });

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("grid_bind_group"),
                layout: &self.bind_group_layout,
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

            let instance_count = (grid.segments.len() / 2) as u32;
            draw_calls.push(DrawCall {
                bind_group,
                instance_count,
                pipeline_type: PipelineType::Line,
            });
        }

        // -- Data series -------------------------------------------------
        for series in series {
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

                    let mut uniforms = uniforms_base;
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
                        layout: &self.bind_group_layout,
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

                    let mut uniforms = uniforms_base;
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
                        layout: &self.bind_group_layout,
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

                    let mut uniforms = uniforms_base;
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
                        layout: &self.bind_group_layout,
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

        draw_calls
    }

    /// Render `draw_calls` to the target surface, clearing to `clear` (RGBA).
    ///
    /// This is the egui-free port of the legacy `CallbackTrait::paint` body: it
    /// acquires the surface texture, opens a render pass over the full canvas,
    /// issues the `set_pipeline`/`set_bind_group`/`draw(0..6, 0..n)` loop, then
    /// submits and presents. Surface errors are propagated rather than
    /// `unwrap`ped on the hot path.
    pub fn render(&self, draw_calls: &[DrawCall], clear: [f64; 4]) -> Result<(), SurfaceError> {
        let RenderTarget::Surface { surface, .. } = &self.target;

        let frame = surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("plot_encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("plot_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0],
                            g: clear[1],
                            b: clear[2],
                            a: clear[3],
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            for call in draw_calls {
                match call.pipeline_type {
                    PipelineType::Line => pass.set_pipeline(&self.line_pipeline),
                    PipelineType::Point => pass.set_pipeline(&self.point_pipeline),
                }
                pass.set_bind_group(0, &call.bind_group, &[]);
                pass.draw(0..6, 0..call.instance_count);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}
