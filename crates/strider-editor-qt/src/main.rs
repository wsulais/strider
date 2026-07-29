// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! PROTOTYPE / THROWAWAY — the Qt host. See `../NOTES.md`.
//!
//! This closes the last untested corner of the renderer's contract:
//!
//! * **The renderer draws into a target the host composites** ([[RFC-0006:C-SURFACE]] 4).
//!   It renders offscreen and the host paints the result into a `QQuickPaintedItem`. This
//!   replaced a native child `QWindow` that wgpu presented to directly — [[ADR-0009]]'s
//!   chosen arrangement, which worked and cost more than it looked: a child window sits
//!   outside the scene graph, so QML could not overlay it, it could not hold keyboard focus
//!   independently of its top level, and every pointer and key event had to be forwarded by
//!   hand. C-SURFACE 4 requires the two targets to be obligation-identical, and they were —
//!   nothing beneath this crate changed to make the swap.
//! * **Frame scheduling belongs to the host** ([[RFC-0006:C-RENDER]] 4). The loop is a QML
//!   `Timer` calling `editor.tick()`. The renderer has no clock and cannot start one.
//! * **The renderer never blocks on presentation** ([[RFC-0006:C-SURFACE]] 3). Acquiring the
//!   swapchain image is a host call, and a frame that cannot be acquired is skipped.
//! * **Chrome is composited by the toolkit; depth-dependent content is not**
//!   ([[RFC-0006:C-OVERLAY]] 1 and 2). The QML panel sits beside the viewport; anchors are
//!   drawn by the renderer and depth-tested against the cloud in hardware.
//!
//! Everything below `render-gpu` is byte-identical to what the terminal shell drives, which
//! is the point: two hosts, one renderer, and the renderer cannot tell them apart.

use std::cell::RefCell;
use std::pin::Pin;

use host_sim::doc::Edit;
use host_sim::host::Host;
use render_core::{EditAction, Lod, PartitionId, HIDE, RECLASS};
use render_gpu::{AnchorPoint, Gpu, Orbit, Shading};

// Device sharing, one module per backend, chosen by the target and not by a flag.
//
// The five calls behind `share::` are the same on both platforms and the frame path below uses
// nothing else, which is what keeps `render_current` free of `cfg`. The two implementations are
// not symmetric and should not be forced to look it: Vulkan needs an image this crate allocated
// and a layout handed back and forth, Metal needs neither. Hiding that difference here is the
// whole point of the seam — an earlier arrangement had the Vulkan layout dance inline in the
// frame path, where a second backend could only have been added by making it conditional.
//
// One declaration, one name. The file chosen by the target is the only place a platform is
// mentioned on this side of the program: everything below says `share::` and nothing below asks
// which platform it is on.
// The host half, promoted alongside the app rather than into a library: threading and retrieval
// scheduling are this application's, not the document model's. `strider-doc` holds what a second
// front end would share; these two hold what it would write for itself.
mod host;
mod retrieval;

#[cfg_attr(target_vendor = "apple", path = "share_metal.rs")]
#[cfg_attr(not(target_vendor = "apple"), path = "share_vulkan.rs")]
mod share;

#[cxx_qt::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    // The only C++ left. `strider_viewport_size` and `strider_copy_frame` are declared in
    // `viewport.h` with C linkage and implemented in Rust below; this block declares the
    // functions going the other way.
    unsafe extern "C++" {
        include!("viewport.h");

        /// Register `Viewport` as a QML type. Must run before the engine loads anything.
        #[rust_name = "register_qml_types"]
        fn strider_register_qml_types();

        /// End the event loop, which is what closing the window does.
        #[rust_name = "quit_app"]
        fn strider_quit(code: i32);

        /// Set the swap interval. Must run before `QGuiApplication` creates any window.
        #[rust_name = "prepare_graphics"]
        fn strider_prepare_graphics(vsync: bool);

    }

    // Device sharing is NOT declared here. It is one function per backend with a different
    // signature each, and this bridge is compiled on every platform — so a declaration here is
    // unused on all but one, which is a warning that cannot be silenced without asserting
    // something false about who calls it. Each `share_*.rs` declares its own with C linkage
    // instead, beside its only caller.

    extern "RustQt" {
        /// The chrome's model and the frame loop's entry point.
        ///
        /// cxx-qt generates the QObject and the property bindings. That is the whole reason
        /// to use it: the previous version of this file was 260 lines of hand-written
        /// `extern "C"` and manual pointer discipline, and it did less.
        #[qobject]
        #[qml_element]
        #[qproperty(QStringList, readouts)]
        #[qproperty(i32, channel_index, cxx_name = "channelIndex")]
        #[qproperty(i32, ramp_index, cxx_name = "rampIndex")]
        /// Whether the ramp control affects anything, so the interface can grey it out rather
        /// than offer a setting with no effect.
        #[qproperty(bool, ramp_applies, cxx_name = "rampApplies")]
        #[qproperty(bool, vsync_on, cxx_name = "vsyncOn")]
        #[qproperty(bool, adaptive_on, cxx_name = "adaptiveOn")]
        /// Whether anything changed since the last paint.
        ///
        /// QML consults this instead of repainting unconditionally. A still camera over a
        /// fully resident view then costs nothing at all — which matters here more than in
        /// most viewers, because a paint is a readback.
        #[qproperty(bool, needs_paint, cxx_name = "needsPaint")]
        type Editor = super::EditorRust;

        /// One frame. Called by a QML `Timer`, because the frame loop is the host's.
        #[qinvokable]
        fn tick(self: Pin<&mut Editor>);

        /// A key, as a Qt key code, forwarded from QML.
        ///
        /// QML can do this now. Under the previous arrangement it could not: the viewport was
        /// a native child window, keyboard focus belongs to the top level, and a child window
        /// cannot take it independently — so keys went to the QML window and the handler on
        /// the child never fired. That is the bug this swap fixes rather than works around.
        ///
        /// `cxx_name` is load-bearing throughout: cxx-qt exposes the Rust name verbatim, so
        /// QML calling the camelCase name it expects fails as a silent `TypeError` inside a
        /// signal handler.
        #[qinvokable]
        #[cxx_name = "keyPressed"]
        fn key_pressed(self: Pin<&mut Editor>, key: i32);

        #[qinvokable]
        fn pan(self: Pin<&mut Editor>, dx: f32, dy: f32);

        #[qinvokable]
        fn zoom(self: Pin<&mut Editor>, factor: f32);

        /// A lasso, in fractions of the viewport — what a rubber-band drag gives.
        #[qinvokable]
        fn lasso(self: Pin<&mut Editor>, u0: f32, v0: f32, u1: f32, v1: f32, remove: bool);

        /// Release everything and leave.
        ///
        /// Called from QML's `onClosing`, which is the route a user takes. The platform-surface
        /// hook in `viewport.cpp` is the safety net for the compositor destroying the window
        /// out from under us; this is the ordinary path, and it also has to destroy the native
        /// child window, because that window keeps the application alive on its own.
        #[qinvokable]
        fn shutdown(self: Pin<&mut Editor>);

        /// What drives colour.
        #[qinvokable]
        #[cxx_name = "setChannel"]
        fn set_channel(self: Pin<&mut Editor>, index: i32);

        /// Which ramp draws it.
        #[qinvokable]
        #[cxx_name = "setRamp"]
        fn set_ramp(self: Pin<&mut Editor>, index: i32);

        /// Pace frames to the display, or run unthrottled.
        #[qinvokable]
        #[cxx_name = "setVsync"]
        fn set_vsync(self: Pin<&mut Editor>, on: bool);

        /// Repaint only on change, or every frame.
        #[qinvokable]
        #[cxx_name = "setAdaptive"]
        fn set_adaptive(self: Pin<&mut Editor>, on: bool);

        /// The names for each control, built by the host so a new ramp or channel appears in
        /// the interface without the renderer or this bridge changing.
        #[qinvokable]
        #[cxx_name = "channelNames"]
        fn channel_names(self: &Editor) -> QStringList;

        #[qinvokable]
        #[cxx_name = "rampNames"]
        fn ramp_names(self: &Editor) -> QStringList;
    }

    impl cxx_qt::Constructor<()> for Editor {}
}

#[derive(Default)]
pub struct EditorRust {
    readouts: cxx_qt_lib::QStringList,
    channel_index: i32,
    ramp_index: i32,
    ramp_applies: bool,
    vsync_on: bool,
    adaptive_on: bool,
    needs_paint: bool,
}

impl cxx_qt::Initialize for ffi::Editor {
    fn initialize(mut self: Pin<&mut Self>) {
        // Publish the initial state, which is NOT optional: a `bool` qproperty defaults to
        // false, so without this the vsync switch read off while `State` said on — and the
        // QML driver is bound to the property, so the display-paced loop never started and
        // the unthrottled one ran in its place. A default that disagrees with itself across
        // the bridge is worse than either value.
        let (channel, ramp, applies, vsync, adaptive) = STATE.with(|s| {
            let s = s.borrow();
            (
                s.channel.index(),
                s.ramp as i32,
                s.channel.uses_ramp(),
                s.vsync,
                s.adaptive,
            )
        });
        self.as_mut().set_channel_index(channel);
        self.as_mut().set_ramp_index(ramp);
        self.as_mut().set_ramp_applies(applies);
        self.as_mut().set_vsync_on(vsync);
        self.as_mut().set_adaptive_on(adaptive);
    }
}

/// Host state. Thread-local because Qt's main thread is the only thread that touches it —
/// not a shortcut, but the arrangement [[RFC-0004:C-HOST]] 3 describes: correct when all
/// work runs on one thread, differing only in throughput.
struct State {
    host: Option<Host>,
    gpu: Option<Gpu>,
    /// The two offscreen targets, and the size they were built for. See `render_current` for
    /// why there are two.
    offscreen: Option<([render_gpu::Offscreen; 2], u32, u32)>,
    /// Which of the two the next frame draws into.
    parity: usize,
    /// Target pairs a resize replaced, held for a few renders before being dropped. See the
    /// comment where they are pushed: on the shared path Qt may still be sampling them.
    retired: Vec<(u8, [render_gpu::Offscreen; 2])>,
    /// GPU buffers by `(partition, level)`, and whether they were built from a mask.
    resident: std::collections::BTreeMap<(u32, u8), (u64, bool)>,
    frames: u64,
    /// What drives colour, and which ramp draws it — **two independent choices**.
    ///
    /// These were one flat list of every ramp crossed with every channel, which is how the
    /// renderer's `Shading` is shaped but not how anyone thinks: changing the ramp should not
    /// require finding the row that happens to pair it with the channel you already had. The
    /// crossing is now done at the point of use, where it belongs.
    channel: ChannelChoice,
    ramp: usize,
    /// Frame pacing and repaint policy, both switchable at run time so their effect can
    /// actually be observed rather than argued about.
    vsync: bool,
    adaptive: bool,
    /// Rates, and whether there is anything new to draw.
    rates: Rates,
    /// The rows last published to QML, so quitting can print them for a harness to read.
    last_readouts: Vec<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            last_readouts: Vec::new(),
            host: None,
            gpu: None,
            offscreen: None,
            parity: 0,
            retired: Vec::new(),
            resident: std::collections::BTreeMap::new(),
            frames: 0,
            channel: ChannelChoice::default(),
            ramp: 0,
            // Both on by default, because both are what you want in normal use; the point of
            // the switches is to be able to turn them off and see the difference.
            vsync: true,
            adaptive: true,
            rates: Rates::default(),
        }
    }
}

/// What drives point colour, independent of which ramp draws it.
///
/// Two of these do not use a ramp at all, which is the reason this is an enum rather than an
/// index: the source's own colour is already a colour, and classification is *categorical*, so
/// a sequential ramp over category numbers would imply an order that does not exist.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum ChannelChoice {
    #[default]
    SourceRgb,
    Classification,
    /// An index into the host's channels — see `host_sim::doc::CHANNEL_LABELS`.
    Channel(u32),
}

impl ChannelChoice {
    fn from_index(i: i32) -> Self {
        match i {
            0 => Self::SourceRgb,
            1 => Self::Classification,
            n if n >= 2 => Self::Channel((n - 2) as u32),
            _ => Self::SourceRgb,
        }
    }

    fn index(self) -> i32 {
        match self {
            Self::SourceRgb => 0,
            Self::Classification => 1,
            Self::Channel(c) => 2 + c as i32,
        }
    }

    /// Whether a ramp choice affects anything. The interface greys the ramp control out when
    /// it does not, rather than letting someone change a setting with no effect.
    fn uses_ramp(self) -> bool {
        matches!(self, Self::Channel(_))
    }

    fn labels() -> Vec<String> {
        let mut v = vec!["source RGB".to_string(), "classification".to_string()];
        v.extend(host_sim::doc::CHANNEL_LABELS.iter().map(|s| s.to_string()));
        v
    }
}

/// Tick and paint rates, measured separately.
///
/// Separately because with damage tracking they diverge, and the divergence is the thing worth
/// seeing: "60 ticks, 2 paints" says the camera is still and the renderer is idle, which is
/// the whole point of only drawing on change. One number would hide it.
struct Rates {
    ticks: u32,
    paints: u32,
    window_start: std::time::Instant,
    /// Last completed second's counts, so the readout does not flicker mid-window.
    last: (u32, u32),
    /// Wall time of the last paint, for an instantaneous frame time.
    last_paint: Option<std::time::Instant>,
    frame_ms: f32,
    /// Set when something happened that changes what a frame would look like.
    dirty: bool,
    /// Paints that produced nothing, and why. A blank viewport is otherwise indistinguishable
    /// from a viewport that is correctly idle.
    null_paints: u32,
    null_reason: &'static str,
    /// Real buffer swaps, which is the only honest measure of the display's rate.
    swaps: u32,
    last_swaps: u32,
    /// Where a frame's time actually goes, in microseconds, smoothed the same way `frame_ms` is.
    ///
    /// Split because "80 fps" names no cause. The three phases have entirely different
    /// remedies: host extract is octree work on the CPU, draw is submission, and readback is a
    /// synchronous stall on the GPU plus a memcpy the size of the viewport — and only the last
    /// of those is a consequence of rendering offscreen rather than presenting.
    extract_us: f32,
    draw_us: f32,
    readback_us: f32,
    /// Bytes copied out of the GPU on the last readback.
    readback_bytes: usize,
    /// When the heartbeat line was last printed.
    ///
    /// The counters below used to reach a log only because a scripted `quit` step dumped them on
    /// the way out — which meant the application carried a harness hook in order to be
    /// observable. A periodic line is ordinary logging, it is useful to a person watching a real
    /// session, and it does not care how the session ends.
    last_log: std::time::Instant,
    /// Totals since start, which a rate cannot substitute for.
    ///
    /// "paints per s 0" was read as "the viewport is not painting" on a backend where it
    /// demonstrably was: the rate is the LAST COMPLETED second's count, so a run that quits
    /// mid-window reports zero for work it actually did. A harness has to be able to ask how
    /// many paints there have ever been.
    total_paints: u64,
    total_swaps: u64,
}

impl Default for Rates {
    fn default() -> Self {
        Self {
            ticks: 0,
            paints: 0,
            window_start: std::time::Instant::now(),
            last_log: std::time::Instant::now(),
            last: (0, 0),
            last_paint: None,
            frame_ms: 0.0,
            // Start dirty: the first frame has to be drawn.
            dirty: true,
            null_paints: 0,
            extract_us: 0.0,
            draw_us: 0.0,
            readback_us: 0.0,
            readback_bytes: 0,
            total_paints: 0,
            total_swaps: 0,
            null_reason: "",
            swaps: 0,
            last_swaps: 0,
        }
    }
}

impl Rates {
    fn tick(&mut self) {
        self.ticks += 1;
        self.roll();
    }

    fn paint(&mut self) {
        self.paints += 1;
        self.total_paints += 1;
        let now = std::time::Instant::now();
        if let Some(prev) = self.last_paint {
            // Exponentially smoothed, so a single stall does not dominate the reading.
            let ms = now.duration_since(prev).as_secs_f32() * 1000.0;
            self.frame_ms = if self.frame_ms == 0.0 {
                ms
            } else {
                self.frame_ms * 0.9 + ms * 0.1
            };
        }
        self.last_paint = Some(now);
        self.roll();
    }

    /// Same exponential smoothing `frame_ms` uses, so the phases and the total are comparable.
    fn blend(slot: &mut f32, us: f32) {
        *slot = if *slot == 0.0 { us } else { *slot * 0.9 + us * 0.1 };
    }

    fn roll(&mut self) {
        if self.window_start.elapsed() >= std::time::Duration::from_secs(1) {
            self.last = (self.ticks, self.paints);
            self.last_swaps = self.swaps;
            self.ticks = 0;
            self.paints = 0;
            self.swaps = 0;
            self.window_start = std::time::Instant::now();
        }
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Run a `#[qinvokable]` body so a panic is *reported* rather than swallowed.
///
/// cxx-qt generates invokables as `noexcept`, so a Rust panic crossing that boundary cannot
/// unwind: the process aborts with "panic in a destructor during cleanup" and the actual
/// message is lost. That cost a debugging round and would cost one every time, so every
/// invokable goes through here.
fn guarded<T: Default>(what: &str, body: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(v) => v,
        Err(e) => {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "<non-string panic>".into());
            eprintln!("PANIC in {what}: {msg}");
            T::default()
        }
    }
}

impl ffi::Editor {
    fn key_pressed(mut self: Pin<&mut Self>, key: i32) {
        // Two axes, two key groups. Digits were previously a flat index into every ramp
        // crossed with every channel, which meant changing the ramp moved you to a different
        // channel as well — the same conflation the dropdowns now avoid.
        const KEY_0: i32 = 0x30;
        const KEY_9: i32 = 0x39;
        const RAMP_KEYS: [i32; 5] = [0x51, 0x57, 0x45, 0x52, 0x54]; // Q W E R T
        const KEY_U: i32 = 0x55;

        if (KEY_0..=KEY_9).contains(&key) {
            let want = key - KEY_0;
            self.as_mut().set_channel(want);
            eprintln!(
                "key         {} -> channel {want}",
                (key as u8) as char
            );
            return;
        }
        if let Some(i) = RAMP_KEYS.iter().position(|k| *k == key) {
            self.as_mut().set_ramp(i as i32);
            eprintln!("key         {} -> ramp {i}", (key as u8) as char);
            return;
        }
        if key == KEY_U {
            let undone = guarded("undo", || {
                STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    s.rates.dirty = true;
                    s.host.as_mut().map(|h| h.undo()).unwrap_or(false)
                })
            });
            eprintln!("key         u -> undo (an edit was removed: {undone})");
        }
    }

    fn shutdown(self: Pin<&mut Self>) {
        guarded("shutdown", || {
            // The last thing the session knows, printed where the session ends. This used to
            // happen in a scripted `quit` step, which meant the numbers were only obtainable by
            // asking the application to drive itself; closing the window is the real path and it
            // is the one a person takes.
            STATE.with(|s| {
                for row in &s.borrow().last_readouts {
                    eprintln!("readout     {}", row.replace('\t', "  "));
                }
            });
            // Still explicit, and still in this order, but there is no longer a surface whose
            // lifetime is entangled with a window's. That entanglement is what segfaulted on
            // Wayland; an offscreen target has no such relationship, so the class of bug is
            // gone rather than handled.
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.resident.clear();
                if SHARING.load(std::sync::atomic::Ordering::Relaxed) {
                    // LEAKED, deliberately, and this is an ordering obligation rather than
                    // sloppiness.
                    //
                    // Qt's scene graph holds wrappers around the textures this process owns, and
                    // its render thread may be drawing with them at this instant — `quit_app`
                    // has not been called yet and will not stop it synchronously. Destroying them
                    // here is a use-after-free inside the driver, and the crash report names it
                    // exactly that: `AGXG14XFamilyRenderContext setFragmentTextures:` on
                    // QSGRenderThread, reached from `QRhiMetal::enqueueShaderResourceBindings`.
                    //
                    // Nothing this side can wait for. So nothing is destroyed: the process is
                    // about to end and the kernel reclaims the device either way. The alternative
                    // is the handshake [[RFC-0006:C-SURFACE]] does not describe — destroy the
                    // window, let Qt release its scene graph, and only then drop the device —
                    // which needs an owner of both, and in this prototype nothing is.
                    std::mem::forget(s.offscreen.take());
                    std::mem::forget(s.gpu.take());
                } else {
                    s.offscreen = None;
                    s.gpu = None;
                }
            });
            ffi::quit_app(0);
        })
    }

    fn set_channel(mut self: Pin<&mut Self>, index: i32) {
        let (i, applies) = guarded("setChannel", || {
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.channel = ChannelChoice::from_index(index);
                s.rates.dirty = true;
                (s.channel.index(), s.channel.uses_ramp())
            })
        });
        self.as_mut().set_channel_index(i);
        self.as_mut().set_ramp_applies(applies);
    }

    fn set_ramp(mut self: Pin<&mut Self>, index: i32) {
        let i = guarded("setRamp", || {
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                let n = s.gpu.as_ref().map(|g| g.ramp_names().len()).unwrap_or(1);
                s.ramp = (index.max(0) as usize).min(n.saturating_sub(1));
                s.rates.dirty = true;
                s.ramp as i32
            })
        });
        self.as_mut().set_ramp_index(i);
    }

    fn set_vsync(mut self: Pin<&mut Self>, on: bool) {
        guarded("setVsync", || {
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.vsync = on;
                s.rates.dirty = true;
            })
        });
        self.as_mut().set_vsync_on(on);
        // Honest about what a runtime toggle can and cannot do. Qt fixes the swap interval when
        // the window is created, so unchecking this cannot make swaps exceed the refresh rate
        // in an already-running process — that needs STRIDER_NO_VSYNC=1 at startup. What it
        // does do is stop the continuous repaint, which is the other half of what was being
        // conflated here.
        eprintln!(
            "pacing      continuous repaint {} (swap interval was fixed at startup: {})",
            if on { "on" } else { "off" },
            if std::env::var("STRIDER_NO_VSYNC").is_ok() {
                "uncapped"
            } else {
                "vsync"
            }
        );
    }

    fn set_adaptive(mut self: Pin<&mut Self>, on: bool) {
        guarded("setAdaptive", || {
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.adaptive = on;
                // Turning it off has to force a paint, or nothing changes until the next
                // change — which is exactly the state the switch exists to leave.
                s.rates.dirty = true;
            })
        });
        self.as_mut().set_adaptive_on(on);
        eprintln!(
            "rendering   {}",
            if on {
                "adaptive — repaint only when something changed"
            } else {
                "immediate — repaint every frame"
            }
        );
    }

    fn channel_names(&self) -> cxx_qt_lib::QStringList {
        let mut list = cxx_qt_lib::QStringList::default();
        for n in ChannelChoice::labels() {
            list.append(cxx_qt_lib::QString::from(&n));
        }
        list
    }

    fn ramp_names(&self) -> cxx_qt_lib::QStringList {
        let mut list = cxx_qt_lib::QStringList::default();
        STATE.with(|s| {
            if let Some(g) = s.borrow().gpu.as_ref() {
                for n in g.ramp_names() {
                    list.append(cxx_qt_lib::QString::from(n));
                }
            }
        });
        list
    }

    fn pan(self: Pin<&mut Self>, dx: f32, dy: f32) {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.rates.dirty = true;
            if let Some(h) = s.host.as_mut() {
                h.cam.centre[0] += dx * h.cam.width;
                h.cam.centre[1] += dy * h.cam.width;
            }
        });
    }

    fn zoom(self: Pin<&mut Self>, factor: f32) {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.rates.dirty = true;
            if let Some(h) = s.host.as_mut() {
                h.zoom(factor);
            }
        });
    }

    fn lasso(self: Pin<&mut Self>, u0: f32, v0: f32, u1: f32, v1: f32, remove: bool) {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.rates.dirty = true;
            let Some(h) = s.host.as_mut() else { return };
            let (min, max) = h.view_box();
            let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t.clamp(0.0, 1.0);
            // Rows run north-to-south on screen, so a rectangle's top edge is the northern
            // one — the flip belongs to the host, which is what owns the camera.
            h.push_edit(Edit {
                min: [
                    lerp(min[0], max[0], u0.min(u1)),
                    lerp(min[1], max[1], 1.0 - v0.max(v1)),
                ],
                max: [
                    lerp(min[0], max[0], u0.max(u1)),
                    lerp(min[1], max[1], 1.0 - v0.min(v1)),
                ],
                only_class: None,
                action: if remove {
                    EditAction::Delete
                } else {
                    EditAction::Classify { class: 5 }
                },
            });
        });
    }

    /// One frame, driven by Qt.
    ///
    /// The order is the same one the terminal shell uses, and for the same reasons:
    /// deliveries, then extract, then draw, then perform effects. The swapchain acquire lives
    /// here, in the host, because it is the step that can block.
    fn tick(mut self: Pin<&mut Self>) {
        // The borrow is released before the property is written: writing it emits a
        // change signal that QML handles synchronously, and a handler calling back into an
        // invokable would hit a `RefCell` double borrow.
        let t_extract = std::time::Instant::now();
        let rows = guarded("tick", || Some(run_frame()));
        let extract_us = t_extract.elapsed().as_secs_f32() * 1e6;
        STATE.with(|s| Rates::blend(&mut s.borrow_mut().rates.extract_us, extract_us));
        let dirty = STATE.with(|s| {
            let mut s = s.borrow_mut();
            // Swaps are counted on whichever thread Qt swapped on; folded in here, on the
            // host's thread, so `Rates` stays single-threaded like everything else.
            let seen = SWAPS.swap(0, std::sync::atomic::Ordering::Relaxed);
            s.rates.swaps += seen as u32;
            s.rates.total_swaps += seen;
            s.rates.tick();
            // A resize is damage.
            //
            // Without this, adaptive mode never re-renders after a resize: nothing marks the
            // frame dirty, so `render_current` is skipped, so the published frame keeps the old
            // size, so `strider_copy_frame` keeps refusing it and the viewport stays grey for
            // ever. The size check used to live inside the render, which is exactly the place
            // that does not run.
            let want = (
                WANT_W.load(std::sync::atomic::Ordering::Relaxed),
                WANT_H.load(std::sync::atomic::Ordering::Relaxed),
            );
            if s.offscreen.as_ref().map(|(_, w, h)| (*w, *h)) != Some(want) {
                s.rates.dirty = true;
            }
            // In immediate mode every frame repaints, which is the baseline the adaptive mode
            // is measured against.
            !s.adaptive || s.rates.dirty
        });
        // Drawn here, on the host's thread, and published for whichever thread paints. Gated on
        // damage so immediate mode is the only case that redraws unconditionally — otherwise a
        // still camera would burn a readback per tick to produce an identical image.
        // A render that produced a frame is itself a reason to paint, and it is the ONLY
        // reason that matters at start-up.
        //
        // The trace settled this. Four `updatePaintNode` calls run before the host has rendered
        // anything, each asking for another paint from inside the scene graph's sync phase —
        // where Qt drops the request. Then the host renders, and by that point nothing is left
        // to ask. `needs_paint` was tied to `dirty`, which the render clears, so the one moment
        // a repaint was actually warranted was the one moment nothing requested it.
        if std::env::var("STRIDER_TRACE").is_ok() {
            // Outside the `dirty` guard. The interesting period is the one with NO renders, so
            // a trace that only prints when a render happens cannot see it.
            let total = STATE.with(|s| s.borrow().rates.total_swaps);
            eprintln!("trace       tick, swaps total: {total}");
        }
        let mut published = false;
        if dirty {
            let ok = render_current();
            published = ok;
            // Paired with the item's trace, so a cold start can be read as a sequence: whether
            // the host produced a frame at all, and whether the item was ever asked to show it.
            if std::env::var("STRIDER_TRACE").is_ok() {
                // TOTAL, not the pending counter. `tick` drains `SWAPS` with `swap(0)` before
                // this line runs, so the pending value is zero by construction — it read zero
                // even while the viewport was painting correctly, which is an instrument
                // reporting on itself rather than on Qt.
                let total = STATE.with(|s| s.borrow().rates.total_swaps);
                eprintln!("trace       swaps total: {total}");
                STATE.with(|s| {
                    let s = s.borrow();
                    if s.rates.total_paints < 4 {
                        eprintln!(
                            "trace       render_current -> {ok}, renders {}, reason {:?}",
                            s.rates.total_paints, s.rates.null_reason
                        );
                    }
                });
            }
        }
        self.as_mut().set_needs_paint(dirty || published);
        if let Some(rows) = rows {
            let mut list = cxx_qt_lib::QStringList::default();
            for r in &rows {
                list.append(cxx_qt_lib::QString::from(r));
            }
            self.as_mut().set_readouts(list);
            // Kept so that quitting can print them. The harness has mis-measured this session
            // six times by asserting on cropped pixels — on antialiased chrome text, on
            // weston's desktop, on the panel twice — and every one of those failures came from
            // asking the screen instead of asking the application. These counters are what the
            // application knows, and a backend that renders nothing cannot fake them.
            STATE.with(|s| s.borrow_mut().last_readouts = rows);
        }
    }
}

/// The finished pixels, and the only thing two threads share.
///
/// The host stays thread-local, which is [[RFC-0004:C-HOST]] 3's requirement rather than a
/// fallback. What crosses threads is one image: produced on the host's thread, consumed on
/// whichever thread Qt paints on. That distinction is the whole design — an earlier attempt at
/// the threaded render loop reached the *host* from the render thread, found a
/// default-constructed one because `STATE` is a `thread_local`, and painted grey in silence.
struct Published {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

static PUBLISHED: std::sync::Mutex<Option<Published>> = std::sync::Mutex::new(None);

/// The size the item last asked for, so the host knows what to render without asking Qt.
static WANT_W: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static WANT_H: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Swaps, counted where any thread may count them.
///
/// `frameSwapped` is emitted on the scene graph's render thread under the threaded loop, so
/// this cannot live in `Rates` — the thread-local one it reached would not be the one the
/// readouts are built from, and the count would silently read zero.
static SWAPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether Qt and the renderer are on one device, and so whether the colour attachment should
/// be one Qt can sample. Set once, before any window exists.
static SHARING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The colour attachment's `VkImage`, published for `updatePaintNode`. Zero means "read back".
static SHARED_IMAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The image Qt should sample, or 0 when there is none and the pixels must be copied.
#[unsafe(no_mangle)]
pub extern "C" fn strider_shared_image() -> u64 {
    SHARED_IMAGE.load(std::sync::atomic::Ordering::Relaxed)
}

fn publish_frame(width: u32, height: u32, pixels: Vec<u8>) {
    if let Ok(mut slot) = PUBLISHED.lock() {
        *slot = Some(Published {
            width,
            height,
            pixels,
        });
    }
}

/// The item's current size, published from `paint` so the host can render at it.
#[unsafe(no_mangle)]
pub extern "C" fn strider_viewport_size(width: u32, height: u32) {
    use std::sync::atomic::Ordering;
    WANT_W.store(width, Ordering::Relaxed);
    WANT_H.store(height, Ordering::Relaxed);
}

/// Copy the latest frame out, if there is one at exactly this size.
///
/// A copy rather than a borrowed pointer. Handing C++ a pointer into a `Vec` the host may
/// reallocate is sound only while both live on one thread, which is precisely the assumption
/// being removed here. The copy is a memcpy of the viewport — the same one Qt was already
/// doing to upload the image, so it costs nothing new.
///
/// # Safety
/// `dst` must point to `len` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn strider_copy_frame(width: u32, height: u32, dst: *mut u8, len: usize) -> bool {
    if dst.is_null() {
        return false;
    }
    let Ok(slot) = PUBLISHED.lock() else {
        return false;
    };
    let Some(frame) = slot.as_ref() else {
        return false;
    };
    if frame.width != width || frame.height != height || frame.pixels.len() != len {
        return false;
    }
    // SAFETY: the caller guarantees `dst` is writable for `len`, and `len` was just checked
    // against the source length.
    unsafe { core::ptr::copy_nonoverlapping(frame.pixels.as_ptr(), dst, len) };
    true
}

/// Render the current frame at the size the item last reported.
///
/// Runs on the host's own thread, from `tick`. The split from painting is what makes the
/// threaded render loop safe: nothing here is reachable from Qt's render thread.
fn render_current() -> bool {
    use std::sync::atomic::Ordering;
    let (width, height) = (WANT_W.load(Ordering::Relaxed), WANT_H.load(Ordering::Relaxed));
    if width == 0 || height == 0 {
        return false;
    }
    guarded("render", || {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            let s = &mut *s;
            let no = |s: &mut State, why: &'static str| {
                s.rates.null_paints += 1;
                s.rates.null_reason = why;
                false
            };
            if s.gpu.is_none() {
                return no(s, "no device");
            }
            if s.host.is_none() {
                return no(s, "source not open");
            }
            if s.host.as_ref().and_then(|h| h.last.as_ref()).is_none() {
                return no(s, "no frame yet");
            }
            let (Some(gpu), Some(host)) = (s.gpu.as_mut(), s.host.as_ref()) else {
                return false;
            };
            let Some(frame) = host.last.as_ref() else {
                return false;
            };

            // Rebuild the target when the item resizes. The host owns the size because the
            // host owns the layout — the renderer is told.
            let stale = s
                .offscreen
                .as_ref()
                .map(|(_, w, h)| (*w, *h) != (width, height))
                .unwrap_or(true);
            if stale {
                // Two targets, not one, and that is the correctness fix rather than a
                // throughput one.
                //
                // With a single image the host renders into the same memory Qt is sampling. It
                // also has to flip that image's layout back to `COLOR_ATTACHMENT_OPTIMAL` before
                // each draw, while Qt samples on its own thread whenever the scene graph reaches
                // it — so the layout changes underneath the sampler and
                // `VUID-vkCmdDraw-None-09600` fires. A barrier cannot fix that; it is a race,
                // not a missing transition, and adding one only changed which frames were wrong.
                //
                // Alternating removes the shared mutable state instead of guarding it. Nothing
                // needs to know when Qt has finished, which matters because there is no way to
                // ask: `wgpu_hal::vulkan::Queue` offers `add_signal_semaphore` and no wait
                // counterpart.
                //
                // The plain path alternates too. One code path, exercised everywhere, rather
                // than a sharing-only arrangement nothing else tests.
                let shared = SHARING.load(std::sync::atomic::Ordering::Relaxed);
                let make = || {
                    shared
                        .then(|| share::shared_target(gpu, width, height))
                        .flatten()
                        .unwrap_or_else(|| gpu.offscreen(width, height))
                };
                let targets = [make(), make()];
                // The pair being replaced is RETIRED rather than dropped.
                //
                // Same fault as the one in `shutdown`, arriving by the other route: on the shared
                // path Qt holds a wrapper around the outgoing textures, and a resize is the one
                // thing that replaces a live target. Dropping them here destroys textures Qt may
                // sample on its next pass, and it is the crash that presented as "closing
                // segfaults" on Vulkan until a start-up nudge made every run reproduce it.
                //
                // A countdown rather than a handshake, because there is nothing to ask: the new
                // handle is published by the render below, and by the time three more renders
                // have gone by Qt has certainly stopped sampling the old one. Off the shared path
                // this is pointless but harmless, and one code path is worth more than a
                // conditional nobody exercises.
                if let Some((old, _, _)) = s.offscreen.take() {
                    s.retired.push((3, old));
                }
                // NOT published here.
                //
                // An earlier version did, to break a chicken-and-egg where `shared` was derived
                // from `SHARED_IMAGE != 0` and so could never become true. That is fixed
                // properly now — `shared` asks the target whether it has an image — and
                // publishing at creation was its own bug: Qt would wrap an image nothing had
                // drawn into, whose layout is `UNDEFINED`, and validation said exactly that:
                //
                //   expects VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL
                //   -- instead, current layout is VK_IMAGE_LAYOUT_UNDEFINED
                //
                // The handle is published after the render that gives the image content.
                SHARED_IMAGE.store(0, std::sync::atomic::Ordering::Relaxed);
                s.offscreen = Some((targets, width, height));
                s.parity = 0;
                // A resize is damage: the previous image is the wrong size, so whatever Qt has
                // cached for this item is stale.
                s.rates.dirty = true;
            }
            let parity = s.parity;
            let Some((targets, w, h)) = s.offscreen.as_ref() else {
                return no(s, "no target");
            };
            let target = &targets[parity];

            let (_label, shading) = resolve(host, s.channel, s.ramp);

            let gpu_draws: Vec<_> = frame
                .draws
                .iter()
                .filter_map(|d| s.resident.get(&(d.id.0, d.lod.0)).map(|(t, _)| (*d, *t)))
                .collect();
            let (vmin, vmax) = host.view_box();
            let cam = Orbit::framing(vmin, vmax, host.ramp.channels[0]);
            let anchors: Vec<AnchorPoint> = host
                .doc
                .anchors
                .iter()
                .map(|a| AnchorPoint {
                    world: [a.x, a.y, a.z],
                })
                .collect();

            // Back to the layout wgpu believes it is in, before wgpu touches it. Paired with
            // the transition after the draw; a no-op on the unshared path.
            share::reclaim_for_draw(gpu, target);

            let (colour, depth) = target.views();
            let t_draw = std::time::Instant::now();
            gpu.draw(
                &colour,
                &depth,
                (*w, *h),
                &gpu_draws,
                &anchors,
                &cam,
                host.ramp.channels[0],
                3.0,
                shading,
            );
            let draw_us = t_draw.elapsed().as_secs_f32() * 1e6;
            // Separately, because `read_rgba` is where the GPU is actually waited on: `draw`
            // only submits. Attributing the wait to the draw would make the readback look free.
            let t_read = std::time::Instant::now();
            // Asked of the target, not of the published handle. The handle says what Qt is
            // sampling right now; this asks what we are drawing into.
            // Asked only when sharing was accepted. On Vulkan the target itself would answer
            // `None`, but on Metal *every* target has a handle — so "is there a handle" is not
            // the same question as "should Qt sample it", and conflating them would put the
            // renderer on the shared path on a machine where Qt never agreed to it.
            let handle = if SHARING.load(std::sync::atomic::Ordering::Relaxed) {
                share::sampled_handle(gpu, target)
            } else {
                0
            };
            let pixels = if handle != 0 {
                // Nothing to copy: Qt samples this image. What is still needed is for the draw
                // to have *finished* — Qt is given a handle, not a promise, and there is no
                // `add_wait_semaphore` on wgpu's queue to make it wait properly. Blocking here
                // is the honest stand-in and it still removes both 2,968 kB transfers.
                gpu.wait_idle();
                // Handed over in whatever state the toolkit expects to find it in. On Vulkan
                // that is a layout transition, and it is safe to leave the image there: the next
                // frame draws into the *other* image of the pair, so nothing takes the layout
                // back while Qt is still sampling. On Metal there are no layouts and this is
                // empty.
                share::release_to_toolkit(gpu, target);
                SHARED_IMAGE.store(handle, std::sync::atomic::Ordering::Relaxed);
                Vec::new()
            } else {
                gpu.read_rgba(target)
            };
            let read_us = t_read.elapsed().as_secs_f32() * 1e6;
            Rates::blend(&mut s.rates.draw_us, draw_us);
            Rates::blend(&mut s.rates.readback_us, read_us);
            s.rates.readback_bytes = pixels.len();

            // Published under the lock, held only for the swap of a `Vec` — not for the draw
            // and not for the readback, both of which happened above on this thread.
            publish_frame(width, height, pixels);
            s.rates.paint();
            s.rates.dirty = false;
            // Next frame draws into the other image. Qt keeps sampling this one until a newer
            // one is published, which is the whole of the handshake we are allowed to have.
            s.parity ^= 1;
            // And a retired pair comes one render closer to being dropped.
            s.retired.retain_mut(|(left, _)| {
                *left = left.saturating_sub(1);
                *left > 0
            });
            true
        })
    })
}

/// One buffer swap happened.
///
/// Emitted on the scene graph's render thread under the threaded loop, so this touches an
/// atomic and nothing else. The previous version reached `STATE`, which is a `thread_local`:
/// on the render thread that is a different, default-constructed `State`, so the count went
/// into an object nobody reads and the readout stayed at zero.
#[unsafe(no_mangle)]
pub extern "C" fn strider_swapped() {
    SWAPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Resolve the two independent choices into the one thing the renderer understands.
///
/// The crossing happens here and nowhere else. The renderer knows a channel index, a range and
/// a bound ramp; that the interface presents those as two dropdowns, and that channel 1 holds
/// intensity read from a file rather than something an analytical pass computed, is entirely
/// this function's business.
fn resolve(host: &Host, channel: ChannelChoice, ramp: usize) -> (String, Shading) {
    match channel {
        ChannelChoice::SourceRgb => ("source RGB".into(), Shading::SourceRgb),
        ChannelChoice::Classification => (
            "classification".into(),
            Shading::Ramped {
                // The sentinel the shader reads as "categorical, use the class palette".
                channel: u32::MAX - 1,
                range: (0.0, 1.0),
                ramp,
            },
        ),
        ChannelChoice::Channel(c) => {
            let i = (c as usize).min(render_core::CHANNELS - 1);
            (
                host_sim::doc::CHANNEL_LABELS[i].to_string(),
                Shading::Ramped {
                    channel: i as u32,
                    // Host-supplied and stable — see `RampStats`. A range observed from the
                    // resident set would make colour depend on the camera.
                    range: host.ramp.channels[i],
                    ramp,
                },
            )
        }
    }
}

fn run_frame() -> Vec<String> {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let s = &mut *s;
        let (channel, ramp) = (s.channel, s.ramp);
        let label = s
            .host
            .as_ref()
            .map(|h| resolve(h, channel, ramp).0)
            .unwrap_or_default();
        let Some(host) = s.host.as_mut() else {
            return vec!["source\tnot open".into()];
        };
        host.log.clear();
        host.tick();
        s.frames += 1;

        let draws = host
            .last
            .as_ref()
            .map(|f| f.draws.to_vec())
            .unwrap_or_default();
        let Some(gpu) = s.gpu.as_mut() else {
            return readouts(host, s.frames, 0, 0, label, s.rates.last, s.rates.frame_ms, s.rates.null_paints, s.rates.null_reason, s.rates.last_swaps, (s.rates.total_paints, s.rates.total_swaps), (s.rates.extract_us, s.rates.draw_us, s.rates.readback_us, s.rates.readback_bytes));
        };

        // The uploads and evictions the policy layer's decisions imply. Drawing happens in
        // `strider_paint`, on Qt's paint call — this only makes sure the buffers exist.
        for d in &draws {
            let key = (d.id.0, d.lod.0);
            let stale = match s.resident.get(&key) {
                None => true,
                Some((_, masked)) => {
                    d.remasked || (*masked && d.hidden == 0 && d.reclassified == 0)
                }
            };
            if !stale {
                continue;
            }
            let Some(token) = host.renderer.token_of(d.id, d.lod) else {
                continue;
            };
            let verts = render_core::Uploads::vertices(&host.store, token);
            let flags = host.renderer.mask_flags(d.id, d.lod);
            let mut masked = Vec::with_capacity(verts.len());
            for (i, v) in verts.iter().enumerate() {
                match flags.and_then(|f| f.get(i).copied()) {
                    Some(HIDE) => continue,
                    Some(x) if x >= RECLASS => masked.push(render_core::Vertex {
                        class: x - RECLASS,
                        ..*v
                    }),
                    _ => masked.push(*v),
                }
            }
            if let Some((old, _)) = s.resident.remove(&key) {
                gpu.free(old);
            }
            s.resident.insert(key, (gpu.upload(&masked), flags.is_some()));
        }
        s.resident.retain(|(id, lod), (tok, _)| {
            let live = host
                .renderer
                .token_of(PartitionId(*id), Lod(*lod))
                .is_some();
            if !live {
                gpu.free(*tok);
            }
            live
        });

        // Damage from the renderer's own side: an upload landing, an eviction, or a mask
        // rebuild all change what a frame looks like without anyone touching the camera.
        let effects = host
            .last
            .as_ref()
            .map(|f| !f.effects.is_empty() || f.draws.iter().any(|d| d.remasked))
            .unwrap_or(false);
        let inflight = host.renderer.inflight_len() > 0;
        if effects || inflight {
            s.rates.dirty = true;
        }

        let points = draws.iter().map(|d| d.points as u64).sum();
        // One line every two seconds. Deliberately one line and not the readout block: a person
        // watching a session wants a heartbeat, and everything a harness asserts on is here.
        if s.rates.last_log.elapsed() >= std::time::Duration::from_secs(2) {
            s.rates.last_log = std::time::Instant::now();
            eprintln!(
                "status      frame {} | paints/swaps {}/{} | {:.1} ms/paint | {} points | \
                 extract {:.2} draw {:.2} readback {:.2} ms | copied {} kB | target {}",
                s.frames,
                s.rates.total_paints,
                s.rates.total_swaps,
                s.rates.frame_ms,
                thousands(points),
                s.rates.extract_us / 1000.0,
                s.rates.draw_us / 1000.0,
                s.rates.readback_us / 1000.0,
                s.rates.readback_bytes / 1024,
                if SHARED_IMAGE.load(std::sync::atomic::Ordering::Relaxed) != 0 {
                    "sampled directly"
                } else {
                    "read back"
                },
            );
        }
        readouts(host, s.frames, points, s.resident.len() as u64, label, s.rates.last, s.rates.frame_ms, s.rates.null_paints, s.rates.null_reason, s.rates.last_swaps, (s.rates.total_paints, s.rates.total_swaps), (s.rates.extract_us, s.rates.draw_us, s.rates.readback_us, s.rates.readback_bytes))
    })
}

#[allow(clippy::too_many_arguments)]
fn readouts(
    host: &Host,
    frames: u64,
    points: u64,
    buffers: u64,
    shading_label: String,
    rates: (u32, u32),
    frame_ms: f32,
    nulls: u32,
    reason: &str,
    swaps: u32,
    totals: (u64, u64),
    phases: (f32, f32, f32, usize),
) -> Vec<String> {
    let st = host.renderer.stats();
    let row = |k: &str, v: String| format!("{k}\t{v}");
    vec![
        row("points in source", thousands(host.source.header().point_count)),
        row("frame", frames.to_string()),
        // Ticks and paints separately: with damage tracking they diverge, and the divergence
        // is what says the renderer is idling rather than working.
        row(
            "ticks / paints per s",
            format!("{} / {}", rates.0, rates.1),
        ),
        // The display's own rate, from real swaps rather than from an animation timer.
        row("swaps per s", swaps.to_string()),
        // Totals, so a run that quits mid-window still reports what it did. Without these the
        // rate reads zero and looks exactly like a viewport that never painted.
        row(
            "paints / swaps total",
            format!("{} / {}", totals.0, totals.1),
        ),
        // The three phases of a frame, so a frame rate has a cause attached to it.
        row(
            "extract / draw / readback",
            format!(
                "{:.2} / {:.2} / {:.2} ms",
                phases.0 / 1000.0,
                phases.1 / 1000.0,
                phases.2 / 1000.0
            ),
        ),
        row(
            "readback size",
            format!(
                "{} kB  ({:.0} MB/s at {:.0} fps)",
                phases.3 / 1024,
                phases.3 as f64 * swaps as f64 / 1_048_576.0,
                swaps
            ),
        ),
        row("blank paints", format!("{nulls} ({reason})")),
        row(
            "frame time",
            if frame_ms > 0.0 {
                format!("{frame_ms:.1} ms  ({:.0} fps)", 1000.0 / frame_ms)
            } else {
                "—".into()
            },
        ),
        // The LOD breakdown, which is the number to watch rather than "points drawn".
        //
        // Zooming in *reduces* points drawn while making the frame more expensive: descent goes
        // deeper, so the traversal visits more octree cells and selects more partitions, and
        // each partition is a separate draw call. Cost lives in the cell count and the draw
        // count, not in the point total — which is why a viewer can slow down while apparently
        // doing less.
        row(
            "levels shown",
            format!(
                "{}..{} of {}",
                host.doc.stats.shallowest, host.doc.stats.deepest, host.level_range.1
            ),
        ),
        row("per level", host.doc.stats.histogram()),
        row(
            "partitions (draw calls)",
            host.doc.stats.selected.to_string(),
        ),
        row(
            "octree cells visited",
            host.doc.stats.keys_visited.to_string(),
        ),
        row(
            "index pages this frame",
            format!(
                "{}{}",
                host.pages_this_frame,
                if host.pages_this_frame > 0 {
                    "  <- blocking read in the frame path"
                } else {
                    ""
                }
            ),
        ),
        // Not a constant, and this is its third correction. It said "QWindow surface" for as long
        // as the offscreen swap had been in place — a readout asserting the opposite of what the
        // code did, which sent one diagnosis of the Vulkan fault down entirely the wrong path
        // before the paint counters contradicted it. Then it named a `QQuickPaintedItem` that no
        // longer exists. It is derived from the published handle now, so it cannot go stale
        // again: a handle means Qt is sampling our own image, no handle means pixels are copied.
        row(
            "target",
            if SHARED_IMAGE.load(std::sync::atomic::Ordering::Relaxed) != 0 {
                "offscreen, sampled by Qt directly".into()
            } else {
                "offscreen, read back into a texture".into()
            },
        ),
        row("shading", shading_label),
        row(
            "uploads resident",
            format!("{} ({} buf)", host.renderer.resident_uploads(), buffers),
        ),
        row("points drawn", thousands(points)),
        row("in flight", host.renderer.inflight_len().to_string()),
        row("edit stack", host.doc.edits.len().to_string()),
        row(
            "masks rebuilt / reused",
            format!("{} / {}", st.remasks, st.mask_reuses),
        ),
        row("requests", st.requests_issued.to_string()),
        row("cancelled", st.cancels.to_string()),
        row("dropped stale", st.dropped_stale.to_string()),
        row("evicted", st.evictions.to_string()),
        row(
            "reads / MB",
            format!(
                "{} / {}",
                host.retrieval.dispatched,
                host.retrieval.bytes_read / 1_000_000
            ),
        ),
        row(
            "read+decode",
            format!("{:.2} s", host.retrieval.decode_us as f64 / 1e6),
        ),
        row(
            "thrown away",
            format!(
                "{:.2} s / {}",
                host.retrieval.wasted_us as f64 / 1e6,
                host.retrieval.wasted_reads
            ),
        ),
    ]
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn main() {
    let path = std::env::var("STRIDER_COPC")
        .unwrap_or_else(|_| "/tank/pointcloud/aams/block1.copc.laz".to_string());
    eprintln!("strider-qt-editor — PROTOTYPE / THROWAWAY");
    eprintln!("source      {path}");

    match Host::open(&path, 200, 130) {
        Ok(mut host) => {
            host.zoom(0.45);
            STATE.with(|s| s.borrow_mut().host = Some(host));
        }
        Err(e) => {
            eprintln!("could not open {path}: {e}");
            std::process::exit(1);
        }
    }

    // The device is acquired before the window exists, which an offscreen target allows and a
    // surface did not. It also removes the retry-every-frame dance the old path needed.
    match Gpu::new() {
        Some(mut gpu) => {
            gpu.add_default_ramps();
            eprintln!("adapter     {} ({})", gpu.adapter_name, gpu.backend);
            eprintln!("ramps       {}", gpu.ramp_names().join(", "));
            STATE.with(|s| s.borrow_mut().gpu = Some(gpu));
        }
        None => {
            eprintln!("no GPU adapter available");
            std::process::exit(1);
        }
    }

    // Before the application, because Qt fixes the swap interval at window creation.
    //
    // `STRIDER_RHI` forces Qt Quick's backend, which matters for tearing: the swap interval is
    // an OpenGL-only control, so if Qt chose Vulkan then asking for interval 1 achieves
    // nothing and the present mode decides. Being able to switch is how the two are compared.
    // Threaded, which is Qt's own default and what the display deserves.
    //
    // This was forced to `basic` for one commit, because `QQuickPaintedItem::paint` runs on the
    // scene graph's render thread under the threaded loop and the host is a `thread_local` — so
    // paint saw a default-constructed `State`, bailed at "no device", and the viewport went
    // grey. Forcing `basic` fixed that by putting everything on one thread, and cost the frame
    // rate for it: host extract is ~1.6 ms at 125 Hz, about a fifth of a core, and under `basic`
    // that serialises with Qt's own rendering instead of overlapping it.
    //
    // What made `threaded` safe was narrowing what crosses threads to one image. The host still
    // renders on its own thread, from `tick`; `paint` locks, copies and draws. Nothing reaches
    // the host from Qt's render thread any more, so there is no reason left to serialise them.
    //
    // Only where this project has measured it, which is Linux. Which render loops a platform and
    // an RHI backend actually support is Qt's judgement and it differs by both — on macOS Qt
    // has its own answer for the Metal backend, and overriding it from here would be asserting
    // something about a combination nothing in this prototype has tested. `STRIDER_RENDER_LOOP`
    // still forces either one, which is how the comparison gets made rather than assumed.
    match (std::env::var("STRIDER_RENDER_LOOP"), share::RENDER_LOOP) {
        (Ok(mode), _) => {
            // SAFETY: single-threaded, before any Qt object exists.
            unsafe { std::env::set_var("QSG_RENDER_LOOP", &mode) };
            eprintln!("loop        QSG_RENDER_LOOP={mode}  (forced)");
        }
        (Err(_), Some(mode)) => {
            // SAFETY: single-threaded, before any Qt object exists.
            unsafe { std::env::set_var("QSG_RENDER_LOOP", mode) };
            eprintln!("loop        QSG_RENDER_LOOP={mode}");
        }
        (Err(_), None) => {
            eprintln!("loop        Qt's own default for this platform and backend");
        }
    }

    if let Ok(backend) = std::env::var("STRIDER_RHI") {
        // SAFETY: single-threaded, before any Qt object exists.
        unsafe { std::env::set_var("QSG_RHI_BACKEND", &backend) };
        eprintln!("rhi         forced to {backend} via STRIDER_RHI");
    } else if std::env::var("STRIDER_SHARE_DEVICE").is_ok() {
        // A shared device is only shared if Qt is on the same API. Left to itself Qt may choose
        // something else — OpenGL, where both are available — and the device handed to it later
        // would belong to an API it is not using. Decided here, from the environment alone,
        // because it must precede `QGuiApplication` while the sharing call itself must follow it.
        //
        // `None` where the platform's default is already right, which is the Metal case: naming a
        // backend that platform does not have would cause the exact fault this exists to prevent.
        if let Some(backend) = share::QT_RHI_FOR_SHARING {
            // SAFETY: single-threaded, before any Qt object exists.
            unsafe { std::env::set_var("QSG_RHI_BACKEND", backend) };
            eprintln!("rhi         {backend}, because STRIDER_SHARE_DEVICE asks for a shared device");
        }
    }
    let vsync = std::env::var("STRIDER_NO_VSYNC").is_err();
    // Offered before any window exists, because `QQuickGraphicsDevice` has to be in place before
    // the first one is shown. Declining is normal and not an error: a GL device, or a Qt without
    // Vulkan, keeps the readback path, which is the same path every non-Vulkan machine uses.
    ffi::prepare_graphics(vsync);
    eprintln!(
        "swap        {}",
        if vsync {
            "interval 1 (vsync). Set STRIDER_NO_VSYNC=1 to run uncapped."
        } else {
            "interval 0 (uncapped)"
        }
    );

    let mut app = cxx_qt_lib::QGuiApplication::new();

    // After the application, before the window.
    //
    // Both halves are load-bearing and they pull in opposite directions.
    // `QVulkanInstance::create()` reaches the platform integration, which only exists once
    // `QGuiApplication` does — calling it earlier segfaults two frames inside QtGui with no
    // message, which is exactly what the first attempt did. And `QQuickGraphicsDevice` has to be
    // in place before the first `QQuickWindow` is shown, so it cannot wait until the engine has
    // loaded. Between those two lines is the only place that satisfies both.
    //
    // Opt-in via `STRIDER_SHARE_DEVICE`. Declining is normal: a GL device, or a Qt without
    // Vulkan, keeps the readback path, which is what every non-Vulkan machine uses anyway.
    let want_sharing = std::env::var("STRIDER_SHARE_DEVICE").is_ok();
    let sharing = want_sharing
        && STATE.with(|s| {
            s.borrow()
                .gpu
                .as_ref()
                .map(|g| share::offer_device(g))
                .unwrap_or(false)
        });
    SHARING.store(sharing, std::sync::atomic::Ordering::Relaxed);
    if want_sharing {
        eprintln!(
            "sharing     {}",
            if sharing {
                "one device for Qt and the renderer"
            } else {
                "none; pixels are read back and copied"
            }
        );
    }

    ffi::register_qml_types();
    let mut engine = cxx_qt_lib::QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&cxx_qt_lib::QUrl::from("qrc:/qt/qml/com/strider/editor/qml/Main.qml"));
    }
    if let Some(app) = app.as_mut() {
        app.exec();
    }

    // Leave without unwinding the graphics stack.
    //
    // With sharing on, Qt's scene graph holds pipelines, a swapchain and command pools created
    // on the device this process also owns through wgpu, and nothing arbitrates the order they
    // are torn down in. Validation says exactly that, twice:
    //
    //   VUID-vkDestroyDevice-device-05137     child objects must be destroyed before the device
    //   VUID-vkDestroyInstance-instance-00629 ... and before the instance
    //
    // followed by a segfault on the scene graph thread. It is a shutdown-ordering fault and not
    // a runtime one: the same session runs a full script, paints every swap, and only dies on
    // the way out. Isolated by the fact that `STRIDER_SHARE_DEVICE=0` exits cleanly on both
    // backends and both render loops, while sharing crashes on both loops.
    //
    // `exit` rather than a handshake, because the kernel reclaims the device and the driver's
    // objects anyway and a throwaway has nothing to gain from getting the order right. A
    // promoted version does: it would destroy the QQuickWindow and let Qt release its scene
    // graph, then drop the wgpu device, in that order — and that ordering belongs in whatever
    // owns both, which in this prototype is nothing.
    if sharing {
        std::process::exit(0);
    }
}
