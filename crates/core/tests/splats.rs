// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Offscreen splat rendering: draw splats (and clay) into a view target and
//! read the pixels back. Skipped (with a message) without a GPU adapter.

use glam::{Mat4, Quat, Vec3};
use splatstudy_core::camera::OrbitCamera;
use splatstudy_core::reference::{Aabb, Splats};
use splatstudy_core::render::clay::{ClayPass, GpuMesh, Look};
use splatstudy_core::render::splat::{SplatLook, SplatPass};
use splatstudy_core::render::{Slot, ViewTarget, readback::Readback};
use splatstudy_core::sculpt::Mesh;
use splatstudy_core::ui::theme::{CLAY, SILHOUETTE_INK, VIEW_BG};

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

fn splats(list: &[(Vec3, f32, [u8; 4])]) -> Splats {
    Splats {
        positions: list.iter().map(|s| s.0).collect(),
        scales: list.iter().map(|s| Vec3::splat(s.1)).collect(),
        rotations: vec![Quat::IDENTITY; list.len()],
        colors: list.iter().map(|s| s.2).collect(),
    }
}

struct Scene<'a> {
    splats: &'a Splats,
    model: Mat4,
    crop: Option<Aabb>,
    /// A clay sphere of this radius at this point, drawn first.
    clay: Option<(Vec3, f32)>,
    opacity: f32,
    flat: bool,
    over: bool,
}

impl<'a> Scene<'a> {
    fn new(splats: &'a Splats) -> Self {
        Self {
            splats,
            model: Mat4::IDENTITY,
            crop: None,
            clay: None,
            opacity: 1.0,
            flat: false,
            over: false,
        }
    }
}

const SIZE: u32 = 96;

fn render(device: &wgpu::Device, queue: &wgpu::Queue, scene: &Scene) -> Vec<u8> {
    let target = ViewTarget::new(device, [SIZE, SIZE]);
    let pass = SplatPass::new(device, scene.splats).expect("splats");
    let camera = OrbitCamera {
        pitch: 0.0,
        ..OrbitCamera::default()
    };
    pass.set_placement(scene.model, scene.crop);
    pass.prepare(
        queue,
        Slot::ReferenceView,
        &camera,
        target.size,
        &SplatLook {
            opacity: scene.opacity,
            flat: scene.flat,
            over: scene.over,
        },
    );
    let clay = ClayPass::new(device);
    let mesh = scene.clay.map(|(centre, radius)| {
        let mut sphere = Mesh::icosphere(3);
        for p in &mut sphere.positions {
            *p = centre + *p * radius;
        }
        GpuMesh::new(device, &sphere)
    });
    clay.prepare(
        queue,
        Slot::ReferenceView,
        &camera,
        target.aspect(),
        &Look::lit(CLAY),
    );
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut rpass = target.begin_pass(&mut encoder, VIEW_BG);
        if let Some(mesh) = &mesh {
            clay.draw(&mut rpass, Slot::ReferenceView, mesh);
        }
        pass.draw(&mut rpass, Slot::ReferenceView);
    }
    let readback = Readback::copy(device, &mut encoder, &target);
    queue.submit([encoder.finish()]);
    readback.map();
    readback.wait(device).expect("read back")
}

fn pixel(data: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

fn close(a: [u8; 4], b: [u8; 4], tolerance: u8) -> bool {
    a.iter()
        .zip(b)
        .take(3)
        .all(|(x, y)| x.abs_diff(y) <= tolerance)
}

const RED: [u8; 4] = [230, 40, 20, 255];
const BLUE: [u8; 4] = [20, 60, 240, 255];

#[test]
fn a_single_splat_shows_its_colour_at_its_centre_and_fades_out() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let s = splats(&[(Vec3::ZERO, 0.4, RED)]);
    let data = render(&device, &queue, &Scene::new(&s));
    let c = SIZE / 2;
    assert!(
        close(pixel(&data, c, c), RED, 3),
        "{:?}",
        pixel(&data, c, c)
    );
    // Gaussian falloff: dimmer away from the centre, gone at the corner.
    let side = pixel(&data, c + 12, c);
    assert!(side[0] < RED[0] - 20 && side[0] > 20, "{side:?}");
    assert_eq!(pixel(&data, 1, 1), VIEW_BG.to_array());
}

#[test]
fn nearer_splats_cover_farther_ones_whatever_the_order_given() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    // The camera is on +Z: z = 1 is nearer.
    for list in [
        [
            (Vec3::new(0.0, 0.0, -1.0), 0.4, RED),
            (Vec3::new(0.0, 0.0, 1.0), 0.4, BLUE),
        ],
        [
            (Vec3::new(0.0, 0.0, 1.0), 0.4, BLUE),
            (Vec3::new(0.0, 0.0, -1.0), 0.4, RED),
        ],
    ] {
        let s = splats(&list);
        let data = render(&device, &queue, &Scene::new(&s));
        let centre = pixel(&data, SIZE / 2, SIZE / 2);
        assert!(close(centre, BLUE, 3), "{centre:?}");
    }
}

#[test]
fn clay_in_front_hides_a_splat_and_clay_behind_does_not() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let s = splats(&[(Vec3::ZERO, 0.4, RED)]);
    let c = SIZE / 2;
    let front = render(
        &device,
        &queue,
        &Scene {
            clay: Some((Vec3::new(0.0, 0.0, 1.5), 0.3)),
            ..Scene::new(&s)
        },
    );
    let p = pixel(&front, c, c);
    assert!(p[0].abs_diff(p[2]) < 10, "clay grey, not red: {p:?}");
    assert!(p[0] > 60, "lit clay: {p:?}");
    let behind = render(
        &device,
        &queue,
        &Scene {
            clay: Some((Vec3::new(0.0, 0.0, -1.5), 0.3)),
            ..Scene::new(&s)
        },
    );
    assert!(
        close(pixel(&behind, c, c), RED, 3),
        "{:?}",
        pixel(&behind, c, c)
    );
}

#[test]
fn placement_crop_ghost_and_silhouette() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let s = splats(&[(Vec3::new(10.0, 0.0, 0.0), 4.0, RED)]);
    let c = SIZE / 2;
    // Centred and scaled down into the clay's space, it shows in the middle.
    let model =
        Mat4::from_scale(Vec3::splat(0.1)) * Mat4::from_translation(Vec3::new(-10.0, 0.0, 0.0));
    let placed = Scene {
        model,
        ..Scene::new(&s)
    };
    let data = render(&device, &queue, &placed);
    assert!(close(pixel(&data, c, c), RED, 3));

    // Cropped away.
    let cropped = Scene {
        crop: Some(Aabb {
            min: Vec3::splat(-1.0),
            max: Vec3::splat(1.0),
        }),
        ..placed
    };
    let data = render(&device, &queue, &cropped);
    assert_eq!(pixel(&data, c, c), VIEW_BG.to_array());

    // Half see-through.
    let ghost = Scene {
        opacity: 0.5,
        model,
        ..Scene::new(&s)
    };
    let data = render(&device, &queue, &ghost);
    let p = pixel(&data, c, c);
    assert!(p[0].abs_diff(RED[0] / 2) <= 3, "{p:?}");

    // Silhouette.
    let flat = Scene {
        flat: true,
        model,
        ..Scene::new(&s)
    };
    let data = render(&device, &queue, &flat);
    assert!(close(pixel(&data, c, c), SILHOUETTE_INK.to_array(), 3));
}

#[test]
fn a_long_splat_is_drawn_along_its_axis() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    // Long in x, thin in y and z.
    let s = Splats {
        positions: vec![Vec3::ZERO],
        scales: vec![Vec3::new(0.6, 0.03, 0.03)],
        rotations: vec![Quat::IDENTITY],
        colors: vec![RED],
    };
    let c = SIZE / 2;
    let data = render(&device, &queue, &Scene::new(&s));
    assert!(pixel(&data, c + 15, c)[0] > 100, "along the axis");
    assert_eq!(pixel(&data, c, c + 15), VIEW_BG.to_array(), "across it");
    // Turned a quarter about z, it stands up.
    let turned = Splats {
        rotations: vec![Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)],
        ..s
    };
    let data = render(&device, &queue, &Scene::new(&turned));
    assert!(pixel(&data, c, c + 15)[0] > 100);
    assert_eq!(pixel(&data, c + 15, c), VIEW_BG.to_array());
}

#[test]
fn a_ghost_shows_over_clay_in_front() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let s = splats(&[(Vec3::ZERO, 0.4, RED)]);
    let c = SIZE / 2;
    let clay_only = render(
        &device,
        &queue,
        &Scene {
            clay: Some((Vec3::new(0.0, 0.0, 1.5), 0.3)),
            opacity: 0.0,
            ..Scene::new(&s)
        },
    );
    let ghost = render(
        &device,
        &queue,
        &Scene {
            clay: Some((Vec3::new(0.0, 0.0, 1.5), 0.3)),
            opacity: 0.5,
            over: true,
            ..Scene::new(&s)
        },
    );
    let (clay, mixed) = (pixel(&clay_only, c, c), pixel(&ghost, c, c));
    // Half red over the grey clay: redder, and less blue.
    assert!(
        mixed[0] > clay[0] && mixed[2] < clay[2],
        "{clay:?} → {mixed:?}"
    );
}
