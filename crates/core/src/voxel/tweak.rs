// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Tweaks in voxel mode: one step of an operation over the whole clay,
//! worked on its signed distance field and meshed again like a remesh.
//!
//! Every click rebuilds the clay from the field at the chosen resolution,
//! so the triangles come out as even as a fresh remesh however many times
//! a tweak is used: nothing piles up from one click to the next, unlike
//! moving the same vertices again and again (see `sculpt/tweak.rs`, the
//! other mode).
//!
//! - inflate and shrink take a fixed amount off the field or add it
//!   ([`swell`]): the surface moves out or in along its normals;
//! - smooth blurs the field ([`passes`]), and sharpen adds back
//!   [`SHARPEN`] times what a blur takes away (an unsharp mask, over a
//!   light blur so the grid's grain doesn't grow), at most [`MAX_SHARPEN`]
//!   of a cell. Both then shift the field as a whole so the clay keeps its
//!   volume (a blur alone wears thin parts away), meshing it a second time
//!   if the first lost some.
//!
//! The mask holds the field back where the clay is masked, as it holds
//! back the brushes; the clay there is still meshed again. Everything is
//! plain arithmetic in a fixed order, so the same clay gives the same
//! bits, whatever the machine.

use glam::Vec3;

use super::grid::Grid;
use super::{MAX_RESOLUTION, MIN_RESOLUTION, mesh_level, sdf};
use crate::sculpt::{Mesh, Tweak};

/// Smooth blurs the field by [`passes`]: smoothing over about 1% of the
/// clay's largest side, gentle but past what meshing again takes off by
/// itself.
const SMOOTH_SHARE: f32 = 1.0 / 50.0;
/// Sharpen adds this many times the difference between a light blur of
/// the field ([`LIGHT`] passes, which keep the grid's own grain from
/// growing) and a blur of [`passes`] more: what is a few cells across or
/// more stands out further, what is finer goes.
pub const SHARPEN: f32 = 3.0;
const LIGHT: usize = 2;
/// Sharpen moves the field by at most this share of a cell, so it never
/// makes a surface away from the clay's own.
pub const MAX_SHARPEN: f32 = 0.5;
/// Inflate and shrink move the surface by this share of the clay's
/// largest side, as the other mode does...
pub const SWELL: f32 = 0.005;
/// ...but by at most this share of a cell: the field is only a true
/// distance for a cell and a half around the surface.
const MAX_SWELL: f32 = 0.8;

/// How far inflate and shrink move the surface on a grid of `step` with
/// `resolution` cells along the clay's largest side.
pub fn swell(step: f32, resolution: u32) -> f32 {
    step * (SWELL * resolution as f32).min(MAX_SWELL)
}

/// Blur passes for smooth and sharpen at `resolution`: each blurs by about
/// half a cell, so more cells take more passes to blur over the same share
/// of the clay.
pub fn passes(resolution: u32) -> usize {
    ((resolution as f32 * SMOOTH_SHARE).round() as usize).max(2)
}

/// Whether a tweak could change anything: false when every vertex is
/// fully masked.
pub fn can_tweak(mesh: &Mesh) -> bool {
    let mask = &mesh.layers.mask;
    mask.is_empty() || (0..mesh.positions.len()).any(|v| mask.get(v) < 1.0)
}

/// The clay after one step of `op`, meshed at `resolution` cells along
/// its largest side, without its mask and chalk (see
/// `Sculpt::layers_for`). `None` if nothing is left, or nothing could
/// move (the clay all masked).
pub fn tweaked(mesh: &Mesh, op: Tweak, resolution: u32) -> Option<Mesh> {
    if !can_tweak(mesh) {
        return None;
    }
    let resolution = resolution.clamp(MIN_RESOLUTION / 4, MAX_RESOLUTION);
    let mut field = sdf::signed_distance(&mesh.positions, &mesh.triangles, resolution, 0)?;
    let free = freedom(mesh, &field);
    apply(&mut field, op, resolution, free.as_deref());
    if matches!(op, Tweak::Inflate | Tweak::Shrink) {
        return mesh_level(&mut field);
    }
    // Smooth and sharpen keep the clay's volume. The field kept its own
    // (see `keep_volume`), but meshing it can lose a little more, which
    // twenty clicks would add up: a second meshing makes up for what the
    // first lost, the field moved by the volume over the area.
    let kept = copy(&field);
    let first = mesh_level(&mut field)?;
    let target = volume_and_area(mesh).0;
    let (volume, area) = volume_and_area(&first);
    let limit = 0.5 * field.step;
    let shift = ((target - volume) / area) as f32;
    if !(target > 0.0 && area > 0.0 && shift.abs() > 1e-3 * field.step) {
        return Some(first);
    }
    let shift = shift.clamp(-limit, limit);
    let mut field = kept;
    for (i, v) in field.values.iter_mut().enumerate() {
        *v -= shift * free.as_ref().map_or(1.0, |f| f[i]);
    }
    mesh_level(&mut field)
}

/// The volume a closed mesh holds and its area, summed in triangle order.
fn volume_and_area(mesh: &Mesh) -> (f64, f64) {
    let (mut volume, mut area) = (0.0f64, 0.0f64);
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|v| mesh.positions[v as usize].as_dvec3());
        volume += a.dot(b.cross(c)) / 6.0;
        area += (b - a).cross(c - a).length() / 2.0;
    }
    (volume, area)
}

fn copy(field: &Grid) -> Grid {
    Grid {
        origin: field.origin,
        step: field.step,
        dims: field.dims,
        values: field.values.clone(),
    }
}

/// One step of `op` on `field`, each node moved `free` of the way.
fn apply(field: &mut Grid, op: Tweak, resolution: u32, free: Option<&[f32]>) {
    let free = |i: usize| free.map_or(1.0, |f| f[i]);
    let step = field.step;
    match op {
        Tweak::Inflate | Tweak::Shrink => {
            let swell = swell(step, resolution);
            let by = if op == Tweak::Inflate { -swell } else { swell };
            for (i, v) in field.values.iter_mut().enumerate() {
                *v += by * free(i);
            }
        }
        Tweak::Smooth => {
            let volume = volume(&field.values, step);
            let blurred = blurred(field, passes(resolution));
            for (i, v) in field.values.iter_mut().enumerate() {
                *v += (blurred[i] - *v) * free(i);
            }
            keep_volume(field, volume, &free);
        }
        Tweak::Sharpen => {
            let volume = volume(&field.values, step);
            // In distances: a blur flattens the field's slope, the more
            // passes the more, which would weigh the two blurs unevenly.
            let mut blur = copy(field);
            for _ in 0..LIGHT {
                blur.blur();
            }
            let light = distances(field, &blur.values);
            for _ in 0..passes(resolution) {
                blur.blur();
            }
            let heavy = distances(field, &blur.values);
            let limit = MAX_SHARPEN * step;
            for (i, v) in field.values.iter_mut().enumerate() {
                let target = light[i] + SHARPEN * (light[i] - heavy[i]);
                *v += (target - *v).clamp(-limit, limit) * free(i);
            }
            keep_volume(field, volume, &free);
        }
    }
}

/// The values of `field` after `passes` of [`Grid::blur`].
fn blurred(field: &Grid, passes: usize) -> Vec<f32> {
    let mut out = copy(field);
    for _ in 0..passes {
        out.blur();
    }
    out.values
}

/// About how far each node is from the zero level of `values` on the grid
/// of `field`: its value over the length of its gradient (at least a
/// quarter, where the field runs flat away from the surface).
fn distances(field: &Grid, values: &[f32]) -> Vec<f32> {
    let dims = field.dims;
    let strides = [1, dims[0], dims[0] * dims[1]];
    (0..values.len())
        .map(|i| {
            let at = field.coords(i);
            let mut slope = 0.0f32;
            for axis in 0..3 {
                let s = strides[axis];
                let (before, after) = (at[axis] > 0, at[axis] + 1 < dims[axis]);
                let lo = if before { values[i - s] } else { values[i] };
                let hi = if after { values[i + s] } else { values[i] };
                let span = (usize::from(before) + usize::from(after)) as f32 * field.step;
                if span > 0.0 {
                    slope += ((hi - lo) / span).powi(2);
                }
            }
            values[i] / slope.sqrt().max(0.25)
        })
        .collect()
}

/// How much of each node's cell is inside, 0 to 1: the field ramps from
/// all inside half a cell in to all outside half a cell out.
fn inside(value: f32, step: f32) -> f32 {
    (0.5 - value / step).clamp(0.0, 1.0)
}

/// The volume inside `values`, in cells, summed in node order.
fn volume(values: &[f32], step: f32) -> f64 {
    values.iter().map(|&v| f64::from(inside(v, step))).sum()
}

/// Shifts `field` by the same amount everywhere (times `free`) so that it
/// holds `target` cells again; by at most half a cell.
fn keep_volume(field: &mut Grid, target: f64, free: &impl Fn(usize) -> f32) {
    let step = field.step;
    let limit = 0.5 * step;
    let mut shift = 0.0f32;
    for _ in 0..8 {
        let (mut volume, mut slope) = (0.0f64, 0.0f64);
        for (i, &v) in field.values.iter().enumerate() {
            let w = free(i);
            let v = v - shift * w;
            volume += f64::from(inside(v, step));
            if v.abs() < 0.5 * step {
                slope += f64::from(w / step);
            }
        }
        let off = volume - target;
        if off.abs() < 0.5 || slope <= 0.0 {
            break;
        }
        // Taking `shift` off grows the inside by `slope` cells per unit.
        shift = (shift - (off / slope) as f32).clamp(-limit, limit);
    }
    for (i, v) in field.values.iter_mut().enumerate() {
        *v -= shift * free(i);
    }
}

/// How free each node is to change, 0 to 1: one less the mask on the clay
/// near it, from points over every triangle about a cell apart, spread to
/// the nodes around them and blurred out past the band of true distances.
/// `None` without a mask.
fn freedom(mesh: &Mesh, field: &Grid) -> Option<Vec<f32>> {
    let mask = &mesh.layers.mask;
    if mask.is_empty() {
        return None;
    }
    let grid = |values| Grid {
        origin: field.origin,
        step: field.step,
        dims: field.dims,
        values,
    };
    let mut weight = grid(vec![0.0; field.len()]);
    let mut masked = grid(vec![0.0; field.len()]);
    let [nx, ny, nz] = field.dims;
    let mut splat = |p: Vec3, m: f32| {
        let at = (p - field.origin) / field.step;
        let base = at.floor();
        let t = at - base;
        let [i, j, k] = base.to_array().map(|b| b as i64);
        for corner in 0..8 {
            let (di, dj, dk) = (corner & 1, (corner >> 1) & 1, corner >> 2);
            let (a, b, c) = (i + di, j + dj, k + dk);
            if a < 0 || b < 0 || c < 0 || a >= nx as i64 || b >= ny as i64 || c >= nz as i64 {
                continue;
            }
            let pick = |d: i64, t: f32| if d == 1 { t } else { 1.0 - t };
            let w = pick(di, t.x) * pick(dj, t.y) * pick(dk, t.z);
            let at = field.index(a as usize, b as usize, c as usize);
            weight.values[at] += w;
            masked.values[at] += w * m;
        }
    };
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|v| mesh.positions[v as usize]);
        let [ma, mb, mc] = t.map(|v| mask.get(v as usize));
        let longest = a.distance(b).max(b.distance(c)).max(c.distance(a));
        let n = ((longest / field.step).ceil() as usize).clamp(1, 256);
        for i in 0..=n {
            for j in 0..=n - i {
                let (u, v) = (i as f32 / n as f32, j as f32 / n as f32);
                let p = a + (b - a) * u + (c - a) * v;
                let m = ma + (mb - ma) * u + (mc - ma) * v;
                splat(p, m);
            }
        }
    }
    for _ in 0..2 {
        weight.blur();
        masked.blur();
    }
    Some(
        weight
            .values
            .iter()
            .zip(&masked.values)
            .map(|(&w, &m)| {
                if w > 1e-6 {
                    (1.0 - m / w).clamp(0.0, 1.0)
                } else {
                    1.0
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meshio::is_closed;
    use crate::sculpt::{Brush, Dab, Sculpt};

    /// Tests run in debug builds: a coarser grid than the app's 160.
    const RESOLUTION: u32 = 64;
    /// Coarser still for twenty clicks of everything.
    const COARSE: u32 = 48;

    /// A unit sphere with bumps a few cells across.
    fn bumpy_sphere() -> Mesh {
        let sphere = Mesh::icosphere(5);
        let positions = sphere
            .positions
            .iter()
            .map(|&p| {
                p * (1.0 + 0.03 * (14.0 * p.x).sin() * (14.0 * p.y).cos() * (14.0 * p.z).sin())
            })
            .collect();
        Mesh::new(positions, sphere.triangles)
    }

    /// A sphere sculpted with a few clay strokes, remeshed.
    pub(crate) fn sculpted() -> Mesh {
        let mut s = Sculpt::new(Mesh::icosphere(5));
        for (k, at) in [
            Vec3::Z,
            Vec3::new(0.6, 0.3, 0.7),
            Vec3::new(-0.5, -0.4, 0.8),
        ]
        .into_iter()
        .enumerate()
        {
            s.begin_stroke();
            for i in 0..8 {
                let c = (at + Vec3::Y * 0.05 * i as f32 + Vec3::X * 0.01 * k as f32).normalize();
                s.dab(Dab {
                    brush: Brush::Clay,
                    centre: c,
                    radius: 0.25,
                    strength: 1.0,
                    invert: false,
                    symmetry: false,
                    direction: if i == 0 { Vec3::ZERO } else { Vec3::Y },
                    buildup: Default::default(),
                    fill: false,
                    wipes: true,
                });
                s.refresh();
            }
            s.end_stroke();
        }
        super::super::remesh(&s.mesh, RESOLUTION).unwrap()
    }

    fn mesh_volume(mesh: &Mesh) -> f32 {
        mesh.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|v| mesh.positions[v as usize]);
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }

    fn centre(mesh: &Mesh) -> Vec3 {
        mesh.positions.iter().sum::<Vec3>() / mesh.positions.len() as f32
    }

    fn mean_radius(mesh: &Mesh) -> f32 {
        let c = centre(mesh);
        mesh.positions.iter().map(|p| p.distance(c)).sum::<f32>() / mesh.positions.len() as f32
    }

    /// How far the surface strays from its mean radius, on average.
    fn roughness(mesh: &Mesh) -> f32 {
        let (c, r) = (centre(mesh), mean_radius(mesh));
        mesh.positions
            .iter()
            .map(|p| (p.distance(c) - r).abs())
            .sum::<f32>()
            / mesh.positions.len() as f32
    }

    /// The spread of the edge lengths (their deviation over their mean),
    /// the smallest angle of any triangle, in degrees, and the share of
    /// triangles with an angle under 15°.
    pub(crate) fn quality(mesh: &Mesh) -> (f32, f32, f32) {
        let mut lengths = Vec::new();
        let mut smallest = 180.0f32;
        let mut thin = 0;
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|v| mesh.positions[v as usize]);
            let mut least = 180.0f32;
            for (p, q, r) in [(a, b, c), (b, c, a), (c, a, b)] {
                lengths.push(p.distance(q));
                let angle = (q - p).angle_between(r - p).to_degrees();
                if angle.is_finite() {
                    least = least.min(angle);
                }
            }
            smallest = smallest.min(least);
            thin += usize::from(least < 15.0);
        }
        let mean = lengths.iter().sum::<f32>() / lengths.len() as f32;
        let var = lengths.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / lengths.len() as f32;
        let thin = thin as f32 / mesh.triangles.len() as f32;
        (var.sqrt() / mean, smallest, thin)
    }

    /// Closed, every edge shared by two triangles facing the same way,
    /// each vertex's triangles one fan around it, in one piece, and with
    /// every vertex in use. (Sharpened often enough, two bumps may meet
    /// and make a handle: that is still a sound surface.)
    pub(crate) fn assert_sound(mesh: &Mesh, what: &str) {
        assert!(is_closed(&mesh.triangles), "{what}: not closed");
        let (positions, triangles) =
            super::super::nets::keep_largest(&mesh.positions, &mesh.triangles);
        assert_eq!(
            triangles.len(),
            mesh.triangles.len(),
            "{what}: not one piece"
        );
        assert_eq!(
            positions.len(),
            mesh.positions.len(),
            "{what}: stray vertices"
        );
        // Around each vertex, the edge after it in each triangle leads to
        // the next triangle: one cycle through all of them.
        let mut fans: Vec<Vec<(u32, u32)>> = vec![Vec::new(); mesh.positions.len()];
        for &[a, b, c] in &mesh.triangles {
            fans[a as usize].push((b, c));
            fans[b as usize].push((c, a));
            fans[c as usize].push((a, b));
        }
        for (v, fan) in fans.iter().enumerate() {
            let (first, mut at) = fan[0];
            let mut steps = 1;
            while at != first {
                let Some(&(_, next)) = fan.iter().find(|&&(from, _)| from == at) else {
                    panic!("{what}: vertex {v}'s fan is open");
                };
                at = next;
                steps += 1;
                assert!(steps <= fan.len(), "{what}: vertex {v}'s fan doesn't close");
            }
            assert_eq!(steps, fan.len(), "{what}: vertex {v} joins two fans");
        }
    }

    #[test]
    fn each_tweak_does_what_it_says() {
        let start = super::super::remesh(&bumpy_sphere(), RESOLUTION).unwrap();
        let (radius, rough) = (mean_radius(&start), roughness(&start));
        let inflated = tweaked(&start, Tweak::Inflate, RESOLUTION).unwrap();
        let shrunk = tweaked(&start, Tweak::Shrink, RESOLUTION).unwrap();
        let smoothed = tweaked(&start, Tweak::Smooth, RESOLUTION).unwrap();
        let sharpened = tweaked(&start, Tweak::Sharpen, RESOLUTION).unwrap();
        // The step: 0.5% of the largest side (2.06), at most 0.8 cells.
        let step = swell(2.06 / RESOLUTION as f32, RESOLUTION);
        let grew = mean_radius(&inflated) - radius;
        let fell = radius - mean_radius(&shrunk);
        assert!(
            (grew - step).abs() < step * 0.3,
            "inflate grew {grew}, not {step}"
        );
        assert!(
            (fell - step).abs() < step * 0.3,
            "shrink took {fell}, not {step}"
        );
        let (smooth, sharp) = (roughness(&smoothed), roughness(&sharpened));
        assert!(smooth < rough * 0.9, "smooth: {smooth} vs {rough}");
        eprintln!("roughness {rough}: smoothed {smooth}, sharpened {sharp}");
        assert!(sharp > rough * 1.05, "sharpen: {sharp} vs {rough}");
        assert!(sharp < rough * 2.0, "gentle: {sharp} vs {rough}");
        let v = mesh_volume(&start);
        for (mesh, what) in [(&smoothed, "smooth"), (&sharpened, "sharpen")] {
            let change = mesh_volume(mesh) / v - 1.0;
            assert!(change.abs() < 0.01, "{what} changed the volume by {change}");
            assert_sound(mesh, what);
        }
    }

    #[test]
    fn twenty_clicks_of_each_tweak_leave_a_sound_even_mesh() {
        let mut bad = Vec::new();
        for (shape, start) in [("sphere", Mesh::icosphere(5)), ("sculpted", sculpted())] {
            let start = super::super::remesh(&start, COARSE).unwrap();
            let fresh = quality(&start);
            let volume = mesh_volume(&start);
            for op in Tweak::ALL {
                let mut mesh = start.clone();
                for _ in 0..20 {
                    mesh = tweaked(&mesh, op, COARSE).unwrap();
                }
                let what = format!("{shape}, 20 × {op:?}");
                assert_sound(&mesh, &what);
                let (spread, smallest, thin) = quality(&mesh);
                let change = mesh_volume(&mesh) / volume - 1.0;
                eprintln!(
                    "{what}: volume {change:+.4}, spread {spread:.3} (fresh {:.3}), smallest {smallest:.1}° (fresh {:.1}°), under 15° {:.3}% (fresh {:.3}%)",
                    fresh.0,
                    fresh.1,
                    thin * 100.0,
                    fresh.2 * 100.0
                );
                if spread > fresh.0 * 1.25 || smallest < 5.0 || thin > fresh.2 * 2.0 + 0.002 {
                    bad.push(format!(
                        "{what}: spread {spread}, smallest {smallest}°, thin {thin}"
                    ));
                }
                if matches!(op, Tweak::Smooth | Tweak::Sharpen) && change.abs() > 0.03 {
                    bad.push(format!("{what}: volume changed by {change}"));
                }
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn the_same_clay_tweaks_to_the_same_bits() {
        let start = sculpted();
        for op in Tweak::ALL {
            let a = tweaked(&start, op, RESOLUTION).unwrap();
            let b = tweaked(&start, op, RESOLUTION).unwrap();
            assert_eq!(a.positions, b.positions, "{op:?}");
            assert_eq!(a.triangles, b.triangles, "{op:?}");
        }
    }

    #[test]
    fn the_mask_holds_the_field_back() {
        let mut start = super::super::remesh(&Mesh::icosphere(5), RESOLUTION).unwrap();
        let n = start.positions.len();
        // The +X half frozen.
        for v in 0..n {
            let m = if start.positions[v].x > 0.0 { 1.0 } else { 0.0 };
            start.layers.mask.set(v, m, n);
        }
        let inflated = tweaked(&start, Tweak::Inflate, RESOLUTION).unwrap();
        let radius = |mesh: &Mesh, side: f32| {
            let far: Vec<f32> = mesh
                .positions
                .iter()
                .filter(|p| p.x * side > 0.5)
                .map(|p| p.length())
                .collect();
            far.iter().sum::<f32>() / far.len() as f32
        };
        let step = swell(2.0 / RESOLUTION as f32, RESOLUTION);
        let held = radius(&inflated, 1.0) - radius(&start, 1.0);
        let grew = radius(&inflated, -1.0) - radius(&start, -1.0);
        assert!(held.abs() < step * 0.2, "the masked side moved {held}");
        assert!(
            (grew - step).abs() < step * 0.3,
            "the free side grew {grew}"
        );
        // All masked: nothing to do.
        for v in 0..n {
            start.layers.mask.set(v, 1.0, n);
        }
        assert!(!can_tweak(&start));
        assert!(tweaked(&start, Tweak::Smooth, RESOLUTION).is_none());
    }

    /// Release timings: `cargo test --release -p splatstudy-core
    /// voxel_tweak_timings -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn voxel_tweak_timings() {
        let time = |what: &str, mesh: &Mesh, resolution: u32| {
            for op in Tweak::ALL {
                let started = std::time::Instant::now();
                let out = tweaked(mesh, op, resolution).unwrap();
                eprintln!(
                    "{what} at {resolution}, {op:?}: {:.0} ms ({} vertices)",
                    started.elapsed().as_secs_f64() * 1e3,
                    out.positions.len()
                );
            }
        };
        let sphere = Mesh::icosphere(5);
        let at160 = super::super::remesh(&sphere, 160).unwrap();
        let at256 = super::super::remesh(&sphere, 256).unwrap();
        time("sphere", &at160, 160);
        time("sphere", &at256, 256);
        let fine = crate::sculpt::cube_sphere(crate::sculpt::START_LEVEL + 2).0;
        eprintln!("fine clay: {} vertices", fine.positions.len());
        time("fine clay", &fine, 160);
        time("fine clay", &fine, 256);

        // Twenty clicks at the app's resolution.
        for (shape, start) in [
            ("sphere", at160.clone()),
            ("sculpted", {
                let s = sculpted();
                super::super::remesh(&s, 160).unwrap()
            }),
        ] {
            let fresh = quality(&start);
            let volume = mesh_volume(&start);
            for op in Tweak::ALL {
                let mut mesh = start.clone();
                for _ in 0..20 {
                    mesh = tweaked(&mesh, op, 160).unwrap();
                }
                assert_sound(&mesh, shape);
                let (spread, smallest, thin) = quality(&mesh);
                let change = mesh_volume(&mesh) / volume - 1.0;
                eprintln!(
                    "160, {shape}, 20 × {op:?}: volume {change:+.4}, radius {:.4} → {:.4}, roughness {:.5} → {:.5}, spread {spread:.3} (fresh {:.3}), smallest {smallest:.1}° (fresh {:.1}°), under 15° {:.3}% (fresh {:.3}%)",
                    mean_radius(&start),
                    mean_radius(&mesh),
                    roughness(&start),
                    roughness(&mesh),
                    fresh.0,
                    fresh.1,
                    thin * 100.0,
                    fresh.2 * 100.0
                );
            }
        }
    }
}
