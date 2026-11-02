// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Strings for the reference: loading, alignment, crop and credit.

pub struct RefTr {
    /// Heading over the up-axis choices.
    pub up: &'static str,
    /// Slider: turn about the vertical.
    pub turn: &'static str,
    /// Centre and scale the reference to the clay again.
    pub fit: &'static str,
    /// Show the whole picture again after panning and zooming.
    pub fit_image: &'static str,
    /// Heading and switch for the crop box.
    pub crop: &'static str,
    /// The crop box's six sides, as seen from the front.
    pub crop_left: &'static str,
    pub crop_right: &'static str,
    pub crop_bottom: &'static str,
    pub crop_top: &'static str,
    pub crop_back: &'static str,
    pub crop_front: &'static str,
    /// Heading over the credit fields.
    pub credit: &'static str,
    pub title: &'static str,
    pub author: &'static str,
    pub licence: &'static str,
    pub source: &'static str,
    /// Notice when a file has more splats than the GPU can hold.
    pub thinned: &'static str,
}

pub const JA: RefTr = RefTr {
    up: "上方向",
    turn: "回転",
    fit: "大きさを合わせる",
    fit_image: "全体を表示",
    crop: "切り抜き",
    crop_left: "左",
    crop_right: "右",
    crop_bottom: "下",
    crop_top: "上",
    crop_back: "奥",
    crop_front: "手前",
    credit: "クレジット",
    title: "作品名",
    author: "作者",
    licence: "ライセンス",
    source: "出典",
    thinned: "スプラットが多いため、一部を間引いて表示しています",
};

pub const EN: RefTr = RefTr {
    up: "up axis",
    turn: "turn",
    fit: "fit to the clay",
    fit_image: "fit to view",
    crop: "crop",
    crop_left: "left",
    crop_right: "right",
    crop_bottom: "bottom",
    crop_top: "top",
    crop_back: "back",
    crop_front: "front",
    credit: "credit",
    title: "title",
    author: "author",
    licence: "licence",
    source: "source",
    thinned: "too many splats for this GPU: showing part of them",
};

pub const FR: RefTr = RefTr {
    up: "axe vertical",
    turn: "rotation",
    fit: "ajuster à la terre",
    fit_image: "tout afficher",
    crop: "recadrer",
    crop_left: "gauche",
    crop_right: "droite",
    crop_bottom: "bas",
    crop_top: "haut",
    crop_back: "arrière",
    crop_front: "avant",
    credit: "crédit",
    title: "titre",
    author: "auteur",
    licence: "licence",
    source: "source",
    thinned: "trop de splats pour ce GPU : une partie seulement est affichée",
};
