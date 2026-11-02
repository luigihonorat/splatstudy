// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Signed distance to a triangle mesh on a grid.
//!
//! Only nodes near the surface (the band) get a true distance. Farther
//! nodes only need to know whether they are inside: a flood fill from the
//! grid's border, which can't cross the band, finds the outside. Band
//! nodes take the side of the triangle nearest to them.

use glam::Vec3;

use super::grid::{self, Grid};

/// Half-width of the band of true distances, in grid steps. Every grid
/// edge that crosses the surface has both ends inside it.
const BAND: f32 = 1.5;

/// Triangles shorter than this many grid steps on every side are found
/// through their corners (see [`signed_distance`]).
const SMALL: f32 = 3.0;

/// The signed distance to the triangles (negative inside), true within
/// [`BAND`] steps of the surface and ±that beyond. `resolution` cells
/// span the longest side of the triangles' box. `close` > 0 seals holes
/// up to about twice that many cells wide (see [`grid::outside`]).
/// `None` if there is nothing to measure.
///
/// Large triangles visit every node around them. The clay's triangles
/// are much smaller than a cell, and hundreds would visit each node, so
/// instead each node near them finds its nearest corner first, then
/// measures only the triangles around that corner.
pub fn signed_distance(
    positions: &[Vec3],
    triangles: &[[u32; 3]],
    resolution: u32,
    close: usize,
) -> Option<Grid> {
    let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
    for t in triangles {
        for &v in t {
            let p = *positions.get(v as usize)?;
            min = min.min(p);
            max = max.max(p);
        }
    }
    let mut g = Grid::around(min, max, resolution, 2 + close, f32::INFINITY)?;
    let mut field = Field {
        band: BAND * g.step,
        tiny: g.step * g.step * 1e-10,
        side: vec![0i8; g.len()],
    };
    let small = SMALL * g.step;
    let mut seeds = vec![false; positions.len()];
    let mut large = Vec::new();
    // The longest side of the small triangles.
    let mut longest_small: f32 = 0.0;
    for t in triangles {
        let [a, b, c] = t.map(|v| positions[v as usize]);
        let longest = a
            .distance_squared(b)
            .max(b.distance_squared(c))
            .max(c.distance_squared(a));
        if longest < small * small {
            t.iter().for_each(|&v| seeds[v as usize] = true);
            longest_small = longest_small.max(longest);
        } else {
            large.push(*t);
        }
    }
    // Corners first: they use the values as scratch space.
    if seeds.contains(&true) {
        field.around_corners(&mut g, positions, triangles, &seeds, longest_small.sqrt());
    }
    for t in large {
        field.triangle(&mut g, positions, t);
    }
    let Field { band, side, .. } = field;

    let wall: Vec<bool> = side.iter().map(|&s| s != 0).collect();
    // An open mesh lets the outside in everywhere: seal it harder.
    let mut out = grid::outside(&g, &wall, close);
    for more in [2, 4] {
        if (0..g.len()).any(|i| !out[i] && !wall[i]) {
            break;
        }
        out = grid::outside(&g, &wall, close + more);
    }
    // Band nodes outside their nearest triangle may still be inside the
    // shape: where pieces overlap, the hidden surfaces of one lie inside
    // another. Outside is only what the outside reaches.
    let seeds: Vec<usize> = (0..g.len())
        .filter(|&i| side[i] > 0 && g.neighbours(i).any(|j| out[j] && side[j] == 0))
        .collect();
    g.flood(&mut out, seeds, |j| side[j] > 0);
    for (i, v) in g.values.iter_mut().enumerate() {
        *v = match side[i] {
            0 if out[i] => band,
            0 => -band,
            s if s > 0 && !out[i] => -band,
            s => v.sqrt().copysign(f32::from(s)),
        };
    }
    Some(g)
}

/// The band being measured: squared distances in the grid's values, and
/// here the side of the nearest triangle, as ±(1 + how squarely it faces
/// the node, 0..126); 0 outside the band.
struct Field {
    band: f32,
    tiny: f32,
    side: Vec<i8>,
}

impl Field {
    /// Offers triangle `abc` (with unit normal `unit`) as the nearest to
    /// the node at index `at` and position `p`.
    fn offer(&mut self, g: &mut Grid, at: usize, p: Vec3, [a, b, c]: [Vec3; 3], unit: Vec3) {
        let height = (p - a).dot(unit);
        let best = g.values[at];
        // The triangle is at least as far as its plane.
        if height.abs() >= self.band || height * height > best * (1.0 + 1e-5) + self.tiny {
            return;
        }
        let d2 = p.distance_squared(closest_point(p, a, b, c));
        if d2 >= self.band * self.band {
            return;
        }
        // Ties happen where the nearest point is an edge or a corner
        // shared by several triangles: the one that faces the node most
        // squarely tells its side best.
        let nearer = d2 < best * (1.0 - 1e-5);
        let facing = if d2 > 0.0 {
            (height.abs() / d2.sqrt()).min(1.0)
        } else {
            1.0
        };
        let rank = 1 + (facing * 126.0) as i8;
        let tie = !nearer && d2 <= best * (1.0 + 1e-5) + self.tiny;
        if nearer || tie && rank > self.side[at].abs() {
            g.values[at] = d2.min(best);
            self.side[at] = if height < 0.0 { -rank } else { rank };
        }
    }

    /// Measures one triangle at every node within the band around it.
    fn triangle(&mut self, g: &mut Grid, positions: &[Vec3], t: [u32; 3]) {
        let corners = t.map(|v| positions[v as usize]);
        let [a, b, c] = corners;
        let Some(unit) = (b - a).cross(c - a).try_normalize() else {
            return; // degenerate: its neighbours cover its spot
        };
        let reach = Vec3::splat(self.band);
        let (lo, hi) = node_box(g, a.min(b).min(c) - reach, a.max(b).max(c) + reach);
        for k in lo[2]..hi[2] {
            for j in lo[1]..hi[1] {
                for i in lo[0]..hi[0] {
                    self.offer(g, g.index(i, j, k), g.point(i, j, k), corners, unit);
                }
            }
        }
    }

    /// Each node within reach of a `seeds` corner finds the nearest one,
    /// then measures the triangles around it.
    fn around_corners(
        &mut self,
        g: &mut Grid,
        positions: &[Vec3],
        triangles: &[[u32; 3]],
        seeds: &[bool],
        longest: f32,
    ) {
        // Every point of a triangle is within 0.58 of its longest side of
        // one of its corners, so every node within a step of the surface
        // (both ends of every edge crossing it) is found. Nodes farther
        // than that may be missed: they only lose some precision.
        let reach = self.band.max(self.band / BAND + longest * 0.6);
        // The distance to the nearest corner goes in the grid's values
        // until the triangles are measured.
        let mut nearest = vec![u32::MAX; g.len()];
        for (v, _) in seeds.iter().enumerate().filter(|(_, s)| **s) {
            let q = positions[v];
            let (lo, hi) = node_box(g, q - Vec3::splat(reach), q + Vec3::splat(reach));
            for k in lo[2]..hi[2] {
                for j in lo[1]..hi[1] {
                    let row = g.index(0, j, k);
                    let yz = Vec3::new(0.0, j as f32, k as f32) * g.step + g.origin - q;
                    for i in lo[0]..hi[0] {
                        let d = yz + Vec3::new(i as f32 * g.step, 0.0, 0.0);
                        let d2 = d.length_squared();
                        let at = row + i;
                        if d2 < reach * reach && d2 < g.values[at] {
                            g.values[at] = d2;
                            nearest[at] = v as u32;
                        }
                    }
                }
            }
        }
        // The triangles around each corner.
        let mut offsets = vec![0u32; positions.len() + 1];
        for t in triangles {
            t.iter().for_each(|&v| offsets[v as usize + 1] += 1);
        }
        for v in 0..positions.len() {
            offsets[v + 1] += offsets[v];
        }
        let mut fill = offsets.clone();
        let mut around = vec![0u32; offsets[positions.len()] as usize];
        for (f, t) in triangles.iter().enumerate() {
            for &v in t {
                around[fill[v as usize] as usize] = f as u32;
                fill[v as usize] += 1;
            }
        }
        // Each triangle's corners and unit normal together, read at once.
        let shapes: Vec<([Vec3; 3], Option<Vec3>)> = triangles
            .iter()
            .map(|t| {
                let corners = t.map(|v| positions[v as usize]);
                let [a, b, c] = corners;
                (corners, (b - a).cross(c - a).try_normalize())
            })
            .collect();
        for k in 0..g.dims[2] {
            for j in 0..g.dims[1] {
                let row = g.index(0, j, k);
                for i in 0..g.dims[0] {
                    let at = row + i;
                    let v = nearest[at];
                    if v == u32::MAX {
                        continue;
                    }
                    g.values[at] = f32::INFINITY;
                    let p = g.point(i, j, k);
                    let ring = offsets[v as usize] as usize..offsets[v as usize + 1] as usize;
                    for &f in &around[ring] {
                        if let (corners, Some(unit)) = shapes[f as usize] {
                            self.offer(g, at, p, corners, unit);
                        }
                    }
                }
            }
        }
    }
}

/// The nodes inside a box, as index ranges `lo..hi` along each axis.
fn node_box(g: &Grid, min: Vec3, max: Vec3) -> ([usize; 3], [usize; 3]) {
    let inv = 1.0 / g.step;
    let lo = ((min - g.origin) * inv).ceil().max(Vec3::ZERO);
    let hi = ((max - g.origin) * inv).floor() + Vec3::ONE;
    let hi = hi.max(Vec3::ZERO);
    let lo = [lo.x as usize, lo.y as usize, lo.z as usize];
    let hi = [
        (hi.x as usize).min(g.dims[0]),
        (hi.y as usize).min(g.dims[1]),
        (hi.z as usize).min(g.dims[2]),
    ];
    (lo, hi)
}

/// The point of triangle `abc` nearest to `p`: by which of its seven
/// regions (inside, three edges, three corners) `p` projects into.
pub fn closest_point(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sculpt::Mesh;

    #[test]
    fn closest_point_matches_a_brute_force_search() {
        let (a, b, c) = (
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.2, 0.0),
            Vec3::new(0.3, 0.9, 0.1),
        );
        // Dense samples of the triangle.
        let mut samples = Vec::new();
        for i in 0..=100 {
            for j in 0..=(100 - i) {
                let (u, v) = (i as f32 / 100.0, j as f32 / 100.0);
                samples.push(a + (b - a) * u + (c - a) * v);
            }
        }
        for k in 0..200 {
            let t = k as f32;
            let p = Vec3::new(
                (t * 0.37).sin() * 2.0,
                (t * 0.71).cos() * 2.0,
                (t * 0.13).sin(),
            );
            let q = closest_point(p, a, b, c);
            let brute = samples
                .iter()
                .map(|s| s.distance(p))
                .fold(f32::INFINITY, f32::min);
            assert!(q.distance(p) <= brute + 1e-5, "{p}");
            assert!(q.distance(p) >= brute - 0.02, "{p}");
        }
    }

    #[test]
    fn distance_to_a_sphere_is_signed_and_close_to_exact() {
        // Large triangles (visited one by one), then small ones (found
        // through their corners).
        for level in [4, 6] {
            let mesh = Mesh::icosphere(level);
            let g = signed_distance(&mesh.positions, &mesh.triangles, 32, 0).unwrap();
            let band = BAND * g.step;
            let mut checked = 0;
            for k in 0..g.dims[2] {
                for j in 0..g.dims[1] {
                    for i in 0..g.dims[0] {
                        let exact = g.point(i, j, k).length() - 1.0;
                        let v = g.values[g.index(i, j, k)];
                        if exact.abs() < band * 0.9 {
                            // The icosphere's flat facets sit up to ~0.002
                            // inside the sphere.
                            assert!((v - exact).abs() < 0.004, "{level}: {v} vs {exact}");
                            checked += 1;
                        } else if exact.abs() > band * 1.1 {
                            assert_eq!(v, band.copysign(exact));
                        }
                    }
                }
            }
            assert!(checked > 1000);
        }
    }
}
