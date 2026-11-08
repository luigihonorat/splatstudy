// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Each view's background colour. The last button of a view's toolbar
//! opens a small window beside it: a colour picker (saturation and value
//! in a square, the hue in a strip under it) and three choices that give
//! the picked colour to the sculpt view, the reference view or both, then
//! close it. While it is open, the view it was opened from shows the
//! picked colour; its cross, or Escape, closes it and leaves both views as
//! they were. The colours outlive the session with the preferences.

use egui::ecolor::{hsv_from_rgb, rgb_from_hsv};
use egui::{Align2, Color32, CursorIcon, Id, Mesh, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use super::toolbar::{Toolbar, ViewOption, on_left};
use super::views::shown;
use super::{App, REFERENCE, SCULPT};
use crate::ui::theme::{self, ViewInks};
use crate::ui::{self, fonts};

/// The picker's width, in points: the square's and the strip's.
const WIDTH: f32 = 184.0;
const SQUARE_HEIGHT: f32 = 138.0;
const HUE_HEIGHT: f32 = 12.0;
/// Between the toolbar and the window.
const GAP: f32 = 6.0;
/// The cross's square, in the window's top right corner.
const CLOSE: f32 = 16.0;

/// Which views a picked colour goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Sculpt,
    Reference,
    Both,
}

impl Target {
    pub(super) const ALL: [Target; 3] = [Target::Sculpt, Target::Reference, Target::Both];

    fn views(self) -> &'static [usize] {
        match self {
            Target::Sculpt => &[SCULPT],
            Target::Reference => &[REFERENCE],
            Target::Both => &[SCULPT, REFERENCE],
        }
    }
}

pub(super) struct BackgroundState {
    /// Each view's colour: the sculpt's, then the reference's.
    pub(super) colours: [Color32; 2],
    /// The window, while it is open.
    pub(super) picker: Option<Picker>,
}

impl Default for BackgroundState {
    fn default() -> Self {
        Self {
            colours: [theme::VIEW_BG; 2],
            picker: None,
        }
    }
}

/// The open window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Picker {
    /// The view whose button opened it, which shows the picked colour
    /// meanwhile.
    pub(super) index: usize,
    /// The picked colour's hue, saturation and value, 0..1, kept apart so
    /// the hue stays put on a grey.
    pub(super) hsv: [f32; 3],
    /// Where its parts were drawn last; nothing before its first frame.
    pub(super) drawn: Drawn,
}

/// Where the window's parts were drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Drawn {
    pub(super) window: Rect,
    pub(super) square: Rect,
    pub(super) hue: Rect,
    pub(super) close: Rect,
    pub(super) default: Rect,
    /// The choices, in the order of [`Target::ALL`].
    pub(super) targets: [Rect; 3],
}

impl Default for Drawn {
    fn default() -> Self {
        Self {
            window: Rect::NOTHING,
            square: Rect::NOTHING,
            hue: Rect::NOTHING,
            close: Rect::NOTHING,
            default: Rect::NOTHING,
            targets: [Rect::NOTHING; 3],
        }
    }
}

/// What a frame of the window ends with.
enum Outcome {
    Open,
    Cancel,
    Apply(Target),
}

fn hsv_of(colour: Color32) -> [f32; 3] {
    let [r, g, b, _] = colour.to_array();
    let (h, s, v) = hsv_from_rgb([r, g, b].map(|c| f32::from(c) / 255.0));
    [h, s, v]
}

fn colour_of([h, s, v]: [f32; 3]) -> Color32 {
    let [r, g, b] = rgb_from_hsv((h, s, v)).map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    Color32::from_rgb(r, g, b)
}

/// A colour as the preferences keep it: six lowercase hex digits.
fn to_hex(colour: Color32) -> String {
    format!("{:02x}{:02x}{:02x}", colour.r(), colour.g(), colour.b())
}

fn from_hex(text: &str) -> Option<Color32> {
    let text = text.trim().trim_start_matches('#');
    if text.len() != 6 || !text.is_ascii() {
        return None;
    }
    let byte = |k: usize| u8::from_str_radix(&text[k..k + 2], 16).ok();
    Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?))
}

/// A mark that reads over `colour`: white on a dark colour, black on a
/// light one.
fn contrast(colour: Color32) -> Color32 {
    theme::view_inks(colour).ink
}

/// Saturation across, value up, at `hue`: a grid of vertices, each the
/// exact colour, so the blend between them stays close.
fn paint_square(painter: &egui::Painter, rect: Rect, hue: f32) {
    const N: u32 = 16;
    let mut mesh = Mesh::default();
    for j in 0..=N {
        for i in 0..=N {
            let (s, v) = (i as f32 / N as f32, 1.0 - j as f32 / N as f32);
            let at = pos2(
                rect.left() + s * rect.width(),
                rect.top() + (1.0 - v) * rect.height(),
            );
            mesh.colored_vertex(at, colour_of([hue, s, v]));
        }
    }
    for j in 0..N {
        for i in 0..N {
            let a = j * (N + 1) + i;
            let c = a + N + 1;
            mesh.add_triangle(a, a + 1, c);
            mesh.add_triangle(a + 1, c + 1, c);
        }
    }
    painter.add(mesh);
}

/// Every hue, left to right.
fn paint_hues(painter: &egui::Painter, rect: Rect) {
    const N: u32 = 36;
    let mut mesh = Mesh::default();
    for i in 0..=N {
        let h = i as f32 / N as f32;
        let x = rect.left() + h * rect.width();
        let colour = colour_of([h, 1.0, 1.0]);
        mesh.colored_vertex(pos2(x, rect.top()), colour);
        mesh.colored_vertex(pos2(x, rect.bottom()), colour);
    }
    for i in 0..N {
        let a = 2 * i;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 3, a + 2);
    }
    painter.add(mesh);
}

impl App {
    /// The colour view `index` is cleared to this frame: the picked one
    /// while the window opened from it is open.
    pub(super) fn view_background(&self, index: usize) -> Color32 {
        match self.background.picker {
            Some(picker) if picker.index == index => colour_of(picker.hsv),
            _ => self.background.colours[index],
        }
    }

    /// What to draw over view `index` in grey and white: dark over a light
    /// background.
    pub(super) fn view_inks(&self, index: usize) -> ViewInks {
        theme::view_inks(self.view_background(index))
    }

    /// Whether the background window opened from view `index` is open.
    pub(super) fn background_open(&self, index: usize) -> bool {
        self.background.picker.is_some_and(|p| p.index == index)
    }

    /// Opens the background window from view `index`'s button, starting
    /// from that view's colour, or closes it, changing nothing.
    pub(super) fn toggle_background(&mut self, index: usize) {
        self.background.picker = (!self.background_open(index)).then(|| Picker {
            index,
            hsv: hsv_of(self.background.colours[index]),
            drawn: Drawn::default(),
        });
        self.flyout = None;
    }

    /// Escape closes the window, changing nothing. Before the other
    /// shortcuts.
    pub(super) fn background_keys(&mut self, ui: &egui::Ui) {
        if self.background.picker.is_some()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.background.picker = None;
        }
    }

    /// The colours for the preferences, `sculpt,reference` in hex; `None`
    /// while both are the default.
    pub(super) fn background_preference(&self) -> Option<String> {
        let [sculpt, reference] = self.background.colours;
        (self.background.colours != [theme::VIEW_BG; 2])
            .then(|| format!("{},{}", to_hex(sculpt), to_hex(reference)))
    }

    /// Restores the colours from the preferences; a value that doesn't
    /// read leaves them as they are.
    pub(super) fn apply_background_preference(&mut self, value: &str) {
        let mut parts = value.split(',').map(from_hex);
        if let (Some(Some(sculpt)), Some(Some(reference)), None) =
            (parts.next(), parts.next(), parts.next())
        {
            self.background.colours = [sculpt, reference];
        }
    }

    /// Draws the open window beside its button and takes its input. Called
    /// with the toolbars, as this frame lays them out.
    pub(super) fn background_window(
        &mut self,
        ui: &egui::Ui,
        rects: [Rect; 2],
        layouts: &[Option<Toolbar>; 2],
    ) {
        let Some(mut picker) = self.background.picker else {
            return;
        };
        let index = picker.index;
        // Gone with its view.
        let placed = layouts[index].as_ref().and_then(|bar| {
            let k = bar
                .options
                .iter()
                .position(|&o| o == ViewOption::Background)?;
            let button = bar.buttons[k];
            let panel = bar.panels.iter().find(|p| p.contains_rect(button))?;
            Some((button, *panel))
        });
        let Some((button, panel)) = placed else {
            self.background.picker = None;
            return;
        };
        // Beside the toolbar, towards the view's middle, its foot level
        // with the button's.
        let (pivot, at) = if on_left(index, self.study.swapped) {
            (
                Align2::LEFT_BOTTOM,
                pos2(panel.right() + GAP, button.bottom()),
            )
        } else {
            (
                Align2::RIGHT_BOTTOM,
                pos2(panel.left() - GAP, button.bottom()),
            )
        };
        let area = rects
            .into_iter()
            .filter(|r| shown(*r))
            .fold(Rect::NOTHING, |a, r| a.union(r));
        let p = self.theme.palette();
        let st = self.lang.tr().study;
        let mut outcome = Outcome::Open;
        let shown = egui::Window::new(st.background)
            .id(Id::unique("view background"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .pivot(pivot)
            .fixed_pos(at)
            .constrain_to(area)
            .show(ui.ctx(), |ui| {
                ui.set_width(WIDTH);
                ui.spacing_mut().item_spacing.y = 10.0;
                let id = ui.scope_id();
                let drawn = &mut picker.drawn;

                // The title, and the cross in the corner.
                let (head, _) = ui.allocate_exact_size(vec2(WIDTH, 20.0), Sense::hover());
                ui.painter().text(
                    head.left_center(),
                    Align2::LEFT_CENTER,
                    st.background,
                    fonts::light(18.0),
                    p.text,
                );
                let close = Rect::from_center_size(
                    pos2(head.right() - CLOSE / 2.0 + 3.0, head.center().y),
                    vec2(CLOSE, CLOSE),
                );
                let response = ui
                    .interact(close, id.with("close"), Sense::click())
                    .on_hover_cursor(CursorIcon::PointingHand);
                let colour = if response.hovered() { p.text } else { p.muted };
                let cross = close.shrink(4.0);
                let stroke = Stroke::new(1.0, colour);
                ui.painter()
                    .line_segment([cross.left_top(), cross.right_bottom()], stroke);
                ui.painter()
                    .line_segment([cross.right_top(), cross.left_bottom()], stroke);
                drawn.close = close;
                if response.clicked() {
                    outcome = Outcome::Cancel;
                }

                // Saturation and value.
                let hsv = &mut picker.hsv;
                let (square, response) =
                    ui.allocate_exact_size(vec2(WIDTH, SQUARE_HEIGHT), Sense::click_and_drag());
                if let Some(pos) = response.interact_pointer_pos() {
                    hsv[1] = ((pos.x - square.left()) / square.width()).clamp(0.0, 1.0);
                    hsv[2] = ((square.bottom() - pos.y) / square.height()).clamp(0.0, 1.0);
                }
                let painter = ui.painter();
                paint_square(painter, square, hsv[0]);
                painter.rect_stroke(square, 0.0, Stroke::new(1.0, p.border), StrokeKind::Outside);
                let picked = colour_of(*hsv);
                let mark = pos2(
                    square.left() + hsv[1] * square.width(),
                    square.bottom() - hsv[2] * square.height(),
                );
                painter.with_clip_rect(square.expand(6.0)).circle_stroke(
                    mark,
                    5.0,
                    Stroke::new(1.0, contrast(picked)),
                );
                drawn.square = square;

                // The hue.
                let (hue, response) =
                    ui.allocate_exact_size(vec2(WIDTH, HUE_HEIGHT), Sense::click_and_drag());
                if let Some(pos) = response.interact_pointer_pos() {
                    hsv[0] = ((pos.x - hue.left()) / hue.width()).clamp(0.0, 1.0);
                }
                let painter = ui.painter();
                paint_hues(painter, hue);
                painter.rect_stroke(hue, 0.0, Stroke::new(1.0, p.border), StrokeKind::Outside);
                let x = (hue.left() + hsv[0] * hue.width()).round() + 0.5;
                let notch =
                    Rect::from_x_y_ranges(x - 2.0..=x + 2.0, hue.y_range()).expand2(vec2(0.0, 2.0));
                painter.rect_stroke(notch, 0.0, Stroke::new(1.0, p.text), StrokeKind::Middle);
                drawn.hue = hue;

                // Back to the default, and the colour in hex.
                let picked = colour_of(*hsv);
                ui.horizontal(|ui| {
                    let response = ui::choice(ui, st.default_background, picked == theme::VIEW_BG);
                    drawn.default = response.rect;
                    if response.clicked() {
                        *hsv = hsv_of(theme::VIEW_BG);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("#{}", to_hex(picked)))
                                .font(fonts::regular(12.0))
                                .color(p.muted),
                        );
                    });
                });
                ui.separator();

                // Where it goes.
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    for (k, target) in Target::ALL.into_iter().enumerate() {
                        let label = match target {
                            Target::Sculpt => st.to_sculpt,
                            Target::Reference => st.to_reference,
                            Target::Both => st.both,
                        };
                        let response = ui::choice(ui, label, false);
                        drawn.targets[k] = response.rect;
                        if response.clicked() {
                            outcome = Outcome::Apply(target);
                        }
                    }
                });
            });
        if let Some(shown) = shown {
            picker.drawn.window = shown.response.rect;
        }
        match outcome {
            Outcome::Open => self.background.picker = Some(picker),
            Outcome::Cancel => self.background.picker = None,
            Outcome::Apply(target) => {
                let colour = colour_of(picker.hsv);
                for &view in target.views() {
                    self.background.colours[view] = colour;
                }
                self.background.picker = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::study::tests::Harness;
    use super::*;
    use crate::render::readback::Readback;

    fn click(h: &mut Harness, at: egui::Pos2) {
        use crate::PointerPhase::{Down, Move, Up};
        h.pointer(Move, at);
        h.pointer(Down, at);
        h.pointer(Up, at);
    }

    /// The background button of view `index`'s toolbar, and whether it
    /// is the toolbar's last.
    fn button(h: &Harness, index: usize) -> (Rect, bool) {
        let bar = h.app.toolbar_layouts(h.app.view_rects)[index]
            .clone()
            .expect("a toolbar");
        let k = bar
            .options
            .iter()
            .position(|&o| o == ViewOption::Background);
        let k = k.expect("the button");
        (bar.buttons[k], k + 1 == bar.options.len())
    }

    /// Opens the window from view `index`'s button and lays it out.
    fn open(h: &mut Harness, index: usize) -> Drawn {
        let (at, _) = button(h, index);
        click(h, at.center());
        assert!(h.app.background_open(index));
        h.frames(1);
        h.app.background.picker.expect("open").drawn
    }

    /// The colour near the top middle of view `index`, as last drawn.
    fn view_pixel(h: &Harness, index: usize) -> Color32 {
        let app = &h.app;
        let target = &app.views[index];
        let mut encoder = app.device.create_command_encoder(&Default::default());
        let readback = Readback::copy(&app.device, &mut encoder, target);
        app.queue.submit([encoder.finish()]);
        readback.map();
        let pixels = readback.wait(&app.device).expect("pixels");
        let i = ((4 * target.size[0] + target.size[0] / 2) * 4) as usize;
        Color32::from_rgb(pixels[i], pixels[i + 1], pixels[i + 2])
    }

    #[test]
    fn colours_keep_their_hex_and_inks_turn_dark_on_light() {
        for colour in [
            theme::VIEW_BG,
            Color32::WHITE,
            Color32::from_rgb(0xf7, 0xf7, 0xf7),
            Color32::from_rgb(30, 60, 200),
        ] {
            assert_eq!(from_hex(&to_hex(colour)), Some(colour));
            let back = colour_of(hsv_of(colour));
            let close = back
                .to_array()
                .iter()
                .zip(colour.to_array())
                .all(|(a, b)| a.abs_diff(b) <= 1);
            assert!(close, "{colour:?} → {back:?}");
        }
        assert_eq!(
            from_hex("#F7f7F7"),
            Some(Color32::from_rgb(0xf7, 0xf7, 0xf7))
        );
        assert_eq!(from_hex("f7f7"), None);
        assert_eq!(from_hex("zzzzzz"), None);
        assert_eq!(theme::view_inks(theme::VIEW_BG), theme::DARK_VIEW_INKS);
        assert_eq!(theme::view_inks(Color32::WHITE), theme::LIGHT_VIEW_INKS);
        assert!(!theme::is_light(Color32::from_rgb(60, 60, 60)));
        assert!(theme::is_light(Color32::from_rgb(200, 200, 200)));
        assert!(
            !theme::is_light(Color32::from_rgb(30, 60, 200)),
            "deep blue"
        );
    }

    #[test]
    fn the_button_is_last_in_each_toolbar_and_is_no_switch() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        for index in [SCULPT, REFERENCE] {
            assert!(button(&h, index).1, "last in view {index}");
        }
        assert!(!ViewOption::Background.switches());
        // The reference alone takes the panels' button, above it.
        h.app.study.layout = super::super::study::Layout::Reference;
        h.frames(2);
        let bar = h.app.toolbar_layouts(h.app.view_rects)[REFERENCE].clone();
        assert!(bar.unwrap().options.contains(&ViewOption::Panels));
        assert!(button(&h, REFERENCE).1);
    }

    #[test]
    fn ref_from_the_reference_toolbar_colours_the_reference_view_only() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let drawn = open(&mut h, REFERENCE);
        assert!(h.app.option_on(ViewOption::Background, REFERENCE));
        // Beside the reference's toolbar, on the view's right edge, and
        // inside the view.
        let (at, _) = button(&h, REFERENCE);
        assert!(drawn.window.right() < at.left(), "{drawn:?}");
        assert!(h.app.view_rects[REFERENCE].contains_rect(drawn.window));

        // A light colour, from the square's top left: white.
        click(&mut h, drawn.square.left_top() + vec2(1.0, 1.0));
        let picked = h.app.view_background(REFERENCE);
        assert!(theme::is_light(picked), "{picked:?}");
        // Shown on the reference view meanwhile, and nowhere else yet.
        assert_eq!(h.app.background.colours, [theme::VIEW_BG; 2]);
        assert_eq!(h.app.view_background(SCULPT), theme::VIEW_BG);
        assert_eq!(view_pixel(&h, REFERENCE), picked);
        assert_eq!(h.app.view_inks(REFERENCE), theme::LIGHT_VIEW_INKS);
        assert_eq!(
            h.app.reference_look().ink,
            theme::LIGHT_VIEW_INKS.silhouette
        );

        let drawn = h.app.background.picker.unwrap().drawn;
        click(&mut h, drawn.targets[1].center());
        assert!(h.app.background.picker.is_none(), "closed");
        assert_eq!(h.app.background.colours, [theme::VIEW_BG, picked]);
        assert_eq!(view_pixel(&h, REFERENCE), picked);
        assert_eq!(view_pixel(&h, SCULPT), theme::VIEW_BG);
        assert_eq!(h.app.view_inks(SCULPT), theme::DARK_VIEW_INKS);

        // Both, then back to the default for the sculpt alone.
        let drawn = open(&mut h, SCULPT);
        assert!(drawn.window.left() > button(&h, SCULPT).0.right());
        assert_eq!(h.app.view_background(SCULPT), theme::VIEW_BG);
        click(&mut h, drawn.hue.center());
        click(&mut h, drawn.square.right_top() + vec2(-1.0, 1.0));
        let hue = h.app.view_background(SCULPT);
        assert!(hue.r() < 40 && hue.g() > 200 && hue.b() > 200, "{hue:?}");
        click(&mut h, drawn.targets[2].center());
        assert_eq!(h.app.background.colours, [hue; 2]);
        let drawn = open(&mut h, SCULPT);
        click(&mut h, drawn.default.center());
        assert_eq!(h.app.view_background(SCULPT), theme::VIEW_BG);
        click(&mut h, drawn.targets[0].center());
        assert_eq!(h.app.background.colours, [theme::VIEW_BG, hue]);
    }

    #[test]
    fn the_cross_and_escape_leave_both_views_as_they_were() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let before = [
            Color32::from_rgb(40, 40, 40),
            Color32::from_rgb(230, 230, 230),
        ];
        h.app.background.colours = before;
        for index in [SCULPT, REFERENCE] {
            // The cross.
            let drawn = open(&mut h, index);
            click(&mut h, drawn.square.center());
            assert_ne!(h.app.view_background(index), before[index], "previewed");
            click(&mut h, drawn.close.center());
            assert!(h.app.background.picker.is_none());
            assert_eq!(h.app.background.colours, before);
            assert_eq!(view_pixel(&h, index), before[index], "restored");

            // Escape.
            let drawn = open(&mut h, index);
            click(&mut h, drawn.hue.center());
            click(&mut h, drawn.square.left_bottom() + vec2(20.0, -60.0));
            assert_ne!(h.app.view_background(index), before[index]);
            h.key(egui::Key::Escape, false);
            assert!(h.app.background.picker.is_none());
            assert_eq!(h.app.background.colours, before);
            assert_eq!(h.app.view_background(index), before[index]);

            // Its button again.
            open(&mut h, index);
            let (at, _) = button(&h, index);
            click(&mut h, at.center());
            assert!(h.app.background.picker.is_none());
            assert_eq!(h.app.background.colours, before);
        }
        // The clicks on the window over the clay sculpted nothing.
        assert_eq!(h.app.sculpt.log().len(), 0);
    }

    #[test]
    fn the_colours_outlive_the_session_with_the_preferences() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let defaults = h.app.preferences();
        assert!(!defaults.contains("background"), "{defaults}");
        let colours = [
            Color32::from_rgb(0x12, 0x34, 0x56),
            Color32::from_rgb(0xf7, 0xf7, 0xf7),
        ];
        h.app.background.colours = colours;
        let saved = h.app.preferences();
        assert!(saved.ends_with("&background=123456,f7f7f7"), "{saved}");

        h.app.background.colours = [theme::VIEW_BG; 2];
        h.app.apply_preferences(&saved);
        assert_eq!(h.app.background.colours, colours);
        // What doesn't read is skipped.
        for bad in [
            "background=123456",
            "background=12345g,000000",
            "background=1,2,3",
        ] {
            h.app.apply_preferences(bad);
            assert_eq!(h.app.background.colours, colours, "{bad}");
        }
        h.app.apply_preferences(&defaults);
        assert_eq!(h.app.background.colours, colours, "absent: unchanged");
    }

    /// The app with the window open over a light reference view, saved as
    /// a PNG in the folder `SPLATSTUDY_SHOTS` names, to look at.
    #[test]
    fn a_picture_of_the_window() {
        let Ok(folder) = std::env::var("SPLATSTUDY_SHOTS") else {
            return;
        };
        let Some(mut h) = Harness::new() else {
            return;
        };
        h.app.study.views[REFERENCE].grid = true;
        h.app.study.views[SCULPT].plumb = true;
        h.app.background.colours[SCULPT] = Color32::from_rgb(0x2a, 0x30, 0x3a);
        h.frames(1);
        let drawn = open(&mut h, REFERENCE);
        let hue = drawn.hue.left_center() + vec2(drawn.hue.width() * 0.08, 0.0);
        click(&mut h, hue);
        click(&mut h, drawn.square.left_top() + vec2(30.0, 12.0));
        let at = h.app.view_rects[SCULPT].center() + vec2(60.0, -40.0);
        h.pointer(crate::PointerPhase::Move, at);

        let size = [1280u32, 720];
        let device = h.app.device.clone();
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shot"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        for k in 0..3 {
            h.app.frame(100.0 + f64::from(k) / 60.0, &view, size, 1.0);
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(size[0] * 4 * size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size[0] * 4),
                    rows_per_image: None,
                },
            },
            target.size(),
        );
        h.app.queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let pixels = buffer.slice(..).get_mapped_range().unwrap().to_vec();
        let file = std::fs::File::create(format!("{folder}/view-background.png")).unwrap();
        let mut png = png::Encoder::new(std::io::BufWriter::new(file), size[0], size[1]);
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        png.write_header()
            .unwrap()
            .write_image_data(&pixels)
            .unwrap();
    }

    #[test]
    fn background_window_fits_in_every_language() {
        use crate::ui::i18n::Lang;
        let Some(mut h) = Harness::new() else { return };
        let mut bad = Vec::new();
        for lang in [Lang::Ja, Lang::En, Lang::Fr] {
            h.app.lang = lang;
            h.frames(1);
            let d = open(&mut h, SCULPT);
            h.frames(1);
            let d2 = h.app.background.picker.expect("open").drawn;
            eprintln!(
                "{lang:?}: square {:?} targets {:?} window {:?}",
                d.square, d2.targets, d2.window
            );
            if d2.targets[2].right() > d2.square.right() + 0.5 {
                bad.push(format!(
                    "{lang:?}: choices reach {} past the picker's {}",
                    d2.targets[2].right(),
                    d2.square.right()
                ));
            }
            h.app.background.picker = None;
            h.frames(1);
        }
        for b in &bad {
            eprintln!("{b}");
        }
        assert!(bad.is_empty());
    }
}
