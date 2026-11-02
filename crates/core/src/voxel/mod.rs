// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Voxels: rebuilding the clay as an even mesh from a grid (remeshing), and
//! making clay from a reference.
//!
//! Both go through a grid of values that are negative inside the shape:
//! the distance to a mesh's triangles ([`sdf`]), or how densely splats
//! cover each spot ([`density`]). Surface nets ([`nets`]) then mesh its
//! zero level.

pub mod density;
pub mod grid;
pub mod nets;
pub mod sdf;

use glam::Vec3;

use crate::reference::{Blob, Reference};
use crate::sculpt::Mesh;

/// Remesh resolutions, in cells along the longest side of the shape.
pub const MIN_RESOLUTION: u32 = 32;
pub const MAX_RESOLUTION: u32 = 256;
pub const DEFAULT_RESOLUTION: u32 = 160;

/// Smoothing passes after surface nets.
const PASSES: usize = 2;

/// Splat density that counts as material, relative to the densest parts
/// of the shell (see [`density::field`]).
pub const DEFAULT_THRESHOLD: f32 = 0.25;

/// Holes in a reference mesh up to about twice this many cells wide are
/// sealed (scans are often open).
const REFERENCE_CLOSE: usize = 2;

/// The clay rebuilt with even triangles about `resolution` cells across:
/// merges what overlaps, closes what is open, drops floating bits. `None`
/// if nothing is left.
pub fn remesh(mesh: &Mesh, resolution: u32) -> Option<Mesh> {
    from_triangles(&mesh.positions, &mesh.triangles, resolution, 0)
}

/// A closed, even mesh around any triangles: the inside of the largest
/// piece. `close` seals holes up to about twice that many cells wide (see
/// [`grid::outside`]); a mesh that is open more widely than that comes out
/// as a thin shell around its surface.
pub fn from_triangles(
    positions: &[Vec3],
    triangles: &[[u32; 3]],
    resolution: u32,
    close: usize,
) -> Option<Mesh> {
    let resolution = resolution.clamp(MIN_RESOLUTION / 4, MAX_RESOLUTION);
    let mut field = sdf::signed_distance(positions, triangles, resolution, close)?;
    mesh_level(&mut field, true)
}

/// Clay in the shape of the reference, `resolution` cells across: from a
/// mesh's triangles, or from where the splats are dense enough
/// (`threshold`, 0..1, see [`density::field`]). Only what is inside the
/// crop box counts. `None` for an image, or if nothing is left.
pub fn clay_from_reference(reference: &Reference, resolution: u32, threshold: f32) -> Option<Mesh> {
    match reference.world_triangles() {
        Some((positions, triangles)) => {
            from_triangles(&positions, &triangles, resolution, REFERENCE_CLOSE)
        }
        None => from_blobs(&reference.world_blobs(), resolution, threshold),
    }
}

/// Clay from splats: see [`density::field`].
pub fn from_blobs(blobs: &[Blob], resolution: u32, threshold: f32) -> Option<Mesh> {
    let resolution = resolution.clamp(MIN_RESOLUTION / 4, MAX_RESOLUTION);
    let mut field = density::field(blobs, resolution, threshold)?;
    mesh_level(&mut field, false)
}

/// Meshes the zero level of `field`, keeps the largest piece and smooths
/// it; `project` pulls the smoothed vertices back onto the level, for a
/// field that is a true distance.
fn mesh_level(field: &mut grid::Grid, project: bool) -> Option<Mesh> {
    nets::untangle(field);
    let (positions, triangles) = nets::surface_nets(field);
    let (mut positions, triangles) = nets::keep_largest(&positions, &triangles);
    if triangles.is_empty() {
        return None;
    }
    nets::smooth(&mut positions, &triangles, PASSES, project.then_some(field));
    Some(Mesh::new(positions, triangles))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::Quat;

    use super::*;
    use crate::meshio::is_closed;
    use crate::reference::{Aabb, Content, RefMesh, Source, Splats};

    fn source() -> Source {
        Source {
            name: "hand made".into(),
            bytes: Arc::from([]),
        }
    }

    #[test]
    fn clay_from_a_splat_reference_follows_its_placement_and_crop() {
        // A shell of splats around (10, 0, 0) in file units, radius 4, and
        // a pile of floaters far off, outside the crop box.
        let mut splats = Splats::default();
        let count = 3000;
        let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
        for i in 0..count + 50 {
            let y = 1.0 - 2.0 * (i % count) as f32 / count as f32;
            let r = (1.0 - y * y).sqrt();
            let a = golden * i as f32;
            let centre = if i < count {
                Vec3::new(10.0, 0.0, 0.0) + Vec3::new(r * a.cos(), y, r * a.sin()) * 4.0
            } else {
                Vec3::new(30.0, 0.0, 0.0)
            };
            splats.positions.push(centre);
            splats.scales.push(Vec3::new(0.1, 0.05, 0.1));
            splats.rotations.push(Quat::IDENTITY);
            splats.colors.push([200, 180, 160, 230]);
        }
        let mut reference = Reference::new(source(), Content::Splats(splats));
        reference.align.centre = Vec3::new(10.0, 0.0, 0.0);
        reference.align.scale = 0.25;
        reference.crop = Some(Aabb {
            min: Vec3::splat(-20.0),
            max: Vec3::splat(20.0),
        });
        let mesh = clay_from_reference(&reference, 48, DEFAULT_THRESHOLD).unwrap();
        assert!(is_closed(&mesh.triangles));
        for p in &mesh.positions {
            assert!((0.9..1.12).contains(&p.length()), "{p}");
        }
    }

    #[test]
    fn clay_from_an_open_scan_is_closed() {
        // A scanned bust is often open at the base: a sphere with a hole.
        let sphere = Mesh::icosphere(4);
        let bottom = Vec3::new(0.0, -1.0, 0.0);
        let triangles: Vec<[u32; 3]> = sphere
            .triangles
            .iter()
            .filter(|t| {
                t.iter()
                    .any(|&v| sphere.positions[v as usize].distance(bottom) > 0.12)
            })
            .copied()
            .collect();
        let scan = RefMesh {
            positions: sphere.positions.iter().map(|p| *p * 2.0).collect(),
            triangles,
            colors: None,
        };
        let mut reference = Reference::new(source(), Content::Mesh(scan));
        reference.align.scale = 0.5;
        let mesh = clay_from_reference(&reference, 48, DEFAULT_THRESHOLD).unwrap();
        assert!(is_closed(&mesh.triangles));
        let near = mesh
            .positions
            .iter()
            .filter(|p| (p.length() - 1.0).abs() < 0.06)
            .count();
        assert!(near as f32 > mesh.positions.len() as f32 * 0.95);
    }

    #[test]
    fn a_remeshed_sphere_stays_round_and_closed() {
        let sphere = Mesh::icosphere(4);
        let resolution = 48;
        let step = 2.0 / resolution as f32;
        let mesh = remesh(&sphere, resolution).unwrap();
        assert!(is_closed(&mesh.triangles));
        for p in &mesh.positions {
            assert!((p.length() - 1.0).abs() < step, "{p}");
        }
        // Euler characteristic of a sphere.
        let (v, f) = (mesh.positions.len() as i64, mesh.triangles.len() as i64);
        assert_eq!(v - f * 3 / 2 + f, 2);
        // Even: no triangle much longer than the grid step.
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
            let longest = a.distance(b).max(b.distance(c)).max(c.distance(a));
            assert!(longest < step * 2.5, "{longest}");
        }
    }

    #[test]
    fn overlapping_and_floating_pieces_merge_or_go() {
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        for (centre, radius) in [
            (Vec3::new(-0.4, 0.0, 0.0), 0.6),
            (Vec3::new(0.4, 0.0, 0.0), 0.6),
            (Vec3::new(0.0, 3.0, 0.0), 0.1),
        ] {
            let ball = Mesh::icosphere(3);
            let offset = positions.len() as u32;
            positions.extend(ball.positions.iter().map(|p| *p * radius + centre));
            triangles.extend(ball.triangles.iter().map(|t| t.map(|v| v + offset)));
        }
        let mesh = from_triangles(&positions, &triangles, 64, 0).unwrap();
        assert!(is_closed(&mesh.triangles));
        // One piece around both big balls; the small one far above is gone.
        assert!(mesh.positions.iter().all(|p| p.y < 1.0));
        assert!(mesh.positions.iter().any(|p| p.x < -0.9));
        assert!(mesh.positions.iter().any(|p| p.x > 0.9));
        // Nothing left inside where the balls overlap.
        assert!(mesh.positions.iter().all(|p| p.length() > 0.15));
    }

    #[test]
    fn an_open_mesh_is_sealed() {
        // A sphere with a hole about seven cells wide.
        let sphere = Mesh::icosphere(4);
        let top = Vec3::new(0.0, 1.0, 0.0);
        let triangles: Vec<[u32; 3]> = sphere
            .triangles
            .iter()
            .filter(|t| {
                t.iter()
                    .any(|&v| sphere.positions[v as usize].distance(top) > 0.15)
            })
            .copied()
            .collect();
        assert!(!is_closed(&triangles));
        let mesh = from_triangles(&sphere.positions, &triangles, 48, 1).unwrap();
        assert!(is_closed(&mesh.triangles));
        // Solid: the vertices lie near the sphere, not on a doubled shell.
        let near = mesh
            .positions
            .iter()
            .filter(|p| (p.length() - 1.0).abs() < 0.06)
            .count();
        assert!(near as f32 > mesh.positions.len() as f32 * 0.95);
    }

    /// `cargo test --release -p splatstudy-core remesh_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn remesh_timing() {
        let sphere = Mesh::icosphere(crate::sculpt::DEFAULT_LEVEL);
        for resolution in [64, 128, 192, 256] {
            let start = std::time::Instant::now();
            let mut lap = {
                let mut last = start;
                move || {
                    let now = std::time::Instant::now();
                    let ms = (now - last).as_secs_f64() * 1e3;
                    last = now;
                    format!("{ms:.0}")
                }
            };
            let mut field =
                sdf::signed_distance(&sphere.positions, &sphere.triangles, resolution, 0).unwrap();
            let t_sdf = lap();
            nets::untangle(&mut field);
            let t_untangle = lap();
            let (positions, triangles) = nets::surface_nets(&field);
            let t_nets = lap();
            let (mut positions, triangles) = nets::keep_largest(&positions, &triangles);
            let t_keep = lap();
            nets::smooth(&mut positions, &triangles, PASSES, Some(&field));
            let t_smooth = lap();
            let mesh = Mesh::new(positions, triangles);
            let t_mesh = lap();
            let total = start.elapsed().as_secs_f64() * 1e3;
            println!(
                "{resolution}: total {total:.0} ms = distance {t_sdf} + untangle {t_untangle} \
                 + nets {t_nets} + largest {t_keep} + smooth {t_smooth} + mesh {t_mesh}; \
                 {} vertices",
                mesh.positions.len()
            );
        }
    }
}
