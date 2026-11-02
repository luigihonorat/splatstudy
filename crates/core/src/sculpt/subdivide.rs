// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Subdivision: every triangle split into four, for finer detail where the
//! clay is already shaped.
//!
//! The old vertices stay where they are, so nothing sculpted is lost. Each
//! edge gets a new vertex at its midpoint, which then moves off the flat
//! triangle towards the curved surface that the normals at the edge's two
//! ends describe (as Phong tessellation does): without that, every old
//! triangle would stay a flat facet.

use glam::Vec3;

use super::mesh::Mesh;

/// How far a new vertex moves from the edge's midpoint towards the tangent
/// planes at its ends. One half puts it back on a sphere, to second order.
const BULGE: f32 = 0.5;

/// Vertices the clay would have after [`subdivide`]: one more per edge.
pub fn subdivided_len(mesh: &Mesh) -> usize {
    mesh.positions.len() + edge_count(mesh)
}

fn edge_count(mesh: &Mesh) -> usize {
    (0..mesh.positions.len() as u32)
        .map(|v| mesh.neighbours.of(v).iter().filter(|&&u| u > v).count())
        .sum()
}

/// The mesh with every triangle split into four, its new vertices eased
/// onto the curve of the surface.
pub fn subdivide(mesh: &Mesh) -> Mesh {
    let mut positions = mesh.positions.clone();
    positions.reserve(edge_count(mesh));
    // The new vertex of each edge, stored at the edge's place in the
    // neighbour list of its lower vertex.
    // `None` for a pair that isn't an edge: a vertex with itself, in a
    // triangle with a corner twice.
    let slot = |a: u32, b: u32| -> Option<usize> {
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        let at = mesh.neighbours.of(lo).iter().position(|&u| u == hi)?;
        Some(mesh.neighbours.start(lo) + at)
    };
    let mut middle = vec![u32::MAX; mesh.neighbours.len()];
    let mut midpoint = |s: usize, a: u32, b: u32, positions: &mut Vec<Vec3>| -> u32 {
        if middle[s] == u32::MAX {
            middle[s] = positions.len() as u32;
            positions.push(eased_midpoint(mesh, a, b));
        }
        middle[s]
    };
    let mut triangles = Vec::with_capacity(mesh.triangles.len() * 4);
    for &[a, b, c] in &mesh.triangles {
        // A triangle with no area has nothing to split: it is left out.
        let (Some(sab), Some(sbc), Some(sca)) = (slot(a, b), slot(b, c), slot(c, a)) else {
            continue;
        };
        let ab = midpoint(sab, a, b, &mut positions);
        let bc = midpoint(sbc, b, c, &mut positions);
        let ca = midpoint(sca, c, a, &mut positions);
        triangles.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
    }
    Mesh::new(positions, triangles)
}

/// The midpoint of edge `ab`, moved towards the tangent planes at `a` and
/// `b`.
fn eased_midpoint(mesh: &Mesh, a: u32, b: u32) -> Vec3 {
    let (pa, pb) = (mesh.positions[a as usize], mesh.positions[b as usize]);
    let (na, nb) = (mesh.normals[a as usize], mesh.normals[b as usize]);
    let m = (pa + pb) * 0.5;
    let on_a = m - na * (m - pa).dot(na);
    let on_b = m - nb * (m - pb).dot(nb);
    m.lerp((on_a + on_b) * 0.5, BULGE)
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
        let fine = subdivide(&mesh);
        assert_eq!(fine.triangles.len(), mesh.triangles.len() * 4);
        assert_eq!(fine.positions.len(), subdivided_len(&mesh));
        // The same as one more level of the icosphere, in size.
        assert_eq!(fine.positions.len(), Mesh::icosphere(3).positions.len());
        assert!(closed(&fine));
        // Every old vertex is still there, exactly.
        for p in &mesh.positions {
            assert!(fine.positions.contains(p));
        }
        for &t in &fine.triangles {
            let [a, b, c] = t.map(|v| fine.positions[v as usize]);
            assert!((b - a).cross(c - a).dot(a + b + c) > 0.0, "winds inward");
        }
    }

    #[test]
    fn new_vertices_follow_the_curve_instead_of_the_facets() {
        let mesh = Mesh::icosphere(2);
        let fine = subdivide(&mesh);
        // Plain midpoints of a level-2 icosphere sink up to ~1.6 % inside
        // the sphere; eased ones stay much closer to it.
        let worst = fine
            .positions
            .iter()
            .map(|p| (p.length() - 1.0).abs())
            .fold(0.0, f32::max);
        assert!(worst < 0.003, "{worst}");
    }

    #[test]
    fn a_triangle_with_a_corner_twice_is_left_out_not_a_panic() {
        let positions = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z];
        let mesh = Mesh::from_ordered(positions, vec![[0, 1, 2], [0, 2, 1], [3, 3, 1]]);
        let fine = subdivide(&mesh);
        assert_eq!(fine.triangles.len(), 8);
        assert!(
            fine.triangles
                .iter()
                .flatten()
                .all(|&v| (v as usize) < fine.positions.len())
        );
    }

    #[test]
    fn subdividing_through_the_sculpt_undoes_and_replays() {
        use crate::sculpt::{Replay, Sculpt};
        let mut s = Sculpt::new(Mesh::icosphere(3));
        let coarse = s.mesh.positions.clone();
        let fine = subdivide(&s.mesh);
        s.set_clock(1.0);
        s.replace_mesh(fine);
        assert_eq!(s.mesh.triangles.len(), 1280 * 4);
        let mut replay = Replay::of(&s);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_eq!(replay.sculpt.mesh.positions, s.mesh.positions);
        assert_eq!(replay.sculpt.mesh.triangles, s.mesh.triangles);
        s.undo();
        assert_eq!(s.mesh.positions, coarse);
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
        let mut mesh = Mesh::icosphere(crate::sculpt::DEFAULT_LEVEL);
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
                mesh = subdivide(&mesh);
                println!(
                    "  subdivided to {} vertices in {:.0} ms",
                    mesh.positions.len(),
                    ms(start)
                );
            }
        }
    }
}
