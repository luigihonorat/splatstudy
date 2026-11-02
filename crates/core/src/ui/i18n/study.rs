// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Strings for the study aids: camera link, silhouette, ghost, guides and timer.

pub struct StudyTr {
    /// The submenu that picks which views show.
    pub views: &'static str,
    pub both: &'static str,
    pub swap_sides: &'static str,
    pub grid_divisions: &'static str,
}

pub const JA: StudyTr = StudyTr {
    views: "表示するビュー",
    both: "両方",
    swap_sides: "左右を入れ替える",
    grid_divisions: "グリッドの分割数",
};

pub const EN: StudyTr = StudyTr {
    views: "views",
    both: "both",
    swap_sides: "swap sides",
    grid_divisions: "grid divisions",
};

pub const FR: StudyTr = StudyTr {
    views: "vues",
    both: "les deux",
    swap_sides: "échanger les côtés",
    grid_divisions: "divisions de la grille",
};
