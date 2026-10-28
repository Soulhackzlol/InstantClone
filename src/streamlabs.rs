//! Streamlabs Desktop (experimental): its Twitch streams go through
//! InstantClone with nothing to set up inside Streamlabs.
//!
//! Streamlabs is a fork of OBS that never reads OBS's service list, so the
//! "InstantClone" service can't reach it. Two levers in Streamlabs' own code
//! do instead, both proven live on Streamlabs 1.21.9:
//!
//!   * Its Twitch service downloads the Enhanced Broadcasting config from the
//!     `multitrack_video_configuration_url` in the services list Streamlabs
//!     keeps under `%APPDATA%\slobs-client\plugin_config\rtmp-services`, read
//!     again at every go-live, and streams wherever that config says. While
//!     connected, the Twitch entry points at InstantClone's config proxy, so an
//!     Enhanced Broadcasting stream lands here with every track and the VOD
//!     audio, and Streamlabs keeps its Twitch title, chat and alerts.
//!   * Its local control pipe (`\\.\pipe\slobs`, on by default, no login for
//!     local clients) reports each step of the Go Live flow. A stream with
//!     Enhanced Broadcasting off would go straight to Twitch, so when the
//!     Twitch step starts we switch Enhanced Broadcasting on and ask Twitch
//!     for one video track: the stream the streamer chose, through us. The
//!     choice is read about a second before the output starts, which leaves
//!     ample time for one pipe call.
//!
//! The Twitch entry stays on our URL until the streamer disconnects, also
//! while InstantClone is closed: Streamlabs then can't start an Enhanced
//! Broadcasting stream, so a stream never skips the delay unnoticed.
//! Disconnecting puts Twitch's own URL back. Only the Twitch entry is
//! touched, and only to or from our URL.
//!
//! The URL names this copy's dashboard port, which is how copies kept for
//! different channels tell their Streamlabs apart: a copy only ever undoes
//! its own URL, and takes another's only on Connect, from a closed copy.

use crate::config::Settings;
use crate::controller::Controller;
use crate::json::{self, Value};
use crate::sync::Mutex;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

/// Twitch's own Enhanced Broadcasting config endpoint, put back on restore.
const TWITCH_CONFIG_URL: &str = "https://ingest.twitch.tv/api/v3/GetClientConfiguration";
/// Marks a config URL as ours, whichever port it names.
const OUR_URL_MARK: &str = "/obs/multitrack-config?from=streamlabs";
/// How long to wait before looking for Streamlabs again.
const RETRY: Duration = Duration::from_secs(5);
/// How long Streamlabs may take to answer one request.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long after Streamlabs reports live its stream must have reached us
/// before the dashboard says it went around InstantClone.
const ARRIVAL_GRACE: Duration = Duration::from_secs(10);

/// What the dashboard shows about Streamlabs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// `off`, `not_installed`, `waiting` (not running), `connected` or `error`.
    pub state: &'static str,
    /// What stops it from working, for the streamer. Empty when nothing does.
    pub problem: String,
    /// The platform Streamlabs is signed in with (`twitch`, `youtube`, ...),
    /// empty when signed out.
    pub platform: String,
    /// Streamlabs is streaming right now.
    pub live: bool,
    /// How the current stream reaches us: `eb` (Enhanced Broadcasting),
    /// `single` (Enhanced Broadcasting switched on for one track), `around`
    /// (it never arrived) or empty.
    pub route: &'static str,
}

static STATUS: Mutex<Status> = Mutex::new(Status {
    state: "off",
    problem: String::new(),
    platform: String::new(),
    live: false,
    route: "",
});

/// Set when we switched Enhanced Broadcasting on for a stream the streamer
/// wanted without it; the config proxy then asks Twitch for one track.
static ONE_TRACK: AtomicBool = AtomicBool::new(false);

/// Requests from the dashboard to the task that owns the pipe.
static REQUESTS: OnceLock<mpsc::Sender<Request>> = OnceLock::new();

enum Request {
    TwitchKey(oneshot::Sender<Result<String, String>>),
}

pub fn status() -> Status {
    STATUS.lock().clone()
}

fn set_status(next: Status) {
    *STATUS.lock() = next;
}

fn update_status(change: impl FnOnce(&mut Status)) {
    change(&mut STATUS.lock());
}

/// The dashboard's view: `GET /streamlabs/status`. `other_copy` names the
/// other InstantClone Streamlabs streams through, if one does, so the
/// dashboard says so before Connect is pressed.
pub fn status_json(enabled: bool, web_port: u16) -> String {
    let s = status();
    let other_copy = services_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| other_copy_port(&text, &[web_port]))
        .map_or(Value::Null, |port| {
            json::obj([
                ("port", Value::Num(f64::from(port))),
                (
                    "running",
                    Value::Bool(crate::portcheck::port_held_by_other_instance(port)),
                ),
            ])
        });
    json::obj([
        ("supported", Value::Bool(cfg!(windows))),
        ("installed", Value::Bool(is_installed())),
        ("enabled", Value::Bool(enabled)),
        ("state", json::str(s.state)),
        ("problem", json::str(s.problem)),
        ("platform", json::str(s.platform)),
        ("live", Value::Bool(s.live)),
        ("route", json::str(s.route)),
        ("other_copy", other_copy),
    ])
    .to_json()
}

/// Whether the config proxy should ask Twitch for one video track. Read
/// once per stream: the flag clears as it is taken.
pub fn take_one_track_request() -> bool {
    ONE_TRACK.swap(false, Ordering::Relaxed)
}

/// The streamer's Twitch stream key, as Streamlabs (signed in with Twitch)
/// fetches it from Twitch itself.
pub async fn twitch_stream_key() -> Result<String, String> {
    let requests = REQUESTS
        .get()
        .ok_or_else(|| "Streamlabs isn't connected.".to_string())?;
    let (reply, answer) = oneshot::channel();
    requests
        .send(Request::TwitchKey(reply))
        .await
        .map_err(|_| "Streamlabs isn't connected.".to_string())?;
    answer
        .await
        .map_err(|_| "Streamlabs closed before answering.".to_string())?
}

// ── The services list ───────────────────────────────────────────────

/// Streamlabs' settings folder. Present once it has run on this machine.
fn streamlabs_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("slobs-client"))
}

fn is_installed() -> bool {
    streamlabs_dir().is_some_and(|dir| dir.is_dir())
}

fn services_path() -> Option<PathBuf> {
    streamlabs_dir().map(|dir| dir.join(r"plugin_config\rtmp-services\services.json"))
}

fn our_config_url(web_port: u16) -> String {
    format!("http://127.0.0.1:{web_port}{OUR_URL_MARK}")
}

/// `services_json` with the Twitch entry's config URL set to `url`, or None
/// when it already is (or there is no Twitch entry). Every other entry and
/// field is left as it was.
pub fn point_twitch_at(services_json: &str, url: &str) -> Result<Option<String>, String> {
    rewrite_twitch_url(services_json, |current| {
        (current != url).then(|| url.to_string())
    })
}

/// `services_json` with the Twitch entry back on Twitch's own config URL,
/// or None when the entry isn't on one of `our_ports` (left alone: another
/// InstantClone, or whatever else set it, wins).
pub fn restore_twitch(services_json: &str, our_ports: &[u16]) -> Result<Option<String>, String> {
    rewrite_twitch_url(services_json, |current| {
        url_port(current)
            .is_some_and(|port| our_ports.contains(&port))
            .then(|| TWITCH_CONFIG_URL.to_string())
    })
}

/// The dashboard port in a config URL an InstantClone wrote, whichever copy.
fn url_port(url: &str) -> Option<u16> {
    url.strip_prefix("http://127.0.0.1:")?
        .strip_suffix(OUR_URL_MARK)?
        .parse()
        .ok()
}

/// The dashboard port of another InstantClone copy that Streamlabs' Twitch
/// entry points at. Streamlabs is one per PC, so it streams through one
/// copy: a streamer with a copy per channel must not see them swap.
fn other_copy_port(services_json: &str, our_ports: &[u16]) -> Option<u16> {
    let root = json::parse(services_json).ok()?;
    let twitch = root
        .get("services")
        .and_then(Value::as_array)?
        .iter()
        .find(|service| service.str_or("name", "") == "Twitch")?;
    url_port(twitch.str_or("multitrack_video_configuration_url", ""))
        .filter(|port| !our_ports.contains(port))
}

/// What to tell the streamer when Streamlabs belongs to another copy.
fn owned_by_other_copy(port: u16, running: bool) -> String {
    if running {
        format!(
            "Streamlabs streams through the other InstantClone (dashboard port {port}). \
             Disconnect it there to use Streamlabs here."
        )
    } else {
        format!(
            "Streamlabs is set to stream through another InstantClone (dashboard port {port}), \
             which is closed. Disconnect, then Connect here to use Streamlabs with this one."
        )
    }
}

fn rewrite_twitch_url(
    services_json: &str,
    decide: impl Fn(&str) -> Option<String>,
) -> Result<Option<String>, String> {
    let mut root = json::parse(services_json)?;
    let Some(Value::Arr(services)) = field_mut(&mut root, "services") else {
        return Err("the services list has no services".into());
    };
    let Some(twitch) = services
        .iter_mut()
        .find(|service| service.str_or("name", "") == "Twitch")
    else {
        return Ok(None);
    };
    let current = twitch.str_or("multitrack_video_configuration_url", "");
    let Some(next) = decide(current) else {
        return Ok(None);
    };
    match field_mut(twitch, "multitrack_video_configuration_url") {
        Some(value) => *value = Value::Str(next),
        None => return Ok(None),
    }
    Ok(Some(root.to_json()))
}

fn field_mut<'a>(value: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    match value {
        Value::Obj(fields) => fields.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

/// Point Streamlabs' Twitch entry at us (`our_ports[0]`; the others are
/// ports this copy used before). Streamlabs reads the file at every go-live,
/// so this takes effect without restarting it. Its own updater replaces the
/// file only when a newer list ships, which a later call puts right again.
///
/// An entry on another copy's port is never taken in the background: only
/// `claim`, the streamer pressing Connect, takes it, and only from a copy
/// that is closed.
fn ensure_pointed_at_us(our_ports: &[u16], take_from_closed_copy: bool) -> Result<(), String> {
    let path = services_path().ok_or("no APPDATA folder")?;
    let text = std::fs::read_to_string(&path).map_err(|_| {
        "Open Streamlabs once so it creates its settings, then try again.".to_string()
    })?;
    if let Some(port) = other_copy_port(&text, our_ports) {
        let running = crate::portcheck::port_held_by_other_instance(port);
        if running || !take_from_closed_copy {
            return Err(owned_by_other_copy(port, running));
        }
    }
    if let Some(next) = point_twitch_at(&text, &our_config_url(our_ports[0]))? {
        std::fs::write(&path, next).map_err(|e| format!("couldn't update Streamlabs: {e}"))?;
    }
    Ok(())
}

/// The streamer pressed Connect: point Streamlabs at this copy now, unless
/// another copy that is open streams through it.
pub fn claim(web_port: u16) -> Result<(), String> {
    if !is_installed() {
        return Ok(());
    }
    ensure_pointed_at_us(&[web_port], true)
}

/// Put Streamlabs' Twitch entry back on Twitch's URL if it points at this
/// copy (on any of `our_ports`). Called on disconnect.
pub fn restore_services(our_ports: &[u16]) {
    let Some(path) = services_path() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    match restore_twitch(&text, our_ports) {
        Ok(Some(original)) => {
            if let Err(e) = std::fs::write(&path, original) {
                log_line!("could not give Streamlabs its Twitch settings back: {e}");
            }
        }
        Ok(None) => {}
        Err(e) => log_line!("could not read Streamlabs' services list: {e}"),
    }
}

/// `body` (an Enhanced Broadcasting config request) asking Twitch for one
/// video track, or None when it has no preferences to set it in.
pub fn ask_for_one_track(body: &str) -> Option<String> {
    let mut request = json::parse(body).ok()?;
    let preferences = field_mut(&mut request, "preferences")?;
    match field_mut(preferences, "maximum_video_tracks") {
        Some(value) => *value = Value::Num(1.0),
        None => match preferences {
            Value::Obj(fields) => fields.push(("maximum_video_tracks".into(), Value::Num(1.0))),
            _ => return None,
        },
    }
    Some(request.to_json())
}

// ── The Go Live flow ────────────────────────────────────────────────

/// What one `streamInfoChanged` event says about the Go Live flow.
#[derive(Debug, Default, PartialEq)]
struct GoLiveStep {
    /// `empty`, `prepopulate`, `waitForNewSettings`, `runChecklist` or `live`.
    lifecycle: String,
    /// The Twitch checklist item: `not-started`, `pending`, `done`, `failed`.
    twitch_step: String,
    twitch_enabled: bool,
    /// Whether this go-live will use Enhanced Broadcasting.
    enhanced_broadcasting: bool,
}

impl GoLiveStep {
    fn from_event(data: &Value) -> GoLiveStep {
        let state = data.get("state").unwrap_or(data);
        let info = state.get("info");
        let text = |path: &str| {
            info.and_then(|i| i.path(path))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        GoLiveStep {
            lifecycle: text("lifecycle"),
            twitch_step: text("checklist.twitch"),
            twitch_enabled: info
                .and_then(|i| i.path("settings.platforms.twitch.enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            enhanced_broadcasting: state.bool_or("enhancedBroadcasting", false),
        }
    }

    /// A Twitch stream about to start without Enhanced Broadcasting, which
    /// would go straight to Twitch: the moment to switch it on.
    fn needs_enhanced_broadcasting(&self) -> bool {
        self.lifecycle == "runChecklist"
            && self.twitch_step == "pending"
            && self.twitch_enabled
            && !self.enhanced_broadcasting
    }
}

// ── The pipe ────────────────────────────────────────────────────────

/// Newline-delimited JSON-RPC over Streamlabs' control pipe. Events that
/// arrive while a call waits for its answer are kept for `next_event`.
struct Rpc<R, W> {
    lines: Lines<BufReader<R>>,
    writer: W,
    next_id: u64,
    queued: VecDeque<Value>,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> Rpc<R, W> {
    fn new(reader: R, writer: W) -> Self {
        Rpc {
            lines: BufReader::new(reader).lines(),
            writer,
            next_id: 0,
            queued: VecDeque::new(),
        }
    }

    /// Call `method` on one of Streamlabs' services. An async method first
    /// answers with a promise; its value follows as an event.
    async fn call(&mut self, resource: &str, method: &str, args: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json::obj([
            ("jsonrpc", json::str("2.0")),
            ("id", Value::Num(id as f64)),
            ("method", json::str(method)),
            (
                "params",
                json::obj([("resource", json::str(resource)), ("args", args)]),
            ),
        ]);
        let mut line = request.to_json();
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("couldn't reach Streamlabs: {e}"))?;
        let answer = tokio::time::timeout(CALL_TIMEOUT, self.answer_to(id))
            .await
            .map_err(|_| format!("Streamlabs didn't answer {resource}.{method}"))??;
        if answer.str_or("_type", "") != "SUBSCRIPTION" || answer.str_or("emitter", "") != "PROMISE"
        {
            return Ok(answer);
        }
        let promise = answer.str_or("resourceId", "").to_string();
        let settled = tokio::time::timeout(CALL_TIMEOUT, self.event_for(&promise))
            .await
            .map_err(|_| format!("Streamlabs didn't finish {resource}.{method}"))??;
        let data = settled.get("data").cloned().unwrap_or(Value::Null);
        if settled.bool_or("isRejected", false) {
            return Err(format!(
                "Streamlabs refused: {}",
                data.str_or("message", "no reason given")
            ));
        }
        Ok(data)
    }

    async fn read_message(&mut self) -> Result<Value, String> {
        let line = self
            .lines
            .next_line()
            .await
            .map_err(|e| format!("lost Streamlabs: {e}"))?
            .ok_or_else(|| "Streamlabs closed".to_string())?;
        json::parse(&line)
    }

    async fn answer_to(&mut self, id: u64) -> Result<Value, String> {
        loop {
            let message = self.read_message().await?;
            if message.get("id").and_then(Value::as_f64) != Some(id as f64) {
                self.keep_event(message);
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(error.str_or("message", "Streamlabs refused").to_string());
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    async fn event_for(&mut self, resource_id: &str) -> Result<Value, String> {
        let matches = |event: &Value| event.str_or("resourceId", "") == resource_id;
        if let Some(at) = self.queued.iter().position(matches) {
            return Ok(self.queued.remove(at).unwrap_or(Value::Null));
        }
        loop {
            let message = self.read_message().await?;
            match message.get("result") {
                Some(event) if matches(event) => return Ok(event.clone()),
                _ => self.keep_event(message),
            }
        }
    }

    fn keep_event(&mut self, message: Value) {
        if let Some(event) = message
            .get("result")
            .filter(|r| r.str_or("_type", "") == "EVENT")
        {
            self.queued.push_back(event.clone());
        }
    }

    /// The next event: its `resourceId` names what fired, `data` what it
    /// says. Cancel-safe, so it can sit in a `select!`.
    async fn next_event(&mut self) -> Result<Value, String> {
        if let Some(event) = self.queued.pop_front() {
            return Ok(event);
        }
        loop {
            let message = self.read_message().await?;
            if let Some(event) = message
                .get("result")
                .filter(|r| r.str_or("_type", "") == "EVENT")
            {
                return Ok(event.clone());
            }
        }
    }
}

#[cfg(windows)]
fn open_pipe() -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    tokio::net::windows::named_pipe::ClientOptions::new().open(r"\\.\pipe\slobs")
}

/// Streamlabs Desktop is Windows-only, so elsewhere it is never running.
#[cfg(not(windows))]
fn open_pipe() -> std::io::Result<tokio::io::DuplexStream> {
    Err(std::io::ErrorKind::Unsupported.into())
}

// ── The task ────────────────────────────────────────────────────────

/// Keeps Streamlabs connected while the setting is on: points its Twitch
/// entry at us, watches its Go Live flow, and answers dashboard requests.
pub async fn run(ctrl: Arc<Controller>, mut settings: watch::Receiver<Settings>) {
    let (sender, mut requests) = mpsc::channel(4);
    let _ = REQUESTS.set(sender);
    // The port Streamlabs was pointed at before a dashboard port change: still
    // this copy's entry, to move along or put back.
    let mut earlier_port: Option<u16> = None;
    loop {
        let (wanted, web_port) = {
            let s = settings.borrow_and_update();
            (s.streamlabs_connect, s.web_port)
        };
        let our_ports: Vec<u16> = std::iter::once(web_port).chain(earlier_port).collect();
        if !wanted {
            restore_services(&our_ports);
            earlier_port = None;
            set_status(Status {
                state: "off",
                ..Status::default()
            });
            if !idle_until_changed(&mut settings, &mut requests, None).await {
                return;
            }
            continue;
        }
        if !is_installed() {
            set_status(Status {
                state: "not_installed",
                ..Status::default()
            });
        } else if let Err(problem) = ensure_pointed_at_us(&our_ports, false) {
            set_status(Status {
                state: "error",
                problem,
                ..Status::default()
            });
        } else {
            earlier_port = Some(web_port);
            connect_once(&ctrl, &mut settings, &mut requests, web_port).await;
        }
        if !idle_until_changed(&mut settings, &mut requests, Some(RETRY)).await {
            return;
        }
    }
}

/// One attempt at the pipe: a session while Streamlabs runs, else `waiting`.
async fn connect_once(
    ctrl: &Arc<Controller>,
    settings: &mut watch::Receiver<Settings>,
    requests: &mut mpsc::Receiver<Request>,
    web_port: u16,
) {
    let Ok(pipe) = open_pipe() else {
        set_status(Status {
            state: "waiting",
            ..Status::default()
        });
        return;
    };
    let (reader, writer) = tokio::io::split(pipe);
    let mut rpc = Rpc::new(reader, writer);
    let ended = session(&mut rpc, ctrl, settings, requests, web_port).await;
    ONE_TRACK.store(false, Ordering::Relaxed);
    if let Err(reason) = ended {
        ctrl.log(format!("[Streamlabs] disconnected: {reason}"));
    }
    set_status(Status {
        state: "waiting",
        ..Status::default()
    });
}

/// Wait for a settings change (or `timeout`), turning dashboard requests
/// away meanwhile. False once the settings channel is gone (shutdown).
async fn idle_until_changed(
    settings: &mut watch::Receiver<Settings>,
    requests: &mut mpsc::Receiver<Request>,
    timeout: Option<Duration>,
) -> bool {
    let deadline = timeout.map(|t| Instant::now() + t);
    loop {
        tokio::select! {
            changed = settings.changed() => return changed.is_ok(),
            _ = sleep_until(deadline) => return true,
            Some(request) = requests.recv() => refuse(request, "Open Streamlabs first."),
        }
    }
}

fn refuse(request: Request, why: &str) {
    match request {
        Request::TwitchKey(reply) => {
            let _ = reply.send(Err(why.to_string()));
        }
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Streamlabs is running: watch it until it closes or the setting changes.
async fn session<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    rpc: &mut Rpc<R, W>,
    ctrl: &Arc<Controller>,
    settings: &mut watch::Receiver<Settings>,
    requests: &mut mpsc::Receiver<Request>,
    web_port: u16,
) -> Result<(), String> {
    rpc.call("StreamingService", "streamInfoChanged", Value::Arr(vec![]))
        .await?;
    rpc.call(
        "StreamingService",
        "streamingStatusChange",
        Value::Arr(vec![]),
    )
    .await?;
    refresh_account(rpc).await?;
    ctrl.log("[Streamlabs] connected");
    let mut arrival_check: Option<Instant> = None;
    loop {
        tokio::select! {
            event = rpc.next_event() => {
                let event = event?;
                on_event(rpc, ctrl, &event, web_port, &mut arrival_check).await?;
            }
            Some(request) = requests.recv() => answer(rpc, request).await,
            changed = settings.changed() => {
                let s = settings.borrow();
                if changed.is_err() || !s.streamlabs_connect || s.web_port != web_port {
                    return Ok(());
                }
            }
            _ = sleep_until(arrival_check) => {
                arrival_check = None;
                check_arrival(ctrl);
            }
        }
    }
}

/// Who Streamlabs is signed in as. Only a Twitch account streams through
/// the Twitch service this relies on.
async fn refresh_account<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    rpc: &mut Rpc<R, W>,
) -> Result<(), String> {
    let signed_in = rpc
        .call("UserService", "isLoggedIn", Value::Arr(vec![]))
        .await?
        .as_bool()
        .unwrap_or(false);
    let platform = if signed_in {
        rpc.call("UserService", "platformType", Value::Arr(vec![]))
            .await?
            .as_str()
            .unwrap_or("")
            .to_string()
    } else {
        String::new()
    };
    let problem = match platform.as_str() {
        "twitch" => String::new(),
        "" => "Sign Streamlabs in with Twitch: that's how its stream reaches InstantClone."
            .to_string(),
        other => format!(
            "Streamlabs is signed in with {other}. Sign it in with Twitch instead: that's how its \
             stream reaches InstantClone, which can still send it to {other}. Or use Settings > \
             Stream > Stream to custom ingest."
        ),
    };
    update_status(|s| {
        s.state = "connected";
        s.platform = platform;
        s.problem = problem;
    });
    Ok(())
}

async fn on_event<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    rpc: &mut Rpc<R, W>,
    ctrl: &Arc<Controller>,
    event: &Value,
    web_port: u16,
    arrival_check: &mut Option<Instant>,
) -> Result<(), String> {
    let data = event.get("data").unwrap_or(&Value::Null);
    match event.str_or("resourceId", "") {
        "StreamingService.streamInfoChanged" => {
            let step = GoLiveStep::from_event(data);
            if step.lifecycle == "prepopulate" {
                // The Go Live window is opening: the account may have
                // changed, and Streamlabs' updater may have replaced the list.
                ONE_TRACK.store(false, Ordering::Relaxed);
                update_status(|s| s.route = "");
                refresh_account(rpc).await?;
                if let Err(problem) = ensure_pointed_at_us(&[web_port], false) {
                    ctrl.log(format!("[Streamlabs] {problem}"));
                }
            }
            if step.needs_enhanced_broadcasting() {
                // Before the call: the config request follows within a second.
                ONE_TRACK.store(true, Ordering::Relaxed);
                update_status(|s| s.route = "single");
                rpc.call(
                    "StreamingService",
                    "SET_ENHANCED_BROADCASTING",
                    Value::Arr(vec![Value::Bool(true)]),
                )
                .await?;
                ctrl.log(
                    "[Streamlabs] Enhanced Broadcasting is off for this stream: sending it \
                     through InstantClone as one track",
                );
            } else if step.lifecycle == "runChecklist" && step.enhanced_broadcasting {
                // Only when nothing else claimed the stream: once switched on
                // above, the next events report Enhanced Broadcasting too.
                update_status(|s| {
                    if s.route.is_empty() {
                        s.route = "eb";
                    }
                });
            }
        }
        "StreamingService.streamingStatusChange" => match data.as_str() {
            Some("live") => {
                update_status(|s| s.live = true);
                *arrival_check = Some(Instant::now() + ARRIVAL_GRACE);
            }
            Some("offline") => {
                ONE_TRACK.store(false, Ordering::Relaxed);
                update_status(|s| {
                    s.live = false;
                    s.route = "";
                });
                *arrival_check = None;
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

/// Streamlabs went live a while ago: say so if nothing reached us, since
/// the delay can't hold back a stream that went straight to the platform.
fn check_arrival(ctrl: &Controller) {
    if ctrl.ingest_alive() || !status().live {
        return;
    }
    update_status(|s| s.route = "around");
    ctrl.log(
        "[Streamlabs] it's live, but its stream isn't reaching InstantClone, so the delay \
         doesn't apply. Go live to Twitch from Streamlabs' Go Live window, without \
         Streamlabs' own Multistream.",
    );
}

async fn answer<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    rpc: &mut Rpc<R, W>,
    request: Request,
) {
    match request {
        Request::TwitchKey(reply) => {
            let key = rpc
                .call("TwitchService", "fetchStreamKey", Value::Arr(vec![]))
                .await
                .and_then(|key| {
                    key.as_str()
                        .filter(|k| !k.is_empty())
                        .map(str::to_string)
                        .ok_or_else(|| "Streamlabs has no Twitch key: sign in with Twitch.".into())
                });
            let _ = reply.send(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVICES: &str = r#"{"format_version":5,"services":[
        {"name":"Twitch","common":true,"multitrack_video_configuration_url":"https://ingest.twitch.tv/api/v3/GetClientConfiguration","servers":[{"name":"Default","url":"auto"}]},
        {"name":"Amazon IVS","multitrack_video_configuration_url":"https://ingest.contribute.live-video.net/api/v3/GetClientConfiguration"}
    ]}"#;

    fn twitch_url(services_json: &str) -> String {
        let root = json::parse(services_json).unwrap();
        root.get("services").and_then(Value::as_array).unwrap()[0]
            .str_or("multitrack_video_configuration_url", "")
            .to_string()
    }

    #[test]
    fn only_the_twitch_entry_is_pointed_at_us() {
        let ours = our_config_url(7799);
        let patched = point_twitch_at(SERVICES, &ours).unwrap().unwrap();
        assert_eq!(twitch_url(&patched), ours);
        assert!(patched.contains("https://ingest.contribute.live-video.net"));
        assert!(patched.contains(r#""format_version":5"#));
        assert_eq!(
            point_twitch_at(&patched, &ours).unwrap(),
            None,
            "already ours"
        );
    }

    #[test]
    fn restore_gives_twitch_its_url_back_only_from_ours() {
        let patched = point_twitch_at(SERVICES, &our_config_url(7800))
            .unwrap()
            .unwrap();
        let restored = restore_twitch(&patched, &[7800]).unwrap().unwrap();
        assert_eq!(twitch_url(&restored), TWITCH_CONFIG_URL);
        assert_eq!(
            restore_twitch(SERVICES, &[7800]).unwrap(),
            None,
            "not ours: left alone"
        );
        assert_eq!(
            restore_twitch(&patched, &[7799]).unwrap(),
            None,
            "another copy's: left alone"
        );
    }

    #[test]
    fn another_copys_entry_is_told_apart_by_its_port() {
        let theirs = point_twitch_at(SERVICES, &our_config_url(7800))
            .unwrap()
            .unwrap();
        assert_eq!(other_copy_port(&theirs, &[7799]), Some(7800));
        assert_eq!(
            other_copy_port(&theirs, &[7799, 7800]),
            None,
            "this copy's own, from before a port change"
        );
        assert_eq!(other_copy_port(SERVICES, &[7799]), None, "Twitch's own");
        assert_eq!(
            url_port("http://127.0.0.1:7799/obs/multitrack-config"),
            None
        );
    }

    #[test]
    fn a_list_without_twitch_is_left_alone() {
        let no_twitch = r#"{"format_version":5,"services":[{"name":"YouTube"}]}"#;
        assert_eq!(point_twitch_at(no_twitch, "http://x").unwrap(), None);
        assert!(point_twitch_at("{}", "http://x").is_err());
    }

    #[test]
    fn a_config_request_asks_for_one_track() {
        let body = r#"{"authentication":"k","preferences":{"vod_track_audio":true,"maximum_video_tracks":null}}"#;
        let one = json::parse(&ask_for_one_track(body).unwrap()).unwrap();
        assert_eq!(
            one.path("preferences.maximum_video_tracks")
                .and_then(Value::as_f64),
            Some(1.0)
        );
        assert_eq!(
            one.path("preferences.vod_track_audio")
                .and_then(Value::as_bool),
            Some(true)
        );

        let without = ask_for_one_track(r#"{"preferences":{}}"#).unwrap();
        assert!(without.contains(r#""maximum_video_tracks":1"#));
        assert_eq!(ask_for_one_track(r#"{"authentication":"k"}"#), None);
    }

    fn go_live_event(lifecycle: &str, twitch_step: &str, enhanced: bool) -> Value {
        json::parse(&format!(
            r#"{{"state":{{"enhancedBroadcasting":{enhanced},"info":{{"lifecycle":"{lifecycle}",
            "checklist":{{"twitch":"{twitch_step}"}},
            "settings":{{"platforms":{{"twitch":{{"enabled":true,"isEnhancedBroadcasting":false}}}}}}}}}}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn a_twitch_step_without_enhanced_broadcasting_gets_it() {
        let step = GoLiveStep::from_event(&go_live_event("runChecklist", "pending", false));
        assert_eq!(step.lifecycle, "runChecklist");
        assert!(step.twitch_enabled);
        assert!(step.needs_enhanced_broadcasting());
    }

    #[test]
    fn enhanced_broadcasting_already_on_or_another_step_is_left_alone() {
        let on = GoLiveStep::from_event(&go_live_event("runChecklist", "pending", true));
        assert!(!on.needs_enhanced_broadcasting());
        let window =
            GoLiveStep::from_event(&go_live_event("waitForNewSettings", "not-started", false));
        assert!(!window.needs_enhanced_broadcasting());
        let done = GoLiveStep::from_event(&go_live_event("runChecklist", "done", false));
        assert!(
            !done.needs_enhanced_broadcasting(),
            "too late once the output starts"
        );
    }

    /// The pipe's shape: a promise answer, then its value as an event, with
    /// an unrelated event in between that `next_event` still hands out.
    #[tokio::test]
    async fn an_async_call_waits_for_its_promise() {
        let (client, mut server) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let mut rpc = Rpc::new(reader, writer);
        let streamlabs = async {
            let mut request = vec![0u8; 512];
            let n = tokio::io::AsyncReadExt::read(&mut server, &mut request)
                .await
                .unwrap();
            assert!(String::from_utf8_lossy(&request[..n]).contains(r#""method":"fetchStreamKey""#));
            server
                .write_all(
                    concat!(
                        r#"{"id":1,"result":{"_type":"SUBSCRIPTION","emitter":"PROMISE","resourceId":"p1"}}"#, "\n",
                        r#"{"id":null,"result":{"_type":"EVENT","emitter":"STREAM","resourceId":"StreamingService.streamingStatusChange","data":"live"}}"#, "\n",
                        r#"{"id":null,"result":{"_type":"EVENT","emitter":"PROMISE","resourceId":"p1","data":"live_1_abc","isRejected":false}}"#, "\n",
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            server
        };
        let (key, _server) = tokio::join!(
            rpc.call("TwitchService", "fetchStreamKey", Value::Arr(vec![])),
            streamlabs
        );
        assert_eq!(key.unwrap().as_str(), Some("live_1_abc"));
        let event = rpc.next_event().await.unwrap();
        assert_eq!(
            event.str_or("resourceId", ""),
            "StreamingService.streamingStatusChange"
        );
    }
}
