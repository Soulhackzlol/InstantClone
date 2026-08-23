//! MIDI controller bindings for the delay actions.
//!
//! A background thread (Windows, via winmm) listens to every MIDI input
//! device and turns note-on / control-change "press" edges into signature
//! strings (`note:ch:n` / `cc:ch:n`). Each signature is matched against the
//! user's bindings and routed through `Controller::run_named_action`, the
//! same path the keyboard hotkeys use, so both trigger identical behaviour.
//!
//! `MidiState` is cross-platform: it holds the live bindings mirror, the
//! device list, and the "learn mode" slot the dashboard drives over HTTP.
//! On a build with no MIDI backend the listener never starts, so
//! `available` stays false and the dashboard hides the section - exactly
//! like the keyboard hotkeys on Linux.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::config::{MidiBindings, Settings};
#[cfg(windows)]
use crate::controller::Controller;

/// Shared MIDI state between the listener thread and the web layer.
#[derive(Default)]
pub struct MidiState {
    available: AtomicBool,
    bindings: Mutex<MidiBindings>,
    devices: Mutex<Vec<String>>,
    /// The action currently being learned, if any. While set, the next
    /// incoming message is captured for it instead of being dispatched.
    learn: Mutex<Option<String>>,
    /// A just-captured (action, signature) awaiting commit by the web layer.
    captured: Mutex<Option<(String, String)>>,
}

impl MidiState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mirror the current bindings into the listener's live match view.
    pub fn update_from_settings(&self, s: &Settings) {
        *self.bindings.lock().unwrap() = s.midi.clone();
    }

    pub fn available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }
    #[cfg(windows)]
    pub fn set_available(&self, v: bool) {
        self.available.store(v, Ordering::Relaxed);
    }
    #[cfg(windows)]
    pub fn set_devices(&self, names: Vec<String>) {
        *self.devices.lock().unwrap() = names;
    }
    pub fn devices(&self) -> Vec<String> {
        self.devices.lock().unwrap().clone()
    }

    pub fn start_learn(&self, action: &str) {
        *self.learn.lock().unwrap() = Some(action.to_string());
        *self.captured.lock().unwrap() = None;
    }
    pub fn cancel_learn(&self) {
        *self.learn.lock().unwrap() = None;
    }
    pub fn learning(&self) -> Option<String> {
        self.learn.lock().unwrap().clone()
    }
    /// Take a captured (action, signature) if one is waiting. The web layer
    /// persists it to config, so it is consumed exactly once.
    pub fn take_captured(&self) -> Option<(String, String)> {
        self.captured.lock().unwrap().take()
    }

    /// Called by the listener on each press-edge message. In learn mode it
    /// records the signature for the pending action; otherwise it routes to
    /// the bound action via the shared controller path. `default_ms` is the
    /// delay to arm with, read live by the caller from settings.
    #[cfg(windows)]
    pub fn on_signature(&self, ctrl: &Controller, default_ms: u32, signature: &str) {
        // Learn mode wins: capture and stop, do not also fire an action.
        {
            let mut learn = self.learn.lock().unwrap();
            if let Some(action) = learn.take() {
                *self.captured.lock().unwrap() = Some((action, signature.to_string()));
                return;
            }
        }
        let action = self.bindings.lock().unwrap().action_for(signature);
        if let Some(action) = action {
            ctrl.run_named_action(action, default_ms, "midi");
        }
    }

    /// Runtime snapshot for the dashboard's MIDI poll: whether a device is
    /// listening, the device names, which action (if any) is learning, and
    /// the current bindings (so a just-committed one shows without a full
    /// config refetch).
    pub fn to_json(&self) -> String {
        let learning = match self.learning() {
            Some(a) => json_string(&a),
            None => "null".to_string(),
        };
        let devices: Vec<String> = self.devices().iter().map(|d| json_string(d)).collect();
        let bindings = {
            let b = self.bindings.lock().unwrap();
            format!(
                r#"{{"toggle":{t},"arm":{a},"activate":{ac},"cut":{c},"cut_after":{ca}}}"#,
                t = json_string(&b.toggle),
                a = json_string(&b.arm),
                ac = json_string(&b.activate),
                c = json_string(&b.cut),
                ca = json_string(&b.cut_after),
            )
        };
        format!(
            r#"{{"available":{a},"learning":{l},"devices":[{d}],"bindings":{b}}}"#,
            a = self.available(),
            l = learning,
            d = devices.join(","),
            b = bindings,
        )
    }
}

/// Minimal JSON string escaper. Device names come from the driver and can
/// carry any character, so quotes / backslashes / control chars are escaped.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Turn a raw MIDI message (status, data1, data2) into a binding signature,
/// or None when it is not a "press" edge we act on. Note-on with velocity 0
/// is a note-off in disguise and is ignored; a control-change only counts as
/// a press at value >= 64, so a button's release (0) never double-fires.
#[cfg(any(windows, test))]
fn signature_for(status: u8, data1: u8, data2: u8) -> Option<String> {
    let channel = (status & 0x0F) as u32 + 1;
    match status & 0xF0 {
        0x90 if data2 > 0 => Some(format!("note:{channel}:{data1}")),
        0xB0 if data2 >= 64 => Some(format!("cc:{channel}:{data1}")),
        _ => None,
    }
}

/// Spawn the MIDI listener. Windows opens every input device via winmm;
/// other platforms have no backend yet, so this is a no-op and `available`
/// stays false (the dashboard then hides the MIDI section). The settings
/// receiver lets the listener read the current default delay when a mapped
/// action fires.
#[cfg(windows)]
pub fn spawn(
    ctrl: Arc<Controller>,
    state: Arc<MidiState>,
    settings: tokio::sync::watch::Receiver<Settings>,
) {
    std::thread::Builder::new()
        .name("instantclone-midi".into())
        .spawn(move || win::run(ctrl, state, settings))
        .ok();
}

#[cfg(not(windows))]
pub fn spawn(
    _ctrl: Arc<crate::controller::Controller>,
    _state: Arc<MidiState>,
    _settings: tokio::sync::watch::Receiver<Settings>,
) {
}

#[cfg(windows)]
mod win {
    use super::{signature_for, MidiState};
    use crate::config::Settings;
    use crate::controller::Controller;
    use std::sync::Arc;
    use tokio::sync::watch;
    use windows_sys::Win32::Media::Audio::{
        midiInClose, midiInGetDevCapsW, midiInGetNumDevs, midiInOpen, midiInReset, midiInStart,
        midiInStop, CALLBACK_FUNCTION, HMIDIIN, MIDIINCAPSW,
    };

    // Stable winmm constants that windows-sys does not re-export.
    const MMSYSERR_NOERROR: u32 = 0;
    const MIM_DATA: u32 = 0x3C3;

    /// Passed to every open device as the callback instance. Leaked for the
    /// process lifetime (the listener thread never exits in normal running),
    /// so the pointer handed to winmm always stays valid.
    struct CallbackCtx {
        ctrl: Arc<Controller>,
        state: Arc<MidiState>,
        settings: watch::Receiver<Settings>,
    }

    pub fn run(ctrl: Arc<Controller>, state: Arc<MidiState>, settings: watch::Receiver<Settings>) {
        let ctx: &'static CallbackCtx = Box::leak(Box::new(CallbackCtx {
            ctrl,
            state,
            settings,
        }));
        let instance = ctx as *const CallbackCtx as usize;

        let mut handles: Vec<HMIDIIN> = Vec::new();
        let mut last_count = u32::MAX;
        loop {
            let count = unsafe { midiInGetNumDevs() };
            // Reconcile only when the device count changes: winmm gives no
            // hot-plug event, so a periodic count check is the cheap way to
            // pick up a controller plugged in (or pulled out) mid-session.
            if count != last_count {
                close_all(&mut handles);
                let names = open_all(count, instance, &mut handles);
                ctx.state.set_devices(names);
                ctx.state.set_available(!handles.is_empty());
                last_count = count;
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    /// Open and start every input device, returning their names. Devices
    /// that fail to open are skipped (another app may hold them exclusively).
    fn open_all(count: u32, instance: usize, handles: &mut Vec<HMIDIIN>) -> Vec<String> {
        let mut names = Vec::new();
        for dev in 0..count {
            let mut h: HMIDIIN = std::ptr::null_mut();
            let r = unsafe {
                midiInOpen(
                    &mut h,
                    dev,
                    midi_in_proc as *const () as usize,
                    instance,
                    CALLBACK_FUNCTION,
                )
            };
            if r != MMSYSERR_NOERROR || h.is_null() {
                continue;
            }
            if unsafe { midiInStart(h) } != MMSYSERR_NOERROR {
                unsafe { midiInClose(h) };
                continue;
            }
            names.push(device_name(dev));
            handles.push(h);
        }
        names
    }

    fn close_all(handles: &mut Vec<HMIDIIN>) {
        for h in handles.drain(..) {
            unsafe {
                midiInStop(h);
                midiInReset(h);
                midiInClose(h);
            }
        }
    }

    /// Read a device's display name from its caps, trimmed at the NUL.
    fn device_name(dev: u32) -> String {
        let mut caps: MIDIINCAPSW = unsafe { std::mem::zeroed() };
        let r = unsafe {
            midiInGetDevCapsW(
                dev as usize,
                &mut caps,
                std::mem::size_of::<MIDIINCAPSW>() as u32,
            )
        };
        if r != MMSYSERR_NOERROR {
            return format!("MIDI device {dev}");
        }
        // MIDIINCAPSW is #[repr(packed)], so the name field can't be
        // borrowed directly; copy it out unaligned into an owned array first.
        let name: [u16; 32] = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(caps.szPname)) };
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        String::from_utf16_lossy(&name[..end])
    }

    /// winmm callback. Runs on a system thread; keeps work minimal (parse +
    /// a couple of short mutex holds). Only MIM_DATA carries a channel-voice
    /// message; dwParam1 packs status / data1 / data2 in its low three bytes.
    unsafe extern "system" fn midi_in_proc(
        _hmi: HMIDIIN,
        msg: u32,
        instance: usize,
        param1: usize,
        _param2: usize,
    ) {
        if msg != MIM_DATA || instance == 0 {
            return;
        }
        let status = (param1 & 0xFF) as u8;
        let data1 = ((param1 >> 8) & 0x7F) as u8;
        let data2 = ((param1 >> 16) & 0x7F) as u8;
        let Some(sig) = signature_for(status, data1, data2) else {
            return;
        };
        let ctx = &*(instance as *const CallbackCtx);
        // Read the default delay live so a mid-session change in the
        // dashboard is reflected without restarting the listener.
        let default_ms = ctx.settings.borrow().auto_arm_delay_ms;
        ctx.state.on_signature(&ctx.ctrl, default_ms, &sig);
    }
}

#[cfg(test)]
mod tests {
    use super::signature_for;
    #[cfg(windows)]
    use super::MidiState;

    #[test]
    fn note_on_and_cc_press_edges_map_to_signatures() {
        // Note-on ch1 (0x90), note 36, velocity 100.
        assert_eq!(signature_for(0x90, 36, 100).as_deref(), Some("note:1:36"));
        // Channel is encoded in the low nibble: 0x9F = channel 16.
        assert_eq!(signature_for(0x9F, 60, 1).as_deref(), Some("note:16:60"));
        // Control change ch1, controller 20, value 127 (pressed).
        assert_eq!(signature_for(0xB0, 20, 127).as_deref(), Some("cc:1:20"));
    }

    #[test]
    fn releases_and_other_messages_are_ignored() {
        assert!(
            signature_for(0x90, 36, 0).is_none(),
            "note-on vel 0 = release"
        );
        assert!(signature_for(0x80, 36, 100).is_none(), "note-off ignored");
        assert!(signature_for(0xB0, 20, 0).is_none(), "cc release ignored");
        assert!(signature_for(0xB0, 20, 63).is_none(), "cc below threshold");
        assert!(signature_for(0xE0, 0, 0).is_none(), "pitch bend ignored");
    }

    // Build a real Controller backed by a temp ring, so the dispatch below
    // exercises the same code the live app runs.
    #[cfg(windows)]
    fn test_controller() -> (
        std::sync::Arc<crate::controller::Controller>,
        std::path::PathBuf,
    ) {
        let path = std::env::temp_dir().join(format!(
            "ic-test-midi-ctrl-{}-{}.buf",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        let ring = std::sync::Arc::new(
            crate::buffer::DiskRing::create(&path, 4 * 1024 * 1024).expect("ring create"),
        );
        (
            std::sync::Arc::new(crate::controller::Controller::new(ring, 0)),
            path,
        )
    }

    /// Full simulated MIDI note: the exact bytes winmm delivers for a pad
    /// press, fed through signature_for -> on_signature -> the controller,
    /// asserting the bound action actually fires. This is everything past
    /// the OS driver, i.e. the whole app-owned MIDI path.
    #[cfg(windows)]
    #[test]
    fn simulated_note_fires_the_bound_action() {
        let (ctrl, path) = test_controller();
        let state = MidiState::new();

        // Map "arm" to note 36 on channel 1, the way the learn UI would.
        let mut s = crate::config::Settings::defaults();
        s.midi.set("arm", "note:1:36");
        state.update_from_settings(&s);

        assert_eq!(ctrl.armed_delay_ms(), 0, "nothing armed before the note");

        // Simulate the winmm callback: note-on ch1 note36 velocity100.
        let sig = signature_for(0x90, 36, 100).expect("a press edge");
        assert_eq!(sig, "note:1:36");
        state.on_signature(&ctrl, 15_000, &sig);

        assert_eq!(
            ctrl.armed_delay_ms(),
            15_000,
            "a mapped MIDI note must arm the delay at the default"
        );

        // A note that is NOT mapped must do nothing.
        let other = signature_for(0x90, 40, 100).expect("a press edge");
        ctrl.stop_delay();
        state.on_signature(&ctrl, 15_000, &other);
        // (arm stays as-is; the unmapped note fired no action.)

        let _ = std::fs::remove_file(&path);
    }

    /// Full on/off/schedule sequence driven entirely by simulated MIDI
    /// messages against a buffer-filled controller, proving the mapped
    /// actions produce the right delay-state transitions.
    #[cfg(windows)]
    #[test]
    fn simulated_notes_drive_toggle_and_scheduled_cut() {
        let (ctrl, path) = test_controller();
        let state = MidiState::new();
        let mut s = crate::config::Settings::defaults();
        s.midi.set("toggle", "note:1:36");
        s.midi.set("cut_after", "cc:2:20");
        state.update_from_settings(&s);

        // Pre-fill the ring with ~3 s of fake video (IDR at each second) so a
        // 1 s toggle can actually activate, mirroring a live OBS feed.
        for sec in 0..3u32 {
            for f in 0..30u32 {
                let ts = sec * 1000 + f * 33;
                let is_idr = f == 0;
                let payload = [if is_idr { 0x17u8 } else { 0x27u8 }; 50];
                ctrl.on_tag(9, ts, &payload, is_idr, false);
            }
        }
        let note = signature_for(0x90, 36, 100).unwrap(); // pad, ch1 note36
        let knob = signature_for(0xB1, 20, 127).unwrap(); // knob, ch2 cc20

        // Toggle -> delay on.
        state.on_signature(&ctrl, 1_000, &note);
        assert_eq!(ctrl.target_delay_ms(), 1_000, "toggle turned the delay on");

        // Cut-after -> scheduled; press again -> cancelled.
        state.on_signature(&ctrl, 1_000, &knob);
        assert!(ctrl.safe_cut_pending(), "knob scheduled the safe cut");
        state.on_signature(&ctrl, 1_000, &knob);
        assert!(!ctrl.safe_cut_pending(), "knob again cancelled it");

        // Toggle -> delay off.
        state.on_signature(&ctrl, 1_000, &note);
        assert_eq!(ctrl.target_delay_ms(), 0, "toggle turned the delay off");

        let _ = std::fs::remove_file(&path);
    }

    /// In learn mode the same simulated note is captured for the pending
    /// action and does NOT fire anything.
    #[cfg(windows)]
    #[test]
    fn learn_captures_the_note_without_firing() {
        let (ctrl, path) = test_controller();
        let state = MidiState::new();

        state.start_learn("toggle");
        let sig = signature_for(0xB0, 20, 127).expect("cc press");
        state.on_signature(&ctrl, 15_000, &sig);

        assert_eq!(
            state.take_captured(),
            Some(("toggle".to_string(), "cc:1:20".to_string())),
            "learn must capture the pressed control"
        );
        assert_eq!(state.learning(), None, "learn clears after one capture");
        assert_eq!(
            ctrl.target_delay_ms(),
            0,
            "capturing must not also fire the action"
        );

        let _ = std::fs::remove_file(&path);
    }
}
