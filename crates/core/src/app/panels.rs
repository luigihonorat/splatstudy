// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Hiding the side panels so the views take the whole width: Tab hides
//! the panels, Shift+Tab the panels and the reference view too, so the
//! sculpt view fills the window. The menu bar always stays.
//!
//! The hidden state lasts for the session; scenes don't keep it.

use egui::{Event, Key};

use super::App;
use super::study::Layout;
use crate::ui::menu;

/// Whether the panels show, and what to put back after Shift+Tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Panels {
    hidden: bool,
    /// Shift+Tab's view of the sculpt alone, and what there was before.
    focus: Option<Before>,
}

/// What Shift+Tab hid, to show again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Before {
    hidden: bool,
    layout: Layout,
}

impl Panels {
    pub(super) fn hidden(&self) -> bool {
        self.hidden
    }

    /// The sculpt view alone, without panels (Shift+Tab).
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

    /// Shift+Tab: the sculpt view alone, without panels, or back to what
    /// there was before, panels and views as they were.
    pub(super) fn toggle_focus(&mut self, layout: &mut Layout) {
        match self.focus.take() {
            Some(before) => {
                *layout = before.layout;
                self.hidden = before.hidden;
            }
            None => {
                self.focus = Some(Before {
                    hidden: self.hidden,
                    layout: *layout,
                });
                self.hidden = true;
                *layout = Layout::Sculpt;
            }
        }
    }

    /// Other views were chosen (from the menu, or with a scene) while the
    /// sculpt showed alone: that is the end of it, and the views stay as
    /// chosen. The panels stay hidden until Tab.
    pub(super) fn settle(&mut self, layout: Layout) {
        if layout != Layout::Sculpt {
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
                self.panels.toggle_focus(&mut self.study.layout);
            } else {
                self.panels.toggle(&mut self.study.layout);
            }
        }
    }

    /// Tab, from the toolbar and the view menu.
    pub(super) fn toggle_panels(&mut self) {
        self.panels.toggle(&mut self.study.layout);
        self.dirty = true;
    }

    /// The panel items in the view menu.
    pub(super) fn panel_items(&mut self, ui: &mut egui::Ui) {
        let st = self.lang.tr().study;
        if menu::item(ui, st.hide_panels, "Tab", self.panels.hidden()).clicked() {
            self.toggle_panels();
        }
        let focused = self.panels.focused();
        if menu::item(ui, st.hide_panels_and_reference, "Shift+Tab", focused).clicked() {
            self.panels.toggle_focus(&mut self.study.layout);
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
                focus: None,
            };
            let mut layout = layout_before;
            panels.toggle_focus(&mut layout);
            assert!(panels.hidden() && panels.focused());
            assert_eq!(layout, Layout::Sculpt);
            panels.toggle_focus(&mut layout);
            assert_eq!((panels.hidden(), layout), (hidden, layout_before));
            assert!(!panels.focused());
        }
    }

    #[test]
    fn tab_out_of_the_sculpt_alone_brings_back_panels_and_views() {
        let mut panels = Panels {
            hidden: true,
            focus: None,
        };
        let mut layout = Layout::Both;
        panels.toggle_focus(&mut layout);
        panels.toggle(&mut layout);
        assert!(!panels.hidden() && !panels.focused());
        assert_eq!(layout, Layout::Both);
    }

    #[test]
    fn choosing_other_views_ends_the_sculpt_alone() {
        let mut panels = Panels::default();
        let mut layout = Layout::Both;
        panels.toggle_focus(&mut layout);
        panels.settle(layout);
        assert!(panels.focused(), "still the sculpt alone");
        layout = Layout::Reference;
        panels.settle(layout);
        assert!(panels.hidden() && !panels.focused());
        // Shift+Tab now starts again from the views chosen.
        panels.toggle_focus(&mut layout);
        panels.toggle_focus(&mut layout);
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
