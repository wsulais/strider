// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The host: opening the source, the frame loop, and performing the renderer's effects.

use std::sync::Arc;

use strider_view::{
    Budget, CancelReason, Delivery, Effect, Frame, Lod, PartitionId, PipelineResultRef, RenderState,
    SurfaceHandle, Target,
};
use strider_io::copc::{Open, Source};
use strider_io::{Delivered, Step};

use strider_doc::doc::{node_id, Camera, Document, Edit};
use crate::retrieval::Retrieval;
use strider_doc::store::Store;

pub struct Host {
    pub path: String,
    pub source: Source,
    /// Shared with worker threads so `source` stays mutable and pageable.
    pub decoder: strider_io::copc::Decoder,
    pub doc: Document,
    pub store: Store,
    pub retrieval: Retrieval,
    pub renderer: RenderState,
    pub cam: Camera,
    pub frame_no: u64,
    pub level: Lod,
    /// Levels the source actually has, coarsest and finest.
    pub level_range: (u8, u8),
    /// A pipeline run the host scheduled separately ([[RFC-0006:C-RENDER]] 3).
    pub pipeline: Option<(u64, PipelineResultRef)>,
    pub pipeline_ready: Option<PipelineResultRef>,
    pub last: Option<Frame>,
    pub log: Vec<String>,
    pub index_pages_fetched: usize,
    /// Pages read during the most recent frame. Blocking reads in the frame path are a stall,
    /// and a stall that only happens while zooming is easy to mistake for rendering cost.
    pub pages_this_frame: usize,
    pub open_rounds: usize,
    pub open_bytes: u64,
    /// Ramp ranges, established once at open and stable thereafter. See `RampStats`.
    pub ramp: strider_doc::doc::RampStats,
}

impl Host {
    /// Open a COPC source by driving `strider_io::copc::Open` to completion.
    ///
    /// The loop is the whole demonstration of [[RFC-0004:C-HOST]] 2 and 4: the library
    /// says which ranges it wants, the host reads them, and nothing in the library ever
    /// touches the file or waits on it.
    pub fn open(path: &str, cols: u16, rows: u16) -> std::io::Result<Self> {
        let mut open = Open::new();
        let mut delivered: Vec<(strider_io::Range, Vec<u8>)> = Vec::new();
        let mut rounds = 0usize;
        let mut bytes = 0u64;
        let source = loop {
            let borrowed: Vec<Delivered<'_>> = delivered
                .iter()
                .map(|(r, b)| Delivered {
                    range: *r,
                    bytes: b.as_slice(),
                })
                .collect();
            let step = open.step(&borrowed).map_err(std::io::Error::other)?;
            match step {
                Step::Need(need) => {
                    rounds += 1;
                    bytes += need.bytes();
                    let mut next = Vec::new();
                    for r in &need.ranges {
                        next.push((*r, Retrieval::read_blocking(path, r.offset, r.len)?));
                    }
                    delivered = next;
                }
                Step::Ready(s) => break s,
            }
        };

        let levels: Vec<u8> = source.hierarchy().nodes().iter().map(|n| n.level()).collect();
        let level_range = (
            levels.iter().copied().min().unwrap_or(0),
            levels.iter().copied().max().unwrap_or(0),
        );

        let b = source.header().bounds;
        let origin = b.min;
        let cam = Camera {
            centre: [
                ((b.min[0] + b.max[0]) / 2.0 - origin[0]) as f32,
                ((b.min[1] + b.max[1]) / 2.0 - origin[1]) as f32,
            ],
            // An eighth of the extent, not the whole of it. The whole extent resolves
            // to the COPC root node alone, which is one partition and shows nothing
            // about fallback, eviction or cancellation.
            width: (b.max[0] - b.min[0]) as f32 / 8.0,
            cols,
            rows,
        };
        let doc = Document::new(&source);
        let level = Lod(level_range.0);

        // Ramp ranges from the coarsest node: a decimated sample of the whole cloud, one
        // read, and independent of where the camera is. See `RampStats` for why neither the
        // resident set nor the type domain will do.
        let ramp = source
            .hierarchy()
            .nodes()
            .iter()
            .min_by_key(|n| n.level())
            .copied()
            .and_then(|node| {
                let bytes = Retrieval::read_blocking(path, node.chunk.offset, node.chunk.len).ok()?;
                let batch = source.decode(&node, &bytes).ok()?;
                Some(strider_doc::doc::RampStats::from_sample(
                    &strider_doc::doc::to_vertices(&batch, origin),
                    "COPC root node, a decimated sample of the whole cloud",
                ))
            })
            .unwrap_or(strider_doc::doc::RampStats {
                channels: [(0.0, 1.0); strider_view::CHANNELS],
                source_has_colour: false,
                provenance: "unavailable",
            });

        Ok(Self {
            path: path.to_string(),
            decoder: source.decoder(),
            source,
            doc,
            store: Store::default(),
            retrieval: Retrieval::new(path),
            renderer: RenderState::new(
                // A stand-in for the handle a `QWindow` yields. The renderer cannot
                // read it, so a real one changes nothing above this line.
                Target::Presenting(SurfaceHandle::from_host(0x5157_494E)),
                Budget {
                    max_uploads: 48,
                    max_points: 6_000_000,
                },
            ),
            cam,
            frame_no: 0,
            level,
            level_range,
            pipeline: None,
            pipeline_ready: None,
            last: None,
            log: Vec::new(),
            index_pages_fetched: 0,
            pages_this_frame: 0,
            open_rounds: rounds,
            open_bytes: bytes,
            ramp,
        })
    }

    /// Level of detail for the camera, chosen from the index alone.
    pub fn level_for_camera(&self) -> Lod {
        let want = self.source.level_for_spacing(self.cam.ground_resolution());
        Lod(want.clamp(self.level_range.0, self.level_range.1))
    }

    /// One turn of the host's frame loop.
    ///
    /// The order is a finding rather than an incidental. Deliveries land *before*
    /// extract; index paging happens *before* extract and outside it; extract is one
    /// call; effects are performed *after* the frame is drawn. Any other order either
    /// puts retrieval inside the synchronisation point ([[RFC-0007:C-EXTRACT]] 4) or
    /// makes the renderer wait for the host ([[RFC-0004:C-HOST]] 4).
    pub fn tick(&mut self) {
        self.frame_no += 1;
        let f = self.frame_no;

        // 1. Uploads for retrieval that completed. Work whose cancellation was requested
        //    still completes, and the renderer drops it.
        for c in self.retrieval.drain() {
            let n = c.verts.len() as u32;
            let token = self.store.upload(c.verts);
            match self.renderer.deliver(c.req, token, n) {
                Delivery::Accepted { id, lod } => {
                    self.log.push(format!(
                        "  deliver r{} -> accepted p{} l{} ({} pts, {:.1} ms)",
                        c.req.0,
                        id.0,
                        lod.0,
                        n,
                        c.cost_us as f64 / 1000.0
                    ));
                }
                Delivery::DroppedStale { req } => {
                    // The upload is freed at once: the renderer never held the token,
                    // so nothing else can be holding it.
                    self.store.free(token);
                    self.log.push(format!(
                        "  deliver r{} -> DROPPED, {:.1} ms of read+decode discarded{}",
                        req.0,
                        c.cost_us as f64 / 1000.0,
                        if c.was_cancelled {
                            " (cancel had been requested)"
                        } else {
                            ""
                        }
                    ));
                }
            }
        }

        // 2. Page the index if the camera has moved somewhere it has not been read.
        //    Outside extract, because paging is retrieval.
        self.page_index();

        // 3. Extract. One call, metadata only, no await.
        self.level = self.level_for_camera();
        let snap = self
            .doc
            .extract(&self.source, &self.cam, self.level_range.1, self.ramp.channels[0], self.pipeline_ready);

        // 4. Draw.
        let frame = self.renderer.advance(f, &snap, &self.store);

        // 5. Perform the effects.
        for e in frame.effects.clone() {
            match e {
                Effect::Request { req, id, lod } => {
                    if let Some(node) = self
                        .source
                        .hierarchy()
                        .nodes()
                        .iter()
                        .find(|n| PartitionId(node_id(n)) == id && n.level() == lod.0)
                        .copied()
                    {
                        self.retrieval.dispatch(
                            req,
                            id,
                            lod,
                            node,
                            self.decoder.clone(),
                            self.doc.origin,
                        );
                    }
                }
                Effect::Cancel { req, reason } => {
                    self.retrieval.note_cancel(req);
                    self.log.push(format!(
                        "  cancel  r{} ({})",
                        req.0,
                        match reason {
                            CancelReason::LeftView => "left view",
                            CancelReason::SupersededByLod => "superseded by lod",
                        }
                    ));
                }
                Effect::Evict { token, .. } => self.store.free(token),
            }
        }

        // 6. A pipeline the host scheduled may have finished.
        if let Some((ready_at, r)) = self.pipeline {
            if ready_at <= f {
                self.pipeline_ready = Some(r);
                self.pipeline = None;
                self.log
                    .push(format!("  pipeline #{} finished, now displayable", r.id));
            }
        }

        self.last = Some(frame);
    }

    /// Read deeper hierarchy pages the current view needs.
    ///
    /// Blocking, and deliberately so: it is *not* in the frame path in a real build —
    /// the index is a cache the host warms. Doing it here, before extract, keeps the
    /// prototype honest about where the cost falls without pretending extract could have
    /// done it.
    fn page_index(&mut self) {
        self.pages_this_frame = 0;
        let view = self.cam.view();
        let query = strider_io::Aabb {
            min: [
                view.min[0] as f64 + self.doc.origin[0],
                view.min[1] as f64 + self.doc.origin[1],
                f64::MIN,
            ],
            max: [
                view.max[0] as f64 + self.doc.origin[0],
                view.max[1] as f64 + self.doc.origin[1],
                f64::MAX,
            ],
        };
        let want = self.level_for_camera().0;
        for _ in 0..6 {
            let Step::Need(need) = self.source.resolve(&query, want) else {
                return;
            };
            let mut bufs = Vec::new();
            for r in &need.ranges {
                match Retrieval::read_blocking(&self.path, r.offset, r.len) {
                    Ok(b) => bufs.push((*r, b)),
                    Err(_) => return,
                }
            }
            let borrowed: Vec<Delivered<'_>> = bufs
                .iter()
                .map(|(r, b)| Delivered {
                    range: *r,
                    bytes: b.as_slice(),
                })
                .collect();
            // A plain mutable borrow now: workers hold a `Decoder`, not the source, so
            // paging is never blocked by a read in flight. It used to be, and the symptom was
            // a viewport with holes — the index simply did not know those nodes existed.
            if self.source.absorb_pages(&borrowed).is_err() {
                return;
            }
            self.index_pages_fetched += borrowed.len();
            // Paging added nodes the edit index has never seen.
            self.doc.reindex_from_nodes();
        }
    }

    /// Run `n` frames at roughly 60 Hz.
    ///
    /// The pacing is the host's business, not the renderer's
    /// ([[RFC-0006:C-RENDER]] 4) — and it is load-bearing for the prototype rather than
    /// cosmetic. An unpaced loop runs twelve frames in under a millisecond, so a real
    /// read that takes 8 ms never lands and the viewport stays empty. That is not a bug
    /// in the renderer; it is the frame budget being real.
    pub fn ticks(&mut self, n: u64) {
        for _ in 0..n {
            self.log.clear();
            self.tick();
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }

    /// A camera drag: pan a little and draw, `n` times, with no pause.
    ///
    /// This is the condition [[RFC-0006:C-RENDER]] 2 exists for and the one a
    /// keystroke-at-a-time session never reaches — "camera motion invalidates in-flight
    /// requests faster than they complete". One keypress per frame lets every read
    /// finish; a drag does not.
    pub fn drag(&mut self, dx: f32, dy: f32, n: u64) {
        for _ in 0..n {
            self.log.clear();
            // 40% of the view per frame: a fling, not a nudge. Panning by less than
            // a node's width never drops a partition from view, so a gentle drag
            // cancels nothing however long it runs.
            self.cam.centre[0] += dx * self.cam.width * 0.4;
            self.cam.centre[1] += dy * self.cam.width * 0.4;
            self.tick();
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }

    /// A continuous zoom, one step per frame — a scroll wheel held down.
    ///
    /// This is the gesture that produces `SupersededByLod`: crossing a level boundary
    /// invalidates every in-flight request at the level being left, and at level 3 and
    /// above a node takes 20 to 40 ms to read and decode, so those requests are still
    /// in flight when it happens.
    pub fn zoom_sweep(&mut self, factor: f32, n: u64) {
        for _ in 0..n {
            self.log.clear();
            self.zoom(factor);
            self.tick();
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }

    pub fn pan(&mut self, dx: f32, dy: f32) {
        let step = self.cam.width * 0.25;
        self.cam.centre[0] += dx * step;
        self.cam.centre[1] += dy * step;
    }

    pub fn zoom(&mut self, factor: f32) {
        let b = self.source.header().bounds;
        let full = (b.max[0] - b.min[0]) as f32;
        self.cam.width = (self.cam.width * factor).clamp(2.0, full * 1.5);
    }

    pub fn push_edit(&mut self, e: Edit) {
        self.doc.append(e, &self.source);
    }

    pub fn undo(&mut self) -> bool {
        let popped = self.doc.edits.pop().is_some();
        if popped {
            // A rebuild, not an incremental removal: popping renumbers nothing here,
            // but a reorder does, and having one path for both keeps the stack's order
            // and the index's indices from drifting apart.
            self.doc.reindex(&self.source);
        }
        popped
    }

    /// Swap two edits. [[RFC-0007:C-EDIT]] 4 says this MUST be treated as changing the
    /// result unless a stated independence test is applied and recorded. No such test
    /// exists here, so the digests change and the masks are rebuilt.
    pub fn reorder(&mut self, i: usize, j: usize) -> bool {
        if i >= self.doc.edits.len() || j >= self.doc.edits.len() || i == j {
            return false;
        }
        self.doc.edits.swap(i, j);
        self.doc.reindex(&self.source);
        true
    }

    pub fn schedule_pipeline(&mut self) {
        let id = 1 + self.pipeline_ready.map(|r| r.id).unwrap_or(0);
        self.pipeline = Some((
            self.frame_no + 8,
            PipelineResultRef {
                id,
                points: 91_000,
            },
        ));
    }

    /// The camera's box, in local metres — what an edit or an anchor is placed against.
    pub fn view_box(&self) -> ([f32; 2], [f32; 2]) {
        let v = self.cam.view();
        (v.min, v.max)
    }
}
