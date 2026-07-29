// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The document, the edits applied to it, and the metadata-only extract a renderer reads.
//!
//! Promoted from the renderer prototype's `host-sim`, which mixed this model with the harness
//! that drove it. The harness dies with the prototype; this does not, and it is deliberately
//! **permissively licensed and toolkit-free** so that a second editor — a web one, say — reads
//! the same document model rather than reimplementing it. That is the whole reason it is a crate
//! and not a module inside `strider-editor-qt`.
//!
//! What lives here is bounded by [[RFC-0007]]:
//!
//! * **Edits are recorded, not applied** ([[RFC-0007:C-EDIT]] 5). Nothing writes to the source.
//! * **Extract is metadata only** ([[RFC-0007:C-EXTRACT]] 4), and provably so: its parameters are
//!   the document and the index. There is no `Store`, so no point can be read, and no
//!   `Retrieval`, so nothing can be fetched or awaited.
//! * **The edit index is octree-keyed** ([[RFC-0007:C-INVALIDATION]] 3, and [[ADR-0005]] already
//!   implied it). A bucket grid was tried first and visited 9,216 buckets to find 0 edits when
//!   resolving a COPC root node; the measurement is in the prototype's `NOTES.md`.
//!
//! What does *not* live here is anything that fetches: retrieval is the host's, injected as a
//! capability ([[RFC-0004:C-HOST]]), and the threading around it belongs to the application.

pub mod doc;
pub mod store;

pub use doc::{Document, ExtractStats, RampStats, CHANNEL_LABELS};
