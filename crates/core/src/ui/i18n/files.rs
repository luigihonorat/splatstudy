// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Strings for scenes and opening files.

pub struct FilesTr {
    /// After the file name, when a file isn't one SplatStudy can open.
    pub cannot_open: &'static str,
    /// A new scene's file name, before the extension.
    pub default_name: &'static str,
    /// After opening a scene whose recorded session didn't lead to its
    /// clay (saved by a version that sculpted differently).
    pub timelapse_lost: &'static str,
}

pub const JA: FilesTr = FilesTr {
    cannot_open: "このファイルは開けません",
    default_name: "習作",
    timelapse_lost: "タイムラプスは復元できませんでした。形は保存したときのままです",
};

pub const EN: FilesTr = FilesTr {
    cannot_open: "this file can't be opened",
    default_name: "study",
    timelapse_lost: "the timelapse couldn't be restored; the clay is as it was saved",
};

pub const FR: FilesTr = FilesTr {
    cannot_open: "impossible d'ouvrir ce fichier",
    default_name: "étude",
    timelapse_lost: "impossible de restaurer l'accéléré ; la terre est telle qu'enregistrée",
};
