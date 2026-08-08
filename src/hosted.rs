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

/// When BAOGRAM_IMPORT_TEST=1, run the serial-upload-becomes-a-post test
/// end to end against the live app: write a known 128x128 bitmap into the
/// `dc34:image` PDDB key exactly as the `image` console command does, ring
/// `VaultOp::ImageLoad`, press 🔥 to accept the staged post, then read the
/// post back out of the gallery and compare its pixels to the upload.
///
/// Emits `BAOGRAM IMPORT TEST: PASS`/`FAIL <reason>` for a harness to grep,
/// and exits the process with 0/1 when BAOGRAM_IMPORT_TEST_EXIT=1.
pub fn spawn_import_test_if_requested(conn: xous::CID) {
    if std::env::var("BAOGRAM_IMPORT_TEST").map(|v| v == "1").unwrap_or(false) {
        std::thread::spawn(move || run_import_test(conn));
    }
}

fn run_import_test(conn: xous::CID) {
    use std::io::Write;

    use pddb::Pddb;

    let case = |name: &str, ok: bool, detail: &str| {
        log::info!(
            "BAOGRAM IMPORT TEST: [{}] {}{}{}",
            if ok { "ok" } else { "FAILED" },
            name,
            if detail.is_empty() { "" } else { " - " },
            detail
        );
        ok
    };
    let finish = |ok: bool, reason: &str| -> ! {
        if ok {
            log::info!("BAOGRAM IMPORT TEST: PASS");
        } else {
            log::error!("BAOGRAM IMPORT TEST: FAIL {}", reason);
        }
        if std::env::var("BAOGRAM_IMPORT_TEST_EXIT").map(|v| v == "1").unwrap_or(false) {
            // give the log server a moment to drain before tearing down
            std::thread::sleep(std::time::Duration::from_millis(500));
            std::process::exit(if ok { 0 } else { 1 });
        }
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    };

    // boot lands in the feed; let the UI settle
    std::thread::sleep(std::time::Duration::from_secs(4));
    let pddb = Pddb::new();
    let before = crate::baogram::storage::load_index(&pddb);
    log::info!("BAOGRAM IMPORT TEST: starting, {} post(s) in the gallery", before.len());

    // --- 1. the upload itself: 2,048 bytes into dc34:image ---------------
    let bits = crate::baogram::render::import_test_pattern();
    let raw: Vec<u8> = bits.iter().flat_map(|w| w.to_ne_bytes()).collect();
    if raw.len() != 2048 {
        finish(false, "test bitmap is not 2048 bytes");
    }
    match pddb.get(
        dc34_api::DC34_DICT,
        dc34_api::DC34_IMAGE,
        None,
        true,
        true,
        Some(2048),
        None::<fn()>,
    ) {
        Ok(mut k) => {
            if k.write_all(&raw).is_err() {
                finish(false, "could not write the dc34:image key");
            }
            pddb.sync().ok();
        }
        Err(_) => finish(false, "could not open the dc34:image key"),
    }

    // --- 2. the notification the console sends after the last chunk ------
    xous::send_message(
        conn,
        xous::Message::new_scalar(crate::VaultOp::ImageLoad.to_usize().unwrap(), 1, 0, 0, 0),
    )
    .ok();
    std::thread::sleep(std::time::Duration::from_millis(1500));

    // --- 3. accept the staged post ---------------------------------------
    key(conn, '🔥', 2000);

    // --- 4. assertions ----------------------------------------------------
    let after = crate::baogram::storage::load_index(&pddb);
    let mut ok = case(
        "upload adds exactly one post",
        after.len() == before.len() + 1,
        &format!("{} -> {}", before.len(), after.len()),
    );
    if !ok {
        finish(false, "the upload did not produce a post");
    }

    let Some(new_id) = after.iter().find(|id| !before.contains(id)) else {
        finish(false, "no new post id in the index");
    };
    let Some(bytes) = crate::baogram::storage::load_post(&pddb, new_id) else {
        finish(false, "the new post could not be read back");
    };

    // parse() verifies the digest and the signature
    let post = match baogram_core::post::Post::parse(&bytes) {
        Ok(p) => p,
        Err(e) => finish(false, &format!("the new post failed to verify: {:?}", e)),
    };
    ok &= case("post is signed and verifies", true, "");

    let expected = crate::baogram::render::display_bitmap_to_small(&bits);
    match post.decode_image() {
        Ok(img) => {
            let want = expected.to_full();
            let differing = (0..baogram_core::image::IMAGE_HEIGHT)
                .flat_map(|y| (0..baogram_core::image::IMAGE_WIDTH).map(move |x| (x, y)))
                .filter(|&(x, y)| img.get(x, y) != want.get(x, y))
                .count();
            ok &= case(
                "stored pixels match the uploaded bitmap",
                differing == 0,
                &format!("{} differing pixels", differing),
            );
        }
        Err(e) => ok &= case("post image decodes", false, &format!("{:?}", e)),
    }

    // the post must not be blank - a bug that zeroed the buffer would still
    // round-trip through every check above
    let dark = expected.packed().iter().map(|b| b.count_zeros()).sum::<u32>();
    ok &= case(
        "image has both dark and light pixels",
        dark > 1000 && dark < (baogram_core::image::SMALL_PIXELS as u32 - 1000),
        &format!("{} dark of {}", dark, baogram_core::image::SMALL_PIXELS),
    );

    // --- 5. a second upload arriving while one is already pending must
    //        not stage twice or clobber the first ------------------------
    let base = after.len();
    for _ in 0..2 {
        xous::send_message(
            conn,
            xous::Message::new_scalar(crate::VaultOp::ImageLoad.to_usize().unwrap(), 1, 0, 0, 0),
        )
        .ok();
        std::thread::sleep(std::time::Duration::from_millis(800));
    }
    // one 🔥 accepts the single staged post; a second stage would need a
    // second 🔥, and the duplicate would be deduplicated by post id anyway,
    // so the count is what distinguishes "staged once" from "clobbered"
    key(conn, '🔥', 2000);
    let twice = crate::baogram::storage::load_index(&pddb);
    ok &= case(
        "a repeat upload does not double-stage",
        twice.len() == base + 1,
        &format!("{} -> {} (expected {})", base, twice.len(), base + 1),
    );

    // --- 6. `image clear` must not create a post -------------------------
    let before_clear = twice.len();
    xous::send_message(
        conn,
        xous::Message::new_scalar(crate::VaultOp::ImageLoad.to_usize().unwrap(), 0, 0, 0, 0),
    )
    .ok();
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let after_clear = crate::baogram::storage::load_index(&pddb);
    ok &= case(
        "image clear does not create a post",
        after_clear.len() == before_clear,
        &format!("{} -> {}", before_clear, after_clear.len()),
    );

    finish(ok, "see the [FAILED] lines above");
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
