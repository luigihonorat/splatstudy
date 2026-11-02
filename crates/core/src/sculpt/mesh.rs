// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The clay: a triangle mesh with fixed topology. Brushes move vertices; the
//! triangles never change, so the GPU buffers keep their layout and only the
//! touched ranges are uploaded.

use std::collections::HashMap;

use glam::Vec3;

/// One vertex as the GPU sees it: position, normal and an sRGB colour,
/// 28 bytes. The clay is white; coloured scans and, later, polypaint fill
/// the colour.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [u8; 4],
}

/// Compressed adjacency lists: the items of `i` are `items[offsets[i]..offsets[i + 1]]`.
#[derive(Clone, Default)]
pub struct Adjacency {
    offsets: Vec<u32>,
    items: Vec<u32>,
}

impl Adjacency {
    pub fn of(&self, i: u32) -> &[u32] {
        let i = i as usize;
        &self.items[self.offsets[i] as usize..self.offsets[i + 1] as usize]
    }

    /// Where the items of `i` start among all the items.
    pub fn start(&self, i: u32) -> usize {
        self.offsets[i as usize] as usize
    }

    /// All the items of every list together.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Clone)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// Counter-clockwise when seen from outside.
    pub triangles: Vec<[u32; 3]>,
    /// Triangles around each vertex.
    pub vertex_faces: Adjacency,
    /// Vertices sharing an edge with each vertex.
    pub neighbours: Adjacency,
}

impl Mesh {
    /// A mesh from any source. Vertices and triangles are reordered so that
    /// neighbours sit close in memory, which keeps sculpting and uploads fast.
    pub fn new(positions: Vec<Vec3>, triangles: Vec<[u32; 3]>) -> Mesh {
        let (positions, triangles) = spatial_order(positions, triangles);
        Self::from_ordered(positions, triangles)
    }

    /// A mesh kept in the given order, for one saved by [`Mesh::new`]
    /// earlier: a recorded session replays exactly only on the same order.
    pub fn from_ordered(positions: Vec<Vec3>, triangles: Vec<[u32; 3]>) -> Mesh {
        let mut mesh = Mesh {
            normals: vec![Vec3::ZERO; positions.len()],
            vertex_faces: vertex_faces(positions.len(), &triangles),
            neighbours: Adjacency::default(),
            positions,
            triangles,
        };
        mesh.neighbours = neighbours(&mesh);
        mesh.recompute_normals();
        mesh
    }

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
        Mesh::new(positions, triangles)
    }

    fn face_normal(&self, [a, b, c]: [u32; 3]) -> Vec3 {
        let (pa, pb, pc) = (
            self.positions[a as usize],
            self.positions[b as usize],
            self.positions[c as usize],
        );
        // The cross product's length is twice the area: larger triangles weigh more.
        (pb - pa).cross(pc - pa)
    }

    /// Area-weighted vertex normals from the triangles.
    pub fn recompute_normals(&mut self) {
        self.normals.iter_mut().for_each(|n| *n = Vec3::ZERO);
        for &t in &self.triangles {
            let n = self.face_normal(t);
            for i in t {
                self.normals[i as usize] += n;
            }
        }
        self.normals
            .iter_mut()
            .for_each(|n| *n = n.normalize_or(Vec3::Y));
    }

    /// Updates the normals that depend on the `moved` vertices: theirs and
    /// those of every vertex sharing a triangle with them. `stamp` is scratch
    /// space with one entry per vertex; `affected` receives those vertices.
    pub fn update_normals(&mut self, moved: &[u32], stamp: &mut Stamp, affected: &mut Vec<u32>) {
        stamp.next();
        affected.clear();
        for &v in moved {
            for &f in self.vertex_faces.of(v) {
                for u in self.triangles[f as usize] {
                    if stamp.mark(u) {
                        affected.push(u);
                    }
                }
            }
        }
        for &v in affected.iter() {
            let n: Vec3 = self
                .vertex_faces
                .of(v)
                .iter()
                .map(|&f| self.face_normal(self.triangles[f as usize]))
                .sum();
            self.normals[v as usize] = n.normalize_or(Vec3::Y);
        }
    }

    pub fn vertex(&self, i: usize) -> Vertex {
        Vertex {
            position: self.positions[i].to_array(),
            normal: self.normals[i].to_array(),
            color: [255; 4],
        }
    }

    pub fn vertices(&self) -> Vec<Vertex> {
        (0..self.positions.len()).map(|i| self.vertex(i)).collect()
    }

    pub fn indices(&self) -> &[u32] {
        bytemuck::cast_slice(&self.triangles)
    }
}

/// Marks items as seen without clearing a whole array between uses.
pub struct Stamp {
    marks: Vec<u32>,
    current: u32,
}

impl Stamp {
    pub fn new(len: usize) -> Self {
        Self {
            marks: vec![0; len],
            current: 0,
        }
    }

    /// Starts a new round; every item is unmarked again.
    pub fn next(&mut self) {
        self.current = self.current.wrapping_add(1);
        if self.current == 0 {
            self.marks.iter_mut().for_each(|m| *m = 0);
            self.current = 1;
        }
    }

    /// Marks `i`; true if it was not marked yet in this round.
    pub fn mark(&mut self, i: u32) -> bool {
        let m = &mut self.marks[i as usize];
        let fresh = *m != self.current;
        *m = self.current;
        fresh
    }
}

/// Reorders vertices along a Morton (Z-order) curve and triangles by their
/// first vertex, so nearby surface is nearby in memory: a brush stroke then
/// touches a few contiguous ranges of the GPU buffer.
fn spatial_order(positions: Vec<Vec3>, triangles: Vec<[u32; 3]>) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let (min, max) = positions
        .iter()
        .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), &p| {
            (lo.min(p), hi.max(p))
        });
    let scale = 1023.0 / (max - min).max_element().max(1e-12);
    let spread = |v: u32| {
        let mut x = v & 0x3ff;
        x = (x | (x << 16)) & 0x0300_00ff;
        x = (x | (x << 8)) & 0x0300_f00f;
        x = (x | (x << 4)) & 0x030c_30c3;
        (x | (x << 2)) & 0x0924_9249
    };
    let code = |p: Vec3| {
        let q = ((p - min) * scale).as_uvec3();
        spread(q.x) | (spread(q.y) << 1) | (spread(q.z) << 2)
    };
    let mut order: Vec<u32> = (0..positions.len() as u32).collect();
    order.sort_by_key(|&i| code(positions[i as usize]));
    let mut remap = vec![0u32; positions.len()];
    for (new, &old) in order.iter().enumerate() {
        remap[old as usize] = new as u32;
    }
    let positions = order.iter().map(|&i| positions[i as usize]).collect();
    let mut triangles: Vec<[u32; 3]> = triangles
        .into_iter()
        .map(|t| t.map(|i| remap[i as usize]))
        .collect();
    triangles.sort_by_key(|t| t[0].min(t[1]).min(t[2]));
    (positions, triangles)
}

fn vertex_faces(vertex_count: usize, triangles: &[[u32; 3]]) -> Adjacency {
    let mut offsets = vec![0u32; vertex_count + 1];
    for t in triangles {
        for &v in t {
            offsets[v as usize + 1] += 1;
        }
    }
    for i in 0..vertex_count {
        offsets[i + 1] += offsets[i];
    }
    let mut fill = offsets.clone();
    let mut items = vec![0u32; offsets[vertex_count] as usize];
    for (f, t) in triangles.iter().enumerate() {
        for &v in t {
            items[fill[v as usize] as usize] = f as u32;
            fill[v as usize] += 1;
        }
    }
    Adjacency { offsets, items }
}

/// The vertices around each vertex, in the order its triangles first
/// reach them. A stamp keeps each ring free of repeats in time linear in
/// its size, even around a crafted vertex with a million triangles.
fn neighbours(mesh: &Mesh) -> Adjacency {
    let n = mesh.positions.len();
    let mut offsets = Vec::with_capacity(n + 1);
    let mut items = Vec::with_capacity(n * 6);
    offsets.push(0);
    let mut seen = Stamp::new(n);
    for v in 0..n as u32 {
        seen.next();
        seen.mark(v);
        for &f in mesh.vertex_faces.of(v) {
            for u in mesh.triangles[f as usize] {
                if seen.mark(u) {
                    items.push(u);
                }
            }
        }
        offsets.push(items.len() as u32);
    }
    Adjacency { offsets, items }
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

        for &t in &mesh.triangles {
            let centre = t.iter().map(|&i| mesh.positions[i as usize]).sum::<Vec3>() / 3.0;
            assert!(
                mesh.face_normal(t).dot(centre) > 0.0,
                "triangle winds inward"
            );
        }
        for (p, n) in mesh.positions.iter().zip(&mesh.normals) {
            assert!((p.length() - 1.0).abs() < 1e-5);
            assert!(n.dot(*p) > 0.999);
        }
    }

    #[test]
    fn adjacency_matches_the_icosahedron() {
        let mesh = Mesh::icosphere(2);
        // Twelve original vertices keep five neighbours; all others have six.
        let fives = (0..mesh.positions.len() as u32)
            .filter(|&v| mesh.neighbours.of(v).len() == 5)
            .count();
        assert_eq!(fives, 12);
        for v in 0..mesh.positions.len() as u32 {
            let n = mesh.neighbours.of(v).len();
            assert!(n == 5 || n == 6);
            assert_eq!(mesh.vertex_faces.of(v).len(), n);
        }
    }

    #[test]
    fn a_huge_fan_builds_quickly_with_each_neighbour_once() {
        // 200k triangles around one vertex: the ring is built in linear time.
        let count = 200_000u32;
        let mut positions = vec![Vec3::ZERO];
        positions.extend((0..count).map(|i| {
            let a = i as f32 / count as f32 * std::f32::consts::TAU;
            Vec3::new(a.cos(), a.sin(), 0.0)
        }));
        let triangles: Vec<[u32; 3]> = (0..count)
            .map(|i| [0, 1 + i, 1 + (i + 1) % count])
            .collect();
        let mesh = Mesh::from_ordered(positions, triangles);
        let ring = mesh.neighbours.of(0);
        assert_eq!(ring.len(), count as usize);
        assert_eq!(&ring[..3], &[1, 2, 3]);
        assert_eq!(mesh.neighbours.of(5), &[0, 4, 6]);
    }

    #[test]
    fn local_normal_update_matches_a_full_recompute() {
        let mut mesh = Mesh::icosphere(3);
        let moved: Vec<u32> = (0..40).collect();
        for &v in &moved {
            mesh.positions[v as usize] *= 1.2;
        }
        let mut stamp = Stamp::new(mesh.positions.len());
        let mut affected = Vec::new();
        mesh.update_normals(&moved, &mut stamp, &mut affected);
        let local = mesh.normals.clone();
        mesh.recompute_normals();
        for (a, b) in local.iter().zip(&mesh.normals) {
            assert!(a.distance(*b) < 1e-5);
        }
    }

    /// Where [`Mesh::new`] spends its time, on a remeshed sphere.
    /// `cargo test --release -p splatstudy-core mesh_new_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn mesh_new_timing() {
        use std::time::Instant;
        let sphere = Mesh::icosphere(crate::sculpt::DEFAULT_LEVEL);
        // Surface nets' output, in the grid's order, as a remesh gives it.
        let raw = {
            use crate::voxel::{nets, sdf};
            let mut field = sdf::signed_distance(&sphere.positions, &sphere.triangles, 160, 0);
            let field = field.as_mut().unwrap();
            nets::untangle(field);
            let (positions, triangles) = nets::surface_nets(field);
            nets::keep_largest(&positions, &triangles)
        };
        for (name, positions, triangles) in [
            (
                "icosphere",
                sphere.positions.clone(),
                sphere.triangles.clone(),
            ),
            ("remeshed at 160", raw.0, raw.1),
        ] {
            let mut best = [f64::MAX; 5];
            for _ in 0..5 {
                let ms = |since: Instant| since.elapsed().as_secs_f64() * 1e3;
                let start = Instant::now();
                let (p, t) = spatial_order(positions.clone(), triangles.clone());
                let order = ms(start);
                let start = Instant::now();
                let faces = vertex_faces(p.len(), &t);
                let t_faces = ms(start);
                let mut m = Mesh {
                    normals: vec![Vec3::ZERO; p.len()],
                    vertex_faces: faces,
                    neighbours: Adjacency::default(),
                    positions: p,
                    triangles: t,
                };
                let start = Instant::now();
                m.neighbours = neighbours(&m);
                let t_ring = ms(start);
                let start = Instant::now();
                m.recompute_normals();
                let t_normals = ms(start);
                let start = Instant::now();
                std::hint::black_box(Mesh::new(positions.clone(), triangles.clone()));
                let t_all = ms(start);
                for (b, t) in best
                    .iter_mut()
                    .zip([order, t_faces, t_ring, t_normals, t_all])
                {
                    *b = b.min(t);
                }
            }
            let [order, faces, ring, normals, all] = best;
            println!(
                "{name}, {} vertices: Mesh::new {all:.1} ms = order {order:.1}                  + faces {faces:.1} + neighbours {ring:.1} + normals {normals:.1}",
                positions.len()
            );
        }
    }
}
