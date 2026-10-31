#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! Homestead (native desktop editions: Windows &
//! macOS) — a clean dashboard for
//! Smart Life / Tuya devices, 1:1 with the single-file web edition in
//! `web/Homestead.html`. Both talk to Tuya's legacy consumer
//! cloud API ("homeassistant" API):
//!   POST {base}auth.do   — login / token refresh
//!   POST {base}skill     — Discovery (device list) / Control (commands)
//! All settings live in one JSON config file (see `config_path`).
//! The shared design tokens are documented in DESIGN.md — keep them in step.

use eframe::egui;
use egui::{
    Align, Align2, Button, Color32, ComboBox, FontId, Frame, Layout, Margin, RichText, CornerRadius,
    ScrollArea, Sense, Stroke, TextEdit,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod hap;
mod bridge;

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

const R_CARD: u8 = 18;
const R_CARD_COMPACT: u8 = 14;
const R_BTN: u8 = 11;
const R_LOGIN: u8 = 22;

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
// Config (JSON, %APPDATA%\Homestead\config.json on Windows)
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

/// Generate a fresh camera record id. Ids used to be a bare
/// millisecond timestamp (web: Date.now() base36) — two cameras
/// created in the same millisecond (or a clock tie) then SHARED one
/// id, and every id-keyed lookup (edit, delete, layout) hit the
/// first match, making the second camera uneditable and
/// undeletable. Ids are now nanosecond timestamps plus a process-wide
/// monotonic counter, and callers still check the result against the
/// existing cameras before using it.
fn new_camera_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{}", nanos, SEQ.fetch_add(1, Ordering::SeqCst))
}

/// Remove one camera by ITEM id ("cam:{record id}") and scrub its
/// layout entries (pins, hidden, assign, alias, sizes, section
/// order). Returns the removed record, or None when the id matched
/// nothing — callers must not claim a deletion that didn't happen.
fn remove_camera_record(
    cameras: &mut Vec<Camera>,
    layout: &mut LayoutCfg,
    item_id: &str,
) -> Option<Camera> {
    let ci = cameras.iter().position(|c| format!("cam:{}", c.id) == item_id)?;
    let removed = cameras.remove(ci);
    layout.pins.retain(|x| x != item_id);
    layout.hidden.retain(|x| x != item_id);
    layout.assign.remove(item_id);
    layout.alias.remove(item_id);
    layout.cam_sizes.remove(item_id);
    for list in layout.section_order.values_mut() {
        list.retain(|x| x != item_id);
    }
    layout.section_order.retain(|_, list| !list.is_empty());
    Some(removed)
}

/// Repair duplicate / missing camera ids in stored data (v1.2.1
/// migration). The FIRST camera with a given id keeps it — layout
/// entries (pins / assign / order / sizes) are keyed by "cam:{id}",
/// were always attached to that first match, and stay attached.
/// Later duplicates and id-less records get fresh ids from `gen`
/// (called until its result is unused). Returns true if anything
/// changed. `layout` is part of the signature for parity with the
/// web edition's repair (which documents the same keep-first rule);
/// entries are deliberately NOT remapped — they were ambiguous.
fn repair_camera_ids(
    cameras: &mut Vec<Camera>,
    _layout: &LayoutCfg,
    gen: &mut dyn FnMut() -> String,
) -> bool {
    let mut seen: HashSet<String> = HashSet::new();
    let mut changed = false;
    for cam in cameras.iter_mut() {
        if cam.id.trim().is_empty() || !seen.insert(cam.id.clone()) {
            let mut fresh = gen();
            while !seen.insert(fresh.clone()) {
                fresh = gen();
            }
            cam.id = fresh;
            changed = true;
        }
    }
    changed
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
    /// User-arranged order of the SECTIONS themselves (v1.2.1), as
    /// section order_keys: "cameras" | "pinned" | "lighting" |
    /// "group:{gid}" | "ungrouped" | "scenes". Empty/absent = the
    /// built-in default order; a section missing from the list slots
    /// in by that default (see effective_section_seq).
    #[serde(default)]
    section_seq: Vec<String>,
    /// Per-camera card size (v1.2.1), keyed by item id ("cam:{id}"):
    /// "large" | "wide"; absent = normal (one grid cell).
    #[serde(default)]
    cam_sizes: HashMap<String, String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Prefs {
    theme: String,
    accent: String,
    density: String,
    /// Legacy fixed interval (v2.0.5 and earlier): still loaded so
    /// old configs parse, and still drives the thermostat fetch
    /// cadence — but the Tuya discovery timer no longer reads it.
    /// That cadence is the dynamic disc_interval_secs (v2.0.6).
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
    /// Live in-tile camera previews (v1.2.0, ffmpeg frame pump).
    #[serde(default = "default_true")]
    previews: bool,
    /// ffmpeg path override ("" = bundled copy beside the app exe,
    /// then PATH). A full path or a bare program name both work.
    #[serde(default)]
    ffmpeg_path: String,
    /// Start with the OS (v1.2.1): an entry exists in the
    /// platform's per-user autostart store (Windows: the HKCU
    /// Run key; macOS: a LaunchAgent plist, v1.4.0). The store
    /// is the source of truth — this is reconciled against it
    /// at startup (see reconcile_autostart).
    #[serde(default)]
    start_with_windows: bool,
    /// How autostart launches the app: "normal" | "minimized" |
    /// "maximized" — carried as a flag in the Run command.
    #[serde(default = "default_launch_mode")]
    launch_mode: String,
    /// Monitor the window was last on (v1.5.0): the OS device
    /// identity ("\\.\DISPLAY2") of the display the window
    /// most recently sat on, updated whenever the window moves
    /// monitors. IDENTITY only — never geometry — so a restore
    /// can never land a window off-screen (the v1.2.5 placement
    /// memory's failure mode). Read ONLY on a Maximized launch:
    /// the app reopens maximized on this display when it still
    /// exists. "" = never recorded yet.
    #[serde(default)]
    last_monitor_device: String,
    /// Keep running in the system tray when the window is closed
    /// (v1.2.1) instead of quitting.
    #[serde(default)]
    close_to_tray: bool,
    /// The one-time "still running in the tray" balloon has shown.
    #[serde(default)]
    tray_notice_shown: bool,
}

fn default_true() -> bool {
    true
}

fn default_launch_mode() -> String {
    "normal".to_string()
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
            // Fresh-install default theme is Black (v1.3.0); a
            // stored preference always wins — this only applies
            // when no theme has ever been saved.
            theme: "black".into(),
            accent: "#6366f1".into(),
            density: "comfortable".into(),
            interval: 30,
            proxy_preset: "auto".into(),
            proxy_custom: String::new(),
            gateway_url: String::new(),
            gateway_type: default_gateway_type(),
            player_cmd: default_player_cmd(),
            previews: true,
            ffmpeg_path: String::new(),
            start_with_windows: false,
            launch_mode: default_launch_mode(),
            last_monitor_device: String::new(),
            close_to_tray: false,
            tray_notice_shown: false,
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
    /// HomeKit thermostat pairing (native editions only). None when
    /// unpaired; old config files without this field load fine.
    #[serde(default)]
    thermostat: Option<crate::hap::ThermostatPairing>,
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
    } else if cfg!(target_os = "macos") {
        // The native macOS per-user data home (v1.4.0) — a Mac
        // user looks in ~/Library/Application Support, never in
        // ~/.config.
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }
    .unwrap_or_else(|| PathBuf::from("."));
    base.join("Homestead").join("config.json")
}

/// The directory holding config.json — sibling files (the macOS
/// single-instance lock, the show-request signal) live here too.
#[cfg(any(target_os = "macos", test))]
fn config_dir() -> PathBuf {
    config_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn load_config() -> Config {
    let path = config_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(mut cfg) = serde_json::from_str::<Config>(&text) {
            // v1.2.1 migration: cameras stored by older builds can
            // share one timestamp id, which made every camera after
            // the first uneditable/undeletable. Repair in place and
            // persist immediately so the fix sticks.
            let mut gen = new_camera_id;
            if repair_camera_ids(&mut cfg.cameras, &cfg.layout, &mut gen) {
                save_config(&cfg);
            }
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
// - device brightness is reported 0..1000; device % = brightness/10.
//   brightnessSet sends a DEVICE-scale % (see the UI<->device mapping
//   at brightness_ui_to_device: sliders show 1..100, sends run 11..100).
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

/// Neutral colour-temperature slider position (≈4600K, mid-scale)
/// shown when a temp-capable device or group currently has no
/// reading — the control stays, at a sane middle value, instead of
/// snapping to the 2700K floor or vanishing (v1.2.2).
const TEMP_NEUTRAL_SLIDER: i64 = 5500;

fn temp_slider_to_kelvin(slider: i64) -> i64 {
    ((2700.0 + (clampf(slider as f64, 1000.0, 10000.0) - 1000.0) * (3800.0 / 9000.0)) / 10.0).round() as i64 * 10
}

/// The five colour-temperature stops (v1.3.1), in Kelvin —
/// the standard temps from the user's reference chart. Every
/// colour-temperature slider in the app has exactly these
/// detents: its value is a stop INDEX (0..=4), and anything
/// displayed or sent is one of these five values (snapped in
/// Kelvin space, before the API-scale conversion).
const TEMP_STOPS_K: [i64; 5] = [2700, 3000, 4000, 5000, 6500];

/// Index of the stop nearest a Kelvin value (ties go to the
/// lower stop). A device reporting an in-between value (3080K,
/// 3780K, 3970K...) DISPLAYS at the nearest stop — its stored
/// reading is not rewritten until the user actually drags.
fn temp_stop_index(kelvin: i64) -> usize {
    let mut best = 0;
    for (i, &s) in TEMP_STOPS_K.iter().enumerate() {
        if (kelvin - s).abs() < (kelvin - TEMP_STOPS_K[best]).abs() {
            best = i;
        }
    }
    best
}

/// The Kelvin stop at a slider index (clamped to the range).
fn temp_stop_kelvin(index: i64) -> i64 {
    TEMP_STOPS_K[(index.max(0) as usize).min(TEMP_STOPS_K.len() - 1)]
}

/// Kelvin -> the API slider scale (the inverse of
/// temp_slider_to_kelvin): what a colorTemperatureSet sends
/// for an exact stop.
fn temp_kelvin_to_slider(kelvin: i64) -> i64 {
    (1000.0 + (kelvin - 2700) as f64 * 9000.0 / 3800.0).round() as i64
}

/// The Kelvin a temp reading DISPLAYS as: the nearest stop
/// (v1.3.1) — used by the card status line.
fn temp_display_kelvin(temp_slider: i64) -> i64 {
    TEMP_STOPS_K[temp_stop_index(temp_slider_to_kelvin(temp_slider))]
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

/// Per-camera card size (v1.2.1), stored in the layout keyed by
/// item id: Normal = one grid cell, Large = two columns, Wide = the
/// full section row. Cameras only — devices and scenes never resize.
#[derive(Clone, Copy, PartialEq, Debug)]
enum CamSize {
    Normal,
    Large,
    Wide,
}

impl CamSize {
    fn parse(s: Option<&String>) -> CamSize {
        match s.map(|v| v.as_str()) {
            Some("large") => CamSize::Large,
            Some("wide") => CamSize::Wide,
            _ => CamSize::Normal,
        }
    }
    fn as_str(&self) -> &'static str {
        match self {
            CamSize::Normal => "normal",
            CamSize::Large => "large",
            CamSize::Wide => "wide",
        }
    }
    /// Grid columns this size occupies in a `cols`-column grid.
    fn span(&self, cols: usize) -> usize {
        let want = match self {
            CamSize::Normal => 1,
            CamSize::Large => 2,
            CamSize::Wide => cols,
        };
        want.max(1).min(cols.max(1))
    }
}

fn cam_size_of(layout: &LayoutCfg, item_id: &str) -> CamSize {
    CamSize::parse(layout.cam_sizes.get(item_id))
}

/// A tile feed spec: (RTSP URL, pump frame width, pump frame
/// height). Normal and Large tiles decode the cheap Stream 2 on
/// Tapo templates (the source selection is unchanged by size);
/// only the pump resolution grows (480×270 → 960×540). A Wide tile
/// uses the card's own selected quality, like the big player.

fn tile_feed(cam: &Camera, size: CamSize) -> (String, usize, usize) {
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
fn tile_rtsp_url(cam: &Camera) -> String {
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

/// File name to look for on disk for a bare player name. The
/// ".exe" suffix is a Windows convention; elsewhere the bare
/// name is already the file name.
fn player_exe_name(name: &str) -> String {
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
fn player_candidates(
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
fn parse_reg_default_value(output: &str) -> Option<String> {
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
fn app_paths_lookup(name: &str) -> Option<PathBuf> {
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
fn app_paths_lookup(_name: &str) -> Option<PathBuf> {
    None
}

// ---------------------------------------------------------------------------
// Startup & system tray (v1.2.1 on Windows; v1.4.0 adds macOS
// with the native mechanisms — a LaunchAgent for start-at-login,
// the menu-bar tray). Starting with the OS, running minimized
// to the tray and closing to the tray are system behaviours a
// web page simply doesn't have; the web edition is unchanged
// and DESIGN.md documents the differences.
// ---------------------------------------------------------------------------

/// The per-user autostart (Run) key + this app's value name in it.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE_NAME: &str = "Homestead";

/// How the app presents itself at launch. Carried as a flag in the
/// autostart command ("--minimized" / "--maximized") and honoured
/// for manual launches (shortcuts) too.
#[derive(Clone, Copy, PartialEq, Debug)]
enum LaunchMode {
    Normal,
    Minimized,
    Maximized,
}

impl LaunchMode {
    fn parse(s: &str) -> LaunchMode {
        match s {
            "minimized" => LaunchMode::Minimized,
            "maximized" => LaunchMode::Maximized,
            _ => LaunchMode::Normal,
        }
    }
    fn as_str(&self) -> &'static str {
        match self {
            LaunchMode::Normal => "normal",
            LaunchMode::Minimized => "minimized",
            LaunchMode::Maximized => "maximized",
        }
    }
    fn flag(&self) -> Option<&'static str> {
        match self {
            LaunchMode::Normal => None,
            LaunchMode::Minimized => Some("--minimized"),
            LaunchMode::Maximized => Some("--maximized"),
        }
    }
}

/// Launch mode requested on the command line. If both flags sneak
/// in, --minimized wins (the more conservative presentation).
fn launch_mode_from_args(args: &[String]) -> LaunchMode {
    if args.iter().any(|a| a == "--minimized") {
        LaunchMode::Minimized
    } else if args.iter().any(|a| a == "--maximized") {
        LaunchMode::Maximized
    } else {
        LaunchMode::Normal
    }
}

/// Launch mode encoded in a stored autostart command string.
fn launch_mode_from_command(cmd: &str) -> LaunchMode {
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
fn resolve_launch_mode(argv: LaunchMode, prefs: LaunchMode) -> LaunchMode {
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
fn autostart_command(exe: &Path, mode: LaunchMode) -> String {
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
fn desired_autostart(prefs: &Prefs, exe: &Path) -> Option<String> {
    if prefs.start_with_windows {
        Some(autostart_command(exe, LaunchMode::parse(&prefs.launch_mode)))
    } else {
        None
    }
}

/// Startup & background panel wording (v1.4.0): the feature is
/// the same on both desktop platforms; the OS nouns are not.
#[cfg(target_os = "windows")]
const AUTOSTART_LABEL: &str = "Start with Windows";
#[cfg(not(target_os = "windows"))]
const AUTOSTART_LABEL: &str = "Start at login";

#[cfg(target_os = "windows")]
const CLOSE_TO_TRAY_LABEL: &str =
    "Keep running in the system tray when the window is closed";
#[cfg(not(target_os = "windows"))]
const CLOSE_TO_TRAY_LABEL: &str =
    "Keep running in the menu bar when the window is closed";

#[cfg(not(target_os = "macos"))]
const AUTOSTART_HELP: &str = "Start with Windows uses the current-user registry Run key — no administrator rights needed, and OFF means no autostart entry exists at all. The “Launch as” choice applies to every launch of the app — autostart, Start Menu, and shortcuts alike — not just the Windows-startup one; a launch that carries its own --minimized or --maximized flag (as the autostart entry does) overrides it for that launch only. “Minimized (to tray)” starts the app hidden: its icon sits in the notification area by the Windows clock, and clicking the icon (or “Open” on its right-click menu) brings the window back. With “Keep running in the system tray” on, closing the window hides it instead of quitting — device refresh and camera previews keep running in the background — and Quit lives on the tray icon's right-click menu. The tray icon appears when the app starts minimized or while close-to-tray is on, and then stays for the session. Logging out doesn't quit the app either way. When “Maximized” is chosen, the app reopens maximized on the monitor it was last on; if that display is no longer connected, it opens maximized where Windows puts it. This panel is a Windows-app feature; the web edition has no equivalent.";

#[cfg(target_os = "macos")]
const AUTOSTART_HELP: &str = "Start at login uses a per-user LaunchAgent in ~/Library/LaunchAgents — no administrator rights needed, and OFF means no LaunchAgent file exists at all. The “Launch as” choice applies to every launch of the app — login, Finder, and Dock alike — not just the login one; a launch that carries its own --minimized or --maximized flag (as the LaunchAgent entry does) overrides it for that launch only. “Minimized (to tray)” starts the app hidden: its icon sits in the menu bar at the top of the screen, and clicking the icon (or “Open” on its menu) brings the window back. With “Keep running in the menu bar” on, closing the window hides it instead of quitting — device refresh and camera previews keep running in the background — and Quit lives on the icon's menu. The icon appears when the app starts minimized or while close-to-tray is on, and then stays for the session. Logging out doesn't quit the app either way. This panel is a native-app feature; the web edition has no equivalent.";

// ---------------------------------------------------------------------------
// Snap to monitor (v1.3.0; mechanism replaced in v1.3.1): a
// header button lists the displays the OS reports at that
// moment; choosing one moves the window onto that display and
// maximizes it there. Click-time only, by design — nothing is
// persisted, and startup placement is never affected (the
// v1.2.5 placement memory stays removed).
//
// v1.3.0 moved the window with a one-frame burst of egui
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

/// One display, as enumerated from the OS when the menu opens.
#[derive(Clone, PartialEq, Debug)]
struct MonitorInfo {
    /// Friendly name when the OS offers one, else the device
    /// name ("DISPLAY1").
    name: String,
    /// OS device identity ("\\.\DISPLAY1") — the match key when
    /// a choice is re-validated at click time.
    device: String,
    /// The HMONITOR value (v1.3.1) — the identity a completed
    /// move is verified against (MonitorFromWindow).
    handle: isize,
    /// Physical-pixel rect [x, y, w, h] in virtual-desktop
    /// coordinates (origins can be negative).
    rect_px: [i32; 4],
    /// THIS monitor's scale factor (its effective DPI / 96).
    scale: f32,
    primary: bool,
}

/// A monitor rect in egui points ([x, y, w, h]), converted with
/// the monitor's OWN scale factor. Used ONLY for the menu's
/// "this window is here" marker, which compares against the
/// window centre egui reports — positions egui itself reports
/// live in this same per-monitor points space, so the marker
/// is consistent. (The snap MOVE never uses points: v1.3.1,
/// see snap_target_point_px.)
fn monitor_rect_points(m: &MonitorInfo) -> [f32; 4] {
    let s = if m.scale > 0.0 { m.scale } else { 1.0 };
    [
        m.rect_px[0] as f32 / s,
        m.rect_px[1] as f32 / s,
        m.rect_px[2] as f32 / s,
        m.rect_px[3] as f32 / s,
    ]
}

fn monitor_orientation(m: &MonitorInfo) -> &'static str {
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
fn snap_target_point_px(m: &MonitorInfo, win_px: [i32; 2]) -> [i32; 2] {
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
fn snap_axis(origin: f32, len: f32, win_len: f32, inset: f32, margin: f32) -> f32 {
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

/// Which monitor a point (egui points, virtual-desktop
/// coordinates) falls on — used to mark the display the window
/// currently sits on (by its centre point).
fn monitor_index_at(monitors: &[MonitorInfo], point: [f32; 2]) -> Option<usize> {
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
fn enumerate_monitors() -> Vec<MonitorInfo> {
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
fn enumerate_monitors() -> Vec<MonitorInfo> {
    Vec::new()
}

/// Snap phases (v1.3.1): the move is a small state machine
/// advanced one step per frame from handle_window_chrome, so
/// each window-state transition settles before the next step —
/// never one blind burst of commands.
#[derive(Clone, Copy, PartialEq, Debug)]
enum SnapPhase {
    /// Leave the states other layers own first: fullscreen is
    /// winit's (released via a viewport command), the maximized
    /// / minimized show state is released with a Win32 restore.
    LeaveStates,
    /// Issue the physical-pixel move (SetWindowPos).
    Move,
    /// Confirm arrival (MonitorFromWindow == the target) before
    /// maximizing; a miss retries the restore+move once.
    Verify,
}

/// What one snap step should do this frame.
#[derive(Clone, Copy, PartialEq, Debug)]
enum SnapAction {
    SendFullscreenOff,
    Restore,
    Wait,
    MoveWindow,
    MaximizeAndFinish,
    Fail,
}

/// The facts the snap decision needs, gathered fresh per frame.
#[derive(Clone, Copy, Default)]
struct SnapFacts {
    /// egui reports the window in (borderless) fullscreen.
    fullscreen: bool,
    /// The window is maximized or minimized (needs a restore
    /// before it can be moved meaningfully).
    zoomed: bool,
    /// MonitorFromWindow says the window is on the target.
    on_target: bool,
    /// The job's overall time budget has run out.
    expired: bool,
}

/// Move attempts per snap: the initial move plus ONE retry.
const SNAP_MAX_MOVES: u8 = 2;
/// Overall budget for one snap. Past it the job fails safe:
/// the window stays wherever it verifiably is — visible, on a
/// real monitor (every move targets a clamped interior point),
/// never wedged mid-transition.
const SNAP_TIMEOUT: Duration = Duration::from_secs(3);

/// Launch-window self-check budget (v2.0.1): the window after
/// the first frame during which a Maximized launch is
/// re-verified against its remembered monitor. Covers an initial
/// snap (≤ SNAP_TIMEOUT) plus a settle pause plus one re-armed
/// snap (≤ SNAP_TIMEOUT) with room to spare; past it the check
/// stops permanently for the session and the window stays
/// wherever it verifiably is.
const STARTUP_CHECK_WINDOW: Duration = Duration::from_secs(10);
/// Settle pause after a snap job ends before the self-check may
/// act on a wrong-monitor reading: a just-maximized window can
/// transiently straddle monitors while the OS animation lands,
/// and spending the one re-arm on that transient would waste it.
const STARTUP_CHECK_SETTLE: Duration = Duration::from_millis(500);

/// The pure snap decision: (next phase, this frame's action).
/// Arrival beats expiry — a window that just landed is
/// maximized, not failed. Kept free of Win32 so the whole
/// sequence is unit-testable headlessly.
fn snap_next(phase: SnapPhase, f: SnapFacts, attempts: u8) -> (SnapPhase, SnapAction) {
    use SnapAction::*;
    use SnapPhase::*;
    if phase == Verify && f.on_target {
        return (Verify, MaximizeAndFinish);
    }
    if f.expired {
        return (phase, Fail);
    }
    match phase {
        LeaveStates => {
            if f.fullscreen {
                (LeaveStates, SendFullscreenOff)
            } else if f.zoomed {
                (Move, Restore)
            } else {
                (Move, Wait)
            }
        }
        Move => (Verify, MoveWindow),
        Verify => {
            if attempts < SNAP_MAX_MOVES {
                (Move, Restore)
            } else {
                (Verify, Fail)
            }
        }
    }
}

/// What the launch-window self-check should do this frame.
/// Kept free of Win32 so the whole decision table is
/// unit-testable headlessly, like snap_next.
#[derive(Clone, Copy, PartialEq, Debug)]
enum StartupCheckAction {
    /// A snap is in flight, or the window is settling after one:
    /// observe only, decide nothing.
    Wait,
    /// The window is on the intended monitor: the check is done.
    Resolve,
    /// The window was maximized, then left that state with no
    /// snap in flight: the user took over — stop permanently,
    /// with no unresolved latch (a deliberate move is never
    /// recorded as a failed landing).
    StopUserTakeover,
    /// The launch window ran out with the window still off the
    /// intended monitor: stop permanently; the caller latches
    /// the monitor it expired on so the remember side does not
    /// overwrite the intended monitor with the failed landing.
    ExpireUnresolved,
    /// Off the intended monitor, settled, re-arm still unused:
    /// arm one more silent snap.
    ArmSnap,
}

/// The facts the self-check decision needs, gathered fresh per
/// frame by the Win32 wrapper.
#[derive(Clone, Copy, Default)]
struct StartupCheckFacts {
    /// A snap job is in flight this frame.
    snap_active: bool,
    /// The launch window has run out.
    expired: bool,
    /// The window is maximized (IsZoomed) this frame.
    maximized: bool,
    /// MonitorFromWindow names the intended monitor.
    on_target: bool,
    /// The post-snap settle pause has passed.
    settle_passed: bool,
    /// The window's top-left moved since the previous check
    /// frame (a move with no snap in flight means something
    /// other than the check moved it).
    moved: bool,
}

/// Pure launch self-check decision (v2.0.1): arrival beats
/// everything (a window on target resolves even on the deadline
/// frame); a user takeover beats expiry (a deliberate move must
/// never be latched as a failed landing, and must never spend
/// the re-arm); the one re-arm is spent only on a settled,
/// off-target reading. A snap-failure restored state looks
/// exactly like a user takeover, so it is exempted from the
/// takeover rule via last_snap_failed — unless the window
/// actually moved since (a drag after a failed snap is still a
/// takeover, and must never be fought).
fn startup_check_next(
    f: StartupCheckFacts,
    rearm_used: bool,
    seen_maximized: bool,
    last_snap_failed: bool,
) -> (StartupCheckAction, bool) {
    use StartupCheckAction::*;
    let seen_maximized = seen_maximized || f.maximized;
    if f.snap_active {
        return (Wait, seen_maximized);
    }
    if f.on_target {
        return (Resolve, seen_maximized);
    }
    if !f.settle_passed {
        return (Wait, seen_maximized);
    }
    if seen_maximized && !f.maximized && (!last_snap_failed || f.moved) {
        return (StopUserTakeover, seen_maximized);
    }
    if f.expired {
        return (ExpireUnresolved, seen_maximized);
    }
    if !rearm_used {
        return (ArmSnap, seen_maximized);
    }
    (Wait, seen_maximized)
}

/// An in-flight snap (v1.3.1): created by a Monitor-menu click,
/// advanced per frame by App::advance_snap, dropped on success,
/// failure or timeout. Never persisted.
struct SnapJob {
    /// Silent job (v1.5.0): a startup monitor restore, not a
    /// menu click — it finishes and fails without toasts.
    silent: bool,
    target: MonitorInfo,
    phase: SnapPhase,
    attempts: u8,
    deadline: Instant,
}

/// Launch-window self-check state (v2.0.1, Windows): the
/// maximized launch is re-verified against its remembered
/// monitor for a bounded window after the first frame. Grouped
/// so the App struct carries one field for the whole check.
/// Identity only throughout — the remembered device string and
/// the intended monitor's handle — never geometry.
#[derive(Clone)]
struct StartupCheckState {
    /// The check is still running (cleared on resolve, user
    /// takeover, or window expiry — never re-armed).
    active: bool,
    /// First frame + STARTUP_CHECK_WINDOW.
    deadline: Instant,
    /// Remembered device identity the check verifies against.
    device: String,
    /// HMONITOR of the intended monitor once resolved from the
    /// device identity (0 = not resolved yet).
    target: isize,
    /// The check's one re-arm has been spent.
    rearm_used: bool,
    /// The window has been observed maximized during the check
    /// (arms the user-takeover rule).
    seen_maximized: bool,
    /// A snap job was in flight on the previous check frame —
    /// the transition to none starts the settle pause.
    prev_snap: bool,
    /// End of the post-snap settle pause (None = no pause).
    settle_until: Option<Instant>,
    /// The window's top-left on the previous check frame (the
    /// user-move signal; None = not observed yet).
    last_pos: Option<[i32; 2]>,
    /// The check expired with the window still off target (see
    /// the remember-side latch below).
    unresolved: bool,
    /// The monitor the window expired on (the failed landing).
    unresolved_monitor: isize,
    /// The last silent startup snap ended in failure (set by
    /// advance_snap for silent jobs only): distinguishes a
    /// snap-failure restored state from a user takeover.
    last_snap_failed: bool,
}

impl Default for StartupCheckState {
    fn default() -> Self {
        Self {
            active: false,
            deadline: Instant::now(),
            device: String::new(),
            target: 0,
            rearm_used: false,
            seen_maximized: false,
            prev_snap: false,
            settle_until: None,
            last_pos: None,
            unresolved: false,
            unresolved_monitor: 0,
            last_snap_failed: false,
        }
    }
}

// ----------------------------------------------------------------
// Direct Win32 window ops for the snap (v1.3.1). The tray code
// already manipulates this same HWND directly (ShowWindowAsync
// from the tray thread, v1.2.3); the snap runs on the UI
// thread — the window's own thread — so the synchronous forms
// are the right ones here: each call has fully landed when it
// returns. Non-Windows builds get inert stubs (enumeration is
// empty there, so no snap job can ever start).
// ----------------------------------------------------------------

/// Maximized (IsZoomed) or minimized (IsIconic) — either means
/// a restore is owed before a move.
#[cfg(target_os = "windows")]
fn win32_needs_restore(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsZoomed};
    unsafe { IsZoomed(hwnd as _) != 0 || IsIconic(hwnd as _) != 0 }
}

#[cfg(not(target_os = "windows"))]
fn win32_needs_restore(_hwnd: isize) -> bool {
    false
}

/// Maximized only (IsZoomed) — the self-check's user-takeover
/// signal, distinct from win32_needs_restore (which also counts
/// minimized).
#[cfg(target_os = "windows")]
fn win32_is_maximized(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsZoomed;
    unsafe { IsZoomed(hwnd as _) != 0 }
}

#[cfg(not(target_os = "windows"))]
fn win32_is_maximized(_hwnd: isize) -> bool {
    false
}

#[cfg(target_os = "windows")]
fn win32_restore_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_RESTORE};
    unsafe {
        ShowWindow(hwnd as _, SW_RESTORE);
    }
}

#[cfg(not(target_os = "windows"))]
fn win32_restore_window(_hwnd: isize) {}

#[cfg(target_os = "windows")]
fn win32_maximize_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_MAXIMIZE};
    unsafe {
        ShowWindow(hwnd as _, SW_MAXIMIZE);
    }
}

#[cfg(not(target_os = "windows"))]
fn win32_maximize_window(_hwnd: isize) {}

/// Move the window's top-left to a PHYSICAL-pixel point,
/// keeping its size and z-order (SetWindowPos).
#[cfg(target_os = "windows")]
fn win32_move_window(hwnd: isize, x: i32, y: i32) {
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
fn win32_move_window(_hwnd: isize, _x: i32, _y: i32) {}

/// The monitor the window is on right now (HMONITOR as isize;
/// MONITOR_DEFAULTTONEAREST always names one for a real
/// window) — the arrival check a move is verified against.
#[cfg(target_os = "windows")]
fn win32_window_monitor(hwnd: isize) -> isize {
    use windows_sys::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
    unsafe { MonitorFromWindow(hwnd as _, MONITOR_DEFAULTTONEAREST) as isize }
}

#[cfg(not(target_os = "windows"))]
fn win32_window_monitor(_hwnd: isize) -> isize {
    0
}

/// The window's current size in PHYSICAL pixels (GetWindowRect)
/// — the size the snap target clamp keeps inside the monitor.
#[cfg(target_os = "windows")]
fn win32_window_size_px(hwnd: isize) -> [i32; 2] {
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
fn win32_window_size_px(_hwnd: isize) -> [i32; 2] {
    [1120, 780]
}

/// The window's top-left in PHYSICAL pixels (GetWindowRect) —
/// the self-check's user-move signal: a top-left change with no
/// snap in flight means something other than the check moved it.
#[cfg(target_os = "windows")]
fn win32_window_pos_px(hwnd: isize) -> [i32; 2] {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetWindowRect(hwnd as _, &mut rc) } != 0 {
        [rc.left, rc.top]
    } else {
        [0, 0]
    }
}

#[cfg(not(target_os = "windows"))]
fn win32_window_pos_px(_hwnd: isize) -> [i32; 2] {
    [0, 0]
}

/// Parse a NAMED REG_SZ value out of `reg query` output (the App
/// Paths parser reads the "(Default)" value; the Run key's values
/// are named, so the name must head the line).
fn parse_reg_named_value(output: &str, name: &str) -> Option<String> {
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

/// What a window-close request should do: quit outright, or hide
/// to the tray (only when close-to-tray is on and the close isn't
/// the app's own Quit).
#[derive(Clone, Copy, PartialEq, Debug)]
enum CloseAction {
    Quit,
    HideToTray,
}

fn close_action(close_to_tray: bool, quitting: bool) -> CloseAction {
    if close_to_tray && !quitting {
        CloseAction::HideToTray
    } else {
        CloseAction::Quit
    }
}

// ---------------------------------------------------------------------------
// Single instance (v1.2.2, Windows) — v1.2.1 had NO instance guard:
// closing to the tray hid the process, and every later Start Menu /
// shortcut launch started ANOTHER process — each with its own tray
// icon (the user's "duplicating icon") and its own set of camera
// pumps competing for the cameras' ~2 RTSP sessions. Now the first
// process holds a named mutex; a later launch just signals the
// primary's named event ("show your window") and exits before it
// creates a window, an icon, or a single pump. Stale state is a
// non-issue: the kernel releases a mutex when its owner process
// dies, however it dies.
// ---------------------------------------------------------------------------

/// Kernel object names for the instance guard and the show signal.
const SINGLE_INSTANCE_MUTEX: &str = "Homestead.SingleInstance";
const SHOW_WINDOW_EVENT: &str = "Homestead.ShowWindow";

/// Set by the primary's waiter thread when a secondary launch asks
/// it to show its window; consumed by handle_window_chrome.
static SHOW_REQUESTED: AtomicBool = AtomicBool::new(false);

/// The egui context, registered by App::new (Windows builds) so the
/// waiter thread can poke the event loop awake — a bare flag could
/// otherwise wait for a frame a sleeping hidden window never runs.
#[cfg(target_os = "windows")]
static WAKE_CTX: Mutex<Option<egui::Context>> = Mutex::new(None);

/// v1.2.3 (Bug C) tray architecture. Field report on v1.2.2: with
/// the window hidden in the tray, "Quit only works when the Window
/// is open and open never works at all" — although the menu
/// rendered and the event plumbing looked right. The code facts,
/// verified against the vendored crates: muda 0.15's
/// MenuEvent::receiver() and tray-icon 0.19's TrayIconEvent::
/// receiver() are two SEPARATE crossbeam channels; the app drains
/// BOTH, but only from App::update (via handle_window_chrome).
/// Menu ids are captured from the created items and compared by
/// equality — correct. The handler's actions were already
/// unconditional. The starving layer is the frame loop itself:
/// eframe 0.27 schedules repaints with winit's request_redraw,
/// which on Windows is RedrawWindow(RDW_INTERNALPAINT) — an
/// invisible window never turns that into a redraw, so while
/// hidden NO frame runs, the 200ms repaint heartbeat never
/// fires, and the channels are never drained. (Quit "worked when
/// open" because frames run while visible; Open could only ever
/// be tested from hidden, where it starved. The single-instance
/// ShowWindow poke had the same flaw: it sets a flag + requests
/// a repaint nobody delivers.)
///
/// The fix removes the frame loop from the critical path:
///   • a dedicated tray thread OWNS both receivers, maps events
///     to TrayActions through the pure functions below, and
///     pushes them onto TRAY_QUEUE;
///   • it pokes WAKE_CTX like before AND acts directly in Win32
///     (ShowWindowAsync on MAIN_HWND): Show restores the window
///     immediately — no frame needed — and Quit makes the window
///     visible so the frame pipeline wakes and the UI thread can
///     run the REAL quit path from the queue (one-frame flash of
///     the window on Quit-from-tray is the accepted tradeoff);
///   • the UI thread drains the queue in handle_window_chrome
///     and applies the actions through the same unconditional
///     show_from_tray / quit_now paths as before.
/// The single-instance waiter routes through the same queue +
/// direct restore for the same reason.
static TRAY_QUEUE: Mutex<std::collections::VecDeque<TrayAction>> =
    Mutex::new(std::collections::VecDeque::new());

fn push_tray_action(a: TrayAction) {
    if let Ok(mut q) = TRAY_QUEUE.lock() {
        q.push_back(a);
    }
}

fn drain_tray_actions() -> Vec<TrayAction> {
    match TRAY_QUEUE.lock() {
        Ok(mut q) => q.drain(..).collect(),
        Err(_) => Vec::new(),
    }
}

/// The main window's HWND, republished by handle_window_chrome
/// every frame so the tray thread can act on it directly.
#[cfg(target_os = "windows")]
static MAIN_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Direct Win32 window show, callable from any thread: restore
/// (un-minimize + foreground) for Show; plain show for Quit (the
/// point is only to wake the frame pipeline so the UI thread
/// executes the real quit path from the queue).
#[cfg(target_os = "windows")]
fn win32_show_main_window(restore: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetForegroundWindow, ShowWindowAsync, SW_RESTORE, SW_SHOW,
    };
    let hwnd = MAIN_HWND.load(Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    unsafe {
        if restore {
            ShowWindowAsync(hwnd as _, SW_RESTORE);
            SetForegroundWindow(hwnd as _);
        } else {
            ShowWindowAsync(hwnd as _, SW_SHOW);
        }
    }
}

/// Pure event→action mapping for tray MENU events (muda channel):
/// the Open item shows, the Quit item quits, anything else is not
/// ours. No state of any kind gates the mapping.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn tray_action_for_menu(
    id: &tray_icon::menu::MenuId,
    open_id: &tray_icon::menu::MenuId,
    quit_id: &tray_icon::menu::MenuId,
) -> Option<TrayAction> {
    if id == open_id {
        Some(TrayAction::Show)
    } else if id == quit_id {
        Some(TrayAction::Quit)
    } else {
        None
    }
}

/// Pure event→action mapping for tray ICON events, reduced to
/// primitives so it is unit-testable without constructing
/// tray-icon's event types: a left-button release shows (the
/// click is only complete on release), a left double-click
/// shows; everything else is ignored — the same mapping
/// drain_tray_events used pre-v1.2.3.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn tray_action_for_icon_parts(double_click: bool, left_button: bool, button_up: bool) -> Option<TrayAction> {
    if (double_click && left_button) || (left_button && button_up) {
        Some(TrayAction::Show)
    } else {
        None
    }
}

/// Hand an action to the UI thread AND do the part a sleeping
/// frame loop can't be trusted with (v1.2.3): wake + direct
/// Win32. Called only from background threads (tray thread,
/// single-instance waiter).
#[cfg(target_os = "windows")]
fn dispatch_tray_action(a: TrayAction) {
    push_tray_action(a);
    if let Ok(slot) = WAKE_CTX.lock() {
        if let Some(ctx) = &*slot {
            ctx.request_repaint();
        }
    }
    win32_show_main_window(matches!(a, TrayAction::Show));
}

/// Spawn the dedicated tray thread exactly once (v1.2.3): it owns
/// both event receivers so tray events are consumed even when no
/// egui frame ever runs. The receivers are 'static crossbeam
/// receivers; the loop blocks briefly on the menu channel and
/// drains the icon channel each pass.
#[cfg(target_os = "windows")]
fn spawn_tray_thread(open_id: tray_icon::menu::MenuId, quit_id: tray_icon::menu::MenuId) {
    use std::sync::atomic::AtomicBool as AB;
    static SPAWNED: AB = AB::new(false);
    if SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("tray-events".to_string())
        .spawn(move || {
            let menu_rx = tray_icon::menu::MenuEvent::receiver();
            let icon_rx = tray_icon::TrayIconEvent::receiver();
            loop {
                match menu_rx.recv_timeout(std::time::Duration::from_millis(40)) {
                    Ok(ev) => {
                        if let Some(a) = tray_action_for_menu(ev.id(), &open_id, &quit_id) {
                            dispatch_tray_action(a);
                        }
                    }
                    Err(_) => {}
                }
                while let Ok(ev) = menu_rx.try_recv() {
                    if let Some(a) = tray_action_for_menu(ev.id(), &open_id, &quit_id) {
                        dispatch_tray_action(a);
                    }
                }
                while let Ok(ev) = icon_rx.try_recv() {
                    use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
                    let (dbl, left, up) = match &ev {
                        TrayIconEvent::DoubleClick { button, .. } => {
                            (true, *button == MouseButton::Left, false)
                        }
                        TrayIconEvent::Click {
                            button,
                            button_state,
                            ..
                        } => (
                            false,
                            *button == MouseButton::Left,
                            *button_state == MouseButtonState::Up,
                        ),
                        _ => (false, false, false),
                    };
                    if let Some(a) = tray_action_for_icon_parts(dbl, left, up) {
                        dispatch_tray_action(a);
                    }
                }
            }
        })
        .ok();
}

#[cfg(target_os = "windows")]
fn maybe_spawn_tray_thread(tray: &TrayState) {
    spawn_tray_thread(tray.open_id.clone(), tray.quit_id.clone());
}

#[cfg(not(target_os = "windows"))]
fn maybe_spawn_tray_thread(_tray: &TrayState) {}

#[derive(Clone, Copy, PartialEq, Debug)]
enum InstanceRole {
    Primary,
    Secondary,
}

/// The role follows from the mutex creation result alone: the name
/// already existed (ERROR_ALREADY_EXISTS) → another process is the
/// primary and this one is a secondary launcher.
fn instance_role(already_exists: bool) -> InstanceRole {
    if already_exists {
        InstanceRole::Secondary
    } else {
        InstanceRole::Primary
    }
}

#[cfg(target_os = "windows")]
fn wide_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A kernel HANDLE riding to the waiter thread (HANDLE is a raw
/// pointer; the handle itself is process-wide and thread-safe to
/// wait on).
#[cfg(target_os = "windows")]
struct SendHandle(usize);
#[cfg(target_os = "windows")]
unsafe impl Send for SendHandle {}

/// The primary's kernel objects. Dropping closes the handles,
/// releasing the mutex for the next launch.
#[cfg(target_os = "windows")]
struct InstanceGuard {
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

/// Acquire the single-instance guard (Windows). The EVENT is
/// created before the MUTEX, so whichever process wins the mutex
/// has necessarily created the event already — a secondary can
/// therefore always signal it. A primary spawns the waiter thread
/// that flips SHOW_REQUESTED (+ a repaint poke) per signal.
#[cfg(target_os = "windows")]
fn acquire_single_instance() -> (InstanceRole, Option<InstanceGuard>) {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject, INFINITE,
    };
    unsafe {
        let ev_name = wide_z(SHOW_WINDOW_EVENT);
        let event = CreateEventW(std::ptr::null(), 0, 0, ev_name.as_ptr());
        let mu_name = wide_z(SINGLE_INSTANCE_MUTEX);
        let mutex = CreateMutexW(std::ptr::null(), 1, mu_name.as_ptr());
        if mutex.is_null() {
            // Couldn't even ask — never block the launch itself.
            if !event.is_null() {
                CloseHandle(event);
            }
            return (InstanceRole::Primary, None);
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // Secondary: ask the primary to show itself, then let
            // main exit BEFORE any window / tray icon / pump exists.
            if !event.is_null() {
                SetEvent(event);
                CloseHandle(event);
            }
            CloseHandle(mutex);
            return (InstanceRole::Secondary, None);
        }
        if !event.is_null() {
            let h = SendHandle(event as usize);
            std::thread::spawn(move || loop {
                // WAIT_OBJECT_0 == 0; anything else ends the watch.
                if WaitForSingleObject(h.0 as _, INFINITE) != 0 {
                    break;
                }
                SHOW_REQUESTED.store(true, Ordering::SeqCst);
                // v1.2.3: route through the same tray dispatch —
                // a flag + repaint request alone starves while the
                // window is hidden (see the Bug C note at
                // TRAY_QUEUE); the dispatch also restores the
                // window directly in Win32.
                dispatch_tray_action(TrayAction::Show);
            });
        }
        (
            InstanceRole::Primary,
            Some(InstanceGuard {
                _mutex: mutex as usize,
                _event: event as usize,
            }),
        )
    }
}

// ----------------------------------------------------------------
// Single instance (v1.4.0, macOS) — the same contract as the
// Windows mutex guard above, with native mechanisms: the
// primary holds an exclusive flock (fs2) on instance.lock in
// the config dir for its whole lifetime; the OS releases it
// when the process dies, however it dies, so a stale lock is
// impossible. A secondary can't signal through a kernel event
// (there is none), so it touches a show-request file the
// primary polls from its frame loop (throttled — the hidden
// 200 ms heartbeat guarantees frames while it matters) and
// exits before creating a window, an icon or a pump.
// ----------------------------------------------------------------

/// Try to take the instance lock at `path`, creating the file
/// if needed. Some(file) = the lock is held (keep the File
/// alive — dropping it releases the lock); None = another
/// process holds it, or the file couldn't be created at all
/// (the caller decides which; see acquire_single_instance).
#[cfg(any(target_os = "macos", all(test, unix)))]
fn try_lock_instance_file(path: &Path) -> std::io::Result<Option<std::fs::File>> {
    use fs2::FileExt;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(path)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(_) => Ok(None),
    }
}

/// The macOS instance guard: just the locked file. Dropping it
/// closes the fd, which releases the flock.
#[cfg(target_os = "macos")]
struct FileInstanceGuard {
    _file: std::fs::File,
}

/// The show-request signal file (macOS single instance): a
/// secondary launch creates it; the primary polls for it,
/// consumes it and shows its window.
#[cfg(target_os = "macos")]
fn show_request_path() -> PathBuf {
    config_dir().join("show-request")
}

#[cfg(target_os = "macos")]
fn acquire_single_instance() -> (InstanceRole, Option<FileInstanceGuard>) {
    let dir = config_dir();
    let _ = std::fs::create_dir_all(&dir);
    match try_lock_instance_file(&dir.join("instance.lock")) {
        Ok(Some(file)) => (InstanceRole::Primary, Some(FileInstanceGuard { _file: file })),
        Ok(None) => {
            // Secondary: ask the primary to show itself, then
            // let main exit BEFORE any window / tray icon /
            // pump exists — the Windows contract, via a file.
            let _ = std::fs::write(show_request_path(), b"show");
            (InstanceRole::Secondary, None)
        }
        // Couldn't even create the lock file — never block the
        // launch itself (same rule as the Windows path).
        Err(_) => (InstanceRole::Primary, None),
    }
}

#[cfg(target_os = "windows")]
fn reg_command() -> Command {
    let mut cmd = Command::new("reg");
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

// ----------------------------------------------------------------
// macOS autostart (v1.4.0): a per-user LaunchAgent plist is the
// native equivalent of the Windows Run key — launchd runs the
// program at login while the file exists in ~/Library/
// LaunchAgents, and removing the file removes the entry
// completely (no launchctl bookkeeping needed for agents that
// only carry RunAtLoad). The conversion helpers are pure
// (command <-> plist), so the round-trip is unit-tested on any
// host; the reg_* macOS impls below just read/write the file.
// reg_run_value reports the stored command in the same
// "\"<exe>\" --flag" shape the registry path reports, so
// reconcile_autostart treats both stores identically.
// ----------------------------------------------------------------

/// This app's LaunchAgent label (= plist file stem).
#[cfg(any(target_os = "macos", test))]
const LAUNCH_AGENT_LABEL: &str = "com.shawnrodis.homestead";

/// Where the LaunchAgent plist lives (macOS).
#[cfg(target_os = "macos")]
fn launch_agent_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| {
        PathBuf::from(h)
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{}.plist", LAUNCH_AGENT_LABEL))
    })
}

/// XML-escape text destined for a plist <string>.
#[cfg(any(target_os = "macos", test))]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Inverse of xml_escape (only the entities it can produce).
#[cfg(any(target_os = "macos", test))]
fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Split a stored autostart command ("\"<exe>\" --minimized")
/// back into its exe path and launch mode — the inverse of
/// autostart_command, shared by the macOS plist writer.
#[cfg(any(target_os = "macos", test))]
fn parse_autostart_command(cmd: &str) -> Option<(String, LaunchMode)> {
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

/// The LaunchAgent plist for a stored autostart command.
#[cfg(any(target_os = "macos", test))]
fn launch_agent_plist(cmd: &str) -> Option<String> {
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

/// The exe a LaunchAgent plist launches: the first <string> of
/// its ProgramArguments array. (The launch-mode flag, when
/// present, is a later <string> in the same array and is read
/// off the whole text by launch_mode_from_command.)
#[cfg(any(target_os = "macos", test))]
fn launch_agent_exe(plist: &str) -> Option<String> {
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

/// Reconstruct the stored autostart command from a plist's
/// text — what reg_run_value reports on macOS. None when the
/// plist isn't this app's agent.
#[cfg(any(target_os = "macos", test))]
fn launch_agent_command(plist: &str) -> Option<String> {
    if !plist.contains(LAUNCH_AGENT_LABEL) {
        return None;
    }
    let exe = launch_agent_exe(plist)?;
    Some(autostart_command(
        Path::new(&exe),
        launch_mode_from_command(plist),
    ))
}

/// The current autostart entry for this app, if one exists —
/// the Run-key value on Windows, the LaunchAgent's command on
/// macOS. Same reported shape on both (see the notes above).
#[cfg(target_os = "windows")]
fn reg_run_value() -> Option<String> {
    let key = format!("HKCU\\{}", RUN_KEY);
    let out = reg_command()
        .args(["query", &key, "/v", RUN_VALUE_NAME])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_reg_named_value(&String::from_utf8_lossy(&out.stdout), RUN_VALUE_NAME)
}

#[cfg(target_os = "macos")]
fn reg_run_value() -> Option<String> {
    let path = launch_agent_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    launch_agent_command(&text)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn reg_run_value() -> Option<String> {
    None
}

/// Write (Some) or remove (None) the autostart entry — the
/// Run-key value on Windows, the LaunchAgent plist on macOS.
#[cfg(target_os = "windows")]
fn reg_set_run_value(cmdline: Option<&str>) -> bool {
    let key = format!("HKCU\\{}", RUN_KEY);
    let status = match cmdline {
        Some(c) => reg_command()
            .args(["add", &key, "/v", RUN_VALUE_NAME, "/t", "REG_SZ", "/d", c, "/f"])
            .status(),
        None => reg_command()
            .args(["delete", &key, "/v", RUN_VALUE_NAME, "/f"])
            .status(),
    };
    status.map(|s| s.success()).unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn reg_set_run_value(cmdline: Option<&str>) -> bool {
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
fn reg_set_run_value(_cmdline: Option<&str>) -> bool {
    false
}

/// Startup reconciliation: the OS autostart STORE is the truth
/// for "Start with Windows / at login". An existing entry wins —
/// the toggle shows ON, its flag sets the stored launch mode, and
/// a stale command (an old exe path after a move, say) is
/// rewritten canonically for the current exe. No entry → the
/// toggle shows OFF, whatever the saved prefs claimed, so the
/// switch never lies.
fn reconcile_autostart(cfg: &mut Config) {
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

/// Apply the Start-with-Windows prefs to the OS autostart store;
/// true when the store afterwards holds exactly what the prefs
/// intend.
fn apply_autostart(prefs: &Prefs) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let want = desired_autostart(prefs, &exe);
    let _ = reg_set_run_value(want.as_deref());
    reg_run_value() == want
}

/// The tray icon and its menu ids. The icon object must simply
/// stay alive — dropping it removes the icon — so it rides along
/// in the App for the whole session once created.
struct TrayState {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    _icon: tray_icon::TrayIcon,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    open_id: tray_icon::menu::MenuId,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    quit_id: tray_icon::menu::MenuId,
}

/// What the user asked the tray to do.
#[derive(Clone, Copy, PartialEq, Debug)]
enum TrayAction {
    Show,
    Quit,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn build_tray() -> Option<TrayState> {
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
    let open_item = MenuItem::new("Open Homestead", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    let open_id = open_item.id().clone();
    let quit_id = quit_item.id().clone();
    let menu = Menu::new();
    menu.append(&open_item).ok()?;
    menu.append(&PredefinedMenuItem::separator()).ok()?;
    menu.append(&quit_item).ok()?;
    let icon = tray_icon::Icon::from_rgba(
        include_bytes!("../assets/tray_icon.rgba").to_vec(),
        64,
        64,
    )
    .ok()?;
    let tray = tray_icon::TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Homestead")
        .with_icon(icon)
        .build()
        .ok()?;
    Some(TrayState {
        _icon: tray,
        open_id,
        quit_id,
    })
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn build_tray() -> Option<TrayState> {
    None
}

/// v1.2.3: tray events are no longer drained here (on Windows).
/// The dedicated tray thread (spawn_tray_thread, by TRAY_QUEUE
/// above) owns both receivers — muda's menu channel and tray-icon's
/// icon channel — and pushes mapped actions onto TRAY_QUEUE; the
/// UI thread just drains the queue in handle_window_chrome.
/// Draining from update() starved on Windows whenever the hidden
/// window ran no frames, which is exactly when tray actions
/// matter (see the Bug C note at TRAY_QUEUE).
///
/// macOS (v1.4.0) deliberately uses the simpler pre-v1.2.3
/// pattern — App::poll_tray_events_macos drains both receivers
/// into the same TRAY_QUEUE from handle_window_chrome. The
/// starvation was a Windows/winit quirk (an invisible window
/// never turns request_redraw into a frame); on macOS the
/// run loop keeps delivering the 200 ms hidden heartbeat, so
/// frames — and therefore polls — keep happening while hidden.

/// The main window's HWND, for the one-time tray balloon.
#[cfg(target_os = "windows")]
fn main_hwnd(frame: &eframe::Frame) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = frame.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn main_hwnd(_frame: &eframe::Frame) -> Option<isize> {
    None
}

/// The one-time "still running in the tray" balloon (Windows,
/// best-effort). Shell_NotifyIcon hangs balloons on an icon owned
/// by a window; the tray-icon crate doesn't expose one, so this
/// adds a TRANSIENT notify icon against the main window (a second
/// (hwnd, id) pair — the real tray icon is a different pair and is
/// untouched), shows the balloon from it, and removes it a few
/// seconds later from a helper thread. Any failure is silent.
#[cfg(target_os = "windows")]
fn show_tray_balloon(hwnd: isize) {
    const BALLOON_UID: u32 = 0x5A21;
    std::thread::spawn(move || unsafe {
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows_sys::Win32::UI::Shell::{
            Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
            NIIF_INFO, NOTIFYICONDATAW,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{LoadIconW, IDI_APPLICATION};

        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd as HWND;
        nid.uID = BALLOON_UID;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_INFO;
        nid.uCallbackMessage = 0x8001;
        let hinst = GetModuleHandleW(std::ptr::null());
        // Resource 101 is the app icon embedded by build.rs; fall
        // back to a stock icon if it can't be loaded.
        let mut hicon = LoadIconW(hinst, 101 as *const u16);
        if hicon.is_null() {
            hicon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
        }
        nid.hIcon = hicon;
        let tip = "Homestead";
        for (i, ch) in tip.encode_utf16().enumerate() {
            if i + 1 < nid.szTip.len() {
                nid.szTip[i] = ch;
            }
        }
        let title = "Homestead";
        for (i, ch) in title.encode_utf16().enumerate() {
            if i + 1 < nid.szInfoTitle.len() {
                nid.szInfoTitle[i] = ch;
            }
        }
        let msg = "Homestead is still running in the system tray — right-click its icon to quit.";
        for (i, ch) in msg.encode_utf16().enumerate() {
            if i + 1 < nid.szInfo.len() {
                nid.szInfo[i] = ch;
            }
        }
        nid.dwInfoFlags = NIIF_INFO;
        if Shell_NotifyIconW(NIM_ADD, &nid) != 0 {
            std::thread::sleep(Duration::from_secs(6));
            let mut bye: NOTIFYICONDATAW = std::mem::zeroed();
            bye.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            bye.hWnd = hwnd as HWND;
            bye.uID = BALLOON_UID;
            Shell_NotifyIconW(NIM_DELETE, &bye);
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn show_tray_balloon(_hwnd: isize) {}

/// VLC's well-known install folders: its installer registers App
/// Paths but does NOT add itself to PATH (the v1.1.0 "vlc: program
/// not found" bug — VLC was installed all along). On macOS the
/// binary lives inside the app bundle in /Applications.
fn vlc_known_dirs() -> Vec<PathBuf> {
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
fn resolve_player_program(prog: &str) -> Option<String> {
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
fn launch_player(player_cmd: &str, url: &str) -> Result<String, String> {
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

// ---------------------------------------------------------------------------
// Live camera previews (v1.2.0, Windows edition) — a bundled ffmpeg.exe
// decodes each camera's RTSP stream to raw RGBA frames on a worker
// thread; camera tiles and the big player upload the latest frame as
// an egui texture, updated in place. One pump per key ("cam:{id}" for
// tiles, "player" for the big player), driven by a small state
// machine: Connecting → Live, and on ffmpeg exit/EOF → Backoff with
// 2 s / 5 s / 15 s retry delays while the preview stays on. The web
// edition can't run ffmpeg, so its tiles embed the gateway feed
// instead — a documented platform difference (DESIGN.md).
// ---------------------------------------------------------------------------

const TILE_W: usize = 480;
const TILE_H: usize = 270;
const PLAYER_W: usize = 960;
const PLAYER_H: usize = 540;

/// Bytes per raw RGBA frame at w×h.
fn frame_bytes(w: usize, h: usize) -> usize {
    w * h * 4
}

/// ffmpeg arguments for the frame pump: TCP RTSP in, scaled + padded
/// to exactly w×h at 12 fps, raw RGBA frames on stdout. The URL (with
/// credentials) travels as a single argv — never through a shell.
fn ffmpeg_args(url: &str, w: usize, h: usize) -> Vec<String> {
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
fn ffmpeg_bundled_name(arch: &str) -> Option<&'static str> {
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
fn ffmpeg_candidates(exe_dir: &Path, override_path: &str, path_var: &str) -> Vec<PathBuf> {
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
fn find_ffmpeg(override_path: &str) -> Option<PathBuf> {
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
fn pump_backoff(streak: u32) -> Duration {
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
fn ffmpeg_failure_reason(stderr_tail: &str) -> Option<String> {
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
enum PumpAction {
    Keep,
    StartFresh,
    RetryDue,
}

fn pump_action(
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
fn tile_pump_wanted(
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
fn backoff_wake_delay(retry_at: Option<Instant>, now: Instant) -> Duration {
    match retry_at {
        Some(t) => t
            .saturating_duration_since(now)
            .clamp(Duration::from_millis(50), Duration::from_secs(1)),
        None => Duration::from_millis(500),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum PumpState {
    Connecting,
    Live,
    Backoff,
}

enum FrameMsg {
    Frame(Vec<u8>),
    /// ffmpeg exited / hit EOF; carries the mapped failure reason
    /// (ffmpeg_failure_reason over the stderr tail), when known.
    Ended(Option<String>),
}

/// Painted play/pause overlay chip for camera video (v1.2.1
/// cosmetic fix): the previous ⏸/▶ FONT-GLYPH button rendered as a
/// big chunky dark square with two white blobs in the symbol font.
/// The icon is drawn with the painter instead — two rounded bars
/// for pause, a triangle for play — inside a 28×28 rounded chip
/// with a translucent dark fill and a subtle light border, matching
/// the web edition's CSS chip. Registered after the video area, so
/// it keeps click priority in its corner.
fn play_pause_chip(ui: &mut Ui, id: &str, rect: egui::Rect, paused: bool) -> egui::Response {
    let resp = ui.interact(rect, egui::Id::new(("cam-chip", id)), Sense::click());
    let painter = ui.painter();
    let fill = if resp.hovered() {
        Color32::from_black_alpha(195)
    } else {
        Color32::from_black_alpha(150)
    };
    painter.rect_filled(rect, CornerRadius::same(9), fill);
    painter.rect_stroke(
        rect,
        CornerRadius::same(9),
        Stroke::new(1.0, Color32::from_white_alpha(60)),
        egui::StrokeKind::Middle,
    );
    let c = rect.center();
    if paused {
        // Play: a filled triangle, optically centred.
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(c.x - 3.4, c.y - 6.0),
                pos2(c.x - 3.4, c.y + 6.0),
                pos2(c.x + 5.4, c.y),
            ],
            Color32::WHITE,
            Stroke::NONE,
        ));
    } else {
        // Pause: two rounded bars.
        for dx in [-3.7, 3.7] {
            let bar = egui::Rect::from_center_size(pos2(c.x + dx, c.y), vec2(3.6, 12.0));
            painter.rect_filled(bar, CornerRadius::same(2), Color32::WHITE);
        }
    }
    resp
}

/// Identity of one pump's feed — when it changes (camera edited,
/// quality switched, ffmpeg moved) the pump is restarted.
fn pump_source_key(ffmpeg: &Path, url: &str, w: usize, h: usize) -> String {
    format!("{}|{}|{}x{}", ffmpeg.display(), url, w, h)
}

/// One running (or backing-off) frame pump, owned by the UI thread.
struct Pump {
    state: PumpState,
    source_key: String,
    w: usize,
    h: usize,
    rx: Receiver<FrameMsg>,
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<std::process::Child>>>,
    texture: Option<egui::TextureHandle>,
    /// Consecutive failures — drives the backoff schedule.
    streak: u32,
    retry_at: Option<Instant>,
    /// Last mapped failure reason (v1.2.2) — shown under the
    /// tile's retry status after repeated failures.
    last_error: Option<String>,
}

impl Pump {
    /// Stop the worker: flag it and kill ffmpeg (killing closes
    /// stdout, which ends the worker's read loop immediately).
    fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut slot) = self.child.lock() {
            if let Some(mut ch) = slot.take() {
                let _ = ch.kill();
            }
        }
    }

    /// A frame arrived: the pump is Live and the failure streak
    /// resets (state-machine transition, kept pure for tests).
    fn note_frame(&mut self) {
        self.state = PumpState::Live;
        self.streak = 0;
        self.retry_at = None;
        self.last_error = None;
    }

    /// ffmpeg exited / hit EOF: enter Backoff with the streak's
    /// delay, remembering why. A no-op when already backing off —
    /// one pump, one state machine, never duplicate restarts.
    fn note_ended(&mut self, reason: Option<String>) {
        if self.state != PumpState::Backoff {
            self.state = PumpState::Backoff;
            self.retry_at = Some(Instant::now() + pump_backoff(self.streak));
            self.streak += 1;
            if reason.is_some() {
                self.last_error = reason;
            }
        }
    }
}

/// Spawn the worker thread for one pump. The thread runs ffmpeg,
/// reads exact w×h RGBA frames and forwards them; it keeps ffmpeg
/// blocked on a full pipe rather than building a backlog here, and
/// the UI keeps only the latest frame, so a slow frame is dropped,
/// never queued. ffmpeg's stderr is captured (last few lines) so
/// an exit can carry its mapped failure reason (v1.2.2).
/// `source_key` is computed by the caller (sync_pumps) so the
/// generation tag for camera edits rides along.
fn spawn_pump(
    ffmpeg: &Path,
    url: &str,
    w: usize,
    h: usize,
    source_key: String,
    ctx: &egui::Context,
) -> Pump {
    let (tx, rx) = channel::<FrameMsg>();
    let stop = Arc::new(AtomicBool::new(false));
    let child_slot: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));
    {
        let stop = stop.clone();
        let child_slot = child_slot.clone();
        let ctx = ctx.clone();
        let ff = ffmpeg.to_path_buf();
        let url = url.to_string();
        thread::spawn(move || {
            let mut cmd = Command::new(&ff);
            cmd.args(ffmpeg_args(&url, w, h))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            // No console window flashing up next to the GUI app.
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(_) => {
                    let _ = tx.send(FrameMsg::Ended(Some(
                        "Could not start ffmpeg".to_string(),
                    )));
                    ctx.request_repaint();
                    return;
                }
            };
            // stderr tail: last lines, for the failure reason.
            let err_tail: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            if let Some(stderr) = child.stderr.take() {
                let tail = err_tail.clone();
                thread::spawn(move || {
                    use std::io::{BufRead, BufReader};
                    let mut reader = BufReader::new(stderr);
                    let mut line = String::new();
                    loop {
                        line.clear();
                        match reader.read_line(&mut line) {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {
                                let t = line.trim().to_string();
                                if !t.is_empty() {
                                    if let Ok(mut v) = tail.lock() {
                                        v.push(t);
                                        if v.len() > 12 {
                                            v.remove(0);
                                        }
                                    }
                                }
                            }
                        }
                    }
                });
            }
            let mut stdout = child.stdout.take();
            if let Ok(mut slot) = child_slot.lock() {
                *slot = Some(child);
            }
            if let Some(mut out) = stdout.take() {
                let mut buf = vec![0u8; frame_bytes(w, h)];
                loop {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    match out.read_exact(&mut buf) {
                        Ok(()) => {
                            if tx.send(FrameMsg::Frame(buf.clone())).is_err() {
                                break;
                            }
                            ctx.request_repaint();
                        }
                        Err(_) => break, // EOF: ffmpeg exited or was killed
                    }
                }
            }
            // Reap ffmpeg if the UI didn't already take it to kill it.
            if let Ok(mut slot) = child_slot.lock() {
                if let Some(mut ch) = slot.take() {
                    let _ = ch.kill();
                    let _ = ch.wait();
                }
            }
            if !stop.load(Ordering::SeqCst) {
                // Brief grace so the stderr reader can drain the
                // final lines (the reason is best-effort).
                thread::sleep(Duration::from_millis(30));
                let tail = err_tail
                    .lock()
                    .map(|v| v.join("\n"))
                    .unwrap_or_default();
                let _ = tx.send(FrameMsg::Ended(ffmpeg_failure_reason(&tail)));
                ctx.request_repaint();
            }
        });
    }
    Pump {
        state: PumpState::Connecting,
        source_key,
        w,
        h,
        rx,
        stop,
        child: child_slot,
        texture: None,
        streak: 0,
        retry_at: None,
        last_error: None,
    }
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

/// A discovery value as a number (v1.3.0, Fix A): the legacy
/// homeassistant API is STRINGLY-TYPED for lights — real payloads
/// carry brightness as "800", not 800 (switches use real numbers;
/// see the census fixtures in the tests) — so a JSON string that
/// parses as a number counts as a number. The pre-1.3.0 reader
/// used as_f64() alone and silently dropped every light reading.
fn value_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn data_f64(data: &Map<String, Value>, key: &str) -> Option<f64> {
    data.get(key).and_then(value_f64)
}

/// A discovery value as a bool (v1.3.0, Fix A): switches report
/// state as a real JSON bool, but LIGHTS report it as the STRING
/// "true"/"false" — the field bug behind every light reading Off
/// while plugs read correctly. The reference implementation only
/// ever checks `state === false`, i.e. it never demanded a bool;
/// this reader accepts bool, "true"/"false" strings and numbers.
fn data_bool(data: &Map<String, Value>, key: &str) -> Option<bool> {
    match data.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0),
        _ => None,
    }
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
        online: data_bool(data, "online") != Some(false),
        state: data_bool(data, "state") == Some(true),
        has_brightness,
        has_temp,
        has_color,
        // The VALUES additionally require a real reading
        // (fresh_value_counts): a stub payload's placeholder
        // (null / 0) yields None — "no current value" — while the
        // capability flags above stay presence-based, so the
        // control itself never disappears with the value.
        brightness_pct: if has_brightness
            && data.get("brightness").map(|v| fresh_value_counts("brightness", v)).unwrap_or(false)
        {
            data_f64(data, "brightness").map(brightness_to_pct)
        } else {
            None
        },
        temp_slider: if has_temp
            && data.get("color_temp").map(|v| fresh_value_counts("color_temp", v)).unwrap_or(false)
        {
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

/// Does this fresh payload value actually REPORT the field
/// (v1.2.2)? A JSON null never does. For brightness / colour
/// temperature a numeric 0 (or a non-numeric junk value) doesn't
/// either: on the reported scales those mean "no reading" — Tuya
/// stubs for lights the account can't fully see carry exactly such
/// placeholders — and treating them as real made colour temp snap
/// to the 2700K floor and brightness read 0 in v1.2.1.
fn fresh_value_counts(key: &str, v: &Value) -> bool {
    if v.is_null() {
        return false;
    }
    if key == "brightness" || key == "color_temp" {
        // Tolerant (v1.3.0): a string "800" is a real reading,
        // not a placeholder — judging it by as_f64() alone made
        // the merge overwrite fresh light values with stale ones.
        return value_f64(v).map(|n| n > 0.0).unwrap_or(false);
    }
    true
}

/// Keep previously-known data fields when a fresh discovery omits
/// them — or reports them as placeholders (see fresh_value_counts).
/// Capabilities therefore stick: once any payload has shown a
/// device has brightness / colour temp / colour, later thin
/// payloads can't take the controls away, because the merged data
/// still carries the keys and their last-known values (same merge
/// the web edition performs).
/// A fresh entry that carries NO usable data object at all (key
/// missing, null, or not an object) gets its last-known data
/// restored wholesale: without this, the device parses to
/// all-defaults (Off) and the data-less entry then poisons every
/// later merge — it drops out of prev_by_id — which is how a
/// single thin cycle turned the whole dashboard Off while a
/// third-party dashboard on the same account still showed the
/// lights On. Explicit values always win: a fresh real reading
/// (including an explicit Off) is never overwritten.
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
                match d.get_mut("data").and_then(|v| v.as_object_mut()) {
                    Some(data) => {
                        for (k, v) in prev_data.iter() {
                            let counts = data
                                .get(k)
                                .map(|fv| fresh_value_counts(k, fv))
                                .unwrap_or(false);
                            if !counts {
                                data.insert(k.clone(), v.clone());
                            }
                        }
                    }
                    None => {
                        if let Some(obj) = d.as_object_mut() {
                            obj.insert("data".to_string(), Value::Object((*prev_data).clone()));
                        }
                    }
                }
            }
            d
        })
        .collect()
}

/// Display names of devices whose fresh discovery entry carries no
/// usable state data (no data object, a non-object data value, or
/// a data object without a "state" key) while the previous entry
/// did carry a state key. Pure: the worker logs the result so the
/// diagnostics log shows which devices the preserve-last-known
/// path held, instead of a thin payload silently zeroing states.
/// Device display names only — no state values, no credentials.
fn devices_with_missing_state(fresh: &[Value], previous: &[Value]) -> Vec<String> {
    let prev_had_state: HashMap<&str, ()> = previous
        .iter()
        .filter_map(|d| {
            let id = d.get("id")?.as_str()?;
            let data = d.get("data")?.as_object()?;
            if data.contains_key("state") {
                Some((id, ()))
            } else {
                None
            }
        })
        .collect();
    fresh
        .iter()
        .filter_map(|d| {
            let id = d.get("id")?.as_str()?;
            if !prev_had_state.contains_key(id) {
                return None;
            }
            let usable = d
                .get("data")
                .and_then(|v| v.as_object())
                .map(|m| m.contains_key("state"))
                .unwrap_or(false);
            if usable {
                None
            } else {
                Some(
                    d.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(id)
                        .to_string(),
                )
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Section model (port of the web edition's buildSections / ordering rules)
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
enum SectionKind {
    Thermostat,
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

/// The hardware brightness floor on the DEVICE scale (v1.3.1):
/// the user's bulbs switch OFF ENTIRELY below 11% — it is their
/// true minimum, not a UI preference. (v1.2.3 lowered the floor
/// to 1% on the theory that 11% was inherited from the
/// reference slider; the field disproved that within a day.)
const BRIGHTNESS_FLOOR_PCT: i64 = 11;

/// Map a UI brightness percent (1..100 — what every slider
/// shows and edits) to the DEVICE percent a brightnessSet
/// sends (11..100), linearly: device = 11 + (ui-1) * 89/99.
/// The slider's bottom therefore displays "1%" while sending
/// exactly the 11% hardware minimum — a lit bulb is never
/// commanded below it — and the top is a true 100%.
fn brightness_ui_to_device(ui: i64) -> i64 {
    let ui = ui.clamp(1, 100);
    (BRIGHTNESS_FLOOR_PCT as f64 + (ui - 1) as f64 * 89.0 / 99.0).round() as i64
}

/// Map a reported DEVICE percent back to the UI scale for
/// display (the inverse of brightness_ui_to_device):
/// ui = 1 + (device-11) * 99/89, clamped to 1..100 — a bulb
/// reporting its 11% minimum displays "1%", and anything
/// reported below 11% (while on) displays as 1% too.
fn brightness_device_to_ui(dev: i64) -> i64 {
    ((1.0 + (dev - BRIGHTNESS_FLOOR_PCT) as f64 * 99.0 / 89.0).round() as i64).clamp(1, 100)
}

/// The brightnessSet payload for a UI percent: the mapped
/// device value (see brightness_ui_to_device). Brightness is
/// ONLY ever sent to devices that natively report a brightness
/// field (v1.3.0, reversing the v1.2.3 route that dimmed
/// colour-only devices through the V of a colorSet): the legacy
/// API surfaces no brightness/temp for a colour-only strip —
/// on/off + colour only — so the app shows no brightness
/// control for it, exactly like the reference. (The strip
/// hardware itself can dim via other channels; this API simply
/// doesn't expose it. See DESIGN.md.)
fn brightness_set_value(ui_pct: i64) -> Value {
    json!(brightness_ui_to_device(ui_pct))
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
            // Only members that natively report a brightness
            // field take part (v1.3.0): a colour-only strip is
            // excluded here — it still joins Toggle and Color.
            for d in devs.iter().filter(|d| d.has_brightness) {
                plan.push((d.id.clone(), "brightnessSet", brightness_set_value(num)));
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

/// The sections' DEFAULT order: Thermostat first (it was the fixed
/// block above the sections through v1.6.x, and stays on top by
/// default now that it is a section), then Cameras, Pinned,
/// Lighting, user groups (creation order, expanded in place),
/// Ungrouped, Scenes. Expressed over a built section list so groups
/// land exactly where they were created relative to each other.
fn default_section_seq(sections: &[Section]) -> Vec<String> {
    fn slot(s: &Section) -> usize {
        match s.kind {
            SectionKind::Thermostat => 0,
            SectionKind::Cameras => 1,
            SectionKind::Pinned => 2,
            SectionKind::Lighting => 3,
            SectionKind::Group => 4,
            SectionKind::Ungrouped => 5,
            SectionKind::Scenes => 6,
        }
    }
    let mut idx: Vec<usize> = (0..sections.len()).collect();
    idx.sort_by_key(|&i| slot(&sections[i])); // stable sort: groups keep creation order
    idx.iter().map(|&i| sections[i].order_key.clone()).collect()
}

/// The EFFECTIVE section sequence: the stored user order (filtered
/// to sections that exist) wins; any section missing from the stored
/// list (a new group, a newly-appearing built-in) slots in right
/// after its nearest already-placed default-order predecessor — or
/// at the front when it has none.
fn effective_section_seq(sections: &[Section], stored: &[String]) -> Vec<String> {
    let present: HashSet<&str> = sections.iter().map(|s| s.order_key.as_str()).collect();
    let mut eff: Vec<String> = stored
        .iter()
        .filter(|k| present.contains(k.as_str()))
        .cloned()
        .collect();
    let default = default_section_seq(sections);
    for (i, key) in default.iter().enumerate() {
        if eff.contains(key) {
            continue;
        }
        let mut at = 0;
        for prev in default[..i].iter().rev() {
            if let Some(p) = eff.iter().position(|k| k == prev) {
                at = p + 1;
                break;
            }
        }
        eff.insert(at, key.clone());
    }
    eff
}

/// Sort a built section list into the effective sequence.
fn order_sections(sections: &mut [Section], stored: &[String]) {
    let eff = effective_section_seq(sections, stored);
    let rank: HashMap<&str, usize> =
        eff.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    sections.sort_by_key(|s| rank[s.order_key.as_str()]);
}

/// The stored sequence after moving `key` one step: start from the
/// effective sequence of the RENDERED sections, swap with the
/// neighbour, then re-append any stored keys for sections that
/// aren't currently rendered (in their stored relative order) so a
/// move never forgets a hidden section's place.
fn moved_section_seq(
    stored: &[String],
    rendered: &[Section],
    key: &str,
    dir: i32,
) -> Vec<String> {
    let mut seq = effective_section_seq(rendered, stored);
    if let Some(i) = seq.iter().position(|k| k == key) {
        let j = i as i32 + dir;
        if j >= 0 && (j as usize) < seq.len() {
            seq.swap(i, j as usize);
        }
    }
    for k in stored {
        if !seq.contains(k) {
            seq.push(k.clone());
        }
    }
    seq
}

/// Greedy row packing for the dashboard grid (v1.2.1): walk the
/// items in order, filling each row left to right; an item that
/// doesn't fit in the row's remaining columns starts a new row.
/// Spans are camera card sizes (1 = Normal, 2 = Large, `cols` =
/// Wide); devices, scenes and the drag gap pass their own span.
/// Mirrors what the web edition's CSS grid does with
/// `grid-column: span N`.
fn pack_rows<T: Clone>(items: &[T], spans: &[usize], cols: usize) -> Vec<Vec<(T, usize)>> {
    let cols = cols.max(1);
    let mut rows: Vec<Vec<(T, usize)>> = Vec::new();
    let mut cur: Vec<(T, usize)> = Vec::new();
    let mut used = 0usize;
    for (i, item) in items.iter().enumerate() {
        let span = spans.get(i).copied().unwrap_or(1).max(1).min(cols);
        if !cur.is_empty() && used + span > cols {
            rows.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push((item.clone(), span));
        used += span;
    }
    if !cur.is_empty() {
        rows.push(cur);
    }
    rows
}

fn build_sections(
    entries: &[CardEntry],
    layout: &LayoutCfg,
    editing: bool,
    query: &str,
    thermostat_paired: bool,
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
    // Sections are CONSTRUCTED in the default order (Thermostat →
    // Cameras → Pinned → Lighting → groups → Ungrouped → Scenes)
    // and then re-sorted by the user's stored section sequence
    // below. An explicit group assignment always wins; unassigned
    // lights land in Lighting, unassigned cameras in Cameras.
    // The thermostat is its own category section: it has no device
    // ids and is never filtered by search — but like Cameras, it
    // only exists once a thermostat is actually paired. Until
    // then there is no section, no card, and no "Not paired"
    // text on the dashboard; the edit bar's "+ Add thermostat"
    // button is the entry point to pairing.
    if thermostat_paired {
        sections.push(Section {
            key: "thermostat".into(),
            order_key: "thermostat".into(),
            title: "Thermostat".into(),
            kind: SectionKind::Thermostat,
            group_id: None,
            ids: Vec::new(),
        });
    }
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
    order_sections(&mut sections, &layout.section_seq);
    sections.retain(|s| {
        !s.ids.is_empty()
            // The thermostat has no device ids by design; it is
            // kept exactly when it was pushed (paired) above.
            || s.kind == SectionKind::Thermostat
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

/// Results from thermostat (HomeKit/HAP) background threads, reported
/// over a dedicated channel so Tuya traffic is untouched.
enum TstatResult {
    Discovered(Vec<crate::hap::DiscoveredDevice>),
    /// A pair-setup session is open (M1 sent, M2 received): the UI
    /// now prompts for the code the thermostat is showing. The
    /// device itself still lives in tstat_discovered; the session
    /// carries its id for matching the panel row.
    PairBegun {
        pending: crate::hap::PendingPairSetup,
    },
    Paired(crate::hap::ThermostatPairing),
    State(crate::hap::ThermostatState),
    /// A fetch success (autorefresh, manual Refresh, post-pairing,
    /// bridge Status, optimistic re-fetch). Clears fetch-busy only —
    /// never the action flag, so a fetch result can never end an
    /// action's "Working…" early.
    Fetched(crate::hap::ThermostatState),
    /// A fetch failure. Clears fetch-busy only.
    FetchFailed(String),
    /// An optimistic set: the PUT succeeded but the readback failed,
    /// so the state is the intended one (see hap::SetOutcome). Handled
    /// like State, plus a prompt re-fetch to confirm.
    StateConfirm(crate::hap::ThermostatState),
    /// A set failure (PUT rejected or network error). Handled like
    /// Failed, plus a prompt re-fetch so the tile reverts from any
    /// optimistic patch to the unit's true state quickly.
    SetFailed(String),
    Failed(String),
}

/// A web-edition thermostat request (arrived over the loopback bridge)
/// awaiting the background result it dispatched. The reply channel
/// goes back to the bridge thread, which answers the HTTP request.
/// The kind is what routes the next TstatResult to the right reply.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BridgePendingKind {
    Status,
    Discover,
    Set,
}

struct BridgePending {
    reply: Sender<bridge::BridgeResponse>,
    kind: BridgePendingKind,
}

// ---------------------------------------------------------------------------
// Auto-update (workstream C) — both editions check the project's GitHub
// "latest release" at most once per 24 h (attempt-based) and compare
// versions numerically. The native editions go further than the web
// page, each with its own asset (v1.4.0 made the choice a platform
// function): Windows downloads the Setup .exe, verifies its name,
// runs it, and exits so the per-user installer can replace the
// running app; macOS downloads the .dmg, verifies its name, and
// OPENS it (drag-to-Applications is the user's step — the app
// never touches its own bundle and does not exit).
// ---------------------------------------------------------------------------

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_API: &str =
    "https://api.github.com/repos/shawnrodis/Homestead/releases/latest";
const RELEASES_PAGE: &str =
    "https://github.com/shawnrodis/Homestead/releases/latest";
const UPDATE_PERIOD_MS: i64 = 24 * 60 * 60 * 1000;

/// The release asset THIS platform installs from: the Windows
/// Setup .exe, or the macOS disk image (v1.4.0). `os` is
/// std::env::consts::OS at the call site; a parameter so both
/// platforms' choices are unit-testable on any host. The macOS
/// app must never be offered the .exe (or vice versa).
fn expected_update_asset_name_for(version: &str, os: &str) -> String {
    if os == "macos" {
        format!("Homestead-v{}.dmg", version)
    } else {
        format!("Homestead-Setup-v{}.exe", version)
    }
}

fn expected_update_asset_name(version: &str) -> String {
    expected_update_asset_name_for(version, std::env::consts::OS)
}

/// Pick this platform's asset out of a release's asset list —
/// an exact name match (the same strictness the download
/// verification applies again before anything runs/opens).
fn select_update_asset<'a>(assets: &'a [Value], version: &str, os: &str) -> Option<&'a Value> {
    let want = expected_update_asset_name_for(version, os);
    assets.iter().find(|a| {
        a.get("name")
            .and_then(|n| n.as_str())
            .map(|n| n == want)
            .unwrap_or(false)
    })
}

/// What "install" means once the asset is downloaded & verified.
#[derive(Clone, Copy, PartialEq, Debug)]
enum UpdateInstall {
    /// Windows: run the Setup .exe, then exit so it can replace
    /// the running app.
    RunInstallerAndExit,
    /// macOS: open the .dmg and let the user drag the app into
    /// Applications; the app keeps running.
    OpenDiskImage,
}

fn update_install_for(os: &str) -> UpdateInstall {
    if os == "macos" {
        UpdateInstall::OpenDiskImage
    } else {
        UpdateInstall::RunInstallerAndExit
    }
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
    asset_name: String,
    asset_url: String,
    asset_size: u64,
}

/// Pull THIS platform's update asset out of a GitHub "latest
/// release" response (the Setup .exe on Windows, the .dmg on
/// macOS — select_update_asset).
fn parse_release(v: &Value) -> Option<ReleaseInfo> {
    let tag = v.get("tag_name")?.as_str()?;
    let version = tag.trim_start_matches('v').to_string();
    let page_url = v.get("html_url")?.as_str()?.to_string();
    let assets = v.get("assets")?.as_array()?;
    let asset = select_update_asset(assets, &version, std::env::consts::OS)?;
    Some(ReleaseInfo {
        version,
        page_url,
        asset_name: asset.get("name")?.as_str()?.to_string(),
        asset_url: asset.get("browser_download_url")?.as_str()?.to_string(),
        asset_size: asset.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
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
        // Record the attempt: this fallback auth counts against
        // Tuya's 180 s limit, so the per-refresh gate must see it
        // (v2.0.5). The gate itself does not cover this path —
        // dead-session recovery and control toggles need it
        // ungated — but its attempts must still advance the
        // timestamp, or the next refresh re-login would trip
        // the wall.
        REFRESH_RELOGIN_LAST_AUTH_MS.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
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
        if is_rate_limit_code(code) {
            return Err(DiscoveryFail::Frequent);
        }
        // A dead session surfaces here as a token/auth code (the
        // access token was revoked or expired server-side, e.g. by
        // a sign-in on another install). Classified separately so
        // the worker can background re-auth instead of treating it
        // like a generic failure (v2.0.2).
        if is_auth_failure_code(code) {
            return Err(DiscoveryFail::Auth(format!("API error: {}", code)));
        }
    }
    if let Some(devs) = json
        .get("payload")
        .and_then(|p| p.get("devices"))
        .and_then(|v| v.as_array())
    {
        // Diagnostics: name the devices whose fresh entry carries
        // no state data, so a thin payload is visible in the log
        // instead of silently zeroing states (v2.0.3).
        let kept = devices_with_missing_state(devs, previous_raw);
        if !kept.is_empty() {
            crate::hap::tstat_log(&format!(
                "discovery: kept last-known state for {} (fresh payload missing state data)",
                kept.join(", ")
            ));
        }
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
    /// The skill response says the session itself is dead
    /// (revoked/expired token). Unlike Msg, this is always worth a
    /// background re-login with saved credentials (v2.0.2).
    Auth(String),
}

// ---------------------------------------------------------------------------
// Rate limiting (v1.2.0): Tuya answers bursts with a "FrequentlyInvoke"
// header code. By product decision the app NEVER surfaces rate-limit
// warnings — no toasts, banners or status lines. Instead: discovery
// falls back to the cached devices and backs off internally (see
// App::note_rate_limited), and control commands retry silently
// (net_control below) before anything counts as a failure. Genuine
// errors (device offline, command rejected) are still reported.
// ---------------------------------------------------------------------------

/// True when a skill-response header code means "rate limited".
fn is_rate_limit_code(code: &str) -> bool {
    code.eq_ignore_ascii_case("FrequentlyInvoke")
        || code.eq_ignore_ascii_case("TooManyRequests")
        || code.to_ascii_lowercase().contains("frequent")
}

/// True when a skill-response header code means the session is dead
/// (revoked/expired token), as opposed to rate limiting or a generic
/// failure. Discovery calls carry no device id, so a token/auth code
/// here can only be about the session itself. Deliberately narrow:
/// a code like InvalidDeviceId is not an auth failure.
fn is_auth_failure_code(code: &str) -> bool {
    let c = code.to_ascii_lowercase();
    c.contains("token")
        || c.contains("auth")
        || c.contains("session")
        || c.contains("login")
        || c.contains("unauthorized")
        || c.contains("expired")
        || c.contains("permission")
}

/// True when a DiscErr should carry the no-saved-login hint:
/// "Session expired" is returned by fresh_session exactly when the
/// refresh token is dead and no saved credentials exist to re-login
/// with, so that message with no saved login names the gate that
/// makes background re-auth impossible.
fn session_expired_hint_needed(err: &str, has_saved_creds: bool) -> bool {
    err == "Session expired" && !has_saved_creds
}

// ---------------------------------------------------------------------------
// Per-refresh background re-login (v2.0.4, gated in v2.0.5): only a
// fresh session returns current states (v2.0.3 field result:
// refresh on the existing session shows stale explicit states
// while a fresh login shows truth), so refresh cycles re-login
// first — internally simulating a sign-off/sign-in with the saved
// credentials, skipping the refresh token. No form, no toast, no
// user interaction. v2.0.5 gates it: Tuya allows at most one auth
// per 180 s ("you cannot auth exceed once in 180 seconds"), so
// the ungated per-cycle re-login ran straight into that wall.
// Now a re-login runs only when at least 185 s have passed since
// the last auth attempt (180 s + 5 s skew margin); other cycles
// run discovery on the existing session, unchanged. That yields
// the user's cadence exactly: 30 s normal refreshes, a fresh
// session roughly every 6th cycle. Guards keep this from becoming
// an auth hammer: no saved creds skips it; a throttle error
// pauses re-logins for a bounded window; any other failure falls
// back to the existing session for that cycle. Gated re-login
// outcomes are logged (at ~20 lines/hour that fits the 200-entry
// log); the no-saved-login hint stays once per stretch.
// ---------------------------------------------------------------------------

/// How long background re-logins pause after the auth endpoint
/// throttles them. Matches the discovery backoff cap (300s).
const REFRESH_RELOGIN_BACKOFF_MS: i64 = 300_000;

/// Minimum spacing between auth.do attempts (v2.0.5): Tuya allows
/// at most one auth per 180 s. The 5 s margin covers server-side
/// clock skew so a re-login at the boundary never trips the wall.
const REFRESH_RELOGIN_MIN_INTERVAL_MS: i64 = 185_000;

/// Last auth.do attempt (ms since epoch), process-wide like the
/// throttle window. Every auth attempt counts against Tuya's
/// 180 s limit, whether it succeeds, fails, or throttles — so the
/// timestamp is recorded when the attempt is made, not on its
/// outcome (per-refresh re-login, dead-session retry, fresh
/// sign-in, and fresh_session's fallback all stamp it). Defaults
/// to 0 ("long ago"): a restored old session (app start with a
/// saved session, not freshly authed) therefore re-logs in on its
/// first refresh, while a fresh sign-in stamps it and the next
/// immediate refresh does not re-login.
static REFRESH_RELOGIN_LAST_AUTH_MS: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(0);

/// Process-wide throttle window for per-refresh re-logins, set by
/// the discovery worker when auth throttles it. A static (not App
/// state) because the worker thread cannot report back to App
/// except through its single NetResult; the window only gates
/// re-login attempts, never discovery itself.
static REFRESH_RELOGIN_THROTTLE_UNTIL_MS: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(0);

/// Whether the no-saved-login hint has already been logged for the
/// current creds-absent stretch. Bounds the hint to once per
/// stretch instead of once per 30s cycle; cleared whenever saved
/// credentials exist, so a later creds-absent stretch logs again.
static REFRESH_RELOGIN_NO_CREDS_HINTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Pure decision: may a background re-login be attempted now?
fn refresh_relogin_allowed(now_ms: i64, throttle_until_ms: i64) -> bool {
    now_ms >= throttle_until_ms
}

/// Pure decision (v2.0.5): may a gated background re-login be
/// attempted now? Open only when the throttle window has lapsed
/// AND at least 185 s have passed since the last auth attempt.
/// Composes the v2.0.4 throttle decision so that test still pins
/// its half; the interval half is pinned by the gate tests.
fn refresh_relogin_gate_open(now_ms: i64, last_auth_ms: i64, throttle_until_ms: i64) -> bool {
    refresh_relogin_allowed(now_ms, throttle_until_ms)
        && now_ms - last_auth_ms >= REFRESH_RELOGIN_MIN_INTERVAL_MS
}

/// Dynamic normal-refresh interval (v2.0.6): the Tuya discovery
/// timer no longer offers fixed options — it adapts. The floor is
/// 30 s (the old 15 s option invited throttling) and the cap is
/// 5 min, matching the discovery backoff cap. Throttling doubles
/// the interval from its CURRENT value (never from a fixed base),
/// so a throttle during partial recovery steps back up from where
/// the interval actually is; success halves it back toward the
/// floor. Pure so tests pin the steps without a timer.
const DISC_INTERVAL_FLOOR_SECS: u64 = 30;
const DISC_INTERVAL_CAP_SECS: u64 = 300;

fn disc_interval_after_throttle(current: u64) -> u64 {
    current.saturating_mul(2).max(60).min(DISC_INTERVAL_CAP_SECS)
}

fn disc_interval_after_success(current: u64) -> u64 {
    if current <= DISC_INTERVAL_FLOOR_SECS {
        DISC_INTERVAL_FLOOR_SECS
    } else {
        (current / 2).max(DISC_INTERVAL_FLOOR_SECS)
    }
}

/// Read-only display text for the dynamic interval (v2.0.6):
/// "Auto 30s", "Auto 2 min", "Auto 5 min" — information, not a
/// choice. Whole minutes only when the value divides evenly.
fn auto_interval_text(secs: u64) -> String {
    if secs >= 60 && secs % 60 == 0 {
        format!("Auto {} min", secs / 60)
    } else {
        format!("Auto {}s", secs)
    }
}

/// True when an auth error means the auth endpoint throttled us
/// (as opposed to wrong credentials or a network failure).
/// Reuses the discovery rate-limit spellings and adds generic
/// throttle/429 wording, because net_auth surfaces API messages
/// verbatim and a throttled login may not use the skill codes.
fn is_throttle_error(err: &str) -> bool {
    if is_rate_limit_code(err) {
        return true;
    }
    let e = err.to_ascii_lowercase();
    e.contains("too many")
        || e.contains("rate limit")
        || e.contains("ratelimit")
        || e.contains("throttl")
        || e.contains("429")
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ControlOutcome {
    Ok,
    RateLimited,
    Failed,
}

/// Classify a control response the way the control paths need.
fn control_outcome(json: &Value) -> ControlOutcome {
    match json
        .get("header")
        .and_then(|h| h.get("code"))
        .and_then(|v| v.as_str())
    {
        Some("SUCCESS") => ControlOutcome::Ok,
        Some(code) if is_rate_limit_code(code) => ControlOutcome::RateLimited,
        _ => ControlOutcome::Failed,
    }
}

/// A single control attempt, classified.
fn net_control_once(
    client: &reqwest::blocking::Client,
    session: &Session,
    dev_id: &str,
    action: &str,
    value: &Value,
    prefs: &Prefs,
) -> ControlOutcome {
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
        Ok(json) => control_outcome(&json),
        Err(_) => ControlOutcome::Failed,
    }
}

/// One control command with silent rate-limit retries: up to 2 extra
/// attempts, ~1 s apart, when Tuya rate-limits — the retry delay is
/// also the extra spacing group broadcasts rely on. Only a genuine
/// failure (or a third rate-limit) returns false, and only then does
/// the caller roll back / report it.
fn net_control(
    client: &reqwest::blocking::Client,
    session: &Session,
    dev_id: &str,
    action: &str,
    value: &Value,
    prefs: &Prefs,
) -> bool {
    for attempt in 0..3 {
        match net_control_once(client, session, dev_id, action, value, prefs) {
            ControlOutcome::Ok => return true,
            ControlOutcome::RateLimited if attempt < 2 => {
                thread::sleep(Duration::from_secs(1));
            }
            _ => return false,
        }
    }
    false
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

/// Header name/status budget. The name block's width is whatever
/// remains in the card header after the icon block — measured
/// honestly by egui at layout time — minus the trailing controls'
/// footprint. The footprints below are MEASURED (headless egui
/// harness, the app's own styles): pin 16 + toggle 48 + hide 13 +
/// two 8px item spacings = 93 for a device; hide alone = 13 for
/// a scene; pin + hide + one spacing = 37 for a camera.
///
/// v1.2.3 root cause (proven by the render gate in the tests,
/// which reads back the galley rows egui actually draws): this
/// helper was never the whole problem. At the card widths the
/// grid ACTUALLY produces — the column formula pins cards to
/// 238–262pt at ordinary window sizes and DPI scalings — the
/// fixed overhead (32pt card padding + ~50pt icon block + ~104pt
/// of trailing controls) leaves only ~52–76pt for the name,
/// while real names need ~80–106pt at 15pt ("Kitchen Light 1"
/// measures 102.6pt). No honest deduction fits them on one row
/// at those widths, so below NAME_TWO_ROW_MIN_W of single-row
/// budget the pin and toggle move to their own right-aligned
/// row under the header (see device_card / camera_card), and
/// the name only reserves the hide button. The v1.2.2 unit test
/// missed this because it fed the helper a 268pt "available"
/// input the user's windows never produce; the stretch of a
/// wider cell still passes through to the budget 1:1.
fn header_name_budget(avail_after_icon: f32, trailing_deduct: f32) -> f32 {
    (avail_after_icon - trailing_deduct).max(40.0)
}

/// Total deduction (true trailing footprint + the name-to-controls
/// gap + a few px of safety) per card kind.
const TRAILING_DEDUCT_DEVICE: f32 = 104.0;
const TRAILING_DEDUCT_SCENE: f32 = 24.0;
const TRAILING_DEDUCT_CAMERA: f32 = 48.0;
/// Two-row header: the name reserves only the hide button's
/// own 13pt (the name truncates INSIDE its block and the ✕
/// glyph is centred with its own padding, so the two never
/// touch); pin + toggle live on the row below.
const TRAILING_DEDUCT_HIDE_ONLY: f32 = 14.0;
/// If the single-row name budget would come out below this, the
/// header switches to its two-row form — real device names need
/// ~105pt, so single-row engages only when it can honour that.
const NAME_TWO_ROW_MIN_W: f32 = 112.0;
/// Scene headers carry only the hide ✕, so their single-row
/// budget is roomier — but the longest scene names ("Turn off
/// everything", ~138pt strong) still miss it at the grid's
/// trough card widths (~238pt, where the budget is ~132pt).
/// Below this budget the scene's ✕ drops to its own row too and
/// the name takes the full header width.
const SCENE_TWO_ROW_MIN_W: f32 = 144.0;

/// Decide the header form: two rows (the name gets nearly the
/// whole card width; pin + toggle drop to their own row)
/// whenever the single-row budget can't fit a real name. A pure
/// function of the measured remainder so the render gate can pin
/// the behaviour at every width.
fn two_row_header(avail_after_icon: f32, trailing_deduct: f32) -> bool {
    header_name_budget(avail_after_icon, trailing_deduct) < NAME_TWO_ROW_MIN_W
}

/// Group-controls strip geometry (v1.2.3, Bug A). Every control
/// group in a section header is ONE unbreakable mini-panel of
/// this height: the All toggle, each slider group (label+value
/// over its slider), and the swatch row all allocate exactly
/// GROUP_PANEL_H, so the wrapped placer can only move WHOLE
/// groups between lines — v1.2.2's height-0 slider allocations
/// (vec2(210, 0)) let it interleave the Brightness and
/// Colour-temperature pairs into a staircase.
/// v1.3.0 alignment fix: the constant is the panels' TRUE
/// content height — label row 18 + item spacing 8 + gap 3 +
/// slider 22 = 51, measured on the headless harness. At 44 the
/// content overflowed its allocation, and the wrapped placer's
/// cross-axis CENTERING then parked each subsequent panel a
/// couple of points lower than the first (the colour-temperature
/// slider sat visibly below the brightness slider). The strip
/// now also lays out with cross-axis Align::Min (see
/// group_controls), so every panel shares the row's top edge by
/// construction, whatever the placer's row-height estimate.
const GROUP_PANEL_H: f32 = 51.0;
/// Width of one group slider panel (label+value row over a full
/// slim slider).
const GROUP_SLIDER_W: f32 = 240.0;
/// Width of the All (label + master toggle) panel.
const GROUP_ALL_W: f32 = 84.0;
/// Width of the swatch panel: 10 swatches × 26 + tightened gaps
/// + the 🎨 custom-colour button, one row.
const GROUP_SWATCH_W: f32 = 352.0;

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
            // Displayed on the UI scale (v1.3.1), like the sliders.
            parts.push(format!("{}%", brightness_device_to_ui(b)));
        }
        if d.color_mode.as_deref() == Some("colour") {
            parts.push("Colour".to_string());
        } else if let Some(t) = d.temp_slider {
            // Displayed at the nearest stop (v1.3.1), like the
            // temp sliders themselves.
            parts.push(format!("{}K", temp_display_kelvin(t)));
        }
    }
    parts.join(" · ")
}

/// Custom toggle switch — 48×27, same geometry as the web edition's switch.
fn toggle_switch(ui: &mut Ui, on: bool, enabled: bool, accent: Color32, track: Color32) -> bool {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(48.0, 27.0), sense);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(14), if on { accent } else { track });
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
    p.rect_filled(track_rect, CornerRadius::same(3), track);
    // The PAINT ratio clamps to the track: a value below the
    // range (e.g. a device reporting under a slider's floor)
    // parks the thumb at the end instead of drawing it off the
    // track — `out.value` itself stays the honest value.
    let ratio = if max > min { ((out.value - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
    let thumb_x = rect.left() + 9.0 + ratio * (rect.width() - 18.0);
    if ratio > 0.0 {
        let fill_rect = egui::Rect::from_min_max(
            pos2(rect.left(), mid_y - 3.0),
            pos2(thumb_x, mid_y + 3.0),
        );
        p.rect_filled(fill_rect, CornerRadius::same(3), accent);
    }
    p.circle_filled(pos2(thumb_x, mid_y), 9.0, Color32::WHITE);
    if !enabled {
        p.rect_filled(rect, CornerRadius::same(3), Color32::from_rgba_unmultiplied(0, 0, 0, 60));
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
    /// A press that hasn't yet earned drag status (see PendingDrag).
    drag_pending: Option<PendingDrag>,
    // startup & tray (v1.2.1; macOS v1.4.0): the tray icon once
    // created (stays for the session), whether the window is
    // currently hidden in the tray, whether a real Quit is in
    // flight (bypasses close-to-tray), and the launch mode the
    // process was started with.
    tray: Option<TrayState>,
    window_hidden: bool,
    quitting: bool,
    arg_launch_mode: LaunchMode,
    /// The EFFECTIVE launch mode (v1.2.4): argv flag if present,
    /// else the saved Settings launch mode. Drives the initial
    /// hidden state and the launch hold below.
    launch_mode: LaunchMode,
    /// Launch hold (v1.2.4): while true, every frame re-asserts
    /// the window invisible. Set when the effective launch mode
    /// is Minimized; cleared only by a real Show action
    /// (show_from_tray), so an arg-less autostart/restartable-app
    /// relaunch can neither flash nor stick open.
    launch_hidden_pending: bool,
    /// In-flight snap-to-monitor job (v1.3.1): created by a
    /// Monitor-menu click — or, silently, by the maximized-launch
    /// monitor restore (v1.5.0) — advanced one step per frame by
    /// handle_window_chrome (see advance_snap), never persisted.
    snap_job: Option<SnapJob>,
    /// One-shot maximized-launch monitor check (v1.5.0): once
    /// true, the startup restore has run for this process (armed
    /// a silent snap or decided none was needed) and never runs
    /// again.
    startup_monitor_checked: bool,
    /// HMONITOR the window was last seen on (Windows; 0 =
    /// unknown yet) — the remember side of the maximized-launch
    /// monitor feature. Compared every frame; a change persists
    /// the new display's device identity.
    last_seen_monitor: isize,
    /// Launch-window self-check state (v2.0.1, Windows): the
    /// bounded re-verification of a Maximized launch against
    /// its remembered monitor (see StartupCheckState).
    startup_check: StartupCheckState,
    // rate-limit backoff (v1.2.0): while set, autorefresh skips
    // discovery cycles instead of warning the user.
    disc_backoff_until: Option<Instant>,
    disc_freq_streak: u32,
    // Dynamic normal-refresh interval (v2.0.6): the timer's cadence,
    // doubling on discovery throttling (floor 30 s, cap 5 min) and
    // halving back toward the floor on success. Runtime-only —
    // restarts begin at the floor and re-adapt within a few cycles.
    disc_interval_secs: u64,
    // live camera previews (v1.2.0): frame pumps keyed "cam:{id}"
    // (tiles) and "player" (the big player window).
    pumps: HashMap<String, Pump>,
    /// Force-retry requests for pump keys (v1.2.2): filled by the
    /// tile Retry button, Watch/tile clicks, a successful player
    /// connection and window focus/visible kicks; consumed (and
    /// cleared) by sync_pumps each frame.
    pump_force: HashSet<String>,
    /// Per-camera edit generation ("cam:{id}" -> counter, v1.2.2):
    /// bumped on every camera edit save and folded into the pump
    /// source key, so saving a camera restarts its pump immediately
    /// (backoff reset) even when the id — or even the URL — is
    /// unchanged on paper.
    cam_gen: HashMap<String, u64>,
    /// Last textures of tile pumps displaced by the big player
    /// (v1.2.2): the tile keeps showing the frozen frame while the
    /// player holds the camera's RTSP session.
    held_textures: HashMap<String, egui::TextureHandle>,
    /// Previous frame's viewport focus, for the focus-gained
    /// backoff kick (v1.2.2).
    prev_focused: bool,
    paused_cams: HashSet<String>,
    player_for: Option<String>,
    /// ffmpeg lookup cache: (override string it was computed for, hit).
    ffmpeg_cache: Option<(String, Option<PathBuf>)>,
    // thermostat (HomeKit/HAP, native editions only): the setup code
    // lives here at runtime ONLY and is never persisted (it is
    // cleared after every pairing attempt); only the derived
    // long-term keys in cfg.thermostat are saved, same trust model
    // as the "keep me logged in" Tuya credentials.
    tstat_setup_code: String,
    /// Action-busy (v1.6.4 split): a user-initiated thermostat
    /// operation is running — discovery or pairing. Only this
    /// flag shows "Working…" on the card and in Settings. (Sets
    /// used to set it too; since v2.0.1 they use tstat_set_busy
    /// below, so tile taps never show the indicator.)
    tstat_busy: bool,
    /// Set-busy (v2.0.1 split): a tile or bridge set is running.
    /// Serialization only — it never shows "Working…" anywhere
    /// and never disables a button (v2.0.2): overlapping taps
    /// paint at once and queue in `tstat_pending_set` below
    /// instead of being dropped, and the tile itself updates
    /// instantly from the optimistic patch instead of waiting
    /// for the round-trip.
    tstat_set_busy: bool,
    /// Queued tile tap (v2.0.2 never-disable rule): the latest
    /// tap that arrived while an operation was running, merged
    /// last-write-wins per field, sent by
    /// `drain_tstat_pending_set` as soon as the channel is free.
    tstat_pending_set: Option<crate::hap::ThermostatChanges>,
    /// Set-failure revert guard (v2.0.1): true from a SetFailed
    /// result until the prompt re-fetch it triggers resolves. While
    /// set, a Fetched/FetchFailed result reverts the tile's state
    /// but leaves the set error line alone, so the failure stays
    /// visible instead of being cleared by the revert itself.
    tstat_revert_pending: bool,
    /// Fetch-busy (v1.6.4 split): a status fetch is running
    /// (autorefresh, manual Refresh, post-pairing, bridge Status,
    /// optimistic re-fetch). Fetches are silent: no "Working…"
    /// text anywhere, and the Refresh button stays enabled while
    /// one runs (v2.0.2). All three flags together keep operations
    /// serialized exactly as the single flag did before.
    tstat_fetch_busy: bool,
    tstat_state: Option<crate::hap::ThermostatState>,
    tstat_error: Option<String>,
    tstat_discovered: Vec<crate::hap::DiscoveredDevice>,
    /// Open pair-setup session for a rotating-code thermostat (see
    /// hap::begin_pair_setup): holds the live connection, the salt
    /// and server key from M2, and the client's ephemeral pair. The
    /// setup code is NEVER stored here — it arrives only at finish,
    /// typed from the code the thermostat shows during the attempt.
    /// Dropping this value closes the socket (Cancel = drop).
    tstat_pending: Option<crate::hap::PendingPairSetup>,
    tstat_tx: Sender<TstatResult>,
    tstat_rx: Receiver<TstatResult>,
    tstat_last_fetch: Option<Instant>,
    /// Jump-to-Thermostat request for Settings (v2.0.2): set by
    /// the edit bar's "+ Add thermostat" button; consumed by
    /// view_settings, which scrolls the Thermostat (HomeKit)
    /// panel into view once, then clears it.
    settings_jump_thermostat: bool,
    /// Web-edition bridge requests (see bridge.rs). Drained once per
    /// frame in poll_net and answered through the same thermostat
    /// spawns as the desktop tile — never a parallel session.
    bridge_rx: Receiver<bridge::BridgeJob>,
    bridge_pending: Option<BridgePending>,
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
        Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/DejaVuSans.ttf"))),
    );
    fonts.font_data.insert(
        "noto-symbols".to_string(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSansSymbols-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "noto-symbols2".to_string(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSansSymbols2-Regular.ttf"
        ))),
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

/// A press on a card that MIGHT become a drag (v1.2.1). egui starts
/// a `Sense::drag()` widget's drag the moment the pointer is down —
/// before any movement — and the previous code promoted that
/// straight into `App.drag`, which made `display_ids` remove the
/// card from its section on the very next frame. A click on any
/// in-card control (a camera's Edit/Delete buttons, say) spans the
/// press frame and the release frame, so the control vanished
/// mid-click and `clicked()` never fired: buttons rendered fine and
/// did nothing. A press is therefore held as PENDING — the layout
/// does not change — until the pointer has genuinely travelled;
/// only then does it become a real drag. A release before that
/// point leaves the click to the control under the pointer.
struct PendingDrag {
    id: String,
    is_scene: bool,
    from_section: String,
    size: egui::Vec2,
    grab_offset: egui::Vec2,
    start_idx: usize,
    press_pos: egui::Pos2,
}

/// How far the pointer must travel from the press point before a
/// pending press turns into a drag — comfortably beyond the jitter
/// of an ordinary click (a few px), small enough that a deliberate
/// drag feels immediate.
const DRAG_PROMOTE_PX: f32 = 8.0;

fn drag_should_promote(press: egui::Pos2, cur: egui::Pos2) -> bool {
    press.distance(cur) >= DRAG_PROMOTE_PX
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        // Let the single-instance waiter thread poke this context
        // awake when a secondary launch asks us to show (v1.2.2).
        #[cfg(target_os = "windows")]
        if let Ok(mut slot) = WAKE_CTX.lock() {
            *slot = Some(cc.egui_ctx.clone());
        }
        let cfg = load_config();
        let (tx, rx) = channel::<NetResult>();
        let (tstat_tx, tstat_rx) = channel::<TstatResult>();
        let (bridge_tx, bridge_rx) = channel::<bridge::BridgeJob>();
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
            li_keep: true,
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
            drag_pending: None,
            tray: None,
            window_hidden: false,
            quitting: false,
            arg_launch_mode: LaunchMode::Normal,
            launch_mode: LaunchMode::Normal,
            launch_hidden_pending: false,
            snap_job: None,
            startup_monitor_checked: false,
            last_seen_monitor: 0,
            startup_check: StartupCheckState::default(),
            disc_backoff_until: None,
            disc_freq_streak: 0,
            disc_interval_secs: DISC_INTERVAL_FLOOR_SECS,
            pumps: HashMap::new(),
            pump_force: HashSet::new(),
            cam_gen: HashMap::new(),
            held_textures: HashMap::new(),
            prev_focused: true,
            paused_cams: HashSet::new(),
            player_for: None,
            ffmpeg_cache: None,
            applied_style: None,
            view: View::Login,
            cfg: Config::default(),
            tstat_setup_code: String::new(),
            tstat_busy: false,
            tstat_set_busy: false,
            tstat_pending_set: None,
            tstat_revert_pending: false,
            tstat_fetch_busy: false,
            tstat_state: None,
            tstat_error: None,
            tstat_discovered: Vec::new(),
            tstat_pending: None,
            tstat_tx,
            tstat_rx,
            tstat_last_fetch: None,
            settings_jump_thermostat: false,
            bridge_rx,
            bridge_pending: None,
        };
        // The loopback bridge for the web edition's thermostat tile:
        // always running (discovery/pairing need it while unpaired),
        // never blocking this thread. A bind failure only disables
        // web-edition control; bridge.rs reports it on stderr.
        bridge::start_bridge(bridge_tx);
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
        // Startup & tray (v1.2.1): record how we were launched, let
        // the autostart store correct the Start-with-Windows prefs, and
        // create the tray icon when this session needs one — the
        // app started minimized (its ONLY way back is the tray) or
        // close-to-tray is on. Once created it stays for the
        // session (see the Settings help text).
        app.arg_launch_mode =
            launch_mode_from_args(&std::env::args().collect::<Vec<String>>());
        reconcile_autostart(&mut app.cfg);
        // Effective launch mode (v1.2.4): the argv flag if one was
        // given, else the saved launch mode — which the reconcile
        // above just re-derived from the Run entry when one
        // exists. window_hidden and the launch hold follow the
        // EFFECTIVE mode, so an arg-less launch (Start menu,
        // restartable-apps relaunch) with "Minimized (to tray)"
        // saved starts hidden too.
        app.launch_mode = resolve_launch_mode(
            app.arg_launch_mode,
            LaunchMode::parse(&app.cfg.prefs.launch_mode),
        );
        app.window_hidden = app.launch_mode == LaunchMode::Minimized;
        app.launch_hidden_pending = app.window_hidden;
        if app.window_hidden || app.cfg.prefs.close_to_tray {
            app.ensure_tray();
        }
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
        let mut style = (*ctx.global_style()).clone();
        let mut v = egui::Visuals::dark();
        v.panel_fill = pal.bg;
        v.window_fill = pal.card;
        v.extreme_bg_color = pal.bg;
        v.faint_bg_color = pal.card;
        v.override_text_color = Some(pal.text);
        v.hyperlink_color = accent;
        v.selection.bg_fill = self.accent_soft();
        v.selection.stroke = Stroke::new(1.0, accent);
        v.window_corner_radius = CornerRadius::same(R_CARD);
        v.menu_corner_radius = CornerRadius::same(R_BTN);
        let widget = |bg: Color32, stroke_c: Color32, fg: Color32| egui::style::WidgetVisuals {
            bg_fill: bg,
            weak_bg_fill: bg,
            bg_stroke: Stroke::new(1.0, stroke_c),
            fg_stroke: Stroke::new(1.0, fg),
            corner_radius: CornerRadius::same(R_BTN),
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
        ctx.set_global_style(style);
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
        // The login-time discovery merges against last-known data
        // too (v1.2.2): v1.2.1 passed an EMPTY previous list here,
        // so any re-login (token expiry, cold start) let a thin
        // payload replace the full device state wholesale — and the
        // result was then persisted as the new cache / merge anchor.
        let previous = self.raw_devices.clone();
        thread::spawn(move || {
            let client = http_client();
            // A fresh sign-in counts as an auth attempt (v2.0.5):
            // record it whether it succeeds or fails, so the next
            // immediate refresh does not re-login and trip the
            // 180 s wall.
            let auth = net_auth(&client, &creds, &plain_pw, &prefs);
            REFRESH_RELOGIN_LAST_AUTH_MS.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
            let msg = match auth {
                Ok(session) => match net_discovery(&client, &session, &prefs, &previous) {
                    Ok(devices) => NetResult::LoginOk { session, devices },
                    Err(DiscoveryFail::Frequent) => NetResult::LoginOk { session, devices: Vec::new() },
                    Err(DiscoveryFail::Msg(e)) | Err(DiscoveryFail::Auth(e)) => NetResult::LoginErr(e),
                },
                Err(e) => NetResult::LoginErr(e),
            };
            let _ = tx.send(msg);
        });
    }

    fn spawn_discovery(&mut self, force: bool) {
        let Some(session) = self.session.clone() else {
            // Diagnostics (v2.0.2): a skipped cycle must name its
            // gate in the log — a silent early return here looks
            // exactly like a frozen dashboard otherwise.
            crate::hap::tstat_log("discovery skipped: no session");
            return;
        };
        if self.disc_busy {
            crate::hap::tstat_log("discovery skipped: already running");
            return;
        }
        self.disc_busy = true;
        let tx = self.tx.clone();
        let prefs = self.cfg.prefs.clone();
        let creds = self.creds_for_net();
        let previous = self.raw_devices.clone();
        thread::spawn(move || {
            let client = http_client();
            // v2.0.4, gated in v2.0.5: per-refresh background re-login
            // (see the header above the helpers). Only a fresh session
            // returns current states, so re-login with the saved
            // credentials before discovery — skipping the refresh
            // token, which is what goes stale — but only when the
            // 180 s gate is open; otherwise discovery runs on the
            // existing session. The existing
            // fresh_session → discovery chain below is unchanged:
            // it runs on the fresh session when re-login succeeded
            // (fresh_session returns it as-is, since it is nowhere
            // near expiry) and on the existing session when
            // re-login was skipped or failed. The v2.0.2 Auth
            // retry and v2.0.3 merge-preserve behavior are
            // preserved because that chain is untouched.
            let mut session = session;
            match &creds {
                Some((c, pw)) => {
                    // A re-login attempt clears the no-creds hint
                    // flag, so a later creds-absent stretch logs
                    // its hint again.
                    REFRESH_RELOGIN_NO_CREDS_HINTED
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    let now = now_ms();
                    if refresh_relogin_gate_open(
                        now,
                        REFRESH_RELOGIN_LAST_AUTH_MS
                            .load(std::sync::atomic::Ordering::Relaxed),
                        REFRESH_RELOGIN_THROTTLE_UNTIL_MS
                            .load(std::sync::atomic::Ordering::Relaxed),
                    ) {
                        // Record the attempt BEFORE the call: Tuya
                        // counts the auth.do call itself against
                        // the 180 s limit, whatever its outcome.
                        REFRESH_RELOGIN_LAST_AUTH_MS
                            .store(now, std::sync::atomic::Ordering::Relaxed);
                        match net_auth(&client, c, pw, &prefs) {
                            Ok(s) => {
                                session = s;
                                // Gated outcomes are logged (v2.0.5):
                                // at ~180 s cadence this is ~20
                                // lines/hour, affordable in the
                                // 200-entry log, and proves the
                                // cadence in the field.
                                crate::hap::tstat_log(
                                    "background re-login ok — fresh session for discovery",
                                );
                            }
                            Err(e) => {
                                if is_throttle_error(&e) {
                                    REFRESH_RELOGIN_THROTTLE_UNTIL_MS.store(
                                        now + REFRESH_RELOGIN_BACKOFF_MS,
                                        std::sync::atomic::Ordering::Relaxed,
                                    );
                                    crate::hap::tstat_log(&format!(
                                        "background re-login throttled ({e}) — re-login paused for 5 minutes, using existing session"
                                    ));
                                } else {
                                    crate::hap::tstat_log(&format!(
                                        "background re-login failed: {e} — using existing session"
                                    ));
                                }
                            }
                        }
                    }
                    // else: gate closed (inside the 180 s window
                    // or the throttle window) — silently use the
                    // existing session; the engagement line already
                    // logged when the throttle window opened.
                }
                None => {
                    // No saved login: re-login is impossible. Name
                    // it once per stretch instead of every cycle;
                    // the DiscErr path still names it whenever a
                    // session actually expires with no saved login.
                    if !REFRESH_RELOGIN_NO_CREDS_HINTED
                        .swap(true, std::sync::atomic::Ordering::Relaxed)
                    {
                        crate::hap::tstat_log(
                            "refresh without saved login — background re-login unavailable (sign in again with Keep me logged in)",
                        );
                    }
                }
            }
            let msg = match fresh_session(&client, &session, &creds, &prefs, force) {
                Ok(s1) => match net_discovery(&client, &s1, &prefs, &previous) {
                    Ok(devices) => NetResult::DiscOk { session: s1, devices },
                    Err(DiscoveryFail::Frequent) => NetResult::DiscFrequent { session: s1 },
                    Err(DiscoveryFail::Auth(e)) => {
                        // Background re-auth (v2.0.2): the session
                        // is dead, so simulate sign-out/sign-in — a
                        // full login with the saved credentials,
                        // skipping the refresh token (which is what
                        // just failed). One attempt, then one
                        // retry; never a loop. No sign-in form is
                        // ever shown on this path.
                        crate::hap::tstat_log(&format!(
                            "discovery auth failure: {e} — background re-login"
                        ));
                        match &creds {
                            Some((c, pw)) => match {
                                // Record the attempt: a dead-session
                                // recovery auth counts against
                                // Tuya's 180 s limit exactly like a
                                // gated re-login, so the gate must
                                // see it (v2.0.5).
                                REFRESH_RELOGIN_LAST_AUTH_MS.store(
                                    now_ms(),
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                                net_auth(&client, c, pw, &prefs)
                            } {
                                Ok(s2) => {
                                    crate::hap::tstat_log(
                                        "background re-login ok — retrying discovery",
                                    );
                                    match net_discovery(&client, &s2, &prefs, &previous) {
                                        Ok(devices) => {
                                            NetResult::DiscOk { session: s2, devices }
                                        }
                                        Err(DiscoveryFail::Frequent) => {
                                            NetResult::DiscFrequent { session: s2 }
                                        }
                                        Err(DiscoveryFail::Auth(e2))
                                        | Err(DiscoveryFail::Msg(e2)) => {
                                            crate::hap::tstat_log(&format!(
                                                "discovery failed after background re-login: {e2}"
                                            ));
                                            NetResult::DiscErr(e2)
                                        }
                                    }
                                }
                                Err(ae) => {
                                    crate::hap::tstat_log(&format!(
                                        "background re-login failed: {ae}"
                                    ));
                                    NetResult::DiscErr(e)
                                }
                            },
                            None => {
                                crate::hap::tstat_log(
                                    "session expired, no saved login — background re-auth unavailable (sign in again with Keep me logged in)",
                                );
                                NetResult::DiscErr(e)
                            }
                        }
                    }
                    Err(DiscoveryFail::Msg(e)) => {
                        // One retry after a forced re-auth (like the web edition).
                        match fresh_session(&client, &s1, &creds, &prefs, true) {
                            Ok(s2) => match net_discovery(&client, &s2, &prefs, &previous) {
                                Ok(devices) => NetResult::DiscOk { session: s2, devices },
                                Err(DiscoveryFail::Frequent) => {
                                    NetResult::DiscFrequent { session: s2 }
                                }
                                Err(DiscoveryFail::Msg(e2)) => NetResult::DiscErr(e2),
                                Err(DiscoveryFail::Auth(e2)) => NetResult::DiscErr(e2),
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
                        concat!("Homestead/", env!("CARGO_PKG_VERSION")),
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
        // update file this release is supposed to ship for THIS
        // platform (the Setup .exe on Windows, the .dmg on macOS).
        if info.asset_name != expected_update_asset_name(&info.version) {
            self.update_error = Some(format!(
                "Unexpected update file name “{}” — refusing to download it. Use the releases page instead.",
                info.asset_name
            ));
            return;
        }
        self.update_error = None;
        self.update_dl = Some((0, info.asset_size));
        let tx = self.tx.clone();
        let url = info.asset_url.clone();
        let name = info.asset_name.clone();
        std::thread::spawn(move || {
            let result: Result<std::path::PathBuf, String> = (|| {
                let mut resp = http_client()
                    .get(&url)
                    .header(
                        "User-Agent",
                        concat!("Homestead/", env!("CARGO_PKG_VERSION")),
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
        // Preserve last-known parsed state when a merged entry
        // still carries no usable state data (missing, empty, or
        // non-object data, or a data object without a "state"
        // key). This is the poisoned-anchor case the raw merge
        // cannot repair — the previous raw entry itself lacks
        // data, so it drops out of the merge — and parsing it
        // would regress a known-good state to Off on missing
        // data. Only lights and switches are retained (their
        // state is the displayed power flag); scenes and other
        // kinds parse normally so renames and fresh readings
        // still flow. Explicit state values always flow through
        // parse_device, so genuine on/off changes are unaffected.
        // The retained device's own raw (which optimistic
        // controls keep in sync) becomes the stored entry, so
        // the anchor and the cache heal instead of staying
        // poisoned.
        let prev_by_id: HashMap<&str, &Device> =
            self.devices.iter().map(|d| (d.id.as_str(), d)).collect();
        let mut kept: Vec<String> = Vec::new();
        let mut stored: Vec<Value> = Vec::with_capacity(raw.len());
        let mut parsed: Vec<Device> = Vec::with_capacity(raw.len());
        for r in raw {
            let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let usable_state = r
                .get("data")
                .and_then(|v| v.as_object())
                .map(|m| m.contains_key("state"))
                .unwrap_or(false);
            if !usable_state {
                if let Some(prev) = prev_by_id.get(id.as_str()) {
                    if prev.kind == "light" || prev.kind == "switch" {
                        kept.push(prev.name.clone());
                        stored.push(prev.raw.clone());
                        parsed.push((*prev).clone());
                        continue;
                    }
                }
            }
            parsed.push(parse_device(&r));
            stored.push(r);
        }
        if !kept.is_empty() {
            crate::hap::tstat_log(&format!(
                "display: kept last-known state for {} (merged entry has no state data)",
                kept.join(", ")
            ));
        }
        self.raw_devices = stored;
        self.devices = parsed;
        self.prune_layout();
        // Cache the stored list (with retained raws), not the
        // incoming one, so the cache anchor heals instead of
        // staying poisoned by data-less entries.
        self.cfg.cache_devices = Some(Value::Array(self.raw_devices.clone()));
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
        lay.cam_sizes.retain(|id, _| ids.contains(id.as_str()));
        // Drop section-sequence entries for groups that no longer
        // exist; built-ins stay (their sections come and go — e.g.
        // Pinned — and the user's place for them must survive).
        let group_keys: HashSet<String> =
            lay.groups.iter().map(|g| format!("group:{}", g.id)).collect();
        lay.section_seq.retain(|k| {
            !k.starts_with("group:") || group_keys.contains(k)
        });
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
            // NEVER fall through to a blank "Add camera" form here:
            // the user clicked Edit on a specific card, and quietly
            // opening an Add form reads as "Edit doesn't work" (and a
            // Save would duplicate instead of updating). Say so.
            self.toast("Couldn't find that camera — it may already have been deleted.", true);
            return;
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
        // Only claim success when a record actually went away —
        // previously this toasted "Camera deleted" even when the id
        // matched nothing, so a failed delete looked identical to a
        // successful one.
        if remove_camera_record(&mut self.cfg.cameras, &mut self.cfg.layout, item_id).is_none() {
            self.toast("Couldn't find that camera — it may already have been deleted.", true);
            return;
        }
        // Its pumps stop on the next sync_pumps pass (no longer
        // desired); close the player if it was showing this camera.
        if self.player_for.as_deref() == Some(item_id) {
            self.player_for = None;
        }
        self.paused_cams.remove(item_id);
        self.cam_gen.remove(item_id);
        self.held_textures.remove(item_id);
        self.pump_force.remove(item_id);
        self.save();
        self.toast("Camera deleted", false);
    }

    // ---------------- live camera previews (v1.2.0) ----------------

    /// ffmpeg location, cached per override string (the lookup stats
    /// a handful of paths; the Settings field is the only input that
    /// changes, besides the files themselves).
    fn ffmpeg_path(&mut self) -> Option<PathBuf> {
        let key = self.cfg.prefs.ffmpeg_path.clone();
        if let Some((k, v)) = &self.ffmpeg_cache {
            if *k == key {
                return v.clone();
            }
        }
        let found = find_ffmpeg(&key);
        self.ffmpeg_cache = Some((key, found.clone()));
        found
    }

    /// Read-only view of the cached lookup (rendering paths). Stale
    /// entries (override just edited) resolve on the spot — the cache
    /// itself is refreshed by ffmpeg_path() from sync_pumps each frame
    /// before anything renders.
    fn ffmpeg_cached(&self) -> Option<PathBuf> {
        match &self.ffmpeg_cache {
            Some((k, v)) if *k == self.cfg.prefs.ffmpeg_path => v.clone(),
            _ => find_ffmpeg(&self.cfg.prefs.ffmpeg_path),
        }
    }

    /// Ask sync_pumps to restart one pump NOW, failure streak
    /// reset (v1.2.2 manual/implicit retry).
    fn kick_pump(&mut self, key: &str) {
        self.pump_force.insert(key.to_string());
    }

    /// Kick every pump currently in Backoff — the window just
    /// became visible/focused again, so feeds that dropped while
    /// the user wasn't looking get an immediate retry instead of
    /// waiting out their delay (v1.2.2 reset trigger).
    fn kick_all_backoff(&mut self) {
        let keys: Vec<String> = self
            .pumps
            .iter()
            .filter(|(_, p)| p.state == PumpState::Backoff)
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            self.pump_force.insert(k);
        }
    }

    /// Reconcile running pumps with what's wanted — called every
    /// frame from update(), before rendering:
    ///  - a TILE pump per camera while previews are on, the camera
    ///    isn't hidden/paused, and the big player isn't open for it
    ///    (tile_pump_wanted — the player takes the RTSP session);
    ///  - the PLAYER pump while the player window is open (the
    ///    camera's current quality selection).
    /// Pump sources are derived fresh from the CURRENT camera
    /// records every frame, and the source key additionally carries
    /// the camera's edit generation: any change (address, port,
    /// template, credentials, size, ffmpeg path, an edit save)
    /// restarts the pump immediately with its failure streak reset
    /// (pump_action). Backoff retries fire on schedule — a wake
    /// repaint is armed for the earliest one so an idle window
    /// can't strand a feed. One pump per key, never duplicates.
    fn sync_pumps(&mut self, ctx: &egui::Context) {
        let ff = self.ffmpeg_path();
        let mut desired: HashMap<String, String> = HashMap::new(); // key -> source key
        let mut specs: HashMap<String, (String, usize, usize)> = HashMap::new(); // key -> (url, w, h)
        if let Some(ffp) = &ff {
            for cam in &self.cfg.cameras {
                let item = format!("cam:{}", cam.id);
                let gen = self.cam_gen.get(&item).copied().unwrap_or(0);
                if tile_pump_wanted(
                    self.cfg.prefs.previews,
                    self.cfg.layout.hidden.contains(&item),
                    self.paused_cams.contains(&item),
                    self.player_for.as_deref() == Some(&item),
                ) {
                    // The tile pump follows the card's size (v1.2.1):
                    // Normal 480×270 SD, Large 960×540 SD, Wide
                    // 960×540 at the card's selected quality. The
                    // source key includes url+w+h+gen, so a resize
                    // or an edit restarts the pump on its own.
                    let (url, w, h) = tile_feed(cam, cam_size_of(&self.cfg.layout, &item));
                    desired.insert(
                        item.clone(),
                        format!("{}|g{}", pump_source_key(ffp, &url, w, h), gen),
                    );
                    specs.insert(item, (url, w, h));
                }
            }
            if let Some(item) = &self.player_for {
                if let Some(ci) = self.camera_index(item) {
                    let cam = self.cfg.cameras[ci].clone();
                    let gen = self.cam_gen.get(item).copied().unwrap_or(0);
                    let url = build_rtsp_url(&cam);
                    desired.insert(
                        "player".to_string(),
                        format!(
                            "{}|g{}",
                            pump_source_key(ffp, &url, PLAYER_W, PLAYER_H),
                            gen
                        ),
                    );
                    specs.insert("player".to_string(), (url, PLAYER_W, PLAYER_H));
                }
            }
        }
        if !self.cfg.prefs.previews {
            self.held_textures.clear();
        }
        // Stop pumps that are no longer wanted at all. A tile pump
        // displaced by the big player keeps its last texture in
        // held_textures, so the tile shows the frozen frame (not a
        // blank "Connecting…") while the player holds the session.
        let keys: Vec<String> = self.pumps.keys().cloned().collect();
        for k in keys {
            if !desired.contains_key(&k) {
                if let Some(mut p) = self.pumps.remove(&k) {
                    if self.player_for.as_deref() == Some(k.as_str()) {
                        if let Some(tex) = p.texture.take() {
                            self.held_textures.insert(k.clone(), tex);
                        }
                    }
                    p.stop();
                }
            }
        }
        // Start / restart pumps per the pure decision (pump_action).
        if let Some(ffp) = &ff {
            let now = Instant::now();
            for (k, want) in &desired {
                let (state, matches, due) = match self.pumps.get(k) {
                    None => (None, true, false),
                    Some(p) => (
                        Some(p.state),
                        p.source_key == *want,
                        p.retry_at.map(|t| now >= t).unwrap_or(false),
                    ),
                };
                let force = self.pump_force.contains(k);
                match pump_action(state, matches, force, due) {
                    PumpAction::Keep => {}
                    action => {
                        if let Some((url, w, h)) = specs.get(k) {
                            let mut np =
                                spawn_pump(ffp, url, *w, *h, want.clone(), ctx);
                            if let Some(mut old) = self.pumps.remove(k) {
                                if action == PumpAction::RetryDue {
                                    np.streak = old.streak;
                                }
                                np.texture = old.texture.take();
                                old.stop();
                            }
                            if np.texture.is_none() {
                                np.texture = self.held_textures.remove(k);
                            }
                            self.pumps.insert(k.clone(), np);
                        }
                    }
                }
            }
        }
        self.pump_force.clear();
        // Drain frame channels: keep only the LATEST frame per pump.
        let mut any_active = false;
        let mut next_retry: Option<Instant> = None;
        for (k, p) in self.pumps.iter_mut() {
            let mut latest: Option<Vec<u8>> = None;
            let mut ended: Option<Option<String>> = None;
            while let Ok(m) = p.rx.try_recv() {
                match m {
                    FrameMsg::Frame(f) => latest = Some(f),
                    FrameMsg::Ended(reason) => ended = Some(reason),
                }
            }
            if let Some(f) = latest {
                let img = egui::ColorImage::from_rgba_unmultiplied([p.w, p.h], &f);
                match &mut p.texture {
                    Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                    None => {
                        p.texture = Some(ctx.load_texture(
                            format!("pump-{}", k),
                            img,
                            egui::TextureOptions::LINEAR,
                        ))
                    }
                }
                p.note_frame();
                // A player connection that succeeds proves the
                // camera is reachable again — reset the TILE pump's
                // backoff too (v1.2.2 reset trigger).
                if k == "player" {
                    if let Some(item) = self.player_for.clone() {
                        self.pump_force.insert(item);
                    }
                }
            }
            if let Some(reason) = ended {
                p.note_ended(reason);
            }
            if p.state != PumpState::Backoff {
                any_active = true;
            } else if let Some(t) = p.retry_at {
                next_retry = Some(next_retry.map(|x: Instant| x.min(t)).unwrap_or(t));
            }
        }
        if any_active {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if let Some(t) = next_retry {
            // Arm a wake-up for the earliest scheduled retry so it
            // fires on time even on an otherwise idle window.
            ctx.request_repaint_after(backoff_wake_delay(Some(t), Instant::now()));
        }
    }

    /// Hand a camera's stream to the configured external player app.
    fn open_external_player(&mut self, item_id: &str) {
        let Some(ci) = self.camera_index(item_id) else { return };
        let cam = self.cfg.cameras[ci].clone();
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

    /// Watch a camera: gateway watch page in the default browser when
    /// a stream gateway is configured; else the in-app player when
    /// ffmpeg is available (v1.2.0); else the external player.
    fn watch_camera(&mut self, item_id: &str) {
        let Some(ci) = self.camera_index(item_id) else { return };
        // Clicking Watch is a backoff reset trigger (v1.2.2): the
        // user is actively trying to see this camera — retry now.
        self.kick_pump(item_id);
        let cam = self.cfg.cameras[ci].clone();
        if let Some(url) = gateway_watch_url(&cam, &self.cfg.prefs) {
            open_external(&url);
            self.toast("Stream opened in your browser", false);
            return;
        }
        if self.ffmpeg_path().is_some() {
            self.player_for = Some(item_id.to_string());
            return;
        }
        self.open_external_player(item_id);
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
            self.drag_pending = None;
            return;
        }
        // Pending press (v1.2.1): promote only after real travel;
        // a release first means it was a click on a card control —
        // drop the candidate WITHOUT touching the layout, which is
        // exactly what lets the control's own click land.
        if let Some(p) = &self.drag_pending {
            if released {
                self.drag_pending = None;
            } else if let Some(cur) = pos {
                if drag_should_promote(p.press_pos, cur) {
                    let p = self.drag_pending.take().unwrap();
                    self.drag = Some(DragState {
                        id: p.id.clone(),
                        is_scene: p.is_scene,
                        from_section: p.from_section.clone(),
                        size: p.size,
                        grab_offset: p.grab_offset,
                        target: Some((p.from_section.clone(), p.start_idx)),
                    });
                }
            }
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

    /// Create the tray icon if this session should have one and
    /// doesn't yet. Once created it stays until the app exits —
    /// the least surprising rule, and the one the Settings panel
    /// documents.
    fn ensure_tray(&mut self) {
        if self.tray.is_none() {
            self.tray = build_tray();
            // v1.2.3: the moment the icon exists, its events are
            // owned by the dedicated tray thread — never by the
            // frame loop (see the Bug C note at TRAY_QUEUE).
            if let Some(tray) = &self.tray {
                maybe_spawn_tray_thread(tray);
            }
        }
    }

    /// macOS tray events (v1.4.0): drain both of tray-icon's
    /// receivers into TRAY_QUEUE, mapped through the same pure
    /// functions the Windows tray thread uses. Called from
    /// handle_window_chrome just before the queue drain, so a
    /// mapped action lands in the same frame it was polled.
    #[cfg(target_os = "macos")]
    fn poll_tray_events_macos(&mut self) {
        let Some(tray) = &self.tray else { return };
        let (open_id, quit_id) = (tray.open_id.clone(), tray.quit_id.clone());
        let menu_rx = tray_icon::menu::MenuEvent::receiver();
        while let Ok(ev) = menu_rx.try_recv() {
            if let Some(a) = tray_action_for_menu(ev.id(), &open_id, &quit_id) {
                push_tray_action(a);
            }
        }
        let icon_rx = tray_icon::TrayIconEvent::receiver();
        while let Ok(ev) = icon_rx.try_recv() {
            use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
            let (dbl, left, up) = match &ev {
                TrayIconEvent::DoubleClick { button, .. } => {
                    (true, *button == MouseButton::Left, false)
                }
                TrayIconEvent::Click {
                    button,
                    button_state,
                    ..
                } => (
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

    /// The real quit path (v1.2.3, extracted unchanged from the
    /// tray-Quit arm): drop the icon FIRST so no phantom icon is
    /// left behind, stop the pumps, save, and REALLY exit — the
    /// quitting flag keeps close-to-tray from swallowing the
    /// close. Runs on the UI thread, from the tray queue.
    fn quit_now(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        self.tray = None;
        for p in self.pumps.values() {
            p.stop();
        }
        self.save();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Bring the window back from the tray (v1.2.2): visible,
    /// UN-minimized and focused — v1.2.1 sent Visible + Focus only,
    /// which left a taskbar-minimized window sitting minimized.
    /// Restoring also kicks backed-off camera pumps: the user is
    /// looking again, so feeds retry immediately.
    fn show_from_tray(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        self.window_hidden = false;
        // A real Show action: the v1.2.4 launch hold is over.
        self.launch_hidden_pending = false;
        self.kick_all_backoff();
    }

    /// Launch hold (v1.2.4): while a Minimized launch hasn't been
    /// shown by a real Show action yet, re-assert the window
    /// invisible. main() already starts the viewport invisible,
    /// but that single builder flag is the ONLY thing that used
    /// to keep an autostart launch hidden — if the window got
    /// shown anyway during startup, nothing ever re-hid it. Runs
    /// AFTER the frame's show drains in handle_window_chrome, so
    /// a Show landing in the same frame wins.
    fn enforce_launch_hold(&self, ctx: &egui::Context) {
        if self.launch_hidden_pending {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }

    /// One-shot maximized-launch monitor restore (v1.5.0,
    /// Windows): on the first frame, if this launch is Maximized
    /// and a monitor was remembered, and that display still
    /// exists and is NOT the one the window opened on, start a
    /// SILENT snap to it — the same restore -> physical-pixel
    /// move -> verify -> maximize machine as a menu snap, so
    /// the end state is maximized on a live monitor by OS
    /// construction. No geometry is stored or restored, and a
    /// missing display is a no-op, so the v1.2.5 off-screen
    /// failure mode cannot recur.
    #[cfg(target_os = "windows")]
    fn maybe_restore_startup_monitor(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if self.startup_monitor_checked {
            return;
        }
        self.startup_monitor_checked = true;
        if self.launch_mode != LaunchMode::Maximized {
            return;
        }
        let saved = self.cfg.prefs.last_monitor_device.clone();
        if saved.is_empty() {
            return;
        }
        // v2.0.1: arm the launch-window self-check alongside the
        // one-shot restore. The check stays live even when this
        // first frame arms no snap — the window may not be
        // ready yet (no HWND), the display may enumerate late,
        // or the window may open on target and drift during
        // startup settling — and the per-frame check covers all
        // three holes the one-shot check left open.
        self.startup_check = StartupCheckState {
            active: true,
            deadline: Instant::now() + STARTUP_CHECK_WINDOW,
            device: saved.clone(),
            ..StartupCheckState::default()
        };
        if self.snap_job.is_some() {
            return;
        }
        let Some(hwnd) = main_hwnd(frame) else {
            return;
        };
        let monitors = enumerate_monitors();
        let Some(target) = monitors.iter().find(|m| m.device == saved) else {
            return;
        };
        if win32_window_monitor(hwnd) == target.handle {
            return;
        }
        self.snap_job = Some(SnapJob {
            silent: true,
            target: target.clone(),
            phase: SnapPhase::LeaveStates,
            attempts: 0,
            deadline: Instant::now() + SNAP_TIMEOUT,
        });
        ctx.request_repaint();
    }

    /// Launch-window self-check (v2.0.1, Windows): called every
    /// frame from handle_window_chrome after advance_snap, so it
    /// sees post-snap state. Re-verifies a Maximized launch
    /// against its remembered monitor: resolves on arrival,
    /// stops permanently on a user takeover or when the window
    /// runs out, and otherwise re-arms ONE silent snap on a
    /// settled off-target reading. Silent throughout — no
    /// toasts, no user-visible messages — and never past the
    /// launch window.
    #[cfg(target_os = "windows")]
    fn startup_monitor_self_check(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if !self.startup_check.active {
            return;
        }
        // A snap ending this frame starts the settle pause;
        // the transition is observed here, every frame.
        let snap_active = self.snap_job.is_some();
        if self.startup_check.prev_snap && !snap_active {
            self.startup_check.settle_until = Some(Instant::now() + STARTUP_CHECK_SETTLE);
        }
        self.startup_check.prev_snap = snap_active;
        let Some(hwnd) = main_hwnd(frame) else {
            // Window not ready yet; the window still bounds
            // the wait, so just keep waiting.
            return;
        };
        // Resolve the intended monitor's handle once; a fresh
        // enumeration also covers a display that appears late.
        if self.startup_check.target == 0 {
            let monitors = enumerate_monitors();
            if let Some(m) = monitors.iter().find(|m| m.device == self.startup_check.device) {
                self.startup_check.target = m.handle;
            }
        }
        // The user-move signal: a top-left change with no snap
        // in flight means something other than the check moved
        // the window. The baseline updates every frame, so a
        // snap's own moves never read as user moves.
        let pos = win32_window_pos_px(hwnd);
        let moved = self.startup_check.last_pos.map(|p| p != pos).unwrap_or(false);
        self.startup_check.last_pos = Some(pos);
        let cur = win32_window_monitor(hwnd);
        let facts = StartupCheckFacts {
            snap_active,
            expired: Instant::now() >= self.startup_check.deadline,
            maximized: win32_is_maximized(hwnd),
            on_target: self.startup_check.target != 0 && cur == self.startup_check.target,
            settle_passed: self
                .startup_check
                .settle_until
                .map(|t| Instant::now() >= t)
                .unwrap_or(true),
            moved,
        };
        let (action, seen) = startup_check_next(
            facts,
            self.startup_check.rearm_used,
            self.startup_check.seen_maximized,
            self.startup_check.last_snap_failed,
        );
        self.startup_check.seen_maximized = seen;
        match action {
            StartupCheckAction::Wait => {}
            StartupCheckAction::Resolve | StartupCheckAction::StopUserTakeover => {
                self.startup_check.active = false;
                self.startup_check.last_snap_failed = false;
            }
            StartupCheckAction::ExpireUnresolved => {
                self.startup_check.active = false;
                self.startup_check.unresolved = true;
                self.startup_check.unresolved_monitor = cur;
                self.startup_check.last_snap_failed = false;
            }
            StartupCheckAction::ArmSnap => {
                // Fresh enumeration: the target must still
                // exist, and its handle may have changed since
                // the check resolved it.
                let monitors = enumerate_monitors();
                let Some(target) = monitors
                    .iter()
                    .find(|m| m.device == self.startup_check.device)
                    .cloned()
                else {
                    // Display gone; wait for the window to run
                    // out (a missing display is a no-op).
                    return;
                };
                self.startup_check.rearm_used = true;
                self.startup_check.last_snap_failed = false;
                self.snap_job = Some(SnapJob {
                    silent: true,
                    target,
                    phase: SnapPhase::LeaveStates,
                    attempts: 0,
                    deadline: Instant::now() + SNAP_TIMEOUT,
                });
                ctx.request_repaint();
            }
        }
    }

    /// Remember which monitor the window is on (v1.5.0,
    /// Windows): called every frame from handle_window_chrome
    /// while no snap is in flight. A changed HMONITOR resolves
    /// to its device identity and persists it — continuously, so
    /// even a crash or forced kill leaves the right monitor
    /// remembered. Never runs while a snap job exists: a move in
    /// flight would otherwise record the monitor being left.
    #[cfg(target_os = "windows")]
    fn remember_window_monitor(&mut self, frame: &eframe::Frame) {
        if self.snap_job.is_some() {
            return;
        }
        // v2.0.1: while the launch self-check is active, the
        // window may sit on a wrong monitor transiently around
        // a snap — recording that monitor would overwrite the
        // intended one the check is still trying to reach.
        if self.startup_check.active {
            return;
        }
        let Some(hwnd) = main_hwnd(frame) else {
            return;
        };
        let cur = win32_window_monitor(hwnd);
        if cur == 0 || cur == self.last_seen_monitor {
            return;
        }
        // v2.0.1: the check expired with the window still off
        // target. The failed landing must not overwrite the
        // intended monitor — but any later monitor change
        // means a deliberate move or a display change, so it
        // clears the latch and recording resumes from then on.
        if self.startup_check.unresolved {
            if cur == self.startup_check.unresolved_monitor {
                return;
            }
            self.startup_check.unresolved = false;
            self.startup_check.unresolved_monitor = 0;
        }
        self.last_seen_monitor = cur;
        let monitors = enumerate_monitors();
        if let Some(m) = monitors.iter().find(|m| m.handle == cur) {
            if self.cfg.prefs.last_monitor_device != m.device {
                self.cfg.prefs.last_monitor_device = m.device.clone();
                save_config(&self.cfg);
            }
        }
    }

    /// Snap to monitor: act on a display chosen in the header
    /// menu. The list is RE-ENUMERATED here, at click time — a
    /// display unplugged since the menu opened turns the click
    /// into a no-op with a note, never a blind move. The click
    /// itself only STARTS a SnapJob (v1.3.1): the actual move
    /// is the per-frame state machine in advance_snap — direct
    /// Win32 in physical pixels, arrival verified before
    /// maximizing. (v1.3.0's one-frame burst of viewport
    /// commands never moved the window on a mixed-DPI desk;
    /// see the snap section header.) Nothing is persisted;
    /// startup is unaffected.
    fn snap_to_monitor(&mut self, ctx: &egui::Context, chosen_device: &str) {
        let monitors = enumerate_monitors();
        let Some(m) = monitors.iter().find(|m| m.device == chosen_device) else {
            self.toast("That display is no longer available", false);
            return;
        };
        self.snap_job = Some(SnapJob {
            silent: false,
            target: m.clone(),
            phase: SnapPhase::LeaveStates,
            attempts: 0,
            deadline: Instant::now() + SNAP_TIMEOUT,
        });
        ctx.request_repaint();
    }

    /// Advance an in-flight snap by ONE step (v1.3.1), called
    /// every frame from handle_window_chrome. The decision is
    /// the pure snap_next; this gathers the frame's facts and
    /// performs the decided effect. A step per frame lets each
    /// window-state transition fully land before the next, and
    /// the move is verified (MonitorFromWindow) before anything
    /// maximizes — the two things the v1.3.0 burst never did.
    fn advance_snap(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(job) = &self.snap_job else { return };
        let (phase, attempts, target, deadline, silent) = (
            job.phase,
            job.attempts,
            job.target.clone(),
            job.deadline,
            job.silent,
        );
        let Some(hwnd) = main_hwnd(frame) else {
            ctx.request_repaint();
            return;
        };
        let facts = SnapFacts {
            fullscreen: ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)),
            zoomed: win32_needs_restore(hwnd),
            on_target: win32_window_monitor(hwnd) == target.handle,
            expired: Instant::now() >= deadline,
        };
        let (next, action) = snap_next(phase, facts, attempts);
        match action {
            SnapAction::SendFullscreenOff => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
            SnapAction::Restore => win32_restore_window(hwnd),
            SnapAction::Wait => {}
            SnapAction::MoveWindow => {
                let size = win32_window_size_px(hwnd);
                let pt = snap_target_point_px(&target, size);
                win32_move_window(hwnd, pt[0], pt[1]);
                if let Some(job) = &mut self.snap_job {
                    job.attempts += 1;
                }
            }
            SnapAction::MaximizeAndFinish => {
                win32_maximize_window(hwnd);
                self.snap_job = None;
                if silent {
                    // A silent startup snap that landed clears
                    // the failure flag (see the self-check's
                    // takeover rule).
                    self.startup_check.last_snap_failed = false;
                }
                if !silent {
                    self.toast(format!("Snapped to {}", target.name), false);
                }
                return;
            }
            SnapAction::Fail => {
                self.snap_job = None;
                if silent {
                    // A silent startup snap that failed leaves
                    // a restored window that looks exactly like
                    // a user takeover — the self-check must
                    // know it was the snap, so it re-arms
                    // instead of stopping.
                    self.startup_check.last_snap_failed = true;
                }
                if !silent {
                    self.toast(
                        format!("Couldn't move to {} — window left as-is", target.name),
                        true,
                    );
                }
                return;
            }
        }
        if let Some(job) = &mut self.snap_job {
            job.phase = next;
        }
        ctx.request_repaint();
    }

    /// Tray actions + window-close behaviour, run every frame
    /// before rendering:
    ///  - tray "Open" / icon left-click, or a secondary launch's
    ///    ShowWindow signal → restore the window (show_from_tray);
    ///  - tray "Quit" → the real quit path (quit_now);
    ///  - a window close with close-to-tray on → cancel the close
    ///    and hide instead, with the one-time balloon;
    ///  - focus regained → backed-off camera pumps retry at once.
    /// v1.2.3: tray events arrive via TRAY_QUEUE, fed by the
    /// dedicated tray thread — this drain is only the UI-thread
    /// half. It still needs a frame to run, which is why the
    /// thread ALSO restores/shows the window directly in Win32:
    /// that both satisfies Open immediately and wakes the frame
    /// pipeline so a queued Quit executes at once. The 200 ms
    /// hidden heartbeat below stays as belt-and-braces for the
    /// cases where frames do run while hidden.
    fn handle_window_chrome(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        // Publish the HWND every frame for the tray thread's
        // direct Win32 actions (v1.2.3).
        #[cfg(target_os = "windows")]
        if let Some(hwnd) = main_hwnd(frame) {
            MAIN_HWND.store(hwnd, Ordering::SeqCst);
        }
        if SHOW_REQUESTED.swap(false, Ordering::SeqCst) {
            self.show_from_tray(ctx);
        }
        // macOS (v1.4.0): tray events are polled here, into the
        // same queue the drain below consumes (no tray thread —
        // see the note at build_tray).
        #[cfg(target_os = "macos")]
        self.poll_tray_events_macos();
        // macOS single instance (v1.4.0): a secondary launch
        // signals by file (no kernel event to wait on). Poll for
        // it throttled — the hidden heartbeat keeps frames coming
        // while hidden, which is when this matters.
        #[cfg(target_os = "macos")]
        {
            static LAST_POLL_MS: std::sync::atomic::AtomicI64 =
                std::sync::atomic::AtomicI64::new(0);
            let now = now_ms();
            if now - LAST_POLL_MS.load(Ordering::SeqCst) >= 400 {
                LAST_POLL_MS.store(now, Ordering::SeqCst);
                let path = show_request_path();
                if path.exists() {
                    let _ = std::fs::remove_file(&path);
                    self.show_from_tray(ctx);
                }
            }
        }
        for action in drain_tray_actions() {
            match action {
                TrayAction::Show => self.show_from_tray(ctx),
                TrayAction::Quit => self.quit_now(ctx),
            }
        }
        // Launch hold (v1.2.4): after this frame's Show signals
        // have had their chance, a still-pending Minimized launch
        // re-asserts its invisibility.
        self.enforce_launch_hold(ctx);
        // Snap to monitor (v1.3.1): advance an in-flight snap
        // one step per frame — the HWND was just published
        // above, fresh for this frame.
        // Maximized-launch monitor memory (v1.5.0, Windows):
        // one-shot restore check, then the every-frame remember
        // side — in this order, so the first frame never records
        // the monitor the window merely opened on over the one
        // it should move to (the remember side skips while a
        // snap job is in flight).
        #[cfg(target_os = "windows")]
        self.maybe_restore_startup_monitor(ctx, frame);
        #[cfg(target_os = "windows")]
        self.remember_window_monitor(frame);
        self.advance_snap(ctx, frame);
        // Launch-window self-check (v2.0.1, Windows): re-verify
        // the maximized launch against its remembered monitor
        // and re-arm one silent snap on a settled miss — never
        // past the launch window, never against a user move.
        #[cfg(target_os = "windows")]
        self.startup_monitor_self_check(ctx, frame);
        // Focus regained (clicking back into the window) is a
        // backoff reset trigger for camera feeds (v1.2.2).
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused && !self.prev_focused {
            self.kick_all_backoff();
        }
        self.prev_focused = focused;
        if ctx.input(|i| i.viewport().close_requested()) {
            if close_action(self.cfg.prefs.close_to_tray, self.quitting)
                == CloseAction::HideToTray
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                self.window_hidden = true;
                self.ensure_tray();
                if !self.cfg.prefs.tray_notice_shown {
                    self.cfg.prefs.tray_notice_shown = true;
                    self.save();
                    if let Some(hwnd) = main_hwnd(frame) {
                        show_tray_balloon(hwnd);
                    }
                }
            }
        }
        if self.window_hidden {
            ctx.request_repaint_after(Duration::from_millis(200));
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
                SectionKind::Scenes | SectionKind::Thermostat => {}
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
        let Some(idx) = self.device_index(id) else { return };
        // Only reachable from controls gated on a native
        // brightness field (v1.3.0) — the command is always a
        // plain brightnessSet; no colour-route synthesis.
        let backup = self.backup_data(idx);
        let value = brightness_set_value(pct);
        self.apply_group_target(idx, "brightnessSet", &value);
        self.spawn_control(id.to_string(), "brightnessSet", value, backup, false);
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
                // The colorSet payload still carries an HSV
                // brightness component for the DEVICE (the API
                // requires the field), but it is not recorded as a
                // brightness reading (v1.3.0): a colour-only device
                // has no brightness state in this app — no slider,
                // no group average — because the API never reports
                // one back.
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
    /// sequentially (~200 ms apart) and reports back a GroupDone
    /// summary. A member Tuya rate-limits is retried silently after
    /// ~1 s inside net_control (that wait is its extra spacing), so
    /// the summary only ever names genuine failures (v1.2.0).
    fn group_broadcast(&mut self, sec: &Section, kind: GroupKind, num: i64, hex: Option<String>) {
        let full = build_sections(&self.all_entries(), &self.cfg.layout, self.editing, "", self.cfg.thermostat.is_some());
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

    /// Tuya rate-limited a discovery call: never warn (v1.2.0) — the
    /// app keeps showing the last known states and stretches the
    /// effective refresh interval instead. Unified with the dynamic
    /// interval (v2.0.6): the throttle doubles the CURRENT interval
    /// (floor 60 s on the first hit, cap 5 min) and the streak
    /// backoff is set to that same value, so the timer's two waits
    /// can never disagree — the effective wait is max(interval,
    /// backoff) and both derive from the one doubling. Success
    /// halves the interval back toward the 30 s floor (see the
    /// DiscOk handler); the streak counter itself only feeds the
    /// diagnostics log line. Returns the new interval in seconds.
    fn note_rate_limited(&mut self) -> u64 {
        self.disc_freq_streak = self.disc_freq_streak.saturating_add(1);
        let old = self.disc_interval_secs;
        let secs = disc_interval_after_throttle(old);
        self.disc_interval_secs = secs;
        self.disc_backoff_until = Some(Instant::now() + Duration::from_secs(secs));
        if secs != old {
            crate::hap::tstat_log(&format!(
                "refresh interval now {secs}s (discovery throttled)"
            ));
        }
        secs
    }

    /// Discovery succeeded: step the dynamic interval back toward
    /// the 30 s floor (halving, never below it) and log the change.
    /// Called on DiscOk and on login-time discovery success — a
    /// successful fetch is a successful fetch either way.
    fn note_discovery_success(&mut self) {
        let old = self.disc_interval_secs;
        let next = disc_interval_after_success(old);
        if next != old {
            self.disc_interval_secs = next;
            crate::hap::tstat_log(&format!(
                "refresh interval now {next}s (discovery healthy)"
            ));
        }
    }

    // ---------------- thermostat (HomeKit/HAP) spawns ----------------
    // Each runs on its own thread and reports over tstat_tx; the UI
    // never blocks on the LAN. A busy flag serialises operations so
    // two pair-verify sessions never interleave on one pairing.

    fn spawn_tstat_discover(&mut self) {
        if self.tstat_busy || self.tstat_fetch_busy || self.tstat_set_busy {
            return;
        }
        self.tstat_busy = true;
        self.tstat_error = None;
        let tx = self.tstat_tx.clone();
        thread::spawn(move || {
            let res = crate::hap::discover(Duration::from_secs(8));
            let _ = tx.send(match res {
                Ok(devs) => TstatResult::Discovered(devs),
                Err(e) => TstatResult::Failed(format!("Discovery failed: {}", e)),
            });
        });
    }

    /// Step 1 of two-step pairing (rotating-code thermostats): open
    /// the pair-setup session (M1 sent, M2 received). No code is
    /// needed here — the thermostat shows its fresh code during the
    /// attempt, and the user types it at Finish (step 2). The
    /// pending session is delivered back as TstatResult::PairBegun
    /// and held in App.tstat_pending until Finish or Cancel.
    fn spawn_tstat_begin(&mut self, device: crate::hap::DiscoveredDevice) {
        if self.tstat_busy || self.tstat_fetch_busy || self.tstat_set_busy {
            return;
        }
        self.tstat_busy = true;
        self.tstat_error = None;
        let tx = self.tstat_tx.clone();
        thread::spawn(move || {
            let res = crate::hap::begin_pair_setup(&device);
            let _ = tx.send(match res {
                Ok(p) => TstatResult::PairBegun { pending: p },
                Err(e) => TstatResult::Failed(format!("Pairing start failed: {}", e)),
            });
        });
    }

    /// Step 2 of two-step pairing: finish the session opened by
    /// spawn_tstat_begin with the code the thermostat is showing
    /// now. Takes (and thereby clears) the pending session, so a
    /// failed finish always returns the panel to Start pairing.
    fn spawn_tstat_finish(&mut self) {
        if self.tstat_busy || self.tstat_fetch_busy || self.tstat_set_busy {
            return;
        }
        let Some(pending) = self.tstat_pending.take() else {
            return;
        };
        let code = self.tstat_setup_code.clone();
        self.tstat_busy = true;
        self.tstat_error = None;
        let tx = self.tstat_tx.clone();
        thread::spawn(move || {
            let res = crate::hap::finish_pair_setup(pending, &code);
            let _ = tx.send(match res {
                Ok(p) => TstatResult::Paired(p),
                Err(e) => TstatResult::Failed(format!("Pairing failed: {}", e)),
            });
        });
    }

    /// Status fetch (v1.6.4 busy split): sets fetch-busy only, so the
    /// card and Settings stay silent — no "Working…" text. Returns
    /// false when an operation is already running (the caller that
    /// needs a guaranteed re-fetch, the optimistic-confirm handler,
    /// runs after the current operation clears instead).
    fn spawn_tstat_fetch(&mut self) -> bool {
        if self.tstat_busy || self.tstat_fetch_busy || self.tstat_set_busy {
            return false;
        }
        let Some(pairing) = self.cfg.thermostat.clone() else {
            return false;
        };
        self.tstat_fetch_busy = true;
        // Attempt time, not just success time: a failed fetch must not
        // make the autorefresh below retry every frame.
        self.tstat_last_fetch = Some(Instant::now());
        let tx = self.tstat_tx.clone();
        thread::spawn(move || {
            let res = crate::hap::fetch_state(&pairing);
            let _ = tx.send(match res {
                Ok(st) => TstatResult::Fetched(st),
                Err(e) => TstatResult::FetchFailed(format!("Thermostat refresh failed: {}", e)),
            });
        });
        true
    }

    /// Tile tap helper (v2.0.2 never-disable rule): a tap ALWAYS
    /// paints at once, even while a set, fetch, or discovery is
    /// running — the buttons never grey out and taps are never
    /// dropped. When the channel is free the set spawns at once;
    /// when it is busy the tap merges into `tstat_pending_set`
    /// (last-write-wins per field) and goes out as soon as the
    /// running operation clears. The authoritative result replaces
    /// the patch when it arrives, and a real failure with no
    /// queued tap reverts it via re-fetch.
    fn tstat_tap(&mut self, changes: crate::hap::ThermostatChanges) {
        if let Some(st) = self.tstat_state.as_mut() {
            crate::hap::apply_changes_to_state(st, &changes);
        }
        self.tstat_error = None;
        if !self.spawn_tstat_set(changes.clone()) {
            self.tstat_pending_set = Some(match self.tstat_pending_set.take() {
                Some(pending) => Self::merge_tstat_pending(pending, changes),
                None => changes,
            });
        }
    }

    /// Merge a queued tap into the pending intent (v2.0.2): later
    /// taps win per field, untouched fields carry over — so a mode
    /// tap followed by a setpoint tap while a set is in flight
    /// sends both, not just the last tap's single field.
    fn merge_tstat_pending(
        base: crate::hap::ThermostatChanges,
        overlay: crate::hap::ThermostatChanges,
    ) -> crate::hap::ThermostatChanges {
        crate::hap::ThermostatChanges {
            target_mode: overlay.target_mode.or(base.target_mode),
            target_temp_c: overlay.target_temp_c.or(base.target_temp_c),
            heat_to_c: overlay.heat_to_c.or(base.heat_to_c),
            cool_to_c: overlay.cool_to_c.or(base.cool_to_c),
            fan_on: overlay.fan_on.or(base.fan_on),
        }
    }

    /// Send the queued tile tap, if any, once the channel is free
    /// (v2.0.2): no operation running, no bridge job outstanding,
    /// and no pairing session open — a set must never interleave
    /// with pair-setup. Called after every thermostat result is
    /// handled, so a tap queued during a slow or timing-out set
    /// goes out the moment that set resolves.
    fn drain_tstat_pending_set(&mut self) {
        if self.tstat_busy
            || self.tstat_fetch_busy
            || self.tstat_set_busy
            || self.bridge_pending.is_some()
            || self.tstat_pending.is_some()
        {
            return;
        }
        if let Some(changes) = self.tstat_pending_set.take() {
            let _ = self.spawn_tstat_set(changes);
        }
    }

    /// Short plain reason for a thermostat set failure (v2.0.3):
    /// the tile must never show a raw OS error dump for a failed
    /// set (a timeout used to surface as a full Winsock paragraph
    /// while the wall unit had applied the change). The full error
    /// still goes to the diagnostics log at the hap layer; the tile
    /// — and the web edition via the bridge error body — shows only
    /// this short reason.
    fn humanize_tstat_error(e: &str) -> String {
        let lower = e.to_lowercase();
        if lower.contains("os error")
            || lower.contains("network error")
            || lower.contains("timed out")
            || lower.contains("timeout")
            || lower.contains("connection")
            || lower.contains("unreachable")
            || lower.contains("reset by peer")
            || lower.contains("broken pipe")
        {
            "the thermostat could not be reached — check that it is online and try again"
                .to_string()
        } else if lower.contains("rejected")
            || lower.contains("invalid value")
            || lower.contains("-70410")
        {
            "the thermostat did not accept the change".to_string()
        } else {
            "the thermostat did not confirm the change — try again".to_string()
        }
    }

    /// Tile/bridge set (v2.0.1 busy split): sets set-busy only, so
    /// the card never shows "Working…" for a set — the tile updates
    /// instantly from the caller's optimistic patch instead. Returns
    /// true when the set actually spawned; a false return means
    /// the tile handler queues the tap in `tstat_pending_set`
    /// (v2.0.2) instead of dropping it.
    fn spawn_tstat_set(&mut self, changes: crate::hap::ThermostatChanges) -> bool {
        if self.tstat_busy || self.tstat_fetch_busy || self.tstat_set_busy {
            return false;
        }
        let Some(pairing) = self.cfg.thermostat.clone() else {
            return false;
        };
        self.tstat_set_busy = true;
        let tx = self.tstat_tx.clone();
        thread::spawn(move || {
            let res = crate::hap::set_state(&pairing, &changes);
            let _ = tx.send(match res {
                Ok(crate::hap::SetOutcome::Confirmed(st)) => TstatResult::State(st),
                Ok(crate::hap::SetOutcome::Optimistic(st)) => TstatResult::StateConfirm(st),
                Err(e) => TstatResult::SetFailed(format!(
                    "Thermostat control failed: {}",
                    Self::humanize_tstat_error(&e.to_string())
                )),
            });
        });
        true
    }

    // ---------------- thermostat bridge (web edition) ----------------
    // Requests arrive on bridge_rx and are answered through the SAME
    // spawns above: one pairing store (cfg.thermostat), one busy
    // flag, one result channel. The bridge thread only transports
    // JSON; it never touches the thermostat or the config itself.
    // A bridge job that finds the thermostat busy gets a 409 at
    // once rather than queuing behind the desktop operation.

    fn bridge_busy_reply(reply: Sender<bridge::BridgeResponse>) {
        let _ = reply.send(bridge::BridgeResponse {
            status: 409,
            body: json!({ "error": "thermostat is busy — try again in a moment" }),
        });
    }

    fn poll_bridge(&mut self) {
        while let Ok(job) = self.bridge_rx.try_recv() {
            self.dispatch_bridge(job);
        }
    }

    fn dispatch_bridge(&mut self, job: bridge::BridgeJob) {
        let reply = job.reply;
        match job.request {
            bridge::BridgeRequest::Status => {
                if self.cfg.thermostat.is_none() {
                    let _ = reply.send(bridge::BridgeResponse {
                        status: 200,
                        body: json!({ "paired": false }),
                    });
                } else if self.tstat_busy || self.tstat_set_busy || self.bridge_pending.is_some() {
                    Self::bridge_busy_reply(reply);
                } else {
                    self.bridge_pending = Some(BridgePending {
                        reply,
                        kind: BridgePendingKind::Status,
                    });
                    self.spawn_tstat_fetch();
                }
            }
            bridge::BridgeRequest::Discover => {
                if self.tstat_busy || self.tstat_set_busy || self.bridge_pending.is_some() {
                    Self::bridge_busy_reply(reply);
                } else {
                    self.bridge_pending = Some(BridgePending {
                        reply,
                        kind: BridgePendingKind::Discover,
                    });
                    self.spawn_tstat_discover();
                }
            }
            bridge::BridgeRequest::PairSave { pairing } => {
                // The bridge ran the two-step pairing on its own
                // threads (pair/begin + pair/finish); this just
                // persists the result exactly like a desktop
                // pairing success. Any desktop pairing session
                // open at the same time is dropped (its socket
                // closes) — the bridge pairing supersedes it.
                self.tstat_pending = None;
                let name = pairing.accessory_name.clone();
                let model = pairing.accessory_model.clone();
                self.cfg.thermostat = Some(pairing);
                self.save();
                self.toast("Thermostat paired over HomeKit", false);
                self.spawn_tstat_fetch();
                let _ = reply.send(bridge::BridgeResponse {
                    status: 200,
                    body: json!({ "ok": true, "name": name, "model": model }),
                });
            }
            bridge::BridgeRequest::Set(req) => {
                if self.cfg.thermostat.is_none() {
                    let _ = reply.send(bridge::BridgeResponse {
                        status: 400,
                        body: json!({ "error": "thermostat is not paired" }),
                    });
                } else if self.tstat_busy || self.tstat_set_busy || self.bridge_pending.is_some() {
                    Self::bridge_busy_reply(reply);
                } else {
                    let changes = crate::hap::ThermostatChanges {
                        target_mode: req.mode,
                        target_temp_c: req.target_f.map(crate::hap::f_to_c),
                        heat_to_c: req.heat_to_f.map(crate::hap::f_to_c),
                        cool_to_c: req.cool_to_f.map(crate::hap::f_to_c),
                        fan_on: req.fan_on,
                    };
                    self.bridge_pending = Some(BridgePending {
                        reply,
                        kind: BridgePendingKind::Set,
                    });
                    let _ = self.spawn_tstat_set(changes);
                }
            }
            bridge::BridgeRequest::Forget => {
                // Exactly the desktop Forget button's semantics:
                // drop this computer's keys and save. The
                // thermostat side is cleared with HomeKit Reset on
                // the unit; the reply carries that note verbatim.
                self.cfg.thermostat = None;
                self.tstat_state = None;
                self.save();
                let _ = reply.send(bridge::BridgeResponse {
                    status: 200,
                    body: json!({
                        "ok": true,
                        "note": "This only forgets the keys on this computer. To clear the thermostat side, use HomeKit Reset on the unit (Menu → Reset → HomeKit Reset). It never touches WiFi, schedule, or the Resideo link.",
                    }),
                });
            }
        }
    }

    fn poll_net(&mut self) {
        self.poll_bridge();
        while let Ok(msg) = self.tstat_rx.try_recv() {
            match msg {
                TstatResult::Discovered(devs) => {
                    self.tstat_busy = false;
                    self.tstat_discovered = devs;
                    if let Some(p) = self.bridge_pending.take() {
                        if p.kind == BridgePendingKind::Discover {
                            let _ = p.reply.send(bridge::BridgeResponse {
                                status: 200,
                                body: bridge::devices_to_json(&self.tstat_discovered),
                            });
                        } else {
                            // Cannot happen while tstat_busy
                            // serialises operations, but never
                            // silently drop a reply channel.
                            self.bridge_pending = Some(p);
                        }
                    }
                }
                TstatResult::PairBegun { pending } => {
                    self.tstat_busy = false;
                    self.tstat_setup_code.clear();
                    self.tstat_error = None;
                    // The session is open and the thermostat is
                    // showing its fresh code: the panel now prompts
                    // for it (Finish pairing / Cancel).
                    self.tstat_pending = Some(pending);
                }
                TstatResult::Paired(p) => {
                    self.tstat_busy = false;
                    self.tstat_setup_code.clear();
                    self.tstat_error = None;
                    self.cfg.thermostat = Some(p);
                    self.save();
                    self.toast("Thermostat paired over HomeKit", false);
                    self.spawn_tstat_fetch();
                }
                TstatResult::State(st) => {
                    self.tstat_set_busy = false;
                    self.tstat_revert_pending = false;
                    self.tstat_error = None;
                    self.tstat_last_fetch = Some(Instant::now());
                    let state_json = bridge::state_to_json(&st);
                    self.tstat_state = Some(st);
                    match self.bridge_pending.take() {
                        Some(bp) if bp.kind == BridgePendingKind::Status => {
                            let _ = bp.reply.send(bridge::BridgeResponse {
                                status: 200,
                                body: json!({ "paired": true, "state": state_json }),
                            });
                        }
                        Some(bp) if bp.kind == BridgePendingKind::Set => {
                            let _ = bp.reply.send(bridge::BridgeResponse {
                                status: 200,
                                body: json!({ "ok": true, "state": state_json }),
                            });
                        }
                        other => self.bridge_pending = other,
                    }
                }
                TstatResult::Fetched(st) => {
                    // Fetch-busy only: a fetch result never clears
                    // the action flag (see Fetched's docs).
                    self.tstat_fetch_busy = false;
                    if self.tstat_revert_pending {
                        // Revert fetch after a set failure: the state
                        // reverts to the unit's truth, but the set
                        // error line stays — otherwise the revert
                        // itself would erase the failure it explains.
                        self.tstat_revert_pending = false;
                    } else {
                        self.tstat_error = None;
                    }
                    self.tstat_last_fetch = Some(Instant::now());
                    let state_json = bridge::state_to_json(&st);
                    self.tstat_state = Some(st);
                    match self.bridge_pending.take() {
                        Some(bp) if bp.kind == BridgePendingKind::Status => {
                            let _ = bp.reply.send(bridge::BridgeResponse {
                                status: 200,
                                body: json!({ "paired": true, "state": state_json }),
                            });
                        }
                        other => self.bridge_pending = other,
                    }
                }
                TstatResult::FetchFailed(e) => {
                    self.tstat_fetch_busy = false;
                    if self.tstat_revert_pending {
                        // Revert fetch failed too: keep the original
                        // set error on the desktop line (it is the
                        // more relevant failure); the bridge reply
                        // below still carries this fetch's error for
                        // its own requester.
                        self.tstat_revert_pending = false;
                    } else {
                        self.tstat_error = Some(e.clone());
                    }
                    match self.bridge_pending.take() {
                        Some(bp) if bp.kind == BridgePendingKind::Status => {
                            // A failed Status fetch still proves the
                            // app is paired (dispatch only fetches when
                            // paired), so the web page can show
                            // "paired, no reading" instead of "unpaired".
                            let mut body = json!({ "error": e });
                            body["paired"] = json!(true);
                            let _ = bp.reply.send(bridge::BridgeResponse {
                                status: 502,
                                body,
                            });
                        }
                        other => self.bridge_pending = other,
                    }
                }
                TstatResult::StateConfirm(st) => {
                    // Optimistic set: the PUT applied, the readback
                    // didn't. Show the intended state now, then
                    // re-fetch promptly to confirm against the unit,
                    // unless a tap was queued while the set ran (the
                    // drain after this message sends it instead).
                    // The re-fetch spawns here only with no queued
                    // tap: the set just cleared set-busy and no
                    // fetch can be running (operations are
                    // serialized across all flags).
                    self.tstat_set_busy = false;
                    self.tstat_revert_pending = false;
                    self.tstat_error = None;
                    self.tstat_last_fetch = Some(Instant::now());
                    let state_json = bridge::state_to_json(&st);
                    self.tstat_state = Some(st);
                    match self.bridge_pending.take() {
                        Some(bp) if bp.kind == BridgePendingKind::Set => {
                            let _ = bp.reply.send(bridge::BridgeResponse {
                                status: 200,
                                body: json!({ "ok": true, "state": state_json }),
                            });
                        }
                        other => self.bridge_pending = other,
                    }
                    // A queued tap supersedes the confirm re-fetch:
                    // the drain after this message sends it, and its
                    // result is authoritative. Otherwise re-fetch to
                    // confirm the optimistic state against the unit.
                    if self.tstat_pending_set.is_none() {
                        let _ = self.spawn_tstat_fetch();
                    }
                }
                TstatResult::SetFailed(e) => {
                    // A real set failure: the tile keeps the error
                    // line, and a prompt re-fetch reverts any
                    // optimistic patch to the unit's true state
                    // instead of waiting for the next autorefresh.
                    // The re-fetch always spawns here, unless a tap
                    // was queued while the set ran: then the drain
                    // after this message sends it instead, and its
                    // result is authoritative (if it fails, its own
                    // SetFailed arms the revert then).
                    self.tstat_set_busy = false;
                    self.tstat_error = Some(e.clone());
                    match self.bridge_pending.take() {
                        Some(bp) => {
                            let mut body = json!({ "error": e });
                            if bp.kind == BridgePendingKind::Status {
                                body["paired"] = json!(true);
                            }
                            let _ = bp.reply.send(bridge::BridgeResponse {
                                status: 502,
                                body,
                            });
                        }
                        None => {}
                    }
                    if self.tstat_pending_set.is_none() {
                        self.tstat_revert_pending = true;
                        let _ = self.spawn_tstat_fetch();
                    }
                }
                TstatResult::Failed(e) => {
                    self.tstat_busy = false;
                    self.tstat_error = Some(e.clone());
                    match self.bridge_pending.take() {
                        Some(bp) => {
                            // A failed Status fetch still proves the
                            // app is paired (dispatch only fetches when
                            // paired), so the web page can show
                            // "paired, no reading" instead of "unpaired".
                            let mut body = json!({ "error": e });
                            if bp.kind == BridgePendingKind::Status {
                                body["paired"] = json!(true);
                            }
                            let _ = bp.reply.send(bridge::BridgeResponse {
                                status: 502,
                                body,
                            });
                        }
                        None => {
                            // Desktop-originated failure. A failed
                            // finish leaves a stale (possibly already
                            // rotated) code in the field, so clear it
                            // and return the panel to Start pairing;
                            // in every other state the field is
                            // already empty, so this is a no-op.
                            self.tstat_setup_code.clear();
                        }
                    }
                }
            }
            // A queued tile tap goes out as soon as the operation
            // that blocked it clears (v2.0.2 never-disable rule).
            self.drain_tstat_pending_set();
        }
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
                        // The login-time discovery was rate-limited:
                        // silent (v1.2.0) — cached devices (if any) are
                        // already on screen, and the backoff lets
                        // autorefresh quietly fill the list in.
                        let secs = self.note_rate_limited();
                        crate::hap::tstat_log(&format!(
                            "login rate-limited: streak {}, backoff {}s",
                            self.disc_freq_streak, secs
                        ));
                        self.save();
                    } else {
                        crate::hap::tstat_log(&format!("login ok: {} devices", devices.len()));
                        self.note_discovery_success();
                        self.set_devices(devices, true);
                    }
                }
                NetResult::LoginErr(e) => {
                    self.login_busy = false;
                    self.pending_login = None;
                    crate::hap::tstat_log(&format!("login failed: {e}"));
                    if self.view == View::Login {
                        self.login_error = Some(e);
                    } else {
                        self.toast(e, true);
                    }
                }
                NetResult::DiscOk { session, devices } => {
                    self.disc_busy = false;
                    self.disc_freq_streak = 0;
                    self.disc_backoff_until = None;
                    self.session = Some(session.clone());
                    self.cfg.session = Some(session);
                    crate::hap::tstat_log(&format!("discovery ok: {} devices", devices.len()));
                    self.note_discovery_success();
                    self.set_devices(devices, true);
                }
                NetResult::DiscFrequent { session } => {
                    self.disc_busy = false;
                    self.session = Some(session.clone());
                    self.cfg.session = Some(session);
                    self.save();
                    // Silent (v1.2.0): keep showing the last known
                    // states and back off internally — no toast.
                    // The diagnostics log still names the gate, so a
                    // frozen dashboard is diagnosable (v2.0.2).
                    let secs = self.note_rate_limited();
                    crate::hap::tstat_log(&format!(
                        "discovery rate-limited: streak {}, backoff {}s",
                        self.disc_freq_streak, secs
                    ));
                    if !self.devices.is_empty() {
                        self.last_fetch = Some(Instant::now());
                    }
                }
                NetResult::DiscErr(e) => {
                    self.disc_busy = false;
                    crate::hap::tstat_log(&format!("discovery failed: {e}"));
                    // A "Session expired" here came from
                    // fresh_session, which returns it exactly when
                    // the refresh token is dead and no saved login
                    // exists to re-login with — so with no saved
                    // login the log must name that gate instead of
                    // leaving a frozen dashboard unexplained
                    // (v2.0.2). No sign-in form is popped: the
                    // dashboard stays up and the next cycle retries.
                    if session_expired_hint_needed(&e, self.cfg.creds.is_some()) {
                        crate::hap::tstat_log(
                            "session expired, no saved login — background re-auth unavailable (sign in again with Keep me logged in)",
                        );
                    }
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
                            // Final verification before anything runs /
                            // opens it: the file name must be exactly
                            // this release's expected update asset.
                            let expected = self
                                .update_info
                                .as_ref()
                                .map(|i| expected_update_asset_name(&i.version))
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
                                    "The downloaded file's name doesn't match the expected update file, so it was not used. Use the releases page instead."
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
        ComboBox::from_id_salt(format!("{}_proxy", id_prefix))
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
            Frame::new()
                .fill(pal.card)
                .stroke(Stroke::new(1.0, pal.border))
                .corner_radius(CornerRadius::same(R_LOGIN))
                .inner_margin(Margin { left: 28, right: 28, top: 30, bottom: 24 })
                .show(ui, |ui| {
                    ui.set_width(344.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("🏠").size(40.0));
                        ui.label(RichText::new("Homestead").size(23.0).strong());
                        ui.label(
                            RichText::new("Your Smart Life / Tuya devices, in one clean dashboard")
                                .size(13.5)
                                .color(pal.dim),
                        );
                    });
                    ui.add_space(18.0);
                    if let Some(err) = self.login_error.clone() {
                        Frame::new()
                            .fill(Color32::from_rgba_unmultiplied(0xf4, 0x3f, 0x5e, 30))
                            .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(0xf4, 0x3f, 0x5e, 102)))
                            .corner_radius(CornerRadius::same(11))
                            .inner_margin(Margin::symmetric(12, 9))
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
                    ComboBox::from_id_salt("li_region")
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
                    ComboBox::from_id_salt("li_platform")
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

    fn header(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        let accent = self.accent();
        egui::Panel::top("app-header")
            .frame(
                Frame::new()
                    .fill(pal.card)
                    .inner_margin(Margin::symmetric(18, 12)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("🏠").size(22.0));
                    ui.label(RichText::new("Homestead").size(18.0).strong());
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
                        // Snap to monitor (v1.3.0) — WINDOWS ONLY
                        // (v1.4.0): the mechanism is direct Win32,
                        // so the button doesn't exist on macOS at
                        // all (the user's multi-monitor desk is
                        // Windows; see DESIGN.md's platform matrix).
                        // The menu lists the displays enumerated
                        // live as it opens; choosing one
                        // re-validates and snaps (see
                        // snap_to_monitor). 🖥 = U+1F5A5, covered by
                        // the embedded Noto Sans Symbols 2 fallback.
                        #[cfg(target_os = "windows")]
                        {
                        let mut snap_choice: Option<String> = None;
                        ui.menu_button("🖥 Monitor", |ui| {
                            ui.set_min_width(264.0);
                            let monitors = enumerate_monitors();
                            if monitors.is_empty() {
                                ui.label(
                                    RichText::new("No displays found").color(pal.dim),
                                );
                            }
                            let centre = ui.ctx().input(|i| {
                                i.viewport().outer_rect.map(|r| [r.center().x, r.center().y])
                            });
                            let current = centre.and_then(|c| monitor_index_at(&monitors, c));
                            for (idx, m) in monitors.iter().enumerate() {
                                let mut line2 = format!(
                                    "{} × {} · {}",
                                    m.rect_px[2],
                                    m.rect_px[3],
                                    monitor_orientation(m)
                                );
                                if m.primary {
                                    line2 += " · Primary";
                                }
                                if current == Some(idx) {
                                    line2 += " · This window is here";
                                }
                                let entry = format!("{}\n{}", m.name, line2);
                                if ui.selectable_label(false, entry).clicked() {
                                    snap_choice = Some(m.device.clone());
                                    ui.close();
                                }
                            }
                        });
                        if let Some(dev) = snap_choice {
                            self.snap_to_monitor(ui.ctx(), &dev);
                        }
                        }
                        // Dynamic interval (v2.0.6): read-only —
                        // the timer adapts it, the user does not pick
                        // it. Fixed options invited throttling.
                        ui.label(
                            RichText::new(auto_interval_text(self.disc_interval_secs))
                                .size(12.0)
                                .color(pal.dim),
                        );
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
            egui::Panel::top("edit-bar")
                .frame(
                    Frame::new()
                        .fill(self.accent_soft())
                        .inner_margin(Margin::symmetric(18, 9)),
                )
                .show(ui, |ui| {
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
                            if self.cfg.thermostat.is_none()
                                && ui.add(Button::new("+ Add thermostat")).clicked()
                            {
                                self.open_thermostat_settings();
                            }
                            if ui.add(Button::new("+ New group")).clicked() {
                                self.new_group = Some(String::new());
                            }
                        });
                    });
                });
        }
    }

    /// Open Settings scrolled to the Thermostat (HomeKit) panel
    /// (v2.0.2): the edit bar's "+ Add thermostat" button. The
    /// jump itself happens in view_settings, which scrolls the
    /// panel into view once and clears the flag.
    fn open_thermostat_settings(&mut self) {
        self.view = View::Settings;
        self.settings_jump_thermostat = true;
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

    /// Thermostat (HomeKit) section on the dashboard: its own
    /// reorderable category section, like Cameras and Lighting. The
    /// header carries the ▾/▸ collapse chevron (v1.6.4, same
    /// collapsed machinery as every other section, keyed by
    /// "thermostat"), the title, the ↻ Refresh button, and (in
    /// editing mode) the ↓/↑ move buttons; the card below holds the
    /// existing thermostat controls unchanged. Collapsed, only the
    /// header renders. It has no count badge, Delete, or "Reset to
    /// name order".
    fn thermostat_section_view(&mut self, ui: &mut Ui, sec_idx: usize, sec_total: usize) {
        let paired = self.cfg.thermostat.is_some();
        let collapsed = self
            .cfg
            .layout
            .collapsed
            .get("thermostat")
            .copied()
            .unwrap_or(false);
        ui.horizontal(|ui| {
            let chev = if collapsed { "▸" } else { "▾" };
            if ui.add(Button::new(chev).frame(false)).clicked() {
                self.cfg
                    .layout
                    .collapsed
                    .insert("thermostat".to_string(), !collapsed);
                self.save();
            }
            ui.label(RichText::new("Thermostat").size(16.5).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.editing {
                    // Same move buttons as every other section
                    // (disabled at the ends); the order persists in
                    // layout.section_seq via move_section.
                    if ui
                        .add_enabled(sec_idx + 1 < sec_total, Button::new("↓").small())
                        .on_hover_text("Move section down")
                        .clicked()
                    {
                        self.move_section("thermostat", 1);
                    }
                    if ui
                        .add_enabled(sec_idx > 0, Button::new("↑").small())
                        .on_hover_text("Move section up")
                        .clicked()
                    {
                        self.move_section("thermostat", -1);
                    }
                }
                if ui
                    .add_enabled(paired, Button::new("↻ Refresh"))
                    .clicked()
                {
                    self.spawn_tstat_fetch();
                }
            });
        });
        ui.add_space(8.0);
        if collapsed {
            // Header only, like every other collapsed section.
            ui.add_space(10.0);
            return;
        }
        self.thermostat_card(ui);
        ui.add_space(14.0);
    }

    /// The thermostat card's status line (v1.6.4): shows the TARGET /
    /// set mode (what the wall unit shows), not the live idle /
    /// heating / cooling state — a unit set to Cool but idle must
    /// read "Cool", not "Off". The mode buttons already highlight
    /// the target mode; the line now agrees with them.
    fn thermostat_status_line(st: &crate::hap::ThermostatState) -> String {
        format!(
            "{} — indoor {:.0}°F, setpoint {:.0}°F{}",
            crate::hap::mode_name(st.target_mode),
            crate::hap::c_to_f(st.current_temp_c),
            crate::hap::c_to_f(st.target_temp_c),
            st.humidity
                .map(|h| format!(", humidity {:.0}%", h))
                .unwrap_or_default()
        )
    }

    /// The thermostat card itself (paired state, mode buttons,
    /// setpoint, fan, error line). The header lives in
    /// thermostat_section_view above.
    fn thermostat_card(&mut self, ui: &mut Ui) {
        let pal = self.pal();
        // Snapshot what the UI needs so button clicks below can spawn
        // threads without borrow fights. The buttons are never
        // gated (v2.0.2): a tap always paints at once and queues
        // behind a running operation instead of greying out; only
        // the action flag shows "Working…" — sets are silent since
        // v2.0.1 (the tile patches instantly instead).
        let paired = self.cfg.thermostat.is_some();
        let action_busy = self.tstat_busy;
        let state = self.tstat_state.clone();
        let error = self.tstat_error.clone();
        Frame::new()
            .fill(pal.card)
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::symmetric(16, 12))
            .show(ui, |ui| {
                if !paired {
                    ui.label(
                        RichText::new("Not paired — pair in Settings → Thermostat (HomeKit).")
                            .size(13.0)
                            .color(pal.dim),
                    );
                    ui.label(
                        RichText::new("Control works on your home network only (local, no cloud).")
                            .size(12.5)
                            .color(pal.dim),
                    );
                    return;
                }
                if action_busy {
                    ui.label(RichText::new("Working…").size(12.5).color(pal.dim));
                }
                if let Some(e) = error {
                    ui.label(RichText::new(e).size(12.5).color(DANGER));
                }
                let Some(st) = state else {
                    ui.label(
                        RichText::new("No reading yet — press ↻ Refresh.")
                            .size(13.0)
                            .color(pal.dim),
                    );
                    return;
                };
                let set_f = crate::hap::c_to_f(st.target_temp_c);
                ui.label(RichText::new(Self::thermostat_status_line(&st)).size(14.0));
                // Mode buttons (Off / Heat / Cool / Auto).
                ui.horizontal(|ui| {
                    for (label, mode) in
                        [("Off", 0u8), ("Heat", 1), ("Cool", 2), ("Auto", 3)]
                    {
                        let selected = st.target_mode == mode;
                        let text = if selected {
                            RichText::new(label).strong().color(Color32::WHITE)
                        } else {
                            RichText::new(label).strong()
                        };
                        let btn = Button::new(text);
                        let resp = if selected {
                            ui.add(btn.fill(self.accent()))
                        } else {
                            ui.add(btn)
                        };
                        if resp.clicked() {
                            self.tstat_tap(crate::hap::ThermostatChanges {
                                target_mode: Some(mode),
                                ..Default::default()
                            });
                        }
                    }
                });
                // Setpoint −/+ in 1°F steps. Auto mode adjusts its
                // heat-to / cool-to pair instead of the main setpoint.
                if st.target_mode == 3 {
                    if let Some(heat) = st.heat_to_c {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "Heat to {:.0}°F",
                                    crate::hap::c_to_f(heat)
                                ))
                                .size(14.0),
                            );
                            if ui.add(Button::new("−")).clicked() {
                                self.tstat_tap(crate::hap::ThermostatChanges {
                                    heat_to_c: Some(crate::hap::f_to_c(
                                        crate::hap::c_to_f(heat) - 1.0,
                                    )),
                                    ..Default::default()
                                });
                            }
                            if ui.add(Button::new("+")).clicked() {
                                self.tstat_tap(crate::hap::ThermostatChanges {
                                    heat_to_c: Some(crate::hap::f_to_c(
                                        crate::hap::c_to_f(heat) + 1.0,
                                    )),
                                    ..Default::default()
                                });
                            }
                        });
                    }
                    if let Some(cool) = st.cool_to_c {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "Cool to {:.0}°F",
                                    crate::hap::c_to_f(cool)
                                ))
                                .size(14.0),
                            );
                            if ui.add(Button::new("−")).clicked() {
                                self.tstat_tap(crate::hap::ThermostatChanges {
                                    cool_to_c: Some(crate::hap::f_to_c(
                                        crate::hap::c_to_f(cool) - 1.0,
                                    )),
                                    ..Default::default()
                                });
                            }
                            if ui.add(Button::new("+")).clicked() {
                                self.tstat_tap(crate::hap::ThermostatChanges {
                                    cool_to_c: Some(crate::hap::f_to_c(
                                        crate::hap::c_to_f(cool) + 1.0,
                                    )),
                                    ..Default::default()
                                });
                            }
                        });
                    }
                } else {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("Setpoint {:.0}°F", set_f)).size(14.0),
                        );
                        if ui.add(Button::new("−")).clicked() {
                            self.tstat_tap(crate::hap::ThermostatChanges {
                                target_temp_c: Some(crate::hap::f_to_c(set_f - 1.0)),
                                ..Default::default()
                            });
                        }
                        if ui.add(Button::new("+")).clicked() {
                            self.tstat_tap(crate::hap::ThermostatChanges {
                                target_temp_c: Some(crate::hap::f_to_c(set_f + 1.0)),
                                ..Default::default()
                            });
                        }
                    });
                }
                // Fan Auto / On, when the accessory exposes a fan.
                if let Some(fan_on) = st.fan_on {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Fan").size(14.0));
                        for (label, on) in [("Auto", false), ("On", true)] {
                            let selected = fan_on == on;
                            let text = if selected {
                                RichText::new(label).strong().color(Color32::WHITE)
                            } else {
                                RichText::new(label).strong()
                            };
                            let btn = Button::new(text);
                            let resp = if selected {
                                ui.add(btn.fill(self.accent()))
                            } else {
                                ui.add(btn)
                            };
                            if resp.clicked() {
                                self.tstat_tap(crate::hap::ThermostatChanges {
                                    fan_on: Some(on),
                                    ..Default::default()
                                });
                            }
                        }
                    });
                }
            });
    }

    fn view_dashboard(&mut self, ui: &mut Ui) {
        let pal = self.pal();
        let entries = self.all_entries();
        let sections = build_sections(&entries, &self.cfg.layout, self.editing, &self.query, self.cfg.thermostat.is_some());
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
                        Frame::new()
                            .fill(self.accent_soft())
                            .stroke(Stroke::new(1.0, self.accent()))
                            .corner_radius(CornerRadius::same(14))
                            .inner_margin(Margin::symmetric(16, 12))
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
                // The thermostat renders in sequence position via
                // the sections loop below (it is a section like any
                // other, not a fixed block above them) — but only
                // when paired; until then it is absent entirely.
                let avail = ui.available_width();
                if self.devices.is_empty() && self.cfg.cameras.is_empty() {
                    // No Tuya devices at all: the thermostat section
                    // renders here too when paired, then the empty
                    // note; when unpaired only the note shows.
                    for sec in &sections {
                        if sec.kind == SectionKind::Thermostat {
                            self.section_view(ui, sec, avail);
                        }
                    }
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
                if sections.is_empty()
                    || sections.iter().all(|s| s.kind == SectionKind::Thermostat)
                {
                    // Nothing survived: either the search matched no
                    // device (the paired thermostat still shows, the
                    // unpaired dashboard shows only the note), or
                    // every device section was filtered out.
                    for sec in &sections {
                        self.section_view(ui, sec, avail);
                    }
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("Nothing matches your search.").size(14.0).color(pal.dim));
                    });
                    return;
                }
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
        let full = build_sections(&self.all_entries(), &self.cfg.layout, self.editing, "", self.cfg.thermostat.is_some());
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
        // The group brightness average covers only members that
        // natively report brightness (v1.3.0) — a colour-only
        // strip never joins it (it has no brightness reading).
        let b_vals: Vec<i64> = devs
            .iter()
            .filter(|d| d.has_brightness)
            .filter_map(|d| d.brightness_pct)
            .collect();
        let t_vals: Vec<i64> = devs.iter().filter_map(|d| d.temp_slider).collect();
        let has_color = devs.iter().any(|d| d.has_color);
        let sec2 = sec.clone();
        Frame::new()
            .fill(pal.card)
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(CornerRadius::same(R_CARD))
            .inner_margin(Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width((ui.available_width() - 32.0).max(200.0));
                // v1.2.3 (Bug A): each control group is ONE
                // unbreakable vertical mini-panel of uniform
                // height (GROUP_PANEL_H); the wrapped placer
                // flows whole groups left→right and wraps whole
                // groups only. (v1.2.2 allocated the sliders at
                // height 0, which let the placer staircase and
                // interleave the label/slider pairs.)
                let gkey = format!("grp:{}", sec2.key);
                // Top-aligned wrapped row (v1.3.0): horizontal_wrapped
                // centres panels on the cross axis, which — against
                // the placer's row-height estimate — let later panels
                // drift a few points down. Align::Min pins every
                // panel to the row top, so the Brightness and Colour
                // temperature slider tracks share one baseline.
                ui.with_layout(Layout::left_to_right(Align::Min).with_main_wrap(true), |ui| {
                    ui.allocate_ui_with_layout(
                        vec2(GROUP_ALL_W, GROUP_PANEL_H),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.label(RichText::new("All").size(12.5).strong().color(pal.dim));
                            if toggle_switch(ui, any_on, true, accent, pal.track) {
                                self.group_broadcast(&sec2, GroupKind::Toggle, 0, None);
                            }
                        },
                    );
                    // Group sliders are capability-gated too
                    // (v1.2.2): shown whenever any member CAN do
                    // brightness / temp, positioned at the average
                    // of the members that currently have readings —
                    // or a neutral default when none do — never
                    // dropped just because values are missing.
                    // Brightness gates on the NATIVE field
                    // (v1.3.0): a group of nothing but colour-only
                    // strips gets no brightness slider at all —
                    // the API reports no brightness for them.
                    if devs.iter().any(|d| d.has_brightness) {
                        let avg = group_average(&b_vals).unwrap_or(100.0);
                        // The header speaks the UI scale too
                        // (v1.3.1), so it agrees with the cards:
                        // average on the device scale, map once.
                        let ui_avg = brightness_device_to_ui(avg.round() as i64);
                        self.group_slider(
                            ui, &sec2, "b", "Brightness", 1.0, 100.0, ui_avg as f32,
                            |v| format!("{}%", v.round() as i64),
                            GroupKind::Brightness,
                        );
                    }
                    if devs.iter().any(|d| d.has_temp) {
                        let avg = group_average(&t_vals).unwrap_or(TEMP_NEUTRAL_SLIDER as f64);
                        // The group temp slider sits at the stop
                        // nearest the members' average Kelvin
                        // (v1.3.1) — its value is a stop index,
                        // and a broadcast sends that exact stop.
                        let stop = temp_stop_index(temp_slider_to_kelvin(avg.round() as i64));
                        self.group_slider(
                            ui, &sec2, "t", "Colour temperature", 0.0, (TEMP_STOPS_K.len() - 1) as f32, stop as f32,
                            |v| format!("{}K", temp_stop_kelvin(v.round() as i64)),
                            GroupKind::Temp,
                        );
                    }
                    if has_color {
                        // Swatches: one unbreakable panel, one row,
                        // vertically centred like its neighbours.
                        // (The custom-colour picker renders BELOW
                        // the strip now, inside this same frame —
                        // it used to open inside the wrapped row
                        // and shove the strip around.)
                        ui.allocate_ui_with_layout(
                            vec2(GROUP_SWATCH_W, GROUP_PANEL_H),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                for (name, hex) in SWATCHES {
                                    if swatch(ui, hex, name, false, pal.text) {
                                        self.group_broadcast(&sec2, GroupKind::Color, 0, Some(hex.to_string()));
                                    }
                                }
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
                            },
                        );
                    }
                });
                if has_color && self.open_color_for.as_deref() == Some(gkey.as_str()) {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
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
                    });
                }
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
        // Real height (v1.2.3): the v1.2.2 height-0 allocation
        // was the Bug A mechanism — the wrapped placer treated
        // the group as zero-height and interleaved the next
        // group's label into this one's slider row.
        ui.allocate_ui_with_layout(vec2(GROUP_SLIDER_W, GROUP_PANEL_H), Layout::top_down(Align::Min), |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).size(12.5).color(pal.dim));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(fmt(live)).size(12.5).strong());
                });
            });
            ui.add_space(3.0);
            let out = slim_slider(ui, egui::Id::new(&key), live, min, max, true, accent, pal.track);
            if out.changed {
                // Same live semantics as the card sliders
                // (slider_row): temp moves in whole stop indices,
                // brightness in UI percent. (Pre-1.3.2 this branch
                // carried a pre-stop-scale remnant — (v/50)*50 —
                // which collapsed every temp position to 0, so the
                // header readout snapped to 2700K the moment the
                // slider was touched.)
                let v = if kind == GroupKind::Temp {
                    out.value.round()
                } else {
                    out.value
                };
                self.slider_live.insert(key.clone(), v);
                self.dragging_slider = true;
            }
            if out.released {
                let v = self.slider_live.remove(&key).unwrap_or(out.value);
                self.dragging_slider = false;
                // Convert to the PLANNER's units here, exactly
                // like the card path converts at commit (v1.3.2):
                // group_plan's Temp arm takes an API slider value
                // (1000..=10000), brightness takes a UI percent.
                // Broadcasting the raw stop index (0..=4) used to
                // clamp to 1000 — a 2700K blast to every member,
                // whatever stop the user had actually chosen.
                let num = match kind {
                    GroupKind::Temp => {
                        temp_kelvin_to_slider(temp_stop_kelvin(v.round() as i64))
                    }
                    _ => v.round() as i64,
                };
                self.group_broadcast(sec, kind, num, None);
            }
        });
    }

    fn section_view(&mut self, ui: &mut Ui, sec: &Section, avail: f32) {
        let pal = self.pal();
        let sec_top = ui.cursor().top();
        let sec_left = ui.cursor().left();
        let collapsed = self.cfg.layout.collapsed.get(&sec.key).copied().unwrap_or(false);
        // This section's place among the rendered ones — drives the
        // Edit-mode section move buttons (disabled at the ends).
        let (sec_idx, sec_total) = {
            let pos = self
                .sections_cache
                .iter()
                .position(|s| s.key == sec.key)
                .unwrap_or(0);
            (pos, self.sections_cache.len().max(1))
        };
        // The thermostat is a single card, not a device grid: its
        // own header + card, no collapse chevron, count badge,
        // Delete, or "Reset to name order".
        if sec.kind == SectionKind::Thermostat {
            self.thermostat_section_view(ui, sec_idx, sec_total);
            return;
        }
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
            Frame::new()
                .fill(pal.card)
                .stroke(Stroke::new(1.0, pal.border))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(8, 1))
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
                if self.editing {
                    // Section reordering (v1.2.1): every section —
                    // built-ins included — can move; the resulting
                    // order persists in layout.section_seq. Added
                    // down-then-up so the pair reads ↑ ↓ left-to-right
                    // in this right-to-left layout.
                    if ui
                        .add_enabled(sec_idx + 1 < sec_total, Button::new("↓").small())
                        .on_hover_text("Move section down")
                        .clicked()
                    {
                        self.move_section(&sec.order_key, 1);
                    }
                    if ui
                        .add_enabled(sec_idx > 0, Button::new("↑").small())
                        .on_hover_text("Move section up")
                        .clicked()
                    {
                        self.move_section(&sec.order_key, -1);
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
        // Column spans (v1.2.1): camera cards can be Large (2
        // columns) or Wide (the full row); everything else — and the
        // drag gap, which inherits the dragged card's span so the
        // row doesn't jump around mid-drag — packs by pack_rows.
        let drag_span = self.drag.as_ref().and_then(|d| {
            if self.camera_index(&d.id).is_some() {
                Some(cam_size_of(&self.cfg.layout, &d.id).span(cols))
            } else {
                None
            }
        });
        let spans: Vec<usize> = display
            .iter()
            .map(|item| match item {
                Some(id) if self.camera_index(id).is_some() => {
                    cam_size_of(&self.cfg.layout, id).span(cols)
                }
                Some(_) => 1,
                None => drag_span.unwrap_or(1),
            })
            .collect();
        let packed = pack_rows(&display, &spans, cols);
        let rows = packed.len();
        for (row_idx, row) in packed.iter().enumerate() {
            let sig = self.row_signature(sec, row_idx, row, card_w);
            let row_h = self.row_heights.get(&sig).copied().unwrap_or(0.0);
            let mut actual_h = 0.0f32;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (item, span) in row {
                    let item_owned = item.clone();
                    let cell_w = *span as f32 * card_w + (*span as f32 - 1.0) * gap;
                    let alloc = ui.allocate_ui_with_layout(
                        vec2(cell_w, row_h),
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
                                // v1.2.1: drag_started() only records a
                                // PENDING drag (see PendingDrag) — the
                                // card stays in the layout until the
                                // pointer has really moved, so clicks
                                // on the card's controls always land.
                                if self.editing && self.drag.is_none() && self.drag_pending.is_none() {
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
                                            self.drag_pending = Some(PendingDrag {
                                                id: id.clone(),
                                                is_scene: sec.kind == SectionKind::Scenes,
                                                from_section: sec.key.clone(),
                                                size: cell_rect.size(),
                                                grab_offset: p - cell_rect.min,
                                                start_idx,
                                                press_pos: p,
                                            });
                                        }
                                    }
                                }
                                if self.camera_index(&id).is_some() {
                                    self.camera_card(ui, &id, sec, cell_w, row_h);
                                } else {
                                    self.device_card(ui, &id, sec, cell_w, row_h);
                                }
                            }
                            None => {
                                // Insertion gap while dragging: the
                                // other cards reflow around it.
                                let r = ui.max_rect().shrink(1.0);
                                let p = ui.painter();
                                p.rect_filled(r, CornerRadius::same(R_CARD), self.accent_soft());
                                p.rect_stroke(r, CornerRadius::same(R_CARD), Stroke::new(1.5, self.accent()), egui::StrokeKind::Middle);
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
            // The live-preview video area adds a fixed 16:9 block, so
            // preview state is part of the shape (v1.2.0) — and so is
            // the card size (v1.2.1): a wider card's video is taller.
            let video = self.cfg.prefs.previews && self.ffmpeg_cached().is_some();
            format!(
                "cam:{}:{}:{}:{}",
                self.cfg.cameras[ci].template,
                video,
                self.paused_cams.contains(id),
                cam_size_of(&self.cfg.layout, id).as_str()
            )
        } else if let Some(di) = self.device_index(id) {
            let d = &self.devices[di];
            format!(
                "dev:{}:{}:{}:{}:{}:{}:{}",
                d.online,
                d.brightness_pct.is_some(),
                d.temp_slider.is_some(),
                d.has_color,
                d.has_brightness,
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
        row: &[(Option<String>, usize)],
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
        for (item, span) in row {
            s.push('|');
            s.push_str(
                &item
                    .as_ref()
                    .map(|id| self.card_shape(id))
                    .unwrap_or_else(|| "gap".to_string()),
            );
            s.push('#');
            s.push_str(&span.to_string());
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
                Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(
                        pal.card.r(),
                        pal.card.g(),
                        pal.card.b(),
                        238,
                    ))
                    .stroke(Stroke::new(1.5, self.accent()))
                    .corner_radius(CornerRadius::same(R_CARD))
                    .inner_margin(Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.set_width((drag.size.x - 28.0).max(80.0));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon).size(20.0));
                            ui.add(
                                egui::Label::new(RichText::new(name).size(15.0).strong())
                                    .truncate(),
                            );
                        });
                    });
            });
    }

    /// Move a whole section up/down (Edit mode, v1.2.1) and persist
    /// the resulting section sequence.
    fn move_section(&mut self, order_key: &str, dir: i32) {
        let rendered = self.sections_cache.clone();
        self.cfg.layout.section_seq =
            moved_section_seq(&self.cfg.layout.section_seq, &rendered, order_key, dir);
        self.save();
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
    /// The pin + toggle pair from a device card's header (added
    /// right-to-left: toggle first so the pin lands LEFT of it,
    /// matching the single-row header's left-to-right reading
    /// order). Shared by the single-row header and the two-row
    /// header's second row (v1.2.3).
    fn header_pin_toggle(&mut self, ui: &mut Ui, dev: &Device, pinned: bool) {
        let pal = self.pal();
        let accent = self.accent();
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

    /// The hide ✕ from a card header, with its click handling.
    /// Shared by the single-row header and the scene two-row
    /// header's second row (v1.2.3).
    fn header_hide_button(&mut self, ui: &mut Ui, dev: &Device) {
        let pal = self.pal();
        let hide = ui
            .add(Button::new(RichText::new("✕").color(pal.dim).size(13.0)).frame(false))
            .on_hover_text("Hide this device");
        if hide.clicked() {
            self.cfg.layout.hidden.push(dev.id.clone());
            self.save();
            self.toast(
                format!("{} hidden — bring it back from Settings", self.display(dev)),
                false,
            );
        }
    }

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
        Frame::new()
            .fill(pal.card)
            .stroke(Stroke::new(1.0, border))
            .corner_radius(CornerRadius::same(radius))
            .inner_margin(Margin { left: pad_x as i8, right: pad_x as i8, top: pad_top as i8, bottom: pad_bot as i8 })
            .show(ui, |ui| {
                ui.set_width((card_w - 2.0 * pad_x).max(80.0));
                if fill_h > 0.0 {
                    // Stretch the frame to the row's full cell height.
                    // egui 0.37 sizes a Frame as content + inner margin + 2*stroke +
                    // outer margin (0.27 did not count the stroke), so enforce
                    // row_h minus pads minus the 1.0 stroke on both sides,
                    // or the measured card comes back as row_h + 2.0 and the
                    // row-height cache ratchets +2.0 every frame, forever.
                    ui.set_min_height((fill_h - pad_top - pad_bot - 2.0).max(0.0));
                }
                // ---------- top row ----------
                let mut two_row = false;
                let mut scene_two_row = false;
                ui.horizontal(|ui| {
                    if self.editing {
                        ui.label(RichText::new("⠿").color(pal.dim).size(16.0));
                    }
                    // icon box
                    let icon_size = if compact { 36.0 } else { 42.0 };
                    Frame::new()
                        .fill(if dev.state && dev.online { self.accent_soft() } else { pal.track })
                        .corner_radius(CornerRadius::same(if compact { 11 } else { 13 }))
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
                    // reaching the pin / toggle / hide controls. The
                    // budget is the REAL remainder after the icon,
                    // minus the trailing controls' measured footprint
                    // (header_name_budget). v1.2.3: when that budget
                    // can't fit a real name (the usual case at the
                    // card widths the grid produces), the header
                    // goes two-row — device pin + toggle move to
                    // their own row below (rendered after this row)
                    // and the name reserves only the hide button;
                    // a scene's lone hide ✕ drops down the same way
                    // when even its roomier budget runs short.
                    let avail_after_icon = ui.available_width();
                    two_row = !dev.is_scene
                        && two_row_header(avail_after_icon, TRAILING_DEDUCT_DEVICE);
                    scene_two_row = dev.is_scene
                        && header_name_budget(avail_after_icon, TRAILING_DEDUCT_SCENE)
                            < SCENE_TWO_ROW_MIN_W;
                    let trailing_w = if dev.is_scene {
                        if scene_two_row {
                            0.0
                        } else {
                            TRAILING_DEDUCT_SCENE
                        }
                    } else if two_row {
                        TRAILING_DEDUCT_HIDE_ONLY
                    } else {
                        TRAILING_DEDUCT_DEVICE
                    };
                    let name_w = header_name_budget(avail_after_icon, trailing_w);
                    ui.allocate_ui_with_layout(
                        vec2(name_w, icon_size),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(self.display(&dev)).size(15.0).strong(),
                                    )
                                    .truncate(),
                                );
                                if !dev.online {
                                    Frame::new()
                                        .fill(Color32::from_rgb(0x64, 0x74, 0x8b))
                                        .corner_radius(CornerRadius::same(6))
                                        .inner_margin(Margin::symmetric(6, 2))
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
                                    .truncate(),
                                );
                            });
                        },
                    );
                    // trailing controls: hide lives here unless
                    // the header went two-row for a scene (then it
                    // drops to the scene's second row); pin + toggle
                    // join it in single-row device headers and move
                    // to their own row in two-row headers (added
                    // right-to-left either way).
                    ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                        if !scene_two_row {
                            self.header_hide_button(ui, &dev);
                        }
                        if !dev.is_scene && !two_row {
                            self.header_pin_toggle(ui, &dev, pinned);
                        }
                    });
                });
                if two_row {
                    // Two-row header, second row (v1.2.3): pin +
                    // toggle right-aligned under the name, so the
                    // name above keeps nearly the whole card width.
                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            self.header_pin_toggle(ui, &dev, pinned);
                        });
                    });
                }
                if scene_two_row {
                    // Scene two-row header (v1.2.3): just the
                    // hide ✕, right-aligned under the name — the
                    // Start button row follows as usual.
                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            self.header_hide_button(ui, &dev);
                        });
                    });
                }
                // ---------- controls ----------
                // Sliders are gated on CAPABILITY (parse-time flags
                // over merged data), never on the current value
                // (v1.2.2): a missing or placeholder reading renders
                // the slider at its last-known value (the merge
                // guarantees one once the device has ever reported)
                // or a neutral default — the control itself must
                // not vanish. Brightness gates on the NATIVE
                // brightness field (v1.3.0): a colour-only strip
                // gets colour controls only, no brightness slider;
                // temperature requires a real colour_temp field.
                if dev.online && !dev.is_scene {
                    if dev.has_brightness {
                        let pct = dev.brightness_pct.filter(|&p| p > 0).unwrap_or(100);
                        // Sliders live on the UI scale (1..100 ->
                        // device 11..100, v1.3.1): the reported
                        // device percent is mapped for display.
                        let ui_pct = brightness_device_to_ui(pct);
                        self.slider_row(ui, &dev, "b", "Brightness", 1.0, 100.0, ui_pct as f32, |v| {
                            format!("{}%", v.round() as i64)
                        });
                    }
                    if dev.has_temp {
                        let t = dev.temp_slider.unwrap_or(TEMP_NEUTRAL_SLIDER);
                        // Temp sliders show stop indices (v1.3.1):
                        // the reported value displays at its
                        // nearest stop without being rewritten.
                        let stop = temp_stop_index(temp_slider_to_kelvin(t));
                        self.slider_row(ui, &dev, "t", "Colour temperature", 0.0, (TEMP_STOPS_K.len() - 1) as f32, stop as f32, |v| {
                            format!("{}K", temp_stop_kelvin(v.round() as i64))
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
                            ComboBox::from_id_salt(("grp", &dev.id))
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
        // Live preview (v1.2.0): with previews on and ffmpeg found, a
        // 16:9 video area replaces the plain icon header. Everything
        // the closure needs is copied out first (it borrows self).
        let show_video = self.cfg.prefs.previews && self.ffmpeg_cached().is_some();
        let paused = self.paused_cams.contains(id);
        // With no live pump, a texture may still be HELD (v1.2.2)
        // while the big player has this camera — the frozen frame
        // stays on the tile.
        let (pump_state, pump_tex, pump_streak, pump_err) = if show_video {
            match self.pumps.get(id) {
                Some(p) => (
                    Some(p.state),
                    p.texture.as_ref().map(|t| t.id()),
                    p.streak,
                    p.last_error.clone(),
                ),
                None => (None, self.held_textures.get(id).map(|t| t.id()), 0, None),
            }
        } else {
            (None, None, 0, None)
        };
        let in_player = self.player_for.as_deref() == Some(id);
        Frame::new()
            .fill(pal.card)
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(CornerRadius::same(radius))
            .inner_margin(Margin { left: pad_x as i8, right: pad_x as i8, top: pad_top as i8, bottom: pad_bot as i8 })
            .show(ui, |ui| {
                ui.set_width((card_w - 2.0 * pad_x).max(80.0));
                if fill_h > 0.0 {
                    // egui 0.37 sizes a Frame as content + inner margin + 2*stroke +
                    // outer margin (0.27 did not count the stroke), so enforce
                    // row_h minus pads minus the 1.0 stroke on both sides,
                    // or the measured card comes back as row_h + 2.0 and the
                    // row-height cache ratchets +2.0 every frame, forever.
                    ui.set_min_height((fill_h - pad_top - pad_bot - 2.0).max(0.0));
                }
                // ---------- live video (replaces the icon header) ----------
                if show_video {
                    let vw = (card_w - 2.0 * pad_x).max(80.0);
                    let vh = vw * 9.0 / 16.0;
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(vw, vh), Sense::click());
                    // Cloned: the Retry buttons below need &mut ui.
                    let painter = ui.painter().clone();
                    painter.rect_filled(rect, CornerRadius::same(8), Color32::BLACK);
                    // Backoff presentation (v1.2.2): a clickable
                    // Retry (force-restart the pump now) instead of
                    // a bare "retrying" label, plus — after repeated
                    // failures — the mapped ffmpeg reason in small
                    // text, so the failure is diagnosable.
                    let backoff = pump_state == Some(PumpState::Backoff) && !paused;
                    let show_reason = backoff && pump_streak >= 2 && pump_err.is_some();
                    let mut retry_clicked = false;
                    if let Some(tid) = pump_tex {
                        painter.image(
                            tid,
                            rect,
                            egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        if backoff {
                            // Frozen last frame + Retry, bottom-left.
                            if show_reason {
                                painter.text(
                                    pos2(rect.left() + 8.0, rect.bottom() - 40.0),
                                    Align2::LEFT_CENTER,
                                    pump_err.clone().unwrap_or_default(),
                                    FontId::proportional(11.0),
                                    Color32::from_white_alpha(200),
                                );
                            }
                            let btn = egui::Rect::from_min_size(
                                pos2(rect.left() + 8.0, rect.bottom() - 32.0),
                                vec2(96.0, 24.0),
                            );
                            if ui
                                .put(btn, Button::new("↻ Retry now").small())
                                .on_hover_text("Restart the feed now")
                                .clicked()
                            {
                                retry_clicked = true;
                            }
                        } else if in_player && pump_state.is_none() {
                            painter.text(
                                pos2(rect.left() + 8.0, rect.bottom() - 10.0),
                                Align2::LEFT_CENTER,
                                "In the big player — tile paused",
                                FontId::proportional(11.5),
                                Color32::WHITE,
                            );
                        }
                    } else if backoff {
                        let btn = egui::Rect::from_center_size(
                            rect.center() + vec2(0.0, -8.0),
                            vec2(196.0, 30.0),
                        );
                        if ui
                            .put(btn, Button::new("Feed unavailable — ↻ Retry"))
                            .on_hover_text("Restart the feed now")
                            .clicked()
                        {
                            retry_clicked = true;
                        }
                        if show_reason {
                            painter.text(
                                rect.center() + vec2(0.0, 16.0),
                                Align2::CENTER_CENTER,
                                pump_err.clone().unwrap_or_default(),
                                FontId::proportional(11.5),
                                pal.dim,
                            );
                        }
                    } else {
                        let msg = if paused {
                            "Preview paused"
                        } else {
                            "Connecting…"
                        };
                        painter.text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            msg,
                            FontId::proportional(13.0),
                            pal.dim,
                        );
                    }
                    if retry_clicked {
                        self.kick_pump(id);
                    }
                    if pump_state == Some(PumpState::Live) && !paused {
                        // LIVE badge: red dot + label, top-left.
                        let c = pos2(rect.left() + 11.0, rect.top() + 11.0);
                        painter.circle_filled(c, 4.0, Color32::from_rgb(0xe5, 0x39, 0x35));
                        painter.text(
                            pos2(c.x + 8.0, c.y),
                            Align2::LEFT_CENTER,
                            "LIVE",
                            FontId::proportional(11.0),
                            Color32::WHITE,
                        );
                    }
                    if resp.clicked() {
                        // Clicking the tile is a backoff reset
                        // trigger too (v1.2.2).
                        self.kick_pump(id);
                        self.player_for = Some(id.to_string());
                    }
                    resp.on_hover_text("Open the big player");
                    // Pause / resume the tile preview, top-right —
                    // a PAINTED chip (v1.2.1), clear of the LIVE
                    // badge at the top-left.
                    let chip_rect = egui::Rect::from_min_size(
                        pos2(rect.right() - 33.0, rect.top() + 5.0),
                        vec2(28.0, 28.0),
                    );
                    let pbtn = play_pause_chip(ui, id, chip_rect, paused).on_hover_text(
                        if paused {
                            "Resume preview"
                        } else {
                            "Pause preview"
                        },
                    );
                    if pbtn.clicked() {
                        if paused {
                            self.paused_cams.remove(id);
                        } else {
                            self.paused_cams.insert(id.to_string());
                        }
                    }
                    ui.add_space(10.0);
                }
                // ---------- top row ----------
                let mut cam_two_row = false;
                ui.horizontal(|ui| {
                    if self.editing {
                        ui.label(RichText::new("⠿").color(pal.dim).size(16.0));
                    }
                    let icon_size = if compact { 36.0 } else { 42.0 };
                    if !show_video {
                        Frame::new()
                            .fill(pal.track)
                            .corner_radius(CornerRadius::same(if compact { 11 } else { 13 }))
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
                    }
                    // v1.2.3: same two-row fallback as device
                    // cards — when the single-row budget can't fit
                    // a real camera name, the pin drops to its own
                    // row and the name reserves only the hide ✕.
                    let cam_avail = ui.available_width();
                    cam_two_row = two_row_header(cam_avail, TRAILING_DEDUCT_CAMERA);
                    let name_w = header_name_budget(
                        cam_avail,
                        if cam_two_row {
                            TRAILING_DEDUCT_HIDE_ONLY
                        } else {
                            TRAILING_DEDUCT_CAMERA
                        },
                    );
                    ui.allocate_ui_with_layout(
                        vec2(name_w, icon_size),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(cam.name.clone()).size(15.0).strong())
                                    .truncate(),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(camera_status_text(&cam)).size(12.5).color(pal.dim),
                                )
                                .truncate(),
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
                        if !cam_two_row {
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
                        }
                    });
                });
                if cam_two_row {
                    // Two-row camera header (v1.2.3): pin on its
                    // own right-aligned row under the name.
                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
                }
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
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(Button::new("Open in external player").small())
                        .on_hover_text("Hand the stream to the player app named in Settings")
                        .clicked()
                    {
                        self.open_external_player(id);
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
                        ComboBox::from_id_salt(("grpcam", id))
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
                    // Card size (v1.2.1): Normal / Large / Wide —
                    // segmented like the quality switch above.
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Size").size(12.5).color(pal.dim));
                        let cur_size = cam_size_of(&self.cfg.layout, id);
                        for (sz, label) in [
                            (CamSize::Normal, "Normal"),
                            (CamSize::Large, "Large"),
                            (CamSize::Wide, "Wide"),
                        ] {
                            let active = cur_size == sz;
                            let btn = if active {
                                Button::new(RichText::new(label).color(Color32::WHITE))
                                    .fill(accent)
                                    .small()
                            } else {
                                Button::new(label).small()
                            };
                            if ui.add(btn).clicked() && !active {
                                if sz == CamSize::Normal {
                                    self.cfg.layout.cam_sizes.remove(id);
                                } else {
                                    self.cfg.layout
                                        .cam_sizes
                                        .insert(id.to_string(), sz.as_str().to_string());
                                }
                                self.save();
                            }
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
                // Temp sliders move in stop indices (v1.3.1):
                // each drag position is a whole detent.
                out.value.round()
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
                // Send the exact stop, converted to the API
                // scale (v1.3.1).
                self.do_temp(&dev.id, temp_kelvin_to_slider(temp_stop_kelvin(v.round() as i64)));
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
        Frame::new()
            .fill(pal.card)
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(CornerRadius::same(R_CARD))
            .inner_margin(Margin::symmetric(22, 20))
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
                            ComboBox::from_id_salt("set_theme")
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
                            ComboBox::from_id_salt("set_density")
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
                                        RichText::new(if cfg!(target_os = "macos") {
                                            "The download's file name is verified before it opens. The disk image opens so you can drag the new version into Applications — quit and relaunch the app afterwards."
                                        } else {
                                            "The download's file name is verified before it runs. This app closes itself once the installer starts, so the installer can replace it."
                                        })
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
                        // ---- Startup & background (native editions) ----
                        let mut start_act: Option<(&'static str, String)> = None;
                        Self::settings_panel(ui, &pal, "Startup & background", |ui| {
                            let mut on = self.cfg.prefs.start_with_windows;
                            if ui.checkbox(&mut on, AUTOSTART_LABEL).changed() {
                                start_act = Some(("toggle", on.to_string()));
                            }
                            if on {
                                ui.add_space(6.0);
                                Self::field_label(ui, &pal, "Launch as");
                                let cur = LaunchMode::parse(&self.cfg.prefs.launch_mode);
                                ui.horizontal(|ui| {
                                    for (m, label) in [
                                        (LaunchMode::Normal, "Normal window"),
                                        (LaunchMode::Minimized, "Minimized (to tray)"),
                                        (LaunchMode::Maximized, "Maximized"),
                                    ] {
                                        let active = cur == m;
                                        let btn = if active {
                                            Button::new(
                                                RichText::new(label).color(Color32::WHITE),
                                            )
                                            .fill(accent)
                                        } else {
                                            Button::new(label)
                                        };
                                        if ui.add(btn).clicked() && !active {
                                            start_act =
                                                Some(("mode", m.as_str().to_string()));
                                        }
                                    }
                                });
                            }
                            ui.add_space(10.0);
                            let mut ctt = self.cfg.prefs.close_to_tray;
                            if ui.checkbox(&mut ctt, CLOSE_TO_TRAY_LABEL).changed() {
                                start_act = Some(("tray", ctt.to_string()));
                            }
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new(AUTOSTART_HELP)
                                    .size(12.5)
                                    .color(pal.dim),
                            );
                        });
                        match start_act {
                            Some(("toggle", v)) => {
                                self.cfg.prefs.start_with_windows = v == "true";
                                if apply_autostart(&self.cfg.prefs) {
                                    self.toast(
                                        if self.cfg.prefs.start_with_windows {
                                            format!("{}: on", AUTOSTART_LABEL)
                                        } else {
                                            format!("{}: off", AUTOSTART_LABEL)
                                        },
                                        false,
                                    );
                                } else {
                                    // Show the store's truth, not the wish.
                                    self.cfg.prefs.start_with_windows =
                                        reg_run_value().is_some();
                                    self.toast(
                                        "Couldn't update the startup entry.",
                                        true,
                                    );
                                }
                                self.save();
                            }
                            Some(("mode", m)) => {
                                self.cfg.prefs.launch_mode = m;
                                if self.cfg.prefs.start_with_windows {
                                    apply_autostart(&self.cfg.prefs);
                                }
                                self.save();
                            }
                            Some(("tray", v)) => {
                                self.cfg.prefs.close_to_tray = v == "true";
                                if self.cfg.prefs.close_to_tray {
                                    self.ensure_tray();
                                }
                                self.save();
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
                            ui.label(
                                RichText::new(format!(
                                    "{} — adjusts automatically",
                                    auto_interval_text(self.disc_interval_secs)
                                ))
                                .size(12.5)
                                .color(pal.dim),
                            );
                        });
                        // ---- Stream gateway & cameras ----
                        Self::settings_panel(ui, &pal, "Stream gateway & cameras", |ui| {
                            ui.label(
                                RichText::new("Cameras you add (✎ Edit → + Add camera) stream over RTSP. This app plays them in place: a bundled copy of ffmpeg decodes each stream for the live tile previews and the ▶ Watch player — it just works, nothing to set up. Alternatively, point ▶ Watch at a small stream gateway on your network — go2rtc or MediaMTX — and Watch opens the gateway's watch page in your browser instead (the gateway needs a stream with the same name as the camera here, fed from the same RTSP address — the card's Copy URL button gives you the exact address). If ffmpeg can't be found, previews switch off and Watch falls back to the external player app below.")
                                    .size(12.5)
                                    .color(pal.dim),
                            );
                            ui.add_space(8.0);
                            let mut previews = self.cfg.prefs.previews;
                            if ui
                                .checkbox(&mut previews, "Live camera previews")
                                .changed()
                            {
                                self.cfg.prefs.previews = previews;
                                self.save();
                            }
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("Live video inside each camera tile (Stream 2 on Tapo cameras, to keep it light). The ▶ Watch player always uses the quality picked on the card.")
                                    .size(12.5)
                                    .color(pal.dim),
                            );
                            ui.add_space(10.0);
                            Self::field_label(ui, &pal, "ffmpeg path (optional override)");
                            let mut ff = self.cfg.prefs.ffmpeg_path.clone();
                            if ui
                                .add(
                                    TextEdit::singleline(&mut ff)
                                        .desired_width(f32::INFINITY)
                                        .hint_text("Leave empty — the bundled ffmpeg is used"),
                                )
                                .changed()
                            {
                                self.cfg.prefs.ffmpeg_path = ff;
                                self.save();
                            }
                            ui.add_space(4.0);
                            let ff_status = match self.ffmpeg_path() {
                                Some(p) => format!("ffmpeg found: {}", p.display()),
                                None => "ffmpeg not found — previews are off and ▶ Watch uses the external player.".to_string(),
                            };
                            ui.label(RichText::new(ff_status).size(12.5).color(pal.dim));
                            ui.add_space(10.0);
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
                            ComboBox::from_id_salt("set_gw_type")
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
                            Self::field_label(ui, &pal, "External player command (fallback)");
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
                                RichText::new("The fallback when ffmpeg isn't available (and what “Open in external player” uses): this program is started with the stream URL as its only argument. A bare name like vlc is found even when it isn't on PATH — the Windows App Paths registry entries and the usual install folders are checked too.")
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
                        // ---- Thermostat (HomeKit) ----
                        let tstat_panel_top = ui.cursor().top();
                        Self::settings_panel(ui, &pal, "Thermostat (HomeKit)", |ui| {
                            ui.label(
                                RichText::new("Control a HomeKit thermostat (e.g. the Resideo T6 Pro) directly over your home network — no cloud, no account. This works only while this computer is on the same home network as the thermostat; away from home, use the Resideo app or Alexa.")
                                    .size(12.5)
                                    .color(pal.dim),
                            );
                            ui.add_space(8.0);
                            let paired = self.cfg.thermostat.clone();
                            match &paired {
                                Some(p) => {
                                    ui.label(
                                        RichText::new(format!(
                                            "Paired with {} ({}) at {}:{}",
                                            if p.accessory_name.is_empty() {
                                                "thermostat"
                                            } else {
                                                &p.accessory_name
                                            },
                                            if p.accessory_model.is_empty() {
                                                "HomeKit accessory"
                                            } else {
                                                &p.accessory_model
                                            },
                                            p.accessory_ip,
                                            p.accessory_port
                                        ))
                                        .size(12.5),
                                    );
                                    ui.add_space(6.0);
                                    if ui
                                        .add(Button::new(
                                            RichText::new("Forget pairing").color(DANGER),
                                        ))
                                        .clicked()
                                    {
                                        self.cfg.thermostat = None;
                                        self.tstat_state = None;
                                        self.save();
                                        self.toast("Thermostat pairing forgotten", false);
                                    }
                                    ui.label(
                                        RichText::new("This only forgets the keys on this computer. To clear the thermostat side, use HomeKit Reset on the unit (Menu → Reset → HomeKit Reset). It never touches WiFi, schedule, or the Resideo link.")
                                            .size(12.5)
                                            .color(pal.dim),
                                    );
                                }
                                None => {
                                    ui.label(RichText::new("Not paired.").size(12.5));
                                    ui.add_space(6.0);
                                    // Two-step pairing for rotating-code
                                    // thermostats: Start pairing opens
                                    // the session (M1/M2) with no code
                                    // needed; the thermostat shows its
                                    // fresh code during the attempt,
                                    // and Finish sends it (M3..M6).
                                    // A code typed before Start would
                                    // already be stale, so the field
                                    // only appears once a session is
                                    // open. Only one session at a
                                    // time: other devices' Start
                                    // buttons disable while one is
                                    // pending.
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add_enabled(
                                                !self.tstat_busy
                                                    && !self.tstat_set_busy
                                                    && self.tstat_pending.is_none(),
                                                Button::new("Discover devices"),
                                            )
                                            .clicked()
                                        {
                                            self.spawn_tstat_discover();
                                        }
                                        if self.tstat_busy {
                                            ui.label(
                                                RichText::new("Working…")
                                                    .size(12.5)
                                                    .color(pal.dim),
                                            );
                                        }
                                    });
                                    let pending_id = self
                                        .tstat_pending
                                        .as_ref()
                                        .map(|p| p.device_id().to_string());
                                    for dev in self.tstat_discovered.clone() {
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new(format!(
                                                "{} — {} {}",
                                                dev.name,
                                                dev.model,
                                                if dev.unpaired {
                                                    "(unpaired)"
                                                } else {
                                                    "(already paired)"
                                                }
                                            )).size(13.0));
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    match &pending_id {
                                                        None => {
                                                            if ui
                                                                .add_enabled(
                                                                    !self.tstat_busy
                                                                        && !self.tstat_set_busy,
                                                                    Button::new("Start pairing"),
                                                                )
                                                                .clicked()
                                                            {
                                                                self.spawn_tstat_begin(dev.clone());
                                                            }
                                                        }
                                                        Some(pid) if *pid == dev.id => {
                                                            ui.label(
                                                                RichText::new("Pairing…")
                                                                    .size(12.5)
                                                                    .color(pal.dim),
                                                            );
                                                        }
                                                        Some(_) => {
                                                            ui.add_enabled(
                                                                false,
                                                                Button::new("Start pairing"),
                                                            );
                                                        }
                                                    }
                                                },
                                            );
                                        });
                                        if pending_id.as_deref() == Some(dev.id.as_str()) {
                                            ui.label(
                                                RichText::new("Code shown on your thermostat — type it here:")
                                                    .size(12.5),
                                            );
                                            ui.add(
                                                TextEdit::singleline(&mut self.tstat_setup_code)
                                                    .hint_text("e.g. 123-45-678"),
                                            );
                                            ui.label(
                                                RichText::new("Type it promptly: the thermostat may time out the pairing session. If it expires, press Start pairing and try again with the fresh code.")
                                                    .size(12.5)
                                                    .color(pal.dim),
                                            );
                                            ui.label(
                                                RichText::new("The code is used for this pairing only and is never stored — only the derived keys are saved, like the saved Tuya login above.")
                                                    .size(12.5)
                                                    .color(pal.dim),
                                            );
                                            ui.horizontal(|ui| {
                                                if ui
                                                    .add_enabled(
                                                        !self.tstat_busy
                                                            && !self.tstat_set_busy
                                                            && !self
                                                                .tstat_setup_code
                                                                .trim()
                                                                .is_empty(),
                                                        Button::new("Finish pairing"),
                                                    )
                                                    .clicked()
                                                {
                                                    self.spawn_tstat_finish();
                                                }
                                                if ui.add(Button::new("Cancel")).clicked() {
                                                    // Dropping the pending
                                                    // session closes its
                                                    // socket at once.
                                                    self.tstat_pending = None;
                                                    self.tstat_error = None;
                                                }
                                            });
                                        }
                                        ui.separator();
                                    }
                                }
                            }
                            if let Some(e) = &self.tstat_error {
                                ui.add_space(6.0);
                                ui.label(RichText::new(e.clone()).size(12.5).color(DANGER));
                            }
                            // ---- Diagnostics log (v1.6.4) ----
                            // Timestamped ring buffer of thermostat
                            // operations (discovery, pairing, fetch,
                            // set) for debugging. It never carries
                            // setup codes, keys, or credentials —
                            // see hap::tstat_log.
                            ui.add_space(10.0);
                            ui.label(RichText::new("Diagnostics log").size(14.0).strong());
                            ui.label(
                                RichText::new("Recent thermostat and sync activity. Copy it into a message if something misbehaves.")
                                    .size(12.5)
                                    .color(pal.dim),
                            );
                            let mut log_text = crate::hap::tstat_log_text();
                            ui.horizontal(|ui| {
                                if ui.add(Button::new("Copy log").small()).clicked() {
                                    ui.ctx().copy_text(log_text.clone());
                                }
                                if ui.add(Button::new("Clear").small()).clicked() {
                                    crate::hap::tstat_log_clear();
                                }
                            });
                            egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut log_text)
                                        .desired_rows(8)
                                        .desired_width(f32::INFINITY)
                                        .interactive(false),
                                );
                            });
                        });
                        if self.settings_jump_thermostat {
                            // One-shot jump (v2.0.2): the edit bar's
                            // "+ Add thermostat" button asked for
                            // this panel. Scroll it to the top of
                            // the Settings view, then clear the
                            // flag (scroll_to_rect applies via the
                            // scroll target on the next frame).
                            self.settings_jump_thermostat = false;
                            let rect = egui::Rect::from_min_max(
                                egui::pos2(0.0, tstat_panel_top),
                                egui::pos2(ui.available_width(), ui.cursor().top()),
                            );
                            ui.scroll_to_rect(rect, Some(Align::TOP));
                        }
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
                                "Your credentials. Your username and password go directly to Tuya's servers (px1.tuya*.com) over HTTPS. If you tick “Keep me logged in”, they are kept in this app's config file (in your user profile, under Homestead\\config.json) so the app can silently sign back in when the session token expires — use it only on your own devices, and use “Forget saved login” above to erase them.",
                                "The proxy. Some networks block direct calls to Tuya's endpoints. In “Auto” mode, China-region traffic is routed through a community CORS proxy (cors.smartathome.co.uk); US and EU traffic goes direct. Note a proxy can see the requests it forwards (they contain a session token, never your password after sign-in — the sign-in request itself does contain your password, so only use proxies you trust for CN logins).",
                                "Customising. Sections appear as Cameras, Pinned, Lighting, your groups, Ungrouped, Scenes by default — and in ✎ Edit mode every section header has ↑/↓ buttons to move the section itself; your order sticks. Inside a section, cards sort A–Z by name by default: drag cards into your own order — drag a card by its body and drop it where it belongs, or use the ↑/↓ buttons (that order then sticks, and each section has a “Reset to name order” button) — create groups (rooms), rename devices with an alias, or hide devices you never use. Dragging a card onto another section moves it there. Use the 📌 button on any card to pin a shortcut copy of it in the Pinned section — the device also stays in its group below. Scenes always live in their own Scenes section.",
                                "Lighting & group controls. Lights organise themselves: any bulb or strip that isn't assigned to one of your groups lives in the built-in Lighting section, and cameras get their own built-in Cameras section the same way — assigning a device to a group always wins over the automatic sections. The Lighting header (and the header of any group containing a light) has group controls: a master switch (on if any member is on; clicking turns the whole group on or off), brightness and colour-temperature sliders showing the members' average, and colour swatches. Group controls broadcast — they send the command to each member in turn, and changing a single light afterwards simply overrides it for that light. Nothing about the group itself is stored.",
                                "Cameras (RTSP). Tapo cameras: in the Tapo app, open the camera → Settings → Advanced Settings → Camera Account and create a camera account, and make sure RTSP is enabled for the camera. The username and password you add here are that camera account — not your TP-Link cloud account. Then use ✎ Edit → + Add camera and enter the camera's IP address (a fixed or DHCP-reserved IP works best, so the address doesn't change). The app builds the stream address for you: rtsp://user:pass@IP:554/stream1 (high quality) or /stream2 (standard) — or pick “Custom path” for other RTSP cameras. Camera cards live alongside your devices and can be pinned, grouped and reordered the same way; unassigned cameras live in the built-in Cameras section (the first section by default) until you assign them to a group of your own. In Edit mode a camera card can also be resized — Normal (one grid cell), Large (two columns) or Wide (the full row).",
                                "Watching a stream. This app plays RTSP streams in place: a bundled copy of ffmpeg decodes the feed for the live tile previews (Tapo Stream 2, the standard-quality stream, to keep tiles light — Large tiles decode it bigger, and a Wide tile uses the quality picked on the card) and for the big ▶ Watch player, which uses the quality picked on the card. Tile previews can be turned off in Settings → Stream gateway & cameras, where you can also point the app at a different ffmpeg. If ffmpeg can't be found, previews switch off and ▶ Watch falls back to the external player app named in Settings (VLC by default) — or use Copy URL and paste the address into VLC's “Open Network Stream”. If a stream gateway (go2rtc / MediaMTX) is configured instead, ▶ Watch opens the gateway's watch page in your browser. (The web edition of this app can't decode RTSP itself: with a gateway configured, its tiles embed the gateway's feed; without one, its Watch hands the stream to a player app.)",
                                "Limits. Tuya rate-limits how often the device list can be fetched and how fast commands can be sent. When that happens the app quietly keeps showing the last known states, retries commands a moment later, and stretches its refresh interval until Tuya is happy again — no action needed. Autorefresh every 30s or slower is comfortable.",
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

    /// The big in-app player (v1.2.0): a 960×540 ffmpeg pump for one
    /// camera at its current quality selection, with a quality
    /// switch, external-player hand-off and Close. Closing stops the
    /// pump (sync_pumps no longer wants it).
    fn player_window(&mut self, ctx: &egui::Context) {
        let Some(item) = self.player_for.clone() else { return };
        let Some(ci) = self.camera_index(&item) else {
            self.player_for = None;
            return;
        };
        let cam = self.cfg.cameras[ci].clone();
        let pal = self.pal();
        let accent = self.accent();
        let (pump_state, pump_tex, pump_streak, pump_err) = match self.pumps.get("player") {
            Some(p) => (
                Some(p.state),
                p.texture.as_ref().map(|t| t.id()),
                p.streak,
                p.last_error.clone(),
            ),
            None => (None, None, 0, None),
        };
        let mut close = false;
        let mut open_ext = false;
        let mut copy_url = false;
        let mut retry = false;
        let mut set_tpl: Option<String> = None;
        egui::Window::new(format!("▶ {}", cam.name))
            .collapsible(false)
            .resizable(true)
            .default_size(vec2(880.0, 580.0))
            .show(ctx, |ui| {
                let vw = ui.available_width().max(240.0);
                let vh = (vw * 9.0 / 16.0).min(540.0);
                let (rect, _) = ui.allocate_exact_size(vec2(vw, vh), Sense::hover());
                let painter = ui.painter();
                painter.rect_filled(rect, CornerRadius::same(8), Color32::BLACK);
                if let Some(tid) = pump_tex {
                    painter.image(
                        tid,
                        rect,
                        egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    if pump_state == Some(PumpState::Backoff) {
                        painter.text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            "Feed unavailable — retrying",
                            FontId::proportional(14.0),
                            Color32::WHITE,
                        );
                        if pump_streak >= 2 {
                            if let Some(err) = &pump_err {
                                painter.text(
                                    rect.center() + vec2(0.0, 22.0),
                                    Align2::CENTER_CENTER,
                                    err.clone(),
                                    FontId::proportional(12.0),
                                    Color32::from_white_alpha(200),
                                );
                            }
                        }
                    }
                } else {
                    painter.text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        match pump_state {
                            Some(PumpState::Backoff) => "Feed unavailable — retrying",
                            _ => "Connecting…",
                        },
                        FontId::proportional(14.0),
                        pal.dim,
                    );
                    if pump_state == Some(PumpState::Backoff) && pump_streak >= 2 {
                        if let Some(err) = &pump_err {
                            painter.text(
                                rect.center() + vec2(0.0, 22.0),
                                Align2::CENTER_CENTER,
                                err.clone(),
                                FontId::proportional(12.0),
                                pal.dim,
                            );
                        }
                    }
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(camera_status_text(&cam)).size(12.5).color(pal.dim),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(Button::new("Close")).clicked() {
                            close = true;
                        }
                        if pump_state == Some(PumpState::Backoff)
                            && ui
                                .add(Button::new("↻ Retry"))
                                .on_hover_text("Restart the feed now")
                                .clicked()
                        {
                            retry = true;
                        }
                        if ui.add(Button::new("Copy URL")).clicked() {
                            copy_url = true;
                        }
                        if ui
                            .add(Button::new("Open in external player"))
                            .on_hover_text(
                                "Hand the stream to the player app named in Settings",
                            )
                            .clicked()
                        {
                            open_ext = true;
                        }
                    });
                });
                // Quality switch (Tapo templates) — the same selection
                // the card's segment buttons edit; the pump restarts
                // on the new stream.
                if cam.template == "tapo1" || cam.template == "tapo2" {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Quality").size(12.5).color(pal.dim));
                        for (tpl, label) in
                            [("tapo1", "Stream 1 · HD"), ("tapo2", "Stream 2 · SD")]
                        {
                            let active = cam.template == tpl;
                            let btn = if active {
                                Button::new(RichText::new(label).color(Color32::WHITE))
                                    .fill(accent)
                            } else {
                                Button::new(label)
                            };
                            if ui.add(btn).clicked() && !active {
                                set_tpl = Some(tpl.to_string());
                            }
                        }
                    });
                }
            });
        if let Some(tpl) = set_tpl {
            if let Some(ci2) = self.camera_index(&item) {
                self.cfg.cameras[ci2].template = tpl;
                self.save();
            }
        }
        if retry {
            self.kick_pump("player");
        }
        if copy_url {
            ctx.copy_text(build_rtsp_url(&cam));
            self.toast("Stream URL copied", false);
        }
        if open_ext {
            self.open_external_player(&item);
        }
        if close {
            self.player_for = None;
        }
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
                ComboBox::from_id_salt("cam_tpl")
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
                        .unwrap_or_else(|| {
                            // Fresh id, verified unused (v1.2.1 —
                            // a bare millisecond timestamp could tie
                            // with an existing camera and share it).
                            let mut id = new_camera_id();
                            while self.cfg.cameras.iter().any(|c| c.id == id) {
                                id = new_camera_id();
                            }
                            id
                        }),
                    name,
                    user: form.user.trim().to_string(),
                    password: obfuscate(&form.pass),
                    host,
                    port,
                    template: form.template.clone(),
                    path: form.path.trim().to_string(),
                };
                if let Some(eid) = &form.id {
                    match self.camera_index(eid) {
                        Some(ci) => {
                            self.cfg.cameras[ci] = cam;
                            // Any edit save invalidates the pump
                            // (v1.2.2): bump the camera's generation
                            // so sync_pumps restarts it immediately
                            // with the backoff reset — even if the
                            // stream URL happens to be unchanged.
                            *self.cam_gen.entry(eid.clone()).or_insert(0) += 1;
                            self.kick_pump(eid);
                        }
                        None => {
                            // The camera being edited is gone —
                            // keep the form open with the reason
                            // instead of silently discarding the
                            // edit behind a "Camera saved" toast.
                            form.error = Some(
                                "That camera no longer exists — it may have been deleted.".to_string(),
                            );
                            self.cam_form = Some(form);
                            return;
                        }
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
                        Frame::new()
                            .fill(fill)
                            .stroke(Stroke::new(1.0, stroke))
                            .corner_radius(CornerRadius::same(13))
                            .inner_margin(Margin::symmetric(17, 10))
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

    /// Automatic-refresh driver (v2.0.5): the timer is the only
    /// Tuya refresh path and it is always on — no toggle, no
    /// manual button. Paused while editing, dragging, or typing.
    /// Extracted from `ui` so the timer path is unit-testable
    /// without an eframe::Frame. The cadence is the dynamic
    /// interval (v2.0.6): it doubles on discovery throttling and
    /// halves back toward the 30 s floor on success, so the timer
    /// itself dodges the discovery-side throttle. The streak
    /// backoff is set to the same value by note_rate_limited, so
    /// the two waits agree and the effective wait is their max.
    fn maybe_autorefresh(&mut self, ctx: &egui::Context) {
        if self.view == View::Dashboard
            && self.session.is_some()
            && !self.editing
            && !self.dragging_slider
            && !ctx.input(|i| i.pointer.primary_down())
        {
            let due = self
                .last_fetch
                .map(|t| t.elapsed() >= Duration::from_secs(self.disc_interval_secs))
                .unwrap_or(false);
            // Rate-limit backoff (v1.2.0): skip cycles silently until
            // it lapses — the cached devices stay on screen meanwhile.
            let backed_off = self
                .disc_backoff_until
                .map(|t| Instant::now() < t)
                .unwrap_or(false);
            if due && !self.disc_busy && !backed_off {
                self.spawn_discovery(false);
            }
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.apply_style(ui.ctx());
        self.poll_net();
        // Tray + close behaviour (v1.2.1): poll tray events, turn a
        // close into a hide when close-to-tray is on.
        self.handle_window_chrome(ui.ctx(), frame);
        // Live previews (v1.2.0): reconcile pumps and upload the
        // latest frames as textures before anything renders.
        self.sync_pumps(ui.ctx());
        // Drag & drop bookkeeping runs BEFORE rendering, hit-testing
        // against last frame's card geometry. It also runs while a
        // press is merely PENDING (v1.2.1), which is when promotion
        // (or quiet cancellation) happens.
        if self.drag.is_some() || self.drag_pending.is_some() {
            self.update_drag(ui.ctx());
        }
        // A verified update download (v1.4.0: the install step is
        // platform-shaped — see UpdateInstall).
        if let Some(path) = self.update_launch.take() {
            match update_install_for(std::env::consts::OS) {
                UpdateInstall::RunInstallerAndExit => {
                    // Run the installer, then exit so the per-user
                    // installer can replace this app while it isn't
                    // running. An installer exit is a real exit —
                    // close-to-tray must not turn it into a hide.
                    self.quitting = true;
                    match Command::new(&path).spawn() {
                        Ok(_) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => {
                            self.update_error = Some(format!(
                                "The installer was downloaded but could not be started: {}",
                                e
                            ));
                        }
                    }
                }
                UpdateInstall::OpenDiskImage => {
                    // macOS: open the .dmg and step aside — the
                    // user drags the new app into Applications and
                    // relaunches when ready. The app never exits
                    // or replaces itself.
                    match Command::new("open").arg(&path).spawn() {
                        Ok(_) => self.toast(
                            "Update downloaded — drag the app into Applications to replace this copy, then relaunch.",
                            false,
                        ),
                        Err(e) => {
                            self.update_error = Some(format!(
                                "The update was downloaded but the disk image could not be opened: {}",
                                e
                            ));
                        }
                    }
                }
            }
        }
        let pal = self.pal();
        match self.view {
            View::Login => {
                egui::CentralPanel::default()
                    .frame(Frame::new().fill(pal.bg))
                    .show(ui, |ui| self.view_login(ui));
            }
            View::Dashboard => {
                self.header(ui);
                egui::CentralPanel::default()
                    .frame(Frame::new().fill(pal.bg))
                    .show(ui, |ui| self.view_dashboard(ui));
            }
            View::Settings => {
                egui::Panel::top("settings-header")
                    .frame(Frame::new().fill(pal.card).inner_margin(Margin::symmetric(18, 12)))
                    .show(ui, |ui| {
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
                    .frame(Frame::new().fill(pal.bg))
                    .show(ui, |ui| self.view_settings(ui));
            }
        }
        // Automatic refresh — always on (v2.0.5: no toggle, no
        // manual button; the timer is the only Tuya refresh path).
        // Paused while editing, dragging, or typing.
        self.maybe_autorefresh(ui.ctx());
        // Thermostat autorefresh — independent of the Tuya session:
        // piggybacks the same interval while on the dashboard.
        if self.view == View::Dashboard && self.cfg.thermostat.is_some() && !self.tstat_busy && !self.tstat_fetch_busy && !self.tstat_set_busy {
            let due = self
                .tstat_last_fetch
                .map(|t| t.elapsed() >= Duration::from_secs(self.cfg.prefs.interval))
                .unwrap_or(true);
            if due {
                self.spawn_tstat_fetch();
            }
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        if self.new_group.is_some() {
            self.new_group_window(ui.ctx());
        }
        if self.cam_form.is_some() {
            self.camera_window(ui.ctx());
        }
        if self.player_for.is_some() {
            self.player_window(ui.ctx());
        }
        self.drag_ghost(ui.ctx());
        self.show_toasts(ui.ctx());
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // App exit: stop every pump (kills its ffmpeg) rather than
        // leaving decoders running behind us.
        for p in self.pumps.values() {
            p.stop();
        }
    }
}

fn main() -> eframe::Result<()> {
    // Single instance (v1.2.2, Windows): when another copy is
    // already running — typically hidden in the tray — signal it to
    // show its window and exit immediately, before creating a
    // window, a tray icon or any camera pump.
    #[cfg(target_os = "windows")]
    let _instance_guard = match acquire_single_instance() {
        (InstanceRole::Secondary, _) => return Ok(()),
        (InstanceRole::Primary, guard) => guard,
    };
    // Single instance (v1.4.0, macOS): the flock guard — a
    // secondary signals via the show-request file and exits
    // just as early (see acquire_single_instance above).
    #[cfg(target_os = "macos")]
    let _instance_guard = match acquire_single_instance() {
        (InstanceRole::Secondary, _) => return Ok(()),
        (InstanceRole::Primary, guard) => guard,
    };
    // Launch mode (v1.2.1 flags; v1.2.4 resolution): the autostart
    // Run command carries --minimized / --maximized per the
    // Settings launch mode, and the same flags work for manual
    // (shortcut) launches. But an argv flag is only ONE input:
    // the EFFECTIVE mode falls back to the saved Settings mode
    // (resolve_launch_mode), so launches with no command line at
    // all — Windows' restartable-apps relaunch after sign-in, a
    // Start Menu click — honour "Minimized (to tray)" too. The
    // saved prefs are read through the same autostart reconcile
    // App::new runs (an existing entry re-derives them; the
    // reconcile is idempotent, App::new's pass becomes a no-op).
    let argv_mode = launch_mode_from_args(&std::env::args().collect::<Vec<String>>());
    let mut startup_cfg = load_config();
    reconcile_autostart(&mut startup_cfg);
    let launch = resolve_launch_mode(
        argv_mode,
        LaunchMode::parse(&startup_cfg.prefs.launch_mode),
    );
    // Startup placement is the OS default (v1.2.4 behaviour,
    // restored in v1.3.0): the v1.2.5 placement memory was
    // field-rejected — a restored window could land slightly
    // off-screen with its title bar out of reach — so no saved
    // geometry is read or written anywhere — the one exception
    // (v1.5.0) is a monitor IDENTITY, never a rectangle: on a
    // Maximized launch the window is moved to the remembered
    // display by the snap machine and ends maximized, so the
    // off-screen failure cannot recur. (The snap-to-monitor
    // header button is the supported way to place the window:
    // it acts at click time only and lets the OS own the result.)
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Homestead")
        .with_inner_size([1120.0, 780.0])
        .with_min_inner_size([720.0, 520.0]);
    match launch {
        // Minimized = hidden in the tray from the very first frame.
        LaunchMode::Minimized => viewport = viewport.with_visible(false),
        LaunchMode::Maximized => viewport = viewport.with_maximized(true),
        LaunchMode::Normal => {}
    }
    let options = eframe::NativeOptions {
        viewport,
        // Wgpu (DirectX on Windows, Metal on Mac) instead of the
        // default Glow (OpenGL) renderer: Glow flashed black and
        // stuttered while dragging/resizing on Windows + NVIDIA,
        // in Utility Closet, File Namer AI, and Tuya alike — the
        // same egui 0.27 stack, the same default renderer, the
        // same symptom. Wgpu is the native path on both platforms.
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Homestead",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
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
    fn config_without_thermostat_parses_as_unpaired() {
        // Old config files predate the HomeKit block: they must load
        // with thermostat == None, never fail the whole file.
        let cfg: Config = serde_json::from_str(r#"{}"#).unwrap();
        assert!(cfg.thermostat.is_none());
        // A partial thermostat block also parses (serde defaults fill
        // the rest); validation, not parsing, rejects it.
        let cfg2: Config = serde_json::from_str(
            r#"{"thermostat":{"accessory_ip":"192.168.1.5"}}"#,
        )
        .unwrap();
        let p = cfg2.thermostat.unwrap();
        assert_eq!(p.accessory_ip, "192.168.1.5");
        assert_eq!(p.accessory_port, 0);
        assert!(p.validate().is_err());
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
        let secs = build_sections(&entries, &LayoutCfg::default(), false, "", true);
        // Default section order: the Thermostat section first, then
        // Cameras, Lighting, Ungrouped.
        assert_eq!(keys(&secs), vec!["thermostat", "cameras", "lighting", "ungrouped"]);
        assert_eq!(secs[0].ids, Vec::<String>::new());
        assert_eq!(secs[1].ids, vec!["cam:1".to_string()]);
        assert_eq!(secs[2].ids, vec!["lb".to_string()]);
        assert_eq!(secs[3].ids, vec!["p1".to_string()]);
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
        let secs = build_sections(&entries, &layout, false, "", true);
        assert_eq!(keys(&secs), vec!["thermostat", "g1"]);
        assert_eq!(secs[1].ids.len(), 2);
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

        // v1.3.0: brightness fan-out reaches ONLY the two field
        // bulbs — the colour-only strip is excluded (pre-1.3.0 it
        // joined via a synthesized colorSet; that route is gone).
        // v1.3.1: the fan-out value is a UI percent — UI 55 maps
        // to device 60 (11 + 54*89/99 = 59.5) on the wire.
        let plan = group_plan(&members, GroupKind::Brightness, 55, None);
        assert_eq!(plan.len(), 2);
        assert!(plan.iter().all(|(_, a, v)| *a == "brightnessSet" && *v == json!(60)));
        assert!(plan.iter().all(|(id, _, _)| id != "ls"), "strip must not be dimmed");

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

    /// The field-reported strip shape (v1.2.3): hue/saturation (and a
    /// colour mode) but NO brightness and NO colour_temp keys in the
    /// discovery payload — presence-based capability detection made
    /// it colour-only and its card had no brightness slider.
    fn strip(id: &str, name: &str) -> Device {
        dev(
            id,
            name,
            json!({ "state": true, "hue": 120, "saturation": 0.5, "color_mode": "colour" }),
        )
    }

    // ---------------- v1.3.0 Fix A: light wire formats ----------------
    // Ground truth (real discovery payloads of the legacy
    // homeassistant API, plus the user's own census): SWITCHES
    // report state as a JSON bool and numeric fields as numbers,
    // but LIGHTS report state as the STRING "true"/"false" and
    // brightness as a STRING ("800"). The strict as_bool/as_f64
    // parser read every light as Off with no brightness — plugs
    // were unaffected, which is exactly the field report.

    fn wire_dev(id: &str, name: &str, dev_type: &str, data: Value) -> Device {
        parse_device(&json!({ "id": id, "name": name, "dev_type": dev_type, "data": data }))
    }

    #[test]
    fn wire_formats_switch_bool_light_string() {
        // Switch ON (bool true) — the plug census shape.
        let on = wire_dev("p1", "Desk", "switch", json!({ "online": true, "state": true }));
        assert!(on.state);
        // Switch OFF (bool false).
        let off = wire_dev("p2", "Rice Cooker", "switch", json!({ "online": true, "state": false }));
        assert!(!off.state);
        // Light ON: string state, string brightness, numeric temp.
        let light = wire_dev(
            "l1",
            "Desk Light",
            "light",
            json!({ "brightness": "800", "color_mode": "white", "color_temp": 29000, "online": true, "state": "true" }),
        );
        assert!(light.state, "string \"true\" must parse as on");
        assert_eq!(light.brightness_pct, Some(80), "string brightness must parse");
        assert!(light.temp_slider.is_some());
        // Light OFF: string "false" (brightness still reported).
        let light_off = wire_dev(
            "l2",
            "Kitchen Light 1",
            "light",
            json!({ "brightness": "570", "color_mode": "white", "color_temp": 7635, "online": false, "state": "false" }),
        );
        assert!(!light_off.state);
        assert_eq!(light_off.brightness_pct, Some(57));
        assert!(!light_off.online);
        // The colour-only strip's real shape: colour mode + string
        // state, nothing else.
        let strip = wire_dev(
            "l3",
            "Desk RGB strip",
            "light",
            json!({ "color_mode": "colour", "online": true, "state": "true" }),
        );
        assert!(strip.state);
        assert!(strip.has_color && !strip.has_brightness && !strip.has_temp);
    }

    #[test]
    fn census_all_lights_on_plugs_match() {
        // The user's census (official Smart Life app): all six
        // lights ON; plugs Desk / Desktop-ShawnR / Server-ShawnR
        // ON, the rest OFF. Lights use the string wire format,
        // plugs the bool format.
        let lights = [
            "Desk RGB strip",
            "Desk Light",
            "Love's Bed Light",
            "Shawn's Bed Light",
            "Kitchen Light 1",
            "Kitchen Light 2",
        ];
        for (i, name) in lights.iter().enumerate() {
            let d = wire_dev(
                &format!("L{i}"),
                name,
                "light",
                json!({ "brightness": "800", "color_mode": "white", "color_temp": 5000, "online": true, "state": "true" }),
            );
            assert!(d.state, "{name} must read ON");
        }
        for name in ["Desk", "Desktop-ShawnR", "Server-ShawnR"] {
            let d = wire_dev(name, name, "switch", json!({ "online": true, "state": true }));
            assert!(d.state, "plug {name} must read ON");
        }
        for name in ["Rice Cooker", "Projector", "Kettle", "Food Processor", "Air Fryer", "Speakers"] {
            let d = wire_dev(name, name, "switch", json!({ "online": true, "state": false }));
            assert!(!d.state, "plug {name} must read OFF");
        }
    }

    #[test]
    fn light_state_survives_refresh_merge_cycles() {
        let on_payload = json!({ "id": "L1", "name": "Desk Light", "dev_type": "light",
            "data": { "brightness": "800", "color_mode": "white", "color_temp": 5000, "online": true, "state": "true" } });
        // First load.
        let first = parse_device(&on_payload);
        assert!(first.state && first.brightness_pct == Some(80));
        // A refresh repeating the same payload: state and the
        // string brightness survive the merge (pre-fix, the fresh
        // string brightness "didn't count" and the merge replaced
        // it with the previous cycle's placeholder).
        let merged = merge_device_data(vec![on_payload.clone()], &[on_payload.clone()]);
        let again = parse_device(&merged[0]);
        assert!(again.state && again.brightness_pct == Some(80));
        // A refresh reporting the light OFF (string "false") wins
        // over the previous ON — fresh real readings always win.
        let off_payload = json!({ "id": "L1", "name": "Desk Light", "dev_type": "light",
            "data": { "brightness": "800", "color_mode": "white", "color_temp": 5000, "online": true, "state": "false" } });
        let merged = merge_device_data(vec![off_payload], &[on_payload.clone()]);
        assert!(!parse_device(&merged[0]).state);
        // A THIN refresh omitting state entirely keeps last-known.
        let thin = json!({ "id": "L1", "name": "Desk Light", "dev_type": "light",
            "data": { "online": true } });
        let merged = merge_device_data(vec![thin], &[on_payload]);
        assert!(parse_device(&merged[0]).state);
    }

    #[test]
    fn strip_payload_capabilities() {
        let d = strip("ls", "Desk RGB strip");
        // Parse evidence: colour from hue/saturation; no brightness
        // FIELD and no temp field in the payload.
        assert!(d.has_color, "strip is colour-capable");
        assert!(!d.has_brightness, "strip reports no brightness field");
        assert!(!d.has_temp, "strip reports no colour_temp field");
        assert_eq!(d.brightness_pct, None, "no brightness reading either");
        // v1.3.0: no brightness field means NO brightness control
        // (the v1.2.3 colour-route synthesis is removed) — the card
        // gate and the group planner key off has_brightness alone.
        assert!(device_is_light(&d), "strip still files under Lighting");
    }

    #[test]
    fn strip_has_no_brightness_route() {
        // v1.3.0 (Fix B): a colour-only device is EXCLUDED from
        // brightness fan-out entirely — no slider, no command —
        // while still joining group Toggle and Color broadcasts.
        let d = strip("ls", "Desk RGB strip");
        let members = vec![d.clone()];
        assert!(
            group_plan(&members, GroupKind::Brightness, 40, None).is_empty(),
            "no brightness command may be planned for a colour-only device"
        );
        let colour = group_plan(&members, GroupKind::Color, 0, Some("#00ff00"));
        assert_eq!(colour.len(), 1);
        assert_eq!(colour[0].1, "colorSet");
        let toggle = group_plan(&members, GroupKind::Toggle, 0, None);
        assert_eq!(toggle.len(), 1);
        assert_eq!(toggle[0].1, "turnOnOff");
        assert!(
            group_plan(&members, GroupKind::Temp, 5000, None).is_empty(),
            "no colour-temp route either — the API reports no temp field"
        );
    }

    #[test]
    fn brightness_ui_device_mapping() {
        // v1.3.1: the UI scale is 1..100, the DEVICE scale is
        // 11..100 (the bulbs switch off entirely below 11% — a
        // hardware minimum), mapped linearly in both directions.
        // Endpoints, both directions.
        assert_eq!(brightness_ui_to_device(1), 11, "bottomed-out slider sends exactly the floor");
        assert_eq!(brightness_ui_to_device(100), 100);
        assert_eq!(brightness_device_to_ui(11), 1, "a bulb at its minimum displays 1%");
        assert_eq!(brightness_device_to_ui(100), 100);
        // Midpoint: UI 50 <-> device 55 (11 + 49*89/99 = 55.05).
        assert_eq!(brightness_ui_to_device(50), 55);
        assert_eq!(brightness_device_to_ui(55), 50);
        // Reported values below the floor (while on) display as
        // 1%, never 0 or negative.
        assert_eq!(brightness_device_to_ui(5), 1);
        assert_eq!(brightness_device_to_ui(0), 1);
        // Sends NEVER go below the floor, whatever the slider
        // (or a stale caller) asks for.
        for ui in [0, 1, 2, 25, 99, 100, 140] {
            assert!(brightness_ui_to_device(ui) >= BRIGHTNESS_FLOOR_PCT, "ui {ui} sends >= 11");
        }
        assert_eq!(brightness_set_value(1), json!(11));
        assert_eq!(brightness_set_value(100), json!(100));
        // Round trip is exact at the anchor points and never
        // drifts by more than 1 anywhere (integer rounding on a
        // 89/99 slope is not a perfect bijection).
        for ui in [1, 25, 50, 100] {
            assert_eq!(brightness_device_to_ui(brightness_ui_to_device(ui)), ui, "ui {ui} round-trips");
        }
        for ui in 1..=100 {
            let back = brightness_device_to_ui(brightness_ui_to_device(ui));
            assert!((back - ui).abs() <= 1, "ui {ui} round-trips within 1 (got {back})");
        }
    }

    #[test]
    fn bulb_brightness_move_stays_brightness_set() {
        // A full bulb payload is unchanged by the new routing.
        let d = dev(
            "lb",
            "Bulb",
            json!({ "state": true, "brightness": 800, "color_temp": 4000, "hue": 10, "saturation": 0.2 }),
        );
        assert!(d.has_brightness && d.has_temp && d.has_color);
        assert_eq!(d.brightness_pct, Some(80), "reported brightness still wins");
        // Group broadcasts carry a UI percent (v1.3.1): UI 40
        // sends device 46 (11 + 39*89/99 = 46.06).
        let plan = group_plan(&[d.clone()], GroupKind::Brightness, 40, None);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].1, "brightnessSet");
        assert_eq!(plan[0].2, json!(46));
        // A bottomed-out group slider sends exactly the floor.
        let plan = group_plan(&[d], GroupKind::Brightness, 1, None);
        assert_eq!(plan[0].2, json!(11));
    }

    #[test]
    fn temp_stops_snap_and_send() {
        // v1.3.1: five detents, snapped in Kelvin space.
        assert_eq!(TEMP_STOPS_K, [2700, 3000, 4000, 5000, 6500]);
        // Representative in-between readings (the user's bulbs)
        // display at the nearest stop.
        assert_eq!(TEMP_STOPS_K[temp_stop_index(3080)], 3000);
        assert_eq!(TEMP_STOPS_K[temp_stop_index(3780)], 4000);
        assert_eq!(TEMP_STOPS_K[temp_stop_index(3970)], 4000);
        assert_eq!(TEMP_STOPS_K[temp_stop_index(2700)], 2700);
        assert_eq!(TEMP_STOPS_K[temp_stop_index(6500)], 6500);
        assert_eq!(TEMP_STOPS_K[temp_stop_index(2000)], 2700, "below range -> first stop");
        assert_eq!(TEMP_STOPS_K[temp_stop_index(9000)], 6500, "above range -> last stop");
        // Every stop round-trips: stop -> API scale -> Kelvin
        // is the stop again (so a drag/send always lands
        // exactly on a stop).
        for (i, &k) in TEMP_STOPS_K.iter().enumerate() {
            let sent = temp_kelvin_to_slider(k);
            assert_eq!(temp_slider_to_kelvin(sent), k, "stop {k} survives the API scale");
            assert_eq!(temp_stop_kelvin(i as i64), k);
            assert_eq!(temp_display_kelvin(sent), k);
        }
        // A reported in-between value displays at its stop via
        // the full reported->slider->display path: the user's
        // 3970K bulb (reported scale 4000-ish) shows 4000K.
        let reported = temp_slider_to_reported(temp_kelvin_to_slider(3970));
        assert_eq!(temp_display_kelvin(temp_reported_to_slider(reported as f64)), 4000);
        // Index clamping at the slider ends.
        assert_eq!(temp_stop_kelvin(-3), 2700);
        assert_eq!(temp_stop_kelvin(99), 6500);
    }

    #[test]
    fn group_temp_broadcast_sends_exact_stop() {
        // v1.3.1: the group temp path sends the converted stop —
        // the same value do_temp would send for that stop.
        let d = dev(
            "lb",
            "Bulb",
            json!({ "state": true, "brightness": 800, "color_temp": 4000 }),
        );
        for &k in TEMP_STOPS_K.iter() {
            let plan = group_plan(&[d.clone()], GroupKind::Temp, temp_kelvin_to_slider(k), None);
            assert_eq!(plan.len(), 1);
            assert_eq!(plan[0].1, "colorTemperatureSet");
            assert_eq!(plan[0].2, json!(temp_kelvin_to_slider(k)), "stop {k} sent exactly");
            assert_eq!(
                temp_slider_to_kelvin(plan[0].2.as_i64().unwrap()),
                k,
                "sent value reads back as the stop"
            );
        }
    }

    #[test]
    fn version_compare() {
        assert!(version_newer("1.1.0", "1.0.0"));
        assert!(!version_newer("1.0.0", "1.1.0"));
        assert!(!version_newer("1.1.0", "1.1.0"));
        assert!(version_newer("1.10.0", "1.9.0"));
        assert!(version_newer("v1.2.0", "1.1.0"));
        assert_eq!(APP_VERSION, "2.0.6");
    }

    #[test]
    fn player_resolution_order() {
        // Bare name → PATH dirs first, then the App Paths value,
        // then well-known install dirs (the VLC fix: its installer
        // registers App Paths but never touches PATH).
        let path_dirs = vec![PathBuf::from("/bin"), PathBuf::from("/usr/bin")];
        let app_path = PathBuf::from("C:\\Program Files\\VideoLAN\\VLC\\vlc.exe");
        let known = vec![PathBuf::from("C:\\Program Files\\VideoLAN\\VLC")];
        // The on-disk file name is platform-shaped (v1.4.0):
        // "vlc.exe" on Windows, the bare name elsewhere.
        let exe_name = if cfg!(target_os = "windows") { "vlc.exe" } else { "vlc" };
        let cands = player_candidates("vlc", &path_dirs, Some(&app_path), &known);
        assert_eq!(cands[0], PathBuf::from("/bin").join(exe_name));
        assert_eq!(cands[1], PathBuf::from("/usr/bin").join(exe_name));
        assert_eq!(cands[2], app_path);
        assert_eq!(
            cands[3],
            PathBuf::from("C:\\Program Files\\VideoLAN\\VLC").join(exe_name)
        );
        assert_eq!(cands.len(), 4);
        // ".exe" isn't doubled, and no App Paths hit keeps the order.
        if cfg!(target_os = "windows") {
            assert_eq!(player_exe_name("vlc"), "vlc.exe");
            assert_eq!(player_exe_name("VLC.exe"), "VLC.exe");
        } else {
            assert_eq!(player_exe_name("vlc"), "vlc");
        }
        let cands = player_candidates("mpv", &path_dirs, None, &[]);
        assert_eq!(cands.len(), 2);
        // A path-like command is used exactly as given.
        assert_eq!(
            resolve_player_program("C:\\tools\\vlc.exe").as_deref(),
            Some("C:\\tools\\vlc.exe")
        );
    }

    #[test]
    fn reg_value_parsing() {
        let out = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\vlc.exe\r\n    (Default)    REG_SZ    C:\\Program Files\\VideoLAN\\VLC\\vlc.exe\r\n\r\n";
        assert_eq!(
            parse_reg_default_value(out).as_deref(),
            Some("C:\\Program Files\\VideoLAN\\VLC\\vlc.exe")
        );
        assert_eq!(parse_reg_default_value("ERROR: not found"), None);
        assert_eq!(parse_reg_default_value(""), None);
    }

    #[test]
    fn rate_limit_classification() {
        // Tuya's burst answer, in its exact spelling and variants.
        assert!(is_rate_limit_code("FrequentlyInvoke"));
        assert!(is_rate_limit_code("frequentlyinvoke"));
        assert!(is_rate_limit_code("FREQUENTLYINVOKE"));
        assert!(is_rate_limit_code("TooManyRequests"));
        assert!(!is_rate_limit_code("SUCCESS"));
        assert!(!is_rate_limit_code("DEVICE_OFFLINE"));
        assert!(!is_rate_limit_code(""));
        // Control classification: success / rate-limited / genuine.
        let ok = json!({ "header": { "code": "SUCCESS" } });
        assert_eq!(control_outcome(&ok), ControlOutcome::Ok);
        let rl = json!({ "header": { "code": "FrequentlyInvoke" } });
        assert_eq!(control_outcome(&rl), ControlOutcome::RateLimited);
        let bad = json!({ "header": { "code": "DEVICE_OFFLINE" } });
        assert_eq!(control_outcome(&bad), ControlOutcome::Failed);
        assert_eq!(control_outcome(&json!({})), ControlOutcome::Failed);
    }

    #[test]
    fn ffmpeg_args_and_frame_sizes() {
        let url = "rtsp://u:p@192.168.1.50:554/stream2";
        let args = ffmpeg_args(url, TILE_W, TILE_H);
        // The URL travels as ONE argv right after -i (never a shell).
        let ipos = args.iter().position(|a| a == "-i").unwrap();
        assert_eq!(args[ipos + 1], url);
        assert!(args.iter().any(|a| a == "tcp"));
        assert!(args.iter().any(|a| a
            == "scale=480:270:force_original_aspect_ratio=decrease,pad=480:270:(ow-iw)/2:(oh-ih)/2,fps=12"));
        let tail = &args[args.len() - 5..];
        assert_eq!(
            tail,
            ["-f", "rawvideo", "-pix_fmt", "rgba", "-"]
        );
        let pargs = ffmpeg_args(url, PLAYER_W, PLAYER_H);
        assert!(pargs.iter().any(|a| a.contains("scale=960:540")));
        // Exact raw-frame sizes the pump reads.
        assert_eq!(frame_bytes(TILE_W, TILE_H), 518_400);
        assert_eq!(frame_bytes(PLAYER_W, PLAYER_H), 2_073_600);
        assert_eq!(frame_bytes(2, 3), 24);
    }

    #[test]
    fn ffmpeg_lookup_order() {
        let bin = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
        // The bundled tier, in order: beside the exe everywhere,
        // plus — on macOS only — the .app's Resources copy
        // (per-arch) as its bundle equivalent. The Settings
        // override ranks after the WHOLE tier, then PATH
        // (ffmpeg_candidates' doc / DESIGN.md's platform table).
        // The expectations are indexed off the tier size so this
        // holds on every host (the first macOS CI run caught the
        // old fixed indices).
        let mut bundled = vec![Path::new("/app").join(bin)];
        if cfg!(target_os = "macos") {
            bundled.push(
                Path::new("/app")
                    .join("../Resources")
                    .join(ffmpeg_bundled_name(std::env::consts::ARCH).unwrap()),
            );
        }
        let n = bundled.len();
        let cands = ffmpeg_candidates(
            Path::new("/app"),
            "/opt/ff/ffmpeg.exe",
            "/usr/bin:/bin",
        );
        assert_eq!(&cands[..n], &bundled[..]); // bundled first
        assert_eq!(cands[n], PathBuf::from("/opt/ff/ffmpeg.exe")); // override
        assert_eq!(cands[n + 1], Path::new("/usr/bin").join(bin)); // then PATH
        assert_eq!(cands[n + 2], Path::new("/bin").join(bin));
        assert_eq!(cands.len(), n + 3);
        // A bare-name override resolves against PATH before plain PATH.
        let cands = ffmpeg_candidates(Path::new("/app"), "myffmpeg", "/usr/bin");
        assert_eq!(cands[n], Path::new("/usr/bin").join("myffmpeg"));
        // No override: just bundled + PATH.
        let cands = ffmpeg_candidates(Path::new("/app"), "", "/usr/bin");
        assert_eq!(cands.len(), n + 1);
    }

    #[test]
    fn tile_and_player_url_selection() {
        // Tiles always take Stream 2 (SD) on Tapo templates…
        assert!(tile_rtsp_url(&cam("tapo1", "")).ends_with("/stream2"));
        assert!(tile_rtsp_url(&cam("tapo2", "")).ends_with("/stream2"));
        // …custom cameras keep their own path…
        assert!(tile_rtsp_url(&cam("custom", "live/ch1")).ends_with("/live/ch1"));
        // …while the player uses the camera's current selection
        // (HD = Stream 1 when tapo1 is picked on the card).
        assert!(build_rtsp_url(&cam("tapo1", "")).ends_with("/stream1"));
        assert!(build_rtsp_url(&cam("tapo2", "")).ends_with("/stream2"));
    }

    #[test]
    fn pump_state_machine() {
        // v1.2.2 schedule: 2 s, 4 s, then an 8 s cap.
        assert_eq!(pump_backoff(0), Duration::from_secs(2));
        assert_eq!(pump_backoff(1), Duration::from_secs(4));
        assert_eq!(pump_backoff(2), Duration::from_secs(8));
        assert_eq!(pump_backoff(9), Duration::from_secs(8));

        let (_tx, rx) = channel::<FrameMsg>();
        let mut p = Pump {
            state: PumpState::Connecting,
            source_key: "k".into(),
            w: TILE_W,
            h: TILE_H,
            rx,
            stop: Arc::new(AtomicBool::new(false)),
            child: Arc::new(Mutex::new(None)),
            texture: None,
            streak: 0,
            retry_at: None,
            last_error: None,
        };
        // Connecting --frame--> Live, streak reset.
        p.note_frame();
        assert_eq!(p.state, PumpState::Live);
        // Live --ended--> Backoff at +2 s, streak 1, reason kept.
        p.note_ended(Some("Connection refused".to_string()));
        assert_eq!(p.state, PumpState::Backoff);
        assert_eq!(p.streak, 1);
        assert_eq!(p.last_error.as_deref(), Some("Connection refused"));
        let first_retry = p.retry_at.unwrap();
        assert!(first_retry > Instant::now());
        assert!(first_retry <= Instant::now() + Duration::from_secs(2));
        // A second Ended while backing off changes nothing (no
        // duplicate restarts, no streak runaway).
        p.note_ended(Some("Connection refused".to_string()));
        assert_eq!(p.streak, 1);
        assert_eq!(p.retry_at.unwrap(), first_retry);
        // Backoff --frame--> Live again, streak + reason cleared.
        p.note_frame();
        assert_eq!(p.state, PumpState::Live);
        assert_eq!(p.streak, 0);
        assert!(p.retry_at.is_none());
        assert!(p.last_error.is_none());
    }

    // ---------------- v1.2.2: failure reasons + pump decisions ----------------

    #[test]
    fn ffmpeg_failure_reason_mapping() {
        assert_eq!(
            ffmpeg_failure_reason("[tcp @ 0x1] Connection refused").as_deref(),
            Some("Connection refused")
        );
        assert_eq!(
            ffmpeg_failure_reason("Server returned 401 Unauthorized (authorization failed)").as_deref(),
            Some("401 Unauthorized")
        );
        assert_eq!(
            ffmpeg_failure_reason("Operation timed out").as_deref(),
            Some("Connection timed out")
        );
        assert_eq!(
            ffmpeg_failure_reason("Failed to resolve hostname cam.local").as_deref(),
            Some("Host not found")
        );
        assert_eq!(
            ffmpeg_failure_reason("Server returned 404 Not Found").as_deref(),
            Some("Stream path not found (404)")
        );
        assert_eq!(ffmpeg_failure_reason(""), None);
        assert_eq!(ffmpeg_failure_reason("some unrelated ffmpeg noise"), None);
    }

    #[test]
    fn pump_action_decisions() {
        use PumpState::{Backoff, Connecting, Live};
        // Missing pump → fresh start.
        assert_eq!(pump_action(None, true, false, false), PumpAction::StartFresh);
        // Source changed (edit gen / URL / size) → immediate fresh
        // restart regardless of state or backoff.
        assert_eq!(pump_action(Some(Live), false, false, false), PumpAction::StartFresh);
        assert_eq!(pump_action(Some(Backoff), false, false, false), PumpAction::StartFresh);
        // Forced (Retry / Watch / focus / player success) → fresh
        // restart out of Backoff, streak reset.
        assert_eq!(pump_action(Some(Backoff), true, true, false), PumpAction::StartFresh);
        // Lapsed backoff → scheduled retry (keeps the streak).
        assert_eq!(pump_action(Some(Backoff), true, false, true), PumpAction::RetryDue);
        // Backoff not yet due, not forced → wait.
        assert_eq!(pump_action(Some(Backoff), true, false, false), PumpAction::Keep);
        // Live / Connecting with matching source → untouched.
        assert_eq!(pump_action(Some(Live), true, false, false), PumpAction::Keep);
        assert_eq!(pump_action(Some(Connecting), true, true, false), PumpAction::Keep);
    }

    #[test]
    fn tile_pump_wanted_rules() {
        assert!(tile_pump_wanted(true, false, false, false));
        assert!(!tile_pump_wanted(false, false, false, false), "previews off");
        assert!(!tile_pump_wanted(true, true, false, false), "hidden");
        assert!(!tile_pump_wanted(true, false, true, false), "user-paused");
        assert!(
            !tile_pump_wanted(true, false, false, true),
            "the big player holds this camera's session — tile stands down"
        );
    }

    #[test]
    fn backoff_wake_delay_clamps() {
        let now = Instant::now();
        assert_eq!(backoff_wake_delay(None, now), Duration::from_millis(500));
        // Due in 300 ms → wake in 300 ms.
        assert_eq!(
            backoff_wake_delay(Some(now + Duration::from_millis(300)), now),
            Duration::from_millis(300)
        );
        // Long delay → capped at 1 s (recomputed each frame anyway).
        assert_eq!(
            backoff_wake_delay(Some(now + Duration::from_secs(30)), now),
            Duration::from_secs(1)
        );
        // Already overdue → the 50 ms floor, not zero.
        assert_eq!(
            backoff_wake_delay(Some(now), now),
            Duration::from_millis(50)
        );
    }

    // ---------------- v1.2.2: degenerate merges + sticky capabilities ----------------

    fn raw_dev(id: &str, data: Value) -> Value {
        json!({ "id": id, "name": id, "dev_type": "light", "data": data })
    }

    #[test]
    fn merge_keeps_last_known_through_thin_payloads() {
        let previous = vec![raw_dev(
            "b1",
            json!({ "state": true, "brightness": 640, "color_temp": 12000, "color_mode": "colour", "hue": 200, "saturation": 0.5 }),
        )];
        // A thin payload: brightness omitted, colour temp reported
        // as the 0 placeholder, colour mode null.
        let fresh = vec![raw_dev(
            "b1",
            json!({ "state": false, "color_temp": 0, "color_mode": null }),
        )];
        let merged = merge_device_data(fresh, &previous);
        let d = parse_device(&merged[0]);
        assert!(d.has_brightness && d.has_temp && d.has_color, "capabilities survive");
        assert_eq!(d.brightness_pct, Some(64), "last-known brightness kept");
        assert_eq!(
            d.temp_slider,
            Some(temp_reported_to_slider(12000.0)),
            "last-known temp kept — no 2700K snap"
        );
        assert_eq!(d.color_mode.as_deref(), Some("colour"));
        assert!(!d.state, "fresh state still wins (state is always a real reading)");
    }

    #[test]
    fn merge_fresh_real_values_win() {
        let previous = vec![raw_dev(
            "b1",
            json!({ "state": true, "brightness": 640, "color_temp": 12000 }),
        )];
        let fresh = vec![raw_dev(
            "b1",
            json!({ "state": true, "brightness": 300, "color_temp": 5000 }),
        )];
        let merged = merge_device_data(fresh, &previous);
        let d = parse_device(&merged[0]);
        assert_eq!(d.brightness_pct, Some(30));
        assert_eq!(d.temp_slider, Some(temp_reported_to_slider(5000.0)));
    }

    // ---------------- v2.0.3: never regress on missing state data ----------------

    #[test]
    fn merge_restores_data_wholesale_when_fresh_data_missing() {
        // The Desk Light / whole-dashboard-Off mechanism: a fresh
        // entry with no data object used to parse to all-defaults
        // (Off) and poison every later merge (it drops out of
        // prev_by_id). Now the last-known data is restored whole.
        let previous = vec![raw_dev(
            "desk",
            json!({ "state": "true", "brightness": 180, "color_temp": 5000, "online": true }),
        )];
        // No "data" key at all.
        let fresh_no_data =
            vec![json!({ "id": "desk", "name": "desk", "dev_type": "light" })];
        let merged = merge_device_data(fresh_no_data, &previous);
        let d = parse_device(&merged[0]);
        assert!(d.state, "state restored, not reset to Off");
        assert_eq!(d.brightness_pct, Some(18));
        // Null data restores the same way.
        let fresh_null_data = vec![json!({ "id": "desk", "name": "desk", "dev_type": "light", "data": null })];
        let merged = merge_device_data(fresh_null_data, &previous);
        assert!(parse_device(&merged[0]).state);
        // A non-object data value restores the same way.
        let fresh_str_data =
            vec![json!({ "id": "desk", "name": "desk", "dev_type": "light", "data": "x" })];
        let merged = merge_device_data(fresh_str_data, &previous);
        assert!(parse_device(&merged[0]).state);
    }

    #[test]
    fn partial_payload_updates_present_keeps_missing() {
        // One cycle, two lights: A reports an explicit Off (must
        // update), B arrives with no data at all (must keep On).
        let previous = vec![
            raw_dev("a", json!({ "state": true, "brightness": 800 })),
            raw_dev("b", json!({ "state": true, "brightness": 800 })),
        ];
        let fresh = vec![
            raw_dev("a", json!({ "state": false, "brightness": 800 })),
            json!({ "id": "b", "name": "b", "dev_type": "light" }),
        ];
        let merged = merge_device_data(fresh, &previous);
        assert!(!parse_device(&merged[0]).state, "explicit Off updates");
        let db = parse_device(&merged[1]);
        assert!(db.state, "missing data keeps last-known On");
        assert_eq!(db.brightness_pct, Some(80));
    }

    #[test]
    fn missing_state_diagnostic_names_only_stateless_entries() {
        let previous = vec![
            raw_dev("a", json!({ "state": true })),
            raw_dev("b", json!({ "state": true })),
            raw_dev("c", json!({ "brightness": 100 })),
        ];
        let fresh = vec![
            json!({ "id": "a", "name": "A Light", "dev_type": "light" }),
            json!({ "id": "b", "name": "B Light", "dev_type": "light", "data": { "state": false } }),
            json!({ "id": "c", "name": "C Light", "dev_type": "light", "data": {} }),
            json!({ "id": "d", "name": "D Light", "dev_type": "light" }),
        ];
        // a: previous had state, fresh has no data -> named.
        // b: fresh carries explicit state -> not named.
        // c: previous had no state key -> not named.
        // d: no previous entry -> not named.
        assert_eq!(
            devices_with_missing_state(&fresh, &previous),
            vec!["A Light".to_string()]
        );
    }

    #[test]
    fn login_style_payload_yields_correct_states_for_all_six() {
        // The user's exact scenario as a fresh (login-style)
        // payload: all six lights On, Desk Light dim/warm, the
        // RGB strip colour-only.
        let payloads = vec![
            json!({ "id": "desk", "name": "Desk Light", "dev_type": "light", "data": { "state": "true", "brightness": 180, "color_temp": 5000, "online": true } }),
            json!({ "id": "rgb", "name": "Desk RGB strip", "dev_type": "light", "data": { "state": true, "color_mode": "colour", "hue": 200, "saturation": 0.5, "online": true } }),
            json!({ "id": "k1", "name": "Kitchen Light 1", "dev_type": "light", "data": { "state": true, "brightness": 1000, "color_temp": 12000, "online": true } }),
            json!({ "id": "k2", "name": "Kitchen Light 2", "dev_type": "light", "data": { "state": true, "brightness": 1000, "color_temp": 12000, "online": true } }),
            json!({ "id": "love", "name": "Love's Bed Light", "dev_type": "light", "data": { "state": true, "brightness": 1000, "color_temp": 12000, "online": true } }),
            json!({ "id": "shawn", "name": "Shawn's Bed Light", "dev_type": "light", "data": { "state": true, "brightness": 1000, "color_temp": 12000, "online": true } }),
        ];
        let devices: Vec<Device> = payloads.iter().map(parse_device).collect();
        assert!(devices.iter().all(|d| d.state), "all six parse On");
        assert_eq!(devices[0].brightness_pct, Some(18));
        assert!(devices[1].has_color && !devices[1].has_brightness);
    }

    #[test]
    fn set_devices_retains_light_when_merged_lacks_state() {
        // Poisoned-anchor display case (the Desk Light pattern):
        // the merged entry carries brightness/temp but no state
        // key, and the previous raw anchor itself has no data, so
        // the merge cannot restore. The displayed device must
        // keep its last-known state, not reset to Off.
        let on_payload = json!({ "id": "desk", "name": "Desk Light", "dev_type": "light",
            "data": { "state": "true", "brightness": 180, "color_temp": 5000, "online": true } });
        let mut app = test_app(vec![parse_device(&on_payload)]);
        // Simulate the poisoned anchor: raw_devices holds a
        // data-less entry (as a thin cycle would have stored).
        app.raw_devices = vec![json!({ "id": "desk", "name": "Desk Light", "dev_type": "light" })];
        let stateless = json!({ "id": "desk", "name": "Desk Light", "dev_type": "light",
            "data": { "brightness": 180, "color_temp": 5000, "online": true } });
        app.set_devices(vec![stateless], false);
        let d = &app.devices[0];
        assert!(d.state, "Desk Light keeps last-known On");
        assert_eq!(d.brightness_pct, Some(18));
        // The stored entry is the retained raw, so the anchor heals.
        assert_eq!(
            app.raw_devices[0]
                .get("data")
                .and_then(|v| v.get("state"))
                .and_then(|v| v.as_str()),
            Some("true")
        );
    }

    #[test]
    fn set_devices_retains_light_when_merged_data_missing() {
        let on_payload = json!({ "id": "desk", "name": "Desk Light", "dev_type": "light",
            "data": { "state": "true", "brightness": 180, "color_temp": 5000, "online": true } });
        let mut app = test_app(vec![parse_device(&on_payload)]);
        app.raw_devices = vec![json!({ "id": "desk", "name": "Desk Light", "dev_type": "light" })];
        app.set_devices(
            vec![json!({ "id": "desk", "name": "Desk Light", "dev_type": "light" })],
            false,
        );
        assert!(app.devices[0].state, "data-less entry keeps last-known On");
    }

    #[test]
    fn set_devices_explicit_false_still_turns_light_off() {
        // Genuine Off must flow: an explicit state=false in the
        // merged entry turns the light off even with a previous On.
        let on_payload = json!({ "id": "desk", "name": "Desk Light", "dev_type": "light",
            "data": { "state": "true", "brightness": 180, "color_temp": 5000, "online": true } });
        let mut app = test_app(vec![parse_device(&on_payload)]);
        let off = json!({ "id": "desk", "name": "Desk Light", "dev_type": "light",
            "data": { "state": false, "brightness": 180, "color_temp": 5000, "online": true } });
        app.set_devices(vec![off], false);
        assert!(!app.devices[0].state, "explicit Off updates");
    }

    #[test]
    fn set_devices_scene_without_state_parses_fresh() {
        // Scenes are exempt from retention: a scene entry without
        // state data parses fresh (renames flow) instead of
        // sticking to the previous parsed scene.
        let old_scene =
            json!({ "id": "sc1", "name": "Old Name", "dev_type": "scene", "data": {} });
        let mut app = test_app(vec![parse_device(&old_scene)]);
        let renamed = json!({ "id": "sc1", "name": "New Name", "dev_type": "scene" });
        app.set_devices(vec![renamed], false);
        assert_eq!(app.devices[0].name, "New Name");
    }

    #[test]
    fn parse_stub_payload_yields_no_values_but_keeps_capability() {
        // No previous to merge against: the stub's keys are present
        // (so capability stands) but its placeholder values parse
        // as "no reading" — no 2700K slider, no 0% slider.
        let d = parse_device(&raw_dev(
            "b1",
            json!({ "state": true, "brightness": null, "color_temp": 0 }),
        ));
        assert!(d.has_brightness && d.has_temp);
        assert_eq!(d.brightness_pct, None);
        assert_eq!(d.temp_slider, None);
    }

    #[test]
    fn fresh_value_counts_rules() {
        assert!(!fresh_value_counts("brightness", &Value::Null));
        assert!(!fresh_value_counts("brightness", &json!(0)));
        assert!(fresh_value_counts("brightness", &json!(250)));
        assert!(!fresh_value_counts("color_temp", &json!(0)));
        assert!(fresh_value_counts("color_temp", &json!(9000)));
        assert!(fresh_value_counts("state", &json!(false)), "false is a real reading");
        assert!(!fresh_value_counts("color_mode", &Value::Null));
        assert!(fresh_value_counts("color_mode", &json!("colour")));
    }

    // ---------------- v1.2.2: header name budget + single instance ----------------

    #[test]
    fn header_name_budget_stretch_and_floor() {
        // The stretch of a wider cell passes through 1:1: a 350px
        // cell vs the 238px token width, same reserves, must give
        // exactly the stretch amount more budget.
        // (avail-after-icon: 350 cell → 268; 238 cell → 156.)
        let wide = header_name_budget(268.0, TRAILING_DEDUCT_DEVICE);
        let token = header_name_budget(156.0, TRAILING_DEDUCT_DEVICE);
        assert_eq!(wide - token, 112.0);
        assert_eq!(wide, 164.0, "a 350px cell fits full device names");
        // Scenes pay only for the lone hide button.
        assert_eq!(
            header_name_budget(268.0, TRAILING_DEDUCT_SCENE),
            244.0
        );
        // Cameras: pin + hide only.
        assert_eq!(
            header_name_budget(268.0, TRAILING_DEDUCT_CAMERA),
            220.0
        );
        // The 40px floor still guards absurdly narrow cells.
        assert_eq!(header_name_budget(50.0, TRAILING_DEDUCT_DEVICE), 40.0);
    }

    #[test]
    fn instance_role_from_mutex_result() {
        assert_eq!(instance_role(true), InstanceRole::Secondary);
        assert_eq!(instance_role(false), InstanceRole::Primary);
    }

    #[test]
    fn release_parse_and_installer_name() {
        // The fixture carries EVERY platform's asset (v1.4.0):
        // parse_release pulls this host's — the running app only
        // ever wants its own platform's file, so the parse result
        // is asserted host-relatively — while the explicit-OS
        // selections below pin each platform's choice on any host
        // (the first macOS CI run caught the old exe-only fixture:
        // on a Mac the parse correctly found no .dmg and the test
        // panicked expecting one to parse).
        let v = json!({
            "tag_name": "v1.2.0",
            "html_url": "https://github.com/x/releases/v1.2.0",
            "assets": [
                { "name": "Homestead.html", "browser_download_url": "https://dl/html", "size": 100 },
                { "name": "Homestead-Setup-v1.2.0.exe", "browser_download_url": "https://dl/exe", "size": 200 },
                { "name": "Homestead-v1.2.0.dmg", "browser_download_url": "https://dl/dmg", "size": 300 }
            ]
        });
        let info = parse_release(&v).expect("release parses");
        assert_eq!(info.version, "1.2.0");
        assert_eq!(info.asset_name, expected_update_asset_name(&info.version));
        let assets = v.get("assets").unwrap().as_array().unwrap();
        let win = select_update_asset(assets, "1.2.0", "windows").expect("windows asset");
        assert_eq!(
            win.get("name").and_then(|n| n.as_str()),
            Some("Homestead-Setup-v1.2.0.exe")
        );
        let mac = select_update_asset(assets, "1.2.0", "macos").expect("macos asset");
        assert_eq!(
            mac.get("name").and_then(|n| n.as_str()),
            Some("Homestead-v1.2.0.dmg")
        );
        assert_ne!(
            "Homestead-Setup-v9.9.9.exe",
            expected_update_asset_name(&info.version)
        );
        assert!(parse_release(&json!({ "tag_name": "v1.2.0", "html_url": "x", "assets": [] })).is_none());
    }

    #[test]
    fn update_asset_selection_per_platform() {
        // One release carrying every platform's assets: each
        // platform must pick exactly its own file — the macOS
        // app must never be offered the Windows .exe, nor the
        // Windows app the .dmg (v1.4.0).
        let assets = json!([
            { "name": "Homestead.html", "browser_download_url": "https://dl/html", "size": 100 },
            { "name": "Homestead-Setup-v1.4.0.exe", "browser_download_url": "https://dl/exe", "size": 200 },
            { "name": "Homestead-v1.4.0.dmg", "browser_download_url": "https://dl/dmg", "size": 300 }
        ]);
        let assets = assets.as_array().unwrap();
        let win = select_update_asset(assets, "1.4.0", "windows").expect("windows asset");
        assert_eq!(
            win.get("name").and_then(|n| n.as_str()),
            Some("Homestead-Setup-v1.4.0.exe")
        );
        let mac = select_update_asset(assets, "1.4.0", "macos").expect("macos asset");
        assert_eq!(
            mac.get("name").and_then(|n| n.as_str()),
            Some("Homestead-v1.4.0.dmg")
        );
        assert_eq!(mac.get("size").and_then(|s| s.as_u64()), Some(300));
        // A release missing the platform's asset selects nothing
        // (no update is offered) rather than the wrong file.
        assert!(select_update_asset(&assets[..2], "1.4.0", "macos").is_none());
        assert_eq!(
            expected_update_asset_name_for("1.4.0", "macos"),
            "Homestead-v1.4.0.dmg"
        );
        assert_eq!(
            expected_update_asset_name_for("1.4.0", "windows"),
            "Homestead-Setup-v1.4.0.exe"
        );
        // And the install step matches the asset: run + exit for
        // the installer, open for the disk image.
        assert_eq!(update_install_for("macos"), UpdateInstall::OpenDiskImage);
        assert_eq!(
            update_install_for("windows"),
            UpdateInstall::RunInstallerAndExit
        );
    }

    // ---------------- v1.2.1: section order ----------------

    fn full_entry_set() -> Vec<CardEntry> {
        // bulb (Lighting + pinnable), plug p1 (the group's member),
        // plug p2 (stays Ungrouped), a camera, a scene — so every
        // section kind has a member (empty sections are retained
        // out of the non-editing model).
        let b = bulb("lb", "Bright bulb");
        let p1 = dev("p1", "Plug one", json!({ "state": false }));
        let p2 = dev("p2", "Plug two", json!({ "state": false }));
        let c = cam("tapo1", "");
        let mut sc = dev("s1", "Movie night", json!({ "state": false }));
        sc.is_scene = true;
        vec![
            device_entry(&b),
            device_entry(&p1),
            device_entry(&p2),
            camera_entry(&c),
            device_entry(&sc),
        ]
    }

    fn grouped_layout() -> LayoutCfg {
        let mut layout = layout_with_group();
        layout.pins.push("lb".into());
        layout.assign.insert("p1".into(), "g1".into());
        layout
    }

    #[test]
    fn section_order_default_thermostat_first() {
        let layout = grouped_layout();
        let secs = build_sections(&full_entry_set(), &layout, false, "", true);
        assert_eq!(
            keys(&secs),
            vec!["thermostat", "cameras", "pinned", "lighting", "g1", "ungrouped", "scenes"]
        );
        // And default_section_seq over the same list agrees.
        assert_eq!(
            default_section_seq(&secs),
            vec!["thermostat", "cameras", "pinned", "lighting", "group:g1", "ungrouped", "scenes"]
        );
    }

    #[test]
    fn section_order_stored_wins() {
        let mut layout = LayoutCfg::default();
        layout.section_seq = vec![
            "scenes".into(),
            "ungrouped".into(),
            "lighting".into(),
            "cameras".into(),
        ];
        let secs = build_sections(&full_entry_set(), &layout, false, "", true);
        // The thermostat is missing from the stored order, so it
        // slots first (no default-order predecessor); the rest
        // keeps the stored order.
        assert_eq!(keys(&secs), vec!["thermostat", "scenes", "ungrouped", "lighting", "cameras"]);
    }

    #[test]
    fn section_order_new_sections_slot_by_default() {
        // Stored order knows only Scenes + Lighting (say it was saved
        // before any camera/pin/group existed). Everything missing
        // slots in by the default order around them.
        let mut layout = grouped_layout();
        layout.section_seq = vec!["scenes".into(), "lighting".into()];
        let secs = build_sections(&full_entry_set(), &layout, false, "", true);
        // thermostat (front, no predecessor) → cameras (after
        // thermostat) → pinned (after cameras) → scenes → lighting
        // (stored relative order) → group (after lighting) →
        // ungrouped (after the group).
        assert_eq!(
            keys(&secs),
            vec!["thermostat", "cameras", "pinned", "scenes", "lighting", "g1", "ungrouped"]
        );
    }

    #[test]
    fn section_move_and_layout_persistence_roundtrip() {
        // Moving within the RENDERED list, ends are no-ops, and the
        // result survives a serde round-trip of the whole layout.
        let mut layout = grouped_layout();
        let secs = build_sections(&full_entry_set(), &layout, false, "", true);
        assert_eq!(
            keys(&secs),
            vec!["thermostat", "cameras", "pinned", "lighting", "g1", "ungrouped", "scenes"]
        );
        // Move Ungrouped up one: it swaps with the group.
        let seq = moved_section_seq(&layout.section_seq, &secs, "ungrouped", -1);
        assert_eq!(
            seq,
            vec!["thermostat", "cameras", "pinned", "lighting", "ungrouped", "group:g1", "scenes"]
        );
        let seq2 = moved_section_seq(&seq, &secs, "thermostat", -1);
        assert_eq!(seq2, seq); // already first: no-op
        let seq3 = moved_section_seq(&seq, &secs, "scenes", 1);
        assert_eq!(seq3, seq); // already last: no-op
        // The thermostat itself moves like any other section: down
        // one swaps it with Cameras, back up restores it.
        let seq_down = moved_section_seq(&seq, &secs, "thermostat", 1);
        assert_eq!(
            seq_down,
            vec!["cameras", "thermostat", "pinned", "lighting", "ungrouped", "group:g1", "scenes"]
        );
        let seq_back = moved_section_seq(&seq_down, &secs, "thermostat", -1);
        assert_eq!(seq_back, seq);
        // Stored keys for sections that aren't rendered right now
        // are re-appended, not forgotten.
        let mut layout2 = LayoutCfg::default();
        layout2.assign.insert("p1".into(), "g1".into());
        layout2.groups.push(Group { id: "g1".into(), name: "Kitchen".into() });
        let secs_no_pins = build_sections(&full_entry_set(), &layout2, false, "", true);
        let with_extra = vec!["pinned".to_string()];
        let seq4 = moved_section_seq(&with_extra, &secs_no_pins, "cameras", 1);
        assert_eq!(seq4.last().map(|s| s.as_str()), Some("pinned"));

        layout.section_seq = seq.clone();
        layout.cam_sizes.insert("cam:1".into(), "wide".into());
        let text = serde_json::to_string(&layout).unwrap();
        let back: LayoutCfg = serde_json::from_str(&text).unwrap();
        assert_eq!(back.section_seq, layout.section_seq);
        assert_eq!(back.cam_sizes.get("cam:1").map(|s| s.as_str()), Some("wide"));
        // And a rebuild with the round-tripped layout honours it:
        // the thermostat stays on top, pinned has moved with the
        // bulb, ungrouped sits above g1.
        let secs2 = build_sections(&full_entry_set(), &back, false, "", true);
        assert_eq!(
            keys(&secs2),
            vec!["thermostat", "cameras", "pinned", "lighting", "ungrouped", "g1", "scenes"]
        );
    }

    #[test]
    fn thermostat_status_line_shows_target_mode() {
        // v1.6.4: the line must read the SET mode (what the wall
        // unit shows), not the live idle/heating/cooling state — a
        // unit set to Cool but idle reads "Cool", not "Off".
        let st = crate::hap::ThermostatState {
            current_mode: 0,
            target_mode: 2,
            current_temp_c: 25.0,
            target_temp_c: 25.0,
            ..Default::default()
        };
        assert_eq!(
            App::thermostat_status_line(&st),
            "Cool — indoor 77°F, setpoint 77°F"
        );
    }

    #[test]
    fn thermostat_move_down_from_first_with_legacy_stored_seq() {
        // v1.6.4: a config saved before v1.6.3 has no "thermostat"
        // in section_seq at all — the ↓ move must still work,
        // because moved_section_seq starts from the EFFECTIVE
        // sequence (which slots the thermostat first), not the raw
        // stored one.
        let mut layout = grouped_layout();
        layout.section_seq = vec![
            "cameras".into(),
            "pinned".into(),
            "lighting".into(),
            "group:g1".into(),
            "ungrouped".into(),
            "scenes".into(),
        ];
        let secs = build_sections(&full_entry_set(), &layout, false, "", true);
        assert_eq!(secs[0].key, "thermostat");
        // First position: ↑ disabled, ↓ enabled (the header's
        // add_enabled conditions: sec_idx > 0 / sec_idx + 1 < total).
        let (sec_idx, sec_total) = (0usize, secs.len());
        assert!(!(sec_idx > 0));
        assert!(sec_idx + 1 < sec_total);
        let down = moved_section_seq(&layout.section_seq, &secs, "thermostat", 1);
        assert_eq!(down[1], "thermostat");
        // The new stored order renders thermostat second, and
        // moving back up restores it to first.
        let mut layout2 = grouped_layout();
        layout2.section_seq = down.clone();
        let secs2 = build_sections(&full_entry_set(), &layout2, false, "", true);
        assert_eq!(secs2[1].key, "thermostat");
        let back = moved_section_seq(&down, &secs2, "thermostat", -1);
        assert_eq!(back[0], "thermostat");
    }

    #[test]
    fn thermostat_collapsed_renders_header_only() {
        // v1.6.4: collapsing the Thermostat section hides the card
        // but keeps the header, and the collapsed flag round-trips
        // through the saved layout. v2.0.2: the test app is paired
        // — an unpaired dashboard has no Thermostat section at all
        // (see thermostat_hidden_until_paired), so collapse is only
        // meaningful when paired.
        let mut app = gate_app();
        app.cfg.thermostat = Some(crate::hap::ThermostatPairing::default());
        app.cfg.layout.collapsed.insert("thermostat".to_string(), true);
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let texts = render_dashboard(&mut app, &ctx, egui::vec2(1330.0, 1200.0), 3);
        let drawn: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        assert!(drawn.iter().any(|t| *t == "Thermostat"));
        assert!(
            !drawn.iter().any(|t| t.contains("No reading yet")),
            "collapsed thermostat must hide the card: {:?}",
            drawn
        );
        let text = serde_json::to_string(&app.cfg.layout).unwrap();
        let back: LayoutCfg = serde_json::from_str(&text).unwrap();
        assert_eq!(back.collapsed.get("thermostat"), Some(&true));
        // Expanded, the card is back.
        let mut app2 = gate_app();
        app2.cfg.thermostat = Some(crate::hap::ThermostatPairing::default());
        let ctx2 = egui::Context::default();
        install_fonts(&ctx2);
        let texts2 = render_dashboard(&mut app2, &ctx2, egui::vec2(1330.0, 1200.0), 3);
        assert!(texts2.iter().any(|(t, _)| t.contains("No reading yet")));
    }

    #[test]
    fn thermostat_hidden_until_paired() {
        // v2.0.2: like Cameras, the Thermostat section does not
        // exist on the dashboard until a thermostat is paired —
        // no section, no card, no "Not paired" text, on the
        // dashboard, with no devices, or under a search.
        let secs = build_sections(&full_entry_set(), &LayoutCfg::default(), false, "", false);
        assert!(!secs.iter().any(|s| s.kind == SectionKind::Thermostat));
        let paired = build_sections(&full_entry_set(), &LayoutCfg::default(), false, "", true);
        assert!(paired[0].kind == SectionKind::Thermostat);
        // Dashboard render level: unpaired shows no Thermostat
        // header at all; paired shows it.
        let mut app = gate_app();
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let texts = render_dashboard(&mut app, &ctx, egui::vec2(1330.0, 1200.0), 3);
        assert!(!texts.iter().any(|(t, _)| t.as_str() == "Thermostat"));
        let mut app2 = gate_app();
        app2.cfg.thermostat = Some(crate::hap::ThermostatPairing::default());
        let ctx2 = egui::Context::default();
        install_fonts(&ctx2);
        let texts2 = render_dashboard(&mut app2, &ctx2, egui::vec2(1330.0, 1200.0), 3);
        assert!(texts2.iter().any(|(t, _)| t.as_str() == "Thermostat"));
        // Search matching nothing: unpaired shows only the note,
        // paired still shows the thermostat above the note.
        let mut app3 = gate_app();
        app3.query = "zzz-no-such-device".to_string();
        let ctx3 = egui::Context::default();
        install_fonts(&ctx3);
        let texts3 = render_dashboard(&mut app3, &ctx3, egui::vec2(1330.0, 1200.0), 3);
        assert!(!texts3.iter().any(|(t, _)| t.as_str() == "Thermostat"));
        assert!(texts3
            .iter()
            .any(|(t, _)| t.contains("Nothing matches your search")));
        let mut app4 = gate_app();
        app4.cfg.thermostat = Some(crate::hap::ThermostatPairing::default());
        app4.query = "zzz-no-such-device".to_string();
        let ctx4 = egui::Context::default();
        install_fonts(&ctx4);
        let texts4 = render_dashboard(&mut app4, &ctx4, egui::vec2(1330.0, 1200.0), 3);
        assert!(texts4.iter().any(|(t, _)| t.as_str() == "Thermostat"));
        assert!(texts4
            .iter()
            .any(|(t, _)| t.contains("Nothing matches your search")));
    }

    #[test]
    fn add_thermostat_button_routes_to_settings() {
        // v2.0.2: the edit bar's "+ Add thermostat" button opens
        // Settings and arms the jump to the Thermostat panel.
        let mut app = gate_app();
        assert!(app.cfg.thermostat.is_none());
        app.open_thermostat_settings();
        assert!(matches!(app.view, View::Settings));
        assert!(app.settings_jump_thermostat);
        // The button itself renders in the edit bar only while
        // unpaired and editing.
        let mut app2 = gate_app();
        app2.editing = true;
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let texts = render_header(&mut app2, &ctx, egui::vec2(1330.0, 200.0), 3);
        assert!(texts.iter().any(|(t, _)| t.as_str() == "+ Add thermostat"));
        let mut app3 = gate_app();
        app3.editing = true;
        app3.cfg.thermostat = Some(crate::hap::ThermostatPairing::default());
        let ctx3 = egui::Context::default();
        install_fonts(&ctx3);
        let texts3 = render_header(&mut app3, &ctx3, egui::vec2(1330.0, 200.0), 3);
        assert!(!texts3.iter().any(|(t, _)| t.as_str() == "+ Add thermostat"));
        let mut app4 = gate_app();
        let ctx4 = egui::Context::default();
        install_fonts(&ctx4);
        let texts4 = render_header(&mut app4, &ctx4, egui::vec2(1330.0, 200.0), 3);
        assert!(!texts4.iter().any(|(t, _)| t.as_str() == "+ Add thermostat"));
    }

    #[test]
    fn thermostat_fetch_busy_shows_no_working_indicator() {
        // v1.6.4 busy split: a status fetch must not show
        // "Working…" on the card; a user action must.
        let mut app = gate_app();
        app.cfg.thermostat = Some(crate::hap::ThermostatPairing::default());
        app.tstat_state = Some(crate::hap::ThermostatState {
            current_mode: 0,
            target_mode: 2,
            current_temp_c: 25.0,
            target_temp_c: 25.0,
            ..Default::default()
        });
        app.tstat_last_fetch = Some(std::time::Instant::now());
        // Fetch-busy only: silent.
        app.tstat_busy = false;
        app.tstat_fetch_busy = true;
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let texts = render_dashboard(&mut app, &ctx, egui::vec2(1330.0, 1200.0), 3);
        assert!(
            !texts.iter().any(|(t, _)| t == "Working…"),
            "fetch-busy must not show Working…: {:?}",
            texts.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>()
        );
        // Action-busy: the indicator shows.
        app.tstat_busy = true;
        app.tstat_fetch_busy = false;
        let ctx2 = egui::Context::default();
        install_fonts(&ctx2);
        let texts2 = render_dashboard(&mut app, &ctx2, egui::vec2(1330.0, 1200.0), 3);
        assert!(texts2.iter().any(|(t, _)| t == "Working…"));
    }

    #[test]
    fn thermostat_set_busy_shows_no_working_indicator() {
        // v2.0.1 busy split: a tile/bridge set must not show
        // "Working…" on the card — the tile patches instantly
        // instead. Only discovery/pairing (tstat_busy) show it.
        let mut app = gate_app();
        app.cfg.thermostat = Some(crate::hap::ThermostatPairing::default());
        app.tstat_state = Some(crate::hap::ThermostatState {
            current_mode: 0,
            target_mode: 2,
            current_temp_c: 25.0,
            target_temp_c: 25.0,
            ..Default::default()
        });
        app.tstat_last_fetch = Some(std::time::Instant::now());
        app.tstat_busy = false;
        app.tstat_fetch_busy = false;
        app.tstat_set_busy = true;
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let texts = render_dashboard(&mut app, &ctx, egui::vec2(1330.0, 1200.0), 3);
        assert!(
            !texts.iter().any(|(t, _)| t == "Working…"),
            "set-busy must not show Working…: {:?}",
            texts.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn thermostat_set_failed_arms_revert_and_keeps_error() {
        // v2.0.1 failure revert: a SetFailed result clears set-busy,
        // keeps the error line, and arms the revert guard. The
        // re-fetch spawn itself is covered by the StateConfirm
        // precedent (same one-liner); here fetch-busy is pre-set so
        // no thread spawns and the test stays single-threaded and
        // deterministic.
        let mut app = gate_app();
        app.cfg.thermostat = Some(crate::hap::ThermostatPairing::default());
        app.tstat_set_busy = true;
        app.tstat_fetch_busy = true;
        app.tstat_tx
            .send(TstatResult::SetFailed(
                "Thermostat control failed: boom".to_string(),
            ))
            .unwrap();
        app.poll_net();
        assert!(!app.tstat_set_busy);
        assert_eq!(
            app.tstat_error.as_deref(),
            Some("Thermostat control failed: boom")
        );
        assert!(app.tstat_revert_pending);
    }

    #[test]
    fn thermostat_revert_fetch_keeps_set_error() {
        // v2.0.1: the revert fetch reverts the tile's state but must
        // not erase the set error it was triggered by — in either
        // outcome. A later normal fetch clears errors as before
        // (flag already consumed).
        let mut app = gate_app();
        app.cfg.thermostat = Some(crate::hap::ThermostatPairing::default());
        app.tstat_state = Some(crate::hap::ThermostatState {
            target_mode: 2,
            target_temp_c: 25.0,
            ..Default::default()
        });
        app.tstat_error = Some("Thermostat control failed: boom".to_string());
        app.tstat_revert_pending = true;
        app.tstat_fetch_busy = true;
        app.tstat_tx
            .send(TstatResult::Fetched(crate::hap::ThermostatState {
                target_mode: 1,
                target_temp_c: 21.0,
                ..Default::default()
            }))
            .unwrap();
        app.poll_net();
        assert!(!app.tstat_fetch_busy);
        assert!(!app.tstat_revert_pending);
        assert_eq!(app.tstat_state.as_ref().unwrap().target_mode, 1);
        assert_eq!(
            app.tstat_error.as_deref(),
            Some("Thermostat control failed: boom")
        );
        // A failed revert fetch likewise keeps the set error.
        app.tstat_revert_pending = true;
        app.tstat_fetch_busy = true;
        app.tstat_tx
            .send(TstatResult::FetchFailed(
                "Thermostat refresh failed: down".to_string(),
            ))
            .unwrap();
        app.poll_net();
        assert!(!app.tstat_fetch_busy);
        assert!(!app.tstat_revert_pending);
        assert_eq!(
            app.tstat_error.as_deref(),
            Some("Thermostat control failed: boom")
        );
    }

    #[test]
    fn thermostat_tap_while_set_busy_paints_and_queues() {
        // v2.0.2 never-disable rule: a tap during a running set
        // (e.g. a slow one heading for a timeout) still paints at
        // once and queues as the pending intent instead of being
        // dropped.
        let mut app = gate_app();
        app.cfg.thermostat = Some(crate::hap::ThermostatPairing::default());
        app.tstat_state = Some(crate::hap::ThermostatState {
            current_mode: 0,
            target_mode: 0,
            current_temp_c: 25.0,
            target_temp_c: 25.0,
            fan_on: Some(false),
            ..Default::default()
        });
        app.tstat_set_busy = true;
        app.tstat_tap(crate::hap::ThermostatChanges {
            target_mode: Some(2),
            ..Default::default()
        });
        assert_eq!(app.tstat_state.as_ref().unwrap().target_mode, 2);
        let pending = app.tstat_pending_set.as_ref().expect("tap queued");
        assert_eq!(pending.target_mode, Some(2));
        assert!(app.tstat_set_busy, "in-flight set untouched");
    }

    #[test]
    fn thermostat_queued_taps_merge_last_write_wins() {
        // v2.0.2: taps queued behind a running set merge per field
        // — a mode tap followed by a setpoint tap sends both, and
        // a repeated mode tap keeps only the latest mode.
        let mut app = gate_app();
        app.cfg.thermostat = Some(crate::hap::ThermostatPairing::default());
        app.tstat_state = Some(crate::hap::ThermostatState {
            target_mode: 0,
            target_temp_c: 25.0,
            ..Default::default()
        });
        app.tstat_set_busy = true;
        app.tstat_tap(crate::hap::ThermostatChanges {
            target_mode: Some(2),
            ..Default::default()
        });
        app.tstat_tap(crate::hap::ThermostatChanges {
            target_temp_c: Some(26.0),
            ..Default::default()
        });
        let pending = app.tstat_pending_set.as_ref().unwrap();
        assert_eq!(pending.target_mode, Some(2));
        assert_eq!(pending.target_temp_c, Some(26.0));
        assert_eq!(app.tstat_state.as_ref().unwrap().target_mode, 2);
        assert_eq!(app.tstat_state.as_ref().unwrap().target_temp_c, 26.0);
        app.tstat_tap(crate::hap::ThermostatChanges {
            target_mode: Some(1),
            ..Default::default()
        });
        let pending = app.tstat_pending_set.as_ref().unwrap();
        assert_eq!(pending.target_mode, Some(1));
        assert_eq!(pending.target_temp_c, Some(26.0));
    }

    #[test]
    fn thermostat_pending_set_drains_only_when_free() {
        // v2.0.2: the drain hands the pending intent out only when
        // no operation is running, and consumes the slot when it
        // does. (No pairing here, so the drain's spawn fails fast
        // with no thread — the gate itself is what is tested.)
        let mut app = gate_app();
        app.tstat_pending_set = Some(crate::hap::ThermostatChanges {
            target_mode: Some(2),
            ..Default::default()
        });
        app.tstat_set_busy = true;
        app.drain_tstat_pending_set();
        assert!(app.tstat_pending_set.is_some(), "set-busy keeps slot");
        app.tstat_set_busy = false;
        app.tstat_fetch_busy = true;
        app.drain_tstat_pending_set();
        assert!(app.tstat_pending_set.is_some(), "fetch-busy keeps slot");
        app.tstat_fetch_busy = false;
        app.tstat_busy = true;
        app.drain_tstat_pending_set();
        assert!(app.tstat_pending_set.is_some(), "action-busy keeps slot");
        app.tstat_busy = false;
        app.drain_tstat_pending_set();
        assert!(app.tstat_pending_set.is_none(), "free channel drains slot");
    }

    #[test]
    fn thermostat_set_failed_with_queued_tap_skips_revert() {
        // v2.0.2: when a tap was queued behind a failing set, the
        // failure does not arm the revert fetch — the queued tap
        // supersedes it (its own result settles the tile). The
        // error line itself is still kept.
        let mut app = gate_app();
        app.tstat_set_busy = true;
        app.tstat_pending_set = Some(crate::hap::ThermostatChanges {
            target_mode: Some(2),
            ..Default::default()
        });
        app.tstat_tx
            .send(TstatResult::SetFailed(
                "Thermostat control failed: boom".to_string(),
            ))
            .unwrap();
        app.poll_net();
        assert!(!app.tstat_set_busy);
        assert_eq!(
            app.tstat_error.as_deref(),
            Some("Thermostat control failed: boom")
        );
        assert!(!app.tstat_revert_pending, "queued tap supersedes revert");
        assert!(app.tstat_pending_set.is_none(), "queued tap drained");
    }

    #[test]
    fn humanize_tstat_error_plain_text() {
        // v2.0.3: the tile must never show a raw OS error dump.
        let dumped = App::humanize_tstat_error(
            "network error: A connection attempt failed because the connected party did not properly respond after a period of time, or established connection failed because connected host has failed to respond. (os error 10060)",
        );
        assert_eq!(
            dumped,
            "the thermostat could not be reached — check that it is online and try again"
        );
        assert!(!dumped.contains("os error"));
        assert_eq!(
            App::humanize_tstat_error("thermostat rejected write(s): 1.2 -> status -70410"),
            "the thermostat did not accept the change"
        );
        assert_eq!(
            App::humanize_tstat_error("bad accessory response: trailing characters"),
            "the thermostat did not confirm the change — try again"
        );
    }

    #[test]
    fn thermostat_verified_success_with_queued_tap_drains_no_error() {
        // v2.0.3: an older set whose PUT response was lost but whose
        // write verified against the unit arrives as State, not
        // SetFailed — so a tap queued behind it must never see a
        // false failure: no error line, and the queued tap drains
        // (its own result settles the tile next).
        let mut app = gate_app();
        app.tstat_set_busy = true;
        app.tstat_pending_set = Some(crate::hap::ThermostatChanges {
            target_mode: Some(1),
            ..Default::default()
        });
        app.tstat_tx
            .send(TstatResult::State(crate::hap::ThermostatState {
                target_mode: 2,
                target_temp_c: 25.5,
                ..Default::default()
            }))
            .unwrap();
        app.poll_net();
        assert!(!app.tstat_set_busy);
        assert_eq!(app.tstat_error, None);
        assert_eq!(app.tstat_state.as_ref().unwrap().target_mode, 2);
        assert!(app.tstat_pending_set.is_none(), "queued tap drained");
    }

    #[test]
    fn thermostat_buttons_stay_clickable_while_set_busy() {
        // v2.0.2 field gate: with a set in flight, clicking the
        // Cool button still paints at once and queues the tap.
        // A disabled egui button swallows the click, so a
        // painted+queued tap is the clickability proof — and it
        // also proves the button is not rendered dim, since
        // egui's dim look is exactly the disabled state.
        let mut app = gate_app();
        app.cfg.thermostat = Some(crate::hap::ThermostatPairing::default());
        app.tstat_state = Some(crate::hap::ThermostatState {
            current_mode: 0,
            target_mode: 0,
            current_temp_c: 25.0,
            target_temp_c: 25.0,
            fan_on: Some(false),
            ..Default::default()
        });
        app.tstat_last_fetch = Some(std::time::Instant::now());
        app.tstat_set_busy = true;
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let size = egui::vec2(1330.0, 1200.0);
        let mut clock = 10.0;
        drive_frame(&mut app, &ctx, size, &mut clock, vec![]);
        let out = drive_frame(&mut app, &ctx, size, &mut clock, vec![]);
        let texts = drawn_texts(&out);
        let cool = texts
            .iter()
            .find(|(t, _)| t.as_str() == "Cool")
            .map(|(_, p)| *p)
            .expect("Cool button drawn");
        drive_frame(&mut app, &ctx, size, &mut clock, vec![egui::Event::PointerMoved(cool)]);
        drive_frame(
            &mut app,
            &ctx,
            size,
            &mut clock,
            vec![egui::Event::PointerButton {
                pos: cool,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        drive_frame(
            &mut app,
            &ctx,
            size,
            &mut clock,
            vec![egui::Event::PointerButton {
                pos: cool,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        drive_frame(&mut app, &ctx, size, &mut clock, vec![]);
        assert_eq!(
            app.tstat_state.as_ref().unwrap().target_mode,
            2,
            "tap painted while set busy"
        );
        let pending = app
            .tstat_pending_set
            .as_ref()
            .expect("tap queued while set busy");
        assert_eq!(pending.target_mode, Some(2));
    }

    #[test]
    fn thermostat_header_refresh_click_spawns_fetch_when_free() {
        // v2.0.2: the section header's Refresh button is never
        // disabled — clicking it while free spawns the fetch.
        // (The spawned thread fails fast on the default pairing's
        // empty keys and its result is never polled here.)
        let mut app = gate_app();
        app.cfg.thermostat = Some(crate::hap::ThermostatPairing::default());
        app.tstat_state = Some(crate::hap::ThermostatState {
            current_mode: 0,
            target_mode: 2,
            current_temp_c: 25.0,
            target_temp_c: 25.0,
            ..Default::default()
        });
        app.tstat_last_fetch = Some(std::time::Instant::now());
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let size = egui::vec2(1330.0, 1200.0);
        let mut clock = 10.0;
        drive_frame(&mut app, &ctx, size, &mut clock, vec![]);
        let out = drive_frame(&mut app, &ctx, size, &mut clock, vec![]);
        let texts = drawn_texts(&out);
        let refresh = texts
            .iter()
            .find(|(t, _)| t.as_str() == "↻ Refresh")
            .map(|(_, p)| *p)
            .expect("Refresh button drawn");
        drive_frame(&mut app, &ctx, size, &mut clock, vec![egui::Event::PointerMoved(refresh)]);
        drive_frame(
            &mut app,
            &ctx,
            size,
            &mut clock,
            vec![egui::Event::PointerButton {
                pos: refresh,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        drive_frame(
            &mut app,
            &ctx,
            size,
            &mut clock,
            vec![egui::Event::PointerButton {
                pos: refresh,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        drive_frame(&mut app, &ctx, size, &mut clock, vec![]);
        assert!(app.tstat_fetch_busy, "Refresh click spawned a fetch");
    }

    fn test_session() -> Session {
        Session {
            access_token: "test-token".to_string(),
            refresh_token: None,
            expires_at_ms: 0,
            region: "1".to_string(),
        }
    }

    #[test]
    fn disc_frequent_leaves_timestamp_frozen_and_sets_backoff() {
        // v2.0.2 regression: a rate-limited cycle is silent by
        // design (no toast), but it must never move the Updated
        // stamp — a frozen stamp with stale tiles is exactly how a
        // DiscFrequent storm masquerades as all-Off, and the
        // diagnostics log is what names the gate instead.
        let mut app = test_app(vec![]);
        app.last_updated = "Updated 14:14:46".to_string();
        app.disc_busy = true;
        app.tx
            .send(NetResult::DiscFrequent { session: test_session() })
            .unwrap();
        app.poll_net();
        assert!(!app.disc_busy);
        assert_eq!(app.last_updated, "Updated 14:14:46");
        assert_eq!(app.disc_freq_streak, 1);
        assert!(app.disc_backoff_until.is_some());
    }

    #[test]
    fn disc_ok_updates_timestamp() {
        // The complement: a completed cycle moves the stamp and
        // replaces the device list with the fresh states.
        let mut app = test_app(vec![]);
        app.last_updated = "Updated 14:14:46".to_string();
        app.disc_busy = true;
        app.tx
            .send(NetResult::DiscOk {
                session: test_session(),
                devices: vec![json!({
                    "id": "l1",
                    "name": "Desk Light",
                    "dev_type": "light",
                    "data": { "online": true, "state": true, "brightness": 800 }
                })],
            })
            .unwrap();
        app.poll_net();
        assert!(!app.disc_busy);
        assert_ne!(app.last_updated, "Updated 14:14:46");
        assert!(app.last_updated.starts_with("Updated "));
        assert_eq!(app.devices.len(), 1);
        assert!(app.devices[0].state);
        assert!(app.disc_backoff_until.is_none());
    }

    #[test]
    fn auth_failure_code_classifier() {
        // Token/auth/session wording means the session is dead;
        // success, rate-limit, and device-level codes are
        // explicitly not auth failures.
        for code in [
            "InvalidAccessToken",
            "invalidaccesstoken",
            "TokenExpired",
            "AccessTokenInvalid",
            "Unauthorized",
            "SessionExpired",
            "LoginRequired",
            "PermissionDenied",
        ] {
            assert!(
                is_auth_failure_code(code),
                "{code} should classify as auth failure"
            );
        }
        for code in [
            "SUCCESS",
            "FrequentlyInvoke",
            "TooManyRequests",
            "InvalidDeviceId",
            "DeviceOffline",
            "",
        ] {
            assert!(
                !is_auth_failure_code(code),
                "{code} should not classify as auth failure"
            );
        }
    }

    #[test]
    fn session_expired_hint_decision() {
        // Pure decision behind the DiscErr handler's hint line:
        // only fresh_session's "Session expired" with no saved
        // login names the missing-credentials gate.
        assert!(session_expired_hint_needed("Session expired", false));
        assert!(!session_expired_hint_needed("Session expired", true));
        assert!(!session_expired_hint_needed(
            "API error: InvalidAccessToken",
            false
        ));
        assert!(!session_expired_hint_needed("Network error: boom", false));
    }

    #[test]
    fn refresh_relogin_allowed_respects_throttle_window() {
        // v2.0.4: the throttle window gates re-login attempts only;
        // discovery itself always runs on whatever session results.
        assert!(refresh_relogin_allowed(1000, 0));
        assert!(refresh_relogin_allowed(1000, 1000)); // boundary: allowed at expiry
        assert!(!refresh_relogin_allowed(999, 1000));
        assert!(!refresh_relogin_allowed(0, 300_000));
        assert!(refresh_relogin_allowed(300_000, 300_000));
    }

    #[test]
    fn refresh_relogin_backoff_window_is_five_minutes() {
        // Matches the discovery backoff cap (300s).
        assert_eq!(REFRESH_RELOGIN_BACKOFF_MS, 300_000);
    }

    #[test]
    fn refresh_relogin_gate_boundary_185s() {
        // v2.0.5: the gate is 180 s + 5 s skew margin. Throttle
        // lapsed (until = 0) throughout, so only the interval half
        // is under test here.
        let now = 1_000_000;
        assert!(!refresh_relogin_gate_open(now, now - 184_999, 0));
        assert!(refresh_relogin_gate_open(now, now - 185_000, 0)); // boundary: open at exactly 185 s
        assert!(refresh_relogin_gate_open(now, now - 200_000, 0));
        assert!(refresh_relogin_gate_open(now, 0, 0)); // "long ago" (restored session default)
        // Throttle window still blocks even with the interval met.
        assert!(!refresh_relogin_gate_open(now, 0, now + 1));
        assert!(refresh_relogin_gate_open(now, 0, now)); // throttle lapsed at expiry
        assert_eq!(REFRESH_RELOGIN_MIN_INTERVAL_MS, 185_000);
    }

    #[test]
    fn refresh_relogin_gate_shared_timestamp_semantics() {
        // v2.0.5: autorefresh and manual Refresh both funnel into
        // spawn_discovery's worker, which reads the one process-wide
        // timestamp — so both paths observe the same gate. This
        // single test owns the static (no other test reads it) to
        // avoid parallel-test interference, and resets it to the
        // 0 default at the end.
        use std::sync::atomic::Ordering::Relaxed;
        // Restored old session: never stamped (static default 0 =
        // "long ago") → first refresh re-logs in promptly.
        REFRESH_RELOGIN_LAST_AUTH_MS.store(0, Relaxed);
        assert!(refresh_relogin_gate_open(
            10_000_000,
            REFRESH_RELOGIN_LAST_AUTH_MS.load(Relaxed),
            0
        ));
        // Fresh sign-in stamps the gate (spawn_login records its
        // auth attempt) → the next immediate refresh, manual or
        // auto, does not re-login.
        let t0 = 10_000_000;
        REFRESH_RELOGIN_LAST_AUTH_MS.store(t0, Relaxed);
        let manual_now = t0 + 100_000;
        let auto_now = t0 + 100_000;
        assert!(!refresh_relogin_gate_open(
            manual_now,
            REFRESH_RELOGIN_LAST_AUTH_MS.load(Relaxed),
            0
        ));
        assert!(!refresh_relogin_gate_open(
            auto_now,
            REFRESH_RELOGIN_LAST_AUTH_MS.load(Relaxed),
            0
        ));
        // ~6th cycle later (185 s+): both paths open.
        let later = t0 + 200_000;
        assert!(refresh_relogin_gate_open(
            later,
            REFRESH_RELOGIN_LAST_AUTH_MS.load(Relaxed),
            0
        ));
        assert!(refresh_relogin_gate_open(
            later,
            REFRESH_RELOGIN_LAST_AUTH_MS.load(Relaxed),
            0
        ));
        REFRESH_RELOGIN_LAST_AUTH_MS.store(0, Relaxed);
    }

    #[test]
    fn dashboard_header_has_no_manual_refresh() {
        // v2.0.5: no manual Tuya-discovery trigger may remain —
        // the header must render neither a Refresh button nor an
        // Autorefresh toggle. Unpaired app, so the thermostat
        // section's own Refresh (a local HAP fetch, kept) is not
        // on screen either; any "↻ Refresh" text would be the
        // removed header button.
        let mut app = gate_app();
        app.cfg.thermostat = None;
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let size = egui::vec2(1330.0, 1200.0);
        let texts = render_header(&mut app, &ctx, size, 2);
        let drawn: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        assert!(
            !drawn.contains(&"↻ Refresh"),
            "header must not render a manual Refresh button: {drawn:?}"
        );
        assert!(
            !drawn.contains(&"Autorefresh"),
            "header must not render an Autorefresh toggle: {drawn:?}"
        );
        // The header itself still renders (brand lives on, not
        // removed with the button).
        assert!(drawn.contains(&"Homestead"), "header brand missing: {drawn:?}");
    }

    #[test]
    fn autorefresh_timer_runs_without_toggle() {
        // v2.0.5: the timer is the only Tuya refresh path and it
        // is always on — no toggle, no flag consulted. With a
        // session, a stale last_fetch, and the dashboard up, the
        // driver spawns discovery (disc_busy set synchronously;
        // the spawned worker's result is never polled here, as in
        // the thermostat fetch test). The driver lives in `ui`
        // (which needs an eframe::Frame), so the test calls the
        // extracted `maybe_autorefresh` directly.
        let mut app = gate_app();
        app.session = Some(test_session());
        app.last_fetch =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(3600));
        app.disc_backoff_until = None;
        app.disc_busy = false;
        app.editing = false;
        app.dragging_slider = false;
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        app.maybe_autorefresh(&ctx);
        assert!(app.disc_busy, "timer driver must spawn discovery");
    }

    #[test]
    fn dynamic_interval_steps() {
        // v2.0.6: throttle doubles from the CURRENT interval (floor
        // 60 s on the first hit, cap 5 min); success halves back
        // toward the 30 s floor, never below it.
        assert_eq!(disc_interval_after_throttle(30), 60);
        assert_eq!(disc_interval_after_throttle(60), 120);
        assert_eq!(disc_interval_after_throttle(120), 240);
        assert_eq!(disc_interval_after_throttle(240), 300);
        assert_eq!(disc_interval_after_throttle(300), 300);
        assert_eq!(disc_interval_after_success(300), 150);
        assert_eq!(disc_interval_after_success(150), 75);
        assert_eq!(disc_interval_after_success(75), 37);
        assert_eq!(disc_interval_after_success(37), 30);
        assert_eq!(disc_interval_after_success(30), 30);
        assert_eq!(auto_interval_text(30), "Auto 30s");
        assert_eq!(auto_interval_text(60), "Auto 1 min");
        assert_eq!(auto_interval_text(120), "Auto 2 min");
        assert_eq!(auto_interval_text(300), "Auto 5 min");
        assert_eq!(auto_interval_text(150), "Auto 150s");
    }

    #[test]
    fn dynamic_interval_drives_timer_not_legacy_prefs() {
        // v2.0.6: the timer reads disc_interval_secs, never the
        // legacy prefs.interval — even an old stored 120 s value
        // must not slow it. Stale last_fetch, interval 300 s: no
        // spawn; interval 30 s: spawn.
        let mut app = gate_app();
        app.session = Some(test_session());
        app.cfg.prefs.interval = 120;
        app.last_fetch =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(100));
        app.disc_backoff_until = None;
        app.disc_busy = false;
        app.editing = false;
        app.dragging_slider = false;
        app.disc_interval_secs = 300;
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        app.maybe_autorefresh(&ctx);
        assert!(!app.disc_busy, "300 s dynamic interval must not fire at 100 s");
        app.disc_interval_secs = 30;
        app.maybe_autorefresh(&ctx);
        assert!(app.disc_busy, "30 s dynamic interval must fire at 100 s");
    }

    #[test]
    fn disc_ok_steps_dynamic_interval_down() {
        // v2.0.6: a successful discovery halves the interval toward
        // the floor (and clears the streak/backoff as before).
        let mut app = test_app(vec![]);
        app.disc_interval_secs = 300;
        app.disc_freq_streak = 2;
        app.disc_backoff_until = Some(std::time::Instant::now());
        app.disc_busy = true;
        app.tx
            .send(NetResult::DiscOk {
                session: test_session(),
                devices: vec![],
            })
            .unwrap();
        app.poll_net();
        assert_eq!(app.disc_interval_secs, 150);
        assert_eq!(app.disc_freq_streak, 0);
        assert!(app.disc_backoff_until.is_none());
    }

    #[test]
    fn header_shows_auto_interval_not_options() {
        // v2.0.6: the header carries a read-only "Auto 30s" display
        // and no fixed interval options at all.
        let mut app = gate_app();
        app.editing = false;
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let texts = render_header(&mut app, &ctx, egui::vec2(1330.0, 200.0), 3);
        assert!(
            texts.iter().any(|(t, _)| t.as_str() == "Auto 30s"),
            "header must show the auto interval display"
        );
        assert!(
            !texts.iter().any(|(t, _)| t.as_str() == "15s"),
            "header must not offer the 15 s option"
        );
        assert!(
            !texts.iter().any(|(t, _)| t.as_str() == "2 min"),
            "header must not offer the 2 min option"
        );
    }

    #[test]
    fn throttle_error_classifier() {
        // Throttle spellings: skill rate-limit codes plus generic
        // throttle/429 wording, since net_auth surfaces API
        // messages verbatim.
        for e in [
            "FrequentlyInvoke",
            "TooManyRequests",
            "API error: FrequentlyInvoke",
            "too many requests",
            "Rate limit exceeded",
            "request throttled",
            "HTTP 429",
        ] {
            assert!(is_throttle_error(e), "{e} should classify as throttle");
        }
        // Not throttle: auth-dead, wrong creds, network, device
        // codes, and empty. A dead session is an Auth failure for
        // the existing retry path, not a throttle backoff.
        for e in [
            "Session expired",
            "Login failed — check username, password and region.",
            "Network error: connection timed out",
            "InvalidDeviceId",
            "",
        ] {
            assert!(
                !is_throttle_error(e),
                "{e} should NOT classify as throttle"
            );
        }
    }

    #[test]
    fn disc_err_session_expired_without_saved_login_logs_hint_and_stays_on_dashboard() {
        // v2.0.2: the frozen-dashboard failure must name its gate
        // in the diagnostics log, and must never pop a sign-in form
        // unprompted — the dashboard stays up for the next cycle.
        let mut app = test_app(vec![]);
        assert!(app.cfg.creds.is_none());
        app.disc_busy = true;
        app.tx
            .send(NetResult::DiscErr("Session expired".to_string()))
            .unwrap();
        app.poll_net();
        assert!(!app.disc_busy);
        assert!(matches!(app.view, View::Dashboard));
        assert!(crate::hap::tstat_log_text().contains("no saved login"));
    }

    #[test]
    fn rate_limit_backoff_stays_bounded() {
        // interval * 2^min(streak,3), clamped to [60, 300]s: a
        // storm backs off without ever parking refreshes for good.
        let mut app = test_app(vec![]);
        let mut prev = 0;
        for _ in 0..6 {
            let secs = app.note_rate_limited();
            assert!((60..=300).contains(&secs), "backoff {secs}s out of bounds");
            assert!(secs >= prev, "backoff must not shrink with streak");
            prev = secs;
        }
    }

    #[test]
    fn thermostat_section_defaults_first_and_survives_search() {
        // Old configs whose stored order never mentions the
        // thermostat: it slots first, like any new section with no
        // default-order predecessor.
        let mut layout = grouped_layout();
        layout.section_seq = vec!["scenes".into(), "lighting".into()];
        let secs = build_sections(&full_entry_set(), &layout, false, "", true);
        assert_eq!(secs[0].key, "thermostat");
        assert_eq!(secs[0].order_key, "thermostat");
        assert_eq!(secs[0].title, "Thermostat");
        // No devices at all: the thermostat section still renders
        // when paired — and is absent until a thermostat is paired.
        let empty = build_sections(&[], &LayoutCfg::default(), false, "", true);
        assert_eq!(keys(&empty), vec!["thermostat"]);
        let empty_unpaired = build_sections(&[], &LayoutCfg::default(), false, "", false);
        assert!(empty_unpaired.is_empty());
        // A search matching no device drops every device section
        // but never the paired thermostat (it is not a device).
        let nomatch = build_sections(
            &full_entry_set(),
            &LayoutCfg::default(),
            false,
            "zzz-no-such-device",
            true,
        );
        assert_eq!(keys(&nomatch), vec!["thermostat"]);
        let nomatch_unpaired = build_sections(
            &full_entry_set(),
            &LayoutCfg::default(),
            false,
            "zzz-no-such-device",
            false,
        );
        assert!(nomatch_unpaired.is_empty());
    }

    #[test]
    fn thermostat_section_moves_and_persists() {
        // A stored order placing the thermostat mid-list is
        // honoured, moves work both ways, and the key survives a
        // serde round-trip of the whole layout.
        let mut layout = grouped_layout();
        layout.section_seq = vec![
            "lighting".into(),
            "thermostat".into(),
            "cameras".into(),
            "pinned".into(),
            "group:g1".into(),
            "ungrouped".into(),
            "scenes".into(),
        ];
        let secs = build_sections(&full_entry_set(), &layout, false, "", true);
        assert_eq!(secs[1].key, "thermostat");
        let down = moved_section_seq(&layout.section_seq, &secs, "thermostat", 1);
        assert_eq!(down[2], "thermostat");
        let up = moved_section_seq(&down, &secs, "thermostat", -1);
        assert_eq!(up, layout.section_seq);
        let text = serde_json::to_string(&layout).unwrap();
        let back: LayoutCfg = serde_json::from_str(&text).unwrap();
        assert_eq!(back.section_seq, layout.section_seq);
        let secs2 = build_sections(&full_entry_set(), &back, false, "", true);
        assert_eq!(secs2[1].key, "thermostat");
    }

    // ---------------- v1.2.1: camera ids, edit/delete ----------------

    #[test]
    fn camera_ids_generated_unique() {
        let mut seen = HashSet::new();
        for _ in 0..2000 {
            assert!(seen.insert(new_camera_id()), "id collision");
        }
    }

    #[test]
    fn camera_repair_and_targeted_delete() {
        // The reported failure, at the model layer: two cameras
        // sharing ONE timestamp id (as pre-1.2.1 builds could store)
        // and identical display names.
        let mk = |id: &str, host: &str| Camera {
            id: id.into(),
            name: "Parking Lot".into(),
            user: "u".into(),
            password: obfuscate("p"),
            host: host.into(),
            port: 554,
            template: "tapo1".into(),
            path: String::new(),
        };
        let mut cams = vec![mk("1699999999999", "10.0.0.1"), mk("1699999999999", "10.0.0.2"), mk("42", "10.0.0.3")];
        let mut layout = LayoutCfg::default();
        layout.pins.push("cam:1699999999999".into());
        let mut n = 0;
        let mut gen = || {
            n += 1;
            format!("fresh-{}", n)
        };
        assert!(repair_camera_ids(&mut cams, &layout, &mut gen));
        // First keeps the shared id (and the layout entries pinned
        // to it); the duplicate gets a fresh one; unique ids stay.
        assert_eq!(cams[0].id, "1699999999999");
        assert_eq!(cams[1].id, "fresh-1");
        assert_eq!(cams[2].id, "42");
        assert_eq!(layout.pins, vec!["cam:1699999999999".to_string()]);
        // Repair is idempotent.
        let mut gen2 = || "unused".to_string();
        assert!(!repair_camera_ids(&mut cams, &layout, &mut gen2));
        // Edit targeting: resolving the second camera by ITS item id
        // reaches the second record (10.0.0.2), never the first.
        let idx = cams
            .iter()
            .position(|c| format!("cam:{}", c.id) == "cam:fresh-1")
            .unwrap();
        assert_eq!(cams[idx].host, "10.0.0.2");
        // A missing/blank id is repaired too.
        let mut cams2 = vec![mk("", "10.0.0.9")];
        let mut gen3 = || "fresh-x".to_string();
        assert!(repair_camera_ids(&mut cams2, &layout, &mut gen3));
        assert_eq!(cams2[0].id, "fresh-x");

        // Delete: only the targeted camera goes, and its layout
        // entries (including size) are cleaned.
        layout.cam_sizes.insert("cam:fresh-1".into(), "large".into());
        layout.hidden.push("cam:fresh-1".into());
        let removed = remove_camera_record(&mut cams, &mut layout, "cam:fresh-1");
        assert_eq!(removed.unwrap().host, "10.0.0.2");
        assert_eq!(cams.len(), 2);
        assert!(cams.iter().all(|c| c.host != "10.0.0.2"));
        assert!(!layout.cam_sizes.contains_key("cam:fresh-1"));
        assert!(!layout.hidden.contains(&"cam:fresh-1".to_string()));
        // Deleting an unknown id removes nothing and says so (None).
        assert!(remove_camera_record(&mut cams, &mut layout, "cam:nope").is_none());
        assert_eq!(cams.len(), 2);
        // The first camera survived with its pin intact.
        assert_eq!(cams[0].host, "10.0.0.1");
        assert_eq!(layout.pins, vec!["cam:1699999999999".to_string()]);
    }

    // ---------------- v1.2.1: camera sizes ----------------

    #[test]
    fn cam_size_span_and_tile_feed() {
        assert_eq!(CamSize::Normal.span(4), 1);
        assert_eq!(CamSize::Large.span(4), 2);
        assert_eq!(CamSize::Wide.span(4), 4);
        assert_eq!(CamSize::Large.span(1), 1); // clamped to the grid
        assert_eq!(CamSize::Wide.span(1), 1);
        assert_eq!(CamSize::parse(Some(&"large".to_string())), CamSize::Large);
        assert_eq!(CamSize::parse(Some(&"wide".to_string())), CamSize::Wide);
        assert_eq!(CamSize::parse(Some(&"huge".to_string())), CamSize::Normal);
        assert_eq!(CamSize::parse(None), CamSize::Normal);

        // Normal: SD stream at 480×270. Large: same SD source, bigger
        // pump. Wide: the card's selected quality at 960×540.
        let (u, w, h) = tile_feed(&cam("tapo1", ""), CamSize::Normal);
        assert!(u.ends_with("/stream2"));
        assert_eq!((w, h), (480, 270));
        let (u, w, h) = tile_feed(&cam("tapo1", ""), CamSize::Large);
        assert!(u.ends_with("/stream2"));
        assert_eq!((w, h), (960, 540));
        let (u, w, h) = tile_feed(&cam("tapo1", ""), CamSize::Wide);
        assert!(u.ends_with("/stream1"));
        assert_eq!((w, h), (960, 540));
        let (u, _, _) = tile_feed(&cam("tapo2", ""), CamSize::Wide);
        assert!(u.ends_with("/stream2"));
        let (u, w, h) = tile_feed(&cam("custom", "live/ch1"), CamSize::Normal);
        assert!(u.ends_with("/live/ch1"));
        assert_eq!((w, h), (480, 270));
    }

    #[test]
    fn pack_rows_with_spans() {
        let items = vec!["a", "b", "c"];
        // 1-wide, 2-wide, 1-wide in 4 columns: b doesn't fit after
        // a+b = 3? It does: a(1)+b(2)=3, c would make 4 — fits.
        let rows = pack_rows(&items, &[1, 2, 1], 4);
        assert_eq!(rows.len(), 1);
        // A wide (full-row) item after a normal one starts a new row.
        let rows = pack_rows(&items, &[1, 3, 1], 3);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1], vec![("b", 3)]);
        // Two larges in 3 columns: second wraps.
        let rows = pack_rows(&items, &[2, 2, 1], 3);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 1);
        assert_eq!(rows[1].len(), 2);
        // Span clamped to the grid width.
        let rows = pack_rows(&items, &[9, 1, 1], 2);
        assert_eq!(rows[0], vec![("a", 2)]);
    }

    #[test]
    fn drag_promotion_threshold() {
        let press = egui::Pos2::new(100.0, 100.0);
        // Click jitter (a few px) must NOT promote — the card stays
        // in the layout and the click lands on its controls.
        assert!(!drag_should_promote(press, egui::Pos2::new(102.0, 101.0)));
        assert!(!drag_should_promote(press, egui::Pos2::new(100.0, 107.9)));
        // A deliberate drag promotes.
        assert!(drag_should_promote(press, egui::Pos2::new(100.0, 108.0)));
        assert!(drag_should_promote(press, egui::Pos2::new(140.0, 100.0)));
    }

    // ---------------- v1.2.1: startup & tray (pure parts) ----------------

    #[test]
    fn autostart_command_all_modes_and_off() {
        let exe = Path::new("C:\\Users\\Shawn\\AppData\\Local\\Homestead\\Homestead.exe");
        assert_eq!(
            autostart_command(exe, LaunchMode::Normal),
            "\"C:\\Users\\Shawn\\AppData\\Local\\Homestead\\Homestead.exe\""
        );
        assert_eq!(
            autostart_command(exe, LaunchMode::Minimized),
            "\"C:\\Users\\Shawn\\AppData\\Local\\Homestead\\Homestead.exe\" --minimized"
        );
        assert_eq!(
            autostart_command(exe, LaunchMode::Maximized),
            "\"C:\\Users\\Shawn\\AppData\\Local\\Homestead\\Homestead.exe\" --maximized"
        );
        // OFF = no entry at all (None), whatever the mode says.
        let mut prefs = Prefs::default();
        assert_eq!(desired_autostart(&prefs, exe), None);
        prefs.start_with_windows = true;
        prefs.launch_mode = "minimized".into();
        assert_eq!(
            desired_autostart(&prefs, exe).as_deref(),
            Some("\"C:\\Users\\Shawn\\AppData\\Local\\Homestead\\Homestead.exe\" --minimized")
        );
        prefs.launch_mode = "maximized".into();
        assert!(desired_autostart(&prefs, exe).unwrap().ends_with("--maximized"));
        prefs.launch_mode = "normal".into();
        assert!(!desired_autostart(&prefs, exe).unwrap().contains("--"));
        // Round-trips: mode survives command construction + parsing.
        for m in [LaunchMode::Normal, LaunchMode::Minimized, LaunchMode::Maximized] {
            assert_eq!(launch_mode_from_command(&autostart_command(exe, m)), m);
            assert_eq!(LaunchMode::parse(m.as_str()), m);
        }
    }

    #[test]
    fn launch_flag_parsing() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        assert_eq!(launch_mode_from_args(&args(&["app.exe"])), LaunchMode::Normal);
        assert_eq!(
            launch_mode_from_args(&args(&["app.exe", "--minimized"])),
            LaunchMode::Minimized
        );
        assert_eq!(
            launch_mode_from_args(&args(&["app.exe", "--maximized"])),
            LaunchMode::Maximized
        );
        assert_eq!(
            launch_mode_from_args(&args(&["app.exe", "--maximized", "--minimized"])),
            LaunchMode::Minimized // minimized wins ties
        );
        assert_eq!(
            launch_mode_from_args(&args(&["app.exe", "--something-else"])),
            LaunchMode::Normal
        );
    }

    // ---------------- v1.2.4: effective launch mode ----------------

    #[test]
    fn launch_mode_resolution() {
        use LaunchMode::*;
        // An explicit argv flag wins over any saved mode.
        assert_eq!(resolve_launch_mode(Minimized, Maximized), Minimized);
        assert_eq!(resolve_launch_mode(Minimized, Normal), Minimized);
        assert_eq!(resolve_launch_mode(Maximized, Minimized), Maximized);
        assert_eq!(resolve_launch_mode(Maximized, Normal), Maximized);
        // No flag → the saved Settings mode decides (the v1.2.4
        // fix: previously the saved mode was never consulted, so
        // arg-less launches always opened Normal).
        assert_eq!(resolve_launch_mode(Normal, Minimized), Minimized);
        assert_eq!(resolve_launch_mode(Normal, Maximized), Maximized);
        assert_eq!(resolve_launch_mode(Normal, Normal), Normal);
    }

    /// The viewport commands one headless frame of the launch
    /// hold produces (drives the REAL App::enforce_launch_hold).
    fn hold_frame_cmds(app: &App, ctx: &egui::Context) -> Vec<egui::ViewportCommand> {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ctx| {
            app.enforce_launch_hold(ctx)
        });
        let cmds = out
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.commands.clone())
            .unwrap_or_default();
        // egui 0.37 debug-asserts that a dropped TexturesDelta is
        // empty; headless tests never apply it, so clear it.
        out.textures_delta.clear();
        cmds
    }

    /// The reboot/login regression (v1.2.4): Windows relaunches
    /// the app — via the Run entry or its "restartable apps"
    /// sign-in feature — possibly with NO command line at all,
    /// while Settings say "Minimized (to tray)". The launch must
    /// resolve to Minimized, the window must be commanded
    /// invisible from the start AND on every early frame, and
    /// only a real Show action may make it visible.
    #[test]
    fn argless_minimized_launch_stays_hidden_until_show() {
        // Arg-less argv (the restartable-apps case) + saved mode.
        let argv = launch_mode_from_args(&["app.exe".to_string()]);
        assert_eq!(argv, LaunchMode::Normal); // no flag arrived
        let effective = resolve_launch_mode(argv, LaunchMode::parse("minimized"));
        assert_eq!(effective, LaunchMode::Minimized);

        // App::new's outcome for that resolution: hidden state and
        // the launch hold both follow the EFFECTIVE mode.
        let mut app = test_app(vec![]);
        app.launch_mode = effective;
        app.window_hidden = app.launch_mode == LaunchMode::Minimized;
        app.launch_hidden_pending = app.window_hidden;
        assert!(app.window_hidden);
        assert!(app.launch_hidden_pending);

        // Early frames: invisibility is re-asserted every frame —
        // the window is commanded invisible and stays so.
        let ctx = egui::Context::default();
        // Warm-up frame (egui 0.37): a fresh context announces its
        // theme once via ViewportCommand::SetTheme; that is egui's
        // own first-frame command, not the launch hold's, so it is
        // spent here and the loop below asserts the hold exactly.
        let _ = hold_frame_cmds(&app, &ctx);
        for _ in 0..3 {
            assert_eq!(
                hold_frame_cmds(&app, &ctx),
                vec![egui::ViewportCommand::Visible(false)]
            );
            assert!(app.window_hidden && app.launch_hidden_pending);
        }

        // A real Show action (tray Open / single-instance summon
        // both land in show_from_tray): the window is commanded
        // visible and the hold is released.
        let out = ctx.run_ui(egui::RawInput::default(), |ctx| {
            app.show_from_tray(ctx)
        });
        let cmds = out
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.commands.clone())
            .unwrap_or_default();
        assert!(cmds.contains(&egui::ViewportCommand::Visible(true)));
        assert!(!app.window_hidden);
        assert!(!app.launch_hidden_pending);
        // See hold_frame_cmds: 0.37 drops debug-assert on
        // unconsumed texture deltas.
        let mut out = out;
        out.textures_delta.clear();

        // Later frames assert nothing: visible is left alone.
        assert!(hold_frame_cmds(&app, &ctx).is_empty());
    }

    // ---------------- v1.3.0: placement memory removal gate ----------------

    #[test]
    fn no_startup_path_reads_saved_window_geometry() {
        // Structural gate for the v1.2.5 removal: the placement
        // feature (capture + restore) is gone from the crate, so
        // no launch path can read saved window geometry. Needles
        // are concatenated so this test doesn't match itself.
        // Scan the production code only (everything before the
        // test module — the legacy-config fixture below names the
        // key on purpose).
        let full = include_str!("main.rs");
        let src = full.split("#[cfg(test)]").next().unwrap_or(full);
        for needle in [
            concat!("window_", "geom"),
            concat!("Window", "Geom"),
            concat!("restore_", "placement"),
            concat!("track_window_", "geometry"),
            concat!("virtual_desktop_", "rect"),
        ] {
            assert!(
                !src.contains(needle),
                "placement-memory remnant {needle:?} is still in main.rs"
            );
        }
    }

    // ---------------- v1.3.0: snap to monitor (pure parts) ----------------

    fn mon(x: i32, y: i32, w: i32, h: i32, scale: f32, primary: bool) -> MonitorInfo {
        MonitorInfo {
            name: "M".into(),
            device: "D".into(),
            handle: 0,
            rect_px: [x, y, w, h],
            scale,
            primary,
        }
    }

    #[test]
    fn snap_conversion_uses_each_monitors_own_scale() {
        // 2560×1440 physical at 150% -> 1706.7×960 points.
        let m = mon(0, 0, 2560, 1440, 1.5, true);
        let r = monitor_rect_points(&m);
        assert!((r[2] - 1706.67).abs() < 0.1 && (r[3] - 960.0).abs() < 0.1);
        // At 200% the same panel is 1280×720 points; at 100% it
        // is the physical size. Negative origins divide too.
        let m2 = mon(-1920, 0, 1920, 1080, 2.0, false);
        assert_eq!(monitor_rect_points(&m2), [-960.0, 0.0, 960.0, 540.0]);
        let m3 = mon(0, 0, 1920, 1080, 1.0, true);
        assert_eq!(monitor_rect_points(&m3), [0.0, 0.0, 1920.0, 1080.0]);
        assert_eq!(monitor_orientation(&m3), "Landscape");
        assert_eq!(monitor_orientation(&mon(0, 0, 1080, 1920, 1.0, false)), "Portrait");
    }

    #[test]
    fn snap_point_px_is_interior_and_clamped() {
        // All values PHYSICAL pixels (v1.3.1): the point is what
        // SetWindowPos receives — no scale conversion can move
        // it onto another monitor.
        let win = [1120, 780];
        // Roomy monitor at 100%: the plain 60 px inset.
        assert_eq!(snap_target_point_px(&mon(0, 0, 1920, 1080, 1.0, true), win), [60, 60]);
        // Negative-origin monitor left of the primary.
        assert_eq!(
            snap_target_point_px(&mon(-1920, 0, 1920, 1080, 1.0, false), win),
            [-1860, 60]
        );
        // Monitor above the primary (negative y).
        assert_eq!(
            snap_target_point_px(&mon(0, -1080, 1920, 1080, 1.0, false), win),
            [60, -1020]
        );
        // Window wider than the monitor: the point still lands
        // strictly inside, with margin.
        let small = mon(0, 0, 800, 600, 1.0, false);
        let p = snap_target_point_px(&small, win);
        assert!(p[0] > 0 && p[0] < 800 && p[1] > 0 && p[1] < 600, "point {p:?} interior");
        // Narrow monitor: the 60 px inset already lands inside.
        let narrow = mon(100, 0, 100, 900, 1.0, false);
        let p = snap_target_point_px(&narrow, win);
        assert_eq!(p[0], 160, "inset x");
        // Very narrow monitor: the inset overshoots the far edge
        // and clamps back inside it (width - margin - 1 = 41).
        let vnarrow = mon(0, 0, 50, 900, 1.0, false);
        let p = snap_target_point_px(&vnarrow, win);
        assert_eq!(p[0], 41, "clamped x");
        // Degenerate tiny monitor: centre point, still interior.
        let tiny = mon(50, 50, 10, 10, 1.0, false);
        assert_eq!(snap_target_point_px(&tiny, win), [55, 55]);
    }

    #[test]
    fn snap_point_px_users_desk_mixed_dpi() {
        // THE v1.3.0 field failure, as fixtures: a 2560x1440
        // landscape primary with a 2160x3840 (4K) PORTRAIT
        // target to its right, at a HIGHER scale factor than
        // the source. v1.3.0 converted the target point to
        // points with the TARGET's scale while winit applied
        // it with the SOURCE's — the window never left the
        // landscape panel. In physical pixels there is nothing
        // to convert: at any target scale the point is strictly
        // inside the portrait rect and NOT on the source.
        let source = mon(0, 0, 2560, 1440, 1.25, true);
        for scale in [1.5, 1.75, 2.0] {
            let target = mon(2560, 0, 2160, 3840, scale, false);
            // The restored window's physical size on the source
            // (1120x780 pt at the source's 125% = 1400x975 px).
            let p = snap_target_point_px(&target, [1400, 975]);
            assert!(
                p[0] >= 2560 && p[0] < 2560 + 2160 && p[1] >= 0 && p[1] < 3840,
                "scale {scale}: point {p:?} inside the portrait target"
            );
            assert!(
                !(p[0] >= source.rect_px[0]
                    && p[0] < source.rect_px[0] + source.rect_px[2]
                    && p[1] >= source.rect_px[1]
                    && p[1] < source.rect_px[1] + source.rect_px[3]),
                "scale {scale}: point {p:?} must NOT land on the source monitor"
            );
            // The inset is 60 pt worth of pixels at the TARGET's
            // scale (the window fits easily, so no clamping).
            assert_eq!(p, [2560 + (60.0 * scale) as i32, (60.0 * scale) as i32]);
        }
        // A window LARGER than the portrait panel in one axis
        // still lands strictly inside it.
        let target = mon(2560, 0, 2160, 3840, 2.0, false);
        let p = snap_target_point_px(&target, [2600, 900]);
        assert!(p[0] > 2560 && p[0] < 2560 + 2160 && p[1] > 0 && p[1] < 3840, "oversize window point {p:?} interior");
    }

    #[test]
    fn snap_state_machine_sequences() {
        use SnapAction::*;
        use SnapPhase::*;
        let calm = SnapFacts { fullscreen: false, zoomed: false, on_target: false, expired: false };
        // Plain window, already restored: straight to Move.
        assert_eq!(snap_next(LeaveStates, calm, 0), (Move, Wait));
        // Maximized window: restore first, move next frame.
        assert_eq!(
            snap_next(LeaveStates, SnapFacts { zoomed: true, ..calm }, 0),
            (Move, Restore)
        );
        // Fullscreen is released first and the phase waits for
        // egui to confirm it (fullscreen wins over zoomed).
        assert_eq!(
            snap_next(LeaveStates, SnapFacts { fullscreen: true, zoomed: true, ..calm }, 0),
            (LeaveStates, SendFullscreenOff)
        );
        // The move itself, then verification.
        assert_eq!(snap_next(Move, calm, 0), (Verify, MoveWindow));
        // Verified arrival: maximize and finish — even if the
        // deadline passed on the same frame.
        assert_eq!(
            snap_next(Verify, SnapFacts { on_target: true, expired: true, ..calm }, 1),
            (Verify, MaximizeAndFinish)
        );
        // First miss (one move spent): retry the restore+move.
        assert_eq!(snap_next(Verify, calm, 1), (Move, Restore));
        // Second miss (both moves spent): fail safe, no
        // maximize on the wrong monitor.
        assert_eq!(snap_next(Verify, calm, 2), (Verify, Fail));
        // Timeout anywhere else fails the job (window stays
        // wherever it verifiably is).
        assert_eq!(
            snap_next(Move, SnapFacts { expired: true, ..calm }, 0),
            (Move, Fail)
        );
        assert_eq!(
            snap_next(LeaveStates, SnapFacts { expired: true, ..calm }, 0),
            (LeaveStates, Fail)
        );
    }

    #[test]
    fn startup_check_decision_table() {
        use StartupCheckAction::*;
        // Settled, maximized, off target, re-arm unused: arm.
        let calm = StartupCheckFacts {
            snap_active: false,
            expired: false,
            maximized: true,
            on_target: false,
            settle_passed: true,
            moved: false,
        };
        assert_eq!(startup_check_next(calm, false, true, false), (ArmSnap, true));
        // A snap in flight: observe only, even on the deadline.
        assert_eq!(
            startup_check_next(
                StartupCheckFacts { snap_active: true, expired: true, ..calm },
                false,
                true,
                false
            ),
            (Wait, true)
        );
        // Arrival beats everything: resolves even on the
        // deadline frame, even restored (straddling target).
        assert_eq!(
            startup_check_next(
                StartupCheckFacts { on_target: true, expired: true, ..calm },
                true,
                true,
                false
            ),
            (Resolve, true)
        );
        assert_eq!(
            startup_check_next(
                StartupCheckFacts { on_target: true, maximized: false, ..calm },
                true,
                true,
                false
            ),
            (Resolve, true)
        );
        // Still settling after a snap: wait, don't spend the arm.
        assert_eq!(
            startup_check_next(
                StartupCheckFacts { settle_passed: false, ..calm },
                false,
                true,
                false
            ),
            (Wait, true)
        );
        // User takeover beats expiry: maximized, then restored
        // with no snap in flight — stop, never latch, never arm.
        assert_eq!(
            startup_check_next(
                StartupCheckFacts { maximized: false, expired: true, ..calm },
                false,
                true,
                false
            ),
            (StopUserTakeover, true)
        );
        // A snap-failure restored state is NOT a takeover: the
        // arm is still spent on it.
        assert_eq!(
            startup_check_next(
                StartupCheckFacts { maximized: false, ..calm },
                false,
                true,
                true
            ),
            (ArmSnap, true)
        );
        // ...unless the window actually moved since the failure:
        // a drag after a failed snap is still a takeover.
        assert_eq!(
            startup_check_next(
                StartupCheckFacts { maximized: false, moved: true, ..calm },
                false,
                true,
                true
            ),
            (StopUserTakeover, true)
        );
        // Never observed maximized (window still starting):
        // a restored state is not a takeover.
        assert_eq!(
            startup_check_next(
                StartupCheckFacts { maximized: false, ..calm },
                false,
                false,
                false
            ),
            (ArmSnap, false)
        );
        // Window expired, settled, re-arm already spent: latch
        // as unresolved (the caller records the failed landing).
        assert_eq!(
            startup_check_next(
                StartupCheckFacts { expired: true, ..calm },
                true,
                true,
                false
            ),
            (ExpireUnresolved, true)
        );
        // Expiry beats the arm: an unused re-arm is never spent
        // past the deadline.
        assert_eq!(
            startup_check_next(
                StartupCheckFacts { expired: true, ..calm },
                false,
                true,
                false
            ),
            (ExpireUnresolved, true)
        );
        // Re-arm spent and still off target: wait out the window.
        assert_eq!(startup_check_next(calm, true, true, false), (Wait, true));
        // A maximized observation is remembered for later frames.
        assert_eq!(startup_check_next(calm, false, false, false), (ArmSnap, true));
    }

    #[test]
    fn snap_current_monitor_detection() {
        let mons = vec![
            mon(0, 0, 1920, 1080, 1.0, true),
            mon(1920, 0, 1080, 1920, 1.0, false),
            mon(-1600, 0, 1600, 900, 1.25, false), // 1280×720 points
        ];
        assert_eq!(monitor_index_at(&mons, [100.0, 100.0]), Some(0));
        assert_eq!(monitor_index_at(&mons, [2400.0, 900.0]), Some(1));
        assert_eq!(monitor_index_at(&mons, [-500.0, 300.0]), Some(2));
        assert_eq!(monitor_index_at(&mons, [5000.0, 100.0]), None);
        // The shared edge belongs to the right-hand monitor.
        assert_eq!(monitor_index_at(&mons, [1920.0, 10.0]), Some(1));
    }

    #[test]
    fn theme_default_is_black_stored_preference_wins() {
        // v1.3.0: fresh state (nothing stored) -> Black.
        assert_eq!(Prefs::default().theme, "black");
        assert_eq!(Config::default().prefs.theme, "black");
        // A stored Dark or Light choice is respected untouched.
        for stored in ["dark", "light", "black"] {
            let cfg: Config = serde_json::from_str(&format!(
                "{{\"prefs\":{{\"theme\":\"{stored}\",\"accent\":\"#6366f1\",\
                 \"density\":\"comfortable\",\"autorefresh\":true,\"interval\":30,\
                 \"proxy_preset\":\"auto\",\"proxy_custom\":\"\"}}}}"
            ))
            .unwrap();
            assert_eq!(cfg.prefs.theme, stored, "stored theme must win");
        }
    }

    #[test]
    fn legacy_config_with_placement_key_still_loads() {
        // A v1.2.5-era config file carries the now-removed key in
        // prefs: it must still load (serde ignores unknown keys),
        // and the next save simply drops it.
        let legacy: Config = serde_json::from_str(
            "{\"prefs\":{\"theme\":\"dark\",\"accent\":\"#6366f1\",\
             \"density\":\"comfortable\",\"autorefresh\":true,\"interval\":30,\
             \"proxy_preset\":\"auto\",\"proxy_custom\":\"\",\
             \"window_geom\":{\"pos\":[2100.0,100.0],\"size\":[900.0,1400.0],\"maximized\":true}}}",
        )
        .unwrap();
        assert_eq!(legacy.prefs.theme, "dark");
        let text = serde_json::to_string(&legacy).unwrap();
        assert!(
            !text.contains(concat!("window_", "geom")),
            "removed key must not be written back: {text}"
        );
    }

    #[test]
    fn reg_named_value_parsing() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\r\n    Homestead    REG_SZ    \"C:\\Users\\Shawn\\app.exe\" --minimized\r\n    OtherApp    REG_SZ    C:\\other.exe\r\n\r\n";
        assert_eq!(
            parse_reg_named_value(out, "Homestead").as_deref(),
            Some("\"C:\\Users\\Shawn\\app.exe\" --minimized")
        );
        assert_eq!(
            parse_reg_named_value(out, "OtherApp").as_deref(),
            Some("C:\\other.exe")
        );
        assert_eq!(parse_reg_named_value(out, "Missing"), None);
        assert_eq!(parse_reg_named_value("ERROR: not found", "Homestead"), None);
        assert_eq!(parse_reg_named_value("", "Homestead"), None);
    }

    // ---------------- v1.4.0: macOS port ----------------

    #[test]
    fn launch_agent_plist_roundtrip() {
        // The stored autostart command decomposes into the
        // LaunchAgent plist and reads back identically, for
        // every launch mode — the registry path stores the
        // string itself, so the plist must not lose information.
        for mode in [
            LaunchMode::Normal,
            LaunchMode::Minimized,
            LaunchMode::Maximized,
        ] {
            let cmd = autostart_command(
                Path::new(
                    "/Applications/Homestead.app/Contents/MacOS/Homestead",
                ),
                mode,
            );
            let plist = launch_agent_plist(&cmd).expect("plist builds");
            assert!(plist.contains(LAUNCH_AGENT_LABEL));
            assert!(plist.contains("<key>RunAtLoad</key>"));
            // The plist carries the app command (exe path + flags).
            assert!(plist.contains("/Applications/Homestead.app/Contents/MacOS/Homestead"));
            // …and unescaped on read-back: an exact round-trip.
            assert_eq!(launch_agent_command(&plist).as_deref(), Some(cmd.as_str()));
        }
        // A foreign plist is not this app's agent.
        assert!(launch_agent_command("<plist><dict></dict></plist>").is_none());
        // Garbage commands don't build plists.
        assert!(launch_agent_plist("").is_none());
        assert!(parse_autostart_command("").is_none());
        // An unquoted stored command (no spaces in the path)
        // parses too.
        assert_eq!(
            parse_autostart_command("/opt/app/Homestead --maximized"),
            Some((
                "/opt/app/Homestead".to_string(),
                LaunchMode::Maximized
            ))
        );
    }

    #[cfg(unix)]
    #[test]
    fn instance_lock_file_excludes_second_holder() {
        // The macOS single-instance guard's core: while one
        // holder has the flock, a second attempt gets None; once
        // the holder drops, the lock is free again (the OS
        // releases it with the fd — no stale locks possible).
        let path = std::env::temp_dir().join(format!(
            "sltd-instance-lock-test-{}.lock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let first = try_lock_instance_file(&path)
            .expect("lock file creates")
            .expect("first holder gets the lock");
        assert!(try_lock_instance_file(&path)
            .expect("second open works")
            .is_none());
        drop(first);
        assert!(try_lock_instance_file(&path)
            .expect("relock works")
            .is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ffmpeg_bundled_names_per_arch() {
        // The names the macOS CI must place in the .app's
        // Resources — the app looks up exactly these.
        assert_eq!(ffmpeg_bundled_name("aarch64"), Some("ffmpeg-arm64"));
        assert_eq!(ffmpeg_bundled_name("x86_64"), Some("ffmpeg-x86_64"));
        assert_eq!(ffmpeg_bundled_name("arm"), None);
    }

    #[test]
    fn snap_monitor_button_is_windows_only() {
        // Structural gate (v1.4.0): the 🖥 Monitor menu button is
        // Windows-only UI (the snap mechanism is direct Win32) —
        // it must stay behind its cfg gate so macOS never renders
        // a button that cannot work.
        let full = include_str!("main.rs");
        let src = full.split("#[cfg(test)]").next().unwrap_or(full);
        let needle = concat!("menu_button(\"🖥 ", "Monitor\"");
        let at = src.find(needle).expect("monitor menu button exists");
        let before = &src[at.saturating_sub(600)..at];
        assert!(
            before.contains("#[cfg(target_os = \"windows\")]"),
            "the Monitor menu button lost its Windows-only cfg gate"
        );
    }

    #[test]
    fn close_action_logic() {
        assert_eq!(close_action(false, false), CloseAction::Quit);
        assert_eq!(close_action(true, false), CloseAction::HideToTray);
        // A real Quit (tray menu / installer) always wins over
        // close-to-tray.
        assert_eq!(close_action(true, true), CloseAction::Quit);
        assert_eq!(close_action(false, true), CloseAction::Quit);
    }

    #[test]
    fn tray_queue_is_fifo_and_drains_empty() {
        // The queue is process-global; keep this test's traffic
        // self-contained (push two, drain two, drain empty).
        push_tray_action(TrayAction::Show);
        push_tray_action(TrayAction::Quit);
        let got = drain_tray_actions();
        assert_eq!(got, vec![TrayAction::Show, TrayAction::Quit]);
        assert!(drain_tray_actions().is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn tray_menu_mapping_is_unconditional() {
        // Menu ids map by identity alone — no hidden/visible state
        // anywhere in the mapping (Bug C: actions must be
        // unconditional).
        let open = tray_icon::menu::MenuId::new("test-open");
        let quit = tray_icon::menu::MenuId::new("test-quit");
        let other = tray_icon::menu::MenuId::new("test-other");
        assert_eq!(tray_action_for_menu(&open, &open, &quit), Some(TrayAction::Show));
        assert_eq!(tray_action_for_menu(&quit, &open, &quit), Some(TrayAction::Quit));
        assert_eq!(tray_action_for_menu(&other, &open, &quit), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn tray_icon_mapping_matches_v122() {
        // Left release shows; left double-click shows; press-only,
        // right and middle clicks do not.
        assert_eq!(tray_action_for_icon_parts(false, true, true), Some(TrayAction::Show));
        assert_eq!(tray_action_for_icon_parts(true, true, false), Some(TrayAction::Show));
        assert_eq!(tray_action_for_icon_parts(false, true, false), None);
        assert_eq!(tray_action_for_icon_parts(false, false, true), None);
        assert_eq!(tray_action_for_icon_parts(true, false, false), None);
    }

    // ---- Headless render harness (v1.2.3, Bug B gate) ----
    // Drives the app's REAL dashboard rendering (view_dashboard →
    // section_view → group_controls / device_card) on a headless
    // egui::Context and reads back the text egui actually
    // tessellated. Name truncation and header layout are therefore
    // measured on drawn output — not on helper math (the v1.2.2
    // budget helper passed its unit test while the field bug
    // persisted, because the loss was never in the helper).

    fn test_app(devices: Vec<Device>) -> App {
        let (tx, rx) = channel::<NetResult>();
        let (tstat_tx, tstat_rx) = channel::<TstatResult>();
        // No bridge thread in tests: poll_bridge would just find
        // the channel disconnected. The sender is dropped here.
        let (_bridge_tx, bridge_rx) = channel::<bridge::BridgeJob>();
        App {
            cfg: Config::default(),
            view: View::Dashboard,
            session: None,
            devices,
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
            startup_check: StartupCheckState::default(),
            slider_live: HashMap::new(),
            dragging_slider: false,
            alias_buf: HashMap::new(),
            group_buf: HashMap::new(),
            new_group: None,
            toasts: Vec::new(),
            applied_style: None,
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
            drag_pending: None,
            tray: None,
            window_hidden: false,
            quitting: false,
            arg_launch_mode: LaunchMode::Normal,
            launch_mode: LaunchMode::Normal,
            launch_hidden_pending: false,
            snap_job: None,
            startup_monitor_checked: false,
            last_seen_monitor: 0,
            disc_backoff_until: None,
            disc_freq_streak: 0,
            disc_interval_secs: DISC_INTERVAL_FLOOR_SECS,
            pumps: HashMap::new(),
            pump_force: HashSet::new(),
            cam_gen: HashMap::new(),
            held_textures: HashMap::new(),
            prev_focused: true,
            paused_cams: HashSet::new(),
            player_for: None,
            ffmpeg_cache: None,
            tstat_setup_code: String::new(),
            tstat_busy: false,
            tstat_set_busy: false,
            tstat_pending_set: None,
            tstat_revert_pending: false,
            tstat_fetch_busy: false,
            tstat_state: None,
            tstat_error: None,
            tstat_discovered: Vec::new(),
            tstat_pending: None,
            tstat_tx,
            tstat_rx,
            tstat_last_fetch: None,
            settings_jump_thermostat: false,
            bridge_rx,
            bridge_pending: None,
        }
    }


    /// The text a galley will actually DRAW: its rows' glyphs,
    /// joined. (Galley::text() returns the untruncated SOURCE
    /// string — useless for detecting elision; the rows carry the
    /// shortened text, ellipsis included, when a label truncated.)
    fn galley_drawn_text(g: &egui::epaint::Galley) -> String {
        g.rows.iter().map(|r| r.text()).collect::<Vec<_>>().join("\n")
    }

    /// Every text string egui actually drew in one output frame,
    /// with the position of its first glyph.
    fn drawn_texts(out: &egui::FullOutput) -> Vec<(String, egui::Pos2)> {
        fn walk(shape: &egui::epaint::Shape, acc: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::epaint::Shape::Text(t) => {
                    acc.push((galley_drawn_text(&t.galley), t.pos));
                }
                egui::epaint::Shape::Vec(v) => {
                    for s in v {
                        walk(s, acc);
                    }
                }
                _ => {}
            }
        }
        let mut acc = Vec::new();
        for cs in &out.shapes {
            walk(&cs.shape, &mut acc);
        }
        acc
    }

    /// Render the dashboard on a headless context at the given
    /// viewport size for `frames` frames; returns the drawn texts
    /// of the LAST frame (layout caches settled by then).
    fn render_dashboard(
        app: &mut App,
        ctx: &egui::Context,
        size: egui::Vec2,
        frames: usize,
    ) -> Vec<(String, egui::Pos2)> {
        let mut texts = Vec::new();
        for i in 0..frames {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(10.0 + i as f64 * 0.05),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| app.view_dashboard(ui));
            });
            texts = drawn_texts(&out);
            out.textures_delta.clear();
        }
        texts
    }

    /// Same harness for the header (which owns the edit bar):
    /// render `frames` frames of `header()` and return the last
    /// frame's drawn texts.
    fn render_header(
        app: &mut App,
        ctx: &egui::Context,
        size: egui::Vec2,
        frames: usize,
    ) -> Vec<(String, egui::Pos2)> {
        let mut texts = Vec::new();
        for i in 0..frames {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(10.0 + i as f64 * 0.05),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| app.header(ui));
            });
            texts = drawn_texts(&out);
            out.textures_delta.clear();
        }
        texts
    }

    fn name_fixture() -> Vec<Device> {
        let bulb_data = json!({ "state": true, "brightness": 800, "color_temp": 29000 });
        vec![
            dev("d1", "Desk Light", bulb_data.clone()),
            dev("d2", "Kitchen Light 1", bulb_data.clone()),
            dev("d3", "Kitchen Light 2", bulb_data.clone()),
            dev("d4", "Love's Bed Light", bulb_data.clone()),
            dev("d5", "Shawn's Bed Light", bulb_data.clone()),
            dev(
                "d6",
                "Desk RGB strip",
                json!({ "state": true, "color_mode": "colour", "hue": 10, "saturation": 0.5 }),
            ),
            parse_device(&json!({
                "id": "s1", "name": "Turn off everything", "dev_type": "scene",
                "data": { "state": false }
            })),
        ]
    }

    /// Render `frames` frames and return the LAST frame's full
    /// output (for gates that need shapes, not just texts).
    fn render_dashboard_full(
        app: &mut App,
        ctx: &egui::Context,
        size: egui::Vec2,
        frames: usize,
    ) -> egui::FullOutput {
        let mut out = None;
        for i in 0..frames {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(30.0 + i as f64 * 0.05),
                ..Default::default()
            };
            let mut frame_out = ctx.run_ui(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| app.view_dashboard(ui));
            });
            frame_out.textures_delta.clear();
            out = Some(frame_out);
        }
        out.expect("frames >= 1")
    }

    /// The gate fixture: the name devices plus the user's camera.
    fn gate_app() -> App {
        let mut app = test_app(name_fixture());
        let mut c = cam("tapo1", "");
        c.name = "Parking Lot".to_string();
        app.cfg.cameras.push(c);
        app
    }

    /// The v1.2.3 gate for Bug B. v1.2.2's unit test asserted the
    /// name-budget HELPER with a 268pt input the user's windows
    /// never produce, while the real render elided every name at
    /// every realistic width (pre-fix sweep: at 1500pt NO fixture
    /// name survived whole). This gate renders the REAL dashboard
    /// and reads back the galley rows egui actually drew. (The
    /// scene card's STATUS line is not in the list: at trough
    /// widths it sits within ~2pt of its budget and ellipsises
    /// there — identically in the web edition; the NAME is the
    /// contract.)
    const GATE_NAMES: [&str; 8] = [
        "Desk Light",
        "Kitchen Light 1",
        "Kitchen Light 2",
        "Love's Bed Light",
        "Shawn's Bed Light",
        "Desk RGB strip",
        "Parking Lot",
        "Turn off everything",
    ];

    fn assert_names_whole(texts: &[(String, egui::Pos2)], names: &[&str], ctx_label: &str) {
        let drawn: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
        for n in names {
            assert!(
                drawn.contains(n),
                "{ctx_label}: name {n:?} was not drawn whole; name-like texts: {:?}",
                drawn
                    .iter()
                    .filter(|t| t.contains("Li")
                        || t.contains("strip")
                        || t.contains("verything")
                        || t.contains("Parking")
                        || t.contains("Scene")
                        || t.contains("Turn")
                        || t.contains('\u{2026}'))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn gate_card_names_render_whole_at_real_widths() {
        // The window widths the app is actually used at (a
        // maximized window on a 150%-scaled 2560px monitor is
        // only ~1700pt wide; 125% scaling ~1590pt; narrower still
        // unmaximized). Cards at these widths are 238–262pt.
        for w in [1150.0f32, 1330.0, 1500.0, 1700.0, 1920.0] {
            let mut app = gate_app();
            let ctx = egui::Context::default();
            install_fonts(&ctx);
            // Rendered at 1200pt tall (was 1080): the dashboard now
            // carries the Thermostat section at the top by default,
            // so the last section no longer fits in a 1080 fold.
            // Height does not affect the name budgets under test.
            let texts = render_dashboard(&mut app, &ctx, egui::vec2(w, 1200.0), 3);
            assert_names_whole(&texts, &GATE_NAMES, &format!("comfortable w={w}"));
        }
        // Compact density: cards shrink to ~198–208pt. Device and
        // camera names must still fit (two-row header); the long
        // scene name is a physical-fit exception (it ellipsises
        // in the web edition at these widths too).
        for w in [1150.0f32, 1330.0] {
            let mut app = gate_app();
            app.cfg.prefs.density = "compact".to_string();
            let ctx = egui::Context::default();
            install_fonts(&ctx);
            let texts = render_dashboard(&mut app, &ctx, egui::vec2(w, 1080.0), 3);
            assert_names_whole(&texts, &GATE_NAMES[..7], &format!("compact w={w}"));
        }
        // Edit mode adds the drag grip (~24pt) — names must hold.
        // (Rendered taller: edit-mode cards grow, and egui culls
        // text below the fold of the scroll area.)
        let mut app = gate_app();
        app.editing = true;
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let texts = render_dashboard(&mut app, &ctx, egui::vec2(1330.0, 1700.0), 3);
        assert_names_whole(&texts, &GATE_NAMES, "editing w=1330");
        // The thermostat is a section in the sequence, not a fixed
        // block: it renders first by default and moves with the
        // section machinery (ordered by section_seq, movable).
        // v2.0.2: the section exists only when paired, so this
        // block uses a paired app; the unpaired case is covered
        // by thermostat_hidden_until_paired.
        let mut app = gate_app();
        app.cfg.thermostat = Some(crate::hap::ThermostatPairing::default());
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let _ = render_dashboard(&mut app, &ctx, egui::vec2(1330.0, 1200.0), 3);
        let cache = &app.sections_cache;
        assert!(
            cache.len() > 1,
            "expected several sections, got {:?}",
            cache.iter().map(|s| s.key.as_str()).collect::<Vec<_>>()
        );
        assert!(
            cache.iter().any(|s| s.key == "thermostat"),
            "thermostat section missing from sections_cache: {:?}",
            cache.iter().map(|s| s.key.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(cache[0].key, "thermostat");
        let moved = moved_section_seq(&app.cfg.layout.section_seq, cache, "thermostat", 1);
        assert_eq!(moved[1], "thermostat");
        let back = moved_section_seq(&moved, cache, "thermostat", -1);
        assert_eq!(back[0], "thermostat");
    }

    #[test]
    fn gate_resize_from_tiny_start_keeps_names_whole() {
        // The user launches minimized to tray: early frames run at
        // a tiny hidden size. No measurement from those frames may
        // survive into the resized render.
        let mut app = gate_app();
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let _ = render_dashboard(&mut app, &ctx, egui::vec2(400.0, 300.0), 3);
        let texts = render_dashboard(&mut app, &ctx, egui::vec2(1920.0, 1080.0), 3);
        assert_names_whole(&texts, &GATE_NAMES, "after resize 400x300 -> 1920x1080");
    }




    #[test]
    fn gate_group_header_groups_stay_atomic() {
        // Bug A gate: in the Lighting header strip, "Brightness"
        // and "Colour temperature" (label + value) must sit on ONE
        // line in adjacent panels, and the swatches must form one
        // straight row — pre-fix the wrapped placer staircased
        // each group lower than the last (Δy ≈ 16pt per group).
        let mut app = gate_app();
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let out = render_dashboard_full(&mut app, &ctx, egui::vec2(1500.0, 1080.0), 3);
        let texts = drawn_texts(&out);
        let first = |needle: &str| {
            texts
                .iter()
                .filter(|(t, _)| t == needle)
                .min_by_key(|(_, p)| (p.y * 10.0) as i32)
                .map(|(_, p)| *p)
        };
        let bright = first("Brightness").expect("group Brightness label drawn");
        let temp = first("Colour temperature").expect("group Colour temperature label drawn");
        assert!(
            (bright.y - temp.y).abs() <= 1.5,
            "strip labels staircased: Brightness y={} vs Colour temperature y={}",
            bright.y,
            temp.y
        );
        assert!(
            temp.x - bright.x >= 235.0 && temp.x - bright.x <= 262.0,
            "strip panels not adjacent/atomic: temp.x - bright.x = {}",
            temp.x - bright.x
        );
        // The fixture bulbs report device 80%, which DISPLAYS
        // as UI 78% under the v1.3.1 mapping (device 11..100 ->
        // UI 1..100).
        let bval = first("78%").expect("group brightness value drawn");
        let tval = texts
            .iter()
            .filter(|(t, _)| {
                t.len() >= 4 && t.ends_with('K') && t[..t.len() - 1].chars().all(|c| c.is_ascii_digit())
            })
            .min_by_key(|(_, p)| (p.y * 10.0) as i32)
            .map(|(_, p)| *p)
            .expect("group temperature value drawn");
        assert!(
            (bval.y - bright.y).abs() <= 1.5 && (tval.y - temp.y).abs() <= 1.5,
            "strip values off their label line: 78% y={} (label {}), temp value y={} (label {})",
            bval.y,
            bright.y,
            tval.y,
            temp.y
        );
        // Slider tracks on ONE baseline (v1.3.0 gate): the two
        // group sliders' track rects (h=6: background + accent
        // fill per slider) inside the strip band must share a
        // centre y — pre-fix the placer's cross-axis centring put
        // the colour-temperature track ~1-3.5 pt below the
        // brightness track while the LABELS still passed the
        // 1.5 pt tolerance above.
        let mut track_ys: Vec<f32> = Vec::new();
        fn walkr(s: &egui::epaint::Shape, acc: &mut Vec<f32>, lo: f32, hi: f32) {
            match s {
                egui::epaint::Shape::Rect(r) => {
                    let c = r.rect.center();
                    if (r.rect.height() - 6.0).abs() < 0.1
                        && r.rect.width() >= 150.0
                        && c.y >= lo
                        && c.y <= hi
                    {
                        acc.push(c.y);
                    }
                }
                egui::epaint::Shape::Vec(v) => {
                    for x in v {
                        walkr(x, acc, lo, hi);
                    }
                }
                _ => {}
            }
        }
        for cs in &out.shapes {
            walkr(&cs.shape, &mut track_ys, bright.y + 15.0, bright.y + 70.0);
        }
        assert!(
            track_ys.len() >= 4,
            "expected both group sliders' track rects in the strip band, found {}",
            track_ys.len()
        );
        let (mut tmin, mut tmax) = (f32::MAX, f32::MIN);
        for y in &track_ys {
            tmin = tmin.min(*y);
            tmax = tmax.max(*y);
        }
        assert!(
            tmax - tmin <= 0.5,
            "group slider tracks off one baseline: centre y spans {:.2}..{:.2}",
            tmin,
            tmax
        );
        // Swatches: one straight row inside the strip band.
        // Filtered to the swatch radius (12) — the strip also
        // contains slider knobs (r=9, on the slider row below the
        // labels by design) and the All toggle's knob (r=10.5).
        let mut swatches: Vec<egui::Pos2> = Vec::new();
        fn walkc(s: &egui::epaint::Shape, acc: &mut Vec<egui::Pos2>) {
            match s {
                egui::epaint::Shape::Circle(c) => {
                    if (c.radius - 12.0).abs() < 0.6 {
                        acc.push(c.center);
                    }
                }
                egui::epaint::Shape::Vec(v) => {
                    for x in v {
                        walkc(x, acc);
                    }
                }
                _ => {}
            }
        }
        for cs in &out.shapes {
            walkc(&cs.shape, &mut swatches);
        }
        let mut centres: Vec<egui::Pos2> = swatches
            .into_iter()
            .filter(|c| c.y >= bright.y && c.y <= bright.y + 42.0)
            .collect();
        centres.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
        centres.dedup_by(|a, b| (a.x - b.x).abs() < 1.0 && (a.y - b.y).abs() < 1.0);
        assert!(
            centres.len() >= 10,
            "expected the 10 strip swatches in the header band, found {}",
            centres.len()
        );
        let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
        for c in &centres {
            ymin = ymin.min(c.y);
            ymax = ymax.max(c.y);
        }
        assert!(
            ymax - ymin <= 3.0,
            "strip swatches not on one row: centre y spans {:.1}..{:.1}",
            ymin,
            ymax
        );
    }

    // ---- Group-header slider DRIVE gate (v1.3.2) ----
    // The v1.3.1 planner tests passed while the field build's
    // group header sliders did nothing useful: the divergence
    // lived in the UI→plan wiring (group_slider), which no test
    // drove. These gates drive the REAL dashboard with synthetic
    // pointer drags on the header sliders (and, for comparison,
    // on a card's sliders) and read back the member states the
    // broadcast applied — the same observable the user sees.

    /// The user's Lighting section, mirrored: five tunable-white
    /// bulbs with MIXED device brightness / colour-temp readings
    /// (3080/3780/3970/3000/6500K) plus the colour-only strip.
    fn group_drive_fixture() -> Vec<Device> {
        let bulb = |id: &str, name: &str, bright_reported: i64, kelvin: i64| {
            dev(
                id,
                name,
                json!({
                    "state": true,
                    "brightness": bright_reported,
                    "color_temp": temp_slider_to_reported(temp_kelvin_to_slider(kelvin)),
                    "color_mode": "white",
                    "online": true
                }),
            )
        };
        vec![
            bulb("b1", "Desk Light", 1000, 3080),
            bulb("b2", "Kitchen Light 1", 800, 3780),
            bulb("b3", "Kitchen Light 2", 600, 3970),
            bulb("b4", "Love's Bed Light", 900, 3000),
            bulb("b5", "Shawn's Bed Light", 400, 6500),
            dev(
                "s1",
                "Desk RGB strip",
                json!({ "state": true, "color_mode": "colour", "hue": 10, "saturation": 0.5, "online": true }),
            ),
        ]
    }

    /// One dashboard frame with explicit input events.
    fn drive_frame(
        app: &mut App,
        ctx: &egui::Context,
        size: egui::Vec2,
        clock: &mut f64,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        *clock += 0.05;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some(*clock),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new())
                .show(ctx, |ui| app.view_dashboard(ui));
        });
        out.textures_delta.clear();
        out
    }

    /// Slider track rects (h≈6) drawn in a frame.
    fn slider_track_rects(out: &egui::FullOutput) -> Vec<egui::Rect> {
        let mut acc = Vec::new();
        fn walk(s: &egui::epaint::Shape, acc: &mut Vec<egui::Rect>) {
            match s {
                egui::epaint::Shape::Rect(r) => {
                    if (r.rect.height() - 6.0).abs() < 0.75 && r.rect.width() >= 100.0 {
                        acc.push(r.rect);
                    }
                }
                egui::epaint::Shape::Vec(v) => {
                    for x in v {
                        walk(x, acc);
                    }
                }
                _ => {}
            }
        }
        for cs in &out.shapes {
            walk(&cs.shape, &mut acc);
        }
        acc
    }

    fn widest_track(rects: &[egui::Rect]) -> egui::Rect {
        rects
            .iter()
            .copied()
            .max_by(|a, b| a.width().partial_cmp(&b.width()).unwrap())
            .expect("a slider track")
    }

    /// A realistic drag on a slim_slider track: press at the
    /// current ratio, travel to the target ratio in steps,
    /// release, then two idle frames for the commit to land.
    fn simulate_slider_drag(
        app: &mut App,
        ctx: &egui::Context,
        size: egui::Vec2,
        clock: &mut f64,
        track: egui::Rect,
        from_ratio: f32,
        to_ratio: f32,
    ) -> egui::FullOutput {
        let y = track.center().y;
        let lo = track.left() + 9.0;
        let hi = track.right() - 9.0;
        let at = |r: f32| egui::pos2(lo + r * (hi - lo), y);
        drive_frame(app, ctx, size, clock, vec![egui::Event::PointerMoved(at(from_ratio))]);
        drive_frame(
            app,
            ctx,
            size,
            clock,
            vec![egui::Event::PointerButton {
                pos: at(from_ratio),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        for i in 1..=6 {
            let r = from_ratio + (to_ratio - from_ratio) * (i as f32 / 6.0);
            drive_frame(app, ctx, size, clock, vec![egui::Event::PointerMoved(at(r))]);
        }
        drive_frame(
            app,
            ctx,
            size,
            clock,
            vec![egui::Event::PointerButton {
                pos: at(to_ratio),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        drive_frame(app, ctx, size, clock, vec![]);
        drive_frame(app, ctx, size, clock, vec![])
    }

    /// The group header's two slider tracks + label positions,
    /// located from the drawn output of a settled frame.
    fn header_slider_tracks(out: &egui::FullOutput) -> (egui::Rect, egui::Rect, egui::Pos2, egui::Pos2) {
        let texts = drawn_texts(out);
        let first = |needle: &str| {
            texts
                .iter()
                .filter(|(t, _)| t == needle)
                .min_by_key(|(_, p)| (p.y * 10.0) as i32)
                .map(|(_, p)| *p)
                .unwrap_or_else(|| panic!("label {needle} drawn"))
        };
        let bright_label = first("Brightness");
        let temp_label = first("Colour temperature");
        let band: Vec<egui::Rect> = slider_track_rects(out)
            .into_iter()
            .filter(|r| r.center().y >= bright_label.y + 12.0 && r.center().y <= bright_label.y + 70.0)
            .collect();
        let b_track = widest_track(
            &band.iter().copied().filter(|r| r.center().x < temp_label.x).collect::<Vec<_>>(),
        );
        let t_track = widest_track(
            &band.iter().copied().filter(|r| r.center().x > temp_label.x).collect::<Vec<_>>(),
        );
        (b_track, t_track, bright_label, temp_label)
    }

    /// The text drawn on a label's own line (its value readout).
    fn value_on_label_line(out: &egui::FullOutput, label: egui::Pos2) -> Option<String> {
        drawn_texts(out)
            .into_iter()
            .filter(|(_, p)| (p.y - label.y).abs() <= 2.0 && p.x > label.x + 30.0)
            .min_by_key(|(_, p)| (p.x * 10.0) as i32)
            .map(|(t, _)| t)
    }

    fn fixture_device(app: &App, id: &str) -> Device {
        app.devices.iter().find(|d| d.id == id).cloned().expect("fixture device")
    }

    #[test]
    fn gate_group_header_sliders_drive_members() {
        let mut app = test_app(group_drive_fixture());
        // Fixture sanity: device-scale readings as parsed.
        for (id, pct) in [("b1", 100), ("b2", 80), ("b3", 60), ("b4", 90), ("b5", 40)] {
            assert_eq!(fixture_device(&app, id).brightness_pct, Some(pct), "{id} fixture brightness");
        }
        assert_eq!(fixture_device(&app, "s1").brightness_pct, None, "strip has no brightness reading");
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let size = egui::vec2(1500.0, 1080.0);
        let mut clock = 50.0;
        // Settle.
        let mut out = drive_frame(&mut app, &ctx, size, &mut clock, vec![]);
        for _ in 0..2 {
            out = drive_frame(&mut app, &ctx, size, &mut clock, vec![]);
        }
        let (b_track, t_track, b_label, t_label) = header_slider_tracks(&out);
        // At rest the header displays the honest aggregates:
        // device-avg brightness 74 -> UI 71%; average Kelvin
        // ≈4066 -> the 4000K stop.
        assert_eq!(
            value_on_label_line(&out, b_label).as_deref(),
            Some("71%"),
            "header brightness at rest"
        );
        assert_eq!(
            value_on_label_line(&out, t_label).as_deref(),
            Some("4000K"),
            "header temp at rest must sit at the members' nearest stop, not the first stop"
        );
        // Drag header brightness to UI 40% (ratio (40-1)/99).
        let out = simulate_slider_drag(&mut app, &ctx, size, &mut clock, b_track, 70.0 / 99.0, 39.0 / 99.0);
        for id in ["b1", "b2", "b3", "b4", "b5"] {
            assert_eq!(
                fixture_device(&app, id).brightness_pct,
                Some(brightness_ui_to_device(40)),
                "{id} must be driven to the dragged UI 40% (device-mapped exactly once)"
            );
        }
        assert_eq!(fixture_device(&app, "s1").brightness_pct, None, "strip untouched by brightness");
        let (_, _, b_label, _) = header_slider_tracks(&out);
        assert_eq!(
            value_on_label_line(&out, b_label).as_deref(),
            Some("40%"),
            "header brightness readout after the drag"
        );
        // Drag header temp to stop index 3 (5000K), from stop 2.
        let (_, t_track, _, t_label) = header_slider_tracks(&out);
        let out = simulate_slider_drag(&mut app, &ctx, size, &mut clock, t_track, 0.5, 0.75);
        for id in ["b1", "b2", "b3", "b4", "b5"] {
            assert_eq!(
                fixture_device(&app, id).temp_slider,
                Some(temp_kelvin_to_slider(5000)),
                "{id} must be driven to the chosen 5000K stop exactly"
            );
        }
        assert_eq!(fixture_device(&app, "s1").temp_slider, None, "strip untouched by temp");
        let (_, _, _, t_label) = header_slider_tracks(&out);
        assert_eq!(
            value_on_label_line(&out, t_label).as_deref(),
            Some("5000K"),
            "header temp readout after the drag"
        );
    }

    #[test]
    fn gate_card_sliders_drive_one_member() {
        // The comparison path: the same gesture on a CARD slider
        // drives exactly that card's device (field-verified
        // behaviour the header must match in effect).
        let mut app = test_app(group_drive_fixture());
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let size = egui::vec2(1500.0, 1080.0);
        let mut clock = 80.0;
        let mut out = drive_frame(&mut app, &ctx, size, &mut clock, vec![]);
        for _ in 0..2 {
            out = drive_frame(&mut app, &ctx, size, &mut clock, vec![]);
        }
        let texts = drawn_texts(&out);
        let (_, _, hb_label, _) = header_slider_tracks(&out);
        // First card = "Desk Light" (name order): its Brightness
        // label is the topmost one BELOW the header strip.
        let card_b_label = texts
            .iter()
            .filter(|(t, p)| t == "Brightness" && p.y > hb_label.y + 70.0)
            .min_by_key(|(_, p)| ((p.y * 10.0) as i32, (p.x * 10.0) as i32))
            .map(|(_, p)| *p)
            .expect("first card Brightness label");
        let tracks = slider_track_rects(&out);
        let b_track = widest_track(
            &tracks
                .iter()
                .copied()
                .filter(|r| {
                    r.center().y >= card_b_label.y + 12.0
                        && r.center().y <= card_b_label.y + 45.0
                        && r.center().x >= card_b_label.x - 20.0
                        && r.center().x <= card_b_label.x + 280.0
                })
                .collect::<Vec<_>>(),
        );
        // b1 starts at UI 100% (ratio 1.0); drag to UI 40%.
        let _ = simulate_slider_drag(&mut app, &ctx, size, &mut clock, b_track, 1.0, 39.0 / 99.0);
        assert_eq!(fixture_device(&app, "b1").brightness_pct, Some(brightness_ui_to_device(40)));
        for (id, pct) in [("b2", 80), ("b3", 60), ("b4", 90), ("b5", 40)] {
            assert_eq!(
                fixture_device(&app, id).brightness_pct,
                Some(pct),
                "{id} must NOT be touched by a card drag"
            );
        }
    }

    #[test]
    fn two_row_header_breakpoint() {
        // Single-row at generous widths (budget >= 112), two-row
        // at the grid's real trough widths.
        assert!(!two_row_header(340.0, TRAILING_DEDUCT_DEVICE)); // ~400pt card
        assert!(two_row_header(190.0, TRAILING_DEDUCT_DEVICE)); // ~272pt card
        assert!(two_row_header(166.0, TRAILING_DEDUCT_DEVICE)); // 238pt trough
        // Cameras deduct less but still flip when tight.
        assert!(!two_row_header(200.0, TRAILING_DEDUCT_CAMERA));
        assert!(two_row_header(150.0, TRAILING_DEDUCT_CAMERA));
        // Two-row name budget at the trough fits "Kitchen Light 1"
        // (102.6pt at 15pt proportional, measured in the harness).
        let b = header_name_budget(166.0, TRAILING_DEDUCT_HIDE_ONLY);
        assert!(b >= 103.0, "two-row trough budget {b} < 103");
    }
}
