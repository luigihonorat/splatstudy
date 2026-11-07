// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The toolbars over the views: a thin strip of icons inside each view's
//! outer edge, the sculpt view's and the reference view's, each switching
//! its own view's options. The view menu and the keys switch the same
//! options, through [`App::apply`]. The sculpt's starts with the tool in
//! hand; it and the lighting open a column of choices beside the toolbar
//! (see [`crate::ui::flyout`]).

use egui::{Color32, CursorIcon, Rect, RectAlign, RichText, Sense, pos2, vec2};

use super::sculpting::{BRUSHES, brush_icon, brush_keys, brush_name};
use super::study::Layout;
use super::views::shown;
use super::{App, REFERENCE, SCULPT};
use crate::reference::Content;
use crate::render::clay::Lighting as Light;
use crate::sculpt::Brush;
use crate::ui::flyout::{self, Flyout, Row};
use crate::ui::i18n::{Tr, study::StudyTr};
use crate::ui::{fonts, icons, theme};

/// What a toolbar button, a key or a view menu item does to a view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ViewOption {
    /// The tool in hand, in the sculpt view: its button opens the tools.
    Tool,
    /// Perspective on, or orthographic.
    Perspective,
    /// The clay's edges, in the sculpt view.
    Wireframe,
    /// The clay shaded flat, facet by facet, in the sculpt view; off, smooth.
    FlatShading,
    /// The next lighting; its button opens the lightings.
    Lighting,
    Grid,
    Plumb,
    Silhouette,
    /// The silhouette difference tint, in the sculpt view.
    Difference,
    /// The reference over the sculpt view.
    Ghost,
    Mirror,
    /// Navigating either view turns the other: shared by both.
    Link,
    /// Fits the view to its clay or reference.
    Frame,
    /// Back to the starting camera, keeping the projection.
    Reset,
    /// Hides the side panels, or shows them: shared by both views.
    Panels,
    /// Opens the window that picks the view's background colour.
    Background,
    /// The calipers shown in the view, or hidden.
    Calipers,
}

/// What the reference view holds, which decides what its toolbar offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RefKind {
    Nothing,
    Splats,
    Mesh,
    Image,
}

impl ViewOption {
    /// The key, as menus and tooltips show it; empty for none.
    pub(super) fn keys(self) -> &'static str {
        use ViewOption::*;
        match self {
            // Each tool has its own key: see `brush_keys`.
            Tool => "",
            Perspective => "",
            Wireframe => "Shift+W",
            FlatShading => "",
            Lighting => "B",
            Grid => "G",
            Plumb => "V",
            Silhouette => "K",
            Difference => "Shift+K",
            Ghost => "O",
            Mirror => "R",
            Link => "L",
            Frame => "F",
            Reset => "Home",
            Panels => "Tab",
            Background => "",
            // Shift+E adds one.
            Calipers => "",
        }
    }

    /// Whether each view has this of its own, so that Shift + click can
    /// give both views the same.
    pub(super) fn per_view(self) -> bool {
        use ViewOption::*;
        matches!(
            self,
            Perspective | Lighting | Grid | Plumb | Silhouette | Mirror | Frame | Reset | Calipers
        )
    }

    /// The name, as menus and tooltips show it, for view `index`.
    pub(super) fn label(self, tr: &Tr, index: usize, kind: RefKind) -> String {
        use ViewOption::*;
        let st = tr.study;
        match self {
            Tool => tr.tools,
            Perspective => st.perspective,
            Wireframe => st.wireframe,
            FlatShading => st.flat_shading,
            Lighting => st.lighting,
            Grid => tr.grid,
            Plumb => tr.plumb_line,
            Silhouette => tr.silhouette,
            Difference => st.difference,
            Ghost => tr.ghost,
            Mirror => tr.mirror_view,
            Link => tr.link_cameras,
            Frame if index == SCULPT => tr.frame_sculpt,
            Frame if kind == RefKind::Image => tr.refs.fit_image,
            Frame => st.frame_reference,
            Reset => tr.reset_view,
            Panels => st.hide_panels,
            Background => st.background,
            Calipers => tr.calipers.calipers,
        }
        .to_owned()
    }

    fn icon(self, painter: &egui::Painter, rect: Rect, colour: Color32, shows: (Brush, Light)) {
        use ViewOption::*;
        let (tool, lighting) = shows;
        match self {
            Tool => brush_icon(tool)(painter, rect, colour),
            Perspective => icons::perspective(painter, rect, colour),
            Wireframe => icons::wireframe(painter, rect, colour),
            FlatShading => icons::flat_shading(painter, rect, colour),
            Lighting => {
                let which = Light::ALL.iter().position(|&l| l == lighting);
                icons::lighting(painter, rect, colour, which.unwrap_or(0));
            }
            Grid => icons::grid(painter, rect, colour),
            Plumb => icons::plumb(painter, rect, colour),
            Silhouette => icons::silhouette(painter, rect, colour),
            Difference => icons::difference(painter, rect, colour),
            Ghost => icons::ghost(painter, rect, colour),
            Mirror => icons::mirror(painter, rect, colour),
            Link => icons::link(painter, rect, colour),
            Frame => icons::frame(painter, rect, colour),
            Reset => icons::reset(painter, rect, colour),
            Panels => icons::panels(painter, rect, colour),
            Background => icons::background(painter, rect, colour),
            Calipers => icons::calipers(painter, rect, colour),
        }
    }

    /// Whether this option is switched on and off, its button inverted
    /// while it is on: not the tool, the lighting or an action.
    pub(super) fn switches(self) -> bool {
        use ViewOption::*;
        !matches!(self, Tool | Lighting | Frame | Reset | Background)
    }

    /// Whether this option's button opens a column of choices rather than
    /// switching it.
    pub(super) fn has_flyout(self) -> bool {
        matches!(self, ViewOption::Tool | ViewOption::Lighting)
    }
}

pub(super) fn lighting_name(st: &StudyTr, lighting: Light) -> &'static str {
    match lighting {
        Light::Studio => st.studio,
        Light::Raking => st.raking,
        Light::Top => st.top_light,
    }
}

/// The buttons of view `index`'s toolbar, in groups from the top: the
/// sculpt's tool, display, guides, comparison and camera, then the
/// sculpt's hides the panels, and last each view's background colour. A
/// picture has no camera, light or outline of its own, and only the clay
/// has edges, a ghost and a difference.
pub(super) fn groups(index: usize, kind: RefKind) -> Vec<Vec<ViewOption>> {
    let mut groups = option_groups(index, kind);
    groups.push(vec![ViewOption::Background]);
    groups
}

fn option_groups(index: usize, kind: RefKind) -> Vec<Vec<ViewOption>> {
    use ViewOption::*;
    if index == SCULPT {
        return vec![
            vec![Tool],
            vec![Perspective, Wireframe, FlatShading, Lighting],
            vec![Grid, Plumb, Calipers],
            vec![Silhouette, Difference, Ghost, Mirror],
            vec![Frame, Reset, Link],
            vec![Panels],
        ];
    }
    let mut groups = match kind {
        RefKind::Image => vec![vec![Grid, Plumb], vec![Mirror], vec![Frame]],
        RefKind::Mesh => vec![
            vec![Perspective, Lighting],
            vec![Grid, Plumb],
            vec![Silhouette, Mirror],
            vec![Frame, Reset, Link],
        ],
        RefKind::Splats | RefKind::Nothing => vec![
            vec![Perspective],
            vec![Grid, Plumb],
            vec![Silhouette, Mirror],
            vec![Frame, Reset, Link],
        ],
    };
    // Calipers lie on a 3D reference only.
    if matches!(kind, RefKind::Mesh | RefKind::Splats) {
        groups[1].push(Calipers);
    }
    groups
}

/// Whether view `index`'s toolbar sits on the view's left edge: each on
/// its view's outer edge, the sculpt's on the left until the sides are
/// swapped. A view shown alone keeps its side.
pub(super) fn on_left(index: usize, swapped: bool) -> bool {
    (index == SCULPT) != swapped
}

/// A button's side, in points, and the smallest it shrinks to in a short
/// view.
const BUTTON: f32 = 30.0;
const MIN_BUTTON: f32 = 22.0;
/// Room around the buttons inside a group, between groups, and between
/// the toolbar and the view's edge.
const PAD: f32 = 2.0;
const GAP: f32 = 6.0;
const INSET: f32 = 8.0;
/// Kept clear at the top and bottom of the view: its label, the timer, the
/// score and the credit.
const CLEAR: f32 = 40.0;

/// Where a toolbar's groups and buttons sit in a view of `rect`: each
/// group's panel, then every button's square, in order. `sizes` counts the
/// buttons of each group. Centred on the view's height, and smaller in a
/// short view.
pub(super) fn layout(rect: Rect, left: bool, sizes: &[usize]) -> (Vec<Rect>, Vec<Rect>) {
    let n = sizes.iter().sum::<usize>().max(1) as f32;
    let g = sizes.len() as f32;
    let fixed = g * 2.0 * PAD + (g - 1.0).max(0.0) * GAP;
    let room = rect.height() - 2.0 * CLEAR;
    let button = ((room - fixed) / n).clamp(MIN_BUTTON, BUTTON).floor();
    let height = n * button + fixed;
    let top = (rect.center().y - height / 2.0)
        .max(rect.top() + INSET)
        .round();
    let width = button + 2.0 * PAD;
    let x = if left {
        rect.left() + INSET
    } else {
        rect.right() - INSET - width
    }
    .round();
    let mut panels = Vec::new();
    let mut buttons = Vec::new();
    let mut y = top;
    for &count in sizes {
        let panel = Rect::from_min_size(pos2(x, y), vec2(width, count as f32 * button + 2.0 * PAD));
        for k in 0..count {
            let at = pos2(x + PAD, y + PAD + k as f32 * button);
            buttons.push(Rect::from_min_size(at, vec2(button, button)));
        }
        y = panel.bottom() + GAP;
        panels.push(panel);
    }
    (panels, buttons)
}

/// An open flyout: the view and the button it opened from, and where it
/// was last drawn (nothing before its first frame).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct OpenFlyout {
    pub(super) index: usize,
    pub(super) option: ViewOption,
    pub(super) panel: Rect,
    pub(super) rows: Vec<Rect>,
}

/// A toolbar as laid out in its view.
#[derive(Clone)]
pub(super) struct Toolbar {
    pub(super) options: Vec<ViewOption>,
    /// Each group's panel, top to bottom.
    pub(super) panels: Vec<Rect>,
    /// Each option's button.
    pub(super) buttons: Vec<Rect>,
}

impl App {
    pub(super) fn reference_kind(&self) -> RefKind {
        match self.reference.current.as_ref().map(|r| &r.content) {
            None => RefKind::Nothing,
            Some(Content::Splats(_)) => RefKind::Splats,
            Some(Content::Mesh(_)) => RefKind::Mesh,
            Some(Content::Image(_)) => RefKind::Image,
        }
    }

    /// Whether `option` is on in view `index`; actions are never on.
    pub(super) fn option_on(&self, option: ViewOption, index: usize) -> bool {
        use ViewOption::*;
        let aids = &self.study.views[index];
        match option {
            Perspective => !self.cameras[index].ortho,
            Wireframe => self.study.wireframe,
            FlatShading => self.study.flat_shading,
            Grid => aids.grid,
            Plumb => aids.plumb,
            Silhouette => aids.silhouette,
            Difference => self.study.matching.tint,
            Ghost => self.study.ghost,
            Mirror => aids.mirror,
            Link => self.study.link,
            Panels => self.panels.hidden(),
            // The tool in hand shows bright; the lighting while its
            // choices are open.
            Tool => true,
            Lighting => self.flyout_open(option, index),
            Background => self.background_open(index),
            Calipers => self.calipers.set.shown[index],
            Frame | Reset => false,
        }
    }

    /// Switches `option` in view `index`, or does it.
    pub(super) fn apply(&mut self, option: ViewOption, index: usize) {
        use ViewOption::*;
        let aids = &mut self.study.views[index];
        match option {
            Tool => self.toggle_flyout(option, index),
            Perspective => {
                let camera = &mut self.cameras[index];
                camera.ortho = !camera.ortho;
                self.follow_camera(index);
            }
            Wireframe => self.study.wireframe ^= true,
            FlatShading => self.study.flat_shading ^= true,
            Lighting => aids.lighting = aids.lighting.next(),
            Grid => aids.grid ^= true,
            Plumb => aids.plumb ^= true,
            Silhouette => aids.silhouette ^= true,
            Difference => self.study.matching.tint ^= true,
            Ghost => self.study.ghost ^= true,
            Mirror => aids.mirror ^= true,
            Link => self.toggle_link(),
            Frame if index == SCULPT => self.frame_sculpt(),
            Frame => self.frame_reference(),
            Reset => self.reset_view(index),
            Panels => self.toggle_panels(),
            Background => self.toggle_background(index),
            Calipers => self.calipers.set.shown[index] ^= true,
        }
        self.dirty = true;
    }

    /// Switches `option` in view `index`, or does it, and does the same in
    /// the other view if it has the option too: Shift + click on a toolbar
    /// button.
    pub(super) fn apply_to_both(&mut self, option: ViewOption, index: usize) {
        use ViewOption::*;
        self.apply(option, index);
        let other = if index == SCULPT { REFERENCE } else { SCULPT };
        if !groups(other, self.reference_kind())
            .concat()
            .contains(&option)
        {
            return;
        }
        let (from, to) = (self.study.views[index], &mut self.study.views[other]);
        match option {
            Perspective => self.cameras[other].ortho = self.cameras[index].ortho,
            Lighting => to.lighting = from.lighting,
            Grid => to.grid = from.grid,
            Plumb => to.plumb = from.plumb,
            Silhouette => to.silhouette = from.silhouette,
            Mirror => to.mirror = from.mirror,
            Calipers => {
                let shown = &mut self.calipers.set.shown;
                shown[other] = shown[index];
            }
            Frame | Reset => self.apply(option, other),
            _ => {}
        }
    }

    /// Each shown view's toolbar, as this frame lays them out in `rects`.
    pub(super) fn toolbar_layouts(&self, rects: [Rect; 2]) -> [Option<Toolbar>; 2] {
        let kind = self.reference_kind();
        [SCULPT, REFERENCE].map(|index| {
            let rect = rects[index];
            if !shown(rect) {
                return None;
            }
            let mut groups = groups(index, kind);
            // The reference alone has the sculpt's panel button, so it
            // is never out of reach; the background stays last.
            if index == REFERENCE && !shown(rects[SCULPT]) {
                groups.insert(groups.len() - 1, vec![ViewOption::Panels]);
            }
            let sizes: Vec<usize> = groups.iter().map(Vec::len).collect();
            let (panels, buttons) = layout(rect, on_left(index, self.study.swapped), &sizes);
            Some(Toolbar {
                options: groups.concat(),
                panels,
                buttons,
            })
        })
    }

    /// The area each view's toolbar covers, where the view takes no press
    /// and shows no brush: empty where there is none.
    pub(super) fn toolbar_areas(&self, rects: [Rect; 2]) -> [Rect; 2] {
        self.toolbar_layouts(rects).map(|layout| {
            layout.map_or(Rect::NOTHING, |bar| {
                bar.panels.iter().fold(Rect::NOTHING, |a, p| a.union(*p))
            })
        })
    }

    /// Whether `option`'s flyout is open over view `index`.
    pub(super) fn flyout_open(&self, option: ViewOption, index: usize) -> bool {
        self.flyout
            .as_ref()
            .is_some_and(|f| f.option == option && f.index == index)
    }

    /// Opens `option`'s flyout over view `index`, or closes it if open;
    /// another flyout closes.
    pub(super) fn toggle_flyout(&mut self, option: ViewOption, index: usize) {
        self.flyout = (!self.flyout_open(option, index)).then_some(OpenFlyout {
            index,
            option,
            panel: Rect::NOTHING,
            rows: Vec::new(),
        });
    }

    /// Whether `option`'s button opened the window open over view `index`:
    /// the background colour's.
    fn window_open(&self, option: ViewOption, index: usize) -> bool {
        option == ViewOption::Background && self.background_open(index)
    }

    /// Where the open flyout was drawn last: there the view takes no
    /// scroll and shows no brush. Empty when none is open.
    pub(super) fn flyout_area(&self) -> Rect {
        self.flyout.as_ref().map_or(Rect::NOTHING, |f| f.panel)
    }

    /// Escape closes an open flyout, and does nothing else. Before the
    /// other shortcuts.
    pub(super) fn flyout_keys(&mut self, ui: &egui::Ui) {
        if self.flyout.is_some()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.flyout = None;
        }
    }

    /// Draws the open flyout beside its button and takes its clicks:
    /// picking a row picks that tool or lighting.
    fn flyout_ui(&mut self, ui: &egui::Ui, rects: [Rect; 2], layouts: &[Option<Toolbar>; 2]) {
        let Some(open) = &self.flyout else {
            return;
        };
        let (index, option) = (open.index, open.option);
        // Gone with its view, or with its button: a picture has no
        // lighting.
        let button = layouts[index].as_ref().and_then(|bar| {
            let k = bar.options.iter().position(|&o| o == option)?;
            Some(bar.buttons[k])
        });
        let Some(button) = button else {
            self.flyout = None;
            return;
        };
        let tr = self.lang.tr();
        let tool_icons = BRUSHES.map(|(brush, _, _)| brush_icon(brush));
        let light_icons: [_; Light::ALL.len()] = std::array::from_fn(|k| {
            move |p: &egui::Painter, r: Rect, c: Color32| icons::lighting(p, r, c, k)
        });
        let (rows, chosen): (Vec<Row>, _) = match option {
            ViewOption::Tool => (
                BRUSHES
                    .iter()
                    .zip(&tool_icons)
                    .map(|(&(brush, _, _), icon)| Row {
                        icon,
                        label: brush_name(tr, brush),
                    })
                    .collect(),
                BRUSHES.iter().position(|&(b, _, _)| b == self.tool.tool),
            ),
            _ => (
                Light::ALL
                    .iter()
                    .zip(&light_icons)
                    .map(|(&light, icon)| Row {
                        icon,
                        label: lighting_name(tr.study, light),
                    })
                    .collect(),
                Light::ALL
                    .iter()
                    .position(|&l| l == self.study.views[index].lighting),
            ),
        };
        let flyout = Flyout {
            id: ui.scope_id().with(("flyout", index)),
            button,
            right: on_left(index, self.study.swapped),
            within: rects[index],
            rows: &rows,
            chosen,
        };
        let (panel, row_rects) = flyout.layout(ui.ctx());
        match flyout.show(ui) {
            flyout::Outcome::Open => {
                if let Some(open) = &mut self.flyout {
                    (open.panel, open.rows) = (panel, row_rects);
                }
            }
            flyout::Outcome::Closed => self.flyout = None,
            flyout::Outcome::Picked(k) => {
                // Shift + click lights both views alike.
                let both = ui.input(|i| i.modifiers.shift);
                match option {
                    ViewOption::Tool => self.choose_brush(BRUSHES[k].0),
                    _ if both => {
                        for aids in &mut self.study.views {
                            aids.lighting = Light::ALL[k];
                        }
                    }
                    _ => self.study.views[index].lighting = Light::ALL[k],
                }
                self.flyout = None;
                self.dirty = true;
            }
        }
    }

    /// Draws both toolbars and takes their clicks. Called after everything
    /// else that takes the pointer in the views, so it is on top.
    pub(super) fn toolbars(&mut self, ui: &egui::Ui, rects: [Rect; 2]) {
        let tr = self.lang.tr();
        let kind = self.reference_kind();
        let layouts = self.toolbar_layouts(rects);
        for (index, layout) in layouts.clone().into_iter().enumerate() {
            let Some(Toolbar {
                options,
                panels,
                buttons,
            }) = layout
            else {
                continue;
            };
            let id = ui.scope_id().with(("toolbar", index));
            let painter = ui.painter().with_clip_rect(rects[index]);
            for (k, panel) in panels.iter().enumerate() {
                // The gaps between the buttons, too, so a press there is
                // not a stroke.
                ui.interact(*panel, id.with(("panel", k)), Sense::click_and_drag());
                painter.rect_filled(*panel, 0.0, theme::TOOLBAR_BG);
            }
            let shows = (self.tool.tool, self.study.views[index].lighting);
            for (k, (option, button)) in options.into_iter().zip(buttons).enumerate() {
                let response = ui
                    .interact(button, id.with(k), Sense::click())
                    .on_hover_cursor(CursorIcon::PointingHand);
                let on = self.option_on(option, index);
                let hovered = response.hovered();
                // A switch that is on is inverted: a light square, its
                // icon dark.
                let inverted = on && option.switches();
                if inverted {
                    let fill = if hovered {
                        theme::TOOL_ON.gamma_multiply(0.8)
                    } else {
                        theme::TOOL_ON
                    };
                    painter.rect_filled(button.shrink(1.0), 0.0, fill);
                } else if hovered
                    || self.flyout_open(option, index)
                    || self.window_open(option, index)
                {
                    painter.rect_filled(button, 0.0, theme::TOOL_HOVER);
                }
                let colour = if inverted {
                    theme::TOOL_INVERTED
                } else if on || hovered {
                    theme::TOOL_ON
                } else {
                    theme::TOOL_OFF
                };
                option.icon(&painter, button, colour, shows);
                if response.clicked() {
                    let both = option.per_view() && ui.input(|i| i.modifiers.shift);
                    if option.has_flyout() {
                        self.toggle_flyout(option, index);
                    } else if both {
                        self.apply_to_both(option, index);
                    } else {
                        self.apply(option, index);
                    }
                }
                // Its flyout or window, open, takes the tooltip's place.
                if self.flyout_open(option, index) || self.window_open(option, index) {
                    continue;
                }
                let (tool, lighting) = shows;
                let (label, keys) = match option {
                    ViewOption::Tool => (brush_name(tr, tool).to_owned(), brush_keys(tool)),
                    ViewOption::Lighting => (
                        format!(
                            "{} · {}",
                            option.label(tr, index, kind),
                            lighting_name(tr.study, lighting)
                        ),
                        option.keys().to_owned(),
                    ),
                    _ => (option.label(tr, index, kind), option.keys().to_owned()),
                };
                // Beside the button, towards the view's middle.
                let mut tip = egui::Tooltip::for_enabled(&response);
                tip.popup = tip.popup.align(if on_left(index, self.study.swapped) {
                    RectAlign::RIGHT
                } else {
                    RectAlign::LEFT
                });
                tip.show(|ui| {
                    let muted = ui.visuals().widgets.inactive.fg_stroke.color;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 18.0;
                        ui.label(RichText::new(label).font(fonts::regular(13.0)));
                        // The perspective and the flat shading have no key.
                        if !keys.is_empty() {
                            ui.label(RichText::new(keys).font(fonts::regular(13.0)).color(muted));
                        }
                    });
                    let hint = if option == ViewOption::Tool {
                        Some(tr.mesh.tool_pie_hint)
                    } else {
                        (option.per_view() && self.study.layout == Layout::Both)
                            .then_some(tr.study.both_views_hint)
                    };
                    if let Some(hint) = hint {
                        ui.label(RichText::new(hint).font(fonts::regular(13.0)).color(muted));
                    }
                });
            }
        }
        self.flyout_ui(ui, rects, &layouts);
        self.background_window(ui, rects, &layouts);
    }
}

#[cfg(test)]
mod tests {
    use super::super::study::tests::Harness;
    use super::*;

    fn view() -> Rect {
        Rect::from_min_size(pos2(200.0, 30.0), vec2(600.0, 800.0))
    }

    #[test]
    fn each_toolbar_sits_on_its_views_outer_edge() {
        assert!(on_left(SCULPT, false) && !on_left(REFERENCE, false));
        assert!(!on_left(SCULPT, true) && on_left(REFERENCE, true));
        let (panels, _) = layout(view(), true, &[3, 2]);
        assert_eq!(panels[0].left(), view().left() + INSET);
        let (panels, _) = layout(view(), false, &[3, 2]);
        assert_eq!(panels[0].right(), view().right() - INSET);
    }

    #[test]
    fn buttons_fill_their_groups_and_shrink_in_a_short_view() {
        let sizes = [3, 2, 4, 3];
        let (panels, buttons) = layout(view(), true, &sizes);
        assert_eq!(panels.len(), 4);
        assert_eq!(buttons.len(), 12);
        assert_eq!(buttons[0].width(), BUTTON);
        assert!(
            buttons
                .iter()
                .all(|b| panels.iter().any(|p| p.contains_rect(*b)))
        );
        // Groups apart, top to bottom, centred on the view.
        for pair in panels.windows(2) {
            assert_eq!(pair[1].top() - pair[0].bottom(), GAP);
        }
        let middle = (panels[0].top() + panels[3].bottom()) / 2.0;
        assert!((middle - view().center().y).abs() <= 1.0);

        let short = Rect::from_min_size(pos2(0.0, 0.0), vec2(500.0, 420.0));
        let (panels, buttons) = layout(short, true, &sizes);
        assert!(buttons[0].width() < BUTTON && buttons[0].width() >= MIN_BUTTON);
        assert!(panels[3].bottom() <= short.bottom() - INSET);
        assert!(panels[0].top() >= short.top() + INSET);
    }

    /// The app's toolbar button for `option` in view `index`.
    fn button_of(h: &Harness, option: ViewOption, index: usize) -> Rect {
        let bar = h.app.toolbar_layouts(h.app.view_rects)[index].clone();
        let bar = bar.expect("a toolbar");
        let k = bar.options.iter().position(|&o| o == option);
        bar.buttons[k.expect("the button")]
    }

    fn click(h: &mut Harness, at: egui::Pos2) {
        use crate::PointerPhase::{Down, Move, Up};
        h.pointer(Move, at);
        h.pointer(Down, at);
        h.pointer(Up, at);
    }

    fn strokes(h: &Harness) -> usize {
        h.app.sculpt.log().len()
    }

    #[test]
    fn the_tool_button_opens_the_tools_and_a_row_picks_one_without_sculpting() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        // Close up, the clay fills the view under the flyout too.
        h.app.cameras[SCULPT].distance = 1.3;
        h.frames(1);
        assert_eq!(groups(SCULPT, RefKind::Nothing)[0], [ViewOption::Tool]);
        let button = button_of(&h, ViewOption::Tool, SCULPT);
        assert!(button.bottom() < button_of(&h, ViewOption::Perspective, SCULPT).top());
        click(&mut h, button.center());
        assert!(h.app.flyout_open(ViewOption::Tool, SCULPT));
        h.frames(1);
        let open = h.app.flyout.clone().expect("open");
        assert_eq!(open.rows.len(), 8);
        // Beside the toolbar, towards the view's middle, and over the clay.
        assert!(open.panel.left() > button.right());
        let pinch = open.rows[6].center();
        let rect = h.app.view_rects[SCULPT];
        assert!(h.app.pick(rect, pinch).is_some(), "over the clay");
        click(&mut h, pinch);
        assert_eq!(h.app.tool.tool, Brush::Pinch);
        assert!(h.app.flyout.is_none(), "closed by the pick");
        assert_eq!(strokes(&h), 0, "the click is no stroke");

        // The button opens it and closes it again.
        click(&mut h, button.center());
        assert!(h.app.flyout.is_some());
        click(&mut h, button.center());
        assert!(h.app.flyout.is_none());

        // A press on the clay elsewhere closes it, and sculpts nothing.
        click(&mut h, button.center());
        h.drag(rect.center(), rect.center() + egui::vec2(40.0, 0.0));
        assert!(h.app.flyout.is_none());
        assert_eq!(strokes(&h), 0);
        assert_eq!(h.app.tool.tool, Brush::Pinch);
        // Closed, the same drag sculpts.
        h.drag(rect.center(), rect.center() + egui::vec2(40.0, 0.0));
        assert!(strokes(&h) > 0);

        // Escape closes it.
        click(&mut h, button.center());
        h.key(egui::Key::Escape, false);
        assert!(h.app.flyout.is_none());
        // Space's size and strength take its place.
        click(&mut h, button.center());
        h.pointer(crate::PointerPhase::Move, rect.center());
        h.app.push_event(crate::InputEvent::Key {
            key: egui::Key::Space,
            pressed: true,
            repeat: false,
        });
        h.frames(1);
        assert!(h.app.popup_open() && !h.app.pie_open() && h.app.flyout.is_none());
    }

    #[test]
    fn the_lighting_button_opens_the_lightings_and_b_still_cycles() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let before = h.app.study.views[SCULPT].lighting;
        let button = button_of(&h, ViewOption::Lighting, SCULPT);
        click(&mut h, button.center());
        assert!(h.app.flyout_open(ViewOption::Lighting, SCULPT));
        assert_eq!(h.app.study.views[SCULPT].lighting, before, "no cycling");
        h.frames(1);
        let rows = h.app.flyout.clone().unwrap().rows;
        assert_eq!(rows.len(), Light::ALL.len());
        click(&mut h, rows[1].center());
        assert_eq!(h.app.study.views[SCULPT].lighting, Light::ALL[1]);
        assert!(h.app.flyout.is_none());
        // Opening the tools closes the lightings.
        click(&mut h, button.center());
        let tool = button_of(&h, ViewOption::Tool, SCULPT);
        click(&mut h, tool.center());
        assert!(h.app.flyout_open(ViewOption::Tool, SCULPT));
        h.key(egui::Key::Escape, false);
        // B goes on cycling.
        let centre = h.app.view_rects[SCULPT].center();
        h.pointer(crate::PointerPhase::Move, centre);
        h.key(egui::Key::B, false);
        assert_eq!(h.app.study.views[SCULPT].lighting, Light::ALL[2]);

        // Swapped, the sculpt's flyout opens to the left of its toolbar,
        // still towards the view's middle.
        h.app.study.swapped = true;
        h.frames(1);
        let button = button_of(&h, ViewOption::Tool, SCULPT);
        click(&mut h, button.center());
        h.frames(1);
        let panel = h.app.flyout.clone().unwrap().panel;
        assert!(panel.right() < button.left());
        assert!(h.app.view_rects[SCULPT].contains_rect(panel));
    }

    #[test]
    fn shift_click_switches_an_aid_in_both_views() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let shift = |h: &mut Harness, held: bool| {
            h.app
                .push_event(crate::InputEvent::Modifiers(crate::input::Modifiers {
                    shift: held,
                    ..Default::default()
                }));
        };
        let press = |h: &mut Harness, option, index| {
            let at = button_of(h, option, index).center();
            click(h, at);
        };
        let views = |h: &Harness| h.app.study.views.map(|v| (v.grid, v.silhouette));

        shift(&mut h, true);
        press(&mut h, ViewOption::Grid, SCULPT);
        assert_eq!(views(&h), [(true, false); 2]);
        press(&mut h, ViewOption::Silhouette, REFERENCE);
        assert_eq!(views(&h), [(true, true); 2]);
        // Both ortho, then both perspective again from either side.
        press(&mut h, ViewOption::Perspective, SCULPT);
        assert!(h.app.cameras.iter().all(|c| c.ortho));
        // Already on in one view and off in the other: both take the
        // clicked view's new state.
        h.app.cameras[SCULPT].ortho = false;
        press(&mut h, ViewOption::Perspective, REFERENCE);
        assert!(h.app.cameras.iter().all(|c| !c.ortho));

        // A plain click switches its own view only.
        shift(&mut h, false);
        press(&mut h, ViewOption::Grid, REFERENCE);
        assert_eq!(views(&h), [(true, true), (false, true)]);

        // Shift + a lighting lights both views.
        press(&mut h, ViewOption::Lighting, SCULPT);
        h.frames(1);
        let rows = h.app.flyout.clone().unwrap().rows;
        shift(&mut h, true);
        click(&mut h, rows[2].center());
        assert!(
            h.app
                .study
                .views
                .iter()
                .all(|v| v.lighting == Light::ALL[2])
        );
    }

    #[test]
    fn flat_shading_sits_under_the_wireframe_and_is_kept_with_the_scene() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let wire = button_of(&h, ViewOption::Wireframe, SCULPT);
        let flat = button_of(&h, ViewOption::FlatShading, SCULPT);
        assert_eq!(flat.left(), wire.left());
        assert_eq!(flat.top(), wire.bottom(), "right under the wireframe");
        assert!(!ViewOption::FlatShading.per_view());
        assert!(!h.app.sculpt_look().faceted, "smooth to start with");

        click(&mut h, flat.center());
        assert!(h.app.study.flat_shading);
        assert!(h.app.option_on(ViewOption::FlatShading, SCULPT));
        assert!(h.app.sculpt_look().faceted);
        assert!(!h.app.study.wireframe, "the wireframe stays as it was");
        assert_eq!(strokes(&h), 0, "the click is no stroke");

        // Kept in the scene, like the wireframe.
        let c = h.app.view_rects[SCULPT].center();
        h.drag(c, c + egui::vec2(30.0, 0.0));
        let saved = h.app.autosave_bytes().expect("worth keeping").scene;
        h.app.new_scene();
        h.app.study.flat_shading = false;
        assert!(h.app.restore_autosave(&saved, None));
        assert!(h.app.study.flat_shading && h.app.sculpt_look().faceted);

        // Clicked again, smooth.
        let flat = button_of(&h, ViewOption::FlatShading, SCULPT);
        click(&mut h, flat.center());
        assert!(!h.app.study.flat_shading && !h.app.sculpt_look().faceted);
    }

    #[test]
    fn a_picture_offers_only_what_applies_to_it() {
        let flat: Vec<ViewOption> = groups(REFERENCE, RefKind::Image).concat();
        assert_eq!(
            flat,
            [
                ViewOption::Grid,
                ViewOption::Plumb,
                ViewOption::Mirror,
                ViewOption::Frame,
                ViewOption::Background,
            ]
        );
        let splats = groups(REFERENCE, RefKind::Splats).concat();
        assert!(!splats.contains(&ViewOption::Lighting));
        assert!(
            groups(REFERENCE, RefKind::Mesh)
                .concat()
                .contains(&ViewOption::Lighting)
        );
        for kind in [
            RefKind::Nothing,
            RefKind::Splats,
            RefKind::Mesh,
            RefKind::Image,
        ] {
            let options = groups(REFERENCE, kind).concat();
            for sculpt_only in [
                ViewOption::Wireframe,
                ViewOption::FlatShading,
                ViewOption::Ghost,
                ViewOption::Difference,
            ] {
                assert!(!options.contains(&sculpt_only), "{kind:?}");
            }
        }
    }
}
