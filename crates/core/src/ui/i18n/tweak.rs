// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Strings for the tweaks: one step of an operation over the whole sculpt,
//! from the sculpt panel. Smooth and inflate share their names with the
//! brushes.

pub struct TweakTr {
    /// The panel group, and its item in the window menu.
    pub tweak: &'static str,
    pub smooth: &'static str,
    pub sharpen: &'static str,
    pub shrink: &'static str,
    pub inflate: &'static str,
}

pub const JA: TweakTr = TweakTr {
    tweak: "微調整",
    smooth: "スムーズ",
    sharpen: "シャープ",
    shrink: "シュリンク",
    inflate: "インフレート",
};

pub const EN: TweakTr = TweakTr {
    tweak: "tweak",
    smooth: "smooth",
    sharpen: "sharpen",
    shrink: "shrink",
    inflate: "inflate",
};

pub const FR: TweakTr = TweakTr {
    tweak: "retouche",
    smooth: "lisser",
    sharpen: "accentuer",
    shrink: "rétrécir",
    inflate: "gonfler",
};
