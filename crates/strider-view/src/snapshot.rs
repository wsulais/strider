// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The extract output: what the renderer is allowed to know.
//!
//! [[RFC-0007:C-EXTRACT]] 4 says extract "MUST operate on metadata alone: which
//! partitions are visible, and which edits apply to each", and MUST NOT read point
//! data or await retrieval. That is enforced here by *absence*: nothing in this module
//! can hold a point, so a snapshot carrying point data does not typecheck. Points
//! reach the renderer only as device buffers it is given access to
//! (`state::Uploads`), and only after the host has completed retrieval it dispatched.
//!
//! What took a rewrite: an early version gave each visible partition one `EditDigest`
//! and nothing else. A digest tells the renderer *that* the effective edit set changed
//! and not *what* it is, so the only recovery is re-retrieval — which turns every
//! lasso into a round trip through the network. "Which edits apply to each" is a
//! stronger phrase than it looks: it means the gestures themselves, which are about a
//! kilobyte each ([[RFC-0007:C-EDIT]] 2) and are still metadata. See NOTES.md.

use alloc::string::String;
use alloc::vec::Vec;

/// A cell of the source's spatial hierarchy — in this prototype a real COPC octree
/// node. An identifier, not data.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct PartitionId(pub u32);

/// Level of detail. `Lod(0)` is the COPC root; higher is finer.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Lod(pub u8);

/// Identity of a partition's *effective edit set* — order-sensitive, because
/// reordering two edits changes the result ([[RFC-0007:C-EDIT]] 4).
///
/// Used only as an equality test: "is my retained edit mask still the one this edit
/// set implies?". The renderer never tries to invert it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct EditDigest(pub u64);

/// What an edit does to the points its region selects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditAction {
    /// Reclassify. A recolour of points already resident.
    Classify { class: u8 },
    /// Remove from view. A mask over points already resident.
    Delete,
}

/// One recorded gesture, as it applies to one partition, in stack order.
///
/// `order` is its index in the document's edit stack, and the renderer needs it
/// because the stack is an ordered sequence and not a set
/// ([[RFC-0007:C-EDIT]] 4): "classify region A as ground" then "delete ground in
/// region B" gives a different picture from the reverse where the regions overlap.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EditRef {
    pub order: u32,
    /// The gesture's region, in the same local metres the vertices use.
    pub min: [f32; 2],
    pub max: [f32; 2],
    /// Attribute predicate: apply only to points of this class
    /// ([[RFC-0007:C-EDIT]] 2, "an optional attribute predicate").
    pub only_class: Option<u8>,
    pub action: EditAction,
}

impl EditRef {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.min[0] && x <= self.max[0] && y >= self.min[1] && y <= self.max[1]
    }
}

/// A partition the camera can see, and everything the renderer may know about it.
#[derive(Clone, Debug)]
pub struct VisiblePartition {
    pub id: PartitionId,
    /// The level of detail the host's camera implies. A request, not a promise: the
    /// renderer draws a coarser resident level while this one is in flight.
    pub lod: Lod,
    /// Identity of the effective edit set below — carried *alongside* the edits, not
    /// instead of them, so "has this changed?" stays one comparison.
    pub edits_digest: EditDigest,
    /// The edits intersecting this partition, in stack order. Bounded by the edits
    /// that intersect it and independent of the stack's length
    /// ([[RFC-0007:C-INVALIDATION]] 3) — a property of how the host builds this
    /// vector, and measured in `checks`.
    pub edits: Vec<EditRef>,
    /// Node bounds in local metres. Metadata, straight from the COPC hierarchy.
    pub min: [f32; 2],
    pub max: [f32; 2],
    /// Points the node holds, from the hierarchy entry. Also metadata — knowing how
    /// many there are is not reading them.
    pub point_count: u32,
}

/// Depth-dependent overlay content: a measurement anchored to a position in the scene.
///
/// It is here, and therefore drawn by the renderer, because [[RFC-0006:C-OVERLAY]] 1
/// requires exactly that. Interface chrome is deliberately *not* representable in a
/// `Snapshot` or a `Frame`; see `state::Frame`.
#[derive(Clone, Debug)]
pub struct Anchor {
    pub label: String,
    pub x: f32,
    pub y: f32,
    /// Height of the anchored point. What a composited label has no equivalent of.
    pub z: f32,
}

/// A processing pipeline's output, already materialised by the host.
///
/// [[RFC-0006:C-RENDER]] 3 makes displaying pipeline output "a separately scheduled
/// operation" the render path must not perform synchronously. Modelled as a reference
/// to something already finished: there is no way for `advance` to ask for one, so the
/// render path cannot reach the planner even by accident.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PipelineResultRef {
    pub id: u32,
    pub points: u32,
}

/// The camera, resolved by the host into the window the renderer draws.
///
/// The renderer holds no camera of its own: [[RFC-0006:C-RENDER]] 4 gives frame
/// scheduling to the host, and the camera is the host's input to the frame.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct View {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub cols: u16,
    pub rows: u16,
}

/// One extract's worth of derived state ([[RFC-0007:C-EXTRACT]] 3).
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Increments once per extract.
    pub generation: u64,
    pub view: View,
    pub visible: Vec<VisiblePartition>,
    pub pipeline: Option<PipelineResultRef>,
}
