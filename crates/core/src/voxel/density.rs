// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Clay from splats: how densely the splats cover each spot, as a grid.
//!
//! Splats describe surfaces: a captured cast is a shell of them, hollow
//! inside, with stray splats (floaters) around. So the shell is found
//! where the density passes a threshold, its gaps are closed, everything
//! the outside can't reach counts as inside, and only the largest piece
//! is kept.

use glam::Vec3;

use super::grid::{self, Grid};
use crate::reference::Blob;

/// Splats fainter than this are ignored.
const MIN_OPACITY: f32 = 0.02;
/// Splats larger than this many cells count as this large, which bounds
/// the work one splat can make.
const MAX_RADIUS: f32 = 2.0;
/// Gaps in the shell up to about twice this many cells wide are closed.
const CLOSE: usize = 2;
/// How far, in clay units, splats may lie from the centre; the reference
/// is fitted inside radius 1, so farther ones are leftovers.
const REACH: f32 = 1.5;

/// The material splats make, negative inside: `threshold` (0..1) is the
/// density that counts as material, relative to the densest parts of the
/// shell. `None` if no splat is left.
pub fn field(blobs: &[Blob], resolution: u32, threshold: f32) -> Option<Grid> {
    let blobs: Vec<&Blob> = blobs
        .iter()
        .filter(|b| {
            b.opacity > MIN_OPACITY
                && b.centre.is_finite()
                && b.radius.is_finite()
                && b.centre.abs().max_element() <= REACH
        })
        .collect();
    let (min, max) = blobs.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), b| {
        (lo.min(b.centre), hi.max(b.centre))
    });
    // The blur and the larger splats spread up to three cells past the centres.
    let mut g = Grid::around(min, max, resolution, 4 + CLOSE, 0.0)?;
    let inv = 1.0 / g.step;
    for b in &blobs {
        let at = (b.centre - g.origin) * inv;
        let radius = (b.radius * inv).min(MAX_RADIUS);
        if radius <= 1.0 {
            // Small: shared out between the eight nodes around it.
            let cell = at.floor();
            let f = at - cell;
            let [i, j, k] = [cell.x as usize, cell.y as usize, cell.z as usize];
            for d in 0..8 {
                let (dx, dy, dz) = (d & 1, d >> 1 & 1, d >> 2);
                let w = if dx == 1 { f.x } else { 1.0 - f.x }
                    * if dy == 1 { f.y } else { 1.0 - f.y }
                    * if dz == 1 { f.z } else { 1.0 - f.z };
                let node = g.index(i + dx, j + dy, k + dz);
                g.values[node] += b.opacity * w;
            }
        } else {
            // Large: a soft ball. A splat is a patch of surface, so its
            // weight grows with its area (radius squared, in cells), as if
            // it were that many small splats side by side.
            let peak = b.opacity / ((2.0 * std::f32::consts::PI).powf(1.5) * radius);
            let lo = (at - Vec3::splat(radius * 2.0)).ceil().max(Vec3::ZERO);
            let hi = (at + Vec3::splat(radius * 2.0)).floor();
            let inv_r2 = 1.0 / (2.0 * radius * radius);
            for k in lo.z as usize..=(hi.z as usize).min(g.dims[2] - 1) {
                for j in lo.y as usize..=(hi.y as usize).min(g.dims[1] - 1) {
                    for i in lo.x as usize..=(hi.x as usize).min(g.dims[0] - 1) {
                        let d2 = Vec3::new(i as f32, j as f32, k as f32).distance_squared(at);
                        let node = g.index(i, j, k);
                        g.values[node] += peak * (-d2 * inv_r2).exp();
                    }
                }
            }
        }
    }
    blur(&mut g);

    // The threshold, relative to the densest parts of the shell (the 95th
    // percentile, so a few piles of splats don't set the scale).
    let mut dense: Vec<f32> = g.values.iter().copied().filter(|&v| v > 1e-6).collect();
    if dense.is_empty() {
        return None;
    }
    let at = (dense.len() - 1) * 95 / 100;
    let (_, &mut top, _) = dense.select_nth_unstable_by(at, f32::total_cmp);
    drop(dense);
    let level = threshold.clamp(0.01, 1.0) * top;

    let shell: Vec<bool> = g.values.iter().map(|&v| v >= level).collect();
    let out = grid::outside(&g, &shell, CLOSE);
    let mut solid: Vec<bool> = out.iter().map(|&o| !o).collect();
    grid::keep_largest(&g, &mut solid);
    // Below zero inside, by how far the density passes the threshold; the
    // closed gaps and the hollow inside are just inside.
    let margin = 0.05;
    for (v, &inside) in g.values.iter_mut().zip(&solid) {
        let t = 1.0 - *v / level;
        *v = if inside {
            t.min(-margin)
        } else {
            t.max(margin)
        };
    }
    Some(g)
}

/// Blurs the grid along x, y and z with the weights 1 4 6 4 1, close to a
/// Gaussian one cell wide: it joins neighbouring splats into a surface.
fn blur(g: &mut Grid) {
    const W: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let steps = [1, g.dims[0], g.dims[0] * g.dims[1]];
    let mut line = Vec::new();
    for axis in 0..3 {
        let n = g.dims[axis];
        let stride = steps[axis];
        let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
        for u in 0..g.dims[a] {
            for v in 0..g.dims[b] {
                let start = u * steps[a] + v * steps[b];
                line.clear();
                line.extend((0..n).map(|t| g.values[start + t * stride]));
                for t in 0..n {
                    let mut sum = 0.0;
                    for (o, w) in W.iter().enumerate() {
                        if let Some(&x) = (t + o).checked_sub(2).and_then(|s| line.get(s)) {
                            sum += x * w;
                        }
                    }
                    g.values[start + t * stride] = sum;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meshio::is_closed;
    use crate::voxel::mesh_level;

    /// `count` blobs spread evenly over a sphere of `radius` around `centre`.
    fn shell(count: usize, centre: Vec3, radius: f32, size: f32) -> Vec<Blob> {
        let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
        (0..count)
            .map(|i| {
                let y = 1.0 - 2.0 * (i as f32 + 0.5) / count as f32;
                let r = (1.0 - y * y).sqrt();
                let a = golden * i as f32;
                Blob {
                    centre: centre + Vec3::new(r * a.cos(), y, r * a.sin()) * radius,
                    radius: size,
                    opacity: 0.8,
                }
            })
            .collect()
    }

    #[test]
    fn a_shell_of_splats_becomes_a_solid_ball() {
        let mut blobs = shell(4000, Vec3::ZERO, 0.9, 0.02);
        // Floaters, and a few large soft splats on the shell.
        blobs.push(Blob {
            centre: Vec3::new(1.4, 1.4, 0.0),
            radius: 0.01,
            opacity: 1.0,
        });
        blobs.extend(shell(40, Vec3::ZERO, 0.9, 0.08));
        let mut g = field(&blobs, 48, 0.25).unwrap();
        let mesh = mesh_level(&mut g, false).unwrap();
        assert!(is_closed(&mesh.triangles));
        for p in &mesh.positions {
            let r = p.length();
            // The outside of the shell: a solid ball, no inner wall.
            assert!((0.84..1.0).contains(&r), "{r}");
        }
    }

    #[test]
    fn nothing_or_only_faint_splats_give_nothing() {
        assert!(field(&[], 48, 0.25).is_none());
        let faint = shell(100, Vec3::ZERO, 0.5, 0.02)
            .into_iter()
            .map(|b| Blob { opacity: 0.01, ..b })
            .collect::<Vec<_>>();
        assert!(field(&faint, 48, 0.25).is_none());
    }
}
