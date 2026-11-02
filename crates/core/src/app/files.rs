// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Scenes: new, open, save, and importing files.

use std::sync::Arc;

use egui::Key;

use super::reference::REFERENCE_EXTENSIONS;
use super::sculpting::start_mesh;
use super::{App, REFERENCE, Request, SCULPT, WEB};
use crate::input::FilePurpose;
use crate::reference::Reference;
use crate::render::clay::GpuMesh;
use crate::scene::{self, SavedReference, Scene, SceneWriter, View};
use crate::sculpt::{Mesh, Sculpt};
use crate::ui::menu;

/// Scene files' extension.
pub(super) const SCENE_EXTENSION: &str = "splatstudy";
/// Sculpts that can be imported (lowercase, no dot).
const SCULPT_EXTENSIONS: &[&str] = &["obj", "stl"];
/// Scenes download as plain bytes; no browser knows the type.
const SCENE_MIME: &str = "application/octet-stream";

/// The open scene's file.
#[derive(Default)]
pub(super) struct FileState {
    /// The file name it was opened or last saved as.
    name: Option<String>,
    /// The file as last opened or saved, for revert.
    saved: Option<Arc<[u8]>>,
    /// The mesh the session started from, kept so saving needn't build the
    /// start sphere again. Filled on the first save or by opening a scene.
    start: Option<Mesh>,
}

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

    /// Opens a scene file: the clay with its recorded session, the views
    /// and the reference. A file that doesn't read leaves everything as it
    /// was.
    fn open_scene(&mut self, name: &str, bytes: Arc<[u8]>) {
        let scene = match Scene::read(&bytes) {
            Ok(scene) => scene,
            Err(err) => {
                log::warn!("{name}: {err}");
                return self.cannot_open(name);
            }
        };
        let Scene {
            mesh,
            checkpoints,
            log,
            reference,
            view,
        } = scene;
        let start = checkpoints
            .into_iter()
            .next()
            .filter(|c| c.log_index == 0)
            .map_or_else(start_mesh, |c| c.mesh);

        self.cancel_export();
        self.playback = None;
        self.drags = [None; 2];
        let (sculpt, complete) = scene::resume_sculpt(start.clone(), mesh, &log);
        self.sculpt = sculpt;
        self.gpu_mesh = GpuMesh::new(&self.device, &self.sculpt.mesh);
        if let Some(view) = view {
            self.cameras = view.cameras;
            self.split = view.split;
        }
        if let Some(reference) = reference {
            self.restore_reference(reference);
        }
        self.files = FileState {
            name: Some(name.to_owned()),
            saved: Some(bytes),
            start: Some(start),
        };
        if !complete {
            self.notify(self.lang.tr().files.timelapse_lost);
        }
    }

    /// The scene as a file: the clay, where the session started, the
    /// session, the reference and the views.
    fn scene_bytes(&mut self) -> Vec<u8> {
        let start = self.files.start.get_or_insert_with(start_mesh);
        let mut w = SceneWriter::new();
        w.mesh(&self.sculpt.mesh)
            .checkpoints(&[(0, 0.0, start)])
            .log(self.sculpt.log());
        if let Some(reference) = self.scene_reference() {
            w.reference(reference);
        }
        w.view(&View {
            cameras: [self.cameras[SCULPT], self.cameras[REFERENCE]],
            split: self.split,
        });
        w.finish()
    }

    /// Saves the scene: a save dialog on desktop, a download on the web.
    fn save_scene(&mut self) {
        let bytes = self.scene_bytes();
        let name = self.files.name.clone().unwrap_or_else(|| {
            let tr = self.lang.tr();
            format!("{}.{SCENE_EXTENSION}", tr.files.default_name)
        });
        self.files.name = Some(name.clone());
        self.files.saved = Some(Arc::from(&bytes[..]));
        self.request(Request::SaveFile {
            name,
            mime: SCENE_MIME,
            bytes,
        });
    }

    /// The mesh the recorded session starts from, for replaying it.
    pub(super) fn session_start(&mut self) -> Mesh {
        self.files.start.get_or_insert_with(start_mesh).clone()
    }

    /// The scene's file name without its extension, for naming exports.
    pub(super) fn scene_stem(&self) -> String {
        let name = self
            .files
            .name
            .as_deref()
            .unwrap_or(self.lang.tr().files.default_name);
        name.strip_suffix(&format!(".{SCENE_EXTENSION}"))
            .unwrap_or(name)
            .to_owned()
    }

    /// Opens the file as it was last opened or saved.
    fn revert_scene(&mut self) {
        if let (Some(name), Some(bytes)) = (self.files.name.clone(), self.files.saved.clone()) {
            self.open_scene(&name, bytes);
        }
    }

    fn ask_for_scene(&mut self) {
        self.request(Request::OpenFile {
            purpose: FilePurpose::Scene,
            extensions: &[SCENE_EXTENSION],
        });
    }

    // INTEGRATION (references): these two are where a scene meets the
    // reference part. Until it keeps the loaded `Reference`, nothing is
    // saved, and a saved reference is opened like a dropped file, without
    // its placement and credit.

    /// The loaded reference, to save in the scene and credit in exports.
    pub(super) fn scene_reference(&self) -> Option<&Reference> {
        None
    }

    /// Puts a scene's reference back: loads its file again, then restores
    /// the placement and credit it was saved with.
    fn restore_reference(&mut self, saved: SavedReference) {
        self.load_reference(&saved.source.name, saved.source.bytes);
    }

    /// Scene items at the top of the file menu.
    pub(super) fn scene_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        // Browsers keep Ctrl+N for a new window.
        let new_keys = if WEB { String::new() } else { self.keys("N") };
        if menu::action(ui, true, tr.new_scene, &new_keys) {
            self.new_scene();
        }
        if menu::action(ui, true, tr.open_scene, &self.keys("O")) {
            self.ask_for_scene();
        }
        if !WEB {
            menu::action(ui, false, tr.open_recent, "");
        }
        if menu::action(ui, true, tr.save_scene, &self.keys("S")) {
            self.save_scene();
        }
        // On the web every save is a download, so "save as" would do the same.
        if !WEB && menu::action(ui, true, tr.save_scene_as, &self.keys("Shift+S")) {
            self.files.name = None;
            self.save_scene();
        }
        if menu::action(ui, self.files.saved.is_some(), tr.revert, "") {
            self.revert_scene();
        }
    }

    /// Import items in the file menu.
    pub(super) fn import_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        if menu::action(ui, true, tr.import_reference, "") {
            self.request(Request::OpenFile {
                purpose: FilePurpose::Reference,
                extensions: REFERENCE_EXTENSIONS,
            });
        }
        if menu::action(ui, true, tr.import_sculpt, "") {
            self.request(Request::OpenFile {
                purpose: FilePurpose::Sculpt,
                extensions: SCULPT_EXTENSIONS,
            });
        }
    }

    /// Ctrl+O and Ctrl+S everywhere (the web shell keeps the browser from
    /// acting on them); Ctrl+N and Ctrl+Shift+S only on desktop, where the
    /// browser doesn't own them or every save is a download anyway.
    pub(super) fn file_shortcuts(&mut self, ui: &egui::Ui) {
        use egui::Modifiers as M;
        let pressed = |m: M, key: Key| ui.input_mut(|i| i.consume_key(m, key));
        if !WEB && pressed(M::COMMAND, Key::N) {
            self.new_scene();
        }
        if pressed(M::COMMAND, Key::O) {
            self.ask_for_scene();
        }
        // The more specific shortcut first: Ctrl+S also matches Ctrl+Shift+S.
        if pressed(M::COMMAND | M::SHIFT, Key::S) {
            if !WEB {
                self.files.name = None;
            }
            self.save_scene();
        } else if pressed(M::COMMAND, Key::S) {
            self.save_scene();
        }
    }

    /// A fresh sphere seen from the default view.
    pub(super) fn new_scene(&mut self) {
        self.cancel_export();
        self.playback = None;
        self.sculpt = Sculpt::new(start_mesh());
        self.gpu_mesh = GpuMesh::new(&self.device, &self.sculpt.mesh);
        self.files = FileState::default();
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
