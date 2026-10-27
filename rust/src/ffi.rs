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

use crate::camera::{build_rtsp_url, tile_rtsp_url};
use crate::config::{load_config, save_config, Camera, Config, Creds, LayoutCfg, Prefs, Session};
use crate::device::{build_sections, camera_entry, device_entry, parse_device, CardEntry};
use crate::ffmpeg::find_ffmpeg;
use crate::monitor::enumerate_monitors;
use crate::net::{fresh_session, http_client, is_rate_limit_code, net_auth, net_control, net_discovery, net_skill, skill_body, DiscoveryFail};
use crate::obfuscate::{b64_encode, deobfuscate, obfuscate};
use crate::registry::{registry_backoff_secs, registry_exists, registry_poll, registry_start, registry_stop};
use crate::system::{apply_autostart, autostart_get, drain_tray_actions};
use crate::update::{parse_release, parse_version, update_install_for, version_newer, RELEASES_API};
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

fn do_discovery(session: &Session, prefs: &Prefs) -> Result<(Session, Vec<Value>), String> {
    let client = http_client();
    let previous = cached_devices();
    match net_discovery(&client, session, prefs, &previous) {
        Ok(devs) => Ok((session.clone(), devs)),
        Err(DiscoveryFail::Frequent) => Ok((session.clone(), previous)),
        Err(DiscoveryFail::Msg(m)) => Err(m),
    }
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
    CString::new("1.5.0-rust").map(|c| c.into_raw()).unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn tuya_load_config() -> *mut c_char {
    let cfg = load_config();
    json_to_cstring(serde_json::to_value(&cfg).unwrap_or(Value::Null))
}

#[no_mangle]
pub extern "C" fn tuya_save_config(config_json: *const c_char) -> *mut c_char {
    let text = ptr_to_string(config_json);
    match serde_json::from_str::<Config>(&text) {
        Ok(cfg) => {
            save_config(&cfg);
            json_to_cstring(json!({"ok": true}))
        }
        Err(e) => err_json(format!("invalid config: {}", e)),
    }
}

#[no_mangle]
pub extern "C" fn tuya_login(creds_json: *const c_char) -> *mut c_char {
    let text = ptr_to_string(creds_json);
    let v: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return err_json(format!("invalid creds json: {}", e)),
    };
    let (creds_v, prefs_v) = split_creds_prefs(&v);
    let prefs = if prefs_v.is_object() { prefs_from_value(&prefs_v) } else { load_config().prefs };
    let mut creds: Creds = match serde_json::from_value(creds_v.clone()) {
        Ok(c) => c,
        Err(e) => return err_json(format!("invalid creds: {}", e)),
    };
    // Plain password for this call, unless the caller says it is the
    // stored obfuscated form. The stored form in `creds.password`
    // is never overwritten here.
    let plain = if creds_v.get("password_obfuscated").and_then(|b| b.as_bool()).unwrap_or(false) {
        deobfuscate(&creds.password)
    } else if let Some(p) = creds_v.get("plain_password").and_then(|x| x.as_str()) {
        p.to_string()
    } else {
        creds.password.clone()
    };
    if creds_v.get("password_obfuscated").and_then(|b| b.as_bool()).unwrap_or(false) {
        // keep stored form as-is
    } else {
        creds.password = obfuscate(&plain);
    }
    let client = http_client();
    let session = match net_auth(&client, &creds, &plain, &prefs) {
        Ok(s) => s,
        Err(e) => return err_json(e),
    };
    match do_discovery(&session, &prefs) {
        Ok((s, devs)) => json_to_cstring(json!({"session": s, "devices": devs})),
        Err(e) => err_json(e),
    }
}

#[no_mangle]
pub extern "C" fn tuya_refresh(session_json: *const c_char, prefs_json: *const c_char) -> *mut c_char {
    let session: Session = match serde_json::from_str(&ptr_to_string(session_json)) {
        Ok(s) => s,
        Err(e) => return err_json(format!("invalid session: {}", e)),
    };
    let prefs_text = ptr_to_string(prefs_json);
    let prefs = if prefs_text.trim().is_empty() {
        load_config().prefs
    } else {
        match serde_json::from_str::<Value>(&prefs_text) {
            Ok(v) => prefs_from_value(&v),
            Err(e) => return err_json(format!("invalid prefs: {}", e)),
        }
    };
    let client = http_client();
    let fresh = match fresh_session(&client, &session, &None, &prefs, false) {
        Ok(s) => s,
        Err(e) => return err_json(e),
    };
    match do_discovery(&fresh, &prefs) {
        Ok((s, devs)) => json_to_cstring(json!({"session": s, "devices": devs})),
        Err(e) => err_json(e),
    }
}

#[no_mangle]
pub extern "C" fn tuya_control(device_id: *const c_char, command_json: *const c_char) -> *mut c_char {
    let dev_id = ptr_to_string(device_id);
    let v: Value = match serde_json::from_str(&ptr_to_string(command_json)) {
        Ok(v) => v,
        Err(e) => return err_json(format!("invalid command json: {}", e)),
    };
    let action = v.get("action").or_else(|| v.get("command")).or_else(|| v.get("name"))
        .and_then(|x| x.as_str()).unwrap_or("turnOnOff").to_string();
    let value = v.get("value").cloned().unwrap_or(Value::Null);
    // Session/prefs may ride inside the command, else fall back to
    // the stored config (the app always has a session after login).
    let (session, prefs) = if let Some(s) = v.get("session") {
        match serde_json::from_value::<Session>(s.clone()) {
            Ok(sess) => (sess, v.get("prefs").map(|p| prefs_from_value(p)).unwrap_or_else(|| load_config().prefs)),
            Err(e) => return err_json(format!("invalid session in command: {}", e)),
        }
    } else {
        let cfg = load_config();
        match cfg.session {
            Some(s) => (s, cfg.prefs),
            None => return err_json("no stored session; call tuya_login first"),
        }
    };
    let client = http_client();
    let ok = net_control(&client, &session, &dev_id, &action, &value, &prefs);
    if ok {
        json_to_cstring(json!({"ok": true, "outcome": "ok"}))
    } else {
        json_to_cstring(json!({"ok": false, "outcome": "failed"}))
    }
}

#[no_mangle]
pub extern "C" fn tuya_build_sections(devices_json: *const c_char, layout_json: *const c_char) -> *mut c_char {
    let devs_v: Value = match serde_json::from_str(&ptr_to_string(devices_json)) {
        Ok(v) => v,
        Err(e) => return err_json(format!("invalid devices json: {}", e)),
    };
    let layout_v: Value = match serde_json::from_str(&ptr_to_string(layout_json)) {
        Ok(v) => v,
        Err(e) => return err_json(format!("invalid layout json: {}", e)),
    };
    // layout_json may be a bare LayoutCfg, or {layout:{...}, cameras:[...]}.
    let (layout_obj, extra_cameras) = if layout_v.get("layout").is_some() {
        (layout_v.get("layout").cloned().unwrap_or(Value::Null), layout_v.get("cameras").cloned())
    } else {
        (layout_v.clone(), layout_v.get("cameras").cloned())
    };
    let layout = layout_from_value(&layout_obj);
    let mut entries: Vec<CardEntry> = Vec::new();
    let mut devices: Vec<crate::device::Device> = Vec::new();
    let mut cameras: Vec<Camera> = Vec::new();
    let items = devs_v.as_array().cloned().unwrap_or_default();
    for raw in &items {
        // Camera records: direct Camera JSON, or wrapped.
        let cam_v: Option<Value> = if raw.get("host").is_some() && raw.get("template").is_some() {
            Some(raw.clone())
        } else if let Some(c) = raw.get("camera") {
            Some(c.clone())
        } else if raw.get("kind").and_then(|k| k.as_str()) == Some("camera") {
            raw.get("record").cloned().or_else(|| Some(raw.clone()))
        } else {
            None
        };
        if let Some(cv) = cam_v {
            if let Ok(cam) = serde_json::from_value::<Camera>(cv) {
                entries.push(camera_entry(&cam));
                cameras.push(cam);
                continue;
            }
        }
        let dev = parse_device(raw);
        entries.push(device_entry(&dev));
        devices.push(dev);
    }
    if let Some(cam_arr) = extra_cameras.and_then(|c| c.as_array().cloned()) {
        for cv in cam_arr {
            if let Ok(cam) = serde_json::from_value::<Camera>(cv) {
                let entry = camera_entry(&cam);
                if !entries.iter().any(|e| e.id == entry.id) {
                    entries.push(entry);
                    cameras.push(cam);
                }
            }
        }
    }
    let sections = build_sections(&entries, &layout, false, "");
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
        out_sections.push(json!({
            "key": s.key, "order_key": s.order_key, "title": s.title,
            "kind": format!("{:?}", s.kind), "group_id": s.group_id, "items": items_out,
        }));
    }
    json_to_cstring(json!({"sections": out_sections}))
}

#[no_mangle]
pub extern "C" fn tuya_camera_start(camera_json: *const c_char, ffmpeg_path: *const c_char, w: i32, h: i32) -> *mut c_char {
    let mut cam: Camera = match serde_json::from_str(&ptr_to_string(camera_json)) {
        Ok(c) => c,
        Err(e) => return err_json(format!("invalid camera json: {}", e)),
    };
    // Accept a plain password for this call without mutating the
    // stored obfuscated form's meaning for the URL builder: the
    // builder deobfuscates `cam.password`, so a plain password must
    // be obfuscated first.
    let raw_v: Value = serde_json::from_str(&ptr_to_string(camera_json)).unwrap_or(Value::Null);
    if let Some(p) = raw_v.get("plain_password").and_then(|x| x.as_str()) {
        cam.password = obfuscate(p);
    }
    let (pw, ph) = if w > 0 && h > 0 { (w as usize, h as usize) } else { (480, 270) };
    // Tile-sized requests use the cheap Stream 2 on Tapo templates
    // (the original tile rule); larger requests use the camera's own
    // quality selection (the original big-player rule).
    let url = if pw <= 480 { tile_rtsp_url(&cam) } else { build_rtsp_url(&cam) };
    let override_path = ptr_to_string(ffmpeg_path);
    let ff = match find_ffmpeg(&override_path) {
        Some(p) => p,
        None => return err_json("ffmpeg not found (bundled copy, override, or PATH)"),
    };
    let pump_id = format!("cam:{}", cam.id);
    match registry_start(&pump_id, &ff, &url, pw, ph) {
        Ok(()) => json_to_cstring(json!({"pump_id": pump_id})),
        Err(e) => err_json(e),
    }
}

#[no_mangle]
pub extern "C" fn tuya_camera_frame(pump_id: *const c_char) -> *mut c_char {
    let id = ptr_to_string(pump_id);
    if !registry_exists(&id) {
        return json_to_cstring(json!({"state": "ended", "error": "unknown pump"}));
    }
    match registry_poll(&id) {
        Some((state, w, h, frame, err)) => {
            let mut out = Map::new();
            out.insert("state".to_string(), Value::String(state));
            out.insert("w".to_string(), json!(w));
            out.insert("h".to_string(), json!(h));
            out.insert("rgba_base64".to_string(), Value::String(frame.map(|b| b64_encode(&b)).unwrap_or_default()));
            if let Some(e) = err {
                out.insert("error".to_string(), Value::String(e));
            }
            if let Some(secs) = registry_backoff_secs(&id) {
                out.insert("backoff_secs".to_string(), json!(secs));
            }
            json_to_cstring(Value::Object(out))
        }
        None => json_to_cstring(json!({"state": "ended", "error": "unknown pump"})),
    }
}

#[no_mangle]
pub extern "C" fn tuya_camera_stop(pump_id: *const c_char) -> *mut c_char {
    registry_stop(&ptr_to_string(pump_id));
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
pub extern "C" fn tuya_update_check(current_version: *const c_char) -> *mut c_char {
    let current = ptr_to_string(current_version);
    let client = http_client();
    let resp = match client.get(RELEASES_API).header("User-Agent", "SmartLifeTuyaDesktop").send() {
        Ok(r) => r,
        Err(e) => return err_json(format!("network error: {}", e)),
    };
    let v: Value = match resp.json() {
        Ok(v) => v,
        Err(e) => return err_json(format!("bad response: {}", e)),
    };
    let info = match parse_release(&v) {
        Some(i) => i,
        None => return err_json("no compatible release asset in response"),
    };
    let newer = version_newer(&info.version, &current);
    let mut out = serde_json::to_value(&info).unwrap_or(Value::Null);
    if let Value::Object(ref mut m) = out {
        m.insert("update_available".to_string(), Value::Bool(newer));
        m.insert("current_version".to_string(), Value::String(current));
        m.insert("install".to_string(), Value::String(match update_install_for(std::env::consts::OS) {
            crate::update::UpdateInstall::RunInstallerAndExit => "run_installer_and_exit".to_string(),
            crate::update::UpdateInstall::OpenDiskImage => "open_disk_image".to_string(),
        }));
        // Keep parse_version exported-observable for tests/shells.
        let _ = parse_version(&info.version);
    }
    json_to_cstring(out)
}

#[no_mangle]
pub extern "C" fn tuya_tray_actions() -> *mut c_char {
    let actions: Vec<String> = drain_tray_actions().iter().map(|a| a.as_str().to_string()).collect();
    json_to_cstring(json!({"actions": actions}))
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
