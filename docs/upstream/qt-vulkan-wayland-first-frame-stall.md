<!--
SPDX-FileCopyrightText: 2026 Strider contributors
SPDX-License-Identifier: CC-BY-4.0
-->

# Qt Quick stops rendering after the first few frames on the Vulkan RHI under Wayland

**Status:** not yet filed. Written to be filed against Qt (`QTBUG`), component *Qt Quick: SceneGraph* or *QPA: Wayland*. Not yet reduced to a minimal example — see [What is still needed](#what-is-still-needed).

**Affects:** Qt 6.11.1, `QSG_RHI_BACKEND=vulkan`, `QT_QPA_PLATFORM=wayland`, niri 26.04.
**Does not affect:** the same application on `QSG_RHI_BACKEND=opengl`, or on xcb, or under weston.

## Summary

A `QQuickWindow` on the **Vulkan** RHI under Wayland renders about four frames after start-up and then stops rendering entirely. The window is mapped and its QML content is visible; it simply never renders again. Requesting a repaint has no effect, from either `QQuickItem::update()` or `QQuickWindow::update()`, called every 100 ms indefinitely.

**Any manual window resize permanently restores rendering**, after which frames arrive continuously with no further intervention.

The **OpenGL RHI is unaffected** on the same machine, same compositor, same application binary, same run script — this is the sharpest discriminator we have and it is a one-word change (`STRIDER_RHI=opengl`).

## Environment

| | |
| --- | --- |
| Qt | 6.11.1 (nixpkgs) |
| Compositor | niri 26.04 — a scrolling tiling compositor built on Smithay |
| Platform plugin | `wayland` |
| RHI backend | `vulkan` (fails) / `opengl` (works) |
| Render loop | `threaded` **and** `basic` both fail |
| GPU | NVIDIA GeForce RTX 4080 SUPER, driver 595.84 |
| Window | 2510x2128, viewport item 2190x2128 |

The application also supplies Qt with its own `VkInstance` and `VkDevice` (`QVulkanInstance::setVkInstance`, `QQuickWindow::setGraphicsDevice(QQuickGraphicsDevice::fromDeviceObjects(...))`). **This is not required to reproduce**: the stall was first observed before any device sharing existed, with Qt creating its own Vulkan device, as soon as the RHI was forced to Vulkan.

## Evidence

Frames were counted by connecting to `QQuickWindow::frameSwapped` and incrementing an atomic. The count is cumulative and never reset.

**Stalled period** — over roughly thirty seconds, several hundred application ticks at 8 ms:

```
tick, swaps total: 3
render_current -> true, renders 1        <- application has content ready
tick, swaps total: 3
render_current -> true, renders 2
tick, swaps total: 4
watchdog continuous=1 paintedSince=0     <- update() called, no paint follows
tick, swaps total: 4
tick, swaps total: 4
...  (repeats for ~30 s, swaps never leaves 4)
```

Throughout, a 100 ms timer calls **both** `QQuickItem::update()` and `QQuickWindow::update()`. `QQuickItem::updatePaintNode` is never invoked.

**Immediately after a manual resize**, unchanged binary, same process:

```
updatePaintNode 3032x2128 node=(nil) ...
tick, swaps total: 5
tick, swaps total: 6
tick, swaps total: 7
...
tick, swaps total: 265           <- and continuing
```

So the compositor, the GPU and the application are all capable of sustained rendering; the scene graph simply does not start until a configure event arrives.

## Expected versus actual

**Expected.** After the window is exposed, `QQuickWindow::update()` results in a scene graph render, and `QQuickItem::update()` on an item with `ItemHasContents` results in `updatePaintNode` being called.

**Actual.** Neither produces a render on the Vulkan RHI under this compositor until an `xdg_toplevel` configure with a changed size is received.

## Diagnosis, to the extent we have one

We have not identified the mechanism. What the evidence constrains:

- **It is not the render loop.** `threaded` and `basic` both stall identically. This rules out the frame-callback wait specific to the threaded loop, which was our first hypothesis.
- **It is not item-level dirty tracking.** `QQuickWindow::update()` requests a whole scene graph pass and is equally ineffective.
- **It is not the application failing to ask.** The requests are made on the GUI thread, on a timer, hundreds of times.
- **It is specific to the Vulkan RHI.** OpenGL on the identical setup starts immediately.

The most plausible remaining explanation is that Qt's Vulkan swapchain is not created, or is created and then invalidated, before the first `xdg_surface` configure carrying a size — and that Qt does not retry until a configure arrives. That would explain why a resize is a permanent fix and why OpenGL, whose surface handling differs, is unaffected. **We have not verified this**, and it should be treated as a hypothesis rather than part of the report until it is.

## Workaround

Manufacture a configure event shortly after the window exists:

```cpp
QTimer::singleShot(250, this, [this] {
    auto *win = window();
    if (!win) return;
    const int w = win->width();
    win->setWidth(w + 1);
    QTimer::singleShot(50, this, [this, w] {
        if (auto *win = window()) win->setWidth(w);
    });
});
```

Crude, and it is the workaround in use. The alternative for affected applications is to stay on the OpenGL RHI, which is not available to anything needing Vulkan texture interop (`QSGVulkanTexture::fromNative`).

## What is still needed

Before filing, this wants a **minimal reproducer**: a bare `QQuickWindow` with a coloured `Rectangle`, run with `QSG_RHI_BACKEND=vulkan QT_QPA_PLATFORM=wayland` under niri, printing a `frameSwapped` count. If that stalls, the report is complete and the application above is irrelevant to it. If it does *not* stall, the next variables to add, in order:

1. A `QQuickItem` subclass with `ItemHasContents` and an `updatePaintNode` that returns `nullptr` until content exists — our item does this, and an item that contributes no node may matter.
2. An externally supplied `VkInstance`/`VkDevice`.
3. A window large enough to matter — ours is 2510x2128 on a HiDPI output, and buffer-size or scale mismatch is not ruled out.

It is also worth checking against other wlroots- and Smithay-based compositors (sway, Hyprland) to establish whether this is niri-specific or general to Wayland compositors that Qt's Vulkan path handles differently from weston. We could not reproduce it under weston on its x11 backend, but that is a weak negative: weston-on-Xvfb is not a representative Wayland session.

## Provenance

Found while building the Qt host in `prototypes/PROTOTYPE-renderer-host` (throwaway; see its `NOTES.md` for the full investigation, including six failed fixes that all assumed the fault was in item invalidation). The measurement that settled it was a cumulative swap counter — an earlier per-tick counter read zero always, because the tick drained it before printing, and that false reading cost two rounds of wrong diagnosis.
