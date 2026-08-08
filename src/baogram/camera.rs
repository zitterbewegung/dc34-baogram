//! Camera capture: drive the generic bao-video still-camera API and
//! quantize the result to a small-format (128x120) Mono1 image — the
//! badge display's native resolution, and ~4x less data to share.

use baogram_core::image::{IMAGE_PIXELS, Mono1Small};
use ux_api::service::gfx::Gfx;

/// Debug metrics from a capture, logged (never displayed) for tuning.
pub struct CaptureMetrics {
    pub capture_ms: u64,
    pub read_ms: u64,
    pub quantize_ms: u64,
    pub threshold: u8,
}

/// Start the live camera preview (bao-video blits frames to the OLED).
pub fn preview_start(gfx: &Gfx) -> Result<(), xous::Error> { gfx.camera_preview_start() }

/// Stop the preview without capturing. Must be called on every exit path
/// from camera mode.
pub fn preview_stop(gfx: &Gfx) { gfx.camera_preview_stop().ok(); }

/// Freeze the next frame, read all 61,440 grayscale bytes, box-average
/// down to 128x120, quantize, and despeckle (3x3 majority — removes the
/// near-threshold noise that both looks bad and breaks compression). The
/// camera is powered down by the freeze on the server side. The raw
/// grayscale buffer is dropped as soon as quantization completes.
pub fn capture_mono1(gfx: &Gfx) -> Result<(Mono1Small, CaptureMetrics), &'static str> {
    let tt = ticktimer_server::Ticktimer::new().unwrap();
    let t0 = tt.elapsed_ms();
    let info = gfx.camera_still_capture().map_err(|_| "capture IPC failed")?;
    if !info.valid {
        return Err("capture rejected (no preview active)");
    }
    let t1 = tt.elapsed_ms();
    let frame = gfx.camera_still_read_frame(&info).map_err(|_| "frame read failed")?;
    if frame.len() != IMAGE_PIXELS {
        return Err("frame length mismatch");
    }
    let t2 = tt.elapsed_ms();
    let threshold = baogram_core::image::global_threshold(&frame);
    let image = Mono1Small::from_gray(&frame, None).map_err(|_| "quantize failed")?.despeckle();
    let t3 = tt.elapsed_ms();
    // `frame` (61,440 bytes) drops here; only the 1,920-byte small image remains
    let metrics = CaptureMetrics {
        capture_ms: t1 - t0,
        read_ms: t2 - t1,
        quantize_ms: t3 - t2,
        threshold,
    };
    log::info!(
        "baogram capture: freeze {} ms, read {} ms, quantize {} ms, threshold {}",
        metrics.capture_ms,
        metrics.read_ms,
        metrics.quantize_ms,
        metrics.threshold
    );
    Ok((image, metrics))
}
