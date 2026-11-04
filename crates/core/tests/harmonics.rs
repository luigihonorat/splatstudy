// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! View-dependent colour: 3DGS files with spherical harmonics of degree 1
//! and 3 load with them, and a splat drawn with them changes colour with
//! the side it is seen from. The render tests are skipped (with a
//! message) without a GPU adapter.

use std::f32::consts::PI;

use glam::{Mat4, Vec3};
use splatstudy_core::camera::OrbitCamera;
use splatstudy_core::reference::load::load;
use splatstudy_core::reference::{Alignment, Content, Harmonics, Splats, UpAxis};
use splatstudy_core::render::splat::{SortOn, SplatLook, SplatPass};
use splatstudy_core::render::{Slot, ViewTarget, readback::Readback};
use splatstudy_core::ui::theme::{SILHOUETTE_INK, VIEW_BG};

/// Two splats, degree 1, ASCII. The first is grey with red's second
/// coefficient at +1 and blue's at -1; the second has f_rest_k = (k + 1) / 10.
const DEGREE_1: &[u8] = include_bytes!("fixtures/sh-degree1.ply");
/// Two splats, degree 3, binary. f_rest_k = (k - 22) / 50 for the first,
/// negated for the second, whose base colour is above white in red.
const DEGREE_3: &[u8] = include_bytes!("fixtures/sh-degree3.ply");

fn splats(name: &str, bytes: &[u8]) -> Splats {
    match load(name, bytes) {
        Ok(Content::Splats(s)) => s,
        Ok(_) => panic!("{name}: splats expected"),
        Err(e) => panic!("{name}: {e}"),
    }
}

/// Coefficient `j` of channel `c` matches `want` to within a step of the
/// packing (the splat's largest number over 127).
fn assert_coefficients(h: &Harmonics, i: usize, largest: f32, want: impl Fn(usize, usize) -> f32) {
    let (_, coefficients) = h.unpack(i);
    assert_eq!(coefficients.len(), Harmonics::coefficients(h.degree));
    let step = largest / 127.0 + 1e-3;
    for (j, k) in coefficients.iter().enumerate() {
        for (c, got) in k.iter().enumerate() {
            let want = want(j, c);
            assert!(
                (got - want).abs() <= step,
                "splat {i}, {j}, {c}: {got} vs {want}"
            );
        }
    }
}

#[test]
fn degree_1_from_an_ascii_ply() {
    let s = splats("degree1.ply", DEGREE_1);
    assert_eq!(s.len(), 2);
    assert_eq!(s.harmonics.degree, 1);
    assert_eq!(s.harmonics.len(), 2);
    // f_rest holds all of red's coefficients, then green's, then blue's.
    assert_coefficients(&s.harmonics, 0, 1.0, |j, c| match (j, c) {
        (1, 0) => 1.0,
        (1, 2) => -1.0,
        _ => 0.0,
    });
    assert_coefficients(&s.harmonics, 1, 0.9, |j, c| (c * 3 + j + 1) as f32 / 10.0);
    // The base colour is what it was without them.
    assert_eq!(s.colors[0], [128, 128, 128, 253]);
}

#[test]
fn degree_3_from_a_binary_ply() {
    let s = splats("degree3.ply", DEGREE_3);
    assert_eq!(s.len(), 2);
    assert_eq!(s.harmonics.degree, 3);
    let rest = |j: usize, c: usize| (c * 15 + j) as f32 / 50.0 - 22.0 / 50.0;
    assert_coefficients(&s.harmonics, 0, 0.46, rest);
    assert_coefficients(&s.harmonics, 1, 0.46, |j, c| -rest(j, c));
    // Red's base colour, 0.5 + 0.2821 · 3, is kept as white in its byte;
    // what is above white stays with the harmonics.
    assert_eq!(s.colors[1][0], 255);
    let (residual, _) = s.harmonics.unpack(1);
    assert!((residual[0] - 0.346).abs() < 0.01, "{residual:?}");
}

#[test]
fn truncated_or_odd_files_are_errors_or_plain() {
    for end in 0..DEGREE_3.len() {
        assert!(
            load("degree3.ply", &DEGREE_3[..end]).is_err(),
            "cut at {end}"
        );
    }
    // A whole row missing from the text.
    let text = std::str::from_utf8(DEGREE_1).unwrap();
    let short = &text[..text.trim_end().rfind('\n').unwrap() + 1];
    assert!(load("degree1.ply", short.as_bytes()).is_err());
    // A set of f_rest that isn't 9, 24 or 45 is no view-dependent colour.
    let odd = text.replace("property float f_rest_8\n", "property float f_rest_x\n");
    let s = splats("odd.ply", odd.as_bytes());
    assert_eq!(s.harmonics.degree, 0);
    assert!(s.harmonics.words.is_empty());
    // Neither is one with a gap in its numbering.
    let gap = text.replace("property float f_rest_8\n", "property float f_rest_9\n");
    assert_eq!(splats("gap.ply", gap.as_bytes()).harmonics.degree, 0);
}

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

const SIZE: u32 = 64;

/// The camera from the front (on +z, looking along -z), or turned to
/// look from behind.
fn camera(behind: bool) -> OrbitCamera {
    OrbitCamera {
        pitch: 0.0,
        yaw: if behind { PI } else { 0.0 },
        ..OrbitCamera::default()
    }
}

/// Draws the slot as `pass` was prepared for it.
fn draw(device: &wgpu::Device, queue: &wgpu::Queue, pass: &SplatPass, slot: Slot) -> Vec<u8> {
    let target = ViewTarget::new(device, [SIZE, SIZE]);
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut rpass = target.begin_pass(&mut encoder, VIEW_BG);
        pass.draw(&mut rpass, slot);
    }
    let readback = Readback::copy(device, &mut encoder, &target);
    queue.submit([encoder.finish()]);
    readback.map();
    readback.wait(device).expect("read back")
}

struct Shot<'a> {
    model: Mat4,
    camera: OrbitCamera,
    look: SplatLook,
    at: [u32; 2],
    pass: &'a SplatPass,
}

impl Shot<'_> {
    /// The pixel at `at`, with the slot prepared and drawn alone.
    fn pixel(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> [u8; 4] {
        self.pass.set_placement(self.model, None);
        let size = [SIZE, SIZE];
        self.pass
            .prepare(queue, Slot::ReferenceView, &self.camera, size, &self.look);
        let data = draw(device, queue, self.pass, Slot::ReferenceView);
        pixel(&data, self.at)
    }
}

fn pixel(data: &[u8], [x, y]: [u32; 2]) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

const CENTRE: [u32; 2] = [SIZE / 2, SIZE / 2];

/// The first splat of the degree-1 fixture alone: grey, with red's
/// coefficient for z at +1 and blue's at -1. From the front (looking
/// along -z) it is blue, from behind red.
fn two_faced() -> Splats {
    let s = splats("degree1.ply", DEGREE_1);
    let mut harmonics = Harmonics::new(1);
    harmonics.words = s.harmonics.splat(0).to_vec();
    Splats {
        positions: vec![Vec3::ZERO],
        scales: vec![s.scales[0]],
        rotations: vec![s.rotations[0]],
        colors: vec![s.colors[0]],
        harmonics,
    }
}

// The pixel centres are a little off the splat's, which covers them at
// about 95 %: the grey is about 122, not 128, over black.

fn is_blue(p: [u8; 4]) -> bool {
    p[2] > 200 && u16::from(p[2]) > u16::from(p[1]) * 3 / 2 && p[0] < 20
}

fn is_red(p: [u8; 4]) -> bool {
    p[0] > 200 && u16::from(p[0]) > u16::from(p[1]) * 3 / 2 && p[2] < 20
}

fn is_grey(p: [u8; 4]) -> bool {
    p[0].abs_diff(p[1]) <= 2 && p[1].abs_diff(p[2]) <= 2 && (110..=130).contains(&p[1])
}

#[test]
fn a_splat_changes_colour_from_one_side_to_the_other() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let s = two_faced();
    for sort in [SortOn::Gpu, SortOn::Cpu] {
        let pass = SplatPass::sorting_on(&device, &s, sort).expect("splats");
        assert_eq!(pass.view_dependent_degree(), 1);
        let shot = |behind: bool, look: SplatLook| Shot {
            model: Mat4::IDENTITY,
            camera: camera(behind),
            look,
            at: CENTRE,
            pass: &pass,
        };
        let on = SplatLook::default();
        let front = shot(false, on).pixel(&device, &queue);
        let back = shot(true, on).pixel(&device, &queue);
        assert!(is_blue(front), "{sort:?}: front {front:?}");
        assert!(is_red(back), "{sort:?}: back {back:?}");

        // Turned off: the base colour, grey from both sides.
        let off = SplatLook {
            view_dependent: false,
            ..on
        };
        for behind in [false, true] {
            let p = shot(behind, off).pixel(&device, &queue);
            assert!(is_grey(p), "{p:?}");
        }
        // The silhouette ignores it.
        let flat = SplatLook { flat: true, ..on };
        for behind in [false, true] {
            let p = shot(behind, flat).pixel(&device, &queue);
            // The ink, as much of it as covers the pixel.
            let ink = SILHOUETTE_INK
                .to_array()
                .map(|v| (u32::from(v) * 95 / 100) as u8);
            assert!((0..3).all(|c| p[c].abs_diff(ink[c]) <= 4), "{p:?}");
        }
    }
}

/// Without view-dependent colour a splat draws exactly as before; with it
/// turned off, a splat that has some draws exactly like one that hasn't.
#[test]
fn base_colour_only_is_unchanged() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let with = two_faced();
    let without = Splats {
        harmonics: Harmonics::default(),
        ..with.clone()
    };
    let plain = SplatPass::sorting_on(&device, &without, SortOn::Cpu).expect("splats");
    assert_eq!(plain.view_dependent_degree(), 0);
    let turned_off = SplatPass::sorting_on(&device, &with, SortOn::Cpu).expect("splats");
    for behind in [false, true] {
        let image = |pass: &SplatPass, look: SplatLook| {
            pass.set_placement(Mat4::IDENTITY, None);
            pass.prepare(
                &queue,
                Slot::ReferenceView,
                &camera(behind),
                [SIZE; 2],
                &look,
            );
            draw(&device, &queue, pass, Slot::ReferenceView)
        };
        let a = image(&plain, SplatLook::default());
        let b = image(
            &turned_off,
            SplatLook {
                view_dependent: false,
                ..Default::default()
            },
        );
        assert!(a == b, "the same pixels");
        let p = pixel(&a, CENTRE);
        assert!(is_grey(p), "{p:?}");
    }
}

/// The view is turned into the file's own coordinates: turning the
/// reference (its up axis, or about the vertical) turns its colours with
/// it.
#[test]
fn the_alignment_turns_the_colours_with_the_splats() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let s = two_faced();
    let pass = SplatPass::sorting_on(&device, &s, SortOn::Cpu).expect("splats");
    let upside_down = Alignment {
        up: UpAxis::NegY,
        ..Default::default()
    };
    let turned = Alignment {
        yaw: PI,
        scale: 0.5,
        ..Default::default()
    };
    for align in [upside_down, turned] {
        let shot = |behind| Shot {
            model: align.matrix(),
            camera: camera(behind),
            look: SplatLook::default(),
            at: CENTRE,
            pass: &pass,
        };
        // The side that was the back now faces the front.
        let front = shot(false).pixel(&device, &queue);
        let back = shot(true).pixel(&device, &queue);
        assert!(is_red(front), "{align:?}: front {front:?}");
        assert!(is_blue(back), "{align:?}: back {back:?}");
    }
}

/// In an orthographic view every splat is seen along the view's own
/// direction; in perspective, along the line from the eye.
#[test]
fn orthographic_views_see_every_splat_from_the_same_direction() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    // Red goes with -x: grey seen straight on, less red seen from the left
    // of the splat (the eye's direction to it leans to +x).
    let mut harmonics = Harmonics::new(1);
    harmonics.push(
        [0.5; 3],
        [128; 3],
        |j, c| if (j, c) == (2, 0) { 1.0 } else { 0.0 },
    );
    let s = Splats {
        positions: vec![Vec3::new(0.8, 0.0, 0.0)],
        scales: vec![Vec3::splat(0.1)],
        rotations: vec![glam::Quat::IDENTITY],
        colors: vec![[128, 128, 128, 255]],
        harmonics,
    };
    let pass = SplatPass::sorting_on(&device, &s, SortOn::Cpu).expect("splats");
    // 0.8 units right of the target: SIZE / 2 / half-height pixels a unit.
    let half = camera(false).half_height();
    let x = (SIZE as f32 / 2.0 * (1.0 + 0.8 / half)) as u32;
    let shot = |ortho| Shot {
        model: Mat4::IDENTITY,
        camera: OrbitCamera {
            ortho,
            ..camera(false)
        },
        look: SplatLook::default(),
        at: [x, SIZE / 2],
        pass: &pass,
    };
    let ortho = shot(true).pixel(&device, &queue);
    let perspective = shot(false).pixel(&device, &queue);
    assert!(is_grey(ortho), "{ortho:?}");
    // 0.5 - C1 · 0.146: about 18 levels less red than green.
    let less = perspective[1] - perspective[0];
    assert!((12..=24).contains(&less), "{perspective:?}");
}

/// The ghost over the sculpt and the reference view draw the same splats
/// with cameras of their own, each with its own colours.
#[test]
fn each_slot_sees_the_splats_from_its_own_camera() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping render test");
        return;
    };
    let s = two_faced();
    let pass = SplatPass::sorting_on(&device, &s, SortOn::Gpu).expect("splats");
    pass.set_placement(Mat4::IDENTITY, None);
    let look = SplatLook::default();
    pass.prepare(&queue, Slot::SculptView, &camera(false), [SIZE; 2], &look);
    pass.prepare(&queue, Slot::ReferenceView, &camera(true), [SIZE; 2], &look);
    let sculpt = pixel(&draw(&device, &queue, &pass, Slot::SculptView), CENTRE);
    let reference = pixel(&draw(&device, &queue, &pass, Slot::ReferenceView), CENTRE);
    assert!(is_blue(sculpt), "{sculpt:?}");
    assert!(is_red(reference), "{reference:?}");
}
