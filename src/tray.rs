//! Windows system-tray icon. Hidden message-only window receives clicks
//! on the tray icon, pops a context menu, and signals shutdown back to
//! the tokio runtime via a oneshot when the user picks Quit.
//!
//! Runs on its own OS thread (Windows GUI requires a message loop on the
//! thread that created the window), so it stays out of tokio's
//! current-thread runtime entirely.
//!
//! Why we have it: with `windows_subsystem = "windows"` there is no
//! console for the user to close, and we still need an exit affordance
//! that doesn't require Task Manager.
//!
//! The menu is rebuilt every time the user clicks the icon, so the
//! status row and conditional items reflect *live* state (read from
//! the controller). Actions like "Cut delay" call straight into the
//! controller's sync methods - they're atomics-only, no runtime needed.

#![cfg(windows)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;

use tokio::sync::watch;

use crate::config::{self, Settings};
use crate::controller::Controller;

use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics,
    GetWindowLongPtrW, KillTimer, LoadIconW, LookupIconIdFromDirectoryEx, MessageBoxW,
    PostMessageW, PostQuitMessage, RegisterClassW, SetForegroundWindow, SetTimer,
    SetWindowLongPtrW, TrackPopupMenu, TranslateMessage, GWLP_USERDATA, HMENU, IDI_APPLICATION,
    IDYES, LR_DEFAULTCOLOR, MB_ICONWARNING, MB_YESNO, MF_DISABLED, MF_GRAYED, MF_SEPARATOR,
    MF_STRING, MSG, SM_CXSMICON, SM_CYSMICON, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RIGHTBUTTON,
    WM_APP, WM_COMMAND, WM_DESTROY, WM_HOTKEY, WM_LBUTTONUP, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
};

const TRAY_MSG: u32 = WM_APP + 1;
// Posted (from any thread) to ask the message loop to re-read hotkey
// bindings from settings and re-register them - the live-apply path when
// the user edits a binding in the dashboard.
const HOTKEY_RELOAD_MSG: u32 = WM_APP + 2;

// Posted (from any thread) with a problem message parked in
// `PENDING_BALLOON`, so the balloon is raised on the thread that owns the
// tray icon.
const BALLOON_MSG: u32 = WM_APP + 3;

// Fires when a capture suspension runs out, so hotkeys come back even if the
// dashboard that asked for it never says it is done.
const RESUME_TIMER_ID: usize = 1;

/// Text for the next balloon. A single slot: if two problems land back to
/// back, the newer one is the one worth showing.
static PENDING_BALLOON: crate::sync::Mutex<String> = crate::sync::Mutex::new(String::new());

/// RegisterHotKey id for an action: its 1-based position in
/// `config::ACTIONS` (0 is not a valid application hotkey id). Deriving both
/// directions from that one list is what keeps the id a binding is filed
/// under and the action a WM_HOTKEY runs from ever drifting apart.
fn hotkey_id(action: &str) -> Option<i32> {
    config::ACTIONS
        .iter()
        .position(|a| *a == action)
        .map(|i| i as i32 + 1)
}

/// Inverse of `hotkey_id`: the action a fired WM_HOTKEY id belongs to.
fn hotkey_action(id: i32) -> Option<&'static str> {
    // `checked_sub` rather than `id - 1`: wParam arrives from the OS, and a
    // stray message with i32::MIN would overflow (a debug-build panic) on
    // the way to a lookup that was never going to match anything.
    let index = usize::try_from(id.checked_sub(1)?).ok()?;
    config::ACTIONS.get(index).copied()
}

/// The tray window handle, published once the message-only window exists so
/// the tokio side can post `HOTKEY_RELOAD_MSG` to it. 0 means "not up yet",
/// in which case a reload request is a safe no-op.
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);

// Menu item IDs. Kept above 0x100 so they can't collide with system IDs.
const ID_OPEN_DASH: usize = 0x101;
const ID_OPEN_DOCK: usize = 0x102;
const ID_CUT_DELAY: usize = 0x103;
const ID_COPY_URL: usize = 0x104;
// 0x105 was "Launch OBS (VOD + EB)", a launcher from before the InstantClone
// service existed. It passed OBS `--config-url` to switch Enhanced
// Broadcasting on for a custom RTMP server, and wrote OBS's VOD-track flag on
// the way. Neither job is its any more: the registered service carries
// `multitrack_video_configuration_url` itself, so picking InstantClone in OBS
// IS Enhanced Broadcasting, and OBS 32.2 locked the built-in VOD Track to
// Custom services, so a VOD track now comes from the unlocker script. Left
// unused rather than recycled, in case an old build's menu message arrives.
const ID_QUIT: usize = 0x1FF;

/// The icon bytes are generated by build.rs (cyan rounded square + white
/// pause bars at 16/32/48 px). We embed the whole multi-resolution ICO
/// and let `LookupIconIdFromDirectoryEx` pick the right size at runtime.
static ICON_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icon.ico"));

/// Per-tray-window state, parked behind GWLP_USERDATA on the hidden
/// window so the WNDPROC can find it on every message.
struct TrayState {
    web_url: String,
    dock_url: String,
    ctrl: Arc<Controller>,
    settings: watch::Receiver<Settings>,
}

/// Spawn the tray on its own OS thread. Returns immediately; the thread
/// runs the message loop until the user picks Quit (or the process dies).
pub fn spawn(settings: watch::Receiver<Settings>, ctrl: Arc<Controller>) {
    let s = settings.borrow().clone();
    let web_url = format!("http://127.0.0.1:{}/", s.web_port);
    let dock_url = format!("http://127.0.0.1:{}/dock", s.web_port);
    std::thread::Builder::new()
        .name("instantclone-tray".into())
        .spawn(move || {
            if let Err(e) = run(web_url, dock_url, ctrl, settings) {
                eprintln!("[tray] init failed: {e}");
            }
        })
        .ok();
}

fn run(
    web_url: String,
    dock_url: String,
    ctrl: Arc<Controller>,
    settings: watch::Receiver<Settings>,
) -> Result<(), String> {
    unsafe {
        let h_instance = GetModuleHandleW(ptr::null());
        let class_name = wide("InstantCloneTrayClass");

        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: h_instance as _,
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        if RegisterClassW(&wc) == 0 {
            return Err("RegisterClassW failed".into());
        }

        let title = wide("InstantClone");
        // HWND_MESSAGE = -3 cast to ptr - message-only window, never visible.
        let parent: HWND = -3isize as HWND;
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            parent,
            ptr::null_mut(),
            h_instance as _,
            ptr::null(),
        );
        if hwnd.is_null() {
            return Err("CreateWindowExW failed".into());
        }

        // Park state on the window so the WNDPROC can find it.
        let state = Box::new(TrayState {
            web_url,
            dock_url,
            ctrl,
            settings,
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);

        // Load the embedded ICO. If anything goes wrong (corrupted bytes,
        // unsupported feature flag on this Windows version) fall back to
        // the generic application icon so the tray still appears.
        let h_icon =
            load_embedded_icon().unwrap_or_else(|| LoadIconW(ptr::null_mut(), IDI_APPLICATION));

        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = TRAY_MSG;
        nid.hIcon = h_icon;
        write_wide(&mut nid.szTip, "InstantClone - click for menu");

        if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
            return Err("Shell_NotifyIconW(NIM_ADD) failed".into());
        }

        // Publish the handle so `request_hotkey_reload` can post to us, then
        // bind whatever hotkeys the user already has configured.
        TRAY_HWND.store(hwnd as isize, Ordering::Release);
        register_hotkeys(hwnd);

        // Message loop until WM_QUIT (PostQuitMessage from the Quit handler
        // or DefWindowProc on WM_DESTROY).
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Tear down in reverse: stop the OS from routing hotkeys to a window
        // we're about to destroy, then drop the tray icon and state.
        TRAY_HWND.store(0, Ordering::Release);
        unregister_hotkeys(hwnd);
        Shell_NotifyIconW(NIM_DELETE, &nid);

        // Reclaim and drop the TrayState.
        let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayState;
        if !raw.is_null() {
            drop(Box::from_raw(raw));
        }
        DestroyWindow(hwnd);
    }
    Ok(())
}

/// Decode the embedded ICO into an HICON at the system's small-icon size.
/// Returns `None` on any FFI failure - caller falls back to the stock icon.
unsafe fn load_embedded_icon() -> Option<windows_sys::Win32::UI::WindowsAndMessaging::HICON> {
    let cx = GetSystemMetrics(SM_CXSMICON);
    let cy = GetSystemMetrics(SM_CYSMICON);
    // `1` = looking for an icon (not a cursor).
    let offset = LookupIconIdFromDirectoryEx(ICON_DATA.as_ptr(), 1, cx, cy, LR_DEFAULTCOLOR);
    if offset <= 0 {
        return None;
    }
    let off = offset as usize;
    if off >= ICON_DATA.len() {
        return None;
    }
    let h = CreateIconFromResourceEx(
        ICON_DATA.as_ptr().add(off),
        (ICON_DATA.len() - off) as u32,
        1,          // fIcon = true
        0x00030000, // version
        cx,
        cy,
        LR_DEFAULTCOLOR,
    );
    if h.is_null() {
        None
    } else {
        Some(h)
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // Tray icon: left- or right-click → show the menu near the cursor.
        TRAY_MSG => {
            let ev = lp as u32;
            if ev == WM_LBUTTONUP || ev == WM_RBUTTONUP {
                show_menu(hwnd);
            }
            0
        }
        // Global hotkey fired. wParam is the RegisterHotKey id from the
        // action table; run the matching delay action straight on the
        // controller (atomic stores, no runtime needed).
        WM_HOTKEY => {
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayState;
            if !state_ptr.is_null() {
                dispatch_hotkey(&*state_ptr, wp as i32);
            }
            0
        }
        // A hotkey or MIDI action was refused; say so where the user is.
        BALLOON_MSG => {
            show_balloon(hwnd);
            0
        }
        // Live re-apply: the user edited a binding, or started / finished
        // recording one.
        HOTKEY_RELOAD_MSG => {
            unregister_hotkeys(hwnd);
            register_hotkeys(hwnd);
            sync_resume_timer(hwnd);
            0
        }
        // A capture suspension ran out without anyone telling us it was
        // over. Put the hotkeys back.
        WM_TIMER => {
            if wp == RESUME_TIMER_ID {
                KillTimer(hwnd, RESUME_TIMER_ID);
                unregister_hotkeys(hwnd);
                register_hotkeys(hwnd);
            }
            0
        }
        // Menu selection.
        WM_COMMAND => {
            let id = wp & 0xFFFF;
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayState;
            if state_ptr.is_null() {
                return 0;
            }
            let state = &*state_ptr;
            match id {
                ID_OPEN_DASH => {
                    let _ = open_url(&state.web_url);
                }
                ID_OPEN_DOCK => {
                    let _ = open_url(&state.dock_url);
                }
                ID_CUT_DELAY => {
                    // Direct sync call - Controller methods are atomic stores,
                    // no runtime needed.
                    state.ctrl.stop_delay();
                }
                ID_COPY_URL => {
                    let url = obs_rtmp_url(&state.settings.borrow());
                    let _ = set_clipboard_text(hwnd, &url);
                }
                ID_QUIT => {
                    // Quitting kills every egress, so if OBS is publishing
                    // right now a stray click here drops the live stream.
                    // Confirm in that case only - when nothing is streaming,
                    // quit stays a single click.
                    if state.ctrl.ingest_alive() && !confirm_quit_while_live(hwnd) {
                        return 0;
                    }
                    // Signal the main loop to tear down egress cleanly and exit
                    // - the same path the web Quit route uses. Idempotent, so a
                    // second click is harmless.
                    state.ctrl.request_quit();
                    PostQuitMessage(0);
                }
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Ask the tray's message loop to re-read hotkey bindings and re-register
/// them. Called from the tokio side after a settings change. Safe no-op
/// until the tray window exists (TRAY_HWND still 0) - the initial bind
/// happens in `run` regardless.
pub fn request_hotkey_reload() {
    let hwnd = TRAY_HWND.load(Ordering::Acquire);
    if hwnd != 0 {
        // SAFETY: PostMessageW is thread-safe; a stale handle (window torn
        // down between the load and the post) is handled by the OS, which
        // just fails the post. TRAY_HWND is set back to 0 before teardown.
        unsafe {
            PostMessageW(hwnd as HWND, HOTKEY_RELOAD_MSG, 0, 0);
        }
    }
}

/// (Re)register every configured hotkey against the tray window. Unbound
/// or malformed entries are skipped; a combo another app already owns logs
/// a friendly line rather than failing silently, so the user knows why a
/// key does nothing.
unsafe fn register_hotkeys(hwnd: HWND) {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayState;
    if state_ptr.is_null() {
        return;
    }
    let state = &*state_ptr;
    // The dashboard is recording a combo. Leaving ours registered would eat
    // the keypress before the browser ever saw it - the user would be unable
    // to record any bound combo, and would trigger its action instead. The
    // conflict list is left as it was: it describes bindings, not this
    // moment, and clearing it would flash a warning off and back on.
    if state.ctrl.hotkeys_suspended() {
        return;
    }
    // Clone so we don't hold the watch borrow across the log calls below.
    let hotkeys = state.settings.borrow().hotkeys.clone();
    let mut conflicts = Vec::new();
    for (action, combo) in hotkeys.entries() {
        if combo.is_empty() {
            continue;
        }
        let Some((mods, vk)) = config::parse_hotkey(combo) else {
            continue;
        };
        let Some(id) = hotkey_id(action) else {
            continue;
        };
        // MOD_NOREPEAT so holding the combo fires once, not on autorepeat.
        if RegisterHotKey(hwnd, id, mods | MOD_NOREPEAT, vk) == 0 {
            // Duplicates inside our own set are impossible (`Hotkeys::set`
            // moves a combo rather than sharing it), so a refusal here can
            // only be another app holding the combo.
            state.ctrl.log(format!(
                "[hotkey] {combo} is already in use by another app - {action} not bound"
            ));
            conflicts.push(action.to_string());
        }
    }
    // Always published, including the empty case, so the dashboard clears a
    // warning the moment the user picks a combo that is actually free.
    state.ctrl.set_hotkey_conflicts(conflicts);
}

/// Keep the resume timer in step with the suspension: armed for whatever is
/// left of the window, cancelled as soon as hotkeys are live again.
unsafe fn sync_resume_timer(hwnd: HWND) {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayState;
    if state_ptr.is_null() {
        return;
    }
    let remaining = (*state_ptr).ctrl.hotkeys_suspend_remaining_ms();
    if remaining > 0 {
        // +250 ms so the timer lands after the deadline, never a tick before.
        SetTimer(hwnd, RESUME_TIMER_ID, remaining + 250, None);
    } else {
        KillTimer(hwnd, RESUME_TIMER_ID);
    }
}

/// Drop every hotkey registration. Unregistering an id that was never
/// registered is harmless, so this can run unconditionally on reload and
/// teardown.
unsafe fn unregister_hotkeys(hwnd: HWND) {
    for i in 0..config::ACTIONS.len() {
        UnregisterHotKey(hwnd, i as i32 + 1);
    }
}

/// Run the delay action bound to a fired hotkey. The RegisterHotKey id maps
/// back to an action name via `hotkey_action`; the shared controller method
/// carries the semantics (identical to the MIDI path).
fn dispatch_hotkey(state: &TrayState, id: i32) {
    let Some(action) = hotkey_action(id) else {
        return;
    };
    let default_ms = state.settings.borrow().auto_arm_delay_ms;
    // A refusal is the one case worth interrupting for: the streamer is in a
    // game, pressed a key, and nothing happened. Successes stay silent so a
    // balloon never lands on a display-captured scene during normal use.
    if let Some(problem) = state.ctrl.run_named_action(action, default_ms, "hotkey") {
        notify_problem(&problem);
    }
}

/// Raise a tray balloon about an action that could not run. Callable from
/// any thread: the text is parked and the tray thread does the drawing.
pub fn notify_problem(text: &str) {
    let hwnd = TRAY_HWND.load(Ordering::Acquire);
    if hwnd == 0 {
        return;
    }
    *PENDING_BALLOON.lock() = text.to_string();
    // SAFETY: same contract as `request_hotkey_reload` - PostMessageW is
    // thread-safe and a stale handle just fails the post.
    unsafe {
        PostMessageW(hwnd as HWND, BALLOON_MSG, 0, 0);
    }
}

/// Show the parked message as a tray balloon. Reuses the icon we already
/// own (uID 1), so this is a modify rather than a second icon.
unsafe fn show_balloon(hwnd: HWND) {
    let text = std::mem::take(&mut *PENDING_BALLOON.lock());
    if text.is_empty() {
        return;
    }
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_WARNING;
    write_wide(&mut nid.szInfoTitle, "InstantClone");
    write_wide(&mut nid.szInfo, &text);
    Shell_NotifyIconW(NIM_MODIFY, &nid);
}

/// Modal Yes/No shown when the user hits Quit while OBS is publishing.
/// Returns true only if they confirm. Blocks the tray thread, which is
/// fine - a tray click has nothing else in flight.
unsafe fn confirm_quit_while_live(hwnd: HWND) -> bool {
    let text = wide(
        "OBS is streaming through InstantClone right now. Quitting stops the \
         delay and drops every destination.\n\nQuit anyway?",
    );
    let title = wide("Quit InstantClone?");
    MessageBoxW(
        hwnd,
        text.as_ptr(),
        title.as_ptr(),
        MB_YESNO | MB_ICONWARNING,
    ) == IDYES
}

unsafe fn show_menu(hwnd: HWND) {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayState;
    if state_ptr.is_null() {
        return;
    }
    let state = &*state_ptr;

    let menu: HMENU = CreatePopupMenu();
    if menu.is_null() {
        return;
    }

    // Top: live status header. Disabled item, no action - just a glance.
    let status = wide(&status_label(&state.ctrl));
    AppendMenuW(
        menu,
        MF_STRING | MF_DISABLED | MF_GRAYED,
        0,
        status.as_ptr(),
    );
    AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());

    // "Cut delay" only when the delay is actually live. Otherwise greyed.
    let cut_label = wide("Cut delay");
    let cut_flags = if state.ctrl.phase() == "active" {
        MF_STRING
    } else {
        MF_STRING | MF_DISABLED | MF_GRAYED
    };
    AppendMenuW(menu, cut_flags, ID_CUT_DELAY, cut_label.as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());

    let open = wide("Open dashboard");
    let dock = wide("Open OBS dock");
    let copy = wide("Copy RTMP URL");
    AppendMenuW(menu, MF_STRING, ID_OPEN_DASH, open.as_ptr());
    AppendMenuW(menu, MF_STRING, ID_OPEN_DOCK, dock.as_ptr());
    AppendMenuW(menu, MF_STRING, ID_COPY_URL, copy.as_ptr());

    AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
    let quit = wide("Quit InstantClone");
    AppendMenuW(menu, MF_STRING, ID_QUIT, quit.as_ptr());

    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    // SetForegroundWindow is required for TrackPopupMenu to behave -
    // without it the menu can vanish on first outside click.
    SetForegroundWindow(hwnd);
    TrackPopupMenu(
        menu,
        TPM_BOTTOMALIGN | TPM_LEFTALIGN | TPM_RIGHTBUTTON,
        pt.x,
        pt.y,
        0,
        hwnd,
        ptr::null(),
    );
    // Post a dummy message so the menu actually closes on outside-click -
    // standard Win32 dance.
    PostMessageW(hwnd, 0, 0, 0);
    DestroyMenu(menu);
}

/// One-line summary of the controller's current state. Read on every
/// menu open so it's always fresh.
fn status_label(ctrl: &Controller) -> String {
    let phase = ctrl.phase();
    match phase {
        "active" => {
            let s = (ctrl.current_delay_ms() + 500) / 1000;
            format!("Status: ACTIVE - {s}s delay")
        }
        "ready" => {
            let s = (ctrl.armed_delay_ms() + 500) / 1000;
            format!("Status: ARMED - {s}s ready to activate")
        }
        "preparing" => {
            let armed = ctrl.armed_delay_ms().max(1) as f32;
            let fill = ctrl.buffer_fill_ms() as f32;
            let pct = ((fill / armed) * 100.0).min(99.0) as u32;
            format!("Status: BUFFERING - {pct}%")
        }
        _ => {
            if ctrl.ingest_alive() {
                "Status: LIVE (no delay)".into()
            } else {
                "Status: idle (no source)".into()
            }
        }
    }
}

/// Reconstruct the same rtmp URL the dashboard shows in the OBS tab.
fn obs_rtmp_url(s: &Settings) -> String {
    let host = if s.ingest_bind_all {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    };
    format!("rtmp://{}:{}/live", host, s.ingest_port)
}

/// RAII wrapper around an HGLOBAL. Frees on Drop unless ownership is
/// transferred (call `.release()` after a successful `SetClipboardData`
/// since the system takes over the handle at that point).
///
/// Previous version of `set_clipboard_text` had three early-return paths
/// between `GlobalAlloc` and `SetClipboardData` that all leaked the
/// HGLOBAL. This struct closes that hole structurally.
struct HglobalGuard(HANDLE);

impl HglobalGuard {
    /// Take the HGLOBAL out of the guard. Caller is now responsible for
    /// it (typically because they just handed it to the OS).
    fn release(mut self) -> HANDLE {
        let h = self.0;
        self.0 = std::ptr::null_mut();
        h
    }
}

impl Drop for HglobalGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                GlobalFree(self.0);
            }
        }
    }
}

/// RAII wrapper around the Windows clipboard: ensures `CloseClipboard`
/// runs on every exit path. Constructed via `open(hwnd)` which returns
/// `None` if `OpenClipboard` fails.
struct ClipboardGuard;

impl ClipboardGuard {
    unsafe fn open(hwnd: HWND) -> Option<Self> {
        if OpenClipboard(hwnd) == 0 {
            None
        } else {
            Some(Self)
        }
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            CloseClipboard();
        }
    }
}

/// Put UTF-16 text on the Windows clipboard. Best-effort: failures are
/// silent (clipboard may be locked by another app). The RAII guards
/// above guarantee no HGLOBAL leak on any error path.
unsafe fn set_clipboard_text(hwnd: HWND, text: &str) -> Result<(), ()> {
    let _clip = ClipboardGuard::open(hwnd).ok_or(())?;
    if EmptyClipboard() == 0 {
        return Err(());
    }

    let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = utf16.len() * 2;
    let h_mem = HglobalGuard(GlobalAlloc(GMEM_MOVEABLE, bytes));
    if h_mem.0.is_null() {
        return Err(());
    }

    // Copy in. If GlobalLock fails (rare), h_mem's Drop runs GlobalFree.
    let dst = GlobalLock(h_mem.0) as *mut u16;
    if dst.is_null() {
        return Err(());
    }
    ptr::copy_nonoverlapping(utf16.as_ptr(), dst, utf16.len());
    GlobalUnlock(h_mem.0);

    // SetClipboardData transfers ownership ONLY on success. If it
    // returns null we still own the HGLOBAL, so let h_mem's Drop free it.
    if SetClipboardData(CF_UNICODETEXT as u32, h_mem.0).is_null() {
        return Err(());
    }
    // System now owns the HGLOBAL - release the guard so Drop is a no-op.
    let _ = h_mem.release();
    Ok(())
}

fn open_url(url: &str) -> std::io::Result<std::process::Child> {
    std::process::Command::new("cmd")
        .args(["/c", "start", "", url])
        .spawn()
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Copy `s` into a fixed-size UTF-16 field, NUL-terminated, truncating to
/// leave room for the terminator. Used for the tray tooltip and for the
/// balloon's title and body, all of which are fixed-cap arrays in
/// NOTIFYICONDATAW. Splitting a surrogate pair at the boundary is
/// acceptable: it renders as a tofu glyph, it does not crash.
fn write_wide(dst: &mut [u16], s: &str) {
    if dst.is_empty() {
        return;
    }
    let mut v: Vec<u16> = OsStr::new(s).encode_wide().collect();
    v.truncate(dst.len().saturating_sub(1));
    v.push(0);
    dst[..v.len()].copy_from_slice(&v);
}

#[cfg(test)]
mod tests {
    use super::{hotkey_action, hotkey_id};
    use crate::config;

    /// The id a hotkey is registered under and the action a fired WM_HOTKEY
    /// runs come from the same list, so this is really asking that the two
    /// directions are inverses - the property that used to depend on two
    /// hand-kept tables staying in the same order.
    #[test]
    fn hotkey_ids_round_trip_to_their_action() {
        for action in config::ACTIONS {
            let id = hotkey_id(action).expect("every action has an id");
            assert!(id >= 1, "RegisterHotKey rejects id 0");
            assert_eq!(hotkey_action(id), Some(action));
        }
        assert_eq!(hotkey_id("not-an-action"), None);
        assert_eq!(hotkey_action(0), None, "no action is filed under 0");
        assert_eq!(hotkey_action(-1), None);
        assert_eq!(hotkey_action(config::ACTIONS.len() as i32 + 1), None);
    }
}
