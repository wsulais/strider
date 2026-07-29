// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Spatial access and the out-of-core execution model ([[RFC-0001]]).
//!
//! What lives here is what every layer above must agree on: the **retrieval port**, and
//! the geometry a partition and a query are both expressed in. A source adapter, an
//! operator and the renderer all speak these types; none of them owns them.
//!
//! The retrieval port is here rather than in a source adapter for a reason worth stating,
//! because the tempting placement is the other one. [[RFC-0004:C-HOST]] 2 is a rule about
//! *hosts*, not about COPC — an E57 or Parquet adapter is bound by the identical
//! obligation and must not inherit it by depending on the COPC crate. Putting the port in
//! the model keeps adapters siblings.
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

pub mod geom;
pub mod retrieval;

pub use geom::Aabb;
pub use retrieval::{Delivered, Need, Range, Step};
