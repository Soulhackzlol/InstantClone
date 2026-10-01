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

#[cfg(any(windows, test))]
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::{MidiBindings, Settings};
#[cfg(windows)]
use crate::controller::Controller;
use crate::sync::Mutex;

/// How long a learn request stays armed, and how long a capture waits to be
/// collected.
///
/// Learn mode swallows the next press instead of firing its action, so it
/// must not outlive the person who asked for it. The dashboard cancels when
/// the tab is hidden or closed, but a browser that is killed outright never
/// gets to, and without a deadline the listener would eat a press mid-stream
/// and then bind it to whatever was being learned days earlier, the moment
/// someone next opened the dashboard.
const LEARN_WINDOW: Duration = Duration::from_secs(30);

/// The learn the Integrations tab arms. Its pad may be on any device.
pub const INTEGRATION_LEARN: &str = "integration";

/// Shared MIDI state between the listener thread and the web layer.
#[derive(Default)]
pub struct MidiState {
    bindings: Mutex<MidiBindings>,
    /// Every input device the system reports, whether or not we opened it.
    /// The dashboard needs the full list to offer a choice.
    devices: Mutex<Vec<String>>,
    /// The devices we actually hold open and are hearing from.
    listening: Mutex<Vec<String>>,
    /// The device the user picked, by name. Empty means all of them.
    selected: Mutex<String>,
    /// The action currently being learned and when it stops being learned.
    /// While set, the next incoming message is captured for it instead of
    /// being dispatched.
    learn: Mutex<Option<(String, Instant)>>,
    /// A just-captured (action, signature) awaiting commit by the web layer,
    /// carrying the deadline of the learn that produced it.
    captured: Mutex<Option<(String, String, Instant)>>,
}

impl MidiState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mirror the current bindings and device choice into the listener's
    /// live view. The listener picks the device change up on its next sweep.
    pub fn update_from_settings(&self, s: &Settings) {
        *self.bindings.lock() = s.midi.clone();
        *self.selected.lock() = s.midi_device.clone();
    }

    /// The device the user picked, or empty for every device.
    pub fn selected_device(&self) -> String {
        self.selected.lock().clone()
    }

    /// Whether anything is actually being heard. Derived rather than
    /// stored: a flag beside the list is a second copy of the same fact,
    /// and the two drift the first time one of them is forgotten.
    pub fn available(&self) -> bool {
        !self.listening.lock().is_empty()
    }

    /// Whether there is a controller we *could* listen to.
    ///
    /// Distinct from `available`, which says whether a device is open right
    /// now. Since devices are only held while a binding exists or a learn is
    /// running, a fresh install with no bindings has nothing open - so
    /// gating "record a binding" on `available` would mean the first binding
    /// could never be made. Learn is gated on this instead: the sweep opens
    /// the device once the learn is armed.
    pub fn connected(&self) -> bool {
        let selected = self.selected_device();
        let devices = self.devices.lock();
        if selected.is_empty() {
            !devices.is_empty()
        } else {
            devices.contains(&selected)
        }
    }
    /// Pure state, no Win32: built everywhere so the tests that drive the
    /// device list run on every CI target, even though only the Windows
    /// listener calls it.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn set_devices(&self, present: Vec<String>, listening: Vec<String>) {
        *self.devices.lock() = present;
        *self.listening.lock() = listening;
    }
    pub fn devices(&self) -> Vec<String> {
        self.devices.lock().clone()
    }
    pub fn listening(&self) -> Vec<String> {
        self.listening.lock().clone()
    }

    /// Arm learn mode for `action`. Any earlier learn or uncollected capture
    /// is dropped: only the request the user just made is live.
    pub fn start_learn(&self, action: &str) {
        self.start_learn_within(action, LEARN_WINDOW);
    }

    /// `start_learn` with an explicit window, so the expiry rule is testable
    /// without waiting out the real one.
    fn start_learn_within(&self, action: &str, window: Duration) {
        *self.learn.lock() = Some((action.to_string(), Instant::now() + window));
        *self.captured.lock() = None;
    }
    pub fn cancel_learn(&self) {
        *self.learn.lock() = None;
        *self.captured.lock() = None;
    }
    /// The action being learned, or None once it is captured or expired.
    pub fn learning(&self) -> Option<String> {
        let mut learn = self.learn.lock();
        match learn.as_ref() {
            Some((action, deadline)) if Instant::now() < *deadline => Some(action.clone()),
            Some(_) => {
                *learn = None;
                None
            }
            None => None,
        }
    }
    /// Take a captured (action, signature) if one is waiting, still inside
    /// its learn window, and learned for one of `actions`. The delay
    /// actions' poll and the integrations' poll each take only their own,
    /// so neither can swallow the other's press. Consumed exactly once.
    pub fn take_captured_for(&self, actions: &[&str]) -> Option<(String, String)> {
        let mut captured = self.captured.lock();
        if !captured
            .as_ref()
            .is_some_and(|(action, _, _)| actions.contains(&action.as_str()))
        {
            return None;
        }
        match captured.take() {
            Some((action, signature, deadline)) if Instant::now() < deadline => {
                Some((action, signature))
            }
            _ => None,
        }
    }

    /// A press from a device opened only for integrations (not the one
    /// picked in Controls): it never runs a delay action, and only the
    /// Integrations tab's learn may capture it.
    #[cfg(windows)]
    pub fn on_integration_signature(&self, ctrl: &Controller, signature: &str) {
        {
            let mut learn = self.learn.lock();
            if let Some((action, deadline)) = learn.take() {
                if action == INTEGRATION_LEARN && Instant::now() < deadline {
                    *self.captured.lock() = Some((action, signature.to_string(), deadline));
                    return;
                }
                // Not ours: the Controls learn keeps waiting for its device.
                *learn = Some((action, deadline));
            }
        }
        if let Some(integrations) = ctrl.integrations() {
            integrations.shortcut(crate::integrations::Shortcut::Midi, signature);
        }
    }

    /// Called by the listener on each press-edge message. In learn mode it
    /// records the signature for the pending action; otherwise it routes to
    /// the bound action via the shared controller path. `default_ms` is the
    /// delay to arm with, read live by the caller from settings.
    #[cfg(windows)]
    pub fn on_signature(&self, ctrl: &Controller, default_ms: u32, signature: &str) {
        // Learn mode wins: capture and stop, do not also fire an action.
        // An expired request is not learn mode any more - the press belongs
        // to whatever the control is actually bound to.
        {
            let mut learn = self.learn.lock();
            if let Some((action, deadline)) = learn.take() {
                if Instant::now() < deadline {
                    *self.captured.lock() = Some((action, signature.to_string(), deadline));
                    return;
                }
            }
        }
        let action = self.bindings.lock().action_for(signature);
        if let Some(action) = action {
            // Same as the keyboard path: a refusal reaches the user as a
            // tray balloon, since a pad press gives no feedback of its own.
            if let Some(problem) = ctrl.run_named_action(action, default_ms, "midi") {
                crate::tray::notify_problem(&problem);
            }
        } else if let Some(integrations) = ctrl.integrations() {
            // Not a delay action's pad: maybe an integration's shortcut.
            integrations.shortcut(crate::integrations::Shortcut::Midi, signature);
        }
    }

    /// Runtime snapshot for the dashboard's MIDI poll: whether a device is
    /// listening, the device names, which action (if any) is learning, and
    /// the current bindings (so a just-committed one shows without a full
    /// config refetch).
    /// The state the dashboard renders. `refused` says why a press that
    /// was just learned wasn't bound.
    pub fn to_json(&self, refused: Option<&str>) -> String {
        let learning = match self.learning() {
            Some(a) => json_string(&a),
            None => "null".to_string(),
        };
        let devices: Vec<String> = self.devices().iter().map(|d| json_string(d)).collect();
        let listening: Vec<String> = self.listening().iter().map(|d| json_string(d)).collect();
        let bindings = {
            let b = self.bindings.lock();
            format!(
                r#"{{"toggle":{t},"arm":{a},"activate":{ac},"cut":{c},"cut_after":{ca},"end_hold":{eh}}}"#,
                t = json_string(&b.toggle),
                a = json_string(&b.arm),
                ac = json_string(&b.activate),
                c = json_string(&b.cut),
                ca = json_string(&b.cut_after),
                eh = json_string(&b.end_hold),
            )
        };
        format!(
            r#"{{"available":{a},"connected":{c},"learning":{l},"devices":[{d}],"listening":[{li}],"device":{sel},"bindings":{b},"refused":{r}}}"#,
            a = self.available(),
            c = self.connected(),
            l = learning,
            d = devices.join(","),
            li = listening.join(","),
            sel = json_string(&self.selected_device()),
            b = bindings,
            r = refused.map_or("null".to_string(), json_string),
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

/// Whether a control-change message is a press: its value just crossed up
/// past the middle. A knob or fader sends a stream of values as it turns;
/// without this, one turn would be dozens of presses. `high` remembers the
/// controls (by device, status and number) currently up.
#[cfg(any(windows, test))]
fn cc_press_edge(
    high: &mut HashSet<(usize, u8, u8)>,
    device: usize,
    status: u8,
    cc: u8,
    value: u8,
) -> bool {
    let key = (device, status, cc);
    if value >= 64 {
        high.insert(key)
    } else {
        high.remove(&key);
        false
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

/// Whether the listener opens `device`: the one picked in Controls (every
/// one when none is), any device an integration's pad was learned on, and
/// every device while the Integrations tab learns a pad.
#[cfg_attr(not(windows), allow(dead_code))]
fn should_open(device: &str, selected: &str, pad_devices: &[&str], learning_pad: bool) -> bool {
    selected.is_empty() || device == selected || learning_pad || pad_devices.contains(&device)
}

/// Whether the listener should be holding MIDI inputs open.
///
/// Opening one claims it exclusively: for as long as we hold a device, no
/// DAW, no VST host and no other MIDI tool on the machine can open it. A
/// user who has bound nothing never asked us to take their controller away
/// from the software they actually use it with, so we hold nothing until
/// there is a binding to serve - or a learn waiting for the press that will
/// become one.
/// Built everywhere so its rule is tested on every CI target; only Windows
/// has a listener to act on it.
#[cfg_attr(not(windows), allow(dead_code))]
fn should_hold_devices(learning: bool, settings: &Settings) -> bool {
    learning
        || settings
            .midi
            .entries()
            .iter()
            .any(|(_, sig)| !sig.is_empty())
        || !crate::integrations::model::shortcuts(&settings.integrations)
            .1
            .is_empty()
}

#[cfg(windows)]
mod win {
    use super::{cc_press_edge, signature_for, MidiState};
    use crate::config::Settings;
    use crate::controller::Controller;
    use std::collections::HashSet;
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
        /// Open device handle -> its name, so a message can say which deck
        /// it came from. winmm hands the handle to every callback; without
        /// this map two controllers sending the same note are one control.
        by_handle: crate::sync::Mutex<Vec<(usize, String)>>,
        /// Control-changes currently up; see `cc_press_edge`.
        cc_high: crate::sync::Mutex<HashSet<(usize, u8, u8)>>,
    }

    pub fn run(ctrl: Arc<Controller>, state: Arc<MidiState>, settings: watch::Receiver<Settings>) {
        let ctx: &'static CallbackCtx = Box::leak(Box::new(CallbackCtx {
            ctrl,
            state,
            settings,
            by_handle: crate::sync::Mutex::new(Vec::new()),
            cc_high: crate::sync::Mutex::new(HashSet::new()),
        }));
        let instance = ctx as *const CallbackCtx as usize;

        let mut handles: Vec<HMIDIIN> = Vec::new();
        let mut last_present: Vec<String> = Vec::new();
        let mut last_targets: Vec<String> = Vec::new();
        let mut last_listening = false;
        loop {
            // winmm gives no hot-plug event, so a periodic sweep is the only
            // way to notice a controller arriving or leaving. Comparing the
            // NAMES rather than the count catches the swap that keeps the
            // count the same (one deck unplugged, another plugged in between
            // two ticks). Holding nothing while a device we want is present
            // means the open failed - another app had it exclusively - so
            // that retries every tick until it is handed back.
            let present = device_names();
            let targets = devices_to_open(ctx, &present);
            let listening = wants_to_listen(ctx);
            let wanted = listening && !targets.is_empty();
            if present != last_present
                || targets != last_targets
                || listening != last_listening
                || (handles.is_empty() && wanted)
            {
                close_all(&mut handles);
                let opened = if listening {
                    open_matching(&present, &targets, instance, &mut handles)
                } else {
                    Vec::new()
                };
                // `open_matching` fills both in step, so zipping is the map
                // from the handle a callback carries to the name a binding
                // was recorded against.
                *ctx.by_handle.lock() = handles
                    .iter()
                    .map(|h| *h as usize)
                    .zip(opened.iter().cloned())
                    .collect();
                // `present` is published either way: enumerating opens
                // nothing, so the dashboard can still offer a device to pick
                // while we are holding none of them.
                ctx.state.set_devices(present.clone(), opened);
                last_present = present;
                last_targets = targets;
                last_listening = listening;
            }
            sleep_until_worth_another_look(ctx, listening);
        }
    }

    /// Whether we should be holding MIDI inputs open at all.
    ///
    /// Opening one claims it exclusively: for as long as we hold a device,
    /// no DAW, no VST host and no other MIDI tool on the machine can open
    /// it. A user who has bound nothing never asked us to take their
    /// controller away from the software they actually use it with, so hold
    /// nothing until there is a binding to serve, or a learn waiting for the
    /// press that will become one.
    fn wants_to_listen(ctx: &CallbackCtx) -> bool {
        let learning = ctx.state.learning().is_some();
        // Scoped: the borrow parks the settings watch, and opening devices
        // below is slow enough to matter.
        let settings = ctx.settings.borrow();
        super::should_hold_devices(learning, &settings)
    }

    /// The present devices `should_open` picks.
    fn devices_to_open(ctx: &CallbackCtx, present: &[String]) -> Vec<String> {
        let selected = ctx.state.selected_device();
        let learning_pad = ctx.state.learning().as_deref() == Some(super::INTEGRATION_LEARN);
        let pads = {
            let settings = ctx.settings.borrow();
            crate::integrations::model::shortcuts(&settings.integrations).1
        };
        let pad_devices: Vec<&str> = pads
            .iter()
            .filter_map(|p| p.split_once('@'))
            .map(|(_, d)| d)
            .collect();
        present
            .iter()
            .filter(|d| super::should_open(d, &selected, &pad_devices, learning_pad))
            .cloned()
            .collect()
    }

    /// Idle between sweeps, but notice a learn starting sooner than the
    /// sweep would. A learn window is short and the press that ends it comes
    /// from a human finger, so waiting a full tick to open the device would
    /// swallow the first thing they hit.
    fn sleep_until_worth_another_look(ctx: &CallbackCtx, listening: bool) {
        const TICK: std::time::Duration = std::time::Duration::from_millis(250);
        for _ in 0..8 {
            std::thread::sleep(TICK);
            if wants_to_listen(ctx) != listening {
                return;
            }
        }
    }

    /// Every input device the system reports, in device order.
    fn device_names() -> Vec<String> {
        (0..unsafe { midiInGetNumDevs() })
            .map(device_name)
            .collect()
    }

    /// Open and start the `targets` among the present devices, returning
    /// the names actually opened. Devices that fail to open are skipped
    /// (another app may hold one exclusively) and retried on a later sweep.
    fn open_matching(
        present: &[String],
        targets: &[String],
        instance: usize,
        handles: &mut Vec<HMIDIIN>,
    ) -> Vec<String> {
        let mut names = Vec::new();
        for (dev, name) in present.iter().enumerate() {
            if !targets.contains(name) {
                continue;
            }
            let dev = dev as u32;
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
            names.push(name.clone());
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
        // Through the same door as the device recorded inside a signature.
        // The name the user picks in the dashboard, the name we match an
        // open device against, and the name a signature carries all have to
        // be the same string, or a device is selectable and never matches.
        crate::config::sanitize_device_name(&String::from_utf16_lossy(&name[..end]))
    }

    /// winmm callback. Runs on a system thread; keeps work minimal (parse +
    /// a couple of short mutex holds). Only MIM_DATA carries a channel-voice
    /// message; dwParam1 packs status / data1 / data2 in its low three bytes.
    unsafe extern "system" fn midi_in_proc(
        hmi: HMIDIIN,
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
        let ctx = &*(instance as *const CallbackCtx);
        if status & 0xF0 == 0xB0
            && !cc_press_edge(&mut ctx.cc_high.lock(), hmi as usize, status, data1, data2)
        {
            return;
        }
        let Some(sig) = signature_for(status, data1, data2) else {
            return;
        };
        // Name the device the press came from. A deck we somehow have no
        // name for still works as an any-device signature rather than
        // going silent.
        let device = ctx
            .by_handle
            .lock()
            .iter()
            .find(|(handle, _)| *handle == hmi as usize)
            .map(|(_, name)| name.clone());
        let sig = match &device {
            Some(name) => format!("{sig}@{name}"),
            None => sig,
        };
        // A device outside the Controls pick is open only for integrations,
        // so its presses never reach a delay action.
        let selected = ctx.state.selected_device();
        if !selected.is_empty() && device.is_some_and(|name| name != selected) {
            ctx.state.on_integration_signature(&ctx.ctrl, &sig);
            return;
        }
        // Read the default delay live so a mid-session change in the
        // dashboard is reflected without restarting the listener.
        let default_ms = ctx.settings.borrow().auto_arm_delay_ms;
        ctx.state.on_signature(&ctx.ctrl, default_ms, &sig);
    }
}

#[cfg(test)]
mod tests {
    use super::should_hold_devices;
    use super::signature_for;
    use std::collections::HashSet;
    // `MidiState` itself is cross-platform - only the listener that feeds it
    // is Windows-only - so the device-choice test below runs everywhere.
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

    #[test]
    fn devices_open_for_controls_and_for_integration_pads() {
        use super::should_open;
        assert!(
            should_open("Deck A", "", &[], false),
            "no pick: every device"
        );
        assert!(should_open("Deck A", "Deck A", &[], false));
        assert!(
            !should_open("Deck B", "Deck A", &[], false),
            "not picked, not used"
        );
        assert!(
            should_open("Deck B", "Deck A", &["Deck B"], false),
            "an integration's pad"
        );
        assert!(should_open("Deck B", "Deck A", &[], true), "learning a pad");
    }

    #[test]
    fn a_knob_turned_up_presses_once() {
        let mut high = HashSet::new();
        let presses = (40..=127u8)
            .filter(|v| super::cc_press_edge(&mut high, 1, 0xB0, 20, *v))
            .count();
        assert_eq!(presses, 1, "one turn up is one press");
        assert!(!super::cc_press_edge(&mut high, 1, 0xB0, 20, 0), "release");
        assert!(
            super::cc_press_edge(&mut high, 1, 0xB0, 20, 127),
            "a new press"
        );
        assert!(
            super::cc_press_edge(&mut high, 2, 0xB0, 20, 127),
            "another deck's control"
        );
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
        let ctrl = std::sync::Arc::new(crate::controller::Controller::new(ring, 0));
        (ctrl, path)
    }

    /// Full simulated MIDI note: the exact bytes winmm delivers for a pad
    /// press, fed through signature_for -> on_signature -> the controller,
    /// asserting the bound action actually fires. This is everything past
    /// the OS driver, i.e. the whole app-owned MIDI path.
    #[cfg(windows)]
    #[test]
    fn simulated_note_fires_the_bound_action() {
        let (ctrl, path) = test_controller();
        ctrl.mark_ingest_alive_for_test();
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

        // A note that is NOT mapped must do nothing. Every dispatched
        // action stamps `last_action` with a fresh seq, even one that
        // changes no state, so an unchanged seq proves nothing fired.
        let seq_before = ctrl.last_action().map(|a| a.seq);
        assert!(seq_before.is_some(), "the mapped note must have stamped");
        let other = signature_for(0x90, 40, 100).expect("a press edge");
        state.on_signature(&ctrl, 15_000, &other);
        assert_eq!(
            ctrl.last_action().map(|a| a.seq),
            seq_before,
            "an unmapped note must not fire any action"
        );

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
        // 1 s toggle can actually activate, mirroring a live OBS feed. The
        // publisher flag goes with it: tags without one is not a state that
        // happens outside a test, and activate refuses it.
        ctrl.mark_ingest_alive_for_test();
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

    /// The first binding on a fresh install has to be recordable.
    ///
    /// Devices are only held open once something is bound, so "is a device
    /// open" and "is a controller plugged in" are different questions, and
    /// gating learn on the first one deadlocks: no binding means no open
    /// device means learn is refused means no binding can ever be made.
    /// This is the end-to-end shape of that, which testing the two halves
    /// separately does not catch.
    #[test]
    fn a_fresh_install_can_still_record_its_first_binding() {
        let state = MidiState::new();
        let fresh = crate::config::Settings::defaults();

        // Fresh install: a controller is plugged in, nothing is bound yet.
        state.set_devices(vec!["Launchpad MK2".to_string()], Vec::new());
        assert!(
            !should_hold_devices(false, &fresh),
            "nothing bound: the controller stays free for other software"
        );
        assert!(!state.available(), "so nothing is open");
        assert!(
            state.connected(),
            "but a controller IS there, which is what learn needs to know"
        );

        // Arming a learn is what asks for the device.
        state.start_learn("arm");
        assert!(
            should_hold_devices(state.learning().is_some(), &fresh),
            "an armed learn opens the device so the press can be heard"
        );

        // A device the user selected but never plugged in is still refused.
        let picky = MidiState::new();
        let mut s = crate::config::Settings::defaults();
        s.midi_device = "Deck B".to_string();
        picky.update_from_settings(&s);
        picky.set_devices(vec!["Deck A".to_string()], Vec::new());
        assert!(
            !picky.connected(),
            "the chosen device is not plugged in, so there is nothing to learn from"
        );
        picky.set_devices(vec!["Deck A".into(), "Deck B".into()], Vec::new());
        assert!(picky.connected(), "plugging it in makes learn available");
    }

    /// A MIDI input is an exclusive device: while InstantClone holds one,
    /// the DAW or VST host the user actually bought it for cannot open it.
    /// Someone who has never bound a pad has not asked for that, so we hold
    /// nothing until there is a reason to.
    #[test]
    fn no_bindings_means_we_leave_every_midi_device_alone() {
        let mut s = crate::config::Settings::defaults();
        assert!(
            !should_hold_devices(false, &s),
            "nothing bound and not learning: hold nothing"
        );
        assert!(
            should_hold_devices(true, &s),
            "a learn needs a device open to hear the press"
        );

        s.midi.set("arm", "note:1:36");
        assert!(
            should_hold_devices(false, &s),
            "one binding is reason enough"
        );

        // Clearing the last binding hands the controller back.
        s.midi.set("arm", "");
        assert!(
            !should_hold_devices(false, &s),
            "the last binding going away releases the device"
        );
    }

    /// An integration started from a pad is a binding too.
    #[test]
    fn an_integration_pad_holds_the_device() {
        use crate::integrations::model::{Handler, Step, StepKind, Trigger};
        let mut s = crate::config::Settings::defaults();
        let mut i = crate::integrations::presets::build("delay_command", "p".into(), "").unwrap();
        i.handlers = vec![Handler {
            enabled: true,
            trigger: Trigger::Shortcut {
                hotkey: String::new(),
                midi: "note:1:40".into(),
            },
            steps: vec![Step::new(StepKind::Clip, &[])],
        }];
        s.integrations.push(i);
        assert!(should_hold_devices(false, &s));
        s.integrations[0].enabled = false;
        assert!(
            !should_hold_devices(false, &s),
            "a switched-off one doesn't"
        );
    }

    /// The Controls tab's poll and the Integrations tab's poll each take
    /// only their own capture.
    #[test]
    fn each_learn_is_collected_by_its_own_poll() {
        let state = MidiState::new();
        *state.captured.lock() = Some((
            "integration".into(),
            "note:1:36@Pad".into(),
            std::time::Instant::now() + super::LEARN_WINDOW,
        ));
        assert_eq!(state.take_captured_for(&crate::config::ACTIONS), None);
        assert_eq!(
            state.take_captured_for(&["integration"]),
            Some(("integration".into(), "note:1:36@Pad".into()))
        );
    }

    /// The whole point of naming the device: two decks that both send pad 1
    /// as note 36 on channel 1 stay two decks, and can drive different
    /// actions. This is the matching half - `canonicalize_midi` covers the
    /// parsing, and the listener appends the name in the callback.
    #[cfg(windows)]
    #[test]
    fn two_devices_sending_the_same_note_drive_different_actions() {
        let (ctrl, path) = test_controller();
        ctrl.mark_ingest_alive_for_test();
        let state = MidiState::new();
        let mut s = crate::config::Settings::defaults();
        s.midi.set("arm", "note:1:36@Deck A");
        s.midi.set("cut", "note:1:36@Deck B");
        state.update_from_settings(&s);

        // Pad 1 on deck A arms.
        let base = signature_for(0x90, 36, 100).expect("a press edge");
        state.on_signature(&ctrl, 7_000, &format!("{base}@Deck A"));
        assert_eq!(ctrl.armed_delay_ms(), 7_000, "deck A armed");

        // The identical pad on deck B cuts instead, and does not re-arm.
        state.on_signature(&ctrl, 7_000, &format!("{base}@Deck B"));
        assert_eq!(ctrl.target_delay_ms(), 0, "deck B cut to live");
        assert_eq!(
            ctrl.armed_delay_ms(),
            7_000,
            "and did not run deck A's arm, which would have disarmed"
        );

        // A deck nobody mapped does nothing at all.
        ctrl.arm_delay(0);
        state.on_signature(&ctrl, 7_000, &format!("{base}@Deck C"));
        assert_eq!(ctrl.armed_delay_ms(), 0, "an unmapped deck is ignored");

        let _ = std::fs::remove_file(&path);
    }

    /// Picking a device is only useful if the listener actually narrows to
    /// it, and the dashboard has to keep seeing every device so the choice
    /// can be changed back.
    #[test]
    fn device_choice_round_trips_through_settings() {
        let state = MidiState::new();
        assert_eq!(state.selected_device(), "", "every device by default");

        let mut s = crate::config::Settings::defaults();
        s.midi_device = "Launchpad MK2".to_string();
        state.update_from_settings(&s);
        assert_eq!(state.selected_device(), "Launchpad MK2");

        let json = state.to_json(None);
        assert!(json.contains(r#""device":"Launchpad MK2""#), "{json}");
        assert!(json.contains(r#""devices":[]"#), "{json}");
        assert!(json.contains(r#""listening":[]"#), "{json}");
    }

    /// A learn request nobody came back for must not sit armed: the press
    /// belongs to whatever the control is bound to, and must never be
    /// committed as a binding later.
    #[cfg(windows)]
    #[test]
    fn an_expired_learn_dispatches_the_press_instead_of_capturing_it() {
        let (ctrl, path) = test_controller();
        ctrl.mark_ingest_alive_for_test();
        let state = MidiState::new();
        let mut s = crate::config::Settings::defaults();
        s.midi.set("arm", "note:1:36");
        state.update_from_settings(&s);

        // Armed, then expired before anyone pressed anything.
        state.start_learn_within("toggle", std::time::Duration::from_millis(0));
        assert_eq!(state.learning(), None, "an expired learn is not learning");

        let sig = signature_for(0x90, 36, 100).expect("a press edge");
        state.on_signature(&ctrl, 15_000, &sig);

        assert_eq!(
            ctrl.armed_delay_ms(),
            15_000,
            "the press must run its bound action"
        );
        assert_eq!(
            state.take_captured_for(&crate::config::ACTIONS),
            None,
            "and must not be captured"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// Cancelling drops an uncollected capture too, so reopening the
    /// dashboard cannot commit a binding the user walked away from.
    #[cfg(windows)]
    #[test]
    fn cancel_learn_discards_a_capture_nobody_collected() {
        let (ctrl, path) = test_controller();
        let state = MidiState::new();

        state.start_learn("cut");
        let sig = signature_for(0x90, 36, 100).expect("a press edge");
        state.on_signature(&ctrl, 15_000, &sig);
        state.cancel_learn();

        assert_eq!(state.take_captured_for(&crate::config::ACTIONS), None);

        let _ = std::fs::remove_file(&path);
    }

    /// In learn mode the same simulated note is captured for the pending
    /// action and does NOT fire anything.
    #[cfg(windows)]
    #[test]
    fn learn_captures_the_note_without_firing() {
        let (ctrl, path) = test_controller();
        // A live publisher and a control already bound to "arm", so the
        // press WOULD visibly arm the delay if learn mode let it through.
        // Without both, "nothing fired" would hold even with learn broken.
        ctrl.mark_ingest_alive_for_test();
        let state = MidiState::new();
        let mut s = crate::config::Settings::defaults();
        s.midi.set("arm", "cc:1:20");
        state.update_from_settings(&s);

        state.start_learn("arm");
        let sig = signature_for(0xB0, 20, 127).expect("cc press");
        state.on_signature(&ctrl, 15_000, &sig);

        assert_eq!(
            state.take_captured_for(&crate::config::ACTIONS),
            Some(("arm".to_string(), "cc:1:20".to_string())),
            "learn must capture the pressed control"
        );
        assert_eq!(state.learning(), None, "learn clears after one capture");
        assert_eq!(
            ctrl.armed_delay_ms(),
            0,
            "capturing must not also fire the action"
        );
        assert!(ctrl.last_action().is_none(), "no action may be stamped");

        let _ = std::fs::remove_file(&path);
    }
}
