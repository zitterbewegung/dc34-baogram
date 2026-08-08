// Hosted-mode stand-ins for the dc34-console servers (power manager, LED
// manager). The baosec emulator's service list runs bao-console rather than
// dc34-console, so without these the vault would park forever in
// `request_connection_blocking` waiting for the names to appear.

use num_traits::ToPrimitive;
use xous_names::XousNames;

pub fn spawn_console_stubs() {
    std::thread::spawn(|| serve_sink(dc34_api::POWER_MANAGER_SERVER));
    std::thread::spawn(|| serve_sink(dc34_api::LED_SERVER));
}

/// When BAOGRAM_TOUR=1, walk the UI through every Baogram screen by feeding
/// the vault server the same KeyPress messages the badge keys produce:
/// idle menu -> Baogram feed -> camera -> capture -> save -> post menu ->
/// share (animated QR) -> back to the feed. Paced so a human can watch.
pub fn spawn_tour_if_requested(conn: xous::CID) {
    if std::env::var("BAOGRAM_TOUR").map(|v| v == "1").unwrap_or(false) {
        std::thread::spawn(move || run_tour(conn));
    }
}

/// When BAOGRAM_SEED=1, capture and save one photo right after boot so a
/// fresh emulator's feed starts with content instead of "No posts yet".
/// Pairs with BAO_CAMERA_IMAGE to seed a specific demo picture.
pub fn spawn_seed_if_requested(conn: xous::CID) {
    if std::env::var("BAOGRAM_SEED").map(|v| v == "1").unwrap_or(false) {
        std::thread::spawn(move || {
            // boot lands in the feed; give the UI a moment to settle
            std::thread::sleep(std::time::Duration::from_secs(3));
            log::info!("BAOGRAM SEED: capturing one photo into the feed");
            key(conn, '🔥', 2500); // feed -> camera preview (camera spin-up)
            key(conn, '🔥', 1500); // capture -> save/retake preview
            key(conn, '🔥', 1000); // save -> feed shows the new post
            log::info!("BAOGRAM SEED: done");
        });
    }
}

fn key(conn: xous::CID, k: char, wait_ms: u64) {
    xous::send_message(
        conn,
        xous::Message::new_scalar(
            crate::VaultOp::KeyPress.to_usize().unwrap(),
            k as u32 as usize,
            0,
            0,
            0,
        ),
    )
    .ok();
    std::thread::sleep(std::time::Duration::from_millis(wait_ms));
}

fn run_tour(conn: xous::CID) {
    let step = |m: &str| log::info!("BAOGRAM TOUR: {}", m);
    // boot lands directly in the Baogram feed
    std::thread::sleep(std::time::Duration::from_secs(3));
    step("camera preview (DC34 logo)");
    key(conn, '🔥', 4000);
    step("capture still");
    key(conn, '🔥', 4000);
    step("sign + save -> feed");
    key(conn, '🔥', 3000);
    step("opening post menu");
    key(conn, '∴', 2500);
    // a key can be lost right after a menu opens; clamp the highlight to
    // the top (Share) before selecting
    for _ in 0..3 {
        key(conn, '↑', 400);
    }
    step("selecting Share -> animated QR");
    key(conn, '∴', 8000);
    step("stopping share");
    key(conn, '\u{8}', 1500);
    step("done - feed shows the saved post");
}

/// Absorb every message sent to `name`. Blocking scalars are acked with
/// arg1 = 1 ("ok") and arg2 = 1; GetVbus reads arg2 as "VBUS present", so the
/// UI behaves as if the badge is plugged in and skips battery warnings.
/// Memory messages are returned untouched by `reply_and_receive_next`.
fn serve_sink(name: &str) -> ! {
    let xns = XousNames::new().unwrap();
    let sid = xns.register_name(name, None).expect("can't register hosted stub server");
    let mut msg_opt = None;
    loop {
        xous::reply_and_receive_next(sid, &mut msg_opt).unwrap();
        if let Some(scalar) = msg_opt.as_mut().unwrap().body.scalar_message_mut() {
            scalar.arg1 = 1;
            scalar.arg2 = 1;
        }
    }
}
