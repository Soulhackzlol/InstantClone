//! One-click self-update for the portable Windows build.
//!
//! Updating is optional - the About tab always links the manual download
//! too - but for users who'd rather not fish a new exe out of GitHub, the
//! "Update now" button drives this two-step flow:
//!
//!   1. [`prepare`]: hit the latest GitHub release, download the new exe
//!      and the published `SHA256SUMS.txt`, verify the hash, and stage the
//!      bytes next to the running exe as `instantclone.exe.new`. Everything
//!      that can fail - offline, rate-limited, hash mismatch, unwritable
//!      folder - fails HERE, before the live exe is touched, and surfaces a
//!      message to the UI. Nothing is committed.
//!
//!   2. [`stage`] parks the verified update and the caller asks for an
//!      ordinary restart, so the update gets the same clean shutdown as
//!      Restart and Quit (every destination sent `deleteStream`, the trace
//!      flushed, integration counters saved). At the very end of that
//!      shutdown, [`restart_now`] checks the staged file's hash again,
//!      renames the running exe to `.old` (Windows lets you rename, but not
//!      delete/overwrite, a running image), moves the staged `.new` into its
//!      place and relaunches. The `.old` / `.new` leftovers are swept on the
//!      next startup (see [`sweep_leftovers`]).
//!
//! Why the checksum matters: we download an executable and then run it, so
//! verifying it against the hash the release pipeline published (over a
//! separate TLS-protected request) is the line between "convenient" and
//! "remote code execution". A mismatch aborts with zero changes made.
//!
//! The user's config lives in a separate file, so settings, destinations,
//! and overlays are untouched by the swap.

use std::path::{Path, PathBuf};

const RELEASE_DL_BASE: &str = "https://github.com/Soulhackzlol/InstantClone/releases/download";

/// Where release files are downloaded from. A debug build can point it at a
/// local mock with INSTANTCLONE_RELEASE_DOWNLOADS (with the update check's
/// INSTANTCLONE_GITHUB_API) to try a whole update without GitHub; a release
/// build always downloads from GitHub.
fn release_downloads() -> String {
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("INSTANTCLONE_RELEASE_DOWNLOADS") {
        return url;
    }
    RELEASE_DL_BASE.to_string()
}

/// A verified update staged on disk, ready for [`stage`].
pub struct Staged {
    /// Path of the currently running exe (the swap target).
    pub exe: PathBuf,
    /// The downloaded-and-verified `*.new` file sitting beside it.
    pub staged: PathBuf,
    /// The version we're moving to (no leading `v`), for UI display.
    pub version: String,
    /// A new build of the version already running, not a newer version.
    pub rebuild: bool,
    /// SHA-256 the staged file was verified against, checked again just
    /// before the swap: the file sits on disk through the shutdown.
    sha256: String,
}

/// The verified update the next restart installs. Set by [`stage`], taken
/// by [`restart_now`] once main has shut everything down cleanly.
static PENDING: crate::sync::Mutex<Option<Staged>> = crate::sync::Mutex::new(None);

/// Release asset filename for the running platform. MUST match the names
/// produced by `.github/workflows/release.yml`. `tag` includes the leading
/// `v` (e.g. `v0.1.13`).
fn release_asset_name(tag: &str) -> String {
    #[cfg(windows)]
    {
        format!("instantclone-{tag}-windows-x64.exe")
    }
    #[cfg(target_os = "linux")]
    {
        format!("instantclone-{tag}-linux-x64")
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        format!("instantclone-{tag}-unknown")
    }
}

/// Sibling path `exe` + `.` + `ext`, e.g. `instantclone.exe.new`.
fn sidecar(exe: &Path, ext: &str) -> PathBuf {
    let mut s = exe.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

/// Delete any `.old` / `.new` leftovers from a previous update swap. Called
/// once at startup: a fresh process that just relaunched sweeps the `.old`
/// copy of the version it replaced, and a stale `.new` (from an update that
/// was staged but never committed) is harmless to drop - it re-downloads.
/// A `.new` younger than `STAGED_IN_USE` stays: two setups can run from one
/// exe (CONFIG_PATH), and the other may be about to swap it in.
pub fn sweep_leftovers() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(sidecar(&exe, "old"));
        let staged = sidecar(&exe, "new");
        if !is_younger_than(&staged, STAGED_IN_USE) {
            let _ = std::fs::remove_file(staged);
        }
    }
}

/// How long a staged `.new` may be waiting for its own setup's shutdown to
/// swap it in. That takes seconds; ten minutes is far beyond any of it.
const STAGED_IN_USE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Whether `path` exists and was last written less than `age` ago.
fn is_younger_than(path: &Path, age: std::time::Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|elapsed| elapsed < age)
}

/// Download + verify + stage the latest release. Returns a [`Staged`] on
/// success, or a user-facing error string on any failure (nothing is
/// committed in the error case).
pub fn prepare() -> Result<Staged, String> {
    let current = crate::update_check::current_version();
    let info = crate::update_check::check_update();
    let latest = info.latest.ok_or_else(|| {
        let why = info.error.unwrap_or_else(|| "no answer".into());
        format!("Couldn't reach GitHub to check for the update ({why}).")
    })?;
    if !info.update_available {
        return Err(format!(
            "You already have the latest build of v{current}. Nothing to install."
        ));
    }

    let agent = crate::https::https_agent();
    let bytes = fetch_verified(&agent, &release_downloads(), &latest)?;

    let exe = std::env::current_exe().map_err(|e| format!("can't locate the running exe: {e}"))?;
    stage_beside(&exe, &bytes, latest, info.rebuild)
}

/// Write verified `bytes` next to `exe` as `<exe>.new`, ready to swap in.
/// The running exe is not touched: that waits for the restart.
fn stage_beside(
    exe: &Path,
    bytes: &[u8],
    version: String,
    rebuild: bool,
) -> Result<Staged, String> {
    let staged = sidecar(exe, "new");
    std::fs::write(&staged, bytes)
        .map_err(|e| format!("can't write the update next to the exe: {e}"))?;

    // A downloaded Linux binary lands non-executable; the swap-then-relaunch
    // would fail with EACCES without this. Windows keys off the extension,
    // so this is unix-only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&staged)
            .map_err(|e| format!("can't stat the staged update: {e}"))?
            .permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&staged, perm)
            .map_err(|e| format!("can't make the staged update executable: {e}"))?;
    }

    Ok(Staged {
        exe: exe.to_path_buf(),
        staged,
        version,
        rebuild,
        sha256: crate::sha256::hex(bytes),
    })
}

/// Park a verified update for the next restart, which installs it once the
/// shutdown has closed every stream cleanly. The caller asks for that
/// restart. A second update staged before it replaces the first.
pub fn stage(s: Staged) {
    *PENDING.lock() = Some(s);
}

/// Whether an update is staged and waiting for its restart.
pub fn is_staged() -> bool {
    PENDING.lock().is_some()
}

/// The staged update, taken by the restart that installs it, so a later
/// restart can't install it again.
fn take_pending() -> Option<Staged> {
    PENDING.lock().take()
}

/// Download the release exe for `version` from `base` and verify it against
/// the release's published `SHA256SUMS.txt`, returning the verified bytes.
/// Split out from [`prepare`] so a localhost mock can exercise the full
/// download-and-verify path with no GitHub round-trip (see tests).
fn fetch_verified(agent: &ureq::Agent, base: &str, version: &str) -> Result<Vec<u8>, String> {
    // Checksum first - cheap, and it tells us what to expect.
    let expected = published_sha256(agent, base, version)?;

    // Then the binary, which we verify before it ever lands at a runnable
    // path.
    let tag = format!("v{version}");
    let exe_url = format!("{base}/{tag}/{}", release_asset_name(&tag));
    let bytes = http_get_bytes(agent, &exe_url)?;
    let got = crate::sha256::hex(&bytes);
    if !got.eq_ignore_ascii_case(&expected) {
        return Err("downloaded file failed its checksum - aborting, no changes made".to_string());
    }
    Ok(bytes)
}

/// The SHA-256 release `version` published for this platform's exe, from
/// the release's `SHA256SUMS.txt` under `base`.
fn published_sha256(agent: &ureq::Agent, base: &str, version: &str) -> Result<String, String> {
    let tag = format!("v{version}");
    let exe_name = release_asset_name(&tag);
    let sums = http_get_text(agent, &format!("{base}/{tag}/SHA256SUMS.txt"))?;
    parse_checksum(&sums, &exe_name)
        .ok_or_else(|| format!("no checksum for {exe_name} in SHA256SUMS.txt"))
}

/// The SHA-256 of the exe release `version` publishes, when it differs from
/// the one running: a new build shipped under the same version number. None
/// when it's the same build or it can't tell (offline, no checksum for this
/// platform, unreadable exe). The hash tells one rebuild from the next.
pub fn published_rebuild(agent: &ureq::Agent, version: &str) -> Option<String> {
    rebuild_hash(agent, &release_downloads(), version, running_sha256()?)
}

fn rebuild_hash(agent: &ureq::Agent, base: &str, version: &str, running: &str) -> Option<String> {
    let published = published_sha256(agent, base, version).ok()?;
    (!published.eq_ignore_ascii_case(running)).then_some(published)
}

/// SHA-256 of the running exe, hashed once per process. An update swap
/// restarts the process, and a file replaced under a running process on
/// Linux is not the code that is running anyway.
fn running_sha256() -> Option<&'static str> {
    static HASH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    HASH.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        let bytes = std::fs::read(exe).ok()?;
        Some(crate::sha256::hex(&bytes))
    })
    .as_deref()
}

/// Install a staged update: check the staged file still has the hash it was
/// verified against, then swap it in. On any failure the running exe stays
/// where it is and the staged file is dropped, so the relaunch that follows
/// starts the current version again.
fn install(s: &Staged) -> Result<(), String> {
    let still_verified = std::fs::read(&s.staged)
        .map(|bytes| crate::sha256::hex(&bytes).eq_ignore_ascii_case(&s.sha256))
        .unwrap_or(false);
    if !still_verified {
        let _ = std::fs::remove_file(&s.staged);
        return Err("the staged update changed or vanished before the restart".into());
    }
    swap_in_place(&s.exe, &s.staged).map_err(|e| e.to_string())
}

/// Move `staged` into `exe`'s place, parking the running image at `.old`.
/// On Windows you can rename (but not delete/overwrite) a running exe, so
/// this rename pair is the move that makes in-place update possible. Rolls
/// back the first rename if the second fails, so a half-swap never leaves
/// the install without a runnable exe. Pure filesystem work, unit-tested.
fn swap_in_place(exe: &Path, staged: &Path) -> std::io::Result<()> {
    let old = sidecar(exe, "old");
    let _ = std::fs::remove_file(&old); // clear any stale .old first
    std::fs::rename(exe, &old)?;
    if let Err(e) = std::fs::rename(staged, exe) {
        let _ = std::fs::rename(&old, exe); // best-effort rollback
        return Err(e);
    }
    Ok(())
}

/// Relaunch in place and exit: the last step of main's clean shutdown on a
/// restart. A staged update is installed first, so the relaunch starts the
/// new version; if installing fails, the current one starts again. Does not
/// return.
pub fn restart_now() -> ! {
    // Resolved before the swap: on Linux the running image's path follows
    // the rename to `.old`, and the relaunch must start the file now at the
    // original path.
    let exe = std::env::current_exe();
    if let Some(update) = take_pending() {
        if let Err(e) = install(&update) {
            log_line!("self-update: staying on the current version: {e}");
        }
    }
    if let Ok(exe) = exe {
        spawn_relauncher(&exe);
    }
    std::process::exit(0);
}

/// Relaunch the fresh exe, handing it our PID so it waits for us to exit
/// (and release the ingest/web ports) before it binds. We spawn the real exe
/// directly - no `cmd`/`ping` shim - so there's no console flash and no blind
/// sleep that can race the port. `--no-browser` stops it from popping a
/// second dashboard tab; the already-open dashboard reconnects on its own.
fn spawn_relauncher(exe: &Path) {
    let pid = std::process::id().to_string();
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--await-pid").arg(&pid).arg("--no-browser");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(DETACHED_PROCESS);
    }
    let _ = cmd.spawn();
}

/// Clear the inheritable flag on a listening socket so a relaunched child
/// never inherits it. Without this, a restart/self-update spawns the new
/// instance as our child, which inherits our listening sockets; when we then
/// exit, the OS keeps the ports bound (still attributed to our dead PID) via
/// the child's inherited copy, and the child can never bind them - the restart
/// hangs and eventually shows a bogus "port in use" prompt.
#[cfg(windows)]
pub fn dont_inherit<S: std::os::windows::io::AsRawSocket>(sock: &S) {
    use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE};
    const HANDLE_FLAG_INHERIT: u32 = 0x1;
    unsafe {
        SetHandleInformation(sock.as_raw_socket() as HANDLE, HANDLE_FLAG_INHERIT, 0);
    }
}
#[cfg(not(windows))]
pub fn dont_inherit<S>(_sock: &S) {}

/// Block until process `pid` exits (or `timeout_ms` elapses) so a relaunched
/// instance never races the outgoing one for the ingest/web ports. Returns
/// at once if the process is already gone.
#[cfg(windows)]
pub fn wait_for_pid_exit(pid: u32, timeout_ms: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};
    const PROCESS_SYNCHRONIZE: u32 = 0x0010_0000;
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return; // already exited (or not visible) - nothing to wait on
        }
        WaitForSingleObject(h, timeout_ms);
        CloseHandle(h);
    }
}
/// Whether a `/proc/<pid>/stat` body describes a process that has already let
/// go of its sockets.
///
/// A zombie counts as gone, and that is the point of parsing the state at all
/// rather than just checking the file exists (or calling `kill(pid, 0)`, which
/// succeeds for zombies too). The kernel closes every descriptor as a process
/// exits, before the entry becomes a zombie, so the ingest and web ports are
/// already free while the entry lingers waiting to be reaped. Whether that
/// reap is prompt depends on whoever launched us - a shell, a terminal that
/// has since closed, systemd - and none of that is our business. Waiting for
/// it would stall every update by the full timeout for no gain.
///
/// An unparseable body reads as gone: the timeout exists so a stuck process
/// cannot wedge the relaunch, and the port pre-flight downstream is the real
/// backstop either way.
///
/// Pure parsing, built on every platform so its test runs on every CI target
/// even though only Linux has a `/proc` to read.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn proc_stat_has_exited(stat: &str) -> bool {
    // `<pid> (<comm>) <state> ...`. `comm` is the executable name, arbitrary
    // and free to contain both spaces and parentheses, so the state is the
    // first field after the LAST ')' - not the second whitespace-separated
    // field, which is where a naive split lands for anything odd.
    match stat.rsplit_once(')') {
        Some((_, rest)) => matches!(rest.split_whitespace().next(), Some("Z") | None),
        None => true,
    }
}

/// Block until process `pid` exits (or `timeout_ms` elapses) so a relaunched
/// instance never races the outgoing one for the ingest/web ports. Returns
/// at once if the process is already gone.
///
/// This is the Linux half of what `WaitForSingleObject` does on Windows.
/// Without it the stub returned instantly, the fresh process reached its port
/// pre-flight while the old one still held 1935 and 7799, and a self-update
/// ended with the app failing to come back.
#[cfg(target_os = "linux")]
pub fn wait_for_pid_exit(pid: u32, timeout_ms: u32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
    loop {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Err(_) => return, // reaped, or never existed
            Ok(stat) if proc_stat_has_exited(&stat) => return,
            Ok(_) => {}
        }
        if std::time::Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

// Windows and Linux are the platforms we publish. Anywhere else the relaunch
// goes ahead without waiting, exactly as it did before Linux had an answer.
#[cfg(not(any(windows, target_os = "linux")))]
pub fn wait_for_pid_exit(_pid: u32, _timeout_ms: u32) {}

/// Find the hash for `name` in a `SHA256SUMS.txt` body. Each line is
/// `<64-hex-hash>  <filename>`; we match the filename and return the hash.
fn parse_checksum(sums: &str, name: &str) -> Option<String> {
    for line in sums.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let hash = it.next()?;
        let file = it.next()?;
        // The filename column may carry a leading `*` (binary-mode marker).
        let file = file.trim_start_matches('*');
        if file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

fn user_agent() -> String {
    format!("InstantClone/{}", crate::update_check::current_version())
}

fn http_get_text(agent: &ureq::Agent, url: &str) -> Result<String, String> {
    let mut resp = agent
        .get(url)
        .header("User-Agent", user_agent())
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    if resp.status() != 200 {
        return Err(format!("server returned HTTP {}", resp.status()));
    }
    resp.body_mut()
        .read_to_string()
        .map_err(|e| format!("read failed: {e}"))
}

fn http_get_bytes(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
    let mut resp = agent
        .get(url)
        .header("User-Agent", user_agent())
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    if resp.status() != 200 {
        return Err(format!("server returned HTTP {}", resp.status()));
    }
    // The release exe is ~1.2 MB; lift the reader's default cap well above
    // that so a future, larger build still downloads.
    resp.body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| format!("read failed: {e}"))
}

#[cfg(test)]
mod tests {
    /// Another setup's update, staged a moment ago from the same exe, is not
    /// swept away; an old leftover, or none at all, is not kept.
    #[test]
    fn a_freshly_staged_update_is_left_for_its_setup() {
        let dir = crate::test_support::TestDir::new("ic-staged-age");
        let staged = dir.path().join("instantclone.exe.new");
        assert!(
            !super::is_younger_than(&staged, super::STAGED_IN_USE),
            "no file"
        );
        std::fs::write(&staged, b"exe").unwrap();
        assert!(super::is_younger_than(&staged, super::STAGED_IN_USE));
        assert!(!super::is_younger_than(&staged, std::time::Duration::ZERO));
    }

    use super::parse_checksum;

    const NAME: &str = "instantclone-v0.1.6-windows-x64.exe";

    /// The self-update asset name MUST match what release.yml publishes, or
    /// the checksum lookup can never find its line. Read from the workflow
    /// itself, so a rename on either side fails here.
    #[test]
    fn release_asset_name_matches_release_workflow() {
        let workflow = include_str!("../.github/workflows/release.yml");
        let name = super::release_asset_name("$tag");
        assert!(
            workflow.contains(&name),
            "release.yml never publishes {name}"
        );
    }

    /// The relaunch handshake reads a process's state, not just whether its
    /// `/proc` entry is there. A zombie has already closed its sockets, so it
    /// must read as gone - otherwise every self-update on Linux stalls for the
    /// full ten seconds whenever the launching shell is slow to reap.
    #[test]
    fn a_zombie_has_already_released_its_ports() {
        assert!(
            super::proc_stat_has_exited("4242 (instantclone) Z 1 4242 4242 0 -1 4194304"),
            "a zombie holds no sockets"
        );
        assert!(
            !super::proc_stat_has_exited("4242 (instantclone) S 1 4242 4242 0 -1 4194304"),
            "sleeping is still running"
        );
        assert!(
            !super::proc_stat_has_exited("4242 (instantclone) R 1 4242 4242 0 -1 4194304"),
            "running is still running"
        );
    }

    /// `comm` is the executable's name and can hold spaces and parentheses,
    /// which is why the state is read after the LAST ')'. Splitting on
    /// whitespace and taking field three - the obvious reading - lands inside
    /// the name for anything odd and misreports the state.
    #[test]
    fn a_process_name_with_spaces_or_parens_does_not_confuse_the_state() {
        assert!(
            super::proc_stat_has_exited("77 (my app (old) v2) Z 1 77 77 0 -1 0"),
            "state is after the last paren, not the second field"
        );
        assert!(!super::proc_stat_has_exited(
            "77 (my app (old) v2) S 1 77 77 0 -1 0"
        ),);
        // Unparseable reads as gone: the timeout exists so a stuck process
        // cannot wedge the relaunch, and the port pre-flight is the backstop.
        assert!(super::proc_stat_has_exited(""));
        assert!(super::proc_stat_has_exited("nonsense with no paren"));
    }

    /// The real thing, on the platform that has a `/proc`: our own process is
    /// alive, and a pid that cannot exist is not.
    #[cfg(target_os = "linux")]
    #[test]
    fn waiting_on_a_dead_pid_returns_immediately() {
        let stat = std::fs::read_to_string(format!("/proc/{}/stat", std::process::id()))
            .expect("our own /proc entry");
        assert!(
            !super::proc_stat_has_exited(&stat),
            "this test process is running: {stat}"
        );

        // A pid past the kernel's maximum can never exist, so the wait must
        // fall straight through rather than burn its timeout.
        let start = std::time::Instant::now();
        super::wait_for_pid_exit(u32::MAX, 10_000);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "a pid that cannot exist must not cost the timeout"
        );
    }

    #[test]
    fn parses_standard_sha256sums_line() {
        let body = format!("{}  {NAME}\n", "a".repeat(64));
        assert_eq!(parse_checksum(&body, NAME), Some("a".repeat(64)));
    }

    #[test]
    fn picks_the_right_file_among_several() {
        let body = format!(
            "{}  other-file.exe\n{}  {NAME}\n",
            "b".repeat(64),
            "c".repeat(64)
        );
        assert_eq!(parse_checksum(&body, NAME), Some("c".repeat(64)));
    }

    #[test]
    fn tolerates_binary_mode_star_and_uppercase() {
        let body = format!("{} *{NAME}\n", "A".repeat(64));
        assert_eq!(parse_checksum(&body, NAME), Some("a".repeat(64)));
    }

    #[test]
    fn rejects_non_hex_and_wrong_length() {
        let body = format!("{}  {NAME}\n", "z".repeat(64));
        assert_eq!(parse_checksum(&body, NAME), None);
        let short = format!("{}  {NAME}\n", "a".repeat(40));
        assert_eq!(parse_checksum(&short, NAME), None);
    }

    #[test]
    fn none_when_file_absent() {
        let body = format!("{}  some-other.exe\n", "a".repeat(64));
        assert_eq!(parse_checksum(&body, NAME), None);
    }
}

// End-to-end tests of the two risky parts - the download+verify and the
// on-disk swap - without GitHub or a live process. The mock stands in for
// the release server, so this is exactly the "build a fake old version and
// try to update" check, just driven from a unit test.
#[cfg(test)]
mod e2e {
    use super::{fetch_verified, install, rebuild_hash, sidecar, stage_beside, swap_in_place};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Spin up a one-shot localhost HTTP server that answers the two GETs
    /// `fetch_verified` makes: the checksum file (path contains
    /// `SHA256SUMS`) and the exe (anything else). Returns its port.
    fn serve_release(exe_body: Vec<u8>, sums_body: String) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut sock, _) = match listener.accept() {
                    Ok(x) => x,
                    Err(_) => break,
                };
                let mut buf = [0u8; 2048];
                let n = sock.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first = req.lines().next().unwrap_or("");
                let body: Vec<u8> = if first.contains("SHA256SUMS") {
                    sums_body.clone().into_bytes()
                } else {
                    exe_body.clone()
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes());
                let _ = sock.write_all(&body);
                let _ = sock.flush();
            }
        });
        port
    }

    #[test]
    fn fetch_verified_accepts_matching_checksum() {
        let exe = b"FAKE-INSTANTCLONE-EXE-BODY-v9.9.9".to_vec();
        let hash = crate::sha256::hex(&exe);
        // Name the asset per the running platform, exactly as fetch_verified
        // resolves it - a Windows-only literal fails the checksum lookup on
        // the Linux build.
        let sums = format!("{hash}  {}\n", super::release_asset_name("v9.9.9"));
        let port = serve_release(exe.clone(), sums);
        let base = format!("http://127.0.0.1:{port}");
        let agent = crate::https::https_agent();
        let got = fetch_verified(&agent, &base, "9.9.9").expect("genuine download should verify");
        assert_eq!(got, exe);
    }

    /// A release republished under the same version number: its checksum
    /// file names a different exe than the one running.
    #[test]
    fn a_new_build_of_the_same_version_is_spotted() {
        let running = crate::sha256::hex(b"BUILD-AS-FIRST-PUBLISHED");
        let rebuilt = crate::sha256::hex(b"BUILD-REPUBLISHED-WITH-A-FIX");
        let sums = format!("{rebuilt}  {}\n", super::release_asset_name("v9.9.9"));
        let base = format!("http://127.0.0.1:{}", serve_release(Vec::new(), sums));
        let agent = crate::https::https_agent();
        assert_eq!(
            rebuild_hash(&agent, &base, "9.9.9", &running),
            Some(rebuilt)
        );
    }

    #[test]
    fn the_build_already_running_is_not_offered_again() {
        let running = crate::sha256::hex(b"BUILD-AS-PUBLISHED");
        // Published in uppercase: the comparison ignores case.
        let sums = format!(
            "{}  {}\n",
            running.to_ascii_uppercase(),
            super::release_asset_name("v9.9.9")
        );
        let base = format!("http://127.0.0.1:{}", serve_release(Vec::new(), sums));
        let agent = crate::https::https_agent();
        assert_eq!(rebuild_hash(&agent, &base, "9.9.9", &running), None);
    }

    #[test]
    fn no_checksum_for_this_platform_is_unknown_not_an_update() {
        let hash = crate::sha256::hex(b"SOME-OTHER-FILE");
        let sums = format!("{hash}  optional-vod-unlocker.lua\n");
        let base = format!("http://127.0.0.1:{}", serve_release(Vec::new(), sums));
        let agent = crate::https::https_agent();
        assert_eq!(rebuild_hash(&agent, &base, "9.9.9", &hash), None);
    }

    #[test]
    fn fetch_verified_rejects_tampered_body() {
        // Publish the checksum of the genuine file, but serve a different
        // (tampered) body - verification must reject it.
        let genuine = b"GENUINE-BODY".to_vec();
        let hash = crate::sha256::hex(&genuine);
        let sums = format!("{hash}  {}\n", super::release_asset_name("v9.9.9"));
        let tampered = b"TAMPERED-PAYLOAD-PRETENDING-TO-BE-THE-RELEASE".to_vec();
        let port = serve_release(tampered, sums);
        let base = format!("http://127.0.0.1:{port}");
        let agent = crate::https::https_agent();
        let err = fetch_verified(&agent, &base, "9.9.9").unwrap_err();
        assert!(
            err.contains("checksum"),
            "expected checksum rejection, got: {err}"
        );
    }

    // Real network: downloads the actual latest release exe from GitHub and
    // verifies it against the real published SHA256SUMS.txt. Ignored by
    // default (needs a published release + outbound HTTPS); run on demand
    // with `cargo test --release real_release -- --ignored` to prove the
    // production download+verify path before tagging.
    #[test]
    #[ignore = "hits real GitHub releases; run explicitly with --ignored"]
    fn real_release_downloads_and_verifies() {
        let info = crate::update_check::check_update();
        let latest = info
            .latest
            .expect("no published release / couldn't reach GitHub");
        let agent = crate::https::https_agent();
        let bytes = super::fetch_verified(&agent, super::RELEASE_DL_BASE, &latest)
            .expect("the real published release should pass its own checksum");
        assert!(
            bytes.len() > 100_000,
            "a real release exe should be >100 KB, got {} bytes",
            bytes.len()
        );
    }

    #[test]
    fn swap_in_place_replaces_exe_and_parks_old() {
        let scratch = crate::test_support::TestDir::new("ic-swap");
        let dir = scratch.path().to_path_buf();
        let exe = dir.join("instantclone.exe");
        let staged = sidecar(&exe, "new");
        std::fs::write(&exe, b"OLD-VERSION").unwrap();
        std::fs::write(&staged, b"NEW-VERSION").unwrap();

        swap_in_place(&exe, &staged).unwrap();

        assert_eq!(std::fs::read(&exe).unwrap(), b"NEW-VERSION");
        assert_eq!(std::fs::read(sidecar(&exe, "old")).unwrap(), b"OLD-VERSION");
        assert!(!staged.exists(), "staged .new should have been consumed");
    }

    /// The half-swap case: the running exe is already parked at `.old`
    /// when the second rename fails (here: the staged file vanished, as
    /// when antivirus quarantines it). The rollback must put the old exe
    /// back, or the install is left with nothing to launch.
    #[test]
    fn swap_in_place_rolls_back_when_the_staged_file_is_missing() {
        let scratch = crate::test_support::TestDir::new("ic-swap-rollback");
        let dir = scratch.path().to_path_buf();
        let exe = dir.join("instantclone.exe");
        let staged = sidecar(&exe, "new");
        std::fs::write(&exe, b"OLD-VERSION").unwrap();
        let _ = std::fs::remove_file(&staged);

        let result = swap_in_place(&exe, &staged);

        let exe_bytes = std::fs::read(&exe);
        let old_left_behind = sidecar(&exe, "old").exists();
        assert!(result.is_err(), "a missing staged file must fail the swap");
        assert_eq!(exe_bytes.unwrap(), b"OLD-VERSION");
        assert!(
            !old_left_behind,
            "rollback must move .old back, not copy it"
        );
    }

    /// A folder of its own per test, holding a fake running exe.
    fn fake_install(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("ic-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("instantclone.exe");
        std::fs::write(&exe, b"OLD-VERSION").unwrap();
        (dir, exe)
    }

    /// The order that keeps a clean shutdown possible: staging only writes
    /// `.new`, the running exe stays put until the restart installs it.
    #[test]
    fn staging_leaves_the_running_exe_alone_until_the_restart_installs_it() {
        let (dir, exe) = fake_install("stage-order");

        let staged = stage_beside(&exe, b"NEW-VERSION", "9.9.9".into(), false).unwrap();
        let before_restart = std::fs::read(&exe).unwrap();
        let installed = install(&staged);
        let after_restart = std::fs::read(&exe).unwrap();
        let parked = std::fs::read(sidecar(&exe, "old")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(before_restart, b"OLD-VERSION", "staging must not swap");
        assert!(installed.is_ok(), "{installed:?}");
        assert_eq!(after_restart, b"NEW-VERSION");
        assert_eq!(parked, b"OLD-VERSION");
    }

    /// The staged file waits on disk through the shutdown, so the restart
    /// checks it again: a changed file is never run.
    #[test]
    fn a_staged_file_changed_before_the_restart_is_not_installed() {
        let (dir, exe) = fake_install("stage-tamper");
        let staged = stage_beside(&exe, b"NEW-VERSION", "9.9.9".into(), false).unwrap();
        std::fs::write(&staged.staged, b"SOMETHING-ELSE").unwrap();

        let installed = install(&staged);
        let exe_bytes = std::fs::read(&exe).unwrap();
        let leftovers = (staged.staged.exists(), sidecar(&exe, "old").exists());
        let _ = std::fs::remove_dir_all(&dir);

        assert!(installed.is_err());
        assert_eq!(exe_bytes, b"OLD-VERSION", "the current version must stay");
        assert_eq!(leftovers, (false, false), "dropped, and nothing parked");
    }

    /// Antivirus can take the staged file during the shutdown. The install
    /// fails without touching the exe, so the relaunch starts it again.
    #[test]
    fn a_staged_file_that_vanished_leaves_the_install_runnable() {
        let (dir, exe) = fake_install("stage-vanish");
        let staged = stage_beside(&exe, b"NEW-VERSION", "9.9.9".into(), true).unwrap();
        std::fs::remove_file(&staged.staged).unwrap();

        let installed = install(&staged);
        let exe_bytes = std::fs::read(&exe).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(installed.is_err());
        assert_eq!(exe_bytes, b"OLD-VERSION");
    }

    /// One staged update at a time, taken by the restart that installs it.
    #[test]
    fn a_staged_update_waits_for_one_restart() {
        let (dir, exe) = fake_install("stage-pending");
        let staged = stage_beside(&exe, b"NEW-VERSION", "9.9.9".into(), false).unwrap();

        super::stage(staged);
        let waiting = super::is_staged();
        let taken = super::take_pending();
        let after = super::is_staged();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(waiting);
        assert_eq!(taken.map(|s| s.version), Some("9.9.9".to_string()));
        assert!(!after, "a later restart must not install it again");
    }
}
