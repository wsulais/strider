// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sharing the renderer's device with Qt, on Vulkan.
//!
//! One of two implementations of the same five-call seam; `share_metal.rs` is the other, and
//! `main.rs` picks between them with a single `cfg` at the `use`. Nothing in the frame path
//! knows which backend it is on, which is the point: the Vulkan layout dance below is real and
//! it has no business appearing in `render_current`.
//!
//! Every call may decline. Declining is normal and is not an error — a machine whose adapter is
//! not the one Qt chose, or a Qt built without Vulkan, keeps the readback path, which is the
//! path most machines are on.

use render_gpu::{Gpu, Offscreen};

/// Qt's threaded render loop, which this platform measured as worth forcing: `basic`
/// serialises host extract with Qt's own rendering and cost about half the frame rate.
/// See NOTES.md, "the threaded loop, recovered by narrowing what crosses threads".
pub const RENDER_LOOP: Option<&str> = Some("threaded");

/// The Qt Quick backend a shared device requires, or `None` where the platform's default is
/// already the right one. Read before `QGuiApplication` exists, which is why it is a constant
/// rather than something asked of a device.
pub const QT_RHI_FOR_SHARING: Option<&str> = Some("vulkan");

// C linkage, not a cxx-qt bridge entry. See the note in `share_metal.rs`: the bridge is one
// module shared by both platforms, so a declaration there is compiled on both and unused on one.
unsafe extern "C" {
    /// Hand Qt the `VkInstance`, `VkPhysicalDevice`, `VkDevice` and queue indices.
    fn strider_share_vulkan(
        instance: u64,
        physical_device: u64,
        device: u64,
        queue_family_index: u32,
        queue_index: u32,
    ) -> bool;
}

/// Offer Qt the instance and device the renderer created.
///
/// The direction is deliberate and is argued in `render_gpu::vulkan`: adopting Qt's device would
/// mean telling wgpu-hal which extensions that device was created with, and Qt publishes
/// neither — a superset is undefined behaviour rather than a diagnosable error.
pub fn offer_device(gpu: &Gpu) -> bool {
    match gpu.vulkan_handles() {
        // SAFETY: every argument is a handle this device owns, read as an integer, and Qt is
        // given them before it creates a window — so nothing is using them yet on either side.
        Some(h) => unsafe {
            strider_share_vulkan(
                h.instance,
                h.physical_device,
                h.device,
                h.queue_family_index,
                h.queue_index,
            )
        },
        None => {
            eprintln!("vulkan      renderer is not on Vulkan — reading back instead");
            false
        }
    }
}

/// A target whose colour attachment Qt can sample directly.
///
/// `None` falls back to an ordinary offscreen target and the readback.
pub fn shared_target(gpu: &Gpu, width: u32, height: u32) -> Option<Offscreen> {
    gpu.offscreen_shared(width, height)
}

/// The handle Qt should sample, or 0 when this target is not shared.
pub fn sampled_handle(_gpu: &Gpu, target: &Offscreen) -> u64 {
    target.vulkan_image().unwrap_or(0)
}

/// Give the image to Qt in the layout its descriptor was written with.
///
/// Safe to leave it there: the next frame draws into the *other* image of the pair, so nothing
/// takes this layout back while Qt is still sampling.
pub fn release_to_toolkit(gpu: &Gpu, target: &Offscreen) {
    gpu.transition_shared(target, true);
}

/// Put the image back in the layout wgpu believes it is in, before wgpu touches it.
pub fn reclaim_for_draw(gpu: &Gpu, target: &Offscreen) {
    gpu.transition_shared(target, false);
}
