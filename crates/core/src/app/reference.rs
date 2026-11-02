// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The reference: what is loaded in the right view, its panel and drawing.

use super::App;
use crate::ui::{self, menu};

/// The reference and how it is shown.
#[derive(Default)]
pub(super) struct ReferenceState {}

impl App {
    pub(super) fn reference_panel(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui::section(ui, tr.reference_tools);
        // Loading references comes next; the control shows where it will be.
        ui.add_enabled_ui(false, |ui| ui::choice(ui, tr.open_reference, false));
    }

    /// Reference items in the edit menu.
    pub(super) fn reference_edit_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        menu::action(ui, false, tr.remove_reference, "");
    }

    /// Draws the reference into a pass already cleared to the view's
    /// background. Nothing is loaded yet.
    pub(super) fn draw_reference(&self, _pass: &mut wgpu::RenderPass<'_>) {}
}
