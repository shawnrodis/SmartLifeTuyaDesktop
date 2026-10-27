// Ported pure-logic tests from the original app (src/main.rs tests),
// plus FFI-shape checks for build_sections. No network, no secrets
// (fixtures use dummy tokens/passwords only).
use serde_json::json;
use tuya_core::*;

fn cam(id: &str, name: &str) -> Camera {
    Camera { id: id.into(), name: name.into(), user: "admin".into(), password: obfuscate("pw"), host: "192.168.1.10".into(), port: 554, template: "tapo1".into(), path: String::new() }
}

#[test]
fn section_ordering_pinned_lighting_groups() {
    let mut layout = LayoutCfg::default();
    layout.groups = vec![Group { id: "g1".into(), name: "Living".into() }];
    layout.pins = vec!["dev-b".into()];
    layout.section_seq = vec!["pinned".into(), "cameras".into(), "lighting".into(), "group:g1".into(), "ungrouped".into(), "scenes".into()];
    let light = parse_device(&json!({"id":"dev-a","name":"Bulb","dev_type":"light","data":{"state":true,"brightness":"800","color_temp":"4000"}}));
    let plug = parse_device(&json!({"id":"dev-b","name":"Plug","dev_type":"switch","data":{"state":false}}));
    let scene = parse_device(&json!({"id":"dev-c","name":"Movie","dev_type":"scene","data":{}}));
    let entries = vec![device_entry(&light), device_entry(&plug), device_entry(&scene)];
    let sections = build_sections(&entries, &layout, false, "");
    let keys: Vec<&str> = sections.iter().map(|s| s.order_key.as_str()).collect();
    assert_eq!(keys[0], "pinned");
    assert!(sections.iter().find(|s| s.order_key=="pinned").unwrap().ids.contains(&"dev-b".to_string()));
    assert!(sections.iter().find(|s| s.order_key=="lighting").unwrap().ids.contains(&"dev-a".to_string()));
    assert!(sections.iter().find(|s| s.order_key=="scenes").unwrap().ids.contains(&"dev-c".to_string()));
    // order_sections with stored seq keeps pinned first
    let mut secs = sections.clone();
    order_sections(&mut secs, &layout.section_seq);
    assert_eq!(secs[0].order_key, "pinned");
}

#[test]
fn rtsp_url_building() {
    let c = cam("1", "Porch");
    assert_eq!(build_rtsp_url(&c), "rtsp://admin:pw@192.168.1.10:554/stream1");
    assert_eq!(tile_rtsp_url(&c), "rtsp://admin:pw@192.168.1.10:554/stream2");
    let mut custom = c.clone();
    custom.template = "custom".into();
    custom.path = "h264".into();
    assert_eq!(camera_path(&custom), "/h264");
    assert_eq!(build_rtsp_url(&custom), "rtsp://admin:pw@192.168.1.10:554/h264");
    // password with @ : / survives via percent-encoding
    let mut tricky = c.clone();
    tricky.password = obfuscate("p@ss:word/x");
    assert!(build_rtsp_url(&tricky).contains("p%40ss%3Aword%2Fx"));
}

#[test]
fn brightness_temp_conversions() {
    assert_eq!(brightness_to_pct(800.0), 80);
    assert_eq!(brightness_ui_to_device(1), 11);
    assert_eq!(brightness_ui_to_device(100), 100);
    assert_eq!(brightness_device_to_ui(11), 1);
    assert_eq!(brightness_device_to_ui(100), 100);
    // temp stops: 2700..6500, nearest-stop display
    assert_eq!(temp_stop_kelvin(0), 2700);
    assert_eq!(temp_stop_kelvin(4), 6500);
    assert_eq!(temp_stop_index(3080), 1); // nearest 3000
    assert_eq!(temp_display_kelvin(temp_kelvin_to_slider(4000)), 4000);
    assert_eq!(temp_slider_to_kelvin(1000), 2700);
    assert_eq!(temp_slider_to_kelvin(10000), 6500);
    // hsv round-trip for a primary
    let (h, s, v) = hex_to_hsv("#ff0000");
    assert_eq!(h, 0.0);
    assert!((s - 1.0).abs() < 1e-9);
    assert_eq!(hsv_to_hex(h, s, v), "#ff0000");
}

#[test]
fn config_repair_and_defaults() {
    let prefs = Prefs::default();
    assert_eq!(prefs.theme, "black");
    assert_eq!(prefs.accent, "#6366f1");
    assert_eq!(prefs.density, "comfortable");
    assert!(prefs.autorefresh);
    assert_eq!(prefs.interval, 30);
    assert_eq!(prefs.gateway_type, "go2rtc");
    assert_eq!(prefs.player_cmd, "vlc");
    assert!(prefs.previews);
    let mut cams = vec![cam("dup", "A"), cam("dup", "B"), cam("", "C")];
    let layout = LayoutCfg::default();
    let mut n = 0;
    let mut gen = move || { n += 1; format!("fresh-{}", n) };
    assert!(repair_camera_ids(&mut cams, &layout, &mut gen));
    assert_eq!(cams[0].id, "dup"); // first keeps id
    assert_ne!(cams[1].id, "dup");
    assert!(!cams[2].id.is_empty());
    // obfuscation round-trip, same XOR 0x5A + base64 scheme
    assert_eq!(deobfuscate(&obfuscate("hello-secret")), "hello-secret");
    assert_ne!(obfuscate("hello-secret"), "hello-secret");
}

#[test]
fn proxy_region_and_rate_limit() {
    assert_eq!(region_base("US"), "https://px1.tuyaus.com/homeassistant/");
    assert_eq!(region_base("CN"), "https://px1.tuyacn.com/homeassistant/");
    assert_eq!(region_key_from_code("1"), "US");
    assert_eq!(region_key_from_code("86"), "CN");
    assert_eq!(region_key_from_code("44"), "EU");
    assert!(apply_proxy("https://x/", "auto", "", "CN").starts_with("https://cors.smartathome.co.uk/"));
    assert_eq!(apply_proxy("https://x/", "auto", "", "EU"), "https://x/");
    assert_eq!(apply_proxy("https://x/", "none", "", "CN"), "https://x/");
    assert!(is_rate_limit_code("FrequentlyInvoke"));
    assert!(is_rate_limit_code("TooManyRequests"));
    assert!(!is_rate_limit_code("SUCCESS"));
    assert_eq!(control_outcome(&json!({"header":{"code":"SUCCESS"}})), ControlOutcome::Ok);
    assert_eq!(control_outcome(&json!({"header":{"code":"FrequentlyInvoke"}})), ControlOutcome::RateLimited);
}

#[test]
fn device_parse_merge_and_groups() {
    // stringly-typed light readings (v1.3.0 Fix A)
    let d = parse_device(&json!({"id":"l1","name":"Strip Light","dev_type":"light","data":{"state":"true","brightness":"500","color_temp":"0","hue":"120","saturation":"0.5"}}));
    assert!(d.state);
    assert_eq!(d.brightness_pct, Some(50));
    assert_eq!(d.temp_slider, None); // 0 = placeholder, no reading
    assert!(device_is_light(&d));
    assert_eq!(d.icon_kind, "strip");
    let fresh = vec![json!({"id":"l1","data":{"brightness":0}})];
    let prev = vec![json!({"id":"l1","data":{"brightness":"500","color_temp":"3000"}})];
    let merged = merge_device_data(fresh, &prev);
    assert_eq!(merged[0]["data"]["brightness"], json!("500"));
    assert_eq!(group_master_on(&[d.clone()]), true);
    assert_eq!(group_average(&[10, 20]), Some(15.0));
    let plan = group_plan(&[d], GroupKind::Toggle, 0, None);
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].1, "turnOnOff");
}

#[test]
fn ffmpeg_and_pump_helpers() {
    assert_eq!(frame_bytes(480, 270), 518_400);
    let args = ffmpeg_args("rtsp://u:p@h:554/stream1", 480, 270);
    assert!(args.contains(&"-rtsp_transport".to_string()));
    assert!(args.contains(&"tcp".to_string()));
    assert_eq!(pump_backoff(0).as_secs(), 2);
    assert_eq!(pump_backoff(1).as_secs(), 4);
    assert_eq!(pump_backoff(9).as_secs(), 8);
    assert_eq!(ffmpeg_failure_reason("401 Unauthorized"), Some("401 Unauthorized".to_string()));
    assert_eq!(ffmpeg_failure_reason("connection refused"), Some("Connection refused".to_string()));
    assert_eq!(ffmpeg_failure_reason("something else"), None);
    assert_eq!(pump_action(None, true, false, false), PumpAction::StartFresh);
    assert!(tile_pump_wanted(true, false, false, false));
    assert!(!tile_pump_wanted(true, false, false, true));
    let cands = ffmpeg_candidates(std::path::Path::new("/app"), "", "/usr/bin:/bin");
    assert!(cands.iter().any(|p| p.to_string_lossy().contains("ffmpeg")));
}

#[test]
fn update_and_auth_parsing() {
    assert!(version_newer("1.10.0", "1.9.0"));
    assert!(!version_newer("1.4.1", "1.5.0"));
    assert_eq!(expected_update_asset_name_for("1.5.0", "macos"), "SmartLifeTuyaDesktop-v1.5.0.dmg");
    assert_eq!(expected_update_asset_name_for("1.5.0", "windows"), "SmartLifeTuyaDesktop-Setup-v1.5.0.exe");
    assert_eq!(update_install_for("macos"), UpdateInstall::OpenDiskImage);
    let s = parse_auth_response(&json!({"access_token":"EUabc","refresh_token":"r","expires_in":3600}), "EU").unwrap();
    assert_eq!(s.region, "EU");
    assert!(parse_auth_response(&json!({"msg":"bad login"}), "EU").is_err());
    // launch agent round-trip
    let plist = launch_agent_plist("\"/Applications/App\" --minimized").unwrap();
    assert_eq!(launch_agent_command(&plist).unwrap(), "\"/Applications/App\" --minimized");
    assert_eq!(parse_autostart_command("\"/x/App\" --maximized").unwrap().1, LaunchMode::Maximized);
}

#[test]
fn tray_queue_and_snap() {
    push_tray_action(TrayAction::Show);
    push_tray_action(TrayAction::Quit);
    let drained = drain_tray_actions();
    assert_eq!(drained, vec![TrayAction::Show, TrayAction::Quit]);
    assert!(drain_tray_actions().is_empty());
    assert_eq!(tray_action_for_icon_parts(false, true, true), Some(TrayAction::Show));
    assert_eq!(tray_action_for_icon_parts(false, true, false), None);
    assert_eq!(close_action(true, false), CloseAction::HideToTray);
    assert_eq!(close_action(true, true), CloseAction::Quit);
    let (phase, action) = snap_next(SnapPhase::Verify, SnapFacts { fullscreen: false, zoomed: false, on_target: true, expired: false }, 0);
    assert_eq!((phase, action), (SnapPhase::Verify, SnapAction::MaximizeAndFinish));
    let m = MonitorInfo { name: "Main".into(), device: "DISPLAY1".into(), handle: 1, rect_px: [0, 0, 2560, 1440], scale: 1.0, primary: true };
    assert_eq!(monitor_orientation(&m), "Landscape");
    assert_eq!(snap_target_point_px(&m, [1120, 780]), [60, 60]);
}

#[test]
fn config_path_platform_shape() {
    let p = config_path();
    let s = p.to_string_lossy();
    assert!(s.ends_with("config.json"));
    assert!(s.contains("SmartLifeTuyaDesktop"));
}

#[test]
fn ffi_smoke() {
    use std::ffi::{CStr, CString};
    use tuya_core::ffi::*;
    unsafe {
        let v = tuya_version();
        assert_eq!(CStr::from_ptr(v).to_str().unwrap(), "1.5.0-rust");
        tuya_free_string(v);
        // build_sections via FFI
        let devices = CString::new(r#"[{"id":"dev-a","name":"Bulb","dev_type":"light","data":{"state":true,"brightness":"800"}},{"id":"cam-9","name":"Porch","user":"admin","password":"KTo=","host":"10.0.0.5","port":554,"template":"tapo1","path":""}]"#).unwrap();
        // password "KTo=" is obfuscate("pw")? verify via core: obfuscate("pw") - use it dynamically instead
        let _ = devices;
        let dev_json = format!(r#"[{{"id":"dev-a","name":"Bulb","dev_type":"light","data":{{"state":true,"brightness":"800"}}}},{{"id":"cam-9","name":"Porch","user":"admin","password":"{}","host":"10.0.0.5","port":554,"template":"tapo1","path":""}}]"#, tuya_core::obfuscate("pw"));
        let devices = CString::new(dev_json).unwrap();
        let layout = CString::new(r#"{"pins":["dev-a"]}"#).unwrap();
        let out = tuya_build_sections(devices.as_ptr(), layout.as_ptr());
        let s = CStr::from_ptr(out).to_str().unwrap().to_string();
        tuya_free_string(out);
        let val: serde_json::Value = serde_json::from_str(&s).unwrap();
        let sections = val["sections"].as_array().unwrap();
        assert!(sections.iter().any(|s| s["order_key"] == "pinned"));
        assert!(sections.iter().any(|s| s["order_key"] == "cameras"));
        // tray actions drained via FFI
        tuya_core::push_tray_action(tuya_core::TrayAction::Show);
        let out = tuya_tray_actions();
        let s = CStr::from_ptr(out).to_str().unwrap().to_string();
        tuya_free_string(out);
        assert_eq!(s, r#"{"actions":["show"]}"#);
        // monitors (Linux: empty, faithful to original non-Windows stub)
        let out = tuya_monitors();
        let s = CStr::from_ptr(out).to_str().unwrap().to_string();
        tuya_free_string(out);
        assert!(s.contains("\"monitors\""));
        // find_ffmpeg with a bogus override returns null path, not an error
        let ov = CString::new(r#"{"ffmpeg_path":"/nonexistent/ffmpeg-xyz"}"#).unwrap();
        let out = tuya_find_ffmpeg(ov.as_ptr());
        let s = CStr::from_ptr(out).to_str().unwrap().to_string();
        tuya_free_string(out);
        assert!(s.contains("\"path\""));
        // malformed command JSON returns a JSON error without any network call
        let dev = CString::new("dev-a").unwrap();
        let cmd = CString::new(r#"not json"#).unwrap();
        let out = tuya_control(dev.as_ptr(), cmd.as_ptr());
        let s = CStr::from_ptr(out).to_str().unwrap().to_string();
        tuya_free_string(out);
        assert!(s.contains("error"));
    }
}
