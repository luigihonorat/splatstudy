// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! UI strings for every language. Each language is one `Tr` value, so a
//! missing string is a compile error. Japanese is the primary language.
//!
//! After changing Japanese text, regenerate the embedded font subset with
//! `assets/fonts/subset.py`.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    Ja,
    En,
    Fr,
}

impl Lang {
    pub const ALL: [Lang; 3] = [Lang::Ja, Lang::En, Lang::Fr];

    pub fn tr(self) -> &'static Tr {
        match self {
            Lang::Ja => &JA,
            Lang::En => &EN,
            Lang::Fr => &FR,
        }
    }

    /// The language's name in that language, for the language switcher.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::Ja => "日本語",
            Lang::En => "english",
            Lang::Fr => "français",
        }
    }

    /// Best match for a BCP 47 tag such as `ja-JP` or `fr-CA`.
    pub fn from_tag(tag: &str) -> Lang {
        let primary = tag
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match primary.as_str() {
            "en" => Lang::En,
            "fr" => Lang::Fr,
            _ => Lang::Ja,
        }
    }
}

pub struct Tr {
    pub sculpt: &'static str,
    pub reference: &'static str,
    pub open_reference: &'static str,
    pub drop_hint: &'static str,
    pub theme_light: &'static str,
    pub theme_dark: &'static str,
}

pub const JA: Tr = Tr {
    sculpt: "塑像",
    reference: "モチーフ",
    open_reference: "モチーフを開く",
    drop_hint: "スプラット・3Dスキャン・画像をここにドロップ",
    theme_light: "ライト",
    theme_dark: "ダーク",
};

pub const EN: Tr = Tr {
    sculpt: "sculpt",
    reference: "reference",
    open_reference: "open reference",
    drop_hint: "drop a splat, 3D scan or image here",
    theme_light: "light",
    theme_dark: "dark",
};

pub const FR: Tr = Tr {
    sculpt: "sculpture",
    reference: "modèle",
    open_reference: "ouvrir un modèle",
    drop_hint: "déposez ici un splat, un scan 3D ou une image",
    theme_light: "clair",
    theme_dark: "sombre",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_pick_the_right_language() {
        assert_eq!(Lang::from_tag("ja-JP"), Lang::Ja);
        assert_eq!(Lang::from_tag("fr-CA"), Lang::Fr);
        assert_eq!(Lang::from_tag("en_US"), Lang::En);
        assert_eq!(Lang::from_tag("de"), Lang::Ja);
    }
}
