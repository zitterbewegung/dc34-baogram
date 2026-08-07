# Baogram security notes

## Trust model

A Baogram post is **authenticated, not confidential**: anyone can read
it (it's literally shown on screens as QR codes); the signature proves
it was created by the holder of the author key and has not been
modified. There is no key certification or web-of-trust in the MVP: a
public key is an identity, and the handle is a self-asserted label —
display the fingerprint when it matters.

## Keys

* The badge identity is a 32-byte Ed25519 seed generated on first launch
  from `rand::thread_rng()`, which on the badge is backed by the
  hardware TRNG through the Xous `getrandom` implementation (the same
  path the vault already relies on for nonces and FIDO keys).
* The seed lives only in the PDDB (`baogram/identity`), which is
  encrypted at rest by the existing PDDB mechanisms. It is never logged
  (the identity's `Debug` prints only the fingerprint), never exported
  over QR, and never serialized into a post.
* The laptop peer generates its own local seed (0600 file). Never copy a
  badge seed to a host; the tooling provides no way to do so.
* Signatures are domain-separated (`"BAOGRAM-SIG-V1" || digest`), so a
  Baogram signature cannot be confused with any other Ed25519 use of the
  same key, and the digest itself is domain-prefixed
  (`"BAOGRAM-POST-V1"`), so no other SHA-256 use collides with it.

## Input validation (hostile-fragment resistance)

Received data is attacker-controlled. Defenses, in order of contact:

1. **QR layer**: rqrr decode; garbage that isn't Base45 or isn't a `BG`
   container is ignored (scanning continues).
2. **Fragment layer**: CRC-32 checked before anything else is trusted;
   fragment geometry (count == ceil(total/chunk), index range, exact
   payload length, container length) is over-determined and checked per
   fragment; totals are bounded (post <= 65,536 B, count <= 4,096,
   chunk <= 1,024) **before** the single reassembly-buffer allocation;
   receipt is tracked in a fixed bitmap. Exact duplicates are ignored;
   conflicting duplicates abort the transfer (an attacker racing a
   legitimate sender can cause a visible failure, but cannot splice
   content into another author's post undetected).
3. **Post layer**: strict canonical parse (every length validated before
   use, no trailing bytes, exactly one encoding), digest recomputation,
   Ed25519 verification, and image decode to exactly 7,680 bytes with
   bounded decoder state. Codec output is capped by the expected length,
   so a malicious PackBits stream cannot expand beyond 7,680 bytes.
4. **Storage**: only verified posts are written; posts are re-verified
   on display, and a corrupt stored post renders a placeholder (never a
   panic) and can be deleted.

## Denial-of-service bounds

* Reassembly memory is bounded by MAX_POST_BYTES + fixed bitmap
  regardless of what the fragments claim.
* The QR stream queue in bao-video is bounded (8 entries,
  oldest-dropped) with consecutive-duplicate suppression.
* The gallery is capped at 32 posts; the index self-heals.

## What the MVP does not defend against

* **Replay/re-share**: posts are public signed artifacts; anyone who
  received one can re-share it byte-identically. The post ID makes
  duplicates a no-op on receivers that already hold it.
* **Sequence-number semantics**: `seq` is informative (monotonic per
  author, gaps allowed); receivers do not enforce ordering.
* **Metadata privacy**: handles, captions, and images are cleartext by
  design.
* **Physical attacks on the badge** and PDDB key extraction are covered
  by the platform's existing threat model, not by Baogram.

## Development-signing caveat

Installing a developer-signed image erases the factory master key and
puts the badge permanently into developer mode (see the warning in the
dc34-vault README and BAOGRAM_HARDWARE_TEST.md). Nothing in this
repository flashes hardware automatically; all flashing instructions
are manual and carry that warning.
