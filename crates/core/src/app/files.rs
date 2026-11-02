// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Scenes: new, open, save, autosave, and importing files.

use std::sync::{Arc, Weak};

use egui::Key;

use super::reference::REFERENCE_EXTENSIONS;
use super::sculpting::start_mesh;
use super::study::Layout as ViewLayout;
use super::{App, Drag, REFERENCE, Request, SCULPT, WEB};
use crate::input::FilePurpose;
use crate::io::Writer;
use crate::reference::Reference;
use crate::reference::load;
use crate::render::clay::GpuMesh;
use crate::scene::autosave::{Autosave, Trouble};
use crate::scene::blob::content_hash;
use crate::scene::reference::write_reference_by_key;
use crate::scene::{BlobKey, SavedReference, Scene, SceneWriter, StudyView, View};
use crate::sculpt::Sculpt;
use crate::ui::menu;

/// Scene files' extension.
pub(super) const SCENE_EXTENSION: &str = "splatstudy";
/// Scenes download as plain bytes; no browser knows the type.
const SCENE_MIME: &str = "application/octet-stream";

/// The open scene's file.
#[derive(Default)]
pub(super) struct FileState {
    /// The file name it was opened or last saved as.
    name: Option<String>,
    /// The file as last opened or saved, for revert.
    saved: Option<Arc<[u8]>>,
    /// Counts the scenes that took the place of the one before (new, open,
    /// restore), so the autosave revision changes even when the new log is
    /// as long as the old.
    generation: u64,
    /// The reference file's key, worked out once per file: hashing a big
    /// splat takes a moment.
    reference_key: Option<(Weak<[u8]>, BlobKey)>,
}

/// A fingerprint of everything an autosave keeps that can change: which
/// scene this is, how far its log has come, the reference with its
/// placement, and the study aids. Cheap enough to work out every frame.
fn revision(
    generation: u64,
    sculpt: &Sculpt,
    reference: Option<&Reference>,
    study: &StudyView,
) -> u64 {
    let mut w = Writer::new();
    w.u64(generation)
        .len(sculpt.log().len())
        .len(sculpt.checkpoints().len());
    if let Some(r) = reference {
        // The file by its address: a new file is a new allocation, and
        // the bytes themselves are too many to hash every frame.
        let file = BlobKey {
            hash: Arc::as_ptr(&r.source.bytes).cast::<u8>() as usize as u64,
            len: r.source.bytes.len() as u64,
        };
        write_reference_by_key(&mut w, r, file);
    }
    let flags = [
        study.swapped,
        study.link,
        study.mirror,
        study.plumb,
        study.grid,
        study.silhouette,
        study.ghost,
    ];
    for on in flags {
        w.u8(on.into());
    }
    w.u8(study.layout)
        .f32(study.plumb_x[0])
        .f32(study.plumb_x[1])
        .u32(study.grid_divisions);
    content_hash(&w.finish())
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
        let complete = self.show_scene(scene);
        self.files.name = Some(name.to_owned());
        self.files.saved = Some(bytes);
        if !complete {
            self.notify(self.lang.tr().files.timelapse_lost);
        }
    }

    /// Puts a scene in place of the open one. False if its timelapse can't
    /// be played from the start.
    fn show_scene(&mut self, scene: Scene) -> bool {
        let Scene {
            mesh,
            checkpoints,
            log,
            reference,
            view,
        } = scene;
        // The timelapse needs the clay the log starts from. Without it the
        // replay would start from the default sphere, and number the
        // checkpoints after it one too low: start a new history from the
        // clay as it is instead.
        let usable = checkpoints
            .first()
            .is_some_and(|c| c.log_index == 0 && c.triangles.is_some());
        let complete = usable || log.is_empty();

        self.cancel_export();
        self.playback = None;
        self.drags = [None; 2];
        self.sculpt = if usable {
            Sculpt::with_history(mesh, log, checkpoints)
        } else {
            Sculpt::new(mesh)
        };
        self.sculpt.take_topology_changed();
        self.gpu_mesh = GpuMesh::new(&self.device, &self.sculpt.mesh);
        if let Some(view) = view {
            self.cameras = view.cameras;
            self.split = view.split;
            if let Some(study) = view.study {
                self.restore_study(&study);
            }
        }
        match reference {
            Some(reference) => self.restore_reference(reference),
            None => self.remove_reference(),
        }
        self.files.replaced();
        complete
    }

    /// The scene as a file: the clay, its checkpoints (the first is where
    /// the session started), the session, the reference and the views.
    fn scene_bytes(&self) -> Vec<u8> {
        self.write_scene(None)
    }

    /// The scene, with the reference file inside, or named by `key` and
    /// left out.
    fn write_scene(&self, key: Option<BlobKey>) -> Vec<u8> {
        let mut w = SceneWriter::new();
        w.mesh(&self.sculpt.mesh)
            .checkpoints(self.sculpt.checkpoints())
            .log(self.sculpt.log());
        match (self.scene_reference(), key) {
            (Some(reference), Some(key)) => w.reference_by_key(reference, key),
            (Some(reference), None) => w.reference(reference),
            (None, _) => &mut w,
        };
        w.view(&View {
            cameras: [self.cameras[SCULPT], self.cameras[REFERENCE]],
            split: self.split,
            study: Some(self.study_view()),
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

    /// The loaded reference, to save in the scene and credit in exports.
    pub(super) fn scene_reference(&self) -> Option<&Reference> {
        self.reference.current.as_ref()
    }

    /// Puts a scene's reference back: loads its file again, then restores
    /// the placement, crop and credit it was saved with.
    fn restore_reference(&mut self, saved: SavedReference) {
        match load::load(&saved.source.name, &saved.source.bytes) {
            Ok(content) => self.set_reference(saved.into_reference(content)),
            Err(err) => {
                log::warn!("{}: {err}", saved.source.name);
                self.remove_reference();
                self.cannot_open(&saved.source.name);
            }
        }
    }

    /// The study aids that are on, for the scene.
    fn study_view(&self) -> StudyView {
        let s = &self.study;
        StudyView {
            layout: match s.layout {
                ViewLayout::Both => 0,
                ViewLayout::Sculpt => 1,
                ViewLayout::Reference => 2,
            },
            swapped: s.swapped,
            link: s.link,
            mirror: s.mirror,
            plumb: s.plumb,
            plumb_x: s.plumb_x,
            grid: s.grid,
            grid_divisions: s.grid_divisions,
            silhouette: s.silhouette,
            ghost: s.ghost,
        }
    }

    fn restore_study(&mut self, view: &StudyView) {
        let s = &mut self.study;
        s.layout = match view.layout {
            1 => ViewLayout::Sculpt,
            2 => ViewLayout::Reference,
            _ => ViewLayout::Both,
        };
        s.swapped = view.swapped;
        s.link = view.link;
        s.mirror = view.mirror;
        s.plumb = view.plumb;
        s.plumb_x = view.plumb_x;
        s.grid = view.grid;
        s.grid_divisions = view.grid_divisions;
        s.silhouette = view.silhouette;
        s.ghost = view.ghost;
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
            self.ask_import_sculpt();
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

    /// A fresh sphere seen from the default view. The autosave goes with
    /// the old scene: there is nothing in this one to keep.
    pub(super) fn new_scene(&mut self) {
        self.cancel_export();
        self.playback = None;
        self.sculpt = Sculpt::new(start_mesh());
        self.gpu_mesh = GpuMesh::new(&self.device, &self.sculpt.mesh);
        self.files.name = None;
        self.files.saved = None;
        self.files.replaced();
        self.reset_view();
    }

    /// Changes whenever something an autosave keeps has changed: a stroke
    /// or any logged edit, a new or reopened scene, the reference or its
    /// placement, crop or credit, a study aid. Camera moves don't count.
    /// It is a fingerprint, so only compare it for equality.
    pub fn scene_revision(&self) -> u64 {
        revision(
            self.files.generation,
            &self.sculpt,
            self.scene_reference(),
            &self.study_view(),
        )
    }

    /// True when an autosave can be taken now: no stroke under way, and
    /// no export, which saving would stall, unless the page or window is
    /// `leaving` and the export is lost anyway.
    pub fn autosave_ready(&self, leaving: bool) -> bool {
        let stroking = self
            .drags
            .iter()
            .any(|d| matches!(d, Some(Drag::Stroke { .. } | Drag::Grab { .. })));
        !stroking && (leaving || !self.exporting())
    }

    /// The scene to autosave, with the reference file apart so the shell
    /// stores it once (see [`crate::scene::autosave`]). `None` when there
    /// is nothing worth keeping: a fresh sphere with no reference. The
    /// first call after a new reference hashes its file.
    pub fn autosave_bytes(&mut self) -> Option<Autosave> {
        let sculpted = !self.sculpt.log().is_empty();
        if !sculpted && self.scene_reference().is_none() && self.files.name.is_none() {
            return None;
        }
        let reference = self.scene_reference().map(|r| r.source.bytes.clone());
        let reference = reference.map(|bytes| (self.files.key_of(&bytes), bytes));
        Some(Autosave {
            scene: self.write_scene(reference.as_ref().map(|(key, _)| *key)),
            reference,
            sculpted,
        })
    }

    /// Opens an autosave at start: `scene` from [`App::autosave_bytes`],
    /// with the reference file stored under the key the scene names, if
    /// it was found. Says so with a notice. False, and nothing changes,
    /// if the scene doesn't read.
    pub fn restore_autosave(&mut self, scene: &[u8], reference: Option<Arc<[u8]>>) -> bool {
        let mut lost = false;
        let read = Scene::read_with(scene, |key| {
            let found = reference.clone().filter(|bytes| key.matches(bytes));
            lost = found.is_none();
            found.inspect(|bytes| self.files.reference_key = Some((Arc::downgrade(bytes), key)))
        });
        let scene = match read {
            Ok(scene) => scene,
            Err(err) => {
                log::warn!("autosave: {err}");
                return false;
            }
        };
        let complete = self.show_scene(scene);
        self.files.name = None;
        self.files.saved = None;
        let tr = self.lang.tr();
        self.notify(match (lost, complete) {
            (true, _) => tr.files.restored_without_reference,
            (false, true) => tr.files.restored,
            (false, false) => tr.files.timelapse_lost,
        });
        true
    }

    /// Autosave stopped, or left the reference out: tells the user. The
    /// shells call it once.
    pub fn autosave_trouble(&mut self, trouble: Trouble) {
        let tr = self.lang.tr();
        self.notify(match trouble {
            Trouble::Unavailable => tr.files.autosave_unavailable,
            Trouble::Full => tr.files.autosave_full,
            Trouble::ReferenceTooBig => tr.files.autosave_reference_too_big,
        });
    }
}

impl FileState {
    /// Another scene took the open one's place.
    fn replaced(&mut self) {
        self.generation += 1;
    }

    /// The key of a reference file, hashed only the first time.
    fn key_of(&mut self, bytes: &Arc<[u8]>) -> BlobKey {
        if let Some((file, key)) = &self.reference_key
            && std::ptr::addr_eq(file.as_ptr(), Arc::as_ptr(bytes))
        {
            return *key;
        }
        let key = BlobKey::of(bytes);
        self.reference_key = Some((Arc::downgrade(bytes), key));
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::reference::{Aabb, Content, RefImage, Source};
    use crate::sculpt::{Brush, Dab, Mesh};
    use glam::Vec3;

    fn reference(bytes: &[u8]) -> Reference {
        Reference::new(
            Source {
                name: "cast.png".into(),
                bytes: Arc::from(bytes),
            },
            Content::Image(RefImage::default()),
        )
    }

    #[test]
    fn the_revision_changes_with_every_kind_of_edit() {
        let mut sculpt = Sculpt::new(Mesh::icosphere(2));
        let mut study = StudyView {
            grid_divisions: 4,
            plumb_x: [0.5; 2],
            ..Default::default()
        };
        let mut seen = std::collections::HashSet::new();
        let mut changed = |r: u64, what: &str| assert!(seen.insert(r), "{what}");

        let base = revision(0, &sculpt, None, &study);
        changed(base, "start");
        assert_eq!(revision(0, &sculpt, None, &study), base, "nothing changed");
        changed(revision(1, &sculpt, None, &study), "another scene");

        sculpt.begin_stroke();
        sculpt.dab(Dab {
            brush: Brush::Clay,
            centre: Vec3::Z,
            radius: 0.4,
            strength: 0.5,
            invert: false,
            symmetry: false,
            direction: Vec3::ZERO,
        });
        changed(revision(0, &sculpt, None, &study), "mid-stroke");
        sculpt.end_stroke();
        changed(revision(0, &sculpt, None, &study), "a stroke");
        sculpt.undo();
        changed(revision(0, &sculpt, None, &study), "undo");
        sculpt.replace_mesh(Mesh::icosphere(1));
        changed(revision(0, &sculpt, None, &study), "a remesh");

        let mut r = reference(&[1, 2, 3, 4]);
        changed(revision(0, &sculpt, Some(&r), &study), "a reference");
        r.align.yaw = 0.25;
        changed(revision(0, &sculpt, Some(&r), &study), "turned");
        r.align.scale = 2.0;
        changed(revision(0, &sculpt, Some(&r), &study), "scaled");
        r.crop = Some(Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::ONE,
        });
        changed(revision(0, &sculpt, Some(&r), &study), "cropped");
        r.credit.author = "someone".into();
        changed(revision(0, &sculpt, Some(&r), &study), "credited");
        // The same placement on another file of the same length.
        let other = Reference {
            align: r.align,
            crop: r.crop,
            credit: r.credit.clone(),
            ..reference(&[1, 2, 3, 4])
        };
        changed(revision(0, &sculpt, Some(&other), &study), "another file");

        let toggles: [fn(&mut StudyView); 11] = [
            |s| s.swapped = true,
            |s| s.link = true,
            |s| s.mirror = true,
            |s| s.plumb = true,
            |s| s.grid = true,
            |s| s.silhouette = true,
            |s| s.ghost = true,
            |s| s.layout = 1,
            |s| s.plumb_x[0] = 0.3,
            |s| s.plumb_x[1] = 0.7,
            |s| s.grid_divisions = 6,
        ];
        for (i, toggle) in toggles.iter().enumerate() {
            toggle(&mut study);
            changed(revision(0, &sculpt, Some(&r), &study), &format!("aid {i}"));
        }
    }

    #[test]
    fn a_reference_file_is_hashed_once() {
        let mut files = FileState::default();
        let bytes: Arc<[u8]> = Arc::from(&[7u8; 1000][..]);
        let key = files.key_of(&bytes);
        assert_eq!(key, BlobKey::of(&bytes));
        // Remembered: a wrong key planted for the same file comes back.
        let planted = BlobKey { hash: 1, len: 1000 };
        files.reference_key = Some((Arc::downgrade(&bytes), planted));
        assert_eq!(files.key_of(&bytes), planted);
        // Another file with the same bytes is hashed afresh.
        let copy: Arc<[u8]> = Arc::from(&bytes[..]);
        assert_eq!(files.key_of(&copy), key);
    }

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
