//! Streamer profiles played end to end (see `sim` for the harness): how
//! different people end, restart and cut a delayed stream, and what their
//! viewers see. The rules every profile keeps: nothing airs twice, nothing
//! from a new OBS session airs undelayed, timestamps never go back, and a
//! stream that ends sends its goodbye (`deleteStream`).

use super::sim::{assert_strictly_increasing, wait, Frame, Sim};
use std::collections::HashSet;
use std::time::{Duration, Instant};

const DELAY_MS: u32 = 2_000;

/// OBS live with the delay on air, then it sends `ms` more.
async fn delayed_stream(sim: &mut Sim, ms: u32) {
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.obs_sends(2_500).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(ms).await;
}

fn frames_after(sim: &mut Sim, conn: usize, at: Instant) -> Vec<Frame> {
    sim.frames(conn).into_iter().filter(|f| f.at > at).collect()
}

fn ended(sim: &mut Sim, conn: usize) -> bool {
    sim.commands(conn).contains(&"deleteStream".into())
}

/// How long after OBS made it a frame reached viewers.
fn delay_of(sim: &Sim, frame: &Frame) -> Duration {
    frame.at.duration_since(sim.produced[&frame.number])
}

/// A multistreamer (Twitch and YouTube), no crash protection, stops in
/// OBS: both platforms air the whole delay, then both end.
#[tokio::test]
async fn multistreamer_stop_airs_the_delay_on_every_platform() {
    let mut sim = Sim::new(false).await;
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.destination_connects("twitch").await;
    sim.destination_connects("youtube").await;
    sim.obs_sends(2_500).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(1_500).await;
    let last_live = sim.next_frame - 1;
    sim.obs_stops();
    let stopped = Instant::now();
    wait(3_000).await;

    for conn in 0..2 {
        let tail = frames_after(&mut sim, conn, stopped);
        assert_eq!(
            tail.last().map(|f| f.number),
            Some(last_live),
            "platform {conn} aired the whole delay"
        );
        assert_strictly_increasing(&tail, "the tail");
        assert!(ended(&mut sim, conn), "platform {conn} ended");
        sim.assert_timestamps_monotonic(conn);
    }
    assert_eq!(sim.ctrl.tail_left(), None);
}

/// A streamer armed a delay but stopped before it went on air: nothing is
/// held back, so the stream ends at once.
#[tokio::test]
async fn armed_but_not_on_yet_a_stop_ends_at_once() {
    let mut sim = Sim::new(false).await;
    sim.ctrl.arm_delay(5_000);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(1_500).await;
    sim.obs_stops();
    wait(800).await;

    assert!(ended(&mut sim, 0), "ended within the moment");
    assert_eq!(sim.ctrl.tail_left(), None, "nothing to count down");
}

/// Stops, then starts OBS again a moment later while the delay of the
/// first stream is still airing: the new stream reaches viewers delayed,
/// never live.
#[tokio::test]
async fn restarting_mid_tail_never_airs_the_new_stream_live() {
    restart_mid_tail(true).await;
}

/// Same, when OBS crashed instead (no crash protection) and came back.
#[tokio::test]
async fn coming_back_from_a_crash_mid_tail_never_airs_live() {
    restart_mid_tail(false).await;
}

async fn restart_mid_tail(stopped: bool) {
    let mut sim = Sim::new(false).await;
    sim.destination_runs("platform");
    delayed_stream(&mut sim, 1_000).await;
    if stopped {
        sim.obs_stops();
    } else {
        sim.obs_crashes();
    }
    wait(600).await;
    sim.obs_connects().await;
    let first_new = sim.next_frame;
    sim.obs_sends(4_500).await;

    assert!(ended(&mut sim, 0), "the first stream ended");
    let new: Vec<Frame> = sim
        .frames(1)
        .into_iter()
        .filter(|f| f.number >= first_new)
        .collect();
    let first = new.first().expect("the destination came back");
    assert!(first.keyframe, "starts on a keyframe");
    let delayed_by = delay_of(&sim, first);
    assert!(
        delayed_by >= Duration::from_millis(1_500),
        "the new stream airs delayed, not live ({delayed_by:?})"
    );
    sim.assert_timestamps_monotonic(1);
}

/// Stops, then panics and hits Cut to live while the delay still airs:
/// the cut wins and the stream ends without the rest.
#[tokio::test]
async fn cut_to_live_during_the_tail_ends_the_stream() {
    let mut sim = Sim::new(false).await;
    sim.destination_connects("platform").await;
    delayed_stream(&mut sim, 1_500).await;
    let last_live = sim.next_frame - 1;
    sim.obs_stops();
    wait(300).await;
    sim.ctrl.stop_delay();
    wait(1_200).await;

    assert!(ended(&mut sim, 0), "it ended");
    assert!(
        sim.frames(0).last().is_some_and(|f| f.number < last_live),
        "without airing the rest"
    );
    sim.assert_timestamps_monotonic(0);
}

/// Marks "cut after this airs", then stops before the mark has aired:
/// the mark airs, nothing airs twice, and the stream ends.
#[tokio::test]
async fn cut_after_this_airs_then_a_stop() {
    let mut sim = Sim::new(false).await;
    sim.destination_connects("platform").await;
    delayed_stream(&mut sim, 1_000).await;
    let mark = sim.next_frame - 1;
    assert_eq!(
        sim.ctrl.run_named_action("cut_after", DELAY_MS, "sim"),
        None,
        "the cut is scheduled"
    );
    sim.obs_sends(500).await;
    sim.obs_stops();
    let stopped = Instant::now();
    wait(3_000).await;

    let frames = sim.frames(0);
    assert!(
        frames.iter().any(|f| f.number == mark),
        "the marked moment aired"
    );
    assert_strictly_increasing(&frames_after(&mut sim, 0, stopped), "after the stop");
    assert!(ended(&mut sim, 0), "it ended");
    sim.assert_timestamps_monotonic(0);
}

/// Stops, hits End now, then goes live again: the new stream starts
/// delayed like any other.
#[tokio::test]
async fn end_now_then_a_new_stream_starts_delayed() {
    let mut sim = Sim::new(false).await;
    sim.destination_runs("platform");
    delayed_stream(&mut sim, 1_000).await;
    sim.obs_stops();
    wait(300).await;
    assert!(sim.ctrl.end_hold_now(), "the delay was airing");
    wait(500).await;
    assert!(ended(&mut sim, 0), "ended at once");
    sim.obs_connects().await;
    let first_new = sim.next_frame;
    sim.obs_sends(4_500).await;

    let first = sim
        .frames(1)
        .into_iter()
        .find(|f| f.number >= first_new)
        .expect("the new stream aired");
    let delayed_by = delay_of(&sim, &first);
    assert!(
        delayed_by >= Duration::from_millis(1_500),
        "delayed, not live ({delayed_by:?})"
    );
}

/// The dashboard's countdown while the delay airs goes down, then away.
#[tokio::test]
async fn the_countdown_goes_down_then_away() {
    let mut sim = Sim::new(false).await;
    sim.destination_connects("platform").await;
    delayed_stream(&mut sim, 1_000).await;
    sim.obs_stops();
    wait(300).await;
    let early = sim.ctrl.tail_left().expect("counting down");
    wait(800).await;
    let later = sim.ctrl.tail_left().expect("still counting down");
    assert!(later < early, "{later:?} after {early:?}");
    assert!(
        early <= Duration::from_millis(u64::from(DELAY_MS) + 600),
        "never more than the delay ({early:?})"
    );
    wait(2_000).await;
    assert_eq!(sim.ctrl.tail_left(), None, "gone once it aired");
}

/// A streamer who stops and starts three times in a row (scene setup,
/// a crash of their own, a restart): every stream airs delayed and whole,
/// and nothing airs twice across all of them.
#[tokio::test]
async fn three_stops_and_starts_in_a_row() {
    const SHORT_DELAY_MS: u32 = 1_000;
    let mut sim = Sim::new(false).await;
    sim.destination_runs("platform");
    sim.ctrl.arm_delay(SHORT_DELAY_MS);
    sim.obs_connects().await;
    sim.obs_sends(1_500).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    // Turning the delay on replays its own stretch once, by design: count
    // replays from then on.
    let activated = Instant::now();
    let mut last_lives = Vec::new();
    for round in 0..3 {
        sim.obs_sends(1_500).await;
        last_lives.push(sim.next_frame - 1);
        sim.obs_stops();
        wait(1_800).await;
        if round < 2 {
            sim.obs_connects().await;
        }
    }
    wait(500).await;

    let mut seen = HashSet::new();
    for (conn, last_live) in last_lives.into_iter().enumerate() {
        let frames = sim.frames(conn);
        assert!(!frames.is_empty(), "stream {conn} aired");
        assert_eq!(
            frames.last().map(|f| f.number),
            Some(last_live),
            "stream {conn} aired its whole delay"
        );
        for f in frames.iter().filter(|f| f.at > activated) {
            assert!(seen.insert(f.number), "frame {} aired twice", f.number);
        }
        sim.assert_timestamps_monotonic(conn);
    }
}

/// Asked for a longer delay than the buffer holds, then stops: what the
/// buffer holds airs, nothing twice, then the end.
#[tokio::test]
async fn a_delay_longer_than_the_buffer_then_a_stop() {
    // About 3 s of stream fits (see `Sim::with_ring_bytes`).
    let mut sim = Sim::with_ring_bytes(false, 720).await;
    sim.ctrl.arm_delay(6_000);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(4_000).await;
    let _ = sim.ctrl.activate_delay();
    sim.obs_sends(2_000).await;
    sim.obs_stops();
    let stopped = Instant::now();
    wait(5_000).await;

    assert_strictly_increasing(&frames_after(&mut sim, 0, stopped), "after the stop");
    assert!(ended(&mut sim, 0), "it ended");
    sim.assert_timestamps_monotonic(0);
}
