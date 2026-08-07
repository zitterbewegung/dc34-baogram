//! Crypto helpers: SHA-256 digests and Ed25519 signatures with domain
//! separation.
//!
//! The post digest is computed as
//! `SHA256("BAOGRAM-POST-V1" || canonical_header || handle || caption || encoded_image)`
//! and the Ed25519 signature is computed over the domain-separated message
//! `"BAOGRAM-SIG-V1" || digest` — never over the raw digest, so a Baogram
//! signature can never be confused with a signature over other content.
//!
//! Secret keys are 32-byte seeds (RFC 8032). The seed is never serialized
//! into a post; posts carry only the 32-byte public key.

use ed25519_compact::{KeyPair, Noise, PublicKey, SecretKey, Seed, Signature};
use sha2::{Digest, Sha256};

use crate::error::{BaogramError, Result};

/// Domain-separation prefix for the post digest.
pub const POST_DIGEST_DOMAIN: &[u8] = b"BAOGRAM-POST-V1";
/// Domain-separation prefix for the signature message.
pub const SIG_DOMAIN: &[u8] = b"BAOGRAM-SIG-V1";

/// Length of an Ed25519 public key in a post.
pub const PUBKEY_LEN: usize = 32;
/// Length of an Ed25519 signature in a post.
pub const SIGNATURE_LEN: usize = 64;
/// Length of the SHA-256 digest in a post.
pub const DIGEST_LEN: usize = 32;
/// Length of a full post ID (digest prefix).
pub const POST_ID_LEN: usize = 16;
/// Length of the short post ID used in fragments.
pub const SHORT_ID_LEN: usize = 8;

/// A Baogram signing identity: an Ed25519 keypair derived from a 32-byte seed.
pub struct Identity {
    keypair: KeyPair,
}

impl Identity {
    /// Derive the keypair from a 32-byte seed. Deterministic (RFC 8032).
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Identity { keypair: KeyPair::from_seed(Seed::new(*seed)) }
    }

    /// The 32-byte public key.
    pub fn public_key(&self) -> [u8; PUBKEY_LEN] {
        *self.keypair.pk
    }

    /// Sign a post digest (domain-separated). Ed25519 is deterministic, so
    /// no RNG is required.
    pub fn sign_digest(&self, digest: &[u8; DIGEST_LEN]) -> [u8; SIGNATURE_LEN] {
        let mut msg = Vec::with_capacity(SIG_DOMAIN.len() + DIGEST_LEN);
        msg.extend_from_slice(SIG_DOMAIN);
        msg.extend_from_slice(digest);
        *self.keypair.sk.sign(&msg, None::<Noise>)
    }

    /// Access the raw secret seed (for persistence by the identity store).
    /// Callers must never place this in a post, log, or QR code.
    pub fn seed(&self) -> [u8; 32] {
        *self.keypair.sk.seed()
    }
}

impl core::fmt::Debug for Identity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // never print secret material
        write!(f, "Identity(pk={})", hex_fingerprint(&self.public_key()))
    }
}

/// Verify a signature over a post digest with the author public key.
pub fn verify_digest_signature(
    public_key: &[u8; PUBKEY_LEN],
    digest: &[u8; DIGEST_LEN],
    signature: &[u8; SIGNATURE_LEN],
) -> Result<()> {
    let pk = PublicKey::new(*public_key);
    let mut msg = Vec::with_capacity(SIG_DOMAIN.len() + DIGEST_LEN);
    msg.extend_from_slice(SIG_DOMAIN);
    msg.extend_from_slice(digest);
    let sig = Signature::new(*signature);
    pk.verify(&msg, &sig).map_err(|_| BaogramError::InvalidSignature)
}

/// Compute the canonical post digest over the domain prefix and the
/// canonical byte regions of the post.
pub fn post_digest(
    canonical_header: &[u8],
    handle: &[u8],
    caption: &[u8],
    encoded_image: &[u8],
) -> [u8; DIGEST_LEN] {
    let mut h = Sha256::new();
    h.update(POST_DIGEST_DOMAIN);
    h.update(canonical_header);
    h.update(handle);
    h.update(caption);
    h.update(encoded_image);
    h.finalize().into()
}

/// Short hex fingerprint of a public key (first 8 bytes, 16 hex chars) for
/// display and for the default handle (its first 8 hex chars).
pub fn hex_fingerprint(public_key: &[u8; PUBKEY_LEN]) -> String {
    public_key[..8].iter().map(|b| format!("{:02x}", b)).collect()
}

/// Re-derive a secret key object from a seed for interop tests.
pub fn secret_key_from_seed(seed: &[u8; 32]) -> SecretKey {
    KeyPair::from_seed(Seed::new(*seed)).sk
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SEED: [u8; 32] = *b"BAOGRAM-TEST-SEED-0000000000001!";

    #[test]
    fn sign_verify_round_trip() {
        let id = Identity::from_seed(&TEST_SEED);
        let digest = [7u8; 32];
        let sig = id.sign_digest(&digest);
        verify_digest_signature(&id.public_key(), &digest, &sig).unwrap();
    }

    #[test]
    fn wrong_digest_fails() {
        let id = Identity::from_seed(&TEST_SEED);
        let sig = id.sign_digest(&[7u8; 32]);
        assert_eq!(
            verify_digest_signature(&id.public_key(), &[8u8; 32], &sig).unwrap_err(),
            BaogramError::InvalidSignature
        );
    }

    #[test]
    fn flipped_sig_fails() {
        let id = Identity::from_seed(&TEST_SEED);
        let digest = [7u8; 32];
        let mut sig = id.sign_digest(&digest);
        sig[0] ^= 1;
        assert_eq!(
            verify_digest_signature(&id.public_key(), &digest, &sig).unwrap_err(),
            BaogramError::InvalidSignature
        );
    }

    #[test]
    fn deterministic_keys() {
        let a = Identity::from_seed(&TEST_SEED);
        let b = Identity::from_seed(&TEST_SEED);
        assert_eq!(a.public_key(), b.public_key());
        assert_eq!(a.sign_digest(&[1u8; 32]), b.sign_digest(&[1u8; 32]));
    }

    #[test]
    fn debug_never_prints_seed() {
        let id = Identity::from_seed(&TEST_SEED);
        let s = format!("{:?}", id);
        let seed_hex: String = TEST_SEED.iter().map(|b| format!("{:02x}", b)).collect();
        assert!(!s.contains(&seed_hex));
        assert!(!s.contains("BAOGRAM-TEST-SEED"));
    }
}
