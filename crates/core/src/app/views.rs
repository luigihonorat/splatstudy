// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The two views side by side: layout, the divider, pointer input,
//! navigation and viewpoints.

use egui::{Align2, Color32, CursorIcon, PointerButton, Pos2, Rect, Sense, Stroke};
use glam::Vec3;

use super::study::Layout;
use super::{App, Drag, REFERENCE, SCULPT};
use crate::camera::OrbitCamera;
use crate::input::{Button, Modifiers, PointerKind, PointerPhase};
use crate::navigation::{NavAction, snap_view};
use crate::sculpt::LayerOp;
use crate::ui::{fonts, i18n::Tr, theme};

/// Navigation speeds: dolly per point dragged, and per point scrolled.
const DOLLY_DRAG: f32 = 0.005;
const DOLLY_WHEEL: f32 = 0.0015;
/// How far, in points, a Ctrl press on the background may move and still
/// be a tap that inverts the mask.
const MASK_TAP: f32 = 4.0;

/// Preset viewpoints: label, numpad digit, yaw and pitch. Right and left
/// are the viewer's, as seen in the front view. Front and back share 5:
/// the key turns from one to the other.
pub(super) fn viewpoints(tr: &Tr) -> [(&'static str, Option<u8>, f32, f32); 7] {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};
    [
        (tr.front, Some(5), 0.0, 0.0),
        (tr.back, Some(5), PI, 0.0),
        (tr.left, Some(4), -FRAC_PI_2, 0.0),
        (tr.right, Some(6), FRAC_PI_2, 0.0),
        (tr.top, Some(8), 0.0, FRAC_PI_2),
        (tr.study.bottom, Some(2), 0.0, -FRAC_PI_2),
        (tr.three_quarter, None, FRAC_PI_4, 0.3),
    ]
}

/// The viewpoints' names in URL options (`view=three-quarter`), in
/// `viewpoints` order.
pub(super) const VIEWPOINT_IDS: [&str; 7] = [
    "front",
    "back",
    "left",
    "right",
    "top",
    "bottom",
    "three-quarter",
];

/// A numpad key as the menus and the shortcuts window show it.
pub(super) fn numpad_name(digit: u8) -> String {
    format!("Numpad {digit}")
}

/// Whether `camera` already looks from `yaw` and `pitch`, give or take a
/// hair, however many turns its yaw has made.
fn looks_from(camera: &OrbitCamera, yaw: f32, pitch: f32) -> bool {
    use std::f32::consts::{PI, TAU};
    const CLOSE: f32 = 0.01;
    let mut there = *camera;
    there.look_from(yaw, pitch);
    let turn = (there.yaw - camera.yaw + PI).rem_euclid(TAU) - PI;
    turn.abs() < CLOSE && (there.pitch - camera.pitch).abs() < CLOSE
}

/// Where the sculpt and reference views sit in `area` (indexed by `SCULPT`
/// and `REFERENCE`), and the divider's x when both show. `split` is the
/// share of the width given to the sculpt view, wherever it sits. A hidden
/// view gets an empty rectangle.
pub(super) fn layout(
    area: Rect,
    split: f32,
    layout: Layout,
    swapped: bool,
) -> ([Rect; 2], Option<f32>) {
    let hidden = Rect::from_min_size(area.min, egui::Vec2::ZERO);
    match layout {
        Layout::Sculpt => ([area, hidden], None),
        Layout::Reference => ([hidden, area], None),
        Layout::Both => {
            let first = if swapped { 1.0 - split } else { split };
            let x = area.left() + (area.width() * first).round();
            let left = Rect::from_x_y_ranges(area.left()..=x, area.y_range());
            let right = Rect::from_x_y_ranges(x + 1.0..=area.right(), area.y_range());
            let rects = if swapped {
                [right, left]
            } else {
                [left, right]
            };
            (rects, Some(x))
        }
    }
}

/// Reflects `pos` about the vertical centre line of `rect` when `on`.
fn mirror_x(rect: Rect, pos: Pos2, on: bool) -> Pos2 {
    if on {
        egui::pos2(rect.left() + rect.right() - pos.x, pos.y)
    } else {
        pos
    }
}

/// The button as the navigation presets name it.
fn button_of(button: PointerButton) -> Button {
    match button {
        PointerButton::Secondary => Button::Secondary,
        PointerButton::Middle => Button::Middle,
        _ => Button::Primary,
    }
}

/// A view is shown when it has an area.
pub(super) fn shown(rect: Rect) -> bool {
    rect.width() > 0.0 && rect.height() > 0.0
}

impl App {
    /// Turns both views to the same direction, so they can be compared.
    pub(super) fn view_from(&mut self, yaw: f32, pitch: f32) {
        for camera in &mut self.cameras {
            camera.look_from(yaw, pitch);
        }
    }

    /// The viewpoint on numpad `digit`, for both views. Where several
    /// share the key, each press turns to the next, as the sculpt view
    /// sees it: from the front, 5 turns to the back, from anywhere else to
    /// the front.
    pub(super) fn numpad_view(&mut self, digit: u8) {
        let views: Vec<(f32, f32)> = viewpoints(self.lang.tr())
            .into_iter()
            .filter(|&(_, key, _, _)| key == Some(digit))
            .map(|(_, _, yaw, pitch)| (yaw, pitch))
            .collect();
        let camera = &self.cameras[SCULPT];
        let next = views
            .iter()
            .position(|&(yaw, pitch)| looks_from(camera, yaw, pitch))
            .map_or(0, |k| (k + 1) % views.len());
        if let Some(&(yaw, pitch)) = views.get(next) {
            self.view_from(yaw, pitch);
        }
    }

    /// Puts view `index`'s camera back where it starts, in the projection
    /// it has.
    pub(super) fn reset_view(&mut self, index: usize) {
        let ortho = self.cameras[index].ortho;
        self.cameras[index] = OrbitCamera {
            ortho,
            ..OrbitCamera::default()
        };
        self.follow_camera(index);
    }

    /// Fits the sculpt view to the clay's bounding sphere.
    pub(super) fn frame_sculpt(&mut self) {
        let positions = &self.sculpt.mesh.positions;
        if positions.is_empty() {
            return;
        }
        let (min, max) = positions
            .iter()
            .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), &p| {
                (lo.min(p), hi.max(p))
            });
        let centre = (min + max) / 2.0;
        let radius = positions
            .iter()
            .map(|p| p.distance(centre))
            .fold(0.0, f32::max);
        self.cameras[SCULPT].frame(centre, radius);
        self.follow_camera(SCULPT);
    }

    /// With the cameras linked, gives the other view the camera of view
    /// `index`, which just moved. Both views share the clay's space, so a
    /// plain copy lines them up.
    pub(super) fn follow_camera(&mut self, index: usize) {
        if self.study.link {
            self.cameras[1 - index] = self.cameras[index];
        }
    }

    pub(super) fn views_ui(&mut self, ui: &mut egui::Ui) {
        let p = self.theme.palette();
        let tr = self.lang.tr();
        let area = ui.max_rect();
        let study = &self.study;
        let (rects, divider) = layout(area, self.split, study.layout, study.swapped);

        // The divider between the two views, draggable.
        if let Some(x) = divider {
            let handle = Rect::from_x_y_ranges(x - 4.0..=x + 4.0, area.y_range());
            let handle = ui.interact(handle, ui.scope_id().with("split"), Sense::drag());
            if handle.dragged()
                && let Some(pos) = handle.interact_pointer_pos()
            {
                let t = (pos.x - area.left()) / area.width();
                let t = if self.study.swapped { 1.0 - t } else { t };
                self.split = t.clamp(0.2, 0.8);
            }
            if handle.hovered() || handle.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
        }

        self.view_rects = rects;
        // The view the pointer is over: the view menu switches its aids.
        let hover = ui.input(|i| i.pointer.hover_pos());
        if let Some(index) = [SCULPT, REFERENCE]
            .into_iter()
            .find(|&i| shown(rects[i]) && hover.is_some_and(|p| rects[i].contains(p)))
        {
            self.study.focus = index;
        }
        // An open flyout lies beside its toolbar and counts as part of it.
        let toolbars = self.toolbar_areas(rects);
        let flyout = self.flyout_area();
        let on_toolbar =
            |index: usize| hover.is_some_and(|p| toolbars[index].contains(p) || flyout.contains(p));
        for index in [SCULPT, REFERENCE] {
            if shown(rects[index]) {
                self.view_input(ui, rects[index], index, on_toolbar(index));
            }
        }

        let painter = ui.painter();
        // The mirror view flips the images; what is drawn over them is not.
        for (index, (view, rect)) in self.views.iter().zip(rects).enumerate() {
            let uv = if self.study.views[index].mirror {
                Rect::from_min_max(egui::pos2(1.0, 0.0), egui::pos2(0.0, 1.0))
            } else {
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
            };
            if let Some(id) = view.egui_id
                && shown(rect)
            {
                painter.image(id, rect, uv, Color32::WHITE);
            }
        }
        // A picture, the crop box and the credit, in this frame's rectangle.
        if shown(rects[REFERENCE]) {
            self.reference_overlay(ui, rects[REFERENCE]);
        }
        painter.hline(area.x_range(), area.top() + 0.5, Stroke::new(1.0, p.border));
        if let Some(x) = divider {
            painter.vline(x + 0.5, area.y_range(), Stroke::new(1.0, p.border));
        }
        self.guides(ui, rects);
        let painter = ui.painter();

        let label_font = fonts::regular(12.0);
        let sculpt_label = if self.playback.is_some() {
            tr.timelapse
        } else {
            tr.sculpt
        };
        for (rect, text) in rects.into_iter().zip([sculpt_label, tr.reference]) {
            if shown(rect) {
                painter.text(
                    rect.left_top() + egui::vec2(14.0, 12.0),
                    Align2::LEFT_TOP,
                    text,
                    label_font.clone(),
                    theme::VIEW_TEXT,
                );
            }
        }
        // Empty reference view: say what can go there, with a link to the
        // gallery; or that a sample is on its way.
        let reference = rects[REFERENCE];
        let loading = self.loading_text();
        if shown(reference) && !self.has_reference() {
            painter.with_clip_rect(reference).text(
                reference.center(),
                Align2::CENTER_CENTER,
                loading.as_deref().unwrap_or(tr.drop_hint),
                fonts::light(16.0),
                theme::VIEW_TEXT,
            );
            self.gallery_link(ui, reference.center() + egui::vec2(0.0, 18.0));
        } else if let Some(text) = loading
            && shown(reference)
        {
            painter.with_clip_rect(reference).text(
                reference.center_top() + egui::vec2(0.0, 12.0),
                Align2::CENTER_TOP,
                text,
                fonts::regular(12.0),
                theme::VIEW_TEXT,
            );
        }
        self.timer_ui(ui, area, rects);
        self.toolbars(ui, rects);
        if shown(rects[SCULPT]) {
            self.brush_flash(ui, rects[SCULPT]);
        }

        let left = rects[SCULPT];
        if shown(left) {
            // The toolbar hides the brush beneath it, but not that of a
            // stroke passing over it.
            if !on_toolbar(SCULPT) || self.drags[SCULPT].is_some() {
                self.tool_cursor(ui, left);
            }
            if self.debug {
                self.debug_hud(ui, left);
            }
        }
        self.tool_pie_ui(ui);
    }

    /// The point of the rendered sculpt view under `pos`, a point of the
    /// screen in the view's `rect`.
    pub(super) fn view_pos(&self, rect: Rect, pos: Pos2) -> Pos2 {
        self.view_pos_in(SCULPT, rect, pos)
    }

    /// The point of view `index`'s rendered image under `pos`, a point of
    /// the screen in the view's `rect`. The mirror view shows the image
    /// flipped, so there x reflects about the view's centre.
    pub(super) fn view_pos_in(&self, index: usize, rect: Rect, pos: Pos2) -> Pos2 {
        mirror_x(rect, pos, self.study.views[index].mirror)
    }

    /// Pointer input in one view: the navigation preset decides what each
    /// press starts; in the sculpt view a left press on the clay that the
    /// preset leaves free sculpts. The reference view has nothing to sculpt,
    /// so a free left drag turns it. Over its toolbar a view takes no press
    /// and no scroll.
    fn view_input(&mut self, ui: &egui::Ui, rect: Rect, index: usize, on_toolbar: bool) {
        let response = ui.interact(
            rect,
            ui.scope_id().with(("view", index)),
            Sense::click_and_drag(),
        );
        let (m, pointer, delta, scroll) = ui.input(|i| {
            (
                Modifiers::from(i.modifiers),
                i.pointer.interact_pos(),
                i.pointer.delta(),
                i.smooth_scroll_delta.y,
            )
        });
        // In the mirror view the image is flipped: the pointer is reflected
        // into the rendered view, and sideways drags turn the other way so
        // the clay still follows the hand. The guides stay on the screen.
        let screen = pointer;
        let pointer = pointer.map(|pos| self.view_pos_in(index, rect, pos));
        // Zooming goes by the hand on the screen, mirrored or not.
        let zoom = self.nav.zoom(delta.x, delta.y);
        let delta = if self.study.views[index].mirror {
            egui::vec2(-delta.x, delta.y)
        } else {
            delta
        };
        let camera_before = self.cameras[index];

        // An open pie menu takes every press; the toolbar takes its own.
        // While a flyout is open, a press anywhere only closes it.
        if self.drags[index].is_none()
            && response.hovered()
            && !self.pie_open()
            && self.flyout.is_none()
            && !on_toolbar
        {
            for (button, ours) in [
                (PointerButton::Primary, Button::Primary),
                (PointerButton::Middle, Button::Middle),
                (PointerButton::Secondary, Button::Secondary),
            ] {
                if !ui.input(|i| i.pointer.button_pressed(button)) {
                    continue;
                }
                // The crop box's handles and the study aids' own (the
                // plumb line's bob) take a left press first, but never one
                // on the clay.
                if ours == Button::Primary
                    && let Some(screen) = screen
                    && (self.crop_press(ui.ctx(), rect, index, screen, m)
                        || self.study_press(rect, index, screen, m))
                {
                    break;
                }
                let hit = match (index, ours, pointer) {
                    (SCULPT, Button::Primary, Some(pos)) if self.playback.is_none() => {
                        self.pick(rect, pos).map(|h| (pos, h))
                    }
                    _ => None,
                };
                let camera = &self.cameras[index];
                let free = (camera.yaw, camera.pitch);
                match (self.nav.action(ours, m, hit.is_some()), hit) {
                    (Some(action), _) => {
                        self.drags[index] = Some(Drag::Navigate {
                            action,
                            button,
                            free,
                        })
                    }
                    (None, Some((pos, hit))) => self.start_stroke(rect, pos, hit),
                    (None, None)
                        if index == SCULPT
                            && ours == Button::Primary
                            && self.playback.is_none()
                            && self.nav.masks(m) =>
                    {
                        if let Some(start) = pointer {
                            self.drags[index] = Some(Drag::MaskTap { start });
                        }
                    }
                    (None, None) if index == REFERENCE && ours == Button::Primary => {
                        self.drags[index] = Some(Drag::Navigate {
                            action: NavAction::Orbit,
                            button,
                            free,
                        });
                    }
                    (None, None) => {}
                }
                break;
            }
        }

        match self.drags[index] {
            Some(Drag::Navigate {
                action,
                button,
                mut free,
            }) if ui.input(|i| i.pointer.button_down(button)) => {
                // The preset may change the action mid-drag (ZBrush style:
                // let go of Alt to scale).
                let action = self.nav.during(action, button_of(button), m);
                let snaps = self.nav.snaps(m);
                let camera = &mut self.cameras[index];
                match action {
                    // While snapping, the turn goes on out of sight and
                    // the camera shows the nearest side; let go and it
                    // turns on from there.
                    NavAction::Orbit if snaps => {
                        let mut turn = *camera;
                        (turn.yaw, turn.pitch) = free;
                        turn.orbit(delta.x, delta.y);
                        free = (turn.yaw, turn.pitch);
                        let (yaw, pitch) = snap_view(turn.yaw, turn.pitch);
                        camera.look_from(yaw, pitch);
                    }
                    NavAction::Orbit => {
                        camera.orbit(delta.x, delta.y);
                        free = (camera.yaw, camera.pitch);
                    }
                    NavAction::Pan => camera.pan(delta.x, delta.y, rect.height()),
                    NavAction::Zoom => camera.dolly((zoom * DOLLY_DRAG).exp()),
                }
                self.drags[index] = Some(Drag::Navigate {
                    action,
                    button,
                    free,
                });
            }
            Some(Drag::Stroke { .. }) if ui.input(|i| i.pointer.primary_down()) => {
                let samples: Vec<_> = self
                    .input
                    .samples
                    .iter()
                    .filter(|s| {
                        matches!(s.phase, PointerPhase::Move | PointerPhase::Down)
                            && s.pressure > 0.0
                    })
                    .map(|s| {
                        let pos = self.view_pos_in(index, rect, egui::pos2(s.pos[0], s.pos[1]));
                        (pos, s.brush_pressure())
                    })
                    .collect();
                for (pos, pressure) in samples {
                    self.stroke_to(rect, pos, pressure);
                }
            }
            Some(Drag::Grab {
                start,
                right,
                up,
                scale,
            }) if ui.input(|i| i.pointer.primary_down()) => {
                if let Some(pos) = pointer {
                    let d = pos - start;
                    self.sculpt.drag((right * d.x - up * d.y) * scale);
                }
            }
            Some(Drag::MaskTap { .. }) if ui.input(|i| i.pointer.primary_down()) => {}
            Some(Drag::MaskTap { start }) => {
                let moved = pointer.is_some_and(|pos| pos.distance(start) > MASK_TAP);
                let op = if moved {
                    LayerOp::ClearMask
                } else {
                    LayerOp::InvertMask
                };
                self.sculpt.edit_layer(op);
                self.drags[index] = None;
            }
            Some(_) => {
                self.sculpt.end_stroke();
                self.drags[index] = None;
            }
            None => {}
        }

        if response.hovered() && !on_toolbar && scroll != 0.0 {
            self.cameras[index].dolly((-scroll * DOLLY_WHEEL).exp());
        }
        if self.cameras[index] != camera_before {
            self.follow_camera(index);
        }
        if index == SCULPT {
            self.sculpt.refresh();
        }
    }

    fn debug_hud(&self, ui: &egui::Ui, rect: Rect) {
        let text = match self.input.last {
            Some(e) => {
                let kind = match e.kind {
                    PointerKind::Mouse => "mouse",
                    PointerKind::Pen if e.eraser => "pen (eraser)",
                    PointerKind::Pen => "pen",
                    PointerKind::Touch => "touch",
                };
                format!("{kind}  pressure {:.3}  {:?}", e.pressure, e.phase)
            }
            None => "no pointer yet".to_owned(),
        };
        ui.painter().text(
            rect.left_bottom() + egui::vec2(14.0, -12.0),
            Align2::LEFT_BOTTOM,
            text,
            fonts::regular(12.0),
            theme::VIEW_INK,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::from_min_size(egui::pos2(200.0, 30.0), egui::vec2(1000.0, 600.0))
    }

    #[test]
    fn swapping_keeps_each_view_its_share() {
        let ([sculpt, reference], x) = layout(area(), 0.3, Layout::Both, false);
        assert_eq!(x, Some(500.0));
        assert_eq!(sculpt.left(), 200.0);
        assert_eq!(sculpt.width(), 300.0);
        assert!(reference.left() > sculpt.right());

        let ([sculpt, reference], x) = layout(area(), 0.3, Layout::Both, true);
        assert_eq!(x, Some(900.0));
        assert_eq!(reference.left(), 200.0);
        assert_eq!(sculpt.right(), 1200.0);
        assert!(
            (sculpt.width() - 299.0).abs() < 1e-3,
            "less the 1px divider"
        );
    }

    #[test]
    fn the_mirror_reflects_a_point_and_back() {
        let rect = area();
        let pos = egui::pos2(260.0, 100.0);
        let mirrored = mirror_x(rect, pos, true);
        assert_eq!(mirrored, egui::pos2(1140.0, 100.0));
        assert_eq!(mirror_x(rect, mirrored, true), pos);
        assert_eq!(mirror_x(rect, rect.center(), true), rect.center());
        assert_eq!(mirror_x(rect, pos, false), pos);
    }

    #[test]
    fn a_single_view_fills_the_area_and_hides_the_other() {
        for swapped in [false, true] {
            let ([sculpt, reference], x) = layout(area(), 0.5, Layout::Sculpt, swapped);
            assert_eq!((sculpt, x), (area(), None));
            assert!(!shown(reference));
            let ([sculpt, reference], _) = layout(area(), 0.5, Layout::Reference, swapped);
            assert_eq!(reference, area());
            assert!(!shown(sculpt));
        }
    }
}
