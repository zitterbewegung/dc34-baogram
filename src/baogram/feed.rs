//! Feed browsing state: an ordered gallery position over the post index.

use pddb::Pddb;

use super::storage;

pub struct FeedState {
    ids: Vec<[u8; 16]>,
    pos: usize,
}

impl FeedState {
    pub fn new() -> FeedState { FeedState { ids: Vec::new(), pos: 0 } }

    /// Reload the index from PDDB, clamping the position.
    pub fn refresh(&mut self, pddb: &Pddb) {
        self.ids = storage::load_index(pddb);
        if self.pos >= self.ids.len() {
            self.pos = self.ids.len().saturating_sub(1);
        }
    }

    pub fn len(&self) -> usize { self.ids.len() }

    pub fn is_empty(&self) -> bool { self.ids.is_empty() }

    /// 1-based position label, e.g. "3/12".
    pub fn position_label(&self) -> String {
        if self.ids.is_empty() { "0/0".to_string() } else { format!("{}/{}", self.pos + 1, self.ids.len()) }
    }

    pub fn current(&self) -> Option<[u8; 16]> { self.ids.get(self.pos).copied() }

    pub fn next(&mut self) {
        if !self.ids.is_empty() {
            self.pos = (self.pos + 1) % self.ids.len();
        }
    }

    pub fn prev(&mut self) {
        if !self.ids.is_empty() {
            self.pos = (self.pos + self.ids.len() - 1) % self.ids.len();
        }
    }

    /// Jump to the newest (last-saved) post.
    pub fn to_latest(&mut self) {
        if !self.ids.is_empty() {
            self.pos = self.ids.len() - 1;
        }
    }
}
