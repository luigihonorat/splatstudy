// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Barlow for Latin text with Noto Sans JP as its fallback, both OFL. The
//! files are subsets made by `assets/fonts/subset.py`; Noto Sans JP is one
//! variable font used at two weights.

use std::sync::Arc;

use egui::FontId;
use egui::epaint::text::{FontData, FontDefinitions, FontFamily, FontTweak, VariationCoords};

const BARLOW_LIGHT: &[u8] = include_bytes!("../../assets/fonts/Barlow-Light.ttf");
const BARLOW_REGULAR: &[u8] = include_bytes!("../../assets/fonts/Barlow-Regular.ttf");
pub(crate) const NOTO_SANS_JP: &[u8] = include_bytes!("../../assets/fonts/NotoSansJP-UI.ttf");

const LIGHT: &str = "light";

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn light(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(LIGHT.into()))
}

fn noto_at(weight: f32) -> FontData {
    FontData::from_static(NOTO_SANS_JP).tweak(FontTweak {
        coords: VariationCoords::new([("wght", weight)]),
        ..Default::default()
    })
}

pub fn definitions() -> FontDefinitions {
    let mut defs = FontDefinitions::empty();
    let data = [
        ("barlow-light", FontData::from_static(BARLOW_LIGHT)),
        ("barlow-regular", FontData::from_static(BARLOW_REGULAR)),
        ("noto-jp-300", noto_at(300.0)),
        ("noto-jp-400", noto_at(400.0)),
    ];
    for (name, font) in data {
        defs.font_data.insert(name.into(), Arc::new(font));
    }
    let regular = vec!["barlow-regular".to_owned(), "noto-jp-400".to_owned()];
    defs.families
        .insert(FontFamily::Proportional, regular.clone());
    defs.families.insert(FontFamily::Monospace, regular);
    defs.families.insert(
        FontFamily::Name(LIGHT.into()),
        vec!["barlow-light".to_owned(), "noto-jp-300".to_owned()],
    );
    defs
}

#[cfg(test)]
mod tests {
    use egui::epaint::text::{Fonts, TextOptions};

    use super::*;

    /// The language tables, as subset.py reads them.
    const TABLES: [&str; 6] = [
        include_str!("i18n.rs"),
        include_str!("i18n/export.rs"),
        include_str!("i18n/files.rs"),
        include_str!("i18n/mesh.rs"),
        include_str!("i18n/refs.rs"),
        include_str!("i18n/study.rs"),
    ];

    /// The contents of every string literal outside comments.
    fn literals(source: &str) -> String {
        let mut out = String::new();
        for line in source.lines() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            let mut inside = false;
            let mut chars = code.chars();
            while let Some(c) = chars.next() {
                match (inside, c) {
                    (_, '"') => inside = !inside,
                    (true, '\\') => {
                        chars.next();
                    }
                    (true, c) => out.push(c),
                    (false, _) => {}
                }
            }
            // Multi-line strings continue on the next line.
            if inside {
                out.push('\n');
            }
        }
        out
    }

    #[test]
    fn the_embedded_fonts_cover_every_ui_string() {
        let mut fonts = Fonts::new(TextOptions::default(), definitions());
        let text: String = TABLES.iter().map(|t| literals(t)).collect();
        assert!(
            text.contains("塑像") && text.contains("sculpter"),
            "strings read"
        );
        let mut missing: Vec<char> = text
            .chars()
            .filter(|c| !c.is_whitespace())
            .filter(|&c| !(fonts.has_glyph(&regular(14.0), c) && fonts.has_glyph(&light(14.0), c)))
            .collect();
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "no glyph for {missing:?}: run assets/fonts/subset.py"
        );
    }
}
