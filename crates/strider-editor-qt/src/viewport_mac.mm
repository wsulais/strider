// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// PROTOTYPE / THROWAWAY — the Metal half of the viewport. See `viewport_platform.h`.
//
// Objective-C++ because AppKit and Metal types cannot be named from plain C++, which is the same
// reason the platform split exists at all: some platform accessors return handles no binding
// generator, and no portable header, can usefully wrap.

#import <AppKit/AppKit.h>
#import <Metal/Metal.h>

#include <cstdio>

#include <QtQuick/QQuickGraphicsDevice>
#include <QtQuick/QQuickWindow>
#include <QtQuick/QSGTexture>
#include <QtQuick/qsgtexture_platform.h>

#include "viewport_mac.h"
#include "viewport_platform.h"

namespace {

/// The device the renderer created, waiting for a window to attach it to.
///
/// No instance: Metal has nothing corresponding to `QVulkanInstance`, which is also why this
/// backend escapes the ordering constraint that cost the Vulkan path a day — there is no
/// `create()` needing a `QGuiApplication` to already exist.
struct SharedMetal {
    QQuickGraphicsDevice device;
    bool valid = false;
    bool applied = false;
};

SharedMetal &shared()
{
    static SharedMetal s;
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
    window->setGraphicsDevice(s.device);
}

QSGTexture *strider_native_texture(::std::uint64_t handle, QQuickWindow *window, const QSize &size)
{
    if (handle == 0 || window == nullptr) {
        return nullptr;
    }
    // No layout, and no synchronisation argument either. Metal tracks neither, so the entire
    // `VUID-vkCmdDraw-None-09600` problem — three failed attempts on the Vulkan side — has no
    // counterpart here. What remains is the part that was never about Vulkan: the host must not
    // draw into a texture Qt is sampling, which is why it alternates between two.
    return QNativeInterface::QSGMetalTexture::fromNative(
        (__bridge id<MTLTexture>)(void *)handle, window, size);
}

::std::uint64_t strider_window_id(QQuickWindow *window)
{
    if (window == nullptr) {
        return 0;
    }
    return strider_window_number(reinterpret_cast<void *>(window->winId()));
}

::std::uint64_t strider_window_number(void *nsview)
{
    // `QWindow::winId()` is an `NSView *` on macOS. A view that is not yet in a window has no
    // number, which is a normal state during start-up rather than an error.
    //
    // `__bridge`, because this file is compiled with ARC: a plain cast from `void *` to an object
    // pointer is an error there rather than a warning, and `__bridge` is the form that transfers
    // no ownership — Qt owns this view and nothing here should retain or release it.
    NSView *view = (__bridge NSView *)nsview;
    if (view == nil || view.window == nil) {
        return 0;
    }
    return static_cast<::std::uint64_t>(view.window.windowNumber);
}

bool strider_share_metal(::std::uint64_t device, ::std::uint64_t queue)
{
    if (device == 0 || queue == 0) {
        return false;
    }
    auto &s = shared();
    // `fromDeviceAndCommandQueue`, NOT `fromDeviceObjects`.
    //
    // Worth spelling out because the plan written from memory named the latter: that overload is
    // Vulkan's, takes a physical device and two queue indices, and is guarded by
    // `QT_CONFIG(vulkan)`. The Metal one takes the two objects themselves. Reading the header
    // settled in a minute what an inference had got wrong.
    s.device = QQuickGraphicsDevice::fromDeviceAndCommandQueue(
        (__bridge MTLDevice *)(void *)device,
        (__bridge MTLCommandQueue *)(void *)queue);
    s.valid = true;
    std::fprintf(stderr, "metal       sharing the renderer's MTLDevice and queue with Qt\n");
    std::fflush(stderr);
    return true;
}

const char *strider_shared_colour_warning()
{
    return ". COLOUR IS WRONG on this path: Metal linearises an sRGB texture on sample and Qt"
           " writes it unencoded, so the viewport is about 30% dark. Measured; see NOTES.md";
}
