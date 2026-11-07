// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Sculpting: the clay mesh, the brushes that shape it, and undo.
//!
//! The engine knows nothing about screens or pointers: the app turns pointer
//! samples into world-space dabs (`dab`) or grabs (`grab`, `drag`).

pub mod bvh;
pub mod layers;
pub mod levels;
pub mod mesh;
pub mod record;
pub mod sphere;
pub mod subdivide;

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use glam::Vec3;

pub use bvh::{Bvh, Hit};
pub use layers::{Layer, LayerOp, Layers};
pub use levels::Levels;
pub use mesh::{Mesh, Stamp, Vertex};
pub use record::{Checkpoint, Entry, Event, Replay};
pub use sphere::{START_LEVEL, cube_sphere};

/// Subdivision level of the icosphere the first versions started from
/// (163,842 vertices). A replay of a log without a start of its own still
/// starts there, so old logs play as they were recorded; new scenes start
/// from [`cube_sphere`].
pub const OLD_START_LEVEL: u32 = 7;

/// The most vertices and triangles the clay may have, from any source
/// (import, remesh, subdivision, a saved scene): its GPU buffers (32 and
/// 12 bytes each) stay well under wgpu's 256 MiB default buffer limit.
pub const MAX_VERTICES: usize = 4_000_000;
pub const MAX_TRIANGLES: usize = 8_000_000;

/// Vertices per GPU upload page.
pub const PAGE: usize = 4096;

/// About the most memory the undo stack keeps, in bytes: past it, the
/// oldest steps are forgotten. One stroke with a large brush on the finest
/// clay holds over ten megabytes, and in the browser all of it lives in
/// the 4 GB the page may use.
pub const UNDO_BUDGET: usize = 256 << 20;

/// Sculpting time between checkpoints of the positions, in seconds: a
/// timelapse snaps back to them, so a replay that drifts (another machine
/// rounding differently) recovers.
pub const CHECKPOINT_EVERY: f64 = 600.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brush {
    /// Grabs the surface under the brush and drags it.
    Move,
    /// Builds up towards a plane, in flat layers, like pressing on clay.
    Clay,
    /// Cuts the area down to its plane, like a spatula.
    Flatten,
    /// Relaxes the surface towards the average of its neighbours.
    Smooth,
    /// Swells the surface outwards, each point along its own normal.
    Inflate,
    /// Cuts a narrow groove along the stroke, like a wire loop tool.
    Crease,
    /// Draws the surface together towards the brush centre, sharpening edges.
    Pinch,
    /// Paints the mask, which holds the clay still under every other
    /// brush; the opposite action takes it off.
    Mask,
}

impl Brush {
    /// Every brush, in the order the tool panel lists them.
    pub const ALL: [Brush; 8] = [
        Brush::Move,
        Brush::Clay,
        Brush::Flatten,
        Brush::Smooth,
        Brush::Inflate,
        Brush::Crease,
        Brush::Pinch,
        Brush::Mask,
    ];
}

/// How a brush's effect fades from its centre to its rim.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Falloff {
    /// A soft dome: full at the centre, flat at both ends.
    #[default]
    Smooth,
    /// A plateau, like a clay tube: full over the inner half, then a soft
    /// shoulder down to the rim.
    Flat,
    /// A narrow peak: most of the effect near the centre.
    Sharp,
    /// A cone, the panel's second choice before flat took its place. Only
    /// timelapses recorded with it replay it; the panel no longer lists it.
    Linear,
}

impl Falloff {
    /// Every falloff, in the order the panel lists them. A log stores
    /// each one by its own byte, not by this order (see `record`).
    pub const ALL: [Falloff; 3] = [Falloff::Smooth, Falloff::Flat, Falloff::Sharp];

    /// The weight at `distance` from the centre of a brush of `radius`:
    /// 1 at the centre, 0 at the rim and beyond.
    pub fn weight(self, distance: f32, radius: f32) -> f32 {
        let t = (distance / radius).min(1.0);
        match self {
            Falloff::Smooth => {
                let s = 1.0 - t * t;
                s * s
            }
            Falloff::Flat => {
                let s = ((1.0 - t) * 2.0).min(1.0);
                s * s * (3.0 - 2.0 * s)
            }
            Falloff::Sharp => {
                let s = 1.0 - t;
                s * s * s
            }
            Falloff::Linear => 1.0 - t,
        }
    }
}

/// How clay builds where one stroke passes again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Buildup {
    /// Every pass adds more, as in most sculpting apps.
    #[default]
    Accumulate,
    /// A stroke lays one layer, [`LAYER`] times its height, and going
    /// over it again in the same stroke adds nothing; the next stroke lays
    /// the next layer.
    Layer,
}

impl Buildup {
    /// Every way, in the order the panel lists them.
    pub const ALL: [Buildup; 2] = [Buildup::Layer, Buildup::Accumulate];
}

/// How thick one stroke's layer of clay is, as a share of the brush
/// radius at full strength (see [`Buildup::Layer`]).
pub const LAYER: f32 = 0.5;

/// One touch of the brush on the surface, in world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dab {
    pub brush: Brush,
    pub centre: Vec3,
    pub radius: f32,
    /// 0..1, already including pressure.
    pub strength: f32,
    /// The opposite action: clay digs, flatten fills, inflate deflates,
    /// crease raises a ridge, pinch spreads, mask unmasks.
    pub invert: bool,
    /// Also apply mirrored across the X = 0 plane.
    pub symmetry: bool,
    /// Which way the stroke is heading, unit length; zero at its first dab.
    pub direction: Vec3,
    /// Clay: whether a stroke builds more where it passes again.
    pub buildup: Buildup,
}

/// One undoable change: the vertices a stroke touched, before and after.
struct Edit {
    indices: Vec<u32>,
    /// Their positions; empty if the stroke only painted the mask.
    before: Vec<Vec3>,
    after: Vec<Vec3>,
    /// Their mask and chalk; empty if neither changed.
    layers_before: Vec<[f32; 2]>,
    layers_after: Vec<[f32; 2]>,
}

impl Step {
    /// Roughly the memory it holds: what [`UNDO_BUDGET`] counts.
    fn bytes(&self) -> usize {
        match self {
            Step::Move(edit) => {
                edit.indices.len() * 4
                    + (edit.before.len() + edit.after.len()) * 12
                    + (edit.layers_before.len() + edit.layers_after.len()) * 8
            }
            Step::Layer(_, layer) => layer.bytes(),
            Step::Topology(old) => {
                old.0.positions.len() * 12 + old.0.triangles.len() * 12 + old.0.layers.bytes()
            }
            Step::Levels(Some(levels)) => levels.memory(),
            _ => 0,
        }
    }
}

/// One step on the undo or redo stack.
enum Step {
    Move(Edit),
    /// The mesh was replaced (remesh, import): the other mesh and its
    /// levels, to swap back. Strokes before a topology change can't be
    /// undone, so this is always alone on its stack.
    Topology(Box<(Mesh, Option<Levels>)>),
    /// The clay went one level up or down (see `levels.rs`); also alone.
    Level {
        up: bool,
    },
    /// A level was added at the top; also alone.
    Subdivide,
    /// Levels were deleted: the levels before, to swap back.
    Levels(Option<Box<Levels>>),
    /// A whole layer was replaced (see `Sculpt::edit_layer`): the layer
    /// before, to swap back.
    Layer(layers::Kind, Layer),
}

pub struct Sculpt {
    pub mesh: Mesh,
    bvh: Bvh,
    bvh_stale: bool,
    stamp: Stamp,
    /// Keeps a vertex from being grabbed twice when both sides overlap.
    unique: Stamp,
    found: Vec<u32>,
    moved: Vec<u32>,
    affected: Vec<u32>,
    smoothed: Vec<Vec3>,
    dirty_pages: Vec<bool>,
    // The open stroke: first-touch positions of every vertex it moved.
    stroke_open: bool,
    touched: Stamp,
    stroke_indices: Vec<u32>,
    stroke_before: Vec<Vec3>,
    /// For each vertex the stroke touched, its place in `stroke_indices`.
    stroke_slot: Vec<u32>,
    /// The mask and chalk of each vertex in `stroke_indices`, when the
    /// stroke first touched it.
    stroke_layers_before: Vec<[f32; 2]>,
    /// Move brush: grabbed vertices, their start positions, weights, and
    /// whether they belong to the mirrored side.
    grabbed: Vec<(u32, Vec3, f32, bool)>,
    /// Move brush: the grabbed surface has moved, and lost its chalk.
    grab_wiped: bool,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Every call that changed the clay, for replay.
    log: Vec<Entry>,
    /// The meshes a replay starts from or snaps to; the first is the start.
    checkpoints: Vec<Checkpoint>,
    recording: bool,
    /// Time stamped on new log entries, in seconds.
    clock: f64,
    /// The clock at the last checkpoint.
    last_checkpoint: f64,
    /// The mesh was replaced since the GPU buffers were last made for it.
    topology_changed: bool,
    /// How the brushes fade towards their rim.
    falloff: Falloff,
    /// The falloff a replay of the log would have reached: a change is
    /// logged before the next dab or grab that differs from it.
    logged_falloff: Falloff,
    /// Reopened with a log, and not changed since: what the first change
    /// logs before itself (see `with_history`).
    resume: Resume,
    /// Tells this sculpt from every other made in this run.
    id: u64,
    /// The clay's other subdivision levels, once it has some.
    levels: Option<Levels>,
    /// The levels changed since this sculpt was built from a checkpoint:
    /// a replay can't just rewind its positions (see `Replay::restart`).
    releveled: bool,
}

/// What a reopened sculpt logs before its first change.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Resume {
    /// Nothing: a new sculpt, or the first change was logged.
    Settled,
    /// An [`Event::Rebuild`]: a replay rebuilds from its own clay, which
    /// is this one.
    Rebuild,
    /// A whole checkpoint: the log doesn't lead to this clay's triangles.
    Checkpoint,
}

fn mirror(v: Vec3) -> Vec3 {
    Vec3::new(-v.x, v.y, v.z)
}

/// The part of `v` that lies along a surface with normal `n`.
fn tangent(v: Vec3, n: Vec3) -> Vec3 {
    v - n * v.dot(n)
}

impl Sculpt {
    /// A sculpt starting from `mesh`, which becomes the first checkpoint.
    pub fn new(mesh: Mesh) -> Sculpt {
        Sculpt::with_levels(mesh, None)
    }

    /// A sculpt starting from `mesh` at one of its `levels` (read with
    /// [`Levels::read`] for that mesh); both become the first checkpoint.
    pub fn with_levels(mesh: Mesh, levels: Option<Levels>) -> Sculpt {
        let mut sculpt = Sculpt {
            levels,
            ..Sculpt::bare(mesh)
        };
        let start = Checkpoint::of(&sculpt.mesh, sculpt.levels_for_checkpoint(), 0, 0.0, true);
        sculpt.checkpoints.push(start);
        sculpt
    }

    /// A sculpt with no checkpoints: for replay, or to restore a saved one.
    fn bare(mesh: Mesh) -> Sculpt {
        static MADE: AtomicU64 = AtomicU64::new(0);
        let n = mesh.positions.len();
        Sculpt {
            id: MADE.fetch_add(1, Ordering::Relaxed),
            bvh: Bvh::build(&mesh),
            bvh_stale: false,
            stamp: Stamp::new(n),
            unique: Stamp::new(n),
            found: Vec::new(),
            moved: Vec::new(),
            affected: Vec::new(),
            smoothed: Vec::new(),
            dirty_pages: vec![true; n.div_ceil(PAGE)],
            stroke_open: false,
            touched: Stamp::new(n),
            stroke_indices: Vec::new(),
            stroke_before: Vec::new(),
            stroke_slot: vec![0; n],
            stroke_layers_before: Vec::new(),
            grabbed: Vec::new(),
            grab_wiped: false,
            undo: Vec::new(),
            redo: Vec::new(),
            log: Vec::new(),
            checkpoints: Vec::new(),
            recording: true,
            clock: 0.0,
            last_checkpoint: 0.0,
            topology_changed: false,
            falloff: Falloff::Smooth,
            logged_falloff: Falloff::Smooth,
            resume: Resume::Settled,
            mesh,
            levels: None,
            releveled: false,
        }
    }

    /// A saved sculpt: its current mesh (in the saved vertex order, see
    /// [`Mesh::from_ordered`]), its log and checkpoints. Undo starts empty.
    ///
    /// The reopened sculpt builds its hierarchy afresh from `mesh`, while a
    /// replay of the log would still have the one built at the last
    /// topology change, which finds vertices in another order. So the
    /// first change after reopening logs an [`Event::Rebuild`] first, and a
    /// replay rebuilds from its own clay, which is `mesh`. (Not before: a
    /// scene reopened and saved untouched saves the same bytes.)
    ///
    /// When the log doesn't lead to `mesh`'s triangles (a history from a
    /// damaged or edited file), a replay can't reach `mesh` by itself: a
    /// whole checkpoint of it is logged instead.
    ///
    /// `levels` are the clay's subdivision levels, read for `mesh`.
    pub fn with_history(
        mesh: Mesh,
        levels: Option<Levels>,
        log: Vec<Entry>,
        checkpoints: Vec<Checkpoint>,
    ) -> Sculpt {
        let resume = if log.is_empty() {
            Resume::Settled
        } else if record::ends_on_triangles_of(&mesh, levels.as_ref(), &log, &checkpoints) {
            Resume::Rebuild
        } else {
            Resume::Checkpoint
        };
        Sculpt {
            resume,
            log,
            checkpoints,
            levels,
            ..Sculpt::bare(mesh)
        }
    }

    /// Logs what a reopened sculpt owes, before its first change.
    fn settle_resume(&mut self) {
        match std::mem::replace(&mut self.resume, Resume::Settled) {
            Resume::Settled => {}
            Resume::Rebuild => {
                self.record(Event::Rebuild);
                // A replay goes back to the default there, as at a checkpoint.
                self.logged_falloff = Falloff::Smooth;
                self.last_checkpoint = self.clock;
            }
            Resume::Checkpoint => self.checkpoint(true),
        }
    }

    /// Tells this sculpt from every other made since the app started.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// How the brushes fade from now on, for dabs and grabs.
    pub fn set_falloff(&mut self, falloff: Falloff) {
        self.falloff = falloff;
    }

    pub fn falloff(&self) -> Falloff {
        self.falloff
    }

    /// Logs the falloff if a replay would not have it yet.
    fn log_falloff(&mut self) {
        self.settle_resume();
        if self.recording && self.falloff != self.logged_falloff {
            self.record(Event::Falloff(self.falloff));
            self.logged_falloff = self.falloff;
        }
    }

    /// The weight of a brush's effect at `distance` from its centre.
    fn weight(&self, distance: f32, radius: f32) -> f32 {
        self.falloff.weight(distance, radius)
    }

    /// The meshes a replay of [`Sculpt::log`] starts from and snaps to.
    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    /// True once after the mesh was replaced (remesh, import, undoing
    /// either, or a replay jumping to a checkpoint): its GPU buffers have
    /// to be made again rather than updated.
    pub fn take_topology_changed(&mut self) -> bool {
        std::mem::take(&mut self.topology_changed)
    }

    /// Replaces the whole mesh (a remesh or an import). The log and the
    /// clock go on; the new mesh is saved as a checkpoint for replay.
    /// Its subdivision levels go: they belong to the old mesh. Strokes
    /// made before can no longer be undone, but this can: undo brings the
    /// previous mesh back, with its levels.
    pub fn replace_mesh(&mut self, mesh: Mesh) {
        self.replace_mesh_with_levels(mesh, None);
    }

    /// [`Sculpt::replace_mesh`], with subdivision levels of its own (read
    /// or built for `mesh`), saved in its checkpoint too.
    pub fn replace_mesh_with_levels(&mut self, mesh: Mesh, levels: Option<Levels>) {
        // Its own checkpoint follows.
        self.resume = Resume::Settled;
        let old = self.swap_mesh(mesh);
        let levels = std::mem::replace(&mut self.levels, levels);
        self.undo.clear();
        self.redo.clear();
        self.undo.push(Step::Topology(Box::new((old, levels))));
        self.checkpoint(true);
    }

    /// Puts `mesh` in place and rebuilds everything sized by it; returns
    /// the mesh it replaced.
    fn swap_mesh(&mut self, mut mesh: Mesh) -> Mesh {
        if self.stroke_open {
            self.end_stroke();
        }
        // Normals as a replay rebuilds them from a checkpoint.
        mesh.recompute_normals();
        let n = mesh.positions.len();
        self.bvh = Bvh::build(&mesh);
        self.bvh_stale = false;
        self.stamp = Stamp::new(n);
        self.unique = Stamp::new(n);
        self.touched = Stamp::new(n);
        self.stroke_slot = vec![0; n];
        self.dirty_pages = vec![true; n.div_ceil(PAGE)];
        self.grabbed.clear();
        self.stroke_indices.clear();
        self.stroke_before.clear();
        self.stroke_layers_before.clear();
        self.topology_changed = true;
        std::mem::replace(&mut self.mesh, mesh)
    }

    /// Logs a checkpoint of the mesh as it is now: the whole mesh after a
    /// topology change, or only the positions.
    fn checkpoint(&mut self, topology: bool) {
        if !self.recording {
            return;
        }
        let index = self.checkpoints.len() as u32;
        self.record(Event::Checkpoint(index));
        // A replay that starts or snaps here starts with the default.
        self.logged_falloff = Falloff::Smooth;
        let levels = topology.then(|| self.levels_for_checkpoint()).flatten();
        let checkpoint = Checkpoint::of(&self.mesh, levels, self.log.len(), self.clock, topology);
        self.checkpoints.push(checkpoint);
        self.last_checkpoint = self.clock;
    }

    /// Sets every position, as a checkpoint of the positions holds them.
    fn snap_positions(&mut self, positions: &[Vec3]) {
        if positions.len() != self.mesh.positions.len() {
            return;
        }
        if self.mesh.positions != positions {
            let moved: Vec<u32> = (0..positions.len() as u32)
                .filter(|&v| self.mesh.positions[v as usize] != positions[v as usize])
                .collect();
            self.mesh.positions.copy_from_slice(positions);
            self.bvh.touch(&self.mesh, &moved);
            self.bvh_stale = true;
        }
        self.mesh.recompute_normals();
        self.dirty_pages.iter_mut().for_each(|d| *d = true);
    }

    /// Sets the time that the next log entries carry, in seconds.
    pub fn set_clock(&mut self, seconds: f64) {
        self.clock = seconds;
    }

    /// Everything done to the clay so far, in order.
    pub fn log(&self) -> &[Entry] {
        &self.log
    }

    fn record(&mut self, event: Event) {
        self.settle_resume();
        if self.recording {
            self.log.push(Entry {
                time: self.clock,
                event,
            });
        }
    }

    /// Brings the hierarchy up to date with the sculpted surface. Called once
    /// per frame rather than after every dab: within a frame the boxes are a
    /// few dabs behind, which only matters at the very edge of a stroke.
    ///
    /// What the brushes find depends on when this happens, so replay has to
    /// refit at the same moments: the log says when.
    pub fn refresh(&mut self) {
        if self.bvh_stale {
            self.record(Event::Refresh);
            self.bvh.refit_touched(&self.mesh);
            self.bvh_stale = false;
        }
    }

    pub fn raycast(&self, origin: Vec3, dir: Vec3) -> Option<Hit> {
        self.bvh.raycast(&self.mesh, origin, dir)
    }

    /// The surface's normal at a hit: its triangle's vertex normals,
    /// blended by where the hit lies in it, so it turns smoothly from one
    /// triangle to the next.
    pub fn normal_at(&self, hit: &Hit) -> Vec3 {
        let Some(&t) = self.mesh.triangles.get(hit.triangle as usize) else {
            return Vec3::Y;
        };
        let [a, b, c] = t.map(|v| self.mesh.positions[v as usize]);
        let [na, nb, nc] = t.map(|v| self.mesh.normals[v as usize]);
        // Barycentric weights from the areas of the three sub-triangles.
        let n = (b - a).cross(c - a);
        let area = n.length_squared();
        if area <= 0.0 {
            return na;
        }
        let p = hit.point;
        let wa = (c - b).cross(p - b).dot(n) / area;
        let wb = (a - c).cross(p - c).dot(n) / area;
        let wc = 1.0 - wa - wb;
        (na * wa + nb * wb + nc * wc).normalize_or(n.normalize_or(Vec3::Y))
    }

    /// Takes the vertex ranges that changed since the last call, for upload.
    pub fn take_dirty(&mut self) -> Vec<Range<usize>> {
        let n = self.mesh.positions.len();
        let mut ranges: Vec<Range<usize>> = Vec::new();
        for (page, dirty) in self.dirty_pages.iter_mut().enumerate() {
            if std::mem::take(dirty) {
                let range = page * PAGE..((page + 1) * PAGE).min(n);
                match ranges.last_mut() {
                    Some(last) if last.end == range.start => last.end = range.end,
                    _ => ranges.push(range),
                }
            }
        }
        ranges
    }

    pub fn begin_stroke(&mut self) {
        self.record(Event::Begin);
        self.stroke_open = true;
        self.touched.next();
        self.stroke_indices.clear();
        self.stroke_before.clear();
        self.stroke_layers_before.clear();
        self.grabbed.clear();
    }

    /// Closes the stroke and makes it one undo step.
    pub fn end_stroke(&mut self) {
        // Nothing open (a camera drag ended, a timelapse started): nothing
        // to log. A stray `End` would count as an edit for the autosave,
        // and on a reopened scene log a whole checkpoint.
        if !std::mem::take(&mut self.stroke_open) {
            return;
        }
        self.record(Event::End);
        if self.stroke_indices.is_empty() {
            return;
        }
        let indices = std::mem::take(&mut self.stroke_indices);
        let before = std::mem::take(&mut self.stroke_before);
        let layers_before = std::mem::take(&mut self.stroke_layers_before);
        self.push_edit(indices, before, layers_before);
        if self.recording && self.clock - self.last_checkpoint >= CHECKPOINT_EVERY {
            // The replay recomputes the normals when it snaps: so does this.
            self.mesh.recompute_normals();
            self.dirty_pages.iter_mut().for_each(|d| *d = true);
            self.checkpoint(false);
        }
    }

    /// Makes the change of `indices` from `before` (positions) and
    /// `layers_before` (mask and chalk) to what they are now one undo
    /// step. What didn't change isn't kept, except the positions of a
    /// stroke that changed nothing at all.
    fn push_edit(
        &mut self,
        indices: Vec<u32>,
        mut before: Vec<Vec3>,
        mut layers_before: Vec<[f32; 2]>,
    ) {
        let mut after: Vec<Vec3> = indices
            .iter()
            .map(|&v| self.mesh.positions[v as usize])
            .collect();
        let mut layers_after: Vec<[f32; 2]> = indices.iter().map(|&v| self.layers_at(v)).collect();
        if layers_after == layers_before {
            layers_after.clear();
            layers_before.clear();
        } else if after == before {
            after.clear();
            before.clear();
        }
        self.undo.push(Step::Move(Edit {
            indices,
            before,
            after,
            layers_before,
            layers_after,
        }));
        self.redo.clear();
        self.forget_oldest_steps(UNDO_BUDGET);
    }

    /// Drops the oldest undo steps while the stack holds more than
    /// `budget` bytes ([`UNDO_BUDGET`]), always keeping the newest. A
    /// replay drops the same ones, so it still undoes what the log says.
    fn forget_oldest_steps(&mut self, budget: usize) {
        let mut total: usize = self.undo.iter().map(Step::bytes).sum();
        let mut old = 0;
        while total > budget && old + 1 < self.undo.len() {
            total -= self.undo[old].bytes();
            old += 1;
        }
        self.undo.drain(..old);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        match self.undo.pop() {
            Some(Step::Move(edit)) => {
                self.record(Event::Undo);
                self.apply_edit(&edit, false);
                self.redo.push(Step::Move(edit));
            }
            Some(Step::Layer(kind, layer)) => {
                self.record(Event::Undo);
                let other = self.swap_layer(kind, layer);
                self.redo.push(Step::Layer(kind, other));
            }
            Some(Step::Topology(old)) => {
                // Logged as a checkpoint of the mesh it brings back.
                let current = self.swap_topology(*old);
                self.undo.clear();
                self.redo.clear();
                self.redo.push(Step::Topology(Box::new(current)));
                self.checkpoint(true);
            }
            Some(step) => {
                self.record(Event::Undo);
                let step = self.undo_level_step(step, true);
                self.redo.push(step);
            }
            None => {}
        }
    }

    /// Puts a mesh and its levels in place; returns those they replaced.
    fn swap_topology(&mut self, (mesh, levels): (Mesh, Option<Levels>)) -> (Mesh, Option<Levels>) {
        let mesh = self.swap_mesh(mesh);
        (mesh, std::mem::replace(&mut self.levels, levels))
    }

    pub fn redo(&mut self) {
        match self.redo.pop() {
            Some(Step::Move(edit)) => {
                self.record(Event::Redo);
                self.apply_edit(&edit, true);
                self.undo.push(Step::Move(edit));
            }
            Some(Step::Layer(kind, layer)) => {
                self.record(Event::Redo);
                let other = self.swap_layer(kind, layer);
                self.undo.push(Step::Layer(kind, other));
            }
            Some(Step::Topology(new)) => {
                let current = self.swap_topology(*new);
                self.undo.clear();
                self.redo.clear();
                self.undo.push(Step::Topology(Box::new(current)));
                self.checkpoint(true);
            }
            Some(step) => {
                self.record(Event::Redo);
                let step = self.undo_level_step(step, false);
                self.undo.push(step);
            }
            None => {}
        }
    }

    /// Puts the vertices of a stroke back as they were before it, or
    /// (`after`) as it left them.
    fn apply_edit(&mut self, edit: &Edit, after: bool) {
        let (positions, layers) = if after {
            (&edit.after, &edit.layers_after)
        } else {
            (&edit.before, &edit.layers_before)
        };
        if !positions.is_empty() {
            self.apply_positions(&edit.indices, positions);
        }
        let n = self.mesh.positions.len();
        for (&v, &[mask, chalk]) in edit.indices.iter().zip(layers) {
            let at = v as usize;
            self.mesh.layers.mask.set(at, mask, n);
            self.mesh.layers.chalk.set(at, chalk, n);
            self.dirty_pages[at / PAGE] = true;
        }
    }

    fn apply_positions(&mut self, indices: &[u32], positions: &[Vec3]) {
        for (&v, &p) in indices.iter().zip(positions) {
            self.mesh.positions[v as usize] = p;
        }
        self.moved.clear();
        self.moved.extend_from_slice(indices);
        self.after_move();
    }

    /// Remembers where a vertex was, and its mask and chalk, before this
    /// stroke first changed it.
    fn touch(&mut self, v: u32) {
        if self.stroke_open && self.touched.mark(v) {
            self.stroke_slot[v as usize] = self.stroke_indices.len() as u32;
            self.stroke_indices.push(v);
            self.stroke_before.push(self.mesh.positions[v as usize]);
            self.stroke_layers_before.push(self.layers_at(v));
        }
    }

    /// The mask and chalk at vertex `v`.
    fn layers_at(&self, v: u32) -> [f32; 2] {
        let layers = &self.mesh.layers;
        [layers.mask.get(v as usize), layers.chalk.get(v as usize)]
    }

    /// How free vertex `v` is to move: 1 unmasked, 0 frozen.
    fn free(&self, v: u32) -> f32 {
        1.0 - self.mesh.layers.mask.get(v as usize)
    }

    /// Moves vertex `v` towards `p`, as far as the mask lets it,
    /// remembering where it was for undo. A frozen vertex stays.
    fn place(&mut self, v: u32, p: Vec3) {
        let free = self.free(v);
        if free <= 0.0 {
            return;
        }
        let p = if free < 1.0 {
            let start = self.mesh.positions[v as usize];
            start + (p - start) * free
        } else {
            p
        };
        self.touch(v);
        self.mesh.positions[v as usize] = p;
        self.moved.push(v);
    }

    /// Takes chalk off the vertices moved since `moved[from]`, as much as
    /// the dab weighs on each: its falloff times its strength, held back
    /// by the mask as the move was. A stroke wipes the chalk off where
    /// the brush went.
    fn wipe_chalk(&mut self, from: usize, dab: Dab) {
        if self.mesh.layers.chalk.is_empty() {
            return;
        }
        let n = self.mesh.positions.len();
        for i in from..self.moved.len() {
            let v = self.moved[i];
            let p = self.mesh.positions[v as usize];
            let w = self.weight(p.distance(dab.centre), dab.radius) * dab.strength * self.free(v);
            let chalk = &mut self.mesh.layers.chalk;
            let left = (chalk.get(v as usize) - w).max(0.0);
            chalk.set(v as usize, left, n);
        }
    }

    /// Replaces a whole layer; returns the one it replaced.
    fn swap_layer(&mut self, kind: layers::Kind, layer: Layer) -> Layer {
        self.dirty_pages.fill(true);
        std::mem::replace(self.mesh.layers.get_mut(kind), layer)
    }

    /// Changes a whole layer: clears or inverts the mask, adds or removes
    /// the chalk. One undo step; false, with nothing done or logged, if
    /// it would change nothing.
    pub fn edit_layer(&mut self, op: LayerOp) -> bool {
        if self.stroke_open {
            self.end_stroke();
        }
        let n = self.mesh.positions.len();
        let layer = op.apply(self.mesh.layers.get(op.kind()), n);
        if layer == *self.mesh.layers.get(op.kind()) {
            return false;
        }
        self.record(Event::Layer(op));
        let before = self.swap_layer(op.kind(), layer);
        self.undo.push(Step::Layer(op.kind(), before));
        self.redo.clear();
        self.forget_oldest_steps(UNDO_BUDGET);
        true
    }

    /// Normals, upload pages and the hierarchy after `self.moved` changed.
    fn after_move(&mut self) {
        if self.moved.is_empty() {
            return;
        }
        self.mesh
            .update_normals(&self.moved, &mut self.stamp, &mut self.affected);
        for &v in &self.affected {
            self.dirty_pages[v as usize / PAGE] = true;
        }
        self.bvh.touch(&self.mesh, &self.moved);
        self.bvh_stale = true;
    }

    /// Applies a dab of any brush but Move (and its mirror image).
    pub fn dab(&mut self, dab: Dab) {
        self.log_falloff();
        self.record(Event::Dab(dab));
        self.moved.clear();
        self.dab_at(dab);
        let mirrored = Dab {
            centre: mirror(dab.centre),
            direction: mirror(dab.direction),
            ..dab
        };
        if dab.symmetry && mirrored.centre.distance(dab.centre) > dab.radius * 0.05 {
            self.dab_at(mirrored);
        }
        self.after_move();
    }

    fn dab_at(&mut self, dab: Dab) {
        let mut found = std::mem::take(&mut self.found);
        found.clear();
        self.bvh.vertices_in_sphere(
            &self.mesh,
            dab.centre,
            dab.radius,
            &mut self.stamp,
            &mut found,
        );
        if !found.is_empty() {
            let from = self.moved.len();
            match dab.brush {
                Brush::Clay => self.clay(&found, dab),
                Brush::Flatten => self.flatten(&found, dab),
                Brush::Smooth => self.smooth(&found, dab),
                Brush::Inflate => self.inflate(&found, dab),
                Brush::Crease => self.crease(&found, dab),
                Brush::Pinch => self.pinch(&found, dab),
                Brush::Mask => self.mask(&found, dab),
                Brush::Move => {}
            }
            self.wipe_chalk(from, dab);
        }
        self.found = found;
    }

    /// The area under the brush as a plane: its falloff-weighted normal and
    /// average position. `None` if the normals cancel out.
    fn area_plane(&self, found: &[u32], dab: Dab) -> Option<(Vec3, Vec3)> {
        let (mut normal, mut position, mut weight) = (Vec3::ZERO, Vec3::ZERO, 0.0);
        for &v in found {
            let p = self.mesh.positions[v as usize];
            let w = self.weight(p.distance(dab.centre), dab.radius);
            normal += self.mesh.normals[v as usize] * w;
            position += p * w;
            weight += w;
        }
        let n = normal.normalize_or_zero();
        (weight > 0.0 && n != Vec3::ZERO).then(|| (n, position / weight))
    }

    /// Only the side facing the brush: never push through a thin form.
    fn faces(&self, v: u32, n: Vec3) -> bool {
        self.mesh.normals[v as usize].dot(n) > 0.0
    }

    fn clay(&mut self, found: &[u32], dab: Dab) {
        let Some((n, _)) = self.area_plane(found, dab) else {
            return;
        };
        // The plane goes through the point under the brush. (Through the
        // average position instead, it would sit below a curved surface and
        // lift the rim more than the centre.)
        let plane_point = dab.centre;
        let sign = if dab.invert { -1.0 } else { 1.0 };
        // How far above the area the clay builds up, at full strength.
        let height = dab.radius * 0.25 * dab.strength;
        // In layers: how far a vertex may rise in this stroke.
        let layer = dab.radius * LAYER * dab.strength;
        for &v in found {
            if !self.faces(v, n) {
                continue;
            }
            let p = self.mesh.positions[v as usize];
            let above = (p - plane_point).dot(n) * sign;
            if above >= height {
                continue;
            }
            let mut lift = (height - above).min(2.0 * height)
                * self.weight(p.distance(dab.centre), dab.radius)
                * 0.3;
            if dab.buildup == Buildup::Layer {
                lift = lift.min(layer - self.risen(v, n) * sign);
                if lift <= 0.0 {
                    continue;
                }
            }
            self.place(v, p + n * (sign * lift));
        }
    }

    /// How far vertex `v` has moved along `n` since the open stroke first
    /// touched it.
    fn risen(&self, v: u32, n: Vec3) -> f32 {
        if !self.stroke_open || !self.touched.is_marked(v) {
            return 0.0;
        }
        let before = self.stroke_before[self.stroke_slot[v as usize] as usize];
        (self.mesh.positions[v as usize] - before).dot(n)
    }

    fn flatten(&mut self, found: &[u32], dab: Dab) {
        // Through the average position, the plane cuts through the bumps and
        // passes over the hollows: flatten lowers one, fill raises the other.
        let Some((n, origin)) = self.area_plane(found, dab) else {
            return;
        };
        for &v in found {
            if !self.faces(v, n) {
                continue;
            }
            let p = self.mesh.positions[v as usize];
            let above = (p - origin).dot(n);
            if (above > 0.0) != dab.invert && above != 0.0 {
                let w = self.weight(p.distance(dab.centre), dab.radius) * dab.strength * 0.3;
                self.place(v, p - n * (above * w));
            }
        }
    }

    fn smooth(&mut self, found: &[u32], dab: Dab) {
        // Compute every target first, then move: the result does not depend
        // on the order of the vertices.
        self.smoothed.clear();
        for &v in found {
            let p = self.mesh.positions[v as usize];
            let ring = self.mesh.neighbours.of(v);
            let average = ring
                .iter()
                .map(|&u| self.mesh.positions[u as usize])
                .sum::<Vec3>()
                / ring.len() as f32;
            let w = self.weight(p.distance(dab.centre), dab.radius) * dab.strength;
            self.smoothed.push(p + (average - p) * w);
        }
        for (i, &v) in found.iter().enumerate() {
            self.place(v, self.smoothed[i]);
        }
    }

    fn inflate(&mut self, found: &[u32], dab: Dab) {
        let sign = if dab.invert { -1.0 } else { 1.0 };
        for &v in found {
            let p = self.mesh.positions[v as usize];
            let step =
                dab.radius * 0.05 * self.weight(p.distance(dab.centre), dab.radius) * dab.strength;
            self.place(v, p + self.mesh.normals[v as usize] * (sign * step));
        }
    }

    fn crease(&mut self, found: &[u32], dab: Dab) {
        let Some((n, _)) = self.area_plane(found, dab) else {
            return;
        };
        let sign = if dab.invert { -1.0 } else { 1.0 };
        let depth = dab.radius * 0.06 * dab.strength;
        // The stroke's line across the surface; zero until the stroke moves.
        let along = tangent(dab.direction, n).normalize_or_zero();
        for &v in found {
            if !self.faces(v, n) {
                continue;
            }
            let p = self.mesh.positions[v as usize];
            let w = self.weight(p.distance(dab.centre), dab.radius);
            // Squared falloff: a narrow cut rather than a dent.
            let cut = -n * (sign * depth * w * w);
            // The sides drawn in towards the line keep the groove sharp.
            let to_centre = tangent(dab.centre - p, n);
            let across = to_centre - along * to_centre.dot(along);
            self.place(v, p + cut + across * (0.1 * w * dab.strength));
        }
    }

    fn pinch(&mut self, found: &[u32], dab: Dab) {
        let Some((n, _)) = self.area_plane(found, dab) else {
            return;
        };
        let sign = if dab.invert { -1.0 } else { 1.0 };
        for &v in found {
            if !self.faces(v, n) {
                continue;
            }
            let p = self.mesh.positions[v as usize];
            // Along the surface only, and a fraction of the way: never past the centre.
            let w = self.weight(p.distance(dab.centre), dab.radius) * dab.strength * 0.2;
            self.place(v, p + tangent(dab.centre - p, n) * (sign * w));
        }
    }

    /// Raises the mask under the brush, or lowers it (`invert`), by the
    /// dab's weight on each vertex. Only the side facing the brush, as
    /// for the brushes that shape the clay.
    fn mask(&mut self, found: &[u32], dab: Dab) {
        let Some((n, _)) = self.area_plane(found, dab) else {
            return;
        };
        let sign = if dab.invert { -1.0 } else { 1.0 };
        let count = self.mesh.positions.len();
        for &v in found {
            if !self.faces(v, n) {
                continue;
            }
            let p = self.mesh.positions[v as usize];
            let w = self.weight(p.distance(dab.centre), dab.radius) * dab.strength;
            let old = self.mesh.layers.mask.get(v as usize);
            let new = (old + sign * w).clamp(0.0, 1.0);
            if new != old {
                self.touch(v);
                self.mesh.layers.mask.set(v as usize, new, count);
                self.dirty_pages[v as usize / PAGE] = true;
            }
        }
    }

    /// Move brush, at the start of a stroke: takes hold of the surface
    /// around `centre` (and its mirror image).
    pub fn grab(&mut self, centre: Vec3, radius: f32, symmetry: bool) {
        self.log_falloff();
        self.record(Event::Grab {
            centre,
            radius,
            symmetry,
        });
        self.grabbed.clear();
        self.grab_wiped = false;
        self.unique.next();
        let mut sides = vec![(centre, false)];
        if symmetry && mirror(centre).distance(centre) > radius * 0.05 {
            sides.push((mirror(centre), true));
        }
        let mut found = std::mem::take(&mut self.found);
        for (c, mirrored) in sides {
            found.clear();
            self.bvh
                .vertices_in_sphere(&self.mesh, c, radius, &mut self.stamp, &mut found);
            for &v in &found {
                // Held back by the mask; a frozen vertex isn't grabbed.
                let free = self.free(v);
                if free > 0.0 && self.unique.mark(v) {
                    let p = self.mesh.positions[v as usize];
                    let w = self.falloff.weight(p.distance(c), radius);
                    let w = if free < 1.0 { w * free } else { w };
                    self.grabbed.push((v, p, w, mirrored));
                }
            }
        }
        self.found = found;
        for i in 0..self.grabbed.len() {
            self.touch(self.grabbed[i].0);
        }
    }

    /// Move brush: places the grabbed surface `offset` away from where it was
    /// grabbed (mirrored on the other side).
    pub fn drag(&mut self, offset: Vec3) {
        self.record(Event::Drag(offset));
        // The first time it moves, the grabbed surface loses its chalk,
        // as much as each vertex is grabbed.
        if !self.grab_wiped && offset != Vec3::ZERO {
            self.grab_wiped = true;
            if !self.mesh.layers.chalk.is_empty() {
                let n = self.mesh.positions.len();
                for &(v, _, weight, _) in &self.grabbed {
                    let chalk = &mut self.mesh.layers.chalk;
                    let left = (chalk.get(v as usize) - weight).max(0.0);
                    chalk.set(v as usize, left, n);
                }
            }
        }
        self.moved.clear();
        for &(v, start, weight, mirrored) in &self.grabbed {
            let o = if mirrored { mirror(offset) } else { offset };
            self.mesh.positions[v as usize] = start + o * weight;
            self.moved.push(v);
        }
        self.after_move();
    }

    /// Copies one half of the clay onto the other, across the X = 0 plane:
    /// the +X half onto the −X half if `to_negative`, else the other way.
    /// The topology stays: each vertex of the other half moves to the
    /// mirror image of the surface found along its own (mirrored) normal.
    /// One undo step.
    pub fn mirror(&mut self, to_negative: bool) {
        if self.stroke_open {
            self.end_stroke();
        }
        self.refresh();
        self.record(Event::Mirror { to_negative });
        // The side that changes, as a sign of x.
        let side = if to_negative { -1.0 } else { 1.0 };
        // Rays start this far behind the mirrored point, so a surface
        // passing right through it is found too.
        let back = 0.05;
        // Farther than this, the surface found belongs to another part of
        // the form: the vertex stays.
        let reach = 0.25;
        let mut indices = Vec::new();
        let mut targets = Vec::new();
        for v in 0..self.mesh.positions.len() {
            let p = self.mesh.positions[v];
            if p.x * side <= 0.0 {
                continue;
            }
            let d = mirror(self.mesh.normals[v]);
            // A hair to the side: on a symmetric mesh the ray would pass
            // exactly through a vertex and might slip between its triangles.
            let m = mirror(p) + d.any_orthonormal_vector() * 1e-5;
            // Signed offsets along `d` of the surface on the kept side.
            let ahead = self
                .raycast(m - d * back, d)
                .map(|h| (h.distance - back, h.point));
            let behind = self
                .raycast(m + d * back, -d)
                .map(|h| (back - h.distance, h.point));
            let nearest = [ahead, behind]
                .into_iter()
                .flatten()
                .filter(|(offset, _)| offset.abs() <= reach)
                .min_by(|a, b| a.0.abs().total_cmp(&b.0.abs()));
            if let Some((_, q)) = nearest {
                // Held back by the mask, as under the brushes.
                let free = self.free(v as u32);
                if free <= 0.0 {
                    continue;
                }
                let target = if free < 1.0 {
                    p + (mirror(q) - p) * free
                } else {
                    mirror(q)
                };
                if target != p {
                    indices.push(v as u32);
                    targets.push(target);
                }
            }
        }
        if indices.is_empty() {
            return;
        }
        let before = indices
            .iter()
            .map(|&v| self.mesh.positions[v as usize])
            .collect();
        let layers = indices.iter().map(|&v| self.layers_at(v)).collect();
        self.apply_positions(&indices, &targets);
        self.push_edit(indices, before, layers);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sculpt() -> Sculpt {
        Sculpt::new(Mesh::icosphere(4))
    }

    fn dab(brush: Brush, centre: Vec3) -> Dab {
        Dab {
            brush,
            centre,
            radius: 0.3,
            strength: 1.0,
            invert: false,
            symmetry: false,
            direction: Vec3::ZERO,
            buildup: Default::default(),
        }
    }

    /// The vertex that started nearest to `target`.
    fn nearest(original: &[Vec3], target: Vec3) -> usize {
        (0..original.len())
            .min_by(|&a, &b| {
                original[a]
                    .distance(target)
                    .total_cmp(&original[b].distance(target))
            })
            .unwrap()
    }

    /// A stroke of `count` dabs from `from` towards +X, along the sphere.
    fn stroke(s: &mut Sculpt, brush: Brush, from: Vec3, count: usize) {
        s.begin_stroke();
        for i in 0..count {
            let centre = (from + Vec3::X * (i as f32 * 0.04)).normalize();
            let direction = if i == 0 { Vec3::ZERO } else { Vec3::X };
            s.dab(Dab {
                direction,
                ..dab(brush, centre)
            });
            s.refresh();
        }
        s.end_stroke();
    }

    #[test]
    fn falloffs_run_from_full_at_the_centre_to_nothing_at_the_rim() {
        for falloff in Falloff::ALL {
            assert_eq!(falloff.weight(0.0, 1.0), 1.0);
            assert_eq!(falloff.weight(1.0, 1.0), 0.0);
            assert_eq!(falloff.weight(2.0, 1.0), 0.0);
        }
        // Flat holds its full weight over the inner half; sharp drops first.
        assert_eq!(Falloff::Flat.weight(0.5, 1.0), 1.0);
        let at = |f: Falloff| f.weight(0.4, 1.0);
        assert!(at(Falloff::Flat) > at(Falloff::Smooth));
        assert!(at(Falloff::Smooth) > at(Falloff::Sharp));
    }

    /// A clay stroke as the app makes it, across the top of the sphere:
    /// each dab centred where a ray from above meets the clay now.
    fn clay_stroke(s: &mut Sculpt, xs: &[f32], buildup: Buildup) {
        s.begin_stroke();
        for &x in xs {
            let hit = s.raycast(Vec3::new(x, 0.0, 3.0), -Vec3::Z).unwrap();
            s.dab(Dab {
                strength: 0.5,
                radius: 0.2,
                direction: Vec3::X,
                buildup,
                ..dab(Brush::Clay, hit.point)
            });
            s.refresh();
        }
        s.end_stroke();
    }

    /// The clay's height above the point (0, y), in brush radii (0.2).
    fn height_at(s: &Sculpt, y: f32) -> f32 {
        s.raycast(Vec3::new(0.0, y, 3.0), -Vec3::Z).unwrap().point.z / 0.2
    }

    /// Dabs every 0.15 radius along X, as the app spaces them.
    fn path() -> Vec<f32> {
        (0..=20).map(|i| -0.3 + i as f32 * 0.03).collect()
    }

    #[test]
    fn a_stroke_in_layers_builds_once_however_often_it_passes() {
        let there_and_back: Vec<f32> = (0..5)
            .flat_map(|k| {
                let mut xs = path();
                if k % 2 == 1 {
                    xs.reverse();
                }
                xs
            })
            .collect();
        for falloff in Falloff::ALL {
            let built = |xs: &[f32], buildup: Buildup, strokes: usize| {
                let mut s = Sculpt::new(Mesh::icosphere(5));
                s.set_falloff(falloff);
                let start = height_at(&s, 0.0);
                for _ in 0..strokes {
                    clay_stroke(&mut s, xs, buildup);
                }
                height_at(&s, 0.0) - start
            };
            let once = built(&path(), Buildup::Layer, 1);
            let scrubbed = built(&there_and_back, Buildup::Layer, 1);
            // Going over it fills up to the layer (a sharp falloff doesn't
            // reach it in one pass) and no further. Measured along the
            // view, not the plane's normal: a little over on the curve.
            let layer = LAYER * 0.5;
            assert!(once <= scrubbed + 0.01, "{falloff:?}: {once} once");
            assert!(
                (scrubbed - layer).abs() < 0.1 * layer,
                "{falloff:?}: {scrubbed} scrubbed"
            );
            // A new stroke lays the next layer.
            let twice = built(&path(), Buildup::Layer, 2);
            assert!(twice > 1.8 * once, "{falloff:?}: {twice}");
            // Accumulating, going over it again builds more.
            let piled = built(&there_and_back, Buildup::Accumulate, 1);
            assert!(piled > 2.0 * scrubbed, "{falloff:?}: {piled}");
        }
    }

    #[test]
    fn clay_builds_up_most_at_the_centre_and_undo_restores_it() {
        let mut s = sculpt();
        let original = s.mesh.positions.clone();
        let top = Vec3::new(0.0, 0.0, 1.0);
        s.begin_stroke();
        for _ in 0..5 {
            s.dab(dab(Brush::Clay, top));
            s.refresh();
        }
        s.end_stroke();
        let height = |p: Vec3| p.length();
        let centre = nearest(&original, top);
        let edge = nearest(&original, Vec3::new(0.25, 0.0, 0.97).normalize());
        let far = nearest(&original, Vec3::new(0.0, 0.0, -1.0));
        assert!(
            height(s.mesh.positions[centre]) > 1.01,
            "clay adds at the centre"
        );
        assert!(height(s.mesh.positions[centre]) > height(s.mesh.positions[edge]));
        assert_eq!(
            s.mesh.positions[far], original[far],
            "the far side is untouched"
        );

        s.undo();
        assert_eq!(s.mesh.positions, original);
        s.redo();
        assert!(height(s.mesh.positions[centre]) > 1.01);
    }

    #[test]
    fn every_brush_stays_symmetric_and_undoes() {
        for brush in shaping().filter(|&b| b != Brush::Move) {
            let mut s = sculpt();
            let original = s.mesh.positions.clone();
            s.begin_stroke();
            for i in 0..4 {
                s.dab(Dab {
                    symmetry: true,
                    direction: if i == 0 { Vec3::ZERO } else { Vec3::Y },
                    ..dab(
                        brush,
                        Vec3::new(0.5, 0.3 + i as f32 * 0.03, 0.8).normalize(),
                    )
                });
                s.refresh();
            }
            s.end_stroke();
            assert_ne!(s.mesh.positions, original, "{brush:?} did nothing");
            // Every vertex has a mirror twin on the icosphere: compare both sides.
            for (i, p) in s.mesh.positions.iter().enumerate() {
                if p.x > 0.2 {
                    let twin = s
                        .mesh
                        .positions
                        .iter()
                        .min_by(|a, b| a.distance(mirror(*p)).total_cmp(&b.distance(mirror(*p))))
                        .unwrap();
                    assert!(
                        twin.distance(mirror(*p)) < 1e-4,
                        "{brush:?}: vertex {i} lost its mirror twin"
                    );
                }
            }
            s.undo();
            assert_eq!(s.mesh.positions, original, "{brush:?} did not undo");
        }
    }

    #[test]
    fn smoothing_reduces_roughness() {
        let mut s = sculpt();
        for (i, p) in s.mesh.positions.iter_mut().enumerate() {
            *p *= if i % 2 == 0 { 1.02 } else { 0.98 };
        }
        let roughness = |s: &Sculpt| {
            let c = Vec3::Z;
            (0..s.mesh.positions.len() as u32)
                .filter(|&v| s.mesh.positions[v as usize].distance(c) < 0.25)
                .map(|v| {
                    let ring = s.mesh.neighbours.of(v);
                    let avg = ring
                        .iter()
                        .map(|&u| s.mesh.positions[u as usize])
                        .sum::<Vec3>()
                        / ring.len() as f32;
                    avg.distance(s.mesh.positions[v as usize])
                })
                .sum::<f32>()
        };
        let before = roughness(&s);
        s.dab(dab(Brush::Smooth, Vec3::Z));
        assert!(roughness(&s) < before * 0.8);
    }

    #[test]
    fn flatten_cuts_the_dome_towards_a_plane() {
        let mut s = sculpt();
        let original = s.mesh.positions.clone();
        let top = nearest(&original, Vec3::Z);
        let flank = nearest(&original, Vec3::new(0.2, 0.0, 0.98).normalize());
        s.begin_stroke();
        for _ in 0..10 {
            s.dab(dab(Brush::Flatten, Vec3::Z));
            s.refresh();
        }
        s.end_stroke();
        let z = |s: &Sculpt, v: usize| s.mesh.positions[v].z;
        assert!(z(&s, top) < original[top].z - 0.005, "the top comes down");
        // Flatter: the top and its flank end up nearer the same height.
        let gap_before = original[top].z - original[flank].z;
        let gap_after = z(&s, top) - z(&s, flank);
        assert!(gap_after < gap_before * 0.7, "{gap_before} → {gap_after}");
        assert!(
            s.mesh.positions.iter().all(|p| p.length() <= 1.0 + 1e-5),
            "flatten never adds"
        );

        // Inverted, it fills a hollow and never cuts.
        let mut s = sculpt();
        for _ in 0..5 {
            s.dab(Dab {
                invert: true,
                radius: 0.15,
                ..dab(Brush::Clay, Vec3::Z)
            });
            s.refresh();
        }
        let dug = s.mesh.positions.clone();
        for _ in 0..10 {
            s.dab(Dab {
                invert: true,
                ..dab(Brush::Flatten, Vec3::Z)
            });
            s.refresh();
        }
        assert!(
            s.mesh.positions[top].z > dug[top].z + 0.005,
            "the hollow fills"
        );
        for (now, before) in s.mesh.positions.iter().zip(&dug) {
            assert!(
                now.length() >= before.length() - 1e-5,
                "fill cut {before} to {now}"
            );
        }
    }

    #[test]
    fn inflate_swells_and_deflate_shrinks() {
        for (invert, grows) in [(false, true), (true, false)] {
            let mut s = sculpt();
            let original = s.mesh.positions.clone();
            s.dab(Dab {
                invert,
                ..dab(Brush::Inflate, Vec3::Z)
            });
            let top = nearest(&original, Vec3::Z);
            let length = s.mesh.positions[top].length();
            assert_eq!(length > 1.005, grows, "invert {invert}: {length}");
            assert_eq!(length < 0.995, !grows, "invert {invert}: {length}");
        }
    }

    #[test]
    fn crease_cuts_a_groove_along_the_stroke() {
        let mut s = sculpt();
        let original = s.mesh.positions.clone();
        let start = Vec3::new(-0.1, 0.0, 1.0);
        stroke(&mut s, Brush::Crease, start, 6);
        // Across the middle of the stroke: the line sinks more than its sides.
        let mid = Vec3::new(0.0, 0.0, 1.0).normalize();
        let side = Vec3::new(0.0, 0.2, 1.0).normalize();
        let depth = |target| {
            let v = nearest(&original, target);
            1.0 - s.mesh.positions[v].length()
        };
        assert!(depth(mid) > 0.01, "the line sinks: {}", depth(mid));
        assert!(depth(mid) > depth(side) * 3.0);

        // Inverted, it raises a ridge instead.
        let mut r = sculpt();
        r.begin_stroke();
        for i in 0..6 {
            let centre = (start + Vec3::X * (i as f32 * 0.04)).normalize();
            r.dab(Dab {
                invert: true,
                direction: if i == 0 { Vec3::ZERO } else { Vec3::X },
                ..dab(Brush::Crease, centre)
            });
            r.refresh();
        }
        r.end_stroke();
        let v = nearest(&original, mid);
        assert!(r.mesh.positions[v].length() > 1.01);
    }

    #[test]
    fn pinch_draws_the_ring_in_and_spread_pushes_it_out() {
        let ring = |s: &Sculpt, original: &[Vec3]| {
            let v = nearest(original, Vec3::new(0.15, 0.0, 0.99).normalize());
            let p = s.mesh.positions[v];
            Vec3::new(p.x, p.y, 0.0).length()
        };
        let mut s = sculpt();
        let original = s.mesh.positions.clone();
        let before = ring(&s, &original);
        s.dab(dab(Brush::Pinch, Vec3::Z));
        assert!(ring(&s, &original) < before * 0.95, "pinched in");
        // Mostly sideways: pinch barely changes the sphere's radius.
        let v = nearest(&original, Vec3::new(0.15, 0.0, 0.99).normalize());
        assert!((s.mesh.positions[v].length() - 1.0).abs() < 0.01);

        let mut s = sculpt();
        s.dab(Dab {
            invert: true,
            ..dab(Brush::Pinch, Vec3::Z)
        });
        assert!(ring(&s, &original) > before * 1.05, "spread out");
    }

    #[test]
    fn move_drags_the_grabbed_area_by_the_offset_at_its_centre() {
        let mut s = sculpt();
        let top = Vec3::Z;
        let centre = nearest(&s.mesh.positions, top);
        let start = s.mesh.positions[centre];
        s.begin_stroke();
        s.grab(top, 0.4, false);
        s.drag(Vec3::new(0.0, 0.0, 0.3));
        s.end_stroke();
        let moved = s.mesh.positions[centre] - start;
        assert!(moved.z > 0.25, "{moved:?}");
        s.undo();
        assert_eq!(s.mesh.positions[centre], start);
    }

    #[test]
    fn mirror_copies_one_half_onto_the_other_and_undoes() {
        let mut s = sculpt();
        // A lump on the +X side only.
        let at = Vec3::new(0.6, 0.2, 0.75).normalize();
        s.begin_stroke();
        for _ in 0..6 {
            s.dab(dab(Brush::Clay, at));
            s.refresh();
        }
        s.end_stroke();
        s.refresh();
        // How far the surface is from where the mirror image of each +X
        // vertex says it should be, at its worst.
        let asymmetry = |s: &Sculpt| {
            (0..s.mesh.positions.len())
                .filter(|&v| s.mesh.positions[v].x > 0.05)
                .map(|v| {
                    let m = mirror(s.mesh.positions[v]);
                    let n = mirror(s.mesh.normals[v]);
                    // A little to the side: a ray right through a vertex
                    // may slip between its triangles.
                    let aside = n.any_orthonormal_vector() * 1e-3;
                    s.raycast(m + n * 0.1 + aside, -n)
                        .map_or(1.0, |h| (h.distance - 0.1).abs())
                })
                .fold(0.0, f32::max)
        };
        let before = s.mesh.positions.clone();
        assert!(asymmetry(&s) > 0.02, "the lump is one-sided");
        s.mirror(true);
        s.refresh();
        assert!(asymmetry(&s) < 0.005, "{}", asymmetry(&s));
        // The kept half didn't move.
        for (now, was) in s.mesh.positions.iter().zip(&before) {
            if was.x >= 0.0 {
                assert_eq!(now, was);
            }
        }
        s.undo();
        assert_eq!(s.mesh.positions, before);
    }

    #[test]
    fn undo_forgets_the_oldest_strokes_past_its_budget() {
        let mut s = sculpt();
        let start = s.mesh.positions.clone();
        for k in 0..4 {
            stroke(
                &mut s,
                Brush::Clay,
                Vec3::new(0.2 * k as f32 - 0.3, 0.3, 0.9),
                3,
            );
        }
        let after_first: Vec<usize> = s.undo.iter().map(Step::bytes).collect();
        assert_eq!(after_first.len(), 4);
        // Room for the newest two only.
        let budget = after_first[2] + after_first[3];
        s.forget_oldest_steps(budget);
        assert_eq!(s.undo.len(), 2);
        // The two newest still undo; the clay keeps the first two strokes.
        s.undo();
        s.undo();
        s.undo();
        assert!(!s.can_undo());
        assert_ne!(s.mesh.positions, start);
        // Always the newest, however small the budget.
        s.redo();
        s.forget_oldest_steps(0);
        assert_eq!(s.undo.len(), 1);
    }

    #[test]
    fn replacing_the_mesh_resizes_everything_and_undo_brings_it_back() {
        let mut s = sculpt();
        let small = s.mesh.positions.len();
        stroke(&mut s, Brush::Clay, Vec3::new(0.0, 0.3, 0.9), 4);
        let sculpted = s.mesh.positions.clone();
        s.replace_mesh(Mesh::icosphere(5));
        assert!(s.take_topology_changed());
        assert!(!s.take_topology_changed());
        let large = s.mesh.positions.len();
        assert!(large > small);
        assert_eq!(s.take_dirty(), vec![0..large]);
        // Sculpting works on the new mesh, all the way to its last vertex.
        stroke(&mut s, Brush::Inflate, Vec3::new(0.0, -0.3, 0.9), 4);
        stroke(&mut s, Brush::Smooth, Vec3::new(0.0, 0.0, -1.0), 2);
        s.begin_stroke();
        s.grab(Vec3::Y, 0.3, true);
        s.drag(Vec3::Y * 0.1);
        s.end_stroke();
        // Undo walks back the strokes, then the mesh.
        for _ in 0..3 {
            s.undo();
        }
        assert_eq!(s.mesh.positions.len(), large);
        s.undo();
        assert!(s.take_topology_changed());
        assert_eq!(s.mesh.positions, sculpted);
        assert!(!s.can_undo(), "strokes before the remesh are gone");
        s.redo();
        assert_eq!(s.mesh.positions.len(), large);
        let topology = s
            .log()
            .iter()
            .filter(|e| matches!(e.event, Event::Checkpoint(_)))
            .count();
        assert_eq!(topology, 3);
        assert_eq!(s.checkpoints().len(), 4);
    }

    /// One stroke of `brush` across the top, as a user would make it:
    /// dabs, or a grab and drags for Move.
    fn any_stroke(s: &mut Sculpt, brush: Brush, symmetry: bool) {
        let at = Vec3::new(0.15, 0.1, 1.0).normalize();
        s.begin_stroke();
        if brush == Brush::Move {
            s.grab(at, 0.4, symmetry);
            s.drag(Vec3::new(0.1, 0.05, 0.2));
            s.drag(Vec3::new(0.1, 0.1, 0.3));
        } else {
            for i in 0..5 {
                s.dab(Dab {
                    symmetry,
                    direction: if i == 0 { Vec3::ZERO } else { Vec3::Y },
                    ..dab(brush, (at + Vec3::Y * (0.03 * i as f32)).normalize())
                });
                s.refresh();
            }
        }
        s.end_stroke();
        s.refresh();
    }

    /// Every brush but the mask itself.
    fn shaping() -> impl Iterator<Item = Brush> {
        Brush::ALL.into_iter().filter(|&b| b != Brush::Mask)
    }

    #[test]
    fn a_full_mask_freezes_the_clay_under_every_brush() {
        for brush in shaping() {
            let mut s = sculpt();
            assert!(s.edit_layer(LayerOp::InvertMask), "all masked");
            let original = s.mesh.positions.clone();
            any_stroke(&mut s, brush, true);
            assert_eq!(s.mesh.positions, original, "{brush:?} moved masked clay");
        }
    }

    #[test]
    fn half_a_mask_holds_its_half_still_even_with_symmetry() {
        for brush in shaping() {
            let mut s = sculpt();
            let n = s.mesh.positions.len();
            for v in 0..n {
                if s.mesh.positions[v].x > 0.0 {
                    s.mesh.layers.mask.set(v, 1.0, n);
                }
            }
            let original = s.mesh.positions.clone();
            any_stroke(&mut s, brush, true);
            let mut free_moved = false;
            for (v, (now, was)) in s.mesh.positions.iter().zip(&original).enumerate() {
                if was.x > 0.0 {
                    assert_eq!(now, was, "{brush:?} moved masked vertex {v}");
                } else if now != was {
                    free_moved = true;
                }
            }
            assert!(free_moved, "{brush:?}: the mirrored side, unmasked, moves");
            s.undo();
            assert_eq!(s.mesh.positions, original);
        }
    }

    #[test]
    fn a_partial_mask_lets_the_clay_move_that_much_less() {
        let mut free = sculpt();
        let mut half = sculpt();
        let n = half.mesh.positions.len();
        for v in 0..n {
            half.mesh.layers.mask.set(v, 0.75, n);
        }
        let original = free.mesh.positions.clone();
        free.dab(dab(Brush::Inflate, Vec3::Z));
        half.dab(dab(Brush::Inflate, Vec3::Z));
        let top = nearest(&original, Vec3::Z);
        let moved = |s: &Sculpt| s.mesh.positions[top].distance(original[top]);
        assert!(moved(&free) > 0.01);
        assert!((moved(&half) - moved(&free) * 0.25).abs() < 1e-5);
    }

    #[test]
    fn the_mask_tool_paints_and_its_opposite_takes_off_and_both_undo() {
        let mut s = sculpt();
        let original = s.mesh.positions.clone();
        let top = nearest(&original, Vec3::Z);
        let far = nearest(&original, -Vec3::Z);
        stroke(&mut s, Brush::Mask, Vec3::new(-0.1, 0.0, 1.0), 3);
        assert_eq!(s.mesh.positions, original, "masking moves nothing");
        assert_eq!(s.mesh.layers.mask.get(top), 1.0);
        assert_eq!(s.mesh.layers.mask.get(far), 0.0);
        let masked = s.mesh.layers.clone();
        // The opposite action lowers it.
        s.begin_stroke();
        s.dab(Dab {
            invert: true,
            strength: 0.4,
            ..dab(Brush::Mask, Vec3::Z)
        });
        s.end_stroke();
        let w = Falloff::Smooth.weight(original[top].distance(Vec3::Z), 0.3);
        assert!((s.mesh.layers.mask.get(top) - (1.0 - 0.4 * w)).abs() < 1e-6);
        s.undo();
        assert_eq!(s.mesh.layers, masked);
        s.undo();
        assert_eq!(s.mesh.layers, Layers::default());
        assert!(!s.can_undo());
        s.redo();
        assert_eq!(s.mesh.layers, masked);
        assert_eq!(s.mesh.positions, original);
        // The undo step keeps the mask alone, not the positions.
        let Some(Step::Move(edit)) = s.undo.last() else {
            panic!("a stroke step");
        };
        assert!(edit.before.is_empty() && !edit.layers_before.is_empty());
    }

    #[test]
    fn invert_and_clear_mask_undo_and_redo() {
        let mut s = sculpt();
        let n = s.mesh.positions.len();
        s.mesh.layers.mask.set(3, 0.25, n);
        assert!(!s.edit_layer(LayerOp::RemoveChalk), "no chalk to remove");
        assert!(s.edit_layer(LayerOp::InvertMask));
        assert_eq!(s.mesh.layers.mask.get(3), 0.75);
        assert_eq!(s.mesh.layers.mask.get(4), 1.0);
        assert!(s.edit_layer(LayerOp::ClearMask));
        assert!(s.mesh.layers.mask.is_empty());
        assert!(!s.edit_layer(LayerOp::ClearMask), "nothing left to clear");
        s.undo();
        assert_eq!(s.mesh.layers.mask.get(4), 1.0);
        s.undo();
        assert_eq!(s.mesh.layers.mask.get(3), 0.25);
        assert_eq!(s.mesh.layers.mask.get(4), 0.0);
        s.redo();
        s.redo();
        assert!(s.mesh.layers.mask.is_empty());
        assert_eq!(s.take_dirty(), vec![0..n]);
    }

    #[test]
    fn strokes_wipe_the_chalk_where_they_pass_and_undo_brings_it_back() {
        for brush in shaping() {
            let mut s = sculpt();
            assert!(s.edit_layer(LayerOp::AddChalk));
            let original = s.mesh.positions.clone();
            let chalked = s.mesh.layers.clone();
            assert_eq!(chalked.chalk.get(0), 1.0);
            any_stroke(&mut s, brush, false);
            let chalk = |v: usize| s.mesh.layers.chalk.get(v);
            let under = nearest(&original, Vec3::new(0.15, 0.15, 1.0).normalize());
            let far = nearest(&original, -Vec3::Z);
            assert!(chalk(under) < 0.5, "{brush:?} left {} chalk", chalk(under));
            assert_eq!(chalk(far), 1.0, "{brush:?} wiped far away");
            // Only where the clay moved.
            for (v, was) in original.iter().enumerate() {
                if s.mesh.positions[v] == *was {
                    assert!(chalk(v) > 0.999, "{brush:?}: vertex {v} didn't move");
                }
            }
            s.undo();
            assert_eq!(s.mesh.positions, original);
            assert_eq!(s.mesh.layers, chalked, "{brush:?}: the chalk is back");
            s.redo();
            assert!(s.mesh.layers.chalk.get(under) < 0.5);
            s.undo();
            s.undo();
            assert!(s.mesh.layers.chalk.is_empty(), "chalk removed by undo");
        }
    }

    #[test]
    fn the_mask_tool_leaves_the_chalk_and_masked_clay_keeps_it() {
        let mut s = sculpt();
        s.edit_layer(LayerOp::AddChalk);
        stroke(&mut s, Brush::Mask, Vec3::new(0.0, 0.0, 1.0), 2);
        assert!(s.mesh.layers.chalk.values().iter().all(|&c| c == 1.0));
        let chalked = s.mesh.layers.chalk.clone();
        let original = s.mesh.positions.clone();
        // Well inside the masked patch.
        s.begin_stroke();
        for _ in 0..3 {
            s.dab(Dab {
                radius: 0.1,
                ..dab(Brush::Clay, Vec3::new(0.02, 0.0, 1.0).normalize())
            });
        }
        s.end_stroke();
        assert_eq!(s.mesh.positions, original, "masked: nothing moves");
        assert_eq!(s.mesh.layers.chalk, chalked, "masked: no chalk wiped");
    }
}
