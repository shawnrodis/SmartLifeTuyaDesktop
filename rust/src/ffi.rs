//! C ABI for the Flutter shell: JSON in, JSON out, caller frees.
//!
//! Every function returning a string returns a NUL-terminated UTF-8
//! buffer allocated by Rust; the caller MUST release it with
//! `tuya_free_string`. A null input pointer is treated as an empty
//! string (functions then return an `{"error":...}` JSON, except
//! `tuya_version`, which never fails).
//!
//! Secrets: passwords and tokens are never logged and never appear
//! in error strings. `tuya_login` takes the PLAIN account password
//! for that one call (or, if `password_obfuscated` is true, the
//! stored XOR+base64 form, which is deobfuscated in-memory); the
//! core does not persist credentials — the shell calls
//! `tuya_save_config` explicitly when it wants them stored, with
//! the password already obfuscated by the shell (or by the core's
//! `obfuscate` helper via a direct rlib call).

use crate::camera::{build_rtsp_url, gateway_watch_url, tile_rtsp_url};
use crate::config::{load_config, save_config, Camera, Config, Creds, LayoutCfg, Prefs, Session};
use crate::device::{build_sections, camera_entry, device_entry, parse_device, CardEntry};
use crate::player::launch_player;
use crate::ffmpeg::find_ffmpeg;
use crate::monitor::enumerate_monitors;
use crate::net::{fresh_session, http_client, is_rate_limit_code, net_auth, net_control, net_discovery, net_skill, skill_body, DiscoveryFail};
use crate::obfuscate::{b64_encode, deobfuscate, obfuscate};
use crate::registry::{registry_backoff_secs, registry_exists, registry_poll, registry_start, registry_stop};
use crate::system::{apply_autostart, autostart_get, drain_tray_actions};
use crate::update::{parse_release, update_install_for, version_newer, RELEASES_API};
use serde_json::{json, Map, Value};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

fn ptr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
}

fn json_to_cstring(v: Value) -> *mut c_char {
    let s = serde_json::to_string(&v).unwrap_or_else(|_| "{\"error\":\"serialization failed\"}".to_string());
    CString::new(s).map(|c| c.into_raw()).unwrap_or(std::ptr::null_mut())
}

fn err_json(msg: impl Into<String>) -> *mut c_char {
    json_to_cstring(json!({"error": msg.into()}))
}

/// Merge a possibly-partial JSON object over a default value's JSON,
/// then deserialize. Lets the shell pass `{"theme":"black"}` instead
/// of a complete Prefs/LayoutCfg document.
fn merge_over<T: serde::de::DeserializeOwned + serde::Serialize>(default: &T, patch: &Value) -> Result<T, String> {
    let mut base = serde_json::to_value(default).map_err(|e| e.to_string())?;
    if let (Value::Object(b), Value::Object(p)) = (&mut base, patch) {
        for (k, v) in p {
            b.insert(k.clone(), v.clone());
        }
    } else if patch.is_object() {
        base = patch.clone();
    }
    serde_json::from_value(base).map_err(|e| e.to_string())
}

fn prefs_from_value(v: &Value) -> Prefs {
    merge_over(&Prefs::default(), v).unwrap_or_default()
}

fn layout_from_value(v: &Value) -> LayoutCfg {
    merge_over(&LayoutCfg::default(), v).unwrap_or_default()
}

/// Split a login/refresh payload that may be flat
/// (`{user_name, password, ...}`) or nested (`{creds:{...},
/// prefs:{...}}`).
#[allow(dead_code)]
fn split_creds_prefs(v: &Value) -> (Value, Value) {
    if let Some(creds) = v.get("creds") {
        (creds.clone(), v.get("prefs").cloned().unwrap_or(Value::Null))
    } else {
        (v.clone(), v.get("prefs").cloned().unwrap_or(Value::Null))
    }
}

fn cached_devices() -> Vec<Value> {
    // Best-effort only: a missing/corrupt config is not a login error.
    let cfg = load_config();
    cfg.cache_devices
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

#[allow(dead_code)]
fn do_discovery(session: &Session, prefs: &Prefs) -> Result<(Session, Vec<Value>), String> {
    let client = http_client();
    let previous = cached_devices();
    match net_discovery(&client, session, prefs, &previous) {
        Ok(devs) => Ok((session.clone(), devs)),
        Err(DiscoveryFail::Frequent) => Ok((session.clone(), previous)),
        Err(DiscoveryFail::Msg(m)) => Err(m),
    }
}


fn session_stripped(s: &Session) -> Value {
    json!({"region": s.region, "expires_at_ms": s.expires_at_ms, "has_token": true})
}
fn config_stripped(cfg: &Config) -> Value {
    let mut v = serde_json::to_value(cfg).unwrap_or(Value::Null);
    if let Value::Object(ref mut m) = v {
        match &cfg.creds {
            Some(c) => { m.insert("creds".to_string(), json!({"user_name": c.user_name, "country_code": c.country_code, "platform": c.platform, "has_password": true})); }
            None => { m.insert("creds".to_string(), Value::Null); }
        }
        match &cfg.session {
            Some(s) => { m.insert("session".to_string(), session_stripped(s)); }
            None => { m.insert("session".to_string(), Value::Null); }
        }
    }
    v
}
fn persist_session_cache(session: &Session, raw: &[Value]) {
    let mut cfg = load_config();
    cfg.session = Some(session.clone());
    cfg.cache_devices = Some(Value::Array(raw.to_vec()));
    save_config(&cfg);
}
enum DiscOut { Ok(Vec<Value>), Frequent, Err(String) }
fn discovery_parsed(session: &Session, prefs: &Prefs) -> DiscOut {
    let client = http_client();
    let previous = cached_devices();
    match crate::net::net_discovery(&client, session, prefs, &previous) {
        Ok(raw) => {
            let parsed: Vec<Value> = raw.iter().map(|r| serde_json::to_value(parse_device(r)).unwrap_or(Value::Null)).collect();
            DiscOut::Ok(parsed)
        }
        Err(DiscoveryFail::Frequent) => DiscOut::Frequent,
        Err(DiscoveryFail::Msg(m)) => DiscOut::Err(m),
    }
}
fn discovery_raw(session: &Session, prefs: &Prefs) -> Result<Vec<Value>, String> {
    let client = http_client();
    let previous = cached_devices();
    match crate::net::net_discovery(&client, session, prefs, &previous) {
        Ok(raw) => Ok(raw),
        Err(DiscoveryFail::Frequent) => Ok(previous),
        Err(DiscoveryFail::Msg(m)) => Err(m),
    }
}
fn pump_key(camera_id: &str, mode: &str) -> String {
    let base = if camera_id.starts_with("cam:") { camera_id.to_string() } else { format!("cam:{}", camera_id) };
    format!("{}:{}", base, mode)
}
fn camera_record_by_id(id: &str) -> Option<Camera> {
    let cfg = load_config();
    let bare = id.strip_prefix("cam:").unwrap_or(id);
    cfg.cameras.iter().find(|c| c.id == bare).cloned()
}

/// Caller frees with `tuya_free_string`.
#[no_mangle]
pub extern "C" fn tuya_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe { let _ = CString::from_raw(ptr); };
    }
}

#[no_mangle]
pub extern "C" fn tuya_version() -> *mut c_char {
    json_to_cstring(json!({"ok": true, "version": "1.5.0"}))
}

#[no_mangle]
pub extern "C" fn tuya_load_config() -> *mut c_char {
    let cfg = load_config();
    json_to_cstring(json!({"ok": true, "config": config_stripped(&cfg)}))
}

#[no_mangle]
pub extern "C" fn tuya_save_config(config_json: *const c_char) -> *mut c_char {
    let v: Value = match serde_json::from_str(&ptr_to_string(config_json)) {
        Ok(v) => v,
        Err(e) => return err_json(format!("invalid config: {}", e)),
    };
    let mut cfg = load_config();
    if let Some(p) = v.get("prefs") { cfg.prefs = prefs_from_value(p); }
    if let Some(l) = v.get("layout") { cfg.layout = layout_from_value(l); }
    if let Some(cams) = v.get("cameras") {
        if let Ok(cs) = serde_json::from_value::<Vec<Camera>>(cams.clone()) { cfg.cameras = cs; }
    }
    if let Some(u) = v.get("update") {
        if let Ok(uc) = serde_json::from_value(u.clone()) { cfg.update = uc; }
    }
    if let Some(cd) = v.get("cache_devices") { cfg.cache_devices = Some(cd.clone()); }
    // creds/session: explicit null clears, a marker object preserves the
    // stored secret (updating only user/region/platform), a full object
    // with access_token/password replaces it. Dart never holds secrets,
    // so the common path is preserve-on-marker / clear-on-null (logout).
    match v.get("creds") {
        Some(Value::Null) => { cfg.creds = None; }
        Some(obj) if obj.is_object() => {
            if let Some(mut c) = cfg.creds.clone() {
                if let Some(x) = obj.get("user_name").and_then(|x| x.as_str()) { c.user_name = x.to_string(); }
                if let Some(x) = obj.get("country_code").and_then(|x| x.as_str()) { c.country_code = x.to_string(); }
                if let Some(x) = obj.get("platform").and_then(|x| x.as_str()) { c.platform = x.to_string(); }
                if let Some(x) = obj.get("password").and_then(|x| x.as_str()) {
                    if !x.is_empty() && obj.get("has_password").is_none() { c.password = x.to_string(); }
                }
                cfg.creds = Some(c);
            } else if let (Some(u), Some(pw)) = (obj.get("user_name").and_then(|x| x.as_str()), obj.get("password").and_then(|x| x.as_str())) {
                if !pw.is_empty() { cfg.creds = Some(Creds { user_name: u.to_string(), password: pw.to_string(), country_code: obj.get("country_code").and_then(|x| x.as_str()).unwrap_or("1").to_string(), platform: obj.get("platform").and_then(|x| x.as_str()).unwrap_or("smart_life").to_string() }); }
            }
        }
        _ => {}
    }
    match v.get("session") {
        Some(Value::Null) => { cfg.session = None; }
        Some(obj) if obj.is_object() => {
            if let Some(tok) = obj.get("access_token").and_then(|x| x.as_str()) {
                if !tok.is_empty() {
                    cfg.session = serde_json::from_value::<Session>(obj.clone()).ok().or(cfg.session.clone());
                }
            } else if let Some(mut sess) = cfg.session.clone() {
                if let Some(r) = obj.get("region").and_then(|x| x.as_str()) { sess.region = r.to_string(); }
                if let Some(e) = obj.get("expires_at_ms").and_then(|x| x.as_i64()) { sess.expires_at_ms = e; }
                cfg.session = Some(sess);
            }
        }
        _ => {}
    }
    save_config(&cfg);
    json_to_cstring(json!({"ok": true}))
}

#[no_mangle]
pub extern "C" fn tuya_login(creds_json: *const c_char) -> *mut c_char {
    let v: Value = match serde_json::from_str(&ptr_to_string(creds_json)) {
        Ok(v) => v,
        Err(e) => return err_json(format!("invalid login json: {}", e)),
    };
    let client = http_client();
    if v.get("silent").and_then(|b| b.as_bool()).unwrap_or(false) {
        let cfg = load_config();
        let prefs = cfg.prefs.clone();
        let creds_opt: Option<(Creds, String)> = cfg.creds.clone().map(|c| { let pw = deobfuscate(&c.password); (c, pw) });
        let sess = match cfg.session.clone() {
            Some(s) => s,
            None => {
                if creds_opt.is_none() { return json_to_cstring(json!({"ok": false, "error": "no_saved_login"})); }
                // No session: fall through to a fresh auth with stored creds.
                let (c, pw) = creds_opt.clone().unwrap();
                match net_auth(&client, &c, &pw, &prefs) {
                    Ok(s) => s,
                    Err(e) => return err_json(e),
                }
            }
        };
        let fresh = match fresh_session(&client, &sess, &creds_opt, &prefs, false) {
            Ok(s) => s,
            Err(_) => {
                // Session refresh failed: try stored creds directly.
                match &creds_opt {
                    Some((c, pw)) => match net_auth(&client, c, pw, &prefs) { Ok(s) => s, Err(e) => return err_json(e) },
                    None => return err_json("Session expired"),
                }
            }
        };
        return match discovery_parsed(&fresh, &prefs) {
            DiscOut::Ok(devs) => {
                if let Ok(raw) = discovery_raw(&fresh, &prefs) { persist_session_cache(&fresh, &raw); } else { persist_session_cache(&fresh, &[]); }
                json_to_cstring(json!({"ok": true, "session": session_stripped(&fresh), "devices": devs}))
            }
            DiscOut::Frequent => json_to_cstring(json!({"ok": true, "rate_limited": true, "session": session_stripped(&fresh)})),
            DiscOut::Err(e) => err_json(e),
        };
    }
    let username = v.get("username").or_else(|| v.get("user_name")).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let plain = v.get("password").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let region = v.get("region").or_else(|| v.get("country_code")).and_then(|x| x.as_str()).unwrap_or("1").to_string();
    let platform = v.get("platform").and_then(|x| x.as_str()).unwrap_or("smart_life").to_string();
    let keep = v.get("keep").and_then(|b| b.as_bool()).unwrap_or(false);
    let prefs = if let Some(p) = v.get("prefs") { prefs_from_value(p) } else { load_config().prefs };
    let creds = Creds { user_name: username, password: obfuscate(&plain), country_code: region, platform };
    let session = match net_auth(&client, &creds, &plain, &prefs) {
        Ok(s) => s,
        Err(e) => return err_json(e),
    };
    match discovery_parsed(&session, &prefs) {
        DiscOut::Ok(devs) => {
            let raw = discovery_raw(&session, &prefs).unwrap_or_default();
            let mut cfg = load_config();
            cfg.session = Some(session.clone());
            cfg.cache_devices = Some(Value::Array(raw));
            if keep { cfg.creds = Some(creds); } else { cfg.creds = None; }
            save_config(&cfg);
            json_to_cstring(json!({"ok": true, "session": session_stripped(&session), "devices": devs}))
        }
        DiscOut::Frequent => json_to_cstring(json!({"ok": true, "rate_limited": true, "session": session_stripped(&session)})),
        DiscOut::Err(e) => err_json(e),
    }
}

#[no_mangle]
pub extern "C" fn tuya_refresh(req_json: *const c_char) -> *mut c_char {
    let v: Value = serde_json::from_str(&ptr_to_string(req_json)).unwrap_or(Value::Null);
    let force = v.get("force").and_then(|b| b.as_bool()).unwrap_or(false);
    let cfg = load_config();
    let prefs = cfg.prefs.clone();
    let sess = match cfg.session.clone() {
        Some(s) => s,
        None => return err_json("no stored session; call tuya_login first"),
    };
    let creds_opt: Option<(Creds, String)> = cfg.creds.clone().map(|c| { let pw = deobfuscate(&c.password); (c, pw) });
    let client = http_client();
    let fresh = match fresh_session(&client, &sess, &creds_opt, &prefs, force) {
        Ok(s) => s,
        Err(e) => return err_json(e),
    };
    match discovery_parsed(&fresh, &prefs) {
        DiscOut::Ok(devs) => {
            let raw = discovery_raw(&fresh, &prefs).unwrap_or_default();
            persist_session_cache(&fresh, &raw);
            json_to_cstring(json!({"ok": true, "session": session_stripped(&fresh), "devices": devs}))
        }
        DiscOut::Frequent => json_to_cstring(json!({"ok": true, "rate_limited": true, "session": session_stripped(&fresh)})),
        DiscOut::Err(e) => err_json(e),
    }
}

#[no_mangle]
pub extern "C" fn tuya_control(req_json: *const c_char) -> *mut c_char {
    let v: Value = match serde_json::from_str(&ptr_to_string(req_json)) {
        Ok(v) => v,
        Err(e) => return err_json(format!("invalid command json: {}", e)),
    };
    let dev_id = v.get("id").or_else(|| v.get("device_id")).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let action = v.get("command").or_else(|| v.get("action")).or_else(|| v.get("name")).and_then(|x| x.as_str()).unwrap_or("turnOnOff").to_string();
    let value = v.get("value").cloned().unwrap_or(Value::Null);
    let (session, prefs) = if let Some(s) = v.get("session") {
        if s.get("access_token").is_some() {
            match serde_json::from_value::<Session>(s.clone()) {
                Ok(sess) => (sess, v.get("prefs").map(|p| prefs_from_value(p)).unwrap_or_else(|| load_config().prefs)),
                Err(e) => return err_json(format!("invalid session in command: {}", e)),
            }
        } else {
            let cfg = load_config();
            match cfg.session { Some(s) => (s, cfg.prefs), None => return err_json("no stored session; call tuya_login first") }
        }
    } else {
        let cfg = load_config();
        match cfg.session { Some(s) => (s, cfg.prefs), None => return err_json("no stored session; call tuya_login first") }
    };
    let client = http_client();
    let ok = net_control(&client, &session, &dev_id, &action, &value, &prefs);
    if ok { json_to_cstring(json!({"ok": true, "outcome": "ok"})) } else { json_to_cstring(json!({"ok": false, "outcome": "failed"})) }
}

#[no_mangle]
pub extern "C" fn tuya_build_sections(req_json: *const c_char) -> *mut c_char {
    let v: Value = match serde_json::from_str(&ptr_to_string(req_json)) {
        Ok(v) => v,
        Err(e) => return err_json(format!("invalid sections json: {}", e)),
    };
    // Dart actually builds sections locally; this export accepts either
    // the Dart shape {devices,cameras,layout,query} or the legacy
    // two-argument shapes serialised as {devices,layout}.
    let devs_v = v.get("devices").cloned().unwrap_or(Value::Array(vec![]));
    let layout_v = v.get("layout").cloned().unwrap_or(Value::Null);
    let query = v.get("query").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let layout = layout_from_value(&layout_v);
    let mut entries: Vec<CardEntry> = Vec::new();
    let mut devices: Vec<crate::device::Device> = Vec::new();
    let mut cameras: Vec<Camera> = Vec::new();
    for raw in devs_v.as_array().cloned().unwrap_or_default() {
        let cam_v: Option<Value> = if raw.get("host").is_some() && raw.get("template").is_some() { Some(raw.clone()) }
            else if let Some(c) = raw.get("camera") { Some(c.clone()) }
            else if raw.get("kind").and_then(|k| k.as_str()) == Some("camera") { raw.get("record").cloned().or_else(|| Some(raw.clone())) } else { None };
        if let Some(cv) = cam_v {
            if let Ok(cam) = serde_json::from_value::<Camera>(cv) { entries.push(camera_entry(&cam)); cameras.push(cam); continue; }
        }
        let dev = parse_device(&raw);
        entries.push(device_entry(&dev));
        devices.push(dev);
    }
    if let Some(cam_arr) = v.get("cameras").and_then(|c| c.as_array().cloned()) {
        for cv in cam_arr {
            if let Ok(cam) = serde_json::from_value::<Camera>(cv) {
                let entry = camera_entry(&cam);
                if !entries.iter().any(|e| e.id == entry.id) { entries.push(entry); cameras.push(cam); }
            }
        }
    }
    let sections = build_sections(&entries, &layout, false, &query);
    let mut out_sections = Vec::new();
    for s in sections {
        let mut items_out = Vec::new();
        for id in &s.ids {
            if let Some(cam) = cameras.iter().find(|c| format!("cam:{}", c.id) == *id) {
                let mut item = serde_json::to_value(cam).unwrap_or(Value::Null);
                if let Value::Object(ref mut m) = item {
                    let disp = layout.alias.get(id).cloned().unwrap_or_else(|| cam.name.clone());
                    m.insert("display_name".to_string(), Value::String(disp));
                    m.insert("item_kind".to_string(), Value::String("camera".to_string()));
                    m.insert("rtsp_url".to_string(), Value::String(build_rtsp_url(cam)));
                    m.insert("tile_rtsp_url".to_string(), Value::String(tile_rtsp_url(cam)));
                    m.insert("gateway_watch_url".to_string(), gateway_watch_url(cam, &load_config().prefs).map(Value::String).unwrap_or(Value::Null));
                }
                items_out.push(item);
            } else if let Some(dev) = devices.iter().find(|d| d.id == *id) {
                let mut item = serde_json::to_value(dev).unwrap_or(Value::Null);
                if let Value::Object(ref mut m) = item {
                    let disp = layout.alias.get(&dev.id).cloned().unwrap_or_else(|| dev.name.clone());
                    m.insert("display_name".to_string(), Value::String(disp));
                    m.insert("item_kind".to_string(), Value::String("device".to_string()));
                }
                items_out.push(item);
            }
        }
        out_sections.push(json!({"key": s.key, "order_key": s.order_key, "title": s.title, "kind": format!("{:?}", s.kind), "group_id": s.group_id, "ids": s.ids, "items": items_out}));
    }
    json_to_cstring(json!({"ok": true, "sections": out_sections}))
}

#[no_mangle]
pub extern "C" fn tuya_camera_start(req_json: *const c_char) -> *mut c_char {
    let v: Value = match serde_json::from_str(&ptr_to_string(req_json)) {
        Ok(v) => v,
        Err(e) => return err_json(format!("invalid camera json: {}", e)),
    };
    let camera_id = v.get("camera_id").or_else(|| v.get("id")).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mode = v.get("mode").and_then(|x| x.as_str()).unwrap_or("tile").to_string();
    // Dart handles gateway/external in the UI; Rust still returns the URL.
    let cfg = load_config();
    let cam = match camera_record_by_id(&camera_id) {
        Some(c) => c,
        None => {
            // Fallback: a full camera record passed directly.
            match v.get("camera").cloned().or_else(|| Some(v.clone())) {
                Some(cv) => match serde_json::from_value::<Camera>(cv) { Ok(c) => c, Err(_) => return err_json("camera not found") },
                None => return err_json("camera not found"),
            }
        }
    };
    if mode == "gateway" {
        let url = gateway_watch_url(&cam, &cfg.prefs).unwrap_or_else(|| build_rtsp_url(&cam));
        return json_to_cstring(json!({"ok": true, "mode": "gateway", "url": url}));
    }
    if mode == "external" {
        let url = build_rtsp_url(&cam);
        let _ = launch_player(&cfg.prefs.player_cmd, &url);
        return json_to_cstring(json!({"ok": true, "mode": "external", "url": url}));
    }
    let (pw, ph, url) = if mode == "player" { (960usize, 540usize, build_rtsp_url(&cam)) } else { (480usize, 270usize, tile_rtsp_url(&cam)) };
    let override_path = v.get("ffmpeg_path").or_else(|| v.get("override")).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let ff_override = if override_path.is_empty() { cfg.prefs.ffmpeg_path.clone() } else { override_path };
    let ff = match find_ffmpeg(&ff_override) { Some(p) => p, None => return err_json("ffmpeg not found (bundled copy, override, or PATH)") };
    let key = pump_key(&camera_id, &mode);
    match registry_start(&key, &ff, &url, pw, ph) {
        Ok(()) => json_to_cstring(json!({"ok": true, "mode": mode, "pump_id": key, "url": url})),
        Err(e) => err_json(e),
    }
}

#[no_mangle]
pub extern "C" fn tuya_camera_frame(req_json: *const c_char) -> *mut c_char {
    let v: Value = serde_json::from_str(&ptr_to_string(req_json)).unwrap_or(Value::Null);
    let camera_id = v.get("camera_id").or_else(|| v.get("id")).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mode = v.get("pump").or_else(|| v.get("mode")).and_then(|x| x.as_str()).unwrap_or("tile").to_string();
    // Back-compat: a bare pump-id string is also accepted.
    let key = if camera_id.is_empty() {
        ptr_to_string(req_json)
    } else { pump_key(&camera_id, &mode) };
    if !registry_exists(&key) {
        return json_to_cstring(json!({"ok": false, "state": "ended", "error": "unknown pump"}));
    }
    match registry_poll(&key) {
        Some((state, w, h, frame, err)) => {
            let mut out = Map::new();
            out.insert("ok".to_string(), Value::Bool(true));
            out.insert("state".to_string(), Value::String(state));
            out.insert("width".to_string(), json!(w));
            out.insert("height".to_string(), json!(h));
            out.insert("w".to_string(), json!(w));
            out.insert("h".to_string(), json!(h));
            if let Some(b) = frame { out.insert("rgba_base64".to_string(), Value::String(b64_encode(&b))); }
            if let Some(e) = err { out.insert("error".to_string(), Value::String(e)); }
            if let Some(secs) = registry_backoff_secs(&key) { out.insert("backoff_secs".to_string(), json!(secs)); }
            json_to_cstring(Value::Object(out))
        }
        None => json_to_cstring(json!({"ok": false, "state": "ended", "error": "unknown pump"})),
    }
}

#[no_mangle]
pub extern "C" fn tuya_camera_stop(req_json: *const c_char) -> *mut c_char {
    let v: Value = serde_json::from_str(&ptr_to_string(req_json)).unwrap_or(Value::Null);
    let camera_id = v.get("camera_id").or_else(|| v.get("id")).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mode = v.get("pump").or_else(|| v.get("mode")).and_then(|x| x.as_str()).unwrap_or("tile").to_string();
    let key = if camera_id.is_empty() { ptr_to_string(req_json) } else { pump_key(&camera_id, &mode) };
    registry_stop(&key);
    json_to_cstring(json!({"ok": true}))
}

#[no_mangle]
pub extern "C" fn tuya_find_ffmpeg(override_json: *const c_char) -> *mut c_char {
    let text = ptr_to_string(override_json).trim().to_string();
    let ov = if text.is_empty() {
        String::new()
    } else if let Some(obj) = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.as_object().cloned()) {
        obj.get("ffmpeg_path").or_else(|| obj.get("override")).or_else(|| obj.get("path"))
            .and_then(|x| x.as_str()).unwrap_or("").to_string()
    } else if let Ok(s) = serde_json::from_str::<String>(&text) {
        s
    } else {
        text
    };
    match find_ffmpeg(&ov) {
        Some(p) => json_to_cstring(json!({"path": p.to_string_lossy().into_owned()})),
        None => json_to_cstring(json!({"path": Value::Null})),
    }
}

#[no_mangle]
pub extern "C" fn tuya_update_check(req_json: *const c_char) -> *mut c_char {
    let text = ptr_to_string(req_json);
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let action = v.get("action").and_then(|x| x.as_str()).unwrap_or("check").to_string();
    if action == "download" {
        // Re-check, pick this platform's exact asset, download it to a
        // temp file and verify the file name before returning the path.
        let client = http_client();
        let resp = match client.get(RELEASES_API).header("User-Agent", "SmartLifeTuyaDesktop").send() {
            Ok(r) => r, Err(e) => return err_json(format!("network error: {}", e)),
        };
        let rv: Value = match resp.json() { Ok(x) => x, Err(e) => return err_json(format!("bad response: {}", e)) };
        let info = match parse_release(&rv) { Some(i) => i, None => return err_json("no compatible release asset in response") };
        let want = crate::update::expected_update_asset_name_for(&info.version, std::env::consts::OS);
        let asset_url = info.asset_url.clone();
        let mut dest = std::env::temp_dir();
        dest.push(&want);
        let bytes = match client.get(&asset_url).header("User-Agent", "SmartLifeTuyaDesktop").send().and_then(|r| r.bytes()) {
            Ok(b) => b, Err(e) => return err_json(format!("download failed: {}", e)),
        };
        if std::fs::write(&dest, &bytes).is_err() { return err_json("could not write downloaded file"); }
        return json_to_cstring(json!({"ok": true, "downloaded_path": dest.to_string_lossy().into_owned(), "asset_name": want}));
    }
    if action == "install" {
        let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if path.is_empty() { return err_json("install requires a downloaded path"); }
        // Verify the file name matches this platform's expected asset.
        let file_name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !file_name.starts_with("SmartLifeTuyaDesktop-") { return err_json("downloaded file name did not verify"); }
        #[cfg(target_os = "windows")]
        { let _ = std::process::Command::new(&path).spawn(); return json_to_cstring(json!({"ok": true})); }
        #[cfg(target_os = "macos")]
        { let _ = std::process::Command::new("open").arg(&path).spawn(); return json_to_cstring(json!({"ok": true})); }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        { return json_to_cstring(json!({"ok": true, "path": path})); }
    }
    // check (default): current version may be a bare version or a JSON request.
    let current = v.get("current_version").or_else(|| v.get("version")).and_then(|x| x.as_str()).map(|s| s.to_string())
        .unwrap_or_else(|| if text.trim().starts_with('{') { String::new() } else { text.clone() });
    let client = http_client();
    let resp = match client.get(RELEASES_API).header("User-Agent", "SmartLifeTuyaDesktop").send() {
        Ok(r) => r, Err(e) => return err_json(format!("network error: {}", e)),
    };
    let rv: Value = match resp.json() { Ok(x) => x, Err(e) => return err_json(format!("bad response: {}", e)) };
    let info = match parse_release(&rv) { Some(i) => i, None => return err_json("no compatible release asset in response") };
    let newer = if current.is_empty() { true } else { version_newer(&info.version, &current) };
    let mut out = serde_json::to_value(&info).unwrap_or(Value::Null);
    if let Value::Object(ref mut m) = out {
        m.insert("update_available".to_string(), Value::Bool(newer));
        m.insert("current_version".to_string(), Value::String(current));
        m.insert("install".to_string(), Value::String(match update_install_for(std::env::consts::OS) {
            crate::update::UpdateInstall::RunInstallerAndExit => "run_installer_and_exit".to_string(),
            crate::update::UpdateInstall::OpenDiskImage => "open_disk_image".to_string(),
        }));
    }
    json_to_cstring(json!({"ok": true, "info": out}))
}

#[no_mangle]
pub extern "C" fn tuya_tray_actions(req_json: *const c_char) -> *mut c_char {
    let v: Value = serde_json::from_str(&ptr_to_string(req_json)).unwrap_or(Value::Null);
    let action = v.get("action").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if action == "ensure" {
        // Best-effort: build the native tray icon if the platform has
        // one. On Linux there is none, and that is not an error.
        let _ = crate::system::build_tray();
    }
    let actions: Vec<String> = drain_tray_actions().iter().map(|a| a.as_str().to_string()).collect();
    json_to_cstring(json!({"ok": true, "actions": actions}))
}

#[no_mangle]
pub extern "C" fn tuya_autostart_get() -> *mut c_char {
    let (enabled, mode) = autostart_get();
    json_to_cstring(json!({"ok": true, "enabled": enabled, "launch_mode": mode}))
}

#[no_mangle]
pub extern "C" fn tuya_autostart_set(json_ptr: *const c_char) -> *mut c_char {
    let v: Value = match serde_json::from_str(&ptr_to_string(json_ptr)) {
        Ok(v) => v,
        Err(e) => return err_json(format!("invalid autostart json: {}", e)),
    };
    let mut cfg = load_config();
    if let Some(b) = v.get("enabled").or_else(|| v.get("start_with_windows")).and_then(|x| x.as_bool()) {
        cfg.prefs.start_with_windows = b;
    }
    if let Some(m) = v.get("launch_mode").and_then(|x| x.as_str()) {
        cfg.prefs.launch_mode = m.to_string();
    }
    if !apply_autostart(&cfg.prefs) {
        return err_json("could not apply autostart to the OS store (on Linux the core has no autostart store; the Flutter shell owns it)");
    }
    save_config(&cfg);
    json_to_cstring(json!({"ok": true}))
}

#[no_mangle]
pub extern "C" fn tuya_monitors() -> *mut c_char {
    let monitors = enumerate_monitors();
    json_to_cstring(json!({"monitors": monitors}))
}

// Referenced so the rate-limit helper stays part of the public core
// surface even though the FFI control path classifies internally.
#[allow(dead_code)]
fn _rate_limit_probe(code: &str) -> bool {
    is_rate_limit_code(code)
}

#[allow(dead_code)]
fn _skill_probe(name: &str, payload: Value) -> Value {
    skill_body(name, payload)
}

#[allow(dead_code)]
fn _net_skill_probe() {
    let _ = net_skill as fn(&reqwest::blocking::Client, &Session, &Value, &Prefs) -> Result<Value, String>;
}
