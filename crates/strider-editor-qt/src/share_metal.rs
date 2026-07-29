// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sharing the renderer's device with Qt, on Metal.
//!
//! One of two implementations of the same seam; `share_vulkan.rs` is the other, and `main.rs`
//! picks between them with a single `cfg` at the `use`.
//!
//! This arm is shorter than its Vulkan sibling and the reason is worth stating, because it is the
//! answer to a question `NOTES.md` left open. `wgpu_hal::metal::Texture::raw_handle()` is public,
//! so the `MTLTexture` wgpu already renders into can be handed straight to Qt — there is no
//! allocate-outside-and-wrap-in inversion, no second kind of target, and (Metal having no image
//! layouts) no transitions to hand back and forth. Two of the five calls below are therefore
//! empty, and that is the design rather than an omission.
//!
//! What is NOT different: two owners of one texture with no handshake. The host still alternates
//! between two targets, and still blocks on the queue before publishing, for the same reason it
//! does on Vulkan — Qt is given a handle, not a promise.

use render_gpu::{Gpu, Offscreen};

/// Nothing forced. Which loops a platform and an RHI backend support is Qt's judgement and it
/// differs by both; on macOS Qt picks the threaded loop by itself — every crash report from this
/// work names `QSGRenderThread` — so there is nothing here worth asserting over it.
pub const RENDER_LOOP: Option<&str> = None;

/// The Qt Quick backend a shared device requires, or `None` where the platform's default is
/// already the right one. Read before `QGuiApplication` exists, which is why it is a constant
/// rather than something asked of a device.
pub const QT_RHI_FOR_SHARING: Option<&str> = None;

// C linkage, not a cxx-qt bridge entry.
//
// The bridge is one `mod ffi` shared by both platforms, so a declaration there is compiled on
// both and unused on one — which is a warning that cannot be fixed without lying about which
// platform uses it. These functions take scalars and return a `bool`, so the C ABI carries them
// with nothing lost, and the declaration then lives beside its only caller. The Rust -> C++
// direction in `viewport.h` already works this way, for a related reason.
unsafe extern "C" {
    /// Hand Qt the `MTLDevice` and `MTLCommandQueue` the renderer created.
    fn strider_share_metal(device: u64, queue: u64) -> bool;
}

/// Offer Qt the device the renderer created.
///
/// The direction matches the Vulkan arm and for the same reason: this crate knows how its own
/// device was made, and adopting Qt's would mean asserting things about a device Qt does not
/// describe. Metal makes it easier — a device and a queue, with no instance and no extension
/// list — but the argument is unchanged.
pub fn offer_device(gpu: &Gpu) -> bool {
    match gpu.metal_handles() {
        // SAFETY: both are handles this device owns, read as integers, and Qt is given them
        // before it creates a window — so nothing is using them yet on either side.
        Some(h) => unsafe { strider_share_metal(h.device, h.queue) },
        None => {
            eprintln!("metal       renderer is not on Metal — reading back instead");
            false
        }
    }
}

/// Always `None`, and unlike the Vulkan arm this is permanent rather than a stub: an ordinary
/// target's own `MTLTexture` is what gets shared, so there is no second kind to build.
pub fn shared_target(_gpu: &Gpu, _width: u32, _height: u32) -> Option<Offscreen> {
    None
}

/// The `MTLTexture` Qt should sample.
pub fn sampled_handle(gpu: &Gpu, target: &Offscreen) -> u64 {
    gpu.metal_texture(target).unwrap_or(0)
}

/// Nothing to do: Metal has no image layouts, so there is no state to hand over.
pub fn release_to_toolkit(_gpu: &Gpu, _target: &Offscreen) {}

/// Nothing to do, likewise. The pre-draw half of the Vulkan barrier pair has no counterpart.
pub fn reclaim_for_draw(_gpu: &Gpu, _target: &Offscreen) {}
