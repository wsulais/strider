// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The Vulkan arm's C entry point, mirroring `viewport_mac.h`.
//
// This header exists because its absence was a link error and nothing else. `share_vulkan.rs`
// declares `strider_share_vulkan` in an `unsafe extern "C"` block, so it wants an unmangled
// symbol; `viewport_vulkan.cpp` defined the function bare, which gives it C++ mangling, and the
// two never met. The Metal arm had the equivalent declaration all along, which is why the
// platform split linked on macOS and only failed once it was built for Linux.
#ifndef STRIDER_VIEWPORT_VULKAN_H
#define STRIDER_VIEWPORT_VULKAN_H

#include <cstdint>

extern "C" bool strider_share_vulkan(::std::uint64_t instance,
                                     ::std::uint64_t physical_device,
                                     ::std::uint64_t device,
                                     ::std::uint32_t queue_family_index,
                                     ::std::uint32_t queue_index);

#endif // STRIDER_VIEWPORT_VULKAN_H
