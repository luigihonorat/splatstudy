// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Getting started and preferences.

use super::App;
use crate::ui::menu;

/// Getting started and options from the URL.
#[derive(Default)]
pub(super) struct HelpState {}

impl App {
    /// Help items between shortcuts and report issue.
    pub(super) fn help_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        menu::action(ui, false, tr.getting_started, "");
    }

    /// Preferences, at the end of the edit menu.
    pub(super) fn preferences_item(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        menu::action(ui, false, tr.preferences, "");
    }

    /// The getting started and preferences windows, while open.
    pub(super) fn help_windows(&mut self, _ui: &egui::Ui) {}
}
