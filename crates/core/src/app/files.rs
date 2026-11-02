// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Scenes: new, open, save, and importing files.

use egui::Key;

use super::sculpting::start_mesh;
use super::{App, WEB};
use crate::render::clay::GpuMesh;
use crate::sculpt::Sculpt;
use crate::ui::menu;

/// The open scene's file.
#[derive(Default)]
pub(super) struct FileState {}

impl App {
    /// Scene items at the top of the file menu.
    pub(super) fn scene_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        // Browsers keep Ctrl+N for a new window.
        let new_keys = if WEB { String::new() } else { self.keys("N") };
        if menu::action(ui, true, tr.new_scene, &new_keys) {
            self.new_scene();
        }
        menu::action(ui, false, tr.open_scene, "");
        if !WEB {
            menu::action(ui, false, tr.open_recent, "");
        }
        menu::action(ui, false, tr.save_scene, "");
        menu::action(ui, false, tr.save_scene_as, "");
        menu::action(ui, false, tr.revert, "");
    }

    /// Import items in the file menu.
    pub(super) fn import_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        menu::action(ui, false, tr.import_reference, "");
        menu::action(ui, false, tr.import_sculpt, "");
    }

    pub(super) fn file_shortcuts(&mut self, ui: &egui::Ui) {
        use egui::Modifiers as M;
        if !WEB && ui.input_mut(|i| i.consume_key(M::COMMAND, Key::N)) {
            self.new_scene();
        }
    }

    /// A fresh sphere seen from the default view.
    pub(super) fn new_scene(&mut self) {
        self.playback = None;
        self.sculpt = Sculpt::new(start_mesh());
        self.gpu_mesh = GpuMesh::new(&self.device, &self.sculpt.mesh);
        self.reset_view();
    }
}
