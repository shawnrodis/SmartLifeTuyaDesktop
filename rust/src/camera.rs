use crate::config::{Camera, LayoutCfg, Prefs};
use crate::convert::pct_encode_component;
use crate::obfuscate::deobfuscate;
use crate::ffmpeg::{PLAYER_H, PLAYER_W, TILE_H, TILE_W};
use std::process::Command;
pub fn camera_path(cam: &Camera) -> String {
    if cam.template == "custom" {
        let p = cam.path.trim();
        if p.is_empty() {
            return "/".to_string();
        }
        if p.starts_with('/') {
            p.to_string()
        } else {
            format!("/{}", p)
        }
    } else if cam.template == "tapo2" {
        "/stream2".to_string()
    } else {
        "/stream1".to_string()
    }
}

/// rtsp://user:pass@host:port/path — credentials percent-encoded so
/// passwords containing @ : / # etc. survive. The stored password is
/// obfuscated; it is deobfuscated here.
pub fn build_rtsp_url(cam: &Camera) -> String {
    let port = if cam.port == 0 { 554 } else { cam.port };
    format!(
        "rtsp://{}:{}@{}:{}{}",
        pct_encode_component(&cam.user),
        pct_encode_component(&deobfuscate(&cam.password)),
        cam.host.trim(),
        port,
        camera_path(cam)
    )
}

/// Per-camera card size (v1.2.1), stored in the layout keyed by
/// item id: Normal = one grid cell, Large = two columns, Wide = the
/// full section row. Cameras only — devices and scenes never resize.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CamSize {
    Normal,
    Large,
    Wide,
}

impl CamSize {
    pub fn parse(s: Option<&String>) -> CamSize {
        match s.map(|v| v.as_str()) {
            Some("large") => CamSize::Large,
            Some("wide") => CamSize::Wide,
            _ => CamSize::Normal,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            CamSize::Normal => "normal",
            CamSize::Large => "large",
            CamSize::Wide => "wide",
        }
    }
    /// Grid columns this size occupies in a `cols`-column grid.
    pub fn span(&self, cols: usize) -> usize {
        let want = match self {
            CamSize::Normal => 1,
            CamSize::Large => 2,
            CamSize::Wide => cols,
        };
        want.max(1).min(cols.max(1))
    }
}

pub fn cam_size_of(layout: &LayoutCfg, item_id: &str) -> CamSize {
    CamSize::parse(layout.cam_sizes.get(item_id))
}

/// A tile feed spec: (RTSP URL, pump frame width, pump frame
/// height). Normal and Large tiles decode the cheap Stream 2 on
/// Tapo templates (the source selection is unchanged by size);
/// only the pump resolution grows (480×270 → 960×540). A Wide tile
/// uses the card's own selected quality, like the big player.
pub fn tile_feed(cam: &Camera, size: CamSize) -> (String, usize, usize) {
    match size {
        CamSize::Normal => (tile_rtsp_url(cam), TILE_W, TILE_H),
        CamSize::Large => (tile_rtsp_url(cam), PLAYER_W, PLAYER_H),
        CamSize::Wide => (build_rtsp_url(cam), PLAYER_W, PLAYER_H),
    }
}

/// The URL a TILE preview uses (v1.2.0): Stream 2 (SD) whenever a
/// Tapo template is set — cheap to decode for a small tile — and the
/// camera's own path otherwise. The big player uses build_rtsp_url
/// (the camera's current quality selection; HD = Stream 1).
pub fn tile_rtsp_url(cam: &Camera) -> String {
    if cam.template == "tapo1" || cam.template == "tapo2" {
        let mut c = cam.clone();
        c.template = "tapo2".to_string();
        build_rtsp_url(&c)
    } else {
        build_rtsp_url(cam)
    }
}

/// In-browser watch URL for a configured stream gateway, or None.
/// The gateway stream must be named exactly like the camera here.
/// go2rtc: {gw}/stream.html?src={name} · MediaMTX: {gw}/{name}/index.m3u8
pub fn gateway_watch_url(cam: &Camera, prefs: &Prefs) -> Option<String> {
    let gw = prefs.gateway_url.trim().trim_end_matches('/');
    if gw.is_empty() {
        return None;
    }
    let name = pct_encode_component(&cam.name);
    if prefs.gateway_type == "mediamtx" {
        Some(format!("{}/{}/index.m3u8", gw, name))
    } else {
        Some(format!("{}/stream.html?src={}", gw, name))
    }
}

pub fn camera_status_text(cam: &Camera) -> String {
    let port = if cam.port == 0 { 554 } else { cam.port };
    let q = if cam.template == "custom" {
        camera_path(cam)
    } else if cam.template == "tapo2" {
        "Stream 2 · Standard".to_string()
    } else {
        "Stream 1 · High".to_string()
    };
    format!("{}:{} · {}", cam.host, port, q)
}

#[cfg(target_os = "windows")]
pub fn open_command(url: &str) -> Command {
    let mut c = Command::new("cmd");
    c.args(["/C", "start", "", url]);
    c
}
#[cfg(target_os = "macos")]
pub fn open_command(url: &str) -> Command {
    let mut c = Command::new("open");
    c.arg(url);
    c
}
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub fn open_command(url: &str) -> Command {
    let mut c = Command::new("xdg-open");
    c.arg(url);
    c
}

/// Open a URL in the default browser (gateway watch page / releases page).
pub fn open_external(url: &str) -> Result<(), String> {
    open_command(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{}", e))
}
