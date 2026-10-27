#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct MonitorInfo {
    /// Friendly name when the OS offers one, else the device
    /// name ("DISPLAY1").
    pub name: String,
    /// OS device identity ("\\.\DISPLAY1") — the match key when
    /// a choice is re-validated at click time.
    pub device: String,
    /// The HMONITOR value (v1.3.1) — the identity a completed
    /// move is verified against (MonitorFromWindow).
    pub handle: isize,
    /// Physical-pixel rect [x, y, w, h] in virtual-desktop
    /// coordinates (origins can be negative).
    pub rect_px: [i32; 4],
    /// THIS monitor's scale factor (its effective DPI / 96).
    pub scale: f32,
    pub primary: bool,
}

/// A monitor rect in UI points ([x, y, w, h]), converted with
/// the monitor's OWN scale factor. Used ONLY for the menu's
/// "this window is here" marker, which compares against the
/// window centre UI reports — positions UI itself reports
/// live in this same per-monitor points space, so the marker
/// is consistent. (The snap MOVE never uses points: v1.3.1,
/// see snap_target_point_px.)
pub fn monitor_rect_points(m: &MonitorInfo) -> [f32; 4] {
    let s = if m.scale > 0.0 { m.scale } else { 1.0 };
    [
        m.rect_px[0] as f32 / s,
        m.rect_px[1] as f32 / s,
        m.rect_px[2] as f32 / s,
        m.rect_px[3] as f32 / s,
    ]
}

pub fn monitor_orientation(m: &MonitorInfo) -> &'static str {
    if m.rect_px[3] > m.rect_px[2] {
        "Portrait"
    } else {
        "Landscape"
    }
}

/// The one point a snap moves the window to (v1.3.1), in
/// PHYSICAL pixels: the monitor's top-left inset by 60 pt
/// worth of pixels at the monitor's own scale, clamped so the
/// point — and the whole window, when it fits — stays inside
/// the rect with an 8 pt margin (also scaled) on every side.
/// This is the point SetWindowPos receives; physical pixels
/// are the whole point of the v1.3.1 mechanism (see the
/// section header above).
pub fn snap_target_point_px(m: &MonitorInfo, win_px: [i32; 2]) -> [i32; 2] {
    let s = if m.scale > 0.0 { m.scale } else { 1.0 };
    let inset = 60.0 * s;
    let margin = 8.0 * s;
    [
        snap_axis(
            m.rect_px[0] as f32,
            m.rect_px[2] as f32,
            win_px[0] as f32,
            inset,
            margin,
        )
        .round() as i32,
        snap_axis(
            m.rect_px[1] as f32,
            m.rect_px[3] as f32,
            win_px[1] as f32,
            inset,
            margin,
        )
        .round() as i32,
    ]
}

/// One axis of the snap target: `origin + inset`, clamped so
/// the point — and the whole window, when it fits — stays
/// inside [origin, origin+len) with `margin` to spare on every
/// side. Negative-coordinate monitors (left of / above the
/// primary) work unchanged: everything is computed in the
/// monitor's own rect space.
pub fn snap_axis(origin: f32, len: f32, win_len: f32, inset: f32, margin: f32) -> f32 {
    if len <= 2.0 * margin {
        // Degenerate strip of a monitor: its centre is the only
        // surely-interior point.
        return origin + len / 2.0;
    }
    // Right/bottom bound: keep the whole window inside when it
    // fits with margins; otherwise keep the point itself inside
    // (the window is maximized immediately after, so the point
    // alone decides the target monitor).
    let hi = if win_len + 2.0 * margin <= len {
        origin + len - margin - win_len
    } else {
        origin + len - margin - 1.0
    };
    let lo = origin + margin;
    (origin + inset).min(hi).max(lo)
}

/// Which monitor a point (UI points, virtual-desktop
/// coordinates) falls on — used to mark the display the window
/// currently sits on (by its centre point).
pub fn monitor_index_at(monitors: &[MonitorInfo], point: [f32; 2]) -> Option<usize> {
    monitors.iter().position(|m| {
        let [x, y, w, h] = monitor_rect_points(m);
        point[0] >= x && point[0] < x + w && point[1] >= y && point[1] < y + h
    })
}
/// Live display enumeration (Windows): EnumDisplayMonitors +
/// GetMonitorInfoW for the rect/primary flag, GetDpiForMonitor
/// for each display's own scale, EnumDisplayDevicesW for the
/// friendly name. Called when the snap menu opens and again when
/// a choice is acted on — never cached across clicks.
#[cfg(target_os = "windows")]
pub fn enumerate_monitors() -> Vec<MonitorInfo> {
    use windows_sys::Win32::Foundation::{BOOL, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, DISPLAY_DEVICEW, HDC,
        HMONITOR, MONITORINFO, MONITORINFOEXW,
    };
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

    // (windows-sys 0.59: BOOL is an i32 alias, LPARAM an isize
    // alias — plain values, not tuple structs.)
    unsafe extern "system" fn collect(
        hmon: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let v = &mut *(data as *mut Vec<isize>);
        v.push(hmon as isize);
        1
    }

    fn wide(s: &[u16]) -> String {
        let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
        String::from_utf16_lossy(&s[..end])
    }

    let mut handles: Vec<isize> = Vec::new();
    let ok = unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect),
            &mut handles as *mut Vec<isize> as isize,
        )
    };
    if ok == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for h in handles {
        let hmon = h as HMONITOR;
        let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        let got = unsafe {
            GetMonitorInfoW(
                hmon,
                &mut info.monitorInfo as *mut MONITORINFO,
            )
        };
        if got == 0 {
            continue;
        }
        let rc = info.monitorInfo.rcMonitor;
        let device = wide(&info.szDevice);
        let (mut dx, mut dy) = (0u32, 0u32);
        let scale = if unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) } == 0
            && dx > 0
        {
            dx as f32 / 96.0
        } else {
            1.0
        };
        // Friendly name ("Dell U2415" etc.) via the display
        // device; fall back to the device name ("DISPLAY1").
        let mut name = String::new();
        let mut dd: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
        dd.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
        let dev_wide: Vec<u16> = device.encode_utf16().chain(std::iter::once(0)).collect();
        if unsafe {
            EnumDisplayDevicesW(dev_wide.as_ptr(), 0, &mut dd, 0)
        } != 0
        {
            name = wide(&dd.DeviceString);
        }
        if name.trim().is_empty() {
            name = device.trim_start_matches(&['\\', '.'][..]).to_string();
        }
        out.push(MonitorInfo {
            name,
            device,
            handle: h,
            rect_px: [rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top],
            scale,
            primary: info.monitorInfo.dwFlags & 1 != 0,
        });
    }
    out
}

/// Non-Windows builds have no enumeration: the snap is a Win32
/// mechanism and its button is Windows-only UI (v1.4.0), so no
/// snap job can ever start elsewhere — this stub just keeps the
/// shared helpers (menu marker maths, the state machine) total.
#[cfg(not(target_os = "windows"))]
pub fn enumerate_monitors() -> Vec<MonitorInfo> {
    Vec::new()
}

