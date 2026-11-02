// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Strings for the study aids: camera link, silhouette, ghost, guides and timer.

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
