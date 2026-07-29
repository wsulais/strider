// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Operators over point clouds.
//!
//! Every operator here declares its requirements before it runs ([[RFC-0002:C-EXEC]] 1) and is
//! correct at partition boundaries only because the planner satisfies the halo it declared
//! ([[RFC-0002:C-HALO]]). `tests/conformance_halo.rs` is where that claim is checked rather than
//! asserted: it computes an operator partition by partition and compares against the same
//! operator over one undivided partition, which is the reference the guard of the same name
//! demands.

pub mod normal;

pub use normal::{hillshade, Normals};
