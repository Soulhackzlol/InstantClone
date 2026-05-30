//! Built-in web UI — first-run wizard, dashboard, settings, overlay, and
//! a small JSON state/config API. Hand-rolled HTTP/1.1 so we don't pull
//! hyper/axum into the RAM budget.
//!
//! Routes
//!     GET  /              — wizard (when !configured) or dashboard
//!     GET  /overlay       — OBS browser-source overlay
//!     GET  /state         — live JSON (delay, fill, alive, stats)
//!     GET  /config        — current settings (stream key NOT echoed)
//!     POST /config        — apply settings (form-encoded)
//!     POST /delay         — ms=NNN, sets target delay live
//!     POST /go-live       — convenience for delay=0
//!     POST /test-egress   — TCP-tests the configured platform endpoint
//!     GET  /platforms     — list of supported platforms (for UI dropdown)

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

pub async fn run(
    addr: String,
    ctrl: Arc<Controller>,
    settings: Arc<watch::Sender<Settings>>,
    cfg_path: PathBuf,
) -> io::Result<()> {
    let listener = TcpListener::bind(&addr).await?;
    eprintln!("[web] listening on http://{}", addr);
    // Single shared sampler: CPU% needs the previous sample to compute a
    // delta, so we cannot construct one per request.
    let sysstat = Arc::new(SysStat::new());
    loop {
        let (sock, _) = listener.accept().await?;
        let ctrl = ctrl.clone();
        let settings = settings.clone();
        let cfg_path = cfg_path.clone();
        let sysstat = sysstat.clone();
        tokio::spawn(async move {
            let _ = serve(sock, ctrl, settings, cfg_path, sysstat).await;
        });
    }
}

async fn serve(
    mut sock: TcpStream,
    ctrl: Arc<Controller>,
    settings: Arc<watch::Sender<Settings>>,
    cfg_path: PathBuf,
    sysstat: Arc<SysStat>,
) -> io::Result<()> {
    // Read until the headers terminator. For our POST bodies (config form,
    // <2 KB) this single read is enough — but be defensive about partials.
    let mut buf = vec![0u8; 16 * 1024];
    let mut used = 0usize;
    let head_end;
    loop {
        let n = sock.read(&mut buf[used..]).await?;
        if n == 0 {
            return Ok(());
        }
        used += n;
        if let Some(idx) = find_subslice(&buf[..used], b"\r\n\r\n") {
            head_end = idx + 4;
            break;
        }
        if used >= buf.len() {
            // Header section larger than our buffer — refuse politely
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

    // CSRF guard — block cross-origin browser POSTs. See `allow_csrf`
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
             Access-Control-Allow-Origin: *\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
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

    // Cap POST body size at 1 MB. Our largest legitimate payload is the
    // settings form (~2 KB). Without this, a local caller advertising a
    // 4 GB Content-Length would force us to allocate and read 4 GB before
    // refusing. The listener is 127.0.0.1-only so this is defense-in-depth.
    const MAX_BODY: usize = 1024 * 1024;
    if content_length > MAX_BODY {
        let body = br#"{"ok":false,"error":"body too large (max 1 MB)"}"# as &[u8];
        let r = format!(
            "HTTP/1.1 413 Payload Too Large\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        sock.write_all(r.as_bytes()).await?;
        sock.write_all(body).await?;
        return Ok(());
    }

    // For POST: ensure we have the full body.
    let mut body_buf = buf[head_end..used].to_vec();
    while body_buf.len() < content_length {
        let mut tmp = [0u8; 4096];
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        body_buf.extend_from_slice(&tmp[..n]);
    }
    if body_buf.len() > content_length {
        body_buf.truncate(content_length);
    }
    let body = std::str::from_utf8(&body_buf).unwrap_or("");

    let (status, ctype, payload) =
        route(method, path, body, &ctrl, &settings, &cfg_path, &sysstat).await;

    // ACAO is intentionally restrictive now — only set on GET responses
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

async fn route(
    method: &str,
    path: &str,
    body: &str,
    ctrl: &Arc<Controller>,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
    sysstat: &Arc<SysStat>,
) -> (&'static str, &'static str, String) {
    // Strip ?query — we only read it for /overlay.
    let (bare_path, query) = match path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path, ""),
    };
    // Pluggable overlays: any GET /overlay/<filename> reads from disk.
    if method == "GET" && bare_path.starts_with("/overlay/") {
        let name = &bare_path["/overlay/".len()..];
        return serve_overlay_file(name, settings);
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
        ("GET", "/config") => ("200 OK", "application/json", settings.borrow().to_json()),
        ("GET", "/platforms") => ("200 OK", "application/json", platforms_json()),
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
        // Two-phase delay endpoints
        ("POST", "/arm") => post_arm(body, ctrl, settings, cfg_path, sysstat).await,
        ("POST", "/activate") => post_activate(ctrl, settings, cfg_path, sysstat).await,
        ("POST", "/stop") => post_stop(ctrl, settings, cfg_path, sysstat).await,
        ("POST", "/disarm") => post_disarm(ctrl, settings, cfg_path, sysstat).await,
        // Legacy one-shot endpoints (Stream Deck etc.)
        ("POST", "/delay") => post_delay(body, ctrl, settings, cfg_path, sysstat).await,
        ("POST", "/go-live") => post_stop(ctrl, settings, cfg_path, sysstat).await,
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
        ("POST", "/destinations") => post_destination_upsert(body, settings, cfg_path).await,
        ("POST", "/destinations/delete") => post_destination_delete(body, settings, cfg_path).await,
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
/// Tick cadence is 250 ms — fine-grained enough for the bar/readout to
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
    // `Connection: keep-alive` is essential — our other routes use close.
    let headers = b"HTTP/1.1 200 OK\r\n\
                    Content-Type: text/event-stream\r\n\
                    Cache-Control: no-store\r\n\
                    Access-Control-Allow-Origin: *\r\n\
                    X-Accel-Buffering: no\r\n\
                    Connection: keep-alive\r\n\r\n";
    sock.write_all(headers).await?;
    // Initial retry hint — clients reconnect after 1 s if the socket drops.
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
                return Ok(()); // client gone — just exit, don't escalate
            }
            last_payload = cur;
            last_send = now;
        }
        tokio::time::sleep(tick).await;
    }
}

// ---- Endpoints ----

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

    // Per-destination summary array — joined from settings (the configured
    // list) with the controller's live runtime stats.
    let snap = ctrl.destination_snapshot();
    let dest_list = s.destinations.iter().map(|d| {
        let st = snap.iter().find(|t| t.0 == d.id);
        let (alive, kbps, tags, bytes, cuts, recon) = st
            .map(|t| (t.1, t.3, t.4, t.5, t.6, t.7))
            .unwrap_or((false, 0u32, 0u64, 0u64, 0u32, 0u32));
        format!(
            r#"{{"id":{id},"name":{n},"enabled":{en},"alive":{al},"bitrate_kbps":{br},"tags_sent":{ts},"bytes_sent":{bs},"cuts":{cu},"reconnects":{rc}}}"#,
            id = json_escape_quoted(&d.id),
            n  = json_escape_quoted(&d.name),
            en = d.enabled, al = alive, br = kbps, ts = tags, bs = bytes, cu = cuts, rc = recon,
        )
    }).collect::<Vec<_>>().join(",");

    format!(
        r#"{{"phase":"{ph}","armed_delay_ms":{ad},"target_delay_ms":{td},"current_delay_ms":{cd},"buffer_fill_ms":{bf},"buffer_target_ms":{btm},"buffer_capacity_ms_est":{bc},"ingest_alive":{ia},"egress_alive":{ea},"destinations_alive":{dla},"destinations_total":{dlt},"buffer_building":{bb},"configured":{cfg},"obs_url":"{ou}","webhook_set":{ws},"video_codec":"{vc}","audio_codec":"{ac}","multitrack_video":{mtv},"multitrack_audio":{mta},"cpu_pct":{cp:.2},"rss_bytes":{rb},"publisher_token":{pt},"consumer_lag":{cl},"backpressure":{bp},"stats":{{"tags_sent":{ts},"bytes_sent":{bs},"cuts":{cu},"ingest_disconnects":{id},"egress_reconnects":{er},"bitrate_kbps":{br}}},"destinations":[{dl}]}}"#,
        ph = ctrl.phase(),
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
        cp = cpu_pct,
        rb = rss_bytes,
        pt = ctrl.publisher_token(),
        cl = consumer_lag,
        bp = backpressure,
        dl = dest_list,
    )
}

fn platforms_json() -> String {
    let mut out = String::from("[");
    for (i, (slug, label)) in config::all_platforms().iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(r#"{{"slug":"{}","label":"{}"}}"#, slug, label));
    }
    out.push(']');
    out
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
    let mut new_settings = settings.borrow().clone();

    // Network + buffer + overlay-dir + webhook URL are applied directly.
    // EXCEPT the webhook: an empty submission means "keep the existing
    // value". The dashboard leaves the field blank for security (so the
    // server-side redacted value isn't shown to the user), so any empty
    // POST without an explicit "delete webhook" intent must be a no-op
    // for that field — otherwise saving any other setting would wipe it.
    for (k, v) in form.iter() {
        if matches!(
            k.as_str(),
            "ingest_port"
                | "ingest_bind_all"
                | "web_port"
                | "web_bind_all"
                | "buffer_mb"
                | "buffer_path"
                | "initial_delay_ms"
                | "overlays_dir"
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
    if new_settings
        .destinations
        .iter()
        .any(|d| d.enabled && d.is_well_formed())
    {
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
    // takes effect this instant — no need to wait for a restart.
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

/// Legacy one-shot delay endpoint — semantically the same as arming and
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
        // Force activate even if buffer hasn't built — controller will
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
    let ms: u32 = form.get("ms").and_then(|v| v.parse().ok()).unwrap_or(0);
    ctrl.arm_delay(ms.min(600_000));
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

fn persist_delay_state(
    ctrl: &Controller,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) {
    let mut ns = settings.borrow().clone();
    let armed = ctrl.armed_delay_ms();
    let target = ctrl.target_delay_ms();
    if ns.armed_delay_ms != armed || ns.target_delay_ms != target {
        ns.armed_delay_ms = armed;
        ns.target_delay_ms = target;
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
    let mut ns = settings.borrow().clone();
    ns.profiles.retain(|p| p.name != name);
    let _ = ns.save(cfg_path);
    let _ = settings.send(ns);
    ("200 OK", "application/json", profiles_json(settings))
}

// ---- Logs viewer ----

fn logs_json(ctrl: &Controller) -> String {
    let q = ctrl.logs.lock().unwrap();
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
    // full RTMP handshake — that would burn a "slot" on the platform.
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
//   id (optional)  — if present and matches existing, edit; else create new
//   name           — display label
//   enabled        — "on"/"off"
//   platform       — slug
//   stream_key     — empty string leaves existing untouched (security)
//   custom_egress_url
//
// POST /destinations/delete with `id=<id>` to remove.

async fn post_destination_upsert(
    body: &str,
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

    if name.trim().is_empty() {
        return (
            "400 Bad Request",
            "application/json",
            r#"{"ok":false,"error":"name required"}"#.into(),
        );
    }

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
        });
    }

    // Validate the new full state — return all errors so the UI can show
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
    if ns
        .destinations
        .iter()
        .any(|d| d.enabled && d.is_well_formed())
    {
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
    let _ = settings.send(ns);
    ("200 OK", "application/json", r#"{"ok":true}"#.into())
}

async fn post_destination_delete(
    body: &str,
    settings: &Arc<watch::Sender<Settings>>,
    cfg_path: &Path,
) -> (&'static str, &'static str, String) {
    let form = config::parse_form(body);
    let id = form.get("id").cloned().unwrap_or_default();
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
    if !ns
        .destinations
        .iter()
        .any(|d| d.enabled && d.is_well_formed())
    {
        ns.configured = false;
    }
    let _ = ns.save(cfg_path);
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
    let mut out = String::from("[");
    for (i, d) in s.destinations.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let url = d.egress_url().unwrap_or_default();
        let (_id, alive, _seq, kbps, tags, bytes, cuts, reconnects) = stats_for(&d.id);
        out.push_str(&format!(
            r#"{{"id":{id},"name":{n},"enabled":{en},"platform":{p},"custom_egress_url":{cu},"twitch_ingest":{ti},"youtube_ingest":{yi},"stream_key_set":{ks},"url_redacted":{ur},"alive":{al},"bitrate_kbps":{br},"tags_sent":{ts},"bytes_sent":{bs},"cuts":{ct},"reconnects":{rc}}}"#,
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

// ----------------------------------------------------------------------
// Pluggable overlays — files under settings.overlays_dir
// ----------------------------------------------------------------------

fn list_overlays(settings: &Arc<watch::Sender<Settings>>) -> String {
    let dir = settings.borrow().overlays_dir.clone();
    let mut names: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            if let Some(name) = e.file_name().to_str() {
                if name.ends_with(".html") || name.ends_with(".htm") {
                    names.push(name.to_string());
                }
            }
        }
    }
    names.sort();
    let mut out = String::from("[");
    for (i, n) in names.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&json_escape_quoted(n));
    }
    out.push(']');
    out
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
    // name-only check above can't catch that — the path string is clean,
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
            r#"{"ok":false,"error":"webhook URL is empty — set it in the System tab and save first"}"#.into(),
        );
    }
    let body =
        r#"{"content":"🧪 **InstantClone**: Test message — webhook is wired up and working."}"#;
    // Map ureq::Error (a fat enum that would trip clippy::result_large_err
    // if propagated) down to just the status code on success or a short
    // string on failure inside the worker thread.
    let send = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::task::spawn_blocking(move || -> Result<u16, String> {
            ureq::AgentBuilder::new()
                .timeout_connect(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(8))
                .build()
                .post(&url)
                .set("Content-Type", "application/json")
                .send_string(body)
                .map(|r| r.status())
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
                "Discord rejected with HTTP {} — check the webhook URL",
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
        "initial_delay_ms" => {
            if let Ok(v) = value.parse() {
                s.initial_delay_ms = v;
            }
        }
        "overlays_dir" => s.overlays_dir = std::path::PathBuf::from(value),
        "discord_webhook_url" => s.discord_webhook_url = value.into(),
        "tracing_enabled" => {
            // Form encoding: checkbox sends "true"/"false" or "on"/"" —
            // treat anything non-empty-non-false as truthy.
            let on = !matches!(value, "" | "false" | "0" | "off");
            s.tracing_enabled = on;
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
///     callers don't send Origin — we'd break legitimate use-cases by
///     rejecting these).
///   * POST WITH an Origin header: must match the Host header (i.e.
///     same-origin from the user's own dashboard). Cross-origin browser
///     POSTs (the actual CSRF surface) are blocked here — a tab on
///     evil.com `fetch('http://127.0.0.1:7799/stop', {method:'POST'})`
///     sends `Origin: https://evil.com`, which won't match Host.
///
/// This is the cheapest defense that closes the CSRF browser surface
/// without breaking headless API users. A token-based scheme would be
/// strictly stronger but requires UI plumbing — punt unless asked.
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
// HTML  —  one page, conditional setup / dashboard, all CSS+JS inline.
// ----------------------------------------------------------------------

/// Compact view for OBS browser-dock embedding. ~280x340 looks decent.
/// Reuses the same `/state` + `/arm` + `/activate` + `/stop` endpoints
/// as the main dashboard so behavior stays identical. Source lives in
/// `web/dock.html` — built-time minified + gzipped (see `build.rs`).
static DOCK_HTML_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/dock.html.gz"));

/// Main dashboard / first-run wizard. Source lives in `web/index.html`;
/// build-time minified + gzipped (see `build.rs`).
static INDEX_HTML_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/index.html.gz"));
/// Render the OBS browser-source overlay. Supports two query knobs:
///   ?lang=en|es|pt|fr|de                         — label localization
///   ?style=minimal|corner|strip|focus|broadcast|ticker  — visual variant
///
/// All six styles share the same DOM skeleton and `/state` polling
/// loop. The differences are spatial density + position, applied via a
/// `body.<style>` class hook. Three shared behaviours:
///   * 4 s idle auto-dim — overlay fades to ~22% opacity during
///     `idle`/`passthrough`, wakes back on the next phase transition.
///   * Phase-change halo — brief accent-glow bloom on any phase change.
///   * Tweened delay readout — the big number animates between values
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

    let (l_delay, l_live, l_preparing, l_ready, l_active, l_passthrough) = match lang {
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
/* ───── Shared design tokens — one palette, one type ramp, six layouts.
   Every style below dresses the same DOM skeleton; the differences are
   spatial density, not art direction. ───────────────────────────── */
:root{{
  --fg:rgba(255,255,255,.94);
  --muted:rgba(255,255,255,.55);
  --dim:rgba(255,255,255,.32);
  --accent:#5ac8fa;
  --accent-glow:rgba(90,200,250,.45);
  --good:#34c759;
  --warn:#ffae00;
  --bad:#ff453a;
  --surface:rgba(10,12,16,.62);
  --surface-strong:rgba(10,12,16,.86);
  --line:rgba(255,255,255,.08);
  --ease-out:cubic-bezier(.16,1,.3,1);
}}
*{{box-sizing:border-box}}
html,body{{margin:0;padding:0;background:transparent;color:var(--fg);
  font-family:-apple-system,Segoe UI,Roboto,Inter,sans-serif;
  font-feature-settings:"tnum" 1;width:100%;height:100%;overflow:hidden;
  -webkit-font-smoothing:antialiased}}

/* Dots — shared across styles. State transitions cross-fade smoothly
   rather than snap, so a phase change doesn't read as a flicker. */
.dot{{display:inline-block;width:8px;height:8px;border-radius:50%;
  background:var(--good);box-shadow:0 0 8px var(--good);vertical-align:middle;
  transition:background .22s ease,box-shadow .22s ease,opacity .22s ease}}
.dot.bad{{background:var(--bad);box-shadow:0 0 8px var(--bad)}}
.dot.warn{{background:var(--warn);box-shadow:0 0 8px var(--warn)}}
.dot.cool{{background:var(--accent);box-shadow:0 0 8px var(--accent)}}
.dot.pulse{{animation:dotPulse 1.6s ease-in-out infinite}}
@keyframes dotPulse{{0%,100%{{opacity:1}}50%{{opacity:.45}}}}

/* Shared entrance — soft blur+fade so the overlay arrives rather than
   pops. Tuned to feel "noticed" without being draw-attention. */
.box{{animation:boxIn .42s var(--ease-out) both;will-change:transform,opacity,filter}}
@keyframes boxIn{{from{{opacity:0;filter:blur(8px)}}to{{opacity:1;filter:blur(0)}}}}

/* Sneaky behaviour: 4 s after we enter idle/passthrough, dim the box so
   the viewer's eye stops snagging on it. Phase transition wakes it. */
body.idle-dim .box{{opacity:.22}}
.box{{transition:opacity .55s ease,box-shadow .42s ease}}

/* Phase-change halo — accent glow briefly blooms around the box on any
   phase transition. Helps the streamer (and the viewer) catch the
   moment a delay arms / activates / cuts. */
body.phase-flash .box{{box-shadow:0 0 0 1px var(--accent-glow),
  0 0 32px 6px var(--accent-glow)}}

/* ── minimal: top-left whisper, the new flagship ─────────── */
body.minimal .box{{position:fixed;left:20px;top:20px;
  background:var(--surface);
  backdrop-filter:blur(14px);-webkit-backdrop-filter:blur(14px);
  padding:10px 16px;border-radius:14px;
  border:1px solid var(--line);min-width:160px}}
body.minimal .l{{font-size:10.5px;text-transform:uppercase;
  letter-spacing:1.6px;color:var(--muted);font-weight:600}}
body.minimal .v{{font-size:30px;font-weight:700;letter-spacing:-1px;
  line-height:1.08;margin-top:1px}}
body.minimal .u{{font-size:15px;color:var(--muted);font-weight:400;margin-left:2px}}
body.minimal .row{{display:flex;gap:13px;align-items:center;margin-top:7px;
  font-size:11.5px;color:var(--muted)}}
body.minimal .row .dot{{margin-right:6px;width:7px;height:7px}}

/* ── corner: bottom-right default — a thin accent line as brand ── */
body.corner .box{{position:fixed;right:28px;bottom:28px;
  background:var(--surface-strong);
  backdrop-filter:blur(16px);-webkit-backdrop-filter:blur(16px);
  padding:16px 22px;border-radius:14px;
  border:1px solid var(--line);min-width:200px;text-align:right}}
body.corner .box::before{{content:"";position:absolute;left:18px;right:18px;top:0;
  height:1px;background:linear-gradient(90deg,transparent,var(--accent),transparent);
  opacity:.65}}
body.corner .l{{font-size:11.5px;text-transform:uppercase;
  letter-spacing:1.8px;color:var(--accent);font-weight:600;
  text-shadow:0 0 14px var(--accent-glow)}}
body.corner .v{{font-size:44px;font-weight:800;letter-spacing:-2px;
  margin-top:3px;line-height:1}}
body.corner .u{{font-size:20px;color:var(--muted);font-weight:400;margin-left:3px}}
body.corner .row{{display:flex;gap:14px;justify-content:flex-end;
  margin-top:10px;font-size:12px;color:var(--muted)}}
body.corner .row .dot{{margin-right:6px}}

/* ── strip: bottom-edge bar that rises in from below ─────── */
body.strip .box{{position:fixed;left:0;right:0;bottom:0;
  background:linear-gradient(180deg,transparent 0%,
    rgba(10,12,16,.0) 20%,rgba(10,12,16,.86) 100%);
  padding:18px 32px 16px;display:flex;align-items:flex-end;gap:28px;
  animation:stripIn .55s var(--ease-out) both}}
@keyframes stripIn{{from{{transform:translateY(20px);opacity:0;filter:blur(8px)}}
  to{{transform:translateY(0);opacity:1;filter:blur(0)}}}}
body.strip .box::before{{content:"";position:absolute;left:0;right:0;bottom:0;
  height:1px;background:linear-gradient(90deg,transparent 5%,
    var(--accent) 50%,transparent 95%);opacity:.4}}
body.strip .group{{display:flex;flex-direction:column;align-items:flex-start}}
body.strip .l{{font-size:10.5px;text-transform:uppercase;letter-spacing:1.8px;
  color:var(--muted);font-weight:600;margin-bottom:1px}}
body.strip .v{{font-size:34px;font-weight:700;letter-spacing:-1px;line-height:1}}
body.strip .u{{font-size:18px;color:var(--muted);font-weight:400;margin-left:2px}}
body.strip .row{{display:flex;gap:16px;margin-left:auto;
  font-size:12.5px;color:var(--muted);align-self:center}}
body.strip .row .dot{{margin-right:6px}}

/* ── focus: dead-centre modal for intermissions ──────────── */
body.focus{{display:flex;align-items:center;justify-content:center}}
body.focus .box{{background:rgba(0,0,0,.78);
  backdrop-filter:blur(18px);-webkit-backdrop-filter:blur(18px);
  padding:38px 60px;border-radius:24px;
  border:1px solid rgba(255,255,255,.10);text-align:center;
  box-shadow:0 30px 80px rgba(0,0,0,.55);
  animation:focusIn .5s var(--ease-out) both}}
@keyframes focusIn{{from{{transform:scale(.94);opacity:0;filter:blur(10px)}}
  to{{transform:scale(1);opacity:1;filter:blur(0)}}}}
body.focus .l{{font-size:12.5px;text-transform:uppercase;letter-spacing:3.5px;
  color:var(--muted);font-weight:600}}
body.focus .v{{font-size:88px;font-weight:800;letter-spacing:-3px;
  margin-top:8px;line-height:.95}}
body.focus .u{{font-size:34px;color:var(--muted);font-weight:400;margin-left:6px}}
body.focus .row{{display:flex;gap:18px;justify-content:center;
  margin-top:14px;font-size:13px;color:var(--muted)}}
body.focus .row .dot{{margin-right:6px}}

/* ── broadcast: TV news bar, sits at top, drops in ────────── */
body.broadcast .box{{position:fixed;left:0;right:0;top:0;height:46px;
  background:linear-gradient(180deg,#c81e1e,#a31616);color:#fff;
  padding:10px 22px;display:flex;align-items:center;gap:20px;
  box-shadow:0 2px 0 rgba(0,0,0,.45),
    inset 0 1px 0 rgba(255,255,255,.25),
    inset 0 -1px 0 rgba(0,0,0,.25);
  animation:bcastIn .45s var(--ease-out) both}}
@keyframes bcastIn{{from{{transform:translateY(-46px)}}to{{transform:translateY(0)}}}}
body.broadcast .l{{font-size:13px;text-transform:uppercase;letter-spacing:3px;
  font-weight:700;font-family:Georgia,'Times New Roman',serif}}
body.broadcast .v{{font-size:24px;font-weight:700;letter-spacing:-.4px}}
body.broadcast .u{{font-size:16px;opacity:.85;margin-left:1px}}
body.broadcast .row{{display:flex;gap:14px;margin-left:auto;
  font-size:11.5px;text-transform:uppercase;letter-spacing:2px;font-weight:600}}
body.broadcast .row .dot{{margin-right:5px;background:#fff;
  box-shadow:0 0 10px rgba(255,255,255,.65)}}
body.broadcast .row .dot.bad{{background:#1a1a1a;box-shadow:none;opacity:.6}}

/* ── ticker: ACTUALLY scrolling marquee, seamless loop ───── */
body.ticker .box{{position:fixed;left:0;right:0;bottom:0;height:38px;
  background:rgba(0,0,0,.88);display:flex;align-items:center;
  border-top:1px solid var(--accent);overflow:hidden}}
body.ticker .group,body.ticker .row{{display:none}}
body.ticker .ticker-track{{display:flex;flex-shrink:0;
  animation:tickerScroll 38s linear infinite;
  white-space:nowrap}}
@keyframes tickerScroll{{from{{transform:translateX(0)}}to{{transform:translateX(-50%)}}}}
body.ticker .ticker-cell{{display:inline-flex;align-items:center;gap:14px;
  padding:0 32px;font-size:13px;letter-spacing:.4px;flex-shrink:0}}
body.ticker .ticker-cell .l{{display:inline;text-transform:uppercase;
  letter-spacing:1.6px;font-weight:700;font-size:11px;color:var(--accent)}}
body.ticker .ticker-cell .v{{font-weight:700;letter-spacing:-.3px}}
body.ticker .ticker-cell .u{{opacity:.65;margin-left:1px}}
body.ticker .ticker-cell .sep{{opacity:.35}}
body.ticker .ticker-cell .dot{{margin-right:6px;width:6px;height:6px}}
</style></head><body class="{style}">
<div class="box">
  <div class="group">
    <div class="l" id="l">{l_delay}</div>
    <div class="v"><span id="v">0.0</span><span class="u">s</span></div>
  </div>
  <div class="row">
    <span><span class="dot" id="i"></span>OBS</span>
    <span><span class="dot" id="e"></span><span id="estatus">{l_live}</span></span>
  </div>
  <div class="ticker-track" id="ticker-track" aria-hidden="true"></div>
</div>
<script>
'use strict';
// Strings from the server, hand-localized in overlay_html().
const L = {{
  live:        "{l_live}",
  preparing:   "{l_preparing}",
  ready:       "{l_ready}",
  active:      "{l_active}",
  delay:       "{l_delay}",
  passthrough: "{l_passthrough}",
}};
// First body class is the style identifier — set server-side from the
// `style=` query param after the allowlist match.
const STYLE = document.body.className.split(/\s+/)[0];

// Hide sub-100 ms jitter so the consumer's "0.1s" doesn't flicker when
// effectively at the live edge with no delay armed.
function fmtDelay(secs){{
  if (!isFinite(secs) || secs < 0.05) return '0.0';
  return secs.toFixed(1);
}}

// Tween a number element from its currently-displayed value to `to`
// over `dur` ms using ease-out-cubic. Arming a 15 s delay then reads as
// the number building up rather than snapping. `format` does the
// stringification so callers don't have to.
const tweens = new WeakMap();
function tweenNumber(el, to, dur, format){{
  const prev = tweens.get(el);
  const from = prev ? prev.target : parseFloat(el.textContent) || 0;
  if (Math.abs(to - from) < 0.005){{ el.textContent = format(to); tweens.set(el,{{target:to}}); return; }}
  if (prev && prev.raf) cancelAnimationFrame(prev.raf);
  const start = performance.now();
  const rec = {{ target: to, raf: 0 }};
  function step(now){{
    const t = Math.min(1, (now - start) / dur);
    const eased = 1 - Math.pow(1 - t, 3);
    el.textContent = format(from + (to - from) * eased);
    if (t < 1) rec.raf = requestAnimationFrame(step);
  }}
  rec.raf = requestAnimationFrame(step);
  tweens.set(el, rec);
}}

// 4 s of idle/passthrough → fade the overlay down. Any non-idle phase
// resets the timer and wakes the box immediately.
let idleTimer = null;
function setIdleDim(idle){{
  if (idle){{
    if (!idleTimer && !document.body.classList.contains('idle-dim')){{
      idleTimer = setTimeout(() => {{
        document.body.classList.add('idle-dim');
        idleTimer = null;
      }}, 4000);
    }}
  }} else {{
    if (idleTimer){{ clearTimeout(idleTimer); idleTimer = null; }}
    document.body.classList.remove('idle-dim');
  }}
}}

// Brief accent halo on every phase transition. Skipped on the first
// /state result so we don't flash just because we went from null → idle.
let lastPhase = null;
function maybeFlashPhase(phase){{
  if (lastPhase !== null && lastPhase !== phase){{
    document.body.classList.add('phase-flash');
    setTimeout(() => document.body.classList.remove('phase-flash'), 480);
  }}
  lastPhase = phase;
}}

// Ticker track builder — two identical cells side by side, animated
// translateX(-50%) for a seamless wrap. Each refresh rewrites both cells
// with the current state; the CSS animation runs on the wrapper and is
// undisturbed by inner text changes.
function renderTicker(parts){{
  const track = document.getElementById('ticker-track');
  if (!track) return;
  const cellHtml = ''
    + '<span class="l">' + parts.label + '</span>'
    + '<span class="v">' + parts.valueText + '<span class="u">s</span></span>'
    + '<span class="sep">·</span>'
    + '<span><span class="dot ' + parts.iCls + '"></span>OBS</span>'
    + '<span class="sep">·</span>'
    + '<span><span class="dot ' + parts.eCls + '"></span>' + parts.statusText + '</span>';
  track.innerHTML =
    '<span class="ticker-cell">' + cellHtml + '</span>' +
    '<span class="ticker-cell">' + cellHtml + '</span>';
}}

async function refresh(){{
  let s;
  try {{ s = await (await fetch('/state')).json(); }} catch(_){{ return; }}

  // Resolve what to display this tick.
  let displayMs = 0, label = L.delay, status = L.live;
  if (!s.ingest_alive){{
    label = L.delay; status = '—'; displayMs = 0;
  }} else if (s.phase === 'idle'){{
    label = L.delay; status = L.passthrough; displayMs = 0;
  }} else if (s.phase === 'preparing'){{
    label = L.preparing; status = L.preparing; displayMs = s.buffer_fill_ms || 0;
  }} else if (s.phase === 'ready'){{
    label = L.ready; status = L.ready; displayMs = s.armed_delay_ms || 0;
  }} else {{ // active
    label = L.delay; status = L.active;
    displayMs = s.current_delay_ms || s.target_delay_ms || 0;
  }}
  const valueSecs = displayMs / 1000;

  // Dot classes — semantic: bad=disconnected, warn=preparing,
  // cool=armed-ready, pulse=live/active.
  const iCls = s.ingest_alive ? 'pulse' : 'bad';
  let eCls;
  if (!s.ingest_alive) eCls = 'bad';
  else if (s.phase === 'preparing') eCls = 'warn pulse';
  else if (s.phase === 'ready')     eCls = 'cool';
  else                              eCls = 'pulse';

  if (STYLE === 'ticker'){{
    renderTicker({{ label, valueText: fmtDelay(valueSecs),
      statusText: status, iCls, eCls }});
  }} else {{
    const vEl = document.getElementById('v');
    tweenNumber(vEl, valueSecs, 380, fmtDelay);
    document.getElementById('l').textContent = label;
    document.getElementById('estatus').textContent = status;
    document.getElementById('i').className = 'dot ' + iCls;
    document.getElementById('e').className = 'dot ' + eCls;
  }}

  setIdleDim(!s.ingest_alive || s.phase === 'idle');
  maybeFlashPhase(s.phase);
}}
refresh();
setInterval(refresh, 500);
</script></body></html>
"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // CLI tools and Stream Deck don't send Origin — must keep working.
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
}
