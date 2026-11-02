// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Strings for scenes, opening files and autosave.

pub struct FilesTr {
    /// After the file name, when a file isn't one SplatStudy can open.
    pub cannot_open: &'static str,
    /// A new scene's file name, before the extension.
    pub default_name: &'static str,
    /// After opening a scene whose recorded session didn't lead to its
    /// clay (saved by a version that sculpted differently).
    pub timelapse_lost: &'static str,
    /// After opening a scene whose reference was damaged and left out.
    pub reference_damaged: &'static str,
    /// At start, when the autosave of the last session was opened.
    pub restored: &'static str,
    /// The same, when the reference file had been dropped by the browser.
    pub restored_without_reference: &'static str,
    /// Autosave stopped: there is nowhere to store it (private window,
    /// storage blocked).
    pub autosave_unavailable: &'static str,
    /// Autosave stopped: the storage is full.
    pub autosave_full: &'static str,
    /// The reference is too big to autosave; the clay still is.
    pub autosave_reference_too_big: &'static str,
    /// At start, when the last session wasn't restored because restoring
    /// it last time never finished (the app may have crashed doing it).
    pub restore_skipped: &'static str,
    /// File menu item, offered after that: restore it anyway.
    pub restore_last: &'static str,
    /// After choosing that item, when the session is no longer stored.
    pub autosave_gone: &'static str,
}

pub const JA: FilesTr = FilesTr {
    cannot_open: "このファイルは開けません",
    default_name: "習作",
    timelapse_lost: "タイムラプスは復元できませんでした。形は保存したときのままです",
    reference_damaged: "参照が壊れていたため、参照なしで開きました",
    restored: "前回の作業を復元しました",
    restored_without_reference: "前回の作業を復元しました（参照は復元できませんでした）",
    autosave_unavailable: "保存先が使えないため、自動保存は止まっています",
    autosave_full: "保存先がいっぱいのため、自動保存は止まっています",
    autosave_reference_too_big: "参照が大きすぎて自動保存できません。塑像は自動保存されます",
    restore_skipped: "前回の作業は復元していません（前回は復元の途中で止まりました）。ファイルメニューから復元できます",
    restore_last: "前回の作業を復元",
    autosave_gone: "前回の作業はもう保存されていません",
};

pub const EN: FilesTr = FilesTr {
    cannot_open: "this file can't be opened",
    default_name: "study",
    timelapse_lost: "the timelapse couldn't be restored; the clay is as it was saved",
    reference_damaged: "the scene's reference was damaged; it opened without it",
    restored: "restored your last session",
    restored_without_reference: "restored your last session, without its reference",
    autosave_unavailable: "autosave is off: there is nowhere to store it",
    autosave_full: "autosave is off: storage is full",
    autosave_reference_too_big: "the reference is too big to autosave; the clay is still saved",
    restore_skipped: "your last session wasn't restored: restoring it stopped halfway last time. restore it from the file menu",
    restore_last: "restore last session",
    autosave_gone: "your last session is no longer stored",
};

pub const FR: FilesTr = FilesTr {
    cannot_open: "impossible d'ouvrir ce fichier",
    default_name: "étude",
    timelapse_lost: "impossible de restaurer l'accéléré ; la terre est telle qu'enregistrée",
    reference_damaged: "la référence de la scène était abîmée ; scène ouverte sans elle",
    restored: "dernière session restaurée",
    restored_without_reference: "dernière session restaurée, sans sa référence",
    autosave_unavailable: "sauvegarde automatique arrêtée : aucun espace de stockage",
    autosave_full: "sauvegarde automatique arrêtée : stockage plein",
    autosave_reference_too_big: "référence trop lourde pour la sauvegarde automatique ; la terre reste sauvegardée",
    restore_skipped: "dernière session non restaurée : la restauration s'est arrêtée en cours la dernière fois. restaurez-la depuis le menu fichier",
    restore_last: "restaurer la dernière session",
    autosave_gone: "la dernière session n'est plus enregistrée",
};
