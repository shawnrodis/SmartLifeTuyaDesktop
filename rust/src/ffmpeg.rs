use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
pub const TILE_W: usize = 480;
pub const TILE_H: usize = 270;
pub const PLAYER_W: usize = 960;
pub const PLAYER_H: usize = 540;

/// Bytes per raw RGBA frame at w×h.
pub fn frame_bytes(w: usize, h: usize) -> usize {
    w * h * 4
}

/// ffmpeg arguments for the frame pump: TCP RTSP in, scaled + padded
/// to exactly w×h at 12 fps, raw RGBA frames on stdout. The URL (with
/// credentials) travels as a single argv — never through a shell.
pub fn ffmpeg_args(url: &str, w: usize, h: usize) -> Vec<String> {
    vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-rtsp_transport".to_string(),
        "tcp".to_string(),
        "-i".to_string(),
        url.to_string(),
        "-vf".to_string(),
        format!(
            "scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2,fps=12",
            w, h, w, h
        ),
        "-f".to_string(),
        "rawvideo".to_string(),
        "-pix_fmt".to_string(),
        "rgba".to_string(),
        "-".to_string(),
    ]
}

/// The bundled ffmpeg's file name for a macOS architecture
/// (v1.4.0): the CI ships one static build per architecture in
/// the .app's Resources under these exact names, and the app
/// picks its own architecture's. None for anything else.
#[cfg(any(target_os = "macos", test))]
pub fn ffmpeg_bundled_name(arch: &str) -> Option<&'static str> {
    match arch {
        "aarch64" => Some("ffmpeg-arm64"),
        "x86_64" => Some("ffmpeg-x86_64"),
        _ => None,
    }
}

/// Candidate ffmpeg locations, in lookup order: (1) beside the app
/// exe (the Windows installer puts it there; on macOS the .app's
/// Resources copy — per-architecture, see ffmpeg_bundled_name —
/// sits beside-equivalent at ../Resources), (2) the Settings
/// override (a path, or a bare name resolved against PATH),
/// (3) PATH.
pub fn ffmpeg_candidates(exe_dir: &Path, override_path: &str, path_var: &str) -> Vec<PathBuf> {
    let bin = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let mut out: Vec<PathBuf> = Vec::new();
    out.push(exe_dir.join(bin));
    #[cfg(target_os = "macos")]
    if let Some(name) = ffmpeg_bundled_name(std::env::consts::ARCH) {
        // .app layout: the exe is Contents/MacOS/<exe>, the
        // bundled binaries are Contents/Resources/<name>.
        out.push(exe_dir.join("../Resources").join(name));
    }
    let ov = override_path.trim();
    if !ov.is_empty() {
        let p = PathBuf::from(ov);
        if p.parent().map(|par| !par.as_os_str().is_empty()).unwrap_or(false) {
            out.push(p);
        } else {
            for dir in std::env::split_paths(path_var) {
                out.push(dir.join(ov));
            }
        }
    }
    for dir in std::env::split_paths(path_var) {
        out.push(dir.join(bin));
    }
    out
}

/// Locate ffmpeg: bundled beside the exe first, then the Settings
/// override, then PATH. None → previews off, Watch falls back.
pub fn find_ffmpeg(override_path: &str) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    ffmpeg_candidates(&exe_dir, override_path, &path_var.to_string_lossy())
        .into_iter()
        .find(|p| p.is_file())
}

/// Retry delay after a pump failure: 2 s, 4 s, then an 8 s cap
/// (v1.2.2 — tightened from 2/5/15 so a feed that dropped for a
/// transient reason — a DHCP move, a freed RTSP session — comes
/// back in seconds, not half a minute).
pub fn pump_backoff(streak: u32) -> Duration {
    match streak {
        0 => Duration::from_secs(2),
        1 => Duration::from_secs(4),
        _ => Duration::from_secs(8),
    }
}

/// Map an ffmpeg stderr tail to a short, user-showable failure
/// reason (v1.2.2) — the tile displays it under the retry status
/// after repeated failures so a dead feed is diagnosable ("the
/// camera moved", "wrong camera-account password") instead of a
/// bare "retrying" loop. None when nothing recognisable is found.
pub fn ffmpeg_failure_reason(stderr_tail: &str) -> Option<String> {
    let s = stderr_tail.to_lowercase();
    if s.contains("401") || s.contains("unauthorized") {
        Some("401 Unauthorized".to_string())
    } else if s.contains("connection refused") {
        Some("Connection refused".to_string())
    } else if s.contains("timed out") || s.contains("timeout") {
        Some("Connection timed out".to_string())
    } else if s.contains("failed to resolve hostname")
        || s.contains("name or service not known")
        || s.contains("temporary failure in name resolution")
        || s.contains("could not resolve")
    {
        Some("Host not found".to_string())
    } else if s.contains("404") {
        Some("Stream path not found (404)".to_string())
    } else if s.contains("connection reset") {
        Some("Connection reset by camera".to_string())
    } else {
        None
    }
}

/// What sync_pumps should do with one pump slot this frame
/// (v1.2.2), as a pure decision so the recovery rules are testable:
/// a missing pump starts fresh; a source change (camera edited —
/// the generation in the source key moves — quality/size change,
/// ffmpeg moved) restarts IMMEDIATELY with the failure streak
/// reset; a forced retry (Retry button, Watch/tile click, window
/// focus, player success, record change) does the same from
/// Backoff; a lapsed backoff retries on schedule, keeping the
/// streak so the delays keep growing to the cap.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PumpAction {
    Keep,
    StartFresh,
    RetryDue,
}

pub fn pump_action(
    state: Option<PumpState>,
    source_matches: bool,
    force: bool,
    due: bool,
) -> PumpAction {
    match state {
        None => PumpAction::StartFresh,
        Some(_) if !source_matches => PumpAction::StartFresh,
        Some(PumpState::Backoff) if force => PumpAction::StartFresh,
        Some(PumpState::Backoff) if due => PumpAction::RetryDue,
        Some(_) => PumpAction::Keep,
    }
}

/// Whether a camera's TILE pump should exist at all (v1.2.2):
/// previews on, camera not hidden, preview not user-paused, and
/// the in-app player is NOT open for this camera — Tapo cameras
/// allow only ~2 concurrent RTSP sessions, so while the player
/// holds one, the tile pump stands down and frees its session
/// (it restarts, fresh, when the player closes).
pub fn tile_pump_wanted(
    previews: bool,
    hidden: bool,
    user_paused: bool,
    player_holds_this: bool,
) -> bool {
    previews && !hidden && !user_paused && !player_holds_this
}

/// How long to wait before the next frame is needed purely to fire
/// a lapsed pump retry — clamped so a Backoff pump restarts on
/// schedule even when nothing else is repainting the window
/// (v1.2.2: in v1.2.1 a lone backed-off pump could sit past its
/// retry time indefinitely on an idle window).
pub fn backoff_wake_delay(retry_at: Option<Instant>, now: Instant) -> Duration {
    match retry_at {
        Some(t) => t
            .saturating_duration_since(now)
            .clamp(Duration::from_millis(50), Duration::from_secs(1)),
        None => Duration::from_millis(500),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PumpState {
    Connecting,
    Live,
    Backoff,
}

pub enum FrameMsg {
    Frame(Vec<u8>),
    /// ffmpeg exited / hit EOF; carries the mapped failure reason
    /// (ffmpeg_failure_reason over the stderr tail), when known.
    Ended(Option<String>),
}
