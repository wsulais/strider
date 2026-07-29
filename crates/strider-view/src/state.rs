// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The renderer: retained state, and one step function over it.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::raster::{build_mask, Mask, UploadToken, Uploads};
use crate::snapshot::{EditDigest, Lod, PartitionId, PipelineResultRef, Snapshot};

/// A drawing target the renderer was handed and does not understand
/// ([[RFC-0006:C-SURFACE]] 1).
///
/// Opaque in the strong sense: there is no accessor, so nothing in this crate can
/// dereference it or ask what produced it ([[RFC-0006:C-SURFACE]] 2). The host knows it
/// came from a `QWindow`; the renderer has no way to find that out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SurfaceHandle(usize);

impl SurfaceHandle {
    /// Constructible by a host, readable by nobody.
    pub fn from_host(raw: usize) -> Self {
        Self(raw)
    }
}

/// Present to a surface, or draw into a target the host composites
/// ([[RFC-0006:C-SURFACE]] 4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Presenting(SurfaceHandle),
    /// C-SURFACE 4 requires switching to not change the obligations in (1) to (3), and
    /// that is enforced structurally: `Target` is read in exactly one place, the last
    /// statement of `advance`, so it cannot reach the request, cancellation,
    /// invalidation or rasterisation logic at all.
    Offscreen,
}

/// How many uploads may be resident. The host's number: the renderer has no way to ask
/// how much device memory exists.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub max_uploads: usize,
    pub max_points: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct RequestId(pub u64);

/// Why the renderer gave up on a request it had issued.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CancelReason {
    /// The camera moved and the partition left the view.
    LeftView,
    /// The camera moved and a different level of detail is wanted for it now.
    SupersededByLod,
}

/// What the host must do on the renderer's behalf.
///
/// The renderer performs none of it, and cannot: retrieval is I/O and eviction touches
/// device memory, both capabilities [[RFC-0004:C-HOST]] withholds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Effect {
    Request {
        req: RequestId,
        id: PartitionId,
        lod: Lod,
    },
    /// [[RFC-0006:C-RENDER]] 2. Note what is *not* here: any way for the host to
    /// acknowledge. The renderer has already forgotten the request by the time this is
    /// emitted, so there is nothing an acknowledgement could update — which is what
    /// "cancellation MUST take effect without waiting for already-dispatched work"
    /// means once the renderer may not block.
    Cancel {
        req: RequestId,
        reason: CancelReason,
    },
    Evict {
        id: PartitionId,
        lod: Lod,
        token: UploadToken,
    },
}

/// Why a partition was drawn from the upload it was drawn from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Freshness {
    /// The wanted level of detail was resident.
    Fresh,
    /// It was not, so a coarser resident level stood in while the request flies. This
    /// is what the renderer does *instead of* blocking, and it is the only other option
    /// it has.
    CoarserFallback { held: Lod },
    /// Nothing coarser was resident but something finer was — the camera zoomed out and
    /// a fine upload from before is still there. Drawing it costs more points than the
    /// frame needs; refusing it costs the user a hole.
    FinerFallback { held: Lod },
}

/// What one partition contributed to the frame.
#[derive(Clone, Copy, Debug)]
pub struct Draw {
    pub id: PartitionId,
    pub lod: Lod,
    pub points: u32,
    pub hidden: u32,
    pub reclassified: u32,
    pub freshness: Freshness,
    /// The edit mask was recomputed this frame because the effective edit set changed.
    /// The *upload* was not re-fetched, which is the finding NOTES.md opens with.
    pub remasked: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Presentation {
    Presented,
    /// Drawn into an offscreen target for the host to composite
    /// ([[RFC-0006:C-SURFACE]] 4).
    HandedToHost,
}

/// One frame's drawn output plus the work the host owes.
///
/// Note the absence: there is no field for interface chrome. Panels, palettes and
/// readouts are the host's to composite ([[RFC-0006:C-OVERLAY]] 2), and a `Frame` gives
/// it nowhere to put them — so the tempting shortcut of routing a depth-anchored label
/// through the chrome path has no path to route through.
#[derive(Clone, Debug)]
pub struct Frame {
    pub frame_no: u64,
    pub generation: u64,
    pub draws: Vec<Draw>,
    /// Present iff the host separately scheduled a pipeline and it finished
    /// ([[RFC-0006:C-RENDER]] 3).
    pub pipeline: Option<PipelineResultRef>,
    /// Visible partitions with nothing resident to stand in for them.
    pub holes: Vec<PartitionId>,
    pub effects: Vec<Effect>,
    pub presentation: Presentation,
}

/// What happened to a batch the host delivered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Delivery {
    Accepted { id: PartitionId, lod: Lod },
    /// The request had been cancelled or superseded, so the batch never becomes render
    /// state. This is the half of [[RFC-0006:C-RENDER]] 2 that makes the other half
    /// free: the host may let dispatched work run to completion, because a completion
    /// nobody is waiting for cannot land.
    DroppedStale { req: RequestId },
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub frames: u64,
    pub requests_issued: u64,
    pub cancels: u64,
    pub accepted: u64,
    pub dropped_stale: u64,
    pub evictions: u64,
    pub remasks: u64,
    /// Summed cost of recomputing edit masks, in point-edit applications. The quantity
    /// [[RFC-0007:C-INVALIDATION]] 3 and 5 are about.
    pub remask_cost: u64,
    pub mask_reuses: u64,
    pub draws_fresh: u64,
    pub draws_fallback: u64,
    pub holes: u64,
}

#[derive(Clone, Debug)]
struct Upload {
    token: UploadToken,
    points: u32,
    last_used: u64,
    /// The retained derived thing ([[RFC-0007:C-INVALIDATION]] 1).
    mask: Option<Mask>,
}

#[derive(Clone, Copy, Debug)]
struct InFlight {
    id: PartitionId,
    lod: Lod,
    issued: u64,
}

/// Everything the renderer keeps between frames ([[RFC-0007:C-INVALIDATION]] 1).
pub struct RenderState {
    target: Target,
    budget: Budget,
    /// Uploads keyed by level of detail alone — *not* by edit set. An edit does not
    /// invalidate an upload, only the mask derived from it, so keying by edit set would
    /// discard points that are still correct.
    resident: BTreeMap<PartitionId, BTreeMap<Lod, Upload>>,
    inflight: BTreeMap<RequestId, InFlight>,
    next_req: u64,
    stats: Stats,
}

impl RenderState {
    pub fn new(target: Target, budget: Budget) -> Self {
        Self {
            target,
            budget,
            resident: BTreeMap::new(),
            inflight: BTreeMap::new(),
            next_req: 1,
            stats: Stats::default(),
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn target(&self) -> Target {
        self.target
    }

    /// Switch between presenting and offscreen operation, which C-SURFACE 4 says must
    /// not change the obligations in (1) to (3). It changes one field and drops nothing:
    /// retained uploads survive, in-flight requests survive, masks survive.
    pub fn retarget(&mut self, target: Target) {
        self.target = target;
    }

    /// Tighten or loosen the residency budget. Mutates the budget and keeps the state:
    /// lowering it makes the next `advance` evict, which is [[RFC-0007:C-INVALIDATION]]
    /// 1's "bounded cache management" — as opposed to discarding render state, which is
    /// the rebuild the clause prohibits.
    pub fn set_budget(&mut self, budget: Budget) {
        self.budget = budget;
    }

    pub fn budget(&self) -> Budget {
        self.budget
    }

    pub fn resident_uploads(&self) -> usize {
        self.resident.values().map(|m| m.len()).sum()
    }

    pub fn resident_points(&self) -> u64 {
        self.resident
            .values()
            .flat_map(|m| m.values())
            .map(|u| u.points as u64)
            .sum()
    }

    pub fn inflight_len(&self) -> usize {
        self.inflight.len()
    }

    pub fn inflight_iter(&self) -> impl Iterator<Item = (RequestId, PartitionId, Lod, u64)> + '_ {
        self.inflight.iter().map(|(r, f)| (*r, f.id, f.lod, f.issued))
    }

    /// `(partition, lod, points, last used, has a mask)`.
    pub fn resident_iter(&self) -> impl Iterator<Item = (PartitionId, Lod, u32, u64, bool)> + '_ {
        self.resident.iter().flat_map(|(id, levels)| {
            levels
                .iter()
                .map(move |(lod, u)| (*id, *lod, u.points, u.last_used, u.mask.is_some()))
        })
    }

    /// One frame. Reads the snapshot and the buffers, mutates retained state, returns
    /// what was drawn and what the host owes. Nothing here can wait for anything.
    pub fn advance(&mut self, frame_no: u64, snap: &Snapshot, uploads: &dyn Uploads) -> Frame {
        self.stats.frames += 1;
        let mut effects = Vec::new();

        // 1. Cancellation sweep, before anything else — so a superseded request is out
        //    of `inflight` before the request pass decides whether to issue its
        //    replacement.
        let mut cancelled = Vec::new();
        for (req, f) in self.inflight.iter() {
            match snap.visible.iter().find(|v| v.id == f.id) {
                None => cancelled.push((*req, CancelReason::LeftView)),
                Some(v) if v.lod != f.lod => cancelled.push((*req, CancelReason::SupersededByLod)),
                Some(_) => {}
            }
        }
        for (req, reason) in cancelled {
            self.inflight.remove(&req);
            self.stats.cancels += 1;
            effects.push(Effect::Cancel { req, reason });
        }

        // 2. Draw what is resident, and ask for what is not.
        let mut draws = Vec::new();
        let mut holes = Vec::new();
        for v in &snap.visible {
            if let Some((lod, freshness)) = self.choose_upload(v.id, v.lod) {
                let upload = self
                    .resident
                    .get_mut(&v.id)
                    .and_then(|m| m.get_mut(&lod))
                    .expect("chosen upload is resident");
                upload.last_used = frame_no;
                let verts = uploads.vertices(upload.token);

                // [[RFC-0007:C-INVALIDATION]] 2 in its literal form: a partition whose
                // effective edit set has not changed since the previous extract is not
                // invalidated on account of an edit. One `u64` comparison decides it.
                let remasked = match &upload.mask {
                    Some(m) if m.digest == v.edits_digest => {
                        self.stats.mask_reuses += 1;
                        false
                    }
                    _ if v.edits.is_empty() => {
                        upload.mask = None;
                        false
                    }
                    _ => {
                        upload.mask = Some(build_mask(verts, &v.edits, v.edits_digest));
                        self.stats.remasks += 1;
                        self.stats.remask_cost += verts.len() as u64 * v.edits.len() as u64;
                        true
                    }
                };
                let (hidden, reclassified) = upload
                    .mask
                    .as_ref()
                    .map(|m| (m.hidden, m.reclassified))
                    .unwrap_or((0, 0));
                match freshness {
                    Freshness::Fresh => self.stats.draws_fresh += 1,
                    _ => self.stats.draws_fallback += 1,
                }
                draws.push(Draw {
                    id: v.id,
                    lod,
                    points: upload.points,
                    hidden,
                    reclassified,
                    freshness,
                    remasked,
                });
            } else {
                holes.push(v.id);
                self.stats.holes += 1;
            }

            let have_wanted = self
                .resident
                .get(&v.id)
                .map(|m| m.contains_key(&v.lod))
                .unwrap_or(false);
            let asking = self
                .inflight
                .values()
                .any(|f| f.id == v.id && f.lod == v.lod);
            if !have_wanted && !asking {
                let req = RequestId(self.next_req);
                self.next_req += 1;
                self.inflight.insert(
                    req,
                    InFlight {
                        id: v.id,
                        lod: v.lod,
                        issued: frame_no,
                    },
                );
                self.stats.requests_issued += 1;
                effects.push(Effect::Request {
                    req,
                    id: v.id,
                    lod: v.lod,
                });
            }
        }

        // 3. Eviction. [[RFC-0007:C-INVALIDATION]] 1's nested qualification: retention
        //    "prohibits rebuilding what is still current, not bounded cache management".
        while self.resident_uploads() > self.budget.max_uploads
            || self.resident_points() > self.budget.max_points
        {
            let victim = self
                .resident
                .iter()
                .flat_map(|(id, levels)| {
                    levels
                        .iter()
                        .map(move |(lod, u)| (*id, *lod, u.last_used, u.token))
                })
                .filter(|(id, lod, ..)| !draws.iter().any(|d| d.id == *id && d.lod == *lod))
                .min_by_key(|(_, _, last_used, _)| *last_used);
            let Some((id, lod, _, token)) = victim else {
                // Everything resident is on screen. Evicting it would blank the frame,
                // so the budget is exceeded and reported rather than enforced — a real
                // limit, and better seen than hidden.
                break;
            };
            if let Some(levels) = self.resident.get_mut(&id) {
                levels.remove(&lod);
                if levels.is_empty() {
                    self.resident.remove(&id);
                }
            }
            self.stats.evictions += 1;
            effects.push(Effect::Evict { id, lod, token });
        }

        Frame {
            frame_no,
            generation: snap.generation,
            draws,
            pipeline: snap.pipeline,
            holes,
            effects,
            presentation: match self.target {
                Target::Presenting(_) => Presentation::Presented,
                Target::Offscreen => Presentation::HandedToHost,
            },
        }
    }

    /// Retrieval the host dispatched has finished. The only door point data comes
    /// through, and it is not `advance` — which is what keeps extract free of retrieval
    /// ([[RFC-0007:C-EXTRACT]] 4).
    pub fn deliver(&mut self, req: RequestId, token: UploadToken, points: u32) -> Delivery {
        match self.inflight.remove(&req) {
            Some(f) => {
                self.resident.entry(f.id).or_default().insert(
                    f.lod,
                    Upload {
                        token,
                        points,
                        last_used: 0,
                        // Deliberately `None`: a fresh upload has no mask, so the next
                        // `advance` derives it from *that* frame's edit set rather than
                        // from whatever was current when the request went out.
                        mask: None,
                    },
                );
                self.stats.accepted += 1;
                Delivery::Accepted {
                    id: f.id,
                    lod: f.lod,
                }
            }
            None => {
                self.stats.dropped_stale += 1;
                Delivery::DroppedStale { req }
            }
        }
    }

    /// Nearest coarser resident level, else nearest finer, else nothing.
    fn choose_upload(&self, id: PartitionId, want: Lod) -> Option<(Lod, Freshness)> {
        let levels = self.resident.get(&id)?;
        if levels.contains_key(&want) {
            return Some((want, Freshness::Fresh));
        }
        if let Some(lod) = levels.keys().filter(|l| **l < want).max() {
            return Some((*lod, Freshness::CoarserFallback { held: *lod }));
        }
        let lod = levels.keys().filter(|l| **l > want).min()?;
        Some((*lod, Freshness::FinerFallback { held: *lod }))
    }

    /// Cost, in point-edit applications, of bringing every visible partition's mask up
    /// to date — without doing it.
    ///
    /// Exists for [[RFC-0007:C-INVALIDATION]] 5, which requires the bound in (3) to be
    /// *measured* across edit stacks differing by an order of magnitude with the
    /// intersecting count held constant. A claim no test is obliged to make is not a
    /// claim.
    pub fn pending_mask_cost(&self, snap: &Snapshot) -> u64 {
        let mut cost = 0;
        for v in &snap.visible {
            let stale = self
                .resident
                .get(&v.id)
                .and_then(|m| m.get(&v.lod))
                .map(|u| u.mask.as_ref().map(|m| m.digest != v.edits_digest).unwrap_or(true))
                .unwrap_or(false);
            if stale {
                cost += v.point_count as u64 * v.edits.len() as u64;
            }
        }
        cost
    }

    /// The retained edit mask for one upload: one entry per vertex, `KEEP`, `HIDE`, or
    /// `RECLASS + class`.
    ///
    /// Exposed because a device backend needs it. The mask is computed here — it is policy,
    /// derived from the gestures the snapshot carried — but *applying* it to a device buffer
    /// is an upload, and uploads are the host's ([[RFC-0004:C-HOST]]). So the host reads
    /// this and rewrites the attribute buffer when `Draw::remasked` says it changed.
    ///
    /// Worth naming as a cost: an edit therefore costs a re-upload of one partition's
    /// attribute buffer. It still costs no *retrieval*, which is the finding that mattered,
    /// but "free" would be too strong.
    pub fn mask_flags(&self, id: PartitionId, lod: Lod) -> Option<&[u8]> {
        self.resident
            .get(&id)?
            .get(&lod)?
            .mask
            .as_ref()
            .map(|m| m.flags.as_slice())
    }

    /// The device handle for an upload, so the host can find the buffer it made.
    pub fn token_of(&self, id: PartitionId, lod: Lod) -> Option<UploadToken> {
        Some(self.resident.get(&id)?.get(&lod)?.token)
    }

    pub fn digest_of(&self, id: PartitionId, lod: Lod) -> Option<EditDigest> {
        self.resident
            .get(&id)?
            .get(&lod)?
            .mask
            .as_ref()
            .map(|m| m.digest)
    }
}
