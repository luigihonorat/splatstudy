// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Strings for mesh operations: export, import, remesh (voxel and quad),
//! mirror, the mask and the chalk.

pub struct MeshTr {
    /// Heading of the remesh controls in the sculpt panel.
    pub mesh: &'static str,
    pub resolution: &'static str,
    pub remesh: &'static str,
    /// Notice after a remesh.
    pub remeshed: &'static str,
    /// After a number, in the remesh notice.
    pub vertices: &'static str,
    /// Notice when a remesh or a reference left nothing.
    pub nothing_left: &'static str,
    /// After the file name: the clay was replaced by an imported mesh.
    pub imported: &'static str,
    /// After the file name: imported, and remeshed because it was open,
    /// uneven, or too coarse or fine to sculpt.
    pub imported_remeshed: &'static str,
    /// The other way from the shared "mirror sculpt" item (X+ → X−).
    pub mirror_to_positive: &'static str,
    /// Making the sculpt's mesh from the reference.
    pub from_reference: &'static str,
    /// How dense the splats must be to count as material.
    pub threshold: &'static str,
    /// Notice after the mesh was made from the reference.
    pub made_from_reference: &'static str,
    /// Splitting every triangle into four.
    pub subdivide: &'static str,
    /// Notice after a subdivision: a new level.
    pub subdivided: &'static str,
    /// Notice when the clay already has too many vertices to subdivide.
    pub too_dense: &'static str,
    /// The stroke stabiliser slider: the brush trails the pointer.
    pub stabiliser: &'static str,
    /// The stabiliser's value when it is at zero.
    pub off: &'static str,
    /// Heading of the brush falloff choices.
    pub falloff: &'static str,
    /// Falloff shapes: a soft dome, a plateau like a clay tube, and a narrow peak.
    pub falloff_smooth: &'static str,
    pub falloff_flat: &'static str,
    pub falloff_sharp: &'static str,
    /// The mirror across X: its on / off switch, after its label.
    pub switch_on: &'static str,
    pub switch_off: &'static str,
    pub symmetry_x: &'static str,
    /// Heading of how clay builds where a stroke passes again: one layer
    /// per stroke, or more with every pass.
    pub buildup: &'static str,
    pub buildup_layer: &'static str,
    pub buildup_accumulate: &'static str,
    /// Notice when a mesh has too many vertices or triangles to sculpt.
    pub too_heavy: &'static str,
    /// Before "2 / 4": the subdivision level the clay is at, of how many.
    pub level: &'static str,
    /// Going one subdivision level down or up, keeping the detail.
    pub level_down: &'static str,
    pub level_up: &'static str,
    /// "level down" and "level up" side by side in the sculpt panel.
    pub down: &'static str,
    pub up: &'static str,
    /// Dropping the subdivision levels above or below the current one.
    pub delete_higher: &'static str,
    pub delete_lower: &'static str,
    /// Notice when the clay can't be split into levels (a broken triangle).
    pub cannot_subdivide: &'static str,
    /// Rebuilding the clay as even quads that follow its curves.
    pub quad_remesh: &'static str,
    /// The quad remesh's slider: about how many quads to make.
    pub quads: &'static str,
    /// Notice after a quad remesh.
    pub quad_remeshed: &'static str,
    /// Notice when no clean quad mesh came out.
    pub quad_failed: &'static str,
    /// The shortcuts window: hold the key and the tools open in a ring
    /// round the pointer.
    pub tool_pie: &'static str,
    /// The toolbar's tool button's tooltip, under the tool's name.
    pub tool_pie_hint: &'static str,
    /// Headings of the sculpt panel's groups, which fold away: the current
    /// tool's settings (the tool itself is picked on the toolbar), the subdivision
    /// levels, and the remeshes.
    pub tool_settings: &'static str,
    pub subdivision: &'static str,
    pub remeshing: &'static str,
    /// The mask tool: masked clay is held still by every other tool.
    pub mask: &'static str,
    /// Object menu: the masked clay freed and the free clay masked.
    pub invert_mask: &'static str,
    /// Object menu: no clay masked any more.
    pub clear_mask: &'static str,
    /// Object menu: the clay covered in a blue tint, like a carpenter's
    /// chalk line, which sculpting wipes off; and that tint taken off.
    pub add_chalk: &'static str,
    pub remove_chalk: &'static str,
}

pub const JA: MeshTr = MeshTr {
    mesh: "メッシュ",
    resolution: "解像度",
    remesh: "ボクセルリメッシュ",
    remeshed: "リメッシュしました。元に戻すと前のメッシュに戻ります",
    vertices: "頂点",
    nothing_left: "形が残りませんでした",
    imported: "塑像として読み込みました。元に戻すと前の塑像に戻ります",
    imported_remeshed: "塑像として読み込み、彫りやすいようにリメッシュしました",
    mirror_to_positive: "左右を写す（X− → X+）",
    from_reference: "モチーフからメッシュ",
    threshold: "しきい値",
    made_from_reference: "モチーフからメッシュを作りました。元に戻すと前の塑像に戻ります",
    subdivide: "細分化",
    subdivided: "細分化しました。レベルを下げると大きな形を直せます",
    too_dense: "頂点が多すぎるため、これ以上細分化できません",
    stabiliser: "手ぶれ補正",
    off: "オフ",
    falloff: "減衰",
    falloff_smooth: "なめらか",
    falloff_flat: "平ら",
    falloff_sharp: "鋭い",
    switch_on: "オン",
    switch_off: "オフ",
    symmetry_x: "X 軸対称",
    buildup: "盛り方",
    buildup_layer: "一筆ずつ",
    buildup_accumulate: "重ねる",
    too_heavy: "頂点が多すぎて彫刻できません",
    level: "レベル",
    level_down: "レベルを下げる",
    level_up: "レベルを上げる",
    down: "下げる",
    up: "上げる",
    delete_higher: "上のレベルを外す",
    delete_lower: "下のレベルを外す",
    cannot_subdivide: "このメッシュは細分化できません",
    quad_remesh: "クアッドリメッシュ",
    quads: "クアッド数",
    quad_remeshed: "クアッドでリメッシュしました。元に戻すと前のメッシュに戻ります",
    quad_failed: "この形はクアッドリメッシュできませんでした。塑像はそのままです",
    tool_pie: "ツールのパイメニュー（押したまま）",
    tool_pie_hint: "Space を押したままでツールのパイメニュー",
    tool_settings: "ツール設定",
    subdivision: "サブディビジョン",
    remeshing: "リメッシュ",
    mask: "マスク",
    invert_mask: "マスクを反転",
    clear_mask: "マスクを消す",
    add_chalk: "チョークを塗る",
    remove_chalk: "チョークを消す",
};

pub const EN: MeshTr = MeshTr {
    mesh: "mesh",
    resolution: "resolution",
    remesh: "voxel remesh",
    remeshed: "remeshed; undo brings the previous mesh back",
    vertices: "vertices",
    nothing_left: "no shape was left",
    imported: "imported as the sculpt; undo brings the previous one back",
    imported_remeshed: "imported as the sculpt, and remeshed to sculpt on",
    mirror_to_positive: "mirror sculpt (X− → X+)",
    from_reference: "mesh from reference",
    threshold: "threshold",
    made_from_reference: "mesh made from the reference; undo brings the previous sculpt back",
    subdivide: "subdivide",
    subdivided: "subdivided; go a level down to change the broad forms",
    too_dense: "too many vertices to subdivide again",
    stabiliser: "stabiliser",
    off: "off",
    falloff: "falloff",
    falloff_smooth: "smooth",
    falloff_flat: "flat",
    falloff_sharp: "sharp",
    switch_on: "on",
    switch_off: "off",
    symmetry_x: "symmetry x",
    buildup: "build-up",
    buildup_layer: "per stroke",
    buildup_accumulate: "accumulate",
    too_heavy: "too many vertices to sculpt",
    level: "level",
    level_down: "level down",
    level_up: "level up",
    down: "down",
    up: "up",
    delete_higher: "delete higher levels",
    delete_lower: "delete lower levels",
    cannot_subdivide: "this mesh can't be subdivided",
    quad_remesh: "quad remesh",
    quads: "quads",
    quad_remeshed: "quad remeshed; undo brings the previous mesh back",
    quad_failed: "this shape could not be quad remeshed; the sculpt is unchanged",
    tool_pie: "tool pie menu (hold)",
    tool_pie_hint: "hold Space for the tool pie",
    tool_settings: "tool settings",
    subdivision: "subdivision",
    remeshing: "remeshing",
    mask: "mask",
    invert_mask: "invert mask",
    clear_mask: "clear mask",
    add_chalk: "add chalk",
    remove_chalk: "remove chalk",
};

pub const FR: MeshTr = MeshTr {
    mesh: "maillage",
    resolution: "résolution",
    remesh: "remaillage voxel",
    remeshed: "remaillé ; annuler rétablit le maillage précédent",
    vertices: "sommets",
    nothing_left: "aucune forme ne reste",
    imported: "importé comme sculpture ; annuler rétablit la précédente",
    imported_remeshed: "importé comme sculpture et remaillé pour être sculpté",
    mirror_to_positive: "symétriser (X− → X+)",
    from_reference: "maillage du modèle",
    threshold: "seuil",
    made_from_reference: "maillage tiré du modèle ; annuler rétablit la sculpture précédente",
    subdivide: "subdiviser",
    subdivided: "subdivisé ; descendre d'un niveau pour reprendre les grandes formes",
    too_dense: "trop de sommets pour subdiviser encore",
    stabiliser: "stabilisateur",
    off: "aucun",
    falloff: "atténuation",
    falloff_smooth: "douce",
    falloff_flat: "plate",
    falloff_sharp: "vive",
    switch_on: "oui",
    switch_off: "non",
    symmetry_x: "symétrie x",
    buildup: "apport",
    buildup_layer: "par trait",
    buildup_accumulate: "cumulé",
    too_heavy: "trop de sommets pour sculpter",
    level: "niveau",
    level_down: "niveau inférieur",
    level_up: "niveau supérieur",
    down: "descendre",
    up: "monter",
    delete_higher: "supprimer les niveaux supérieurs",
    delete_lower: "supprimer les niveaux inférieurs",
    cannot_subdivide: "ce maillage ne peut pas être subdivisé",
    quad_remesh: "remaillage quad",
    quads: "quads",
    quad_remeshed: "remaillé en quads ; annuler rétablit le maillage précédent",
    quad_failed: "cette forme n'a pas pu être remaillée en quads ; la sculpture est inchangée",
    tool_pie: "menu circulaire des outils (maintenir)",
    tool_pie_hint: "maintenir Espace pour le menu circulaire des outils",
    tool_settings: "réglages de l'outil",
    subdivision: "subdivision",
    remeshing: "remaillage",
    mask: "masque",
    invert_mask: "inverser le masque",
    clear_mask: "effacer le masque",
    add_chalk: "ajouter la craie",
    remove_chalk: "enlever la craie",
};
