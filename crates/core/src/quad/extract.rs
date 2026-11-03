// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Reading a polygon mesh off the two fields.
//!
//! Two joined samples whose grids put them at the same grid point become
//! one vertex; two a grid step apart get an edge. Around each vertex the
//! edges are sorted by angle, and the faces are found by always taking the
//! next edge clockwise: every edge then borders exactly two faces, so the
//! mesh is closed, whatever the fields did. Most faces are quads; the rest
//! are triangles and the odd larger polygon where the field turns.

use std::collections::HashSet;

use glam::Vec3;

use super::field::{Frame, match_orientation, match_position};
use super::graph::Graph;

/// Vertices closer than this share of the grid spacing are merged.
const MERGE: f32 = 0.5;
/// Faces touching themselves are mended in at most this many passes.
const MAX_SPLIT_PASSES: usize = 20;

/// The polygon mesh, triangulated: each quad as two triangles (listed in
/// `quads`), larger polygons as a fan around a new vertex in the middle.
pub struct Extracted {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    /// Pairs of triangles that make one quad.
    pub quads: Vec<[u32; 2]>,
}

struct Sets {
    parent: Vec<u32>,
}

impl Sets {
    fn new(n: usize) -> Sets {
        Sets {
            parent: (0..n as u32).collect(),
        }
    }

    fn find(&mut self, mut i: u32) -> u32 {
        let mut root = i;
        while self.parent[root as usize] != root {
            root = self.parent[root as usize];
        }
        while self.parent[i as usize] != root {
            let next = self.parent[i as usize];
            self.parent[i as usize] = root;
            i = next;
        }
        root
    }

    fn join(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            // The lower index stays the root: the same input, the same sets.
            let (lo, hi) = (a.min(b), a.max(b));
            self.parent[hi as usize] = lo;
        }
    }
}

/// The vertices the samples make: where each sample went, and each
/// vertex's position and normal.
struct Vertices {
    of_sample: Vec<u32>,
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
}

fn gather(sets: &mut Sets, g: &Graph, p: &[Vec3]) -> Vertices {
    let n = g.len();
    const NONE: u32 = u32::MAX;
    let mut of_root = vec![NONE; n];
    let mut of_sample = vec![0; n];
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut counts = Vec::new();
    for i in 0..n {
        let root = sets.find(i as u32) as usize;
        if of_root[root] == NONE {
            of_root[root] = positions.len() as u32;
            positions.push(Vec3::ZERO);
            normals.push(Vec3::ZERO);
            counts.push(0.0f32);
        }
        let v = of_root[root] as usize;
        of_sample[i] = v as u32;
        positions[v] += p[i];
        normals[v] += g.normals[i];
        counts[v] += 1.0;
    }
    for v in 0..positions.len() {
        positions[v] /= counts[v];
        normals[v] = normals[v].normalize_or(Vec3::Y);
    }
    Vertices {
        of_sample,
        positions,
        normals,
    }
}

/// The edges between vertices, each once, lower index first.
fn edges_between(links: &[(u32, u32)], vertices: &Vertices) -> Vec<(u32, u32)> {
    let mut edges: Vec<(u32, u32)> = links
        .iter()
        .map(|&(i, j)| {
            let (a, b) = (
                vertices.of_sample[i as usize],
                vertices.of_sample[j as usize],
            );
            (a.min(b), a.max(b))
        })
        .filter(|(a, b)| a != b)
        .collect();
    edges.sort_unstable();
    edges.dedup();
    edges
}

/// Reads the polygon mesh off the fields `o` (orientation) and `p`
/// (position) on graph `g`, with grid spacing `scale`. `None` if it comes
/// out empty or tangled beyond use.
pub fn extract(g: &Graph, o: &[Vec3], p: &[Vec3], scale: f32) -> Option<Extracted> {
    let n = g.len();
    let mut sets = Sets::new(n);
    let mut links = Vec::new();
    for i in 0..n {
        for &j in g.of(i) {
            let j = j as usize;
            if j <= i {
                continue;
            }
            let (ni, nj) = (g.normals[i], g.normals[j]);
            let (qi, qj) = match_orientation(o[i], ni, o[j], nj);
            let (ti, tj) = (ni.cross(qi), nj.cross(qj));
            let here = Frame {
                v: g.positions[i],
                n: ni,
                q: qi,
                p: p[i],
            };
            let there = Frame {
                v: g.positions[j],
                n: nj,
                q: qj,
                p: p[j],
            };
            let (a, b) = match_position(here, there, scale);
            // Grid steps from each sample's own point to the shared one.
            let steps = |x: Vec3, origin: Vec3, q: Vec3, t: Vec3| {
                let d = x - origin;
                [(q.dot(d) / scale).round(), (t.dot(d) / scale).round()]
            };
            let si = steps(a, p[i], qi, ti);
            let sj = steps(b, p[j], qj, tj);
            let apart = (si[0] - sj[0]).abs() + (si[1] - sj[1]).abs();
            if apart == 0.0 {
                sets.join(i as u32, j as u32);
            } else if apart == 1.0 {
                links.push((i as u32, j as u32));
            }
        }
    }

    // Vertices, then merge those that ended up nearly on top of each other.
    let mut vertices = gather(&mut sets, g, p);
    let mut edges = edges_between(&links, &vertices);
    for _ in 0..3 {
        let mut merged = false;
        let mut first = vec![u32::MAX; vertices.positions.len()];
        for (i, &v) in vertices.of_sample.iter().enumerate() {
            if first[v as usize] == u32::MAX {
                first[v as usize] = i as u32;
            }
        }
        for &(a, b) in &edges {
            let d = vertices.positions[a as usize].distance(vertices.positions[b as usize]);
            if d < MERGE * scale {
                sets.join(first[a as usize], first[b as usize]);
                merged = true;
            }
        }
        if !merged {
            break;
        }
        vertices = gather(&mut sets, g, p);
        edges = edges_between(&links, &vertices);
    }

    let m = vertices.positions.len();
    let mut around: Vec<Vec<u32>> = vec![Vec::new(); m];
    for &(a, b) in &edges {
        around[a as usize].push(b);
        around[b as usize].push(a);
    }
    remove_loose(&mut around);

    // Edges around each vertex, counter-clockwise seen from outside.
    for (v, list) in around.iter_mut().enumerate() {
        let (pv, nv) = (vertices.positions[v], vertices.normals[v]);
        let x = nv.any_orthonormal_vector();
        let y = nv.cross(x);
        let angle = |u: u32| {
            let d = vertices.positions[u as usize] - pv;
            d.dot(y).atan2(d.dot(x))
        };
        list.sort_by(|&a, &b| angle(a).total_cmp(&angle(b)).then(a.cmp(&b)));
    }
    let mut passes = 0;
    let faces = loop {
        let faces = faces(&around)?;
        let touches: Vec<(u32, u32, u32)> = faces.iter().filter_map(|face| touch(face)).collect();
        if touches.is_empty() {
            break faces;
        }
        if passes == MAX_SPLIT_PASSES {
            return None;
        }
        // Splits that don't share vertices can be made together.
        let mut busy = vec![false; around.len()];
        for (x, p, q) in touches {
            let near = std::iter::once(x).chain(around[x as usize].iter().copied());
            if near.clone().any(|v| busy[v as usize]) {
                continue;
            }
            for v in near.collect::<Vec<_>>() {
                busy[v as usize] = true;
            }
            split(&mut around, x, p, q)?;
            vertices.positions.push(vertices.positions[x as usize]);
            vertices.normals.push(vertices.normals[x as usize]);
            // Each half a little towards its own edges, so the two don't
            // sit on top of each other.
            let y = around.len() - 1;
            for v in [x as usize, y] {
                let ring = &around[v];
                if !ring.is_empty() {
                    let centre = ring
                        .iter()
                        .map(|&u| vertices.positions[u as usize])
                        .sum::<Vec3>()
                        / ring.len() as f32;
                    vertices.positions[v] = vertices.positions[v].lerp(centre, 0.5);
                }
            }
        }
        remove_loose(&mut around);
        passes += 1;
    };

    let mut out = Extracted {
        positions: vertices.positions,
        normals: vertices.normals,
        triangles: Vec::new(),
        quads: Vec::new(),
    };
    let mut taken: HashSet<(u32, u32)> = HashSet::new();
    for (a, list) in around.iter().enumerate() {
        for &b in list {
            if (a as u32) < b {
                taken.insert((a as u32, b));
            }
        }
    }
    for face in &faces {
        triangulate(&mut out, &mut taken, face);
    }
    (!out.triangles.is_empty()).then_some(out)
}

/// The faces of the graph whose edges around each vertex are in
/// `around`, in counter-clockwise order: from each edge u → v, the face
/// goes on along the edge from v that comes just clockwise of v → u.
fn faces(around: &[Vec<u32>]) -> Option<Vec<Vec<u32>>> {
    let m = around.len();
    let mut offsets = Vec::with_capacity(m + 1);
    offsets.push(0usize);
    for list in around {
        offsets.push(offsets[offsets.len() - 1] + list.len());
    }
    let total = offsets[m];
    // Where u sits among v's edges, by binary search on a sorted copy.
    let sorted: Vec<Vec<(u32, u32)>> = around
        .iter()
        .map(|list| {
            let mut s: Vec<(u32, u32)> = list
                .iter()
                .enumerate()
                .map(|(k, &u)| (u, k as u32))
                .collect();
            s.sort_unstable();
            s
        })
        .collect();
    let slot_of = |v: usize, u: u32| -> Option<usize> {
        let s = &sorted[v];
        s.binary_search_by_key(&u, |&(x, _)| x)
            .ok()
            .map(|k| s[k].1 as usize)
    };
    let mut used = vec![false; total];
    let mut faces = Vec::new();
    for start_v in 0..m {
        for start_k in 0..around[start_v].len() {
            if used[offsets[start_v] + start_k] {
                continue;
            }
            let mut face = Vec::new();
            let (mut v, mut k) = (start_v, start_k);
            loop {
                let e = offsets[v] + k;
                if used[e] {
                    // Back at the start, or tangled: the edges are not
                    // symmetric, which can't happen for a graph.
                    if (v, k) != (start_v, start_k) {
                        return None;
                    }
                    break;
                }
                used[e] = true;
                face.push(v as u32);
                if face.len() > total {
                    return None;
                }
                let u = around[v][k] as usize;
                let back = slot_of(u, v as u32)?;
                let deg = around[u].len();
                v = u;
                k = (back + deg - 1) % deg;
            }
            faces.push(face);
        }
    }
    Some(faces)
}

/// Takes away loose ends, which border no face, until none are left.
fn remove_loose(around: &mut [Vec<u32>]) {
    let mut loose: Vec<u32> = (0..around.len() as u32)
        .filter(|&v| around[v as usize].len() == 1)
        .collect();
    while let Some(v) = loose.pop() {
        let ends = std::mem::take(&mut around[v as usize]);
        for u in ends {
            let list = &mut around[u as usize];
            list.retain(|&x| x != v);
            if list.len() == 1 {
                loose.push(u);
            }
        }
    }
}

/// A vertex that `face` passes twice, and the vertices the face comes
/// from each time.
fn touch(face: &[u32]) -> Option<(u32, u32, u32)> {
    let len = face.len();
    let mut seen: Vec<(u32, usize)> = face.iter().enumerate().map(|(k, &v)| (v, k)).collect();
    seen.sort_unstable();
    let w = seen.windows(2).find(|w| w[0].0 == w[1].0)?;
    let before = |k: usize| face[(k + len - 1) % len];
    Some((w[0].0, before(w[0].1), before(w[1].1)))
}

/// Splits vertex `x` in two where a face touches itself there, coming in
/// from `p` once and from `q` the other time: the edges from `p` round to
/// just before `q` stay, the rest go to a new vertex at the same place.
/// Whatever hung inside the face through `x` comes loose from it (and is
/// dropped later, as a separate piece).
fn split(around: &mut Vec<Vec<u32>>, x: u32, p: u32, q: u32) -> Option<()> {
    let list = &around[x as usize];
    let ip = list.iter().position(|&u| u == p)?;
    let iq = list.iter().position(|&u| u == q)?;
    let len = list.len();
    let arc = |from: usize, to: usize| -> Vec<u32> {
        let mut out = Vec::new();
        let mut k = from;
        while k != to {
            out.push(list[k]);
            k = (k + 1) % len;
        }
        out
    };
    let (kept, moved) = (arc(ip, iq), arc(iq, ip));
    let y = around.len() as u32;
    for &u in &moved {
        for w in around[u as usize].iter_mut().filter(|w| **w == x) {
            *w = y;
        }
    }
    around[x as usize] = kept;
    around.push(moved);
    Some(())
}

/// Splits a polygon into triangles: a quad along its shorter diagonal
/// (unless that pair is joined already), anything larger around a new
/// vertex in its middle.
fn triangulate(out: &mut Extracted, taken: &mut HashSet<(u32, u32)>, polygon: &[u32]) {
    let key = |a: u32, b: u32| (a.min(b), a.max(b));
    match *polygon {
        [a, b, c] => out.triangles.push([a, b, c]),
        [a, b, c, d] => {
            let at = |i: u32| out.positions[i as usize];
            let ac = (key(a, c), [[a, b, c], [a, c, d]]);
            let bd = (key(b, d), [[a, b, d], [b, c, d]]);
            let (first, second) = if at(a).distance_squared(at(c)) <= at(b).distance_squared(at(d))
            {
                (ac, bd)
            } else {
                (bd, ac)
            };
            for (diagonal, pair) in [first, second] {
                if taken.insert(diagonal) {
                    let t = out.triangles.len() as u32;
                    out.triangles.extend(pair);
                    out.quads.push([t, t + 1]);
                    return;
                }
            }
            fan(out, polygon);
        }
        _ => fan(out, polygon),
    }
}

fn fan(out: &mut Extracted, polygon: &[u32]) {
    let count = polygon.len() as f32;
    let centre = polygon
        .iter()
        .map(|&v| out.positions[v as usize])
        .sum::<Vec3>()
        / count;
    let normal = polygon
        .iter()
        .map(|&v| out.normals[v as usize])
        .sum::<Vec3>()
        .normalize_or(Vec3::Y);
    let c = out.positions.len() as u32;
    out.positions.push(centre);
    out.normals.push(normal);
    for (k, &a) in polygon.iter().enumerate() {
        let b = polygon[(k + 1) % polygon.len()];
        out.triangles.push([a, b, c]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_triangle_hanging_inside_a_face_comes_loose() {
        // A square 0-1-2-3 with a triangle 1-4-5 hanging inside it from
        // corner 1, counter-clockwise seen from +Z.
        let positions = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.6, 0.3, 0.0),
            Vec3::new(0.7, 0.4, 0.0),
        ];
        let mut around: Vec<Vec<u32>> = vec![Vec::new(); 6];
        for (a, b) in [(0, 1), (1, 2), (2, 3), (3, 0), (1, 4), (4, 5), (5, 1)] {
            around[a].push(b as u32);
            around[b].push(a as u32);
        }
        for (v, list) in around.iter_mut().enumerate() {
            let angle = |u: u32| {
                let d = positions[u as usize] - positions[v];
                d.y.atan2(d.x)
            };
            list.sort_by(|&a, &b| angle(a).total_cmp(&angle(b)));
        }
        let found = faces(&around).unwrap();
        let (x, p, q) = found
            .iter()
            .find_map(|f| touch(f))
            .expect("a face touches itself");
        assert_eq!(x, 1);
        split(&mut around, x, p, q).unwrap();
        let found = faces(&around).unwrap();
        assert!(found.iter().all(|f| touch(f).is_none()), "{found:?}");
        // The square's two faces and the triangle's two, apart now.
        assert_eq!(found.len(), 4);
        assert!(found.iter().any(|f| f.contains(&6)));
    }

    #[test]
    fn the_faces_of_a_square_grid_on_a_cube_are_quads() {
        // A cube's eight corners and twelve edges: six quad faces.
        let corners: Vec<Vec3> = (0..8)
            .map(|i| Vec3::new((i & 1) as f32, (i >> 1 & 1) as f32, (i >> 2) as f32) * 2.0 - 1.0)
            .collect();
        let mut around: Vec<Vec<u32>> = vec![Vec::new(); 8];
        for a in 0..8u32 {
            for bit in [1, 2, 4] {
                around[a as usize].push(a ^ bit);
            }
        }
        for (v, list) in around.iter_mut().enumerate() {
            let n = corners[v].normalize();
            let x = n.any_orthonormal_vector();
            let y = n.cross(x);
            let angle = |u: u32| {
                let d = corners[u as usize] - corners[v];
                d.dot(y).atan2(d.dot(x))
            };
            list.sort_by(|&a, &b| angle(a).total_cmp(&angle(b)));
        }
        let faces = faces(&around).unwrap();
        assert_eq!(faces.len(), 6);
        assert!(faces.iter().all(|f| f.len() == 4));
        // Wound counter-clockwise seen from outside.
        for f in &faces {
            let [a, b, c] = [f[0], f[1], f[2]].map(|i| corners[i as usize]);
            assert!((b - a).cross(c - a).dot(a + c) > 0.0);
        }
    }
}
