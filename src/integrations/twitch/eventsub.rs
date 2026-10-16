//! Twitch EventSub over WebSocket: follows, subs, gifted subs, bits,
//! raids, channel point redemptions and ad breaks, as integration events.
//!
//! The connection only opens while an enabled integration starts on one of
//! these events and the streamer's account is logged in. Subscriptions are
//! made for exactly those events, over Helix, within the 10 s Twitch allows
//! after the welcome; they end with the connection, so a change in what is
//! wanted simply starts a fresh session. When Twitch moves the session to
//! another server, the old connection keeps delivering until the new one is
//! welcomed, so nothing is missed in between.
//!
//! Each event needs a permission (scope) the login asked for. A login from
//! before these events existed lacks it: Twitch refuses that subscription,
//! and the status says which events need the user to connect again.

use super::Twitch;
use crate::integrations::event::{Event, EventKind};
use crate::integrations::ws;
use crate::json::{self, Value};
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

const HOST: &str = "eventsub.wss.twitch.tv";
/// Twitch's default keepalive is 10 s and it allows up to 600 s; 30 s is
/// fewer wakeups and still plenty to notice a dead connection.
const PATH: &str = "/ws?keepalive_timeout_seconds=30";
/// How long the welcome may take, and how much longer than the keepalive
/// a silence may last before the connection counts as dead.
const WELCOME_WAIT: Duration = Duration::from_secs(10);
const QUIET_SLACK: Duration = Duration::from_secs(10);
const RETRY_FIRST: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(120);
/// How long a connection must listen before its loss starts the backoff
/// over. A server that welcomes us and drops us at once would otherwise be
/// reconnected to, with every subscription asked again, every 5 s.
const STEADY: Duration = Duration::from_secs(60);
/// Twitch may deliver a notification twice; ids seen this recently are
/// dropped.
const SEEN_IDS: usize = 64;

/// The EventSub subscription behind an event: its type, version and the
/// scope it needs (empty: none).
pub fn subscription(kind: EventKind) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match kind {
        EventKind::TwitchFollow => ("channel.follow", "2", "moderator:read:followers"),
        EventKind::TwitchSub => ("channel.subscribe", "1", "channel:read:subscriptions"),
        EventKind::TwitchResub => (
            "channel.subscription.message",
            "1",
            "channel:read:subscriptions",
        ),
        EventKind::TwitchGift => (
            "channel.subscription.gift",
            "1",
            "channel:read:subscriptions",
        ),
        EventKind::TwitchCheer => ("channel.cheer", "1", "bits:read"),
        EventKind::TwitchRaid => ("channel.raid", "1", ""),
        EventKind::TwitchRedeem => (
            "channel.channel_points_custom_reward_redemption.add",
            "1",
            "channel:read:redemptions",
        ),
        EventKind::TwitchAd => ("channel.ad_break.begin", "1", "channel:read:ads"),
        _ => return None,
    })
}

/// Where one event's subscription stands.
#[derive(Clone, Debug, PartialEq)]
pub enum SubState {
    Pending,
    Listening,
    /// The login lacks this event's permission: connect Twitch again.
    NeedsConsent,
    Failed(String),
}

/// What the dashboard shows about Twitch events.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// `off`, `connecting`, `connected` or `error`.
    pub state: &'static str,
    pub problem: String,
    /// By event id (`twitch_raid`), so the editor finds a trigger's.
    pub subs: BTreeMap<&'static str, SubState>,
}

impl Status {
    pub fn to_json(&self) -> Value {
        let subs = self
            .subs
            .iter()
            .map(|(id, state)| {
                let text = match state {
                    SubState::Pending => "pending".to_string(),
                    SubState::Listening => "listening".to_string(),
                    SubState::NeedsConsent => "needs_consent".to_string(),
                    SubState::Failed(why) => format!("error: {why}"),
                };
                (id.to_string(), json::str(text))
            })
            .collect();
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
            ("subs", Value::Obj(subs)),
        ])
    }
}

/// How a session ended, when not by an error.
enum Ended {
    /// What is wanted, or the account, changed: start a fresh session.
    Changed,
    /// Nothing could be subscribed (every event needs consent): wait for a
    /// change instead of reconnecting for nothing.
    NothingToHear,
}

type Writer = tokio::io::WriteHalf<tokio_native_tls::TlsStream<tokio::net::TcpStream>>;

/// One welcomed connection to Twitch.
struct Link {
    inbox: mpsc::Receiver<Result<ws::Message, String>>,
    writer: Writer,
    reader: tokio::task::JoinHandle<()>,
    session_id: String,
    keepalive: Duration,
}

impl Drop for Link {
    /// Dropping the link closes its connection.
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// A connection to Twitch's new server, opening while the old one listens.
type Moving = Pin<Box<dyn Future<Output = Result<Link, String>> + Send>>;

/// Keep the connection while events are wanted. Runs for the app's life.
pub async fn run(
    twitch: Arc<Twitch>,
    mut wanted: watch::Receiver<Vec<EventKind>>,
    mut account: watch::Receiver<u64>,
    events: mpsc::Sender<Event>,
) {
    let mut retry = RETRY_FIRST;
    loop {
        let kinds = wanted.borrow_and_update().clone();
        account.borrow_and_update();
        let user_id = twitch.main_user_id();
        if kinds.is_empty() || user_id.is_empty() {
            twitch.set_event_status(|s| {
                s.state = "off";
                s.problem.clear();
                s.subs.clear();
            });
            wait_for_change(&mut wanted, &mut account, None).await;
            continue;
        }
        twitch.set_event_status(|s| {
            s.state = "connecting";
            s.problem.clear();
            s.subs = kinds.iter().map(|k| (k.id(), SubState::Pending)).collect();
        });
        let started = Instant::now();
        let ended = session(&twitch, &kinds, (&mut wanted, &mut account), &events).await;
        match ended {
            Ok(Ended::Changed) => retry = RETRY_FIRST,
            Ok(Ended::NothingToHear) => {
                twitch.set_event_status(|s| s.state = "off");
                wait_for_change(&mut wanted, &mut account, None).await;
            }
            Err(problem) => {
                // A connection that listened for a while starts the backoff
                // over: only failures in a row wait longer and longer.
                if twitch.event_status.lock().state == "connected" && started.elapsed() >= STEADY {
                    retry = RETRY_FIRST;
                }
                (twitch.log)(format!(
                    "[twitch] events: {problem}; retrying in {} s",
                    retry.as_secs()
                ));
                twitch.set_event_status(|s| {
                    s.state = "error";
                    s.problem = problem;
                });
                wait_for_change(&mut wanted, &mut account, Some(retry)).await;
                retry = (retry * 2).min(RETRY_MAX);
            }
        }
    }
}

/// Until what is wanted or the account changes, or `limit` passes.
async fn wait_for_change(
    wanted: &mut watch::Receiver<Vec<EventKind>>,
    account: &mut watch::Receiver<u64>,
    limit: Option<Duration>,
) {
    let limit = async {
        match limit {
            Some(limit) => tokio::time::sleep(limit).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        _ = wanted.changed() => {}
        _ = account.changed() => {}
        _ = limit => {}
    }
}

/// One session, from the welcome until it ends, following Twitch when it
/// moves the session to another server.
async fn session(
    twitch: &Arc<Twitch>,
    kinds: &[EventKind],
    watching: (
        &mut watch::Receiver<Vec<EventKind>>,
        &mut watch::Receiver<u64>,
    ),
    events: &mpsc::Sender<Event>,
) -> Result<Ended, String> {
    let (wanted, account) = watching;
    let mut link = open(HOST.to_string(), PATH.to_string()).await?;
    if !subscribe_all(twitch, kinds, &link.session_id).await? {
        return Ok(Ended::NothingToHear);
    }
    twitch.set_event_status(|s| s.state = "connected");
    (twitch.log)(format!(
        "[twitch] events: listening ({})",
        listening_names(twitch)
    ));
    let mut seen: VecDeque<String> = VecDeque::new();
    let mut moving: Option<Moving> = None;
    loop {
        tokio::select! {
            _ = wanted.changed() => return Ok(Ended::Changed),
            _ = account.changed() => return Ok(Ended::Changed),
            new = async { moving.as_mut().expect("guarded by is_some").await }, if moving.is_some() => {
                moving = None;
                link = moved(twitch, new)?;
            }
            message = tokio::time::timeout(link.keepalive + QUIET_SLACK, next_json(&mut link.inbox, &mut link.writer)) => {
                let message = match (message, moving.take()) {
                    (Ok(Ok(message)), still_moving) => {
                        moving = still_moving;
                        message
                    }
                    // The old server may hang up before the new one welcomes
                    // us: the move decides how this ends.
                    (_, Some(new)) => {
                        link = moved(twitch, new.await)?;
                        continue;
                    }
                    (Ok(Err(problem)), None) => return Err(problem),
                    (Err(_), None) => return Err("Twitch went quiet".to_string()),
                };
                if let Some(url) = take(twitch, &message, &mut seen, events).await {
                    let (host, path) = split_url(&url).ok_or("Twitch sent a reconnect address that isn't one")?;
                    moving = Some(Box::pin(open(host, path)));
                }
            }
        }
    }
}

/// The new link once Twitch's move is done. The subscriptions moved with the
/// session, and replacing the old link closes its connection, as Twitch asks,
/// only now that the new one is welcomed.
fn moved(twitch: &Twitch, new: Result<Link, String>) -> Result<Link, String> {
    let link = new.map_err(|e| format!("couldn't follow Twitch to its new server: {e}"))?;
    (twitch.log)("[twitch] events: moved to a new Twitch server".to_string());
    Ok(link)
}

/// Connect to Twitch at this address and wait for its welcome.
async fn open(host: String, path: String) -> Result<Link, String> {
    // Each stage is timed: a link that accepts the connection and then
    // stalls (a captive portal, a dying proxy) must not hang this task.
    let tcp = tokio::time::timeout(
        WELCOME_WAIT,
        tokio::net::TcpStream::connect((host.as_str(), 443)),
    )
    .await
    .map_err(|_| "Twitch didn't answer".to_string())?
    .map_err(|e| format!("couldn't reach Twitch: {e}"))?;
    let connector = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    let tls = tokio::time::timeout(WELCOME_WAIT, async {
        let mut tls = tokio_native_tls::TlsConnector::from(connector)
            .connect(&host, tcp)
            .await
            .map_err(|e| format!("couldn't reach Twitch securely: {e}"))?;
        ws::handshake(&mut tls, (&host, &path), None, "Twitch").await?;
        Ok::<_, String>(tls)
    })
    .await
    .map_err(|_| "Twitch didn't finish connecting".to_string())??;
    let (mut reader, writer) = tokio::io::split(tls);
    let (inbox_tx, inbox) = mpsc::channel(64);
    let reader = tokio::spawn(async move {
        loop {
            let message = ws::read_message(&mut reader, "Twitch").await;
            let lost = message.is_err();
            if inbox_tx.send(message).await.is_err() || lost {
                return;
            }
        }
    });
    let mut link = Link {
        inbox,
        writer,
        reader,
        session_id: String::new(),
        keepalive: Duration::from_secs(30),
    };
    let welcome = tokio::time::timeout(WELCOME_WAIT, next_json(&mut link.inbox, &mut link.writer))
        .await
        .map_err(|_| "Twitch sent no welcome".to_string())??;
    if welcome
        .path("metadata.message_type")
        .and_then(Value::as_str)
        != Some("session_welcome")
    {
        return Err("Twitch didn't welcome the connection".to_string());
    }
    link.session_id = welcome
        .path("payload.session.id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if let Some(keepalive) = keepalive_of(&welcome) {
        link.keepalive = keepalive;
    }
    Ok(link)
}

/// The silence Twitch's welcome allows between keepalives. Twitch picks 10
/// to 600 s; a welcome outside that range is held to it, so a broken one
/// can neither overflow `Duration` (a panic that would end the task) nor
/// make every short pause count as a dead connection.
fn keepalive_of(welcome: &Value) -> Option<Duration> {
    let seconds = welcome
        .path("payload.session.keepalive_timeout_seconds")
        .and_then(Value::as_f64)?;
    Duration::try_from_secs_f64(seconds.clamp(10.0, 600.0)).ok()
}

/// The next JSON message, answering pings on the way.
async fn next_json(
    inbox: &mut mpsc::Receiver<Result<ws::Message, String>>,
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
) -> Result<Value, String> {
    loop {
        match inbox.recv().await.ok_or("lost Twitch")?? {
            ws::Message::Text(text) => {
                return json::parse(&text)
                    .map_err(|_| "Twitch sent something unreadable".to_string())
            }
            ws::Message::Ping(payload) => ws::write_frame(writer, 10, &payload, "Twitch").await?,
            ws::Message::Close(payload) => return Err(close_reason(&payload)),
        }
    }
}

/// Follow one message. `Some(address)` when Twitch moves the session there.
async fn take(
    twitch: &Twitch,
    message: &Value,
    seen: &mut VecDeque<String>,
    events: &mpsc::Sender<Event>,
) -> Option<String> {
    match message
        .path("metadata.message_type")
        .and_then(Value::as_str)?
    {
        "notification" => {
            let id = message
                .path("metadata.message_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !id.is_empty() {
                if seen.iter().any(|s| s == id) {
                    return None;
                }
                seen.push_back(id.to_string());
                if seen.len() > SEEN_IDS {
                    seen.pop_front();
                }
            }
            let kind = message
                .path("payload.subscription.type")
                .and_then(Value::as_str)?;
            let event = message.path("payload.event")?;
            if let Some(event) = to_event(kind, event) {
                let _ = events.send(event).await;
            }
            None
        }
        "session_reconnect" => message
            .path("payload.session.reconnect_url")
            .and_then(Value::as_str)
            .map(str::to_string),
        "revocation" => {
            let kind = message
                .path("payload.subscription.type")
                .and_then(Value::as_str)
                .unwrap_or("");
            let why = message
                .path("payload.subscription.status")
                .and_then(Value::as_str)
                .unwrap_or("");
            (twitch.log)(format!("[twitch] events: Twitch stopped {kind} ({why})"));
            let id = EventKind::ALL
                .into_iter()
                .find(|k| subscription(*k).is_some_and(|(t, ..)| t == kind))
                .map_or("", |k| k.id());
            twitch.set_event_status(|s| {
                if let Some(state) = s.subs.get_mut(id) {
                    *state = if why == "authorization_revoked" {
                        SubState::NeedsConsent
                    } else {
                        SubState::Failed(format!("Twitch stopped it ({why})"))
                    };
                }
            });
            None
        }
        _ => None,
    }
}

/// Subscribe to each wanted event. `Ok(true)`: at least one is listening. `Ok(false)`: none can be until
/// the user connects Twitch again. `Err`: none is listening because Twitch
/// or the network failed, which is worth another try.
async fn subscribe_all(
    twitch: &Arc<Twitch>,
    kinds: &[EventKind],
    session_id: &str,
) -> Result<bool, String> {
    let mut any = false;
    let mut failure = None;
    for kind in kinds {
        let Some((kind_type, version, _)) = subscription(*kind) else {
            continue;
        };
        let (me, session) = (twitch.clone(), session_id.to_string());
        let outcome =
            tokio::task::spawn_blocking(move || me.subscribe_event(kind_type, version, &session))
                .await
                .unwrap_or_else(|_| SubState::Failed("crashed".to_string()));
        any |= outcome == SubState::Listening;
        if let SubState::Failed(why) = &outcome {
            failure.get_or_insert_with(|| why.clone());
        }
        twitch.set_event_status(|s| {
            s.subs.insert(kind.id(), outcome);
        });
    }
    match failure {
        Some(why) if !any => Err(format!("couldn't subscribe: {why}")),
        _ => Ok(any),
    }
}

fn listening_names(twitch: &Twitch) -> String {
    let status = twitch.event_status.lock();
    status
        .subs
        .iter()
        .filter(|(_, state)| **state == SubState::Listening)
        .map(|(id, _)| id.trim_start_matches("twitch_"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The condition Twitch wants for a subscription: whose channel.
pub fn condition(kind_type: &str, user_id: &str) -> Value {
    match kind_type {
        "channel.raid" => json::obj([("to_broadcaster_user_id", json::str(user_id))]),
        "channel.follow" => json::obj([
            ("broadcaster_user_id", json::str(user_id)),
            ("moderator_user_id", json::str(user_id)),
        ]),
        _ => json::obj([("broadcaster_user_id", json::str(user_id))]),
    }
}

/// A Twitch notification as an integration event, with its variables.
pub fn to_event(kind_type: &str, e: &Value) -> Option<Event> {
    // Anonymous gifts and cheers have no name.
    let user = |field: &str| match e.str_or(field, "") {
        "" => "Anonymous".to_string(),
        name => name.to_string(),
    };
    let number = |path: &str| {
        e.path(path)
            .and_then(Value::as_f64)
            .map_or("0".to_string(), |n| format!("{n:.0}"))
    };
    let tier = || match e.str_or("tier", "1000") {
        "2000" => "2",
        "3000" => "3",
        _ => "1",
    };
    let yes_no = |b: bool| if b { "yes" } else { "no" };
    Some(match kind_type {
        "channel.follow" => Event::new(EventKind::TwitchFollow)
            .with("user", user("user_name"))
            .with("user_login", e.str_or("user_login", "")),
        "channel.subscribe" => Event::new(EventKind::TwitchSub)
            .with("user", user("user_name"))
            .with("user_login", e.str_or("user_login", ""))
            .with("tier", tier())
            .with("gift", yes_no(e.bool_or("is_gift", false))),
        "channel.subscription.message" => Event::new(EventKind::TwitchResub)
            .with("user", user("user_name"))
            .with("user_login", e.str_or("user_login", ""))
            .with("tier", tier())
            .with("months", number("cumulative_months"))
            .with("streak", number("streak_months"))
            .with(
                "message",
                e.path("message.text").and_then(Value::as_str).unwrap_or(""),
            ),
        "channel.subscription.gift" => Event::new(EventKind::TwitchGift)
            .with("user", user("user_name"))
            .with("user_login", e.str_or("user_login", ""))
            .with("count", number("total"))
            .with("tier", tier())
            .with("total", number("cumulative_total")),
        "channel.cheer" => Event::new(EventKind::TwitchCheer)
            .with("user", user("user_name"))
            .with("user_login", e.str_or("user_login", ""))
            .with("bits", number("bits"))
            .with("message", e.str_or("message", ""))
            .with("words", without_cheers(e.str_or("message", ""))),
        "channel.raid" => Event::new(EventKind::TwitchRaid)
            .with("user", user("from_broadcaster_user_name"))
            .with("user_login", e.str_or("from_broadcaster_user_login", ""))
            .with("viewers", number("viewers")),
        "channel.channel_points_custom_reward_redemption.add" => {
            Event::new(EventKind::TwitchRedeem)
                .with("user", user("user_name"))
                .with("user_login", e.str_or("user_login", ""))
                .with(
                    "reward",
                    e.path("reward.title").and_then(Value::as_str).unwrap_or(""),
                )
                .with("cost", number("reward.cost"))
                .with("input", e.str_or("user_input", ""))
        }
        "channel.ad_break.begin" => Event::new(EventKind::TwitchAd)
            .with("seconds", number("duration_seconds"))
            .with("automatic", yes_no(e.bool_or("is_automatic", false))),
        _ => return None,
    })
}

/// A cheer's message without its cheers: Twitch writes each as a word of
/// letters then the bits (`Cheer100`, `Kappa50`), which an alert has no use
/// for once it says how many bits came.
fn without_cheers(message: &str) -> String {
    message
        .split_whitespace()
        .filter(|word| {
            let digits = word.trim_start_matches(|c: char| c.is_ascii_alphabetic());
            !(digits.len() < word.len()
                && !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Host and path of a Twitch `wss://` address (port 443).
fn split_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("wss://")?;
    let (host, path) = match rest.find(['/', '?']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let path = if path.starts_with('?') {
        format!("/{path}")
    } else {
        path.to_string()
    };
    host.ends_with(".twitch.tv")
        .then(|| (host.to_string(), path))
}

/// What Twitch's close code means.
fn close_reason(payload: &[u8]) -> String {
    let code = (payload.len() >= 2).then(|| u16::from_be_bytes([payload[0], payload[1]]));
    match code {
        Some(4003) => "Twitch closed the unused connection".to_string(),
        Some(4004) => "Twitch's move to a new server timed out".to_string(),
        Some(4007) => "Twitch refused the move to a new server".to_string(),
        Some(4002) => "Twitch didn't hear back from us in time".to_string(),
        _ => "Twitch closed the connection".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Value {
        json::parse(s).unwrap()
    }

    /// A broken welcome used to panic (`1e300` overflows `Duration`) and
    /// end the task, so no Twitch event arrived until a restart.
    #[test]
    fn a_welcome_keepalive_is_held_to_twitchs_range() {
        let welcome = |seconds: &str| {
            parse(&format!(
                r#"{{"payload":{{"session":{{"keepalive_timeout_seconds":{seconds}}}}}}}"#
            ))
        };
        let secs = |s: u64| Some(Duration::from_secs(s));
        assert_eq!(keepalive_of(&welcome("30")), secs(30));
        assert_eq!(keepalive_of(&welcome("1e300")), secs(600));
        assert_eq!(keepalive_of(&welcome("0")), secs(10));
        assert_eq!(keepalive_of(&welcome("-5")), secs(10));
        assert_eq!(keepalive_of(&welcome("null")), None);
    }

    #[test]
    fn every_twitch_event_has_a_subscription_and_back() {
        for kind in EventKind::ALL {
            let twitch = kind.group() == "Twitch";
            assert_eq!(subscription(kind).is_some(), twitch, "{kind:?}");
        }
        // A new login asks for every permission these events need.
        let asked: Vec<&str> = super::super::auth::MAIN_SCOPES.split(' ').collect();
        for kind in EventKind::ALL {
            if let Some((_, _, scope)) = subscription(kind).filter(|(.., s)| !s.is_empty()) {
                assert!(asked.contains(&scope), "{scope}");
            }
        }
    }

    #[test]
    fn notifications_become_events_with_their_values() {
        let gift = to_event(
            "channel.subscription.gift",
            &parse(r#"{"user_name":null,"total":5,"tier":"2000","cumulative_total":null,"is_anonymous":true}"#),
        )
        .unwrap();
        assert_eq!(gift.kind, EventKind::TwitchGift);
        assert_eq!(gift.var("user"), Some("Anonymous"));
        assert_eq!(
            (gift.var("count"), gift.var("tier"), gift.var("total")),
            (Some("5"), Some("2"), Some("0"))
        );

        let raid = to_event(
            "channel.raid",
            &parse(r#"{"from_broadcaster_user_name":"Ana","viewers":42}"#),
        )
        .unwrap();
        assert_eq!(
            (raid.var("user"), raid.var("viewers")),
            (Some("Ana"), Some("42"))
        );

        let redeem = to_event(
            "channel.channel_points_custom_reward_redemption.add",
            &parse(
                r#"{"user_name":"Bo","user_input":"hi","reward":{"title":"Hydrate","cost":500}}"#,
            ),
        )
        .unwrap();
        assert_eq!(
            (
                redeem.var("reward"),
                redeem.var("cost"),
                redeem.var("input")
            ),
            (Some("Hydrate"), Some("500"), Some("hi"))
        );

        let resub = to_event(
            "channel.subscription.message",
            &parse(r#"{"user_name":"Cy","tier":"1000","cumulative_months":13,"streak_months":null,"message":{"text":"gg"}}"#),
        )
        .unwrap();
        assert_eq!(
            (
                resub.var("months"),
                resub.var("streak"),
                resub.var("message")
            ),
            (Some("13"), Some("0"), Some("gg"))
        );
        assert!(to_event("channel.unknown", &parse("{}")).is_none());
    }

    /// Each event's variables match what its kind declares, so the editor's
    /// samples and the real thing agree.
    #[test]
    fn real_events_carry_the_declared_variables() {
        for kind in EventKind::ALL {
            let Some((kind_type, ..)) = subscription(kind) else {
                continue;
            };
            let event = to_event(kind_type, &parse("{}")).unwrap();
            let mut real: Vec<&str> = event.vars.iter().map(|(n, _)| *n).collect();
            let mut declared: Vec<&str> = kind.vars().iter().map(|(n, _)| *n).collect();
            real.sort_unstable();
            declared.sort_unstable();
            assert_eq!(real, declared, "{kind:?}");
        }
    }

    #[test]
    fn a_cheer_alert_gets_the_words_without_the_cheers() {
        assert_eq!(without_cheers("Cheer100 gg Kappa50  wp"), "gg wp");
        // Letters then a number read as a cheer: rare in a message.
        assert_eq!(without_cheers("gg2 wp b100x 100"), "wp b100x 100");
        assert_eq!(without_cheers("2024 Cheer"), "2024 Cheer");
        assert_eq!(without_cheers("Cheer500"), "");
    }

    #[test]
    fn conditions_name_the_channel_the_way_each_type_wants() {
        assert!(condition("channel.raid", "7")
            .get("to_broadcaster_user_id")
            .is_some());
        let follow = condition("channel.follow", "7");
        assert_eq!(follow.str_or("moderator_user_id", ""), "7");
        assert_eq!(
            condition("channel.cheer", "7").str_or("broadcaster_user_id", ""),
            "7"
        );
    }

    /// A Twitch with no account, for following messages.
    fn twitch_for_test(dir: &crate::test_support::TestDir) -> Arc<Twitch> {
        let (chat, _) = mpsc::channel(1);
        let (events, _) = mpsc::channel(1);
        let store = Arc::new(crate::integrations::store::Store::open(dir.path()));
        Arc::new(Twitch::new(store, (chat, events), Box::new(|_| {}), ""))
    }

    /// Twitch may deliver a notification twice: it reaches the integrations
    /// once. A move names the address to follow.
    #[tokio::test]
    async fn a_notification_is_delivered_once_and_a_move_names_its_address() {
        let dir = crate::test_support::TestDir::new("eventsub");
        let twitch = twitch_for_test(&dir);
        let (tx, mut rx) = mpsc::channel(8);
        let mut seen = VecDeque::new();
        let raid = parse(
            r#"{"metadata":{"message_type":"notification","message_id":"m1"},"payload":{"subscription":{"type":"channel.raid"},"event":{"from_broadcaster_user_name":"Ana","viewers":3}}}"#,
        );
        assert_eq!(take(&twitch, &raid, &mut seen, &tx).await, None);
        assert_eq!(take(&twitch, &raid, &mut seen, &tx).await, None);
        assert_eq!(rx.try_recv().expect("delivered").var("user"), Some("Ana"));
        assert!(rx.try_recv().is_err(), "the repeat was dropped");

        let moved = parse(
            r#"{"metadata":{"message_type":"session_reconnect"},"payload":{"session":{"reconnect_url":"wss://x.twitch.tv/ws?id=1"}}}"#,
        );
        assert_eq!(
            take(&twitch, &moved, &mut seen, &tx).await.as_deref(),
            Some("wss://x.twitch.tv/ws?id=1")
        );
    }

    /// A permission taken back asks the streamer to connect Twitch again,
    /// on that event's line in the dashboard.
    #[tokio::test]
    async fn a_revoked_permission_asks_to_connect_again() {
        let dir = crate::test_support::TestDir::new("eventsub-revoked");
        let twitch = twitch_for_test(&dir);
        twitch.set_event_status(|s| {
            s.subs.insert("twitch_raid", SubState::Listening);
        });
        let (tx, _rx) = mpsc::channel(8);
        let revoked = parse(
            r#"{"metadata":{"message_type":"revocation"},"payload":{"subscription":{"type":"channel.raid","status":"authorization_revoked"}}}"#,
        );
        assert_eq!(
            take(&twitch, &revoked, &mut VecDeque::new(), &tx).await,
            None
        );
        assert_eq!(
            twitch.event_status.lock().subs.get("twitch_raid"),
            Some(&SubState::NeedsConsent)
        );
    }

    #[test]
    fn reconnect_addresses_split_into_host_and_path() {
        assert_eq!(
            split_url("wss://eventsub.wss.twitch.tv?id=abc"),
            Some(("eventsub.wss.twitch.tv".into(), "/?id=abc".into()))
        );
        assert_eq!(
            split_url("wss://e.twitch.tv/ws?x=1"),
            Some(("e.twitch.tv".into(), "/ws?x=1".into()))
        );
        assert_eq!(split_url("https://evil.example"), None);
        assert_eq!(split_url("wss://evil.example/ws"), None, "only Twitch");
        assert_eq!(split_url("wss://host:1234/ws"), None, "only port 443");
    }
}
