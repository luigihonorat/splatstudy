// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Clay from splats by depth fusion, the way recent splat-to-mesh methods
//! (2DGS, Gaussian Opacity Fields, RaDe-GS) do it: the splats are drawn
//! from many directions around the shape, each view keeps the depth where
//! its rays have gathered half their opacity (the median depth), and the
//! views are fused into a truncated signed distance on a grid.
//!
//! Each view trusts its depth over a stretch that grows with how fuzzy
//! the surface is there (how deep the opacity takes to gather), and less
//! so. A view that sees through a node, past it to empty space or to a
//! surface well behind it, votes for it being outside, against the views
//! that see a surface near it.
//!
//! The result is solid, not a shell: the nodes no view sees into stay
//! inside. A splat has no front or back, so each
//! one's outer side is taken to be the side the views see most of it
//! from. A ray that meets a surface from its inner side, through a gap in
//! the shell or from behind a capture that only saw the front, carves
//! nothing: gaps don't become tunnels, and a front-only capture becomes
//! solid behind its front.
//!
//! The crop box is a wall: the solid is built from all the splats around
//! the box, inside it or not, so the cut doesn't open the shell to the
//! views, and is then cut by the box, which closes the cut with a flat
//! face.
//!
//! The views are drawn on the CPU, front to back, so the median depth is
//! exact and the same on every platform, and the whole thing runs at once
//! (no waiting for the GPU across frames). Splats much smaller than a
//! pixel are merged first, a pixel-sized cell at a time. On desktop the
//! views and the grid are shared out between threads.

use glam::{Mat3, UVec3, Vec3};

use super::grid::{self, Grid};
use crate::reference::{Aabb, Alignment, Splats};

/// Splats fainter than this are left out.
const MIN_OPACITY: f32 = 0.02;
/// How far, in clay units, the shape may reach from the centre; the
/// reference is fitted inside radius 1, so farther splats are leftovers.
const REACH: f32 = 1.5;
/// Views around the shape, spread evenly over a sphere.
const VIEWS: usize = 32;
/// Pixel size of the views, in grid steps.
const PIXEL: f32 = 1.5;
/// Splats whose three sigmas fit in a pixel are merged with those in
/// the same pixel-sized cell.
const MERGE: f32 = 1.0;
/// Splats larger than this fraction of the region (three sigmas) are
/// left out: haze and background, too large to say where a surface is.
const LARGE: f32 = 0.3;
/// Views are at most this many pixels across.
const MAX_PIXELS: usize = 1024;
/// Added to each splat's footprint, in square pixels: every splat covers
/// at least about a pixel, and splats a little apart join into a surface.
const DILATE: f32 = 0.25;
/// A pixel stops gathering once less than this much light gets through.
const OPAQUE: f32 = 0.05;
/// Rays gathering less opacity than this fraction of the surface's level
/// pass through empty space.
const EMPTY: f32 = 0.5;
/// The truncation of the signed distance, in grid steps: how far from a
/// surface a view says anything about it but "outside".
const TRUNCATION: f32 = 3.0;
/// A splat's outer side is known when the views see one side this much
/// more than the other (as a fraction of all they see of it).
const SIDEDNESS: f32 = 0.2;
/// A view trusts its depth over this many times the depth of the
/// surface's fuzz around it, at least the truncation...
const FUZZ: f32 = 1.5;
/// ...and at most this many truncations.
const MAX_REACH: f32 = 4.0;
/// Splats this many region radii around count, and the views start that
/// far out.
const AROUND: f32 = 3.0;
/// Grid nodes added around the splats.
const PAD: usize = 4;
/// Gaps in the solid up to about twice this many nodes wide are closed.
const CLOSE: usize = 2;
/// How much a view that sees through a node counts towards it being
/// outside, against the views that see a surface near it (each weighing
/// up to 1).
const FREE: f32 = 1.0;

/// A splat in the clay's space.
#[derive(Clone, Copy)]
struct Splat {
    centre: Vec3,
    /// Covariance: xx, xy, xz, yy, yz, zz.
    cov: [f32; 6],
    opacity: f32,
}

impl Splat {
    fn times(&self, v: Vec3) -> Vec3 {
        let [xx, xy, xz, yy, yz, zz] = self.cov;
        Vec3::new(
            xx * v.x + xy * v.y + xz * v.z,
            xy * v.x + yy * v.y + yz * v.z,
            xz * v.x + yz * v.y + zz * v.z,
        )
    }
}

/// An orthographic view looking along `forward`, centred on the region.
#[derive(Clone, Copy)]
struct View {
    right: Vec3,
    up: Vec3,
    forward: Vec3,
}

/// What every view shares: the region they look at and their pixels.
struct Frame {
    centre: Vec3,
    /// Radius of the sphere around the region, which each view frames.
    radius: f32,
    /// How far in front of the centre the views start (depth 0), and how
    /// far around it splats count: farther than the region, so a view
    /// rarely starts inside a figure the crop box cuts.
    near: f32,
    /// Pixel size, in clay units.
    pixel: f32,
    /// Pixels across.
    size: usize,
}

/// What a view saw at each pixel.
struct Depths {
    /// Depth where the ray gathered the surface's level of opacity;
    /// infinite where it passed through empty space, NaN where it gathered
    /// some opacity but not enough to tell.
    depth: Vec<f32>,
    /// How deep the surface's fuzz runs around that depth.
    fuzz: Vec<f32>,
    /// Until the splats are oriented, the splat at that depth; then, the
    /// pixel's weight, negative where the ray met the surface from inside.
    weight: Vec<f32>,
    splat: Vec<u32>,
}

/// The splats' solid, negative inside, `resolution` cells across: what is
/// inside the crop box (`crop`, in file coordinates) of the splats placed
/// by `align`. `level` (0..1) is the opacity a ray gathers before it counts
/// as having reached the surface; `smoothness` blurs the result that many
/// more times (see [`Grid::blur`]). `None` if nothing is left.
pub fn field(
    splats: &Splats,
    align: &Alignment,
    crop: Option<Aabb>,
    resolution: u32,
    level: f32,
    smoothness: u32,
) -> Option<Grid> {
    let level = level.clamp(0.05, 0.95);
    let m = align.matrix();
    let turn = Mat3::from_mat4(m);
    let kept = |p: Vec3| crop.is_none_or(|c| c.contains(p));

    // The region: where the splats inside the crop box are.
    let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
    for (i, &p) in splats.positions.iter().enumerate() {
        let opacity = f32::from(splats.colors[i][3]) / 255.0;
        let w = m.transform_point3(p);
        if opacity > MIN_OPACITY && w.is_finite() && w.abs().max_element() <= REACH && kept(p) {
            min = min.min(w);
            max = max.max(w);
        }
    }
    let mut g = Grid::around(min, max, resolution, PAD, 0.0)?;
    let step = g.step;
    let far = g.point(g.dims[0] - 1, g.dims[1] - 1, g.dims[2] - 1);
    let frame = {
        let centre = (g.origin + far) * 0.5;
        let radius = (far - g.origin).length() * 0.5 + step;
        let size = ((2.0 * radius / (step * PIXEL)).ceil() as usize).clamp(16, MAX_PIXELS);
        Frame {
            centre,
            radius,
            near: radius * AROUND,
            pixel: 2.0 * radius / size as f32,
            size,
        }
    };

    // Every splat in that sphere, inside the crop box or not, in the
    // clay's space, with its shortest axis: the normal of the surface it
    // lies on, either way. Splats much smaller than a pixel are merged
    // with those in the same small cell, which the views could not tell
    // apart anyway. In the order of a curve through space, so splats next
    // to each other in memory are near each other and land near each other
    // in each view's drawing order.
    let cell = step * PIXEL * MERGE;
    let corner = frame.centre - Vec3::splat(frame.near);
    let mut placed: Vec<(u64, Splat, Vec3)> = (0..splats.positions.len())
        .filter_map(|i| {
            let opacity = f32::from(splats.colors[i][3]) / 255.0;
            let centre = m.transform_point3(splats.positions[i]);
            let scale = splats.scales[i] * align.scale;
            let reach = 3.0 * scale.max_element();
            if !(opacity > MIN_OPACITY
                && centre.is_finite()
                && scale.is_finite()
                && centre.distance(frame.centre) < frame.near + reach
                && reach < frame.radius * LARGE)
            {
                return None;
            }
            let rotation = turn * Mat3::from_quat(splats.rotations[i].normalize());
            let axes = rotation * Mat3::from_diagonal(scale);
            let c = axes * axes.transpose();
            let shortest = if scale.x <= scale.y && scale.x <= scale.z {
                0
            } else if scale.y <= scale.z {
                1
            } else {
                2
            };
            let normal = rotation.col(shortest).normalize_or(Vec3::Z);
            let splat = Splat {
                centre,
                cov: [
                    c.x_axis.x, c.y_axis.x, c.z_axis.x, c.y_axis.y, c.z_axis.y, c.z_axis.z,
                ],
                opacity: opacity.min(0.99),
            };
            let at = ((centre - corner) / cell)
                .max(Vec3::ZERO)
                .as_uvec3()
                .min(UVec3::splat(0x1f_ffff));
            // Large splats keep to themselves: odd keys are never merged.
            let large = u64::from(reach > cell);
            Some((morton(at) << 1 | large, splat, normal))
        })
        .collect();
    if placed.is_empty() {
        return None;
    }
    placed.sort_unstable_by_key(|p| p.0);
    let mut world = Vec::with_capacity(placed.len() / 4);
    let mut normals = Vec::with_capacity(placed.len() / 4);
    for run in placed.chunk_by(|a, b| a.0 == b.0 && a.0 & 1 == 0) {
        if let [(_, splat, normal)] = run {
            world.push(*splat);
            normals.push(*normal);
        } else {
            let (splat, normal) = merge(run.iter().map(|p| &p.1));
            world.push(splat);
            normals.push(normal);
        }
    }
    drop(placed);

    let views = directions(VIEWS);
    // Each thread draws some views and adds up what it saw of each splat.
    let drawn = parallel(&views, |views| {
        let mut sides = vec![[0f32; 2]; world.len()];
        let mut scratch = Scratch::default();
        let depths: Vec<Depths> = views
            .iter()
            .map(|v| draw(&world, &normals, &frame, v, level, &mut sides, &mut scratch))
            .collect();
        (depths, sides)
    });
    let mut sides = vec![[0f32; 2]; world.len()];
    let mut maps = Vec::with_capacity(views.len());
    for (depths, s) in drawn {
        for (total, s) in sides.iter_mut().zip(&s) {
            total[0] += s[0];
            total[1] += s[1];
        }
        maps.extend(depths);
    }
    // Each splat's outer side: +1 along its normal, -1 against it, 0 if
    // the views don't tell.
    let outer: Vec<i8> = sides
        .iter()
        .map(|&[along, all]| {
            if all > 0.0 && along.abs() > SIDEDNESS * all {
                if along < 0.0 { 1 } else { -1 }
            } else {
                0
            }
        })
        .collect();
    let truncation = TRUNCATION * step;
    for (map, view) in maps.iter_mut().zip(&views) {
        weigh(map, &frame, view, &normals, &outer, truncation);
    }

    fuse(&mut g, &frame, &views, &maps, truncation);
    // One pass takes out the grain of the splats; the rest are asked for.
    for _ in 0..=smoothness {
        g.blur();
    }
    // Sealed and filled before the cut, so the cut face is whole and flat.
    settle(&mut g, truncation, CLOSE);
    if let Some(crop) = crop {
        wall(&mut g, align, crop, truncation);
        settle(&mut g, truncation, 0);
    }
    Some(g)
}

/// The Morton code of a cell (21 bits per axis): the bits of its
/// coordinates interleaved, so nearby cells mostly get nearby codes.
fn morton(cell: UVec3) -> u64 {
    let spread = |v: u32| {
        let mut x = u64::from(v) & 0x1f_ffff;
        x = (x | x << 32) & 0x001f_0000_0000_ffff;
        x = (x | x << 16) & 0x001f_0000_ff00_00ff;
        x = (x | x << 8) & 0x100f_00f0_0f00_f00f;
        x = (x | x << 4) & 0x10c3_0c30_c30c_30c3;
        (x | x << 2) & 0x1249_2492_4924_9249
    };
    spread(cell.x) | spread(cell.y) << 1 | spread(cell.z) << 2
}

/// One splat standing for several small ones close together: the same
/// mean and spread (weighted by opacity), as opaque as they are stacked,
/// its normal the shortest axis of the spread.
fn merge<'a>(splats: impl Iterator<Item = &'a Splat> + Clone) -> (Splat, Vec3) {
    let (mut weight, mut centre, mut through) = (0.0, Vec3::ZERO, 1.0);
    for s in splats.clone() {
        weight += s.opacity;
        centre += s.centre * s.opacity;
        through *= 1.0 - s.opacity;
    }
    centre /= weight;
    let mut cov = [0.0; 6];
    for s in splats {
        let d = s.centre - centre;
        let spread = [
            d.x * d.x,
            d.x * d.y,
            d.x * d.z,
            d.y * d.y,
            d.y * d.z,
            d.z * d.z,
        ];
        for k in 0..6 {
            cov[k] += s.opacity * (s.cov[k] + spread[k]);
        }
    }
    cov.iter_mut().for_each(|c| *c /= weight);
    let splat = Splat {
        centre,
        cov,
        opacity: (1.0 - through).min(0.99),
    };
    (splat, shortest_axis(cov))
}

/// The eigenvector of the smallest eigenvalue of a symmetric 3 × 3
/// matrix (xx, xy, xz, yy, yz, zz), by the closed form for the
/// eigenvalues.
fn shortest_axis([xx, xy, xz, yy, yz, zz]: [f32; 6]) -> Vec3 {
    let off = xy * xy + xz * xz + yz * yz;
    let q = (xx + yy + zz) / 3.0;
    let p2 = (xx - q).powi(2) + (yy - q).powi(2) + (zz - q).powi(2) + 2.0 * off;
    let p = (p2 / 6.0).sqrt();
    if p.is_nan() || p <= 1e-30 {
        return Vec3::Z;
    }
    let a = Mat3::from_cols(
        Vec3::new(xx, xy, xz),
        Vec3::new(xy, yy, yz),
        Vec3::new(xz, yz, zz),
    );
    let b = (a - Mat3::from_diagonal(Vec3::splat(q))) * (1.0 / p);
    let r = (b.determinant() / 2.0).clamp(-1.0, 1.0);
    let phi = r.acos() / 3.0;
    let smallest = q + 2.0 * p * (phi + 2.0 * std::f32::consts::FRAC_PI_3).cos();
    let m = a - Mat3::from_diagonal(Vec3::splat(smallest));
    // Rows of `m` (it is symmetric: its columns) span the plane the axis
    // is normal to; the longest cross product of two of them is the axis.
    let (r0, r1, r2) = (m.x_axis, m.y_axis, m.z_axis);
    [r0.cross(r1), r0.cross(r2), r1.cross(r2)]
        .into_iter()
        .max_by(|u, v| u.length_squared().total_cmp(&v.length_squared()))
        .and_then(|v| v.try_normalize())
        .unwrap_or(Vec3::Z)
}

/// `n` directions spread evenly over the sphere (a Fibonacci spiral),
/// as views looking along them.
fn directions(n: usize) -> Vec<View> {
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    (0..n)
        .map(|i| {
            let z = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
            let r = (1.0 - z * z).sqrt();
            let a = golden * i as f32;
            let forward = Vec3::new(r * a.cos(), z, r * a.sin()).normalize();
            let helper = if forward.y.abs() < 0.9 {
                Vec3::Y
            } else {
                Vec3::X
            };
            let right = helper.cross(forward).normalize();
            let up = forward.cross(right);
            View { right, up, forward }
        })
        .collect()
}

/// Buffers a thread reuses from view to view.
#[derive(Default)]
struct Scratch {
    /// Per splat: its depth key, then its place in the drawing order;
    /// `u32::MAX` if out of view.
    slots: Vec<u32>,
    counts: Vec<u32>,
    /// The splats in view, front to back.
    footprints: Vec<Footprint>,
    /// How much of each footprint the view saw.
    seen: Vec<f32>,
    light: Vec<f32>,
    /// Per pixel, the depths where the ray has gathered a little and most
    /// of its opacity.
    first: Vec<f32>,
    last: Vec<f32>,
    /// Per tile of the view, its pixels that still let light through.
    open: Vec<u16>,
}

/// Tiles are this many pixels square.
const TILE: usize = 4;

/// A splat as a view sees it, in pixels.
#[derive(Clone, Copy, Default)]
struct Footprint {
    /// Centre, from the view's top left corner, y down.
    x: f32,
    y: f32,
    /// The inverse of its 2D covariance: xx, xy, yy.
    conic: [f32; 3],
    /// Reach on screen.
    radius: f32,
    /// Depth at the centre, and how it changes per pixel across.
    depth: f32,
    slope: [f32; 2],
    /// How far from the centre's depth it may be: three sigmas along the view.
    thick: f32,
    /// 0 for a splat too thin to see.
    opacity: f32,
    splat: u32,
}

/// Where the splat `s` falls in `view`, if it shows at all.
fn footprint(s: &Splat, frame: &Frame, view: &View, splat: u32) -> Footprint {
    let inv_pixel = 1.0 / frame.pixel;
    let d = s.centre - frame.centre;
    // The covariance seen along the view, in clay units.
    let sr = s.times(view.right);
    let su = s.times(view.up);
    let (rr, ru, uu) = (sr.dot(view.right), sr.dot(view.up), su.dot(view.up));
    let (rf, uf) = (sr.dot(view.forward), su.dot(view.forward));
    let ff = s.times(view.forward).dot(view.forward);
    let ip2 = inv_pixel * inv_pixel;
    let a = rr * ip2 + DILATE;
    let b = -ru * ip2;
    let c = uu * ip2 + DILATE;
    let det = a * c - b * b;
    if det.is_nan() || det <= 1e-12 {
        return Footprint::default();
    }
    let mid = 0.5 * (a + c);
    let spread = (mid * mid - det).max(0.0).sqrt();
    // The splat's depth across its footprint: the mean depth of the
    // Gaussian along each ray, a plane through its centre.
    let det_w = rr * uu - ru * ru;
    let slope = if det_w > 1e-20 {
        [
            (rf * uu - uf * ru) / det_w * frame.pixel,
            -(uf * rr - rf * ru) / det_w * frame.pixel,
        ]
    } else {
        [0.0; 2]
    };
    Footprint {
        x: (d.dot(view.right) + frame.radius) * inv_pixel,
        y: (frame.radius - d.dot(view.up)) * inv_pixel,
        conic: [c / det, -b / det, a / det],
        // Out to where it fades below 1/255, at most three sigmas.
        radius: (2.0 * (255.0 * s.opacity).ln()).sqrt().min(3.0) * (mid + spread).sqrt(),
        depth: d.dot(view.forward) + frame.near,
        slope,
        thick: 3.0 * ff.max(0.0).sqrt(),
        opacity: s.opacity,
        splat,
    }
}

/// Draws the splats seen along `view`, front to back: each pixel keeps
/// the depth where its ray has gathered `level` opacity, and the splat
/// there. `sides` gathers, per splat, how much of it the view saw times
/// which side (along its normal or against it), and how much in all.
fn draw(
    splats: &[Splat],
    normals: &[Vec3],
    frame: &Frame,
    view: &View,
    level: f32,
    sides: &mut [[f32; 2]],
    scratch: &mut Scratch,
) -> Depths {
    let size = frame.size;
    let n = size * size;
    let mut depth = vec![f32::NAN; n];
    let mut splat_at = vec![u32::MAX; n];
    let light = &mut scratch.light;
    light.clear();
    light.resize(n, 1.0);
    let span = frame.near + frame.radius;

    // Front to back, by a counting sort on the depth. The splats are read
    // in their own order and their footprints written in the drawing
    // order, so drawing reads memory in order.
    let slots = &mut scratch.slots;
    slots.clear();
    slots.extend(splats.iter().map(|s| {
        let d = s.centre - frame.centre;
        let z = d.dot(view.forward) + frame.near;
        let beside = d.dot(view.right).abs().max(d.dot(view.up).abs());
        if (0.0..span).contains(&z) && beside < frame.radius * 1.5 {
            (z / span * 65_535.0) as u32
        } else {
            u32::MAX
        }
    }));
    let counts = &mut scratch.counts;
    counts.clear();
    counts.resize(1 << 16, 0);
    for &k in slots.iter() {
        if k != u32::MAX {
            counts[k as usize] += 1;
        }
    }
    let mut start = 0;
    for c in counts.iter_mut() {
        let count = *c;
        *c = start;
        start += count;
    }
    let footprints = &mut scratch.footprints;
    footprints.clear();
    footprints.resize(start as usize, Footprint::default());
    for (i, (s, slot)) in splats.iter().zip(slots.iter_mut()).enumerate() {
        if *slot != u32::MAX {
            let at = &mut counts[*slot as usize];
            footprints[*at as usize] = footprint(s, frame, view, i as u32);
            *slot = *at;
            *at += 1;
        }
    }

    // Besides the median depth, where the ray has gathered half as much
    // and where half of what remains is gone: how deep the surface's fuzz
    // runs, which sets how far this view's depth can be trusted.
    let through = 1.0 - level;
    let near = 1.0 - 0.5 * level;
    let far = 0.5 * through;
    let first = &mut scratch.first;
    first.clear();
    first.resize(n, f32::NAN);
    let last = &mut scratch.last;
    last.clear();
    last.resize(n, f32::NAN);
    let seen_all = &mut scratch.seen;
    seen_all.clear();
    seen_all.resize(footprints.len(), 0.0);
    // How many pixels of each tile still let light through: splats over
    // tiles that are all opaque are hidden, and skipped whole.
    let tiles = size.div_ceil(TILE);
    let open = &mut scratch.open;
    open.clear();
    open.extend((0..tiles * tiles).map(|t| {
        let w = (size - (t % tiles) * TILE).min(TILE);
        let h = (size - (t / tiles) * TILE).min(TILE);
        (w * h) as u16
    }));
    for (f, seen_out) in footprints.iter().zip(seen_all.iter_mut()) {
        if f.opacity == 0.0 {
            continue;
        }
        let r = f.radius;
        let edge = size as i32 - 1;
        let x0 = (-floor(r + 0.5 - f.x)).max(0) as usize;
        let y0 = (-floor(r + 0.5 - f.y)).max(0) as usize;
        let x1 = floor(f.x + r - 0.5).min(edge) as isize;
        let y1 = floor(f.y + r - 0.5).min(edge) as isize;
        if x1 < x0 as isize || y1 < y0 as isize {
            continue;
        }
        let (x1, y1) = (x1 as usize, y1 as usize);
        let hidden = (y0 / TILE..=y1 / TILE)
            .all(|ty| (x0 / TILE..=x1 / TILE).all(|tx| open[ty * tiles + tx] == 0));
        if hidden {
            continue;
        }
        let [ca, cb, cc] = f.conic;
        let mut seen = 0.0;
        // Tile by tile, skipping the opaque ones: a large splat behind
        // the surface costs little.
        for ty in y0 / TILE..=y1 / TILE {
            for tx in x0 / TILE..=x1 / TILE {
                if open[ty * tiles + tx] == 0 {
                    continue;
                }
                for yy in y0.max(ty * TILE)..=y1.min(ty * TILE + TILE - 1) {
                    let dy = yy as f32 + 0.5 - f.y;
                    let row = yy * size;
                    for xx in x0.max(tx * TILE)..=x1.min(tx * TILE + TILE - 1) {
                        let p = row + xx;
                        let t = light[p];
                        if t < OPAQUE {
                            continue;
                        }
                        let dx = xx as f32 + 0.5 - f.x;
                        let power = -0.5 * (ca * dx * dx + cc * dy * dy) - cb * dx * dy;
                        if power < -4.5 {
                            continue;
                        }
                        let alpha = f.opacity * fast_exp(power);
                        if alpha < 1.0 / 255.0 {
                            continue;
                        }
                        let after = t * (1.0 - alpha);
                        if t > far && after <= near {
                            let offset = f.slope[0] * dx + f.slope[1] * dy;
                            let z = f.depth + offset.clamp(-f.thick, f.thick);
                            if t > near {
                                first[p] = z;
                            }
                            if t > through && after <= through {
                                depth[p] = z;
                                splat_at[p] = f.splat;
                            }
                            if after <= far {
                                last[p] = z;
                            }
                        }
                        seen += alpha * t;
                        light[p] = after;
                        if after < OPAQUE {
                            open[ty * tiles + tx] -= 1;
                        }
                    }
                }
            }
        }
        *seen_out = seen;
    }
    for ((n, &slot), side) in normals.iter().zip(slots.iter()).zip(sides.iter_mut()) {
        if slot != u32::MAX {
            let seen = seen_all[slot as usize];
            if seen > 0.0 {
                let facing = n.dot(view.forward);
                side[0] += seen * facing;
                side[1] += seen * facing.abs();
            }
        }
    }
    let empty = 1.0 - EMPTY * level;
    let mut fuzz = vec![0.0; n];
    for p in 0..n {
        if depth[p].is_nan() {
            if light[p] > empty {
                depth[p] = f32::INFINITY;
            }
        } else if last[p].is_finite() {
            fuzz[p] = last[p] - first[p];
        } else {
            fuzz[p] = 2.0 * (depth[p] - first[p]);
        }
    }
    Depths {
        depth,
        fuzz,
        weight: Vec::new(),
        splat: splat_at,
    }
}

/// e^x for x from about -87 to 0, to about 1e-4: two to the power of the
/// whole part by the float's exponent bits, of the rest by a cubic. The
/// library's `exp` is several times slower, in the browser most of all.
fn fast_exp(x: f32) -> f32 {
    let y = (x * std::f32::consts::LOG2_E).max(-126.0);
    let whole = floor(y);
    let f = y - whole as f32;
    let p = 1.0 + f * (0.695_833_6 + f * (0.226_067_2 + f * 0.078_024_52));
    f32::from_bits(((whole + 127) as u32) << 23) * p
}

/// Replaces each depth on a surface by the median of those around it
/// (3 × 3, on a surface), taking out single stray depths and the grain of
/// the splats.
fn median(depth: &mut [f32], size: usize) {
    let before = depth.to_vec();
    let mut around = Vec::with_capacity(9);
    for y in 1..size.saturating_sub(1) {
        for x in 1..size - 1 {
            let p = y * size + x;
            if !before[p].is_finite() {
                continue;
            }
            around.clear();
            for dy in [size.wrapping_neg(), 0, size] {
                for dx in [usize::MAX, 0, 1] {
                    let d = before[p.wrapping_add(dy).wrapping_add(dx)];
                    if d.is_finite() {
                        around.push(d);
                    }
                }
            }
            if around.len() >= 5 {
                let mid = around.len() / 2;
                let (_, &mut m, _) = around.select_nth_unstable_by(mid, f32::total_cmp);
                depth[p] = m;
            }
        }
    }
}

/// The whole number at or below `v`, for `v` well inside i32's range.
/// `f32::floor` is a library call on the x86-64 baseline.
fn floor(v: f32) -> i32 {
    let i = v as i32;
    if (i as f32) > v { i - 1 } else { i }
}

/// Turns the splat at each pixel into the pixel's weight: how squarely
/// the view meets the surface there, from the depths around it, negative
/// where it meets the surface's inner side.
fn weigh(
    map: &mut Depths,
    frame: &Frame,
    view: &View,
    normals: &[Vec3],
    outer: &[i8],
    truncation: f32,
) {
    median(&mut map.depth, frame.size);
    let size = frame.size;
    let depth = &map.depth;
    let hit = |x: usize, y: usize| {
        let d = depth[y * size + x];
        d.is_finite().then_some(d)
    };
    // The slope of the depth across one neighbour pair, in depth per
    // clay unit; `None` if neither side is on a surface.
    let slope = |at: f32, before: Option<f32>, after: Option<f32>| -> Option<f32> {
        match (before, after) {
            (Some(b), Some(a)) => Some((a - b) / (2.0 * frame.pixel)),
            (Some(b), None) => Some((at - b) / frame.pixel),
            (None, Some(a)) => Some((a - at) / frame.pixel),
            (None, None) => None,
        }
    };
    let mut weight = vec![0.0; size * size];
    for y in 0..size {
        for x in 0..size {
            let p = y * size + x;
            let Some(d) = hit(x, y) else {
                continue;
            };
            let s = map.splat[p] as usize;
            let left = (x > 0).then(|| hit(x - 1, y)).flatten();
            let right = (x + 1 < size).then(|| hit(x + 1, y)).flatten();
            let above = (y > 0).then(|| hit(x, y - 1)).flatten();
            let below = (y + 1 < size).then(|| hit(x, y + 1)).flatten();
            let facing = normals[s].dot(view.forward);
            let square = match (slope(d, left, right), slope(d, above, below)) {
                (Some(sx), Some(sy)) => 1.0 / (1.0 + sx * sx + sy * sy).sqrt(),
                _ => facing.abs(),
            };
            let inner = f32::from(outer[s]) * facing > 0.0;
            // A fuzzy surface is trusted over a longer stretch of the
            // ray, and less.
            let reach = (FUZZ * map.fuzz[p]).clamp(truncation, MAX_REACH * truncation);
            map.fuzz[p] = reach;
            let w = square.max(0.05) * truncation / reach;
            weight[p] = if inner { -w } else { w };
        }
    }
    map.weight = weight;
    map.splat = Vec::new();
}

/// Fuses the views into `g`: the signed distance, truncated at
/// `truncation`, where views saw a surface near a node; outside where a
/// view saw through it; inside where no view saw into it.
fn fuse(g: &mut Grid, frame: &Frame, views: &[View], maps: &[Depths], truncation: f32) {
    let [nx, ny, _] = g.dims;
    let slab = nx * ny;
    let origin = g.origin;
    let step = g.step;
    let inv_pixel = 1.0 / frame.pixel;
    let size = frame.size as isize;
    let layers: Vec<(usize, &mut [f32])> = g.values.chunks_mut(slab).enumerate().collect();
    parallel_mut(layers, |(k, values)| {
        let mut sum = vec![0f32; slab];
        let mut weight = vec![0f32; slab];
        let mut free = vec![0u16; slab];
        for (view, map) in views.iter().zip(maps) {
            for j in 0..ny {
                let start = origin + Vec3::new(0.0, j as f32, *k as f32) * step - frame.centre;
                let x0 = (start.dot(view.right) + frame.radius) * inv_pixel;
                let y0 = (frame.radius - start.dot(view.up)) * inv_pixel;
                let z0 = start.dot(view.forward) + frame.near;
                let (dx, dy, dz) = (
                    view.right.x * step * inv_pixel,
                    -view.up.x * step * inv_pixel,
                    view.forward.x * step,
                );
                for i in 0..nx {
                    let fi = i as f32;
                    let px = (x0 + dx * fi) as isize;
                    let py = (y0 + dy * fi) as isize;
                    if px < 0 || py < 0 || px >= size || py >= size {
                        continue;
                    }
                    let p = (py * size + px) as usize;
                    let d = map.depth[p];
                    let at = j * nx + i;
                    if d == f32::INFINITY {
                        free[at] += 1;
                        continue;
                    }
                    if d.is_nan() {
                        continue;
                    }
                    let z = z0 + dz * fi;
                    let w = map.weight[p];
                    let reach = map.fuzz[p];
                    if w > 0.0 {
                        let sdf = d - z;
                        if sdf > reach {
                            free[at] += 1;
                        } else if sdf > -reach {
                            sum[at] += w * sdf.clamp(-truncation, truncation);
                            weight[at] += w;
                        }
                    } else {
                        let sdf = z - d;
                        if sdf.abs() < reach {
                            sum[at] -= w * sdf.clamp(-truncation, truncation);
                            weight[at] -= w;
                        }
                    }
                }
            }
        }
        for at in 0..slab {
            let free = FREE * f32::from(free[at]);
            values[at] = if weight[at] + free > 0.0 {
                ((sum[at] + free * truncation) / (weight[at] + free)).clamp(-truncation, truncation)
            } else {
                -truncation
            };
        }
    });
}

/// Cuts the solid in `g` by the crop box (in file coordinates, placed by
/// `align`), so the cut is a flat wall.
fn wall(g: &mut Grid, align: &Alignment, crop: Aabb, truncation: f32) {
    let to_file = align.matrix().inverse();
    let scale = align.scale;
    let centre = crop.centre();
    let half = crop.size() * 0.5;
    for at in 0..g.len() {
        let [i, j, k] = g.coords(at);
        let p = to_file.transform_point3(g.point(i, j, k));
        let q = (p - centre).abs() - half;
        let outside = q.max(Vec3::ZERO).length() + q.max_element().min(0.0);
        let d = (outside * scale).clamp(-truncation, truncation);
        let v = &mut g.values[at];
        *v = v.max(d);
    }
}

/// One solid piece: the border outside, gaps up to about twice
/// [`CLOSE`] nodes wide sealed, hollows filled and floating bits dropped;
/// the values follow, just across zero where the shape changed.
fn settle(g: &mut Grid, truncation: f32, close: usize) {
    for at in g.border() {
        g.values[at] = truncation;
    }
    let wall: Vec<bool> = g.values.iter().map(|&v| v < 0.0).collect();
    let out = grid::outside(g, &wall, close);
    let mut solid: Vec<bool> = out.iter().map(|&o| !o).collect();
    // Sealing may block the outside from part of the border; the border
    // is outside all the same.
    for at in g.border() {
        solid[at] = false;
    }
    grid::keep_largest(g, &mut solid);
    let nudge = g.step * 0.05;
    for (v, &inside) in g.values.iter_mut().zip(&solid) {
        *v = if inside { v.min(-nudge) } else { v.max(nudge) };
    }
}

/// How many threads to share work between: one on the web.
fn threads() -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        1
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::available_parallelism().map_or(1, |n| n.get().min(8))
    }
}

/// `f` over `items` cut into one run per thread, in order.
fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&[T]) -> R + Sync) -> Vec<R> {
    let n = threads().min(items.len()).max(1);
    if n == 1 {
        return vec![f(items)];
    }
    let per = items.len().div_ceil(n);
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(per)
            .map(|chunk| {
                let f = &f;
                scope.spawn(move || f(chunk))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("a fusion thread panicked"))
            .collect()
    })
}

/// `f` on each of `items`, shared out between the threads.
fn parallel_mut<T: Send>(items: Vec<T>, f: impl Fn(&mut T) + Sync) {
    let n = threads().min(items.len()).max(1);
    if n == 1 {
        items.into_iter().for_each(|mut item| f(&mut item));
        return;
    }
    let per = items.len().div_ceil(n);
    let mut items = items;
    std::thread::scope(|scope| {
        for chunk in items.chunks_mut(per) {
            let f = &f;
            scope.spawn(move || chunk.iter_mut().for_each(f));
        }
    });
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use glam::Quat;

    use super::*;
    use crate::reference::{Content, Reference, Source};
    use crate::sculpt::Mesh;
    use crate::voxel::clay_from_reference;

    /// Flat splats (discs `size` across, a tenth as thick) on the points
    /// `surface` gives, each facing its normal.
    fn discs(surface: &[(Vec3, Vec3)], size: f32, opacity: u8) -> Splats {
        let mut s = Splats::default();
        for &(p, n) in surface {
            s.positions.push(p);
            s.scales.push(Vec3::new(size, size, size * 0.1));
            s.rotations
                .push(Quat::from_rotation_arc(Vec3::Z, n.normalize()));
            s.colors.push([180, 170, 160, opacity]);
        }
        s
    }

    /// `n` points spread evenly over a sphere, with their normals.
    fn sphere(n: usize, centre: Vec3, radius: f32) -> Vec<(Vec3, Vec3)> {
        let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
        (0..n)
            .map(|i| {
                let y = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
                let r = (1.0 - y * y).sqrt();
                let a = golden * i as f32;
                let d = Vec3::new(r * a.cos(), y, r * a.sin());
                (centre + d * radius, d)
            })
            .collect()
    }

    fn reference(splats: Splats) -> Reference {
        Reference::new(
            Source {
                name: "made here".into(),
                bytes: Arc::from([]),
            },
            Content::Splats(splats),
        )
    }

    /// Whether every edge joins two triangles wound the same way and the
    /// triangles around every vertex make one fan; the pieces; the genus.
    fn topology(mesh: &Mesh) -> (bool, usize, i64) {
        let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
        let mut fans: HashMap<u32, HashMap<u32, u32>> = HashMap::new();
        for &[a, b, c] in &mesh.triangles {
            for (x, y, z) in [(a, b, c), (b, c, a), (c, a, b)] {
                *edges.entry((x, y)).or_default() += 1;
                fans.entry(x).or_default().insert(y, z);
            }
        }
        let edges_ok = edges
            .iter()
            .all(|(&(a, b), &n)| n == 1 && edges.get(&(b, a)) == Some(&1));
        let fans_ok = fans.values().all(|next| {
            let start = *next.keys().next().unwrap();
            let (mut at, mut steps) = (start, 0);
            loop {
                let Some(&to) = next.get(&at) else {
                    return false;
                };
                at = to;
                steps += 1;
                if at == start {
                    return steps == next.len();
                }
                if steps > next.len() {
                    return false;
                }
            }
        });
        let mut parent: Vec<u32> = (0..mesh.positions.len() as u32).collect();
        fn root(p: &mut [u32], mut v: u32) -> u32 {
            while p[v as usize] != v {
                p[v as usize] = p[p[v as usize] as usize];
                v = p[v as usize];
            }
            v
        }
        for &[a, b, c] in &mesh.triangles {
            for (x, y) in [(a, b), (b, c)] {
                let (rx, ry) = (root(&mut parent, x), root(&mut parent, y));
                parent[rx as usize] = ry;
            }
        }
        let mut roots: Vec<u32> = mesh
            .triangles
            .iter()
            .map(|t| root(&mut parent, t[0]))
            .collect();
        roots.sort_unstable();
        roots.dedup();
        let (v, f) = (mesh.positions.len() as i64, mesh.triangles.len() as i64);
        let chi = v - f * 3 / 2 + f;
        (edges_ok && fans_ok, roots.len(), (2 - chi) / 2)
    }

    /// The clay from `reference`, checked to be one closed, manifold
    /// piece; and its genus.
    fn clay(reference: &Reference, resolution: u32) -> (Mesh, i64) {
        let mesh = clay_from_reference(reference, crate::voxel::tests::at(resolution)).unwrap();
        let (manifold, pieces, genus) = topology(&mesh);
        assert!(manifold, "not closed and manifold");
        assert_eq!(pieces, 1);
        (mesh, genus)
    }

    #[test]
    fn a_shell_of_splats_becomes_a_solid_ball() {
        let splats = discs(&sphere(6000, Vec3::ZERO, 0.8), 0.03, 200);
        let (mesh, genus) = clay(&reference(splats), 48);
        assert_eq!(genus, 0);
        for p in &mesh.positions {
            assert!((p.length() - 0.8).abs() < 0.05, "{p}");
        }
    }

    #[test]
    fn a_capture_of_only_the_front_is_solid_behind_it() {
        // The front half of a ball: no splats behind it.
        let front: Vec<_> = sphere(8000, Vec3::ZERO, 0.8)
            .into_iter()
            .filter(|(p, _)| p.z > 0.0)
            .collect();
        let (mesh, genus) = clay(&reference(discs(&front, 0.03, 200)), 48);
        assert_eq!(genus, 0);
        // Where it was seen, the surface is the ball's...
        let seen: Vec<&Vec3> = mesh.positions.iter().filter(|p| p.z > 0.15).collect();
        assert!(seen.len() > 300, "{}", seen.len());
        for p in seen {
            assert!((p.length() - 0.8).abs() < 0.05, "{p}");
        }
        // ...and there is no inner wall: the clay is solid behind it.
        assert!(mesh.positions.iter().all(|p| p.length() > 0.7 || p.z < 0.1));
    }

    #[test]
    fn a_torus_keeps_its_hole() {
        let (big, small) = (0.6, 0.2);
        let mut surface = Vec::new();
        for i in 0..240 {
            for j in 0..60 {
                let u = i as f32 / 240.0 * std::f32::consts::TAU;
                let v = j as f32 / 60.0 * std::f32::consts::TAU;
                let ring = Vec3::new(u.cos(), 0.0, u.sin());
                let n = ring * v.cos() + Vec3::Y * v.sin();
                surface.push((ring * big + n * small, n));
            }
        }
        let (mesh, genus) = clay(&reference(discs(&surface, 0.02, 220)), 64);
        assert_eq!(genus, 1);
        for p in &mesh.positions {
            let ring = Vec3::new(p.x, 0.0, p.z).normalize() * big;
            assert!((p.distance(ring) - small).abs() < 0.05, "{p}");
        }
    }

    /// Splats over the faces of the box from -h to h, but the one facing
    /// +z if `open`.
    fn box_surface(h: f32, open: bool) -> Vec<(Vec3, Vec3)> {
        let mut surface = Vec::new();
        let n = 50;
        for axis in 0..3 {
            for sign in [-1.0, 1.0] {
                if open && axis == 2 && sign > 0.0 {
                    continue;
                }
                let mut normal = Vec3::ZERO;
                normal[axis] = sign;
                for i in 0..n {
                    for j in 0..n {
                        let u = (i as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                        let v = (j as f32 + 0.5) / n as f32 * 2.0 - 1.0;
                        let mut p = normal * h;
                        p[(axis + 1) % 3] = u * h;
                        p[(axis + 2) % 3] = v * h;
                        surface.push((p, normal));
                    }
                }
            }
        }
        surface
    }

    #[test]
    fn a_box_missing_a_face_is_solid() {
        let (mesh, genus) = clay(&reference(discs(&box_surface(0.6, true), 0.02, 220)), 48);
        assert_eq!(genus, 0);
        // No walls inside: every vertex is near the box's outside.
        for p in &mesh.positions {
            assert!(p.abs().max_element() > 0.5, "{p}");
        }
    }

    /// A number in 0..1 that looks random, from a point.
    fn rand_like(p: Vec3) -> f32 {
        ((p.x * 12.9898 + p.y * 78.233 + p.z * 37.719).sin() * 43_758.547)
            .fract()
            .abs()
    }

    #[test]
    fn sparse_and_faint_patches_leave_no_holes() {
        // A ball whose top is sparse and whose side is faint.
        let surface: Vec<_> = sphere(6000, Vec3::ZERO, 0.8)
            .into_iter()
            .filter(|(p, _)| p.y < 0.5 || rand_like(*p) < 0.3)
            .collect();
        let mut splats = discs(&surface, 0.03, 200);
        for (p, c) in splats.positions.iter().zip(&mut splats.colors) {
            if p.x > 0.5 {
                c[3] = 60;
            }
        }
        let (mesh, genus) = clay(&reference(splats), 48);
        assert_eq!(genus, 0);
        // Solid: nothing deep inside.
        assert!(mesh.positions.iter().all(|p| p.length() > 0.6));
    }

    #[test]
    fn a_crop_through_the_shape_closes_it_with_a_flat_face() {
        let mut reference = reference(discs(&sphere(6000, Vec3::ZERO, 0.8), 0.03, 200));
        reference.crop = Some(Aabb {
            min: Vec3::splat(-2.0),
            max: Vec3::new(2.0, 0.3, 2.0),
        });
        let (mesh, genus) = clay(&reference, 48);
        assert_eq!(genus, 0);
        let step = 1.6 / 48.0;
        assert!(
            mesh.positions.iter().all(|p| p.y < 0.3 + step),
            "past the cut"
        );
        // The cut is a flat disc across the ball, not a ring.
        let cap = |lo: f32, hi: f32| {
            mesh.positions
                .iter()
                .filter(|p| (p.y - 0.3).abs() < step * 0.25 && (lo..hi).contains(&p.x.hypot(p.z)))
                .count()
        };
        assert!(cap(0.0, 0.4) > 50, "{}", cap(0.0, 0.4));
        // Elsewhere it follows the ball.
        for p in mesh.positions.iter().filter(|p| p.y < 0.2) {
            assert!((p.length() - 0.8).abs() < 0.05, "{p}");
        }
    }

    /// How bumpy a ball is: the spread of its vertices' distances from
    /// the centre.
    fn roughness(mesh: &Mesh) -> f32 {
        let n = mesh.positions.len() as f32;
        let mean = mesh.positions.iter().map(|p| p.length()).sum::<f32>() / n;
        let var = mesh
            .positions
            .iter()
            .map(|p| (p.length() - mean).powi(2))
            .sum::<f32>()
            / n;
        var.sqrt()
    }

    #[test]
    fn more_smoothness_gives_a_smoother_closed_mesh() {
        // A ball with a bumpy skin.
        let surface: Vec<_> = sphere(8000, Vec3::ZERO, 0.8)
            .into_iter()
            .map(|(p, n)| {
                let bump = (p * 40.0).to_array().map(f32::sin);
                (p + n * 0.02 * bump[0] * bump[1] * bump[2], n)
            })
            .collect();
        let reference = reference(discs(&surface, 0.03, 200));
        let mut last = f32::MAX;
        for smoothness in [0, 2, 6] {
            let how = crate::voxel::FromReference {
                resolution: 64,
                smoothness,
                ..Default::default()
            };
            let mesh = clay_from_reference(&reference, how).unwrap();
            let (manifold, pieces, genus) = topology(&mesh);
            assert!(manifold && pieces == 1 && genus == 0, "{smoothness}");
            let rough = roughness(&mesh);
            assert!(rough < last * 0.95, "{smoothness}: {rough} after {last}");
            last = rough;
        }
    }

    #[test]
    fn the_shortest_axis_of_a_flat_spread() {
        let axis = shortest_axis([1.0, 0.2, 0.0, 0.5, 0.0, 0.01]);
        assert!(axis.dot(Vec3::Z).abs() > 0.99, "{axis}");
        let turned = Quat::from_rotation_x(0.7) * Quat::from_rotation_y(0.3);
        let m = Mat3::from_quat(turned) * Mat3::from_diagonal(Vec3::new(1.0, 0.6, 0.05));
        let c = m * m.transpose();
        let axis = shortest_axis([
            c.x_axis.x, c.y_axis.x, c.z_axis.x, c.y_axis.y, c.z_axis.y, c.z_axis.z,
        ]);
        assert!(axis.dot(turned * Vec3::Z).abs() > 0.99, "{axis}");
    }

    #[test]
    fn fast_exp_is_close() {
        for i in 0..200 {
            let x = -(i as f32) * 0.05;
            assert!((fast_exp(x) - x.exp()).abs() < 2e-4, "{x}");
        }
    }
}
