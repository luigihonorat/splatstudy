// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Scenes: new, open, save, and importing files.

use std::sync::Arc;

use egui::Key;

use super::reference::REFERENCE_EXTENSIONS;
use super::sculpting::start_mesh;
use super::{App, WEB};
use crate::input::FilePurpose;
use crate::render::clay::GpuMesh;
use crate::sculpt::Sculpt;
use crate::ui::menu;

/// Scene files' extension.
pub(super) const SCENE_EXTENSION: &str = "splatstudy";

/// The open scene's file.
#[derive(Default)]
pub(super) struct FileState {}

/// A file name's extension, lowercase, without the dot.
fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// What a dropped file is for, going by its extension.
fn purpose_of(name: &str) -> FilePurpose {
    let ext = extension(name);
    if ext == SCENE_EXTENSION {
        FilePurpose::Scene
    } else if REFERENCE_EXTENSIONS.contains(&ext.as_str()) {
        FilePurpose::Reference
    } else {
        FilePurpose::Any
    }
}

impl App {
    /// A file arrived from the shell: dropped, picked or fetched.
    pub(super) fn on_file(&mut self, purpose: FilePurpose, name: &str, bytes: Arc<[u8]>) {
        let purpose = match purpose {
            FilePurpose::Any => purpose_of(name),
            purpose => purpose,
        };
        match purpose {
            FilePurpose::Scene => self.open_scene(name, bytes),
            FilePurpose::Reference => self.load_reference(name, bytes),
            FilePurpose::Sculpt => self.import_sculpt(name, bytes),
            FilePurpose::Any => self.cannot_open(name),
        }
    }

    /// Tells the user a file couldn't be opened.
    pub(super) fn cannot_open(&mut self, name: &str) {
        let tr = self.lang.tr();
        self.notify(format!("{name} — {}", tr.files.cannot_open));
    }

    /// Opens a scene file. Not written yet.
    fn open_scene(&mut self, name: &str, _bytes: Arc<[u8]>) {
        self.cannot_open(name);
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_files_go_by_extension() {
        assert_eq!(purpose_of("bust.PLY"), FilePurpose::Reference);
        assert_eq!(purpose_of("cast.spz"), FilePurpose::Reference);
        assert_eq!(purpose_of("photo.jpeg"), FilePurpose::Reference);
        assert_eq!(purpose_of("study.splatstudy"), FilePurpose::Scene);
        assert_eq!(purpose_of("notes.txt"), FilePurpose::Any);
        assert_eq!(purpose_of("no extension"), FilePurpose::Any);
    }
}
