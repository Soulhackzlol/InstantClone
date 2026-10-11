//! Windows system-tray icon. A hidden window receives clicks on the tray
//! icon, pops a context menu, and signals shutdown back to the tokio
//! runtime when the user picks Quit or Windows ends the session.
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
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
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
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_NOSOUND,
    NIIF_RESPECT_QUIET_TIME, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIN_BALLOONUSERCLICK,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    GetSystemMetrics, GetWindowLongPtrW, KillTimer, LoadIconW, LookupIconIdFromDirectoryEx,
    MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, SetTimer, SetWindowLongPtrW, TrackPopupMenu, TranslateMessage,
    GWLP_USERDATA, HMENU, IDI_APPLICATION, IDYES, LR_DEFAULTCOLOR, MB_ICONWARNING, MB_YESNO,
    MF_DISABLED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, SM_CXSMICON, SM_CYSMICON,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RIGHTBUTTON, WM_APP, WM_COMMAND, WM_DESTROY, WM_ENDSESSION,
    WM_HOTKEY, WM_LBUTTONUP, WM_QUERYENDSESSION, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_POPUP,
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

// Posted (from any thread) when the ports the tooltip names have changed.
const TOOLTIP_MSG: u32 = WM_APP + 4;

// Posted (from any thread) when the icon's colour has changed.
const ICON_MSG: u32 = WM_APP + 5;

/// How long Windows ending the session waits for the clean teardown. The
/// teardown takes about a second; Windows itself gives up on a window that
/// has not answered WM_ENDSESSION after about five.
const END_SESSION_WAIT: std::time::Duration = std::time::Duration::from_millis(4_500);

/// Set by the runtime once its shutdown epilogue has run, so a session
/// ending can hold the process until the platforms got their goodbye.
static SHUTDOWN_DONE: (std::sync::Mutex<bool>, std::sync::Condvar) =
    (std::sync::Mutex::new(false), std::sync::Condvar::new());

// Fires when a capture suspension runs out, so hotkeys come back even if the
// dashboard that asked for it never says it is done.
const RESUME_TIMER_ID: usize = 1;

/// What a tray balloon is about, which sets its icon, sound and title.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum BalloonKind {
    /// An action the user just took could not run (a hotkey, a menu item).
    #[default]
    Problem,
    /// Something to know that the user didn't just ask for, like an armed
    /// delay the buffer can't hold: no sound.
    Notice,
    /// An update: no sound, held back while Windows is set to quiet, and a
    /// click opens System > About.
    Update,
}

/// Text and kind of the next balloon. A single slot: if two land back to
/// back, the newer one is the one worth showing.
static PENDING_BALLOON: crate::sync::Mutex<(String, BalloonKind)> =
    crate::sync::Mutex::new((String::new(), BalloonKind::Problem));

/// The balloon on screen announces an update, so clicking it opens
/// System > About.
static BALLOON_IS_UPDATE: AtomicBool = AtomicBool::new(false);

/// Integration shortcuts register from this id up, one per combo, in the
/// order of `INTEGRATION_KEYS`. Far above the delay actions' ids.
const INTEGRATION_HOTKEY_BASE: i32 = 100;
/// More combos than anyone binds; keeps a broken config from registering
/// hundreds of system-wide hotkeys.
const MAX_INTEGRATION_HOTKEYS: usize = 64;

/// The combos integrations listen for, as registered: id
/// `INTEGRATION_HOTKEY_BASE + n` is `INTEGRATION_KEYS[n]`.
static INTEGRATION_KEYS: crate::sync::Mutex<Vec<String>> = crate::sync::Mutex::new(Vec::new());

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

/// The shell broadcasts `TaskbarCreated` when the notification area appears -
/// at logon once explorer is ready, and again every time explorer restarts.
/// Registered at startup, read in the window procedure; 0 until then.
static TASKBAR_CREATED_MSG: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

// Menu item IDs. Kept above 0x100 so they can't collide with system IDs.
const ID_OPEN_DASH: usize = 0x101;
const ID_OPEN_DOCK: usize = 0x102;
const ID_CUT_DELAY: usize = 0x103;
const ID_COPY_URL: usize = 0x104;
const ID_END_HOLD: usize = 0x106;
const ID_UPDATE: usize = 0x107;
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
pub(crate) static ICON_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icon.ico"));

/// The ICO in the colour this copy shows (see
/// `instance_lock::show_tray_color`): the embedded one for the brand blue.
fn icon_data() -> std::borrow::Cow<'static, [u8]> {
    let color = crate::instance_lock::shown_tray_color() as usize;
    match crate::instance_lock::TRAY_COLORS
        .get(color)
        .filter(|_| color != 0)
    {
        Some(&(_, rgb)) => std::borrow::Cow::Owned(recolor_icon(ICON_DATA, rgb)),
        None => std::borrow::Cow::Borrowed(ICON_DATA),
    }
}

/// The square's colour as build.rs draws it, BGRA.
const ICON_CYAN_BGRA: [u8; 4] = [0xfa, 0xc8, 0x5a, 0xff];

/// `ico` with its cyan square repainted `rgb`; the white bars and the
/// transparent corners stay. build.rs draws the square in that one exact
/// colour with no anti-aliasing, so matching it finds every pixel of it.
pub(crate) fn recolor_icon(ico: &[u8], [r, g, b]: [u8; 3]) -> Vec<u8> {
    let mut out = ico.to_vec();
    let count = ico
        .get(4..6)
        .map_or(0, |n| u16::from_le_bytes([n[0], n[1]]));
    for entry in 0..count as usize {
        let at = 6 + 16 * entry;
        let Some(dir) = ico.get(at..at + 16) else {
            break;
        };
        // 0 means 256 in an ICONDIRENTRY.
        let side = if dir[0] == 0 { 256 } else { dir[0] as usize };
        let image = u32::from_le_bytes([dir[12], dir[13], dir[14], dir[15]]) as usize;
        // The pixels follow the 40-byte BITMAPINFOHEADER.
        let start = image + 40;
        let end = (start + side * side * 4).min(out.len());
        let Some(pixels) = out.get_mut(start..end) else {
            continue;
        };
        for pixel in pixels.as_chunks_mut::<4>().0 {
            if *pixel == ICON_CYAN_BGRA {
                *pixel = [b, g, r, 0xff];
            }
        }
    }
    out
}

/// Per-tray-window state, parked behind GWLP_USERDATA on the hidden
/// window so the WNDPROC can find it on every message.
struct TrayState {
    /// The folder this setup lives in: what tells two copies' icons apart.
    label: String,
    ctrl: Arc<Controller>,
    settings: watch::Receiver<Settings>,
}

/// Spawn the tray on its own OS thread. Returns immediately; the thread
/// runs the message loop until the user picks Quit (or the process dies).
/// `label` names this setup (its folder) in the tooltip and the menu.
pub fn spawn(settings: watch::Receiver<Settings>, ctrl: Arc<Controller>, label: String) {
    std::thread::Builder::new()
        .name("instantclone-tray".into())
        .spawn(move || {
            if let Err(e) = run(label, ctrl, settings) {
                log_line!("[tray] init failed: {e}");
            }
        })
        .ok();
}

fn run(
    label: String,
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
        // A top-level window that is never shown, not a message-only one
        // (HWND_MESSAGE). Message-only windows get no broadcasts, and the
        // two that matter here are broadcasts: WM_QUERYENDSESSION /
        // WM_ENDSESSION at logoff and shutdown, without which Windows just
        // killed the process mid-stream, and TaskbarCreated after explorer
        // restarts. WS_EX_TOOLWINDOW keeps it out of Alt-Tab and the taskbar
        // should anything ever show it.
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            h_instance as _,
            ptr::null(),
        );
        if hwnd.is_null() {
            return Err("CreateWindowExW failed".into());
        }

        // Park state on the window so the WNDPROC can find it. Keep a handle
        // to the controller first: the state is moved in, and the icon path
        // below still needs somewhere to report to.
        let log_ctrl = ctrl.clone();
        let tip = tooltip_text(&label, &settings.borrow());
        let state = Box::new(TrayState {
            label,
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
        write_wide(&mut nid.szTip, &tip);

        // Publish the handle and bind the hotkeys BEFORE touching the shell,
        // and never let the icon decide whether the rest of this thread runs.
        //
        // `Shell_NotifyIconW(NIM_ADD)` legitimately fails at logon when the
        // notification area does not exist yet - which is exactly the
        // start-with-Windows path this release exists to fix. Aborting here
        // used to mean no icon, no hotkeys (TRAY_HWND stayed 0, so nothing
        // could ever register), no balloons, and with `windows_subsystem =
        // "windows"` no console and no way to quit: an invisible process with
        // the headline feature dead and nothing on screen to say why.
        TRAY_HWND.store(hwnd as isize, Ordering::Release);
        register_hotkeys(hwnd);

        // Ask the shell to tell us when the notification area shows up, then
        // try once now. Either the icon appears, or TaskbarCreated brings us
        // back - which also covers explorer restarting mid-session, after
        // which the icon used to be gone for good.
        TASKBAR_CREATED_MSG.store(
            RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            Ordering::Release,
        );
        if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
            log_ctrl.log(
                "[tray] the notification area is not ready; hotkeys are live and the icon \
                 will appear when it is",
            );
        }

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
    let icon = icon_data();
    let offset = LookupIconIdFromDirectoryEx(icon.as_ptr(), 1, cx, cy, LR_DEFAULTCOLOR);
    if offset <= 0 {
        return None;
    }
    let off = offset as usize;
    if off >= icon.len() {
        return None;
    }
    let h = CreateIconFromResourceEx(
        icon.as_ptr().add(off),
        (icon.len() - off) as u32,
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
            } else if ev == NIN_BALLOONUSERCLICK && BALLOON_IS_UPDATE.load(Ordering::Acquire) {
                let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayState;
                if !state_ptr.is_null() {
                    crate::open_browser(&about_url(&dashboard_url(
                        &(*state_ptr).settings.borrow(),
                        "",
                    )));
                }
            }
            0
        }
        // The notification area (re)appeared: at logon after explorer was
        // ready, or because explorer restarted and took our icon with it.
        // Re-adding is idempotent from our side - the shell has no record of
        // the old one either way.
        m if m != 0 && m == TASKBAR_CREATED_MSG.load(Ordering::Acquire) => {
            readd_tray_icon(hwnd);
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
        TOOLTIP_MSG => {
            refresh_tooltip(hwnd);
            0
        }
        ICON_MSG => {
            refresh_icon(hwnd);
            0
        }
        // Logoff or shutdown is asking. Never object: the user is leaving.
        WM_QUERYENDSESSION => 1,
        // The session is ending for real. Run the same clean teardown as
        // Quit (every destination gets its goodbye, the trace and counters
        // are flushed, the buffer file removed) and hold Windows here until
        // it is done, because the process may be killed the moment this
        // returns.
        WM_ENDSESSION => {
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayState;
            if wp != 0 && !state_ptr.is_null() {
                (*state_ptr)
                    .ctrl
                    .log("[tray] Windows is ending the session - shutting down cleanly");
                (*state_ptr).ctrl.request_quit();
                wait_for_shutdown(END_SESSION_WAIT);
            }
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
                // Built from the settings at the click, not at startup: the
                // dashboard port can move live.
                ID_OPEN_DASH => {
                    crate::open_browser(&dashboard_url(&state.settings.borrow(), ""));
                }
                ID_OPEN_DOCK => {
                    crate::open_browser(&dashboard_url(&state.settings.borrow(), "dock"));
                }
                ID_UPDATE => {
                    crate::open_browser(&about_url(&dashboard_url(&state.settings.borrow(), "")));
                }
                // The same path as a hotkey: logged, shown on the dashboard,
                // and saved, so a cut from here survives a restart too.
                ID_CUT_DELAY => run_action(state, "cut"),
                ID_END_HOLD => run_action(state, "end_hold"),
                // The address the dashboard shows, `localhost` included: an
                // IP literal fails in OBS under one of its IP Family settings.
                ID_COPY_URL => {
                    let url = state.settings.borrow().obs_url();
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
                "[hotkey] {combo} is already in use by another app (or another InstantClone) - \
                 {action} not bound"
            ));
            conflicts.push(action.to_string());
        }
    }
    // Always published, including the empty case, so the dashboard clears a
    // warning the moment the user picks a combo that is actually free.
    state.ctrl.set_hotkey_conflicts(conflicts);
    register_integration_hotkeys(hwnd, state, &hotkeys);
}

/// Register the combos integrations start from. A combo a delay action
/// already holds stays the delay action's (the dashboard refuses to save
/// that pair, so this only guards a hand-edited config).
unsafe fn register_integration_hotkeys(hwnd: HWND, state: &TrayState, delay: &config::Hotkeys) {
    let (mut keys, _) =
        crate::integrations::model::shortcuts(&state.settings.borrow().integrations);
    keys.retain(|combo| !delay.entries().iter().any(|(_, bound)| bound == combo));
    // Past the id range nothing registers: say so rather than drop them.
    let mut refused = keys.split_off(keys.len().min(MAX_INTEGRATION_HOTKEYS));
    if !refused.is_empty() {
        state.ctrl.log(format!(
            "[hotkey] only {MAX_INTEGRATION_HOTKEYS} integration hotkeys can be live; {} more won't start anything",
            refused.len()
        ));
    }
    for (n, combo) in keys.iter().enumerate() {
        let Some((mods, vk)) = config::parse_hotkey(combo) else {
            continue;
        };
        if RegisterHotKey(
            hwnd,
            INTEGRATION_HOTKEY_BASE + n as i32,
            mods | MOD_NOREPEAT,
            vk,
        ) == 0
        {
            state.ctrl.log(format!(
                "[hotkey] {combo} is already in use by another app - the integration using it won't start from it"
            ));
            refused.push(combo.clone());
        }
    }
    // Always published, the empty case too, so a fixed key clears its warning.
    if let Some(integrations) = state.ctrl.integrations() {
        integrations.set_refused_keys(refused);
    }
    *INTEGRATION_KEYS.lock() = keys;
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
    let registered = std::mem::take(&mut *INTEGRATION_KEYS.lock());
    for n in 0..registered.len() {
        UnregisterHotKey(hwnd, INTEGRATION_HOTKEY_BASE + n as i32);
    }
}

/// Run the delay action bound to a fired hotkey. The RegisterHotKey id maps
/// back to an action name via `hotkey_action`; the shared controller method
/// carries the semantics (identical to the MIDI path).
fn dispatch_hotkey(state: &TrayState, id: i32) {
    if id >= INTEGRATION_HOTKEY_BASE {
        let combo = usize::try_from(id - INTEGRATION_HOTKEY_BASE)
            .ok()
            .and_then(|n| INTEGRATION_KEYS.lock().get(n).cloned());
        if let (Some(combo), Some(integrations)) = (combo, state.ctrl.integrations()) {
            integrations.shortcut(crate::integrations::Shortcut::Hotkey, &combo);
        }
        return;
    }
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

/// Run a delay action picked from the menu, through the same path as a
/// hotkey, and say so if it couldn't run.
fn run_action(state: &TrayState, action: &str) {
    let default_ms = state.settings.borrow().auto_arm_delay_ms;
    if let Some(problem) = state.ctrl.run_named_action(action, default_ms, "tray") {
        notify_problem(&problem);
    }
}

/// Raise a tray balloon about an action that could not run. Callable from
/// any thread: the text is parked and the tray thread does the drawing.
pub fn notify_problem(text: &str) {
    post_balloon(text, BalloonKind::Problem);
}

/// Announce an update with a quiet balloon that opens System > About when
/// clicked. False when there is no tray to show it (not up yet, or the post
/// failed), so the caller can try again later.
pub fn notify_update(text: &str) -> bool {
    post_balloon(text, BalloonKind::Update)
}

/// Raise a quiet balloon (no sound, info icon) about something the user
/// should know but didn't just ask for, like an armed delay the buffer
/// can't hold. Callable from any thread.
pub fn notify_notice(text: &str) {
    post_balloon(text, BalloonKind::Notice);
}

pub(crate) fn post_balloon(text: &str, kind: BalloonKind) -> bool {
    let hwnd = TRAY_HWND.load(Ordering::Acquire);
    if hwnd == 0 {
        return false;
    }
    *PENDING_BALLOON.lock() = (text.to_string(), kind);
    // SAFETY: same contract as `request_hotkey_reload` - PostMessageW is
    // thread-safe and a stale handle just fails the post.
    unsafe { PostMessageW(hwnd as HWND, BALLOON_MSG, 0, 0) != 0 }
}

/// The dashboard opened on System > About, where updates are installed.
fn about_url(web_url: &str) -> String {
    format!("{web_url}#about")
}

/// Show the parked message as a tray balloon. Reuses the icon we already
/// own (uID 1), so this is a modify rather than a second icon.
unsafe fn show_balloon(hwnd: HWND) {
    let (text, kind) = std::mem::take(&mut *PENDING_BALLOON.lock());
    if text.is_empty() {
        return;
    }
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_INFO;
    // A notice or an update is news, not a problem: no sound. An update is
    // also held back while the user has asked Windows for quiet (focus
    // assist, a game in full screen).
    let (flags, title) = match kind {
        BalloonKind::Problem => (NIIF_WARNING, "InstantClone"),
        BalloonKind::Notice => (NIIF_INFO | NIIF_NOSOUND, "InstantClone"),
        BalloonKind::Update => (
            NIIF_INFO | NIIF_NOSOUND | NIIF_RESPECT_QUIET_TIME,
            "InstantClone update",
        ),
    };
    BALLOON_IS_UPDATE.store(kind == BalloonKind::Update, Ordering::Release);
    nid.dwInfoFlags = flags;
    write_wide(&mut nid.szInfoTitle, title);
    write_wide(&mut nid.szInfo, &text);
    // Refused when the icon isn't in the notification area (explorer not
    // ready yet, or it restarted and the re-add is still pending). Said in
    // the log, so a notification that never showed can be told apart from
    // one Windows kept quiet (Do not disturb, notifications off for apps).
    if Shell_NotifyIconW(NIM_MODIFY, &nid) == 0 {
        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayState;
        if !state_ptr.is_null() {
            (*state_ptr)
                .ctrl
                .log(format!("[tray] Windows refused a notification: {text}"));
        }
    }
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

    // Top: which setup this is, then its live status. Disabled items, no
    // action - just a glance. With two copies running, the first line is
    // what says which icon is which.
    let header = wide(&menu_header(&state.label, &state.settings.borrow()));
    AppendMenuW(
        menu,
        MF_STRING | MF_DISABLED | MF_GRAYED,
        0,
        header.as_ptr(),
    );
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
    // Crash protection: ending the hold ends the stream everywhere, so the
    // item only exists while a hold is on air.
    let end_hold_label = wide("End crash protection (ends the stream)");
    if state.ctrl.hold_active() {
        AppendMenuW(menu, MF_STRING, ID_END_HOLD, end_hold_label.as_ptr());
    }
    AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());

    // An update a check found, one click from the updater. Gone once the
    // user turns the checks off.
    let update = crate::update_check::available_label()
        .filter(|_| state.settings.borrow().update_check_enabled)
        .map(|label| wide(&label));
    if let Some(label) = &update {
        AppendMenuW(menu, MF_STRING, ID_UPDATE, label.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
    }

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

/// Put our icon back in the notification area. Called when the shell says
/// the area exists, which is both the logon race and an explorer restart.
///
/// Builds the descriptor fresh rather than caching one: the only fields that
/// matter are the window, the id and the callback message, and re-deriving
/// them is cheaper than keeping a `NOTIFYICONDATAW` alive across threads.
unsafe fn readd_tray_icon(hwnd: HWND) {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayState;
    if state_ptr.is_null() {
        return;
    }
    let h_icon =
        load_embedded_icon().unwrap_or_else(|| LoadIconW(ptr::null_mut(), IDI_APPLICATION));
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = TRAY_MSG;
    nid.hIcon = h_icon;
    let state = &*state_ptr;
    write_wide(
        &mut nid.szTip,
        &tooltip_text(&state.label, &state.settings.borrow()),
    );
    Shell_NotifyIconW(NIM_ADD, &nid);
}

/// Rewrite the tooltip after the ports it names have moved.
unsafe fn refresh_tooltip(hwnd: HWND) {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayState;
    if state_ptr.is_null() {
        return;
    }
    let state = &*state_ptr;
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_TIP;
    write_wide(
        &mut nid.szTip,
        &tooltip_text(&state.label, &state.settings.borrow()),
    );
    Shell_NotifyIconW(NIM_MODIFY, &nid);
}

/// Ask the tray to rewrite its tooltip. Safe no-op before the tray exists.
pub fn request_tooltip_refresh() {
    let hwnd = TRAY_HWND.load(Ordering::Acquire);
    if hwnd != 0 {
        // SAFETY: same contract as `request_hotkey_reload`.
        unsafe {
            PostMessageW(hwnd as HWND, TOOLTIP_MSG, 0, 0);
        }
    }
}

/// Redraw the icon in the colour this copy now shows.
unsafe fn refresh_icon(hwnd: HWND) {
    let Some(h_icon) = load_embedded_icon() else {
        return;
    };
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_ICON;
    nid.hIcon = h_icon;
    Shell_NotifyIconW(NIM_MODIFY, &nid);
    // The shell keeps a copy of its own.
    DestroyIcon(h_icon);
}

/// Ask the tray to redraw its icon. Safe no-op before the tray exists.
pub fn request_icon_refresh() {
    let hwnd = TRAY_HWND.load(Ordering::Acquire);
    if hwnd != 0 {
        // SAFETY: same contract as `request_hotkey_reload`.
        unsafe {
            PostMessageW(hwnd as HWND, ICON_MSG, 0, 0);
        }
    }
}

/// Called by the runtime when its shutdown epilogue has finished.
pub fn note_shutdown_done() {
    let (done, wake) = &SHUTDOWN_DONE;
    if let Ok(mut done) = done.lock() {
        *done = true;
    }
    wake.notify_all();
}

/// Block until `note_shutdown_done`, or `timeout`. Runs on the tray thread
/// while Windows waits on WM_ENDSESSION; the runtime's teardown is on
/// another thread, so it never waits on this one.
fn wait_for_shutdown(timeout: std::time::Duration) {
    let (done, wake) = &SHUTDOWN_DONE;
    if let Ok(guard) = done.lock() {
        let _ = wake.wait_timeout_while(guard, timeout, |done| !*done);
    }
}

/// The tooltip: which setup (its folder) and the ports it listens on, so
/// two copies side by side are told apart at a hover.
fn tooltip_text(label: &str, s: &Settings) -> String {
    format!(
        "InstantClone - {label}\nRTMP :{}, dashboard :{}",
        s.ingest_port, s.web_port
    )
}

/// First line of the menu: which setup this icon belongs to.
fn menu_header(label: &str, s: &Settings) -> String {
    format!("InstantClone - {label} (RTMP :{})", s.ingest_port)
}

/// The dashboard (`""`) or the dock (`"dock"`) at the current web port.
fn dashboard_url(s: &Settings, page: &str) -> String {
    format!("http://127.0.0.1:{}/{page}", s.web_port)
}

/// One-line summary of the controller's current state. Read on every
/// menu open so it's always fresh.
fn status_label(ctrl: &Controller) -> String {
    if let Some(status) = ctrl.hold_status() {
        return format!(
            "Status: RECONNECT SCREEN - {} left",
            crate::crash_hold::minutes_seconds(status.remaining)
        );
    }
    let phase = ctrl.phase();
    match phase {
        "active" => {
            let s = (ctrl.current_delay_ms() + 500) / 1000;
            format!("Status: ACTIVE - {s}s delay")
        }
        "ready" => {
            let armed = ctrl.armed_delay_ms();
            let runs = ctrl.delay_runs_ms(armed);
            let s = (runs + 500) / 1000;
            if runs < armed {
                let armed_s = (armed + 500) / 1000;
                format!("Status: ARMED - {s}s ready (the buffer can't hold {armed_s}s)")
            } else {
                format!("Status: ARMED - {s}s ready to activate")
            }
        }
        "preparing" => {
            // Toward what will run: a delay the buffer can't hold fills
            // to what it holds, never to what was armed.
            let goal = ctrl.delay_runs_ms(ctrl.armed_delay_ms()).max(1) as f32;
            let fill = ctrl.buffer_fill_ms() as f32;
            let pct = ((fill / goal) * 100.0).min(99.0) as u32;
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
    use super::{dashboard_url, hotkey_action, hotkey_id, menu_header, tooltip_text};
    use super::{recolor_icon, ICON_CYAN_BGRA, ICON_DATA};
    use crate::config;

    /// A second copy's icon swaps the cyan square for its colour at every
    /// size, and leaves the bars, the corners and the ICO's layout alone.
    /// (The pixels sit at an offset that isn't a multiple of 4 in the file,
    /// so they are counted at every byte.)
    #[test]
    fn a_coloured_icon_repaints_only_the_square() {
        let (_, purple) = crate::instance_lock::TRAY_COLORS[1];
        let colored = recolor_icon(ICON_DATA, purple);
        assert_eq!(colored.len(), ICON_DATA.len());
        let cyan = ICON_DATA
            .windows(4)
            .filter(|p| *p == ICON_CYAN_BGRA)
            .count();
        assert!(
            cyan > 16 * 16 / 2,
            "the embedded icon is drawn in brand cyan"
        );
        let purple_bgra = [purple[2], purple[1], purple[0], 0xff];
        assert_eq!(
            colored.windows(4).filter(|p| *p == purple_bgra).count(),
            cyan,
            "every cyan pixel, and only those, is repainted"
        );
        assert!(!colored.windows(4).any(|p| p == ICON_CYAN_BGRA));
        let changed = ICON_DATA
            .iter()
            .zip(&colored)
            .filter(|(a, b)| a != b)
            .count();
        assert!(changed <= cyan * 3, "nothing outside the square moved");
    }

    /// The menu's links follow the settings in force, so a live port change
    /// never leaves them pointing at the old port, and two copies' icons
    /// say which setup and ports each one is.
    #[test]
    fn links_and_labels_follow_the_current_settings() {
        let mut s = config::Settings::defaults();
        s.web_port = 8080;
        s.ingest_port = 1935;
        assert_eq!(dashboard_url(&s, ""), "http://127.0.0.1:8080/");
        s.web_port = 8181;
        assert_eq!(dashboard_url(&s, "dock"), "http://127.0.0.1:8181/dock");
        assert_eq!(
            s.obs_url(),
            "rtmp://localhost:1935/live",
            "what Copy RTMP URL copies"
        );
        let tip = tooltip_text("Second channel", &s);
        assert_eq!(
            tip,
            "InstantClone - Second channel\nRTMP :1935, dashboard :8181"
        );
        assert!(tip.encode_utf16().count() < 128, "fits the tooltip field");
        assert_eq!(
            menu_header("Second channel", &s),
            "InstantClone - Second channel (RTMP :1935)"
        );
    }

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
