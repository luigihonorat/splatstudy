// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Cropping a mesh reference to a closed shape.
//!
//! The crop box's six planes cut the triangles exactly, one plane after
//! the other, and each cut is closed with a flat cap before the next plane
//! cuts. The next plane then cuts that cap too, so caps meet along the
//! box's edges and corners, and a box wholly inside the shape comes out as
//! the box itself.
//!
//! A cap is made of the loops the cut leaves open on its plane: each new
//! point on an edge is shared by the two triangles of that edge, so the
//! loops are connected. Loops inside an odd number of others are holes
//! (a hollow cast cut across); the rest are filled, each with the holes
//! directly inside it, by ear clipping. Scans are often open, and a cut
//! through an opening leaves a chain that doesn't close: it is left open.
//!
//! Only triangles that reach one of the planes (infinite, not just the
//! box's faces) are cut; the many wholly inside are kept as they are, and
//! the rest dropped, in a single pass over the mesh.

use std::collections::HashMap;

use glam::{DVec2, Vec3};

use super::{Aabb, RefMesh};

/// A mesh cut by a crop box, closed with caps where it was closed.
#[derive(Clone, Debug, Default)]
pub struct Cropped {
    /// The mesh's own vertices at their own indices (some now unused),
    /// then the new ones on the box's faces.
    pub positions: Vec<Vec3>,
    /// The surface kept, then the caps from `caps` on.
    pub triangles: Vec<[u32; 3]>,
    /// One per position, when the mesh has colours; new points take the
    /// colour between the two they are cut between.
    pub colors: Option<Vec<[u8; 4]>>,
    /// Where the caps start in `triangles`.
    pub caps: usize,
    /// Each cap triangle's box face: 0..6 for -x, +x, -y, +y, -z, +z.
    pub cap_faces: Vec<u8>,
}

impl Cropped {
    /// The whole mesh, uncut.
    pub fn whole(mesh: &RefMesh) -> Self {
        Self {
            positions: mesh.positions.clone(),
            triangles: mesh.triangles.clone(),
            colors: mesh.colors.clone(),
            caps: mesh.triangles.len(),
            cap_faces: Vec::new(),
        }
    }

    /// The caps on vertices of their own, so that smooth normals made
    /// from the triangles are flat on them and the surface's own don't
    /// bend towards them. The shape stays closed only in position.
    pub fn with_flat_caps(mut self) -> Self {
        let mut copies: HashMap<(u8, u32), u32> = HashMap::new();
        let caps = &mut self.triangles[self.caps..];
        for (t, &face) in caps.iter_mut().zip(&self.cap_faces) {
            for v in t {
                *v = *copies.entry((face, *v)).or_insert_with(|| {
                    let i = *v as usize;
                    self.positions.push(self.positions[i]);
                    if let Some(colors) = &mut self.colors {
                        colors.push(colors[i]);
                    }
                    (self.positions.len() - 1) as u32
                });
            }
        }
        self
    }
}

/// Where a point is from a plane: on it within a small tolerance.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    In,
    On,
    Out,
}

/// One face of the crop box: what is past it is cut away.
#[derive(Clone, Copy)]
struct Plane {
    axis: usize,
    /// The box's larger side on `axis`: what is above is out.
    max: bool,
    at: f32,
    /// Points nearer than this are on the plane.
    eps: f32,
}

impl Plane {
    /// The six faces, in the order of [`Cropped::cap_faces`].
    fn of(b: Aabb) -> [Plane; 6] {
        let scale = b
            .min
            .abs()
            .max(b.max.abs())
            .max(b.max - b.min)
            .max_element();
        let eps = if scale.is_finite() { scale * 1e-6 } else { 0.0 };
        std::array::from_fn(|k| {
            let axis = k / 2;
            let max = k % 2 == 1;
            Plane {
                axis,
                max,
                at: if max { b.max[axis] } else { b.min[axis] },
                eps,
            }
        })
    }

    /// Positive outside.
    fn distance(&self, p: Vec3) -> f32 {
        if self.max {
            p[self.axis] - self.at
        } else {
            self.at - p[self.axis]
        }
    }

    /// Points that aren't finite are out.
    fn side(&self, p: Vec3) -> Side {
        let d = self.distance(p);
        if d < -self.eps {
            Side::In
        } else if d <= self.eps {
            Side::On
        } else {
            Side::Out
        }
    }

    /// `p` on the plane, seen from outside: anticlockwise here is
    /// anticlockwise seen from outside the box.
    fn uv(&self, p: Vec3) -> DVec2 {
        let (u, v) = match self.axis {
            0 => (p.y, p.z),
            1 => (p.z, p.x),
            _ => (p.x, p.y),
        };
        let (u, v) = (f64::from(u), f64::from(v));
        if self.max {
            DVec2::new(u, v)
        } else {
            DVec2::new(v, u)
        }
    }
}

/// `mesh` cut by `crop` (in the mesh's coordinates), closed where the cuts
/// close. Never panics: triangles with indices out of range are dropped.
pub fn crop(mesh: &RefMesh, crop: Aabb) -> Cropped {
    let planes = Plane::of(crop);
    let n = mesh.positions.len();
    let (mut kept, cut) = classify(mesh, &planes);
    let mut work = Work {
        positions: Vec::new(),
        colors: None,
        faces: vec![NO_FACE; cut.len()],
        triangles: cut,
    };
    if !work.triangles.is_empty() {
        work.positions = mesh.positions.clone();
        work.colors = mesh.colors.clone().filter(|c| c.len() == n);
        for (k, plane) in planes.iter().enumerate() {
            work.cut(*plane, k as u8);
        }
    }
    let Work {
        mut positions,
        mut colors,
        triangles,
        faces,
    } = work;
    if positions.is_empty() {
        positions = mesh.positions.clone();
        colors = mesh.colors.clone().filter(|c| c.len() == n);
    }
    kept.extend(
        triangles
            .iter()
            .zip(&faces)
            .filter(|(_, f)| **f == NO_FACE)
            .map(|(t, _)| *t),
    );
    let caps = kept.len();
    let mut cap_faces = Vec::new();
    for (t, &f) in triangles.iter().zip(&faces) {
        if f != NO_FACE {
            kept.push(*t);
            cap_faces.push(f);
        }
    }
    Cropped {
        positions,
        triangles: kept,
        colors,
        caps,
        cap_faces,
    }
}

/// The triangles wholly inside the box, kept as they are, and those that
/// reach one of its planes, to cut. A triangle wholly past one plane yet
/// crossing another is cut too: it closes that other plane's loop before
/// it is dropped. The rest have each plane wholly on one side, one of them
/// wholly past: dropped.
fn classify(mesh: &RefMesh, planes: &[Plane; 6]) -> (Vec<[u32; 3]>, Vec<[u32; 3]>) {
    // Per vertex, a bit per plane: past it, and not inside it.
    let codes: Vec<[u8; 2]> = mesh
        .positions
        .iter()
        .map(|&p| {
            let (mut out, mut not_in) = (0u8, 0u8);
            for (k, plane) in planes.iter().enumerate() {
                let side = plane.side(p);
                // Not finite: out.
                out |= u8::from(side == Side::Out) << k;
                not_in |= u8::from(side != Side::In) << k;
            }
            [out, not_in]
        })
        .collect();
    let mut kept = Vec::with_capacity(mesh.triangles.len());
    let mut cut = Vec::new();
    for &t in &mesh.triangles {
        let [Some(&a), Some(&b), Some(&c)] = t.map(|i| codes.get(i as usize)) else {
            continue;
        };
        let not_in = a[1] | b[1] | c[1];
        if not_in == 0 {
            kept.push(t);
        } else if not_in & !(a[0] & b[0] & c[0]) != 0 {
            cut.push(t);
        }
    }
    (kept, cut)
}

/// The face of a triangle that is no cap.
const NO_FACE: u8 = u8::MAX;

/// The triangles still to cut, with the points made so far.
struct Work {
    positions: Vec<Vec3>,
    colors: Option<Vec<[u8; 4]>>,
    triangles: Vec<[u32; 3]>,
    /// Per triangle: the face whose cap it is part of, or [`NO_FACE`].
    faces: Vec<u8>,
}

impl Work {
    /// Cuts away what is past `plane` and caps the cut as face `face`.
    fn cut(&mut self, plane: Plane, face: u8) {
        let mut triangles = Vec::with_capacity(self.triangles.len());
        let mut faces = Vec::with_capacity(self.triangles.len());
        let mut made: HashMap<(u32, u32), u32> = HashMap::new();
        // Edges on the plane, as the triangles kept go round them.
        let mut rims: Vec<(u32, u32)> = Vec::new();
        let old = std::mem::take(&mut self.triangles);
        let old_faces = std::mem::take(&mut self.faces);
        for (t, &f) in old.iter().zip(&old_faces) {
            let s = t.map(|i| plane.side(self.positions[i as usize]));
            // Lying on the plane, as a scan's flat base on a box fitted to
            // it: kept, as the surface there. Its edges meet its
            // neighbours' the other way round, so no cap covers it again.
            let flat = s == [Side::On; 3];
            if !flat && !s.contains(&Side::In) {
                continue;
            }
            // The kept part, at most four corners, each with whether it
            // is on the plane.
            let mut poly = [(0u32, false); 4];
            let mut len = 0;
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                let (sa, sb) = (s[e], s[(e + 1) % 3]);
                if sa != Side::Out && len < 4 {
                    poly[len] = (a, sa == Side::On);
                    len += 1;
                }
                let crosses = matches!((sa, sb), (Side::In, Side::Out) | (Side::Out, Side::In));
                if crosses && len < 4 {
                    poly[len] = (self.split(a, b, plane, &mut made), true);
                    len += 1;
                }
            }
            for j in 1..len.saturating_sub(1) {
                let corners = [poly[0], poly[j], poly[j + 1]];
                if !flat && corners.iter().all(|c| c.1) {
                    continue;
                }
                for e in 0..3 {
                    let (a, b) = (corners[e], corners[(e + 1) % 3]);
                    if a.1 && b.1 {
                        rims.push((a.0, b.0));
                    }
                }
                triangles.push(corners.map(|c| c.0));
                faces.push(f);
            }
        }
        let loops = close_loops(&rims);
        let caps = cap(&loops, |i| plane.uv(self.positions[i as usize]));
        faces.extend(std::iter::repeat_n(face, caps.len()));
        triangles.extend(caps);
        self.triangles = triangles;
        self.faces = faces;
    }

    /// The point where edge `a`–`b` crosses `plane`, made once for both
    /// triangles of the edge.
    fn split(&mut self, a: u32, b: u32, plane: Plane, made: &mut HashMap<(u32, u32), u32>) -> u32 {
        let key = (a.min(b), a.max(b));
        if let Some(&i) = made.get(&key) {
            return i;
        }
        let (lo, hi) = (key.0 as usize, key.1 as usize);
        let (p, q) = (self.positions[lo], self.positions[hi]);
        let (dp, dq) = (plane.distance(p), plane.distance(q));
        let t = (dp / (dp - dq)).clamp(0.0, 1.0);
        let t = if t.is_finite() { t } else { 0.5 };
        let mut x = p.lerp(q, t);
        x[plane.axis] = plane.at;
        if !x.is_finite() {
            // An end that isn't finite: keep the other, on the plane.
            x = if p.is_finite() { p } else { q };
            x[plane.axis] = plane.at;
        }
        self.positions.push(x);
        if let Some(colors) = &mut self.colors {
            let [cp, cq] = [colors[lo], colors[hi]];
            colors.push(std::array::from_fn(|c| {
                (f32::from(cp[c]) + (f32::from(cq[c]) - f32::from(cp[c])) * t).round() as u8
            }));
        }
        let i = (self.positions.len() - 1) as u32;
        made.insert(key, i);
        i
    }
}

/// The closed loops a cap must fill, from the edges on the plane as the
/// kept triangles go round them: each loop goes the other way round, as
/// the cap's own triangles do. Edges met both ways are inside the surface
/// and cancel; a chain that doesn't close is left out.
fn close_loops(rims: &[(u32, u32)]) -> Vec<Vec<u32>> {
    let mut count: HashMap<(u32, u32), i32> = HashMap::new();
    for &e in rims {
        *count.entry(e).or_default() += 1;
    }
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&(a, b), &n) in &count {
        let back = count.get(&(b, a)).copied().unwrap_or(0);
        for _ in back..n {
            next.entry(b).or_default().push(a);
        }
    }
    // In a fixed order, so the same cut gives the same cap.
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort_unstable();
    for to in next.values_mut() {
        to.sort_unstable();
    }
    let mut loops = Vec::new();
    for s in starts {
        while let Some(first) = next.get_mut(&s).and_then(Vec::pop) {
            let mut ring = vec![s];
            let mut at: HashMap<u32, usize> = HashMap::from([(s, 0)]);
            let mut cur = first;
            loop {
                if let Some(&k) = at.get(&cur) {
                    // Back at a corner already met: what lies between is
                    // a loop of its own.
                    let sub = ring.split_off(k);
                    for v in &sub {
                        at.remove(v);
                    }
                    loops.push(sub);
                    if k == 0 {
                        break;
                    }
                }
                at.insert(cur, ring.len());
                ring.push(cur);
                match next.get_mut(&cur).and_then(Vec::pop) {
                    Some(n) => cur = n,
                    // An open chain.
                    None => break,
                }
            }
        }
    }
    loops
}

/// Twice the signed area of triangle `a b c`: positive anticlockwise.
fn orient(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    (b - a).perp_dot(c - a)
}

/// A loop on the plane.
struct Ring {
    corners: Vec<u32>,
    points: Vec<DVec2>,
    /// Twice the signed area.
    area: f64,
    min: DVec2,
    max: DVec2,
}

impl Ring {
    fn new(corners: &[u32], uv: &impl Fn(u32) -> DVec2) -> Option<Ring> {
        if corners.len() < 3 {
            return None;
        }
        let points: Vec<DVec2> = corners.iter().map(|&i| uv(i)).collect();
        let mut area = 0.0;
        let (mut min, mut max) = (DVec2::INFINITY, DVec2::NEG_INFINITY);
        for (i, &p) in points.iter().enumerate() {
            area += p.perp_dot(points[(i + 1) % points.len()]);
            min = min.min(p);
            max = max.max(p);
        }
        (area != 0.0 && area.is_finite()).then(|| Ring {
            corners: corners.to_vec(),
            points,
            area,
            min,
            max,
        })
    }

    /// Even-odd: whether `p` is inside.
    fn contains(&self, p: DVec2) -> bool {
        if p.cmplt(self.min).any() || p.cmpgt(self.max).any() {
            return false;
        }
        let mut inside = false;
        let n = self.points.len();
        for i in 0..n {
            let (a, b) = (self.points[i], self.points[(i + 1) % n]);
            if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y) {
                inside = !inside;
            }
        }
        inside
    }

    /// Goes round anticlockwise if `ccw`, else clockwise.
    fn turn(&mut self, ccw: bool) {
        if (self.area > 0.0) != ccw {
            self.corners.reverse();
            self.points.reverse();
            self.area = -self.area;
        }
    }
}

/// Triangles filling `loops`, anticlockwise in `uv`. A loop inside an odd
/// number of others is a hole in the one directly around it.
fn cap(loops: &[Vec<u32>], uv: impl Fn(u32) -> DVec2) -> Vec<[u32; 3]> {
    let mut rings: Vec<Ring> = loops.iter().filter_map(|l| Ring::new(l, &uv)).collect();
    // A point just inside each ring's first edge's middle, to test it
    // with: not a corner, which loops may share.
    let probes: Vec<DVec2> = rings
        .iter()
        .map(|r| (r.points[0] + r.points[1]) * 0.5)
        .collect();
    let around: Vec<Vec<usize>> = (0..rings.len())
        .map(|i| {
            (0..rings.len())
                .filter(|&j| j != i && rings[j].contains(probes[i]))
                .collect()
        })
        .collect();
    let depth: Vec<usize> = around.iter().map(Vec::len).collect();
    let mut holes: Vec<Vec<usize>> = vec![Vec::new(); rings.len()];
    for (r, d) in rings.iter_mut().zip(&depth) {
        r.turn(d.is_multiple_of(2));
    }
    for i in 0..rings.len() {
        // A hole's ring directly around it: the smallest one level up.
        if !depth[i].is_multiple_of(2)
            && let Some(parent) = around[i]
                .iter()
                .copied()
                .filter(|&j| depth[j] + 1 == depth[i])
                .min_by(|&a, &b| rings[a].area.abs().total_cmp(&rings[b].area.abs()))
        {
            holes[parent].push(i);
        }
    }
    let mut triangles = Vec::new();
    for i in 0..rings.len() {
        if depth[i].is_multiple_of(2) {
            let inner: Vec<&Ring> = holes[i].iter().map(|&h| &rings[h]).collect();
            fill(&rings[i], &inner, &mut triangles);
        }
    }
    triangles
}

/// A corner of the polygon being filled: where it is, and its vertex.
type Corner = (DVec2, u32);

/// Fills `outer` (anticlockwise) less `holes` (clockwise, inside it).
fn fill(outer: &Ring, holes: &[&Ring], out: &mut Vec<[u32; 3]>) {
    let mut poly: Vec<Corner> = outer
        .points
        .iter()
        .copied()
        .zip(outer.corners.iter().copied())
        .collect();
    // From the right, so each bridge goes to what is already joined.
    let mut holes: Vec<&&Ring> = holes.iter().collect();
    holes.sort_by(|a, b| b.max.x.total_cmp(&a.max.x));
    for hole in holes {
        bridge(&mut poly, hole);
    }
    ear_clip(&poly, out);
}

/// Joins `hole` into `poly` along a bridge from the hole's rightmost
/// corner to a corner of `poly` it can see, gone over both ways.
fn bridge(poly: &mut Vec<Corner>, hole: &Ring) {
    let Some(m) = (0..hole.points.len()).max_by(|&a, &b| {
        let (a, b) = (hole.points[a], hole.points[b]);
        a.x.total_cmp(&b.x).then(b.y.total_cmp(&a.y))
    }) else {
        return;
    };
    let mp = hole.points[m];
    let n = poly.len();
    // The nearest edge a ray to the right crosses.
    let mut hit: Option<(f64, usize)> = None;
    for i in 0..n {
        let (a, b) = (poly[i].0, poly[(i + 1) % n].0);
        if (a.y > mp.y) == (b.y > mp.y) {
            continue;
        }
        let x = a.x + (mp.y - a.y) * (b.x - a.x) / (b.y - a.y);
        if x >= mp.x && hit.is_none_or(|(best, _)| x < best) {
            hit = Some((x, i));
        }
    }
    let Some((x, i)) = hit else {
        return;
    };
    let ray = DVec2::new(x, mp.y);
    let j = (i + 1) % n;
    let mut p = if poly[i].0.x > poly[j].0.x { i } else { j };
    // A corner inside the triangle from the hole to the hit may hide that
    // end: take the one nearest the ray's direction instead.
    let (pp, tri) = (poly[p].0, [mp, ray, poly[p].0]);
    let mut best = f64::INFINITY;
    for (k, &(q, _)) in poly.iter().enumerate() {
        if k == p || q == pp || q.x <= mp.x || !in_triangle_any(tri, q) {
            continue;
        }
        let tan = (q.y - mp.y).abs() / (q.x - mp.x);
        if tan < best && locally_inside(poly, k, mp) {
            best = tan;
            p = k;
        }
    }
    let mut joined = Vec::with_capacity(n + hole.points.len() + 2);
    joined.extend_from_slice(&poly[..=p]);
    let len = hole.points.len();
    for k in 0..=len {
        let h = (m + k) % len;
        joined.push((hole.points[h], hole.corners[h]));
    }
    joined.push(poly[p]);
    joined.extend_from_slice(&poly[p + 1..]);
    *poly = joined;
}

/// Whether `q` is inside or on triangle `t`, however it turns.
fn in_triangle_any(t: [DVec2; 3], q: DVec2) -> bool {
    let d = [
        orient(t[0], t[1], q),
        orient(t[1], t[2], q),
        orient(t[2], t[0], q),
    ];
    d.iter().all(|&d| d >= 0.0) || d.iter().all(|&d| d <= 0.0)
}

/// Whether the way from corner `k` of anticlockwise `poly` towards `to`
/// starts inside it.
fn locally_inside(poly: &[Corner], k: usize, to: DVec2) -> bool {
    let n = poly.len();
    let (a, q, b) = (poly[(k + n - 1) % n].0, poly[k].0, poly[(k + 1) % n].0);
    if orient(a, q, b) >= 0.0 {
        orient(a, q, to) >= 0.0 && orient(q, b, to) >= 0.0
    } else {
        orient(a, q, to) >= 0.0 || orient(q, b, to) >= 0.0
    }
}

/// Fills anticlockwise `poly` by cutting off ears, one corner at a time.
/// A corner in line with its neighbours goes as a flat triangle, which
/// keeps every edge of the loop in the cap. If no ear is left (a loop that
/// crosses itself, as noisy scans give), a corner goes anyway.
fn ear_clip(poly: &[Corner], out: &mut Vec<[u32; 3]>) {
    let n = poly.len();
    if n < 3 {
        return;
    }
    let pt = |i: usize| poly[i].0;
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    // Only corners that turn the wrong way (or not at all) can sit in an
    // ear; corners only ever turn less as ears go. They go in a grid.
    let mut grid = Grid::new(
        poly,
        (0..n).filter(|&i| orient(pt(prev[i]), pt(i), pt(next[i])) <= 0.0),
    );
    let is_ear = |grid: &Grid, prev: &[usize], next: &[usize], i: usize| {
        let (a, b, c) = (pt(prev[i]), pt(i), pt(next[i]));
        if orient(a, b, c) <= 0.0 {
            return false;
        }
        let lo = a.min(b).min(c);
        let hi = a.max(b).max(c);
        !grid.any_in(lo, hi, |q| {
            let p = pt(q);
            q != prev[i]
                && q != i
                && q != next[i]
                && p != a
                && p != b
                && p != c
                && orient(pt(prev[q]), p, pt(next[q])) <= 0.0
                && orient(a, b, p) >= 0.0
                && orient(b, c, p) >= 0.0
                && orient(c, a, p) >= 0.0
        })
    };
    let mut i = 0;
    let mut left = n;
    let mut misses = 0;
    while left > 3 {
        let (p, q) = (prev[i], next[i]);
        let flat = orient(pt(p), pt(i), pt(q)) == 0.0;
        let mut take = flat || is_ear(&grid, &prev, &next, i);
        if !take && misses >= left {
            // Stuck: take the first corner that turns the right way, or
            // this one.
            let mut k = i;
            for _ in 0..left {
                if orient(pt(prev[k]), pt(k), pt(next[k])) > 0.0 {
                    break;
                }
                k = next[k];
            }
            i = k;
            take = true;
        }
        if take {
            let (p, q) = (prev[i], next[i]);
            out.push([poly[p].1, poly[i].1, poly[q].1]);
            next[p] = q;
            prev[q] = p;
            grid.remove(poly, i);
            left -= 1;
            misses = 0;
            i = q;
        } else {
            misses += 1;
            i = q;
        }
    }
    out.push([poly[prev[i]].1, poly[i].1, poly[next[i]].1]);
}

/// Corners in square cells, to find those inside an ear quickly.
struct Grid {
    min: DVec2,
    inv: DVec2,
    side: usize,
    cells: Vec<Vec<usize>>,
}

impl Grid {
    fn new(poly: &[Corner], corners: impl Iterator<Item = usize>) -> Grid {
        let (mut min, mut max) = (DVec2::INFINITY, DVec2::NEG_INFINITY);
        for &(p, _) in poly {
            min = min.min(p);
            max = max.max(p);
        }
        let side = ((poly.len() as f64).sqrt().ceil() as usize).clamp(1, 256);
        let size = (max - min).max(DVec2::splat(f64::MIN_POSITIVE));
        let mut grid = Grid {
            min,
            inv: DVec2::splat(side as f64) / size,
            side,
            cells: vec![Vec::new(); side * side],
        };
        for i in corners {
            let c = grid.cell(poly[i].0);
            grid.cells[c[1] * side + c[0]].push(i);
        }
        grid
    }

    fn cell(&self, p: DVec2) -> [usize; 2] {
        let c = ((p - self.min) * self.inv).floor();
        let clamp = |v: f64| {
            if v.is_finite() {
                (v.max(0.0) as usize).min(self.side - 1)
            } else {
                0
            }
        };
        [clamp(c.x), clamp(c.y)]
    }

    fn any_in(&self, lo: DVec2, hi: DVec2, mut f: impl FnMut(usize) -> bool) -> bool {
        let (a, b) = (self.cell(lo), self.cell(hi));
        for y in a[1]..=b[1] {
            for x in a[0]..=b[0] {
                if self.cells[y * self.side + x].iter().any(|&i| f(i)) {
                    return true;
                }
            }
        }
        false
    }

    fn remove(&mut self, poly: &[Corner], i: usize) {
        let c = self.cell(poly[i].0);
        let cell = &mut self.cells[c[1] * self.side + c[0]];
        if let Some(k) = cell.iter().position(|&j| j == i) {
            cell.swap_remove(k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meshio::is_closed;
    use crate::sculpt::mesh::Mesh;

    fn cube(half: f32) -> RefMesh {
        let positions = (0..8)
            .map(|i| {
                Vec3::new(
                    if i & 1 == 0 { -half } else { half },
                    if i & 2 == 0 { -half } else { half },
                    if i & 4 == 0 { -half } else { half },
                )
            })
            .collect();
        // Anticlockwise from outside.
        let triangles = vec![
            [0, 4, 6],
            [0, 6, 2],
            [1, 3, 7],
            [1, 7, 5],
            [0, 1, 5],
            [0, 5, 4],
            [2, 6, 7],
            [2, 7, 3],
            [0, 2, 3],
            [0, 3, 1],
            [4, 5, 7],
            [4, 7, 6],
        ];
        RefMesh {
            positions,
            triangles,
            colors: None,
        }
    }

    fn sphere(level: u32, radius: f32, inward: bool) -> RefMesh {
        let m = Mesh::icosphere(level);
        let positions = m.positions.iter().map(|&p| p * radius).collect();
        let triangles = m
            .triangles
            .iter()
            .map(|&[a, b, c]| if inward { [a, c, b] } else { [a, b, c] })
            .collect();
        RefMesh {
            positions,
            triangles,
            colors: None,
        }
    }

    /// Two meshes as one.
    fn join(a: RefMesh, b: &RefMesh) -> RefMesh {
        let mut m = a;
        let n = m.positions.len() as u32;
        m.positions.extend(&b.positions);
        m.triangles
            .extend(b.triangles.iter().map(|t| t.map(|i| i + n)));
        m
    }

    fn volume(c: &Cropped) -> f32 {
        c.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| c.positions[i as usize]);
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }

    fn boxed(min: f32, max: f32) -> Aabb {
        Aabb {
            min: Vec3::splat(min),
            max: Vec3::splat(max),
        }
    }

    /// Every corner used is inside `b`, within a little.
    fn inside(c: &Cropped, b: Aabb) -> bool {
        let tiny = Vec3::splat(1e-4);
        c.triangles.iter().flatten().all(|&i| {
            let p = c.positions[i as usize];
            p.cmpge(b.min - tiny).all() && p.cmple(b.max + tiny).all()
        })
    }

    #[test]
    fn half_a_cube_comes_out_closed() {
        let b = Aabb {
            min: Vec3::splat(-2.0),
            max: Vec3::new(0.25, 2.0, 2.0),
        };
        let c = crop(&cube(1.0), b);
        assert!(is_closed(&c.triangles));
        assert!(
            (volume(&c) - 2.0 * 2.0 * 1.25).abs() < 1e-4,
            "{}",
            volume(&c)
        );
        assert!(inside(&c, b));
        assert!(c.caps < c.triangles.len());
        assert!(c.cap_faces.iter().all(|&f| f == 1));
    }

    #[test]
    fn a_box_corner_meets_three_caps() {
        let b = boxed(0.0, 2.0);
        let c = crop(&cube(1.0), b);
        assert!(is_closed(&c.triangles));
        assert!((volume(&c) - 1.0).abs() < 1e-4, "{}", volume(&c));
        let mut faces = c.cap_faces.clone();
        faces.sort_unstable();
        faces.dedup();
        assert_eq!(faces, vec![0, 2, 4]);
    }

    #[test]
    fn a_box_inside_the_shape_comes_out_as_the_box() {
        let b = boxed(-0.5, 0.5);
        let c = crop(&sphere(2, 2.0, false), b);
        assert!(is_closed(&c.triangles));
        assert!((volume(&c) - 1.0).abs() < 1e-4, "{}", volume(&c));
        assert_eq!(c.caps, 0);
    }

    #[test]
    fn corners_on_a_plane_still_close() {
        // The icosphere has corners at exactly x = 0, and the cube's faces
        // lie on the box.
        let ball = sphere(2, 1.0, false);
        let b = Aabb {
            min: Vec3::splat(-2.0),
            max: Vec3::new(0.0, 2.0, 2.0),
        };
        let c = crop(&ball, b);
        assert!(is_closed(&c.triangles));
        let whole = volume(&Cropped::whole(&ball));
        assert!((volume(&c) - whole / 2.0).abs() < 1e-3, "{}", volume(&c));

        // Faces on the box are kept as they are, whichever way they turn.
        let c = crop(&cube(1.0), boxed(-1.0, 1.0));
        assert!(is_closed(&c.triangles));
        assert!((volume(&c) - 8.0).abs() < 1e-4);
        assert_eq!(c.triangles.len(), 12);
        let mut mixed = cube(1.0);
        for t in mixed.triangles.iter_mut().step_by(4) {
            t.swap(1, 2);
        }
        let c = crop(&mixed, boxed(-1.0, 1.0));
        assert_eq!(c.triangles.len(), 12);
        assert_eq!(c.caps, 12);
        let c = crop(
            &cube(1.0),
            Aabb {
                min: Vec3::splat(-1.0),
                max: Vec3::new(0.0, 1.0, 1.0),
            },
        );
        assert!(is_closed(&c.triangles));
        assert!((volume(&c) - 4.0).abs() < 1e-4);
    }

    #[test]
    fn a_hollow_cast_cut_across_has_a_ring_for_a_cap() {
        let shell = join(sphere(3, 1.0, false), &sphere(3, 0.6, true));
        let b = Aabb {
            min: Vec3::splat(-2.0),
            max: Vec3::new(2.0, 0.1, 2.0),
        };
        let c = crop(&shell, b);
        assert!(is_closed(&c.triangles));
        // As much as the outer ball's cut less the inner one's.
        let part = |level: u32, r: f32| {
            let ball = sphere(level, r, false);
            volume(&crop(&ball, b))
        };
        let want = part(3, 1.0) - part(3, 0.6);
        assert!((volume(&c) - want).abs() < 1e-3, "{} {want}", volume(&c));
        // Nested again: a ball inside the hollow is filled.
        let nested = join(shell, &sphere(2, 0.3, false));
        let c = crop(&nested, b);
        assert!(is_closed(&c.triangles));
        assert!((volume(&c) - want - part(2, 0.3)).abs() < 1e-3);
    }

    #[test]
    fn a_crop_that_keeps_all_or_nothing() {
        let ball = sphere(2, 1.0, false);
        let c = crop(&ball, boxed(-3.0, 3.0));
        assert_eq!(c.triangles, ball.triangles);
        assert_eq!(c.caps, c.triangles.len());
        let c = crop(&ball, boxed(5.0, 6.0));
        assert!(c.triangles.is_empty());
        // Past one plane and crossing another's line, outside the box.
        let c = crop(
            &ball,
            Aabb {
                min: Vec3::new(2.0, -0.5, -0.5),
                max: Vec3::new(3.0, 0.5, 0.5),
            },
        );
        assert!(c.triangles.is_empty());
    }

    #[test]
    fn an_open_scan_caps_the_loops_that_close() {
        // A cube without its top.
        let mut open = cube(1.0);
        open.triangles.retain(|t| !t.iter().all(|&i| i & 2 != 0));
        // Cut across the opening: the chain on x = 0 doesn't close.
        let b = Aabb {
            min: Vec3::splat(-2.0),
            max: Vec3::new(0.0, 2.0, 2.0),
        };
        let c = crop(&open, b);
        assert!(!c.triangles.is_empty());
        assert_eq!(c.caps, c.triangles.len());
        // Cut below it: the loop closes and the cube with it.
        let b = Aabb {
            min: Vec3::splat(-2.0),
            max: Vec3::new(2.0, 0.5, 2.0),
        };
        let c = crop(&open, b);
        assert!(is_closed(&c.triangles));
        assert!((volume(&c) - 6.0).abs() < 1e-4);
    }

    #[test]
    fn slivers_and_broken_input_never_panic() {
        let mut m = cube(1.0);
        // A flat sliver across the cut, a repeated corner, a corner that
        // isn't finite and an index past the end.
        m.positions.push(Vec3::new(-1.0, 0.0, 0.0));
        m.positions.push(Vec3::new(1.0, 0.0, 0.0));
        m.positions.push(Vec3::new(f32::NAN, 0.0, 0.0));
        m.triangles.push([8, 9, 9]);
        m.triangles.push([8, 9, 0]);
        m.triangles.push([8, 10, 0]);
        m.triangles.push([8, 9, 99]);
        m.colors = Some(vec![[200; 4]; m.positions.len()]);
        let c = crop(
            &m,
            Aabb {
                min: Vec3::splat(-2.0),
                max: Vec3::new(0.0, 2.0, 2.0),
            },
        );
        assert!(c.positions.iter().all(|p| p.is_finite() || p.x.is_nan()));
        assert_eq!(c.colors.as_ref().map(Vec::len), Some(c.positions.len()));
        // A box with no thickness.
        let c = crop(
            &cube(1.0),
            Aabb {
                min: Vec3::ZERO,
                max: Vec3::new(0.0, 1.0, 1.0),
            },
        );
        assert!(volume(&c).abs() < 1e-4);
    }

    #[test]
    fn random_boxes_through_a_ball_or_a_shell_always_close() {
        let ball = sphere(3, 1.0, false);
        let shell = join(sphere(3, 1.0, false), &sphere(2, 0.5, true));
        let mut seed = 0x2545_f491_u32;
        let mut rand = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        for mesh in [ball, shell] {
            let whole = volume(&Cropped::whole(&mesh));
            for _ in 0..200 {
                let a = Vec3::new(rand(), rand(), rand()) * 2.4 - 1.2;
                let b = Vec3::new(rand(), rand(), rand()) * 2.4 - 1.2;
                let mut b = Aabb {
                    min: a.min(b),
                    max: a.max(b),
                };
                // Now and then through corners of the mesh.
                if rand() < 0.3 {
                    b.max.x = mesh.positions[(rand() * 600.0) as usize].x;
                    b.min.y = mesh.positions[(rand() * 600.0) as usize].y;
                }
                let c = crop(&mesh, b);
                assert!(c.triangles.is_empty() || is_closed(&c.triangles), "{b:?}");
                assert!(inside(&c, b), "{b:?}");
                let v = volume(&c);
                assert!(v > -1e-4 && v <= whole + 1e-4, "{b:?} {v}");
            }
        }
    }

    /// Each triangle in four, sharing the new middles.
    fn subdivide(m: &RefMesh) -> RefMesh {
        let mut positions = m.positions.clone();
        let mut middles: HashMap<(u32, u32), u32> = HashMap::new();
        let mut middle = |a: u32, b: u32| {
            *middles.entry((a.min(b), a.max(b))).or_insert_with(|| {
                positions.push((positions[a as usize] + positions[b as usize]) * 0.5);
                (positions.len() - 1) as u32
            })
        };
        let mut triangles = Vec::with_capacity(m.triangles.len() * 4);
        for &[a, b, c] in &m.triangles {
            let (ab, bc, ca) = (middle(a, b), middle(b, c), middle(c, a));
            triangles.extend([[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]);
        }
        RefMesh {
            positions,
            triangles,
            colors: None,
        }
    }

    /// `cargo test --release -p splatstudy-core --lib crop_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn crop_timing() {
        use std::time::Instant;

        use crate::reference::{Content, Reference, Source};
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/samples/brutus.ply");
        let bytes = std::fs::read(path).unwrap();
        let Content::Mesh(mesh) = crate::reference::load::load("brutus.ply", &bytes).unwrap()
        else {
            panic!("not a mesh");
        };
        let mesh = subdivide(&mesh);
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
        println!(
            "{} vertices, {} triangles",
            mesh.positions.len(),
            mesh.triangles.len()
        );
        let mut r = Reference::new(
            Source {
                name: "brutus.ply".into(),
                bytes: std::sync::Arc::from([]),
            },
            Content::Mesh(mesh.clone()),
        );
        r.fit();
        let range = r.crop_range().unwrap();
        let at = |f: Vec3| range.min + (range.max - range.min) * f;
        let boxes = [
            ("all kept", None),
            ("box around all", Some(range)),
            (
                "half, cut across",
                Some(Aabb {
                    min: range.min,
                    max: at(Vec3::new(1.0, 0.5, 1.0)),
                }),
            ),
            (
                "corner, three cuts",
                Some(Aabb {
                    min: at(Vec3::splat(0.4)),
                    max: range.max,
                }),
            ),
            (
                "all six sides cut",
                Some(Aabb {
                    min: at(Vec3::splat(0.3)),
                    max: at(Vec3::splat(0.7)),
                }),
            ),
        ];
        for (name, b) in boxes {
            r.crop = b;
            // As before: every corner moved, the triangles inside kept.
            let t = Instant::now();
            let m = r.align.matrix();
            let positions: Vec<Vec3> = mesh
                .positions
                .iter()
                .map(|&p| m.transform_point3(p))
                .collect();
            let kept: Vec<[u32; 3]> = mesh
                .triangles
                .iter()
                .filter(|t| {
                    t.iter()
                        .all(|&i| b.is_none_or(|b| b.contains(mesh.positions[i as usize])))
                })
                .copied()
                .collect();
            let before = ms(t);
            std::hint::black_box((positions, kept));
            let t = Instant::now();
            std::hint::black_box(b.map(|b| classify(&mesh, &Plane::of(b))));
            let sort = ms(t);
            let t = Instant::now();
            let cropped = b.map(|b| crop(&mesh, b));
            let clip = ms(t);
            let t = Instant::now();
            let world = r.world_mesh().unwrap();
            let world_ms = ms(t);
            let t = Instant::now();
            let flat = world.with_flat_caps();
            let flat_ms = ms(t);
            // A fit after the crop: the cut kept, its corners moved again.
            let t = Instant::now();
            let m = r.align.matrix();
            let moved: Vec<Vec3> = flat
                .positions
                .iter()
                .map(|&p| m.transform_point3(p))
                .collect();
            let move_ms = ms(t);
            std::hint::black_box(moved);
            // What GpuMesh::from_triangles does on the CPU: smooth normals.
            let t = Instant::now();
            let mut normals = vec![Vec3::ZERO; flat.positions.len()];
            for tri in &flat.triangles {
                let [a, b, c] = tri.map(|i| flat.positions[i as usize]);
                let n = (b - a).cross(c - a);
                for &i in tri {
                    normals[i as usize] += n;
                }
            }
            let normals: Vec<Vec3> = normals.iter().map(|n| n.normalize_or(Vec3::Y)).collect();
            let normals_ms = ms(t);
            std::hint::black_box(normals);
            println!(
                "{name:20} before {before:5.1} | sort {sort:5.1} | clip+cap {clip:5.1} \
                 | world_mesh {world_ms:5.1} | flat caps {flat_ms:4.1} | move {move_ms:4.1} \
                 | normals {normals_ms:5.1} ms | {} triangles, {} caps, closed: {}",
                flat.triangles.len(),
                flat.triangles.len() - flat.caps,
                cropped.is_some_and(|c| is_closed(&c.triangles)),
            );
        }
    }

    #[test]
    fn flat_caps_have_their_own_corners() {
        let c = crop(&cube(1.0), boxed(0.0, 2.0)).with_flat_caps();
        let surface: std::collections::HashSet<u32> =
            c.triangles[..c.caps].iter().flatten().copied().collect();
        assert!(
            c.triangles[c.caps..]
                .iter()
                .flatten()
                .all(|i| !surface.contains(i))
        );
        assert!((volume(&c) - 1.0).abs() < 1e-4);
    }
}
