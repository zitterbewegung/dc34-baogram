//! Persistent Baogram identity: Ed25519 seed, public key, monotonically
//! increasing post sequence number, and the user handle.
//!
//! Record layout in PDDB key `baogram/identity` (big-endian):
//! `[ver u8 = 1][seed 32][pubkey 32][seq u64][handle_len u8][handle ...]`
//!
//! The seed never leaves PDDB: it is not logged, not exported over QR, and
//! never serialized into a post.

use std::io::{Read, Write};

use baogram_core::crypto::{Identity, hex_fingerprint};
use baogram_core::post::HANDLE_MAX_BYTES;
use pddb::Pddb;
use rand::RngCore;

use super::{BAOGRAM_DICT, KEY_IDENTITY};

const IDENTITY_VERSION: u8 = 1;
const IDENTITY_MAX_LEN: usize = 1 + 32 + 32 + 8 + 1 + HANDLE_MAX_BYTES;

/// The default handle is `<username>-<num>`: `<username>` is the first entry
/// in the vault's configured usernames list (falling back to "bao" when none
/// is set), truncated so the whole handle fits the protocol's 24-byte cap,
/// and `<num>` is the first 8 hex chars of the key fingerprint.
fn default_handle(public_key: &[u8; 32]) -> String {
    let num = &hex_fingerprint(public_key)[..8];
    let mut name = String::from("bao");
    if let Ok(file) = std::fs::File::open(crate::VAULT_CONFIG_USERNAMES) {
        use std::io::BufRead;
        if let Some(first) =
            std::io::BufReader::new(file).lines().map_while(Result::ok).find(|l| !l.trim().is_empty())
        {
            name = first.trim().to_string();
        }
    }
    while name.len() > HANDLE_MAX_BYTES - 1 - num.len() {
        name.pop();
    }
    format!("{}-{}", name, num)
}

pub struct BaogramIdentity {
    seed: [u8; 32],
    pub public_key: [u8; 32],
    seq: u64,
    pub handle: String,
}

impl BaogramIdentity {
    /// Load the identity, creating and persisting a fresh one on first
    /// launch. The seed comes from `rand::thread_rng()`, which is backed by
    /// the hardware TRNG on the badge (via the Xous getrandom patch).
    pub fn load_or_create(pddb: &Pddb) -> BaogramIdentity {
        if let Some(mut id) = Self::load(pddb) {
            // migrate retired default handles ("anon-<fp8>", bare "<fp8>") to
            // the current default; user-chosen handles are left alone
            let num = hex_fingerprint(&id.public_key)[..8].to_string();
            let retired = [format!("anon-{}", num), num];
            let current = default_handle(&id.public_key);
            if id.handle != current && retired.contains(&id.handle) {
                id.handle = current;
                id.persist(pddb);
                log::info!("baogram: migrated default handle to {}", id.handle);
            }
            return id;
        }
        let mut seed = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut seed);
        let public_key = Identity::from_seed(&seed).public_key();
        let handle = default_handle(&public_key);
        let id = BaogramIdentity { seed, public_key, seq: 0, handle };
        id.persist(pddb);
        log::info!("baogram: created new identity {}", id.fingerprint());
        id
    }

    fn load(pddb: &Pddb) -> Option<BaogramIdentity> {
        let mut key = pddb.get(BAOGRAM_DICT, KEY_IDENTITY, None, false, false, None, None::<fn()>).ok()?;
        let mut buf = Vec::new();
        key.read_to_end(&mut buf).ok()?;
        if buf.len() < 1 + 32 + 32 + 8 + 1 || buf[0] != IDENTITY_VERSION {
            log::warn!("baogram: identity record malformed ({} bytes); regenerating", buf.len());
            return None;
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&buf[1..33]);
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(&buf[33..65]);
        // recompute the public key from the seed; if they disagree the
        // record is corrupt and the derived key wins
        let derived = Identity::from_seed(&seed).public_key();
        if derived != public_key {
            log::warn!("baogram: stored public key disagrees with seed; using derived key");
            public_key = derived;
        }
        let seq = u64::from_be_bytes(buf[65..73].try_into().unwrap());
        let handle_len = (buf[73] as usize).min(HANDLE_MAX_BYTES);
        let mut handle = if buf.len() >= 74 + handle_len {
            // lossy conversion can EXPAND invalid bytes into 3-byte
            // replacement chars; re-truncate so a corrupt record can never
            // exceed the protocol bound and block posting forever
            String::from_utf8_lossy(&buf[74..74 + handle_len]).into_owned()
        } else {
            String::new()
        };
        while handle.len() > HANDLE_MAX_BYTES {
            handle.pop();
        }
        Some(BaogramIdentity { seed, public_key, seq, handle })
    }

    fn persist(&self, pddb: &Pddb) {
        let mut rec = Vec::with_capacity(IDENTITY_MAX_LEN);
        rec.push(IDENTITY_VERSION);
        rec.extend_from_slice(&self.seed);
        rec.extend_from_slice(&self.public_key);
        rec.extend_from_slice(&self.seq.to_be_bytes());
        let hbytes = self.handle.as_bytes();
        let hlen = hbytes.len().min(HANDLE_MAX_BYTES);
        rec.push(hlen as u8);
        rec.extend_from_slice(&hbytes[..hlen]);
        // update = delete-then-recreate, matching vault storage conventions
        pddb.delete_key(BAOGRAM_DICT, KEY_IDENTITY, None).ok();
        match pddb.get(BAOGRAM_DICT, KEY_IDENTITY, None, true, true, Some(IDENTITY_MAX_LEN), None::<fn()>)
        {
            Ok(mut key) => {
                if let Err(e) = key.write_all(&rec) {
                    log::error!("baogram: couldn't write identity: {:?}", e);
                }
            }
            Err(e) => log::error!("baogram: couldn't create identity key: {:?}", e),
        }
        pddb.sync().ok();
    }

    /// The signing identity (keypair re-derived from the seed).
    pub fn signer(&self) -> Identity { Identity::from_seed(&self.seed) }

    /// Take the next post sequence number (persistent, monotonic).
    pub fn next_seq(&mut self, pddb: &Pddb) -> u64 {
        self.seq += 1;
        self.persist(pddb);
        self.seq
    }

    pub fn seq(&self) -> u64 { self.seq }

    /// 16-hex-char fingerprint of the public key.
    pub fn fingerprint(&self) -> String { hex_fingerprint(&self.public_key) }

    /// Change the handle (truncated to the protocol bound) and persist.
    #[allow(dead_code)]
    pub fn set_handle(&mut self, pddb: &Pddb, handle: &str) {
        let mut h = handle.to_string();
        while h.len() > HANDLE_MAX_BYTES {
            h.pop();
        }
        self.handle = h;
        self.persist(pddb);
    }
}

impl core::fmt::Debug for BaogramIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // never print the seed
        write!(f, "BaogramIdentity(fp={}, seq={}, handle={:?})", self.fingerprint(), self.seq, self.handle)
    }
}
