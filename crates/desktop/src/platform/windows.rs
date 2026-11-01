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

    unsafe extern "system" fn find_own_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let mut process = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process)) };
        if process == unsafe { GetCurrentProcessId() } && unsafe { IsWindowVisible(hwnd) }.as_bool()
        {
            unsafe { *(lparam.0 as *mut HWND) = hwnd };
            return FALSE;
        }
        TRUE
    }

    unsafe {
        let mut hwnd = HWND::default();
        let _ = EnumWindows(
            Some(find_own_window),
            LPARAM(&mut hwnd as *mut HWND as isize),
        );
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

#[cfg(not(target_os = "windows"))]
pub fn set_taskbar_badge(_label: Option<String>) {}
