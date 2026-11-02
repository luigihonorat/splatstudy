// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! SplatStudy's core: everything except the window and the input source.
//!
//! A platform shell (SDL3 on desktop, the browser on the web) creates the wgpu
//! device and surface, feeds [`InputEvent`]s into an [`App`] and calls
//! [`App::frame`] when [`FrameOutput::repaint`] asks for it. The core never
//! touches the platform directly; time arrives as an argument.

mod app;
pub mod camera;
pub mod input;
pub mod navigation;
pub mod reference;
pub mod render;
pub mod sculpt;
pub mod ui;

pub use app::{App, FrameOutput, Request};
pub use egui::{CursorIcon, Key};
pub use input::{
    Button, FilePurpose, InputEvent, Modifiers, PointerEvent, PointerKind, PointerPhase,
};
pub use ui::i18n::Lang;
