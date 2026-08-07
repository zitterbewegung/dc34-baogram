//! Animated-QR share state and the receive worker.
//!
//! Share: the serialized post is held once; fragments are materialized one
//! at a time (Base45 + one QrCode each) — no precomputed QR matrices.
//!
//! Receive: a worker thread drives the continuous QR stream, feeds
//! Base45 fragments into the bounded reassembler, publishes progress, and
//! verifies the finished post before anything is offered for saving.

use baogram_core::error::BaogramError;
use baogram_core::fragment::{Fragment, FragmentIter, Reassembler};
use baogram_core::post::Post;
use num_traits::ToPrimitive;
use qrcode::QrCode;
use ux_api::service::gfx::Gfx;

use super::{Pending, PendingSource, RX_DONE_CANCELED, RX_DONE_FAILED, RX_DONE_OK, Shared};
use crate::VaultOp;

/// Diagnostic payload sizes cycled with the jog dial in share mode.
const PAYLOAD_STEPS: [usize; 5] = [24, 40, 64, 80, 96];
/// Diagnostic frame periods (ms) cycled with the jog dial in share mode.
const PERIOD_STEPS: [u32; 4] = [250, 400, 500, 750];
/// Conservative defaults until measured on hardware.
pub const DEFAULT_PAYLOAD: usize = 64;
pub const DEFAULT_PERIOD_MS: u32 = 500;
/// The redraw pump ticks roughly every 250 ms (see totp::pumper).
const PUMP_MS: u32 = 250;

pub struct ShareState {
    post_bytes: Vec<u8>,
    short_id: [u8; 8],
    pub payload_bytes: usize,
    pub period_ms: u32,
    pub frag_idx: usize,
    /// Set after a diagnostics change so the UI can flash the new settings.
    pub settings_dirty: bool,
}

impl ShareState {
    pub fn new(post: &Post, post_bytes: Vec<u8>) -> ShareState {
        let count = post_bytes.len().div_ceil(DEFAULT_PAYLOAD);
        log::info!(
            "baogram share: {} bytes -> {} fragments of <= {} bytes @ {} ms",
            post_bytes.len(),
            count,
            DEFAULT_PAYLOAD,
            DEFAULT_PERIOD_MS
        );
        ShareState {
            short_id: post.short_id(),
            post_bytes,
            payload_bytes: DEFAULT_PAYLOAD,
            period_ms: DEFAULT_PERIOD_MS,
            frag_idx: 0,
            settings_dirty: false,
        }
    }

    pub fn count(&self) -> usize { self.post_bytes.len().div_ceil(self.payload_bytes) }

    pub fn short_id_hex(&self) -> String { self.short_id.iter().map(|b| format!("{:02x}", b)).collect() }

    /// Lazily build the QR code for the current fragment. One fragment and
    /// one QrCode exist at a time.
    pub fn current_qr(&self) -> Option<QrCode> {
        let iter = FragmentIter::new(&self.short_id, &self.post_bytes, self.payload_bytes).ok()?;
        let frag = iter.fragment_at(self.frag_idx)?;
        let encoded = frag.to_base45();
        match QrCode::with_error_correction_level(encoded.as_bytes(), qrcode::EcLevel::M) {
            Ok(code) => Some(code),
            Err(e) => {
                log::error!("baogram share: QR build failed: {:?}", e);
                None
            }
        }
    }

    pub fn advance(&mut self) { self.frag_idx = (self.frag_idx + 1) % self.count(); }

    /// Pump quanta to hold each fragment on screen.
    pub fn quanta_per_frame(&self) -> u32 { (self.period_ms / PUMP_MS).max(1) }

    /// Cycle the fragment payload size (diagnostics). Restarts the loop
    /// because the fragment count changes.
    pub fn cycle_payload(&mut self) {
        let pos = PAYLOAD_STEPS.iter().position(|&p| p == self.payload_bytes).unwrap_or(0);
        self.payload_bytes = PAYLOAD_STEPS[(pos + 1) % PAYLOAD_STEPS.len()];
        self.frag_idx = 0;
        self.settings_dirty = true;
        log::info!(
            "baogram share diagnostics: payload {} bytes -> {} fragments",
            self.payload_bytes,
            self.count()
        );
    }

    /// Cycle the display period (diagnostics).
    pub fn cycle_period(&mut self) {
        let pos = PERIOD_STEPS.iter().position(|&p| p == self.period_ms).unwrap_or(2);
        self.period_ms = PERIOD_STEPS[(pos + 1) % PERIOD_STEPS.len()];
        self.settings_dirty = true;
        log::info!("baogram share diagnostics: period {} ms", self.period_ms);
    }
}

enum RxOutcome {
    Complete,
    Canceled,
    Failed(&'static str),
}

/// Start the continuous QR stream and reassemble fragments until the
/// transfer completes, the user cancels (any key aborts the stream in
/// bao-video), or a fatal error occurs. The camera is stopped on every
/// exit path. A verified post is left in `shared.pending`; the main loop
/// is woken with VaultOp::BaogramRxDone.
pub fn spawn_receive_worker(shared: Shared, main_conn: xous::CID) {
    std::thread::spawn(move || {
        let xns = xous_names::XousNames::new().unwrap();
        let gfx = Gfx::new(&xns).unwrap();
        let tt = ticktimer_server::Ticktimer::new().unwrap();
        {
            let mut s = shared.lock().unwrap();
            s.rx_progress = (0, 0);
            s.rx_error = None;
            s.rx_active = true;
        }
        let done_code;
        if gfx.qr_stream_start().is_err() {
            log::warn!("baogram receive: camera busy");
            shared.lock().unwrap().rx_error = Some("camera busy".to_string());
            done_code = RX_DONE_FAILED;
        } else {
            gfx.qr_stream_set_status("Baogram: scanning...").ok();
            let mut reassembler: Option<Reassembler> = None;
            let mut decode_stamps: Vec<u64> = Vec::new();
            let outcome = loop {
                match gfx.qr_stream_read() {
                    Ok(Some(text)) => {
                        decode_stamps.push(tt.elapsed_ms());
                        match Fragment::from_base45(text.trim()) {
                            Ok(frag) => {
                                // A fragment from a *different* post (another
                                // badge sharing nearby) is not part of this
                                // transfer: skip it rather than aborting.
                                // Conflicts within the same post ID remain
                                // fatal below.
                                if let Some(r) = reassembler.as_ref() {
                                    if frag.short_post_id != r.short_post_id() {
                                        log::info!(
                                            "baogram receive: ignoring fragment from other post {:x?}",
                                            &frag.short_post_id[..4]
                                        );
                                        continue;
                                    }
                                }
                                let feed_result = match reassembler.as_mut() {
                                    None => match Reassembler::new(&frag) {
                                        Ok((r, res)) => {
                                            reassembler = Some(r);
                                            Ok(res)
                                        }
                                        Err(e) => Err(e),
                                    },
                                    Some(r) => r.feed(&frag),
                                };
                                match feed_result {
                                    Ok(_) => {
                                        let r = reassembler.as_ref().unwrap();
                                        let progress = (r.received_count(), r.total_count());
                                        shared.lock().unwrap().rx_progress = progress;
                                        gfx.qr_stream_set_status(&format!(
                                            "Baogram {}/{}",
                                            progress.0, progress.1
                                        ))
                                        .ok();
                                        if r.is_complete() {
                                            break RxOutcome::Complete;
                                        }
                                    }
                                    Err(BaogramError::FragmentConflict) => {
                                        break RxOutcome::Failed("conflicting fragments");
                                    }
                                    Err(e) => {
                                        // malformed but non-conflicting: skip it
                                        log::debug!("baogram receive: dropping fragment: {:?}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                // Not a Baogram fragment (or CRC failure):
                                // ignore and keep scanning.
                                log::debug!("baogram receive: ignoring QR: {:?}", e);
                            }
                        }
                    }
                    Ok(None) => break RxOutcome::Canceled,
                    Err(_) => break RxOutcome::Failed("stream read error"),
                }
            };
            // stop on every exit path (idempotent if a key already aborted)
            gfx.qr_stream_stop().ok();
            if decode_stamps.len() > 1 {
                let total: u64 = decode_stamps.last().unwrap() - decode_stamps.first().unwrap();
                log::info!(
                    "baogram receive: {} decodes, avg interval {} ms",
                    decode_stamps.len(),
                    total / (decode_stamps.len() as u64 - 1)
                );
            }
            done_code = match outcome {
                RxOutcome::Complete => {
                    let bytes = reassembler.take().unwrap().into_bytes().unwrap();
                    let t0 = tt.elapsed_ms();
                    match Post::parse(&bytes) {
                        Ok(post) => {
                            log::info!(
                                "baogram receive: verified post {:x?} in {} ms ({} bytes)",
                                &post.post_id()[..4],
                                tt.elapsed_ms() - t0,
                                bytes.len()
                            );
                            let mut s = shared.lock().unwrap();
                            s.pending = Some(Pending {
                                source: PendingSource::Received,
                                image: None,
                                post: Some((post, bytes)),
                            });
                            RX_DONE_OK
                        }
                        Err(e) => {
                            log::warn!("baogram receive: post rejected: {:?}", e);
                            shared.lock().unwrap().rx_error = Some(format!("invalid post: {:?}", e));
                            RX_DONE_FAILED
                        }
                    }
                }
                RxOutcome::Canceled => RX_DONE_CANCELED,
                RxOutcome::Failed(reason) => {
                    log::warn!("baogram receive failed: {}", reason);
                    shared.lock().unwrap().rx_error = Some(reason.to_string());
                    RX_DONE_FAILED
                }
            };
        }
        shared.lock().unwrap().rx_active = false;
        xous::send_message(
            main_conn,
            xous::Message::new_scalar(VaultOp::BaogramRxDone.to_usize().unwrap(), done_code, 0, 0, 0),
        )
        .ok();
    });
}
