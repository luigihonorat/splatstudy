// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The gallery on the desktop. The core asks for a sample by its path next
//! to the web page (`samples/<file>`, `samples/thumbs/<id>.png`), as in the
//! browser; only the gallery's own samples are answered.
//!
//! The thumbnails (about 100 KiB in all) are built into the app. A cast is
//! downloaded from the web app's site the first time it is chosen and kept
//! in the app's folder, under `samples/`, so it opens offline from then on.
//! A download goes to a temporary file that is renamed only once complete,
//! so a partial one is never taken for the cast.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use splatstudy_core::reference::samples::{SAMPLES, Sample};
use ureq::tls::{Certificate, RootCerts, TlsConfig};

/// The web app; the samples sit under `samples/` next to its page.
const SITE: &str = "https://app.splatstudy.com/";
/// No cast is larger: the host takes at most 24 MiB per file.
const MAX_CAST: u64 = 32 << 20;
/// Downloads in progress end in this, after the cast's name.
const PARTIAL: &str = ".partial";
/// Temporary files older than this are left over from a run that stopped
/// mid-download, and are removed.
const STALE: Duration = Duration::from_secs(24 * 60 * 60);

/// What a fetch asks for.
#[derive(Clone, Copy)]
pub enum Wanted {
    Cast(&'static Sample),
    Thumbnail(&'static Sample),
}

/// The sample a fetched path names: the gallery's own list only, never any
/// other path or URL.
pub fn wanted(url: &str) -> Option<Wanted> {
    SAMPLES.iter().find_map(|s| {
        if url == s.path() {
            Some(Wanted::Cast(s))
        } else if url == s.thumb_path() {
            Some(Wanted::Thumbnail(s))
        } else {
            None
        }
    })
}

/// Each sample's thumbnail by id, as the web build serves them.
macro_rules! thumbnails {
    ($($id:literal),* $(,)?) => {
        &[$(($id, include_bytes!(concat!("../../../web/samples/thumbs/", $id, ".png")))),*]
    };
}

const THUMBNAILS: &[(&str, &[u8])] = thumbnails![
    "brutus", "venus", "david", "homer", "socrates", "caesar", "hercules", "foot", "hands",
];

/// A sample's thumbnail, empty if it has none (the gallery leaves it blank).
pub fn thumbnail(sample: &Sample) -> Arc<[u8]> {
    THUMBNAILS
        .iter()
        .find(|(id, _)| *id == sample.id)
        .map_or_else(|| Arc::from([]), |(_, png)| Arc::from(*png))
}

/// Where the casts are kept: `samples/` in the app's folder.
pub fn folder() -> Option<PathBuf> {
    crate::config::folder().map(|f| f.join("samples"))
}

/// The cast's copy in `folder`, if a complete one is there. Downloads in
/// progress have another name, so they never count.
pub fn cached(folder: &Path, sample: &Sample) -> Option<PathBuf> {
    let path = folder.join(sample.file);
    let meta = std::fs::metadata(&path).ok()?;
    (meta.is_file() && meta.len() > 0).then_some(path)
}

/// The cast in `folder`, downloaded first unless it is there already.
pub fn cast(folder: &Path, sample: &Sample) -> io::Result<PathBuf> {
    if let Some(path) = cached(folder, sample) {
        return Ok(path);
    }
    let url = format!("{SITE}{}", sample.path());
    log::info!("downloading {url}");
    let mut response = agent().get(&url).call().map_err(io::Error::other)?;
    let body = response.body_mut();
    let expected = body.content_length();
    let reader = body.with_config().limit(MAX_CAST).reader();
    keep(folder, sample, expected, reader)
}

/// The client, made once: a short wait to connect and for the answer, a
/// long one for the body (a cast is several megabytes), and the root
/// certificates the OS trusts.
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let found = rustls_native_certs::load_native_certs();
        for e in &found.errors {
            log::warn!("root certificates: {e}");
        }
        let roots = found
            .certs
            .iter()
            .map(|der| Certificate::from_der(der.as_ref()).to_owned());
        let tls = TlsConfig::builder()
            .root_certs(RootCerts::from(roots))
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        ureq::Agent::config_builder()
            .user_agent(format!("SplatStudy/{}", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(20)))
            .timeout_recv_body(Some(Duration::from_secs(300)))
            .tls_config(tls)
            .build()
            .into()
    })
}

/// Writes `body` into `folder` as the sample's file, by way of a temporary
/// file that is renamed only when the body is complete: as long as
/// `expected` when the server said, and not empty. Anything else leaves no
/// file behind.
fn keep(
    folder: &Path,
    sample: &Sample,
    expected: Option<u64>,
    body: impl Read,
) -> io::Result<PathBuf> {
    std::fs::create_dir_all(folder)?;
    remove_stale(folder);
    // Named per download, so two at once never write the same file.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let file = sample.file;
    let partial = folder.join(format!("{file}.{}-{n}{PARTIAL}", std::process::id()));
    let path = folder.join(file);
    let written = write_all(&partial, body).and_then(|len| match expected {
        _ if len == 0 => Err(io::Error::other("empty")),
        Some(want) if want != len => {
            Err(io::Error::other(format!("{len} of {want} bytes arrived")))
        }
        _ => Ok(()),
    });
    let renamed = written.and_then(|()| std::fs::rename(&partial, &path));
    if let Err(e) = renamed {
        let _ = std::fs::remove_file(&partial);
        // Another download of the same cast may have just finished.
        return cached(folder, sample).ok_or(e);
    }
    Ok(path)
}

/// Copies `body` into a new file at `path`, flushed to disk; its length.
fn write_all(path: &Path, mut body: impl Read) -> io::Result<u64> {
    let mut out = io::BufWriter::new(std::fs::File::create(path)?);
    let len = io::copy(&mut body, &mut out)?;
    out.into_inner().map_err(io::Error::from)?.sync_all()?;
    Ok(len)
}

/// Removes temporary files left by a run that stopped mid-download.
fn remove_stale(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let partial = entry.file_name().to_string_lossy().ends_with(PARTIAL);
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > STALE));
        if partial && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty folder for one test.
    fn scratch(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("splatstudy-gallery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        folder
    }

    /// Yields `bytes`, then fails as a dropped connection does.
    struct Cut(io::Cursor<Vec<u8>>);

    impl Read for Cut {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.0.read(buf)? {
                0 => Err(io::ErrorKind::ConnectionReset.into()),
                n => Ok(n),
            }
        }
    }

    /// What a fetch of `url` asks for, as (is it a cast, which sample).
    fn asked(url: &str) -> Option<(bool, &'static str)> {
        wanted(url).map(|w| match w {
            Wanted::Cast(s) => (true, s.id),
            Wanted::Thumbnail(s) => (false, s.id),
        })
    }

    #[test]
    fn only_the_gallerys_own_samples_can_be_fetched() {
        for s in SAMPLES {
            assert_eq!(asked(&s.path()), Some((true, s.id)));
            assert_eq!(asked(&s.thumb_path()), Some((false, s.id)));
        }
        for other in [
            "samples/other.ply",
            "samples/../preferences",
            "../samples/homer.ply",
            "/samples/homer.ply",
            "https://example.com/samples/homer.ply",
            "samples/homer.ply?x=1",
            "samples/thumbs/homer.ply",
            "licenses.html",
            "",
        ] {
            assert_eq!(asked(other), None, "{other}");
        }
    }

    #[test]
    fn every_sample_has_its_thumbnail_built_in() {
        assert_eq!(THUMBNAILS.len(), SAMPLES.len());
        for s in SAMPLES {
            let png = thumbnail(s);
            assert!(png.starts_with(b"\x89PNG"), "{}", s.id);
        }
    }

    #[test]
    fn a_complete_download_is_kept_and_found_again() {
        let folder = scratch("complete");
        let homer = Sample::by_id("homer").unwrap();
        assert_eq!(cached(&folder, homer), None);
        let body = b"ply\n...".to_vec();
        let path = keep(&folder, homer, Some(body.len() as u64), &body[..]).unwrap();
        assert_eq!(path, folder.join("homer.ply"));
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(cached(&folder, homer), Some(path.clone()));
        // Without a length from the server, too.
        let brutus = Sample::by_id("brutus").unwrap();
        keep(&folder, brutus, None, &body[..]).unwrap();
        assert!(cached(&folder, brutus).is_some());
        // Opened from there, not downloaded again.
        assert_eq!(cast(&folder, homer).unwrap(), path);
        let left: Vec<_> = std::fs::read_dir(&folder).unwrap().flatten().collect();
        assert_eq!(left.len(), 2, "no temporary files left");
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_partial_download_is_never_taken_for_the_cast() {
        let folder = scratch("partial");
        let homer = Sample::by_id("homer").unwrap();
        // Cut off midway.
        let cut = Cut(io::Cursor::new(vec![1; 1000]));
        assert!(keep(&folder, homer, None, cut).is_err());
        // Shorter than the server said.
        assert!(keep(&folder, homer, Some(2000), &[1u8; 1000][..]).is_err());
        // Nothing at all.
        assert!(keep(&folder, homer, None, io::empty()).is_err());
        assert_eq!(cached(&folder, homer), None);
        assert_eq!(
            std::fs::read_dir(&folder).unwrap().count(),
            0,
            "no leftovers"
        );

        // A temporary file from a run that stopped doesn't count either,
        // nor does an empty file under the cast's name.
        std::fs::write(folder.join(format!("homer.ply.1-0{PARTIAL}")), [1; 10]).unwrap();
        assert_eq!(cached(&folder, homer), None);
        std::fs::write(folder.join("homer.ply"), []).unwrap();
        assert_eq!(cached(&folder, homer), None);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// Downloads a cast from the live site (needs the internet):
    /// `cargo test -p splatstudy-native -- --ignored downloads`.
    #[test]
    #[ignore]
    fn downloads_a_cast_from_the_site() {
        let folder = scratch("live");
        let hands = Sample::by_id("hands").unwrap();
        let path = cast(&folder, hands).expect("downloaded");
        let bytes = std::fs::read(&path).unwrap();
        let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/samples");
        assert_eq!(bytes, std::fs::read(shipped.join(hands.file)).unwrap());
        assert_eq!(cached(&folder, hands), Some(path));
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
