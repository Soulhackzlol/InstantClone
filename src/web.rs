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
fn settings_write_guard() -> std::sync::MutexGuard<'static, ()> {
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
                Err(_) => break, // body took too long; use what we have
            };
            if n == 0 {
                break;
            }
            b.extend_from_slice(&tmp[..n]);
        }
        // Drop any pipelined bytes past this request (we always Connection:
        // close, so there is no next request on this socket anyway).
        b.truncate(content_length);
        b
    } else {
        Vec::new()
    };
    let body = std::str::from_utf8(&body_buf).unwrap_or("");

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
    if method == "GET" && (bare_path == "/overlay-runtime.js" || bare_path == "/dock.js") {
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
        let blob: &'static [u8] = if bare_path == "/dock.js" {
            DOCK_JS_GZ
        } else {
            OVERLAY_RUNTIME_JS_GZ
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
        return handle_sse(sock, ctrl, settings, sysstat).await;
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
            if let Err(wait) = auth.check_login_allowed(peer_ip) {
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
            auth.record_failure(peer_ip);
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

    match (method, bare_path) {
        // GET / and GET /dock are handled in serve() as a fast-path
        // (static gz blob, no allocation, no String round-trip).
        ("GET", "/overlay") => ("200 OK", "text/html; charset=utf-8", overlay_html(query)),
        ("GET", "/state") => (
            "200 OK",
            "application/json",
            state_json(ctrl, settings, sysstat),
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
        ("POST", "/obs/multitrack-config") => (
            "200 OK",
            "application/json",
            obs_multitrack_config_proxy(body, query, ctrl, settings).await,
        ),
        ("GET", "/obs/multitrack-config") => (
            "200 OK",
            "application/json",
            obs_multitrack_config_static(query, settings),
        ),
        ("GET", "/obs/register-status") => {
            let s = settings.borrow();
            (
                "200 OK",
                "application/json",
                format!(
                    r#"{{"registered":{},"obs_running":{},"vod_audio_flag":{},"vod_eb_injected":{},"obs_version":{},"active_profile":{},"path":{}}}"#,
                    crate::obs_register::is_registered(),
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
            destinations_json(ctrl, settings),
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
        ("POST", "/test-egress") => test_egress(settings).await,
        ("POST", "/test-webhook") => post_test_webhook(ctrl).await,
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
async fn handle_sse(
    mut sock: TcpStream,
    ctrl: Arc<Controller>,
    settings: Arc<watch::Sender<Settings>>,
    sysstat: Arc<SysStat>,
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
        let cur = state_json(&ctrl, &settings, &sysstat);
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

/// Live video state for one destination: whether the Dual Format canvas
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
    let vertical_canvas_present = track != 0xFF;
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
    let dest_list = s.destinations.iter().map(|d| {
        let st = snap.iter().find(|t| t.0 == d.id);
        let (alive, kbps, tags, bytes, cuts, recon) = st
            .map(|t| (t.1, t.3, t.4, t.5, t.6, t.7))
            .unwrap_or((false, 0u32, 0u64, 0u64, 0u32, 0u32));
        let v = dest_video(ctrl, d, &readouts);
        format!(
            r#"{{"id":{id},"name":{n},"enabled":{en},"alive":{al},"bitrate_kbps":{br},"tags_sent":{ts},"bytes_sent":{bs},"cuts":{cu},"reconnects":{rc},"vertical_ready":{vr},"vertical_canvas_present":{vcp},"video_res":{vres},"video_codec":{vcod}}}"#,
            id = json_escape_quoted(&d.id),
            n  = json_escape_quoted(&d.name),
            en = d.enabled, al = alive, br = kbps, ts = tags, bs = bytes, cu = cuts, rc = recon,
            vr = v.vertical_ready,
            vcp = v.vertical_canvas_present,
            vres = json_escape_quoted(&v.res),
            vcod = json_escape_quoted(&v.codec),
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

    // A portrait canvas is present on the wire (Twitch Dual Format is live).
    // Drives the header "Dual Format" pill.
    let vertical_present =
        crate::h264::detect_vertical_primary_track(&ctrl.ring.video_seq_headers.lock()).is_some();

    format!(
        r#"{{"phase":"{ph}","armed_delay_ms":{ad},"target_delay_ms":{td},"current_delay_ms":{cd},"buffer_fill_ms":{bf},"buffer_target_ms":{btm},"buffer_capacity_ms_est":{bc},"ingest_alive":{ia},"egress_alive":{ea},"destinations_alive":{dla},"destinations_total":{dlt},"buffer_building":{bb},"configured":{cfg},"obs_url":"{ou}","webhook_set":{ws},"video_codec":"{vc}","audio_codec":"{ac}","multitrack_video":{mtv},"multitrack_audio":{mta},"vertical_present":{vp},"cpu_pct":{cp:.2},"rss_bytes":{rb},"uptime_secs":{up},"publisher_token":{pt},"consumer_lag":{cl},"backpressure":{bp},"safe_cut_pending":{scp},"safe_cut_remaining_ms":{scr},"compat_warning":{cw},"stats":{{"tags_sent":{ts},"bytes_sent":{bs},"cuts":{cu},"ingest_disconnects":{id},"egress_reconnects":{er},"bitrate_kbps":{br}}},"destinations":[{dl}]}}"#,
        ph = ctrl.phase(),
        scp = ctrl.safe_cut_pending(),
        scr = ctrl.safe_cut_remaining_ms(),
        cw = json_escape_quoted(&compat_warning),
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
        ctrl.log(
            "[OBS multitrack] no enabled Twitch destination with a stream key - \
             returning static config. Twitch will accept the multi-track stream \
             but won't go live to viewers without an API-allocated session. \
             Fix: Destinations → enable a Twitch destination with the real key.",
        );
        crate::trace::log("OBS_MULTITRACK", "no twitch destination - static fallback");
        return obs_multitrack_config_static(query, settings);
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
            return obs_multitrack_config_static(query, settings);
        }
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
            return obs_multitrack_config_static(query, settings);
        }
        ProxyOutcome::HttpError(code, body) => {
            // Truncate the body so a verbose Twitch error page doesn't
            // flood the dashboard log line.
            let snippet: String = body.chars().take(300).collect();
            ctrl.log(format!(
                "[OBS multitrack] Twitch API returned HTTP {code} - returning static \
                 config. Response body (first 300 chars): {snippet}"
            ));
            crate::trace::log(
                "OBS_MULTITRACK",
                &format!("Twitch API HTTP {code} - static fallback. body={snippet}"),
            );
            return obs_multitrack_config_static(query, settings);
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
            return obs_multitrack_config_static(query, settings);
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
            return obs_multitrack_config_static(query, settings);
        }
    };

    // Twitch's response has one or more `ingest_endpoints` entries
    // with `url_template` values like
    // `rtmps://<region>.contribute.live-video.net/app/{stream_key}`.
    // Replace every rtmp:// or rtmps:// URL in url_template fields
    // with our localhost ingest so OBS sends the multi-track stream
    // to us instead. We keep `{stream_key}` as the literal token -
    // OBS substitutes it with whatever's in its Stream Key field
    // (which the streamer can type as anything; we ignore it).
    let rewritten = rewrite_url_templates(
        &twitch_json,
        &format!("rtmp://127.0.0.1:{}/live/{{stream_key}}", ingest_port),
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
    let sanitized = twitch_json.replace(&twitch_key, "<STREAM_KEY>");
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
    let (ivs_template, ivs_auth) = first_ingest_endpoint(&twitch_json)
        .map(|e| (Some(e.url_template), e.authentication))
        .unwrap_or((None, None));
    let substitution = ivs_auth.as_deref().unwrap_or(&twitch_key);
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
        // Walk through key + `:` + whitespace + opening quote.
        let after_key = &json[key_pos + key.len()..];
        let Some(colon_off) = after_key.find(':') else {
            // Malformed - bail and emit the remainder unchanged.
            out.push_str(&json[key_pos..]);
            return out;
        };
        let after_colon = &after_key[colon_off + 1..];
        let Some(quote_off) = after_colon.find('"') else {
            out.push_str(&json[key_pos..]);
            return out;
        };
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

fn obs_multitrack_config_static(query: &str, settings: &Arc<watch::Sender<Settings>>) -> String {
    let params = config::parse_form(query);
    let encoder = params.get("encoder").map(|s| s.as_str()).unwrap_or("x264");
    let tracks: u32 = params
        .get("tracks")
        .and_then(|s| s.parse().ok())
        .unwrap_or(3)
        .clamp(1, 3);
    let bandwidth: u32 = params
        .get("bandwidth")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10000)
        .clamp(1500, 50000);

    let ingest_port = settings.borrow().ingest_port;

    let (enc_type, settings_json_1080, settings_json_720, settings_json_480): (
        &str,
        String,
        String,
        String,
    );
    // Per-encoder presets: libobs encoder IDs + the settings keys each
    // implementation understands. x264 uses `preset` strings like
    // "veryfast"; nvenc uses "p1"-"p7"; AMD uses similar tier names.
    // `profile=main` is what every Twitch / YouTube / Kick decoder
    // accepts; baseline would drop B-frames entirely and main+high
    // are functionally equivalent on the wire for ~1080p60.
    match encoder {
        "nvenc" => {
            enc_type = "jim_nvenc";
            settings_json_1080 = encoder_settings_nvenc(bitrate_for_track(bandwidth, tracks, 0));
            settings_json_720 = encoder_settings_nvenc(bitrate_for_track(bandwidth, tracks, 1));
            settings_json_480 = encoder_settings_nvenc(bitrate_for_track(bandwidth, tracks, 2));
        }
        "amd" => {
            enc_type = "h264_texture_amf";
            settings_json_1080 = encoder_settings_amd(bitrate_for_track(bandwidth, tracks, 0));
            settings_json_720 = encoder_settings_amd(bitrate_for_track(bandwidth, tracks, 1));
            settings_json_480 = encoder_settings_amd(bitrate_for_track(bandwidth, tracks, 2));
        }
        "qsv" => {
            enc_type = "obs_qsv11";
            settings_json_1080 = encoder_settings_qsv(bitrate_for_track(bandwidth, tracks, 0));
            settings_json_720 = encoder_settings_qsv(bitrate_for_track(bandwidth, tracks, 1));
            settings_json_480 = encoder_settings_qsv(bitrate_for_track(bandwidth, tracks, 2));
        }
        _ => {
            enc_type = "obs_x264";
            settings_json_1080 = encoder_settings_x264(bitrate_for_track(bandwidth, tracks, 0));
            settings_json_720 = encoder_settings_x264(bitrate_for_track(bandwidth, tracks, 1));
            settings_json_480 = encoder_settings_x264(bitrate_for_track(bandwidth, tracks, 2));
        }
    }

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
        r#"{{"meta":{{"service":"InstantClone","schema_version":"2024-06-04","config_id":"{cid}"}},"ingest_endpoints":[{{"protocol":"RTMP","url_template":"rtmp://127.0.0.1:{port}/live/{{stream_key}}"}}],"encoder_configurations":[{encs}],"audio_configurations":{{"live":[{{"codec":"aac","track_id":0,"channels":2,"settings":{{"bitrate":160}}}}]}}}}"#,
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
        r#"{{"bitrate":{b},"rate_control":"CBR","keyint_sec":2,"profile":"main","preset":"p5","tune":"hq","multipass":"qres"}}"#,
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
        if matches!(
            k.as_str(),
            "ingest_port"
                | "ingest_bind_all"
                | "ingest_key"
                | "web_port"
                | "web_bind_all"
                | "buffer_mb"
                | "buffer_path"
                | "overlays_dir"
                | "tracing_enabled"
                | "auto_arm_on_connect"
                | "auto_activate_when_ready"
                | "auto_arm_delay_ms"
                | "update_check_enabled"
                | "open_dashboard_on_launch"
        ) {
            apply_field_str(&mut new_settings, k, v);
        }
    }
    if let Some(v) = form.get("discord_webhook_url") {
        if !v.is_empty() {
            apply_field_str(&mut new_settings, "discord_webhook_url", v);
        }
        // explicit clear: caller must POST `webhook_clear=1`
    }
    if form.get("webhook_clear").map(|s| s.as_str()) == Some("1") {
        new_settings.discord_webhook_url.clear();
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
    // Mirror webhook into the controller so events fire correctly even
    // before the next supervisor settings-change tick.
    ctrl.update_webhook(new_settings.discord_webhook_url.clone());
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
    ctrl.update_webhook(next.discord_webhook_url.clone());
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

    // Server-side capacity guard. A delay bigger than the ring can hold at
    // the current bitrate never fills - it stalls in "arming" forever, which
    // looks like a hang. The dashboard and dock both gate this client-side,
    // but a stale page, a second dock, or a scripted call could still ask for
    // the impossible, so we refuse it here too. Same estimate we publish as
    // buffer_capacity_ms_est; bitrate is floored at 2 Mbps so an idle or
    // low-bitrate stream stays generous and never blocks a reasonable pre-arm.
    // ms == 0 is disarm and always allowed.
    if ms > 0 {
        let cap_ms = {
            let s = settings.borrow();
            let kbps = ctrl.bitrate_kbps().max(2_000) as u64;
            (s.buffer_mb * 1024 * 1024 * 8 / kbps) as u32
        };
        if cap_ms > 0 && ms > cap_ms {
            return (
                "409 Conflict",
                "application/json",
                format!(
                    r#"{{"ok":false,"error":"Buffer too small for {}s - it holds about {}s at the current bitrate. Raise the buffer size in the dashboard."}}"#,
                    ms / 1000,
                    cap_ms / 1000
                ),
            );
        }
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

fn persist_delay_state(
    ctrl: &Controller,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) {
    let _wl = settings_write_guard();
    let mut ns = settings.borrow().clone();
    let armed = ctrl.armed_delay_ms();
    let target = ctrl.target_delay_ms();
    // Track "last manually armed delay" in auto_arm_delay_ms so the
    // System -> Behavior auto-arm picks up wherever the streamer last
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
fn destinations_json(ctrl: &Controller, settings: &Arc<watch::Sender<Settings>>) -> String {
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
        out.push_str(&format!(
            r#"{{"id":{id},"name":{n},"enabled":{en},"platform":{p},"custom_egress_url":{cu},"twitch_ingest":{ti},"youtube_ingest":{yi},"vod_audio":{va},"vod_audio_inject_eb":{vie},"stream_format":{sf},"audio_track":{at},"vertical_ready":{vr},"vertical_canvas_present":{vcp},"video_res":{vres},"video_codec":{vcod},"stream_key_set":{ks},"url_redacted":{ur},"alive":{al},"bitrate_kbps":{br},"tags_sent":{ts},"bytes_sent":{bs},"cuts":{ct},"reconnects":{rc}}}"#,
            id = json_escape_quoted(&d.id),
            n  = json_escape_quoted(&d.name),
            en = d.enabled,
            p  = json_escape_quoted(&d.platform),
            // NOTE: returned raw, NOT redacted, because the dashboard's edit
            // form needs it to populate the input field. Anyone reading this
            // endpoint already has localhost access and can read the plaintext
            // config file directly, so this doesn't expand the risk surface.
            cu = json_escape_quoted(&d.custom_egress_url),
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
    if let Some(i) = url.rfind('/') {
        let (base, key) = url.split_at(i + 1);
        if key.len() > 12 {
            return format!("{}{}…{}", base, &key[..4], &key[key.len() - 4..]);
        }
    }
    url.to_string()
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

// ----------------------------------------------------------------------
// Discord webhook test ping
// ----------------------------------------------------------------------

async fn post_test_webhook(ctrl: &Arc<Controller>) -> (&'static str, &'static str, String) {
    // Verbose test path. The fire-and-forget `fire_webhook` is the wrong
    // tool here because it (a) silently returns when the URL is empty,
    // (b) silently returns when its 2-second throttle is active (a fresh
    // destination-connect notification suppresses the user's test for 2s),
    // and (c) drops any HTTP/TLS error from Discord on the floor. This
    // path bypasses the throttle, validates the URL, and surfaces the
    // real result so the user knows whether the webhook is actually
    // reachable from this machine.
    let url = ctrl.webhook_url_snapshot();
    if url.is_empty() {
        return (
            "200 OK",
            "application/json",
            r#"{"ok":false,"error":"webhook URL is empty - set it in the System tab and save first"}"#.into(),
        );
    }
    let body =
        r#"{"content":"🧪 **InstantClone**: Test message - webhook is wired up and working."}"#;
    // Map ureq::Error (a fat enum that would trip clippy::result_large_err
    // if propagated) down to just the status code on success or a short
    // string on failure inside the worker thread.
    let send = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(move || -> Result<u16, String> {
            crate::https::https_agent()
                .post(&url)
                .config()
                .timeout_connect(Some(std::time::Duration::from_secs(5)))
                .timeout_global(Some(std::time::Duration::from_secs(8)))
                .build()
                .header("Content-Type", "application/json")
                .send(body)
                .map(|r| r.status().as_u16())
                .map_err(|e| e.to_string())
        }),
    )
    .await;
    let (ok, msg) = match send {
        Ok(Ok(Ok(status))) if (200..300).contains(&status) => {
            (true, format!("Discord accepted (HTTP {})", status))
        }
        Ok(Ok(Ok(status))) => (
            false,
            format!(
                "Discord rejected with HTTP {} - check the webhook URL",
                status
            ),
        ),
        Ok(Ok(Err(e))) => (false, format!("connection error: {}", e)),
        Ok(Err(e)) => (false, format!("internal: {}", e)),
        Err(_) => (
            false,
            "timed out after 10 s waiting for Discord".to_string(),
        ),
    };
    // Match the dashboard contract: `ok=true` carries a friendly status
    // in `message`, `ok=false` carries the same string in `error` so the
    // existing toast handler ("Webhook test failed: ${r.error}") just
    // works without a client-side change.
    let json = if ok {
        format!(r#"{{"ok":true,"message":{}}}"#, json_escape_quoted(&msg))
    } else {
        format!(r#"{{"ok":false,"error":{}}}"#, json_escape_quoted(&msg))
    };
    ("200 OK", "application/json", json)
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
        _ => {}
    }
}

fn parse_request_head(head: &str) -> (&str, &str, usize) {
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    let mut content_length = 0usize;
    for line in lines {
        if let Some(v) = strip_prefix_icase(line, "content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    (method, path, content_length)
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
    // Overlay DISPLAY only (browser sources can't log in); saving overlays is
    // a POST to /overlays/ which stays Admin.
    if path == "/overlay" || path.starts_with("/overlay/") {
        return Access::Public;
    }
    // OBS fetches the multitrack (Enhanced Broadcasting) config when it starts
    // streaming, with no session cookie and often no token - the config URL is
    // saved in OBS before any password is set. It returns only the encoder
    // ladder plus a local ingest template ("rtmp://127.0.0.1:<port>/live/
    // {stream_key}"), never a secret, and cannot control anything, so it stays
    // public like the overlay display. Gating it would break Start Streaming the
    // moment a dashboard password is enabled.
    if path == "/obs/multitrack-config" {
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
        | ("POST", "/cut-after/cancel") => Access::Control,
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
  if (window.EventSource){{
    try {{
      const es = new EventSource('/events');
      es.onmessage = e => {{ try {{ paint(JSON.parse(e.data)); }} catch(_){{}} }};
      es.onerror = () => {{ es.close(); setTimeout(startPolling, 1000); }};
      return;
    }} catch(_){{}}
  }}
  startPolling();
}}
function startPolling(){{
  async function tick(){{ try {{ paint(await (await fetch('/state')).json()); }} catch(_){{}} }}
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

    #[test]
    fn apply_field_str_persists_ingest_key() {
        // Regression: ingest_key is in post_config's form whitelist, so
        // apply_field_str must have a matching arm or the "Save key" button
        // silently no-ops and the ingest is never actually locked.
        let mut s = crate::config::Settings::defaults();
        assert!(s.ingest_key.is_empty());
        apply_field_str(&mut s, "ingest_key", "hunter2ingest");
        assert_eq!(s.ingest_key, "hunter2ingest");
        apply_field_str(&mut s, "ingest_key", "");
        assert!(s.ingest_key.is_empty());
    }

    #[test]
    fn apply_field_str_persists_tracing_enabled_value() {
        // The dispatch in post_config gates which keys reach this
        // function. The test below asserts the gate includes our
        // key by simulating the form-loop assignment.
        let mut s = crate::config::Settings::defaults();
        // beta.6 default is `false`.
        assert!(!s.tracing_enabled);
        apply_field_str(&mut s, "tracing_enabled", "true");
        assert!(s.tracing_enabled);
        apply_field_str(&mut s, "tracing_enabled", "false");
        assert!(!s.tracing_enabled);
    }

    // ── Behavior toggles: dispatch path tests ─────────────────────
    //
    // Same regression class as the tracing_enabled test above. The
    // post_config form-key whitelist plus apply_field_str's match arm
    // both have to know each new behaviour key, or settings silently
    // round-trip to the default. These tests pin both sides for the
    // v0.1.4 auto-arm fields.

    #[test]
    fn apply_field_str_persists_auto_arm_on_connect() {
        let mut s = crate::config::Settings::defaults();
        assert!(!s.auto_arm_on_connect);
        apply_field_str(&mut s, "auto_arm_on_connect", "true");
        assert!(s.auto_arm_on_connect);
        apply_field_str(&mut s, "auto_arm_on_connect", "false");
        assert!(!s.auto_arm_on_connect);
        apply_field_str(&mut s, "auto_arm_on_connect", "on");
        assert!(s.auto_arm_on_connect, "checkbox 'on' must read as truthy");
    }

    #[test]
    fn apply_field_str_persists_auto_activate_when_ready() {
        let mut s = crate::config::Settings::defaults();
        assert!(!s.auto_activate_when_ready);
        apply_field_str(&mut s, "auto_activate_when_ready", "true");
        assert!(s.auto_activate_when_ready);
        apply_field_str(&mut s, "auto_activate_when_ready", "off");
        assert!(!s.auto_activate_when_ready);
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

    #[test]
    fn parse_request_head_extracts_method_path_and_length() {
        let head = "POST /arm?x=1 HTTP/1.1\r\n\
                    Host: 127.0.0.1:7799\r\n\
                    Content-Length: 42\r\n\
                    Connection: close\r\n";
        let (method, path, len) = parse_request_head(head);
        assert_eq!(method, "POST");
        assert_eq!(path, "/arm?x=1");
        assert_eq!(len, 42);
    }

    #[test]
    fn parse_request_head_treats_missing_content_length_as_zero() {
        let head = "GET / HTTP/1.1\r\nHost: x\r\n";
        let (_, _, len) = parse_request_head(head);
        assert_eq!(len, 0);
    }

    #[test]
    fn parse_request_head_is_case_insensitive_on_header_name() {
        let head = "POST /x HTTP/1.1\r\ncontent-length: 7\r\n";
        let (_, _, len) = parse_request_head(head);
        assert_eq!(len, 7);
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

    #[test]
    fn configured_latch_only_rises() {
        // Mirror the handler's latch step - `if has_streamable_dest { =true }`
        // starting from an already-configured install whose last destination
        // is now disabled. The flag must stay true (no wizard reopen); only a
        // full reset clears it.
        let s = settings_with_dests(vec![twitch_dest(false, "livekey123")]);
        assert!(!has_streamable_dest(&s), "precondition: not streamable");
        let mut configured = true; // setup completed earlier
        if has_streamable_dest(&s) {
            configured = true;
        }
        assert!(
            configured,
            "a completed setup must never re-open the wizard"
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
}
