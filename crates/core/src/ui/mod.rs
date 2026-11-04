// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The interface: theme, fonts, strings and the few widgets the design needs.

pub mod fonts;
pub mod i18n;
pub mod icons;
pub mod menu;
pub mod pie;
pub mod theme;

use egui::{Response, Sense, Stroke, Ui, WidgetText};

/// A text link in the site's nav style: muted, the text colour on hover, and
/// the text colour with a 1px underline when it is the chosen item.
pub fn choice(ui: &mut Ui, text: impl Into<WidgetText>, chosen: bool) -> Response {
    let visuals = ui.visuals().clone();
    let galley = text.into().into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    );
    let size = galley.size() + egui::vec2(0.0, 4.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let color = if chosen || response.hovered() {
        visuals.widgets.active.fg_stroke.color
    } else {
        visuals.widgets.inactive.fg_stroke.color
    };
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.galley(rect.min, galley, color);
        if chosen {
            let y = rect.bottom() - 0.5;
            painter.hline(rect.x_range(), y, Stroke::new(1.0, color));
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small muted heading over a group of controls.
pub fn section(ui: &mut Ui, text: &str) {
    let color = ui.visuals().widgets.inactive.fg_stroke.color;
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(text)
            .font(fonts::regular(11.0))
            .color(color),
    );
    ui.add_space(2.0);
}

/// A slider in the site's style: label and value on one line, a 1px track
/// underneath, filled in the text colour up to a small round handle.
pub fn slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    show: impl Fn(f32) -> String,
) -> Response {
    slider_with(ui, label, value, range, None, show)
}

/// A [`slider`] whose value can also be typed: a click on the number
/// edits it (Enter or a click elsewhere keeps it, Escape leaves it as it
/// was). Dragging goes in steps of `step`; a typed number is kept as it
/// is, within the range.
pub fn typed_slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    step: f32,
    show: impl Fn(f32) -> String,
) -> Response {
    slider_with(ui, label, value, range, Some(step), show)
}

/// The number in `text`, ignoring anything but digits, the point and a
/// leading minus (so "10,000" and "10 000" read as 10000).
fn typed_number(text: &str) -> Option<f32> {
    let kept: String = text
        .trim()
        .chars()
        .enumerate()
        .filter(|&(i, c)| c.is_ascii_digit() || c == '.' || (c == '-' && i == 0))
        .map(|(_, c)| c)
        .collect();
    kept.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn slider_with(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    step: Option<f32>,
    show: impl Fn(f32) -> String,
) -> Response {
    let (lo, hi) = (*range.start(), *range.end());
    let width = ui.available_width();
    let (rect, mut response) =
        ui.allocate_exact_size(egui::vec2(width, 34.0), Sense::click_and_drag());
    let font = fonts::regular(13.0);
    let shown = show(*value);
    // Where the number is, when it can be typed.
    let number = step.map(|_| {
        let galley =
            ui.painter()
                .layout_no_wrap(shown.clone(), font.clone(), egui::Color32::PLACEHOLDER);
        egui::Align2::RIGHT_TOP
            .anchor_size(rect.right_top(), galley.size())
            .expand(3.0)
    });
    let typing_id = response.id.with("typing");
    let entry_id = response.id.with("entry");
    let fresh_id = response.id.with("fresh");
    let mut typing: Option<String> = ui.data(|d| d.get_temp(typing_id));
    // The field opens the frame after the click: in the click's own frame
    // it would count as clicked outside, and lose the focus at once.
    let fresh: bool = ui.data(|d| d.get_temp(fresh_id)).unwrap_or(false);
    let on_number =
        |pos: Option<egui::Pos2>| number.is_some_and(|r| pos.is_some_and(|p| r.contains(p)));
    // Where the press began while the button is down; where it was let go
    // for the click itself.
    let pressed_at = ui.input(|i| i.pointer.press_origin());
    let pressed_on_number = on_number(pressed_at.or(response.interact_pointer_pos()));
    let mut started = false;
    if typing.is_none() && pressed_on_number {
        if response.clicked() {
            let text: String = shown
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                .collect();
            let mut state =
                egui::text_edit::TextEditState::load(ui.ctx(), entry_id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text_selection::CCursorRange::select_all_str(
                    &text,
                )));
            state.store(ui.ctx(), entry_id);
            ui.ctx().request_repaint();
            typing = Some(text);
            started = true;
        }
    } else if typing.is_none()
        && (response.dragged() || response.clicked())
        && let Some(pos) = response.interact_pointer_pos()
    {
        let t = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let mut new = lo + t * (hi - lo);
        if let Some(step) = step.filter(|&s| s > 0.0) {
            new = ((new / step).round() * step).clamp(lo, hi);
        }
        if new != *value {
            *value = new;
            response.mark_changed();
        }
    }
    let visuals = ui.visuals();
    let text = visuals.widgets.active.fg_stroke.color;
    let muted = visuals.widgets.inactive.fg_stroke.color;
    let line = visuals.widgets.noninteractive.bg_stroke.color;
    let over_number = typing.is_none() && response.hovered() && on_number(response.hover_pos());
    let active = response.hovered() || response.dragged() || typing.is_some();
    let painter = ui.painter().clone();
    painter.text(
        rect.left_top(),
        egui::Align2::LEFT_TOP,
        label,
        font.clone(),
        if active { text } else { muted },
    );
    let field_open = typing.is_some() && !started;
    if let Some(entry) = typing.as_mut().filter(|_| field_open) {
        if fresh {
            ui.memory_mut(|m| m.request_focus(entry_id));
        }
        let field = egui::Rect::from_min_max(
            egui::pos2(rect.right() - 96.0, rect.top()),
            egui::pos2(rect.right(), rect.top() + 18.0),
        );
        // In a child of its own, over the slider: the panel's layout goes
        // on below the slider as before.
        let mut over = ui.new_child(egui::UiBuilder::new().max_rect(field));
        let edit = over.add(
            egui::TextEdit::singleline(entry)
                .id(entry_id)
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::ZERO)
                .font(font.clone())
                .text_color(text)
                .align(egui::Align2::RIGHT_TOP)
                .desired_width(field.width())
                .char_limit(12),
        );
        // Chosen: the text colour and a 1px underline.
        painter.hline(field.x_range(), field.bottom(), Stroke::new(1.0, text));
        if !fresh && !edit.has_focus() {
            let keep = !ui.input(|i| i.key_pressed(egui::Key::Escape));
            if keep && let Some(typed) = typed_number(entry) {
                let new = typed.clamp(lo, hi);
                if new != *value {
                    *value = new;
                    response.mark_changed();
                }
            }
            typing = None;
        }
    } else {
        painter.text(
            rect.right_top(),
            egui::Align2::RIGHT_TOP,
            &shown,
            font,
            if over_number || started { text } else { muted },
        );
    }
    ui.data_mut(|d| d.insert_temp(fresh_id, started));
    ui.data_mut(|d| match &typing {
        Some(t) => {
            d.insert_temp(typing_id, t.clone());
        }
        None => {
            d.remove::<String>(typing_id);
        }
    });
    let y = rect.bottom() - 6.0;
    let x = rect.left() + ((*value - lo) / (hi - lo)).clamp(0.0, 1.0) * rect.width();
    painter.hline(rect.x_range(), y, Stroke::new(1.0, line));
    painter.hline(rect.left()..=x, y, Stroke::new(1.0, text));
    painter.circle_filled(egui::pos2(x, y), if active { 4.5 } else { 3.5 }, text);
    let cursor = if over_number {
        egui::CursorIcon::Text
    } else {
        egui::CursorIcon::ResizeHorizontal
    };
    response.on_hover_cursor(cursor)
}

#[cfg(test)]
mod tests {
    use super::typed_number;

    #[test]
    fn typed_numbers_read_with_or_without_separators() {
        assert_eq!(typed_number("12345"), Some(12345.0));
        assert_eq!(typed_number(" 10,000 "), Some(10000.0));
        assert_eq!(typed_number("10 000"), Some(10000.0));
        assert_eq!(typed_number("2.5"), Some(2.5));
        assert_eq!(typed_number("-3"), Some(-3.0));
        assert_eq!(typed_number(""), None);
        assert_eq!(typed_number("abc"), None);
    }
}
