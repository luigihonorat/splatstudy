// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Subdivision levels in the sculpt panel, the edit menu and the keys:
//! going a level down to change the broad forms, and up again to the
//! detail (see `sculpt/levels.rs`).

use egui::Key;

use super::Pending;
use crate::app::App;
use crate::ui::{self, menu};

impl App {
    /// "level 2 / 4", counting from 1, or `None` without levels.
    fn level_label(&self) -> Option<String> {
        let (current, count) = self.sculpt.levels()?;
        let m = self.lang.tr().mesh;
        Some(format!("{} {} / {count}", m.level, current + 1))
    }

    /// Whether the clay can go a level down and a level up.
    fn level_moves(&self) -> (bool, bool) {
        let live = self.playback.is_none();
        self.sculpt
            .levels()
            .map_or((false, false), |(current, count)| {
                (live && current > 0, live && current + 1 < count)
            })
    }

    /// Goes a level up or down at the next frame: a fine level takes a
    /// moment to build.
    pub(super) fn ask_level(&mut self, up: bool) {
        let (down_ok, up_ok) = self.level_moves();
        if (up && up_ok) || (!up && down_ok) {
            let m = self.lang.tr().mesh;
            self.ask(
                Pending::Level(up),
                if up { m.level_up } else { m.level_down },
            );
        }
    }

    /// Goes a level up or down now, and says which level the clay is at.
    pub(super) fn change_level(&mut self, up: bool) {
        if self.sculpt.change_level(up) {
            self.sync_clay();
            if let Some(label) = self.level_label() {
                self.notify(label);
            }
        }
    }

    fn delete_levels(&mut self, higher: bool) {
        if self.playback.is_none() && self.sculpt.delete_levels(higher) {
            let label = self.level_label();
            self.notify(label.unwrap_or_else(|| self.lang.tr().mesh.mesh.to_owned()));
        }
    }

    /// As in ZBrush, D goes a level up and Shift+D a level down; Page Up
    /// and Page Down too.
    pub(super) fn level_shortcuts(&mut self, ui: &egui::Ui) {
        use egui::Modifiers as M;
        let pressed = |m: M, key: Key| ui.input_mut(|i| i.consume_key(m, key));
        if self.sculpt.levels().is_none() {
            return;
        }
        // Shift+D first: plain D would take it too.
        if pressed(M::SHIFT, Key::D) || pressed(M::NONE, Key::PageDown) {
            self.ask_level(false);
        }
        if pressed(M::NONE, Key::D) || pressed(M::NONE, Key::PageUp) {
            self.ask_level(true);
        }
    }

    /// Level items in the edit menu, after "subdivide".
    pub(super) fn level_items(&mut self, ui: &mut egui::Ui) {
        let Some(label) = self.level_label() else {
            return;
        };
        let m = self.lang.tr().mesh;
        let (down, up) = self.level_moves();
        let level_down = format!("{} ({label})", m.level_down);
        if menu::action(ui, down, &level_down, "Shift+D") {
            self.ask_level(false);
        }
        if menu::action(ui, up, m.level_up, "D") {
            self.ask_level(true);
        }
        if menu::action(ui, up, m.delete_higher, "") {
            self.delete_levels(true);
        }
        if menu::action(ui, down, m.delete_lower, "") {
            self.delete_levels(false);
        }
    }

    /// The levels under "subdivide" in the sculpt panel: where the clay
    /// is, the way down and up, and deleting the others.
    pub(super) fn levels_panel(&mut self, ui: &mut egui::Ui) {
        let Some(label) = self.level_label() else {
            return;
        };
        let m = self.lang.tr().mesh;
        let (down, up) = self.level_moves();
        let muted = ui.visuals().widgets.inactive.fg_stroke.color;
        ui.label(egui::RichText::new(label).color(muted));
        let clicked = |ui: &mut egui::Ui, enabled: bool, text: &str| {
            ui.add_enabled_ui(enabled, |ui| ui::choice(ui, text, false))
                .inner
                .clicked()
        };
        // One per line: side by side they overflow the panel in Japanese.
        if clicked(ui, down, m.level_down) {
            self.ask_level(false);
        }
        if clicked(ui, up, m.level_up) {
            self.ask_level(true);
        }
        if clicked(ui, up, m.delete_higher) {
            self.delete_levels(true);
        }
        if clicked(ui, down, m.delete_lower) {
            self.delete_levels(false);
        }
    }
}
