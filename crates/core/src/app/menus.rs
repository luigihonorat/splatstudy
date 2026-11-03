// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The menu bar. Each part of the editor fills its own items through a hook
//! in its file, so this one keeps only the order of the menus.

use egui::Key;

use super::views::viewpoints;
use super::{App, ISSUES_URL, LICENCES_URL, WEB};
use crate::navigation::NavStyle;
use crate::ui::{self, i18n::Lang, menu};

// Items that do nothing yet are listed disabled, so the menus keep their
// shape while the features land. They get a shortcut when they work.

impl App {
    pub(super) fn bar(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(tr.menu_file, |ui| self.file_menu(ui));
            ui.menu_button(tr.menu_edit, |ui| self.edit_menu(ui));
            ui.menu_button(tr.menu_object, |ui| self.object_menu(ui));
            ui.menu_button(tr.menu_view, |ui| self.view_menu(ui));
            ui.menu_button(tr.menu_timelapse, |ui| self.timelapse_menu(ui));
            ui.menu_button(tr.navigation, |ui| self.navigation_menu(ui));
            ui.menu_button(tr.menu_language, |ui| self.language_menu(ui));
            ui.menu_button(tr.menu_help, |ui| self.help_menu(ui));
        });
    }

    /// A shortcut with the platform's command key, as menus show it.
    pub(super) fn keys(&self, key: &str) -> String {
        format!("{}+{key}", if self.mac { "Cmd" } else { "Ctrl" })
    }

    fn file_menu(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui.set_min_width(menu::MIN_WIDTH);
        self.scene_items(ui);
        ui.separator();
        self.import_items(ui);
        ui.separator();
        menu::submenu(ui, tr.export, |ui| {
            ui.set_min_width(120.0);
            self.mesh_export_items(ui);
        });
        if !WEB {
            ui.separator();
            if menu::action(ui, true, tr.exit, &self.keys("Q")) {
                self.quit = true;
            }
        }
    }

    fn edit_menu(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui.set_min_width(menu::MIN_WIDTH);
        let live = self.playback.is_none();
        if menu::action(ui, live && self.sculpt.can_undo(), tr.undo, &self.keys("Z")) {
            self.sculpt.undo();
        }
        if menu::action(
            ui,
            live && self.sculpt.can_redo(),
            tr.redo,
            &self.keys("Shift+Z"),
        ) {
            self.sculpt.redo();
        }
        ui.separator();
        self.reference_edit_items(ui);
        ui.separator();
        self.preferences_item(ui);
    }

    fn object_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(menu::MIN_WIDTH);
        self.mesh_edit_items(ui);
    }

    fn timelapse_menu(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui.set_min_width(menu::MIN_WIDTH);
        let recorded = !self.sculpt.log().is_empty();
        if menu::action(
            ui,
            recorded && self.playback.is_none(),
            tr.watch_timelapse,
            "",
        ) {
            self.start_playback();
        }
        self.video_export_items(ui);
    }

    fn view_menu(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui.set_min_width(menu::MIN_WIDTH);
        self.link_item(ui);
        menu::submenu(ui, tr.viewpoints, |ui| {
            ui.set_min_width(160.0);
            for (label, key, yaw, pitch) in viewpoints(tr) {
                if menu::action(ui, true, label, key.map_or("", Key::name)) {
                    self.view_from(yaw, pitch);
                }
            }
        });
        if menu::action(ui, true, tr.frame_sculpt, "F") {
            self.frame_sculpt();
        }
        if menu::action(ui, true, tr.reset_view, "Home") {
            self.reset_view();
        }
        ui.separator();
        self.study_toggle_items(ui);
        ui.separator();
        if menu::action(ui, true, tr.switch_theme, "") {
            self.theme = self.theme.toggled();
            ui::theme::apply(ui.ctx(), self.theme);
        }
    }

    fn navigation_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(menu::MIN_WIDTH);
        for style in NavStyle::ALL {
            if menu::item(ui, self.nav_name(style), "", self.nav == style).clicked() {
                self.nav = style;
            }
        }
        ui.separator();
        menu::note(ui, self.nav_hint());
    }

    fn language_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(menu::MIN_WIDTH);
        for lang in Lang::ALL {
            if menu::item(ui, lang.native_name(), "", self.lang == lang).clicked() {
                self.lang = lang;
            }
        }
    }

    fn help_menu(&mut self, ui: &mut egui::Ui) {
        let tr = self.lang.tr();
        ui.set_min_width(menu::MIN_WIDTH);
        if menu::action(ui, true, tr.shortcuts, "") {
            self.show_shortcuts = true;
        }
        self.help_items(ui);
        if menu::action(ui, true, tr.report_issue, "") {
            ui.ctx().open_url(egui::OpenUrl::new_tab(ISSUES_URL));
        }
        if menu::action(ui, true, tr.licences, "") {
            ui.ctx().open_url(egui::OpenUrl::new_tab(LICENCES_URL));
        }
        ui.separator();
        if menu::action(ui, true, tr.about, "") {
            self.show_about = true;
        }
    }
}
