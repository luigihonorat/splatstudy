// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Menu items in the site's nav style: muted text, the text colour on
//! hover, a 1px underline under the chosen item, the shortcut on the right.
//! Items that do nothing yet are drawn faded inside `add_enabled_ui`.
//! Items that turn something on and off have a tick in front instead.

use egui::containers::menu::{MenuButton, MenuConfig, MenuState, SubMenu, SubMenuButton};
use egui::{PopupCloseBehavior, Rect, Response, Sense, Stroke, TextStyle, Ui, pos2, vec2};

use super::fonts;

/// Width every menu opens at, so short menus don't look cramped.
pub const MIN_WIDTH: f32 = 200.0;
/// Space between an item's label and its shortcut.
const GAP: f32 = 32.0;
/// Room in front of a [`check`] item's label for its tick.
const TICK_WIDTH: f32 = 18.0;

/// What sits at the right end of a row.
enum Right<'a> {
    Keys(&'a str),
    Arrow,
}

pub fn item(ui: &mut Ui, label: &str, shortcut: &str, chosen: bool) -> Response {
    row(ui, label, Right::Keys(shortcut), chosen, false, None)
}

/// An item with a tick in front while `on`, for something that is turned
/// on and off. The tick's room stays when it is off, so labels line up.
pub fn check(ui: &mut Ui, label: &str, shortcut: &str, on: bool) -> Response {
    row(ui, label, Right::Keys(shortcut), false, false, Some(on))
}

/// A menu in the menu bar that stays open when its items are clicked, so
/// several can be turned on or off in a row; a click outside closes it.
pub fn sticky_menu(ui: &mut Ui, title: &str, content: impl FnOnce(&mut Ui)) {
    let mut config = MenuConfig::find(ui);
    config.close_behavior = PopupCloseBehavior::CloseOnClickOutside;
    MenuButton::new(title).config(config).ui(ui, content);
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
    let response = row(ui, label, Right::Arrow, false, open, None);
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

fn row(
    ui: &mut Ui,
    label: &str,
    right: Right,
    chosen: bool,
    open: bool,
    tick: Option<bool>,
) -> Response {
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
    let left_width = if tick.is_some() { TICK_WIDTH } else { 0.0 };
    let desired = left_width + label.size().x + right_width;
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
        let on = tick == Some(true);
        let color = if chosen || on || open || response.hovered() {
            text
        } else {
            muted
        };
        let painter = ui.painter();
        let y = rect.center().y;
        if on {
            // A 1px tick, on half pixels so it stays sharp like the
            // panels' chevrons.
            let at = pos2(rect.left().round() + 1.5, y.round() + 0.5);
            let points = vec![
                at + vec2(0.0, 0.0),
                at + vec2(3.0, 3.0),
                at + vec2(9.0, -4.0),
            ];
            painter.add(egui::Shape::line(points, Stroke::new(1.0, color)));
        }
        let label_min = pos2(rect.left() + left_width, y - label.size().y / 2.0);
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
