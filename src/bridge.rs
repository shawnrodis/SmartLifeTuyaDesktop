// ---------------------------------------------------------------------------
// Localhost bridge: lets the single-file web edition control the
// thermostat through the desktop app.
//
// A browser page cannot speak HomeKit (no raw TCP, no mDNS, no
// SRP/ChaCha20 sessions), so the desktop app serves a tiny HTTP API
// on loopback only, and the web page calls it. This module is ONLY
// the transport:
//
//   * It binds 127.0.0.1 (never 0.0.0.0) and refuses any peer that is
//     not loopback, so nothing off this computer can reach it.
//   * It exposes thermostat endpoints ONLY. It never serves Tuya
//     credentials, config contents, or any other app state — it does
//     not even know they exist. Status/discover/set/forget requests
//     are forwarded to the UI thread over a channel and answered
//     through the SAME spawn/report machinery the desktop tile uses
//     (one pairing store in cfg.thermostat, one busy flag, one
//     result channel); only pairing itself runs on the bridge's own
//     threads (see the two-step note below), with the finished
//     pairing handed to the UI thread to persist.
//   * It runs on its own thread and never blocks the UI thread; the
//     UI thread only drains the request channel once per frame.
//   * It runs always, paired or not: discovery and pairing from the
//     web page need it before any pairing exists.
//   * Two-step pairing (rotating-code thermostats): POST
//     /thermostat/pair/begin opens a session (M1/M2, no code needed)
//     and returns a pending_id; POST /thermostat/pair/finish sends
//     the code the thermostat is showing (M3..M6) and, on success,
//     hands the finished pairing to the UI thread to persist;
//     POST /thermostat/pair/cancel drops the session. The sessions
//     live only in the bridge's PairSessions map — never in config,
//     never on the UI thread. The setup code (pin) is carried in
//     memory for one finish request only. It is never written to
//     config, never logged, never echoed in a response, and never
//     part of a pending session (the code arrives only at finish).
//     BridgeRequest deliberately has NO Debug impl, so a stray log
//     can never print a pin.
//
// CORS: every response carries Access-Control-Allow-Origin: * plus
// the Private-Network header, and OPTIONS is answered 204, so the
// page served from GitHub Pages (https) can call http://localhost
// (localhost is a browser-trustworthy origin — the standard
// companion-app pattern). Whether a given browser actually permits
// the https-page-to-localhost fetch is browser behaviour the field
// test must confirm; see the report notes.
// ---------------------------------------------------------------------------

use crate::hap::{DiscoveredDevice, PendingPairSetup, ThermostatState};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Loopback only — never 0.0.0.0. The port is fixed so the web page
/// knows where to look; it is not shown in user-facing copy.
pub const BRIDGE_BIND_ADDR: &str = "127.0.0.1";
pub const BRIDGE_PORT: u16 = 18780;

/// Largest request body accepted (a pair/set JSON body is < 1 KiB;
/// anything bigger is not one of our requests).
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;

/// How long an HTTP handler waits for the UI thread to answer. HAP
/// discovery alone takes ~8 s and pairing can take longer; 90 s
/// covers the retries while still bounding a hung request.
const REPLY_TIMEOUT: Duration = Duration::from_secs(90);

// ---------------------------------------------------------------------------
// Wire types: what crosses the channel to the UI thread.
// ---------------------------------------------------------------------------

/// Answer to one bridge request: an HTTP status plus a JSON body.
pub struct BridgeResponse {
    pub status: u16,
    pub body: Value,
}

/// A validated set request. Temperatures are Fahrenheit here (the UI
/// boundary); main.rs converts to Celsius for hap::ThermostatChanges,
/// exactly like the desktop tile does.
#[derive(Clone, Debug, Default)]
pub struct SetRequest {
    pub mode: Option<u8>,
    pub target_f: Option<f64>,
    pub heat_to_f: Option<f64>,
    pub cool_to_f: Option<f64>,
    pub fan_on: Option<bool>,
}

/// One thermostat operation for the UI thread. NOTE: no Debug impl
/// on purpose — PairFinish carries the setup code, and it must never
/// be printable by a debug log. PairSave carries only the derived
/// pairing (no code ever reaches it: the bridge hands the UI thread
/// the finished pairing, never the pin).
pub enum BridgeRequest {
    Status,
    Discover,
    /// A pairing finished on the bridge's own threads, for the UI
    /// thread to persist in config like a desktop pairing success.
    PairSave {
        pairing: crate::hap::ThermostatPairing,
    },
    Set(SetRequest),
    Forget,
}

/// Open pair-setup sessions for the web edition, keyed by fresh
/// random ids. Lives only in memory on the bridge threads: a session
/// holds a live socket to the thermostat, so it can never cross to
/// the UI thread or survive a restart. Finish and cancel remove the
/// entry; at most one session exists at a time (a second begin gets
/// 409), so two pages cannot interleave pair-setup on one unit.
pub struct PairSessions {
    inner: Mutex<HashMap<String, PendingPairSetup>>,
}

impl PairSessions {
    pub fn new() -> Self {
        PairSessions {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().map(|m| m.is_empty()).unwrap_or(false)
    }

    /// Store a session if none is open; None means one already is
    /// (the caller answers 409). The check and the insert share one
    /// lock, so two simultaneous begins cannot both succeed.
    pub fn insert_if_empty(&self, pending: PendingPairSetup) -> Option<String> {
        let mut map = self.inner.lock().ok()?;
        if !map.is_empty() {
            return None;
        }
        let mut id = fresh_pending_id();
        while map.contains_key(&id) {
            id = fresh_pending_id();
        }
        map.insert(id.clone(), pending);
        Some(id)
    }

    /// Remove and return a session, if it exists. Dropping the
    /// returned value closes its socket (cancel = take + drop).
    pub fn take(&self, pending_id: &str) -> Option<PendingPairSetup> {
        self.inner.lock().ok()?.remove(pending_id)
    }
}

fn fresh_pending_id() -> String {
    let bytes: [u8; 16] = rand::random();
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// A request plus the channel its answer goes back on.
pub struct BridgeJob {
    pub request: BridgeRequest,
    pub reply: Sender<BridgeResponse>,
}

// ---------------------------------------------------------------------------
// JSON shapes shared with the web edition.
// ---------------------------------------------------------------------------

/// Thermostat state in UI units (Fahrenheit, mode names), mirroring
/// the desktop tile's display: current mode + indoor temp, target
/// mode + setpoint(s), humidity and fan only when the accessory
/// exposes them (null otherwise).
pub fn state_to_json(st: &ThermostatState) -> Value {
    // Round to 1 decimal so float noise (e.g. 73.4000000001) never
    // reaches the page; the page rounds again for display.
    let f1 = |c: f64| (crate::hap::c_to_f(c) * 10.0).round() / 10.0;
    json!({
        "name": st.name,
        "current_mode": crate::hap::mode_name(st.current_mode),
        "mode": crate::hap::mode_name(st.target_mode),
        "temp_f": f1(st.current_temp_c),
        "target_f": f1(st.target_temp_c),
        "heat_to_f": st.heat_to_c.map(f1),
        "cool_to_f": st.cool_to_c.map(f1),
        "humidity": st.humidity,
        "fan_on": st.fan_on,
    })
}

/// Discovery list for the pairing picker. `paired` is the inverse of
/// the mDNS unpaired flag: true means the device already has a
/// controller pairing (possibly another controller's).
pub fn devices_to_json(devs: &[DiscoveredDevice]) -> Value {
    json!({
        "devices": devs
            .iter()
            .map(|d| json!({
                "id": d.id,
                "name": d.name,
                "model": d.model,
                "paired": !d.unpaired,
            }))
            .collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------------------
// HTTP layer: pure request -> reply given a backend. This is the
// unit-testable seam: tests drive it with a stub backend, no socket.
// ---------------------------------------------------------------------------

/// One HTTP reply: status + already-serialized JSON body (empty for
/// 204). CORS headers are added by the socket writer, not here.
pub struct HttpReply {
    pub status: u16,
    pub body: String,
}

fn err_reply(status: u16, msg: &str) -> HttpReply {
    HttpReply {
        status,
        body: serde_json::to_string(&json!({ "error": msg }))
            .unwrap_or_else(|_| "{\"error\":\"internal error\"}".to_string()),
    }
}

fn backend_reply(resp: BridgeResponse) -> HttpReply {
    HttpReply {
        status: resp.status,
        body: serde_json::to_string(&resp.body)
            .unwrap_or_else(|_| "{\"error\":\"internal error\"}".to_string()),
    }
}

/// CORS headers every response carries, including the Private-Network
/// header browsers require for public-page-to-localhost fetches.
fn cors_header_block() -> String {
    [
        "Access-Control-Allow-Origin: *",
        "Access-Control-Allow-Methods: GET, POST, OPTIONS",
        "Access-Control-Allow-Headers: Content-Type",
        "Access-Control-Allow-Private-Network: true",
    ]
    .join("\r\n")
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Content Too Large",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Unknown",
    }
}

fn parse_begin_body(body: &[u8]) -> Result<String, &'static str> {
    let v: Value = serde_json::from_slice(body).map_err(|_| "malformed JSON body")?;
    v.get("device_id")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
        .ok_or("pair/begin needs a device_id string")
}

fn parse_finish_body(body: &[u8]) -> Result<(String, String), &'static str> {
    let v: Value = serde_json::from_slice(body).map_err(|_| "malformed JSON body")?;
    let pending_id = v
        .get("pending_id")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("pair/finish needs a pending_id string")?;
    let pin = v
        .get("pin")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("pair/finish needs a pin string")?;
    Ok((pending_id.to_string(), pin.to_string()))
}

fn parse_cancel_body(body: &[u8]) -> Result<String, &'static str> {
    let v: Value = serde_json::from_slice(body).map_err(|_| "malformed JSON body")?;
    v.get("pending_id")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
        .ok_or("pair/cancel needs a pending_id string")
}

/// Resolve a device id to a live address (fresh discovery: the
/// page's device list carries no addresses, and the accessory must
/// be advertising right now for pairing to work at all), open the
/// pair-setup session, and store it. Returns the pending id.
fn begin_session(sessions: &PairSessions, device_id: &str) -> Result<String, (u16, String)> {
    let devs = crate::hap::discover(Duration::from_secs(8))
        .map_err(|e| (502, format!("discovery failed: {e}")))?;
    let dev = devs
        .iter()
        .find(|d| d.id == device_id)
        .ok_or_else(|| {
            (
                404,
                "device not found — press Discover again, then pair".to_string(),
            )
        })?;
    let pending = crate::hap::begin_pair_setup(dev)
        .map_err(|e| (502, format!("could not start pairing: {e}")))?;
    sessions.insert_if_empty(pending).ok_or((
        409,
        "a pairing is already in progress — finish or cancel it first".to_string(),
    ))
}

fn opt_temp(obj: &Map<String, Value>, key: &str) -> Result<Option<f64>, &'static str> {
    match obj.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_f64()
            .map(Some)
            .ok_or("temperature fields must be numbers"),
    }
}

fn parse_set_body(body: &[u8]) -> Result<BridgeRequest, &'static str> {
    let v: Value = serde_json::from_slice(body).map_err(|_| "malformed JSON body")?;
    let obj = v.as_object().ok_or("set body must be a JSON object")?;
    let mut req = SetRequest::default();
    if let Some(m) = obj.get("mode") {
        let lower = m
            .as_str()
            .ok_or("mode must be a string")?
            .to_ascii_lowercase();
        req.mode = Some(match lower.as_str() {
            "off" => 0,
            "heat" => 1,
            "cool" => 2,
            "auto" => 3,
            _ => return Err("unknown mode (use off, heat, cool, or auto)"),
        });
    }
    if let Some(f) = obj.get("fan") {
        let lower = f
            .as_str()
            .ok_or("fan must be a string")?
            .to_ascii_lowercase();
        req.fan_on = Some(match lower.as_str() {
            "on" => true,
            "auto" => false,
            _ => return Err("unknown fan value (use on or auto)"),
        });
    }
    req.target_f = opt_temp(obj, "target_f")?;
    req.heat_to_f = opt_temp(obj, "heat_to_f")?;
    req.cool_to_f = opt_temp(obj, "cool_to_f")?;
    if req.mode.is_none()
        && req.target_f.is_none()
        && req.heat_to_f.is_none()
        && req.cool_to_f.is_none()
        && req.fan_on.is_none()
    {
        return Err("nothing to set");
    }
    Ok(BridgeRequest::Set(req))
}

/// Route one request. Never panics on hostile input: every parse
/// failure becomes a 400, every unknown path a 404. Bodies are never
/// echoed back (a finish body contains the setup code). Pairing runs
/// on the bridge's own threads through `sessions`; only the finished
/// pairing crosses to the UI thread (PairSave), via `backend`.
pub fn handle_http(
    method: &str,
    raw_path: &str,
    body: &[u8],
    backend: &dyn Fn(BridgeRequest) -> BridgeResponse,
    sessions: &PairSessions,
) -> HttpReply {
    // Browsers preflight cross-origin POSTs (and private-network
    // fetches) with OPTIONS; answer it before any routing.
    if method == "OPTIONS" {
        return HttpReply {
            status: 204,
            body: String::new(),
        };
    }
    let path = raw_path.split('?').next().unwrap_or(raw_path);
    match (method, path) {
        ("GET", "/thermostat/status") => backend_reply(backend(BridgeRequest::Status)),
        ("GET", "/thermostat/discover") => backend_reply(backend(BridgeRequest::Discover)),
        ("POST", "/thermostat/pair/begin") => match parse_begin_body(body) {
            Err(m) => err_reply(400, m),
            Ok(device_id) => {
                if !sessions.is_empty() {
                    err_reply(
                        409,
                        "a pairing is already in progress — finish or cancel it first",
                    )
                } else {
                    match begin_session(sessions, &device_id) {
                        Ok(pending_id) => HttpReply {
                            status: 200,
                            body: json!({ "pending_id": pending_id }).to_string(),
                        },
                        Err((status, msg)) => err_reply(status, &msg),
                    }
                }
            }
        },
        ("POST", "/thermostat/pair/finish") => match parse_finish_body(body) {
            Err(m) => err_reply(400, m),
            Ok((pending_id, pin)) => {
                // A malformed code must not consume the session:
                // the user can retype without starting over.
                if crate::hap::normalize_setup_code(&pin).is_err() {
                    err_reply(400, "pin must be 8 digits (e.g. 123-45-678)")
                } else {
                    match sessions.take(&pending_id) {
                        None => err_reply(
                            404,
                            "no such pairing session — press Start pairing first",
                        ),
                        Some(pending) => {
                            match crate::hap::finish_pair_setup(pending, &pin) {
                                Ok(pairing) => {
                                    backend_reply(backend(BridgeRequest::PairSave { pairing }))
                                }
                                Err(e) => {
                                    // The session is spent either
                                    // way (the SRP state is gone
                                    // after a failed M3), so the
                                    // error tells the page to start
                                    // over with a fresh code.
                                    err_reply(502, &format!("pairing failed: {e}"))
                                }
                            }
                        }
                    }
                }
            }
        },
        ("POST", "/thermostat/pair/cancel") => match parse_cancel_body(body) {
            Err(m) => err_reply(400, m),
            Ok(pending_id) => match sessions.take(&pending_id) {
                // Dropping the taken session closes its socket.
                Some(_) => HttpReply {
                    status: 200,
                    body: json!({ "ok": true }).to_string(),
                },
                None => err_reply(404, "no such pairing session"),
            },
        },
        ("POST", "/thermostat/set") => match parse_set_body(body) {
            Ok(req) => backend_reply(backend(req)),
            Err(m) => err_reply(400, m),
        },
        ("POST", "/thermostat/forget") => backend_reply(backend(BridgeRequest::Forget)),
        _ => {
            let known = matches!(
                path,
                "/thermostat/status"
                    | "/thermostat/discover"
                    | "/thermostat/pair/begin"
                    | "/thermostat/pair/finish"
                    | "/thermostat/pair/cancel"
                    | "/thermostat/set"
                    | "/thermostat/forget"
            );
            if known {
                err_reply(405, "method not allowed")
            } else {
                err_reply(404, "not found")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Socket layer: loopback listener, one thread per connection.
// ---------------------------------------------------------------------------

/// The loopback gate, applied to every accepted connection even
/// though a 127.0.0.1 listener only ever sees loopback peers: a
/// future bind change must not silently widen this.
pub fn is_loopback_peer(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Start the bridge thread. Never blocks the caller; a bind failure
/// (port already in use) only disables web-edition control — the
/// desktop tile keeps working — and is reported on stderr.
/// Pairing sessions live in the map created here, shared by every
/// connection thread (see PairSessions).
pub fn start_bridge(tx: Sender<BridgeJob>) {
    let sessions = Arc::new(PairSessions::new());
    let spawn = thread::Builder::new()
        .name("tstat-bridge".to_string())
        .spawn(move || serve(tx, sessions));
    if let Err(e) = spawn {
        eprintln!("thermostat bridge: could not start thread: {e}");
    }
}

fn serve(tx: Sender<BridgeJob>, sessions: Arc<PairSessions>) {
    let listener = match TcpListener::bind((BRIDGE_BIND_ADDR, BRIDGE_PORT)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "thermostat bridge: could not listen on {BRIDGE_BIND_ADDR}:{BRIDGE_PORT}: {e} \
                 (web-edition thermostat control unavailable; the desktop tile still works)"
            );
            return;
        }
    };
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let tx = tx.clone();
                let sessions = sessions.clone();
                thread::spawn(move || handle_connection(s, tx, sessions));
            }
            Err(_) => continue,
        }
    }
}

fn handle_connection(stream: TcpStream, tx: Sender<BridgeJob>, sessions: Arc<PairSessions>) {
    let mut stream = stream;
    let reply = process_connection(&mut stream, &tx, &sessions);
    let _ = write_response(&mut stream, &reply);
    let _ = stream.shutdown(Shutdown::Both);
}

fn process_connection(
    stream: &mut TcpStream,
    tx: &Sender<BridgeJob>,
    sessions: &PairSessions,
) -> HttpReply {
    match stream.peer_addr() {
        Ok(a) if is_loopback_peer(&a) => {}
        _ => {
            return HttpReply {
                status: 403,
                body: "{\"error\":\"forbidden: loopback only\"}".to_string(),
            }
        }
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let (method, path, body) = match read_request(stream) {
        Ok(t) => t,
        Err(m) => return err_reply(400, m),
    };
    let backend = |req: BridgeRequest| -> BridgeResponse {
        let (reply_tx, reply_rx) = channel::<BridgeResponse>();
        let sent = tx.send(BridgeJob {
            request: req,
            reply: reply_tx,
        });
        if sent.is_err() {
            return BridgeResponse {
                status: 503,
                body: json!({ "error": "desktop app is shutting down" }),
            };
        }
        match reply_rx.recv_timeout(REPLY_TIMEOUT) {
            Ok(r) => r,
            Err(_) => BridgeResponse {
                status: 504,
                body: json!({ "error": "the desktop app did not answer in time — try again" }),
            },
        }
    };
    handle_http(&method, &path, &body, &backend, sessions)
}

fn read_request(stream: &mut TcpStream) -> Result<(String, String, Vec<u8>), &'static str> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if buf.len() > MAX_HEADER_BYTES {
            return Err("request headers too large");
        }
        match buf.windows(4).position(|w| w == b"\r\n\r\n") {
            Some(p) => break p + 4,
            None => match stream.read(&mut chunk) {
                Ok(0) => return Err("connection closed before request"),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => return Err("could not read request"),
            },
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.lines();
    let request_line = lines.next().ok_or("empty request")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("missing method")?.to_string();
    let path = parts.next().ok_or("missing path")?.to_string();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| "bad Content-Length")?;
                if content_length > MAX_BODY_BYTES {
                    return Err("request body too large");
                }
            }
        }
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        if body.len() > MAX_BODY_BYTES {
            return Err("request body too large");
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err("connection closed mid-body"),
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(_) => return Err("could not read request"),
        }
    }
    body.truncate(content_length);
    Ok((method, path, body))
}

fn write_response(stream: &mut TcpStream, reply: &HttpReply) -> std::io::Result<()> {
    let mut out = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}\r\nConnection: close\r\n\r\n",
        reply.status,
        reason_phrase(reply.status),
        reply.body.len(),
        cors_header_block()
    );
    out.push_str(&reply.body);
    stream.write_all(out.as_bytes())
}

// ---------------------------------------------------------------------------
// Unit tests (no hardware, no thermostat needed). The HTTP layer is
// driven through handle_http with stub backends; the socket layer
// gets one live loopback round-trip for CORS + 400 behaviour.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;
    use std::net::TcpListener;

    fn stub(status: u16, body: Value) -> impl Fn(BridgeRequest) -> BridgeResponse {
        move |_| BridgeResponse {
            status,
            body: body.clone(),
        }
    }

    #[test]
    fn bind_address_is_loopback_only() {
        // The "refused non-loopback bind" check at the level a test
        // can prove: the server binds this address and no other.
        let ip: std::net::IpAddr = BRIDGE_BIND_ADDR.parse().unwrap();
        assert!(ip.is_loopback());
        assert_eq!(BRIDGE_PORT, 18780);
    }

    #[test]
    fn loopback_peer_predicate() {
        assert!(is_loopback_peer(&"127.0.0.1:1".parse().unwrap()));
        assert!(is_loopback_peer(&"[::1]:1".parse().unwrap()));
        assert!(!is_loopback_peer(&"192.168.1.5:1".parse().unwrap()));
        assert!(!is_loopback_peer(&"8.8.8.8:1".parse().unwrap()));
    }

    #[test]
    fn unpaired_status_passes_through() {
        let r = handle_http(
            "GET",
            "/thermostat/status",
            b"",
            &stub(200, json!({ "paired": false })),
            &PairSessions::new(),
        );
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "{\"paired\":false}");
    }

    #[test]
    fn options_answers_204_and_cors_covers_private_network() {
        let r = handle_http(
            "OPTIONS",
            "/thermostat/status",
            b"",
            &stub(200, json!({})),
            &PairSessions::new(),
        );
        assert_eq!(r.status, 204);
        assert_eq!(r.body, "");
        let cors = cors_header_block();
        assert!(cors.contains("Access-Control-Allow-Origin: *"));
        assert!(cors.contains("Access-Control-Allow-Headers: Content-Type"));
        assert!(cors.contains("Access-Control-Allow-Private-Network: true"));
    }

    #[test]
    fn malformed_json_is_400_never_panic() {
        let sessions = PairSessions::new();
        // Begin: garbage, non-object, missing device_id, empty id.
        for body in [
            &b"{not json"[..],
            b"[]",
            b"{}",
            b"{\"device_id\":\"\"}",
        ] {
            let r = handle_http(
                "POST",
                "/thermostat/pair/begin",
                body,
                &stub(200, json!({})),
                &sessions,
            );
            assert_eq!(r.status, 400, "body {:?}", String::from_utf8_lossy(body));
        }
        // Finish: garbage, missing pin, missing pending_id, empty id.
        // One body carries a real-looking code: it must never be
        // echoed back.
        for body in [
            &b"{not json"[..],
            b"{\"pending_id\":\"x\"}",
            b"{\"pin\":\"123-45-678\"}",
            b"{\"pending_id\":\"\",\"pin\":\"123-45-678\"}",
        ] {
            let r = handle_http(
                "POST",
                "/thermostat/pair/finish",
                body,
                &stub(200, json!({})),
                &sessions,
            );
            assert_eq!(r.status, 400, "body {:?}", String::from_utf8_lossy(body));
            assert!(!r.body.contains("123-45-678"));
        }
        // Cancel: garbage, missing pending_id.
        for body in [&b"{not json"[..], b"{}"] {
            let r = handle_http(
                "POST",
                "/thermostat/pair/cancel",
                body,
                &stub(200, json!({})),
                &sessions,
            );
            assert_eq!(r.status, 400, "body {:?}", String::from_utf8_lossy(body));
        }
        // Set: garbage, non-object, unknown mode, unknown fan,
        // non-numeric temp, and an empty set.
        for body in [
            &b"{oops"[..],
            b"[]",
            b"{\"mode\":\"banana\"}",
            b"{\"fan\":\"sometimes\"}",
            b"{\"target_f\":\"warm\"}",
            b"{}",
        ] {
            let r = handle_http(
                "POST",
                "/thermostat/set",
                body,
                &stub(200, json!({})),
                &sessions,
            );
            assert_eq!(r.status, 400, "body {:?}", String::from_utf8_lossy(body));
        }
    }

    #[test]
    fn unknown_path_404_wrong_method_405() {
        let sessions = PairSessions::new();
        let r = handle_http("GET", "/nope", b"", &stub(200, json!({})), &sessions);
        assert_eq!(r.status, 404);
        let r = handle_http(
            "POST",
            "/thermostat/status",
            b"",
            &stub(200, json!({})),
            &sessions,
        );
        assert_eq!(r.status, 405);
        let r = handle_http(
            "GET",
            "/thermostat/pair/begin",
            b"",
            &stub(200, json!({})),
            &sessions,
        );
        assert_eq!(r.status, 405);
        // The old single-shot pair route is gone, not kept as an
        // alias: a stale client gets a plain 404.
        let r = handle_http(
            "POST",
            "/thermostat/pair",
            b"{\"device_id\":\"x\",\"pin\":\"123-45-678\"}",
            &stub(200, json!({})),
            &sessions,
        );
        assert_eq!(r.status, 404);
        assert!(!r.body.contains("123-45-678"));
        // Query strings do not break routing.
        let r = handle_http(
            "GET",
            "/thermostat/status?x=1",
            b"",
            &stub(200, json!({ "paired": false })),
            &sessions,
        );
        assert_eq!(r.status, 200);
    }

    #[test]
    fn set_body_parses_modes_fan_and_temps() {
        let req = parse_set_body(b"{\"mode\":\"Cool\",\"target_f\":74,\"fan\":\"On\"}").unwrap();
        match req {
            BridgeRequest::Set(s) => {
                assert_eq!(s.mode, Some(2));
                assert_eq!(s.target_f, Some(74.0));
                assert_eq!(s.fan_on, Some(true));
                assert_eq!(s.heat_to_f, None);
            }
            _ => panic!("expected Set"),
        }
        let req = parse_set_body(b"{\"heat_to_f\":68,\"cool_to_f\":78}").unwrap();
        match req {
            BridgeRequest::Set(s) => {
                assert_eq!(s.mode, None);
                assert_eq!(s.heat_to_f, Some(68.0));
                assert_eq!(s.cool_to_f, Some(78.0));
            }
            _ => panic!("expected Set"),
        }
    }

    #[test]
    fn state_json_uses_fahrenheit_and_mode_names() {
        let st = ThermostatState {
            name: "Thermostat".to_string(),
            current_mode: 2,
            target_mode: 2,
            current_temp_c: 22.5,
            target_temp_c: 23.0,
            heat_to_c: Some(20.0),
            cool_to_c: None,
            humidity: Some(45.0),
            fan_on: Some(true),
            ..Default::default()
        };
        let j = state_to_json(&st);
        assert_eq!(j["temp_f"], 72.5);
        assert_eq!(j["target_f"], 73.4);
        assert_eq!(j["heat_to_f"], 68.0);
        assert!(j["cool_to_f"].is_null());
        assert_eq!(j["humidity"], 45.0);
        assert_eq!(j["fan_on"], true);
        assert_eq!(j["mode"], "Cool");
        assert_eq!(j["current_mode"], "Cool");
    }

    #[test]
    fn live_loopback_roundtrip_cors_and_400() {
        // Real sockets, stub backend whose receiver is dropped (the
        // two requests below fail before ever reaching it: OPTIONS
        // short-circuits, malformed JSON 400s).
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = channel::<BridgeJob>();
        drop(rx);
        let sessions = Arc::new(PairSessions::new());
        let server = thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                if let Ok(s) = stream {
                    handle_connection(s, tx.clone(), sessions.clone());
                }
            }
        });
        let mut roundtrip = |req: &str| -> String {
            let mut sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
            sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            sock.write_all(req.as_bytes()).unwrap();
            let mut out = String::new();
            sock.read_to_string(&mut out).unwrap();
            out
        };
        let preflight = roundtrip("OPTIONS /thermostat/status HTTP/1.1\r\nHost: x\r\n\r\n");
        assert!(preflight.contains("204"), "preflight: {preflight}");
        assert!(preflight.contains("Access-Control-Allow-Origin: *"), "preflight: {preflight}");
        assert!(
            preflight.contains("Access-Control-Allow-Private-Network: true"),
            "preflight: {preflight}"
        );
        let bad = roundtrip(
            "POST /thermostat/pair/begin HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\n\r\n{bad",
        );
        assert!(bad.contains("400"), "bad begin: {bad}");
        server.join().unwrap();
    }

    // -- Two-step pairing session tests ------------------------------
    // A minimal M1/M2-only accessory: answers one pair-setup M1 with
    // a fixed M2 (salt + public key), then waits for the client to
    // close. Begin only stores M2's values — no SRP math runs until
    // finish, which these tests never reach — so the values need not
    // be a real server key.

    fn spawn_m1m2_server() -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let head_end = loop {
                match buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    Some(p) => break p + 4,
                    None => match s.read(&mut chunk) {
                        Ok(0) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        Err(_) => return,
                    },
                }
            };
            let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
            let mut content_len = 0usize;
            for line in head.lines().skip(1) {
                if let Some((k, v)) = line.split_once(':') {
                    if k.trim().eq_ignore_ascii_case("content-length") {
                        content_len = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            while buf.len() < head_end + content_len {
                match s.read(&mut chunk) {
                    Ok(0) => return,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(_) => return,
                }
            }
            // Tag 6 = State (M2), 2 = Salt, 3 = PublicKey.
            let body = crate::hap::tlv_encode(&[
                (6, &[2u8][..]),
                (2, &[0xabu8; 16][..]),
                (3, &[0xcdu8; 384][..]),
            ]);
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            let _ = s.write_all(resp.as_bytes());
            let _ = s.write_all(&body);
            // Wait for the client to close (cancel drops the pending)
            // or time out; either way exit without panicking.
            loop {
                match s.read(&mut chunk) {
                    Ok(0) => return,
                    Ok(_) => {}
                    Err(_) => return,
                }
            }
        });
        (port, handle)
    }

    fn m1m2_device(port: u16) -> DiscoveredDevice {
        DiscoveredDevice {
            id: "test-id".to_string(),
            name: "Test T6".to_string(),
            model: "T6".to_string(),
            addresses: vec!["127.0.0.1".to_string()],
            port,
            ..Default::default()
        }
    }

    #[test]
    fn pair_sessions_second_begin_409_and_finish_after_cancel_404() {
        let sessions = PairSessions::new();
        let (port1, server1) = spawn_m1m2_server();
        let pending = crate::hap::begin_pair_setup(&m1m2_device(port1)).unwrap();
        let id = sessions.insert_if_empty(pending).expect("first insert");
        // A second session cannot open while one is pending: the
        // map-level insert refuses, and the HTTP layer answers 409
        // before any discovery runs.
        let (port2, server2) = spawn_m1m2_server();
        let pending2 = crate::hap::begin_pair_setup(&m1m2_device(port2)).unwrap();
        assert!(sessions.insert_if_empty(pending2).is_none());
        let r = handle_http(
            "POST",
            "/thermostat/pair/begin",
            b"{\"device_id\":\"x\"}",
            &stub(200, json!({})),
            &sessions,
        );
        assert_eq!(r.status, 409);
        // Unknown finish/cancel: 404, no panic, pin never echoed.
        let r = handle_http(
            "POST",
            "/thermostat/pair/finish",
            b"{\"pending_id\":\"nope\",\"pin\":\"123-45-678\"}",
            &stub(200, json!({})),
            &sessions,
        );
        assert_eq!(r.status, 404);
        assert!(!r.body.contains("123-45-678"));
        let r = handle_http(
            "POST",
            "/thermostat/pair/cancel",
            b"{\"pending_id\":\"nope\"}",
            &stub(200, json!({})),
            &sessions,
        );
        assert_eq!(r.status, 404);
        // Cancel the real session, then finishing it fails cleanly
        // (404, no panic): the session is gone, not half-alive.
        let r = handle_http(
            "POST",
            "/thermostat/pair/cancel",
            format!("{{\"pending_id\":\"{id}\"}}").as_bytes(),
            &stub(200, json!({})),
            &sessions,
        );
        assert_eq!(r.status, 200);
        let r = handle_http(
            "POST",
            "/thermostat/pair/finish",
            format!("{{\"pending_id\":\"{id}\",\"pin\":\"123-45-678\"}}").as_bytes(),
            &stub(200, json!({})),
            &sessions,
        );
        assert_eq!(r.status, 404);
        assert!(!r.body.contains("123-45-678"));
        server1.join().unwrap();
        server2.join().unwrap();
    }
}
