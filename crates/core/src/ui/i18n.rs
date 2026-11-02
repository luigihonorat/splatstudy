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
    pub tools: &'static str,
    pub clay: &'static str,
    pub smooth: &'static str,
    pub move_: &'static str,
    pub size: &'static str,
    pub strength: &'static str,
    pub symmetry: &'static str,
    pub history: &'static str,
    pub undo: &'static str,
    pub redo: &'static str,
    pub new_sphere: &'static str,
    pub reference_tools: &'static str,
    pub navigation: &'static str,
    pub hint_zbrush: &'static str,
    pub hint_blender: &'static str,
    pub hint_maya: &'static str,
}

pub const JA: Tr = Tr {
    sculpt: "塑像",
    reference: "モチーフ",
    open_reference: "モチーフを開く",
    drop_hint: "スプラット・3Dスキャン・画像をここにドロップ",
    theme_light: "ライト",
    theme_dark: "ダーク",
    tools: "ツール",
    clay: "クレイ",
    smooth: "スムーズ",
    move_: "ムーブ",
    size: "大きさ",
    strength: "強さ",
    symmetry: "左右対称",
    history: "履歴",
    undo: "元に戻す",
    redo: "やり直す",
    new_sphere: "新しい球",
    reference_tools: "モチーフ",
    navigation: "視点操作",
    hint_zbrush: "背景をドラッグ：回転
Alt：移動
右ドラッグ：回転
Ctrl＋右：拡大",
    hint_blender: "中ボタン：回転
Shift＋中：移動
Ctrl＋中：拡大",
    hint_maya: "Alt＋左：回転
Alt＋中：移動
Alt＋右：拡大",
};

pub const EN: Tr = Tr {
    sculpt: "sculpt",
    reference: "reference",
    open_reference: "open reference",
    drop_hint: "drop a splat, 3D scan or image here",
    theme_light: "light",
    theme_dark: "dark",
    tools: "tools",
    clay: "clay",
    smooth: "smooth",
    move_: "move",
    size: "size",
    strength: "strength",
    symmetry: "symmetry",
    history: "history",
    undo: "undo",
    redo: "redo",
    new_sphere: "new sphere",
    reference_tools: "reference",
    navigation: "navigation",
    hint_zbrush: "drag the background: turn
Alt: pan
right drag: turn
Ctrl + right: zoom",
    hint_blender: "middle: turn
Shift + middle: pan
Ctrl + middle: zoom",
    hint_maya: "Alt + left: turn
Alt + middle: pan
Alt + right: zoom",
};

pub const FR: Tr = Tr {
    sculpt: "sculpture",
    reference: "modèle",
    open_reference: "ouvrir un modèle",
    drop_hint: "déposez ici un splat, un scan 3D ou une image",
    theme_light: "clair",
    theme_dark: "sombre",
    tools: "outils",
    clay: "argile",
    smooth: "lisser",
    move_: "déplacer",
    size: "taille",
    strength: "force",
    symmetry: "symétrie",
    history: "historique",
    undo: "annuler",
    redo: "rétablir",
    new_sphere: "nouvelle sphère",
    reference_tools: "modèle",
    navigation: "navigation",
    hint_zbrush: "glisser le fond : tourner
Alt : déplacer
clic droit : tourner
Ctrl + droit : zoomer",
    hint_blender: "milieu : tourner
Maj + milieu : déplacer
Ctrl + milieu : zoomer",
    hint_maya: "Alt + gauche : tourner
Alt + milieu : déplacer
Alt + droit : zoomer",
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
