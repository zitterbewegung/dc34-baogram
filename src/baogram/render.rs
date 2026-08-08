//! Rendering helpers: convert full-resolution Mono1 images into the
//! 128x128 display bitmap format used by `Gfx::bitmap`.
//!
//! The display bitmap is `[u32; 512]` (128x128 x 1bpp), bit index
//! `x + y * 128`, and a **set bit is dark** (matching `src/bitmaps/`).
//! Baogram Mono1 uses 1 = white, so bits invert during conversion.

use baogram_core::image::{Mono1Image, Mono1Small, SMALL_HEIGHT, SMALL_WIDTH};

/// Rows of the display used by the 2x-downsampled 128x120 preview; the
/// bottom 8 rows are left light for a status/label line drawn via TextView.
pub const PREVIEW_ROWS: usize = 120;

/// Deterministic 2x downsample of a full-resolution 256x240 Mono1 image to
/// a 128x120 preview placed at the top of a 128x128 display bitmap.
pub fn mono1_to_display_bitmap(img: &Mono1Image) -> [u32; 512] {
    let small = img.downsample_2x(); // 128x120 packed, MSB-first, 1 = white
    let mut bits = [0u32; 512];
    const OUT_W: usize = 128;
    const ROW_BYTES: usize = OUT_W / 8; // 16
    for y in 0..PREVIEW_ROWS {
        for x in 0..OUT_W {
            let white = small[y * ROW_BYTES + x / 8] & (0x80 >> (x % 8)) != 0;
            if !white {
                // set bit = dark
                let bitnum = x + y * OUT_W;
                bits[bitnum / 32] |= 1 << (bitnum % 32);
            }
        }
    }
    // bottom 8 rows stay 0 (light) as the label strip
    bits
}

/// Convert a 128x128 display bitmap — as uploaded over serial by the
/// `image` console command and stored in the `dc34:image` PDDB key — into
/// a Baogram small-format image, so an uploaded picture can become a post.
///
/// Two conventions flip here:
///   * display: bit `x + y * 128` set = **dark**; Mono1: 1 = **white**
///   * the upload is 128 rows tall, a post is 128x120 (`SMALL_HEIGHT`)
///
/// The height is reconciled by nearest-neighbour row resampling
/// (`src_y = y * 128 / 120`), so the whole picture stays visible rather
/// than being cropped; 8 of the 128 source rows are dropped. Columns are
/// 1:1 — every horizontal pixel survives exactly.
pub fn display_bitmap_to_small(bits: &[u32; 512]) -> Mono1Small {
    const SRC_W: usize = 128;
    const SRC_H: usize = 128;
    let mut packed = [0u8; baogram_core::image::SMALL_PACKED_LEN];
    for y in 0..SMALL_HEIGHT {
        let src_y = y * SRC_H / SMALL_HEIGHT;
        for x in 0..SMALL_WIDTH {
            let bitnum = x + src_y * SRC_W;
            let dark = bits[bitnum / 32] & (1 << (bitnum % 32)) != 0;
            if !dark {
                // Mono1: 1 = white
                packed[y * baogram_core::image::SMALL_ROW_BYTES + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    Mono1Small::from_packed(&packed).expect("SMALL_PACKED_LEN buffer is always valid")
}

/// A full-screen "broken post" placeholder: dark border, light interior.
pub fn corrupt_placeholder() -> [u32; 512] {
    let mut bits = [0u32; 512];
    const W: usize = 128;
    for y in 0..PREVIEW_ROWS {
        for x in 0..W {
            let border = x < 2 || x >= W - 2 || y < 2 || y >= PREVIEW_ROWS - 2;
            let diag = x == y || (W - 1 - x) == y;
            if border || diag {
                let bitnum = x + y * W;
                bits[bitnum / 32] |= 1 << (bitnum % 32);
            }
        }
    }
    bits
}

/// Build the 128x128 display bitmap for a serial upload's test pattern:
/// deterministic, asymmetric in both axes (so a flip or transpose shows up),
/// and used by both the unit tests here and the hosted import test in
/// `src/hosted.rs`.
pub fn import_test_pattern() -> [u32; 512] {
    let mut bits = [0u32; 512];
    for y in 0..128 {
        for x in 0..128 {
            // a dark wedge that grows with y, a dark 8x8 marker in the
            // top-left corner only, and vertical stripes near the bottom
            let wedge = x < y / 2;
            let marker = x < 8 && y < 8;
            let stripe = x % 8 == 0 && y > 96;
            if wedge || marker || stripe {
                let n = x + y * 128;
                bits[n / 32] |= 1 << (n % 32);
            }
        }
    }
    bits
}

#[cfg(test)]
mod tests {
    use baogram_core::image::*;

    use super::*;

    fn set(bits: &mut [u32; 512], x: usize, y: usize) {
        let n = x + y * 128;
        bits[n / 32] |= 1 << (n % 32);
    }

    #[test]
    fn all_light_upload_is_all_white() {
        let img = display_bitmap_to_small(&[0u32; 512]);
        assert!(img.packed().iter().all(|&b| b == 0xff));
    }

    #[test]
    fn all_dark_upload_is_all_black() {
        let img = display_bitmap_to_small(&[u32::MAX; 512]);
        assert!(img.packed().iter().all(|&b| b == 0x00));
    }

    /// Columns are 1:1 — no horizontal resampling, no off-by-one.
    #[test]
    fn every_column_survives() {
        let mut bits = [0u32; 512];
        for y in 0..128 {
            for x in (0..128).step_by(2) {
                set(&mut bits, x, y);
            }
        }
        let img = display_bitmap_to_small(&bits);
        for y in 0..SMALL_HEIGHT {
            for x in 0..SMALL_WIDTH {
                assert_eq!(img.get(x, y), x % 2 == 1, "col {} row {}", x, y);
            }
        }
    }

    /// Row resampling drops exactly 8 of 128 rows, never duplicates one,
    /// preserves order, and fills every one of the 120 output rows.
    #[test]
    fn row_mapping_is_nearest_neighbour_and_monotonic() {
        let mut landed = Vec::new();
        for src_y in 0..128 {
            let mut bits = [0u32; 512];
            for x in 0..128 {
                set(&mut bits, x, src_y);
            }
            let img = display_bitmap_to_small(&bits);
            landed.push((0..SMALL_HEIGHT).filter(|&y| !img.get(0, y)).collect::<Vec<_>>());
        }
        assert_eq!(landed.iter().filter(|h| h.is_empty()).count(), 8, "expected 8 dropped rows");
        let mut last: Option<usize> = None;
        let mut covered = std::collections::HashSet::new();
        for hits in &landed {
            for &y in hits {
                assert!(covered.insert(y), "output row {} written twice", y);
                if let Some(l) = last {
                    assert!(y > l, "row order not monotonic");
                }
                last = Some(y);
            }
        }
        assert_eq!(covered.len(), SMALL_HEIGHT, "not every output row covered");
    }

    /// The polarity/layout invariant: encoding a post for the display and
    /// decoding it back as an upload is the identity, on every row the
    /// display encoder actually writes.
    #[test]
    fn round_trips_through_the_display_encoder() {
        let mut packed = [0u8; SMALL_PACKED_LEN];
        for (i, b) in packed.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(37) ^ 0x5a;
        }
        let orig = Mono1Small::from_packed(&packed).unwrap();
        let back = display_bitmap_to_small(&mono1_to_display_bitmap(&orig.to_full()));
        let mut compared = 0;
        for y in 0..SMALL_HEIGHT {
            let src_y = y * 128 / SMALL_HEIGHT;
            // rows >= PREVIEW_ROWS are the blank label strip, not image data
            if src_y >= PREVIEW_ROWS {
                continue;
            }
            for x in 0..SMALL_WIDTH {
                assert_eq!(back.get(x, y), orig.get(x, src_y), "x {} y {}", x, y);
            }
            compared += 1;
        }
        assert!(compared > 100, "only compared {} rows", compared);
    }

    /// The test pattern must be asymmetric both ways, or the hosted import
    /// test could pass on a flipped or transposed image.
    #[test]
    fn test_pattern_is_asymmetric() {
        let img = display_bitmap_to_small(&import_test_pattern());
        let mut h_mirror = 0;
        let mut v_mirror = 0;
        for y in 0..SMALL_HEIGHT {
            for x in 0..SMALL_WIDTH {
                if img.get(x, y) != img.get(SMALL_WIDTH - 1 - x, y) {
                    h_mirror += 1;
                }
                if img.get(x, y) != img.get(x, SMALL_HEIGHT - 1 - y) {
                    v_mirror += 1;
                }
            }
        }
        assert!(h_mirror > 1000, "pattern is nearly h-symmetric ({} differing)", h_mirror);
        assert!(v_mirror > 1000, "pattern is nearly v-symmetric ({} differing)", v_mirror);
    }
}
