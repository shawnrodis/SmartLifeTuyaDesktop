use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
#[derive(Clone, Serialize, Deserialize)]
pub struct Creds {
    pub user_name: String,
    /// Stored obfuscated (XOR + base64), exactly like the web edition —
    /// this is NOT encryption, it just avoids plain text at a glance.
    pub password: String,
    pub country_code: String,
    pub platform: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at_ms: i64,
    pub region: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
}

/// A user-added RTSP camera (e.g. Tapo). Cameras are NOT Tuya devices,
/// but on the dashboard they are peers of devices: same sections, name
/// sort / custom order, pinning, groups and hiding.
#[derive(Clone, Serialize, Deserialize)]
pub struct Camera {
    pub id: String,
    pub name: String,
    pub user: String,
    /// Stored obfuscated (XOR + base64), exactly like the account password.
    pub password: String,
    pub host: String,
    #[serde(default = "default_rtsp_port")]
    pub port: u16,
    /// "tapo1" | "tapo2" | "custom"
    pub template: String,
    #[serde(default)]
    pub path: String,
}

pub fn default_rtsp_port() -> u16 {
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
pub fn new_camera_id() -> String {
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
pub fn remove_camera_record(
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
pub fn repair_camera_ids(
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

pub const CAMERA_TEMPLATES: [(&str, &str); 3] = [
    ("tapo1", "Tapo Stream 1 — High Quality"),
    ("tapo2", "Tapo Stream 2 — Standard"),
    ("custom", "Custom path"),
];

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct LayoutCfg {
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub assign: HashMap<String, String>,
    #[serde(default)]
    pub alias: HashMap<String, String>,
    #[serde(default)]
    pub hidden: Vec<String>,
    #[serde(default)]
    pub pins: Vec<String>,
    #[serde(default)]
    pub section_order: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub collapsed: HashMap<String, bool>,
    /// User-arranged order of the SECTIONS themselves (v1.2.1), as
    /// section order_keys: "cameras" | "pinned" | "lighting" |
    /// "group:{gid}" | "ungrouped" | "scenes". Empty/absent = the
    /// built-in default order; a section missing from the list slots
    /// in by that default (see effective_section_seq).
    #[serde(default)]
    pub section_seq: Vec<String>,
    /// Per-camera card size (v1.2.1), keyed by item id ("cam:{id}"):
    /// "large" | "wide"; absent = normal (one grid cell).
    #[serde(default)]
    pub cam_sizes: HashMap<String, String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Prefs {
    pub theme: String,
    pub accent: String,
    pub density: String,
    pub autorefresh: bool,
    pub interval: u64,
    pub proxy_preset: String,
    pub proxy_custom: String,
    /// Stream gateway base URL for in-app camera playback ("" = none).
    #[serde(default)]
    pub gateway_url: String,
    /// "go2rtc" | "mediamtx"
    #[serde(default = "default_gateway_type")]
    pub gateway_type: String,
    /// External player command for ▶ Watch when no gateway is set.
    #[serde(default = "default_player_cmd")]
    pub player_cmd: String,
    /// Live in-tile camera previews (v1.2.0, ffmpeg frame pump).
    #[serde(default = "default_true")]
    pub previews: bool,
    /// ffmpeg path override ("" = bundled copy beside the app exe,
    /// then PATH). A full path or a bare program name both work.
    #[serde(default)]
    pub ffmpeg_path: String,
    /// Start with the OS (v1.2.1): an entry exists in the
    /// platform's per-user autostart store (Windows: the HKCU
    /// Run key; macOS: a LaunchAgent plist, v1.4.0). The store
    /// is the source of truth — this is reconciled against it
    /// at startup (see reconcile_autostart).
    #[serde(default)]
    pub start_with_windows: bool,
    /// How autostart launches the app: "normal" | "minimized" |
    /// "maximized" — carried as a flag in the Run command.
    #[serde(default = "default_launch_mode")]
    pub launch_mode: String,
    /// Keep running in the system tray when the window is closed
    /// (v1.2.1) instead of quitting.
    #[serde(default)]
    pub close_to_tray: bool,
    /// The one-time "still running in the tray" balloon has shown.
    #[serde(default)]
    pub tray_notice_shown: bool,
}

pub fn default_true() -> bool {
    true
}

pub fn default_launch_mode() -> String {
    "normal".to_string()
}

pub fn default_gateway_type() -> String {
    "go2rtc".to_string()
}

pub fn default_player_cmd() -> String {
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
            autorefresh: true,
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
            close_to_tray: false,
            tray_notice_shown: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub creds: Option<Creds>,
    #[serde(default)]
    pub session: Option<Session>,
    #[serde(default)]
    pub prefs: Prefs,
    #[serde(default)]
    pub layout: LayoutCfg,
    #[serde(default)]
    pub cameras: Vec<Camera>,
    #[serde(default)]
    pub update: UpdateCfg,
    #[serde(default)]
    pub cache_devices: Option<Value>,
}

/// Auto-update bookkeeping (mirrors the web edition's slc_update).
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct UpdateCfg {
    #[serde(default)]
    pub last_check_ms: i64,
    #[serde(default)]
    pub dismissed: String,
}

pub fn config_path() -> PathBuf {
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
    base.join("SmartLifeTuyaDesktop").join("config.json")
}

/// The directory holding config.json — sibling files (the macOS
/// single-instance lock, the show-request signal) live here too.
pub fn config_dir() -> PathBuf {
    config_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn load_config() -> Config {
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

pub fn save_config(cfg: &Config) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(&path, text);
    }
}
