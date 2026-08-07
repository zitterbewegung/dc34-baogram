//! High-level operations the main loop and UI call: init, save, delete,
//! and share preparation. Receive lives in `transfer::spawn_receive_worker`.

use std::sync::{Arc, Mutex};

use baogram_core::post::Post;
use pddb::Pddb;

use super::feed::FeedState;
use super::identity::BaogramIdentity;
use super::storage::{self, SaveResult};
use super::transfer::ShareState;
use super::{BaogramShared, Pending, PendingSource, Shared};

/// Load (or create) the identity and feed state. Called once after the
/// PDDB is mounted.
pub fn init(pddb: &Pddb) -> Shared {
    let identity = BaogramIdentity::load_or_create(pddb);
    log::info!("baogram: identity {:?}", identity);
    let mut feed = FeedState::new();
    feed.refresh(pddb);
    Arc::new(Mutex::new(BaogramShared {
        identity,
        feed,
        pending: None,
        share: None,
        feed_cache: None,
        rx_progress: (0, 0),
        rx_error: None,
        rx_active: false,
    }))
}

/// Sign (if locally captured) and persist the pending post. Returns a
/// short status string for the UI.
pub fn save_pending(shared: &Shared, pddb: &Pddb) -> String {
    let mut s = shared.lock().unwrap();
    let Some(pending) = s.pending.take() else {
        return "nothing to save".to_string();
    };
    let (post_id, serialized) = match pending.source {
        PendingSource::Captured => {
            let Some(image) = pending.image else {
                return "internal error: no image".to_string();
            };
            let tt = ticktimer_server::Ticktimer::new().unwrap();
            let seq = s.identity.next_seq(pddb);
            let handle = s.identity.handle.clone();
            let signer = s.identity.signer();
            let t0 = tt.elapsed_ms();
            // caption is empty in the MVP capture flow
            let post = match Post::create(&signer, seq, &handle, "", &image) {
                Ok(p) => p,
                Err(e) => return format!("sign failed: {:?}", e),
            };
            let t1 = tt.elapsed_ms();
            let serialized = post.serialize();
            log::info!(
                "baogram save: raw {} -> encoded {} bytes (ratio {:.2}), sign+digest {} ms, total {} bytes",
                baogram_core::image::MONO1_PACKED_LEN,
                post.encoded_image.len(),
                post.encoded_image.len() as f32 / baogram_core::image::MONO1_PACKED_LEN as f32,
                t1 - t0,
                serialized.len()
            );
            (post.post_id(), serialized)
        }
        PendingSource::Received => {
            let Some((post, bytes)) = pending.post else {
                return "internal error: no post".to_string();
            };
            (post.post_id(), bytes)
        }
    };
    let result = storage::save_post(pddb, &post_id, &serialized);
    match result {
        SaveResult::Saved => {
            s.feed.refresh(pddb);
            s.feed.to_latest();
            s.feed_cache = None;
            "saved".to_string()
        }
        SaveResult::AlreadyPresent => {
            s.feed.refresh(pddb);
            "already in gallery".to_string()
        }
        SaveResult::GalleryFull => {
            format!("gallery full ({} posts) - delete one first", super::MAX_POSTS)
        }
        SaveResult::Error => "save failed".to_string(),
    }
}

/// Stash a freshly captured image as the pending post.
pub fn set_pending_capture(shared: &Shared, image: baogram_core::image::Mono1Image) {
    let mut s = shared.lock().unwrap();
    s.pending = Some(Pending { source: PendingSource::Captured, image: Some(image), post: None });
}

/// Drop the pending post (retake or discard).
pub fn clear_pending(shared: &Shared) { shared.lock().unwrap().pending = None; }

/// Delete the currently displayed post from the gallery.
pub fn delete_current(shared: &Shared, pddb: &Pddb) -> String {
    let mut s = shared.lock().unwrap();
    let Some(id) = s.feed.current() else {
        return "nothing to delete".to_string();
    };
    storage::delete_post(pddb, &id);
    s.feed.refresh(pddb);
    s.feed_cache = None;
    "deleted".to_string()
}

/// Prepare the share state for the currently displayed post. Returns false
/// (with no state change) if the post is missing or fails verification.
pub fn start_share(shared: &Shared, pddb: &Pddb) -> bool {
    let mut s = shared.lock().unwrap();
    let Some(id) = s.feed.current() else {
        return false;
    };
    let Some(bytes) = storage::load_post(pddb, &id) else {
        return false;
    };
    match Post::parse(&bytes) {
        Ok(post) => {
            s.share = Some(ShareState::new(&post, bytes));
            true
        }
        Err(e) => {
            log::warn!("baogram: refusing to share corrupt post: {:?}", e);
            false
        }
    }
}

/// Tear down the share state.
pub fn stop_share(shared: &Shared) { shared.lock().unwrap().share = None; }

/// Author fingerprint of the currently displayed post, for the post menu.
pub fn current_author_info(shared: &Shared, pddb: &Pddb) -> String {
    let s = shared.lock().unwrap();
    let Some(id) = s.feed.current() else {
        return "no post selected".to_string();
    };
    let Some(bytes) = storage::load_post(pddb, &id) else {
        return "post unreadable".to_string();
    };
    match Post::parse(&bytes) {
        Ok(post) => {
            let fp = baogram_core::crypto::hex_fingerprint(&post.author_pubkey);
            let own = if post.author_pubkey == s.identity.public_key { " (you)" } else { "" };
            format!(
                "author: {}{}\nfingerprint: {}\nseq: {}\npost: {}",
                if post.handle.is_empty() { "anon" } else { &post.handle },
                own,
                fp,
                post.seq,
                post.post_id().iter().map(|b| format!("{:02x}", b)).collect::<String>()
            )
        }
        Err(e) => format!("corrupt post: {:?}", e),
    }
}
