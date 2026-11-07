#!/bin/sh
# SPDX-FileCopyrightText: 2026 The SplatStudy contributors
# SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
#
# Builds SplatStudy.app for macOS (Intel and Apple Silicon in one binary) and
# zips it into target/macos/SplatStudy-macos.zip. Run it on a Mac from the
# repository root; it needs Rust, CMake, the Xcode command line tools and
# cargo-about (for the licence notices).
#
# The app is signed ad hoc only, so macOS blocks it the first time; allow it
# in System Settings > Privacy & Security, or clear the quarantine flag:
#   xattr -dr com.apple.quarantine SplatStudy.app

set -eu

TARGETS="${TARGETS:-x86_64-apple-darwin aarch64-apple-darwin}"
# The oldest macOS that runs on Apple Silicon; SDL3 and wgpu's Metal need less.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"

out=target/macos
app="$out/SplatStudy.app"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

bins=""
for target in $TARGETS; do
    rustup target add "$target"
    cargo build --release -p splatstudy-native --target "$target"
    bins="$bins target/$target/release/splatstudy"
done

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
# shellcheck disable=SC2086 # one argument per binary
lipo -create -output "$app/Contents/MacOS/splatstudy" $bins

iconset="$out/SplatStudy.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
for size in 16 32 128 256; do
    sips -z $size $size crates/native/assets/icon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
done
for size in 16 32 128; do
    double=$((size * 2))
    sips -z $double $double crates/native/assets/icon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/SplatStudy.icns"
rm -rf "$iconset"

cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleDisplayName</key><string>SplatStudy</string>
    <key>CFBundleExecutable</key><string>splatstudy</string>
    <key>CFBundleIconFile</key><string>SplatStudy</string>
    <key>CFBundleIdentifier</key><string>com.splatstudy.SplatStudy</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleName</key><string>SplatStudy</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.graphics-design</string>
    <key>LSMinimumSystemVersion</key><string>$MACOSX_DEPLOYMENT_TARGET</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
EOF

cargo xtask notices "$app/Contents/Resources/licenses.html"

codesign --force --deep --sign - "$app"
rm -f "$out/SplatStudy-macos.zip"
ditto -c -k --keepParent "$app" "$out/SplatStudy-macos.zip"
echo "built $app and $out/SplatStudy-macos.zip"
