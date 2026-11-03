// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Keyboard shortcuts, and the shortcuts and about windows. Each part of the
//! editor handles its own keys through a hook in its file.

use egui::{Align2, Key, RichText};

use super::sculpting::{BRUSHES, brush_name};
use super::views::viewpoints;
use super::{App, REPO_URL, SITE_URL, WEB};
use crate::navigation::NavStyle;
use crate::sculpt::Brush;
use crate::ui;

impl App {
    pub(super) fn shortcuts(&mut self, ui: &egui::Ui) {
        use egui::Modifiers as M;
        let pressed = |m: M, key: Key| ui.input_mut(|i| i.consume_key(m, key));
        // Typing in a text field (a credit, a title): its letters, Ctrl+Z
        // and Home belong to the field, not to the tools and views.
        if !ui.ctx().text_edit_focused() {
            self.sculpt_shortcuts(ui);
            for (_, key, yaw, pitch) in viewpoints(self.lang.tr()) {
                if key.is_some_and(|key| pressed(M::NONE, key)) {
                    self.view_from(yaw, pitch);
                }
            }
            if pressed(M::NONE, Key::F) {
                self.frame_sculpt();
            }
            if pressed(M::NONE, Key::Home) {
                self.reset_view();
            }
            self.study_shortcuts(ui);
        }
        self.file_shortcuts(ui);
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
                    .map(|&(brush, key, _)| {
                        let keys = if brush == Brush::Smooth {
                            format!("{} / Shift", key.name())
                        } else {
                            key.name().to_owned()
                        };
                        (brush_name(tr, brush), keys)
                    })
                    .collect();
                rows.extend([
                    (tr.invert, self.nav.invert_key().to_owned()),
                    (tr.size, "[  ]".to_owned()),
                    (tr.undo, self.keys("Z")),
                    (tr.redo, self.keys("Shift+Z")),
                ]);
                for (label, key, _, _) in viewpoints(tr) {
                    if let Some(key) = key {
                        rows.push((label, key.name().to_owned()));
                    }
                }
                rows.push((tr.frame_sculpt, "F".to_owned()));
                rows.push((tr.reset_view, "Home".to_owned()));
                for (label, key) in [
                    (tr.link_cameras, "L"),
                    (tr.study.swap_sides, "W"),
                    (tr.mirror_view, "R"),
                    (tr.silhouette, "K"),
                    (tr.study.difference, "Shift+K"),
                    (tr.ghost, "O"),
                    (tr.plumb_line, "V"),
                    (tr.grid, "G"),
                    (tr.study.measure, "D"),
                    (tr.study.lighting, "B"),
                    (tr.mesh.remesh, "Shift+R"),
                    (tr.mesh.subdivide, "Shift+U"),
                ] {
                    rows.push((label, key.to_owned()));
                }
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
    }
}
