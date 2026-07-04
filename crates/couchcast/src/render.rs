//! wgpu setup and the compositing render pass.
//!
//! The frame is drawn in two passes: the live video texture as a fullscreen
//! aspect-fit quad (NV12 → RGB in-shader), then egui's menu/HUD on top with
//! `LoadOp::Load`. wgpu runs on its Vulkan backend on Linux — the backend with
//! the path to DMABUF import for the eventual zero-copy video path.

use std::sync::Arc;

use couchcast_media::VideoFrame;
use winit::window::Window;

/// Fullscreen NV12→RGB video shader. Two planes (Y in R8, interleaved UV in Rg8)
/// are sampled and converted with the BT.709 limited-range matrix, then
/// linearized so the sRGB swapchain re-encodes correctly (matching egui). The
/// quad is scaled to letterbox the video into the window's aspect ratio.
const VIDEO_WGSL: &str = r#"
struct Scale { scale: vec2<f32>, _pad: vec2<f32> };
@group(0) @binding(0) var y_tex: texture_2d<f32>;
@group(0) @binding(1) var uv_tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<uniform> u: Scale;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = corners[i];
    var out: VsOut;
    out.pos = vec4<f32>(c * u.scale, 0.0, 1.0);
    // Flip Y: NDC up is +y, texture down is +v.
    out.uv = vec2<f32>(c.x * 0.5 + 0.5, 1.0 - (c.y * 0.5 + 0.5));
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let y = textureSample(y_tex, samp, in.uv).r;
    let uv = textureSample(uv_tex, samp, in.uv).rg;
    let yv = 1.1643 * (y - 0.0627);
    let cb = uv.x - 0.5020;
    let cr = uv.y - 0.5020;
    var rgb = vec3<f32>(
        yv + 1.7927 * cr,
        yv - 0.2132 * cb - 0.5329 * cr,
        yv + 2.1124 * cb,
    );
    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    // Gamma-encoded video → linear, so the sRGB target encodes it back correctly.
    let linear = pow(rgb, vec3<f32>(2.2));
    return vec4<f32>(linear, 1.0);
}
"#;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ScaleUniform {
    scale: [f32; 2],
    _pad: [f32; 2],
}

/// GPU textures for the current video frame.
struct VideoResources {
    y_tex: wgpu::Texture,
    uv_tex: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    width: u32,
    height: u32,
}

/// Owns the GPU device/surface, the video pipeline, and the egui paint backend.
pub struct Renderer {
    surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    egui_renderer: egui_wgpu::Renderer,
    video_pipeline: wgpu::RenderPipeline,
    video_bgl: wgpu::BindGroupLayout,
    video_sampler: wgpu::Sampler,
    scale_buf: wgpu::Buffer,
    video: Option<VideoResources>,
    /// One-line summary of the selected GPU adapter, for the debug overlay.
    adapter_info: String,
}

impl Renderer {
    /// Create the device/surface for `window`. Blocks briefly on adapter/device
    /// acquisition — cheap and one-time at startup.
    pub fn new(window: Arc<Window>) -> anyhow::Result<Self> {
        pollster::block_on(Self::new_async(window))
    }

    async fn new_async(window: Arc<Window>) -> anyhow::Result<Self> {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle_from_env()
        });

        let surface = instance.create_surface(window.clone())?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await?;
        let info = adapter.get_info();
        tracing::info!(adapter = ?info, "selected GPU adapter");
        let adapter_info = format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("couchcast-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let egui_renderer =
            egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());

        // --- Video pipeline ---
        let video_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("video-bgl"),
            entries: &[
                tex_entry(0),
                tex_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("video-shader"),
            source: wgpu::ShaderSource::Wgsl(VIDEO_WGSL.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("video-pl"),
            bind_group_layouts: &[Some(&video_bgl)],
            ..Default::default()
        });
        let video_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("video-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let video_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("video-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let scale_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("video-scale"),
            size: std::mem::size_of::<ScaleUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            surface,
            device,
            queue,
            config,
            egui_renderer,
            video_pipeline,
            video_bgl,
            video_sampler,
            scale_buf,
            video: None,
            adapter_info,
        })
    }

    /// A one-line summary of the selected GPU adapter (name, backend, type).
    pub fn adapter_info(&self) -> &str {
        &self.adapter_info
    }

    /// Reconfigure the swapchain after a resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    fn reconfigure(&self) {
        self.surface.configure(&self.device, &self.config);
    }

    /// Upload a decoded NV12 frame into the video textures (recreating them if
    /// the resolution changed).
    pub fn upload_video(&mut self, frame: &VideoFrame) {
        let (w, h) = (frame.width(), frame.height());
        if w == 0 || h == 0 {
            return;
        }
        self.ensure_video_textures(w, h);
        let video = self.video.as_ref().expect("just ensured");

        if let Some(y) = frame.plane(0) {
            write_plane(&self.queue, &video.y_tex, y.data, y.stride as u32, w, h);
        }
        if let Some(uv) = frame.plane(1) {
            // NV12 UV plane is half resolution, two bytes per texel (Rg8).
            write_plane(
                &self.queue,
                &video.uv_tex,
                uv.data,
                uv.stride as u32,
                w / 2,
                h / 2,
            );
        }
    }

    fn ensure_video_textures(&mut self, w: u32, h: u32) {
        if let Some(v) = &self.video
            && v.width == w
            && v.height == h
        {
            return;
        }
        let y_tex = self.create_plane_texture("video-y", w, h, wgpu::TextureFormat::R8Unorm);
        let uv_tex =
            self.create_plane_texture("video-uv", w / 2, h / 2, wgpu::TextureFormat::Rg8Unorm);
        let y_view = y_tex.create_view(&Default::default());
        let uv_view = uv_tex.create_view(&Default::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("video-bg"),
            layout: &self.video_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&y_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&uv_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.video_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.scale_buf.as_entire_binding(),
                },
            ],
        });
        self.video = Some(VideoResources {
            y_tex,
            uv_tex,
            bind_group,
            width: w,
            height: h,
        });
    }

    fn create_plane_texture(
        &self,
        label: &str,
        w: u32,
        h: u32,
        format: wgpu::TextureFormat,
    ) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    /// Update the letterbox scale uniform for the current window/video aspect.
    fn update_scale(&self) {
        let scale = match &self.video {
            Some(v) if v.width > 0 && v.height > 0 => {
                let surf = self.config.width as f32 / self.config.height as f32;
                let vid = v.width as f32 / v.height as f32;
                if vid > surf {
                    [1.0, surf / vid]
                } else {
                    [vid / surf, 1.0]
                }
            }
            _ => [1.0, 1.0],
        };
        self.queue.write_buffer(
            &self.scale_buf,
            0,
            bytemuck::bytes_of(&ScaleUniform {
                scale,
                _pad: [0.0, 0.0],
            }),
        );
    }

    /// Draw one frame: video quad (or black), then egui on top.
    pub fn render(
        &mut self,
        pixels_per_point: f32,
        paint_jobs: &[egui::ClippedPrimitive],
        textures_delta: &egui::TexturesDelta,
    ) {
        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(st)
            | wgpu::CurrentSurfaceTexture::Suboptimal(st) => st,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.reconfigure();
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Validation => {
                tracing::error!("surface get_current_texture validation error");
                return;
            }
        };

        self.update_scale();

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame-encoder"),
            });

        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point,
        };

        for (id, delta) in &textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, delta);
        }
        let user_bufs = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            paint_jobs,
            &screen,
        );

        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("main-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();

            if let Some(video) = &self.video {
                pass.set_pipeline(&self.video_pipeline);
                pass.set_bind_group(0, &video.bind_group, &[]);
                pass.draw(0..6, 0..1);
            }

            self.egui_renderer.render(&mut pass, paint_jobs, &screen);
        }

        self.queue.submit(
            user_bufs
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        surface_texture.present();

        for id in &textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
    }
}

/// A sampled-texture bind-group-layout entry for a video plane.
fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// Upload one plane's rows into `tex`, honoring the source row stride.
fn write_plane(queue: &wgpu::Queue, tex: &wgpu::Texture, data: &[u8], stride: u32, w: u32, h: u32) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
    );
}
