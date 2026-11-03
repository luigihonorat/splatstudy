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
//! - `LEVL`: the clay's subdivision levels, if it has any, and those of
//!   the checkpoints that kept some (see [`Levels::write`]). Readers from
//!   before it open the clay at the level it was saved at, without the
//!   others; they can't read a log with level changes either, so they
//!   open it without its timelapse.
//! - `QUAD`: which triangles of the clay pair up into quads, and of the
//!   checkpoints that had quads, as
//!   [diagonal codes](crate::sculpt::mesh::diagonals). Readers from before
//!   it open the clay as triangles. Codes that don't fit their clay are
//!   dropped, and the clay opens as triangles.
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
use crate::sculpt::{Checkpoint, Entry, Levels, Mesh, Sculpt};

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
pub const LEVELS: u32 = tag(b"LEVL");
pub const QUADS: u32 = tag(b"QUAD");
/// The chunks this version reads; others are skipped without inflating.
const KNOWN: [u32; 8] = [
    MESH,
    CHECKPOINTS,
    LOG,
    REFERENCE,
    VIEW,
    REFERENCE_BY_KEY,
    LEVELS,
    QUADS,
];

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
    /// The clay's other subdivision levels, if it has any.
    pub levels: Option<Levels>,
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

    /// Appends a chunk that `write` fills, about `size` bytes long.
    /// Stored as it is, it goes straight into the file rather than
    /// through a copy: a fine clay's chunks are tens of megabytes.
    fn chunk_with(&mut self, tag: u32, size: usize, write: impl FnOnce(&mut Writer)) -> &mut Self {
        if self.pack {
            let mut w = Writer::new();
            w.reserve(size);
            write(&mut w);
            return self.chunk(tag, true, &w.finish());
        }
        self.out.reserve(size + 16).u32(tag).u32(0);
        let at = self.out.position();
        self.out.len(0);
        write(&mut self.out);
        let len = self.out.position() - at - 8;
        self.out.set_u64(at, len as u64);
        self
    }

    /// The clay as it is now.
    pub fn mesh(&mut self, mesh: &Mesh) -> &mut Self {
        let size = 16 + 12 * (mesh.positions.len() + mesh.triangles.len());
        self.chunk_with(MESH, size, |w| write_mesh(w, mesh))
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

    /// The clay's subdivision levels and those its checkpoints kept; no
    /// chunk at all when there are none.
    pub fn levels(&mut self, sculpt: &Sculpt) -> &mut Self {
        let kept: Vec<(usize, &Checkpoint, &Levels)> = sculpt
            .checkpoints()
            .iter()
            .enumerate()
            .filter_map(|(i, c)| Some((i, c, c.levels.as_deref()?)))
            .collect();
        if sculpt.level_data().is_none() && kept.is_empty() {
            return self;
        }
        let size = sculpt.level_data().map_or(0, Levels::memory)
            + kept.iter().map(|(_, _, l)| l.memory()).sum::<usize>();
        self.chunk_with(LEVELS, size, |w| {
            match sculpt.level_data() {
                Some(levels) => {
                    w.u8(1);
                    levels.write(w, &sculpt.mesh.positions);
                }
                None => {
                    w.u8(0);
                }
            }
            w.len(kept.len());
            for (i, checkpoint, levels) in kept {
                w.u32(i as u32);
                levels.write(w, &checkpoint.positions);
            }
        })
    }

    /// The quads of the clay and of its checkpoints; no chunk at all when
    /// there are none.
    pub fn quads(&mut self, sculpt: &Sculpt) -> &mut Self {
        let kept: Vec<(usize, &[u8])> = sculpt
            .checkpoints()
            .iter()
            .enumerate()
            .filter_map(|(i, c)| Some((i, c.diagonals.as_deref()?)))
            .collect();
        let live = sculpt.mesh.diagonals();
        if live.is_none() && kept.is_empty() {
            return self;
        }
        let mut w = Writer::new();
        match &live {
            Some(codes) => w.u8(1).bytes(codes),
            None => w.u8(0),
        };
        w.len(kept.len());
        for (i, codes) in kept {
            w.u32(i as u32).bytes(codes);
        }
        self.chunk(QUADS, self.pack, &w.finish())
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
        let mut levels = None;
        let mut quads = None;
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
                LEVELS => levels = Some(chunk.payload),
                QUADS => quads = Some(chunk.payload),
                _ => {}
            }
        }
        let mut mesh = mesh.ok_or(ReadError::Invalid {
            offset: data.len(),
            what: "a scene without clay",
        })?;
        if let Some(payload) = quads
            && let Err(err) = read_quads(&payload, &mut mesh, &mut checkpoints)
        {
            // Only the quads are lost: the clay and its history are whole.
            damaged("quads", err);
        }
        let levels = match levels {
            Some(payload) => match read_levels(&payload, &mesh, &mut checkpoints) {
                Ok(levels) => levels,
                Err(err) => {
                    // The clay opens at the level it was saved at; a history
                    // through levels can't be replayed without them.
                    damaged("levels", err);
                    history_damaged = true;
                    None
                }
            },
            None => None,
        };
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
            levels,
            checkpoints,
            log,
            reference,
            view,
            history_damaged,
            reference_damaged,
        })
    }
}

/// Reads a `LEVL` chunk: the clay's levels, and those of the checkpoints,
/// which it puts in place. Levels that don't fit their clay are an error.
fn read_levels(
    payload: &[u8],
    mesh: &Mesh,
    checkpoints: &mut [Checkpoint],
) -> Result<Option<Levels>, ReadError> {
    let mut r = Reader::new(payload);
    let levels = match r.u8()? {
        0 => None,
        1 => Some(Levels::read(&mut r, &mesh.positions, &mesh.triangles)?),
        _ => return Err(r.invalid("unknown kind of levels")),
    };
    let kept = r.len(5)?;
    for _ in 0..kept {
        let i = r.u32()? as usize;
        let checkpoint = checkpoints
            .get_mut(i)
            .filter(|c| c.has_mesh())
            .ok_or_else(|| r.invalid("levels for a checkpoint that has no mesh"))?;
        let triangles = checkpoint.triangles.clone().unwrap_or_default();
        let read = Levels::read(&mut r, &checkpoint.positions, &triangles)?;
        checkpoint.levels = Some(Arc::new(read));
    }
    Ok(levels)
}

/// Reads a `QUAD` chunk into the clay and its checkpoints. The clay's
/// codes are checked against its triangles now; a checkpoint's when its
/// mesh is built (see [`Checkpoint::diagonals`]), but they must already be
/// one per triangle. Nothing is put in place unless the whole chunk fits.
fn read_quads(
    payload: &[u8],
    mesh: &mut Mesh,
    checkpoints: &mut [Checkpoint],
) -> Result<(), ReadError> {
    let mut r = Reader::new(payload);
    let live = match r.u8()? {
        0 => None,
        1 => Some(r.bytes()?),
        _ => return Err(r.invalid("unknown kind of quads")),
    };
    let count = r.len(12)?;
    let mut kept = Vec::with_capacity(count.min(checkpoints.len()));
    for _ in 0..count {
        let i = r.u32()? as usize;
        let codes = r.bytes()?;
        let fits = checkpoints
            .get(i)
            .filter(|c| c.has_mesh())
            .and_then(|c| c.triangles.as_ref())
            .is_some_and(|t| t.len().div_ceil(4) == codes.len());
        if !fits {
            return Err(r.invalid("quads that don't fit their checkpoint"));
        }
        kept.push((i, codes));
    }
    if let Some(codes) = live
        && !mesh.set_diagonals(codes)
    {
        return Err(r.invalid("quads that don't fit the clay"));
    }
    for (i, codes) in kept {
        checkpoints[i].diagonals = Some(codes.into());
    }
    Ok(())
}

fn write_mesh(w: &mut Writer, mesh: &Mesh) {
    w.len(mesh.positions.len())
        .vec3s(&mesh.positions)
        .len(mesh.triangles.len())
        .u32s(mesh.indices());
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
    use crate::sculpt::{Brush, Dab, Event, Replay, Sculpt};

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
            .levels(s)
            .quads(s)
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
        let resumed = Sculpt::with_history(scene.mesh, scene.levels, scene.log, scene.checkpoints);
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
        let resumed = Sculpt::with_history(scene.mesh, scene.levels, scene.log, scene.checkpoints);
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

    /// A session through subdivision levels, with a remesh undone so that
    /// a checkpoint keeps levels too, ending below the top.
    fn leveled_session() -> Sculpt {
        let mut s = Sculpt::new(Mesh::icosphere(2));
        s.set_clock(1.0);
        s.subdivide();
        s.subdivide();
        let dab = |s: &mut Sculpt, at: Vec3, brush: Brush| {
            s.begin_stroke();
            s.dab(Dab {
                brush,
                centre: at.normalize(),
                radius: 0.3,
                strength: 0.7,
                invert: false,
                symmetry: true,
                direction: Vec3::X,
            });
            s.end_stroke();
            s.refresh();
        };
        dab(&mut s, Vec3::new(0.1, 0.2, 1.0), Brush::Crease);
        s.set_clock(2.0);
        s.change_level(false);
        dab(&mut s, Vec3::new(-0.3, 0.1, 1.0), Brush::Inflate);
        s.set_clock(3.0);
        s.replace_mesh(Mesh::icosphere(1));
        s.undo();
        s.change_level(false);
        dab(&mut s, Vec3::new(0.0, -0.4, 1.0), Brush::Clay);
        s
    }

    #[test]
    fn a_scene_keeps_every_level_and_replays_through_them() {
        let live = leveled_session();
        assert_eq!(live.levels(), Some((0, 3)));
        assert!(live.checkpoints().iter().any(|c| c.levels.is_some()));
        for data in [save(&live, &view()), {
            let mut w = SceneWriter::stored();
            w.mesh(&live.mesh)
                .history(&live, &mut HistoryCache::default())
                .levels(&live);
            w.finish()
        }] {
            let scene = Scene::read(&data).expect("reads back");
            assert!(!scene.history_damaged);
            assert_eq!(scene.levels.as_ref(), live.level_data());
            assert_eq!(scene.checkpoints, live.checkpoints());
            let mut replay = Replay::new(scene.checkpoints.clone(), scene.log.clone());
            assert!(replay.seek(f64::INFINITY, usize::MAX));
            assert_eq!(
                position_bits(&replay.sculpt.mesh.positions),
                position_bits(&live.mesh.positions)
            );
            assert_eq!(replay.sculpt.level_data(), live.level_data());

            // Reopened, it goes up to the finest level as the live sculpt
            // does, sculpts on, and still replays bit for bit.
            let mut resumed =
                Sculpt::with_history(scene.mesh, scene.levels, scene.log, scene.checkpoints);
            let mut live = leveled_session();
            for s in [&mut resumed, &mut live] {
                s.set_clock(9.0);
                assert!(s.change_level(true));
                assert!(s.change_level(true));
            }
            assert_eq!(resumed.mesh.positions, live.mesh.positions);
            let rebuilds = resumed.log().iter().filter(|e| e.event == Event::Rebuild);
            assert_eq!(rebuilds.count(), 1, "reopened without a whole checkpoint");
            let mut replay = Replay::of(&resumed);
            assert!(replay.seek(f64::INFINITY, usize::MAX));
            assert_eq!(replay.sculpt.mesh.positions, resumed.mesh.positions);
            assert_eq!(replay.sculpt.level_data(), resumed.level_data());
        }
    }

    /// Writes two scenes into the folder named by `SPLATSTUDY_DEMO_DIR`, for
    /// looking at levels in the app: a sphere subdivided three times, fine
    /// grooves cut at the top level, then a broad pull made two levels
    /// down; saved at that low level, and again back at the top.
    /// `SPLATSTUDY_DEMO_DIR=… cargo test -p splatstudy-core demo_scenes -- --ignored`
    #[test]
    #[ignore]
    fn demo_scenes() {
        let dir = std::env::var("SPLATSTUDY_DEMO_DIR").expect("SPLATSTUDY_DEMO_DIR");
        let mut s = Sculpt::new(Mesh::icosphere(4));
        let mut clock = 0.0;
        for _ in 0..3 {
            clock += 1.0;
            s.set_clock(clock);
            assert!(s.subdivide());
        }
        for row in 0..5 {
            clock += 1.0;
            s.set_clock(clock);
            s.begin_stroke();
            let y = -0.3 + row as f32 * 0.15;
            for i in 0..40 {
                let x = -0.45 + i as f32 * 0.0225;
                s.dab(Dab {
                    brush: Brush::Crease,
                    centre: Vec3::new(x, y, 1.0).normalize(),
                    radius: 0.06,
                    strength: 1.0,
                    invert: false,
                    symmetry: false,
                    direction: Vec3::X,
                });
                if i % 4 == 3 {
                    s.refresh();
                }
            }
            s.end_stroke();
            s.refresh();
        }
        s.change_level(false);
        s.change_level(false);
        clock += 1.0;
        s.set_clock(clock);
        s.begin_stroke();
        s.grab(Vec3::new(0.0, 0.0, 1.0), 0.7, false);
        s.drag(Vec3::new(0.0, 0.25, 0.45));
        s.end_stroke();
        s.refresh();
        let save = |s: &Sculpt, name: &str| {
            let mut w = SceneWriter::new();
            w.mesh(&s.mesh)
                .checkpoints(s.checkpoints())
                .log(s.log())
                .levels(s);
            std::fs::write(format!("{dir}/{name}"), w.finish()).unwrap();
        };
        save(&s, "levels-low.splatstudy");
        s.change_level(true);
        s.change_level(true);
        save(&s, "levels-high.splatstudy");
    }

    #[test]
    fn a_reopened_scene_with_levels_saves_the_same_bytes() {
        let live = leveled_session();
        let data = save(&live, &view());
        let scene = Scene::read(&data).unwrap();
        let resumed = Sculpt::with_history(scene.mesh, scene.levels, scene.log, scene.checkpoints);
        assert_eq!(save(&resumed, &view()), data);
    }

    #[test]
    fn damaged_levels_open_the_clay_without_them() {
        let live = leveled_session();
        let mut w = SceneWriter::new();
        w.mesh(&live.mesh)
            .checkpoints(live.checkpoints())
            .log(live.log())
            .chunk(LEVELS, true, b"not levels");
        let scene = Scene::read(&w.finish()).expect("the clay opens");
        assert!(scene.levels.is_none() && scene.history_damaged);
        assert!(scene.log.is_empty());
        // Levels that belong to other clay are refused the same way.
        let mut other = Sculpt::new(Mesh::icosphere(3));
        other.subdivide();
        let mut w = SceneWriter::new();
        w.mesh(&Mesh::icosphere(4)).levels(&other);
        let scene = Scene::read(&w.finish()).expect("the clay opens");
        assert!(scene.levels.is_none());
        // Corrupted bytes anywhere: an error or a scene, never a panic.
        let data = save(&live, &view());
        let mut seed = 777u32;
        for _ in 0..60 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let mut bad = data.clone();
            let at = (seed as usize >> 4) % bad.len();
            bad[at] ^= (seed >> 24) as u8 | 1;
            if let Ok(scene) = Scene::read(&bad) {
                let mut replay = Replay::new(scene.checkpoints.clone(), scene.log.clone());
                replay.seek(f64::INFINITY, usize::MAX);
                let mut s =
                    Sculpt::with_history(scene.mesh, scene.levels, scene.log, scene.checkpoints);
                s.change_level(true);
                s.change_level(false);
                s.subdivide();
            }
        }
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

    /// What an autosave costs on the default sphere as a session goes on
    /// over several reloads: 20 strokes per page load, then a save, then
    /// one more stroke and a save again.
    /// `cargo test --release -p splatstudy-core autosave_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn autosave_timing() {
        use std::time::Instant;

        fn strokes(s: &mut Sculpt, clock: &mut f64, from: usize, count: usize) {
            for k in from..from + count {
                *clock += 4.0;
                s.set_clock(*clock);
                s.begin_stroke();
                let (a, b) = (k as f32 * 0.37, k as f32 * 0.21);
                let at = Vec3::new(a.sin(), b.cos(), 0.6).normalize();
                for i in 0..40 {
                    s.dab(Dab {
                        brush: Brush::Clay,
                        centre: (at + Vec3::Y * (i as f32 * 0.01)).normalize(),
                        radius: 0.1,
                        strength: 0.5,
                        invert: false,
                        symmetry: true,
                        direction: Vec3::Y,
                    });
                    if i % 4 == 3 {
                        s.refresh();
                    }
                }
                s.end_stroke();
                s.refresh();
            }
        }
        // As `App::autosave_bytes` writes it.
        let mut cache = HistoryCache::default();
        let mut save = |s: &Sculpt| {
            let mut w = SceneWriter::stored();
            w.mesh(&s.mesh).history(s, &mut cache);
            w.finish()
        };
        let ms = |since: Instant| since.elapsed().as_secs_f64() * 1e3;
        let mut s = Sculpt::new(Mesh::icosphere(crate::sculpt::OLD_START_LEVEL));
        for reload in 0..8 {
            // Each page load starts the clock near 0 again.
            let mut clock = 1.0;
            strokes(&mut s, &mut clock, reload * 21, 20);
            let start = Instant::now();
            let first = save(&s);
            let t_first = ms(start);
            strokes(&mut s, &mut clock, reload * 21 + 20, 1);
            let start = Instant::now();
            let next = save(&s);
            let t_next = ms(start);
            println!(
                "reload {reload}: {} checkpoints; first save {:.2} MB in {t_first:.0} ms; \
                 one stroke later {:.2} MB in {t_next:.0} ms",
                s.checkpoints().len(),
                first.len() as f64 / 1e6,
                next.len() as f64 / 1e6,
            );
            let scene = Scene::read(&next).expect("reads");
            s = Sculpt::with_history(scene.mesh, scene.levels, scene.log, scene.checkpoints);
        }
    }

    /// A session on quads: the start clay with its levels, a stroke, a
    /// remesh to triangles and then to quads again, and another stroke.
    fn quad_session() -> Sculpt {
        let (mesh, levels) = crate::sculpt::cube_sphere(3);
        let mut s = Sculpt::with_levels(mesh, Some(levels));
        let stroke = |s: &mut Sculpt, clock: f64| {
            s.set_clock(clock);
            s.begin_stroke();
            s.dab(Dab {
                brush: Brush::Clay,
                centre: Vec3::new(0.2, 0.3, 1.0).normalize(),
                radius: 0.4,
                strength: 0.6,
                invert: false,
                symmetry: true,
                direction: Vec3::X,
            });
            s.end_stroke();
            s.refresh();
        };
        stroke(&mut s, 1.0);
        s.set_clock(2.0);
        s.replace_mesh(Mesh::icosphere(2));
        stroke(&mut s, 3.0);
        s.set_clock(4.0);
        s.replace_mesh(crate::sculpt::cube_sphere(2).0);
        stroke(&mut s, 5.0);
        s
    }

    fn sorted(quads: &[[u32; 2]]) -> Vec<[u32; 2]> {
        let mut q: Vec<[u32; 2]> = quads.iter().map(|&[a, b]| [a.min(b), a.max(b)]).collect();
        q.sort_unstable();
        q
    }

    #[test]
    fn quads_come_back_in_a_scene_and_an_autosave() {
        let live = quad_session();
        assert!(!live.mesh.quads.is_empty());
        let with_quads: Vec<bool> = live
            .checkpoints()
            .iter()
            .filter(|c| c.triangles.is_some())
            .map(|c| c.diagonals.is_some())
            .collect();
        assert_eq!(with_quads, [true, false, true]);
        let stored = {
            let mut w = SceneWriter::stored();
            w.mesh(&live.mesh)
                .history(&live, &mut HistoryCache::default())
                .levels(&live)
                .quads(&live);
            w.finish()
        };
        for data in [save(&live, &view()), stored] {
            let scene = Scene::read(&data).expect("reads back");
            assert!(!scene.history_damaged);
            assert_eq!(scene.mesh.quads, sorted(&live.mesh.quads));
            assert_eq!(scene.checkpoints, live.checkpoints());
            // Export writes the quads again.
            let obj = String::from_utf8(crate::meshio::obj::write(&scene.mesh)).unwrap();
            let faces = |obj: &str, corners: usize| {
                obj.lines()
                    .filter(|l| l.starts_with("f ") && l.split(' ').count() == corners + 1)
                    .count()
            };
            assert_eq!(faces(&obj, 4), live.mesh.quads.len());
            assert_eq!(faces(&obj, 3), 0);
            // The timelapse has them at each checkpoint, and its clay is
            // still the saved clay bit for bit.
            let mut replay = Replay::new(scene.checkpoints.clone(), scene.log.clone());
            assert!(replay.seek(0.5, usize::MAX));
            assert_eq!(
                replay.sculpt.mesh.quads.len(),
                live.checkpoints()[0].triangles.as_ref().unwrap().len() / 2
            );
            assert!(replay.seek(2.5, usize::MAX));
            assert!(replay.sculpt.mesh.quads.is_empty());
            assert!(replay.seek(f64::INFINITY, usize::MAX));
            assert_eq!(replay.sculpt.mesh.quads, scene.mesh.quads);
            assert_eq!(
                position_bits(&replay.sculpt.mesh.positions),
                position_bits(&live.mesh.positions)
            );
            // Reopened, the levels above keep the quads.
            let mut resumed =
                Sculpt::with_history(scene.mesh, scene.levels, scene.log, scene.checkpoints);
            resumed.set_clock(9.0);
            assert!(resumed.subdivide());
            assert_eq!(resumed.mesh.quads.len(), resumed.mesh.triangles.len() / 2);
        }
        // Saved again untouched, it is the same file.
        let data = save(&live, &view());
        let scene = Scene::read(&data).unwrap();
        let reopened = Sculpt::with_history(scene.mesh, scene.levels, scene.log, scene.checkpoints);
        assert_eq!(save(&reopened, &view()), data);
    }

    #[test]
    fn damaged_quads_open_the_clay_as_triangles() {
        let live = quad_session();
        let codes = live.mesh.diagonals().unwrap();
        let triangles = live.mesh.triangles.len();
        let chunk = |payload: &[u8]| {
            let mut w = SceneWriter::new();
            w.mesh(&live.mesh)
                .history(&live, &mut HistoryCache::default())
                .levels(&live)
                .chunk(QUADS, true, payload);
            Scene::read(&w.finish()).expect("the clay still opens")
        };
        let mut bent = codes.clone();
        bent[0] ^= 1;
        let mut short = Writer::new();
        short.u8(1).bytes(&codes[1..]).len(0);
        let mut other = Writer::new();
        other.u8(1).bytes(&codes).len(1).u32(1).bytes(&codes);
        let mut unknown = Writer::new();
        unknown.u8(1).bytes(&codes).len(1).u32(99).bytes(&codes);
        let mut kind = Writer::new();
        kind.u8(7);
        for payload in [
            b"not quads".to_vec(),
            Vec::new(),
            {
                let mut w = Writer::new();
                w.u8(1).bytes(&bent).len(0);
                w.finish()
            },
            short.finish(),
            // Codes for the start sphere given to the icosphere's checkpoint.
            other.finish(),
            unknown.finish(),
            kind.finish(),
        ] {
            let scene = chunk(&payload);
            assert!(scene.mesh.quads.is_empty());
            assert!(scene.checkpoints.iter().all(|c| c.diagonals.is_none()));
            assert!(!scene.history_damaged, "the timelapse is whole");
            assert_eq!(scene.mesh.triangles.len(), triangles);
        }
        // Every truncation of a good chunk: never a panic, and never quads
        // that don't fit.
        let mut good = Writer::new();
        good.u8(1).bytes(&codes).len(0);
        let good = good.finish();
        for cut in (0..good.len()).step_by(7) {
            let scene = chunk(&good[..cut]);
            assert!(scene.mesh.quads.is_empty());
        }
        assert_eq!(chunk(&good).mesh.quads, sorted(&live.mesh.quads));
    }

    /// What the heavy operations cost on the start clay subdivided to
    /// 1.57 million vertices: subdividing, levels, a stroke, the autosave
    /// and a saved scene.
    /// `cargo test --release -p splatstudy-core heavy_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn heavy_timing() {
        use std::time::Instant;
        let ms = |since: Instant| since.elapsed().as_secs_f64() * 1e3;
        let (mesh, levels) = crate::sculpt::cube_sphere(crate::sculpt::START_LEVEL);
        let mut s = Sculpt::with_levels(mesh, Some(levels));
        for _ in 0..2 {
            let t = Instant::now();
            s.subdivide();
            println!("subdivide to {}: {:.0} ms", s.mesh.positions.len(), ms(t));
        }
        let stroke = |s: &mut Sculpt, radius: f32| {
            let t = Instant::now();
            s.begin_stroke();
            for i in 0..40 {
                s.dab(Dab {
                    brush: Brush::Clay,
                    centre: Vec3::new(-0.4 + i as f32 * 0.02, 0.2, 0.9).normalize(),
                    radius,
                    strength: 0.5,
                    invert: false,
                    symmetry: true,
                    direction: Vec3::X,
                });
                s.refresh();
            }
            s.end_stroke();
            ms(t) / 40.0
        };
        println!(
            "dab, radius 0.1: {:.1} ms; radius 0.5: {:.1} ms",
            stroke(&mut s, 0.1),
            stroke(&mut s, 0.5)
        );
        for up in [false, false, true, true] {
            let t = Instant::now();
            s.change_level(up);
            println!(
                "level {}: {:.0} ms, now {}",
                if up { "up" } else { "down" },
                ms(t),
                s.mesh.positions.len()
            );
        }
        let mut cache = HistoryCache::default();
        for _ in 0..2 {
            let t = Instant::now();
            let mut w = SceneWriter::stored();
            w.mesh(&s.mesh).history(&s, &mut cache).levels(&s);
            let bytes = w.finish();
            println!(
                "autosave: {:.1} MB in {:.0} ms",
                bytes.len() as f64 / 1e6,
                ms(t)
            );
        }
        let t = Instant::now();
        let mut w = SceneWriter::new();
        w.mesh(&s.mesh)
            .history(&s, &mut HistoryCache::default())
            .levels(&s);
        let bytes = w.finish();
        println!(
            "saved scene: {:.1} MB in {:.0} ms",
            bytes.len() as f64 / 1e6,
            ms(t)
        );
        let t = Instant::now();
        let scene = Scene::read(&bytes).expect("reads");
        println!("read scene: {:.0} ms", ms(t));
        drop(scene);
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
