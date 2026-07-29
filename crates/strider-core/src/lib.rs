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
//!   the data path. Spatial properties belong to the pipeline stage, and halo
//!   membership is not an exception to that: it is derived from the stage's own
//!   partition bounds together with a point's position, so the column carrying it
//!   is a cache of that derivation ([[RFC-0002:C-HALO]] 1, [[RFC-0002:C-EXEC]] 4).
