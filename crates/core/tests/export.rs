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

mod job {
    use glam::Vec3;
    use splatstudy_core::export::{
        EncodedFrame, Gpu, Job, Progress, Settings, Sink, Support, VideoConfig, VideoEncoder,
        mp4::Mp4, zip::Zip,
    };
    use splatstudy_core::sculpt::{Brush, Dab, Replay, Sculpt};

    use super::*;

    /// Stands in for the browser's encoder: each frame becomes a short
    /// "access unit" holding its index and the first pixel, a frame later,
    /// as real encoders lag.
    #[derive(Default)]
    struct FakeEncoder {
        config: Option<VideoConfig>,
        queue: Vec<EncodedFrame>,
        out: Vec<EncodedFrame>,
        finished: bool,
        frames: u64,
    }

    impl VideoEncoder for FakeEncoder {
        fn support(&self) -> Support {
            Support::Yes
        }
        fn start(&mut self, config: &VideoConfig) -> Result<(), String> {
            self.config = Some(*config);
            Ok(())
        }
        fn pending(&self) -> usize {
            self.queue.len()
        }
        fn encode(&mut self, rgba: &[u8], index: u64, key: bool) -> Result<(), String> {
            let c = self.config.unwrap();
            assert_eq!(rgba.len(), (c.width * c.height * 4) as usize);
            assert_eq!(index, self.frames, "frames arrive in order");
            self.frames += 1;
            let mut data = 9u32.to_be_bytes().to_vec();
            data.extend_from_slice(&(index as u32).to_be_bytes());
            data.extend_from_slice(&rgba[..4]);
            data.push(0x65);
            self.out.append(&mut self.queue);
            self.queue.push(EncodedFrame { data, key });
            Ok(())
        }
        fn finish(&mut self) {
            self.out.append(&mut self.queue);
            self.finished = true;
        }
        fn take(&mut self) -> Vec<EncodedFrame> {
            std::mem::take(&mut self.out)
        }
        fn config(&self) -> Option<Vec<u8>> {
            (self.frames > 0).then(|| vec![1, 0x64, 0, 0x28, 0xFF, 0xE0, 0])
        }
        fn done(&self) -> bool {
            self.finished
        }
        fn error(&self) -> Option<String> {
            None
        }
        fn cancel(&mut self) {}
    }

    /// Three seconds of strokes on a small sphere.
    fn replay() -> Replay {
        let start = Mesh::icosphere(3);
        let mut s = Sculpt::new(start.clone());
        for k in 0..6 {
            s.set_clock(k as f64 * 0.5);
            s.begin_stroke();
            s.dab(Dab {
                brush: Brush::Clay,
                centre: Vec3::new(0.2 * k as f32 - 0.5, 0.3, 1.0).normalize(),
                radius: 0.4,
                strength: 1.0,
                invert: false,
                symmetry: true,
                direction: Vec3::ZERO,
            });
            s.end_stroke();
            s.refresh();
        }
        Replay::new(start, s.log().to_vec())
    }

    fn settings() -> Settings {
        Settings {
            layout: Layout::SideBySide,
            shape: Shape::Wide,
            short_side: 90,
            fps: 10,
            speed: 1.0,
        }
    }

    /// Steps a job to the end like the app does, frame after frame.
    fn run(
        job: &mut Job,
        gpu: &Gpu,
        mut encoder: Option<&mut FakeEncoder>,
    ) -> (Progress, Vec<(String, Vec<u8>)>) {
        let mut writes = Vec::new();
        let mut reference_draws = 0;
        // Steps don't wait for the GPU, so this spins; give it time when
        // other tests share the GPU.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while std::time::Instant::now() < deadline {
            let encoder = encoder.as_deref_mut().map(|e| e as &mut dyn VideoEncoder);
            let progress = job.step(
                gpu,
                encoder,
                &mut |_, _, aspect| {
                    assert!(aspect > 0.0);
                    reference_draws += 1;
                },
                &mut writes,
            );
            if !matches!(progress, Progress::Working { .. }) {
                assert!(reference_draws > 0, "the reference half was drawn");
                return (progress, writes);
            }
        }
        panic!("the export never finished");
    }

    fn be32(d: &[u8], at: usize) -> usize {
        u32::from_be_bytes(d[at..at + 4].try_into().unwrap()) as usize
    }

    #[test]
    fn a_video_export_muxes_every_frame_in_order() {
        let Some((device, queue)) = device() else {
            eprintln!("no GPU adapter; skipping export test");
            return;
        };
        let clay = ClayPass::new(&device);
        let gpu = Gpu {
            device: &device,
            queue: &queue,
            clay: &clay,
        };
        let replay = replay();
        let settings = settings();
        let expected = settings.timelapse_frames(replay.duration()) + 4 * 10;
        let mut encoder = FakeEncoder::default();
        let mut job = Job::new(
            &device,
            settings,
            replay,
            [OrbitCamera::default(); 2],
            Some("credit".into()),
            EndCard::default(),
            "study".into(),
            Sink::Video(Mp4::new(160, 90, 10)),
            Some(&mut encoder),
        )
        .unwrap();
        assert_eq!(job.total(), expected);
        assert_eq!(encoder.config.unwrap().width, 160);
        let (progress, _) = run(&mut job, &gpu, Some(&mut encoder));
        let Progress::File { name, mime, bytes } = progress else {
            panic!("{progress:?}");
        };
        assert_eq!((name.as_str(), mime), ("study.mp4", "video/mp4"));
        assert_eq!(encoder.frames, expected);
        // The sample size table lists every frame, each 13 bytes.
        let stsz = bytes.windows(4).position(|w| w == b"stsz").unwrap();
        // After the type: version and flags, a size for all (0: they vary),
        // the count, then each size.
        assert_eq!(be32(&bytes, stsz + 8), 0);
        assert_eq!(be32(&bytes, stsz + 12), expected as usize);
        assert_eq!(be32(&bytes, stsz + 16), 13);
        // In mdat, the frames follow each other in order.
        let mdat = bytes.windows(4).position(|w| w == b"mdat").unwrap() + 4;
        for i in 0..expected as usize {
            assert_eq!(be32(&bytes, mdat + 13 * i + 4), i);
        }
        // The last frames are the end card: black where the clay was.
        let last = mdat + 13 * (expected as usize - 1);
        assert_eq!(&bytes[last + 8..last + 11], &[0, 0, 0]);
    }

    #[test]
    fn image_exports_zip_or_write_every_frame() {
        let Some((device, queue)) = device() else {
            eprintln!("no GPU adapter; skipping export test");
            return;
        };
        let clay = ClayPass::new(&device);
        let gpu = Gpu {
            device: &device,
            queue: &queue,
            clay: &clay,
        };
        let new_job = |sink| {
            Job::new(
                &device,
                settings(),
                replay(),
                [OrbitCamera::default(); 2],
                None,
                EndCard::default(),
                "study".into(),
                sink,
                None,
            )
            .unwrap()
        };

        let mut job = new_job(Sink::Zip(Zip::new()));
        let total = job.total() as usize;
        let (progress, writes) = run(&mut job, &gpu, None);
        assert!(writes.is_empty());
        let Progress::File { name, bytes, .. } = progress else {
            panic!("{progress:?}");
        };
        assert_eq!(name, "study.zip");
        let end = bytes.len() - 22;
        assert_eq!(
            u16::from_le_bytes([bytes[end + 10], bytes[end + 11]]) as usize,
            total
        );

        let mut job = new_job(Sink::Folder("out".into()));
        let (progress, writes) = run(&mut job, &gpu, None);
        assert!(matches!(progress, Progress::Written { frames } if frames as usize == total));
        assert_eq!(writes.len(), total);
        let first = std::path::Path::new(&writes[0].0);
        assert_eq!(first, std::path::Path::new("out").join("study_00000.png"));
        assert_eq!(&writes[0].1[1..4], b"PNG");
    }
}
