// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The two views side by side: layout, the divider, pointer input,
//! navigation and viewpoints.

use egui::{Align2, Color32, CursorIcon, Key, PointerButton, Rect, Sense, Stroke};
use glam::Vec3;

use super::{App, Drag, REFERENCE, SCULPT};
use crate::camera::OrbitCamera;
use crate::input::{Button, Modifiers, PointerKind, PointerPhase};
use crate::navigation::NavAction;
use crate::ui::{fonts, i18n::Tr, theme};

/// Navigation speeds: dolly per point dragged, and per point scrolled.
const DOLLY_DRAG: f32 = 0.005;
const DOLLY_WHEEL: f32 = 0.0015;

/// Preset viewpoints: label, key, yaw and pitch. Right and left are the
/// viewer's, as seen in the front view.
pub(super) fn viewpoints(tr: &Tr) -> [(&'static str, Option<Key>, f32, f32); 6] {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};
    [
        (tr.front, Some(Key::Num1), 0.0, 0.0),
        (tr.back, None, PI, 0.0),
        (tr.left, None, -FRAC_PI_2, 0.0),
        (tr.right, Some(Key::Num3), FRAC_PI_2, 0.0),
        (tr.top, Some(Key::Num7), 0.0, FRAC_PI_2),
        (tr.three_quarter, None, FRAC_PI_4, 0.3),
    ]
}

impl App {
    /// Turns both views to the same direction, so they can be compared.
    pub(super) fn view_from(&mut self, yaw: f32, pitch: f32) {
        for camera in &mut self.cameras {
            camera.look_from(yaw, pitch);
        }
    }

    pub(super) fn reset_view(&mut self) {
        self.cameras = [OrbitCamera::default(); 2];
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
    }

    pub(super) fn views_ui(&mut self, ui: &mut egui::Ui) {
        let p = self.theme.palette();
        let tr = self.lang.tr();
        let area = ui.max_rect();

        // The divider between the two views, draggable.
        let x = area.left() + (area.width() * self.split).round();
        let handle = Rect::from_x_y_ranges(x - 4.0..=x + 4.0, area.y_range());
        let handle = ui.interact(handle, ui.scope_id().with("split"), Sense::drag());
        if handle.dragged()
            && let Some(pos) = handle.interact_pointer_pos()
        {
            self.split = ((pos.x - area.left()) / area.width()).clamp(0.2, 0.8);
        }
        if handle.hovered() || handle.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }

        let left = Rect::from_x_y_ranges(area.left()..=x, area.y_range());
        let right = Rect::from_x_y_ranges(x + 1.0..=area.right(), area.y_range());
        self.view_rects = [left, right];
        self.view_input(ui, left, SCULPT);
        self.view_input(ui, right, REFERENCE);

        let painter = ui.painter();
        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        for (view, rect) in self.views.iter().zip([left, right]) {
            if let Some(id) = view.egui_id {
                painter.image(id, rect, uv, Color32::WHITE);
            }
        }
        painter.hline(area.x_range(), area.top() + 0.5, Stroke::new(1.0, p.border));
        painter.vline(x + 0.5, area.y_range(), Stroke::new(1.0, p.border));

        let label_font = fonts::regular(12.0);
        let sculpt_label = if self.playback.is_some() {
            tr.timelapse
        } else {
            tr.sculpt
        };
        for (rect, text) in [(left, sculpt_label), (right, tr.reference)] {
            painter.text(
                rect.left_top() + egui::vec2(14.0, 12.0),
                Align2::LEFT_TOP,
                text,
                label_font.clone(),
                theme::VIEW_TEXT,
            );
        }
        // Empty reference view: say what can go there.
        painter.with_clip_rect(right).text(
            right.center(),
            Align2::CENTER_CENTER,
            tr.drop_hint,
            fonts::light(16.0),
            theme::VIEW_TEXT,
        );

        self.tool_cursor(ui, left);
        if self.debug {
            self.debug_hud(ui, left);
        }
    }

    /// Pointer input in one view: the navigation preset decides what each
    /// press starts; in the sculpt view a left press on the clay that the
    /// preset leaves free sculpts. The reference view has nothing to sculpt,
    /// so a free left drag turns it.
    fn view_input(&mut self, ui: &egui::Ui, rect: Rect, index: usize) {
        let response = ui.interact(
            rect,
            ui.scope_id().with(("view", index)),
            Sense::click_and_drag(),
        );
        let (m, pointer, delta, scroll) = ui.input(|i| {
            let m = Modifiers {
                alt: i.modifiers.alt,
                ctrl: i.modifiers.ctrl,
                shift: i.modifiers.shift,
                command: i.modifiers.command,
            };
            (
                m,
                i.pointer.interact_pos(),
                i.pointer.delta(),
                i.smooth_scroll_delta.y,
            )
        });

        if self.drags[index].is_none() && response.hovered() {
            for (button, ours) in [
                (PointerButton::Primary, Button::Primary),
                (PointerButton::Middle, Button::Middle),
                (PointerButton::Secondary, Button::Secondary),
            ] {
                if !ui.input(|i| i.pointer.button_pressed(button)) {
                    continue;
                }
                let hit = match (index, ours, pointer) {
                    (SCULPT, Button::Primary, Some(pos)) if self.playback.is_none() => {
                        self.pick(rect, pos).map(|h| (pos, h))
                    }
                    _ => None,
                };
                match (self.nav.action(ours, m, hit.is_some()), hit) {
                    (Some(action), _) => {
                        self.drags[index] = Some(Drag::Navigate { action, button })
                    }
                    (None, Some((pos, hit))) => self.start_stroke(rect, pos, hit),
                    (None, None) if index == REFERENCE && ours == Button::Primary => {
                        self.drags[index] = Some(Drag::Navigate {
                            action: NavAction::Orbit,
                            button,
                        });
                    }
                    (None, None) => {}
                }
                break;
            }
        }

        match self.drags[index] {
            Some(Drag::Navigate { action, button })
                if ui.input(|i| i.pointer.button_down(button)) =>
            {
                let camera = &mut self.cameras[index];
                match action {
                    NavAction::Orbit => camera.orbit(delta.x, delta.y),
                    NavAction::Pan => camera.pan(delta.x, delta.y, rect.height()),
                    NavAction::Zoom => camera.dolly((delta.y * DOLLY_DRAG).exp()),
                }
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
                    .map(|s| (egui::pos2(s.pos[0], s.pos[1]), s.pressure))
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
            Some(_) => {
                self.sculpt.end_stroke();
                self.drags[index] = None;
            }
            None => {}
        }

        if response.hovered() && scroll != 0.0 {
            self.cameras[index].dolly((-scroll * DOLLY_WHEEL).exp());
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
