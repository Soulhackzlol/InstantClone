//! Built-in web UI - first-run wizard, dashboard, settings, overlay, and
//! a small JSON state/config API. Hand-rolled HTTP/1.1 so we don't pull
//! hyper/axum into the RAM budget.
//!
//! Routes
//!     GET  /              - wizard (when !configured) or dashboard
//!     GET  /overlay       - OBS browser-source overlay
//!     GET  /state         - live JSON (delay, fill, alive, stats)
//!     GET  /config        - current settings (stream key NOT echoed)
//!     POST /config        - apply settings (form-encoded)
//!     POST /delay         - ms=NNN, sets target delay live
//!     POST /go-live       - convenience for delay=0
//!     POST /test-egress   - TCP-tests the configured platform endpoint
//!     GET  /platforms     - list of supported platforms (for UI dropdown)

use crate::config::{self, Settings};
use crate::controller::Controller;
use crate::rtmp::client::EgressUrl;
use crate::sysstat::SysStat;
use std::io;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

/// Serializes read-modify-write cycles on the single per-process settings
/// channel. Each mutating handler does `settings.borrow().clone()` -> mutate
/// -> `settings.send(whole_struct)`; a connection runs per task, so without
/// this lock two overlapping POSTs each start from the same snapshot and the
/// later `send()` clobbers the other's change. That lost update used to
/// silently reset the `configured` flag and bounce users into the first-run
/// wizard. This is the one lock that deliberately keeps `std`'s poison
/// serializes the read-modify-write-send cycle so concurrent POSTs can't
/// clobber each other. Uses `crate::sync::Mutex` like the rest of the codebase:
/// under `panic = "abort"` a poisoned lock is unreachable in release, and the
/// fail-fast path in debug is the right signal (see `crate::sync`). The
/// critical section never `.await`s (the `!Send` guard makes that a compile
/// error), so it is safe to take from the sync persist/overlay handlers.
static SETTINGS_WRITE_LOCK: crate::sync::Mutex<()> = crate::sync::Mutex::new(());

/// Take the settings write lock for a full read-modify-write-send cycle. Hold
/// the guard from just before `settings.borrow().clone()` until after
/// `settings.send(..)`.
pub(crate) fn settings_write_guard() -> std::sync::MutexGuard<'static, ()> {
    SETTINGS_WRITE_LOCK.lock()
}

/// True when at least one destination is enabled and fully addressable (a
/// resolvable egress URL with a stream key). This is the "ready to stream"
/// condition first-run setup waits for, and the only thing that raises the
/// `configured` latch. It never lowers it: once setup is complete, turning
/// the last destination off or deleting it keeps the user on the dashboard
/// rather than reopening the wizard - only an explicit full reset clears the
/// flag. See `post_destination_toggle` / `post_destination_delete`.
fn has_streamable_dest(s: &Settings) -> bool {
    s.destinations
        .iter()
        .any(|d| d.enabled && d.is_well_formed())
}

pub async fn run(
    addr: String,
    ctrl: Arc<Controller>,
    settings: Arc<watch::Sender<Settings>>,
    cfg_path: PathBuf,
    auth: Arc<crate::auth::AuthState>,
) -> io::Result<()> {
    let listener = TcpListener::bind(&addr).await?;
    // Don't let a restart/self-update child inherit this listener, or the port
    // stays bound after we exit and the new instance can't reclaim it.
    crate::self_update::dont_inherit(&listener);
    eprintln!("[web] listening on http://{}", addr);
    // Single shared sampler: CPU% needs the previous sample to compute a
    // delta, so we cannot construct one per request.
    let sysstat = Arc::new(SysStat::new());
    loop {
        let (sock, peer) = listener.accept().await?;
        let ctrl = ctrl.clone();
        let settings = settings.clone();
        let cfg_path = cfg_path.clone();
        let sysstat = sysstat.clone();
        let auth = auth.clone();
        // Peer IP keys the login rate limiter. Behind a reverse proxy every
        // request shares the proxy's IP, which just makes the limit global -
        // safe, since a spoofed X-Forwarded-For must never relax it.
        let peer_ip = peer.ip().to_string();
        tokio::spawn(async move {
            let _ = serve(sock, ctrl, settings, cfg_path, sysstat, auth, peer_ip).await;
        });
    }
}

async fn serve(
    mut sock: TcpStream,
    ctrl: Arc<Controller>,
    settings: Arc<watch::Sender<Settings>>,
    cfg_path: PathBuf,
    sysstat: Arc<SysStat>,
    auth: Arc<crate::auth::AuthState>,
    peer_ip: String,
) -> io::Result<()> {
    // Read until the headers terminator. For our POST bodies (config form,
    // <2 KB) this single read is enough - but be defensive about partials.
    let mut buf = vec![0u8; 16 * 1024];
    let mut used = 0usize;
    let head_end;
    // Total deadline for the whole request-head read, so a slowloris client
    // dripping one byte at a time can't pin a connection (and its buffers / FD)
    // open forever. A deadline bounds the aggregate, unlike a per-read timeout.
    let head_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let n = match tokio::time::timeout_at(head_deadline, sock.read(&mut buf[used..])).await {
            Ok(r) => r?,
            Err(_) => return Ok(()), // headers took too long; drop
        };
        if n == 0 {
            return Ok(());
        }
        used += n;
        if let Some(idx) = find_subslice(&buf[..used], b"\r\n\r\n") {
            head_end = idx + 4;
            break;
        }
        if used >= buf.len() {
            // Header section larger than our buffer - refuse politely
            // instead of dropping the TCP connection without explanation.
            let _ = sock
                .write_all(
                    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            return Ok(());
        }
    }

    let head_str = std::str::from_utf8(&buf[..head_end]).unwrap_or("");
    let (method, path, content_length) = parse_request_head(head_str);
    // A body we cannot read honestly. Answering 400 keeps a malformed request
    // from being run as a well-formed one with no arguments - which for
    // `POST /arm` would mean dropping a live delay. See `parse_request_head`.
    let Some(content_length) = content_length else {
        let body =
            r#"{"ok":false,"error":"malformed Content-Length or unsupported Transfer-Encoding"}"#;
        write_simple(&mut sock, "400 Bad Request", "application/json", body, "").await?;
        return Ok(());
    };
    let (origin, host) = parse_origin_host(head_str);
    let accept_gzip = accepts_gzip(head_str);

    // CSRF guard - block cross-origin browser POSTs. See `allow_csrf`
    // for the policy. Pre-flight OPTIONS gets a generic 204 so browsers
    // doing a CORS preflight don't see this as a hard reject.
    if method == "OPTIONS" {
        let r = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\n\
                 Access-Control-Allow-Methods: GET, POST\r\n\
                 Access-Control-Allow-Headers: content-type\r\nConnection: close\r\n\r\n";
        sock.write_all(r.as_bytes()).await?;
        return Ok(());
    }
    if !allow_csrf(method, &origin, &host) {
        let body = r#"{"ok":false,"error":"cross-origin POSTs are blocked (CSRF guard)"}"#;
        let r = format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        sock.write_all(r.as_bytes()).await?;
        return Ok(());
    }

    // Strip the query string once for ALL fast-path matches. Without this,
    // `GET /?utm=x` (browsers do this automatically when arriving from a
    // shared link) would fall through to the slow path and 404, because
    // `path` here still carries the `?...` suffix.
    let bare_path = path.split_once('?').map(|(p, _)| p).unwrap_or(path);

    // Request body: only POST routes (the config form, login, the auth
    // mutations) consume one, so we read a body for POST alone. A GET or HEAD
    // that advertises a large Content-Length therefore never makes us allocate
    // or block reading a body it has no business sending - it is served (or
    // gated) straight from the head we already have.
    const MAX_BODY: usize = 32 * 1024 * 1024;
    let body_buf = if method == "POST" {
        if content_length > MAX_BODY {
            let body = br#"{"ok":false,"error":"body too large (max 32 MB)"}"# as &[u8];
            let r = format!(
                "HTTP/1.1 413 Payload Too Large\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(r.as_bytes()).await?;
            sock.write_all(body).await?;
            return Ok(());
        }
        let mut b = buf[head_end..used].to_vec();
        // Same idea as the header deadline: bound the whole body read so a
        // client advertising a large Content-Length can't feed it one slow
        // byte at a time.
        let body_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        while b.len() < content_length {
            let mut tmp = [0u8; 4096];
            let n = match tokio::time::timeout_at(body_deadline, sock.read(&mut tmp)).await {
                Ok(r) => r?,
                Err(_) => break, // body took too long; refused below
            };
            if n == 0 {
                break;
            }
            b.extend_from_slice(&tmp[..n]);
        }
        // The client stopped (EOF or the deadline) before the body it
        // announced. Running the route on the fragment would read a cut-off
        // `ms=30000` as `ms=3` and arm that, so refuse instead.
        if b.len() < content_length {
            let body = r#"{"ok":false,"error":"request body shorter than its Content-Length"}"#;
            write_simple(&mut sock, "400 Bad Request", "application/json", body, "").await?;
            return Ok(());
        }
        // Drop any pipelined bytes past this request (we always Connection:
        // close, so there is no next request on this socket anyway).
        b.truncate(content_length);
        b
    } else {
        Vec::new()
    };
    // Same reasoning as the Content-Length refusal: reading a body that is not
    // UTF-8 as "" would run `POST /arm` with no `ms`, which is a disarm.
    let Ok(body) = std::str::from_utf8(&body_buf) else {
        let body = r#"{"ok":false,"error":"request body is not valid UTF-8"}"#;
        write_simple(&mut sock, "400 Bad Request", "application/json", body, "").await?;
        return Ok(());
    };

    // Optional dashboard auth. Off by default (a single is_empty() inside),
    // fail-closed once a password is set. The whole security-critical surface
    // lives in one auditable function; a `Handled` result means it already
    // wrote a response (login page, 401, redirect, an /auth/* mutation).
    let (dock_set_cookie, is_admin) = match auth_gate(
        &mut sock, method, path, bare_path, head_str, body, &settings, &cfg_path, &auth, &peer_ip,
    )
    .await?
    {
        AuthDecision::Handled => return Ok(()),
        AuthDecision::Allow { cookie, is_admin } => (cookie, is_admin),
    };

    // Fast-path: static, pre-gzipped dashboard + dock. These two pages
    // dominate the binary (~125 KB raw); shipping only the gz blob saves
    // ~100 KB. Every real browser sends `Accept-Encoding: gzip`; for the
    // rare client that doesn't we return 406 with a friendly hint rather
    // than ship a runtime inflater that would erase the savings.
    if method == "GET" && (bare_path == "/" || bare_path == "/dock") {
        if !accept_gzip {
            let body = b"this build serves gzip-encoded HTML; \
                         retry with Accept-Encoding: gzip" as &[u8];
            let r = format!(
                "HTTP/1.1 406 Not Acceptable\r\nContent-Type: text/plain; charset=utf-8\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(r.as_bytes()).await?;
            sock.write_all(body).await?;
            return Ok(());
        }
        let blob: &'static [u8] = if bare_path == "/" {
            INDEX_HTML_GZ
        } else {
            DOCK_HTML_GZ
        };
        let r = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
             Content-Encoding: gzip\r\nVary: Accept-Encoding\r\n\
             Content-Length: {}\r\n\
             Access-Control-Allow-Origin: *\r\n{}Cache-Control: no-store\r\nConnection: close\r\n\r\n",
            blob.len(),
            dock_set_cookie
        );
        sock.write_all(r.as_bytes()).await?;
        sock.write_all(blob).await?;
        return Ok(());
    }

    // Overlay Studio runtime - static pre-gzipped JS, same fast-path as
    // the dashboard. Served to the dashboard tab only; baked overlays
    // inline what they need and never request this.
    if method == "GET"
        && (bare_path == "/overlay-runtime.js"
            || bare_path == "/dock.js"
            || bare_path == "/integrations.js")
    {
        if !accept_gzip {
            let body = b"this build serves gzip-encoded JS; \
                         retry with Accept-Encoding: gzip" as &[u8];
            let r = format!(
                "HTTP/1.1 406 Not Acceptable\r\nContent-Type: text/plain; charset=utf-8\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(r.as_bytes()).await?;
            sock.write_all(body).await?;
            return Ok(());
        }
        let blob: &'static [u8] = match bare_path {
            "/dock.js" => DOCK_JS_GZ,
            "/integrations.js" => INTEGRATIONS_JS_GZ,
            _ => OVERLAY_RUNTIME_JS_GZ,
        };
        let r = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/javascript; charset=utf-8\r\n\
             Content-Encoding: gzip\r\nVary: Accept-Encoding\r\n\
             Content-Length: {}\r\n\
             Access-Control-Allow-Origin: *\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            blob.len()
        );
        sock.write_all(r.as_bytes()).await?;
        sock.write_all(blob).await?;
        return Ok(());
    }

    // Optional VOD-unlocker OBS script, handed to the browser as a Save-As
    // attachment. Serving it (rather than writing it server-side to a fixed
    // path) lets the user drop it wherever their OBS scripts folder actually
    // lives - portable installs vary, and OBS loads scripts by absolute path.
    // The bytes are embedded (see VOD_UNLOCKER_LUA), so this always matches
    // the running binary and needs no network round-trip.
    if method == "GET" && bare_path == "/obs/vod-script/download" {
        let blob = VOD_UNLOCKER_LUA.as_bytes();
        let r = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/x-lua; charset=utf-8\r\n\
             Content-Disposition: attachment; filename=\"optional-vod-unlocker.lua\"\r\n\
             Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            blob.len()
        );
        sock.write_all(r.as_bytes()).await?;
        sock.write_all(blob).await?;
        return Ok(());
    }

    // Server-Sent Events: long-lived stream of state JSON. Beats per-tab
    // 500 ms polling on idle CPU because (a) no HTTP/CSRF overhead per
    // tick and (b) the wire only carries frames when something actually
    // changed. Clients fall back to `GET /state` polling if EventSource
    // is unavailable or the connection drops.
    if method == "GET" && bare_path == "/events" {
        return handle_sse(sock, ctrl, settings, sysstat, Feed::Dashboard).await;
    }

    // The overlay's own feed. Same machinery, a payload carrying nothing but
    // what a widget paints - see `overlay_state_json` for why an overlay gets
    // its own rather than a share of the dashboard's.
    if method == "GET" && bare_path == "/overlay-events" {
        return handle_sse(sock, ctrl, settings, sysstat, Feed::Overlay).await;
    }

    // Lifecycle controls (admin-gated by auth_gate above): acknowledge and
    // flush the 200 to the client BEFORE tripping the shutdown signal. The main
    // loop can exit the process the instant its egress teardown finishes, so
    // signalling first would race our own exit and drop the response. Write,
    // half-close so the FIN follows the bytes, then signal.
    if method == "POST" && (bare_path == "/app/restart" || bare_path == "/app/quit") {
        let restart = bare_path == "/app/restart";
        let payload = if restart {
            r#"{"ok":true,"restarting":true}"#
        } else {
            r#"{"ok":true,"quitting":true}"#
        };
        write_simple(&mut sock, "200 OK", "application/json", payload, "").await?;
        let _ = sock.shutdown().await;
        if restart {
            ctrl.request_restart();
        } else {
            ctrl.request_quit();
        }
        return Ok(());
    }

    // Crash-protection preview: the reconnect screen from the same renderer
    // the stream uses. Binary, so it can't go through `route`. Admin-gated
    // by auth_gate (classify_access's default). A 24-frame strip takes tens
    // of milliseconds to draw, so it renders on a blocking thread: on this
    // runtime's one thread it would stall every destination's egress.
    if method == "GET" && bare_path == "/crash-protection/preview" {
        let query = path.split_once('?').map_or("", |(_, q)| q).to_string();
        let saved = settings.borrow().crash_protection.clone();
        let image = tokio::task::spawn_blocking(move || crash_protection_preview(&query, &saved))
            .await
            .map_err(io::Error::other)?;
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\
             Cache-Control: no-store\r\nConnection: close\r\n\r\n",
            image.len()
        );
        sock.write_all(head.as_bytes()).await?;
        sock.write_all(&image).await?;
        return Ok(());
    }

    let (status, ctype, payload) = route(
        method, path, body, &ctrl, &settings, &cfg_path, &sysstat, is_admin,
    )
    .await;

    // ACAO is intentionally restrictive now - only set on GET responses
    // so overlays / docks loaded as foreign origins can still read state.
    // POST endpoints get NO ACAO header, which (with credentials=false)
    // blocks cross-origin script-readable responses too.
    let acao = if method == "GET" {
        "Access-Control-Allow-Origin: *\r\n"
    } else {
        ""
    };
    let resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        status, ctype, payload.len(), acao
    );
    sock.write_all(resp.as_bytes()).await?;
    sock.write_all(payload.as_bytes()).await?;
    Ok(())
}

/// Render the preview for the saved crash-protection settings with any
/// unsaved `crash_protection.*` edits from the query applied on top, so
/// the dashboard can preview before Save. `orientation=vertical` flips it
/// to 9:16; `phase` (0..1) picks the moment in the 2 s loop, or
/// `frames=N` stacks N evenly spaced moments into one strip the dashboard
/// animates; `size=thumb` draws the small version the theme picker shows.
fn crash_protection_preview(
    query: &str,
    saved: &crate::crash_protection::CrashProtection,
) -> Vec<u8> {
    const DEFAULT_PHASE: f32 = 0.25;
    const MAX_FRAMES: usize = 24;
    let form = config::parse_form(query);
    let mut settings = saved.clone();
    for (key, value) in &form {
        if let Some(field) = key.strip_prefix(crate::crash_protection::KEY_PREFIX) {
            settings.set(field, value);
        }
    }
    let (long_side, short_side) = match form.get("size").map(String::as_str) {
        Some("thumb") => (192, 108),
        _ => (480, 270),
    };
    let (width, height) = if form.get("orientation").map(String::as_str) == Some("vertical") {
        (short_side, long_side)
    } else {
        (long_side, short_side)
    };
    let frames = form
        .get("frames")
        .and_then(|value| value.parse::<usize>().ok())
        .map(|count| count.clamp(1, MAX_FRAMES));
    let phases: Vec<f32> = match frames {
        Some(count) => (0..count).map(|i| i as f32 / count as f32).collect(),
        None => vec![form
            .get("phase")
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| (0.0..1.0).contains(value))
            .unwrap_or(DEFAULT_PHASE)],
    };
    crate::slate::preview_png(&settings, width, height, &phases)
}

/// Result of the auth gate: either it already wrote a response (login page,
/// 401, redirect, or an /auth/* mutation) and the caller returns, or the
/// request is allowed through with an optional `/dock` Set-Cookie header.
enum AuthDecision {
    Handled,
    /// The request may proceed. `cookie` is an optional Set-Cookie line (the
    /// dock-token handoff); `is_admin` is true for a full dashboard session or
    /// when auth is off, false for a dock-token-only caller. Routes use it to
    /// redact secrets and refuse settings writes for the dock.
    Allow {
        cookie: String,
        is_admin: bool,
    },
}

/// The optional dashboard-auth gate, kept in one auditable place so `serve()`
/// stays readable and the entire security surface (login, logout, the
/// fail-closed public/control/admin gate, and the /auth/* management endpoints)
/// is reviewable together. Off by default: with no password set it returns
/// `Allow` after a single `is_empty()`.
#[allow(clippy::too_many_arguments)]
async fn auth_gate(
    sock: &mut TcpStream,
    method: &str,
    path: &str,
    bare_path: &str,
    head_str: &str,
    body: &str,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
    auth: &Arc<crate::auth::AuthState>,
    peer_ip: &str,
) -> io::Result<AuthDecision> {
    let mut dock_set_cookie = String::new();
    // True for a full dashboard session and (below) when auth is off entirely.
    // Flipped to the session state once a password is set, so a dock-token
    // caller is marked non-admin and routes can redact secrets from it.
    let mut is_admin = true;

    // OFF BY DEFAULT: with no password set this is a single is_empty() on the
    // borrow (no allocation, no clone) and we fall straight through. Once a
    // password is set it fails closed - see `classify_access` (default = admin).
    if !settings.borrow().dashboard_password_hash.is_empty() {
        let (pw_hash, dock_token) = {
            let s = settings.borrow();
            (s.dashboard_password_hash.clone(), s.dock_token.clone())
        };
        let cookies = parse_cookies(head_str);
        let session_cookie = cookies.get("ic_session").cloned().unwrap_or_default();
        let has_session = auth.validate_session(&session_cookie);
        // Only a real session is admin; a dock token authorizes Control routes
        // but never confers admin, so it never sees a redacted-away secret.
        is_admin = has_session;

        // --- login page + login/logout (reachable without a session) ---
        if method == "GET" && bare_path == "/login" {
            write_simple(sock, "200 OK", "text/html; charset=utf-8", LOGIN_HTML, "").await?;
            return Ok(AuthDecision::Handled);
        }
        if method == "POST" && bare_path == "/login" {
            // Rate-limit BEFORE hashing so a locked-out or spamming client
            // burns no CPU. Vital on the single-threaded runtime, where a
            // 230ms hash on every attempt would otherwise stall the stream.
            // begin_login_attempt counts this try as it checks, so N concurrent
            // requests can't all clear the gate before the hash below resolves.
            if let Err(wait) = auth.begin_login_attempt(peer_ip) {
                let msg = format!(
                    r#"{{"ok":false,"error":"too many attempts, wait {}s"}}"#,
                    wait.as_secs() + 1
                );
                write_simple(sock, "429 Too Many Requests", "application/json", &msg, "").await?;
                return Ok(AuthDecision::Handled);
            }
            let password = crate::config::parse_form(body)
                .get("password")
                .cloned()
                .unwrap_or_default();
            // PBKDF2 off the runtime thread so the stream never hitches.
            let hash_for = pw_hash.clone();
            let ok = tokio::task::spawn_blocking(move || {
                crate::crypto::verify_password(&password, &hash_for)
            })
            .await
            .unwrap_or(false);
            if ok {
                auth.record_success(peer_ip);
                let token = auth.create_session();
                let set = format!(
                    "Set-Cookie: ic_session={}; HttpOnly; SameSite=Strict; Path=/{}\r\n",
                    token,
                    secure_flag(head_str)
                );
                write_simple(sock, "200 OK", "application/json", r#"{"ok":true}"#, &set).await?;
                return Ok(AuthDecision::Handled);
            }
            // The attempt was already counted by begin_login_attempt above.
            write_simple(
                sock,
                "401 Unauthorized",
                "application/json",
                r#"{"ok":false,"error":"wrong password"}"#,
                "",
            )
            .await?;
            return Ok(AuthDecision::Handled);
        }
        if method == "POST" && bare_path == "/logout" {
            if !session_cookie.is_empty() {
                auth.revoke_session(&session_cookie);
            }
            let clear = "Set-Cookie: ic_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0\r\n";
            write_simple(sock, "200 OK", "application/json", r#"{"ok":true}"#, clear).await?;
            return Ok(AuthDecision::Handled);
        }

        // --- the gate for every other route ---
        let access = classify_access(method, bare_path);
        let dock_supplied = query_param(path, "token")
            .or_else(|| cookies.get("ic_dock").cloned())
            .unwrap_or_default();
        let has_dock = !dock_token.is_empty()
            && crate::crypto::constant_time_eq(dock_supplied.as_bytes(), dock_token.as_bytes());
        let allowed = match access {
            Access::Public => true,
            Access::Control => has_session || has_dock,
            Access::Admin => has_session,
        };
        if !allowed {
            // Browser navigation to a protected page bounces to /login; an
            // API/XHR call gets a clean 401 the dashboard can react to.
            if method == "GET" && wants_html(head_str) {
                let r = "HTTP/1.1 302 Found\r\nLocation: /login\r\n\
                         Content-Length: 0\r\nConnection: close\r\n\r\n";
                sock.write_all(r.as_bytes()).await?;
                return Ok(AuthDecision::Handled);
            }
            write_simple(
                sock,
                "401 Unauthorized",
                "application/json",
                r#"{"ok":false,"error":"authentication required"}"#,
                "",
            )
            .await?;
            return Ok(AuthDecision::Handled);
        }
        // Dock authenticated via ?token: hand it a cookie so its later
        // same-origin control calls (which won't carry the query) are allowed.
        if bare_path == "/dock" && has_dock && query_param(path, "token").is_some() {
            dock_set_cookie = format!(
                "Set-Cookie: ic_dock={}; HttpOnly; SameSite=Strict; Path=/{}\r\n",
                dock_token,
                secure_flag(head_str)
            );
        }
    }

    // Auth management (cookie-bearing responses). Reached only after the gate
    // above (these classify as admin, so session-gated when auth is on); open
    // when auth is off so the very first password can be set locally.
    if method == "POST" && bare_path == "/auth/set-password" {
        // Bootstrapping the FIRST password is the one admin action reachable
        // without a session (there is no session to require yet), so restrict
        // that single case to loopback. On a bind-all box this stops a LAN peer
        // from seizing the dashboard before the owner sets a password; changing
        // an existing password already required an admin session at the gate.
        let first_time = settings.borrow().dashboard_password_hash.is_empty();
        if first_time && !is_loopback(peer_ip) {
            write_simple(
                sock,
                "403 Forbidden",
                "application/json",
                r#"{"ok":false,"error":"the first password must be set from the local machine"}"#,
                "",
            )
            .await?;
            return Ok(AuthDecision::Handled);
        }
        let pw = crate::config::parse_form(body)
            .get("password")
            .cloned()
            .unwrap_or_default();
        // Enforce a minimum server-side, not just in the UI, so a short
        // password can't be set via a direct API call.
        if pw.chars().count() < 8 {
            write_simple(
                sock,
                "400 Bad Request",
                "application/json",
                r#"{"ok":false,"error":"password must be at least 8 characters"}"#,
                "",
            )
            .await?;
            return Ok(AuthDecision::Handled);
        }
        let hash = tokio::task::spawn_blocking(move || crate::crypto::hash_password(&pw))
            .await
            .unwrap_or_default();
        if hash.is_empty() {
            write_simple(
                sock,
                "500 Internal Server Error",
                "application/json",
                r#"{"ok":false,"error":"hashing failed"}"#,
                "",
            )
            .await?;
            return Ok(AuthDecision::Handled);
        }
        {
            let _wl = settings_write_guard();
            let mut ns = settings.borrow().clone();
            ns.dashboard_password_hash = hash;
            if ns.dock_token.is_empty() {
                ns.dock_token = crate::crypto::random_token();
            }
            let _ = ns.save(cfg_path);
            let _ = settings.send(ns);
        }
        // Every prior session dies; issue a fresh one for whoever set it.
        auth.revoke_all();
        let token = auth.create_session();
        let set = format!(
            "Set-Cookie: ic_session={}; HttpOnly; SameSite=Strict; Path=/{}\r\n",
            token,
            secure_flag(head_str)
        );
        write_simple(sock, "200 OK", "application/json", r#"{"ok":true}"#, &set).await?;
        return Ok(AuthDecision::Handled);
    }
    if method == "POST" && bare_path == "/auth/disable" {
        // Already disabled when no password is set: a no-op without a config
        // write (and there is no session this request could have proven). When
        // auth is on, the gate above already required an admin session to reach
        // this point.
        if settings.borrow().dashboard_password_hash.is_empty() {
            write_simple(sock, "200 OK", "application/json", r#"{"ok":true}"#, "").await?;
            return Ok(AuthDecision::Handled);
        }
        {
            let _wl = settings_write_guard();
            let mut ns = settings.borrow().clone();
            ns.dashboard_password_hash.clear();
            ns.dock_token.clear();
            let _ = ns.save(cfg_path);
            let _ = settings.send(ns);
        }
        auth.revoke_all();
        let clear = "Set-Cookie: ic_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0\r\n";
        write_simple(sock, "200 OK", "application/json", r#"{"ok":true}"#, clear).await?;
        return Ok(AuthDecision::Handled);
    }
    if method == "POST" && bare_path == "/auth/regen-dock" {
        // The dock token only gates anything once a password is set, so refuse
        // to rotate it (and churn config) from an unauthenticated request while
        // auth is off. When auth is on, the gate above already required an admin
        // session to reach this point.
        if settings.borrow().dashboard_password_hash.is_empty() {
            write_simple(
                sock,
                "400 Bad Request",
                "application/json",
                r#"{"ok":false,"error":"enable a dashboard password before rotating the dock token"}"#,
                "",
            )
            .await?;
            return Ok(AuthDecision::Handled);
        }
        let new_token;
        {
            let _wl = settings_write_guard();
            let mut ns = settings.borrow().clone();
            ns.dock_token = crate::crypto::random_token();
            new_token = ns.dock_token.clone();
            let _ = ns.save(cfg_path);
            let _ = settings.send(ns);
        }
        let msg = format!(r#"{{"ok":true,"dock_token":"{}"}}"#, new_token);
        write_simple(sock, "200 OK", "application/json", &msg, "").await?;
        return Ok(AuthDecision::Handled);
    }

    Ok(AuthDecision::Allow {
        cookie: dock_set_cookie,
        is_admin,
    })
}

// The central request dispatcher genuinely needs all of these: the request
// parts, the shared runtime handles, and the caller's admin flag. Bundling them
// into a struct would only move the argument list, not remove it.
#[allow(clippy::too_many_arguments)]
async fn route(
    method: &str,
    path: &str,
    body: &str,
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
    sysstat: &Arc<SysStat>,
    is_admin: bool,
) -> (&'static str, &'static str, String) {
    // Strip ?query - we only read it for /overlay.
    let (bare_path, query) = match path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path, ""),
    };
    // Pluggable overlays: any GET /overlay/<filename> reads from disk.
    if method == "GET" && bare_path.starts_with("/overlay/") {
        let name = &bare_path["/overlay/".len()..];
        return serve_overlay_file(name, settings);
    }
    // Overlay Studio writes: POST /overlays/<slug> saves a baked overlay,
    // POST /overlays/<slug>/delete removes it. The slug is restricted to a
    // safe charset (no separators) so no path can escape overlays_dir.
    // POST /overlays/seeded marks the built-in presets as installed; POST
    // /overlays/reset wipes the Studio overlays and clears that flag so the
    // dashboard re-seeds the defaults on its next load.
    if method == "POST" && bare_path.starts_with("/overlays/") {
        let rest = &bare_path["/overlays/".len()..];
        if rest == "seeded" {
            return overlays_mark_seeded(settings, cfg_path);
        }
        if rest == "reset" {
            return overlays_reset(settings, cfg_path);
        }
        if let Some(slug) = rest.strip_suffix("/delete") {
            return overlay_delete(slug, settings);
        }
        return overlay_save(rest, body, settings);
    }

    // Dock layouts: GET /docks/<id> returns the saved layout JSON (or
    // `null`); POST /docks/<id> saves the request body as that dock's
    // layout, or clears it when the body is empty. Persisted in settings
    // so a customized dock survives OBS wiping its browser cache.
    if let Some(id) = bare_path.strip_prefix("/docks/") {
        if method == "GET" {
            return dock_layout_get(id, settings);
        }
        if method == "POST" {
            return dock_layout_save(id, body, settings, cfg_path).await;
        }
    }

    // Integrations: `/hooks/<token>` is how outside apps trigger a web call
    // integration; the rest is the dashboard's integrations API.
    if let Some(token) = bare_path.strip_prefix("/hooks/") {
        return crate::integrations::api::hook(ctrl, token, body, query);
    }
    if bare_path.starts_with("/integrations")
        || bare_path.starts_with("/connections/")
        || bare_path.starts_with("/twitch/")
    {
        if let Some(reply) =
            crate::integrations::api::route(method, bare_path, body, ctrl, settings, cfg_path).await
        {
            return reply;
        }
    }

    match (method, bare_path) {
        // GET / and GET /dock are handled in serve() as a fast-path
        // (static gz blob, no allocation, no String round-trip).
        ("GET", "/overlay") => ("200 OK", "text/html; charset=utf-8", overlay_html(query)),
        ("GET", "/state") => (
            "200 OK",
            "application/json",
            state_json(ctrl, settings, sysstat),
        ),
        ("GET", "/overlay-state") => (
            "200 OK",
            "application/json",
            overlay_state_json(ctrl, settings),
        ),
        ("GET", "/config") => (
            "200 OK",
            "application/json",
            // A dock-token caller (is_admin == false) gets the config with the
            // raw ingest key and dock token blanked; a full session gets them.
            settings
                .borrow()
                .to_json(crate::autostart::is_enabled(), is_admin),
        ),
        ("GET", "/docks") => dock_list_json(settings),
        ("GET", "/platforms") => ("200 OK", "application/json", platforms_json()),
        // OBS sends a POST with a system-info payload (CPU/GPU/encoder
        // capabilities + the user's stream-key field as the auth
        // value). We proxy that payload to Twitch's real config
        // endpoint with the streamer's real Twitch key swapped in,
        // then rewrite the response's ingest URL to point back at us
        // so OBS sends multi-track to InstantClone instead of Twitch
        // directly. Falls back to a self-contained static config when
        // no Twitch destination is configured or the upstream call
        // fails. GET is supported as an escape hatch for poking the
        // static fallback from a browser address bar.
        ("POST", "/obs/multitrack-config") => {
            let authorized = eb_request_authorized(body, &settings.borrow().ingest_key);
            let config = obs_multitrack_config_proxy(body, query, ctrl, settings).await;
            // A refused request gets the static config and its publish is
            // turned away: it must not replace the live session's tracks.
            if authorized {
                ctrl.note_eb_config(&config);
            }
            ("200 OK", "application/json", config)
        }
        ("GET", "/obs/multitrack-config") => (
            "200 OK",
            "application/json",
            obs_multitrack_config_static(query, body, settings),
        ),
        ("GET", "/obs/register-status") => {
            let s = settings.borrow();
            (
                "200 OK",
                "application/json",
                format!(
                    r#"{{"registered":{},"obs_running":{},"vod_audio_flag":{},"vod_eb_injected":{},"obs_version":{},"active_profile":{},"path":{}}}"#,
                    crate::obs_register::is_registered(s.web_port, s.ingest_port),
                    crate::obs_register::is_obs_running(),
                    crate::obs_register::vod_audio_flag_set(),
                    crate::obs_register::vod_eb_injection_present(s.web_port),
                    match crate::obs_register::obs_version() {
                        Some((a, b, c)) => format!(r#""{a}.{b}.{c}""#),
                        None => "null".to_string(),
                    },
                    match crate::obs_register::active_profile() {
                        Some(p) => format!(r#""{}""#, p.replace('\\', "\\\\").replace('"', "\\\"")),
                        None => "null".to_string(),
                    },
                    match crate::obs_register::services_json_path() {
                        Some(p) =>
                            format!(r#""{}""#, p.display().to_string().replace('\\', "\\\\")),
                        None => "null".to_string(),
                    }
                ),
            )
        }
        ("POST", "/obs/register") => {
            let s = settings.borrow();
            match crate::obs_register::register(s.web_port, s.ingest_port) {
                Ok(()) => (
                    "200 OK",
                    "application/json",
                    r#"{"ok":true,"message":"Registered with OBS - restart OBS to see the InstantClone service in the dropdown."}"#.to_string(),
                ),
                Err(e) => (
                    "500 Internal Server Error",
                    "application/json",
                    format!(r#"{{"ok":false,"error":"{}"}}"#, e.to_string().replace('"', "'")),
                ),
            }
        }
        ("POST", "/obs/unregister") => match crate::obs_register::unregister() {
            Ok(()) => (
                "200 OK",
                "application/json",
                r#"{"ok":true,"message":"Unregistered. Restart OBS to refresh the service list."}"#
                    .to_string(),
            ),
            Err(e) => (
                "500 Internal Server Error",
                "application/json",
                format!(
                    r#"{{"ok":false,"error":"{}"}}"#,
                    e.to_string().replace('"', "'")
                ),
            ),
        },
        ("POST", "/obs/launch-with-eb") => {
            let s = settings.borrow();
            match crate::obs_register::launch_obs_with_eb_config(s.web_port, &s.dock_token) {
                Ok(exe) => {
                    ctrl.log(format!(
                        "obs-eb-launch: spawned {} with --config-url",
                        exe.display()
                    ));
                    (
                        "200 OK",
                        "application/json",
                        format!(
                            r#"{{"ok":true,"message":"Launched OBS with Enhanced Broadcasting enabled. OBS will pick up multi-track video from InstantClone on this session only - if you close OBS and reopen normally, you'll need this button again.","exe":"{}"}}"#,
                            exe.display()
                                .to_string()
                                .replace('\\', "\\\\")
                                .replace('"', "'")
                        ),
                    )
                }
                Err(e) => (
                    "500 Internal Server Error",
                    "application/json",
                    format!(
                        r#"{{"ok":false,"error":"{}"}}"#,
                        e.to_string().replace('"', "'")
                    ),
                ),
            }
        }
        ("POST", "/obs/setup-vod-eb") => {
            // One-click VOD-audio + Enhanced Broadcasting setup. Runs the
            // three steps in order and reports each independently so the
            // dashboard can show a red-to-green checklist: a failure in one
            // step (e.g. OBS still open, so the flag write is blocked) is
            // surfaced with its own message instead of failing the whole
            // operation silently.
            let (web_port, dock_token) = {
                let s = settings.borrow();
                (s.web_port, s.dock_token.clone())
            };
            let (flag_ok, flag_msg) = match crate::obs_register::set_vod_audio_flag(true) {
                Ok(true) => (true, "VOD-track flag written to OBS config".to_string()),
                Ok(false) => (
                    false,
                    "OBS config not found - is OBS installed?".to_string(),
                ),
                Err(e) => (
                    false,
                    format!("could not write OBS config (close OBS and retry): {e}"),
                ),
            };
            let (launch_ok, launch_msg) =
                match crate::obs_register::launch_obs_with_eb_config(web_port, &dock_token) {
                    Ok(exe) => {
                        ctrl.log("[vod-eb setup] launched OBS with --config-url");
                        (true, format!("OBS launched ({})", exe.display()))
                    }
                    Err(e) => (false, e.to_string()),
                };
            let verified = crate::obs_register::vod_audio_flag_set();
            let verify_msg = if verified {
                "VOD-track flag confirmed in OBS config"
            } else {
                "VOD-track flag not present after write - close OBS and try again"
            };
            let all_ok = flag_ok && launch_ok && verified;
            (
                // Always 200: partial success is still a valid response;
                // the per-step `ok` flags carry the detail the UI renders.
                "200 OK",
                "application/json",
                format!(
                    r#"{{"ok":{ok},"steps":[{{"name":"VOD-track flag","ok":{f},"msg":"{fm}"}},{{"name":"Launch OBS (EB)","ok":{l},"msg":"{lm}"}},{{"name":"Verify flag","ok":{v},"msg":"{vm}"}}]}}"#,
                    ok = all_ok,
                    f = flag_ok,
                    fm = json_escape(&flag_msg),
                    l = launch_ok,
                    lm = json_escape(&launch_msg),
                    v = verified,
                    vm = json_escape(verify_msg),
                ),
            )
        }
        ("POST", "/shortcut/create-eb") => match crate::obs_register::create_eb_shortcut() {
            Ok(path) => {
                ctrl.log(format!(
                    "created VOD+EB desktop shortcut: {}",
                    path.display()
                ));
                let kind = if path.extension().and_then(|e| e.to_str()) == Some("lnk") {
                    "shortcut"
                } else {
                    "launcher (.cmd fallback)"
                };
                (
                    "200 OK",
                    "application/json",
                    format!(
                        r#"{{"ok":true,"path":"{p}","kind":"{k}"}}"#,
                        p = json_escape(&path.display().to_string()),
                        k = kind,
                    ),
                )
            }
            Err(e) => (
                "500 Internal Server Error",
                "application/json",
                format!(
                    r#"{{"ok":false,"error":"{}"}}"#,
                    json_escape(&e.to_string())
                ),
            ),
        },
        ("GET", "/update-check") => {
            // check_update() uses blocking ureq with a ~10 s ceiling. Hand
            // it to a blocking thread so a slow GitHub doesn't pin the
            // single-threaded web runtime and stall unrelated requests
            // (most visibly: the About sub-tab's Check button would
            // freeze /destinations + /state for any open dashboard).
            let info = tokio::task::spawn_blocking(crate::update_check::check_update)
                .await
                .unwrap_or_else(|_| crate::update_check::UpdateInfo {
                    current: crate::update_check::current_version().to_string(),
                    latest: None,
                    update_available: false,
                    error: Some("update check task panicked".into()),
                });
            ("200 OK", "application/json", info.to_json())
        }
        ("POST", "/update/apply") => {
            // Stage (download + verify) inline so any failure surfaces as a
            // clean JSON error the About tab can show. The actual swap +
            // relaunch + process exit is deferred to a detached thread so
            // this 200 flushes to the browser FIRST - the browser then polls
            // /config until we're back and reloads.
            match tokio::task::spawn_blocking(crate::self_update::prepare).await {
                Ok(Ok(staged)) => {
                    let v = staged.version.replace('"', "'");
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(1200));
                        crate::self_update::commit_and_relaunch(staged);
                    });
                    (
                        "200 OK",
                        "application/json",
                        format!(r#"{{"ok":true,"restarting":true,"version":"{v}"}}"#),
                    )
                }
                Ok(Err(msg)) => (
                    "200 OK",
                    "application/json",
                    format!(
                        r#"{{"ok":false,"error":"{}"}}"#,
                        msg.replace('\\', "\\\\").replace('"', "'")
                    ),
                ),
                Err(_) => (
                    "200 OK",
                    "application/json",
                    r#"{"ok":false,"error":"update task panicked"}"#.to_string(),
                ),
            }
        }
        // POST /app/restart and /app/quit are handled in `serve` (they must
        // flush their 200 before the shutdown signal exits the process), so
        // they never reach the router.

        // Reveal one of our known files in the OS file browser. The keyword
        // is fixed (not a client-supplied path), so this can only ever open
        // our own buffer / overlays / trace - never an arbitrary location.
        ("POST", "/reveal/buffer") => {
            reveal_path(&settings.borrow().buffer_path.clone());
            ("200 OK", "application/json", r#"{"ok":true}"#.to_string())
        }
        ("POST", "/reveal/overlays") => {
            reveal_path(&settings.borrow().overlays_dir.clone());
            ("200 OK", "application/json", r#"{"ok":true}"#.to_string())
        }
        ("POST", "/reveal/trace") => {
            reveal_path(Path::new("./instantclone-trace.log"));
            ("200 OK", "application/json", r#"{"ok":true}"#.to_string())
        }
        ("GET", "/obs/launch-status") => {
            let exe = crate::obs_register::find_obs_executable();
            (
                "200 OK",
                "application/json",
                format!(
                    r#"{{"installed":{},"exe":{}}}"#,
                    exe.is_some(),
                    match exe {
                        Some(p) => format!(
                            "\"{}\"",
                            p.display()
                                .to_string()
                                .replace('\\', "\\\\")
                                .replace('"', "'")
                        ),
                        None => "null".to_string(),
                    }
                ),
            )
        }
        ("GET", "/twitch_ingests") => ("200 OK", "application/json", twitch_ingests_json()),
        ("GET", "/profiles") => ("200 OK", "application/json", profiles_json(settings)),
        ("GET", "/logs") => ("200 OK", "application/json", logs_json(ctrl)),
        ("GET", "/overlays") => ("200 OK", "application/json", list_overlays(settings)),
        ("GET", "/destinations") => (
            "200 OK",
            "application/json",
            // Custom URLs raw for a full session only; see destinations_json.
            destinations_json(ctrl, settings, is_admin),
        ),

        ("POST", "/config") => post_config(body, ctrl, settings, cfg_path).await,
        ("POST", "/config/reset") => post_config_reset(query, ctrl, settings, cfg_path).await,
        // Two-phase delay endpoints
        ("POST", "/arm") => post_arm(body, ctrl, settings, cfg_path, sysstat).await,
        ("POST", "/activate") => post_activate(ctrl, settings, cfg_path, sysstat).await,
        ("POST", "/stop") => post_stop(ctrl, settings, cfg_path, sysstat).await,
        ("POST", "/disarm") => post_disarm(ctrl, settings, cfg_path, sysstat).await,
        // Legacy one-shot endpoints (Stream Deck etc.)
        ("POST", "/delay") => post_delay(body, ctrl, settings, cfg_path, sysstat).await,
        ("POST", "/go-live") => post_stop(ctrl, settings, cfg_path, sysstat).await,
        ("POST", "/cut-after") => post_cut_after(ctrl, settings, sysstat).await,
        ("POST", "/cut-after/cancel") => post_cut_after_cancel(ctrl, settings, sysstat).await,
        ("POST", "/crash-protection/end") => {
            ctrl.end_hold_now();
            (
                "200 OK",
                "application/json",
                state_json(ctrl, settings, sysstat),
            )
        }
        // MIDI mapping (Windows). Learn mode is server-side because the MIDI
        // events arrive at the backend, not the browser; the dashboard polls.
        #[cfg(windows)]
        ("POST", "/hotkeys/capture") => post_hotkey_capture(body, ctrl).await,
        ("POST", "/midi/learn") => post_midi_learn(body, ctrl).await,
        ("POST", "/midi/learn/cancel") => post_midi_learn_cancel(ctrl).await,
        ("POST", "/midi/poll") => post_midi_poll(ctrl, settings, cfg_path).await,
        ("POST", "/test-egress") => test_egress(settings).await,
        ("POST", "/logs/clear") => {
            ctrl.clear_logs();
            ("200 OK", "application/json", r#"{"ok":true}"#.into())
        }
        // Profiles CRUD
        ("POST", "/profiles") => post_profile_add(body, settings, cfg_path).await,
        ("POST", "/profiles/delete") => post_profile_del(body, settings, cfg_path).await,
        // Destinations CRUD
        ("POST", "/destinations") => post_destination_upsert(body, ctrl, settings, cfg_path).await,
        ("POST", "/destinations/toggle") => {
            post_destination_toggle(body, ctrl, settings, cfg_path).await
        }
        ("POST", "/destinations/delete") => {
            post_destination_delete(body, ctrl, settings, cfg_path).await
        }
        _ => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found".into(),
        ),
    }
}

// ---- Server-Sent Events (state stream) ----

/// Long-lived SSE handler. Sends a `data: <json>\n\n` frame whenever the
/// state JSON would actually change (string-diff against last sent), with
/// a heartbeat every 10 s to keep middleboxes from killing the socket.
///
/// Tick cadence is 250 ms - fine-grained enough for the bar/readout to
/// feel responsive, but only writes to the wire on change. With one tab
/// open this is the same compute as polling at 4 Hz; with N tabs it's N×
/// the compute but no per-request HTTP overhead. The win is upstream:
/// the dashboard JS no longer fires a fresh `fetch('/state')` every 500
/// ms, so connection-close churn and CSRF parsing disappear.
/// Which payload an SSE connection carries.
///
/// The dashboard is a logged-in operator surface and gets everything. An
/// overlay is a picture on a stream: it is served to anyone who can reach the
/// port, so it gets a payload that is only the numbers it draws.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Feed {
    Dashboard,
    Overlay,
}

async fn handle_sse(
    mut sock: TcpStream,
    ctrl: Arc<Controller>,
    settings: Arc<watch::Sender<Settings>>,
    sysstat: Arc<SysStat>,
    feed: Feed,
) -> io::Result<()> {
    // SSE preamble. `X-Accel-Buffering: no` tells nginx-style proxies not
    // to buffer; `Cache-Control: no-store` keeps browsers from caching;
    // `Connection: keep-alive` is essential - our other routes use close.
    let headers = b"HTTP/1.1 200 OK\r\n\
                    Content-Type: text/event-stream\r\n\
                    Cache-Control: no-store\r\n\
                    Access-Control-Allow-Origin: *\r\n\
                    X-Accel-Buffering: no\r\n\
                    Connection: keep-alive\r\n\r\n";
    sock.write_all(headers).await?;
    // Initial retry hint - clients reconnect after 1 s if the socket drops.
    sock.write_all(b"retry: 1000\n\n").await?;

    let mut last_payload = String::new();
    let mut last_send = std::time::Instant::now();
    let heartbeat = Duration::from_secs(10);
    let tick = Duration::from_millis(250);

    loop {
        let cur = match feed {
            Feed::Dashboard => state_json(&ctrl, &settings, &sysstat),
            Feed::Overlay => overlay_state_json(&ctrl, &settings),
        };
        let now = std::time::Instant::now();
        let changed = cur != last_payload;
        let beat_due = now.duration_since(last_send) >= heartbeat;
        if changed || beat_due {
            // SSE wire format: each data frame is `data: <line>\n\n`.
            // We pre-allocate to avoid two small writes for the common
            // small payloads, since TCP write coalescing isn't guaranteed.
            let mut frame = String::with_capacity(cur.len() + 8);
            frame.push_str("data: ");
            frame.push_str(&cur);
            frame.push_str("\n\n");
            if sock.write_all(frame.as_bytes()).await.is_err() {
                return Ok(()); // client gone - just exit, don't escalate
            }
            last_payload = cur;
            last_send = now;
        }
        tokio::time::sleep(tick).await;
    }
}

// ---- Endpoints ----

/// Parse each cached video seq-header once per poll, not once per
/// destination: the res/codec readout depends only on which TrackId a
/// dest forwards, and horizontal dests all share track 0 while vertical
/// dests share the one detected portrait primary. Keeps the
/// frequently-polled `/state` and `/destinations` endpoints off a
/// per-dest lock + Exp-Golomb parse.
fn video_readouts(ctrl: &Controller) -> std::collections::BTreeMap<u8, (String, String)> {
    let headers = ctrl.ring.video_seq_headers.lock();
    headers
        .iter()
        .map(|(&track, h)| {
            let res = crate::h264::sps_dimensions(h)
                .map(|(w, hh)| format!("{}x{}", w, hh))
                .unwrap_or_default();
            let codec = match crate::h264::seq_header_codec(h) {
                crate::h264::VideoCodec::Unknown => String::new(),
                c => c.label().to_string(),
            };
            (track, (res, codec))
        })
        .collect()
}

/// Live video state for one destination: whether a vertical 9:16 canvas
/// is on the wire, whether this destination can forward yet, and the
/// resolution + codec of the track it actually sends.
///
/// Shared by `/state` and `/destinations` so the two endpoints cannot
/// drift. They previously derived this independently and `/state` omitted
/// it entirely, which froze the dashboard's format icon and res/codec
/// readout at whatever was true when the page last fetched
/// `/destinations`.
struct DestVideo {
    vertical_canvas_present: bool,
    vertical_ready: bool,
    res: String,
    codec: String,
}

fn dest_video(
    ctrl: &Controller,
    dest: &crate::config::Destination,
    readouts: &std::collections::BTreeMap<u8, (String, String)>,
) -> DestVideo {
    use std::sync::atomic::Ordering;
    // Detected globally and stored on every dest each supervisor tick, so
    // it is meaningful for Twitch cards too - that's what lets the format
    // icon show "both" only when Dual Format is actually on, rather than
    // just because the destination is Twitch.
    let track = ctrl
        .destination_state(&dest.id)
        .vertical_primary_track
        .load(Ordering::Relaxed);
    // The sequence-header cache outlives the OBS session (it is cleared on
    // the next publish), so a canvas only counts while OBS is live.
    let vertical_canvas_present = track != 0xFF && ctrl.ingest_alive();
    let vertical = dest.wants_vertical();
    // The track this destination actually forwards: the detected portrait
    // primary for vertical dests, track 0 (horizontal primary) otherwise.
    // Misses resolve to empty when the track isn't cached yet (non-AVC we
    // can't measure, or an unresolved vertical canvas whose 0xFF sentinel
    // is never a map key).
    let target = if vertical { track } else { 0 };
    let (res, codec) = readouts.get(&target).cloned().unwrap_or_default();
    DestVideo {
        vertical_canvas_present,
        // Non-vertical destinations always report ready so the badge logic
        // stays simple; vertical ones wait for the canvas to resolve.
        vertical_ready: if vertical {
            vertical_canvas_present
        } else {
            true
        },
        res,
        codec,
    }
}

/// The crash-protection hold for `/state`: `null`, or why it is on air and
/// how long it has left.
fn hold_json(ctrl: &Controller) -> String {
    match ctrl.hold_status() {
        Some(status) => format!(
            r#"{{"reason":"{}","remaining_ms":{},"total_ms":{}}}"#,
            match status.reason {
                crate::crash_hold::HoldReason::Crash => "crash",
                crate::crash_hold::HoldReason::Freeze => "freeze",
            },
            status.remaining.as_millis(),
            status.total.as_millis()
        ),
        None => "null".into(),
    }
}

fn state_json(
    ctrl: &Controller,
    settings: &Arc<watch::Sender<Settings>>,
    sysstat: &Arc<SysStat>,
) -> String {
    let s = settings.borrow();
    let kbps = ctrl.bitrate_kbps().max(2_000) as u64;
    let max_buffer_ms = s.buffer_mb * 1024 * 1024 * 8 / kbps;
    let (alive_count, total_count) = ctrl.destination_alive_summary();
    let (cpu_pct, rss_bytes) = sysstat.sample();
    let consumer_lag = ctrl.max_consumer_lag();
    // True backpressure: delivered delay has grown materially beyond
    // what the user asked for, sustained > 1.5 s. The old "tags-behind"
    // threshold falsely tripped for any active delay (a 5 s delay
    // intentionally keeps the consumer ~400 tags behind), so this is
    // now timestamp-based. See `Controller::is_backpressured`.
    let backpressure = ctrl.is_backpressured();

    // Per-destination summary array - joined from settings (the configured
    // list) with the controller's live runtime stats.
    let snap = ctrl.destination_snapshot();
    // `/state` is the dashboard's per-tick source of truth, so it carries
    // every per-dest field that can change mid-session. `/destinations` is
    // only refetched on config edits; anything live that lives solely
    // there freezes on the cards until the user reloads.
    let readouts = video_readouts(ctrl);
    let holding = ctrl.hold_active();
    let dest_list = s.destinations.iter().map(|d| {
        let st = snap.iter().find(|t| t.0 == d.id);
        let (alive, kbps, tags, bytes, cuts, recon) = st
            .map(|t| (t.1, t.3, t.4, t.5, t.6, t.7))
            .unwrap_or((false, 0u32, 0u64, 0u64, 0u32, 0u32));
        let v = dest_video(ctrl, d, &readouts);
        format!(
            r#"{{"id":{id},"name":{n},"enabled":{en},"alive":{al},"bitrate_kbps":{br},"tags_sent":{ts},"bytes_sent":{bs},"cuts":{cu},"reconnects":{rc},"vertical_ready":{vr},"vertical_canvas_present":{vcp},"video_res":{vres},"video_codec":{vcod},"on_hold":{oh}}}"#,
            id = json_escape_quoted(&d.id),
            n  = json_escape_quoted(&d.name),
            en = d.enabled, al = alive, br = kbps, ts = tags, bs = bytes, cu = cuts, rc = recon,
            vr = v.vertical_ready,
            vcp = v.vertical_canvas_present,
            vres = json_escape_quoted(&v.res),
            vcod = json_escape_quoted(&v.codec),
            oh = holding && alive && crate::crash_hold::covers(ctrl, &ctrl.destination_state(&d.id)),
        )
    }).collect::<Vec<_>>().join(",");

    // Encoder settings that will bite one of the enabled destinations.
    // Empty far more often than not - see `compat::compat_warning`. Only
    // computed while OBS is actually publishing: with no live stream the
    // measured params are stale or zero, and a warning about a session
    // that already ended is pure noise.
    let compat_warning = if ctrl.ingest_alive() {
        crate::compat::compat_warning(&ctrl.stream_params(), &s.destinations).unwrap_or_default()
    } else {
        String::new()
    };

    // A portrait canvas is on the wire right now (OBS's Additional canvas,
    // or Twitch Dual Format). Drives the header "Vertical" pill.
    let vertical_present = ctrl.ingest_alive() && ctrl.vertical_track_on_wire().is_some();

    format!(
        r#"{{"phase":"{ph}","armed_delay_ms":{ad},"target_delay_ms":{td},"current_delay_ms":{cd},"buffer_fill_ms":{bf},"buffer_target_ms":{btm},"buffer_capacity_ms_est":{bc},"ingest_alive":{ia},"egress_alive":{ea},"destinations_alive":{dla},"destinations_total":{dlt},"buffer_building":{bb},"configured":{cfg},"obs_url":"{ou}","webhook_set":{ws},"video_codec":"{vc}","audio_codec":"{ac}","multitrack_video":{mtv},"multitrack_audio":{mta},"vertical_present":{vp},"cpu_pct":{cp:.2},"rss_bytes":{rb},"uptime_secs":{up},"publisher_token":{pt},"consumer_lag":{cl},"backpressure":{bp},"safe_cut_pending":{scp},"safe_cut_remaining_ms":{scr},"hold":{hold},"compat_warning":{cw},"hotkey_conflicts":[{hkc}],"last_action":{la},"stats":{{"tags_sent":{ts},"bytes_sent":{bs},"cuts":{cu},"ingest_disconnects":{id},"egress_reconnects":{er},"bitrate_kbps":{br}}},"destinations":[{dl}]}}"#,
        ph = ctrl.phase(),
        scp = ctrl.safe_cut_pending(),
        scr = ctrl.safe_cut_remaining_ms(),
        hold = hold_json(ctrl),
        cw = json_escape_quoted(&compat_warning),
        la = ctrl
            .last_action()
            .map(|a| {
                format!(
                    r#"{{"seq":{s},"action":"{ac}","source":"{src}","problem":{p}}}"#,
                    s = a.seq,
                    ac = json_escape(&a.action),
                    src = json_escape(&a.source),
                    p = a
                        .problem
                        .as_deref()
                        .map(json_escape_quoted)
                        .unwrap_or_else(|| "null".to_string()),
                )
            })
            .unwrap_or_else(|| "null".to_string()),
        hkc = ctrl
            .hotkey_conflicts()
            .iter()
            .map(|a| format!("\"{a}\""))
            .collect::<Vec<_>>()
            .join(","),
        ad = ctrl.armed_delay_ms(),
        td = ctrl.target_delay_ms(),
        cd = ctrl.current_delay_ms(),
        bf = ctrl.buffer_fill_ms(),
        btm = ctrl.target_buffer_ms(),
        bc = max_buffer_ms,
        ia = ctrl.ingest_alive(),
        ea = ctrl.egress_alive(),
        dla = alive_count,
        dlt = total_count,
        bb = ctrl.buffer_building(),
        cfg = s.configured,
        ou = s.obs_url(),
        ws = !s.discord_webhook_url.is_empty(),
        ts = ctrl.tags_sent(),
        bs = ctrl.bytes_sent(),
        cu = ctrl.cuts_performed(),
        id = ctrl.ingest_disconnects(),
        er = ctrl.egress_reconnects(),
        br = ctrl.bitrate_kbps(),
        vc = ctrl.video_codec().label(),
        ac = ctrl.audio_codec().label(),
        mtv = ctrl.multitrack_video(),
        mta = ctrl.multitrack_audio(),
        vp = vertical_present,
        cp = cpu_pct,
        rb = rss_bytes,
        up = ctrl.uptime_secs(),
        pt = ctrl.publisher_token(),
        cl = consumer_lag,
        bp = backpressure,
        dl = dest_list,
    )
}

/// The state an overlay is allowed to see: the numbers it paints, and
/// nothing else.
///
/// An overlay is not an operator surface. It is a picture composited into a
/// live stream, loaded by an OBS browser source that cannot log in, so its
/// page and its data are reachable by anyone who can reach the port. That
/// makes `/state` the wrong feed for it twice over.
///
/// It carries too much. `/state` exists for a logged-in dashboard and
/// includes the ingest URL, the publisher token, host CPU and memory, the
/// compat warning, the hotkey conflict list, and every destination's id and
/// name - "Twitch main", "client backup". An overlay draws none of it, and
/// an overlay is on screen: whatever it holds is one bug away from being on
/// the stream.
///
/// And reaching it means being allowed to reach the routes beside it. The
/// only way to let an unauthenticated browser source read `/state` is to
/// open `Access::Control`, which is arm, activate, cut and go-live. A
/// picture would become a control path, and anyone who could see the overlay
/// could drive the delay.
///
/// So the fields below are the complete set the two overlay renderers
/// actually read (`overlay_html` here and `web/overlay-runtime.js` for saved
/// overlays), and the destination list is reduced to liveness flags with the
/// names dropped. Nothing here identifies the streamer, authorises anything,
/// or describes the machine. Adding a field is a decision to put it on
/// screen; there is a test that fails if a secret-shaped one appears.
fn overlay_state_json(ctrl: &Controller, settings: &Arc<watch::Sender<Settings>>) -> String {
    let (alive_count, total_count) = ctrl.destination_alive_summary();
    // Config order, so the dots keep their positions between ticks, joined
    // with live state the same way `/state` does it - but the id and name
    // that join them stay here.
    let snap = ctrl.destination_snapshot();
    let dots = settings
        .borrow()
        .destinations
        .iter()
        .map(|d| {
            let alive = snap.iter().find(|t| t.0 == d.id).map(|t| t.1) == Some(true);
            format!(r#"{{"alive":{alive}}}"#)
        })
        .collect::<Vec<_>>()
        .join(",");

    format!(
        r#"{{"phase":"{ph}","armed_delay_ms":{ad},"target_delay_ms":{td},"current_delay_ms":{cd},"buffer_fill_ms":{bf},"buffer_target_ms":{btm},"ingest_alive":{ia},"destinations_alive":{dla},"destinations_total":{dlt},"stats":{{"cuts":{cu},"bitrate_kbps":{br}}},"destinations":[{dl}]}}"#,
        ph = ctrl.phase(),
        ad = ctrl.armed_delay_ms(),
        td = ctrl.target_delay_ms(),
        cd = ctrl.current_delay_ms(),
        bf = ctrl.buffer_fill_ms(),
        btm = ctrl.target_buffer_ms(),
        ia = ctrl.ingest_alive(),
        dla = alive_count,
        dlt = total_count,
        cu = ctrl.cuts_performed(),
        br = ctrl.bitrate_kbps(),
        dl = dots,
    )
}

fn platforms_json() -> String {
    // Per-platform first-run help: a deep-link to where the stream key
    // lives in that platform's dashboard, and a one-line quirk worth
    // surfacing before the user wastes a stream session on it (Kick's
    // no-B-frames rule is the prime example - without that hint, OBS's
    // default config gets dropped by AWS IVS within seconds).
    //
    // Hand-written rather than table-driven because the strings are
    // short, stable, and need careful copyediting per platform; a
    // generator would just add indirection. JSON-safe at source - no
    // string here contains an unescaped " or \.
    r#"[
  {"slug":"twitch","label":"Twitch","key_url":"https://dashboard.twitch.tv/u/_/settings/stream","key_help":"Twitch Creator Dashboard → Settings → Stream → Primary Stream Key","tip":"Twitch's transcoded quality ladder (1080p / 720p / 480p / 360p / 160p) is account-tier gated - non-Affiliates get Source-Only at any bitrate, Affiliate / Partner get the ladder. In Source-Only mode every viewer must decode your full source bitrate, and above ~8 Mbps mobile devices may fail (Error #1000 / black screen with audio). Stay ≤ 8 Mbps if your audience includes mobile and you're not sure your account gets transcoded."},
  {"slug":"youtube","label":"YouTube Live","key_url":"https://studio.youtube.com/channel/UC/livestreaming","key_help":"YouTube Studio → Go live → Stream tab → Stream key","tip":"First-time live: YouTube requires a 24h verification window after enabling live streaming."},
  {"slug":"kick","label":"Kick","key_url":"https://kick.com/dashboard/settings/stream","key_help":"Kick Creator Dashboard → Settings → Stream - copy BOTH the Server URL and the Stream key","tip":"Kick gives you a Server URL and a Stream key in Settings → Stream - paste both (the Server is per-streamer, so there is no single URL to hardcode). Kick ingests over RTMPS (TLS on :443); InstantClone connects over it automatically. What Kick enforces: H.264, CBR, keyframe interval 2 s, bitrate ≤ 8000 kbps, up to 60 fps. B-frames: contrary to a lot of older guides, Kick's normal ingest accepts them - the strict no-B-frames rule is AWS IVS real-time/WHIP, which Kick doesn't use for OBS streaming, so you usually don't need to change anything. If Kick ever rejects your stream, set B-frames to 0 in OBS (Output → Advanced); that's safe for Twitch/YouTube too. InstantClone forwards one encode without re-encoding, so B-frames can't be stripped for Kick alone. And with Twitch Enhanced Broadcasting on, Twitch chooses the encode settings (including B-frames) for you."},
  {"slug":"trovo","label":"Trovo","key_url":"https://studio.trovo.live/channel/myinfo","key_help":"Trovo Studio → Channel → My Info → Stream Key","tip":null},
  {"slug":"restream","label":"Restream.io","key_url":"https://app.restream.io/channel-settings","key_help":"Restream → Channel Settings → Stream Key","tip":"Restream relays your single stream to multiple platforms - per-platform limits apply on the downstream side, not here."},
  {"slug":"custom","label":"Custom RTMP URL","key_url":null,"key_help":null,"tip":null},
  {"slug":"sink","label":"Local test sink (nothing leaves your PC)","key_url":null,"key_help":null,"tip":"InstantClone runs its own tiny RTMP receiver on this PC and streams to it - test arm / activate / cut end to end with zero risk: no real platform, no stream key, nothing leaves your machine. While it's receiving, open http://127.0.0.1:SINK_WEB_PORT/ to watch exactly what a platform would get (including the delay)."}
]"#.replace("SINK_WEB_PORT", &crate::config::SINK_WEB_PORT.to_string())
}

/// Serve the multi-track-video config endpoint OBS calls when its
/// service has a `multitrack_video_configuration_url`. The schema is
/// the 2024-06-04 revision documented in OBS's
/// `frontend/utility/models/multitrack-video.hpp` - every field name,
/// order, and the `framerate` substruct shape match what
/// `nlohmann::json::FromJson` deserialises into. Missing `config_id`
/// in the `meta` block is what rejected our first hand-written test
/// payload; OBS treats it as required even though some downstream
/// docs imply otherwise.
///
/// Query knobs (all optional):
///   * `encoder` = `x264` (default) | `nvenc` | `amd` | `qsv` - picks
///     the libobs encoder ID and an appropriate preset/profile bundle.
///   * `tracks` = 2 | 3 (default) - 1080p+720p or 1080p+720p+480p.
///   * `bandwidth` = total Kbps budget (default 10000). Split across
///     tracks with the high-rez track getting ~60 %, mid ~30 %,
///     low ~10 %.
///
/// `{stream_key}` in the `url_template` is OBS's substitution token -
/// it replaces with whatever the user typed in the Stream Key field at
/// stream start. Our ingest doesn't authenticate on the key; we just
/// accept whatever shows up after `/live/`.
/// Proxy OBS's multitrack-config POST through to Twitch's real
/// `GetClientConfiguration` endpoint and rewrite the response so the
/// stream lands at us instead of going straight to Twitch.
///
/// Flow:
///   1. OBS POSTs a JSON payload with system info + an `authentication`
///      field (the stream key the user typed in OBS's Stream Key
///      field). For a stream that's being proxied through us the
///      typed value won't authenticate with Twitch directly.
///   2. We look up the streamer's real Twitch key in our destinations
///      and string-replace the `authentication` value in the payload.
///   3. We POST the modified payload to
///      `https://ingest.twitch.tv/api/v3/GetClientConfiguration`.
///   4. Twitch returns its real tier-appropriate config (encoder
///      bitrates, track count, codec recommendations). We rewrite
///      every `url_template` field in the `ingest_endpoints` array to
///      point at our localhost RTMP ingest.
///   5. OBS encodes per Twitch's actual recommendations and sends the
///      multi-track stream to us. We forward it raw to Twitch via the
///      EB passthrough on this branch.
///
/// If anything in steps 2-4 fails (no Twitch destination configured,
/// Twitch API down, response unparseable) we fall back to the
/// hand-crafted static config from `obs_multitrack_config_static`.
/// That keeps the path usable for streamers who haven't put a Twitch
/// destination in our app yet, or who are testing without internet.
async fn obs_multitrack_config_proxy(
    body: &str,
    query: &str,
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
) -> String {
    // Enforce the InstantClone ingest key at the Enhanced Broadcasting entry
    // point. Under proxy EB, OBS publishes with the Twitch session token (which
    // we broker), so begin_publish never sees the user's key - the ONLY place it
    // is presented is the `authentication` field of THIS request (OBS puts its
    // Stream Key field there). If we don't check it here, a wrong key still gets
    // a brokered session and streams. Empty ingest key = open, same as
    // begin_publish. Strip any query the user appended, matching begin_publish.
    if !eb_request_authorized(body, &settings.borrow().ingest_key) {
        ctrl.log(
            "[OBS multitrack] rejected config request - wrong InstantClone key in the \
             OBS Stream Key field. Returning static config; the publish will be refused.",
        );
        crate::trace::log(
            "OBS_MULTITRACK",
            "wrong ingest key in request - refusing to broker an EB session",
        );
        return obs_multitrack_config_static(query, body, settings);
    }

    // OBS is back while crash protection keeps its Twitch session live on
    // the reconnect screen: hand it the same config, so it publishes the
    // same tracks with the same session token and the destination carries
    // on without a restart.
    if let Some(config) = ctrl.held_eb_config() {
        ctrl.log(
            "[OBS multitrack] crash protection: OBS is back - continuing the live \
             Twitch session",
        );
        crate::trace::log(
            "OBS_MULTITRACK",
            "hold active - reusing the live EB session",
        );
        return config;
    }

    // The streamer's real Twitch key lives in our destinations list.
    // Pick the first enabled Twitch destination with a non-empty key.
    // Also report what we found in the dashboard event log - the
    // 2026-06-01 EB test couldn't distinguish "proxy succeeded" from
    // "proxy silently fell back" because neither path was visible to
    // the user, and the symptom (Twitch's edge dropping us at ~60 s
    // because no transcoder session was provisioned via their API)
    // looked identical to a generic network drop.
    let twitch_key = {
        let s = settings.borrow();
        s.destinations
            .iter()
            .find(|d| d.enabled && d.platform == "twitch" && !d.stream_key.is_empty())
            .map(|d| d.stream_key.clone())
    };
    let Some(twitch_key) = twitch_key else {
        return obs_multitrack_config_local(body, query, ctrl, settings);
    };

    // Swap the `authentication` field in OBS's payload with the real
    // Twitch key. JSON-parser-free: the field is a flat top-level
    // string value, easy to splice on string boundaries.
    let modified_body = match replace_auth_field(body, &twitch_key) {
        Some(b) => b,
        None => {
            ctrl.log(
                "[OBS multitrack] OBS's POST body didn't expose an authentication \
                 field - schema may have changed. Returning static config. \
                 Send the next instantclone-trace.log for diagnosis.",
            );
            crate::trace::log(
                "OBS_MULTITRACK",
                "could not patch authentication field - static fallback",
            );
            return obs_multitrack_config_static(query, body, settings);
        }
    };

    // Twitch picks the main track's codec from the ones OBS offers: HEVC
    // for a 2K (1440p) channel on a GPU that has it. A horizontal
    // destination besides Twitch gets that same track, so while one is
    // enabled only H.264 is offered. Twitch keeps the vertical track's codec
    // whatever this list says, and refuses a session whose tracks don't
    // match its config, so the vertical track is forwarded as it comes.
    let needs_h264 = feeds_main_track_beyond_twitch(&settings.borrow().destinations, &twitch_key);
    let modified_body = match offer_only_h264(&modified_body) {
        Some(h264_only) if needs_h264 && h264_only != modified_body => {
            ctrl.log(
                "[OBS multitrack] asking Twitch for an H.264 main track - your \
                 other destinations can't play HEVC or AV1",
            );
            crate::trace::log("OBS_MULTITRACK", "supported_codecs narrowed to h264");
            h264_only
        }
        _ => modified_body,
    };

    let ingest_port = settings.borrow().ingest_port;

    // ureq is sync - run it on a blocking thread so we don't stall
    // the tokio runtime. 15 s outer timeout matches OBS's own
    // GetClientConfiguration timeout; if it hits the wall we fall
    // back to the static config rather than make the streamer wait.
    //
    // We capture a discriminated outcome (transport vs HTTP status
    // vs body-read vs timeout) so the dashboard log can name the
    // failure mode instead of just saying "failed". The 2026-06-01
    // EB test couldn't tell DNS vs TLS vs HTTP 4xx vs slow-response
    // apart because we threw the original error away.
    enum ProxyOutcome {
        Ok(String),
        TransportError(String),
        HttpError(u16, String),
        ReadError(String),
        Timeout,
    }
    let twitch_response = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::task::spawn_blocking(move || -> ProxyOutcome {
            let agent = crate::https::https_agent();
            // Match OBS's user-agent shape so Twitch's API doesn't
            // route us through a different code path / WAF rule than
            // the OBS client. Mostly defensive - the API is
            // documented as content-type-only auth. Timeouts go on
            // the request, not the agent, so the shared agent stays
            // reusable for different policies (webhook etc.).
            let req = agent
                .post("https://ingest.twitch.tv/api/v3/GetClientConfiguration")
                .config()
                .timeout_connect(Some(std::time::Duration::from_secs(6)))
                .timeout_global(Some(std::time::Duration::from_secs(12)))
                .build()
                .header("Content-Type", "application/json")
                .header("User-Agent", "obs-studio/32.1.2 InstantClone-proxy");
            // `http_status_as_error(false)` on the agent keeps 4xx in
            // the Ok branch so we can pull the body before deciding.
            match req.send(&modified_body) {
                Ok(resp) => {
                    let code = resp.status().as_u16();
                    let mut body = resp.into_body();
                    match body.read_to_string() {
                        Ok(s) if (200..300).contains(&code) => ProxyOutcome::Ok(s),
                        Ok(s) => ProxyOutcome::HttpError(code, s),
                        Err(e) => ProxyOutcome::ReadError(e.to_string()),
                    }
                }
                Err(e) => ProxyOutcome::TransportError(e.to_string()),
            }
        }),
    )
    .await;

    let outcome = match twitch_response {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => ProxyOutcome::TransportError(format!("spawn_blocking panic: {e}")),
        Err(_) => ProxyOutcome::Timeout,
    };

    let twitch_json = match outcome {
        ProxyOutcome::Ok(s) => s,
        ProxyOutcome::Timeout => {
            ctrl.log(
                "[OBS multitrack] Twitch GetClientConfiguration timed out after 15 s - \
                 returning static config. Twitch's API may be slow or unreachable. \
                 Try `curl -v https://ingest.twitch.tv/api/v3/GetClientConfiguration` \
                 from this machine.",
            );
            crate::trace::log("OBS_MULTITRACK", "Twitch API timed out - static fallback");
            return obs_multitrack_config_static(query, body, settings);
        }
        ProxyOutcome::HttpError(code, response) => {
            // Truncate the body so a verbose Twitch error page doesn't
            // flood the dashboard log line.
            let snippet: String = response.chars().take(300).collect();
            ctrl.log(format!(
                "[OBS multitrack] Twitch API returned HTTP {code} - returning static \
                 config. Response body (first 300 chars): {snippet}"
            ));
            crate::trace::log(
                "OBS_MULTITRACK",
                &format!("Twitch API HTTP {code} - static fallback. body={snippet}"),
            );
            return obs_multitrack_config_static(query, body, settings);
        }
        ProxyOutcome::TransportError(e) => {
            ctrl.log(format!(
                "[OBS multitrack] Twitch API transport error - returning static config. \
                 Detail: {e}. Likely DNS / TLS / connectivity."
            ));
            crate::trace::log(
                "OBS_MULTITRACK",
                &format!("Twitch API transport error: {e} - static fallback"),
            );
            return obs_multitrack_config_static(query, body, settings);
        }
        ProxyOutcome::ReadError(e) => {
            ctrl.log(format!(
                "[OBS multitrack] Twitch API responded but the body couldn't be read - \
                 returning static config. Detail: {e}."
            ));
            crate::trace::log(
                "OBS_MULTITRACK",
                &format!("Twitch API read error: {e} - static fallback"),
            );
            return obs_multitrack_config_static(query, body, settings);
        }
    };

    apply_twitch_multitrack_config(&twitch_json, &twitch_key, ingest_port, ctrl, settings)
}

/// What the proxy does with a successful Twitch response: point OBS at our
/// ingest, remember the session tokens OBS will publish with, and aim the
/// matching Twitch destination at the session's IVS endpoint. Split from the
/// HTTP call in `obs_multitrack_config_proxy` so it can be tested without the
/// network; the behaviour is unchanged.
fn apply_twitch_multitrack_config(
    twitch_json: &str,
    twitch_key: &str,
    ingest_port: u16,
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
) -> String {
    // Twitch's response has one or more `ingest_endpoints` entries
    // with `url_template` values like
    // `rtmps://<region>.contribute.live-video.net/app/{stream_key}`.
    // Replace every rtmp:// or rtmps:// URL in url_template fields with our
    // localhost ingest so OBS sends the multi-track stream to us instead. We
    // keep `{stream_key}` as the literal token - OBS substitutes it with the
    // session `authentication` token from this config. When an ingest key is
    // set that token would not match it, so we remember the token below and
    // begin_publish accepts it (see Controller::remember_eb_key).
    let rewritten = rewrite_url_templates(
        twitch_json,
        &format!("rtmp://localhost:{}/live/{{stream_key}}", ingest_port),
    );
    // Extract the *original* IVS ingest URL from Twitch's response
    // BEFORE rewriting it to localhost, substitute the streamer's real
    // stream key into the `{stream_key}` placeholder, and stash it on
    // the Twitch destination state. The egress supervisor uses this
    // override to forward the multi-track stream to the
    // session-allocated IVS endpoint instead of the configured
    // `live.twitch.tv` URL - the IVS endpoint is the only one that
    // runs the EB transcoder pipeline, so without this swap the
    // stream reaches Twitch but no transcoder picks it up, and the
    // session dies at the TCP-retransmit-timeout boundary (~60 s).
    // Sanitize and trace the full Twitch response so we can see what
    // fields it actually returned - Status block (eligibility),
    // url_template placeholders, optional authentication tokens, and
    // any error html_en_us payload. The stream key gets redacted out
    // of any url_template via simple substring replacement so the
    // trace stays shareable.
    let sanitized = twitch_json.replace(twitch_key, "<STREAM_KEY>");
    crate::trace::log(
        "OBS_MULTITRACK_RESPONSE",
        &format!("(stream key redacted) {sanitized}"),
    );

    // Twitch's API returns each ingest_endpoint with TWO fields that
    // matter for our purposes: `url_template` (the dial-time host
    // path with a `{stream_key}` placeholder) and `authentication`
    // (optional - a session-bound token like
    // `v1_<hash>_<id>_<hex_profile>_<key>` that OBS substitutes into
    // the placeholder when present). The token encodes the
    // resolutions/bitrates Twitch provisioned for this session, and
    // without it the IVS edge accepts the publish but never binds it
    // to the transcoder pipeline - which is exactly what 60 s
    // disconnects + Inspector showing "x" for resolutions told us.
    //
    // When `authentication` is set we use it as the substitution
    // value. When absent (rare - non-IVS multitrack services), fall
    // back to the user's configured Twitch stream key so we at least
    // attempt a valid auth.
    let (ivs_template, ivs_auth) = first_ingest_endpoint(twitch_json)
        .map(|e| (Some(e.url_template), e.authentication))
        .unwrap_or((None, None));
    let substitution = ivs_auth.as_deref().unwrap_or(twitch_key);
    // OBS will publish the EB stream to our ingest using an endpoint's session
    // token as the stream key (it fills the {stream_key} token in the url_template
    // we returned). Remember EVERY endpoint's token, not just the first: OBS's
    // create_service picks the first endpoint matching its RTMP/RTMPS preference
    // and publishes with THAT one's token. Twitch currently returns the same
    // token for its RTMP and RTMPS endpoints, but we don't rely on that. Each is
    // remembered so begin_publish accepts the EB publish once an ingest key is set
    // - the token and the ingest key are different by design.
    let endpoint_auths = all_ingest_endpoint_auths(twitch_json);
    for auth in &endpoint_auths {
        ctrl.remember_eb_key(auth.clone());
    }
    // Non-IVS multitrack (no token on any endpoint): OBS falls back to its Stream
    // Key field, which our proxy swapped for the real Twitch key - remember that
    // so the publish is still accepted.
    if endpoint_auths.is_empty() {
        ctrl.remember_eb_key(twitch_key.to_string());
    }
    let ivs_url = ivs_template.map(|t| t.replace("{stream_key}", substitution));
    if let Some(ivs) = ivs_url.as_ref() {
        // Apply the override to EXACTLY one Twitch destination: the
        // one whose stream key we sent in the GetClientConfiguration
        // call. The IVS session-allocated `authentication` token
        // embeds resolutions + bitrates for one stream, and the IVS
        // edge enforces it - pointing two egresses at the same URL
        // with the same token would collide on Twitch's side and at
        // most one publish would survive. Settings-driven lookup
        // matches by stream key (not by id) to be robust across
        // wizard-vs-destinations-tab key edits.
        let (chosen_id, twitch_count) = {
            let s = settings.borrow();
            let twitch_count = s
                .destinations
                .iter()
                .filter(|d| d.enabled && d.platform == "twitch")
                .count();
            let chosen = s
                .destinations
                .iter()
                .find(|d| d.enabled && d.platform == "twitch" && d.stream_key == twitch_key)
                .map(|d| d.id.clone());
            (chosen, twitch_count)
        };
        if let Some(id) = chosen_id {
            let state = ctrl.destination_state(&id);
            *state.eb_override_url.lock() = Some(ivs.clone());
            let auths = if endpoint_auths.is_empty() {
                vec![twitch_key.to_string()]
            } else {
                endpoint_auths.clone()
            };
            ctrl.remember_eb_session(crate::controller::EbSession {
                config: rewritten.clone(),
                auths,
                dest_id: id,
                ivs_url: ivs.clone(),
            });
        }
        // Clean up any stale override on OTHER Twitch destinations -
        // the proxy might have run before and left stale state from a
        // previous session shape (e.g. the user removed one Twitch
        // dest and re-added it under a new id).
        {
            let other_ids: Vec<String> = settings
                .borrow()
                .destinations
                .iter()
                .filter(|d| d.enabled && d.platform == "twitch" && d.stream_key != twitch_key)
                .map(|d| d.id.clone())
                .collect();
            for id in &other_ids {
                let state = ctrl.destination_state(id);
                *state.eb_override_url.lock() = None;
            }
        }
        if twitch_count > 1 {
            ctrl.log(format!(
                "[OBS multitrack] {} enabled Twitch destinations detected. EB \
                 transcoder ladders are session-bound to one stream key - only \
                 the first Twitch destination will receive the multi-track \
                 ladder. Other Twitch destinations stream a single flattened \
                 track to live.twitch.tv (still works, no EB transcode).",
                twitch_count
            ));
        }
        ctrl.log(format!(
            "[OBS multitrack] Twitch GetClientConfiguration call succeeded - \
             multi-track session at {} (stream key hidden). Egress will switch \
             to the IVS endpoint for this session.",
            ivs.split("/app/").next().unwrap_or(ivs)
        ));
    } else {
        ctrl.log(
            "[OBS multitrack] Twitch GetClientConfiguration call succeeded but \
             we couldn't parse the ingest URL out of the response. Egress will \
             use the configured destination URL - this typically means EB \
             will reach Twitch's edge but no transcoder session.",
        );
    }
    crate::trace::log(
        "OBS_MULTITRACK",
        "Twitch config received + rewritten to localhost ingest",
    );
    if !crate::local_eb_config::names_additional_canvas(&rewritten) {
        log_vertical_destinations_waiting(
            ctrl,
            settings,
            "Twitch's config for this channel has no vertical track (Dual Format isn't on for it)",
        );
    }
    rewritten
}

/// The two fields we care about per `ingest_endpoints[i]` entry in
/// Twitch's `GetClientConfiguration` response. `url_template` always
/// contains the raw template still holding the `{stream_key}`
/// placeholder; `authentication` is the session-bound token Twitch
/// sometimes returns (IVS multitrack always does - it encodes the
/// resolutions/bitrates the API allocated for the session, and the
/// IVS edge expects it as the substitution value, not the user's
/// regular Twitch stream key).
struct IngestEndpoint {
    url_template: String,
    authentication: Option<String>,
}

/// Whether an OBS multitrack-config request may broker an Enhanced Broadcasting
/// session. Under proxy EB, OBS publishes with the Twitch session token (not the
/// user's key), so `begin_publish` can't enforce the ingest key - the user's key
/// is presented only here, as the request's `authentication` field (OBS's Stream
/// Key field). An empty ingest key means auth is off, so everything is allowed.
/// The query is stripped to match `Controller::begin_publish`; constant-time
/// compare matches the ingest-key / password / dock-token paths.
fn eb_request_authorized(body: &str, ingest_key: &str) -> bool {
    if ingest_key.is_empty() {
        return true;
    }
    let presented = read_string_field(body, "authentication").unwrap_or_default();
    let presented = presented.split('?').next().unwrap_or("");
    crate::crypto::constant_time_eq(presented.as_bytes(), ingest_key.as_bytes())
}

/// Pull the first `ingest_endpoints[…]` entry's `url_template` and
/// optional `authentication` out of Twitch's response. The parser is
/// scoped to the substring starting at the first `"ingest_endpoints"`
/// occurrence so we don't accidentally pick up an `authentication`
/// field from some other part of the response. None if the response
/// shape doesn't expose those fields, in which case the proxy logs
/// and falls back.
fn first_ingest_endpoint(json: &str) -> Option<IngestEndpoint> {
    let arr_pos = json.find("\"ingest_endpoints\"")?;
    let after_arr = &json[arr_pos..];
    let url_template = read_string_field(after_arr, "url_template")?;
    let authentication = read_string_field(after_arr, "authentication");
    Some(IngestEndpoint {
        url_template,
        authentication,
    })
}

/// Every `authentication` token inside the `ingest_endpoints` array. OBS may
/// publish with any endpoint's token (it picks the first matching its RTMP/RTMPS
/// preference), so the broker remembers all of them. Scoped to the array - the
/// endpoint objects hold only string fields, so the first `]` after the array
/// key closes it, which keeps us from scooping an `authentication` from
/// elsewhere in the response.
fn all_ingest_endpoint_auths(json: &str) -> Vec<String> {
    let Some(start) = json.find("\"ingest_endpoints\"") else {
        return Vec::new();
    };
    let region = &json[start..];
    let region = match region.find(']') {
        Some(end) => &region[..end],
        None => region,
    };
    let needle = "\"authentication\"";
    let mut auths = Vec::new();
    let mut cursor = 0;
    while let Some(pos) = region[cursor..].find(needle) {
        let abs = cursor + pos;
        if let Some(val) = read_string_field(&region[abs..], "authentication") {
            if !val.is_empty() {
                auths.push(val);
            }
        }
        cursor = abs + needle.len();
    }
    auths
}

/// Find `"<key>": "<value>"` in a JSON-ish substring and return the
/// value. JSON-parser-free string ops: handles both compact and
/// indented styles, requires the field's value to be a plain string
/// (no embedded escaped quotes - fine for everything Twitch returns
/// in this response).
fn read_string_field(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let key_pos = json.find(needle.as_str())?;
    let after_key = &json[key_pos + needle.len()..];
    let colon_off = after_key.find(':')?;
    let after_colon = &after_key[colon_off + 1..];
    let quote_off = after_colon.find('"')?;
    let after_quote = &after_colon[quote_off + 1..];
    let end_quote_off = after_quote.find('"')?;
    Some(after_quote[..end_quote_off].to_string())
}

/// Replace the value of a top-level `"authentication"` field in a JSON
/// string with `new_value`. Returns `None` if the field can't be
/// located unambiguously - we'd rather fall back to a static config
/// than ship a malformed payload to Twitch and have the streamer
/// puzzle over an opaque 4xx.
fn replace_auth_field(json: &str, new_value: &str) -> Option<String> {
    // Match both `"authentication":"..."` and `"authentication": "..."`.
    // We don't allow embedded whitespace inside the value because OBS
    // never emits one and a multi-line value would mean we're not
    // looking at the field we think we are.
    let key_pos = json.find(r#""authentication""#)?;
    let after_key = &json[key_pos + r#""authentication""#.len()..];
    // Skip whitespace + colon.
    let colon_offset = after_key.find(':')?;
    let after_colon = &after_key[colon_offset + 1..];
    let quote_offset = after_colon.find('"')?;
    let value_start_abs = key_pos + r#""authentication""#.len() + colon_offset + 1 + quote_offset;
    let after_quote = &json[value_start_abs + 1..];
    let end_quote_offset = after_quote.find('"')?;
    let value_end_abs = value_start_abs + 1 + end_quote_offset;
    Some(format!(
        "{}\"{}\"{}",
        &json[..value_start_abs],
        new_value.replace('\\', "\\\\").replace('"', "\\\""),
        &json[value_end_abs + 1..]
    ))
}

/// Whether an enabled horizontal destination other than the Twitch one
/// holding the EB session (`session_key`) gets the main EB track. Kick and
/// most ingests only decode H.264, and so does a second Twitch account,
/// which streams outside any EB session. The local test sink takes anything.
fn feeds_main_track_beyond_twitch(destinations: &[config::Destination], session_key: &str) -> bool {
    destinations.iter().any(|d| {
        let holds_session = d.platform == "twitch" && d.stream_key == session_key;
        d.enabled && !holds_session && d.platform != "sink" && !d.wants_vertical()
    })
}

/// OBS's config request with `client.supported_codecs` narrowed to
/// `["h264"]`, so Twitch builds every track as H.264. `None` when the list
/// is missing or has no H.264 to keep; the request then goes unchanged.
fn offer_only_h264(json: &str) -> Option<String> {
    const KEY: &str = r#""supported_codecs""#;
    let after_key = &json[json.find(KEY)? + KEY.len()..];
    let list = after_key.trim_start().strip_prefix(':')?.trim_start();
    if !list.starts_with('[') {
        return None;
    }
    let list_len = list.find(']')? + 1;
    if !list[..list_len].contains(r#""h264""#) {
        return None;
    }
    let list_start = json.len() - list.len();
    Some(format!(
        r#"{}["h264"]{}"#,
        &json[..list_start],
        &json[list_start + list_len..]
    ))
}

/// Self-triggered Twitch GetClientConfiguration for VOD-audio mode.
/// Called from the supervisor when a destination has `vod_audio=true`
/// but no eb_override_url yet. We construct a minimal POST body asking
/// for a VOD-audio slot (no multi-track video unless `want_eb` is set),
/// fire it to Twitch's API, and return the session-allocated IVS URL
/// with the auth token substituted. None on any failure - the
/// supervisor logs and the next tick retries.
pub async fn fetch_twitch_vod_session(stream_key: String, want_eb: bool) -> Option<String> {
    // Minimal request body. OBS sends a much larger envelope with
    // client info, encoder caps, etc., but Twitch's API accepts the
    // shape below for the VOD-only path (no multi-track video).
    // `vod_track_audio: true` is the only knob that actually allocates
    // the VOD slot; the rest is housekeeping. If `want_eb` is set we
    // also signal multi-track video so the response carries the EB
    // ladder (the Phase C path).
    let body = if want_eb {
        format!(
            r#"{{"schema_version":"2024-06-04","authentication":"{}","preferences":{{"vod_track_audio":true,"maximum_aggregate_bitrate":10000,"maximum_video_tracks":5}},"capabilities":{{"plugin":{{"name":"InstantClone-proxy","version":"1.0.0"}}}},"client":{{"name":"obs-studio","version":"32.1.2","os":"windows"}}}}"#,
            stream_key.replace('\\', "\\\\").replace('"', "\\\"")
        )
    } else {
        format!(
            r#"{{"schema_version":"2024-06-04","authentication":"{}","preferences":{{"vod_track_audio":true,"maximum_aggregate_bitrate":8000,"maximum_video_tracks":1}},"capabilities":{{"plugin":{{"name":"InstantClone-proxy","version":"1.0.0"}}}},"client":{{"name":"obs-studio","version":"32.1.2","os":"windows"}}}}"#,
            stream_key.replace('\\', "\\\\").replace('"', "\\\"")
        )
    };
    let twitch_response = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::task::spawn_blocking(move || -> Option<String> {
            let agent = crate::https::https_agent();
            let req = agent
                .post("https://ingest.twitch.tv/api/v3/GetClientConfiguration")
                .config()
                .timeout_connect(Some(std::time::Duration::from_secs(6)))
                .timeout_global(Some(std::time::Duration::from_secs(12)))
                .build()
                .header("Content-Type", "application/json")
                .header("User-Agent", "obs-studio/32.1.2 InstantClone-proxy");
            let resp = req.send(&body).ok()?;
            if !(200..300).contains(&resp.status().as_u16()) {
                return None;
            }
            resp.into_body().read_to_string().ok()
        }),
    )
    .await
    .ok()?
    .ok()??;

    let endpoint = first_ingest_endpoint(&twitch_response)?;
    let substitution = endpoint.authentication.as_deref().unwrap_or(&stream_key);
    Some(endpoint.url_template.replace("{stream_key}", substitution))
}

/// Replace every `"url_template":"<rtmp[s]://...>"` value in a JSON
/// blob with `new_value`. Twitch's response has one or more such
/// fields (one per region they offer) - every one of them needs to
/// point at us so OBS doesn't accidentally pick a Twitch URL.
fn rewrite_url_templates(json: &str, new_value: &str) -> String {
    let mut out = String::with_capacity(json.len());
    let mut cursor = 0;
    let key = "\"url_template\"";
    while let Some(rel_pos) = json[cursor..].find(key) {
        let key_pos = cursor + rel_pos;
        // Copy everything before the key verbatim.
        out.push_str(&json[cursor..key_pos]);
        // Walk through key + `:` + whitespace + opening quote. Only a key is
        // followed by a colon: the same text as a VALUE ("note":"url_template")
        // is followed by `,` `}` or `]`, and searching on for the next colon
        // would take the next field's value for ours and drop the key between.
        // Likewise a non-string value (`null`) is not a URL to replace. In both
        // cases copy the match verbatim and keep looking.
        let after_key = &json[key_pos + key.len()..];
        let colon_off = after_key.len() - after_key.trim_start().len();
        let after_colon = after_key[colon_off..].strip_prefix(':').unwrap_or("");
        let quote_off = after_colon.len() - after_colon.trim_start().len();
        if !after_key[colon_off..].starts_with(':') || !after_colon[quote_off..].starts_with('"') {
            out.push_str(key);
            cursor = key_pos + key.len();
            continue;
        }
        let value_start_abs = key_pos + key.len() + colon_off + 1 + quote_off;
        let after_quote = &json[value_start_abs + 1..];
        let Some(end_quote_off) = after_quote.find('"') else {
            out.push_str(&json[key_pos..]);
            return out;
        };
        let value_end_abs = value_start_abs + 1 + end_quote_off;
        // Emit the key, colon, opening quote, our new value, closing
        // quote - leaving the original `{stream_key}` placeholder
        // semantics intact via the `new_value` argument the caller
        // passes in.
        out.push_str(key);
        out.push_str(": \"");
        out.push_str(new_value);
        out.push('"');
        cursor = value_end_abs + 1;
    }
    // Tail.
    out.push_str(&json[cursor..]);
    out
}

/// `body` is OBS's config request when there is one (empty for GET): it
/// names the GPU, so the ladder uses its hardware encoder.
fn obs_multitrack_config_static(
    query: &str,
    body: &str,
    settings: &Arc<watch::Sender<Settings>>,
) -> String {
    let params = config::parse_form(query);
    let available = crate::obs_register::available_video_encoders();
    let encoder = choose_encoder(query, body, available.as_deref());
    let bandwidth = config_url_bandwidth(query).unwrap_or(10000);
    let tracks: u32 = params
        .get("tracks")
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
        .clamp(1, 3);

    let ingest_port = settings.borrow().ingest_port;

    let track = |index: u32| encoder_for(&encoder, bitrate_for_track(bandwidth, tracks, index));
    let (top, middle, bottom) = (track(0), track(1), track(2));
    let enc_type = top.encoder_type;
    let (settings_json_1080, settings_json_720, settings_json_480) = (
        top.settings_json,
        middle.settings_json,
        bottom.settings_json,
    );

    // Per-call config_id: a monotonic-ish value derived from the
    // process clock means OBS treats each config-fetch as fresh
    // (matches Twitch's behaviour - they hand out a new ID each call).
    // Format doesn't matter to OBS as long as it's a non-empty string.
    let config_id = format!(
        "instantclone-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );

    // 1080p60 always present. 720p60 when tracks >= 2. 480p30 when
    // tracks == 3. The encoder_configurations array is emitted in
    // resolution-descending order - OBS reads track 0 as primary.
    let mut enc_configs = String::new();
    enc_configs.push_str(&format!(
        r#"{{"type":"{enc}","width":1920,"height":1080,"framerate":{{"numerator":60,"denominator":1}},"canvas_index":0,"settings":{s}}}"#,
        enc = enc_type,
        s = settings_json_1080,
    ));
    if tracks >= 2 {
        enc_configs.push(',');
        enc_configs.push_str(&format!(
            r#"{{"type":"{enc}","width":1280,"height":720,"framerate":{{"numerator":60,"denominator":1}},"canvas_index":0,"settings":{s}}}"#,
            enc = enc_type,
            s = settings_json_720,
        ));
    }
    if tracks >= 3 {
        enc_configs.push(',');
        enc_configs.push_str(&format!(
            r#"{{"type":"{enc}","width":854,"height":480,"framerate":{{"numerator":30,"denominator":1}},"canvas_index":0,"settings":{s}}}"#,
            enc = enc_type,
            s = settings_json_480,
        ));
    }

    format!(
        r#"{{"meta":{{"service":"InstantClone","schema_version":"2024-06-04","config_id":"{cid}"}},"ingest_endpoints":[{{"protocol":"RTMP","url_template":"rtmp://localhost:{port}/live/{{stream_key}}"}}],"encoder_configurations":[{encs}],"audio_configurations":{{"live":[{{"codec":"aac","track_id":0,"channels":2,"settings":{{"bitrate":160}}}}]}}}}"#,
        cid = config_id,
        port = ingest_port,
        encs = enc_configs,
    )
}

/// Split the user's total bandwidth budget across N tracks. With three
/// tracks the split is roughly 60 / 30 / 10 % (matches OBS's beta
/// Twitch defaults). With two tracks it's 67 / 33 %. Single track gets
/// everything. Returns an integer Kbps value clamped to ≥ 500.
fn bitrate_for_track(total_kbps: u32, tracks: u32, index: u32) -> u32 {
    let pct = match (tracks, index) {
        (1, 0) => 100,
        (2, 0) => 67,
        (2, _) => 33,
        (3, 0) => 60,
        (3, 1) => 30,
        (3, _) => 10,
        _ => 33,
    };
    ((total_kbps as u64 * pct as u64) / 100).max(500) as u32
}

fn encoder_settings_x264(bitrate: u32) -> String {
    format!(
        r#"{{"bitrate":{b},"rate_control":"CBR","keyint_sec":2,"profile":"main","preset":"veryfast"}}"#,
        b = bitrate
    )
}

fn encoder_settings_nvenc(bitrate: u32) -> String {
    format!(
        r#"{{"bitrate":{b},"rate_control":"CBR","keyint_sec":2,"profile":"main","preset":"p5","preset2":"p5","tune":"hq","multipass":"qres"}}"#,
        b = bitrate
    )
}

fn encoder_settings_amd(bitrate: u32) -> String {
    format!(
        r#"{{"bitrate":{b},"rate_control":"CBR","keyint_sec":2,"profile":"main","preset":"quality"}}"#,
        b = bitrate
    )
}

fn encoder_settings_qsv(bitrate: u32) -> String {
    format!(
        r#"{{"bitrate":{b},"rate_control":"CBR","keyint_sec":2,"profile":"main","target_usage":"balanced"}}"#,
        b = bitrate
    )
}

/// Encoder for InstantClone's own configs: the family (which settings
/// keys apply), the exact libobs id, and why, for the log.
struct EncoderChoice {
    family: &'static str,
    id: &'static str,
    reason: String,
}

/// libobs H.264 encoder ids per family, newest first. OBS 31 moved NVENC to
/// `obs_nvenc_h264_tex` and QSV to `obs_qsv11_v2`; OBS 30.2, the first with
/// Enhanced Broadcasting, only has `jim_nvenc` and `obs_qsv11`.
fn encoder_ids(family: &str) -> &'static [&'static str] {
    match family {
        "nvenc" => &["obs_nvenc_h264_tex", "jim_nvenc"],
        "amd" => &["h264_texture_amf"],
        "qsv" => &["obs_qsv11_v2", "obs_qsv11"],
        _ => &["obs_x264"],
    }
}

/// Canonical family name for a config URL's `encoder=` value.
fn encoder_family(name: &str) -> &'static str {
    match name {
        "nvenc" => "nvenc",
        "amd" => "amd",
        "qsv" => "qsv",
        _ => "x264",
    }
}

/// An explicit `encoder=` in the config URL wins, otherwise the GPU OBS
/// composites on. Either way the id must be one OBS's log lists as
/// available: naming an encoder OBS doesn't have stops the stream from
/// starting at all, which is worse than x264 being heavy. `available` is
/// None when no OBS log could be read; then an explicit choice is trusted
/// and a guessed one falls back to x264.
fn choose_encoder(query: &str, body: &str, available: Option<&[String]>) -> EncoderChoice {
    let x264 = |reason: String| EncoderChoice {
        family: "x264",
        id: "obs_x264",
        reason,
    };
    let explicit = config::parse_form(query)
        .get("encoder")
        .map(|name| encoder_family(name));
    let (family, source) = match explicit {
        Some(family) => (family, "set in the config URL"),
        None => match crate::local_eb_config::gpu_encoder_family(body) {
            Some(family) => (family, "the GPU OBS runs on"),
            None => return x264("OBS reported no NVIDIA, AMD or Intel GPU".into()),
        },
    };
    if family == "x264" {
        return x264(source.into());
    }
    let listed = |id: &str| available.is_some_and(|list| list.iter().any(|e| e == id));
    match (
        available,
        encoder_ids(family).iter().copied().find(|id| listed(id)),
    ) {
        (_, Some(id)) => EncoderChoice {
            family,
            id,
            reason: format!("{source}; {id} is available in OBS"),
        },
        // Can't check: the oldest id, which every OBS with Enhanced
        // Broadcasting still registers (newer ones as a compatibility alias).
        (None, None) if explicit.is_some() => EncoderChoice {
            family,
            id: encoder_ids(family).last().copied().unwrap_or("obs_x264"),
            reason: source.into(),
        },
        (Some(_), None) => x264(format!("OBS lists no {family} encoder")),
        (None, None) => x264("OBS's log couldn't be read to check GPU encoders".into()),
    }
}

/// `bandwidth` from the config URL's query, in kbps, when it has one.
fn config_url_bandwidth(query: &str) -> Option<u32> {
    config::parse_form(query)
        .get("bandwidth")
        .and_then(|s| s.parse::<u32>().ok())
        .map(|kbps| kbps.clamp(1500, 50000))
}

/// Log, once per config request, which enabled vertical destinations will
/// get nothing this stream and why, so a streamer isn't left guessing.
fn log_vertical_destinations_waiting(
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    why: &str,
) {
    let names: Vec<String> = settings
        .borrow()
        .destinations
        .iter()
        .filter(|d| d.enabled && d.wants_vertical())
        .map(|d| d.name.clone())
        .collect();
    if names.is_empty() {
        return;
    }
    ctrl.log(format!(
        "[OBS multitrack] {} set to Vertical, but {why}, so it gets nothing this stream.",
        names.join(", ")
    ));
}

/// Encoder id and settings for one track. x264 uses `preset` strings like
/// "veryfast"; nvenc uses "p1"-"p7"; AMD uses similar tier names.
/// `profile=main` is what every Twitch / YouTube / Kick decoder accepts;
/// baseline would drop B-frames entirely and main+high are functionally
/// equivalent on the wire for ~1080p60.
fn encoder_for(choice: &EncoderChoice, kbps: u32) -> crate::local_eb_config::TrackEncoder {
    let settings_json = match choice.family {
        "nvenc" => encoder_settings_nvenc(kbps),
        "amd" => encoder_settings_amd(kbps),
        "qsv" => encoder_settings_qsv(kbps),
        _ => encoder_settings_x264(kbps),
    };
    crate::local_eb_config::TrackEncoder {
        encoder_type: choice.id.into(),
        settings_json,
    }
}

/// No Twitch destination, so there is no Twitch config to proxy: build one
/// from the canvases OBS described (see `local_eb_config`). The main canvas
/// gets one track, and OBS's Additional canvas gets one when it sends one,
/// which is what feeds destinations set to Vertical. Falls back to the
/// static ladder when OBS didn't describe its canvases.
fn obs_multitrack_config_local(
    body: &str,
    query: &str,
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
) -> String {
    let offered = crate::local_eb_config::parse_canvases(body);
    let canvases = canvases_to_encode(&offered, &settings.borrow().destinations);
    if canvases.len() < offered.len() {
        ctrl.log(
            "[OBS multitrack] OBS's Additional canvas is not encoded: no destination is set \
             to Vertical",
        );
    }
    let available = crate::obs_register::available_video_encoders();
    let choice = choose_encoder(query, body, available.as_deref());
    // Explicit URL value, then OBS's own bandwidth cap, then a default
    // that keeps the main track at a normal 1080p60 bitrate.
    let bandwidth = config_url_bandwidth(query)
        .or_else(|| crate::local_eb_config::requested_budget_kbps(body))
        .unwrap_or_else(|| crate::local_eb_config::default_budget_kbps(&canvases));
    // The main track is what OBS streams with Enhanced Broadcasting off, so
    // turning EB on changes nothing about it (see `StreamerSettings`).
    let streamer = crate::obs_register::active_stream_settings();
    match &streamer {
        Some(own) => ctrl.log(format!(
            "[OBS multitrack] main track uses your OBS stream settings: {}, {} kbps",
            own.encoder
                .as_ref()
                .map_or("your bitrate with InstantClone's encoder", |e| {
                    e.encoder_type.as_str()
                }),
            own.bitrate_kbps
        )),
        None => ctrl.log(
            "[OBS multitrack] couldn't read your OBS stream settings (Settings > Output) - \
             using InstantClone's defaults",
        ),
    }
    let own_encoder = streamer.as_ref().and_then(|own| own.encoder.as_ref());
    let vertical_own = own_encoder_for_vertical(own_encoder, &settings.borrow().destinations);
    if canvases.len() > 1 {
        ctrl.log(match vertical_own {
            Some(own) => format!(
                "[OBS multitrack] vertical track uses your encoder ({})",
                own.codec().label()
            ),
            None => "[OBS multitrack] vertical track uses H.264, which every vertical \
                     destination plays"
                .to_string(),
        });
    }
    let ingest_port = settings.borrow().ingest_port;
    let built = crate::local_eb_config::build(
        &canvases,
        bandwidth,
        streamer.as_ref(),
        ingest_port,
        |kbps| picked_encoder(vertical_own, &choice, kbps),
    );
    let Some((config, plan)) = built else {
        ctrl.log(
            "[OBS multitrack] no Twitch destination, and OBS didn't describe its canvases - \
             using the default 1080p ladder.",
        );
        crate::trace::log(
            "OBS_MULTITRACK",
            "no twitch destination, no canvases - static fallback",
        );
        return obs_multitrack_config_static(query, body, settings);
    };
    let budget = match streamer {
        Some(_) => String::new(),
        None => format!("{bandwidth} kbps total, "),
    };
    ctrl.log(format!(
        "[OBS multitrack] no Twitch destination - building the config here: {}, \
         {budget}InstantClone's encoder: {} ({})",
        plan.describe(),
        choice.family,
        choice.reason
    ));
    crate::trace::log("OBS_MULTITRACK", "local config built from OBS's canvases");
    if !plan.feeds_vertical() {
        log_vertical_destinations_waiting(
            ctrl,
            settings,
            "OBS sent no 9:16 canvas (in OBS: Settings → Stream → Enhanced Broadcasting → \
             Additional canvas, pick your vertical canvas, for example Aitum Vertical)",
        );
    }
    config
}

/// The canvases OBS offered that are worth encoding. The Additional canvas
/// only feeds destinations set to Vertical, so with none enabled it would
/// cost a whole encode (and, on the defaults, a share of the bitrate) for a
/// track nobody receives.
fn canvases_to_encode(
    offered: &[crate::local_eb_config::ObsCanvas],
    destinations: &[config::Destination],
) -> Vec<crate::local_eb_config::ObsCanvas> {
    let streams_vertical = destinations.iter().any(|d| d.enabled && d.wants_vertical());
    let keep = if streams_vertical { offered.len() } else { 1 };
    offered.iter().take(keep).copied().collect()
}

/// The encoder for a track InstantClone fills in: the streamer's own at
/// `kbps` when it suits every vertical destination (see
/// `own_encoder_for_vertical`), otherwise InstantClone's choice.
fn picked_encoder(
    own_for_vertical: Option<&crate::local_eb_config::TrackEncoder>,
    choice: &EncoderChoice,
    kbps: u32,
) -> crate::local_eb_config::TrackEncoder {
    match own_for_vertical {
        Some(own) => own.at_bitrate(kbps),
        None => encoder_for(choice, kbps),
    }
}

/// The streamer's own encoder when every enabled vertical destination plays
/// its codec (HEVC to YouTube, say), so the vertical track matches the main
/// one; None means InstantClone's H.264, which plays everywhere.
fn own_encoder_for_vertical<'a>(
    own: Option<&'a crate::local_eb_config::TrackEncoder>,
    destinations: &[config::Destination],
) -> Option<&'a crate::local_eb_config::TrackEncoder> {
    own.filter(|own| {
        let codec = own.codec();
        destinations
            .iter()
            .filter(|d| d.enabled && d.wants_vertical())
            .all(|d| d.plays(codec))
    })
}

fn twitch_ingests_json() -> String {
    let mut out = String::from("[");
    for (i, (slug, label)) in config::twitch_ingests().iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(r#"{{"slug":"{}","label":"{}"}}"#, slug, label));
    }
    out.push(']');
    out
}

/// Exact config keys `POST /config` will write, besides the `hotkey.<action>`
/// and `midi.<action>` families. Every other field has its own route
/// (destinations, auth, profiles) or is not user-settable at all, and the
/// default is refusal.
///
/// This list and `apply_field_str` must cover the same keys: a key allowed
/// here that the applier does not know is silently dropped, which is how the
/// MIDI clear button spent 0.1.14 pretending to work. A named list rather than
/// a `matches!` so a test can walk every key through save and load.
const SETTABLE_KEYS: &[&str] = &[
    "ingest_port",
    "ingest_bind_all",
    "ingest_key",
    "web_port",
    "web_bind_all",
    "buffer_mb",
    "buffer_path",
    "overlays_dir",
    "tracing_enabled",
    "auto_arm_on_connect",
    "auto_activate_when_ready",
    "auto_arm_delay_ms",
    "update_check_enabled",
    "open_dashboard_on_launch",
    "midi_device",
    "twitch_client_id",
];

/// Whether `POST /config` may write key `k`. See `SETTABLE_KEYS`.
fn is_settable_key(k: &str) -> bool {
    SETTABLE_KEYS.contains(&k)
        || k.starts_with("hotkey.")
        || k.starts_with("midi.")
        || k.starts_with(crate::crash_protection::KEY_PREFIX)
}

async fn post_config(
    body: &str,
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) -> (&'static str, &'static str, String) {
    let form = config::parse_form(body);
    // Held across the whole clone -> mutate -> save -> send below so a
    // concurrent POST can't clobber this write. See SETTINGS_WRITE_LOCK.
    let _wl = settings_write_guard();
    let mut new_settings = settings.borrow().clone();

    // Network + buffer + overlay-dir + webhook URL are applied directly.
    // EXCEPT the webhook: an empty submission means "keep the existing
    // value". The dashboard leaves the field blank for security (so the
    // server-side redacted value isn't shown to the user), so any empty
    // POST without an explicit "delete webhook" intent must be a no-op
    // for that field - otherwise saving any other setting would wipe it.
    for (k, v) in form.iter() {
        if is_settable_key(k) {
            apply_field_str(&mut new_settings, k, v);
        }
    }

    // Autostart lives in the registry, not in Settings, so it is applied
    // here rather than through `apply_field_str`. A failure is logged and
    // surfaced but must not abort the save - the rest of the settings the
    // user just edited are unrelated and should still land.
    if let Some(v) = form.get("start_with_windows") {
        let want = v == "on" || v == "true";
        if let Err(e) = crate::autostart::set(want) {
            ctrl.log(format!(
                "start with Windows: could not {} the startup entry - {e}",
                if want { "create" } else { "remove" }
            ));
        }
    }

    // Backward-compat wizard fields: when the wizard POSTs
    // platform/stream_key/custom_egress_url, write them into
    // destinations[0] (creating "Main" if the list is empty). This keeps
    // the first-run setup flow working without UI changes.
    let wizard_platform = form.get("platform").cloned();
    let wizard_key = form.get("stream_key").cloned();
    let wizard_custom = form.get("custom_egress_url").cloned();
    if wizard_platform.is_some() || wizard_key.is_some() || wizard_custom.is_some() {
        if new_settings.destinations.is_empty() {
            new_settings.destinations.push(config::Destination {
                id: "main".into(),
                name: "Main".into(),
                enabled: true,
                platform: "twitch".into(),
                stream_key: String::new(),
                custom_egress_url: String::new(),
                twitch_ingest: String::new(),
                youtube_ingest: String::new(),
                vod_audio: false,
                vod_audio_inject_eb: false,
                stream_format: "horizontal".into(),
                audio_track: "auto".into(),
            });
        }
        let d = &mut new_settings.destinations[0];
        if let Some(v) = wizard_platform {
            d.platform = v;
        }
        if let Some(v) = wizard_key {
            if !v.is_empty() {
                d.stream_key = v;
            }
        }
        if let Some(v) = wizard_custom {
            d.custom_egress_url = v;
        }
        // The Twitch step of the wizard can opt into VOD audio mode. Only
        // meaningful for Twitch; the wizard only shows the toggle there, so
        // whatever it posts is already platform-correct.
        if let Some(v) = form.get("vod_audio") {
            d.vod_audio = v == "on" || v == "true";
        }
    }

    let errors = new_settings.validate();
    if !errors.is_empty() {
        let msg = errors.join("; ");
        return (
            "400 Bad Request",
            "application/json",
            format!(r#"{{"ok":false,"error":"{}"}}"#, json_escape(&msg)),
        );
    }

    let old = settings.borrow().clone();
    let needs_restart =
        old.buffer_mb != new_settings.buffer_mb || old.buffer_path != new_settings.buffer_path;

    // Mark as configured the moment we have at least one usable destination.
    if has_streamable_dest(&new_settings) {
        new_settings.configured = true;
    }

    if let Err(e) = new_settings.save(cfg_path) {
        return (
            "500 Internal Server Error",
            "application/json",
            format!(
                r#"{{"ok":false,"error":"save failed: {}"}}"#,
                json_escape(&e.to_string())
            ),
        );
    }
    // Same for the ingest key: mirror it inline so locking down the ingest port
    // takes effect this instant. Waiting for the supervisor tick would leave a
    // brief window where begin_publish still enforces the previous (usually
    // empty) key and accepts any publisher - exactly the exposure the user just
    // moved to close.
    ctrl.update_ingest_key(new_settings.ingest_key.clone());
    // Flip the trace switch right away so a toggle in the System tab
    // takes effect this instant - no need to wait for a restart.
    crate::trace::set_enabled(new_settings.tracing_enabled);
    let _ = settings.send(new_settings.clone());

    let restart_msg = if needs_restart {
        ",\"restart_required\":true,\"restart_reason\":\"buffer size/path changed\""
    } else {
        ""
    };
    (
        "200 OK",
        "application/json",
        format!(r#"{{"ok":true{}}}"#, restart_msg),
    )
}

/// Reset the persisted config to defaults. Two scopes:
///
/// - `scope=settings`: app-level knobs (ports, buffer, webhook,
///   overlays dir, diagnostics) go back to defaults. Destinations,
///   profiles, and the `configured` flag stay so the user doesn't get
///   booted back into the wizard or lose stream keys.
/// - `scope=all`: full `Settings::defaults()` - destinations and
///   profiles are wiped, `configured=false` so the next page load
///   shows the wizard. The OBS service registration in
///   `services.json` is intentionally NOT touched here: it lives
///   outside our config and has its own surface on the OBS tab.
///
/// In both cases the controller's webhook + trace toggle are
/// updated in-process so the change is immediate, not next-restart.
async fn post_config_reset(
    query: &str,
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) -> (&'static str, &'static str, String) {
    let scope = config::parse_form(query)
        .get("scope")
        .cloned()
        .unwrap_or_else(|| "settings".to_string());
    let _wl = settings_write_guard();
    let mut next = Settings::defaults();
    if scope == "settings" {
        // Carry over the user's stream destinations and profiles -
        // a settings reset must not silently lose their stream keys.
        let prev = settings.borrow().clone();
        next.destinations = prev.destinations;
        next.profiles = prev.profiles;
        next.configured = prev.configured;
        // Integrations and their connections are the user's own work too.
        next.integrations = prev.integrations;
        next.discord_channels = prev.discord_channels;
        next.phone = prev.phone;
        next.integrations_migrated = prev.integrations_migrated;
        // Resetting it would silently log out of Twitch.
        next.twitch_client_id = prev.twitch_client_id;
    } else if scope != "all" {
        return (
            "400 Bad Request",
            "application/json",
            r#"{"ok":false,"error":"unknown scope (use 'settings' or 'all')"}"#.to_string(),
        );
    }
    if let Err(e) = next.save(cfg_path) {
        return (
            "500 Internal Server Error",
            "application/json",
            format!(
                r#"{{"ok":false,"error":"save failed: {}"}}"#,
                json_escape(&e.to_string())
            ),
        );
    }
    crate::trace::set_enabled(next.tracing_enabled);
    if scope == "all" {
        // Nuke the controller's live delay state too. Settings on
        // disk going back to 0 isn't enough - the in-memory atoms
        // would otherwise keep an armed delay alive past the reset
        // and confuse the wizard reload. clear_logs makes the
        // event-log tab match the "fresh install" feel.
        ctrl.arm_delay(0);
        ctrl.clear_logs();
        // Wipe the Studio overlays (from the still-current dir, before the
        // send below swaps in defaults). The seeded flag is back to false in
        // `next`, so the dashboard re-bakes the presets on its next load.
        wipe_studio_overlays(&settings.borrow().overlays_dir);
        // A factory reset forgets the Twitch logins as well.
        if let Some(integrations) = ctrl.integrations() {
            integrations.twitch_logout(crate::integrations::twitch::Which::Main);
            integrations.twitch_logout(crate::integrations::twitch::Which::Bot);
        }
    }
    ctrl.log(format!("config reset (scope={})", scope));
    reconcile_obs_vod_files(&next, ctrl);
    let _ = settings.send(next);
    (
        "200 OK",
        "application/json",
        format!(r#"{{"ok":true,"scope":"{}"}}"#, scope),
    )
}

/// Legacy one-shot delay endpoint - semantically the same as arming and
/// immediately activating. Used by Stream Deck / API integrations that
/// don't care about the two-phase UX.
async fn post_delay(
    body: &str,
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
    sysstat: &Arc<SysStat>,
) -> (&'static str, &'static str, String) {
    let form = config::parse_form(body);
    let ms: u32 = form.get("ms").and_then(|v| v.parse().ok()).unwrap_or(0);
    let ms = ms.min(600_000);
    if let Some(refusal) = arm_refusal(ctrl, settings, ms) {
        return refusal;
    }
    ctrl.arm_delay(ms);
    if ms > 0 {
        // Force activate even if buffer hasn't built - controller will
        // hold at live edge until it has, with buffer_building=true.
        let _ = ctrl.activate_delay();
    }
    persist_delay_state(ctrl, settings, cfg_path);
    (
        "200 OK",
        "application/json",
        state_json(ctrl, settings, sysstat),
    )
}

// ---- Two-phase delay endpoints ----

/// Why this arm cannot be granted, as a ready-to-send response - or None
/// when it can. Shared by `/arm` and `/delay`, which ask the same question
/// and used to answer it twice.
///
/// `ms == 0` is disarm and is always allowed.
fn arm_refusal(
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    ms: u32,
) -> Option<(&'static str, &'static str, String)> {
    if ms == 0 {
        return None;
    }
    // Nothing publishing: the buffer cannot fill, so arming would sit in
    // "preparing" forever. The hotkey and MIDI paths refuse this too, so every
    // surface behaves the same. Auto-arm-on-connect is unaffected: it fires on
    // the connect itself, when a publisher is already there.
    if !ctrl.ingest_alive() {
        return Some(conflict(crate::controller::NO_INGEST));
    }
    // Capacity guard. A delay bigger than the ring can hold at the current
    // bitrate never fills - it stalls in "arming" forever, which looks like a
    // hang. The dashboard and dock both gate this client-side, but a stale
    // page, a second dock, or a scripted call could still ask for the
    // impossible. Same estimate we publish as buffer_capacity_ms_est; bitrate
    // is floored at 2 Mbps so a low-bitrate stream stays generous.
    let cap_ms = {
        let s = settings.borrow();
        let kbps = ctrl.bitrate_kbps().max(2_000) as u64;
        (s.buffer_mb * 1024 * 1024 * 8 / kbps) as u32
    };
    if cap_ms > 0 && ms > cap_ms {
        return Some(conflict(&format!(
            "Buffer too small for {}s - it holds about {}s at the current bitrate. \
             Raise the buffer size in the dashboard.",
            ms / 1000,
            cap_ms / 1000
        )));
    }
    None
}

/// A 409 carrying one human-readable reason.
fn conflict(error: &str) -> (&'static str, &'static str, String) {
    (
        "409 Conflict",
        "application/json",
        format!(r#"{{"ok":false,"error":"{}"}}"#, json_escape(error)),
    )
}

async fn post_arm(
    body: &str,
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
    sysstat: &Arc<SysStat>,
) -> (&'static str, &'static str, String) {
    let form = config::parse_form(body);
    let ms: u32 = form
        .get("ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
        .min(600_000);

    if let Some(refusal) = arm_refusal(ctrl, settings, ms) {
        return refusal;
    }
    ctrl.arm_delay(ms);
    persist_delay_state(ctrl, settings, cfg_path);
    (
        "200 OK",
        "application/json",
        state_json(ctrl, settings, sysstat),
    )
}

async fn post_activate(
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
    sysstat: &Arc<SysStat>,
) -> (&'static str, &'static str, String) {
    match ctrl.activate_delay() {
        Ok(_) => {
            persist_delay_state(ctrl, settings, cfg_path);
            (
                "200 OK",
                "application/json",
                state_json(ctrl, settings, sysstat),
            )
        }
        Err(e) => (
            "409 Conflict",
            "application/json",
            format!(r#"{{"ok":false,"error":"{}"}}"#, json_escape(&e.message())),
        ),
    }
}

async fn post_stop(
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
    sysstat: &Arc<SysStat>,
) -> (&'static str, &'static str, String) {
    ctrl.stop_delay();
    persist_delay_state(ctrl, settings, cfg_path);
    (
        "200 OK",
        "application/json",
        state_json(ctrl, settings, sysstat),
    )
}

async fn post_disarm(
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
    sysstat: &Arc<SysStat>,
) -> (&'static str, &'static str, String) {
    ctrl.arm_delay(0); // arm(0) also resets target
    persist_delay_state(ctrl, settings, cfg_path);
    (
        "200 OK",
        "application/json",
        state_json(ctrl, settings, sysstat),
    )
}

// ---- "Cut after this airs" (scheduled safe cut) ----
//
// No persist_delay_state here: scheduling doesn't change armed/target,
// and when the mark fires it goes through the same stop_delay path the
// supervisor behaviours use - the next explicit delay action persists.

async fn post_cut_after(
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    sysstat: &Arc<SysStat>,
) -> (&'static str, &'static str, String) {
    match ctrl.schedule_safe_cut() {
        Ok(_) => (
            "200 OK",
            "application/json",
            state_json(ctrl, settings, sysstat),
        ),
        Err(e) => (
            "409 Conflict",
            "application/json",
            format!(r#"{{"ok":false,"error":"{}"}}"#, json_escape(e)),
        ),
    }
}

async fn post_cut_after_cancel(
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    sysstat: &Arc<SysStat>,
) -> (&'static str, &'static str, String) {
    ctrl.cancel_safe_cut();
    (
        "200 OK",
        "application/json",
        state_json(ctrl, settings, sysstat),
    )
}

/// Stand global hotkeys down while the dashboard records a combo, and put
/// them back when it is done (`on=0`).
///
/// Without this, recording is impossible for any combo that is already
/// bound: `RegisterHotKey` takes the keypress system-wide, so the browser
/// never sees it and the action fires instead. The backend holds a deadline
/// rather than a flag, so a dashboard that is closed mid-capture cannot
/// leave the user with no hotkeys.
#[cfg(windows)]
async fn post_hotkey_capture(
    body: &str,
    ctrl: &Arc<Controller>,
) -> (&'static str, &'static str, String) {
    /// Long enough for someone to think about which combo they want, short
    /// enough that a browser that vanishes costs one window and no more.
    const CAPTURE_WINDOW_MS: u32 = 30_000;

    let form = config::parse_form(body);
    let on = !matches!(
        form.get("on").map(String::as_str).unwrap_or("1"),
        "" | "0" | "false" | "off"
    );
    ctrl.suspend_hotkeys(if on { CAPTURE_WINDOW_MS } else { 0 });
    #[cfg(windows)]
    crate::tray::request_hotkey_reload();
    ("200 OK", "application/json", r#"{"ok":true}"#.into())
}

// ---- MIDI mapping ----
//
// The listener thread (see crate::midi) receives controller messages and,
// in learn mode, captures the next one for a chosen action. Because those
// events land at the backend, the whole learn flow is server-side and the
// dashboard drives it over these routes.

/// Arm learn mode for one action: the next MIDI press is captured for it.
async fn post_midi_learn(
    body: &str,
    ctrl: &Arc<Controller>,
) -> (&'static str, &'static str, String) {
    let form = config::parse_form(body);
    let action = form.get("action").map(String::as_str).unwrap_or("");
    if !config::ACTIONS.contains(&action) {
        return (
            "400 Bad Request",
            "application/json",
            r#"{"ok":false,"error":"unknown action"}"#.into(),
        );
    }
    // Deliberately `connected` and not `available`: nothing is held open
    // until a binding exists, so gating on "a device is open" would make the
    // very first binding impossible to record. Arming the learn is what
    // causes the device to be opened.
    if !ctrl.midi().connected() {
        // Two different problems wear the same "not available" flag, and
        // telling someone to connect a controller they can see plugged in
        // is the least useful thing we could say.
        let selected = ctrl.midi().selected_device();
        let error = if selected.is_empty() {
            "No MIDI device detected. Connect a controller and try again.".to_string()
        } else {
            format!("\"{selected}\" isn't connected. Plug it in, or pick every device again.")
        };
        // Escaped once, over the whole message: the device name is quoted
        // inside it, and those quotes would otherwise close the JSON string.
        return (
            "409 Conflict",
            "application/json",
            format!(r#"{{"ok":false,"error":"{}"}}"#, json_escape(&error)),
        );
    }
    ctrl.midi().start_learn(action);
    ("200 OK", "application/json", r#"{"ok":true}"#.into())
}

/// Drop a pending learn without binding anything.
async fn post_midi_learn_cancel(ctrl: &Arc<Controller>) -> (&'static str, &'static str, String) {
    ctrl.midi().cancel_learn();
    ("200 OK", "application/json", r#"{"ok":true}"#.into())
}

/// Poll during learn: commit a freshly-captured binding (through the same
/// guarded save path the settings form uses) and return the runtime state
/// the dashboard renders (device availability, names, and what is learning).
async fn post_midi_poll(
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) -> (&'static str, &'static str, String) {
    if let Some((action, signature)) = ctrl.midi().take_captured() {
        let _wl = settings_write_guard();
        let mut new_settings = settings.borrow().clone();
        new_settings.midi.set(&action, &signature);
        if new_settings.save(cfg_path).is_ok() {
            ctrl.midi().update_from_settings(&new_settings);
            let _ = settings.send(new_settings);
        }
    }
    ("200 OK", "application/json", ctrl.midi().to_json())
}

/// Write the current delay state (armed / target, and the "last manually
/// armed" preference) back to the config file. Called by every route that
/// moves it, and by the runtime on behalf of the hotkey / MIDI paths, which
/// run outside the web layer entirely.
pub(crate) fn persist_delay_state(
    ctrl: &Controller,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) {
    let _wl = settings_write_guard();
    let mut ns = settings.borrow().clone();
    let armed = ctrl.armed_delay_ms();
    let target = ctrl.target_delay_ms();
    // Track "last manually armed delay" in auto_arm_delay_ms so the
    // System -> General auto-arm picks up wherever the streamer last
    // explicitly armed. Only updates on non-zero arm so a Disarm
    // (arm_delay(0)) doesn't wipe the preference.
    let new_auto_arm = if armed > 0 { Some(armed) } else { None };
    let auto_arm_changed = match new_auto_arm {
        Some(v) => ns.auto_arm_delay_ms != v,
        None => false,
    };
    if ns.armed_delay_ms != armed || ns.target_delay_ms != target || auto_arm_changed {
        ns.armed_delay_ms = armed;
        ns.target_delay_ms = target;
        if let Some(v) = new_auto_arm {
            ns.auto_arm_delay_ms = v;
        }
        let _ = ns.save(cfg_path);
        let _ = settings.send(ns);
    }
}

// ---- Profiles ----

fn profiles_json(settings: &Arc<watch::Sender<Settings>>) -> String {
    let s = settings.borrow();
    let mut out = String::from("[");
    for (i, p) in s.profiles.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            r#"{{"name":"{}","delay_ms":{}}}"#,
            json_escape(&p.name),
            p.delay_ms
        ));
    }
    out.push(']');
    out
}

async fn post_profile_add(
    body: &str,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) -> (&'static str, &'static str, String) {
    let form = config::parse_form(body);
    let name = form.get("name").cloned().unwrap_or_default();
    let delay_ms: u32 = form
        .get("delay_ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if name.trim().is_empty() {
        return (
            "400 Bad Request",
            "application/json",
            r#"{"ok":false,"error":"name required"}"#.into(),
        );
    }
    let _wl = settings_write_guard();
    let mut ns = settings.borrow().clone();
    // Replace existing by name, else append.
    if let Some(p) = ns.profiles.iter_mut().find(|p| p.name == name) {
        p.delay_ms = delay_ms;
    } else {
        ns.profiles.push(config::DelayProfile { name, delay_ms });
    }
    let _ = ns.save(cfg_path);
    let _ = settings.send(ns);
    ("200 OK", "application/json", profiles_json(settings))
}

async fn post_profile_del(
    body: &str,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) -> (&'static str, &'static str, String) {
    let form = config::parse_form(body);
    let name = form.get("name").cloned().unwrap_or_default();
    let _wl = settings_write_guard();
    let mut ns = settings.borrow().clone();
    ns.profiles.retain(|p| p.name != name);
    let _ = ns.save(cfg_path);
    let _ = settings.send(ns);
    ("200 OK", "application/json", profiles_json(settings))
}

// ---- Logs viewer ----

fn logs_json(ctrl: &Controller) -> String {
    let q = ctrl.logs.lock();
    let mut out = String::from(r#"{"lines":["#);
    for (i, line) in q.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(&json_escape(line));
        out.push('"');
    }
    out.push_str("]}");
    out
}

async fn test_egress(
    settings: &Arc<watch::Sender<Settings>>,
) -> (&'static str, &'static str, String) {
    let url_str = match settings.borrow().egress_url() {
        Some(u) => u,
        None => {
            return (
                "200 OK",
                "application/json",
                r#"{"ok":false,"error":"set platform + stream key first"}"#.into(),
            );
        }
    };
    let parsed = match EgressUrl::parse(&url_str) {
        Ok(p) => p,
        Err(e) => {
            return (
                "200 OK",
                "application/json",
                format!(
                    r#"{{"ok":false,"error":"{}"}}"#,
                    json_escape(&e.to_string())
                ),
            )
        }
    };
    // DNS + TCP connect with 3 s timeout. We deliberately don't run the
    // full RTMP handshake - that would burn a "slot" on the platform.
    let connect = async {
        let _addrs: Vec<_> = (parsed.host.as_str(), parsed.port)
            .to_socket_addrs()
            .map(|i| i.collect())
            .unwrap_or_default();
        TcpStream::connect((parsed.host.as_str(), parsed.port)).await
    };
    let res = tokio::time::timeout(Duration::from_secs(3), connect).await;
    let payload = match res {
        Ok(Ok(_)) => format!(
            r#"{{"ok":true,"message":"reached {}:{}"}}"#,
            parsed.host, parsed.port
        ),
        Ok(Err(e)) => format!(
            r#"{{"ok":false,"error":"{}"}}"#,
            json_escape(&e.to_string())
        ),
        Err(_) => r#"{"ok":false,"error":"timed out after 3s"}"#.into(),
    };
    ("200 OK", "application/json", payload)
}

// ---- Helpers ----

// ----------------------------------------------------------------------
// Destinations CRUD
// ----------------------------------------------------------------------
//
// POST /destinations with form fields:
//   id (optional)  - if present and matches existing, edit; else create new
//   name           - display label
//   enabled        - "on"/"off"
//   platform       - slug
//   stream_key     - empty string leaves existing untouched (security)
//   custom_egress_url
//
// POST /destinations/delete with `id=<id>` to remove.

async fn post_destination_upsert(
    body: &str,
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) -> (&'static str, &'static str, String) {
    let form = config::parse_form(body);
    let id = form.get("id").cloned().unwrap_or_else(generate_dest_id);
    let name = form.get("name").cloned().unwrap_or_default();
    let enabled = matches!(
        form.get("enabled").map(String::as_str),
        Some("on" | "true" | "1")
    );
    let platform = form
        .get("platform")
        .cloned()
        .unwrap_or_else(|| "twitch".into());
    let stream_key = form.get("stream_key").cloned().unwrap_or_default();
    let custom = form.get("custom_egress_url").cloned().unwrap_or_default();
    let twitch_ingest = form.get("twitch_ingest").cloned().unwrap_or_default();
    let youtube_ingest = form.get("youtube_ingest").cloned().unwrap_or_default();
    let vod_audio = matches!(
        form.get("vod_audio").map(String::as_str),
        Some("on" | "true" | "1")
    );
    // Present only when a caller explicitly sends it (the dedicated EB-inject
    // flow does; the dashboard's save/toggle deliberately don't). `None` means
    // "leave it alone" on an edit, so a plain save or enable/disable toggle
    // can't silently blank this Twitch EB-inject flag; a fresh insert falls
    // back to false.
    let vod_audio_inject_eb = form
        .get("vod_audio_inject_eb")
        .map(|v| matches!(v.as_str(), "on" | "true" | "1"));
    let stream_format =
        normalize_stream_format(&platform, form.get("stream_format").map(String::as_str));
    let audio_track = normalize_audio_track(form.get("audio_track").map(String::as_str));

    if name.trim().is_empty() {
        return (
            "400 Bad Request",
            "application/json",
            r#"{"ok":false,"error":"name required"}"#.into(),
        );
    }

    let _wl = settings_write_guard();
    let mut ns = settings.borrow().clone();
    if let Some(existing) = ns.destinations.iter_mut().find(|d| d.id == id) {
        existing.name = name;
        existing.enabled = enabled;
        existing.platform = platform;
        if !stream_key.is_empty() {
            existing.stream_key = stream_key;
        }
        existing.custom_egress_url = custom;
        existing.twitch_ingest = twitch_ingest;
        existing.youtube_ingest = youtube_ingest;
        existing.vod_audio = vod_audio;
        if let Some(v) = vod_audio_inject_eb {
            existing.vod_audio_inject_eb = v;
        }
        existing.stream_format = stream_format;
        existing.audio_track = audio_track;
    } else {
        ns.destinations.push(config::Destination {
            id,
            name,
            enabled,
            platform,
            stream_key,
            custom_egress_url: custom,
            twitch_ingest,
            youtube_ingest,
            vod_audio,
            vod_audio_inject_eb: vod_audio_inject_eb.unwrap_or(false),
            stream_format,
            audio_track,
        });
    }

    // Validate the new full state - return all errors so the UI can show
    // "destination 'Backup' is missing a stream key" specifically.
    let errs = ns.validate();
    if !errs.is_empty() {
        return (
            "400 Bad Request",
            "application/json",
            format!(
                r#"{{"ok":false,"error":"{}"}}"#,
                json_escape(&errs.join("; "))
            ),
        );
    }
    if has_streamable_dest(&ns) {
        ns.configured = true;
    }
    if let Err(e) = ns.save(cfg_path) {
        return (
            "500 Internal Server Error",
            "application/json",
            format!(
                r#"{{"ok":false,"error":"{}"}}"#,
                json_escape(&e.to_string())
            ),
        );
    }
    reconcile_obs_vod_files(&ns, ctrl);
    let _ = settings.send(ns);
    ("200 OK", "application/json", r#"{"ok":true}"#.into())
}

/// Reconcile OBS's external files (user.ini) against the current
/// destinations. Idempotent. A failed write does NOT abort the
/// upstream destination save - we still want the user's config to
/// land - but we log it to the dashboard event log so the user sees
/// why their toggle didn't take effect. The expected failure mode is
/// OBS holding the files open: PermissionDenied, recoverable by
/// closing OBS and toggling once more.
///
/// Also runs a best-effort cleanup pass that strips any stale
/// `multitrack_video_configuration_url` injection from the active
/// profile's service.json. v0.1.0..0.1.2 wrote that on every
/// `vod_audio_inject_eb` toggle, but we now know OBS's `rtmp_custom`
/// plugin discards the key on load (see `obs_register.rs` comment
/// block), so the injection was always dead code. The cleanup means
/// upgraders end up with a clean file.
fn reconcile_obs_vod_files(s: &Settings, ctrl: &Arc<Controller>) {
    let any_vod = s
        .destinations
        .iter()
        .any(|d| d.enabled && d.platform == "twitch" && d.vod_audio);
    // user.ini flag tracks "any VOD-audio destination wants it".
    if let Err(e) = crate::obs_register::set_vod_audio_flag(any_vod) {
        ctrl.log(format!(
            "vod-audio: couldn't write OBS user config ({}). \
             Close OBS, then toggle the destination off and back on to retry.",
            e
        ));
    }
    // One-time cleanup of legacy v0.1.0..0.1.2 service.json injection.
    // Phase C now uses the --config-url CLI flag via the
    // /obs/launch-with-eb button instead of file injection, since OBS's
    // rtmp_custom plugin discards unknown settings keys at load time.
    if let Err(e) = crate::obs_register::revert_vod_eb(s.web_port) {
        ctrl.log(format!(
            "vod-eb cleanup: couldn't strip legacy injection from \
             service.json ({}). Harmless - the injection never reached \
             OBS anyway.",
            e
        ));
    }
}

/// Flip a single destination's `enabled` flag and nothing else. The dock's
/// quick-toggle strip calls this instead of `/destinations` because the full
/// upsert rebuilds the destination from its form and would blank the fields
/// the dock doesn't send (stream key, custom URL, VOD-audio, etc.).
async fn post_destination_toggle(
    body: &str,
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) -> (&'static str, &'static str, String) {
    let form = config::parse_form(body);
    let id = form.get("id").cloned().unwrap_or_default();
    let enabled = matches!(
        form.get("enabled").map(String::as_str),
        Some("on" | "true" | "1")
    );

    let _wl = settings_write_guard();
    let mut ns = settings.borrow().clone();
    let Some(dest) = ns.destinations.iter_mut().find(|d| d.id == id) else {
        return (
            "404 Not Found",
            "application/json",
            r#"{"ok":false,"error":"no such destination"}"#.into(),
        );
    };
    dest.enabled = enabled;
    // Switching on a half-filled destination would stream nothing and, once
    // on, fail every later save. Say what's missing instead.
    if enabled {
        let errors = ns.validate();
        if !errors.is_empty() {
            return (
                "400 Bad Request",
                "application/json",
                format!(
                    r#"{{"ok":false,"error":"{}"}}"#,
                    json_escape(&errors.join("; "))
                ),
            );
        }
    }

    // `configured` is a first-run setup latch, not a live "has an active
    // destination" flag. Toggling your last destination off must not bounce
    // you back into the wizard - only an explicit full reset clears it. So
    // this only ever raises the latch, never lowers it.
    if has_streamable_dest(&ns) {
        ns.configured = true;
    }
    if let Err(e) = ns.save(cfg_path) {
        return (
            "500 Internal Server Error",
            "application/json",
            format!(
                r#"{{"ok":false,"error":"{}"}}"#,
                json_escape(&e.to_string())
            ),
        );
    }
    reconcile_obs_vod_files(&ns, ctrl);
    let _ = settings.send(ns);
    ("200 OK", "application/json", r#"{"ok":true}"#.into())
}

async fn post_destination_delete(
    body: &str,
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) -> (&'static str, &'static str, String) {
    let form = config::parse_form(body);
    let id = form.get("id").cloned().unwrap_or_default();
    let _wl = settings_write_guard();
    let mut ns = settings.borrow().clone();
    let before = ns.destinations.len();
    ns.destinations.retain(|d| d.id != id);
    if ns.destinations.len() == before {
        return (
            "404 Not Found",
            "application/json",
            r#"{"ok":false,"error":"no such destination"}"#.into(),
        );
    }
    // Deleting the last destination leaves `configured` alone: setup was
    // already completed once, so we keep the user on the dashboard (empty
    // Destinations tab) rather than reopening the first-run wizard. Only an
    // explicit `scope=all` reset returns them to the wizard.
    let _ = ns.save(cfg_path);
    reconcile_obs_vod_files(&ns, ctrl);
    let _ = settings.send(ns);
    ("200 OK", "application/json", r#"{"ok":true}"#.into())
}

/// Restrict a dock slot id to a filesystem/config-safe charset so it can
/// key a `dock.<id>=` line without needing escaping. Returns the trimmed
/// id or None if it is empty, too long, or has a disallowed character.
fn sanitize_dock_id(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty() || id.len() > 40 {
        return None;
    }
    if id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Some(id.to_string())
    } else {
        None
    }
}

/// JSON array of the saved dock slot ids, so the editor can show which docks
/// exist and offer to copy their URLs.
fn dock_list_json(settings: &Arc<watch::Sender<Settings>>) -> (&'static str, &'static str, String) {
    let s = settings.borrow();
    let mut out = String::from("[");
    for (i, id) in s.docks.keys().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(&json_escape(id));
        out.push('"');
    }
    out.push(']');
    ("200 OK", "application/json", out)
}

fn dock_layout_get(
    id: &str,
    settings: &Arc<watch::Sender<Settings>>,
) -> (&'static str, &'static str, String) {
    let Some(id) = sanitize_dock_id(id) else {
        return (
            "400 Bad Request",
            "application/json",
            r#"{"ok":false,"error":"bad dock id"}"#.into(),
        );
    };
    // Return the opaque layout blob verbatim, or JSON null so the dock
    // falls back to its built-in default preset.
    match settings.borrow().docks.get(&id) {
        Some(layout) => ("200 OK", "application/json", layout.clone()),
        None => ("200 OK", "application/json", "null".into()),
    }
}

async fn dock_layout_save(
    id: &str,
    body: &str,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) -> (&'static str, &'static str, String) {
    let Some(id) = sanitize_dock_id(id) else {
        return (
            "400 Bad Request",
            "application/json",
            r#"{"ok":false,"error":"bad dock id"}"#.into(),
        );
    };
    let body = body.trim();
    if body.len() > config::MAX_DOCK_LAYOUT_LEN {
        return (
            "413 Payload Too Large",
            "application/json",
            r#"{"ok":false,"error":"layout too large"}"#.into(),
        );
    }
    // The blob lives on one line of the key=value config file, so a
    // newline would corrupt the next parse. Compact JSON never has one.
    if body.contains('\n') || body.contains('\r') {
        return (
            "400 Bad Request",
            "application/json",
            r#"{"ok":false,"error":"layout must be single-line json"}"#.into(),
        );
    }
    let _wl = settings_write_guard();
    let mut ns = settings.borrow().clone();
    if body.is_empty() {
        ns.docks.remove(&id); // empty body resets the slot to default
    } else {
        if !ns.docks.contains_key(&id) && ns.docks.len() >= config::MAX_DOCKS {
            return (
                "400 Bad Request",
                "application/json",
                r#"{"ok":false,"error":"too many saved docks"}"#.into(),
            );
        }
        ns.docks.insert(id, body.to_string());
    }
    if let Err(e) = ns.save(cfg_path) {
        return (
            "500 Internal Server Error",
            "application/json",
            format!(
                r#"{{"ok":false,"error":"{}"}}"#,
                json_escape(&e.to_string())
            ),
        );
    }
    let _ = settings.send(ns);
    ("200 OK", "application/json", r#"{"ok":true}"#.into())
}

/// Live per-destination snapshot: id, name, status, bitrate, frames, etc.
/// Joins settings (the user's configured list) with the controller's
/// runtime stats (only present for destinations that were spawned).
///
/// `include_secrets` is the caller's admin flag. A custom URL can carry the
/// stream key in its path (rtmp://host/app/SECRET), so only a full dashboard
/// session (whose edit form needs it) gets it raw; a dock-token caller gets it
/// blank plus `custom_egress_url_set`, the same rule `GET /config` applies.
fn destinations_json(
    ctrl: &Controller,
    settings: &Arc<watch::Sender<Settings>>,
    include_secrets: bool,
) -> String {
    let s = settings.borrow();
    let snap = ctrl.destination_snapshot();
    let stats_for = |id: &str| {
        snap.iter()
            .find(|t| t.0 == id)
            .cloned()
            .unwrap_or_else(|| (id.into(), false, 0, 0, 0, 0, 0, 0))
    };
    let readouts = video_readouts(ctrl);
    let mut out = String::from("[");
    for (i, d) in s.destinations.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let url = d.egress_url().unwrap_or_default();
        let (_id, alive, _seq, kbps, tags, bytes, cuts, reconnects) = stats_for(&d.id);
        // Vertical destinations report whether their canvas is resolved
        // yet (Twitch Dual Format live + a portrait track detected). The
        // dashboard turns this into a green "Vertical" badge vs an amber
        // "waiting for Dual Format" hint.
        let v = dest_video(ctrl, d, &readouts);
        let custom_url_shown = if include_secrets {
            d.custom_egress_url.as_str()
        } else {
            ""
        };
        out.push_str(&format!(
            r#"{{"id":{id},"name":{n},"enabled":{en},"platform":{p},"custom_egress_url":{cu},"custom_egress_url_set":{cus},"twitch_ingest":{ti},"youtube_ingest":{yi},"vod_audio":{va},"vod_audio_inject_eb":{vie},"stream_format":{sf},"audio_track":{at},"vertical_ready":{vr},"vertical_canvas_present":{vcp},"video_res":{vres},"video_codec":{vcod},"stream_key_set":{ks},"url_redacted":{ur},"alive":{al},"bitrate_kbps":{br},"tags_sent":{ts},"bytes_sent":{bs},"cuts":{ct},"reconnects":{rc}}}"#,
            id = json_escape_quoted(&d.id),
            n  = json_escape_quoted(&d.name),
            en = d.enabled,
            p  = json_escape_quoted(&d.platform),
            cu = json_escape_quoted(custom_url_shown),
            cus = !d.custom_egress_url.is_empty(),
            ti = json_escape_quoted(&d.twitch_ingest),
            yi = json_escape_quoted(&d.youtube_ingest),
            va = d.vod_audio,
            vie = d.vod_audio_inject_eb,
            sf = json_escape_quoted(&d.stream_format),
            at = json_escape_quoted(&d.audio_track),
            vr = v.vertical_ready,
            vcp = v.vertical_canvas_present,
            vres = json_escape_quoted(&v.res),
            vcod = json_escape_quoted(&v.codec),
            ks = !d.stream_key.is_empty(),
            ur = json_escape_quoted(&redact_url(&url)),
            al = alive,
            br = kbps,
            ts = tags,
            bs = bytes,
            ct = cuts,
            rc = reconnects,
        ));
    }
    out.push(']');
    out
}

fn redact_url(url: &str) -> String {
    crate::config::elide_after_last_slash(url, 12, 4, 4)
}

fn json_escape_quoted(s: &str) -> String {
    format!(r#""{}""#, json_escape(s))
}

fn generate_dest_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("d{:x}", nanos as u64)
}

/// Normalize the destination form's `stream_format` field. Only "vertical"
/// on a non-Twitch platform is honored; everything else (absent, "horizontal",
/// a typo, or ANY value on Twitch - which gets native dual-canvas passthrough)
/// resolves to the safe horizontal default.
fn normalize_stream_format(platform: &str, raw: Option<&str>) -> String {
    if platform != "twitch" && raw == Some("vertical") {
        "vertical".to_string()
    } else {
        "horizontal".to_string()
    }
}

/// Normalize the destination form's `audio_track` field. Known routing modes
/// ("both", "1", "2") pass through; everything else (absent, "auto", a typo)
/// resolves to the "auto" default. The platform-specific meaning is applied
/// later by the egress supervisor (see main.rs), not here.
fn normalize_audio_track(raw: Option<&str>) -> String {
    match raw {
        Some("both") | Some("1") | Some("2") => raw.unwrap().to_string(),
        _ => "auto".to_string(),
    }
}

// ----------------------------------------------------------------------
// Pluggable overlays - files under settings.overlays_dir
// ----------------------------------------------------------------------

/// List overlays on disk for the Studio. Each entry carries the slug
/// (filename without extension), a display name (the overlay's `<title>`,
/// which the baker sets to the doc name), and `studio` - whether the file
/// is a Studio-authored overlay (carries an `ic-doc` comment) versus a
/// legacy hand-dropped `.html`. Studio overlays can be re-edited; legacy
/// ones are still listed and usable as browser sources.
fn list_overlays(settings: &Arc<watch::Sender<Settings>>) -> String {
    let dir = settings.borrow().overlays_dir.clone();
    let mut items: Vec<(String, String, bool, bool)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let fname = match e.file_name().into_string() {
                Ok(s) => s,
                Err(_) => continue,
            };
            let slug = if let Some(s) = fname.strip_suffix(".html") {
                s.to_string()
            } else if let Some(s) = fname.strip_suffix(".htm") {
                s.to_string()
            } else {
                continue;
            };
            // Files are small; reading them whole to pull the title and
            // detect the ic-doc marker is cheap and keeps the list honest.
            let content = std::fs::read_to_string(e.path()).unwrap_or_default();
            let studio = content.contains("<!--ic-doc:");
            // `autohide` lets the dashboard show a "stays up / hides when live"
            // quick toggle (it appends ?autohide=off to the copied URL).
            let autohide = content.contains("data-ah-");
            let name = extract_title(&content).unwrap_or_else(|| slug.clone());
            items.push((slug, name, studio, autohide));
        }
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = String::from("[");
    for (i, (slug, name, studio, autohide)) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            r#"{{"slug":{},"name":{},"studio":{},"autohide":{}}}"#,
            json_escape_quoted(slug),
            json_escape_quoted(name),
            studio,
            autohide
        ));
    }
    out.push(']');
    out
}

/// Pull the text between the first `<title>` and `</title>`. Used by the
/// overlay list to show a friendly name without parsing the whole doc.
fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let s = lower.find("<title>")?;
    let start = s + "<title>".len();
    let end_rel = lower[start..].find("</title>")?;
    let title = html[start..start + end_rel].trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

/// Open the OS file browser highlighting `path` (or opening it, when it's a
/// directory). Backs the System tab's reveal buttons. Callers map a fixed
/// keyword to a known app path - never a client-supplied one - so this can
/// only ever surface our own files.
fn reveal_path(path: &Path) {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    // Open the folder that holds the target (or the folder itself). We open
    // the directory rather than `/select`-ing the file: explorer's
    // `/select,PATH` switch is unreliable to spawn (its comma syntax fights
    // both Rust's arg-escaping and cmd-style quoting, so it tends to land on
    // a default location), whereas a plain folder path - auto-quoted by
    // `arg` so spaces are safe - opens reliably.
    let dir = if abs.is_dir() {
        abs.clone()
    } else {
        abs.parent().map(|p| p.to_path_buf()).unwrap_or(abs.clone())
    };
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(&dir).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&dir).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
    }
}

/// A user overlay slug: ASCII alphanumerics plus `-`/`_`, max 64 chars.
/// The served artifact is `<slug>.html` inside overlays_dir. Stricter than
/// `serve_overlay_file`'s filename check (no dots, no separators) because a
/// slug never carries an extension - the `.html` is appended here, so no
/// crafted slug can escape the overlays directory.
fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 64
        && slug
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Save a Studio-baked overlay. The body is the full self-contained HTML
/// the Studio produced (lean live overlay + the editable doc embedded as
/// an `ic-doc` comment). Writes `overlays_dir/<slug>.html`.
fn overlay_save(
    slug: &str,
    body: &str,
    settings: &Arc<watch::Sender<Settings>>,
) -> (&'static str, &'static str, String) {
    if !valid_slug(slug) {
        return (
            "400 Bad Request",
            "application/json",
            r#"{"ok":false,"error":"invalid overlay name - use letters, numbers, - or _ (max 64)"}"#
                .into(),
        );
    }
    let dir = settings.borrow().overlays_dir.clone();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return (
            "500 Internal Server Error",
            "application/json",
            format!(
                r#"{{"ok":false,"error":"{}"}}"#,
                e.to_string().replace('"', "'")
            ),
        );
    }
    let path = dir.join(format!("{slug}.html"));
    match std::fs::write(&path, body.as_bytes()) {
        Ok(()) => (
            "200 OK",
            "application/json",
            format!(
                r#"{{"ok":true,"slug":"{}","url":"/overlay/{}.html"}}"#,
                slug, slug
            ),
        ),
        Err(e) => (
            "500 Internal Server Error",
            "application/json",
            format!(
                r#"{{"ok":false,"error":"{}"}}"#,
                e.to_string().replace('"', "'")
            ),
        ),
    }
}

/// Delete a Studio overlay and its per-overlay assets directory.
fn overlay_delete(
    slug: &str,
    settings: &Arc<watch::Sender<Settings>>,
) -> (&'static str, &'static str, String) {
    if !valid_slug(slug) {
        return (
            "400 Bad Request",
            "application/json",
            r#"{"ok":false,"error":"invalid overlay name"}"#.into(),
        );
    }
    let dir = settings.borrow().overlays_dir.clone();
    // Best-effort: removing a non-existent file is not an error worth
    // surfacing - the end state (overlay gone) is what the caller wants.
    let _ = std::fs::remove_file(dir.join(format!("{slug}.html")));
    let _ = std::fs::remove_dir_all(dir.join(slug));
    ("200 OK", "application/json", r#"{"ok":true}"#.into())
}

/// Delete every Studio-authored overlay (files carrying the `ic-doc` marker)
/// in `dir`, plus each one's per-overlay assets directory. Hand-written legacy
/// `.html` files (no marker) are left alone - they ship with the app and aren't
/// user data the dashboard can recreate.
fn wipe_studio_overlays(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let is_html = path
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| x.eq_ignore_ascii_case("html") || x.eq_ignore_ascii_case("htm"))
            .unwrap_or(false);
        if !is_html {
            continue;
        }
        if std::fs::read_to_string(&path)
            .unwrap_or_default()
            .contains("<!--ic-doc:")
        {
            let _ = std::fs::remove_file(&path);
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let _ = std::fs::remove_dir_all(dir.join(stem));
            }
        }
    }
}

/// Mark the built-in preset overlays as seeded (the dashboard calls this once,
/// after it bakes them on first run), so deleted ones don't reappear.
fn overlays_mark_seeded(
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) -> (&'static str, &'static str, String) {
    let _wl = settings_write_guard();
    let mut next = settings.borrow().clone();
    if !next.overlays_seeded {
        next.overlays_seeded = true;
        if let Err(e) = next.save(cfg_path) {
            return (
                "500 Internal Server Error",
                "application/json",
                format!(
                    r#"{{"ok":false,"error":"{}"}}"#,
                    json_escape(&e.to_string())
                ),
            );
        }
        let _ = settings.send(next);
    }
    ("200 OK", "application/json", r#"{"ok":true}"#.into())
}

/// Restore the default overlays: wipe the Studio overlays and clear the seeded
/// flag so the dashboard re-bakes the built-in presets on its next load.
fn overlays_reset(
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) -> (&'static str, &'static str, String) {
    let _wl = settings_write_guard();
    let mut next = settings.borrow().clone();
    wipe_studio_overlays(&next.overlays_dir);
    next.overlays_seeded = false;
    if let Err(e) = next.save(cfg_path) {
        return (
            "500 Internal Server Error",
            "application/json",
            format!(
                r#"{{"ok":false,"error":"{}"}}"#,
                json_escape(&e.to_string())
            ),
        );
    }
    let _ = settings.send(next);
    ("200 OK", "application/json", r#"{"ok":true}"#.into())
}

fn serve_overlay_file(
    name: &str,
    settings: &Arc<watch::Sender<Settings>>,
) -> (&'static str, &'static str, String) {
    // First-pass name sanity (cheap): no separators, no parent refs, no
    // Windows drive markers. Catches the common "?file=../../etc/passwd"
    // probe before we hit the filesystem.
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains(':')
    {
        return (
            "400 Bad Request",
            "text/plain; charset=utf-8",
            "invalid overlay name".into(),
        );
    }
    let dir = settings.borrow().overlays_dir.clone();
    let path = dir.join(name);

    // Second-pass: canonicalize and confirm the resolved file is still
    // inside the overlays dir. Without this, a symlink inside overlays/
    // pointing at C:\Users\…\.ssh\id_rsa would be served as text. The
    // name-only check above can't catch that - the path string is clean,
    // the filesystem does the redirect.
    let canon_path = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return (
                "404 Not Found",
                "text/plain; charset=utf-8",
                format!("overlay '{}' not found in {}", name, dir.display()),
            )
        }
    };
    let canon_dir = match dir.canonicalize() {
        Ok(p) => p,
        // If the overlays dir itself can't be canonicalized, refuse
        // rather than risk serving anything: misconfigured state.
        Err(_) => {
            return (
                "500 Internal Server Error",
                "text/plain; charset=utf-8",
                "overlays_dir is misconfigured".into(),
            )
        }
    };
    if !canon_path.starts_with(&canon_dir) {
        return (
            "403 Forbidden",
            "text/plain; charset=utf-8",
            "overlay path escapes the overlays directory".into(),
        );
    }

    match std::fs::read_to_string(&canon_path) {
        Ok(content) => ("200 OK", "text/html; charset=utf-8", content),
        Err(_) => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            format!("overlay '{}' not found in {}", name, dir.display()),
        ),
    }
}

fn apply_field_str(s: &mut Settings, key: &str, value: &str) {
    // Wraps Settings::apply_field but is callable from outside the module.
    // Implementing here avoids exposing it on Settings.
    match key {
        "platform" => s.platform = value.into(),
        "stream_key" => s.stream_key = value.into(),
        "custom_egress_url" => s.custom_egress_url = value.into(),
        "ingest_port" => {
            if let Ok(v) = value.parse() {
                s.ingest_port = v;
            }
        }
        "ingest_bind_all" => s.ingest_bind_all = value == "on" || value == "true" || value == "1",
        "ingest_key" => s.ingest_key = value.into(),
        "web_port" => {
            if let Ok(v) = value.parse() {
                s.web_port = v;
            }
        }
        "web_bind_all" => s.web_bind_all = value == "on" || value == "true" || value == "1",
        "buffer_mb" => {
            if let Ok(v) = value.parse() {
                s.buffer_mb = v;
            }
        }
        "buffer_path" => s.buffer_path = std::path::PathBuf::from(value),
        "overlays_dir" => s.overlays_dir = std::path::PathBuf::from(value),
        "discord_webhook_url" => s.discord_webhook_url = value.into(),
        "tracing_enabled" => {
            // Form encoding: checkbox sends "true"/"false" or "on"/"" -
            // treat anything non-empty-non-false as truthy.
            let on = !matches!(value, "" | "false" | "0" | "off");
            s.tracing_enabled = on;
        }
        "auto_arm_on_connect" => {
            s.auto_arm_on_connect = !matches!(value, "" | "false" | "0" | "off");
        }
        "auto_activate_when_ready" => {
            s.auto_activate_when_ready = !matches!(value, "" | "false" | "0" | "off");
        }
        "auto_arm_delay_ms" => {
            if let Ok(v) = value.parse() {
                s.auto_arm_delay_ms = v;
            }
        }
        "update_check_enabled" => {
            s.update_check_enabled = !matches!(value, "" | "false" | "0" | "off");
        }
        "open_dashboard_on_launch" => {
            s.open_dashboard_on_launch = !matches!(value, "" | "false" | "0" | "off");
        }
        // hotkey.<action>=<combo> and midi.<action>=<signature>. Both
        // `set`s canonicalize and drop anything malformed, so an empty or
        // bad value simply clears the binding - which is exactly how the
        // dashboard's clear button asks for an unbind.
        k if k.starts_with("hotkey.") => {
            s.hotkeys.set(&k["hotkey.".len()..], value);
        }
        k if k.starts_with("midi.") => {
            s.midi.set(&k["midi.".len()..], value);
        }
        // crash_protection.<field>=<value>. `set` clamps and drops anything
        // malformed, same contract as the config loader.
        k if k.starts_with(crate::crash_protection::KEY_PREFIX) => {
            s.crash_protection
                .set(&k[crate::crash_protection::KEY_PREFIX.len()..], value);
        }
        // Empty means "every device", which is also what an unknown name
        // amounts to - the listener simply finds nothing to open and the
        // dashboard says so.
        //
        // Sanitised the same way the loader and the device lister do it. The
        // dashboard only ever sends a name it got from the enumerated list,
        // so this is about the two appliers agreeing rather than about a
        // reachable bug: a name that survives one path and not the other
        // matches no device, and the user sees a pick that silently does
        // nothing until the next restart tidies it up.
        "midi_device" => s.midi_device = config::sanitize_device_name(value),
        "twitch_client_id" => s.twitch_client_id = value.trim().to_string(),
        _ => {}
    }
}

/// Split the request line into method + path, and resolve the body length.
///
/// The length is `None` when the request describes a body we cannot read
/// honestly: a `Content-Length` that is not a number, two that disagree, or a
/// `Transfer-Encoding` we do not implement. The caller answers 400 rather than
/// guessing.
///
/// Guessing was the old behaviour - `parse().unwrap_or(0)` - and it read as an
/// empty body, which is not a neutral choice here. Every POST route takes its
/// arguments from that body, and several treat "no argument" as a real
/// instruction: `POST /arm` with no `ms` is a disarm. So a request that was
/// merely malformed did not fail, it dropped the streamer's delay. Absent is
/// still `Some(0)` - a POST with no body is ordinary and must keep working.
fn parse_request_head(head: &str) -> (&str, &str, Option<usize>) {
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    let mut content_length: Option<usize> = None;
    let mut seen_length = false;
    let mut bad = false;
    for line in lines {
        if let Some(v) = strip_prefix_icase(line, "content-length:") {
            match v.trim().parse::<usize>() {
                // A second, disagreeing Content-Length is the classic
                // request-smuggling shape. We never proxy and always close,
                // so it cannot be smuggled past us - but there is no honest
                // reading of two lengths, so refuse rather than pick one.
                Ok(n) if !seen_length || content_length == Some(n) => {
                    content_length = Some(n);
                    seen_length = true;
                }
                _ => bad = true,
            }
        } else if let Some(v) = strip_prefix_icase(line, "transfer-encoding:") {
            // We do not implement chunked. Ignoring it would hand the route an
            // empty body and run it with no arguments; saying so is honest.
            if !v.trim().eq_ignore_ascii_case("identity") {
                bad = true;
            }
        }
    }
    if bad {
        return (method, path, None);
    }
    (method, path, Some(content_length.unwrap_or(0)))
}

/// Extract the Origin and Host headers verbatim (or empty strings).
/// Used by the CSRF guard on state-changing endpoints.
fn parse_origin_host(head: &str) -> (String, String) {
    let mut origin = String::new();
    let mut host = String::new();
    for line in head.split("\r\n") {
        if let Some(v) = strip_prefix_icase(line, "origin:") {
            origin = v.trim().to_string();
        } else if let Some(v) = strip_prefix_icase(line, "host:") {
            host = v.trim().to_string();
        }
    }
    (origin, host)
}

/// True iff the client's `Accept-Encoding` header advertises `gzip` (with
/// a non-zero q value if specified). Used by the static-page fast-path
/// to decide whether to ship the pre-gzipped HTML blob directly.
fn accepts_gzip(head: &str) -> bool {
    for line in head.split("\r\n") {
        if let Some(v) = strip_prefix_icase(line, "accept-encoding:") {
            for part in v.split(',') {
                let p = part.trim();
                // Each entry is `token` or `token;q=N`. We only need to
                // confirm gzip appears with q != 0.
                let (token, q) = match p.split_once(';') {
                    Some((t, params)) => {
                        let qv = params
                            .split(';')
                            .find_map(|kv| {
                                let kv = kv.trim();
                                kv.strip_prefix("q=").or_else(|| kv.strip_prefix("Q="))
                            })
                            .and_then(|s| s.parse::<f32>().ok())
                            .unwrap_or(1.0);
                        (t.trim(), qv)
                    }
                    None => (p, 1.0),
                };
                if q > 0.0
                    && (token.eq_ignore_ascii_case("gzip") || token.eq_ignore_ascii_case("*"))
                {
                    return true;
                }
            }
        }
    }
    false
}

/// CSRF guard for POST endpoints. Returns true if the request should be
/// allowed.
///
/// Policy:
///   * GET / HEAD: always allowed (read-only).
///   * POST with NO Origin header: allowed (CLI tools like curl,
///     Stream Deck "Web Request" action, and most server-to-server
///     callers don't send Origin - we'd break legitimate use-cases by
///     rejecting these).
///   * POST WITH an Origin header: must match the Host header (i.e.
///     same-origin from the user's own dashboard). Cross-origin browser
///     POSTs (the actual CSRF surface) are blocked here - a tab on
///     evil.com `fetch('http://127.0.0.1:7799/stop', {method:'POST'})`
///     sends `Origin: https://evil.com`, which won't match Host.
///
/// This is the cheapest defense that closes the CSRF browser surface
/// without breaking headless API users. A token-based scheme would be
/// strictly stronger but requires UI plumbing - punt unless asked.
fn allow_csrf(method: &str, origin: &str, host: &str) -> bool {
    if !matches!(method, "POST" | "PUT" | "DELETE" | "PATCH") {
        return true;
    }
    if origin.is_empty() {
        return true;
    }
    // Origin is "scheme://host[:port]"; we want the host[:port] part.
    let origin_host = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or(origin)
        .split('/')
        .next()
        .unwrap_or("");
    !host.is_empty() && origin_host.eq_ignore_ascii_case(host)
}

fn strip_prefix_icase<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() < prefix.len() {
        return None;
    }
    if s.as_bytes()
        .iter()
        .zip(prefix.as_bytes())
        .all(|(a, b)| a.eq_ignore_ascii_case(b))
    {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---- Optional dashboard auth: request classification + helpers -----------

/// Access level a route requires WHEN auth is enabled. The default is `Admin`,
/// so any route not explicitly listed below is protected (fail closed): a new
/// endpoint is locked down unless someone deliberately opens it here.
#[derive(Debug, PartialEq, Eq)]
enum Access {
    /// No auth: the login page and overlay DISPLAY (OBS browser sources).
    Public,
    /// Session cookie OR the least-privilege dock token: status, delay
    /// control, and the dock itself. Never anything that reveals a secret.
    Control,
    /// Session cookie only: everything that changes config, destinations,
    /// files, or the app lifecycle, or that could disclose a stream key.
    Admin,
}

fn classify_access(method: &str, path: &str) -> Access {
    if path == "/login" {
        return Access::Public;
    }
    // Web call integrations: outside apps (a Stream Deck, a script) can't log
    // in. The secret token in the path is the credential, and all a caller
    // can do is run the integrations that hold that exact token.
    if path.starts_with("/hooks/") && (method == "GET" || method == "POST") {
        return Access::Public;
    }
    // Overlay DISPLAY only (browser sources can't log in); saving overlays is
    // a POST to /overlays/ which stays Admin. GET only: nothing writes under
    // /overlay, and a future route that does must not inherit this exemption.
    if method == "GET" && (path == "/overlay" || path.starts_with("/overlay/")) {
        return Access::Public;
    }
    // OBS fetches the multitrack (Enhanced Broadcasting) config when it starts
    // streaming, with no session cookie and often no token - the config URL is
    // saved in OBS before any password is set. It returns only the encoder
    // ladder plus a local ingest template ("rtmp://localhost:<port>/live/
    // {stream_key}"), never a secret, and cannot control anything, so it stays
    // public like the overlay display. Gating it would break Start Streaming the
    // moment a dashboard password is enabled.
    if path == "/obs/multitrack-config" {
        return Access::Public;
    }
    // The overlay's read-only feed, and only by GET. An OBS browser source
    // cannot log in, so without this an overlay stops updating the moment a
    // dashboard password is set - it paints once and freezes, silently,
    // because both its fetch and its EventSource swallow the 401.
    //
    // Public is safe here only because the payload is: `overlay_state_json`
    // carries the numbers a widget draws and nothing else. The alternative -
    // letting overlays reach `/state` - would mean opening `Access::Control`
    // to unauthenticated callers, and Control is arm, activate, cut and
    // go-live. A picture must not be a way to drive the delay.
    if method == "GET" && (path == "/overlay-state" || path == "/overlay-events") {
        return Access::Public;
    }
    match (method, path) {
        ("GET", "/state")
        | ("GET", "/events")
        | ("GET", "/dock")
        | ("GET", "/dock.js")
        | ("GET", "/docks")
        | ("GET", "/profiles")
        | ("GET", "/platforms")
        // Read + operational routes the OBS dock needs so it can render and run
        // the stream. GET /config is redacted for a dock caller (no raw ingest
        // key or dock token - see route + to_json); GET /destinations is already
        // redacted (a "key set" boolean, never the raw key); toggling a
        // destination on/off is operational, not a settings change. Editing
        // settings, upserting destinations, and every secret write stay Admin.
        | ("GET", "/config")
        | ("GET", "/destinations")
        | ("GET", "/overlays")
        | ("POST", "/destinations/toggle")
        | ("POST", "/arm")
        | ("POST", "/activate")
        | ("POST", "/stop")
        | ("POST", "/disarm")
        | ("POST", "/delay")
        | ("POST", "/go-live")
        | ("POST", "/cut-after")
        | ("POST", "/cut-after/cancel")
        // Ending a crash-protection hold is operational, like a cut.
        | ("POST", "/crash-protection/end") => Access::Control,
        ("POST", p) if p.starts_with("/docks/") => Access::Control,
        // GET a saved dock layout (the dock loads its own persisted layout).
        ("GET", p) if p.starts_with("/docks/") => Access::Control,
        _ => Access::Admin,
    }
}

/// Parse the Cookie header into name -> value (`a=b; c=d`).
fn parse_cookies(head: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for line in head.split("\r\n") {
        if let Some(v) = strip_prefix_icase(line, "cookie:") {
            for pair in v.split(';') {
                if let Some((k, val)) = pair.split_once('=') {
                    out.insert(k.trim().to_string(), val.trim().to_string());
                }
            }
        }
    }
    out
}

/// First value of query parameter `name` in a `path?a=b&c=d` string. Tokens
/// are hex, so no URL-decoding is needed.
fn query_param(path: &str, name: &str) -> Option<String> {
    let q = path.split_once('?')?.1;
    for pair in q.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == name {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// "; Secure" when the request reached us over HTTPS (a reverse proxy sets
/// `X-Forwarded-Proto: https`), so the auth cookie is never sent in the clear
/// once TLS is in front. Empty over plain HTTP so local use still works.
///
/// This trusts the header, which is correct because it is fail-safe in the only
/// two ways that matter: a spoofed `https` from a direct plain-HTTP client only
/// makes the cookie MORE restrictive (a `Secure` cookie the browser then won't
/// resend over that same plain connection), and a proxy that omits the header
/// over real TLS merely drops `Secure` (the cookie still works, just without
/// that one hardening bit). Neither weakens auth; the documented deployment is
/// a proxy that sets the header.
fn secure_flag(head: &str) -> &'static str {
    for line in head.split("\r\n") {
        if let Some(v) = strip_prefix_icase(line, "x-forwarded-proto:") {
            if v.trim().eq_ignore_ascii_case("https") {
                return "; Secure";
            }
        }
    }
    ""
}

/// True when the client prefers HTML (a browser navigation), so an
/// unauthorized response should redirect to the login page rather than 401.
fn wants_html(head: &str) -> bool {
    for line in head.split("\r\n") {
        if let Some(v) = strip_prefix_icase(line, "accept:") {
            return v.contains("text/html");
        }
    }
    false
}

/// Write a small response with an optional extra header block (Set-Cookie).
/// True if `ip` (a peer address string like "127.0.0.1" or "::1") is an
/// IPv4/IPv6 loopback address. Used to keep first-time password bootstrap on
/// the local machine. An unparseable value is treated as non-loopback so the
/// restriction fails closed.
fn is_loopback(ip: &str) -> bool {
    ip.parse::<std::net::IpAddr>()
        .map(|a| a.is_loopback())
        .unwrap_or(false)
}

async fn write_simple(
    sock: &mut TcpStream,
    status: &str,
    ctype: &str,
    body: &str,
    extra_headers: &str,
) -> io::Result<()> {
    let resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Cache-Control: no-store\r\nConnection: close\r\n\r\n{}",
        status, ctype, body.len(), extra_headers, body
    );
    sock.write_all(resp.as_bytes()).await
}

/// Self-contained login page (no external assets, so it renders before any
/// authenticated request). Served at GET /login only when auth is enabled.
const LOGIN_HTML: &str = r##"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>InstantClone - Sign in</title>
<style>
:root{color-scheme:dark}
body{margin:0;height:100vh;display:flex;align-items:center;justify-content:center;background:#07080a;color:#e8edf3;font:15px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}
.card{width:320px;max-width:88vw;background:#11141a;border:1px solid #1f242d;border-radius:14px;padding:26px}
h1{margin:0 0 4px;font-size:18px}
p{margin:0 0 18px;color:#8b95a3;font-size:13px}
input{width:100%;box-sizing:border-box;padding:11px 12px;border-radius:9px;border:1px solid #2a313c;background:#0c0e12;color:#e8edf3;font-size:14px}
input:focus{outline:none;border-color:#5ac8fa}
button{width:100%;margin-top:12px;padding:11px;border:0;border-radius:9px;background:#5ac8fa;color:#04121a;font-weight:700;font-size:14px;cursor:pointer}
button:disabled{opacity:.6;cursor:default}
.err{margin-top:12px;color:#ff6b6b;font-size:13px;min-height:1.2em}
.dot{display:inline-block;width:8px;height:8px;border-radius:50%;background:#5ac8fa;margin-right:8px}
</style></head><body>
<form class="card" onsubmit="return signIn(event)">
<h1><span class="dot"></span>InstantClone</h1>
<p>Enter the dashboard password to continue.</p>
<input id="pw" type="password" placeholder="Password" autocomplete="current-password" autofocus>
<button id="btn" type="submit">Sign in</button>
<div class="err" id="err"></div>
</form>
<script>
async function signIn(e){
  e.preventDefault();
  var btn=document.getElementById('btn'),err=document.getElementById('err');
  btn.disabled=true;btn.textContent='Signing in...';err.textContent='';
  try{
    var r=await fetch('/login',{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:'password='+encodeURIComponent(document.getElementById('pw').value)});
    if(r.ok){location.href='/';return false;}
    var j=await r.json().catch(function(){return {};});
    err.textContent=(j&&j.error)?j.error:'Sign in failed';
  }catch(_){err.textContent='Network error';}
  btn.disabled=false;btn.textContent='Sign in';
  return false;
}
</script></body></html>"##;

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

// ----------------------------------------------------------------------
// HTML  -  one page, conditional setup / dashboard, all CSS+JS inline.
// ----------------------------------------------------------------------

/// Compact view for OBS browser-dock embedding. ~280x340 looks decent.
/// Reuses the same `/state` + `/arm` + `/activate` + `/stop` endpoints
/// as the main dashboard so behavior stays identical. Source lives in
/// `web/dock.html` - built-time minified + gzipped (see `build.rs`).
static DOCK_HTML_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/dock.html.gz"));

/// Dock logic (widget rendering, gear editor, state polling). Split out of
/// `dock.html` so the page stays markup+CSS; source in `web/dock.js`,
/// build-time gzipped (see `build.rs`).
static DOCK_JS_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/dock.js.gz"));

/// Main dashboard / first-run wizard. Source lives in `web/index.html`;
/// build-time minified + gzipped (see `build.rs`).
static INDEX_HTML_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/index.html.gz"));
/// The dashboard's Integrations tab; source in `web/integrations.js`,
/// build-time gzipped like the dock script.
static INTEGRATIONS_JS_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/integrations.js.gz"));

/// Overlay Studio author-time runtime + baker. Loaded only by the
/// dashboard (never by a live overlay in OBS). Source lives in
/// `web/overlay-runtime.js`; build-time gzipped (see `build.rs`).
static OVERLAY_RUNTIME_JS_GZ: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/overlay-runtime.js.gz"));

/// Optional VOD-unlocker OBS Lua script, embedded from `obs/`. Served by
/// `GET /obs/vod-script/download` as a Save-As attachment. Embedding keeps it
/// in lockstep with the running binary (no release-asset version skew) and
/// needs no network. ~8 KB of text; not worth gzipping for a one-off download.
static VOD_UNLOCKER_LUA: &str = include_str!("../obs/instantclone-vod-track.lua");
/// Render the OBS browser-source overlay. Supports two query knobs:
///   ?lang=en|es|pt|fr|de                         - label localization
///   ?style=minimal|corner|strip|focus|broadcast|ticker  - visual variant
///
/// All six styles share the same DOM skeleton and `/state` polling
/// loop. The differences are spatial density + position, applied via a
/// `body.<style>` class hook. Three shared behaviours:
///   * 4 s idle auto-dim - overlay fades to ~22% opacity during
///     `idle`/`passthrough`, wakes back on the next phase transition.
///   * Phase-change halo - brief accent-glow bloom on any phase change.
///   * Tweened delay readout - the big number animates between values
///     instead of snapping, so arming reads as a building number.
fn overlay_html(query: &str) -> String {
    let params = config::parse_form(query);
    let lang = params.get("lang").map(String::as_str).unwrap_or("en");
    let style = params.get("style").map(String::as_str).unwrap_or("minimal");

    // Sanitise so a malformed URL can't break the variant selector.
    let style = match style {
        "minimal" | "corner" | "strip" | "focus" | "broadcast" | "ticker" => style,
        _ => "minimal",
    };
    // Same treatment, and for a sharper reason: `lang` is written into the
    // page's own `<html lang="...">` attribute below. This route is public
    // and needs no auth, so an unvalidated value is a reflected-XSS hole -
    // one clicked link and script runs on our origin, which is same-origin
    // for the CSRF check and can read the stream key or repoint the egress.
    // An allow-list, not escaping: these are the languages we have strings
    // for, so anything else has no business reaching the page.
    let lang = match lang {
        "en" | "es" | "pt" | "fr" | "de" => lang,
        _ => "en",
    };

    let (l_delay, _l_live, l_preparing, l_ready, l_active, l_passthrough) = match lang {
        "es" => (
            "Retraso",
            "EN VIVO",
            "Preparando",
            "Listo",
            "Activo",
            "Sin retraso",
        ),
        "pt" => (
            "Atraso",
            "AO VIVO",
            "Preparando",
            "Pronto",
            "Ativo",
            "Sem atraso",
        ),
        "fr" => (
            "Délai",
            "EN DIRECT",
            "Préparation",
            "Prêt",
            "Actif",
            "Sans délai",
        ),
        "de" => ("Verz.", "LIVE", "Aufbau", "Bereit", "Aktiv", "Ohne Verz."),
        _ => (
            "Delay",
            "LIVE",
            "Preparing",
            "Ready",
            "Active",
            "Passthrough",
        ),
    };

    format!(
        r##"<!doctype html><html lang="{lang}"><head><meta charset="utf-8">
<title>InstantClone overlay</title><style>
/* Shared tokens. The OBS / LIVE dot row that used to live next to the
   number is GONE -its three states (idle / armed-cool / warn-pulse)
   now live on the number itself, conveyed by the current colour
   class on <body>. The colour class is the status. */
:root{{
  --idle:rgba(255,255,255,.55);
  --amber:#ffb73a;
  --cyan:#5ac8fa;
  --red:#ff5a5a;
  --surface:rgba(10,12,16,.62);
  --surface-strong:rgba(10,12,16,.86);
  --line:rgba(255,255,255,.10);
  --ease-out:cubic-bezier(.16,1,.3,1);
}}
*{{box-sizing:border-box}}
html,body{{margin:0;padding:0;background:transparent;color:var(--idle);
  font-family:'Inter','SF Pro Display',-apple-system,Segoe UI,Roboto,sans-serif;
  font-feature-settings:"tnum" 1;width:100%;height:100%;overflow:hidden;
  -webkit-font-smoothing:antialiased}}

/* Colour state lives on <body>. Each class sets `color: <hue>`; child
   text uses `color: inherit` and `text-shadow: 0 0 N currentColor` so
   the hue + glow track together with one declaration. */
body.state-idle  {{color:var(--idle)}}
body.state-amber {{color:var(--amber)}}
body.state-ok    {{color:var(--cyan)}}
body.state-red   {{color:var(--red)}}

/* Soft entrance. The overlay arrives rather than pops. */
.box{{animation:boxIn .42s var(--ease-out) both;
  transition:color .55s ease,opacity .55s ease,filter .55s ease,box-shadow .42s ease}}
@keyframes boxIn{{from{{opacity:0;filter:blur(8px)}}to{{opacity:1;filter:blur(0)}}}}

/* Idle / passthrough fades the whole overlay down after 4 s of nothing
   happening, so the viewer's eye stops snagging on a static number. */
body.idle-dim .box{{opacity:.32;filter:blur(.3px)}}

/* Phase-change halo. Brief bloom in the current state colour on every
   transition, so 'I just armed' / 'I just activated' / 'I just cut'
   read as a moment instead of a slide. */
body.phase-flash .box{{box-shadow:0 0 0 1px color-mix(in oklch,currentColor 40%,transparent),
  0 0 36px 8px color-mix(in oklch,currentColor 35%,transparent)}}

/* The strip layout reuses .track / .fill / .label DOM nodes; every
   other style hides them so the corner / focus / minimal etc. boxes
   don't end up with a duplicate number or a stray progress line. */
.track,.fill,.label{{display:none}}
body.strip .track,body.strip .fill,body.strip .label{{display:block}}

/* Number + breathing pulse. The "live" body class adds a subtle 3 s
   breath to the number's text-shadow, signalling the clock is
   running on a delayed feed without strobing the viewer. */
.v{{font-variant-numeric:tabular-nums;font-weight:700;letter-spacing:-1px;
  color:inherit;text-shadow:0 0 18px currentColor;
  transition:text-shadow .35s ease}}
body.live .v{{animation:breathe 3.2s ease-in-out infinite}}
@keyframes breathe{{0%,100%{{text-shadow:0 0 16px currentColor}}
  50%{{text-shadow:0 0 28px currentColor}}}}

/* ── minimal: top-left whisper ─────────────────────────────── */
body.minimal .box{{position:fixed;left:24px;top:24px;
  display:flex;align-items:baseline;gap:6px}}
body.minimal .l{{display:none}} /* label hidden - the colour is the label */
body.minimal .v{{font-size:38px;letter-spacing:-1.5px;line-height:1}}
body.minimal .u{{font-size:18px;font-weight:500;opacity:.7;letter-spacing:-.2px}}

/* ── corner: bottom-right block ────────────────────────────── */
body.corner .box{{position:fixed;right:28px;bottom:28px;
  background:var(--surface-strong);
  backdrop-filter:blur(16px);-webkit-backdrop-filter:blur(16px);
  padding:18px 24px;border-radius:14px;
  border:1px solid var(--line);min-width:200px;text-align:right;
  display:flex;flex-direction:column;align-items:flex-end;gap:2px}}
body.corner .box::before{{content:"";position:absolute;left:20px;right:20px;top:0;
  height:1px;background:linear-gradient(90deg,transparent,currentColor,transparent);
  opacity:.6;transition:opacity .42s ease}}
body.corner .l{{font-size:11px;text-transform:uppercase;letter-spacing:2px;
  font-weight:700;color:currentColor;opacity:.78;text-shadow:0 0 12px currentColor}}
body.corner .v{{font-size:46px;font-weight:800;letter-spacing:-2px;
  margin-top:2px;line-height:1}}
body.corner .u{{font-size:22px;opacity:.6;font-weight:400;margin-left:3px}}

/* ── strip: a glowing line across the bottom edge ────────── */
body.strip{{display:block}}
/* Strip uses the .label DOM node (with #v2) for its number on the
   right side. Hide the primary .group entirely so we don't render two
   copies of the number. */
body.strip .group{{display:none}}
body.strip .box{{position:fixed;left:0;right:0;bottom:0;height:38px;
  display:flex;align-items:flex-end;animation:none;
  background:none;border:0;padding:0}}
body.strip .track{{position:absolute;left:0;right:0;bottom:0;height:2px;
  background:rgba(255,255,255,.04)}}
body.strip .fill{{position:absolute;left:0;bottom:0;height:2px;width:0;
  background:currentColor;
  box-shadow:0 0 12px currentColor,0 -2px 22px currentColor;
  transition:width .42s var(--ease-out),background-color .35s ease,opacity .35s ease;
  opacity:0}}
body.strip.has-fill .fill{{opacity:1}}
body.strip.live .fill{{animation:stripPulse 3s ease-in-out infinite}}
@keyframes stripPulse{{0%,100%{{box-shadow:0 0 12px currentColor,0 -2px 22px currentColor}}
  50%{{box-shadow:0 0 24px currentColor,0 -2px 36px currentColor}}}}
body.strip .label{{position:absolute;right:24px;bottom:10px;
  display:flex;align-items:baseline;gap:4px;
  opacity:0;transform:translateY(4px);
  transition:opacity .42s ease,transform .42s ease}}
body.strip.has-fill .label{{opacity:1;transform:none}}
body.strip .l{{display:none}}
body.strip .v{{font-size:22px;letter-spacing:-.6px;line-height:1}}
body.strip .u{{font-size:11px;font-weight:600;letter-spacing:2px;
  text-transform:uppercase;opacity:.65}}

/* ── focus: dead-centre intermission card ─────────────────── */
body.focus{{display:flex;align-items:center;justify-content:center}}
body.focus .box{{background:rgba(0,0,0,.78);
  backdrop-filter:blur(18px);-webkit-backdrop-filter:blur(18px);
  padding:40px 64px;border-radius:24px;
  border:1px solid color-mix(in oklch,currentColor 22%,transparent);
  text-align:center;box-shadow:0 30px 80px rgba(0,0,0,.55);
  animation:focusIn .5s var(--ease-out) both}}
@keyframes focusIn{{from{{transform:scale(.94);opacity:0;filter:blur(10px)}}
  to{{transform:scale(1);opacity:1;filter:blur(0)}}}}
body.focus .l{{font-size:12.5px;text-transform:uppercase;letter-spacing:3.5px;
  color:currentColor;opacity:.78;font-weight:600}}
body.focus .v{{font-size:96px;font-weight:800;letter-spacing:-3px;
  margin-top:10px;line-height:.95}}
body.focus .u{{font-size:34px;opacity:.55;font-weight:400;margin-left:6px}}

/* ── broadcast: TV-news red bar at top. State colour applied to a
   trailing accent strip so the red brand stays even when the
   underlying state goes amber/cyan. ─────────────────────────── */
body.broadcast .box{{position:fixed;left:0;right:0;top:0;height:44px;
  background:linear-gradient(180deg,#c81e1e,#a31616);color:#fff;
  padding:10px 22px;display:flex;align-items:center;gap:18px;
  box-shadow:0 2px 0 rgba(0,0,0,.45),
    inset 0 1px 0 rgba(255,255,255,.25),
    inset 0 -1px 0 rgba(0,0,0,.25);
  animation:bcastIn .45s var(--ease-out) both}}
@keyframes bcastIn{{from{{transform:translateY(-46px)}}to{{transform:translateY(0)}}}}
/* Group lives inside a 44 px bar, so label + number must sit on a
   single baseline rather than stack. */
body.broadcast .group{{display:flex;align-items:baseline;gap:14px}}
body.broadcast .l{{font-size:13px;text-transform:uppercase;letter-spacing:3px;
  font-weight:700;font-family:Georgia,'Times New Roman',serif;color:#fff;
  opacity:.9}}
body.broadcast .v{{font-size:22px;color:#fff;letter-spacing:-.4px;
  text-shadow:0 0 8px rgba(0,0,0,.4)}}
body.broadcast .u{{font-size:14px;opacity:.85;margin-left:1px;color:#fff}}
/* Accent strip on the bottom of the bar carries the state colour. */
body.broadcast .box::after{{content:"";position:absolute;left:0;right:0;bottom:0;
  height:2px;background:currentColor;
  box-shadow:0 0 12px currentColor;opacity:.85;
  transition:background-color .35s ease}}

/* ── ticker: scrolling marquee, seamless wrap ─────────────── */
body.ticker .box{{position:fixed;left:0;right:0;bottom:0;height:38px;
  background:rgba(0,0,0,.88);display:flex;align-items:center;
  border-top:1px solid currentColor;overflow:hidden;
  transition:border-color .35s ease}}
body.ticker .group,body.ticker .v,body.ticker .l,body.ticker .u{{display:none}}
body.ticker .ticker-track{{display:flex;flex-shrink:0;
  animation:tickerScroll 32s linear infinite;
  white-space:nowrap}}
@keyframes tickerScroll{{from{{transform:translateX(0)}}to{{transform:translateX(-50%)}}}}
body.ticker .ticker-cell{{display:inline-flex;align-items:center;gap:14px;
  padding:0 38px;font-size:13px;letter-spacing:.4px;flex-shrink:0;color:#fff}}
body.ticker .ticker-cell .label{{display:inline-flex;text-transform:uppercase;
  letter-spacing:1.6px;font-weight:700;font-size:11px;color:currentColor;
  opacity:.78}}
body.ticker .ticker-cell .value{{font-weight:700;letter-spacing:-.3px;
  color:#fff;text-shadow:0 0 10px currentColor}}
body.ticker .ticker-cell .unit{{opacity:.55;margin-left:1px}}
body.ticker .ticker-cell .sep{{opacity:.3}}
</style></head><body class="{style} state-idle">
<div class="box">
  <div class="track" aria-hidden="true"></div>
  <div class="fill" aria-hidden="true"></div>
  <div class="group">
    <div class="l" id="l">{l_delay}</div>
    <div class="v"><span id="v">0.0</span><span class="u">s</span></div>
  </div>
  <div class="label" aria-hidden="true"><span class="v" id="v2">0.0</span><span class="u">s</span></div>
  <div class="ticker-track" id="ticker-track" aria-hidden="true"></div>
</div>
<script>
'use strict';
const L = {{
  delay:       "{l_delay}",
  preparing:   "{l_preparing}",
  ready:       "{l_ready}",
  active:      "{l_active}",
  passthrough: "{l_passthrough}",
}};
const STYLE = document.body.className.split(/\s+/)[0];
const body = document.body;
const fillEl = document.querySelector('.fill');
const labelEl = document.getElementById('l');
const vEls = document.querySelectorAll('.v #v, .label #v2, .v#v, span#v, span#v2');
const vMain = document.getElementById('v');
const vAlt  = document.getElementById('v2');

function fmtDelay(secs){{
  if (!isFinite(secs) || secs < 0.05) return '0.0';
  return secs.toFixed(1);
}}

// Ease-out cubic tween between numbers - phase changes read as a build
// rather than a snap. Reused for both the main number and the strip's
// label number; they stay in lockstep because the same value flows in.
const tweens = new WeakMap();
function tweenNumber(el, to, dur){{
  if (!el) return;
  const prev = tweens.get(el);
  const from = prev ? prev.target : parseFloat(el.textContent) || 0;
  if (Math.abs(to - from) < 0.005){{ el.textContent = fmtDelay(to); tweens.set(el,{{target:to}}); return; }}
  if (prev && prev.raf) cancelAnimationFrame(prev.raf);
  const start = performance.now();
  const rec = {{ target: to, raf: 0 }};
  function step(now){{
    const t = Math.min(1, (now - start) / dur);
    const e = 1 - Math.pow(1 - t, 3);
    el.textContent = fmtDelay(from + (to - from) * e);
    if (t < 1) rec.raf = requestAnimationFrame(step);
  }}
  rec.raf = requestAnimationFrame(step);
  tweens.set(el, rec);
}}

// `?autohide=off` disables the 4 s idle-dim entirely so the overlay
// stays at full opacity even during passthrough. Default behaviour
// (no param, or `?autohide=on`) is the original dim-after-4s.
const AUTOHIDE = !new URLSearchParams(location.search).get('autohide')
  || new URLSearchParams(location.search).get('autohide') !== 'off';
let idleTimer = null;
function setIdleDim(idle){{
  if (!AUTOHIDE){{ body.classList.remove('idle-dim'); return; }}
  if (idle){{
    if (!idleTimer && !body.classList.contains('idle-dim')){{
      idleTimer = setTimeout(() => {{ body.classList.add('idle-dim'); idleTimer = null; }}, 4000);
    }}
  }} else {{
    if (idleTimer){{ clearTimeout(idleTimer); idleTimer = null; }}
    body.classList.remove('idle-dim');
  }}
}}

let lastPhase = null;
function maybeFlashPhase(phase){{
  if (lastPhase !== null && lastPhase !== phase){{
    body.classList.add('phase-flash');
    setTimeout(() => body.classList.remove('phase-flash'), 480);
  }}
  lastPhase = phase;
}}

// Replace all state-* and live classes in one swap so the body's
// colour class stays consistent (no flash of multiple colours during a
// transition).
function setState(stateClass, opts){{
  body.classList.remove('state-idle','state-amber','state-ok','state-red','live','has-fill');
  body.classList.add(stateClass);
  if (opts && opts.live)    body.classList.add('live');
  if (opts && opts.hasFill) body.classList.add('has-fill');
}}

function renderTicker(parts){{
  const track = document.getElementById('ticker-track');
  if (!track) return;
  const cell = ''
    + '<span class="label">' + parts.label + '</span>'
    + '<span class="value">' + parts.valueText + '<span class="unit">s</span></span>'
    + '<span class="sep">·</span>'
    + '<span class="label">' + parts.status + '</span>';
  track.innerHTML =
    '<span class="ticker-cell">' + cell + '</span>' +
    '<span class="ticker-cell">' + cell + '</span>';
}}

function paint(s){{
  let displayMs = 0, fillFrac = 0;
  let stateClass = 'state-idle';
  let live = false, hasFill = false;
  let label = L.delay, status = L.passthrough;

  if (!s.ingest_alive){{
    stateClass = 'state-idle';
    status = '-';
  }} else if (s.phase === 'idle'){{
    stateClass = 'state-idle';
    status = L.passthrough;
  }} else if (s.phase === 'preparing'){{
    stateClass = 'state-amber'; hasFill = true;
    displayMs = s.buffer_fill_ms || 0;
    fillFrac = Math.max(0, Math.min(1, displayMs / (s.armed_delay_ms || 1)));
    label = L.preparing; status = L.preparing;
  }} else if (s.phase === 'ready'){{
    stateClass = 'state-ok'; hasFill = true;
    displayMs = s.armed_delay_ms || 0;
    fillFrac = 1;
    label = L.ready; status = L.ready;
  }} else {{
    stateClass = 'state-ok'; hasFill = true; live = true;
    displayMs = s.target_delay_ms || s.armed_delay_ms || s.current_delay_ms || 0;
    fillFrac = 1;
    label = L.delay; status = L.active;
  }}
  if (s.ingest_alive && s.destinations_total > 0 && s.destinations_alive === 0){{
    stateClass = 'state-red'; hasFill = true; live = false;
  }}
  setState(stateClass, {{ live, hasFill }});

  if (fillEl) fillEl.style.width = (fillFrac * 100).toFixed(2) + '%';
  const secs = displayMs / 1000;
  if (STYLE === 'ticker'){{
    renderTicker({{ label, valueText: fmtDelay(secs), status }});
  }} else {{
    if (labelEl) labelEl.textContent = label;
    tweenNumber(vMain, secs, 380);
    tweenNumber(vAlt,  secs, 380);
  }}

  setIdleDim(!s.ingest_alive || s.phase === 'idle');
  maybeFlashPhase(s.phase);
}}

function start(){{
  // /overlay-events, not /events: an OBS browser source has no session, and
  // the overlay feed is the read-only one it is allowed to have.
  if (window.EventSource){{
    try {{
      const es = new EventSource('/overlay-events');
      es.onmessage = e => {{ try {{ paint(JSON.parse(e.data)); }} catch(_){{}} }};
      es.onerror = () => {{ es.close(); setTimeout(startPolling, 1000); }};
      return;
    }} catch(_){{}}
  }}
  startPolling();
}}
function startPolling(){{
  async function tick(){{ try {{ paint(await (await fetch('/overlay-state')).json()); }} catch(_){{}} }}
  tick(); setInterval(tick, 500);
}}
start();
</script></body></html>
"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Settings save: tracing_enabled regression test ──────────
    //
    // `post_config` whitelists which form keys get dispatched into
    // `apply_field_str`. Any field that handles a save but isn't in
    // the whitelist gets silently dropped - exactly what happened
    // with `tracing_enabled` between 3f9db09 (toggle added) and
    // 6a3990b (default flipped to off). Invisible until users
    // actually tried to enable the toggle on a fresh install.

    /// A controller with a real ring, so `arm_refusal` can be asked the
    /// question the HTTP handlers ask it.
    fn refusal_harness(
        buffer_mb: u64,
    ) -> (
        Arc<Controller>,
        Arc<watch::Sender<Settings>>,
        std::path::PathBuf,
    ) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("ic-arm-refusal-{nanos}.buf"));
        let _ = std::fs::remove_file(&path);
        let ring =
            Arc::new(crate::buffer::DiskRing::create(&path, 4 * 1024 * 1024).expect("ring create"));
        let ctrl = Arc::new(Controller::new(ring, 0));
        let mut s = Settings::defaults();
        s.buffer_mb = buffer_mb;
        (ctrl, Arc::new(watch::channel(s).0), path)
    }

    /// An overlay is a picture, not a control surface.
    ///
    /// The feed it reads has to be public, because an OBS browser source
    /// cannot log in and an overlay that stops updating under a dashboard
    /// password is a broken overlay. Public is only acceptable while the
    /// payload stays a picture's worth of data, so this pins both halves:
    /// the fields the renderers need are present, and nothing that names the
    /// streamer, describes the machine, or authorises anything is.
    #[test]
    fn the_overlay_feed_carries_only_what_an_overlay_paints() {
        let (ctrl, settings, path) = refusal_harness(1024);
        {
            let mut s = settings.borrow().clone();
            s.destinations.push(crate::config::Destination {
                id: "dest-secret-id".into(),
                name: "Twitch main - client account".into(),
                enabled: true,
                platform: "custom".into(),
                stream_key: "SECRETKEY".into(),
                custom_egress_url: "rtmp://host/app".into(),
                twitch_ingest: String::new(),
                youtube_ingest: String::new(),
                vod_audio: false,
                vod_audio_inject_eb: false,
                stream_format: "horizontal".into(),
                audio_track: "auto".into(),
            });
            // `send_replace`, not `send`: the harness drops its receiver, and
            // `send` is a no-op with no receivers - the destination would
            // never land and the leak assertions below would all pass on an
            // empty list, proving nothing.
            settings.send_replace(s);
        }
        let json = overlay_state_json(&ctrl, &settings);
        // The fixture has to reach the payload or every leak assertion below
        // passes on an empty list and proves nothing. Anchored on the entry
        // opening rather than the whole entry, so a field smuggled in beside
        // `alive` still satisfies this and is caught where it should be.
        //
        // (`destinations_total` would be the wrong anchor: like `/state`, it
        // counts the controller's live destinations, not the configured ones.)
        assert!(
            json.contains(r#""destinations":[{"alive""#),
            "the fixture destination never landed: {json}"
        );

        // Everything the two renderers read, or a widget silently blanks.
        for field in [
            "phase",
            "armed_delay_ms",
            "target_delay_ms",
            "current_delay_ms",
            "buffer_fill_ms",
            "buffer_target_ms",
            "ingest_alive",
            "destinations_alive",
            "destinations_total",
            "cuts",
            "bitrate_kbps",
            "alive",
        ] {
            assert!(json.contains(field), "overlays render {field}: {json}");
        }

        // And nothing else. A destination's name is the sharpest case: it is
        // the streamer's own words ("client account"), and an overlay is on
        // screen, so `/state`'s list would have put it one bug from air.
        for leak in [
            "Twitch main",
            "dest-secret-id",
            "obs_url",
            "publisher_token",
            "cpu_pct",
            "rss_bytes",
            "uptime_secs",
            "webhook",
            "compat_warning",
            "hotkey_conflicts",
            "last_action",
            "ingest_key",
            "dock_token",
            "SECRETKEY",
        ] {
            assert!(
                !json.contains(leak),
                "an overlay has no use for {leak}: {json}"
            );
        }
        let _ = std::fs::remove_file(path);
    }

    /// Reading the overlay feed must never imply being able to drive the
    /// delay. `/state` is Control, and Control is arm / activate / cut /
    /// go-live - so pointing overlays at it, or widening it to reach them,
    /// would have turned a picture into a way to cut someone's stream.
    #[test]
    fn the_overlay_feed_is_readable_but_not_a_control_path() {
        assert_eq!(classify_access("GET", "/overlay-state"), Access::Public);
        assert_eq!(classify_access("GET", "/overlay-events"), Access::Public);

        // GET only. Nothing writes through these paths today, and if
        // something ever tries it lands on the admin default rather than
        // inheriting the read exemption.
        assert_eq!(classify_access("POST", "/overlay-state"), Access::Admin);
        assert_eq!(classify_access("POST", "/overlay-events"), Access::Admin);

        // The dashboard's own feed stays behind a login. If this ever
        // relaxes to Public, the overlay split above has been undone and
        // every control route went with it.
        assert_eq!(classify_access("GET", "/state"), Access::Control);
        assert_eq!(classify_access("GET", "/events"), Access::Control);
        assert_eq!(classify_access("POST", "/arm"), Access::Control);
    }

    /// The shipped overlay JS must ask for the overlay feed, not the
    /// dashboard's. This is the half a Rust test cannot otherwise see: the
    /// renderers are JavaScript, and pointing one back at `/state` would
    /// compile, pass every other test, and freeze every overlay the first
    /// time a user set a dashboard password.
    #[test]
    fn neither_overlay_renderer_asks_for_the_dashboard_feed() {
        // Matching one exact spelling (`fetch('/state')`) let every other
        // spelling through: double quotes, a template literal, a query string,
        // an origin prefix. So the check is "a URL string that ends in this
        // path", whatever surrounds it. Prose in comments ("not /events and
        // /state.") is not a URL string and must not count.
        for (js, want) in [
            (r#"fetch("/state")"#, 1),
            ("new EventSource(`/events`)", 1),
            ("fetch('/state?t=1')", 1),
            ("fetch(location.origin + '/events#x')", 1),
            ("// read /overlay-events, not /events and /state.", 0),
        ] {
            assert_eq!(
                feed_references(js, "/events") + feed_references(js, "/state"),
                want,
                "{js}"
            );
        }

        let saved = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/web/overlay-runtime.js"
        ));
        let builtin = overlay_html("");
        for (what, js) in [
            ("saved overlays", saved),
            ("the built-in overlay", builtin.as_str()),
        ] {
            assert!(
                feed_references(js, "/overlay-events") > 0
                    && feed_references(js, "/overlay-state") > 0,
                "{what} must read the overlay feed"
            );
            assert_eq!(
                feed_references(js, "/events") + feed_references(js, "/state"),
                0,
                "{what} must not read the dashboard feed - it needs a session"
            );
        }
    }

    /// How many times `js` uses `path` as a URL: the path followed by a
    /// string's closing quote, a query or a fragment. `/overlay-state` does not
    /// contain `/state`, so the two feeds never match each other.
    fn feed_references(js: &str, path: &str) -> usize {
        js.match_indices(path)
            .filter(|(at, _)| {
                matches!(
                    js[at + path.len()..].chars().next(),
                    Some('\'' | '"' | '`' | '?' | '#')
                )
            })
            .count()
    }

    /// The dashboard's copy of the offline guard. The hotkey and MIDI paths
    /// refuse to build a delay with nothing publishing; `/arm` and `/delay`
    /// have to refuse it too, or the browser is a way around the rule.
    #[test]
    fn arm_refusal_matches_the_hotkey_rules() {
        let (ctrl, settings, path) = refusal_harness(1024);

        // Nothing publishing: arming is refused, and says why.
        let refused = arm_refusal(&ctrl, &settings, 5_000).expect("offline arm is refused");
        assert_eq!(refused.0, "409 Conflict");
        assert!(
            refused.2.contains(crate::controller::NO_INGEST),
            "the reason has to reach the user: {}",
            refused.2
        );

        // Disarming is never refused - it frees the buffer, and OBS dying
        // mid-delay is exactly when someone reaches for it.
        assert!(arm_refusal(&ctrl, &settings, 0).is_none(), "disarm offline");

        // With a publisher, a sane delay goes through.
        ctrl.mark_ingest_alive_for_test();
        assert!(arm_refusal(&ctrl, &settings, 5_000).is_none());

        // More delay than the buffer can hold never fills, so it is refused
        // with the size it can actually manage.
        let (small, small_settings, small_path) = refusal_harness(8);
        small.mark_ingest_alive_for_test();
        let too_big = arm_refusal(&small, &small_settings, 600_000).expect("over capacity");
        assert_eq!(too_big.0, "409 Conflict");
        assert!(
            too_big.2.contains("Buffer too small") && too_big.2.contains("Raise the buffer size"),
            "the message has to read as a sentence: {}",
            too_big.2
        );
        assert!(
            !too_big.2.contains("  "),
            "no stray whitespace in a user-facing string: {}",
            too_big.2
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&small_path);
    }

    /// The whitelist and the applier have to agree, in both directions, for
    /// every binding key. This is the pair that broke: `midi.<action>` was
    /// missing from both, so the dashboard's clear button reported success
    /// and changed nothing.
    #[test]
    fn binding_keys_are_settable_and_actually_applied() {
        for action in config::ACTIONS {
            let hotkey = format!("hotkey.{action}");
            let midi = format!("midi.{action}");
            assert!(is_settable_key(&hotkey), "{hotkey} must be settable");
            assert!(is_settable_key(&midi), "{midi} must be settable");

            let mut s = Settings::defaults();
            apply_field_str(&mut s, &hotkey, "ctrl+alt+d");
            apply_field_str(&mut s, &midi, "note:1:36");
            let hotkeys: Vec<(&str, String)> = s
                .hotkeys
                .entries()
                .iter()
                .map(|(a, combo)| (*a, combo.to_string()))
                .collect();
            let signatures: Vec<(&str, String)> = s
                .midi
                .entries()
                .iter()
                .map(|(a, sig)| (*a, sig.to_string()))
                .collect();
            for (bound_action, combo) in &hotkeys {
                let expected = if *bound_action == action {
                    "Ctrl+Alt+D"
                } else {
                    ""
                };
                assert_eq!(combo, expected, "{hotkey} landed on {bound_action}");
            }
            for (bound_action, sig) in &signatures {
                let expected = if *bound_action == action {
                    "note:1:36"
                } else {
                    ""
                };
                assert_eq!(sig, expected, "{midi} landed on {bound_action}");
            }

            // And the clear the × button sends must actually clear it.
            apply_field_str(&mut s, &hotkey, "");
            apply_field_str(&mut s, &midi, "");
            assert!(s.hotkeys.entries().iter().all(|(_, c)| c.is_empty()));
            assert!(s.midi.entries().iter().all(|(_, sig)| sig.is_empty()));
        }

        // Secrets and per-route fields stay out of this door.
        assert!(!is_settable_key("dock_token"));
        assert!(!is_settable_key("dashboard_password_hash"));
        assert!(!is_settable_key("configured"));
    }

    #[test]
    fn normalize_stream_format_rules() {
        // Vertical honored on non-Twitch platforms.
        assert_eq!(
            normalize_stream_format("youtube", Some("vertical")),
            "vertical"
        );
        assert_eq!(
            normalize_stream_format("kick", Some("vertical")),
            "vertical"
        );
        assert_eq!(
            normalize_stream_format("custom", Some("vertical")),
            "vertical"
        );
        // Twitch is always horizontal (native dual-canvas), even if the form
        // somehow carried "vertical".
        assert_eq!(
            normalize_stream_format("twitch", Some("vertical")),
            "horizontal"
        );
        // Everything else falls back to horizontal.
        assert_eq!(
            normalize_stream_format("youtube", Some("horizontal")),
            "horizontal"
        );
        assert_eq!(normalize_stream_format("youtube", None), "horizontal");
        assert_eq!(
            normalize_stream_format("youtube", Some("garbage")),
            "horizontal"
        );
    }

    /// One settable key in the save round trip below: what the form posts to
    /// set it, what the field then reads as, what the form posts to put it
    /// back, and how to read the field.
    struct SettableRow {
        key: &'static str,
        set: &'static str,
        reads_as: &'static str,
        reset: &'static str,
        read: fn(&Settings) -> String,
    }

    /// A form value, percent-encoded so `parse_form` hands the route exactly
    /// this string (a raw `+` in a hotkey would otherwise arrive as a space).
    fn form_encode(value: &str) -> String {
        value
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }

    /// Every key `POST /config` accepts, driven through the real handler, the
    /// config file, and `Settings::load`, then set back the same way.
    ///
    /// This is the regression class that kept recurring: a key missing from
    /// the whitelist, from `apply_field_str`, or from save/load is dropped
    /// without an error, and the dashboard shows "Saved" for a setting that
    /// reverts on the next launch (tracing_enabled, ingest_key and the MIDI
    /// clear button all shipped like that). Calling `apply_field_str` alone
    /// skipped the whitelist, so removing a key from it still passed.
    #[tokio::test]
    async fn every_settable_key_survives_post_config_save_and_load() {
        let rows: &[SettableRow] = &[
            SettableRow {
                key: "ingest_port",
                set: "19350",
                reads_as: "19350",
                reset: "1935",
                read: |s| s.ingest_port.to_string(),
            },
            SettableRow {
                key: "ingest_bind_all",
                set: "on",
                reads_as: "true",
                reset: "false",
                read: |s| s.ingest_bind_all.to_string(),
            },
            SettableRow {
                key: "ingest_key",
                set: "loop_key-1",
                reads_as: "loop_key-1",
                reset: "",
                read: |s| s.ingest_key.clone(),
            },
            SettableRow {
                key: "web_port",
                set: "17799",
                reads_as: "17799",
                reset: "7799",
                read: |s| s.web_port.to_string(),
            },
            SettableRow {
                key: "web_bind_all",
                set: "1",
                reads_as: "true",
                reset: "",
                read: |s| s.web_bind_all.to_string(),
            },
            SettableRow {
                key: "buffer_mb",
                set: "640",
                reads_as: "640",
                reset: "500",
                read: |s| s.buffer_mb.to_string(),
            },
            SettableRow {
                key: "buffer_path",
                set: "loop-ring.buf",
                reads_as: "loop-ring.buf",
                reset: "./instantclone.buf",
                read: |s| s.buffer_path.display().to_string(),
            },
            SettableRow {
                key: "overlays_dir",
                set: "loop-overlays",
                reads_as: "loop-overlays",
                reset: "./overlays",
                read: |s| s.overlays_dir.display().to_string(),
            },
            SettableRow {
                key: "tracing_enabled",
                set: "true",
                reads_as: "true",
                reset: "false",
                read: |s| s.tracing_enabled.to_string(),
            },
            // A checkbox posts "on" when ticked; it must read as true.
            SettableRow {
                key: "auto_arm_on_connect",
                set: "on",
                reads_as: "true",
                reset: "off",
                read: |s| s.auto_arm_on_connect.to_string(),
            },
            SettableRow {
                key: "auto_activate_when_ready",
                set: "true",
                reads_as: "true",
                reset: "0",
                read: |s| s.auto_activate_when_ready.to_string(),
            },
            SettableRow {
                key: "auto_arm_delay_ms",
                set: "30000",
                reads_as: "30000",
                reset: "15000",
                read: |s| s.auto_arm_delay_ms.to_string(),
            },
            SettableRow {
                key: "update_check_enabled",
                set: "false",
                reads_as: "false",
                reset: "true",
                read: |s| s.update_check_enabled.to_string(),
            },
            SettableRow {
                key: "open_dashboard_on_launch",
                set: "off",
                reads_as: "false",
                reset: "on",
                read: |s| s.open_dashboard_on_launch.to_string(),
            },
            SettableRow {
                key: "midi_device",
                set: "LoopDevice",
                reads_as: "LoopDevice",
                reset: "",
                read: |s| s.midi_device.clone(),
            },
            SettableRow {
                key: "twitch_client_id",
                set: " abcdefghij0123456789klmnopqrst ",
                reads_as: "abcdefghij0123456789klmnopqrst",
                reset: "",
                read: |s| s.twitch_client_id.clone(),
            },
        ];

        // The table and the whitelist must list the same keys, so a new
        // settable key cannot skip this test and a dropped one cannot hide.
        for key in SETTABLE_KEYS {
            assert!(
                rows.iter().any(|r| r.key == *key),
                "{key} is settable but has no row here - add one"
            );
        }
        for row in rows {
            assert!(is_settable_key(row.key), "{} is not settable", row.key);
        }

        let defaults = Settings::defaults();
        for row in rows {
            // Precondition: the new value differs from the default, or the
            // round trip would pass on a key that never landed.
            assert_ne!(
                (row.read)(&defaults),
                row.reads_as,
                "{}: pick a non-default",
                row.key
            );
        }

        // Every binding, too: `hotkey.<action>` and `midi.<action>`.
        let letters = ["A", "B", "C", "D", "E"];
        let bindings: Vec<(String, String, String)> = config::ACTIONS
            .iter()
            .zip(letters)
            .enumerate()
            .flat_map(|(i, (action, letter))| {
                [
                    (
                        action.to_string(),
                        format!("hotkey.{action}"),
                        format!("Ctrl+Alt+{letter}"),
                    ),
                    (
                        action.to_string(),
                        format!("midi.{action}"),
                        format!("note:1:{}", 36 + i),
                    ),
                ]
            })
            .collect();

        let live = Live::new(Settings::defaults());
        let post = |pairs: Vec<(String, String)>| {
            pairs
                .iter()
                .map(|(k, v)| format!("{k}={}", form_encode(v)))
                .collect::<Vec<_>>()
                .join("&")
        };

        let set_body = post(
            rows.iter()
                .map(|r| (r.key.to_string(), r.set.to_string()))
                .chain(bindings.iter().map(|(_, k, v)| (k.clone(), v.clone())))
                .collect(),
        );
        let (status, _, body) =
            post_config(&set_body, &live.ctrl, &live.settings, &live.cfg_path).await;
        assert_eq!(status, "200 OK", "{body}");
        let loaded = Settings::load(&live.cfg_path).expect("config was written");
        for row in rows {
            assert_eq!((row.read)(&loaded), row.reads_as, "{} after load", row.key);
            assert_eq!(
                (row.read)(&live.settings.borrow()),
                row.reads_as,
                "{} live",
                row.key
            );
        }
        for (action, key, value) in &bindings {
            let bound = if key.starts_with("hotkey.") {
                loaded.hotkeys.entries()
            } else {
                loaded.midi.entries()
            };
            let got = bound
                .iter()
                .find(|(a, _)| *a == action.as_str())
                .map(|(_, v)| *v);
            assert_eq!(got, Some(value.as_str()), "{key} after load");
        }

        let reset_body = post(
            rows.iter()
                .map(|r| (r.key.to_string(), r.reset.to_string()))
                .chain(bindings.iter().map(|(_, k, _)| (k.clone(), String::new())))
                .collect(),
        );
        let (status, _, body) =
            post_config(&reset_body, &live.ctrl, &live.settings, &live.cfg_path).await;
        assert_eq!(status, "200 OK", "{body}");
        let loaded = Settings::load(&live.cfg_path).expect("config was written");
        for row in rows {
            assert_eq!(
                (row.read)(&loaded),
                (row.read)(&defaults),
                "{} back to default",
                row.key
            );
        }
        assert!(loaded.hotkeys.entries().iter().all(|(_, c)| c.is_empty()));
        assert!(loaded.midi.entries().iter().all(|(_, m)| m.is_empty()));
    }

    #[test]
    fn crash_protection_preview_applies_unsaved_edits() {
        let saved = crate::crash_protection::CrashProtection::default();
        let landscape = crash_protection_preview("", &saved);
        let arcade = crash_protection_preview("crash_protection.theme=arcade", &saved);
        let vertical = crash_protection_preview("orientation=vertical", &saved);
        // PNG: width and height sit big-endian at bytes 16 and 20.
        let size = |png: &[u8]| {
            let read = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
            (read(16), read(20))
        };
        assert_eq!(&landscape[1..4], b"PNG");
        assert_ne!(landscape, arcade, "query edits reach the render");
        assert_eq!(size(&vertical), (270, 480));
        let thumb = crash_protection_preview("size=thumb", &saved);
        assert_eq!(size(&thumb), (192, 108));
        let strip = crash_protection_preview("frames=24", &saved);
        assert_eq!(size(&strip), (480, 270 * 24), "24 frames stacked");
        let capped = crash_protection_preview("frames=500&size=thumb", &saved);
        assert_eq!(size(&capped), (192, 108 * 24));
        for preview_only in ["orientation", "size", "phase", "frames"] {
            assert!(
                !is_settable_key(preview_only),
                "preview-only params never persist"
            );
        }
    }

    #[test]
    fn apply_field_str_persists_crash_protection_fields() {
        let mut s = crate::config::Settings::defaults();
        assert!(is_settable_key("crash_protection.enabled"));
        apply_field_str(&mut s, "crash_protection.enabled", "on");
        apply_field_str(&mut s, "crash_protection.theme", "arcade");
        apply_field_str(&mut s, "crash_protection.hold_secs", "45");
        assert!(s.crash_protection.enabled);
        assert_eq!(
            s.crash_protection.theme,
            crate::crash_protection::SlateTheme::Arcade
        );
        assert_eq!(s.crash_protection.hold_secs, 45);
    }

    #[test]
    fn apply_field_str_parses_auto_arm_delay_ms() {
        let mut s = crate::config::Settings::defaults();
        // defaults() seeds at 15 s; replace via the form-key path.
        assert_eq!(s.auto_arm_delay_ms, 15_000);
        apply_field_str(&mut s, "auto_arm_delay_ms", "30000");
        assert_eq!(s.auto_arm_delay_ms, 30_000);
        // Garbage values are ignored (parse failure leaves the field
        // alone) so a hand-edited form doesn't reset to zero.
        apply_field_str(&mut s, "auto_arm_delay_ms", "not-a-number");
        assert_eq!(s.auto_arm_delay_ms, 30_000);
    }

    // ── OBS multitrack-config proxy helpers ──────────────────────
    //
    // The proxy path forwards OBS's POST to Twitch with our
    // destination's stream key swapped in, then rewrites the response
    // to point the multi-track ingest at us. Both string helpers run
    // without a JSON parser, so they're easy to get subtly wrong -
    // these tests pin down the invariants we depend on.

    #[test]
    fn replace_auth_field_swaps_value_in_typical_obs_body() {
        // OBS's actual payload has many top-level keys before/after
        // `authentication`. Verify our string ops cope with both
        // orderings and don't touch sibling fields.
        let body = r#"{"authentication":"live_typed_in_obs","capabilities":{"cpu":{"name":"AMD"}},"client":{"name":"obs-studio"}}"#;
        let patched = replace_auth_field(body, "live_real_twitch_key").unwrap();
        assert!(patched.contains(r#""authentication":"live_real_twitch_key""#));
        assert!(!patched.contains("live_typed_in_obs"));
        // Sibling keys must survive untouched.
        assert!(patched.contains(r#""capabilities":{"cpu":{"name":"AMD"}}"#));
        assert!(patched.contains(r#""client":{"name":"obs-studio"}"#));
    }

    #[test]
    fn replace_auth_field_tolerates_whitespace_around_colon() {
        // OBS's encoder formats with `"key": "value"` indent; some
        // libs emit compact `"key":"value"` instead. Both must work.
        let indented = r#"{ "authentication" : "old" , "x": 1 }"#;
        let patched = replace_auth_field(indented, "new").unwrap();
        assert!(patched.contains(r#""new""#));
        assert!(!patched.contains(r#""old""#));
    }

    #[test]
    fn replace_auth_field_returns_none_when_absent() {
        // If OBS ever changes their schema and drops `authentication`,
        // we must NOT silently emit a half-modified body that Twitch
        // accepts but with wrong values. `None` triggers the static
        // fallback at the caller.
        let body = r#"{"client":"obs-studio"}"#;
        assert!(replace_auth_field(body, "x").is_none());
    }

    #[test]
    fn offer_only_h264_narrows_the_codecs_obs_offers() {
        // OBS 32 serialises the request with nlohmann (compact, keys sorted).
        let body = r#"{"client":{"name":"obs-studio","supported_codecs":["av1","h265","h264"],"version":"32.2.2"},"preferences":{"canvases":[{"width":1080}]}}"#;
        assert_eq!(
            offer_only_h264(body).unwrap(),
            r#"{"client":{"name":"obs-studio","supported_codecs":["h264"],"version":"32.2.2"},"preferences":{"canvases":[{"width":1080}]}}"#
        );
        let spaced = r#"{"client": {"supported_codecs" : [ "h265", "h264" ] }}"#;
        assert_eq!(
            offer_only_h264(spaced).unwrap(),
            r#"{"client": {"supported_codecs" : ["h264"] }}"#
        );
    }

    #[test]
    fn offer_only_h264_leaves_requests_it_cannot_narrow() {
        // No list, no H.264 in it, or a list that isn't an array: Twitch
        // must still get a request, so the caller keeps the original.
        assert!(offer_only_h264(r#"{"client":{"name":"obs-studio"}}"#).is_none());
        assert!(offer_only_h264(r#"{"client":{"supported_codecs":["h265"]}}"#).is_none());
        assert!(offer_only_h264(r#"{"supported_codecs":"h264","x":["h264"]}"#).is_none());
    }

    #[test]
    fn only_a_horizontal_destination_besides_twitch_narrows_the_codecs() {
        let dest = |platform: &str, enabled: bool, format: &str| config::Destination {
            id: platform.into(),
            name: platform.into(),
            enabled,
            platform: platform.into(),
            stream_key: "k".into(),
            custom_egress_url: String::new(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: format.into(),
            audio_track: "auto".into(),
        };
        let twitch = dest("twitch", true, "horizontal");
        let test_sink = dest("sink", true, "horizontal");
        let kick = dest("kick", true, "horizontal");
        let kick_off = dest("kick", false, "horizontal");
        let tiktok = dest("restream", true, "vertical");
        let second_twitch = config::Destination {
            stream_key: "other-account".into(),
            ..twitch.clone()
        };
        assert!(!feeds_main_track_beyond_twitch(
            &[twitch.clone(), test_sink],
            "k"
        ));
        assert!(!feeds_main_track_beyond_twitch(
            &[twitch.clone(), kick_off],
            "k"
        ));
        assert!(!feeds_main_track_beyond_twitch(
            &[twitch.clone(), tiktok],
            "k"
        ));
        assert!(feeds_main_track_beyond_twitch(
            &[twitch.clone(), second_twitch],
            "k"
        ));
        assert!(feeds_main_track_beyond_twitch(&[twitch, kick], "k"));
    }

    #[test]
    fn rewrite_url_templates_replaces_every_endpoint() {
        // Twitch returns multiple `ingest_endpoints` for regional
        // load-balancing. Every one of them must end up pointing at
        // our localhost ingest - missing even one leaves a chance
        // OBS picks a Twitch URL and bypasses our proxy.
        let response = r#"{"ingest_endpoints":[{"url_template":"rtmps://fra.contribute.live-video.net/app/{stream_key}"},{"url_template":"rtmps://jfk.contribute.live-video.net/app/{stream_key}"}]}"#;
        let rewritten = rewrite_url_templates(response, "rtmp://127.0.0.1:1935/live/{stream_key}");
        assert_eq!(rewritten.matches("contribute.live-video.net").count(), 0);
        assert_eq!(
            rewritten
                .matches("rtmp://127.0.0.1:1935/live/{stream_key}")
                .count(),
            2
        );
    }

    #[test]
    fn rewrite_url_templates_preserves_unrelated_fields() {
        // The function must not corrupt other fields that happen to
        // contain `url_template` as a substring of their value (e.g.
        // a `"description": "url_template is the field..."`).
        // Our matcher requires the full quoted `"url_template"` key
        // marker, so adjacent text is safe.
        let response = r#"{"description":"see url_template","ingest_endpoints":[{"url_template":"rtmp://old"}]}"#;
        let rewritten = rewrite_url_templates(response, "rtmp://new");
        assert!(rewritten.contains(r#""description":"see url_template""#));
        assert!(rewritten.contains(r#""rtmp://new""#));

        // The sharper decoy: a VALUE that is exactly "url_template", quotes
        // and all. Only a key is followed by a colon; the matcher used to take
        // the next field's value for this one's and drop the key in between.
        let response = r#"{"note":"url_template","keep":"rtmp://decoy","ingest_endpoints":[{"url_template":"rtmp://old"}]}"#;
        let rewritten = rewrite_url_templates(response, "rtmp://new");
        assert!(
            rewritten.contains(r#""note":"url_template","keep":"rtmp://decoy""#),
            "{rewritten}"
        );
        assert_eq!(rewritten.matches("rtmp://new").count(), 1, "{rewritten}");
        assert!(!rewritten.contains("rtmp://old"), "{rewritten}");

        // A non-string value is not a URL to replace either.
        let response = r#"{"url_template":null,"keep":"rtmp://decoy"}"#;
        assert_eq!(rewrite_url_templates(response, "rtmp://new"), response);
    }

    #[test]
    fn rewrite_url_templates_handles_zero_matches_gracefully() {
        // If Twitch ever changes the response shape, the function
        // must return the input verbatim rather than corrupt it.
        let response = r#"{"ingest_endpoints":[]}"#;
        let rewritten = rewrite_url_templates(response, "rtmp://new");
        assert_eq!(rewritten, response);
    }

    #[test]
    fn accepts_gzip_when_listed() {
        assert!(accepts_gzip(
            "GET / HTTP/1.1\r\nAccept-Encoding: gzip, deflate, br\r\n"
        ));
        assert!(accepts_gzip("GET / HTTP/1.1\r\nAccept-Encoding: gzip\r\n"));
        // case insensitive header name + token
        assert!(accepts_gzip("GET / HTTP/1.1\r\naccept-encoding: GZIP\r\n"));
    }

    #[test]
    fn accepts_gzip_via_wildcard() {
        assert!(accepts_gzip("GET / HTTP/1.1\r\nAccept-Encoding: *\r\n"));
    }

    #[test]
    fn refuses_when_header_missing_or_identity_only() {
        assert!(!accepts_gzip("GET / HTTP/1.1\r\nHost: x\r\n"));
        assert!(!accepts_gzip(
            "GET / HTTP/1.1\r\nAccept-Encoding: identity\r\n"
        ));
        assert!(!accepts_gzip(
            "GET / HTTP/1.1\r\nAccept-Encoding: deflate, br\r\n"
        ));
    }

    #[test]
    fn refuses_gzip_when_q_zero() {
        // RFC 7231: `q=0` explicitly forbids that coding.
        assert!(!accepts_gzip(
            "GET / HTTP/1.1\r\nAccept-Encoding: gzip;q=0\r\n"
        ));
        assert!(!accepts_gzip(
            "GET / HTTP/1.1\r\nAccept-Encoding: *;q=0\r\n"
        ));
    }

    #[test]
    fn accepts_gzip_with_explicit_quality() {
        assert!(accepts_gzip(
            "GET / HTTP/1.1\r\nAccept-Encoding: gzip;q=0.5, deflate\r\n"
        ));
    }

    /// `/overlay` needs no auth, and its `lang` is written into the page's
    /// own `<html lang="...">`. An unvalidated value there is reflected XSS
    /// on our own origin: script that runs there passes the same-origin CSRF
    /// check, so it can read the stream key or repoint the egress. Both
    /// query parameters are allow-listed; neither is escaped and reflected.
    #[test]
    fn overlay_query_parameters_cannot_reach_the_page() {
        for probe in [
            r#"en"><script>alert(1)</script>"#,
            r#"en" onload="evil()"#,
            "../../etc/passwd",
            "<img src=x onerror=y>",
            "en'",
        ] {
            let enc: String = probe
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_string()
                    } else {
                        format!("%{:02X}", c as u32)
                    }
                })
                .collect();
            for key in ["lang", "style"] {
                let html = overlay_html(&format!("{key}={enc}"));
                assert!(
                    !html.contains(probe),
                    "{key}={probe:?} was reflected into the page"
                );
                assert!(
                    !html.contains("<script>alert"),
                    "{key}={probe:?} injected a script tag"
                );
            }
        }

        // The languages we do have strings for still work.
        assert!(overlay_html("lang=es").contains(r#"<html lang="es""#));
        assert!(
            overlay_html("lang=zz").contains(r#"<html lang="en""#),
            "unknown falls back"
        );
    }

    #[test]
    fn redact_url_hides_long_keys() {
        let redacted = redact_url("rtmp://live.twitch.tv/app/live_123456789_abcdefgh");
        assert!(redacted.starts_with("rtmp://live.twitch.tv/app/"));
        assert!(redacted.contains("…"));
        assert!(
            !redacted.contains("live_123456789_abcdefgh"),
            "the actual key must not appear in the redacted form"
        );
    }

    #[test]
    fn redact_url_leaves_short_keys_alone() {
        // < 12 char tail isn't worth redacting (probably a fake/test key).
        let url = "rtmp://my.server/live/short";
        assert_eq!(redact_url(url), url);
    }

    // ── HTTP request-head parsing ────────────────────────────────────

    /// A malformed body length must not read as "no body".
    ///
    /// Every POST route takes its arguments from the body, and several treat
    /// an absent argument as an instruction: `POST /arm` with no `ms` is a
    /// disarm. So `parse().unwrap_or(0)` meant a merely corrupt request did
    /// not fail - it dropped the streamer's delay. Found by fuzzing the live
    /// HTTP surface, which got a 200 for `Content-Length: -5`.
    #[test]
    fn an_unreadable_body_length_is_refused_not_guessed() {
        let bad = [
            ("POST /arm HTTP/1.1\r\nContent-Length: -5\r\n", "negative"),
            (
                "POST /arm HTTP/1.1\r\nContent-Length: abc\r\n",
                "not a number",
            ),
            ("POST /arm HTTP/1.1\r\nContent-Length: \r\n", "empty"),
            (
                "POST /arm HTTP/1.1\r\nContent-Length: 12x\r\n",
                "trailing junk",
            ),
            (
                "POST /arm HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 9\r\n",
                "two that disagree - the request-smuggling shape",
            ),
            (
                "POST /arm HTTP/1.1\r\nTransfer-Encoding: chunked\r\n",
                "an encoding we do not implement",
            ),
        ];
        for (head, why) in bad {
            let (_, _, len) = parse_request_head(head);
            assert_eq!(len, None, "{why} must be refused, not read as empty");
        }
    }

    /// The ordinary shapes still work. A POST with no body is normal, and two
    /// Content-Length headers that agree are merely redundant.
    #[test]
    fn ordinary_body_lengths_still_parse() {
        for (head, want, why) in [
            (
                "POST /x HTTP/1.1\r\nContent-Length: 0\r\n",
                Some(0),
                "explicit zero",
            ),
            (
                "POST /x HTTP/1.1\r\nHost: y\r\n",
                Some(0),
                "absent is a real zero",
            ),
            ("GET / HTTP/1.1\r\nHost: y\r\n", Some(0), "GET with no body"),
            (
                "POST /x HTTP/1.1\r\nContent-Length: 7\r\nContent-Length: 7\r\n",
                Some(7),
                "duplicates that agree",
            ),
            (
                "POST /x HTTP/1.1\r\nTransfer-Encoding: identity\r\n",
                Some(0),
                "identity is the no-op encoding",
            ),
        ] {
            let (_, _, len) = parse_request_head(head);
            assert_eq!(len, want, "{why}");
        }
    }

    #[test]
    fn parse_request_head_extracts_method_path_and_length() {
        let head = "POST /arm?x=1 HTTP/1.1\r\n\
                    Host: 127.0.0.1:7799\r\n\
                    Content-Length: 42\r\n\
                    Connection: close\r\n";
        let (method, path, len) = parse_request_head(head);
        assert_eq!(method, "POST");
        assert_eq!(path, "/arm?x=1");
        assert_eq!(len, Some(42));
    }

    #[test]
    fn parse_request_head_is_case_insensitive_on_header_name() {
        let head = "POST /x HTTP/1.1\r\ncontent-length: 7\r\n";
        let (_, _, len) = parse_request_head(head);
        assert_eq!(len, Some(7));
    }

    #[test]
    fn parse_origin_host_pulls_both_headers() {
        let head = "POST / HTTP/1.1\r\n\
                    Host: 127.0.0.1:7799\r\n\
                    Origin: http://127.0.0.1:7799\r\n";
        let (o, h) = parse_origin_host(head);
        assert_eq!(o, "http://127.0.0.1:7799");
        assert_eq!(h, "127.0.0.1:7799");
    }

    // ── CSRF policy ──────────────────────────────────────────────────

    #[test]
    fn csrf_allows_all_gets() {
        // GETs are always read-only; never gated.
        assert!(allow_csrf("GET", "", ""));
        assert!(allow_csrf("GET", "https://evil.com", "127.0.0.1:7799"));
    }

    #[test]
    fn csrf_allows_post_without_origin() {
        // CLI tools and Stream Deck don't send Origin - must keep working.
        assert!(allow_csrf("POST", "", "127.0.0.1:7799"));
    }

    #[test]
    fn csrf_allows_same_origin_post() {
        assert!(allow_csrf(
            "POST",
            "http://127.0.0.1:7799",
            "127.0.0.1:7799"
        ));
    }

    #[test]
    fn csrf_blocks_cross_origin_post() {
        // The real attack: a tab on evil.com fetching our local API.
        assert!(!allow_csrf("POST", "https://evil.com", "127.0.0.1:7799"));
    }

    // ── Misc helpers ─────────────────────────────────────────────────

    #[test]
    fn find_subslice_finds_header_terminator() {
        let buf = b"GET / HTTP/1.1\r\nHost: x\r\n\r\nBODY";
        let idx = find_subslice(buf, b"\r\n\r\n").unwrap();
        assert_eq!(&buf[idx..idx + 4], b"\r\n\r\n");
        assert_eq!(&buf[idx + 4..], b"BODY");
    }

    #[test]
    fn find_subslice_returns_none_when_absent() {
        assert!(find_subslice(b"abc", b"xy").is_none());
    }

    // ── `configured` first-run latch ─────────────────────────────────
    //
    // `has_streamable_dest` is the sole condition that raises the
    // `configured` latch. If it ever counted a disabled or key-less
    // destination as streamable - or missed a real one - the toggle/upsert
    // handlers would flip `configured` wrongly and bounce users into (or out
    // of) the first-run wizard. This pins the exact contract.

    fn twitch_dest(enabled: bool, stream_key: &str) -> crate::config::Destination {
        crate::config::Destination {
            id: "d1".into(),
            name: "Main".into(),
            enabled,
            platform: "twitch".into(),
            stream_key: stream_key.into(),
            custom_egress_url: String::new(),
            twitch_ingest: String::new(),
            youtube_ingest: String::new(),
            vod_audio: false,
            vod_audio_inject_eb: false,
            stream_format: String::new(),
            audio_track: "auto".into(),
        }
    }

    /// The vertical track takes the streamer's own encoder only when every
    /// enabled vertical destination plays its codec; one that doesn't (or an
    /// unknown encoder) means H.264 for the whole vertical track.
    #[test]
    fn the_vertical_track_uses_the_streamers_codec_only_where_it_plays() {
        let hevc = crate::local_eb_config::TrackEncoder {
            encoder_type: "h265_texture_amf".into(),
            settings_json: "{}".into(),
        };
        let own = Some(&hevc);
        let to = |dests: &[crate::config::Destination]| own_encoder_for_vertical(own, dests);
        assert!(
            to(&[vertical_dest("youtube", true)]).is_some(),
            "YouTube plays HEVC"
        );
        assert!(to(&[vertical_dest("youtube", true), vertical_dest("kick", true)]).is_none());
        assert!(
            to(&[vertical_dest("youtube", true), vertical_dest("kick", false)]).is_some(),
            "a disabled destination doesn't hold the codec back"
        );
        assert!(
            to(&[vertical_dest("custom", true)]).is_none(),
            "unknown server"
        );
        let h264 = crate::local_eb_config::TrackEncoder {
            encoder_type: "h264_texture_amf".into(),
            settings_json: "{}".into(),
        };
        let kick_only = [vertical_dest("kick", true)];
        assert!(
            own_encoder_for_vertical(Some(&h264), &kick_only).is_some(),
            "the streamer's own H.264 plays everywhere"
        );
        assert!(own_encoder_for_vertical(None, &kick_only).is_none());
    }

    fn vertical_dest(platform: &str, enabled: bool) -> crate::config::Destination {
        crate::config::Destination {
            platform: platform.into(),
            stream_format: "vertical".into(),
            enabled,
            ..twitch_dest(true, "key")
        }
    }

    /// OBS's Additional canvas is encoded only while an enabled destination
    /// streams vertical; otherwise it is a whole encode nobody receives.
    #[test]
    fn the_additional_canvas_is_encoded_only_for_a_vertical_destination() {
        use crate::local_eb_config::ObsCanvas;
        let at = |width, height| ObsCanvas {
            width,
            height,
            fps_num: 60,
            fps_den: 1,
        };
        let offered = [at(2560, 1440), at(1080, 1920)];
        let horizontal_only = [twitch_dest(true, "key")];
        assert_eq!(canvases_to_encode(&offered, &horizontal_only).len(), 1);
        assert_eq!(
            canvases_to_encode(&offered, &[]).len(),
            1,
            "no destinations"
        );
        let off = [vertical_dest("youtube", false)];
        assert_eq!(canvases_to_encode(&offered, &off).len(), 1, "disabled");
        let twitch_set_vertical = [crate::config::Destination {
            stream_format: "vertical".into(),
            ..twitch_dest(true, "key")
        }];
        assert_eq!(
            canvases_to_encode(&offered, &twitch_set_vertical).len(),
            1,
            "Twitch never takes the vertical feed from here"
        );
        let on = [twitch_dest(true, "key"), vertical_dest("kick", true)];
        assert_eq!(canvases_to_encode(&offered, &on), offered.to_vec());
        assert!(canvases_to_encode(&[], &on).is_empty());
    }

    /// End to end past OBS's files: the streamer's own HEVC settings, and a
    /// vertical destination. YouTube plays HEVC, so the vertical track is
    /// the streamer's encoder at the main track's quality per pixel; Kick
    /// doesn't, so it is InstantClone's H.264 at that same bitrate. The main
    /// track is the streamer's untouched either way.
    #[test]
    fn a_vertical_destination_gets_the_streamers_codec_only_if_it_plays_it() {
        use crate::local_eb_config::{build, parse_canvases, StreamerSettings, TrackEncoder};
        let own_settings = r#"{"bitrate":6000,"rate_control":"CBR","preset":"quality"}"#;
        let streamer = StreamerSettings {
            encoder: Some(TrackEncoder {
                encoder_type: "h265_texture_amf".into(),
                settings_json: own_settings.into(),
            }),
            bitrate_kbps: 6000,
            rescale: None,
        };
        let request = r#"{"preferences":{"canvases":[
            {"width":2560,"height":1440,"framerate":{"numerator":60,"denominator":1}},
            {"width":1080,"height":1920,"framerate":{"numerator":60,"denominator":1}}]}}"#;
        let choice = EncoderChoice {
            family: "amd",
            id: "h264_texture_amf",
            reason: String::new(),
        };
        let config_for = |vertical: &str| {
            let dests = [vertical_dest(vertical, true)];
            let canvases = canvases_to_encode(&parse_canvases(request), &dests);
            let own = streamer.encoder.as_ref();
            let vertical_own = own_encoder_for_vertical(own, &dests);
            let (config, _) = build(&canvases, 10_000, Some(&streamer), 1935, |kbps| {
                picked_encoder(vertical_own, &choice, kbps)
            })
            .expect("canvases were sent");
            assert!(crate::config::is_valid_json(&config), "{config}");
            config
        };

        let youtube = config_for("youtube");
        assert!(youtube.contains(r#""type":"h265_texture_amf","width":2560"#));
        assert!(
            youtube.contains(own_settings),
            "main track untouched: {youtube}"
        );
        assert!(
            youtube.contains(r#""type":"h265_texture_amf","width":1080"#),
            "{youtube}"
        );
        assert!(youtube.contains(r#"{"bitrate":3375,"rate_control":"CBR","preset":"quality"}"#));

        let kick = config_for("kick");
        assert!(kick.contains(own_settings), "main track untouched: {kick}");
        assert!(
            kick.contains(r#""type":"h264_texture_amf","width":1080"#),
            "{kick}"
        );
        assert!(kick.contains(r#""bitrate":3375"#), "{kick}");
    }

    fn settings_with_dests(dests: Vec<crate::config::Destination>) -> Settings {
        let mut s = Settings::defaults();
        s.destinations = dests;
        s
    }

    #[test]
    fn has_streamable_dest_requires_enabled_and_addressable() {
        // No destinations at all: not streamable.
        assert!(!has_streamable_dest(&settings_with_dests(vec![])));
        // Enabled + real stream key: streamable.
        assert!(has_streamable_dest(&settings_with_dests(vec![
            twitch_dest(true, "livekey123")
        ])));
        // Disabled, even with a key: does NOT count - toggling the last
        // destination off must not make setup look incomplete.
        assert!(!has_streamable_dest(&settings_with_dests(vec![
            twitch_dest(false, "livekey123")
        ])));
        // Enabled but no key: not addressable yet, so not streamable.
        assert!(!has_streamable_dest(&settings_with_dests(vec![
            twitch_dest(true, "")
        ])));
    }

    /// The latch through the real settings handler: a completed setup whose
    /// only destination is now disabled saves an unrelated setting, and must
    /// stay configured (no wizard reopen); a fresh install saving a usable
    /// destination through the wizard fields must become configured.
    ///
    /// `post_destination_toggle` and `post_destination_delete` carry the same
    /// latch but are not driven here: both call `reconcile_obs_vod_files`,
    /// which rewrites the developer's real OBS `user.ini` (it has no test
    /// seam), and a test must never flip someone's VOD-track flag.
    #[tokio::test]
    async fn configured_latch_only_rises() {
        let mut s = settings_with_dests(vec![twitch_dest(false, "livekey123")]);
        s.configured = true;
        assert!(!has_streamable_dest(&s), "precondition: not streamable");
        let live = Live::new(s);
        let (status, _, body) = post_config(
            "auto_arm_delay_ms=20000",
            &live.ctrl,
            &live.settings,
            &live.cfg_path,
        )
        .await;
        assert_eq!(status, "200 OK", "{body}");
        assert!(
            live.settings.borrow().configured,
            "a completed setup must never re-open the wizard"
        );
        assert!(Settings::load(&live.cfg_path).unwrap().configured);

        let fresh = Live::new(Settings::defaults());
        let (status, _, body) = post_config(
            "platform=twitch&stream_key=livekey123",
            &fresh.ctrl,
            &fresh.settings,
            &fresh.cfg_path,
        )
        .await;
        assert_eq!(status, "200 OK", "{body}");
        assert!(
            fresh.settings.borrow().configured,
            "a usable destination raises it"
        );
    }

    // ── Overlay Studio CRUD ──────────────────────────────────────
    //
    // The Studio writes a baked overlay to overlays_dir/<slug>.html and
    // reads it back through the same list endpoint. These pin the slug
    // guard (no crafted name escapes the directory), the save/list/delete
    // round-trip, and back-compat with hand-dropped (non-Studio) .html.

    fn settings_with_overlays_dir(dir: &std::path::Path) -> Arc<watch::Sender<Settings>> {
        let mut s = crate::config::Settings::defaults();
        s.overlays_dir = dir.to_path_buf();
        let (tx, _rx) = watch::channel(s);
        Arc::new(tx)
    }

    fn unique_tmp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("ic-overlay-test-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn multitrack_config_keeps_stream_key_token() {
        // The ingest url_template we hand OBS always keeps the {stream_key}
        // token; OBS fills it (with its Stream Key field, or the EB session
        // token). EB publishes are accepted via Controller::remember_eb_key, not
        // by embedding a key in this URL.
        let mut s = crate::config::Settings::defaults();
        s.ingest_port = 1935;
        s.ingest_key = "abc123def".into();
        let (tx, _rx) = watch::channel(s);
        let cfg = obs_multitrack_config_static("encoder=x264&tracks=1", "", &Arc::new(tx));
        assert!(cfg.contains("/live/{stream_key}"));
        // This route is public (OBS fetches it with no session), so the
        // ingest key it would otherwise protect must never ride along.
        assert!(
            !cfg.contains("abc123def"),
            "public config leaked the ingest key"
        );
    }

    #[test]
    fn own_configs_use_the_gpu_encoder_only_when_obs_has_it() {
        let amd_request = r#"{"capabilities":{"gpu":[{"vendor_id":4098}]}}"#;
        let with_amf = vec!["h264_texture_amf".to_string(), "obs_x264".to_string()];
        let without_amf = vec!["obs_x264".to_string()];
        assert_eq!(
            choose_encoder("", amd_request, Some(&with_amf)).family,
            "amd"
        );
        let nvidia_request = r#"{"capabilities":{"gpu":[{"vendor_id":4318}]}}"#;
        let obs_30_2 = vec!["jim_nvenc".to_string(), "obs_x264".to_string()];
        assert_eq!(
            choose_encoder("", nvidia_request, Some(&obs_30_2)).id,
            "jim_nvenc",
            "OBS 30.2 has only the old NVENC id"
        );
        assert_eq!(
            choose_encoder("encoder=nvenc", "", Some(&with_amf)).family,
            "x264",
            "an explicit encoder OBS doesn't list must not stop the stream"
        );
        assert_eq!(
            choose_encoder("", amd_request, Some(&without_amf)).family,
            "x264",
            "an AMD card without AMF in OBS must not get an encoder OBS can't create"
        );
        assert_eq!(choose_encoder("", amd_request, None).family, "x264");
        assert_eq!(choose_encoder("", "", Some(&with_amf)).family, "x264");
        assert_eq!(
            choose_encoder("encoder=qsv", amd_request, None).id,
            "obs_qsv11",
            "with no log to check, an explicit encoder is trusted"
        );
    }

    #[test]
    fn static_ladder_keeps_encoder_ids_and_bitrate_split() {
        let (tx, _rx) = watch::channel(crate::config::Settings::defaults());
        let cfg = obs_multitrack_config_static(
            "encoder=x264&tracks=3&bandwidth=10000",
            "",
            &Arc::new(tx),
        );
        assert_eq!(cfg.matches(r#""type":"obs_x264""#).count(), 3);
        assert!(
            cfg.contains(r#""bitrate":6000"#),
            "top rung keeps its 60% share"
        );
        assert!(
            !cfg.contains(r#""canvas_index":1"#),
            "the ladder never names a second canvas"
        );
    }

    #[test]
    fn all_ingest_endpoint_auths_collects_every_endpoint() {
        // Real Twitch shape: an RTMP and an RTMPS endpoint. OBS picks one by its
        // RTMPS preference, so the broker must remember both tokens. (Twitch here
        // returns the same token for both, but distinct tokens must also work.)
        let json = r#"{"meta":{"config_id":"x"},"ingest_endpoints":[
            {"protocol":"RTMP","url_template":"rtmp://a/app/{stream_key}","authentication":"tok_rtmp"},
            {"protocol":"RTMPS","url_template":"rtmps://a/app/{stream_key}","authentication":"tok_rtmps"}
        ],"encoder_configurations":[{"authentication":"NOT_AN_ENDPOINT_TOKEN"}]}"#;
        let auths = all_ingest_endpoint_auths(json);
        assert_eq!(auths, vec!["tok_rtmp".to_string(), "tok_rtmps".to_string()]);
        // The `authentication` in encoder_configurations is outside the array and
        // must not be scooped up.
        assert!(!auths.iter().any(|a| a.contains("NOT_AN_ENDPOINT")));

        // No endpoints / no tokens -> empty, and the proxy's twitch-key fallback
        // covers it.
        assert!(all_ingest_endpoint_auths(r#"{"meta":{}}"#).is_empty());
        assert!(all_ingest_endpoint_auths(
            r#"{"ingest_endpoints":[{"protocol":"RTMP","url_template":"rtmp://a/{stream_key}"}]}"#
        )
        .is_empty());
    }

    #[test]
    fn eb_request_authorized_enforces_ingest_key() {
        // OBS presents the user's Stream Key field as the request's
        // `authentication`. This is the ONLY place the ingest key is checkable
        // under proxy EB (the publish itself carries the Twitch token).
        let body = r#"{"schema_version":"2024-06-04","authentication":"secretkey","client":{}}"#;

        // No ingest key configured: auth off, anything allowed.
        assert!(eb_request_authorized(body, ""));

        // Correct key allowed; wrong key (the reported bug) refused.
        assert!(eb_request_authorized(body, "secretkey"));
        assert!(!eb_request_authorized(body, "wrongkey"));

        // A user-appended query on the key field is stripped before matching.
        let body_q = r#"{"authentication":"secretkey?bandwidthtest=true","client":{}}"#;
        assert!(eb_request_authorized(body_q, "secretkey"));

        // Missing authentication field with a key set: refused, not allowed.
        assert!(!eb_request_authorized(r#"{"client":{}}"#, "secretkey"));
    }

    #[test]
    fn valid_slug_accepts_safe_names_rejects_traversal() {
        assert!(valid_slug("my-overlay"));
        assert!(valid_slug("Tournament_2"));
        assert!(valid_slug("a"));
        assert!(!valid_slug(""));
        assert!(!valid_slug("../evil"));
        assert!(!valid_slug("a/b"));
        assert!(!valid_slug("a\\b"));
        assert!(!valid_slug("c:evil"));
        assert!(!valid_slug("dot.name")); // no extension/dots - we append .html
        assert!(!valid_slug("space name"));
        assert!(!valid_slug(&"x".repeat(65))); // over the 64-char cap
    }

    #[test]
    fn overlay_save_list_delete_round_trip() {
        let dir = unique_tmp_dir("crud");
        let settings = settings_with_overlays_dir(&dir);

        // A baked overlay carries the ic-doc marker + a <title>.
        let html = "<!doctype html><!--ic-doc:%7B%22name%22%3A%22Tourney%22%7D-->\
                    <html><head><title>Tourney</title></head><body>x</body></html>";
        let (status, _, _) = overlay_save("tourney", html, &settings);
        assert_eq!(status, "200 OK");
        assert!(dir.join("tourney.html").is_file());

        // It shows up in the list as a Studio overlay with the title name.
        let listed = list_overlays(&settings);
        assert!(listed.contains(r#""slug":"tourney""#));
        assert!(listed.contains(r#""name":"Tourney""#));
        assert!(listed.contains(r#""studio":true"#));

        // It serves verbatim from /overlay/<slug>.html.
        let (sstatus, sctype, sbody) = serve_overlay_file("tourney.html", &settings);
        assert_eq!(sstatus, "200 OK");
        assert_eq!(sctype, "text/html; charset=utf-8");
        assert!(sbody.contains("ic-doc:"));

        // Delete removes the file and drops it from the list.
        let (dstatus, _, _) = overlay_delete("tourney", &settings);
        assert_eq!(dstatus, "200 OK");
        assert!(!dir.join("tourney.html").exists());
        assert!(!list_overlays(&settings).contains(r#""slug":"tourney""#));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn overlay_save_rejects_bad_slug_and_writes_nothing() {
        let dir = unique_tmp_dir("badslug");
        let settings = settings_with_overlays_dir(&dir);
        let (status, _, body) = overlay_save("../escape", "x", &settings);
        assert_eq!(status, "400 Bad Request");
        assert!(body.contains("invalid overlay name"));
        // Nothing leaked outside the dir, and the dir stayed empty.
        let count = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
        assert_eq!(count, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_overlays_marks_handwritten_html_as_legacy() {
        let dir = unique_tmp_dir("legacy");
        std::fs::write(
            dir.join("classic.html"),
            "<!doctype html><html><head><title>Classic</title></head><body>hi</body></html>",
        )
        .unwrap();
        let settings = settings_with_overlays_dir(&dir);
        let listed = list_overlays(&settings);
        assert!(listed.contains(r#""slug":"classic""#));
        assert!(listed.contains(r#""name":"Classic""#));
        assert!(listed.contains(r#""studio":false"#));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wipe_keeps_legacy_and_list_reports_autohide() {
        let dir = unique_tmp_dir("wipe");
        // A Studio overlay (ic-doc marker) that bakes an auto-hide (data-ah-).
        std::fs::write(
            dir.join("studio.html"),
            "<!doctype html><!--ic-doc:%7B%7D--><html><head><title>S</title></head>\
             <body><div class=\"icw\" data-ah-active=\"4000\"></div></body></html>",
        )
        .unwrap();
        // A hand-written legacy file: no marker, no auto-hide.
        std::fs::write(
            dir.join("legacy.html"),
            "<!doctype html><html><head><title>L</title></head><body>hi</body></html>",
        )
        .unwrap();

        // The list reports studio + autohide per file.
        let settings = settings_with_overlays_dir(&dir);
        let listed = list_overlays(&settings);
        assert!(listed.contains(r#""slug":"studio","name":"S","studio":true,"autohide":true"#));
        assert!(listed.contains(r#""slug":"legacy","name":"L","studio":false,"autohide":false"#));

        // Restore-defaults wipes only the Studio (ic-doc) file; legacy stays.
        wipe_studio_overlays(&dir);
        assert!(!dir.join("studio.html").exists());
        assert!(dir.join("legacy.html").is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_title_pulls_first_title() {
        assert_eq!(
            extract_title("<html><head><title>Hello</title></head>"),
            Some("Hello".to_string())
        );
        assert_eq!(extract_title("<html><body>no title</body></html>"), None);
        assert_eq!(
            extract_title("<title>  spaced  </title>"),
            Some("spaced".to_string())
        );
    }

    // ── Live HTTP surface ────────────────────────────────────────────
    //
    // These drive the real `serve` (head parse, CSRF guard, auth gate,
    // router) over a loopback socket: the bugs they pin live in the glue
    // between those pieces, which no helper-level test reaches.

    /// Removes a test's temp directory when dropped. Kept as the last field of
    /// `Live` so it drops after the controller, whose ring file lives inside
    /// and cannot be deleted on Windows while it is open.
    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Everything `serve` and the handlers need, confined to a private temp
    /// directory. Holds a settings receiver because `watch::Sender::send` is a
    /// silent no-op without one: a handler would "save" nothing and every
    /// assertion about the live settings would pass without testing anything.
    struct Live {
        ctrl: Arc<Controller>,
        settings: Arc<watch::Sender<Settings>>,
        _rx: watch::Receiver<Settings>,
        cfg_path: PathBuf,
        auth: Arc<crate::auth::AuthState>,
        _dir: TempDir,
    }

    impl Live {
        fn new(mut s: Settings) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("ic-live-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            // Nothing a request does may land outside the temp directory.
            s.overlays_dir = dir.join("overlays");
            s.buffer_path = dir.join("ring.buf");
            let ring = crate::buffer::DiskRing::create(&dir.join("ctrl.buf"), 4 * 1024 * 1024)
                .expect("ring create");
            let (tx, rx) = watch::channel(s);
            Live {
                ctrl: Arc::new(Controller::new(Arc::new(ring), 0)),
                settings: Arc::new(tx),
                _rx: rx,
                cfg_path: dir.join("instantclone.cfg"),
                auth: Arc::new(crate::auth::AuthState::new()),
                _dir: TempDir(dir),
            }
        }

        async fn send(&self, raw: &[u8]) -> Response {
            self.send_from("127.0.0.1", raw).await
        }

        /// One request through the real `serve`, as if from `peer_ip`. The
        /// write half is closed after the request, so a body shorter than its
        /// Content-Length reaches EOF instead of waiting out the body deadline.
        async fn send_from(&self, peer_ip: &str, raw: &[u8]) -> Response {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            let mut client = TcpStream::connect(addr).await.expect("connect");
            let (sock, _) = listener.accept().await.expect("accept");
            let server = tokio::spawn(serve(
                sock,
                self.ctrl.clone(),
                self.settings.clone(),
                self.cfg_path.clone(),
                Arc::new(SysStat::new()),
                self.auth.clone(),
                peer_ip.to_string(),
            ));
            client.write_all(raw).await.expect("write request");
            client.shutdown().await.expect("half-close");
            let mut out = Vec::new();
            // Windows can end a loopback connection with a reset instead of
            // a close once the whole response is out (seen under load). A
            // client reading by Content-Length never notices, and `parse`
            // below still fails on a response the reset cut short.
            match client.read_to_end(&mut out).await {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset && !out.is_empty() => {}
                Err(e) => {
                    let got = String::from_utf8_lossy(&out);
                    panic!(
                        "read response failed after {} bytes: {e:?}\n{got}",
                        out.len()
                    );
                }
            }
            server.await.expect("serve task").expect("serve");
            Response::parse(&out)
        }
    }

    struct Response {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl Response {
        /// Parse, asserting on the way that the bytes are a well-formed
        /// HTTP/1.1 response: CRLF line ends, no folded header lines, a status
        /// code, and a Content-Length that matches the body. Every live test
        /// gets this check for free.
        fn parse(raw: &[u8]) -> Self {
            let text = std::str::from_utf8(raw).expect("response is UTF-8");
            let (head, body) = text
                .split_once("\r\n\r\n")
                .unwrap_or_else(|| panic!("no CRLF blank line ends the head: {text:?}"));
            assert!(
                !head.replace("\r\n", "").contains(['\r', '\n']),
                "bare CR or LF in the head: {head:?}"
            );
            let mut lines = head.split("\r\n");
            let status_line = lines.next().unwrap_or_default();
            let status = status_line
                .strip_prefix("HTTP/1.1 ")
                .and_then(|s| s.get(..3))
                .and_then(|code| code.parse().ok())
                .unwrap_or_else(|| panic!("bad status line: {status_line:?}"));
            let mut headers = Vec::new();
            for line in lines {
                assert!(
                    !line.starts_with([' ', '\t']),
                    "folded (obsolete) header line: {line:?}"
                );
                let (name, value) = line
                    .split_once(':')
                    .unwrap_or_else(|| panic!("header without a colon: {line:?}"));
                headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
            }
            let response = Response {
                status,
                headers,
                body: body.to_string(),
            };
            let length: usize = response
                .header("content-length")
                .and_then(|v| v.parse().ok())
                .expect("a numeric Content-Length");
            assert_eq!(
                length,
                response.body.len(),
                "Content-Length must match the body"
            );
            response
        }

        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        }

        /// The `ic_session` token a Set-Cookie on this response hands out.
        fn session_cookie(&self) -> String {
            self.header("set-cookie")
                .and_then(|c| c.split(';').next())
                .and_then(|c| c.strip_prefix("ic_session="))
                .expect("a session cookie")
                .to_string()
        }
    }

    /// A request with a correct Content-Length. `headers` is zero or more
    /// complete `Name: value\r\n` lines.
    fn request(method: &str, path: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut raw = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{headers}Content-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        raw.extend_from_slice(body);
        raw
    }

    /// A request whose body length cannot be read gets a 400 that is itself a
    /// valid HTTP response. It used to be built from a multi-line raw string:
    /// bare LF line ends and an indented `Content-Length:` line, which reads as
    /// an obsolete folded header, so a client got garbage instead of the error.
    #[tokio::test]
    async fn a_malformed_content_length_gets_a_well_formed_400() {
        let live = Live::new(Settings::defaults());
        let r = live
            .send(b"POST /arm HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: abc\r\n\r\n")
            .await;
        assert_eq!(r.status, 400);
        assert!(r.body.contains("malformed Content-Length"), "{}", r.body);
    }

    /// A publisher connected and a 5 s delay armed: the state a bad request
    /// must leave alone.
    fn armed_live() -> Live {
        let live = Live::new(Settings::defaults());
        live.ctrl.mark_ingest_alive_for_test();
        live.ctrl.arm_delay(5_000);
        live
    }

    /// One byte that is not UTF-8 used to turn the whole body into "", and
    /// `POST /arm` with no `ms` is a disarm: a corrupt request dropped the
    /// streamer's delay. Same bug class as the Content-Length fix.
    #[tokio::test]
    async fn a_body_that_is_not_utf8_is_refused_not_read_as_empty() {
        let live = armed_live();
        let r = live
            .send(&request("POST", "/arm", "", b"ms=9000\xff"))
            .await;
        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(
            live.ctrl.armed_delay_ms(),
            5_000,
            "the armed delay must survive"
        );
    }

    /// A client that stops before the body it announced (EOF or the body
    /// deadline) must not have the fragment run: `ms=30000` cut after four
    /// bytes is `ms=3`, a 3 ms delay armed in place of a 30 s one.
    #[tokio::test]
    async fn a_body_shorter_than_its_content_length_is_refused_not_run_truncated() {
        let live = armed_live();
        let r = live
            .send(b"POST /arm HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 8\r\n\r\nms=3")
            .await;
        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(
            live.ctrl.armed_delay_ms(),
            5_000,
            "the armed delay must survive"
        );
    }

    /// Over 32 MB is refused from the head alone, before any body is read or
    /// buffered. Exactly 32 MB passes the size check, which the short-body
    /// refusal then shows without the test sending 32 MB.
    #[tokio::test]
    async fn an_oversized_body_is_refused_before_any_of_it_is_read() {
        const LIMIT: usize = 32 * 1024 * 1024;
        let live = Live::new(Settings::defaults());
        let head = |len: usize| {
            format!("POST /config HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {len}\r\n\r\n")
        };
        let over = live.send(head(LIMIT + 1).as_bytes()).await;
        assert_eq!(over.status, 413, "{}", over.body);
        let at_limit = live.send(head(LIMIT).as_bytes()).await;
        assert_eq!(
            at_limit.status, 400,
            "the limit itself is not too large: {}",
            at_limit.body
        );
    }

    /// A head that fills the 16 KB buffer without ending is answered with a
    /// 400, not a silently dropped connection.
    #[tokio::test]
    async fn a_request_head_larger_than_the_buffer_is_refused() {
        let live = Live::new(Settings::defaults());
        let mut raw = b"GET /state HTTP/1.1\r\nX-Pad: ".to_vec();
        raw.resize(16 * 1024, b'a');
        let r = live.send(&raw).await;
        assert_eq!(r.status, 400);
    }

    // ── Route access: the regression net ────────────────────────────
    //
    // With a dashboard password set, every route is Public, Control (session
    // or the dock token) or Admin (session only), and anything unlisted in
    // `classify_access` falls to Admin. The net below reads this file's own
    // source, so a new route fails the build's tests until someone writes
    // down which level it should have.

    /// Routes answered before the access gate runs. They skip auth entirely,
    /// so this list is exact: the login page and the login/logout actions.
    const BEFORE_GATE: &[(&str, &str)] =
        &[("GET", "/login"), ("POST", "/login"), ("POST", "/logout")];

    /// Every exact route and the level it must have once a password is set.
    const ROUTE_ACCESS: &[(&str, &str, Access)] = &[
        // Fast paths in `serve`.
        ("GET", "/", Access::Admin),
        ("GET", "/dock", Access::Control),
        ("GET", "/dock.js", Access::Control),
        ("GET", "/overlay-runtime.js", Access::Admin),
        ("GET", "/integrations.js", Access::Admin),
        ("GET", "/obs/vod-script/download", Access::Admin),
        ("GET", "/events", Access::Control),
        ("GET", "/overlay-events", Access::Public),
        // Renders arbitrary settings from the query: a settings page tool.
        ("GET", "/crash-protection/preview", Access::Admin),
        ("POST", "/app/restart", Access::Admin),
        ("POST", "/app/quit", Access::Admin),
        // Auth management in `auth_gate`, after the gate.
        ("POST", "/auth/set-password", Access::Admin),
        ("POST", "/auth/disable", Access::Admin),
        ("POST", "/auth/regen-dock", Access::Admin),
        // The router.
        ("GET", "/overlay", Access::Public),
        ("GET", "/state", Access::Control),
        ("GET", "/overlay-state", Access::Public),
        // Ending a hold is operational, like a cut: the dock has the button.
        ("POST", "/crash-protection/end", Access::Control),
        ("GET", "/config", Access::Control),
        ("GET", "/docks", Access::Control),
        ("GET", "/platforms", Access::Control),
        ("POST", "/obs/multitrack-config", Access::Public),
        ("GET", "/obs/multitrack-config", Access::Public),
        ("GET", "/obs/register-status", Access::Admin),
        ("POST", "/obs/register", Access::Admin),
        ("POST", "/obs/unregister", Access::Admin),
        ("POST", "/obs/launch-with-eb", Access::Admin),
        ("POST", "/obs/setup-vod-eb", Access::Admin),
        ("POST", "/shortcut/create-eb", Access::Admin),
        ("GET", "/update-check", Access::Admin),
        ("POST", "/update/apply", Access::Admin),
        ("POST", "/reveal/buffer", Access::Admin),
        ("POST", "/reveal/overlays", Access::Admin),
        ("POST", "/reveal/trace", Access::Admin),
        ("GET", "/obs/launch-status", Access::Admin),
        ("GET", "/twitch_ingests", Access::Admin),
        ("GET", "/profiles", Access::Control),
        ("GET", "/logs", Access::Admin),
        ("GET", "/overlays", Access::Control),
        ("GET", "/destinations", Access::Control),
        ("POST", "/config", Access::Admin),
        ("POST", "/config/reset", Access::Admin),
        ("POST", "/arm", Access::Control),
        ("POST", "/activate", Access::Control),
        ("POST", "/stop", Access::Control),
        ("POST", "/disarm", Access::Control),
        ("POST", "/delay", Access::Control),
        ("POST", "/go-live", Access::Control),
        ("POST", "/cut-after", Access::Control),
        ("POST", "/cut-after/cancel", Access::Control),
        ("POST", "/hotkeys/capture", Access::Admin),
        ("POST", "/midi/learn", Access::Admin),
        ("POST", "/midi/learn/cancel", Access::Admin),
        ("POST", "/midi/poll", Access::Admin),
        ("POST", "/test-egress", Access::Admin),
        ("POST", "/logs/clear", Access::Admin),
        ("POST", "/profiles", Access::Admin),
        ("POST", "/profiles/delete", Access::Admin),
        ("POST", "/destinations", Access::Admin),
        ("POST", "/destinations/toggle", Access::Control),
        ("POST", "/destinations/delete", Access::Admin),
    ];

    /// Prefix routes in the router, checked with a sample path beneath each.
    const PREFIX_ACCESS: &[(&str, &str, Access)] = &[
        ("GET", "/overlay/", Access::Public),
        ("POST", "/overlays/", Access::Admin),
        ("GET", "/docks/", Access::Control),
        ("POST", "/docks/", Access::Control),
        // Web call integrations: the token in the path is the credential.
        ("GET", "/hooks/", Access::Public),
        ("POST", "/hooks/", Access::Public),
        // The integrations API (see `integrations::api`): settings and
        // secrets, so a session only.
        ("GET", "/integrations", Access::Admin),
        ("POST", "/integrations", Access::Admin),
        ("POST", "/connections/", Access::Admin),
        ("POST", "/twitch/", Access::Admin),
    ];

    /// The production half of this file, so the tables above are never
    /// mistaken for routes.
    fn production_source() -> &'static str {
        let src = include_str!("web.rs");
        &src[..src.find("mod tests {").expect("the test module")]
    }

    /// The body of `fn <name>(`, up to the first line that is a lone `}`.
    fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
        let start = src
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("fn {name} not found"));
        let rest = &src[start..];
        let end = rest
            .match_indices("\n}")
            .map(|(at, _)| at)
            .find(|at| matches!(rest[at + 2..].chars().next(), None | Some('\r' | '\n')))
            .unwrap_or_else(|| panic!("end of fn {name} not found"));
        &rest[..end]
    }

    /// The text of the string literal starting at `s` (just past its quote).
    fn literal(s: &str) -> &str {
        &s[..s.find('"').unwrap_or(0)]
    }

    /// `("METHOD", "/path")` match arms, for any upper-case method.
    fn match_arm_routes(body: &str) -> Vec<(String, String)> {
        let mut routes = Vec::new();
        for (at, _) in body.match_indices("(\"") {
            let rest = &body[at + 2..];
            let method = literal(rest);
            if method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) {
                continue;
            }
            let Some(after) = rest[method.len()..].strip_prefix("\", \"") else {
                continue;
            };
            let path = literal(after);
            if path.starts_with('/') {
                routes.push((method.to_string(), path.to_string()));
            }
        }
        routes
    }

    /// Paths compared with `bare_path` via `needle` (`bare_path == "`, or a
    /// prefix test), each with the method its statement checks, if any. The
    /// statement runs back to the previous `{`, `}` or `;`, so a condition
    /// rustfmt wraps over several lines is still read whole.
    fn bare_path_tests(body: &str, needle: &str) -> Vec<(Option<String>, String)> {
        let method_marker = "method == \"";
        let mut found = Vec::new();
        for (at, _) in body.match_indices(needle) {
            let path = literal(&body[at + needle.len()..]).to_string();
            let statement_start = body[..at].rfind(['{', '}', ';']).map_or(0, |i| i + 1);
            let statement = &body[statement_start..at];
            let method = statement
                .find(method_marker)
                .map(|m| literal(&statement[m + method_marker.len()..]).to_string());
            found.push((method, path));
        }
        found
    }

    /// `method == "M" && bare_path == "/p"` fast paths. A comparison with no
    /// method in its statement (`let restart = bare_path == ...`) is not a
    /// route of its own.
    fn guarded_routes(body: &str) -> Vec<(String, String)> {
        bare_path_tests(body, "bare_path == \"")
            .into_iter()
            .filter_map(|(method, path)| Some((method?, path)))
            .collect()
    }

    fn route_set<'a>(
        routes: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> std::collections::BTreeSet<(String, String)> {
        routes
            .into_iter()
            .map(|(m, p)| (m.to_string(), p.to_string()))
            .collect()
    }

    #[test]
    fn every_route_has_a_deliberate_access_level() {
        let src = production_source();
        let gate = fn_body(src, "auth_gate");
        let (before_gate, after_gate) =
            gate.split_at(gate.find("classify_access(").expect("the gate classifies"));

        let answered_before = route_set(
            guarded_routes(before_gate)
                .iter()
                .map(|(m, p)| (m.as_str(), p.as_str())),
        );
        assert_eq!(
            answered_before,
            route_set(BEFORE_GATE.iter().copied()),
            "a route answered before the access gate needs no login at all"
        );

        let mut found = route_set(
            match_arm_routes(fn_body(src, "route"))
                .iter()
                .map(|(m, p)| (m.as_str(), p.as_str())),
        );
        for body in [fn_body(src, "serve"), after_gate] {
            for (m, p) in guarded_routes(body) {
                found.insert((m, p));
            }
        }
        let table = route_set(ROUTE_ACCESS.iter().map(|(m, p, _)| (*m, *p)));
        for (m, p) in &found {
            assert!(
                table.contains(&(m.clone(), p.clone())),
                "unclassified route {m} {p}: add it to ROUTE_ACCESS with the access \
                 level it must have once a dashboard password is set"
            );
        }
        for (m, p) in &table {
            assert!(
                found.contains(&(m.clone(), p.clone())),
                "ROUTE_ACCESS lists {m} {p}, which no longer exists: remove the row"
            );
        }

        let route = fn_body(src, "route");
        let mut prefixes: Vec<String> = ["bare_path.starts_with(\"", "bare_path.strip_prefix(\""]
            .iter()
            .flat_map(|needle| bare_path_tests(route, needle))
            .map(|(_, p)| p)
            .collect();
        prefixes.sort();
        prefixes.dedup();
        let mut listed: Vec<String> = PREFIX_ACCESS
            .iter()
            .map(|(_, p, _)| p.to_string())
            .collect();
        listed.sort();
        listed.dedup();
        assert_eq!(
            prefixes, listed,
            "a prefix route was added or removed: update PREFIX_ACCESS"
        );

        for (m, p, want) in ROUTE_ACCESS {
            assert_eq!(&classify_access(m, p), want, "{m} {p}");
        }
        for (m, prefix, want) in PREFIX_ACCESS {
            assert_eq!(
                &classify_access(m, &format!("{prefix}sample")),
                want,
                "{m} {prefix}*"
            );
        }
        for p in [
            "/overlays/seeded",
            "/overlays/reset",
            "/overlays/sample/delete",
        ] {
            assert_eq!(classify_access("POST", p), Access::Admin, "POST {p}");
        }
        // Fail closed: whatever nobody listed needs a session.
        for m in ["GET", "POST", "PUT", "DELETE", "HEAD"] {
            assert_eq!(classify_access(m, "/not-a-route"), Access::Admin, "{m}");
        }
    }

    /// The overlay display is public because an OBS browser source cannot log
    /// in, and it only ever GETs. Public for any method would hand a future
    /// `POST /overlay/...` to anyone who can reach the port, so the exemption
    /// is GET only. The multitrack config is public for both methods on
    /// purpose: OBS POSTs it, and GET is the browser escape hatch.
    #[test]
    fn public_exemptions_cover_only_the_methods_their_callers_use() {
        assert_eq!(classify_access("GET", "/overlay"), Access::Public);
        assert_eq!(classify_access("GET", "/overlay/x.html"), Access::Public);
        assert_eq!(classify_access("POST", "/overlay"), Access::Admin);
        assert_eq!(classify_access("POST", "/overlay/x.html"), Access::Admin);
        assert_eq!(
            classify_access("GET", "/obs/multitrack-config"),
            Access::Public
        );
        assert_eq!(
            classify_access("POST", "/obs/multitrack-config"),
            Access::Public
        );
    }

    /// A valid stored hash for the password "password" (the published PBKDF2
    /// vector, one iteration), so a test can turn auth on without paying for
    /// 210 000 iterations of an unoptimized hash.
    const TEST_PASSWORD_HASH: &str = "pbkdf2-sha256$1$73616c74$\
        120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b";
    const TEST_DOCK_TOKEN: &str = "dock0token0under0test";

    /// Auth on, with a known dock token.
    fn locked_live() -> Live {
        let mut s = Settings::defaults();
        s.dashboard_password_hash = TEST_PASSWORD_HASH.into();
        s.dock_token = TEST_DOCK_TOKEN.into();
        Live::new(s)
    }

    fn session_header(token: &str) -> String {
        format!("Cookie: ic_session={token}\r\n")
    }

    /// The dock token opens Control routes and nothing more. Only routes that
    /// are harmless even if the gate broke are sent for real: a regression
    /// here must fail the test, not register with the developer's OBS or open
    /// a file browser. The rest of the Admin list is pinned by the
    /// classification table above.
    #[tokio::test]
    async fn the_dock_token_cannot_reach_admin_routes() {
        let live = locked_live();
        let by_query = |path: &str| {
            let sep = if path.contains('?') { '&' } else { '?' };
            format!("{path}{sep}token={TEST_DOCK_TOKEN}")
        };
        let by_cookie = format!("Cookie: ic_dock={TEST_DOCK_TOKEN}\r\n");

        // The token itself is good: it opens a Control route both ways.
        let ok = live
            .send(&request("GET", &by_query("/state"), "", b""))
            .await;
        assert_eq!(ok.status, 200, "{}", ok.body);
        let ok = live.send(&request("GET", "/state", &by_cookie, b"")).await;
        assert_eq!(ok.status, 200, "{}", ok.body);

        for (method, path) in [
            ("GET", "/"),
            ("GET", "/logs"),
            ("POST", "/config"),
            ("POST", "/config/reset"),
            ("POST", "/auth/set-password"),
            ("POST", "/auth/disable"),
            ("POST", "/auth/regen-dock"),
            ("POST", "/app/quit"),
            ("POST", "/app/restart"),
            ("POST", "/destinations"),
            ("POST", "/destinations/delete"),
            ("POST", "/logs/clear"),
            ("POST", "/profiles"),
            ("POST", "/overlays/probe"),
        ] {
            let r = live.send(&request(method, &by_query(path), "", b"")).await;
            assert_eq!(r.status, 401, "{method} {path} via ?token=");
            let r = live.send(&request(method, path, &by_cookie, b"")).await;
            assert_eq!(r.status, 401, "{method} {path} via the ic_dock cookie");
        }
        let s = live.settings.borrow();
        assert_eq!(
            s.dashboard_password_hash, TEST_PASSWORD_HASH,
            "auth untouched"
        );
        assert_eq!(s.dock_token, TEST_DOCK_TOKEN, "dock token untouched");
    }

    /// An empty stored dock token means "no dock access", never "an empty
    /// token matches": `?token=` must not open anything.
    #[tokio::test]
    async fn an_empty_dock_token_grants_nothing() {
        let mut s = Settings::defaults();
        s.dashboard_password_hash = TEST_PASSWORD_HASH.into();
        s.dock_token.clear();
        let live = Live::new(s);
        for (path, headers) in [
            ("/state?token=", ""),
            ("/state?token=anything", ""),
            ("/state", "Cookie: ic_dock=\r\n"),
        ] {
            let r = live.send(&request("GET", path, headers, b"")).await;
            assert_eq!(r.status, 401, "{path} {headers:?}");
        }
    }

    /// Changing the password ends every earlier session, so a stolen cookie
    /// dies with the old password; whoever changed it gets a fresh one.
    #[tokio::test]
    async fn changing_the_password_ends_every_old_session() {
        let live = locked_live();
        let old = live.auth.create_session();
        let r = live
            .send(&request("GET", "/logs", &session_header(&old), b""))
            .await;
        assert_eq!(r.status, 200, "precondition: the session is admin");

        let r = live
            .send(&request(
                "POST",
                "/auth/set-password",
                &session_header(&old),
                b"password=a-new-password",
            ))
            .await;
        assert_eq!(r.status, 200, "{}", r.body);
        let fresh = r.session_cookie();

        let r = live
            .send(&request("GET", "/logs", &session_header(&old), b""))
            .await;
        assert_eq!(r.status, 401, "the old session must be dead");
        let r = live
            .send(&request("GET", "/logs", &session_header(&fresh), b""))
            .await;
        assert_eq!(r.status, 200, "the new session works");
        assert_ne!(
            live.settings.borrow().dashboard_password_hash,
            TEST_PASSWORD_HASH
        );
    }

    /// Turning auth off clears the dock token too, in memory and on disk, so
    /// switching auth back on later cannot revive an old token.
    #[tokio::test]
    async fn disabling_auth_clears_the_dock_token() {
        let live = locked_live();
        let session = live.auth.create_session();
        let r = live
            .send(&request(
                "POST",
                "/auth/disable",
                &session_header(&session),
                b"",
            ))
            .await;
        assert_eq!(r.status, 200, "{}", r.body);
        for s in [
            live.settings.borrow().clone(),
            Settings::load(&live.cfg_path).expect("saved"),
        ] {
            assert!(s.dashboard_password_hash.is_empty());
            assert!(s.dock_token.is_empty());
        }
    }

    /// Rotating the dock token retires the old one at once.
    #[tokio::test]
    async fn regenerating_the_dock_token_retires_the_old_one() {
        let live = locked_live();
        let session = live.auth.create_session();
        let r = live
            .send(&request(
                "POST",
                "/auth/regen-dock",
                &session_header(&session),
                b"",
            ))
            .await;
        assert_eq!(r.status, 200, "{}", r.body);
        let fresh = read_string_field(&r.body, "dock_token").expect("the new token");
        assert!(!fresh.is_empty() && fresh != TEST_DOCK_TOKEN);

        let old = live
            .send(&request(
                "GET",
                &format!("/state?token={TEST_DOCK_TOKEN}"),
                "",
                b"",
            ))
            .await;
        assert_eq!(old.status, 401, "the old token must stop working");
        let new = live
            .send(&request("GET", &format!("/state?token={fresh}"), "", b""))
            .await;
        assert_eq!(new.status, 200, "{}", new.body);
    }

    /// The first password is the one admin action reachable without a
    /// session, so it is local-only: a LAN peer on a bind-all box must not be
    /// able to claim the dashboard before its owner does. An IPv4-mapped IPv6
    /// loopback is not recognised as local and is refused too (fail closed).
    #[tokio::test]
    async fn the_first_password_must_be_set_from_this_machine() {
        let live = Live::new(Settings::defaults());
        for peer in ["192.168.1.50", "::ffff:127.0.0.1"] {
            let r = live
                .send_from(
                    peer,
                    &request("POST", "/auth/set-password", "", b"password=long-enough-pw"),
                )
                .await;
            assert_eq!(r.status, 403, "from {peer}: {}", r.body);
            assert!(live.settings.borrow().dashboard_password_hash.is_empty());
        }
    }

    #[test]
    fn is_loopback_fails_closed() {
        for ip in ["127.0.0.1", "127.8.9.10", "::1"] {
            assert!(is_loopback(ip), "{ip}");
        }
        for ip in [
            "::ffff:127.0.0.1",
            "192.168.1.50",
            "0.0.0.0",
            "",
            "localhost",
            "garbage",
        ] {
            assert!(!is_loopback(ip), "{ip}");
        }
    }

    /// A custom URL can carry the stream key in its path, so `/destinations`
    /// shows it raw only to a full session (the edit form needs it). The dock
    /// token reads this route too, and gets it blank plus a "set" flag.
    #[tokio::test]
    async fn the_dock_token_cannot_read_a_custom_egress_url() {
        const SECRET: &str = "SECRETKEY1234567890";
        let mut s = Settings::defaults();
        s.dashboard_password_hash = TEST_PASSWORD_HASH.into();
        s.dock_token = TEST_DOCK_TOKEN.into();
        s.destinations.push(crate::config::Destination {
            platform: "custom".into(),
            stream_key: String::new(),
            custom_egress_url: format!("rtmp://host/app/{SECRET}"),
            ..twitch_dest(true, "")
        });
        let live = Live::new(s);

        let dock = live
            .send(&request(
                "GET",
                &format!("/destinations?token={TEST_DOCK_TOKEN}"),
                "",
                b"",
            ))
            .await;
        assert_eq!(dock.status, 200, "{}", dock.body);
        assert!(
            !dock.body.contains(SECRET),
            "the dock read the key: {}",
            dock.body
        );
        assert!(
            dock.body.contains(r#""custom_egress_url":"""#),
            "{}",
            dock.body
        );
        assert!(
            dock.body.contains(r#""custom_egress_url_set":true"#),
            "{}",
            dock.body
        );

        let session = live.auth.create_session();
        let admin = live
            .send(&request(
                "GET",
                "/destinations",
                &session_header(&session),
                b"",
            ))
            .await;
        assert_eq!(admin.status, 200, "{}", admin.body);
        assert!(
            admin.body.contains(&format!(
                r#""custom_egress_url":"rtmp://host/app/{SECRET}""#
            )),
            "the dashboard's edit form needs the raw URL: {}",
            admin.body
        );
        assert!(admin.body.contains(r#""custom_egress_url_set":true"#));
    }

    // ── OBS multitrack config ────────────────────────────────────────

    /// A usable Twitch destination with id `id` and stream key `key`.
    fn twitch_dest_with(id: &str, key: &str) -> crate::config::Destination {
        crate::config::Destination {
            id: id.into(),
            name: format!("Twitch {id}"),
            ..twitch_dest(true, key)
        }
    }

    /// During a crash-protection hold the kept config carries the live
    /// Twitch session's token. A wrong ingest key must not get it, nor move
    /// the destination off that session; the right key gets it back as is.
    #[tokio::test]
    async fn a_wrong_ingest_key_never_gets_the_config_a_hold_keeps() {
        let mut s = settings_with_dests(vec![twitch_dest_with("a", "live_real_key_123")]);
        s.ingest_key = "right-key".into();
        let live = Live::new(s);
        let ctrl = &live.ctrl;
        ctrl.update_crash_protection(crate::crash_protection::CrashProtection {
            enabled: true,
            ..Default::default()
        });
        let dest = ctrl.destination_state("a");
        dest.egress_alive
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let held_ivs = "rtmps://ivs/app/held-token";
        *dest.eb_override_url.lock() = Some(held_ivs.into());
        ctrl.remember_eb_session(crate::controller::EbSession {
            config: "{held-config}".into(),
            auths: vec!["held-token".into()],
            dest_id: "a".into(),
            ivs_url: held_ivs.into(),
        });
        ctrl.begin_publish("right-key", "127.0.0.1").await.unwrap();
        ctrl.on_tag(9, 0, &[0x17, 0, 0, 0, 0, 1], false, true);
        ctrl.mark_ingest_dead();
        assert!(ctrl.hold_active());

        let wrong = r#"{"authentication":"wrong-key","client":{}}"#;
        let cfg = obs_multitrack_config_proxy(wrong, "", ctrl, &live.settings).await;
        assert!(!cfg.contains("{held-config}"), "{cfg}");
        assert_eq!(dest.eb_override_url.lock().as_deref(), Some(held_ivs));

        let right = r#"{"authentication":"right-key","client":{}}"#;
        let cfg = obs_multitrack_config_proxy(right, "", ctrl, &live.settings).await;
        assert_eq!(cfg, "{held-config}");
    }

    /// The ingest key is the only thing standing between a stranger's OBS and
    /// a brokered Twitch session. A wrong key gets the static config and
    /// nothing else: no Twitch key, no session, no egress override.
    #[tokio::test]
    async fn a_wrong_ingest_key_brokers_nothing() {
        let mut s = settings_with_dests(vec![twitch_dest_with("a", "live_real_key_123")]);
        s.ingest_key = "right-key".into();
        let live = Live::new(s);
        let body = r#"{"authentication":"wrong-key","client":{}}"#;
        let query = "encoder=x264&tracks=1";
        let cfg = obs_multitrack_config_proxy(body, query, &live.ctrl, &live.settings).await;
        assert_eq!(
            cfg,
            obs_multitrack_config_static(query, body, &live.settings)
        );
        assert!(!cfg.contains("live_real_key_123") && !cfg.contains("right-key"));
        assert!(live
            .ctrl
            .destination_state("a")
            .eb_override_url
            .lock()
            .is_none());
        assert!(logs_json(&live.ctrl).contains("rejected config request"));
    }

    /// With no usable Twitch destination there is nothing to broker: the
    /// static config comes back and the log says why. Asserting the log line,
    /// not just the output, matters: were a bad destination ever picked, the
    /// Twitch call would fail and fall back to the same static config.
    #[tokio::test]
    async fn no_usable_twitch_destination_falls_back_to_the_static_config() {
        let youtube = crate::config::Destination {
            platform: "youtube".into(),
            ..twitch_dest(true, "yt_key")
        };
        for (dests, why) in [
            (vec![], "no destinations"),
            (vec![twitch_dest(false, "livekey123")], "disabled"),
            (vec![twitch_dest(true, "")], "no stream key"),
            (vec![youtube], "not Twitch"),
        ] {
            let live = Live::new(settings_with_dests(dests));
            let body = r#"{"authentication":"anything"}"#;
            let cfg = obs_multitrack_config_proxy(body, "", &live.ctrl, &live.settings).await;
            assert_eq!(
                cfg,
                obs_multitrack_config_static("", body, &live.settings),
                "{why}"
            );
            assert!(
                logs_json(&live.ctrl).contains("no Twitch destination, and OBS didn't describe"),
                "{why}"
            );
        }
    }

    /// OBS's body without an `authentication` field cannot be patched with
    /// the real key, so the proxy falls back rather than send Twitch a body
    /// it did not mean to.
    #[tokio::test]
    async fn a_body_without_authentication_falls_back_to_the_static_config() {
        let live = Live::new(settings_with_dests(vec![twitch_dest_with("a", "live_key")]));
        let body = r#"{"client":{}}"#;
        let cfg = obs_multitrack_config_proxy(body, "", &live.ctrl, &live.settings).await;
        assert_eq!(cfg, obs_multitrack_config_static("", body, &live.settings));
        assert!(logs_json(&live.ctrl).contains("didn't expose an authentication"));
    }

    /// A real-shaped Twitch answer: OBS is pointed at us on every endpoint,
    /// the Twitch destination whose key was sent gets the session's IVS URL
    /// with the session token, and a stale override on another Twitch
    /// destination is cleared (two egresses on one session token collide).
    #[tokio::test]
    async fn a_twitch_session_is_applied_to_exactly_the_matching_destination() {
        let live = Live::new(settings_with_dests(vec![
            twitch_dest_with("a", "key_a"),
            twitch_dest_with("b", "key_b"),
        ]));
        *live.ctrl.destination_state("b").eb_override_url.lock() = Some("rtmps://stale".into());
        let twitch = r#"{"meta":{"config_id":"x"},"ingest_endpoints":[
            {"protocol":"RTMP","url_template":"rtmp://jfk.contribute.live-video.net/app/{stream_key}","authentication":"v1_session"},
            {"protocol":"RTMPS","url_template":"rtmps://jfk.contribute.live-video.net:443/app/{stream_key}","authentication":"v1_session"}
        ],"encoder_configurations":[]}"#;

        let out = apply_twitch_multitrack_config(twitch, "key_a", 1935, &live.ctrl, &live.settings);

        assert_eq!(out.matches("contribute.live-video.net").count(), 0, "{out}");
        assert_eq!(
            out.matches("rtmp://localhost:1935/live/{stream_key}")
                .count(),
            2
        );
        assert_eq!(
            *live.ctrl.destination_state("a").eb_override_url.lock(),
            Some("rtmp://jfk.contribute.live-video.net/app/v1_session".to_string())
        );
        assert!(live
            .ctrl
            .destination_state("b")
            .eb_override_url
            .lock()
            .is_none());
        assert!(logs_json(&live.ctrl).contains("2 enabled Twitch destinations"));
    }

    /// Endpoints without a session token (non-IVS multitrack): the user's own
    /// Twitch key fills the template instead.
    #[tokio::test]
    async fn endpoints_without_a_token_use_the_twitch_key() {
        let live = Live::new(settings_with_dests(vec![twitch_dest_with("a", "key_a")]));
        let twitch = r#"{"ingest_endpoints":[{"url_template":"rtmp://x.contribute.live-video.net/app/{stream_key}"}]}"#;
        apply_twitch_multitrack_config(twitch, "key_a", 1935, &live.ctrl, &live.settings);
        assert_eq!(
            *live.ctrl.destination_state("a").eb_override_url.lock(),
            Some("rtmp://x.contribute.live-video.net/app/key_a".to_string())
        );
    }

    /// A 200 with no ingest endpoints carries no session to switch to: no
    /// override is set and the response passes through unchanged.
    #[tokio::test]
    async fn a_response_without_endpoints_sets_no_override() {
        let live = Live::new(settings_with_dests(vec![twitch_dest_with("a", "key_a")]));
        let twitch = r#"{"meta":{"config_id":"x"},"encoder_configurations":[]}"#;
        let out = apply_twitch_multitrack_config(twitch, "key_a", 1935, &live.ctrl, &live.settings);
        assert_eq!(out, twitch);
        assert!(live
            .ctrl
            .destination_state("a")
            .eb_override_url
            .lock()
            .is_none());
        assert!(logs_json(&live.ctrl).contains("couldn't parse the ingest URL"));
    }

    // ── Overlay files ────────────────────────────────────────────────

    /// `/overlay/<name>` is public, so its name check is the first line of
    /// defence against reading files outside the overlays folder.
    #[test]
    fn overlay_names_that_could_leave_the_folder_are_refused() {
        let dir = unique_tmp_dir("names");
        let settings = settings_with_overlays_dir(&dir);
        for name in [
            "",
            "..",
            "../x",
            "..\\x",
            "a/b",
            "a\\b",
            "c:x",
            "C:\\Windows\\win.ini",
        ] {
            let (status, _, body) = serve_overlay_file(name, &settings);
            assert_eq!(status, "400 Bad Request", "{name:?}");
            assert_eq!(body, "invalid overlay name", "{name:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The second line of defence: a clean name that is a symlink to a file
    /// outside the folder must not be served. Creating a symlink on Windows
    /// needs Developer Mode or admin rights; without them the check is skipped
    /// with a printed note rather than silently passing.
    #[test]
    fn an_overlay_symlink_cannot_escape_the_folder() {
        let dir = unique_tmp_dir("symlink-in");
        let outside = unique_tmp_dir("symlink-out");
        let secret = outside.join("secret.txt");
        std::fs::write(&secret, "PRIVATE-MATERIAL").unwrap();
        let link = dir.join("escape.html");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&secret, &link);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&secret, &link);
        match made {
            Err(e) => eprintln!(
                "NOTE: skipped the overlay symlink-escape check: cannot create a symlink \
                 here ({e}). On Windows this needs Developer Mode or admin rights."
            ),
            Ok(()) => {
                let (status, _, body) =
                    serve_overlay_file("escape.html", &settings_with_overlays_dir(&dir));
                assert_eq!(status, "403 Forbidden", "{body}");
                assert!(!body.contains("PRIVATE"));
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
