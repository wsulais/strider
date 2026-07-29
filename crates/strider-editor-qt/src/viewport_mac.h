// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// PROTOTYPE / THROWAWAY — what the Objective-C++ side of the viewport offers plain C++.
#ifndef STRIDER_VIEWPORT_MAC_H
#define STRIDER_VIEWPORT_MAC_H

#include <cstdint>

#include <QtCore/QSize>

// The CGWindowID of the window containing this `NSView *`, or 0 if it is not in one yet.
::std::uint64_t strider_window_number(void *nsview);

// Hand Qt the `MTLDevice` and `MTLCommandQueue` the renderer already created.
//
// The Metal sibling of `strider_share_vulkan`, and the same argument for the direction: this
// process knows how its own device was made and Qt does not describe its. Simpler here because
// there is no instance to adopt and no extension set to match — a device and a queue are the
// whole handshake.
//
// C linkage, because Rust declares it in `share_metal.rs` rather than through the cxx-qt bridge:
// the bridge is compiled on every platform and this exists for one.
extern "C" bool strider_share_metal(::std::uint64_t device, ::std::uint64_t queue);

#endif // STRIDER_VIEWPORT_MAC_H
