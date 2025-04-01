use std::os::windows::ffi::OsStringExt;
use std::{ffi::OsString, path::PathBuf};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::{
    Foundation::{HWND, LPARAM, MAX_PATH},
    System::{
        ProcessStatus::GetModuleFileNameExW,
        Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
    },
    UI::WindowsAndMessaging::{
        EnumWindows, GA_ROOTOWNER, GetAncestor, GetForegroundWindow, GetLastActivePopup,
        GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    },
};
use windows::core::BOOL;

#[derive(Debug, Clone)]
pub struct Window {
    pub title: String,
    pub minimized: bool,
    pub window_handle: HWND,
    pub focused: bool,
    pub process_id: u32,
    pub process_path: Option<PathBuf>,
}

#[allow(unsafe_code)]
unsafe fn null_terminated_wchar_to_string(slice: &[u16]) -> OsString {
    match slice.iter().position(|&x| x == 0) {
        Some(pos) => OsString::from_wide(&slice[..pos]),
        None => OsString::from_wide(slice),
    }
}

#[allow(unsafe_code)]
unsafe fn get_process_path(pid: u32) -> Result<PathBuf, windows::core::Error> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)?;

        let mut exe_buf = [0u16; MAX_PATH as usize + 1];
        GetModuleFileNameExW(Some(handle), None, exe_buf.as_mut_slice());

        let _ = CloseHandle(handle);

        Ok(PathBuf::from(null_terminated_wchar_to_string(&exe_buf)))
    }
}

/// Get the window information
#[allow(unsafe_code)]
unsafe fn get_window_info(hwnd: HWND) -> Option<Window> {
    unsafe {
        let mut bytes: [u16; 500] = [0; 500];
        let len = GetWindowTextW(hwnd, &mut bytes);
        let title = String::from_utf16_lossy(&bytes[..len as usize])
            .trim()
            .to_owned();

        if title.is_empty() {
            return None;
        }

        let minimized = IsIconic(hwnd).as_bool();

        let forground_hwnd = GetForegroundWindow();
        let focused = hwnd == forground_hwnd;

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));

        let path = get_process_path(pid).ok();

        Some(Window {
            title,
            minimized,
            focused,
            window_handle: hwnd,
            process_id: pid,
            process_path: path,
        })
    }
}

/// Check if the window is an alt-tab window
#[allow(unsafe_code)]
unsafe fn is_alt_tab_window(hwnd: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return false;
        }

        let mut hwnd_try = GetAncestor(hwnd, GA_ROOTOWNER);
        let mut hwnd_walk = HWND::default();
        while hwnd_try != hwnd_walk {
            hwnd_walk = hwnd_try;
            hwnd_try = GetLastActivePopup(hwnd_walk);
            if IsWindowVisible(hwnd_try).as_bool() {
                break;
            }
        }
        if hwnd_walk != hwnd {
            return false;
        }

        true
    }
}

#[allow(unsafe_code)]
unsafe extern "system" fn callback(hwnd: HWND, param1: LPARAM) -> BOOL {
    unsafe {
        // Skip invisible windows and tool windows
        if !is_alt_tab_window(hwnd) {
            return true.into();
        }

        let windows = &mut *(param1.0 as *mut Vec<Window>);

        if let Some(window) = get_window_info(hwnd) {
            windows.push(window);
        }

        true.into()
    }
}

pub fn enumerate_windows() -> Vec<Window> {
    let mut windows = Vec::new();
    #[allow(unsafe_code)]
    unsafe {
        EnumWindows(
            Some(callback),
            LPARAM(&mut windows as *mut Vec<Window> as isize),
        )
        .unwrap();
    }

    windows
}
