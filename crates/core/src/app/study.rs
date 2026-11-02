// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Study aids for comparing the sculpt with the reference: camera link,
//! silhouette, ghost, mirror view, plumb line, grid and the pose timer.

use super::App;
use crate::ui::menu;

/// Which aids are on.
#[derive(Default)]
pub(super) struct StudyState {}

impl App {
    /// The camera link, at the top of the view menu.
    pub(super) fn link_item(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        menu::action(ui, false, tr.link_cameras, "");
    }

    /// The comparison aids in the view menu.
    pub(super) fn study_toggle_items(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        for label in [
            tr.silhouette,
            tr.ghost,
            tr.mirror_view,
            tr.plumb_line,
            tr.grid,
        ] {
            menu::action(ui, false, label, "");
        }
    }

    pub(super) fn study_shortcuts(&mut self, _ui: &egui::Ui) {}
}
