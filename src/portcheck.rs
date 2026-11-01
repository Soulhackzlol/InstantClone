//! Startup port pre-flight: detect if the configured RTMP / web ports are
//! already in use before we try to bind them.
//!
//! The bind checks (`is_port_free`, `find_free_port`, `wait_until_bindable`)
//! are pure `std::net` and run on every platform. The extras that identify
//! the owning process (IpHelper) and prompt the user (MessageBox) are
//! Windows-only, gated below; the caller in main.rs supplies a headless
//! stderr path for those on other platforms.
//!
//! Why we have it: a silent bind-loop on a busy port is the worst possible
//! first-run UX - on Windows there's no console (`windows_subsystem =
//! "windows"`), on a headless VPS there's no one watching a hung process.

use std::net::TcpListener;

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::ptr;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, NO_ERROR};
#[cfg(windows)]
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
#[cfg(windows)]
use windows_sys::Win32::Networking::WinSock::{ntohs, AF_INET};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDNO, IDYES, MB_DEFBUTTON2, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_YESNO,
    MB_YESNOCANCEL, MESSAGEBOX_RESULT, MESSAGEBOX_STYLE,
};

/// Header struct of the variable-length table returned by
/// `GetExtendedTcpTable(TCP_TABLE_OWNER_PID_LISTENER)`. We never
/// instantiate this - we cast a `Vec<u8>` buffer to it.
#[cfg(windows)]
#[repr(C)]
struct MibTcpTableOwnerPid {
    num_entries: u32,
    table: [MIB_TCPROW_OWNER_PID; 1],
}

/// Pid + full executable path of the process owning a given listening
/// TCP port. Returns `None` if no listener is on that port, or if the
/// OS-level lookup failed entirely.
#[cfg(windows)]
pub fn find_process_on_port(port: u16) -> Option<(u32, Option<String>)> {
    unsafe {
        // First call: get required buffer size.
        let mut size: u32 = 0;
        GetExtendedTcpTable(
            ptr::null_mut(),
            &mut size,
            0, // bOrder = FALSE
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
        if size == 0 {
            return None;
        }

        let mut buf = vec![0u8; size as usize];
        let r = GetExtendedTcpTable(
            buf.as_mut_ptr() as *mut _,
            &mut size,
            0,
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
        if r != NO_ERROR {
            return None;
        }

        let table = &*(buf.as_ptr() as *const MibTcpTableOwnerPid);
        let n = table.num_entries as usize;
        let rows = table.table.as_ptr();
        for i in 0..n {
            let row = &*rows.add(i);
            // `dwLocalPort` is a DWORD with the port in network byte
            // order packed into the low 16 bits. Use ntohs to convert.
            let row_port = ntohs(row.dwLocalPort as u16);
            if row_port == port {
                let pid = row.dwOwningPid;
                return Some((pid, process_name(pid)));
            }
        }
        None
    }
}

/// Best-effort full executable path for a PID. `None` if the OS denies
/// us access (e.g. PID belongs to an elevated process) - the caller
/// still has the PID to show.
#[cfg(windows)]
pub(crate) fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut size: u32 = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        // The full path: for another InstantClone, its folder is the one
        // detail that tells the user which copy it is.
        Some(String::from_utf16_lossy(&buf[..size as usize]))
    }
}

/// The file name part of an executable path, for showing in a dialog.
#[cfg(windows)]
fn exe_basename(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Whether `path` is an InstantClone exe. By file name, since a portable
/// copy can sit in any folder: `instantclone.exe`, or a renamed download
/// such as `InstantClone-0.1.16.exe`.
#[cfg(windows)]
fn is_instantclone_exe(path: &str) -> bool {
    exe_basename(path)
        .to_ascii_lowercase()
        .starts_with("instantclone")
}

/// Whether another running InstantClone (not this process) listens on
/// `port`. Used to leave OBS's registration alone when it points at a copy
/// that is running right now.
///
/// Windows names the owner, so this is exact there. Elsewhere there is no
/// cheap way to ask, so anything listening counts: the caller only uses the
/// answer to decide NOT to rewrite something, which is the safe direction to
/// be wrong in.
pub fn port_held_by_other_instance(port: u16) -> bool {
    #[cfg(windows)]
    {
        find_process_on_port(port).is_some_and(|(pid, exe)| {
            pid != std::process::id() && exe.as_deref().is_some_and(is_instantclone_exe)
        })
    }
    #[cfg(not(windows))]
    {
        !is_port_free(&format!("127.0.0.1:{port}"))
    }
}

/// True iff we can bind a TCP listener at `addr` right now. Released
/// immediately. Cheap pre-flight check.
pub fn is_port_free(addr: &str) -> bool {
    TcpListener::bind(addr).is_ok()
}

/// Poll until `addr` becomes bindable, up to `timeout`. Returns `true` the
/// instant a bind succeeds, `false` if the whole window elapses still busy.
///
/// Used on a restart / self-update relaunch: the outgoing instance can take a
/// moment to release its listening socket (Windows may even still attribute
/// the port to the now-dead PID), and we'd rather wait that out silently than
/// pop the "switch port?" modal mid-handoff. The window is generous because a
/// genuine conflict is rare and a spurious modal on every restart is worse.
pub fn wait_until_bindable(addr: &str, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if is_port_free(addr) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

/// Find the first port in `[start ..= start+range]` that we can bind
/// on `host`. Returns `None` if every port in the window is busy.
pub fn find_free_port(host: &str, start: u16, range: u16) -> Option<u16> {
    for offset in 0..=range {
        let port = start.checked_add(offset)?;
        if is_port_free(&format!("{}:{}", host, port)) {
            return Some(port);
        }
    }
    None
}

/// Which of this copy's two ports is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortRole {
    /// Where OBS sends the stream (RTMP).
    Stream,
    /// Where the dashboard opens.
    Dashboard,
}

impl PortRole {
    /// The technical name, for the log.
    pub fn label(self) -> &'static str {
        match self {
            PortRole::Stream => "RTMP port",
            PortRole::Dashboard => "Web port",
        }
    }

    /// In plain words, for a dialog.
    #[cfg(windows)]
    fn plain(self) -> &'static str {
        match self {
            PortRole::Stream => "the port OBS sends your stream to",
            PortRole::Dashboard => "the port the dashboard opens on",
        }
    }
}

#[cfg(windows)]
pub enum ConflictChoice {
    /// Run on this port instead.
    SwitchPort(u16),
    /// Use the InstantClone that is already running: open its dashboard
    /// (when its port is known) and quit.
    UseRunning(Option<u16>),
    Quit,
}

/// Who holds a port this copy wants.
#[cfg(windows)]
#[derive(Debug, PartialEq, Eq)]
enum Holder<'a> {
    /// Another InstantClone: its exe, or None while it is still starting
    /// and hasn't bound the port yet (see `instance_lock::reserve_port`).
    Copy(Option<&'a str>),
    /// Any other program, by its file name when Windows tells it.
    Program(Option<&'a str>),
}

#[cfg(windows)]
fn holder(owner: Option<&(u32, Option<String>)>, starting_copy: bool) -> Holder<'_> {
    match owner {
        Some((_, Some(exe))) if is_instantclone_exe(exe) => Holder::Copy(Some(exe)),
        Some((_, exe)) => Holder::Program(exe.as_deref().map(exe_basename)),
        None if starting_copy => Holder::Copy(None),
        None => Holder::Program(None),
    }
}

/// Set once the user confirmed a second InstantClone: its other port moves
/// too without asking the same question again.
#[cfg(windows)]
static SECOND_COPY_CONFIRMED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Ask the user what to do about a taken port.
///
/// Another InstantClone almost always means InstantClone was opened twice,
/// so the first answer opens the one already running. Starting a second copy
/// (another channel, its own settings) takes a second, explicit yes.
#[cfg(windows)]
pub fn ask_user(
    role: PortRole,
    port: u16,
    owner: Option<(u32, Option<String>)>,
    proposed_port: Option<u16>,
    starting_copy: bool,
) -> ConflictChoice {
    match holder(owner.as_ref(), starting_copy) {
        Holder::Copy(exe) => ask_about_copy(exe, proposed_port),
        Holder::Program(name) => ask_about_program(role, port, name, proposed_port),
    }
}

#[cfg(windows)]
fn ask_about_copy(exe: Option<&str>, proposed_port: Option<u16>) -> ConflictChoice {
    use std::sync::atomic::Ordering;
    if SECOND_COPY_CONFIRMED.load(Ordering::Relaxed) {
        return proposed_port.map_or_else(no_port_for_second_copy, ConflictChoice::SwitchPort);
    }
    let flags = MB_YESNOCANCEL | MB_ICONINFORMATION;
    match message_box(
        "InstantClone is already open",
        &already_open_message(exe),
        flags,
    ) {
        IDYES => ConflictChoice::UseRunning(exe.and_then(dashboard_port_of)),
        IDNO => confirm_second_copy(exe, proposed_port),
        _ => ConflictChoice::Quit,
    }
}

#[cfg(windows)]
fn confirm_second_copy(exe: Option<&str>, proposed_port: Option<u16>) -> ConflictChoice {
    let Some(port) = proposed_port else {
        return no_port_for_second_copy();
    };
    let here = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.display().to_string()))
        .unwrap_or_default();
    // No is the default: a stray Enter keeps a single InstantClone.
    let flags = MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2;
    match message_box(
        "Start a second InstantClone?",
        &second_copy_message(&here),
        flags,
    ) {
        IDYES => {
            SECOND_COPY_CONFIRMED.store(true, std::sync::atomic::Ordering::Relaxed);
            ConflictChoice::SwitchPort(port)
        }
        _ => ConflictChoice::UseRunning(exe.and_then(dashboard_port_of)),
    }
}

#[cfg(windows)]
fn no_port_for_second_copy() -> ConflictChoice {
    show_error(
        "InstantClone",
        "There's no free port left for a second InstantClone. Close the other \
         InstantClone, or the program using its ports, and try again.",
    );
    ConflictChoice::Quit
}

#[cfg(windows)]
fn ask_about_program(
    role: PortRole,
    port: u16,
    name: Option<&str>,
    proposed_port: Option<u16>,
) -> ConflictChoice {
    let text = program_message(role, port, name, proposed_port);
    let Some(new_port) = proposed_port else {
        show_error("InstantClone", &text);
        return ConflictChoice::Quit;
    };
    match message_box("InstantClone", &text, MB_YESNO | MB_ICONWARNING) {
        IDYES => ConflictChoice::SwitchPort(new_port),
        _ => ConflictChoice::Quit,
    }
}

/// The folder an exe runs from: what tells two InstantClones apart.
#[cfg(windows)]
fn folder_of(exe: &str) -> &str {
    exe.rsplit_once(['\\', '/']).map_or(exe, |(dir, _)| dir)
}

/// The dashboard port of the InstantClone running from `exe`, from the
/// settings beside it, where a copy keeps them unless told otherwise.
#[cfg(windows)]
fn dashboard_port_of(exe: &str) -> Option<u16> {
    let settings = std::path::Path::new(folder_of(exe)).join("instantclone.config.json");
    crate::config::Settings::load(&settings)
        .ok()
        .map(|s| s.web_port)
}

/// The first question: InstantClone is open already, use that one?
#[cfg(windows)]
fn already_open_message(exe: Option<&str>) -> String {
    match exe {
        Some(exe) => format!(
            "InstantClone is already open, from this folder:\n{}\n\n\
             Yes: open the InstantClone that is already running.\n\
             No: start a second InstantClone as well.\n\
             Cancel: do nothing.",
            folder_of(exe)
        ),
        None => "InstantClone is already starting.\n\n\
                 Yes: let that one open.\n\
                 No: start a second InstantClone as well.\n\
                 Cancel: do nothing."
            .to_string(),
    }
}

/// The second question, before a second copy starts.
#[cfg(windows)]
fn second_copy_message(here: &str) -> String {
    format!(
        "A second InstantClone is only needed to stream two channels at once, \
         each with its own settings. This one keeps its settings in:\n{here}\n\n\
         It uses different ports, so OBS has to be set up for it on its own: \
         its dashboard shows how.\n\n\
         Yes: start the second InstantClone.\n\
         No: open the one already running."
    )
}

/// What to say when another program holds the port.
#[cfg(windows)]
fn program_message(
    role: PortRole,
    port: u16,
    name: Option<&str>,
    proposed_port: Option<u16>,
) -> String {
    let who = name.map_or_else(
        || "Another program".to_string(),
        |n| format!("Another program, {n},"),
    );
    match proposed_port {
        Some(new_port) => {
            let after = match role {
                PortRole::Stream => {
                    "OBS needs the new address then: the dashboard's OBS tab shows it.".to_string()
                }
                PortRole::Dashboard => {
                    format!("The dashboard then opens at http://127.0.0.1:{new_port}/")
                }
            };
            format!(
                "{who} is using {} ({port}).\n\n\
                 Use {new_port} instead and continue? InstantClone remembers it. {after}",
                role.plain()
            )
        }
        None => format!(
            "{who} is using {} ({port}), and the next ports up to {} are taken too.\n\n\
             Close that program and start InstantClone again.",
            role.plain(),
            port.saturating_add(9)
        ),
    }
}

#[cfg(windows)]
fn message_box(title: &str, body: &str, flags: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    let title = wide(title);
    let body = wide(body);
    unsafe { MessageBoxW(ptr::null_mut(), body.as_ptr(), title.as_ptr(), flags) }
}

/// Pop a one-shot native error dialog. Used by main.rs for fatal
/// cold-start failures that would otherwise leave the user staring at
/// a closed (or never-opened) console - buffer file unwritable, etc.
/// Windows-only; other platforms print to stderr at the call site.
#[cfg(windows)]
pub fn show_error(title: &str, body: &str) {
    let title_w = wide(title);
    let body_w = wide(body);
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            body_w.as_ptr(),
            title_w.as_ptr(),
            MB_OK | MB_ICONWARNING,
        );
    }
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Bind on an OS-chosen ephemeral port (port 0) so the test doesn't
    /// clash with anything else on the dev box.
    fn bind_eph() -> (TcpListener, u16) {
        let l = TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
        let p = l.local_addr().unwrap().port();
        (l, p)
    }

    #[test]
    fn is_port_free_distinguishes_held_from_open() {
        let (held, port) = bind_eph();
        assert!(
            !is_port_free(&format!("127.0.0.1:{port}")),
            "held port must NOT report as free"
        );
        drop(held);
        // Brief breath for the OS to release the socket. On Windows the
        // socket lingers briefly after drop; the loop tolerates this.
        for _ in 0..20 {
            if is_port_free(&format!("127.0.0.1:{port}")) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("released port did not become free");
    }

    #[test]
    fn wait_until_bindable_returns_when_socket_released() {
        use std::time::Duration;
        let (held, port) = bind_eph();
        let addr = format!("127.0.0.1:{port}");
        // Release the socket after a short hold, on another thread.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            drop(held);
        });
        assert!(
            wait_until_bindable(&addr, Duration::from_secs(5)),
            "must report bindable once the holder releases within the window"
        );
    }

    #[test]
    fn wait_until_bindable_times_out_on_a_held_port() {
        use std::time::Duration;
        let (_held, port) = bind_eph(); // held for the whole test
        let addr = format!("127.0.0.1:{port}");
        assert!(
            !wait_until_bindable(&addr, Duration::from_millis(400)),
            "a permanently held port must time out as not bindable"
        );
    }

    #[test]
    fn find_free_port_skips_held_and_returns_next() {
        const WINDOW: u16 = 50;
        // Hold a known port and ask find_free_port to start there. It
        // should jump to a later one. The OS can hand out an ephemeral port
        // within WINDOW of 65535, where the scan has no room to move on and
        // `port + WINDOW` would overflow, so draw again until it has room.
        let (held, port) = loop {
            let (listener, port) = bind_eph();
            if port <= u16::MAX - WINDOW {
                break (listener, port);
            }
        };
        let found = find_free_port("127.0.0.1", port, WINDOW)
            .expect("some port in the window must be free");
        assert_ne!(found, port, "must skip the held port");
        assert!(found > port && found <= port + WINDOW);
        drop(held);
    }

    /// Who holds the port decides which question is asked.
    #[cfg(windows)]
    #[test]
    fn the_holder_is_told_apart() {
        let copy = (
            4242,
            Some(r"C:\Streams\Second\InstantClone.exe".to_string()),
        );
        assert_eq!(
            holder(Some(&copy), false),
            Holder::Copy(Some(r"C:\Streams\Second\InstantClone.exe"))
        );
        let nginx = (77, Some(r"C:\Tools\nginx.exe".to_string()));
        assert_eq!(
            holder(Some(&nginx), false),
            Holder::Program(Some("nginx.exe"))
        );
        // Claimed by a copy that is starting and hasn't bound it yet.
        assert_eq!(holder(None, true), Holder::Copy(None));
        assert_eq!(holder(None, false), Holder::Program(None));

        assert!(is_instantclone_exe(r"D:\x\instantclone-0.1.16.exe"));
        assert!(!is_instantclone_exe(r"D:\instantclone\obs64.exe"));
    }

    /// InstantClone opened twice: the first question names the folder of the
    /// one running and offers it first; ports and process ids stay out.
    #[cfg(windows)]
    #[test]
    fn opened_twice_offers_the_running_one_first() {
        let text = already_open_message(Some(r"C:\Streams\Second\InstantClone.exe"));
        assert!(text.contains(r"C:\Streams\Second"), "{text}");
        assert!(
            !text.contains("InstantClone.exe"),
            "the folder, not the exe: {text}"
        );
        assert!(
            text.contains("Yes: open the InstantClone that is already running"),
            "{text}"
        );
        assert!(text.contains("No: start a second InstantClone"), "{text}");
        let starting = already_open_message(None);
        assert!(starting.contains("already starting"), "{starting}");
        for text in [text, starting, second_copy_message(r"C:\Here")] {
            assert!(!text.contains("PID") && !text.contains("port :"), "{text}");
            assert!(!text.contains('\u{2014}'), "no em dashes in dialog copy");
        }
    }

    /// Another program: what the port is for, in plain words.
    #[cfg(windows)]
    #[test]
    fn another_program_is_explained_plainly() {
        let text = program_message(PortRole::Stream, 1935, Some("nginx.exe"), Some(1936));
        assert!(
            text.contains("Another program, nginx.exe, is using"),
            "{text}"
        );
        assert!(
            text.contains("the port OBS sends your stream to (1935)"),
            "{text}"
        );
        assert!(text.contains("Use 1936 instead"), "{text}");
        let text = program_message(PortRole::Dashboard, 7799, None, Some(7800));
        assert!(text.contains("http://127.0.0.1:7800/"), "{text}");
        let text = program_message(PortRole::Stream, 1935, None, None);
        assert!(text.contains("up to 1944"), "{text}");
        assert!(!text.contains('\u{2014}'), "no em dashes in dialog copy");
    }

    #[cfg(windows)]
    #[test]
    fn the_running_copys_dashboard_is_read_beside_its_exe() {
        let dir = std::env::temp_dir().join(format!("ic-portcheck-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("instantclone.config.json"), "web_port=17123\n").unwrap();
        let exe = dir.join("instantclone.exe").display().to_string();
        assert_eq!(dashboard_port_of(&exe), Some(17123));
        assert_eq!(
            dashboard_port_of(r"C:\nowhere\at\all\instantclone.exe"),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// This process listening is never "another" InstantClone.
    #[cfg(windows)]
    #[test]
    fn our_own_port_is_not_another_instance() {
        let (_held, port) = bind_eph();
        assert!(!port_held_by_other_instance(port));
    }

    #[cfg(windows)]
    #[test]
    fn find_process_on_port_finds_self_when_holding_socket() {
        // Bind a port in this very process, then ask the FFI who owns
        // it. The answer must be our own PID. Process name resolution
        // can vary (test binary path), so we only assert PID equality.
        let (held, port) = bind_eph();
        let owner = find_process_on_port(port);
        drop(held);

        let (pid, _name) = owner.expect("FFI should locate the listener");
        assert_eq!(
            pid,
            std::process::id(),
            "owning PID must be this test process"
        );
    }
}
