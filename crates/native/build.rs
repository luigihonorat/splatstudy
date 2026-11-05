// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Builds the app icon into the Windows .exe. Explorer shows it, and SDL
//! uses the .exe's first icon for the window and the taskbar. macOS and
//! Linux set theirs at startup (`set_icon` in main.rs).

fn main() {
    println!("cargo:rerun-if-changed=assets/splatstudy.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/splatstudy.ico");
        if let Err(err) = res.compile() {
            panic!("app icon: {err}");
        }
    }
}
