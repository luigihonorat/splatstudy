// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The calipers over the views (see [`crate::calipers`]): placing one with
//! a drag between two points of the surface, in either view, moving its
//! ends, drawing them in both views, and their group in the sculpt panel.
//!
//! Both views look into the clay's space, where the reference is placed,
//! so a caliper is drawn in each through that view's own camera: with the
//! cameras linked it lies at the same place over the reference and over
//! the sculpt. A point is picked on the clay in the sculpt view, and on
//! the reference in the reference view: on a mesh by a ray through a tree
//! of its triangles, on splats where the splats along the ray have
//! covered half the view (see [`splat_depth`]). Off the surface, a
//! dragged end slides square to the view at the depth it had.
//!
//! An end's handle and a placing drag take the left press before the clay
//! does, so neither sculpts nor turns the view; with Alt, Ctrl or Cmd held
//! the press navigates as usual.

use egui::{Align2, CursorIcon, Pos2, Rect, Sense, Stroke, vec2};
use glam::{Mat4, Vec2, Vec3};

use super::panels::Group;
use super::toolbar::RefKind;
use super::views::shown;
use super::{App, REFERENCE, SCULPT};
use crate::calipers::{Calipers, length_text, ratio_text};
use crate::camera::OrbitCamera;
use crate::input::Modifiers;
use crate::reference::{Aabb, Content, Reference, Splats, clip};
use crate::sculpt::{Bvh, Layers, Mesh, mesh::Adjacency};
use crate::ui::{self, fonts, guides};

/// How near an end a press must land to take it, in points.
const END_REACH: f32 = 7.0;
/// A placing drag shorter than this, in points, is a click: no caliper.
const MIN_DRAG: f32 = 4.0;
/// Points along a caliper tested for the surface in front of it.
const OCCLUSION_SAMPLES: usize = 16;

/// The calipers and what is being done with them.
#[derive(Default)]
pub(super) struct CaliperState {
    pub(super) set: Calipers,
    /// "add caliper" was chosen: the next drag in a view places one.
    pub(super) placing: bool,
    drag: Option<EndDrag>,
    /// The pointer is over an end it can take, this frame.
    hover: Option<(usize, usize)>,
    /// The caliper pointed at in the panel, drawn brighter.
    highlight: Option<usize>,
    /// A mesh reference's triangles in a tree, to pick on.
    surface: Option<Surface>,
}

/// An end being dragged.
#[derive(Clone, Copy, Debug)]
struct EndDrag {
    view: usize,
    caliper: usize,
    end: usize,
    /// Where the end was: Escape puts it back.
    from: Vec3,
    /// A caliper being placed: let go where it started, it is dropped.
    new: bool,
}

/// A mesh reference as it is picked on, in the file's coordinates, so
/// turning or fitting the reference doesn't build it again; a crop does.
struct Surface {
    key: SurfaceKey,
    mesh: Mesh,
    bvh: Bvh,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SurfaceKey {
    /// The file, by the address of its bytes.
    file: usize,
    crop: Option<Aabb>,
}

impl SurfaceKey {
    fn of(r: &Reference) -> Self {
        Self {
            file: std::sync::Arc::as_ptr(&r.source.bytes).cast::<u8>() as usize,
            crop: r.crop,
        }
    }
}

impl Surface {
    /// The reference's mesh, cut by its crop box, in a tree; `None` for
    /// splats and pictures.
    fn build(r: &Reference) -> Option<Self> {
        let Content::Mesh(m) = &r.content else {
            return None;
        };
        let cut = match r.crop {
            Some(crop) => clip::crop(m, crop),
            None => clip::Cropped::whole(m),
        };
        // Only picked on: no normals or neighbours needed.
        let mesh = Mesh {
            positions: cut.positions,
            normals: Vec::new(),
            triangles: cut.triangles,
            vertex_faces: Adjacency::default(),
            neighbours: Adjacency::default(),
            quads: Vec::new(),
            layers: Layers::default(),
        };
        let bvh = Bvh::build(&mesh);
        Some(Self {
            key: SurfaceKey::of(r),
            mesh,
            bvh,
        })
    }
}

/// How far along the ray `origin + t · dir` (`dir` of unit length) the
/// splats in front have covered half the view behind them: where a
/// splat reference's surface is under a point, as far as the eye can
/// tell. `None` when they never cover that much. Each splat counts with
/// its opacity and its Gaussian falloff at the ray's nearest point;
/// splats outside `crop` don't count. Brute force, once per pick.
pub(super) fn splat_depth(
    splats: &Splats,
    crop: Option<Aabb>,
    origin: Vec3,
    dir: Vec3,
) -> Option<f32> {
    let mut along: Vec<(f32, f32)> = Vec::new();
    let items = splats
        .positions
        .iter()
        .zip(&splats.scales)
        .zip(&splats.rotations)
        .zip(&splats.colors);
    for (((&p, &scale), &rotation), colour) in items {
        let opacity = f32::from(colour[3]) / 255.0;
        if opacity < 0.02 || crop.is_some_and(|c| !c.contains(p)) {
            continue;
        }
        let t = (p - origin).dot(dir);
        let off = origin + dir * t - p;
        let reach = 3.0 * scale.max_element();
        if t <= 0.0 || off.length_squared() > reach * reach {
            continue;
        }
        let local = rotation.inverse() * off / scale.max(Vec3::splat(1e-9));
        let m2 = local.length_squared();
        if m2 < 9.0 {
            along.push((t, opacity * (-0.5 * m2).exp()));
        }
    }
    along.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut clear = 1.0;
    for (t, alpha) in along {
        clear *= 1.0 - alpha.min(0.99);
        if clear < 0.5 {
            return Some(t);
        }
    }
    None
}

/// The clay's space to clip space, for a camera seeing a view of `rect`.
fn to_clip(camera: &OrbitCamera, rect: Rect) -> Mat4 {
    camera.projection(rect.aspect_ratio()) * camera.view()
}

/// `p` on the screen in a view of `rect` seen through `to_clip`, flipped
/// left to right in the mirror view; `None` behind the eye.
pub(super) fn on_screen(to_clip: Mat4, rect: Rect, mirror: bool, p: Vec3) -> Option<Pos2> {
    let clip = to_clip * p.extend(1.0);
    (clip.w > 1e-6).then(|| {
        let ndc = clip.truncate() / clip.w;
        let x = if mirror { -ndc.x } else { ndc.x };
        Pos2::new(
            rect.left() + (x + 1.0) / 2.0 * rect.width(),
            rect.top() + (1.0 - ndc.y) / 2.0 * rect.height(),
        )
    })
}

/// The ray through `pos`, a point of the rendered image of a view of
/// `rect`: its origin and unit direction.
fn ray(camera: &OrbitCamera, rect: Rect, pos: Pos2) -> (Vec3, Vec3) {
    let ndc = Vec2::new(
        (pos.x - rect.left()) / rect.width() * 2.0 - 1.0,
        1.0 - (pos.y - rect.top()) / rect.height() * 2.0,
    );
    camera.ray(ndc, rect.aspect_ratio())
}

/// Where the ray meets the plane through `at` square to the camera, or
/// `at` itself when it runs along the plane.
fn on_plane(camera: &OrbitCamera, (origin, dir): (Vec3, Vec3), at: Vec3) -> Vec3 {
    let normal = (camera.target - camera.eye()).normalize_or_zero();
    let facing = dir.dot(normal);
    if facing.abs() < 1e-6 {
        return at;
    }
    origin + dir * ((at - origin).dot(normal) / facing)
}

impl App {
    /// Whether view `index` draws the calipers: it shows, they are on in
    /// it, and it shows something 3D (the reference view, a 3D reference).
    pub(super) fn calipers_drawn(&self, index: usize) -> bool {
        self.calipers.set.shown[index] && shown(self.view_rects[index]) && self.calipers_fit(index)
    }

    /// Whether view `index` can hold calipers: the sculpt view always,
    /// the reference view with a 3D reference.
    pub(super) fn calipers_fit(&self, index: usize) -> bool {
        index == SCULPT || matches!(self.reference_kind(), RefKind::Splats | RefKind::Mesh)
    }

    /// Whether the calipers have the pointer: placing one, dragging an
    /// end or over one. The brush outline stays away.
    pub(super) fn caliper_busy(&self) -> bool {
        let c = &self.calipers;
        c.placing || c.drag.is_some() || c.hover.is_some()
    }

    /// "add caliper": the next drag in a view places one, and both views
    /// show them. Chosen again, it is put away.
    pub(super) fn add_caliper(&mut self) {
        let c = &mut self.calipers;
        c.placing = !c.placing;
        if c.placing {
            c.set.shown = [true; 2];
        }
        self.dirty = true;
    }

    pub(super) fn delete_calipers(&mut self) {
        let c = &mut self.calipers;
        c.set.clear();
        c.drag = None;
        c.hover = None;
        self.dirty = true;
    }

    /// Calipers come and go with the scene; what was being done with them
    /// stops.
    pub(super) fn set_calipers(&mut self, set: Calipers) {
        self.calipers = CaliperState {
            set,
            surface: self.calipers.surface.take(),
            ..Default::default()
        };
    }

    /// The point of the surface under `pos`, a point of view `index`'s
    /// rendered image in `rect`: the clay in the sculpt view (not while
    /// the timelapse plays), the reference in the reference view.
    fn surface_point(&mut self, rect: Rect, index: usize, pos: Pos2) -> Option<Vec3> {
        if index == SCULPT {
            return self
                .playback
                .is_none()
                .then(|| self.pick(rect, pos))
                .flatten()
                .map(|h| h.point);
        }
        let r = self.reference.current.as_ref()?;
        let (origin, dir) = ray(&self.cameras[REFERENCE], rect, pos);
        let to_world = r.align.matrix();
        let to_file = to_world.inverse();
        let (o, d) = (
            to_file.transform_point3(origin),
            to_file.transform_vector3(dir).normalize_or_zero(),
        );
        let t = match &r.content {
            Content::Mesh(_) => {
                let surface = &mut self.calipers.surface;
                if surface.as_ref().is_none_or(|s| s.key != SurfaceKey::of(r)) {
                    *surface = Surface::build(r);
                }
                let s = surface.as_ref()?;
                s.bvh.raycast(&s.mesh, o, d)?.distance
            }
            Content::Splats(splats) => splat_depth(splats, r.crop, o, d)?,
            Content::Image(_) => return None,
        };
        Some(to_world.transform_point3(o + d * t))
    }

    /// Whether the surface hides `p` in view `index`: clay in front of it
    /// in the sculpt view, a mesh reference's surface in the reference
    /// view once its tree is built. Splats are never in front: their
    /// calipers are drawn over them.
    fn hidden_behind(&self, index: usize, rect: Rect, p: Vec3) -> bool {
        let camera = &self.cameras[index];
        let Some(screen) = on_screen(to_clip(camera, rect), rect, false, p) else {
            return false;
        };
        let (origin, dir) = ray(camera, rect, screen);
        let depth = (p - origin).dot(dir);
        // A point on the surface is not behind it.
        let slack = 2.0 * camera.world_per_point(p, rect.height());
        if index == SCULPT {
            if self.playback.is_some() {
                return false;
            }
            return self
                .sculpt
                .raycast(origin, dir)
                .is_some_and(|h| h.distance < depth - slack);
        }
        let (Some(r), Some(s)) = (&self.reference.current, &self.calipers.surface) else {
            return false;
        };
        if s.key != SurfaceKey::of(r) {
            return false;
        }
        let to_world = r.align.matrix();
        let to_file = to_world.inverse();
        let (o, d) = (
            to_file.transform_point3(origin),
            to_file.transform_vector3(dir).normalize_or_zero(),
        );
        s.bvh.raycast(&s.mesh, o, d).is_some_and(|h| {
            to_world
                .transform_point3(o + d * h.distance)
                .distance(origin)
                < depth - slack
        })
    }

    /// The visible end of a shown caliper under `screen` in view `index`:
    /// the caliper and which end.
    fn end_at(&self, rect: Rect, index: usize, screen: Pos2) -> Option<(usize, usize)> {
        if !self.calipers_drawn(index) {
            return None;
        }
        let to_clip = to_clip(&self.cameras[index], rect);
        let mirror = self.study.views[index].mirror;
        let mut best: Option<((usize, usize), f32)> = None;
        for (k, c) in self.calipers.set.list.iter().enumerate() {
            if !c.visible {
                continue;
            }
            for (end, &p) in c.ends.iter().enumerate() {
                let Some(at) = on_screen(to_clip, rect, mirror, p) else {
                    continue;
                };
                let d = at.distance(screen);
                if d <= END_REACH && best.is_none_or(|(_, b)| d < b) {
                    best = Some(((k, end), d));
                }
            }
        }
        best.map(|(found, _)| found)
    }

    /// A left press at `screen` in view `index`, before the view sees it:
    /// true if the calipers took it. Placing, any plain press is theirs,
    /// on the surface or not; otherwise only one on an end.
    pub(super) fn caliper_press(
        &mut self,
        rect: Rect,
        index: usize,
        screen: Pos2,
        m: Modifiers,
    ) -> bool {
        if m.alt || m.ctrl || m.command || !self.calipers_fit(index) {
            return false;
        }
        if !self.calipers.placing {
            let Some((caliper, end)) = self.end_at(rect, index, screen) else {
                return false;
            };
            self.calipers.drag = Some(EndDrag {
                view: index,
                caliper,
                end,
                from: self.calipers.set.list[caliper].ends[end],
                new: false,
            });
            return true;
        }
        let pos = self.view_pos_in(index, rect, screen);
        if let Some(point) = self.surface_point(rect, index, pos) {
            let c = &mut self.calipers;
            let caliper = c.set.add([point; 2]);
            c.placing = false;
            c.drag = Some(EndDrag {
                view: index,
                caliper,
                end: 1,
                from: point,
                new: true,
            });
        }
        true
    }

    /// Moves the dragged end with the pointer, onto the surface under it
    /// or, off the surface, square to the view at its depth. Let go, a
    /// new caliper too short to see is dropped and placing goes on.
    fn caliper_follow(&mut self, ui: &egui::Ui, rects: [Rect; 2]) {
        let Some(drag) = self.calipers.drag else {
            return;
        };
        let (pointer, held) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.primary_down()));
        let rect = rects[drag.view];
        if drag.caliper >= self.calipers.set.list.len() || !shown(rect) {
            self.calipers.drag = None;
            return;
        }
        if held {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            if let Some(screen) = pointer {
                let pos = self.view_pos_in(drag.view, rect, screen);
                let at = self.calipers.set.list[drag.caliper].ends[drag.end];
                let camera = self.cameras[drag.view];
                let point = self
                    .surface_point(rect, drag.view, pos)
                    .unwrap_or_else(|| on_plane(&camera, ray(&camera, rect, pos), at));
                self.calipers.set.list[drag.caliper].ends[drag.end] = point;
            }
            return;
        }
        self.calipers.drag = None;
        if drag.new {
            let to_clip = to_clip(&self.cameras[drag.view], rect);
            let ends = self.calipers.set.list[drag.caliper].ends;
            let [a, b] = ends.map(|p| on_screen(to_clip, rect, false, p));
            if a.zip(b).is_none_or(|(a, b)| a.distance(b) < MIN_DRAG) {
                self.calipers.set.remove(drag.caliper);
                self.calipers.placing = true;
            }
        }
        self.dirty = true;
    }

    /// Escape puts a dragged end back (drops a caliper being placed), or
    /// stops placing; Shift+E starts placing. Before the study aids'
    /// keys: E alone is the measure.
    pub(super) fn caliper_keys(&mut self, ui: &egui::Ui) {
        use egui::{Key, Modifiers as M};
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        if ui.input_mut(|i| i.consume_key(M::SHIFT, Key::E)) {
            self.add_caliper();
        }
        let c = &mut self.calipers;
        if (c.placing || c.drag.is_some()) && ui.input_mut(|i| i.consume_key(M::NONE, Key::Escape))
        {
            match c.drag.take() {
                Some(d) if d.new => {
                    c.set.remove(d.caliper);
                }
                Some(d) => {
                    if let Some(caliper) = c.set.list.get_mut(d.caliper) {
                        caliper.ends[d.end] = d.from;
                    }
                }
                None => c.placing = false,
            }
            self.dirty = true;
        }
    }

    /// The calipers over the shown views, after the guides: the dragged
    /// end follows the pointer, then each caliper is drawn in each view
    /// that shows them, with its number and length, dimmer where the
    /// surface is in front of it. While placing, each view says what to do.
    pub(super) fn calipers_ui(&mut self, ui: &egui::Ui, rects: [Rect; 2]) {
        self.caliper_follow(ui, rects);
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let free = self.calipers.drag.is_none()
            && self.drags.iter().all(Option::is_none)
            && !self.pie_open()
            && self.flyout.is_none();
        let toolbars = self.toolbar_areas(rects);
        self.calipers.hover = None;
        let tr = self.lang.tr().calipers;
        for index in [SCULPT, REFERENCE] {
            let rect = rects[index];
            if !shown(rect) || !self.calipers_fit(index) {
                continue;
            }
            let over = pointer.filter(|&p| rect.contains(p) && !toolbars[index].contains(p));
            if self.calipers.placing {
                let inks = self.view_inks(index);
                ui.painter().with_clip_rect(rect).text(
                    egui::pos2(rect.center().x, rect.bottom() - 58.0),
                    Align2::CENTER_BOTTOM,
                    tr.place_hint,
                    fonts::regular(13.0),
                    inks.text,
                );
                if free && over.is_some() {
                    ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                }
            } else if free && let Some(p) = over {
                self.calipers.hover = self.end_at(rect, index, p);
                if self.calipers.hover.is_some() {
                    ui.ctx().set_cursor_icon(CursorIcon::Grab);
                }
            }
        }
        // The end pointed at shows taken in both views.
        for index in [SCULPT, REFERENCE] {
            let rect = rects[index];
            if self.calipers_drawn(index) {
                // The reference's tree, for what hides the calipers, once
                // nothing is held: not built afresh at every step of a crop.
                let held = ui.input(|i| i.pointer.primary_down());
                if index == REFERENCE && !held && self.reference_kind() == RefKind::Mesh {
                    self.ensure_surface();
                }
                self.draw_calipers(ui, index, rect);
            }
        }
        self.calipers.highlight = None;
    }

    /// Builds the mesh reference's tree if it isn't there for the
    /// reference as it is now.
    fn ensure_surface(&mut self) {
        let Some(r) = &self.reference.current else {
            return;
        };
        let surface = &mut self.calipers.surface;
        if surface.as_ref().is_none_or(|s| s.key != SurfaceKey::of(r)) {
            *surface = Surface::build(r);
        }
    }

    fn draw_calipers(&self, ui: &egui::Ui, index: usize, rect: Rect) {
        let camera = &self.cameras[index];
        let to_clip = to_clip(camera, rect);
        let mirror = self.study.views[index].mirror;
        let inks = self.view_inks(index);
        let painter = ui.painter().with_clip_rect(rect);
        let c = &self.calipers;
        for (k, caliper) in c.set.list.iter().enumerate() {
            if !caliper.visible {
                continue;
            }
            let [Some(a), Some(b)] = caliper.ends.map(|p| on_screen(to_clip, rect, mirror, p))
            else {
                continue;
            };
            let active = c.highlight == Some(k)
                || c.drag.is_some_and(|d| d.caliper == k)
                || c.hover.is_some_and(|(h, _)| h == k);
            let bright = inks.ink.gamma_multiply(if active { 1.0 } else { 0.85 });
            let dim = inks.ink.gamma_multiply(0.3);
            // A soft edge of the view's own background under each mark, so
            // it reads over the clay and the cast as over the empty view.
            let halo = Stroke::new(3.0, self.view_background(index).gamma_multiply(0.55));
            // Along the line, bright where it is in sight, dim where the
            // surface is in front.
            let samples: Vec<(Pos2, bool)> = (0..=OCCLUSION_SAMPLES)
                .filter_map(|s| {
                    let t = s as f32 / OCCLUSION_SAMPLES as f32;
                    let p = caliper.ends[0].lerp(caliper.ends[1], t);
                    let at = on_screen(to_clip, rect, mirror, p)?;
                    Some((at, !self.hidden_behind(index, rect, p)))
                })
                .collect();
            let tick = guides::across([a, b]) * guides::TICK;
            for pair in samples.windows(2) {
                painter.line_segment([pair[0].0, pair[1].0], halo);
            }
            for at in [a, b] {
                painter.line_segment([at - tick, at + tick], halo);
            }
            for pair in samples.windows(2) {
                let [(from, a_seen), (to, b_seen)] = [pair[0], pair[1]];
                let colour = if a_seen && b_seen { bright } else { dim };
                painter.line_segment([from, to], Stroke::new(1.0, colour));
            }
            let seen = |end: usize| {
                let sample = if end == 0 {
                    samples.first()
                } else {
                    samples.last()
                };
                sample.is_some_and(|&(_, seen)| seen)
            };
            // The tips: a short tick across each end, as the measure's,
            // and the point itself.
            for (end, at) in [a, b].into_iter().enumerate() {
                let colour = if seen(end) { bright } else { dim };
                painter.line_segment([at - tick, at + tick], Stroke::new(1.0, colour));
                painter.circle_filled(at, 1.8, colour);
                let taken = c.hover == Some((k, end))
                    || c.drag
                        .is_some_and(|d| d.caliper == k && d.end == end && d.view == index);
                if taken {
                    painter.circle_stroke(at, END_REACH - 1.0, Stroke::new(1.0, inks.ink));
                }
            }
            if a.distance(b) >= MIN_DRAG {
                // Beside the middle, on the side away from the view's
                // centre, as the measure's.
                let at = guides::label_at([a, b], rect.center(), 16.0);
                let text = format!("{} · {}", caliper.number, length_text(caliper.length()));
                let galley = painter.layout_no_wrap(text, fonts::light(14.0), bright);
                let corner = at - galley.size() / 2.0;
                for d in [
                    vec2(-1.0, 0.0),
                    vec2(1.0, 0.0),
                    vec2(0.0, -1.0),
                    vec2(0.0, 1.0),
                ] {
                    painter.galley_with_override_text_color(corner + d, galley.clone(), halo.color);
                }
                painter.galley(corner, galley, bright);
            }
        }
    }

    /// The calipers' group in the sculpt panel, when the window menu
    /// leaves it in.
    pub(super) fn calipers_group(&mut self, ui: &mut egui::Ui) {
        if !self.panels.holds(Group::Calipers) {
            return;
        }
        let title = self.lang.tr().calipers.calipers;
        ui::group(ui, "calipers", title, |ui| self.calipers_panel(ui));
    }

    /// Each caliper on a row: its number, its length and, after the
    /// first, its length as a share of the first's; a button that shows
    /// or hides it and one that deletes it. Then adding one and deleting
    /// them all.
    fn calipers_panel(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr().calipers;
        let visuals = ui.visuals().clone();
        let text = visuals.widgets.active.fg_stroke.color;
        let muted = visuals.widgets.inactive.fg_stroke.color;
        if self.calipers.set.list.is_empty() {
            ui.label(egui::RichText::new(tr.empty).color(muted));
        }
        let mut remove = None;
        let mut flip = None;
        // Within the panel as it shows, though a long line elsewhere in it
        // may have widened its content.
        let width = ui
            .available_width()
            .min(ui.clip_rect().right() - ui.cursor().left() - 4.0);
        for (k, c) in self.calipers.set.list.iter().enumerate() {
            let (row, response) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
            if response.hovered() {
                self.calipers.highlight = Some(k);
            }
            let colour = if c.visible { text } else { muted };
            let painter = ui.painter();
            let y = row.center().y;
            let font = fonts::regular(13.0);
            painter.text(
                egui::pos2(row.left(), y),
                Align2::LEFT_CENTER,
                c.number.to_string(),
                font.clone(),
                muted,
            );
            painter.text(
                egui::pos2(row.left() + 22.0, y),
                Align2::LEFT_CENTER,
                length_text(c.length()),
                font.clone(),
                colour,
            );
            if let Some(ratio) = self.calipers.set.ratio(k) {
                let shown = painter.text(
                    egui::pos2(row.left() + 76.0, y),
                    Align2::LEFT_CENTER,
                    ratio_text(ratio),
                    font.clone(),
                    muted,
                );
                ui.interact(shown, ui.scope_id().with(("ratio", k)), Sense::hover())
                    .on_hover_text(tr.ratio);
            }
            let button = |right: f32| {
                Rect::from_center_size(egui::pos2(row.right() - right, y), vec2(18.0, 18.0))
            };
            let eye = ui
                .interact(
                    button(30.0),
                    ui.scope_id().with(("show", k)),
                    Sense::click(),
                )
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(tr.show_hide);
            let bin = ui
                .interact(
                    button(8.0),
                    ui.scope_id().with(("delete", k)),
                    Sense::click(),
                )
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(tr.delete);
            let ink = |r: &egui::Response| if r.hovered() { text } else { muted };
            // Shown: an open eye; hidden: the eye shut.
            let centre = eye.rect.center();
            let line = Stroke::new(1.0, ink(&eye));
            let lid: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let x = -6.0 + i as f32;
                    egui::pos2(centre.x + x, centre.y - 3.5 * (1.0 - (x / 6.0).powi(2)))
                })
                .collect();
            painter.add(egui::Shape::line(lid.clone(), line));
            if c.visible {
                let under: Vec<Pos2> = lid
                    .iter()
                    .map(|p| egui::pos2(p.x, 2.0 * centre.y - p.y))
                    .collect();
                painter.add(egui::Shape::line(under, line));
                painter.circle_filled(centre, 1.8, ink(&eye));
            }
            // Delete: a cross.
            let centre = bin.rect.center();
            let line = Stroke::new(1.0, ink(&bin));
            for d in [vec2(4.0, 4.0), vec2(4.0, -4.0)] {
                painter.line_segment([centre - d, centre + d], line);
            }
            if eye.clicked() {
                flip = Some(k);
            }
            if bin.clicked() {
                remove = Some(k);
            }
        }
        if let Some(k) = flip {
            let c = &mut self.calipers.set.list[k];
            c.visible = !c.visible;
            self.dirty = true;
        }
        if let Some(k) = remove {
            self.calipers.set.remove(k);
            self.calipers.drag = None;
            self.dirty = true;
        }
        ui.add_space(4.0);
        // One under the other: side by side they don't fit in Japanese.
        ui.vertical(|ui| {
            let add = ui::choice(ui, tr.add, self.calipers.placing).on_hover_text("Shift+E");
            if add.clicked() {
                self.add_caliper();
            }
            let any = !self.calipers.set.list.is_empty();
            if ui
                .add_enabled_ui(any, |ui| ui::choice(ui, tr.delete_all, false))
                .inner
                .clicked()
            {
                self.delete_calipers();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::study::tests::Harness;
    use super::super::toolbar::ViewOption;
    use super::*;
    use glam::Quat;

    #[test]
    fn splats_are_picked_where_they_cover_half_the_view() {
        // A wall of three thin, faint layers along the ray, and a solid one
        // behind them.
        let mut splats = Splats::default();
        for (z, alpha) in [(0.5, 40), (0.3, 40), (0.1, 40), (-0.2, 255)] {
            splats.positions.push(Vec3::new(0.0, 0.0, z));
            splats.scales.push(Vec3::new(0.1, 0.1, 0.01));
            splats.rotations.push(Quat::IDENTITY);
            splats.colors.push([200, 200, 200, alpha]);
        }
        let eye = Vec3::new(0.0, 0.0, 2.0);
        let t = splat_depth(&splats, None, eye, Vec3::NEG_Z).expect("covered");
        assert!((eye.z - t - (-0.2)).abs() < 1e-4, "the solid one: {t}");
        // Faint ones alone never cover half.
        splats.colors[3][3] = 0;
        assert_eq!(splat_depth(&splats, None, eye, Vec3::NEG_Z), None);
        splats.colors[3][3] = 255;
        // Off to the side the ray misses them all.
        assert_eq!(
            splat_depth(&splats, None, Vec3::new(1.0, 0.0, 2.0), Vec3::NEG_Z),
            None
        );
        // The crop leaves out the solid one.
        let crop = Aabb {
            min: Vec3::new(-1.0, -1.0, 0.0),
            max: Vec3::ONE,
        };
        assert_eq!(splat_depth(&splats, Some(crop), eye, Vec3::NEG_Z), None);
        // Behind the eye nothing counts.
        assert_eq!(
            splat_depth(&splats, None, Vec3::new(0.0, 0.0, -1.0), Vec3::NEG_Z),
            None
        );
    }

    #[test]
    fn a_point_lands_where_the_view_shows_it_mirrored_or_not() {
        let camera = OrbitCamera {
            yaw: 0.4,
            pitch: 0.2,
            ..Default::default()
        };
        let rect = Rect::from_min_size(egui::pos2(200.0, 30.0), vec2(500.0, 600.0));
        let p = Vec3::new(0.2, 0.3, -0.1);
        let m = to_clip(&camera, rect);
        let at = on_screen(m, rect, false, p).unwrap();
        // The ray back through it passes through the point.
        let (o, d) = ray(&camera, rect, at);
        assert!((o + d * (p - o).dot(d)).distance(p) < 1e-4);
        // The plane square to the view through the point holds it too.
        assert!(on_plane(&camera, (o, d), p).distance(p) < 1e-4);
        // Mirrored, it is flipped about the view's middle.
        let flipped = on_screen(m, rect, true, p).unwrap();
        assert!((flipped.x - (rect.left() + rect.right() - at.x)).abs() < 1e-3);
        assert_eq!(flipped.y, at.y);
        // Behind the eye, nowhere.
        let behind = camera.eye() + (camera.eye() - camera.target);
        assert!(on_screen(m, rect, false, behind).is_none());
    }

    /// An octahedron as an OBJ, as big as the clay.
    fn octahedron() -> Vec<u8> {
        let mut obj = String::new();
        for v in ["1 0 0", "-1 0 0", "0 1 0", "0 -1 0", "0 0 1", "0 0 -1"] {
            obj += &format!("v {v}\n");
        }
        for f in [
            "1 3 5", "3 2 5", "2 4 5", "4 1 5", "3 1 6", "2 3 6", "4 2 6", "1 4 6",
        ] {
            obj += &format!("f {f}\n");
        }
        obj.into_bytes()
    }

    #[test]
    fn a_caliper_placed_in_either_view_shows_at_the_same_place_in_both() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        h.app.load_reference("octahedron.obj", octahedron().into());
        h.frames(2);
        assert!(h.app.study.link);
        let [sculpt, reference] = h.app.view_rects;
        let strokes = h.app.sculpt.log().len();
        let yaw = h.app.cameras[SCULPT].yaw;

        // Placed on the clay: no stroke, no turn.
        h.app.add_caliper();
        let c = sculpt.center();
        h.drag(c - vec2(40.0, 0.0), c + vec2(30.0, 20.0));
        assert_eq!(h.app.sculpt.log().len(), strokes, "no stroke");
        assert_eq!(h.app.cameras[SCULPT].yaw, yaw, "no turn");
        assert!(!h.app.calipers.placing);
        let first = h.app.calipers.set.list[0];
        assert!(first.length() > 0.05, "{first:?}");
        let radius = h.app.sculpt.mesh.positions[0].length();
        for end in first.ends {
            assert!((end.length() - radius).abs() < 0.05 * radius, "on the clay");
        }

        // Placed on the reference: on its surface, |x|+|y|+|z| = 1 times
        // its placement.
        h.app.add_caliper();
        let c = reference.center();
        h.drag(c + vec2(-20.0, -50.0), c + vec2(10.0, 40.0));
        assert_eq!(h.app.calipers.set.list.len(), 2);
        assert_eq!(h.app.cameras[REFERENCE].yaw, yaw, "no turn");
        let second = h.app.calipers.set.list[1];
        let r = h.app.reference.current.as_ref().unwrap();
        let to_file = r.align.matrix().inverse();
        for end in second.ends {
            let p = to_file.transform_point3(end);
            assert!((p.abs().element_sum() - 1.0).abs() < 1e-3, "on it: {p}");
        }

        // With the cameras linked, both views show each end at the same
        // place in their own rectangle, the mirror view flipped.
        h.app.study.views[REFERENCE].mirror = true;
        h.frames(1);
        let [sculpt, reference] = h.app.view_rects;
        for end in first.ends.into_iter().chain(second.ends) {
            let s = on_screen(to_clip(&h.app.cameras[SCULPT], sculpt), sculpt, false, end).unwrap();
            let r = on_screen(
                to_clip(&h.app.cameras[REFERENCE], reference),
                reference,
                true,
                end,
            )
            .unwrap();
            let s = (s - sculpt.left_top()) / sculpt.size();
            let r = (r - reference.left_top()) / reference.size();
            assert!((s.x - (1.0 - r.x)).abs() < 1e-3 && (s.y - r.y).abs() < 1e-3);
        }

        // An end dragged in the reference view moves; Escape mid-drag puts
        // it back.
        let to = to_clip(&h.app.cameras[REFERENCE], reference);
        let tip = on_screen(to, reference, true, second.ends[0]).unwrap();
        h.drag(tip, tip + vec2(0.0, 30.0));
        let moved = h.app.calipers.set.list[1].ends[0];
        assert!(moved.distance(second.ends[0]) > 0.01);
        assert_eq!(h.app.cameras[REFERENCE].yaw, yaw, "no turn");
        let tip = on_screen(to, reference, true, moved).unwrap();
        use crate::PointerPhase::{Down, Move, Up};
        h.pointer(Move, tip);
        h.pointer(Down, tip);
        h.pointer(Move, tip + vec2(25.0, 0.0));
        h.key(egui::Key::Escape, false);
        h.pointer(Up, tip + vec2(25.0, 0.0));
        assert_eq!(h.app.calipers.set.list[1].ends[0], moved);

        // A click while placing is no caliper; Escape stops placing.
        h.key(egui::Key::E, true);
        assert!(h.app.calipers.placing, "Shift+E");
        assert!(!h.app.study.measure.on, "not the measure");
        h.drag(sculpt.center(), sculpt.center());
        assert_eq!(h.app.calipers.set.list.len(), 2);
        assert!(h.app.calipers.placing);
        h.key(egui::Key::Escape, false);
        assert!(!h.app.calipers.placing);
        assert!(!h.app.study.measure.on);
        // E alone is still the measure.
        h.key(egui::Key::E, false);
        assert!(h.app.study.measure.on);
    }

    #[test]
    fn calipers_on_splats_land_on_them() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        // A sheet of solid splats at z = 0, facing the default camera.
        let mut bytes = Vec::new();
        for i in 0..21 {
            for j in 0..21 {
                let (x, y) = (i as f32 * 0.1 - 1.0, j as f32 * 0.1 - 1.0);
                for v in [x, y, 0.0, 0.08, 0.08, 0.01] {
                    bytes.extend_from_slice(&f32::to_le_bytes(v));
                }
                bytes.extend_from_slice(&[160, 160, 160, 255, 255, 128, 128, 128]);
            }
        }
        h.app.load_reference("sheet.splat", bytes.into());
        h.app.reference.current.as_mut().unwrap().align = Default::default();
        let camera = &mut h.app.cameras[SCULPT];
        (camera.yaw, camera.pitch) = (0.0, 0.0);
        h.app.follow_camera(SCULPT);
        h.frames(2);
        let reference = h.app.view_rects[REFERENCE];
        h.app.add_caliper();
        let c = reference.center();
        h.drag(c - vec2(50.0, 0.0), c + vec2(50.0, 0.0));
        let caliper = h.app.calipers.set.list.first().copied().expect("placed");
        for end in caliper.ends {
            assert!(end.z.abs() < 0.03, "on the sheet: {end}");
        }
    }

    /// Clicks `option`'s toolbar button in view `index`.
    fn press(h: &mut Harness, option: ViewOption, index: usize) {
        use crate::PointerPhase::{Down, Move, Up};
        let bar = h.app.toolbar_layouts(h.app.view_rects)[index].clone();
        let bar = bar.expect("a toolbar");
        let k = bar.options.iter().position(|&o| o == option);
        let at = bar.buttons[k.expect("the button")].center();
        h.pointer(Move, at);
        h.pointer(Down, at);
        h.pointer(Up, at);
    }

    #[test]
    fn each_toolbar_shows_or_hides_its_views_calipers() {
        use super::super::toolbar::groups;
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let has = |index, kind| groups(index, kind).concat().contains(&ViewOption::Calipers);
        assert!(has(SCULPT, RefKind::Nothing));
        assert!(has(REFERENCE, RefKind::Mesh) && has(REFERENCE, RefKind::Splats));
        assert!(!has(REFERENCE, RefKind::Image) && !has(REFERENCE, RefKind::Nothing));

        h.app.load_reference("octahedron.obj", octahedron().into());
        h.frames(1);
        assert!(h.app.calipers_drawn(SCULPT) && h.app.calipers_drawn(REFERENCE));
        press(&mut h, ViewOption::Calipers, REFERENCE);
        assert_eq!(h.app.calipers.set.shown, [true, false]);
        assert!(!h.app.calipers_drawn(REFERENCE));
        assert!(!h.app.option_on(ViewOption::Calipers, REFERENCE));
        // Shift + click: both views alike.
        let shift = |h: &mut Harness, shift| {
            let m = crate::input::Modifiers {
                shift,
                ..Default::default()
            };
            h.app.push_event(crate::InputEvent::Modifiers(m));
        };
        shift(&mut h, true);
        press(&mut h, ViewOption::Calipers, SCULPT);
        assert_eq!(h.app.calipers.set.shown, [false; 2]);
        shift(&mut h, false);
        press(&mut h, ViewOption::Calipers, SCULPT);
        assert_eq!(h.app.calipers.set.shown, [true, false]);
        // Hidden, an end can't be taken: the press sculpts.
        let c = h.app.view_rects[SCULPT].center();
        let point = h.app.pick(h.app.view_rects[SCULPT], c).unwrap().point;
        h.app.calipers.set.add([point, point + Vec3::X * 0.3]);
        h.app.calipers.set.shown[SCULPT] = false;
        h.drag(c, c + vec2(20.0, 0.0));
        assert!(!h.app.sculpt.log().is_empty(), "a stroke");
        assert_eq!(h.app.calipers.set.list[0].ends[0], point);
        // Adding one shows them in both views again.
        h.app.add_caliper();
        assert_eq!(h.app.calipers.set.shown, [true; 2]);

        // The URL can hide them in a view.
        h.app.apply_url_options("reference=no-calipers");
        assert_eq!(h.app.calipers.set.shown, [true, false]);
    }

    #[test]
    fn calipers_come_back_with_the_scene_and_count_as_a_change() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        h.app.load_reference("octahedron.obj", octahedron().into());
        h.frames(1);
        let before = h.app.scene_revision();
        h.app.calipers.set.add([Vec3::ZERO, Vec3::Y]);
        let one = h.app.scene_revision();
        assert_ne!(one, before);
        h.app.calipers.set.list[0].ends[1] = Vec3::X;
        assert_ne!(h.app.scene_revision(), one, "an end moved");
        h.app.calipers.set.add([Vec3::NEG_Y, Vec3::Y]);
        h.app.calipers.set.list[1].visible = false;
        h.app.calipers.set.shown[REFERENCE] = false;
        let set = h.app.calipers.set.clone();
        let log = h.app.sculpt.log().len();

        let saved = h.app.autosave_bytes().expect("worth keeping").scene;
        assert_eq!(h.app.sculpt.log().len(), log, "never in the log");
        h.app.new_scene();
        assert!(h.app.calipers.set.is_default(), "a new scene has none");
        assert!(h.app.restore_autosave(&saved, None));
        assert_eq!(h.app.calipers.set, set);
    }

    #[test]
    fn the_panel_lists_them_and_its_group_can_be_taken_out() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        use crate::PointerPhase::{Down, Move, Up};
        // The calipers' group alone in the sculpt panel, at its top.
        for group in Group::SCULPT {
            if group != Group::Calipers {
                h.app.panels.toggle_group(group);
            }
        }
        h.app.calipers.set.add([Vec3::ZERO, Vec3::Y]);
        h.app.calipers.set.add([Vec3::ZERO, Vec3::Y * 1.5]);
        h.app.calipers.set.add([Vec3::ZERO, Vec3::Y * 0.5]);
        // What the widgets were is known a frame later.
        h.frames(2);
        // Each row's two small buttons: show or hide, then delete.
        let buttons = |h: &Harness| {
            let mut rects: Vec<Rect> = h.app.ctx.viewport(|v| {
                v.this_pass
                    .widgets
                    .layers()
                    .flat_map(|(_, widgets)| widgets.iter())
                    .filter(|w| w.sense.senses_click() && (w.rect.width() - 18.0).abs() < 0.01)
                    .map(|w| w.rect)
                    .filter(|r| r.right() < super::super::PANEL_WIDTH)
                    .collect()
            });
            rects.sort_by(|a, b| {
                (a.top(), a.left())
                    .partial_cmp(&(b.top(), b.left()))
                    .unwrap()
            });
            rects
        };
        let click = |h: &mut Harness, at: Pos2| {
            h.pointer(Move, at);
            h.pointer(Down, at);
            h.pointer(Up, at);
            h.frames(1);
        };
        let rows = buttons(&h);
        assert_eq!(rows.len(), 6, "{rows:?}");
        click(&mut h, rows[0].center());
        assert!(!h.app.calipers.set.list[0].visible, "hidden");
        assert!(h.app.calipers.set.list[1].visible);
        click(&mut h, rows[0].center());
        assert!(h.app.calipers.set.list[0].visible, "shown again");
        click(&mut h, rows[3].center());
        let left: Vec<u32> = h.app.calipers.set.list.iter().map(|c| c.number).collect();
        assert_eq!(left, [1, 3], "the second deleted");
        assert_eq!(h.app.calipers.set.ratio(1), Some(0.5));
        h.app.delete_calipers();
        assert!(h.app.calipers.set.list.is_empty());

        // The window menu takes the group out, and the preferences keep it.
        h.app.panels.toggle_group(Group::Calipers);
        assert!(h.app.preferences().contains("calipers"));
        h.frames(1);
        h.frames(1);
        assert!(buttons(&h).is_empty());
    }
}
