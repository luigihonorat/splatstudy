<h1>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="web/logo/splatstudy-wordmark-white.svg">
    <img src="web/logo/splatstudy-wordmark-black.svg" alt="SplatStudy" width="200">
  </picture>
</h1>

3D の参照を横に置いて塑像する、デジタル彫刻ツール。

画面は左右に分かれています。左が自分の塑像、右がモチーフ（参照）です。モチーフにはガウシアンスプラットがいちばん向いていますが、3D スキャン、3D メッシュ、画像（PureRef のような画像パネル）も使えます。

美術予備校では、石膏像や人物モデルを前にして粘土で塑像を学びます。SplatStudy は、その学び方を予備校に通えない人にも届けるためのツールです。

> **Built with Claude: Japan 2026** の参加作品です（2026年11月2日〜8日）。コードはすべてこの期間に Claude Code と書いています。

## 使う

ブラウザ版：<https://app.splatstudy.com>（WebGPU が必要です。Chrome / Edge の最新版、Safari 26 以降）

開発中です。今の状態は下の「進み具合」を見てください。

## 進み具合

- [x] ブラウザ版の土台（WebGPU、左右分割、日本語・英語・フランス語）
- [ ] 塑像（ブラシ、筆圧、対称、元に戻す）
- [ ] モチーフの読み込み（スプラット、3D スキャン、画像）
- [ ] 学ぶための道具（視点の連動、シルエット比較、反転、鉛直線、タイマー）
- [x] デスクトップ版の土台（SDL3、ペン筆圧。Windows を CI でビルド）

## ビルド

Rust 1.90 以降と `wasm32-unknown-unknown` ターゲットが必要です。

```
cargo test -p splatstudy-core
cargo xtask web          # dist/ にブラウザ版
cargo xtask serve        # http://127.0.0.1:8080
cargo run -p splatstudy-native --release   # デスクトップ版（CMake が必要）
```

## Claude Code での開発

このリポジトリは Claude Code と一緒に作っています。

- [`CLAUDE.md`](CLAUDE.md)：Claude Code への指示（ルール、構成、規約、完了の定義）
- [`.claude/hooks/guard-paths.sh`](.claude/hooks/guard-paths.sh)：ハッカソン中に過去のプロトタイプを開かないようにするフック

## ライセンス

GPL-3.0-or-later に、アプリストアで配布するための追加許可（[`LICENSE-EXCEPTION`](LICENSE-EXCEPTION)）を付けています。フォント（Barlow、Noto Sans JP）は SIL Open Font License です。すべてのファイルのライセンスは [REUSE](https://reuse.software/) 形式で書かれています（[`LICENSES/`](LICENSES)）。使っている第三者のソフトウェアとフォントのライセンス表示は、配布するすべてのビルドに `licenses.html` として付けています（ブラウザ版：<https://app.splatstudy.com/licenses.html>）。

視点操作は ZBrush・Blender・Maya 風から選べます。Blender、Maya、ZBrush はそれぞれの権利者の商標で、操作方法の説明のためにだけ名前を挙げています。

参加のしかたは [`CONTRIBUTING.md`](CONTRIBUTING.md)、脆弱性の報告は [`SECURITY.md`](SECURITY.md) を見てください。

---

**English.** SplatStudy is a digital sculpting tool for studying from 3D references: your sculpture on the left, the reference on the right (Gaussian splats, 3D scans, meshes or images). It brings the Japanese art-prep-school practice of sculpting from plaster casts and live models to anyone with a browser. Made for Built with Claude: Japan 2026. Licence: GPL-3.0-or-later with an app store exception.
