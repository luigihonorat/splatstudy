// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! A bounding volume hierarchy over the triangles: finds what the pointer
//! hits and which vertices lie inside the brush. The tree's shape is built
//! once (the topology never changes); after sculpting, `refit` only
//! recomputes the boxes, and `refit_touched` only those around the
//! triangles that moved.
//!
//! A box is the exact bounds of its triangles (min and max round
//! nothing), so refitting part of the tree gives the very boxes a full
//! refit would: replays stay bit for bit whichever is used.

use glam::Vec3;

use super::mesh::{Mesh, Stamp, morton_codes, radix_order};

const LEAF_SIZE: usize = 4;

#[derive(Clone, Copy, Debug)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// Leaf: first index into `order`; inner: index of the left child (the
    /// right child follows it).
    first: u32,
    /// Triangles in a leaf; 0 for an inner node.
    count: u32,
}

pub struct Bvh {
    nodes: Vec<Node>,
    /// Triangle indices, grouped by leaf.
    order: Vec<u32>,
    /// Each node's parent; the root's is itself.
    parent: Vec<u32>,
    /// The leaf holding each triangle.
    leaf_of: Vec<u32>,
    /// Nodes whose box is out of date, each once (`stale` marks them).
    touched: Vec<u32>,
    stale: Vec<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub distance: f32,
    pub point: Vec3,
    pub triangle: u32,
}

impl Bvh {
    /// The tree over `mesh`'s triangles: sorted along a Morton curve by
    /// their centroids (a linear-time radix sort), then split where the
    /// codes first differ, so each split is a cut of space, as the median
    /// split along the longest side it replaced was, at a fraction of the
    /// cost (a level of 3 million triangles is built in tens of
    /// milliseconds, not most of a second). Only arithmetic and integer
    /// sorts: the same tree on every machine.
    pub fn build(mesh: &Mesh) -> Bvh {
        let centroids: Vec<Vec3> = mesh
            .triangles
            .iter()
            .map(|t| t.iter().map(|&i| mesh.positions[i as usize]).sum::<Vec3>() / 3.0)
            .collect();
        let codes = morton_codes(&centroids);
        drop(centroids);
        let order = radix_order(&codes);
        let sorted: Vec<u32> = order.iter().map(|&t| codes[t as usize]).collect();
        drop(codes);
        let empty = Node {
            min: Vec3::ZERO,
            max: Vec3::ZERO,
            first: 0,
            count: 0,
        };
        let mut nodes = Vec::with_capacity(mesh.triangles.len() / LEAF_SIZE * 2 + 1);
        nodes.push(empty);
        // (node, start, end) ranges of `order` still to split.
        let mut todo = vec![(0usize, 0usize, order.len())];
        while let Some((node, start, end)) = todo.pop() {
            if end - start <= LEAF_SIZE {
                nodes[node].first = start as u32;
                nodes[node].count = (end - start) as u32;
                continue;
            }
            let mid = start + split(&sorted[start..end]);
            let left = nodes.len();
            nodes.push(empty);
            nodes.push(empty);
            nodes[node].first = left as u32;
            todo.push((left, start, mid));
            todo.push((left + 1, mid, end));
        }
        let mut parent = vec![0u32; nodes.len()];
        let mut leaf_of = vec![0u32; mesh.triangles.len()];
        for (i, node) in nodes.iter().enumerate() {
            if node.count > 0 {
                let first = node.first as usize;
                for &t in &order[first..first + node.count as usize] {
                    leaf_of[t as usize] = i as u32;
                }
            } else if !order.is_empty() {
                parent[node.first as usize] = i as u32;
                parent[node.first as usize + 1] = i as u32;
            }
        }
        let mut bvh = Bvh {
            stale: vec![false; nodes.len()],
            nodes,
            order,
            parent,
            leaf_of,
            touched: Vec::new(),
        };
        bvh.refit(mesh);
        bvh
    }

    /// Notes that the vertices `moved` have moved: the boxes around their
    /// triangles are refitted by the next [`Bvh::refit_touched`].
    pub fn touch(&mut self, mesh: &Mesh, moved: &[u32]) {
        for &v in moved {
            for &f in mesh.vertex_faces.of(v) {
                let leaf = self.leaf_of[f as usize];
                if !self.stale[leaf as usize] {
                    self.stale[leaf as usize] = true;
                    self.touched.push(leaf);
                }
            }
        }
    }

    /// Refits the boxes of the touched leaves and of everything above
    /// them; the same boxes as [`Bvh::refit`], in time proportional to
    /// what moved.
    pub fn refit_touched(&mut self, mesh: &Mesh) {
        // Up from each leaf, until a node already on the list.
        let mut at = 0;
        while at < self.touched.len() {
            let up = self.parent[self.touched[at] as usize];
            if !self.stale[up as usize] {
                self.stale[up as usize] = true;
                self.touched.push(up);
            }
            at += 1;
        }
        // Children come after their parent: in reverse, they go first.
        let mut touched = std::mem::take(&mut self.touched);
        touched.sort_unstable_by(|a, b| b.cmp(a));
        for &i in &touched {
            self.fit(mesh, i as usize);
            self.stale[i as usize] = false;
        }
        touched.clear();
        self.touched = touched;
    }

    /// Recomputes one node's box, from its triangles or its children.
    fn fit(&mut self, mesh: &Mesh, i: usize) {
        let node = self.nodes[i];
        let (min, max) = if node.count > 0 {
            let tris = &self.order[node.first as usize..(node.first + node.count) as usize];
            tris.iter().flat_map(|&t| mesh.triangles[t as usize]).fold(
                (Vec3::MAX, Vec3::MIN),
                |(lo, hi), v| {
                    let p = mesh.positions[v as usize];
                    (lo.min(p), hi.max(p))
                },
            )
        } else {
            let (l, r) = (
                self.nodes[node.first as usize],
                self.nodes[node.first as usize + 1],
            );
            (l.min.min(r.min), l.max.max(r.max))
        };
        self.nodes[i].min = min;
        self.nodes[i].max = max;
    }

    /// Recomputes every box from the current vertex positions. Children are
    /// always stored after their parent, so a reverse pass sees them first.
    pub fn refit(&mut self, mesh: &Mesh) {
        // No triangles: the root is an empty leaf that looks like an inner node.
        if self.order.is_empty() {
            return;
        }
        for i in (0..self.nodes.len()).rev() {
            self.fit(mesh, i);
        }
        for &i in &self.touched {
            self.stale[i as usize] = false;
        }
        self.touched.clear();
    }

    /// The nearest triangle hit by the ray `origin + t * dir`.
    pub fn raycast(&self, mesh: &Mesh, origin: Vec3, dir: Vec3) -> Option<Hit> {
        if self.order.is_empty() {
            return None;
        }
        let inv = dir.recip();
        let mut best: Option<Hit> = None;
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let node = &self.nodes[i as usize];
            let limit = best.map_or(f32::INFINITY, |h| h.distance);
            if !ray_hits_box(origin, inv, node.min, node.max, limit) {
                continue;
            }
            if node.count == 0 {
                stack.push(node.first);
                stack.push(node.first + 1);
                continue;
            }
            for &t in &self.order[node.first as usize..(node.first + node.count) as usize] {
                let [a, b, c] = mesh.triangles[t as usize].map(|v| mesh.positions[v as usize]);
                if let Some(d) = ray_triangle(origin, dir, a, b, c)
                    && d < best.map_or(f32::INFINITY, |h| h.distance)
                {
                    best = Some(Hit {
                        distance: d,
                        point: origin + dir * d,
                        triangle: t,
                    });
                }
            }
        }
        best
    }

    /// Appends to `out` every vertex within `radius` of `centre`, once each.
    pub fn vertices_in_sphere(
        &self,
        mesh: &Mesh,
        centre: Vec3,
        radius: f32,
        stamp: &mut Stamp,
        out: &mut Vec<u32>,
    ) {
        stamp.next();
        if self.order.is_empty() {
            return;
        }
        let r2 = radius * radius;
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let node = &self.nodes[i as usize];
            let nearest = centre.clamp(node.min, node.max);
            if nearest.distance_squared(centre) > r2 {
                continue;
            }
            if node.count == 0 {
                stack.push(node.first);
                stack.push(node.first + 1);
                continue;
            }
            for &t in &self.order[node.first as usize..(node.first + node.count) as usize] {
                for v in mesh.triangles[t as usize] {
                    if mesh.positions[v as usize].distance_squared(centre) <= r2 && stamp.mark(v) {
                        out.push(v);
                    }
                }
            }
        }
    }
}

/// Where to split a run of sorted Morton codes: at the first code whose
/// highest bit differing between the first and the last is set, so the
/// halves lie on either side of a plane; in the middle when all are equal.
/// Never 0 nor the whole run, for runs of two or more.
fn split(codes: &[u32]) -> usize {
    let (first, last) = (codes[0], codes[codes.len() - 1]);
    if first == last {
        return codes.len() / 2;
    }
    let bit = 31 - (first ^ last).leading_zeros();
    codes.partition_point(|&c| (c ^ first) >> bit == 0)
}

fn ray_hits_box(origin: Vec3, inv_dir: Vec3, min: Vec3, max: Vec3, limit: f32) -> bool {
    let t1 = (min - origin) * inv_dir;
    let t2 = (max - origin) * inv_dir;
    let near = t1.min(t2).max_element();
    let far = t1.max(t2).min_element();
    far >= near.max(0.0) && near <= limit
}

/// Möller–Trumbore; both sides count, so thin forms can be sculpted from behind.
fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raycast_finds_the_front_of_the_sphere() {
        let mesh = Mesh::icosphere(4);
        let bvh = Bvh::build(&mesh);
        let hit = bvh
            .raycast(&mesh, Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z)
            .expect("hit");
        assert!((hit.point.z - 1.0).abs() < 0.01, "{:?}", hit.point);
        assert!(
            bvh.raycast(&mesh, Vec3::new(3.0, 0.0, 5.0), Vec3::NEG_Z)
                .is_none()
        );
    }

    #[test]
    fn refitting_what_moved_gives_the_boxes_of_a_full_refit() {
        let mut mesh = Mesh::icosphere(4);
        let mut part = Bvh::build(&mesh);
        let mut full = Bvh::build(&mesh);
        for round in 0..4 {
            let centre = Vec3::new(0.3 * round as f32 - 0.4, 0.5, 0.7).normalize();
            let moved: Vec<u32> = (0..mesh.positions.len() as u32)
                .filter(|&v| mesh.positions[v as usize].distance(centre) < 0.3)
                .collect();
            assert!(!moved.is_empty());
            for &v in &moved {
                mesh.positions[v as usize] *= 1.1 + 0.05 * round as f32;
            }
            part.touch(&mesh, &moved);
            part.refit_touched(&mesh);
            full.refit(&mesh);
            for (a, b) in part.nodes.iter().zip(&full.nodes) {
                assert_eq!((a.min, a.max), (b.min, b.max));
            }
        }
        assert!(part.stale.iter().all(|s| !s));
    }

    #[test]
    fn sphere_query_matches_brute_force_after_refit() {
        let mut mesh = Mesh::icosphere(4);
        for p in &mut mesh.positions {
            p.y *= 1.5; // deform, then refit
        }
        let mut bvh = Bvh::build(&mesh);
        bvh.refit(&mesh);
        let centre = Vec3::new(0.3, 1.2, 0.6);
        let mut stamp = Stamp::new(mesh.positions.len());
        let mut found = Vec::new();
        bvh.vertices_in_sphere(&mesh, centre, 0.4, &mut stamp, &mut found);
        found.sort_unstable();
        let expected: Vec<u32> = (0..mesh.positions.len() as u32)
            .filter(|&v| mesh.positions[v as usize].distance(centre) <= 0.4)
            .collect();
        assert!(!expected.is_empty());
        assert_eq!(found, expected);
    }
}
