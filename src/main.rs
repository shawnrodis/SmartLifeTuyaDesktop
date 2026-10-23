#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! SmartLife & Tuya Desktop (Windows edition) — a clean dashboard for
//! Smart Life / Tuya devices, 1:1 with the single-file web edition in
//! `web/SmartLife_Tuya_Desktop.html`. Both talk to Tuya's legacy consumer
//! cloud API ("homeassistant" API):
//!   POST {base}auth.do   — login / token refresh
//!   POST {base}skill     — Discovery (device list) / Control (commands)
//! All settings live in one JSON config file (see `config_path`).
//! The shared design tokens are documented in DESIGN.md — keep them in step.

use eframe::egui;
use egui::{
    Align, Align2, Button, Color32, ComboBox, FontId, Frame, Layout, Margin, RichText, Rounding,
    ScrollArea, Sense, Stroke, TextEdit,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Design tokens (DESIGN.md — mirrored 1:1 with the web edition's CSS vars)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Palette {
    bg: Color32,
    card: Color32,
    border: Color32,
    text: Color32,
    dim: Color32,
    faint: Color32,
    track: Color32,
}

const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x0b, 0x0e, 0x14),
    card: Color32::from_rgb(0x15, 0x1a, 0x24),
    border: Color32::from_rgb(0x23, 0x2b, 0x3a),
    text: Color32::from_rgb(0xe7, 0xeb, 0xf2),
    dim: Color32::from_rgb(0x8b, 0x94, 0xa7),
    faint: Color32::from_rgb(0x5b, 0x64, 0x78),
    track: Color32::from_rgb(0x2a, 0x32, 0x45),
};
const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xee, 0xf1, 0xf6),
    card: Color32::from_rgb(0xff, 0xff, 0xff),
    border: Color32::from_rgb(0xdd, 0xe3, 0xee),
    text: Color32::from_rgb(0x17, 0x20, 0x2e),
    dim: Color32::from_rgb(0x5d, 0x68, 0x79),
    faint: Color32::from_rgb(0x8b, 0x94, 0xa7),
    track: Color32::from_rgb(0xdb, 0xe1, 0xec),
};
/// "Black" theme — true-black background for OLED screens; near-black
/// cards and subtle borders, same text/accent system as Dark.
const BLACK: Palette = Palette {
    bg: Color32::from_rgb(0x00, 0x00, 0x00),
    card: Color32::from_rgb(0x0c, 0x0d, 0x11),
    border: Color32::from_rgb(0x1f, 0x23, 0x2e),
    text: Color32::from_rgb(0xe7, 0xeb, 0xf2),
    dim: Color32::from_rgb(0x8b, 0x94, 0xa7),
    faint: Color32::from_rgb(0x5b, 0x64, 0x78),
    track: Color32::from_rgb(0x22, 0x25, 0x2e),
};
const DANGER: Color32 = Color32::from_rgb(0xf4, 0x3f, 0x5e);
const OK_GREEN: Color32 = Color32::from_rgb(0x22, 0xc5, 0x5e);
const TOAST_BG_DARK: Color32 = Color32::from_rgb(0x1e, 0x25, 0x34);
const TOAST_BG_BLACK: Color32 = Color32::from_rgb(0x14, 0x16, 0x1c);
const TOAST_BORDER_BLACK: Color32 = Color32::from_rgb(0x26, 0x2a, 0x35);
const TOAST_BG_LIGHT: Color32 = Color32::from_rgb(0x17, 0x20, 0x2e);
const TOAST_BORDER: Color32 = Color32::from_rgb(0x33, 0x3f, 0x58);
const ERR_BG: Color32 = Color32::from_rgb(0x4c, 0x11, 0x20);
const ERR_BORDER: Color32 = Color32::from_rgb(0x88, 0x13, 0x37);

const R_CARD: f32 = 18.0;
const R_CARD_COMPACT: f32 = 14.0;
const R_BTN: f32 = 11.0;
const R_LOGIN: f32 = 22.0;

const ACCENTS: [(&str, &str); 7] = [
    ("Indigo", "#6366f1"),
    ("Sky", "#0ea5e9"),
    ("Teal", "#14b8a6"),
    ("Green", "#22c55e"),
    ("Amber", "#f59e0b"),
    ("Rose", "#f43f5e"),
    ("Violet", "#8b5cf6"),
];

/// Colour swatches shown on colour-capable cards (whites + basics) — same as web.
const SWATCHES: [(&str, &str); 10] = [
    ("Warm white", "#ffd9a0"),
    ("Cool white", "#eaf2ff"),
    ("Red", "#ff2d2d"),
    ("Orange", "#ff7a00"),
    ("Yellow", "#ffd400"),
    ("Green", "#22c55e"),
    ("Cyan", "#00c2ff"),
    ("Blue", "#2456ff"),
    ("Purple", "#8b3dff"),
    ("Pink", "#ff4fa3"),
];

fn type_icon(kind: &str) -> &'static str {
    match kind {
        "light" => "💡",
        "strip" => "🌈",
        "switch" => "🔌",
        "scene" => "🎬",
        "cover" => "🪟",
        "climate" => "🌡️",
        "fan" => "🌀",
        "lock" => "🔒",
        "sensor" => "📡",
        "camera" => "📷",
        _ => "🔘",
    }
}

fn hex_to_color32(hex: &str) -> Color32 {
    let h = hex.trim_start_matches('#');
    if h.len() == 6 {
        if let Ok(n) = u32::from_str_radix(h, 16) {
            return Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8);
        }
    }
    Color32::from_rgb(0x63, 0x66, 0xf1)
}

// ---------------------------------------------------------------------------
// Config (JSON, %APPDATA%\SmartLifeTuyaDesktop\config.json on Windows)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
struct Creds {
    user_name: String,
    /// Stored obfuscated (XOR + base64), exactly like the web edition —
    /// this is NOT encryption, it just avoids plain text at a glance.
    password: String,
    country_code: String,
    platform: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Session {
    access_token: String,
    refresh_token: Option<String>,
    expires_at_ms: i64,
    region: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Group {
    id: String,
    name: String,
}

/// A user-added RTSP camera (e.g. Tapo). Cameras are NOT Tuya devices,
/// but on the dashboard they are peers of devices: same sections, name
/// sort / custom order, pinning, groups and hiding.
#[derive(Clone, Serialize, Deserialize)]
struct Camera {
    id: String,
    name: String,
    user: String,
    /// Stored obfuscated (XOR + base64), exactly like the account password.
    password: String,
    host: String,
    #[serde(default = "default_rtsp_port")]
    port: u16,
    /// "tapo1" | "tapo2" | "custom"
    template: String,
    #[serde(default)]
    path: String,
}

fn default_rtsp_port() -> u16 {
    554
}

const CAMERA_TEMPLATES: [(&str, &str); 3] = [
    ("tapo1", "Tapo Stream 1 — High Quality"),
    ("tapo2", "Tapo Stream 2 — Standard"),
    ("custom", "Custom path"),
];

#[derive(Clone, Serialize, Deserialize, Default)]
struct LayoutCfg {
    #[serde(default)]
    groups: Vec<Group>,
    #[serde(default)]
    assign: HashMap<String, String>,
    #[serde(default)]
    alias: HashMap<String, String>,
    #[serde(default)]
    hidden: Vec<String>,
    #[serde(default)]
    pins: Vec<String>,
    #[serde(default)]
    section_order: HashMap<String, Vec<String>>,
    #[serde(default)]
    collapsed: HashMap<String, bool>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Prefs {
    theme: String,
    accent: String,
    density: String,
    autorefresh: bool,
    interval: u64,
    proxy_preset: String,
    proxy_custom: String,
    /// Stream gateway base URL for in-app camera playback ("" = none).
    #[serde(default)]
    gateway_url: String,
    /// "go2rtc" | "mediamtx"
    #[serde(default = "default_gateway_type")]
    gateway_type: String,
    /// External player command for ▶ Watch when no gateway is set.
    #[serde(default = "default_player_cmd")]
    player_cmd: String,
}

fn default_gateway_type() -> String {
    "go2rtc".to_string()
}

fn default_player_cmd() -> String {
    "vlc".to_string()
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            accent: "#6366f1".into(),
            density: "comfortable".into(),
            autorefresh: true,
            interval: 30,
            proxy_preset: "auto".into(),
            proxy_custom: String::new(),
            gateway_url: String::new(),
            gateway_type: default_gateway_type(),
            player_cmd: default_player_cmd(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
struct Config {
    #[serde(default)]
    creds: Option<Creds>,
    #[serde(default)]
    session: Option<Session>,
    #[serde(default)]
    prefs: Prefs,
    #[serde(default)]
    layout: LayoutCfg,
    #[serde(default)]
    cameras: Vec<Camera>,
    #[serde(default)]
    update: UpdateCfg,
    #[serde(default)]
    cache_devices: Option<Value>,
}

/// Auto-update bookkeeping (mirrors the web edition's slc_update).
#[derive(Serialize, Deserialize, Clone, Default)]
struct UpdateCfg {
    #[serde(default)]
    last_check_ms: i64,
    #[serde(default)]
    dismissed: String,
}

fn config_path() -> PathBuf {
    let base: PathBuf = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }
    .unwrap_or_else(|| PathBuf::from("."));
    base.join("SmartLifeTuyaDesktop").join("config.json")
}

fn load_config() -> Config {
    let path = config_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str::<Config>(&text) {
            return cfg;
        }
    }
    Config::default()
}

fn save_config(cfg: &Config) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(&path, text);
    }
}

// ---------------------------------------------------------------------------
// Password obfuscation (same XOR 0x5A + base64 scheme as the web edition)
// ---------------------------------------------------------------------------

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

fn b64_decode(s: &str) -> Vec<u8> {
    let mut rev = [255u8; 256];
    for (i, &c) in B64.iter().enumerate() {
        rev[c as usize] = i as u8;
    }
    let mut out = Vec::new();
    let bytes: Vec<u8> = s.bytes().filter(|&b| b != b'=').collect();
    for chunk in bytes.chunks(4) {
        let mut n: u32 = 0;
        for (i, &c) in chunk.iter().enumerate() {
            n |= (rev[c as usize] as u32) << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    out
}

fn obfuscate(plain: &str) -> String {
    let xored: Vec<u8> = plain.bytes().map(|b| b ^ 0x5A).collect();
    b64_encode(&xored)
}

fn deobfuscate(s: &str) -> String {
    let bytes = b64_decode(s);
    String::from_utf8_lossy(&bytes.iter().map(|b| b ^ 0x5A).collect::<Vec<u8>>()).into_owned()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tuya "homeassistant" API — endpoints, proxy, value scales (mirror the web)
// ---------------------------------------------------------------------------

fn region_base(region: &str) -> &'static str {
    match region {
        "US" => "https://px1.tuyaus.com/homeassistant/",
        "CN" => "https://px1.tuyacn.com/homeassistant/",
        _ => "https://px1.tuyaeu.com/homeassistant/",
    }
}

fn region_key_from_code(code: &str) -> &'static str {
    match code {
        "1" => "US",
        "86" => "CN",
        _ => "EU",
    }
}

fn region_from_token(token: &str) -> Option<&'static str> {
    match token.get(0..2) {
        Some("EU") => Some("EU"),
        Some("AY") => Some("CN"),
        Some("AZ") => Some("US"),
        _ => None,
    }
}

const SMARTATHOME_PROXY: &str = "https://cors.smartathome.co.uk/";

const PROXY_PRESETS: [(&str, &str); 6] = [
    ("auto", "Auto — direct for US/EU, community proxy for CN (recommended)"),
    ("smartathome", "SmartAtHome community proxy (all requests)"),
    ("corsproxy", "corsproxy.io (encoded URL)"),
    ("cors-anywhere", "cors-anywhere.herokuapp.com (may need activation)"),
    ("none", "None — direct connection"),
    ("custom", "Custom…"),
];

fn url_encode(s: &str) -> String {
    // application/x-www-form-urlencoded component encoding (for proxy {url}).
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Resolve the final URL for a request — identical rules to the web edition.
fn apply_proxy(url: &str, preset: &str, custom: &str, region: &str) -> String {
    match preset {
        "none" => url.to_string(),
        "smartathome" => format!("{}{}", SMARTATHOME_PROXY, url),
        "cors-anywhere" => format!("https://cors-anywhere.herokuapp.com/{}", url),
        "corsproxy" => format!("https://corsproxy.io/?url={}", url_encode(url)),
        "custom" => {
            let c = custom.trim();
            if c.is_empty() {
                url.to_string()
            } else if c.contains("{url}") {
                c.replace("{url}", &url_encode(url))
            } else {
                format!("{}{}", c, url)
            }
        }
        _ => {
            // auto: US/EU endpoints accept direct calls; CN goes via the proxy.
            if region == "CN" {
                format!("{}{}", SMARTATHOME_PROXY, url)
            } else {
                url.to_string()
            }
        }
    }
}

// Value scales (identical to the web edition):
// - device brightness is reported 0..1000; UI % = brightness/10.
//   brightnessSet sends the % value; its useful range is 11..100.
// - colour temperature is SENT on a 1000..10000 scale labelled 2700K/6500K,
//   but REPORTED stretched: reported = (sent-1000)*4.033 + 1000.
// - colour is HSV: hue 0..360, saturation 0..1, brightness = %.

fn clampf(v: f64, lo: f64, hi: f64) -> f64 {
    v.min(hi).max(lo)
}

fn brightness_to_pct(reported: f64) -> i64 {
    clampf((reported / 10.0).round(), 0.0, 100.0) as i64
}

fn temp_reported_to_slider(reported: f64) -> i64 {
    clampf(((reported - 1000.0) / 4.033 + 1000.0).round(), 1000.0, 10000.0) as i64
}

fn temp_slider_to_reported(slider: i64) -> i64 {
    (((slider - 1000) as f64) * 4.033 + 1000.0).round() as i64
}

fn temp_slider_to_kelvin(slider: i64) -> i64 {
    ((2700.0 + (clampf(slider as f64, 1000.0, 10000.0) - 1000.0) * (3800.0 / 9000.0)) / 10.0).round() as i64 * 10
}

fn hex_to_hsv(hex: &str) -> (f64, f64, f64) {
    let h = hex.trim_start_matches('#');
    let n = u32::from_str_radix(h, 16).unwrap_or(0);
    let (r, g, b) = (
        ((n >> 16) & 255) as f64 / 255.0,
        ((n >> 8) & 255) as f64 / 255.0,
        (n & 255) as f64 / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let mut hue = 0.0;
    if d > 0.0 {
        if max == r {
            hue = ((g - b) / d).rem_euclid(6.0);
        } else if max == g {
            hue = (b - r) / d + 2.0;
        } else {
            hue = (r - g) / d + 4.0;
        }
        hue *= 60.0;
        if hue < 0.0 {
            hue += 360.0;
        }
    }
    (hue.round(), if max > 0.0 { d / max } else { 0.0 }, max)
}

fn hsv_to_hex(h: f64, s: f64, v: f64) -> String {
    let h = h.rem_euclid(360.0);
    let s = clampf(s, 0.0, 1.0);
    let v = clampf(v, 0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    format!(
        "#{:02x}{:02x}{:02x}",
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8
    )
}

fn norm_saturation(s: f64) -> f64 {
    if s > 1.0 {
        clampf(s / 100.0, 0.0, 1.0)
    } else {
        clampf(s, 0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// Cameras — RTSP URL building + playback hand-off (mirror the web edition)
// ---------------------------------------------------------------------------

/// Percent-encode one URL component with JavaScript `encodeURIComponent`
/// semantics, so both editions build byte-identical stream URLs.
fn pct_encode_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*'
            | b'\'' | b'(' | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn camera_path(cam: &Camera) -> String {
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
fn build_rtsp_url(cam: &Camera) -> String {
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

/// In-browser watch URL for a configured stream gateway, or None.
/// The gateway stream must be named exactly like the camera here.
/// go2rtc: {gw}/stream.html?src={name} · MediaMTX: {gw}/{name}/index.m3u8
fn gateway_watch_url(cam: &Camera, prefs: &Prefs) -> Option<String> {
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

fn camera_status_text(cam: &Camera) -> String {
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
fn open_command(url: &str) -> Command {
    let mut c = Command::new("cmd");
    c.args(["/C", "start", "", url]);
    c
}
#[cfg(target_os = "macos")]
fn open_command(url: &str) -> Command {
    let mut c = Command::new("open");
    c.arg(url);
    c
}
#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn open_command(url: &str) -> Command {
    let mut c = Command::new("xdg-open");
    c.arg(url);
    c
}

/// Open a URL in the default browser (gateway watch page / releases page).
fn open_external(url: &str) -> Result<(), String> {
    open_command(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{}", e))
}

/// Launch the configured external player with the stream URL.
/// Returns the program name on success.
fn launch_player(player_cmd: &str, url: &str) -> Result<String, String> {
    let mut parts = player_cmd.split_whitespace();
    let prog = parts.next().unwrap_or("vlc").to_string();
    Command::new(&prog)
        .args(parts)
        .arg(url)
        .spawn()
        .map(|_| prog.clone())
        .map_err(|e| format!("{}: {}", prog, e))
}

// ---------------------------------------------------------------------------
// Device model (port of the web edition's parseDevice)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Device {
    id: String,
    name: String,
    dev_type: String,
    kind: String, // "scene" | "light" | "switch" | "other"
    icon_kind: String,
    is_scene: bool,
    online: bool,
    state: bool,
    has_brightness: bool,
    has_temp: bool,
    has_color: bool,
    brightness_pct: Option<i64>,
    temp_slider: Option<i64>,
    hue: Option<f64>,
    saturation: Option<f64>,
    color_mode: Option<String>,
    raw: Value,
}

fn data_f64(data: &Map<String, Value>, key: &str) -> Option<f64> {
    data.get(key).and_then(|v| v.as_f64())
}

fn parse_device(raw: &Value) -> Device {
    let empty = Map::new();
    let data = raw.get("data").and_then(|v| v.as_object()).unwrap_or(&empty);
    let dev_type = raw
        .get("dev_type")
        .or_else(|| raw.get("devType"))
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    let is_scene = dev_type == "scene";
    let has_brightness = data.contains_key("brightness");
    let has_temp = data.contains_key("color_temp");
    let has_color =
        data.contains_key("color_mode") || data.contains_key("hue") || data.contains_key("saturation");
    let kind = if is_scene {
        "scene"
    } else if has_color || has_temp || has_brightness {
        "light"
    } else if dev_type == "switch" {
        "switch"
    } else {
        "other"
    };
    let name = raw
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| raw.get("id").and_then(|v| v.as_str()).unwrap_or("Device").to_string());
    let icon_kind = if kind == "light" {
        let lower = name.to_lowercase();
        let looks_strip = ["strip", "bar", "led strip", "tape"].iter().any(|p| lower.contains(p))
            || (has_color && !has_temp && !has_brightness);
        if looks_strip {
            "strip".to_string()
        } else {
            "light".to_string()
        }
    } else if ["light", "strip", "switch", "scene", "cover", "climate", "fan", "lock", "sensor"]
        .contains(&dev_type.as_str())
    {
        dev_type.clone()
    } else {
        "default".to_string()
    };
    Device {
        id: raw.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        name,
        dev_type,
        kind: kind.to_string(),
        icon_kind,
        is_scene,
        online: data.get("online").and_then(|v| v.as_bool()) != Some(false),
        state: data.get("state").and_then(|v| v.as_bool()) == Some(true),
        has_brightness,
        has_temp,
        has_color,
        brightness_pct: if has_brightness {
            data_f64(data, "brightness").map(brightness_to_pct)
        } else {
            None
        },
        temp_slider: if has_temp {
            data_f64(data, "color_temp").map(temp_reported_to_slider)
        } else {
            None
        },
        hue: data_f64(data, "hue"),
        saturation: data_f64(data, "saturation").map(norm_saturation),
        color_mode: data.get("color_mode").and_then(|v| v.as_str()).map(|s| s.to_string()),
        raw: raw.clone(),
    }
}

/// Keep previously-known data fields when a fresh discovery omits them
/// (same merge the web edition performs).
fn merge_device_data(fresh: Vec<Value>, previous: &[Value]) -> Vec<Value> {
    if previous.is_empty() {
        return fresh;
    }
    let prev_by_id: HashMap<String, &Map<String, Value>> = previous
        .iter()
        .filter_map(|d| {
            let id = d.get("id")?.as_str()?;
            let data = d.get("data")?.as_object()?;
            Some((id.to_string(), data))
        })
        .collect();
    fresh
        .into_iter()
        .map(|mut d| {
            let id = d.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if let Some(prev_data) = prev_by_id.get(&id) {
                if let Some(data) = d.get_mut("data").and_then(|v| v.as_object_mut()) {
                    for (k, v) in prev_data.iter() {
                        if !data.contains_key(k) {
                            data.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            d
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Section model (port of the web edition's buildSections / ordering rules)
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
enum SectionKind {
    Pinned,
    Lighting,
    Cameras,
    Group,
    Ungrouped,
    Scenes,
}

#[derive(Clone)]
struct Section {
    key: String,
    order_key: String,
    title: String,
    kind: SectionKind,
    group_id: Option<String>,
    ids: Vec<String>,
}

/// One dashboard card's identity for the section model — a Tuya device
/// or a user-added camera. Sections, ordering and search work on entries;
/// rendering looks the full record up by id.
#[derive(Clone)]
struct CardEntry {
    id: String,
    name: String,
    is_scene: bool,
    /// Light-capable (brightness / colour temp / colour) — such devices
    /// live in the automatic Lighting section unless grouped.
    light: bool,
    /// A user-added RTSP camera — lives in the automatic Cameras
    /// section unless grouped.
    camera: bool,
}

/// Light-capable = can do brightness, colour temperature or colour
/// (bulbs and RGB strips; cameras never are).
fn device_is_light(d: &Device) -> bool {
    !d.is_scene && (d.has_brightness || d.has_temp || d.has_color)
}

fn device_entry(d: &Device) -> CardEntry {
    CardEntry {
        id: d.id.clone(),
        name: d.name.clone(),
        is_scene: d.is_scene,
        light: device_is_light(d),
        camera: false,
    }
}

fn camera_entry(c: &Camera) -> CardEntry {
    // Item ids are "cam:{record id}" everywhere (layout keys, section
    // ids, camera_index) — the record itself keeps its bare id.
    CardEntry {
        id: format!("cam:{}", c.id),
        name: c.name.clone(),
        is_scene: false,
        light: false,
        camera: true,
    }
}

/// The "no explicit group" label for an item in the group dropdown.
fn auto_section_label(e: &CardEntry) -> &'static str {
    if e.camera {
        "Cameras (automatic)"
    } else if e.light {
        "Lighting (automatic)"
    } else {
        "Ungrouped"
    }
}

// ---------------------------------------------------------------------------
// Group controls (broadcast, not binding) — a group action computes a
// plan of per-member Control commands and sends them one by one. No
// group state is stored: header widgets reflect member states
// (any-on, averages), exactly like the web edition.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum GroupKind {
    Toggle,
    Brightness,
    Temp,
    Color,
}

/// Master-toggle display state: ON if ANY (non-scene) member is on.
fn group_master_on(members: &[Device]) -> bool {
    members.iter().filter(|d| !d.is_scene).any(|d| d.state)
}

fn group_average(values: &[i64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<i64>() as f64 / values.len() as f64)
    }
}

/// The fan-out plan for one group action. `num` carries the brightness
/// percent or temperature slider value; `hex` the colour. Offline
/// members are skipped (their cards disable controls too).
fn group_plan(
    members: &[Device],
    kind: GroupKind,
    num: i64,
    hex: Option<&str>,
) -> Vec<(String, &'static str, Value)> {
    let devs: Vec<&Device> = members
        .iter()
        .filter(|d| !d.is_scene && d.online)
        .collect();
    let mut plan = Vec::new();
    match kind {
        GroupKind::Toggle => {
            let target: i64 = if group_master_on(members) { 0 } else { 1 };
            for d in devs {
                plan.push((d.id.clone(), "turnOnOff", json!(target)));
            }
        }
        GroupKind::Brightness => {
            let pct = num.clamp(11, 100);
            for d in devs.iter().filter(|d| d.has_brightness) {
                plan.push((d.id.clone(), "brightnessSet", json!(pct)));
            }
        }
        GroupKind::Temp => {
            let tv = num.clamp(1000, 10000);
            for d in devs.iter().filter(|d| d.has_temp) {
                plan.push((d.id.clone(), "colorTemperatureSet", json!(tv)));
            }
        }
        GroupKind::Color => {
            let (mut hue, mut sat, _) = hex_to_hsv(hex.unwrap_or("#ffffff"));
            if sat < 0.02 {
                hue = 0.0;
                sat = 0.0;
            }
            let sat = (sat * 100.0).round() / 100.0;
            for d in devs.iter().filter(|d| d.has_color) {
                plan.push((
                    d.id.clone(),
                    "colorSet",
                    json!({
                        "hue": hue as i64,
                        "saturation": sat,
                        "brightness": d.brightness_pct.unwrap_or(100)
                    }),
                ));
            }
        }
    }
    plan
}

/// Display name of a device (alias wins) — used for card titles.
fn display_name(layout: &LayoutCfg, d: &Device) -> String {
    layout.alias.get(&d.id).cloned().unwrap_or_else(|| d.name.clone())
}

fn entry_display_name(layout: &LayoutCfg, e: &CardEntry) -> String {
    layout.alias.get(&e.id).cloned().unwrap_or_else(|| e.name.clone())
}

fn compare_by_name(layout: &LayoutCfg, a: &CardEntry, b: &CardEntry) -> std::cmp::Ordering {
    let na = entry_display_name(layout, a).to_lowercase();
    let nb = entry_display_name(layout, b).to_lowercase();
    na.cmp(&nb).then_with(|| a.id.cmp(&b.id))
}

fn order_section_items(
    items: &[&CardEntry],
    custom_order: Option<&Vec<String>>,
    layout: &LayoutCfg,
) -> Vec<String> {
    let by_id: HashMap<&str, &CardEntry> = items.iter().map(|d| (d.id.as_str(), *d)).collect();
    let mut used: HashSet<&str> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    if let Some(order) = custom_order {
        for id in order {
            if by_id.contains_key(id.as_str()) && used.insert(id.as_str()) {
                out.push(id.clone());
            }
        }
    }
    let mut rest: Vec<&CardEntry> = items.iter().filter(|d| !used.contains(d.id.as_str())).cloned().collect();
    rest.sort_by(|a, b| compare_by_name(layout, a, b));
    out.extend(rest.iter().map(|d| d.id.clone()));
    out
}

fn filter_entries<'a>(entries: &'a [CardEntry], layout: &LayoutCfg, query: &str) -> Vec<&'a CardEntry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return entries.iter().collect();
    }
    entries
        .iter()
        .filter(|d| {
            entry_display_name(layout, d).to_lowercase().contains(&q) || d.name.to_lowercase().contains(&q)
        })
        .collect()
}

fn build_sections(
    entries: &[CardEntry],
    layout: &LayoutCfg,
    editing: bool,
    query: &str,
) -> Vec<Section> {
    let visible: Vec<&CardEntry> = filter_entries(entries, layout, query)
        .into_iter()
        .filter(|d| !layout.hidden.contains(&d.id))
        .collect();
    let scenes: Vec<&CardEntry> = visible.iter().filter(|d| d.is_scene).cloned().collect();
    let normal: Vec<&CardEntry> = visible.iter().filter(|d| !d.is_scene).cloned().collect();
    let pin_set: HashSet<&str> = layout.pins.iter().map(|s| s.as_str()).collect();
    let so = &layout.section_order;
    let mut sections: Vec<Section> = Vec::new();

    let group_ids: HashSet<&str> = layout.groups.iter().map(|g| g.id.as_str()).collect();
    let assigned = |d: &CardEntry| -> bool {
        match layout.assign.get(&d.id) {
            Some(g) => group_ids.contains(g.as_str()),
            None => false,
        }
    };
    let pinned: Vec<&CardEntry> = normal.iter().filter(|d| pin_set.contains(d.id.as_str())).cloned().collect();
    if !pinned.is_empty() {
        sections.push(Section {
            key: "pinned".into(),
            order_key: "pinned".into(),
            title: "Pinned".into(),
            kind: SectionKind::Pinned,
            group_id: None,
            ids: order_section_items(&pinned, so.get("pinned"), layout),
        });
    }
    // Built-in automatic sections (Pinned → Lighting → Cameras →
    // groups → Ungrouped → Scenes): an explicit group assignment wins;
    // unassigned lights land in Lighting, unassigned cameras in Cameras.
    let lighting: Vec<&CardEntry> = normal
        .iter()
        .filter(|d| !assigned(d) && d.light)
        .cloned()
        .collect();
    sections.push(Section {
        key: "lighting".into(),
        order_key: "lighting".into(),
        title: "Lighting".into(),
        kind: SectionKind::Lighting,
        group_id: None,
        ids: order_section_items(&lighting, so.get("lighting"), layout),
    });
    let cameras_sec: Vec<&CardEntry> = normal
        .iter()
        .filter(|d| !assigned(d) && d.camera)
        .cloned()
        .collect();
    sections.push(Section {
        key: "cameras".into(),
        order_key: "cameras".into(),
        title: "Cameras".into(),
        kind: SectionKind::Cameras,
        group_id: None,
        ids: order_section_items(&cameras_sec, so.get("cameras"), layout),
    });
    for g in &layout.groups {
        let devs: Vec<&CardEntry> = normal
            .iter()
            .filter(|d| layout.assign.get(&d.id) == Some(&g.id))
            .cloned()
            .collect();
        let ok = format!("group:{}", g.id);
        sections.push(Section {
            key: g.id.clone(),
            order_key: ok.clone(),
            title: g.name.clone(),
            kind: SectionKind::Group,
            group_id: Some(g.id.clone()),
            ids: order_section_items(&devs, so.get(&ok), layout),
        });
    }
    let ungrouped: Vec<&CardEntry> = normal
        .iter()
        .filter(|d| !assigned(d) && !d.light && !d.camera)
        .cloned()
        .collect();
    sections.push(Section {
        key: "ungrouped".into(),
        order_key: "ungrouped".into(),
        title: "Ungrouped".into(),
        kind: SectionKind::Ungrouped,
        group_id: None,
        ids: order_section_items(&ungrouped, so.get("ungrouped"), layout),
    });
    if !scenes.is_empty() {
        sections.push(Section {
            key: "scenes".into(),
            order_key: "scenes".into(),
            title: "Scenes".into(),
            kind: SectionKind::Scenes,
            group_id: None,
            ids: order_section_items(&scenes, so.get("scenes"), layout),
        });
    }
    sections.retain(|s| {
        !s.ids.is_empty()
            || (editing
                && (s.kind == SectionKind::Group
                    || s.kind == SectionKind::Lighting
                    || s.kind == SectionKind::Cameras))
    });
    sections
}

// ---------------------------------------------------------------------------
// Network layer — runs on background threads; every result carries the
// (possibly refreshed) session back to the UI, like ensureSession on the web.
// ---------------------------------------------------------------------------

enum NetResult {
    LoginOk { session: Session, devices: Vec<Value> },
    LoginErr(String),
    DiscOk { session: Session, devices: Vec<Value> },
    DiscFrequent { session: Session },
    DiscErr(String),
    ControlDone { id: String, ok: bool, backup: String, announce: bool },
    UpdateChecked { manual: bool, result: Result<ReleaseInfo, String> },
    UpdateProgress { got: u64, total: u64 },
    UpdateDownloaded { result: Result<std::path::PathBuf, String> },
    GroupDone { title: String, total: usize, failed: Vec<String> },
}

// ---------------------------------------------------------------------------
// Auto-update (workstream C) — both editions check the project's GitHub
// "latest release" at most once per 24 h (attempt-based) and compare
// versions numerically. This edition can go further than the web page:
// it downloads the Setup .exe, verifies its name, runs it, and exits so
// the per-user installer can replace the running app.
// ---------------------------------------------------------------------------

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_API: &str =
    "https://api.github.com/repos/shawnrodis/SmartLifeTuyaDesktop/releases/latest";
const RELEASES_PAGE: &str =
    "https://github.com/shawnrodis/SmartLifeTuyaDesktop/releases/latest";
const UPDATE_PERIOD_MS: i64 = 24 * 60 * 60 * 1000;

fn expected_installer_name(version: &str) -> String {
    format!("SmartLifeTuyaDesktop-Setup-v{}.exe", version)
}

fn parse_version(s: &str) -> Vec<u64> {
    s.trim()
        .trim_start_matches('v')
        .split('.')
        .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// True when `latest` is a strictly newer X.Y.Z than `current`
/// (numeric: 1.10.0 > 1.9.0).
fn version_newer(latest: &str, current: &str) -> bool {
    let a = parse_version(latest);
    let b = parse_version(current);
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

#[derive(Clone)]
struct ReleaseInfo {
    version: String,
    page_url: String,
    exe_name: String,
    exe_url: String,
    exe_size: u64,
}

/// Pull the installer asset out of a GitHub "latest release" response.
fn parse_release(v: &Value) -> Option<ReleaseInfo> {
    let tag = v.get("tag_name")?.as_str()?;
    let version = tag.trim_start_matches('v').to_string();
    let page_url = v.get("html_url")?.as_str()?.to_string();
    let assets = v.get("assets")?.as_array()?;
    let exe = assets.iter().find(|a| {
        a.get("name")
            .and_then(|n| n.as_str())
            .map(|n| n.starts_with("SmartLifeTuyaDesktop-Setup-") && n.ends_with(".exe"))
            .unwrap_or(false)
    })?;
    Some(ReleaseInfo {
        version,
        page_url,
        exe_name: exe.get("name")?.as_str()?.to_string(),
        exe_url: exe.get("browser_download_url")?.as_str()?.to_string(),
        exe_size: exe.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
    })
}

fn http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

fn parse_auth_response(json: &Value, fallback_region: &str) -> Result<Session, String> {
    if let Some(token) = json.get("access_token").and_then(|v| v.as_str()) {
        let expires_in = json.get("expires_in").and_then(|v| v.as_i64()).unwrap_or(0);
        return Ok(Session {
            access_token: token.to_string(),
            refresh_token: json.get("refresh_token").and_then(|v| v.as_str()).map(|s| s.to_string()),
            expires_at_ms: now_ms() + expires_in * 1000,
            region: region_from_token(token).unwrap_or(fallback_region).to_string(),
        });
    }
    let msg = json
        .get("msg")
        .or_else(|| json.get("errorMsg"))
        .or_else(|| json.get("errorCode"))
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| "Login failed — check username, password and region.".to_string());
    Err(msg)
}

fn net_auth(client: &reqwest::blocking::Client, creds: &Creds, plain_password: &str, prefs: &Prefs) -> Result<Session, String> {
    let region = region_key_from_code(&creds.country_code);
    let url = apply_proxy(
        &format!("{}auth.do", region_base(region)),
        &prefs.proxy_preset,
        &prefs.proxy_custom,
        region,
    );
    let resp = client
        .post(&url)
        .form(&[
            ("userName", creds.user_name.as_str()),
            ("password", plain_password),
            ("countryCode", creds.country_code.as_str()),
            ("bizType", creds.platform.as_str()),
            ("from", "tuya"),
        ])
        .send()
        .map_err(|e| format!("Network error: {}", e))?;
    let json: Value = resp.json().map_err(|e| format!("Bad response: {}", e))?;
    parse_auth_response(&json, region)
}

fn net_refresh(client: &reqwest::blocking::Client, session: &Session, prefs: &Prefs) -> Result<Session, String> {
    let refresh = session.refresh_token.clone().ok_or("No refresh token")?;
    let url = apply_proxy(
        &format!(
            "{}auth.do?grant_type=refresh_token&refresh_token={}&rand={}",
            region_base(&session.region),
            url_encode(&refresh),
            rand_f64()
        ),
        &prefs.proxy_preset,
        &prefs.proxy_custom,
        &session.region,
    );
    let resp = client.get(&url).send().map_err(|e| format!("Network error: {}", e))?;
    let json: Value = resp.json().map_err(|e| format!("Bad response: {}", e))?;
    let mut s = parse_auth_response(&json, &session.region)?;
    if s.refresh_token.is_none() {
        s.refresh_token = session.refresh_token.clone();
    }
    Ok(s)
}

fn rand_f64() -> f64 {
    // Tiny xorshift — only used as a cache-busting "rand" parameter.
    let mut x = (now_ms() as u64) ^ 0x9E3779B97F4A7C15;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x % 1_000_000) as f64 / 1_000_000.0
}

/// Refresh or silently re-login if the token is near expiry (mirrors the
/// web edition's ensureSession, but done inside the worker thread).
fn fresh_session(
    client: &reqwest::blocking::Client,
    session: &Session,
    creds: &Option<(Creds, String)>,
    prefs: &Prefs,
    force: bool,
) -> Result<Session, String> {
    if !force && now_ms() < session.expires_at_ms - 60_000 {
        return Ok(session.clone());
    }
    if session.refresh_token.is_some() {
        if let Ok(s) = net_refresh(client, session, prefs) {
            return Ok(s);
        }
    }
    if let Some((c, pw)) = creds {
        return net_auth(client, c, pw, prefs);
    }
    Err("Session expired".to_string())
}

fn skill_body(name: &str, payload: Value) -> Value {
    json!({ "header": { "name": name, "namespace": if name == "Discovery" { "discovery" } else { "control" }, "payloadVersion": 1 }, "payload": payload })
}

fn net_skill(
    client: &reqwest::blocking::Client,
    session: &Session,
    body: &Value,
    prefs: &Prefs,
) -> Result<Value, String> {
    let url = apply_proxy(
        &format!("{}skill", region_base(&session.region)),
        &prefs.proxy_preset,
        &prefs.proxy_custom,
        &session.region,
    );
    let resp = client
        .post(&url)
        .json(body)
        .send()
        .map_err(|e| format!("Network error: {}", e))?;
    resp.json().map_err(|e| format!("Bad response: {}", e))
}

fn net_discovery(
    client: &reqwest::blocking::Client,
    session: &Session,
    prefs: &Prefs,
    previous_raw: &[Value],
) -> Result<Vec<Value>, DiscoveryFail> {
    let body = skill_body("Discovery", json!({ "accessToken": session.access_token }));
    let json = net_skill(client, session, &body, prefs).map_err(DiscoveryFail::Msg)?;
    if let Some(code) = json.get("header").and_then(|h| h.get("code")).and_then(|v| v.as_str()) {
        if code == "FrequentlyInvoke" {
            return Err(DiscoveryFail::Frequent);
        }
    }
    if let Some(devs) = json
        .get("payload")
        .and_then(|p| p.get("devices"))
        .and_then(|v| v.as_array())
    {
        return Ok(merge_device_data(devs.clone(), previous_raw));
    }
    let code = json
        .get("header")
        .and_then(|h| h.get("code"))
        .and_then(|v| v.as_str())
        .unwrap_or("Unexpected response");
    Err(DiscoveryFail::Msg(format!("API error: {}", code)))
}

enum DiscoveryFail {
    Frequent,
    Msg(String),
}

fn net_control(
    client: &reqwest::blocking::Client,
    session: &Session,
    dev_id: &str,
    action: &str,
    value: &Value,
    prefs: &Prefs,
) -> bool {
    let mut payload = Map::new();
    payload.insert("accessToken".into(), Value::String(session.access_token.clone()));
    payload.insert("devId".into(), Value::String(dev_id.to_string()));
    if action == "colorSet" {
        payload.insert("color".into(), value.clone()); // colour uses the "color" key
    } else {
        payload.insert("value".into(), value.clone()); // everything else uses "value"
    }
    let body = skill_body(action, Value::Object(payload));
    match net_skill(client, session, &body, prefs) {
        Ok(json) => {
            json.get("header").and_then(|h| h.get("code")).and_then(|v| v.as_str()) == Some("SUCCESS")
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

use egui::{pos2, vec2, Ui};

#[derive(PartialEq, Clone, Copy)]
enum View {
    Login,
    Dashboard,
    Settings,
}

struct Toast {
    msg: String,
    err: bool,
    at: Instant,
}

fn status_text(d: &Device) -> String {
    if !d.online {
        return "Offline".to_string();
    }
    if d.is_scene {
        return "Scene — tap Start to run".to_string();
    }
    let mut parts = vec![if d.state { "On".to_string() } else { "Off".to_string() }];
    if d.state {
        if let Some(b) = d.brightness_pct {
            parts.push(format!("{}%", b));
        }
        if d.color_mode.as_deref() == Some("colour") {
            parts.push("Colour".to_string());
        } else if let Some(t) = d.temp_slider {
            parts.push(format!("{}K", temp_slider_to_kelvin(t)));
        }
    }
    parts.join(" · ")
}

/// Custom toggle switch — 48×27, same geometry as the web edition's switch.
fn toggle_switch(ui: &mut Ui, on: bool, enabled: bool, accent: Color32, track: Color32) -> bool {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(48.0, 27.0), sense);
    let p = ui.painter();
    p.rect_filled(rect, Rounding::same(13.5), if on { accent } else { track });
    let cx = if on { rect.right() - 13.5 } else { rect.left() + 13.5 };
    p.circle_filled(pos2(cx, rect.center().y), 10.5, Color32::WHITE);
    enabled && resp.clicked()
}

struct SliderOut {
    value: f32,
    changed: bool,
    released: bool,
}

/// Slim slider matching the web edition: 22px tall, 6px track filled with
/// the accent up to the value, 18px white thumb. Commits happen on release.
fn slim_slider(
    ui: &mut Ui,
    id: egui::Id,
    value: f32,
    min: f32,
    max: f32,
    enabled: bool,
    accent: Color32,
    track: Color32,
) -> SliderOut {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(
        vec2(width, 22.0),
        if enabled { Sense::click_and_drag() } else { Sense::hover() },
    );
    let mut out = SliderOut { value, changed: false, released: false };
    if enabled && (resp.dragged() || resp.clicked()) {
        if let Some(pos) = resp.interact_pointer_pos() {
            let lo = rect.left() + 9.0;
            let hi = rect.right() - 9.0;
            let ratio = ((pos.x - lo) / (hi - lo)).clamp(0.0, 1.0);
            out.value = min + ratio * (max - min);
            out.changed = true;
        }
    }
    let was_dragging = ui.data(|d| d.get_temp::<bool>(id.with("wasdrag")).unwrap_or(false));
    let dragging = enabled && resp.dragged();
    if was_dragging && !dragging {
        out.released = true;
    }
    ui.data_mut(|d| d.insert_temp(id.with("wasdrag"), dragging));

    let p = ui.painter();
    let mid_y = rect.center().y;
    let track_rect = egui::Rect::from_min_max(
        pos2(rect.left(), mid_y - 3.0),
        pos2(rect.right(), mid_y + 3.0),
    );
    p.rect_filled(track_rect, Rounding::same(3.0), track);
    let ratio = if max > min { (out.value - min) / (max - min) } else { 0.0 };
    let thumb_x = rect.left() + 9.0 + ratio * (rect.width() - 18.0);
    if ratio > 0.0 {
        let fill_rect = egui::Rect::from_min_max(
            pos2(rect.left(), mid_y - 3.0),
            pos2(thumb_x, mid_y + 3.0),
        );
        p.rect_filled(fill_rect, Rounding::same(3.0), accent);
    }
    p.circle_filled(pos2(thumb_x, mid_y), 9.0, Color32::WHITE);
    if !enabled {
        p.rect_filled(rect, Rounding::same(3.0), Color32::from_rgba_unmultiplied(0, 0, 0, 60));
    }
    out
}

/// Circular colour swatch (26px), same as the web edition.
fn swatch(ui: &mut Ui, hex: &str, name: &str, active: bool, text_color: Color32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    let p = ui.painter();
    p.circle_filled(rect.center(), 12.0, hex_to_color32(hex));
    if active {
        p.circle_stroke(rect.center(), 12.5, Stroke::new(2.0, text_color));
    } else {
        p.circle_stroke(
            rect.center(),
            12.0,
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(127, 127, 127, 90)),
        );
    }
    resp.on_hover_text(name).clicked()
}

struct App {
    cfg: Config,
    view: View,
    session: Option<Session>,
    devices: Vec<Device>,
    raw_devices: Vec<Value>,
    tx: Sender<NetResult>,
    rx: Receiver<NetResult>,
    login_busy: bool,
    disc_busy: bool,
    pending_login: Option<(Creds, bool)>,
    // login form
    li_user: String,
    li_pass: String,
    li_region: String,
    li_platform: String,
    li_keep: bool,
    login_error: Option<String>,
    // dashboard
    query: String,
    editing: bool,
    last_fetch: Option<Instant>,
    last_updated: String,
    open_color_for: Option<String>,
    color_pick: HashMap<String, Color32>,
    slider_live: HashMap<String, f32>,
    dragging_slider: bool,
    alias_buf: HashMap<String, String>,
    group_buf: HashMap<String, String>,
    new_group: Option<String>,
    toasts: Vec<Toast>,
    applied_style: Option<(String, String, String)>,
    // cameras
    cam_form: Option<CameraForm>,
    group_backups: HashMap<String, String>,
    // auto-update
    update_checking: bool,
    update_info: Option<ReleaseInfo>,
    update_error: Option<String>,
    update_dl: Option<(u64, u64)>,
    update_launch: Option<std::path::PathBuf>,
    // dashboard grid (workstream B): measured row heights per signature
    row_heights: HashMap<String, f32>,
    // drag & drop (workstream D): cell geometry + sections from the last
    // rendered frame, and the in-flight drag
    cell_rects: Vec<(String, Option<String>, egui::Rect)>,
    section_rects: Vec<(String, egui::Rect)>,
    sections_cache: Vec<Section>,
    drag: Option<DragState>,
}

/// State of the Add/Edit camera dialog (Rust edition of the web modal).
struct CameraForm {
    id: Option<String>,
    name: String,
    user: String,
    pass: String,
    host: String,
    port: String,
    template: String,
    path: String,
    error: Option<String>,
}

/// An in-flight card drag (Edit mode). The dragged card is hidden from
/// every section's display list; `target` is (section key, insertion
/// index into that section's list with the dragged card removed).
/// Glyph coverage (workstream E): egui's bundled fonts (Ubuntu Light +
/// a Noto Emoji subset) leave many symbols as tofu boxes — ⚙, ←/↑/↓,
/// ✕, ↺, ⠿, ★… The app therefore embeds three open-licensed outline
/// fonts as fallbacks (assets/fonts/): DejaVu Sans (arrows, geometric
/// shapes, braille grips), Noto Sans Symbols and Noto Sans Symbols 2
/// (⚙ and the newer pictographs egui's emoji subset misses). Every
/// non-ASCII glyph used by the UI was audited against the union of
/// these cmaps with fontTools; the only uncovered one — the fullwidth
/// plus U+FF0B — was swapped for a plain "+".
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "dejavu".to_string(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/DejaVuSans.ttf")),
    );
    fonts.font_data.insert(
        "noto-symbols".to_string(),
        egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSansSymbols-Regular.ttf"
        )),
    );
    fonts.font_data.insert(
        "noto-symbols2".to_string(),
        egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSansSymbols2-Regular.ttf"
        )),
    );
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let list = fonts.families.entry(fam).or_default();
        for name in ["dejavu", "noto-symbols", "noto-symbols2"] {
            list.push(name.to_string());
        }
    }
    ctx.set_fonts(fonts);
}

/// An in-flight card drag (Edit mode): the dragged card's id, the
/// section it started in, its size and the pointer's grab offset, and
/// the current insertion target.
struct DragState {
    id: String,
    is_scene: bool,
    from_section: String,
    size: egui::Vec2,
    grab_offset: egui::Vec2,
    target: Option<(String, usize)>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        let cfg = load_config();
        let (tx, rx) = channel::<NetResult>();
        let mut app = App {
            session: cfg.session.clone(),
            devices: Vec::new(),
            raw_devices: Vec::new(),
            tx,
            rx,
            login_busy: false,
            disc_busy: false,
            pending_login: None,
            li_user: String::new(),
            li_pass: String::new(),
            li_region: "1".to_string(),
            li_platform: "smart_life".to_string(),
            li_keep: false,
            login_error: None,
            query: String::new(),
            editing: false,
            last_fetch: None,
            last_updated: String::new(),
            open_color_for: None,
            color_pick: HashMap::new(),
            slider_live: HashMap::new(),
            dragging_slider: false,
            alias_buf: HashMap::new(),
            group_buf: HashMap::new(),
            new_group: None,
            toasts: Vec::new(),
            cam_form: None,
            group_backups: HashMap::new(),
            update_checking: false,
            update_info: None,
            update_error: None,
            update_dl: None,
            update_launch: None,
            row_heights: HashMap::new(),
            cell_rects: Vec::new(),
            section_rects: Vec::new(),
            sections_cache: Vec::new(),
            drag: None,
            applied_style: None,
            view: View::Login,
            cfg: Config::default(),
        };
        // Prefill the login form from the saved login (like the web edition).
        if let Some(c) = &cfg.creds {
            app.li_user = c.user_name.clone();
            app.li_pass = deobfuscate(&c.password);
            app.li_region = c.country_code.clone();
            app.li_platform = c.platform.clone();
            app.li_keep = true;
        }
        let saved_session = cfg.session.clone();
        app.cfg = cfg;
        app.apply_style(&cc.egui_ctx);
        // Silent auto-update check, throttled to once per 24 h
        // (attempt-based, like the web edition).
        if now_ms() - app.cfg.update.last_check_ms >= UPDATE_PERIOD_MS {
            app.spawn_update_check(false);
        }
        // Restore: fresh saved session → dashboard; else saved creds → silent login.
        if let Some(s) = saved_session {
            if now_ms() < s.expires_at_ms - 60_000 {
                app.session = Some(s);
                app.view = View::Dashboard;
                if let Some(Value::Array(devs)) = app.cfg.cache_devices.clone() {
                    app.set_devices(devs, false);
                }
                app.spawn_discovery(false);
                return app;
            }
        }
        if app.cfg.creds.is_some() {
            if let Some((creds, pw)) = app.creds_for_net() {
                app.spawn_login(creds, pw);
            }
        }
        app
    }

    fn pal(&self) -> Palette {
        match self.cfg.prefs.theme.as_str() {
            "light" => LIGHT,
            "black" => BLACK,
            _ => DARK,
        }
    }
    fn accent(&self) -> Color32 {
        hex_to_color32(&self.cfg.prefs.accent)
    }
    fn accent_soft(&self) -> Color32 {
        let a = self.accent();
        Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 0x29)
    }
    fn compact(&self) -> bool {
        self.cfg.prefs.density == "compact"
    }

    fn apply_style(&mut self, ctx: &egui::Context) {
        let key = (
            self.cfg.prefs.theme.clone(),
            self.cfg.prefs.accent.clone(),
            self.cfg.prefs.density.clone(),
        );
        if self.applied_style.as_ref() == Some(&key) {
            return;
        }
        self.applied_style = Some(key);
        let pal = self.pal();
        let accent = self.accent();
        let mut style = (*ctx.style()).clone();
        let mut v = egui::Visuals::dark();
        v.panel_fill = pal.bg;
        v.window_fill = pal.card;
        v.extreme_bg_color = pal.bg;
        v.faint_bg_color = pal.card;
        v.override_text_color = Some(pal.text);
        v.hyperlink_color = accent;
        v.selection.bg_fill = self.accent_soft();
        v.selection.stroke = Stroke::new(1.0, accent);
        v.window_rounding = Rounding::same(R_CARD);
        v.menu_rounding = Rounding::same(R_BTN);
        let widget = |bg: Color32, stroke_c: Color32, fg: Color32| egui::style::WidgetVisuals {
            bg_fill: bg,
            weak_bg_fill: bg,
            bg_stroke: Stroke::new(1.0, stroke_c),
            fg_stroke: Stroke::new(1.0, fg),
            rounding: Rounding::same(R_BTN),
            expansion: 0.0,
        };
        v.widgets.noninteractive = widget(pal.card, pal.border, pal.text);
        v.widgets.inactive = widget(pal.card, pal.border, pal.text);
        v.widgets.hovered = widget(pal.track, accent, pal.text);
        v.widgets.active = widget(accent, accent, Color32::WHITE);
        v.widgets.open = widget(pal.track, accent, pal.text);
        style.visuals = v;
        style.text_styles = [
            (egui::TextStyle::Heading, FontId::proportional(16.5)),
            (egui::TextStyle::Body, FontId::proportional(14.5)),
            (egui::TextStyle::Button, FontId::proportional(13.5)),
            (egui::TextStyle::Small, FontId::proportional(12.5)),
            (egui::TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.item_spacing = vec2(8.0, 8.0);
        style.spacing.button_padding = vec2(12.0, 6.0);
        ctx.set_style(style);
    }

    fn toast(&mut self, msg: impl Into<String>, err: bool) {
        self.toasts.push(Toast { msg: msg.into(), err, at: Instant::now() });
    }

    fn save(&self) {
        save_config(&self.cfg);
    }

    fn creds_for_net(&self) -> Option<(Creds, String)> {
        self.cfg
            .creds
            .as_ref()
            .map(|c| (c.clone(), deobfuscate(&c.password)))
    }

    // ---------------- network spawns ----------------

    fn spawn_login(&mut self, creds: Creds, plain_pw: String) {
        self.login_busy = true;
        let tx = self.tx.clone();
        let prefs = self.cfg.prefs.clone();
        thread::spawn(move || {
            let client = http_client();
            let msg = match net_auth(&client, &creds, &plain_pw, &prefs) {
                Ok(session) => match net_discovery(&client, &session, &prefs, &[]) {
                    Ok(devices) => NetResult::LoginOk { session, devices },
                    Err(DiscoveryFail::Frequent) => NetResult::LoginOk { session, devices: Vec::new() },
                    Err(DiscoveryFail::Msg(e)) => NetResult::LoginErr(e),
                },
                Err(e) => NetResult::LoginErr(e),
            };
            let _ = tx.send(msg);
        });
    }

    fn spawn_discovery(&mut self, force: bool) {
        let Some(session) = self.session.clone() else { return };
        if self.disc_busy {
            return;
        }
        self.disc_busy = true;
        let tx = self.tx.clone();
        let prefs = self.cfg.prefs.clone();
        let creds = self.creds_for_net();
        let previous = self.raw_devices.clone();
        thread::spawn(move || {
            let client = http_client();
            let msg = match fresh_session(&client, &session, &creds, &prefs, force) {
                Ok(s1) => match net_discovery(&client, &s1, &prefs, &previous) {
                    Ok(devices) => NetResult::DiscOk { session: s1, devices },
                    Err(DiscoveryFail::Frequent) => NetResult::DiscFrequent { session: s1 },
                    Err(DiscoveryFail::Msg(e)) => {
                        // One retry after a forced re-auth (like the web edition).
                        match fresh_session(&client, &s1, &creds, &prefs, true) {
                            Ok(s2) => match net_discovery(&client, &s2, &prefs, &previous) {
                                Ok(devices) => NetResult::DiscOk { session: s2, devices },
                                Err(DiscoveryFail::Frequent) => {
                                    NetResult::DiscFrequent { session: s2 }
                                }
                                Err(DiscoveryFail::Msg(e2)) => NetResult::DiscErr(e2),
                            },
                            Err(_) => NetResult::DiscErr(e),
                        }
                    }
                },
                Err(e) => NetResult::DiscErr(e),
            };
            let _ = tx.send(msg);
        });
    }

    fn spawn_control(&mut self, id: String, action: &'static str, value: Value, backup: String, announce: bool) {
        let Some(session) = self.session.clone() else { return };
        let tx = self.tx.clone();
        let prefs = self.cfg.prefs.clone();
        let creds = self.creds_for_net();
        thread::spawn(move || {
            let client = http_client();
            let ok = match fresh_session(&client, &session, &creds, &prefs, false) {
                Ok(s) => net_control(&client, &s, &id, action, &value, &prefs),
                Err(_) => false,
            };
            let _ = tx.send(NetResult::ControlDone { id, ok, backup, announce });
        });
    }

    // ---------------- auto-update ----------------

    fn spawn_update_check(&mut self, manual: bool) {
        if self.update_checking {
            return;
        }
        self.update_checking = true;
        if manual {
            self.update_error = None;
        }
        // Attempt-based throttle: record the attempt before the call.
        self.cfg.update.last_check_ms = now_ms();
        self.save();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result: Result<ReleaseInfo, String> = (|| {
                let resp = http_client()
                    .get(RELEASES_API)
                    .header(
                        "User-Agent",
                        concat!("SmartLifeTuyaDesktop/", env!("CARGO_PKG_VERSION")),
                    )
                    .header("Accept", "application/vnd.github+json")
                    .send()
                    .map_err(|e| e.to_string())?;
                let v: Value = resp.json().map_err(|e| e.to_string())?;
                parse_release(&v).ok_or_else(|| "Unexpected response from GitHub".to_string())
            })();
            let _ = tx.send(NetResult::UpdateChecked { manual, result });
        });
    }

    fn spawn_update_download(&mut self) {
        let Some(info) = self.update_info.clone() else { return };
        if self.update_dl.is_some() {
            return;
        }
        // Verify BEFORE downloading that the asset is exactly the
        // installer this release is supposed to ship.
        if info.exe_name != expected_installer_name(&info.version) {
            self.update_error = Some(format!(
                "Unexpected installer name “{}” — refusing to download it. Use the releases page instead.",
                info.exe_name
            ));
            return;
        }
        self.update_error = None;
        self.update_dl = Some((0, info.exe_size));
        let tx = self.tx.clone();
        let url = info.exe_url.clone();
        let name = info.exe_name.clone();
        std::thread::spawn(move || {
            let result: Result<std::path::PathBuf, String> = (|| {
                let mut resp = http_client()
                    .get(&url)
                    .header(
                        "User-Agent",
                        concat!("SmartLifeTuyaDesktop/", env!("CARGO_PKG_VERSION")),
                    )
                    .send()
                    .map_err(|e| e.to_string())?;
                let total = resp.content_length().unwrap_or(0);
                let path = std::env::temp_dir().join(&name);
                let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
                let mut got: u64 = 0;
                let mut buf = [0u8; 65536];
                loop {
                    let n = std::io::Read::read(&mut resp, &mut buf).map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    std::io::Write::write_all(&mut file, &buf[..n]).map_err(|e| e.to_string())?;
                    got += n as u64;
                    let _ = tx.send(NetResult::UpdateProgress { got, total });
                }
                Ok(path)
            })();
            let _ = tx.send(NetResult::UpdateDownloaded { result });
        });
    }

    // ---------------- device state ----------------

    fn set_devices(&mut self, raw: Vec<Value>, persist: bool) {
        self.raw_devices = raw.clone();
        self.devices = raw.iter().map(parse_device).collect();
        self.prune_layout();
        self.cfg.cache_devices = Some(Value::Array(raw));
        self.last_fetch = Some(Instant::now());
        self.last_updated = format!(
            "Updated {}",
            chrono::Local::now().format("%H:%M:%S")
        );
        if persist {
            self.save();
        }
    }

    fn prune_layout(&mut self) {
        let mut ids: HashSet<&str> = self.devices.iter().map(|d| d.id.as_str()).collect();
        let cam_ids: Vec<String> = self
            .cfg
            .cameras
            .iter()
            .map(|c| format!("cam:{}", c.id))
            .collect();
        ids.extend(cam_ids.iter().map(|s| s.as_str()));
        let lay = &mut self.cfg.layout;
        lay.pins.retain(|id| ids.contains(id.as_str()));
        lay.hidden.retain(|id| ids.contains(id.as_str()));
        lay.assign.retain(|id, _| ids.contains(id.as_str()));
        lay.alias.retain(|id, _| ids.contains(id.as_str()));
        for list in lay.section_order.values_mut() {
            list.retain(|id| ids.contains(id.as_str()));
        }
        lay.section_order.retain(|_, list| !list.is_empty());
    }

    fn device_index(&self, id: &str) -> Option<usize> {
        self.devices.iter().position(|d| d.id == id)
    }

    fn camera_index(&self, id: &str) -> Option<usize> {
        self.cfg.cameras.iter().position(|c| format!("cam:{}", c.id) == id)
    }

    /// Devices + cameras as one peer list for the section model.
    fn all_entries(&self) -> Vec<CardEntry> {
        let mut v: Vec<CardEntry> = self.devices.iter().map(device_entry).collect();
        v.extend(self.cfg.cameras.iter().map(camera_entry));
        v
    }

    fn open_camera_form(&mut self, edit_id: Option<&str>) {
        if let Some(eid) = edit_id {
            if let Some(ci) = self.camera_index(eid) {
                let c = &self.cfg.cameras[ci];
                self.cam_form = Some(CameraForm {
                    id: Some(eid.to_string()),
                    name: c.name.clone(),
                    user: c.user.clone(),
                    pass: deobfuscate(&c.password),
                    host: c.host.clone(),
                    port: c.port.to_string(),
                    template: c.template.clone(),
                    path: c.path.clone(),
                    error: None,
                });
                return;
            }
        }
        self.cam_form = Some(CameraForm {
            id: None,
            name: String::new(),
            user: String::new(),
            pass: String::new(),
            host: String::new(),
            port: "554".to_string(),
            template: "tapo1".to_string(),
            path: String::new(),
            error: None,
        });
    }

    fn delete_camera(&mut self, item_id: &str) {
        if let Some(ci) = self.camera_index(item_id) {
            self.cfg.cameras.remove(ci);
        }
        let lay = &mut self.cfg.layout;
        lay.pins.retain(|x| x != item_id);
        lay.hidden.retain(|x| x != item_id);
        lay.assign.remove(item_id);
        lay.alias.remove(item_id);
        for list in lay.section_order.values_mut() {
            list.retain(|x| x != item_id);
        }
        lay.section_order.retain(|_, list| !list.is_empty());
        self.save();
        self.toast("Camera deleted", false);
    }

    /// Watch a camera: gateway watch page in the default browser when a
    /// stream gateway is configured, else the external player command.
    fn watch_camera(&mut self, item_id: &str) {
        let Some(ci) = self.camera_index(item_id) else { return };
        let cam = self.cfg.cameras[ci].clone();
        if let Some(url) = gateway_watch_url(&cam, &self.cfg.prefs) {
            open_external(&url);
            self.toast("Stream opened in your browser", false);
            return;
        }
        let url = build_rtsp_url(&cam);
        match launch_player(&self.cfg.prefs.player_cmd, &url) {
            Ok(prog) => self.toast(&format!("Opening stream in {}…", prog), false),
            Err(e) => self.toast(
                &format!(
                    "Could not start the player ({}). Check the player command in Settings, or copy the stream URL into VLC.",
                    e
                ),
                true,
            ),
        }
    }

    // ---------------- drag & drop (Edit mode) ----------------

    /// Section display list while dragging: the dragged card is removed
    /// everywhere and a gap (None) sits at the current insertion point
    /// of the target section, so the other cards reflow around it.
    fn display_ids(&self, sec: &Section) -> Vec<Option<String>> {
        let mut ids: Vec<Option<String>> = sec.ids.iter().map(|i| Some(i.clone())).collect();
        if let Some(drag) = &self.drag {
            ids.retain(|i| i.as_deref() != Some(drag.id.as_str()));
            if let Some((tkey, tidx)) = &drag.target {
                if *tkey == sec.key {
                    let at = (*tidx).min(ids.len());
                    ids.insert(at, None);
                }
            }
        }
        ids
    }

    /// Insertion target for a pointer position, from last frame's
    /// geometry. Compatible sections only: scenes never mix with
    /// devices/cameras.
    fn drag_target_at(
        &self,
        pos: egui::Pos2,
        drag_id: &str,
        is_scene: bool,
    ) -> Option<(String, usize)> {
        let mut chosen: Option<String> = None;
        let mut chosen_dy = f32::INFINITY;
        for (key, rect) in &self.section_rects {
            let Some(sec) = self.sections_cache.iter().find(|s| &s.key == key) else {
                continue;
            };
            if (sec.kind == SectionKind::Scenes) != is_scene {
                continue;
            }
            let expanded = rect.expand(6.0);
            let dy = if expanded.contains(pos) {
                0.0
            } else if pos.y < expanded.top() {
                expanded.top() - pos.y
            } else if pos.y > expanded.bottom() {
                pos.y - expanded.bottom()
            } else {
                0.0
            };
            let inside_x = pos.x >= expanded.left() && pos.x <= expanded.right();
            let score = if expanded.contains(pos) {
                -1.0
            } else if inside_x && dy < 48.0 {
                dy
            } else {
                f32::INFINITY
            };
            if score < chosen_dy {
                chosen_dy = score;
                chosen = Some(key.clone());
            }
        }
        let key = chosen?;
        // Count item cells (excluding the dragged card and the gap)
        // whose centre is "before" the pointer in reading order.
        let mut idx = 0usize;
        for (skey, cid, rect) in &self.cell_rects {
            if *skey != key {
                continue;
            }
            let Some(cid) = cid else { continue };
            if cid == drag_id {
                continue;
            }
            let c = rect.center();
            let before = if pos.y > c.y + rect.height() / 2.0 {
                true
            } else if pos.y < c.y - rect.height() / 2.0 {
                false
            } else {
                pos.x > c.x
            };
            if before {
                idx += 1;
            } else {
                break;
            }
        }
        Some((key, idx))
    }

    fn update_drag(&mut self, ctx: &egui::Context) {
        let (pos, released, escaped) = ctx.input(|i| {
            (
                i.pointer.hover_pos().or_else(|| i.pointer.interact_pos()),
                i.pointer.any_released(),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if escaped {
            self.drag = None;
            return;
        }
        if let (Some(d), Some(p)) = (self.drag.as_ref(), pos) {
            let (id, is_scene) = (d.id.clone(), d.is_scene);
            let t = self.drag_target_at(p, &id, is_scene);
            if let Some(d) = self.drag.as_mut() {
                d.target = t;
            }
        }
        if released {
            self.commit_drag();
        }
    }

    fn commit_drag(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        let Some((tkey, tidx)) = drag.target.clone() else { return };
        let Some(tsec) = self.sections_cache.iter().find(|s| s.key == tkey).cloned() else {
            return;
        };
        if (tsec.kind == SectionKind::Scenes) != drag.is_scene {
            return;
        }
        if !drag.is_scene {
            match &tsec.kind {
                SectionKind::Pinned => {
                    if !self.cfg.layout.pins.contains(&drag.id) {
                        self.cfg.layout.pins.push(drag.id.clone());
                    }
                }
                SectionKind::Group => {
                    if let Some(gid) = &tsec.group_id {
                        self.cfg.layout.assign.insert(drag.id.clone(), gid.clone());
                    }
                }
                SectionKind::Ungrouped | SectionKind::Lighting | SectionKind::Cameras => {
                    self.cfg.layout.assign.remove(&drag.id);
                }
                SectionKind::Scenes => {}
            }
        }
        let mut list: Vec<String> = tsec
            .ids
            .iter()
            .filter(|i| **i != drag.id)
            .cloned()
            .collect();
        let at = tidx.min(list.len());
        list.insert(at, drag.id.clone());
        self.cfg.layout.section_order.insert(tsec.order_key.clone(), list);
        self.save();
    }

    fn display(&self, d: &Device) -> String {
        display_name(&self.cfg.layout, d)
    }

    fn backup_data(&self, idx: usize) -> String {
        self.devices[idx]
            .raw
            .get("data")
            .cloned()
            .unwrap_or_else(|| json!({}))
            .to_string()
    }

    fn restore_backup(&mut self, id: &str, backup: &str) {
        if let Some(idx) = self.device_index(id) {
            if let Ok(data) = serde_json::from_str::<Value>(backup) {
                let mut raw = self.devices[idx].raw.clone();
                if let Some(obj) = raw.as_object_mut() {
                    obj.insert("data".into(), data);
                }
                self.devices[idx] = parse_device(&raw);
                if let Some(ridx) = self.raw_devices.iter().position(|r| r.get("id").and_then(|v| v.as_str()) == Some(id)) {
                    self.raw_devices[ridx] = raw;
                }
            }
        }
    }

    // ---------------- control actions (optimistic, like the web) ----------------

    fn do_toggle(&mut self, id: &str) {
        let Some(idx) = self.device_index(id) else { return };
        let backup = self.backup_data(idx);
        let new_state = !self.devices[idx].state;
        self.devices[idx].state = new_state;
        if let Some(data) = self.devices[idx].raw.get_mut("data").and_then(|v| v.as_object_mut()) {
            data.insert("state".into(), json!(new_state));
        }
        self.spawn_control(id.to_string(), "turnOnOff", json!(if new_state { 1 } else { 0 }), backup, false);
    }

    fn do_scene_start(&mut self, id: &str) {
        let Some(idx) = self.device_index(id) else { return };
        let backup = self.backup_data(idx);
        self.spawn_control(id.to_string(), "turnOnOff", json!(1), backup, true);
    }

    fn do_brightness(&mut self, id: &str, pct: i64) {
        let pct = pct.clamp(11, 100);
        let Some(idx) = self.device_index(id) else { return };
        let backup = self.backup_data(idx);
        self.devices[idx].brightness_pct = Some(pct);
        self.devices[idx].state = true;
        if let Some(data) = self.devices[idx].raw.get_mut("data").and_then(|v| v.as_object_mut()) {
            data.insert("brightness".into(), json!(pct * 10));
            data.insert("state".into(), json!(true));
        }
        self.spawn_control(id.to_string(), "brightnessSet", json!(pct), backup, false);
    }

    fn do_temp(&mut self, id: &str, slider: i64) {
        let slider = slider.clamp(1000, 10000);
        let Some(idx) = self.device_index(id) else { return };
        let backup = self.backup_data(idx);
        self.devices[idx].temp_slider = Some(slider);
        self.devices[idx].state = true;
        self.devices[idx].color_mode = Some("white".to_string());
        if let Some(data) = self.devices[idx].raw.get_mut("data").and_then(|v| v.as_object_mut()) {
            data.insert("color_temp".into(), json!(temp_slider_to_reported(slider)));
            data.insert("state".into(), json!(true));
            data.insert("color_mode".into(), json!("white"));
        }
        self.spawn_control(id.to_string(), "colorTemperatureSet", json!(slider), backup, false);
    }

    fn do_color(&mut self, id: &str, hex: &str) {
        let Some(idx) = self.device_index(id) else { return };
        let backup = self.backup_data(idx);
        let (mut hue, mut sat, _) = hex_to_hsv(hex);
        if sat < 0.02 {
            hue = 0.0;
            sat = 0.0; // white, like the reference
        }
        let sat = (sat * 100.0).round() / 100.0;
        let value = json!({
            "hue": hue as i64,
            "saturation": sat,
            "brightness": self.devices[idx].brightness_pct.unwrap_or(100)
        });
        self.devices[idx].hue = Some(hue);
        self.devices[idx].saturation = Some(sat);
        self.devices[idx].state = true;
        self.devices[idx].color_mode = Some("colour".to_string());
        if let Some(data) = self.devices[idx].raw.get_mut("data").and_then(|v| v.as_object_mut()) {
            data.insert("hue".into(), json!(hue));
            data.insert("saturation".into(), json!(sat));
            data.insert("state".into(), json!(true));
            data.insert("color_mode".into(), json!("colour"));
        }
        self.open_color_for = None;
        self.spawn_control(id.to_string(), "colorSet", value, backup, false);
    }

    /// Optimistic local state for one fanned-out group command — the
    /// same mutations the per-card do_* functions apply.
    fn apply_group_target(&mut self, idx: usize, action: &str, value: &Value) {
        let dev = &mut self.devices[idx];
        match action {
            "turnOnOff" => {
                let on = value.as_i64().unwrap_or(0) != 0;
                dev.state = on;
                if let Some(data) = dev.raw.get_mut("data").and_then(|v| v.as_object_mut()) {
                    data.insert("state".into(), json!(on));
                }
            }
            "brightnessSet" => {
                let pct = value.as_i64().unwrap_or(100);
                dev.brightness_pct = Some(pct);
                dev.state = true;
                if let Some(data) = dev.raw.get_mut("data").and_then(|v| v.as_object_mut()) {
                    data.insert("brightness".into(), json!(pct * 10));
                    data.insert("state".into(), json!(true));
                }
            }
            "colorTemperatureSet" => {
                let slider = value.as_i64().unwrap_or(4000);
                dev.temp_slider = Some(slider);
                dev.state = true;
                dev.color_mode = Some("white".to_string());
                if let Some(data) = dev.raw.get_mut("data").and_then(|v| v.as_object_mut()) {
                    data.insert("color_temp".into(), json!(temp_slider_to_reported(slider)));
                    data.insert("state".into(), json!(true));
                    data.insert("color_mode".into(), json!("white"));
                }
            }
            "colorSet" => {
                let hue = value.get("hue").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let sat = value.get("saturation").and_then(|v| v.as_f64()).unwrap_or(0.0);
                dev.hue = Some(hue);
                dev.saturation = Some(sat);
                dev.state = true;
                dev.color_mode = Some("colour".to_string());
                if let Some(data) = dev.raw.get_mut("data").and_then(|v| v.as_object_mut()) {
                    data.insert("hue".into(), json!(hue));
                    data.insert("saturation".into(), json!(sat));
                    data.insert("state".into(), json!(true));
                    data.insert("color_mode".into(), json!("colour"));
                }
            }
            _ => {}
        }
    }

    /// Broadcast one group action to a section's members: optimistic
    /// local updates now, then ONE thread sends the commands
    /// sequentially (~200 ms apart — Tuya rate-limits bursts) and
    /// reports back a GroupDone summary.
    fn group_broadcast(&mut self, sec: &Section, kind: GroupKind, num: i64, hex: Option<String>) {
        let full = build_sections(&self.all_entries(), &self.cfg.layout, self.editing, "");
        let Some(fsec) = full.iter().find(|s| s.key == sec.key) else { return };
        let members: Vec<Device> = fsec
            .ids
            .iter()
            .filter_map(|id| self.device_index(id).map(|i| self.devices[i].clone()))
            .collect();
        let plan = group_plan(&members, kind, num, hex.as_deref());
        if plan.is_empty() {
            return;
        }
        self.group_backups.clear();
        for (id, action, value) in &plan {
            if let Some(idx) = self.device_index(id) {
                self.group_backups.insert(id.clone(), self.backup_data(idx));
                self.apply_group_target(idx, action, value);
            }
        }
        let Some(session) = self.session.clone() else { return };
        let tx = self.tx.clone();
        let prefs = self.cfg.prefs.clone();
        let creds = self.creds_for_net();
        let title = sec.title.clone();
        let total = plan.len();
        thread::spawn(move || {
            let client = http_client();
            let mut failed: Vec<String> = Vec::new();
            match fresh_session(&client, &session, &creds, &prefs, false) {
                Ok(s) => {
                    for (i, (id, action, value)) in plan.iter().enumerate() {
                        if !net_control(&client, &s, id, action, value, &prefs) {
                            failed.push(id.clone());
                        }
                        if i + 1 < plan.len() {
                            thread::sleep(Duration::from_millis(200));
                        }
                    }
                }
                Err(_) => {
                    failed = plan.iter().map(|(id, _, _)| id.clone()).collect();
                }
            }
            let _ = tx.send(NetResult::GroupDone { title, total, failed });
        });
    }

    fn toggle_pin(&mut self, id: &str) {
        if let Some(idx) = self.device_index(id) {
            if self.devices[idx].is_scene {
                return; // scenes can't be pinned
            }
        } else {
            return;
        }
        let lay = &mut self.cfg.layout;
        if let Some(pos) = lay.pins.iter().position(|p| p == id) {
            lay.pins.remove(pos);
        } else {
            lay.pins.push(id.to_string());
        }
        self.save();
    }

    // ---------------- net result handling ----------------

    fn poll_net(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                NetResult::LoginOk { session, devices } => {
                    self.login_busy = false;
                    self.login_error = None;
                    if let Some((creds, keep)) = self.pending_login.take() {
                        self.cfg.creds = if keep { Some(creds) } else { None };
                    }
                    self.session = Some(session.clone());
                    self.cfg.session = Some(session);
                    self.view = View::Dashboard;
                    if devices.is_empty() {
                        self.toast("Tuya is rate-limiting the device list — press Refresh in a few seconds", true);
                        self.save();
                    } else {
                        self.set_devices(devices, true);
                    }
                }
                NetResult::LoginErr(e) => {
                    self.login_busy = false;
                    self.pending_login = None;
                    if self.view == View::Login {
                        self.login_error = Some(e);
                    } else {
                        self.toast(e, true);
                    }
                }
                NetResult::DiscOk { session, devices } => {
                    self.disc_busy = false;
                    self.session = Some(session.clone());
                    self.cfg.session = Some(session);
                    self.set_devices(devices, true);
                }
                NetResult::DiscFrequent { session } => {
                    self.disc_busy = false;
                    self.session = Some(session.clone());
                    self.cfg.session = Some(session);
                    self.save();
                    if self.devices.is_empty() {
                        self.toast("Tuya is rate-limiting the device list — wait a few seconds and press Refresh", true);
                    } else {
                        self.toast("Tuya is rate-limiting — showing last known states", false);
                        self.last_fetch = Some(Instant::now());
                    }
                }
                NetResult::DiscErr(e) => {
                    self.disc_busy = false;
                    self.toast(e, true);
                }
                NetResult::ControlDone { id, ok, backup, announce } => {
                    if ok && announce {
                        let name = self
                            .device_index(&id)
                            .map(|i| self.display(&self.devices[i].clone()))
                            .unwrap_or_else(|| id.clone());
                        self.toast(format!("▶ Scene started: {}", name), false);
                    }
                    if !ok {
                        let name = self
                            .device_index(&id)
                            .map(|i| self.display(&self.devices[i].clone()))
                            .unwrap_or_else(|| id.clone());
                        self.restore_backup(&id, &backup);
                        self.toast(format!("Command failed for {}", name), true);
                    }
                }
                NetResult::UpdateChecked { manual, result } => {
                    self.update_checking = false;
                    match result {
                        Ok(info) => {
                            let newer = version_newer(&info.version, APP_VERSION);
                            self.update_info = Some(info);
                            if manual && !newer {
                                self.toast("You're on the latest version", false);
                            }
                        }
                        Err(e) => {
                            // Auto-checks fail silently; manual checks say so.
                            if manual {
                                self.update_error = Some(format!("Update check failed: {}", e));
                            }
                        }
                    }
                }
                NetResult::GroupDone { title, total, failed } => {
                    let mut names: Vec<String> = Vec::new();
                    for id in &failed {
                        if let Some(b) = self.group_backups.remove(id) {
                            self.restore_backup(id, &b);
                        }
                        names.push(
                            self.device_index(id)
                                .map(|i| self.display(&self.devices[i].clone()))
                                .unwrap_or_else(|| id.clone()),
                        );
                    }
                    self.group_backups.clear();
                    if !names.is_empty() {
                        self.toast(
                            format!(
                                "{}: {} of {} updated — {} didn't respond",
                                title,
                                total - failed.len(),
                                total,
                                names.join(", ")
                            ),
                            true,
                        );
                    }
                    self.spawn_discovery(false);
                }
                NetResult::UpdateProgress { got, total } => {
                    self.update_dl = Some((got, total));
                }
                NetResult::UpdateDownloaded { result } => {
                    self.update_dl = None;
                    match result {
                        Ok(path) => {
                            // Final verification before anything runs it:
                            // the file name must be exactly this release's
                            // expected installer name.
                            let expected = self
                                .update_info
                                .as_ref()
                                .map(|i| expected_installer_name(&i.version))
                                .unwrap_or_default();
                            let actual = path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string();
                            if !expected.is_empty() && actual == expected {
                                self.update_launch = Some(path);
                            } else {
                                self.update_error = Some(
                                    "The downloaded file's name doesn't match the expected installer, so it was not run. Use the releases page instead."
                                        .to_string(),
                                );
                            }
                        }
                        Err(e) => {
                            self.update_error = Some(format!("Download failed: {}", e));
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

const REGIONS: [(&str, &str); 3] = [
    ("1", "United States (US)"),
    ("44", "Europe (EU)"),
    ("86", "China (CN)"),
];
const PLATFORMS: [(&str, &str); 2] = [("smart_life", "Smart Life"), ("tuya", "Tuya Smart")];
const INTERVALS: [(u64, &str); 4] = [(15, "15s"), (30, "30s"), (60, "60s"), (120, "2 min")];

fn interval_label(v: u64) -> &'static str {
    INTERVALS
        .iter()
        .find(|(x, _)| *x == v)
        .map(|(_, l)| *l)
        .unwrap_or("30s")
}

fn label_of<'a>(table: &[(&'a str, &'a str)], value: &str) -> &'a str {
    table
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, l)| *l)
        .unwrap_or("")
}

impl App {
    fn field_label(ui: &mut Ui, pal: &Palette, text: &str) {
        ui.label(RichText::new(text).size(12.5).color(pal.dim).strong());
        ui.add_space(2.0);
    }

    fn proxy_fields(&mut self, ui: &mut Ui, pal: &Palette, id_prefix: &str) {
        Self::field_label(ui, pal, "CORS proxy");
        let cur = self.cfg.prefs.proxy_preset.clone();
        let mut chosen: Option<String> = None;
        ComboBox::from_id_source(format!("{}_proxy", id_prefix))
            .selected_text(label_of(&PROXY_PRESETS, &cur))
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for (id, label) in PROXY_PRESETS {
                    if ui.selectable_label(cur == id, label).clicked() {
                        chosen = Some(id.to_string());
                    }
                }
            });
        if let Some(c) = chosen {
            self.cfg.prefs.proxy_preset = c;
            self.save();
        }
        if self.cfg.prefs.proxy_preset == "custom" {
            ui.add_space(6.0);
            Self::field_label(ui, pal, "Custom proxy prefix");
            let mut custom = self.cfg.prefs.proxy_custom.clone();
            if ui
                .add(TextEdit::singleline(&mut custom).desired_width(f32::INFINITY).hint_text("https://my-proxy.example/ or …?url={url}"))
                .changed()
            {
                self.cfg.prefs.proxy_custom = custom;
                self.save();
            }
        }
    }

    fn view_login(&mut self, ui: &mut Ui) {
        let pal = self.pal();
        let accent = self.accent();
        ui.vertical_centered(|ui| {
            ui.add_space(36.0);
            Frame::none()
                .fill(pal.card)
                .stroke(Stroke::new(1.0, pal.border))
                .rounding(Rounding::same(R_LOGIN))
                .inner_margin(Margin { left: 28.0, right: 28.0, top: 30.0, bottom: 24.0 })
                .show(ui, |ui| {
                    ui.set_width(344.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("🏠").size(40.0));
                        ui.label(RichText::new("SmartLife & Tuya Desktop").size(23.0).strong());
                        ui.label(
                            RichText::new("Your Smart Life / Tuya devices, in one clean dashboard")
                                .size(13.5)
                                .color(pal.dim),
                        );
                    });
                    ui.add_space(18.0);
                    if let Some(err) = self.login_error.clone() {
                        Frame::none()
                            .fill(Color32::from_rgba_unmultiplied(0xf4, 0x3f, 0x5e, 30))
                            .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(0xf4, 0x3f, 0x5e, 102)))
                            .rounding(Rounding::same(11.0))
                            .inner_margin(Margin::symmetric(12.0, 9.0))
                            .show(ui, |ui| {
                                ui.set_width(320.0);
                                ui.label(RichText::new(err).size(13.0).color(if self.cfg.prefs.theme == "light" {
                                    Color32::from_rgb(0xbe, 0x12, 0x3c)
                                } else {
                                    Color32::from_rgb(0xfb, 0x71, 0x85)
                                }));
                            });
                        ui.add_space(12.0);
                    }
                    Self::field_label(ui, &pal, "Smart Life / Tuya username");
                    ui.add(
                        TextEdit::singleline(&mut self.li_user)
                            .desired_width(f32::INFINITY)
                            .hint_text("email or phone"),
                    );
                    ui.add_space(10.0);
                    Self::field_label(ui, &pal, "Password");
                    ui.add(
                        TextEdit::singleline(&mut self.li_pass)
                            .password(true)
                            .desired_width(f32::INFINITY)
                            .hint_text("Your Smart Life password"),
                    );
                    ui.add_space(10.0);
                    Self::field_label(ui, &pal, "Region (where your account was registered)");
                    let mut region = self.li_region.clone();
                    ComboBox::from_id_source("li_region")
                        .selected_text(label_of(&REGIONS, &region))
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for (v, l) in REGIONS {
                                ui.selectable_value(&mut region, v.to_string(), l);
                            }
                        });
                    self.li_region = region;
                    ui.add_space(10.0);
                    Self::field_label(ui, &pal, "App platform");
                    let mut platform = self.li_platform.clone();
                    ComboBox::from_id_source("li_platform")
                        .selected_text(label_of(&PLATFORMS, &platform))
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for (v, l) in PLATFORMS {
                                ui.selectable_value(&mut platform, v.to_string(), l);
                            }
                        });
                    self.li_platform = platform;
                    ui.add_space(8.0);
                    ui.checkbox(&mut self.li_keep, "Keep me logged in on this device");
                    ui.add_space(12.0);
                    let btn = Button::new(RichText::new("Log in").size(15.0).color(Color32::WHITE)).fill(accent);
                    let clicked = ui
                        .add_enabled(!self.login_busy, btn)
                        .on_hover_text("Sign in to Tuya")
                        .clicked();
                    if self.login_busy {
                        ui.add_space(8.0);
                        ui.vertical_centered(|ui| ui.spinner());
                    }
                    if clicked {
                        self.submit_login();
                    }
                    ui.add_space(14.0);
                    ui.separator();
                    egui::CollapsingHeader::new("Connection / proxy settings").show(ui, |ui| {
                        ui.add_space(6.0);
                        self.proxy_fields(ui, &pal, "li");
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("Most regions connect directly. A proxy is only needed where Tuya's servers don't accept direct browser/app calls (China region, some networks). See Help in Settings.")
                                .size(12.0)
                                .color(pal.dim),
                        );
                    });
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Your credentials are only sent to Tuya's own servers and are stored only in this app's config on this device (and only if you tick “Keep me logged in”). Nothing is sent anywhere else.")
                            .size(12.0)
                            .color(pal.dim),
                    );
                });
        });
    }

    fn submit_login(&mut self) {
        let creds = Creds {
            user_name: self.li_user.trim().to_string(),
            password: obfuscate(&self.li_pass),
            country_code: self.li_region.clone(),
            platform: self.li_platform.clone(),
        };
        self.login_error = None;
        self.pending_login = Some((creds.clone(), self.li_keep));
        self.spawn_login(creds, self.li_pass.clone());
    }

    fn do_logout(&mut self) {
        self.session = None;
        self.devices.clear();
        self.raw_devices.clear();
        self.cfg.creds = None;
        self.cfg.session = None;
        self.editing = false;
        self.save();
        self.view = View::Login;
        self.toast("Logged out", false);
    }

    fn header(&mut self, ctx: &egui::Context) {
        let pal = self.pal();
        let accent = self.accent();
        egui::TopBottomPanel::top("app-header")
            .frame(
                Frame::none()
                    .fill(pal.card)
                    .inner_margin(Margin::symmetric(18.0, 12.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("🏠").size(22.0));
                    ui.label(RichText::new("SmartLife & Tuya Desktop").size(18.0).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(Button::new("Log out")).clicked() {
                            self.do_logout();
                            return;
                        }
                        if ui.add(Button::new("⚙ Settings")).clicked() {
                            self.view = View::Settings;
                        }
                        let edit_label = if self.editing { "✔ Done editing" } else { "✎ Edit" };
                        if ui.add(Button::new(edit_label)).clicked() {
                            self.set_editing(!self.editing);
                        }
                        if ui.add(Button::new("↻ Refresh")).clicked() {
                            self.spawn_discovery(false);
                        }
                        let mut interval = self.cfg.prefs.interval;
                        ComboBox::from_id_source("hdr_interval")
                            .selected_text(interval_label(interval))
                            .width(74.0)
                            .show_ui(ui, |ui| {
                                for (v, l) in INTERVALS {
                                    ui.selectable_value(&mut interval, v, l);
                                }
                            });
                        if interval != self.cfg.prefs.interval {
                            self.cfg.prefs.interval = interval;
                            self.save();
                        }
                        let mut auto = self.cfg.prefs.autorefresh;
                        if ui.checkbox(&mut auto, "Autorefresh").changed() {
                            self.cfg.prefs.autorefresh = auto;
                            self.save();
                        }
                        ui.add(
                            TextEdit::singleline(&mut self.query)
                                .desired_width(180.0)
                                .hint_text("🔍 Search devices…"),
                        );
                        if !self.last_updated.is_empty() {
                            ui.label(RichText::new(self.last_updated.clone()).size(12.0).color(pal.dim));
                        }
                        let _ = accent;
                    });
                });
            });
        if self.editing {
            egui::TopBottomPanel::top("edit-bar")
                .frame(
                    Frame::none()
                        .fill(self.accent_soft())
                        .inner_margin(Margin::symmetric(18.0, 9.0)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("✎ Edit mode — drag cards to reorder within a section, rename devices, assign groups, hide devices. Use 📌 on any card to pin a shortcut to the top.")
                                .size(13.0)
                                .color(pal.dim),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add(Button::new("Done").fill(self.accent()).stroke(Stroke::new(1.0, self.accent()))).clicked() {
                                self.set_editing(false);
                            }
                            if ui.add(Button::new("+ Add camera")).clicked() {
                                self.open_camera_form(None);
                            }
                            if ui.add(Button::new("+ New group")).clicked() {
                                self.new_group = Some(String::new());
                            }
                        });
                    });
                });
        }
    }

    fn set_editing(&mut self, on: bool) {
        self.editing = on;
        if on {
            self.alias_buf = self
                .devices
                .iter()
                .map(|d| (d.id.clone(), self.cfg.layout.alias.get(&d.id).cloned().unwrap_or_default()))
                .collect();
            self.group_buf = self
                .cfg
                .layout
                .groups
                .iter()
                .map(|g| (g.id.clone(), g.name.clone()))
                .collect();
        }
    }

    fn view_dashboard(&mut self, ui: &mut Ui) {
        let pal = self.pal();
        let entries = self.all_entries();
        let sections = build_sections(&entries, &self.cfg.layout, self.editing, &self.query);
        self.sections_cache = sections.clone();
        // Geometry maps are rebuilt by section_view below; update_drag
        // has already consumed last frame's rects in this update pass.
        self.cell_rects.clear();
        self.section_rects.clear();
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(6.0);
                // ---- update banner (dismissible per version) ----
                if let Some(info) = self.update_info.clone() {
                    if version_newer(&info.version, APP_VERSION)
                        && self.cfg.update.dismissed != info.version
                    {
                        let mut act: Option<&str> = None;
                        Frame::none()
                            .fill(self.accent_soft())
                            .stroke(Stroke::new(1.0, self.accent()))
                            .rounding(Rounding::same(14.0))
                            .inner_margin(Margin::symmetric(16.0, 12.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    if let Some((got, total)) = self.update_dl {
                                        let pct = if total > 0 { got * 100 / total } else { 0 };
                                        ui.label(
                                            RichText::new(format!(
                                                "Downloading version {}… {}%",
                                                info.version, pct
                                            ))
                                            .size(13.5)
                                            .strong(),
                                        );
                                    } else {
                                        ui.label(
                                            RichText::new(format!(
                                                "Version {} is available — you're on {}.",
                                                info.version, APP_VERSION
                                            ))
                                            .size(13.5)
                                            .strong(),
                                        );
                                    }
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.add(Button::new("Dismiss")).clicked() {
                                            act = Some("dismiss");
                                        }
                                        if self.update_dl.is_none()
                                            && ui
                                                .add(
                                                    Button::new(
                                                        RichText::new("Download & install")
                                                            .color(Color32::WHITE),
                                                    )
                                                    .fill(self.accent()),
                                                )
                                                .clicked()
                                        {
                                            act = Some("download");
                                        }
                                    });
                                });
                                if let Some(err) = &self.update_error {
                                    ui.add_space(4.0);
                                    ui.label(RichText::new(err.clone()).size(12.5).color(DANGER));
                                    if ui.add(Button::new("Open releases page")).clicked() {
                                        act = Some("releases");
                                    }
                                }
                            });
                        match act {
                            Some("dismiss") => {
                                self.cfg.update.dismissed = info.version.clone();
                                self.save();
                            }
                            Some("download") => self.spawn_update_download(),
                            Some("releases") => {
                                let _ = open_external(&info.page_url);
                            }
                            _ => {}
                        }
                        ui.add_space(12.0);
                    }
                }
                if self.devices.is_empty() && self.cfg.cameras.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("No devices to show. Press ↻ Refresh, or unhide devices in Settings.")
                                .size(14.0)
                                .color(pal.dim),
                        );
                    });
                    return;
                }
                if sections.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("Nothing matches your search.").size(14.0).color(pal.dim));
                    });
                    return;
                }
                let avail = ui.available_width();
                for sec in &sections {
                    self.section_view(ui, sec, avail);
                }
                ui.add_space(50.0);
            });
    }

    /// Group-wide controls strip for the Lighting section and for user
    /// groups containing at least one light — the same components as
    /// the cards (toggle_switch, slim_slider, swatches + picker),
    /// broadcasting via group_broadcast. Pinned, Ungrouped, Cameras
    /// and Scenes never get these.
    fn group_controls(&mut self, ui: &mut Ui, sec: &Section) {
        if sec.kind != SectionKind::Lighting && sec.kind != SectionKind::Group {
            return;
        }
        let full = build_sections(&self.all_entries(), &self.cfg.layout, self.editing, "");
        let Some(fsec) = full.iter().find(|s| s.key == sec.key) else { return };
        let members: Vec<Device> = fsec
            .ids
            .iter()
            .filter_map(|id| self.device_index(id).map(|i| self.devices[i].clone()))
            .collect();
        let devs: Vec<&Device> = members.iter().filter(|d| !d.is_scene).collect();
        if !devs.iter().any(|d| device_is_light(d)) {
            return;
        }
        let pal = self.pal();
        let accent = self.accent();
        let any_on = group_master_on(&members);
        let b_vals: Vec<i64> = devs.iter().filter_map(|d| d.brightness_pct).collect();
        let t_vals: Vec<i64> = devs.iter().filter_map(|d| d.temp_slider).collect();
        let has_color = devs.iter().any(|d| d.has_color);
        let sec2 = sec.clone();
        Frame::none()
            .fill(pal.card)
            .stroke(Stroke::new(1.0, pal.border))
            .rounding(Rounding::same(R_CARD))
            .inner_margin(Margin::symmetric(16.0, 12.0))
            .show(ui, |ui| {
                ui.set_width((ui.available_width() - 32.0).max(200.0));
                ui.horizontal_wrapped(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("All").size(12.5).strong().color(pal.dim));
                        if toggle_switch(ui, any_on, true, accent, pal.track) {
                            self.group_broadcast(&sec2, GroupKind::Toggle, 0, None);
                        }
                    });
                    if let Some(avg) = group_average(&b_vals) {
                        self.group_slider(
                            ui, &sec2, "b", "Brightness", 11.0, 100.0, avg as f32,
                            |v| format!("{}%", v.round() as i64),
                            GroupKind::Brightness,
                        );
                    }
                    if let Some(avg) = group_average(&t_vals) {
                        self.group_slider(
                            ui, &sec2, "t", "Colour temperature", 1000.0, 10000.0, avg as f32,
                            |v| format!("{}K", temp_slider_to_kelvin((v / 50.0).round() as i64 * 50)),
                            GroupKind::Temp,
                        );
                    }
                    if has_color {
                        ui.horizontal_wrapped(|ui| {
                            for (name, hex) in SWATCHES {
                                if swatch(ui, hex, name, false, pal.text) {
                                    self.group_broadcast(&sec2, GroupKind::Color, 0, Some(hex.to_string()));
                                }
                            }
                            let gkey = format!("grp:{}", sec2.key);
                            let more = ui
                                .add(Button::new("🎨").frame(false))
                                .on_hover_text("More colours…");
                            if more.clicked() {
                                if self.open_color_for.as_deref() == Some(gkey.as_str()) {
                                    self.open_color_for = None;
                                } else {
                                    self.open_color_for = Some(gkey.clone());
                                    self.color_pick.insert(gkey.clone(), Color32::WHITE);
                                }
                            }
                            if self.open_color_for.as_deref() == Some(gkey.as_str()) {
                                let mut color = self
                                    .color_pick
                                    .get(&gkey)
                                    .copied()
                                    .unwrap_or(Color32::WHITE);
                                if egui::color_picker::color_edit_button_srgba(
                                    ui,
                                    &mut color,
                                    egui::color_picker::Alpha::Opaque,
                                )
                                .changed()
                                {
                                    self.color_pick.insert(gkey.clone(), color);
                                }
                                if ui
                                    .add(Button::new(RichText::new("Apply colour").color(Color32::WHITE)).fill(accent))
                                    .clicked()
                                {
                                    let hex = format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b());
                                    self.open_color_for = None;
                                    self.group_broadcast(&sec2, GroupKind::Color, 0, Some(hex));
                                }
                            }
                        });
                    }
                });
            });
        ui.add_space(12.0);
    }

    /// One average slider in the group-controls strip; commits on
    /// release, like the card sliders.
    fn group_slider(
        &mut self,
        ui: &mut Ui,
        sec: &Section,
        tag: &str,
        label: &str,
        min: f32,
        max: f32,
        avg: f32,
        fmt: impl Fn(f32) -> String,
        kind: GroupKind,
    ) {
        let pal = self.pal();
        let accent = self.accent();
        let key = format!("grp:{}|{}", sec.key, tag);
        let live = self.slider_live.get(&key).copied().unwrap_or(avg);
        ui.allocate_ui_with_layout(vec2(210.0, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).size(12.5).color(pal.dim));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(fmt(live)).size(12.5).strong());
                });
            });
            ui.add_space(3.0);
            let out = slim_slider(ui, egui::Id::new(&key), live, min, max, true, accent, pal.track);
            if out.changed {
                let v = if tag == "t" {
                    (out.value / 50.0).round() * 50.0
                } else {
                    out.value
                };
                self.slider_live.insert(key.clone(), v);
                self.dragging_slider = true;
            }
            if out.released {
                let v = self.slider_live.remove(&key).unwrap_or(out.value);
                self.dragging_slider = false;
                self.group_broadcast(sec, kind, v.round() as i64, None);
            }
        });
    }

    fn section_view(&mut self, ui: &mut Ui, sec: &Section, avail: f32) {
        let pal = self.pal();
        let sec_top = ui.cursor().top();
        let sec_left = ui.cursor().left();
        let collapsed = self.cfg.layout.collapsed.get(&sec.key).copied().unwrap_or(false);
        // ---- header ----
        ui.horizontal(|ui| {
            let chev = if collapsed { "▸" } else { "▾" };
            if ui.add(Button::new(chev).frame(false)).clicked() {
                self.cfg.layout.collapsed.insert(sec.key.clone(), !collapsed);
                self.save();
            }
            if self.editing && sec.kind == SectionKind::Group {
                if let Some(gid) = &sec.group_id {
                    let mut name = self.group_buf.get(gid).cloned().unwrap_or_else(|| sec.title.clone());
                    let resp = ui.add(TextEdit::singleline(&mut name).desired_width(170.0));
                    if resp.changed() {
                        self.group_buf.insert(gid.clone(), name.clone());
                    }
                    if resp.lost_focus() {
                        let trimmed = name.trim().to_string();
                        if !trimmed.is_empty() && trimmed != sec.title {
                            if let Some(g) = self.cfg.layout.groups.iter_mut().find(|g| &g.id == gid) {
                                g.name = trimmed;
                            }
                            self.save();
                        }
                    }
                }
            } else {
                ui.label(RichText::new(sec.title.clone()).size(16.5).strong());
            }
            Frame::none()
                .fill(pal.card)
                .stroke(Stroke::new(1.0, pal.border))
                .rounding(Rounding::same(10.0))
                .inner_margin(Margin::symmetric(8.0, 1.0))
                .show(ui, |ui| {
                    ui.label(RichText::new(format!("{}", sec.ids.len())).size(12.0).color(pal.dim));
                });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.editing && sec.kind == SectionKind::Group {
                    if ui.add(Button::new(RichText::new("Delete").color(DANGER)).small()).clicked() {
                        if let Some(gid) = &sec.group_id {
                            let gid = gid.clone();
                            self.cfg.layout.groups.retain(|g| g.id != gid);
                            self.cfg.layout.assign.retain(|_, v| v != &gid);
                            self.cfg.layout.section_order.remove(&format!("group:{}", gid));
                            self.save();
                            self.toast("Group deleted — its devices are Ungrouped now", false);
                        }
                    }
                }
                if self.editing && self.cfg.layout.section_order.contains_key(&sec.order_key) {
                    if ui.add(Button::new("↺ Reset to name order").small()).clicked() {
                        self.cfg.layout.section_order.remove(&sec.order_key);
                        self.save();
                        self.toast(format!("“{}” is back to name order", sec.title), false);
                    }
                }
            });
        });
        ui.add_space(8.0);
        self.group_controls(ui, sec);
        if collapsed {
            ui.add_space(10.0);
            return;
        }
        // ---- grid: true rows, like the web edition's CSS grid ----
        // Uniform columns; every card in a row fills a cell of
        // card width × the row's tallest card height (content
        // top-aligned, frame = the whole cell). egui has no stretching
        // grid, so each row renders on its own: cells are allocated at
        // the row's measured height (cached from the previous frame
        // under a content signature), cards enforce that height, and a
        // changed measurement is picked up a frame later.
        let compact = self.compact();
        let (min_w, gap) = if compact { (198.0, 10.0) } else { (238.0, 14.0) };
        let cols = (((avail + gap) / (min_w + gap)).floor() as usize).max(1);
        let card_w = (avail - (cols as f32 - 1.0) * gap) / cols as f32;
        let display = self.display_ids(sec);
        if display.is_empty() {
            if self.editing {
                ui.label(RichText::new("No items").color(pal.dim));
            }
            ui.add_space(14.0);
            self.section_rects
                .push((sec.key.clone(), egui::Rect::from_min_max(
                    egui::pos2(sec_left, sec_top),
                    egui::pos2(sec_left + avail, ui.cursor().top()),
                )));
            return;
        }
        let rows = display.len().div_ceil(cols);
        for (row_idx, chunk) in display.chunks(cols).enumerate() {
            let sig = self.row_signature(sec, row_idx, chunk, card_w);
            let row_h = self.row_heights.get(&sig).copied().unwrap_or(0.0);
            let mut actual_h = 0.0f32;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for item in chunk {
                    let item_owned = item.clone();
                    let alloc = ui.allocate_ui_with_layout(
                        vec2(card_w, row_h),
                        Layout::top_down(Align::Min),
                        |ui| match &item_owned {
                            Some(id) => {
                                let id = id.clone();
                                // Drag source (Edit mode): a drag sense
                                // over the whole cell, registered BEFORE
                                // the card's own widgets so buttons,
                                // sliders and fields keep priority in
                                // their own areas — like the web edition,
                                // where you drag a card by its body.
                                if self.editing && self.drag.is_none() {
                                    let cell_rect = ui.max_rect();
                                    let resp = ui.interact(
                                        cell_rect,
                                        egui::Id::new(("card-drag", &id)),
                                        Sense::drag(),
                                    );
                                    if resp.drag_started() {
                                        if let Some(p) = resp.interact_pointer_pos() {
                                            let start_idx = sec
                                                .ids
                                                .iter()
                                                .position(|x| x == &id)
                                                .unwrap_or(0);
                                            self.drag = Some(DragState {
                                                id: id.clone(),
                                                is_scene: sec.kind == SectionKind::Scenes,
                                                from_section: sec.key.clone(),
                                                size: cell_rect.size(),
                                                grab_offset: p - cell_rect.min,
                                                target: Some((sec.key.clone(), start_idx)),
                                            });
                                        }
                                    }
                                }
                                if self.camera_index(&id).is_some() {
                                    self.camera_card(ui, &id, sec, card_w, row_h);
                                } else {
                                    self.device_card(ui, &id, sec, card_w, row_h);
                                }
                            }
                            None => {
                                // Insertion gap while dragging: the
                                // other cards reflow around it.
                                let r = ui.max_rect().shrink(1.0);
                                let p = ui.painter();
                                p.rect_filled(r, Rounding::same(R_CARD), self.accent_soft());
                                p.rect_stroke(r, Rounding::same(R_CARD), Stroke::new(1.5, self.accent()));
                            }
                        },
                    );
                    actual_h = actual_h.max(alloc.response.rect.height());
                    self.cell_rects
                        .push((sec.key.clone(), item_owned, alloc.response.rect));
                }
            });
            if row_h == 0.0 || actual_h > row_h + 0.5 {
                self.row_heights.insert(sig, actual_h);
                ui.ctx().request_repaint();
            }
            if row_idx + 1 < rows {
                ui.add_space(gap);
            }
        }
        ui.add_space(14.0);
        self.section_rects
            .push((sec.key.clone(), egui::Rect::from_min_max(
                egui::pos2(sec_left, sec_top),
                egui::pos2(sec_left + avail, ui.cursor().top()),
            )));
    }

    /// Height-relevant shape of one card: anything that changes how
    /// tall the card renders (controls present, online, colour picker
    /// open). Part of the row-height cache signature.
    fn card_shape(&self, id: &str) -> String {
        if let Some(ci) = self.camera_index(id) {
            format!("cam:{}", self.cfg.cameras[ci].template)
        } else if let Some(di) = self.device_index(id) {
            let d = &self.devices[di];
            format!(
                "dev:{}:{}:{}:{}:{}:{}",
                d.online,
                d.brightness_pct.is_some(),
                d.temp_slider.is_some(),
                d.has_color,
                d.is_scene,
                self.open_color_for.as_deref() == Some(id)
            )
        } else {
            "?".to_string()
        }
    }

    fn row_signature(
        &self,
        sec: &Section,
        row_idx: usize,
        chunk: &[Option<String>],
        card_w: f32,
    ) -> String {
        let mut s = format!(
            "{}|{}|{}|{}|{}",
            sec.order_key,
            row_idx,
            self.editing,
            self.compact(),
            card_w as i32
        );
        for item in chunk {
            s.push('|');
            s.push_str(
                &item
                    .as_ref()
                    .map(|id| self.card_shape(id))
                    .unwrap_or_else(|| "gap".to_string()),
            );
        }
        s
    }

    /// The floating ghost card that follows the pointer during a drag.
    fn drag_ghost(&self, ctx: &egui::Context) {
        let Some(drag) = &self.drag else { return };
        let Some(pos) = ctx.input(|i| {
            i.pointer.hover_pos().or_else(|| i.pointer.interact_pos())
        }) else {
            return;
        };
        let (icon, name) = if let Some(ci) = self.camera_index(&drag.id) {
            ("📷", self.cfg.cameras[ci].name.clone())
        } else if let Some(di) = self.device_index(&drag.id) {
            let d = self.devices[di].clone();
            (type_icon(&d.icon_kind), self.display(&d))
        } else {
            return;
        };
        let pal = self.pal();
        let top_left = pos - drag.grab_offset;
        egui::Area::new(egui::Id::new("drag-ghost"))
            .fixed_pos(top_left)
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                Frame::none()
                    .fill(Color32::from_rgba_unmultiplied(
                        pal.card.r(),
                        pal.card.g(),
                        pal.card.b(),
                        238,
                    ))
                    .stroke(Stroke::new(1.5, self.accent()))
                    .rounding(Rounding::same(R_CARD))
                    .inner_margin(Margin::symmetric(14.0, 12.0))
                    .show(ui, |ui| {
                        ui.set_width((drag.size.x - 28.0).max(80.0));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon).size(20.0));
                            ui.add(
                                egui::Label::new(RichText::new(name).size(15.0).strong())
                                    .truncate(true),
                            );
                        });
                    });
            });
    }

    fn move_in_section(&mut self, sec: &Section, id: &str, dir: i32) {
        let mut ids = sec.ids.clone();
        let Some(i) = ids.iter().position(|x| x == id) else { return };
        let j = i as i32 + dir;
        if j < 0 || j as usize >= ids.len() {
            return;
        }
        ids.swap(i, j as usize);
        self.cfg.layout.section_order.insert(sec.order_key.clone(), ids);
        self.save();
    }
}

impl App {
    fn device_card(&mut self, ui: &mut Ui, id: &str, sec: &Section, card_w: f32, fill_h: f32) {
        let Some(idx) = self.device_index(id) else { return };
        let dev = self.devices[idx].clone();
        let pal = self.pal();
        let accent = self.accent();
        let compact = self.compact();
        let (pad_x, pad_top, pad_bot, radius) = if compact {
            (13.0, 11.0, 10.0, R_CARD_COMPACT)
        } else {
            (16.0, 15.0, 14.0, R_CARD)
        };
        let border = if dev.state && dev.online { accent } else { pal.border };
        let pinned = self.cfg.layout.pins.contains(&dev.id);
        Frame::none()
            .fill(pal.card)
            .stroke(Stroke::new(1.0, border))
            .rounding(Rounding::same(radius))
            .inner_margin(Margin { left: pad_x, right: pad_x, top: pad_top, bottom: pad_bot })
            .show(ui, |ui| {
                ui.set_width((card_w - 2.0 * pad_x).max(80.0));
                if fill_h > 0.0 {
                    // Stretch the frame to the row's full cell height.
                    ui.set_min_height((fill_h - pad_top - pad_bot).max(0.0));
                }
                // ---------- top row ----------
                ui.horizontal(|ui| {
                    if self.editing {
                        ui.label(RichText::new("⠿").color(pal.dim).size(16.0));
                    }
                    // icon box
                    let icon_size = if compact { 36.0 } else { 42.0 };
                    Frame::none()
                        .fill(if dev.state && dev.online { self.accent_soft() } else { pal.track })
                        .rounding(Rounding::same(if compact { 11.0 } else { 13.0 }))
                        .show(ui, |ui| {
                            ui.allocate_ui_with_layout(
                                vec2(icon_size, icon_size),
                                Layout::centered_and_justified(egui::Direction::LeftToRight),
                                |ui| {
                                    ui.label(
                                        RichText::new(type_icon(&dev.icon_kind))
                                            .size(if compact { 18.0 } else { 21.0 }),
                                    );
                                },
                            );
                        });
                    // name + status — a fixed-width block so the name
                    // truncates with an ellipsis instead of ever
                    // reaching the pin / toggle / hide controls.
                    let trailing_w = if dev.is_scene { 28.0 } else { 116.0 };
                    let name_w = (ui.available_width() - trailing_w - 8.0).max(40.0);
                    ui.allocate_ui_with_layout(
                        vec2(name_w, icon_size),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(self.display(&dev)).size(15.0).strong(),
                                    )
                                    .truncate(true),
                                );
                                if !dev.online {
                                    Frame::none()
                                        .fill(Color32::from_rgb(0x64, 0x74, 0x8b))
                                        .rounding(Rounding::same(6.0))
                                        .inner_margin(Margin::symmetric(6.0, 2.0))
                                        .show(ui, |ui| {
                                            ui.label(RichText::new("OFFLINE").size(10.5).color(Color32::WHITE).strong());
                                        });
                                }
                            });
                            ui.horizontal(|ui| {
                                let dot_color = if dev.state && dev.online { OK_GREEN } else { pal.dim };
                                ui.label(RichText::new("●").size(9.0).color(dot_color));
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(status_text(&dev)).size(12.5).color(pal.dim),
                                    )
                                    .truncate(true),
                                );
                            });
                        },
                    );
                    // trailing controls: pin, switch, hide (added right-to-left)
                    ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                        let hide = ui
                            .add(Button::new(RichText::new("✕").color(pal.dim).size(13.0)).frame(false))
                            .on_hover_text("Hide this device");
                        if hide.clicked() {
                            self.cfg.layout.hidden.push(dev.id.clone());
                            self.save();
                            self.toast(format!("{} hidden — bring it back from Settings", self.display(&dev)), false);
                        }
                        if !dev.is_scene {
                            let on = dev.state && dev.online;
                            if toggle_switch(ui, on, dev.online, accent, pal.track) {
                                self.do_toggle(&dev.id);
                            }
                            let pin = ui
                                .add(
                                    Button::new(
                                        RichText::new("📌")
                                            .size(15.0)
                                            .color(if pinned { pal.text } else { pal.faint }),
                                    )
                                    .frame(false),
                                )
                                .on_hover_text(if pinned {
                                    "Unpin (remove shortcut from the top)"
                                } else {
                                    "Pin a shortcut to the top"
                                });
                            if pin.clicked() {
                                self.toggle_pin(&dev.id);
                            }
                        }
                    });
                });
                // ---------- controls ----------
                if dev.online && !dev.is_scene {
                    if let Some(pct) = dev.brightness_pct {
                        self.slider_row(ui, &dev, "b", "Brightness", 11.0, 100.0, pct as f32, |v| {
                            format!("{}%", v.round() as i64)
                        });
                    }
                    if let Some(t) = dev.temp_slider {
                        self.slider_row(ui, &dev, "t", "Colour temperature", 1000.0, 10000.0, t as f32, |v| {
                            format!("{}K", temp_slider_to_kelvin((v / 50.0).round() as i64 * 50))
                        });
                    }
                    if dev.has_color {
                        ui.add_space(4.0);
                        let current_hex = dev
                            .hue
                            .map(|h| hsv_to_hex(h, dev.saturation.unwrap_or(0.0), 1.0));
                        ui.horizontal_wrapped(|ui| {
                            for (name, hex) in SWATCHES {
                                let active = current_hex.as_deref() == Some(hex);
                                if swatch(ui, hex, name, active, pal.text) {
                                    self.do_color(&dev.id, hex);
                                }
                            }
                            let more = ui
                                .add(Button::new("🎨").frame(false))
                                .on_hover_text("More colours…");
                            if more.clicked() {
                                if self.open_color_for.as_deref() == Some(dev.id.as_str()) {
                                    self.open_color_for = None;
                                } else {
                                    self.open_color_for = Some(dev.id.clone());
                                    let c = current_hex
                                        .as_deref()
                                        .map(hex_to_color32)
                                        .unwrap_or(Color32::WHITE);
                                    self.color_pick.insert(dev.id.clone(), c);
                                }
                            }
                        });
                        if self.open_color_for.as_deref() == Some(dev.id.as_str()) {
                            ui.add_space(8.0);
                            ui.separator();
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                let mut color = self
                                    .color_pick
                                    .get(&dev.id)
                                    .copied()
                                    .unwrap_or(Color32::WHITE);
                                if egui::color_picker::color_edit_button_srgba(
                                    ui,
                                    &mut color,
                                    egui::color_picker::Alpha::Opaque,
                                )
                                .changed()
                                {
                                    self.color_pick.insert(dev.id.clone(), color);
                                }
                                if ui
                                    .add(Button::new(RichText::new("Apply colour").color(Color32::WHITE)).fill(accent))
                                    .clicked()
                                {
                                    let hex = format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b());
                                    self.do_color(&dev.id, &hex);
                                }
                            });
                        }
                    }
                }
                // ---------- scene ----------
                if dev.is_scene {
                    ui.add_space(10.0);
                    let btn = Button::new(RichText::new("▶ Start scene").size(14.0).color(Color32::WHITE)).fill(accent);
                    if ui
                        .add_enabled(dev.online, btn)
                        .on_hover_text("Run this scene")
                        .clicked()
                    {
                        self.do_scene_start(&dev.id);
                    }
                }
                // ---------- edit tools ----------
                if self.editing {
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(8.0);
                    let mut alias = self
                        .alias_buf
                        .get(&dev.id)
                        .cloned()
                        .unwrap_or_else(|| self.cfg.layout.alias.get(&dev.id).cloned().unwrap_or_default());
                    let aresp = ui.add(
                        TextEdit::singleline(&mut alias)
                            .desired_width(f32::INFINITY)
                            .hint_text("Display name (alias)"),
                    );
                    if aresp.changed() {
                        self.alias_buf.insert(dev.id.clone(), alias.clone());
                    }
                    if aresp.lost_focus() {
                        let v = alias.trim().to_string();
                        if v.is_empty() {
                            self.cfg.layout.alias.remove(&dev.id);
                        } else {
                            self.cfg.layout.alias.insert(dev.id.clone(), v);
                        }
                        self.save();
                    }
                    if !dev.is_scene {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let cur_assign = self.cfg.layout.assign.get(&dev.id).cloned();
                            let auto_label = if device_is_light(&dev) {
                                "Lighting (automatic)"
                            } else {
                                "Ungrouped"
                            };
                            let cur_label = match &cur_assign {
                                Some(gid) => self
                                    .cfg
                                    .layout
                                    .groups
                                    .iter()
                                    .find(|g| &g.id == gid)
                                    .map(|g| g.name.clone())
                                    .unwrap_or_else(|| auto_label.to_string()),
                                None => auto_label.to_string(),
                            };
                            let mut chosen: Option<Option<String>> = None;
                            let combo_w = (ui.available_width() - 92.0).max(84.0);
                            ComboBox::from_id_source(("grp", &dev.id))
                                .selected_text(cur_label)
                                .width(combo_w)
                                .show_ui(ui, |ui| {
                                    if ui.selectable_label(cur_assign.is_none(), auto_label).clicked() {
                                        chosen = Some(None);
                                    }
                                    for g in &self.cfg.layout.groups {
                                        if ui
                                            .selectable_label(cur_assign.as_deref() == Some(g.id.as_str()), g.name.clone())
                                            .clicked()
                                        {
                                            chosen = Some(Some(g.id.clone()));
                                        }
                                    }
                                });
                            if let Some(c) = chosen {
                                match c {
                                    Some(gid) => {
                                        self.cfg.layout.assign.insert(dev.id.clone(), gid);
                                    }
                                    None => {
                                        self.cfg.layout.assign.remove(&dev.id);
                                    }
                                }
                                self.save();
                            }
                            if ui.add(Button::new(if pinned { "📌 Unpin" } else { "📌 Pin" }).small()).clicked() {
                                self.toggle_pin(&dev.id);
                            }
                        });
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add(Button::new("↑ Move up").small()).clicked() {
                            self.move_in_section(sec, &dev.id, -1);
                        }
                        if ui.add(Button::new("↓ Move down").small()).clicked() {
                            self.move_in_section(sec, &dev.id, 1);
                        }
                    });
                }
            });
    }

    /// An RTSP camera card — a peer of the device card: same frame,
    /// grid cell, pinning, grouping and edit tools. Quality toggle for
    /// the Tapo templates, Watch + Copy URL actions.
    fn camera_card(&mut self, ui: &mut Ui, id: &str, sec: &Section, card_w: f32, fill_h: f32) {
        let Some(ci) = self.camera_index(id) else { return };
        let cam = self.cfg.cameras[ci].clone();
        let pal = self.pal();
        let accent = self.accent();
        let compact = self.compact();
        let (pad_x, pad_top, pad_bot, radius) = if compact {
            (13.0, 11.0, 10.0, R_CARD_COMPACT)
        } else {
            (16.0, 15.0, 14.0, R_CARD)
        };
        let pinned = self.cfg.layout.pins.contains(&id.to_string());
        Frame::none()
            .fill(pal.card)
            .stroke(Stroke::new(1.0, pal.border))
            .rounding(Rounding::same(radius))
            .inner_margin(Margin { left: pad_x, right: pad_x, top: pad_top, bottom: pad_bot })
            .show(ui, |ui| {
                ui.set_width((card_w - 2.0 * pad_x).max(80.0));
                if fill_h > 0.0 {
                    ui.set_min_height((fill_h - pad_top - pad_bot).max(0.0));
                }
                // ---------- top row ----------
                ui.horizontal(|ui| {
                    if self.editing {
                        ui.label(RichText::new("⠿").color(pal.dim).size(16.0));
                    }
                    let icon_size = if compact { 36.0 } else { 42.0 };
                    Frame::none()
                        .fill(pal.track)
                        .rounding(Rounding::same(if compact { 11.0 } else { 13.0 }))
                        .show(ui, |ui| {
                            ui.allocate_ui_with_layout(
                                vec2(icon_size, icon_size),
                                Layout::centered_and_justified(egui::Direction::LeftToRight),
                                |ui| {
                                    ui.label(
                                        RichText::new("📷").size(if compact { 18.0 } else { 21.0 }),
                                    );
                                },
                            );
                        });
                    let name_w = (ui.available_width() - 60.0 - 8.0).max(40.0);
                    ui.allocate_ui_with_layout(
                        vec2(name_w, icon_size),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(cam.name.clone()).size(15.0).strong())
                                    .truncate(true),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(camera_status_text(&cam)).size(12.5).color(pal.dim),
                                )
                                .truncate(true),
                            );
                        },
                    );
                    ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                        let hide = ui
                            .add(Button::new(RichText::new("✕").color(pal.dim).size(13.0)).frame(false))
                            .on_hover_text("Hide this camera");
                        if hide.clicked() {
                            self.cfg.layout.hidden.push(id.to_string());
                            self.save();
                            self.toast(
                                format!("{} hidden — bring it back from Settings", cam.name),
                                false,
                            );
                        }
                        let pin = ui
                            .add(
                                Button::new(
                                    RichText::new("📌")
                                        .size(15.0)
                                        .color(if pinned { pal.text } else { pal.faint }),
                                )
                                .frame(false),
                            )
                            .on_hover_text(if pinned {
                                "Unpin (remove shortcut from the top)"
                            } else {
                                "Pin a shortcut to the top"
                            });
                        if pin.clicked() {
                            self.toggle_pin(id);
                        }
                    });
                });
                // ---------- quality (Tapo templates) ----------
                if cam.template == "tapo1" || cam.template == "tapo2" {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        for (tpl, label) in [("tapo1", "Stream 1 · HD"), ("tapo2", "Stream 2 · SD")] {
                            let active = cam.template == tpl;
                            let btn = if active {
                                Button::new(RichText::new(label).color(Color32::WHITE)).fill(accent)
                            } else {
                                Button::new(label)
                            };
                            if ui.add(btn).clicked() && !active {
                                if let Some(ci2) = self.camera_index(id) {
                                    self.cfg.cameras[ci2].template = tpl.to_string();
                                    self.save();
                                }
                            }
                        }
                    });
                }
                // ---------- actions ----------
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let watch = Button::new(
                        RichText::new("▶ Watch").size(14.0).color(Color32::WHITE),
                    )
                    .fill(accent);
                    if ui.add(watch).clicked() {
                        self.watch_camera(id);
                    }
                    if ui.add(Button::new("Copy URL")).clicked() {
                        ui.ctx().copy_text(build_rtsp_url(&cam));
                        self.toast("Stream URL copied", false);
                    }
                });
                // ---------- edit tools ----------
                if self.editing {
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(Button::new("✎ Edit").small()).clicked() {
                            self.open_camera_form(Some(id));
                        }
                        if ui
                            .add(Button::new(RichText::new("🗑 Delete").color(DANGER)).small())
                            .clicked()
                        {
                            self.delete_camera(id);
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let cur_assign = self.cfg.layout.assign.get(id).cloned();
                        let cur_label = match &cur_assign {
                            Some(gid) => self
                                .cfg
                                .layout
                                .groups
                                .iter()
                                .find(|g| &g.id == gid)
                                .map(|g| g.name.clone())
                                .unwrap_or_else(|| "Cameras (automatic)".to_string()),
                            None => "Cameras (automatic)".to_string(),
                        };
                        let mut chosen: Option<Option<String>> = None;
                        let combo_w = (ui.available_width() - 92.0).max(84.0);
                        ComboBox::from_id_source(("grpcam", id))
                            .selected_text(cur_label)
                            .width(combo_w)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(cur_assign.is_none(), "Cameras (automatic)").clicked() {
                                    chosen = Some(None);
                                }
                                for g in &self.cfg.layout.groups {
                                    if ui
                                        .selectable_label(
                                            cur_assign.as_deref() == Some(g.id.as_str()),
                                            g.name.clone(),
                                        )
                                        .clicked()
                                    {
                                        chosen = Some(Some(g.id.clone()));
                                    }
                                }
                            });
                        if let Some(c) = chosen {
                            match c {
                                Some(gid) => {
                                    self.cfg.layout.assign.insert(id.to_string(), gid);
                                }
                                None => {
                                    self.cfg.layout.assign.remove(id);
                                }
                            }
                            self.save();
                        }
                        if ui
                            .add(Button::new(if pinned { "📌 Unpin" } else { "📌 Pin" }).small())
                            .clicked()
                        {
                            self.toggle_pin(id);
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add(Button::new("↑ Move up").small()).clicked() {
                            self.move_in_section(sec, id, -1);
                        }
                        if ui.add(Button::new("↓ Move down").small()).clicked() {
                            self.move_in_section(sec, id, 1);
                        }
                    });
                }
            });
    }

    /// One labelled slider row (Brightness / Colour temperature), with the
    /// live value while dragging and a commit on release — like the web.
    fn slider_row(
        &mut self,
        ui: &mut Ui,
        dev: &Device,
        tag: &str,
        label: &str,
        min: f32,
        max: f32,
        value: f32,
        fmt: impl Fn(f32) -> String,
    ) {
        let pal = self.pal();
        let accent = self.accent();
        let key = format!("{}|{}", dev.id, tag);
        let live = self.slider_live.get(&key).copied().unwrap_or(value);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).size(12.5).color(pal.dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(fmt(live)).size(12.5).strong());
            });
        });
        ui.add_space(3.0);
        let out = slim_slider(
            ui,
            egui::Id::new(&key),
            live,
            min,
            max,
            true,
            accent,
            pal.track,
        );
        if out.changed {
            let v = if tag == "t" {
                (out.value / 50.0).round() * 50.0 // temp slider steps by 50, like the web
            } else {
                out.value
            };
            self.slider_live.insert(key.clone(), v);
            self.dragging_slider = true;
        }
        if out.released {
            let v = self.slider_live.remove(&key).unwrap_or(out.value);
            self.dragging_slider = false;
            if tag == "t" {
                self.do_temp(&dev.id, v.round() as i64);
            } else {
                self.do_brightness(&dev.id, v.round() as i64);
            }
        }
    }
}

impl App {
    fn settings_panel<R>(
        ui: &mut Ui,
        pal: &Palette,
        title: &str,
        add: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        let mut result = None;
        Frame::none()
            .fill(pal.card)
            .stroke(Stroke::new(1.0, pal.border))
            .rounding(Rounding::same(R_CARD))
            .inner_margin(Margin::symmetric(22.0, 20.0))
            .show(ui, |ui| {
                ui.set_width((ui.available_width() - 44.0).max(200.0));
                ui.label(RichText::new(title).size(15.5).strong());
                ui.add_space(8.0);
                result = Some(add(ui));
            });
        ui.add_space(16.0);
        result.unwrap()
    }

    fn view_settings(&mut self, ui: &mut Ui) {
        let pal = self.pal();
        let accent = self.accent();
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let max_w = 760.0_f32.min(ui.available_width());
                let x_off = ((ui.available_width() - max_w) / 2.0).max(0.0);
                ui.horizontal(|ui| {
                    ui.add_space(x_off);
                    ui.vertical(|ui| {
                        ui.set_width(max_w);
                        // ---- Appearance ----
                        Self::settings_panel(ui, &pal, "Appearance", |ui| {
                            Self::field_label(ui, &pal, "Theme");
                            let mut theme = self.cfg.prefs.theme.clone();
                            ComboBox::from_id_source("set_theme")
                                .selected_text(match theme.as_str() {
                                    "light" => "Light",
                                    "black" => "Black",
                                    _ => "Dark",
                                })
                                .width(ui.available_width())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut theme, "dark".to_string(), "Dark");
                                    ui.selectable_value(&mut theme, "light".to_string(), "Light");
                                    ui.selectable_value(&mut theme, "black".to_string(), "Black");
                                });
                            if theme != self.cfg.prefs.theme {
                                self.cfg.prefs.theme = theme;
                                self.save();
                            }
                            ui.add_space(10.0);
                            Self::field_label(ui, &pal, "Accent colour");
                            ui.horizontal(|ui| {
                                for (name, hex) in ACCENTS {
                                    let c = hex_to_color32(hex);
                                    let (rect, resp) =
                                        ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
                                    let p = ui.painter();
                                    p.circle_filled(rect.center(), 13.0, c);
                                    if self.cfg.prefs.accent == hex {
                                        p.circle_stroke(rect.center(), 14.0, Stroke::new(3.0, pal.text));
                                    }
                                    if resp.on_hover_text(name).clicked() {
                                        self.cfg.prefs.accent = hex.to_string();
                                        self.save();
                                    }
                                }
                            });
                            let _ = accent;
                            ui.add_space(10.0);
                            Self::field_label(ui, &pal, "Card density");
                            let mut density = self.cfg.prefs.density.clone();
                            ComboBox::from_id_source("set_density")
                                .selected_text(if density == "compact" { "Compact" } else { "Comfortable" })
                                .width(ui.available_width())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut density, "comfortable".to_string(), "Comfortable");
                                    ui.selectable_value(&mut density, "compact".to_string(), "Compact");
                                });
                            if density != self.cfg.prefs.density {
                                self.cfg.prefs.density = density;
                                self.save();
                            }
                        });
                        // ---- App updates ----
                        let mut upd_act: Option<&str> = None;
                        Self::settings_panel(ui, &pal, "App updates", |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "You're running version {}. The app checks GitHub for updates at most once a day; you can also check now.",
                                    APP_VERSION
                                ))
                                .size(12.5)
                                .color(pal.dim),
                            );
                            ui.add_space(8.0);
                            if self.update_checking {
                                ui.label(RichText::new("Checking for updates…").size(13.5));
                            } else if let Some((got, total)) = self.update_dl {
                                let frac = if total > 0 {
                                    got as f32 / total as f32
                                } else {
                                    0.0
                                };
                                ui.add(
                                    egui::ProgressBar::new(frac)
                                        .text(format!("Downloading the installer… {}%", (frac * 100.0).round() as i64)),
                                );
                            } else if let Some(info) = &self.update_info {
                                if version_newer(&info.version, APP_VERSION) {
                                    ui.label(
                                        RichText::new(format!("Version {} is available.", info.version))
                                            .size(13.5)
                                            .strong(),
                                    );
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add(
                                                Button::new(
                                                    RichText::new("Download & install").color(Color32::WHITE),
                                                )
                                                .fill(accent),
                                            )
                                            .clicked()
                                        {
                                            upd_act = Some("download");
                                        }
                                        if ui.add(Button::new("Open releases page")).clicked() {
                                            upd_act = Some("releases");
                                        }
                                    });
                                    ui.add_space(6.0);
                                    ui.label(
                                        RichText::new("The download's file name is verified before it runs. This app closes itself once the installer starts, so the installer can replace it.")
                                            .size(12.5)
                                            .color(pal.dim),
                                    );
                                } else {
                                    ui.label(
                                        RichText::new(format!(
                                            "You're up to date — {} is the latest version.",
                                            info.version
                                        ))
                                        .size(13.5),
                                    );
                                }
                            }
                            if let Some(err) = &self.update_error {
                                ui.add_space(6.0);
                                ui.label(RichText::new(err.clone()).size(12.5).color(DANGER));
                                if ui.add(Button::new("Open releases page")).clicked() {
                                    upd_act = Some("releases");
                                }
                            }
                            ui.add_space(8.0);
                            if ui.add(Button::new("Check for updates")).clicked() {
                                upd_act = Some("check");
                            }
                        });
                        match upd_act {
                            Some("check") => self.spawn_update_check(true),
                            Some("download") => self.spawn_update_download(),
                            Some("releases") => {
                                let _ = open_external(RELEASES_PAGE);
                            }
                            _ => {}
                        }
                        // ---- Connection ----
                        Self::settings_panel(ui, &pal, "Connection", |ui| {
                            ui.label(
                                RichText::new("Tuya's cloud API is called straight from this app. The US and EU endpoints accept direct calls; the China endpoint (and some networks) need a CORS-style proxy in between.")
                                    .size(12.5)
                                    .color(pal.dim),
                            );
                            ui.add_space(8.0);
                            self.proxy_fields(ui, &pal, "set");
                            ui.add_space(10.0);
                            Self::field_label(ui, &pal, "Autorefresh interval");
                            let mut interval = self.cfg.prefs.interval;
                            ComboBox::from_id_source("set_interval")
                                .selected_text(interval_label(interval))
                                .width(ui.available_width())
                                .show_ui(ui, |ui| {
                                    for (v, l) in INTERVALS {
                                        ui.selectable_value(&mut interval, v, l);
                                    }
                                });
                            if interval != self.cfg.prefs.interval {
                                self.cfg.prefs.interval = interval;
                                self.save();
                            }
                        });
                        // ---- Stream gateway & cameras ----
                        Self::settings_panel(ui, &pal, "Stream gateway & cameras", |ui| {
                            ui.label(
                                RichText::new("Cameras you add (✎ Edit → + Add camera) stream over RTSP, which this app can't play directly. Point ▶ Watch at a small stream gateway on your network — go2rtc or MediaMTX — and Watch opens the gateway's watch page in your browser. The gateway needs a stream with the same name as the camera here, fed from the same RTSP address (the card's Copy URL button gives you the exact address). Leave the address empty to hand streams to an external player app instead.")
                                    .size(12.5)
                                    .color(pal.dim),
                            );
                            ui.add_space(8.0);
                            Self::field_label(ui, &pal, "Stream gateway address (optional)");
                            let mut gw = self.cfg.prefs.gateway_url.clone();
                            if ui
                                .add(
                                    TextEdit::singleline(&mut gw)
                                        .desired_width(f32::INFINITY)
                                        .hint_text("http://192.168.1.10:1984 (go2rtc) · http://192.168.1.10:8889 (MediaMTX)"),
                                )
                                .changed()
                            {
                                self.cfg.prefs.gateway_url = gw;
                                self.save();
                            }
                            ui.add_space(10.0);
                            Self::field_label(ui, &pal, "Gateway type");
                            let mut gwt = self.cfg.prefs.gateway_type.clone();
                            ComboBox::from_id_source("set_gw_type")
                                .selected_text(if gwt == "mediamtx" { "MediaMTX (HLS)" } else { "go2rtc" })
                                .width(ui.available_width())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut gwt, "go2rtc".to_string(), "go2rtc");
                                    ui.selectable_value(&mut gwt, "mediamtx".to_string(), "MediaMTX (HLS)");
                                });
                            if gwt != self.cfg.prefs.gateway_type {
                                self.cfg.prefs.gateway_type = gwt;
                                self.save();
                            }
                            ui.add_space(10.0);
                            Self::field_label(ui, &pal, "External player command");
                            let mut pc = self.cfg.prefs.player_cmd.clone();
                            if ui
                                .add(
                                    TextEdit::singleline(&mut pc)
                                        .desired_width(f32::INFINITY)
                                        .hint_text("vlc"),
                                )
                                .changed()
                            {
                                self.cfg.prefs.player_cmd = pc;
                                self.save();
                            }
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("Used when no gateway address is set: Watch starts this program with the stream URL as its only argument.")
                                    .size(12.5)
                                    .color(pal.dim),
                            );
                        });
                        // ---- Saved login ----
                        Self::settings_panel(ui, &pal, "Saved login", |ui| {
                            let info = match &self.cfg.creds {
                                Some(c) => format!(
                                    "A login is saved for {} in this device's config (used for silent re-login when the session expires).",
                                    c.user_name
                                ),
                                None => "No login is saved on this device.".to_string(),
                            };
                            ui.label(RichText::new(info).size(12.5).color(pal.dim));
                            ui.add_space(8.0);
                            if ui
                                .add(Button::new(RichText::new("Forget saved login").color(DANGER)))
                                .clicked()
                            {
                                self.cfg.creds = None;
                                self.cfg.session = None;
                                self.save();
                                self.toast("Saved login forgotten", false);
                            }
                        });
                        // ---- Hidden devices ----
                        Self::settings_panel(ui, &pal, "Hidden devices", |ui| {
                            ui.label(
                                RichText::new("Devices and cameras you hid with the ✕ on their card. They stay hidden until you bring them back here.")
                                    .size(12.5)
                                    .color(pal.dim),
                            );
                            ui.add_space(6.0);
                            if self.cfg.layout.hidden.is_empty() {
                                ui.label(RichText::new("Nothing is hidden. 🎉").size(12.5).color(pal.dim));
                            }
                            let hidden = self.cfg.layout.hidden.clone();
                            for id in hidden {
                                let name = self
                                    .device_index(&id)
                                    .map(|i| self.display(&self.devices[i].clone()))
                                    .or_else(|| {
                                        self.camera_index(&id)
                                            .map(|ci| self.cfg.cameras[ci].name.clone())
                                    })
                                    .unwrap_or_else(|| id.clone());
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(name).size(14.0));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.add(Button::new("Unhide").small()).clicked() {
                                            self.cfg.layout.hidden.retain(|x| x != &id);
                                            self.save();
                                        }
                                    });
                                });
                                ui.separator();
                            }
                        });
                        // ---- Help ----
                        Self::settings_panel(ui, &pal, "Help", |ui| {
                            let paras = [
                                "How login works. This app signs in to Tuya's cloud with the same account you use in the Smart Life (or Tuya Smart) app, then asks Tuya for your device list and sends control commands — the same approach as the classic Smart Life web dashboards. Pick the region where your account was registered; the wrong region is the most common cause of “login failed”.",
                                "Your credentials. Your username and password go directly to Tuya's servers (px1.tuya*.com) over HTTPS. If you tick “Keep me logged in”, they are kept in this app's config file (in your user profile, under SmartLifeTuyaDesktop\\config.json) so the app can silently sign back in when the session token expires — use it only on your own devices, and use “Forget saved login” above to erase them.",
                                "The proxy. Some networks block direct calls to Tuya's endpoints. In “Auto” mode, China-region traffic is routed through a community CORS proxy (cors.smartathome.co.uk); US and EU traffic goes direct. Note a proxy can see the requests it forwards (they contain a session token, never your password after sign-in — the sign-in request itself does contain your password, so only use proxies you trust for CN logins).",
                                "Customising. Sections sort A–Z by name by default. Use ✎ Edit to drag cards into your own order inside a section — drag a card by its body and drop it where it belongs, or use the ↑/↓ buttons (that order then sticks, and each section has a “Reset to name order” button) — create groups (rooms), rename devices with an alias, or hide devices you never use. Dragging a card onto another section moves it there. Use the 📌 button on any card to pin a shortcut copy of it in the Pinned section at the top — the device also stays in its group below. Scenes always live in their own Scenes section.",
                                "Lighting & group controls. Lights organise themselves: any bulb or strip that isn't assigned to one of your groups lives in the built-in Lighting section, and cameras get their own built-in Cameras section the same way — assigning a device to a group always wins over the automatic sections. The Lighting header (and the header of any group containing a light) has group controls: a master switch (on if any member is on; clicking turns the whole group on or off), brightness and colour-temperature sliders showing the members' average, and colour swatches. Group controls broadcast — they send the command to each member in turn, and changing a single light afterwards simply overrides it for that light. Nothing about the group itself is stored.",
                                "Cameras (RTSP). Tapo cameras: in the Tapo app, open the camera → Settings → Advanced Settings → Camera Account and create a camera account, and make sure RTSP is enabled for the camera. The username and password you add here are that camera account — not your TP-Link cloud account. Then use ✎ Edit → + Add camera and enter the camera's IP address (a fixed or DHCP-reserved IP works best, so the address doesn't change). The app builds the stream address for you: rtsp://user:pass@IP:554/stream1 (high quality) or /stream2 (standard) — or pick “Custom path” for other RTSP cameras. Camera cards live alongside your devices and can be pinned, grouped and reordered the same way; unassigned cameras live in the built-in Cameras section until you assign them to a group of your own.",
                                "Watching a stream. RTSP can't play inside this app. Without a gateway, ▶ Watch hands the stream address to the player app named in Settings → Stream gateway & cameras (VLC by default) — or use Copy URL and paste the address into VLC's “Open Network Stream”. For in-browser playback, run a small stream gateway such as go2rtc or MediaMTX on your network, give it a stream with the same name as the camera here (fed from the same RTSP address — Copy URL has it), and enter the gateway's address in Settings → Stream gateway & cameras. ▶ Watch then opens the gateway's watch page in your browser.",
                                "Limits. Tuya rate-limits the device-list call; if you refresh very often the app may briefly show the last known states. Autorefresh every 30s or slower is comfortable.",
                            ];
                            for p in paras {
                                ui.label(RichText::new(p).size(13.5).color(pal.dim));
                                ui.add_space(6.0);
                            }
                        });
                        ui.add_space(30.0);
                    });
                });
            });
    }

    /// Add/Edit camera dialog — the Rust edition of the web modal.
    fn camera_window(&mut self, ctx: &egui::Context) {
        let Some(mut form) = self.cam_form.take() else { return };
        let pal = self.pal();
        let mut action: Option<&str> = None; // "save" | "cancel" | "delete"
        egui::Window::new(if form.id.is_some() { "Edit camera" } else { "Add camera" })
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(320.0);
                let field = |ui: &mut Ui, label: &str, buf: &mut String, password: bool| {
                    ui.label(RichText::new(label).size(12.5).strong().color(pal.dim));
                    ui.add_space(4.0);
                    ui.add(
                        TextEdit::singleline(buf)
                            .desired_width(f32::INFINITY)
                            .password(password),
                    );
                    ui.add_space(8.0);
                };
                field(ui, "Name", &mut form.name, false);
                field(ui, "Username", &mut form.user, false);
                field(ui, "Password", &mut form.pass, true);
                field(ui, "IP address or host", &mut form.host, false);
                field(ui, "Port", &mut form.port, false);
                ui.label(RichText::new("Template").size(12.5).strong().color(pal.dim));
                ui.add_space(4.0);
                let cur_label = CAMERA_TEMPLATES
                    .iter()
                    .find(|(k, _)| *k == form.template)
                    .map(|(_, l)| *l)
                    .unwrap_or("Tapo Stream 1 — High Quality");
                ComboBox::from_id_source("cam_tpl")
                    .selected_text(cur_label)
                    .width(ui.available_width() - 8.0)
                    .show_ui(ui, |ui| {
                        for (k, l) in CAMERA_TEMPLATES {
                            ui.selectable_value(&mut form.template, k.to_string(), l);
                        }
                    });
                ui.add_space(8.0);
                if form.template == "custom" {
                    field(ui, "Stream path", &mut form.path, false);
                }
                ui.label(
                    RichText::new("Tapo: in the Tapo app, enable RTSP (Advanced Settings → Camera Account) and create a camera account — the username and password are that account, not your TP-Link cloud login.")
                        .size(12.5)
                        .color(pal.dim),
                );
                if let Some(err) = &form.error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(err.clone()).size(12.5).color(DANGER));
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if form.id.is_some()
                        && ui
                            .add(Button::new(RichText::new("Delete").color(DANGER)))
                            .clicked()
                    {
                        action = Some("delete");
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(Button::new(RichText::new("Save camera").color(Color32::WHITE)).fill(self.accent()))
                            .clicked()
                        {
                            action = Some("save");
                        }
                        if ui.add(Button::new("Cancel")).clicked() {
                            action = Some("cancel");
                        }
                    });
                });
            });
        match action {
            Some("save") => {
                let name = form.name.trim().to_string();
                let host = form.host.trim().to_string();
                if name.is_empty() || host.is_empty() {
                    form.error = Some("Name and host are required.".to_string());
                    self.cam_form = Some(form);
                    return;
                }
                let port: u16 = form
                    .port
                    .trim()
                    .parse::<u32>()
                    .map(|p| p.min(65535) as u16)
                    .unwrap_or(554);
                let cam = Camera {
                    id: form
                        .id
                        .as_ref()
                        .map(|eid| eid.trim_start_matches("cam:").to_string())
                        .unwrap_or_else(|| format!("{}", now_ms())),
                    name,
                    user: form.user.trim().to_string(),
                    password: obfuscate(&form.pass),
                    host,
                    port,
                    template: form.template.clone(),
                    path: form.path.trim().to_string(),
                };
                if let Some(eid) = &form.id {
                    if let Some(ci) = self.camera_index(eid) {
                        self.cfg.cameras[ci] = cam;
                    }
                } else {
                    self.cfg.cameras.push(cam);
                }
                self.save();
                self.toast("Camera saved", false);
            }
            Some("delete") => {
                if let Some(eid) = form.id.clone() {
                    self.delete_camera(&eid);
                }
            }
            Some(_) => {}
            None => {
                self.cam_form = Some(form);
            }
        }
    }

    fn show_toasts(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.at.elapsed() < Duration::from_millis(3600));
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(200));
        let pal = self.pal();
        let items: Vec<(String, bool)> =
            self.toasts.iter().map(|t| (t.msg.clone(), t.err)).collect();
        let dark = self.cfg.prefs.theme != "light";
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -24.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    for (msg, err) in items {
                        let (fill, stroke) = if err {
                            (ERR_BG, ERR_BORDER)
                        } else if self.cfg.prefs.theme == "black" {
                            (TOAST_BG_BLACK, TOAST_BORDER_BLACK)
                        } else if dark {
                            (TOAST_BG_DARK, TOAST_BORDER)
                        } else {
                            (TOAST_BG_LIGHT, TOAST_BORDER)
                        };
                        Frame::none()
                            .fill(fill)
                            .stroke(Stroke::new(1.0, stroke))
                            .rounding(Rounding::same(13.0))
                            .inner_margin(Margin::symmetric(17.0, 10.0))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(msg)
                                        .size(13.5)
                                        .color(Color32::from_rgb(0xee, 0xf1, 0xf7)),
                                );
                            });
                        ui.add_space(8.0);
                    }
                });
                let _ = pal;
            });
    }

    fn new_group_window(&mut self, ctx: &egui::Context) {
        let Some(mut name) = self.new_group.clone() else { return };
        let mut close: Option<Option<String>> = None;
        egui::Window::new("New group")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(280.0);
                ui.label("Name for the new group (e.g. Living Room):");
                ui.add_space(6.0);
                let resp = ui.add(TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
                resp.request_focus();
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.add(Button::new("Create").fill(self.accent())).clicked()
                        || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        let trimmed = name.trim().to_string();
                        if !trimmed.is_empty() {
                            close = Some(Some(trimmed));
                        }
                    }
                    if ui.add(Button::new("Cancel")).clicked() {
                        close = Some(None);
                    }
                });
            });
        match close {
            Some(Some(group_name)) => {
                self.cfg.layout.groups.push(Group {
                    id: format!("g{}", now_ms()),
                    name: group_name,
                });
                self.save();
                self.new_group = None;
            }
            Some(None) => {
                self.new_group = None;
            }
            None => {
                self.new_group = Some(name);
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_style(ctx);
        self.poll_net();
        // Drag & drop bookkeeping runs BEFORE rendering, hit-testing
        // against last frame's card geometry.
        if self.drag.is_some() {
            self.update_drag(ctx);
        }
        // A verified installer download: run it, then exit so the
        // per-user installer can replace this app while it isn't running.
        if let Some(path) = self.update_launch.take() {
            match Command::new(&path).spawn() {
                Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Err(e) => {
                    self.update_error = Some(format!(
                        "The installer was downloaded but could not be started: {}",
                        e
                    ));
                }
            }
        }
        let pal = self.pal();
        match self.view {
            View::Login => {
                egui::CentralPanel::default()
                    .frame(Frame::none().fill(pal.bg))
                    .show(ctx, |ui| self.view_login(ui));
            }
            View::Dashboard => {
                self.header(ctx);
                egui::CentralPanel::default()
                    .frame(Frame::none().fill(pal.bg))
                    .show(ctx, |ui| self.view_dashboard(ui));
            }
            View::Settings => {
                egui::TopBottomPanel::top("settings-header")
                    .frame(Frame::none().fill(pal.card).inner_margin(Margin::symmetric(18.0, 12.0)))
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("⚙️").size(22.0));
                            ui.label(RichText::new("Settings").size(18.0).strong());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.add(Button::new("← Back to dashboard")).clicked() {
                                    self.view = View::Dashboard;
                                }
                            });
                        });
                    });
                egui::CentralPanel::default()
                    .frame(Frame::none().fill(pal.bg))
                    .show(ctx, |ui| self.view_settings(ui));
            }
        }
        // Autorefresh — paused while editing, dragging, or typing.
        if self.view == View::Dashboard
            && self.session.is_some()
            && self.cfg.prefs.autorefresh
            && !self.editing
            && !self.dragging_slider
            && !ctx.input(|i| i.pointer.primary_down())
        {
            let due = self
                .last_fetch
                .map(|t| t.elapsed() >= Duration::from_secs(self.cfg.prefs.interval))
                .unwrap_or(false);
            if due && !self.disc_busy {
                self.spawn_discovery(false);
            }
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        if self.new_group.is_some() {
            self.new_group_window(ctx);
        }
        if self.cam_form.is_some() {
            self.camera_window(ctx);
        }
        self.drag_ghost(ctx);
        self.show_toasts(ctx);
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("SmartLife & Tuya Desktop")
            .with_inner_size([1120.0, 780.0])
            .with_min_inner_size([720.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SmartLife & Tuya Desktop",
        options,
        Box::new(|cc| Box::new(App::new(cc))),
    )
}

// ---------------------------------------------------------------------------
// Tests (cargo test): camera URLs, section classification (incl. the
// automatic Lighting / Cameras sections), group fan-out plans, and
// update version/release parsing — mirrors the web edition's harness.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn cam(template: &str, path: &str) -> Camera {
        Camera {
            id: "1".into(),
            name: "Porch".into(),
            user: "tap o".into(),
            password: obfuscate("p@ss:w/rd#?x"),
            host: "192.168.1.50".into(),
            port: 554,
            template: template.into(),
            path: path.into(),
        }
    }

    #[test]
    fn rtsp_url_encodes_credentials() {
        let c = cam("tapo1", "");
        assert_eq!(
            build_rtsp_url(&c),
            "rtsp://tap%20o:p%40ss%3Aw%2Frd%23%3Fx@192.168.1.50:554/stream1"
        );
    }

    #[test]
    fn rtsp_url_templates_and_ports() {
        assert!(build_rtsp_url(&cam("tapo2", "")).ends_with(":554/stream2"));
        let mut c = cam("custom", "live/ch1");
        assert!(build_rtsp_url(&c).ends_with(":554/live/ch1"));
        c.path = "/onvif/stream".into();
        assert!(build_rtsp_url(&c).ends_with(":554/onvif/stream"));
        c.port = 8554;
        assert!(build_rtsp_url(&c).contains(":8554/"));
        c.port = 0; // treated as default
        assert!(build_rtsp_url(&c).contains(":554/"));
    }

    #[test]
    fn gateway_urls() {
        let prefs = Prefs { gateway_url: "http://gw.local:1984/".into(), gateway_type: "go2rtc".into(), ..Default::default() };
        assert_eq!(
            gateway_watch_url(&cam("tapo1", ""), &prefs).as_deref(),
            Some("http://gw.local:1984/stream.html?src=Porch")
        );
        let prefs2 = Prefs { gateway_url: "http://gw.local:8889".into(), gateway_type: "mediamtx".into(), ..Default::default() };
        assert_eq!(
            gateway_watch_url(&cam("tapo1", ""), &prefs2).as_deref(),
            Some("http://gw.local:8889/Porch/index.m3u8")
        );
        let prefs3 = Prefs::default();
        assert_eq!(gateway_watch_url(&cam("tapo1", ""), &prefs3), None);
    }

    fn dev(id: &str, name: &str, data: Value) -> Device {
        parse_device(&json!({ "id": id, "name": name, "dev_type": "light", "data": data }))
    }

    fn bulb(id: &str, name: &str) -> Device {
        dev(id, name, json!({ "state": true, "brightness": 800 }))
    }

    fn layout_with_group() -> LayoutCfg {
        let mut l = LayoutCfg::default();
        l.groups.push(Group { id: "g1".into(), name: "Kitchen".into() });
        l
    }

    fn keys(secs: &[Section]) -> Vec<&str> {
        secs.iter().map(|s| s.key.as_str()).collect()
    }

    #[test]
    fn classification_lighting_cameras_ungrouped() {
        let b = bulb("lb", "Bright bulb");
        let plug = dev("p1", "Plug", json!({ "state": false }));
        let c = cam("tapo1", "");
        let entries = vec![device_entry(&b), device_entry(&plug), camera_entry(&c)];
        let secs = build_sections(&entries, &LayoutCfg::default(), false, "");
        assert_eq!(keys(&secs), vec!["lighting", "cameras", "ungrouped"]);
        assert_eq!(secs[0].ids, vec!["lb".to_string()]);
        assert_eq!(secs[1].ids, vec!["cam:1".to_string()]);
        assert_eq!(secs[2].ids, vec!["p1".to_string()]);
        assert!(device_is_light(&b));
        assert!(!device_is_light(&plug));
        assert_eq!(auto_section_label(&entries[0]), "Lighting (automatic)");
        assert_eq!(auto_section_label(&entries[1]), "Ungrouped");
        assert_eq!(auto_section_label(&entries[2]), "Cameras (automatic)");
    }

    #[test]
    fn classification_assignment_wins() {
        let b = bulb("lb", "Bright bulb");
        let c = cam("tapo1", "");
        let mut layout = layout_with_group();
        layout.assign.insert("lb".into(), "g1".into());
        layout.assign.insert("cam:1".into(), "g1".into());
        let entries = vec![device_entry(&b), camera_entry(&c)];
        let secs = build_sections(&entries, &layout, false, "");
        assert_eq!(keys(&secs), vec!["g1"]);
        assert_eq!(secs[0].ids.len(), 2);
    }

    #[test]
    fn group_plan_fanout() {
        let on_bulb = bulb("lb", "On bulb");
        let temp_bulb = dev("lt", "Temp bulb", json!({ "state": false, "brightness": 400, "color_temp": 29000 }));
        let strip = dev("ls", "RGB strip", json!({ "state": false, "color_mode": "colour", "hue": 10, "saturation": 0.5 }));
        let plug = dev("p1", "Plug", json!({ "state": false }));
        let members = vec![on_bulb.clone(), temp_bulb, strip, plug];
        assert!(group_master_on(&members));
        assert!(!group_master_on(&[members[1].clone()]));

        let plan = group_plan(&members, GroupKind::Toggle, 0, None);
        assert_eq!(plan.len(), 4);
        assert!(plan.iter().all(|(_, a, v)| *a == "turnOnOff" && *v == json!(0)));

        let plan = group_plan(&members, GroupKind::Brightness, 55, None);
        assert_eq!(plan.len(), 2);
        assert!(plan.iter().all(|(_, _, v)| *v == json!(55)));

        let plan = group_plan(&members, GroupKind::Temp, 6000, None);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].0, "lt");

        let plan = group_plan(&members, GroupKind::Color, 0, Some("#00ff00"));
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].0, "ls");
        assert_eq!(plan[0].2["hue"], json!(120));
        assert_eq!(plan[0].2["brightness"], json!(100)); // strip reports no brightness

        let off = dev("lo", "Off bulb", json!({ "state": false, "brightness": 500, "online": false }));
        let plan = group_plan(&[off, on_bulb], GroupKind::Brightness, 70, None);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].0, "lb");

        assert_eq!(group_average(&[80, 40]), Some(60.0));
        assert_eq!(group_average(&[]), None);
    }

    #[test]
    fn version_compare() {
        assert!(version_newer("1.1.0", "1.0.0"));
        assert!(!version_newer("1.0.0", "1.1.0"));
        assert!(!version_newer("1.1.0", "1.1.0"));
        assert!(version_newer("1.10.0", "1.9.0"));
        assert!(version_newer("v1.2.0", "1.1.0"));
        assert_eq!(APP_VERSION, "1.1.0");
    }

    #[test]
    fn release_parse_and_installer_name() {
        let v = json!({
            "tag_name": "v1.2.0",
            "html_url": "https://github.com/x/releases/v1.2.0",
            "assets": [
                { "name": "SmartLife_Tuya_Desktop.html", "browser_download_url": "https://dl/html", "size": 100 },
                { "name": "SmartLifeTuyaDesktop-Setup-v1.2.0.exe", "browser_download_url": "https://dl/exe", "size": 200 }
            ]
        });
        let info = parse_release(&v).expect("release parses");
        assert_eq!(info.version, "1.2.0");
        assert_eq!(info.exe_name, "SmartLifeTuyaDesktop-Setup-v1.2.0.exe");
        assert_eq!(info.exe_name, expected_installer_name(&info.version));
        assert_ne!(
            "SmartLifeTuyaDesktop-Setup-v9.9.9.exe",
            expected_installer_name(&info.version)
        );
        assert!(parse_release(&json!({ "tag_name": "v1.2.0", "html_url": "x", "assets": [] })).is_none());
    }
}
