// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Quad remeshing: the clay rebuilt as even quads whose edges follow the
//! shape's curves, a mesh to sculpt on with clean loops.
//!
//! After instant field-aligned meshes (Jakob, Tarini, Panozzo and
//! Sorkine-Hornung, 2015):
//!
//! 1. The clay is voxel remeshed finely first (two samples per quad
//!    edge) and smoothed: that closes, merges and evens out whatever it
//!    was, and irons out bumps smaller than a quad, so the rest always
//!    starts from a clean, closed surface.
//! 2. Two fields are smoothed over its vertices, coarse to fine
//!    ([`field`]): which way the quad edges run, and where the corners
//!    fall.
//! 3. The quads are read off the fields ([`extract`]), relaxed so they are
//!    even, and pulled back onto the surface.
//! 4. Triangles that came out folded over (where the field turns sharply,
//!    or in deep, narrow creases) are unfolded, mended by collapsing or
//!    flipping edges, or as a last resort cut out and filled with a fan.
//! 5. The result is checked: closed, every vertex a single fan, nothing
//!    folded. If not, it is tried again from another start, then given up.
//!
//! The clay stays a triangle mesh: each quad is two triangles, listed in
//! [`Mesh::quads`] so export can write quads.

pub mod extract;
pub mod field;
pub mod graph;
pub mod surface;

use std::collections::HashMap;

use glam::Vec3;

use crate::sculpt::Mesh;
use crate::voxel;
use extract::Extracted;
use graph::{Graph, Hierarchy};
use surface::Surface;

/// The quad count slider's range and starting value.
pub const MIN_QUADS: u32 = 1_000;
pub const MAX_QUADS: u32 = 20_000;
pub const DEFAULT_QUADS: u32 = 5_000;

/// Voxels along a quad edge in the fine voxel remesh the fields run on.
const SAMPLES_PER_EDGE: f32 = 2.0;
/// The fine voxel remesh is made coarser beyond this many vertices.
const MAX_SAMPLES: usize = 250_000;
/// At most one quad for this many vertices of the clay.
const INPUT_VERTICES_PER_QUAD: usize = 4;
/// Smoothing passes over the voxel remesh first.
const SMOOTH_PASSES: usize = 10;
/// Passes evening out the quads at the end.
const RELAX_PASSES: usize = 6;
/// Further passes, only around triangles that came out flipped.
const FIX_PASSES: usize = 8;
/// Rounds of mending triangles still folded after those.
const MEND_ROUNDS: usize = 6;
/// Then rounds of cutting them out and filling the hole, before giving up.
const PATCH_ROUNDS: usize = 4;
/// Tries, each from another starting orientation and a slightly larger
/// grid, before giving up.
const ATTEMPTS: u32 = 3;

/// The clay rebuilt with about `quads` quads, the same size and in the
/// same place. `None` if the input has no surface or no clean quad mesh
/// came out of it.
pub fn remesh(mesh: &Mesh, quads: u32) -> Option<Mesh> {
    if mesh.triangles.is_empty() || mesh.positions.iter().any(|p| !p.is_finite()) {
        return None;
    }
    // Quads much finer than the clay's own triangles would follow its
    // every bump, and fold over them.
    let finest = (mesh.positions.len() / INPUT_VERTICES_PER_QUAD) as u32;
    let target = quads.min(finest).clamp(100, MAX_QUADS) as f32;
    let (min, max) = mesh
        .positions
        .iter()
        .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), &p| {
            (lo.min(p), hi.max(p))
        });
    let size = (max - min).max_element();
    let area = area(&mesh.positions, &mesh.triangles);
    if !(size > 0.0 && area > 0.0 && size.is_finite() && area.is_finite()) {
        return None;
    }
    let edge = (area / target).sqrt();
    let resolution = (size * SAMPLES_PER_EDGE / edge)
        .ceil()
        .clamp(voxel::MIN_RESOLUTION as f32, voxel::MAX_RESOLUTION as f32);
    let mut base = voxel::remesh(mesh, resolution as u32)?;
    if base.positions.len() > MAX_SAMPLES {
        // A tangle with far more surface than its triangles' area said
        // (a soup of crossing triangles): coarser, to keep it quick.
        let coarser = resolution * (MAX_SAMPLES as f32 / base.positions.len() as f32).sqrt();
        base = voxel::remesh(mesh, coarser.max(voxel::MIN_RESOLUTION as f32) as u32)?;
    }
    smooth(&mut base);
    // The voxel remesh merged and closed things: measure again.
    let edge = (area_of(&base) / target).sqrt();
    if !(edge > 0.0 && edge.is_finite()) {
        return None;
    }
    let hierarchy = Hierarchy::build(Graph::of_mesh(&base));
    let surface = Surface::new(&base, edge * 0.5);
    (0..ATTEMPTS).find_map(|attempt| {
        let scale = edge * (1.0 + 0.04 * attempt as f32);
        let o = field::orientation(&hierarchy, attempt);
        let p = field::position(&hierarchy, &o, scale);
        let extracted = extract::extract(&hierarchy.levels[0], &o, &p, scale)?;
        finish(extracted, &surface)
    })
}

/// Smooths away bumps smaller than a quad (a scan's or splats' noise),
/// which would fold the quads laid over them, without shrinking the shape
/// (Taubin's λ|μ smoothing).
fn smooth(mesh: &mut Mesh) {
    for _ in 0..SMOOTH_PASSES {
        for factor in [0.5, -0.53] {
            let moved: Vec<Vec3> = (0..mesh.positions.len() as u32)
                .map(|v| {
                    let ring = mesh.neighbours.of(v);
                    let p = mesh.positions[v as usize];
                    if ring.is_empty() {
                        return p;
                    }
                    let centre = ring
                        .iter()
                        .map(|&u| mesh.positions[u as usize])
                        .sum::<Vec3>()
                        / ring.len() as f32;
                    p + (centre - p) * factor
                })
                .collect();
            mesh.positions = moved;
        }
    }
    mesh.recompute_normals();
}

fn area(positions: &[Vec3], triangles: &[[u32; 3]]) -> f32 {
    triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| positions.get(i as usize).copied().unwrap_or(Vec3::ZERO));
            (b - a).cross(c - a).length() * 0.5
        })
        .sum()
}

fn area_of(mesh: &Mesh) -> f32 {
    area(&mesh.positions, &mesh.triangles)
}

/// The Euler characteristic, V − E + F, of a closed mesh: 2 for a sphere,
/// 2 less for each handle.
#[cfg(test)]
fn euler(vertices: usize, triangles: &[[u32; 3]]) -> i64 {
    // Every edge borders two triangles.
    vertices as i64 - (triangles.len() * 3 / 2) as i64 + triangles.len() as i64
}

/// Keeps the largest piece, evens out the quads on the surface, mends
/// triangles that came out flipped, checks the result and makes it the
/// clay's mesh.
fn finish(ex: Extracted, surface: &Surface) -> Option<Mesh> {
    let mut ex = largest_piece(ex);
    if ex.triangles.is_empty() {
        return None;
    }
    for v in 0..ex.positions.len() {
        land(surface, &mut ex, v);
    }
    let mut around = rings(&ex);
    let all: Vec<u32> = (0..ex.positions.len() as u32).collect();
    for _ in 0..RELAX_PASSES {
        relax(surface, &mut ex, &around, &all, 0.5, true);
    }
    // Triangles folded over: their corners are moved further, a few times.
    // If that is not enough, they are mended (see `mend`), and as a last
    // resort the patch round them is cut out and filled again (see
    // `patch`), wider each time.
    for round in 0..=MEND_ROUNDS + PATCH_ROUNDS {
        for pass in 0..FIX_PASSES {
            let corners: Vec<u32> = wrong(&ex)
                .into_iter()
                .flat_map(|t| ex.triangles[t])
                .collect();
            if corners.is_empty() {
                break;
            }
            // Along the surface first, the corners and their neighbours.
            // Then, if the surface itself is too rough there, freely, a
            // wider patch many times over, which unfolds it (a little off
            // the surface).
            let free = pass >= FIX_PASSES / 2;
            let (rings_out, times) = if free { (3, 20) } else { (1, 1) };
            let near = grow(&around, corners, rings_out);
            for _ in 0..times {
                relax(surface, &mut ex, &around, &near, 1.0, !free);
            }
        }
        let bad = wrong(&ex);
        if bad.is_empty() {
            break;
        }
        if round == MEND_ROUNDS + PATCH_ROUNDS {
            return None;
        }
        if round < MEND_ROUNDS {
            if mend(&mut ex, &bad) == 0 {
                continue;
            }
        } else {
            let width = round - MEND_ROUNDS + 1;
            let Some(filled) = patch(&mut ex, &around, &bad, width) else {
                continue;
            };
            around = rings(&ex);
            for &v in &filled {
                land(surface, &mut ex, v as usize);
            }
            let near = grow(&around, filled, 1);
            for _ in 0..4 {
                relax(surface, &mut ex, &around, &near, 0.5, true);
            }
        }
        ex = largest_piece(ex);
        around = rings(&ex);
    }
    is_closed_manifold(ex.positions.len(), &ex.triangles)
        .then(|| with_quads(ex.positions, ex.triangles, &ex.quads))
}

/// Puts vertex `v` on the surface, on the side it faces.
fn land(surface: &Surface, ex: &mut Extracted, v: usize) {
    if let Some((p, n)) = surface.nearest(ex.positions[v], ex.normals[v]) {
        ex.positions[v] = p;
        ex.normals[v] = n;
    }
}

/// Moves each vertex in `which` by `step` of the way towards the middle of
/// its neighbours (`around`): along the surface and back onto it, or
/// freely if not `along`.
fn relax(
    surface: &Surface,
    ex: &mut Extracted,
    around: &[Vec<u32>],
    which: &[u32],
    step: f32,
    along: bool,
) {
    let moved: Vec<Vec3> = which
        .iter()
        .map(|&v| {
            let (v, ring) = (v as usize, &around[v as usize]);
            let p = ex.positions[v];
            if ring.is_empty() {
                return p;
            }
            let centre =
                ring.iter().map(|&u| ex.positions[u as usize]).sum::<Vec3>() / ring.len() as f32;
            let (d, n) = (centre - p, ex.normals[v]);
            if along {
                p + (d - n * n.dot(d)) * step
            } else {
                p + d * step
            }
        })
        .collect();
    for (&v, p) in which.iter().zip(moved) {
        ex.positions[v as usize] = p;
        if along {
            land(surface, ex, v as usize);
        }
    }
}

/// The vertices joined to each vertex by a polygon edge: every triangle
/// edge but the diagonals splitting the quads.
fn rings(ex: &Extracted) -> Vec<Vec<u32>> {
    let mut diagonals: Vec<(u32, u32)> = ex
        .quads
        .iter()
        .filter_map(|&[a, b]| {
            let (ta, tb) = (ex.triangles[a as usize], ex.triangles[b as usize]);
            let mut shared = ta.iter().copied().filter(|v| tb.contains(v));
            let (x, y) = (shared.next()?, shared.next()?);
            Some((x.min(y), x.max(y)))
        })
        .collect();
    diagonals.sort_unstable();
    let mut around = vec![Vec::new(); ex.positions.len()];
    for &[a, b, c] in &ex.triangles {
        for (u, v) in [(a, b), (b, c), (c, a)] {
            if u < v && diagonals.binary_search(&(u, v)).is_err() {
                around[u as usize].push(v);
                around[v as usize].push(u);
            }
        }
    }
    around
}

/// The triangles folded over against their neighbours (facing away from
/// the mesh's own normals at their corners).
fn wrong(ex: &Extracted) -> Vec<usize> {
    let face = |t: [u32; 3]| {
        let [a, b, c] = t.map(|i| ex.positions[i as usize]);
        (b - a).cross(c - a)
    };
    let mut normals = vec![Vec3::ZERO; ex.positions.len()];
    for &t in &ex.triangles {
        let f = face(t);
        for v in t {
            normals[v as usize] += f;
        }
    }
    (0..ex.triangles.len())
        .filter(|&k| {
            let t = ex.triangles[k];
            let f = face(t);
            let around: Vec3 = t.iter().map(|&i| normals[i as usize]).sum();
            // Slivers with next to no area are left: they show nothing.
            // (Not finite counts as folded too.)
            let facing = f.dot(around) + 1e-3 * f.length() * around.length();
            facing.is_nan() || facing < 0.0
        })
        .collect()
}

/// Mends each of the `bad` triangles, where that keeps the mesh a closed
/// surface, and not two next to each other at once: collapses its
/// shortest edge that can go (the ends meet in the middle), or else flips
/// its longest edge to join the two corners across it. Returns how many
/// were mended.
fn mend(ex: &mut Extracted, bad: &[usize]) -> usize {
    let n = ex.positions.len();
    let mut ring: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut across: HashMap<(u32, u32), usize> = HashMap::with_capacity(ex.triangles.len() * 3);
    for (k, &[a, b, c]) in ex.triangles.iter().enumerate() {
        for (u, v) in [(a, b), (b, c), (c, a)] {
            ring[u as usize].push(v);
            across.insert((u, v), k);
        }
    }
    for r in &mut ring {
        r.sort_unstable();
        r.dedup();
    }
    let mut busy = vec![false; n];
    let mut gone = vec![false; ex.triangles.len()];
    // Triangles changed in place, no longer halves of a quad.
    let mut unpaired = vec![false; ex.triangles.len()];
    let mut into: Vec<u32> = (0..n as u32).collect();
    let mut mended = 0;
    for &t in bad {
        let tri = ex.triangles[t];
        if gone[t] || unpaired[t] || tri.iter().any(|&v| busy[v as usize]) {
            continue;
        }
        let length =
            |(u, v): (u32, u32)| ex.positions[u as usize].distance(ex.positions[v as usize]);
        let mut edges = [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])];
        edges.sort_by(|&a, &b| length(a).total_cmp(&length(b)));
        // An edge u → v, its two triangles, and the corners across it.
        let wings = |(u, v): (u32, u32)| {
            let (t1, t2) = (*across.get(&(u, v))?, *across.get(&(v, u))?);
            let opposite = |k: usize| ex.triangles[k].into_iter().find(|&x| x != u && x != v);
            let (a, b) = (opposite(t1)?, opposite(t2)?);
            (a != b).then_some((t1, t2, a, b))
        };
        let valence = |x: u32| ring[x as usize].len();
        let collapsible = edges.into_iter().find_map(|(u, v)| {
            let (t1, t2, a, b) = wings((u, v))?;
            let (ru, rv) = (&ring[u as usize], &ring[v as usize]);
            let shared: Vec<u32> = ru
                .iter()
                .copied()
                .filter(|x| rv.binary_search(x).is_ok())
                .collect();
            let mut expected = [a, b];
            expected.sort_unstable();
            // The vertex left, and the two across, keep three edges at
            // least, and the vertex left no more than eight (many thin
            // triangles round one vertex fold all the more).
            let joined = (ru.len() + rv.len()).saturating_sub(4);
            let thin = !(3..=8).contains(&joined) || valence(a) < 4 || valence(b) < 4;
            (shared == expected && !thin).then_some((u, v, t1, t2))
        });
        let mut mark = |xs: &[u32]| {
            for &x in xs {
                busy[x as usize] = true;
                for &y in &ring[x as usize] {
                    busy[y as usize] = true;
                }
            }
        };
        if let Some((u, v, t1, t2)) = collapsible {
            mark(&[u, v]);
            gone[t1] = true;
            gone[t2] = true;
            into[v as usize] = u;
            ex.positions[u as usize] = (ex.positions[u as usize] + ex.positions[v as usize]) * 0.5;
            mended += 1;
            continue;
        }
        let flippable = edges.into_iter().rev().find_map(|(u, v)| {
            let (t1, t2, a, b) = wings((u, v))?;
            let joined = ring[a as usize].binary_search(&b).is_ok();
            (!joined && valence(u) > 3 && valence(v) > 3).then_some((u, v, t1, t2, a, b))
        });
        if let Some((u, v, t1, t2, a, b)) = flippable {
            // u → v → a and v → u → b become u → b → a and b → v → a.
            mark(&[u, v, a, b]);
            ex.triangles[t1] = [u, b, a];
            ex.triangles[t2] = [b, v, a];
            unpaired[t1] = true;
            unpaired[t2] = true;
            mended += 1;
        }
    }
    let mut renumber = vec![u32::MAX; ex.triangles.len()];
    let mut triangles = Vec::with_capacity(ex.triangles.len());
    for (k, t) in ex.triangles.iter().enumerate() {
        if !gone[k] {
            renumber[k] = triangles.len() as u32;
            triangles.push(t.map(|v| into[v as usize]));
        }
    }
    ex.triangles = triangles;
    ex.quads
        .retain(|q| !q.iter().any(|&t| unpaired[t as usize]));
    ex.quads = ex
        .quads
        .iter()
        .map(|q| q.map(|t| renumber[t as usize]))
        .filter(|q| !q.contains(&u32::MAX))
        .collect();
    mended
}

/// `start` and every vertex up to `width` edges away from it.
fn grow(around: &[Vec<u32>], start: Vec<u32>, width: usize) -> Vec<u32> {
    let mut near = start;
    near.sort_unstable();
    near.dedup();
    for _ in 0..width {
        near = near
            .iter()
            .flat_map(|&v| std::iter::once(v).chain(around[v as usize].iter().copied()))
            .collect();
        near.sort_unstable();
        near.dedup();
    }
    near
}

/// Cuts out the triangles within `width` edges of the `bad` ones and fills
/// each hole left with a fan round a new vertex in its middle. The new
/// vertices are returned with the hole's rim, or `None` if a rim passes a
/// vertex twice (the patch has to be wider).
fn patch(ex: &mut Extracted, around: &[Vec<u32>], bad: &[usize], width: usize) -> Option<Vec<u32>> {
    let corners: Vec<u32> = bad.iter().flat_map(|&t| ex.triangles[t]).collect();
    let mut inside = vec![false; ex.positions.len()];
    for v in grow(around, corners, width.saturating_sub(1)) {
        inside[v as usize] = true;
    }
    let cut: Vec<bool> = ex
        .triangles
        .iter()
        .map(|t| t.iter().any(|&v| inside[v as usize]))
        .collect();
    // The rim: edges u → v of kept triangles whose other side is cut,
    // followed v → u round the hole.
    let mut cut_edges: Vec<(u32, u32)> = Vec::new();
    for (k, &[a, b, c]) in ex.triangles.iter().enumerate() {
        if cut[k] {
            cut_edges.extend([(a, b), (b, c), (c, a)]);
        }
    }
    cut_edges.sort_unstable();
    let mut rim: Vec<(u32, u32)> = Vec::new();
    for (k, &[a, b, c]) in ex.triangles.iter().enumerate() {
        if !cut[k] {
            for (u, v) in [(a, b), (b, c), (c, a)] {
                if cut_edges.binary_search(&(v, u)).is_ok() {
                    rim.push((v, u));
                }
            }
        }
    }
    rim.sort_unstable();
    if rim.windows(2).any(|w| w[0].0 == w[1].0) {
        return None;
    }
    let next = |v: u32| rim.binary_search_by_key(&v, |e| e.0).ok().map(|k| rim[k].1);
    let mut done = vec![false; rim.len()];
    let mut filled = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    for start in 0..rim.len() {
        if done[start] {
            continue;
        }
        let mut hole = Vec::new();
        let mut at = rim[start].0;
        loop {
            let k = rim.binary_search_by_key(&at, |e| e.0).ok()?;
            if done[k] {
                break;
            }
            done[k] = true;
            hole.push(at);
            at = next(at)?;
        }
        if at != rim[start].0 || hole.len() < 3 {
            return None;
        }
        let centre =
            hole.iter().map(|&v| ex.positions[v as usize]).sum::<Vec3>() / hole.len() as f32;
        let normal = hole
            .iter()
            .map(|&v| ex.normals[v as usize])
            .sum::<Vec3>()
            .normalize_or(Vec3::Y);
        let c = ex.positions.len() as u32;
        ex.positions.push(centre);
        ex.normals.push(normal);
        for (i, &u) in hole.iter().enumerate() {
            let v = hole[(i + 1) % hole.len()];
            triangles.push([u, v, c]);
        }
        filled.push(c);
        filled.extend(hole);
    }
    if triangles.is_empty() {
        return None;
    }
    let mut renumber = vec![u32::MAX; ex.triangles.len()];
    let mut kept = Vec::with_capacity(ex.triangles.len() + triangles.len());
    for (k, &t) in ex.triangles.iter().enumerate() {
        if !cut[k] {
            renumber[k] = kept.len() as u32;
            kept.push(t);
        }
    }
    kept.extend(triangles);
    ex.triangles = kept;
    ex.quads = ex
        .quads
        .iter()
        .map(|q| q.map(|t| renumber[t as usize]))
        .filter(|q| !q.contains(&u32::MAX))
        .collect();
    Some(filled)
}

/// Only the piece with the most triangles, without vertices nothing uses.
fn largest_piece(ex: Extracted) -> Extracted {
    let n = ex.positions.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn root(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            let up = parent[parent[i as usize] as usize];
            parent[i as usize] = up;
            i = up;
        }
        i
    }
    for t in &ex.triangles {
        for k in 1..3 {
            let (a, b) = (root(&mut parent, t[0]), root(&mut parent, t[k]));
            if a != b {
                parent[a.max(b) as usize] = a.min(b);
            }
        }
    }
    let mut count = vec![0usize; n];
    for t in &ex.triangles {
        count[root(&mut parent, t[0]) as usize] += 1;
    }
    let Some(keep) = (0..n).max_by_key(|&v| (count[v], std::cmp::Reverse(v))) else {
        return ex;
    };
    let keep = keep as u32;
    const NONE: u32 = u32::MAX;
    let mut remap = vec![NONE; n];
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut tri_remap = vec![NONE; ex.triangles.len()];
    let mut triangles = Vec::new();
    for (k, t) in ex.triangles.iter().enumerate() {
        if root(&mut parent, t[0]) != keep {
            continue;
        }
        let t = t.map(|v| {
            if remap[v as usize] == NONE {
                remap[v as usize] = positions.len() as u32;
                positions.push(ex.positions[v as usize]);
                normals.push(ex.normals[v as usize]);
            }
            remap[v as usize]
        });
        tri_remap[k] = triangles.len() as u32;
        triangles.push(t);
    }
    let quads = ex
        .quads
        .iter()
        .map(|q| q.map(|t| tri_remap[t as usize]))
        .filter(|q| !q.contains(&NONE))
        .collect();
    Extracted {
        positions,
        normals,
        triangles,
        quads,
    }
}

/// Every edge borders exactly two triangles, wound opposite ways, and the
/// triangles around each vertex form a single fan.
pub fn is_closed_manifold(vertices: usize, triangles: &[[u32; 3]]) -> bool {
    let mut directed: Vec<(u32, u32)> = Vec::with_capacity(triangles.len() * 3);
    // Around each vertex, from one neighbour to the next.
    let mut turns: Vec<(u32, u32, u32)> = Vec::with_capacity(triangles.len() * 3);
    for &[a, b, c] in triangles {
        if a == b || b == c || c == a || [a, b, c].iter().any(|&v| v as usize >= vertices) {
            return false;
        }
        directed.extend([(a, b), (b, c), (c, a)]);
        turns.extend([(a, b, c), (b, c, a), (c, a, b)]);
    }
    directed.sort_unstable();
    if directed.windows(2).any(|w| w[0] == w[1]) {
        return false;
    }
    if !directed
        .iter()
        .all(|&(a, b)| directed.binary_search(&(b, a)).is_ok())
    {
        return false;
    }
    turns.sort_unstable();
    let mut start = 0;
    while start < turns.len() {
        let v = turns[start].0;
        let end = start + turns[start..].partition_point(|t| t.0 == v);
        let fan = &turns[start..end];
        // Follow the fan from its first triangle all the way round.
        let mut at = fan[0].1;
        let mut steps = 0;
        loop {
            let Ok(k) = fan.binary_search_by_key(&at, |t| t.1) else {
                return false;
            };
            at = fan[k].2;
            steps += 1;
            if at == fan[0].1 || steps > fan.len() {
                break;
            }
        }
        if steps != fan.len() {
            return false;
        }
        start = end;
    }
    true
}

/// The mesh for the clay, in the order [`Mesh::new`] gives it, with the
/// quads found again in that order.
fn with_quads(positions: Vec<Vec3>, triangles: Vec<[u32; 3]>, quads: &[[u32; 2]]) -> Mesh {
    let key = |p: Vec3| p.to_array().map(f32::to_bits);
    let before: Vec<[u32; 3]> = positions.iter().map(|&p| key(p)).collect();
    let old_triangles = triangles.clone();
    let mut mesh = Mesh::new(positions, triangles);
    let mut index = HashMap::with_capacity(mesh.positions.len());
    for (i, &p) in mesh.positions.iter().enumerate() {
        if index.insert(key(p), i as u32).is_some() {
            // Two vertices in the same place: no telling them apart.
            return mesh;
        }
    }
    let Some(remap) = before
        .iter()
        .map(|k| index.get(k).copied())
        .collect::<Option<Vec<u32>>>()
    else {
        return mesh;
    };
    // A triangle by its corners, starting from the lowest.
    let canonical = |t: [u32; 3]| {
        let k = (0..3).min_by_key(|&k| t[k]).unwrap_or(0);
        [t[k], t[(k + 1) % 3], t[(k + 2) % 3]]
    };
    let found: HashMap<[u32; 3], u32> = mesh
        .triangles
        .iter()
        .enumerate()
        .map(|(i, &t)| (canonical(t), i as u32))
        .collect();
    let new_index = |t: u32| {
        let t = old_triangles.get(t as usize)?.map(|v| remap[v as usize]);
        found.get(&canonical(t)).copied()
    };
    mesh.quads = quads
        .iter()
        .filter_map(|&[a, b]| Some([new_index(a)?, new_index(b)?]))
        .collect();
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meshio::is_closed;

    /// The share of vertices with four quad edges (not counting the
    /// diagonals that split the quads).
    fn regular_share(mesh: &Mesh) -> f32 {
        let mut valence: Vec<i32> = (0..mesh.positions.len() as u32)
            .map(|v| mesh.neighbours.of(v).len() as i32)
            .collect();
        for &[a, b] in &mesh.quads {
            let (ta, tb) = (mesh.triangles[a as usize], mesh.triangles[b as usize]);
            for v in ta.iter().filter(|v| tb.contains(v)) {
                valence[*v as usize] -= 1;
            }
        }
        valence.iter().filter(|&&k| k == 4).count() as f32 / valence.len() as f32
    }

    fn check(mesh: &Mesh) {
        assert!(is_closed(&mesh.triangles));
        assert!(is_closed_manifold(mesh.positions.len(), &mesh.triangles));
        let quad_triangles = mesh.quads.len() * 2;
        assert!(
            quad_triangles as f32 > mesh.triangles.len() as f32 * 0.85,
            "{} of {} triangles in quads",
            quad_triangles,
            mesh.triangles.len()
        );
    }

    #[test]
    fn a_sphere_comes_out_as_even_quads_on_the_sphere() {
        let mesh = remesh(&Mesh::icosphere(5), 2000).expect("remeshed");
        check(&mesh);
        let quads = mesh.quads.len();
        assert!((1500..2600).contains(&quads), "{quads} quads");
        let regular = regular_share(&mesh);
        assert!(regular > 0.8, "{regular} regular");
        for p in &mesh.positions {
            assert!((p.length() - 1.0).abs() < 0.01, "{p}");
        }
        // Even: edges within a factor of two or so of each other.
        let mut lengths: Vec<f32> = mesh
            .triangles
            .iter()
            .flat_map(|t| {
                let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
                [a.distance(b), b.distance(c), c.distance(a)]
            })
            .collect();
        lengths.sort_by(f32::total_cmp);
        let at = |q: f32| lengths[((lengths.len() - 1) as f32 * q) as usize];
        assert!(at(0.95) < at(0.05) * 2.5, "{} {}", at(0.05), at(0.95));
    }

    /// A unit sphere pushed out to a cube, corners and all.
    fn cube() -> Mesh {
        let sphere = Mesh::icosphere(4);
        let positions = sphere
            .positions
            .iter()
            .map(|p| *p / p.abs().max_element())
            .collect();
        Mesh::new(positions, sphere.triangles.clone())
    }

    fn lumpy() -> Mesh {
        let sphere = Mesh::icosphere(5);
        let positions = sphere
            .positions
            .iter()
            .map(|&p| p * (1.0 + 0.2 * (3.0 * p.x).sin() * (4.0 * p.y).sin() * (2.0 * p.z).cos()))
            .collect();
        Mesh::new(positions, sphere.triangles.clone())
    }

    fn torus() -> Mesh {
        let (rings, sides) = (96u32, 48u32);
        let mut positions = Vec::new();
        for i in 0..rings {
            let a = i as f32 / rings as f32 * std::f32::consts::TAU;
            for j in 0..sides {
                let b = j as f32 / sides as f32 * std::f32::consts::TAU;
                let r = 0.7 + 0.3 * b.cos();
                positions.push(Vec3::new(r * a.cos(), 0.3 * b.sin(), r * a.sin()));
            }
        }
        let at = |i: u32, j: u32| (i % rings) * sides + j % sides;
        let mut triangles = Vec::new();
        for i in 0..rings {
            for j in 0..sides {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                triangles.push([a, c, b]);
                triangles.push([a, d, c]);
            }
        }
        Mesh::new(positions, triangles)
    }

    #[test]
    fn a_cube_a_lumpy_shape_and_a_torus_stay_closed_and_manifold() {
        for (name, input, quads) in [
            ("cube", cube(), 1500),
            ("lumpy", lumpy(), 3000),
            ("torus", torus(), 2000),
        ] {
            let before = euler(input.positions.len(), &input.triangles);
            let mesh = remesh(&input, quads).unwrap_or_else(|| panic!("{name}"));
            check(&mesh);
            assert_eq!(
                euler(mesh.positions.len(), &mesh.triangles),
                before,
                "{name}"
            );
            // The same size and place.
            let size = |m: &Mesh| {
                m.positions
                    .iter()
                    .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), &p| {
                        (lo.min(p), hi.max(p))
                    })
            };
            let (a, b) = (size(&input), size(&mesh));
            assert!(
                a.0.distance(b.0) < 0.05 && a.1.distance(b.1) < 0.05,
                "{name}: {a:?} {b:?}"
            );
        }
    }

    #[test]
    fn garbage_gives_nothing_or_a_sound_mesh() {
        let nan = Vec3::new(f32::NAN, 0.0, 0.0);
        let flat = Mesh::new(vec![Vec3::ZERO; 3], vec![[0, 1, 2]]);
        let cases = [
            Mesh::new(Vec::new(), Vec::new()),
            Mesh::new(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]]),
            Mesh::new(vec![nan, Vec3::X, Vec3::Y], vec![[0, 1, 2]]),
            flat,
            Mesh::new(
                vec![Vec3::ZERO, Vec3::X * 1e30, Vec3::Y * 1e30],
                vec![[0, 1, 2]],
            ),
            // A soup of random triangles.
            {
                let mut h = 7u32;
                let mut next = move || {
                    h ^= h << 13;
                    h ^= h >> 17;
                    h ^= h << 5;
                    h as f32 / u32::MAX as f32 - 0.5
                };
                let positions: Vec<Vec3> = (0..300)
                    .map(|_| Vec3::new(next(), next(), next()))
                    .collect();
                let triangles = (0..100u32).map(|i| [i, i + 100, i + 200]).collect();
                Mesh::new(positions, triangles)
            },
        ];
        for (k, mesh) in cases.iter().enumerate() {
            for quads in [0, 1000, u32::MAX] {
                if let Some(out) = remesh(mesh, quads) {
                    assert!(
                        is_closed_manifold(out.positions.len(), &out.triangles),
                        "case {k}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_manifold_check_catches_a_pinched_vertex() {
        // Two tetrahedra sharing one corner: closed edges, but not a fan.
        let tet = [[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]];
        assert!(is_closed_manifold(4, &tet));
        let mut two: Vec<[u32; 3]> = tet.to_vec();
        two.extend(
            tet.iter()
                .map(|t| t.map(|v| if v == 0 { 0 } else { v + 3 })),
        );
        assert!(is_closed(&two));
        assert!(!is_closed_manifold(7, &two));
    }

    /// Timings for the browser's budget:
    /// `cargo test -p splatstudy-core --release quad::tests::timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing() {
        use std::time::Instant;
        let sphere = Mesh::icosphere(crate::sculpt::OLD_START_LEVEL);
        let positions = sphere
            .positions
            .iter()
            .map(|&p| p * (1.0 + 0.1 * (5.0 * p.x).sin() * (4.0 * p.y).sin() * (3.0 * p.z).cos()))
            .collect();
        let clay = Mesh::new(positions, sphere.triangles.clone());
        for quads in [MIN_QUADS, DEFAULT_QUADS, 10_000, MAX_QUADS] {
            let start = Instant::now();
            let mesh = remesh(&clay, quads).expect("remeshed");
            println!(
                "{quads} quads asked: {} quads, {} vertices, {:.0}% regular, {:.2} s",
                mesh.quads.len(),
                mesh.positions.len(),
                regular_share(&mesh) * 100.0,
                start.elapsed().as_secs_f64()
            );
        }
    }
}
