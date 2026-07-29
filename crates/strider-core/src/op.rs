// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What an operator declares about itself, before it runs.
//!
//! [[RFC-0002:C-EXEC]] 1 is the whole reason this module exists: an operator expressed only as an
//! opaque stream transform has no way to tell the scheduler it needs points it was not handed, so
//! it is either silently wrong at every partition boundary or forces the dataset into memory.
//! Requirements are therefore **declared**, and satisfying them is the planner's obligation.
//!
//! The declaration is deliberately inert. Nothing here executes, allocates a device or touches a
//! source: `declare()` is callable on an operator that will never run, which is what lets a
//! planner reason about a pipeline before committing to it.

use crate::Aabb;

/// An argument to a type constructor: another type, or an integer.
///
/// [[RFC-0002:C-PORT]] 1's sketch admits exactly these two, told apart by CBOR major type when
/// encoded. Kept as an enum rather than as text because the clause is explicit that `"grid<2>"`
/// is an *encoding* of a type and must not be the type itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeArg {
    Type(TypeExpr),
    Count(u32),
}

/// A type constructor applied to its arguments — what a port accepts.
///
/// Parameterised rather than bare wherever an obligation reads the argument: C-PORT 1 requires a
/// `grid` offered without its axis count to be refused, because [[RFC-0002:C-GRID]] 3 quantifies
/// over "every axis they share" and [[RFC-0002:C-SUMMARY]] 2's bound is a product over axes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeExpr {
    pub constructor: &'static str,
    pub arguments: Vec<TypeArg>,
}

impl TypeExpr {
    /// A constructor with no arguments — `points`.
    pub fn bare(constructor: &'static str) -> Self {
        Self {
            constructor,
            arguments: Vec::new(),
        }
    }

    /// A constructor parameterised by a count — `grid<2>`.
    pub fn counted(constructor: &'static str, n: u32) -> Self {
        Self {
            constructor,
            arguments: vec![TypeArg::Count(n)],
        }
    }

    /// `set<T>`: any number of inputs, and the operator's result is invariant to their order.
    ///
    /// Cardinality is part of the type rather than a separate declaration (C-PORT 2), so the
    /// invariance claim travels with the port instead of being asserted somewhere else.
    pub fn set(inner: TypeExpr) -> Self {
        Self {
            constructor: "set",
            arguments: vec![TypeArg::Type(inner)],
        }
    }
}

/// How far beyond a partition an operator needs to see, in CRS units.
///
/// # Why a distance and not a neighbour count
///
/// The obvious declaration for a k-nearest-neighbour operator is `k`, and it cannot be used: `k`
/// says how many neighbours are wanted, not how far away they are, so a planner cannot turn it
/// into an extent to fetch without already knowing the local density — which varies across the
/// cloud and is exactly what an out-of-core planner has not read yet. PDAL has the same shape and
/// resolves it by simply reading whatever is in memory, which is available to a tool that loads
/// the file.
///
/// So an operator that wants neighbours declares the **radius** it will search, and takes its `k`
/// from within that radius. Where fewer than `k` neighbours fall inside it, the operator works
/// with fewer rather than reaching further, because reaching further is precisely the thing the
/// declaration promised not to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Halo {
    /// The operator reads only the points it was handed.
    None,
    /// The operator reads points within this distance of the partition's bounds.
    Radius(f64),
}

impl Halo {
    pub fn radius(self) -> f64 {
        match self {
            Halo::None => 0.0,
            Halo::Radius(r) => r,
        }
    }

    /// The bounds a partition must be grown to, to satisfy this halo.
    ///
    /// The planner's obligation, not the operator's (C-EXEC's preamble), so it lives beside the
    /// declaration rather than inside any operator.
    pub fn grow(self, bounds: &Aabb) -> Aabb {
        let r = self.radius();
        Aabb {
            min: [bounds.min[0] - r, bounds.min[1] - r, bounds.min[2] - r],
            max: [bounds.max[0] + r, bounds.max[1] + r, bounds.max[2] + r],
        }
    }
}

/// One input port: a name, what it accepts, how far it sees, and what it reads.
///
/// Halo width and attributes read are declared **per port** (C-EXEC 1): an operator reading a grid
/// on one port and points on another does not read one set of attributes.
#[derive(Clone, Debug)]
pub struct Port {
    pub name: &'static str,
    pub accepts: TypeExpr,
    pub halo: Halo,
    /// The attributes this port reads. Declared so a planner can project: a port that reads only
    /// position need not have colour or GPS time delivered to it.
    pub reads: &'static [&'static str],
}

/// Everything an operator says about itself without being executed (C-EXEC 1).
#[derive(Clone, Debug)]
pub struct Declaration {
    /// Ports in declaration order, which is part of the declaration. Names must be unique; an
    /// operator reading no input declares none.
    pub ports: Vec<Port>,
    /// Declared once for the operator, being a property of its traversal rather than of an input.
    pub passes: u32,
}

impl Declaration {
    /// Whether the declaration is well-formed on the points C-EXEC 1 states directly.
    ///
    /// Checked here rather than trusted, because a malformed declaration misleads the planner in
    /// exactly the direction that produces wrong answers quietly — a duplicate port name makes one
    /// port's halo silently stand in for another's.
    pub fn duplicate_port(&self) -> Option<&'static str> {
        for (i, p) in self.ports.iter().enumerate() {
            if self.ports[..i].iter().any(|q| q.name == p.name) {
                return Some(p.name);
            }
        }
        None
    }

    /// The widest halo any port declares — what a partitioning must satisfy to run this operator.
    pub fn widest_halo(&self) -> Halo {
        self.ports
            .iter()
            .map(|p| p.halo)
            .fold(Halo::None, |a, b| {
                if b.radius() > a.radius() {
                    b
                } else {
                    a
                }
            })
    }
}

/// An operator: something that declares its requirements before it runs.
///
/// Deliberately no `run` method here. What an operator does with a batch differs by operator and
/// belongs to the crate implementing it; what every operator owes the planner is this declaration,
/// and that is what a shared trait is for.
pub trait Operator {
    fn declare(&self) -> Declaration;
}

/// Whether a point is the partition's own or was supplied to satisfy a halo.
///
/// [[RFC-0002:C-HALO]] 1 requires this to be a dedicated boolean in the batch and fixes what it
/// records: a point is a **core point** of the partition whose bounds contain its position, and a
/// **halo point** otherwise. Nothing else may decide it — not an ordering among the partitions
/// whose halo reaches the point, and not a choice made where the partition was assembled.
pub const HALO_ATTRIBUTE: &str = "is_halo";

/// Classify a position against a partition's bounds, as C-HALO 1 defines it.
///
/// Faces are half-open on the maximum side, which is the closedness C-EXEC 4 requires a
/// partitioning to assign: a point lying exactly on a face is then a core point of exactly one of
/// the partitions sharing it, and no position within the extent belongs to none.
pub fn is_core(bounds: &Aabb, p: [f64; 3]) -> bool {
    (0..3).all(|i| p[i] >= bounds.min[i] && p[i] < bounds.max[i])
}
