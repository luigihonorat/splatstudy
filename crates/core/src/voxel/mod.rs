// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Voxels: rebuilding the clay as an even mesh from a grid (remeshing), and
//! making clay from a reference.
//!
//! Both go through a grid of signed distances, negative inside the shape:
//! to a mesh's triangles ([`sdf`]), or to the surface splats show from
//! many sides ([`fusion`]). Surface nets ([`nets`]) then mesh its zero
//! level.

pub mod fusion;
pub mod grid;
pub mod nets;
pub mod sdf;

use glam::Vec3;

use crate::reference::{Content, Reference};
use crate::sculpt::Mesh;

/// Remesh resolutions, in cells along the longest side of the shape.
pub const MIN_RESOLUTION: u32 = 32;
pub const MAX_RESOLUTION: u32 = 384;
pub const DEFAULT_RESOLUTION: u32 = 160;

/// Smoothing passes after surface nets, each pulled back onto the level:
/// they even out the triangles and take off the grid's stair steps.
const PASSES: usize = 2;

/// The opacity a ray through the splats gathers before it counts as
/// having reached the surface (see [`fusion::field`]). Well past half: a
/// capture's fuzz (hair, a beard, floaters) gathers some opacity in front
/// of the surface proper.
pub const DEFAULT_THRESHOLD: f32 = 0.7;
/// The thresholds offered. Lower, a fuzzy capture's haze counts as
/// surface and the clay comes out spongy; 1 is never quite reached.
pub const THRESHOLDS: std::ops::RangeInclusive<f32> = 0.3..=0.95;

/// How much the clay from a reference is smoothed: passes of
/// [`grid::Grid::blur`] over its field, each smoothing the shape by
/// about half a cell; 0 keeps it as found.
pub const MAX_SMOOTHNESS: u32 = 10;
pub const DEFAULT_SMOOTHNESS: u32 = 1;

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
    mesh_level(&mut field)
}

/// How to make clay from a reference.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FromReference {
    /// Cells along the longest side.
    pub resolution: u32,
    /// For splats, the opacity (0..1) a ray gathers before it counts as
    /// having reached the surface (see [`fusion::field`]).
    pub threshold: f32,
    /// How much the result is smoothed, 0..=[`MAX_SMOOTHNESS`].
    pub smoothness: u32,
}

impl Default for FromReference {
    fn default() -> Self {
        Self {
            resolution: DEFAULT_RESOLUTION,
            threshold: DEFAULT_THRESHOLD,
            smoothness: DEFAULT_SMOOTHNESS,
        }
    }
}

/// Clay in the shape of the reference: from a mesh's triangles, or from
/// the surface the splats show (see [`fusion::field`]). Only what is
/// inside the crop box counts, closed flat where the box cuts it. `None`
/// for an image, or if nothing is left.
pub fn clay_from_reference(reference: &Reference, how: FromReference) -> Option<Mesh> {
    let resolution = how.resolution.clamp(MIN_RESOLUTION / 4, MAX_RESOLUTION);
    let smoothness = how.smoothness.min(MAX_SMOOTHNESS);
    let mut field = match &reference.content {
        Content::Splats(splats) => fusion::field(
            splats,
            &reference.align,
            reference.crop,
            resolution,
            how.threshold,
            smoothness,
        )?,
        _ => {
            let (positions, triangles) = reference.world_triangles()?;
            let mut field =
                sdf::signed_distance(&positions, &triangles, resolution, REFERENCE_CLOSE)?;
            for _ in 0..smoothness {
                field.blur();
            }
            field
        }
    };
    mesh_level(&mut field)
}

/// Meshes the zero level of `field`, keeps the largest piece and smooths
/// it along itself, pulling the vertices back onto the level each pass.
fn mesh_level(field: &mut grid::Grid) -> Option<Mesh> {
    nets::untangle(field);
    let (positions, triangles) = nets::surface_nets(field);
    let (mut positions, triangles) = nets::keep_largest(&positions, &triangles);
    if triangles.is_empty() {
        return None;
    }
    nets::smooth(&mut positions, &triangles, PASSES, Some(field));
    Some(Mesh::new(positions, triangles))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::Quat;

    use super::*;
    use crate::meshio::is_closed;
    use crate::reference::{Aabb, Content, RefMesh, Source, Splats};

    /// The default way to make clay from a reference, at `resolution`.
    pub(crate) fn at(resolution: u32) -> FromReference {
        FromReference {
            resolution,
            ..Default::default()
        }
    }

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
        let mesh = clay_from_reference(&reference, at(48)).unwrap();
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
        let mesh = clay_from_reference(&reference, at(48)).unwrap();
        assert!(is_closed(&mesh.triangles));
        let near = mesh
            .positions
            .iter()
            .filter(|p| (p.length() - 1.0).abs() < 0.06)
            .count();
        assert!(near as f32 > mesh.positions.len() as f32 * 0.95);
    }

    #[test]
    fn clay_from_a_cropped_hollow_cast_is_solid_at_the_cut() {
        // A hollow ball, walls from radius 0.6 to 1, cut in half: the clay
        // fills the ring the cut leaves and keeps the hollow open.
        let outer = Mesh::icosphere(4);
        let inner = Mesh::icosphere(3);
        let mut positions = outer.positions.clone();
        let mut triangles = outer.triangles.clone();
        let n = positions.len() as u32;
        positions.extend(inner.positions.iter().map(|p| *p * 0.6));
        triangles.extend(
            inner
                .triangles
                .iter()
                .map(|&[a, b, c]| [a + n, c + n, b + n]),
        );
        let cast = RefMesh {
            positions,
            triangles,
            colors: None,
        };
        let mut reference = Reference::new(source(), Content::Mesh(cast));
        reference.crop = Some(Aabb {
            min: Vec3::splat(-2.0),
            max: Vec3::new(2.0, 0.0, 2.0),
        });
        let mesh = clay_from_reference(&reference, at(64)).unwrap();
        assert!(is_closed(&mesh.triangles));
        let flat: Vec<Vec3> = mesh
            .positions
            .iter()
            .copied()
            .filter(|p| p.y.abs() < 0.01)
            .collect();
        let ring = |lo: f32, hi: f32| {
            flat.iter()
                .filter(|p| (lo..hi).contains(&p.length()))
                .count()
        };
        // Across the wall's cut, not only at its two edges...
        assert!(ring(0.7, 0.9) > 20, "{}", ring(0.7, 0.9));
        // ...and nothing across the hollow.
        assert_eq!(ring(0.0, 0.5), 0);
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
        let sphere = Mesh::icosphere(crate::sculpt::OLD_START_LEVEL);
        for resolution in [64, 128, 160, 192, 256] {
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
            let count = mesh.positions.len();
            let total = start.elapsed().as_secs_f64() * 1e3;
            // Then in place of the clay: hierarchy, undo, checkpoint.
            let mut sculpt = crate::sculpt::Sculpt::new(sphere.clone());
            let start = std::time::Instant::now();
            sculpt.replace_mesh(mesh);
            let t_replace = start.elapsed().as_secs_f64() * 1e3;
            println!(
                "{resolution}: total {total:.0} ms = distance {t_sdf} + untangle {t_untangle} \
                 + nets {t_nets} + largest {t_keep} + smooth {t_smooth} + mesh {t_mesh}; \
                 {count} vertices; then replace_mesh {t_replace:.0} ms"
            );
        }
    }
}
