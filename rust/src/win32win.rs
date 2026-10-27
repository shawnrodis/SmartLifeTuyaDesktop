#[cfg(target_os = "windows")]
pub fn win32_needs_restore(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsZoomed};
    unsafe { IsZoomed(hwnd as _) != 0 || IsIconic(hwnd as _) != 0 }
}

#[cfg(not(target_os = "windows"))]
pub fn win32_needs_restore(_hwnd: isize) -> bool {
    false
}

#[cfg(target_os = "windows")]
pub fn win32_restore_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_RESTORE};
    unsafe {
        ShowWindow(hwnd as _, SW_RESTORE);
    }
}

#[cfg(not(target_os = "windows"))]
pub fn win32_restore_window(_hwnd: isize) {}

#[cfg(target_os = "windows")]
pub fn win32_maximize_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_MAXIMIZE};
    unsafe {
        ShowWindow(hwnd as _, SW_MAXIMIZE);
    }
}

#[cfg(not(target_os = "windows"))]
pub fn win32_maximize_window(_hwnd: isize) {}

/// Move the window's top-left to a PHYSICAL-pixel point,
/// keeping its size and z-order (SetWindowPos).
#[cfg(target_os = "windows")]
pub fn win32_move_window(hwnd: isize, x: i32, y: i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOSIZE, SWP_NOZORDER};
    unsafe {
        SetWindowPos(
            hwnd as _,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER,
        );
    }
}

#[cfg(not(target_os = "windows"))]
pub fn win32_move_window(_hwnd: isize, _x: i32, _y: i32) {}

/// The monitor the window is on right now (HMONITOR as isize;
/// MONITOR_DEFAULTTONEAREST always names one for a real
/// window) — the arrival check a move is verified against.
#[cfg(target_os = "windows")]
pub fn win32_window_monitor(hwnd: isize) -> isize {
    use windows_sys::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
    unsafe { MonitorFromWindow(hwnd as _, MONITOR_DEFAULTTONEAREST) as isize }
}

#[cfg(not(target_os = "windows"))]
pub fn win32_window_monitor(_hwnd: isize) -> isize {
    0
}

/// The window's current size in PHYSICAL pixels (GetWindowRect)
/// — the size the snap target clamp keeps inside the monitor.
#[cfg(target_os = "windows")]
pub fn win32_window_size_px(hwnd: isize) -> [i32; 2] {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetWindowRect(hwnd as _, &mut rc) } != 0 && rc.right > rc.left {
        [rc.right - rc.left, rc.bottom - rc.top]
    } else {
        [1120, 780]
    }
}

#[cfg(not(target_os = "windows"))]
pub fn win32_window_size_px(_hwnd: isize) -> [i32; 2] {
    [1120, 780]
}
