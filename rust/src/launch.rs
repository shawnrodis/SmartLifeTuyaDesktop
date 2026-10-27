use crate::config::Prefs;
use std::path::Path;
/// The per-user autostart (Run) key + this app's value name in it.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_VALUE_NAME: &str = "SmartLifeTuyaDesktop";

/// How the app presents itself at launch. Carried as a flag in the
/// autostart command ("--minimized" / "--maximized") and honoured
/// for manual launches (shortcuts) too.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum LaunchMode {
    Normal,
    Minimized,
    Maximized,
}

impl LaunchMode {
    pub fn parse(s: &str) -> LaunchMode {
        match s {
            "minimized" => LaunchMode::Minimized,
            "maximized" => LaunchMode::Maximized,
            _ => LaunchMode::Normal,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            LaunchMode::Normal => "normal",
            LaunchMode::Minimized => "minimized",
            LaunchMode::Maximized => "maximized",
        }
    }
    pub fn flag(&self) -> Option<&'static str> {
        match self {
            LaunchMode::Normal => None,
            LaunchMode::Minimized => Some("--minimized"),
            LaunchMode::Maximized => Some("--maximized"),
        }
    }
}

/// Launch mode requested on the command line. If both flags sneak
/// in, --minimized wins (the more conservative presentation).
pub fn launch_mode_from_args(args: &[String]) -> LaunchMode {
    if args.iter().any(|a| a == "--minimized") {
        LaunchMode::Minimized
    } else if args.iter().any(|a| a == "--maximized") {
        LaunchMode::Maximized
    } else {
        LaunchMode::Normal
    }
}

/// Launch mode encoded in a stored autostart command string.
pub fn launch_mode_from_command(cmd: &str) -> LaunchMode {
    if cmd.contains("--minimized") {
        LaunchMode::Minimized
    } else if cmd.contains("--maximized") {
        LaunchMode::Maximized
    } else {
        LaunchMode::Normal
    }
}

/// The EFFECTIVE launch mode for this process (v1.2.4). An
/// explicit command-line flag wins — it describes THIS launch.
/// Otherwise the saved Settings launch mode applies to EVERY
/// launch: before v1.2.4 only the argv flag was consulted, so any
/// launch that didn't come through the registry Run entry — a
/// Start Menu / shortcut / installer-finish launch, or Windows'
/// "restartable apps" sign-in relaunch (bare exe path, no Run
/// command line at all) — opened as a normal visible window
/// whatever Settings said. `argv` is Normal both for "no flag"
/// and an explicit plain launch, which is exactly when the saved
/// preference should decide.
pub fn resolve_launch_mode(argv: LaunchMode, prefs: LaunchMode) -> LaunchMode {
    if argv != LaunchMode::Normal {
        argv
    } else {
        prefs
    }
}

/// The stored autostart command: the exe path in quotes (install
/// paths contain spaces), plus the launch-mode flag if any. The
/// Run key stores it verbatim; the macOS LaunchAgent stores the
/// same command decomposed into ProgramArguments.
pub fn autostart_command(exe: &Path, mode: LaunchMode) -> String {
    let mut s = format!("\"{}\"", exe.display());
    if let Some(f) = mode.flag() {
        s.push(' ');
        s.push_str(f);
    }
    s
}

/// What the autostart store SHOULD hold for these prefs: the
/// command when autostart is ON, or None when OFF — OFF is a
/// first-class state meaning NO autostart entry exists at all.
pub fn desired_autostart(prefs: &Prefs, exe: &Path) -> Option<String> {
    if prefs.start_with_windows {
        Some(autostart_command(exe, LaunchMode::parse(&prefs.launch_mode)))
    } else {
        None
    }
}

/// Startup & background panel wording (v1.4.0): the feature is
/// the same on both desktop platforms; the OS nouns are not.
#[cfg(target_os = "windows")]
pub const AUTOSTART_LABEL: &str = "Start with Windows";
#[cfg(not(target_os = "windows"))]
pub const AUTOSTART_LABEL: &str = "Start at login";

#[cfg(target_os = "windows")]
pub const CLOSE_TO_TRAY_LABEL: &str =
    "Keep running in the system tray when the window is closed";
#[cfg(not(target_os = "windows"))]
pub const CLOSE_TO_TRAY_LABEL: &str =
    "Keep running in the menu bar when the window is closed";

#[cfg(not(target_os = "macos"))]
pub const AUTOSTART_HELP: &str = "Start with Windows uses the current-user registry Run key — no administrator rights needed, and OFF means no autostart entry exists at all. The “Launch as” choice applies to every launch of the app — autostart, Start Menu, and shortcuts alike — not just the Windows-startup one; a launch that carries its own --minimized or --maximized flag (as the autostart entry does) overrides it for that launch only. “Minimized (to tray)” starts the app hidden: its icon sits in the notification area by the Windows clock, and clicking the icon (or “Open” on its right-click menu) brings the window back. With “Keep running in the system tray” on, closing the window hides it instead of quitting — device refresh and camera previews keep running in the background — and Quit lives on the tray icon's right-click menu. The tray icon appears when the app starts minimized or while close-to-tray is on, and then stays for the session. Logging out doesn't quit the app either way. This panel is a Windows-app feature; the web edition has no equivalent.";

#[cfg(target_os = "macos")]
pub const AUTOSTART_HELP: &str = "Start at login uses a per-user LaunchAgent in ~/Library/LaunchAgents — no administrator rights needed, and OFF means no LaunchAgent file exists at all. The “Launch as” choice applies to every launch of the app — login, Finder, and Dock alike — not just the login one; a launch that carries its own --minimized or --maximized flag (as the LaunchAgent entry does) overrides it for that launch only. “Minimized (to tray)” starts the app hidden: its icon sits in the menu bar at the top of the screen, and clicking the icon (or “Open” on its menu) brings the window back. With “Keep running in the menu bar” on, closing the window hides it instead of quitting — device refresh and camera previews keep running in the background — and Quit lives on the icon's menu. The icon appears when the app starts minimized or while close-to-tray is on, and then stays for the session. Logging out doesn't quit the app either way. This panel is a native-app feature; the web edition has no equivalent.";

// ---------------------------------------------------------------------------
// Snap to monitor (v1.3.0; mechanism replaced in v1.3.1): a
// header button lists the displays the OS reports at that
// moment; choosing one moves the window onto that display and
// maximizes it there. Click-time only, by design — nothing is
// persisted, and startup placement is never affected (the
// v1.2.5 placement memory stays removed).
//
// v1.3.0 moved the window with a one-frame burst of UI
// viewport commands (Fullscreen(false) -> Maximized(false) ->
// OuterPosition(points) -> Maximized(true)). In the field that
// never moved the window at all: the target point was computed
// in points with the TARGET display's scale factor, but winit
// converts an OuterPosition using the scale factor of the
// monitor the window is CURRENTLY on — on a mixed-DPI desk
// (1440p landscape beside a 4K portrait at a higher scale) the
// converted point landed back inside the SOURCE monitor, and
// the trailing maximize raced the still-transitioning window
// state, leaving the window un-maximized where it started.
//
// v1.3.1 performs the move in Win32 directly, against the app
// window's HWND (the tray code already works that way), in
// PHYSICAL pixels end-to-end — no points conversion anywhere:
// a per-frame state machine (snap_next / advance_snap) leaves
// fullscreen/maximized, SetWindowPos-es the window to a single
// clamped interior point of the target monitor, VERIFIES
// arrival with MonitorFromWindow (one restore+move retry),
// and only then ShowWindow(SW_MAXIMIZE) — after which the OS
// owns the geometry (the monitor's work area), so the result
// cannot be out of bounds by construction.
// ---------------------------------------------------------------------------
