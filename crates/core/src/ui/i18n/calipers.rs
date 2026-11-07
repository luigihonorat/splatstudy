// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Strings for the calipers: lengths set between two points, seen on the
//! reference and on the sculpt alike.

pub struct CalipersTr {
    /// The panel group, the window menu item and the toolbar button that
    /// shows or hides them in a view.
    pub calipers: &'static str,
    /// Starts placing a new caliper.
    pub add: &'static str,
    pub delete_all: &'static str,
    /// Over the views while a new caliper is being placed.
    pub place_hint: &'static str,
    /// In the panel while there is no caliper yet.
    pub empty: &'static str,
    /// A caliper's own button in the panel's list.
    pub show_hide: &'static str,
    pub delete: &'static str,
    /// Beside each caliper after the first: its length as a share of the
    /// first one's.
    pub ratio: &'static str,
}

pub const JA: CalipersTr = CalipersTr {
    calipers: "キャリパー",
    add: "キャリパーを追加",
    delete_all: "すべて削除",
    place_hint: "2点の間をドラッグ",
    empty: "同じ長さをモチーフと塑像で比べる",
    show_hide: "表示・非表示",
    delete: "削除",
    ratio: "1本目との比",
};

pub const EN: CalipersTr = CalipersTr {
    calipers: "calipers",
    add: "add caliper",
    delete_all: "delete all",
    place_hint: "drag between two points",
    empty: "compare one length on the reference and on the sculpt",
    show_hide: "show or hide",
    delete: "delete",
    ratio: "ratio to the first",
};

pub const FR: CalipersTr = CalipersTr {
    calipers: "compas",
    add: "ajouter un compas",
    delete_all: "tout supprimer",
    place_hint: "glisser entre deux points",
    empty: "comparer une même longueur sur le modèle et sur la sculpture",
    show_hide: "afficher ou masquer",
    delete: "supprimer",
    ratio: "rapport au premier",
};
