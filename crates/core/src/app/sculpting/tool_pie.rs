// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The tools in a pie menu: hold Shift + Space over the sculpt view,
//! point at a tool and let go of Space (or click it).

use egui::Key;

use super::{BRUSHES, brush_icon, brush_name};
use crate::app::{App, SCULPT};
use crate::ui::pie::{Outcome, Pie};

/// The key held, with Shift, for the tool pie; alone it holds the size
/// and strength at the pointer (see `quick_settings`).
pub(in crate::app) const TOOL_PIE_KEY: Key = Key::Space;

impl App {
    /// Opens the tool pie on Shift + its key over the sculpt view, and
    /// picks or closes it on the key's release or Escape. Before the other
    /// shortcuts, so Escape closes the pie and nothing else. The key alone
    /// opens the size and strength instead (see `quick_settings`).
    pub(super) fn tool_pie_keys(&mut self, ui: &egui::Ui) {
        if self.quick_settings_keys(ui) {
            return;
        }
        match self.tool.pie {
            Some(pie) => self.tool_pie_outcome(pie.keys(ui)),
            // Not in the middle of a stroke or a turn of the view.
            None if self.drags[SCULPT].is_none() && self.playback.is_none() => {
                let shift = egui::Modifiers::SHIFT;
                self.tool.pie = Pie::open(ui, TOOL_PIE_KEY, shift, self.view_rects[SCULPT]);
                // The pie takes the place of an open flyout.
                if self.tool.pie.is_some() {
                    self.flyout = None;
                }
            }
            None => {}
        }
    }

    /// Draws the open tool pie over everything and takes clicks on it.
    pub(in crate::app) fn tool_pie_ui(&mut self, ui: &egui::Ui) {
        if self.playback.is_some() {
            self.tool.pie = None;
        }
        let tr = self.lang.tr();
        let labels = BRUSHES.map(|(brush, _, _)| brush_name(tr, brush));
        let icons = BRUSHES.map(|(brush, _, _)| brush_icon(brush));
        let icons = icons.each_ref().map(|icon| icon as crate::ui::icons::Icon);
        let chosen = BRUSHES.iter().position(|&(b, _, _)| b == self.tool.tool);
        let Some(pie) = &mut self.tool.pie else {
            return;
        };
        let outcome = pie.show(ui, &labels, &icons, chosen);
        self.tool_pie_outcome(outcome);
    }

    fn tool_pie_outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Open => {}
            Outcome::Picked(index) => {
                self.choose_brush(BRUSHES[index].0);
                self.tool.pie = None;
            }
            Outcome::Cancelled => self.tool.pie = None,
        }
    }

    /// Whether the tool pie is open.
    #[cfg(test)]
    pub(in crate::app) fn pie_open(&self) -> bool {
        self.tool.pie.is_some()
    }

    /// Whether the tool pie or the size and strength at the pointer are
    /// open: no stroke or view drag may start.
    pub(in crate::app) fn popup_open(&self) -> bool {
        self.tool.pie.is_some() || self.tool.quick.is_some()
    }
}
