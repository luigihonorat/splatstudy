// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Hiding the side panels so the views take the whole width: Tab hides
//! the panels, Shift+Tab the panels and the reference view too, so the
//! sculpt view fills the window. The menu bar always stays.
//!
//! The hidden state lasts for the session; scenes don't keep it.
//!
//! The window menu also chooses which groups the panels hold. That choice
//! is a preference, so it outlives the session. A side panel with none of
//! its groups left goes away, and the views take its width.

use egui::{Event, Key};

use super::study::Layout;
use super::{App, REFERENCE, views};
use crate::ui::{i18n::Tr, menu};

/// Whether the panels show, and what to put back after Shift+Tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Panels {
    hidden: bool,
    /// Shift+Tab's view of the sculpt alone, and what there was before.
    focus: Option<Before>,
    /// The groups taken out of the panels from the window menu, by
    /// [`Group`] index. All of them show at first.
    removed: [bool; Group::ALL.len()],
}

/// A group of controls in a side panel, which the window menu can take out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Group {
    ToolSettings,
    Subdivision,
    Remeshing,
    QuadRemesh,
    Tweak,
    Calipers,
    Reference,
    Alignment,
    Crop,
    Credit,
}

impl Group {
    pub(super) const ALL: [Group; 10] = [
        Group::ToolSettings,
        Group::Subdivision,
        Group::Remeshing,
        Group::QuadRemesh,
        Group::Tweak,
        Group::Calipers,
        Group::Reference,
        Group::Alignment,
        Group::Crop,
        Group::Credit,
    ];
    /// The sculpt panel's groups, on the left.
    pub(super) const SCULPT: [Group; 6] = [
        Group::ToolSettings,
        Group::Subdivision,
        Group::Remeshing,
        Group::QuadRemesh,
        Group::Tweak,
        Group::Calipers,
    ];
    /// The reference panel's groups, on the right.
    pub(super) const REFERENCE: [Group; 4] = [
        Group::Reference,
        Group::Alignment,
        Group::Crop,
        Group::Credit,
    ];

    /// Its name in the preferences string.
    pub(super) fn name(self) -> &'static str {
        match self {
            Group::ToolSettings => "tools",
            Group::Subdivision => "subdivision",
            Group::Remeshing => "remeshing",
            Group::QuadRemesh => "quad",
            Group::Tweak => "tweak",
            Group::Calipers => "calipers",
            Group::Reference => "reference",
            Group::Alignment => "alignment",
            Group::Crop => "crop",
            Group::Credit => "credit",
        }
    }

    /// Its heading in the panel, which the window menu repeats.
    pub(super) fn title(self, tr: &Tr) -> &'static str {
        match self {
            Group::ToolSettings => tr.mesh.tool_settings,
            Group::Subdivision => tr.mesh.subdivision,
            Group::Remeshing => tr.mesh.remeshing,
            Group::QuadRemesh => tr.mesh.quad_remesh,
            Group::Tweak => tr.tweak.tweak,
            Group::Calipers => tr.calipers.calipers,
            Group::Reference => tr.reference_tools,
            Group::Alignment => tr.refs.alignment,
            Group::Crop => tr.refs.crop,
            Group::Credit => tr.refs.credit,
        }
    }
}

/// What Shift+Tab hid, to show again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Before {
    hidden: bool,
    layout: Layout,
    /// The view shown alone.
    alone: Layout,
}

impl Panels {
    pub(super) fn hidden(&self) -> bool {
        self.hidden
    }

    /// Whether `group` is in its panel (the window menu's choice; Tab
    /// hides the panels without changing it).
    pub(super) fn holds(&self, group: Group) -> bool {
        !self.removed[group as usize]
    }

    /// Whether any of `groups` is in its panel, so the panel is worth its
    /// width.
    pub(super) fn holds_any(&self, groups: &[Group]) -> bool {
        groups.iter().any(|&g| self.holds(g))
    }

    /// The window menu: takes `group` out of its panel or puts it back.
    pub(super) fn toggle_group(&mut self, group: Group) {
        self.removed[group as usize] ^= true;
    }

    /// The groups taken out, for the preferences: `crop,credit`.
    pub(super) fn removed_names(&self) -> String {
        let names: Vec<_> = Group::ALL
            .into_iter()
            .filter(|&g| !self.holds(g))
            .map(Group::name)
            .collect();
        names.join(",")
    }

    /// Takes out the groups named in `names`, as saved by
    /// [`Panels::removed_names`], and puts back the others. Unknown names
    /// are skipped.
    pub(super) fn set_removed_names(&mut self, names: &str) {
        let names: Vec<_> = names.split(',').collect();
        for group in Group::ALL {
            self.removed[group as usize] = names.contains(&group.name());
        }
    }

    /// One view alone, without panels (Shift+Tab).
    pub(super) fn focused(&self) -> bool {
        self.focus.is_some()
    }

    /// Tab: hides the panels or shows them again. Out of the sculpt
    /// alone, it shows the panels and the views there were before.
    pub(super) fn toggle(&mut self, layout: &mut Layout) {
        match self.focus.take() {
            Some(before) => {
                *layout = before.layout;
                self.hidden = false;
            }
            None => self.hidden = !self.hidden,
        }
    }

    /// Shift+Tab: one view alone (`alone`, the sculpt or the reference),
    /// without panels, or back to what there was before, panels and views
    /// as they were.
    pub(super) fn toggle_focus(&mut self, layout: &mut Layout, alone: Layout) {
        match self.focus.take() {
            Some(before) => {
                *layout = before.layout;
                self.hidden = before.hidden;
            }
            None => {
                self.focus = Some(Before {
                    hidden: self.hidden,
                    layout: *layout,
                    alone,
                });
                self.hidden = true;
                *layout = alone;
            }
        }
    }

    /// Other views were chosen (from the menu, or with a scene) while one
    /// view showed alone: that is the end of it, and the views stay as
    /// chosen. The panels stay hidden until Tab.
    pub(super) fn settle(&mut self, layout: Layout) {
        if self.focus.is_some_and(|b| layout != b.alone) {
            self.focus = None;
        }
    }
}

/// Takes the Tab presses out of `events`, unless a text field is being
/// typed in (`typing`), where Tab goes on to the next field. They are the
/// panels' key, not egui's focus moving from widget to widget. Returns
/// each press: true with Shift held.
pub(super) fn take_tab_presses(events: &mut Vec<Event>, typing: bool) -> Vec<bool> {
    let mut presses = Vec::new();
    if typing {
        return presses;
    }
    events.retain(|event| match event {
        Event::Key {
            key: Key::Tab,
            pressed,
            repeat,
            modifiers,
            ..
        } => {
            if *pressed && !*repeat && (modifiers.is_none() || modifiers.shift_only()) {
                presses.push(modifiers.shift);
            }
            false
        }
        _ => true,
    });
    presses
}

impl App {
    /// Tab and Shift+Tab, taken out of this frame's events before egui
    /// sees them.
    pub(super) fn panel_keys(&mut self) {
        let typing = self.ctx.text_edit_focused();
        for shift in take_tab_presses(&mut self.events, typing) {
            if shift {
                let alone = self.view_under_pointer();
                self.panels.toggle_focus(&mut self.study.layout, alone);
            } else {
                self.panels.toggle(&mut self.study.layout);
            }
        }
    }

    /// The view Shift+Tab shows alone: the reference with the pointer
    /// over it, the sculpt otherwise.
    fn view_under_pointer(&self) -> Layout {
        let pointer = self.ctx.input(|i| i.pointer.latest_pos());
        let reference = self.view_rects[REFERENCE];
        if views::shown(reference) && pointer.is_some_and(|p| reference.contains(p)) {
            Layout::Reference
        } else {
            Layout::Sculpt
        }
    }

    /// Tab, from the toolbar and the view menu.
    pub(super) fn toggle_panels(&mut self) {
        self.panels.toggle(&mut self.study.layout);
        self.dirty = true;
    }

    /// The window menu: each panel's groups, ticked while they show. It
    /// stays open, so several can be turned on or off in a row.
    pub(super) fn window_menu(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui.set_min_width(menu::MIN_WIDTH);
        for (k, groups) in [&Group::SCULPT[..], &Group::REFERENCE[..]]
            .into_iter()
            .enumerate()
        {
            if k > 0 {
                ui.separator();
            }
            for &group in groups {
                if menu::check(ui, group.title(tr), "", self.panels.holds(group)).clicked() {
                    self.panels.toggle_group(group);
                    self.dirty = true;
                }
            }
        }
    }

    /// The panel items in the view menu.
    pub(super) fn panel_items(&mut self, ui: &mut egui::Ui) {
        let st = self.lang.tr().study;
        if menu::item(ui, st.hide_panels, "Tab", self.panels.hidden()).clicked() {
            self.toggle_panels();
        }
        let focused = self.panels.focused();
        if menu::item(ui, st.hide_panels_and_reference, "Shift+Tab", focused).clicked() {
            self.panels
                .toggle_focus(&mut self.study.layout, Layout::Sculpt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_hides_the_panels_and_shows_them_again() {
        let mut panels = Panels::default();
        let mut layout = Layout::Both;
        panels.toggle(&mut layout);
        assert!(panels.hidden() && !panels.focused());
        assert_eq!(layout, Layout::Both, "both views stay");
        panels.toggle(&mut layout);
        assert_eq!(panels, Panels::default());
    }

    #[test]
    fn shift_tab_shows_the_sculpt_alone_then_puts_everything_back() {
        for (hidden, layout_before) in [
            (false, Layout::Both),
            (true, Layout::Both),
            (false, Layout::Reference),
            (true, Layout::Sculpt),
        ] {
            let mut panels = Panels {
                hidden,
                ..Panels::default()
            };
            let mut layout = layout_before;
            panels.toggle_focus(&mut layout, Layout::Sculpt);
            assert!(panels.hidden() && panels.focused());
            assert_eq!(layout, Layout::Sculpt);
            panels.toggle_focus(&mut layout, Layout::Sculpt);
            assert_eq!((panels.hidden(), layout), (hidden, layout_before));
            assert!(!panels.focused());
        }
    }

    #[test]
    fn shift_tab_over_the_reference_shows_it_alone() {
        let mut panels = Panels::default();
        let mut layout = Layout::Both;
        panels.toggle_focus(&mut layout, Layout::Reference);
        assert_eq!(layout, Layout::Reference);
        panels.settle(layout);
        assert!(panels.focused(), "still the reference alone");
        panels.toggle_focus(&mut layout, Layout::Sculpt);
        assert_eq!(layout, Layout::Both);
        assert!(!panels.hidden() && !panels.focused());
    }

    #[test]
    fn tab_out_of_the_sculpt_alone_brings_back_panels_and_views() {
        let mut panels = Panels {
            hidden: true,
            ..Panels::default()
        };
        let mut layout = Layout::Both;
        panels.toggle_focus(&mut layout, Layout::Sculpt);
        panels.toggle(&mut layout);
        assert!(!panels.hidden() && !panels.focused());
        assert_eq!(layout, Layout::Both);
    }

    #[test]
    fn choosing_other_views_ends_the_sculpt_alone() {
        let mut panels = Panels::default();
        let mut layout = Layout::Both;
        panels.toggle_focus(&mut layout, Layout::Sculpt);
        panels.settle(layout);
        assert!(panels.focused(), "still the sculpt alone");
        layout = Layout::Reference;
        panels.settle(layout);
        assert!(panels.hidden() && !panels.focused());
        // Shift+Tab now starts again from the views chosen.
        panels.toggle_focus(&mut layout, Layout::Sculpt);
        panels.toggle_focus(&mut layout, Layout::Sculpt);
        assert_eq!(layout, Layout::Reference);
        assert!(panels.hidden());
    }

    #[test]
    fn hidden_panels_give_the_views_the_whole_width() {
        use super::super::study::tests::Harness;
        use super::super::toolbar::ViewOption;
        use super::super::{PANEL_WIDTH, REFERENCE, SCULPT};
        use crate::PointerPhase::{Down, Move, Up};
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let width = |h: &Harness| h.app.ctx.content_rect().width();
        let rects = |h: &Harness| h.app.view_rects;
        assert_eq!(rects(&h)[SCULPT].left(), PANEL_WIDTH);
        assert_eq!(rects(&h)[REFERENCE].right(), width(&h) - PANEL_WIDTH);

        h.key(Key::Tab, false);
        h.frames(1);
        assert!(h.app.panels.hidden());
        assert_eq!(rects(&h)[SCULPT].left(), 0.0);
        assert_eq!(rects(&h)[REFERENCE].right(), width(&h));
        let focus = h.app.ctx.memory(|m| m.focused());
        assert_eq!(focus, None, "Tab moves no keyboard focus");

        // The sculpt alone fills the window; Shift+Tab puts back the
        // hidden panels and both views.
        h.key(Key::Tab, true);
        h.frames(1);
        assert_eq!(rects(&h)[SCULPT].x_range(), 0.0..=width(&h));
        assert!(!super::super::views::shown(rects(&h)[REFERENCE]));
        h.key(Key::Tab, true);
        h.frames(1);
        assert!(h.app.panels.hidden() && !h.app.panels.focused());
        assert_eq!(h.app.study.layout, Layout::Both);

        // The toolbar's button shows the panels and hides them again.
        for hidden in [false, true] {
            let bar = h.app.toolbar_layouts(rects(&h))[SCULPT].clone().unwrap();
            let k = bar.options.iter().position(|&o| o == ViewOption::Panels);
            let at = bar.buttons[k.expect("a panels button")].center();
            h.pointer(Move, at);
            h.pointer(Down, at);
            h.pointer(Up, at);
            h.frames(1);
            assert_eq!(h.app.panels.hidden(), hidden);
        }
    }

    #[test]
    fn removed_groups_round_trip_through_their_names() {
        let mut panels = Panels::default();
        assert!(Group::ALL.iter().all(|&g| panels.holds(g)), "all at first");
        assert_eq!(panels.removed_names(), "");
        panels.toggle_group(Group::Crop);
        panels.toggle_group(Group::ToolSettings);
        assert_eq!(panels.removed_names(), "tools,crop");

        let mut back = Panels::default();
        back.set_removed_names("crop,unknown,tools");
        assert_eq!(back, panels);
        back.set_removed_names("");
        assert_eq!(back, Panels::default(), "an empty list puts all back");
    }

    #[test]
    fn a_panel_without_groups_gives_its_width_to_the_views() {
        use super::super::study::tests::Harness;
        use super::super::{PANEL_WIDTH, REFERENCE, SCULPT};
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        let width = h.app.ctx.content_rect().width();
        for group in Group::SCULPT {
            h.app.panels.toggle_group(group);
        }
        h.frames(1);
        let rects = h.app.view_rects;
        assert_eq!(rects[SCULPT].left(), 0.0);
        assert_eq!(rects[REFERENCE].right(), width - PANEL_WIDTH);

        // One group back is enough to bring the panel back.
        h.app.panels.toggle_group(Group::Remeshing);
        h.frames(1);
        assert_eq!(h.app.view_rects[SCULPT].left(), PANEL_WIDTH);

        // The preferences keep the choice.
        let saved = h.app.preferences();
        assert!(
            saved.ends_with("&hide=tools,subdivision,quad,tweak,calipers"),
            "{saved}"
        );
        h.app.apply_preferences("lang=en");
        assert!(
            !h.app.panels.holds(Group::ToolSettings),
            "no hide key: kept"
        );
        h.app.apply_preferences(
            &saved.replace("hide=tools,subdivision,quad,tweak,calipers", "hide="),
        );
        assert_eq!(h.app.panels.removed_names(), "");
    }

    #[test]
    fn the_window_menu_stays_open_while_groups_are_ticked() {
        use super::super::study::tests::Harness;
        use crate::PointerPhase::{Down, Move, Up};
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        // What can be clicked in layers of `order`, top to bottom, then
        // left to right.
        let clickable = |h: &Harness, order: egui::Order| {
            let mut rects: Vec<egui::Rect> = h.app.ctx.viewport(|v| {
                v.this_pass
                    .widgets
                    .layers()
                    .filter(|(layer, _)| layer.order == order)
                    .flat_map(|(_, widgets)| widgets.iter())
                    .filter(|w| w.sense.senses_click())
                    .map(|w| w.rect)
                    .collect()
            });
            rects.sort_by(|a, b| {
                (a.top(), a.left())
                    .partial_cmp(&(b.top(), b.left()))
                    .unwrap()
            });
            rects
        };
        let click = |h: &mut Harness, at: egui::Pos2| {
            h.pointer(Move, at);
            h.pointer(Down, at);
            h.pointer(Up, at);
            h.frames(1);
        };
        // file, edit, object, view, timelapse, navigation, language,
        // window, help.
        let bar: Vec<_> = clickable(&h, egui::Order::Background)
            .into_iter()
            .filter(|r| r.bottom() < 30.0)
            .collect();
        assert_eq!(bar.len(), 9, "{bar:?}");
        click(&mut h, bar[7].center());
        // The rows, without the menu's own area.
        let rows: Vec<_> = clickable(&h, egui::Order::Foreground)
            .into_iter()
            .filter(|r| r.height() < 40.0)
            .collect();
        assert_eq!(rows.len(), Group::ALL.len(), "{rows:?}");

        click(&mut h, rows[Group::Crop as usize].center());
        assert!(!h.app.panels.holds(Group::Crop));
        assert!(egui::Popup::is_any_open(&h.app.ctx), "still open");
        click(&mut h, rows[0].center());
        assert!(!h.app.panels.holds(Group::ToolSettings));
        assert_eq!(h.app.panels.removed_names(), "tools,crop");

        let away = h.app.view_rects[super::super::SCULPT].center();
        click(&mut h, away);
        assert!(
            !egui::Popup::is_any_open(&h.app.ctx),
            "a click outside closes it"
        );
    }

    fn tab(pressed: bool, modifiers: egui::Modifiers) -> Event {
        Event::Key {
            key: Key::Tab,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn tab_presses_leave_the_events_unless_typing() {
        use egui::Modifiers as M;
        let other = Event::Key {
            key: Key::G,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: M::NONE,
        };
        let events = vec![
            tab(true, M::NONE),
            tab(false, M::NONE),
            other.clone(),
            tab(true, M::SHIFT),
            tab(true, M::CTRL),
        ];
        let mut taken = events.clone();
        assert_eq!(take_tab_presses(&mut taken, false), [false, true]);
        assert_eq!(taken, [other], "egui sees no Tab, so no focus moves");

        let mut typing = events.clone();
        assert!(take_tab_presses(&mut typing, true).is_empty());
        assert_eq!(typing, events, "in a text field Tab goes to the next one");
    }
}
