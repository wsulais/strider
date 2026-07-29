// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The document graph, the spatial index over its edit stack, the camera, and extract.
//!
//! The structural claim this module makes: **extract cannot read point data**, because
//! `extract` takes `&Document` and `&Source` and `Source` holds only the COPC index —
//! node keys, cubes, point counts, chunk offsets. The points live in `store::Store`,
//! which extract has no parameter for. [[RFC-0007:C-EXTRACT]] 4 becomes a fact about
//! the signature rather than a rule somebody has to remember.
//!
//! It makes a second claim that only appeared once the source was real: **hierarchy
//! paging cannot happen inside extract either.** A 13 GB COPC's index is itself paged,
//! so resolving a query can require reading a deeper page — which is retrieval, which
//! C-EXTRACT 4 forbids at the synchronisation point. Paging is therefore a host
//! activity outside extract, and extract answers from whatever index has arrived. See
//! NOTES.md.

use std::collections::BTreeMap;

use arrow::array::{Array, Float64Array, StructArray, UInt16Array, UInt8Array};
use arrow::record_batch::RecordBatch;
use strider_view::{
    Anchor, EditAction, EditDigest, EditRef, Lod, PartitionId, PipelineResultRef, Snapshot, Vertex,
    View, VisiblePartition,
};
use strider_io::copc::{Node, Source, VoxelKey};
use strider_io::Aabb;

/// One recorded gesture ([[RFC-0007:C-EDIT]] 2). Its stored size is independent of the
/// number of points it affects, which is the property the whole edit model rests on.
#[derive(Clone, Copy, Debug)]
pub struct Edit {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub only_class: Option<u8>,
    pub action: EditAction,
}

impl Edit {
    fn intersects_bounds(&self, b: &Aabb, origin: [f64; 3]) -> bool {
        let nmin = [(b.min[0] - origin[0]) as f32, (b.min[1] - origin[1]) as f32];
        let nmax = [(b.max[0] - origin[0]) as f32, (b.max[1] - origin[1]) as f32];
        self.min[0] <= nmax[0]
            && self.max[0] >= nmin[0]
            && self.min[1] <= nmax[1]
            && self.max[1] >= nmin[1]
    }
}

/// The edit stack, indexed against the **source's own octree**.
///
/// The first attempt here was a uniform bucket grid over the extent, and measuring it
/// killed it. [[RFC-0007:C-INVALIDATION]] 3 bounds determining a partition's effective
/// edit set "by a function of the edits intersecting that partition". A grid query
/// visits a cell count set by the *partition's* size, so resolving the COPC root node —
/// which covers the whole extent — visited 9 216 buckets to find **zero** edits. That
/// is bounded by the wrong quantity, and it is worst exactly where a viewport starts:
/// zoomed out, at the coarsest level.
///
/// Indexing against the octree instead makes a lookup one map probe, because the thing
/// being asked about *is* a node. [[ADR-0005]] said so already — "it composes with the
/// index rather than fighting it" — and the grid was fighting it.
#[derive(Default)]
pub struct EditIndex {
    by_node: BTreeMap<VoxelKey, Vec<u32>>,
    /// Keys the index has seen, so insertion can descend the octree instead of scanning
    /// it. Without this, inserting one edit walks every node the hierarchy knows —
    /// bounded by the *index's* size, which is not what
    /// [[RFC-0007:C-INVALIDATION]] 4 says. With it, a subtree that does not meet the
    /// edit is pruned whole, so the walk is bounded by the edit's own extent.
    present: std::collections::BTreeSet<VoxelKey>,
    /// Bounds of the octree root, needed to derive a node's cube while descending.
    root: Option<Aabb>,
    max_level: u8,
    /// Lookups performed, and entries those lookups walked. The pair
    /// [[RFC-0007:C-INVALIDATION]] 3 is about: the second must track the edits
    /// intersecting the partition and nothing else.
    pub probes: u64,
    pub entries_visited: u64,
    /// Nodes touched while inserting edits — the quantity C-INVALIDATION 4 bounds.
    pub nodes_touched: u64,
    pub inserts: u64,
}

impl EditIndex {
    /// Note which nodes exist, so insertion can prune.
    fn observe(&mut self, nodes: &[Node], root: Aabb) {
        self.root = Some(root);
        for n in nodes {
            self.present.insert(n.key);
            self.max_level = self.max_level.max(n.level());
        }
    }

    /// Insert one edit by descending the octree, pruning subtrees it does not meet.
    ///
    /// Touches only nodes its own region meets, plus the pruned siblings along the way,
    /// and reads no other edit ([[RFC-0007:C-INVALIDATION]] 4). The first version of
    /// this scanned the whole node list, which is bounded by the index rather than by
    /// the edit — and grows as hierarchy paging loads more of the index, so it got worse
    /// the longer a session ran.
    fn insert(&mut self, i: u32, e: &Edit, origin: [f64; 3], max_level: u8) {
        self.inserts += 1;
        let Some(root) = self.root else { return };
        let mut stack = vec![VoxelKey { level: 0, x: 0, y: 0, z: 0 }];
        while let Some(key) = stack.pop() {
            self.nodes_touched += 1;
            let b = key.bounds(&root);
            if !e.intersects_bounds(&b, origin) {
                continue; // prunes this whole subtree
            }
            if self.present.contains(&key) {
                self.by_node.entry(key).or_default().push(i);
            }
            if key.level as u8 >= max_level {
                continue;
            }
            for dx in 0..2 {
                for dy in 0..2 {
                    for dz in 0..2 {
                        let child = VoxelKey {
                            level: key.level + 1,
                            x: (key.x << 1) | dx,
                            y: (key.y << 1) | dy,
                            z: (key.z << 1) | dz,
                        };
                        stack.push(child);
                    }
                }
            }
        }
    }

    /// Rebuild from scratch. Needed on undo and on reorder, which renumber the stack,
    /// and when hierarchy paging adds nodes the index has never seen.
    ///
    /// Deliberately *not* what a per-partition query calls. C-INVALIDATION 3 bounds
    /// determining an effective edit set, and this is index maintenance — a distinction
    /// worth keeping visible, because collapsing the two is how a naive edit log ends up
    /// replaying the whole stack per partition read.
    fn rebuild(&mut self, edits: &[Edit], nodes: &[Node], root: Aabb, origin: [f64; 3]) {
        self.by_node.clear();
        self.observe(nodes, root);
        let max_level = nodes.iter().map(|n| n.level()).max().unwrap_or(0);
        for (i, e) in edits.iter().enumerate() {
            self.insert(i as u32, e, origin, max_level);
        }
    }

    /// A partition's candidate edits, in stack order. One map probe.
    fn for_node(&mut self, key: VoxelKey) -> &[u32] {
        self.probes += 1;
        let found = self.by_node.get(&key).map(|v| v.as_slice()).unwrap_or(&[]);
        self.entries_visited += found.len() as u64;
        found
    }
}

pub struct Camera {
    /// Local metres from the source origin.
    pub centre: [f32; 2],
    pub width: f32,
    pub cols: u16,
    pub rows: u16,
}

impl Camera {
    /// Height follows from the aspect. Terminal cells are about twice as tall as wide,
    /// so the plan is not squashed.
    pub fn height(&self) -> f32 {
        self.width * (self.rows as f32 * 2.0) / self.cols as f32
    }

    pub fn view(&self) -> View {
        let h = self.height();
        View {
            min: [self.centre[0] - self.width / 2.0, self.centre[1] - h / 2.0],
            max: [self.centre[0] + self.width / 2.0, self.centre[1] + h / 2.0],
            cols: self.cols,
            rows: self.rows,
        }
    }

    /// The metres per screen cell — what a level of detail is chosen against.
    pub fn ground_resolution(&self) -> f64 {
        (self.width / self.cols as f32) as f64
    }

    /// Where the eye is, for the per-node distance the level-of-detail rule needs.
    ///
    /// Mirrors `render_gpu::Orbit::framing` rather than importing it, because the host must
    /// not depend on the device layer to decide what to *load* — that would make residency a
    /// function of the graphics backend.
    pub fn eye_for(&self, z_anchor: (f32, f32)) -> [f32; 3] {
        let span = self.width.max(self.height());
        let distance = span * 1.45;
        let (yaw, pitch) = (0.72f32, 0.55f32);
        [
            self.centre[0] + distance * pitch.cos() * yaw.cos(),
            self.centre[1] + distance * pitch.cos() * yaw.sin(),
            z_anchor.0 + (z_anchor.1 - z_anchor.0) * 0.25 + distance * pitch.sin(),
        ]
    }
}

/// What one extract cost and what it selected.
///
/// Kept because "points drawn" turned out to be the wrong number to watch: zooming in reduces
/// it while making the frame *more* expensive, because descent visits more octree cells and
/// selects more partitions. Cost lives in the traversal and in the draw count, not in the
/// point total.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExtractStats {
    /// Octree cells the traversal touched, including those pruned.
    pub keys_visited: u32,
    /// Cells that existed in the index and were selected.
    pub selected: u32,
    /// Deepest level actually selected, and the shallowest.
    pub deepest: u8,
    pub shallowest: u8,
    /// How many partitions came from each level, indexed by level.
    pub per_level: [u16; 24],
}

impl ExtractStats {
    /// A compact histogram, skipping levels that contributed nothing.
    pub fn histogram(&self) -> String {
        self.per_level
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(l, n)| format!("L{l}:{n}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub struct Document {
    /// Ordered, not a set ([[RFC-0007:C-EDIT]] 4).
    pub edits: Vec<Edit>,
    pub index: EditIndex,
    pub anchors: Vec<Anchor>,
    pub generation: u64,
    /// Source coordinates minus this give the local `f32` the renderer uses.
    pub origin: [f64; 3],
    /// What the last extract cost and selected.
    pub stats: ExtractStats,
}

impl Document {
    pub fn new(source: &Source) -> Self {
        let b = source.header().bounds;
        let origin = b.min;
        let mid = [
            ((b.min[0] + b.max[0]) / 2.0 - origin[0]) as f32,
            ((b.min[1] + b.max[1]) / 2.0 - origin[1]) as f32,
        ];
        Self {
            edits: Vec::new(),
            index: EditIndex::default(),
            // Seeded low in the extent, so at least one starts under whatever is above
            // it — the case [[RFC-0006:C-OVERLAY]] 1's rationale is about.
            anchors: vec![
                Anchor {
                    label: "dbh 0.42m".into(),
                    x: mid[0],
                    y: mid[1],
                    z: (b.min[2] - origin[2]) as f32 + 0.5,
                },
                Anchor {
                    label: "crown top".into(),
                    x: mid[0] + 12.0,
                    y: mid[1] + 8.0,
                    z: (b.max[2] - origin[2]) as f32 - 0.5,
                },
            ],
            generation: 0,
            origin,
            stats: ExtractStats::default(),
        }
    }

    /// Re-index using the nodes already observed. Used after paging, where the caller holds a
    /// mutable borrow of the source and cannot lend it out again.
    pub fn reindex_from_nodes(&mut self) {
        let edits = self.edits.clone();
        let origin = self.origin;
        let (Some(root), max_level) = (self.index.root, self.index.max_level) else {
            return;
        };
        self.index.by_node.clear();
        for (i, e) in edits.iter().enumerate() {
            self.index.insert(i as u32, e, origin, max_level);
        }
        let _ = root;
    }

    /// Re-index after a change that renumbers the stack, or after paging added nodes.
    pub fn reindex(&mut self, source: &Source) {
        let edits = self.edits.clone();
        let origin = self.origin;
        self.index
            .rebuild(&edits, source.hierarchy().nodes(), *source.root_bounds(), origin);
    }

    /// Append one gesture, touching only the nodes it meets
    /// ([[RFC-0007:C-INVALIDATION]] 4).
    pub fn append(&mut self, e: Edit, source: &Source) {
        let i = self.edits.len() as u32;
        let origin = self.origin;
        let nodes = source.hierarchy().nodes();
        let max_level = nodes.iter().map(|n| n.level()).max().unwrap_or(0);
        self.index.observe(nodes, *source.root_bounds());
        self.edits.push(e);
        self.index.insert(i, &e, origin, max_level);
    }

    /// The one synchronisation point ([[RFC-0007:C-EXTRACT]] 3).
    ///
    /// Metadata only, and provably: the parameters are the document and the *index*.
    /// There is no `Store`, so no point can be read; there is no `Retrieval`, so nothing
    /// can be fetched or awaited ([[RFC-0007:C-EXTRACT]] 4).
    #[allow(clippy::too_many_arguments)]
    pub fn extract(
        &mut self,
        source: &Source,
        cam: &Camera,
        max_level: u8,
        level_anchor: (f32, f32),
        pipeline: Option<PipelineResultRef>,
    ) -> Snapshot {
        self.generation += 1;
        let view = cam.view();
        let query = Aabb {
            min: [
                view.min[0] as f64 + self.origin[0],
                view.min[1] as f64 + self.origin[1],
                f64::MIN,
            ],
            max: [
                view.max[0] as f64 + self.origin[0],
                view.max[1] as f64 + self.origin[1],
                f64::MAX,
            ],
        };

        // Hierarchical level of detail, and the union of levels rather than one level.
        //
        // This replaces a single-level selection that was simply wrong about COPC. An octree
        // in the EPT sense PARTITIONS its points: each point appears exactly once, at the
        // level where the spacing criterion places it. So the points at level 6 are the ones
        // that *first appear* at level 6 — a sparse shell — and full density for a region is
        // the union of every level from the root down. Drawing one level rendered a cloud
        // with holes in it and a point count far below what the region holds.
        //
        // Descent is per node and driven by distance, which is what makes this hierarchical:
        // a node near the eye is refined until its point spacing is finer than the screen can
        // show, and a node twice as far stops one level earlier because its spacing subtends
        // half the angle.
        let eye = cam.eye_for(level_anchor);
        let target_spacing = cam.ground_resolution();
        let mut stats = ExtractStats {
            shallowest: u8::MAX,
            ..Default::default()
        };
        let mut visible = Vec::new();
        let mut stack = vec![VoxelKey {
            level: 0,
            x: 0,
            y: 0,
            z: 0,
        }];
        let root_bounds = *source.root_bounds();
        let mut deepest = 0u8;
        while let Some(key) = stack.pop() {
            stats.keys_visited += 1;
            let cube = key.bounds(&root_bounds);
            if !cube.intersects_xy(&query) {
                continue;
            }
            // A node the index has never heard of cannot be drawn, and its subtree is
            // unreachable until a hierarchy page arrives. Not an error: the host pages the
            // index outside extract, because paging is retrieval.
            if let Some(node) = source.node(key) {
                deepest = deepest.max(node.level());
                stats.selected += 1;
                stats.deepest = stats.deepest.max(node.level());
                stats.shallowest = stats.shallowest.min(node.level());
                if let Some(slot) = stats.per_level.get_mut(node.level() as usize) {
                    *slot = slot.saturating_add(1);
                }
                visible.push(self.visible_partition(node, view));
            }

            // Refine while this node's spacing is coarser than the screen needs at its own
            // distance. `spacing_at` is the source's own statement about level resolution, so
            // no measurement of the points is required to decide.
            let spacing = source.info().spacing_at(key.level as u8);
            let centre = [
                (cube.min[0] + cube.max[0]) * 0.5 - self.origin[0],
                (cube.min[1] + cube.max[1]) * 0.5 - self.origin[1],
            ];
            let dx = centre[0] as f32 - eye[0];
            let dy = centre[1] as f32 - eye[1];
            let distance = (dx * dx + dy * dy).sqrt().max(1.0);
            let reference = (eye[2].abs()).max(1.0);
            // Farther nodes tolerate coarser spacing, in proportion to distance.
            let admissible = target_spacing as f32 * (distance / reference).max(1.0);
            if (spacing as f32) > admissible && (key.level as u8) < max_level {
                // The children the index recorded, not the eight a key could have.
                //
                // Pushing all eight and asking about each made the traversal cost a function
                // of the octree's shape rather than of the file's content: 2025 cells visited
                // to find 14 nodes, at 30x zoom on block1.copc.laz. An empty key still has a
                // slot, so descent passes through it; an unread page has one too, so descent
                // stops there and the host reads the page instead of concluding it is empty.
                stack.extend(source.children(key));
            }
        }

        if stats.shallowest == u8::MAX {
            stats.shallowest = 0;
        }
        self.stats = stats;

        Snapshot {
            generation: self.generation,
            view,
            visible,
            anchors: self.anchors.clone(),
            pipeline,
        }
    }
}

impl Document {
    /// One visible partition, with its effective edit set.
    fn visible_partition(&mut self, node: &Node, view: View) -> VisiblePartition {
        let min = [
            (node.bounds.min[0] - self.origin[0]) as f32,
            (node.bounds.min[1] - self.origin[1]) as f32,
        ];
        let max = [
            (node.bounds.max[0] - self.origin[0]) as f32,
            (node.bounds.max[1] - self.origin[1]) as f32,
        ];
        let candidates: Vec<u32> = self.index.for_node(node.key).to_vec();
        let mut edits: Vec<EditRef> = candidates
            .into_iter()
            .map(|i| {
                let e = self.edits[i as usize];
                EditRef {
                    order: i,
                    min: e.min,
                    max: e.max,
                    only_class: e.only_class,
                    action: e.action,
                }
            })
            .collect();
        edits.sort_by_key(|e| e.order);
        let _ = view;
        VisiblePartition {
            id: PartitionId(node_id(node)),
            // The node's OWN level, not one level for the whole frame. Two partitions in one
            // snapshot may now differ, which is what hierarchical level of detail means and
            // what the renderer's per-partition `lod` was always for.
            lod: Lod(node.level()),
            edits_digest: digest(&edits),
            edits,
            min,
            max,
            point_count: node.point_count,
        }
    }
}

/// A stable `u32` for a COPC node key, so the renderer can hold an opaque partition
/// identity without knowing what an octree is.
pub fn node_id(n: &strider_io::copc::Node) -> u32 {
    let k = n.key;
    // Levels reach about 12 and coordinates about 4096 at that depth; this packs both
    // without collision for anything COPC produces.
    ((k.level as u32) << 28) | (((k.x as u32) & 0x3ff) << 18) | (((k.y as u32) & 0x3ff) << 8)
        | ((k.z as u32) & 0xff)
}

/// Order-sensitive digest of an effective edit set.
///
/// FNV-1a over `(order, region, predicate, action)` in stack order, with the order index
/// folded in explicitly. [[RFC-0007:C-EDIT]] 4 requires reordering to be treated as
/// changing the result, and a digest that ignored order would silently make it
/// result-preserving — which is the sort of thing that is correct in every test written
/// against a stack of one.
pub fn digest(refs: &[EditRef]) -> EditDigest {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |b: u64| {
        h ^= b;
        h = h.wrapping_mul(0x100_0000_01b3);
    };
    for (position, r) in refs.iter().enumerate() {
        feed(position as u64);
        feed(r.order as u64);
        for v in [r.min[0], r.min[1], r.max[0], r.max[1]] {
            feed(v.to_bits() as u64);
        }
        feed(match r.only_class {
            None => 0xff00,
            Some(c) => c as u64,
        });
        feed(match r.action {
            EditAction::Delete => 0xd1,
            EditAction::Classify { class } => 0xc000 | class as u64,
        });
    }
    EditDigest(h)
}

/// One Arrow batch to the vertex format the renderer rasterises.
///
/// This is the only place the two representations meet. `f64` source coordinates become
/// `f32` local ones once, at upload — the conversion a graphics pipeline needs, done by
/// the host because choosing an origin is a host decision.
pub fn to_vertices(batch: &RecordBatch, origin: [f64; 3]) -> Vec<Vertex> {
    let Some(position) = batch
        .column_by_name(strider_io::copc::batch::POSITION)
        .and_then(|c| c.as_any().downcast_ref::<StructArray>())
    else {
        return Vec::new();
    };
    let axis = |i: usize| {
        position
            .column(i)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("GeoArrow separated coordinates are Float64 children")
    };
    let (x, y, z) = (axis(0), axis(1), axis(2));
    let u8col = |name: &str| {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<UInt8Array>().cloned())
    };
    let u16col = |name: &str| {
        batch
            .column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<UInt16Array>().cloned())
    };
    let class = u8col("classification");
    let ret = u8col("return_number");
    let nret = u8col("number_of_returns");
    let intensity = u16col("intensity");
    // Absent on point format 6, which has no colour. `None` is the honest answer and the
    // renderer falls back to another ramp rather than inventing grey.
    let (red, green, blue) = (u16col("red"), u16col("green"), u16col("blue"));

    // LAS stores colour as three 16-bit channels, and writers very often put 8-bit values
    // in them. No field says which, so the scale has to be inferred from the data — and
    // inferring it is the *host's* job, not a library crate's. Per node rather than per
    // file, because a node is the batch in hand.
    let scale = {
        let peak = [&red, &green, &blue]
            .iter()
            .filter_map(|c| c.as_ref())
            .flat_map(|c| (0..batch.num_rows()).map(move |i| c.value(i)))
            .max()
            .unwrap_or(0);
        if peak > 255 {
            65535.0
        } else {
            255.0
        }
    };
    let channel = |c: &Option<UInt16Array>, i: usize| {
        c.as_ref().map(|c| c.value(i) as f32 / scale).unwrap_or(0.0)
    };

    (0..batch.num_rows())
        .map(|i| Vertex {
            x: (x.value(i) - origin[0]) as f32,
            y: (y.value(i) - origin[1]) as f32,
            z: (z.value(i) - origin[2]) as f32,
            class: class.as_ref().map(|c| c.value(i)).unwrap_or(0),
            rgb: [channel(&red, i), channel(&green, i), channel(&blue, i)],
            // Which attribute goes in which channel is the HOST's decision, and the
            // renderer never learns it — see `CHANNEL_LABELS`. A channel an analytical pass
            // computed would be filled here identically.
            channels: [
                (z.value(i) - origin[2]) as f32,
                intensity.as_ref().map(|c| c.value(i)).unwrap_or(0) as f32,
                nret.as_ref().map(|c| c.value(i)).unwrap_or(1) as f32,
                ret.as_ref().map(|c| c.value(i)).unwrap_or(1) as f32,
            ],
        })
        .collect()
}

/// What this host put in each channel. Published for the interface to label; the renderer
/// has no equivalent and needs none.
pub const CHANNEL_LABELS: [&str; strider_view::CHANNELS] =
    ["height", "intensity", "number of returns", "return number"];

/// The range each channel spans, and the source's colour availability.
///
/// **Where a range comes from is the whole question.** Deriving it from the resident set is
/// wrong: it shifts as the camera moves, so the same point changes colour when a neighbour
/// loads and two frames cannot be compared. Deriving it from the attribute's type domain is
/// also wrong: LAS intensity is a 16-bit field and this file uses 9 883..65 535 of it, so a
/// 0..65 535 ramp wastes most of its range.
///
/// What this does instead is read the COPC **root node**, which is a decimated sample of the
/// entire cloud rather than of any one region — one read, bounded, and stable under panning
/// because it does not depend on where the camera is. It is an approximation: an outlier
/// outside the sample widens the true range without widening this one.
///
/// The exact answer is a **summary** in the sense CONTEXT.md gives the word — a reduction
/// whose size is bounded independently of its input, which a min/max trivially is — and
/// computing one over the full extent is a separately scheduled operation
/// ([[RFC-0006:C-RENDER]] 3), not something the frame does. That is where an analytical
/// engine belongs, and [[RFC-0006:C-LAYERING]] 2 says it is a separate optional component
/// above the spatial access layer rather than anything the renderer may depend on.
#[derive(Clone, Copy, Debug)]
pub struct RampStats {
    pub channels: [(f32, f32); strider_view::CHANNELS],
    pub source_has_colour: bool,
    /// How the ranges were obtained, so a reader is never guessing.
    pub provenance: &'static str,
}

impl RampStats {
    pub fn from_sample(verts: &[Vertex], provenance: &'static str) -> Self {
        let mut channels = [(f32::MAX, f32::MIN); strider_view::CHANNELS];
        let mut source_has_colour = false;
        for v in verts {
            for (c, r) in v.channels.iter().zip(channels.iter_mut()) {
                r.0 = r.0.min(*c);
                r.1 = r.1.max(*c);
            }
            source_has_colour |= v.rgb != [0.0, 0.0, 0.0];
        }
        for r in channels.iter_mut() {
            if r.0 > r.1 {
                *r = (0.0, 1.0);
            }
        }
        Self {
            channels,
            source_has_colour,
            provenance,
        }
    }
}

/// Whether the source carries colour at all — point formats 7 and 8 do, 6 does not.
pub fn has_colour(batch: &RecordBatch) -> bool {
    batch.column_by_name("red").is_some()
}
