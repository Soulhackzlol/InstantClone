//! The Twitch connection: logins, token upkeep, chat, markers and clips.
//!
//! "Log in once and forget it" is this module's job:
//! - Logging in is the Device Code flow (see `auth`), no secret, no server.
//! - Access tokens are refreshed before they run out, and validated hourly
//!   as Twitch asks. A refresh keeps the 30-day unused window from ever
//!   closing while InstantClone is in use.
//! - Refresh tokens are single-use, so the new pair is saved before the old
//!   one could be needed again.
//! - Chat reconnects on its own; a refused token triggers a refresh and a
//!   reconnect instead of a dead chat.
//! - If Twitch really has logged us out, the dashboard says so with a
//!   one-click reconnect. Nothing here ever touches the stream.

pub mod auth;
pub mod helix;
pub mod irc;

use super::store::{Store, TwitchAccount};
use crate::json::{self, Value};
use crate::sync::Mutex;
use irc::{ChatMessage, ChatState, Outgoing};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch, Notify};

/// The app's Twitch client id, baked in at build time. A build without one
/// (a fork, a dev build) can still set it at run time.
const BUILT_IN_CLIENT_ID: Option<&str> = option_env!("INSTANTCLONE_TWITCH_CLIENT_ID");
const CLIENT_ID_ENV: &str = "INSTANTCLONE_TWITCH_CLIENT_ID";

/// Refresh an access token when it has less than this left.
const REFRESH_MARGIN: Duration = Duration::from_secs(20 * 60);
const UPKEEP_EVERY: Duration = Duration::from_secs(5 * 60);
const VALIDATE_EVERY: Duration = Duration::from_secs(55 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Main,
    Bot,
}

impl Which {
    pub fn from_id(id: &str) -> Which {
        if id == "bot" {
            Which::Bot
        } else {
            Which::Main
        }
    }

    fn scopes(self) -> &'static str {
        match self {
            Which::Main => auth::MAIN_SCOPES,
            Which::Bot => auth::BOT_SCOPES,
        }
    }
}

struct LoginFlow {
    which: Which,
    user_code: String,
    uri: String,
    expires_at: Instant,
    error: Option<String>,
    /// Bumped per attempt so a superseded poll loop stops quietly.
    attempt: u64,
}

#[derive(Default)]
struct Conn {
    state: Arc<Mutex<ChatState>>,
    out: Mutex<Option<mpsc::Sender<Outgoing>>>,
    stop: Mutex<Option<watch::Sender<bool>>>,
}

impl Conn {
    fn stop(&self) {
        if let Some(stop) = self.stop.lock().take() {
            let _ = stop.send(true);
        }
        self.out.lock().take();
        *self.state.lock() = ChatState::Off;
    }
}

pub struct Twitch {
    client_id: String,
    store: Arc<Store>,
    login: Mutex<Option<LoginFlow>>,
    login_attempts: std::sync::atomic::AtomicU64,
    main: Conn,
    bot: Conn,
    incoming: mpsc::Sender<ChatMessage>,
    /// Shown on the dashboard when Twitch logged us out.
    notice: Mutex<Option<String>>,
    upkeep_now: Arc<Notify>,
    /// Writes a line to the dashboard log.
    log: Box<dyn Fn(String) + Send + Sync>,
}

impl Twitch {
    pub fn new(
        store: Arc<Store>,
        incoming: mpsc::Sender<ChatMessage>,
        log: Box<dyn Fn(String) + Send + Sync>,
    ) -> Twitch {
        let client_id = std::env::var(CLIENT_ID_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| BUILT_IN_CLIENT_ID.map(str::to_string))
            .unwrap_or_default();
        Twitch {
            client_id,
            store,
            login: Mutex::new(None),
            login_attempts: Default::default(),
            main: Conn::default(),
            bot: Conn::default(),
            incoming,
            notice: Mutex::new(None),
            upkeep_now: Arc::new(Notify::new()),
            log,
        }
    }

    pub fn available(&self) -> bool {
        !self.client_id.is_empty()
    }

    fn conn(&self, which: Which) -> &Conn {
        match which {
            Which::Main => &self.main,
            Which::Bot => &self.bot,
        }
    }

    fn account(&self, which: Which) -> Option<TwitchAccount> {
        let accounts = self.store.accounts.lock();
        match which {
            Which::Main => accounts.main.clone(),
            Which::Bot => accounts.bot.clone(),
        }
    }

    fn set_account(&self, which: Which, account: Option<TwitchAccount>) {
        {
            let mut accounts = self.store.accounts.lock();
            match which {
                Which::Main => accounts.main = account,
                Which::Bot => accounts.bot = account,
            }
        }
        let _ = self.store.save();
    }

    /// The streamer's channel login, or empty.
    pub fn channel(&self) -> String {
        self.account(Which::Main)
            .map(|a| a.login)
            .unwrap_or_default()
    }

    /// Start chat and the token upkeep. Call from the engine's runtime.
    pub fn start(self: &Arc<Self>) {
        self.restart_chat(Which::Main);
        self.restart_chat(Which::Bot);
        let me = self.clone();
        tokio::spawn(async move { me.upkeep().await });
    }

    fn restart_chat(&self, which: Which) {
        let conn = self.conn(which);
        conn.stop();
        let (Some(account), Some(main)) = (self.account(which), self.account(Which::Main)) else {
            return;
        };
        let (out_tx, out_rx) = mpsc::channel(64);
        let (stop_tx, stop_rx) = watch::channel(false);
        *conn.out.lock() = Some(out_tx);
        *conn.stop.lock() = Some(stop_tx);
        let login = irc::Login {
            login: account.login,
            token: account.access,
            channel: main.login,
        };
        let incoming = (which == Which::Main).then(|| self.incoming.clone());
        let state = conn.state.clone();
        let upkeep_now = self.upkeep_now.clone();
        tokio::spawn(async move {
            irc::run(login, incoming, out_rx, state.clone(), stop_rx).await;
            // Twitch refused the token: refresh now rather than leaving chat
            // down until the next upkeep round.
            if *state.lock() == ChatState::AuthFailed {
                upkeep_now.notify_one();
            }
        });
    }

    /// Begin a Device Code login. The dashboard shows the code from
    /// `status_json` and the user enters it on twitch.tv/activate.
    pub fn begin_login(self: &Arc<Self>, which: Which) {
        let attempt = self
            .login_attempts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        *self.login.lock() = Some(LoginFlow {
            which,
            user_code: String::new(),
            uri: String::new(),
            expires_at: Instant::now() + Duration::from_secs(600),
            error: (!self.available()).then(|| "This build has no Twitch app id.".to_string()),
            attempt,
        });
        if !self.available() {
            return;
        }
        let me = self.clone();
        tokio::spawn(async move { me.run_login(which, attempt).await });
    }

    pub fn cancel_login(&self) {
        self.login_attempts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        *self.login.lock() = None;
    }

    fn login_current(&self, attempt: u64) -> bool {
        self.login
            .lock()
            .as_ref()
            .is_some_and(|f| f.attempt == attempt)
    }

    fn login_failed(&self, attempt: u64, error: String) {
        if let Some(flow) = self.login.lock().as_mut().filter(|f| f.attempt == attempt) {
            flow.error = Some(error);
        }
    }

    async fn run_login(self: Arc<Self>, which: Which, attempt: u64) {
        let client_id = self.client_id.clone();
        let started = tokio::task::spawn_blocking({
            let client_id = client_id.clone();
            move || auth::start_device(&client_id, which.scopes())
        })
        .await
        .unwrap_or_else(|_| Err("login crashed".to_string()));
        let code = match started {
            Ok(code) => code,
            Err(e) => return self.login_failed(attempt, e),
        };
        if let Some(flow) = self.login.lock().as_mut().filter(|f| f.attempt == attempt) {
            flow.user_code = code.user_code.clone();
            flow.uri = code.verification_uri.clone();
            flow.expires_at = Instant::now() + Duration::from_secs(code.expires_in);
        }
        let mut interval = Duration::from_secs(code.interval);
        let deadline = Instant::now() + Duration::from_secs(code.expires_in);
        let tokens = loop {
            tokio::time::sleep(interval).await;
            if !self.login_current(attempt) {
                return;
            }
            if Instant::now() >= deadline {
                return self.login_failed(
                    attempt,
                    "The code expired before it was entered. Try again.".into(),
                );
            }
            let (id, device) = (client_id.clone(), code.device_code.clone());
            let poll = tokio::task::spawn_blocking(move || {
                auth::poll_device(&id, which.scopes(), &device)
            })
            .await
            .unwrap_or(auth::Poll::Pending);
            match poll {
                auth::Poll::Pending => {}
                auth::Poll::SlowDown => interval += Duration::from_secs(5),
                auth::Poll::Done(tokens) => break tokens,
                auth::Poll::Ended(e) => return self.login_failed(attempt, e),
            }
        };
        let access = tokens.access.clone();
        let validated = tokio::task::spawn_blocking(move || auth::validate(&access)).await;
        let who = match validated {
            Ok(Ok(v)) => v,
            _ => {
                return self.login_failed(
                    attempt,
                    "Twitch logged you in but would not say who you are. Try again.".into(),
                )
            }
        };
        if !self.login_current(attempt) {
            return;
        }
        self.set_account(
            which,
            Some(TwitchAccount {
                login: who.login,
                user_id: who.user_id,
                access: tokens.access,
                refresh: tokens.refresh,
                expires_at_ms: unix_ms() + tokens.expires_in * 1000,
            }),
        );
        *self.login.lock() = None;
        *self.notice.lock() = None;
        self.restart_chat(which);
        if which == Which::Main {
            // The bot joins the streamer's channel, which may have changed.
            self.restart_chat(Which::Bot);
        }
    }

    /// Forget an account and revoke its token at Twitch.
    pub fn logout(&self, which: Which) {
        self.conn(which).stop();
        if let Some(account) = self.account(which) {
            let client_id = self.client_id.clone();
            std::thread::spawn(move || {
                let form = [
                    ("client_id", client_id.as_str()),
                    ("token", account.access.as_str()),
                ];
                let _ = crate::https::https_agent()
                    .post("https://id.twitch.tv/oauth2/revoke")
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .send(auth::form_encode(&form));
            });
        }
        self.set_account(which, None);
        if which == Which::Main {
            // No channel to join any more.
            self.bot.stop();
        }
    }

    async fn upkeep(self: Arc<Self>) {
        let mut last_validate = Instant::now() - VALIDATE_EVERY;
        loop {
            let validate_now = last_validate.elapsed() >= VALIDATE_EVERY;
            if validate_now {
                last_validate = Instant::now();
            }
            for which in [Which::Main, Which::Bot] {
                self.keep_fresh(which, validate_now).await;
            }
            tokio::select! {
                _ = tokio::time::sleep(UPKEEP_EVERY) => {}
                _ = self.upkeep_now.notified() => {}
            }
        }
    }

    async fn keep_fresh(&self, which: Which, validate: bool) {
        let Some(account) = self.account(which) else {
            return;
        };
        let auth_failed = *self.conn(which).state.lock() == ChatState::AuthFailed;
        let expiring = account.expires_at_ms <= unix_ms() + REFRESH_MARGIN.as_millis() as u64;
        if validate && !expiring && !auth_failed {
            let access = account.access.clone();
            match tokio::task::spawn_blocking(move || auth::validate(&access)).await {
                Ok(Err(auth::TokenError::Revoked)) => {}
                _ => return,
            }
        } else if !expiring && !auth_failed {
            return;
        }
        let (id, refresh) = (self.client_id.clone(), account.refresh.clone());
        let result = tokio::task::spawn_blocking(move || auth::refresh(&id, &refresh))
            .await
            .unwrap_or_else(|_| Err(auth::TokenError::Temporary("refresh crashed".into())));
        match result {
            Ok(tokens) => {
                self.set_account(
                    which,
                    Some(TwitchAccount {
                        access: tokens.access,
                        refresh: tokens.refresh,
                        expires_at_ms: unix_ms() + tokens.expires_in * 1000,
                        ..account
                    }),
                );
                if auth_failed || *self.conn(which).state.lock() == ChatState::Off {
                    self.restart_chat(which);
                }
            }
            Err(auth::TokenError::Revoked) => {
                self.conn(which).stop();
                self.set_account(which, None);
                let who = if which == Which::Main {
                    "your Twitch account"
                } else {
                    "the bot account"
                };
                (self.log)(format!(
                    "[twitch] Twitch logged out {who}; connect it again in Integrations"
                ));
                *self.notice.lock() = Some(format!(
                    "Twitch logged out {who} (it was revoked, or InstantClone went unused for 30 days). Connect it again."
                ));
            }
            // Offline or Twitch hiccup: the next upkeep round tries again.
            Err(auth::TokenError::Temporary(e)) => {
                (self.log)(format!(
                    "[twitch] couldn't refresh the login yet, will retry: {e}"
                ));
            }
        }
    }

    /// Ask the upkeep loop to check tokens now (after a refused call).
    pub fn check_tokens_soon(&self) {
        self.upkeep_now.notify_one();
    }

    /// Queue a chat message. Blocking-safe: only a channel send.
    pub fn send(&self, text: &str, as_bot: bool, reply_to: Option<&str>) -> Result<(), String> {
        let connected = |c: &Conn| *c.state.lock() == ChatState::Connected;
        let conn = if as_bot && connected(&self.bot) {
            &self.bot
        } else {
            &self.main
        };
        if !connected(conn) {
            return Err(match self.account(Which::Main) {
                None => "connect Twitch first".to_string(),
                Some(_) => "Twitch chat is reconnecting; try again in a moment".to_string(),
            });
        }
        let out = conn
            .out
            .lock()
            .clone()
            .ok_or("Twitch chat is not running")?;
        out.try_send(Outgoing {
            text: text.to_string(),
            reply_to: reply_to.map(str::to_string),
        })
        .map_err(|_| "too many chat messages queued; slow down".to_string())
    }

    fn helix_call<T>(
        &self,
        call: impl FnOnce(&helix::Auth) -> Result<T, String>,
    ) -> Result<T, String> {
        let account = self.account(Which::Main).ok_or("connect Twitch first")?;
        let auth = helix::Auth {
            client_id: &self.client_id,
            token: &account.access,
            user_id: &account.user_id,
        };
        let result = call(&auth);
        if matches!(&result, Err(e) if e.contains("log in to Twitch again")) {
            self.check_tokens_soon();
        }
        result
    }

    pub fn marker(&self, description: &str) -> Result<(), String> {
        self.helix_call(|a| helix::create_marker(a, description))
    }

    pub fn clip(&self) -> Result<String, String> {
        self.helix_call(helix::create_clip)
    }

    pub fn status_json(&self) -> Value {
        let conn_json = |which: Which| {
            let account = self.account(which);
            let state = match &*self.conn(which).state.lock() {
                _ if account.is_none() => "off".to_string(),
                ChatState::Off => "off".to_string(),
                ChatState::Connecting => "connecting".to_string(),
                ChatState::Connected => "connected".to_string(),
                ChatState::AuthFailed => "reconnecting".to_string(),
                ChatState::Error(e) => format!("error: {e}"),
            };
            json::obj([
                (
                    "login",
                    json::str(account.map(|a| a.login).unwrap_or_default()),
                ),
                ("chat", json::str(state)),
            ])
        };
        let flow = match &*self.login.lock() {
            Some(f) => json::obj([
                (
                    "which",
                    json::str(if f.which == Which::Bot { "bot" } else { "main" }),
                ),
                ("user_code", json::str(&f.user_code)),
                ("uri", json::str(&f.uri)),
                (
                    "expires_in_s",
                    Value::Num(
                        f.expires_at
                            .saturating_duration_since(Instant::now())
                            .as_secs() as f64,
                    ),
                ),
                ("error", f.error.as_deref().map_or(Value::Null, json::str)),
            ]),
            None => Value::Null,
        };
        json::obj([
            ("available", Value::Bool(self.available())),
            ("main", conn_json(Which::Main)),
            ("bot", conn_json(Which::Bot)),
            ("login", flow),
            (
                "notice",
                self.notice.lock().as_deref().map_or(Value::Null, json::str),
            ),
        ])
    }
}

pub fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
