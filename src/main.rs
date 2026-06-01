// Release builds on Windows run without a console — the tray icon is
// the user-facing exit affordance. Debug builds keep the console so
// `cargo run` still prints log output.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! InstantClone entry point.
//!
//! Settings live in a single file (default `./instantclone.config.json`).
//! On first launch — when `configured=false` — we open the user's browser
//! at the setup wizard. The web UI is the configuration surface; almost
//! nothing comes from env vars.
//!
//! Three supervisors run forever:
//!
//! * `supervise_ingest` owns the RTMP listener task; restarts it when
//!   `ingest_port` / `ingest_bind_all` change.
//! * `supervise_egress` owns the outbound publisher; reconnects when
//!   platform/key change OR when the egress URL becomes valid for the
//!   first time.
//! * `supervise_web` owns the HTTP server; restarts on `web_port` change
//!   so the user can move it without a binary restart.
//!
//! Changing buffer_mb / buffer_path is the one thing that still requires a
//! full restart (the DiskRing is immutable once mapped). The UI shows a
//! sticky "restart required" banner when those are pending.

mod buffer;
mod config;
mod controller;
mod h264;
mod https;
mod obs_register;
#[cfg(windows)]
mod portcheck;
mod rtmp;
mod sink;
mod sysstat;
mod trace;
#[cfg(windows)]
mod tray;
mod web;

use crate::config::Settings;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

fn main() -> std::io::Result<()> {
    // Subcommand dispatch — keeps the proxy + sink in one binary so users
    // don't need two `cargo run` recipes to test end-to-end locally.
    let raw_args: Vec<String> = std::env::args().collect();
    if raw_args.get(1).map(String::as_str) == Some("sink") {
        // The release binary builds with `windows_subsystem = "windows"`
        // so double-clicking the proxy doesn't flash a console window.
        // That same flag silences the sink CLI: its `println!` calls
        // would write into a null stdout. Try to attach to the parent
        // console (e.g. the PowerShell that launched us); fall back to
        // allocating a fresh one when there is no parent console (the
        // user double-clicked the .exe). Skip both when stdout is
        // already a valid handle — that means a caller already piped or
        // redirected us, and stealing it would break their capture.
        #[cfg(all(windows, not(debug_assertions)))]
        unsafe {
            use windows_sys::Win32::System::Console::{
                AllocConsole, AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE,
            };
            let h = GetStdHandle(STD_OUTPUT_HANDLE);
            if h.is_null() && AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                AllocConsole();
            }
        }
        return sink::run_cli(&raw_args[2..]);
    }

    // `--no-browser` (or `INSTANTCLONE_NO_BROWSER=1`) suppresses the
    // first-paint browser pop, useful for autostart / headless setups.
    let suppress_browser = raw_args.iter().any(|a| a == "--no-browser")
        || std::env::var("INSTANTCLONE_NO_BROWSER").ok().as_deref() == Some("1");

    let cfg_path: PathBuf = std::env::var("CONFIG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./instantclone.config.json"));

    // Egress trace — wire-level append-only log next to the config.
    // Captures handshake details, sequence headers, every video tag,
    // and every cut so any "Twitch live looks broken but VOD is fine"
    // class of bug can be diagnosed by diffing the file against a known-
    // good capture. Opt-out via INSTANTCLONE_NO_TRACE=1.
    trace::init("./instantclone-trace.log");

    let mut settings = Settings::load_or_default(&cfg_path);
    // Honour the persisted tracing toggle from disk. init() defaults to
    // enabled; if the user disabled it last session, flip it off before
    // any code path starts writing.
    trace::set_enabled(settings.tracing_enabled);
    // If the file didn't exist, persist the smart defaults so the file
    // appears on disk immediately (useful for the user to find and edit).
    if !cfg_path.exists() {
        let _ = settings.save(&cfg_path);
    }

    // Port pre-flight (Windows only). Without this, a busy port leaves
    // us in a silent retry loop with no console to print to — the worst
    // possible first-run UX. If either port is taken we identify the
    // owning process, propose the next free port in the +0..+9 window,
    // and pop a native modal asking the user to switch or quit.
    #[cfg(windows)]
    {
        let host_ingest = if settings.ingest_bind_all {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        };
        if !portcheck::is_port_free(&format!("{}:{}", host_ingest, settings.ingest_port)) {
            match preflight_resolve("RTMP port", settings.ingest_port, host_ingest) {
                Some(new_port) => settings.ingest_port = new_port,
                None => return Ok(()),
            }
        }
        let host_web = if settings.web_bind_all {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        };
        if !portcheck::is_port_free(&format!("{}:{}", host_web, settings.web_port)) {
            match preflight_resolve("Web port", settings.web_port, host_web) {
                Some(new_port) => settings.web_port = new_port,
                None => return Ok(()),
            }
        }
        // Persist any chosen replacement port so the user doesn't get
        // re-prompted on every launch.
        let _ = settings.save(&cfg_path);
    }

    // Create overlays/ directory and write the three built-in templates if
    // any are missing. Users can edit them or drop in their own .html files.
    ensure_overlays_dir(&settings.overlays_dir);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(async move {
        let ring = Arc::new(buffer::DiskRing::create(
            &settings.buffer_path,
            settings.buffer_bytes(),
        )?);
        // The new model: cold-start *arms* a delay so the buffer rebuilds
        // toward what the user had before, but doesn't yank the viewer
        // back the moment the stream resumes. They explicitly hit
        // "Activate" once the buffer is ready (or auto-activate via the
        // config flag, if we add one later).
        let initial_armed = settings
            .armed_delay_ms
            .max(settings.target_delay_ms)
            .max(settings.initial_delay_ms);
        let ctrl = Arc::new(controller::Controller::new(ring, initial_armed));

        let (tx, rx) = watch::channel(settings.clone());
        let tx = Arc::new(tx);

        // (Banner removed: with windows_subsystem=windows there is no
        // attached console for the user to read it, and debug builds
        // already get per-supervisor log lines below.)

        // Auto-open the browser on every launch so the user always sees
        // the dashboard within a second of double-clicking the exe. The
        // tray icon stays running in the background — closing the tab
        // doesn't kill the proxy. `--no-browser` skips this for autostart
        // / headless setups.
        if !suppress_browser {
            let url = format!("http://127.0.0.1:{}/", settings.web_port);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                open_browser(&url);
            });
        }

        // System-tray icon (Windows): hidden message-only window in its
        // own OS thread, signals shutdown back here via a oneshot. The
        // tray gives us the exit affordance that the dropped console
        // window used to provide. The tray also reaches into the
        // controller for status + Cut delay, and into settings for the
        // RTMP URL clipboard action. On non-Windows the receiver just
        // sits pending forever (the matching `tray::spawn` is gated by cfg).
        let (_tray_tx, tray_rx) = tokio::sync::oneshot::channel::<()>();
        #[cfg(windows)]
        tray::spawn(rx.clone(), ctrl.clone(), _tray_tx);

        let ingest_sup = tokio::spawn(supervise_ingest(rx.clone(), ctrl.clone()));
        let egress_sup = tokio::spawn(supervise_egress(rx.clone(), ctrl.clone()));
        let web_sup = tokio::spawn(supervise_web(
            rx.clone(),
            ctrl.clone(),
            tx.clone(),
            cfg_path.clone(),
        ));

        let shutdown_reason: &str;
        tokio::select! {
            _ = ingest_sup => { shutdown_reason = "ingest supervisor exited"; }
            _ = egress_sup => { shutdown_reason = "egress supervisor exited"; }
            _ = web_sup    => { shutdown_reason = "web supervisor exited"; }
            _ = tokio::signal::ctrl_c() => { shutdown_reason = "ctrl-c"; }
            _ = tray_rx                 => { shutdown_reason = "tray quit"; }
        }
        eprintln!("\nShutting down ({shutdown_reason}) — closing active streams cleanly...");
        // Flush the egress trace so the last few thousand events make
        // it to disk before the BufWriter is dropped on process exit.
        trace::flush();
        // Flip shutdown on every active destination so each pump sends
        // `deleteStream` to its upstream before the runtime drops them.
        // Tiny window — if a pump is mid-await it'll just exit on next
        // loop tick.
        for (_id, st) in ctrl.all_destination_states() {
            st.shutdown_requested
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        tokio::time::sleep(Duration::from_millis(800)).await;

        // Clean up the pre-allocated ring file. Its contents are never
        // reused across runs (OBS reconnects → new publisher token, the
        // write cursor resets to 0 and overwrites everything), so leaving
        // a 300 MB file sitting on disk between sessions is just clutter.
        // We only remove it if it looks like our own ring file (matches
        // the configured `buffer_path` and the configured capacity).
        let cleanup_path = settings.buffer_path.clone();
        let cleanup_cap = settings.buffer_bytes();
        if let Ok(meta) = std::fs::metadata(&cleanup_path) {
            if meta.is_file() && meta.len() == cleanup_cap {
                let _ = std::fs::remove_file(&cleanup_path);
            }
        }
        Ok::<_, std::io::Error>(())
    })
}

async fn supervise_ingest(mut rx: watch::Receiver<Settings>, ctrl: Arc<controller::Controller>) {
    let mut current_addr = rx.borrow().ingest_addr();
    let mut handle = tokio::spawn(rtmp::server::run(current_addr.clone(), ctrl.clone()));
    loop {
        tokio::select! {
            r = &mut handle => {
                eprintln!("[ingest] task ended: {:?}", r);
                tokio::time::sleep(Duration::from_secs(1)).await;
                handle = tokio::spawn(rtmp::server::run(current_addr.clone(), ctrl.clone()));
            }
            ch = rx.changed() => {
                if ch.is_err() { return; }
                let new_addr = rx.borrow().ingest_addr();
                if new_addr != current_addr {
                    eprintln!("[ingest] hot-restart {} → {}", current_addr, new_addr);
                    ctrl.log(format!("ingest: rebinding {} → {}", current_addr, new_addr));
                    handle.abort();
                    let _ = (&mut handle).await;
                    current_addr = new_addr;
                    handle = tokio::spawn(rtmp::server::run(current_addr.clone(), ctrl.clone()));
                }
            }
        }
    }
}

/// One egress pump per destination. Owns a map { dest_id → (url, JoinHandle) }
/// and diffs it against the active-destinations list whenever settings
/// change. Adds/removes/restarts as needed.
async fn supervise_egress(mut rx: watch::Receiver<Settings>, ctrl: Arc<controller::Controller>) {
    // Currently-running egress pumps, indexed by Destination.id.
    let mut running: std::collections::HashMap<
        String,
        (String, tokio::task::JoinHandle<std::io::Result<()>>),
    > = std::collections::HashMap::new();

    // Mirror webhook URL into the controller on every settings change.
    let initial_webhook = { rx.borrow().discord_webhook_url.clone() };
    ctrl.update_webhook(initial_webhook);

    loop {
        // Snapshot the current desired destinations.
        let desired: Vec<(config::Destination, String)> = { rx.borrow().active_destinations() };

        // 1) Stop any pump whose dest is no longer desired (or whose URL changed).
        let desired_ids: std::collections::HashSet<String> =
            desired.iter().map(|(d, _)| d.id.clone()).collect();
        let to_remove: Vec<String> = running
            .keys()
            .filter(|id| !desired_ids.contains(*id))
            .cloned()
            .collect();
        for id in to_remove {
            if let Some((_url, handle)) = running.remove(&id) {
                // Cooperative shutdown: ask the pump to send deleteStream
                // and close cleanly. Give it a short window — then
                // HARD-ABORT no matter what. Without the explicit abort,
                // dropping the JoinHandle leaves the task running
                // detached: a Twitch destination stuck in a connect-fail
                // loop would keep retrying forever even after the user
                // toggled it off.
                let state = ctrl.destination_state(&id);
                state
                    .shutdown_requested
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                let abort = handle.abort_handle();
                let _ = tokio::time::timeout(Duration::from_millis(1500), handle).await;
                abort.abort();
                ctrl.remove_destination_state(&id);
                ctrl.log(format!("[{}] removed", id));
            }
        }

        // 2) For each desired dest: spawn if missing, or restart if URL
        //    changed, OR if the previous pump has finished (e.g. it
        //    bailed out cleanly when ingest went away — we want a fresh
        //    pump now that ingest is back).
        let ingest_alive = ctrl.ingest_alive();
        for (dest, url) in &desired {
            let needs_restart = match running.get(&dest.id) {
                Some((existing_url, handle)) => existing_url != url || handle.is_finished(),
                None => true,
            };
            if needs_restart {
                if let Some((_old_url, handle)) = running.remove(&dest.id) {
                    handle.abort();
                    let _ = handle.await;
                }
                // Don't open a fresh egress while OBS isn't sending —
                // we'd either burn TCP to Twitch / YouTube for an empty
                // publish slot, or sit blocked in next_or_wait. The
                // pump itself bails out cleanly on ingest loss; here we
                // just refuse to spawn its replacement until ingest is
                // back. The next supervisor tick (~2 s) re-checks.
                if !ingest_alive {
                    continue;
                }
                let state = ctrl.destination_state(&dest.id);
                // Defensive: a previous teardown may have left the
                // shutdown flag set on the (cached) Arc. Reset it before
                // the new pump enters its loop or it would immediately
                // send deleteStream and exit.
                state
                    .shutdown_requested
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                // Enhanced Broadcasting (multi-track video) pass-through
                // is currently a Twitch-only capability — Twitch's
                // ingest accepts the simulcast and uses it to populate
                // the transcoded quality ladder, bypassing the
                // account-tier source-only fallback. Every other RTMP
                // ingest we know of rejects or mishandles multi-track
                // video, so they get the same single-track flatten the
                // ingest used to apply globally.
                state.pass_through_multitrack_video.store(
                    dest.platform == "twitch",
                    std::sync::atomic::Ordering::Relaxed,
                );
                let label = dest.name.clone();
                let url_clone = url.clone();
                let handle = tokio::spawn(controller::run_egress(
                    ctrl.clone(),
                    label,
                    url_clone.clone(),
                    state,
                ));
                running.insert(dest.id.clone(), (url_clone, handle));
                ctrl.log(format!("[{}] starting egress", dest.name));
            }
        }

        if running.is_empty() {
            eprintln!("[egress] idle — add a destination in the web UI");
        }

        // Wake at most every 2 s to re-check ingest_alive, finished
        // pumps, and desired-set membership. Important: this MUST fire
        // even when `running` is empty, otherwise the supervisor blocks
        // forever after refusing to spawn while ingest was dead, and
        // OBS connecting wouldn't wake anything. (Before this change,
        // users had to toggle a destination off+on to get the supervisor
        // to look at the world again.) Settings changes still preempt
        // the wait via `rx.changed()` for instant response.
        let periodic_wake = tokio::time::sleep(Duration::from_secs(2));
        tokio::select! {
            ch = rx.changed() => {
                if ch.is_err() { return; }
                // Mirror webhook URL on every settings change.
                let new_webhook = { rx.borrow().discord_webhook_url.clone() };
                ctrl.update_webhook(new_webhook);
            }
            _ = periodic_wake => {
                // One or more pumps exited unexpectedly. Drop dead ones
                // so the diff loop above respawns them.
                running.retain(|_, (_, h)| !h.is_finished());
            }
        }
    }
}

async fn supervise_web(
    mut rx: watch::Receiver<Settings>,
    ctrl: Arc<controller::Controller>,
    tx: Arc<watch::Sender<Settings>>,
    cfg_path: PathBuf,
) {
    let mut current_addr = rx.borrow().web_addr();
    let spawn_one =
        |addr: String| tokio::spawn(web::run(addr, ctrl.clone(), tx.clone(), cfg_path.clone()));
    let mut handle = spawn_one(current_addr.clone());
    loop {
        tokio::select! {
            r = &mut handle => {
                eprintln!("[web] task ended: {:?}", r);
                tokio::time::sleep(Duration::from_secs(1)).await;
                handle = spawn_one(current_addr.clone());
            }
            ch = rx.changed() => {
                if ch.is_err() { return; }
                let new_addr = rx.borrow().web_addr();
                if new_addr != current_addr {
                    eprintln!("[web] hot-restart {} → {}", current_addr, new_addr);
                    handle.abort();
                    let _ = (&mut handle).await;
                    current_addr = new_addr;
                    handle = spawn_one(current_addr.clone());
                }
            }
        }
    }
}

/// Run the port-conflict dialog and return the user's choice.
/// Returns `Some(new_port)` if the user accepted a fallback,
/// or `None` if they chose to quit (or no free port was available).
#[cfg(windows)]
fn preflight_resolve(label: &str, port: u16, host: &str) -> Option<u16> {
    let owner = portcheck::find_process_on_port(port);
    let proposed = portcheck::find_free_port(host, port.saturating_add(1), 9);
    match portcheck::ask_user(label, port, owner, proposed) {
        portcheck::ConflictChoice::SwitchPort(p) => Some(p),
        portcheck::ConflictChoice::Quit => None,
    }
}

fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let _ = Command::new("cmd").args(["/c", "start", "", url]).spawn();
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open").arg(url).spawn();
}

/// Create the overlays directory (if missing) and write any of the three
/// built-in templates that don't already exist on disk. Idempotent — safe
/// to call on every startup. Users can freely edit the files; the next
/// run won't overwrite them.
fn ensure_overlays_dir(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    let drop = |name: &str, body: &str| {
        let path = dir.join(name);
        if !path.exists() {
            let _ = std::fs::write(&path, body);
        }
    };
    drop("minimal.html", OVERLAY_MINIMAL);
    drop("corner.html", OVERLAY_CORNER);
    drop("strip.html", OVERLAY_STRIP);
    drop("README.md", OVERLAY_README);
}

const OVERLAY_README: &str = r#"# InstantClone overlay plugins

Any `.html` file you drop into this folder becomes an OBS browser-source
overlay. The dashboard's *Overlay* tab lists them and gives you the URL
to paste into OBS.

Each overlay can fetch live state from the proxy:

    GET /state  →  JSON  {
      phase: "idle"|"preparing"|"ready"|"active",
      current_delay_ms, armed_delay_ms, buffer_fill_ms, ...
      ingest_alive, egress_alive,
      destinations_alive, destinations_total,
      destinations: [ { id, name, enabled, alive, bitrate_kbps, ... } ],
      stats: { bitrate_kbps, cuts, ingest_disconnects, ... }
    }

Recommended pattern:

    <script>
      async function tick(){
        try { const s = await (await fetch('/state')).json();
              document.getElementById('v').textContent = (s.current_delay_ms/1000).toFixed(1);
            } catch(_) {}
      }
      tick(); setInterval(tick, 500);
    </script>

The three bundled overlays (minimal, corner, strip) are useful starting
points — copy one and modify.
"#;

const OVERLAY_MINIMAL: &str = r##"<!doctype html>
<html><head><meta charset="utf-8"><title>Delay (minimal)</title>
<style>
body{margin:0;background:transparent;color:#fff;font-family:-apple-system,Segoe UI,Roboto,sans-serif}
.box{position:fixed;left:24px;top:24px;background:rgba(10,12,16,.65);
  backdrop-filter:blur(8px);padding:12px 18px;border-radius:12px;
  border:1px solid rgba(255,255,255,.08);min-width:180px}
.l{font-size:11px;text-transform:uppercase;letter-spacing:1.5px;color:rgba(255,255,255,.55)}
.v{font-size:32px;font-weight:700;letter-spacing:-1px;font-variant-numeric:tabular-nums;line-height:1.1;margin-top:2px}
.u{font-size:16px;color:rgba(255,255,255,.55);font-weight:400;margin-left:3px}
.dot{display:inline-block;width:8px;height:8px;border-radius:50%;background:#3c3;
  box-shadow:0 0 8px #3c3;margin-right:6px;vertical-align:middle}
.dot.bad{background:#f55;box-shadow:0 0 8px #f55}
.row{display:flex;gap:14px;margin-top:8px;font-size:12px;color:rgba(255,255,255,.7)}
</style></head><body>
<div class="box">
  <div class="l">Delay</div>
  <div class="v"><span id="v">0</span><span class="u">s</span></div>
  <div class="row"><span><span class="dot" id="i"></span>OBS</span>
    <span><span class="dot" id="e"></span>LIVE</span></div>
</div>
<script>
async function t(){try{const s=await(await fetch('/state')).json();
v.textContent=(s.current_delay_ms/1000).toFixed(1);
i.className='dot '+(s.ingest_alive?'':'bad');
e.className='dot '+(s.egress_alive?'':'bad');}catch(_){}}
t();setInterval(t,500);
</script></body></html>
"##;

const OVERLAY_CORNER: &str = r##"<!doctype html>
<html><head><meta charset="utf-8"><title>Delay (corner)</title>
<style>
body{margin:0;background:transparent;color:#fff;font-family:-apple-system,Segoe UI,Roboto,sans-serif}
.box{position:fixed;right:32px;bottom:32px;background:rgba(10,12,16,.85);
  padding:18px 26px;border-radius:16px;border:2px solid #6cf;min-width:220px;text-align:right}
.l{font-size:13px;text-transform:uppercase;letter-spacing:2px;color:#6cf}
.v{font-size:48px;font-weight:800;letter-spacing:-2px;font-variant-numeric:tabular-nums;margin-top:4px}
.u{font-size:22px;color:rgba(255,255,255,.6);font-weight:400;margin-left:4px}
.row{display:flex;gap:14px;justify-content:flex-end;margin-top:10px;font-size:13px;color:rgba(255,255,255,.75)}
.dot{display:inline-block;width:8px;height:8px;border-radius:50%;background:#3c3;
  box-shadow:0 0 8px #3c3;margin-right:6px;vertical-align:middle}
.dot.bad{background:#f55;box-shadow:0 0 8px #f55}
</style></head><body>
<div class="box">
  <div class="l">Stream Delay</div>
  <div class="v"><span id="v">0</span><span class="u">s</span></div>
  <div class="row"><span><span class="dot" id="i"></span>OBS</span>
    <span><span class="dot" id="e"></span>LIVE</span></div>
</div>
<script>
async function t(){try{const s=await(await fetch('/state')).json();
v.textContent=(s.current_delay_ms/1000).toFixed(1);
i.className='dot '+(s.ingest_alive?'':'bad');
e.className='dot '+(s.egress_alive?'':'bad');}catch(_){}}
t();setInterval(t,500);
</script></body></html>
"##;

const OVERLAY_STRIP: &str = r##"<!doctype html>
<html><head><meta charset="utf-8"><title>Delay (strip)</title>
<style>
body{margin:0;background:transparent;color:#fff;font-family:-apple-system,Segoe UI,Roboto,sans-serif}
.box{position:fixed;left:0;right:0;bottom:0;
  background:linear-gradient(180deg,transparent,rgba(10,12,16,.9));
  padding:18px 32px;display:flex;align-items:center;gap:32px;font-size:18px}
.l{font-size:12px;text-transform:uppercase;letter-spacing:2px;color:rgba(255,255,255,.55)}
.v{font-size:36px;font-weight:700;font-variant-numeric:tabular-nums;letter-spacing:-1px}
.u{font-size:18px;color:rgba(255,255,255,.55);font-weight:400;margin-left:3px}
.row{display:flex;gap:18px;margin-left:auto;font-size:14px;color:rgba(255,255,255,.75)}
.group{display:flex;flex-direction:column;align-items:flex-start}
.dot{display:inline-block;width:8px;height:8px;border-radius:50%;background:#3c3;
  box-shadow:0 0 8px #3c3;margin-right:6px;vertical-align:middle}
.dot.bad{background:#f55;box-shadow:0 0 8px #f55}
</style></head><body>
<div class="box">
  <div class="group"><span class="l">Delay</span>
    <span class="v"><span id="v">0</span><span class="u">s</span></span></div>
  <div class="row"><span><span class="dot" id="i"></span>OBS</span>
    <span><span class="dot" id="e"></span>LIVE</span></div>
</div>
<script>
async function t(){try{const s=await(await fetch('/state')).json();
v.textContent=(s.current_delay_ms/1000).toFixed(1);
i.className='dot '+(s.ingest_alive?'':'bad');
e.className='dot '+(s.egress_alive?'':'bad');}catch(_){}}
t();setInterval(t,500);
</script></body></html>
"##;
