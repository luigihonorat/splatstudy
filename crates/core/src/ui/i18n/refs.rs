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
    /// Notice when a mesh is larger than the GPU's buffers.
    pub mesh_too_large: &'static str,
    /// Heading of the reference panel's group that sets which way is up
    /// and how the reference turns.
    pub alignment: &'static str,
    /// The switch for editing the crop box, under the "crop" heading: its
    /// handles and sides show while it is on; off, the crop holds.
    pub crop_box: &'static str,
    /// Takes the crop away, under the crop box switch.
    pub remove_crop: &'static str,
    /// The switch for splat colours that change with the side they are
    /// seen from (3DGS spherical harmonics); off shows each splat's base
    /// colour from every side.
    pub view_dependent_colour: &'static str,
    /// File menu item that opens the gallery of bundled casts.
    pub open_gallery: &'static str,
    /// The same, shorter, under "import reference" in the reference panel.
    pub panel_gallery: &'static str,
    /// The gallery window's title.
    pub gallery: &'static str,
    /// A link under the empty reference view's drop hint, to the gallery.
    pub gallery_link: &'static str,
    /// Under the gallery's grid while no cast is pointed at.
    pub gallery_note: &'static str,
    /// While a cast downloads; `{name}` is its short name.
    pub loading: &'static str,
    /// Notice when a cast from the gallery couldn't be downloaded.
    pub download_failed: &'static str,
    /// Each bundled sample's short name under its thumbnail, by its id
    /// (see `reference/samples.rs`).
    pub sample_names: &'static [(&'static str, &'static str)],
}

pub const JA: RefTr = RefTr {
    up: "上方向",
    turn: "回転",
    fit: "メッシュに合わせる",
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
    mesh_too_large: "メッシュが大きすぎて、この GPU では表示できません",
    alignment: "位置合わせ",
    crop_box: "切り抜き枠",
    remove_crop: "切り抜きを解除",
    view_dependent_colour: "見る向きで変わる色",
    open_gallery: "ギャラリーからモチーフを選ぶ…",
    panel_gallery: "ギャラリーから読み込む",
    gallery: "ギャラリー",
    gallery_link: "ギャラリーから石膏像を選ぶ",
    gallery_note: "クリックするとモチーフとして読み込みます",
    loading: "{name}を読み込み中…",
    download_failed: "石膏像をダウンロードできませんでした。接続を確認してください",
    sample_names: &[
        ("brutus", "ブルータス"),
        ("venus", "ミロのヴィーナス"),
        ("david", "ダビデ"),
        ("homer", "ホメロス"),
        ("socrates", "ソクラテス"),
        ("caesar", "カエサル"),
        ("hercules", "ヘラクレス"),
        ("foot", "運動選手の右足"),
        ("hands", "リンカーンの手"),
    ],
};

pub const EN: RefTr = RefTr {
    up: "up axis",
    turn: "turn",
    fit: "fit to the mesh",
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
    mesh_too_large: "this mesh is too large for this GPU",
    alignment: "alignment",
    crop_box: "crop box",
    remove_crop: "remove crop",
    view_dependent_colour: "view-dependent colour",
    open_gallery: "open reference from gallery…",
    panel_gallery: "load from gallery",
    gallery: "gallery",
    gallery_link: "or choose a cast from the gallery",
    gallery_note: "click a cast to load it as the reference",
    loading: "loading {name}…",
    download_failed: "couldn't download the cast; check the connection",
    sample_names: &[
        ("brutus", "brutus"),
        ("venus", "venus de milo"),
        ("david", "david"),
        ("homer", "homer"),
        ("socrates", "socrates"),
        ("caesar", "julius caesar"),
        ("hercules", "hercules"),
        ("foot", "an athlete's foot"),
        ("hands", "lincoln's hands"),
    ],
};

pub const FR: RefTr = RefTr {
    up: "axe vertical",
    turn: "rotation",
    fit: "ajuster au maillage",
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
    mesh_too_large: "ce maillage est trop grand pour ce GPU",
    alignment: "alignement",
    crop_box: "boîte de recadrage",
    remove_crop: "retirer le recadrage",
    view_dependent_colour: "couleur selon l'angle de vue",
    open_gallery: "choisir un modèle dans la galerie…",
    panel_gallery: "charger depuis la galerie",
    gallery: "galerie",
    gallery_link: "ou choisissez un plâtre dans la galerie",
    gallery_note: "cliquez sur un plâtre pour le charger comme modèle",
    loading: "chargement : {name}…",
    download_failed: "impossible de télécharger le plâtre ; vérifiez la connexion",
    sample_names: &[
        ("brutus", "brutus"),
        ("venus", "vénus de milo"),
        ("david", "david"),
        ("homer", "homère"),
        ("socrates", "socrate"),
        ("caesar", "jules césar"),
        ("hercules", "hercule"),
        ("foot", "pied d'athlète"),
        ("hands", "mains de lincoln"),
    ],
};
