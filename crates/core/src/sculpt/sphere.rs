// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The clay a new scene starts from: a sphere with the topology of a
//! subdivided cube, with its subdivision levels already built, so the
//! broad forms can be blocked in at once a few levels down (as in ZBrush).
//!
//! Level 0 is the cube: 8 corners, each face split by one diagonal. Every
//! level above splits each triangle into four (see [`Split`]), so a face
//! stays a grid of squares, each split by a diagonal parallel to the
//! face's own: level k has 2^k squares along each cube edge.
//!
//! Every level is that cube grid on the unit sphere. Each point of the
//! cube is first spread with tan(x·π/4) along each axis, which evens out
//! the squares' sizes (projected directly, they would crowd towards the
//! cube's corners), then pushed out to the sphere. The levels' detail is
//! whatever makes each level that sphere over the level below smoothed up
//! with Loop's scheme.

use glam::{DVec3, Vec3};

use super::levels::Levels;
use super::mesh::Mesh;
use super::subdivide::Split;

/// The level the clay starts at: 128 squares along each cube edge,
/// 98,306 vertices. Two more subdivisions reach 1,572,866.
pub const START_LEVEL: usize = 7;

/// The cube's corners, at ±1, and its twelve triangles, wound
/// counter-clockwise seen from outside.
fn cube() -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let corner =
        |p: Vec3| u32::from(p.x > 0.0) << 2 | u32::from(p.y > 0.0) << 1 | u32::from(p.z > 0.0);
    let corners = (0..8)
        .map(|i| {
            let s = |bit: u32| if i & bit != 0 { 1.0 } else { -1.0 };
            Vec3::new(s(4), s(2), s(1))
        })
        .collect();
    let mut triangles = Vec::with_capacity(12);
    for axis in 0..3 {
        for side in [-1.0, 1.0] {
            let normal = Vec3::AXES[axis] * side;
            let (u, v) = (Vec3::AXES[(axis + 1) % 3], Vec3::AXES[(axis + 2) % 3]);
            let quad = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                .map(|(i, j)| corner(normal + u * i + v * j));
            let at =
                |k: usize| normal + u * [-1.0, 1.0, 1.0, -1.0][k] + v * [-1.0, -1.0, 1.0, 1.0][k];
            let outward = (at(1) - at(0)).cross(at(2) - at(0)).dot(normal) > 0.0;
            let [a, b, c, d] = if outward {
                quad
            } else {
                [quad[0], quad[3], quad[2], quad[1]]
            };
            triangles.extend([[a, b, c], [a, c, d]]);
        }
    }
    (corners, triangles)
}

/// A point of the cube's surface on the unit sphere: spread with
/// tan(x·π/4) along each axis so the squares come out of even size, then
/// pushed out.
fn on_sphere(p: Vec3) -> Vec3 {
    let spread = p
        .as_dvec3()
        .to_array()
        .map(|x| (x * std::f64::consts::FRAC_PI_4).tan());
    DVec3::from_array(spread).normalize().as_vec3()
}

/// The start clay at `level` (see [`START_LEVEL`]), with every level from
/// the cube up.
pub fn cube_sphere(level: usize) -> (Mesh, Levels) {
    let (mut cube, triangles) = cube();
    let mut base = Mesh::from_ordered(cube.iter().map(|&p| on_sphere(p)).collect(), triangles);
    // Each face's two triangles, for export as quads; every level keeps them.
    base.quads = (0..6).map(|f| [2 * f, 2 * f + 1]).collect();
    Levels::build(base, level, |_, split: &Split, _| {
        // The finer grid on the cube: plain midpoints, which are exact.
        cube = split.midpoints(&cube);
        cube.iter().map(|&p| on_sphere(p)).collect()
    })
    .expect("the cube has no broken triangles")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sculpt::{Brush, Dab, Replay, Sculpt};

    #[test]
    fn the_cube_is_closed_and_faces_out() {
        let (corners, triangles) = cube();
        let mesh = Mesh::from_ordered(corners, triangles);
        assert_eq!(mesh.triangles.len(), 12);
        for &t in &mesh.triangles {
            let [a, b, c] = t.map(|v| mesh.positions[v as usize]);
            assert!((b - a).cross(c - a).dot(a + b + c) > 0.0);
        }
        // Each corner meets three faces; each edge joins two triangles.
        for v in 0..8 {
            let ring = mesh.neighbours.of(v).len();
            assert!(ring == 3 || ring == 4 || ring == 5 || ring == 6);
        }
        assert_eq!(crate::sculpt::subdivide::subdivided_len(&mesh), 8 + 18);
    }

    /// Every level is a grid of squares on each cube face, each split by a
    /// diagonal parallel to its face's: on the cube, every triangle is half
    /// a square of side 2/2^k, its long side along the face's diagonal.
    #[test]
    fn every_level_stays_a_grid_of_squares_with_parallel_diagonals() {
        let (corners, triangles) = cube();
        let mut cube = corners;
        let mut mesh = Mesh::from_ordered(cube.clone(), triangles);
        // The diagonal of each face, from its two triangles at level 0.
        let diagonal_of = |cube: &[Vec3], t: [u32; 3]| {
            let p = t.map(|v| cube[v as usize]);
            let normal = (p[1] - p[0]).cross(p[2] - p[0]).normalize();
            let long = [(0, 1), (1, 2), (2, 0)]
                .into_iter()
                .max_by(|a, b| p[a.0].distance(p[a.1]).total_cmp(&p[b.0].distance(p[b.1])))
                .unwrap();
            (normal.round(), (p[long.1] - p[long.0]).normalize())
        };
        let faces: Vec<(Vec3, Vec3)> = mesh
            .triangles
            .iter()
            .map(|&t| diagonal_of(&cube, t))
            .collect();
        for level in 1..=5 {
            let split = Split::of(&mesh).unwrap();
            cube = split.midpoints(&cube);
            mesh = Mesh::from_ordered(cube.clone(), split.triangles(&mesh));
            let n = 1 << level;
            assert_eq!(mesh.positions.len(), 6 * n * n + 2);
            let side = 2.0 / n as f32;
            for (i, &t) in mesh.triangles.iter().enumerate() {
                let p = t.map(|v| cube[v as usize]);
                let mut sides = [
                    p[0].distance(p[1]),
                    p[1].distance(p[2]),
                    p[2].distance(p[0]),
                ];
                sides.sort_by(f32::total_cmp);
                assert!((sides[0] - side).abs() < 1e-6 && (sides[1] - side).abs() < 1e-6);
                assert!((sides[2] - side * 2f32.sqrt()).abs() < 1e-6);
                // Each triangle descends from one of level 0's twelve.
                let (normal, diagonal) = diagonal_of(&cube, t);
                let (face_normal, face_diagonal) = faces[i >> (2 * level)];
                assert_eq!(normal, face_normal, "level {level}: off its face");
                assert!(diagonal.dot(face_diagonal).abs() > 0.9999, "level {level}");
            }
        }
    }

    #[test]
    fn the_start_clay_is_closed_with_every_vertex_apart() {
        let (mesh, _) = cube_sphere(START_LEVEL);
        let mut edges = std::collections::HashMap::new();
        for &[a, b, c] in &mesh.triangles {
            for e in [(a, b), (b, c), (c, a)] {
                *edges.entry(e).or_insert(0) += 1;
            }
        }
        assert!(
            edges.values().all(|&n| n == 1),
            "an edge twice the same way"
        );
        assert!(
            edges.keys().all(|&(a, b)| edges.contains_key(&(b, a))),
            "an open edge"
        );
        let mut seen = std::collections::HashSet::new();
        for p in &mesh.positions {
            assert!(
                seen.insert(p.to_array().map(f32::to_bits)),
                "two vertices at {p}"
            );
        }
        for &t in &mesh.triangles {
            let [a, b, c] = t.map(|v| mesh.positions[v as usize]);
            let n = (b - a).cross(c - a);
            assert!(n.dot(a + b + c) > 0.0, "a triangle facing in");
            assert!(n.length() > 1e-7, "a sliver");
        }
    }

    #[test]
    fn every_level_exports_as_quads() {
        let (mesh, levels) = cube_sphere(3);
        let quads = |m: &Mesh| {
            let obj = String::from_utf8(crate::meshio::obj::write(m)).unwrap();
            let faces: Vec<usize> = obj
                .lines()
                .filter(|l| l.starts_with("f "))
                .map(|l| l.split_whitespace().count() - 1)
                .collect();
            assert!(faces.iter().all(|&n| n == 4), "a triangle left over");
            faces.len()
        };
        assert_eq!(quads(&mesh), 6 * 8 * 8);
        let mut s = Sculpt::with_levels(mesh, Some(levels));
        let top = s.mesh.quads.clone();
        for level in (0..3).rev() {
            assert!(s.change_level(false));
            let n = 1 << level;
            assert_eq!(quads(&s.mesh), 6 * n * n, "level {level}");
        }
        while s.change_level(true) {}
        assert_eq!(s.mesh.quads, top);
        s.subdivide();
        assert_eq!(quads(&s.mesh), 6 * 16 * 16);
    }

    #[test]
    fn the_start_clay_is_a_round_even_sphere() {
        let (mesh, levels) = cube_sphere(5);
        assert_eq!(levels.current(), 5);
        assert_eq!(levels.counts(), &[8, 26, 98, 386, 1538, 6146]);
        for p in &mesh.positions {
            assert!((p.length() - 1.0).abs() < 1e-5);
        }
        // The squares come out of even size: their sides (each triangle's
        // two shorter edges) vary much less than a plain projection's.
        let spread = |mesh: &Mesh| {
            let sides: Vec<f32> = mesh
                .triangles
                .iter()
                .flat_map(|t| {
                    let p = t.map(|v| mesh.positions[v as usize]);
                    let mut e = [
                        p[0].distance(p[1]),
                        p[1].distance(p[2]),
                        p[2].distance(p[0]),
                    ];
                    e.sort_by(f32::total_cmp);
                    [e[0], e[1]]
                })
                .collect();
            let short = sides.iter().copied().fold(f32::MAX, f32::min);
            let long = sides.iter().copied().fold(0.0, f32::max);
            short / long
        };
        let (corners, triangles) = cube();
        let mut plain = Mesh::from_ordered(corners.clone(), triangles);
        let mut grid = corners;
        for _ in 0..5 {
            let split = Split::of(&plain).unwrap();
            grid = split.midpoints(&grid);
            plain = Mesh::from_ordered(grid.clone(), split.triangles(&plain));
        }
        plain.positions = grid.iter().map(|p| p.normalize()).collect();
        assert!(spread(&mesh) > 0.65, "{}", spread(&mesh));
        assert!(spread(&plain) < spread(&mesh) - 0.1, "{}", spread(&plain));
    }

    #[test]
    fn stepping_down_shows_the_coarser_spheres_and_back_up_is_exact() {
        let (mesh, levels) = cube_sphere(4);
        let mut s = Sculpt::with_levels(mesh, Some(levels));
        let top = s.mesh.positions.clone();
        for level in (0..4).rev() {
            assert!(s.change_level(false));
            assert_eq!(s.levels(), Some((level, 5)));
            let n = 1 << level;
            assert_eq!(s.mesh.positions.len(), 6 * n * n + 2);
            for p in &s.mesh.positions {
                assert!((p.length() - 1.0).abs() < 1e-5, "level {level}: {p}");
            }
        }
        while s.change_level(true) {}
        assert_eq!(s.mesh.positions, top);
    }

    fn stroke(s: &mut Sculpt, brush: Brush, at: Vec3, radius: f32) {
        s.begin_stroke();
        for i in 0..8 {
            s.dab(Dab {
                brush,
                centre: (at + Vec3::Y * (i as f32 * 0.03)).normalize(),
                radius,
                strength: 0.8,
                invert: false,
                symmetry: true,
                direction: if i == 0 { Vec3::ZERO } else { Vec3::Y },
            });
            s.refresh();
        }
        s.end_stroke();
    }

    #[test]
    fn a_session_from_the_start_clay_replays_bit_for_bit() {
        let (mesh, levels) = cube_sphere(4);
        let mut s = Sculpt::with_levels(mesh, Some(levels));
        assert!(
            s.checkpoints()[0].levels.is_some(),
            "the start keeps its levels"
        );
        s.set_clock(1.0);
        stroke(&mut s, Brush::Crease, Vec3::new(0.1, 0.2, 1.0), 0.1);
        s.set_clock(2.0);
        s.change_level(false);
        s.change_level(false);
        stroke(&mut s, Brush::Inflate, Vec3::new(-0.3, 0.0, 1.0), 0.5);
        s.set_clock(3.0);
        s.change_level(true);
        s.change_level(true);
        s.subdivide();
        stroke(&mut s, Brush::Clay, Vec3::new(0.3, -0.2, 1.0), 0.2);
        let mut replay = Replay::of(&s);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_eq!(replay.sculpt.mesh.triangles, s.mesh.triangles);
        assert_eq!(replay.sculpt.mesh.positions, s.mesh.positions);
        assert_eq!(replay.sculpt.level_data(), s.level_data());
    }

    /// Opens every scene in the folder `SPLATSTUDY_OLD_SCENES` names (saved
    /// by an earlier version), replays it to the end and checks it lands on
    /// the saved clay; then sculpts on and checks the replay again.
    /// `SPLATSTUDY_OLD_SCENES=... cargo test -p splatstudy-core old_scenes -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn old_scenes_still_open_and_replay() {
        use crate::scene::Scene;
        let dir = std::env::var("SPLATSTUDY_OLD_SCENES").expect("SPLATSTUDY_OLD_SCENES");
        let bits = |p: &[Vec3]| {
            p.iter()
                .flat_map(|p| p.to_array().map(f32::to_bits))
                .collect::<Vec<_>>()
        };
        let mut seen = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "splatstudy") {
                continue;
            }
            let scene = Scene::read(&std::fs::read(&path).unwrap()).unwrap();
            assert!(!scene.history_damaged, "{path:?}");
            let mut replay = Replay::new(scene.checkpoints.clone(), scene.log.clone());
            assert!(replay.seek(f64::INFINITY, usize::MAX));
            assert_eq!(
                bits(&replay.sculpt.mesh.positions),
                bits(&scene.mesh.positions),
                "{path:?}"
            );
            assert_eq!(
                replay.sculpt.level_data(),
                scene.levels.as_ref(),
                "{path:?}"
            );
            let mut s =
                Sculpt::with_history(scene.mesh, scene.levels, scene.log, scene.checkpoints);
            s.set_clock(1e4);
            stroke(&mut s, Brush::Clay, Vec3::new(0.2, 0.1, 1.0), 0.2);
            s.change_level(false);
            let mut replay = Replay::of(&s);
            assert!(replay.seek(f64::INFINITY, usize::MAX));
            assert_eq!(
                bits(&replay.sculpt.mesh.positions),
                bits(&s.mesh.positions),
                "{path:?}"
            );
            println!("{path:?}: replays bit for bit, before and after sculpting on");
            seen += 1;
        }
        assert!(seen > 0);
    }

    /// What the start clay costs: building it, subdividing it twice, a
    /// frame of dabs at each size, and the memory its levels take.
    /// `cargo test --release -p splatstudy-core start_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn start_timing() {
        use std::time::Instant;
        let ms = |since: Instant| since.elapsed().as_secs_f64() * 1e3;
        let frame = |s: &mut Sculpt| {
            let frames = 30;
            s.take_dirty();
            let start = Instant::now();
            s.begin_stroke();
            for f in 0..frames {
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
                s.refresh();
            }
            s.end_stroke();
            let t = ms(start) / frames as f64;
            let pages: usize = s.take_dirty().iter().map(|r| r.len()).sum();
            (t, pages / crate::sculpt::PAGE)
        };
        let start = Instant::now();
        let (mesh, levels) = cube_sphere(START_LEVEL);
        let t_build = ms(start);
        let start = Instant::now();
        let mut s = Sculpt::with_levels(mesh, Some(levels));
        let t_sculpt = ms(start);
        println!("start clay: built in {t_build:.0} ms, sculpt set up in {t_sculpt:.0} ms");
        let mut old = Sculpt::new(Mesh::icosphere(crate::sculpt::OLD_START_LEVEL));
        let (t, pages) = frame(&mut old);
        println!("old icosphere, 163842 vertices: a frame {t:.1} ms, {pages} pages uploaded");
        for round in 0..3 {
            let (t, pages) = frame(&mut s);
            let levels = s.level_data().unwrap();
            println!(
                "{} vertices: a frame {t:.1} ms, {pages} pages uploaded; levels hold {:.1} MB",
                s.mesh.positions.len(),
                levels.memory() as f64 / 1e6
            );
            if round < 2 {
                let start = Instant::now();
                assert!(s.subdivide());
                println!("  subdivided in {:.0} ms", ms(start));
            }
        }
        let (positions, triangles) = (s.mesh.positions.len(), s.mesh.triangles.len());
        // Positions, normals, two adjacency lists (about six items each),
        // triangles, and the GPU's vertices and indices.
        let mesh =
            positions * (12 + 12 + 2 * 4 * 7) + triangles * 12 + positions * 28 + triangles * 12;
        println!(
            "the finest clay itself: about {:.0} MB with its GPU copy",
            mesh as f64 / 1e6
        );
        let start = Instant::now();
        s.change_level(false);
        println!("down from the top: {:.0} ms", ms(start));
        let start = Instant::now();
        s.change_level(true);
        println!("up to the top: {:.0} ms", ms(start));
    }
}
