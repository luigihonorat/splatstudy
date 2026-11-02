// SPDX-License-Identifier: GPL-3.0-or-later WITH AdditionRef-SplatStudy-app-store-exception
//! The interface: theme, fonts, strings and the few widgets the design needs.

pub mod fonts;
pub mod i18n;
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
