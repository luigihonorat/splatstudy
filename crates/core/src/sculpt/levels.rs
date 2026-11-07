// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Multiresolution: the clay kept at several subdivision levels at once,
//! as in ZBrush's subdivision levels or Blender's Multires. Level 0 is the
//! base; each level above is the one below split by [`Split`], so its
//! vertices start with those of the level below, in the same order.
//!
//! The clay is always one level, sculpted directly (the live mesh). The
//! others wait:
//!
//! - **Below**, as positions. Leaving a level carries what was sculpted on
//!   it down to every level below: each vertex there moves by the change
//!   of its own vertex and of its ring above, half each, so broad changes
//!   come down whole and fine ones blur rather than alias.
//! - **Above**, as detail: how far each vertex was from the level below
//!   smoothed up with Loop's scheme, measured in a frame of that smoothed
//!   surface (its normal, and a tangent towards the vertex's first
//!   neighbour). Stepping up smooths the level up again and adds the
//!   detail back in the new frames, so detail rides on broad changes made
//!   below.
//!
//! Each level remembers its detail as long as nothing it rests on has
//! changed, so going down and up again without sculpting gives back the
//! same clay bit for bit; the detail is measured afresh only when the
//! level or one below it was sculpted.
//!
//! Every step is plain arithmetic in a fixed order: a replay of the log
//! (see `record.rs`) goes through the same levels and gets the same bits.

use std::sync::Arc;

use glam::Vec3;

use super::mesh::{Adjacency, Mesh, area_normals};
use super::subdivide::{Split, child_quads, parent_quads, parents};
use super::{Event, MAX_VERTICES, Sculpt, Step};
use crate::io::{ReadError, Reader, Writer};

/// The most levels the clay may have: each has four times the triangles
/// of the one below, so the vertex limit comes long before.
pub const MAX_LEVELS: usize = 12;

/// The clay's subdivision levels, apart from the live one.
#[derive(Clone, Debug, PartialEq)]
pub struct Levels {
    /// Vertices of each level, from the base up.
    counts: Vec<usize>,
    /// The level the clay is at.
    current: usize,
    /// Each level below the current one, from the base up.
    below: Vec<Below>,
    /// The detail of each level above the current one, nearest first.
    above: Vec<Detail>,
    /// The clay's positions when it came to this level, or when what was
    /// sculpted on it last went down: sculpting since is the difference.
    arrival: Vec<Vec3>,
    /// The current level's detail, while the clay is still exactly what it
    /// was rebuilt from (see [`Below::detail`]).
    detail: Option<Detail>,
}

/// A level below the clay.
#[derive(Clone, Debug, PartialEq)]
struct Below {
    positions: Vec<Vec3>,
    /// Its detail over the level below it, while still true: kept from
    /// when it was rebuilt from it, dropped as soon as either changes.
    /// Never for the base.
    detail: Option<Detail>,
}

/// Offsets of a level's vertices from the level below smoothed up, each
/// as (tangent, bitangent, normal) in its own frame there. Empty when
/// every offset is zero, as after a subdivision.
#[derive(Clone, Debug, Default, PartialEq)]
struct Detail(Vec<Vec3>);

/// A vertex's frame on a surface: tangent, bitangent, normal.
fn frame(positions: &[Vec3], normals: &[Vec3], ring: &Adjacency, v: usize) -> [Vec3; 3] {
    let n = normals[v];
    let p = positions[v];
    let t = ring
        .of(v as u32)
        .first()
        .map(|&u| {
            let e = positions[u as usize] - p;
            (e - n * e.dot(n)).normalize_or_zero()
        })
        .filter(|t| *t != Vec3::ZERO)
        .unwrap_or_else(|| n.any_orthonormal_vector());
    [t, n.cross(t), n]
}

/// The detail of `positions` over `smooth`, a surface with the triangles
/// and rings of `topology`.
fn encode(smooth: &[Vec3], topology: &Mesh, positions: &[Vec3]) -> Detail {
    let mut normals = vec![Vec3::ZERO; smooth.len()];
    area_normals(smooth, &topology.triangles, &mut normals);
    let mut flat = true;
    let offsets = (0..smooth.len())
        .map(|v| {
            let e = positions[v] - smooth[v];
            if e == Vec3::ZERO {
                return Vec3::ZERO;
            }
            flat = false;
            let [t, b, n] = frame(smooth, &normals, &topology.neighbours, v);
            Vec3::new(e.dot(t), e.dot(b), e.dot(n))
        })
        .collect();
    if flat {
        Detail::default()
    } else {
        Detail(offsets)
    }
}

/// `smooth`, the positions of `mesh`, with `detail` added back.
fn decode(mesh: &Mesh, detail: &Detail) -> Vec<Vec3> {
    let smooth = &mesh.positions;
    if detail.0.is_empty() {
        return smooth.clone();
    }
    (0..smooth.len())
        .map(|v| {
            let d = detail.0[v];
            if d == Vec3::ZERO {
                return smooth[v];
            }
            let [t, b, n] = frame(smooth, &mesh.normals, &mesh.neighbours, v);
            smooth[v] + t * d.x + b * d.y + n * d.z
        })
        .collect()
}

/// The level above `live` as Loop's scheme makes it, with its rings and
/// the normals of the smooth surface.
/// Its quads follow (see [`child_quads`]).
fn smoothed(live: &Mesh, split: &Split) -> Mesh {
    let mut mesh = Mesh::from_ordered(
        split.positions(live, &live.positions),
        split.triangles(live),
    );
    mesh.quads = child_quads(&live.triangles, &live.quads);
    mesh
}

/// A change of the vertices of a level, brought down to the `coarse`
/// vertices of the level below (`triangles` being the finer level's): half
/// each vertex's own change, half the average over its triangles' other
/// corners.
fn restrict(delta: &[Vec3], triangles: &[[u32; 3]], coarse: usize) -> Vec<Vec3> {
    let mut sum = vec![Vec3::ZERO; coarse];
    let mut count = vec![0u32; coarse];
    for &[a, b, c] in triangles {
        for [x, y, z] in [[a, b, c], [b, c, a], [c, a, b]] {
            if (x as usize) < coarse {
                sum[x as usize] += delta[y as usize] + delta[z as usize];
                count[x as usize] += 2;
            }
        }
    }
    (0..coarse)
        .map(|v| match count[v] {
            0 => delta[v],
            k => delta[v] * 0.5 + sum[v] * (0.5 / k as f32),
        })
        .collect()
}

impl Levels {
    /// The clay as the only level, ready to be subdivided.
    fn start(mesh: &Mesh) -> Levels {
        Levels {
            counts: vec![mesh.positions.len()],
            current: 0,
            below: Vec::new(),
            above: Vec::new(),
            arrival: mesh.positions.clone(),
            detail: None,
        }
    }

    /// The level the clay is at, from 0.
    pub fn current(&self) -> usize {
        self.current
    }

    /// Vertices of each level, from the base up.
    pub fn counts(&self) -> &[usize] {
        &self.counts
    }

    /// How many levels there are.
    pub fn len(&self) -> usize {
        self.counts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// Roughly how many bytes the levels hold.
    pub fn memory(&self) -> usize {
        let detail = |d: &Detail| d.0.len() * 12;
        self.below
            .iter()
            .map(|b| b.positions.len() * 12 + b.detail.as_ref().map_or(0, detail))
            .sum::<usize>()
            + self.above.iter().map(detail).sum::<usize>()
            + self.arrival.len() * 12
            + self.detail.as_ref().map_or(0, detail)
    }

    /// Vertices of the finest level.
    pub fn finest(&self) -> usize {
        self.counts.last().copied().unwrap_or(0)
    }

    /// Carries what was sculpted on this level since it came here down to
    /// every level below.
    fn settle(&mut self, live: &Mesh) {
        if live.positions == self.arrival {
            return;
        }
        self.detail = None;
        let mut delta: Vec<Vec3> = live
            .positions
            .iter()
            .zip(&self.arrival)
            .map(|(p, a)| *p - *a)
            .collect();
        let mut triangles = std::borrow::Cow::Borrowed(&live.triangles[..]);
        for j in (0..self.current).rev() {
            let coarse = self.counts[j];
            let down = restrict(&delta, &triangles, coarse);
            let below = &mut self.below[j];
            for (p, d) in below.positions.iter_mut().zip(&down) {
                *p += *d;
            }
            below.detail = None;
            if j == 0 {
                break;
            }
            match parents(&triangles, coarse) {
                Some(t) => triangles = t.into(),
                None => break,
            }
            delta = down;
        }
        self.arrival.clone_from(&live.positions);
    }

    /// One level up from `live`, the clay at the current level: the clay
    /// it becomes. `None`, with nothing changed, at the top.
    fn up(&mut self, live: &Mesh) -> Option<Mesh> {
        let next = *self.counts.get(self.current + 1)?;
        let split = Split::of(live)?;
        if split.len() != next || self.above.is_empty() {
            return None;
        }
        self.settle(live);
        let smooth = smoothed(live, &split);
        let detail = self.above.remove(0);
        Some(self.enter_above(live, smooth, detail))
    }

    /// Goes up from `live` to `smooth`, the level above as Loop's scheme
    /// makes it from `live`, with `detail` added back: the clay it becomes.
    fn enter_above(&mut self, live: &Mesh, mut smooth: Mesh, detail: Detail) -> Mesh {
        smooth.positions = decode(&smooth, &detail);
        self.below.push(Below {
            positions: live.positions.clone(),
            detail: self.detail.take(),
        });
        self.detail = Some(detail);
        self.arrival.clone_from(&smooth.positions);
        self.current += 1;
        smooth
    }

    /// Levels built up from `base`, `top` times: each level is the one
    /// below smoothed up with detail making it `target(level, split, below)`
    /// (up to rounding), exactly as going up to it would rebuild it. The
    /// clay at the top, and its levels. `None` if `base` has a triangle with
    /// a corner twice.
    pub fn build(
        base: Mesh,
        top: usize,
        mut target: impl FnMut(usize, &Split, &Mesh) -> Vec<Vec3>,
    ) -> Option<(Mesh, Levels)> {
        let mut levels = Levels::start(&base);
        let mut live = base;
        for level in 1..=top.min(MAX_LEVELS - 1) {
            let split = Split::of(&live)?;
            let smooth = smoothed(&live, &split);
            let detail = encode(&smooth.positions, &smooth, &target(level, &split, &live));
            levels.counts.push(split.len());
            live = levels.enter_above(&live, smooth, detail);
        }
        // As `Sculpt::swap_mesh` would have them.
        live.recompute_normals();
        Some((live, levels))
    }

    /// One level down from `live`: the clay it becomes. `None`, with
    /// nothing changed, at the base.
    fn down(&mut self, live: &Mesh) -> Option<Mesh> {
        let level = self.current.checked_sub(1)?;
        let triangles = parents(&live.triangles, self.counts[level])?;
        let mut coarse = Mesh::from_ordered(self.below[level].positions.clone(), triangles);
        coarse.quads = parent_quads(&live.quads);
        let split = Split::of(&coarse)?;
        if split.len() != live.positions.len() {
            return None;
        }
        self.settle(live);
        let below = self.below.pop()?;
        coarse.positions = below.positions;
        coarse.recompute_normals();
        let detail = match self.detail.take() {
            Some(detail) => detail,
            None => encode(
                &split.positions(&coarse, &coarse.positions),
                live,
                &live.positions,
            ),
        };
        self.above.insert(0, detail);
        self.detail = below.detail;
        self.arrival.clone_from(&coarse.positions);
        self.current = level;
        Some(coarse)
    }

    /// A new level above the top, as the top smoothed up with no detail:
    /// the clay it becomes. `None`, with nothing changed, unless the clay
    /// is at the top, or if it has a triangle with a corner twice.
    fn subdivide(&mut self, live: &Mesh) -> Option<Mesh> {
        if self.current + 1 != self.len() || self.len() >= MAX_LEVELS {
            return None;
        }
        let split = Split::of(live)?;
        self.counts.push(split.len());
        self.above.push(Detail::default());
        let mesh = self.up(live);
        if mesh.is_none() {
            self.counts.pop();
            self.above.pop();
        }
        mesh
    }

    /// Undoes [`Levels::subdivide`]: down from the top, which is dropped.
    fn unsubdivide(&mut self, live: &Mesh) -> Option<Mesh> {
        if self.current + 1 != self.len() {
            return None;
        }
        let mesh = self.down(live)?;
        self.above.remove(0);
        self.counts.pop();
        Some(mesh)
    }

    /// Drops every level above the clay; true if there were any.
    fn delete_higher(&mut self) -> bool {
        if self.above.is_empty() {
            return false;
        }
        self.above.clear();
        self.counts.truncate(self.current + 1);
        true
    }

    /// Drops every level below the clay, which becomes the base; true if
    /// there were any.
    fn delete_lower(&mut self) -> bool {
        if self.current == 0 {
            return false;
        }
        self.below.clear();
        self.counts.drain(..self.current);
        self.current = 0;
        self.detail = None;
        true
    }

    /// Writes the levels, `positions` being the clay's at the current
    /// level (which aren't written: the scene has them).
    pub fn write(&self, w: &mut Writer, positions: &[Vec3]) {
        w.u32(self.current as u32).len(self.counts.len());
        for &count in &self.counts {
            w.len(count);
        }
        let detail = |w: &mut Writer, detail: Option<&Detail>| match detail {
            None => {
                w.u8(0);
            }
            Some(d) if d.0.is_empty() => {
                w.u8(1);
            }
            Some(d) => {
                w.u8(2).vec3s(&d.0);
            }
        };
        for (j, below) in self.below.iter().enumerate() {
            w.vec3s(&below.positions);
            if j > 0 {
                detail(w, below.detail.as_ref());
            }
        }
        for d in &self.above {
            detail(w, Some(d));
        }
        if self.current > 0 {
            detail(w, self.detail.as_ref());
        }
        // Where the clay was when it came here: mostly where it is.
        let bits = |p: Vec3| p.to_array().map(f32::to_bits);
        let moved: Vec<usize> = (0..positions.len().min(self.arrival.len()))
            .filter(|&i| bits(positions[i]) != bits(self.arrival[i]))
            .collect();
        w.len(moved.len());
        for i in moved {
            w.u32(i as u32).vec3(self.arrival[i]);
        }
    }

    /// Reads what [`Levels::write`] wrote, for clay with `positions` and
    /// `triangles` at the current level. Refuses levels that don't fit
    /// that clay or each other.
    pub fn read(
        r: &mut Reader,
        positions: &[Vec3],
        triangles: &[[u32; 3]],
    ) -> Result<Levels, ReadError> {
        let current = r.u32()? as usize;
        let len = r.len(8)?;
        if !(2..=MAX_LEVELS).contains(&len) || current >= len {
            return Err(r.invalid("levels that can't be"));
        }
        let mut counts = Vec::with_capacity(len);
        for _ in 0..len {
            let count = usize::try_from(r.u64()?).unwrap_or(usize::MAX);
            if count > MAX_VERTICES || counts.last().is_some_and(|&last| count <= last) {
                return Err(r.invalid("levels with sizes that can't be"));
            }
            counts.push(count);
        }
        if counts[current] != positions.len() {
            return Err(r.invalid("levels that don't fit the clay"));
        }
        // The clay's triangles must split down to every level below.
        let mut t = std::borrow::Cow::Borrowed(triangles);
        for j in (0..current).rev() {
            match parents(&t, counts[j]) {
                Some(coarse) => t = coarse.into(),
                None => return Err(r.invalid("levels that don't fit the clay's triangles")),
            }
        }
        let vec3s = |r: &mut Reader, n: usize| -> Result<Vec<Vec3>, ReadError> {
            if r.remaining() / 12 < n {
                return Err(r.invalid("levels cut short"));
            }
            (0..n)
                .map(|_| match r.vec3()? {
                    p if p.is_finite() => Ok(p),
                    _ => Err(r.invalid("a level position that isn't a number")),
                })
                .collect()
        };
        let detail = |r: &mut Reader, n: usize| -> Result<Option<Detail>, ReadError> {
            match r.u8()? {
                0 => Ok(None),
                1 => Ok(Some(Detail::default())),
                2 => Ok(Some(Detail(vec3s(r, n)?))),
                _ => Err(r.invalid("unknown kind of level detail")),
            }
        };
        let mut below = Vec::with_capacity(current);
        for (j, &count) in counts.iter().enumerate().take(current) {
            let positions = vec3s(r, count)?;
            let detail = if j > 0 { detail(r, count)? } else { None };
            below.push(Below { positions, detail });
        }
        let mut above = Vec::with_capacity(len - current - 1);
        for &count in &counts[current + 1..] {
            above.push(detail(r, count)?.ok_or_else(|| r.invalid("a level above without detail"))?);
        }
        let detail = if current > 0 {
            detail(r, positions.len())?
        } else {
            None
        };
        let mut arrival = positions.to_vec();
        let moved = r.len(16)?;
        for _ in 0..moved {
            let i = r.u32()? as usize;
            let p = r.vec3()?;
            if i >= arrival.len() || !p.is_finite() {
                return Err(r.invalid("a level position that can't be"));
            }
            arrival[i] = p;
        }
        Ok(Levels {
            counts,
            current,
            below,
            above,
            arrival,
            detail,
        })
    }
}

impl Sculpt {
    /// The clay's level and how many levels there are, if it has been
    /// subdivided into levels.
    pub fn levels(&self) -> Option<(usize, usize)> {
        self.levels.as_ref().map(|l| (l.current(), l.len()))
    }

    /// The clay's levels, to save them.
    pub fn level_data(&self) -> Option<&Levels> {
        self.levels.as_ref()
    }

    /// Vertices the finest level would have after [`Sculpt::subdivide`].
    pub fn subdivided_len(&self) -> usize {
        match &self.levels {
            // Each level has four times the triangles of the one below; a
            // closed mesh has one edge per one and a half triangles.
            Some(levels) if levels.current() + 1 < levels.len() => {
                let up = (levels.len() - 1 - levels.current()) as u32;
                let triangles = self.mesh.triangles.len().saturating_mul(4usize.pow(up));
                levels.finest().saturating_add(triangles / 2 * 3)
            }
            _ => super::subdivide::subdivided_len(&self.mesh),
        }
    }

    /// Before a change of level: ends any stroke, and logs what a reopened
    /// sculpt owes, while the clay is still as it was.
    fn before_level_change(&mut self) {
        if self.stroke_open {
            self.end_stroke();
        }
        self.settle_resume();
    }

    /// Puts the clay of another level in place, with the mask and chalk
    /// carried onto it.
    fn show_level(&mut self, mut mesh: Mesh) {
        mesh.layers = self.mesh.layers.carried(self.mesh.positions.len(), &mesh);
        self.swap_mesh(mesh);
        self.releveled = true;
    }

    /// Adds a subdivision level above the top, smoothed with Loop's scheme,
    /// and goes up to it. The first time, the clay as it is becomes the
    /// base. Only at the top level. One undo step: undo drops the level
    /// again. False, with nothing done, if it can't be done.
    pub fn subdivide(&mut self) -> bool {
        self.before_level_change();
        let mut levels = self
            .levels
            .take()
            .unwrap_or_else(|| Levels::start(&self.mesh));
        let mesh = levels.subdivide(&self.mesh);
        let done = mesh.is_some();
        self.levels = (levels.len() > 1).then_some(levels);
        if let Some(mesh) = mesh {
            self.record(Event::Subdivide);
            self.show_level(mesh);
            self.undo.clear();
            self.redo.clear();
            self.undo.push(Step::Subdivide);
        }
        done
    }

    /// One level up (`up`) or down, keeping the detail of every level. Undo
    /// goes back; strokes made before can't be undone any more. False, with
    /// nothing done, at the top or the base, or without levels.
    pub fn change_level(&mut self, up: bool) -> bool {
        self.before_level_change();
        if !self.step_level(up) {
            return false;
        }
        self.record(Event::Level { up });
        self.undo.clear();
        self.redo.clear();
        self.undo.push(Step::Level { up });
        true
    }

    /// Goes one level up or down, unlogged; false if it can't.
    fn step_level(&mut self, up: bool) -> bool {
        let Some(levels) = &mut self.levels else {
            return false;
        };
        let mesh = if up {
            levels.up(&self.mesh)
        } else {
            levels.down(&self.mesh)
        };
        match mesh {
            Some(mesh) => {
                self.show_level(mesh);
                true
            }
            None => false,
        }
    }

    /// Drops every level above the clay (`higher`) or below it, which then
    /// becomes the base. The clay itself doesn't change, so strokes stay
    /// undoable; undo brings the levels back.
    pub fn delete_levels(&mut self, higher: bool) -> bool {
        self.before_level_change();
        let Some(levels) = &self.levels else {
            return false;
        };
        let mut changed = levels.clone();
        let done = if higher {
            changed.delete_higher()
        } else {
            changed.delete_lower()
        };
        if !done {
            return false;
        }
        self.record(Event::DeleteLevels { higher });
        let before = std::mem::replace(&mut self.levels, (changed.len() > 1).then_some(changed));
        self.releveled = true;
        self.undo.push(Step::Levels(before.map(Box::new)));
        self.redo.clear();
        true
    }

    /// Undoes or redoes a level step; `undo` is the direction. Returns the
    /// step for the other stack.
    pub(super) fn undo_level_step(&mut self, step: Step, undo: bool) -> Step {
        match step {
            Step::Level { up } => {
                // Undo goes the other way, redo the same.
                self.step_level(up != undo);
                Step::Level { up }
            }
            Step::Subdivide => {
                if let Some(levels) = &mut self.levels {
                    let mesh = if undo {
                        levels.unsubdivide(&self.mesh)
                    } else {
                        levels.subdivide(&self.mesh)
                    };
                    if levels.len() < 2 {
                        self.levels = None;
                    }
                    if let Some(mesh) = mesh {
                        self.show_level(mesh);
                    }
                } else if !undo {
                    let mut levels = Levels::start(&self.mesh);
                    if let Some(mesh) = levels.subdivide(&self.mesh) {
                        self.levels = Some(levels);
                        self.show_level(mesh);
                    }
                }
                Step::Subdivide
            }
            Step::Levels(levels) => {
                let other = std::mem::replace(&mut self.levels, levels.map(|l| *l));
                self.releveled = true;
                Step::Levels(other.map(Box::new))
            }
            other => other,
        }
    }

    /// The levels a whole checkpoint keeps with its mesh.
    pub(super) fn levels_for_checkpoint(&self) -> Option<Arc<Levels>> {
        self.levels.clone().map(Arc::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sculpt::{Brush, Dab, LayerOp, Replay};

    fn dab(brush: Brush, centre: Vec3, radius: f32) -> Dab {
        Dab {
            brush,
            centre,
            radius,
            strength: 0.8,
            invert: false,
            symmetry: true,
            direction: Vec3::X,
            buildup: Default::default(),
        }
    }

    fn stroke(s: &mut Sculpt, brush: Brush, at: Vec3, radius: f32) {
        s.begin_stroke();
        for i in 0..6 {
            s.dab(Dab {
                direction: if i == 0 { Vec3::ZERO } else { Vec3::Y },
                ..dab(
                    brush,
                    (at + Vec3::Y * (i as f32 * 0.02)).normalize(),
                    radius,
                )
            });
            s.refresh();
        }
        s.end_stroke();
    }

    /// A small sphere subdivided twice: three levels.
    fn leveled() -> Sculpt {
        let mut s = Sculpt::new(Mesh::icosphere(2));
        assert!(s.subdivide());
        assert!(s.subdivide());
        assert_eq!(s.levels(), Some((2, 3)));
        s
    }

    fn worst(a: &[Vec3], b: &[Vec3]) -> f32 {
        assert_eq!(a.len(), b.len());
        a.iter()
            .zip(b)
            .map(|(a, b)| a.distance(*b))
            .fold(0.0, f32::max)
    }

    #[test]
    fn going_down_and_up_without_sculpting_gives_the_same_clay() {
        let mut s = leveled();
        stroke(&mut s, Brush::Clay, Vec3::new(0.2, 0.3, 0.9), 0.15);
        stroke(&mut s, Brush::Crease, Vec3::new(-0.2, 0.1, 0.95), 0.1);
        // The first time down to the base, the sculpted detail of each
        // level is measured; from then on, round trips give back the same
        // bits.
        while s.change_level(false) {}
        while s.change_level(true) {}
        let finest = s.mesh.positions.clone();
        let triangles = s.mesh.triangles.clone();
        for _ in 0..3 {
            assert!(s.change_level(false));
            assert!(s.change_level(false));
            assert_eq!(s.levels(), Some((0, 3)));
            assert!(!s.change_level(false), "no level below the base");
            assert!(s.change_level(true));
            assert!(s.change_level(true));
            assert!(!s.change_level(true), "no level above the top");
            assert_eq!(s.mesh.positions, finest);
            assert_eq!(s.mesh.triangles, triangles);
        }
    }

    #[test]
    fn the_first_round_trip_after_sculpting_stays_within_rounding() {
        let mut s = leveled();
        stroke(&mut s, Brush::Clay, Vec3::new(0.2, 0.3, 0.9), 0.15);
        stroke(&mut s, Brush::Inflate, Vec3::new(-0.3, -0.2, 0.9), 0.2);
        let finest = s.mesh.positions.clone();
        s.change_level(false);
        s.change_level(false);
        s.change_level(true);
        s.change_level(true);
        assert!(worst(&s.mesh.positions, &finest) < 1e-5);
    }

    #[test]
    fn a_change_at_a_low_level_moves_the_finest_clay_and_keeps_its_detail() {
        let mut s = leveled();
        // Fine detail at the top.
        let at = Vec3::new(0.0, 0.0, 1.0);
        stroke(&mut s, Brush::Crease, Vec3::new(-0.05, 0.0, 1.0), 0.08);
        let detailed = s.mesh.positions.clone();
        // The same sculpt without the crease, for comparison.
        let mut plain = leveled();
        plain.change_level(false);
        plain.change_level(true);
        let smooth_top = plain.mesh.positions.clone();
        // Down to the base, then a broad move of the top of the sphere, on
        // both.
        for s in [&mut s, &mut plain] {
            s.change_level(false);
            s.change_level(false);
            s.begin_stroke();
            s.grab(at, 0.8, false);
            s.drag(Vec3::new(0.0, 0.0, 0.3));
            s.end_stroke();
            s.change_level(true);
            s.change_level(true);
        }
        let lifted = s.mesh.positions.clone();
        // The top of the clay went up with the broad move...
        let top = (0..lifted.len())
            .min_by(|&a, &b| {
                detailed[a]
                    .distance(at)
                    .total_cmp(&detailed[b].distance(at))
            })
            .unwrap();
        assert!(lifted[top].z > detailed[top].z + 0.2, "{:?}", lifted[top]);
        // ...and the crease rides on it: along the moved surface's normals,
        // it cuts as deep below the plain clay as it did before.
        let cut = |v: usize| detailed[v].distance(smooth_top[v]);
        let deepest = (0..detailed.len()).map(cut).fold(0.0, f32::max);
        assert!(deepest > 1e-3, "the crease cut {deepest}");
        let creased: Vec<usize> = (0..detailed.len())
            .filter(|&v| cut(v) > deepest * 0.5)
            .collect();
        let before: f32 = creased
            .iter()
            .map(|&v| (smooth_top[v] - detailed[v]).length())
            .sum();
        let after: f32 = creased
            .iter()
            .map(|&v| (plain.mesh.positions[v] - lifted[v]).dot(plain.mesh.normals[v]))
            .sum();
        assert!(
            (after - before).abs() < before * 0.3,
            "crease depth {before} → {after}"
        );
    }

    #[test]
    fn sculpting_at_the_top_shows_at_the_levels_below() {
        let mut s = leveled();
        let base = {
            let mut plain = leveled();
            plain.change_level(false);
            plain.change_level(false);
            plain.mesh.positions.clone()
        };
        // A broad swelling at the top level.
        s.begin_stroke();
        s.grab(Vec3::Z, 0.9, false);
        s.drag(Vec3::new(0.0, 0.0, 0.4));
        s.end_stroke();
        s.change_level(false);
        s.change_level(false);
        let top = (0..base.len())
            .min_by(|&a, &b| {
                base[a]
                    .distance(Vec3::Z)
                    .total_cmp(&base[b].distance(Vec3::Z))
            })
            .unwrap();
        assert!(s.mesh.positions[top].z > base[top].z + 0.3);
    }

    #[test]
    fn undo_and_redo_go_through_levels() {
        let mut s = Sculpt::new(Mesh::icosphere(2));
        let coarse = s.mesh.positions.clone();
        assert!(s.subdivide());
        let fine = s.mesh.positions.clone();
        stroke(&mut s, Brush::Clay, Vec3::new(0.1, 0.2, 0.95), 0.2);
        let sculpted = s.mesh.positions.clone();
        assert!(s.change_level(false));
        assert_eq!(s.mesh.positions.len(), coarse.len());
        // Undo goes back up, with the stroke.
        s.undo();
        assert_eq!(s.levels(), Some((1, 2)));
        assert!(worst(&s.mesh.positions, &sculpted) < 1e-5);
        assert!(!s.can_undo(), "strokes before the switch are gone");
        s.redo();
        assert_eq!(s.levels(), Some((0, 2)));
        s.change_level(true);
        // Strokes after the subdivision undo, then the subdivision itself.
        let mut s = Sculpt::new(Mesh::icosphere(2));
        s.subdivide();
        stroke(&mut s, Brush::Clay, Vec3::new(0.1, 0.2, 0.95), 0.2);
        s.undo();
        assert_eq!(s.mesh.positions, fine);
        s.undo();
        assert_eq!(s.levels(), None);
        assert_eq!(s.mesh.positions, coarse);
        s.redo();
        assert_eq!(s.mesh.positions, fine);
        s.redo();
        assert_eq!(s.mesh.positions, sculpted);
    }

    #[test]
    fn deleting_levels_keeps_the_clay_and_undoes() {
        let mut s = leveled();
        s.change_level(false);
        let clay = s.mesh.positions.clone();
        stroke(&mut s, Brush::Clay, Vec3::new(0.1, 0.2, 0.95), 0.2);
        assert!(s.delete_levels(true));
        assert_eq!(s.levels(), Some((1, 2)));
        assert!(!s.change_level(true));
        assert!(s.delete_levels(false));
        assert_eq!(s.levels(), None);
        s.undo();
        assert_eq!(s.levels(), Some((1, 2)));
        s.undo();
        assert_eq!(s.levels(), Some((1, 3)));
        s.undo();
        assert_eq!(s.mesh.positions, clay, "the stroke undoes too");
        s.redo();
        s.redo();
        assert_eq!(s.levels(), Some((1, 2)));
        // Deleting the lower levels makes this one the base.
        assert!(s.delete_levels(false));
        assert!(s.subdivide());
        assert_eq!(s.levels(), Some((1, 2)));
    }

    #[test]
    fn a_new_mesh_drops_the_levels_and_undo_brings_them_back() {
        let mut s = leveled();
        s.change_level(false);
        let levels = s.level_data().cloned();
        s.replace_mesh(Mesh::icosphere(1));
        assert_eq!(s.levels(), None);
        s.undo();
        assert_eq!(s.level_data().cloned(), levels);
        assert!(s.change_level(true));
    }

    /// Subdivisions, level changes both ways with strokes at each level,
    /// undo and redo of each kind, deletions, and a remesh undone.
    fn session() -> Sculpt {
        let mut s = Sculpt::new(Mesh::icosphere(2));
        let mut clock = 0.0;
        let mut tick = |s: &mut Sculpt| {
            clock += 1.0;
            s.set_clock(clock);
        };
        tick(&mut s);
        s.subdivide();
        s.edit_layer(LayerOp::AddChalk);
        stroke(&mut s, Brush::Mask, Vec3::new(-0.3, 0.2, 0.9), 0.3);
        stroke(&mut s, Brush::Clay, Vec3::new(0.2, 0.3, 0.9), 0.3);
        tick(&mut s);
        s.subdivide();
        stroke(&mut s, Brush::Crease, Vec3::new(-0.1, 0.0, 1.0), 0.1);
        tick(&mut s);
        s.change_level(false);
        stroke(&mut s, Brush::Inflate, Vec3::new(0.0, -0.4, 0.9), 0.4);
        tick(&mut s);
        s.change_level(false);
        s.begin_stroke();
        s.grab(Vec3::Y, 0.6, true);
        s.drag(Vec3::new(0.05, 0.2, 0.0));
        s.end_stroke();
        s.undo();
        s.redo();
        tick(&mut s);
        s.change_level(true);
        s.undo();
        s.redo();
        stroke(&mut s, Brush::Smooth, Vec3::new(0.3, 0.0, 0.9), 0.3);
        tick(&mut s);
        s.change_level(true);
        stroke(&mut s, Brush::Pinch, Vec3::new(0.1, 0.1, 0.95), 0.15);
        tick(&mut s);
        s.subdivide();
        s.undo();
        s.redo();
        s.undo();
        tick(&mut s);
        s.change_level(false);
        s.delete_levels(true);
        s.undo();
        s.change_level(true);
        tick(&mut s);
        s.replace_mesh(Mesh::icosphere(1));
        s.undo();
        stroke(&mut s, Brush::Flatten, Vec3::new(0.0, 0.5, 0.8), 0.2);
        s
    }

    fn assert_same(a: &Sculpt, b: &Sculpt) {
        assert_eq!(a.mesh.triangles, b.mesh.triangles);
        assert_eq!(a.mesh.positions, b.mesh.positions);
        assert_eq!(a.mesh.normals, b.mesh.normals);
        assert_eq!(a.mesh.layers, b.mesh.layers);
        assert_eq!(a.levels, b.levels);
    }

    #[test]
    fn the_mask_and_chalk_go_up_and_down_the_levels_and_undo() {
        let mut s = Sculpt::new(Mesh::icosphere(2));
        let n = s.mesh.positions.len();
        stroke(&mut s, Brush::Mask, Vec3::new(0.2, 0.3, 0.9), 0.4);
        s.edit_layer(LayerOp::AddChalk);
        let base = s.mesh.layers.clone();
        assert!(!base.mask.is_empty());
        assert!(s.subdivide());
        let fine = s.mesh.layers.clone();
        assert!(fine.fits(s.mesh.positions.len()));
        for v in 0..n {
            assert_eq!(fine.mask.get(v), base.mask.get(v));
        }
        assert!(fine.chalk.values().iter().all(|&c| c == 1.0));
        // A new vertex between a masked and a free one is half masked.
        let between =
            (n..s.mesh.positions.len()).any(|v| fine.mask.get(v) > 0.0 && fine.mask.get(v) < 1.0);
        assert!(between);
        // Painted at the top, the mask comes down to the base's vertices.
        stroke(&mut s, Brush::Mask, Vec3::new(-0.4, -0.2, 0.9), 0.3);
        let painted = s.mesh.layers.clone();
        assert!(s.change_level(false));
        assert_eq!(s.mesh.positions.len(), n);
        for v in 0..n {
            assert_eq!(s.mesh.layers.mask.get(v), painted.mask.get(v));
        }
        assert!(s.mesh.layers.fits(n));
        // Back up, the vertices from below keep theirs; those between take
        // the average again.
        s.undo();
        assert_eq!(s.mesh.positions.len(), painted.mask.values().len());
        for v in 0..n {
            assert_eq!(s.mesh.layers.mask.get(v), painted.mask.get(v));
        }
        assert!(!s.can_undo(), "a level change starts the undo afresh");
    }

    #[test]
    fn a_blurred_mask_goes_down_the_levels_as_a_painted_one_and_replays() {
        let mut s = Sculpt::new(Mesh::icosphere(2));
        let n = s.mesh.positions.len();
        stroke(&mut s, Brush::Mask, Vec3::new(0.2, 0.3, 0.9), 0.4);
        assert!(s.subdivide());
        let painted = s.mesh.layers.clone();
        // Blurred at the top, the mask comes down to the base's vertices.
        stroke(&mut s, Brush::BlurMask, Vec3::new(0.0, 0.3, 0.9), 0.4);
        let blurred = s.mesh.layers.clone();
        assert_ne!(blurred, painted);
        assert!((0..n).any(|v| blurred.mask.get(v) != painted.mask.get(v)));
        assert!(s.change_level(false));
        for v in 0..n {
            assert_eq!(s.mesh.layers.mask.get(v), blurred.mask.get(v));
        }
        // Blurred below, then up again: the vertices from below keep theirs.
        stroke(&mut s, Brush::BlurMask, Vec3::new(0.3, 0.2, 0.9), 0.4);
        let below = s.mesh.layers.clone();
        assert!(s.change_level(true));
        for v in 0..n {
            assert_eq!(s.mesh.layers.mask.get(v), below.mask.get(v));
        }
        let mut replay = Replay::of(&s);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_same(&replay.sculpt, &s);
    }

    #[test]
    fn replay_through_level_changes_is_bit_exact() {
        let live = session();
        assert!(live.levels().is_some());
        let mut replay = Replay::of(&live);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_same(&replay.sculpt, &live);
        // Scrubbing back and forth lands on what a fresh replay gives.
        let steps = 8;
        for k in (0..=steps).rev() {
            let time = replay.duration() * k as f64 / steps as f64;
            let mut fresh = Replay::of(&live);
            assert!(fresh.seek(time, usize::MAX));
            assert!(replay.seek(time, usize::MAX));
            assert_same(&replay.sculpt, &fresh.sculpt);
        }
        while !replay.seek(f64::INFINITY, 30) {}
        assert_same(&replay.sculpt, &live);
    }

    #[test]
    fn levels_round_trip_through_bytes() {
        let live = session();
        let levels = live.level_data().unwrap();
        let mut w = Writer::new();
        levels.write(&mut w, &live.mesh.positions);
        let bytes = w.finish();
        let read = Levels::read(
            &mut Reader::new(&bytes),
            &live.mesh.positions,
            &live.mesh.triangles,
        )
        .unwrap();
        assert_eq!(&read, levels);
        // Cut short anywhere, or written for other clay: an error.
        for end in (0..bytes.len()).step_by(41) {
            assert!(
                Levels::read(
                    &mut Reader::new(&bytes[..end]),
                    &live.mesh.positions,
                    &live.mesh.triangles
                )
                .is_err()
            );
        }
        let other = Mesh::icosphere(3);
        assert!(
            Levels::read(&mut Reader::new(&bytes), &other.positions, &other.triangles).is_err()
        );
        // Sizes past the limit are refused before anything is allocated.
        let mut w = Writer::new();
        w.u32(0).len(2).len(10).len(MAX_VERTICES + 1);
        assert!(Levels::read(&mut Reader::new(&w.finish()), &live.mesh.positions, &[]).is_err());
    }

    /// What going up and down costs near the vertex limit.
    /// `cargo test --release -p splatstudy-core level_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn level_timing() {
        use std::time::Instant;
        let ms = |since: Instant| since.elapsed().as_secs_f64() * 1e3;
        // A remeshed sphere of about 35,000 vertices, subdivided three
        // times to about 2.2 million, a little past the app's limit (1.5).
        let base = crate::voxel::remesh(&Mesh::icosphere(4), 86).unwrap();
        println!("base: {} vertices", base.positions.len());
        let mut s = Sculpt::new(base);
        for _ in 0..3 {
            let start = Instant::now();
            s.subdivide();
            println!(
                "subdivide to {} vertices: {:.0} ms",
                s.mesh.positions.len(),
                ms(start)
            );
        }
        stroke(&mut s, Brush::Clay, Vec3::new(0.2, 0.3, 0.9), 0.2);
        for (label, up) in [
            ("down after sculpting", false),
            ("up", true),
            ("down, detail kept", false),
            ("down", false),
            ("up", true),
            ("up to the top", true),
        ] {
            let start = Instant::now();
            s.change_level(up);
            println!(
                "{label}: {:.0} ms, now {} vertices",
                ms(start),
                s.mesh.positions.len()
            );
        }
        // Sculpting at the top: the levels keep each level's vertices
        // after the one below's, not in the spatial order a new mesh gets.
        let dabs = |s: &mut Sculpt| {
            let start = Instant::now();
            s.begin_stroke();
            for i in 0..120 {
                s.dab(dab(
                    Brush::Clay,
                    Vec3::new(-0.5 + i as f32 * 0.008, 0.2, 0.9).normalize(),
                    0.2,
                ));
                if i % 4 == 3 {
                    s.refresh();
                }
            }
            s.end_stroke();
            ms(start) / 30.0
        };
        let leveled = dabs(&mut s);
        let mut spatial = Sculpt::new(Mesh::new(
            s.mesh.positions.clone(),
            s.mesh.triangles.clone(),
        ));
        let reordered = dabs(&mut spatial);
        println!(
            "a frame of four dabs at the top: {leveled:.1} ms; in spatial order {reordered:.1} ms"
        );
        let levels = s.level_data().unwrap();
        let bytes: usize = levels
            .below
            .iter()
            .map(|b| b.positions.len() * 12)
            .sum::<usize>()
            + levels
                .below
                .iter()
                .filter_map(|b| b.detail.as_ref())
                .map(|d| d.0.len() * 12)
                .sum::<usize>()
            + levels.above.iter().map(|d| d.0.len() * 12).sum::<usize>()
            + levels.arrival.len() * 12
            + levels.detail.as_ref().map_or(0, |d| d.0.len() * 12);
        println!("levels hold {:.1} MB", bytes as f64 / 1e6);
    }
}
