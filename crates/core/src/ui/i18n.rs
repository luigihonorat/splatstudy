// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! UI strings for every language. Each language is one `Tr` value, so a
//! missing string is a compile error. Japanese is the primary language.
//!
//! Each part of the editor keeps its own strings in a file under `i18n/`,
//! with the same three languages, so parts built at the same time don't edit
//! one file. Strings shared by the whole app stay here.
//!
//! After changing Japanese text, regenerate the embedded font subset with
//! `assets/fonts/subset.py`.

pub mod export;
pub mod files;
pub mod help;
pub mod mesh;
pub mod refs;
pub mod study;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    Ja,
    En,
    Fr,
}

impl Lang {
    pub const ALL: [Lang; 3] = [Lang::Ja, Lang::En, Lang::Fr];

    pub fn tr(self) -> &'static Tr {
        match self {
            Lang::Ja => &JA,
            Lang::En => &EN,
            Lang::Fr => &FR,
        }
    }

    /// The language's name in that language, for the language switcher.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::Ja => "日本語",
            Lang::En => "english",
            Lang::Fr => "français",
        }
    }

    /// Best match for a BCP 47 tag such as `ja-JP` or `fr-CA`.
    pub fn from_tag(tag: &str) -> Lang {
        let primary = tag
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match primary.as_str() {
            "en" => Lang::En,
            "fr" => Lang::Fr,
            _ => Lang::Ja,
        }
    }
}

pub struct Tr {
    pub sculpt: &'static str,
    pub reference: &'static str,
    pub open_reference: &'static str,
    pub drop_hint: &'static str,
    pub tools: &'static str,
    pub clay: &'static str,
    pub smooth: &'static str,
    pub move_: &'static str,
    pub flatten: &'static str,
    pub inflate: &'static str,
    pub crease: &'static str,
    pub pinch: &'static str,
    pub invert: &'static str,
    pub size: &'static str,
    pub strength: &'static str,
    pub symmetry: &'static str,
    pub undo: &'static str,
    pub redo: &'static str,
    pub new_sphere: &'static str,
    pub watch_timelapse: &'static str,
    pub timelapse: &'static str,
    pub play: &'static str,
    pub pause: &'static str,
    pub back_to_sculpt: &'static str,
    pub reference_tools: &'static str,
    pub navigation: &'static str,
    pub hint_zbrush: &'static str,
    pub hint_blender: &'static str,
    pub hint_maya: &'static str,
    pub menu_file: &'static str,
    pub menu_edit: &'static str,
    pub menu_view: &'static str,
    pub menu_language: &'static str,
    pub menu_help: &'static str,
    pub menu_object: &'static str,
    pub menu_timelapse: &'static str,
    /// The navigation presets, named after the software whose hands they keep.
    pub nav_zbrush: &'static str,
    pub nav_blender: &'static str,
    pub nav_maya: &'static str,
    pub new_scene: &'static str,
    pub open_scene: &'static str,
    pub open_recent: &'static str,
    pub save_scene: &'static str,
    pub save_scene_as: &'static str,
    pub revert: &'static str,
    pub import_reference: &'static str,
    pub import_sculpt: &'static str,
    pub export: &'static str,
    pub exit: &'static str,
    pub mirror_sculpt: &'static str,
    pub remove_reference: &'static str,
    pub preferences: &'static str,
    pub link_cameras: &'static str,
    pub viewpoints: &'static str,
    pub front: &'static str,
    pub back: &'static str,
    pub left: &'static str,
    pub right: &'static str,
    pub top: &'static str,
    pub three_quarter: &'static str,
    pub frame_sculpt: &'static str,
    pub reset_view: &'static str,
    pub silhouette: &'static str,
    pub ghost: &'static str,
    pub mirror_view: &'static str,
    pub plumb_line: &'static str,
    pub grid: &'static str,
    pub switch_theme: &'static str,
    pub shortcuts: &'static str,
    pub getting_started: &'static str,
    pub report_issue: &'static str,
    pub licences: &'static str,
    pub about: &'static str,
    pub about_text: &'static str,
    pub refs: &'static refs::RefTr,
    pub study: &'static study::StudyTr,
    pub mesh: &'static mesh::MeshTr,
    pub files: &'static files::FilesTr,
    pub video: &'static export::ExportTr,
    pub help: &'static help::HelpTr,
}

pub const JA: Tr = Tr {
    sculpt: "塑像",
    reference: "モチーフ",
    open_reference: "モチーフを開く",
    drop_hint: "スプラット・3Dスキャン・画像をここにドロップ",
    tools: "ツール",
    clay: "クレイ",
    smooth: "スムーズ",
    move_: "ムーブ",
    flatten: "フラッテン",
    inflate: "インフレート",
    crease: "クリース",
    pinch: "ピンチ",
    invert: "逆の動き（押している間）",
    size: "大きさ",
    strength: "強さ",
    symmetry: "左右対称",
    undo: "元に戻す",
    redo: "やり直す",
    new_sphere: "新しい球",
    watch_timelapse: "タイムラプスを見る",
    timelapse: "タイムラプス",
    play: "再生",
    pause: "一時停止",
    back_to_sculpt: "塑像に戻る",
    reference_tools: "モチーフ",
    navigation: "視点操作",
    hint_zbrush: "背景をドラッグ：回転
Alt：移動
右ドラッグ：回転
Ctrl＋右：拡大",
    hint_blender: "中ボタン：回転
Shift＋中：移動
Ctrl＋中：拡大",
    hint_maya: "Alt＋左：回転
Alt＋中：移動
Alt＋右：拡大",
    menu_file: "ファイル",
    menu_edit: "編集",
    menu_view: "表示",
    menu_language: "言語",
    menu_help: "ヘルプ",
    menu_object: "オブジェクト",
    menu_timelapse: "タイムラプス",
    nav_zbrush: "ZBrush 風",
    nav_blender: "Blender 風",
    nav_maya: "Maya 風",
    new_scene: "新規シーン",
    open_scene: "シーンを開く…",
    open_recent: "最近使ったシーン",
    save_scene: "シーンを保存",
    save_scene_as: "名前を付けて保存…",
    revert: "保存した状態に戻す",
    import_reference: "モチーフを読み込む…",
    import_sculpt: "メッシュを読み込む…",
    export: "書き出し",
    exit: "終了",
    mirror_sculpt: "左右を写す（X+ → X−）",
    remove_reference: "モチーフを外す",
    preferences: "環境設定…",
    link_cameras: "視点を連動",
    viewpoints: "視点",
    front: "正面",
    back: "背面",
    left: "左",
    right: "右",
    top: "上",
    three_quarter: "斜め前",
    frame_sculpt: "塑像に合わせる",
    reset_view: "視点をリセット",
    silhouette: "シルエット",
    ghost: "モチーフを重ねる",
    mirror_view: "左右反転表示",
    plumb_line: "垂直線",
    grid: "グリッド",
    switch_theme: "テーマを切り替える",
    shortcuts: "キーボードショートカット",
    getting_started: "はじめに",
    report_issue: "問題を報告",
    licences: "ライセンス",
    about: "SplatStudy について",
    about_text: "3D のモチーフを見ながら塑像するための、デジタル彫刻ツール。",
    refs: &refs::JA,
    study: &study::JA,
    mesh: &mesh::JA,
    files: &files::JA,
    video: &export::JA,
    help: &help::JA,
};

pub const EN: Tr = Tr {
    sculpt: "sculpt",
    reference: "reference",
    open_reference: "open reference",
    drop_hint: "drop a splat, 3D scan or image here",
    tools: "tools",
    clay: "clay",
    smooth: "smooth",
    move_: "move",
    flatten: "flatten",
    inflate: "inflate",
    crease: "crease",
    pinch: "pinch",
    invert: "opposite action (hold)",
    size: "size",
    strength: "strength",
    symmetry: "symmetry",
    undo: "undo",
    redo: "redo",
    new_sphere: "new sphere",
    watch_timelapse: "watch timelapse",
    timelapse: "timelapse",
    play: "play",
    pause: "pause",
    back_to_sculpt: "back to sculpting",
    reference_tools: "reference",
    navigation: "navigation",
    hint_zbrush: "drag the background: turn
Alt: pan
right drag: turn
Ctrl + right: zoom",
    hint_blender: "middle: turn
Shift + middle: pan
Ctrl + middle: zoom",
    hint_maya: "Alt + left: turn
Alt + middle: pan
Alt + right: zoom",
    menu_file: "file",
    menu_edit: "edit",
    menu_view: "view",
    menu_language: "language",
    menu_help: "help",
    menu_object: "object",
    menu_timelapse: "timelapse",
    nav_zbrush: "zbrush style",
    nav_blender: "blender style",
    nav_maya: "maya style",
    new_scene: "new scene",
    open_scene: "open scene…",
    open_recent: "open recent",
    save_scene: "save scene",
    save_scene_as: "save scene as…",
    revert: "revert to saved",
    import_reference: "import reference…",
    import_sculpt: "import mesh…",
    export: "export",
    exit: "exit",
    mirror_sculpt: "mirror sculpt (X+ → X−)",
    remove_reference: "remove reference",
    preferences: "preferences…",
    link_cameras: "link cameras",
    viewpoints: "viewpoints",
    front: "front",
    back: "back",
    left: "left",
    right: "right",
    top: "top",
    three_quarter: "three-quarter",
    frame_sculpt: "frame sculpt",
    reset_view: "reset view",
    silhouette: "silhouette",
    ghost: "ghost overlay",
    mirror_view: "mirror view",
    plumb_line: "plumb line",
    grid: "grid",
    switch_theme: "switch theme",
    shortcuts: "keyboard shortcuts",
    getting_started: "getting started",
    report_issue: "report an issue",
    licences: "licences",
    about: "about SplatStudy",
    about_text: "sculpt from 3D references, the way students study from plaster casts.",
    refs: &refs::EN,
    study: &study::EN,
    mesh: &mesh::EN,
    files: &files::EN,
    video: &export::EN,
    help: &help::EN,
};

pub const FR: Tr = Tr {
    sculpt: "sculpture",
    reference: "modèle",
    open_reference: "ouvrir un modèle",
    drop_hint: "déposez ici un splat, un scan 3D ou une image",
    tools: "outils",
    clay: "argile",
    smooth: "lisser",
    move_: "déplacer",
    flatten: "aplanir",
    inflate: "gonfler",
    crease: "creuser",
    pinch: "pincer",
    invert: "action inverse (maintenir)",
    size: "taille",
    strength: "force",
    symmetry: "symétrie",
    undo: "annuler",
    redo: "rétablir",
    new_sphere: "nouvelle sphère",
    watch_timelapse: "revoir en accéléré",
    timelapse: "accéléré",
    play: "lecture",
    pause: "pause",
    back_to_sculpt: "reprendre la sculpture",
    reference_tools: "modèle",
    navigation: "navigation",
    hint_zbrush: "glisser le fond : tourner
Alt : déplacer
clic droit : tourner
Ctrl + droit : zoomer",
    hint_blender: "milieu : tourner
Maj + milieu : déplacer
Ctrl + milieu : zoomer",
    hint_maya: "Alt + gauche : tourner
Alt + milieu : déplacer
Alt + droit : zoomer",
    menu_file: "fichier",
    menu_edit: "édition",
    menu_view: "affichage",
    menu_language: "langue",
    menu_help: "aide",
    menu_object: "objet",
    menu_timelapse: "accéléré",
    nav_zbrush: "style zbrush",
    nav_blender: "style blender",
    nav_maya: "style maya",
    new_scene: "nouvelle scène",
    open_scene: "ouvrir une scène…",
    open_recent: "scènes récentes",
    save_scene: "enregistrer la scène",
    save_scene_as: "enregistrer sous…",
    revert: "revenir à la version enregistrée",
    import_reference: "importer un modèle…",
    import_sculpt: "importer un maillage…",
    export: "exporter",
    exit: "quitter",
    mirror_sculpt: "symétriser (X+ → X−)",
    remove_reference: "retirer le modèle",
    preferences: "préférences…",
    link_cameras: "lier les caméras",
    viewpoints: "points de vue",
    front: "face",
    back: "dos",
    left: "gauche",
    right: "droite",
    top: "dessus",
    three_quarter: "trois-quarts",
    frame_sculpt: "cadrer la sculpture",
    reset_view: "réinitialiser la vue",
    silhouette: "silhouette",
    ghost: "superposer le modèle",
    mirror_view: "vue miroir",
    plumb_line: "fil à plomb",
    grid: "grille",
    switch_theme: "changer de thème",
    shortcuts: "raccourcis clavier",
    getting_started: "premiers pas",
    report_issue: "signaler un problème",
    licences: "licences",
    about: "à propos de SplatStudy",
    about_text: "sculpter d'après des modèles 3D, comme on étudie d'après le plâtre.",
    refs: &refs::FR,
    study: &study::FR,
    mesh: &mesh::FR,
    files: &files::FR,
    video: &export::FR,
    help: &help::FR,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_pick_the_right_language() {
        assert_eq!(Lang::from_tag("ja-JP"), Lang::Ja);
        assert_eq!(Lang::from_tag("fr-CA"), Lang::Fr);
        assert_eq!(Lang::from_tag("en_US"), Lang::En);
        assert_eq!(Lang::from_tag("de"), Lang::Ja);
    }
}
