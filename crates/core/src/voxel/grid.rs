// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! A regular grid of values over a box, and the flood fills that tell the
//! inside of a shape from the outside.

use glam::{IVec3, Vec3};

/// Values at the nodes of a regular grid. Shapes are negative inside.
pub struct Grid {
    /// Where node (0, 0, 0) sits.
    pub origin: Vec3,
    /// Distance between neighbouring nodes.
    pub step: f32,
    /// Nodes along x, y and z.
    pub dims: [usize; 3],
    /// Node (i, j, k) is at `i + dims[0] * (j + dims[1] * k)`.
    pub values: Vec<f32>,
}

impl Grid {
    /// A grid over the box from `min` to `max` with `resolution` cells
    /// along its longest side, plus `pad` nodes all around. `None` for an
    /// empty or flat box.
    pub fn around(min: Vec3, max: Vec3, resolution: u32, pad: usize, fill: f32) -> Option<Grid> {
        let size = max - min;
        let longest = size.max_element();
        if !(longest.is_finite() && longest > 0.0) || resolution == 0 {
            return None;
        }
        let step = longest / resolution as f32;
        let dims = size
            .to_array()
            .map(|s| (s / step).ceil() as usize + 1 + 2 * pad);
        Some(Grid {
            origin: min - Vec3::splat(step * pad as f32),
            step,
            dims,
            values: vec![fill; dims[0] * dims[1] * dims[2]],
        })
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn index(&self, i: usize, j: usize, k: usize) -> usize {
        i + self.dims[0] * (j + self.dims[1] * k)
    }

    /// The node at an index.
    pub fn coords(&self, index: usize) -> [usize; 3] {
        let [nx, ny, _] = self.dims;
        [index % nx, (index / nx) % ny, index / (nx * ny)]
    }

    pub fn point(&self, i: usize, j: usize, k: usize) -> Vec3 {
        self.origin + Vec3::new(i as f32, j as f32, k as f32) * self.step
    }

    /// Index steps to the six neighbours, and whether each exists.
    pub fn neighbours(&self, index: usize) -> impl Iterator<Item = usize> + use<> {
        let [i, j, k] = self.coords(index);
        let [nx, ny, nz] = self.dims;
        let (sx, sy) = (1, nx);
        let sz = nx * ny;
        [
            (i > 0, index.wrapping_sub(sx)),
            (i + 1 < nx, index + sx),
            (j > 0, index.wrapping_sub(sy)),
            (j + 1 < ny, index + sy),
            (k > 0, index.wrapping_sub(sz)),
            (k + 1 < nz, index + sz),
        ]
        .into_iter()
        .filter_map(|(ok, n)| ok.then_some(n))
    }

    /// The nodes on the outer faces of the grid.
    pub fn border(&self) -> Vec<usize> {
        let [nx, ny, nz] = self.dims;
        let mut nodes = Vec::new();
        for k in 0..nz {
            for j in 0..ny {
                let face = k == 0 || k + 1 == nz || j == 0 || j + 1 == ny;
                for i in 0..nx {
                    if face || i == 0 || i + 1 == nx {
                        nodes.push(self.index(i, j, k));
                    }
                }
            }
        }
        nodes
    }

    /// Marks every node reachable from `seeds` through nodes where `open`
    /// holds (the seeds included), six neighbours at a time. Returns how
    /// many nodes it marked.
    ///
    /// It fills whole runs along x at once and then looks for open runs in
    /// the four rows beside them, so it walks memory in order.
    pub fn flood(
        &self,
        mark: &mut [bool],
        seeds: impl IntoIterator<Item = usize>,
        open: impl Fn(usize) -> bool,
    ) -> usize {
        let [nx, ny, nz] = self.dims;
        let fresh = |mark: &[bool], at: usize| !mark[at] && open(at);
        let mut stack: Vec<[usize; 3]> = seeds.into_iter().map(|i| self.coords(i)).collect();
        let mut count = 0;
        while let Some([i, j, k]) = stack.pop() {
            let row = self.index(0, j, k);
            if !fresh(mark, row + i) {
                continue;
            }
            let (mut lo, mut hi) = (i, i);
            while lo > 0 && fresh(mark, row + lo - 1) {
                lo -= 1;
            }
            while hi + 1 < nx && fresh(mark, row + hi + 1) {
                hi += 1;
            }
            mark[row + lo..=row + hi].fill(true);
            count += hi - lo + 1;
            let beside = [
                (j > 0, j.wrapping_sub(1), k),
                (j + 1 < ny, j + 1, k),
                (k > 0, j, k.wrapping_sub(1)),
                (k + 1 < nz, j, k + 1),
            ];
            for (exists, j, k) in beside {
                if !exists {
                    continue;
                }
                let row = self.index(0, j, k);
                let mut x = lo;
                while x <= hi {
                    if fresh(mark, row + x) {
                        stack.push([x, j, k]);
                        while x <= hi && fresh(mark, row + x) {
                            x += 1;
                        }
                    } else {
                        x += 1;
                    }
                }
            }
        }
        count
    }

    /// Blurs the values along x, y and z with the weights 1 2 1, which
    /// smooths the shape their zero level makes by about half a node per
    /// pass. The outer nodes keep their values.
    pub fn blur(&mut self) {
        let steps = [1, self.dims[0], self.dims[0] * self.dims[1]];
        let mut line = Vec::new();
        for axis in 0..3 {
            let n = self.dims[axis];
            let stride = steps[axis];
            let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
            for u in 0..self.dims[a] {
                for v in 0..self.dims[b] {
                    let start = u * steps[a] + v * steps[b];
                    line.clear();
                    line.extend((0..n).map(|t| self.values[start + t * stride]));
                    for t in 1..n.saturating_sub(1) {
                        self.values[start + t * stride] =
                            0.25 * line[t - 1] + 0.5 * line[t] + 0.25 * line[t + 1];
                    }
                }
            }
        }
    }

    /// The value and its gradient at any point, interpolated between the
    /// eight nodes around it (clamped to the grid).
    pub fn sample(&self, p: Vec3) -> (f32, Vec3) {
        let max = IVec3::new(
            self.dims[0] as i32 - 2,
            self.dims[1] as i32 - 2,
            self.dims[2] as i32 - 2,
        );
        let g = (p - self.origin) / self.step;
        let cell = g
            .floor()
            .as_ivec3()
            .clamp(IVec3::ZERO, max.max(IVec3::ZERO));
        let f = (g - cell.as_vec3()).clamp(Vec3::ZERO, Vec3::ONE);
        let c = cell.as_uvec3();
        let v = |dx: usize, dy: usize, dz: usize| {
            self.values[self.index(c.x as usize + dx, c.y as usize + dy, c.z as usize + dz)]
        };
        let (v000, v100, v010, v110) = (v(0, 0, 0), v(1, 0, 0), v(0, 1, 0), v(1, 1, 0));
        let (v001, v101, v011, v111) = (v(0, 0, 1), v(1, 0, 1), v(0, 1, 1), v(1, 1, 1));
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        // Along x first, then y, then z.
        let x00 = lerp(v000, v100, f.x);
        let x10 = lerp(v010, v110, f.x);
        let x01 = lerp(v001, v101, f.x);
        let x11 = lerp(v011, v111, f.x);
        let y0 = lerp(x00, x10, f.y);
        let y1 = lerp(x01, x11, f.y);
        let value = lerp(y0, y1, f.z);
        let dx = lerp(
            lerp(v100 - v000, v110 - v010, f.y),
            lerp(v101 - v001, v111 - v011, f.y),
            f.z,
        );
        let dy = lerp(x10 - x00, x11 - x01, f.z);
        let dz = y1 - y0;
        (value, Vec3::new(dx, dy, dz) / self.step)
    }
}

/// Which nodes are outside a shape whose surface runs through the `wall`
/// nodes: those reached from the grid's border without crossing a wall.
///
/// With `close` > 0, the walls are first thickened by that many nodes, so
/// gaps up to about twice as wide are sealed (a scan's holes, the gaps
/// between splats); the outside then grows back by as many nodes, but not
/// through the walls, so the shape keeps its size.
pub fn outside(grid: &Grid, wall: &[bool], close: usize) -> Vec<bool> {
    let n = grid.len();
    let mut blocked = wall.to_vec();
    // The nodes the thickening blocks.
    let mut thick = Vec::new();
    let mut front: Vec<usize> = (0..n).filter(|&i| wall[i]).collect();
    for _ in 0..close {
        let start = thick.len();
        for &i in &front {
            for j in grid.neighbours(i) {
                if !blocked[j] {
                    blocked[j] = true;
                    thick.push(j);
                }
            }
        }
        front = thick[start..].to_vec();
    }
    let mut out = vec![false; n];
    let seeds: Vec<usize> = grid.border().into_iter().filter(|&i| !blocked[i]).collect();
    grid.flood(&mut out, seeds, |j| !blocked[j]);
    if close > 0 {
        // Only blocked nodes can be next to the outside without being in
        // it: the first step grows into those.
        let mut front: Vec<usize> = thick
            .into_iter()
            .filter(|&j| !out[j] && grid.neighbours(j).any(|i| out[i]))
            .collect();
        front.iter().for_each(|&j| out[j] = true);
        for _ in 1..close {
            let mut next = Vec::new();
            for &i in &front {
                for j in grid.neighbours(i) {
                    if !wall[j] && !out[j] {
                        out[j] = true;
                        next.push(j);
                    }
                }
            }
            front = next;
        }
    }
    out
}

/// Keeps only the largest 6-connected group of `solid` nodes.
pub fn keep_largest(grid: &Grid, solid: &mut [bool]) {
    let n = grid.len();
    let mut seen = vec![false; n];
    let mut best: Option<(usize, usize)> = None;
    for start in 0..n {
        if !solid[start] || seen[start] {
            continue;
        }
        let size = grid.flood(&mut seen, [start], |j| solid[j]);
        if best.is_none_or(|(s, _)| size > s) {
            best = Some((size, start));
        }
    }
    let Some((_, start)) = best else {
        return;
    };
    let mut keep = vec![false; n];
    grid.flood(&mut keep, [start], |j| solid[j]);
    solid.copy_from_slice(&keep);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(resolution: u32) -> Grid {
        Grid::around(Vec3::splat(-1.0), Vec3::splat(1.0), resolution, 2, 0.0).unwrap()
    }

    #[test]
    fn sample_interpolates_a_linear_field_exactly() {
        let mut g = cube(8);
        for k in 0..g.dims[2] {
            for j in 0..g.dims[1] {
                for i in 0..g.dims[0] {
                    let p = g.point(i, j, k);
                    let at = g.index(i, j, k);
                    g.values[at] = 2.0 * p.x - p.y + 0.5 * p.z + 1.0;
                }
            }
        }
        let p = Vec3::new(0.13, -0.42, 0.77);
        let (v, grad) = g.sample(p);
        assert!((v - (2.0 * p.x - p.y + 0.5 * p.z + 1.0)).abs() < 1e-5);
        assert!(grad.distance(Vec3::new(2.0, -1.0, 0.5)) < 1e-4, "{grad}");
    }

    #[test]
    fn a_closed_wall_keeps_its_inside_and_closing_seals_a_hole() {
        let g = cube(16);
        let centre = Vec3::ZERO;
        // A spherical shell one node thick, with a hole on top.
        let shell = |hole: f32| -> Vec<bool> {
            (0..g.len())
                .map(|i| {
                    let [x, y, z] = g.coords(i);
                    let p = g.point(x, y, z);
                    (p.distance(centre) - 0.7).abs() < g.step * 0.6 && p.y < 0.7 - hole
                })
                .collect()
        };
        let inside =
            |out: &[bool], wall: &[bool]| (0..g.len()).filter(|&i| !out[i] && !wall[i]).count();
        let closed = shell(0.0);
        let out = outside(&g, &closed, 0);
        assert!(inside(&out, &closed) > 100);
        assert!(!out[g.index(g.dims[0] / 2, g.dims[1] / 2, g.dims[2] / 2)]);

        let open = shell(0.1);
        assert_eq!(inside(&outside(&g, &open, 0), &open), 0, "leaks");
        let sealed = outside(&g, &open, 3);
        assert!(inside(&sealed, &open) > 80);
    }

    #[test]
    fn keep_largest_drops_the_floaters() {
        let g = cube(10);
        let mut solid = vec![false; g.len()];
        for i in 3..9 {
            for j in 3..9 {
                for k in 3..9 {
                    solid[g.index(i, j, k)] = true;
                }
            }
        }
        solid[g.index(12, 12, 12)] = true;
        keep_largest(&g, &mut solid);
        assert_eq!(solid.iter().filter(|&&s| s).count(), 216);
        assert!(!solid[g.index(12, 12, 12)]);
    }
}
