// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The COPC info record and the octree hierarchy — the source's **index**.
//!
//! Everything in this module is metadata: node identities, node bounds, point counts,
//! and where each node's compressed chunk lives. No points. That separation is what
//! lets a consumer resolve *which* partitions a request touches without reading any of
//! them, which is what the spatial access layer does and what
//! [[RFC-0007:C-EXTRACT]] 4 relies on being possible.

use crate::error::{Error, Result};
use crate::las::{f64_at, i32_at, u64_at, Vlr};
use strider_core::{Aabb, Range};
use core::fmt;

/// User id both COPC records carry.
pub const USER_ID: &str = "copc";
/// Record id of the info VLR.
pub const INFO_RECORD_ID: u16 = 1;
/// Record id of the hierarchy record.
pub const HIERARCHY_RECORD_ID: u16 = 1000;
/// Declared length of the info VLR payload.
pub const INFO_LEN: usize = 160;
/// Size of one hierarchy entry.
pub const ENTRY_LEN: usize = 32;

/// The COPC info record.
#[derive(Clone, Copy, Debug)]
pub struct Info {
    /// Centre of the root node's cube.
    pub center: [f64; 3],
    /// Half the root cube's side. The octree is a cube, not the header's bounding box.
    pub halfsize: f64,
    /// Root-node spacing: the root cube's side divided by the points it holds. Level
    /// `n`'s spacing is this over `2^n`, which is what makes a level-of-detail choice
    /// answerable from the index alone.
    pub spacing: f64,
    pub root_hier_offset: u64,
    pub root_hier_size: u64,
}

impl Info {
    pub fn parse(v: &Vlr<'_>) -> Result<Self> {
        if v.data.len() < INFO_LEN {
            return Err(Error::ShortRead {
                what: "the COPC info record",
                declared: INFO_LEN,
                delivered: v.data.len(),
            });
        }
        let b = v.data;
        Ok(Info {
            center: [f64_at(b, 0), f64_at(b, 8), f64_at(b, 16)],
            halfsize: f64_at(b, 24),
            spacing: f64_at(b, 32),
            root_hier_offset: u64_at(b, 40),
            root_hier_size: u64_at(b, 48),
        })
    }

    /// The root node's cube.
    pub fn root_bounds(&self) -> Aabb {
        Aabb {
            min: [
                self.center[0] - self.halfsize,
                self.center[1] - self.halfsize,
                self.center[2] - self.halfsize,
            ],
            max: [
                self.center[0] + self.halfsize,
                self.center[1] + self.halfsize,
                self.center[2] + self.halfsize,
            ],
        }
    }

    /// Point spacing at a level. The quantity a level-of-detail choice is made against.
    pub fn spacing_at(&self, level: u8) -> f64 {
        self.spacing / (1u64 << level) as f64
    }

    /// Where the root hierarchy page lives.
    pub fn root_page(&self) -> Range {
        Range::new(self.root_hier_offset, self.root_hier_size)
    }
}

/// An octree node's identity: its level and its cell within that level.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct VoxelKey {
    pub level: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl fmt::Display for VoxelKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}-{}-{}", self.level, self.x, self.y, self.z)
    }
}

impl VoxelKey {
    /// The node's cube, derived from the root cube. Cheap, and derived rather than
    /// stored because the file does not store it.
    pub fn bounds(&self, root: &Aabb) -> Aabb {
        let side = (root.max[0] - root.min[0]) / (1u64 << self.level) as f64;
        let (x, y, z) = (self.x as f64, self.y as f64, self.z as f64);
        Aabb {
            min: [
                root.min[0] + x * side,
                root.min[1] + y * side,
                root.min[2] + z * side,
            ],
            max: [
                root.min[0] + (x + 1.0) * side,
                root.min[1] + (y + 1.0) * side,
                root.min[2] + (z + 1.0) * side,
            ],
        }
    }

    /// The key one level up, or `None` at the root.
    pub fn parent(&self) -> Option<VoxelKey> {
        if self.level <= 0 {
            return None;
        }
        Some(VoxelKey {
            level: self.level - 1,
            x: self.x >> 1,
            y: self.y >> 1,
            z: self.z >> 1,
        })
    }

    /// Which of the parent's eight octants this key occupies, as a bit position.
    pub fn octant(&self) -> u8 {
        ((self.x & 1) | ((self.y & 1) << 1) | ((self.z & 1) << 2)) as u8
    }

    /// The child in a given octant.
    pub fn child(&self, octant: u8) -> VoxelKey {
        VoxelKey {
            level: self.level + 1,
            x: (self.x << 1) | (octant & 1) as i32,
            y: (self.y << 1) | ((octant >> 1) & 1) as i32,
            z: (self.z << 1) | ((octant >> 2) & 1) as i32,
        }
    }
}

/// One hierarchy entry.
///
/// `point_count == -1` marks a **page reference** rather than a node: the hierarchy is
/// paged, so a large file's index is itself read incrementally. That is the property
/// that makes an 11 GB source openable without reading its whole index, and it is why
/// `Hierarchy` distinguishes the two rather than flattening them.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub key: VoxelKey,
    pub offset: u64,
    pub byte_size: u32,
    pub point_count: i32,
}

impl Entry {
    pub fn is_page_ref(&self) -> bool {
        self.point_count < 0
    }

    /// Where this node's compressed chunk, or this page's bytes, live.
    pub fn range(&self) -> Range {
        Range::new(self.offset, self.byte_size as u64)
    }
}

/// One hierarchy page: a flat run of 32-byte entries.
pub fn parse_page(b: &[u8]) -> Result<Vec<Entry>> {
    // Not `is_multiple_of`: that is stable since 1.87 and the workspace declares 1.85.
    if b.len() % ENTRY_LEN != 0 {
        return Err(Error::RaggedHierarchyPage(b.len()));
    }
    let mut out = Vec::with_capacity(b.len() / ENTRY_LEN);
    for e in b.chunks_exact(ENTRY_LEN) {
        out.push(Entry {
            key: VoxelKey {
                level: i32_at(e, 0),
                x: i32_at(e, 4),
                y: i32_at(e, 8),
                z: i32_at(e, 12),
            },
            offset: u64_at(e, 16),
            byte_size: i32_at(e, 24) as u32,
            point_count: i32_at(e, 28),
        });
    }
    Ok(out)
}

/// A node with points: an entry that is not a page reference, with its cube resolved.
#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub key: VoxelKey,
    pub bounds: Aabb,
    pub point_count: u32,
    pub chunk: Range,
}

impl Node {
    pub fn level(&self) -> u8 {
        self.key.level as u8
    }
}

/// The part of the index that has been read so far, and what remains unread.
///
/// Loaded lazily by design. A 13 GB source's hierarchy is paged, and reading all of it
/// to answer a request over one corner of the extent would make opening the file cost a
/// function of its size — the failure the whole project exists to avoid.
/// What the index knows about one key.
///
/// Structure and payload are separate on purpose. A key can be part of the tree without
/// having points to draw — the writer may record it with a count of zero, and a page
/// reference is a key whose subtree is real but unread — and descent has to pass through
/// those to reach what is below them.
#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    /// Position in `nodes`, if this key has points.
    node: Option<u32>,
    /// One bit per octant: which children the index has seen.
    ///
    /// This is the link that makes traversal a tree walk. Without it a consumer has to
    /// synthesise all eight child keys of every node it visits and ask about each, which
    /// visits cells that do not exist — measured at 2025 cells to find 14 nodes. With it,
    /// descent only ever touches keys the index has actually recorded.
    children: u8,
}

#[derive(Clone, Debug, Default)]
pub struct Hierarchy {
    nodes: Vec<Node>,
    /// The paged index, in memory, keyed.
    ///
    /// Not a second copy of the file's index: it *is* the part of the file's index that has
    /// been read, and it has to be materialised because a COPC hierarchy page is an unordered
    /// flat run of entries. There is no way to find a key inside a page without scanning it and
    /// no way to know which page holds a key without having read its ancestors, so the on-disk
    /// form is a tree to descend, never a map to query. What this type owes a caller is that
    /// descending it costs the size of the subtree walked and not the size of the file.
    by_key: std::collections::BTreeMap<VoxelKey, Slot>,
    unread_pages: Vec<Entry>,
    pages_read: usize,
}

impl Hierarchy {
    /// Fold a page's entries in, separating nodes from references to further pages.
    pub fn absorb(&mut self, page: &[Entry], root: &Aabb) {
        self.pages_read += 1;
        for e in page {
            if e.is_page_ref() {
                self.unread_pages.push(*e);
            }
            // A `point_count` of exactly zero is a key the writer recorded as empty. It stays
            // out of `nodes` deliberately — a consumer asking for it would issue a retrieval
            // for nothing — but it does get a slot, because its subtree may not be empty and
            // descent has to be able to pass through it.
            // A negative level is not a key. The root page carries one as a terminator, so it
            // must not become a slot, or the tree grows a parent above its own root.
            if e.key.level < 0 {
                continue;
            }
            let slot = self.by_key.entry(e.key).or_default();
            if !e.is_page_ref() && e.point_count > 0 {
                slot.node = Some(self.nodes.len() as u32);
                self.nodes.push(Node {
                    key: e.key,
                    bounds: e.key.bounds(root),
                    point_count: e.point_count as u32,
                    chunk: e.range(),
                });
            }
            self.link(e.key);
        }
        self.unread_pages.retain(|p| p.key != VoxelKey {
            level: -1,
            x: 0,
            y: 0,
            z: 0,
        });
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// One node by key.
    ///
    /// Present for the caller that has a key from somewhere else. Traversal should not need
    /// it: use `children`, which yields keys the index holds rather than keys to ask about.
    pub fn node(&self, key: VoxelKey) -> Option<&Node> {
        let slot = self.by_key.get(&key)?;
        self.nodes.get(slot.node? as usize)
    }

    /// The children of `key` that the index has seen, as keys.
    ///
    /// Empty means one of two different things, and a caller that refines has to tell them
    /// apart: either `key` is a leaf, or its subtree lives in a page not yet read. Ask
    /// `pages_meeting` for the second.
    pub fn children(&self, key: VoxelKey) -> impl Iterator<Item = VoxelKey> + '_ {
        let mask = self.by_key.get(&key).map_or(0, |s| s.children);
        (0..8u8)
            .filter(move |o| mask & (1 << o) != 0)
            .map(move |o| key.child(o))
    }

    /// Join a newly known key to its parent and to any children already known.
    ///
    /// Both directions, because pages need not arrive parent-first: retrieval is batched and
    /// unordered, so a subtree page can land before the page that referenced it.
    fn link(&mut self, key: VoxelKey) {
        if let Some(parent) = key.parent() {
            self.by_key.entry(parent).or_default().children |= 1 << key.octant();
        }
        let mut mask = 0u8;
        for octant in 0..8u8 {
            if self.by_key.contains_key(&key.child(octant)) {
                mask |= 1 << octant;
            }
        }
        if let Some(slot) = self.by_key.get_mut(&key) {
            slot.children |= mask;
        }
    }

    pub fn pages_read(&self) -> usize {
        self.pages_read
    }

    /// Unread pages whose cube meets the query, as ranges to retrieve.
    ///
    /// This is the incremental half of index traversal: a page reference carries the
    /// key of the subtree it covers, so a query that misses that cube never reads it.
    pub fn pages_meeting(&self, root: &Aabb, query: &Aabb, max_level: u8) -> Vec<Entry> {
        self.unread_pages
            .iter()
            .copied()
            .filter(|p| p.key.level as u8 <= max_level && p.key.bounds(root).intersects_xy(query))
            .collect()
    }

    pub fn forget_page(&mut self, key: VoxelKey) {
        self.unread_pages.retain(|p| p.key != key);
    }

    pub fn unread_pages(&self) -> usize {
        self.unread_pages.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> Aabb {
        Aabb {
            min: [0.0, 0.0, 0.0],
            max: [8.0, 8.0, 8.0],
        }
    }

    fn key(level: i32, x: i32, y: i32, z: i32) -> VoxelKey {
        VoxelKey { level, x, y, z }
    }

    fn node_entry(k: VoxelKey, count: i32) -> Entry {
        Entry {
            key: k,
            offset: 1,
            byte_size: 1,
            point_count: count,
        }
    }

    #[test]
    fn octant_round_trips_through_parent() {
        for octant in 0..8u8 {
            let parent = key(2, 1, 2, 3);
            let child = parent.child(octant);
            assert_eq!(child.parent(), Some(parent));
            assert_eq!(child.octant(), octant);
        }
        assert_eq!(key(0, 0, 0, 0).parent(), None);
    }

    #[test]
    fn children_are_the_keys_the_index_holds() {
        let mut h = Hierarchy::default();
        h.absorb(
            &[
                node_entry(key(0, 0, 0, 0), 10),
                node_entry(key(1, 0, 0, 0), 20),
                node_entry(key(1, 1, 1, 1), 30),
            ],
            &root(),
        );
        let mut got: Vec<_> = h.children(key(0, 0, 0, 0)).collect();
        got.sort_by_key(|k| (k.x, k.y, k.z));
        assert_eq!(got, vec![key(1, 0, 0, 0), key(1, 1, 1, 1)]);
        // Seven of the eight octants below 1-0-0-0 do not exist, and descent must not ask.
        assert_eq!(h.children(key(1, 0, 0, 0)).count(), 0);
    }

    #[test]
    fn a_subtree_page_arriving_before_its_parent_still_links() {
        // Retrieval is batched and unordered, so this ordering is reachable. If linking only
        // ran parent-to-child, the whole subtree would be silently unreachable — which looks
        // like a sparse render rather than like a bug.
        let mut h = Hierarchy::default();
        h.absorb(&[node_entry(key(1, 0, 0, 0), 20)], &root());
        h.absorb(&[node_entry(key(0, 0, 0, 0), 10)], &root());
        assert_eq!(
            h.children(key(0, 0, 0, 0)).collect::<Vec<_>>(),
            vec![key(1, 0, 0, 0)]
        );
    }

    #[test]
    fn an_empty_key_is_still_a_path_to_its_children() {
        // `point_count == 0` is a key with nothing to draw. It must not be a wall: pruning
        // there would drop every point below it.
        let mut h = Hierarchy::default();
        h.absorb(
            &[
                node_entry(key(0, 0, 0, 0), 10),
                node_entry(key(1, 0, 0, 0), 0),
                node_entry(key(2, 0, 0, 0), 40),
            ],
            &root(),
        );
        assert!(h.node(key(1, 0, 0, 0)).is_none(), "nothing to retrieve");
        assert_eq!(
            h.children(key(1, 0, 0, 0)).collect::<Vec<_>>(),
            vec![key(2, 0, 0, 0)],
            "but the path through it is open"
        );
    }

    #[test]
    fn a_page_reference_is_a_key_without_a_node() {
        let mut h = Hierarchy::default();
        h.absorb(
            &[
                node_entry(key(0, 0, 0, 0), 10),
                Entry {
                    key: key(1, 1, 0, 0),
                    offset: 64,
                    byte_size: 32,
                    point_count: -1,
                },
            ],
            &root(),
        );
        assert_eq!(h.unread_pages(), 1);
        assert!(h.node(key(1, 1, 0, 0)).is_none());
        // Known to exist, so descent reaches it and stops — which is what tells the host to
        // read the page rather than to conclude the subtree is empty.
        assert_eq!(
            h.children(key(0, 0, 0, 0)).collect::<Vec<_>>(),
            vec![key(1, 1, 0, 0)]
        );
    }
}
