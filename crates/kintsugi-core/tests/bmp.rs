//! BMP decoder integration tests: hand-built files, hand-checked pixels.

use kintsugi_core::codec::bmp::decode_bmp;
use kintsugi_core::error::Error;

/// Build an uncompressed BMP by hand.
///
/// `pixels` is top-down, row-major, no padding; one byte per pixel for
/// 8-bit, three bytes (BGR) per pixel for 24-bit, four for 32-bit.
fn build_bmp(width: u32, height: u32, bpp: u16, palette: &[[u8; 3]], pixels: &[u8]) -> Vec<u8> {
    let bytes_per_pixel = bpp as usize / 8;
    let stride = width as usize * bytes_per_pixel;
    let padded = (stride + 3) & !3;
    let palette_bytes = if bpp == 8 { palette.len() * 4 } else { 0 };
    let data_offset = 14 + 40 + palette_bytes;
    let size = data_offset + padded * height as usize;

    let mut out = vec![0u8; size];
    out[0..2].copy_from_slice(b"BM");
    out[2..6].copy_from_slice(&(size as u32).to_le_bytes());
    out[10..14].copy_from_slice(&(data_offset as u32).to_le_bytes());

    let info = &mut out[14..];
    info[0..4].copy_from_slice(&40u32.to_le_bytes());
    info[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    info[8..12].copy_from_slice(&(height as i32).to_le_bytes()); // positive → bottom-up
    info[12..14].copy_from_slice(&1u16.to_le_bytes());
    info[14..16].copy_from_slice(&bpp.to_le_bytes());
    // compression (offset 16), image size (20), resolutions (24/28), colors (32/36) stay 0

    if bpp == 8 {
        // biClrUsed, the way a real encoder writes it.
        out[46..50].copy_from_slice(&(palette.len() as u32).to_le_bytes());
        for (i, [r, g, b]) in palette.iter().enumerate() {
            let at = 14 + 40 + i * 4;
            out[at] = *b;
            out[at + 1] = *g;
            out[at + 2] = *r;
            out[at + 3] = 0;
        }
    }

    for row in 0..height as usize {
        // Write rows bottom-up so the *decoded* image reads top-down.
        let target = height as usize - 1 - row;
        let src = &pixels[row * stride..row * stride + stride];
        out[data_offset + target * padded..][..stride].copy_from_slice(src);
    }
    out
}

#[test]
fn decodes_24bpp_bottom_up() {
    // 3x2 image, one padded row (9 bytes → 12 with padding).
    let pixels: Vec<u8> = [
        (10, 20, 30),
        (40, 50, 60),
        (70, 80, 90), // row 0
        (100, 110, 120),
        (130, 140, 150),
        (160, 170, 180), // row 1
    ]
    .iter()
    .flat_map(|&(r, g, b)| [b, g, r])
    .collect();
    let bmp = build_bmp(3, 2, 24, &[], &pixels);
    let image = decode_bmp(&bmp).unwrap();
    assert_eq!((image.width, image.height), (3, 2));
    assert_eq!(image.rgba()[0..4], [10, 20, 30, 255]);
    assert_eq!(image.rgba()[4..8], [40, 50, 60, 255]);
    assert_eq!(image.rgba()[8..12], [70, 80, 90, 255]);
    assert_eq!(image.rgba()[12..16], [100, 110, 120, 255]);
    assert_eq!(image.rgba()[16..20], [130, 140, 150, 255]);
    assert_eq!(image.rgba()[20..24], [160, 170, 180, 255]);
}

#[test]
fn decodes_8bpp_with_palette() {
    let palette = [[255, 0, 0], [0, 255, 0], [0, 0, 255]];
    let pixels = [0u8, 1, 2, 2, 1, 0];
    let bmp = build_bmp(3, 2, 8, &palette, &pixels);
    let image = decode_bmp(&bmp).unwrap();
    assert_eq!((image.width, image.height), (3, 2));
    assert_eq!(image.rgba()[0..4], [255, 0, 0, 255]);
    assert_eq!(image.rgba()[4..8], [0, 255, 0, 255]);
    assert_eq!(image.rgba()[8..12], [0, 0, 255, 255]);
    assert_eq!(image.rgba()[12..16], [0, 0, 255, 255]);
}

#[test]
fn decodes_32bpp_and_forces_opaque_alpha() {
    let pixels: Vec<u8> = [(1, 2, 3, 0), (4, 5, 6, 0)]
        .iter()
        .flat_map(|&(r, g, b, a)| [b, g, r, a])
        .collect();
    let bmp = build_bmp(2, 1, 32, &[], &pixels);
    let image = decode_bmp(&bmp).unwrap();
    // The legacy engine ignored byte 4; the faithful repair renders opaque.
    assert_eq!(image.rgba()[0..8], [1, 2, 3, 255, 4, 5, 6, 255]);
}

#[test]
fn clamps_short_palettes_declared_as_256() {
    // Era-correct quirk: `biClrUsed = 0` claims 256 entries, while the file
    // only stores the three it uses before the pixel data starts.
    let palette = [[255, 0, 0], [0, 255, 0], [0, 0, 255]];
    let mut bmp = build_bmp(3, 2, 8, &palette, &[0u8, 1, 2, 2, 1, 0]);
    bmp[46..50].copy_from_slice(&0u32.to_le_bytes());
    let image = decode_bmp(&bmp).unwrap();
    assert_eq!(image.rgba()[0..4], [255, 0, 0, 255]);
    assert_eq!(image.rgba()[8..12], [0, 0, 255, 255]);
}

#[test]
fn rejects_palette_index_beyond_stored_entries() {
    let palette = [[255, 0, 0], [0, 255, 0]];
    // Pixel index 2, but only two entries stored: a genuine corruption.
    let bmp = build_bmp(2, 1, 8, &palette, &[0u8, 2]);
    assert!(matches!(
        decode_bmp(&bmp),
        Err(Error::Corrupt { ref detail, .. }) if detail.contains("palette index")
    ));
}

#[test]
fn rejects_compressed_and_odd_bpp() {
    let mut bmp = build_bmp(2, 1, 24, &[], &[0; 6]);
    // Compression field lives at 14 + 16.
    bmp[30..34].copy_from_slice(&3u32.to_le_bytes()); // BI_BITFIELDS
    assert!(matches!(
        decode_bmp(&bmp),
        Err(Error::Unsupported { ref detail, .. }) if detail.contains("compression")
    ));

    let bmp16 = build_bmp(2, 1, 16, &[], &[0; 4]);
    assert!(matches!(
        decode_bmp(&bmp16),
        Err(Error::Unsupported { ref detail, .. }) if detail.contains("16 bits")
    ));
}

#[test]
fn rejects_truncated_pixel_data() {
    let pixels = [0u8; 3 * 2 * 3]; // 3x2 at 24bpp
    let mut bmp = build_bmp(3, 2, 24, &[], &pixels);
    // Keep the headers and five pixel bytes of the eighteen promised.
    let data_offset = u32::from_le_bytes(bmp[10..14].try_into().unwrap()) as usize;
    bmp.truncate(data_offset + 5);
    assert!(matches!(decode_bmp(&bmp), Err(Error::Corrupt { .. })));
}

/// A header is not a promise. A 194-byte file claiming to be 65535x65535
/// (16 GB of RGBA) must be refused *before* anything tries to allocate it:
/// `Image::new` panics on an impossible size, and an abort from a malformed
/// game file would be a crash, not a diagnosis.
#[test]
fn refuses_a_header_that_claims_an_impossible_size() {
    let mut bmp = build_bmp(1, 1, 24, &[], &[0, 0, 0, 0]);
    bmp[18..22].copy_from_slice(&65535u32.to_le_bytes()); // biWidth
    bmp[22..26].copy_from_slice(&65535u32.to_le_bytes()); // biHeight

    let err = decode_bmp(&bmp).unwrap_err().to_string();
    assert!(
        err.contains("65535x65535") && err.contains("pixel limit"),
        "expected the pixel-limit refusal, got: {err}"
    );
}

/// Zero in either dimension is degenerate, not a 0x0 image.
#[test]
fn refuses_a_degenerate_size() {
    let bmp = build_bmp(0, 4, 24, &[], &[]);
    let err = decode_bmp(&bmp).unwrap_err().to_string();
    assert!(err.contains("degenerate size"), "{err}");
}

#[test]
fn rejects_bad_signature() {
    assert!(matches!(
        decode_bmp(b"NOT A BMP AT ALL"),
        Err(Error::Unsupported { .. })
    ));
}
