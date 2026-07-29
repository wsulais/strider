// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Metal-specific interop: sharing this device, and its render target, with a host.
//!
//! Sibling of `vulkan.rs`, and deliberately much smaller. Three differences do that, and all
//! three were established from wgpu-hal's own source rather than inferred:
//!
//! * **A wgpu texture's `MTLTexture` is reachable.** `wgpu_hal::metal::Texture::raw_handle()` is
//!   public, where `wgpu_hal::vulkan::Texture` keeps its `raw: vk::Image` private. So there is
//!   no allocate-outside-and-wrap-in inversion here: an *ordinary* `Gpu::offscreen` target can
//!   be handed to a toolkit as it is, and this module needs no allocation code, no memory-type
//!   search, and no second kind of target.
//! * **Metal has no image layouts.** The whole `VUID-vkCmdDraw-None-09600` class of fault — three
//!   failed attempts on the Vulkan side, and a race behind them — does not exist, so there is no
//!   `transition_shared` counterpart to write.
//! * **There is no instance to adopt.** Qt's Vulkan path needs `QVulkanInstance::setVkInstance`
//!   before a device can be shared, with an ordering constraint that cost a day. Metal has no
//!   instance object: a device and a command queue are the whole handshake.
//!
//! What does NOT change is the thing that has bitten every attempt at this: two owners of one
//! texture with no handshake. The host still alternates between two targets so that nothing
//! mutates what the other side may be reading.

use crate::{Gpu, Offscreen};

/// The raw Metal objects behind this device, for a host that must share them.
///
/// Pointers as integers, for the same reason `VulkanHandles` carries integers: this crate hands
/// them to a host that knows what to do with them and cannot itself name an Objective-C type
/// ([[RFC-0006:C-TOOLKIT]] 2 — it has never heard of Qt).
#[derive(Clone, Copy, Debug)]
pub struct MetalHandles {
    /// `id<MTLDevice>`
    pub device: u64,
    /// `id<MTLCommandQueue>`
    pub queue: u64,
}

impl Gpu {
    /// The raw Metal handles, or `None` when this is not a Metal device.
    ///
    /// `None` is a normal answer rather than a failure, exactly as on Vulkan: where wgpu chose
    /// something else the sharing path does not exist and the host reads back instead.
    pub fn metal_handles(&self) -> Option<MetalHandles> {
        // SAFETY: both handles are read and copied out as integers. Nothing is called through
        // them here, nothing is retained or released, and neither is allowed to outlive the
        // `Gpu` that owns the objects they name.
        let device = unsafe { self.device.as_hal::<wgpu::hal::api::Metal>() }?;
        let queue = unsafe { self.queue.as_hal::<wgpu::hal::api::Metal>() }?;
        Some(MetalHandles {
            device: &**device.raw_device() as *const _ as u64,
            queue: queue.as_raw() as *const _ as u64,
        })
    }

    /// The colour attachment's `MTLTexture`, for a host that hands it to a toolkit to sample.
    ///
    /// Asked of an ordinary target: there is no shared variant on this backend, which is the
    /// asymmetry with `vulkan::offscreen_shared` worth noticing rather than smoothing over.
    pub fn metal_texture(&self, target: &Offscreen) -> Option<u64> {
        // SAFETY: the handle is copied out as an integer and outlives nothing. The texture stays
        // owned by the `Offscreen`, which the caller keeps alive while the toolkit samples it.
        let texture = unsafe { target.colour.as_hal::<wgpu::hal::api::Metal>() }?;
        Some(texture.raw_handle() as *const _ as u64)
    }
}
