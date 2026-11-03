// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Signed distance to a triangle mesh on a grid.
//!
//! Only nodes near the surface (the band) get a true distance. Farther
//! nodes only need to know whether they are inside: a flood fill from the
//! grid's border finds the outside. It can't cross the nodes within a step
//! of the surface (the walls), which take the side of the triangle nearest
//! to them.

use glam::Vec3;

use super::grid::{self, Grid};

/// Half-width of the band of true distances, in grid steps.
const BAND: f32 = 1.5;

/// Nodes nearer than this many steps to the surface (the walls) measure
/// every triangle around them; the rest of the band, only the nearest
/// triangles of the walls next to them. Every grid edge that crosses the
/// surface has both ends nearer, so the walls keep the outside out.
const NEAR: f32 = 1.0;

/// The signed distance to the triangles (negative inside), true within
/// [`BAND`] steps of the surface and ±that beyond. `resolution` cells
/// span the longest side of the triangles' box. `close` > 0 seals holes
/// up to about twice that many cells wide (see [`grid::outside`]), but not
/// creases (see [`reopen_creases`]). `None` if there is nothing to measure.
///
/// Each triangle visits every node nearer to it than [`NEAR`], so each
/// of those finds its truly nearest triangle however the triangles are
/// cut or joined, and with it the side of the surface it is on.
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
        // Not quite a whole step: where faces lie on the grid, the edge
        // where two meet is often exactly a step from a node.
        near: (NEAR + 1e-4) * g.step,
        tiny: g.step * 1e-5,
        side: vec![0i8; g.len()],
        nearest: vec![u32::MAX; g.len()],
    };
    for (f, &t) in triangles.iter().enumerate() {
        if let Some(shape) = Shape::new(positions, t) {
            field.triangle(&mut g, f as u32, shape);
        }
    }
    // The nodes nearer than `NEAR` wall the outside off from the inside.
    let wall: Vec<bool> = field.side.iter().map(|&s| s != 0).collect();
    field.spread(&mut g, positions, triangles, &wall);
    let Field { band, side, .. } = field;

    let mut out = Vec::new();
    // An open mesh lets the outside in everywhere: seal it harder.
    for more in [0, 2, 4] {
        out = grid::outside(&g, &wall, close + more);
        if close + more > 0 {
            reopen_creases(&g, &wall, &side, &mut out);
        }
        if (0..g.len()).any(|i| !out[i] && !wall[i]) {
            break;
        }
    }
    // Band nodes outside their nearest triangle may still be inside the
    // shape: where pieces overlap, the hidden surfaces of one lie inside
    // another. Outside is only what the outside reaches.
    let seeds: Vec<usize> = (0..g.len())
        .filter(|&i| side[i] > 0 && !out[i] && g.neighbours(i).any(|j| out[j]))
        .collect();
    g.flood(&mut out, seeds, |j| side[j] > 0);
    for (i, v) in g.values.iter_mut().enumerate() {
        *v = match side[i] {
            _ if !wall[i] => v.min(band).copysign(if out[i] { 1.0 } else { -1.0 }),
            s if s > 0 && !out[i] => -band,
            s => v.copysign(f32::from(s)),
        };
    }
    Some(g)
}

/// Gives back to the outside the creases that sealing closed off.
///
/// Sealing can't tell a hole from a crease: it shuts both, and the
/// outside grows back only a few nodes into them. Behind a hole lies the
/// inside of the shape, whose walls face away from it; a crease is lined
/// with walls that face into it. So each piece of the nodes the outside
/// didn't reach, but that touches it with no wall between (or touches the
/// grid's border), goes back outside if most of the band nodes around it
/// are on the outer side of their nearest triangle.
fn reopen_creases(g: &Grid, wall: &[bool], side: &[i8], out: &mut [bool]) {
    let [nx, ny, nz] = g.dims;
    let (sy, sz) = (nx, nx * ny);
    let left = |at: usize, out: &[bool]| !wall[at] && !out[at];
    let mut seeds = Vec::new();
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let at = g.index(i, j, k);
                if !left(at, out) {
                    continue;
                }
                let border =
                    i == 0 || j == 0 || k == 0 || i + 1 == nx || j + 1 == ny || k + 1 == nz;
                if border || [1, sy, sz].iter().any(|&d| out[at - d] || out[at + d]) {
                    seeds.push(at);
                }
            }
        }
    }
    let mut seen = vec![false; g.len()];
    let mut piece = Vec::new();
    for start in seeds {
        if seen[start] {
            continue;
        }
        // The piece, and how its walls face.
        seen[start] = true;
        piece.clear();
        piece.push(start);
        let mut facing = 0i64;
        let mut next = 0;
        while let Some(&i) = piece.get(next) {
            next += 1;
            for j in g.neighbours(i) {
                if wall[j] {
                    facing += i64::from(side[j].signum());
                } else if !seen[j] && !out[j] {
                    seen[j] = true;
                    piece.push(j);
                }
            }
        }
        if facing > 0 {
            piece.iter().for_each(|&i| out[i] = true);
        }
    }
}

/// The band being measured: distances in the grid's values, and here the
/// sides. A wall has the side of its nearest triangle, as ±(1 + how
/// squarely it faces the node, 0..126); the rest of the band ±1, or 0
/// where unsure (see [`Field::spread`]); nodes beyond the band 0.
struct Field {
    band: f32,
    /// See [`NEAR`].
    near: f32,
    /// Distances closer than this are the same.
    tiny: f32,
    side: Vec<i8>,
    /// The index of each node's nearest triangle, while measuring.
    nearest: Vec<u32>,
}

/// One triangle: its corners and unit normal.
#[derive(Clone, Copy)]
struct Shape {
    corners: [Vec3; 3],
    unit: Vec3,
}

impl Shape {
    /// `None` if degenerate: its neighbours cover its spot.
    fn new(positions: &[Vec3], t: [u32; 3]) -> Option<Shape> {
        let corners = t.map(|v| positions[v as usize]);
        let [a, b, c] = corners;
        let unit = (b - a).cross(c - a).try_normalize()?;
        Some(Shape { corners, unit })
    }
}

/// Triangle `f` measured at a node: `d` away, and `above` its plane.
struct Hit {
    f: u32,
    d: f32,
    above: f32,
}

impl Hit {
    /// Takes the triangle as the node's nearest (distance `value`, `side`
    /// and `nearest`) if it is within `near` and nearer than the one the
    /// node has, or as near and facing it more squarely. Distances closer
    /// than `tiny` are the same.
    fn take(self, [near, tiny]: [f32; 2], value: &mut f32, side: &mut i8, nearest: &mut u32) {
        let Hit { f, d, above } = self;
        let best = *value;
        if d > near || d > best * (1.0 + 1e-5) + tiny {
            return;
        }
        // Ties happen where the nearest point is an edge or a corner
        // shared by several triangles: the one that faces the node most
        // squarely tells its side best.
        let facing = if d > 0.0 {
            (above.abs() / d).min(1.0)
        } else {
            1.0
        };
        let rank = 1 + (facing * 126.0) as i8;
        if d < best * (1.0 - 1e-5) || rank > side.abs() {
            *value = d.min(best);
            *side = if above < 0.0 { -rank } else { rank };
            *nearest = f;
        }
    }
}

impl Field {
    /// Measures triangle `f` at every node nearer to it than [`NEAR`].
    ///
    /// Each row of nodes along x visits only where the slab around the
    /// triangle's plane crosses it, within the ball around the triangle;
    /// and it measures only where neither the plane nor the triangle's
    /// disc in it is farther than the nearest triangle found so far.
    fn triangle(&mut self, g: &mut Grid, f: u32, Shape { corners, unit }: Shape) {
        let [a, b, c] = corners;
        let centre = (a + b + c) / 3.0;
        let radius = centre
            .distance_squared(a)
            .max(centre.distance_squared(b))
            .max(centre.distance_squared(c))
            .sqrt();
        let (near, tiny, step) = (self.near, self.tiny, g.step);
        let reach = radius + near;
        let (lo, hi) = node_box(g, centre - Vec3::splat(reach), centre + Vec3::splat(reach));
        // Nodes along a row per unit of height above the plane: huge or
        // infinite where the plane runs along the rows.
        let slope = 1.0 / (unit.x * step);
        for k in lo[2]..hi[2] {
            for j in lo[1]..hi[1] {
                let start = g.point(0, j, k);
                let across = start - centre;
                let yz = across.y * across.y + across.z * across.z;
                if yz >= reach * reach {
                    continue;
                }
                // The nodes of the row near the plane.
                let height = (start - a).dot(unit);
                let (first, last) = if slope.abs() < 1e6 {
                    let (x0, x1) = ((-near - height) * slope, (near - height) * slope);
                    let first = x0.min(x1).ceil().max(lo[0] as f32) as usize;
                    let last = x0.max(x1).ceil().clamp(0.0, hi[0] as f32) as usize;
                    (first, last)
                } else if height.abs() < near {
                    (lo[0], hi[0])
                } else {
                    continue;
                };
                if first >= last {
                    continue;
                }
                let row = g.index(0, j, k);
                let nodes = row + first..row + last;
                let values = &mut g.values[nodes.clone()];
                let sides = &mut self.side[nodes.clone()];
                let nearest = &mut self.nearest[nodes];
                for (n, ((value, side), nearest)) in
                    values.iter_mut().zip(sides).zip(nearest).enumerate()
                {
                    let most = (*value * (1.0 + 1e-5) + tiny).min(near);
                    let along = (first + n) as f32 * step;
                    let x = across.x + along;
                    let above = height + unit.x * along;
                    // The triangle is at least as far as its plane, and
                    // as the rim of its disc around the centre in that
                    // plane: most nodes already know a nearer one.
                    if above.abs() > most {
                        continue;
                    }
                    let flat = x * x + yz - above * above;
                    let rim = radius + (most * most - above * above).sqrt();
                    if flat > rim * rim {
                        continue;
                    }
                    let p = start + Vec3::X * along;
                    let d = p.distance(closest_point(p, a, b, c));
                    Hit { f, d, above }.take([near, tiny], value, side, nearest);
                }
            }
        }
    }

    /// The rest of the band, from [`NEAR`] on, takes the distance to the
    /// nearest of the nearest triangles of the `wall` nodes next to it. A
    /// step along the axis most towards the surface brings a node at
    /// least 0.58 steps nearer it (where it is flat), so every node of the
    /// band has a wall next to it.
    ///
    /// It also takes the side of the farthest of those walls. The distance
    /// changes by at most a step from node to node, so a node more than a
    /// step from the surface is on the side of every node next to it; but
    /// walls on the surface itself tell no side, and a node with only
    /// those next to it gets none.
    fn spread(&mut self, g: &mut Grid, positions: &[Vec3], triangles: &[[u32; 3]], wall: &[bool]) {
        const LISTED: u32 = u32::MAX - 1;
        let mut rest = Vec::new();
        for i in (0..g.len()).filter(|&i| wall[i]) {
            for j in g.neighbours(i) {
                if !wall[j] && self.nearest[j] != LISTED {
                    self.nearest[j] = LISTED;
                    rest.push(j);
                }
            }
        }
        let sure = self.tiny * 1e3;
        for j in rest {
            let [x, y, z] = g.coords(j);
            let p = g.point(x, y, z);
            let (mut nearest, mut farthest) = (f32::INFINITY, sure);
            for i in g.neighbours(j).filter(|&i| wall[i]) {
                let [a, b, c] = triangles[self.nearest[i] as usize].map(|v| positions[v as usize]);
                nearest = nearest.min(p.distance(closest_point(p, a, b, c)));
                if g.values[i] > farthest {
                    farthest = g.values[i];
                    self.side[j] = self.side[i].signum();
                }
            }
            if nearest < self.band {
                g.values[j] = nearest;
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
        // Triangles longer, then shorter than a step.
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
                            // inside the sphere. Beyond the walls, the
                            // nearest triangle of a wall next to a node
                            // may not quite be its own.
                            let wall = exact.abs() < NEAR * g.step * 0.95;
                            let off = if wall { 0.004 } else { 0.004 + 0.15 * g.step };
                            assert!((v - exact).abs() < off, "{level}: {v} vs {exact}");
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

    /// A block with deep, narrow grooves along its top, long along z: the
    /// creases of a fold, narrower than the holes that sealing closes. Some
    /// of its faces lie on the grid's nodes, where the distances to two
    /// faces tie.
    struct Comb {
        size: Vec3,
        /// The height of the grooves' floor.
        floor: f32,
        /// The grooves' ends along x.
        grooves: Vec<(f32, f32)>,
    }

    impl Comb {
        fn new(step: f32) -> Comb {
            // Teeth 4 steps wide between grooves 4.3 to 6.7 steps wide
            // and 10 deep: wider than the band, and deeper than the
            // outside grows back after sealing.
            let mut grooves = Vec::new();
            let mut x = 4.0 * step;
            for width in [4.3, 5.5, 6.7] {
                grooves.push((x, x + width * step));
                x += (width + 4.0) * step;
            }
            Comb {
                size: Vec3::new(x, 16.0 * step, 40.0 * step),
                floor: 6.0 * step,
                grooves,
            }
        }

        /// The exact signed distance, from boxes: the block is the base
        /// and the teeth; what is around it is the grooves and everything
        /// beyond the block's box.
        fn distance(&self, p: Vec3) -> f32 {
            let to_box = |min: Vec3, max: Vec3| (min - p).max(p - max).max(Vec3::ZERO).length();
            let Vec3 { x: w, y: h, z: l } = self.size;
            let mut teeth = vec![0.0];
            for &(x0, x1) in &self.grooves {
                teeth.extend([x0, x1]);
            }
            teeth.push(w);
            let outside = teeth
                .chunks(2)
                .map(|t| to_box(Vec3::new(t[0], 0.0, 0.0), Vec3::new(t[1], h, l)))
                .fold(to_box(Vec3::ZERO, Vec3::new(w, self.floor, l)), f32::min);
            if outside > 0.0 {
                return outside;
            }
            let walls = p.min(self.size - p).min_element();
            let inside = self
                .grooves
                .iter()
                .map(|&(x0, x1)| to_box(Vec3::new(x0, self.floor, 0.0), Vec3::new(x1, h, l)))
                .fold(walls, f32::min);
            -inside
        }

        /// Every face cut into its own strips of sliver triangles, about
        /// 0.35 by 2.8 steps, joined to nothing: the nearest corner to a
        /// point is seldom a corner of the nearest triangle.
        fn triangles(&self, step: f32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
            let mut positions = Vec::new();
            let mut triangles = Vec::new();
            // A rectangle facing u × v.
            let mut rectangle = |origin: Vec3, u: Vec3, v: Vec3| {
                let nu = (u.length() / (0.35 * step)).ceil() as u32;
                let nv = (v.length() / (2.8 * step)).ceil() as u32;
                let base = positions.len() as u32;
                for j in 0..=nv {
                    for i in 0..=nu {
                        let (s, t) = (i as f32 / nu as f32, j as f32 / nv as f32);
                        positions.push(origin + u * s + v * t);
                    }
                }
                let at = |i: u32, j: u32| base + i + j * (nu + 1);
                for j in 0..nv {
                    for i in 0..nu {
                        triangles.push([at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                        triangles.push([at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
                    }
                }
            };
            let Vec3 { x: w, y: h, z: l } = self.size;
            // The outline seen from +z, counter-clockwise.
            let mut outline = vec![(0.0, 0.0), (w, 0.0), (w, h)];
            for &(x0, x1) in self.grooves.iter().rev() {
                outline.extend([(x1, h), (x1, self.floor), (x0, self.floor), (x0, h)]);
            }
            outline.push((0.0, h));
            for (k, &(ax, ay)) in outline.iter().enumerate() {
                let (bx, by) = outline[(k + 1) % outline.len()];
                let a = Vec3::new(ax, ay, 0.0);
                rectangle(a, Vec3::new(bx - ax, by - ay, 0.0), Vec3::Z * l);
            }
            // Both ends: the base and the teeth.
            let mut parts = vec![(0.0, 0.0, w, self.floor)];
            let mut x = 0.0;
            for &(x0, x1) in &self.grooves {
                parts.push((x, self.floor, x0, h));
                x = x1;
            }
            parts.push((x, self.floor, w, h));
            for (x0, y0, x1, y1) in parts {
                let (u, v) = (Vec3::X * (x1 - x0), Vec3::Y * (y1 - y0));
                rectangle(Vec3::new(x0, y0, l), u, v);
                rectangle(Vec3::new(x0, y0, 0.0), v, u);
            }
            (positions, triangles)
        }
    }

    #[test]
    fn narrow_creases_and_slivers_get_the_exact_distance() {
        let step = 0.025;
        let comb = Comb::new(step);
        let (positions, triangles) = comb.triangles(step);
        assert!(triangles.len() > 10_000);
        for close in [0, super::super::REFERENCE_CLOSE] {
            let g = signed_distance(&positions, &triangles, 40, close).unwrap();
            assert!((g.step - step).abs() < 1e-6);
            let band = BAND * g.step;
            let (mut wrong_side, mut wrong_distance, mut checked) = (Vec::new(), 0, 0);
            for k in 0..g.dims[2] {
                for j in 0..g.dims[1] {
                    for i in 0..g.dims[0] {
                        let p = g.point(i, j, k);
                        let exact = comb.distance(p);
                        let v = g.values[g.index(i, j, k)];
                        if exact.abs() < 0.01 * step {
                            continue; // on the surface: either side
                        }
                        if (v < 0.0) != (exact < 0.0) {
                            wrong_side.push(p);
                        }
                        if exact.abs() < band * 0.9 {
                            checked += 1;
                            if (v - exact).abs() > 0.01 * step {
                                wrong_distance += 1;
                            }
                        }
                    }
                }
            }
            assert!(checked > 10_000);
            let some = &wrong_side[..wrong_side.len().min(4)];
            assert!(
                wrong_side.is_empty(),
                "close {close}: wrong side at {some:?}…"
            );
            assert_eq!(wrong_distance, 0, "close {close}");
        }
    }
}
