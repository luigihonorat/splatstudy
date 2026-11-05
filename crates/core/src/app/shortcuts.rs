// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Keyboard shortcuts, and the shortcuts and about windows. Each part of the
//! editor handles its own keys through a hook in its file.

use egui::{Align2, Key, RichText};

use super::sculpting::{BRUSHES, TOOL_PIE_KEY, brush_keys, brush_name};
use super::toolbar::ViewOption;
use super::views::{numpad_name, viewpoints};
use super::{App, REPO_URL, SITE_URL, WEB};
use crate::navigation::NavStyle;
use crate::ui;

impl App {
    pub(super) fn shortcuts(&mut self, ui: &egui::Ui) {
        use egui::Modifiers as M;
        let pressed = |m: M, key: Key| ui.input_mut(|i| i.consume_key(m, key));
        let numpad = std::mem::take(&mut self.numpad);
        // Typing in a text field (a credit, a title): its letters, Ctrl+Z
        // and Home belong to the field, not to the tools and views.
        if !ui.ctx().text_edit_focused() {
            self.flyout_keys(ui);
            self.sculpt_shortcuts(ui);
            for digit in numpad {
                self.numpad_view(digit);
            }
            // In the view under the pointer.
            if pressed(M::NONE, Key::F) {
                self.apply(ViewOption::Frame, self.key_view(ui));
            }
            if pressed(M::NONE, Key::Home) {
                self.apply(ViewOption::Reset, self.key_view(ui));
            }
            self.study_shortcuts(ui);
        }
        self.file_shortcuts(ui);
        self.screenshot_shortcut(ui);
        if !WEB && pressed(M::COMMAND, Key::Q) {
            self.quit = true;
        }
        self.sculpt.refresh();
    }

    /// A navigation preset as the menus name it.
    pub(super) fn nav_name(&self, style: NavStyle) -> &'static str {
        let tr = self.lang.tr();
        match style {
            NavStyle::ZBrush => tr.nav_zbrush,
            NavStyle::Blender => tr.nav_blender,
            NavStyle::Maya => tr.nav_maya,
        }
    }

    pub(super) fn nav_hint(&self) -> &'static str {
        let tr = self.lang.tr();
        match self.nav {
            NavStyle::ZBrush => tr.hint_zbrush,
            NavStyle::Blender => tr.hint_blender,
            NavStyle::Maya => tr.hint_maya,
        }
    }

    /// The windows: keyboard shortcuts and about, opened from the help menu,
    /// then each part's own.
    pub(super) fn windows(&mut self, ui: &egui::Ui) {
        let tr = self.lang.tr();
        // Wide enough that the centred title isn't cut short.
        let window = |title: &'static str, width: f32| {
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .min_width(width)
                .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
        };

        let mut open = self.show_shortcuts;
        let room = ui.ctx().content_rect().height() - 120.0;
        window(tr.shortcuts, 420.0)
            .vscroll(true)
            .default_height(room.max(120.0))
            .max_height(room.max(120.0))
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                let muted = ui.visuals().widgets.inactive.fg_stroke.color;
                let mut rows: Vec<(&str, String)> = BRUSHES
                    .iter()
                    .map(|&(brush, _, _)| (brush_name(tr, brush), brush_keys(brush)))
                    .collect();
                rows.extend([
                    (tr.mesh.tool_pie, TOOL_PIE_KEY.name().to_owned()),
                    (tr.invert, self.nav.invert_key().to_owned()),
                    (tr.size, "[  ]".to_owned()),
                    (tr.undo, self.keys("Z")),
                    (tr.redo, self.keys("Shift+Z")),
                ]);
                for (label, key, _, _) in viewpoints(tr) {
                    if let Some(key) = key {
                        rows.push((label, numpad_name(key)));
                    }
                }
                rows.push((tr.study.hide_panels, "Tab".to_owned()));
                rows.push((tr.study.hide_panels_and_reference, "Shift+Tab".to_owned()));
                rows.push((tr.frame_sculpt, "F".to_owned()));
                rows.push((tr.reset_view, "Home".to_owned()));
                for (label, key) in [
                    (tr.link_cameras, "L"),
                    (tr.study.swap_sides, "W"),
                    (tr.mirror_view, "R"),
                    (tr.silhouette, "K"),
                    (tr.study.difference, "Shift+K"),
                    (tr.ghost, "O"),
                    (tr.study.wireframe, "Shift+W"),
                    (tr.plumb_line, "V"),
                    (tr.grid, "G"),
                    (tr.study.measure, "E"),
                    (tr.study.lighting, "B"),
                    (tr.mesh.remesh, "Shift+R"),
                    (tr.mesh.quad_remesh, "Shift+Q"),
                    (tr.mesh.subdivide, "Shift+U"),
                    (tr.mesh.level_down, "Shift+D / PgDn"),
                    (tr.mesh.level_up, "D / PgUp"),
                ] {
                    rows.push((label, key.to_owned()));
                }
                rows.push((tr.video.screenshot, self.keys("P")));
                rows.push((tr.mesh.invert_mask, self.keys("I")));
                rows.push((tr.mesh.clear_mask, "Alt+M".to_owned()));
                rows.push((tr.open_scene, self.keys("O")));
                rows.push((tr.save_scene, self.keys("S")));
                if !WEB {
                    rows.push((tr.save_scene_as, self.keys("Shift+S")));
                    rows.push((tr.new_scene, self.keys("N")));
                    rows.push((tr.exit, self.keys("Q")));
                }
                egui::Grid::new("shortcuts")
                    .num_columns(2)
                    .spacing([32.0, 6.0])
                    .show(ui, |ui| {
                        for (label, keys) in rows {
                            ui.label(label);
                            ui.label(RichText::new(keys).color(muted));
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
                ui.label(RichText::new(tr.study.panels_note).color(muted));
                ui::section(ui, tr.navigation);
                ui.label(RichText::new(self.nav_hint()).color(muted));
            });
        self.show_shortcuts = open;

        let mut open = self.show_about;
        window(tr.about, 340.0)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.set_max_width(320.0);
                let muted = ui.visuals().widgets.inactive.fg_stroke.color;
                ui.label(tr.about_text);
                ui.label(
                    RichText::new(format!(
                        "SplatStudy {} · GPL-3.0-or-later",
                        env!("CARGO_PKG_VERSION")
                    ))
                    .color(muted),
                );
                ui.label(RichText::new(tr.trademarks).color(muted).small());
                for (text, url) in [
                    ("splatstudy.com", SITE_URL),
                    ("github.com/luigihonorat/splatstudy", REPO_URL),
                ] {
                    if ui::choice(ui, text, false).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    }
                }
            });
        self.show_about = open;

        self.help_windows(ui);
        self.export_window(ui);
        self.screenshot_step(ui);
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, PI};

    use super::super::study::tests::Harness;
    use super::super::{REFERENCE, SCULPT};
    use super::*;
    use crate::sculpt::Brush;

    #[test]
    fn the_digit_row_and_the_letters_pick_the_tools() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let digits = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
        ];
        for (&(brush, letter, _), digit) in BRUSHES.iter().zip(digits) {
            h.key(digit, false);
            assert_eq!(h.app.tool.tool, brush, "{digit:?}");
            h.key(Key::Num1, false);
            h.key(letter, false);
            assert_eq!(h.app.tool.tool, brush, "{letter:?}");
        }
        assert_eq!(BRUSHES[0].0, Brush::Move);
        assert_eq!(BRUSHES[7].0, Brush::Mask);
        // 5 picks a tool now, not the orthographic view.
        assert!(!h.app.cameras[SCULPT].ortho);
        assert_eq!(brush_keys(Brush::Move), "1 / M");
        assert_eq!(brush_keys(Brush::Smooth), "4 / S / Shift");
    }

    #[test]
    fn the_numpad_turns_both_views_and_picks_no_tool() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let tool = h.app.tool.tool;
        let looks = |h: &Harness| {
            let camera = h.app.cameras[SCULPT];
            assert_eq!(
                (camera.yaw, camera.pitch),
                (h.app.cameras[REFERENCE].yaw, h.app.cameras[REFERENCE].pitch),
                "both views turn"
            );
            (camera.yaw, camera.pitch)
        };
        h.numpad(8);
        let (yaw, pitch) = looks(&h);
        assert!(yaw == 0.0 && pitch > 1.5, "top");
        h.numpad(2);
        let (yaw, pitch) = looks(&h);
        assert!(yaw == 0.0 && pitch < -1.5, "bottom");
        h.numpad(4);
        assert_eq!(looks(&h), (-FRAC_PI_2, 0.0), "left");
        h.numpad(6);
        assert_eq!(looks(&h), (FRAC_PI_2, 0.0), "right");
        // 5 turns to the front, then from there to the back and back.
        h.numpad(5);
        assert_eq!(looks(&h), (0.0, 0.0), "front");
        h.numpad(5);
        assert_eq!(looks(&h), (PI, 0.0), "back");
        h.numpad(5);
        assert_eq!(looks(&h), (0.0, 0.0), "front again");
        // A front reached by turning a full circle is still the front.
        h.app.cameras[SCULPT].yaw = std::f32::consts::TAU + 0.001;
        h.numpad(5);
        assert_eq!(looks(&h), (PI, 0.0));
        // Nearly the front is not the front.
        h.app.cameras[SCULPT].yaw = 0.2;
        h.numpad(5);
        assert_eq!(looks(&h), (0.0, 0.0));
        // Digits without a view do nothing; none picks a tool.
        for digit in [0, 1, 3, 7, 9] {
            h.numpad(digit);
            assert_eq!(looks(&h), (0.0, 0.0));
        }
        assert_eq!(h.app.tool.tool, tool);
    }
}
