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

/// End now from the tray, a hotkey or MIDI (threads with no runtime): the
/// destination ends with deleteStream, and nothing panics.
#[tokio::test]
async fn end_now_from_a_plain_thread_ends_the_destination_cleanly() {
    let mut sim = Sim::new(true).await;
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(1_000).await;
    sim.obs_crashes();
    wait(800).await;
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

/// Crash protection with a short hold, for the scenarios where it runs out.
fn one_second_hold(sim: &Sim) {
    sim.ctrl
        .update_crash_protection(crate::crash_protection::CrashProtection {
            enabled: true,
            hold_secs: 1,
            ..Default::default()
        });
}

/// OBS crashes with a delay longer than the hold time. The hold runs out
/// while the tail is still airing, and the tail airs in full before the
/// stream ends: viewers see everything OBS sent before the crash, then a
/// clean end. It used to end at the deadline, dropping the rest.
#[tokio::test]
async fn a_hold_that_runs_out_mid_tail_airs_the_tail_in_full_then_ends() {
    const DELAY_MS: u32 = 3_000;
    let mut sim = Sim::new(true).await;
    one_second_hold(&sim);
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(3_500).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(2_000).await;
    let last_live = sim.next_frame - 1;
    sim.obs_crashes();
    let crashed = Instant::now();
    wait(1_500).await;
    // What the supervisor does every tick.
    sim.ctrl.expire_hold();
    wait(3_000).await;

    let tail: Vec<Frame> = sim
        .frames(0)
        .into_iter()
        .filter(|f| f.at > crashed)
        .collect();
    assert_eq!(
        tail.last().map(|f| f.number),
        Some(last_live),
        "the whole tail aired"
    );
    assert_strictly_increasing(&tail, "the tail");
    assert!(
        sim.commands(0).contains(&"deleteStream".into()),
        "then it ended"
    );
    assert!(sim.pumps[0].is_finished(), "the pump ended");
    sim.assert_timestamps_monotonic(0);
}

/// OBS freezes with a delay on, and stays frozen past the hold. The
/// destination ends; it used to fall back into the normal delay logic,
/// which saw too little delay and cut back into the tail again and again.
#[tokio::test]
async fn a_freeze_hold_that_runs_out_never_replays_the_tail() {
    const DELAY_MS: u32 = 1_500;
    let mut sim = Sim::new(true).await;
    one_second_hold(&sim);
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(2_500).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(2_000).await;
    sim.obs_stalls(300).await;
    let froze = Instant::now();
    sim.freeze_detected();
    assert!(sim.ctrl.hold_active(), "the freeze is held");
    sim.obs_stalls(1_500).await;
    sim.ctrl.expire_hold();
    sim.obs_stalls(3_000).await;

    // From the last frame before the freeze on (activating the delay
    // replayed its own stretch once, as it should).
    let frames = sim.frames(0);
    let from = frames.iter().rposition(|f| f.at < froze).unwrap_or(0);
    assert_strictly_increasing(&frames[from..], "a frozen OBS past the hold");
    assert!(
        sim.commands(0).contains(&"deleteStream".into()),
        "the destination ended"
    );
    sim.assert_timestamps_monotonic(0);
}

/// End now while the delay tail is airing ends the stream at once: the
/// streamer asked for it, so the rest of the tail doesn't air first.
#[tokio::test]
async fn end_now_mid_tail_ends_the_stream_at_once() {
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
    let last_live = sim.next_frame - 1;
    sim.obs_crashes();
    wait(500).await;
    assert!(sim.ctrl.end_hold_now(), "a hold was on");
    wait(1_000).await;

    assert!(sim.commands(0).contains(&"deleteStream".into()), "it ended");
    assert!(
        sim.frames(0).last().is_some_and(|f| f.number < last_live),
        "without airing the rest of the tail"
    );
}

/// OBS comes back from a freeze on a P-frame, mid-GOP, with a delay on.
/// The rejoin lands at least the delay back: the rebuild counts from OBS's
/// first keyframe, not its first frame, and the rejoin takes the newest
/// keyframe at least that old, not the nearest one.
#[tokio::test]
async fn a_freeze_rejoin_lands_at_least_the_delay_back() {
    const DELAY_MS: u32 = 1_000;
    let mut sim = Sim::new(true).await;
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(2_000).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    // Frames 20..41: the freeze starts two frames into a GOP.
    sim.obs_sends(2_200).await;
    sim.obs_stalls(300).await;
    sim.freeze_detected();
    sim.obs_stalls(1_500).await;
    let back = Instant::now();
    sim.obs_sends(4_000).await;

    let rejoined = sim
        .frames(0)
        .into_iter()
        .find(|f| f.at > back && sim.produced[&f.number] > back)
        .expect("the stream came back");
    assert!(rejoined.keyframe, "rejoins at a keyframe");
    let delayed_by = rejoined
        .at
        .duration_since(sim.produced[&rejoined.number])
        .as_millis() as u32;
    assert!(
        delayed_by + 50 >= DELAY_MS,
        "rejoined {delayed_by} ms behind, short of the {DELAY_MS} ms delay"
    );
    sim.assert_timestamps_monotonic(0);
}

/// A pump that ends (OBS gone, protection off) stops holding its place in
/// the buffer. A stale place pinned the buffer's trim, so it grew to the
/// whole disk allowance while the destination waited to reconnect.
#[tokio::test]
async fn a_pump_that_ends_releases_its_place_in_the_buffer() {
    let mut sim = Sim::new(false).await;
    sim.obs_connects().await;
    sim.destination_runs("platform");
    sim.obs_sends(1_500).await;
    let dest = sim.ctrl.destination_state("platform");
    assert_ne!(
        dest.consumer_seq.load(Ordering::Relaxed),
        u64::MAX,
        "reading"
    );
    sim.obs_crashes();
    wait(1_000).await;
    assert_eq!(
        dest.consumer_seq.load(Ordering::Relaxed),
        u64::MAX,
        "the ended pump released its place"
    );
}
