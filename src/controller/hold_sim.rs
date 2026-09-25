//! Crash-protection simulations. A real destination pump streams to an
//! in-process platform (`sink::record`) that keeps everything it receives,
//! while the test plays OBS by feeding tags to the controller in real
//! time: live, crash, freeze, stop, back again. The assertions are about
//! what viewers would see: the tail airs once, the screen covers the gap,
//! nothing from before the hold is replayed, and timestamps never go back.

use super::*;
use crate::rtmp::client::{EgressClient, EgressUrl};
use crate::sink::{record, Received};
use std::path::PathBuf;
use tokio::sync::mpsc;

/// OBS's frame interval (10 fps) and keyframe cadence (every 500 ms).
const FRAME_MS: u32 = 100;
const GOP_FRAMES: u32 = 5;
/// The stream's resolution: small, so the screen encodes in no time.
const WIDTH: usize = 64;
const HEIGHT: usize = 36;

struct Sim {
    ctrl: Arc<Controller>,
    ring_path: PathBuf,
    port: u16,
    rx: mpsc::UnboundedReceiver<Received>,
    got: Vec<Received>,
    /// Frames get a number that never repeats, across OBS sessions, so a
    /// replay shows up as a number seen twice.
    next_frame: u32,
    produced: HashMap<u32, Instant>,
    /// This OBS session's wire clock.
    wire_ts: u32,
    pumps: Vec<tokio::task::JoinHandle<io::Result<()>>>,
}

impl Sim {
    async fn new(protected: bool) -> Sim {
        static UNIQ: AtomicU32 = AtomicU32::new(0);
        let n = UNIQ.fetch_add(1, Ordering::Relaxed);
        let ring_path =
            std::env::temp_dir().join(format!("ic-hold-sim-{}-{n}.buf", std::process::id()));
        let _ = std::fs::remove_file(&ring_path);
        let ring = Arc::new(DiskRing::create(&ring_path, 16 * 1024 * 1024).unwrap());
        let ctrl = Arc::new(Controller::new(ring, 0));
        ctrl.update_crash_protection(crate::crash_protection::CrashProtection {
            enabled: protected,
            ..Default::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(record(listener, tx));
        Sim {
            ctrl,
            ring_path,
            port,
            rx,
            got: Vec::new(),
            next_frame: 0,
            produced: HashMap::new(),
            wire_ts: 0,
            pumps: Vec::new(),
        }
    }

    /// OBS starts publishing: a new session, its clock from 0, the
    /// sequence headers, and a keyframe first.
    async fn obs_connects(&mut self) {
        self.ctrl.begin_publish("key", "127.0.0.1").await.unwrap();
        self.wire_ts = 0;
        let video = crate::slate::test_sequence_header(WIDTH, HEIGHT);
        self.ctrl.on_tag(9, 0, &video, false, true);
        // AAC-LC, 48 kHz, stereo.
        self.ctrl.on_tag(8, 0, &[0xAF, 0, 0x11, 0x90], false, true);
        self.next_frame = self.next_frame.div_ceil(GOP_FRAMES) * GOP_FRAMES;
    }

    /// OBS sends `ms` of video and audio in real time.
    async fn obs_sends(&mut self, ms: u32) {
        for _ in 0..ms / FRAME_MS {
            let frame = self.next_frame;
            let keyframe = frame.is_multiple_of(GOP_FRAMES);
            self.ctrl.on_tag(
                9,
                self.wire_ts,
                &frame_tag(frame, keyframe),
                keyframe,
                false,
            );
            self.ctrl
                .on_tag(8, self.wire_ts, &[0xAF, 1, 0x21, 0x00], false, false);
            self.produced.insert(frame, Instant::now());
            self.next_frame += 1;
            self.wire_ts += FRAME_MS;
            tokio::time::sleep(Duration::from_millis(u64::from(FRAME_MS))).await;
        }
    }

    /// OBS is connected but sends nothing for `ms` (its clock runs on).
    async fn obs_stalls(&mut self, ms: u32) {
        wait(ms).await;
        self.wire_ts += ms;
    }

    fn obs_crashes(&self) {
        self.ctrl.mark_ingest_dead();
    }

    fn obs_stops(&self) {
        self.ctrl.note_unpublish();
        self.ctrl.mark_ingest_dead();
    }

    /// What the supervisor does once OBS has sent no video for a while.
    fn freeze_detected(&self) {
        let frozen_at = process_now_ms() + crate::crash_hold::FREEZE_AFTER.as_millis() as u64;
        self.ctrl.check_ingest_freeze_at(frozen_at);
    }

    /// A destination connects to the platform and its pump starts.
    async fn destination_connects(&mut self, id: &str) {
        let dest = self.ctrl.destination_state(id);
        dest.egress_alive.store(true, Ordering::Relaxed);
        let url = EgressUrl::parse(&format!("rtmp://127.0.0.1:{}/live/key", self.port)).unwrap();
        let sink = EgressClient::connect(&url)
            .await
            .unwrap()
            .spawn_reader_drain();
        let ctrl = self.ctrl.clone();
        self.pumps.push(tokio::spawn(
            async move { pump_dest(&ctrl, &dest, sink).await },
        ));
    }

    /// Everything the platform has received so far.
    fn received(&mut self) -> &[Received] {
        while let Ok(message) = self.rx.try_recv() {
            self.got.push(message);
        }
        &self.got
    }

    /// OBS's frames as connection `conn` received them.
    fn frames(&mut self, conn: usize) -> Vec<Frame> {
        self.received()
            .iter()
            .filter(|m| m.conn == conn && m.kind == 9)
            .filter_map(|m| {
                let number = frame_number(&m.payload)?;
                Some(Frame {
                    number,
                    keyframe: m.payload[0] == 0x17,
                    at: m.at,
                })
            })
            .collect()
    }

    /// Reconnect-screen video tags connection `conn` received in `window`.
    fn screen_tags(&mut self, conn: usize, from: Instant, to: Instant) -> usize {
        self.received()
            .iter()
            .filter(|m| m.conn == conn && m.kind == 9 && (from..to).contains(&m.at))
            .filter(|m| m.payload.get(1) == Some(&1) && frame_number(&m.payload).is_none())
            .count()
    }

    fn commands(&mut self, conn: usize) -> Vec<String> {
        self.received()
            .iter()
            .filter(|m| m.conn == conn)
            .filter_map(|m| m.command.clone())
            .collect()
    }

    /// Video and audio timestamps each never go backwards.
    fn assert_timestamps_monotonic(&mut self, conn: usize) {
        for kind in [8, 9] {
            let stamps: Vec<u32> = self
                .received()
                .iter()
                .filter(|m| m.conn == conn && m.kind == kind)
                .map(|m| m.ts)
                .collect();
            let back = stamps.windows(2).position(|pair| pair[1] < pair[0]);
            assert_eq!(
                back, None,
                "type {kind} timestamps went backwards: {stamps:?}"
            );
        }
    }
}

impl Drop for Sim {
    fn drop(&mut self) {
        for pump in &self.pumps {
            pump.abort();
        }
        let _ = std::fs::remove_file(&self.ring_path);
    }
}

#[derive(Debug, Clone, Copy)]
struct Frame {
    number: u32,
    keyframe: bool,
    at: Instant,
}

/// A legacy AVC video tag carrying frame `number` in its NAL unit.
fn frame_tag(number: u32, keyframe: bool) -> Vec<u8> {
    let (flags, nal_type) = if keyframe { (0x17, 0x65) } else { (0x27, 0x41) };
    let mut tag = vec![flags, 1, 0, 0, 0, 0, 0, 0, 7, nal_type, b'I', b'C'];
    tag.extend_from_slice(&number.to_be_bytes());
    tag
}

fn frame_number(payload: &[u8]) -> Option<u32> {
    if payload.len() != 16 || payload[1] != 1 || &payload[10..12] != b"IC" {
        return None;
    }
    Some(u32::from_be_bytes(payload[12..16].try_into().ok()?))
}

async fn wait(ms: u32) {
    tokio::time::sleep(Duration::from_millis(u64::from(ms))).await;
}

fn assert_strictly_increasing(frames: &[Frame], what: &str) {
    let numbers: Vec<u32> = frames.iter().map(|f| f.number).collect();
    let replay = numbers.windows(2).position(|pair| pair[1] <= pair[0]);
    assert_eq!(replay, None, "{what}: a frame was replayed: {numbers:?}");
}

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
