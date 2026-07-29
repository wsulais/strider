// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Rasterisation, the retained edit mask, and the depth test.
//!
//! The renderer draws a top-down plan: for each cell, the highest point that landed in
//! it. That gives a real depth buffer over real points, which is what lets
//! [[RFC-0006:C-OVERLAY]] 1 be *demonstrated* rather than asserted — a measurement
//! anchored under a canopy is occluded by the canopy, and the prototype can say by
//! which partition.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::snapshot::{Anchor, EditAction, EditRef, PartitionId, View};

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

/// The drawn output: a top-down plan with a depth buffer.
///
/// Returned by value each frame. That is the literal reading of "the renderer is a
/// function from state to drawn output rather than a loop"
/// ([[RFC-0006:C-SURFACE]] 3's rationale) — in a real renderer these are the commands
/// recorded into the surface, and here they are cells a host can paint into a terminal
/// or into a Qt widget without either knowing about the other.
#[derive(Clone, Debug)]
pub struct Raster {
    pub cols: u16,
    pub rows: u16,
    /// Highest point in each cell. `f32::MIN` where nothing landed.
    pub height: Vec<f32>,
    /// Class of that highest point.
    pub class: Vec<u8>,
    /// Which partition supplied it — what an occlusion verdict names.
    pub owner: Vec<u32>,
    /// Points that landed in each cell, after the edit mask.
    pub hits: Vec<u32>,
    pub drawn_points: u64,
    pub masked_out: u64,
}

impl Raster {
    pub(crate) fn new(view: &View) -> Self {
        let n = view.cols as usize * view.rows as usize;
        Self {
            cols: view.cols,
            rows: view.rows,
            height: vec![f32::MIN; n],
            class: vec![0; n],
            owner: vec![u32::MAX; n],
            hits: vec![0; n],
            drawn_points: 0,
            masked_out: 0,
        }
    }

    pub fn at(&self, col: u16, row: u16) -> usize {
        row as usize * self.cols as usize + col as usize
    }

    pub fn filled(&self, col: u16, row: u16) -> bool {
        self.hits[self.at(col, row)] > 0
    }

    /// The height range of what was actually drawn, or `None` if nothing was.
    ///
    /// Exists so a host can frame the camera on the cloud rather than on the source's
    /// declared extent. The two differ by a lot: this file spans 70 m of z, and the points a
    /// viewport at level 6 actually holds sit within about 4 m of the ground.
    pub fn drawn_z_range(&self) -> Option<(f32, f32)> {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for (i, z) in self.height.iter().enumerate() {
            if self.hits[i] > 0 {
                lo = lo.min(*z);
                hi = hi.max(*z);
            }
        }
        (lo <= hi).then_some((lo, hi))
    }

    pub(crate) fn draw(
        &mut self,
        view: &View,
        id: PartitionId,
        verts: &[Vertex],
        mask: Option<&Mask>,
    ) {
        let (w, h) = (
            view.max[0] - view.min[0],
            view.max[1] - view.min[1],
        );
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        for (i, v) in verts.iter().enumerate() {
            let mut class = v.class;
            if let Some(m) = mask {
                match m.flags.get(i).copied().unwrap_or(KEEP) {
                    HIDE => {
                        self.masked_out += 1;
                        continue;
                    }
                    f if f >= RECLASS => class = f - RECLASS,
                    _ => {}
                }
            }
            let u = (v.x - view.min[0]) / w;
            let t = (v.y - view.min[1]) / h;
            if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&t) {
                continue;
            }
            let col = (u * view.cols as f32) as u16;
            // Rows run north-to-south so the plan reads like a map.
            let row = ((1.0 - t) * view.rows as f32) as u16;
            let idx = self.at(col.min(view.cols - 1), row.min(view.rows - 1));
            self.drawn_points += 1;
            self.hits[idx] += 1;
            if v.z > self.height[idx] {
                self.height[idx] = v.z;
                self.class[idx] = class;
                self.owner[idx] = id.0;
            }
        }
    }
}

/// Whether an anchor is in front of the cloud, and what hid it if not.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnchorVerdict {
    Visible,
    /// Hidden by points nearer the camera — for a top-down view, points above it.
    Occluded { by: PartitionId, above_by_cm: i32 },
    /// Outside the view.
    OffScreen,
}

/// An anchor as the renderer emits it.
#[derive(Clone, Debug)]
pub struct DrawnAnchor {
    pub label: String,
    pub col: u16,
    pub row: u16,
    pub z: f32,
    pub verdict: AnchorVerdict,
}

/// Depth-test every anchor against what was actually drawn.
///
/// Against what was *drawn*, not against what is visible: a partition still in flight
/// has nothing on screen to occlude with, so this keeps the verdict honest about the
/// frame the user is looking at. It also means an anchor can flicker from visible to
/// occluded as a finer level arrives — which is a real property of the design and worth
/// seeing rather than smoothing over.
pub(crate) fn test_anchors(anchors: &[Anchor], view: &View, raster: &Raster) -> Vec<DrawnAnchor> {
    let (w, h) = (view.max[0] - view.min[0], view.max[1] - view.min[1]);
    let mut out = Vec::with_capacity(anchors.len());
    for a in anchors {
        let u = (a.x - view.min[0]) / w;
        let t = (a.y - view.min[1]) / h;
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&t) {
            out.push(DrawnAnchor {
                label: a.label.clone(),
                col: 0,
                row: 0,
                z: a.z,
                verdict: AnchorVerdict::OffScreen,
            });
            continue;
        }
        let col = ((u * view.cols as f32) as u16).min(view.cols - 1);
        let row = (((1.0 - t) * view.rows as f32) as u16).min(view.rows - 1);
        let idx = raster.at(col, row);
        let verdict = if raster.hits[idx] > 0 && raster.height[idx] > a.z {
            AnchorVerdict::Occluded {
                by: PartitionId(raster.owner[idx]),
                above_by_cm: ((raster.height[idx] - a.z) * 100.0) as i32,
            }
        } else {
            AnchorVerdict::Visible
        };
        out.push(DrawnAnchor {
            label: a.label.clone(),
            col,
            row,
            z: a.z,
            verdict,
        });
    }
    out
}
