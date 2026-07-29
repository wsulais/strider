// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Typed, recoverable failures. A malformed source is a `Result`, never a panic:
//! library crates must not assume a host that can absorb one ([[RFC-0004:C-HOST]]).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("not a LAS file: expected signature LASF, found {found:?}")]
    NotLas { found: [u8; 4] },

    #[error("LAS {major}.{minor} is not supported; COPC requires 1.4")]
    UnsupportedVersion { major: u8, minor: u8 },

    #[error(
        "point data record format {0} is not supported; COPC admits only 6, 7 and 8 \
         (LAS 1.4 extended formats)"
    )]
    UnsupportedPointFormat(u8),

    #[error("no COPC info VLR (user id `copc`, record id 1): this is a LAS/LAZ file, not a COPC")]
    NoCopcInfo,

    #[error("no LASzip VLR (user id `laszip encoded`, record id 22204)")]
    NoLaszipVlr,

    #[error("the source declared {declared} bytes for {what}, and {delivered} were delivered")]
    ShortRead {
        what: &'static str,
        declared: usize,
        delivered: usize,
    },

    /// From the retrieval port, which defines it: every consumer of the port can hit
    /// this, so it is not this adapter's to name.
    #[error(transparent)]
    NotDelivered(#[from] strider_core::retrieval::NotDelivered),

    #[error("`step` was called again after it returned `Ready`; the `Open` is spent")]
    SteppedAfterReady,

    #[error("hierarchy page of {0} bytes is not a whole number of 32-byte entries")]
    RaggedHierarchyPage(usize),

    #[error("node {key} declares {declared} points, and {decoded} were decoded")]
    NodePointCount {
        key: crate::copc::VoxelKey,
        declared: u32,
        decoded: u32,
    },

    #[error("LASzip decompression failed: {0}")]
    Laszip(String),

    #[error("building an Arrow batch failed: {0}")]
    Arrow(#[from] arrow::error::ArrowError),
}

impl From<laz::LasZipError> for Error {
    fn from(e: laz::LasZipError) -> Self {
        Error::Laszip(e.to_string())
    }
}

pub type Result<T> = core::result::Result<T, Error>;
