// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// PROTOTYPE / THROWAWAY — what every platform owes the portable viewport.
//
// `viewport.cpp` contains no `#ifdef` and names no platform. Everything that differs between
// Vulkan and Metal is behind these four functions, implemented once per backend —
// `viewport_vulkan.cpp` and `viewport_mac.mm` — and selected by `build.rs` from the target.
//
// The split follows the one on the Rust side, deliberately: `share_vulkan.rs` and
// `share_metal.rs` sit behind a single `share::`, and these sit behind a single header. A reader
// looking for "what is different about macOS" finds two files rather than a dozen branches.
#ifndef STRIDER_VIEWPORT_PLATFORM_H
#define STRIDER_VIEWPORT_PLATFORM_H

#include <cstdint>

#include <QtCore/QSize>

class QQuickWindow;
class QSGTexture;

// Apply the device the renderer offered, if one was offered and accepted.
//
// Must run before the window is first shown. On Vulkan this is two calls — the instance matters
// as much as the device, since a device from one instance used with a surface from another is
// invalid — and on Metal it is one, there being no instance object to adopt.
void strider_apply_shared_graphics(QQuickWindow *window);

// Wrap a handle the renderer published as a `QSGTexture` the scene graph can sample.
//
// The handle is a `VkImage` or an `id<MTLTexture>` depending on which file is compiled, and the
// portable side neither knows nor asks. Returns null if the platform declines, which leaves the
// caller on the readback path.
QSGTexture *strider_native_texture(::std::uint64_t handle, QQuickWindow *window, const QSize &size);

// The window's id in whatever namespace this platform's screenshot tools use: a CGWindowID on
// macOS, an X11 window id under xcb, 0 where there is no such thing (Wayland, by design).
//
// Published because no shell tool can ask: `xwininfo` needs an id to start from and macOS has no
// equivalent client at all. It is the same principle as the geometry line — ask the application,
// not the screen — and it is what lets one harness capture one window on both platforms.
::std::uint64_t strider_window_id(QQuickWindow *window);

// What is known to be wrong about this platform's shared path, appended to the line announcing
// it, or "" where nothing is. A defect nobody is told about is how a plausible wrong image gets
// mistaken for a correct one, and this prototype has been fooled that way repeatedly.
const char *strider_shared_colour_warning();

#endif // STRIDER_VIEWPORT_PLATFORM_H
