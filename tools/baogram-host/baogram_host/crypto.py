"""SHA-256 digests and domain-separated Ed25519 signatures.

Mirrors libraries/baogram-core/src/crypto.rs exactly:

* post digest = SHA256(b"BAOGRAM-POST-V1" || canonical_header || handle ||
  caption || encoded_image)
* signature   = Ed25519_sign(sk, b"BAOGRAM-SIG-V1" || digest)

Secret keys are 32-byte seeds (RFC 8032). A seed is never embedded in a
post; posts carry only the 32-byte public key.
"""

from __future__ import annotations

import hashlib

from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey,
    Ed25519PublicKey,
)
from cryptography.exceptions import InvalidSignature

POST_DIGEST_DOMAIN = b"BAOGRAM-POST-V1"
SIG_DOMAIN = b"BAOGRAM-SIG-V1"

PUBKEY_LEN = 32
SIGNATURE_LEN = 64
DIGEST_LEN = 32
POST_ID_LEN = 16
SHORT_ID_LEN = 8


class Identity:
    """A Baogram signing identity derived from a 32-byte seed."""

    def __init__(self, seed: bytes):
        if len(seed) != 32:
            raise ValueError("seed must be exactly 32 bytes")
        self._sk = Ed25519PrivateKey.from_private_bytes(seed)
        self._seed = bytes(seed)

    @property
    def public_key(self) -> bytes:
        from cryptography.hazmat.primitives.serialization import (
            Encoding,
            PublicFormat,
        )

        return self._sk.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)

    def sign_digest(self, digest: bytes) -> bytes:
        if len(digest) != DIGEST_LEN:
            raise ValueError("digest must be 32 bytes")
        return self._sk.sign(SIG_DOMAIN + digest)

    @property
    def seed(self) -> bytes:
        return self._seed


def post_digest(
    canonical_header: bytes, handle: bytes, caption: bytes, encoded_image: bytes
) -> bytes:
    h = hashlib.sha256()
    h.update(POST_DIGEST_DOMAIN)
    h.update(canonical_header)
    h.update(handle)
    h.update(caption)
    h.update(encoded_image)
    return h.digest()


def verify_digest_signature(public_key: bytes, digest: bytes, signature: bytes) -> bool:
    """True iff `signature` is a valid Ed25519 signature over the
    domain-separated digest by `public_key`."""
    if len(public_key) != PUBKEY_LEN or len(signature) != SIGNATURE_LEN:
        return False
    try:
        pk = Ed25519PublicKey.from_public_bytes(public_key)
        pk.verify(signature, SIG_DOMAIN + digest)
        return True
    except (InvalidSignature, ValueError):
        return False


def hex_fingerprint(public_key: bytes) -> str:
    """First 8 bytes of the public key as 16 lowercase hex chars."""
    return public_key[:8].hex()
