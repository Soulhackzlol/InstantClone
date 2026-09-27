//! The harness the pump simulations share. A real destination pump streams
//! to an in-process platform (`sink::record`) that keeps everything it
//! receives, while the test plays OBS by feeding tags to the controller in
//! real time. Every video frame and its audio carry a number that never
//! repeats across OBS sessions, so what viewers saw can be read back: a
//! replay is a number seen twice, and the delay is when a frame arrived
//! minus when OBS made it.

use super::*;
use crate::rtmp::client::{EgressClient, EgressUrl};
use crate::sink::{record, Received};
use std::path::PathBuf;
use tokio::sync::mpsc;

/// OBS's frame interval (10 fps) and default keyframe cadence (every 500 ms).
pub(super) const FRAME_MS: u32 = 100;
pub(super) const GOP_FRAMES: u32 = 5;
/// The stream's resolution: small, so the screen encodes in no time.
const WIDTH: usize = 64;
const HEIGHT: usize = 36;

pub(super) struct Sim {
    pub(super) ctrl: Arc<Controller>,
    ring_path: PathBuf,
    port: u16,
    rx: mpsc::UnboundedReceiver<Received>,
    got: Vec<Received>,
    /// Frames get a number that never repeats, across OBS sessions, so a
    /// replay shows up as a number seen twice.
    pub(super) next_frame: u32,
    pub(super) produced: HashMap<u32, Instant>,
    /// This OBS session's wire clock.
    wire_ts: u32,
    /// When the wire clock read `clock_wire_ts`. Frames go out on that
    /// schedule rather than one sleep after another, so the wire clock
    /// never drifts from the wall clock the pump paces by.
    clock_start: Instant,
    clock_wire_ts: u32,
    /// A keyframe every this many frames.
    pub(super) gop_frames: u32,
    /// How far each frame's audio is stamped behind its video. A real
    /// muxer's audio trails the video it arrives after by a few ms.
    pub(super) audio_lag_ms: u32,
    pub(super) pumps: Vec<tokio::task::JoinHandle<io::Result<()>>>,
}

impl Sim {
    pub(super) async fn new(protected: bool) -> Sim {
        static UNIQ: AtomicU32 = AtomicU32::new(0);
        let n = UNIQ.fetch_add(1, Ordering::Relaxed);
        let ring_path =
            std::env::temp_dir().join(format!("ic-pump-sim-{}-{n}.buf", std::process::id()));
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
            clock_start: Instant::now(),
            clock_wire_ts: 0,
            gop_frames: GOP_FRAMES,
            audio_lag_ms: 0,
            pumps: Vec::new(),
        }
    }

    /// OBS starts publishing: a new session, its clock from 0, the
    /// sequence headers, and a keyframe first.
    pub(super) async fn obs_connects(&mut self) {
        self.obs_connects_at(0).await;
    }

    /// OBS starts publishing with its wire clock at `wire_ts`.
    pub(super) async fn obs_connects_at(&mut self, wire_ts: u32) {
        self.ctrl.begin_publish("key", "127.0.0.1").await.unwrap();
        self.wire_ts = wire_ts;
        self.clock_start = Instant::now();
        self.clock_wire_ts = wire_ts;
        let video = crate::slate::test_sequence_header(WIDTH, HEIGHT);
        self.ctrl.on_tag(9, wire_ts, &video, false, true);
        // AAC-LC, 48 kHz, stereo.
        self.ctrl
            .on_tag(8, wire_ts, &[0xAF, 0, 0x11, 0x90], false, true);
        self.next_frame = self.next_frame.div_ceil(self.gop_frames) * self.gop_frames;
    }

    /// OBS sends `ms` of video and audio in real time.
    pub(super) async fn obs_sends(&mut self, ms: u32) {
        for _ in 0..ms / FRAME_MS {
            let frame = self.next_frame;
            let keyframe = frame.is_multiple_of(self.gop_frames);
            self.ctrl.on_tag(
                9,
                self.wire_ts,
                &frame_tag(frame, keyframe),
                keyframe,
                false,
            );
            let audio_ts = self.wire_ts.wrapping_sub(self.audio_lag_ms);
            self.ctrl
                .on_tag(8, audio_ts, &audio_tag(frame), false, false);
            self.produced.insert(frame, Instant::now());
            self.next_frame += 1;
            self.advance_clock(FRAME_MS).await;
        }
    }

    /// OBS is connected but sends nothing for `ms` (its clock runs on).
    pub(super) async fn obs_stalls(&mut self, ms: u32) {
        self.advance_clock(ms).await;
    }

    async fn advance_clock(&mut self, ms: u32) {
        self.wire_ts = self.wire_ts.wrapping_add(ms);
        let elapsed = self.wire_ts.wrapping_sub(self.clock_wire_ts);
        let due = self.clock_start + Duration::from_millis(u64::from(elapsed));
        tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
    }

    /// The encoder restarts mid-session with a new resolution: a new video
    /// sequence header, and the next frame is a keyframe.
    pub(super) fn obs_changes_resolution(&mut self, width: usize, height: usize) -> Vec<u8> {
        let header = crate::slate::test_sequence_header(width, height);
        self.ctrl.on_tag(9, self.wire_ts, &header, false, true);
        self.next_frame = self.next_frame.div_ceil(self.gop_frames) * self.gop_frames;
        header
    }

    pub(super) fn obs_crashes(&self) {
        self.ctrl.mark_ingest_dead();
    }

    pub(super) fn obs_stops(&self) {
        self.ctrl.note_unpublish();
        self.ctrl.mark_ingest_dead();
    }

    /// What the supervisor does once OBS has sent no video for a while.
    pub(super) fn freeze_detected(&self) {
        let frozen_at = process_now_ms() + crate::crash_hold::FREEZE_AFTER.as_millis() as u64;
        self.ctrl.check_ingest_freeze_at(frozen_at);
    }

    fn platform_url(&self) -> String {
        format!("rtmp://127.0.0.1:{}/live/key", self.port)
    }

    /// A destination connects to the platform and its pump starts.
    pub(super) async fn destination_connects(&mut self, id: &str) {
        let dest = self.ctrl.destination_state(id);
        dest.egress_alive.store(true, Ordering::Relaxed);
        let url = EgressUrl::parse(&self.platform_url()).unwrap();
        let sink = EgressClient::connect(&url)
            .await
            .unwrap()
            .spawn_reader_drain();
        let ctrl = self.ctrl.clone();
        self.pumps.push(tokio::spawn(
            async move { pump_dest(&ctrl, &dest, sink).await },
        ));
    }

    /// A destination the supervisor runs: it connects once OBS is sending,
    /// and reconnects (with its backoff) after every session ends.
    pub(super) fn destination_runs(&mut self, id: &str) {
        let dest = self.ctrl.destination_state(id);
        let ctrl = self.ctrl.clone();
        let url = self.platform_url();
        let label = id.to_string();
        self.pumps.push(tokio::spawn(async move {
            run_egress(ctrl, label, url, dest).await
        }));
    }

    /// Everything the platform has received so far.
    pub(super) fn received(&mut self) -> &[Received] {
        while let Ok(message) = self.rx.try_recv() {
            self.got.push(message);
        }
        &self.got
    }

    /// OBS's frames as connection `conn` received them.
    pub(super) fn frames(&mut self, conn: usize) -> Vec<Frame> {
        self.received()
            .iter()
            .enumerate()
            .filter(|(_, m)| m.conn == conn && m.kind == 9)
            .filter_map(|(index, m)| {
                let number = frame_number(&m.payload)?;
                Some(Frame {
                    number,
                    keyframe: m.payload[0] == 0x17,
                    ts: m.ts,
                    at: m.at,
                    index,
                })
            })
            .collect()
    }

    /// The numbers of the frames whose audio connection `conn` received.
    pub(super) fn audio_numbers(&mut self, conn: usize) -> Vec<u32> {
        self.received()
            .iter()
            .filter(|m| m.conn == conn && m.kind == 8)
            .filter_map(|m| audio_number(&m.payload))
            .collect()
    }

    /// Reconnect-screen video tags connection `conn` received in `window`.
    pub(super) fn screen_tags(&mut self, conn: usize, from: Instant, to: Instant) -> usize {
        self.received()
            .iter()
            .filter(|m| m.conn == conn && m.kind == 9 && (from..to).contains(&m.at))
            .filter(|m| m.payload.get(1) == Some(&1) && frame_number(&m.payload).is_none())
            .count()
    }

    pub(super) fn commands(&mut self, conn: usize) -> Vec<String> {
        self.received()
            .iter()
            .filter(|m| m.conn == conn)
            .filter_map(|m| m.command.clone())
            .collect()
    }

    /// Video and audio timestamps each never go backwards.
    pub(super) fn assert_timestamps_monotonic(&mut self, conn: usize) {
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
pub(super) struct Frame {
    pub(super) number: u32,
    pub(super) keyframe: bool,
    /// Output timestamp on the wire.
    pub(super) ts: u32,
    pub(super) at: Instant,
    /// Where it sits in everything the platform received.
    pub(super) index: usize,
}

/// A legacy AVC video tag carrying frame `number` in its NAL unit.
fn frame_tag(number: u32, keyframe: bool) -> Vec<u8> {
    let (flags, nal_type) = if keyframe { (0x17, 0x65) } else { (0x27, 0x41) };
    let mut tag = vec![flags, 1, 0, 0, 0, 0, 0, 0, 7, nal_type, b'I', b'C'];
    tag.extend_from_slice(&number.to_be_bytes());
    tag
}

pub(super) fn frame_number(payload: &[u8]) -> Option<u32> {
    if payload.len() != 16 || payload[1] != 1 || &payload[10..12] != b"IC" {
        return None;
    }
    Some(u32::from_be_bytes(payload[12..16].try_into().ok()?))
}

/// A legacy AAC tag carrying the number of the frame it belongs to.
fn audio_tag(number: u32) -> Vec<u8> {
    let mut tag = vec![0xAF, 1, b'I', b'C'];
    tag.extend_from_slice(&number.to_be_bytes());
    tag
}

fn audio_number(payload: &[u8]) -> Option<u32> {
    if payload.len() != 8 || payload[1] != 1 || &payload[2..4] != b"IC" {
        return None;
    }
    Some(u32::from_be_bytes(payload[4..8].try_into().ok()?))
}

pub(super) async fn wait(ms: u32) {
    tokio::time::sleep(Duration::from_millis(u64::from(ms))).await;
}

pub(super) fn assert_strictly_increasing(frames: &[Frame], what: &str) {
    let numbers: Vec<u32> = frames.iter().map(|f| f.number).collect();
    let replay = numbers.windows(2).position(|pair| pair[1] <= pair[0]);
    assert_eq!(replay, None, "{what}: a frame was replayed: {numbers:?}");
}
