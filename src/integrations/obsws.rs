//! The connection to OBS's own WebSocket server (obs-websocket 5, built
//! into OBS 28 and later). Scene triggers hear from it which scene is on
//! air and when it switches, and OBS event triggers when recording starts
//! or stops, a replay is saved or a source is muted. OBS steps ask it to
//! switch a scene, show or hide a source, change a text source, record,
//! save the replay buffer, mute, switch a filter, play media or press one
//! of OBS's hotkeys.
//!
//! Only scene, source, filter and output events are asked for, and OBS is
//! only asked to do what an OBS step says. The address, port and password
//! are read from OBS's own settings file, so the user only has to switch
//! the server on in OBS.
//!
//! The WebSocket client is `ws`, shared with Twitch EventSub. A reader
//! task owns the socket's read half, so waiting for a message can always be
//! interrupted (by a step's request, or the connection no longer being
//! wanted) without losing half a frame.

use super::event::{Event, EventKind};
use super::ws::{self, base64};
use crate::json::{self, Value};
use crate::sync::Mutex;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};

/// How long to wait before trying again after OBS refused or went away.
const RETRY: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a step's request may wait for OBS's answer.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// obs-websocket's event categories (`EventSubscription`): scenes, inputs
/// (sources added, removed, renamed or muted), filters (added, removed or
/// renamed) and outputs (recording, the replay buffer).
const SCENE_EVENTS: u32 = 1 << 2;
const INPUT_EVENTS: u32 = 1 << 3;
const FILTER_EVENTS: u32 = 1 << 5;
const OUTPUT_EVENTS: u32 = 1 << 6;
/// A filter list's request id is this and the source's name.
const FILTERS_ID: &str = "filters:";
/// OBS's own Screenshot hotkey: it saves to OBS's recording folder.
pub const SCREENSHOT_HOTKEY: &str = "OBSBasic.Screenshot";
const DEFAULT_PORT: u16 = 4455;

/// What the dashboard shows about the connection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// `off`, `connecting`, `connected` or `error`.
    pub state: &'static str,
    /// Why it isn't connected, for the user.
    pub problem: String,
    pub scenes: Vec<String>,
    /// Every source (input) in OBS, for OBS steps to pick from.
    pub sources: Vec<String>,
    /// Each source's and scene's filters, by its name.
    pub filters: BTreeMap<String, Vec<String>>,
    /// The hotkeys a step can press by name (see `pressable_hotkeys`).
    pub hotkeys: Vec<String>,
    pub current: String,
}

impl Status {
    pub fn to_json(&self) -> Value {
        json::obj([
            (
                "state",
                json::str(if self.state.is_empty() {
                    "off"
                } else {
                    self.state
                }),
            ),
            ("problem", json::str(&self.problem)),
            (
                "scenes",
                Value::Arr(self.scenes.iter().map(json::str).collect()),
            ),
            (
                "sources",
                Value::Arr(self.sources.iter().map(json::str).collect()),
            ),
            (
                "filters",
                Value::Obj(
                    self.filters
                        .iter()
                        .map(|(name, list)| {
                            (
                                name.clone(),
                                Value::Arr(list.iter().map(json::str).collect()),
                            )
                        })
                        .collect(),
                ),
            ),
            (
                "hotkeys",
                Value::Arr(self.hotkeys.iter().map(json::str).collect()),
            ),
            ("current", json::str(&self.current)),
        ])
    }
}

/// What OBS tells the engine.
#[derive(Debug)]
pub enum ObsNews {
    /// The scene on air when the connection opened: not a switch.
    Current(String),
    /// The program scene switched.
    Switched(String),
    /// Recording started or stopped, a replay was saved, a source muted.
    Happened(Event),
}

/// Something an OBS step asks OBS to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Put this scene on air.
    Scene(String),
    /// Show (`Some(true)`), hide (`Some(false)`) or flip (`None`) a source
    /// in a scene; a blank scene is the one on air.
    Source {
        scene: String,
        source: String,
        visible: Option<bool>,
    },
    /// Set what a text source says.
    Text { source: String, text: String },
    /// Start (`true`) or stop recording.
    Record(bool),
    /// Save what the replay buffer holds.
    SaveReplay,
    /// Mute (`Some(true)`), unmute (`Some(false)`) or flip (`None`) a
    /// source's audio.
    Mute { source: String, muted: Option<bool> },
    /// Switch a source's or scene's filter on, off, or flip it (`None`).
    Filter {
        source: String,
        filter: String,
        enabled: Option<bool>,
    },
    /// Play a media source from the start (`true`), or stop it.
    Media { source: String, play: bool },
    /// Press one of OBS's hotkeys by name, bound to keys or not.
    Hotkey(String),
    /// Show this page in the scene on air, as a browser source as big as
    /// the canvas: the dashboard's "Add to OBS" for the alerts page.
    BrowserSource { name: String, url: String },
    /// Read what OBS is doing (the scene on air, streaming, recording) and,
    /// for each source, whether it is muted and showing in `scene` (blank:
    /// the one on air). Changes nothing.
    Check { scene: String, sources: Vec<String> },
}

/// Longest key a checked source's values get, so `{obs2.key.visible}`
/// stays inside a variable name's 64 characters.
const MAX_SOURCE_KEY: usize = 24;

/// The names an OBS step lists in one field, one per line: one mute or
/// show can take several sources. Blank lines and repeats are dropped.
pub fn listed_names(field: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in field.lines().map(str::trim).filter(|name| !name.is_empty()) {
        if !names.iter().any(|known| known == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// What a check that reads several sources names each one's values by:
/// `{obs.mic_aux.muted}`. Lowercase letters, digits and `_`; a clash gets
/// a number. The editor makes the same keys (`sourceKeys`).
pub fn source_keys(sources: &[String]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for source in sources {
        let mut key = String::new();
        for c in source.chars().flat_map(char::to_lowercase) {
            if c.is_ascii_alphanumeric() {
                key.push(c);
            } else if !key.is_empty() && !key.ends_with('_') {
                key.push('_');
            }
        }
        let key: String = key.chars().take(MAX_SOURCE_KEY).collect();
        let base = match key.trim_end_matches('_') {
            "" => "source".to_string(),
            key => key.to_string(),
        };
        let mut key = base.clone();
        let mut n = 2;
        while keys.contains(&key) {
            key = format!("{base}{n}");
            n += 1;
        }
        keys.push(key);
    }
    keys
}

/// What OBS said, as (field, value): the step keeps each one under its own
/// answer name, like `{obs.muted}`. Empty for an action that only does
/// something.
pub type Answer = Vec<(String, String)>;

/// An action and where its outcome goes. The answer channel is a std one:
/// a step waits for it on a blocking thread.
pub struct Command {
    pub action: Action,
    pub reply: std::sync::mpsc::Sender<Result<Answer, String>>,
    /// After this the step has given up: doing it then would switch a
    /// scene nobody asked for any more.
    pub deadline: std::time::Instant,
}

/// Not `Debug`: it holds the WebSocket password.
struct Settings {
    port: u16,
    /// `None` when OBS's server doesn't ask for one.
    password: Option<String>,
}

/// One whole message from OBS, as the reader task hands it over.
enum Incoming {
    Json(Value),
    /// A ping, to answer with this payload.
    Ping(Vec<u8>),
}

/// Keep a connection open while `want` is true, telling `news` about
/// scenes, keeping `status` current and doing what `commands` ask. Runs
/// until the engine stops.
pub async fn run(
    mut want: watch::Receiver<bool>,
    status: Arc<Mutex<Status>>,
    news: mpsc::Sender<ObsNews>,
    mut commands: mpsc::Receiver<Command>,
) {
    loop {
        while !*want.borrow_and_update() {
            set_state(&status, "off", "");
            tokio::select! {
                changed = want.changed() => if changed.is_err() {
                    return;
                },
                Some(command) = commands.recv() => refuse(command, "OBS isn't connected right now"),
            }
        }
        set_state(&status, "connecting", "");
        let problem = match read_settings() {
            Err(problem) => problem,
            Ok(settings) => {
                let ended = session(&settings, &mut want, &status, &news, &mut commands).await;
                // No scene is known to be on air any more.
                let _ = news.send(ObsNews::Current(String::new())).await;
                match ended {
                    Ok(()) => continue,
                    Err(problem) => problem,
                }
            }
        };
        set_state(&status, "error", &problem);
        // Wait, but wake at once if it is no longer wanted. Steps asking
        // meanwhile hear why OBS isn't there.
        let retry = tokio::time::sleep(RETRY);
        tokio::pin!(retry);
        loop {
            tokio::select! {
                _ = &mut retry => break,
                changed = want.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    break;
                }
                Some(command) = commands.recv() => refuse(command, &problem),
            }
        }
    }
}

fn refuse(command: Command, why: &str) {
    let _ = command.reply.send(Err(why.to_string()));
}

fn set_state(status: &Mutex<Status>, state: &'static str, problem: &str) {
    let mut s = status.lock();
    s.state = state;
    s.problem = problem.to_string();
    if state != "connected" {
        s.current.clear();
    }
}

const NO_PASSWORD: &str = "OBS wants a WebSocket password but none is saved. Set one in OBS: Tools > WebSocket Server Settings.";

/// OBS's WebSocket server settings, from the file OBS keeps them in. A
/// portable OBS keeps that file beside itself, out of reach: OBS's own
/// defaults (port 4455, no password) are tried then.
fn read_settings() -> Result<Settings, String> {
    let Some(path) = crate::obs_register::obs_config_dirs()
        .into_iter()
        .map(|dir| dir.join("plugin_config/obs-websocket/config.json"))
        .find(|p| p.exists())
    else {
        return Ok(Settings {
            port: DEFAULT_PORT,
            password: None,
        });
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("couldn't read OBS's WebSocket settings: {e}"))?;
    parse_settings(&text)
}

fn parse_settings(text: &str) -> Result<Settings, String> {
    let v = json::parse(text.trim_start_matches('\u{feff}'))
        .map_err(|_| "OBS's WebSocket settings file is damaged".to_string())?;
    if !v.bool_or("server_enabled", false) {
        return Err(
            "Switch on OBS's WebSocket server: Tools > WebSocket Server Settings > Enable WebSocket server."
                .to_string(),
        );
    }
    let port = v
        .get("server_port")
        .and_then(Value::as_f64)
        .filter(|p| (1.0..=65535.0).contains(p))
        .map_or(DEFAULT_PORT, |p| p as u16);
    let password = if v.bool_or("auth_required", false) {
        let saved = v
            .get("server_password")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| NO_PASSWORD.to_string())?;
        Some(saved.to_string())
    } else {
        None
    };
    Ok(Settings { port, password })
}

/// One connection, from handshake until OBS goes away or it is no longer
/// wanted (`Ok`).
async fn session(
    settings: &Settings,
    want: &mut watch::Receiver<bool>,
    status: &Arc<Mutex<Status>>,
    news: &mpsc::Sender<ObsNews>,
    commands: &mut mpsc::Receiver<Command>,
) -> Result<(), String> {
    let addr = format!("127.0.0.1:{}", settings.port);
    let mut stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr))
        .await
        .map_err(|_| "OBS didn't answer. Is OBS open?".to_string())?
        .map_err(|_| "OBS isn't open, or its WebSocket server is off.".to_string())?;
    // Something else answering on OBS's port must not hold the connection
    // half open for ever.
    tokio::time::timeout(CONNECT_TIMEOUT * 2, greet(&mut stream, settings))
        .await
        .map_err(|_| {
            "Something answered on OBS's port, but it isn't OBS's WebSocket server.".to_string()
        })??;
    let (mut reader, mut writer) = stream.into_split();
    let (inbox_tx, mut inbox) = mpsc::channel(64);
    let reader_task = tokio::spawn(async move {
        loop {
            let message = read_message(&mut reader).await;
            let lost = message.is_err();
            if inbox_tx.send(message).await.is_err() || lost {
                return;
            }
        }
    });
    let mut link = Link {
        writer: &mut writer,
        inbox: &mut inbox,
        status,
        news,
    };
    let result = link.serve(want, commands).await;
    reader_task.abort();
    result
}

/// The WebSocket handshake and obs-websocket's Hello and Identify.
async fn greet(stream: &mut TcpStream, settings: &Settings) -> Result<(), String> {
    handshake(stream, settings.port).await?;
    let hello = read_json(stream).await?;
    let identify = identify_message(&hello, settings.password.as_deref())?;
    write_frame(stream, 1, identify.as_bytes()).await?;
    let identified = read_json(stream).await?;
    if identified.u64_or("op", 99) != 2 {
        return Err("OBS refused to connect.".to_string());
    }
    Ok(())
}

/// A live connection: where requests go and messages come from.
struct Link<'a> {
    writer: &'a mut OwnedWriteHalf,
    inbox: &'a mut mpsc::Receiver<Result<Incoming, String>>,
    status: &'a Arc<Mutex<Status>>,
    news: &'a mpsc::Sender<ObsNews>,
}

impl Link<'_> {
    async fn serve(
        &mut self,
        want: &mut watch::Receiver<bool>,
        commands: &mut mpsc::Receiver<Command>,
    ) -> Result<(), String> {
        self.request_lists().await?;
        set_state(self.status, "connected", "");
        loop {
            tokio::select! {
                changed = want.changed() => {
                    if changed.is_err() || !*want.borrow() {
                        let _ = write_frame(self.writer, 8, &[]).await;
                        return Ok(());
                    }
                }
                Some(command) = commands.recv() => {
                    if std::time::Instant::now() >= command.deadline {
                        continue;
                    }
                    let outcome = match &command.action {
                        Action::Check { scene, sources } => self.check(scene, sources).await,
                        action => self.perform(action).await.map(|()| Answer::new()),
                    };
                    let _ = command.reply.send(outcome);
                }
                message = self.inbox.recv() => {
                    let message = message.ok_or("lost OBS")??;
                    self.take(message).await?;
                }
            }
        }
    }

    /// Deal with a message that isn't the answer anyone is waiting for.
    async fn take(&mut self, message: Incoming) -> Result<(), String> {
        match message {
            Incoming::Ping(payload) => write_frame(self.writer, 10, &payload).await,
            Incoming::Json(v) => match handle_message(&v, self.status, self.news).await {
                Follow::Nothing => Ok(()),
                Follow::Lists => self.request_lists().await,
                Follow::Filters(names) => self.request_filters(&names).await,
            },
        }
    }

    async fn request_lists(&mut self) -> Result<(), String> {
        send_request(self.writer, "GetSceneList", "scenes", Value::Null).await?;
        send_request(self.writer, "GetInputList", "inputs", Value::Null).await?;
        send_request(self.writer, "GetHotkeyList", "hotkeys", Value::Null).await
    }

    /// Ask for the filters of each named source or scene; the answers
    /// land in the status (see `handle_message`).
    async fn request_filters(&mut self, names: &[String]) -> Result<(), String> {
        for name in names {
            let data = json::obj([("sourceName", json::str(name))]);
            let id = format!("{FILTERS_ID}{name}");
            send_request(self.writer, "GetSourceFilterList", &id, data).await?;
        }
        Ok(())
    }

    /// Ask OBS for one thing and wait for its answer, dealing with whatever
    /// else arrives meanwhile.
    async fn call(&mut self, request: &str, data: Value) -> Result<Value, String> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = format!("step{}", NEXT.fetch_add(1, Ordering::Relaxed));
        send_request(self.writer, request, &id, data).await?;
        loop {
            let message = tokio::time::timeout(CALL_TIMEOUT, self.inbox.recv())
                .await
                .map_err(|_| "OBS didn't answer in time".to_string())?
                .ok_or("lost OBS")??;
            let Incoming::Json(v) = &message else {
                self.take(message).await?;
                continue;
            };
            if v.u64_or("op", 99) != 7 || v.path("d.requestId").and_then(Value::as_str) != Some(&id)
            {
                self.take(message).await?;
                continue;
            }
            return answer(v);
        }
    }

    async fn perform(&mut self, action: &Action) -> Result<(), String> {
        match action {
            Action::Scene(scene) => {
                let data = json::obj([("sceneName", json::str(scene))]);
                self.call("SetCurrentProgramScene", data).await.map(drop)
            }
            Action::Text { source, text } => {
                let data = json::obj([
                    ("inputName", json::str(source)),
                    ("inputSettings", json::obj([("text", json::str(text))])),
                ]);
                self.call("SetInputSettings", data).await.map(drop)
            }
            Action::Source {
                scene,
                source,
                visible,
            } => self.show_source(scene, source, *visible).await,
            Action::Record(start) => {
                let request = if *start { "StartRecord" } else { "StopRecord" };
                self.call(request, Value::Null).await.map(drop)
            }
            Action::SaveReplay => self.call("SaveReplayBuffer", Value::Null).await.map(drop),
            Action::Mute { source, muted } => {
                let name = ("inputName", json::str(source));
                match muted {
                    Some(muted) => {
                        let data = json::obj([name, ("inputMuted", Value::Bool(*muted))]);
                        self.call("SetInputMute", data).await.map(drop)
                    }
                    None => self
                        .call("ToggleInputMute", json::obj([name]))
                        .await
                        .map(drop),
                }
            }
            Action::Filter {
                source,
                filter,
                enabled,
            } => self.switch_filter(source, filter, *enabled).await,
            Action::Media { source, play } => {
                let action = if *play {
                    "OBS_WEBSOCKET_MEDIA_INPUT_ACTION_RESTART"
                } else {
                    "OBS_WEBSOCKET_MEDIA_INPUT_ACTION_STOP"
                };
                let data = json::obj([
                    ("inputName", json::str(source)),
                    ("mediaAction", json::str(action)),
                ]);
                self.call("TriggerMediaInputAction", data).await.map(drop)
            }
            Action::Hotkey(name) => {
                let data = json::obj([("hotkeyName", json::str(name))]);
                self.call("TriggerHotkeyByName", data).await.map(drop)
            }
            Action::BrowserSource { name, url } => self.add_browser_source(name, url).await,
            // Answered by `check`, which has values to give back.
            Action::Check { .. } => Ok(()),
        }
    }

    /// What `Action::Check` reads. With sources, `muted` and `visible` say
    /// whether all of them are; with several, each also has its own under
    /// its key (`source_keys`). A source that has no sound gets no `muted`;
    /// one OBS doesn't have at all fails the step, as a typo would
    /// otherwise read as "not muted, not showing".
    async fn check(&mut self, scene: &str, sources: &[String]) -> Result<Answer, String> {
        let yes_no = |on: bool| if on { "yes" } else { "no" }.to_string();
        let on_air = self.call("GetCurrentProgramScene", Value::Null).await?;
        let current = on_air.str_or("currentProgramSceneName", "").to_string();
        let streaming = self.call("GetStreamStatus", Value::Null).await?;
        let recording = self.call("GetRecordStatus", Value::Null).await?;
        let mut answer = vec![
            ("scene".to_string(), current.clone()),
            (
                "streaming".to_string(),
                yes_no(streaming.bool_or("outputActive", false)),
            ),
            (
                "recording".to_string(),
                yes_no(recording.bool_or("outputActive", false)),
            ),
        ];
        if sources.is_empty() {
            return Ok(answer);
        }
        let inputs = self.call("GetInputList", Value::Null).await?;
        let scenes = self.call("GetSceneList", Value::Null).await?;
        let known: Vec<String> = names(&inputs, "inputs", "inputName", false)
            .into_iter()
            .chain(names(&scenes, "scenes", "sceneName", false))
            .collect();
        if let Some(missing) = sources.iter().find(|source| !known.contains(source)) {
            return Err(format!("OBS has no source \"{missing}\""));
        }
        let scene = match scene.trim() {
            "" => current,
            name => name.to_string(),
        };
        // (muted, if it has sound; showing) for each source.
        let mut readings = Vec::new();
        for source in sources {
            let asked = json::obj([("inputName", json::str(source))]);
            let muted = match self.call("GetInputMute", asked).await {
                Ok(mute) => Some(mute.bool_or("inputMuted", false)),
                Err(_) => None,
            };
            readings.push((muted, self.is_showing(&scene, source).await));
        }
        let with_sound: Vec<bool> = readings.iter().filter_map(|(muted, _)| *muted).collect();
        if !with_sound.is_empty() {
            answer.push(("muted".to_string(), yes_no(with_sound.iter().all(|m| *m))));
        }
        let all_showing = readings.iter().all(|(_, showing)| *showing);
        answer.push(("visible".to_string(), yes_no(all_showing)));
        if sources.len() > 1 {
            for (key, (muted, showing)) in source_keys(sources).iter().zip(readings) {
                if let Some(muted) = muted {
                    answer.push((format!("{key}.muted"), yes_no(muted)));
                }
                answer.push((format!("{key}.visible"), yes_no(showing)));
            }
        }
        Ok(answer)
    }

    /// Whether `source` is in `scene` and switched on there.
    async fn is_showing(&mut self, scene: &str, source: &str) -> bool {
        let at = json::obj([
            ("sceneName", json::str(scene)),
            ("sourceName", json::str(source)),
        ]);
        let Ok(found) = self.call("GetSceneItemId", at).await else {
            return false;
        };
        let Some(item) = found.get("sceneItemId").and_then(Value::as_f64) else {
            return false;
        };
        let item = json::obj([
            ("sceneName", json::str(scene)),
            ("sceneItemId", Value::Num(item)),
        ]);
        self.call("GetSceneItemEnabled", item)
            .await
            .is_ok_and(|v| v.bool_or("sceneItemEnabled", false))
    }

    /// A browser source showing `url` in the scene on air. One already in
    /// OBS (by that name, or any showing the same page) is pointed at
    /// `url` and added to this scene if it isn't there yet; otherwise a
    /// new one is made, canvas-sized, its sound going through OBS so it
    /// reaches the stream.
    async fn add_browser_source(&mut self, name: &str, url: &str) -> Result<(), String> {
        let on_air = self.call("GetCurrentProgramScene", Value::Null).await?;
        let scene = on_air.str_or("currentProgramSceneName", "").to_string();
        if let Some(existing) = self.browser_source_for(name, url).await? {
            let source = json::str(&existing);
            let point = json::obj([
                ("inputName", source.clone()),
                ("inputSettings", json::obj([("url", json::str(url))])),
            ]);
            self.call("SetInputSettings", point).await?;
            let item = json::obj([("sceneName", json::str(&scene)), ("sourceName", source)]);
            if self.call("GetSceneItemId", item.clone()).await.is_ok() {
                return Ok(());
            }
            return self.call("CreateSceneItem", item).await.map(drop);
        }
        let source = json::str(name);
        let video = self.call("GetVideoSettings", Value::Null).await?;
        let settings = json::obj([
            ("url", json::str(url)),
            ("width", Value::Num(video.u64_or("baseWidth", 1920) as f64)),
            (
                "height",
                Value::Num(video.u64_or("baseHeight", 1080) as f64),
            ),
            ("reroute_audio", Value::Bool(true)),
        ]);
        let create = json::obj([
            ("sceneName", json::str(&scene)),
            ("inputName", source),
            ("inputKind", json::str("browser_source")),
            ("inputSettings", settings),
            ("sceneItemEnabled", Value::Bool(true)),
        ]);
        self.call("CreateInput", create).await.map(drop)
    }

    /// The browser source named `name`, or else one already showing the
    /// page at `url`, so adding the page again never makes a second copy.
    async fn browser_source_for(
        &mut self,
        name: &str,
        url: &str,
    ) -> Result<Option<String>, String> {
        let kind = json::obj([("inputKind", json::str("browser_source"))]);
        let list = self.call("GetInputList", kind).await?;
        let sources = names(&list, "inputs", "inputName", false);
        if sources.iter().any(|s| s == name) {
            return Ok(Some(name.to_string()));
        }
        for source in sources {
            let asked = json::obj([("inputName", json::str(&source))]);
            let settings = self.call("GetInputSettings", asked).await?;
            let shows = settings.path("inputSettings.url").and_then(Value::as_str);
            if shows.is_some_and(|shows| same_page(shows, url)) {
                return Ok(Some(source));
            }
        }
        Ok(None)
    }

    async fn switch_filter(
        &mut self,
        source: &str,
        filter: &str,
        enabled: Option<bool>,
    ) -> Result<(), String> {
        let where_ = || {
            vec![
                ("sourceName".to_string(), json::str(source)),
                ("filterName".to_string(), json::str(filter)),
            ]
        };
        let on = match enabled {
            Some(on) => on,
            None => {
                let now = self
                    .call("GetSourceFilter", Value::Obj(where_()))
                    .await
                    .map_err(|_| format!("\"{source}\" has no filter \"{filter}\""))?;
                !now.bool_or("filterEnabled", true)
            }
        };
        let mut set = where_();
        set.push(("filterEnabled".to_string(), Value::Bool(on)));
        self.call("SetSourceFilterEnabled", Value::Obj(set))
            .await
            .map(drop)
    }

    async fn show_source(
        &mut self,
        scene: &str,
        source: &str,
        visible: Option<bool>,
    ) -> Result<(), String> {
        let scene = match scene.trim() {
            "" => self.status.lock().current.clone(),
            name => name.to_string(),
        };
        let found = self
            .call(
                "GetSceneItemId",
                json::obj([
                    ("sceneName", json::str(&scene)),
                    ("sourceName", json::str(source)),
                ]),
            )
            .await
            .map_err(|_| format!("\"{source}\" isn't in the scene \"{scene}\""))?;
        let item = found
            .get("sceneItemId")
            .and_then(Value::as_f64)
            .ok_or("OBS didn't say where that source is")?;
        let where_ = || {
            vec![
                ("sceneName".to_string(), json::str(&scene)),
                ("sceneItemId".to_string(), Value::Num(item)),
            ]
        };
        let show = match visible {
            Some(show) => show,
            None => {
                let now = self
                    .call("GetSceneItemEnabled", Value::Obj(where_()))
                    .await?;
                !now.bool_or("sceneItemEnabled", true)
            }
        };
        let mut set = where_();
        set.push(("sceneItemEnabled".to_string(), Value::Bool(show)));
        self.call("SetSceneItemEnabled", Value::Obj(set))
            .await
            .map(drop)
    }
}

/// A request's answer: its data, or why OBS said no.
fn answer(v: &Value) -> Result<Value, String> {
    let d = v.get("d").cloned().unwrap_or(Value::Null);
    if d.path("requestStatus.result").and_then(Value::as_bool) == Some(true) {
        return Ok(d.get("responseData").cloned().unwrap_or(Value::Null));
    }
    let comment = d
        .path("requestStatus.comment")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Err(if comment.is_empty() {
        "OBS said no".to_string()
    } else {
        format!("OBS said no: {comment}")
    })
}

async fn send_request(
    writer: &mut OwnedWriteHalf,
    request: &str,
    id: &str,
    data: Value,
) -> Result<(), String> {
    let mut d = vec![
        ("requestType".to_string(), json::str(request)),
        ("requestId".to_string(), json::str(id)),
    ];
    if !matches!(data, Value::Null) {
        d.push(("requestData".to_string(), data));
    }
    let message = json::obj([("op", Value::Num(6.0)), ("d", Value::Obj(d))]).to_json();
    write_frame(writer, 1, message.as_bytes()).await
}

/// What a message from OBS asks for next.
#[derive(Debug, PartialEq)]
enum Follow {
    Nothing,
    /// The scene and source lists again: one was added, removed or renamed.
    Lists,
    /// The filters of these sources or scenes.
    Filters(Vec<String>),
}

/// Follow one message from OBS.
async fn handle_message(
    message: &Value,
    status: &Arc<Mutex<Status>>,
    news: &mpsc::Sender<ObsNews>,
) -> Follow {
    let d = message.get("d").cloned().unwrap_or(Value::Null);
    match message.u64_or("op", 99) {
        5 => handle_event(&d, status, news).await,
        // Request response: one of the lists.
        7 => {
            let Some(data) = d.get("responseData") else {
                return Follow::Nothing;
            };
            let id = d.str_or("requestId", "");
            if let Some(name) = id.strip_prefix(FILTERS_ID) {
                let list = names(data, "filters", "filterName", false);
                status.lock().filters.insert(name.to_string(), list);
                return Follow::Nothing;
            }
            match id {
                "scenes" => {
                    let current = data.str_or("currentProgramSceneName", "").to_string();
                    // OBS lists scenes bottom to top; the dashboard shows them
                    // as OBS's scene dock does.
                    let scenes = names(data, "scenes", "sceneName", true);
                    {
                        let mut s = status.lock();
                        s.scenes = scenes.clone();
                        s.current = current.clone();
                        forget_gone_filters(&mut s);
                    }
                    let _ = news.send(ObsNews::Current(current)).await;
                    Follow::Filters(scenes)
                }
                "inputs" => {
                    let sources = names(data, "inputs", "inputName", false);
                    let mut s = status.lock();
                    s.sources = sources.clone();
                    forget_gone_filters(&mut s);
                    Follow::Filters(sources)
                }
                "hotkeys" => {
                    status.lock().hotkeys = pressable_hotkeys(data);
                    Follow::Nothing
                }
                _ => Follow::Nothing,
            }
        }
        _ => Follow::Nothing,
    }
}

/// An event from OBS: a scene switch, a list that changed, or one of the
/// OBS events triggers can start on.
async fn handle_event(
    d: &Value,
    status: &Arc<Mutex<Status>>,
    news: &mpsc::Sender<ObsNews>,
) -> Follow {
    let data = d.get("eventData").cloned().unwrap_or(Value::Null);
    let happened = match d.str_or("eventType", "") {
        "CurrentProgramSceneChanged" => {
            let name = data.str_or("sceneName", "").to_string();
            status.lock().current = name.clone();
            let _ = news.send(ObsNews::Switched(name)).await;
            return Follow::Nothing;
        }
        "SceneListChanged" | "SceneNameChanged" | "SceneCreated" | "SceneRemoved"
        | "InputCreated" | "InputRemoved" | "InputNameChanged" => return Follow::Lists,
        "SourceFilterCreated" | "SourceFilterRemoved" | "SourceFilterNameChanged" => {
            return Follow::Filters(vec![data.str_or("sourceName", "").to_string()]);
        }
        "RecordStateChanged" => match data.str_or("outputState", "") {
            "OBS_WEBSOCKET_OUTPUT_STARTED" => Event::new(EventKind::ObsRecordingStarted),
            "OBS_WEBSOCKET_OUTPUT_STOPPED" => Event::new(EventKind::ObsRecordingStopped)
                .with("file", data.str_or("outputPath", "")),
            _ => return Follow::Nothing,
        },
        "ReplayBufferSaved" => {
            Event::new(EventKind::ObsReplaySaved).with("file", data.str_or("savedReplayPath", ""))
        }
        "InputMuteStateChanged" => Event::new(EventKind::ObsMuteChanged)
            .with("obs_source", data.str_or("inputName", ""))
            .with(
                "muted",
                if data.bool_or("inputMuted", false) {
                    "yes"
                } else {
                    "no"
                },
            ),
        _ => return Follow::Nothing,
    };
    let _ = news.send(ObsNews::Happened(happened)).await;
    Follow::Nothing
}

/// Drop the filter lists of sources and scenes that are gone.
fn forget_gone_filters(s: &mut Status) {
    let Status {
        scenes,
        sources,
        filters,
        ..
    } = s;
    filters.retain(|name, _| scenes.contains(name) || sources.contains(name));
}

/// The hotkeys a step can press by name alone: each listed once (a
/// hotkey every source or scene has, like mute or "switch to scene",
/// can't say which one by name), and none of libobs' per-source ones.
fn pressable_hotkeys(data: &Value) -> Vec<String> {
    let all: Vec<&str> = data
        .get("hotkeys")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let mut out: Vec<String> = all
        .iter()
        .filter(|name| {
            !name.starts_with("libobs.") && all.iter().filter(|n| n == name).count() == 1
        })
        .map(|name| name.to_string())
        .collect();
    out.sort();
    out
}

/// The `field` of each item in the `list` of an answer.
/// Two addresses of the same page on this PC: `localhost` and 127.0.0.1
/// are one host, and the query (`?at=top`) doesn't make another page.
fn same_page(a: &str, b: &str) -> bool {
    let page = |url: &str| {
        let url = url.trim().to_ascii_lowercase();
        let url = url
            .split(['?', '#'])
            .next()
            .unwrap_or("")
            .trim_end_matches('/');
        url.replacen("://localhost:", "://127.0.0.1:", 1)
    };
    page(a) == page(b)
}

fn names(data: &Value, list: &str, field: &str, reversed: bool) -> Vec<String> {
    let mut out: Vec<String> = data
        .get(list)
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .map(|s| s.str_or(field, "").to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if reversed {
        out.reverse();
    }
    out
}

/// The Identify message answering OBS's Hello, with the password proof
/// when OBS asks for one.
fn identify_message(hello: &Value, password: Option<&str>) -> Result<String, String> {
    if hello.u64_or("op", 99) != 0 {
        return Err("that isn't OBS's WebSocket server".to_string());
    }
    let mut d = vec![
        ("rpcVersion".to_string(), Value::Num(1.0)),
        (
            "eventSubscriptions".to_string(),
            Value::Num(f64::from(
                SCENE_EVENTS | INPUT_EVENTS | FILTER_EVENTS | OUTPUT_EVENTS,
            )),
        ),
    ];
    if let Some(auth) = hello.path("d.authentication") {
        let password = password.ok_or_else(|| NO_PASSWORD.to_string())?;
        // Without both, no proof could ever match: say so instead of
        // sending one OBS can only refuse.
        let (Some(salt), Some(challenge)) = (
            auth.get("salt").and_then(Value::as_str),
            auth.get("challenge").and_then(Value::as_str),
        ) else {
            return Err("OBS asked for a password without the challenge to answer it.".to_string());
        };
        let proof = auth_proof(password, salt, challenge);
        d.push(("authentication".to_string(), json::str(proof)));
    }
    Ok(json::obj([("op", Value::Num(1.0)), ("d", Value::Obj(d))]).to_json())
}

/// obs-websocket's password proof:
/// base64(sha256(base64(sha256(password + salt)) + challenge)).
fn auth_proof(password: &str, salt: &str, challenge: &str) -> String {
    let secret = base64(&crate::sha256::digest(
        format!("{password}{salt}").as_bytes(),
    ));
    base64(&crate::sha256::digest(
        format!("{secret}{challenge}").as_bytes(),
    ))
}

async fn handshake(stream: &mut TcpStream, port: u16) -> Result<(), String> {
    let host = format!("127.0.0.1:{port}");
    ws::handshake(stream, (&host, "/"), Some("obswebsocket.json"), "OBS").await
}

/// The next whole text message during the handshake, as JSON, answering
/// pings on the way.
async fn read_json(stream: &mut TcpStream) -> Result<Value, String> {
    loop {
        match read_message(stream).await? {
            Incoming::Json(v) => return Ok(v),
            Incoming::Ping(payload) => write_frame(stream, 10, &payload).await?,
        }
    }
}

/// The next whole message. A close or a broken connection is an error.
async fn read_message(stream: &mut (impl AsyncRead + Unpin)) -> Result<Incoming, String> {
    match ws::read_message(stream, "OBS").await? {
        ws::Message::Text(text) => json::parse(&text)
            .map(Incoming::Json)
            .map_err(|_| "OBS sent something unreadable".to_string()),
        ws::Message::Ping(payload) => Ok(Incoming::Ping(payload)),
        ws::Message::Close(payload) => Err(close_reason(&payload)),
    }
}

/// What OBS's close code means, for the user.
fn close_reason(payload: &[u8]) -> String {
    let code = (payload.len() >= 2).then(|| u16::from_be_bytes([payload[0], payload[1]]));
    match code {
        Some(4009) => "OBS refused the password. Restart OBS if you just changed it.".to_string(),
        Some(4010) => {
            "This OBS is too old for scene triggers: update to OBS 28 or later.".to_string()
        }
        _ => "OBS closed the connection".to_string(),
    }
}

async fn write_frame(
    stream: &mut (impl AsyncWrite + Unpin),
    opcode: u8,
    payload: &[u8],
) -> Result<(), String> {
    ws::write_frame(stream, opcode, payload, "OBS").await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked example from the obs-websocket protocol document.
    #[test]
    fn the_password_proof_matches_obs_websockets_example() {
        assert_eq!(
            auth_proof(
                "supersecretpassword",
                "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=",
                "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY="
            ),
            "1Ct943GAT+6YQUUX47Ia/ncufilbe6+oD6lY+5kaCu4="
        );
    }

    #[test]
    fn a_step_can_list_several_sources() {
        assert_eq!(
            listed_names(" Mic/Aux \n\nDesktop Audio\nMic/Aux\n"),
            vec!["Mic/Aux".to_string(), "Desktop Audio".to_string()]
        );
        assert!(listed_names("  \n ").is_empty());
        let sources = [
            "Mic/Aux",
            "mic aux",
            "Cámara 1",
            "!!",
            "A very long source name for the stream",
        ]
        .map(String::from);
        assert_eq!(
            source_keys(&sources),
            vec![
                "mic_aux",
                "mic_aux2",
                "c_mara_1",
                "source",
                "a_very_long_source_name"
            ]
        );
    }

    #[test]
    fn the_alerts_page_is_found_under_any_of_its_addresses() {
        let ours = "http://127.0.0.1:7799/alerts";
        assert!(same_page("http://localhost:7799/alerts?at=top", ours));
        assert!(same_page("HTTP://127.0.0.1:7799/alerts/", ours));
        assert!(!same_page("http://127.0.0.1:7800/alerts", ours));
        assert!(!same_page(
            "http://127.0.0.1:7799/overlay/pulse-dot.html",
            ours
        ));
    }

    #[test]
    fn settings_need_the_server_switched_on() {
        let on = r#"{"server_enabled":true,"server_port":4456,"auth_required":true,"server_password":"pw"}"#;
        let s = parse_settings(on).unwrap();
        assert_eq!((s.port, s.password.as_deref()), (4456, Some("pw")));
        let open = r#"{"server_enabled":true,"auth_required":false,"server_password":"pw"}"#;
        let s = parse_settings(open).unwrap();
        assert_eq!((s.port, s.password.as_deref()), (4455, None));
        let unsaved = r#"{"server_enabled":true,"auth_required":true}"#;
        let missing = parse_settings(unsaved).err();
        assert!(missing.is_some_and(|e| e.contains("password")));
        let off = parse_settings(r#"{"server_enabled":false}"#).err();
        assert!(off.is_some_and(|e| e.contains("Tools")));
        assert!(parse_settings("nope").is_err());
    }

    #[test]
    fn identify_answers_hello_with_or_without_a_password() {
        let open = json::parse(r#"{"op":0,"d":{"rpcVersion":1}}"#).unwrap();
        let msg = json::parse(&identify_message(&open, None).unwrap()).unwrap();
        assert_eq!(msg.u64_or("op", 0), 1);
        assert!(msg.path("d.authentication").is_none());
        assert_eq!(
            msg.path("d.eventSubscriptions").and_then(Value::as_f64),
            Some(108.0),
            "scenes, sources, filters and outputs"
        );
        let locked = json::parse(
            r#"{"op":0,"d":{"rpcVersion":1,"authentication":{"salt":"s","challenge":"c"}}}"#,
        )
        .unwrap();
        assert!(identify_message(&locked, None).is_err_and(|e| e.contains("password")));
        let msg = json::parse(&identify_message(&locked, Some("pw")).unwrap()).unwrap();
        assert!(msg.path("d.authentication").is_some());
        let no_challenge =
            json::parse(r#"{"op":0,"d":{"rpcVersion":1,"authentication":{"salt":"s"}}}"#).unwrap();
        assert!(identify_message(&no_challenge, Some("pw")).is_err_and(|e| e.contains("challenge")));
    }

    #[test]
    fn answers_carry_their_data_or_why_obs_said_no() {
        let yes = json::parse(
            r#"{"op":7,"d":{"requestId":"step1","requestStatus":{"result":true,"code":100},"responseData":{"sceneItemId":7}}}"#,
        )
        .unwrap();
        assert_eq!(
            answer(&yes)
                .unwrap()
                .get("sceneItemId")
                .and_then(Value::as_f64),
            Some(7.0)
        );
        let no = json::parse(
            r#"{"op":7,"d":{"requestId":"step2","requestStatus":{"result":false,"code":600,"comment":"No source was found"}}}"#,
        )
        .unwrap();
        assert_eq!(answer(&no).unwrap_err(), "OBS said no: No source was found");
    }

    #[tokio::test]
    async fn lists_land_in_the_status_by_request() {
        let status = Arc::new(Mutex::new(Status::default()));
        let (news, mut heard) = mpsc::channel(4);
        let scenes = json::parse(
            r#"{"op":7,"d":{"requestId":"scenes","requestStatus":{"result":true},"responseData":{"currentProgramSceneName":"Game","scenes":[{"sceneName":"Game"},{"sceneName":"Lobby"}]}}}"#,
        )
        .unwrap();
        let inputs = json::parse(
            r#"{"op":7,"d":{"requestId":"inputs","requestStatus":{"result":true},"responseData":{"inputs":[{"inputName":"Cam"},{"inputName":"Title"}]}}}"#,
        )
        .unwrap();
        assert_eq!(
            handle_message(&scenes, &status, &news).await,
            Follow::Filters(vec!["Lobby".into(), "Game".into()])
        );
        assert_eq!(
            handle_message(&inputs, &status, &news).await,
            Follow::Filters(vec!["Cam".into(), "Title".into()])
        );
        let blur = json::parse(
            r#"{"op":7,"d":{"requestId":"filters:Cam","requestStatus":{"result":true},"responseData":{"filters":[{"filterName":"Blur"}]}}}"#,
        )
        .unwrap();
        assert_eq!(handle_message(&blur, &status, &news).await, Follow::Nothing);
        let s = status.lock().clone();
        assert_eq!(
            s.scenes,
            ["Lobby", "Game"],
            "as OBS's scene dock shows them"
        );
        assert_eq!(s.sources, ["Cam", "Title"]);
        assert_eq!(
            s.filters.get("Cam").map(Vec::as_slice),
            Some(&["Blur".to_string()][..])
        );
        assert!(matches!(heard.recv().await, Some(ObsNews::Current(name)) if name == "Game"));
    }

    #[tokio::test]
    async fn obs_events_become_integration_events() {
        let status = Arc::new(Mutex::new(Status::default()));
        let (news, mut heard) = mpsc::channel(4);
        let event = |kind: &str, data: &str| {
            json::parse(&format!(
                r#"{{"op":5,"d":{{"eventType":"{kind}","eventData":{data}}}}}"#
            ))
            .unwrap()
        };
        let stopped = event(
            "RecordStateChanged",
            r#"{"outputActive":false,"outputState":"OBS_WEBSOCKET_OUTPUT_STOPPED","outputPath":"C:/v/a.mkv"}"#,
        );
        handle_message(&stopped, &status, &news).await;
        let Some(ObsNews::Happened(e)) = heard.recv().await else {
            panic!("a recording event");
        };
        assert_eq!(
            (e.kind, e.var("file")),
            (EventKind::ObsRecordingStopped, Some("C:/v/a.mkv"))
        );

        let stopping = event(
            "RecordStateChanged",
            r#"{"outputState":"OBS_WEBSOCKET_OUTPUT_STOPPING"}"#,
        );
        handle_message(&stopping, &status, &news).await;
        let muted = event(
            "InputMuteStateChanged",
            r#"{"inputName":"Mic","inputMuted":true}"#,
        );
        handle_message(&muted, &status, &news).await;
        let Some(ObsNews::Happened(e)) = heard.recv().await else {
            panic!("a mute event, and nothing for stopping");
        };
        assert_eq!(
            (e.var("obs_source"), e.var("muted")),
            (Some("Mic"), Some("yes"))
        );

        let filter_added = event(
            "SourceFilterCreated",
            r#"{"sourceName":"Cam","filterName":"Blur"}"#,
        );
        assert_eq!(
            handle_message(&filter_added, &status, &news).await,
            Follow::Filters(vec!["Cam".into()])
        );
    }

    #[test]
    fn only_hotkeys_a_name_can_press_are_offered() {
        let data = json::parse(
            r#"{"hotkeys":["OBSBasic.StartRecording","libobs.mute","libobs.mute","OBSBasic.SelectScene","OBSBasic.SelectScene","OBSBasic.Screenshot"]}"#,
        )
        .unwrap();
        assert_eq!(
            pressable_hotkeys(&data),
            ["OBSBasic.Screenshot", "OBSBasic.StartRecording"]
        );
    }

    #[test]
    fn close_codes_explain_themselves() {
        assert!(close_reason(&4009u16.to_be_bytes()).contains("password"));
        assert!(close_reason(&[]).contains("closed"));
    }
}
