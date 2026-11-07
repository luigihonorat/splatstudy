// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The record of a session: every call that changed the clay, with the time
//! it was made. The brushes are deterministic, so applying the same calls to
//! the same start mesh rebuilds the study step by step, as a timelapse.
//!
//! Checkpoints hold whole meshes: the start, every mesh that replaced the
//! clay (remesh, import), and every few minutes the positions alone. A
//! replay starts from the nearest one before where it seeks, and snaps to
//! each one it passes.

use std::sync::Arc;

use glam::Vec3;

use super::subdivide::parents;
use super::{
    Brush, Buildup, Bvh, Dab, Falloff, LayerOp, Layers, Levels, MAX_HARDNESS, MAX_TRIANGLES,
    MAX_VERTICES, Mesh, OLD_START_LEVEL, Resume, Sculpt,
};
use crate::io::{ReadError, Reader, Writer};

/// Pauses longer than this, in seconds, are shortened to it in playback.
const MAX_GAP: f64 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Begin,
    Dab(Dab),
    /// A symmetric dab from a log made before both sides took the larger
    /// of their weights: where they overlap, it applies both in full (see
    /// `Sculpt::summed_dab`). Only read from old logs, so that they replay
    /// as they were sculpted.
    SummedDab(Dab),
    Grab {
        centre: Vec3,
        radius: f32,
        symmetry: bool,
    },
    Drag(Vec3),
    End,
    Undo,
    Redo,
    /// The hierarchy was refitted (see `Sculpt::refresh`).
    Refresh,
    /// The clay became checkpoint number `n` (see [`Sculpt::checkpoints`]).
    Checkpoint(u32),
    /// One half was copied onto the other (see `Sculpt::mirror`).
    Mirror {
        to_negative: bool,
    },
    /// The brushes fade this way from the next dab or grab on (see
    /// `Sculpt::set_falloff`). Every checkpoint goes back to the default.
    /// Stored as its own byte (see `falloff_byte`).
    Falloff(Falloff),
    /// The scene was reopened here: everything worked out from the clay
    /// (its hierarchy, its normals) was made afresh from it, the undo
    /// history and any open stroke dropped, and the falloff went back to
    /// the default. The clay itself didn't change, so this holds no mesh
    /// (see `Sculpt::with_history`).
    Rebuild,
    /// A subdivision level was added at the top (see `Sculpt::subdivide`).
    Subdivide,
    /// The clay went one subdivision level up or down.
    Level {
        up: bool,
    },
    /// The levels above the clay (`higher`) or below it were deleted.
    DeleteLevels {
        higher: bool,
    },
    /// A whole layer changed: the mask cleared or inverted, the chalk
    /// added or removed (see `Sculpt::edit_layer`).
    Layer(LayerOp),
}

impl Event {
    /// True for the events that change the clay's subdivision levels.
    fn changes_levels(&self) -> bool {
        matches!(
            self,
            Event::Subdivide | Event::Level { .. } | Event::DeleteLevels { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    /// Seconds, on the app's clock.
    pub time: f64,
    pub event: Event,
}

/// The clay at one point of the log.
#[derive(Clone, Debug, PartialEq)]
pub struct Checkpoint {
    /// How many log entries come before it: replaying from here starts at
    /// this entry.
    pub log_index: usize,
    /// Seconds, on the app's clock.
    pub time: f64,
    pub positions: Arc<[Vec3]>,
    /// The triangles, when the mesh changed; `None` keeps the triangles of
    /// the clay at that point (a checkpoint of the positions only).
    pub triangles: Option<Arc<[[u32; 3]]>>,
    /// With the triangles, the clay's other subdivision levels, if it had
    /// any. Saved apart from the rest (see `scene`), so it isn't part of
    /// [`Checkpoint::write`].
    pub levels: Option<Arc<Levels>>,
    /// With the triangles, the clay's quads as
    /// [diagonal codes](crate::sculpt::mesh::diagonals), if it had any:
    /// export and the wireframe show them during the timelapse. Saved
    /// apart, like the levels; they don't change a replay's clay.
    pub diagonals: Option<Arc<[u8]>>,
    /// With the triangles, the clay's mask and chalk, if it had any.
    /// Saved apart too.
    pub layers: Option<Arc<Layers>>,
}

impl Checkpoint {
    pub(super) fn of(
        mesh: &Mesh,
        levels: Option<Arc<Levels>>,
        log_index: usize,
        time: f64,
        topology: bool,
    ) -> Checkpoint {
        Checkpoint {
            log_index,
            time,
            positions: mesh.positions.as_slice().into(),
            triangles: topology.then(|| mesh.triangles.as_slice().into()),
            levels: levels.filter(|_| topology),
            diagonals: topology.then(|| mesh.diagonals()).flatten().map(Into::into),
            layers: (topology && !mesh.layers.is_empty()).then(|| Arc::new(mesh.layers.clone())),
        }
    }

    /// A checkpoint of a whole mesh, without levels.
    pub fn start(mesh: &Mesh) -> Checkpoint {
        Checkpoint::of(mesh, None, 0, 0.0, true)
    }

    /// True if it holds a whole mesh, with triangles that all fit.
    pub fn has_mesh(&self) -> bool {
        let n = self.positions.len();
        self.triangles.as_ref().is_some_and(|triangles| {
            !triangles.is_empty() && triangles.iter().flatten().all(|&v| (v as usize) < n)
        })
    }

    /// The whole mesh and its levels, for a checkpoint that has it.
    fn mesh(&self) -> Option<(Mesh, Option<Levels>)> {
        let triangles = self.triangles.as_ref().filter(|_| self.has_mesh())?;
        let mut mesh = Mesh::from_ordered(self.positions.to_vec(), triangles.to_vec());
        if let Some(codes) = &self.diagonals
            && !mesh.set_diagonals(codes)
        {
            log::warn!("checkpoint quads that don't fit its triangles, left out");
        }
        mesh.layers = self.layers_or_none();
        Some((mesh, self.levels.as_deref().cloned()))
    }

    /// Its mask and chalk, or none if they don't fit its positions.
    fn layers_or_none(&self) -> Layers {
        match &self.layers {
            Some(layers) if layers.fits(self.positions.len()) => (**layers).clone(),
            _ => Layers::default(),
        }
    }

    pub fn write(&self, w: &mut Writer) {
        w.u64(self.log_index as u64).f64(self.time);
        w.len(self.positions.len()).vec3s(&self.positions);
        match &self.triangles {
            Some(triangles) => {
                w.u8(1)
                    .len(triangles.len())
                    .u32s(bytemuck::cast_slice(triangles));
            }
            None => {
                w.u8(0);
            }
        }
    }

    pub fn read(r: &mut Reader) -> Result<Checkpoint, ReadError> {
        let log_index = usize::try_from(r.u64()?).map_err(|_| r.invalid("log index too large"))?;
        let time = r.f64()?;
        let n = r.len(12)?;
        if n > MAX_VERTICES {
            return Err(r.invalid("too many vertices"));
        }
        let positions = (0..n)
            .map(|_| match r.vec3()? {
                p if p.is_finite() => Ok(p),
                _ => Err(r.invalid("a position that isn't a finite number")),
            })
            .collect::<Result<Arc<[Vec3]>, _>>()?;
        let triangles = match r.u8()? {
            0 => None,
            1 => {
                let count = r.len(12)?;
                if count > MAX_TRIANGLES {
                    return Err(r.invalid("too many triangles"));
                }
                let mut triangles = Vec::with_capacity(count);
                for _ in 0..count {
                    let t = [r.u32()?, r.u32()?, r.u32()?];
                    if t.iter().any(|&v| v as usize >= n) {
                        return Err(r.invalid("a triangle corner that isn't a vertex"));
                    }
                    if t[0] == t[1] || t[1] == t[2] || t[2] == t[0] {
                        return Err(r.invalid("a triangle with a corner twice"));
                    }
                    triangles.push(t);
                }
                Some(triangles.into())
            }
            _ => return Err(r.invalid("unknown checkpoint kind")),
        };
        Ok(Checkpoint {
            log_index,
            time,
            positions,
            triangles,
            levels: None,
            diagonals: None,
            layers: None,
        })
    }
}

/// Event tags in the saved log. Never renumber: saved scenes keep them.
mod tag {
    pub const BEGIN: u8 = 0;
    pub const DAB: u8 = 1;
    pub const GRAB: u8 = 2;
    pub const DRAG: u8 = 3;
    pub const END: u8 = 4;
    pub const UNDO: u8 = 5;
    pub const REDO: u8 = 6;
    pub const REFRESH: u8 = 7;
    pub const CHECKPOINT: u8 = 8;
    pub const MIRROR: u8 = 9;
    pub const FALLOFF: u8 = 10;
    pub const REBUILD: u8 = 11;
    pub const SUBDIVIDE: u8 = 12;
    pub const LEVEL: u8 = 13;
    pub const DELETE_LEVELS: u8 = 14;
    pub const LAYER: u8 = 15;
}

impl Entry {
    pub fn write(&self, w: &mut Writer) {
        w.f64(self.time);
        match self.event {
            Event::Begin => {
                w.u8(tag::BEGIN);
            }
            Event::Dab(d) | Event::SummedDab(d) => {
                // Bits 2 and 3 came later: older logs don't have them,
                // which reads back as clay that accumulates and symmetric
                // dabs that add up where they overlap.
                let larger = matches!(self.event, Event::Dab(_)) && d.symmetry;
                // Bit 4 came later still: the mask blur is stored as the
                // Smooth brush with it, smoothing the mask instead of the
                // clay. Older logs never set it, nor bit 5, clay's fill,
                // nor bit 6, a dab that wipes its chalk away at once.
                let (brush, on_mask) = match d.brush {
                    Brush::BlurMask => (Brush::Smooth, true),
                    brush => (brush, false),
                };
                let flags = u8::from(d.invert)
                    | u8::from(d.symmetry) << 1
                    | u8::from(d.buildup == Buildup::Layer) << 2
                    | u8::from(larger) << 3
                    | u8::from(on_mask) << 4
                    | u8::from(d.fill) << 5
                    | u8::from(d.wipes) << 6;
                w.u8(tag::DAB)
                    .u8(brush as u8)
                    .vec3(d.centre)
                    .f32(d.radius)
                    .f32(d.strength)
                    .u8(flags)
                    .vec3(d.direction);
            }
            Event::Grab {
                centre,
                radius,
                symmetry,
            } => {
                w.u8(tag::GRAB)
                    .vec3(centre)
                    .f32(radius)
                    .u8(u8::from(symmetry));
            }
            Event::Drag(offset) => {
                w.u8(tag::DRAG).vec3(offset);
            }
            Event::End => {
                w.u8(tag::END);
            }
            Event::Undo => {
                w.u8(tag::UNDO);
            }
            Event::Redo => {
                w.u8(tag::REDO);
            }
            Event::Refresh => {
                w.u8(tag::REFRESH);
            }
            Event::Checkpoint(n) => {
                w.u8(tag::CHECKPOINT).u32(n);
            }
            Event::Mirror { to_negative } => {
                w.u8(tag::MIRROR).u8(u8::from(to_negative));
            }
            Event::Falloff(f) => {
                w.u8(tag::FALLOFF).u8(falloff_byte(f));
            }
            Event::Rebuild => {
                w.u8(tag::REBUILD);
            }
            Event::Subdivide => {
                w.u8(tag::SUBDIVIDE);
            }
            Event::Level { up } => {
                w.u8(tag::LEVEL).u8(u8::from(up));
            }
            Event::DeleteLevels { higher } => {
                w.u8(tag::DELETE_LEVELS).u8(u8::from(higher));
            }
            Event::Layer(op) => {
                w.u8(tag::LAYER).u8(op as u8);
            }
        }
    }

    pub fn read(r: &mut Reader) -> Result<Entry, ReadError> {
        let time = r.f64()?;
        let flag = |r: &mut Reader| match r.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(r.invalid("a flag that isn't 0 or 1")),
        };
        let event = match r.u8()? {
            tag::BEGIN => Event::Begin,
            tag::DAB => {
                let brush = *Brush::ALL
                    .get(r.u8()? as usize)
                    .ok_or_else(|| r.invalid("unknown brush"))?;
                let (centre, radius, strength) = (r.vec3()?, r.f32()?, r.f32()?);
                let flags = r.u8()?;
                if flags > 127 {
                    return Err(r.invalid("unknown dab flags"));
                }
                // Bit 4: the Smooth brush on the mask (see `write`).
                let brush = match (brush, flags & 16 != 0) {
                    (brush, false) => brush,
                    (Brush::Smooth, true) => Brush::BlurMask,
                    _ => return Err(r.invalid("a brush that can't work on the mask")),
                };
                let dab = Dab {
                    brush,
                    centre,
                    radius,
                    strength,
                    invert: flags & 1 != 0,
                    symmetry: flags & 2 != 0,
                    direction: r.vec3()?,
                    buildup: if flags & 4 != 0 {
                        Buildup::Layer
                    } else {
                        Buildup::Accumulate
                    },
                    fill: flags & 32 != 0,
                    wipes: flags & 64 != 0,
                };
                if dab.symmetry && flags & 8 == 0 {
                    Event::SummedDab(dab)
                } else {
                    Event::Dab(dab)
                }
            }
            tag::GRAB => Event::Grab {
                centre: r.vec3()?,
                radius: r.f32()?,
                symmetry: flag(r)?,
            },
            tag::DRAG => Event::Drag(r.vec3()?),
            tag::END => Event::End,
            tag::UNDO => Event::Undo,
            tag::REDO => Event::Redo,
            tag::REFRESH => Event::Refresh,
            tag::CHECKPOINT => Event::Checkpoint(r.u32()?),
            tag::MIRROR => Event::Mirror {
                to_negative: flag(r)?,
            },
            tag::FALLOFF => Event::Falloff(
                falloff_from_byte(r.u8()?).ok_or_else(|| r.invalid("unknown falloff"))?,
            ),
            tag::REBUILD => Event::Rebuild,
            tag::SUBDIVIDE => Event::Subdivide,
            tag::LEVEL => Event::Level { up: flag(r)? },
            tag::DELETE_LEVELS => Event::DeleteLevels { higher: flag(r)? },
            tag::LAYER => Event::Layer(
                *LayerOp::ALL
                    .get(r.u8()? as usize)
                    .ok_or_else(|| r.invalid("unknown layer edit"))?,
            ),
            _ => return Err(r.invalid("unknown log event")),
        };
        Ok(Entry { time, event })
    }
}

/// The byte a log stores for each falloff. These never change, whatever
/// order the panel lists the falloffs in: a new falloff takes the next
/// free byte here and in [`falloff_from_byte`].
fn falloff_byte(falloff: Falloff) -> u8 {
    match falloff {
        Falloff::Smooth => 0,
        Falloff::Linear => 1,
        // The old sharp, a narrow peak, keeps its byte: the new sharp is
        // another curve, so it takes a byte of its own.
        Falloff::Peak => 2,
        Falloff::Flat => 3,
        Falloff::Sharp => 4,
        // The slider's falloffs: 16 and up, one byte for each step.
        Falloff::Hard(hardness) => HARD_BYTES + hardness.min(MAX_HARDNESS),
    }
}

/// The byte of the softest falloff on the slider, [`Falloff::Hard`]`(0)`.
const HARD_BYTES: u8 = 16;

/// The falloff a log's byte stands for (see [`falloff_byte`]).
fn falloff_from_byte(byte: u8) -> Option<Falloff> {
    Some(match byte {
        0 => Falloff::Smooth,
        1 => Falloff::Linear,
        2 => Falloff::Peak,
        3 => Falloff::Flat,
        4 => Falloff::Sharp,
        b if (HARD_BYTES..=HARD_BYTES + MAX_HARDNESS).contains(&b) => Falloff::Hard(b - HARD_BYTES),
        _ => return None,
    })
}

/// Logs saved by the builds that brought the flat falloff, before it had
/// its own byte, store it as linear's 1. Those builds also brought the
/// build-up bit, which every dab in the default "per stroke" mode sets
/// and older builds never wrote; and a falloff is logged right before the
/// dab or grab that first uses it. So a linear just before such a dab, or
/// anywhere after one, was flat. (Flat used only in "accumulate" mode,
/// or only with Move, still reads as linear.)
fn recover_flat(entries: &mut [Entry]) {
    let layer_dab = |entry: Option<&Entry>| {
        matches!(
            entry,
            Some(Entry {
                event: Event::Dab(dab) | Event::SummedDab(dab),
                ..
            }) if dab.buildup == Buildup::Layer
        )
    };
    let mut layered = false;
    for i in 0..entries.len() {
        layered |= layer_dab(entries.get(i));
        if entries[i].event == Event::Falloff(Falloff::Linear)
            && (layered || layer_dab(entries.get(i + 1)))
        {
            entries[i].event = Event::Falloff(Falloff::Flat);
        }
    }
}

/// A whole log: its length, then each entry.
pub fn write_log(w: &mut Writer, entries: &[Entry]) {
    w.len(entries.len());
    for entry in entries {
        entry.write(w);
    }
}

/// Reads what [`write_log`] wrote.
pub fn read_log(r: &mut Reader) -> Result<Vec<Entry>, ReadError> {
    // Every entry takes at least its time and tag.
    let n = r.len(9)?;
    let mut entries = (0..n)
        .map(|_| Entry::read(r))
        .collect::<Result<Vec<_>, _>>()?;
    recover_flat(&mut entries);
    Ok(entries)
}

/// Every checkpoint: their count, then each one.
pub fn write_checkpoints(w: &mut Writer, checkpoints: &[Checkpoint]) {
    w.len(checkpoints.len());
    for checkpoint in checkpoints {
        checkpoint.write(w);
    }
}

/// Reads what [`write_checkpoints`] wrote.
pub fn read_checkpoints(r: &mut Reader) -> Result<Vec<Checkpoint>, ReadError> {
    let n = r.len(25)?;
    (0..n).map(|_| Checkpoint::read(r)).collect()
}

/// True if a replay can start from these checkpoints: the first holds a
/// whole mesh at the start of the log. Otherwise a replay starts from the
/// default sphere, and a log made on other clay can't be played.
pub fn starts_with_mesh(checkpoints: &[Checkpoint]) -> bool {
    checkpoints
        .first()
        .is_some_and(|c| c.log_index == 0 && c.has_mesh())
}

/// True if replaying `log` from `checkpoints` ends on `mesh`'s triangles:
/// those of the last whole-mesh checkpoint the log snaps to, or of the
/// start. Only then can a replay go on from its own clay where a reopened
/// sculpt goes on from `mesh` (see [`Event::Rebuild`]).
///
/// With subdivision levels on either side, the triangles at every level
/// come from the base's: the bases must match.
pub(super) fn ends_on_triangles_of(
    mesh: &Mesh,
    levels: Option<&Levels>,
    log: &[Entry],
    checkpoints: &[Checkpoint],
) -> bool {
    if !starts_with_mesh(checkpoints) {
        return false;
    }
    // As `Replay::snap` does: a checkpoint without a usable mesh is skipped.
    let (at, last) = log
        .iter()
        .enumerate()
        .rev()
        .filter_map(|(i, e)| match e.event {
            Event::Checkpoint(n) => Some((i, checkpoints.get(n as usize)?)),
            _ => None,
        })
        .find(|(_, c)| c.has_mesh())
        .unwrap_or((0, &checkpoints[0]));
    let triangles = last.triangles.as_deref().unwrap_or_default();
    let releveled = log[at..].iter().any(|e| e.event.changes_levels());
    if !releveled && levels.is_none() && last.levels.is_none() {
        return last.positions.len() == mesh.positions.len()
            && triangles == mesh.triangles.as_slice();
    }
    let ours = base_of(&mesh.triangles, mesh.positions.len(), levels);
    let theirs = base_of(triangles, last.positions.len(), last.levels.as_deref());
    ours.is_some() && ours == theirs
}

/// The base level's vertex count and triangles, for clay with `triangles`
/// and `vertices` at the current one of its `levels`.
fn base_of(
    triangles: &[[u32; 3]],
    vertices: usize,
    levels: Option<&Levels>,
) -> Option<(usize, Vec<[u32; 3]>)> {
    let mut t = triangles.to_vec();
    let Some(levels) = levels else {
        return Some((vertices, t));
    };
    let counts = levels.counts();
    for &count in counts[..levels.current()].iter().rev() {
        t = parents(&t, count)?;
    }
    Some((counts[0], t))
}

impl Sculpt {
    /// A sculpt that keeps no log of its own, for playing one back.
    fn replaying((mesh, levels): (Mesh, Option<Levels>)) -> Sculpt {
        Sculpt {
            recording: false,
            topology_changed: true,
            levels,
            ..Sculpt::bare(mesh)
        }
    }

    /// Back to `positions` and `layers` on the same triangles: the state
    /// [`Sculpt::replaying`] would make of the checkpoint this sculpt was
    /// built from, without building it again. Unless `reshaped`, the
    /// hierarchy's shape came from those same positions, so a refit gives
    /// the boxes a new build would; after an [`Event::Rebuild`] it came from
    /// others, so it is built again. The GPU buffers keep their layout:
    /// every page is just dirty.
    fn rewind(&mut self, positions: &[Vec3], layers: Layers, reshaped: bool) {
        self.mesh.positions.copy_from_slice(positions);
        self.mesh.layers = layers;
        self.mesh.recompute_normals();
        if reshaped {
            self.bvh = Bvh::build(&self.mesh);
        } else {
            self.bvh.refit(&self.mesh);
        }
        self.bvh_stale = false;
        self.dirty_pages.fill(true);
        self.stroke_open = false;
        self.stroke_indices.clear();
        self.stroke_before.clear();
        self.stroke_layers_before.clear();
        self.grabbed.clear();
        self.undo.clear();
        self.redo.clear();
        self.clock = 0.0;
        self.last_checkpoint = 0.0;
        self.falloff = Falloff::Smooth;
        self.logged_falloff = Falloff::Smooth;
        self.resume = Resume::Settled;
    }

    /// Does what a log entry recorded. Checkpoints need the checkpoint
    /// itself: [`Replay`] applies those.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Begin => self.begin_stroke(),
            Event::Dab(dab) => self.dab(dab),
            Event::SummedDab(dab) => self.summed_dab(dab),
            Event::Grab {
                centre,
                radius,
                symmetry,
            } => self.grab(centre, radius, symmetry),
            Event::Drag(offset) => self.drag(offset),
            Event::End => self.end_stroke(),
            Event::Undo => self.undo(),
            Event::Redo => self.redo(),
            Event::Refresh => self.refresh(),
            Event::Checkpoint(_) => {}
            Event::Mirror { to_negative } => self.mirror(to_negative),
            Event::Falloff(falloff) => self.falloff = falloff,
            Event::Rebuild => self.rebuild(),
            Event::Subdivide => {
                self.subdivide();
            }
            Event::Level { up } => {
                self.change_level(up);
            }
            Event::DeleteLevels { higher } => {
                self.delete_levels(higher);
            }
            Event::Layer(op) => {
                self.edit_layer(op);
            }
        }
    }

    /// What reopening a scene does: the state `Sculpt::with_history` makes
    /// of the clay as it is, so a replay goes on exactly as the reopened
    /// sculpt did.
    fn rebuild(&mut self) {
        self.mesh.recompute_normals();
        self.bvh = Bvh::build(&self.mesh);
        self.bvh_stale = false;
        self.dirty_pages.fill(true);
        self.stroke_open = false;
        self.stroke_indices.clear();
        self.stroke_before.clear();
        self.stroke_layers_before.clear();
        self.grabbed.clear();
        self.undo.clear();
        self.redo.clear();
        self.falloff = Falloff::Smooth;
    }
}

/// Plays a log back from its checkpoints.
pub struct Replay {
    /// The clay as far as it has been played.
    pub sculpt: Sculpt,
    checkpoints: Vec<Checkpoint>,
    /// The checkpoints a replay can start from: those with triangles and a
    /// log index inside the log, in log order. Never empty: the first is
    /// where the log starts.
    starts: Vec<usize>,
    entries: Vec<Entry>,
    /// When each entry plays, from 0: the recorded times with long pauses shortened.
    times: Vec<f64>,
    /// How many entries have been applied.
    next: usize,
    /// The checkpoint whose mesh the clay was last built from (at a
    /// restart, or a snap to new triangles): going back to it only needs
    /// its positions again.
    built_from: usize,
    /// An [`Event::Rebuild`] was played since: the hierarchy no longer has
    /// the shape a build from `built_from` gives.
    reshaped: bool,
}

impl Replay {
    /// Plays back what `sculpt` recorded.
    pub fn of(sculpt: &Sculpt) -> Replay {
        Replay::new(sculpt.checkpoints().to_vec(), sculpt.log().to_vec())
    }

    /// A replay of `entries` from `checkpoints`, the first being the start
    /// mesh. Without a usable first checkpoint (a damaged file), it starts
    /// from the default sphere.
    pub fn new(mut checkpoints: Vec<Checkpoint>, entries: Vec<Entry>) -> Replay {
        let mut times = Vec::with_capacity(entries.len());
        let mut t = 0.0;
        for (i, entry) in entries.iter().enumerate() {
            if i > 0 {
                let gap = entry.time - entries[i - 1].time;
                // Backwards (a reopened scene's new clock) or not a number: no pause.
                t += if gap > 0.0 { gap.min(MAX_GAP) } else { 0.0 };
            }
            times.push(t);
        }
        let usable = |c: &Checkpoint| c.log_index <= entries.len() && c.has_mesh();
        if !starts_with_mesh(&checkpoints) {
            let sphere = Mesh::icosphere(OLD_START_LEVEL);
            checkpoints.insert(0, Checkpoint::start(&sphere));
        }
        // Sorted by log index, the first is checkpoint 0, at the start.
        let mut starts: Vec<usize> = (0..checkpoints.len())
            .filter(|&i| usable(&checkpoints[i]))
            .collect();
        starts.sort_by_key(|&i| checkpoints[i].log_index);
        let built_from = starts[0];
        let start = checkpoints[built_from].mesh().expect("checked above");
        Replay {
            sculpt: Sculpt::replaying(start),
            checkpoints,
            starts,
            entries,
            times,
            next: 0,
            built_from,
            reshaped: false,
        }
    }

    /// Starts over from checkpoint `index`.
    fn restart(&mut self, index: usize) {
        let checkpoint = &self.checkpoints[index];
        self.next = checkpoint.log_index;
        if index == self.built_from && !self.sculpt.releveled {
            // Scrubbing back within the same mesh and levels: no new
            // adjacency, hierarchy or GPU buffers.
            let layers = checkpoint.layers_or_none();
            self.sculpt
                .rewind(&checkpoint.positions, layers, self.reshaped);
        } else {
            let mesh = checkpoint.mesh().expect("starts have triangles");
            self.sculpt = Sculpt::replaying(mesh);
            self.built_from = index;
        }
        self.reshaped = false;
    }

    /// Does what checkpoint `n` holds: becomes its mesh, or snaps to its
    /// positions. Ignores a checkpoint that doesn't fit (a damaged file).
    fn snap(&mut self, n: u32) {
        let Some(checkpoint) = self.checkpoints.get(n as usize) else {
            return;
        };
        // As a replay starting here would have it.
        self.sculpt.falloff = Falloff::Smooth;
        if checkpoint.triangles.is_none() {
            self.sculpt.snap_positions(&checkpoint.positions);
        } else if let Some((mesh, levels)) = checkpoint.mesh() {
            self.built_from = n as usize;
            self.reshaped = false;
            self.sculpt.swap_mesh(mesh);
            self.sculpt.levels = levels;
            self.sculpt.releveled = false;
            self.sculpt.undo.clear();
            self.sculpt.redo.clear();
        }
    }

    /// Playback length, in seconds.
    pub fn duration(&self) -> f64 {
        self.times.last().copied().unwrap_or(0.0)
    }

    /// Playback time reached so far.
    pub fn position(&self) -> f64 {
        self.next.checked_sub(1).map_or(0.0, |i| self.times[i])
    }

    pub fn finished(&self) -> bool {
        self.next == self.entries.len()
    }

    /// Plays up to `time`. Behind, or far ahead past a checkpoint, it
    /// starts again from the last checkpoint before `time`. Applies at most
    /// `budget` entries, so a long jump spreads over several frames; true
    /// once `time` is reached.
    pub fn seek(&mut self, time: f64, budget: usize) -> bool {
        let target = self.times.partition_point(|&t| t <= time);
        let start = self
            .starts
            .iter()
            .rev()
            .copied()
            .find(|&i| self.checkpoints[i].log_index <= target)
            .unwrap_or(self.starts[0]);
        if time < self.position() || self.checkpoints[start].log_index > self.next {
            self.restart(start);
        }
        for _ in 0..budget {
            match self.times.get(self.next) {
                Some(&t) if t <= time => {
                    match self.entries[self.next].event {
                        Event::Checkpoint(n) => self.snap(n),
                        Event::Rebuild => {
                            self.sculpt.apply(Event::Rebuild);
                            self.reshaped = true;
                        }
                        event => self.sculpt.apply(event),
                    }
                    self.next += 1;
                }
                _ => return true,
            }
        }
        self.times.get(self.next).is_none_or(|&t| t > time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voxel;

    /// A session that uses every brush, Move, undo and redo, with refits at
    /// uneven moments as frames would make them. `at` shifts the strokes.
    fn session_at(s: &mut Sculpt, start: f64, shift: f32) -> f64 {
        let mut clock = start;
        for (k, brush) in Brush::ALL.into_iter().enumerate() {
            clock += 1.5;
            s.set_clock(clock);
            let at = Vec3::new(0.3 - k as f32 * 0.1, 0.2 + shift, 0.9).normalize();
            s.set_falloff(Falloff::ALL[k % Falloff::ALL.len()]);
            s.begin_stroke();
            if brush == Brush::Move {
                s.grab(at, 0.3, true);
                for i in 1..5 {
                    s.drag(Vec3::new(0.0, 0.02, 0.03) * i as f32);
                    if i % 2 == 0 {
                        s.refresh();
                    }
                }
            } else {
                for i in 0..8 {
                    s.set_clock(clock + i as f64 * 0.02);
                    s.dab(Dab {
                        brush,
                        centre: (at + Vec3::Y * (i as f32 * 0.03)).normalize(),
                        radius: 0.25,
                        strength: 0.7,
                        invert: i % 3 == 2,
                        symmetry: true,
                        direction: if i == 0 { Vec3::ZERO } else { Vec3::Y },
                        // Both build-ups, so the log and the replay carry
                        // them.
                        buildup: Buildup::ALL[k % Buildup::ALL.len()],
                        fill: k % 3 == 1,
                        wipes: k % 2 == 0,
                    });
                    if i % 3 == 0 {
                        s.refresh();
                    }
                }
            }
            s.end_stroke();
            s.refresh();
        }
        // A long pause, then second thoughts.
        clock += 60.0;
        s.set_clock(clock);
        s.undo();
        s.undo();
        s.redo();
        clock
    }

    /// The clay as a scene saves and reopens it: in its order, with its
    /// mask and chalk.
    fn saved(s: &Sculpt) -> Mesh {
        let mut mesh = Mesh::from_ordered(s.mesh.positions.clone(), s.mesh.triangles.clone());
        mesh.layers = s.mesh.layers.clone();
        mesh
    }

    fn session(s: &mut Sculpt) {
        session_at(s, 0.0, 0.0);
    }

    fn assert_same(replay: &Replay, live: &Sculpt) {
        assert_eq!(replay.sculpt.mesh.triangles, live.mesh.triangles);
        assert_eq!(replay.sculpt.mesh.positions, live.mesh.positions);
        assert_eq!(replay.sculpt.mesh.normals, live.mesh.normals);
        assert_same_layers(&replay.sculpt.mesh.layers, &live.mesh.layers);
    }

    /// The same mask and chalk, bit for bit.
    fn assert_same_layers(a: &Layers, b: &Layers) {
        let bits = |l: &crate::sculpt::Layer, n: usize| -> Vec<u32> {
            (0..n).map(|v| l.get(v).to_bits()).collect()
        };
        let n = a.mask.values().len().max(b.mask.values().len());
        assert_eq!(bits(&a.mask, n), bits(&b.mask, n), "mask");
        let n = a.chalk.values().len().max(b.chalk.values().len());
        assert_eq!(bits(&a.chalk, n), bits(&b.chalk, n), "chalk");
    }

    /// Chalk added, a mask painted and taken off in part, every brush
    /// over both, the mask inverted, cleared and brought back, the chalk
    /// removed and brought back.
    fn layered_session(s: &mut Sculpt, start: f64) -> f64 {
        let mut clock = start + 1.0;
        s.set_clock(clock);
        assert!(s.edit_layer(LayerOp::AddChalk));
        for (k, invert) in [false, false, true].into_iter().enumerate() {
            clock += 1.0;
            s.set_clock(clock);
            s.begin_stroke();
            for i in 0..6 {
                s.dab(Dab {
                    brush: Brush::Mask,
                    centre: Vec3::new(0.25 - 0.1 * k as f32, 0.05 * i as f32, 0.95).normalize(),
                    radius: 0.2,
                    strength: 0.6,
                    invert,
                    symmetry: k == 1,
                    direction: if i == 0 { Vec3::ZERO } else { Vec3::Y },
                    buildup: Default::default(),
                    fill: false,
                    wipes: false,
                });
                if i % 2 == 0 {
                    s.refresh();
                }
            }
            s.end_stroke();
        }
        clock = session_at(s, clock, 0.05);
        s.set_clock(clock + 1.0);
        assert!(s.edit_layer(LayerOp::InvertMask));
        clock = session_at(s, clock + 2.0, -0.05);
        s.set_clock(clock + 1.0);
        assert!(s.edit_layer(LayerOp::ClearMask));
        s.undo();
        s.redo();
        s.undo();
        assert!(s.edit_layer(LayerOp::RemoveChalk));
        s.undo();
        clock + 2.0
    }

    #[test]
    fn replay_with_mask_and_chalk_equals_the_live_sculpt_bit_for_bit() {
        let mut live = Sculpt::new(Mesh::icosphere(4));
        let clock = layered_session(&mut live, 0.0);
        assert!(!live.mesh.layers.mask.is_empty() && !live.mesh.layers.chalk.is_empty());
        // Then a remesh, which takes the layers along in its checkpoint,
        // and more of the same on it.
        live.set_clock(clock);
        let mut remeshed = voxel::remesh(&live.mesh, 40).expect("remesh");
        remeshed.layers = live.layers_for(&remeshed);
        assert!(!remeshed.layers.chalk.is_empty());
        live.replace_mesh(remeshed);
        layered_session(&mut live, clock + 1.0);
        let mut replay = Replay::of(&live);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_same(&replay, &live);

        // Seeking back to the start of each mesh, and forward in small
        // steps, gives what a fresh replay gives at every point.
        let steps = 9;
        for k in (0..=steps).rev() {
            let time = replay.duration() * k as f64 / steps as f64;
            let mut fresh = Replay::of(&live);
            assert!(fresh.seek(time, usize::MAX));
            assert!(replay.seek(time, usize::MAX));
            assert_same(&replay, &fresh.sculpt);
        }
        while !replay.seek(f64::INFINITY, 30) {}
        assert_same(&replay, &live);

        // The log, layer edits and all, saves and reads back.
        let mut w = Writer::new();
        write_log(&mut w, live.log());
        assert_eq!(read_log(&mut Reader::new(&w.finish())).unwrap(), live.log());
    }

    /// A stroke of `brush` down the front, at `x`, one second after the
    /// last.
    fn mask_stroke(s: &mut Sculpt, brush: Brush, x: f32, symmetry: bool, clock: &mut f64) {
        *clock += 1.0;
        s.set_clock(*clock);
        s.begin_stroke();
        for i in 0..6 {
            s.dab(Dab {
                brush,
                centre: Vec3::new(x, 0.05 * i as f32 - 0.1, 0.95).normalize(),
                radius: 0.2,
                strength: 0.7,
                invert: false,
                symmetry,
                direction: if i == 0 { Vec3::ZERO } else { Vec3::Y },
                buildup: Default::default(),
                fill: false,
                wipes: false,
            });
            if i % 2 == 0 {
                s.refresh();
            }
        }
        s.end_stroke();
    }

    #[test]
    fn a_mask_blur_stroke_replays_to_the_same_mask_bit_for_bit() {
        let mut live = Sculpt::new(Mesh::icosphere(4));
        let mut clock = 0.0;
        mask_stroke(&mut live, Brush::Mask, 0.2, false, &mut clock);
        let painted = live.mesh.layers.clone();
        let positions = live.mesh.positions.clone();
        // Over the mask's edge, at two falloffs, the second with symmetry.
        live.set_falloff(Falloff::Sharp);
        mask_stroke(&mut live, Brush::BlurMask, 0.3, false, &mut clock);
        assert_ne!(live.mesh.layers, painted, "the blur changed nothing");
        live.set_falloff(Falloff::Smooth);
        mask_stroke(&mut live, Brush::BlurMask, 0.1, true, &mut clock);
        assert_eq!(live.mesh.positions, positions, "the blur moved the clay");
        let blurred = live.mesh.layers.clone();
        // One undo step a stroke, as the mask brush's.
        live.undo();
        live.undo();
        assert_eq!(live.mesh.layers, painted);
        live.redo();
        live.redo();
        assert_eq!(live.mesh.layers, blurred);
        // Sculpting on, held back by the blurred mask.
        mask_stroke(&mut live, Brush::Inflate, 0.2, false, &mut clock);

        // The log saves and reads back as it was...
        let mut w = Writer::new();
        write_log(&mut w, live.log());
        let log = read_log(&mut Reader::new(&w.finish())).unwrap();
        assert_eq!(log, live.log());
        let blurs = log
            .iter()
            .filter(|e| matches!(e.event, Event::Dab(dab) if dab.brush == Brush::BlurMask));
        assert_eq!(blurs.count(), 12);
        // ...and replays to the same mask and clay, bit for bit, also
        // scrubbing back and forth.
        let mut replay = Replay::new(live.checkpoints().to_vec(), log);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_same(&replay, &live);
        let steps = 7;
        for k in (0..=steps).rev() {
            let time = replay.duration() * k as f64 / steps as f64;
            let mut fresh = Replay::of(&live);
            assert!(fresh.seek(time, usize::MAX));
            assert!(replay.seek(time, usize::MAX));
            assert_same(&replay, &fresh.sculpt);
        }
        while !replay.seek(f64::INFINITY, 5) {}
        assert_same(&replay, &live);
    }

    #[test]
    fn a_mask_blur_is_logged_as_a_smooth_dab_on_the_mask() {
        let dab = Dab {
            brush: Brush::BlurMask,
            centre: Vec3::Z,
            radius: 0.2,
            strength: 0.5,
            invert: false,
            symmetry: true,
            direction: Vec3::X,
            buildup: Buildup::Layer,
            fill: false,
            wipes: false,
        };
        let bytes = log_bytes(&[Event::Dab(dab)]);
        // From the end: the direction after the flags, and the centre,
        // radius and strength between the brush and the flags.
        let n = bytes.len();
        let (brush_at, flags_at) = (n - 12 - 1 - 4 - 4 - 12 - 1, n - 12 - 1);
        assert_eq!(bytes[brush_at], Brush::Smooth as u8);
        assert_eq!(bytes[flags_at], 16 | 8 | 4 | 2);
        assert_eq!(read_events(&bytes).unwrap(), [Event::Dab(dab)]);
        // Without the bit, as every older log wrote dabs: a smooth dab.
        let mut old = bytes.clone();
        old[flags_at] &= 15;
        let smooth = Dab {
            brush: Brush::Smooth,
            ..dab
        };
        assert_eq!(read_events(&old).unwrap(), [Event::Dab(smooth)]);
        // The panel's brushes keep their bytes and never set the bit; on
        // any but smooth, it is an error.
        for brush in Brush::ALL {
            let mut bytes = log_bytes(&[Event::Dab(Dab { brush, ..dab })]);
            assert_eq!(bytes[brush_at], brush as u8, "{brush:?}");
            assert_eq!(bytes[flags_at] & 16, 0, "{brush:?}");
            bytes[flags_at] |= 16;
            let read = read_events(&bytes);
            if brush == Brush::Smooth {
                assert_eq!(read.unwrap(), [Event::Dab(dab)]);
            } else {
                assert!(read.is_err(), "{brush:?}");
            }
        }
        // It has no brush byte of its own, and the bit above the chalk's
        // wipe (bit 6) is unknown.
        let mut unknown = old;
        unknown[brush_at] = Brush::ALL.len() as u8;
        assert!(read_events(&unknown).is_err());
        let mut high = bytes;
        high[flags_at] |= 128;
        assert!(read_events(&high).is_err());
    }

    #[test]
    fn replay_rebuilds_the_session_exactly() {
        let mut live = Sculpt::new(Mesh::icosphere(4));
        session(&mut live);
        let mut replay = Replay::of(&live);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert!(replay.finished());
        assert_same(&replay, &live);
        assert!(replay.sculpt.log().is_empty(), "playback keeps no log");
    }

    #[test]
    fn seeking_back_and_in_small_steps_gives_the_same_clay() {
        let mut live = Sculpt::new(Mesh::icosphere(4));
        session(&mut live);
        let mut replay = Replay::of(&live);
        let half = replay.duration() / 2.0;
        assert!(replay.seek(half, usize::MAX));
        let at_half = replay.sculpt.mesh.positions.clone();
        assert_ne!(at_half, live.mesh.positions);
        // Forward to the end ten entries at a time, then back to the middle.
        while !replay.seek(f64::INFINITY, 10) {}
        assert_eq!(replay.sculpt.mesh.positions, live.mesh.positions);
        assert!(replay.seek(half, usize::MAX));
        assert_eq!(replay.sculpt.mesh.positions, at_half);
    }

    #[test]
    fn long_pauses_are_shortened() {
        let mut live = Sculpt::new(Mesh::icosphere(2));
        session(&mut live);
        let recorded = live.log().last().unwrap().time - live.log()[0].time;
        let replay = Replay::of(&live);
        assert!(recorded > 60.0);
        assert!(replay.duration() < recorded - 55.0, "{}", replay.duration());
    }

    /// Sculpt, remesh, sculpt, undo the remesh, redo it, sculpt again.
    fn remeshed_session() -> Sculpt {
        let mut live = Sculpt::new(Mesh::icosphere(4));
        let clock = session_at(&mut live, 0.0, 0.0);
        live.set_clock(clock + 1.0);
        live.replace_mesh(voxel::remesh(&live.mesh, 48).expect("remesh"));
        let clock = session_at(&mut live, clock + 2.0, 0.1);
        // Undo goes back through the strokes to the mesh before the remesh.
        while live.can_undo() {
            live.undo();
        }
        assert_eq!(
            live.mesh.positions.len(),
            live.checkpoints()[0].positions.len()
        );
        live.redo();
        let clock = session_at(&mut live, clock + 1.0, -0.1);
        // Into a fresh stroke: the open stroke is closed first.
        live.begin_stroke();
        live.grab(Vec3::Z, 0.3, false);
        live.drag(Vec3::Z * 0.1);
        live.set_clock(clock + 1.0);
        live.replace_mesh(voxel::remesh(&live.mesh, 40).expect("remesh"));
        live
    }

    #[test]
    fn replay_through_remeshes_equals_the_live_sculpt_bit_for_bit() {
        let live = remeshed_session();
        assert!(live.checkpoints().len() >= 4);
        let mut replay = Replay::of(&live);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert!(replay.finished());
        assert_same(&replay, &live);

        // Seeking back starts from the nearest checkpoint, and forward again
        // in small steps lands on the same clay.
        let mut at = Vec::new();
        let steps = 7;
        for k in 0..=steps {
            let time = replay.duration() * k as f64 / steps as f64;
            let mut fresh = Replay::of(&live);
            assert!(fresh.seek(time, usize::MAX));
            at.push((time, fresh.sculpt.mesh.positions.clone()));
        }
        for (time, positions) in at.iter().rev() {
            assert!(replay.seek(*time, usize::MAX));
            assert_eq!(&replay.sculpt.mesh.positions, positions, "at {time}");
        }
        while !replay.seek(f64::INFINITY, 25) {}
        assert_same(&replay, &live);
    }

    #[test]
    fn periodic_checkpoints_keep_undo_and_replay_exactly() {
        let mut live = Sculpt::new(Mesh::icosphere(3));
        let clock = session_at(&mut live, 0.0, 0.0);
        let mut clock = session_at(&mut live, clock + super::super::CHECKPOINT_EVERY, 0.1);
        let periodic = live
            .checkpoints()
            .iter()
            .filter(|c| c.triangles.is_none())
            .count();
        assert!(periodic >= 1);
        assert!(live.can_undo(), "a checkpoint of the positions keeps undo");
        clock += 1.0;
        live.set_clock(clock);
        live.undo();
        let mut replay = Replay::of(&live);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_same(&replay, &live);
    }

    #[test]
    fn a_clay_dab_keeps_its_fill_and_older_ones_read_without() {
        let dab = Dab {
            brush: Brush::Clay,
            centre: Vec3::Z,
            radius: 0.2,
            strength: 0.5,
            invert: false,
            symmetry: false,
            direction: Vec3::X,
            buildup: Buildup::Layer,
            fill: true,
            wipes: false,
        };
        let mut w = Writer::new();
        Entry {
            time: 1.0,
            event: Event::Dab(dab),
        }
        .write(&mut w);
        let mut bytes = w.finish();
        let read = |bytes: &[u8]| Entry::read(&mut Reader::new(bytes)).unwrap().event;
        assert_eq!(read(&bytes), Event::Dab(dab));
        // Time, tag, brush, centre, radius and strength, then the flags.
        let flags = 8 + 1 + 1 + 12 + 4 + 4;
        assert_eq!(bytes[flags], 4 | 32);
        bytes[flags] = 4;
        assert_eq!(read(&bytes), Event::Dab(Dab { fill: false, ..dab }));
    }

    #[test]
    fn a_dab_from_before_layers_reads_as_accumulating() {
        let dab = Dab {
            brush: Brush::Clay,
            centre: Vec3::Z,
            radius: 0.2,
            strength: 0.5,
            invert: true,
            symmetry: true,
            direction: Vec3::X,
            buildup: Buildup::Layer,
            fill: false,
            wipes: false,
        };
        let bytes = |dab: Dab| {
            let mut w = Writer::new();
            Entry {
                time: 1.0,
                event: Event::Dab(dab),
            }
            .write(&mut w);
            w.finish()
        };
        let read = |bytes: &[u8]| Entry::read(&mut Reader::new(bytes)).unwrap().event;
        assert_eq!(read(&bytes(dab)), Event::Dab(dab));
        // Older logs wrote only the invert and symmetry bits: time, tag,
        // brush, centre, radius and strength come before the flags.
        let mut old = bytes(dab);
        let flags = 8 + 1 + 1 + 12 + 4 + 4;
        assert_eq!(old[flags], 15);
        old[flags] &= 3;
        // Their symmetric dabs added up where both sides overlap.
        assert_eq!(
            read(&old),
            Event::SummedDab(Dab {
                buildup: Buildup::Accumulate,
                fill: false,
                ..dab
            })
        );
        // Those from before the larger weight, with the build-up bit.
        old[flags] = 7;
        assert_eq!(read(&old), Event::SummedDab(dab));
        let summed = Entry {
            time: 1.0,
            event: Event::SummedDab(dab),
        };
        let mut w = Writer::new();
        summed.write(&mut w);
        assert_eq!(w.finish(), old, "saved again as they were");
        // Without symmetry the bit makes no difference.
        let single = Dab {
            symmetry: false,
            ..dab
        };
        assert_eq!(bytes(single)[flags], 5);
        assert_eq!(read(&bytes(single)), Event::Dab(single));
    }

    /// A log of `events`, as bytes.
    fn log_bytes(events: &[Event]) -> Vec<u8> {
        let entries: Vec<Entry> = events
            .iter()
            .map(|&event| Entry { time: 1.0, event })
            .collect();
        let mut w = Writer::new();
        write_log(&mut w, &entries);
        w.finish()
    }

    fn read_events(bytes: &[u8]) -> Result<Vec<Event>, ReadError> {
        let log = read_log(&mut Reader::new(bytes))?;
        Ok(log.into_iter().map(|e| e.event).collect())
    }

    #[test]
    fn every_falloff_keeps_its_byte_in_the_log() {
        // Fixed for good: changing one replays old timelapses with
        // another falloff.
        let pinned = [
            (Falloff::Smooth, 0),
            (Falloff::Linear, 1),
            (Falloff::Peak, 2),
            (Falloff::Flat, 3),
            (Falloff::Sharp, 4),
        ];
        for (falloff, byte) in pinned {
            let bytes = log_bytes(&[Event::Falloff(falloff)]);
            // The count, the time and the tag come before it.
            assert_eq!(bytes[bytes.len() - 2], tag::FALLOFF);
            assert_eq!(bytes.last(), Some(&byte), "{falloff:?}");
            assert_eq!(read_events(&bytes).unwrap(), [Event::Falloff(falloff)]);
        }
        // Every falloff the panel offers has a byte of its own.
        for falloff in Falloff::ALL {
            assert!(pinned.iter().any(|&(f, _)| f == falloff), "{falloff:?}");
        }
        // The slider's falloffs: one byte each, from 16 up.
        for h in 0..=MAX_HARDNESS {
            let falloff = Falloff::Hard(h);
            let bytes = log_bytes(&[Event::Falloff(falloff)]);
            assert_eq!(bytes.last(), Some(&(16 + h)), "{falloff:?}");
            assert_eq!(read_events(&bytes).unwrap(), [Event::Falloff(falloff)]);
        }
        for byte in [5, 15, 16 + MAX_HARDNESS + 1] {
            let mut unknown = log_bytes(&[Event::Falloff(Falloff::Smooth)]);
            *unknown.last_mut().unwrap() = byte;
            assert!(read_events(&unknown).is_err(), "{byte}");
        }
    }

    #[test]
    fn an_old_linear_falloff_replays_as_linear() {
        // Logs from before the flat falloff: byte 1, then dabs without
        // the build-up bit.
        let dab = Dab {
            brush: Brush::Smooth,
            centre: Vec3::Z,
            radius: 0.2,
            strength: 0.5,
            invert: false,
            symmetry: false,
            direction: Vec3::ZERO,
            buildup: Buildup::Accumulate,
            fill: false,
            wipes: false,
        };
        let old = [Event::Falloff(Falloff::Linear), Event::Dab(dab)];
        assert_eq!(read_events(&log_bytes(&old)).unwrap(), old);
        assert_eq!(Falloff::Linear.weight(0.5, 1.0), 0.5);
        assert_eq!(Falloff::Linear.weight(0.25, 2.0), 0.875);
        assert_eq!(Falloff::Linear.weight(1.5, 1.0), 0.0);

        // Logs from the builds that wrote flat as 1 set the build-up bit
        // on their dabs: there, 1 was flat.
        let layered = Dab {
            buildup: Buildup::Layer,
            fill: false,
            ..dab
        };
        let flat = |events: &[Event]| {
            read_events(&log_bytes(events))
                .unwrap()
                .into_iter()
                .filter(|e| matches!(e, Event::Falloff(_)))
                .collect::<Vec<_>>()
        };
        let linear = Event::Falloff(Falloff::Linear);
        assert_eq!(
            flat(&[linear, Event::Dab(layered)]),
            [Event::Falloff(Falloff::Flat)]
        );
        // An old session reopened and carried on in those builds: its
        // own linear stays linear, the later 1 was flat.
        let grab = Event::Grab {
            centre: Vec3::Z,
            radius: 0.2,
            symmetry: false,
        };
        assert_eq!(
            flat(&[
                linear,
                Event::Dab(dab),
                Event::Falloff(Falloff::Smooth),
                Event::Dab(layered),
                linear,
                grab,
            ]),
            [
                Event::Falloff(Falloff::Linear),
                Event::Falloff(Falloff::Smooth),
                Event::Falloff(Falloff::Flat),
            ]
        );
    }

    /// A few clay strokes with `falloff`, logged.
    fn strokes_with(falloff: Falloff) -> Sculpt {
        let mut s = Sculpt::new(Mesh::icosphere(4));
        s.set_clock(1.0);
        s.set_falloff(falloff);
        s.begin_stroke();
        for i in 0..6 {
            s.set_clock(1.0 + i as f64 * 0.02);
            s.dab(Dab {
                brush: Brush::Clay,
                centre: Vec3::new(i as f32 * 0.05, 0.1, 1.0).normalize(),
                radius: 0.35,
                strength: 0.8,
                invert: false,
                symmetry: false,
                direction: if i == 0 { Vec3::ZERO } else { Vec3::X },
                buildup: Buildup::Accumulate,
                fill: false,
                wipes: false,
            });
            s.refresh();
        }
        s.end_stroke();
        s
    }

    #[test]
    fn an_old_sharp_falloff_replays_as_the_narrow_peak() {
        // Timelapses from before sharp became a plateau store their sharp
        // as 2: it reads as the old curve, (1 - t)³, bit for bit.
        let old = log_bytes(&[Event::Falloff(Falloff::Peak)]);
        assert_eq!(old.last(), Some(&2));
        assert_eq!(read_events(&old).unwrap(), [Event::Falloff(Falloff::Peak)]);
        assert_eq!(Falloff::Peak.weight(0.5, 1.0), 0.125);
        assert_eq!(Falloff::Peak.weight(0.25, 2.0), 0.875 * 0.875 * 0.875);
        assert_eq!(Falloff::Peak.weight(1.5, 1.0), 0.0);

        // A session with it saved and reopened replays to the same clay,
        // which the new sharp would not have made.
        let live = strokes_with(Falloff::Peak);
        let mut w = Writer::new();
        write_log(&mut w, live.log());
        write_checkpoints(&mut w, live.checkpoints());
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        let (log, checkpoints) = (read_log(&mut r).unwrap(), read_checkpoints(&mut r).unwrap());
        assert!(log.iter().any(|e| e.event == Event::Falloff(Falloff::Peak)));
        let reopened = Sculpt::with_history(saved(&live), None, log, checkpoints);
        let mut replay = Replay::of(&reopened);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_same(&replay, &live);
        assert_ne!(
            strokes_with(Falloff::Sharp).mesh.positions,
            live.mesh.positions
        );
    }

    #[test]
    fn the_new_sharp_falloff_round_trips_with_its_own_byte() {
        let live = strokes_with(Falloff::Sharp);
        let mut w = Writer::new();
        write_log(&mut w, live.log());
        write_checkpoints(&mut w, live.checkpoints());
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        let (log, checkpoints) = (read_log(&mut r).unwrap(), read_checkpoints(&mut r).unwrap());
        assert_eq!(log, live.log());
        assert!(
            log.iter()
                .any(|e| e.event == Event::Falloff(Falloff::Sharp))
        );
        assert!(!log.iter().any(|e| e.event == Event::Falloff(Falloff::Peak)));
        let reopened = Sculpt::with_history(saved(&live), None, log, checkpoints);
        let mut replay = Replay::of(&reopened);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_same(&replay, &live);
    }

    #[test]
    fn the_log_and_checkpoints_round_trip_and_replay_the_same() {
        let live = remeshed_session();
        let mut w = Writer::new();
        write_log(&mut w, live.log());
        write_checkpoints(&mut w, live.checkpoints());
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        let log = read_log(&mut r).unwrap();
        let mut checkpoints = read_checkpoints(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(log, live.log());
        // The mask and chalk are saved apart (see `scene`).
        assert!(live.checkpoints().iter().any(|c| c.layers.is_some()));
        for (read, kept) in checkpoints.iter_mut().zip(live.checkpoints()) {
            read.layers.clone_from(&kept.layers);
        }
        assert_eq!(checkpoints, live.checkpoints());

        // A scene reopened: the current mesh in its order, and its history.
        let current = saved(&live);
        let reopened = Sculpt::with_history(current, None, log, checkpoints);
        let mut replay = Replay::of(&reopened);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_same(&replay, &live);
    }

    #[test]
    fn ending_without_a_stroke_logs_nothing_even_after_reopening() {
        let mut live = Sculpt::new(Mesh::icosphere(3));
        session_at(&mut live, 0.0, 0.0);
        let (log, checkpoints) = (live.log().to_vec(), live.checkpoints().to_vec());
        // A camera drag ending: no stroke is open.
        live.end_stroke();
        assert_eq!(live.log(), &log[..]);
        let current = saved(&live);
        let mut reopened = Sculpt::with_history(current, None, log.clone(), checkpoints.clone());
        reopened.end_stroke();
        assert_eq!(reopened.log(), &log[..], "no stray End");
        assert_eq!(
            reopened.checkpoints(),
            &checkpoints[..],
            "no resume checkpoint"
        );
    }

    #[test]
    fn a_reopened_scene_sculpted_on_still_replays_bit_for_bit() {
        let mut live = Sculpt::new(Mesh::icosphere(4));
        let clock = session_at(&mut live, 0.0, 0.0);
        // Saved with a falloff other than the default in force.
        live.set_falloff(Falloff::Sharp);
        let mut w = Writer::new();
        write_log(&mut w, live.log());
        write_checkpoints(&mut w, live.checkpoints());
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        let (log, checkpoints) = (read_log(&mut r).unwrap(), read_checkpoints(&mut r).unwrap());
        let current = saved(&live);
        let mut reopened = Sculpt::with_history(current, None, log, checkpoints);
        // Untouched, it saves what it read.
        assert_eq!(reopened.log(), live.log());
        assert_eq!(reopened.checkpoints(), live.checkpoints());

        reopened.set_falloff(Falloff::Sharp);
        let clock = session_at(&mut reopened, clock + 5.0, 0.05);
        // Twice: a reopened scene reopened again.
        let current = saved(&reopened);
        let mut again = Sculpt::with_history(
            current,
            None,
            reopened.log().to_vec(),
            reopened.checkpoints().to_vec(),
        );
        session_at(&mut again, clock + 5.0, -0.05);
        // Each reopening logged a rebuild, not another whole mesh.
        assert_eq!(again.checkpoints().len(), live.checkpoints().len());
        let rebuilds = again.log().iter().filter(|e| e.event == Event::Rebuild);
        assert_eq!(rebuilds.count(), 2);
        let mut replay = Replay::of(&again);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_same(&replay, &again);

        // Scrubbing back across the rebuilds and forward again in small
        // steps lands on the same clay as a fresh replay.
        let steps = 9;
        for k in (0..=steps).rev() {
            let time = replay.duration() * k as f64 / steps as f64;
            let mut fresh = Replay::of(&again);
            assert!(fresh.seek(time, usize::MAX));
            assert!(replay.seek(time, usize::MAX));
            assert_same(&replay, &fresh.sculpt);
        }
        while !replay.seek(f64::INFINITY, 40) {}
        assert_same(&replay, &again);

        // And the log, rebuilds and all, saves and reads back.
        let mut w = Writer::new();
        write_log(&mut w, again.log());
        assert_eq!(
            read_log(&mut Reader::new(&w.finish())).unwrap(),
            again.log()
        );
    }

    #[test]
    fn a_history_that_leads_elsewhere_resumes_from_a_whole_checkpoint() {
        let mut live = Sculpt::new(Mesh::icosphere(3));
        let clock = session_at(&mut live, 0.0, 0.0);
        // A clay the log doesn't lead to: other triangles.
        let other = Mesh::icosphere(2);
        let mut reopened = Sculpt::with_history(
            other.clone(),
            None,
            live.log().to_vec(),
            live.checkpoints().to_vec(),
        );
        session_at(&mut reopened, clock + 5.0, 0.05);
        assert_eq!(reopened.checkpoints().len(), live.checkpoints().len() + 1);
        assert_eq!(
            reopened.checkpoints().last().unwrap().triangles.as_deref(),
            Some(other.triangles.as_slice())
        );
        assert!(!reopened.log().iter().any(|e| e.event == Event::Rebuild));
        let mut replay = Replay::of(&reopened);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_same(&replay, &reopened);
    }

    #[test]
    fn a_first_checkpoint_without_triangles_is_no_start() {
        let sphere = Mesh::icosphere(1);
        let mut start = Checkpoint::start(&sphere);
        assert!(starts_with_mesh(std::slice::from_ref(&start)));
        start.triangles = Some(Vec::new().into());
        assert!(!starts_with_mesh(std::slice::from_ref(&start)));
        start.triangles = None;
        assert!(!starts_with_mesh(std::slice::from_ref(&start)));
        assert!(!starts_with_mesh(&[]));
    }

    #[test]
    fn a_checkpoint_with_a_triangle_corner_twice_is_refused() {
        let mut bad = Checkpoint::start(&Mesh::icosphere(1));
        let mut triangles = bad.triangles.as_deref().unwrap().to_vec();
        triangles[5][2] = triangles[5][0];
        bad.triangles = Some(triangles.into());
        let mut w = Writer::new();
        bad.write(&mut w);
        assert!(Checkpoint::read(&mut Reader::new(&w.finish())).is_err());
    }

    #[test]
    fn damaged_history_is_an_error_or_replays_without_panicking() {
        let mut live = Sculpt::new(Mesh::icosphere(1));
        session(&mut live);
        live.replace_mesh(Mesh::icosphere(2));
        let mut w = Writer::new();
        write_log(&mut w, live.log());
        write_checkpoints(&mut w, live.checkpoints());
        let bytes = w.finish();
        for end in (0..bytes.len()).step_by(7) {
            let mut r = Reader::new(&bytes[..end]);
            assert!(
                read_log(&mut r)
                    .and_then(|_| read_checkpoints(&mut r))
                    .is_err(),
                "cut at {end}"
            );
        }
        // Bytes changed here and there: an error, or a replay that runs.
        for k in 1..64 {
            let mut bad = bytes.clone();
            let at = bad.len() * k / 64;
            bad[at] ^= 0x5A;
            let mut r = Reader::new(&bad);
            if let (Ok(log), Ok(checkpoints)) = (read_log(&mut r), read_checkpoints(&mut r)) {
                Replay::new(checkpoints, log).seek(f64::INFINITY, usize::MAX);
            }
        }
        // A position that isn't a number, or more vertices than the clay
        // may have: errors.
        let mut w = Writer::new();
        let mut bad = Checkpoint::start(&Mesh::icosphere(1));
        let mut positions = bad.positions.to_vec();
        positions[3].y = f32::NAN;
        bad.positions = positions.into();
        bad.write(&mut w);
        assert!(Checkpoint::read(&mut Reader::new(&w.finish())).is_err());
        let mut w = Writer::new();
        w.u64(0).f64(0.0).len(MAX_VERTICES + 1);
        let mut bytes = w.finish();
        bytes.resize(bytes.len() + (MAX_VERTICES + 1) * 12 + 1, 0);
        assert!(Checkpoint::read(&mut Reader::new(&bytes)).is_err());
        // No checkpoints at all: the default sphere.
        let replay = Replay::new(Vec::new(), Vec::new());
        assert_eq!(replay.sculpt.mesh.positions.len(), 163_842);
    }

    /// What dragging the timelapse's playhead backwards costs per frame, on
    /// the default sphere: each seek goes back to the start checkpoint.
    /// `cargo test --release -p splatstudy-core scrub_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn scrub_timing() {
        let mut live = Sculpt::new(Mesh::icosphere(OLD_START_LEVEL));
        let mut clock = 0.0;
        for k in 0..3 {
            clock = session_at(&mut live, clock + 1.0, k as f32 * 0.05);
        }
        let mut replay = Replay::of(&live);
        let tries = 10;
        let (mut total, mut rebuilt) = (0.0, 0);
        for k in 0..tries {
            assert!(replay.seek(f64::INFINITY, usize::MAX));
            replay.sculpt.take_topology_changed();
            replay.sculpt.take_dirty();
            // Back behind the clay: the restart alone, no entries played.
            let time = replay.duration() * k as f64 / tries as f64;
            let start = std::time::Instant::now();
            replay.seek(time, 0);
            if replay.sculpt.take_topology_changed() {
                rebuilt += 1;
            }
            replay.sculpt.take_dirty();
            total += start.elapsed().as_secs_f64() * 1e3;
        }
        println!(
            "seeking back: restart {:.1} ms; {rebuilt} of {tries} rebuilt the GPU mesh",
            total / tries as f64
        );
    }
}
