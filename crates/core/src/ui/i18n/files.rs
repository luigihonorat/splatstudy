// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Strings for scenes and opening files.

pub struct FilesTr {
    /// After the file name, when a file isn't one SplatStudy can open.
    pub cannot_open: &'static str,
}

pub const JA: FilesTr = FilesTr {
    cannot_open: "このファイルは開けません",
};

pub const EN: FilesTr = FilesTr {
    cannot_open: "this file can't be opened",
};

pub const FR: FilesTr = FilesTr {
    cannot_open: "impossible d'ouvrir ce fichier",
};
