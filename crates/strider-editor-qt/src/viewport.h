// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// PROTOTYPE / THROWAWAY.
//
// The viewport as an ordinary QML item.
//
// This replaces a native child `QWindow` that wgpu presented to directly. That arrangement
// was the one [[ADR-0009]] chose — no graphics interop at all — and it worked, but it put the
// viewport outside the scene graph, which cost three things at once: QML could not draw over
// it, a child window cannot take keyboard focus independently of its top level, and every
// pointer and key event had to be forwarded by hand from C++ into Rust.
//
// A `QQuickPaintedItem` gets pointer, keyboard, focus, layout and overlay from QML for free.
// It is [[RFC-0006:C-SURFACE]] 4's offscreen case — "a target it does not present directly,
// whose result the host composites" — which the renderer already supported, so nothing below
// this application changed to make the swap.
//
// What it costs is a readback per frame: about 3 MB at 1000x760, so roughly 180 MB/s at
// 60 Hz. That is the price of not doing per-backend external-memory interop, which is exactly
// the work [[ADR-0009]] deferred until depth-insensitive overlay is wanted.
#ifndef STRIDER_VIEWPORT_H
#define STRIDER_VIEWPORT_H

#include <cstdint>

#include <QtCore/QTimer>
#include <QtGui/QImage>

#include <QtQuick/QQuickItem>

class QQuickWindow;
class QSGTexture;

// Declared in the header because moc is run against the header and its output is compiled as
// its own translation unit. With the class in the .cpp the generated file cannot see it.
class StriderViewportItem : public QQuickItem
{
    Q_OBJECT
    Q_PROPERTY(bool continuous READ continuous WRITE setContinuous NOTIFY continuousChanged)
public:
    explicit StriderViewportItem(QQuickItem *parent = nullptr);
    // Replaces `QQuickPaintedItem::paint`. One node, two texture sources: the renderer's own
    // `VkImage` where the device is shared, and an image uploaded per frame where it is not.
    // Collapsing both onto a `QSGSimpleTextureNode` is worth doing on its own — it is the same
    // scene graph object either way, so the shared path is not a second rendering design.
    QSGNode *updatePaintNode(QSGNode *node, UpdatePaintNodeData *data) override;
    // The reliable hook. `windowChanged` did not fire here, so the swap connection was never
    // made and the continuous loop silently did nothing — the kind of failure that looks like
    // it works because some other path happens to repaint.
    void componentComplete() override;

    bool continuous() const { return m_continuous; }
    void setContinuous(bool on);

Q_SIGNALS:
    void continuousChanged();

private Q_SLOTS:
    // Re-prime the loop if a swap has stopped arriving.
    //
    // Under the basic render loop Qt renders only when something asks it to, and the only thing
    // asking is the previous render — so the frameSwapped -> update chain is self-sustaining
    // right up until it misses one beat, after which nothing ever repaints again. That is the
    // "needs a window resize before anything appears" symptom: a resize is the external
    // invalidation that restarts it.
    void onWatchdog();

    // Fires once per actual buffer swap, which is the only thing that knows the display's real
    // rate. `FrameAnimation` does not: it is driven by Qt's animation driver, a fixed 60 Hz
    // timer, so it reports 60 on a 144 Hz monitor and looks like a cap in the renderer.
    void onFrameSwapped();

private:
    void attachToWindow();
    void reportGraphics(QQuickWindow *w);
    bool m_continuous = true;
    bool m_reported = false;
    bool m_warnedEmpty = false;
    bool m_sizeReported = false;
    bool m_paintReported = false;
    QImage m_frame;
    // Recreated when the size changes, and on the shared path when the image handle changes.
    QSGTexture *m_texture = nullptr;
    ::std::uint64_t m_textureImage = 0;
    QSize m_textureSize;
    bool m_sharedTextureReported = false;
    bool m_nudged = false;
    bool m_paintedSinceCheck = false;
    bool m_dprReported = false;
    bool m_firstFrameReported = false;
    QTimer m_watchdog;
    QMetaObject::Connection m_swapConnection;
};

// Register the QML types. Called from Rust before the engine loads anything.
void strider_register_qml_types();

// End the event loop.
void strider_quit(int code);

// Rust side of the paint. Returns a pointer to `width * height` RGBA8 pixels valid until the
// next call, or null if there is nothing to show yet.
//
// C linkage rather than cxx's `extern "Rust"`, for the reason the previous version of this
// file recorded: cxx's generated header pulls in the whole cxx-qt object model, and this
// translation unit has no business seeing it.
extern "C" {
// The renderer's colour attachment as a `VkImage`, or 0 when it is not shared.
::std::uint64_t strider_shared_image();
// Tell the host the item's size. It renders at that size on its own thread.
void strider_viewport_size(::std::uint32_t width, ::std::uint32_t height);
// Copy the latest frame, if one exists at exactly this size. False means "nothing to draw yet",
// which is a normal state during a resize and not an error.
bool strider_copy_frame(::std::uint32_t width,
                        ::std::uint32_t height,
                        ::std::uint8_t *dst,
                        ::std::size_t len);
// One real buffer swap happened. Counted in Rust so the frame rate reported is the display's
// and not an animation timer's.
void strider_swapped();
}

// Set the default surface format's swap interval BEFORE any window exists.
//
// This has to happen before `QGuiApplication` creates anything, because Qt fixes the swap
// interval at window creation — which is why running uncapped cannot be a checkbox that takes
// effect immediately, and is a startup choice instead.
void strider_prepare_graphics(bool vsync);

#endif // STRIDER_VIEWPORT_H
