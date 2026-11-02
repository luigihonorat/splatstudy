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
//! - `CKPT`: the meshes the log starts from; the first one, at log index 0,
//!   is where the timelapse begins.
//! - `LOG_`: every recorded call, see [`log`].
//! - `REF_`: the reference file, kept whole, with its placement and credit.
//! - `VIEW`: the cameras, the divider and the study aids that were on.
//!
//! Reading returns a [`ReadError`] for anything malformed; it never panics.

pub mod log;
pub mod reference;

use glam::Vec3;

use crate::camera::OrbitCamera;
use crate::io::{ReadError, Reader, Writer};
use crate::reference::Reference;
use crate::sculpt::{Entry, Mesh, Sculpt};

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

const DEFLATED: u32 = 1;
/// Deflate effort: the clay is mostly floats, which barely compress past
/// the fastest level, and saving shouldn't stall the browser.
const DEFLATE_LEVEL: u8 = 1;
/// A deflated chunk may not grow past this when inflated, so a corrupt or
/// hostile file can't ask for all the memory.
const MAX_INFLATED: usize = 1 << 30;

/// A scene as read from a file.
pub struct Scene {
    /// The clay as it was saved.
    pub mesh: Mesh,
    /// Where the log starts from, in log order; may be empty in a file
    /// without a log.
    pub checkpoints: Vec<Checkpoint>,
    pub log: Vec<Entry>,
    pub reference: Option<SavedReference>,
    pub view: Option<View>,
}

/// A mesh the log continues from: before `log_index` entries had been
/// applied, the clay was `mesh`.
pub struct Checkpoint {
    pub log_index: usize,
    /// On the app's clock, in seconds.
    pub time: f64,
    pub mesh: Mesh,
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
}

impl Default for SceneWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneWriter {
    pub fn new() -> Self {
        let mut out = Writer::new();
        out.raw(MAGIC).u32(VERSION);
        Self { out }
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
        self.chunk(MESH, true, &w.finish())
    }

    /// The meshes the log starts from: `(log index, time, mesh)`, the first
    /// at index 0.
    pub fn checkpoints(&mut self, checkpoints: &[(usize, f64, &Mesh)]) -> &mut Self {
        let mut w = Writer::new();
        w.len(checkpoints.len());
        for &(index, time, mesh) in checkpoints {
            w.u64(index as u64).f64(time);
            write_mesh(&mut w, mesh);
        }
        self.chunk(CHECKPOINTS, true, &w.finish())
    }

    pub fn log(&mut self, entries: &[Entry]) -> &mut Self {
        let mut w = Writer::new();
        log::write_entries(&mut w, entries);
        self.chunk(LOG, true, &w.finish())
    }

    /// The reference file is stored as it came: most formats are packed
    /// already, and big ones would take long to deflate.
    pub fn reference(&mut self, reference: &Reference) -> &mut Self {
        let mut w = Writer::new();
        reference::write_reference(&mut w, reference);
        self.chunk(REFERENCE, false, &w.finish())
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

/// One chunk of a file, its payload inflated.
pub struct Chunk {
    pub tag: u32,
    pub payload: Vec<u8>,
}

/// Splits a file into its chunks, after checking the header.
pub fn chunks(data: &[u8]) -> Result<Vec<Chunk>, ReadError> {
    let mut r = Reader::new(data);
    if r.take(MAGIC.len()).ok() != Some(&MAGIC[..]) {
        return Err(ReadError::Invalid {
            offset: 0,
            what: "not a splatstudy scene",
        });
    }
    let version = r.u32()?;
    if version == 0 || version > VERSION {
        return Err(r.invalid("a scene from a newer splatstudy"));
    }
    let mut chunks = Vec::new();
    while !r.is_empty() {
        let tag = r.u32()?;
        let flags = r.u32()?;
        let start = r.position();
        let payload = r.bytes()?;
        let payload = if flags & DEFLATED != 0 {
            miniz_oxide::inflate::decompress_to_vec_with_limit(payload, MAX_INFLATED).map_err(
                |_| ReadError::Invalid {
                    offset: start,
                    what: "damaged compressed data",
                },
            )?
        } else {
            payload.to_vec()
        };
        chunks.push(Chunk { tag, payload });
    }
    Ok(chunks)
}

impl Scene {
    /// Reads a whole scene file.
    pub fn read(data: &[u8]) -> Result<Scene, ReadError> {
        let mut mesh = None;
        let mut checkpoints = Vec::new();
        let mut log = Vec::new();
        let mut reference = None;
        let mut view = None;
        for chunk in chunks(data)? {
            let mut r = Reader::new(&chunk.payload);
            match chunk.tag {
                MESH => mesh = Some(read_mesh(&mut r)?),
                CHECKPOINTS => checkpoints = read_checkpoints(&mut r)?,
                LOG => log = log::read_entries(&mut r)?,
                REFERENCE => reference = Some(reference::read_reference(&mut r)?),
                VIEW => view = Some(read_view(&mut r)?),
                _ => {}
            }
        }
        let mesh = mesh.ok_or(ReadError::Invalid {
            offset: data.len(),
            what: "a scene without clay",
        })?;
        if checkpoints
            .iter()
            .any(|c: &Checkpoint| c.log_index > log.len())
        {
            return Err(ReadError::Invalid {
                offset: data.len(),
                what: "a checkpoint past the end of the log",
            });
        }
        Ok(Scene {
            mesh,
            checkpoints,
            log,
            reference,
            view,
        })
    }

    /// The mesh the log starts from, if the file has one.
    pub fn start(&self) -> Option<&Mesh> {
        self.checkpoints
            .first()
            .filter(|c| c.log_index == 0)
            .map(|c| &c.mesh)
    }
}

/// The live sculpt for a scene that was saved as `saved` after `log` was
/// applied to `start`: the log is applied again, so the new sculpt has the
/// session's undo steps and goes on recording after it, and the timelapse
/// covers both. True when that gave the saved clay bit for bit.
///
/// If it doesn't (a scene from a version whose brushes worked differently),
/// the saved clay wins and the session starts afresh from it: false.
pub fn resume_sculpt(start: Mesh, saved: Mesh, log: &[Entry]) -> (Sculpt, bool) {
    if log.is_empty() {
        return (Sculpt::new(saved), true);
    }
    if start.triangles == saved.triangles {
        let mut sculpt = Sculpt::new(start);
        for entry in log {
            sculpt.set_clock(entry.time);
            sculpt.apply(entry.event);
        }
        let same = |a: &[Vec3], b: &[Vec3]| {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(p, q)| p.to_array().map(f32::to_bits) == q.to_array().map(f32::to_bits))
        };
        if same(&sculpt.mesh.positions, &saved.positions) {
            return (sculpt, true);
        }
    }
    (Sculpt::new(saved), false)
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

/// Reads a mesh in its saved order, refusing indices out of range (which
/// would panic later) and empty meshes (nothing to sculpt).
fn read_mesh(r: &mut Reader) -> Result<Mesh, ReadError> {
    let n = r.len(12)?;
    let mut positions = Vec::with_capacity(n);
    for _ in 0..n {
        let p = r.vec3()?;
        if !p.is_finite() {
            return Err(r.invalid("a vertex that isn't a number"));
        }
        positions.push(p);
    }
    let m = r.len(12)?;
    let mut triangles = Vec::with_capacity(m);
    for _ in 0..m {
        let t = [r.u32()?, r.u32()?, r.u32()?];
        if t.iter().any(|&i| i as usize >= n) {
            return Err(r.invalid("a triangle with a vertex that isn't there"));
        }
        triangles.push(t);
    }
    if triangles.is_empty() {
        return Err(r.invalid("a mesh without triangles"));
    }
    Ok(Mesh::from_ordered(positions, triangles))
}

fn read_checkpoints(r: &mut Reader) -> Result<Vec<Checkpoint>, ReadError> {
    let n = r.len(16)?;
    let mut checkpoints: Vec<Checkpoint> = Vec::with_capacity(n);
    for _ in 0..n {
        let log_index = usize::try_from(r.u64()?).map_err(|_| r.invalid("a checkpoint index"))?;
        let time = r.f64()?;
        let mesh = read_mesh(r)?;
        if checkpoints.last().is_some_and(|c| c.log_index > log_index) {
            return Err(r.invalid("checkpoints out of order"));
        }
        checkpoints.push(Checkpoint {
            log_index,
            time,
            mesh,
        });
    }
    Ok(checkpoints)
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
pub(crate) fn position_bits(positions: &[Vec3]) -> Vec<u32> {
    positions
        .iter()
        .flat_map(|p| p.to_array().map(f32::to_bits))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sculpt::{Brush, Dab};

    /// A short session on a small sphere: a few strokes, an undo.
    fn session() -> (Mesh, Sculpt) {
        let start = Mesh::icosphere(3);
        let mut s = Sculpt::new(start.clone());
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
        (start, s)
    }

    fn save(start: &Mesh, s: &Sculpt, view: &View) -> Vec<u8> {
        let mut w = SceneWriter::new();
        w.mesh(&s.mesh)
            .checkpoints(&[(0, 0.0, start)])
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
        let (start, live) = session();
        let view = view();
        let data = save(&start, &live, &view);
        let scene = Scene::read(&data).expect("reads back");

        assert_eq!(
            position_bits(&scene.mesh.positions),
            position_bits(&live.mesh.positions)
        );
        assert_eq!(scene.mesh.triangles, live.mesh.triangles);
        assert_eq!(scene.view, Some(view));
        assert!(scene.reference.is_none());
        let start_back = scene.start().expect("checkpoint 0");
        assert_eq!(start_back.triangles, start.triangles);

        // Replaying the saved log on the saved start gives the saved clay.
        let mut replay = crate::sculpt::Replay::new(start_back.clone(), scene.log.clone());
        assert!(replay.seek(f64::INFINITY, usize::MAX));
        assert_eq!(
            position_bits(&replay.sculpt.mesh.positions),
            position_bits(&live.mesh.positions)
        );
        // Writing what was read gives the same bytes.
        let mut w = SceneWriter::new();
        w.mesh(&scene.mesh)
            .checkpoints(&[(0, 0.0, start_back)])
            .log(&scene.log)
            .view(&view);
        assert_eq!(w.finish(), data);
    }

    #[test]
    fn a_resumed_sculpt_has_the_session_and_goes_on_recording() {
        let (start, live) = session();
        let scene = Scene::read(&save(&start, &live, &view())).unwrap();
        let start = scene.start().unwrap().clone();
        let (mut resumed, complete) = resume_sculpt(start.clone(), scene.mesh, &scene.log);
        assert!(complete);
        assert_eq!(
            position_bits(&resumed.mesh.positions),
            position_bits(&live.mesh.positions)
        );
        // The same log, entry for entry, and the same undo steps.
        let encode = |entries: &[Entry]| {
            let mut w = Writer::new();
            log::write_entries(&mut w, entries);
            w.finish()
        };
        assert_eq!(encode(resumed.log()), encode(live.log()));
        assert_eq!(resumed.can_undo(), live.can_undo());
        assert_eq!(resumed.can_redo(), live.can_redo());
        resumed.redo();
        assert_eq!(resumed.log().len(), live.log().len() + 1);

        // A saved clay the log doesn't lead to: the clay wins, without a log.
        let mut odd = live.mesh.clone();
        odd.positions[0] += Vec3::splat(0.01);
        let kept = odd.positions[0];
        let (resumed, complete) = resume_sculpt(start, odd, live.log());
        assert!(!complete);
        assert_eq!(resumed.mesh.positions[0], kept);
        assert!(resumed.log().is_empty());
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
        let (start, live) = session();
        let mut w = SceneWriter::new();
        w.chunk(tag(b"ZZZZ"), false, b"from the future")
            .mesh(&live.mesh)
            .chunk(tag(b"YYYY"), true, &[7; 100])
            .checkpoints(&[(0, 0.0, &start)]);
        let scene = Scene::read(&w.finish()).expect("reads");
        assert_eq!(scene.mesh.positions.len(), live.mesh.positions.len());
        assert!(scene.log.is_empty());
        assert!(scene.view.is_none());
    }

    #[test]
    fn every_truncation_and_corruption_is_an_error_not_a_panic() {
        let (start, live) = session();
        let data = save(&start, &live, &view());
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
            let _ = Scene::read(&bad);
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
        // No clay at all.
        assert!(Scene::read(&SceneWriter::new().finish()).is_err());
        // A newer version.
        let mut data = SceneWriter::new().finish();
        data[MAGIC.len()] = 99;
        assert!(Scene::read(&data).is_err());
    }
}
