// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The renderer: a pure step function over a derived snapshot.
//!
//! Promoted from `prototypes/PROTOTYPE-renderer-host/render-core`, which existed to answer one
//! question — **can the renderer be what its clauses describe?** Every clause pushes it towards
//! a step function and none of them says what the resulting shape is:
//!
//! * It may not own a thread, block on presentation, or block on device work
//!   ([[RFC-0006:C-SURFACE]] 3, [[RFC-0004:C-HOST]] 3 and 4).
//! * Frame scheduling belongs to the host ([[RFC-0006:C-RENDER]] 4).
//! * Its data requests must be cancellable, and cancellation must take effect *without waiting
//!   for already-dispatched work* ([[RFC-0006:C-RENDER]] 2).
//! * It reads a derived snapshot, never the document graph ([[RFC-0007:C-EXTRACT]] 1 to 3), and
//!   that snapshot is metadata only ([[RFC-0007:C-EXTRACT]] 4).
//! * Its state is retained across frames, not rebuilt ([[RFC-0007:C-INVALIDATION]] 1).
//!
//! Taken together they leave exactly one shape: `advance(frame_no, &Snapshot) -> Frame`. The
//! renderer computes; the host performs. It has no clock — the frame number arrives as an
//! argument — no I/O, since retrieval is an *effect* the host executes, and no way to wait for
//! anything.
//!
//! # `no_std` is the enforcement, not a preference
//!
//! With no `std` in scope there is no `thread::spawn` to call, no `Instant::now`, no `File`, and
//! nothing to block on. The prohibitions above are **unreachable rather than merely unused**,
//! which is what makes them true of every future edit and not just of the code as written.
//!
//! This crate therefore has **no dependencies at all**, and that is load-bearing:
//! `GUARD-NO-TOOLKIT-UNDER-LIBRARY-CRATES` and `GUARD-NO-QUERY-ENGINE-UNDER-CORE` both walk
//! `strider-view`'s dependency tree, and a tree that is a single node cannot contain a toolkit
//! or a query engine. Keep it that way: anything needing an allocator, a device or a clock
//! belongs in a crate *beneath* this one — `strider-view-wgpu` is the first of them.

#![no_std]

extern crate alloc;

mod raster;
mod snapshot;
mod state;

pub use raster::{UploadToken, Uploads, Vertex, CHANNELS, HIDE, KEEP, RECLASS};
pub use snapshot::{
    Anchor, EditAction, EditDigest, EditRef, Lod, PartitionId, PipelineResultRef, Snapshot, View,
    VisiblePartition,
};
pub use state::{
    Budget, CancelReason, Delivery, Draw, Effect, Frame, Freshness, Presentation, RenderState,
    RequestId, Stats, SurfaceHandle, Target,
};
