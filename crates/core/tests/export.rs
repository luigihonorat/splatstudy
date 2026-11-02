// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Composing exported frames offscreen and reading them back. Skipped
//! (with a message) on machines without any GPU adapter.
//!
//! Set `SPLATSTUDY_FRAMES` to a folder to also get the frames as PNGs,
//! for looking at them.

use splatstudy_core::camera::OrbitCamera;
use splatstudy_core::export::{self, Compositor, EndCard, Layout, Overlay, Shape};
use splatstudy_core::render::clay::{ClayPass, GpuMesh, Look};
use splatstudy_core::render::{Slot, readback::Readback};
use splatstudy_core::sculpt::Mesh;
use splatstudy_core::ui::theme::{self, CLAY, SILHOUETTE_INK, VIEW_BG};

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

struct Frame {
    size: [u32; 2],
    pixels: Vec<u8>,
}

impl Frame {
    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.size[0] + x) * 4) as usize;
        self.pixels[i..i + 4].try_into().unwrap()
    }

    /// Pixels in a box that aren't the background.
    fn inked(&self, x: std::ops::Range<u32>, y: std::ops::Range<u32>) -> usize {
        let bg = VIEW_BG.to_array();
        y.flat_map(|y| x.clone().map(move |x| (x, y)))
            .filter(|&(x, y)| self.at(x, y) != bg)
            .count()
    }

    fn save(&self, name: &str) {
        if let Ok(dir) = std::env::var("SPLATSTUDY_FRAMES") {
            let png = export::png(&self.pixels, self.size).unwrap();
            std::fs::write(std::path::Path::new(&dir).join(name), png).unwrap();
        }
    }
}

/// The clay in the sculpt half and a flat stand-in for the reference in
/// the other, composed with `overlay`.
fn compose(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    compositor: &mut Compositor,
    overlay: &Overlay,
) -> Frame {
    let clay = ClayPass::new(device);
    let mesh = GpuMesh::new(device, &Mesh::icosphere(3));
    let camera = OrbitCamera::default();
    let mut encoder = device.create_command_encoder(&Default::default());
    let sculpt = compositor.sculpt_view();
    clay.prepare(
        queue,
        Slot::ExportSculpt,
        &camera,
        sculpt.aspect(),
        &Look::lit(CLAY),
    );
    {
        let mut pass = sculpt.begin_pass(&mut encoder, VIEW_BG);
        clay.draw(&mut pass, Slot::ExportSculpt, &mesh);
    }
    if let Some(reference) = compositor.reference_view() {
        let look = Look {
            flat: true,
            ..Look::lit(SILHOUETTE_INK)
        };
        clay.prepare(
            queue,
            Slot::ExportReference,
            &camera,
            reference.aspect(),
            &look,
        );
        let mut pass = reference.begin_pass(&mut encoder, VIEW_BG);
        clay.draw(&mut pass, Slot::ExportReference, &mesh);
    }
    let mut buffers = compositor.compose(device, queue, &mut encoder, overlay);
    let readback = Readback::copy(device, &mut encoder, compositor.output());
    buffers.push(encoder.finish());
    queue.submit(buffers);
    readback.map();
    Frame {
        size: compositor.size(),
        pixels: readback.wait(device).expect("read back"),
    }
}

#[test]
fn side_by_side_frame_has_both_views_the_rule_and_the_credit() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping export test");
        return;
    };
    let size = Shape::Wide.size(540);
    assert_eq!(size, [960, 540]);
    let mut compositor = Compositor::new(&device, size, Layout::SideBySide);
    let credit = "Bust of a woman — A. Museum · CC BY 4.0";
    let frame = compose(
        &device,
        &queue,
        &mut compositor,
        &Overlay::Views {
            credit: Some(credit),
        },
    );
    frame.save("side-by-side.png");
    let [w, h] = size;
    let half = w / 2;
    // The rule, one pixel wide, between two black edges.
    let rule = theme::DARK.border.to_array();
    assert_eq!(frame.at(half, h / 3), rule);
    assert_eq!(frame.at(half - 1, h / 3), VIEW_BG.to_array());
    assert_eq!(frame.at(half + 1, h / 3), VIEW_BG.to_array());
    // Lit clay on the left, the flat stand-in on the right.
    let left = frame.at(half / 2, h / 2);
    let right = frame.at(half + half / 2, h / 2);
    assert_ne!(left, VIEW_BG.to_array());
    let ink = SILHOUETTE_INK.to_array();
    assert!(
        right.iter().zip(ink).all(|(a, b)| a.abs_diff(b) <= 1),
        "{right:?}"
    );
    // The credit is written near the reference's bottom left corner, and
    // nowhere under the sculpt.
    assert!(frame.inked(half + 20..half + 200, h - 40..h - 15) > 50);
    assert_eq!(frame.inked(20..200, h - 40..h - 15), 0);
}

#[test]
fn every_shape_and_layout_composes() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping export test");
        return;
    };
    for shape in Shape::ALL {
        for layout in Layout::ALL {
            let size = shape.size(360);
            let mut compositor = Compositor::new(&device, size, layout);
            let frame = compose(
                &device,
                &queue,
                &mut compositor,
                &Overlay::Views {
                    credit: Some("credit"),
                },
            );
            assert_eq!(frame.pixels.len(), (size[0] * size[1] * 4) as usize);
            let p = compositor.placement();
            // The sculpt's centre shows clay.
            let s = p.sculpt;
            assert_ne!(
                frame.at(s.x + s.w / 2, s.y + s.h / 2),
                VIEW_BG.to_array(),
                "{shape:?} {layout:?}"
            );
            frame.save(&format!("{shape:?}-{layout:?}.png"));
        }
    }
}

#[test]
fn end_card_shows_the_title_and_the_footer() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping export test");
        return;
    };
    let size = Shape::Wide.size(540);
    let mut compositor = Compositor::new(&device, size, Layout::SideBySide);
    let card = EndCard {
        title: "Bust of a woman".into(),
        lines: vec![
            "A. Museum".into(),
            "CC BY 4.0".into(),
            "https://example.org/bust".into(),
        ],
        footer: "made with splatstudy · splatstudy.com".into(),
    };
    let frame = compose(&device, &queue, &mut compositor, &Overlay::EndCard(&card));
    frame.save("end-card.png");
    let [w, h] = size;
    // Text in the middle and at the bottom; the views are gone.
    assert!(frame.inked(w / 4..3 * w / 4, h / 2 - 70..h / 2 + 40) > 200);
    assert!(frame.inked(w / 3..2 * w / 3, h - 50..h - 30) > 50);
    assert_eq!(frame.inked(0..w / 6, 0..h / 4), 0);
}
