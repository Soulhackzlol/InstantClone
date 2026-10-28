//! OBS Studio services.json patcher.
//!
//! OBS knows about streaming services through a `services.json` file
//! in the user's `AppData` directory. Each entry declares the server
//! URLs, recommended encoder settings, and - critically for our
//! use-case - the `multitrack_video_configuration_url` that turns on
//! the Enhanced Broadcasting / multi-track-video UI inside OBS.
//!
//! By inserting an InstantClone entry into that file we let the
//! streamer pick us straight from the OBS service dropdown - no
//! Custom-server fiddling, no manual JSON paste, multi-track Video
//! "Auto" buttons work because they hit our local config endpoint.
//!
//! No `serde_json` dependency - the file is well-formed JSON produced
//! by OBS, so a careful string-level insert + an idempotent check by
//! exact-substring match handles all the cases we care about. Every
//! write goes via a `.bak` copy first; if anything goes sideways the
//! user can restore manually.
//!
//! Config-dir resolution (services.json + logs both live under it):
//!   * Windows: `%APPDATA%\obs-studio` (or `%LOCALAPPDATA%`) and each known
//!     portable OBS's `<OBS folder>\config\obs-studio`, the open one first,
//!     otherwise the one used last.
//!   * Linux: `$XDG_CONFIG_HOME/obs-studio` (native / deb), then the Flatpak
//!     and Snap locations.
//!   * macOS: not handled yet (no candidates).

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// Candidate OBS config directories for this platform, in priority order.
/// Both `services.json` and the log folder are resolved relative to these,
/// so one list drives every OBS-path lookup.
pub(crate) fn obs_config_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        obs_installs_by_use()
            .into_iter()
            .map(|install| install.config_dir)
            .collect()
    }
    #[cfg(target_os = "linux")]
    {
        obs_config_dirs_from(
            std::env::var_os("HOME").map(PathBuf::from),
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        )
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Vec::new()
    }
}

/// Pure builder for the Linux OBS config-dir candidates - no env reads, so the
/// XDG / Flatpak / Snap layout is unit-testable. Empty when no home resolves.
#[cfg(target_os = "linux")]
fn obs_config_dirs_from(home: Option<PathBuf>, xdg: Option<PathBuf>) -> Vec<PathBuf> {
    let Some(home) = home else {
        return Vec::new();
    };
    let xdg = xdg
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    vec![
        xdg.join("obs-studio"), // native / deb / ppa
        home.join(".var/app/com.obsproject.Studio/config/obs-studio"), // flatpak
        home.join("snap/obs-studio/current/.config/obs-studio"), // snap
    ]
}

// ── Portable OBS (Windows) ──────────────────────────────────────────
//
// A portable OBS keeps its whole config inside its own folder instead of
// AppData, and nothing else on the machine records where that folder is.
// The running obs64.exe is the one thing that tells us, so the last one seen
// is remembered: registering needs OBS closed, and the next start of
// InstantClone may come before OBS's.

/// The installed-OBS config dirs under AppData.
#[cfg(windows)]
fn appdata_obs_dirs() -> Vec<PathBuf> {
    ["APPDATA", "LOCALAPPDATA"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .map(|base| PathBuf::from(base).join("obs-studio"))
        .collect()
}

/// Where a portable OBS keeps its config, relative to its own folder (the
/// one holding `bin\64bit\obs64.exe`). Unchanged through OBS 33.
#[cfg(windows)]
const PORTABLE_CONFIG_DIR: &str = "config\\obs-studio";

/// The OBS exe, relative to its own folder.
#[cfg(windows)]
const PORTABLE_EXE: &str = "bin\\64bit\\obs64.exe";

/// Files that switch OBS into portable mode when they sit in its folder,
/// as OBS's own obs-main.cpp checks them.
#[cfg(windows)]
const PORTABLE_MARKERS: [&str; 4] = [
    "portable_mode",
    "obs_portable_mode",
    "portable_mode.txt",
    "obs_portable_mode.txt",
];

/// How long one look at the running OBS is reused. A dashboard status poll
/// resolves several OBS paths, and each would otherwise walk the process list.
#[cfg(windows)]
const RUNNING_OBS_TTL: std::time::Duration = std::time::Duration::from_secs(2);

/// The file that remembers portable OBS folders between runs, one per line.
/// Unset in tests, where they are only remembered in memory.
#[cfg(windows)]
static PORTABLE_OBS_FILE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Every portable OBS folder seen running or added by hand, oldest first.
#[cfg(windows)]
static PORTABLE_OBS_ROOTS: crate::sync::Mutex<Vec<PathBuf>> = crate::sync::Mutex::new(Vec::new());

/// Name of the file, beside the settings, that remembers portable OBS folders.
#[cfg(windows)]
pub const PORTABLE_OBS_FILE_NAME: &str = "obs-portable-folder.txt";

/// Remember portable OBS folders in `file`, and load the ones it holds.
/// Called once at startup, before anything resolves an OBS path.
#[cfg(windows)]
pub fn remember_portable_obs_in(file: PathBuf) {
    let saved = fs::read_to_string(&file)
        .map(|text| parse_portable_roots(&text))
        .unwrap_or_default();
    *PORTABLE_OBS_ROOTS.lock() = saved;
    let _ = PORTABLE_OBS_FILE.set(file);
}

/// One folder per line; folders no longer holding a portable OBS are dropped.
#[cfg(windows)]
fn parse_portable_roots(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .filter(|root| has_own_config(root))
        .collect()
}

/// Every portable OBS known: the remembered ones and any running now, which
/// is remembered from here on.
#[cfg(windows)]
fn portable_obs_roots() -> Vec<PathBuf> {
    let mut known = PORTABLE_OBS_ROOTS.lock();
    let before = known.len();
    for root in running_obs_roots().into_iter().flatten() {
        if is_portable_obs_root(&root) && !known.contains(&root) {
            known.push(root);
        }
    }
    if known.len() != before {
        save_portable_obs_roots(&known);
    }
    known.retain(|root| has_own_config(root));
    known.clone()
}

/// Every OBS, the one whose settings the rest of this module reads (profile,
/// logs, WebSocket, the VOD track) first: one registered to this copy, then
/// an open one, then the one used last. A second OBS on the PC, or one a
/// copy for another channel streams from, must not answer for this one's.
#[cfg(windows)]
fn obs_installs_by_use() -> Vec<ObsInstall> {
    let mut installs = obs_installs();
    installs.sort_by_cached_key(|install| {
        (
            std::cmp::Reverse(is_registered_here(install)),
            std::cmp::Reverse(is_install_running(install)),
            std::cmp::Reverse(last_used(&install.config_dir)),
        )
    });
    installs
}

/// This copy's dashboard port, which our service entry in OBS names. 0 until
/// startup sets it (and in tests).
static OWN_WEB_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

/// Called at startup and when the dashboard port changes.
pub fn set_own_web_port(web_port: u16) {
    OWN_WEB_PORT.store(web_port, std::sync::atomic::Ordering::Relaxed);
}

/// Whether this OBS's InstantClone service points at this copy.
#[cfg(windows)]
fn is_registered_here(install: &ObsInstall) -> bool {
    let own = OWN_WEB_PORT.load(std::sync::atomic::Ordering::Relaxed);
    own != 0
        && fs::read_to_string(install.services_json())
            .ok()
            .and_then(|file| registered_ports(&file))
            .is_some_and(|(web, _)| web == Some(own))
}

/// When OBS with these settings last started: it writes a new log file into
/// `logs` each time, which touches the folder. None for one never started.
#[cfg(windows)]
fn last_used(config_dir: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(config_dir.join("logs"))
        .and_then(|meta| meta.modified())
        .ok()
}

/// Ask for a portable OBS's obs64.exe with Windows' own Open dialog, for one
/// that hasn't been open while InstantClone ran, and remember its folder.
/// With `replacing` (an install's settings folder), the pick takes that
/// one's place: the same OBS, moved. None when the dialog is cancelled.
/// Blocks until the dialog closes.
#[cfg(windows)]
pub fn pick_portable_obs(replacing: Option<&Path>) -> Option<io::Result<PathBuf>> {
    let picked = pick_portable_exe()?;
    Some(add_portable_obs(&picked).inspect(|root| {
        if let Some(old) = replacing.filter(|old| *old != root.join(PORTABLE_CONFIG_DIR)) {
            drop_portable_root(old);
        }
    }))
}

/// Forget a portable OBS (by its settings folder), taking InstantClone out of
/// its service list first so nothing is left pointing here. Refused while
/// that OBS runs: it rewrites its list on quit, and would be found again.
#[cfg(windows)]
pub fn forget_portable_obs(config_dir: &Path) -> io::Result<()> {
    let install = obs_installs()
        .into_iter()
        .find(|i| i.portable && i.config_dir == config_dir)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "That OBS isn't in the list."))?;
    if is_install_running(&install) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "That OBS is open. Close it first.",
        ));
    }
    if fs::read_to_string(install.services_json()).is_ok_and(|file| entry_exists(&file)) {
        unregister_in(&install.services_json())?;
    }
    drop_portable_root(config_dir);
    Ok(())
}

/// Take the portable OBS with this settings folder off the remembered list.
#[cfg(windows)]
fn drop_portable_root(config_dir: &Path) {
    let mut known = PORTABLE_OBS_ROOTS.lock();
    known.retain(|root| root.join(PORTABLE_CONFIG_DIR) != config_dir);
    save_portable_obs_roots(&known);
}

#[cfg(not(windows))]
pub fn forget_portable_obs(_config_dir: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "portable OBS is Windows-only",
    ))
}

/// The obs64.exe picked in the Open dialog; None when cancelled or when
/// another pick is already on screen (that one answers instead).
#[cfg(windows)]
fn pick_portable_exe() -> Option<PathBuf> {
    static PICKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if PICKING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return None;
    }
    // A thread of its own: the shell's dialog wants a single-threaded COM
    // apartment, which a pool thread may already be set up otherwise.
    let picked = std::thread::spawn(open_dialog::pick_obs_exe)
        .join()
        .ok()
        .flatten();
    PICKING.store(false, std::sync::atomic::Ordering::Release);
    picked
}

/// Portable OBS is a Windows build; elsewhere OBS keeps its settings in the
/// home folder, found on its own.
#[cfg(not(windows))]
pub fn pick_portable_obs(_replacing: Option<&Path>) -> Option<io::Result<PathBuf>> {
    Some(Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "portable OBS is Windows-only",
    )))
}

/// Remember the portable OBS that `exe` belongs to. Returns its folder.
#[cfg(windows)]
fn add_portable_obs(exe: &Path) -> io::Result<PathBuf> {
    let root = obs_root_of_exe(exe).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "That isn't OBS's own obs64.exe. Pick the one in the portable OBS's bin\\64bit folder.",
        )
    })?;
    if !has_own_config(&root) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "That OBS isn't portable: it keeps its settings in AppData, so it's the installed OBS \
             in the list. If it's a new portable OBS, open it once first.",
        ));
    }
    let mut known = PORTABLE_OBS_ROOTS.lock();
    if !known.contains(&root) {
        known.push(root.clone());
        save_portable_obs_roots(&known);
    }
    Ok(root)
}

/// Windows' standard Open dialog, shown from the dashboard. The click that
/// asked for it left the browser in front, and Windows lets only the
/// foreground app put a window over it, so the dialog hangs off an
/// invisible topmost owner that is brought forward first.
#[cfg(windows)]
mod open_dialog {
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_DONTADDTORECENT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, CreateWindowExW, DestroyWindow, GetForegroundWindow, GetSystemMetrics,
        GetWindowThreadProcessId, SetForegroundWindow, ShowWindow, SM_CXSCREEN, SM_CYSCREEN,
        SW_SHOWNA, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };

    /// Room for the picked path, in UTF-16 units (Windows' long-path limit).
    const PATH_CAPACITY: usize = 32_768;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The obs64.exe the streamer picks, or None when they cancel.
    pub fn pick_obs_exe() -> Option<PathBuf> {
        let title = wide("Pick the portable OBS: its bin\\64bit\\obs64.exe");
        // Pairs of label and pattern, each ending in a NUL, the list in two.
        let filter: Vec<u16> = "OBS Studio (obs64.exe)\0obs64.exe\0\0"
            .encode_utf16()
            .collect();
        let class = wide("STATIC");
        let mut path = vec![0u16; PATH_CAPACITY];
        let picked = unsafe {
            CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
            let owner = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                GetSystemMetrics(SM_CXSCREEN) / 2,
                GetSystemMetrics(SM_CYSCREEN) / 3,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            bring_forward(owner);
            let mut dialog: OPENFILENAMEW = std::mem::zeroed();
            dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
            dialog.hwndOwner = owner;
            dialog.lpstrFilter = filter.as_ptr();
            dialog.lpstrFile = path.as_mut_ptr();
            dialog.nMaxFile = PATH_CAPACITY as u32;
            dialog.lpstrTitle = title.as_ptr();
            dialog.Flags = OFN_EXPLORER
                | OFN_FILEMUSTEXIST
                | OFN_PATHMUSTEXIST
                | OFN_NOCHANGEDIR
                | OFN_DONTADDTORECENT;
            let picked = GetOpenFileNameW(&mut dialog) != 0;
            if !owner.is_null() {
                DestroyWindow(owner);
            }
            CoUninitialize();
            picked
        };
        if !picked {
            return None;
        }
        let end = path.iter().position(|&c| c == 0).unwrap_or(path.len());
        Some(PathBuf::from(String::from_utf16_lossy(&path[..end])))
    }

    /// Put `window` in front. Windows only takes that from the app in
    /// front, so for that moment this thread joins its input.
    unsafe fn bring_forward(window: HWND) {
        if window.is_null() {
            return;
        }
        ShowWindow(window, SW_SHOWNA);
        let front = GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut());
        let ours = GetCurrentThreadId();
        let joined = front != 0 && front != ours && AttachThreadInput(ours, front, 1) != 0;
        SetForegroundWindow(window);
        BringWindowToTop(window);
        if joined {
            AttachThreadInput(ours, front, 0);
        }
    }
}

#[cfg(windows)]
fn save_portable_obs_roots(roots: &[PathBuf]) {
    let Some(file) = PORTABLE_OBS_FILE.get() else {
        return;
    };
    let text: String = roots
        .iter()
        .map(|root| format!("{}\n", root.display()))
        .collect();
    if let Err(e) = fs::write(file, text) {
        // Only costs the next start: it finds the folder again once OBS runs.
        log_line!(
            "could not remember the portable OBS folders in {}: {e}",
            file.display()
        );
    }
}

/// The folder of each running OBS, looked up at most once per
/// `RUNNING_OBS_TTL`. `None` stands for an OBS whose path can't be read
/// (one running elevated). Streamlabs' engine is called obs64.exe too, but
/// it never sits in `bin\64bit`, so it is no OBS here.
#[cfg(windows)]
fn running_obs_roots() -> Vec<Option<PathBuf>> {
    type Seen = (std::time::Instant, Vec<Option<PathBuf>>);
    static CACHE: crate::sync::Mutex<Option<Seen>> = crate::sync::Mutex::new(None);
    let mut cache = CACHE.lock();
    if let Some((seen_at, roots)) = cache.as_ref() {
        if seen_at.elapsed() < RUNNING_OBS_TTL {
            return roots.clone();
        }
    }
    let roots: Vec<Option<PathBuf>> = obs_process_ids()
        .into_iter()
        .filter_map(|pid| match crate::portcheck::process_name(pid) {
            Some(exe) => obs_root_of_exe(Path::new(&exe)).map(Some),
            None => Some(None),
        })
        .collect();
    *cache = Some((std::time::Instant::now(), roots.clone()));
    roots
}

/// OBS's own folder from its exe path, `<root>\bin\64bit\obs64.exe`. None
/// for an exe laid out any other way.
#[cfg(windows)]
fn obs_root_of_exe(exe: &Path) -> Option<PathBuf> {
    let named = |dir: &Path, name: &str| {
        dir.file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
    };
    let bit64 = exe.parent()?;
    let bin = bit64.parent()?;
    if !named(bit64, "64bit") || !named(bin, "bin") {
        return None;
    }
    bin.parent().map(Path::to_path_buf)
}

/// Whether `root` holds an OBS settings folder of its own, as only a
/// portable OBS does.
#[cfg(windows)]
fn has_own_config(root: &Path) -> bool {
    root.join(PORTABLE_CONFIG_DIR).is_dir()
}

/// Whether OBS in `root` runs portable, by OBS's own rule: a marker file in
/// its folder, or the `--portable` flag. The flag leaves no trace, so a
/// config folder with no AppData OBS beside it counts as portable too. An
/// installed OBS has no config folder of its own and never matches.
#[cfg(windows)]
fn is_portable_obs_root(root: &Path) -> bool {
    if !has_own_config(root) {
        return false;
    }
    PORTABLE_MARKERS
        .iter()
        .any(|marker| root.join(marker).exists())
        || !appdata_obs_dirs().iter().any(|dir| dir.is_dir())
}

/// Locate the user's `services.json`. Returns `None` when no candidate path
/// exists - typical when OBS isn't installed at all.
pub fn services_json_path() -> Option<PathBuf> {
    obs_config_dirs()
        .into_iter()
        .map(|d| services_json_in(&d))
        .find(|p| p.exists())
}

fn services_json_in(config_dir: &Path) -> PathBuf {
    config_dir.join("plugin_config/rtmp-services/services.json")
}

// ── Every OBS on this machine ───────────────────────────────────────
//
// A PC can hold an installed OBS and any number of portable ones, each with
// its own settings and its own service list. Registering is per OBS, the
// streamer picks which, and "close OBS first" only stops the one that's open.

/// One OBS the streamer can register InstantClone in.
#[derive(Clone, Debug, PartialEq)]
pub struct ObsInstall {
    /// Where this OBS keeps its settings; its service list lives under it.
    /// Also the install's id in the dashboard.
    pub config_dir: PathBuf,
    /// OBS's own folder (the one holding `bin\64bit\obs64.exe`), when known.
    pub root: Option<PathBuf>,
    pub portable: bool,
}

impl ObsInstall {
    pub fn services_json(&self) -> PathBuf {
        services_json_in(&self.config_dir)
    }
}

/// The installed OBS (its AppData settings) and every known portable OBS.
#[cfg(windows)]
pub fn obs_installs() -> Vec<ObsInstall> {
    let installed_root = obs_executable_candidates()
        .into_iter()
        .find(|exe| exe.exists())
        .and_then(|exe| obs_root_of_exe(&exe));
    let mut installs: Vec<ObsInstall> = appdata_obs_dirs()
        .into_iter()
        .filter(|dir| dir.is_dir())
        .map(|config_dir| ObsInstall {
            config_dir,
            root: installed_root.clone(),
            portable: false,
        })
        .collect();
    installs.extend(portable_obs_roots().into_iter().map(|root| ObsInstall {
        config_dir: root.join(PORTABLE_CONFIG_DIR),
        root: Some(root),
        portable: true,
    }));
    installs
}

/// Each OBS settings folder present (native, Flatpak, Snap).
#[cfg(not(windows))]
pub fn obs_installs() -> Vec<ObsInstall> {
    obs_config_dirs()
        .into_iter()
        .filter(|dir| dir.is_dir())
        .map(|config_dir| ObsInstall {
            config_dir,
            root: None,
            portable: false,
        })
        .collect()
}

/// Whether this OBS is open. A portable one is matched by its folder; any
/// other running OBS counts as the installed one. An OBS whose folder can't
/// be read counts as every one, the safe side: a list written under a running
/// OBS is overwritten when it quits.
#[cfg(windows)]
pub fn is_install_running(install: &ObsInstall) -> bool {
    let portables = portable_obs_roots();
    running_obs_roots().iter().any(|running| match running {
        None => true,
        Some(root) if install.portable => install.root.as_ref() == Some(root),
        Some(root) => !portables.contains(root),
    })
}

#[cfg(not(windows))]
pub fn is_install_running(_install: &ObsInstall) -> bool {
    is_obs_running()
}

/// Where one OBS's service list stands.
#[derive(Debug, PartialEq, Eq)]
pub enum Registration {
    /// No service list yet: OBS hasn't been opened since it was installed.
    Missing,
    NotRegistered,
    /// Registered with the ports and URLs this InstantClone uses.
    Current,
    /// Registered, but pointing somewhere OBS can't reach us any more.
    Stale,
    /// Registered to another InstantClone that is running (its web port,
    /// 0 when unknown).
    OtherInstance(u16),
}

impl Registration {
    pub fn as_str(&self) -> &'static str {
        match self {
            Registration::Missing => "missing",
            Registration::NotRegistered => "none",
            Registration::Current => "current",
            Registration::Stale => "stale",
            Registration::OtherInstance(_) => "other",
        }
    }
}

pub fn registration_of(install: &ObsInstall, web_port: u16, ingest_port: u16) -> Registration {
    let Ok(file) = fs::read_to_string(install.services_json()) else {
        return Registration::Missing;
    };
    if !entry_exists(&file) {
        return Registration::NotRegistered;
    }
    if registration_is_current(&file, web_port, ingest_port) {
        return Registration::Current;
    }
    match other_instance_port(
        &file,
        web_port,
        ingest_port,
        crate::portcheck::port_held_by_other_instance,
    ) {
        Some(port) => Registration::OtherInstance(port),
        None => Registration::Stale,
    }
}

/// True when services.json holds an InstantClone entry that matches the
/// ports and URLs we would write today. Used by the System tab to render
/// the button as "Unregister" instead of "Register".
///
/// Deliberately stricter than "an entry with our name exists". An entry
/// written by an older build, or before the user retuned a port, still
/// carries our name while pointing OBS somewhere useless, and reporting
/// that as registered leaves the user with no way to notice or fix it.
/// Reporting it as unregistered puts the Register button back, and
/// `register_in` is remove-and-replace, so one click repairs it.
/// Repair a registration the user already made, when a new build or a
/// retuned port has left it pointing somewhere OBS cannot use.
///
/// Returns None when there is nothing to do, or `Some(result)` when a repair
/// was attempted.
///
/// Two rules keep this from being a surprise:
///
/// - Only an entry that is already there. Someone who never pressed Register
///   has not asked us to write into OBS's `services.json`, and an unregister
///   removes the entry, so it stays removed.
/// - Only while OBS is closed. OBS holds `services.json` in memory and
///   rewrites it on exit, so repairing underneath a running OBS is undone a
///   moment later and leaves the user worse off than not trying: the button
///   would read registered while the file on disk says otherwise.
///
/// Everything else is the existing `register_in`, which is remove-and-replace,
/// so this is exactly the click the dashboard would have asked for.
pub fn refresh_stale_registration(web_port: u16, ingest_port: u16) -> RegistrationRefresh {
    let verdicts = obs_installs()
        .iter()
        .map(|install| refresh_install(install, web_port, ingest_port))
        .collect();
    let verdict = most_pressing(verdicts);
    REFRESH_WAITING.store(
        verdict == RegistrationRefresh::Deferred,
        std::sync::atomic::Ordering::Relaxed,
    );
    verdict
}

/// `refresh_stale_registration` for one OBS: only its own entry, and only
/// while that OBS is closed.
fn refresh_install(install: &ObsInstall, web_port: u16, ingest_port: u16) -> RegistrationRefresh {
    let path = install.services_json();
    let Ok(file) = fs::read_to_string(&path) else {
        return RegistrationRefresh::NotNeeded;
    };
    // The decision is `refresh_decision`, so what the tests pin is what runs.
    match refresh_decision(
        &file,
        web_port,
        ingest_port,
        is_install_running(install),
        crate::portcheck::port_held_by_other_instance,
    ) {
        RegistrationRefresh::Repaired => match register_in(&path, web_port, ingest_port) {
            Ok(()) => RegistrationRefresh::Repaired,
            Err(e) => RegistrationRefresh::Failed(e.to_string()),
        },
        verdict => verdict,
    }
}

/// The one verdict the caller acts on across several OBS: a wait first (the
/// caller comes back for it), then a failure, a repair, another instance.
fn most_pressing(verdicts: Vec<RegistrationRefresh>) -> RegistrationRefresh {
    let rank = |v: &RegistrationRefresh| match v {
        RegistrationRefresh::Deferred => 4,
        RegistrationRefresh::Failed(_) => 3,
        RegistrationRefresh::Repaired => 2,
        RegistrationRefresh::OtherInstance(_) => 1,
        RegistrationRefresh::NotNeeded => 0,
    };
    verdicts
        .into_iter()
        .max_by_key(rank)
        .unwrap_or(RegistrationRefresh::NotNeeded)
}

/// Set while the startup check is waiting for OBS to close to refresh an
/// out-of-date entry, so the OBS tab can say it updates by itself.
static REFRESH_WAITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn refresh_waiting() -> bool {
    REFRESH_WAITING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether services.json holds an InstantClone entry at all, current or not.
pub fn has_entry() -> bool {
    services_json_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .is_some_and(|file| entry_exists(&file))
}

/// What `refresh_stale_registration` did, and whether coming back later would
/// help. `Deferred` is the interesting one: the entry does need repairing,
/// but OBS is open, and OBS rewrites `services.json` from memory when it
/// exits - so the repair has to wait for it to close.
#[derive(Debug, PartialEq, Eq)]
pub enum RegistrationRefresh {
    /// No entry of ours, or the one there is already right. Nothing to do,
    /// now or later.
    NotNeeded,
    /// Wanted, but OBS holds the file. Worth trying again when it closes.
    Deferred,
    Repaired,
    Failed(String),
    /// The entry points at another InstantClone that is running right now
    /// (a second copy from another folder, with its own ports). It is not
    /// stale, it is someone else's, so it is left alone. Carries that copy's
    /// dashboard port.
    OtherInstance(u16),
}

/// Should we repair, wait, or leave it alone? Split out so it can be tested
/// without an OBS install; `Repaired` here means "a repair is warranted", and
/// the caller downgrades it to `Failed` if writing the file does not work.
///
/// `other_instance_on` answers whether another running InstantClone listens
/// on a port. Only asked about the ports an out-of-date entry names, so the
/// usual startup with a current entry never looks at the process table.
fn refresh_decision(
    file: &str,
    web_port: u16,
    ingest_port: u16,
    obs_running: bool,
    other_instance_on: impl Fn(u16) -> bool,
) -> RegistrationRefresh {
    if !entry_exists(file) || registration_is_current(file, web_port, ingest_port) {
        return RegistrationRefresh::NotNeeded;
    }
    if let Some(port) = other_instance_port(file, web_port, ingest_port, other_instance_on) {
        return RegistrationRefresh::OtherInstance(port);
    }
    if obs_running {
        return RegistrationRefresh::Deferred;
    }
    RegistrationRefresh::Repaired
}

/// The dashboard port of the other running InstantClone our entry points at,
/// if it points at one. An entry naming our own ports is ours (stale or not);
/// one naming ports nobody else holds is a leftover from before a port change.
fn other_instance_port(
    file: &str,
    web_port: u16,
    ingest_port: u16,
    other_instance_on: impl Fn(u16) -> bool,
) -> Option<u16> {
    let (their_web, their_ingest) = registered_ports(file)?;
    let held =
        |port: Option<u16>, ours: u16| port.is_some_and(|p| p != ours && other_instance_on(p));
    (held(their_web, web_port) || held(their_ingest, ingest_port)).then(|| their_web.unwrap_or(0))
}

/// The (dashboard, RTMP) ports our entry in `file` points OBS at, read from
/// its multi-track config URL and its server URL. Either may be missing in
/// a hand-edited entry; None when there is no entry at all.
fn registered_ports(file: &str) -> Option<(Option<u16>, Option<u16>)> {
    let entry = &file[entry_span(file)?];
    let port_after = |prefix: &str| -> Option<u16> {
        let rest = &entry[entry.find(prefix)? + prefix.len()..];
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        rest[..digits].parse().ok()
    };
    let web = port_after("http://127.0.0.1:");
    let ingest = ["rtmp://localhost:", "rtmp://127.0.0.1:", "rtmp://[::1]:"]
        .iter()
        .find_map(|prefix| port_after(prefix));
    Some((web, ingest))
}

/// When our entry in OBS points at another InstantClone running right now,
/// that copy's dashboard port (0 if the entry names no dashboard port). For
/// the OBS tab: pressing Register here would take OBS away from that copy.
pub fn registered_to_other_instance(web_port: u16, ingest_port: u16) -> Option<u16> {
    let file = fs::read_to_string(services_json_path()?).ok()?;
    if !entry_exists(&file) || registration_is_current(&file, web_port, ingest_port) {
        return None;
    }
    other_instance_port(
        &file,
        web_port,
        ingest_port,
        crate::portcheck::port_held_by_other_instance,
    )
}

pub fn is_registered(web_port: u16, ingest_port: u16) -> bool {
    let Some(p) = services_json_path() else {
        return false;
    };
    let Ok(file) = fs::read_to_string(&p) else {
        return false;
    };
    entry_exists(&file) && registration_is_current(&file, web_port, ingest_port)
}

/// Whether the entry in `file` still matches what `entry_json` produces.
///
/// Checks the two URLs that decide whether OBS can reach us, the RTMP
/// server and the multi-track config endpoint. Both carry a port, so a
/// retuned port shows up here too.
///
/// Leftover literal servers inside OUR entry also count as stale: they
/// would sit in the dropdown as options that fail under one IP Family
/// setting each. So does an entry from before HEVC and AV1 were offered:
/// OBS would switch an AV1 encoder to H.264 for it.
///
/// Every check runs against `entry_span` and not the whole file. Scanning
/// the file would let anything InstantClone-shaped elsewhere in
/// services.json hold the answer at "stale" permanently, and since
/// re-registering only rewrites our own entry, the user would be stuck
/// with a Register button that never turns into Unregister no matter how
/// many times they press it.
fn registration_is_current(file: &str, web_port: u16, ingest_port: u16) -> bool {
    let Some(span) = entry_span(file) else {
        return false;
    };
    let entry = &file[span];
    let stale_literals = [
        format!("rtmp://127.0.0.1:{ingest_port}/live"),
        format!("rtmp://[::1]:{ingest_port}/live"),
    ];
    entry.contains(&server_url(ingest_port))
        && entry.contains(&multitrack_url(web_port))
        && entry.contains(r#""av1""#)
        && !stale_literals.iter().any(|u| entry.contains(u.as_str()))
}

/// The RTMP URL we advertise to OBS.
///
/// `localhost`, not a literal, and that is the whole point. OBS's
/// Settings -> Advanced -> Network -> IP Family is a resolver setting: it
/// picks the address family passed to `getaddrinfo`. Under IP Family =
/// IPv6 an IPv4 literal cannot resolve (`getaddrinfo("127.0.0.1",
/// AF_INET6)` returns host-not-found), and under IPv4 an IPv6 literal
/// cannot either. `localhost` is the one spelling that resolves under all
/// three settings, so a single entry covers every configuration and the
/// user never has to know the setting exists.
///
/// This is safe only because the ingest binds both families (see
/// `Settings::ingest_addrs`). Under IPv4+IPv6 OBS resolves both `::1` and
/// `127.0.0.1` and races them; happy-eyeballs reports failure only when
/// every candidate fails, so even if our IPv6 leg could not bind, the
/// IPv4 leg still wins the race.
fn server_url(ingest_port: u16) -> String {
    format!("rtmp://localhost:{ingest_port}/live")
}

fn multitrack_url(web_port: u16) -> String {
    format!("http://127.0.0.1:{web_port}/obs/multitrack-config")
}

/// Build the services.json entry for InstantClone. The
/// `multitrack_video_configuration_url` is what OBS hits when the user
/// enables multi-track video - we serve it from our own web port.
///
/// `"name"` MUST stay the first key: `remove_entry` walks backwards from
/// the name marker to the nearest `{` without tracking strings, so it only
/// finds our object's opening brace while nothing precedes the name.
///
/// One server entry, on `localhost`. See `server_url` for why that
/// spelling is the only one that works under every OBS IP Family setting,
/// and why a single entry beats offering the user a choice.
fn entry_json(web_port: u16, ingest_port: u16) -> String {
    format!(
        r#"{{
            "name": "InstantClone",
            "common": true,
            "more_info_link": "https://github.com/Soulhackzlol/InstantClone",
            "stream_key_link": "http://127.0.0.1:{web}/",
            "multitrack_video_configuration_url": "{mtv}",
            "multitrack_video_name": "Multi-track via InstantClone",
            "multitrack_video_learn_more_link": "https://github.com/Soulhackzlol/InstantClone",
            "servers": [
                {{
                    "name": "InstantClone (local proxy)",
                    "url": "{server}"
                }}
            ],
            "recommended": {{
                "keyint": 2,
                "profile": "main",
                "max video bitrate": 50000,
                "max audio bitrate": 320
            }},
            "supported video codecs": ["h264", "hevc", "av1"]
        }}"#,
        web = web_port,
        mtv = multitrack_url(web_port),
        server = server_url(ingest_port),
    )
}

/// Check whether an "InstantClone" entry is present anywhere in
/// services.json. Exact-substring match on the name key - services.json
/// is OBS-generated and stable enough that this won't false-positive on
/// e.g. a "FooInstantCloneBar" service name (no such service exists in
/// the upstream services.json).
fn entry_exists(file: &str) -> bool {
    file.contains(r#""name": "InstantClone""#) || file.contains(r#""name":"InstantClone""#)
}

/// Register InstantClone in one OBS's service list (`path`). Idempotent and
/// self-healing - a pre-existing entry is removed and replaced so a changed
/// web_port (e.g. user retuned the dashboard port in System settings)
/// refreshes the URL OBS will hit. Returns an `io::Error` with a
/// user-readable message when something goes wrong, so the UI can surface it.
pub fn register_in(path: &Path, web_port: u16, ingest_port: u16) -> io::Result<()> {
    let original = fs::read_to_string(path).map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "this OBS has no service list yet - open it once, close it, then register",
        )
    })?;
    let patched = with_entry_registered(&original, web_port, ingest_port)?;
    let bak = path.with_extension("json.instantclone.bak");
    write_or_friendly(&bak, &original)?;
    write_or_friendly(path, &patched)?;
    Ok(())
}

/// `register_in`'s whole edit of services.json, without the disk. Pure so the
/// properties that matter (idempotent, everything else byte-for-byte
/// untouched, never a second entry) are unit-testable.
fn with_entry_registered(original: &str, web_port: u16, ingest_port: u16) -> io::Result<String> {
    looks_like_services_json(original)?;
    // If our entry is already there, strip it first so a port-changed
    // re-register actually refreshes the URL. When it cannot be cut out
    // cleanly, refuse: inserting anyway would leave OBS two entries.
    let base = if entry_exists(original) {
        remove_entry(original).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "couldn't cleanly replace the existing InstantClone entry in services.json - \
                 restore it from services.json.instantclone.bak or let OBS regenerate it",
            )
        })?
    } else {
        original.to_string()
    };
    insert_entry(&base, &entry_json(web_port, ingest_port)).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "couldn't locate the `\"services\":[` array in services.json - file shape unexpected",
        )
    })
}

/// Remove our entry from one OBS's service list (`path`). Returns Ok(())
/// even when not registered (idempotent) - only surfaces an error if the
/// file is present but in an unexpected shape we can't safely edit.
pub fn unregister_in(path: &Path) -> io::Result<()> {
    let original = fs::read_to_string(path)?;
    if !entry_exists(&original) {
        return Ok(()); // nothing to do
    }
    looks_like_services_json(&original)?;
    let stripped = remove_entry(&original).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "couldn't unambiguously remove the InstantClone entry - restore from services.json.instantclone.bak manually",
        )
    })?;
    let bak = path.with_extension("json.instantclone.bak");
    write_or_friendly(&bak, &original)?;
    write_or_friendly(path, &stripped)?;
    Ok(())
}

/// Reject a services.json that doesn't at least look like the expected
/// shape - a JSON object containing a `"services"` array. Spares the
/// user a half-corrupted file when something else (a botched manual
/// edit, a different program writing to the same path) trashed it
/// since OBS last touched it.
fn looks_like_services_json(file: &str) -> io::Result<()> {
    let trimmed = file.trim_start();
    if !trimmed.starts_with('{') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "services.json doesn't start with a JSON object - file may be corrupted, close OBS and run it once to regenerate it.",
        ));
    }
    if !file.contains("\"services\"") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "services.json is missing the `services` array - file looks unfamiliar, restore from services.json.instantclone.bak or let OBS regenerate it.",
        ));
    }
    Ok(())
}

/// Wrap `fs::write` with a friendlier error when Windows reports the
/// file is locked by another process - usually OBS holding it open.
fn write_or_friendly(path: &std::path::Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    match fs::write(path, contents) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Windows: ERROR_ACCESS_DENIED (5) and ERROR_SHARING_VIOLATION
            // (32) are the two ways OBS-holding-the-file shows up.
            // PermissionDenied catches the unix-style mapping of those.
            let locked = matches!(e.raw_os_error(), Some(5) | Some(32))
                || e.kind() == io::ErrorKind::PermissionDenied;
            if locked {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "the OBS config file is locked - close OBS Studio first, then try again.",
                ))
            } else {
                Err(e)
            }
        }
    }
}

/// Splice `entry` into the `"services":[ … ]` array as the first
/// element. Returns `None` if the array marker can't be found -
/// services.json is OBS-managed so this should always succeed for an
/// install we can trust, but we hand back `None` rather than corrupt
/// the file if anything looks off.
fn insert_entry(file: &str, entry: &str) -> Option<String> {
    // Find the array opening bracket after the `"services"` key. OBS
    // formats with two-space indent and no leading whitespace at the
    // bracket, but we tolerate optional whitespace just in case.
    let key = "\"services\"";
    let key_pos = file.find(key)?;
    let after_key = &file[key_pos + key.len()..];
    // Skip whitespace + colon + whitespace until the `[`.
    let bracket_offset = after_key.find('[')?;
    let absolute_bracket = key_pos + key.len() + bracket_offset;
    let (head, tail) = file.split_at(absolute_bracket + 1);
    // Insert our entry followed by a comma so the existing first
    // service stays valid JSON; an empty list gets no comma.
    let separator = if tail.trim_start().starts_with(']') {
        ""
    } else {
        ","
    };
    Some(format!("{head}\n{entry}{separator}{tail}"))
}

/// Byte range of our service object in `file`, braces included.
///
/// Scoping matters: the rest of the file belongs to OBS and to other
/// services, and a stray `InstantClone`-flavoured string elsewhere (a
/// leftover server object inside another service's `servers` array, say)
/// must not be read as ours. Anything that inspects "our entry" has to go
/// through here rather than searching the whole file.
fn entry_span(file: &str) -> Option<std::ops::Range<usize>> {
    // Locate the marker (handles both indented and non-indented
    // formats). The trailing quote is what keeps this from matching a
    // nested `"name": "InstantClone (local proxy)"` server object.
    let marker = if file.contains(r#""name": "InstantClone""#) {
        r#""name": "InstantClone""#
    } else {
        r#""name":"InstantClone""#
    };
    let marker_pos = file.find(marker)?;

    // One string-aware pass, tracking the stack of open braces, so the
    // object we return is the one that actually encloses the marker.
    // Walking backwards to the nearest `{` instead would land inside a
    // string the moment a key ordered before `name` carried one - a
    // `url_template` holding `{stream_key}` is exactly that shape - and
    // brace-match from there into a span that is not our entry at all.
    let bytes = file.as_bytes();
    let mut open_objects: Vec<usize> = Vec::new();
    let mut entry_start: Option<usize> = None;
    let mut in_str = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        match b {
            b'\\' if in_str => escape = true,
            b'"' => in_str = !in_str,
            b'{' if !in_str => open_objects.push(i),
            b'}' if !in_str => {
                // Unbalanced: this is not JSON we should be editing.
                let opened = open_objects.pop()?;
                if entry_start == Some(opened) {
                    return Some(opened..i + 1);
                }
            }
            _ => {}
        }
        if entry_start.is_none() && i == marker_pos {
            entry_start = Some(*open_objects.last()?);
        }
    }
    None
}

/// Delete our entry plus any preceding/trailing comma so the surrounding
/// array stays valid.
fn remove_entry(file: &str) -> Option<String> {
    let span = entry_span(file)?;
    let (start, end) = (span.start, span.end);
    let bytes = file.as_bytes();
    // Also strip the trailing comma + whitespace if our entry isn't
    // the last item, or the leading comma if it is.
    let mut left = start;
    let mut right = end;
    // Skip whitespace right of `end` looking for a `,`.
    let mut probe = end;
    while probe < bytes.len() && bytes[probe].is_ascii_whitespace() {
        probe += 1;
    }
    if probe < bytes.len() && bytes[probe] == b',' {
        right = probe + 1;
        // Take the whitespace before the entry too. The whitespace after
        // the comma stays for the next element, so leaving this run as
        // well would add a blank line on every re-register.
        while left > 0 && bytes[left - 1].is_ascii_whitespace() {
            left -= 1;
        }
    } else {
        // No trailing comma - try eating a leading one instead so the
        // remaining array doesn't end with a stray `,]`.
        let mut probe = start;
        while probe > 0 && bytes[probe - 1].is_ascii_whitespace() {
            probe -= 1;
        }
        if probe > 0 && bytes[probe - 1] == b',' {
            left = probe - 1;
        }
    }
    Some(format!("{}{}", &file[..left], &file[right..]))
}

// ── VOD-audio integration: OBS user.ini flag ────────────────────────
//
// OBS gates the VOD Track checkbox behind `EnableCustomServerVodTrack`
// in `[General]` of the user config. The frontend reads it as:
//
//   bool enableForCustomServer = config_get_bool(
//       App()->GetUserConfig(), "General", "EnableCustomServerVodTrack");
//   bool enableVodTrack = ui->service->currentText() == "Twitch";
//   if (enableForCustomServer && IsCustomService())
//       enableVodTrack = true;
//
// (frontend/settings/OBSBasicSettings_Stream.cpp on master)
//
// IMPORTANT: OBS 32 split the legacy global.ini into two files -
// `global.ini` (app-level) and `user.ini` (user-level) - and
// `GetUserConfig()` now returns user.ini. v0.1.0..0.1.2 wrote the
// flag to global.ini, which OBS 32 simply ignored. Issue #9 was the
// streamer report ("the bypass didn't work") that surfaced this.
//
// Strategy: prefer user.ini when it exists (OBS 32+), fall back to
// global.ini for older installs. On every write we also strip the
// legacy key from global.ini if it's there, so upgraders don't carry
// dead state across files.
//
// We write/remove this flag in lockstep with the per-destination
// `vod_audio` toggle so the streamer doesn't have to hand-edit an INI
// file. The flag is global to the user's OBS - it affects every
// Custom RTMP setup they have, not just InstantClone. That's
// deliberate on OBS's side: they only want power users who understand
// the implication enabling this, which is exactly what our dashboard
// toggle's copy spells out.

/// Pure path-resolution helper: given an OBS install directory
/// (typically `%APPDATA%\obs-studio`), pick the authoritative user
/// config file - user.ini on OBS 32+, falling back to global.ini on
/// older installs that haven't been split. Separated from
/// `obs_user_config_path` so tests can exercise the prefer-user.ini
/// logic with a temp directory instead of mutating real %APPDATA%.
fn resolve_user_config_in(obs_dir: &Path) -> Option<PathBuf> {
    let user = obs_dir.join("user.ini");
    if user.exists() {
        return Some(user);
    }
    let global = obs_dir.join("global.ini");
    global.exists().then_some(global)
}

/// Pure helper for the cleanup-only path that resolves the legacy
/// global.ini under an OBS install directory, but ONLY when user.ini
/// is present alongside (i.e. OBS 32+, where global.ini's
/// `EnableCustomServerVodTrack` is dead state to strip). Returns None
/// on older installs where global.ini is still the live config.
fn legacy_global_ini_in(obs_dir: &Path) -> Option<PathBuf> {
    let user = obs_dir.join("user.ini");
    if !user.exists() {
        return None;
    }
    let global = obs_dir.join("global.ini");
    global.exists().then_some(global)
}

/// Authoritative user-config path: user.ini in OBS 32+, falling back
/// to global.ini for pre-32 installs that haven't been split yet.
/// `[Basic] Profile=` lives in this file too, so the active-profile
/// detection below shares the same lookup.
fn obs_user_config_path() -> Option<PathBuf> {
    // Walk the same cross-platform candidate dirs as services.json (Windows
    // APPDATA/LOCALAPPDATA, Linux XDG/Flatpak/Snap). The old %APPDATA%-only
    // lookup returned None on Linux, silently no-op'ing the VOD-audio flag.
    obs_config_dirs()
        .into_iter()
        .find_map(|obs_dir| resolve_user_config_in(&obs_dir))
}

/// Legacy global.ini path - only consulted for the post-upgrade
/// cleanup pass that strips dead `EnableCustomServerVodTrack` entries
/// left behind by v0.1.0..0.1.2. None on pre-32 installs where
/// `obs_user_config_path` already points at global.ini.
fn legacy_global_ini_path_for_cleanup() -> Option<PathBuf> {
    obs_config_dirs()
        .into_iter()
        .find_map(|obs_dir| legacy_global_ini_in(&obs_dir))
}

/// Read `[General] EnableCustomServerVodTrack` from OBS's user config.
/// Returns `false` when OBS isn't installed, the file is missing, or
/// the key isn't set. We treat any value other than literal `true` /
/// `1` as off, matching OBS's `config_get_bool` parser.
pub fn vod_audio_flag_set() -> bool {
    let Some(p) = obs_user_config_path() else {
        return false;
    };
    let Ok(s) = fs::read_to_string(&p) else {
        return false;
    };
    ini_get(&s, "General", "EnableCustomServerVodTrack")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1"))
        .unwrap_or(false)
}

/// Set `[General] EnableCustomServerVodTrack=<true|false>` in OBS's
/// user config. Idempotent. Returns `Ok(false)` if OBS isn't installed
/// (the file path doesn't exist) so callers can degrade gracefully
/// instead of treating "no OBS" as a hard error.
///
/// Disabling rewrites the existing line to `=false` rather than
/// deleting it. OBS's `config_get_bool` reads `=false` and "key
/// absent" identically, but the in-place rewrite is robust against
/// OBS rewriting user.ini on shutdown and moving the key between
/// duplicate [General] blocks - wherever it lands, the next toggle
/// just flips the value. The downside (one extra line on a never-
/// enabled install) is gated away: insertion only happens on enable.
///
/// On OBS 32+ we also flip the same key to false in the now-orphan
/// global.ini, so users upgrading from v0.1.0..0.1.2 (which mistakenly
/// wrote there) end up with no live flag anywhere.
pub fn set_vod_audio_flag(enable: bool) -> io::Result<bool> {
    let Some(p) = obs_user_config_path() else {
        return Ok(false);
    };
    let original = fs::read_to_string(&p)?;
    let updated = ini_set(&original, "General", "EnableCustomServerVodTrack", enable);
    if updated != original {
        let bak = p.with_extension("ini.instantclone.bak");
        write_or_friendly(&bak, &original)?;
        write_or_friendly(&p, &updated)?;
    }
    // Best-effort legacy cleanup: silently strip the key from
    // global.ini if v0.1.0..0.1.2 ever wrote it there. Failures are
    // non-fatal - the user.ini write above is the load-bearing one,
    // global.ini being unwritable just means the dead key lingers as
    // harmless noise until the user closes OBS.
    if let Some(legacy) = legacy_global_ini_path_for_cleanup() {
        if let Ok(legacy_original) = fs::read_to_string(&legacy) {
            let cleaned = ini_set(
                &legacy_original,
                "General",
                "EnableCustomServerVodTrack",
                false,
            );
            if cleaned != legacy_original {
                let bak = legacy.with_extension("ini.instantclone.bak");
                let _ = write_or_friendly(&bak, &legacy_original);
                let _ = write_or_friendly(&legacy, &cleaned);
            }
        }
    }
    Ok(true)
}

/// Section name if `line` is a `[Section]` header, else None.
///
/// The BOM strip is load-bearing. OBS writes every ini it owns with a
/// UTF-8 BOM, and the first section header sits immediately after it, so
/// line one reads as `\u{feff}[General]`. U+FEFF is not Unicode
/// White_Space, which means `str::trim` leaves it attached and a plain
/// `strip_prefix('[')` misses the header - silently hiding every key in
/// the file's first section.
fn ini_section(line: &str) -> Option<&str> {
    line.trim()
        .trim_start_matches('\u{feff}')
        .strip_prefix('[')?
        .strip_suffix(']')
}

/// Tiny INI reader / writer. OBS's global.ini is a strict
/// "[Section]" + "key=value" file with no comments / continuations -
/// no need to pull a full INI crate for two helpers.
fn ini_get<'a>(file: &'a str, section: &str, key: &str) -> Option<&'a str> {
    let mut cur: Option<&str> = None;
    for line in file.lines() {
        let line = line.trim();
        if let Some(rest) = ini_section(line) {
            cur = Some(rest);
            continue;
        }
        if cur != Some(section) {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == key {
                return Some(v.trim());
            }
        }
    }
    None
}

fn ini_set(file: &str, section: &str, key: &str, enable: bool) -> String {
    // Strategy: walk lines, copy through, ALWAYS rewrite the key's
    // value when we encounter it (true OR false - never drop), or
    // insert at end of section / end of file if missing AND we're
    // enabling. Robust against OBS rewriting user.ini and moving the
    // key around between duplicate [General] blocks - we don't depend
    // on "where" the key lives, just that it lives in the target
    // section *somewhere*. Disabling is just `=false`, which OBS's
    // `config_get_bool` reads identically to "key absent" - so we
    // never need to delete a line we can't reliably find.
    let want_value = if enable { "true" } else { "false" };
    let mut out = String::with_capacity(file.len() + 64);
    let mut cur: Option<String> = None;
    let mut in_target_section = false;
    let mut wrote_key = false;
    let mut section_end_idx: Option<usize> = None;

    // First pass: walk and either rewrite the existing key or remember
    // where the target section ends (so we can insert if absent).
    for line in file.lines() {
        let trimmed = line.trim();
        if let Some(rest) = ini_section(trimmed) {
            // Leaving a section without finding our key - remember the
            // line we're ABOUT to add (the new section header) as the
            // place to splice the key in if needed.
            if in_target_section && !wrote_key {
                section_end_idx = Some(out.len());
            }
            in_target_section = rest == section;
            cur = Some(rest.to_string());
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // Every occurrence, not just the first: a file with duplicate
        // target sections would otherwise keep a stale value in the later
        // block (disable would leave a second `=true` behind).
        if in_target_section {
            if let Some((k, _)) = trimmed.split_once('=') {
                if k.trim() == key {
                    out.push_str(key);
                    out.push('=');
                    out.push_str(want_value);
                    out.push('\n');
                    wrote_key = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }

    // End-of-file inside the target section without finding the key.
    if in_target_section && !wrote_key && enable {
        out.push_str(key);
        out.push('=');
        out.push_str(want_value);
        out.push('\n');
        wrote_key = true;
    }
    // Target section ended before EOF without finding the key:
    // splice the line in at the boundary.
    if !wrote_key && enable {
        if let Some(idx) = section_end_idx {
            let line = format!("{}={}\n", key, want_value);
            out.insert_str(idx, &line);
            wrote_key = true;
        }
    }
    // Section doesn't exist at all - append it with our key.
    if !wrote_key && enable && cur.as_deref() != Some(section) {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("[{}]\n{}={}\n", section, key, want_value));
    }
    // `cur` is set only for triggering the !cur match on absent section
    let _ = cur;
    out
}

// ── VOD+EB experimental: per-profile service.json injection ─────────
//
// When the streamer enables the experimental "Also enable Enhanced
// Broadcasting" sub-toggle, we inject
// `"multitrack_video_configuration_url": "<our endpoint>"` into the
// active OBS profile's service.json under `settings`. OBS's
// rtmp-custom plugin only writes `server` / `key` / `use_auth` /
// `username` / `password` to settings, and discards every other key
// in its `rtmp_custom_update` function. That means the injected URL
// is dropped on LOAD (never reaches OBS's MultitrackVideoAutoConfigURL
// lookup), not only on SAVE - so the injection mechanism is dead.
// We keep `inject_vod_eb` / `revert_vod_eb` only for cleanup of stale
// state left behind by v0.1.0..0.1.2. New Phase C is the
// `launch_obs_with_eb_config` helper below: spawn obs64.exe with the
// `--config-url` CLI flag, which is the only path OBS's frontend
// actually honours for Custom RTMP services.
//
// We touch only the ACTIVE profile (not every profile we find). This
// is the user-chosen scope from the dashboard: minimal footprint, the
// user is warned that switching OBS profiles disables the injection.

// Relative to an OBS config dir (the `.../obs-studio` paths from obs_config_dirs).
const PROFILES_DIR_REL: &str = "basic/profiles";

/// Active OBS profile's DISPLAY name, read from `[Basic] Profile=<name>`
/// in OBS's user config (user.ini on 32+, global.ini on older installs).
/// None when OBS isn't installed or the key isn't present.
///
/// This is the label to show a user. It is NOT a directory name - see
/// `active_profile_dir` for that.
pub fn active_profile() -> Option<String> {
    let p = obs_user_config_path()?;
    let s = fs::read_to_string(&p).ok()?;
    ini_get(&s, "Basic", "Profile").map(|v| v.to_string())
}

/// Active OBS profile's DIRECTORY name under `basic/profiles/`.
///
/// OBS keeps the display name and the directory name as two separate
/// values, and they diverge whenever the name is not filesystem-safe.
/// Profile creation runs the name through `GetFileSafeName` (spaces and
/// reserved characters are replaced) and then `GetClosestUnusedFileName`
/// (a numeric suffix when the directory is taken), then stores both:
///
/// ```text
/// config_set_string(userConfig, "Basic", "Profile",    profile.name);
/// config_set_string(userConfig, "Basic", "ProfileDir", profile.directoryName);
/// ```
///
/// A profile named "Sin Título" therefore lives in `Sin_Título`, so
/// building a path from the display name misses it entirely.
///
/// Resolution order:
///   1. `[Basic] ProfileDir` - authoritative when OBS wrote it.
///   2. Reverse lookup: the profile whose own `basic.ini` declares
///      `[General] Name=<display name>`. Covers installs predating
///      `ProfileDir` and any case where it went stale.
///   3. The display name as-is, which is correct for the plain-ASCII
///      names that make up most profiles.
pub fn active_profile_dir() -> Option<String> {
    let p = obs_user_config_path()?;
    let s = fs::read_to_string(&p).ok()?;
    if let Some(dir) = ini_get(&s, "Basic", "ProfileDir").filter(|d| !d.is_empty()) {
        return Some(dir.to_string());
    }
    let name = ini_get(&s, "Basic", "Profile")?;
    Some(profile_dir_by_name(&obs_config_dirs(), name).unwrap_or_else(|| name.to_string()))
}

/// Directory of the profile whose `basic.ini` declares `[General]
/// Name=<name>`. OBS writes that key into every profile it creates, so
/// it is the authoritative reverse mapping from display name to folder.
///
/// Takes the roots rather than reading the environment so the scan is
/// unit-testable, matching `obs_config_dirs_from`.
fn profile_dir_by_name(roots: &[PathBuf], name: &str) -> Option<String> {
    for root in roots {
        let Ok(entries) = fs::read_dir(root.join(PROFILES_DIR_REL)) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(text) = fs::read_to_string(entry.path().join("basic.ini")) else {
                continue;
            };
            if ini_get(&text, "General", "Name") == Some(name) {
                return entry.file_name().into_string().ok();
            }
        }
    }
    None
}

/// Path to the active profile's service.json, or None if OBS isn't
/// installed / no profile is selected.
pub fn active_profile_service_json_path() -> Option<PathBuf> {
    let dir = active_profile_dir()?;
    obs_config_dirs()
        .into_iter()
        .map(|obs_dir| {
            obs_dir
                .join(PROFILES_DIR_REL)
                .join(&dir)
                .join("service.json")
        })
        .find(|p| p.exists())
}

/// The active profile's stream settings (Settings > Output > Streaming):
/// what OBS streams with when Enhanced Broadcasting is off. None when OBS,
/// the profile, or a usable bitrate can't be read.
pub fn active_stream_settings() -> Option<crate::local_eb_config::StreamerSettings> {
    let profile = active_profile_path()?;
    let basic = fs::read_to_string(profile.join("basic.ini")).ok()?;
    let encoder = fs::read_to_string(profile.join("streamEncoder.json")).ok();
    stream_settings_from(&basic, encoder.as_deref())
}

/// The active profile's folder, the first config dir that holds its basic.ini.
fn active_profile_path() -> Option<PathBuf> {
    let dir = active_profile_dir()?;
    obs_config_dirs()
        .into_iter()
        .map(|obs_dir| obs_dir.join(PROFILES_DIR_REL).join(&dir))
        .find(|p| p.join("basic.ini").exists())
}

/// True when the active profile's SAVED settings ask for Enhanced
/// Broadcasting on a service that can deliver it. Saved is not applied:
/// OBS rebuilds its stream output only while nothing is running, so a
/// Stream settings change made while the Replay Buffer or a recording runs
/// is written to disk but the next stream still goes out on the old setup.
/// The dashboard compares this against what actually arrives.
pub fn active_profile_wants_eb() -> bool {
    let Some(profile) = active_profile_path() else {
        return false;
    };
    let (Ok(basic), Ok(service)) = (
        fs::read_to_string(profile.join("basic.ini")),
        fs::read_to_string(profile.join("service.json")),
    ) else {
        return false;
    };
    profile_wants_eb(&basic, &service)
}

/// See `active_profile_wants_eb`. Only a Common service (the registered
/// InstantClone entry) carries the config URL to OBS: Custom RTMP drops the
/// key on load (see Phase C below), so a leftover URL there never enables EB
/// and must not count.
fn profile_wants_eb(basic_ini: &str, service_json: &str) -> bool {
    if ini_get(basic_ini, "Stream1", "EnableMultitrackVideo") != Some("true") {
        return false;
    }
    let compact: String = service_json
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    compact.contains(r#""type":"rtmp_common""#)
        && compact.contains(r#""multitrack_video_configuration_url":"http"#)
}

/// Parse a profile's `basic.ini` and `streamEncoder.json`. Advanced output
/// names the encoder in `[AdvOut] Encoder` and keeps its settings in
/// `streamEncoder.json`; Simple output keeps only `[SimpleOutput] VBitrate`
/// (OBS derives the rest), so there only the bitrate is taken.
fn stream_settings_from(
    basic_ini: &str,
    encoder_json: Option<&str>,
) -> Option<crate::local_eb_config::StreamerSettings> {
    use crate::local_eb_config::{number_field, StreamerSettings, TrackEncoder};
    let positive = |kbps: u32| (kbps > 0).then_some(kbps);
    if ini_get(basic_ini, "Output", "Mode") != Some("Advanced") {
        let kbps = ini_get(basic_ini, "SimpleOutput", "VBitrate")?.parse().ok();
        return Some(StreamerSettings {
            encoder: None,
            bitrate_kbps: kbps.and_then(positive)?,
            rescale: None,
        });
    }
    let id = ini_get(basic_ini, "AdvOut", "Encoder").filter(|id| !id.is_empty())?;
    // Handed back to OBS as the track's settings, and OBS refuses to start a
    // stream on a malformed config: a broken hand edit keeps the defaults.
    let json = encoder_json?.trim_start_matches('\u{feff}').trim();
    if !(json.starts_with('{') && crate::config::is_valid_json(json)) {
        return None;
    }
    let rescale = match ini_get(basic_ini, "AdvOut", "Rescale") {
        Some("true") => ini_get(basic_ini, "AdvOut", "RescaleRes").and_then(parse_size),
        _ => None,
    };
    Some(StreamerSettings {
        bitrate_kbps: number_field(json, "bitrate").and_then(positive)?,
        encoder: Some(TrackEncoder {
            encoder_type: id.to_string(),
            settings_json: json.to_string(),
        }),
        rescale,
    })
}

/// "1920x1080" as (width, height), both non-zero.
fn parse_size(value: &str) -> Option<(u32, u32)> {
    let (width, height) = value.split_once('x')?;
    let size = (width.trim().parse().ok()?, height.trim().parse().ok()?);
    (size.0 > 0 && size.1 > 0).then_some(size)
}

/// True when our `multitrack_video_configuration_url` is currently
/// injected into the active profile's service.json. Used by the UI
/// status indicator so a manual edit / profile switch is reflected.
pub fn vod_eb_injection_present(web_port: u16) -> bool {
    let Some(p) = active_profile_service_json_path() else {
        return false;
    };
    let Ok(s) = fs::read_to_string(&p) else {
        return false;
    };
    let needle = format!(
        "\"multitrack_video_configuration_url\":\"http://127.0.0.1:{}/obs/multitrack-config\"",
        web_port
    );
    let needle_spaced = format!(
        "\"multitrack_video_configuration_url\": \"http://127.0.0.1:{}/obs/multitrack-config\"",
        web_port
    );
    s.contains(&needle) || s.contains(&needle_spaced)
}

/// Remove the injected URL from the active OBS profile's service.json.
/// Idempotent. Returns Ok(true) when the active profile exists and we
/// removed (or there was nothing to remove); Ok(false) when there's no
/// active profile.
pub fn revert_vod_eb(web_port: u16) -> io::Result<bool> {
    let Some(p) = active_profile_service_json_path() else {
        return Ok(false);
    };
    let original = fs::read_to_string(&p)?;
    let needle = format!("http://127.0.0.1:{}/obs/multitrack-config", web_port);
    if !original.contains(&needle) {
        return Ok(true); // nothing to remove
    }
    let stripped = strip_service_json_key(&original, web_port);
    if stripped == original {
        return Ok(true); // the registered service's own URL: not ours to remove
    }
    let bak = p.with_extension("json.instantclone.bak");
    write_or_friendly(&bak, &original)?;
    write_or_friendly(&p, &stripped)?;
    Ok(true)
}

// ── Phase C v2: launch OBS with --config-url ────────────────────────
//
// OBS's frontend looks for the multi-track config URL in this order
// (frontend/utility/GoLiveAPI_Network.cpp::MultitrackVideoAutoConfigURL):
//
//   1. `--config-url <url>` command-line argument
//   2. `multitrack_video_configuration_url` key in the active service's
//      settings (services.json for rtmp_common, service.json for
//      anything else).
//
// For rtmp_custom services, path 2 is dead - the plugin's
// `rtmp_custom_update` only extracts 5 known keys from the settings
// object and discards the rest, so OBS never sees our injection. The
// CLI flag is the only durable way to enable Enhanced Broadcasting
// alongside VOD audio (which itself requires Custom RTMP, since
// rtmp_common services gate the VOD track checkbox on the service name
// being literally "Twitch"). That's why this is a "Launch OBS" button
// instead of an auto-injected file edit.

/// Standard Windows install paths for obs64.exe. Other installed locations
/// aren't auto-discovered (rare, and the user can always launch OBS
/// themselves with the flag).
#[cfg(windows)]
fn obs_executable_candidates() -> [PathBuf; 2] {
    [
        PathBuf::from("C:\\Program Files\\obs-studio\\bin\\64bit\\obs64.exe"),
        PathBuf::from("C:\\Program Files (x86)\\obs-studio\\bin\\64bit\\obs64.exe"),
    ]
}

/// obs64.exe of the OBS in use (see `obs_installs_by_use`), else the first
/// standard install path that exists.
#[cfg(windows)]
pub fn find_obs_executable() -> Option<PathBuf> {
    obs_installs_by_use()
        .into_iter()
        .filter_map(|install| install.root.map(|root| root.join(PORTABLE_EXE)))
        .chain(obs_executable_candidates())
        .find(|p| p.exists())
}

/// Native `obs` binary from the standard prefixes, then a PATH scan. Flatpak
/// / Snap installs expose no plain binary path (they launch via `flatpak run`
/// / the snap wrapper), so those users launch OBS themselves.
#[cfg(target_os = "linux")]
pub fn find_obs_executable() -> Option<PathBuf> {
    for p in ["/usr/bin/obs", "/usr/local/bin/obs", "/bin/obs"] {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("obs"))
        .find(|p| p.exists())
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn find_obs_executable() -> Option<PathBuf> {
    None
}

// ── OBS version + optional VOD-unlocker script install ──────────────

/// Pull the first `<maj>.<min>.<patch>` triple out of a string. Used to
/// read OBS's version from its log header line
/// (`... OBS 32.2.1 (64-bit, windows)`). Ignores 2-part numbers.
fn first_version_triple(s: &str) -> Option<(u32, u32, u32)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let parts: Vec<&str> = s[start..i].split('.').collect();
            if parts.len() >= 3 {
                if let (Ok(a), Ok(b), Ok(c)) =
                    (parts[0].parse(), parts[1].parse(), parts[2].parse())
                {
                    return Some((a, b, c));
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Best-effort OBS version, parsed from the newest log file's header line
/// (`... OBS 32.2.1 (64-bit, windows)`). `None` when OBS has never run or
/// its logs can't be read. Callers treat `None` as "unknown - allow with
/// a note", not a hard block: this only gates the experimental
/// VOD-unlocker script.
pub fn obs_version() -> Option<(u32, u32, u32)> {
    newest_obs_log_head()?
        .lines()
        .take(30)
        .filter(|l| l.contains("OBS"))
        .find_map(first_version_triple)
}

/// Video encoder ids OBS registered in its most recent session, from the
/// "Available Encoders" block it logs at startup. None when there is no
/// readable log. Enhanced Broadcasting configs must only name encoders in
/// this list: OBS refuses to start the stream on one it doesn't have.
pub fn available_video_encoders() -> Option<Vec<String>> {
    let encoders = parse_available_video_encoders(&newest_obs_log_head()?);
    (!encoders.is_empty()).then_some(encoders)
}

/// Parse the block OBS logs at startup:
///
/// ```text
/// 15:28:19.316: Available Encoders:
/// 15:28:19.316:   Video Encoders:
/// 15:28:19.316: \t- obs_x264 (x264)
/// 15:28:19.316:   Audio Encoders:
/// ```
fn parse_available_video_encoders(log: &str) -> Vec<String> {
    let mut lines = log
        .lines()
        .skip_while(|l| !l.contains("Available Encoders:"))
        .skip_while(|l| !l.contains("Video Encoders:"))
        .skip(1);
    let mut encoders = Vec::new();
    for line in lines.by_ref() {
        // Drop the "HH:MM:SS.mmm: " timestamp, then expect "- <id> (<name>)".
        let entry = line.split_once(": ").map_or(line, |(_, rest)| rest).trim();
        let Some(item) = entry.strip_prefix("- ") else {
            break;
        };
        if let Some(id) = item.split_whitespace().next() {
            encoders.push(id.to_string());
        }
    }
    encoders
}

/// Enough of an OBS log to hold its startup section: the version line and
/// the "Available Encoders" block come right after plugin loading. A log
/// grows for the whole OBS session (tens of MB on a long one), and this is
/// read on every stream start, so the rest is never loaded.
const OBS_LOG_HEAD_BYTES: u64 = 512 * 1024;

/// The start of the most recently written OBS log, which is the running
/// session's while OBS is open. Read lossily: a plugin that writes one
/// non-UTF-8 byte must not hide the whole log (and with it the encoder
/// list and the OBS version).
fn newest_obs_log_head() -> Option<String> {
    let logs_dir = obs_config_dirs()
        .into_iter()
        .map(|d| d.join("logs"))
        .find(|p| p.is_dir())?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(&logs_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
            if newest.as_ref().is_none_or(|(t, _)| modified > *t) {
                newest = Some((modified, path));
            }
        }
    }
    let mut head = Vec::new();
    fs::File::open(newest?.1)
        .ok()?
        .take(OBS_LOG_HEAD_BYTES)
        .read_to_end(&mut head)
        .ok()?;
    Some(String::from_utf8_lossy(&head).into_owned())
}

/// Whether an `obs64.exe` process is currently running. Used by the setup
/// wizard to show the "close OBS first" note only when it actually applies:
/// registering edits `services.json`, which a running OBS overwrites on exit,
/// so the warning is noise if OBS is already shut. Any enumeration failure
/// reads as "not running" - the note just stays hidden, which is harmless.
#[cfg(windows)]
pub fn is_obs_running() -> bool {
    !running_obs_roots().is_empty()
}

/// PIDs of every running `obs64.exe` (Streamlabs' engine included; the
/// caller tells them apart by path). Empty when the list can't be read.
#[cfg(windows)]
fn obs_process_ids() -> Vec<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Vec::new();
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut pids = Vec::new();
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                if String::from_utf16_lossy(&entry.szExeFile[..end])
                    .eq_ignore_ascii_case("obs64.exe")
                {
                    pids.push(entry.th32ProcessID);
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        pids
    }
}

/// Whether an `obs` process is currently running, by scanning `/proc/*/comm`.
/// Any read failure reads as "not running" - the setup note just stays hidden,
/// which is harmless.
#[cfg(target_os = "linux")]
pub fn is_obs_running() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        // Only numeric PID dirs carry a comm file worth reading.
        let name = entry.file_name();
        let Some(pid) = name.to_str() else { continue };
        if !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) {
            if comm.trim() == "obs" {
                return true;
            }
        }
    }
    false
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn is_obs_running() -> bool {
    false
}

/// Spawn OBS Studio with the `--config-url` flag pointing at InstantClone's
/// multitrack-config endpoint, so Enhanced Broadcasting comes up pre-wired.
///
/// Returns Ok when the spawn syscall succeeded (NOT when OBS finishes loading,
/// which is async). Errors carry an actionable "OBS not found" message so the
/// dashboard can tell the user what to do.
#[cfg(windows)]
pub fn launch_obs_with_eb_config(web_port: u16, dock_token: &str) -> io::Result<PathBuf> {
    let exe = find_obs_executable().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "obs64.exe not found in the standard install locations. A portable \
             OBS is found once it has been open while InstantClone runs. Or \
             launch OBS manually with the --config-url <endpoint> flag.",
        )
    })?;
    let config_url = eb_config_url(web_port, dock_token);
    // Working dir = OBS's own folder: its logging and locale paths resolve
    // relative to bin/64bit.
    let workdir = exe.parent().unwrap_or(&exe);
    let mut args = vec!["--config-url", &config_url];
    // A portable OBS started by a `--portable` shortcut has no marker file,
    // and without the flag it would open the AppData config instead.
    if portable_obs_roots()
        .iter()
        .any(|root| exe.starts_with(root))
    {
        args.push("--portable");
    }
    use std::os::windows::process::CommandExt;
    std::process::Command::new(&exe)
        .args(args)
        .current_dir(workdir)
        // CREATE_NEW_PROCESS_GROUP: OBS gets its own console group so
        // InstantClone closing doesn't propagate a break to it.
        .creation_flags(0x0000_0200)
        .spawn()?;
    Ok(exe)
}

/// Linux: try a native `obs` binary, then the Flatpak app, then the Snap.
/// Ubuntu's default OBS is commonly a Flatpak or Snap with no plain binary on
/// PATH, so covering those is what makes the launch button actually work
/// instead of failing for most desktop users.
#[cfg(target_os = "linux")]
pub fn launch_obs_with_eb_config(web_port: u16, dock_token: &str) -> io::Result<PathBuf> {
    let config_url = eb_config_url(web_port, dock_token);

    // 1) Native binary (deb / ppa / most distros).
    if let Some(exe) = find_obs_executable() {
        let workdir = exe.parent().unwrap_or(&exe);
        std::process::Command::new(&exe)
            .args(["--config-url", &config_url])
            .current_dir(workdir)
            .spawn()?;
        return Ok(exe);
    }

    // 2) Flatpak: present when its per-app data dir exists; launched via
    //    `flatpak run`, which forwards our flag to OBS.
    if flatpak_obs_present() && command_on_path("flatpak") {
        std::process::Command::new("flatpak")
            .args(["run", "com.obsproject.Studio", "--config-url", &config_url])
            .spawn()?;
        return Ok(PathBuf::from("flatpak run com.obsproject.Studio"));
    }

    // 3) Snap.
    if snap_obs_present() && command_on_path("snap") {
        std::process::Command::new("snap")
            .args(["run", "obs-studio", "--config-url", &config_url])
            .spawn()?;
        return Ok(PathBuf::from("snap run obs-studio"));
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "OBS not found. Looked for a native obs binary, the Flatpak \
         com.obsproject.Studio, and the obs-studio snap. Launch OBS manually \
         with the --config-url <endpoint> flag.",
    ))
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn launch_obs_with_eb_config(_web_port: u16, _dock_token: &str) -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "launching OBS is not supported on this platform",
    ))
}

/// The `--config-url` OBS should hit for Enhanced Broadcasting. When the
/// dashboard password is on, the dock token rides in the query so OBS (which
/// cannot sign in) still passes the auth gate to the control-plane multitrack
/// endpoint; empty token = the plain URL for the default (no-auth) install.
#[cfg(any(windows, target_os = "linux"))]
fn eb_config_url(web_port: u16, dock_token: &str) -> String {
    if dock_token.is_empty() {
        format!("http://127.0.0.1:{web_port}/obs/multitrack-config")
    } else {
        format!("http://127.0.0.1:{web_port}/obs/multitrack-config?token={dock_token}")
    }
}

/// A `~/.var/app/com.obsproject.Studio` dir means the Flatpak OBS is installed.
#[cfg(target_os = "linux")]
fn flatpak_obs_present() -> bool {
    std::env::var_os("HOME")
        .map(|h| {
            PathBuf::from(h)
                .join(".var/app/com.obsproject.Studio")
                .exists()
        })
        .unwrap_or(false)
}

/// A `~/snap/obs-studio` dir means the Snap OBS is installed.
#[cfg(target_os = "linux")]
fn snap_obs_present() -> bool {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("snap/obs-studio").exists())
        .unwrap_or(false)
}

/// True if `name` resolves to a file on PATH.
#[cfg(target_os = "linux")]
fn command_on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(name).exists()))
        .unwrap_or(false)
}

// ── Desktop shortcut: one-click "Start InstantClone + OBS (VOD + EB)" ──
//
// Generates a clickable launcher on the Desktop that cold-starts the app
// with `--launch-eb`, so a streamer who always wants VOD audio + Enhanced
// Broadcasting gets the whole setup from a single double-click. On Windows
// we prefer a real .lnk (so it carries the app icon), created via
// PowerShell's WScript.Shell COM to avoid a COM crate, falling back to a
// .cmd launcher. On Linux we write a freedesktop `.desktop` launcher.

/// Best-effort Desktop folder. `%USERPROFILE%\Desktop` / `$HOME/Desktop`
/// covers the vast majority of installs; localized or redirected desktops
/// are not auto-discovered. None when we can't resolve a home dir.
fn desktop_dir() -> Option<PathBuf> {
    let home = std::env::var("USERPROFILE")
        .ok()
        .or_else(|| std::env::var("HOME").ok())?;
    Some(PathBuf::from(home).join("Desktop"))
}

/// PowerShell single-quoted-string escaping: a literal `'` becomes `''`.
#[cfg(windows)]
fn ps_single_quote(s: &str) -> String {
    s.replace('\'', "''")
}

/// Try to create a real Windows `.lnk` shortcut via PowerShell COM.
/// Returns Ok only when PowerShell reports success AND the file exists.
#[cfg(windows)]
fn try_create_lnk(lnk: &Path, exe: &Path) -> io::Result<()> {
    let workdir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{lnk}');\
         $s.TargetPath='{exe}';$s.Arguments='--launch-eb';\
         $s.IconLocation='{exe}';$s.WorkingDirectory='{wd}';$s.Save()",
        lnk = ps_single_quote(&lnk.display().to_string()),
        exe = ps_single_quote(&exe.display().to_string()),
        wd = ps_single_quote(&workdir.display().to_string()),
    );
    use std::os::windows::process::CommandExt as _;
    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        // A release build has no console, so PowerShell would flash one open.
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status()?;
    if status.success() && lnk.exists() {
        Ok(())
    } else {
        Err(io::Error::other("PowerShell did not create the shortcut"))
    }
}

/// Create a Desktop shortcut that launches InstantClone in VOD+EB mode
/// (`instantclone --launch-eb`). Returns the path of the file actually
/// created (a `.lnk` when possible, otherwise a `.cmd` fallback) so the UI
/// can tell the user exactly what to look for.
#[cfg(windows)]
pub fn create_eb_shortcut() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let desktop = desktop_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not locate your Desktop folder",
        )
    })?;
    // Make sure the directory exists (it always should, but a redirected
    // profile mid-setup could briefly not have it).
    if !desktop.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Desktop folder not found at {}", desktop.display()),
        ));
    }
    let lnk = desktop.join("InstantClone (VOD + EB).lnk");
    if try_create_lnk(&lnk, &exe).is_ok() {
        return Ok(lnk);
    }
    // Fallback: a plain .cmd launcher. `start "" "<exe>"` detaches so the
    // console window closes immediately after spawning the app.
    let cmd = desktop.join("InstantClone (VOD + EB).cmd");
    let body = format!(
        "@echo off\r\nstart \"\" \"{}\" --launch-eb\r\n",
        exe.display()
    );
    write_or_friendly(&cmd, &body)?;
    Ok(cmd)
}

/// Linux: write a freedesktop `.desktop` launcher on the Desktop. It must be
/// marked executable (GNOME also treats that as the "trusted" bit) to run on
/// double-click.
#[cfg(target_os = "linux")]
pub fn create_eb_shortcut() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let desktop = desktop_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not locate your Desktop folder",
        )
    })?;
    if !desktop.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Desktop folder not found at {}", desktop.display()),
        ));
    }
    let file = desktop.join("InstantClone (VOD + EB).desktop");
    let icon = crate::autostart::ensure_icon();
    let icon_line = icon
        .as_ref()
        .and_then(|p| p.to_str())
        .map(|i| format!("Icon={i}\n"))
        .unwrap_or_default();
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=InstantClone (VOD + EB)\n\
         Comment=Start InstantClone in VOD + EB mode and launch OBS\n\
         {icon_line}\
         Exec=\"{}\" --launch-eb\n\
         Terminal=false\n",
        exe.display()
    );
    write_or_friendly(&file, &entry)?;
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(&file)?.permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&file, perm)?;
    Ok(file)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn create_eb_shortcut() -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "desktop shortcut creation is not supported on this platform",
    ))
}

/// `file` with our legacy `multitrack_video_configuration_url` taken out
/// of a Custom (`rtmp_custom`) service, where v0.1.0..0.1.2 injected it.
/// The registered InstantClone service (`rtmp_common`) carries the same URL
/// on purpose, copied by OBS from services.json: that one must stay, or
/// Enhanced Broadcasting loses its config. Edited as JSON, never by lines:
/// OBS writes service.json on one line. Anything else comes back as it was.
fn strip_service_json_key(file: &str, web_port: u16) -> String {
    let ours = format!("http://127.0.0.1:{web_port}/obs/multitrack-config");
    let Ok(mut doc) = crate::json::parse(file.trim_start_matches('\u{feff}')) else {
        return file.to_string();
    };
    if doc.str_or("type", "") != "rtmp_custom" {
        return file.to_string();
    }
    let crate::json::Value::Obj(fields) = &mut doc else {
        return file.to_string();
    };
    let Some((_, crate::json::Value::Obj(settings))) =
        fields.iter_mut().find(|(key, _)| key == "settings")
    else {
        return file.to_string();
    };
    let before = settings.len();
    settings.retain(|(key, value)| {
        !(key == "multitrack_video_configuration_url" && value.as_str() == Some(ours.as_str()))
    });
    if settings.len() == before {
        return file.to_string();
    }
    doc.to_json()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32 as TestUniq, Ordering as TestOrd};

    #[test]
    fn available_video_encoders_are_read_from_the_startup_block() {
        let log = "15:28:19.316: Available Encoders:\n\
                   15:28:19.316:   Video Encoders:\n\
                   15:28:19.316: \t- ffmpeg_svt_av1 (SVT-AV1)\n\
                   15:28:19.316: \t- h264_texture_amf (AMD HW H.264 (AVC))\n\
                   15:28:19.316: \t- obs_x264 (x264)\n\
                   15:28:19.316:   Audio Encoders:\n\
                   15:28:19.316: \t- ffmpeg_aac (FFmpeg AAC)\n";
        assert_eq!(
            parse_available_video_encoders(log),
            vec!["ffmpeg_svt_av1", "h264_texture_amf", "obs_x264"]
        );
        assert!(parse_available_video_encoders("no block here").is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_obs_config_dirs_cover_native_flatpak_snap() {
        use std::path::PathBuf;
        let dirs =
            obs_config_dirs_from(Some(PathBuf::from("/home/u")), Some(PathBuf::from("/xdg")));
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/xdg/obs-studio"),
                PathBuf::from("/home/u/.var/app/com.obsproject.Studio/config/obs-studio"),
                PathBuf::from("/home/u/snap/obs-studio/current/.config/obs-studio"),
            ]
        );
        // No home -> no candidates (every OBS lookup then returns None).
        assert!(obs_config_dirs_from(None, None).is_empty());
        // A relative XDG value is ignored; falls back to ~/.config.
        let fb = obs_config_dirs_from(Some(PathBuf::from("/home/u")), Some(PathBuf::from("rel")));
        assert_eq!(fb[0], PathBuf::from("/home/u/.config/obs-studio"));
    }

    fn fake_services_json() -> String {
        // Minimal but realistic shape. OBS's actual file has many more
        // services; we only need the structural skeleton for our edits.
        r#"{
  "format_version": 4,
  "services": [
    {
      "name": "Twitch",
      "common": true
    },
    {
      "name": "YouTube - RTMPS",
      "common": true
    }
  ]
}
"#
        .to_string()
    }

    #[test]
    fn first_version_triple_reads_obs_log_header() {
        // The real OBS log header line.
        assert_eq!(
            first_version_triple("18:52:33.123: OBS 32.2.1 (64-bit, windows)"),
            Some((32, 2, 1))
        );
        assert_eq!(first_version_triple("OBS 30.0.0"), Some((30, 0, 0)));
        // Two-part numbers don't count; need major.minor.patch.
        assert_eq!(first_version_triple("version 1.2 only"), None);
        assert_eq!(first_version_triple("no digits here"), None);
    }

    /// Whitespace-blind view of a JSON fixture, for "same document" checks
    /// where only indentation may differ.
    fn without_whitespace(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    fn count_entries(file: &str) -> usize {
        file.matches(r#""name": "InstantClone""#).count()
    }

    /// Register then unregister must give the user back the file they
    /// had. A leftover comma or brace still "contains Twitch", so the
    /// result is compared whole and parsed, not probed for substrings.
    #[test]
    fn register_then_remove_restores_the_original_document() {
        let original = fake_services_json();
        let patched = with_entry_registered(&original, 7799, 1935).expect("register");
        assert!(crate::config::is_valid_json(&patched), "{patched}");
        assert_eq!(count_entries(&patched), 1);

        let stripped = remove_entry(&patched).expect("remove must succeed");
        assert!(crate::config::is_valid_json(&stripped), "{stripped}");
        assert_eq!(without_whitespace(&stripped), without_whitespace(&original));
    }

    /// An OBS with no services listed: our entry becomes the only element,
    /// so it takes no comma (`[{entry},]` is invalid JSON, and OBS would
    /// drop the whole list), and removing it gives the empty list back.
    #[test]
    fn registering_into_an_empty_service_list_keeps_the_file_valid() {
        let original = "{\n  \"format_version\": 5,\n  \"services\": []\n}\n";
        let patched = with_entry_registered(original, 7799, 1935).expect("register");
        assert!(crate::config::is_valid_json(&patched), "{patched}");
        assert_eq!(count_entries(&patched), 1);

        let stripped = remove_entry(&patched).expect("remove must succeed");
        assert!(crate::config::is_valid_json(&stripped), "{stripped}");
        assert_eq!(without_whitespace(&stripped), without_whitespace(original));
    }

    /// Registering touches nothing but our own entry: cut the entry back
    /// out of the result by its exact span and every other byte of OBS's
    /// file must still be there, in order.
    #[test]
    fn register_preserves_every_other_byte() {
        let original = fake_services_json();
        let patched = with_entry_registered(&original, 7799, 1935).expect("register");
        let spliced = format!("\n{},", entry_json(7799, 1935));
        assert_eq!(patched.replacen(&spliced, "", 1), original);
    }

    /// `register_in` runs from the dashboard button and again from the stale
    /// entry repair at startup. Running it twice must be the same file as
    /// running it once, not a second entry or a file that grows each time.
    #[test]
    fn register_twice_is_the_same_as_once() {
        let once = with_entry_registered(&fake_services_json(), 7799, 1935).unwrap();
        let twice = with_entry_registered(&once, 7799, 1935).unwrap();
        assert_eq!(twice, once);
    }

    /// OBS rewrites services.json on its own schedule and does not keep our
    /// entry first. With our entry last, removal takes the leading-comma
    /// path, which must still leave one entry and valid JSON behind.
    #[test]
    fn register_and_remove_handle_our_entry_last_in_the_array() {
        let original = fake_services_json();
        let last = original.replacen(
            "\n  ]",
            &format!(",\n    {}\n  ]", entry_json(7799, 1935)),
            1,
        );
        assert!(crate::config::is_valid_json(&last), "fixture: {last}");

        let refreshed = with_entry_registered(&last, 8800, 1935).expect("register");
        assert!(crate::config::is_valid_json(&refreshed), "{refreshed}");
        assert_eq!(count_entries(&refreshed), 1);
        assert!(refreshed.contains(":8800/") && !refreshed.contains(":7799/"));

        let stripped = remove_entry(&last).expect("remove");
        assert!(crate::config::is_valid_json(&stripped), "{stripped}");
        assert_eq!(without_whitespace(&stripped), without_whitespace(&original));
    }

    /// When our old entry cannot be cut out cleanly (here: a file cut off
    /// mid-entry by a crash during a write), inserting anyway would leave
    /// two InstantClone entries in OBS's service list. Refuse instead.
    #[test]
    fn register_refuses_when_the_old_entry_cannot_be_removed() {
        let truncated =
            r#"{"format_version": 4, "services": [{"name": "InstantClone", "common": true"#;
        assert!(entry_exists(truncated) && remove_entry(truncated).is_none());
        match with_entry_registered(truncated, 7799, 1935) {
            Ok(patched) => panic!("must refuse, wrote {} entries", count_entries(&patched)),
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidData),
        }
        assert!(with_entry_registered("definitely not services.json", 7799, 1935).is_err());
    }

    /// The single server entry must be the `localhost` spelling. An IP
    /// literal of either family is unreachable under one of OBS's three IP
    /// Family settings, because that setting picks the address family
    /// handed to `getaddrinfo`. Measured on Windows 11:
    /// `getaddrinfo("127.0.0.1", AF_INET6)` returns 11001 host-not-found,
    /// while `localhost` resolves under AF_INET, AF_INET6 and AF_UNSPEC.
    #[test]
    fn entry_advertises_localhost_so_every_ip_family_setting_resolves() {
        let entry = entry_json(7799, 1935);
        assert!(entry.contains(r#""url": "rtmp://localhost:1935/live""#));
        // A literal would strand one IP Family setting apiece.
        assert!(
            !entry.contains("rtmp://127.0.0.1:1935/live"),
            "no v4 literal"
        );
        assert!(!entry.contains("rtmp://[::1]:1935/live"), "no v6 literal");
        // One server, so the user never has to pick.
        assert_eq!(entry.matches(r#""url": "rtmp://"#).count(), 1);
    }

    /// Updating should not leave someone with a service entry OBS cannot
    /// reach and a button they have to notice and press. Repair it for them,
    /// but only the entry they already asked for, and only when OBS is not
    /// holding services.json in memory ready to write it back out.
    #[test]
    fn a_stale_entry_is_repaired_only_when_it_is_safe_to() {
        let current = entry_json(7799, 1935);
        let stale = current.replace("rtmp://localhost:1935/live", "rtmp://127.0.0.1:1935/live");
        let file_current = format!(r#"{{"services":[{current}]}}"#);
        let file_stale = format!(r#"{{"services":[{stale}]}}"#);
        let file_none = r#"{"services":[{"name":"Twitch","servers":[]}]}"#;

        use RegistrationRefresh::*;
        let nobody = |_: u16| false;
        assert_eq!(
            refresh_decision(&file_stale, 7799, 1935, false, nobody),
            Repaired,
            "an out-of-date entry the user registered is repaired"
        );
        assert_eq!(
            refresh_decision(&file_stale, 7799, 1935, true, nobody),
            Deferred,
            "OBS is open, so wait for it rather than have the repair undone"
        );
        assert_eq!(
            refresh_decision(&file_current, 7799, 1935, false, nobody),
            NotNeeded,
            "an entry that is already right is left alone"
        );
        assert_eq!(
            refresh_decision(file_none, 7799, 1935, false, nobody),
            NotNeeded,
            "someone who never registered is not signed up for us editing OBS"
        );
        assert_eq!(
            refresh_decision(file_none, 7799, 1935, true, nobody),
            NotNeeded,
            "and with OBS open there is still nothing to come back for"
        );
        // A retuned port is the same kind of staleness.
        assert_eq!(
            refresh_decision(&file_current, 7799, 1936, false, nobody),
            Repaired
        );
    }

    /// Two InstantClones from two folders: the entry points at the other
    /// one, which is running. That is not stale, it is the other copy's, and
    /// rewriting it would silently move OBS off the copy it streams through.
    #[test]
    fn an_entry_pointing_at_another_running_copy_is_left_alone() {
        let theirs = format!(r#"{{"services":[{}]}}"#, entry_json(7800, 1936));
        let running_other = |port: u16| port == 7800 || port == 1936;
        use RegistrationRefresh::*;
        assert_eq!(
            refresh_decision(&theirs, 7799, 1935, false, running_other),
            OtherInstance(7800)
        );
        assert_eq!(
            refresh_decision(&theirs, 7799, 1935, true, running_other),
            OtherInstance(7800),
            "nothing to wait for either"
        );
        // Only the RTMP port is held (that copy's dashboard is on a port it
        // later moved): still the other copy's.
        assert_eq!(
            refresh_decision(&theirs, 7799, 1935, false, |p| p == 1936),
            OtherInstance(7800)
        );
        // Nobody on those ports: a leftover from before our own port change.
        assert_eq!(
            refresh_decision(&theirs, 7799, 1935, false, |_| false),
            Repaired
        );
        // Our own ports are never "another" copy, whatever holds them.
        let ours_stale = format!(
            r#"{{"services":[{}]}}"#,
            entry_json(7799, 1935).replace("rtmp://localhost:", "rtmp://127.0.0.1:")
        );
        assert_eq!(
            refresh_decision(&ours_stale, 7799, 1935, false, |_| true),
            Repaired
        );
        assert_eq!(registered_ports(&theirs), Some((Some(7800), Some(1936))));
        assert_eq!(registered_ports(r#"{"services":[]}"#), None);
    }

    /// An entry from an older build still carries our name while pointing
    /// OBS at an address it may not be able to resolve. Reporting that as
    /// registered would leave the user no way to notice, so staleness has
    /// to read as unregistered and put the Register button back.
    #[test]
    fn a_stale_entry_does_not_count_as_registered() {
        let current = entry_json(7799, 1935);
        assert!(registration_is_current(&current, 7799, 1935));

        // Pre-IPv6 builds wrote the v4 literal.
        let legacy = current.replace("rtmp://localhost:1935/live", "rtmp://127.0.0.1:1935/live");
        assert!(entry_exists(&legacy), "still ours by name");
        assert!(
            !registration_is_current(&legacy, 7799, 1935),
            "a v4-literal entry is stale and must prompt a re-register"
        );

        // A retuned port is stale for the same reason.
        assert!(!registration_is_current(&current, 7799, 1936));
        assert!(!registration_is_current(&current, 7800, 1935));

        // Our current URL present but literal servers left over beside it:
        // the dropdown would still offer options that fail under one IP
        // Family setting each, so this has to read as stale too.
        let with_leftovers = current.replace(
            r#""url": "rtmp://localhost:1935/live""#,
            r#""url": "rtmp://localhost:1935/live"}, {"name": "old", "url": "rtmp://[::1]:1935/live""#,
        );
        assert!(with_leftovers.contains("rtmp://localhost:1935/live"));
        assert!(
            !registration_is_current(&with_leftovers, 7799, 1935),
            "a superset of our entry is still out of date"
        );

        // One from before HEVC and AV1 were offered: OBS would swap an AV1
        // encoder for H.264 under it.
        let h264_only = current.replace(r#"["h264", "hevc", "av1"]"#, r#"["h264"]"#);
        assert_ne!(h264_only, current);
        assert!(!registration_is_current(&h264_only, 7799, 1935));
    }

    /// Found on a real install: a leftover `InstantClone (local proxy)`
    /// server object sitting inside Twitch's `servers` array, unrelated to
    /// our own entry. A whole-file staleness scan sees that literal and
    /// reports stale forever, and because re-registering only rewrites our
    /// entry, the Register button can never turn into Unregister no matter
    /// how many times it is pressed. The check has to look at our entry
    /// only.
    #[test]
    fn a_stray_instantclone_server_in_another_service_does_not_block_registration() {
        let polluted = format!(
            r#"{{"format_version":4,"services":[
{},
{{"name":"Twitch","common":true,"servers":[
                {{ "name": "InstantClone (local proxy)", "url": "rtmp://127.0.0.1:1935/live" }},
                {{"name":"Asia: Hong Kong","url":"rtmp://hk.contribute.live-video.net/app"}}]}}
]}}"#,
            entry_json(7799, 1935)
        );
        assert!(
            polluted.contains("rtmp://127.0.0.1:1935/live"),
            "the stray must be present for this test to mean anything"
        );
        assert!(
            registration_is_current(&polluted, 7799, 1935),
            "a stray in someone else's entry must not make ours look stale"
        );
    }

    /// Removing our entry must leave the unrelated stray alone: it is not
    /// ours to delete, and clobbering another service's servers array
    /// would be far worse than the dead dropdown option it causes.
    #[test]
    fn remove_entry_leaves_a_stray_in_another_service_untouched() {
        let polluted = format!(
            r#"{{"format_version":4,"services":[
{},
{{"name":"Twitch","common":true,"servers":[
                {{ "name": "InstantClone (local proxy)", "url": "rtmp://127.0.0.1:1935/live" }}]}}
]}}"#,
            entry_json(7799, 1935)
        );
        let stripped = remove_entry(&polluted).expect("remove");
        assert!(!entry_exists(&stripped), "our entry is gone");
        assert!(
            stripped.contains(
                r#"{ "name": "InstantClone (local proxy)", "url": "rtmp://127.0.0.1:1935/live" }"#
            ),
            "Twitch's array must be untouched"
        );
        assert!(stripped.contains("Twitch"));
    }

    #[test]
    fn remove_handles_single_entry_array() {
        // After our entry has been the only thing in the services
        // array, removing it should leave a syntactically valid (if
        // empty) array, not a stray comma.
        let single = r#"{
  "format_version": 4,
  "services": [
    {
      "name": "InstantClone",
      "common": true
    }
  ]
}
"#;
        let out = remove_entry(single).expect("remove must succeed");
        assert!(!entry_exists(&out));
        assert!(crate::config::is_valid_json(&out), "{out}");
    }

    #[test]
    fn entry_exists_matches_both_indent_styles() {
        let indented = r#"  "name": "InstantClone","#;
        let compact = r#""name":"InstantClone","#;
        assert!(entry_exists(indented));
        assert!(entry_exists(compact));
        assert!(!entry_exists(r#""name": "InstantCloneAndCompany""#));
    }

    /// The span has to be the object that encloses our name key, whatever
    /// order the keys are in. A backward scan for the nearest `{` used to
    /// land inside a preceding string - `{stream_key}` in a url template is
    /// exactly that shape - and hand back a span that was not our entry,
    /// which reads as "stale" forever and pins the Register button.
    #[test]
    fn entry_span_is_the_object_around_our_name_whatever_the_key_order() {
        let file = r#"{"services":[
  {"name": "Twitch", "servers": [{"name": "Auto", "url": "rtmp://live.twitch.tv/app"}]},
  {"recommended": {"url_template": "rtmp://localhost:1935/live/{stream_key}"},
   "name": "InstantClone",
   "servers": [{"name": "InstantClone (local proxy)", "url": "rtmp://localhost:1935/live"}]},
  {"name": "YouTube", "servers": [{"name": "Primary", "url": "rtmp://a.rtmp.youtube.com/live2"}]}
]}"#;
        let span = entry_span(file).expect("our entry");
        let entry = &file[span];
        assert!(entry.starts_with('{') && entry.ends_with('}'), "{entry}");
        assert!(
            entry.contains(r#""name": "InstantClone""#),
            "the span must hold our name key: {entry}"
        );
        assert!(
            entry.contains("{stream_key}"),
            "and the whole object around it: {entry}"
        );
        assert!(
            !entry.contains("Twitch") && !entry.contains("YouTube"),
            "and nothing belonging to another service: {entry}"
        );
    }

    #[test]
    fn looks_like_services_json_rejects_garbage() {
        // A file that doesn't even start with `{` is almost certainly
        // not OBS's services.json - refuse to splice into it so we
        // don't compound the corruption.
        assert!(looks_like_services_json("garbage not even json").is_err());
        // Valid JSON object but no services array - could be some
        // other config file at the wrong path; bail out.
        assert!(looks_like_services_json(r#"{"foo":"bar"}"#).is_err());
        // The real shape passes.
        assert!(looks_like_services_json(&fake_services_json()).is_ok());
    }

    /// Re-registering after the dashboard or ingest port changed must
    /// swap the URLs in place, through the same transform `register_in` runs.
    #[test]
    fn re_register_refreshes_changed_port() {
        let v1 = with_entry_registered(&fake_services_json(), 7799, 1935).unwrap();
        assert!(v1.contains(":7799/"));
        let v2 = with_entry_registered(&v1, 8800, 1936).unwrap();
        assert!(
            v2.contains(":8800/") && v2.contains("localhost:1936/"),
            "{v2}"
        );
        assert!(!v2.contains(":7799/") && !v2.contains(":1935/"), "{v2}");
        assert_eq!(count_entries(&v2), 1, "exactly one InstantClone entry");
        assert!(crate::config::is_valid_json(&v2), "{v2}");
    }

    // ── INI read/write ───────────────────────────────────────────────

    /// OBS's ini files start with a BOM on Windows and repeat key names across sections.
    #[test]
    fn ini_get_reads_the_right_section_bom_or_not() {
        // ini get reads the first section of a bom prefixed file
        // Every ini OBS owns starts with a UTF-8 BOM immediately followed by
        // `[General]`, so the first section is exactly the one a BOM-blind
        // parser drops. Verified against a real install: `user.ini` opens
        // `ef bb bf 5b 47 65 6e ...`. U+FEFF is not Unicode White_Space, so
        // `trim` does not save us here.
        {
            let ini = "\u{feff}[General]\nName=NewUi2\nEnableCustomServerVodTrack=true\n\
                       [Basic]\nProfile=Sin Título\n";
            assert_eq!(
                ini_get(ini, "General", "EnableCustomServerVodTrack"),
                Some("true"),
                "the BOM must not hide the first section"
            );
            assert_eq!(ini_get(ini, "General", "Name"), Some("NewUi2"));
            // Later sections were always reachable; keep them that way.
            assert_eq!(ini_get(ini, "Basic", "Profile"), Some("Sin Título"));
        }
        // ini get reads value from correct section
        {
            let ini =
                "[Basic]\nProfile=Untitled\n[General]\nName=Foo\nEnableCustomServerVodTrack=true\n";
            assert_eq!(
                ini_get(ini, "General", "EnableCustomServerVodTrack"),
                Some("true")
            );
            assert_eq!(ini_get(ini, "Basic", "Profile"), Some("Untitled"));
            assert_eq!(ini_get(ini, "General", "Missing"), None);
            // Key exists in a different section - must not match.
            assert_eq!(ini_get(ini, "Basic", "EnableCustomServerVodTrack"), None);
        }
    }

    /// The shapes OBS really leaves behind: a BOM before the first section, and
    /// a section that appears twice.
    #[test]
    fn ini_set_handles_bom_and_duplicate_sections() {
        // ini set edits the bom prefixed section instead of appending a duplicate
        // The duplicate-`[General]` files seen in the wild were self-inflicted:
        // `ini_set` could not see the BOM'd header, so it appended a second
        // section instead of editing the first. Writing must now land in the
        // existing section and leave the BOM on the file.
        {
            let ini = "\u{feff}[General]\nPre19Defaults=false\n\n[Basic]\nProfile=A\n";
            let out = ini_set(ini, "General", "EnableCustomServerVodTrack", true);

            assert_eq!(
                out.matches("[General]").count(),
                1,
                "must edit OBS's own section, not append a rival one"
            );
            assert!(
                out.starts_with('\u{feff}'),
                "the BOM belongs to OBS - we must not strip it on write"
            );
            assert_eq!(
                ini_get(&out, "General", "EnableCustomServerVodTrack"),
                Some("true")
            );
            assert_eq!(ini_get(&out, "General", "Pre19Defaults"), Some("false"));
            assert_eq!(ini_get(&out, "Basic", "Profile"), Some("A"));
        }
        // ini set rewrites the key in every duplicate section
        // Files written by older builds can carry two `[General]` blocks,
        // each with the flag (see the BOM test above). Rewriting only the
        // first left the second at `=true`, so turning VOD audio off did not
        // turn the OBS checkbox off for whichever block OBS reads.
        {
            let ini = "[General]\nEnableCustomServerVodTrack=true\n\n[Basic]\nProfile=A\n\n\
                       [General]\nEnableCustomServerVodTrack=true\nName=X\n";
            let out = ini_set(ini, "General", "EnableCustomServerVodTrack", false);

            assert_eq!(out.matches("EnableCustomServerVodTrack=false").count(), 2);
            assert!(!out.contains("EnableCustomServerVodTrack=true"), "{out}");
            // Every other line is untouched and nothing is appended.
            assert_eq!(out.matches("[General]").count(), 2);
            assert_eq!(ini_get(&out, "Basic", "Profile"), Some("A"));
            assert!(out.contains("Name=X\n"));
            assert_eq!(out.lines().count(), ini.lines().count());
        }
        // ini set flips value to false in trailing duplicate general section
        // Reported on the v0.1.3 test build: user.ini ends up with two
        // `[General]` sections. The second one was ours: the BOM on line
        // one hid OBS's real `[General]` from `ini_set`, which then
        // appended its own. `ini_section` fixes the cause, but files
        // written before that fix still carry the duplicate, so the walk
        // has to keep handling it. Our toggle-ON lands the key in the
        // trailing block; this test confirms toggle-OFF flips its value
        // in place to `false` without disturbing either section's other
        // keys or trying to delete the line - which would be fragile
        // against OBS rewriting the file and moving the key around. The
        // file shape mirrors what OBS 32 wrote after the first toggle +
        // restart, trimmed down to the bits that exercise the
        // duplicate-section walk.
        {
            let ini = "[General]\n\
                Pre19Defaults=false\n\
                FirstRun=true\n\
                \n\
                [Basic]\n\
                Profile=Sin Título\n\
                \n\
                [Audio]\n\
                DisableAudioDucking=true\n\
                \n\
                [General]\n\
                EnableCustomServerVodTrack=true\n\
                FirstRun=true\n";
            let out = ini_set(ini, "General", "EnableCustomServerVodTrack", false);

            // Value flipped, line stays.
            assert_eq!(
                ini_get(&out, "General", "EnableCustomServerVodTrack"),
                Some("false")
            );
            assert!(out.contains("EnableCustomServerVodTrack=false"));
            assert!(!out.contains("EnableCustomServerVodTrack=true"));

            // Both [General] sections are still there - we never want to
            // collapse OBS's own structure, just edit our one key.
            assert_eq!(out.matches("[General]").count(), 2);

            // Sibling keys in BOTH [General] blocks survive.
            assert!(out.contains("Pre19Defaults=false"));
            assert!(out.contains("FirstRun=true"));

            // Unrelated sections are untouched.
            assert_eq!(ini_get(&out, "Basic", "Profile"), Some("Sin Título"));
            assert_eq!(ini_get(&out, "Audio", "DisableAudioDucking"), Some("true"));
        }
        // ini set lands in trailing general when duplicate sections exist
        // Sibling property: toggle-ON on a fresh duplicate-section file
        // (no key present in either [General]) lands the key in the
        // trailing [General] - which is what we observed during the
        // v0.1.3 test build. Documenting this so a future refactor that
        // chooses to insert into the first block instead doesn't surprise
        // the next maintainer.
        {
            let ini = "[General]\n\
                FirstRun=true\n\
                \n\
                [Basic]\n\
                Profile=A\n\
                \n\
                [General]\n\
                ConfirmOnExit=true\n";
            let out = ini_set(ini, "General", "EnableCustomServerVodTrack", true);

            assert_eq!(
                ini_get(&out, "General", "EnableCustomServerVodTrack"),
                Some("true")
            );
            // Lands inside the trailing [General], after ConfirmOnExit.
            let trailing = out.rfind("[General]").expect("trailing [General] kept");
            assert!(out[trailing..].contains("EnableCustomServerVodTrack=true"));
            // Idempotent toggle-on - second call must not double-write.
            let twice = ini_set(&out, "General", "EnableCustomServerVodTrack", true);
            assert_eq!(twice.matches("EnableCustomServerVodTrack=true").count(), 1);
        }
    }

    // ── Active profile directory resolution ──────────────────────────

    /// `[Basic] Profile` is a display name and `[Basic] ProfileDir` is the
    /// folder. Confirmed divergent on a real install: profile "Sin Título"
    /// lives in `basic/profiles/Sin_Título/`. Building a path from the
    /// display name misses it, which silently no-ops every per-profile
    /// read and write we do.
    #[test]
    fn profile_dir_reverse_lookup_matches_on_the_profiles_own_name_key() {
        let root = fresh_obs_dir("profile-dir");
        let profiles = root.join(PROFILES_DIR_REL);
        for (dir, name) in [("NewUi2", "NewUi2"), ("Sin_Título", "Sin Título")] {
            std::fs::create_dir_all(profiles.join(dir)).unwrap();
            std::fs::write(
                profiles.join(dir).join("basic.ini"),
                format!("\u{feff}[General]\nName={name}\n\n[Output]\nBindIP=default\n"),
            )
            .unwrap();
        }
        let roots = vec![root.to_path_buf()];

        assert_eq!(
            profile_dir_by_name(&roots, "Sin Título").as_deref(),
            Some("Sin_Título"),
            "a sanitized directory must still be found by display name"
        );
        assert_eq!(
            profile_dir_by_name(&roots, "NewUi2").as_deref(),
            Some("NewUi2")
        );
        assert_eq!(profile_dir_by_name(&roots, "Nope"), None);
        // No profiles directory at all must not panic.
        let empty = fresh_obs_dir("empty");
        assert_eq!(profile_dir_by_name(&[empty.to_path_buf()], "A"), None);
    }

    /// A real Advanced-output profile (AMD H.264, CBR 6000): the local
    /// Enhanced Broadcasting config must stream with exactly these.
    const ADVANCED_BASIC_INI: &str = "[General]\nName=Main\n\n[Output]\nMode=Advanced\n\n\
        [AdvOut]\nEncoder=h264_texture_amf\nRescale=false\nRescaleRes=1920x1080\n";
    const AMD_STREAM_ENCODER: &str =
        r#"{"bitrate":6000,"rate_control":"CBR","preset":"quality","profile":"high","bf":2}"#;

    #[test]
    fn advanced_output_settings_are_taken_whole() {
        let own = stream_settings_from(ADVANCED_BASIC_INI, Some(AMD_STREAM_ENCODER))
            .expect("readable settings");
        let encoder = own.encoder.expect("Advanced output names its encoder");
        assert_eq!(encoder.encoder_type, "h264_texture_amf");
        assert_eq!(encoder.settings_json, AMD_STREAM_ENCODER);
        assert_eq!(own.bitrate_kbps, 6000);
        assert_eq!(own.rescale, None, "Rescale Output is off");
    }

    #[test]
    fn rescale_output_sets_the_main_track_size_only_when_on() {
        let on = ADVANCED_BASIC_INI.replace("Rescale=false", "Rescale=true");
        let own = stream_settings_from(&on, Some(AMD_STREAM_ENCODER)).unwrap();
        assert_eq!(own.rescale, Some((1920, 1080)));
        let bad_size = on.replace("RescaleRes=1920x1080", "RescaleRes=0x1080");
        let own = stream_settings_from(&bad_size, Some(AMD_STREAM_ENCODER)).unwrap();
        assert_eq!(own.rescale, None, "a size OBS would refuse is ignored");
    }

    #[test]
    fn simple_output_contributes_only_its_bitrate() {
        let simple = "[Output]\nMode=Simple\n\n[SimpleOutput]\nVBitrate=4500\n";
        let own = stream_settings_from(simple, Some(AMD_STREAM_ENCODER)).unwrap();
        assert!(
            own.encoder.is_none(),
            "Simple output keeps no encoder settings"
        );
        assert_eq!(own.bitrate_kbps, 4500);
        let no_mode = "[SimpleOutput]\nVBitrate=3000\n";
        let own = stream_settings_from(no_mode, None).unwrap();
        assert_eq!(own.bitrate_kbps, 3000, "OBS defaults to Simple output");
    }

    /// Anything that can't be mirrored faithfully gives None, and the local
    /// config keeps its own defaults instead of guessing.
    #[test]
    fn settings_that_cannot_be_mirrored_are_not_guessed() {
        let unreadable = [
            (ADVANCED_BASIC_INI, None),
            (ADVANCED_BASIC_INI, Some("not json")),
            (ADVANCED_BASIC_INI, Some(r#"{"bitrate":6000,}"#)),
            (
                ADVANCED_BASIC_INI,
                Some(r#"{"bitrate":6000,"preset":"quality"}}"#),
            ),
            (ADVANCED_BASIC_INI, Some(r#"["bitrate",6000]"#)),
            (
                ADVANCED_BASIC_INI,
                Some(r#"{"rate_control":"CQP","cqp":20}"#),
            ),
            (ADVANCED_BASIC_INI, Some(r#"{"bitrate":0}"#)),
        ];
        for (ini, json) in unreadable {
            assert!(stream_settings_from(ini, json).is_none(), "{json:?}");
        }
        let no_encoder = ADVANCED_BASIC_INI.replace("Encoder=h264_texture_amf", "Encoder=");
        assert!(stream_settings_from(&no_encoder, Some(AMD_STREAM_ENCODER)).is_none());
        let simple_without_bitrate = "[Output]\nMode=Simple\n";
        assert!(stream_settings_from(simple_without_bitrate, None).is_none());
    }

    #[test]
    fn a_bom_prefixed_stream_encoder_file_still_reads() {
        let with_bom = format!("\u{feff}{AMD_STREAM_ENCODER}\r\n");
        let own = stream_settings_from(ADVANCED_BASIC_INI, Some(&with_bom)).unwrap();
        assert_eq!(own.encoder.unwrap().settings_json, AMD_STREAM_ENCODER);
    }

    const EB_ON_INI: &str =
        "\u{feff}[General]\nName=Main\n\n[Stream1]\nEnableMultitrackVideo=true\n";
    const INSTANTCLONE_SERVICE: &str = r#"{"type":"rtmp_common","settings":{"service":"InstantClone","server":"rtmp://localhost:1935/live","multitrack_video_configuration_url":"http://127.0.0.1:7799/obs/multitrack-config"}}"#;

    /// Enhanced Broadcasting counts only when OBS has it ticked and the service can use it.
    #[test]
    fn eb_is_wanted_only_when_ticked_on_a_service_that_delivers_it() {
        // eb ticked on the instantclone service is wanted
        {
            assert!(profile_wants_eb(EB_ON_INI, INSTANTCLONE_SERVICE));
            let spaced = INSTANTCLONE_SERVICE.replace("\":", "\": ");
            assert!(
                profile_wants_eb(EB_ON_INI, &spaced),
                "JSON spacing is not meaningful"
            );
        }
        // eb unticked or unset is not wanted
        {
            let off = EB_ON_INI.replace("=true", "=false");
            assert!(!profile_wants_eb(&off, INSTANTCLONE_SERVICE));
            assert!(!profile_wants_eb("[Stream1]\n", INSTANTCLONE_SERVICE));
        }
        // a service that cannot deliver eb is not wanted
        // Custom RTMP drops the config URL on load, so a leftover one there
        // can't turn EB on, and warning that EB "didn't apply" would be wrong.
        {
            let custom = INSTANTCLONE_SERVICE.replace("rtmp_common", "rtmp_custom");
            assert!(!profile_wants_eb(EB_ON_INI, &custom));
            let no_url = r#"{"type":"rtmp_common","settings":{"service":"InstantClone"}}"#;
            assert!(!profile_wants_eb(EB_ON_INI, no_url));
            let empty_url =
                INSTANTCLONE_SERVICE.replace("http://127.0.0.1:7799/obs/multitrack-config", "");
            assert!(!profile_wants_eb(EB_ON_INI, &empty_url));
        }
    }

    /// Insert, replace, create the section, and switching off without leaving a stray line.
    #[test]
    fn ini_set_writes_a_key_where_obs_reads_it() {
        // ini set inserts when section exists but key missing
        {
            let ini = "[General]\nName=Foo\n[Basic]\nProfile=A\n";
            let out = ini_set(ini, "General", "EnableCustomServerVodTrack", true);
            assert!(out.contains("EnableCustomServerVodTrack=true"));
            assert_eq!(
                ini_get(&out, "General", "EnableCustomServerVodTrack"),
                Some("true")
            );
            // Existing [Basic] block is preserved.
            assert_eq!(ini_get(&out, "Basic", "Profile"), Some("A"));
        }
        // ini set replaces existing key value
        {
            let ini = "[General]\nEnableCustomServerVodTrack=false\nName=Foo\n";
            let out = ini_set(ini, "General", "EnableCustomServerVodTrack", true);
            assert_eq!(
                ini_get(&out, "General", "EnableCustomServerVodTrack"),
                Some("true")
            );
            assert!(!out.contains("EnableCustomServerVodTrack=false"));
            // Sibling key not disturbed.
            assert_eq!(ini_get(&out, "General", "Name"), Some("Foo"));
        }
        // ini set creates section when absent
        {
            let ini = "[Basic]\nProfile=A\n";
            let out = ini_set(ini, "General", "EnableCustomServerVodTrack", true);
            assert_eq!(
                ini_get(&out, "General", "EnableCustomServerVodTrack"),
                Some("true")
            );
            assert_eq!(ini_get(&out, "Basic", "Profile"), Some("A"));
        }
        // ini set writes false in place when disabling
        {
            // Issue #9 follow-up: disabling REWRITES the value as `false`
            // instead of deleting the line. OBS's config_get_bool reads
            // `=false` and "key absent" identically, but rewriting the
            // line in place is robust against OBS shuffling the key into
            // a different [General] block on its own file rewrite -
            // wherever the key ended up, we just flip the value.
            let ini = "[General]\nEnableCustomServerVodTrack=true\nName=Foo\n";
            let out = ini_set(ini, "General", "EnableCustomServerVodTrack", false);
            assert_eq!(
                ini_get(&out, "General", "EnableCustomServerVodTrack"),
                Some("false")
            );
            assert!(!out.contains("EnableCustomServerVodTrack=true"));
            assert_eq!(ini_get(&out, "General", "Name"), Some("Foo"));
        }
        // ini set does not create false line when disabling a virgin file
        {
            // A user who has never toggled VOD audio on shouldn't end up
            // with a stray `EnableCustomServerVodTrack=false` line in
            // their user.ini just because the dashboard reconcile ran on
            // a clean state. Insertion is still gated to the enable path.
            let ini = "[General]\nName=Foo\n[Basic]\nProfile=A\n";
            let out = ini_set(ini, "General", "EnableCustomServerVodTrack", false);
            assert_eq!(out, ini, "no key found + disable should be a no-op");
        }
    }

    // ── service.json injection ─────────────────────────────────────

    fn fake_rtmp_custom_service_json() -> String {
        // Minimal but realistic shape OBS writes for a Custom RTMP service.
        r#"{
    "settings": {
        "key": "main",
        "server": "rtmp://127.0.0.1:1935/live",
        "use_auth": false
    },
    "type": "rtmp_custom"
}
"#
        .to_string()
    }

    /// Manually crafted legacy state from v0.1.0..0.1.2's broken
    /// inject path: an `rtmp_custom` service.json that was once
    /// patched by the old `inject_service_json_key` helper. Strip must
    /// remove our key without disturbing real OBS settings, so an
    /// upgrade from a polluted file ends up clean.
    fn legacy_polluted_rtmp_custom_service_json() -> String {
        r#"{
    "settings": {
        "multitrack_video_configuration_url": "http://127.0.0.1:7799/obs/multitrack-config",
        "key": "main",
        "server": "rtmp://127.0.0.1:1935/live",
        "use_auth": false
    },
    "type": "rtmp_custom"
}
"#
        .to_string()
    }

    #[test]
    fn strip_service_json_key_removes_only_our_key() {
        let polluted = legacy_polluted_rtmp_custom_service_json();
        let stripped = strip_service_json_key(&polluted, 7799);
        let doc = crate::json::parse(&stripped).expect("still valid JSON");
        assert_eq!(
            doc.path("settings.multitrack_video_configuration_url"),
            None
        );
        // Sibling settings stay untouched.
        let setting = |name: &str| {
            doc.path(&format!("settings.{name}"))
                .and_then(|v| v.as_str())
        };
        assert_eq!(setting("server"), Some("rtmp://127.0.0.1:1935/live"));
        assert_eq!(setting("key"), Some("main"));
    }

    /// OBS writes service.json on one line, and the key may be the last
    /// one: a line-based cut emptied the file or left a dangling comma.
    #[test]
    fn strip_service_json_key_keeps_one_line_files_valid() {
        let compact = r#"{"settings":{"key":"main","multitrack_video_configuration_url":"http://127.0.0.1:7799/obs/multitrack-config"},"type":"rtmp_custom"}"#;
        let stripped = strip_service_json_key(compact, 7799);
        assert_eq!(
            stripped,
            r#"{"settings":{"key":"main"},"type":"rtmp_custom"}"#
        );
    }

    /// The registered InstantClone service keeps its URL: that is how OBS
    /// finds the Enhanced Broadcasting config, not a leftover. Saving a
    /// destination used to empty this file.
    #[test]
    fn strip_service_json_key_leaves_the_registered_service_alone() {
        assert_eq!(
            strip_service_json_key(INSTANTCLONE_SERVICE, 7799),
            INSTANTCLONE_SERVICE
        );
        let with_bom = format!("\u{feff}{INSTANTCLONE_SERVICE}");
        assert_eq!(strip_service_json_key(&with_bom, 7799), with_bom);
    }

    #[test]
    fn strip_service_json_key_is_noop_when_not_present() {
        let original = fake_rtmp_custom_service_json();
        let stripped = strip_service_json_key(&original, 7799);
        assert_eq!(stripped, original);
    }

    // ── OBS user-config path resolution ─────────────────────────────
    //
    // Issue #9 (v0.1.2): the EnableCustomServerVodTrack flag was
    // being written to global.ini, but OBS 32 reads the VOD-track
    // gate from user.ini (via App()->GetUserConfig()). These tests
    // lock the prefer-user.ini behaviour so the file selection
    // can't silently regress back to global.ini in a future refactor.

    /// A test's own temp dir, removed when the test ends, pass or fail.
    struct ScratchDir(std::path::PathBuf);

    impl std::ops::Deref for ScratchDir {
        type Target = std::path::Path;
        fn deref(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fresh_obs_dir(label: &str) -> ScratchDir {
        // Each test gets its own dir so a tempdir cleanup race
        // between them can't make assertions flap.
        let dir = std::env::temp_dir().join(format!(
            "ic-obs-{}-{}-{}",
            label,
            std::process::id(),
            UNIQ_OBS.fetch_add(1, TestOrd::SeqCst),
        ));
        std::fs::create_dir_all(&dir).expect("fresh_obs_dir create_dir_all");
        ScratchDir(dir)
    }

    #[cfg(windows)]
    #[test]
    fn a_portable_obs_folder_is_found_from_its_exe() {
        let exe = Path::new(r"D:\Tools\OBS\bin\64bit\obs64.exe");
        assert_eq!(obs_root_of_exe(exe), Some(PathBuf::from(r"D:\Tools\OBS")));
        assert_eq!(obs_root_of_exe(Path::new(r"D:\obs64.exe")), None);
    }

    /// Streamlabs' engine is an obs64.exe too; with it open, OBS must not
    /// read as open.
    #[cfg(windows)]
    #[test]
    fn streamlabs_engine_is_no_obs() {
        let engine = Path::new(
            r"C:\Program Files\Streamlabs OBS\resources\app.asar.unpacked\node_modules\obs-studio-node\obs64.exe",
        );
        assert_eq!(obs_root_of_exe(engine), None);
    }

    /// With several OBS, a wait wins (the startup loop comes back for it),
    /// then a failure, then a repair.
    #[test]
    fn several_obs_report_the_most_pressing_refresh() {
        use RegistrationRefresh::*;
        assert_eq!(most_pressing(vec![NotNeeded, Repaired, Deferred]), Deferred);
        assert_eq!(
            most_pressing(vec![Repaired, Failed("x".into())]),
            Failed("x".into())
        );
        assert_eq!(most_pressing(vec![OtherInstance(7800), Repaired]), Repaired);
        assert_eq!(most_pressing(vec![]), NotNeeded);
    }

    /// The marker makes it portable whatever AppData holds; an installed OBS
    /// (no config folder of its own) never counts.
    #[cfg(windows)]
    #[test]
    fn only_an_obs_folder_with_its_own_config_counts_as_portable() {
        let root = fresh_obs_dir("portable");
        assert!(
            !is_portable_obs_root(&root),
            "no config folder: an installed OBS"
        );

        std::fs::create_dir_all(root.join(PORTABLE_CONFIG_DIR)).unwrap();
        std::fs::write(root.join("portable_mode.txt"), "").unwrap();
        assert!(is_portable_obs_root(&root));
    }

    /// OBS 31 moved the per-user file: the new one wins, the old one is the fallback.
    #[test]
    fn the_user_config_is_found_in_either_obs_layout() {
        // resolve user config prefers user ini when both files exist
        {
            let dir = fresh_obs_dir("prefer-user");
            let user = dir.join("user.ini");
            let global = dir.join("global.ini");
            std::fs::write(&user, "[General]\n").unwrap();
            std::fs::write(&global, "[General]\n").unwrap();

            let resolved = resolve_user_config_in(&dir).expect("must resolve");
            assert_eq!(
                resolved, user,
                "OBS 32+ keeps user.ini as the authoritative user config - \
                 writing to global.ini is the v0.1.0..0.1.2 bug from issue #9"
            );
        }
        // resolve user config falls back to global ini when user ini missing
        {
            let dir = fresh_obs_dir("fallback-global");
            let global = dir.join("global.ini");
            std::fs::write(&global, "[General]\n").unwrap();

            let resolved = resolve_user_config_in(&dir).expect("must resolve");
            assert_eq!(
                resolved, global,
                "pre-OBS-32 installs only have global.ini; the resolver \
                 must still find the user config there"
            );
        }
        // resolve user config returns none when neither exists
        {
            let dir = fresh_obs_dir("no-obs");
            assert!(
                resolve_user_config_in(&dir).is_none(),
                "no OBS install means no config to write to - caller \
                 should degrade gracefully, not error"
            );
        }
        // legacy global ini only reported when user ini present
        {
            // Case 1: OBS 32+ with both files - legacy global.ini is
            // dead state we want to clean up.
            let dir = fresh_obs_dir("legacy-both");
            let user = dir.join("user.ini");
            let global = dir.join("global.ini");
            std::fs::write(&user, "[General]\n").unwrap();
            std::fs::write(&global, "[General]\n").unwrap();
            assert_eq!(legacy_global_ini_in(&dir), Some(global));

            // Case 2: pre-32 install - global.ini IS the live config,
            // not legacy. The cleanup path must skip so we don't
            // strip the flag from the file OBS is actively reading.
            let dir = fresh_obs_dir("legacy-no-user");
            let global = dir.join("global.ini");
            std::fs::write(&global, "[General]\n").unwrap();
            assert!(legacy_global_ini_in(&dir).is_none());
        }
    }

    static UNIQ_OBS: TestUniq = TestUniq::new(0);
}
