// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The gallery: the bundled plaster casts as thumbnails, one click to load
//! one as the reference, credited, as `?sample=` does.
//!
//! The thumbnails sit next to the page (`samples/thumbs/`), drawn by the
//! app's own renderer (`cargo xtask samples thumbs`), and are fetched the
//! first time the gallery opens. Every shell answers the same fetches: the
//! web build from next to the page, the desktop app from the web app's
//! site (casts downloaded once and kept, thumbnails built in).

use std::collections::HashMap;

use egui::{Align2, Color32, Rect, Sense, Stroke, Vec2};

use super::{App, REFERENCE, Request};
use crate::input::FilePurpose;
use crate::reference::load::load_png;
use crate::reference::samples::{SAMPLES, Sample};
use crate::ui::{fonts, menu, theme};

/// Whether this build has a gallery: every shell can fetch the samples.
pub(super) const HAS_GALLERY: bool = true;

/// A thumbnail's side in the window, in points.
const THUMB: f32 = 120.0;
const COLUMNS: usize = 3;
const GAP: Vec2 = Vec2::new(16.0, 14.0);

#[derive(Default)]
pub(super) struct GalleryState {
    open: bool,
    /// The thumbnails were asked for (once, when the gallery first opens).
    asked: bool,
    /// Each sample's thumbnail by id, once it arrived; `None` if it
    /// couldn't be fetched or read.
    thumbs: HashMap<&'static str, Option<egui::TextureHandle>>,
    /// The sample on its way, from the gallery or `?sample=`, until it
    /// arrives or another file takes its place.
    pub(super) loading: Option<&'static Sample>,
}

impl App {
    /// Opens the gallery, fetching the thumbnails the first time.
    pub(super) fn open_gallery(&mut self) {
        let gallery = &mut self.reference.gallery;
        gallery.open = true;
        if std::mem::replace(&mut gallery.asked, true) {
            return;
        }
        for sample in SAMPLES {
            self.request(Request::Fetch {
                url: sample.thumb_path(),
                purpose: FilePurpose::Thumbnail,
            });
        }
    }

    /// Fetches a bundled sample as the reference; it is credited when it
    /// arrives. Until then the reference view says it is loading.
    pub(super) fn load_sample(&mut self, sample: &'static Sample) {
        self.request(Request::Fetch {
            url: sample.path(),
            purpose: FilePurpose::Reference,
        });
        self.reference.gallery.loading = Some(sample);
    }

    /// A thumbnail arrived, named `<id>.png`; empty if its fetch failed.
    pub(super) fn on_thumbnail(&mut self, name: &str, bytes: &[u8]) {
        let Some(sample) = name.strip_suffix(".png").and_then(Sample::by_id) else {
            return log::warn!("{name}: not a sample's thumbnail");
        };
        let texture = match load_png(bytes) {
            Ok(image) => {
                let size = [image.size[0] as usize, image.size[1] as usize];
                let pixels = egui::ColorImage::from_rgba_unmultiplied(size, &image.rgba);
                Some(self.ctx.load_texture(
                    format!("thumbnail {}", sample.id),
                    pixels,
                    egui::TextureOptions::LINEAR,
                ))
            }
            Err(e) => {
                log::warn!("{name}: {e}");
                None
            }
        };
        self.reference.gallery.thumbs.insert(sample.id, texture);
    }

    /// The gallery item in the file menu, after importing a reference.
    pub(super) fn gallery_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        if HAS_GALLERY && menu::action(ui, true, tr.refs.open_gallery, "") {
            self.open_gallery();
        }
    }

    /// The sample the loaded reference came from.
    fn shown_sample(&self) -> Option<&'static Sample> {
        let r = self.reference.current.as_ref()?;
        Sample::by_file(&r.source.name)
    }

    /// The gallery window while it is open: a grid of thumbnails with their
    /// names, and a line under it with the credit of the one pointed at.
    pub(super) fn gallery_window(&mut self, ui: &egui::Ui) {
        if !self.reference.gallery.open {
            return;
        }
        let tr = self.lang.tr();
        let shown = self.shown_sample();
        let loading = self.reference.gallery.loading;
        let mut open = true;
        let mut chosen = None;
        let width = COLUMNS as f32 * THUMB + (COLUMNS - 1) as f32 * GAP.x;
        egui::Window::new(tr.refs.gallery)
            .collapsible(false)
            .resizable(false)
            .min_width(width)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.set_max_width(width);
                let muted = ui.visuals().widgets.inactive.fg_stroke.color;
                let mut pointed = None;
                for row in SAMPLES.chunks(COLUMNS) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = GAP.x;
                        for sample in row {
                            let thumb = self.reference.gallery.thumbs.get(sample.id);
                            let name = sample_name(self, sample);
                            let on = [shown, loading].iter().flatten().any(|s| s.id == sample.id);
                            let response = cell(ui, thumb, name, on);
                            if response.hovered() {
                                pointed = Some(sample);
                            }
                            if response.clicked() {
                                chosen = Some(sample);
                            }
                        }
                    });
                    ui.add_space(GAP.y - ui.spacing().item_spacing.y);
                }
                // A 1px rule, then who made the cast pointed at.
                let (rule, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), Sense::hover());
                let line = ui.visuals().widgets.noninteractive.bg_stroke.color;
                ui.painter()
                    .hline(rule.x_range(), rule.center().y, Stroke::new(1.0, line));
                let text = match (pointed, loading) {
                    (Some(s), _) => format!("{} — {}, {}", s.title, s.author, s.licence),
                    (None, Some(s)) => tr.refs.loading.replace("{name}", sample_name(self, s)),
                    (None, None) => tr.refs.gallery_note.to_owned(),
                };
                // Room for three lines always (the longest credit), so the
                // window keeps its height as the pointer moves.
                let font = fonts::regular(12.0);
                let rows = 3.0 * ui.fonts_mut(|f| f.row_height(&font));
                let galley = ui.painter().layout(text, font, muted, width);
                let height = galley.size().y.max(rows);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
                ui.painter().galley(rect.min, galley, muted);
            });
        let gallery = &mut self.reference.gallery;
        gallery.open = open;
        if let Some(sample) = chosen {
            gallery.open = false;
            if shown.is_none_or(|s| s.id != sample.id) {
                self.load_sample(sample);
            }
        }
    }

    /// Under the empty reference view's drop hint, centred at `at`: a link
    /// to the gallery.
    pub(super) fn gallery_link(&mut self, ui: &egui::Ui, at: egui::Pos2) {
        if !HAS_GALLERY || self.reference.gallery.loading.is_some() {
            return;
        }
        let text = self.lang.tr().refs.gallery_link;
        let inks = self.view_inks(REFERENCE);
        let clicked = egui::Area::new(egui::Id::unique("gallery link"))
            .fixed_pos(at)
            .pivot(Align2::CENTER_TOP)
            .show(ui.ctx(), |ui| {
                // The views' muted text with a 1px underline, as the
                // first-run hint is.
                let galley =
                    ui.painter()
                        .layout_no_wrap(text.to_owned(), fonts::regular(14.0), inks.text);
                let size = galley.size() + egui::vec2(0.0, 4.0);
                let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                let colour = if response.hovered() {
                    inks.ink
                } else {
                    inks.text
                };
                ui.painter().galley(rect.min, galley, colour);
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom() - 0.5,
                    Stroke::new(1.0, colour),
                );
                response
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
            })
            .inner;
        if clicked {
            self.open_gallery();
        }
    }

    /// What the reference view says while a sample downloads: in its
    /// middle when it is empty, else along its top.
    pub(super) fn loading_text(&self) -> Option<String> {
        let sample = self.reference.gallery.loading?;
        let name = sample_name(self, sample);
        Some(self.lang.tr().refs.loading.replace("{name}", name))
    }
}

/// A sample's short name in the app's language.
fn sample_name(app: &App, sample: &Sample) -> &'static str {
    app.lang
        .tr()
        .refs
        .sample_names
        .iter()
        .find(|(id, _)| *id == sample.id)
        .map_or(sample.id, |(_, name)| name)
}

/// One sample: its thumbnail (the views' black until it arrives) and its
/// name under it, muted, in the text colour when pointed at, and
/// underlined when it is the reference.
fn cell(
    ui: &mut egui::Ui,
    thumb: Option<&Option<egui::TextureHandle>>,
    name: &str,
    chosen: bool,
) -> egui::Response {
    let visuals = ui.visuals().clone();
    let galley = ui.painter().layout(
        name.to_owned(),
        fonts::regular(13.0),
        Color32::PLACEHOLDER,
        THUMB,
    );
    let size = egui::vec2(THUMB, THUMB + 6.0 + galley.size().y.max(16.0) + 3.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let image = Rect::from_min_size(rect.min, Vec2::splat(THUMB));
        match thumb {
            Some(Some(texture)) => {
                let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                painter.image(texture.id(), image, uv, Color32::WHITE);
            }
            _ => {
                painter.rect_filled(image, 0.0, theme::VIEW_BG);
            }
        }
        let colour = if chosen || response.hovered() {
            visuals.widgets.active.fg_stroke.color
        } else {
            visuals.widgets.inactive.fg_stroke.color
        };
        let text = egui::pos2(
            rect.center().x - galley.size().x / 2.0,
            image.bottom() + 6.0,
        );
        let text_rect = Rect::from_min_size(text, galley.size());
        painter.galley_with_override_text_color(text, galley, colour);
        if chosen {
            painter.hline(
                text_rect.x_range(),
                text_rect.bottom() + 1.5,
                Stroke::new(1.0, colour),
            );
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::i18n::Lang;

    #[test]
    fn every_sample_has_a_name_in_every_language() {
        for lang in Lang::ALL {
            let names = lang.tr().refs.sample_names;
            for sample in SAMPLES {
                assert!(
                    names
                        .iter()
                        .any(|(id, name)| *id == sample.id && !name.is_empty()),
                    "{lang:?}: {}",
                    sample.id
                );
            }
            assert_eq!(names.len(), SAMPLES.len(), "{lang:?}: names of no sample");
        }
    }

    /// A binary PLY tetrahedron, written as the samples are.
    fn tetrahedron() -> Vec<u8> {
        let mut out = b"ply\nformat binary_little_endian 1.0\nelement vertex 4\n\
            property float x\nproperty float y\nproperty float z\n\
            element face 4\nproperty list uchar int vertex_indices\nend_header\n"
            .to_vec();
        for v in [
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ] {
            v.iter()
                .for_each(|x| out.extend_from_slice(&x.to_le_bytes()));
        }
        for t in [[0i32, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]] {
            out.push(3);
            t.iter()
                .for_each(|i| out.extend_from_slice(&i.to_le_bytes()));
        }
        out
    }

    fn file(purpose: FilePurpose, name: &str, bytes: Vec<u8>) -> crate::InputEvent {
        crate::InputEvent::File {
            purpose,
            name: name.into(),
            bytes: bytes.into(),
        }
    }

    #[test]
    fn the_gallery_fetches_its_thumbnails_once_and_loads_a_sample_credited() {
        let Some(mut h) = super::super::study::tests::Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        h.app.open_gallery();
        let thumbs = h
            .frame_requests()
            .into_iter()
            .filter(|r| {
                matches!(r, Request::Fetch { url, purpose: FilePurpose::Thumbnail }
                    if url.starts_with("samples/thumbs/"))
            })
            .count();
        assert_eq!(thumbs, SAMPLES.len());
        h.app.reference.gallery.open = false;
        h.app.open_gallery();
        assert!(h.frame_requests().is_empty(), "asked for once");

        // A thumbnail arrives; one that couldn't be fetched is left blank.
        let web = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web");
        let homer = std::fs::read(web.join("samples/thumbs/homer.png")).unwrap();
        h.app
            .push_event(file(FilePurpose::Thumbnail, "homer.png", homer.clone()));
        h.app
            .push_event(file(FilePurpose::Thumbnail, "socrates.png", Vec::new()));
        let gallery = &h.app.reference.gallery;
        assert!(gallery.thumbs["homer"].is_some());
        assert!(gallery.thumbs["socrates"].is_none());
        h.frames(1);

        // Chosen: fetched as the reference, loading until it is there.
        let sample = Sample::by_id("homer").unwrap();
        h.app.load_sample(sample);
        let requests = h.frame_requests();
        assert!(requests.contains(&Request::Fetch {
            url: "samples/homer.ply".into(),
            purpose: FilePurpose::Reference,
        }));
        assert_eq!(h.app.loading_text().as_deref(), Some("loading homer…"));
        // Thumbnails arriving meanwhile don't end it.
        h.app
            .push_event(file(FilePurpose::Thumbnail, "homer.png", homer));
        assert!(h.app.loading_text().is_some());
        h.frames(1);
        h.app
            .push_event(file(FilePurpose::Reference, "homer.ply", tetrahedron()));
        assert!(h.app.loading_text().is_none());
        let reference = h.app.reference.current.as_ref().expect("loaded");
        assert_eq!(reference.credit, sample.credit());
        assert_eq!(h.app.shown_sample().map(|s| s.id), Some("homer"));
    }

    #[test]
    fn a_sample_that_cannot_be_fetched_stops_loading() {
        let Some(mut h) = super::super::study::tests::Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        h.app.load_sample(Sample::by_id("foot").unwrap());
        h.frames(1);
        // The shell hands over an empty file when the fetch fails.
        h.app.push_event(file(
            FilePurpose::Reference,
            "athlete-right-foot.ply",
            Vec::new(),
        ));
        assert!(h.app.loading_text().is_none());
        assert!(h.app.reference.current.is_none());
        let notice = h.app.notice.as_ref().map(|(text, _)| text.as_str());
        assert_eq!(notice, Some(h.app.lang.tr().refs.download_failed));

        // With a reference shown, a failed download leaves it as it was.
        h.app
            .push_event(file(FilePurpose::Reference, "homer.ply", tetrahedron()));
        h.app.notice = None;
        h.app.load_sample(Sample::by_id("brutus").unwrap());
        h.frames(1);
        h.app
            .push_event(file(FilePurpose::Reference, "brutus.ply", Vec::new()));
        assert!(h.app.notice.is_some());
        assert_eq!(h.app.shown_sample().map(|s| s.id), Some("homer"));
    }

    /// The desktop app has the gallery too: the tests run on it.
    #[test]
    fn the_empty_reference_view_links_to_the_gallery_on_every_build() {
        let Some(mut h) = super::super::study::tests::Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        assert!(h.app.reference.current.is_none());
        // The link sits just under the drop hint, in the middle.
        let link = h.app.view_rects[REFERENCE].center() + egui::vec2(0.0, 28.0);
        h.drag(link, link);
        let gallery = &h.app.reference.gallery;
        assert!(gallery.open && gallery.asked, "the link opened the gallery");
    }
}
