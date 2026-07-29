// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Rendering: the namespace crate.
//!
//! Re-exports and nothing else. The renderer is [`strider_view_core`], always present and
//! dependency-free; a backend that can put its output on a device is opt-in behind a feature.
//!
//! ```text
//! strider-view            this crate — re-exports, declares the default set
//! strider-view-core       the renderer: a step function, no_std, zero dependencies
//! strider-view-wgpu       a backend: wgpu, WGSL, and per-provider interop  (feature "wgpu")
//! ```
//!
//! # Why the default set is empty
//!
//! `strider-io` defaults to carrying an adapter, because a consumer asking for IO wants to read
//! something. Rendering is not like that: a host computing a frame, or a test asserting on the
//! renderer's effects, needs the model and no device at all. Defaulting to a backend would put
//! wgpu beneath every dependent including those, so the backend is spelled rather than assumed.

#![forbid(unsafe_code)]

pub use strider_view_core::*;

/// The GPU backend: wgpu, the WGSL point pipeline, colour ramps, and the provider-specific
/// interop a host needs to share a device or a texture with a toolkit.
#[cfg(feature = "wgpu")]
pub use strider_view_wgpu as wgpu;
