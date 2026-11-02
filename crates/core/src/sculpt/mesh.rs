// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The clay: a triangle mesh with fixed topology. Brushes move vertices; the
//! triangles never change, so the GPU buffers keep their layout and only the
//! touched ranges are uploaded.

use std::collections::HashMap;

use glam::Vec3;

/// One vertex as the GPU sees it: position then normal, 24 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// Counter-clockwise when seen from outside.
    pub triangles: Vec<[u32; 3]>,
}

impl Mesh {
    /// A unit sphere made by subdividing an icosahedron `level` times:
    /// `10 * 4^level + 2` vertices of nearly equal spacing, which brushes
    /// need (a UV sphere crowds its vertices at the poles).
    pub fn icosphere(level: u32) -> Mesh {
        let t = (1.0 + 5f32.sqrt()) / 2.0;
        let mut positions: Vec<Vec3> = [
            [-1.0, t, 0.0],
            [1.0, t, 0.0],
            [-1.0, -t, 0.0],
            [1.0, -t, 0.0],
            [0.0, -1.0, t],
            [0.0, 1.0, t],
            [0.0, -1.0, -t],
            [0.0, 1.0, -t],
            [t, 0.0, -1.0],
            [t, 0.0, 1.0],
            [-t, 0.0, -1.0],
            [-t, 0.0, 1.0],
        ]
        .into_iter()
        .map(|p| Vec3::from(p).normalize())
        .collect();
        let mut triangles: Vec<[u32; 3]> = vec![
            [0, 11, 5],
            [0, 5, 1],
            [0, 1, 7],
            [0, 7, 10],
            [0, 10, 11],
            [1, 5, 9],
            [5, 11, 4],
            [11, 10, 2],
            [10, 7, 6],
            [7, 1, 8],
            [3, 9, 4],
            [3, 4, 2],
            [3, 2, 6],
            [3, 6, 8],
            [3, 8, 9],
            [4, 9, 5],
            [2, 4, 11],
            [6, 2, 10],
            [8, 6, 7],
            [9, 8, 1],
        ];

        for _ in 0..level {
            let mut midpoints: HashMap<(u32, u32), u32> =
                HashMap::with_capacity(triangles.len() * 3 / 2);
            let mut midpoint = |a: u32, b: u32, positions: &mut Vec<Vec3>| -> u32 {
                let key = (a.min(b), a.max(b));
                *midpoints.entry(key).or_insert_with(|| {
                    let p = (positions[a as usize] + positions[b as usize]).normalize();
                    positions.push(p);
                    positions.len() as u32 - 1
                })
            };
            let mut next = Vec::with_capacity(triangles.len() * 4);
            for [a, b, c] in triangles {
                let ab = midpoint(a, b, &mut positions);
                let bc = midpoint(b, c, &mut positions);
                let ca = midpoint(c, a, &mut positions);
                next.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
            }
            triangles = next;
        }

        let mut mesh = Mesh {
            normals: vec![Vec3::ZERO; positions.len()],
            positions,
            triangles,
        };
        mesh.recompute_normals();
        mesh
    }

    /// Area-weighted vertex normals from the triangles.
    pub fn recompute_normals(&mut self) {
        self.normals.iter_mut().for_each(|n| *n = Vec3::ZERO);
        for &[a, b, c] in &self.triangles {
            let (pa, pb, pc) = (
                self.positions[a as usize],
                self.positions[b as usize],
                self.positions[c as usize],
            );
            // The cross product's length is twice the area: larger triangles weigh more.
            let n = (pb - pa).cross(pc - pa);
            for i in [a, b, c] {
                self.normals[i as usize] += n;
            }
        }
        self.normals
            .iter_mut()
            .for_each(|n| *n = n.normalize_or(Vec3::Y));
    }

    pub fn vertices(&self) -> Vec<Vertex> {
        self.positions
            .iter()
            .zip(&self.normals)
            .map(|(p, n)| Vertex {
                position: p.to_array(),
                normal: n.to_array(),
            })
            .collect()
    }

    pub fn indices(&self) -> &[u32] {
        bytemuck::cast_slice(&self.triangles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icosphere_has_the_expected_size() {
        for level in 0..5 {
            let mesh = Mesh::icosphere(level);
            let faces = 20 * 4usize.pow(level);
            assert_eq!(mesh.positions.len(), 10 * 4usize.pow(level) + 2);
            assert_eq!(mesh.triangles.len(), faces);
        }
    }

    #[test]
    fn icosphere_is_closed_and_faces_outward() {
        let mesh = Mesh::icosphere(3);
        // Closed and manifold: every directed edge appears exactly once and
        // its reverse exists, so each edge joins exactly two triangles.
        let mut edges = HashMap::new();
        for &[a, b, c] in &mesh.triangles {
            for e in [(a, b), (b, c), (c, a)] {
                *edges.entry(e).or_insert(0) += 1;
            }
        }
        assert!(edges.values().all(|&n| n == 1));
        assert!(edges.keys().all(|&(a, b)| edges.contains_key(&(b, a))));
        // Euler characteristic of a sphere.
        let (v, e, f) = (
            mesh.positions.len() as i64,
            edges.len() as i64 / 2,
            mesh.triangles.len() as i64,
        );
        assert_eq!(v - e + f, 2);

        for &[a, b, c] in &mesh.triangles {
            let (pa, pb, pc) = (
                mesh.positions[a as usize],
                mesh.positions[b as usize],
                mesh.positions[c as usize],
            );
            let centre = (pa + pb + pc) / 3.0;
            assert!(
                (pb - pa).cross(pc - pa).dot(centre) > 0.0,
                "triangle winds inward"
            );
        }
        for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
            assert!((p.length() - 1.0).abs() < 1e-5);
            assert!(n.dot(*p) > 0.999);
        }
    }
}
