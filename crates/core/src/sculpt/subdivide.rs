// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Subdivision: every triangle split into four, with Loop's smoothing, so a
//! coarse mesh becomes a finer, rounder one.
//!
//! The finer mesh keeps the coarse one's vertices first, in their order,
//! then one new vertex per edge; its triangles come in fours, one group
//! per coarse triangle, in the coarse order. So the coarse triangles can
//! always be read back from the fine ones (see [`parents`]), which is
//! what lets the clay go up and down subdivision levels (`levels.rs`).
//!
//! Loop's scheme moves every vertex: a new one goes to 3/8 of each end of
//! its edge plus 1/8 of the two vertices facing it, an old one towards
//! the average of its ring. Along an edge that doesn't join exactly two
//! triangles (an open border, a fin) a new vertex stays at the midpoint
//! and the old vertices there stay put. Only arithmetic, no trigonometry:
//! the result is the same on every machine that rounds as IEEE 754 says.

use glam::Vec3;

use super::mesh::Mesh;

/// How a mesh splits into the next level: which edge each new vertex
/// sits on, and which new vertices border each triangle.
pub struct Split {
    /// Vertices before the split.
    old: usize,
    /// The two ends of each edge, in the order of their new vertices: the
    /// new vertex of edge `e` is vertex `old + e`.
    edges: Vec<[u32; 2]>,
    /// For each triangle `[a, b, c]`, the edges `ab`, `bc` and `ca`.
    sides: Vec<[u32; 3]>,
}

impl Split {
    /// The split of `mesh`. Edges are numbered by their lower vertex, then
    /// by where the other one comes in its ring, so the new vertices lie
    /// in the same spatial order as the old. `None` if a triangle has a
    /// corner twice: it has no edges to split.
    pub fn of(mesh: &Mesh) -> Option<Split> {
        let n = mesh.positions.len();
        let mut slot_edge = vec![u32::MAX; mesh.neighbours.len()];
        let mut edges = Vec::new();
        for v in 0..n as u32 {
            let start = mesh.neighbours.start(v);
            for (i, &u) in mesh.neighbours.of(v).iter().enumerate() {
                if u > v {
                    slot_edge[start + i] = edges.len() as u32;
                    edges.push([v, u]);
                }
            }
        }
        let edge = |a: u32, b: u32| -> Option<u32> {
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            let at = mesh.neighbours.of(lo).iter().position(|&u| u == hi)?;
            Some(slot_edge[mesh.neighbours.start(lo) + at])
        };
        let sides = mesh
            .triangles
            .iter()
            .map(|&[a, b, c]| Some([edge(a, b)?, edge(b, c)?, edge(c, a)?]))
            .collect::<Option<Vec<_>>>()?;
        Some(Split {
            old: n,
            edges,
            sides,
        })
    }

    /// Vertices after the split.
    pub fn len(&self) -> usize {
        self.old + self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The finer triangles: four per coarse triangle `[a, b, c]`, as
    /// `[a, ab, ca]`, `[b, bc, ab]`, `[c, ca, bc]` and `[ab, bc, ca]`.
    pub fn triangles(&self, mesh: &Mesh) -> Vec<[u32; 3]> {
        let old = self.old as u32;
        let mut triangles = Vec::with_capacity(mesh.triangles.len() * 4);
        for (&[a, b, c], s) in mesh.triangles.iter().zip(&self.sides) {
            let [ab, bc, ca] = s.map(|e| old + e);
            triangles.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        triangles
    }

    /// `values` at the old vertices, then each edge's average of its two
    /// ends: a plain midpoint split, without smoothing.
    pub fn midpoints(&self, values: &[Vec3]) -> Vec<Vec3> {
        let mut out = Vec::with_capacity(self.len());
        out.extend_from_slice(&values[..self.old]);
        out.extend(
            self.edges
                .iter()
                .map(|&[a, b]| (values[a as usize] + values[b as usize]) * 0.5),
        );
        out
    }

    /// Loop's scheme applied to `positions` on the mesh's triangles: the
    /// finer level's positions, old vertices first.
    pub fn positions(&self, mesh: &Mesh, positions: &[Vec3]) -> Vec<Vec3> {
        let e = self.edges.len();
        // The vertices facing each edge, summed, and how many triangles
        // it borders, in triangle order so the sums are always the same.
        let mut facing = vec![Vec3::ZERO; e];
        let mut faces = vec![0u8; e];
        for (&[a, b, c], s) in mesh.triangles.iter().zip(&self.sides) {
            for (edge, opposite) in s.iter().zip([c, a, b]) {
                facing[*edge as usize] += positions[opposite as usize];
                faces[*edge as usize] = faces[*edge as usize].saturating_add(1);
            }
        }
        // Old vertices on an edge that isn't between two triangles stay.
        let mut fixed = vec![false; self.old];
        for (&[a, b], &count) in self.edges.iter().zip(&faces) {
            if count != 2 {
                fixed[a as usize] = true;
                fixed[b as usize] = true;
            }
        }
        let mut out = Vec::with_capacity(self.len());
        for v in 0..self.old {
            let p = positions[v];
            let ring = mesh.neighbours.of(v as u32);
            if fixed[v] || ring.is_empty() {
                out.push(p);
                continue;
            }
            // Warren's weights: Loop's own, without the cosine.
            let k = ring.len() as f32;
            let beta = if ring.len() == 3 {
                3.0 / 16.0
            } else {
                3.0 / (8.0 * k)
            };
            let sum = ring
                .iter()
                .fold(Vec3::ZERO, |s, &u| s + positions[u as usize]);
            out.push(p * (1.0 - k * beta) + sum * beta);
        }
        for ((&[a, b], &count), &facing) in self.edges.iter().zip(&faces).zip(&facing) {
            let ends = positions[a as usize] + positions[b as usize];
            out.push(if count == 2 {
                ends * 0.375 + facing * 0.125
            } else {
                ends * 0.5
            });
        }
        out
    }
}

/// The quads (pairs of triangles, see `Mesh::quads`) of the finer mesh
/// split from `triangles` with `quads`: each quad becomes four, two across
/// the edge its triangles share and one inside each of them, so a
/// triangulated quad grid stays one, with its diagonals parallel. Pairs
/// that aren't two triangles sharing an edge are left out.
pub fn child_quads(triangles: &[[u32; 3]], quads: &[[u32; 2]]) -> Vec<[u32; 2]> {
    let mut out = Vec::with_capacity(quads.len() * 4);
    for &[p, q] in quads {
        let (Some(tp), Some(tq)) = (triangles.get(p as usize), triangles.get(q as usize)) else {
            continue;
        };
        // The corners of each that the other shares, and the one it doesn't.
        let mut shared = [(0, 0); 2];
        let mut count = 0;
        for (i, v) in tp.iter().enumerate() {
            if let Some(j) = tq.iter().position(|u| u == v) {
                if count < 2 {
                    shared[count] = (i, j);
                }
                count += 1;
            }
        }
        if p == q || count != 2 {
            continue;
        }
        let off = |at: [usize; 2]| (0..3).find(|i| !at.contains(i)).unwrap_or(0) as u32;
        let op = off([shared[0].0, shared[1].0]);
        let oq = off([shared[0].1, shared[1].1]);
        // Child i of a triangle is at its corner i; child 3 is the middle.
        let child = |t: u32, i: u32| 4 * t + i;
        for &(i, j) in &shared {
            out.push([child(p, i as u32), child(q, j as u32)]);
        }
        out.push([child(p, op), child(p, 3)]);
        out.push([child(q, oq), child(q, 3)]);
    }
    out
}

/// The quads of the coarser mesh that the triangles with `quads` were
/// split from: the pairs of coarse triangles that a quad straddles.
pub fn parent_quads(quads: &[[u32; 2]]) -> Vec<[u32; 2]> {
    let mut out: Vec<[u32; 2]> = quads
        .iter()
        .filter(|[s, t]| s / 4 != t / 4)
        .map(|[s, t]| [(s / 4).min(t / 4), (s / 4).max(t / 4)])
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Vertices the clay would have after [`subdivide`]: one more per edge.
pub fn subdivided_len(mesh: &Mesh) -> usize {
    mesh.positions.len()
        + (0..mesh.positions.len() as u32)
            .map(|v| mesh.neighbours.of(v).iter().filter(|&&u| u > v).count())
            .sum::<usize>()
}

/// The mesh with every triangle split into four and smoothed, its
/// vertices in the order [`Split`] gives. `None` if a triangle has a
/// corner twice.
pub fn subdivide(mesh: &Mesh) -> Option<Mesh> {
    let split = Split::of(mesh)?;
    Some(Mesh::from_ordered(
        split.positions(mesh, &mesh.positions),
        split.triangles(mesh),
    ))
}

/// The coarse triangles that `triangles` were split from (see
/// [`Split::triangles`]), the coarse mesh having `coarse` vertices. `None`
/// unless every group of four has that shape, with the corners among the
/// coarse vertices and the edge vertices after them.
pub fn parents(triangles: &[[u32; 3]], coarse: usize) -> Option<Vec<[u32; 3]>> {
    let (groups, rest) = triangles.as_chunks::<4>();
    if !rest.is_empty() {
        return None;
    }
    let coarse = u32::try_from(coarse).ok()?;
    groups
        .iter()
        .map(|&four| {
            let [[a, ab, ca], [b, bc, ab1], [c, ca1, bc1], last] = four;
            let shaped = ab1 == ab && ca1 == ca && bc1 == bc && last == [ab, bc, ca];
            let corners = a < coarse && b < coarse && c < coarse;
            let middles = ab >= coarse && bc >= coarse && ca >= coarse;
            (shaped && corners && middles).then_some([a, b, c])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn closed(mesh: &Mesh) -> bool {
        let mut edges = HashMap::new();
        for &[a, b, c] in &mesh.triangles {
            for e in [(a, b), (b, c), (c, a)] {
                *edges.entry(e).or_insert(0) += 1;
            }
        }
        edges.values().all(|&n| n == 1) && edges.keys().all(|&(a, b)| edges.contains_key(&(b, a)))
    }

    #[test]
    fn splits_every_triangle_in_four_and_stays_closed() {
        let mesh = Mesh::icosphere(2);
        let fine = subdivide(&mesh).unwrap();
        assert_eq!(fine.triangles.len(), mesh.triangles.len() * 4);
        assert_eq!(fine.positions.len(), subdivided_len(&mesh));
        // The same as one more level of the icosphere, in size.
        assert_eq!(fine.positions.len(), Mesh::icosphere(3).positions.len());
        assert!(closed(&fine));
        for &t in &fine.triangles {
            let [a, b, c] = t.map(|v| fine.positions[v as usize]);
            assert!((b - a).cross(c - a).dot(a + b + c) > 0.0, "winds inward");
        }
        // The coarse triangles read back from the fine ones.
        assert_eq!(
            parents(&fine.triangles, mesh.positions.len()).as_deref(),
            Some(mesh.triangles.as_slice())
        );
    }

    #[test]
    fn the_surface_comes_out_smooth_and_round() {
        // A coarse icosahedron, subdivided three times: Loop's surface is
        // smooth, so the sphere's bumps even out level by level.
        let mut mesh = Mesh::icosphere(0);
        let spread = |m: &Mesh| {
            let r: Vec<f32> = m.positions.iter().map(|p| p.length()).collect();
            let (lo, hi) = r
                .iter()
                .fold((f32::MAX, 0f32), |(lo, hi), &r| (lo.min(r), hi.max(r)));
            (hi - lo) / hi
        };
        let first = spread(&mesh);
        for _ in 0..3 {
            mesh = subdivide(&mesh).unwrap();
        }
        assert!(first < 1e-6, "the icosahedron's corners are on the sphere");
        // Plain midpoints would leave the facets' centres 21 % lower.
        assert!(spread(&mesh) < 0.03, "{}", spread(&mesh));
        assert!(closed(&mesh));
    }

    #[test]
    fn an_open_border_keeps_its_corners_and_midpoints() {
        // Two triangles making a square: every edge but the diagonal is a
        // border.
        let positions = vec![Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y];
        let mesh = Mesh::from_ordered(positions.clone(), vec![[0, 1, 2], [0, 2, 3]]);
        let fine = subdivide(&mesh).unwrap();
        assert_eq!(&fine.positions[..4], &positions[..]);
        assert_eq!(fine.positions.len(), 9);
        assert!(fine.positions.iter().all(|p| p.z == 0.0));
    }

    #[test]
    fn a_triangle_with_a_corner_twice_has_no_split() {
        let positions = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z];
        let mesh = Mesh::from_ordered(positions, vec![[0, 1, 2], [0, 2, 1], [3, 3, 1]]);
        assert!(subdivide(&mesh).is_none());
    }

    #[test]
    fn parents_refuse_what_no_split_made() {
        let mesh = Mesh::icosphere(1);
        let fine = subdivide(&mesh).unwrap();
        let n = mesh.positions.len();
        assert!(parents(&fine.triangles[..fine.triangles.len() - 1], n).is_none());
        let mut swapped = fine.triangles.clone();
        swapped.swap(0, 1);
        assert!(parents(&swapped, n).is_none());
        // Too few coarse vertices: a corner would be an edge vertex.
        assert!(parents(&fine.triangles, n - 1).is_none());
        assert!(parents(&mesh.triangles, 0).is_none());
    }

    /// How heavy the clay gets: subdividing the default sphere twice, and
    /// what a frame of sculpting costs at each size.
    /// `cargo test --release -p splatstudy-core subdivide_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn subdivide_timing() {
        use std::time::Instant;

        use crate::sculpt::{Brush, Dab, Sculpt};
        let ms = |since: Instant| since.elapsed().as_secs_f64() * 1e3;
        let mut mesh = Mesh::icosphere(crate::sculpt::OLD_START_LEVEL);
        for round in 0..3 {
            let n = mesh.positions.len();
            let start = Instant::now();
            let mut s = Sculpt::new(mesh.clone());
            let t_sculpt = ms(start);
            // A frame: four dabs of a mid-sized brush, then the refit.
            let frames = 30;
            let (mut t_dabs, mut t_refit) = (0.0, 0.0);
            s.begin_stroke();
            for f in 0..frames {
                let start = Instant::now();
                for d in 0..4 {
                    let x = -0.5 + (f * 4 + d) as f32 * 0.008;
                    s.dab(Dab {
                        brush: Brush::Clay,
                        centre: Vec3::new(x, 0.2, 0.9).normalize(),
                        radius: 0.2,
                        strength: 0.5,
                        invert: false,
                        symmetry: true,
                        direction: Vec3::X,
                        buildup: Default::default(),
                        fill: false,
                        wipes: false,
                    });
                }
                t_dabs += ms(start);
                let start = Instant::now();
                s.refresh();
                t_refit += ms(start);
            }
            s.end_stroke();
            println!(
                "{n} vertices: sculpt setup {t_sculpt:.0} ms; per frame dabs {:.1} + refit {:.1} ms",
                t_dabs / frames as f64,
                t_refit / frames as f64,
            );
            if round < 2 {
                let start = Instant::now();
                mesh = subdivide(&mesh).unwrap();
                println!(
                    "  subdivided to {} vertices in {:.0} ms",
                    mesh.positions.len(),
                    ms(start)
                );
            }
        }
    }
}
