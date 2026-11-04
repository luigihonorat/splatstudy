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
        best.map(|(_, q, t)| (q, self.normal_at(q, t)))
    }

    /// Where the line through `p` along `direction` crosses the mesh
    /// within `reach` either way, nearest `p`, on a triangle facing the
    /// same way as `direction`; and the surface normal there. Unlike the
    /// nearest point, this finds the tip of a ridge from inside it.
    pub fn along(&self, p: Vec3, direction: Vec3, reach: f32) -> Option<(Vec3, Vec3)> {
        let d = direction.normalize_or_zero();
        if d == Vec3::ZERO || !p.is_finite() || reach.is_nan() || reach <= 0.0 {
            return None;
        }
        let from = p - d * reach;
        let steps = ((2.0 * reach / (self.cell * 0.25)).ceil() as usize).clamp(1, 4096);
        let mut cells: Vec<usize> = (0..=steps)
            .map(|k| self.index(self.cell_of(from + d * (2.0 * reach * k as f32 / steps as f32))))
            .collect();
        cells.sort_unstable();
        cells.dedup();
        let mut best: Option<(f32, u32)> = None;
        for k in cells {
            let range = self.offsets[k] as usize..self.offsets[k + 1] as usize;
            for &t in &self.items[range] {
                let [a, b, c] =
                    self.mesh.triangles[t as usize].map(|i| self.mesh.positions[i as usize]);
                if (b - a).cross(c - a).dot(d) <= 0.0 {
                    continue;
                }
                // Möller and Trumbore's line and triangle test.
                let (e1, e2) = (b - a, c - a);
                let h = d.cross(e2);
                let det = e1.dot(h);
                if det.abs() < 1e-12 {
                    continue;
                }
                let s = p - a;
                let u = s.dot(h) / det;
                let q = s.cross(e1);
                let v = d.dot(q) / det;
                if !(0.0..=1.0).contains(&u) || v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let along = e2.dot(q) / det;
                if along.abs() <= reach && best.is_none_or(|(x, _)| along.abs() < x.abs()) {
                    best = Some((along, t));
                }
            }
        }
        best.map(|(along, t)| {
            let q = p + d * along;
            (q, self.normal_at(q, t))
        })
    }

    /// The corners' normals of triangle `t`, blended by where `q` is in it.
    fn normal_at(&self, q: Vec3, t: u32) -> Vec3 {
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
        blended.normalize_or(face.normalize_or(Vec3::Y))
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

    #[test]
    fn a_line_from_inside_finds_the_tip() {
        // A thin cone pointing up +X: from a point inside it near the
        // tip, the nearest point is on its side, the line along +X
        // reaches the tip.
        let (n, tip) = (24, Vec3::X * 2.0);
        let mut positions = vec![tip, Vec3::ZERO];
        for k in 0..n {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            positions.push(Vec3::new(0.0, a.cos() * 0.3, a.sin() * 0.3));
        }
        let mut triangles = Vec::new();
        for k in 0..n {
            let (i, j) = (2 + k, 2 + (k + 1) % n);
            triangles.push([0, i, j]);
            triangles.push([1, j, i]);
        }
        let mesh = Mesh::new(positions, triangles);
        let surface = Surface::new(&mesh, 0.1);
        let inside = Vec3::new(1.7, 0.0, 0.0);
        let (q, _) = surface.along(inside, Vec3::X, 0.5).expect("crossed");
        assert!(q.distance(tip) < 0.02, "{q}");
        let (q, _) = surface.nearest(inside, Vec3::X).expect("near");
        assert!(q.distance(tip) > 0.1, "{q}");
        assert!(surface.along(inside, Vec3::X, 0.1).is_none());
    }
}
