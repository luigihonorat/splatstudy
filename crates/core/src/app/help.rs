// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Getting started, the first-run hint and preferences, plus what the shells
//! tell the app about the session: options from the page's URL (or the
//! command line) and where a text field is being edited, for the IME.

use egui::{Align2, RichText};

use super::sculpting::{BRUSHES, brush_name};
use super::{App, Request, SCULPT};
use crate::input::FilePurpose;
use crate::ui::{self, menu};

/// The getting started window and what the URL asked for.
#[derive(Default)]
pub(super) struct HelpState {
    show_getting_started: bool,
    /// First visit: a link under the sculpt view points to getting started
    /// until it is opened or the first stroke is made.
    first_run_hint: bool,
    /// View names from `?view=`, lowercase, in order. Read by the study part
    /// through [`App::url_views`].
    url_views: Vec<String>,
}

/// A text field being edited, so the shell can put its text input (and the
/// IME's candidate window) there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextField {
    /// Left, top, width and height, in points.
    pub rect: [f32; 4],
    /// The theme's text, background and line colours, as RGB.
    pub text: [u8; 3],
    pub background: [u8; 3],
    pub line: [u8; 3],
}

impl App {
    /// Applies options given in the page's query string (`?a=1&b=2`, with or
    /// without the `?`) or, on desktop, on the command line:
    ///
    /// - `sample=<id>` loads `samples/<id>` as the reference, through
    ///   [`Request::Fetch`].
    /// - `view=<name>[,<name>…]` (repeatable) asks for study views by name,
    ///   such as `silhouette` or `ghost`. They are only stored here; the study
    ///   part reads them with [`App::url_views`] and applies the names it
    ///   knows.
    ///
    /// `lang` and `debug` are read by the shells themselves.
    pub fn apply_url_options(&mut self, query: &str) {
        for (key, value) in query_pairs(query) {
            match key {
                "sample" if is_sample_id(value) => self.request(Request::Fetch {
                    url: format!("samples/{value}"),
                    purpose: FilePurpose::Reference,
                }),
                "sample" => log::warn!("ignored sample id {value:?}"),
                "view" => self.help.url_views.extend(
                    value
                        .split(',')
                        .map(|v| v.trim().to_ascii_lowercase())
                        .filter(|v| !v.is_empty()),
                ),
                _ => {}
            }
        }
        self.dirty = true;
    }

    /// The study views asked for in the URL (`?view=silhouette,ghost`), in
    /// order. Names the study part doesn't know should be ignored.
    #[allow(dead_code)] // until the study part reads it
    pub(super) fn url_views(&self) -> &[String] {
        &self.help.url_views
    }

    /// The shell knows whether this is the first visit (it keeps a flag in
    /// storage); the first visit shows a link to getting started.
    pub fn set_first_run(&mut self, first: bool) {
        self.help.first_run_hint = first;
        self.dirty = true;
    }

    /// The text field being edited, if any, after the last frame.
    pub fn text_field(&self) -> Option<TextField> {
        if !self.ctx.text_edit_focused() {
            return None;
        }
        let id = self.ctx.memory(|m| m.focused())?;
        let rect = self.ctx.read_response(id)?.rect;
        let p = self.theme.palette();
        let rgb = |c: egui::Color32| [c.r(), c.g(), c.b()];
        Some(TextField {
            rect: [rect.left(), rect.top(), rect.width(), rect.height()],
            text: rgb(p.text),
            background: rgb(p.bg),
            line: rgb(p.border),
        })
    }

    /// Help items between shortcuts and report issue.
    pub(super) fn help_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        if menu::action(ui, true, tr.getting_started, "") {
            self.open_getting_started();
        }
    }

    /// Preferences, at the end of the edit menu.
    pub(super) fn preferences_item(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        menu::action(ui, false, tr.preferences, "");
    }

    fn open_getting_started(&mut self) {
        self.help.show_getting_started = true;
        self.help.first_run_hint = false;
    }

    /// The getting started window while open, and the first-run hint.
    pub(super) fn help_windows(&mut self, ui: &egui::Ui) {
        self.first_run_hint(ui);
        let tr = self.lang.tr();
        let h = tr.help;
        let mut open = self.help.show_getting_started;
        egui::Window::new(tr.getting_started)
            .collapsible(false)
            .resizable(false)
            .min_width(420.0)
            .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .open(&mut open)
            .show(ui.ctx(), |ui| {
                ui.set_max_width(420.0);
                let muted = ui.visuals().widgets.inactive.fg_stroke.color;
                ui.label(h.intro);
                ui::section(ui, h.reference_heading);
                ui.label(h.reference_text);
                ui::section(ui, h.sculpt_heading);
                ui.label(h.sculpt_text);
                let keys: Vec<String> = BRUSHES
                    .iter()
                    .map(|&(brush, key, _)| format!("{} {}", key.name(), brush_name(tr, brush)))
                    .collect();
                ui.label(RichText::new(keys.join("   ")).color(muted));
                ui::section(ui, tr.navigation);
                ui.label(self.nav_hint());
                ui.label(RichText::new(h.navigation_text).color(muted));
                ui.add_space(8.0);
                ui.label(RichText::new(h.more).color(muted));
            });
        self.help.show_getting_started = open;
    }

    /// A quiet link under the sculpt view on the first visit, until it is
    /// followed or the first stroke is made.
    fn first_run_hint(&mut self, ui: &egui::Ui) {
        if !self.help.first_run_hint || self.help.show_getting_started {
            return;
        }
        if self.sculpt.can_undo() {
            self.help.first_run_hint = false;
            return;
        }
        let view = self.view_rects[SCULPT];
        if !view.is_positive() {
            return;
        }
        let text = self.lang.tr().help.first_run_hint;
        let clicked = egui::Area::new(egui::Id::unique("first run hint"))
            .fixed_pos(view.center_bottom() - egui::vec2(0.0, 24.0))
            .pivot(Align2::CENTER_BOTTOM)
            .show(ui.ctx(), |ui| ui::choice(ui, text, false).clicked())
            .inner;
        if clicked {
            self.open_getting_started();
        }
    }
}

/// `key=value` pairs of a query string; a bare key has an empty value.
fn query_pairs(query: &str) -> impl Iterator<Item = (&str, &str)> {
    query
        .trim_start_matches('?')
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
}

/// A sample id is a plain file name: no folders, no leading dot.
fn is_sample_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_strings_split_into_pairs() {
        let pairs: Vec<_> = query_pairs("?lang=ja&debug&view=ghost").collect();
        assert_eq!(pairs, [("lang", "ja"), ("debug", ""), ("view", "ghost")]);
        assert_eq!(query_pairs("").count(), 0);
        assert_eq!(query_pairs("?").count(), 0);
    }

    #[test]
    fn sample_ids_stay_inside_the_samples_folder() {
        assert!(is_sample_id("venus.spz"));
        assert!(is_sample_id("cast_01-head.ply"));
        assert!(!is_sample_id(""));
        assert!(!is_sample_id("../index.html"));
        assert!(!is_sample_id(".hidden"));
        assert!(!is_sample_id("a/b.ply"));
        assert!(!is_sample_id("https:%2F%2Fexample.com"));
    }
}
