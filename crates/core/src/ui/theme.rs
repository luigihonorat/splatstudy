// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Colours and spacing after thevirtualform.com: monochrome, thin 1px rules,
//! no boxes; the chosen item gets the text colour and a 1px underline.

use egui::{Color32, CornerRadius, Margin, Shadow, Stroke, Style, TextStyle, Visuals, vec2};

use super::fonts;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    #[default]
    Dark,
}

/// The 3D views stay black in both themes, so the clay reads the same.
pub const VIEW_BG: Color32 = Color32::BLACK;
/// The clay's colour, before lighting.
pub const CLAY: Color32 = hex(0xC4C4C4);
/// Text and outlines drawn over the black views.
pub const VIEW_TEXT: Color32 = hex(0x8A8A8A);
pub const VIEW_INK: Color32 = hex(0xF2F2F2);
/// The brush outline's mirror image on the clay's other side, with
/// symmetry on: a muted grey beside the brush's own `VIEW_INK`.
pub const MIRROR_INK: Color32 = hex(0x8F8F8F);
/// Silhouette mode: shapes filled flat on the black view.
pub const SILHOUETTE_INK: Color32 = hex(0xE6E6E6);
/// Plumb line, grid and other guides over the views.
pub const GUIDE: Color32 = hex(0xEE8A2A);
/// The crop box around a reference.
pub const CROP_LINE: Color32 = hex(0xEE8A2A);
/// The silhouette difference over the sculpt view: where the clay reaches
/// past the reference's outline, and where it falls short of it. Muted, so
/// the form still reads through them.
pub const CLAY_OVER: Color32 = hex(0xB0694A);
pub const CLAY_UNDER: Color32 = hex(0x5B84A8);
/// The wireframe over the clay: a dark grey line, blended so the shading
/// still reads through it.
pub const WIRE: Color32 = hex(0x262626);
pub const WIRE_OPACITY: f32 = 0.6;
/// How strongly the ghost of the reference shows over the sculpt.
pub const GHOST_OPACITY: f32 = 0.35;

/// The site's CSS custom properties, one set per mode.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: Color32,
    pub surface: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub vp_bg: Color32,
    pub vp_grid: Color32,
    pub vp_grid_strong: Color32,
    pub vp_geo: Color32,
    pub highlight: Color32,
    pub axis_x: Color32,
    pub axis_y: Color32,
    pub axis_z: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub const LIGHT: Palette = Palette {
    bg: hex(0xF7F7F7),
    surface: hex(0xFFFFFF),
    text: hex(0x111111),
    muted: hex(0x666666),
    faint: hex(0xABABAB),
    border: hex(0xDDDDDD),
    border_strong: hex(0xBBBBBB),
    vp_bg: hex(0xE9E9E9),
    vp_grid: hex(0xD3D3D3),
    vp_grid_strong: hex(0xB9B9B9),
    vp_geo: hex(0xD8D8D8),
    highlight: hex(0xEE8A2A),
    axis_x: hex(0xD9534F),
    axis_y: hex(0x43A047),
    axis_z: hex(0x3F7BD9),
};

pub const DARK: Palette = Palette {
    bg: hex(0x111111),
    surface: hex(0x171717),
    text: hex(0xF2F2F2),
    muted: hex(0xA3A3A3),
    faint: hex(0x5E5E5E),
    border: hex(0x2C2C2C),
    border_strong: hex(0x474747),
    vp_bg: hex(0x1B1B1B),
    vp_grid: hex(0x2A2A2A),
    vp_grid_strong: hex(0x3C3C3C),
    vp_geo: hex(0x8E8E8E),
    highlight: hex(0xEE8A2A),
    axis_x: hex(0xD9534F),
    axis_y: hex(0x43A047),
    axis_z: hex(0x3F7BD9),
};

impl ThemeMode {
    pub fn palette(self) -> &'static Palette {
        match self {
            ThemeMode::Light => &LIGHT,
            ThemeMode::Dark => &DARK,
        }
    }

    pub fn toggled(self) -> ThemeMode {
        match self {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
        }
    }
}

/// Installs fonts, text sizes, spacing and colours for `mode`.
pub fn apply(ctx: &egui::Context, mode: ThemeMode) {
    let p = mode.palette();
    let mut style = Style {
        text_styles: [
            (TextStyle::Small, fonts::regular(12.0)),
            (TextStyle::Body, fonts::regular(14.0)),
            (TextStyle::Button, fonts::regular(14.0)),
            (TextStyle::Heading, fonts::light(20.0)),
            (TextStyle::Monospace, fonts::regular(13.0)),
        ]
        .into(),
        ..Style::default()
    };

    style.spacing.item_spacing = vec2(16.0, 8.0);
    style.spacing.button_padding = vec2(0.0, 2.0);
    style.spacing.window_margin = Margin::same(16);
    style.spacing.interact_size.y = 22.0;
    style.animation_time = 0.15;

    let mut v = if mode == ThemeMode::Dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.override_text_color = Some(p.text);
    v.weak_text_color = Some(p.muted);
    v.hyperlink_color = p.text;
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.faint_bg_color = p.surface;
    v.extreme_bg_color = p.surface;
    v.selection.bg_fill = p.border;
    v.selection.stroke = Stroke::new(1.0, p.text);
    v.button_frame = false;
    v.indent_has_left_vline = true;
    v.slider_trailing_fill = true;

    // Controls have no background or box; state shows in the text colour.
    for (w, fg, line) in [
        (&mut v.widgets.noninteractive, p.text, p.border),
        (&mut v.widgets.inactive, p.muted, p.border_strong),
        (&mut v.widgets.hovered, p.text, p.text),
        (&mut v.widgets.active, p.text, p.text),
        (&mut v.widgets.open, p.text, p.text),
    ] {
        w.bg_fill = Color32::TRANSPARENT;
        w.weak_bg_fill = Color32::TRANSPARENT;
        w.bg_stroke = Stroke::new(1.0, line);
        w.fg_stroke = Stroke::new(1.0, fg);
        w.corner_radius = CornerRadius::ZERO;
        w.expansion = 0.0;
    }

    style.visuals = v;
    ctx.set_style_of(egui::Theme::Light, style.clone());
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.set_theme(match mode {
        ThemeMode::Light => egui::ThemePreference::Light,
        ThemeMode::Dark => egui::ThemePreference::Dark,
    });
}
