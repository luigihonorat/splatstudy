// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Strings for getting started and the first-run hint.

pub struct HelpTr {
    /// The first lines of the getting started window.
    pub intro: &'static str,
    pub reference_heading: &'static str,
    pub reference_text: &'static str,
    pub sculpt_heading: &'static str,
    /// `{invert}` stands for the opposite action's key, which depends on
    /// the navigation style.
    pub sculpt_text: &'static str,
    /// Over the study aids' keys.
    pub study_heading: &'static str,
    /// Under the navigation hint.
    pub navigation_text: &'static str,
    pub more: &'static str,
    /// A link under the sculpt view on the first visit; opens getting started.
    pub first_run_hint: &'static str,
}

pub const JA: HelpTr = HelpTr {
    intro: "右にモチーフ、左に塑像。予備校で石膏像を見て作るように、見比べながら形をとります。",
    reference_heading: "モチーフ",
    reference_text: "スプラット（.ply .splat .spz）、3D スキャン（.obj .stl）、画像をウィンドウにドロップするか、ファイル › モチーフを読み込む。",
    sculpt_heading: "塑像",
    sculpt_text: "粘土の上をドラッグ。{invert}：逆の動き　Shift：スムーズ　[ ]：大きさ",
    study_heading: "見比べる",
    navigation_text: "ほかの操作方法は「視点操作」メニューから。",
    more: "すべてのショートカット：ヘルプ › キーボードショートカット",
    first_run_hint: "はじめての方へ：使い方",
};

pub const EN: HelpTr = HelpTr {
    intro: "the reference on the right, your sculpt on the left: compare them as you work, the way students study from plaster casts.",
    reference_heading: "reference",
    reference_text: "drop a splat (.ply .splat .spz), a 3D scan (.obj .stl) or an image on the window, or use file › import reference.",
    sculpt_heading: "sculpt",
    sculpt_text: "drag on the clay. {invert}: opposite action   Shift: smooth   [ ]: size",
    study_heading: "compare",
    navigation_text: "other styles are in the navigation menu.",
    more: "every shortcut: help › keyboard shortcuts",
    first_run_hint: "new here? getting started",
};

pub const FR: HelpTr = HelpTr {
    intro: "le modèle à droite, votre sculpture à gauche : comparez-les en travaillant, comme on étudie d'après le plâtre.",
    reference_heading: "modèle",
    reference_text: "déposez un splat (.ply .splat .spz), un scan 3D (.obj .stl) ou une image sur la fenêtre, ou passez par fichier › importer un modèle.",
    sculpt_heading: "sculpture",
    sculpt_text: "glissez sur l'argile. {invert} : action inverse   Maj : lisser   [ ] : taille",
    study_heading: "comparer",
    navigation_text: "les autres styles sont dans le menu navigation.",
    more: "tous les raccourcis : aide › raccourcis clavier",
    first_run_hint: "première visite ? premiers pas",
};
