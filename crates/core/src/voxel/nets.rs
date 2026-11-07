// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Surface nets: a mesh along the zero level of a grid.
//!
//! Every cell the surface passes through gets one vertex, at the average
//! of the points where the surface crosses the cell's edges. Every grid
//! edge the surface crosses becomes a four-sided face joining the vertices
//! of the four cells around that edge. The result is closed, with evenly
//! sized faces, which is what sculpting wants; a few passes of smoothing
//! then take off the grid's stair steps.

use glam::Vec3;

use super::grid::Grid;

/// The nodes next to the surface: those with a neighbour on the other
/// side. Only around them is there anything to mesh.
fn crossings(grid: &Grid) -> Vec<usize> {
    let [nx, ny, nz] = grid.dims;
    let values = &grid.values;
    let mut near = vec![false; values.len()];
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let p = grid.index(i, j, k);
                let inside = values[p] < 0.0;
                for (more, q) in [
                    (i + 1 < nx, p + 1),
                    (j + 1 < ny, p + nx),
                    (k + 1 < nz, p + nx * ny),
                ] {
                    if more && (values[q] < 0.0) != inside {
                        near[p] = true;
                        near[q] = true;
                    }
                }
            }
        }
    }
    (0..values.len()).filter(|&p| near[p]).collect()
}

/// The mesh along the zero level of `grid` (negative inside), counter-
/// clockwise seen from outside. The grid's outer nodes must be outside.
pub fn surface_nets(grid: &Grid) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let [nx, ny, nz] = grid.dims;
    if nx < 2 || ny < 2 || nz < 2 {
        return (Vec::new(), Vec::new());
    }
    let values = &grid.values;
    let steps = [1, nx, nx * ny];
    let [sx, sy, sz] = steps;
    // The eight corners of a cell, numbered by their x, y, z bits.
    let corner_offsets = [0, sx, sy, sx + sy, sz, sx + sz, sy + sz, sx + sy + sz];
    let corner_at = |c: usize| Vec3::new((c & 1) as f32, (c >> 1 & 1) as f32, (c >> 2) as f32);
    // The twelve edges, as pairs of corners differing in one bit.
    let mut edges = [(0usize, 0usize); 12];
    let mut e = 0;
    for a in 0..8 {
        for bit in [1, 2, 4] {
            if a & bit == 0 {
                edges[e] = (a, a | bit);
                e += 1;
            }
        }
    }
    let near = crossings(grid);

    // A vertex for each cell the surface passes through; cells are named
    // by their lowest corner. Every such cell has a corner next to the
    // surface.
    const UNSEEN: u32 = u32::MAX;
    const EMPTY: u32 = u32::MAX - 1;
    let mut cell_vertex = vec![UNSEEN; values.len()];
    let mut positions = Vec::new();
    for &node in &near {
        let [i, j, k] = grid.coords(node);
        for d in 0..8 {
            let (dx, dy, dz) = (d & 1, d >> 1 & 1, d >> 2);
            if i < dx || j < dy || k < dz {
                continue;
            }
            let [ci, cj, ck] = [i - dx, j - dy, k - dz];
            if ci + 1 >= nx || cj + 1 >= ny || ck + 1 >= nz {
                continue;
            }
            let base = grid.index(ci, cj, ck);
            if cell_vertex[base] != UNSEEN {
                continue;
            }
            cell_vertex[base] = EMPTY;
            let v = corner_offsets.map(|offset| values[base + offset]);
            let mut sum = Vec3::ZERO;
            let mut count = 0.0;
            for &(a, b) in &edges {
                if (v[a] < 0.0) != (v[b] < 0.0) {
                    let t = v[a] / (v[a] - v[b]);
                    sum += corner_at(a).lerp(corner_at(b), t);
                    count += 1.0;
                }
            }
            if count > 0.0 {
                cell_vertex[base] = positions.len() as u32;
                positions.push(grid.point(ci, cj, ck) + sum / count * grid.step);
            }
        }
    }

    // A face for each grid edge that crosses the surface.
    let mut triangles = Vec::with_capacity(positions.len() * 2);
    for &p in &near {
        let at = grid.coords(p);
        let inside = values[p] < 0.0;
        for a in 0..3 {
            if at[a] + 1 >= grid.dims[a] || inside == (values[p + steps[a]] < 0.0) {
                continue;
            }
            // The two other axes, so that (a, b, c) turns like (x, y, z).
            let (b, c) = ((a + 1) % 3, (a + 2) % 3);
            if at[b] == 0 || at[c] == 0 {
                continue; // the border: never crossed
            }
            let quad = [p - steps[b] - steps[c], p - steps[c], p, p - steps[b]]
                .map(|cell| cell_vertex[cell]);
            if quad.iter().any(|&v| v >= EMPTY) {
                continue;
            }
            // Counter-clockwise about +a as listed: the outside is towards
            // +a when this node is inside.
            let [q0, q1, q2, q3] = if inside {
                quad
            } else {
                [quad[3], quad[2], quad[1], quad[0]]
            };
            // Split along the shorter diagonal.
            let pos = |v: u32| positions[v as usize];
            if pos(q0).distance_squared(pos(q2)) <= pos(q1).distance_squared(pos(q3)) {
                triangles.push([q0, q1, q2]);
                triangles.push([q0, q2, q3]);
            } else {
                triangles.push([q0, q1, q3]);
                triangles.push([q1, q2, q3]);
            }
        }
    }
    (positions, triangles)
}

/// Removes the checkerboards: grid faces whose diagonal corners are
/// inside and the other two outside. Surface nets would join four faces
/// along one edge there; nudging the outside corner nearest the surface
/// just inside joins the two inside corners, and every edge then joins
/// two faces. Likewise cells whose inside (or outside) corners make two
/// groups touching only at a corner, where the cell's vertex would pinch
/// two sheets together. Always inwards, so it ends.
pub fn untangle(grid: &mut Grid) {
    let steps = [1, grid.dims[0], grid.dims[0] * grid.dims[1]];
    let nudge = -grid.step * 1e-3;
    // A checkerboard's corners are all next to the surface, and so is at
    // least one corner of any cell the surface passes.
    let mut todo = Vec::new();
    for node in crossings(grid) {
        around(grid, node, &mut todo);
    }
    todo.sort_unstable();
    todo.dedup();
    while !todo.is_empty() {
        let mut next = Vec::new();
        for &p in &todo {
            let at = grid.coords(p);
            // The three faces with this node as their lowest corner.
            for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                if at[a] + 1 >= grid.dims[a] || at[b] + 1 >= grid.dims[b] {
                    continue;
                }
                let corners = [p, p + steps[a], p + steps[a] + steps[b], p + steps[b]];
                let v = corners.map(|c| grid.values[c]);
                let inside = v.map(|x| x < 0.0);
                if inside[0] != inside[2] || inside[1] != inside[3] || inside[0] == inside[1] {
                    continue;
                }
                let (c, _) = corners
                    .iter()
                    .zip(v)
                    .filter(|(_, x)| *x >= 0.0)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .expect("two corners outside");
                grid.values[*c] = nudge;
                around(grid, *c, &mut next);
            }
            // The cell with this node as its lowest corner: if its inside
            // corners (or its outside ones) make two groups that touch only
            // at a corner, its one vertex would pinch two sheets together.
            if (0..3).all(|a| at[a] + 1 < grid.dims[a]) {
                let corners: [usize; 8] = std::array::from_fn(|c| {
                    p + (c & 1) * steps[0] + (c >> 1 & 1) * steps[1] + (c >> 2) * steps[2]
                });
                let inside = corners
                    .iter()
                    .enumerate()
                    .fold(0u8, |m, (c, &n)| m | u8::from(grid.values[n] < 0.0) << c);
                // The outer nodes stay outside, so the mesh stays closed.
                let nearest = corners
                    .into_iter()
                    .filter(|&n| grid.values[n] >= 0.0 && !on_border(grid, n))
                    .min_by(|&a, &b| grid.values[a].total_cmp(&grid.values[b]));
                if let Some(c) = nearest
                    && (groups(inside) > 1 || groups(!inside) > 1)
                {
                    grid.values[c] = nudge;
                    around(grid, c, &mut next);
                }
            }
        }
        todo = next;
    }
}

/// The lowest corners of the faces and cells around `node`, to look at
/// again after it changed.
fn around(grid: &Grid, node: usize, next: &mut Vec<usize>) {
    let steps = [1, grid.dims[0], grid.dims[0] * grid.dims[1]];
    let at = grid.coords(node);
    for d in 0..8 {
        let back = [d & 1, d >> 1 & 1, d >> 2];
        if (0..3).all(|a| at[a] >= back[a]) {
            next.push(node - back[0] * steps[0] - back[1] * steps[1] - back[2] * steps[2]);
        }
    }
}

fn on_border(grid: &Grid, node: usize) -> bool {
    let at = grid.coords(node);
    (0..3).any(|a| at[a] == 0 || at[a] + 1 == grid.dims[a])
}

/// How many groups the corners of a cell set in `mask` make, joined along
/// the cell's edges.
fn groups(mask: u8) -> u32 {
    let mut left = mask;
    let mut count = 0;
    while left != 0 {
        count += 1;
        let mut group = left & left.wrapping_neg();
        loop {
            let mut grown = group;
            for c in 0..8 {
                if group >> c & 1 == 1 {
                    for bit in [1, 2, 4] {
                        grown |= 1 << (c ^ bit);
                    }
                }
            }
            grown &= mask;
            if grown == group {
                break;
            }
            group = grown;
        }
        left &= !group;
    }
    count
}

/// Moves each vertex halfway to the average of its neighbours, `passes`
/// times; with a `grid`, each pass then pulls the vertices back onto its
/// zero level, so the surface relaxes along itself without shrinking.
pub fn smooth(positions: &mut [Vec3], triangles: &[[u32; 3]], passes: usize, grid: Option<&Grid>) {
    let n = positions.len();
    let mut sum = vec![Vec3::ZERO; n];
    let mut count = vec![0u32; n];
    for _ in 0..passes {
        sum.iter_mut().for_each(|s| *s = Vec3::ZERO);
        count.iter_mut().for_each(|c| *c = 0);
        // Each edge in each direction once, on a closed mesh: every
        // neighbour counts once.
        for &[a, b, c] in triangles {
            for (from, to) in [(a, b), (b, c), (c, a)] {
                sum[from as usize] += positions[to as usize];
                count[from as usize] += 1;
            }
        }
        for v in 0..n {
            if count[v] > 0 {
                let average = sum[v] / count[v] as f32;
                positions[v] = positions[v].lerp(average, 0.5);
            }
        }
        if let Some(grid) = grid {
            for p in positions.iter_mut() {
                let (value, gradient) = grid.sample(*p);
                let g2 = gradient.length_squared();
                if g2 > 1e-12 {
                    let step = gradient * (value / g2);
                    *p -= step.clamp_length_max(grid.step);
                }
            }
        }
    }
}

/// Keeps only the largest connected piece (by triangles), and the vertices
/// it uses, renumbered in their order.
pub fn keep_largest(positions: &[Vec3], triangles: &[[u32; 3]]) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let n = positions.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn root(parent: &mut [u32], mut v: u32) -> u32 {
        while parent[v as usize] != v {
            let up = parent[parent[v as usize] as usize];
            parent[v as usize] = up;
            v = up;
        }
        v
    }
    for &[a, b, c] in triangles {
        for (x, y) in [(a, b), (b, c)] {
            let (rx, ry) = (root(&mut parent, x), root(&mut parent, y));
            if rx != ry {
                parent[rx.max(ry) as usize] = rx.min(ry);
            }
        }
    }
    let mut size = vec![0u32; n];
    for t in triangles {
        size[root(&mut parent, t[0]) as usize] += 1;
    }
    let Some(best) = (0..n).max_by_key(|&v| (size[v], std::cmp::Reverse(v))) else {
        return (Vec::new(), Vec::new());
    };
    let mut remap = vec![u32::MAX; n];
    let mut kept = Vec::new();
    for v in 0..n {
        if root(&mut parent, v as u32) as usize == best && size[best] > 0 {
            remap[v] = kept.len() as u32;
            kept.push(positions[v]);
        }
    }
    let kept_triangles = triangles
        .iter()
        .filter(|t| remap[t[0] as usize] != u32::MAX)
        .map(|t| t.map(|v| remap[v as usize]))
        .collect();
    (kept, kept_triangles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meshio::is_closed;

    /// A ball of `radius` as an exact distance field.
    fn ball(radius: f32, resolution: u32) -> Grid {
        let mut g = Grid::around(
            Vec3::splat(-radius),
            Vec3::splat(radius),
            resolution,
            2,
            0.0,
        )
        .unwrap();
        for k in 0..g.dims[2] {
            for j in 0..g.dims[1] {
                for i in 0..g.dims[0] {
                    let at = g.index(i, j, k);
                    g.values[at] = g.point(i, j, k).length() - radius;
                }
            }
        }
        g
    }

    #[test]
    fn a_ball_becomes_a_closed_outward_sphere() {
        let g = ball(1.0, 24);
        let (mut positions, triangles) = surface_nets(&g);
        assert!(is_closed(&triangles));
        for p in &positions {
            assert!((p.length() - 1.0).abs() < g.step * 0.5, "{p}");
        }
        for t in &triangles {
            let [a, b, c] = t.map(|v| positions[v as usize]);
            assert!((b - a).cross(c - a).dot(a + b + c) > 0.0, "faces inward");
        }
        smooth(&mut positions, &triangles, 2, Some(&g));
        for p in &positions {
            assert!((p.length() - 1.0).abs() < g.step * 0.1, "{p}");
        }
    }

    /// Whether the triangles around every vertex make a single fan.
    fn no_pinches(triangles: &[[u32; 3]]) -> bool {
        let mut fans: std::collections::HashMap<u32, Vec<(u32, u32)>> = Default::default();
        for &[a, b, c] in triangles {
            for (x, y, z) in [(a, b, c), (b, c, a), (c, a, b)] {
                fans.entry(x).or_default().push((y, z));
            }
        }
        fans.values().all(|fan| {
            let (mut at, mut steps) = (fan[0].0, 0);
            loop {
                let Some(&(_, to)) = fan.iter().find(|(from, _)| *from == at) else {
                    return false;
                };
                at = to;
                steps += 1;
                if at == fan[0].0 {
                    return steps == fan.len();
                }
                if steps > fan.len() {
                    return false;
                }
            }
        })
    }

    #[test]
    fn corners_touching_across_a_cell_are_joined_not_pinched() {
        // Two nodes inside, at opposite corners of one cell; and, the
        // other way round, a block with two opposite corners of a cell
        // outside it.
        for flip in [1.0, -1.0] {
            let mut g = Grid::around(Vec3::ZERO, Vec3::splat(7.0), 7, 0, flip).unwrap();
            let (a, b) = (g.index(3, 3, 3), g.index(4, 4, 4));
            if flip > 0.0 {
                g.values[a] = -1.0;
                g.values[b] = -1.0;
            } else {
                for k in 2..6 {
                    for j in 2..6 {
                        for i in 2..6 {
                            let at = g.index(i, j, k);
                            g.values[at] = -1.0;
                        }
                    }
                }
                g.values[a] = 1.0;
                g.values[b] = 1.0;
                for at in g.border() {
                    g.values[at] = 1.0;
                }
            }
            untangle(&mut g);
            let (_, triangles) = surface_nets(&g);
            assert!(is_closed(&triangles), "{flip}");
            assert!(no_pinches(&triangles), "{flip}");
        }
    }

    #[test]
    fn keep_largest_drops_the_small_piece() {
        let big = ball(1.0, 16);
        let (mut positions, mut triangles) = surface_nets(&big);
        let count = (positions.len(), triangles.len());
        let small = ball(0.3, 4);
        let (p2, t2) = surface_nets(&small);
        let offset = positions.len() as u32;
        positions.extend(p2.iter().map(|p| *p + Vec3::X * 3.0));
        triangles.extend(t2.iter().map(|t| t.map(|v| v + offset)));
        let (kept, kept_triangles) = keep_largest(&positions, &triangles);
        assert_eq!((kept.len(), kept_triangles.len()), count);
        assert!(is_closed(&kept_triangles));
    }
}
