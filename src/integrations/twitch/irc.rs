//! Twitch chat over IRC (TLS, port 6697).
//!
//! IRC is line-based and runs over the TLS stack the crate already links,
//! so chat needs no websocket dependency. One connection per account:
//! the streamer's reads chat and posts, a bot account only posts.
//!
//! The connection lives until the stop signal fires. It reconnects on its
//! own with a growing backoff, answers PINGs, and paces sends under
//! Twitch's limit so a busy integration never gets the account muted.

use crate::sync::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};

const HOST: &str = "irc.chat.twitch.tv";
const PORT: u16 = 6697;
/// Twitch's limit for a regular account: 20 messages per 30 seconds.
const SEND_WINDOW: Duration = Duration::from_secs(30);
const SEND_LIMIT: usize = 20;
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// A connection that stays up this long resets the backoff.
const STABLE: Duration = Duration::from_secs(60);
/// No line from Twitch for this long means the socket is dead.
const READ_TIMEOUT: Duration = Duration::from_secs(360);

/// A chat message as integrations see it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatMessage {
    pub id: String,
    pub user_login: String,
    pub display_name: String,
    pub text: String,
    pub is_broadcaster: bool,
    pub is_mod: bool,
    pub is_vip: bool,
    pub is_sub: bool,
    /// Sent in a partner channel during a Shared Chat session: Twitch
    /// shows it here too, tagged with the room it came from.
    pub from_shared_chat: bool,
}

/// A line to post, optionally as a reply. `delivery` hears what became of
/// it: Twitch confirms a posted message (USERSTATE) and says why it refused
/// one (a NOTICE: slow mode, a duplicate, a ban...).
pub struct Outgoing {
    pub text: String,
    pub reply_to: Option<String>,
    pub delivery: Option<std::sync::mpsc::SyncSender<Delivery>>,
}

/// What became of an outgoing message, in the order it happens.
#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Written to the connection, past the rate limit.
    Written,
    Posted,
    /// Twitch refused it; its own words for why.
    Refused(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ChatState {
    #[default]
    Off,
    Connecting,
    Connected,
    /// Twitch refused the token; the manager refreshes it and restarts us.
    AuthFailed,
    Error(String),
}

/// What a connection needs to log in.
#[derive(Clone)]
pub struct Login {
    pub login: String,
    /// Shared with the manager, which swaps in each refreshed token: a
    /// reconnect hours later must not log in with the one we started with.
    pub token: Arc<Mutex<String>>,
    /// Channel to join (the streamer's login).
    pub channel: String,
}

/// Run one account's chat connection until `stop` flips to true.
/// `incoming` is `None` for a send-only (bot) connection.
pub async fn run(
    login: Login,
    incoming: Option<mpsc::Sender<ChatMessage>>,
    mut outgoing: mpsc::Receiver<Outgoing>,
    state: Arc<Mutex<ChatState>>,
    mut stop: watch::Receiver<bool>,
) {
    let mut backoff = Duration::from_secs(1);
    let mut sent: VecDeque<Instant> = VecDeque::new();
    loop {
        if *stop.borrow() {
            break;
        }
        *state.lock() = ChatState::Connecting;
        let started = Instant::now();
        let result = session(
            &login,
            incoming.as_ref(),
            &mut outgoing,
            &state,
            &mut stop,
            &mut sent,
        )
        .await;
        if *stop.borrow() {
            break;
        }
        match result {
            Err(SessionEnd::AuthFailed) => {
                *state.lock() = ChatState::AuthFailed;
                return;
            }
            Err(SessionEnd::Io(e)) => *state.lock() = ChatState::Error(e),
            Ok(()) => {}
        }
        if started.elapsed() >= STABLE {
            backoff = Duration::from_secs(1);
        }
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = stop.changed() => {}
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
    // No state write here: whoever stopped us owns the state now.
}

enum SessionEnd {
    AuthFailed,
    Io(String),
}

async fn session(
    login: &Login,
    incoming: Option<&mpsc::Sender<ChatMessage>>,
    outgoing: &mut mpsc::Receiver<Outgoing>,
    state: &Arc<Mutex<ChatState>>,
    stop: &mut watch::Receiver<bool>,
    sent: &mut VecDeque<Instant>,
) -> Result<(), SessionEnd> {
    let io = |e: std::io::Error| SessionEnd::Io(e.to_string());
    // Messages written and not yet confirmed or refused, oldest first:
    // Twitch answers each in the order it was sent.
    let mut awaiting: VecDeque<std::sync::mpsc::SyncSender<Delivery>> = VecDeque::new();
    let tcp = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect((HOST, PORT)))
        .await
        .map_err(|_| SessionEnd::Io("Twitch chat did not answer".to_string()))?
        .map_err(io)?;
    let connector = native_tls::TlsConnector::new().map_err(|e| SessionEnd::Io(e.to_string()))?;
    let tls = tokio_native_tls::TlsConnector::from(connector)
        .connect(HOST, tcp)
        .await
        .map_err(|e| SessionEnd::Io(format!("TLS: {e}")))?;
    let (reader, mut writer) = tokio::io::split(tls);
    let mut lines = BufReader::new(reader).lines();

    let channel = login.channel.to_lowercase();
    let hello = format!(
        "CAP REQ :twitch.tv/tags twitch.tv/commands\r\nPASS oauth:{}\r\nNICK {}\r\nJOIN #{}\r\n",
        login.token.lock(),
        login.login.to_lowercase(),
        channel
    );
    writer.write_all(hello.as_bytes()).await.map_err(io)?;

    loop {
        tokio::select! {
            line = tokio::time::timeout(READ_TIMEOUT, lines.next_line()) => {
                let line = match line {
                    Ok(Ok(Some(line))) => line,
                    Ok(Ok(None)) => return Err(SessionEnd::Io("Twitch closed the chat connection".into())),
                    Ok(Err(e)) => return Err(io(e)),
                    Err(_) => return Err(SessionEnd::Io("chat went quiet; reconnecting".into())),
                };
                let msg = parse_line(&line);
                match msg.command {
                    "PING" => {
                        let pong = format!("PONG :{}\r\n", msg.trailing);
                        writer.write_all(pong.as_bytes()).await.map_err(io)?;
                    }
                    "001" => *state.lock() = ChatState::Connected,
                    "NOTICE" if is_auth_failure(msg.trailing) => return Err(SessionEnd::AuthFailed),
                    "RECONNECT" => return Ok(()),
                    "USERSTATE" | "NOTICE" => {
                        if let Some(outcome) = delivery_for(&msg) {
                            if let Some(waiting) = awaiting.pop_front() {
                                let _ = waiting.try_send(outcome);
                            }
                        }
                    }
                    "PRIVMSG" => {
                        if let (Some(tx), Some(chat)) = (incoming, chat_message(&msg)) {
                            // A full queue drops the message: chat keeps
                            // moving and a stalled engine must not stall it.
                            let _ = tx.try_send(chat);
                        }
                    }
                    _ => {}
                }
            }
            out = outgoing.recv() => {
                let Some(out) = out else { return Ok(()) };
                pace(sent).await;
                let line = privmsg_line(&channel, &out);
                writer.write_all(line.as_bytes()).await.map_err(io)?;
                if let Some(delivery) = out.delivery {
                    let _ = delivery.try_send(Delivery::Written);
                    awaiting.push_back(delivery);
                }
            }
            _ = stop.changed() => return Ok(()),
        }
    }
}

/// Wait until one more message fits in Twitch's rate window.
async fn pace(sent: &mut VecDeque<Instant>) {
    loop {
        while sent.front().is_some_and(|t| t.elapsed() >= SEND_WINDOW) {
            sent.pop_front();
        }
        if sent.len() < SEND_LIMIT {
            sent.push_back(Instant::now());
            return;
        }
        let oldest = *sent.front().expect("window is full");
        tokio::time::sleep(SEND_WINDOW.saturating_sub(oldest.elapsed())).await;
    }
}

fn privmsg_line(channel: &str, out: &Outgoing) -> String {
    let text: String = out
        .text
        .chars()
        .filter(|c| *c != '\r' && *c != '\n')
        .collect();
    match &out.reply_to {
        Some(id) if valid_msg_id(id) => {
            format!("@reply-parent-msg-id={id} PRIVMSG #{channel} :{text}\r\n")
        }
        _ => format!("PRIVMSG #{channel} :{text}\r\n"),
    }
}

fn valid_msg_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

/// The answer to a message we sent, if this line is one: USERSTATE follows
/// a posted message, and a NOTICE with a `msg_*` id refuses one.
fn delivery_for(line: &Line) -> Option<Delivery> {
    match line.command {
        "USERSTATE" => Some(Delivery::Posted),
        "NOTICE" if line.tag("msg-id").starts_with("msg_") => {
            Some(Delivery::Refused(line.trailing.to_string()))
        }
        _ => None,
    }
}

fn is_auth_failure(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("login authentication failed") || t.contains("improperly formatted auth")
}

/// One IRC line, split into its parts. Borrowed from the line.
#[derive(Debug, Default, PartialEq)]
pub struct Line<'a> {
    pub tags: Vec<(&'a str, String)>,
    pub nick: &'a str,
    pub command: &'a str,
    pub target: &'a str,
    pub trailing: &'a str,
}

impl Line<'_> {
    fn tag(&self, name: &str) -> &str {
        self.tags
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    }
}

pub fn parse_line(line: &str) -> Line<'_> {
    let mut rest = line.trim_end_matches(['\r', '\n']);
    let mut out = Line::default();
    if let Some(tagged) = rest.strip_prefix('@') {
        let (tags, after) = tagged.split_once(' ').unwrap_or((tagged, ""));
        out.tags = tags
            .split(';')
            .filter_map(|kv| {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                (!k.is_empty()).then(|| (k, unescape_tag(v)))
            })
            .collect();
        rest = after;
    }
    if let Some(prefixed) = rest.strip_prefix(':') {
        let (prefix, after) = prefixed.split_once(' ').unwrap_or((prefixed, ""));
        out.nick = prefix.split('!').next().unwrap_or("");
        rest = after;
    }
    let (head, trailing) = match rest.split_once(" :") {
        Some((head, trailing)) => (head, trailing),
        None => (rest, ""),
    };
    let mut parts = head.split(' ').filter(|p| !p.is_empty());
    out.command = parts.next().unwrap_or("");
    out.target = parts.next().unwrap_or("");
    out.trailing = trailing;
    if out.command == "PING" && out.trailing.is_empty() {
        out.trailing = out.target;
    }
    out
}

/// IRCv3 tag value escapes.
fn unescape_tag(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(':') => out.push(';'),
            Some('s') => out.push(' '),
            Some('r') => out.push('\r'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn chat_message(line: &Line) -> Option<ChatMessage> {
    if line.trailing.is_empty() {
        return None;
    }
    let badges = line.tag("badges");
    let has_badge = |name: &str| badges.split(',').any(|b| b.split('/').next() == Some(name));
    let display = line.tag("display-name");
    // `/me` arrives as a CTCP ACTION; integrations see the text.
    let text = line
        .trailing
        .strip_prefix("\u{1}ACTION ")
        .map(|t| t.trim_end_matches('\u{1}'))
        .unwrap_or(line.trailing);
    Some(ChatMessage {
        id: line.tag("id").to_string(),
        user_login: line.nick.to_string(),
        display_name: if display.is_empty() {
            line.nick.to_string()
        } else {
            display.to_string()
        },
        text: text.to_string(),
        is_broadcaster: has_badge("broadcaster"),
        is_mod: line.tag("mod") == "1" || has_badge("moderator"),
        is_vip: has_badge("vip") || line.tag("vip") == "1",
        is_sub: line.tag("subscriber") == "1" || has_badge("subscriber") || has_badge("founder"),
        from_shared_chat: {
            let source = line.tag("source-room-id");
            !source.is_empty() && source != line.tag("room-id")
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_tagged_privmsg() {
        let raw = "@badge-info=;badges=moderator/1,subscriber/12;display-name=Ana\\sB;id=b34ccfc7-4977-403a-8a94-33c6bac34fb8;mod=1;subscriber=1 :ana!ana@ana.tmi.twitch.tv PRIVMSG #texaz :!delay please\r\n";
        let line = parse_line(raw);
        assert_eq!(line.command, "PRIVMSG");
        assert_eq!(line.target, "#texaz");
        let msg = chat_message(&line).unwrap();
        assert_eq!(msg.display_name, "Ana B");
        assert_eq!(msg.user_login, "ana");
        assert_eq!(msg.text, "!delay please");
        assert!(msg.is_mod && msg.is_sub && !msg.is_vip && !msg.is_broadcaster);
        assert_eq!(msg.id, "b34ccfc7-4977-403a-8a94-33c6bac34fb8");
    }

    #[test]
    fn notices_messages_from_a_shared_chat_partner() {
        let partner = "@badges=;display-name=Bo;room-id=111;source-room-id=222 :bo!bo@bo.tmi.twitch.tv PRIVMSG #texaz :!delay";
        assert!(chat_message(&parse_line(partner)).unwrap().from_shared_chat);
        let ours = "@badges=;display-name=Al;room-id=111;source-room-id=111 :al!al@al.tmi.twitch.tv PRIVMSG #texaz :!delay";
        assert!(!chat_message(&parse_line(ours)).unwrap().from_shared_chat);
        let plain =
            "@badges=;display-name=Al;room-id=111 :al!al@al.tmi.twitch.tv PRIVMSG #texaz :!delay";
        assert!(!chat_message(&parse_line(plain)).unwrap().from_shared_chat);
    }

    #[test]
    fn reads_me_actions_and_broadcaster_badges() {
        let raw = "@badges=broadcaster/1;display-name=Texaz :texaz!texaz@texaz.tmi.twitch.tv PRIVMSG #texaz :\u{1}ACTION waves\u{1}";
        let msg = chat_message(&parse_line(raw)).unwrap();
        assert_eq!(msg.text, "waves");
        assert!(msg.is_broadcaster);
    }

    #[test]
    fn parses_ping_in_both_shapes() {
        assert_eq!(parse_line("PING :tmi.twitch.tv").trailing, "tmi.twitch.tv");
        assert_eq!(parse_line("PING tmi.twitch.tv").trailing, "tmi.twitch.tv");
    }

    #[test]
    fn spots_auth_failures() {
        let line = parse_line(":tmi.twitch.tv NOTICE * :Login authentication failed");
        assert_eq!(line.command, "NOTICE");
        assert!(is_auth_failure(line.trailing));
    }

    #[test]
    fn twitch_answers_say_whether_a_message_was_posted() {
        let posted = parse_line("@badges=;mod=0 :tmi.twitch.tv USERSTATE #texaz");
        assert_eq!(delivery_for(&posted), Some(Delivery::Posted));
        let slow = parse_line(
            "@msg-id=msg_slowmode :tmi.twitch.tv NOTICE #texaz :This room is in slow mode.",
        );
        assert_eq!(
            delivery_for(&slow),
            Some(Delivery::Refused("This room is in slow mode.".into()))
        );
        let other = parse_line("@msg-id=host_on :tmi.twitch.tv NOTICE #texaz :Now hosting.");
        assert_eq!(delivery_for(&other), None, "not about a message");
    }

    #[test]
    fn outgoing_lines_are_single_line_and_reply_safely() {
        let out = Outgoing {
            text: "hi\r\nPRIVMSG #x :spam".into(),
            reply_to: Some("abc-123".into()),
            delivery: None,
        };
        assert_eq!(
            privmsg_line("texaz", &out),
            "@reply-parent-msg-id=abc-123 PRIVMSG #texaz :hiPRIVMSG #x :spam\r\n"
        );
        let hostile = Outgoing {
            text: "x".into(),
            reply_to: Some("1; evil".into()),
            delivery: None,
        };
        assert_eq!(privmsg_line("texaz", &hostile), "PRIVMSG #texaz :x\r\n");
    }

    #[tokio::test]
    async fn pacing_admits_the_limit_then_holds() {
        let mut sent = VecDeque::new();
        for _ in 0..SEND_LIMIT {
            pace(&mut sent).await;
        }
        assert_eq!(sent.len(), SEND_LIMIT);
        let next = tokio::time::timeout(Duration::from_millis(50), pace(&mut sent)).await;
        assert!(next.is_err(), "the 21st message must wait");
    }
}
