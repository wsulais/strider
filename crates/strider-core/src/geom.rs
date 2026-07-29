// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The little geometry every layer shares.
//!
//! Here rather than in a source adapter because a bounding box is a property of the
//! *model* — a partition has one, a query is one, a grid is georeferenced against one —
//! and an adapter that owned the type would make every other adapter depend on it.

/// An axis-aligned bounding box, in whatever coordinates its holder declares.
///
/// Carries no coordinate reference system on purpose. A box means nothing without one,
/// and the system is carried on the coordinate *field* of a batch
/// ([[RFC-0005:C-CRS]] 1) rather than duplicated onto every box that describes it —
/// duplicating it is how two answers about one system come to exist.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Aabb {
    /// Overlap in the horizontal plane only.
    ///
    /// Separate from a full three-dimensional test because a viewport query is a column:
    /// it constrains x and y and takes every z. Testing z as well would need a caller to
    /// invent infinities, which is how a query accidentally excludes the ground.
    pub fn intersects_xy(&self, other: &Aabb) -> bool {
        self.min[0] <= other.max[0]
            && self.max[0] >= other.min[0]
            && self.min[1] <= other.max[1]
            && self.max[1] >= other.min[1]
    }

    pub fn contains_xy(&self, x: f64, y: f64) -> bool {
        x >= self.min[0] && x <= self.max[0] && y >= self.min[1] && y <= self.max[1]
    }

    pub fn span(&self) -> [f64; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }
}
