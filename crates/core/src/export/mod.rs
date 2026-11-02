// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Exporting the timelapse: frames composed offscreen, then encoded as an
//! MP4 video, or saved as PNG images when no video encoder is there.

pub mod compositor;
pub mod layout;
pub mod mp4;
pub mod video;
pub mod zip;

pub use compositor::{Compositor, EndCard, Overlay};
pub use layout::{Layout, Placement, PxRect, Shape, place};
pub use video::{EncodedFrame, Support, VideoConfig, VideoEncoder};

/// An RGBA8 frame as a PNG, without the alpha (frames are opaque).
pub fn png(rgba: &[u8], size: [u32; 2]) -> Result<Vec<u8>, png::EncodingError> {
    let rgb: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, size[0], size[1]);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder.write_header()?.write_image_data(&rgb)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn frames_become_pngs_that_decode_back() {
        let size = [5, 3];
        let rgba: Vec<u8> = (0..size[0] * size[1])
            .flat_map(|i| [i as u8 * 10, 200, 7, 255])
            .collect();
        let data = super::png(&rgba, size).unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(&data))
            .read_info()
            .unwrap();
        let mut back = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut back).unwrap();
        assert_eq!([info.width, info.height], size);
        assert_eq!(&back[..3], &[0, 200, 7]);
        assert_eq!(&back[3 * 14..3 * 15], &[140, 200, 7]);
    }
}
