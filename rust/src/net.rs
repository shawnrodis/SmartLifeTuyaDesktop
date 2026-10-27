use crate::config::{Creds, Prefs, Session};
use crate::device::merge_device_data;
use crate::obfuscate::now_ms;
use crate::region::{apply_proxy, region_base, region_from_token, region_key_from_code, url_encode};
use serde_json::{json, Map, Value};
use std::thread;
use std::time::Duration;
pub fn http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

pub fn parse_auth_response(json: &Value, fallback_region: &str) -> Result<Session, String> {
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

pub fn net_auth(client: &reqwest::blocking::Client, creds: &Creds, plain_password: &str, prefs: &Prefs) -> Result<Session, String> {
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

pub fn net_refresh(client: &reqwest::blocking::Client, session: &Session, prefs: &Prefs) -> Result<Session, String> {
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

pub fn rand_f64() -> f64 {
    // Tiny xorshift — only used as a cache-busting "rand" parameter.
    let mut x = (now_ms() as u64) ^ 0x9E3779B97F4A7C15;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x % 1_000_000) as f64 / 1_000_000.0
}

/// Refresh or silently re-login if the token is near expiry (mirrors the
/// web edition's ensureSession, but done inside the worker thread).
pub fn fresh_session(
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

pub fn skill_body(name: &str, payload: Value) -> Value {
    json!({ "header": { "name": name, "namespace": if name == "Discovery" { "discovery" } else { "control" }, "payloadVersion": 1 }, "payload": payload })
}

pub fn net_skill(
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

pub fn net_discovery(
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

pub enum DiscoveryFail {
    Frequent,
    Msg(String),
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
pub fn is_rate_limit_code(code: &str) -> bool {
    code.eq_ignore_ascii_case("FrequentlyInvoke")
        || code.eq_ignore_ascii_case("TooManyRequests")
        || code.to_ascii_lowercase().contains("frequent")
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ControlOutcome {
    Ok,
    RateLimited,
    Failed,
}

/// Classify a control response the way the control paths need.
pub fn control_outcome(json: &Value) -> ControlOutcome {
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
pub fn net_control_once(
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
pub fn net_control(
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
