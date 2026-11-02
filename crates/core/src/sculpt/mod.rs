// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Sculpting: the clay mesh, the brushes that shape it, and undo.
//!
//! The engine knows nothing about screens or pointers: the app turns pointer
//! samples into world-space dabs (`dab`) or grabs (`grab`, `drag`).

pub mod bvh;
pub mod mesh;

use std::ops::Range;

use glam::Vec3;

pub use bvh::{Bvh, Hit};
pub use mesh::{Mesh, Stamp, Vertex};

/// Subdivision level of the default sphere: 163,842 vertices, 327,680
/// triangles; enough for a head study, light enough for the browser.
pub const DEFAULT_LEVEL: u32 = 7;

/// Vertices per GPU upload page.
pub const PAGE: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brush {
    /// Builds up towards a plane, in flat layers, like pressing on clay.
    Clay,
    /// Relaxes the surface towards the average of its neighbours.
    Smooth,
    /// Grabs the surface under the brush and drags it.
    Move,
}

/// One touch of the brush on the surface, in world units.
#[derive(Clone, Copy, Debug)]
pub struct Dab {
    pub brush: Brush,
    pub centre: Vec3,
    pub radius: f32,
    /// 0..1, already including pressure.
    pub strength: f32,
    /// Clay digs instead of adding.
    pub invert: bool,
    /// Also apply mirrored across the X = 0 plane.
    pub symmetry: bool,
}

/// One undoable change: the vertices a stroke moved, before and after.
struct Edit {
    indices: Vec<u32>,
    before: Vec<Vec3>,
    after: Vec<Vec3>,
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
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

/// Smooth bump: 1 at the centre, 0 at the rim, flat at both ends.
fn falloff(distance: f32, radius: f32) -> f32 {
    let t = (distance / radius).min(1.0);
    let s = 1.0 - t * t;
    s * s
}

fn mirror(v: Vec3) -> Vec3 {
    Vec3::new(-v.x, v.y, v.z)
}

impl Sculpt {
    pub fn new(mesh: Mesh) -> Sculpt {
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
            mesh,
        }
    }

    /// Brings the hierarchy up to date with the sculpted surface. Called once
    /// per frame rather than after every dab: within a frame the boxes are a
    /// few dabs behind, which only matters at the very edge of a stroke.
    pub fn refresh(&mut self) {
        if self.bvh_stale {
            self.bvh.refit(&self.mesh);
            self.bvh_stale = false;
        }
    }

    pub fn raycast(&self, origin: Vec3, dir: Vec3) -> Option<Hit> {
        self.bvh.raycast(&self.mesh, origin, dir)
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
        self.stroke_open = true;
        self.touched.next();
        self.stroke_indices.clear();
        self.stroke_before.clear();
        self.grabbed.clear();
    }

    /// Closes the stroke and makes it one undo step.
    pub fn end_stroke(&mut self) {
        if !std::mem::take(&mut self.stroke_open) || self.stroke_indices.is_empty() {
            return;
        }
        let indices = std::mem::take(&mut self.stroke_indices);
        let before = std::mem::take(&mut self.stroke_before);
        let after = indices
            .iter()
            .map(|&v| self.mesh.positions[v as usize])
            .collect();
        self.undo.push(Edit {
            indices,
            before,
            after,
        });
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(edit) = self.undo.pop() {
            self.apply_positions(&edit.indices, &edit.before);
            self.redo.push(edit);
        }
    }

    pub fn redo(&mut self) {
        if let Some(edit) = self.redo.pop() {
            self.apply_positions(&edit.indices, &edit.after);
            self.undo.push(edit);
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

    /// Applies a clay or smooth dab (and its mirror image).
    pub fn dab(&mut self, dab: Dab) {
        self.moved.clear();
        self.dab_at(dab, dab.centre);
        let mirrored = mirror(dab.centre);
        if dab.symmetry && mirrored.distance(dab.centre) > dab.radius * 0.05 {
            self.dab_at(dab, mirrored);
        }
        self.after_move();
    }

    fn dab_at(&mut self, dab: Dab, centre: Vec3) {
        let mut found = std::mem::take(&mut self.found);
        found.clear();
        self.bvh
            .vertices_in_sphere(&self.mesh, centre, dab.radius, &mut self.stamp, &mut found);
        if !found.is_empty() {
            match dab.brush {
                Brush::Clay => self.clay(&found, centre, dab),
                Brush::Smooth => self.smooth(&found, centre, dab),
                Brush::Move => {}
            }
        }
        self.found = found;
    }

    fn clay(&mut self, found: &[u32], centre: Vec3, dab: Dab) {
        // The area's plane: through the point under the brush, facing the
        // weighted average normal. (Through the average position instead, it
        // would sit below a curved surface and lift the rim more than the centre.)
        let (mut plane_normal, mut weight) = (Vec3::ZERO, 0.0);
        for &v in found {
            let w = falloff(self.mesh.positions[v as usize].distance(centre), dab.radius);
            plane_normal += self.mesh.normals[v as usize] * w;
            weight += w;
        }
        if weight <= 0.0 {
            return;
        }
        let n = plane_normal.normalize_or_zero();
        if n == Vec3::ZERO {
            return;
        }
        let plane_point = centre;
        let sign = if dab.invert { -1.0 } else { 1.0 };
        // How far above the area the clay builds up, at full strength.
        let height = dab.radius * 0.25 * dab.strength;
        for &v in found {
            // Only the side facing the brush: never push through a thin form.
            if self.mesh.normals[v as usize].dot(n) <= 0.0 {
                continue;
            }
            let p = self.mesh.positions[v as usize];
            let above = (p - plane_point).dot(n) * sign;
            if above >= height {
                continue;
            }
            let lift =
                (height - above).min(2.0 * height) * falloff(p.distance(centre), dab.radius) * 0.3;
            self.touch(v);
            self.mesh.positions[v as usize] = p + n * (sign * lift);
            self.moved.push(v);
        }
    }

    fn smooth(&mut self, found: &[u32], centre: Vec3, dab: Dab) {
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
            let w = falloff(p.distance(centre), dab.radius) * dab.strength;
            self.smoothed.push(p + (average - p) * w);
        }
        for (i, &v) in found.iter().enumerate() {
            self.touch(v);
            self.mesh.positions[v as usize] = self.smoothed[i];
            self.moved.push(v);
        }
    }

    /// Move brush, at the start of a stroke: takes hold of the surface
    /// around `centre` (and its mirror image).
    pub fn grab(&mut self, centre: Vec3, radius: f32, symmetry: bool) {
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
                        .push((v, p, falloff(p.distance(c), radius), mirrored));
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
        self.moved.clear();
        for &(v, start, weight, mirrored) in &self.grabbed {
            let o = if mirrored { mirror(offset) } else { offset };
            self.mesh.positions[v as usize] = start + o * weight;
            self.moved.push(v);
        }
        self.after_move();
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
        let nearest = |s: &Sculpt, target: Vec3| {
            (0..s.mesh.positions.len())
                .min_by(|&a, &b| {
                    original[a]
                        .distance(target)
                        .total_cmp(&original[b].distance(target))
                })
                .unwrap()
        };
        let centre = nearest(&s, top);
        let edge = nearest(&s, Vec3::new(0.25, 0.0, 0.97).normalize());
        let far = nearest(&s, Vec3::new(0.0, 0.0, -1.0));
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
    fn symmetric_dabs_stay_symmetric() {
        let mut s = sculpt();
        let mut d = dab(Brush::Clay, Vec3::new(0.5, 0.3, 0.8).normalize());
        d.symmetry = true;
        s.dab(d);
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
                    "vertex {i} lost its mirror twin"
                );
            }
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
    fn move_drags_the_grabbed_area_by_the_offset_at_its_centre() {
        let mut s = sculpt();
        let top = Vec3::Z;
        let centre = (0..s.mesh.positions.len())
            .min_by(|&a, &b| {
                s.mesh.positions[a]
                    .distance(top)
                    .total_cmp(&s.mesh.positions[b].distance(top))
            })
            .unwrap();
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
}
