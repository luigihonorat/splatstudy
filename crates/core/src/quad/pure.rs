// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Quads only: the mesh read off at twice the edge length is split once
//! more, every polygon into as many quads as it has sides, round a new
//! vertex in its middle and through the middle of each of its edges (the
//! split of Catmull and Clark's subdivision, without their smoothing).
//! Quads become four quads, triangles three, pentagons five: no triangle
//! is left, and the loops of the coarser mesh carry on through the finer.
//! (Instant Meshes' "pure quad" mode does the same.)

use std::collections::{HashMap, HashSet};

use glam::Vec3;

use super::extract::Extracted;

/// Rounds of taking out corners only two polygons meet at.
const DISSOLVE_ROUNDS: usize = 4;

/// The polygons of `ex`, each wound counter-clockwise seen from outside:
/// each fan round a centre the mesh was given (a larger polygon, or a hole
/// filled), each pair of triangles listed as a quad, and every other
/// triangle on its own.
pub fn polygons(ex: &Extracted) -> Vec<Vec<u32>> {
    let n = ex.positions.len();
    let mut paired = vec![false; ex.triangles.len()];
    for &[a, b] in &ex.quads {
        for t in [a, b] {
            if let Some(p) = paired.get_mut(t as usize) {
                *p = true;
            }
        }
    }
    // The triangles round each vertex.
    let mut start = vec![0usize; n + 1];
    for t in &ex.triangles {
        for &v in t {
            start[v as usize + 1] += 1;
        }
    }
    for v in 0..n {
        start[v + 1] += start[v];
    }
    let mut fill = start.clone();
    let mut round = vec![0u32; start[n]];
    for (k, t) in ex.triangles.iter().enumerate() {
        for &v in t {
            round[fill[v as usize]] = k as u32;
            fill[v as usize] += 1;
        }
    }
    let mut used = vec![false; ex.triangles.len()];
    let mut gone = vec![false; n];
    let mut rim = vec![false; n];
    let mut polygons = Vec::new();
    for c in 0..n {
        let fan = &round[start[c]..start[c + 1]];
        if !ex.centres.get(c).copied().unwrap_or(false)
            || rim[c]
            || fan.len() < 3
            || fan.iter().any(|&t| paired[t as usize] || used[t as usize])
        {
            continue;
        }
        let Some(polygon) = link(&ex.triangles, fan, c as u32) else {
            continue;
        };
        if polygon.iter().any(|&v| gone[v as usize]) {
            continue;
        }
        for &t in fan {
            used[t as usize] = true;
        }
        gone[c] = true;
        for &v in &polygon {
            rim[v as usize] = true;
        }
        polygons.push(polygon);
    }
    for &[a, b] in &ex.quads {
        let (a, b) = (a as usize, b as usize);
        if a == b || used[a] || used[b] {
            continue;
        }
        if let Some(quad) = join(ex.triangles[a], ex.triangles[b]) {
            used[a] = true;
            used[b] = true;
            polygons.push(quad.to_vec());
        }
    }
    for (k, t) in ex.triangles.iter().enumerate() {
        if !used[k] {
            polygons.push(t.to_vec());
        }
    }
    polygons
}

/// The corners round `c` of the triangles `fan`, in order, if they go
/// once round it.
fn link(triangles: &[[u32; 3]], fan: &[u32], c: u32) -> Option<Vec<u32>> {
    let mut next: Vec<(u32, u32)> = fan
        .iter()
        .map(|&t| {
            let tri = triangles[t as usize];
            let k = tri.iter().position(|&v| v == c).unwrap_or(0);
            (tri[(k + 1) % 3], tri[(k + 2) % 3])
        })
        .collect();
    next.sort_unstable();
    if next.windows(2).any(|w| w[0].0 == w[1].0) {
        return None;
    }
    let first = next[0].0;
    let mut polygon = vec![first];
    let mut at = next[0].1;
    while at != first {
        if polygon.len() >= next.len() {
            return None;
        }
        polygon.push(at);
        at = next[next.binary_search_by_key(&at, |e| e.0).ok()?].1;
    }
    (polygon.len() == next.len()).then_some(polygon)
}

/// The quad two triangles make, sharing an edge wound opposite ways.
fn join(first: [u32; 3], second: [u32; 3]) -> Option<[u32; 4]> {
    for k in 0..3 {
        let [a, b, c] = [first[k], first[(k + 1) % 3], first[(k + 2) % 3]];
        if (0..3).any(|j| second[j] == a && second[(j + 1) % 3] == c) {
            let d = *second.iter().find(|&&v| v != a && v != c)?;
            return (d != b).then_some([a, b, c, d]);
        }
    }
    None
}

/// Takes out the corners only two polygons meet at, which would split
/// into quads folded flat against each other, wherever that keeps the
/// mesh closed: each of the two polygons loses the corner, and one left
/// with two corners goes.
pub fn dissolve_twos(polygons: &mut Vec<Vec<u32>>, vertices: usize) {
    let key = |a: u32, b: u32| (a.min(b), a.max(b));
    for _ in 0..DISSOLVE_ROUNDS {
        let mut count = vec![0u32; vertices];
        for p in polygons.iter() {
            for &v in p {
                count[v as usize] += 1;
            }
        }
        if !count.contains(&2) {
            return;
        }
        let mut of: HashMap<u32, Vec<usize>> = HashMap::new();
        let mut edges: HashSet<(u32, u32)> = HashSet::new();
        for (k, p) in polygons.iter().enumerate() {
            for (i, &v) in p.iter().enumerate() {
                if count[v as usize] == 2 {
                    of.entry(v).or_default().push(k);
                }
                edges.insert(key(v, p[(i + 1) % p.len()]));
            }
        }
        let mut twos: Vec<(u32, Vec<usize>)> = of.into_iter().collect();
        twos.sort_unstable();
        let mut busy = vec![false; polygons.len()];
        let mut changed = false;
        for (v, around) in twos {
            let &[p, q] = around.as_slice() else {
                continue;
            };
            if busy[p] || busy[q] {
                continue;
            }
            let beside = |k: usize| {
                let poly = &polygons[k];
                let i = poly.iter().position(|&x| x == v)?;
                let len = poly.len();
                Some((poly[(i + len - 1) % len], poly[(i + 1) % len]))
            };
            let (Some((a, b)), Some((c, d))) = (beside(p), beside(q)) else {
                continue;
            };
            // The two polygons meet along a → v → b one way, b → v → a the
            // other.
            if (c, d) != (b, a) || a == b {
                continue;
            }
            let (small_p, small_q) = (polygons[p].len() == 3, polygons[q].len() == 3);
            if (small_p && small_q) || (!small_p && !small_q && edges.contains(&key(a, b))) {
                continue;
            }
            for k in [p, q] {
                polygons[k].retain(|&x| x != v);
                busy[k] = true;
            }
            edges.insert(key(a, b));
            changed = true;
        }
        polygons.retain(|p| p.len() >= 3);
        if !changed {
            return;
        }
    }
}

/// The quads that split `polygons`, with their vertices: those of the
/// polygons (renumbered, unused ones left out), then one in the middle of
/// each edge, then one in the middle of each polygon. Each new vertex
/// starts at the average of what it splits.
pub struct Split {
    /// How many of the vertices are the polygons' corners: the first.
    pub corners: usize,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub quads: Vec<[u32; 4]>,
}

pub fn split(positions: &[Vec3], normals: &[Vec3], polygons: &[Vec<u32>]) -> Split {
    const NONE: u32 = u32::MAX;
    let mut renumber = vec![NONE; positions.len()];
    let mut out = Split {
        corners: 0,
        positions: Vec::new(),
        normals: Vec::new(),
        quads: Vec::with_capacity(polygons.iter().map(Vec::len).sum()),
    };
    for p in polygons {
        for &v in p {
            if renumber[v as usize] == NONE {
                renumber[v as usize] = out.positions.len() as u32;
                out.positions.push(positions[v as usize]);
                out.normals.push(normals[v as usize]);
            }
        }
    }
    out.corners = out.positions.len();
    let mut middles: HashMap<(u32, u32), u32> = HashMap::new();
    for p in polygons {
        let len = p.len();
        let corners: Vec<u32> = p.iter().map(|&v| renumber[v as usize]).collect();
        let mids: Vec<u32> = (0..len)
            .map(|i| {
                let (a, b) = (corners[i], corners[(i + 1) % len]);
                *middles.entry((a.min(b), a.max(b))).or_insert_with(|| {
                    let (a, b) = (a as usize, b as usize);
                    out.positions
                        .push((out.positions[a] + out.positions[b]) * 0.5);
                    out.normals
                        .push((out.normals[a] + out.normals[b]).normalize_or(out.normals[a]));
                    out.positions.len() as u32 - 1
                })
            })
            .collect();
        let c = out.positions.len() as u32;
        let count = len as f32;
        out.positions.push(
            corners
                .iter()
                .map(|&v| out.positions[v as usize])
                .sum::<Vec3>()
                / count,
        );
        out.normals.push(
            corners
                .iter()
                .map(|&v| out.normals[v as usize])
                .sum::<Vec3>()
                .normalize_or(Vec3::Y),
        );
        for i in 0..len {
            out.quads
                .push([corners[i], mids[i], c, mids[(i + len - 1) % len]]);
        }
    }
    out
}

/// Triangles, and the pairs of them that make each quad.
pub type Triangulated = (Vec<[u32; 3]>, Vec<[u32; 2]>);

/// The quads as triangles, each split along its shorter diagonal (or the
/// other one, where `flip` says so, or where the shorter one is taken
/// already): the triangles, and the pairs that make each quad. `None` if
/// both diagonals of a quad are taken.
pub fn triangulate(positions: &[Vec3], quads: &[[u32; 4]], flip: &[bool]) -> Option<Triangulated> {
    let key = |a: u32, b: u32| (a.min(b), a.max(b));
    let mut taken: HashSet<(u32, u32)> = HashSet::with_capacity(quads.len());
    let mut triangles = Vec::with_capacity(quads.len() * 2);
    let mut pairs = Vec::with_capacity(quads.len());
    for (k, &[a, b, c, d]) in quads.iter().enumerate() {
        let at = |i: u32| positions[i as usize];
        let ac = (key(a, c), [[a, b, c], [a, c, d]]);
        let bd = (key(b, d), [[a, b, d], [b, c, d]]);
        let shorter = at(a).distance_squared(at(c)) <= at(b).distance_squared(at(d));
        let (first, second) = if shorter != flip.get(k).copied().unwrap_or(false) {
            (ac, bd)
        } else {
            (bd, ac)
        };
        let pair = if taken.insert(first.0) {
            first.1
        } else if taken.insert(second.0) {
            second.1
        } else {
            return None;
        };
        let t = triangles.len() as u32;
        triangles.extend(pair);
        pairs.push([t, t + 1]);
    }
    Some((triangles, pairs))
}

/// The vertices joined to each vertex by a quad's side.
pub fn rings(vertices: usize, quads: &[[u32; 4]]) -> Vec<Vec<u32>> {
    let mut around = vec![Vec::new(); vertices];
    for q in quads {
        for i in 0..4 {
            let (u, v) = (q[i], q[(i + 1) % 4]);
            // Each side is in two quads, once each way: added from one.
            if u < v {
                around[u as usize].push(v);
                around[v as usize].push(u);
            }
        }
    }
    around
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> (Vec<Vec3>, Vec<Vec<u32>>) {
        let corners = (0..8)
            .map(|i| Vec3::new((i & 1) as f32, (i >> 1 & 1) as f32, (i >> 2) as f32) * 2.0 - 1.0)
            .collect();
        // Counter-clockwise seen from outside.
        let faces = vec![
            vec![0, 2, 3, 1],
            vec![4, 5, 7, 6],
            vec![0, 1, 5, 4],
            vec![2, 6, 7, 3],
            vec![0, 4, 6, 2],
            vec![1, 3, 7, 5],
        ];
        (corners, faces)
    }

    #[test]
    fn a_cube_splits_into_twenty_four_closed_quads() {
        let (corners, faces) = cube();
        let normals: Vec<Vec3> = corners.iter().map(|p| p.normalize()).collect();
        let s = split(&corners, &normals, &faces);
        assert_eq!(s.quads.len(), 24);
        // 8 corners, 12 edge middles, 6 face middles.
        assert_eq!(s.positions.len(), 26);
        let (triangles, pairs) = triangulate(&s.positions, &s.quads, &[]).unwrap();
        assert_eq!(pairs.len() * 2, triangles.len());
        assert!(super::super::is_closed_manifold(
            s.positions.len(),
            &triangles
        ));
        for t in &triangles {
            let [a, b, c] = t.map(|i| s.positions[i as usize]);
            assert!((b - a).cross(c - a).dot(a + b + c) > 0.0, "wound outwards");
        }
    }

    #[test]
    fn a_corner_two_faces_share_is_taken_out() {
        // A tetrahedron 0-1-2-3 whose edge 0-1 has a vertex 4 in its
        // middle: only the two quads on either side meet there.
        let mut polygons = vec![
            vec![0, 4, 1, 2],
            vec![1, 4, 0, 3],
            vec![2, 1, 3],
            vec![0, 2, 3],
        ];
        dissolve_twos(&mut polygons, 5);
        assert!(polygons.iter().all(|p| !p.contains(&4)), "{polygons:?}");
        // A tetrahedron.
        assert_eq!(polygons.len(), 4);
    }
}
