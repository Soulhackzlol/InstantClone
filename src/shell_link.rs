//! Windows `.lnk` shortcuts, written in-process through the shell's own
//! ShellLink COM object.
//!
//! This replaces a hidden PowerShell call that did the same thing:
//! antivirus engines read "unsigned app runs hidden PowerShell that drops a
//! shortcut" as malware persistence, and Windows Defender quarantined the
//! app mid-stream for it (issue #101).
//!
//! windows-sys has the ShellLink class id but no interface bindings, so the
//! two interfaces used here are declared by hand. Slot order comes from
//! ShObjIdl_core.h (IShellLinkW) and ObjIdl.h (IPersistFile); unused slots
//! are kept as plain placeholders so every used method sits at its index.

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;
use std::ptr::NonNull;

use windows_sys::core::{GUID, HRESULT};
use windows_sys::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows_sys::Win32::UI::Shell::ShellLink;

const IID_ISHELLLINKW: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
const IID_IPERSISTFILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);

type SetText = unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT;
type SetTextAndNumber = unsafe extern "system" fn(*mut c_void, *const u16, i32) -> HRESULT;
type QueryInterface =
    unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
type Release = unsafe extern "system" fn(*mut c_void) -> u32;

#[repr(C)]
struct ShellLinkVtbl {
    query_interface: QueryInterface,
    _add_ref: usize,
    _release: Release,
    // GetPath, GetIDList, SetIDList, GetDescription, SetDescription,
    // GetWorkingDirectory.
    _unused_3_to_8: [usize; 6],
    set_working_directory: SetText,
    _get_arguments: usize,
    set_arguments: SetText,
    // GetHotkey, SetHotkey, GetShowCmd, SetShowCmd, GetIconLocation.
    _unused_12_to_16: [usize; 5],
    set_icon_location: SetTextAndNumber,
    _set_relative_path: usize,
    _resolve: usize,
    set_path: SetText,
}

#[repr(C)]
struct PersistFileVtbl {
    _query_interface: usize,
    _add_ref: usize,
    _release: Release,
    // GetClassID, IsDirty, Load.
    _unused_3_to_5: [usize; 3],
    save: SetTextAndNumber,
}

/// The first three slots of every COM vtable (IUnknown's), so any interface
/// can be released the same way.
#[repr(C)]
struct UnknownVtbl {
    _query_interface: usize,
    _add_ref: usize,
    release: Release,
}

/// One COM interface pointer, released when dropped. Both pointers are
/// checked when it is adopted, so neither is ever read while null.
struct Interface<V> {
    /// The object: it starts with a pointer to its vtable.
    object: NonNull<*const V>,
    vtbl: NonNull<V>,
}

impl<V> Interface<V> {
    /// Wrap a pointer COM just handed over (one reference, ours to release).
    /// None when it handed over no object, or one without a vtable.
    ///
    /// Safety: `raw` is null or a live COM object whose vtable is `V`.
    unsafe fn adopt(raw: *mut c_void) -> Option<Self> {
        let object = NonNull::new(raw.cast::<*const V>())?;
        let vtbl = NonNull::new(object.as_ptr().read().cast_mut())?;
        Some(Self { object, vtbl })
    }

    fn vtbl(&self) -> &V {
        // Checked non-null in `adopt`; the object keeps it alive until drop.
        unsafe { self.vtbl.as_ref() }
    }

    fn this(&self) -> *mut c_void {
        self.object.as_ptr().cast()
    }
}

impl<V> Drop for Interface<V> {
    fn drop(&mut self) {
        // Every vtable here is #[repr(C)] and starts with IUnknown's slots.
        let unknown = self.vtbl.cast::<UnknownVtbl>();
        unsafe { (unknown.as_ref().release)(self.this()) };
    }
}

/// The error for a COM call that reported success but handed over nothing.
fn no_object(step: &str) -> io::Error {
    io::Error::other(format!("could not {step} (no object came back)"))
}

/// Write a shortcut at `lnk` that runs `exe` with `arguments` from `workdir`,
/// showing the exe's own icon.
pub fn create(lnk: &Path, exe: &Path, arguments: &str, workdir: &Path) -> io::Result<()> {
    unsafe {
        // S_OK and S_FALSE both need a matching CoUninitialize. A thread
        // already in the other apartment mode (RPC_E_CHANGED_MODE) can still
        // use ShellLink, which supports both, so that is not an error.
        let initialized = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) >= 0;
        let result = write(lnk, exe, arguments, workdir);
        if initialized {
            CoUninitialize();
        }
        result
    }
}

unsafe fn write(lnk: &Path, exe: &Path, arguments: &str, workdir: &Path) -> io::Result<()> {
    let mut raw = std::ptr::null_mut();
    check(
        CoCreateInstance(
            &ShellLink,
            std::ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_ISHELLLINKW,
            &mut raw,
        ),
        "create the shortcut",
    )?;
    let link =
        Interface::<ShellLinkVtbl>::adopt(raw).ok_or_else(|| no_object("create the shortcut"))?;
    let calls = link.vtbl();
    check(
        (calls.set_path)(link.this(), wide(exe.as_os_str()).as_ptr()),
        "set its target",
    )?;
    check(
        (calls.set_arguments)(link.this(), wide(arguments.as_ref()).as_ptr()),
        "set its arguments",
    )?;
    check(
        (calls.set_working_directory)(link.this(), wide(workdir.as_os_str()).as_ptr()),
        "set its folder",
    )?;
    check(
        (calls.set_icon_location)(link.this(), wide(exe.as_os_str()).as_ptr(), 0),
        "set its icon",
    )?;

    let mut raw = std::ptr::null_mut();
    check(
        (calls.query_interface)(link.this(), &IID_IPERSISTFILE, &mut raw),
        "open it for saving",
    )?;
    let file =
        Interface::<PersistFileVtbl>::adopt(raw).ok_or_else(|| no_object("open it for saving"))?;
    // TRUE: the shortcut keeps this path as its own file name.
    check(
        (file.vtbl().save)(file.this(), wide(lnk.as_os_str()).as_ptr(), 1),
        "save it",
    )
}

fn check(result: HRESULT, step: &str) -> io::Result<()> {
    if result >= 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "could not {step} (HRESULT {result:#010x})"
        )))
    }
}

/// NUL-terminated UTF-16, keeping any path Windows allows (not only UTF-8).
fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}
