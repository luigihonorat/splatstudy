// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Autosave through the app, as a shell drives it: watch the revision, take
//! the bytes when ready, restore them in a new app. Skipped (with a
//! message) on machines without any GPU adapter.

use std::sync::Arc;

use splatstudy_core::scene::{self, BlobKey};
use splatstudy_core::{
    App, Button, FilePurpose, InputEvent, Key, Lang, Modifiers, PointerEvent, PointerKind,
    PointerPhase, Request,
};

const SIZE: [u32; 2] = [1200, 800];
/// A small tetrahedron, as a mesh reference.
const TETRA: &[u8] = b"v 0 0 0\nv 2 0 0\nv 0 2 0\nv 0 0 2\nf 1 3 2\nf 1 2 4\nf 1 4 3\nf 2 3 4\n";

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

struct Shell {
    app: App,
    target: wgpu::TextureView,
    now: f64,
}

impl Shell {
    fn new((device, queue): &(wgpu::Device, wgpu::Queue)) -> Self {
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("window"),
            size: wgpu::Extent3d {
                width: SIZE[0],
                height: SIZE[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let mut shell = Self {
            app: App::new(device.clone(), queue.clone(), format, Lang::En),
            target: texture.create_view(&Default::default()),
            now: 1.0,
        };
        shell.frame();
        shell.frame();
        shell
    }

    fn frame(&mut self) -> Vec<Request> {
        self.now += 1.0 / 60.0;
        self.app.frame(self.now, &self.target, SIZE, 1.0).requests
    }

    fn pointer(&mut self, phase: PointerPhase, pos: [f32; 2]) {
        let pressed = matches!(phase, PointerPhase::Down | PointerPhase::Up);
        self.app.push_event(InputEvent::Pointer(PointerEvent {
            id: 1,
            kind: PointerKind::Mouse,
            phase,
            pos,
            button: pressed.then_some(Button::Primary),
            pressure: if phase == PointerPhase::Up { 0.0 } else { 1.0 },
            eraser: false,
        }));
        self.frame();
    }

    /// A drag across the sculpt view; checks autosave waits for its end.
    fn stroke(&mut self) {
        self.pointer(PointerPhase::Down, [340.0, 380.0]);
        for i in 1..=10 {
            self.pointer(PointerPhase::Move, [340.0 + 7.0 * i as f32, 380.0]);
        }
        assert!(!self.app.autosave_ready(false), "mid-stroke");
        assert!(
            !self.app.autosave_ready(true),
            "mid-stroke, even when leaving"
        );
        self.pointer(PointerPhase::Up, [410.0, 380.0]);
        self.frame();
        assert!(self.app.autosave_ready(false), "after the stroke");
    }

    fn open(&mut self, name: &str, bytes: &[u8]) {
        self.app.push_event(InputEvent::File {
            purpose: FilePurpose::Any,
            name: name.to_owned(),
            bytes: Arc::from(bytes),
        });
        self.frame();
    }

    fn ctrl(&mut self, key: Key) -> Vec<Request> {
        let ctrl = Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        self.app.push_event(InputEvent::Modifiers(ctrl));
        for pressed in [true, false] {
            self.app.push_event(InputEvent::Key {
                key,
                pressed,
                repeat: false,
            });
        }
        let requests = self.frame();
        self.app
            .push_event(InputEvent::Modifiers(Modifiers::default()));
        self.frame();
        requests
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn a_session_comes_back_from_its_autosave() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping autosave test");
        return;
    };
    let mut first = Shell::new(&gpu);
    let fresh = first.app.scene_revision();
    first.frame();
    assert_eq!(
        first.app.scene_revision(),
        fresh,
        "frames alone change nothing"
    );
    assert!(
        first.app.autosave_bytes().is_none(),
        "nothing worth keeping yet"
    );

    first.open("tetra.obj", TETRA);
    let with_reference = first.app.scene_revision();
    assert_ne!(with_reference, fresh, "a reference");
    let save = first
        .app
        .autosave_bytes()
        .expect("a reference is worth keeping");
    assert!(!save.sculpted);
    let (key, file) = save.reference.expect("the file goes apart");
    assert_eq!(&file[..], TETRA);
    assert_eq!(key, BlobKey::of(TETRA));
    assert!(
        !find(&save.scene, TETRA),
        "the scene names the file, without it"
    );
    assert_eq!(scene::reference_key(&save.scene), Ok(Some(key)));

    first.stroke();
    let sculpted = first.app.scene_revision();
    assert_ne!(sculpted, with_reference, "a stroke");
    first.app.apply_url_options("view=silhouette");
    assert_ne!(first.app.scene_revision(), sculpted, "a study aid");
    let save = first.app.autosave_bytes().expect("a session");
    assert!(save.sculpted);

    // A new tab: the scene and the file stored under its key.
    let mut second = Shell::new(&gpu);
    assert!(
        second
            .app
            .restore_autosave(&save.scene, Some(Arc::from(TETRA)))
    );
    second.frame();
    let again = second.app.autosave_bytes().expect("restored");
    assert!(
        again.scene == save.scene,
        "the restored session saves the same"
    );
    assert_eq!(again.reference.map(|(k, _)| k), Some(key));

    // The file went missing (the browser dropped it): the clay comes back.
    let mut third = Shell::new(&gpu);
    assert!(third.app.restore_autosave(&save.scene, None));
    let lost = third.app.autosave_bytes().expect("the session");
    assert!(lost.reference.is_none() && lost.sculpted);
    // So does a file that isn't the one named.
    let mut fourth = Shell::new(&gpu);
    assert!(
        fourth
            .app
            .restore_autosave(&save.scene, Some(Arc::from(&b"other"[..])))
    );
    assert!(fourth.app.autosave_bytes().unwrap().reference.is_none());

    // Damage leaves the app as it was.
    let before = fourth.app.scene_revision();
    assert!(
        !fourth
            .app
            .restore_autosave(&save.scene[..save.scene.len() / 2], None)
    );
    assert!(!fourth.app.restore_autosave(b"nonsense", None));
    assert_eq!(fourth.app.scene_revision(), before);
}

#[test]
fn a_scene_without_its_start_gets_a_new_history_from_its_clay() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping autosave test");
        return;
    };
    let mut first = Shell::new(&gpu);
    first.stroke();
    let full = first.app.autosave_bytes().expect("a session").scene;
    let clay = scene::Scene::read(&full).unwrap();
    assert!(!clay.log.is_empty());
    // The same clay and log, without the checkpoints the log starts from.
    let mut w = scene::SceneWriter::new();
    w.mesh(&clay.mesh).log(&clay.log);
    let broken = w.finish();

    let mut second = Shell::new(&gpu);
    second.open("broken.splatstudy", &broken);
    let reopened = second.app.autosave_bytes().expect("the clay");
    let scene = scene::Scene::read(&reopened.scene).unwrap();
    // A new history whose start is the clay as it was opened, so later
    // checkpoints keep their numbers in the replay.
    assert!(scene.log.is_empty());
    assert_eq!(scene.checkpoints.len(), 1);
    assert_eq!(scene.checkpoints[0].log_index, 0);
    assert_eq!(
        scene.checkpoints[0].triangles.as_deref(),
        Some(&clay.mesh.triangles[..])
    );
    assert_eq!(scene.mesh.triangles, clay.mesh.triangles);
}

#[test]
fn a_first_checkpoint_with_no_triangles_starts_a_new_history_too() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping autosave test");
        return;
    };
    let mut first = Shell::new(&gpu);
    first.stroke();
    let full = first.app.autosave_bytes().expect("a session").scene;
    let mut clay = scene::Scene::read(&full).unwrap();
    clay.checkpoints[0].triangles = Some(Vec::new().into());
    let mut w = scene::SceneWriter::new();
    w.mesh(&clay.mesh)
        .checkpoints(&clay.checkpoints)
        .log(&clay.log);
    let mut second = Shell::new(&gpu);
    second.open("empty-start.splatstudy", &w.finish());
    let scene = scene::Scene::read(&second.app.autosave_bytes().unwrap().scene).unwrap();
    assert!(scene.log.is_empty());
    assert_eq!(
        scene.checkpoints[0].triangles.as_deref(),
        Some(&clay.mesh.triangles[..])
    );
}

#[test]
fn a_restored_clay_is_kept_even_with_no_history() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping autosave test");
        return;
    };
    let mut first = Shell::new(&gpu);
    first.stroke();
    let full = first.app.autosave_bytes().expect("a session").scene;
    let clay = scene::Scene::read(&full).unwrap();
    // The sculpted clay, its log damaged: it opens without a history.
    let mut w = scene::SceneWriter::stored();
    w.mesh(&clay.mesh)
        .checkpoints(&clay.checkpoints)
        .chunk(scene::LOG, false, b"damaged");
    let damaged = w.finish();
    let mut second = Shell::new(&gpu);
    assert!(second.app.restore_autosave(&damaged, None));
    second.app.apply_url_options("view=grid");
    let kept = second
        .app
        .autosave_bytes()
        .expect("the restored clay is kept");
    let kept = scene::Scene::read(&kept.scene).unwrap();
    assert_eq!(kept.mesh.positions, clay.mesh.positions);

    // A new scene after it has nothing to keep again.
    second.ctrl(Key::N);
    assert!(second.app.autosave_bytes().is_none());
}

#[test]
fn a_scene_file_restores_too_and_a_new_scene_clears() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter; skipping autosave test");
        return;
    };
    let mut first = Shell::new(&gpu);
    first.open("tetra.obj", TETRA);
    first.stroke();
    // A plain scene file, with the reference inside, reads as an autosave.
    let saved = match first.ctrl(Key::S).as_slice() {
        [Request::SaveFile { bytes, .. }] => bytes.clone(),
        other => panic!("expected a save, got {other:?}"),
    };
    let mut second = Shell::new(&gpu);
    assert!(second.app.restore_autosave(&saved, None));
    let restored = second.app.autosave_bytes().expect("restored");
    assert_eq!(
        restored.reference.map(|(_, f)| f.to_vec()),
        Some(TETRA.to_vec())
    );

    // A new scene keeps the reference and starts the clay again: the next
    // autosave overwrites the session with that.
    let before = second.app.scene_revision();
    second.ctrl(Key::N);
    assert_ne!(second.app.scene_revision(), before);
    let new = second.app.autosave_bytes().expect("the reference");
    assert!(!new.sculpted);
    assert!(new.reference.is_some());

    // Without a reference a new scene has nothing to keep, and its
    // revision still differs from the one before, whose log was as empty.
    let mut third = Shell::new(&gpu);
    third.app.apply_url_options("view=grid");
    let before = third.app.scene_revision();
    third.ctrl(Key::N);
    assert!(third.app.autosave_bytes().is_none());
    assert_ne!(third.app.scene_revision(), before);
}
