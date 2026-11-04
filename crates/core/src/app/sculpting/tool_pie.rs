// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The tools in a pie menu: hold Space over the sculpt view, point at a
//! tool and let go (or click it).

use egui::Key;

use super::{BRUSHES, brush_name};
use crate::app::{App, SCULPT};
use crate::ui::pie::{Outcome, Pie};

/// The key held for the tool pie.
pub(in crate::app) const TOOL_PIE_KEY: Key = Key::Space;

impl App {
    /// Opens the tool pie on its key over the sculpt view, and picks or
    /// closes it on the key's release or Escape. Before the other
    /// shortcuts, so Escape closes the pie and nothing else.
    pub(super) fn tool_pie_keys(&mut self, ui: &egui::Ui) {
        match self.tool.pie {
            Some(pie) => self.tool_pie_outcome(pie.keys(ui, BRUSHES.len())),
            // Not in the middle of a stroke or a turn of the view.
            None if self.drags[SCULPT].is_none() && self.playback.is_none() => {
                self.tool.pie = Pie::open(ui, TOOL_PIE_KEY, self.view_rects[SCULPT]);
            }
            None => {}
        }
    }

    /// Draws the open tool pie over everything and takes clicks on it.
    pub(in crate::app) fn tool_pie_ui(&mut self, ui: &egui::Ui) {
        let Some(pie) = self.tool.pie else {
            return;
        };
        if self.playback.is_some() {
            self.tool.pie = None;
            return;
        }
        let tr = self.lang.tr();
        let labels = BRUSHES.map(|(brush, _, _)| brush_name(tr, brush));
        let chosen = BRUSHES.iter().position(|&(b, _, _)| b == self.tool.tool);
        let outcome = pie.show(ui, &labels, chosen);
        self.tool_pie_outcome(outcome);
    }

    fn tool_pie_outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Open => {}
            Outcome::Picked(index) => {
                self.tool.tool = BRUSHES[index].0;
                self.tool.pie = None;
            }
            Outcome::Cancelled => self.tool.pie = None,
        }
    }

    /// Whether a pie menu is open: no stroke or view drag may start.
    pub(in crate::app) fn pie_open(&self) -> bool {
        self.tool.pie.is_some()
    }
}
