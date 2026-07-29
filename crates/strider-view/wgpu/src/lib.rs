// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The device half of the renderer, on wgpu.
//!
//! `strider-view` decides what to draw, what to request, what to cancel and which points an
//! edit hides. This crate turns that decision into draw calls. Two things are worth
//! stating about the split, because both are clause-shaped:
//!
//! * **The device layer holds no policy.** It is handed a list of buffers and a camera and
//!   it rasterises them. It cannot request a partition, cannot evict one, and has never
//!   heard of an edit — the host uploads vertices whose classification already reflects the
//!   effective edit set, so the gesture model stops at `strider-view`.
//! * **The depth test is real.** [[RFC-0006:C-OVERLAY]] 1 requires depth-dependent content
//!   to be drawn by the renderer, and here an anchor is a billboard rasterised against the
//!   same depth buffer the cloud wrote. Occlusion is a `<` in hardware, not a decision
//!   anybody made — which is exactly the thing a toolkit compositing the same label cannot
//!   reproduce at any price.
//!
//! What it deliberately does **not** do: own a thread, block on presentation, or create a
//! window ([[RFC-0006:C-SURFACE]] 2 and 3). It records commands and submits them. Reading
//! the result back is the host's, and the host is where the blocking map lives.

mod ramp;

// Vulkan-specific interop, in its own module.
//
// Nothing in it is reachable on another backend and none of it belongs in the portable
// renderer. Separated before the Metal work rather than after, because two providers
// interleaved in this file is the arrangement that rule exists to prevent.
//
// The cfg is on the TARGET rather than on a cargo feature, because which backend exists is a
// property of the platform and not of a build choice here: `wgpu::hal::api::Vulkan` is not a
// type at all on an Apple target, so this module does not compile there — five errors, none of
// them about anything this crate wrote. wgpu picks Metal on Apple and Vulkan elsewhere.
#[cfg(not(target_vendor = "apple"))]
pub mod vulkan;

#[cfg(target_vendor = "apple")]
pub mod metal;

pub use ramp::{Ramp, Shading, RAMP_TEXELS};

use bytemuck::{Pod, Zeroable};
use strider_view::{Draw, Vertex};
use std::collections::BTreeMap;
use wgpu::util::DeviceExt;

/// How many frames of GPU work may be in flight before a readback blocks on one of them.
///
/// The readback ring has `READBACK_LAG + 1` slots: one being written this frame, and
/// `READBACK_LAG` others in flight. Reading the oldest means the GPU has had `READBACK_LAG`
/// whole frames to finish it, so the wait is ~free in steady state — which is the point. A
/// blocking read every frame turned `draw` into a lie: it "returned" in 0.1 ms because it
/// only *submits*, and the readback absorbed all the real GPU time in its fence wait.
const READBACK_LAG: usize = 2;

/// One point as the device sees it. `strider_view::Vertex` cannot derive `Pod`, being in a
/// crate with no dependencies, so the conversion happens once at upload.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuVertex {
    pos: [f32; 3],
    class: u32,
    rgb: [f32; 3],
    channels: [f32; strider_view::CHANNELS],
}

// The stride, asserted, because I have now made the same mistake twice in one sitting.
//
// `vertex_attr_array!` numbers locations sequentially over whatever it is given, so any
// field the shader does not declare — a padding float, say — shifts every location after it
// by one. wgpu does not reject that: the shader reads the neighbouring field, or zero, and
// the frame renders in a plausible wrong colour. Twice now that has been "every point is
// black" and "the ramp is flat".
//
// This assertion does not catch a reordering, but it does catch the thing that actually
// happened: a field added or removed without the attribute list being updated to match.
// The vertex attribute list and the WGSL entry point both spell out the channel block by hand,
// and neither is derived from `CHANNELS`. So changing `CHANNELS` alone produces a buffer the
// shader reads only part of — silently, because wgpu does not reject a short read and the missing
// attribute comes back as zero. That is not a hypothetical: this exact class of bug has rendered
// every point black once and flattened the ramp once.
const _: () = assert!(
    strider_view::CHANNELS == 5,
    "CHANNELS changed. Update BOTH the `vertex_attr_array!` in the points pipeline (a vec4 plus \
     one scalar per extra channel — Vulkan has no wider vertex format) and `points.wgsl`'s \
     channel locations and `shade()` selector, then update this assertion."
);

const _: () = assert!(
    core::mem::size_of::<GpuVertex>() == 12 + 4 + 12 + 4 * strider_view::CHANNELS,
    "GpuVertex has padding or an unaccounted field; the shader's @location numbering \
     will be off by one from this struct's field order"
);

// No padding fields, deliberately. An earlier version had `_pad: f32` between `attrs` and
// `rgb`, and `vertex_attr_array!` assigns locations sequentially — so the padding took
// location 3 and `rgb` took 4, while the shader declared `rgb` at 3. The shader then read
// the padding, and every point rendered black. A location mismatch does not fail validation;
// it silently reads zero, which looks exactly like data the file does not carry. Keeping the
// struct free of padding keeps the two numberings impossible to desynchronise.

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuAnchor {
    world: [f32; 3],
    kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    z_lo: f32,
    z_hi: f32,
    point_size: f32,
    ramp_channel: u32,
    ramp_lo: f32,
    ramp_hi: f32,
    /// Target size in pixels. The shader expands a point into a screen-aligned quad in clip
    /// space, which needs the real dimensions — they were hardcoded to 1600x1000, so every
    /// target of another size got the wrong point size.
    viewport: [f32; 2],
}

/// A 3D camera over the cloud. Orbit, because a point cloud read from directly above is a
/// height map and the whole reason depth matters is that it is not one.
#[derive(Clone, Copy, Debug)]
pub struct Orbit {
    pub target: [f32; 3],
    /// Metres from the target.
    pub distance: f32,
    /// Radians. `yaw` around z, `pitch` up from the horizon.
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
}

impl Orbit {
    /// Frame the camera from the host's camera state and a **stable** height anchor.
    ///
    /// `z_anchor` must not depend on what is currently drawn, and that is the whole point of
    /// this signature. An earlier version took the drawn height range, which put the camera
    /// in a feedback loop with residency: zooming changed the level, the level changed which
    /// points were resident, that changed the drawn height range, and the camera moved. The
    /// symptom is a cloud that drifts while you zoom, and it is not a jitter to be damped —
    /// it is a camera being derived from something it should be independent of.
    ///
    /// The anchor a host should pass is one established once, such as the height range of the
    /// COPC root node: a decimated sample of the whole cloud, fixed for the session.
    pub fn framing(view_min: [f32; 2], view_max: [f32; 2], z_anchor: (f32, f32)) -> Self {
        let (lo, hi) = z_anchor;
        let span = (view_max[0] - view_min[0]).max(view_max[1] - view_min[1]);
        Self {
            target: [
                (view_min[0] + view_max[0]) * 0.5,
                (view_min[1] + view_max[1]) * 0.5,
                // A fraction of the anchor's range rather than its midpoint: the interesting
                // structure sits near the ground, and aiming halfway up a 70 m range puts it
                // at the bottom of the frame.
                lo + (hi - lo) * 0.25,
            ],
            // Depends on the view width alone, so zooming changes exactly one thing.
            distance: span * 1.45,
            yaw: 0.72,
            pitch: 0.55,
            fov_y: 0.85,
        }
    }

    pub fn eye(&self) -> [f32; 3] {
        let (cp, sp) = (self.pitch.cos(), self.pitch.sin());
        let (cy, sy) = (self.yaw.cos(), self.yaw.sin());
        [
            self.target[0] + self.distance * cp * cy,
            self.target[1] + self.distance * cp * sy,
            self.target[2] + self.distance * sp,
        ]
    }

    /// View-projection, right-handed, with wgpu's 0..1 depth range.
    ///
    /// Written out rather than pulled from a maths crate: it is thirty lines, and a
    /// prototype whose question is about boundaries should not add a dependency to answer a
    /// question it is not asking.
    fn view_proj(&self, aspect: f32) -> [[f32; 4]; 4] {
        let eye = self.eye();
        let up = [0.0f32, 0.0, 1.0];
        let f = normalise(sub(self.target, eye));
        let s = normalise(cross(f, up));
        let u = cross(s, f);
        let view = [
            [s[0], u[0], -f[0], 0.0],
            [s[1], u[1], -f[1], 0.0],
            [s[2], u[2], -f[2], 0.0],
            [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
        ];
        let (near, far) = (0.5f32, self.distance * 6.0 + 500.0);
        let t = 1.0 / (self.fov_y * 0.5).tan();
        // Reverse-free 0..1 depth: wgpu's convention, so no correction matrix.
        let proj = [
            [t / aspect, 0.0, 0.0, 0.0],
            [0.0, t, 0.0, 0.0],
            [0.0, 0.0, far / (near - far), -1.0],
            [0.0, 0.0, near * far / (near - far), 0.0],
        ];
        mul(proj, view)
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalise(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-6);
    [a[0] / l, a[1] / l, a[2] / l]
}
fn mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut o = [[0.0f32; 4]; 4];
    for (c, col) in b.iter().enumerate() {
        for r in 0..4 {
            o[c][r] = (0..4).map(|k| a[k][r] * col[k]).sum();
        }
    }
    o
}

/// An anchor to draw, and nothing about whether it is visible — the depth buffer decides.
#[derive(Clone, Copy, Debug)]
pub struct AnchorPoint {
    pub world: [f32; 3],
}

/// An offscreen target the host composites ([[RFC-0006:C-SURFACE]] 4).
pub struct Offscreen {
    pub width: u32,
    pub height: u32,
    colour: wgpu::Texture,
    depth: wgpu::Texture,
    /// The colour attachment's `VkImage`, when this target was built to be shared.
    ///
    /// Only set on a target whose image **this crate allocated**. A texture wgpu created cannot
    /// supply one: `wgpu_hal::vulkan::Texture` keeps its `raw: vk::Image` private and exposes no
    /// accessor, unlike `Device`, `Adapter` and `Queue`, which expose everything. So the image
    /// is allocated here and wrapped INTO wgpu rather than taken out of it — the same inversion
    /// the device needed, for the same reason.
    image: Option<u64>,
}

impl Offscreen {
    /// The views `draw` needs. Same pair a presenting target yields, which is what makes
    /// C-SURFACE 4's "switching MUST NOT change the obligations" true by construction here:
    /// there is one draw path and it cannot tell the two apart.
    pub fn views(&self) -> (wgpu::TextureView, wgpu::TextureView) {
        (
            self.colour.create_view(&Default::default()),
            self.depth.create_view(&Default::default()),
        )
    }

    /// The colour attachment's `VkImage`, for a host that samples it directly.
    ///
    /// `None` unless this target came from `Gpu::offscreen_shared`.
    pub fn vulkan_image(&self) -> Option<u64> {
        self.image
    }
}

/// An opaque native surface handle, as a host supplies it ([[RFC-0006:C-SURFACE]] 1).
///
/// Pointers and an integer. This crate cannot ask what produced them
/// ([[RFC-0006:C-SURFACE]] 2) — the values happen to come from Qt here, and nothing in the
/// type or in any code below this line could discover that.
#[derive(Clone, Copy, Debug)]
pub enum NativeSurface {
    Xlib {
        /// `Display *`
        display: *mut core::ffi::c_void,
        /// The X11 window id.
        window: u64,
        screen: i32,
    },
    Wayland {
        /// `wl_display *`
        display: *mut core::ffi::c_void,
        /// `wl_surface *`
        surface: *mut core::ffi::c_void,
    },
}

/// A surface the renderer presents to, plus the depth buffer that goes with it.
///
/// Owned by the **host**, deliberately. Acquiring a swapchain image is the one step that
/// can block, and [[RFC-0006:C-SURFACE]] 3 forbids the renderer blocking on presentation —
/// so `acquire` is a host call and `draw` only ever sees views that already exist.
pub struct Presenting {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::Texture,
    pub width: u32,
    pub height: u32,
}

/// Device, queue and pipelines.
pub struct Gpu {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    points: wgpu::RenderPipeline,
    anchors: wgpu::RenderPipeline,
    camera_buf: wgpu::Buffer,
    buffers: BTreeMap<u64, (wgpu::Buffer, u32)>,
    next: u64,
    /// One bind group per ramp, built once. Switching ramp is then a bind rather than an
    /// upload, which is what makes a ramp a UI control rather than a reload.
    ramps: Vec<(Ramp, wgpu::BindGroup)>,
    camera_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pub adapter_name: String,
    pub backend: String,
    colour_format: wgpu::TextureFormat,
    /// A ring of `MAP_READ` staging buffers, one per frame in flight.
    ///
    /// One buffer per frame was the OOM: a fresh `MAP_READ` buffer every paint, with wgpu
    /// freeing lazily, let the allocator run ahead of the frees. A ring is a different fix
    /// for a different reason: with `READBACK_LAG + 1` buffers, the copy this frame writes
    /// into one slot while the readback maps the slot copied `READBACK_LAG` frames ago. The
    /// wait then overlaps the GPU's current work instead of serialising with it.
    readback: Vec<wgpu::Buffer>,
    /// The submission that wrote each ring slot, so a read can wait on exactly one old
    /// submission instead of `wait_indefinitely` draining the whole queue.
    readback_sub: Vec<Option<wgpu::SubmissionIndex>>,
    /// Next ring slot to copy into.
    readback_next: usize,
    /// Size the ring was built for.
    readback_size: (u32, u32),
    /// The anchor quad buffer, cached because anchors do not move between frames.
    anchor_buf: Option<(u32, wgpu::Buffer)>,
}

impl Gpu {
    /// Acquire a device with no surface — the offscreen case.
    pub fn new() -> Option<Self> {
        Self::open(None).map(|(gpu, ..)| gpu)
    }

    /// Acquire a device compatible with a host-supplied native surface, and configure it.
    ///
    /// The adapter must be chosen *against* the surface, which is why this is one call
    /// rather than `new()` followed by an attach: an adapter that cannot present to the
    /// host's surface is not a usable answer, and finding that out later is worse.
    ///
    /// # Safety
    ///
    /// The handles must be valid and must outlive the returned `Presenting`.
    pub unsafe fn new_presenting(
        native: NativeSurface,
        width: u32,
        height: u32,
    ) -> Option<(Self, Presenting)> {
        let (gpu, surface, format) = Self::open(Some(native))?;
        let surface = surface?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            format,
            width: width.max(1),
            height: height.max(1),
            // Fifo: present-mode selection is a host policy — it is the frame budget's
            // shape — and Fifo is the one every backend supports.
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: surface.get_capabilities(&gpu.adapter).alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&gpu.device, &config);
        let depth = gpu.depth_texture(width.max(1), height.max(1));
        let presenting = Presenting {
            surface,
            config,
            depth,
            width: width.max(1),
            height: height.max(1),
        };
        Some((gpu, presenting))
    }

    fn open(
        native: Option<NativeSurface>,
    ) -> Option<(Self, Option<wgpu::Surface<'static>>, wgpu::TextureFormat)> {
        use raw_window_handle as rwh;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = match native {
            None => None,
            Some(NativeSurface::Xlib {
                display,
                window,
                screen,
            }) => {
                let mut dh = rwh::XlibDisplayHandle::new(core::ptr::NonNull::new(display), screen);
                dh.screen = screen;
                let wh = rwh::XlibWindowHandle::new(window);
                // SAFETY: the caller of `new_presenting` promised these outlive the surface.
                Some(unsafe {
                    instance
                        .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                            raw_display_handle: Some(rwh::RawDisplayHandle::Xlib(dh)),
                            raw_window_handle: rwh::RawWindowHandle::Xlib(wh),
                        })
                        .ok()?
                })
            }
            Some(NativeSurface::Wayland { display, surface }) => {
                let dh = rwh::WaylandDisplayHandle::new(core::ptr::NonNull::new(display)?);
                let wh = rwh::WaylandWindowHandle::new(core::ptr::NonNull::new(surface)?);
                Some(unsafe {
                    instance
                        .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                            raw_display_handle: Some(rwh::RawDisplayHandle::Wayland(dh)),
                            raw_window_handle: rwh::RawWindowHandle::Wayland(wh),
                        })
                        .ok()?
                })
            }
        };
        let adapter = pollster_block(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: surface.as_ref(),
        }))
        .ok()?;
        let info = adapter.get_info();
        let (device, queue) = pollster_block(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("strider prototype"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()?;

        // The colour format is chosen HERE, before the pipelines are built, because a
        // render pipeline bakes its target format in. Getting this order wrong is what
        // produced a bare "wgpu error: Validation Error" on the first presenting run: the
        // pipelines were built for `Rgba8UnormSrgb` while an X11 Vulkan swapchain hands back
        // `Bgra8UnormSrgb`, and the mismatch is only detectable at the render pass.
        let format = match surface.as_ref() {
            None => wgpu::TextureFormat::Rgba8UnormSrgb,
            Some(surface) => {
                let caps = surface.get_capabilities(&adapter);
                caps.formats
                    .iter()
                    .copied()
                    .find(|f| f.is_srgb())
                    .unwrap_or(caps.formats[0])
            }
        };

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("points"),
            source: wgpu::ShaderSource::Wgsl(include_str!("points.wgsl").into()),
        });

        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Camera uniform, plus the ramp as a 1-D texture and its sampler. The ramp being a
        // bound resource rather than shader code is what makes it interchangeable.
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera and ramp"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D1,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strider"),
            bind_group_layouts: &[Some(&camera_layout)],
            immediate_size: 0,
        });

        let depth = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };

        let points = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("points"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_points"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuVertex>() as u64,
                    // One quad per point: the vertex index picks the corner, the instance
                    // picks the point.
                    step_mode: wgpu::VertexStepMode::Instance,
                    // Offsets are sequential, so the struct above must have no field the
                    // shader does not declare — a mismatch reads as zero rather than
                    // failing, which is how every point once rendered black.
                    // One entry per field of `GpuVertex`, in field order, and nothing else —
                    // see the assertion beside that struct.
                    // The channel block is a `vec4` plus a scalar because Vulkan has no
                    // five-component vertex format. Both halves must be declared, and the
                    // assertion below fails the build if `CHANNELS` moves without this list —
                    // which is the third time this layout has needed defending, the previous two
                    // being a padding float and a flat ramp.
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, 1 => Uint32, 2 => Float32x3, 3 => Float32x4, 4 => Float32
                    ],
                }],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(depth.clone()),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_points"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            cache: None,
            multiview_mask: None,
        });

        let anchors = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("anchors"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_anchors"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuAnchor>() as u64,
                    // One quad per anchor: the vertex index picks the corner, the instance
                    // picks the anchor.
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32],
                }],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                // Tested against the cloud, but writes nothing: an anchor must not occlude
                // another anchor, only be occluded by geometry.
                depth_write_enabled: Some(false),
                ..depth
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_anchors"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            cache: None,
            multiview_mask: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ramp"),
            // Clamped, so a value at or past an end of the range takes the end colour rather
            // than wrapping to the other end of the ramp.
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Some((
            Self {
                adapter,
                device,
                queue,
                points,
                anchors,
                camera_buf,
                buffers: BTreeMap::new(),
                readback: Vec::new(),
                readback_sub: Vec::new(),
                readback_next: 0,
                readback_size: (0, 0),
                anchor_buf: None,
                next: 0,
                ramps: Vec::new(),
                camera_layout,
                sampler,
                adapter_name: info.name,
                backend: format!("{:?}", info.backend),
                colour_format: format,
            },
            surface,
            format,
        ))
    }

    /// The colour format the pipelines were built for. A presenting target must be
    /// configured with exactly this.
    pub fn colour_format(&self) -> wgpu::TextureFormat {
        self.colour_format
    }

    /// Perform an upload the renderer asked the host for. Returns the token
    /// `strider-view` will hold — it never sees the buffer.
    pub fn upload(&mut self, verts: &[Vertex]) -> u64 {
        let gpu: Vec<GpuVertex> = verts
            .iter()
            .map(|v| {
                GpuVertex {
                    pos: [v.x, v.y, v.z],
                    class: v.class as u32,
                    rgb: v.rgb,
                    channels: v.channels,
                }
            })
            .collect();
        let buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("points"),
                contents: bytemuck::cast_slice(&gpu),
                usage: wgpu::BufferUsages::VERTEX,
            });
        self.next += 1;
        self.buffers.insert(self.next, (buf, verts.len() as u32));
        self.next
    }

    /// Perform an `Effect::Evict`.
    pub fn free(&mut self, token: u64) {
        self.buffers.remove(&token);
    }

    pub fn resident_buffers(&self) -> usize {
        self.buffers.len()
    }

    /// The ramps this device has bind groups for, in the order the host registered them.
    pub fn ramp_names(&self) -> Vec<&'static str> {
        self.ramps.iter().map(|(r, _)| r.name).collect()
    }

    /// Register a ramp and return its index, for `Shading::Ramped`.
    ///
    /// Uploaded once. A host adding a project palette calls this; nothing in the shader or
    /// the pipeline changes.
    pub fn add_ramp(&mut self, ramp: Ramp) -> usize {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(ramp.name),
            size: wgpu::Extent3d {
                width: RAMP_TEXELS as u32,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D1,
            // Not `*Srgb`: the texels were converted to linear when the ramp was built, so
            // asking the sampler to convert again is the double-transfer bug in another
            // costume.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            ramp.bytes(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((RAMP_TEXELS * 4) as u32),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: RAMP_TEXELS as u32,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(ramp.name),
            layout: &self.camera_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.camera_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.ramps.push((ramp, bind));
        self.ramps.len() - 1
    }

    /// Register the built-in ramps, so a host that wants the usual set says one thing.
    pub fn add_default_ramps(&mut self) {
        for r in Ramp::all() {
            self.add_ramp(r);
        }
    }

    fn depth_texture(&self, width: u32, height: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
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
        })
    }

    pub fn offscreen(&self, width: u32, height: u32) -> Offscreen {
        let make = |format, usage, label| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        Offscreen {
            width,
            height,
            image: None,
            colour: make(
                self.colour_format,
                // `TEXTURE_BINDING` because Qt samples this image directly on the shared path.
                // `COPY_SRC` stays for the readback path, which every non-Vulkan machine uses —
                // one texture serves both, so the two paths cannot drift apart.
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                "colour",
            ),
            depth: make(
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
                "depth",
            ),
        }
    }

    /// Draw one frame.
    ///
    /// `draws` is `strider-view`'s decision, unchanged: which partition, at which level,
    /// from which buffer. This function adds no policy — it does not choose what to draw,
    /// and it has no way to ask for anything it was not given.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        colour_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        size: (u32, u32),
        draws: &[(Draw, u64)],
        anchors: &[AnchorPoint],
        cam: &Orbit,
        z_range: (f32, f32),
        point_size: f32,
        shading: Shading,
    ) -> FrameStats {
        let uniform = CameraUniform {
            view_proj: cam.view_proj(size.0 as f32 / size.1 as f32),
            z_lo: z_range.0,
            z_hi: z_range.1,
            point_size,
            ramp_channel: shading.channel_or_sentinel(),
            ramp_lo: shading.range().0,
            ramp_hi: shading.range().1,
            viewport: [size.0 as f32, size.1 as f32],
        };
        self.queue
            .write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&uniform));

        let anchor_data: Vec<GpuAnchor> = anchors
            .iter()
            .map(|a| GpuAnchor {
                world: a.world,
                kind: 0,
            })
            .collect();
        // Anchors do not move between frames, so the same quad buffer is drawn every
        // frame. Rebuilding it each frame was a `create_buffer_init` for nothing; cache it
        // and recreate only when the count changes.
        let anchor_buf = if anchor_data.is_empty() {
            None
        } else {
            let n = anchor_data.len() as u32;
            match &self.anchor_buf {
                Some((k, b)) if *k == n => Some(b.clone()),
                _ => {
                    let b = self
                        .device
                        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("anchors"),
                            contents: bytemuck::cast_slice(&anchor_data),
                            usage: wgpu::BufferUsages::VERTEX,
                        });
                    self.anchor_buf = Some((n, b.clone()));
                    Some(b)
                }
            }
        };

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        let mut points_drawn = 0u64;
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cloud"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: colour_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.043,
                            g: 0.047,
                            b: 0.055,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.points);
            // The ramp is part of the bind group, so choosing one is a bind.
            // A host that registered no ramp cannot draw: the bind group carries the camera
            // too. Returning early is better than a validation error, and better than a
            // silent black frame.
            let Some((_, bind)) = self.ramps.get(shading.ramp_index()).or(self.ramps.first())
            else {
                return FrameStats {
                    points_drawn: 0,
                    buffers_drawn: 0,
                };
            };
            pass.set_bind_group(0, bind, &[]);
            for (_, token) in draws {
                let Some((buf, count)) = self.buffers.get(token) else {
                    continue;
                };
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..6, 0..*count);
                points_drawn += *count as u64;
            }
            // Anchors last, into the depth buffer the cloud just wrote. This is the whole
            // of C-OVERLAY 1: the hardware decides, not the host.
            if let Some(ab) = &anchor_buf {
                pass.set_pipeline(&self.anchors);
                pass.set_bind_group(0, bind, &[]);
                pass.set_vertex_buffer(0, ab.slice(..));
                pass.draw(0..6, 0..anchor_data.len() as u32);
            }
        }
        self.queue.submit([enc.finish()]);
        FrameStats {
            points_drawn,
            buffers_drawn: draws.len(),
        }
    }

    /// Read an offscreen target back. The blocking map is **here**, on the host's side of
    /// the line, and not in `draw` — a renderer that blocked on device work could not
    /// observe the cancellation [[RFC-0006:C-RENDER]] 2 requires.
    /// Block until every submitted command has completed.
    ///
    /// The shared path needs it and has nothing better: Qt is handed a `VkImage`, not a promise,
    /// and `wgpu_hal::vulkan::Queue` offers `add_signal_semaphore` but no wait counterpart, so
    /// there is no way to make Qt's submission wait on ours. Blocking the host thread is the
    /// honest stand-in — it costs the overlap but not the two transfers, which are the point.
    pub fn wait_idle(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }

    pub fn read_rgba(&mut self, target: &Offscreen) -> Vec<u8> {
        let bpr = (target.width * 4).next_multiple_of(256);
        let size = (target.width, target.height);
        if self.readback_size != size {
            let make = || {
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("readback"),
                    size: (bpr * target.height) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            };
            self.readback = (0..READBACK_LAG + 1).map(|_| make()).collect();
            self.readback_sub = vec![None; READBACK_LAG + 1];
            self.readback_next = 0;
            self.readback_size = size;
        }

        // This frame's copy goes into the next slot.
        let slot = self.readback_next;
        let buf = self.readback[slot].clone();
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.colour,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(target.height),
                },
            },
            wgpu::Extent3d {
                width: target.width,
                height: target.height,
                depth_or_array_layers: 1,
            },
        );
        let idx = self.queue.submit([enc.finish()]);
        self.readback_sub[slot] = Some(idx.clone());
        self.readback_next = (slot + 1) % (READBACK_LAG + 1);

        // Read the slot the ring will next overwrite: the one copied `READBACK_LAG` frames
        // ago, once the ring is full. After the cursor advances, `readback_next` points at
        // exactly that slot, because the ring is `READBACK_LAG + 1` long.
        let read_slot = self.readback_next;
        let old = self.readback_sub[read_slot].clone();
        let (source_slot, wait_for) = match old {
            // Ring not full yet: nothing is old enough, so block on the copy just
            // submitted. The warmup is two frames and invisible.
            None => (slot, idx),
            // Steady state: wait only on a `READBACK_LAG`-frames-old submission. The GPU
            // has had that long to finish it, so this returns ~immediately and does NOT
            // wait for this frame's draw — the overlap that removes the fence.
            Some(o) => (read_slot, o),
        };
        let source = &self.readback[source_slot];
        let slice = source.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(wait_for),
            timeout: None,
        });
        let mapped = slice.get_mapped_range();
        let mut out = Vec::with_capacity((target.width * target.height * 4) as usize);
        for row in 0..target.height {
            let start = (row * bpr) as usize;
            out.extend_from_slice(&mapped[start..start + (target.width * 4) as usize]);
        }
        drop(mapped);
        source.unmap();
        out
    }
}

impl Gpu {
    /// Acquire the next swapchain image.
    ///
    /// A **host** call, and the placement is the clause rather than taste: this is the step
    /// that blocks, and [[RFC-0006:C-SURFACE]] 3 forbids the renderer blocking on
    /// presentation. `draw` therefore only ever receives views that already exist, and a
    /// renderer which cannot reach the acquire cannot be stalled by it — which is what
    /// leaves it able to observe the cancellation [[RFC-0006:C-RENDER]] 2 requires.
    ///
    /// `None` means the surface was lost or out of date; the host reconfigures and tries the
    /// next frame rather than looping here.
    pub fn acquire(&self, p: &Presenting) -> Option<wgpu::SurfaceTexture> {
        use wgpu::CurrentSurfaceTexture as C;
        match p.surface.get_current_texture() {
            C::Success(t) => Some(t),
            // Suboptimal is still a usable image; the reconfigure is advice, not a failure,
            // and dropping the frame to take it would be a stutter for nothing.
            C::Suboptimal(t) => {
                p.surface.configure(&self.device, &p.config);
                Some(t)
            }
            // Every remaining case says "skip this frame". Skipping is available precisely
            // because the host owns the loop: the renderer is not waiting on us.
            C::Outdated | C::Lost => {
                p.surface.configure(&self.device, &p.config);
                None
            }
            _ => None,
        }
    }

    /// Reconfigure after the host's window changed size. The host owns the size, because it
    /// owns the window.
    pub fn resize(&self, p: &mut Presenting, width: u32, height: u32) {
        let (w, h) = (width.max(1), height.max(1));
        if (w, h) == (p.width, p.height) {
            return;
        }
        p.config.width = w;
        p.config.height = h;
        p.width = w;
        p.height = h;
        p.surface.configure(&self.device, &p.config);
        p.depth = self.depth_texture(w, h);
    }

    /// Read an offscreen target back. The blocking map is on the host's side of the line.
    pub fn depth_view_of(&self, p: &Presenting) -> wgpu::TextureView {
        p.depth.create_view(&Default::default())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FrameStats {
    pub points_drawn: u64,
    pub buffers_drawn: usize,
}

/// A three-line executor, so the prototype does not add `pollster` to answer a question it
/// is not asking. Blocking is fine *here* — this is the host.
fn pollster_block<F: std::future::Future>(mut fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    static VTABLE: RawWakerVTable = RawWakerVTable::new(|_| RawWaker::new(std::ptr::null(), &VTABLE), |_| {}, |_| {}, |_| {});
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = unsafe { std::pin::Pin::new_unchecked(&mut fut) };
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

