//! Delay transition simulations (see `sim` for the harness). A real
//! destination pump streams to an in-process platform while the test plays
//! OBS in real time and turns a delay on, up, down or off. The assertions
//! are about what viewers would see: the screen for a moment over the jump,
//! then the stream where the delay puts them, on a keyframe.

use super::sim::{assert_strictly_increasing, Frame, Sim, FRAME_MS, TIMER_SLOP_MS};
use crate::delay_transition::DelayTransition;
use std::time::{Duration, Instant};

fn transition_on(sim: &Sim) {
    sim.ctrl.update_delay_transition(DelayTransition {
        enabled: true,
        ..DelayTransition::default()
    });
}

/// OBS live with `delay_ms` armed and the buffer full of it, on one
/// destination, crash protection off (the transition doesn't need it).
async fn live_with_delay_armed(delay_ms: u32) -> Sim {
    let mut sim = Sim::new(false).await;
    sim.ctrl.arm_delay(delay_ms);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(delay_ms + 1_000).await;
    sim
}

/// The last frame aired before `at` and the first after it.
fn either_side(frames: &[Frame], at: Instant) -> (Frame, Frame) {
    let before = frames.iter().rev().find(|f| f.at <= at).copied();
    let after = frames.iter().find(|f| f.at > at).copied();
    (
        before.expect("live before"),
        after.expect("the stream came back"),
    )
}

/// The frames either side of the first screen around `at`: the screen
/// comes at once, so the last frame before it can be from just before.
fn around_the_screen(frames: &[Frame], at: Instant) -> (Frame, Frame) {
    let from = at - Duration::from_millis(3 * u64::from(FRAME_MS));
    let after: Vec<Frame> = frames.iter().copied().filter(|f| f.at > from).collect();
    let gap = after
        .windows(2)
        .position(|pair| pair[1].at.duration_since(pair[0].at).as_millis() > 500)
        .expect("the screen left a gap in the stream");
    (after[gap], after[gap + 1])
}

/// The delay goes on: the screen right away, about a second of it, then
/// the stream the delay back, so the delay is on at once (viewers see its
/// last seconds again).
#[tokio::test]
async fn a_delay_turned_on_plays_the_screen_then_goes_back() {
    const DELAY_MS: u32 = 2_000;
    let mut sim = live_with_delay_armed(DELAY_MS).await;
    transition_on(&sim);
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    let activated = Instant::now();
    sim.obs_sends(3_000).await;

    let frames = sim.frames(0);
    let (last_live, first_back) = around_the_screen(&frames, activated);
    let paused_after = last_live
        .at
        .saturating_duration_since(activated)
        .as_millis();
    assert!(
        paused_after < 600,
        "the screen came {paused_after} ms after the delay turned on"
    );
    let screen = sim.screen_tags(0, last_live.at, first_back.at);
    assert!(
        (30..60).contains(&screen),
        "the screen played for a moment ({screen} frames)"
    );
    assert!(first_back.keyframe, "back on a keyframe");
    assert!(first_back.number < last_live.number, "back in time");
    let delayed_by = sim.delay_ms(&first_back);
    assert!(
        delayed_by + TIMER_SLOP_MS >= DELAY_MS,
        "back {delayed_by} ms behind, short of the {DELAY_MS} ms delay"
    );
    assert!(!sim.ended(0), "the session went on");
    assert_eq!(
        sim.ctrl.cuts_performed(),
        1,
        "the jump under the screen counts as a cut"
    );
    sim.assert_timestamps_monotonic(0);
}

/// Without the transition the same delay jumps viewers back: they see
/// frames they already saw. (What the transition is there to replace.)
#[tokio::test]
async fn without_the_transition_the_delay_jumps_back() {
    const DELAY_MS: u32 = 1_500;
    let mut sim = live_with_delay_armed(DELAY_MS).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(2_000).await;
    let numbers: Vec<u32> = sim.frames(0).iter().map(|f| f.number).collect();
    assert!(
        numbers.windows(2).any(|pair| pair[1] <= pair[0]),
        "the delay jumped back"
    );
}

/// Raising a delay that is already on plays the screen again, then the
/// stream at the raised delay.
#[tokio::test]
async fn raising_the_delay_plays_the_screen_then_the_raised_delay() {
    const FIRST_MS: u32 = 1_000;
    const RAISED_MS: u32 = 2_500;
    let mut sim = live_with_delay_armed(RAISED_MS).await;
    sim.ctrl.arm_delay(FIRST_MS);
    transition_on(&sim);
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(2_500).await;
    sim.ctrl.arm_delay(RAISED_MS);
    let raised = Instant::now();
    sim.obs_sends(3_000).await;

    let frames = sim.frames(0);
    let (_, first_back) = around_the_screen(&frames, raised);
    assert!(first_back.keyframe, "back on a keyframe");
    let delayed_by = sim.delay_ms(&first_back);
    assert!(
        delayed_by + TIMER_SLOP_MS >= RAISED_MS,
        "back {delayed_by} ms behind, short of the raised {RAISED_MS} ms"
    );
    sim.assert_timestamps_monotonic(0);
}

/// Cutting the delay while the screen is up: back on the stream live once
/// the screen has had its moment.
#[tokio::test]
async fn cutting_the_delay_mid_transition_goes_back_live() {
    const DELAY_MS: u32 = 3_000;
    let mut sim = live_with_delay_armed(DELAY_MS).await;
    transition_on(&sim);
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(300).await;
    sim.ctrl.stop_delay();
    let cut = Instant::now();
    sim.obs_sends(2_000).await;

    let frames = sim.frames(0);
    let (_, first_back) = either_side(&frames, cut);
    let back_after = first_back.at.duration_since(cut).as_millis();
    assert!(
        back_after < 1_500,
        "back on the stream {back_after} ms after the cut"
    );
    let delayed_by = sim.delay_ms(&first_back);
    assert!(
        delayed_by < 1_000,
        "back {delayed_by} ms behind live after the cut"
    );
    assert_strictly_increasing(&sim.frames_after(0, first_back.at), "live after the cut");
    sim.assert_timestamps_monotonic(0);
}

/// The delay goes off with the catch-up screen on: instead of jumping
/// forward, viewers see the screen for a moment, then the stream near live
/// on a keyframe. Nothing is replayed.
#[tokio::test]
async fn a_delay_turned_off_plays_the_catch_up_screen_then_goes_live() {
    const DELAY_MS: u32 = 2_000;
    let mut sim = live_with_delay_armed(DELAY_MS).await;
    sim.ctrl.update_delay_transition(DelayTransition {
        catch_up: true,
        ..DelayTransition::default()
    });
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(DELAY_MS + 1_000).await;
    sim.ctrl.stop_delay();
    let stopped = Instant::now();
    sim.obs_sends(3_000).await;

    let frames = sim.frames_after(0, stopped);
    assert_strictly_increasing(&frames, "after the delay went off");
    let (last_delayed, first_back) = around_the_screen(&frames, stopped);
    let screen = sim.screen_tags(0, last_delayed.at, first_back.at);
    assert!(
        (30..60).contains(&screen),
        "the screen played for a moment ({screen} frames)"
    );
    assert!(first_back.keyframe, "back on a keyframe");
    let delayed_by = sim.delay_ms(&first_back);
    assert!(delayed_by < 1_000, "back {delayed_by} ms behind live");
    assert!(!sim.ended(0), "the session went on");
    sim.assert_timestamps_monotonic(0);
}

/// Without the catch-up screen the delay going off cuts straight to live,
/// with no screen.
#[tokio::test]
async fn without_the_catch_up_screen_a_delay_turned_off_cuts_straight() {
    const DELAY_MS: u32 = 2_000;
    let mut sim = live_with_delay_armed(DELAY_MS).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(DELAY_MS + 1_000).await;
    sim.ctrl.stop_delay();
    let stopped = Instant::now();
    sim.obs_sends(2_000).await;

    let first_back = sim.frames_after(0, stopped)[0];
    let screen = sim.screen_tags(0, stopped, first_back.at);
    assert_eq!(screen, 0, "no screen");
    let live = *sim.frames_after(0, stopped).last().unwrap();
    assert!(sim.delay_ms(&live) < 1_000, "live after the cut");
}

/// Lowering the delay with the catch-up screen on: the screen, then the
/// stream at the lower delay, not live.
#[tokio::test]
async fn lowering_the_delay_plays_the_catch_up_screen_then_the_lower_delay() {
    const DELAY_MS: u32 = 3_000;
    const LOWERED_MS: u32 = 1_000;
    let mut sim = live_with_delay_armed(DELAY_MS).await;
    sim.ctrl.update_delay_transition(DelayTransition {
        catch_up: true,
        ..DelayTransition::default()
    });
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(DELAY_MS + 1_000).await;
    sim.ctrl.arm_delay(LOWERED_MS);
    let lowered = Instant::now();
    sim.obs_sends(3_000).await;

    let frames = sim.frames_after(0, lowered);
    assert_strictly_increasing(&frames, "after the delay went down");
    let (_, first_back) = around_the_screen(&frames, lowered);
    assert!(first_back.keyframe, "back on a keyframe");
    let delayed_by = sim.delay_ms(&first_back);
    assert!(
        delayed_by + TIMER_SLOP_MS >= LOWERED_MS && delayed_by < DELAY_MS,
        "back {delayed_by} ms behind, not at the lowered {LOWERED_MS} ms"
    );
    sim.assert_timestamps_monotonic(0);
}

/// A raise the buffer can't cover yet lands seconds later, once it has
/// filled: it is still the streamer's change, so the screen still plays.
#[tokio::test]
async fn a_raise_that_waits_for_the_buffer_still_plays_the_screen() {
    const FIRST_MS: u32 = 1_000;
    const RAISED_MS: u32 = 4_000;
    let mut sim = live_with_delay_armed(FIRST_MS).await;
    transition_on(&sim);
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(2_000).await;
    sim.ctrl.arm_delay(RAISED_MS);
    let raised = Instant::now();
    sim.obs_sends(4_000).await;

    let frames = sim.frames(0);
    let (last_before, first_back) = around_the_screen(&frames, raised);
    let waited = last_before.at.duration_since(raised).as_millis();
    assert!(
        waited > 300,
        "the raise waited for the buffer ({waited} ms)"
    );
    assert!(first_back.keyframe, "back on a keyframe");
    let delayed_by = sim.delay_ms(&first_back);
    assert!(
        delayed_by + TIMER_SLOP_MS >= RAISED_MS,
        "back {delayed_by} ms behind, short of the raised {RAISED_MS} ms"
    );
    sim.assert_timestamps_monotonic(0);
}

/// How many screens played in `frames`: each leaves a gap in the stream.
fn screens_in(frames: &[Frame]) -> usize {
    frames
        .windows(2)
        .filter(|pair| pair[1].at.duration_since(pair[0].at).as_millis() > 500)
        .count()
}

/// With both screens on, a raise plays the one screen for it: landing on
/// the raised delay must not read as a delay then lowered.
#[tokio::test]
async fn a_raise_with_both_screens_on_plays_one_screen() {
    for (first, raised, buffered) in [(1_000, 2_500, 2_500), (1_000, 4_000, 1_000)] {
        let mut sim = live_with_delay_armed(buffered).await;
        sim.ctrl.update_delay_transition(DelayTransition {
            enabled: true,
            catch_up: true,
            ..DelayTransition::default()
        });
        sim.ctrl.arm_delay(first);
        sim.ctrl
            .activate_delay()
            .expect("the buffer holds the delay");
        sim.obs_sends(2_500).await;
        sim.ctrl.arm_delay(raised);
        let at = Instant::now();
        sim.obs_sends(6_000).await;
        let frames = sim.frames_after(0, at - Duration::from_millis(100));
        assert_eq!(screens_in(&frames), 1, "{first} -> {raised} ms");
    }
}
