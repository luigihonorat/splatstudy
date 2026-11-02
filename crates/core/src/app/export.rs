// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Exporting the timelapse as a video or images.

use super::App;

/// The export being set up or running.
#[derive(Default)]
pub(super) struct ExportState {}

impl App {
    /// Video items in the file → export menu, after the mesh formats.
    pub(super) fn video_export_items(&mut self, _ui: &mut egui::Ui) {}

    /// Export controls in the timelapse panel.
    pub(super) fn playback_export_items(&mut self, _ui: &mut egui::Ui) {}

    /// The export window, while one is open.
    pub(super) fn export_window(&mut self, _ui: &egui::Ui) {}
}
