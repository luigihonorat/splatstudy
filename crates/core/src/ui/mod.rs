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
    let (lo, hi) = (*range.start(), *range.end());
    let width = ui.available_width();
    let (rect, mut response) =
        ui.allocate_exact_size(egui::vec2(width, 34.0), Sense::click_and_drag());
    if (response.dragged() || response.clicked())
        && let Some(pos) = response.interact_pointer_pos()
    {
        let t = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let new = lo + t * (hi - lo);
        if new != *value {
            *value = new;
            response.mark_changed();
        }
    }
    let visuals = ui.visuals();
    let text = visuals.widgets.active.fg_stroke.color;
    let muted = visuals.widgets.inactive.fg_stroke.color;
    let line = visuals.widgets.noninteractive.bg_stroke.color;
    let active = response.hovered() || response.dragged();
    let painter = ui.painter();
    let font = fonts::regular(13.0);
    painter.text(
        rect.left_top(),
        egui::Align2::LEFT_TOP,
        label,
        font.clone(),
        if active { text } else { muted },
    );
    painter.text(
        rect.right_top(),
        egui::Align2::RIGHT_TOP,
        show(*value),
        font,
        muted,
    );
    let y = rect.bottom() - 6.0;
    let x = rect.left() + ((*value - lo) / (hi - lo)).clamp(0.0, 1.0) * rect.width();
    painter.hline(rect.x_range(), y, Stroke::new(1.0, line));
    painter.hline(rect.left()..=x, y, Stroke::new(1.0, text));
    painter.circle_filled(egui::pos2(x, y), if active { 4.5 } else { 3.5 }, text);
    response.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
}
