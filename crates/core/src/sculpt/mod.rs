// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Sculpting: the clay mesh, the brushes that shape it, and undo.
//!
//! The engine knows nothing about screens or pointers: the app turns pointer
//! samples into world-space dabs (`dab`) or grabs (`grab`, `drag`).

pub mod bvh;
pub mod mesh;
pub mod record;
pub mod subdivide;

use std::ops::Range;

use glam::Vec3;

pub use bvh::{Bvh, Hit};
pub use mesh::{Mesh, Stamp, Vertex};
pub use record::{Checkpoint, Entry, Event, Replay};

/// Subdivision level of the default sphere: 163,842 vertices, 327,680
/// triangles; enough for a head study, light enough for the browser.
pub const DEFAULT_LEVEL: u32 = 7;

/// Vertices per GPU upload page.
pub const PAGE: usize = 4096;

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
}

impl Brush {
    /// Every brush, in the order the tool panel lists them.
    pub const ALL: [Brush; 7] = [
        Brush::Move,
        Brush::Clay,
        Brush::Flatten,
        Brush::Smooth,
        Brush::Inflate,
        Brush::Crease,
        Brush::Pinch,
    ];
}

/// How a brush's effect fades from its centre to its rim.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Falloff {
    /// A soft dome: full at the centre, flat at both ends.
    #[default]
    Smooth,
    /// A cone: fades evenly to the rim.
    Linear,
    /// A narrow peak: most of the effect near the centre.
    Sharp,
}

impl Falloff {
    /// Every falloff, in the order the panel lists them.
    pub const ALL: [Falloff; 3] = [Falloff::Smooth, Falloff::Linear, Falloff::Sharp];

    /// The weight at `distance` from the centre of a brush of `radius`:
    /// 1 at the centre, 0 at the rim and beyond.
    pub fn weight(self, distance: f32, radius: f32) -> f32 {
        let t = (distance / radius).min(1.0);
        match self {
            Falloff::Smooth => {
                let s = 1.0 - t * t;
                s * s
            }
            Falloff::Linear => 1.0 - t,
            Falloff::Sharp => {
                let s = 1.0 - t;
                s * s * s
            }
        }
    }
}

/// One touch of the brush on the surface, in world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dab {
    pub brush: Brush,
    pub centre: Vec3,
    pub radius: f32,
    /// 0..1, already including pressure.
    pub strength: f32,
    /// The opposite action: clay digs, flatten fills, inflate deflates,
    /// crease raises a ridge, pinch spreads.
    pub invert: bool,
    /// Also apply mirrored across the X = 0 plane.
    pub symmetry: bool,
    /// Which way the stroke is heading, unit length; zero at its first dab.
    pub direction: Vec3,
}

/// One undoable change: the vertices a stroke moved, before and after.
struct Edit {
    indices: Vec<u32>,
    before: Vec<Vec3>,
    after: Vec<Vec3>,
}

/// One step on the undo or redo stack.
enum Step {
    Move(Edit),
    /// The mesh was replaced (remesh, import): the other mesh, to swap
    /// back. Strokes before a topology change can't be undone, so this is
    /// always alone on its stack.
    Topology(Box<Mesh>),
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
    /// Move brush: grabbed vertices, their start positions, weights, and
    /// whether they belong to the mirrored side.
    grabbed: Vec<(u32, Vec3, f32, bool)>,
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
        let mut sculpt = Sculpt::bare(mesh);
        sculpt
            .checkpoints
            .push(Checkpoint::of(&sculpt.mesh, 0, 0.0, true));
        sculpt
    }

    /// A sculpt with no checkpoints: for replay, or to restore a saved one.
    fn bare(mesh: Mesh) -> Sculpt {
        let n = mesh.positions.len();
        Sculpt {
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
            grabbed: Vec::new(),
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
            mesh,
        }
    }

    /// A saved sculpt: its current mesh (in the saved vertex order, see
    /// [`Mesh::from_ordered`]), its log and checkpoints. Undo starts empty.
    pub fn with_history(mesh: Mesh, log: Vec<Entry>, checkpoints: Vec<Checkpoint>) -> Sculpt {
        // What a replay of the log has at its end: the last falloff logged
        // since the last checkpoint.
        let logged_falloff = log
            .iter()
            .rev()
            .find_map(|e| match e.event {
                Event::Falloff(f) => Some(f),
                Event::Checkpoint(_) => Some(Falloff::Smooth),
                _ => None,
            })
            .unwrap_or_default();
        Sculpt {
            log,
            checkpoints,
            logged_falloff,
            ..Sculpt::bare(mesh)
        }
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
    /// Strokes made before can no longer be undone, but this can: undo
    /// brings the previous mesh back.
    pub fn replace_mesh(&mut self, mesh: Mesh) {
        let old = self.swap_mesh(mesh);
        self.undo.clear();
        self.redo.clear();
        self.undo.push(Step::Topology(Box::new(old)));
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
        self.dirty_pages = vec![true; n.div_ceil(PAGE)];
        self.grabbed.clear();
        self.stroke_indices.clear();
        self.stroke_before.clear();
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
        let checkpoint = Checkpoint::of(&self.mesh, self.log.len(), self.clock, topology);
        self.checkpoints.push(checkpoint);
        self.last_checkpoint = self.clock;
    }

    /// Sets every position, as a checkpoint of the positions holds them.
    fn snap_positions(&mut self, positions: &[Vec3]) {
        if positions.len() != self.mesh.positions.len() {
            return;
        }
        if self.mesh.positions != positions {
            self.mesh.positions.copy_from_slice(positions);
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
            self.bvh.refit(&self.mesh);
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
        self.grabbed.clear();
    }

    /// Closes the stroke and makes it one undo step.
    pub fn end_stroke(&mut self) {
        self.record(Event::End);
        if !std::mem::take(&mut self.stroke_open) || self.stroke_indices.is_empty() {
            return;
        }
        let indices = std::mem::take(&mut self.stroke_indices);
        let before = std::mem::take(&mut self.stroke_before);
        self.push_edit(indices, before);
        if self.recording && self.clock - self.last_checkpoint >= CHECKPOINT_EVERY {
            // The replay recomputes the normals when it snaps: so does this.
            self.mesh.recompute_normals();
            self.dirty_pages.iter_mut().for_each(|d| *d = true);
            self.checkpoint(false);
        }
    }

    /// Makes the move of `indices` from `before` to where they are now one
    /// undo step.
    fn push_edit(&mut self, indices: Vec<u32>, before: Vec<Vec3>) {
        let after = indices
            .iter()
            .map(|&v| self.mesh.positions[v as usize])
            .collect();
        self.undo.push(Step::Move(Edit {
            indices,
            before,
            after,
        }));
        self.redo.clear();
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
                self.apply_positions(&edit.indices, &edit.before);
                self.redo.push(Step::Move(edit));
            }
            Some(Step::Topology(mesh)) => {
                // Logged as a checkpoint of the mesh it brings back.
                let current = self.swap_mesh(*mesh);
                self.undo.clear();
                self.redo.clear();
                self.redo.push(Step::Topology(Box::new(current)));
                self.checkpoint(true);
            }
            None => {}
        }
    }

    pub fn redo(&mut self) {
        match self.redo.pop() {
            Some(Step::Move(edit)) => {
                self.record(Event::Redo);
                self.apply_positions(&edit.indices, &edit.after);
                self.undo.push(Step::Move(edit));
            }
            Some(Step::Topology(mesh)) => {
                let current = self.swap_mesh(*mesh);
                self.undo.clear();
                self.redo.clear();
                self.undo.push(Step::Topology(Box::new(current)));
                self.checkpoint(true);
            }
            None => {}
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

    /// Remembers where a vertex was before this stroke first moved it.
    fn touch(&mut self, v: u32) {
        if self.stroke_open && self.touched.mark(v) {
            self.stroke_indices.push(v);
            self.stroke_before.push(self.mesh.positions[v as usize]);
        }
    }

    /// Moves vertex `v` to `p`, remembering where it was for undo.
    fn place(&mut self, v: u32, p: Vec3) {
        self.touch(v);
        self.mesh.positions[v as usize] = p;
        self.moved.push(v);
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
            match dab.brush {
                Brush::Clay => self.clay(&found, dab),
                Brush::Flatten => self.flatten(&found, dab),
                Brush::Smooth => self.smooth(&found, dab),
                Brush::Inflate => self.inflate(&found, dab),
                Brush::Crease => self.crease(&found, dab),
                Brush::Pinch => self.pinch(&found, dab),
                Brush::Move => {}
            }
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
        for &v in found {
            if !self.faces(v, n) {
                continue;
            }
            let p = self.mesh.positions[v as usize];
            let above = (p - plane_point).dot(n) * sign;
            if above >= height {
                continue;
            }
            let lift = (height - above).min(2.0 * height)
                * self.weight(p.distance(dab.centre), dab.radius)
                * 0.3;
            self.place(v, p + n * (sign * lift));
        }
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
                if self.unique.mark(v) {
                    let p = self.mesh.positions[v as usize];
                    self.grabbed
                        .push((v, p, self.falloff.weight(p.distance(c), radius), mirrored));
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
                let target = mirror(q);
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
        self.apply_positions(&indices, &targets);
        self.push_edit(indices, before);
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
        for brush in Brush::ALL.into_iter().filter(|&b| b != Brush::Move) {
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
}
