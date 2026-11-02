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
            clock: None,
            title: None,
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
                    clock: Some("1:23"),
                    title: Some(("title", 1.0)),
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

#[test]
fn a_picture_fills_the_reference_half_keeping_its_proportions() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping export test");
        return;
    };
    let size = Shape::Wide.size(540);
    let mut compositor = Compositor::new(&device, size, Layout::SideBySide);
    // Two pixels side by side: red, then blue.
    compositor.set_image(&splatstudy_core::reference::RefImage {
        size: [2, 1],
        rgba: vec![255, 0, 0, 255, 0, 0, 255, 255],
    });
    let frame = compose(
        &device,
        &queue,
        &mut compositor,
        &Overlay::Views {
            credit: None,
            clock: None,
            title: None,
        },
    );
    frame.save("picture.png");
    let [w, h] = size;
    let centre = w / 2 + 1 + (w / 2 - 1) / 2;
    let red = frame.at(centre - 120, h / 2);
    let blue = frame.at(centre + 120, h / 2);
    assert!(red[0] > 200 && red[2] < 50, "{red:?}");
    assert!(blue[2] > 200 && blue[0] < 50, "{blue:?}");
    // Twice as wide as high: black above and below.
    assert_eq!(frame.at(centre, 40), VIEW_BG.to_array());
    assert_eq!(frame.at(centre, h - 40), VIEW_BG.to_array());
}

#[test]
fn the_clock_and_the_title_go_in_their_corners() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping export test");
        return;
    };
    let size = Shape::Wide.size(540);
    let mut compositor = Compositor::new(&device, size, Layout::SideBySide);
    let mut frame = |clock, title| {
        let overlay = Overlay::Views {
            credit: None,
            clock,
            title,
        };
        compose(&device, &queue, &mut compositor, &overlay)
    };
    let plain = frame(None, None);
    let marked = frame(Some("12:34"), Some(("Brutus の塑像", 1.0)));
    let faded = frame(None, Some(("Brutus の塑像", 0.3)));
    marked.save("clock-and-title.png");
    let [w, h] = size;
    // The brightest red in a box: the text is grey, so any channel will do.
    let brightest = |f: &Frame, x: std::ops::Range<u32>, y: std::ops::Range<u32>| {
        y.flat_map(|y| x.clone().map(move |x| (x, y)))
            .map(|(x, y)| f.at(x, y)[0])
            .max()
            .unwrap()
    };
    // The clock in the sculpt's bottom left corner, across from where the
    // credit goes; nothing there without it.
    let (clock_x, clock_y) = (8..90, h - 34..h - 6);
    assert_eq!(plain.inked(clock_x.clone(), clock_y.clone()), 0);
    assert!(marked.inked(clock_x, clock_y) > 30);
    assert_eq!(marked.inked(w / 2 + 8..w / 2 + 90, h - 34..h - 6), 0);
    // The title in the frame's top left corner, dimmer as it fades.
    let (title_x, title_y) = (16..260, 12..60);
    assert_eq!(plain.inked(title_x.clone(), title_y.clone()), 0);
    assert!(marked.inked(title_x.clone(), title_y.clone()) > 100);
    let (full, dim) = (
        brightest(&marked, title_x.clone(), title_y.clone()),
        brightest(&faded, title_x, title_y),
    );
    assert!(dim > 0 && dim < full / 2, "{dim} vs {full}");
}

mod job {
    use glam::Vec3;
    use splatstudy_core::export::{
        CameraMotion, Captions, EncodedFrame, Gpu, Job, Moment, Progress, Settings, Sink, Support,
        VideoConfig, VideoEncoder, mp4::Mp4, zip::Zip,
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
        let _ = start;
        Replay::of(&s)
    }

    fn settings() -> Settings {
        Settings {
            layout: Layout::SideBySide,
            shape: Shape::Wide,
            short_side: 90,
            fps: 10,
            speed: 1.0,
            look: Look::lit(CLAY),
            ..Settings::default()
        }
    }

    fn captions(credit: Option<&str>) -> Captions {
        Captions {
            credit: credit.map(str::to_owned),
            ..Captions::default()
        }
    }

    /// Steps a job to the end like the app does, frame after frame.
    fn run(
        job: &mut Job,
        gpu: &Gpu,
        encoder: Option<&mut FakeEncoder>,
    ) -> (Progress, Vec<(String, Vec<u8>)>) {
        let (progress, writes, draws) = run_drawing(job, gpu, encoder);
        assert!(
            draws.iter().any(|d| d.0 == Slot::ExportReference),
            "the reference half was drawn"
        );
        (progress, writes)
    }

    /// Each time the reference was drawn: the slot and the camera's yaw.
    type Draws = Vec<(Slot, f32)>;

    /// [`run`], also giving every draw of the reference in order.
    fn run_drawing(
        job: &mut Job,
        gpu: &Gpu,
        mut encoder: Option<&mut FakeEncoder>,
    ) -> (Progress, Vec<(String, Vec<u8>)>, Draws) {
        let mut writes = Vec::new();
        let mut draws = Vec::new();
        // Steps don't wait for the GPU, so this spins; give it time when
        // other tests share the GPU.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while std::time::Instant::now() < deadline {
            let encoder = encoder.as_deref_mut().map(|e| e as &mut dyn VideoEncoder);
            let progress = job.step(
                gpu,
                encoder,
                &mut |_, slot, camera, size| {
                    assert!(size[0] > 0 && size[1] > 0);
                    draws.push((slot, camera.yaw));
                },
                &mut writes,
            );
            if !matches!(progress, Progress::Working { .. }) {
                return (progress, writes, draws);
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
            captions(Some("credit")),
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
                captions(None),
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

    #[test]
    fn the_ghost_and_the_turn_at_the_end_reach_both_halves() {
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
        let settings = Settings {
            ghost: true,
            turntable: true,
            turn_reference: true,
            clock: true,
            ..settings()
        };
        let replay = replay();
        let timeline = settings.timeline(replay.duration());
        let mut job = Job::new(
            &device,
            settings,
            replay,
            [OrbitCamera::default(); 2],
            Captions {
                title: Some("study".into()),
                ..captions(Some("credit"))
            },
            "study".into(),
            Sink::Folder("out".into()),
            None,
        )
        .unwrap();
        assert_eq!(job.timeline(), timeline);
        assert_eq!(job.total(), timeline.total());
        let (progress, writes, draws) = run_drawing(&mut job, &gpu, None);
        assert!(matches!(progress, Progress::Written { .. }), "{progress:?}");

        // Every frame but the card's lays the ghost over the clay, then
        // draws the reference half, both from the same turned camera.
        let shown = (timeline.total() - timeline.card) as usize;
        assert_eq!(draws.len(), 2 * shown);
        for (i, pair) in draws.chunks(2).enumerate() {
            assert_eq!(
                [pair[0].0, pair[1].0],
                [Slot::ExportSculpt, Slot::ExportReference]
            );
            assert_eq!(pair[0].1, pair[1].1, "the halves turn together");
            let turn = settings.turn_at(&timeline, i as u64);
            assert!((pair[0].1 - turn).abs() < 1e-6, "frame {i}");
        }
        // Still through the timelapse and the hold, round in the turn.
        let hold = timeline.timelapse as usize;
        let middle = hold + timeline.hold as usize + timeline.turn as usize / 2;
        assert_eq!(timeline.at(middle as u64), Moment::Turn(0.5));
        assert!(draws[..2 * (hold + 1)].iter().all(|d| d.1 == 0.0));
        assert!((draws[2 * middle].1 - std::f32::consts::PI).abs() < 1e-5);

        // The frames show it: the hold doesn't change, the turn does.
        let pixels = |i: usize| {
            let mut reader = png::Decoder::new(std::io::Cursor::new(&writes[i].1))
                .read_info()
                .unwrap();
            let mut out = vec![0; reader.output_buffer_size().unwrap()];
            reader.next_frame(&mut out).unwrap();
            out
        };
        assert_eq!(pixels(hold), pixels(hold + 1));
        assert_ne!(pixels(hold), pixels(middle));
    }

    #[test]
    fn unlinked_the_reference_keeps_still_while_the_clay_turns() {
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
        let settings = Settings {
            camera: CameraMotion::Turntable,
            ..settings()
        };
        let replay = replay();
        let timeline = settings.timeline(replay.duration());
        let mut job = Job::new(
            &device,
            settings,
            replay,
            [OrbitCamera::default(); 2],
            captions(None),
            "study".into(),
            Sink::Zip(Zip::new()),
            None,
        )
        .unwrap();
        let (_, _, draws) = run_drawing(&mut job, &gpu, None);
        // No ghost: only the reference half is drawn, and never turned.
        let shown = (timeline.total() - timeline.card) as usize;
        assert_eq!(draws.len(), shown);
        assert!(
            draws
                .iter()
                .all(|d| d.0 == Slot::ExportReference && d.1 == 0.0)
        );
        // The clay turned during the timelapse all the same.
        let middle = timeline.timelapse / 2;
        assert!(settings.turn_at(&timeline, middle) > 1.0);
    }
}
