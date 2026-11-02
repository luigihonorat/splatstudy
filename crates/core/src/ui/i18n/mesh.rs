// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Strings for mesh operations: export, import, remesh and mirror.

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
    /// Making clay from the reference.
    pub from_reference: &'static str,
    /// How dense the splats must be to count as material.
    pub threshold: &'static str,
    /// Notice after clay was made from the reference.
    pub made_from_reference: &'static str,
    /// Splitting every triangle into four.
    pub subdivide: &'static str,
    /// Notice after a subdivision.
    pub subdivided: &'static str,
    /// Notice when the clay already has too many vertices to subdivide.
    pub too_dense: &'static str,
    /// The stroke stabiliser slider: the brush trails the pointer.
    pub stabiliser: &'static str,
    /// The stabiliser's value when it is at zero.
    pub off: &'static str,
    /// Heading of the brush falloff choices.
    pub falloff: &'static str,
    /// Falloff shapes: a soft dome, a cone, and a narrow peak.
    pub falloff_smooth: &'static str,
    pub falloff_linear: &'static str,
    pub falloff_sharp: &'static str,
    /// Notice when a mesh has too many vertices or triangles to sculpt.
    pub too_heavy: &'static str,
}

pub const JA: MeshTr = MeshTr {
    mesh: "メッシュ",
    resolution: "解像度",
    remesh: "リメッシュ",
    remeshed: "リメッシュしました。元に戻すと前のメッシュに戻ります",
    vertices: "頂点",
    nothing_left: "形が残りませんでした",
    imported: "塑像として読み込みました。元に戻すと前の塑像に戻ります",
    imported_remeshed: "塑像として読み込み、彫りやすいようにリメッシュしました",
    mirror_to_positive: "左右を写す（X− → X+）",
    from_reference: "モチーフから粘土を作る",
    threshold: "しきい値",
    made_from_reference: "モチーフから粘土を作りました。元に戻すと前の塑像に戻ります",
    subdivide: "細分化",
    subdivided: "細分化しました。元に戻すと前のメッシュに戻ります",
    too_dense: "頂点が多すぎるため、これ以上細分化できません",
    stabiliser: "手ぶれ補正",
    off: "オフ",
    falloff: "減衰",
    falloff_smooth: "なめらか",
    falloff_linear: "直線",
    falloff_sharp: "鋭い",
    too_heavy: "頂点が多すぎて彫刻できません",
};

pub const EN: MeshTr = MeshTr {
    mesh: "mesh",
    resolution: "resolution",
    remesh: "remesh",
    remeshed: "remeshed; undo brings the previous mesh back",
    vertices: "vertices",
    nothing_left: "no shape was left",
    imported: "imported as the sculpt; undo brings the previous one back",
    imported_remeshed: "imported as the sculpt, and remeshed to sculpt on",
    mirror_to_positive: "mirror sculpt (X− → X+)",
    from_reference: "clay from reference",
    threshold: "threshold",
    made_from_reference: "clay made from the reference; undo brings the previous sculpt back",
    subdivide: "subdivide",
    subdivided: "subdivided; undo brings the previous mesh back",
    too_dense: "too many vertices to subdivide again",
    stabiliser: "stabiliser",
    off: "off",
    falloff: "falloff",
    falloff_smooth: "smooth",
    falloff_linear: "linear",
    falloff_sharp: "sharp",
    too_heavy: "too many vertices to sculpt",
};

pub const FR: MeshTr = MeshTr {
    mesh: "maillage",
    resolution: "résolution",
    remesh: "remailler",
    remeshed: "remaillé ; annuler rétablit le maillage précédent",
    vertices: "sommets",
    nothing_left: "aucune forme ne reste",
    imported: "importé comme sculpture ; annuler rétablit la précédente",
    imported_remeshed: "importé comme sculpture et remaillé pour être sculpté",
    mirror_to_positive: "symétriser (X− → X+)",
    from_reference: "argile depuis la référence",
    threshold: "seuil",
    made_from_reference: "argile tirée de la référence ; annuler rétablit la sculpture précédente",
    subdivide: "subdiviser",
    subdivided: "subdivisé ; annuler rétablit le maillage précédent",
    too_dense: "trop de sommets pour subdiviser encore",
    stabiliser: "stabilisateur",
    off: "aucun",
    falloff: "atténuation",
    falloff_smooth: "douce",
    falloff_linear: "linéaire",
    falloff_sharp: "vive",
    too_heavy: "trop de sommets pour sculpter",
};
