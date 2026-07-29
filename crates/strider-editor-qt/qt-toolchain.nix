# SPDX-FileCopyrightText: 2026 Strider contributors
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# PROTOTYPE / THROWAWAY — a Qt6 prefix that `cxx-qt-build` can actually use.
#
# Not in devenv.nix on purpose: Qt belongs to this one throwaway target and must not reach
# the workspace, because a toolkit beneath a library crate is what [[RFC-0006:C-TOOLKIT]] 1
# forbids and what [[RFC-0001:C-LICENSE]] 4 makes a licence violation.
#
# Four things are needed and none is obvious. Recorded because rediscovering them costs an
# afternoon:
#
#   1. **A joined prefix.** nixpkgs ships each Qt module as its own store path, while
#      `qt-build-utils` asks qmake for one lib directory and expects every module's `.prl`
#      file to be in it. Without the join it panics with
#      "Could not find a prl path for Qt module: Qml".
#   2. **A qmake that reports the join.** qmake6 reports its compiled-in prefix, which is
#      qtbase's path, not the join's. A `qt.conf` beside the binary overrides it — and the
#      binary must be *copied* rather than symlinked, since qmake resolves `qt.conf`
#      relative to the real executable.
#   3. **libglvnd.** Qt6Gui's `.prl` names `-lGLX -lOpenGL`, which live in libglvnd rather
#      than in libGL, so the link fails without it on LIBRARY_PATH.
#   4. **Referencing cxx-qt-lib from Rust.** Depending on it is not enough: with no type
#      used, the linker drops its objects and the generated initializer fails with
#      "undefined reference to cxx_qt_init_crate_cxx_qt_lib".
#
# ONE file for both platforms rather than a sibling, because the two shells share three of
# the four traps above and the join is the same construction. A second file would be the
# duplication that made the two hosts' cameras drift — see NOTES.md, "the two bugs that made a
# working renderer look broken".
#
# What macOS changes:
#
#   * **Qt modules are frameworks.** `QtGui.framework/Resources/QtGui.prl`, not
#     `libQt6Gui.prl`. `qt-build-utils` handles both, so trap (1) still bites — the join is
#     what puts qtdeclarative's frameworks beside qtbase's — but nothing extra is needed.
#   * **No GL, no Vulkan loader, no X server.** wgpu selects Metal, Qt selects Metal, and the
#     display is real, so libglvnd, the Vulkan loader, Xvfb, openbox and weston all go.
#   * **Qt is still built with `QT_FEATURE_vulkan 1`** even here, which is why the Vulkan
#     headers are NOT dropped from the shell: Qt's own headers reach for `vulkan/vulkan.h`
#     wherever that feature is on, whatever the platform actually runs.
#   * **A framework search path is needed, and it cannot be set from `build.rs`.** Trap (5),
#     and it is the one that stops the build dead: `#include <QtCore/QObject>` resolves inside
#     `QtCore.framework/Headers`, which `-I` cannot reach — it needs `-F <lib>`. `qt-build-utils`
#     knows this and hands the framework paths to `cc`, but the first thing to fail is the
#     **cxx-qt crate's own** build script compiling `qobject.cxx.cpp`, and nothing in this
#     project's `build.rs` can reach that invocation. `NIX_CFLAGS_COMPILE` can, because every
#     compiler here is the nix cc-wrapper, so the flag is set once for the whole shell.
{ pkgs ? import <nixpkgs> { } }:
let
  darwin = pkgs.stdenv.hostPlatform.isDarwin;
  qtJoined = pkgs.symlinkJoin {
    name = "qt6-joined";
    # qtwayland supplies the `wayland` platform plugin and the Wayland client libraries; a
    # qtbase-only prefix can only ever run under xcb. It does not exist on darwin, where the
    # platform plugin is cocoa and lives in qtbase.
    paths = with pkgs.qt6; [
      qtbase qtbase.dev qtdeclarative qtdeclarative.dev qtshadertools
    ] ++ pkgs.lib.optionals (!darwin) [ qtwayland qtwayland.dev ];
  };
  # (2): a real copy plus a qt.conf, so qmake reports the joined prefix.
  qmakeWrapped = pkgs.runCommand "qmake6-joined" { } ''
    mkdir -p $out/bin
    cp -L ${qtJoined}/bin/qmake6 $out/bin/qmake6
    printf '[Paths]\nPrefix=%s\n' "${qtJoined}" > $out/bin/qt.conf
  '';
in
pkgs.mkShell ({
  # Qt is built with QT_FEATURE_vulkan 1 — on macOS too — so `QVulkanInstance`'s class body is
  # behind `<vulkan/vulkan.h>`. Without the headers the type is only forward-declared and the
  # error reads "incomplete type", which says nothing about the missing package.
  packages = [ qmakeWrapped pkgs.cmake pkgs.ninja pkgs.vulkan-headers ]
    ++ pkgs.lib.optionals (!darwin) ([ pkgs.gcc pkgs.libGL pkgs.libglvnd
                                       pkgs.vulkan-loader pkgs.vulkan-validation-layers ]
    # Xvfb is the test path: this project's Linux development machines are frequently
    # headless, and presenting to a real surface is the thing that cannot be checked
    # offscreen.
    ++ [ pkgs.xvfb pkgs.xdpyinfo pkgs.imagemagick pkgs.xdotool pkgs.xorg.xwininfo ]
    # A window manager, because without one X11 has no concept of input focus: `SetInputFocus`
    # fails with BadMatch, nothing is focused, and synthetic keys go nowhere. That made the
    # keyboard untestable in this harness and produced an absence of evidence that looked
    # exactly like a broken handler.
    ++ [ pkgs.openbox ]
    # Weston on its X11 backend is how the Wayland path gets tested without a Wayland
    # session: Xvfb provides a screen, weston provides a real Wayland compositor inside it,
    # and Qt speaks actual Wayland to it. The protocol under test is not simulated; only
    # weston's own output is X11.
    ++ [ pkgs.weston ])
    # macOS has a display, so there is no server to start and no compositor to nest. What it
    # still needs is a way to MEASURE a screenshot: `screencapture` is in the base system,
    # ImageMagick is not.
    ++ pkgs.lib.optionals darwin [ pkgs.imagemagick ];

  # (5) The framework search path, for every compiler in the shell — ours and the ones inside
  # dependencies' build scripts alike. Harmless where there are no frameworks.
  NIX_CFLAGS_COMPILE = pkgs.lib.optionalString darwin "-F ${qtJoined}/lib";

  QMAKE = "${qmakeWrapped}/bin/qmake6";
  QT_PREFIX = qtJoined;
  QT_QPA_PLATFORM_PLUGIN_PATH = "${qtJoined}/lib/qt-6/plugins/platforms";
  # (6) The QML import path, which is trap (1) again at RUN time rather than at build time.
  #
  # Qt resolves QML imports against the prefix compiled into QtCore, and that is qtbase's own
  # store path — where `QtQuick.Controls` does not exist, because it ships in qtdeclarative.
  # The failure is `module "QtQuick.Controls" is not installed` from an engine that loaded
  # nothing, and no window ever appears: the process sits in `app.exec()` with an empty engine,
  # at 0% CPU, which looks exactly like a hang.
  QML_IMPORT_PATH = "${qtJoined}/lib/qt-6/qml";
  QML2_IMPORT_PATH = "${qtJoined}/lib/qt-6/qml";
} // pkgs.lib.optionalAttrs (!darwin) {
  # (3)
  LIBRARY_PATH = "${pkgs.libglvnd}/lib";
  # wgpu needs the loader at run time; the driver ICDs are already under /run/opengl-driver.
  LD_LIBRARY_PATH = "${pkgs.vulkan-loader}/lib:${pkgs.libglvnd}/lib:/run/opengl-driver/lib";
})
