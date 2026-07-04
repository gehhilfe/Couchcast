//! wgpu setup and the compositing render pass.
//!
//! The frame is drawn in two passes: the live video texture as a fullscreen
//! aspect-fit quad (YUV → RGB in-shader), then egui's menu/HUD on top with
//! `LoadOp::Load`. wgpu runs on its Vulkan backend on Linux — the backend with
//! the path to DMABUF import for the eventual zero-copy video path.
//!
//! One shader and pipeline handle both the SDR path (8-bit NV12, BT.709) and the
//! HDR path (10-bit P010, BT.2020 + PQ tone-mapped to the display). They differ
//! only in the plane texture formats (`R8`/`Rg8` vs `R16`/`Rg16`) and a `hdr`
//! flag in the uniform that selects the colour math — see [`VIDEO_WGSL`].

use std::sync::Arc;

use couchcast_media::{PixelFormat, VideoFrame};
use winit::window::Window;

/// Fullscreen YUV→RGB video shader for both the SDR and HDR paths.
///
/// Both are semi-planar 4:2:0: a full-res Y plane plus a half-res interleaved UV
/// plane, sampled as normalized floats regardless of bit depth (`R16`/`Rg16`
/// textures normalize P010's left-justified 10-bit samples to ≈[0,1] just like
/// `R8`/`Rg8` do for NV12). The `hdr` uniform picks the conversion:
///
/// * SDR (`hdr == 0`): BT.709 limited-range YCbCr → RGB, then gamma → linear so
///   the sRGB swapchain re-encodes it correctly (matching egui).
/// * HDR (`hdr == 1`): BT.2020 limited-range YCbCr → PQ-encoded R'G'B', then the
///   SMPTE ST 2084 EOTF to scene-linear light. The final step depends on the
///   swapchain (`hdr_output`):
///   * SDR swapchain (`hdr_output == 0`): an extended-Reinhard tone-map from the
///     HDR luminance range down to SDR, then BT.2020 → BT.709 gamut. Output is
///     linear for the sRGB swapchain to encode.
///   * HDR swapchain (`hdr_output == 1`): no tone-map — absolute nits are scaled
///     to scRGB units (1.0 == 80 cd/m²) and BT.2020 → BT.709 primaries applied in
///     linear light, letting the compositor/display map to its peak.
///
/// SDR content needs no `hdr_output` branch: its linear output is written as-is
/// to the scRGB float target (which does no sRGB encode) or encoded by the sRGB
/// target — the same values are correct for both.
///
/// The quad is scaled to letterbox the video into the window's aspect ratio.
const VIDEO_WGSL: &str = r#"
struct Params { scale: vec2<f32>, hdr: u32, hdr_output: u32 };
@group(0) @binding(0) var y_tex: texture_2d<f32>;
@group(0) @binding(1) var uv_tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<uniform> u: Params;

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

// SMPTE ST 2084 (PQ) EOTF: nonlinear signal [0,1] → linear luminance where 1.0
// corresponds to 10000 cd/m². Applied per channel.
fn pq_eotf(n: vec3<f32>) -> vec3<f32> {
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let np = pow(max(n, vec3<f32>(0.0)), vec3<f32>(1.0 / m2));
    let num = max(np - c1, vec3<f32>(0.0));
    let den = c2 - c3 * np;
    return pow(num / den, vec3<f32>(1.0 / m1));
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let y = textureSample(y_tex, samp, in.uv).r;
    let uv = textureSample(uv_tex, samp, in.uv).rg;

    if (u.hdr == 0u) {
        // --- SDR path: NV12, BT.709 limited range ---
        let yv = 1.1643 * (y - 0.0627);
        let cb = uv.x - 0.5020;
        let cr = uv.y - 0.5020;
        var rgb = vec3<f32>(
            yv + 1.7927 * cr,
            yv - 0.2132 * cb - 0.5329 * cr,
            yv + 2.1124 * cb,
        );
        rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        // Gamma-encoded video → linear, so the sRGB target encodes it back.
        return vec4<f32>(pow(rgb, vec3<f32>(2.2)), 1.0);
    }

    // --- HDR path: P010, BT.2020 limited range, PQ ---
    // 10-bit limited-range expand (normalized: black = 64/1023, chroma centre =
    // 512/1023, luma scale = 1023/876, chroma scale = 1023/896).
    let yv = 1.1678 * (y - 0.0626);
    let cb = 1.1417 * (uv.x - 0.5005);
    let cr = 1.1417 * (uv.y - 0.5005);
    // BT.2020 non-constant-luminance YCbCr → PQ-encoded R'G'B' (Kr=0.2627,
    // Kb=0.0593).
    var pq = vec3<f32>(
        yv + 1.4746 * cr,
        yv - 0.16455 * cb - 0.57135 * cr,
        yv + 1.8814 * cb,
    );
    pq = clamp(pq, vec3<f32>(0.0), vec3<f32>(1.0));
    // PQ → scene-linear light in BT.2020 primaries (1.0 == 10000 nits).
    let lin = pq_eotf(pq);

    if (u.hdr_output != 0u) {
        // --- HDR passthrough to a scRGB (extended-sRGB-linear) swapchain ---
        // Keep the full luminance range: convert absolute nits to scRGB units
        // (1.0 == 80 cd/m², the scRGB reference), map BT.2020 → BT.709 primaries
        // in linear light, and let the compositor/display tone-map to its peak.
        let scrgb_white = 80.0;
        let nits = lin * 10000.0;
        let scene = nits / scrgb_white;
        let r = dot(vec3<f32>( 1.66049, -0.58764, -0.07285), scene);
        let g = dot(vec3<f32>(-0.12455,  1.13290, -0.00835), scene);
        let b = dot(vec3<f32>(-0.01821, -0.10064,  1.11885), scene);
        // Clamp only the lower bound (no BT.709 out-of-gamut negatives for now);
        // the upper range is left open so highlights exceed 1.0 as HDR.
        return vec4<f32>(max(vec3<f32>(r, g, b), vec3<f32>(0.0)), 1.0);
    }

    // --- Tone-map to the SDR display ---
    // Normalize so HDR diffuse white (203 nits, BT.2408) sits near display white,
    // lift with a fixed exposure, then roll off highlights with an
    // extended-Reinhard curve whose white point is the assumed 1000-nit content
    // peak. Per channel; deliberately simple and tunable.
    let ref_white = 203.0;
    let peak = 1000.0;
    let exposure = 2.0;
    let x = lin * (10000.0 / ref_white) * exposure;
    let w = (peak / ref_white) * exposure;
    let mapped = x * (1.0 + x / (w * w)) / (1.0 + x);

    // BT.2020 → BT.709 gamut (linear RGB), then clamp out-of-gamut results.
    let r = dot(vec3<f32>( 1.66049, -0.58764, -0.07285), mapped);
    let g = dot(vec3<f32>(-0.12455,  1.13290, -0.00835), mapped);
    let b = dot(vec3<f32>(-0.01821, -0.10064,  1.11885), mapped);
    let rgb = clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb, 1.0);
}
"#;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ScaleUniform {
    scale: [f32; 2],
    /// 1 = the current frame is HDR content (P010), 0 = SDR. Selects the shader's
    /// colour math.
    hdr: u32,
    /// 1 = presenting to an HDR (scRGB fp16) swapchain, 0 = an SDR swapchain.
    /// Selects HDR passthrough vs tone-map for HDR content. The two `u32`s plus
    /// the `vec2<f32>` make one 16-byte block matching the WGSL `Params` layout.
    hdr_output: u32,
}

/// GPU textures for the current video frame.
struct VideoResources {
    y_tex: wgpu::Texture,
    uv_tex: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    width: u32,
    height: u32,
    /// The layout the textures were created for; a change (e.g. NV12 → P010)
    /// forces a rebuild since the plane texture formats differ.
    format: PixelFormat,
    /// Whether the current frame renders through the shader's HDR path.
    hdr: bool,
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
    /// The compiled video shader and its layout, kept so the pipeline can be
    /// rebuilt for a new surface format when HDR output is toggled.
    video_shader: wgpu::ShaderModule,
    video_pipeline_layout: wgpu::PipelineLayout,
    /// The SDR (sRGB) surface format, always available.
    sdr_format: wgpu::TextureFormat,
    /// The HDR surface format (scRGB `Rgba16Float`) if the surface advertises it.
    hdr_format: Option<wgpu::TextureFormat>,
    /// Whether the surface is currently configured for HDR (scRGB) output.
    hdr_output: bool,
    /// One-line summary of the selected GPU adapter, for the debug overlay.
    adapter_info: String,
}

impl Renderer {
    /// Create the device/surface for `window`. Blocks briefly on adapter/device
    /// acquisition — cheap and one-time at startup. `prefer_hdr` requests an HDR
    /// (scRGB) swapchain when the surface advertises one; it silently stays SDR
    /// otherwise.
    pub fn new(window: Arc<Window>, prefer_hdr: bool) -> anyhow::Result<Self> {
        pollster::block_on(Self::new_async(window, prefer_hdr))
    }

    async fn new_async(window: Arc<Window>, prefer_hdr: bool) -> anyhow::Result<Self> {
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

        // 16-bit-norm textures back the P010 (10-bit HDR) planes. It is a
        // native-only wgpu feature but universally present on desktop Vulkan;
        // intersect with the adapter so a device that somehow lacks it still
        // starts (SDR/NV12 keeps working, only P010 upload would be unavailable).
        let features = wgpu::Features::TEXTURE_FORMAT_16BIT_NORM & adapter.features();
        if !features.contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM) {
            tracing::warn!("adapter lacks TEXTURE_FORMAT_16BIT_NORM; P010/HDR capture will not render");
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("couchcast-device"),
                required_features: features,
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await?;

        let caps = surface.get_capabilities(&adapter);
        let sdr_format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        // wgpu-hal's Vulkan backend configures an `Rgba16Float` surface with the
        // scRGB (extended-sRGB-linear) HDR color space, so the format appearing in
        // the caps is exactly the "HDR display available" signal.
        let hdr_format = caps
            .formats
            .iter()
            .copied()
            .find(|f| *f == wgpu::TextureFormat::Rgba16Float);
        let hdr_output = prefer_hdr && hdr_format.is_some();
        let format = if hdr_output {
            hdr_format.expect("checked is_some")
        } else {
            sdr_format
        };
        match hdr_format {
            Some(_) => tracing::info!(hdr_output, "HDR (scRGB) surface format available"),
            None => tracing::info!("no HDR surface format advertised; SDR output only"),
        }
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

        let video_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("video-shader"),
            source: wgpu::ShaderSource::Wgsl(VIDEO_WGSL.into()),
        });
        let video_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("video-pl"),
                bind_group_layouts: &[Some(&video_bgl)],
                ..Default::default()
            });
        let video_pipeline =
            build_video_pipeline(&device, &video_pipeline_layout, &video_shader, format);

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
            video_shader,
            video_pipeline_layout,
            sdr_format,
            hdr_format,
            hdr_output,
            adapter_info,
        })
    }

    /// A one-line summary of the selected GPU adapter (name, backend, type).
    pub fn adapter_info(&self) -> &str {
        &self.adapter_info
    }

    /// Whether the surface can present true HDR (a scRGB `Rgba16Float` swapchain).
    pub fn hdr_available(&self) -> bool {
        self.hdr_format.is_some()
    }

    /// Whether HDR output is currently active (surface configured for scRGB).
    pub fn hdr_output(&self) -> bool {
        self.hdr_output
    }

    /// Switch the swapchain between SDR (sRGB) and HDR (scRGB) output. Returns
    /// whether the surface format actually changed (false if the target state
    /// already held or the surface has no HDR format). Rebuilds the surface
    /// config, the video pipeline, and the egui backend for the new format (their
    /// pipelines bake in the target format).
    ///
    /// Because the egui backend is rebuilt, it loses its uploaded textures; the
    /// caller must, when this returns `true`, recreate the egui context so the
    /// next frame re-emits them. Cheap and safe to call from the render thread
    /// between frames.
    #[must_use]
    pub fn set_hdr_output(&mut self, on: bool) -> bool {
        let target = on && self.hdr_format.is_some();
        if target == self.hdr_output {
            return false;
        }
        let format = match (target, self.hdr_format) {
            (true, Some(f)) => f,
            _ => self.sdr_format,
        };
        self.hdr_output = target;
        self.config.format = format;
        self.surface.configure(&self.device, &self.config);
        self.video_pipeline = build_video_pipeline(
            &self.device,
            &self.video_pipeline_layout,
            &self.video_shader,
            format,
        );
        self.egui_renderer =
            egui_wgpu::Renderer::new(&self.device, format, egui_wgpu::RendererOptions::default());
        tracing::info!(hdr_output = self.hdr_output, ?format, "reconfigured swapchain");
        true
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

    /// Upload a decoded frame (NV12 or P010) into the video textures, recreating
    /// them if the resolution or pixel format changed. The plane layout is
    /// identical for both formats — full-res Y, half-res interleaved UV — so only
    /// the texture bit depth and the shader's `hdr` flag differ.
    pub fn upload_video(&mut self, frame: &VideoFrame) {
        let (w, h) = (frame.width(), frame.height());
        if w == 0 || h == 0 {
            return;
        }
        self.ensure_video_textures(w, h, frame.format(), frame.is_hdr());
        let video = self.video.as_ref().expect("just ensured");

        if let Some(y) = frame.plane(0) {
            write_plane(&self.queue, &video.y_tex, y.data, y.stride as u32, w, h);
        }
        if let Some(uv) = frame.plane(1) {
            // The UV plane is half resolution with both components interleaved
            // (Rg8 for NV12, Rg16 for P010); the stride carries the byte width.
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

    fn ensure_video_textures(&mut self, w: u32, h: u32, format: PixelFormat, hdr: bool) {
        if let Some(v) = &self.video
            && v.width == w
            && v.height == h
            && v.format == format
        {
            // Same layout: reuse the textures, but keep the HDR flag current (the
            // colorimetry of an incoming stream can change without a resize).
            if v.hdr != hdr {
                self.video.as_mut().expect("just matched").hdr = hdr;
            }
            return;
        }
        // P010 carries 10-bit samples in 16-bit words, so it needs R16/Rg16
        // planes; NV12 is 8-bit R8/Rg8.
        let (y_format, uv_format) = match format {
            PixelFormat::Nv12 => (wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm),
            PixelFormat::P010 => (wgpu::TextureFormat::R16Unorm, wgpu::TextureFormat::Rg16Unorm),
        };
        let y_tex = self.create_plane_texture("video-y", w, h, y_format);
        let uv_tex = self.create_plane_texture("video-uv", w / 2, h / 2, uv_format);
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
            format,
            hdr,
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

    /// Update the letterbox scale + HDR-mode uniform for the current window/video.
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
        let hdr = self.video.as_ref().is_some_and(|v| v.hdr) as u32;
        self.queue.write_buffer(
            &self.scale_buf,
            0,
            bytemuck::bytes_of(&ScaleUniform {
                scale,
                hdr,
                hdr_output: self.hdr_output as u32,
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

/// Build the fullscreen video render pipeline targeting `format`. Split out so it
/// can be rebuilt when the surface format changes (SDR ↔ HDR toggle) — a render
/// pipeline bakes in its color-target format.
fn build_video_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("video-pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
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
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse and validate the video WGSL with naga (wgpu's shader compiler), so a
    /// syntax or type error in the SDR/HDR shader is caught here rather than at
    /// pipeline creation on a machine with a GPU. Mirrors the validation wgpu runs
    /// internally when the pipeline is built.
    #[test]
    fn video_shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(VIDEO_WGSL).expect("WGSL should parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("WGSL should validate");
    }

    /// The uniform the shader reads must match the WGSL `Params` block: a 16-byte
    /// block (vec2<f32> + two 4-byte scalars).
    #[test]
    fn scale_uniform_matches_params_layout() {
        assert_eq!(std::mem::size_of::<ScaleUniform>(), 16);
    }
}
