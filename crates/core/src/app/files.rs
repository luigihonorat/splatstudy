// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Scenes: new, open, save, autosave, and importing files.

use std::sync::Arc;

use egui::Key;

use super::reference::REFERENCE_EXTENSIONS;
use super::sculpting::{Pending, start_sculpt};
use super::study::{Aids, Layout as ViewLayout};
use super::{App, Drag, REFERENCE, Request, SCULPT, WEB};
use crate::input::FilePurpose;
use crate::io::Writer;
use crate::meshio;
use crate::reference::Reference;
use crate::reference::load;
use crate::render::clay::{GpuMesh, Lighting};
use crate::scene::autosave::{Autosave, Trouble};
use crate::scene::blob::content_hash;
use crate::scene::reference::write_reference_by_key;
use crate::scene::{
    BlobKey, HistoryCache, SavedReference, Scene, SceneWriter, StudyView, View, ViewAids,
};
use crate::sculpt::Sculpt;
use crate::sculpt::record::starts_with_mesh;
use crate::ui::menu;

/// Scene files' extension.
pub(super) const SCENE_EXTENSION: &str = "splatstudy";
/// Saving a clay with more vertices than this takes long enough (about a
/// second at 1.5 million in the browser) to say so first.
const SAVE_NOTICE_VERTICES: usize = 300_000;
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
    /// splat takes a moment. The file is known by its address and length
    /// (holding on to it would keep its memory after it is dropped), and
    /// the key is forgotten whenever another reference may take its place.
    reference_key: Option<(FileAddress, BlobKey)>,
    /// The checkpoints and log as last saved, to encode only what is new.
    history: HistoryCache,
    /// The scene came from the autosave: worth keeping even unchanged.
    restored: bool,
    /// The autosave wasn't restored at start, in case restoring it is what
    /// made the app crash: the file menu offers it.
    restore_offered: bool,
    /// The user asked for it; the shell takes the request.
    restore_asked: bool,
}

/// Where a file's bytes are, and how many: tells it apart from any other
/// file alive at the same time.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileAddress {
    at: usize,
    len: usize,
}

impl FileAddress {
    fn of(bytes: &Arc<[u8]>) -> Self {
        Self {
            at: Arc::as_ptr(bytes).cast::<u8>() as usize,
            len: bytes.len(),
        }
    }
}

/// A fingerprint of everything an autosave keeps that can change: which
/// scene this is, how far its log has come, the reference with its
/// placement, and the study aids. Cheap enough to work out every frame.
fn revision(
    generation: u64,
    sculpt: &Sculpt,
    reference: Option<&Reference>,
    study: &StudyView,
    ortho: [bool; 2],
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
    study.write(&mut w, ortho);
    content_hash(&w.finish())
}

/// A file name's extension, lowercase, without the dot.
fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// The extensions a file asked for `purpose` may have.
fn extensions_for(purpose: FilePurpose) -> &'static [&'static str] {
    match purpose {
        FilePurpose::Scene => &[SCENE_EXTENSION],
        FilePurpose::Reference => REFERENCE_EXTENSIONS,
        FilePurpose::Sculpt => meshio::EXTENSIONS,
        FilePurpose::Any => &[],
    }
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
    /// True if a file named `name`, dropped or picked for `purpose`, may
    /// be one the app opens. The shells ask before reading it: anything
    /// else is handed over empty, without reading, and the app says it
    /// can't open it.
    pub fn can_open(purpose: FilePurpose, name: &str) -> bool {
        let purpose = match purpose {
            FilePurpose::Any => purpose_of(name),
            purpose => purpose,
        };
        extensions_for(purpose).contains(&extension(name).as_str())
    }

    /// A file arrived from the shell: dropped, picked or fetched.
    pub(super) fn on_file(&mut self, purpose: FilePurpose, name: &str, bytes: Arc<[u8]>) {
        let purpose = match purpose {
            FilePurpose::Any => purpose_of(name),
            purpose => purpose,
        };
        match purpose {
            FilePurpose::Scene => self.open_scene(name, bytes),
            FilePurpose::Reference => {
                self.files.reference_key = None;
                self.load_reference(name, bytes);
            }
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
        self.files.restored = false;
        if !complete {
            self.notify(self.lang.tr().files.timelapse_lost);
        }
    }

    /// Puts a scene in place of the open one. False if its timelapse can't
    /// be played from the start.
    fn show_scene(&mut self, scene: Scene) -> bool {
        let Scene {
            mesh,
            levels,
            checkpoints,
            log,
            reference,
            view,
            history_damaged,
            reference_damaged,
        } = scene;
        // The timelapse needs the clay the log starts from. Without it the
        // replay would start from the default sphere, and number the
        // checkpoints after it one too low: start a new history from the
        // clay as it is instead. (As the replay decides: a first checkpoint
        // with no triangles, or triangles that don't fit, isn't usable.)
        let usable = starts_with_mesh(&checkpoints);
        let complete = (usable || log.is_empty()) && !history_damaged;

        self.cancel_export();
        self.playback = None;
        self.drags = [None; 2];
        self.sculpt = if usable {
            Sculpt::with_history(mesh, levels, log, checkpoints)
        } else {
            Sculpt::with_levels(mesh, levels)
        };
        self.sculpt.take_topology_changed();
        self.gpu_mesh = GpuMesh::upload(&self.device, &self.queue, &self.sculpt.mesh);
        if let Some(view) = view {
            self.cameras = view.cameras;
            self.split = view.split;
            if let Some(study) = view.study {
                self.restore_study(&study);
            }
        }
        // Another file may come back at the old one's address.
        self.files.reference_key = None;
        match reference {
            Some(reference) => self.restore_reference(reference),
            None => self.remove_reference(),
        }
        if reference_damaged {
            self.notify(self.lang.tr().files.reference_damaged);
        }
        self.files.replaced();
        complete
    }

    /// The scene as a file: the clay, its checkpoints (the first is where
    /// the session started), the session, the reference and the views.
    fn scene_bytes(&mut self) -> Vec<u8> {
        self.write_scene(SceneWriter::new(), None, true)
    }

    /// The scene, with the reference file inside, or named by `key` and
    /// left out. Without its `history`, it is the clay as it is, with its
    /// levels and quads, and no timelapse.
    fn write_scene(&mut self, mut w: SceneWriter, key: Option<BlobKey>, history: bool) -> Vec<u8> {
        w.mesh(&self.sculpt.mesh);
        if history {
            w.history(&self.sculpt, &mut self.files.history)
                .levels(&self.sculpt)
                .quads(&self.sculpt);
        } else {
            w.levels_now(&self.sculpt).quads_now(&self.sculpt);
        }
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

    /// Saves the scene, at the next frame and after a notice if the clay
    /// is fine enough to take a moment to pack.
    fn ask_save(&mut self) {
        if self.sculpt.mesh.positions.len() > SAVE_NOTICE_VERTICES {
            self.ask(Pending::SaveScene, self.lang.tr().save_scene);
        } else {
            self.save_scene();
        }
    }

    /// Saves the scene: a save dialog on desktop, a download on the web.
    pub(super) fn save_scene(&mut self) {
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

    /// Saves the scene as it is, without its timelapse, as another file:
    /// smaller, to share or to start a new session from. The open scene
    /// keeps its name, its history and what revert goes back to.
    pub(super) fn save_copy(&mut self) {
        let bytes = self.write_scene(SceneWriter::new(), None, false);
        let name = format!("{}-no-timelapse.{SCENE_EXTENSION}", self.scene_stem());
        self.request(Request::SaveFile {
            name,
            mime: SCENE_MIME,
            bytes,
        });
    }

    /// [`save_copy`](Self::save_copy), at the next frame and after a
    /// notice for a fine clay, as saving does.
    fn ask_save_copy(&mut self) {
        if self.sculpt.mesh.positions.len() > SAVE_NOTICE_VERTICES {
            self.ask(Pending::SaveCopy, self.lang.tr().files.save_copy);
        } else {
            self.save_copy();
        }
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
            views: s.views.map(|aids| ViewAids {
                mirror: aids.mirror,
                plumb: aids.plumb,
                grid: aids.grid,
                silhouette: aids.silhouette,
                lighting: Lighting::ALL
                    .iter()
                    .position(|&l| l == aids.lighting)
                    .unwrap_or(0) as u8,
            }),
            plumb_x: s.plumb_x,
            grid_divisions: s.grid_divisions,
            ghost: s.ghost,
            wireframe: s.wireframe,
            tint: s.matching.tint,
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
        s.views = view.views.map(|aids| Aids {
            mirror: aids.mirror,
            plumb: aids.plumb,
            grid: aids.grid,
            silhouette: aids.silhouette,
            lighting: Lighting::ALL
                .get(usize::from(aids.lighting))
                .copied()
                .unwrap_or_default(),
        });
        s.plumb_x = view.plumb_x;
        s.grid_divisions = view.grid_divisions;
        s.ghost = view.ghost;
        s.wireframe = view.wireframe;
        s.matching.tint = view.tint;
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
            self.ask_save();
        }
        // On the web every save is a download, so "save as" would do the same.
        if !WEB && menu::action(ui, true, tr.save_scene_as, &self.keys("Shift+S")) {
            self.files.name = None;
            self.ask_save();
        }
        if menu::action(ui, true, tr.files.save_copy, "") {
            self.ask_save_copy();
        }
        if menu::action(ui, self.files.saved.is_some(), tr.revert, "") {
            self.revert_scene();
        }
        if self.files.restore_offered
            && menu::action(ui, !self.files.restore_asked, tr.files.restore_last, "")
        {
            self.files.restore_asked = true;
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
            self.ask_save();
        } else if pressed(M::COMMAND, Key::S) {
            self.ask_save();
        }
    }

    /// A fresh sphere seen from the default view. The autosave goes with
    /// the old scene: there is nothing in this one to keep.
    pub(super) fn new_scene(&mut self) {
        self.cancel_export();
        self.playback = None;
        self.sculpt = start_sculpt();
        self.gpu_mesh = GpuMesh::upload(&self.device, &self.queue, &self.sculpt.mesh);
        self.files.name = None;
        self.files.saved = None;
        self.files.restored = false;
        self.files.replaced();
        self.reset_view(SCULPT);
        self.reset_view(REFERENCE);
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
            self.cameras.map(|c| c.ortho),
        )
    }

    /// True when an autosave can be taken now: no stroke under way, and
    /// no export, which saving would stall, unless the page or window is
    /// `leaving` and the export is lost anyway. Also false while the
    /// autosave is only offered (see [`App::offer_restore`]) and the open
    /// scene has nothing worth keeping: that would drop the offered one.
    pub fn autosave_ready(&self, leaving: bool) -> bool {
        let stroking = self
            .drags
            .iter()
            .any(|d| matches!(d, Some(Drag::Stroke { .. } | Drag::Grab { .. })));
        let offered = self.files.restore_offered && !self.worth_keeping();
        !stroking && !offered && (leaving || !self.exporting())
    }

    /// False for a fresh sphere: nothing sculpted, no reference, not from
    /// a file or the autosave, and the clay as the session started.
    fn worth_keeping(&self) -> bool {
        let sculpt = &self.sculpt;
        let changed = || {
            sculpt.checkpoints().first().is_none_or(|start| {
                *start.positions != *sculpt.mesh.positions
                    || start
                        .triangles
                        .as_deref()
                        .is_some_and(|t| t != sculpt.mesh.triangles.as_slice())
            })
        };
        !sculpt.log().is_empty()
            || self.scene_reference().is_some()
            || self.files.name.is_some()
            || self.files.restored
            || changed()
    }

    /// The scene to autosave, with the reference file apart so the shell
    /// stores it once (see [`crate::scene::autosave`]). Nothing in it is
    /// deflated: it is written often, and kept only until the next one.
    /// `None` when there is nothing worth keeping: a fresh sphere with no
    /// reference. The first call after a new reference hashes its file.
    pub fn autosave_bytes(&mut self) -> Option<Autosave> {
        if !self.worth_keeping() {
            return None;
        }
        // The autosave now holds this scene: the offered one is gone.
        self.files.restore_offered = false;
        let sculpted = !self.sculpt.log().is_empty();
        let reference = self.scene_reference().map(|r| r.source.bytes.clone());
        let reference = reference.map(|bytes| (self.files.key_of(&bytes), bytes));
        Some(Autosave {
            scene: self.write_scene(
                SceneWriter::stored(),
                reference.as_ref().map(|(key, _)| *key),
                true,
            ),
            reference,
            sculpted,
        })
    }

    /// The shell didn't restore the autosave at start, because the last
    /// restore never got to draw a frame: perhaps it crashed the app. Says
    /// so, and offers it in the file menu.
    pub fn offer_restore(&mut self) {
        self.files.restore_offered = true;
        self.files.restore_asked = false;
        self.notify(self.lang.tr().files.restore_skipped);
    }

    /// True once after the user chose to restore the offered autosave:
    /// the shell restores it then, through [`App::restore_autosave`].
    pub fn take_restore_request(&mut self) -> bool {
        std::mem::take(&mut self.files.restore_asked)
    }

    /// The autosave the user asked for is no longer stored.
    pub fn autosave_gone(&mut self) {
        self.files.restore_offered = false;
        self.notify(self.lang.tr().files.autosave_gone);
    }

    /// Opens an autosave at start: `scene` from [`App::autosave_bytes`],
    /// with the reference file stored under the key the scene names, if
    /// it was found. Says so with a notice. False, and nothing changes,
    /// if the scene doesn't read.
    pub fn restore_autosave(&mut self, scene: &[u8], reference: Option<Arc<[u8]>>) -> bool {
        let mut lost = false;
        let mut known = None;
        let read = Scene::read_with(scene, |key| {
            let found = reference.clone().filter(|bytes| key.matches(bytes));
            lost = found.is_none();
            found.inspect(|bytes| known = Some((FileAddress::of(bytes), key)))
        });
        let scene = match read {
            Ok(scene) => scene,
            Err(err) => {
                log::warn!("autosave: {err}");
                return false;
            }
        };
        let lost = lost || scene.reference_damaged;
        let complete = self.show_scene(scene);
        // Hashed already, by `matches`.
        self.files.reference_key = known.filter(|(file, _)| {
            self.scene_reference()
                .is_some_and(|r| FileAddress::of(&r.source.bytes) == *file)
        });
        self.files.name = None;
        self.files.saved = None;
        self.files.restored = true;
        self.files.restore_offered = false;
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

    /// The key of the reference file, `bytes`, hashed only the first time.
    /// Its address tells it from any other file alive now; a file that
    /// replaced it at the same address is caught by `reference_key` being
    /// cleared whenever a reference comes in.
    fn key_of(&mut self, bytes: &Arc<[u8]>) -> BlobKey {
        let file = FileAddress::of(bytes);
        if let Some((known, key)) = self.reference_key
            && known == file
        {
            return key;
        }
        let key = BlobKey::of(bytes);
        self.reference_key = Some((file, key));
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

        let base = revision(0, &sculpt, None, &study, [false; 2]);
        changed(base, "start");
        assert_eq!(
            revision(0, &sculpt, None, &study, [false; 2]),
            base,
            "nothing changed"
        );
        changed(
            revision(1, &sculpt, None, &study, [false; 2]),
            "another scene",
        );

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
        changed(revision(0, &sculpt, None, &study, [false; 2]), "mid-stroke");
        sculpt.end_stroke();
        changed(revision(0, &sculpt, None, &study, [false; 2]), "a stroke");
        sculpt.undo();
        changed(revision(0, &sculpt, None, &study, [false; 2]), "undo");
        sculpt.replace_mesh(Mesh::icosphere(1));
        changed(revision(0, &sculpt, None, &study, [false; 2]), "a remesh");

        let mut r = reference(&[1, 2, 3, 4]);
        changed(
            revision(0, &sculpt, Some(&r), &study, [false; 2]),
            "a reference",
        );
        r.align.yaw = 0.25;
        changed(revision(0, &sculpt, Some(&r), &study, [false; 2]), "turned");
        r.align.scale = 2.0;
        changed(revision(0, &sculpt, Some(&r), &study, [false; 2]), "scaled");
        r.crop = Some(Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::ONE,
        });
        changed(
            revision(0, &sculpt, Some(&r), &study, [false; 2]),
            "cropped",
        );
        r.credit.author = "someone".into();
        changed(
            revision(0, &sculpt, Some(&r), &study, [false; 2]),
            "credited",
        );
        // The same placement on another file of the same length.
        let other = Reference {
            align: r.align,
            crop: r.crop,
            credit: r.credit.clone(),
            ..reference(&[1, 2, 3, 4])
        };
        changed(
            revision(0, &sculpt, Some(&other), &study, [false; 2]),
            "another file",
        );

        let toggles: [fn(&mut StudyView); 17] = [
            |s| s.swapped = true,
            |s| s.link = true,
            |s| s.views[0].mirror = true,
            |s| s.views[1].mirror = true,
            |s| s.views[0].plumb = true,
            |s| s.views[1].grid = true,
            |s| s.views[0].silhouette = true,
            |s| s.views[1].silhouette = true,
            |s| s.views[1].lighting = 2,
            |s| s.ghost = true,
            |s| s.wireframe = true,
            |s| s.tint = true,
            |s| s.layout = 1,
            |s| s.plumb_x[0] = 0.3,
            |s| s.plumb_x[1] = 0.7,
            |s| s.grid_divisions = 6,
            |s| s.views[0].grid = true,
        ];
        for (i, toggle) in toggles.iter().enumerate() {
            toggle(&mut study);
            changed(
                revision(0, &sculpt, Some(&r), &study, [false; 2]),
                &format!("aid {i}"),
            );
        }
        let ortho = revision(0, &sculpt, Some(&r), &study, [false, true]);
        changed(ortho, "the reference view orthographic");
    }

    #[test]
    fn a_reference_file_is_hashed_once() {
        let mut files = FileState::default();
        let bytes: Arc<[u8]> = Arc::from(&[7u8; 1000][..]);
        let key = files.key_of(&bytes);
        assert_eq!(key, BlobKey::of(&bytes));
        // Remembered: a wrong key planted for the same file comes back.
        let planted = BlobKey { hash: 1, len: 1000 };
        files.reference_key = Some((FileAddress::of(&bytes), planted));
        assert_eq!(files.key_of(&bytes), planted);
        // Another file with the same bytes is hashed afresh.
        let copy: Arc<[u8]> = Arc::from(&bytes[..]);
        assert_eq!(files.key_of(&copy), key);
    }

    #[test]
    fn the_key_cache_keeps_no_file_alive() {
        let mut files = FileState::default();
        let bytes: Arc<[u8]> = Arc::from(&[7u8; 1000][..]);
        files.key_of(&bytes);
        // Only the app's own handle is left: dropping it frees the file.
        assert_eq!(Arc::strong_count(&bytes), 1);
        assert_eq!(Arc::weak_count(&bytes), 0);
    }

    #[test]
    fn files_are_checked_by_name_before_they_are_read() {
        assert!(App::can_open(FilePurpose::Any, "bust.ply"));
        assert!(App::can_open(FilePurpose::Any, "study.SPLATSTUDY"));
        assert!(!App::can_open(FilePurpose::Any, "movie.mp4"));
        assert!(!App::can_open(FilePurpose::Any, "no extension"));
        assert!(App::can_open(FilePurpose::Reference, "cast.spz"));
        assert!(!App::can_open(FilePurpose::Reference, "study.splatstudy"));
        assert!(App::can_open(FilePurpose::Scene, "study.splatstudy"));
        assert!(!App::can_open(FilePurpose::Scene, "cast.ply"));
        assert!(App::can_open(FilePurpose::Sculpt, "head.obj"));
        assert!(!App::can_open(FilePurpose::Sculpt, "photo.png"));
    }

    #[test]
    fn a_copy_without_timelapse_reopens_the_scene_and_leaves_it_as_it_was() {
        use super::super::study::tests::Harness;
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let app = &mut h.app;
        app.sculpt.subdivide();
        for k in 0..3 {
            app.sculpt.set_clock(1.0 + k as f64);
            app.sculpt.begin_stroke();
            app.sculpt.dab(Dab {
                brush: Brush::Clay,
                centre: Vec3::new(0.2 * k as f32, 0.1, 1.0).normalize(),
                radius: 0.4,
                strength: 0.5,
                invert: false,
                symmetry: true,
                direction: Vec3::ZERO,
            });
            app.sculpt.end_stroke();
            app.sculpt.refresh();
        }
        let png = crate::export::png(&[120, 90, 60, 255].repeat(16), [4, 4]).unwrap();
        app.load_reference("cast.png", Arc::from(png));
        let cast = app.reference.current.as_mut().expect("the picture opens");
        cast.credit.author = "A. Sculptor".into();
        cast.credit.licence = "CC BY 4.0".into();
        app.study.views[SCULPT].grid = true;
        app.study.swapped = true;
        app.cameras[REFERENCE].orbit(30.0, 5.0);
        app.files.name = Some("bust.splatstudy".into());
        app.save_scene();
        h.frame_requests();

        // Saving the copy leaves the open scene as it was.
        let app = &mut h.app;
        let saved = app.files.saved.clone().unwrap();
        let revision = app.scene_revision();
        app.save_copy();
        assert_eq!(app.files.name.as_deref(), Some("bust.splatstudy"));
        assert!(Arc::ptr_eq(app.files.saved.as_ref().unwrap(), &saved));
        assert_eq!(app.scene_revision(), revision);
        let (name, bytes) = h
            .frame_requests()
            .into_iter()
            .find_map(|r| match r {
                Request::SaveFile { name, bytes, .. } => Some((name, bytes)),
                _ => None,
            })
            .expect("the copy is saved");
        assert_eq!(name, "bust-no-timelapse.splatstudy");
        assert!(
            bytes.len() < saved.len(),
            "{} < {}",
            bytes.len(),
            saved.len()
        );

        // Opened, it is the same scene without its timelapse.
        let Some(mut other) = Harness::new() else {
            return;
        };
        other.app.open_scene(&name, Arc::from(bytes));
        other.frames(2);
        let (a, b) = (&h.app, &other.app);
        assert_eq!(b.sculpt.mesh.positions, a.sculpt.mesh.positions);
        assert_eq!(b.sculpt.mesh.triangles, a.sculpt.mesh.triangles);
        assert_eq!(b.sculpt.level_data(), a.sculpt.level_data());
        assert_eq!(b.sculpt.levels(), a.sculpt.levels());
        assert_eq!(b.cameras, a.cameras);
        assert_eq!(b.study_view(), a.study_view());
        let (ra, rb) = (a.scene_reference().unwrap(), b.scene_reference().unwrap());
        assert_eq!(rb.credit, ra.credit);
        assert_eq!(*rb.source.bytes, *ra.source.bytes);
        assert!(b.sculpt.log().is_empty());
        let lost = b.lang.tr().files.timelapse_lost;
        assert!(
            b.notice.as_ref().is_none_or(|(text, _)| text != lost),
            "no timelapse lost: there was none"
        );

        // The timelapse menu leaves watching it off; played anyway, it
        // shows the clay as it is and ends there.
        let b = &mut other.app;
        b.start_playback();
        other.frames(3);
        let playback = other.app.playback.as_ref().expect("plays");
        assert_eq!(playback.replay.duration(), 0.0);
        assert_eq!(
            playback.replay.sculpt.mesh.positions,
            h.app.sculpt.mesh.positions
        );
        other.app.playback = None;
        // Sculpting on starts its timelapse from the clay as it opened.
        let b = &mut other.app;
        b.sculpt.set_clock(10.0);
        b.sculpt.begin_stroke();
        b.sculpt.dab(Dab {
            brush: Brush::Clay,
            centre: Vec3::Z,
            radius: 0.4,
            strength: 0.5,
            invert: false,
            symmetry: false,
            direction: Vec3::ZERO,
        });
        b.sculpt.end_stroke();
        b.sculpt.refresh();
        b.start_playback();
        other.frames(3);
        assert!(other.app.playback.is_some());
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
