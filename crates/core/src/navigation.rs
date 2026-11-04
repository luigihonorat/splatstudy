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

    /// What a drag that `button` started as `action` does now that the
    /// modifiers are `m`. In ZBrush style, Alt + drag on the background
    /// moves, and letting go of Alt while still dragging scales instead.
    pub fn during(self, action: NavAction, button: Button, m: Modifiers) -> NavAction {
        match (self, button, action) {
            (NavStyle::ZBrush, Button::Primary, NavAction::Pan) if !m.alt => NavAction::Zoom,
            _ => action,
        }
    }

    /// How far a zoom drag that moved the pointer by `dx`, `dy` points on
    /// the screen goes, in points: positive moves the view away, negative
    /// closer. Up is always closer. In ZBrush and Maya style a sideways
    /// drag zooms too, right closer and left away (Maya dollies in to the
    /// right); each step goes by whichever way the pointer moved more, so
    /// a slanted drag doesn't cancel itself out. Blender style zooms with
    /// up and down only.
    pub fn zoom(self, dx: f32, dy: f32) -> f32 {
        match self {
            NavStyle::ZBrush | NavStyle::Maya if dx.abs() > dy.abs() => -dx,
            _ => dy,
        }
    }

    /// Whether a turn in progress snaps to the nearest front, side, back,
    /// top or bottom view: Shift in ZBrush style.
    pub fn snaps(self, m: Modifiers) -> bool {
        self == NavStyle::ZBrush && m.shift
    }

    /// The key held for a brush's opposite action: Alt in ZBrush style,
    /// where Ctrl is masking, and Ctrl in the others, where Alt navigates.
    pub fn invert_key(self) -> &'static str {
        match self {
            NavStyle::ZBrush => "Alt",
            NavStyle::Blender | NavStyle::Maya => "Ctrl",
        }
    }

    /// Whether the brush does its opposite action with the modifiers `m`.
    pub fn inverts(self, m: Modifiers) -> bool {
        match self {
            NavStyle::ZBrush => m.alt,
            NavStyle::Blender | NavStyle::Maya => m.ctrl,
        }
    }
}

/// The axis-aligned view nearest to `yaw` and `pitch` (radians): both
/// rounded to quarter turns.
pub fn snap_view(yaw: f32, pitch: f32) -> (f32, f32) {
    use std::f32::consts::FRAC_PI_2;
    let quarter = |a: f32| (a / FRAC_PI_2).round() * FRAC_PI_2;
    (quarter(yaw), quarter(pitch))
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

    #[test]
    fn zbrush_style_matches_its_documented_navigation() {
        let z = NavStyle::ZBrush;
        let ctrl = Modifiers { ctrl: true, ..NONE };
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        // On the background: drag turns, Alt + drag moves.
        assert_eq!(
            z.action(Button::Primary, NONE, false),
            Some(NavAction::Orbit)
        );
        assert_eq!(z.action(Button::Primary, ALT, false), Some(NavAction::Pan));
        // Shift doesn't change what a drag does; it snaps the turn.
        assert_eq!(
            z.action(Button::Primary, shift, false),
            Some(NavAction::Orbit)
        );
        // On the clay, the left button sculpts with any modifier: Alt is
        // the opposite action, not navigation.
        for m in [NONE, ALT, ctrl, shift] {
            assert_eq!(z.action(Button::Primary, m, true), None, "{m:?}");
        }
        // The right button anywhere.
        for on_clay in [false, true] {
            assert_eq!(
                z.action(Button::Secondary, NONE, on_clay),
                Some(NavAction::Orbit)
            );
            assert_eq!(
                z.action(Button::Secondary, ALT, on_clay),
                Some(NavAction::Pan)
            );
            assert_eq!(
                z.action(Button::Secondary, ctrl, on_clay),
                Some(NavAction::Zoom)
            );
        }
    }

    #[test]
    fn letting_go_of_alt_mid_drag_scales_in_zbrush_style_only() {
        let z = NavStyle::ZBrush;
        let pan = z.action(Button::Primary, ALT, false).unwrap();
        // Alt still held: it keeps moving.
        assert_eq!(z.during(pan, Button::Primary, ALT), NavAction::Pan);
        // Alt let go: it scales, and keeps scaling if Alt comes back.
        let zoom = z.during(pan, Button::Primary, NONE);
        assert_eq!(zoom, NavAction::Zoom);
        assert_eq!(z.during(zoom, Button::Primary, ALT), NavAction::Zoom);
        // A turn stays a turn, and the right button keeps its action.
        assert_eq!(
            z.during(NavAction::Orbit, Button::Primary, ALT),
            NavAction::Orbit
        );
        assert_eq!(
            z.during(NavAction::Pan, Button::Secondary, NONE),
            NavAction::Pan
        );
        // Maya style turns with Alt + left, and letting go keeps turning.
        let maya = NavStyle::Maya.action(Button::Primary, ALT, true).unwrap();
        assert_eq!(
            NavStyle::Maya.during(maya, Button::Primary, NONE),
            NavAction::Orbit
        );
        for style in [NavStyle::Blender, NavStyle::Maya] {
            for action in [NavAction::Orbit, NavAction::Pan, NavAction::Zoom] {
                for button in [Button::Primary, Button::Middle, Button::Secondary] {
                    assert_eq!(style.during(action, button, NONE), action, "{style:?}");
                }
            }
        }
    }

    #[test]
    fn the_opposite_action_key_follows_the_style() {
        let ctrl = Modifiers { ctrl: true, ..NONE };
        assert_eq!(NavStyle::ZBrush.invert_key(), "Alt");
        assert!(NavStyle::ZBrush.inverts(ALT));
        assert!(!NavStyle::ZBrush.inverts(ctrl), "Ctrl is masking in ZBrush");
        for style in [NavStyle::Blender, NavStyle::Maya] {
            assert_eq!(style.invert_key(), "Ctrl");
            assert!(style.inverts(ctrl), "{style:?}");
            assert!(!style.inverts(ALT), "{style:?}");
        }
        // Maya style: Alt + left on the clay navigates, so never sculpts
        // inverted.
        assert_eq!(
            NavStyle::Maya.action(Button::Primary, ALT, true),
            Some(NavAction::Orbit)
        );
        assert!(!NavStyle::Maya.inverts(NONE));
    }

    #[test]
    fn zbrush_and_maya_zoom_sideways_too() {
        for style in [NavStyle::ZBrush, NavStyle::Maya] {
            // Up closer, down away, as before.
            assert_eq!(style.zoom(0.0, -5.0), -5.0, "{style:?}");
            assert_eq!(style.zoom(0.0, 5.0), 5.0, "{style:?}");
            // Right closer, left away.
            assert_eq!(style.zoom(4.0, 0.0), -4.0, "{style:?}");
            assert_eq!(style.zoom(-4.0, 0.0), 4.0, "{style:?}");
            // A slanted step goes by the larger movement, whole.
            assert_eq!(style.zoom(6.0, 2.0), -6.0, "{style:?}");
            assert_eq!(style.zoom(-1.0, -3.0), -3.0, "{style:?}");
            assert_eq!(style.zoom(1.0, 3.0), 3.0, "{style:?}");
        }
        // Up and right both come closer: a drag up and to the right zooms
        // in all the way, whichever way each step leans.
        let z = NavStyle::ZBrush;
        assert!(z.zoom(3.0, -2.0) < 0.0 && z.zoom(2.0, -3.0) < 0.0);
        // Blender style keeps to up and down.
        assert_eq!(NavStyle::Blender.zoom(6.0, 2.0), 2.0);
        assert_eq!(NavStyle::Blender.zoom(6.0, 0.0), 0.0);
    }

    #[test]
    fn shift_snaps_a_turn_in_zbrush_style() {
        use std::f32::consts::{FRAC_PI_2, PI};
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        assert!(NavStyle::ZBrush.snaps(shift));
        assert!(!NavStyle::ZBrush.snaps(NONE));
        assert!(!NavStyle::Blender.snaps(shift), "Shift + middle pans there");
        assert!(!NavStyle::Maya.snaps(shift));

        let close =
            |(a, b): (f32, f32), (c, d): (f32, f32)| (a - c).abs() < 1e-5 && (b - d).abs() < 1e-5;
        assert!(close(snap_view(0.3, 0.12), (0.0, 0.0)), "front");
        assert!(close(snap_view(1.2, -0.2), (FRAC_PI_2, 0.0)), "right side");
        assert!(close(snap_view(-1.0, 0.1), (-FRAC_PI_2, 0.0)), "left side");
        assert!(close(snap_view(3.0, 0.0), (PI, 0.0)), "back");
        assert!(close(snap_view(0.1, 1.4), (0.0, FRAC_PI_2)), "top");
        assert!(close(snap_view(0.1, -1.0), (0.0, -FRAC_PI_2)), "bottom");
    }
}
