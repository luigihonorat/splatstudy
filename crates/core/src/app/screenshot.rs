// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! A screenshot of the views as they look now, saved as a PNG: each view
//! drawn again offscreen at the picture's size, with the same cameras and
//! the aids that belong in a picture (silhouette, lighting, wireframe,
//! ghost, mirror, grid, plumb line), then put side by side by the
//! timelapse export's compositor, with the reference's credit. The
//! toolbars, panels, brush and labels stay out.
//!
//! The pixels come back from the GPU a frame or more later (the browser
//! can't wait for them), so a screenshot is started, then collected.

use egui::{Key, Rect};

use super::export::credit_line;
use super::study::Layout as ViewLayout;
use super::views::shown;
use super::{App, REFERENCE, Request, SCULPT};
use crate::camera::OrbitCamera;
use crate::export::{self, Compositor, Layout, Overlay, Shape, ViewOverlay, place_split};
use crate::reference::Content;
use crate::render::Slot;
use crate::render::readback::Readback;
use crate::ui::menu;

/// The shortcut, with the command key: "print" the views.
pub(super) const SHOT_KEY: Key = Key::P;
/// The short side of a picture of a fixed shape, as for videos.
const SHORT_SIDE: u32 = 1080;
/// "As on screen" is drawn at twice the points of the views, and at least
/// this on its short side…
const SCREEN_MIN_SHORT: f32 = 1080.0;
/// …and at most this on its long side.
const SCREEN_MAX_LONG: f32 = 4096.0;

/// The picture's proportions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShotFrame {
    /// Those of the views in the window, at twice their points.
    Screen,
    /// A video's frame, 1080 pixels on its short side.
    Shape(Shape),
}

/// A screenshot asked for or on its way.
#[derive(Default)]
pub(super) struct ShotState {
    /// Taken at the next frame's hook.
    asked: Option<ShotFrame>,
    pending: Option<Pending>,
    /// Saved this session, to number the files.
    taken: u32,
}

/// A picture drawn, waiting for the GPU to hand its pixels back.
struct Pending {
    /// Keeps the targets alive until the copy is done.
    _compositor: Compositor,
    readback: Readback,
    name: String,
}

/// The camera for a view `out` wide (width over height) that shows what
/// `camera` shows in a view `screen` wide: the same height, and as much
/// across as that view showed up to its height, the field of view widened
/// where `out` is narrower. Also returns how much larger, for its height,
/// the picture shows the form than the screen did (1 unless widened).
pub(super) fn shot_camera(camera: &OrbitCamera, screen: f32, out: f32) -> (OrbitCamera, f32) {
    let half = (camera.fov_y / 2.0).tan();
    let across = half * screen.min(1.0);
    let mut shot = *camera;
    let out = out.max(0.05);
    if out * half < across {
        shot.fov_y = 2.0 * (across / out).atan();
    }
    (shot, half / (shot.fov_y / 2.0).tan())
}

/// The plumb line hanging at `share` of a view `screen` wide, moved to
/// where it falls on the form in a view `out` wide that shows it `scale`
/// times as large for its height (see [`shot_camera`]).
pub(super) fn shot_plumb(share: f32, screen: f32, out: f32, scale: f32) -> f32 {
    0.5 + (share - 0.5) * screen / out.max(0.05) * scale
}

/// The pixel size of an "as on screen" picture of `points`, the views'
/// area: twice as large, at least [`SCREEN_MIN_SHORT`] on its short side
/// and at most [`SCREEN_MAX_LONG`] (and `max`, the GPU's limit) on its
/// long side.
pub(super) fn screen_size(points: egui::Vec2, max: u32) -> [u32; 2] {
    let (short, long) = (points.min_elem().max(1.0), points.max_elem().max(1.0));
    let scale = (2.0f32)
        .max(SCREEN_MIN_SHORT / short)
        .min(SCREEN_MAX_LONG.min(max as f32) / long);
    [points.x, points.y].map(|v| ((v * scale).round() as u32).clamp(1, max))
}

impl App {
    /// The screenshot items in the file → export menu.
    pub(super) fn screenshot_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        let v = tr.video;
        menu::submenu(ui, v.screenshot, |ui| {
            ui.set_min_width(160.0);
            let busy = self.shot.pending.is_some();
            if menu::action(ui, !busy, v.as_on_screen, &self.keys("P")) {
                self.shot.asked = Some(ShotFrame::Screen);
            }
            for (shape, label) in [
                (Shape::Wide, v.wide),
                (Shape::Square, v.square),
                (Shape::Tall, v.tall),
            ] {
                if menu::action(ui, !busy, label, "") {
                    self.shot.asked = Some(ShotFrame::Shape(shape));
                }
            }
        });
    }

    /// Ctrl+P: a screenshot as on screen.
    pub(super) fn screenshot_shortcut(&mut self, ui: &egui::Ui) {
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, SHOT_KEY)) {
            self.shot.asked = Some(ShotFrame::Screen);
        }
    }

    /// Takes a screenshot asked for, and saves one whose pixels are back.
    /// Called every frame.
    pub(super) fn screenshot_step(&mut self, ui: &egui::Ui) {
        if self.shot.pending.is_none()
            && let Some(frame) = self.shot.asked.take()
        {
            self.shot.pending = Some(self.start_screenshot(frame));
        }
        let Some(pending) = &self.shot.pending else {
            return;
        };
        let tr = self.lang.tr();
        match pending.readback.try_take(&self.device) {
            Ok(None) => ui.ctx().request_repaint(),
            Ok(Some(pixels)) => {
                let pending = self.shot.pending.take().expect("pending");
                match export::png(&pixels, pending.readback.size()) {
                    Ok(bytes) => {
                        self.request(Request::SaveFile {
                            name: pending.name,
                            mime: "image/png",
                            bytes,
                        });
                        self.notify(tr.video.screenshot_saved);
                    }
                    Err(err) => self.notify(format!("{} ({err})", tr.video.failed)),
                }
            }
            Err(_) => {
                self.shot.pending = None;
                self.notify(tr.video.failed);
            }
        }
    }

    /// The picture's size, and its layout as the window's views are now.
    fn shot_placement(&self, frame: ShotFrame) -> ([u32; 2], Layout) {
        let max = self.device.limits().max_texture_dimension_2d;
        let rects = self.view_rects.into_iter().filter(|&r| shown(r));
        let area = rects.fold(Rect::NOTHING, Rect::union);
        let (size, both) = match frame {
            ShotFrame::Screen if area.is_positive() => {
                (screen_size(area.size(), max), Layout::SideBySide)
            }
            ShotFrame::Screen => (Shape::Wide.size(SHORT_SIDE), Layout::SideBySide),
            ShotFrame::Shape(shape) => (shape.size(SHORT_SIDE), shape.natural_layout()),
        };
        let layout = match self.study.layout {
            ViewLayout::Both => both,
            ViewLayout::Sculpt => Layout::SculptOnly,
            ViewLayout::Reference => Layout::ReferenceOnly,
        };
        (size, layout)
    }

    /// Draws the views offscreen into a picture of `frame`'s shape and
    /// starts reading it back.
    fn start_screenshot(&mut self, frame: ShotFrame) -> Pending {
        let (size, layout) = self.shot_placement(frame);
        let placement = place_split(size, layout, self.split, self.study.swapped);
        let mut compositor = Compositor::placed(&self.device, size, placement);
        let screen_aspect = |index: usize| {
            let rect = self.view_rects[index];
            if shown(rect) {
                rect.aspect_ratio()
            } else {
                // Not on screen now (a single view): as wide as its frame.
                f32::NAN
            }
        };
        let mut overlays = [ViewOverlay::default(); 2];
        let mut cameras = self.cameras;
        for (index, target) in [
            (SCULPT, compositor.sculpt_view()),
            (REFERENCE, compositor.reference_view()),
        ] {
            let Some(target) = target else {
                continue;
            };
            let out = target.aspect();
            let screen = Some(screen_aspect(index))
                .filter(|a| a.is_finite())
                .unwrap_or(out);
            let (camera, scale) = shot_camera(&self.cameras[index], screen, out);
            cameras[index] = camera;
            let aids = &self.study.views[index];
            overlays[index] = ViewOverlay {
                mirror: aids.mirror,
                grid: aids.grid.then_some(self.study.grid_divisions),
                plumb: aids
                    .plumb
                    .then(|| shot_plumb(self.study.plumb_x[index], screen, out, scale)),
            };
        }
        compositor.set_overlays(overlays[SCULPT], overlays[REFERENCE]);
        compositor.set_backgrounds([SCULPT, REFERENCE].map(|index| self.view_background(index)));

        let reference = self.scene_reference();
        if let Some(Content::Image(image)) = reference.map(|r| &r.content) {
            compositor.set_image(image);
            if let Some(framing) = self.image_framing(self.view_rects[REFERENCE]) {
                compositor.frame_image(framing);
            }
        }
        // Credited whenever the reference is in the picture: in its view,
        // or laid over the sculpt as the ghost.
        let ghost = self.ghost_look().is_some()
            && reference.is_some_and(|r| !matches!(r.content, Content::Image(_)));
        let in_picture = layout.shows_reference() || (ghost && layout.shows_sculpt());
        let credit = reference
            .filter(|_| in_picture)
            .and_then(|r| credit_line(&r.credit));

        let mut encoder = self.device.create_command_encoder(&Default::default());
        if let Some(view) = compositor.sculpt_view() {
            let mut pass = view.begin_pass(&mut encoder, compositor.backgrounds()[SCULPT]);
            let camera = &cameras[SCULPT];
            self.draw_sculpt(&mut pass, Slot::ExportSculpt, camera, view.size, true);
        }
        if let Some(view) = compositor.reference_view() {
            let mut pass = view.begin_pass(&mut encoder, compositor.backgrounds()[REFERENCE]);
            let look = self.reference_look();
            let camera = &cameras[REFERENCE];
            self.draw_reference_sized(&mut pass, Slot::ExportReference, camera, view.size, &look);
        }
        let overlay = Overlay::Views {
            credit: credit.as_deref(),
            clock: None,
            title: None,
        };
        let mut buffers = compositor.compose(&self.device, &self.queue, &mut encoder, &overlay);
        let readback = Readback::copy(&self.device, &mut encoder, compositor.output());
        buffers.push(encoder.finish());
        self.queue.submit(buffers);
        readback.map();

        self.shot.taken += 1;
        let stem = self.scene_stem();
        let name = match self.shot.taken {
            1 => format!("{stem}-screenshot.png"),
            n => format!("{stem}-screenshot-{n}.png"),
        };
        Pending {
            _compositor: compositor,
            readback,
            name,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::study::tests::Harness;
    use super::*;
    use crate::reference::{Credit, RefImage, Reference, Source};
    use crate::ui::theme;

    #[test]
    fn a_narrower_picture_widens_the_view_to_keep_what_was_across() {
        let camera = OrbitCamera::default();
        // As wide or wider: the camera as it is, the form as large.
        assert_eq!(shot_camera(&camera, 1.5, 1.5), (camera, 1.0));
        assert_eq!(shot_camera(&camera, 0.5, 0.5), (camera, 1.0));
        assert_eq!(shot_camera(&camera, 1.2, 16.0 / 9.0), (camera, 1.0));
        // A wide view in a tall picture: what showed up and down shows
        // across, so the form is smaller.
        let (tall, scale) = shot_camera(&camera, 1.5, 0.5);
        let across = (tall.fov_y / 2.0).tan() * 0.5;
        assert!((across - (camera.fov_y / 2.0).tan()).abs() < 1e-5);
        assert!((scale - 0.5).abs() < 1e-5);
    }

    #[test]
    fn the_plumb_line_stays_on_the_same_point_of_the_form() {
        // The same shape: where it was.
        assert_eq!(shot_plumb(0.3, 1.5, 1.5, 1.0), 0.3);
        assert_eq!(shot_plumb(0.5, 1.5, 0.4, 0.2), 0.5);
        // Twice as wide for its height: half as far from the middle.
        assert!((shot_plumb(0.3, 1.0, 2.0, 1.0) - 0.4).abs() < 1e-6);
        // Widened to half the size: closer to the middle again.
        assert!((shot_plumb(0.3, 1.0, 0.5, 0.5) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn as_on_screen_is_twice_the_points_within_limits() {
        assert_eq!(screen_size(egui::vec2(1000.0, 600.0), 8192), [2000, 1200]);
        // Small views: at least 1080 high.
        assert_eq!(screen_size(egui::vec2(800.0, 400.0), 8192), [2160, 1080]);
        // Very wide: at most 4096 across, or the GPU's limit.
        assert_eq!(screen_size(egui::vec2(3000.0, 1000.0), 8192), [4096, 1365]);
        assert_eq!(screen_size(egui::vec2(3000.0, 1000.0), 2048), [2048, 683]);
    }

    /// A screenshot of `frame` as the app saves it: its file name and
    /// decoded RGB pixels with their size.
    fn shoot(h: &mut Harness, frame: ShotFrame) -> (String, [u32; 2], Vec<u8>) {
        h.app.shot.asked = Some(frame);
        for _ in 0..200 {
            for request in h.frame_requests() {
                if let Request::SaveFile { name, mime, bytes } = request {
                    assert_eq!(mime, "image/png");
                    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
                    let mut reader = decoder.read_info().unwrap();
                    let mut rgb = vec![0; reader.output_buffer_size().unwrap()];
                    let info = reader.next_frame(&mut rgb).unwrap();
                    return (name, [info.width, info.height], rgb);
                }
            }
        }
        panic!("no screenshot came back");
    }

    /// A grey picture as the reference, credited when `credited`.
    fn picture(h: &mut Harness, credited: bool) {
        let image = RefImage {
            size: [4, 4],
            rgba: [90, 90, 90, 255].repeat(16),
        };
        let mut reference = Reference::new(
            Source {
                name: "cast.png".into(),
                bytes: Arc::from(&[1u8, 2, 3][..]),
            },
            Content::Image(image),
        );
        if credited {
            reference.credit = Credit {
                title: "Head of a Cast".into(),
                author: "Someone".into(),
                licence: "CC BY 4.0".into(),
                url: String::new(),
            };
        }
        h.app.set_reference(reference);
        h.frames(2);
    }

    /// Pixels in `rect` (x, y, w, h) of an RGB picture `width` wide that
    /// aren't the views' background.
    fn inked(rgb: &[u8], width: u32, rect: [u32; 4]) -> usize {
        let bg = theme::VIEW_BG;
        let [x0, y0, w, h] = rect;
        let mut count = 0;
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let i = ((y * width + x) * 3) as usize;
                let px = &rgb[i..i + 3];
                let far = px
                    .iter()
                    .zip([bg.r(), bg.g(), bg.b()])
                    .any(|(&a, b)| a.abs_diff(b) > 24);
                count += usize::from(far);
            }
        }
        count
    }

    #[test]
    fn screenshots_have_the_size_chosen_and_keep_the_views_order() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let (name, size, rgb) = shoot(&mut h, ShotFrame::Shape(Shape::Wide));
        assert_eq!(size, [1920, 1080]);
        assert_eq!(rgb.len(), 1920 * 1080 * 3);
        assert_eq!(name, "study-screenshot.png");
        // The clay is in the left half, the empty reference in the right.
        assert!(inked(&rgb, 1920, [200, 300, 560, 480]) > 1000);
        assert_eq!(inked(&rgb, 1920, [1160, 300, 560, 480]), 0);

        let (name, size, _) = shoot(&mut h, ShotFrame::Shape(Shape::Tall));
        assert_eq!(size, [1080, 1920]);
        assert_eq!(name, "study-screenshot-2.png");

        // As on screen: the views' area, twice as large.
        let area = h.app.view_rects[SCULPT].union(h.app.view_rects[REFERENCE]);
        let (_, size, _) = shoot(&mut h, ShotFrame::Screen);
        let expected = screen_size(area.size(), u32::MAX);
        assert_eq!(size, expected);

        // Swapped: the clay on the right.
        h.app.study.swapped = true;
        h.frames(1);
        let (_, _, rgb) = shoot(&mut h, ShotFrame::Shape(Shape::Wide));
        assert_eq!(inked(&rgb, 1920, [200, 300, 560, 480]), 0);
        assert!(inked(&rgb, 1920, [1160, 300, 560, 480]) > 1000);

        // The sculpt alone fills the picture.
        h.app.study.layout = ViewLayout::Sculpt;
        h.frames(1);
        let (_, _, rgb) = shoot(&mut h, ShotFrame::Shape(Shape::Square));
        assert!(inked(&rgb, 1080, [340, 340, 400, 400]) > 1000);
    }

    #[test]
    fn the_credit_is_in_the_picture_whenever_the_reference_is() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        // Along the bottom of the reference half, under the picture.
        let credit_area = [961 + 10, 1080 - 70, 700, 60];
        picture(&mut h, false);
        let (_, _, plain) = shoot(&mut h, ShotFrame::Shape(Shape::Wide));
        picture(&mut h, true);
        let (_, _, credited) = shoot(&mut h, ShotFrame::Shape(Shape::Wide));
        let differ = |a: &[u8], b: &[u8], [x0, y0, w, hh]: [u32; 4]| {
            (y0..y0 + hh)
                .flat_map(|y| (x0..x0 + w).map(move |x| ((y * 1920 + x) * 3) as usize))
                .filter(|&i| a[i..i + 3] != b[i..i + 3])
                .count()
        };
        assert!(
            differ(&plain, &credited, credit_area) > 200,
            "the credit is written"
        );
        // Nothing else changes.
        assert_eq!(differ(&plain, &credited, [0, 0, 1920, 900]), 0);

        // The sculpt alone with no ghost leaves the reference, and its
        // credit, out; a picture can't be a ghost.
        h.app.study.layout = ViewLayout::Sculpt;
        h.app.study.ghost = true;
        h.frames(1);
        let (_, _, alone) = shoot(&mut h, ShotFrame::Shape(Shape::Wide));
        assert_eq!(inked(&alone, 1920, [1300, 1000, 600, 70]), 0);

        // A 3D reference as the ghost over the sculpt alone: credited in
        // the sculpt's bottom right corner.
        let corner = [1300, 1000, 600, 70];
        let mut splat = Vec::new();
        for i in 0..8 {
            let x = i as f32 * 0.1 - 0.35;
            for v in [x, 0.0, 0.0, 0.05, 0.05, 0.05] {
                splat.extend_from_slice(&v.to_le_bytes());
            }
            splat.extend_from_slice(&[160, 160, 160, 255, 255, 128, 128, 128]);
        }
        h.app.load_reference("dots.splat", Arc::from(splat));
        h.frames(2);
        let (_, _, uncredited) = shoot(&mut h, ShotFrame::Shape(Shape::Wide));
        assert_eq!(inked(&uncredited, 1920, corner), 0);
        h.app.reference.current.as_mut().unwrap().credit.licence = "CC0".into();
        let (_, _, ghosted) = shoot(&mut h, ShotFrame::Shape(Shape::Wide));
        assert!(inked(&ghosted, 1920, corner) > 30);
        // Without the ghost, no reference and no credit.
        h.app.study.ghost = false;
        let (_, _, plain) = shoot(&mut h, ShotFrame::Shape(Shape::Wide));
        assert_eq!(inked(&plain, 1920, corner), 0);
    }
}
