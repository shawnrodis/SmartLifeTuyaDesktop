use std::path::{Path, PathBuf};
use std::process::Command;
pub fn player_exe_name(name: &str) -> String {
    if cfg!(target_os = "windows") && !name.to_lowercase().ends_with(".exe") {
        format!("{}.exe", name)
    } else {
        name.to_string()
    }
}

/// Ordered on-disk candidates for a BARE player name (pure — the
/// caller gathers the inputs, so the order is unit-testable):
/// (1) every PATH directory, (2) the Windows App Paths registry
/// value, (3) well-known install directories (VLC's VideoLAN
/// folders). The first candidate that exists wins.
pub fn player_candidates(
    name: &str,
    path_dirs: &[PathBuf],
    app_path: Option<&Path>,
    known_dirs: &[PathBuf],
) -> Vec<PathBuf> {
    let exe = player_exe_name(name);
    let mut out: Vec<PathBuf> = Vec::new();
    for d in path_dirs {
        out.push(d.join(&exe));
    }
    if let Some(p) = app_path {
        out.push(p.to_path_buf());
    }
    for d in known_dirs {
        out.push(d.join(&exe));
    }
    out
}

/// Extract the (Default) value from `reg query … /ve` output: the
/// text after REG_SZ on its line. Pure, for tests.
pub fn parse_reg_default_value(output: &str) -> Option<String> {
    for line in output.lines() {
        if let Some(pos) = line.find("REG_SZ") {
            let val = line[pos + "REG_SZ".len()..].trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

/// The Windows "App Paths" registry entry for a program — where
/// installers (VLC included) register their real location.
/// HKLM first, then HKCU.
#[cfg(target_os = "windows")]
pub fn app_paths_lookup(name: &str) -> Option<PathBuf> {
    let exe = player_exe_name(name);
    for root in ["HKLM", "HKCU"] {
        let key = format!(
            "{}\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{}",
            root, exe
        );
        let mut cmd = Command::new("reg");
        cmd.args(["query", &key, "/ve"]);
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        if let Ok(out) = cmd.output() {
            if out.status.success() {
                if let Some(v) = parse_reg_default_value(&String::from_utf8_lossy(&out.stdout)) {
                    return Some(PathBuf::from(v));
                }
            }
        }
    }
    None
}

#[cfg(not(target_os = "windows"))]
pub fn app_paths_lookup(_name: &str) -> Option<PathBuf> {
    None
}

pub fn vlc_known_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(pf) = std::env::var_os(var) {
            v.push(PathBuf::from(pf).join("VideoLAN").join("VLC"));
        }
    }
    if cfg!(target_os = "macos") {
        v.push(PathBuf::from("/Applications/VLC.app/Contents/MacOS"));
    }
    v
}

/// Resolve a player program to a spawnable path. A command with a
/// path separator is used as given. A BARE name is searched in
/// order: PATH, the App Paths registry entries, and — for vlc —
/// the VideoLAN install folders. None only after every lookup
/// failed, so "program not found" is reported exactly then.
pub fn resolve_player_program(prog: &str) -> Option<String> {
    if prog.contains('\\') || prog.contains('/') {
        return Some(prog.to_string());
    }
    let path_dirs: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    let app_path = app_paths_lookup(prog);
    let known = if prog.eq_ignore_ascii_case("vlc") || prog.eq_ignore_ascii_case("vlc.exe") {
        vlc_known_dirs()
    } else {
        Vec::new()
    };
    player_candidates(prog, &path_dirs, app_path.as_deref(), &known)
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Launch the configured external player with the stream URL.
/// Returns the program name (as configured) on success.
pub fn launch_player(player_cmd: &str, url: &str) -> Result<String, String> {
    let mut parts = player_cmd.split_whitespace();
    let prog = parts.next().unwrap_or("vlc").to_string();
    let spawn_path = match resolve_player_program(&prog) {
        Some(p) => p,
        None => {
            let searched = if cfg!(target_os = "windows") {
                "searched PATH, the Windows App Paths registry entries, and the usual install folders"
            } else {
                "searched PATH and the usual install folders"
            };
            return Err(format!("{}: program not found ({})", prog, searched))
        }
    };
    Command::new(&spawn_path)
        .args(parts)
        .arg(url)
        .spawn()
        .map(|_| prog.clone())
        .map_err(|e| format!("{}: {}", prog, e))
}
