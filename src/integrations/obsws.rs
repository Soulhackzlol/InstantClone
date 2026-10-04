//! The connection to OBS's own WebSocket server (obs-websocket 5, built
//! into OBS 28 and later). Scene triggers hear from it which scene is on
//! air and when it switches; OBS steps ask it to switch a scene, show or
//! hide a source, or change a text source.
//!
//! Only scene and source events are asked for, and OBS is only asked to do
//! what an OBS step says. The address, port and password are read from
//! OBS's own settings file, so the user only has to switch the server on in
//! OBS.
//!
//! A minimal WebSocket client (RFC 6455) lives here rather than a crate:
//! one local connection, text frames, no extensions. A reader task owns the
//! socket's read half, so waiting for a message can always be interrupted
//! (by a step's request, or the connection no longer being wanted) without
//! losing half a frame.

use crate::json::{self, Value};
use crate::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};

/// How long to wait before trying again after OBS refused or went away.
const RETRY: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a step's request may wait for OBS's answer.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// A scene list is small; anything this big is not OBS talking.
const MAX_MESSAGE: usize = 4 * 1024 * 1024;
/// obs-websocket's event categories (`EventSubscription`): scenes, and
/// inputs (sources added, removed or renamed).
const SCENE_EVENTS: u32 = 1 << 2;
const INPUT_EVENTS: u32 = 1 << 3;
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
            ("current", json::str(&self.current)),
        ])
    }
}

/// What OBS tells the engine.
#[derive(Debug, PartialEq)]
pub enum SceneNews {
    /// The scene on air when the connection opened: not a switch.
    Current(String),
    /// The program scene switched.
    Switched(String),
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
}

/// An action and where its outcome goes. The answer channel is a std one:
/// a step waits for it on a blocking thread.
pub struct Command {
    pub action: Action,
    pub reply: std::sync::mpsc::Sender<Result<(), String>>,
    /// After this the step has given up: doing it then would switch a
    /// scene nobody asked for any more.
    pub deadline: std::time::Instant,
}

/// Not `Debug`: it holds the WebSocket password.
struct Settings {
    port: u16,
    password: String,
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
    news: mpsc::Sender<SceneNews>,
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
                let _ = news.send(SceneNews::Current(String::new())).await;
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

/// OBS's WebSocket server settings, from the file OBS keeps them in.
fn read_settings() -> Result<Settings, String> {
    let path = crate::obs_register::obs_config_dirs()
        .into_iter()
        .map(|dir| dir.join("plugin_config/obs-websocket/config.json"))
        .find(|p| p.exists())
        .ok_or(
            "OBS's WebSocket settings weren't found. In OBS: Tools > WebSocket Server Settings.",
        )?;
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
        v.str_or("server_password", "").to_string()
    } else {
        String::new()
    };
    Ok(Settings { port, password })
}

/// One connection, from handshake until OBS goes away or it is no longer
/// wanted (`Ok`).
async fn session(
    settings: &Settings,
    want: &mut watch::Receiver<bool>,
    status: &Arc<Mutex<Status>>,
    news: &mpsc::Sender<SceneNews>,
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
    let identify = identify_message(&hello, &settings.password)?;
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
    news: &'a mpsc::Sender<SceneNews>,
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
                    let outcome = self.perform(&command.action).await;
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
            Incoming::Json(v) => {
                if handle_message(&v, self.status, self.news).await {
                    self.request_lists().await?;
                }
                Ok(())
            }
        }
    }

    async fn request_lists(&mut self) -> Result<(), String> {
        send_request(self.writer, "GetSceneList", "scenes", Value::Null).await?;
        send_request(self.writer, "GetInputList", "inputs", Value::Null).await
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
        }
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

/// Follow one message from OBS. True when the scene and source lists
/// should be asked for again (one was added, removed or renamed).
async fn handle_message(
    message: &Value,
    status: &Arc<Mutex<Status>>,
    news: &mpsc::Sender<SceneNews>,
) -> bool {
    let d = message.get("d").cloned().unwrap_or(Value::Null);
    match message.u64_or("op", 99) {
        // Event
        5 => match d.str_or("eventType", "") {
            "CurrentProgramSceneChanged" => {
                let name = d
                    .get("eventData")
                    .map(|e| e.str_or("sceneName", "").to_string())
                    .unwrap_or_default();
                status.lock().current = name.clone();
                let _ = news.send(SceneNews::Switched(name)).await;
                false
            }
            "SceneListChanged" | "SceneNameChanged" | "SceneCreated" | "SceneRemoved"
            | "InputCreated" | "InputRemoved" | "InputNameChanged" => true,
            _ => false,
        },
        // Request response: one of the lists.
        7 => {
            let Some(data) = d.get("responseData") else {
                return false;
            };
            match d.str_or("requestId", "") {
                "scenes" => {
                    let current = data.str_or("currentProgramSceneName", "").to_string();
                    // OBS lists scenes bottom to top; the dashboard shows them
                    // as OBS's scene dock does.
                    let scenes = names(data, "scenes", "sceneName", true);
                    {
                        let mut s = status.lock();
                        s.scenes = scenes;
                        s.current = current.clone();
                    }
                    let _ = news.send(SceneNews::Current(current)).await;
                }
                "inputs" => status.lock().sources = names(data, "inputs", "inputName", false),
                _ => {}
            }
            false
        }
        _ => false,
    }
}

/// The `field` of each item in the `list` of an answer.
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
fn identify_message(hello: &Value, password: &str) -> Result<String, String> {
    if hello.u64_or("op", 99) != 0 {
        return Err("that isn't OBS's WebSocket server".to_string());
    }
    let mut d = vec![
        ("rpcVersion".to_string(), Value::Num(1.0)),
        (
            "eventSubscriptions".to_string(),
            Value::Num(f64::from(SCENE_EVENTS | INPUT_EVENTS)),
        ),
    ];
    if let Some(auth) = hello.path("d.authentication") {
        if password.is_empty() {
            return Err("OBS wants a WebSocket password but none is saved. Set one in OBS: Tools > WebSocket Server Settings.".to_string());
        }
        let proof = auth_proof(
            password,
            auth.str_or("salt", ""),
            auth.str_or("challenge", ""),
        );
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
    let mut key = [0u8; 16];
    crate::crypto::os_random(&mut key);
    let request = format!(
        "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: obswebsocket.json\r\n\r\n",
        base64(&key)
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("lost OBS: {e}"))?;
    // Read the answer's head, a byte at a time: it is short, and the first
    // frame may follow right behind it.
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 8192 {
            return Err("that isn't OBS's WebSocket server".to_string());
        }
        head.push(
            stream
                .read_u8()
                .await
                .map_err(|e| format!("lost OBS: {e}"))?,
        );
    }
    let status_line = String::from_utf8_lossy(&head);
    if !status_line.starts_with("HTTP/1.1 101") {
        return Err("OBS's WebSocket server refused the connection".to_string());
    }
    Ok(())
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
    let mut message = Vec::new();
    loop {
        let (fin, opcode, payload) = read_frame(stream).await?;
        match opcode {
            // Continuation, text, binary.
            0..=2 => {
                if message.len() + payload.len() > MAX_MESSAGE {
                    return Err("OBS sent too much at once".to_string());
                }
                message.extend_from_slice(&payload);
                if fin {
                    let text = String::from_utf8_lossy(&message);
                    return json::parse(&text)
                        .map(Incoming::Json)
                        .map_err(|_| "OBS sent something unreadable".to_string());
                }
            }
            8 => return Err(close_reason(&payload)),
            9 => return Ok(Incoming::Ping(payload)),
            _ => {}
        }
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

async fn read_frame(stream: &mut (impl AsyncRead + Unpin)) -> Result<(bool, u8, Vec<u8>), String> {
    let lost = |e: std::io::Error| format!("lost OBS: {e}");
    let b0 = stream.read_u8().await.map_err(lost)?;
    let b1 = stream.read_u8().await.map_err(lost)?;
    let len = match b1 & 0x7f {
        126 => u64::from(stream.read_u16().await.map_err(lost)?),
        127 => stream.read_u64().await.map_err(lost)?,
        n => u64::from(n),
    };
    if len > MAX_MESSAGE as u64 {
        return Err("OBS sent too much at once".to_string());
    }
    let mask = if b1 & 0x80 != 0 {
        let mut m = [0u8; 4];
        stream.read_exact(&mut m).await.map_err(lost)?;
        Some(m)
    } else {
        None
    };
    let mut payload = vec![0u8; len as usize];
    stream.read_exact(&mut payload).await.map_err(lost)?;
    if let Some(m) = mask {
        payload
            .iter_mut()
            .enumerate()
            .for_each(|(i, b)| *b ^= m[i % 4]);
    }
    Ok((b0 & 0x80 != 0, b0 & 0x0f, payload))
}

/// One frame from us: always final, always masked (clients must mask).
async fn write_frame(
    stream: &mut (impl AsyncWrite + Unpin),
    opcode: u8,
    payload: &[u8],
) -> Result<(), String> {
    stream
        .write_all(&encode_frame(opcode, payload))
        .await
        .map_err(|e| format!("lost OBS: {e}"))
}

fn encode_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut mask = [0u8; 4];
    crate::crypto::os_random(&mut mask);
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | opcode);
    match payload.len() {
        n if n < 126 => out.push(0x80 | n as u8),
        n if n <= 0xffff => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// Standard base64 with padding, as obs-websocket and the handshake use.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

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
    fn settings_need_the_server_switched_on() {
        let on = r#"{"server_enabled":true,"server_port":4456,"auth_required":true,"server_password":"pw"}"#;
        let s = parse_settings(on).unwrap();
        assert_eq!((s.port, s.password.as_str()), (4456, "pw"));
        let open = r#"{"server_enabled":true,"auth_required":false,"server_password":"pw"}"#;
        let s = parse_settings(open).unwrap();
        assert_eq!((s.port, s.password.as_str()), (4455, ""));
        let off = parse_settings(r#"{"server_enabled":false}"#).err();
        assert!(off.is_some_and(|e| e.contains("Tools")));
        assert!(parse_settings("nope").is_err());
    }

    #[test]
    fn identify_answers_hello_with_or_without_a_password() {
        let open = json::parse(r#"{"op":0,"d":{"rpcVersion":1}}"#).unwrap();
        let msg = json::parse(&identify_message(&open, "").unwrap()).unwrap();
        assert_eq!(msg.u64_or("op", 0), 1);
        assert!(msg.path("d.authentication").is_none());
        assert_eq!(
            msg.path("d.eventSubscriptions").and_then(Value::as_f64),
            Some(12.0),
            "scenes and sources"
        );
        let locked = json::parse(
            r#"{"op":0,"d":{"rpcVersion":1,"authentication":{"salt":"s","challenge":"c"}}}"#,
        )
        .unwrap();
        assert!(identify_message(&locked, "").is_err_and(|e| e.contains("password")));
        let msg = json::parse(&identify_message(&locked, "pw").unwrap()).unwrap();
        assert!(msg.path("d.authentication").is_some());
    }

    #[test]
    fn frames_are_masked_and_sized() {
        let small = encode_frame(1, b"hi");
        assert_eq!(small[0], 0x81);
        assert_eq!(small[1], 0x80 | 2);
        let mask = [small[2], small[3], small[4], small[5]];
        assert_eq!([small[6] ^ mask[0], small[7] ^ mask[1]], *b"hi");
        let mid = encode_frame(1, &[0u8; 300]);
        assert_eq!(mid[1], 0x80 | 126);
        assert_eq!(u16::from_be_bytes([mid[2], mid[3]]), 300);
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
        assert!(!handle_message(&scenes, &status, &news).await);
        assert!(!handle_message(&inputs, &status, &news).await);
        let s = status.lock().clone();
        assert_eq!(
            s.scenes,
            ["Lobby", "Game"],
            "as OBS's scene dock shows them"
        );
        assert_eq!(s.sources, ["Cam", "Title"]);
        assert_eq!(heard.recv().await, Some(SceneNews::Current("Game".into())));
    }

    #[test]
    fn close_codes_explain_themselves() {
        assert!(close_reason(&4009u16.to_be_bytes()).contains("password"));
        assert!(close_reason(&[]).contains("closed"));
    }
}
