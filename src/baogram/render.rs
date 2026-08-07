//! Rendering helpers: convert full-resolution Mono1 images into the
//! 128x128 display bitmap format used by `Gfx::bitmap`.
//!
//! The display bitmap is `[u32; 512]` (128x128 x 1bpp), bit index
//! `x + y * 128`, and a **set bit is dark** (matching `src/bitmaps/`).
//! Baogram Mono1 uses 1 = white, so bits invert during conversion.

use baogram_core::image::Mono1Image;

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
