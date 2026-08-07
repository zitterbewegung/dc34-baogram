//! Baogram: signed, local, full-spatial-resolution monochrome
//! picture-sharing for the DC34 badge.
//!
//! Canonical formats live in `baogram-core` (host-testable, no Xous deps);
//! this module supplies the badge-side identity, PDDB storage, camera
//! capture, feed browsing, animated-QR share/receive, and rendering. UI
//! branding is "Baogram"; internal crate/service names are unchanged.

pub mod camera;
pub mod controller;
pub mod feed;
pub mod identity;
pub mod render;
pub mod storage;
pub mod transfer;

use std::sync::{Arc, Mutex};

use baogram_core::post::Post;

/// PDDB dictionary holding all Baogram state.
pub const BAOGRAM_DICT: &str = "baogram";
/// Key: identity record (seed, public key, sequence counter, handle).
pub const KEY_IDENTITY: &str = "identity";
/// Key: ordered post index (concatenated 16-byte post IDs).
pub const KEY_INDEX: &str = "index";
/// Prefix for per-post keys: `post.<32 hex chars>`.
pub const POST_KEY_PREFIX: &str = "post.";
/// Gallery capacity for the MVP.
pub const MAX_POSTS: usize = 32;

/// Where a pending (not yet saved) post came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingSource {
    /// Captured with the local camera; not yet signed/saved. Retake allowed.
    Captured,
    /// Received over QR and signature-verified. Save or reject only.
    Received,
}

/// A post awaiting the user's Save/Retake/Discard decision.
pub struct Pending {
    pub source: PendingSource,
    /// For `Captured`: the quantized image, signed at save time.
    pub image: Option<baogram_core::image::Mono1Image>,
    /// For `Received`: the verified post and its serialized bytes.
    pub post: Option<(Post, Vec<u8>)>,
}

/// Result of a receive-worker session, delivered with VaultOp::BaogramRxDone.
pub const RX_DONE_OK: usize = 0;
pub const RX_DONE_CANCELED: usize = 1;
pub const RX_DONE_FAILED: usize = 2;

/// Cached feed rendering for the currently displayed post.
pub struct FeedCache {
    pub id: [u8; 16],
    pub bits: [u32; 512],
    pub label: String,
    /// None if the stored post failed to parse (corrupt); offer deletion.
    pub valid: bool,
}

/// All Baogram state shared between the main loop, the UI, and the
/// receive worker.
pub struct BaogramShared {
    pub identity: identity::BaogramIdentity,
    pub feed: feed::FeedState,
    pub pending: Option<Pending>,
    pub share: Option<transfer::ShareState>,
    pub feed_cache: Option<FeedCache>,
    /// (received, total) fragments for the in-progress receive.
    pub rx_progress: (usize, usize),
    /// Human-readable reason for the last receive failure.
    pub rx_error: Option<String>,
    /// True while the receive worker is running.
    pub rx_active: bool,
}

pub type Shared = Arc<Mutex<BaogramShared>>;
