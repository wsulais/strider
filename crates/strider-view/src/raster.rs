// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The vertex format a device buffer carries, and the retained edit mask.
//!
//! Depth-dependent content ([[RFC-0006:C-OVERLAY]] 1) is now handled in hardware: the
//! device layer draws anchor billboards against the same depth buffer the cloud wrote, so
//! occlusion is a `<` in the GPU and not a decision anybody made on the CPU. An earlier
//! version rasterised every point into a top-down plan here just to depth-test anchors;
//! that was a second, redundant copy of a depth test the GPU already did, and it cost on
//! the order of the point count per frame. What remains is what only the CPU can do: the
//! mask an edit set implies, cached against its digest.

use alloc::vec;
use alloc::vec::Vec;

use crate::snapshot::{EditAction, EditRef};

/// One point as the renderer holds it: the vertex format a device buffer carries.
///
/// `f32`, and local to the source's origin, because that is what a graphics pipeline
/// takes. The conversion from the source's `f64` coordinates is the host's, done once
/// at upload — doing it per frame would be the renderer re-deriving what it was given.
#[derive(Clone, Copy, Debug)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// LAS classification. The one attribute the renderer interprets, and only because an
    /// edit may carry an attribute predicate ([[RFC-0007:C-EDIT]] 2) which `build_mask`
    /// evaluates. Nothing else here is interpreted.
    pub class: u8,
    /// Colour as the source recorded it, normalised to 0..1 by the host.
    pub rgb: [f32; 3],
    /// Scalar channels the host chose to make rampable, and whose **meaning the renderer
    /// does not know**.
    ///
    /// This is deliberately not `intensity`, `return_number`, `number_of_returns`. Naming
    /// them would put LAS — and therefore COPC — inside the renderer, and then a derived
    /// attribute like height above ground could not be ramped without changing the shader.
    /// The host decides what each channel carries and publishes the labels; the renderer
    /// ramps whichever index it is told to. A channel computed by an analytical engine and
    /// one read straight from a file are indistinguishable here, which is the point.
    pub channels: [f32; CHANNELS],
}

/// How many rampable channels a vertex carries.
///
/// Four is a budget, not a semantic claim: it costs 16 bytes a point and covers the
/// attributes a viewer offers at once. A host wanting a fifth swaps one out, which is a
/// re-upload and not a shader change.
pub const CHANNELS: usize = 5;

/// A handle to device memory the host owns and the renderer does not.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct UploadToken(pub u64);

/// Access to the buffers the host uploaded.
///
/// A capability, injected ([[RFC-0004:C-HOST]]). The renderer can read a buffer it was
/// given a token for and can do nothing else — it cannot allocate one, free one, or
/// retrieve the points that fill one.
pub trait Uploads {
    fn vertices(&self, token: UploadToken) -> &[Vertex];
}

/// Which points a partition's effective edit set hides or reclassifies.
///
/// This is the expensive derived thing, and the reason [[RFC-0007:C-INVALIDATION]] 2 is
/// worth obeying: computing it costs one pass over the partition's points per
/// intersecting edit, while checking whether it is still valid costs one `u64`
/// comparison. The camera moves far more often than the edit stack changes, so caching
/// this across frames is where the clause pays.
#[derive(Clone, Debug)]
pub(crate) struct Mask {
    pub digest: crate::snapshot::EditDigest,
    /// One entry per vertex. `KEEP`, `HIDE`, or `RECLASS + class`.
    pub flags: Vec<u8>,
    pub hidden: u32,
    pub reclassified: u32,
}

/// Mask values, public so a device backend can apply what the policy layer decided.
pub const KEEP: u8 = 0;
pub const HIDE: u8 = 1;
pub const RECLASS: u8 = 2;

/// Apply an ordered edit set to a partition's points.
///
/// Walked in stack order, because the stack is a sequence and not a set
/// ([[RFC-0007:C-EDIT]] 4): "classify region A as ground" then "delete ground in region
/// B" is not the same picture as the reverse where A and B overlap. That is not a
/// hypothetical here — the prototype's `r I J` command swaps two edits and the digest
/// changes, which is the clause's requirement made observable.
pub(crate) fn build_mask(
    verts: &[Vertex],
    edits: &[EditRef],
    digest: crate::snapshot::EditDigest,
) -> Mask {
    let mut flags = vec![KEEP; verts.len()];
    let mut hidden = 0u32;
    let mut reclassified = 0u32;
    for e in edits {
        for (i, v) in verts.iter().enumerate() {
            if !e.contains(v.x, v.y) {
                continue;
            }
            // The attribute predicate is read against the point's *current* class —
            // the class a previous edit in this same set may have given it. Reading the
            // source's class instead would make the stack a set.
            let current = match flags[i] {
                f if f >= RECLASS => f - RECLASS,
                _ => v.class,
            };
            if let Some(want) = e.only_class {
                if current != want {
                    continue;
                }
            }
            match e.action {
                EditAction::Delete => {
                    if flags[i] != HIDE {
                        hidden += 1;
                        if flags[i] >= RECLASS {
                            reclassified -= 1;
                        }
                    }
                    flags[i] = HIDE;
                }
                EditAction::Classify { class } => {
                    if flags[i] == HIDE {
                        // An edit does not resurrect a point a later delete removed;
                        // but this one is *earlier* in the stack than that delete only
                        // if the walk order says so, and it does not. So a classify
                        // after a delete leaves it deleted.
                        continue;
                    }
                    if flags[i] < RECLASS {
                        reclassified += 1;
                    }
                    flags[i] = RECLASS.saturating_add(class);
                }
            }
        }
    }
    Mask {
        digest,
        flags,
        hidden,
        reclassified,
    }
}
