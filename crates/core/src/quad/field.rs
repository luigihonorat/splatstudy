// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The two fields of an instant field-aligned mesh (after Jakob, Tarini,
//! Panozzo and Sorkine-Hornung, 2015).
//!
//! - **Orientation:** at each sample, a direction in its tangent plane,
//!   standing for four (it turned by quarter turns about the normal): the
//!   directions the quad edges run there. Smoothing makes neighbours agree
//!   as closely as their normals allow, which lines the edges up with the
//!   shape's main curves.
//! - **Position:** at each sample, a point near it in its tangent plane,
//!   standing for a square grid of points `scale` apart along the
//!   orientation: the corners of the quads there. Smoothing makes
//!   neighbours' grids meet.

use glam::Vec3;

use super::graph::Hierarchy;

/// Smoothing passes on each level, and more on the finest, which the
/// mesh is read from.
const ORIENTATION_PASSES: usize = 6;
const POSITION_PASSES: usize = 6;
const FINEST_PASSES: usize = 8;

/// Of the four directions `o0` stands for (about `n0`) and the four `o1`
/// stands for (about `n1`), the closest pair.
pub fn match_orientation(o0: Vec3, n0: Vec3, o1: Vec3, n1: Vec3) -> (Vec3, Vec3) {
    let a = [o0, n0.cross(o0)];
    let b = [o1, n1.cross(o1)];
    let (mut best, mut pick) = (-1.0, (0, 0));
    for (i, ai) in a.iter().enumerate() {
        for (j, bj) in b.iter().enumerate() {
            let d = ai.dot(*bj).abs();
            if d > best {
                best = d;
                pick = (i, j);
            }
        }
    }
    let (ai, bj) = (a[pick.0], b[pick.1]);
    (ai, if ai.dot(bj) < 0.0 { -bj } else { bj })
}

/// A point on both tangent planes (`v0`, `n0`) and (`v1`, `n1`), as near
/// their midpoint as possible: where the two grids are compared.
fn meeting_point(v0: Vec3, n0: Vec3, v1: Vec3, n1: Vec3) -> Vec3 {
    let mid = (v0 + v1) * 0.5;
    let c = n0.dot(n1);
    let det = 1.0 - c * c;
    if det < 1e-4 {
        return mid;
    }
    // mid + α n0 + β n1 lies on both planes.
    let (r0, r1) = (n0.dot(v0 - mid), n1.dot(v1 - mid));
    let alpha = (r0 - c * r1) / det;
    let beta = (r1 - c * r0) / det;
    let x = mid + n0 * alpha + n1 * beta;
    // Nearly parallel planes meet far away: compare at the midpoint then.
    if x.distance_squared(mid) > v0.distance_squared(v1) {
        mid
    } else {
        x
    }
}

/// The grid point of origin `p`, axes `q` and `t`, spacing `scale`, nearest
/// to `target`.
pub fn round_to_grid(p: Vec3, q: Vec3, t: Vec3, target: Vec3, scale: f32) -> Vec3 {
    let d = target - p;
    p + q * ((q.dot(d) / scale).round() * scale) + t * ((t.dot(d) / scale).round() * scale)
}

/// One sample: where it is, its normal, its matched direction and grid origin.
#[derive(Clone, Copy)]
pub struct Frame {
    pub v: Vec3,
    pub n: Vec3,
    pub q: Vec3,
    pub p: Vec3,
}

/// A point of each sample's grid, the two nearest each other around
/// where the samples' tangent planes meet. `q` of both must be matched.
pub fn match_position(a: Frame, b: Frame, scale: f32) -> (Vec3, Vec3) {
    let (ta, tb) = (a.n.cross(a.q), b.n.cross(b.q));
    let middle = meeting_point(a.v, a.n, b.v, b.n);
    let x = round_to_grid(a.p, a.q, ta, middle, scale);
    let y = round_to_grid(b.p, b.q, tb, x, scale);
    (x, y)
}

/// Some direction in the plane normal to `n`, different for each `seed`.
fn any_tangent(n: Vec3, seed: u32) -> Vec3 {
    let mut h = seed.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    let mut next = || {
        h ^= h >> 15;
        h = h.wrapping_mul(0x2C1B_3C6D);
        h ^= h >> 12;
        h = h.wrapping_mul(0x297A_2D39);
        h ^= h >> 15;
        (h as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    let r = Vec3::new(next(), next(), next());
    (r - n * n.dot(r)).normalize_or(n.any_orthonormal_vector())
}

/// The orientation field on the finest level.
pub fn orientation(h: &Hierarchy, seed: u32) -> Vec<Vec3> {
    let Some(coarsest) = h.levels.last() else {
        return Vec::new();
    };
    let mut o: Vec<Vec3> = (0..coarsest.len())
        .map(|i| any_tangent(coarsest.normals[i], seed.wrapping_add(i as u32)))
        .collect();
    for k in (0..h.levels.len()).rev() {
        let g = &h.levels[k];
        if k + 1 < h.levels.len() {
            // Handed down from the coarser level.
            o = h.parents[k]
                .iter()
                .enumerate()
                .map(|(i, &p)| {
                    let (n, op) = (g.normals[i], o[p as usize]);
                    (op - n * n.dot(op)).normalize_or(n.any_orthonormal_vector())
                })
                .collect();
        }
        let passes = if k == 0 {
            FINEST_PASSES
        } else {
            ORIENTATION_PASSES
        };
        for _ in 0..passes {
            for i in 0..g.len() {
                let n = g.normals[i];
                let mut sum = o[i];
                let mut weight = 0.0;
                for &j in g.of(i) {
                    let j = j as usize;
                    let (a, b) = match_orientation(sum, n, o[j], g.normals[j]);
                    sum = a * weight + b;
                    sum -= n * n.dot(sum);
                    sum = sum.normalize_or(a);
                    weight += 1.0;
                }
                o[i] = sum;
            }
        }
    }
    o
}

/// The position field on the finest level, for the finest orientation
/// `o` and grid spacing `scale`.
pub fn position(h: &Hierarchy, o_fine: &[Vec3], scale: f32) -> Vec<Vec3> {
    // The orientation on every level, carried up from the finest by
    // averaging (matched) children.
    let mut orientations = vec![o_fine.to_vec()];
    for k in 0..h.parents.len() {
        let (fine, coarse) = (&h.levels[k], &h.levels[k + 1]);
        let below = &orientations[k];
        let mut up = vec![Vec3::ZERO; coarse.len()];
        for (i, &p) in h.parents[k].iter().enumerate() {
            let p = p as usize;
            let n = coarse.normals[p];
            let o = below[i];
            up[p] = if up[p] == Vec3::ZERO {
                o
            } else {
                let (a, b) = match_orientation(up[p], n, o, fine.normals[i]);
                a + b
            };
        }
        let up = up
            .iter()
            .enumerate()
            .map(|(c, &o)| {
                let n = coarse.normals[c];
                (o - n * n.dot(o)).normalize_or(n.any_orthonormal_vector())
            })
            .collect();
        orientations.push(up);
    }
    let Some(coarsest) = h.levels.last() else {
        return Vec::new();
    };
    let mut p: Vec<Vec3> = coarsest.positions.clone();
    for k in (0..h.levels.len()).rev() {
        let g = &h.levels[k];
        let o = &orientations[k];
        if k + 1 < h.levels.len() {
            p = h.parents[k]
                .iter()
                .enumerate()
                .map(|(i, &parent)| {
                    let (v, n) = (g.positions[i], g.normals[i]);
                    let pp = p[parent as usize];
                    let pp = pp - n * n.dot(pp - v);
                    round_to_grid(pp, o[i], n.cross(o[i]), v, scale)
                })
                .collect();
        }
        // The orientations stay put now: each link's matched pair, once.
        let mut matched = Vec::with_capacity(g.links().len());
        for i in 0..g.len() {
            for &j in g.of(i) {
                let j = j as usize;
                matched.push(match_orientation(o[i], g.normals[i], o[j], g.normals[j]));
            }
        }
        let passes = if k == 0 {
            FINEST_PASSES
        } else {
            POSITION_PASSES
        };
        for _ in 0..passes {
            for i in 0..g.len() {
                let (v, n) = (g.positions[i], g.normals[i]);
                let mut sum = p[i];
                let mut weight = 0.0;
                let range = g.links_of(i);
                for (&j, &(qi, qj)) in g.links()[range.clone()].iter().zip(&matched[range]) {
                    let j = j as usize;
                    let here = Frame {
                        v,
                        n,
                        q: qi,
                        p: sum,
                    };
                    let there = Frame {
                        v: g.positions[j],
                        n: g.normals[j],
                        q: qj,
                        p: p[j],
                    };
                    let (a, b) = match_position(here, there, scale);
                    sum = (a * weight + b) / (weight + 1.0);
                    sum -= n * n.dot(sum - v);
                    weight += 1.0;
                }
                p[i] = round_to_grid(sum, o[i], n.cross(o[i]), v, scale);
            }
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_finds_the_quarter_turn_that_agrees() {
        let n = Vec3::Z;
        let (a, b) = match_orientation(Vec3::X, n, Vec3::new(-0.1, 1.0, 0.0).normalize(), n);
        assert!(a.dot(b) > 0.99, "{a} {b}");
    }

    #[test]
    fn neighbouring_grids_meet_at_a_shared_point() {
        let scale = 0.1;
        let a = Frame {
            v: Vec3::ZERO,
            n: Vec3::Z,
            q: Vec3::X,
            p: Vec3::new(0.01, 0.0, 0.0),
        };
        let b = Frame {
            v: Vec3::new(0.05, 0.0, 0.0),
            n: Vec3::Z,
            q: Vec3::Y,
            p: Vec3::new(0.11, 0.1, 0.0),
        };
        let (qa, qb) = match_orientation(a.q, a.n, b.q, b.n);
        let (x, y) = match_position(Frame { q: qa, ..a }, Frame { q: qb, ..b }, scale);
        assert!(x.distance(y) < 1e-5, "{x} {y}");
    }
}
