// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The COPC source adapter.
//!
//! Reached through `strider-io`, which re-exports it behind the `copc` feature. Usable
//! directly by a consumer that wants one format and no facade.
//!
//! # The shape, and why it is this shape
//!
//! Every published LAS or COPC reader takes a path or a seekable stream.
//! [[RFC-0004:C-HOST]] admits neither: (1) forbids calling filesystem APIs, (2)
//! requires every read to be "an explicit offset and length, not against a seekable
//! handle" and to support several ranges as one operation, and (4) forbids blocking the
//! calling thread awaiting retrieval.
//!
//! What survives those three is not a reader but a **resumable state machine**: it
//! returns the byte ranges it wants and is called again with them.
//!
//! ```no_run
//! use strider_io_copc::{Open, Step};
//! # fn fetch(_: &strider_io_copc::Range) -> Vec<u8> { unimplemented!() }
//! let mut open = Open::new();
//! let mut delivered = Vec::new();
//! let source = loop {
//!     // The host retrieves; the library never does.
//!     match open.step(&delivered)? {
//!         Step::Need(need) => { /* fetch every range in `need`, refill `delivered` */ }
//!         Step::Ready(source) => break source,
//!     }
//! # ; unreachable!()
//! };
//! # Ok::<(), strider_io_copc::Error>(())
//! ```
//!
//! Opening any COPC costs three rounds, and the third reads the **root hierarchy page
//! only**. Deeper pages are read when a request needs them, so opening a 13 GB source
//! costs the same as opening a 13 MB one — which is [[RFC-0002:C-MEMORY]]'s claim
//! applied to the index rather than to the points.
//!
//! # What it produces
//!
//! A plain Arrow record batch with no Strider envelope ([[RFC-0002:C-EXEC]] 3), with
//! coordinates in GeoArrow's separated layout and the coordinate reference system
//! carried on the coordinate field ([[RFC-0005:C-CRS]] 1 and 4).
//!
//! # What is deliberately absent
//!
//! No `std::fs`, no `std::path`, no thread, and nothing to await — the crate has no way
//! to obtain bytes at all except from its caller. `laz` is taken for its arithmetic
//! decoder only, through the byte-oriented `laz::record` API; its chunk-table reader is
//! avoided because reading a chunk table means seeking.

pub mod batch;
pub mod copc;
pub mod error;
pub mod las;
pub mod source;

pub use copc::{Entry, Hierarchy, Info, Node, VoxelKey};
pub use error::{Error, Result};
pub use las::{Header, Transform};
pub use source::{Decoder, Open, Source};

// Re-exported for convenience, and defined in `strider-core`: the port is the model's,
// not this adapter's ([[RFC-0004:C-HOST]] 2).
pub use strider_core::{Aabb, Delivered, Need, Range, Step};
