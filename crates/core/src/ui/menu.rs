// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Menu items in the site's nav style: muted text, the text colour on
//! hover, a 1px underline under the chosen item, the shortcut on the right.
//! Items that do nothing yet are drawn faded inside `add_enabled_ui`.

use egui::containers::menu::{MenuState, SubMenu, SubMenuButton};
use egui::{Rect, Response, Sense, Stroke, TextStyle, Ui, pos2, vec2};

use super::fonts;

/// Width every menu opens at, so short menus don't look cramped.
pub const MIN_WIDTH: f32 = 200.0;
/// Space between an item's label and its shortcut.
const GAP: f32 = 32.0;

/// What sits at the right end of a row.
enum Right<'a> {
    Keys(&'a str),
    Arrow,
}

pub fn item(ui: &mut Ui, label: &str, shortcut: &str, chosen: bool) -> Response {
    row(ui, label, Right::Keys(shortcut), chosen, false)
}

/// An item that is drawn faded and ignores clicks when `enabled` is false.
/// Returns true when it was clicked.
pub fn action(ui: &mut Ui, enabled: bool, label: &str, shortcut: &str) -> bool {
    ui.add_enabled_ui(enabled, |ui| item(ui, label, shortcut, false))
        .inner
        .clicked()
}

/// An item that opens `content` to its side when hovered.
pub fn submenu(ui: &mut Ui, label: &str, content: impl FnOnce(&mut Ui)) {
    let id = SubMenu::id_from_widget_id(ui.next_auto_id());
    let open = MenuState::from_ui(ui, |state, _| state.open_item == Some(id));
    let response = row(ui, label, Right::Arrow, false, open);
    SubMenu::new().show(ui, &response, content);
}

/// A small muted note under a group of items.
pub fn note(ui: &mut Ui, text: &str) {
    let color = ui.visuals().widgets.inactive.fg_stroke.color;
    ui.label(
        egui::RichText::new(text)
            .font(fonts::regular(11.0))
            .color(color),
    );
}

fn row(ui: &mut Ui, label: &str, right: Right, chosen: bool, open: bool) -> Response {
    let font = TextStyle::Body.resolve(ui.style());
    let placeholder = egui::Color32::PLACEHOLDER;
    let label = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), placeholder);
    let keys = match right {
        Right::Keys(keys) if !keys.is_empty() => Some(ui.painter().layout_no_wrap(
            keys.to_owned(),
            font,
            placeholder,
        )),
        _ => None,
    };
    let arrow = ui.spacing().icon_width;
    let right_width = match (&right, &keys) {
        (Right::Arrow, _) => GAP + arrow,
        (_, Some(keys)) => GAP + keys.size().x,
        _ => 0.0,
    };
    let desired = label.size().x + right_width;
    let width = if ui.layout().horizontal_justify() {
        ui.available_width().max(desired)
    } else {
        desired
    };
    let height = ui.spacing().interact_size.y;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let text = visuals.widgets.active.fg_stroke.color;
        let muted = visuals.widgets.inactive.fg_stroke.color;
        let color = if chosen || open || response.hovered() {
            text
        } else {
            muted
        };
        let painter = ui.painter();
        let y = rect.center().y;
        let label_min = pos2(rect.left(), y - label.size().y / 2.0);
        let label_rect = Rect::from_min_size(label_min, label.size());
        painter.galley(label_min, label, color);
        if let Some(keys) = keys {
            let at = pos2(rect.right() - keys.size().x, y - keys.size().y / 2.0);
            painter.galley(at, keys, muted);
        }
        if let Right::Arrow = right {
            let at =
                Rect::from_center_size(pos2(rect.right() - arrow / 2.0, y), vec2(arrow, arrow));
            painter.add(SubMenuButton::arrow_shape(at, color));
        }
        if chosen {
            let y = label_rect.bottom() + 1.5;
            painter.hline(label_rect.x_range(), y, Stroke::new(1.0, color));
        }
    }
    response
}
