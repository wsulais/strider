// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Normal estimation by plane fit over a bounded neighbourhood, and the hillshade that follows.
//!
//! The shape is PDAL's `filters.normal`: take each point's neighbours, fit a plane, and the
//! normal is the eigenvector of the smallest eigenvalue of the neighbourhood's covariance. What
//! differs is the neighbourhood, and the difference is forced by [[RFC-0002:C-EXEC]] 1.
//!
//! # Why a radius rather than a k
//!
//! PDAL takes `knn=8` and searches its in-memory index for eight neighbours however far away they
//! are. That is available to a tool that has loaded the file. It is not available here: `k` says
//! how many neighbours are wanted, not how far away they are, so a planner cannot turn it into an
//! extent to fetch without already knowing the local density — which varies across the cloud and
//! is exactly what an out-of-core planner has not read yet.
//!
//! So the declared requirement is a **radius**, and `k` is taken from within it. Where fewer than
//! `k` neighbours fall inside the radius the fit uses fewer, and where fewer than three do the
//! normal is undefined and reported as such — reaching further is the one thing the declaration
//! promised not to do, and an operator that quietly reached further would be wrong precisely at
//! partition boundaries, where the halo runs out.

use strider_core::op::{Declaration, Halo, Operator, Port, TypeExpr};

/// Estimate a normal per point from the points within `radius`.
#[derive(Clone, Copy, Debug)]
pub struct Normals {
    /// The neighbourhood radius, in CRS units. This *is* the declared halo.
    pub radius: f64,
    /// At most this many neighbours are used, nearest first. A cap on work, not on reach: the
    /// reach is the radius, and it is what the planner was told.
    pub max_neighbours: usize,
}

impl Default for Normals {
    fn default() -> Self {
        // PDAL's default k, with a radius the caller is expected to set for their data. There is
        // no defensible default radius — it is a property of the cloud's density, not of the
        // algorithm — so this one is small enough to be obviously wrong rather than quietly so.
        Self {
            radius: 1.0,
            max_neighbours: 8,
        }
    }
}

impl Operator for Normals {
    fn declare(&self) -> Declaration {
        Declaration {
            ports: vec![Port {
                name: "points",
                accepts: TypeExpr::bare("points"),
                // The halo IS the radius. Stating it any other way would let the two drift, and a
                // halo narrower than the search is wrong only near partition edges — the failure
                // mode this whole declaration exists to prevent.
                halo: Halo::Radius(self.radius),
                // Position only. A planner may project everything else away for this port.
                reads: &["position"],
            }],
            passes: 1,
        }
    }
}

/// A unit normal, or `None` where the neighbourhood was too small to define a plane.
pub type Normal = Option<[f64; 3]>;

impl Normals {
    /// Estimate a normal for every point in `core`, using `all` as the neighbourhood source.
    ///
    /// `all` is the partition's own points **plus** its halo; `core` indexes into `all` and names
    /// the points the answer is wanted for. Splitting them is what makes the halo do its job: a
    /// halo point contributes to a core point's fit but gets no answer of its own, because it is
    /// some other partition's core point and that partition has neighbours for it that this one
    /// does not ([[RFC-0002:C-HALO]] 1).
    pub fn estimate(&self, all: &[[f64; 3]], core: &[usize]) -> Vec<Normal> {
        core.iter().map(|&i| self.at(all, i)).collect()
    }

    fn at(&self, all: &[[f64; 3]], i: usize) -> Normal {
        let p = all[i];
        let r2 = self.radius * self.radius;

        // Nearest-first within the radius. A linear scan: this is the reference implementation,
        // and a spatial index is an optimisation that must produce the same answer, which is
        // easier to assert against something obviously correct.
        let mut near: Vec<(f64, usize)> = all
            .iter()
            .enumerate()
            .filter_map(|(j, q)| {
                let d2 = sq_dist(p, *q);
                (d2 <= r2).then_some((d2, j))
            })
            .collect();
        near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));
        near.truncate(self.max_neighbours.max(3));

        if near.len() < 3 {
            return None;
        }

        // Covariance about the neighbourhood centroid.
        let n = near.len() as f64;
        let mut c = [0.0f64; 3];
        for &(_, j) in &near {
            for k in 0..3 {
                c[k] += all[j][k];
            }
        }
        for v in &mut c {
            *v /= n;
        }
        let mut m = [[0.0f64; 3]; 3];
        for &(_, j) in &near {
            let d = [all[j][0] - c[0], all[j][1] - c[1], all[j][2] - c[2]];
            for a in 0..3 {
                for b in 0..3 {
                    m[a][b] += d[a] * d[b];
                }
            }
        }
        for row in &mut m {
            for v in row.iter_mut() {
                *v /= n;
            }
        }

        smallest_eigenvector(m).map(|v| {
            // Oriented upward. Not a geometric truth — a plane's normal has two directions and
            // nothing local distinguishes them — but hillshade needs a consistent choice, and for
            // terrain "up" is the one a reader expects.
            if v[2] < 0.0 {
                [-v[0], -v[1], -v[2]]
            } else {
                v
            }
        })
    }
}

fn sq_dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// The eigenvector of the smallest eigenvalue of a symmetric 3x3, by inverse iteration.
///
/// Small and fixed-size, so an iterative method with a fixed budget is both simple and bounded —
/// no dependency, and no way for a degenerate neighbourhood to make it loop.
fn smallest_eigenvector(m: [[f64; 3]; 3]) -> Option<[f64; 3]> {
    // Deflate towards the smallest eigenvalue by iterating with (tr*I - M), whose largest
    // eigenvector is M's smallest. Power iteration on that is stable and needs no inverse.
    let tr = m[0][0] + m[1][1] + m[2][2];
    if !tr.is_finite() || tr <= 0.0 {
        return None;
    }
    let a = [
        [tr - m[0][0], -m[0][1], -m[0][2]],
        [-m[1][0], tr - m[1][1], -m[1][2]],
        [-m[2][0], -m[2][1], tr - m[2][2]],
    ];
    // Three starts, because a single fixed start can be orthogonal to the answer.
    let mut best: Option<([f64; 3], f64)> = None;
    for start in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
        let mut v = start;
        for _ in 0..64 {
            let w = [
                a[0][0] * v[0] + a[0][1] * v[1] + a[0][2] * v[2],
                a[1][0] * v[0] + a[1][1] * v[1] + a[1][2] * v[2],
                a[2][0] * v[0] + a[2][1] * v[1] + a[2][2] * v[2],
            ];
            let n = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt();
            if !n.is_finite() || n < 1e-300 {
                break;
            }
            v = [w[0] / n, w[1] / n, w[2] / n];
        }
        // Rayleigh quotient against the ORIGINAL matrix: the winner is the one with the smallest
        // eigenvalue, which is the plane's normal.
        let mv = [
            m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
            m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
            m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
        ];
        let lambda = v[0] * mv[0] + v[1] * mv[1] + v[2] * mv[2];
        if lambda.is_finite() && best.as_ref().is_none_or(|(_, l)| lambda < *l) {
            best = Some((v, lambda));
        }
    }
    best.map(|(v, _)| v)
}

/// Lambertian shade of a normal against a light direction, in `0.0..=1.0`.
///
/// Hillshade is this and nothing more once normals exist, which is the point of computing them:
/// the renderer already ramps an arbitrary scalar channel, so a shade is a channel and the grey
/// ramp draws it with no renderer change at all.
///
/// `azimuth` and `altitude` are in degrees, as every GIS states them — azimuth clockwise from
/// north, altitude above the horizon. The GDAL/QGIS defaults are 315 and 45.
pub fn hillshade(normal: Normal, azimuth_deg: f64, altitude_deg: f64) -> f32 {
    let Some(n) = normal else {
        // No plane, no shade. Flat mid-grey reads as "unknown" rather than as a slope that
        // happens to face away, which a zero would.
        return 0.5;
    };
    let az = (90.0 - azimuth_deg).to_radians();
    let alt = altitude_deg.to_radians();
    let light = [
        alt.cos() * az.cos(),
        alt.cos() * az.sin(),
        alt.sin(),
    ];
    let dot = n[0] * light[0] + n[1] * light[1] + n[2] * light[2];
    dot.clamp(0.0, 1.0) as f32
}
