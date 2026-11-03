// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The nearest point of a triangle mesh, found through a grid of cells
//! listing the triangles that cross them.

use glam::{IVec3, Vec3};

use crate::sculpt::Mesh;
use crate::voxel::sdf::closest_point;

/// Cells along the longest side at most.
const MAX_CELLS: f32 = 160.0;
/// Rings of cells searched around a point before giving up.
const MAX_RINGS: i32 = 4;

pub struct Surface<'a> {
    mesh: &'a Mesh,
    min: Vec3,
    cell: f32,
    dims: IVec3,
    offsets: Vec<u32>,
    items: Vec<u32>,
}

impl<'a> Surface<'a> {
    /// The grid for `mesh`, with cells about `cell` wide.
    pub fn new(mesh: &'a Mesh, cell: f32) -> Surface<'a> {
        let (min, max) = mesh
            .positions
            .iter()
            .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), &p| {
                (lo.min(p), hi.max(p))
            });
        let extent = (max - min).max(Vec3::splat(1e-6));
        let cell = cell.max(extent.max_element() / MAX_CELLS).max(1e-6);
        let dims = (extent / cell).ceil().as_ivec3().max(IVec3::ONE);
        let len = (dims.x * dims.y * dims.z) as usize;
        let mut surface = Surface {
            mesh,
            min,
            cell,
            dims,
            offsets: vec![0; len + 1],
            items: Vec::new(),
        };
        // Counted, then filled in place.
        for pass in 0..2 {
            let mut fill = surface.offsets.clone();
            for (t, tri) in mesh.triangles.iter().enumerate() {
                let [a, b, c] = tri.map(|i| mesh.positions[i as usize]);
                let lo = surface.cell_of(a.min(b).min(c));
                let hi = surface.cell_of(a.max(b).max(c));
                for z in lo.z..=hi.z {
                    for y in lo.y..=hi.y {
                        for x in lo.x..=hi.x {
                            let k = surface.index(IVec3::new(x, y, z));
                            if pass == 0 {
                                surface.offsets[k + 1] += 1;
                            } else {
                                surface.items[fill[k] as usize] = t as u32;
                                fill[k] += 1;
                            }
                        }
                    }
                }
            }
            if pass == 0 {
                for k in 0..len {
                    surface.offsets[k + 1] += surface.offsets[k];
                }
                surface.items = vec![0; surface.offsets[len] as usize];
            }
        }
        surface
    }

    fn cell_of(&self, p: Vec3) -> IVec3 {
        ((p - self.min) / self.cell)
            .floor()
            .as_ivec3()
            .clamp(IVec3::ZERO, self.dims - 1)
    }

    fn index(&self, c: IVec3) -> usize {
        ((c.z * self.dims.y + c.y) * self.dims.x + c.x) as usize
    }

    /// The nearest point of the mesh to `p` on a triangle facing the same
    /// side as `facing` (so a point on one side of a thin part never jumps
    /// to the other), and the surface normal there; `None` if there is
    /// none within a few cells. The normal is blended from the triangle's
    /// corners, so it turns smoothly across the surface.
    pub fn nearest(&self, p: Vec3, facing: Vec3) -> Option<(Vec3, Vec3)> {
        let centre = self.cell_of(p);
        let mut best: Option<(f32, Vec3, u32)> = None;
        for ring in 0..=MAX_RINGS {
            let lo = (centre - ring).max(IVec3::ZERO);
            let hi = (centre + ring).min(self.dims - 1);
            for z in lo.z..=hi.z {
                for y in lo.y..=hi.y {
                    for x in lo.x..=hi.x {
                        let c = IVec3::new(x, y, z);
                        if (c - centre).abs().max_element() != ring {
                            continue;
                        }
                        let k = self.index(c);
                        let range = self.offsets[k] as usize..self.offsets[k + 1] as usize;
                        for &t in &self.items[range] {
                            let [a, b, cc] = self.mesh.triangles[t as usize]
                                .map(|i| self.mesh.positions[i as usize]);
                            if (b - a).cross(cc - a).dot(facing) < 0.0 {
                                continue;
                            }
                            let q = closest_point(p, a, b, cc);
                            let d = q.distance_squared(p);
                            if best.is_none_or(|(bd, _, _)| d < bd) {
                                best = Some((d, q, t));
                            }
                        }
                    }
                }
            }
            // Anything in a further ring is at least this far away.
            if let Some((d, _, _)) = best
                && d.sqrt() <= ring as f32 * self.cell
            {
                break;
            }
        }
        best.map(|(_, q, t)| {
            // The corners' normals, blended by where `q` is in the triangle.
            let tri = self.mesh.triangles[t as usize];
            let [a, b, c] = tri.map(|i| self.mesh.positions[i as usize]);
            let face = (b - a).cross(c - a);
            let weights = [
                (c - b).cross(q - b),
                (a - c).cross(q - c),
                (b - a).cross(q - a),
            ]
            .map(|w| w.dot(face).max(0.0));
            let blended: Vec3 = tri
                .iter()
                .zip(weights)
                .map(|(&i, w)| self.mesh.normals[i as usize] * w)
                .sum();
            (q, blended.normalize_or(face.normalize_or(Vec3::Y)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_land_on_the_sphere() {
        let mesh = Mesh::icosphere(3);
        let surface = Surface::new(&mesh, 0.1);
        for p in [Vec3::new(0.3, 0.2, 0.9), Vec3::new(-1.1, 0.1, 0.05)] {
            let (q, n) = surface.nearest(p, p).unwrap();
            assert!((q.length() - 1.0).abs() < 0.02, "{q}");
            assert!(n.dot(q.normalize()) > 0.95);
        }
    }
}
