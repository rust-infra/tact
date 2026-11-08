#[allow(unused_imports)]
pub use super::{PackagingTarget, TARGET};

/// Shows a taskbar overlay badge on the app's top-level window.
///
/// Windows has no numeric badge API on the taskbar button, so a warning
/// overlay marks "a decision is waiting"; the in-app gate banner and the OS
/// notification carry the count. `None` clears the overlay.
#[cfg(target_os = "windows")]
pub fn set_taskbar_badge(label: Option<String>) {
    use windows::Win32::Foundation::{FALSE, HWND, LPARAM, TRUE};
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IDI_WARNING, IsWindowVisible, LoadIconW,
    };
    use windows::core::{BOOL, HSTRING};

    // SAFETY: `EnumWindows` calls this with the `lparam` value passed below,
    // which is a `*mut HWND` built with `&raw mut hwnd` and pointing at the
    // caller's live `hwnd` local. The callback never outlives `EnumWindows`,
    // so the pointee stays valid for the whole enumeration, and `EnumWindows`
    // calls back on this same thread, so the write cannot race.
    unsafe extern "system" fn find_own_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let mut process = 0u32;
        // SAFETY: `Some(&mut process)` is a valid out-pointer for the duration
        // of the call; `hwnd` came from the OS.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process)) };
        // SAFETY: both take no pointers and no caller-side preconditions; they
        // only read kernel state for an HWND the OS just supplied.
        if process == unsafe { GetCurrentProcessId() } && unsafe { IsWindowVisible(hwnd) }.as_bool()
        {
            // SAFETY: see the callback comment above — `lparam` is the
            // `*mut HWND` built with `&raw mut hwnd` in the enclosing call.
            unsafe { *(lparam.0 as *mut HWND) = hwnd };
            return FALSE;
        }
        TRUE
    }

    // SAFETY: every call below only touches handles derived within this block
    // (`hwnd` from the enumeration, `taskbar` from `CoCreateInstance`), and
    // each is checked for failure before use. COM is initialized elsewhere for
    // this thread; a missing apartment surfaces as an `HrInit`/create error,
    // not memory unsafety.
    unsafe {
        let mut hwnd = HWND::default();
        let _ = EnumWindows(Some(find_own_window), LPARAM((&raw mut hwnd) as isize));
        if hwnd.0.is_null() {
            return;
        }
        let Ok(taskbar) =
            CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER)
        else {
            return;
        };
        if taskbar.HrInit().is_err() {
            return;
        }
        match label {
            Some(label) => {
                let icon = LoadIconW(None, IDI_WARNING).unwrap_or_default();
                let _ = taskbar.SetOverlayIcon(hwnd, icon, &HSTRING::from(label));
            }
            None => {
                let _ = taskbar.SetOverlayIcon(hwnd, Default::default(), &HSTRING::new());
            }
        }
    }
}

/// Groups the process's windows, notifications, and taskbar button under
/// `app_id`.
///
/// Without it Windows files a bare executable under its path, so the installed
/// shortcut's icon never reaches the running window.
#[cfg(target_os = "windows")]
pub fn set_app_user_model_id(app_id: &str) {
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    use windows::core::HSTRING;

    // SAFETY: the call takes a borrowed, NUL-terminated `HSTRING` that lives
    // through the statement and has no other preconditions; the returned HRESULT
    // is a status code, so ignoring it is sound (a failure only means the window
    // keeps the default AppUserModelID).
    let _ = unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(app_id)) };
}

#[cfg(not(target_os = "windows"))]
pub fn set_taskbar_badge(_label: Option<String>) {}
