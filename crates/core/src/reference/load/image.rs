// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Pictures: PNG and JPEG to RGBA8. Very large pictures are halved until
//! they fit a texture every GPU can hold.

use super::LoadError;
use crate::reference::RefImage;

/// The longest side kept; larger pictures are reduced.
pub const MAX_SIDE: u32 = 4096;

/// The longest side read at all. A header alone can declare any size, and
/// the pixels are allocated before they are read, so larger pictures are
/// refused up front.
pub const MAX_READ_SIDE: u32 = 16_384;
/// The most pixels read: 64 megapixels, 256 MiB once in RGBA.
pub const MAX_READ_PIXELS: u64 = 64 << 20;

/// Refuses sizes over [`MAX_READ_SIDE`] or [`MAX_READ_PIXELS`].
fn check_size(width: u32, height: u32) -> Result<(), LoadError> {
    if width.max(height) > MAX_READ_SIDE || u64::from(width) * u64::from(height) > MAX_READ_PIXELS {
        Err(LoadError::Unsupported("picture too large"))
    } else {
        Ok(())
    }
}

pub fn load_png(bytes: &[u8]) -> Result<RefImage, LoadError> {
    let bad = |_| LoadError::Invalid("broken PNG");
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(bad)?;
    let header = reader.info();
    check_size(header.width, header.height)?;
    let size = reader
        .output_buffer_size()
        .ok_or(LoadError::Invalid("PNG too large"))?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(bad)?;
    buf.truncate(info.buffer_size());
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err(LoadError::Unsupported("PNG palette")),
    };
    if info.bit_depth != png::BitDepth::Eight {
        return Err(LoadError::Unsupported("PNG bit depth"));
    }
    let rgba = to_rgba(&buf, info.width, info.height, info.line_size, channels)?;
    Ok(fit(RefImage {
        size: [info.width, info.height],
        rgba,
    }))
}

pub fn load_jpeg(bytes: &[u8]) -> Result<RefImage, LoadError> {
    use zune_jpeg::zune_core::{
        bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions,
    };
    let side = MAX_READ_SIDE as usize;
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(side)
        .set_max_height(side);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder
        .decode_headers()
        .map_err(|_| LoadError::Invalid("broken JPEG"))?;
    let header = decoder.info().ok_or(LoadError::Invalid("broken JPEG"))?;
    check_size(u32::from(header.width), u32::from(header.height))?;
    let rgba = decoder
        .decode()
        .map_err(|_| LoadError::Invalid("broken JPEG"))?;
    let info = decoder.info().ok_or(LoadError::Invalid("broken JPEG"))?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    if rgba.len() != (w * h * 4) as usize || w == 0 || h == 0 {
        return Err(LoadError::Invalid("broken JPEG"));
    }
    Ok(fit(RefImage { size: [w, h], rgba }))
}

fn to_rgba(
    buf: &[u8],
    width: u32,
    height: u32,
    line: usize,
    channels: usize,
) -> Result<Vec<u8>, LoadError> {
    if width == 0 || height == 0 || line < width as usize * channels {
        return Err(LoadError::Empty);
    }
    let mut out = Vec::with_capacity(width as usize * height as usize * 4);
    for row in buf.chunks(line).take(height as usize) {
        for px in row[..width as usize * channels].chunks_exact(channels) {
            out.extend_from_slice(&match *px {
                [g] => [g, g, g, 255],
                [g, a] => [g, g, g, a],
                [r, g, b] => [r, g, b, 255],
                [r, g, b, a] => [r, g, b, a],
                _ => [0; 4],
            });
        }
    }
    if out.len() != width as usize * height as usize * 4 {
        return Err(LoadError::Invalid("broken PNG"));
    }
    Ok(out)
}

/// Halves the picture (averaging 2×2 pixels) until its longest side fits
/// [`MAX_SIDE`].
fn fit(mut image: RefImage) -> RefImage {
    while image.size[0].max(image.size[1]) > MAX_SIDE {
        let [w, h] = image.size;
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut out = Vec::with_capacity((nw * nh * 4) as usize);
        for y in 0..nh {
            for x in 0..nw {
                let mut sum = [0u32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (2 * x + dx).min(w - 1);
                    let sy = (2 * y + dy).min(h - 1);
                    let i = ((sy * w + sx) * 4) as usize;
                    for (total, &v) in sum.iter_mut().zip(&image.rgba[i..i + 4]) {
                        *total += u32::from(v);
                    }
                }
                out.extend(sum.map(|s| ((s + 2) / 4) as u8));
            }
        }
        image = RefImage {
            size: [nw, nh],
            rgba: out,
        };
    }
    image
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A PNG of `width` × `height` RGB pixels.
    pub(crate) fn png_rgb(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(rgb).unwrap();
        writer.finish().unwrap();
        out
    }

    /// A baseline JPEG of one 8×8 block in one colour, written by hand:
    /// only the DC terms are coded, with every quantiser 1. `ycc` is the
    /// colour as Y, Cb, Cr.
    pub(crate) fn jpeg_flat(ycc: [u8; 3]) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        // Quantisation table 0: all ones.
        j.extend_from_slice(&[0xFF, 0xDB, 0, 67, 0]);
        j.extend_from_slice(&[1; 64]);
        // Frame: 8 bits, 8×8, three components at full resolution.
        j.extend_from_slice(&[0xFF, 0xC0, 0, 17, 8, 0, 8, 0, 8, 3]);
        for id in 1..=3 {
            j.extend_from_slice(&[id, 0x11, 0]);
        }
        // DC table 0: twelve 4-bit codes for sizes 0..11. AC table 0: one
        // 1-bit code, end of block.
        j.extend_from_slice(&[0xFF, 0xC4, 0, 31, 0x00]);
        j.extend_from_slice(&[0, 0, 0, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        j.extend(0..12u8);
        j.extend_from_slice(&[0xFF, 0xC4, 0, 20, 0x10]);
        j.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x00]);
        // Scan header.
        j.extend_from_slice(&[0xFF, 0xDA, 0, 12, 3, 1, 0, 2, 0, 3, 0, 0, 63, 0]);
        // Entropy-coded data: per component, the DC difference then EOB.
        let mut bits: Vec<bool> = Vec::new();
        let push = |value: u32, n: u32, bits: &mut Vec<bool>| {
            for k in (0..n).rev() {
                bits.push(value >> k & 1 == 1);
            }
        };
        for c in ycc {
            // A flat block's DC coefficient is 8 × (value − 128).
            let dc = 8 * (i32::from(c) - 128);
            let size = 32 - dc.unsigned_abs().leading_zeros();
            push(size, 4, &mut bits); // the DC code for `size` is `size` in 4 bits
            let raw = if dc < 0 { dc - 1 } else { dc } as u32 & ((1 << size) - 1);
            push(raw, size, &mut bits);
            push(0, 1, &mut bits); // end of block
        }
        while !bits.len().is_multiple_of(8) {
            bits.push(true);
        }
        for byte in bits.chunks(8) {
            let b = byte.iter().fold(0u8, |acc, &bit| acc << 1 | u8::from(bit));
            j.push(b);
            if b == 0xFF {
                j.push(0);
            }
        }
        j.extend_from_slice(&[0xFF, 0xD9]);
        j
    }

    #[test]
    fn png_to_rgba() {
        let rgb: Vec<u8> = (0..16).flat_map(|i| [i * 16, 255 - i * 16, 7]).collect();
        let image = load_png(&png_rgb(4, 4, &rgb)).unwrap();
        assert_eq!(image.size, [4, 4]);
        assert_eq!(&image.rgba[4..8], &[16, 239, 7, 255]);
    }

    #[test]
    fn jpeg_to_rgba() {
        // Y 128 with no colour difference is mid grey.
        let image = load_jpeg(&jpeg_flat([128, 128, 128])).unwrap();
        assert_eq!(image.size, [8, 8]);
        for px in image.rgba.as_chunks::<4>().0 {
            assert!(px[..3].iter().all(|&c| c.abs_diff(128) <= 1), "{px:?}");
            assert_eq!(px[3], 255);
        }
        // A strong red: Y 76, Cb 85, Cr 255.
        let image = load_jpeg(&jpeg_flat([76, 85, 255])).unwrap();
        let px = &image.rgba[..4];
        assert!(px[0] > 230 && px[1] < 25 && px[2] < 25, "{px:?}");
    }

    #[test]
    fn large_pictures_are_halved() {
        let image = fit(RefImage {
            size: [MAX_SIDE * 2 + 1, 2],
            rgba: vec![200; ((MAX_SIDE * 2 + 1) * 2 * 4) as usize],
        });
        assert_eq!(image.size, [MAX_SIDE, 1]);
        assert!(image.rgba.iter().all(|&v| v == 200));
    }

    #[test]
    fn broken_pictures_are_errors() {
        let png = png_rgb(4, 4, &[9; 48]);
        let jpeg = jpeg_flat([128, 128, 128]);
        for end in 0..png.len() - 12 {
            assert!(load_png(&png[..end]).is_err(), "png cut at {end}");
        }
        for end in 0..jpeg.len() - 2 {
            // zune-jpeg pads a cut scan with zeros, so only check it doesn't panic.
            let _ = load_jpeg(&jpeg[..end]);
        }
        assert!(load_jpeg(&jpeg[..20]).is_err());
        assert!(load_png(b"\x89PNG\r\n\x1a\n garbage").is_err());
        assert!(load_jpeg(b"\xFF\xD8 garbage").is_err());
    }

    /// A PNG of a few dozen bytes declaring `width` × `height`: its header,
    /// then the start of some pixel data.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let start = miniz_oxide::deflate::compress_to_vec_zlib(&[0; 64], 6);
        writer.write_chunk(png::chunk::IDAT, &start).unwrap();
        drop(writer);
        out
    }

    #[test]
    fn huge_sizes_are_refused_before_reading() {
        for (w, h) in [(30_000, 30_000), (20_000, 1), (9_000, 9_000)] {
            assert_eq!(
                load_png(&png_header(w, h)).err(),
                Some(LoadError::Unsupported("picture too large")),
                "{w} × {h}"
            );
        }
        assert!(check_size(MAX_READ_SIDE, 4096).is_ok());
        // A JPEG whose frame says 9000 × 9000.
        let mut jpeg = jpeg_flat([128, 128, 128]);
        let frame = jpeg.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        jpeg[frame + 5..frame + 9].copy_from_slice(&[0x23, 0x28, 0x23, 0x28]);
        assert_eq!(
            load_jpeg(&jpeg).err(),
            Some(LoadError::Unsupported("picture too large"))
        );
    }
}
