//! Tray / autostart / single-instance helpers that do not depend on a
//! UI framework. The tray icon itself (tray-icon crate) exists only on
//! Windows/macOS, exactly as in the original; on other platforms
//! `build_tray` returns None and the action queue still works, so the
//! Flutter shell can drive Show/Quit from its own tray implementation.
//!
//! No secrets are logged anywhere in this module.

use crate::config::{config_dir, config_path, Config, Prefs};
use crate::launch::{autostart_command, desired_autostart, launch_mode_from_command, LaunchMode};
#[cfg(target_os = "windows")]
use crate::launch::{RUN_KEY, RUN_VALUE_NAME};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub const SINGLE_INSTANCE_MUTEX: &str = "SmartLifeTuyaDesktop.SingleInstance";
pub const SHOW_WINDOW_EVENT: &str = "SmartLifeTuyaDesktop.ShowWindow";
pub const LAUNCH_AGENT_LABEL: &str = "com.shawnrodis.smartlifetuya";

/// What the user asked the tray to do.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum TrayAction {
    Show,
    Quit,
}

impl TrayAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            TrayAction::Show => "show",
            TrayAction::Quit => "quit",
        }
    }
    pub fn from_str(s: &str) -> Option<TrayAction> {
        match s {
            "show" => Some(TrayAction::Show),
            "quit" => Some(TrayAction::Quit),
            _ => None,
        }
    }
}

static TRAY_QUEUE: Mutex<VecDeque<TrayAction>> = Mutex::new(VecDeque::new());

pub fn push_tray_action(a: TrayAction) {
    if let Ok(mut q) = TRAY_QUEUE.lock() {
        q.push_back(a);
    }
}

pub fn drain_tray_actions() -> Vec<TrayAction> {
    match TRAY_QUEUE.lock() {
        Ok(mut q) => q.drain(..).collect(),
        Err(_) => Vec::new(),
    }
}

/// Pure event->action mapping for tray icon events, reduced to
/// primitives (same mapping as the original): a left-button release
/// shows, a left double-click shows; everything else is ignored.
pub fn tray_action_for_icon_parts(
    double_click: bool,
    left_button: bool,
    button_up: bool,
) -> Option<TrayAction> {
    if (double_click && left_button) || (left_button && button_up) {
        Some(TrayAction::Show)
    } else {
        None
    }
}

/// Pure mapping for a tray menu item id, where ids are the plain
/// strings "open" / "quit" (the Flutter shell maps its own menu ids
/// to these before calling; the original compared tray-icon MenuIds).
pub fn tray_action_for_menu_id(id: &str) -> Option<TrayAction> {
    match id {
        "open" => Some(TrayAction::Show),
        "quit" => Some(TrayAction::Quit),
        _ => None,
    }
}

/// What a window-close request should do.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CloseAction {
    Quit,
    HideToTray,
}

pub fn close_action(close_to_tray: bool, quitting: bool) -> CloseAction {
    if close_to_tray && !quitting {
        CloseAction::HideToTray
    } else {
        CloseAction::Quit
    }
}

// --- Tray icon (Windows/macOS only, as in the original) ---

/// Opaque handle: the icon object must stay alive for the session.
/// On Linux this type is empty and `build_tray` returns None.
pub struct TrayState {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub _icon: tray_icon::TrayIcon,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub open_id: tray_icon::menu::MenuId,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub quit_id: tray_icon::menu::MenuId,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn build_tray() -> Option<TrayState> {
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
    let open_item = MenuItem::new("Open SmartLife & Tuya Desktop", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    let open_id = open_item.id().clone();
    let quit_id = quit_item.id().clone();
    let menu = Menu::new();
    menu.append(&open_item).ok()?;
    menu.append(&PredefinedMenuItem::separator()).ok()?;
    menu.append(&quit_item).ok()?;
    // Icon bytes live in the app crate's assets; the core reads them
    // from a path the Flutter shell supplies via env, else falls back
    // to a built-in 1x1 transparent icon so construction never fails
    // for a missing asset.
    let rgba: Vec<u8> = std::env::var_os("TUYA_TRAY_ICON_RGBA")
        .and_then(|p| std::fs::read(p).ok())
        .filter(|b| b.len() == 64 * 64 * 4)
        .unwrap_or_else(|| vec![0u8; 64 * 64 * 4]);
    let icon = tray_icon::Icon::from_rgba(rgba, 64, 64).ok()?;
    let tray = tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("SmartLife & Tuya Desktop")
        .with_icon(icon)
        .build()
        .ok()?;
    Some(TrayState { _icon: tray, open_id, quit_id })
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn build_tray() -> Option<TrayState> {
    None
}

/// Spawn the dedicated tray thread exactly once (Windows only in the
/// original; on macOS the shell polls the queue). It owns both tray
/// event receivers so events are consumed even when no UI frame runs.
/// Without a UI framework there is no repaint to poke and no HWND to
/// restore here: the thread only enqueues actions, and the Flutter
/// shell drains them via `tuya_tray_actions` and acts (show/quit).
/// On non-Windows platforms this is a no-op returning false.
#[cfg(target_os = "windows")]
/// Spawn the dedicated tray thread exactly once (Windows, v1.2.3
/// architecture): it owns both event receivers so tray events are
/// consumed even when no UI frame runs, maps them through the pure
/// functions, and pushes actions onto the static queue. The caller
/// (Flutter shell) drains the queue and acts; unlike the original
/// there is no UI-framework repaint to poke and no HWND restore
/// here — the shell owns its window, so show/quit are its job.
/// Menu ids are the ones captured from `build_tray`.
pub fn spawn_tray_thread(open_id: tray_icon::menu::MenuId, quit_id: tray_icon::menu::MenuId) -> bool {
    static SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPAWNED.swap(true, Ordering::SeqCst) {
        return false;
    }
    std::thread::Builder::new()
        .name("tray-events".to_string())
        .spawn(move || {
            let menu_rx = tray_icon::menu::MenuEvent::receiver();
            let icon_rx = tray_icon::TrayIconEvent::receiver();
            loop {
                match menu_rx.recv_timeout(std::time::Duration::from_millis(40)) {
                    Ok(ev) => {
                        if ev.id() == &open_id {
                            push_tray_action(TrayAction::Show);
                        } else if ev.id() == &quit_id {
                            push_tray_action(TrayAction::Quit);
                        }
                    }
                    Err(_) => {}
                }
                while let Ok(ev) = menu_rx.try_recv() {
                    if ev.id() == &open_id {
                        push_tray_action(TrayAction::Show);
                    } else if ev.id() == &quit_id {
                        push_tray_action(TrayAction::Quit);
                    }
                }
                while let Ok(ev) = icon_rx.try_recv() {
                    use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
                    let (dbl, left, up) = match &ev {
                        TrayIconEvent::DoubleClick { button, .. } => {
                            (true, *button == MouseButton::Left, false)
                        }
                        TrayIconEvent::Click { button, button_state, .. } => (
                            false,
                            *button == MouseButton::Left,
                            *button_state == MouseButtonState::Up,
                        ),
                        _ => (false, false, false),
                    };
                    if let Some(a) = tray_action_for_icon_parts(dbl, left, up) {
                        push_tray_action(a);
                    }
                }
            }
        })
        .is_ok()
}

#[cfg(not(target_os = "windows"))]
pub fn spawn_tray_thread() -> bool {
    false
}

pub fn tray_thread_spawned_note() -> &'static str {
    "tray thread: Windows only; macOS/Linux shells drain the queue themselves"
}

// --- Single instance ---

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum InstanceRole {
    Primary,
    Secondary,
}

pub fn instance_role(already_exists: bool) -> InstanceRole {
    if already_exists {
        InstanceRole::Secondary
    } else {
        InstanceRole::Primary
    }
}

pub static SHOW_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn show_requested_take() -> bool {
    SHOW_REQUESTED.swap(false, Ordering::SeqCst)
}

#[cfg(target_os = "windows")]
pub struct InstanceGuard {
    _mutex: usize,
    _event: usize,
}

#[cfg(target_os = "windows")]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            use windows_sys::Win32::Foundation::CloseHandle;
            CloseHandle(self._mutex as _);
            CloseHandle(self._event as _);
        }
    }
}

#[cfg(target_os = "windows")]
pub fn acquire_single_instance() -> (InstanceRole, Option<InstanceGuard>) {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject, INFINITE,
    };
    fn wide_z(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    unsafe {
        let ev_name = wide_z(SHOW_WINDOW_EVENT);
        let event = CreateEventW(std::ptr::null(), 0, 0, ev_name.as_ptr());
        let mu_name = wide_z(SINGLE_INSTANCE_MUTEX);
        let mutex = CreateMutexW(std::ptr::null(), 1, mu_name.as_ptr());
        if mutex.is_null() {
            if !event.is_null() {
                CloseHandle(event);
            }
            return (InstanceRole::Primary, None);
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if !event.is_null() {
                SetEvent(event);
                CloseHandle(event);
            }
            CloseHandle(mutex);
            return (InstanceRole::Secondary, None);
        }
        if !event.is_null() {
            let h = event as usize;
            std::thread::spawn(move || loop {
                if WaitForSingleObject(h as _, INFINITE) != 0 {
                    break;
                }
                SHOW_REQUESTED.store(true, Ordering::SeqCst);
                push_tray_action(TrayAction::Show);
            });
        }
        (InstanceRole::Primary, Some(InstanceGuard { _mutex: mutex as usize, _event: event as usize }))
    }
}

/// File-lock guard (macOS original; also used on Linux/Unix in the
/// core, where the original had no guard — dropping releases it).
#[cfg(unix)]
pub struct FileInstanceGuard {
    _file: std::fs::File,
}

#[cfg(unix)]
pub fn try_lock_instance_file(path: &Path) -> std::io::Result<Option<std::fs::File>> {
    use fs2::FileExt;
    let file = std::fs::OpenOptions::new().create(true).write(true).open(path)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(_) => Ok(None),
    }
}

#[cfg(unix)]
pub fn show_request_path() -> PathBuf {
    config_dir().join("show-request")
}

#[cfg(unix)]
pub fn acquire_single_instance() -> (InstanceRole, Option<FileInstanceGuard>) {
    let dir = config_dir();
    let _ = std::fs::create_dir_all(&dir);
    match try_lock_instance_file(&dir.join("instance.lock")) {
        Ok(Some(file)) => (InstanceRole::Primary, Some(FileInstanceGuard { _file: file })),
        Ok(None) => {
            let _ = std::fs::write(show_request_path(), b"show");
            (InstanceRole::Secondary, None)
        }
        Err(_) => (InstanceRole::Primary, None),
    }
}

#[cfg(not(any(target_os = "windows", unix)))]
pub fn acquire_single_instance() -> (InstanceRole, Option<()>) {
    (InstanceRole::Primary, None)
}

// --- Autostart store ---

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

pub fn parse_autostart_command(cmd: &str) -> Option<(String, LaunchMode)> {
    let cmd = cmd.trim();
    let exe = if let Some(rest) = cmd.strip_prefix('"') {
        rest.split('"').next()?.to_string()
    } else {
        cmd.split_whitespace().next()?.to_string()
    };
    if exe.is_empty() {
        return None;
    }
    Some((exe, launch_mode_from_command(cmd)))
}

pub fn launch_agent_plist(cmd: &str) -> Option<String> {
    let (exe, mode) = parse_autostart_command(cmd)?;
    let mut lines: Vec<String> = vec![
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_string(),
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">".to_string(),
        "<plist version=\"1.0\">".to_string(),
        "<dict>".to_string(),
        "    <key>Label</key>".to_string(),
        format!("    <string>{}</string>", LAUNCH_AGENT_LABEL),
        "    <key>ProgramArguments</key>".to_string(),
        "    <array>".to_string(),
        format!("        <string>{}</string>", xml_escape(&exe)),
    ];
    if let Some(flag) = mode.flag() {
        lines.push(format!("        <string>{}</string>", flag));
    }
    lines.extend([
        "    </array>".to_string(),
        "    <key>RunAtLoad</key>".to_string(),
        "    <true/>".to_string(),
        "</dict>".to_string(),
        "</plist>".to_string(),
        String::new(),
    ]);
    Some(lines.join("\n"))
}

pub fn launch_agent_exe(plist: &str) -> Option<String> {
    let after = plist.split("<key>ProgramArguments</key>").nth(1)?;
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")? + start;
    let exe = xml_unescape(after[start..end].trim());
    if exe.is_empty() {
        None
    } else {
        Some(exe)
    }
}

pub fn launch_agent_command(plist: &str) -> Option<String> {
    if !plist.contains(LAUNCH_AGENT_LABEL) {
        return None;
    }
    let exe = launch_agent_exe(plist)?;
    Some(autostart_command(Path::new(&exe), launch_mode_from_command(plist)))
}

#[cfg(target_os = "macos")]
pub fn launch_agent_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| {
        PathBuf::from(h).join("Library").join("LaunchAgents").join(format!("{}.plist", LAUNCH_AGENT_LABEL))
    })
}

pub fn parse_reg_named_value(output: &str, name: &str) -> Option<String> {
    for line in output.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix(name) {
            if let Some(pos) = rest.find("REG_SZ") {
                let val = rest[pos + "REG_SZ".len()..].trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
pub fn reg_command() -> Command {
    let mut cmd = Command::new("reg");
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

#[cfg(not(target_os = "windows"))]
pub fn reg_command() -> Command {
    Command::new("reg")
}

#[cfg(target_os = "windows")]
pub fn reg_run_value() -> Option<String> {
    let key = format!("HKCU\\{}", RUN_KEY);
    let out = reg_command().args(["query", &key, "/v", RUN_VALUE_NAME]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_reg_named_value(&String::from_utf8_lossy(&out.stdout), RUN_VALUE_NAME)
}

#[cfg(target_os = "macos")]
pub fn reg_run_value() -> Option<String> {
    let path = launch_agent_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    launch_agent_command(&text)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn reg_run_value() -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
pub fn reg_set_run_value(cmdline: Option<&str>) -> bool {
    let key = format!("HKCU\\{}", RUN_KEY);
    let status = match cmdline {
        Some(c) => reg_command().args(["add", &key, "/v", RUN_VALUE_NAME, "/t", "REG_SZ", "/d", c, "/f"]).status(),
        None => reg_command().args(["delete", &key, "/v", RUN_VALUE_NAME, "/f"]).status(),
    };
    status.map(|s| s.success()).unwrap_or(false)
}

#[cfg(target_os = "macos")]
pub fn reg_set_run_value(cmdline: Option<&str>) -> bool {
    let Some(path) = launch_agent_path() else {
        return false;
    };
    match cmdline {
        Some(cmd) => {
            let Some(plist) = launch_agent_plist(cmd) else {
                return false;
            };
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(&path, plist).is_ok()
        }
        None => !path.exists() || std::fs::remove_file(&path).is_ok(),
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn reg_set_run_value(_cmdline: Option<&str>) -> bool {
    false
}

pub fn reconcile_autostart(cfg: &mut Config) {
    let Ok(exe) = std::env::current_exe() else { return };
    match reg_run_value() {
        Some(existing) => {
            cfg.prefs.start_with_windows = true;
            let mode = launch_mode_from_command(&existing);
            cfg.prefs.launch_mode = mode.as_str().to_string();
            let want = autostart_command(&exe, mode);
            if existing != want {
                reg_set_run_value(Some(&want));
            }
        }
        None => {
            cfg.prefs.start_with_windows = false;
        }
    }
}

pub fn apply_autostart(prefs: &Prefs) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let want = desired_autostart(prefs, &exe);
    let _ = reg_set_run_value(want.as_deref());
    reg_run_value() == want
}

/// Current autostart state as the FFI reports it: whether the OS
/// store holds an entry, and the launch mode encoded in it.
pub fn autostart_get() -> (bool, String) {
    match reg_run_value() {
        Some(cmd) => (true, launch_mode_from_command(&cmd).as_str().to_string()),
        None => (false, "normal".to_string()),
    }
}

pub fn config_path_string() -> String {
    config_path().to_string_lossy().into_owned()
}
