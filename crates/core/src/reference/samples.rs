// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! The references shipped with the web build, under `samples/` next to the
//! page, and who made them. Each one is CC0 or CC BY, approved before it is
//! added, and under 24 MiB (the host's limit per file).

use super::Credit;

/// A bundled reference.
pub struct Sample {
    /// Short name, as in `?sample=<id>`.
    pub id: &'static str,
    /// File name under `samples/`.
    pub file: &'static str,
    pub title: &'static str,
    pub author: &'static str,
    pub licence: &'static str,
    pub url: &'static str,
}

/// Plaster casts of the kind art schools draw and model from: busts,
/// heads, a foot and hands. Most were scanned by SMK (the National Gallery
/// of Denmark) from its Royal Cast Collection and released into the public
/// domain; the hands by the Smithsonian, CC0. Converted to binary PLY and
/// stood upright, nothing else changed (`cargo xtask samples`). Each has a
/// thumbnail for the gallery at [`Sample::thumb_path`].
pub const SAMPLES: &[Sample] = &[
    Sample {
        id: "brutus",
        file: "brutus.ply",
        title: "Brutus, plaster cast after Michelangelo (KAS105)",
        author: "SMK – National Gallery of Denmark, The Royal Cast Collection",
        licence: "CC0 1.0",
        url: "https://open.smk.dk/artwork/image/KAS105",
    },
    Sample {
        id: "venus",
        file: "venus-de-milo.ply",
        title: "Venus de Milo, plaster cast (KAS434)",
        author: "SMK – National Gallery of Denmark, The Royal Cast Collection",
        licence: "CC0 1.0",
        url: "https://open.smk.dk/artwork/image/KAS434",
    },
    Sample {
        id: "david",
        file: "head-of-david.ply",
        title: "Head of David, plaster cast after Michelangelo (KAS2232)",
        author: "SMK – National Gallery of Denmark, The Royal Cast Collection",
        licence: "CC0 1.0",
        url: "https://open.smk.dk/artwork/image/KAS2232",
    },
    Sample {
        id: "homer",
        file: "homer.ply",
        title: "Portrait of Homer, plaster cast (KAS210)",
        author: "SMK – National Gallery of Denmark, The Royal Cast Collection",
        licence: "CC0 1.0",
        url: "https://open.smk.dk/artwork/image/KAS210",
    },
    Sample {
        id: "socrates",
        file: "socrates.ply",
        title: "Portrait of Socrates, plaster cast (KAS635)",
        author: "SMK – National Gallery of Denmark, The Royal Cast Collection",
        licence: "CC0 1.0",
        url: "https://open.smk.dk/artwork/image/KAS635",
    },
    Sample {
        id: "caesar",
        file: "julius-caesar.ply",
        title: "Julius Caesar, plaster cast (KAS297)",
        author: "SMK – National Gallery of Denmark, The Royal Cast Collection",
        licence: "CC0 1.0",
        url: "https://open.smk.dk/artwork/image/KAS297",
    },
    Sample {
        id: "hercules",
        file: "farnese-hercules-head.ply",
        title: "Head from the Farnese Hercules, plaster cast (KAS701)",
        author: "SMK – National Gallery of Denmark, The Royal Cast Collection",
        licence: "CC0 1.0",
        url: "https://open.smk.dk/artwork/image/KAS701",
    },
    Sample {
        id: "foot",
        file: "athlete-right-foot.ply",
        title: "Right foot of an athlete (?), plaster cast (KAS27)",
        author: "SMK – National Gallery of Denmark, The Royal Cast Collection",
        licence: "CC0 1.0",
        url: "https://open.smk.dk/artwork/image/KAS27",
    },
    Sample {
        id: "hands",
        file: "lincoln-hands.ply",
        title: "Abraham Lincoln's hands, plaster casts after Leonard Wells Volk (S/NPG.71.6)",
        author: "National Portrait Gallery, Smithsonian Institution; transfer from the National Museum of American History",
        licence: "CC0 1.0",
        url: "https://npg.si.edu/object/npg_S_NPG.71.6",
    },
];

impl Sample {
    pub fn by_id(id: &str) -> Option<&'static Sample> {
        SAMPLES.iter().find(|s| s.id == id)
    }

    /// The sample a loaded file came from, by its file name.
    pub fn by_file(name: &str) -> Option<&'static Sample> {
        let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
        SAMPLES.iter().find(|s| s.file.eq_ignore_ascii_case(name))
    }

    /// Where the web build fetches it from, next to the page.
    pub fn path(&self) -> String {
        format!("samples/{}", self.file)
    }

    /// Its picture in the gallery, next to the page too.
    pub fn thumb_path(&self) -> String {
        format!("samples/thumbs/{}.png", self.id)
    }

    pub fn credit(&self) -> Credit {
        Credit {
            title: self.title.into(),
            author: self.author.into(),
            licence: self.licence.into(),
            url: self.url.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sample_is_credited_and_openly_licensed() {
        for s in SAMPLES {
            assert!(
                !s.title.is_empty() && !s.author.is_empty() && !s.url.is_empty(),
                "{}",
                s.id
            );
            let open = s.licence.starts_with("CC0") || s.licence.starts_with("CC BY");
            let restricted = ["NC", "ND", "SA"].iter().any(|r| s.licence.contains(r));
            assert!(open && !restricted, "{}: {}", s.id, s.licence);
            assert_eq!(Sample::by_file(&s.path()).map(|f| f.id), Some(s.id));
        }
        assert!(Sample::by_id("nothing").is_none());
    }

    #[test]
    fn every_sample_ships_with_its_thumbnail_and_fits_the_host() {
        let web = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web");
        for s in SAMPLES {
            let file = std::fs::metadata(web.join(s.path())).expect(s.file);
            assert!(file.len() < 24 << 20, "{} is over the host's limit", s.file);
            let thumb = std::fs::metadata(web.join(s.thumb_path())).expect(s.id);
            assert!(thumb.len() < 64 << 10, "{}'s thumbnail is too large", s.id);
        }
    }
}
