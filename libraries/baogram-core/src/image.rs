//! Image representation and quantization.
//!
//! The MVP image is a full-spatial-resolution 256x240 frame. Capture produces
//! 8-bit grayscale luminance; storage and sharing use 1-bit monochrome packed
//! MSB-first, row-major. All 256x240 = 61,440 pixel positions are preserved;
//! only the per-pixel depth is quantized to one bit.
//!
//! Bit convention: **1 = white (light), 0 = black (dark)**, matching the
//! badge display convention where luminance above threshold renders white.

use crate::error::{BaogramError, Result};

/// Width of a Baogram MVP image in pixels.
pub const IMAGE_WIDTH: usize = 256;
/// Height of a Baogram MVP image in pixels.
pub const IMAGE_HEIGHT: usize = 240;
/// Number of pixels (and bytes of 8-bit grayscale) in a full frame.
pub const IMAGE_PIXELS: usize = IMAGE_WIDTH * IMAGE_HEIGHT; // 61,440
/// Length in bytes of a packed 1-bit monochrome image: 256/8 * 240.
pub const MONO1_PACKED_LEN: usize = IMAGE_WIDTH / 8 * IMAGE_HEIGHT; // 7,680
/// Bytes per packed row.
pub const MONO1_ROW_BYTES: usize = IMAGE_WIDTH / 8; // 32

/// A full-resolution 1-bit monochrome image, packed MSB-first, row-major.
///
/// Pixel (x, y) lives at byte `y * MONO1_ROW_BYTES + x / 8`,
/// bit `7 - (x % 8)`. Bit value 1 = white, 0 = black.
#[derive(Clone, PartialEq, Eq)]
pub struct Mono1Image {
    data: Box<[u8; MONO1_PACKED_LEN]>,
}

impl Mono1Image {
    /// Wrap exactly MONO1_PACKED_LEN packed bytes.
    pub fn from_packed(packed: &[u8]) -> Result<Self> {
        if packed.len() != MONO1_PACKED_LEN {
            return Err(BaogramError::BadPackedSize);
        }
        let mut data = Box::new([0u8; MONO1_PACKED_LEN]);
        data.copy_from_slice(packed);
        Ok(Mono1Image { data })
    }

    /// The packed bytes, exactly MONO1_PACKED_LEN long.
    pub fn packed(&self) -> &[u8; MONO1_PACKED_LEN] {
        &self.data
    }

    /// Read pixel (x, y); true = white.
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> bool {
        debug_assert!(x < IMAGE_WIDTH && y < IMAGE_HEIGHT);
        self.data[y * MONO1_ROW_BYTES + x / 8] & (0x80 >> (x % 8)) != 0
    }

    /// Set pixel (x, y); true = white.
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, white: bool) {
        debug_assert!(x < IMAGE_WIDTH && y < IMAGE_HEIGHT);
        let byte = &mut self.data[y * MONO1_ROW_BYTES + x / 8];
        let mask = 0x80 >> (x % 8);
        if white {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
    }

    /// Quantize an 8-bit grayscale frame (exactly IMAGE_PIXELS bytes,
    /// row-major) to 1-bit monochrome using a deterministic global threshold.
    ///
    /// When `threshold_override` is None, the threshold is the mean luminance
    /// of the full frame (integer division, so deterministic). A pixel is
    /// white iff `luminance > threshold`, matching the badge preview
    /// convention. No dithering is applied: error diffusion would harm
    /// run-length compression and make validation nondeterministic across
    /// implementations.
    pub fn quantize(gray: &[u8], threshold_override: Option<u8>) -> Result<Self> {
        if gray.len() != IMAGE_PIXELS {
            return Err(BaogramError::BadFrameSize);
        }
        let threshold = match threshold_override {
            Some(t) => t,
            None => global_threshold(gray),
        };
        let mut data = Box::new([0u8; MONO1_PACKED_LEN]);
        for (row_idx, row) in gray.chunks_exact(IMAGE_WIDTH).enumerate() {
            let out_row = &mut data[row_idx * MONO1_ROW_BYTES..(row_idx + 1) * MONO1_ROW_BYTES];
            for (byte_idx, oct) in row.chunks_exact(8).enumerate() {
                let mut b = 0u8;
                for (bit, &lum) in oct.iter().enumerate() {
                    if lum > threshold {
                        b |= 0x80 >> bit;
                    }
                }
                out_row[byte_idx] = b;
            }
        }
        Ok(Mono1Image { data })
    }

    /// Deterministic 2x box downsample to 128x120, returned as packed Mono1
    /// bytes (128/8 * 120 = 1,920 bytes), for on-badge preview rendering.
    ///
    /// Each output pixel is the majority of its 2x2 source block; ties
    /// (2 white / 2 black) resolve to white so thin light features survive.
    pub fn downsample_2x(&self) -> Vec<u8> {
        const OUT_W: usize = IMAGE_WIDTH / 2; // 128
        const OUT_H: usize = IMAGE_HEIGHT / 2; // 120
        const OUT_ROW_BYTES: usize = OUT_W / 8; // 16
        let mut out = vec![0u8; OUT_ROW_BYTES * OUT_H];
        for oy in 0..OUT_H {
            for ox in 0..OUT_W {
                let mut white = 0u8;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    if self.get(ox * 2 + dx, oy * 2 + dy) {
                        white += 1;
                    }
                }
                if white >= 2 {
                    out[oy * OUT_ROW_BYTES + ox / 8] |= 0x80 >> (ox % 8);
                }
            }
        }
        out
    }
}

impl core::fmt::Debug for Mono1Image {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Mono1Image({}x{})", IMAGE_WIDTH, IMAGE_HEIGHT)
    }
}

/// Width of a small-format (pixel format 2) image in pixels.
pub const SMALL_WIDTH: usize = 128;
/// Height of a small-format image in pixels.
pub const SMALL_HEIGHT: usize = 120;
/// Pixels (and bytes of 8-bit grayscale) in a small frame.
pub const SMALL_PIXELS: usize = SMALL_WIDTH * SMALL_HEIGHT; // 15,360
/// Packed length of a small 1-bit image: 128/8 * 120.
pub const SMALL_PACKED_LEN: usize = SMALL_WIDTH / 8 * SMALL_HEIGHT; // 1,920
/// Bytes per packed small row.
pub const SMALL_ROW_BYTES: usize = SMALL_WIDTH / 8; // 16

/// A small-format 1-bit image (128x120, pixel format 2) — the badge
/// display's native resolution. Same packing conventions as
/// [`Mono1Image`]. Posts in this format are ~4x smaller before
/// compression, which is what makes single-digit QR-frame shares
/// possible; nothing is lost on the 128x128 badge screen.
#[derive(Clone, PartialEq, Eq)]
pub struct Mono1Small {
    data: Box<[u8; SMALL_PACKED_LEN]>,
}

impl Mono1Small {
    /// Wrap exactly SMALL_PACKED_LEN packed bytes.
    pub fn from_packed(packed: &[u8]) -> Result<Self> {
        if packed.len() != SMALL_PACKED_LEN {
            return Err(BaogramError::BadPackedSize);
        }
        let mut data = Box::new([0u8; SMALL_PACKED_LEN]);
        data.copy_from_slice(packed);
        Ok(Mono1Small { data })
    }

    pub fn packed(&self) -> &[u8; SMALL_PACKED_LEN] {
        &self.data
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> bool {
        debug_assert!(x < SMALL_WIDTH && y < SMALL_HEIGHT);
        self.data[y * SMALL_ROW_BYTES + x / 8] & (0x80 >> (x % 8)) != 0
    }

    /// Quantize a full 256x240 grayscale frame to a small image: 2x2 box
    /// average in the grayscale domain (integer floor division), then the
    /// deterministic mean threshold over the downscaled frame. Averaging
    /// before thresholding preserves tone better than thresholding first.
    pub fn from_gray(gray: &[u8], threshold_override: Option<u8>) -> Result<Self> {
        if gray.len() != IMAGE_PIXELS {
            return Err(BaogramError::BadFrameSize);
        }
        let mut small_gray = vec![0u8; SMALL_PIXELS];
        for y in 0..SMALL_HEIGHT {
            for x in 0..SMALL_WIDTH {
                let a = gray[(y * 2) * IMAGE_WIDTH + x * 2] as u32;
                let b = gray[(y * 2) * IMAGE_WIDTH + x * 2 + 1] as u32;
                let c = gray[(y * 2 + 1) * IMAGE_WIDTH + x * 2] as u32;
                let d = gray[(y * 2 + 1) * IMAGE_WIDTH + x * 2 + 1] as u32;
                small_gray[y * SMALL_WIDTH + x] = ((a + b + c + d) / 4) as u8;
            }
        }
        let threshold = match threshold_override {
            Some(t) => t,
            None => {
                let sum: u64 = small_gray.iter().map(|&p| p as u64).sum();
                (sum / small_gray.len() as u64) as u8
            }
        };
        let mut data = Box::new([0u8; SMALL_PACKED_LEN]);
        for y in 0..SMALL_HEIGHT {
            for x in 0..SMALL_WIDTH {
                if small_gray[y * SMALL_WIDTH + x] > threshold {
                    data[y * SMALL_ROW_BYTES + x / 8] |= 0x80 >> (x % 8);
                }
            }
        }
        Ok(Mono1Small { data })
    }

    /// 3x3 majority despeckle (border coordinates clamp/replicate): a
    /// pixel becomes white iff at least 5 of the 9 samples are white.
    /// Removes the near-threshold salt-and-pepper noise that breaks
    /// run-length compression; applied at capture time, before signing.
    pub fn despeckle(&self) -> Mono1Small {
        let mut out = Box::new([0u8; SMALL_PACKED_LEN]);
        for y in 0..SMALL_HEIGHT {
            for x in 0..SMALL_WIDTH {
                let mut white = 0u32;
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let sx = (x as i32 + dx).clamp(0, SMALL_WIDTH as i32 - 1) as usize;
                        let sy = (y as i32 + dy).clamp(0, SMALL_HEIGHT as i32 - 1) as usize;
                        if self.get(sx, sy) {
                            white += 1;
                        }
                    }
                }
                if white >= 5 {
                    out[y * SMALL_ROW_BYTES + x / 8] |= 0x80 >> (x % 8);
                }
            }
        }
        Mono1Small { data: out }
    }

    /// Pixel-double to the full 256x240 format (each small pixel becomes a
    /// 2x2 block) so every full-resolution rendering path applies
    /// unchanged. Lossless with respect to the small image's content.
    pub fn to_full(&self) -> Mono1Image {
        let mut full = Mono1Image::from_packed(&[0u8; MONO1_PACKED_LEN]).unwrap();
        for y in 0..SMALL_HEIGHT {
            for x in 0..SMALL_WIDTH {
                if self.get(x, y) {
                    full.set(x * 2, y * 2, true);
                    full.set(x * 2 + 1, y * 2, true);
                    full.set(x * 2, y * 2 + 1, true);
                    full.set(x * 2 + 1, y * 2 + 1, true);
                }
            }
        }
        full
    }
}

impl core::fmt::Debug for Mono1Small {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Mono1Small({}x{})", SMALL_WIDTH, SMALL_HEIGHT)
    }
}

/// Mean luminance of the frame via integer division — the deterministic
/// global threshold used when no override is supplied.
pub fn global_threshold(gray: &[u8]) -> u8 {
    debug_assert_eq!(gray.len(), IMAGE_PIXELS);
    let sum: u64 = gray.iter().map(|&p| p as u64).sum();
    (sum / gray.len() as u64) as u8
}

/// Deterministic synthetic 256x240 grayscale test frame.
///
/// Layout (rows, top to bottom):
/// - rows 0..=59: horizontal gradient, luminance = x (0..=255);
/// - rows 60..=119: left half dark (32), right half light (224);
/// - rows 120..=179: 16x16 checkerboard of 0 and 255;
/// - rows 180..=239: vertical gradient, luminance = (y - 180) * 255 / 59.
///
/// The same function is implemented in the hosted bao-video camera stub so
/// captured hosted frames can be verified against a golden SHA-256.
pub fn synthetic_test_frame() -> Vec<u8> {
    let mut frame = vec![0u8; IMAGE_PIXELS];
    for y in 0..IMAGE_HEIGHT {
        for x in 0..IMAGE_WIDTH {
            let v: u8 = match y {
                0..=59 => x as u8,
                60..=119 => {
                    if x < IMAGE_WIDTH / 2 {
                        32
                    } else {
                        224
                    }
                }
                120..=179 => {
                    if ((x / 16) + (y / 16)) % 2 == 0 {
                        255
                    } else {
                        0
                    }
                }
                _ => ((y - 180) * 255 / 59) as u8,
            };
            frame[y * IMAGE_WIDTH + x] = v;
        }
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_len_is_7680() {
        assert_eq!(MONO1_PACKED_LEN, 7680);
        assert_eq!(IMAGE_PIXELS, 61_440);
    }

    #[test]
    fn quantize_all_black_and_all_white() {
        let black = vec![0u8; IMAGE_PIXELS];
        let img = Mono1Image::quantize(&black, None).unwrap();
        // threshold = 0; 0 > 0 is false -> all black
        assert!(img.packed().iter().all(|&b| b == 0));

        let white = vec![255u8; IMAGE_PIXELS];
        let img = Mono1Image::quantize(&white, Some(0)).unwrap();
        assert!(img.packed().iter().all(|&b| b == 0xff));

        // With mean threshold on a uniform 255 frame, threshold = 255 and
        // 255 > 255 is false: uniform frames quantize to all-black. This is
        // the documented, deterministic edge behavior.
        let img = Mono1Image::quantize(&white, None).unwrap();
        assert!(img.packed().iter().all(|&b| b == 0));
    }

    #[test]
    fn quantize_rejects_wrong_size() {
        assert_eq!(Mono1Image::quantize(&[0u8; 100], None).unwrap_err(), BaogramError::BadFrameSize);
    }

    #[test]
    fn bit_addressing_round_trip() {
        let mut img = Mono1Image::from_packed(&[0u8; MONO1_PACKED_LEN]).unwrap();
        img.set(0, 0, true);
        img.set(255, 239, true);
        img.set(7, 3, true);
        assert!(img.get(0, 0));
        assert!(img.get(255, 239));
        assert!(img.get(7, 3));
        assert!(!img.get(1, 0));
        // MSB-first: pixel x=0 is bit 7 of byte 0
        assert_eq!(img.packed()[0], 0x80);
        // pixel x=7 is bit 0
        assert_eq!(img.packed()[3 * MONO1_ROW_BYTES], 0x01);
        assert_eq!(img.packed()[MONO1_PACKED_LEN - 1] & 0x01, 0x01);
    }

    #[test]
    fn synthetic_frame_matches_golden_sha() {
        use sha2::{Digest, Sha256};
        // shared with xous-core services/bao-video/src/still.rs — the hosted
        // camera serves this exact frame
        let digest = Sha256::digest(synthetic_test_frame());
        let hex: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(hex, "c82bfcf7e7ee582e204c151af00570cb832dcddced95df56f484d1cc7d0303db");
    }

    #[test]
    fn synthetic_frame_shape() {
        let f = synthetic_test_frame();
        assert_eq!(f.len(), IMAGE_PIXELS);
        // gradient row
        assert_eq!(f[0], 0);
        assert_eq!(f[255], 255);
        // dark/light band
        assert_eq!(f[60 * IMAGE_WIDTH], 32);
        assert_eq!(f[60 * IMAGE_WIDTH + 200], 224);
        // checkerboard: at y=120, y/16 == 7 (odd), so x=0 is dark and x=16 light
        assert_eq!(f[120 * IMAGE_WIDTH], 0);
        assert_eq!(f[120 * IMAGE_WIDTH + 16], 255);
        // at y=128, y/16 == 8 (even), x=0 is light
        assert_eq!(f[128 * IMAGE_WIDTH], 255);
    }

    #[test]
    fn downsample_dimensions() {
        let img = Mono1Image::from_packed(&[0xffu8; MONO1_PACKED_LEN]).unwrap();
        let small = img.downsample_2x();
        assert_eq!(small.len(), 128 / 8 * 120);
        assert!(small.iter().all(|&b| b == 0xff));
    }
}
