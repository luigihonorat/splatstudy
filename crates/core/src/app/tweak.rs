// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The tweak group in the sculpt panel: four buttons that each apply one
//! step of an operation to the whole clay, in opposite pairs: smooth and
//! sharpen, shrink and inflate. Each click is one undo step and plays back
//! in the timelapse.
//!
//! In voxel mode (the default) a click works on the clay's signed
//! distance field and meshes it again at the group's own resolution (see
//! [`crate::voxel::tweak`]), like a remesh: a checkpoint, after a window
//! saying so, and the subdivision levels go. However often it is used,
//! the triangles stay as even as a fresh remesh. Out of voxel mode a click
//! moves the clay's vertices (see [`crate::sculpt::tweak`]): instant, and
//! the levels stay, but used over and over it wears the triangles thin.

use super::App;
use super::panels::Group;
use super::sculpting::{Pending, resolution_slider, switch};
use crate::sculpt::Tweak;
use crate::ui;
use crate::voxel;

impl App {
    /// The tweak group, if the window menu left it in.
    pub(super) fn tweak_group(&mut self, ui: &mut egui::Ui) {
        if !self.panels.holds(Group::Tweak) {
            return;
        }
        let title = self.lang.tr().tweak.tweak;
        ui::group(ui, "tweak", title, |ui| self.tweak_controls(ui));
    }

    /// Smooth over shrink on the left, sharpen over inflate on the right;
    /// then the voxel mode, and its resolution while it is on.
    fn tweak_controls(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        let t = tr.tweak;
        // Not in the middle of a stroke (see `sculpt_drag_open`).
        let live = self.playback.is_none() && !self.sculpt_drag_open();
        let columns = [
            [(Tweak::Smooth, t.smooth), (Tweak::Shrink, t.shrink)],
            [(Tweak::Sharpen, t.sharpen), (Tweak::Inflate, t.inflate)],
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
        switch(ui, tr, &mut self.tool.voxel_tweaks, t.voxel_mode);
        if self.tool.voxel_tweaks {
            let max = voxel::MAX_RESOLUTION;
            resolution_slider(ui, tr, &mut self.tool.tweak_resolution, max);
        }
        if let Some(op) = clicked {
            self.tweak(op);
        }
    }

    /// One step of `op` over the whole clay, as one undo step: in voxel
    /// mode at the next frame but one, after a window saying so (see
    /// [`App::ask`]); otherwise at once.
    fn tweak(&mut self, op: Tweak) {
        if self.playback.is_some() || self.sculpt_drag_open() {
            return;
        }
        if self.tool.voxel_tweaks {
            // All masked: nothing would move.
            if voxel::tweak::can_tweak(&self.sculpt.mesh) {
                self.ask(Pending::Tweak(op), self.lang.tr().tweak.working);
            }
        } else if self.sculpt.tweak(op) {
            self.dirty = true;
        }
    }

    /// One step of `op` in voxel mode: the clay meshed again from its
    /// tweaked field, with its mask and chalk, in place of the old as a
    /// remesh would put it.
    pub(super) fn voxel_tweak(&mut self, op: Tweak) {
        if !voxel::tweak::can_tweak(&self.sculpt.mesh) {
            return;
        }
        let resolution = self.tool.tweak_resolution.round() as u32;
        match voxel::tweak::tweaked(&self.sculpt.mesh, op, resolution) {
            Some(mut mesh) => {
                mesh.layers = self.sculpt.layers_for(&mesh);
                self.replace_clay(mesh);
            }
            None => self.notify(self.lang.tr().mesh.nothing_left),
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

    /// The tweak buttons, row by row: smooth, sharpen, shrink, inflate;
    /// then the voxel mode's on and off (and its label, which takes clicks
    /// to select its text).
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

    /// The tweak group alone in the sculpt panel, at its top.
    fn tweak_group_alone(h: &mut Harness) {
        for group in Group::SCULPT {
            if group != Group::Tweak {
                h.app.panels.toggle_group(group);
            }
        }
        // What the widgets were is known a frame later.
        h.frames(2);
    }

    #[test]
    fn each_button_tweaks_the_clay_once_and_undo_and_the_timelapse_follow() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        assert!(h.app.tool.voxel_tweaks, "voxel mode starts on");
        tweak_group_alone(&mut h);
        // The voxel mode's off.
        let rects = buttons(&h);
        assert!(rects.len() >= 6, "{rects:?}");
        click(&mut h, rects[5].center());
        assert!(!h.app.tool.voxel_tweaks);
        h.frames(1);
        let rects = buttons(&h)[..4].to_vec();
        let order = [Tweak::Smooth, Tweak::Sharpen, Tweak::Shrink, Tweak::Inflate];
        for (rect, op) in rects.iter().zip(order) {
            let before = h.app.sculpt.mesh.positions.clone();
            let log = h.app.sculpt.log().len();
            click(&mut h, rect.center());
            assert!(!h.app.busy(), "{op:?} at once, without a window");
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

    #[test]
    fn in_voxel_mode_each_button_meshes_the_clay_again_after_its_window() {
        use crate::PointerPhase::{Down, Move, Up};
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        h.app.tool.tweak_resolution = 48.0;
        tweak_group_alone(&mut h);
        let rects = buttons(&h);
        assert!(rects.len() >= 6, "the four, then on and off: {rects:?}");
        let window = egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("working"));
        let shown = |h: &Harness| h.app.ctx.memory(|m| m.areas().visible_last_frame(&window));
        let order = [Tweak::Smooth, Tweak::Sharpen, Tweak::Shrink, Tweak::Inflate];
        for (rect, op) in rects.iter().zip(order) {
            let clay = |h: &Harness| {
                let mesh = &h.app.sculpt.mesh;
                (mesh.positions.clone(), mesh.triangles.clone())
            };
            let before = clay(&h);
            let (log, checkpoints) = (h.app.sculpt.log().len(), h.app.sculpt.checkpoints().len());
            for phase in [Move, Down, Up] {
                h.pointer(phase, rect.center());
            }
            // The window shows before the clay changes.
            let mut frames_shown = 0;
            for _ in 0..4 {
                if clay(&h) != before {
                    break;
                }
                frames_shown += usize::from(shown(&h));
                h.frames(1);
            }
            assert!(frames_shown >= 1, "{op:?}: the window never showed first");
            let after = clay(&h);
            assert_ne!(after, before, "{op:?} did nothing");
            assert!(!h.app.busy());
            // A checkpoint in the log, not a tweak: replay doesn't work it
            // out again.
            let events: Vec<Event> = h.app.sculpt.log()[log..].iter().map(|e| e.event).collect();
            assert!(
                !events.iter().any(|e| matches!(e, Event::Tweak(_))),
                "{events:?}"
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(e, Event::Checkpoint(_)))
                    .count(),
                1,
                "{op:?}: {events:?}"
            );
            assert_eq!(h.app.sculpt.checkpoints().len(), checkpoints + 1);
            h.app.sculpt.undo();
            assert!(clay(&h) == before, "{op:?} undone");
            h.app.sculpt.redo();
            assert!(clay(&h) == after, "{op:?} redone");
            h.frames(1);
        }

        // The timelapse plays the session back to the same clay.
        let mut replay = Replay::of(&h.app.sculpt);
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_eq!(replay.sculpt.mesh.triangles, h.app.sculpt.mesh.triangles);
        assert_eq!(replay.sculpt.mesh.positions, h.app.sculpt.mesh.positions);
        assert_eq!(replay.sculpt.mesh.normals, h.app.sculpt.mesh.normals);
    }

    #[test]
    fn the_resolution_shows_only_in_voxel_mode() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        tweak_group_alone(&mut h);
        let draggable = |h: &Harness| {
            h.app.ctx.viewport(|v| {
                v.this_pass
                    .widgets
                    .layers()
                    .flat_map(|(_, widgets)| widgets.iter())
                    .filter(|w| w.rect.right() < PANEL_WIDTH && w.rect.top() > 30.0)
                    .filter(|w| w.sense.senses_drag())
                    .count()
            })
        };
        assert_eq!(
            h.app.tool.tweak_resolution,
            crate::voxel::DEFAULT_RESOLUTION as f32
        );
        let on = draggable(&h);
        let off = buttons(&h)[5];
        click(&mut h, off.center());
        assert!(!h.app.tool.voxel_tweaks);
        h.frames(2);
        // The slider and the field that types its value.
        assert!(draggable(&h) < on, "voxel mode is off: no resolution");
    }
}
