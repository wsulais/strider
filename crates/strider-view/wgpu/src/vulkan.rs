// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Vulkan-specific interop: sharing this device, and its render target, with a host.
//!
//! Separate from `lib.rs` because none of it exists on another backend. Every entry point here
//! returns `None` rather than failing where the renderer is not on Vulkan, which is a normal
//! answer — the host falls back to reading pixels, the path every non-Vulkan machine uses.
//!
//! The Metal equivalent will be a sibling module, not additions to this one.

use crate::{Gpu, Offscreen};

/// The raw Vulkan objects behind this device, for a host that must share them.
///
/// # Why the handles go OUT rather than a foreign device coming IN
///
/// The obvious arrangement is the other way round: let Qt create the device and build wgpu on
/// it. `wgpu_hal::vulkan::Adapter::device_from_raw` accepts exactly that — and it also demands
/// `enabled_extensions` and `features`, which must match what the device was *actually* created
/// with. Qt publishes its `VkDevice`, its `VkPhysicalDevice` and its queue indices, but not the
/// extension list or the feature set it enabled. Passing a superset makes wgpu call entry points
/// that were never enabled, which is undefined behaviour rather than a diagnosable error.
///
/// So the device is created here, where the extension set is known because this crate chose it,
/// and handed to Qt through `QQuickGraphicsDevice::fromDeviceObjects`. Nothing is guessed.
#[derive(Clone, Copy, Debug)]
pub struct VulkanHandles {
    pub instance: u64,
    pub physical_device: u64,
    pub device: u64,
    pub queue_family_index: u32,
    pub queue_index: u32,
}

impl Gpu {
    /// The raw Vulkan handles, or `None` when this is not a Vulkan device.
    ///
    /// `None` is a normal answer rather than a failure: where wgpu chose GL the sharing path
    /// does not exist and the host reads back instead. Both paths stay, because the readback one
    /// is also what a machine without Vulkan gets.
    pub fn vulkan_handles(&self) -> Option<VulkanHandles> {
        use ash::vk::Handle as _;
        // SAFETY: the handles are read and copied out as integers. None is used to call Vulkan
        // here, and none is allowed to outlive the `Gpu` that owns the objects they name.
        let device = unsafe { self.device.as_hal::<wgpu::hal::api::Vulkan>() }?;
        Some(VulkanHandles {
            instance: device.shared_instance().raw_instance().handle().as_raw(),
            physical_device: device.raw_physical_device().as_raw(),
            device: device.raw_device().handle().as_raw(),
            queue_family_index: device.queue_family_index(),
            queue_index: device.queue_index(),
        })
    }
}

impl Gpu {
    /// An offscreen target whose colour attachment Qt can sample directly.
    ///
    /// The image is allocated here, with ash, on this device — then wrapped into wgpu with
    /// `texture_from_raw` and declared `TextureMemory::External`, which is wgpu-hal's own term
    /// for memory it does not own. Rendering then goes through the ordinary `draw` path: the
    /// texture is a `wgpu::Texture` like any other and nothing downstream knows the difference.
    ///
    /// `None` on any backend but Vulkan, and on any allocation failure. Both leave the caller
    /// on the readback path, which is where every non-Vulkan machine already is.
    ///
    /// # Why not simply share the texture wgpu makes
    ///
    /// Because it cannot be reached. `wgpu_hal::vulkan::Texture` holds `raw: vk::Image`
    /// privately with no accessor, while `Device`, `Adapter` and `Queue` all expose theirs.
    /// So the image has to enter wgpu from outside rather than leave it.
    pub fn offscreen_shared(&self, width: u32, height: u32) -> Option<Offscreen> {
        use ash::vk;
        use ash::vk::Handle as _;

        let hal_device = unsafe { self.device.as_hal::<wgpu::hal::api::Vulkan>() }?;
        let raw = hal_device.raw_device();
        let instance = hal_device.shared_instance().raw_instance();
        let physical = hal_device.raw_physical_device();

        // Matches `Rgba8UnormSrgb`, which is what an offscreen target uses. Stated rather than
        // derived: a mismatch between this and the pipeline's format is only caught at the
        // render pass, as a bare "Validation Error" with nothing pointing at the cause.
        let format = vk::Format::R8G8B8A8_SRGB;
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            // `SAMPLED` is what Qt needs; the other two keep this image usable by the same draw
            // and readback paths the unshared target uses.
            .usage(
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC,
            )
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            // Qt builds its own view of this image and does not use the format declared here.
            // Without `MUTABLE_FORMAT` that is `VUID-VkImageViewCreateInfo-image-12397`, and
            // validation is the only thing that says so — the picture looks right regardless.
            .flags(vk::ImageCreateFlags::MUTABLE_FORMAT)
            .initial_layout(vk::ImageLayout::UNDEFINED);

        // SAFETY: `info` is fully populated above and the device outlives the image, which is
        // owned by the `Offscreen` returned here.
        let image = unsafe { raw.create_image(&info, None) }.ok()?;

        let reqs = unsafe { raw.get_image_memory_requirements(image) };
        let props = unsafe { instance.get_physical_device_memory_properties(physical) };
        let type_index = props
            .memory_types_as_slice()
            .iter()
            .enumerate()
            .position(|(i, t)| {
                reqs.memory_type_bits & (1 << i) != 0
                    && t.property_flags
                        .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            })? as u32;

        let alloc = vk::MemoryAllocateInfo::default()
            .allocation_size(reqs.size)
            .memory_type_index(type_index);
        // SAFETY: size and type index come from this image's own requirements.
        let memory = unsafe { raw.allocate_memory(&alloc, None) }.ok()?;
        // SAFETY: freshly allocated memory of the right size and type, bound once.
        unsafe { raw.bind_image_memory(image, memory, 0) }.ok()?;

        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let hal_desc = wgpu::hal::TextureDescriptor {
            label: Some("shared colour"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUses::COLOR_TARGET
                | wgpu::TextureUses::RESOURCE
                | wgpu::TextureUses::COPY_SRC,
            memory_flags: wgpu::hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        };
        // SAFETY: the image was created with the extent, format and usages `hal_desc` declares,
        // and its memory is bound. `External` is accurate: this crate frees it, not wgpu.
        let hal_texture = unsafe {
            hal_device.texture_from_raw(image, &hal_desc, None, wgpu::hal::vulkan::TextureMemory::External)
        };
        drop(hal_device);

        let desc = wgpu::TextureDescriptor {
            label: Some("shared colour"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        };
        // SAFETY: `hal_texture` was just built from this device and `desc` restates the same
        // extent, format and usages.
        let colour = unsafe {
            self.device
                .create_texture_from_hal::<wgpu::hal::api::Vulkan>(hal_texture, &desc)
        };

        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        Some(Offscreen {
            width,
            height,
            colour,
            depth,
            image: Some(image.as_raw()),
        })
    }

    /// Move the shared colour image between the layout wgpu leaves it in and the one Qt samples.
    ///
    /// Two owners, one image, and neither library will yield its layout tracking. wgpu's
    /// `derive_image_layout` matches on exact usage bits, so a render target is always left in
    /// `COLOR_ATTACHMENT_OPTIMAL`; Qt takes the layout passed to `fromNative` as the one the
    /// descriptor is written with and does not barrier from it. Sampling then reads an image in
    /// the wrong layout — `VUID-vkCmdDraw-None-09600`, formally undefined, and it renders fine
    /// on this driver, which is the worst way for it to be wrong.
    ///
    /// So the transition is done here, by hand, and **undone before wgpu next touches the
    /// image**. wgpu believes the layout is `COLOR_ATTACHMENT_OPTIMAL` throughout and by the
    /// time it draws again that is true, so its own tracking stays correct. Qt is told
    /// `SHADER_READ_ONLY_OPTIMAL` and that is true while it samples.
    ///
    /// Cheap: one barrier on a reused command buffer, submitted on a queue that has just been
    /// waited on. It is not free, and it is the price of the two libraries not sharing a
    /// tracker; the alternative is a dummy pass whose only purpose is to make wgpu emit this.
    pub fn transition_shared(&self, target: &Offscreen, to_sampled: bool) -> Option<()> {
        use ash::vk;
        if target.image.is_none() {
            return None;
        }
        let hal_device = unsafe { self.device.as_hal::<wgpu::hal::api::Vulkan>() }?;
        let raw = hal_device.raw_device();
        let queue = hal_device.raw_queue();
        let family = hal_device.queue_family_index();
        use ash::vk::Handle as _;
        let image = vk::Image::from_raw(target.image?);

        let (old, new, src, dst) = if to_sampled {
            (
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                vk::AccessFlags::SHADER_READ,
            )
        } else {
            (
                // `UNDEFINED`, not `SHADER_READ_ONLY_OPTIMAL`.
                //
                // Reclaiming the image for drawing must not assert what it was doing before.
                // On its first use it has never been sampled, and validation said so:
                //
                //   expects VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL
                //   -- instead, current layout is VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL
                //
                // An `UNDEFINED` source layout is legal from any state and discards the
                // contents, which costs nothing here: the next thing that happens is a render
                // pass that clears the whole attachment.
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::AccessFlags::empty(),
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            )
        };

        // SAFETY: every handle below comes from this device, the pool is created and freed
        // within this call, and the queue is idle because the caller waited on it.
        unsafe {
            let pool = raw
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(family)
                        .flags(vk::CommandPoolCreateFlags::TRANSIENT),
                    None,
                )
                .ok()?;
            let buf = raw
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
                .ok()?[0];
            raw.begin_command_buffer(
                buf,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .ok()?;
            let barrier = vk::ImageMemoryBarrier::default()
                .old_layout(old)
                .new_layout(new)
                .src_access_mask(src)
                .dst_access_mask(dst)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            raw.cmd_pipeline_barrier(
                buf,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
            raw.end_command_buffer(buf).ok()?;
            let bufs = [buf];
            let submit = vk::SubmitInfo::default().command_buffers(&bufs);
            raw.queue_submit(queue, &[submit], vk::Fence::null()).ok()?;
            raw.queue_wait_idle(queue).ok()?;
            raw.destroy_command_pool(pool, None);
        }
        Some(())
    }

}
