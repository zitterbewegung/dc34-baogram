//! PDDB-backed post gallery.
//!
//! Dictionary `baogram`: `index` holds the ordered list of 16-byte post
//! IDs (concatenated); each post lives at `post.<32-hex-id>` as its full
//! canonical serialization. Posts are verified at parse time, deduplicated
//! by full post ID, capped at MAX_POSTS, and the index self-heals if an
//! entry references a missing or corrupt key.

use std::io::{Read, Write};

use pddb::Pddb;

use super::{BAOGRAM_DICT, KEY_INDEX, MAX_POSTS, POST_KEY_PREFIX};

/// Alloc hint: typical PackBits post is ~2.7 KiB; raw is ~7.9 KiB.
const POST_ALLOC_HINT: usize = 8192;
const INDEX_ALLOC_HINT: usize = MAX_POSTS * 16;

#[derive(Debug, PartialEq, Eq)]
pub enum SaveResult {
    Saved,
    AlreadyPresent,
    GalleryFull,
    Error,
}

pub fn post_key_name(id: &[u8; 16]) -> String {
    let hex: String = id.iter().map(|b| format!("{:02x}", b)).collect();
    format!("{}{}", POST_KEY_PREFIX, hex)
}

/// Read the ordered index. Entries whose post key is missing are dropped
/// (and the pruned index is written back), so a partially failed save or
/// delete can never wedge the feed.
pub fn load_index(pddb: &Pddb) -> Vec<[u8; 16]> {
    let raw = match pddb.get(BAOGRAM_DICT, KEY_INDEX, None, false, false, None, None::<fn()>) {
        Ok(mut key) => {
            let mut buf = Vec::new();
            match key.read_to_end(&mut buf) {
                Ok(_) => buf,
                Err(_) => return Vec::new(),
            }
        }
        Err(_) => return Vec::new(),
    };
    let mut ids: Vec<[u8; 16]> = raw.chunks_exact(16).map(|c| c.try_into().unwrap()).collect();
    // self-heal: drop ids whose post key is unreadable
    let before = ids.len();
    ids.retain(|id| {
        pddb.get(BAOGRAM_DICT, &post_key_name(id), None, false, false, None, None::<fn()>).is_ok()
    });
    if ids.len() != before {
        log::warn!("baogram: pruned {} dangling index entries", before - ids.len());
        store_index(pddb, &ids);
    }
    ids
}

fn store_index(pddb: &Pddb, ids: &[[u8; 16]]) {
    let mut raw = Vec::with_capacity(ids.len() * 16);
    for id in ids {
        raw.extend_from_slice(id);
    }
    pddb.delete_key(BAOGRAM_DICT, KEY_INDEX, None).ok();
    match pddb.get(BAOGRAM_DICT, KEY_INDEX, None, true, true, Some(INDEX_ALLOC_HINT), None::<fn()>) {
        Ok(mut key) => {
            if let Err(e) = key.write_all(&raw) {
                log::error!("baogram: couldn't write index: {:?}", e);
            }
        }
        Err(e) => log::error!("baogram: couldn't create index: {:?}", e),
    }
}

/// Save a serialized, *already verified* post. Deduplicates by post ID and
/// enforces the gallery cap. Syncs PDDB on success.
pub fn save_post(pddb: &Pddb, id: &[u8; 16], serialized: &[u8]) -> SaveResult {
    let mut ids = load_index(pddb);
    if ids.iter().any(|existing| existing == id) {
        return SaveResult::AlreadyPresent;
    }
    if ids.len() >= MAX_POSTS {
        return SaveResult::GalleryFull;
    }
    let key_name = post_key_name(id);
    match pddb.get(
        BAOGRAM_DICT,
        &key_name,
        None,
        true,
        true,
        Some(POST_ALLOC_HINT.max(serialized.len())),
        None::<fn()>,
    ) {
        Ok(mut key) => {
            if let Err(e) = key.write_all(serialized) {
                log::error!("baogram: couldn't write post {}: {:?}", key_name, e);
                pddb.delete_key(BAOGRAM_DICT, &key_name, None).ok();
                return SaveResult::Error;
            }
        }
        Err(e) => {
            log::error!("baogram: couldn't create post {}: {:?}", key_name, e);
            return SaveResult::Error;
        }
    }
    ids.push(*id);
    store_index(pddb, &ids);
    pddb.sync().ok();
    SaveResult::Saved
}

/// Load a post's raw bytes; None if missing or unreadable. Callers must
/// parse (and thereby verify) with `baogram_core::post::Post::parse` and
/// treat failures as a corrupt entry, never a panic.
pub fn load_post(pddb: &Pddb, id: &[u8; 16]) -> Option<Vec<u8>> {
    let mut key = pddb.get(BAOGRAM_DICT, &post_key_name(id), None, false, false, None, None::<fn()>).ok()?;
    let mut buf = Vec::new();
    key.read_to_end(&mut buf).ok()?;
    if buf.is_empty() || buf.len() > baogram_core::post::MAX_POST_BYTES {
        return None;
    }
    Some(buf)
}

/// Delete a post and remove it from the index. Syncs PDDB.
pub fn delete_post(pddb: &Pddb, id: &[u8; 16]) {
    pddb.delete_key(BAOGRAM_DICT, &post_key_name(id), None).ok();
    let mut ids = load_index(pddb);
    ids.retain(|existing| existing != id);
    store_index(pddb, &ids);
    pddb.sync().ok();
}
