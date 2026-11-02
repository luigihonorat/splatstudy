# SplatStudy — Claude Code への指示

3D の参照（ガウシアンスプラット・3D スキャン・メッシュ・画像）を右に、塑像を左に置いて制作するデジタル彫刻ツール。予備校で石膏像やモデルを見て塑像する学び方を、誰でも使える形にする。

## ハッカソンのルール（Built with Claude: Japan 2026）

- 提出締切は **2026-11-08 20:00 JST**。コードはすべて 2026-11-02 12:00 以降に書いたもの。
- **このプロジェクト以外のフォルダ（隣にある過去のプロトタイプなど）は開かない・読まない・コピーしない。** `.claude/hooks/guard-paths.sh` がツール呼び出しを止める。
- 権利のないコード・データ・素材は使わない。参照データは CC0 か CC BY（クレジット表記つき）で、ユーザーの承認を得たものだけ。
- 依存クレートは寛容なライセンス（MIT / Apache-2.0 / BSD / zlib / OFL のフォント）のみ。GPL の追加許可（App Store 例外）を保つため。

## 構成

```
crates/core    エディタ・描画・UI。プラットフォーム非依存（SDL も web-sys も使わない）
crates/native  デスクトップ版（SDL3：ペン筆圧のため。winit が全OSで筆圧対応したら置き換え）
crates/web     ブラウザ版（WebGPU、Pointer Events）
xtask          ビルド作業（cargo xtask web / serve）
web/           index.html などの静的ファイル
```

- core はイベントを受け取り（`App::push_event`）、フレームを描く（`App::frame`）。時刻は引数で受け取り、`std::time::Instant` は使わない。
- 3D ビューはそれぞれオフスクリーンのテクスチャに描き、egui の画像として表示する。
- テクスチャ形式はすべて非 sRGB。色の変換はシェーダーで明示的に行う。
- 将来の機能（ポリペイント、VR）を妨げない：頂点に色の場所を残す。描画はカメラ（ビュー）を引数で受け取る。

## コマンド

```
cargo test -p splatstudy-core        # 単体テスト
cargo xtask web                      # dist/ にブラウザ版をビルド
cargo xtask serve --port 8080        # dist/ をローカルで配信
cargo build -p splatstudy-native     # デスクトップ版（SDL3 をソースからビルドするため CMake が必要。
                                     # Windows では環境変数 CMAKE に Visual Studio 付属の cmake.exe を指定）
```

## 規約

- すべてのソースファイルの先頭に SPDX ヘッダー：
  `SPDX-License-Identifier: GPL-3.0-or-later WITH AdditionRef-SplatStudy-app-store-exception`
- UI の文字列はすべて `crates/core/src/ui/i18n.rs`（日本語・英語・フランス語）。日本語を変えたら `crates/core/assets/fonts/subset.py` でフォントを作り直す。
- デザインは thevirtualform.com に合わせる：モノクロ、1px の線、箱を使わない、選択中は文字色と 1px の下線。色は `ui/theme.rs` のトークンだけを使う。
- 彫刻ビューでは左ドラッグ＝彫る。視点操作は Alt / Space ＋ドラッグ（Houdini 式）。
- ローダーは `Result` を返し、壊れた入力でパニックしない。
- 小さくコミットする。コミットは `git commit -s`（DCO の Signed-off-by）で、`Co-Authored-By: Claude` も付ける。詳しくは `CONTRIBUTING.md`。
- メモ・計画・開発ログはリポジトリの外（`../`）に置く。

## 完了の定義

- テストで確認したか、スクリーンショットを実際に見て確認した。
- `cargo xtask web` が通り、wasm のサイズ予算（12 MiB）を超えない。
