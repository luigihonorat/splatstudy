// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Strings for the study aids: camera link, silhouette, ghost, guides, timer,
//! silhouette match, proportion measure and lighting.

pub struct StudyTr {
    /// The submenu that picks which views show.
    pub views: &'static str,
    pub both: &'static str,
    pub swap_sides: &'static str,
    pub grid_divisions: &'static str,
    pub pose_timer: &'static str,
    pub start: &'static str,
    pub resume: &'static str,
    pub stop: &'static str,
    /// The notice when the last round ends.
    pub time_up: &'static str,
    /// A round's length, in minutes.
    pub minutes: fn(u32) -> String,
    /// How many rounds the timer runs.
    pub rounds: fn(u32) -> String,
    /// Before the silhouette match score, a share in percent.
    pub match_score: &'static str,
    /// Tints where the clay's outline is too big and too small.
    pub difference: &'static str,
    /// The difference tint's legend: the clay reaches past the reference.
    pub too_big: &'static str,
    /// The difference tint's legend: the clay falls short of it.
    pub too_small: &'static str,
    /// The proportion measure, in the view menu.
    pub measure: &'static str,
    /// Shown in a view still waiting for its measured line.
    pub measure_hint: &'static str,
    /// The lighting submenu and its three lightings.
    pub lighting: &'static str,
    pub studio: &'static str,
    pub raking: &'static str,
    pub top_light: &'static str,
    /// The mesh's edges drawn over the clay, in the view menu.
    pub wireframe: &'static str,
    /// The perspective, on; off, the view is orthographic.
    pub perspective: &'static str,
    /// Fits the reference view to the reference.
    pub frame_reference: &'static str,
    /// In the view menu, over the items that switch one view's own aids.
    pub sculpt_view: &'static str,
    pub reference_view: &'static str,
    /// Hides the side panels (Tab), and the reference view too
    /// (Shift+Tab), so the views take the whole window.
    pub hide_panels: &'static str,
    pub hide_panels_and_reference: &'static str,
    /// Under the shortcuts: what Tab does after Shift+Tab, and in a text
    /// field.
    pub panels_note: &'static str,
}

pub const JA: StudyTr = StudyTr {
    views: "表示するビュー",
    both: "両方",
    swap_sides: "左右を入れ替える",
    grid_divisions: "グリッドの分割数",
    pose_timer: "タイマー",
    start: "開始",
    resume: "再開",
    stop: "止める",
    time_up: "時間です",
    minutes: ja_minutes,
    rounds: ja_rounds,
    match_score: "一致",
    difference: "シルエットのずれ",
    too_big: "はみ出し",
    too_small: "足りない",
    measure: "比率を測る",
    measure_hint: "ドラッグして長さを測る",
    lighting: "ライト",
    studio: "スタジオ",
    raking: "斜光",
    top_light: "天窓",
    wireframe: "ワイヤーフレーム",
    perspective: "パース表示",
    frame_reference: "モチーフに合わせる",
    sculpt_view: "塑像のビュー",
    reference_view: "モチーフのビュー",
    hide_panels: "パネルを隠す",
    hide_panels_and_reference: "パネルとモチーフを隠す",
    panels_note: "Shift+Tab のあとの Tab は、パネルとモチーフを両方戻す
文字の入力中は、Tab で次の欄へ移る",
};

pub const EN: StudyTr = StudyTr {
    views: "views",
    both: "both",
    swap_sides: "swap sides",
    grid_divisions: "grid divisions",
    pose_timer: "pose timer",
    start: "start",
    resume: "resume",
    stop: "stop",
    time_up: "time is up",
    minutes: en_minutes,
    rounds: en_rounds,
    match_score: "match",
    difference: "silhouette difference",
    too_big: "too big",
    too_small: "too small",
    measure: "measure proportions",
    measure_hint: "drag to measure a length",
    lighting: "lighting",
    studio: "studio",
    raking: "raking",
    top_light: "top",
    wireframe: "wireframe",
    perspective: "perspective",
    frame_reference: "frame reference",
    sculpt_view: "sculpt view",
    reference_view: "reference view",
    hide_panels: "hide panels",
    hide_panels_and_reference: "hide panels and reference",
    panels_note: "after Shift+Tab, Tab brings back the panels and the reference
in a text field, Tab goes to the next field",
};

pub const FR: StudyTr = StudyTr {
    views: "vues",
    both: "les deux",
    swap_sides: "échanger les côtés",
    grid_divisions: "divisions de la grille",
    pose_timer: "minuteur de pose",
    start: "démarrer",
    resume: "reprendre",
    stop: "arrêter",
    time_up: "temps écoulé",
    minutes: fr_minutes,
    rounds: fr_rounds,
    match_score: "accord",
    difference: "écart de silhouette",
    too_big: "en trop",
    too_small: "en moins",
    measure: "mesurer les proportions",
    measure_hint: "glisser pour mesurer une longueur",
    lighting: "éclairage",
    studio: "atelier",
    raking: "rasant",
    top_light: "zénithal",
    wireframe: "fil de fer",
    perspective: "perspective",
    frame_reference: "cadrer le modèle",
    sculpt_view: "vue sculpture",
    reference_view: "vue modèle",
    hide_panels: "masquer les panneaux",
    hide_panels_and_reference: "masquer les panneaux et le modèle",
    panels_note: "après Maj+Tab, Tab rétablit les panneaux et le modèle
dans un champ de texte, Tab passe au champ suivant",
};

fn ja_minutes(n: u32) -> String {
    format!("{n}分")
}

fn ja_rounds(n: u32) -> String {
    format!("{n}回")
}

fn en_minutes(n: u32) -> String {
    format!("{n} min")
}

fn en_rounds(n: u32) -> String {
    if n == 1 {
        "1 round".to_owned()
    } else {
        format!("{n} rounds")
    }
}

fn fr_minutes(n: u32) -> String {
    format!("{n} min")
}

fn fr_rounds(n: u32) -> String {
    if n == 1 {
        "1 tour".to_owned()
    } else {
        format!("{n} tours")
    }
}
