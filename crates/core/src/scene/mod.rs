// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! `.splatstudy` scene files: the clay, the recorded session that made it,
//! the reference and the views, so a study can be closed and picked up
//! again, and its timelapse still plays.
//!
//! The file is `SPLATSTUDY\0`, a `u32` version, then chunks. Each chunk is a
//! four-letter tag (as a little-endian `u32`), `u32` flags (bit 0: the
//! payload is deflated) and a `u64` payload length. Readers skip chunks
//! they don't know, and read only the fields they know inside a chunk, so
//! newer files still open, minus what is new.
//!
//! - `MESH`: the clay as it is now, in its own vertex order.
//! - `CKPT`: the clay's checkpoints; the first, at log index 0, is where
//!   the timelapse begins (see [`Checkpoint`]).
//! - `LOG_`: every recorded call (see [`Entry`]).
//! - `REF_`: the reference file, kept whole, with its placement and credit.
//! - `VIEW`: the cameras, the divider and the study aids that were on.
//! - `REFH`: in autosaves only, the reference as in `REF_` but with its
//!   file named by a [`BlobKey`] and stored apart, so the file is written
//!   once rather than with every save. Readers from before it skip it and
//!   open the scene without its reference.
//!
//! The log and checkpoints use the encodings next to their types, in
//! `sculpt::record`, so whoever adds an event also adds how it is saved.
//!
//! Reading returns a [`ReadError`] for anything malformed; it never panics.

pub mod autosave;
pub mod blob;
pub mod reference;

use std::sync::Arc;

use crate::camera::OrbitCamera;
use crate::io::{ReadError, Reader, Writer};
use crate::reference::Reference;
use crate::sculpt::record::{read_checkpoints, read_log, write_checkpoints, write_log};
use crate::sculpt::{Checkpoint, Entry, Mesh, Sculpt};

pub use blob::BlobKey;
pub use reference::SavedReference;

const MAGIC: &[u8; 11] = b"SPLATSTUDY\0";
/// Bumped only for changes old readers can't skip over.
pub const VERSION: u32 = 1;

/// Chunk tags, readable in a hex dump.
const fn tag(name: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*name)
}
pub const MESH: u32 = tag(b"MESH");
pub const CHECKPOINTS: u32 = tag(b"CKPT");
pub const LOG: u32 = tag(b"LOG_");
pub const REFERENCE: u32 = tag(b"REF_");
pub const VIEW: u32 = tag(b"VIEW");
pub const REFERENCE_BY_KEY: u32 = tag(b"REFH");
/// The chunks this version reads; others are skipped without inflating.
const KNOWN: [u32; 6] = [MESH, CHECKPOINTS, LOG, REFERENCE, VIEW, REFERENCE_BY_KEY];

const DEFLATED: u32 = 1;
/// Deflate effort: the clay is mostly floats, which barely compress past
/// the fastest level, and saving shouldn't stall the browser.
const DEFLATE_LEVEL: u8 = 1;
/// All deflated chunks of a file together may not grow past this when
/// inflated, so a corrupt or hostile file can't ask for all the memory.
const MAX_INFLATED: usize = 1 << 30;
/// The most clay a scene may hold. Far beyond what sculpting makes, and
/// small enough that the copies made of it and its GPU buffers fit.
const MAX_VERTICES: usize = 4 << 20;
const MAX_TRIANGLES: usize = 8 << 20;

/// A scene as read from a file.
pub struct Scene {
    /// The clay as it was saved.
    pub mesh: Mesh,
    /// The clay at points of the log; the first is where it starts.
    pub checkpoints: Vec<Checkpoint>,
    pub log: Vec<Entry>,
    pub reference: Option<SavedReference>,
    pub view: Option<View>,
    /// The log or checkpoints were damaged and left out: the clay opens
    /// as it is, without its timelapse.
    pub history_damaged: bool,
    /// The reference was damaged and left out.
    pub reference_damaged: bool,
}

/// How the two views were looking.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Sculpt view, then reference view.
    pub cameras: [OrbitCamera; 2],
    /// Share of the width given to the sculpt view.
    pub split: f32,
    /// Absent in files from before the study aids.
    pub study: Option<StudyView>,
}

/// The study aids that were on, as plain values the app maps back. The
/// pose timer isn't kept: a reopened scene starts a new sitting.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StudyView {
    /// 0: both views, 1: the sculpt alone, 2: the reference alone.
    pub layout: u8,
    pub swapped: bool,
    pub link: bool,
    pub mirror: bool,
    pub plumb: bool,
    /// Where each plumb line hangs, as a share of its view's width.
    pub plumb_x: [f32; 2],
    pub grid: bool,
    pub grid_divisions: u32,
    pub silhouette: bool,
    pub ghost: bool,
}

/// Writes a scene chunk by chunk.
pub struct SceneWriter {
    out: Writer,
    /// The clay, checkpoints and log are deflated.
    pack: bool,
}

impl Default for SceneWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneWriter {
    /// A scene file to keep or share: the clay and its history deflated.
    pub fn new() -> Self {
        let mut out = Writer::new();
        out.raw(MAGIC).u32(VERSION);
        Self { out, pack: true }
    }

    /// A scene stored as it is, nothing deflated: for autosaves, which
    /// are written every few seconds to storage that doesn't need them
    /// small. Deflating a long history stalls the app for a moment.
    pub fn stored() -> Self {
        Self {
            pack: false,
            ..Self::new()
        }
    }

    /// Appends a chunk, deflating the payload if asked.
    pub fn chunk(&mut self, tag: u32, deflate: bool, payload: &[u8]) -> &mut Self {
        if deflate {
            let packed = miniz_oxide::deflate::compress_to_vec(payload, DEFLATE_LEVEL);
            self.out
                .u32(tag)
                .u32(DEFLATED)
                .len(packed.len())
                .raw(&packed);
        } else {
            self.out.u32(tag).u32(0).len(payload.len()).raw(payload);
        }
        self
    }

    /// The clay as it is now.
    pub fn mesh(&mut self, mesh: &Mesh) -> &mut Self {
        let mut w = Writer::new();
        write_mesh(&mut w, mesh);
        self.chunk(MESH, self.pack, &w.finish())
    }

    /// The clay's checkpoints, the first being the start of the log.
    pub fn checkpoints(&mut self, checkpoints: &[Checkpoint]) -> &mut Self {
        let mut w = Writer::new();
        write_checkpoints(&mut w, checkpoints);
        self.chunk(CHECKPOINTS, self.pack, &w.finish())
    }

    pub fn log(&mut self, entries: &[Entry]) -> &mut Self {
        let mut w = Writer::new();
        write_log(&mut w, entries);
        self.chunk(LOG, self.pack, &w.finish())
    }

    /// The checkpoints and log of `sculpt`, as [`SceneWriter::checkpoints`]
    /// and [`SceneWriter::log`] write them, encoding only what was added
    /// since `cache` last saw this sculpt.
    pub fn history(&mut self, sculpt: &Sculpt, cache: &mut HistoryCache) -> &mut Self {
        cache.update(sculpt);
        let pack = self.pack;
        self.chunk(CHECKPOINTS, pack, &cache.checkpoints.bytes)
            .chunk(LOG, pack, &cache.log.bytes)
    }

    /// The reference file is stored as it came: most formats are packed
    /// already, and big ones would take long to deflate.
    pub fn reference(&mut self, reference: &Reference) -> &mut Self {
        let mut w = Writer::new();
        reference::write_reference(&mut w, reference);
        self.chunk(REFERENCE, false, &w.finish())
    }

    /// The reference with its file left out, named by `key`; whoever reads
    /// the scene has to find the file under that key.
    pub fn reference_by_key(&mut self, reference: &Reference, key: BlobKey) -> &mut Self {
        let mut w = Writer::new();
        reference::write_reference_by_key(&mut w, reference, key);
        self.chunk(REFERENCE_BY_KEY, false, &w.finish())
    }

    pub fn view(&mut self, view: &View) -> &mut Self {
        let mut w = Writer::new();
        for camera in &view.cameras {
            w.vec3(camera.target)
                .f32(camera.yaw)
                .f32(camera.pitch)
                .f32(camera.distance)
                .f32(camera.fov_y);
        }
        w.f32(view.split);
        if let Some(study) = &view.study {
            let flags = [
                study.swapped,
                study.link,
                study.mirror,
                study.plumb,
                study.grid,
                study.silhouette,
                study.ghost,
            ]
            .iter()
            .enumerate()
            .fold(0u8, |bits, (i, &on)| bits | u8::from(on) << i);
            w.u8(flags)
                .u8(study.layout)
                .f32(study.plumb_x[0])
                .f32(study.plumb_x[1])
                .u32(study.grid_divisions);
        }
        self.chunk(VIEW, false, &w.finish())
    }

    pub fn finish(self) -> Vec<u8> {
        self.out.finish()
    }
}

/// A sculpt's checkpoints and log as saved, kept from one save to the
/// next. Both only ever grow at the end, so a save encodes what is new and
/// copies the rest, instead of encoding every whole mesh again.
#[derive(Default)]
pub struct HistoryCache {
    /// The sculpt they belong to (see [`Sculpt::id`]).
    sculpt: Option<u64>,
    checkpoints: Encoded,
    log: Encoded,
}

impl HistoryCache {
    /// Brings the encodings up to date with `sculpt`.
    fn update(&mut self, sculpt: &Sculpt) {
        if self.sculpt != Some(sculpt.id()) {
            *self = Self {
                sculpt: Some(sculpt.id()),
                ..Self::default()
            };
        }
        self.checkpoints
            .extend(sculpt.checkpoints(), |c, w| c.write(w));
        self.log.extend(sculpt.log(), |e, w| e.write(w));
    }
}

/// A list as written: its length, then each item.
struct Encoded {
    /// How many items are in `bytes`.
    count: usize,
    bytes: Vec<u8>,
}

impl Default for Encoded {
    fn default() -> Self {
        let mut w = Writer::new();
        w.len(0);
        Self {
            count: 0,
            bytes: w.finish(),
        }
    }
}

impl Encoded {
    /// Encodes the items past those already in, and the new length.
    fn extend<T>(&mut self, items: &[T], write: impl Fn(&T, &mut Writer)) {
        if items.len() < self.count {
            // Not the list it was: start over.
            *self = Self::default();
        }
        if items.len() == self.count {
            return;
        }
        let mut w = Writer::new();
        for item in &items[self.count..] {
            write(item, &mut w);
        }
        self.bytes.extend_from_slice(&w.finish());
        self.count = items.len();
        let mut w = Writer::new();
        w.len(self.count);
        let len = w.finish();
        self.bytes[..len.len()].copy_from_slice(&len);
    }
}

/// One chunk of a file, its payload inflated.
pub struct Chunk {
    pub tag: u32,
    pub payload: Vec<u8>,
}

/// Splits a file into the chunks this version reads, after checking the
/// header. Other chunks are skipped without inflating them.
pub fn chunks(data: &[u8]) -> Result<Vec<Chunk>, ReadError> {
    chunks_of(data, &KNOWN, MAX_INFLATED)
}

/// The chunks with one of the `wanted` tags, inflated; the rest are only
/// stepped over.
fn chunks_of(data: &[u8], wanted: &[u32], max: usize) -> Result<Vec<Chunk>, ReadError> {
    let mut r = Reader::new(data);
    if r.take(MAGIC.len()).ok() != Some(&MAGIC[..]) {
        return Err(ReadError::Invalid {
            offset: 0,
            what: "not a SplatStudy scene",
        });
    }
    let version = r.u32()?;
    if version == 0 || version > VERSION {
        return Err(r.invalid("a scene from a newer SplatStudy"));
    }
    let mut chunks = Vec::new();
    let mut inflated = 0;
    while !r.is_empty() {
        let tag = r.u32()?;
        let flags = r.u32()?;
        let start = r.position();
        let payload = r.bytes()?;
        if !wanted.contains(&tag) {
            continue;
        }
        let payload = if flags & DEFLATED != 0 {
            let limit = max - inflated;
            let payload = miniz_oxide::inflate::decompress_to_vec_with_limit(payload, limit)
                .map_err(|e| ReadError::Invalid {
                    offset: start,
                    what: match e.status {
                        miniz_oxide::inflate::TINFLStatus::HasMoreOutput => "a scene too big",
                        _ => "damaged compressed data",
                    },
                })?;
            inflated += payload.len();
            payload
        } else {
            payload.to_vec()
        };
        chunks.push(Chunk { tag, payload });
    }
    Ok(chunks)
}

/// The key of the reference file a scene names but doesn't hold (an
/// autosave's `REFH` chunk), so the file can be fetched before reading.
pub fn reference_key(data: &[u8]) -> Result<Option<BlobKey>, ReadError> {
    let mut key = None;
    for chunk in chunks_of(data, &[REFERENCE, REFERENCE_BY_KEY], MAX_INFLATED)? {
        match chunk.tag {
            REFERENCE_BY_KEY => key = Some(reference::key_in(&chunk.payload)?),
            REFERENCE => key = None,
            _ => {}
        }
    }
    Ok(key)
}

impl Scene {
    /// Reads a whole scene file.
    pub fn read(data: &[u8]) -> Result<Scene, ReadError> {
        Self::read_with(data, |_| None)
    }

    /// Reads a scene whose reference file may be stored apart: `find`
    /// gives the file for a key, or `None`, and then the scene opens
    /// without its reference.
    pub fn read_with(
        data: &[u8],
        mut find: impl FnMut(BlobKey) -> Option<Arc<[u8]>>,
    ) -> Result<Scene, ReadError> {
        let mut mesh = None;
        let mut checkpoints = Vec::new();
        let mut log = Vec::new();
        let mut reference = None;
        let mut view = None;
        let mut history_damaged = false;
        let mut reference_damaged = false;
        // A damaged part is logged and left out; only damaged clay (or
        // a damaged file) loses the scene.
        let damaged = |what: &str, err: ReadError| log::warn!("scene {what}: {err}");
        for chunk in chunks(data)? {
            let mut r = Reader::new(&chunk.payload);
            match chunk.tag {
                MESH => mesh = Some(read_mesh(&mut r)?),
                CHECKPOINTS => match read_checkpoints(&mut r) {
                    Ok(read) => checkpoints = read,
                    Err(err) => {
                        damaged("checkpoints", err);
                        history_damaged = true;
                    }
                },
                LOG => match read_log(&mut r) {
                    Ok(read) => log = read,
                    Err(err) => {
                        damaged("log", err);
                        history_damaged = true;
                    }
                },
                REFERENCE => match reference::read_reference(&mut r) {
                    Ok(read) => reference = Some(read),
                    Err(err) => {
                        damaged("reference", err);
                        reference_damaged = true;
                    }
                },
                REFERENCE_BY_KEY => match reference::read_reference_by_key(&mut r, &mut find) {
                    Ok(read) => reference = read,
                    Err(err) => {
                        damaged("reference", err);
                        reference_damaged = true;
                    }
                },
                VIEW => view = Some(read_view(&mut r)?),
                _ => {}
            }
        }
        let mesh = mesh.ok_or(ReadError::Invalid {
            offset: data.len(),
            what: "a scene without clay",
        })?;
        if history_damaged {
            // Half a history can't be replayed; the clay itself is fine.
            checkpoints.clear();
            log.clear();
        }
        if reference_damaged {
            reference = None;
        }
        Ok(Scene {
            mesh,
            checkpoints,
            log,
            reference,
            view,
            history_damaged,
            reference_damaged,
        })
    }
}

fn write_mesh(w: &mut Writer, mesh: &Mesh) {
    w.len(mesh.positions.len());
    for &p in &mesh.positions {
        w.vec3(p);
    }
    w.len(mesh.triangles.len());
    for t in &mesh.triangles {
        w.u32(t[0]).u32(t[1]).u32(t[2]);
    }
}

/// Reads a mesh in its saved order, refusing indices out of range and
/// triangles with a corner twice (either would panic later), and empty
/// meshes (nothing to sculpt).
fn read_mesh(r: &mut Reader) -> Result<Mesh, ReadError> {
    let n = r.len(12)?;
    if n > MAX_VERTICES {
        return Err(r.invalid("clay with too many vertices"));
    }
    let mut positions = Vec::with_capacity(n);
    for _ in 0..n {
        let p = r.vec3()?;
        if !p.is_finite() {
            return Err(r.invalid("a vertex that isn't a number"));
        }
        positions.push(p);
    }
    let m = r.len(12)?;
    if m > MAX_TRIANGLES {
        return Err(r.invalid("clay with too many triangles"));
    }
    let mut triangles = Vec::with_capacity(m);
    for _ in 0..m {
        let t = [r.u32()?, r.u32()?, r.u32()?];
        if t.iter().any(|&i| i as usize >= n) {
            return Err(r.invalid("a triangle with a vertex that isn't there"));
        }
        if t[0] == t[1] || t[1] == t[2] || t[2] == t[0] {
            return Err(r.invalid("a triangle with a corner twice"));
        }
        triangles.push(t);
    }
    if triangles.is_empty() {
        return Err(r.invalid("a mesh without triangles"));
    }
    Ok(Mesh::from_ordered(positions, triangles))
}

fn read_view(r: &mut Reader) -> Result<View, ReadError> {
    let mut cameras = [OrbitCamera::default(); 2];
    for camera in &mut cameras {
        let target = r.vec3()?;
        let (yaw, pitch, distance, fov_y) = (r.f32()?, r.f32()?, r.f32()?, r.f32()?);
        let ok = target.is_finite()
            && [yaw, pitch, distance, fov_y].iter().all(|v| v.is_finite())
            && distance > 0.0
            && fov_y > 0.0
            && fov_y < std::f32::consts::PI;
        if !ok {
            return Err(r.invalid("a camera that can't be"));
        }
        *camera = OrbitCamera {
            target,
            yaw,
            pitch: pitch.clamp(-1.55, 1.55),
            distance,
            fov_y,
        };
    }
    let split = r.f32()?;
    let split = if split.is_finite() {
        split.clamp(0.2, 0.8)
    } else {
        0.5
    };
    let study = if r.is_empty() {
        None
    } else {
        let flags = r.u8()?;
        let on = |i: u32| flags & (1 << i) != 0;
        let layout = r.u8()?;
        let share = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.5
            }
        };
        let plumb_x = [share(r.f32()?), share(r.f32()?)];
        let grid_divisions = r.u32()?.clamp(1, 64);
        Some(StudyView {
            layout: if layout <= 2 { layout } else { 0 },
            swapped: on(0),
            link: on(1),
            mirror: on(2),
            plumb: on(3),
            plumb_x,
            grid: on(4),
            grid_divisions,
            silhouette: on(5),
            ghost: on(6),
        })
    };
    Ok(View {
        cameras,
        split,
        study,
    })
}

/// Vertex positions as bytes, for comparing meshes bit for bit.
#[cfg(test)]
pub(crate) fn position_bits(positions: &[glam::Vec3]) -> Vec<u32> {
    positions
        .iter()
        .flat_map(|p| p.to_array().map(f32::to_bits))
        .collect()
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::sculpt::{Brush, Dab, Replay, Sculpt};

    /// A short session on a small sphere: a few strokes, an undo.
    fn session() -> Sculpt {
        let mut s = Sculpt::new(Mesh::icosphere(3));
        for k in 0..3 {
            s.set_clock(1.0 + k as f64);
            s.begin_stroke();
            for i in 0..5 {
                s.dab(Dab {
                    brush: [Brush::Clay, Brush::Inflate, Brush::Crease][k],
                    centre: Vec3::new(0.1 * i as f32, 0.2, 1.0).normalize(),
                    radius: 0.3,
                    strength: 0.6,
                    invert: k == 1,
                    symmetry: true,
                    direction: Vec3::X,
                });
                s.refresh();
            }
            s.end_stroke();
        }
        s.undo();
        s
    }

    fn save(s: &Sculpt, view: &View) -> Vec<u8> {
        let mut w = SceneWriter::new();
        w.mesh(&s.mesh)
            .checkpoints(s.checkpoints())
            .log(s.log())
            .view(view);
        w.finish()
    }

    fn view() -> View {
        let mut turned = OrbitCamera::default();
        turned.orbit(40.0, -10.0);
        turned.dolly(0.8);
        View {
            cameras: [turned, OrbitCamera::default()],
            split: 0.4,
            study: Some(StudyView {
                layout: 2,
                swapped: true,
                link: false,
                mirror: true,
                plumb: true,
                plumb_x: [0.25, 0.75],
                grid: false,
                grid_divisions: 6,
                silhouette: true,
                ghost: false,
            }),
        }
    }

    #[test]
    fn a_scene_round_trips_and_its_log_replays_to_the_saved_clay() {
        let live = session();
        let view = view();
        let data = save(&live, &view);
        let scene = Scene::read(&data).expect("reads back");

        assert_eq!(
            position_bits(&scene.mesh.positions),
            position_bits(&live.mesh.positions)
        );
        assert_eq!(scene.mesh.triangles, live.mesh.triangles);
        assert_eq!(scene.view, Some(view));
        assert!(scene.reference.is_none());
        assert_eq!(scene.checkpoints, live.checkpoints());
        assert_eq!(scene.checkpoints[0].log_index, 0);

        // Replaying the saved log from the saved checkpoints gives the clay.
        let mut replay = Replay::new(scene.checkpoints.clone(), scene.log.clone());
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_eq!(
            position_bits(&replay.sculpt.mesh.positions),
            position_bits(&live.mesh.positions)
        );
        // The sculpt the app makes of it saves to the same bytes.
        let resumed = Sculpt::with_history(scene.mesh, scene.log, scene.checkpoints);
        assert_eq!(save(&resumed, &view), data);
    }

    #[test]
    fn a_scene_with_a_new_mesh_mid_session_replays_through_it() {
        let mut live = session();
        // A remesh or an import: other triangles, then more strokes.
        live.set_clock(10.0);
        live.replace_mesh(Mesh::icosphere(2));
        live.set_clock(11.0);
        live.begin_stroke();
        live.dab(Dab {
            brush: Brush::Inflate,
            centre: Vec3::Z,
            radius: 0.5,
            strength: 1.0,
            invert: false,
            symmetry: true,
            direction: Vec3::ZERO,
        });
        live.end_stroke();
        live.refresh();
        let data = save(&live, &view());
        let scene = Scene::read(&data).unwrap();
        assert_eq!(scene.mesh.triangles, live.mesh.triangles);
        let mut replay = Replay::new(scene.checkpoints.clone(), scene.log.clone());
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_eq!(replay.sculpt.mesh.triangles, live.mesh.triangles);
        assert_eq!(
            position_bits(&replay.sculpt.mesh.positions),
            position_bits(&live.mesh.positions)
        );
        let resumed = Sculpt::with_history(scene.mesh, scene.log, scene.checkpoints);
        assert_eq!(save(&resumed, &view()), data);
    }

    #[test]
    fn an_autosave_names_its_reference_file_and_finds_it_again() {
        use crate::reference::{Content, RefImage, Source};

        let live = session();
        let file: Arc<[u8]> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let mut original = Reference::new(
            Source {
                name: "cast.png".into(),
                bytes: file.clone(),
            },
            Content::Image(RefImage::default()),
        );
        original.align.yaw = 0.5;
        let key = BlobKey::of(&file);
        let mut w = SceneWriter::new();
        w.mesh(&live.mesh)
            .checkpoints(live.checkpoints())
            .log(live.log())
            .reference_by_key(&original, key)
            .view(&view());
        let data = w.finish();
        assert!(
            !data.windows(file.len()).any(|w| w == &file[..]),
            "the file isn't in the scene"
        );
        assert_eq!(reference_key(&data), Ok(Some(key)));

        let mut asked = Vec::new();
        let scene = Scene::read_with(&data, |k| {
            asked.push(k);
            Some(file.clone())
        })
        .expect("reads");
        assert_eq!(asked, [key]);
        let reference = scene.reference.expect("the reference came back");
        assert_eq!(reference.source.bytes, file);
        assert_eq!(reference.source.name, "cast.png");
        assert_eq!(reference.align, original.align);
        assert_eq!(scene.log, live.log());

        // Without the file the clay still opens.
        let scene = Scene::read(&data).expect("reads");
        assert!(scene.reference.is_none());
        assert_eq!(scene.log, live.log());

        // A scene with its file inside names no key.
        let mut w = SceneWriter::new();
        w.mesh(&live.mesh).reference(&original);
        assert_eq!(reference_key(&w.finish()), Ok(None));
        assert_eq!(reference_key(&save(&live, &view())), Ok(None));
        assert!(reference_key(b"junk").is_err());
    }

    #[test]
    fn the_history_cache_writes_what_a_whole_encoding_writes() {
        let whole = |s: &Sculpt| {
            let mut w = SceneWriter::new();
            w.checkpoints(s.checkpoints()).log(s.log());
            w.finish()
        };
        let cached = |s: &Sculpt, cache: &mut HistoryCache| {
            let mut w = SceneWriter::new();
            w.history(s, cache);
            w.finish()
        };
        let mut cache = HistoryCache::default();
        let mut live = Sculpt::new(Mesh::icosphere(2));
        assert_eq!(cached(&live, &mut cache), whole(&live), "empty log");
        for k in 0..3 {
            live.set_clock(k as f64);
            live.begin_stroke();
            live.dab(Dab {
                brush: Brush::Inflate,
                centre: Vec3::new(0.2 * k as f32, 0.1, 1.0).normalize(),
                radius: 0.4,
                strength: 0.5,
                invert: false,
                symmetry: false,
                direction: Vec3::ZERO,
            });
            live.end_stroke();
            live.refresh();
            assert_eq!(cached(&live, &mut cache), whole(&live), "stroke {k}");
        }
        live.replace_mesh(Mesh::icosphere(1));
        assert_eq!(cached(&live, &mut cache), whole(&live), "a new mesh");
        // Another sculpt, even one as long, is encoded afresh.
        let other = session();
        assert_eq!(cached(&other, &mut cache), whole(&other), "another sculpt");
        assert_eq!(cached(&live, &mut cache), whole(&live), "and back");
    }

    #[test]
    fn an_autosave_is_stored_undeflated_and_reads_the_same() {
        let live = session();
        let mut cache = HistoryCache::default();
        let mut w = SceneWriter::stored();
        w.mesh(&live.mesh).history(&live, &mut cache).view(&view());
        let stored = w.finish();
        let mut r = Reader::new(&stored[MAGIC.len() + 4..]);
        while !r.is_empty() {
            let _tag = r.u32().unwrap();
            assert_eq!(r.u32().unwrap() & DEFLATED, 0, "nothing deflated");
            r.bytes().unwrap();
        }
        let packed = save(&live, &view());
        assert!(packed.len() < stored.len());
        let (a, b) = (Scene::read(&stored).unwrap(), Scene::read(&packed).unwrap());
        assert_eq!(
            position_bits(&a.mesh.positions),
            position_bits(&b.mesh.positions)
        );
        assert_eq!(a.mesh.triangles, b.mesh.triangles);
        assert_eq!(
            (a.checkpoints, a.log, a.view),
            (b.checkpoints, b.log, b.view)
        );
    }

    #[test]
    fn a_view_without_study_aids_still_reads() {
        let mut v = view();
        v.study = None;
        let mut w = SceneWriter::new();
        w.mesh(&Mesh::icosphere(1)).view(&v);
        assert_eq!(Scene::read(&w.finish()).unwrap().view, Some(v));
    }

    #[test]
    fn unknown_chunks_are_skipped() {
        let live = session();
        let mut w = SceneWriter::new();
        w.chunk(tag(b"ZZZZ"), false, b"from the future")
            .mesh(&live.mesh)
            .chunk(tag(b"YYYY"), true, &[7; 100])
            .checkpoints(live.checkpoints());
        let scene = Scene::read(&w.finish()).expect("reads");
        assert_eq!(scene.mesh.positions.len(), live.mesh.positions.len());
        assert!(scene.log.is_empty());
        assert!(scene.view.is_none());
    }

    #[test]
    fn a_damaged_part_is_left_out_and_the_clay_still_opens() {
        let live = session();
        let mut w = SceneWriter::new();
        w.mesh(&live.mesh)
            .checkpoints(live.checkpoints())
            .chunk(LOG, true, b"not a log")
            .chunk(REFERENCE, false, b"not a reference")
            .view(&view());
        let scene = Scene::read(&w.finish()).expect("the clay opens");
        assert_eq!(scene.mesh.triangles, live.mesh.triangles);
        assert!(scene.history_damaged && scene.reference_damaged);
        // Half a history is no history.
        assert!(scene.log.is_empty() && scene.checkpoints.is_empty());
        assert!(scene.reference.is_none());
        assert_eq!(scene.view, Some(view()));

        let scene = Scene::read(&save(&live, &view())).unwrap();
        assert!(!scene.history_damaged && !scene.reference_damaged);
    }

    #[test]
    fn unknown_chunks_are_never_inflated() {
        // Marked deflated but not deflate data: reading it would fail.
        let mut w = SceneWriter::new();
        w.mesh(&Mesh::icosphere(1));
        w.out.u32(tag(b"XXXX")).u32(DEFLATED).len(4).raw(&[0xff; 4]);
        assert!(Scene::read(&w.finish()).is_ok());
    }

    #[test]
    fn inflating_stops_at_a_total_for_the_whole_file() {
        let mut w = SceneWriter::new();
        w.chunk(LOG, true, &[0; 600]).chunk(VIEW, true, &[0; 600]);
        let data = w.finish();
        assert_eq!(chunks_of(&data, &KNOWN, 2000).map(|c| c.len()), Ok(2));
        // Each chunk alone fits; both together don't.
        assert!(chunks_of(&data, &KNOWN, 1000).is_err());
        // Chunks left out don't count.
        assert!(chunks_of(&data, &[LOG], 1000).is_ok());
    }

    #[test]
    fn every_truncation_and_corruption_is_an_error_not_a_panic() {
        let live = session();
        let data = save(&live, &view());
        // Cutting anywhere fails cleanly (a cut on a chunk boundary still
        // leaves the clay or loses it, and either is fine as long as no
        // panic happens). Step through the header byte by byte, the rest
        // in strides to keep the test quick.
        let ends = (0..64).chain((64..data.len()).step_by(97));
        for end in ends {
            let _ = Scene::read(&data[..end]);
        }
        assert!(Scene::read(&data[..data.len() - 1]).is_err());
        assert!(Scene::read(b"SPLATSTUDY").is_err());
        assert!(Scene::read(b"").is_err());
        // Flipped bytes: an error or a scene, never a panic.
        let mut seed = 12345u32;
        for _ in 0..300 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let mut bad = data.clone();
            let at = (seed as usize >> 4) % bad.len();
            bad[at] ^= (seed >> 24) as u8 | 1;
            // What still reads must also replay without a panic.
            if let Ok(scene) = Scene::read(&bad) {
                let mut replay = Replay::new(scene.checkpoints, scene.log);
                replay.seek(f64::INFINITY, usize::MAX);
            }
        }
    }

    #[test]
    fn bad_meshes_are_refused() {
        // A triangle pointing past the vertices.
        let mut w = Writer::new();
        w.len(3);
        for p in [Vec3::ZERO, Vec3::X, Vec3::Y] {
            w.vec3(p);
        }
        w.len(1).u32(0).u32(1).u32(3);
        let mut scene = SceneWriter::new();
        scene.chunk(MESH, false, &w.finish());
        assert!(Scene::read(&scene.finish()).is_err());
        // A triangle with a corner twice: subdividing it would panic.
        let mut w = Writer::new();
        w.len(3);
        for p in [Vec3::ZERO, Vec3::X, Vec3::Y] {
            w.vec3(p);
        }
        w.len(2).u32(0).u32(1).u32(2).u32(0).u32(2).u32(2);
        let mut scene = SceneWriter::new();
        scene.chunk(MESH, false, &w.finish());
        assert!(Scene::read(&scene.finish()).is_err());
        // No clay at all.
        assert!(Scene::read(&SceneWriter::new().finish()).is_err());
        // A newer version.
        let mut data = SceneWriter::new().finish();
        data[MAGIC.len()] = 99;
        assert!(Scene::read(&data).is_err());
    }
}
