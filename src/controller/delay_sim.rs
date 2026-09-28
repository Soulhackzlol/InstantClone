//! Delay simulations (see `sim` for the harness): the normal lifecycle of a
//! delay, played in real time through a real pump. OBS streams at 10 fps
//! with a keyframe every 500 ms; the streamer arms a delay, goes delayed,
//! cuts back to live, schedules "cut after this airs", and changes the
//! delay on air. The assertions are about what viewers would see: where the
//! picture jumps, whether it lands on a keyframe, how far behind OBS it
//! runs, that nothing airs more often than the delay itself explains, and
//! that timestamps never go back.

use super::sim::{assert_strictly_increasing, wait, Frame, Sim, FRAME_MS, GOP_FRAMES};
use super::*;
use std::collections::HashSet;

/// The delay most scenarios arm.
const DELAY_MS: u32 = 2_000;
const GOP_MS: u32 = FRAME_MS * GOP_FRAMES;
/// Room for scheduling: the pump and OBS each sleep to a schedule, and the
/// test binary runs many of these at once.
const JITTER_MS: u32 = 150;
/// Each frame's audio is stamped this far behind its video, so a cut lands
/// on a keyframe whose own audio is older than it and has to be dropped.
const AUDIO_LAG_MS: u32 = 30;

async fn delay_sim() -> Sim {
    let mut sim = Sim::new(false).await;
    sim.audio_lag_ms = AUDIO_LAG_MS;
    sim
}

/// How long after OBS made it `frame` reached the platform.
fn delay_ms(sim: &Sim, frame: &Frame) -> u32 {
    frame
        .at
        .duration_since(sim.produced[&frame.number])
        .as_millis() as u32
}

/// Every frame in `frames` ran between `min_ms` and `max_ms` behind OBS.
fn assert_delays(sim: &Sim, frames: &[Frame], min_ms: u32, max_ms: u32, what: &str) {
    let delays: Vec<(u32, u32)> = frames
        .iter()
        .map(|f| (f.number, delay_ms(sim, f)))
        .collect();
    let off = delays
        .iter()
        .find(|(_, ms)| !(min_ms..=max_ms).contains(ms));
    assert_eq!(
        off, None,
        "{what}: a frame ran outside {min_ms}..={max_ms} ms behind OBS: {delays:?}"
    );
}

/// Sleeps can wake a few ms early on a loaded test runner.
const TIMER_SLOP_MS: u32 = 50;

/// A delay is honored: every frame in `frames` ran at least `delay_ms`
/// behind OBS (never closer to live than asked) and at most one keyframe
/// interval more, which is how far back the newest keyframe that old sits.
fn assert_delay_honored(sim: &Sim, frames: &[Frame], delay_ms: u32, gop_ms: u32, what: &str) {
    let most = delay_ms + gop_ms + JITTER_MS;
    assert_delays(sim, frames, delay_ms - TIMER_SLOP_MS, most, what);
}

/// Where the picture jumps back to an earlier frame.
fn backward_jumps(frames: &[Frame]) -> Vec<usize> {
    (1..frames.len())
        .filter(|&i| frames[i].number < frames[i - 1].number)
        .collect()
}

/// Where the picture skips ahead past frames it never showed.
fn forward_jumps(frames: &[Frame]) -> Vec<usize> {
    (1..frames.len())
        .filter(|&i| frames[i].number > frames[i - 1].number + 1)
        .collect()
}

/// A jump back replays what the delay holds once; nothing airs a third time.
fn assert_airs_at_most_twice(frames: &[Frame]) {
    let mut seen: HashMap<u32, u32> = HashMap::new();
    for frame in frames {
        *seen.entry(frame.number).or_default() += 1;
    }
    let mut thrice: Vec<u32> = seen
        .into_iter()
        .filter(|(_, n)| *n > 2)
        .map(|(number, _)| number)
        .collect();
    thrice.sort_unstable();
    assert!(thrice.is_empty(), "frames aired three times: {thrice:?}");
}

/// The pump re-sent the video and audio sequence headers between
/// `frames[i - 1]` and `frames[i]`, on `frames[i]`'s own timestamp, so the
/// decoder has its config before the frame it jumped to.
fn assert_headers_lead(sim: &mut Sim, conn: usize, frames: &[Frame], i: usize) {
    let (from, to, ts) = (frames[i - 1].index, frames[i].index, frames[i].ts);
    let between = &sim.received()[from + 1..to];
    let is_header = |kind: u8| {
        between
            .iter()
            .any(|m| m.conn == conn && m.kind == kind && m.payload.get(1) == Some(&0) && m.ts == ts)
    };
    assert!(
        is_header(9),
        "no video sequence header led frame {}",
        frames[i].number
    );
    assert!(
        is_header(8),
        "no audio sequence header led frame {}",
        frames[i].number
    );
}

/// Audio kept up with the video: every frame that aired had its audio air
/// too, except the one audio frame each join or cut drops because it is
/// stamped just before the keyframe it lands on. `joins` counts the pump's
/// seed plus its cuts.
fn assert_audio_kept_up(sim: &mut Sim, conn: usize, joins: usize) {
    let video = sim.frames(conn);
    let Some(last) = video.last().map(|f| f.number) else {
        return;
    };
    let mut owed: HashMap<u32, i64> = HashMap::new();
    for frame in &video {
        *owed.entry(frame.number).or_default() += 1;
    }
    for number in sim.audio_numbers(conn) {
        *owed.entry(number).or_default() -= 1;
    }
    // The newest frame's audio may still be on its way.
    owed.remove(&last);
    let mut missing: Vec<u32> = owed
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(number, _)| *number)
        .collect();
    missing.sort_unstable();
    assert!(
        missing.len() <= joins,
        "audio went missing for frames {missing:?} ({joins} joins and cuts)"
    );
}

fn cuts(sim: &Sim, id: &str) -> u32 {
    sim.ctrl
        .destination_state(id)
        .cuts_performed
        .load(Ordering::Relaxed)
}

/// Live (no delay): a destination joining mid-stream starts on OBS's newest
/// keyframe, then every frame goes out once, in order, as soon as it can.
#[tokio::test]
async fn live_frames_go_out_once_in_order_as_obs_makes_them() {
    let mut sim = delay_sim().await;
    sim.obs_connects().await;
    sim.obs_sends(700).await;
    let newest_keyframe = (sim.next_frame - 1) / GOP_FRAMES * GOP_FRAMES;
    sim.destination_connects("platform").await;
    sim.obs_sends(2_000).await;

    let frames = sim.frames(0);
    assert!(frames.len() >= 18, "{} frames aired", frames.len());
    assert_eq!(
        frames[0].number, newest_keyframe,
        "joins on the newest keyframe"
    );
    assert!(frames[0].keyframe);
    assert_strictly_increasing(&frames, "live");
    assert_delays(&sim, &frames, 0, GOP_MS + JITTER_MS, "live");
    assert_eq!(cuts(&sim, "platform"), 0);
    sim.assert_timestamps_monotonic(0);
    assert_audio_kept_up(&mut sim, 0, 1);
}

/// Going delayed: one jump back to the keyframe nearest the delay, led by
/// the sequence headers on its own timestamp, then the stream runs that far
/// behind OBS without ever jumping again.
#[tokio::test]
async fn activating_jumps_back_once_to_a_keyframe_the_delay_ago() {
    let mut sim = delay_sim().await;
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(2_100).await;
    let activated = Instant::now();
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(3_400).await;

    let frames = sim.frames(0);
    let back = backward_jumps(&frames);
    assert_eq!(back.len(), 1, "exactly one jump back");
    let i = back[0];
    let landed = frames[i];
    assert!(landed.keyframe, "lands on a keyframe");
    let late = landed.at.duration_since(activated).as_millis() as u32;
    assert!(
        late <= FRAME_MS + JITTER_MS,
        "jumped {late} ms after activating"
    );
    assert_headers_lead(&mut sim, 0, &frames, i);
    let delayed = &frames[i..];
    assert_strictly_increasing(delayed, "delayed");
    let span = delayed.last().unwrap().at.duration_since(landed.at);
    assert!(
        span >= Duration::from_millis(2_500),
        "only {span:?} delayed"
    );
    assert_delay_honored(&sim, delayed, DELAY_MS, GOP_MS, "delayed");
    assert_eq!(cuts(&sim, "platform"), 1, "no re-cut once on the delay");
    assert_airs_at_most_twice(&frames);
    sim.assert_timestamps_monotonic(0);
    assert_audio_kept_up(&mut sim, 0, 2);
}

/// Cutting back to live: one jump forward to the newest keyframe, with
/// nothing that already aired shown again, and the delay gone.
#[tokio::test]
async fn cutting_to_live_jumps_forward_to_the_newest_keyframe() {
    let mut sim = delay_sim().await;
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(2_100).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(2_000).await;
    let cut_at = Instant::now();
    sim.ctrl.stop_delay();
    sim.obs_sends(1_500).await;

    let frames = sim.frames(0);
    let ahead = forward_jumps(&frames);
    assert_eq!(ahead.len(), 1, "exactly one jump forward");
    let i = ahead[0];
    let landed = frames[i];
    assert!(landed.at > cut_at, "the jump is the cut");
    let late = landed.at.duration_since(cut_at).as_millis() as u32;
    assert!(late <= FRAME_MS + JITTER_MS, "cut {late} ms after asking");
    assert!(landed.keyframe, "lands on a keyframe");
    let aired_before = frames[..i].iter().map(|f| f.number).max().unwrap();
    assert!(landed.number > aired_before, "nothing is replayed");
    assert_headers_lead(&mut sim, 0, &frames, i);
    assert_strictly_increasing(&frames[i..], "live again");
    assert_delays(&sim, &frames[i..], 0, GOP_MS + JITTER_MS, "live again");
    assert_eq!(cuts(&sim, "platform"), 2, "activate, then cut");
    sim.assert_timestamps_monotonic(0);
    assert_audio_kept_up(&mut sim, 0, 3);
}

/// "Cut after this airs": everything up to the moment the streamer marked
/// airs first, delayed, and only then does the stream jump to live.
#[tokio::test]
async fn cut_after_this_airs_waits_for_the_marked_frame_then_goes_live() {
    let mut sim = delay_sim().await;
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(2_100).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(600).await;
    let marked = sim.next_frame - 1;
    sim.ctrl.schedule_safe_cut().expect("a delay is on air");
    sim.obs_sends(3_000).await;

    let frames = sim.frames(0);
    let back = backward_jumps(&frames);
    assert_eq!(back.len(), 1, "the activation is the only jump back");
    let ahead = forward_jumps(&frames);
    assert_eq!(ahead.len(), 1, "exactly one jump forward");
    let (from, to) = (back[0], ahead[0]);
    let delayed = &frames[from..to];
    assert!(
        delayed.windows(2).all(|p| p[1].number == p[0].number + 1),
        "every delayed frame aired, in order"
    );
    let aired = delayed
        .iter()
        .find(|f| f.number == marked)
        .expect("the marked frame aired before the cut");
    let waited = frames[to].at.duration_since(aired.at).as_millis() as u32;
    // The pump looks at the mark every 500 ms, on its next frame.
    assert!(
        waited <= 500 + FRAME_MS + JITTER_MS,
        "cut {waited} ms after the mark aired"
    );
    assert!(frames[to].keyframe, "lands on a keyframe");
    assert_delays(&sim, &frames[to..], 0, GOP_MS + JITTER_MS, "live again");
    assert!(!sim.ctrl.safe_cut_pending());
    assert_eq!(sim.ctrl.target_delay_ms(), 0);
    assert_eq!(sim.ctrl.armed_delay_ms(), DELAY_MS, "armed for next time");
    assert_eq!(cuts(&sim, "platform"), 2);
    sim.assert_timestamps_monotonic(0);
    assert_audio_kept_up(&mut sim, 0, 3);
}

/// Raising the delay on air past what the buffer holds: the pump holds its
/// place while the buffer grows, then jumps back once and plays on. It
/// used to jump as soon as the buffer spanned the new delay less the re-cut
/// dead band, landing short enough that the next check jumped back to the
/// same keyframe and viewers saw the same stretch twice in a row. Now it
/// waits for a keyframe at least the new delay old.
///
/// Raised here to the most the buffer can reach at a moment its oldest
/// keyframe sits 400 ms in, which is where the old build-up cut short.
#[tokio::test]
async fn raising_the_delay_on_air_jumps_back_exactly_once() {
    let mut sim = delay_sim().await;
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(2_500).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(1_700).await;
    let ring = &sim.ctrl.ring;
    let oldest = ring.oldest_ts().unwrap();
    let oldest_keyframe = ring.oldest_idr_at_or_after(ring.front_seq().unwrap());
    let gap = oldest_keyframe.unwrap().ts_ms - oldest;
    assert!(gap >= 300, "the oldest keyframe sits only {gap} ms in");
    let dead_band = recut_dead_band_ms(sim.ctrl.keyframe_interval_ms()) as u32;
    let raised = (ring.latest_ts().unwrap() - oldest) as u32 + dead_band;
    let raised_at = Instant::now();
    sim.ctrl.arm_delay(raised);
    sim.obs_sends(3_000).await;

    assert_eq!(cuts(&sim, "platform"), 2, "activate, then one jump back");
    let after: Vec<Frame> = sim
        .frames(0)
        .into_iter()
        .filter(|f| f.at > raised_at)
        .collect();
    let mut seen = HashSet::new();
    let again = after.iter().find(|f| !seen.insert(f.number));
    assert!(
        again.is_none(),
        "{again:?} aired twice after raising the delay"
    );
    let reached = delay_ms(&sim, after.last().unwrap());
    assert!(
        reached + TIMER_SLOP_MS >= raised,
        "ran {reached} ms behind, asked for {raised}"
    );
    sim.assert_timestamps_monotonic(0);
}

/// Lowering the delay on air: one jump forward to the new delay, with
/// nothing that already aired shown again.
#[tokio::test]
async fn lowering_the_delay_on_air_jumps_forward_once() {
    const LOWERED_MS: u32 = 1_000;
    let mut sim = delay_sim().await;
    sim.ctrl.arm_delay(3_000);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(3_100).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(900).await;
    let lowered_at = Instant::now();
    sim.ctrl.arm_delay(LOWERED_MS);
    sim.obs_sends(1_500).await;

    let frames = sim.frames(0);
    let back = backward_jumps(&frames);
    assert_eq!(back.len(), 1, "the activation is the only jump back");
    let ahead = forward_jumps(&frames);
    assert_eq!(ahead.len(), 1, "exactly one jump forward");
    let i = ahead[0];
    assert!(frames[i].at > lowered_at, "the jump is the change");
    assert!(frames[i].keyframe, "lands on a keyframe");
    // What aired live before going delayed airs again by nature; nothing
    // from the delayed stretch may.
    let aired_delayed = frames[back[0]..i].iter().map(|f| f.number).max().unwrap();
    assert!(frames[i].number > aired_delayed, "nothing is replayed");
    assert_airs_at_most_twice(&frames);
    assert_strictly_increasing(&frames[i..], "after lowering");
    assert_delay_honored(&sim, &frames[i..], LOWERED_MS, GOP_MS, "after lowering");
    assert_eq!(cuts(&sim, "platform"), 2, "activate, then lower");
    sim.assert_timestamps_monotonic(0);
    assert_audio_kept_up(&mut sim, 0, 3);
}

/// A keyframe every 6 s, in frames.
const GOP_6S: u32 = 60;

/// A destination is switched on 2.6 s into a 2 s delay, with a keyframe
/// every 6 s: the only keyframe in the stream so far is OBS's first one.
async fn destination_added_mid_delay_on_a_long_gop(sim: &mut Sim) {
    sim.gop_frames = GOP_6S;
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.destination_connects("first").await;
    sim.obs_sends(1_700).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(2_600).await;
    sim.destination_connects("added").await;
    sim.obs_sends(2_400).await;
}

/// The added destination joins on a keyframe and plays on from there,
/// without the jump back a live start followed by a cut to the delay would
/// show, at least the delay behind and within a keyframe interval of it.
#[tokio::test]
async fn a_destination_added_mid_delay_joins_without_a_jump_on_a_long_gop() {
    let mut sim = delay_sim().await;
    destination_added_mid_delay_on_a_long_gop(&mut sim).await;

    let frames = sim.frames(1);
    let first = frames.first().expect("the added destination is on air");
    assert!(first.keyframe, "joins on a keyframe");
    assert_delay_honored(&sim, &frames, DELAY_MS, GOP_6S * FRAME_MS, "added");
    assert_strictly_increasing(&frames, "the added destination");
    assert_eq!(cuts(&sim, "added"), 0, "no jump after joining");
    sim.assert_timestamps_monotonic(1);
}

/// The keyframe nearest the delay when the destination is added is OBS's
/// first one, 4.3 s back, but the buffer keeps only the armed delay plus
/// 2 s, so it was trimmed as soon as the first destination played past it.
/// With no keyframe left to join on, the seed waits for OBS's next one and
/// starts there, at the live edge: the added destination's viewers watch
/// live while the other destination and the dashboard are on the delay.
#[tokio::test]
async fn a_destination_added_mid_delay_on_a_long_gop_never_airs_live() {
    let mut sim = delay_sim().await;
    destination_added_mid_delay_on_a_long_gop(&mut sim).await;

    let frames = sim.frames(1);
    let first = frames.first().expect("the added destination is on air");
    let joined = delay_ms(&sim, first);
    assert!(
        joined + TIMER_SLOP_MS >= DELAY_MS,
        "joined {joined} ms behind OBS with a {DELAY_MS} ms delay on"
    );
}

/// Going delayed with a 4 s keyframe interval: a 2 s delay can only land
/// on a keyframe about 0 s or 4 s back, and it takes the one at least the
/// delay old, then stays put instead of chasing the exact delay.
///
/// The re-cut dead band (2.5 s here) used to apply to going delayed too:
/// live sat 2 s from the 2 s delay, inside the band, so the pump never cut
/// and viewers kept watching live while the dashboard said delayed.
#[tokio::test]
async fn activating_with_a_long_gop_never_lands_short_of_the_delay() {
    const GOP_4S: u32 = 40;
    let mut sim = delay_sim().await;
    sim.gop_frames = GOP_4S;
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(4_500).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(1_500).await;

    let frames = sim.frames(0);
    let back = backward_jumps(&frames);
    assert_eq!(back.len(), 1, "exactly one jump back");
    let i = back[0];
    assert!(frames[i].keyframe, "lands on a keyframe");
    assert_delay_honored(&sim, &frames[i..], DELAY_MS, GOP_4S * FRAME_MS, "delayed");
    assert_strictly_increasing(&frames[i..], "delayed");
    assert_eq!(cuts(&sim, "platform"), 1, "no re-cut once on the delay");
    sim.assert_timestamps_monotonic(0);
}

/// OBS's 32-bit millisecond clock rolls over every 49.7 days. Going
/// delayed across the rollover (live is past it, the delay before it) and
/// cutting back across it again must behave exactly as anywhere else.
#[tokio::test]
async fn going_delayed_and_back_across_the_timestamp_wrap() {
    let mut sim = delay_sim().await;
    sim.ctrl.arm_delay(DELAY_MS);
    // The clock rolls over 1.5 s in.
    sim.obs_connects_at(0u32.wrapping_sub(1_500)).await;
    sim.destination_connects("platform").await;
    sim.obs_sends(2_100).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(1_000).await;
    let cut_at = Instant::now();
    sim.ctrl.stop_delay();
    sim.obs_sends(1_500).await;

    let frames = sim.frames(0);
    let back = backward_jumps(&frames);
    assert_eq!(back.len(), 1, "exactly one jump back");
    let ahead = forward_jumps(&frames);
    assert_eq!(ahead.len(), 1, "exactly one jump forward");
    let (from, to) = (back[0], ahead[0]);
    assert!(frames[from].keyframe && frames[to].keyframe);
    assert!(frames[to].at > cut_at);
    assert_delay_honored(&sim, &frames[from..to], DELAY_MS, GOP_MS, "delayed");
    let aired_before = frames[..to].iter().map(|f| f.number).max().unwrap();
    assert!(frames[to].number > aired_before, "nothing is replayed");
    assert_delays(&sim, &frames[to..], 0, GOP_MS + JITTER_MS, "live again");
    assert_eq!(cuts(&sim, "platform"), 2);
    sim.assert_timestamps_monotonic(0);
    assert_audio_kept_up(&mut sim, 0, 3);
}

/// OBS drops and reconnects inside the half second the pump waits for its
/// next frame, with crash protection off: the pump never sees it gone,
/// only a new publisher. It carries on on the same connection, from a
/// keyframe of the new session, with its timestamps still going forward.
async fn obs_reconnects_inside_the_pumps_wait(sim: &mut Sim) -> Instant {
    sim.obs_sends(1_000).await;
    wait(100).await;
    sim.obs_crashes();
    wait(100).await;
    sim.obs_connects().await;
    let back = Instant::now();
    sim.obs_sends(1_500).await;
    back
}

fn assert_rejoined_the_new_session(sim: &mut Sim, back: Instant) {
    assert!(
        !sim.commands(0).contains(&"deleteStream".into()),
        "the destination never ended"
    );
    assert!(sim.received().iter().all(|m| m.conn == 0), "one connection");
    let frames = sim.frames(0);
    let rejoined = frames
        .iter()
        .find(|f| f.at > back)
        .expect("the stream came back");
    assert!(rejoined.keyframe, "rejoins at a keyframe");
    let paused = rejoined.at.duration_since(back).as_millis() as u32;
    assert!(
        paused <= GOP_MS + FRAME_MS + JITTER_MS,
        "back {paused} ms after OBS"
    );
    assert_strictly_increasing(&frames, "across the reconnect");
    sim.assert_timestamps_monotonic(0);
}

/// A destination that joined mid-stream, on a keyframe after OBS's first.
#[tokio::test]
async fn a_fast_reconnect_rejoins_the_new_session_on_a_keyframe() {
    let mut sim = delay_sim().await;
    sim.obs_connects().await;
    // The newest frame is a keyframe: the pump joins on it and runs level
    // with OBS, parked on the next frame when OBS drops.
    sim.obs_sends(600).await;
    sim.destination_connects("platform").await;
    let back = obs_reconnects_inside_the_pumps_wait(&mut sim).await;
    assert_rejoined_the_new_session(&mut sim, back);
}

/// The same, for a destination that joined on OBS's very first keyframe
/// (the usual case: it connects as OBS starts). Its timeline is anchored at
/// input ts 0, so the new session's first keyframe, also at ts 0, passes
/// the "older than the anchor" guard. It used to go out before the pump
/// noticed the new publisher, on output timestamp 0, back at the start of
/// the stream.
#[tokio::test]
async fn a_fast_reconnect_keeps_timestamps_forward_when_the_pump_joined_at_the_start() {
    let mut sim = delay_sim().await;
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    let back = obs_reconnects_inside_the_pumps_wait(&mut sim).await;
    assert_rejoined_the_new_session(&mut sim, back);
}

/// OBS's encoder restarts mid-session with a new resolution: new config,
/// then a keyframe. Returns the header and the first frame made with it.
async fn obs_changes_resolution_mid_stream(sim: &mut Sim) -> (Vec<u8>, u32) {
    // Audio level with the video, the order OBS's interleaver sends in. With
    // audio stamped behind, the audio config resent on the video's newer
    // timestamp would run ahead of the audio still to go out.
    sim.audio_lag_ms = 0;
    sim.obs_connects().await;
    sim.destination_connects("platform").await;
    sim.obs_sends(1_000).await;
    let header = sim.obs_changes_resolution(128, 72);
    let first_new = sim.next_frame;
    sim.obs_sends(1_000).await;
    (header, first_new)
}

/// Where connection 0 received `header`, and the last timestamp of media
/// it had received by then.
fn header_arrival(sim: &mut Sim, header: &[u8]) -> (usize, u32, u32) {
    let received = sim.received();
    let at = received
        .iter()
        .position(|m| m.conn == 0 && m.kind == 9 && m.payload == header)
        .expect("the new sequence header was sent");
    let last_media_ts = received[..at]
        .iter()
        .filter(|m| m.conn == 0 && (m.kind == 8 || m.kind == 9))
        .map(|m| m.ts)
        .max()
        .unwrap();
    (at, received[at].ts, last_media_ts)
}

/// A new sequence header mid-stream is resent once, on the last output
/// timestamp (never behind what already went out), with the audio config
/// alongside, and the stream carries on.
#[tokio::test]
async fn a_new_sequence_header_is_resent_mid_stream_on_the_last_timestamp() {
    let mut sim = delay_sim().await;
    let (header, first_new) = obs_changes_resolution_mid_stream(&mut sim).await;

    let (at, ts, last_media_ts) = header_arrival(&mut sim, &header);
    assert_eq!(ts, last_media_ts, "on the last output timestamp");
    let sent = sim
        .received()
        .iter()
        .filter(|m| m.conn == 0 && m.payload == header)
        .count();
    assert_eq!(sent, 1, "resent once");
    let audio_config = sim.received()[at..]
        .iter()
        .find(|m| m.conn == 0 && m.kind == 8 && m.payload.get(1) == Some(&0));
    assert_eq!(
        audio_config.map(|m| m.ts),
        Some(ts),
        "audio config alongside"
    );
    let frames = sim.frames(0);
    let second_new = frames
        .iter()
        .find(|f| f.number == first_new + 1)
        .expect("the stream carried on");
    assert!(at < second_new.index, "before the stream moved on");
    sim.assert_timestamps_monotonic(0);
}

/// The header has to reach the decoder before the frame encoded with it.
/// The pump compares the header generation at the top of its loop, then
/// parks waiting for the next frame; the frame that wakes it used to go
/// out before the loop came round again, decoded with the old config.
#[tokio::test]
async fn a_new_sequence_header_goes_out_before_the_frame_that_needs_it() {
    let mut sim = delay_sim().await;
    let (header, first_new) = obs_changes_resolution_mid_stream(&mut sim).await;

    let (at, _, _) = header_arrival(&mut sim, &header);
    let frames = sim.frames(0);
    let needs_it = frames
        .iter()
        .find(|f| f.number == first_new)
        .expect("the new keyframe aired");
    assert!(
        at < needs_it.index,
        "frame {first_new} went out before its sequence header"
    );
}

/// OBS stops and starts again while a delay is on air, crash protection
/// off. The destination ends with OBS, and the supervisor reconnects it
/// once OBS is back. Returns the first frame OBS made after the restart.
async fn obs_restarts_mid_delay(sim: &mut Sim) -> u32 {
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    sim.destination_runs("platform");
    sim.obs_sends(1_700).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    sim.obs_sends(500).await;
    sim.obs_stops();
    wait(700).await;
    sim.obs_connects().await;
    let first_new = sim.next_frame;
    sim.obs_sends(2_500).await;
    first_new
}

#[tokio::test]
async fn obs_restarting_mid_delay_brings_the_destination_back_on_a_keyframe() {
    let mut sim = delay_sim().await;
    let first_new = obs_restarts_mid_delay(&mut sim).await;

    assert_eq!(sim.ctrl.target_delay_ms(), DELAY_MS, "the delay stays on");
    assert!(sim.commands(0).contains(&"deleteStream".into()));
    let frames = sim.frames(1);
    let first = frames.first().expect("the destination came back");
    assert!(first.keyframe, "starts on a keyframe");
    assert!(first.number >= first_new, "on the new session's video");
    sim.assert_timestamps_monotonic(1);
}

/// The delay stays on through the restart, so viewers must come back
/// delayed: never on live video, and never jumping back once the buffer
/// has rebuilt. The restart empties the buffer, so the destination waits
/// for it to reach back to the delay before it goes on air again.
#[tokio::test]
async fn obs_restarting_mid_delay_never_puts_viewers_on_live() {
    let mut sim = delay_sim().await;
    obs_restarts_mid_delay(&mut sim).await;

    let frames = sim.frames(1);
    let first = frames.first().expect("the destination came back");
    let joined = delay_ms(&sim, first);
    assert!(
        joined + TIMER_SLOP_MS >= DELAY_MS,
        "came back {joined} ms behind OBS with a {DELAY_MS} ms delay on"
    );
    assert_strictly_increasing(&frames, "after the restart");
}

/// Twitch's Enhanced Broadcasting binds each track's decoder config to its
/// session, so every connection has to lead with every track's config,
/// including the one made when the supervisor restarts egress while OBS
/// keeps publishing. A cache that kept only the last track (the shape that
/// shipped once) leaves the others with none, and Twitch drops the stream.
#[tokio::test]
async fn every_track_config_leads_each_twitch_connection() {
    let mut sim = delay_sim().await;
    sim.obs_connects().await;
    for track in 1u8..=3 {
        let mut rung = vec![0x96, 0x00, b'a', b'v', b'c', b'1', track];
        rung.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        sim.ctrl.on_tag(9, 0, &rung, false, true);
    }
    let twitch = sim.ctrl.destination_state("twitch");
    twitch
        .pass_through_multitrack_video
        .store(true, Ordering::Relaxed);
    sim.destination_connects("twitch").await;
    sim.obs_sends(600).await;
    twitch.shutdown_requested.store(true, Ordering::Relaxed);
    sim.obs_sends(300).await;
    assert!(sim.pumps[0].is_finished(), "the first session ended");
    twitch.shutdown_requested.store(false, Ordering::Relaxed);
    sim.destination_connects("twitch").await;
    sim.obs_sends(600).await;

    for conn in [0, 1] {
        let first = sim.frames(conn).first().expect("frames aired").index;
        let configs: HashSet<&[u8]> = sim.received()[..first]
            .iter()
            .filter(|m| m.conn == conn && m.kind == 9)
            .map(|m| m.payload.as_slice())
            .collect();
        assert_eq!(configs.len(), 4, "connection {conn} led with every track");
    }
}

/// A delay longer than the buffer can hold at this bitrate (a hotkey,
/// MIDI or auto-arm skip the dashboard's capacity check, and the bitrate
/// can rise after arming) must not wait forever. Once the buffer is full
/// it counts as ready, the destination joins as far back as the buffer
/// safely reaches, and the stream runs there without stalling or
/// replaying. The log says the delay is capped.
#[tokio::test]
async fn a_delay_longer_than_the_buffer_runs_at_what_it_holds() {
    // About 7.5 s of stream (24 bytes a frame at 10 fps).
    let mut sim = Sim::with_ring_bytes(false, 1_800).await;
    sim.ctrl.arm_delay(30_000);
    sim.obs_connects().await;
    sim.obs_sends(9_000).await;
    assert_eq!(
        sim.ctrl.phase(),
        "ready",
        "a full buffer holds all it ever will"
    );
    sim.ctrl
        .activate_delay()
        .expect("activates at what the buffer holds");
    sim.destination_runs("platform");
    sim.obs_sends(6_000).await;

    let frames = sim.frames(0);
    assert!(
        frames.len() >= 30,
        "the destination joined and kept streaming: {} frames",
        frames.len()
    );
    assert_strictly_increasing(&frames, "a delay capped by the buffer");
    let behind = delay_ms(&sim, frames.last().unwrap());
    assert!(
        (2_500..=7_500).contains(&behind),
        "delayed by about what the buffer holds, not {behind} ms"
    );
    let gaps: Vec<u32> = frames
        .windows(2)
        .map(|w| w[1].at.duration_since(w[0].at).as_millis() as u32)
        .collect();
    assert!(
        gaps.iter().all(|gap| *gap < 1_000),
        "no stall while capped: {gaps:?}"
    );
    assert!(
        sim.ctrl
            .logs
            .lock()
            .iter()
            .any(|line| line.contains("the buffer is full")),
        "the log says the delay is capped"
    );
    sim.assert_timestamps_monotonic(0);
}

/// The keyframe interval is measured from the first few gaps only. When
/// the encoder's real interval turns out much longer (OBS's "auto" after a
/// restart, or early scene-cut keyframes), a landed delay can sit more than
/// the expected band above its target. Cutting to fix that must move
/// forward: the keyframe at the delay is the one the pump already passed,
/// and cutting back to it replayed the same stretch on every check.
#[tokio::test]
async fn a_longer_keyframe_interval_than_measured_never_replays() {
    let mut sim = delay_sim().await;
    sim.ctrl.arm_delay(DELAY_MS);
    sim.obs_connects().await;
    // The measurement: keyframes every 500 ms (frames 0..29).
    sim.obs_sends(3_000).await;
    sim.ctrl
        .activate_delay()
        .expect("the buffer holds the delay");
    // Then the real interval: a keyframe every 4 s, at frames 40 and 80.
    sim.gop_frames = 40;
    // Join 1 s after keyframe 80: it isn't the delay old yet, so the join
    // lands on keyframe 40, about 5 s back - past the expected band.
    sim.obs_sends(6_000).await;
    sim.destination_connects("platform").await;
    sim.obs_sends(10_000).await;

    let frames = sim.frames(0);
    let back = backward_jumps(&frames);
    assert!(back.is_empty(), "the picture jumped back at {back:?}");
    assert_airs_at_most_twice(&frames);
    sim.assert_timestamps_monotonic(0);
}
