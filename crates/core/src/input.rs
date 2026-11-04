// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Platform-neutral input, modelled on the web's Pointer Events. Both shells
//! translate into these types; the core turns them into egui input and keeps
//! the raw pen samples (with pressure) for sculpting.

use std::sync::Arc;

/// Positions are in logical points, origin at the top-left of the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerEvent {
    pub id: u64,
    pub kind: PointerKind,
    pub phase: PointerPhase,
    pub pos: [f32; 2],
    /// The button that changed, for `Down` and `Up`.
    pub button: Option<Button>,
    /// 0..1. Shells report 1.0 for a pressed mouse button.
    pub pressure: f32,
    /// The pen is used with its eraser end.
    pub eraser: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    Mouse,
    Pen,
    Touch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    Down,
    Move,
    Up,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Primary,
    Secondary,
    Middle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    /// Cmd on macOS, Ctrl elsewhere.
    pub command: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    Pointer(PointerEvent),
    /// The pointer left the window.
    PointerLeft,
    /// Scroll, in points (`lines` false) or in lines.
    Wheel {
        delta: [f32; 2],
        lines: bool,
    },
    Key {
        key: egui::Key,
        pressed: bool,
        repeat: bool,
    },
    Text(String),
    /// A key of the numpad, `0`..=`9`, whatever NumLock makes of it. It
    /// is its own input, not a [`Key`](Self::Key): the digit row picks
    /// tools, the numpad turns the views. With NumLock on, the shell also
    /// sends its digit as [`Text`](Self::Text) for the text fields.
    Numpad {
        digit: u8,
        pressed: bool,
        repeat: bool,
    },
    Modifiers(Modifiers),
    Focus(bool),
    /// A file the user dropped, picked or the shell fetched, with its name
    /// (no folders) and contents.
    File {
        purpose: FilePurpose,
        name: String,
        bytes: Arc<[u8]>,
    },
    /// The folder picked after [`Request::PickFolder`](crate::Request) (desktop).
    Folder {
        path: String,
    },
    /// The shell couldn't save or write a file the app asked for: which
    /// file, and why (in the system's words).
    WriteFailed {
        message: String,
    },
}

/// What an opened file is for: the menu item or request that asked for it.
/// Dropped files are `Any`, and the app goes by their extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilePurpose {
    Any,
    Reference,
    Sculpt,
    Scene,
}

impl From<egui::Modifiers> for Modifiers {
    fn from(m: egui::Modifiers) -> Self {
        Modifiers {
            alt: m.alt,
            ctrl: m.ctrl,
            shift: m.shift,
            command: m.command,
        }
    }
}

impl From<Modifiers> for egui::Modifiers {
    fn from(m: Modifiers) -> Self {
        egui::Modifiers {
            alt: m.alt,
            ctrl: m.ctrl,
            shift: m.shift,
            mac_cmd: false,
            command: m.command,
        }
    }
}

impl From<Button> for egui::PointerButton {
    fn from(b: Button) -> Self {
        match b {
            Button::Primary => egui::PointerButton::Primary,
            Button::Secondary => egui::PointerButton::Secondary,
            Button::Middle => egui::PointerButton::Middle,
        }
    }
}

/// Translates our events into egui's and remembers the pen state egui drops.
#[derive(Default)]
pub(crate) struct InputState {
    pub modifiers: Modifiers,
    pub pointer_pos: Option<[f32; 2]>,
    /// The last pointer sample, for the debug HUD and the brush.
    pub last: Option<PointerEvent>,
    /// Every pointer sample since the last frame, in order.
    pub samples: Vec<PointerEvent>,
}

impl InputState {
    pub fn translate(&mut self, event: InputEvent, out: &mut Vec<egui::Event>) {
        let modifiers = self.modifiers.into();
        match event {
            InputEvent::Pointer(p) => {
                let pos = egui::pos2(p.pos[0], p.pos[1]);
                self.pointer_pos = Some(p.pos);
                self.last = Some(p);
                self.samples.push(p);
                match p.phase {
                    PointerPhase::Move => out.push(egui::Event::PointerMoved(pos)),
                    PointerPhase::Down | PointerPhase::Up => {
                        out.push(egui::Event::PointerMoved(pos));
                        if let Some(button) = p.button {
                            out.push(egui::Event::PointerButton {
                                pos,
                                button: button.into(),
                                pressed: p.phase == PointerPhase::Down,
                                modifiers,
                            });
                        }
                    }
                    PointerPhase::Cancel => out.push(egui::Event::PointerGone),
                }
            }
            InputEvent::PointerLeft => {
                self.pointer_pos = None;
                out.push(egui::Event::PointerGone);
            }
            InputEvent::Wheel { delta, lines } => out.push(egui::Event::MouseWheel {
                unit: if lines {
                    egui::MouseWheelUnit::Line
                } else {
                    egui::MouseWheelUnit::Point
                },
                delta: egui::vec2(delta[0], delta[1]),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers,
            }),
            InputEvent::Key {
                key,
                pressed,
                repeat,
            } => out.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat,
                modifiers,
            }),
            InputEvent::Text(text) => out.push(egui::Event::Text(text)),
            InputEvent::Modifiers(m) => {
                self.modifiers = m;
                out.push(egui::Event::ModifiersChanged(m.into()));
            }
            InputEvent::Focus(focused) => out.push(egui::Event::WindowFocused(focused)),
            // The app handles these itself; egui never sees them.
            InputEvent::Numpad { .. }
            | InputEvent::File { .. }
            | InputEvent::Folder { .. }
            | InputEvent::WriteFailed { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pen(phase: PointerPhase, button: Option<Button>) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            id: 1,
            kind: PointerKind::Pen,
            phase,
            pos: [10.0, 20.0],
            button,
            pressure: 0.4,
            eraser: false,
        })
    }

    #[test]
    fn pen_down_becomes_move_then_press_and_keeps_pressure() {
        let mut state = InputState::default();
        let mut out = Vec::new();
        state.translate(pen(PointerPhase::Down, Some(Button::Primary)), &mut out);
        assert!(matches!(out[0], egui::Event::PointerMoved(_)));
        assert!(matches!(
            out[1],
            egui::Event::PointerButton {
                pressed: true,
                button: egui::PointerButton::Primary,
                ..
            }
        ));
        assert_eq!(state.samples.len(), 1);
        assert_eq!(state.last.unwrap().pressure, 0.4);
    }
}
