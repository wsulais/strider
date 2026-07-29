// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Spatial access and the out-of-core execution model ([[RFC-0001]]).
//!
//! Nothing is implemented yet — this crate exists so the workspace, the licence
//! split ([[RFC-0001:C-LICENSE]]), and the portability gate
//! ([[RFC-0004:C-PORT-GATE]]) are real and checkable before any code lands.
//!
//! Two constraints shape every signature added here:
//!
//! * Capabilities are **injected, never ambient** ([[RFC-0004:C-HOST]]) — storage,
//!   byte retrieval and parallelism come from the caller, so this crate compiles
//!   for a target with no filesystem, no threads and no blocking I/O.
//! * Batches are **plain Arrow** ([[RFC-0002:C-EXEC]] 3) — no Strider envelope on
//!   the data path. Spatial properties belong to the pipeline stage, with halo
//!   membership the sole per-point exception ([[RFC-0002:C-HALO]] 1).
