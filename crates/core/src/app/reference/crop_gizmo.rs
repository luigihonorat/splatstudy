// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The crop box's handles in the reference view, after Houdini's box
//! handle: a round handle on the centre of each face moves that face alone
//! along its normal, the opposite one staying put, and a handle on each
//! corner moves the three faces that meet there.
//!
//! A face follows the pointer along its axis as the screen shows it; a
//! corner follows it in the plane facing the eye. Both write the crop
//! sliders, clamped as the sliders are, so the panel shows each move. The
//! handles show while the crop is on and its group is open in the panel,
//! and a press on one is taken before the view sees it: it never turns
//! the camera nor starts anything else.

use egui::{CursorIcon, Pos2, Rect, Stroke, Vec2};
use glam::{Mat4, Vec3};

use super::{CropSliders, transform_box, upright};
use crate::app::{App, REFERENCE, views::shown};
use crate::camera::OrbitCamera;
use crate::input::Modifiers;
use crate::reference::{Aabb, Alignment, Content};
use crate::ui::theme;

/// How near a handle a press must land to take it, in points.
const REACH: f32 = 8.0;
/// How much further a corner may be than a face handle and still win
/// it, in points: where they crowd together, faces come first.
const CORNER_BIAS: f32 = 2.0;
/// The handles' radii, in points: face handles filled, corners as rings,
/// and the one pointed at or dragged a little larger, as the sliders'.
const FACE_RADIUS: f32 = 3.5;
const CORNER_RADIUS: f32 = 3.0;
const ACTIVE_RADIUS: f32 = 4.5;
/// A face's handle hides while its axis points almost at the eye: when it
/// looks shorter than this share of its length across the view (about
/// 12° off the line of sight), a drag along it would be all jitter.
const MIN_FORESHORTENING: f32 = 0.2;

/// One of the crop box's handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Handle {
    /// The centre of a face, by its slider: left, right, bottom, top,
    /// back, front.
    Face(usize),
    /// A corner: bit `a` set is the high side along upright axis `a`.
    Corner(usize),
}

/// A handle being dragged: where the press was, the sides then, and how
/// each side it moves follows the pointer, as fractions of the range per
/// point the pointer moves.
#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct CropDrag {
    pub(in crate::app) handle: Handle,
    start: Pos2,
    sides: [f32; 6],
    moves: [Option<(usize, Vec2)>; 3],
}

impl CropDrag {
    /// The sides with the pointer at `pos`, clamped as the sliders clamp.
    fn sides_at(&self, pos: Pos2) -> [f32; 6] {
        let delta = pos - self.start;
        let mut sliders = CropSliders {
            sides: self.sides,
            range: None,
        };
        for (side, gradient) in self.moves.into_iter().flatten() {
            sliders.set_side(side, self.sides[side] + gradient.dot(delta));
        }
        sliders.sides
    }
}

/// `p` on the screen, through `to_clip`, in `rect`, flipped left to right
/// in the mirror view; `None` behind the eye.
pub(super) fn project(to_clip: Mat4, rect: Rect, mirror: bool, p: Vec3) -> Option<Pos2> {
    let clip = to_clip * p.extend(1.0);
    (clip.w > 1e-4).then(|| {
        let ndc = clip.truncate() / clip.w;
        let x = if mirror { -ndc.x } else { ndc.x };
        Pos2::new(
            rect.left() + (x + 1.0) / 2.0 * rect.width(),
            rect.top() + (1.0 - ndc.y) / 2.0 * rect.height(),
        )
    })
}

/// The crop box as the reference view shows it, in the upright frame the
/// sliders work in: what puts it on the screen and where its handles are.
struct Gizmo {
    /// Upright coordinates to the clay's space, and to clip space.
    to_world: Mat4,
    to_clip: Mat4,
    camera: OrbitCamera,
    rect: Rect,
    mirror: bool,
    /// The upright box the sliders span.
    range: Aabb,
    sides: [f32; 6],
}

impl Gizmo {
    fn new(
        camera: &OrbitCamera,
        rect: Rect,
        mirror: bool,
        align: Alignment,
        sliders: CropSliders,
    ) -> Option<Self> {
        let turn = upright(align.up);
        let to_world = align.matrix() * turn.inverse();
        Some(Self {
            to_world,
            to_clip: camera.projection(rect.aspect_ratio()) * camera.view() * to_world,
            camera: *camera,
            rect,
            mirror,
            range: transform_box(turn, sliders.range?),
            sides: sliders.sides,
        })
    }

    fn size(&self) -> Vec3 {
        self.range.size().max(Vec3::splat(1e-12))
    }

    /// The point at fractions `f` of the range, in upright coordinates.
    fn at(&self, f: Vec3) -> Vec3 {
        self.range.min + f * self.size()
    }

    fn screen(&self, f: Vec3) -> Option<Pos2> {
        project(self.to_clip, self.rect, self.mirror, self.at(f))
    }

    /// Where `handle` sits, as fractions of the range.
    fn place(&self, handle: Handle) -> Vec3 {
        let [l, r, b, t, back, front] = self.sides;
        let (lo, hi) = (Vec3::new(l, b, back), Vec3::new(r, t, front));
        match handle {
            Handle::Face(i) => {
                let mut p = (lo + hi) / 2.0;
                p[i / 2] = self.sides[i];
                p
            }
            Handle::Corner(c) => {
                Vec3::select(glam::BVec3::new(c & 1 != 0, c & 2 != 0, c & 4 != 0), hi, lo)
            }
        }
    }

    /// How far face `i`'s handle moves on the screen per unit of its
    /// slider, in points; `None` when its axis points (almost) at the eye
    /// or behind it.
    fn face_axis(&self, i: usize) -> Option<Vec2> {
        const H: f32 = 0.01;
        let a = i / 2;
        let p = self.place(Handle::Face(i));
        let step = Vec3::AXES[a] * H;
        let d = (self.screen(p + step)? - self.screen(p - step)?) / (2.0 * H);
        // As long as the same step would look across the view.
        let world = self.to_world.transform_point3(self.at(p));
        let length = self
            .to_world
            .transform_vector3(Vec3::AXES[a] * self.size()[a])
            .length();
        let across = length / self.camera.world_per_point(world, self.rect.height());
        (d.length() > 1e-3 && d.length() >= MIN_FORESHORTENING * across).then_some(d)
    }

    /// The handles that show, and where.
    fn handles(&self) -> Vec<(Handle, Pos2)> {
        (0..6)
            .filter(|&i| self.face_axis(i).is_some())
            .map(Handle::Face)
            .chain((0..8).map(Handle::Corner))
            .filter_map(|h| Some((h, self.screen(self.place(h))?)))
            .collect()
    }

    /// The handle under `pos`: the nearest within reach, faces first.
    fn hit(&self, pos: Pos2) -> Option<Handle> {
        self.handles()
            .into_iter()
            .filter(|(_, at)| at.distance(pos) <= REACH)
            .map(|(h, at)| {
                let bias = if matches!(h, Handle::Corner(_)) {
                    CORNER_BIAS
                } else {
                    0.0
                };
                (h, at.distance(pos) + bias)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(h, _)| h)
    }

    /// A drag of `handle` from `pos`. A face follows the pointer along its
    /// axis as the screen shows it: moving the pointer along that line by
    /// the axis's length on the screen moves the face by the whole range.
    /// A corner follows the pointer in the plane through it that faces
    /// the eye, and each of its three faces takes its share of the move.
    fn drag(&self, handle: Handle, pos: Pos2) -> Option<CropDrag> {
        let moves = match handle {
            Handle::Face(i) => {
                let d = self.face_axis(i)?;
                [Some((i, d / d.length_sq())), None, None]
            }
            Handle::Corner(c) => {
                let world = self.to_world.transform_point3(self.at(self.place(handle)));
                let per_point = self.camera.world_per_point(world, self.rect.height());
                let (right, up) = self.camera.right_up();
                let back = self.to_world.inverse();
                let right = back.transform_vector3(right * per_point) / self.size();
                let up = back.transform_vector3(up * per_point) / self.size();
                // The mirror view shows the image flipped left to right.
                let sign = if self.mirror { -1.0 } else { 1.0 };
                [0, 1, 2].map(|a| {
                    let side = 2 * a + (c >> a & 1);
                    Some((side, Vec2::new(sign * right[a], -up[a])))
                })
            }
        };
        Some(CropDrag {
            handle,
            start: pos,
            sides: self.sides,
            moves,
        })
    }

    /// The four corners of face `i`, in order around it, on the screen.
    fn face_outline(&self, i: usize) -> Vec<Pos2> {
        let (a, b) = ((i / 2 + 1) % 3, (i / 2 + 2) % 3);
        let mut p = self.place(Handle::Face(i));
        [(0, 0), (1, 0), (1, 1), (0, 1)]
            .into_iter()
            .filter_map(|(u, v)| {
                p[a] = self.sides[2 * a + u];
                p[b] = self.sides[2 * b + v];
                self.screen(p)
            })
            .collect()
    }
}

impl App {
    /// The crop box's handles in the reference view laid out in `rect`,
    /// if they show: the crop is on and its group was drawn open in the
    /// panel this pass, or one of them is being dragged.
    fn crop_gizmo(&self, ctx: &egui::Context, rect: Rect) -> Option<Gizmo> {
        let state = &self.reference;
        let r = state.current.as_ref()?;
        let editing =
            state.crop_drag.is_some() || state.crop_shown == Some(ctx.cumulative_pass_nr());
        if r.crop.is_none() || matches!(r.content, Content::Image(_)) || !editing || !shown(rect) {
            return None;
        }
        let camera = &self.cameras[REFERENCE];
        let mirror = self.study.views[REFERENCE].mirror;
        Gizmo::new(camera, rect, mirror, r.align, state.crop)
    }

    /// A left press at `screen` in view `index`, before the view sees it:
    /// returns true if a crop handle took it. With a modifier the press
    /// still navigates, as the measure leaves it.
    pub(in crate::app) fn crop_press(
        &mut self,
        ctx: &egui::Context,
        rect: Rect,
        index: usize,
        screen: Pos2,
        m: Modifiers,
    ) -> bool {
        if index != REFERENCE || m.alt || m.ctrl || m.command {
            return false;
        }
        let Some(gizmo) = self.crop_gizmo(ctx, rect) else {
            return false;
        };
        let Some(drag) = gizmo.hit(screen).and_then(|h| gizmo.drag(h, screen)) else {
            return false;
        };
        self.reference.crop_drag = Some(drag);
        true
    }

    /// Moves the dragged handle's faces with the pointer, and lets go of
    /// it with the button, fitting the reference to the new box as the
    /// sliders do once let go.
    pub(super) fn crop_follow(&mut self, ui: &egui::Ui) {
        let state = &mut self.reference;
        let Some(drag) = state.crop_drag else {
            return;
        };
        let (held, pointer) = ui.input(|i| (i.pointer.primary_down(), i.pointer.interact_pos()));
        let Some(r) = state.current.as_mut() else {
            state.crop_drag = None;
            return;
        };
        if !held {
            state.crop_drag = None;
            r.fit();
            return;
        }
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        if let Some(pos) = pointer
            && r.crop.is_some()
        {
            state.crop.sides = drag.sides_at(pos);
            r.crop = state.crop.to_box(r.align.up);
        }
    }

    /// Draws the handles over the crop box: the one pointed at or dragged
    /// larger and in the view's bright ink, with the face it moves outlined.
    pub(super) fn crop_handles(&self, ui: &egui::Ui, painter: &egui::Painter, rect: Rect) {
        let Some(gizmo) = self.crop_gizmo(ui.ctx(), rect) else {
            return;
        };
        let active = match self.reference.crop_drag {
            Some(drag) => Some(drag.handle),
            None => self.crop_hover(ui, rect).and_then(|p| gizmo.hit(p)),
        };
        if active.is_some() && self.reference.crop_drag.is_none() {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
        let bright = theme::view_inks(self.view_background(REFERENCE)).ink;
        let ink = Stroke::new(1.0, bright);
        if let Some(Handle::Face(i)) = active {
            let outline = gizmo.face_outline(i);
            if outline.len() == 4 {
                for k in 0..4 {
                    painter.line_segment([outline[k], outline[(k + 1) % 4]], ink);
                }
            }
        }
        let line = Stroke::new(1.0, theme::CROP_LINE);
        let mut handles = gizmo.handles();
        // Corners under faces, and the active handle over all.
        handles.sort_by_key(|&(h, _)| (Some(h) == active, matches!(h, Handle::Face(_))));
        for (handle, at) in handles {
            match handle {
                _ if Some(handle) == active => {
                    painter.circle_filled(at, ACTIVE_RADIUS, bright);
                }
                Handle::Face(_) => {
                    painter.circle_filled(at, FACE_RADIUS, theme::CROP_LINE);
                }
                Handle::Corner(_) => {
                    painter.circle_stroke(at, CORNER_RADIUS, line);
                }
            }
        }
    }

    /// Where the pointer is, when it is free to take a handle there: over
    /// the reference view itself, not its toolbar, a menu or a window,
    /// and nothing else under way.
    fn crop_hover(&self, ui: &egui::Ui, rect: Rect) -> Option<Pos2> {
        let (pointer, m) = ui.input(|i| (i.pointer.hover_pos(), Modifiers::from(i.modifiers)));
        let pointer = pointer?;
        let toolbar = self.toolbar_areas(self.view_rects)[REFERENCE];
        let busy = self.drags[REFERENCE].is_some()
            || self.popup_open()
            || self.flyout.is_some()
            || self.study.measure.drawing.is_some()
            || self.study.plumb_drag.is_some()
            || m.alt
            || m.ctrl
            || m.command;
        (!busy && ui.rect_contains_pointer(rect) && !toolbar.contains(pointer)).then_some(pointer)
    }
}

#[cfg(test)]
mod tests {
    use egui::pos2;

    use super::*;
    use crate::app::study::tests::Harness;
    use crate::reference::UpAxis;

    fn rect() -> Rect {
        Rect::from_min_size(pos2(640.0, 30.0), egui::vec2(640.0, 690.0))
    }

    /// The default camera, square on to the front: x right, y up.
    fn front() -> OrbitCamera {
        OrbitCamera {
            pitch: 0.0,
            ortho: true,
            ..Default::default()
        }
    }

    /// A box from -1 to 1 placed as it is, cropped to its middle half.
    fn gizmo(camera: OrbitCamera, mirror: bool) -> Gizmo {
        let sliders = CropSliders {
            sides: [0.25, 0.75, 0.25, 0.75, 0.25, 0.75],
            range: Some(Aabb {
                min: Vec3::splat(-1.0),
                max: Vec3::ONE,
            }),
        };
        Gizmo::new(&camera, rect(), mirror, Alignment::default(), sliders).unwrap()
    }

    #[test]
    fn faces_seen_face_on_have_no_handle_and_the_others_sit_on_the_box() {
        let g = gizmo(front(), false);
        let faces: Vec<Handle> = g
            .handles()
            .into_iter()
            .map(|(h, _)| h)
            .filter(|h| matches!(h, Handle::Face(_)))
            .collect();
        // Back and front point at the eye.
        assert_eq!(faces, (0..4).map(Handle::Face).collect::<Vec<_>>());
        let centre = rect().center();
        let right = g.screen(g.place(Handle::Face(1))).unwrap();
        let left = g.screen(g.place(Handle::Face(0))).unwrap();
        let top = g.screen(g.place(Handle::Face(3))).unwrap();
        assert!((right.y - centre.y).abs() < 0.01 && right.x > centre.x + 10.0);
        assert!((left.x - centre.x + (right.x - centre.x)).abs() < 0.01);
        assert!(top.y < centre.y - 10.0, "y up is up the screen");
        // Turned, all six show.
        let mut turned = front();
        turned.orbit(60.0, 40.0);
        assert_eq!(
            gizmo(turned, false)
                .handles()
                .iter()
                .filter(|(h, _)| matches!(h, Handle::Face(_)))
                .count(),
            6
        );
    }

    #[test]
    fn a_press_takes_the_nearest_handle_within_reach() {
        let g = gizmo(front(), false);
        let right = g.screen(g.place(Handle::Face(1))).unwrap();
        assert_eq!(g.hit(right + egui::vec2(3.0, -4.0)), Some(Handle::Face(1)));
        assert_eq!(g.hit(right + egui::vec2(REACH + 1.0, 0.0)), None);
        assert_eq!(g.hit(rect().center()), None, "not inside the box");
        // The top right corner, seen from the front, is the front one: the
        // back one sits under it, as near.
        let corner = g.screen(g.place(Handle::Corner(0b011))).unwrap();
        assert!(matches!(
            g.hit(corner + egui::vec2(1.0, 1.0)),
            Some(Handle::Corner(0b011 | 0b111))
        ));
        // The mirror view shows the right face on the left.
        let m = gizmo(front(), true);
        let mirrored = m.screen(m.place(Handle::Face(1))).unwrap();
        assert!((mirrored.x - (rect().left() + rect().right() - right.x)).abs() < 0.01);
        assert_eq!(m.hit(mirrored), Some(Handle::Face(1)));
    }

    #[test]
    fn a_face_follows_the_pointer_along_its_axis_alone() {
        let mut camera = front();
        camera.orbit(50.0, 30.0);
        camera.ortho = false;
        for mirror in [false, true] {
            let g = gizmo(camera, mirror);
            let at = g.screen(g.place(Handle::Face(1))).unwrap();
            let d = g.face_axis(1).unwrap();
            let drag = g.drag(Handle::Face(1), at).unwrap();
            // A tenth of the range along the axis; across it, nothing.
            let sides = drag.sides_at(at + d * 0.1);
            assert!((sides[1] - 0.85).abs() < 1e-4, "{sides:?}");
            assert_eq!(sides[0], 0.25, "the opposite face stays put");
            assert_eq!(&sides[2..], &g.sides[2..]);
            let across = drag.sides_at(at + d.rot90() * 0.3);
            assert!((across[1] - 0.75).abs() < 1e-4);
            // And it lands under the pointer, give or take perspective.
            let mut moved = gizmo(camera, mirror);
            moved.sides = sides;
            let now = moved.screen(moved.place(Handle::Face(1))).unwrap();
            assert!(now.distance(at + d * 0.1) < 2.0, "{now:?}");
        }
    }

    #[test]
    fn faces_stop_short_of_their_opposite_and_the_range() {
        let g = gizmo(front(), false);
        let at = g.screen(g.place(Handle::Face(1))).unwrap();
        let d = g.face_axis(1).unwrap();
        let drag = g.drag(Handle::Face(1), at).unwrap();
        assert!((drag.sides_at(at - d * 2.0)[1] - 0.26).abs() < 1e-6);
        assert_eq!(drag.sides_at(at + d * 2.0)[1], 1.0);
    }

    #[test]
    fn a_corner_moves_its_three_faces_in_the_plane_facing_the_eye() {
        let g = gizmo(front(), false);
        // Top right front.
        let corner = Handle::Corner(0b111);
        let at = g.screen(g.place(corner)).unwrap();
        let drag = g.drag(corner, at).unwrap();
        let right = g.screen(g.place(Handle::Face(1))).unwrap();
        let left = g.screen(g.place(Handle::Face(0))).unwrap();
        // Half the box's width on the screen is a quarter of the range.
        let half = (right.x - left.x) / 2.0;
        let sides = drag.sides_at(at + egui::vec2(half, half));
        assert!((sides[1] - 1.0).abs() < 1e-4, "{sides:?}");
        assert!((sides[3] - 0.5).abs() < 1e-4, "down the screen is down");
        assert_eq!(sides[5], 0.75, "facing the eye, depth stays");
        assert_eq!([sides[0], sides[2], sides[4]], [0.25; 3]);
    }

    #[test]
    fn handles_follow_the_up_axis() {
        // A Z-up file: the sliders' "top" is the file's high z, and its
        // handle is still at the top of the screen.
        let sliders = CropSliders {
            sides: [0.0, 1.0, 0.0, 1.0, 0.0, 1.0],
            range: Some(Aabb {
                min: Vec3::ZERO,
                max: Vec3::new(1.0, 2.0, 3.0),
            }),
        };
        let align = Alignment {
            up: UpAxis::Z,
            centre: Vec3::new(0.5, 1.0, 1.5),
            scale: 0.5,
            ..Default::default()
        };
        let g = Gizmo::new(&front(), rect(), false, align, sliders).unwrap();
        let top = g.screen(g.place(Handle::Face(3))).unwrap();
        let bottom = g.screen(g.place(Handle::Face(2))).unwrap();
        assert!(top.y < bottom.y);
        // The file's z is 3 long, scaled by a half: 1.5 of the view's
        // height in world units.
        let expected = 1.5 / (2.0 * front().half_height()) * rect().height();
        assert!((bottom.y - top.y - expected).abs() < 0.5);
        let box_ = transform_box(align.matrix(), sliders.to_box(UpAxis::Z).unwrap());
        assert!((box_.size().y - 1.5).abs() < 1e-4);
    }

    /// A cube from -1 to 1, as an OBJ.
    fn cube_obj() -> Vec<u8> {
        let mut obj = String::new();
        for i in 0..8 {
            let c = |bit: usize| if i & bit == 0 { -1.0 } else { 1.0 };
            obj += &format!("v {} {} {}\n", c(1), c(2), c(4));
        }
        // Two triangles on each face: the corners with bit `a` at `side`.
        for a in [1, 2, 4] {
            for side in [0, a] {
                let mut corners: Vec<usize> = (0..8).filter(|i| i & a == side).collect();
                corners.swap(2, 3);
                let [p, q, r, s] = [0, 1, 2, 3].map(|k| corners[k] + 1);
                obj += &format!("f {p} {q} {r}\nf {p} {r} {s}\n");
            }
        }
        obj.into_bytes()
    }

    /// The app with the cube as its reference, cropped to the whole range.
    fn cropped_cube() -> Option<Harness> {
        let mut h = Harness::new()?;
        h.app.load_reference("cube.obj", cube_obj().into());
        let state = &mut h.app.reference;
        let r = state.current.as_mut().unwrap();
        r.crop = state.crop.to_box(r.align.up);
        r.fit();
        h.frames(2);
        Some(h)
    }

    /// The handles as the reference view shows them now.
    fn gizmo_of(app: &App) -> Gizmo {
        let rect = app.view_rects[REFERENCE];
        let state = &app.reference;
        let r = state.current.as_ref().unwrap();
        let mirror = app.study.views[REFERENCE].mirror;
        Gizmo::new(&app.cameras[REFERENCE], rect, mirror, r.align, state.crop).unwrap()
    }

    #[test]
    fn a_face_handle_drags_its_face_alone_and_never_the_camera() {
        let Some(mut h) = cropped_cube() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let g = gizmo_of(&h.app);
        let at = g.screen(g.place(Handle::Face(1))).unwrap();
        let d = g.face_axis(1).unwrap();
        let camera = h.app.cameras[REFERENCE];
        let before = h.app.reference.crop.sides;
        let log = h.app.sculpt.log().len();

        // The right face in by a fifth of the range.
        h.drag(at, at - d * 0.2);
        let after = h.app.reference.crop.sides;
        assert!((after[1] - (before[1] - 0.2)).abs() < 0.01, "{after:?}");
        assert_eq!([after[0], after[2], after[3], after[4], after[5]], {
            let [l, _, b, t, back, front] = before;
            [l, b, t, back, front]
        });
        assert_eq!(h.app.cameras[REFERENCE], camera, "the camera stays");
        assert!(h.app.drags.iter().all(Option::is_none));
        assert!(h.app.reference.crop_drag.is_none(), "let go");
        assert_eq!(h.app.sculpt.log().len(), log);
        // The box is the sliders'.
        let r = h.app.reference.current.as_ref().unwrap();
        assert_eq!(r.crop, h.app.reference.crop.to_box(r.align.up));

        // Away from the handles, a drag still turns the view.
        let rect = h.app.view_rects[REFERENCE];
        let empty = rect.left_bottom() + egui::vec2(60.0, -80.0);
        h.drag(empty, empty + egui::vec2(40.0, 0.0));
        assert_ne!(h.app.cameras[REFERENCE], camera);
        assert_eq!(h.app.reference.crop.sides, after);
    }

    #[test]
    fn a_pen_drags_a_handle_too() {
        use crate::{Button, InputEvent, PointerEvent, PointerKind, PointerPhase};
        let Some(mut h) = cropped_cube() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let g = gizmo_of(&h.app);
        let at = g.screen(g.place(Handle::Face(3))).unwrap();
        let d = g.face_axis(3).unwrap();
        let camera = h.app.cameras[REFERENCE];
        let to = at - d * 0.3;
        let pen = |h: &mut Harness, phase, pos: Pos2| {
            let down = phase != PointerPhase::Move;
            h.app.push_event(InputEvent::Pointer(PointerEvent {
                id: 2,
                kind: PointerKind::Pen,
                phase,
                pos: [pos.x, pos.y],
                button: down.then_some(Button::Primary),
                pressure: if phase == PointerPhase::Up { 0.0 } else { 0.4 },
                eraser: false,
            }));
            h.frames(1);
        };
        // No hover first: the pen lands on the handle.
        pen(&mut h, PointerPhase::Down, at);
        for k in 1..=4 {
            pen(&mut h, PointerPhase::Move, at.lerp(to, k as f32 / 4.0));
        }
        pen(&mut h, PointerPhase::Up, to);
        let top = h.app.reference.crop.sides[3];
        assert!((top - 0.7).abs() < 0.01, "{top}");
        assert_eq!(h.app.cameras[REFERENCE], camera);
    }

    /// When the mesh shown was built, and for which placement and crop.
    fn built(
        app: &App,
    ) -> (
        f64,
        (crate::reference::Alignment, Option<crate::reference::Aabb>),
    ) {
        match &app.reference.drawn {
            Some(super::super::Drawn::Mesh {
                built_at,
                built_for,
                mesh: Some(_),
                ..
            }) => (*built_at, *built_for),
            _ => panic!("no mesh shown"),
        }
    }

    #[test]
    fn a_dragged_crop_cuts_the_mesh_now_and_then_and_when_let_go() {
        use crate::PointerPhase::{Down, Move, Up};
        let Some(mut h) = cropped_cube() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let g = gizmo_of(&h.app);
        let at = g.screen(g.place(Handle::Face(1))).unwrap();
        let d = g.face_axis(1).unwrap();
        h.pointer(Move, at);
        h.pointer(Down, at);
        // A fifth of a second of moves, a frame each.
        let mut builds = Vec::new();
        for k in 1..=12 {
            h.pointer(Move, at - d * (0.2 * k as f32 / 12.0));
            builds.push(built(&h.app).0);
        }
        builds.dedup();
        assert!((1..=3).contains(&builds.len()), "{builds:?}");
        // Held still, it catches up.
        h.frames(8);
        let r = h.app.reference.current.as_ref().unwrap();
        assert_eq!(built(&h.app).1.1, r.crop);
        // Let go: fitted, and built for the fit.
        h.pointer(Up, at - d * 0.2);
        h.frames(1);
        let r = h.app.reference.current.as_ref().unwrap();
        assert_eq!(built(&h.app).1, (r.align, r.crop));
    }

    #[test]
    fn the_handles_show_only_with_the_crop_group() {
        let Some(mut h) = cropped_cube() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let g = gizmo_of(&h.app);
        let at = g.screen(g.place(Handle::Face(1))).unwrap();
        let camera = h.app.cameras[REFERENCE];
        let before = h.app.reference.crop.sides;
        // Taken out of the panel (the window menu), the group leaves the
        // box without handles, and a press there turns the view.
        h.app.panels.toggle_group(crate::app::panels::Group::Crop);
        h.frames(1);
        h.drag(at, at - egui::vec2(40.0, 0.0));
        assert_eq!(h.app.reference.crop.sides, before);
        assert_ne!(h.app.cameras[REFERENCE], camera);
    }
}
