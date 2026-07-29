// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Byte retrieval, as [[RFC-0004:C-HOST]] 2 requires it to be shaped.
//!
//! The clause is unusually specific about the shape, and the specificity is the whole
//! design:
//!
//! * *"Every read MUST be expressed as an explicit offset and length, not against a
//!   seekable handle"* — so there is no `Read`, no `Seek`, and no trait method that
//!   could be handed a file. There is a `Range`.
//! * *"The interface MUST support requesting several ranges as one operation"* — so a
//!   `Need` carries a slice of ranges, not one. C-HOST's rationale gives the reason:
//!   resolving a viewport names on the order of a hundred nodes at once, and issuing
//!   those sequentially over a network multiplies latency by the node count.
//! * *"A library crate MUST NOT block its calling thread awaiting retrieval"*
//!   ([[RFC-0004:C-HOST]] 4) — so nothing here can be awaited. A reader **returns**
//!   the ranges it wants and is called again with the bytes.
//!
//! What follows from those three together is not a trait at all but a resumable state
//! machine, and that turned out to be the same shape the renderer has for the same
//! reason. Both are things that must make progress without being allowed to wait.
//! Whether that is one pattern worth naming is a governance question this crate raises
//! and does not answer.
//!
//! It lives in `strider-core` rather than in a source adapter because the obligation is
//! about hosts and not about any one format: an E57 or Parquet adapter is bound by the same
//! clause, and would otherwise have to depend on the COPC crate to name a byte range.

/// A half-open byte range of a source, named absolutely.
///
/// Deliberately not `std::ops::Range<u64>`: an offset and a **length** is what an
/// object-store `Range:` header and a browser `Blob.slice` both take, and C-HOST's
/// interfaces are shaped by the weakest plausible backend.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Range {
    pub offset: u64,
    pub len: u64,
}

impl Range {
    pub fn new(offset: u64, len: u64) -> Self {
        Self { offset, len }
    }

    pub fn end(&self) -> u64 {
        self.offset + self.len
    }
}

/// Bytes the caller retrieved for a range the reader asked for.
///
/// Borrowed rather than owned so a caller serving a read from cache need not copy —
/// which is the second thing C-HOST's rationale says explicit offsets buy.
#[derive(Clone, Copy, Debug)]
pub struct Delivered<'a> {
    pub range: Range,
    pub bytes: &'a [u8],
}

/// A batch of ranges the reader wants before it can continue.
#[derive(Clone, Debug, Default)]
pub struct Need {
    pub ranges: Vec<Range>,
}

impl Need {
    pub fn one(r: Range) -> Self {
        Self { ranges: vec![r] }
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Total bytes this need would transfer. The caller may want it for budgeting; the
    /// reader never looks.
    pub fn bytes(&self) -> u64 {
        self.ranges.iter().map(|r| r.len).sum()
    }
}

/// Where a resumable read got to.
#[derive(Clone, Debug)]
pub enum Step<T> {
    /// Retrieve these and call `step` again with them. Not an error and not a failure
    /// to make progress — it *is* the progress.
    Need(Need),
    /// Finished.
    Ready(T),
}

impl<T> Step<T> {
    pub fn need(&self) -> Option<&Need> {
        match self {
            Step::Need(n) => Some(n),
            Step::Ready(_) => None,
        }
    }

    pub fn ready(self) -> Option<T> {
        match self {
            Step::Ready(t) => Some(t),
            Step::Need(_) => None,
        }
    }
}

/// A range the reader asked for and did not get back.
///
/// Its own error rather than a variant of a larger enum, because every consumer of the
/// port can hit it and none of them should have to depend on an adapter's error type to
/// name it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NotDelivered(pub Range);

impl core::fmt::Display for NotDelivered {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "a range the reader asked for was not delivered: offset {}, length {}. \
             Every range in a `Need` must come back before the next `step`",
            self.0.offset, self.0.len
        )
    }
}

impl std::error::Error for NotDelivered {}

/// Find the bytes delivered for an exact range.
pub fn find<'a>(delivered: &[Delivered<'a>], want: Range) -> Result<&'a [u8], NotDelivered> {
    delivered
        .iter()
        .find(|d| d.range == want)
        .map(|d| d.bytes)
        .ok_or(NotDelivered(want))
}
