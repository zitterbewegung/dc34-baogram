//! # baogram-core
//!
//! Canonical formats and pure logic for **Baogram**, the signed, local,
//! full-spatial-resolution monochrome picture-sharing application for the
//! DEF CON 34 Baochip badge.
//!
//! This crate deliberately has **no** dependencies on Xous, camera drivers,
//! display services, PDDB, or the dc34 UI. Everything here compiles and
//! tests natively on a host machine; the badge application and the laptop
//! peer both build on these exact bytes-on-the-wire definitions.
//!
//! Modules:
//! - [`image`]: 256x240 Mono1 representation, deterministic quantization,
//!   preview downsampling, and the synthetic hosted-mode test frame.
//! - [`codec`]: RawMono1 and the deterministic PackBitsMono1 codec.
//! - [`post`]: the canonical `BGRM` post container (v1), parse + verify.
//! - [`fragment`]: `BG` fragments, Base45 QR text, and the bounded
//!   any-order reassembler.
//! - [`crypto`]: SHA-256 digests and domain-separated Ed25519 signatures.
//! - [`error`]: the shared error enum.
//!
//! Byte order is **big-endian** everywhere. See `BAOGRAM_PROTOCOL.md` in
//! the repository root for the normative protocol description.

pub mod codec;
pub mod crypto;
pub mod error;
pub mod fragment;
pub mod image;
pub mod post;

pub use error::{BaogramError, Result};

#[cfg(test)]
mod golden;
