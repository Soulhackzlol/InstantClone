//! Crash-protection simulations (see `sim` for the harness). A real
//! destination pump streams to an in-process platform while the test plays
//! OBS in real time: live, crash, freeze, stop, back again. The assertions
//! are about what viewers would see: the tail airs once, the screen covers
//! the gap, nothing from before the hold is replayed, and timestamps never
//! go back.

use super::sim::{assert_strictly_increasing, wait, Frame, Sim};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// OBS crashes with no delay: the screen covers the gap on the same
/// connection, and OBS coming back rejoins at its first keyframe with
/// nothing replayed.
#[tokio::test]
async fn crash_screen_then_obs_back_on_the_same_connection() {
    let mut sim = Sim::new(true).await;
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(1_500).await;
    sim.obs_crashes();
    let crashed = Instant::now();
    wait(1_500).await;
    sim.obs_connects().await;
    let back = Instant::now();
    sim.obs_sends(1_500).await;

    let frames = sim.frames(0);
    assert!(
        frames.iter().any(|f| f.at < crashed),
        "live before the crash"
    );
    assert!(
        sim.screen_tags(0, crashed, back) >= 5,
        "the screen covered the gap"
    );
    let after: Vec<Frame> = frames.iter().copied().filter(|f| f.at > back).collect();
    assert!(
        after.first().is_some_and(|f| f.keyframe),
        "rejoins at a keyframe"
    );
    assert_strictly_increasing(&frames, "crash and back");
    assert!(
        !sim.commands(0).contains(&"deleteStream".into()),
        "never ended"
    );
    assert!(sim.received().iter().all(|m| m.conn == 0), "one connection");
    sim.assert_timestamps_monotonic(0);
}

/// OBS crashes with a 1.5 s delay on air: the tail airs once (no loop),
/// then the screen; when OBS is back the screen stays up until the delay
/// has rebuilt, so the stream comes back still delayed.
#[tokio::test]
async fn a_crash_mid_delay_airs_the_tail_once_and_the_delay_survives_the_return() {
    const DELAY_MS: u32 = 1_500;
    let mut sim = Sim::new(true).await;
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(2_500).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(2_000).await;
    let last_live = sim.next_frame - 1;
    sim.obs_crashes();
    let crashed = Instant::now();
    wait(3_000).await;

    let tail: Vec<Frame> = sim
        .frames(0)
        .into_iter()
        .filter(|f| f.at > crashed)
        .collect();
    assert_strictly_increasing(&tail, "the tail");
    assert_eq!(
        tail.last().map(|f| f.number),
        Some(last_live),
        "the whole tail aired"
    );
    let tail_ms = tail.last().unwrap().at.duration_since(crashed).as_millis();
    assert!(
        tail_ms >= 1_000,
        "the tail took the delay to air, not {tail_ms} ms"
    );
    let screen_from = tail.last().unwrap().at;
    assert!(
        sim.screen_tags(0, screen_from, Instant::now()) >= 5,
        "then the screen"
    );

    sim.obs_connects().await;
    let back = Instant::now();
    sim.obs_sends(3_000).await;
    let after: Vec<Frame> = sim.frames(0).into_iter().filter(|f| f.at > back).collect();
    let rejoined = after.first().expect("the stream came back");
    assert!(rejoined.keyframe, "rejoins at a keyframe");
    let delayed_by = rejoined.at.duration_since(sim.produced[&rejoined.number]);
    assert!(
        delayed_by >= Duration::from_millis(1_000),
        "still delayed after OBS came back, not live ({delayed_by:?})"
    );
    assert_strictly_increasing(&after, "after the return");
    sim.assert_timestamps_monotonic(0);
}

/// OBS comes back from a crash while the delay tail is still airing. The
/// rest of the tail went with the old session; the stream must not jump to
/// live: the screen covers the gap until the delay has rebuilt.
#[tokio::test]
async fn obs_back_mid_tail_never_puts_viewers_on_live() {
    const DELAY_MS: u32 = 3_000;
    let mut sim = Sim::new(true).await;
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(3_500).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(2_000).await;
    sim.obs_crashes();
    wait(1_000).await;
    sim.obs_connects().await;
    let back = Instant::now();
    sim.obs_sends(5_000).await;

    let first_new = sim
        .frames(0)
        .into_iter()
        .find(|f| f.at > back && sim.produced[&f.number] > back)
        .expect("the stream came back");
    assert!(first_new.keyframe, "rejoins at a keyframe");
    let delayed_by = first_new.at.duration_since(sim.produced[&first_new.number]);
    assert!(
        delayed_by >= Duration::from_millis(2_000),
        "OBS's new video must air delayed, not live ({delayed_by:?})"
    );
    sim.assert_timestamps_monotonic(0);
}

/// End now from the tray, a hotkey or MIDI (threads with no runtime) with
/// a webhook set: the destination ends with deleteStream, and nothing
/// panics.
#[tokio::test]
async fn end_now_from_a_plain_thread_ends_the_destination_cleanly() {
    let mut sim = Sim::new(true).await;
    sim.ctrl.update_webhook("http://127.0.0.1:9/webhook".into());
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(1_000).await;
    sim.obs_crashes();
    wait(800).await;
    // Clear the throttle the hold's own post set, so End now posts too.
    sim.ctrl.webhook_last_fire_ms.store(0, Ordering::Relaxed);
    let ctrl = sim.ctrl.clone();
    let hotkey = std::thread::spawn(move || ctrl.run_named_action("end_hold", 0, "hotkey"));
    assert_eq!(hotkey.join().expect("no panic off the runtime"), None);
    wait(800).await;
    assert!(sim.commands(0).contains(&"deleteStream".into()));
    assert!(sim.pumps[0].is_finished(), "the pump ended");
}

/// OBS freezes (connected, no video): the screen covers it, a destination
/// that (re)connects mid-freeze goes straight to the screen instead of
/// replaying old video, and both rejoin at OBS's first keyframe after it.
#[tokio::test]
async fn a_freeze_is_covered_and_a_reconnecting_destination_goes_straight_to_the_screen() {
    let mut sim = Sim::new(true).await;
    sim.obs_connects().await;
    sim.destination_connects("first").await;
    sim.obs_sends(1_000).await;
    let last_before = sim.next_frame - 1;
    sim.obs_stalls(300).await;
    sim.freeze_detected();
    assert!(sim.ctrl.hold_active());
    sim.obs_stalls(800).await;
    sim.destination_connects("second").await;
    sim.obs_stalls(800).await;
    let back = Instant::now();
    sim.obs_sends(1_500).await;

    let second = sim.frames(1);
    assert!(second.iter().all(|f| f.at > back), "no old video replayed");
    assert!(sim.screen_tags(1, back - Duration::from_millis(700), back) >= 3);
    for conn in [0, 1] {
        let after: Vec<Frame> = sim
            .frames(conn)
            .into_iter()
            .filter(|f| f.at > back)
            .collect();
        let first = after.first().expect("the stream came back");
        assert!(first.keyframe, "conn {conn} rejoins at a keyframe");
        assert!(
            first.number > last_before,
            "conn {conn} rejoins on new video"
        );
        sim.assert_timestamps_monotonic(conn);
    }
    assert_strictly_increasing(&sim.frames(0), "the first destination");
}

/// Stopping in OBS while the screen covers a freeze ends the stream.
#[tokio::test]
async fn stopping_during_a_freeze_ends_the_destination() {
    let mut sim = Sim::new(true).await;
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(1_000).await;
    sim.obs_stalls(300).await;
    sim.freeze_detected();
    sim.obs_stalls(500).await;
    sim.obs_stops();
    wait(800).await;
    assert!(sim.commands(0).contains(&"deleteStream".into()));
}

/// Crash protection off: OBS dropping ends the destination at once, as
/// before crash protection existed.
#[tokio::test]
async fn protection_off_ends_the_destination_when_obs_drops() {
    let mut sim = Sim::new(false).await;
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(1_000).await;
    sim.obs_crashes();
    let crashed = Instant::now();
    wait(800).await;
    assert!(sim.commands(0).contains(&"deleteStream".into()));
    assert_eq!(sim.screen_tags(0, crashed, Instant::now()), 0, "no screen");
}

/// OBS comes back with a delay armed, so the screen stays up while the
/// delay rebuilds from its new video, then OBS stops on purpose before it
/// has. There is no hold left to end, so the screen has to notice OBS is
/// gone and end the stream at once, not wait out the rebuild grace.
#[tokio::test]
async fn obs_stopping_while_the_delay_rebuilds_ends_the_destination() {
    const DELAY_MS: u32 = 1_000;
    let mut sim = Sim::new(true).await;
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(1_000).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_crashes();
    wait(1_500).await;
    sim.obs_connects().await;
    let back = Instant::now();
    sim.obs_sends(300).await;
    assert_eq!(sim.ctrl.hold_state(), crate::crash_hold::HoldState::Resumed);
    sim.obs_stops();
    wait(600).await;

    let back_on_air = sim.frames(0).into_iter().filter(|f| f.at > back).count();
    assert_eq!(back_on_air, 0, "OBS's short new video never aired");
    assert!(
        sim.commands(0).contains(&"deleteStream".into()),
        "the stream ended when OBS stopped"
    );
    assert!(sim.pumps[0].is_finished(), "the pump ended");
    sim.assert_timestamps_monotonic(0);
}
