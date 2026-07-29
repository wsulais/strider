// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! verifies [[RFC-0002:C-HALO]] and [[RFC-0002:C-EXEC]] 1
//!
//! The claim under test is the one the whole declaration mechanism exists to support: **an
//! operator run partition by partition, given the halo it declared, produces the same answer as
//! the same operator run over one undivided partition.** If that fails the operator's declaration
//! is wrong, and the failure appears only near partition boundaries — which is why it is checked
//! against a reference rather than eyeballed.
//!
//! The negative half matters as much. A run with the halo deliberately withheld MUST differ,
//! because a test that passes either way is testing nothing: it would pass against an operator
//! that ignored its neighbours entirely.

use strider_algo::Normals;
use strider_core::op::{is_core, Halo, Operator};
use strider_core::Aabb;

/// A deterministic, uneven cloud. Uneven on purpose: a uniform grid makes every neighbourhood
/// identical, so a halo bug that drops a few neighbours changes nothing and the test passes for
/// the wrong reason.
fn cloud() -> Vec<[f64; 3]> {
    let mut pts = Vec::new();
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    for _ in 0..1200 {
        let x = rand() * 20.0;
        let y = rand() * 20.0;
        // A gentle surface plus noise, so normals are well-defined and vary across the extent.
        let z = (x * 0.25).sin() * 1.5 + (y * 0.2).cos() * 1.0 + rand() * 0.05;
        pts.push([x, y, z]);
    }
    pts
}

/// Split the extent into a 3x3 column grid. Half-open on the maximum side, which is the
/// closedness C-EXEC 4 requires: a point on a face is a core point of exactly one partition.
fn partitions() -> Vec<Aabb> {
    let mut out = Vec::new();
    for i in 0..3 {
        for j in 0..3 {
            out.push(Aabb {
                min: [i as f64 * 20.0 / 3.0, j as f64 * 20.0 / 3.0, f64::MIN],
                max: [
                    (i + 1) as f64 * 20.0 / 3.0,
                    (j + 1) as f64 * 20.0 / 3.0,
                    f64::MAX,
                ],
            });
        }
    }
    out
}

/// Run the operator over one partition, given `halo` to grow its fetch by.
///
/// Returns `(index into the whole cloud, normal)` for the partition's **core** points only. A
/// halo point gets no answer here: it is another partition's core point, and that partition has
/// neighbours for it this one does not.
fn run_partition(
    op: &Normals,
    pts: &[[f64; 3]],
    bounds: &Aabb,
    halo: Halo,
) -> Vec<(usize, Option<[f64; 3]>)> {
    let grown = halo.grow(bounds);
    let supplied: Vec<usize> = (0..pts.len())
        .filter(|&i| is_core(&grown, pts[i]))
        .collect();
    let local: Vec<[f64; 3]> = supplied.iter().map(|&i| pts[i]).collect();
    let core_local: Vec<usize> = (0..supplied.len())
        .filter(|&k| is_core(bounds, local[k]))
        .collect();
    let normals = op.estimate(&local, &core_local);
    core_local
        .iter()
        .zip(normals)
        .map(|(&k, n)| (supplied[k], n))
        .collect()
}

fn assemble(op: &Normals, pts: &[[f64; 3]], halo: Halo) -> Vec<Option<[f64; 3]>> {
    let mut out = vec![None; pts.len()];
    let mut seen = vec![false; pts.len()];
    for b in partitions() {
        for (i, n) in run_partition(op, pts, &b, halo) {
            assert!(
                !seen[i],
                "point {i} is a core point of more than one partition — C-HALO 1 requires exactly one"
            );
            seen[i] = true;
            out[i] = n;
        }
    }
    // Every point belongs to some partition. C-HALO 1: the partitioning leaves no position within
    // its extent belonging to none.
    assert!(seen.iter().all(|&s| s), "some point was in no partition");
    out
}

fn reference(op: &Normals, pts: &[[f64; 3]]) -> Vec<Option<[f64; 3]>> {
    let all: Vec<usize> = (0..pts.len()).collect();
    op.estimate(pts, &all)
}

fn differences(a: &[Option<[f64; 3]>], b: &[Option<[f64; 3]>]) -> usize {
    a.iter()
        .zip(b)
        .filter(|(x, y)| match (x, y) {
            (Some(p), Some(q)) => {
                // Normals are unit vectors; compare by angle, not by float equality.
                let dot = p[0] * q[0] + p[1] * q[1] + p[2] * q[2];
                dot.abs() < 1.0 - 1e-9
            }
            (None, None) => false,
            _ => true,
        })
        .count()
}

#[test]
fn partitioned_with_declared_halo_matches_single_partition() {
    let op = Normals {
        radius: 1.5,
        max_neighbours: 8,
    };
    let pts = cloud();

    // The halo actually declared, taken from the declaration rather than restated — a test that
    // hardcoded the radius would keep passing if the declaration drifted from the search.
    let declared = op.declare().widest_halo();
    assert_eq!(
        declared.radius(),
        op.radius,
        "the declared halo must be the radius the operator searches"
    );

    let split = assemble(&op, &pts, declared);
    let whole = reference(&op, &pts);
    let diff = differences(&split, &whole);
    assert_eq!(
        diff, 0,
        "{diff} of {} normals differ from the single-partition reference",
        pts.len()
    );
}

#[test]
fn withholding_the_halo_changes_the_answer() {
    // The negative control. Without it the test above would pass against an operator that read no
    // neighbours at all, since such an operator is trivially partition-independent.
    let op = Normals {
        radius: 1.5,
        max_neighbours: 8,
    };
    let pts = cloud();
    let starved = assemble(&op, &pts, Halo::None);
    let whole = reference(&op, &pts);
    assert!(
        differences(&starved, &whole) > 0,
        "withholding the halo changed nothing, so this suite cannot detect a halo bug"
    );
}

#[test]
fn a_wider_halo_than_declared_changes_nothing() {
    // Necessity, the other half of GUARD-HALO-CORRECTNESS's name. If a halo wider than declared
    // improved the answer, the declaration would be too narrow and the operator wrong at
    // boundaries in a way the positive test could miss on this particular cloud.
    let op = Normals {
        radius: 1.5,
        max_neighbours: 8,
    };
    let pts = cloud();
    let declared = assemble(&op, &pts, op.declare().widest_halo());
    let generous = assemble(&op, &pts, Halo::Radius(op.radius * 3.0));
    assert_eq!(
        differences(&declared, &generous),
        0,
        "a wider halo changed the answer, so the declared halo is insufficient"
    );
}
