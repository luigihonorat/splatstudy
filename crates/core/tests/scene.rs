// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Saving and opening scenes through the app, the way the shells drive it:
//! input events in, requests out. Skipped (with a message) on machines
//! without any GPU adapter.

use std::sync::Arc;

use splatstudy_core::{
    App, Button, FilePurpose, InputEvent, Key, Lang, Modifiers, PointerEvent, PointerKind,
    PointerPhase, Request,
};

const SIZE: [u32; 2] = [1200, 800];

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

/// An app drawing into an offscreen texture, with its own clock.
struct Harness {
    app: App,
    target: wgpu::TextureView,
    now: f64,
}

impl Harness {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
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
        let mut harness = Self {
            app: App::new(device.clone(), queue.clone(), format, Lang::En),
            target: texture.create_view(&Default::default()),
            now: 1.0,
        };
        // Lay the views out before anything points at them.
        harness.frame();
        harness.frame();
        harness
    }

    fn frame(&mut self) -> Vec<Request> {
        self.now += 1.0 / 60.0;
        self.app.frame(self.now, &self.target, SIZE, 1.0).requests
    }

    fn pointer(&mut self, phase: PointerPhase, pos: [f32; 2]) {
        let button = matches!(phase, PointerPhase::Down | PointerPhase::Up);
        self.app.push_event(InputEvent::Pointer(PointerEvent {
            id: 1,
            kind: PointerKind::Mouse,
            phase,
            pos,
            button: button.then_some(Button::Primary),
            pressure: if phase == PointerPhase::Up { 0.0 } else { 1.0 },
            eraser: false,
        }));
    }

    /// A left drag across the middle of the sculpt view.
    fn stroke(&mut self, y: f32) {
        self.pointer(PointerPhase::Down, [360.0, y]);
        self.frame();
        for i in 1..=12 {
            self.pointer(
                PointerPhase::Move,
                [360.0 + 6.0 * i as f32, y + 2.0 * i as f32],
            );
            self.frame();
        }
        self.pointer(PointerPhase::Up, [432.0, y + 24.0]);
        self.frame();
        self.frame();
    }

    /// Presses Ctrl (and Shift) with `key`, and returns what the app asked for.
    fn shortcut(&mut self, key: Key, shift: bool) -> Vec<Request> {
        let held = Modifiers {
            ctrl: true,
            command: true,
            shift,
            alt: false,
        };
        self.app.push_event(InputEvent::Modifiers(held));
        self.app.push_event(InputEvent::Key {
            key,
            pressed: true,
            repeat: false,
        });
        let requests = self.frame();
        self.app.push_event(InputEvent::Key {
            key,
            pressed: false,
            repeat: false,
        });
        self.app
            .push_event(InputEvent::Modifiers(Modifiers::default()));
        self.frame();
        requests
    }

    /// Ctrl+S, and the bytes it saved.
    fn save(&mut self) -> (String, Vec<u8>) {
        let requests = self.shortcut(Key::S, false);
        match requests.as_slice() {
            [Request::SaveFile { name, bytes, .. }] => (name.clone(), bytes.clone()),
            other => panic!("expected one save, got {other:?}"),
        }
    }

    fn open(&mut self, name: &str, bytes: &[u8]) {
        self.app.push_event(InputEvent::File {
            purpose: FilePurpose::Any,
            name: name.to_owned(),
            bytes: Arc::from(bytes),
        });
        self.frame();
    }
}

#[test]
fn a_saved_scene_opens_to_the_same_scene() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping scene test");
        return;
    };
    let mut first = Harness::new(&device, &queue);
    let (_, empty) = first.save();
    first.stroke(380.0);
    first.stroke(440.0);
    let (name, saved) = first.save();
    assert_eq!(name, "study.splatstudy");
    assert_ne!(saved, empty, "the strokes changed the scene");

    // Opening it elsewhere and saving again gives the same file: the same
    // clay, start, log (so the same timelapse) and views.
    let mut second = Harness::new(&device, &queue);
    second.open(&name, &saved);
    let (name_again, again) = second.save();
    assert_eq!(name_again, name);
    assert!(again == saved, "the reopened scene saves differently");

    // Ctrl+O asks for a scene file.
    let requests = second.shortcut(Key::O, false);
    assert!(
        matches!(
            requests.as_slice(),
            [Request::OpenFile {
                purpose: FilePurpose::Scene,
                extensions: ["splatstudy"],
            }]
        ),
        "{requests:?}"
    );
}

#[test]
fn a_damaged_scene_leaves_the_open_one_alone() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping scene test");
        return;
    };
    let mut app = Harness::new(&device, &queue);
    app.stroke(400.0);
    let (name, saved) = app.save();
    app.open("broken.splatstudy", &saved[..saved.len() / 2]);
    app.open("nonsense.splatstudy", b"not a scene at all");
    let (_, again) = app.save();
    assert_eq!(name, "study.splatstudy");
    assert!(again == saved, "a damaged file changed the scene");
}
