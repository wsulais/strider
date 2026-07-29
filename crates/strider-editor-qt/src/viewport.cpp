// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#include "viewport.h"

#include <QtCore/QThread>
#include <QtGui/QGuiApplication>
#include <QtGui/QImage>
#include <QtGui/QSurfaceFormat>
#include <QtQuick/QQuickWindow>
#include <QtQuick/QSGRendererInterface>
#include <cstdio>
#include <QtQuick/QSGSimpleTextureNode>
#include <QtQuick/qsgtexture_platform.h>

// Counts the first frames so the cold start can be read off a log instead of guessed at.
//
// Three fixes for "nothing appears until you resize" have now been written and none worked,
// because the fault does not reproduce under Xvfb — it has only ever been seen on a real
// display. So this reports what the item actually does, on the machine where it goes wrong.
static int g_trace = -1;
static bool tracing()
{
    if (g_trace < 0) {
        g_trace = qEnvironmentVariableIsSet("STRIDER_TRACE") ? 40 : 0;
    }
    return g_trace > 0;
}

#include <QtQml/qqml.h>
#include <QtQuick/QQuickItem>

#include "viewport_platform.h"

// The whole of the C++ this application now needs.
//
// Compare what it replaced: a `QWindow` subclass with mouse, wheel and key handlers,
// platform-surface teardown, geometry forwarding, native-handle extraction for two windowing
// systems, and an explicit destroy — about 170 lines, most of it input plumbing that QML
// already does. What remains is a paint.
StriderViewportItem::StriderViewportItem(QQuickItem *parent)
  : QQuickItem(parent)
{
    // `Image`, not `FramebufferObject`. The reason recorded here used to be that an FBO target
    // paints on the scene graph's render thread while an image target does not, so every call
    // into Rust would stay on one thread. **That was wrong** — under the threaded render loop
    // Qt calls `paint` on the render thread whatever the target — and believing it cost a day
    // of blaming Vulkan for a blank viewport.
    //
    // Painting no longer needs to be on any particular thread. The host renders on its own
    // thread from `tick` and publishes the pixels; `paint` locks, copies, and draws. The image
    // target remains the right choice because it needs no graphics interop, but it guarantees
    // nothing about threads and nothing here relies on it doing so.
    // A plain item with contents. `QQuickPaintedItem` is gone: it existed to give the readback
    // a `QPainter`, and the readback is now one of two texture sources behind one node.
    setFlag(QQuickItem::ItemHasContents, true);
    setAcceptedMouseButtons(Qt::AllButtons);

    // The window is not available in the constructor, so wait for it. Both hooks, because
    // `windowChanged` alone did not fire and the connection was never made.
    connect(this, &QQuickItem::windowChanged, this, &StriderViewportItem::attachToWindow);

    // Deliberately slower than a frame. This is a watchdog, not a clock: if it ran at frame rate
    // it would take over pacing from the display, which is the bug the frameSwapped driver
    // replaced. At 100 ms it costs nothing while a swap chain is alive and restarts one that has
    // stalled within a tenth of a second.
    m_watchdog.setInterval(100);
    connect(&m_watchdog, &QTimer::timeout, this, &StriderViewportItem::onWatchdog);
    m_watchdog.start();
}

void StriderViewportItem::onWatchdog()
{
    if (tracing()) {
        std::fprintf(stderr,
                     "trace       watchdog continuous=%d paintedSince=%d\n",
                     int(m_continuous), int(m_paintedSinceCheck));
        std::fflush(stderr);
    }
    if (m_continuous && !m_paintedSinceCheck) {
        // BOTH, and the second is the one that matters.
        //
        // `QQuickItem::update` marks this item dirty and relies on the window scheduling a
        // render. On two compositors here that is enough; on a real display it demonstrably is
        // not — the watchdog reports `paintedSince=0` indefinitely while calling `update()`
        // every tick, so the request is being made and no render follows.
        //
        // `QQuickWindow::update` asks for a whole scene graph pass instead of marking one item.
        // It is a heavier request and a different code path, which is the point: item dirty
        // tracking is exactly what appears not to be working.
        update();
        if (auto *w = window()) {
            w->update();
        }
    }
    m_paintedSinceCheck = false;
}

void StriderViewportItem::componentComplete()
{
    QQuickItem::componentComplete();
    attachToWindow();
}

void StriderViewportItem::attachToWindow()
{
    if (m_swapConnection)
        disconnect(m_swapConnection);
    auto *w = window();
    if (w == nullptr) {
        return;
    }
    // Before the window is shown. What that takes differs by backend and this file does not
    // need to know: one call, implemented once per platform, idempotent by contract.
    strider_apply_shared_graphics(w);
    // Queued, because under the threaded render loop `frameSwapped` is emitted on the scene
    // graph's render thread and `QQuickItem::update` belongs to the GUI thread. A direct
    // connection here calls `update` from the wrong thread — which mostly works, right up until
    // it does not.
    m_swapConnection = connect(w,
                               &QQuickWindow::frameSwapped,
                               this,
                               &StriderViewportItem::onFrameSwapped,
                               Qt::QueuedConnection);
    // Once only, even though this can run twice now that both hooks call it.
    if (!m_reported) {
        m_reported = true;
        reportGraphics(w);
    }
    if (m_continuous) {
        update();
    }

    // Nudge the window's size once, shortly after it exists.
    //
    // This is a WORKAROUND for something below this application, and it is deliberately the
    // crudest possible one: it does by hand what the user was doing by hand.
    //
    // Measured on niri: Qt renders four frames at start-up and then stops. `swaps total` sits
    // at 4 for thirty seconds while the watchdog calls both `QQuickItem::update` and
    // `QQuickWindow::update` every 100 ms, on BOTH render loops. So Qt is not skipping the item,
    // it is not rendering at all, and nothing on the invalidation side can reach it. A manual
    // window resize restores it permanently — swaps then climb past 250 without a pause.
    //
    // The trigger is a configure event, so one is manufactured. `STRIDER_NO_NUDGE=1` disables
    // it, which is how to check whether the underlying fault has been fixed elsewhere.
    //
    // Restricted to the platform the fault is on. It manufactures a resize, and a resize is the
    // one path that replaces a live texture — which is its own source of crashes (see NOTES.md,
    // "the exit crash is a resize crash"). Doing that on every platform to work around one
    // platform's bug trades a stall we do not have for a crash we do.
    const bool wayland = QGuiApplication::platformName() == QLatin1String("wayland");
    if (wayland && !m_nudged && !qEnvironmentVariableIsSet("STRIDER_NO_NUDGE")) {
        m_nudged = true;
        QTimer::singleShot(250, this, [this] {
            auto *win = window();
            if (win == nullptr) {
                return;
            }
            const int w = win->width();
            win->setWidth(w + 1);
            QTimer::singleShot(50, this, [this, w] {
                if (auto *win = window()) {
                    win->setWidth(w);
                    std::fprintf(stderr, "viewport    nudged the window size to force a configure\n");
                    std::fflush(stderr);
                }
            });
        });
    }
}

// What Qt ACTUALLY chose, printed once. Reported rather than assumed because the setting that
// governs tearing depends on the backend, and the two disagree about who owns it:
//
//   * `QSurfaceFormat::swapInterval` is honoured by the OpenGL backend only. On Vulkan, Metal
//     and D3D the present mode governs and the interval is silently ignored — so a request for
//     1 can be accepted, reported, and mean nothing.
//   * The driver may refuse a swap interval it was asked for, in which case the *effective*
//     value on the window differs from the default format's.
//
// Printing both is what turns "it tears" from a guess into a measurement.
void StriderViewportItem::reportGraphics(QQuickWindow *w)
{
    const char *api = "unknown";
    switch (w->rendererInterface() ? w->rendererInterface()->graphicsApi()
                                   : QSGRendererInterface::Unknown) {
        case QSGRendererInterface::OpenGL:      api = "OpenGL"; break;
        case QSGRendererInterface::Vulkan:      api = "Vulkan"; break;
        case QSGRendererInterface::Metal:       api = "Metal"; break;
        case QSGRendererInterface::Direct3D11:  api = "D3D11"; break;
        case QSGRendererInterface::Direct3D12:  api = "D3D12"; break;
        case QSGRendererInterface::Software:    api = "software"; break;
        default:                                api = "unknown"; break;
    }
    const int requested = QSurfaceFormat::defaultFormat().swapInterval();
    const int effective = w->format().swapInterval();
    // `fprintf`, not `qInfo`: Qt's logging rules can filter info messages depending on how
    // the build and the environment are configured, and a diagnostic that might not appear is
    // not a diagnostic. This one has to be unconditional.
    std::fprintf(stderr,
                 "rhi         %s, swap interval requested %d, effective %d%s\n",
                 api,
                 requested,
                 effective,
                 (qstrcmp(api, "OpenGL") == 0)
                     ? ""
                     : "  <- swap interval is IGNORED on this backend; the present mode governs");
    std::fprintf(stderr,
                 "loop        QSG_RENDER_LOOP=%s\n",
                 qEnvironmentVariableIsSet("QSG_RENDER_LOOP")
                     ? qgetenv("QSG_RENDER_LOOP").constData()
                     : "(default)");
    std::fflush(stderr);
}

void StriderViewportItem::setContinuous(bool on)
{
    if (m_continuous == on)
        return;
    m_continuous = on;
    Q_EMIT continuousChanged();
    if (on)
        update();
}

void StriderViewportItem::onFrameSwapped()
{
    // Geometry printed from here rather than from `paint`, because a zero-size
    // `QQuickPaintedItem` gets no paint node at all — Qt never calls `paint`, so instrumenting
    // `paint` cannot report the one state that makes it silent. Swaps do happen, so this runs.
    if (!m_sizeReported) {
        m_sizeReported = true;
        std::fprintf(stderr,
                     "viewport    %gx%g at (%g,%g), visible %d, opacity %g, window %gx%g\n",
                     width(), height(), x(), y(), int(isVisible()), opacity(),
                     window() ? window()->width() * 1.0 : -1.0,
                     window() ? window()->height() * 1.0 : -1.0);
        // The window's place on the SCREEN, in logical units, so a harness can crop to the
        // viewport without a window manager to ask.
        //
        // On X11 that question goes to `xwininfo`, and getting it wrong measured the chrome
        // panel three times. There is no equivalent client on macOS — `screencapture` takes the
        // whole screen and nothing tells a shell script where a window landed — so the
        // application publishes what it already knows. It is the same principle the readouts
        // follow: ask the application, not the screen.
        if (auto *w = window()) {
            std::fprintf(stderr,
                         "geometry    window %dx%d+%d+%d logical, viewport %gx%g+%g+%g in it\n",
                         w->width(), w->height(), w->x(), w->y(),
                         width(), height(), x(), y());
            // Zero where the platform has no id a screenshot tool can use, which is Wayland
            // and is why the harness has never been able to capture that window directly.
            if (const auto id = strider_window_id(w); id != 0) {
                std::fprintf(stderr,
                             "capture     window id %llu\n",
                             static_cast<unsigned long long>(id));
            }
        }
        std::fflush(stderr);
    }
    strider_swapped();
    // Asking for the next frame from inside the swap is what produces a continuous loop at the
    // display's own rate. It cannot starve the event loop the way a zero-interval timer does,
    // because the next swap gates it — which is the bug this replaces.
    if (m_continuous)
        update();
}

QSGNode *StriderViewportItem::updatePaintNode(QSGNode *node, UpdatePaintNodeData *)
{
    // DEVICE pixels, not logical ones.
    //
    // The item's `width()` is in logical units, and on any display with a scale factor — every
    // Mac, and a fractionally-scaled Linux desktop — that is fewer pixels than the item covers.
    // Rendering at the logical size and letting the scene graph stretch the result is a viewport
    // that is soft everywhere, which for a point cloud reads as the renderer having lost points
    // rather than as a resolution mistake.
    //
    // The RECT below stays logical, because that is what the scene graph positions with. Only
    // the texture and what the host draws into are in pixels.
    const qreal dpr = window() != nullptr ? window()->effectiveDevicePixelRatio() : 1.0;
    const auto w = static_cast<::std::uint32_t>(width() * dpr);
    const auto h = static_cast<::std::uint32_t>(height() * dpr);
    if (!m_dprReported && w != 0) {
        m_dprReported = true;
        std::fprintf(stderr,
                     "viewport    %gx%g logical at dpr %g -> %ux%u pixels\n",
                     width(), height(), dpr, w, h);
        std::fflush(stderr);
    }
    if (w == 0 || h == 0) {
        if (!m_warnedEmpty) {
            m_warnedEmpty = true;
            std::fprintf(stderr, "viewport    ZERO SIZE — nothing to draw\n");
            std::fflush(stderr);
        }
        // The FUNCTOR overload, not the name one. `QQuickItem::update` is a plain public
        // function — not a slot, not `Q_INVOKABLE` — so `invokeMethod(this, "update", ...)`
        // fails the meta-object lookup and returns false *silently*. This fix was written twice
        // in that form and did nothing both times, which is why the first paint still needed a
        // window resize to appear.
        QMetaObject::invokeMethod(this, [this] { update(); }, Qt::QueuedConnection);
        delete node;
        return nullptr;
    }

    // Tell the host what to draw at. It cannot ask Qt: it does not know it is inside Qt, and
    // this call is not necessarily on the host's thread.
    strider_viewport_size(w, h);
    m_paintedSinceCheck = true;
    if (tracing()) {
        --g_trace;
        std::fprintf(stderr,
                     "trace       updatePaintNode %ux%u node=%p shared=%llu tex=%p\n",
                     w, h, (void *)node,
                     (unsigned long long)strider_shared_image(), (void *)m_texture);
        std::fflush(stderr);
    }

    // Braces, not parentheses: `QSize size(int(w), int(h))` is parsed as a function
    // declaration, and every later use of `size` then fails in a way that names the wrong thing.
    const QSize size{int(w), int(h)};
    const ::std::uint64_t sharedImage = strider_shared_image();

    // Rebuilt only when something about the texture actually changed. A `QSGTexture` per frame
    // would undo the point of the shared path on one side and thrash uploads on the other.
    // The NODE owns the texture, so replacing it is `setTexture` and nothing else.
    //
    // This used to `delete m_texture` here while the `QSGSimpleTextureNode` still referenced it,
    // which is a use-after-free the scene graph trips over on its next render. It only showed up
    // on a resize — the one path that replaces a live texture — which is why it presented as
    // "closing crashes" and why simulating a resize with the start-up nudge made every run
    // crash, including ones that had been clean for days.
    if (m_texture != nullptr && (m_textureSize != size || m_textureImage != sharedImage)) {
        m_texture = nullptr;
    }

    if (sharedImage != 0) {
        if (m_texture == nullptr) {
            // A handle the renderer published, wrapped by whichever platform file was compiled.
            // What kind of handle it is — a `VkImage`, an `id<MTLTexture>` — is not a question
            // this file can ask, and the two paths are one node with two sources rather than two
            // rendering designs.
            m_texture = strider_native_texture(sharedImage, window(), size);
            if (m_texture != nullptr && !m_sharedTextureReported) {
                m_sharedTextureReported = true;
                std::fprintf(stderr,
                             "viewport    sampling the renderer's own texture — no readback%s\n",
                             strider_shared_colour_warning());
                std::fflush(stderr);
            }
        }
    } else {
        // The readback path, unchanged in substance and now expressed as the same node. Every
        // non-Vulkan machine lands here, so it is not a fallback in the sense of being worse
        // tested — it is the ordinary case.
        if (m_frame.width() != int(w) || m_frame.height() != int(h)) {
            m_frame = QImage(int(w), int(h), QImage::Format_RGBA8888);
        }
        if (strider_copy_frame(w, h, m_frame.bits(), ::std::size_t(m_frame.sizeInBytes()))) {
            m_texture = window()->createTextureFromImage(m_frame);
        }
    }

    m_textureSize = size;
    m_textureImage = sharedImage;

    if (m_texture == nullptr && tracing()) {
        std::fprintf(stderr, "trace       no texture — asking for another paint\n");
        std::fflush(stderr);
    }
    if (m_texture == nullptr) {
        // Nothing to show yet — a resize in flight, or the host has not rendered its first
        // frame. Returning null is TERMINAL unless something asks again: Qt only calls this
        // when the item is dirty, and the thing that normally dirties it is the previous
        // frame's swap. That is the whole of "nothing appears until you resize the window" —
        // a resize was the only external event invalidating the item.
        //
        // Queued because this may be the render thread, and `update` belongs to the GUI one.
        // The FUNCTOR overload, not the name one. `QQuickItem::update` is a plain public
        // function — not a slot, not `Q_INVOKABLE` — so `invokeMethod(this, "update", ...)`
        // fails the meta-object lookup and returns false *silently*. This fix was written twice
        // in that form and did nothing both times, which is why the first paint still needed a
        // window resize to appear.
        // The node is KEPT, not deleted. Destroying it removes the item's content, and an item
        // with no content is one Qt has less reason to visit again — which compounds the
        // start-up stall rather than recovering from it. Returning it unchanged leaves whatever
        // was last shown on screen, which for the first frame is nothing and for a resize is
        // the previous image: both better than a hole.
        //
        // The request below is kept as a backstop, but it is not what recovers the cold start:
        // issued from inside the sync phase, Qt drops it. `Editor::tick` publishing
        // `needsPaint` after a successful render is the path that actually works.
        QMetaObject::invokeMethod(this, [this] { update(); }, Qt::QueuedConnection);
        return node;
    }

    auto *textureNode = static_cast<QSGSimpleTextureNode *>(node);
    if (textureNode == nullptr) {
        textureNode = new QSGSimpleTextureNode;
        textureNode->setFiltering(QSGTexture::Linear);
    }
    // Ownership handed to the node, which destroys the previous texture when a new one is set —
    // at a point in the frame where it is safe to, which this file cannot know from outside.
    textureNode->setOwnsTexture(true);
    // ONLY when it actually changed, and this is a crash rather than an optimisation.
    //
    // `QSGSimpleTextureNode::setTexture` deletes the texture it owns and *then* stores the
    // pointer it was given, without comparing the two. Handing it the texture it already holds is
    // therefore a use-after-free: it destroys the object and assigns the freed pointer, and the
    // next thing to touch the material dereferences it.
    //
    // The readback path hid this for as long as it existed, because `createTextureFromImage`
    // returns a fresh texture every frame and the pointer was never the same twice. The shared
    // path reuses one wrapper for as long as the handle is unchanged — which, with adaptive
    // rendering, is every paint between two host renders — so it crashed on the second paint of
    // the first frame. Named by the crash report in one run: `QSGOpaqueTextureMaterial::setTexture`
    // on the QSGRenderThread, EXC_BAD_ACCESS on a pointer that had just been freed.
    if (textureNode->texture() != m_texture) {
        textureNode->setTexture(m_texture);
    }
    textureNode->setRect(0, 0, width(), height());
    // Once, and it is what a harness has to wait for.
    //
    // Every earlier line — the window's geometry, the adapter, the RHI — is printed before the
    // host has rendered anything, so a screenshot triggered by any of them catches an empty
    // viewport and reports that nothing was presented. That is the seventh mis-measurement in
    // this harness's history and the first one on macOS. This line means a texture with the
    // host's pixels in it is in the scene graph; the swap after it puts them on screen.
    if (!m_firstFrameReported) {
        m_firstFrameReported = true;
        std::fprintf(stderr, "viewport    first frame in the scene graph, %ux%u\n", w, h);
        std::fflush(stderr);
    }
    return textureNode;
}

void strider_prepare_graphics(bool vsync)
{
    QSurfaceFormat format = QSurfaceFormat::defaultFormat();
    format.setSwapInterval(vsync ? 1 : 0);
    // NOT `setColorSpace(QColorSpace::SRgb)`.
    //
    // That was one attempt at the shared path's colour shift, on the reasoning that an sRGB
    // swapchain would restore the encode Qt's write skips. It changed nothing measurable —
    // 0.3566 mean before and after, against the readback path's 0.5172 — so either Qt does not
    // reach the Metal swapchain's format from here or the flag does not do what was assumed. The
    // line is gone rather than left in with a comment claiming a fix, and the attempt is recorded
    // in NOTES.md where refuted attempts belong.
    QSurfaceFormat::setDefaultFormat(format);
    if (!vsync) {
        // Belt and braces: the scene graph honours this even where the platform ignores the
        // swap interval.
        qputenv("QSG_NO_VSYNC", "1");
    }
}

void strider_quit(int code)
{
    QGuiApplication::exit(code);
}

void strider_register_qml_types()
{
    qmlRegisterType<StriderViewportItem>("com.strider.viewport", 1, 0, "Viewport");
}
