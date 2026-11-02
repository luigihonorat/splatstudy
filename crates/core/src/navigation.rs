// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! View navigation presets, so people can keep the hands they learned in
//! other sculpting and 3D software. Blender, Maya and ZBrush are trademarks
//! of their respective owners; the names only describe the mappings.

use crate::input::{Button, Modifiers};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NavStyle {
    /// Drag the background to turn: works with only a pen.
    #[default]
    ZBrush,
    Blender,
    Maya,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavAction {
    Orbit,
    Pan,
    Zoom,
}

impl NavStyle {
    pub const ALL: [NavStyle; 3] = [NavStyle::ZBrush, NavStyle::Blender, NavStyle::Maya];

    pub fn name(self) -> &'static str {
        match self {
            NavStyle::ZBrush => "ZBrush",
            NavStyle::Blender => "Blender",
            NavStyle::Maya => "Maya",
        }
    }

    /// What a press of `button` starts. `on_clay` is true when the press
    /// landed on the sculpt, where the left button sculpts unless the
    /// preset uses it for navigation with these modifiers.
    pub fn action(self, button: Button, m: Modifiers, on_clay: bool) -> Option<NavAction> {
        use NavAction::*;
        match (self, button) {
            (NavStyle::ZBrush, Button::Primary) if !on_clay => {
                Some(if m.alt { Pan } else { Orbit })
            }
            (NavStyle::ZBrush, Button::Secondary) => Some(if m.alt {
                Pan
            } else if m.ctrl {
                Zoom
            } else {
                Orbit
            }),
            (NavStyle::Blender, Button::Middle) => Some(if m.shift {
                Pan
            } else if m.ctrl {
                Zoom
            } else {
                Orbit
            }),
            (NavStyle::Maya, Button::Primary) if m.alt => Some(Orbit),
            (NavStyle::Maya, Button::Middle) if m.alt => Some(Pan),
            (NavStyle::Maya, Button::Secondary) if m.alt => Some(Zoom),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers {
        alt: false,
        ctrl: false,
        shift: false,
        command: false,
    };
    const ALT: Modifiers = Modifiers { alt: true, ..NONE };

    #[test]
    fn the_left_button_sculpts_on_the_clay_in_every_preset() {
        for style in NavStyle::ALL {
            assert_eq!(style.action(Button::Primary, NONE, true), None, "{style:?}");
        }
    }

    #[test]
    fn presets_match_their_software() {
        assert_eq!(
            NavStyle::ZBrush.action(Button::Primary, NONE, false),
            Some(NavAction::Orbit)
        );
        assert_eq!(
            NavStyle::ZBrush.action(Button::Primary, ALT, false),
            Some(NavAction::Pan)
        );
        assert_eq!(
            NavStyle::Blender.action(Button::Middle, NONE, true),
            Some(NavAction::Orbit)
        );
        assert_eq!(
            NavStyle::Blender.action(
                Button::Middle,
                Modifiers {
                    shift: true,
                    ..NONE
                },
                true
            ),
            Some(NavAction::Pan)
        );
        assert_eq!(
            NavStyle::Maya.action(Button::Primary, ALT, true),
            Some(NavAction::Orbit)
        );
        assert_eq!(
            NavStyle::Maya.action(Button::Secondary, ALT, false),
            Some(NavAction::Zoom)
        );
        assert_eq!(NavStyle::Maya.action(Button::Middle, NONE, false), None);
    }
}
