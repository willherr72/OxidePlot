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
    /// A headless offscreen texture (no window). Rendered via `render_to_rgba`;
    /// used by the MCP server and CLI to produce plot images without a surface.
    Offscreen {
        texture: wgpu::Texture,
        width: u32,
        height: u32,
    },
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

    /// Create a headless renderer that draws to an offscreen `width` x `height`
    /// texture (no surface/window). Read the result back with `render_to_rgba`.
    ///
    /// Uses a non-sRGB `Rgba8Unorm` target so clear/colour values are stored
    /// directly (byte = value*255), matching the on-screen appearance and making
    /// the read-back pixels straightforward. Powers the MCP server and CLI.
    pub async fn new_offscreen(width: u32, height: u32) -> Self {
        let w = width.max(1);
        let h = height.max(1);

        let instance = Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .expect("no wgpu adapter found for offscreen rendering");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await
            .expect("failed to request wgpu device");

        let format = TextureFormat::Rgba8Unorm;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen_plot_target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let (line_pipeline, point_pipeline, bind_group_layout) = create_pipelines(&device, format);

        Self {
            device,
            queue,
            target: RenderTarget::Offscreen {
                texture,
                width: w,
                height: h,
            },
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
            // Offscreen renderers are constructed at a fixed size per output;
            // the MCP server / CLI builds a fresh renderer to change size.
            RenderTarget::Offscreen { .. } => {}
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
        let surface = match &self.target {
            RenderTarget::Surface { surface, .. } => surface,
            // Offscreen renderers never present a frame — use `render_to_rgba`.
            RenderTarget::Offscreen { .. } => return Ok(()),
        };

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

    /// Render `draw_calls` to the offscreen target (clearing to `clear`, RGBA in
    /// 0..1) and read the pixels back as a tight `width*height*4` RGBA8 buffer
    /// (row-major, top-to-bottom). Panics if called on a surface renderer.
    ///
    /// This is the headless twin of [`render`]: the render pass is identical, but
    /// the frame is copied into a mappable buffer and read back on the CPU. The
    /// GPU copy requires each row to be padded to `COPY_BYTES_PER_ROW_ALIGNMENT`
    /// (256); the padding is stripped before returning.
    pub fn render_to_rgba(&self, draw_calls: &[DrawCall], clear: [f64; 4]) -> Vec<u8> {
        let (texture, width, height) = match &self.target {
            RenderTarget::Offscreen {
                texture,
                width,
                height,
            } => (texture, *width, *height),
            RenderTarget::Surface { .. } => {
                panic!("render_to_rgba called on a surface renderer; use render()")
            }
        };

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Row padding: bytes_per_row must be a multiple of 256.
        let unpadded_bpr = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bpr = unpadded_bpr.div_ceil(align) * align;
        let buffer_size = (padded_bpr as u64) * (height as u64);

        let out_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen_readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("offscreen_encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("offscreen_pass"),
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

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &out_buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bpr),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(std::iter::once(encoder.finish()));

        // Map the read-back buffer and block until the GPU is done.
        let slice = out_buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        // Block until the GPU has finished and the buffer is mapped.
        self.device.poll(wgpu::Maintain::Wait);

        let mapped = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity((unpadded_bpr * height) as usize);
        for row in 0..height {
            let start = (row * padded_bpr) as usize;
            let end = start + unpadded_bpr as usize;
            pixels.extend_from_slice(&mapped[start..end]);
        }
        drop(mapped);
        out_buf.unmap();

        pixels
    }
}

#[cfg(test)]
mod offscreen_tests {
    use super::*;

    /// Build draw calls for a simple 4-point amber line spanning the viewport.
    fn simple_line_calls(r: &PlotRenderer, w: u32, h: u32) -> Vec<DrawCall> {
        let series = SeriesGpuData {
            points: vec![[0.0, 0.0], [1.0, 1.0], [2.0, 0.2], [3.0, 0.9]],
            color: [1.0, 0.42, 0.17, 1.0],
            line_width: 3.0,
            point_radius: 4.0,
            draw_mode: DrawMode::Lines,
        };
        let grid = GridGpuData {
            segments: vec![],
            color: [0.3, 0.3, 0.3, 1.0],
            line_width: 1.0,
        };
        let uniforms = PlotUniforms {
            view_min: [0.0, -0.1],
            view_max: [3.0, 1.1],
            resolution: [w as f32, h as f32],
            line_width: 3.0,
            point_radius: 4.0,
            color: [0.0, 0.0, 0.0, 0.0],
            _padding: [0.0; 4],
        };
        r.build_draw_calls(&[series], &grid, uniforms)
    }

    fn clear_bytes(clear: [f64; 4]) -> [u8; 3] {
        [
            (clear[0] * 255.0).round() as u8,
            (clear[1] * 255.0).round() as u8,
            (clear[2] * 255.0).round() as u8,
        ]
    }

    #[test]
    fn offscreen_dims_and_nonblank() {
        let r = pollster::block_on(PlotRenderer::new_offscreen(300, 200));
        let calls = simple_line_calls(&r, 300, 200);
        let clear = [0.05, 0.06, 0.08, 1.0];
        let buf = r.render_to_rgba(&calls, clear);

        assert_eq!(buf.len(), 300 * 200 * 4);
        let clr = clear_bytes(clear);
        let drew = buf
            .chunks_exact(4)
            .any(|px| [px[0], px[1], px[2]] != clr);
        assert!(drew, "offscreen render produced a blank (clear-only) image");
    }

    #[test]
    fn offscreen_empty_is_solid_clear() {
        // 300*4 = 1200 bytes/row → NOT 256-aligned, so this exercises the
        // row-padding un-pad path (incl. the final row).
        let r = pollster::block_on(PlotRenderer::new_offscreen(300, 200));
        let clear = [0.1, 0.2, 0.3, 1.0];
        let buf = r.render_to_rgba(&[], clear);

        assert_eq!(buf.len(), 300 * 200 * 4);
        let clr = clear_bytes(clear);
        for px in buf.chunks_exact(4) {
            for c in 0..3 {
                assert!(
                    (px[c] as i16 - clr[c] as i16).abs() <= 2,
                    "pixel {px:?} differs from clear {clr:?}"
                );
            }
            assert_eq!(px[3], 255, "alpha should be opaque");
        }
    }

    #[test]
    fn offscreen_png_dump() {
        // Visual artifact for eyeballing — asserts a successful encode + write.
        let (w, h) = (640u32, 400u32);
        let r = pollster::block_on(PlotRenderer::new_offscreen(w, h));
        let calls = simple_line_calls(&r, w, h);
        let buf = r.render_to_rgba(&calls, [0.055, 0.059, 0.075, 1.0]);

        let img = image::RgbaImage::from_raw(w, h, buf).expect("buffer sized for image");
        let _ = std::fs::create_dir_all("target");
        img.save("target/oxideplot-offscreen-m1.png")
            .expect("write PNG");
    }
}
