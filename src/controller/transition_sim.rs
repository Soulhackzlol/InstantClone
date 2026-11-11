//! Delay transition simulations (see `sim` for the harness). A real
//! destination pump streams to an in-process platform while the test plays
//! OBS in real time and turns a delay on. The assertions are about what
//! viewers would see: no jump back (nothing replayed), the screen while the
//! delay builds, then the stream again from where they were, the delay
//! behind, with at most one keyframe interval passed over.

use super::sim::{assert_strictly_increasing, Frame, Sim, GOP_FRAMES, TIMER_SLOP_MS};
use crate::delay_transition::DelayTransition;
use std::time::Instant;

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

/// The delay goes on: viewers see the screen instead of the last seconds
/// again, then the stream from where they were, the delay behind.
#[tokio::test]
async fn a_delay_turned_on_plays_the_screen_and_goes_on_from_where_viewers_were() {
    const DELAY_MS: u32 = 1_500;
    let mut sim = live_with_delay_armed(DELAY_MS).await;
    transition_on(&sim);
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    let activated = Instant::now();
    sim.obs_sends(4_000).await;

    let frames = sim.frames(0);
    assert_strictly_increasing(&frames, "through the transition");
    let (last_live, first_back) = either_side(&frames, activated);
    let screen = sim.screen_tags(0, last_live.at, first_back.at);
    assert!(
        screen >= 30,
        "the screen covered the wait ({screen} frames)"
    );
    assert!(first_back.keyframe, "back on a keyframe");
    let delayed_by = sim.delay_ms(&first_back);
    assert!(
        delayed_by + TIMER_SLOP_MS >= DELAY_MS,
        "back {delayed_by} ms behind, short of the {DELAY_MS} ms delay"
    );
    let passed_over = first_back.number - last_live.number - 1;
    assert!(
        passed_over < GOP_FRAMES,
        "{passed_over} frames passed over, more than a keyframe interval"
    );
    assert!(!sim.ended(0), "the session went on");
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

/// Raising a delay that is already on plays the screen for the difference:
/// the stream comes back at the new delay, still with nothing replayed.
#[tokio::test]
async fn raising_the_delay_plays_the_screen_for_the_difference() {
    const FIRST_MS: u32 = 1_000;
    const RAISED_MS: u32 = 2_500;
    let mut sim = live_with_delay_armed(FIRST_MS).await;
    transition_on(&sim);
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(2_500).await;
    sim.ctrl.arm_delay(RAISED_MS);
    let raised = Instant::now();
    sim.obs_sends(4_000).await;

    let frames = sim.frames(0);
    assert_strictly_increasing(&frames, "through both transitions");
    // The stream runs on at the old delay until the pump's next cut check,
    // then the screen leaves a gap: the frame after the gap is the return.
    let after_raise: Vec<Frame> = frames.iter().copied().filter(|f| f.at > raised).collect();
    let gap = after_raise
        .windows(2)
        .position(|pair| pair[1].at.duration_since(pair[0].at).as_millis() > 500)
        .expect("the screen left a gap in the stream");
    let first_back = after_raise[gap + 1];
    let delayed_by = sim.delay_ms(&first_back);
    assert!(
        delayed_by + TIMER_SLOP_MS >= RAISED_MS,
        "back {delayed_by} ms behind, short of the raised {RAISED_MS} ms"
    );
    let waited = first_back.at.duration_since(raised).as_millis() as u32;
    assert!(
        waited + TIMER_SLOP_MS >= RAISED_MS - FIRST_MS,
        "the screen stayed up for the difference, not {waited} ms"
    );
    sim.assert_timestamps_monotonic(0);
}

/// Cutting the delay while the screen is up ends it at once: back on the
/// stream, live.
#[tokio::test]
async fn cutting_the_delay_mid_transition_goes_back_live() {
    const DELAY_MS: u32 = 3_000;
    let mut sim = live_with_delay_armed(DELAY_MS).await;
    transition_on(&sim);
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(1_000).await;
    sim.ctrl.stop_delay();
    let cut = Instant::now();
    sim.obs_sends(2_000).await;

    let frames = sim.frames(0);
    assert_strictly_increasing(&frames, "through the cut");
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
    sim.assert_timestamps_monotonic(0);
}
