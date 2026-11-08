// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The tweak group in the sculpt panel: four buttons that each apply one
//! step of an operation to the whole clay (see [`crate::sculpt::tweak`]),
//! in opposite pairs: smooth and sharpen, shrink and inflate. Each click
//! is one undo step and plays back in the timelapse.

use super::App;
use super::panels::Group;
use crate::sculpt::Tweak;
use crate::ui;

impl App {
    /// The tweak group, if the window menu left it in.
    pub(super) fn tweak_group(&mut self, ui: &mut egui::Ui) {
        if !self.panels.holds(Group::Tweak) {
            return;
        }
        let title = self.lang.tr().tweak.tweak;
        ui::group(ui, "tweak", title, |ui| self.tweak_buttons(ui));
    }

    /// Smooth over shrink on the left, sharpen over inflate on the right.
    fn tweak_buttons(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr().tweak;
        // Not in the middle of a stroke (see `sculpt_drag_open`).
        let live = self.playback.is_none() && !self.sculpt_drag_open();
        let columns = [
            [(Tweak::Smooth, tr.smooth), (Tweak::Shrink, tr.shrink)],
            [(Tweak::Sharpen, tr.sharpen), (Tweak::Inflate, tr.inflate)],
        ];
        let mut clicked = None;
        ui.add_enabled_ui(live, |ui| {
            ui.columns(2, |uis| {
                for (ui, column) in uis.iter_mut().zip(columns) {
                    for (op, label) in column {
                        if ui::choice(ui, label, false).clicked() {
                            clicked = Some(op);
                        }
                    }
                }
            });
        });
        if let Some(op) = clicked {
            self.tweak(op);
        }
    }

    /// One step of `op` over the whole clay, as one undo step.
    fn tweak(&mut self, op: Tweak) {
        if self.playback.is_some() || self.sculpt_drag_open() {
            return;
        }
        if self.sculpt.tweak(op) {
            self.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect};

    use super::super::PANEL_WIDTH;
    use super::super::study::tests::Harness;
    use super::*;
    use crate::sculpt::{Event, Replay};

    /// The tweak buttons, row by row: smooth, sharpen, shrink, inflate.
    fn buttons(h: &Harness) -> Vec<Rect> {
        let mut rects: Vec<Rect> = h.app.ctx.viewport(|v| {
            v.this_pass
                .widgets
                .layers()
                .flat_map(|(_, widgets)| widgets.iter())
                .filter(|w| w.sense.senses_click() && w.rect.right() < PANEL_WIDTH)
                // Not the menu bar above the panel, the group's heading,
                // which runs the panel's width, nor the panel's scroll bar.
                .filter(|w| w.rect.top() > 30.0)
                .filter(|w| (10.0..PANEL_WIDTH * 0.5).contains(&w.rect.width()))
                .map(|w| w.rect)
                .collect()
        });
        rects.sort_by(|a, b| {
            (a.top(), a.left())
                .partial_cmp(&(b.top(), b.left()))
                .unwrap()
        });
        rects
    }

    fn click(h: &mut Harness, at: Pos2) {
        use crate::PointerPhase::{Down, Move, Up};
        h.pointer(Move, at);
        h.pointer(Down, at);
        h.pointer(Up, at);
        h.frames(1);
    }

    #[test]
    fn each_button_tweaks_the_clay_once_and_undo_and_the_timelapse_follow() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        // The tweak group alone in the sculpt panel, at its top.
        for group in Group::SCULPT {
            if group != Group::Tweak {
                h.app.panels.toggle_group(group);
            }
        }
        // What the widgets were is known a frame later.
        h.frames(2);
        let rects = buttons(&h);
        assert_eq!(rects.len(), 4, "{rects:?}");
        let order = [Tweak::Smooth, Tweak::Sharpen, Tweak::Shrink, Tweak::Inflate];
        for (rect, op) in rects.iter().zip(order) {
            let before = h.app.sculpt.mesh.positions.clone();
            let log = h.app.sculpt.log().len();
            click(&mut h, rect.center());
            assert_ne!(h.app.sculpt.mesh.positions, before, "{op:?} did nothing");
            let tweaks: Vec<Event> = h.app.sculpt.log()[log..]
                .iter()
                .map(|e| e.event)
                .filter(|e| matches!(e, Event::Tweak(_)))
                .collect();
            assert_eq!(tweaks, [Event::Tweak(op)], "one step of {op:?}");
            let after = h.app.sculpt.mesh.positions.clone();
            h.app.sculpt.undo();
            assert_eq!(h.app.sculpt.mesh.positions, before, "{op:?} undone");
            h.app.sculpt.redo();
            assert_eq!(h.app.sculpt.mesh.positions, after, "{op:?} redone");
            h.frames(1);
        }

        // The timelapse plays the session back to the same clay.
        let mut replay = Replay::of(&h.app.sculpt);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_eq!(replay.sculpt.mesh.positions, h.app.sculpt.mesh.positions);
        assert_eq!(replay.sculpt.mesh.normals, h.app.sculpt.mesh.normals);

        // The window menu takes the group out, and the preferences keep it.
        h.app.panels.toggle_group(Group::Tweak);
        assert!(h.app.preferences().contains("tweak"));
        h.frames(2);
        // The panel went with its last group; the view's own toolbar is
        // there now.
        assert!(buttons(&h).iter().all(|r| !rects.contains(r)));
    }
}
