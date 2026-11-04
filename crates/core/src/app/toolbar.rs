// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The toolbars over the views: a thin strip of icons inside each view's
//! outer edge, the sculpt view's and the reference view's, each switching
//! its own view's options. The view menu and the keys switch the same
//! options, through [`App::apply`].

use egui::{Color32, CursorIcon, Rect, RectAlign, RichText, Sense, pos2, vec2};

use super::views::shown;
use super::{App, REFERENCE, SCULPT};
use crate::reference::Content;
use crate::render::clay::Lighting as Light;
use crate::ui::i18n::{Tr, study::StudyTr};
use crate::ui::{fonts, icons, theme};

/// What a toolbar button, a key or a view menu item does to a view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ViewOption {
    /// Perspective on, or orthographic.
    Perspective,
    /// The clay's edges, in the sculpt view.
    Wireframe,
    /// The next lighting.
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
    /// The key, as menus and tooltips show it.
    pub(super) fn keys(self) -> &'static str {
        use ViewOption::*;
        match self {
            Perspective => "5",
            Wireframe => "Shift+W",
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
        }
    }

    /// The name, as menus and tooltips show it, for view `index`.
    pub(super) fn label(self, tr: &Tr, index: usize, kind: RefKind) -> String {
        use ViewOption::*;
        let st = tr.study;
        match self {
            Perspective => st.perspective,
            Wireframe => st.wireframe,
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
        }
        .to_owned()
    }

    fn icon(self, painter: &egui::Painter, rect: Rect, colour: Color32, lighting: Light) {
        use ViewOption::*;
        match self {
            Perspective => icons::perspective(painter, rect, colour),
            Wireframe => icons::wireframe(painter, rect, colour),
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
        }
    }
}

pub(super) fn lighting_name(st: &StudyTr, lighting: Light) -> &'static str {
    match lighting {
        Light::Studio => st.studio,
        Light::Raking => st.raking,
        Light::Top => st.top_light,
    }
}

/// The buttons of view `index`'s toolbar, in groups from the top:
/// display, guides, comparison and camera. A picture has no camera, light
/// or outline of its own, and only the clay has edges, a ghost and a
/// difference.
pub(super) fn groups(index: usize, kind: RefKind) -> Vec<Vec<ViewOption>> {
    use ViewOption::*;
    if index == SCULPT {
        return vec![
            vec![Perspective, Wireframe, Lighting],
            vec![Grid, Plumb],
            vec![Silhouette, Difference, Ghost, Mirror],
            vec![Frame, Reset, Link],
        ];
    }
    match kind {
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
    }
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
            Grid => aids.grid,
            Plumb => aids.plumb,
            Silhouette => aids.silhouette,
            Difference => self.study.matching.tint,
            Ghost => self.study.ghost,
            Mirror => aids.mirror,
            Link => self.study.link,
            Lighting | Frame | Reset => false,
        }
    }

    /// Switches `option` in view `index`, or does it.
    pub(super) fn apply(&mut self, option: ViewOption, index: usize) {
        use ViewOption::*;
        let aids = &mut self.study.views[index];
        match option {
            Perspective => {
                let camera = &mut self.cameras[index];
                camera.ortho = !camera.ortho;
                self.follow_camera(index);
            }
            Wireframe => self.study.wireframe ^= true,
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
        }
        self.dirty = true;
    }

    /// Each shown view's toolbar, as this frame lays them out in `rects`.
    pub(super) fn toolbar_layouts(&self, rects: [Rect; 2]) -> [Option<Toolbar>; 2] {
        let kind = self.reference_kind();
        [SCULPT, REFERENCE].map(|index| {
            let rect = rects[index];
            if !shown(rect) {
                return None;
            }
            let groups = groups(index, kind);
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

    /// Draws both toolbars and takes their clicks. Called after everything
    /// else that takes the pointer in the views, so it is on top.
    pub(super) fn toolbars(&mut self, ui: &egui::Ui, rects: [Rect; 2]) {
        let tr = self.lang.tr();
        let kind = self.reference_kind();
        let layouts = self.toolbar_layouts(rects);
        for (index, layout) in layouts.into_iter().enumerate() {
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
            let lighting = self.study.views[index].lighting;
            for (k, (option, button)) in options.into_iter().zip(buttons).enumerate() {
                let response = ui
                    .interact(button, id.with(k), Sense::click())
                    .on_hover_cursor(CursorIcon::PointingHand);
                let on = self.option_on(option, index);
                let hovered = response.hovered();
                if hovered {
                    painter.rect_filled(button, 0.0, theme::TOOL_HOVER);
                }
                let colour = if on || hovered {
                    theme::TOOL_ON
                } else {
                    theme::TOOL_OFF
                };
                option.icon(&painter, button, colour, lighting);
                let mut label = option.label(tr, index, kind);
                if option == ViewOption::Lighting {
                    label = format!("{label} · {}", lighting_name(tr.study, lighting));
                }
                // Beside the button, towards the view's middle.
                let mut tip = egui::Tooltip::for_enabled(&response);
                tip.popup = tip.popup.align(if on_left(index, self.study.swapped) {
                    RectAlign::RIGHT
                } else {
                    RectAlign::LEFT
                });
                tip.show(|ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 18.0;
                        ui.label(RichText::new(label).font(fonts::regular(13.0)));
                        let muted = ui.visuals().widgets.inactive.fg_stroke.color;
                        ui.label(
                            RichText::new(option.keys())
                                .font(fonts::regular(13.0))
                                .color(muted),
                        );
                    });
                });
                if response.clicked() {
                    self.apply(option, index);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn a_picture_offers_only_what_applies_to_it() {
        let flat: Vec<ViewOption> = groups(REFERENCE, RefKind::Image).concat();
        assert_eq!(
            flat,
            [
                ViewOption::Grid,
                ViewOption::Plumb,
                ViewOption::Mirror,
                ViewOption::Frame
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
                ViewOption::Ghost,
                ViewOption::Difference,
            ] {
                assert!(!options.contains(&sculpt_only), "{kind:?}");
            }
        }
    }
}
