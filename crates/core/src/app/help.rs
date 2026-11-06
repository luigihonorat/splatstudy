// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Getting started, the first-run hint and preferences, plus what the shells
//! tell the app about the session: options from the page's URL (or the
//! command line) and where a text field is being edited, for the IME.

use egui::{Align2, RichText};

use super::sculpting::{BRUSHES, brush_digit, brush_name};
use super::study::Layout;
use super::{App, SCULPT};
use crate::navigation::NavStyle;
use crate::reference::samples::Sample;
use crate::ui::i18n::Lang;
use crate::ui::theme::ThemeMode;
use crate::ui::{self, menu, theme};

/// The getting started window and what the URL asked for.
#[derive(Default)]
pub(super) struct HelpState {
    show_getting_started: bool,
    /// First visit: a link under the sculpt view points to getting started
    /// until it is opened or the first stroke is made.
    first_run_hint: bool,
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
    /// - `sample=<id>` loads a bundled sample (see
    ///   [`SAMPLES`](crate::reference::samples::SAMPLES)) as the reference,
    ///   through [`Request::Fetch`].
    /// - `view=<name>[,<name>…]` (repeatable) turns study aids on:
    ///   `silhouette`, `ghost`, `mirror`, `plumb`, `grid`, `link`, `swap`,
    ///   `ortho`, and the layout `both`, `sculpt` or `reference`. The
    ///   aids each view has of its own turn on in both. Unknown names are
    ///   logged and skipped.
    ///
    /// `lang` and `debug` are read by the shells themselves.
    pub fn apply_url_options(&mut self, query: &str) {
        for (key, value) in query_pairs(query) {
            match key {
                "sample" => match Sample::by_id(value) {
                    Some(sample) => self.load_sample(sample),
                    None => log::warn!("no sample {value:?}"),
                },
                "view" => {
                    for name in value.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                        if !self.apply_view(&name.to_ascii_lowercase()) {
                            log::warn!("no view {name:?}");
                        }
                    }
                }
                _ => {}
            }
        }
        self.dirty = true;
    }

    /// The preferences that outlive a session, as `key=value` pairs joined
    /// by `&`: `lang=ja&theme=dark&nav=zbrush`, then `&hide=crop,credit`
    /// when the window menu took groups out of the panels. The shells store this
    /// string as is (localStorage on the web, a file on desktop) and hand it
    /// back to [`App::apply_preferences`] at start.
    pub fn preferences(&self) -> String {
        let lang = match self.lang {
            Lang::Ja => "ja",
            Lang::En => "en",
            Lang::Fr => "fr",
        };
        let theme = match self.theme {
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        };
        let nav = self.nav.name().to_ascii_lowercase();
        let mut saved = format!("lang={lang}&theme={theme}&nav={nav}");
        let removed = self.panels.removed_names();
        if !removed.is_empty() {
            saved += &format!("&hide={removed}");
        }
        saved
    }

    /// Restores preferences saved from [`App::preferences`]. Unknown keys
    /// and values are skipped, so an older or newer string still loads.
    pub fn apply_preferences(&mut self, saved: &str) {
        for (key, value) in query_pairs(saved) {
            match key {
                "lang" if !value.is_empty() => self.lang = Lang::from_tag(value),
                "theme" => {
                    let theme = match value {
                        "light" => ThemeMode::Light,
                        "dark" => ThemeMode::Dark,
                        _ => continue,
                    };
                    self.theme = theme;
                    theme::apply(&self.ctx, theme);
                }
                "nav" => {
                    if let Some(nav) = NavStyle::ALL
                        .into_iter()
                        .find(|n| n.name().eq_ignore_ascii_case(value))
                    {
                        self.nav = nav;
                    }
                }
                "hide" => self.panels.set_removed_names(value),
                _ => {}
            }
        }
        self.dirty = true;
    }

    /// Turns on one study aid by its URL name, in both views; false if
    /// there is none.
    fn apply_view(&mut self, name: &str) -> bool {
        let st = &mut self.study;
        match name {
            "silhouette" => st.views.iter_mut().for_each(|v| v.silhouette = true),
            "ghost" => st.ghost = true,
            "mirror" => st.views.iter_mut().for_each(|v| v.mirror = true),
            "plumb" => st.views.iter_mut().for_each(|v| v.plumb = true),
            "grid" => st.views.iter_mut().for_each(|v| v.grid = true),
            "ortho" => self.cameras.iter_mut().for_each(|c| c.ortho = true),
            "link" => st.link = true,
            "swap" => st.swapped = true,
            "both" => st.layout = Layout::Both,
            "sculpt" => st.layout = Layout::Sculpt,
            "reference" => st.layout = Layout::Reference,
            _ => return false,
        }
        true
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
                ui.label(h.sculpt_text.replace("{invert}", self.nav.invert_key()));
                let keys: Vec<String> = BRUSHES
                    .iter()
                    .map(|&(brush, _, _)| {
                        format!("{} {}", brush_digit(brush), brush_name(tr, brush))
                    })
                    .collect();
                ui.label(RichText::new(keys.join("   ")).color(muted));
                ui::section(ui, h.study_heading);
                let study = [
                    ("L", tr.link_cameras),
                    ("W", tr.study.swap_sides),
                    ("R", tr.mirror_view),
                    ("K", tr.silhouette),
                    ("O", tr.ghost),
                    ("V", tr.plumb_line),
                    ("G", tr.grid),
                ]
                .map(|(key, name)| format!("{key} {name}"));
                ui.label(RichText::new(study.join("   ")).color(muted));
                ui::section(ui, tr.navigation);
                ui.label(self.nav_hint());
                ui.label(RichText::new(h.navigation_text).color(muted));
                ui.add_space(8.0);
                ui.label(RichText::new(h.more).color(muted));
            });
        self.help.show_getting_started = open;
    }

    /// A quiet link under the sculpt view on the first visit, until it is
    /// followed, the first stroke is made or a reference is loaded.
    fn first_run_hint(&mut self, ui: &egui::Ui) {
        if !self.help.first_run_hint || self.help.show_getting_started {
            return;
        }
        if self.sculpt.can_undo() || self.reference.current.is_some() {
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
            .show(ui.ctx(), |ui| {
                // The views' own muted text, like the drop hint, with a 1px
                // underline that marks it as a link.
                let galley = ui.painter().layout_no_wrap(
                    text.to_owned(),
                    egui::TextStyle::Body.resolve(ui.style()),
                    theme::VIEW_TEXT,
                );
                let size = galley.size() + egui::vec2(0.0, 4.0);
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
                let colour = if response.hovered() {
                    theme::VIEW_INK
                } else {
                    theme::VIEW_TEXT
                };
                ui.painter().galley(rect.min, galley, colour);
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom() - 0.5,
                    egui::Stroke::new(1.0, colour),
                );
                response
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
            })
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
}
