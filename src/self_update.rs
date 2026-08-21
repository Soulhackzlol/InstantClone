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
//!   2. [`commit_and_relaunch`]: rename the running exe to `.old` (Windows
//!      lets you rename, but not delete/overwrite, a running image), move
//!      the staged `.new` into its place, spawn a tiny detached `cmd` that
//!      waits ~2 s for our TCP ports to free and then relaunches the fresh
//!      exe, and hard-exit. The `.old` / `.new` leftovers are swept on the
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

/// A verified update staged on disk, ready for [`commit_and_relaunch`].
pub struct Staged {
    /// Path of the currently running exe (the swap target).
    pub exe: PathBuf,
    /// The downloaded-and-verified `*.new` file sitting beside it.
    pub staged: PathBuf,
    /// The version we're moving to (no leading `v`), for UI display.
    pub version: String,
}

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
pub fn sweep_leftovers() {
    if let Ok(exe) = std::env::current_exe() {
        for ext in ["old", "new"] {
            let _ = std::fs::remove_file(sidecar(&exe, ext));
        }
    }
}

/// Download + verify + stage the latest release. Returns a [`Staged`] on
/// success, or a user-facing error string on any failure (nothing is
/// committed in the error case).
pub fn prepare() -> Result<Staged, String> {
    let current = crate::update_check::current_version();
    let info = crate::update_check::check_update();
    let latest = info
        .latest
        .ok_or_else(|| info.error.unwrap_or_else(|| "couldn't reach GitHub".into()))?;
    if !info.update_available {
        return Err(format!("Already on the latest release (v{current})."));
    }

    let agent = crate::https::https_agent();
    let bytes = fetch_verified(&agent, RELEASE_DL_BASE, &latest)?;

    let exe = std::env::current_exe().map_err(|e| format!("can't locate the running exe: {e}"))?;
    let staged = sidecar(&exe, "new");
    std::fs::write(&staged, &bytes)
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
        exe,
        staged,
        version: latest,
    })
}

/// Download the release exe for `version` from `base` and verify it against
/// the release's published `SHA256SUMS.txt`, returning the verified bytes.
/// Split out from [`prepare`] so a localhost mock can exercise the full
/// download-and-verify path with no GitHub round-trip (see tests).
fn fetch_verified(agent: &ureq::Agent, base: &str, version: &str) -> Result<Vec<u8>, String> {
    let tag = format!("v{version}");
    let exe_name = release_asset_name(&tag);
    let exe_url = format!("{base}/{tag}/{exe_name}");
    let sums_url = format!("{base}/{tag}/SHA256SUMS.txt");

    // Checksum first - cheap, and it tells us what to expect.
    let sums = http_get_text(agent, &sums_url)?;
    let expected = parse_checksum(&sums, &exe_name)
        .ok_or_else(|| format!("no checksum for {exe_name} in SHA256SUMS.txt"))?;

    // Then the binary, which we verify before it ever lands at a runnable
    // path.
    let bytes = http_get_bytes(agent, &exe_url)?;
    let got = crate::sha256::hex(&bytes);
    if !got.eq_ignore_ascii_case(&expected) {
        return Err("downloaded file failed its checksum - aborting, no changes made".to_string());
    }
    Ok(bytes)
}

/// Swap the staged update into place and relaunch. Does not return on
/// success (the process exits). On a swap failure it logs to stderr and
/// returns, leaving the running version intact.
pub fn commit_and_relaunch(s: Staged) {
    if let Err(e) = swap_in_place(&s.exe, &s.staged) {
        eprintln!("self-update: swap failed, staying on the current version: {e}");
        return;
    }
    spawn_relauncher(&s.exe);
    // Hard exit so our ports + buffer-file handle release immediately; the
    // relauncher's short delay covers the OS freeing them before the new
    // process binds. Config was already persisted on every change.
    std::process::exit(0);
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

/// Relaunch the running exe and exit - the same mechanism as the update
/// swap, minus the swap. Backs the "Restart now" button (e.g. to apply a
/// buffer-size / port change that needs a fresh process). Does not return.
pub fn restart_now() -> ! {
    if let Ok(exe) = std::env::current_exe() {
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
#[cfg(not(windows))]
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
    use super::parse_checksum;

    const NAME: &str = "instantclone-v0.1.6-windows-x64.exe";

    /// The self-update asset name MUST match what release.yml publishes, or
    /// the checksum lookup can never find its line. This locks that contract
    /// per platform.
    #[test]
    fn release_asset_name_matches_release_workflow() {
        let n = super::release_asset_name("v1.2.3");
        #[cfg(windows)]
        assert_eq!(n, "instantclone-v1.2.3-windows-x64.exe");
        #[cfg(target_os = "linux")]
        assert_eq!(n, "instantclone-v1.2.3-linux-x64");
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
    use super::{fetch_verified, sidecar, swap_in_place};
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
        let dir = std::env::temp_dir().join(format!("ic-swap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("instantclone.exe");
        let staged = sidecar(&exe, "new");
        std::fs::write(&exe, b"OLD-VERSION").unwrap();
        std::fs::write(&staged, b"NEW-VERSION").unwrap();

        swap_in_place(&exe, &staged).unwrap();

        assert_eq!(std::fs::read(&exe).unwrap(), b"NEW-VERSION");
        assert_eq!(std::fs::read(sidecar(&exe, "old")).unwrap(), b"OLD-VERSION");
        assert!(!staged.exists(), "staged .new should have been consumed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
