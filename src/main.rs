// Release builds on Windows run without a console - the tray icon is
// the user-facing exit affordance. Debug builds keep the console so
// `cargo run` still prints log output.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! InstantClone entry point.
//!
//! Settings live in a single file (default `./instantclone.config.json`).
//! On first launch - when `configured=false` - we open the user's browser
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
mod self_update;
mod sha256;
mod sink;
mod sysstat;
mod trace;
#[cfg(windows)]
mod tray;
mod update_check;
mod web;

use crate::config::Settings;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

fn main() -> std::io::Result<()> {
    // Subcommand dispatch - keeps the proxy + sink in one binary so users
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
        // already a valid handle - that means a caller already piped or
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

    // `--launch-eb` cold-starts straight into VOD-audio + Enhanced
    // Broadcasting mode: once the web server is up we write OBS's
    // VOD-track unlock flag and launch OBS with the `--config-url` flag.
    // This is what the "Create desktop shortcut" button targets, so a
    // double-click brings up the whole VOD+EB setup with no dashboard
    // clicks. Idempotent and safe to pass on every launch.
    let launch_eb = raw_args.iter().any(|a| a == "--launch-eb");

    // Sweep any `.old` / `.new` exe left over from a self-update swap. When
    // we got here via the relauncher, this deletes the previous version we
    // just replaced; otherwise it's a harmless no-op.
    self_update::sweep_leftovers();

    // Relaunch handshake: a restart / self-update spawns us with
    // `--await-pid <old>`. Wait for that instance to fully exit (and release
    // the ingest/web ports) before the port pre-flight + bind below, so the
    // two never collide. Capped so a stuck old process can't wedge us.
    if let Some(i) = raw_args.iter().position(|a| a == "--await-pid") {
        if let Some(pid) = raw_args.get(i + 1).and_then(|s| s.parse::<u32>().ok()) {
            self_update::wait_for_pid_exit(pid, 10_000);
        }
    }

    let cfg_path: PathBuf = std::env::var("CONFIG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./instantclone.config.json"));

    // Egress trace - wire-level append-only log next to the config.
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
    // us in a silent retry loop with no console to print to - the worst
    // possible first-run UX. If either port is taken we identify the
    // owning process, propose the next free port in the +0..+9 window,
    // and pop a native modal asking the user to switch or quit.
    #[cfg(windows)]
    {
        // When relaunched by a restart / self-update, the outgoing instance
        // may still be releasing its listening sockets (the OS can report the
        // port as busy - even owned by the now-dead PID - for a brief window
        // after exit). Poll through that window instead of popping the
        // "switch port?" modal, which would otherwise block the relaunch.
        let relaunched = raw_args.iter().any(|a| a == "--await-pid");
        let host_ingest = if settings.ingest_bind_all {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        };
        if !port_free_with_grace(
            &format!("{}:{}", host_ingest, settings.ingest_port),
            relaunched,
        ) {
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
        if !port_free_with_grace(&format!("{}:{}", host_web, settings.web_port), relaunched) {
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
        let ring = match buffer::DiskRing::create(&settings.buffer_path, settings.buffer_bytes()) {
            Ok(r) => Arc::new(r),
            Err(e) => {
                // Cold-start showstopper. Under windows_subsystem=windows
                // there's no console to print this to, so pop a native
                // dialog before exiting - otherwise the binary appears
                // to do nothing and the user has no path to diagnose.
                // Mirrors how `preflight_resolve` surfaces port
                // conflicts before the runtime starts.
                let msg = format!(
                    "InstantClone couldn't open or create the delay buffer file.\n\n\
                     Path:  {}\n\
                     Size:  {} MB\n\
                     Error: {}\n\n\
                     Common causes:\n\
                     - Another instance of InstantClone is still running\n\
                     - Antivirus or backup software is scanning the file\n\
                     - The drive is full or read-only\n\
                     - The path is on a removable drive that's disconnected\n\n\
                     Fix one of the above and relaunch, or change \
                     'buffer_path' in instantclone.config.json to a writable folder.",
                    settings.buffer_path.display(),
                    settings.buffer_mb,
                    e,
                );
                #[cfg(windows)]
                portcheck::show_error("InstantClone -buffer error", &msg);
                eprintln!("{}", msg);
                return Err(e);
            }
        };
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
        // tray icon stays running in the background - closing the tab
        // doesn't kill the proxy. `--no-browser` skips this for autostart
        // / headless setups.
        if !suppress_browser {
            let url = format!("http://127.0.0.1:{}/", settings.web_port);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                open_browser(&url);
            });
        }

        // `--launch-eb` (the desktop-shortcut entry point): once the web
        // server has had a moment to bind, write OBS's VOD-track flag and
        // launch OBS pointed at our multitrack-config endpoint. Runs on a
        // blocking thread so the file write + process spawn don't stall
        // the async runtime, and degrades quietly (a missing OBS just logs
        // a line) so a stale shortcut never crashes the app.
        if launch_eb {
            let web_port = settings.web_port;
            let ctrl_eb = ctrl.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(600)).await;
                let _ =
                    tokio::task::spawn_blocking(move || {
                        match obs_register::set_vod_audio_flag(true) {
                            Ok(true) => ctrl_eb.log("[--launch-eb] VOD-track flag written"),
                            Ok(false) => ctrl_eb
                                .log("[--launch-eb] OBS config not found - is OBS installed?"),
                            Err(e) => ctrl_eb.log(format!("[--launch-eb] flag write failed: {e}")),
                        }
                        match obs_register::launch_obs_with_eb_config(web_port) {
                            Ok(exe) => ctrl_eb
                                .log(format!("[--launch-eb] launched OBS ({})", exe.display())),
                            Err(e) => ctrl_eb.log(format!("[--launch-eb] OBS launch failed: {e}")),
                        }
                    })
                    .await;
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
        eprintln!("\nShutting down ({shutdown_reason}) - closing active streams cleanly...");
        // Flush the egress trace so the last few thousand events make
        // it to disk before the BufWriter is dropped on process exit.
        trace::flush();
        // Flip shutdown on every active destination so each pump sends
        // `deleteStream` to its upstream before the runtime drops them.
        // Tiny window - if a pump is mid-await it'll just exit on next
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

    // Managed "Local test sink" child process. Spawned while any
    // enabled destination has platform "sink"; killed when the last
    // one is disabled or removed. Lives here (not in the pump) because
    // several sink destinations share the one child, exactly like
    // several Twitch destinations share the one Twitch edge.
    let mut sink_child: Option<tokio::process::Child> = None;
    let mut sink_last_spawn: Option<std::time::Instant> = None;

    // Behaviour-toggle state. Auto-arm uses the `prev_ingest_alive`
    // edge detector (fires once per publisher session, on the false ->
    // true transition). Auto-activate reads the controller's
    // `auto_activate_pending` slot which fires once per arm action -
    // arm sets it, cut clears it. See Controller::auto_activate_pending
    // for the full state semantics.
    //
    // Initialised pessimistically (assume the worst-case prior state)
    // so the first iteration's transition detection is accurate even
    // on a fresh process start where ingest is already live from a
    // tight relaunch.
    let mut prev_ingest_alive = ctrl.ingest_alive();

    loop {
        // Snapshot the current desired destinations.
        let desired: Vec<(config::Destination, String)> = { rx.borrow().active_destinations() };

        // Keep the managed test-sink child in sync with the desired set
        // BEFORE the pump diff below, so a freshly enabled sink
        // destination has its receiver listening by the time its pump
        // dials 127.0.0.1.
        let wants_sink = desired.iter().any(|(d, _)| d.platform == "sink");
        manage_test_sink(&mut sink_child, &mut sink_last_spawn, wants_sink, &ctrl).await;

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
                // and close cleanly. Give it a short window - then
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
        //    bailed out cleanly when ingest went away - we want a fresh
        //    pump now that ingest is back).
        let ingest_alive = ctrl.ingest_alive();
        // Vertical-canvas detection runs once per supervisor tick (~2 s)
        // off the shared seq-header cache, so it self-heals as Twitch Dual
        // Format turns on/off mid-stream. `None` until a portrait track is
        // seen; we map that to the 0xFF "unresolved" sentinel each tick so
        // a vanished vertical canvas reverts vertical destinations to
        // "waiting" without a restart.
        let vertical_track = {
            let headers = ctrl.ring.video_seq_headers.lock().unwrap();
            crate::h264::detect_vertical_primary_track(&headers)
        };
        for (dest, url) in &desired {
            // Keep each destination's vertical policy in sync with its
            // current settings and the detected canvas every tick - this
            // is cheap (atomic stores) and means a format edit or a newly
            // appearing vertical canvas takes effect without waiting for
            // an egress restart.
            {
                let state = ctrl.destination_state(&dest.id);
                state
                    .egress_vertical
                    .store(dest.wants_vertical(), std::sync::atomic::Ordering::Relaxed);
                state.vertical_primary_track.store(
                    vertical_track.unwrap_or(0xFF),
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
            // A vertical destination with no 9:16 canvas on the wire yet has
            // nothing to send. Don't open the upstream connection - it would
            // just sit idle, get dropped on the platform's inactivity
            // timeout, and churn reconnects. Tear down any pump and wait for
            // the canvas; the card shows "Waiting for Dual Format". It spawns
            // the moment detection resolves (self-heals within a tick).
            if dest.wants_vertical() && vertical_track.is_none() {
                if let Some((_u, handle)) = running.remove(&dest.id) {
                    let st = ctrl.destination_state(&dest.id);
                    st.shutdown_requested
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    let abort = handle.abort_handle();
                    let _ = tokio::time::timeout(Duration::from_millis(1500), handle).await;
                    abort.abort();
                    ctrl.log(format!(
                        "[{}] vertical: no Dual Format canvas yet - holding off connecting",
                        dest.name
                    ));
                }
                continue;
            }
            // Enhanced Broadcasting override: when the
            // /obs/multitrack-config proxy gets back a real
            // session-allocated IVS URL from Twitch's API, it stashes
            // it on the destination state. The egress MUST connect to
            // that IVS endpoint instead of the user's configured
            // `live.twitch.tv` - only the IVS edge runs the EB
            // transcoder pipeline. Pull the override (if any) here so
            // a change to it triggers the same URL-changed restart
            // path as a user editing the destination.
            let override_url = ctrl
                .destination_state(&dest.id)
                .eb_override_url
                .lock()
                .unwrap()
                .clone();
            // VOD-audio mode: when this is a Twitch destination with
            // vod_audio=true and we haven't yet fetched an IVS session
            // for it, fire one off in the background. The fetch task
            // populates eb_override_url; the next supervisor tick
            // (~2 s) reads it and restarts egress with the IVS URL.
            // Only triggers once ingest is alive - no point burning
            // Twitch API quota for a stream that hasn't started.
            if ingest_alive
                && override_url.is_none()
                && dest.platform == "twitch"
                && dest.vod_audio
                && !dest.stream_key.is_empty()
            {
                let state = ctrl.destination_state(&dest.id);
                // Single-flight: only one session fetch runs at a time.
                // Without this the supervisor would spawn a new API call
                // every ~2 s while no override is set, each allocating a
                // different IVS session and each triggering its own egress
                // restart. try_claim_vod_fetch also re-checks the override
                // under its mutex, so a fetch that just completed on the
                // previous tick can't race a redundant second one in.
                if state.try_claim_vod_fetch() {
                    let key = dest.stream_key.clone();
                    let want_eb = dest.vod_audio_inject_eb;
                    let ctrl_for_log = ctrl.clone();
                    let dest_id = dest.id.clone();
                    // Capture the session this fetch belongs to. If OBS
                    // disconnects before it returns, the epoch moves on and
                    // apply_vod_session_if_current drops the stale result.
                    let epoch = state.session_epoch();
                    tokio::spawn(async move {
                        let result = web::fetch_twitch_vod_session(key, want_eb).await;
                        // complete_vod_fetch applies the URL only if we're
                        // still in the session that asked for it, then releases
                        // the latch whatever the outcome. A tick that then
                        // observes the latch free re-checks the override (now
                        // Some on success) and skips, so no duplicate spawns.
                        match state.complete_vod_fetch(result, epoch) {
                            controller::VodFetchOutcome::Applied => ctrl_for_log.log(format!(
                                "[{}] VOD-audio session allocated by Twitch{}",
                                dest_id,
                                if want_eb { " (with EB ladder)" } else { "" }
                            )),
                            controller::VodFetchOutcome::DiscardedStale => {
                                ctrl_for_log.log(format!(
                                    "[{}] VOD-audio: discarded stale session - publisher \
                                 reconnected while the request was in flight",
                                    dest_id
                                ))
                            }
                            controller::VodFetchOutcome::Failed => ctrl_for_log.log(format!(
                                "[{}] VOD-audio: Twitch API call failed - retrying on next tick",
                                dest_id
                            )),
                        }
                    });
                }
            }
            let effective_url = override_url.as_deref().unwrap_or(url.as_str()).to_string();
            let url = &effective_url;
            let needs_restart = match running.get(&dest.id) {
                Some((existing_url, handle)) => existing_url != url || handle.is_finished(),
                None => true,
            };
            if needs_restart {
                if let Some((_old_url, handle)) = running.remove(&dest.id) {
                    handle.abort();
                    let _ = handle.await;
                }
                // Don't open a fresh egress while OBS isn't sending -
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
                // follows the per-destination IVS session, not the
                // platform. The /obs/multitrack-config proxy sets
                // eb_override_url on exactly one Twitch destination
                // (the one whose stream key it sent to Twitch's API);
                // ONLY that destination owns a session-allocated
                // transcoder ladder. A second Twitch destination -
                // simulcasting to a different Twitch account - has no
                // IVS session here, so it must NOT receive raw
                // multi-track tags (they'd collide on Twitch's edge
                // with the first dest's session). It falls back to the
                // single-track flatten just like a YouTube destination.
                let has_eb_session = override_url.is_some();
                state
                    .pass_through_multitrack_video
                    .store(has_eb_session, std::sync::atomic::Ordering::Relaxed);
                // Audio multi-track passthrough is decoupled from EB.
                // Twitch's regular ingest (live.twitch.tv) has supported
                // VOD audio (OBS "Pista VOD de Twitch", wire TrackId 1)
                // for years and accepts Enhanced-RTMP multi-track audio
                // bit-faithfully. So pass through for any Twitch
                // destination, with or without an EB session. Non-Twitch
                // destinations still drop TrackId != 0 so a simulcast
                // YouTube / Kick doesn't get a track its decoder can't
                // map. Before v0.1.3 we reused the video flag here,
                // which silently dropped VOD audio on every non-EB
                // Twitch destination and wrapped the live audio in
                // multi-track framing the regular ingest renders silent.
                let is_twitch = dest.platform == "twitch";
                state
                    .pass_through_multitrack_audio
                    .store(is_twitch, std::sync::atomic::Ordering::Relaxed);
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
            eprintln!("[egress] idle - add a destination in the web UI");
        }

        // Wake at most every 2 s to re-check ingest_alive, finished
        // pumps, and desired-set membership. Important: this MUST fire
        // even when `running` is empty, otherwise the supervisor blocks
        // forever after refusing to spawn while ingest was dead, and
        // OBS connecting wouldn't wake anything. (Before this change,
        // users had to toggle a destination off+on to get the supervisor
        // to look at the world again.) Settings changes still preempt
        // the wait via `rx.changed()` for instant response.
        // ── Behaviour: auto-pre-arm on stream start ───────────────
        //
        // Edge-detect ingest_alive going false -> true. That's the
        // moment OBS's publisher handshake just completed. Pre-arm
        // fires once per publisher session (the edge condition
        // self-limits) and only when nothing is already armed (so a
        // user who pre-armed and then manually disarmed in the same
        // session doesn't get auto-rearmed).
        {
            let s = rx.borrow();
            let armed_now = ctrl.armed_delay_ms();
            if !prev_ingest_alive
                && ingest_alive
                && s.auto_arm_on_connect
                && armed_now == 0
                && s.auto_arm_delay_ms > 0
            {
                // arm_delay sets target=0 -> non-active, so it'll also
                // flip Controller's auto_activate_pending slot true.
                // That means a pre-arm + auto-activate combo auto-goes-
                // live without the streamer touching anything.
                ctrl.arm_delay(s.auto_arm_delay_ms);
                ctrl.log(format!(
                    "pre-arm: stream started -> armed {} s of delay",
                    s.auto_arm_delay_ms / 1000
                ));
            }
        }

        // ── Behaviour: auto-activate when ready ────────────────────
        //
        // Fires once per arm action, gated by Controller's
        // `auto_activate_pending` slot (set by arm_delay when arming
        // from a non-active state, cleared by activate_delay /
        // stop_delay / begin_publish). This makes Cut actually stick:
        // after the streamer cuts, the slot is clear and the next
        // phase-reverts-to-ready tick doesn't snap us back to active.
        // A subsequent re-arm refills the slot, so the NEXT arm cycle
        // still auto-activates as expected.
        //
        // Phase check stays as the readiness signal - we fire when the
        // buffer's actually full enough, not the instant the slot is
        // pending. If the slot is pending but activate_delay returns
        // BufferShort (a transient race where phase reports ready but
        // the stricter buffer check disagrees), we quietly skip; the
        // slot stays true so the next tick retries.
        {
            let s = rx.borrow();
            if ctrl.auto_activate_pending()
                && s.auto_activate_when_ready
                && ctrl.phase() == "ready"
                && ctrl.target_delay_ms() == 0
            {
                match ctrl.activate_delay() {
                    Ok(ms) => ctrl.log(format!(
                        "auto-activate: buffer ready -> live with {} s delay",
                        ms / 1000
                    )),
                    Err(_) => {
                        // Phase said "ready" but activate_delay's
                        // stricter buffer check disagreed. Quietly skip;
                        // next 2 s tick re-evaluates. The pending slot
                        // stays set (activate_delay didn't reach the
                        // Ok arm) so the retry actually happens.
                    }
                }
            }
        }
        prev_ingest_alive = ingest_alive;

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

/// Spawn / reap / kill the `instantclone sink` child that backs "Local
/// test sink" destinations. Called every supervisor tick.
///
/// Lifecycle rules:
/// * want && not running  → spawn (throttled to one attempt / 10 s so a
///   port conflict doesn't respawn-spam every 2 s tick)
/// * want && running      → no-op (one child serves all sink dests)
/// * !want && running     → kill; the destination was disabled/removed
/// * child died on its own → reap + log, next tick may respawn
///
/// The child's `[sink]`-prefixed lines are forwarded into the dashboard
/// log ring, so "publish accepted" and the 1 Hz stat lines show up in
/// the Logs tab - that's the whole point of a testing destination.
async fn manage_test_sink(
    child: &mut Option<tokio::process::Child>,
    last_spawn: &mut Option<std::time::Instant>,
    want: bool,
    ctrl: &Arc<controller::Controller>,
) {
    // Reap first: a child that died (port conflict, crash) must not
    // count as running, or we would never respawn it.
    if let Some(c) = child.as_mut() {
        if let Ok(Some(status)) = c.try_wait() {
            ctrl.log(format!("[test-sink] exited ({status})"));
            *child = None;
        }
    }
    if !want {
        if let Some(mut c) = child.take() {
            let _ = c.kill().await;
            ctrl.log("[test-sink] stopped - no sink destination enabled");
        }
        return;
    }
    if child.is_some() {
        return;
    }
    if let Some(t) = last_spawn {
        if t.elapsed() < Duration::from_secs(10) {
            return;
        }
    }
    *last_spawn = Some(std::time::Instant::now());
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            ctrl.log(format!("[test-sink] can't locate own exe: {e}"));
            return;
        }
    };
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args([
        "sink",
        "--port",
        &config::SINK_RTMP_PORT.to_string(),
        "--web-port",
        &config::SINK_WEB_PORT.to_string(),
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    // The runtime dropping the supervisor task (app quit) takes the
    // child with it - no orphaned sink holding the ports.
    .kill_on_drop(true);
    // The parent runs without a console (windows_subsystem = windows);
    // without this flag the child would flash one open.
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    match cmd.spawn() {
        Ok(mut c) => {
            if let Some(out) = c.stdout.take() {
                tokio::spawn(forward_sink_lines(out, ctrl.clone()));
            }
            if let Some(err) = c.stderr.take() {
                tokio::spawn(forward_sink_lines(err, ctrl.clone()));
            }
            ctrl.log(format!(
                "[test-sink] started - rtmp :{}, live player http://127.0.0.1:{}/",
                config::SINK_RTMP_PORT,
                config::SINK_WEB_PORT
            ));
            *child = Some(c);
        }
        Err(e) => ctrl.log(format!("[test-sink] failed to spawn: {e}")),
    }
}

/// Pipe one of the sink child's output streams into the dashboard log.
/// Only `[sink...]`-prefixed lines pass - the startup banner box art and
/// blank lines stay out of the 512-entry log ring.
async fn forward_sink_lines(
    stream: impl tokio::io::AsyncRead + Unpin,
    ctrl: Arc<controller::Controller>,
) {
    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.starts_with("[sink") {
            ctrl.log(line);
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

/// Is `addr` bindable? When `grace` is set (we were relaunched by a restart /
/// self-update), wait out the outgoing instance's socket-release window before
/// deciding it's taken - so a normal handoff never pops the "switch port?"
/// modal. Returns the moment the port frees, so the common case is instant; a
/// genuine conflict still falls through after the window.
#[cfg(windows)]
fn port_free_with_grace(addr: &str, grace: bool) -> bool {
    if grace {
        portcheck::wait_until_bindable(addr, Duration::from_secs(20))
    } else {
        portcheck::is_port_free(addr)
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

/// Create the overlays directory (if missing) and drop the folder README
/// plus the hand-write example, if absent. Idempotent - safe on every
/// startup; user edits are never overwritten. The built-in *overlays*
/// themselves are no longer dropped here: the dashboard bakes the Studio
/// presets to real files on first load (see `seedPresets`).
fn ensure_overlays_dir(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    let drop = |name: &str, body: &str| {
        let path = dir.join(name);
        if !path.exists() {
            let _ = std::fs::write(&path, body);
        }
    };
    drop("README.md", OVERLAY_README);
    drop("custom-template.html", OVERLAY_CUSTOM_TEMPLATE);
}

// Embedded straight from the canonical files in ../overlays/ so a binary
// install drops the *same* content that lives in the repo. include_str!
// keeps them in lockstep - no risk of drifting silently.
const OVERLAY_README: &str = include_str!("../overlays/README.md");
const OVERLAY_CUSTOM_TEMPLATE: &str = include_str!("../overlays/custom-template.html");
