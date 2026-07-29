// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// PROTOTYPE / THROWAWAY — the Vulkan half of the viewport. See `viewport_platform.h`.

#include <cstdio>

#include <QtGui/QGuiApplication>
#include <QtGui/QVulkanInstance>
#include <QtQuick/QQuickGraphicsDevice>
#include <QtQuick/QQuickWindow>
#include <QtQuick/QSGTexture>
#include <QtQuick/qsgtexture_platform.h>

#include "viewport_platform.h"
#include "viewport_vulkan.h"

namespace {

/// The instance and device the renderer created, waiting for a window to attach them to.
///
/// A file-local static rather than something the portable header exposes: what a backend needs to
/// remember between `strider_share_vulkan` and the first window is that backend's business.
struct SharedVulkan {
    QVulkanInstance *instance = nullptr;
    QQuickGraphicsDevice device;
    bool valid = false;
    bool applied = false;
};

SharedVulkan &shared()
{
    static SharedVulkan s;
    return s;
}

} // namespace

void strider_apply_shared_graphics(QQuickWindow *window)
{
    auto &s = shared();
    if (window == nullptr || !s.valid || s.applied) {
        return;
    }
    s.applied = true;
    // Both halves, and before the window is shown. Qt creates its own instance otherwise, and a
    // device belonging to one instance used with a surface from another is invalid.
    if (s.instance != nullptr) {
        window->setVulkanInstance(s.instance);
    }
    window->setGraphicsDevice(s.device);
}

QSGTexture *strider_native_texture(::std::uint64_t handle, QQuickWindow *window, const QSize &size)
{
    if (handle == 0 || window == nullptr) {
        return nullptr;
    }
    // The layout stated is the one the renderer leaves the image in after drawing to it as a
    // colour attachment. Qt barriers from here to shader-read itself; stating the wrong one is not
    // an error Qt can detect, it is a garbled or blank frame.
    return QNativeInterface::QSGVulkanTexture::fromNative(
        reinterpret_cast<VkImage>(handle),
        VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL,
        window,
        size);
}

::std::uint64_t strider_window_id(QQuickWindow *window)
{
    if (window == nullptr) {
        return 0;
    }
    // `winId()` is the X11 window id under xcb, which is exactly what `xwininfo -id` and
    // `import -window` want. Under Wayland it is a pointer to a platform window and means nothing
    // to any screenshot tool, so it is refused rather than published as a number that looks
    // usable — an id that is wrong is worse than an id that is absent, which is the lesson six
    // mis-measurements in NOTES.md keep teaching.
    if (QGuiApplication::platformName() != QLatin1String("xcb")) {
        return 0;
    }
    return static_cast<::std::uint64_t>(window->winId());
}

bool strider_share_vulkan(::std::uint64_t instance,
                          ::std::uint64_t physical_device,
                          ::std::uint64_t device,
                          ::std::uint32_t queue_family_index,
                          ::std::uint32_t queue_index)
{
    if (instance == 0 || device == 0 || physical_device == 0) {
        return false;
    }
    // Adopted, not created. `setVkInstance` makes `QVulkanInstance` a handle onto an existing
    // instance rather than the owner of a new one, which is the only way Qt's device and the
    // renderer's can be the same device.
    static QVulkanInstance vkInstance;
    vkInstance.setVkInstance(reinterpret_cast<VkInstance>(instance));
    if (!vkInstance.create()) {
        std::fprintf(stderr,
                     "vulkan      Qt refused the renderer's VkInstance (%d) - reading back instead\n",
                     int(vkInstance.errorCode()));
        std::fflush(stderr);
        return false;
    }
    auto &s = shared();
    s.instance = &vkInstance;
    // Kept rather than applied. `setGraphicsDevice` is a member of `QQuickWindow`, not a static,
    // so it cannot be called until there is a window — and it must be called before that window
    // is first shown. `strider_apply_shared_graphics` is the one place that satisfies both.
    s.device = QQuickGraphicsDevice::fromDeviceObjects(
        reinterpret_cast<VkPhysicalDevice>(physical_device),
        reinterpret_cast<VkDevice>(device),
        int(queue_family_index),
        int(queue_index));
    s.valid = true;
    std::fprintf(stderr,
                 "vulkan      sharing the renderer's device with Qt (queue family %u, index %u)\n",
                 unsigned(queue_family_index),
                 unsigned(queue_index));
    std::fflush(stderr);
    return true;
}

const char *strider_shared_colour_warning()
{
    // Nothing on this backend: Qt builds its own view of the image and takes it as UNORM, so no
    // conversion happens on sample. `VK_IMAGE_CREATE_MUTABLE_FORMAT_BIT` is what makes that legal
    // and it is set where the image is created.
    return "";
}
