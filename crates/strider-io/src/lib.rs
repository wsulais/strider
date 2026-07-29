// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Source adapters for point cloud formats.
//!
//! This crate is a **namespace**: it holds no format logic and re-exports the adapters
//! its features enable. Today that is COPC; the shape is chosen so that Parquet, E57 and
//! LAS arrive as siblings rather than as modules here.
//!
//! ```toml
//! strider-io = "0"                                    # the cross-platform default set
//! strider-io = { version = "0", features = ["e57"] }   # plus one more, when it exists
//! ```
//!
//! # Where things live
//!
//! | what | crate |
//! |---|---|
//! | the retrieval port, and `Aabb` | `strider-core` |
//! | COPC: header, hierarchy, Arrow batches | `strider-io-copc` |
//! | this table | `strider-io` |
//!
//! The port is in `strider-core` and not here, which is the placement worth explaining
//! because the obvious one is different. [[RFC-0004:C-HOST]] 2 binds *every* adapter
//! identically; if the port lived here, an adapter would depend on its own facade, and if
//! it lived in `strider-io-copc`, an E57 adapter would depend on COPC to name a byte
//! range. Adapters are siblings, so what they share belongs below all of them.
//!
//! # What every adapter owes
//!
//! Not a trait — there is one adapter, and a trait abstracted from one implementation
//! describes that implementation rather than the category. It is a contract stated in
//! prose until a second adapter can argue with it:
//!
//! * Reads are **explicit offset and length**, batched, never against a seekable handle,
//!   and nothing is awaited ([[RFC-0004:C-HOST]] 2 and 4) — so an adapter is a resumable
//!   state machine, not a reader.
//! * Output is a **plain Arrow record batch** with no Strider wrapper
//!   ([[RFC-0002:C-EXEC]] 3).
//! * Coordinates are **GeoArrow**, separated layout, with the coordinate reference system
//!   on the coordinate field ([[RFC-0005:C-CRS]] 1 and 4).
//! * Opening a source costs a number of reads independent of its size. A 13 GB file and a
//!   13 MB one open in the same three rounds, because only the index's root is read.

#![forbid(unsafe_code)]

/// The retrieval port every adapter speaks, re-exported so a consumer needs one
/// dependency rather than two.
pub use strider_core::{Aabb, Delivered, Need, Range, Step};

#[cfg(feature = "copc")]
pub use strider_io_copc as copc;

/// COPC, promoted to the crate root because it is the only adapter.
///
/// Kept as a deliberate convenience rather than a pattern: when a second adapter lands,
/// these go away and callers name the format, because `strider_io::Source` would then be
/// ambiguous about which format's source it is.
#[cfg(feature = "copc")]
pub use strider_io_copc::{Open, Source};

/// Every enabled adapter, for a build that wants to report what it can read.
///
/// Useful beyond diagnostics: [[RFC-0002:C-EXTENSION]] 1's argument — that a consumer
/// compiles their own in, so no published document can enumerate the set — applies to
/// formats exactly as it does to operators. What a build supports is a property of that
/// build, so it has to be askable at run time.
pub fn adapters() -> &'static [&'static str] {
    &[
        #[cfg(feature = "copc")]
        "copc",
    ]
}
