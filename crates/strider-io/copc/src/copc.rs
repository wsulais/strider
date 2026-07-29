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
#[derive(Clone, Debug, Default)]
pub struct Hierarchy {
    nodes: Vec<Node>,
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
            } else if e.point_count > 0 {
                self.nodes.push(Node {
                    key: e.key,
                    bounds: e.key.bounds(root),
                    point_count: e.point_count as u32,
                    chunk: e.range(),
                });
            }
            // A `point_count` of exactly zero is a node the writer recorded as empty.
            // Kept out of `nodes` deliberately: a consumer asking for it would issue a
            // retrieval for nothing.
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
