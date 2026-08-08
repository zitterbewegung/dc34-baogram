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

/// Whether a picture uploaded over serial (the `image` console command)
/// should be staged as a post when the badge is in `mode`.
///
/// True only inside Baogram, on a screen that is not busy. Notably **false**
/// on the idle/conference screens: there the upload is the *avatar* — the
/// bitmap that alternates with the DC logo — and someone using that
/// original workflow did not ask to be dropped into a post preview. Also
/// false mid-capture, mid-share, mid-receive, and while a post is already
/// on the preview screen awaiting a decision.
pub fn upload_stageable(mode: crate::VaultMode) -> bool {
    use crate::VaultMode::*;
    match mode {
        Launcher | BaogramFeed | BaogramProfile => true,
        // busy: bao-video owns the display, or a share loop is running
        BaogramCamera | BaogramReceive | BaogramShare { .. } => false,
        // already deciding about a post
        BaogramPreview | BaogramPostMenu => false,
        // the vault side, where an upload means "set my avatar"
        _ => false,
    }
}

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
    /// Uploaded over serial with the `image` console command (dc34-image)
    /// and converted from the 128x128 avatar bitmap. Signed locally at save
    /// time exactly like a capture, but there is nothing to "retake" — the
    /// left key discards.
    Imported,
}

/// A post awaiting the user's Save/Retake/Discard decision.
pub struct Pending {
    pub source: PendingSource,
    /// For `Captured`/`Imported`: the small-format image, signed at save time.
    pub image: Option<baogram_core::image::Mono1Small>,
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

#[cfg(test)]
mod tests {
    use crate::VaultMode::*;

    use super::upload_stageable;

    /// Every mode, stated explicitly. A new VaultMode variant makes this
    /// list incomplete rather than silently inheriting a default.
    #[test]
    fn upload_stages_only_inside_baogram() {
        let allowed = [Launcher, BaogramFeed, BaogramProfile];
        let refused = [
            // the vault side: an upload here means "set my avatar"
            Idle,
            IdleDevMode,
            ShowKey { quantum: 0 },
            ResponseGene { quantum: 0 },
            ConfirmGene,
            GeneScan,
            FactoryTest,
            StandAloneTest,
            Tour,
            TokenTour,
            DefconHelp,
            About,
            Totp,
            Password,
            TokenHelp,
            // busy, or already deciding about a post
            BaogramCamera,
            BaogramPreview,
            BaogramPostMenu,
            BaogramShare { quantum: 0 },
            BaogramReceive,
        ];
        for m in allowed {
            assert!(upload_stageable(m), "{:?} should stage an upload", m);
        }
        for m in refused {
            assert!(!upload_stageable(m), "{:?} must not stage an upload", m);
        }
        assert_eq!(
            allowed.len() + refused.len(),
            23,
            "VaultMode gained or lost a variant - decide what it does with an upload"
        );
    }

    /// The idle/conference screens are the specific regression this guards:
    /// uploading an avatar there must not hijack the display.
    #[test]
    fn conference_screens_never_hijacked_by_an_upload() {
        assert!(!upload_stageable(Idle));
        assert!(!upload_stageable(IdleDevMode));
        assert!(!upload_stageable(ConfirmGene));
    }
}
