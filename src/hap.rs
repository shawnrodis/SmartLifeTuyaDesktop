//! HomeKit (HAP) thermostat control, written from scratch from the HAP
//! protocol description. No Apple software, account, or device is involved:
//! this module speaks the HomeKit Accessory Protocol directly over the
//! home network (mDNS discovery, SRP pair-setup, pair-verify, and the
//! ChaCha20-Poly1305 control session) using only pure-Rust, cross-platform
//! crates, so it builds for the Windows GNU target as well as macOS.
//!
//! Trust model (same as the app's "keep me logged in" Tuya credentials):
//! the HomeKit setup code is NEVER stored -- only the long-term pairing
//! keys derived from it are persisted in config.json, on the user's own
//! machine. Control works on the home network only (local, no cloud).
//!
//! Hardware interop is NOT verifiable in this environment: there is no
//! thermostat reachable from here, so pairing and control against a real
//! unit are field-tested by the user on their PC. Everything testable
//! without hardware (TLV, SRP math consistency, framing, error paths)
//! is covered by the unit tests at the bottom of this file.

use std::collections::HashMap;
use std::fmt;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use num_bigint::BigUint;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use x25519_dalek::{EphemeralSecret, PublicKey as XPublicKey};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum HapError {
    Io(std::io::Error),
    Msg(String),
}

impl fmt::Display for HapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HapError::Io(e) => write!(f, "network error: {}", e),
            HapError::Msg(m) => write!(f, "{}", m),
        }
    }
}

impl std::error::Error for HapError {}

impl From<std::io::Error> for HapError {
    fn from(e: std::io::Error) -> Self {
        HapError::Io(e)
    }
}

impl From<serde_json::Error> for HapError {
    fn from(e: serde_json::Error) -> Self {
        HapError::Msg(format!("bad accessory response: {}", e))
    }
}

pub type HapResult<T> = Result<T, HapError>;

fn err<T>(msg: impl Into<String>) -> HapResult<T> {
    Err(HapError::Msg(msg.into()))
}

// ---------------------------------------------------------------------------
// Diagnostics log (v1.6.4): an in-memory ring buffer of thermostat
// operations, shown in Settings and served to the web tile. Timestamped,
// capped at 200 entries (oldest evicted). Worker threads push directly;
// the UI thread and the bridge thread only read. NEVER log setup
// codes, pairing keys, tokens, Tuya credentials, or config contents:
// the log call sites below pass outcomes, temperatures, modes,
// characteristic ids, and status codes only, and the line builders
// take no secret parameter at all (see the tests).
// ---------------------------------------------------------------------------

const TSTAT_LOG_CAP: usize = 200;

fn tstat_log_buf() -> &'static std::sync::Mutex<std::collections::VecDeque<String>> {
    static BUF: std::sync::OnceLock<std::sync::Mutex<std::collections::VecDeque<String>>> =
        std::sync::OnceLock::new();
    BUF.get_or_init(|| std::sync::Mutex::new(std::collections::VecDeque::new()))
}

/// Push one timestamped line, evicting the oldest past the cap.
pub fn tstat_log(line: &str) {
    let stamp = chrono::Local::now().format("%H:%M:%S");
    let mut buf = match tstat_log_buf().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    buf.push_back(format!("{stamp} {line}"));
    while buf.len() > TSTAT_LOG_CAP {
        buf.pop_front();
    }
}

/// The whole buffer as display text (oldest first, one line each).
pub fn tstat_log_text() -> String {
    let join = |g: &std::collections::VecDeque<String>| g.iter().cloned().collect::<Vec<_>>().join("\n");
    match tstat_log_buf().lock() {
        Ok(g) => join(&g),
        Err(p) => join(&p.into_inner()),
    }
}

/// Drop every entry (Settings Clear button; tests).
pub fn tstat_log_clear() {
    if let Ok(mut g) = tstat_log_buf().lock() {
        g.clear();
    }
}

/// Pair log lines take a stage + outcome only: by construction they
/// can never carry the setup code (it is not a parameter here, and
/// the call sites pass outcomes/errors only).
fn pair_begin_log_line(device_name: &str) -> String {
    format!("Pair begin: {device_name} — session open, awaiting the code on the unit")
}

fn pair_finish_log_line(ok: bool, err: Option<&str>) -> String {
    match (ok, err) {
        (true, _) => "Pair finish: paired".to_string(),
        (false, Some(e)) => format!("Pair finish failed: {e}"),
        (false, None) => "Pair finish failed".to_string(),
    }
}

/// Set log line: requested changes, PUT result (incl. per-char
/// statuses), readback outcome, duration. Its parameters are
/// temperatures, modes, ids, and statuses only — no secret material
/// exists in this function's inputs at all.
fn set_log_line(requested: &str, put: &str, readback: &str, secs: f64) -> String {
    format!("Set: {requested} — {put}, {readback} ({secs:.1}s)")
}

// ---------------------------------------------------------------------------
// TLV8 (HAP section 12): tag u8, length u8, value; values longer than 255
// bytes are split into same-tag 255-byte chunks, concatenated on decode.
// ---------------------------------------------------------------------------

pub const TAG_METHOD: u8 = 0;
pub const TAG_IDENTIFIER: u8 = 1;
pub const TAG_SALT: u8 = 2;
pub const TAG_PUBLIC_KEY: u8 = 3;
pub const TAG_PROOF: u8 = 4;
pub const TAG_ENCRYPTED_DATA: u8 = 5;
pub const TAG_STATE: u8 = 6;
pub const TAG_ERROR: u8 = 7;
#[allow(dead_code)]
pub const TAG_RETRY_DELAY: u8 = 8;
#[allow(dead_code)]
pub const TAG_CERTIFICATE: u8 = 9;
pub const TAG_SIGNATURE: u8 = 10;
#[allow(dead_code)]
pub const TAG_PERMISSIONS: u8 = 11;
#[allow(dead_code)]
pub const TAG_FRAGMENT_DATA: u8 = 12;
#[allow(dead_code)]
pub const TAG_FRAGMENT_LAST: u8 = 13;
#[allow(dead_code)]
pub const TAG_SESSION_ID: u8 = 0x0E;
#[allow(dead_code)]
pub const TAG_SEPARATOR: u8 = 255;

pub const STATE_M1: u8 = 0x01;
pub const STATE_M2: u8 = 0x02;
pub const STATE_M3: u8 = 0x03;
pub const STATE_M4: u8 = 0x04;
pub const STATE_M5: u8 = 0x05;
pub const STATE_M6: u8 = 0x06;

pub const ERR_UNKNOWN: u8 = 1;
pub const ERR_AUTHENTICATION: u8 = 2;
pub const ERR_BACKOFF: u8 = 3;
pub const ERR_MAX_PEERS: u8 = 4;
pub const ERR_MAX_TRIES: u8 = 5;
pub const ERR_UNAVAILABLE: u8 = 6;
pub const ERR_BUSY: u8 = 7;

fn hap_error_name(code: u8) -> &'static str {
    match code {
        ERR_UNKNOWN => "unknown error",
        ERR_AUTHENTICATION => "authentication failed (wrong setup code?)",
        ERR_BACKOFF => "accessory asked to back off and retry later",
        ERR_MAX_PEERS => "accessory already has the maximum number of pairings",
        ERR_MAX_TRIES => "too many failed pairing attempts",
        ERR_UNAVAILABLE => "accessory unavailable",
        ERR_BUSY => "accessory busy",
        _ => "accessory error",
    }
}

/// Encode items, chunking any value longer than 255 bytes.
pub fn tlv_encode(items: &[(u8, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (tag, value) in items {
        if value.is_empty() {
            out.push(*tag);
            out.push(0);
            continue;
        }
        for chunk in value.chunks(255) {
            out.push(*tag);
            out.push(chunk.len() as u8);
            out.extend_from_slice(chunk);
        }
    }
    out
}

/// Decode, concatenating consecutive same-tag chunks. A truncated
/// header or value is a hard error, not a silent truncation.
pub fn tlv_decode(buf: &[u8]) -> HapResult<Vec<(u8, Vec<u8>)>> {
    let mut items: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        if i + 2 > buf.len() {
            return err("truncated TLV header");
        }
        let tag = buf[i];
        let len = buf[i + 1] as usize;
        if i + 2 + len > buf.len() {
            return err("truncated TLV value");
        }
        let value = &buf[i + 2..i + 2 + len];
        if let Some((last_tag, last_val)) = items.last_mut() {
            if *last_tag == tag {
                last_val.extend_from_slice(value);
                i += 2 + len;
                continue;
            }
        }
        items.push((tag, value.to_vec()));
        i += 2 + len;
    }
    Ok(items)
}

pub fn tlv_get<'a>(items: &'a [(u8, Vec<u8>)], tag: u8) -> Option<&'a [u8]> {
    items.iter().find(|(t, _)| *t == tag).map(|(_, v)| v.as_slice())
}

/// Any response containing an Error tag is a failure -- surface its code.
pub fn tlv_check_no_error(items: &[(u8, Vec<u8>)]) -> HapResult<()> {
    if let Some(code) = tlv_get(items, TAG_ERROR).and_then(|v| v.first().copied()) {
        return err(format!(
            "accessory refused (error {}: {})",
            code,
            hap_error_name(code)
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Small helpers: hex, UUID, setup-code format, temperatures, modes
// ---------------------------------------------------------------------------

pub fn hex_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for b in data {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

pub fn hex_decode(s: &str) -> HapResult<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return err("hex string has odd length");
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for pair in bytes.chunks(2) {
        let hi = (pair[0] as char).to_digit(16);
        let lo = (pair[1] as char).to_digit(16);
        match (hi, lo) {
            (Some(h), Some(l)) => out.push(((h << 4) | l) as u8),
            _ => return err("invalid hex character"),
        }
    }
    Ok(out)
}

fn hex_decode_32(s: &str) -> HapResult<[u8; 32]> {
    let v = hex_decode(s)?;
    if v.len() != 32 {
        return err(format!("expected 32 bytes, got {}", v.len()));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    Ok(out)
}

/// Fresh random UUID (v4, hyphenated lowercase) for a controller pairing id.
pub fn fresh_pairing_id() -> String {
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11],
        b[12], b[13], b[14], b[15]
    )
}

/// Accept "12345678" or "123-45-678", return the dashed HAP form.
/// Anything else is an error -- never guess at a setup code.
pub fn normalize_setup_code(input: &str) -> HapResult<String> {
    let digits: String = input.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 8 {
        return err("setup code must be 8 digits (e.g. 123-45-678)");
    }
    Ok(format!("{}-{}-{}", &digits[0..3], &digits[3..5], &digits[5..8]))
}

pub fn c_to_f(c: f64) -> f64 {
    c * 9.0 / 5.0 + 32.0
}

pub fn f_to_c(f: f64) -> f64 {
    (f - 32.0) * 5.0 / 9.0
}

pub fn mode_name(mode: u8) -> &'static str {
    match mode {
        0 => "Off",
        1 => "Heat",
        2 => "Cool",
        3 => "Auto",
        _ => "?",
    }
}

// ---------------------------------------------------------------------------
// Crypto primitives (wrappers only -- the crates do the real work)
// ---------------------------------------------------------------------------

fn sha512(data: &[u8]) -> [u8; 64] {
    let mut h = Sha512::new();
    h.update(data);
    h.finalize().into()
}

fn hkdf_sha512(ikm: &[u8], salt: &[u8], info: &[u8], len: usize) -> HapResult<Vec<u8>> {
    let hk = Hkdf::<Sha512>::new(Some(salt), ikm);
    let mut out = vec![0u8; len];
    hk.expand(info, &mut out)
        .map_err(|_| HapError::Msg("HKDF expand failed".into()))?;
    Ok(out)
}

fn chacha_encrypt(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    plaintext: &[u8],
) -> HapResult<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload { msg: plaintext, aad },
        )
        .map_err(|_| HapError::Msg("ChaCha20-Poly1305 encryption failed".into()))
}

fn chacha_decrypt(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext: &[u8],
) -> HapResult<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload { msg: ciphertext, aad },
        )
        .map_err(|_| HapError::Msg("ChaCha20-Poly1305 authentication failed".into()))
}

/// Pairing nonces: 4 zero bytes + an 8-byte ASCII label ("PS-Msg05", ...).
fn pairing_nonce(label: &[u8; 8]) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..12].copy_from_slice(label);
    n
}

/// Control-session nonces: 4 zero bytes + the 64-bit counter, little-endian.
fn frame_nonce(counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..12].copy_from_slice(&counter.to_le_bytes());
    n
}

// ---------------------------------------------------------------------------
// SRP-6a client for pair-setup (HAP pair-setup, M1..M6).
// Username is "Pair-Setup", password is the dashed setup code.
// N is the RFC 5054 3072-bit prime, g = 5, hash SHA-512, and k is the
// HAP pair-setup constant given in the protocol description.
// ---------------------------------------------------------------------------

/// RFC 5054 3072-bit prime (384 bytes when encoded big-endian).
const SRP_N_HEX: &str = concat!(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F1437",
    "4FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF05",
    "98DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD961C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3B",
    "E39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCBF6955817183995497CEA956AE515D2261898FA051015728E5A8AAAC42DAD33170D04507A33",
    "A85521ABDF1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7ABF5AE8CDB0933D71E8C94E04A25619DCEE3D2261AD2EE6BF12FFA06D98A0864",
    "D87602733EC86A64521F2B18177B200CBBE117577A615D6C770988C0BAD946E208E24FA074E5AB3143DB5BFCE0FD108E4B82D120A93AD2CAFFFFFFFFFFFFFFFF",
);

/// HAP pair-setup k constant (64 bytes, from the protocol description).
const SRP_K_HEX: &str = "a9c2e2559bf0ebb53f0cbbf62282906bede7f2182f00678211fbd5bde5b285033a4993503b87397f9be5ec02080fedbc0835587ad039060879b8621e8c3659e0";

fn srp_n() -> BigUint {
    BigUint::parse_bytes(SRP_N_HEX.as_bytes(), 16).expect("SRP prime constant")
}

fn srp_k() -> BigUint {
    BigUint::parse_bytes(SRP_K_HEX.as_bytes(), 16).expect("SRP k constant")
}

fn srp_g() -> BigUint {
    BigUint::from(5u32)
}

/// Left-pad big-endian bytes to 384 bytes (the SRP group size).
fn pad384(bytes: &[u8]) -> HapResult<[u8; 384]> {
    if bytes.len() > 384 {
        return err("SRP value larger than the 384-byte group");
    }
    let mut out = [0u8; 384];
    out[384 - bytes.len()..].copy_from_slice(bytes);
    Ok(out)
}

/// Salt padded to 16 bytes (left-padded with zeros if short; accessories
/// always send exactly 16, so this only matters defensively). The same
/// padded form is used for both x and the client proof.
fn pad16(salt: &[u8]) -> Vec<u8> {
    if salt.len() >= 16 {
        return salt.to_vec();
    }
    let mut out = vec![0u8; 16 - salt.len()];
    out.extend_from_slice(salt);
    out
}

/// SRP client state: private exponent `a` and public value A (384 bytes).
pub struct SrpClient {
    a: BigUint,
    a_pub: [u8; 384],
}

impl SrpClient {
    pub fn new() -> HapResult<Self> {
        let mut ab = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut ab);
        let mut a = BigUint::from_bytes_be(&ab);
        if a == BigUint::from(0u32) {
            a = BigUint::from(1u32);
        }
        let a_pub = pad384(&srp_g().modpow(&a, &srp_n()).to_bytes_be())?;
        Ok(SrpClient { a, a_pub })
    }

    pub fn public_bytes(&self) -> &[u8; 384] {
        &self.a_pub
    }

    /// x = SHA512(salt_16 || SHA512("Pair-Setup:<dashed-pin>")).
    fn compute_x(salt: &[u8], pin_dashed: &str) -> BigUint {
        let inner = sha512(format!("Pair-Setup:{}", pin_dashed).as_bytes());
        let mut msg = pad16(salt);
        msg.extend_from_slice(&inner);
        BigUint::from_bytes_be(&sha512(&msg))
    }

    /// Full client computation once M2's salt and public key arrive.
    /// Returns the session key K plus the values the caller needs for
    /// the proof (x is kept internal; u is not needed after this).
    pub fn session(&self, salt: &[u8], b_raw: &[u8], pin_dashed: &str) -> HapResult<[u8; 64]> {
        let n = srp_n();
        let k = srp_k();
        let g = srp_g();
        let x = Self::compute_x(salt, pin_dashed);
        let mut u_msg = Vec::with_capacity(384 + b_raw.len());
        u_msg.extend_from_slice(&self.a_pub);
        u_msg.extend_from_slice(b_raw);
        let u = BigUint::from_bytes_be(&sha512(&u_msg));
        let b = BigUint::from_bytes_be(b_raw);
        // S = (B - k*g^x) ^ (a + u*x) mod N, computed without ever
        // going negative: (B + N - (k*g^x mod N)) mod N.
        let gx = g.modpow(&x, &n);
        let kgx = (&k * &gx) % &n;
        let base = (&b + &n - &kgx) % &n;
        let exp = &self.a + &u * &x;
        let s = base.modpow(&exp, &n);
        let s_bytes = pad384(&s.to_bytes_be())?;
        Ok(sha512(&s_bytes))
    }

    /// M = SHA512( (SHA512(N) XOR SHA512(0x05)) || SHA512("Pair-Setup")
    ///             || salt_16 || A || B_raw || K ).
    pub fn client_proof(&self, salt: &[u8], b_raw: &[u8], k: &[u8; 64]) -> [u8; 64] {
        let h_n = sha512(&srp_n().to_bytes_be());
        let h_g = sha512(&[0x05u8]);
        let mut xor = [0u8; 64];
        for i in 0..64 {
            xor[i] = h_n[i] ^ h_g[i];
        }
        let h_user = sha512(b"Pair-Setup");
        let mut msg = Vec::with_capacity(64 + 64 + 16 + 384 + b_raw.len() + 64);
        msg.extend_from_slice(&xor);
        msg.extend_from_slice(&h_user);
        msg.extend_from_slice(&pad16(salt));
        msg.extend_from_slice(&self.a_pub);
        msg.extend_from_slice(b_raw);
        msg.extend_from_slice(k);
        sha512(&msg)
    }

    /// Server proof check: SHA512(A || M || K) must equal M4's proof.
    pub fn verify_server_proof(&self, m: &[u8; 64], k: &[u8; 64], proof: &[u8]) -> bool {
        let mut msg = Vec::with_capacity(384 + 64 + 64);
        msg.extend_from_slice(&self.a_pub);
        msg.extend_from_slice(m);
        msg.extend_from_slice(k);
        sha512(&msg).as_slice() == proof
    }
}

// ---------------------------------------------------------------------------
// Persisted pairing (serde). EVERY field has a default so that a partial
// or older pairing block still parses -- a malformed block must never
// nuke the whole config file on load (load_config falls back to
// Config::default() when parsing fails outright).
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct ThermostatPairing {
    /// This controller's pairing id (UUID string).
    #[serde(default)]
    pub ios_pairing_id: String,
    /// This controller's Ed25519 seed, hex (32 bytes). Never the setup code.
    #[serde(default)]
    pub ios_device_ltsk_hex: String,
    /// This controller's Ed25519 public key, hex (32 bytes).
    #[serde(default)]
    pub ios_device_ltpk_hex: String,
    /// The accessory's pairing id (from M6).
    #[serde(default)]
    pub accessory_pairing_id: String,
    /// The accessory's Ed25519 public key, hex (32 bytes).
    #[serde(default)]
    pub accessory_ltpk_hex: String,
    /// Last-known accessory address (discovery refreshes this on use).
    #[serde(default)]
    pub accessory_ip: String,
    #[serde(default)]
    pub accessory_port: u16,
    /// Display name / model from discovery (cosmetic only).
    #[serde(default)]
    pub accessory_name: String,
    #[serde(default)]
    pub accessory_model: String,
}

impl ThermostatPairing {
    /// True when the block has everything pairing verification needs.
    /// An incomplete block fails here with a clear message instead of
    /// panicking or connecting to a nonsense address.
    pub fn validate(&self) -> HapResult<()> {
        if self.ios_pairing_id.is_empty()
            || self.accessory_pairing_id.is_empty()
            || self.accessory_ip.is_empty()
            || self.accessory_port == 0
        {
            return err("thermostat pairing is incomplete -- pair again in Settings");
        }
        hex_decode_32(&self.ios_device_ltsk_hex)?;
        hex_decode_32(&self.ios_device_ltpk_hex)?;
        hex_decode_32(&self.accessory_ltpk_hex)?;
        Ok(())
    }

    fn signing_key(&self) -> HapResult<SigningKey> {
        let seed = hex_decode_32(&self.ios_device_ltsk_hex)?;
        Ok(SigningKey::from_bytes(&seed))
    }

    fn accessory_verifying_key(&self) -> HapResult<VerifyingKey> {
        let raw = hex_decode_32(&self.accessory_ltpk_hex)?;
        VerifyingKey::from_bytes(&raw)
            .map_err(|_| HapError::Msg("stored accessory public key is invalid".into()))
    }
}

// ---------------------------------------------------------------------------
// Connection: one TCP stream used for plain pairing HTTP first, then
// flipped into the encrypted control session after pair-verify. A single
// internal buffer serves both phases, so no bytes are ever lost between
// the plain and secure stages.
// ---------------------------------------------------------------------------

const NET_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const NET_IO_TIMEOUT: Duration = Duration::from_secs(20);

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

pub struct Conn {
    stream: TcpStream,
    host_header: String,
    buf: Vec<u8>,
    out_key: Option<[u8; 32]>,
    in_key: Option<[u8; 32]>,
    out_ctr: u64,
    in_ctr: u64,
}

impl Conn {
    pub fn connect(host: &str, port: u16) -> HapResult<Self> {
        use std::net::ToSocketAddrs;
        let mut addrs = (host, port)
            .to_socket_addrs()
            .map_err(|e| HapError::Msg(format!("cannot resolve {}: {}", host, e)))?;
        let addr = addrs.next().ok_or_else(|| {
            HapError::Msg(format!("cannot resolve {} to an address", host))
        })?;
        match TcpStream::connect_timeout(&addr, NET_CONNECT_TIMEOUT) {
            Ok(s) => {
                let _ = s.set_read_timeout(Some(NET_IO_TIMEOUT));
                let _ = s.set_write_timeout(Some(NET_IO_TIMEOUT));
                let _ = s.set_nodelay(true);
                Ok(Conn {
                    stream: s,
                    host_header: format!("{}:{}", host, port),
                    buf: Vec::new(),
                    out_key: None,
                    in_key: None,
                    out_ctr: 0,
                    in_ctr: 0,
                })
            }
            Err(e) => Err(HapError::Io(e)),
        }
    }

    /// Shut the socket down promptly (used when a pending pairing is
    /// abandoned: the accessory must see the session end at once).
    pub fn shutdown(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    /// Fill `buf`: raw socket bytes in the plain phase, one decrypted
    /// frame in the secure phase.
    fn fill(&mut self) -> HapResult<()> {
        match self.in_key {
            None => {
                let mut tmp = [0u8; 4096];
                let n = self.stream.read(&mut tmp)?;
                if n == 0 {
                    return err("accessory closed the connection");
                }
                self.buf.extend_from_slice(&tmp[..n]);
                Ok(())
            }
            Some(key) => {
                let mut lenb = [0u8; 2];
                self.stream.read_exact(&mut lenb)?;
                let len = u16::from_le_bytes(lenb) as usize;
                let mut ct = vec![0u8; len + 16];
                self.stream.read_exact(&mut ct)?;
                let nonce = frame_nonce(self.in_ctr);
                let pt = chacha_decrypt(&key, &nonce, &lenb, &ct)?;
                self.in_ctr += 1;
                self.buf.extend_from_slice(&pt);
                Ok(())
            }
        }
    }

    /// Like fill(), but returns Ok(false) on a clean end-of-stream
    /// instead of erroring. Plain phase: read() returning 0. Secure
    /// phase: read_exact hitting UnexpectedEof on a frame boundary
    /// (or mid-frame, which for a close-delimited body just means the
    /// final frame was truncated). Any other I/O error -- including
    /// the socket read timeout -- still propagates, so a
    /// close-delimited read waits at most NET_IO_TIMEOUT of silence.
    fn fill_eof(&mut self) -> HapResult<bool> {
        match self.in_key {
            None => {
                let mut tmp = [0u8; 4096];
                match self.stream.read(&mut tmp) {
                    Ok(0) => Ok(false),
                    Ok(n) => {
                        self.buf.extend_from_slice(&tmp[..n]);
                        Ok(true)
                    }
                    Err(e) => Err(HapError::Io(e)),
                }
            }
            Some(key) => {
                let mut lenb = [0u8; 2];
                match self.stream.read_exact(&mut lenb) {
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
                    Err(e) => Err(HapError::Io(e)),
                    Ok(()) => {
                        let len = u16::from_le_bytes(lenb) as usize;
                        let mut ct = vec![0u8; len + 16];
                        match self.stream.read_exact(&mut ct) {
                            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                                Ok(false)
                            }
                            Err(e) => Err(HapError::Io(e)),
                            Ok(()) => {
                                let nonce = frame_nonce(self.in_ctr);
                                let pt = chacha_decrypt(&key, &nonce, &lenb, &ct)?;
                                self.in_ctr += 1;
                                self.buf.extend_from_slice(&pt);
                                Ok(true)
                            }
                        }
                    }
                }
            }
        }
    }

    /// Decode one chunked body starting at `pos` in buf, refilling via
    /// fill() as needed. Works purely on self.buf + fill(), so it is
    /// correct in both phases (decrypted frames in the secure phase).
    /// Returns the decoded body and the index just past the terminal
    /// chunk's trailers; leftover bytes stay buffered for the next
    /// keep-alive response.
    fn read_chunked_body(&mut self, mut pos: usize) -> HapResult<(Vec<u8>, usize)> {
        let mut body = Vec::new();
        loop {
            // Chunk-size line: hex, with any ";extension" ignored.
            let line_end = loop {
                if let Some(rel) = find_subslice(&self.buf[pos..], b"\r\n") {
                    break pos + rel;
                }
                self.fill()?;
            };
            let size_text = String::from_utf8_lossy(&self.buf[pos..line_end]).into_owned();
            let size_str = size_text.split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size_str, 16).map_err(|_| {
                HapError::Msg(format!("bad chunk size {:?}", size_str))
            })?;
            pos = line_end + 2;
            if size == 0 {
                // Terminal chunk: trailers up to the empty line.
                loop {
                    let trailer_end = loop {
                        if let Some(rel) = find_subslice(&self.buf[pos..], b"\r\n") {
                            break pos + rel;
                        }
                        self.fill()?;
                    };
                    let empty = trailer_end == pos;
                    pos = trailer_end + 2;
                    if empty {
                        break;
                    }
                }
                return Ok((body, pos));
            }
            // Chunk data + CRLF.
            while self.buf.len() < pos + size + 2 {
                self.fill()?;
            }
            body.extend_from_slice(&self.buf[pos..pos + size]);
            if &self.buf[pos + size..pos + size + 2] != b"\r\n" {
                return err("chunked body: missing CRLF after chunk data");
            }
            pos += size + 2;
        }
    }

    /// Read one HTTP response (status + body). Unsolicited EVENT
    /// responses are consumed and skipped (we never subscribe).
    fn read_http_response(&mut self) -> HapResult<(u16, Vec<u8>)> {
        let mut events_skipped = 0;
        loop {
            let hdr_end = loop {
                if let Some(pos) = find_subslice(&self.buf, b"\r\n\r\n") {
                    break pos;
                }
                self.fill()?;
            };
            let header_bytes = self.buf[..hdr_end].to_vec();
            let header_text = String::from_utf8_lossy(&header_bytes);
            let mut lines = header_text.lines();
            let status_line = lines.next().unwrap_or("").to_string();
            // Content-Length presence matters: an explicit "0" means
            // an empty body now, while a missing header means the
            // body runs to end-of-stream (see below).
            let mut content_len: Option<usize> = None;
            let mut chunked = false;
            for line in lines {
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_len = Some(v.trim().parse().unwrap_or(0));
                } else if let Some(v) = lower.strip_prefix("transfer-encoding:") {
                    chunked = v
                        .split(',')
                        .any(|c| c.trim().eq_ignore_ascii_case("chunked"));
                }
            }
            let body_start = hdr_end + 4;
            let (body, consumed) = if chunked {
                // Chunked wins over Content-Length when both appear.
                self.read_chunked_body(body_start)?
            } else if let Some(len) = content_len {
                let total = body_start + len;
                while self.buf.len() < total {
                    self.fill()?;
                }
                (self.buf[body_start..total].to_vec(), total)
            } else {
                // Neither framing header: the body runs until the
                // accessory closes the connection (or the read
                // timeout fires). The connection is unusable
                // afterwards, but our callers open a fresh session
                // per operation, so that costs nothing.
                while self.fill_eof()? {}
                let body = self.buf[body_start..].to_vec();
                (body, self.buf.len())
            };
            self.buf.drain(..consumed);
            if status_line.starts_with("EVENT") {
                events_skipped += 1;
                if events_skipped > 10 {
                    return err("too many unsolicited EVENT responses");
                }
                continue;
            }
            let code: u16 = status_line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            return Ok((code, body));
        }
    }

    /// Plain (unencrypted) POST; returns (status, body). Non-2xx is an error.
    fn post_plain(&mut self, path: &str, body: &[u8], content_type: &str) -> HapResult<(u16, Vec<u8>)> {
        let req = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
            path,
            self.host_header,
            content_type,
            body.len()
        );
        self.stream.write_all(req.as_bytes())?;
        self.stream.write_all(body)?;
        let (code, resp) = self.read_http_response()?;
        if !(200..300).contains(&code) {
            let snippet: String = String::from_utf8_lossy(&resp).chars().take(200).collect();
            return err(format!("HTTP {} from accessory on {}: {}", code, path, snippet));
        }
        Ok((code, resp))
    }

    /// Plain TLV POST: decodes the response and rejects any Error tag.
    fn post_tlv(&mut self, path: &str, body: &[u8]) -> HapResult<Vec<(u8, Vec<u8>)>> {
        let (_code, resp) = self.post_plain(path, body, "application/pairing+tlv8")?;
        let items = tlv_decode(&resp)?;
        tlv_check_no_error(&items)?;
        Ok(items)
    }

    /// Flip this connection into the encrypted control session. The
    /// plain-phase buffer must be empty -- leftover bytes would mean
    /// the plain and secure streams desynchronised.
    fn set_secure(&mut self, out_key: [u8; 32], in_key: [u8; 32]) -> HapResult<()> {
        if !self.buf.is_empty() {
            return err("leftover bytes before secure session start");
        }
        self.out_key = Some(out_key);
        self.in_key = Some(in_key);
        self.out_ctr = 0;
        self.in_ctr = 0;
        Ok(())
    }

    /// Secure request: split into <=1024-byte plaintext chunks, frame
    /// each (2-byte LE length + ciphertext+tag), and send ALL frames in
    /// a single write_all, as HAP requires.
    fn request_secure(&mut self, req: &[u8]) -> HapResult<(u16, Vec<u8>)> {
        let out_key = self
            .out_key
            .ok_or_else(|| HapError::Msg("secure session not established".into()))?;
        let mut frames = Vec::new();
        let mut counter = self.out_ctr;
        for chunk in req.chunks(1024) {
            let lenb = (chunk.len() as u16).to_le_bytes();
            let ct = chacha_encrypt(&out_key, &frame_nonce(counter), &lenb, chunk)?;
            frames.extend_from_slice(&lenb);
            frames.extend_from_slice(&ct);
            counter += 1;
        }
        self.out_ctr = counter;
        self.stream.write_all(&frames)?;
        self.read_http_response()
    }
}

fn check_state(items: &[(u8, Vec<u8>)], expected: u8, step: &str) -> HapResult<()> {
    match tlv_get(items, TAG_STATE).and_then(|v| v.first().copied()) {
        Some(s) if s == expected => Ok(()),
        Some(s) => err(format!(
            "{}: unexpected pairing state {} (expected {})",
            step, s, expected
        )),
        None => err(format!("{}: pairing response missing state", step)),
    }
}

// ---------------------------------------------------------------------------
// Two-step pair-setup (M1..M6) for accessories whose setup code rotates
// per attempt (field-verified on the Resideo T6: the unit shows a fresh
// code on its screen each time a pairing session starts, so a code
// typed before the session begins is always stale). begin opens the
// connection and runs M1..M2; finish runs M3..M6 on that SAME held-open
// connection once the user has read the fresh code off the screen.
// The code is NEVER part of the pending state: it arrives only at
// finish, is used for one SRP computation, and is never stored.
// ---------------------------------------------------------------------------

/// Pair-setup session between M2 and M3: the live connection, the
/// SRP client state (whose public value A does not depend on the
/// code), and the M2 values. No setup code is held here, by
/// construction: there is no pin field, and the Debug impl below
/// prints only the device identity, never key material.
pub struct PendingPairSetup {
    conn: Option<Conn>,
    srp: SrpClient,
    salt: Vec<u8>,
    b_raw: Vec<u8>,
    ip: String,
    port: u16,
    name: String,
    model: String,
    device_id: String,
}

impl fmt::Debug for PendingPairSetup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingPairSetup")
            .field("device_id", &self.device_id)
            .field("name", &self.name)
            .field("address", &format!("{}:{}", self.ip, self.port))
            .finish()
    }
}

impl Drop for PendingPairSetup {
    /// Abandoning a pending pairing closes the socket at once, so the
    /// accessory sees the session end (cancel, or a dropped begin).
    fn drop(&mut self) {
        if let Some(mut conn) = self.conn.take() {
            conn.shutdown();
        }
    }
}

impl PendingPairSetup {
    /// mDNS id of the device this session belongs to.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }
}

/// Resolve a discovered device to its IPv4-first address, as the
/// single-shot flow did.
fn device_address(device: &DiscoveredDevice) -> HapResult<&str> {
    device
        .addresses
        .iter()
        .find(|a| a.contains('.'))
        .or_else(|| device.addresses.first())
        .map(|s| s.as_str())
        .ok_or_else(|| HapError::Msg("discovered device has no address".into()))
}

/// Begin pairing: open the connection, send M1 (no code needed), read
/// M2 (salt + server public key B). The accessory shows its fresh
/// setup code on screen once this session starts. Logs the outcome
/// only (device name, never any code or key).
pub fn begin_pair_setup(device: &DiscoveredDevice) -> HapResult<PendingPairSetup> {
    let res = begin_pair_setup_inner(device);
    match &res {
        Ok(p) => tstat_log(&pair_begin_log_line(&p.name)),
        Err(e) => tstat_log(&format!("Pair begin failed: {e}")),
    }
    res
}

fn begin_pair_setup_inner(device: &DiscoveredDevice) -> HapResult<PendingPairSetup> {
    let ip = device_address(device)?.to_string();
    let mut conn = Conn::connect(&ip, device.port)?;
    let m1 = tlv_encode(&[(TAG_STATE, &[STATE_M1]), (TAG_METHOD, &[0x00])]);
    let m2 = conn.post_tlv("/pair-setup", &m1)?;
    check_state(&m2, STATE_M2, "pair-setup M2")?;
    let salt = tlv_get(&m2, TAG_SALT)
        .ok_or_else(|| HapError::Msg("pair-setup M2 missing salt".into()))?
        .to_vec();
    let b_raw = tlv_get(&m2, TAG_PUBLIC_KEY)
        .ok_or_else(|| HapError::Msg("pair-setup M2 missing server public key".into()))?
        .to_vec();
    let srp = SrpClient::new()?;
    Ok(PendingPairSetup {
        conn: Some(conn),
        srp,
        salt,
        b_raw,
        ip,
        port: device.port,
        name: device.name.clone(),
        model: device.model.clone(),
        device_id: device.id.clone(),
    })
}

/// True when a pair-setup failure means the accessory-side session is
/// gone (idle timeout or a closed connection): the only recovery is
/// to start over with a fresh session and a fresh screen code.
fn is_session_expired(e: &HapError) -> bool {
    match e {
        HapError::Io(io) => io.kind() == std::io::ErrorKind::TimedOut,
        HapError::Msg(m) => m.contains("accessory closed the connection"),
    }
}

/// M5..M6 on an already-running pair-setup session: fresh controller
/// identity + signature, encrypted, then the accessory's identity
/// verified. Called only by finish_pair_setup.
fn pair_setup_m5_m6(
    conn: &mut Conn,
    k: &[u8; 64],
    ip: &str,
    port: u16,
    name: &str,
    model: &str,
) -> HapResult<ThermostatPairing> {
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let signing = SigningKey::from_bytes(&seed);
    let ltpk = signing.verifying_key().to_bytes();
    let pairing_id = fresh_pairing_id();
    let ios_x = hkdf_sha512(
        k,
        b"Pair-Setup-Controller-Sign-Salt",
        b"Pair-Setup-Controller-Sign-Info",
        32,
    )?;
    let enc_key = hkdf_sha512(
        k,
        b"Pair-Setup-Encrypt-Salt",
        b"Pair-Setup-Encrypt-Info",
        32,
    )?;
    let enc_key_arr: [u8; 32] = enc_key
        .try_into()
        .map_err(|_| HapError::Msg("HKDF output length mismatch".into()))?;
    let mut info = Vec::new();
    info.extend_from_slice(&ios_x);
    info.extend_from_slice(pairing_id.as_bytes());
    info.extend_from_slice(&ltpk);
    let sig = signing.sign(&info).to_bytes();
    let sub = tlv_encode(&[
        (TAG_IDENTIFIER, pairing_id.as_bytes()),
        (TAG_PUBLIC_KEY, &ltpk),
        (TAG_SIGNATURE, &sig),
    ]);
    let enc = chacha_encrypt(&enc_key_arr, &pairing_nonce(b"PS-Msg05"), b"", &sub)?;
    let m5 = tlv_encode(&[(TAG_STATE, &[STATE_M5]), (TAG_ENCRYPTED_DATA, &enc)]);
    let m6 = conn.post_tlv("/pair-setup", &m5)?;
    check_state(&m6, STATE_M6, "pair-setup M6")?;
    let m6data = tlv_get(&m6, TAG_ENCRYPTED_DATA)
        .ok_or_else(|| HapError::Msg("pair-setup M6 missing encrypted data".into()))?;
    let inner = chacha_decrypt(&enc_key_arr, &pairing_nonce(b"PS-Msg06"), b"", m6data)?;
    let inner_items = tlv_decode(&inner)?;
    let acc_sig = tlv_get(&inner_items, TAG_SIGNATURE)
        .ok_or_else(|| HapError::Msg("pair-setup M6 missing accessory signature".into()))?
        .to_vec();
    let acc_id = tlv_get(&inner_items, TAG_IDENTIFIER)
        .ok_or_else(|| HapError::Msg("pair-setup M6 missing accessory id".into()))?
        .to_vec();
    let acc_ltpk = tlv_get(&inner_items, TAG_PUBLIC_KEY)
        .ok_or_else(|| HapError::Msg("pair-setup M6 missing accessory public key".into()))?
        .to_vec();
    if acc_ltpk.len() != 32 {
        return err("pair-setup M6 accessory public key is not 32 bytes");
    }
    let acc_x = hkdf_sha512(
        k,
        b"Pair-Setup-Accessory-Sign-Salt",
        b"Pair-Setup-Accessory-Sign-Info",
        32,
    )?;
    let mut acc_info = Vec::new();
    acc_info.extend_from_slice(&acc_x);
    acc_info.extend_from_slice(&acc_id);
    acc_info.extend_from_slice(&acc_ltpk);
    let mut ltpk_arr = [0u8; 32];
    ltpk_arr.copy_from_slice(&acc_ltpk);
    let vk = VerifyingKey::from_bytes(&ltpk_arr)
        .map_err(|_| HapError::Msg("accessory public key is invalid".into()))?;
    let sig_obj = Signature::try_from(acc_sig.as_slice())
        .map_err(|_| HapError::Msg("accessory signature is malformed".into()))?;
    vk.verify(&acc_info, &sig_obj)
        .map_err(|_| HapError::Msg("accessory signature mismatch".into()))?;

    Ok(ThermostatPairing {
        ios_pairing_id: pairing_id,
        ios_device_ltsk_hex: hex_encode(&seed),
        ios_device_ltpk_hex: hex_encode(&ltpk),
        accessory_pairing_id: String::from_utf8_lossy(&acc_id).into_owned(),
        accessory_ltpk_hex: hex_encode(&acc_ltpk),
        accessory_ip: ip.to_string(),
        accessory_port: port,
        accessory_name: name.to_string(),
        accessory_model: model.to_string(),
        ..Default::default()
    })
}

/// Finish pairing on a begun session: compute the SRP proof from the
/// just-read code, send M3, verify M4, run M5/M6. Consumes the pending
/// state; on any failure the connection is closed with it.
pub fn finish_pair_setup(mut pending: PendingPairSetup, code: &str) -> HapResult<ThermostatPairing> {
    // The setup code is passed straight to SRP and never logged:
    // the log line below carries the outcome only.
    let res = finish_pair_setup_inner(pending, code);
    match &res {
        Ok(_) => tstat_log(&pair_finish_log_line(true, None)),
        Err(e) => tstat_log(&pair_finish_log_line(false, Some(&format!("{e}")))),
    }
    res
}

fn finish_pair_setup_inner(mut pending: PendingPairSetup, code: &str) -> HapResult<ThermostatPairing> {
    let pin = normalize_setup_code(code)?;
    let mut conn = pending
        .conn
        .take()
        .ok_or_else(|| HapError::Msg("pairing session already finished".into()))?;
    let mut run = || -> HapResult<ThermostatPairing> {
        let k = pending.srp.session(&pending.salt, &pending.b_raw, &pin)?;
        let m_proof = pending
            .srp
            .client_proof(&pending.salt, &pending.b_raw, &k);
        let m3 = tlv_encode(&[
            (TAG_STATE, &[STATE_M3]),
            (TAG_PUBLIC_KEY, pending.srp.public_bytes()),
            (TAG_PROOF, &m_proof),
        ]);
        let m4 = conn.post_tlv("/pair-setup", &m3)?;
        check_state(&m4, STATE_M4, "pair-setup M4")?;
        let server_proof = tlv_get(&m4, TAG_PROOF)
            .ok_or_else(|| HapError::Msg("pair-setup M4 missing server proof".into()))?;
        if !pending
            .srp
            .verify_server_proof(&m_proof, &k, server_proof)
        {
            return err("accessory proof mismatch -- wrong setup code?");
        }
        pair_setup_m5_m6(
            &mut conn,
            &k,
            &pending.ip,
            pending.port,
            &pending.name,
            &pending.model,
        )
    };
    run().map_err(|e| {
        if is_session_expired(&e) {
            HapError::Msg(
                "pairing session timed out -- press Start pairing and try again with the fresh code"
                    .into(),
            )
        } else {
            e
        }
    })
}

// ---------------------------------------------------------------------------
// Pair-verify (M1..M4) + encrypted control session.
// Control keys are derived from the X25519 shared secret, NOT from the
// pair-verify session key (which only protects the M2/M3 handshake).
// ---------------------------------------------------------------------------

pub struct SecureConn {
    conn: Conn,
}

impl SecureConn {
    pub fn open(pairing: &ThermostatPairing) -> HapResult<Self> {
        pairing.validate()?;
        let mut conn = Conn::connect(&pairing.accessory_ip, pairing.accessory_port)?;

        // M1: ephemeral X25519 public key.
        let secret = EphemeralSecret::random_from_rng(rand::thread_rng());
        let ios_pub = XPublicKey::from(&secret).to_bytes();
        let m1 = tlv_encode(&[(TAG_STATE, &[STATE_M1]), (TAG_PUBLIC_KEY, &ios_pub)]);
        let m2 = conn.post_tlv("/pair-verify", &m1)?;
        check_state(&m2, STATE_M2, "pair-verify M2")?;
        let acc_pub_raw = tlv_get(&m2, TAG_PUBLIC_KEY)
            .ok_or_else(|| HapError::Msg("pair-verify M2 missing accessory public key".into()))?
            .to_vec();
        if acc_pub_raw.len() != 32 {
            return err("pair-verify M2 accessory public key is not 32 bytes");
        }
        let enc = tlv_get(&m2, TAG_ENCRYPTED_DATA)
            .ok_or_else(|| HapError::Msg("pair-verify M2 missing encrypted data".into()))?
            .to_vec();
        let mut acc_pub_arr = [0u8; 32];
        acc_pub_arr.copy_from_slice(&acc_pub_raw);
        let acc_pub = XPublicKey::from(acc_pub_arr);
        let shared = secret.diffie_hellman(&acc_pub).to_bytes();
        let session_key = hkdf_sha512(
            &shared,
            b"Pair-Verify-Encrypt-Salt",
            b"Pair-Verify-Encrypt-Info",
            32,
        )?;
        let session_key_arr: [u8; 32] = session_key
            .try_into()
            .map_err(|_| HapError::Msg("HKDF output length mismatch".into()))?;
        let inner = chacha_decrypt(&session_key_arr, &pairing_nonce(b"PV-Msg02"), b"", &enc)?;
        let inner_items = tlv_decode(&inner)?;
        let id = tlv_get(&inner_items, TAG_IDENTIFIER)
            .ok_or_else(|| HapError::Msg("pair-verify M2 missing accessory id".into()))?;
        if id != pairing.accessory_pairing_id.as_bytes() {
            return err("pair-verify: accessory id does not match the saved pairing");
        }
        let sig_raw = tlv_get(&inner_items, TAG_SIGNATURE)
            .ok_or_else(|| HapError::Msg("pair-verify M2 missing signature".into()))?
            .to_vec();
        // Verify signature over (accessory_session_pub || accessory_id || ios_pub).
        let mut msg = Vec::new();
        msg.extend_from_slice(&acc_pub_raw);
        msg.extend_from_slice(pairing.accessory_pairing_id.as_bytes());
        msg.extend_from_slice(&ios_pub);
        let vk = pairing.accessory_verifying_key()?;
        let sig_obj = Signature::try_from(sig_raw.as_slice())
            .map_err(|_| HapError::Msg("pair-verify signature is malformed".into()))?;
        vk.verify(&msg, &sig_obj)
            .map_err(|_| HapError::Msg("pair-verify: accessory signature mismatch".into()))?;

        // M3: our signature over (ios_pub || ios_pairing_id || accessory_pub).
        let mut info = Vec::new();
        info.extend_from_slice(&ios_pub);
        info.extend_from_slice(pairing.ios_pairing_id.as_bytes());
        info.extend_from_slice(&acc_pub_raw);
        let signing = pairing.signing_key()?;
        let sig3 = signing.sign(&info).to_bytes();
        let sub = tlv_encode(&[
            (TAG_IDENTIFIER, pairing.ios_pairing_id.as_bytes()),
            (TAG_SIGNATURE, &sig3),
        ]);
        let enc3 = chacha_encrypt(&session_key_arr, &pairing_nonce(b"PV-Msg03"), b"", &sub)?;
        let m3 = tlv_encode(&[(TAG_STATE, &[STATE_M3]), (TAG_ENCRYPTED_DATA, &enc3)]);
        let m4 = conn.post_tlv("/pair-verify", &m3)?;
        check_state(&m4, STATE_M4, "pair-verify M4")?;

        // Control keys from the shared secret.
        let out_key = hkdf_sha512(
            &shared,
            b"Control-Salt",
            b"Control-Write-Encryption-Key",
            32,
        )?;
        let in_key = hkdf_sha512(
            &shared,
            b"Control-Salt",
            b"Control-Read-Encryption-Key",
            32,
        )?;
        let out_arr: [u8; 32] = out_key
            .try_into()
            .map_err(|_| HapError::Msg("HKDF output length mismatch".into()))?;
        let in_arr: [u8; 32] = in_key
            .try_into()
            .map_err(|_| HapError::Msg("HKDF output length mismatch".into()))?;
        conn.set_secure(out_arr, in_arr)?;
        Ok(SecureConn { conn })
    }

    fn request(&mut self, method: &str, path: &str, body: Option<&[u8]>) -> HapResult<(u16, Vec<u8>)> {
        let mut req = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\n",
            method, path, self.conn.host_header
        );
        if let Some(b) = body {
            req.push_str(&format!(
                "Content-Type: application/hap+json\r\nContent-Length: {}\r\n",
                b.len()
            ));
        }
        req.push_str("Connection: keep-alive\r\n\r\n");
        let mut full = req.into_bytes();
        if let Some(b) = body {
            full.extend_from_slice(b);
        }
        let (code, resp) = self.conn.request_secure(&full)?;
        if !(200..300).contains(&code) {
            let snippet: String = String::from_utf8_lossy(&resp).chars().take(200).collect();
            return err(format!("HTTP {} from accessory on {}: {}", code, path, snippet));
        }
        Ok((code, resp))
    }

    pub fn get_accessories(&mut self) -> HapResult<Vec<AccessoryInfo>> {
        let (_code, body) = self.request("GET", "/accessories", None)?;
        let root: serde_json::Value = serde_json::from_slice(&body)?;
        let accs = root
            .get("accessories")
            .ok_or_else(|| HapError::Msg("/accessories response missing accessories".into()))?
            .clone();
        let parsed: Vec<AccessoryInfo> = serde_json::from_value(accs)?;
        Ok(parsed)
    }

    /// Read a set of characteristics. Returns only the values present;
    /// characteristics the accessory reports with an error status are
    /// simply absent from the map.
    pub fn get_characteristics(
        &mut self,
        ids: &[(u32, u32)],
    ) -> HapResult<HashMap<(u32, u32), serde_json::Value>> {
        let query: Vec<String> = ids.iter().map(|(a, i)| format!("{}.{}", a, i)).collect();
        let path = format!("/characteristics?id={}", query.join(","));
        let (_code, body) = self.request("GET", &path, None)?;
        let root: serde_json::Value = serde_json::from_slice(&body)?;
        let mut out = HashMap::new();
        if let Some(list) = root.get("characteristics").and_then(|v| v.as_array()) {
            for item in list {
                let aid = item.get("aid").and_then(|v| v.as_u64());
                let iid = item.get("iid").and_then(|v| v.as_u64());
                if let (Some(a), Some(i), Some(val)) =
                    (aid, iid, item.get("value").cloned())
                {
                    out.insert((a as u32, i as u32), val);
                }
            }
        }
        Ok(out)
    }

    /// Write characteristics. Ok(vec![]) means full success (204 / empty
    /// body). Otherwise returns the rejected (aid, iid, status) entries
    /// so the caller can report exactly what failed.
    pub fn put_characteristics(
        &mut self,
        writes: &[(u32, u32, serde_json::Value)],
    ) -> HapResult<Vec<(u32, u32, i64)>> {
        let chars: Vec<serde_json::Value> = writes
            .iter()
            .map(|(a, i, v)| serde_json::json!({ "aid": a, "iid": i, "value": v }))
            .collect();
        let body = serde_json::json!({ "characteristics": chars }).to_string();
        let (_code, resp) = self.request("PUT", "/characteristics", Some(body.as_bytes()))?;
        if resp.is_empty() {
            return Ok(Vec::new());
        }
        let root: serde_json::Value = serde_json::from_slice(&resp)?;
        let mut failures = Vec::new();
        if let Some(list) = root.get("characteristics").and_then(|v| v.as_array()) {
            for item in list {
                let aid = item.get("aid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let iid = item.get("iid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let status = item.get("status").and_then(|v| v.as_i64()).unwrap_or(0);
                if status != 0 {
                    failures.push((aid, iid, status));
                }
            }
        }
        Ok(failures)
    }
}

// ---------------------------------------------------------------------------
// Accessory model + thermostat summary mapping.
// HAP type strings are full UUIDs ("0000004A-0000-1000-8000-0026BB765291");
// we compare only the 8-digit prefix, uppercased.
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug, Clone)]
pub struct AccessoryInfo {
    pub aid: u32,
    #[serde(default)]
    pub services: Vec<ServiceInfo>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ServiceInfo {
    // Service iid: part of the HAP accessory record (characteristic
    // addressing keys off aid+iid); retained even though the
    // thermostat mapping does not need it today.
    #[allow(dead_code)]
    pub iid: u32,
    #[serde(rename = "type")]
    pub type_uuid: String,
    #[serde(default)]
    pub characteristics: Vec<CharInfo>,
    #[allow(dead_code)]
    #[serde(default)]
    pub linked: Vec<u32>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CharInfo {
    pub iid: u32,
    #[serde(rename = "type")]
    pub type_uuid: String,
    #[serde(default)]
    pub value: Option<serde_json::Value>,
    #[allow(dead_code)]
    #[serde(default)]
    pub perms: Vec<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub format: Option<String>,
    /// HAP characteristic properties for write validation (v1.6.4):
    /// the step the accessory accepts, plus its range. Absent on
    /// accessories that don't report them (then set_state falls back
    /// to 0.5 °C steps with no clamping).
    #[serde(default, rename = "minStep")]
    pub min_step: Option<f64>,
    #[serde(default, rename = "minValue")]
    pub min_value: Option<f64>,
    #[serde(default, rename = "maxValue")]
    pub max_value: Option<f64>,
}

/// Normalize a HAP service/characteristic type to its 8-digit prefix.
/// Full UUIDs ("0000004A-0000-1000-8000-0026BB765291") truncate to the
/// first 8 chars as before, but some accessories (including the
/// Resideo T6) report short forms ("4A", "11", "004A"): the part
/// before any '-' is uppercased and left-padded with '0' to 8 chars,
/// so "4A" -> "0000004A" and full-UUID matching is unchanged.
fn short_type(full: &str) -> String {
    let base = full.split('-').next().unwrap_or("");
    let upper = base.to_ascii_uppercase();
    let count = upper.chars().count();
    if count >= 8 {
        upper.chars().take(8).collect()
    } else {
        let mut s = String::with_capacity(8);
        for _ in 0..(8 - count) {
            s.push('0');
        }
        s.push_str(&upper);
        s
    }
}

/// HAP service / characteristic type prefixes (from the spec).
const SVC_THERMOSTAT: &str = "0000004A";
const SVC_FAN: &str = "00000040";
const SVC_ACCESSORY_INFO: &str = "0000003E";
const CH_NAME: &str = "00000023";
const CH_CURRENT_MODE: &str = "0000000F";
const CH_TARGET_MODE: &str = "00000033";
const CH_CURRENT_TEMP: &str = "00000011";
const CH_TARGET_TEMP: &str = "00000035";
const CH_TEMP_UNITS: &str = "00000036";
const CH_HEAT_TO: &str = "00000012";
const CH_COOL_TO: &str = "0000000D";
const CH_HUMIDITY: &str = "00000010";
const CH_FAN_ACTIVE: &str = "000000B0";

#[derive(Clone, Debug, Default)]
pub struct ThermostatState {
    pub name: String,
    pub current_mode: u8,
    pub target_mode: u8,
    pub current_temp_c: f64,
    pub target_temp_c: f64,
    pub heat_to_c: Option<f64>,
    pub cool_to_c: Option<f64>,
    pub humidity: Option<f64>,
    pub fan_on: Option<bool>,
    /// True when the accessory reports temperatures in Fahrenheit.
    pub temp_units_f: bool,
    // Write targets (aid, iid), filled when the characteristic exists.
    pub w_target_mode: Option<(u32, u32)>,
    pub w_target_temp: Option<(u32, u32)>,
    pub w_heat_to: Option<(u32, u32)>,
    pub w_cool_to: Option<(u32, u32)>,
    pub w_fan_active: Option<(u32, u32)>,
    // Write validation (v1.6.4): the step/range each temperature
    // characteristic reports, recorded alongside its write target
    // so set_state can quantize writes the accessory accepts.
    pub step_target_temp: Option<f64>,
    pub min_target_temp: Option<f64>,
    pub max_target_temp: Option<f64>,
    pub step_heat_to: Option<f64>,
    pub min_heat_to: Option<f64>,
    pub max_heat_to: Option<f64>,
    pub step_cool_to: Option<f64>,
    pub min_cool_to: Option<f64>,
    pub max_cool_to: Option<f64>,
    // Read targets (aid, iid) for characteristics that are never
    // written but must be readable via /characteristics when
    // /accessories omits their values.
    pub r_current_mode: Option<(u32, u32)>,
    pub r_current_temp: Option<(u32, u32)>,
    pub r_humidity: Option<(u32, u32)>,
    pub r_units: Option<(u32, u32)>,
}

fn as_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))
}

fn as_u8(v: &serde_json::Value) -> Option<u8> {
    v.as_u64().map(|u| u as u8)
}

pub fn state_from_accessories(accs: &[AccessoryInfo]) -> ThermostatState {
    let mut st = ThermostatState::default();
    for acc in accs {
        for svc in &acc.services {
            let stype = short_type(&svc.type_uuid);
            if stype == SVC_ACCESSORY_INFO {
                for ch in &svc.characteristics {
                    if short_type(&ch.type_uuid) == CH_NAME {
                        if let Some(v) = &ch.value {
                            if let Some(s) = v.as_str() {
                                st.name = s.to_string();
                            }
                        }
                    }
                }
            }
            if stype == SVC_THERMOSTAT {
                for ch in &svc.characteristics {
                    let ctype = short_type(&ch.type_uuid);
                    // Record the address regardless of whether a value
                    // is present: /accessories may omit values, and the
                    // /characteristics overlay then reads them by
                    // address. (Previously targets were recorded only
                    // inside the value arms below, so a valueless
                    // /accessories left nothing to overlay.)
                    match ctype.as_str() {
                        CH_CURRENT_MODE => {
                            st.r_current_mode = Some((acc.aid, ch.iid));
                        }
                        CH_TARGET_MODE => {
                            st.w_target_mode = Some((acc.aid, ch.iid));
                        }
                        CH_CURRENT_TEMP => {
                            st.r_current_temp = Some((acc.aid, ch.iid));
                        }
                        CH_TARGET_TEMP => {
                            st.w_target_temp = Some((acc.aid, ch.iid));
                            st.step_target_temp = ch.min_step;
                            st.min_target_temp = ch.min_value;
                            st.max_target_temp = ch.max_value;
                        }
                        CH_TEMP_UNITS => {
                            st.r_units = Some((acc.aid, ch.iid));
                        }
                        CH_HEAT_TO => {
                            st.w_heat_to = Some((acc.aid, ch.iid));
                            st.step_heat_to = ch.min_step;
                            st.min_heat_to = ch.min_value;
                            st.max_heat_to = ch.max_value;
                        }
                        CH_COOL_TO => {
                            st.w_cool_to = Some((acc.aid, ch.iid));
                            st.step_cool_to = ch.min_step;
                            st.min_cool_to = ch.min_value;
                            st.max_cool_to = ch.max_value;
                        }
                        CH_HUMIDITY => {
                            st.r_humidity = Some((acc.aid, ch.iid));
                        }
                        _ => {}
                    }
                    let v = match &ch.value {
                        Some(v) => v,
                        None => continue,
                    };
                    match ctype.as_str() {
                        CH_CURRENT_MODE => {
                            if let Some(m) = as_u8(v) {
                                st.current_mode = m;
                            }
                        }
                        CH_TARGET_MODE => {
                            if let Some(m) = as_u8(v) {
                                st.target_mode = m;
                            }
                        }
                        CH_CURRENT_TEMP => {
                            if let Some(t) = as_f64(v) {
                                st.current_temp_c = t;
                            }
                        }
                        CH_TARGET_TEMP => {
                            if let Some(t) = as_f64(v) {
                                st.target_temp_c = t;
                            }
                        }
                        CH_TEMP_UNITS => {
                            st.temp_units_f = as_u8(v).map(|u| u == 1).unwrap_or(false);
                        }
                        CH_HEAT_TO => {
                            if let Some(t) = as_f64(v) {
                                st.heat_to_c = Some(t);
                            }
                        }
                        CH_COOL_TO => {
                            if let Some(t) = as_f64(v) {
                                st.cool_to_c = Some(t);
                            }
                        }
                        CH_HUMIDITY => {
                            if let Some(h) = as_f64(v) {
                                st.humidity = Some(h);
                            }
                        }
                        _ => {}
                    }
                }
            }
            if stype == SVC_FAN {
                for ch in &svc.characteristics {
                    if short_type(&ch.type_uuid) == CH_FAN_ACTIVE {
                        // Address recorded even without a value, so the
                        // overlay can read it via /characteristics.
                        st.w_fan_active = Some((acc.aid, ch.iid));
                        if let Some(v) = &ch.value {
                            st.fan_on = as_u8(v).map(|u| u != 0);
                        }
                    }
                }
            }
        }
    }
    st
}

// ---------------------------------------------------------------------------
// High-level operations used by the UI threads.
// ---------------------------------------------------------------------------

/// Optional changes for a set_state call. All temperatures are Celsius;
/// the UI converts from Fahrenheit before calling.
#[derive(Clone, Debug, Default)]
pub struct ThermostatChanges {
    pub target_mode: Option<u8>,
    pub target_temp_c: Option<f64>,
    pub heat_to_c: Option<f64>,
    pub cool_to_c: Option<f64>,
    pub fan_on: Option<bool>,
}

/// Apply /characteristics values onto a state mapped from
/// /accessories, using the same per-type mapping in both directions.
/// A value present in the overlay wins over the /accessories value;
/// a missing entry leaves the /accessories value untouched.
fn overlay_values(
    st: &mut ThermostatState,
    vals: &HashMap<(u32, u32), serde_json::Value>,
) {
    if let Some(t) = st.r_current_mode {
        if let Some(v) = vals.get(&t) {
            if let Some(m) = as_u8(v) {
                st.current_mode = m;
            }
        }
    }
    if let Some(t) = st.w_target_mode {
        if let Some(v) = vals.get(&t) {
            if let Some(m) = as_u8(v) {
                st.target_mode = m;
            }
        }
    }
    if let Some(t) = st.r_current_temp {
        if let Some(v) = vals.get(&t) {
            if let Some(t) = as_f64(v) {
                st.current_temp_c = t;
            }
        }
    }
    if let Some(t) = st.w_target_temp {
        if let Some(v) = vals.get(&t) {
            if let Some(t) = as_f64(v) {
                st.target_temp_c = t;
            }
        }
    }
    if let Some(t) = st.w_heat_to {
        if let Some(v) = vals.get(&t) {
            if let Some(t) = as_f64(v) {
                st.heat_to_c = Some(t);
            }
        }
    }
    if let Some(t) = st.w_cool_to {
        if let Some(v) = vals.get(&t) {
            if let Some(t) = as_f64(v) {
                st.cool_to_c = Some(t);
            }
        }
    }
    if let Some(t) = st.r_humidity {
        if let Some(v) = vals.get(&t) {
            if let Some(h) = as_f64(v) {
                st.humidity = Some(h);
            }
        }
    }
    if let Some(t) = st.r_units {
        if let Some(v) = vals.get(&t) {
            st.temp_units_f = as_u8(v).map(|u| u == 1).unwrap_or(false);
        }
    }
    if let Some(t) = st.w_fan_active {
        if let Some(v) = vals.get(&t) {
            st.fan_on = as_u8(v).map(|u| u != 0);
        }
    }
}

/// One session's full read: /accessories mapping plus a
/// /characteristics overlay for every discovered target. The overlay
/// never fails the fetch: if the accessory won't answer
/// /characteristics, the /accessories-derived state stands.
fn fetch_via(sess: &mut SecureConn) -> HapResult<ThermostatState> {
    let accs = sess.get_accessories()?;
    let mut st = state_from_accessories(&accs);
    let mut ids: Vec<(u32, u32)> = Vec::new();
    for t in [
        st.r_current_mode,
        st.w_target_mode,
        st.r_current_temp,
        st.w_target_temp,
        st.w_heat_to,
        st.w_cool_to,
        st.r_humidity,
        st.r_units,
        st.w_fan_active,
    ] {
        if let Some(id) = t {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    if !ids.is_empty() {
        if let Ok(vals) = sess.get_characteristics(&ids) {
            overlay_values(&mut st, &vals);
        }
    }
    Ok(st)
}

fn fetch_state_inner(pairing: &ThermostatPairing) -> HapResult<ThermostatState> {
    let mut sess = SecureConn::open(pairing)?;
    fetch_via(&mut sess)
}

/// Fetch the thermostat state, with one automatic retry on IO failure
/// (e.g. a stale connection) using a fresh session. Counters are never
/// reused across sessions -- the retry opens a brand-new pair-verify.
/// Logs the outcome + duration (modes and temperatures only).
pub fn fetch_state(pairing: &ThermostatPairing) -> HapResult<ThermostatState> {
    let started = std::time::Instant::now();
    let res = match fetch_state_inner(pairing) {
        Ok(s) => Ok(s),
        Err(HapError::Io(_)) => fetch_state_inner(pairing),
        Err(e) => Err(e),
    };
    match &res {
        Ok(st) => tstat_log(&format!(
            "Fetch ok: {} indoor {:.0}°F setpoint {:.0}°F ({:.1}s)",
            mode_name(st.target_mode),
            c_to_f(st.current_temp_c),
            c_to_f(st.target_temp_c),
            started.elapsed().as_secs_f64()
        )),
        Err(e) => tstat_log(&format!(
            "Fetch failed: {e} ({:.1}s)",
            started.elapsed().as_secs_f64()
        )),
    }
    res
}

/// Quantize a Celsius write to the characteristic's accepted step
/// (round to nearest step; fall back to 0.5 °C when the accessory
/// reports no step or a non-positive one), then clamp to its
/// reported range. The T6 rejects unrounded values (e.g. 78 °F ->
/// 25.555... °C) with HAP status -70410 (Invalid Value).
pub fn quantize_temp(value: f64, step: Option<f64>, min: Option<f64>, max: Option<f64>) -> f64 {
    let step = step.filter(|s| *s > 0.0).unwrap_or(0.5);
    let mut v = (value / step).round() * step;
    // Scrub binary float dust (25.500000000000004 -> 25.5) so the
    // JSON value is exactly the stepped value.
    v = (v * 1_000_000_000.0).round() / 1_000_000_000.0;
    if let Some(lo) = min {
        if v < lo {
            v = lo;
        }
    }
    if let Some(hi) = max {
        if v > hi {
            v = hi;
        }
    }
    v
}

/// Outcome of a set: Confirmed means the post-PUT readback succeeded
/// (the state carries real read values); Optimistic means the PUT
/// succeeded but the readback failed, so the state is the pre-write
/// state patched with exactly the values that were PUT (never
/// invented values) — the caller must re-fetch promptly to confirm.
pub enum SetOutcome {
    Confirmed(ThermostatState),
    Optimistic(ThermostatState),
}

/// Patch a state with the values from a writes list, matching each
/// write's (aid, iid) against the state's write targets. Writes to
/// unknown addresses are ignored; fields with no write keep their
/// current value.
fn apply_writes_to_state(st: &mut ThermostatState, writes: &[(u32, u32, serde_json::Value)]) {
    for (a, i, v) in writes {
        let addr = (*a, *i);
        if Some(addr) == st.w_target_mode {
            if let Some(m) = as_u8(v) {
                st.target_mode = m;
            }
        } else if Some(addr) == st.w_target_temp {
            if let Some(t) = as_f64(v) {
                st.target_temp_c = t;
            }
        } else if Some(addr) == st.w_heat_to {
            if let Some(t) = as_f64(v) {
                st.heat_to_c = Some(t);
            }
        } else if Some(addr) == st.w_cool_to {
            if let Some(t) = as_f64(v) {
                st.cool_to_c = Some(t);
            }
        } else if Some(addr) == st.w_fan_active {
            st.fan_on = as_u8(v).map(|u| u != 0);
        }
    }
}

/// Apply changes, then read back exactly the written characteristics
/// and report the actual values (the returned state reflects the
/// readback). If the PUT succeeds but the readback fails, the set
/// still counts as applied — the returned state is the intended one
/// (Optimistic) instead of an error, because the device already
/// applied the write; failing here is what left the tile stale
/// until a much later autorefresh.
pub fn set_state(
    pairing: &ThermostatPairing,
    changes: &ThermostatChanges,
) -> HapResult<SetOutcome> {
    let started = std::time::Instant::now();
    let mut sess = SecureConn::open(pairing)?;
    let mut st = fetch_via(&mut sess)?;
    let mut writes: Vec<(u32, u32, serde_json::Value)> = Vec::new();
    let mut requested: Vec<String> = Vec::new();
    if let Some(m) = changes.target_mode {
        if let Some(t) = st.w_target_mode {
            writes.push((t.0, t.1, serde_json::json!(m)));
            requested.push(format!("mode={}", mode_name(m)));
        }
    }
    if let Some(t) = changes.target_temp_c {
        if let Some(w) = st.w_target_temp {
            let q = quantize_temp(t, st.step_target_temp, st.min_target_temp, st.max_target_temp);
            writes.push((w.0, w.1, serde_json::json!(q)));
            requested.push(format!("target={q}°C (req {t}°C)"));
        }
    }
    if let Some(t) = changes.heat_to_c {
        if let Some(w) = st.w_heat_to {
            let q = quantize_temp(t, st.step_heat_to, st.min_heat_to, st.max_heat_to);
            writes.push((w.0, w.1, serde_json::json!(q)));
            requested.push(format!("heat-to={q}°C (req {t}°C)"));
        }
    }
    if let Some(t) = changes.cool_to_c {
        if let Some(w) = st.w_cool_to {
            let q = quantize_temp(t, st.step_cool_to, st.min_cool_to, st.max_cool_to);
            writes.push((w.0, w.1, serde_json::json!(q)));
            requested.push(format!("cool-to={q}°C (req {t}°C)"));
        }
    }
    if let Some(on) = changes.fan_on {
        if let Some(w) = st.w_fan_active {
            writes.push((w.0, w.1, serde_json::json!(if on { 1 } else { 0 })));
            requested.push(format!("fan={}", if on { "on" } else { "auto" }));
        }
    }
    if writes.is_empty() {
        return Ok(SetOutcome::Confirmed(st));
    }
    let secs = || started.elapsed().as_secs_f64();
    let failures = sess.put_characteristics(&writes)?;
    if !failures.is_empty() {
        let desc: Vec<String> = failures
            .iter()
            .map(|(a, i, s)| format!("{a}.{i} -> status {s}"))
            .collect();
        let msg = format!("thermostat rejected write(s): {}", desc.join(", "));
        tstat_log(&set_log_line(
            &requested.join(", "),
            &format!("PUT rejected: {}", desc.join(", ")),
            "no readback",
            secs(),
        ));
        return err(msg);
    }
    // Read back the written characteristics and patch the state with
    // the same overlay mapping the fetch path uses.
    let ids: Vec<(u32, u32)> = writes.iter().map(|(a, i, _)| (*a, *i)).collect();
    match sess.get_characteristics(&ids) {
        Ok(back) => {
            overlay_values(&mut st, &back);
            tstat_log(&set_log_line(&requested.join(", "), "PUT ok", "readback ok", secs()));
            Ok(SetOutcome::Confirmed(st))
        }
        Err(e) => {
            // The PUT already applied on the device (a mode change
            // shows instantly on the unit) — failing the whole set
            // here is what left the tile stale until autorefresh.
            // Return the intended state instead; the caller
            // re-fetches promptly to confirm.
            let mut intended = st;
            apply_writes_to_state(&mut intended, &writes);
            tstat_log(&set_log_line(
                &requested.join(", "),
                "PUT ok",
                &format!("readback failed ({e}) — optimistic state"),
                secs(),
            ));
            Ok(SetOutcome::Optimistic(intended))
        }
    }
}

// ---------------------------------------------------------------------------
// Discovery via mDNS (_hap._tcp). Status flag bit 0x01 = unpaired.
// ---------------------------------------------------------------------------

// Discovery record: id/category/flags/counters are the accessory's
// advertised TXT record, kept whole for the UI and the Mac edition
// even though pairing itself keys off the addresses + port.
#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct DiscoveredDevice {
    pub id: String,
    pub name: String,
    pub model: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub unpaired: bool,
    pub category: u32,
    pub feature_flags: u32,
    pub config_num: u32,
    pub state_num: u32,
    pub protocol_version: String,
}

/// Discovery via mDNS, with a diagnostics-log entry for the outcome.
/// Device names/models come from the mDNS TXT record (not secrets).
pub fn discover(timeout: Duration) -> HapResult<Vec<DiscoveredDevice>> {
    let res = discover_inner(timeout);
    match &res {
        Ok(devs) => {
            if devs.is_empty() {
                tstat_log("Discovery: no devices found");
            } else {
                let names: Vec<String> = devs
                    .iter()
                    .map(|d| format!("{} ({})", d.name, d.model))
                    .collect();
                tstat_log(&format!("Discovery: {} device(s): {}", devs.len(), names.join(", ")));
            }
        }
        Err(e) => tstat_log(&format!("Discovery failed: {e}")),
    }
    res
}

fn discover_inner(timeout: Duration) -> HapResult<Vec<DiscoveredDevice>> {
    use mdns_sd::{ServiceDaemon, ServiceEvent};
    use std::time::Instant;

    let daemon = ServiceDaemon::new()
        .map_err(|e| HapError::Msg(format!("mDNS service failed to start: {}", e)))?;
    let receiver = daemon
        .browse("_hap._tcp.local.")
        .map_err(|e| HapError::Msg(format!("mDNS browse failed: {}", e)))?;
    let deadline = Instant::now() + timeout;
    let mut found: HashMap<String, DiscoveredDevice> = HashMap::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(svc)) => {
                let txt = &svc.txt_properties;
                let get = |k: &str| txt.get_property_val_str(k).unwrap_or("").to_string();
                let parse_u32 = |k: &str| get(k).parse::<u32>().unwrap_or(0);
                let mut addresses: Vec<String> = svc
                    .addresses
                    .iter()
                    .map(|a| a.to_ip_addr().to_string())
                    .collect();
                // Prefer IPv4 first (most home accessories answer on v4).
                addresses.sort_by_key(|a| !a.contains('.'));
                let fullname = svc.fullname.clone();
                let name = fullname
                    .strip_suffix("._hap._tcp.local.")
                    .unwrap_or(&fullname)
                    .to_string();
                let id = get("id");
                let dev = DiscoveredDevice {
                    id: id.clone(),
                    name,
                    model: get("md"),
                    addresses,
                    port: svc.port,
                    unpaired: parse_u32("sf") & 0x01 != 0,
                    category: parse_u32("ci"),
                    feature_flags: parse_u32("ff"),
                    config_num: parse_u32("c#"),
                    state_num: parse_u32("s#"),
                    protocol_version: get("pv"),
                };
                found.insert(if id.is_empty() { fullname } else { id }, dev);
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.shutdown();
    let mut out: Vec<DiscoveredDevice> = found.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

// ---------------------------------------------------------------------------
// Unit tests (no hardware needed). The SRP consistency test implements a
// minimal server side from the same formulas and asserts client and
// server arrive at the same session key and verify each other's proofs.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn tlv_roundtrip() {
        let items = vec![
            (TAG_STATE, vec![STATE_M1]),
            (TAG_METHOD, vec![0x00]),
            (TAG_IDENTIFIER, b"test-id".to_vec()),
        ];
        let refs: Vec<(u8, &[u8])> =
            items.iter().map(|(t, v)| (*t, v.as_slice())).collect();
        let enc = tlv_encode(&refs);
        let dec = tlv_decode(&enc).unwrap();
        assert_eq!(dec, items);
    }

    #[test]
    fn tlv_chunking_boundaries() {
        // 300 bytes -> 255 + 45; 255 exact -> one piece; 256 -> 255 + 1;
        // 510 -> 255 + 255.
        for (len, expected_pieces) in [(300usize, 2), (255, 1), (256, 2), (510, 2)] {
            let value = vec![0xABu8; len];
            let enc = tlv_encode(&[(TAG_PUBLIC_KEY, &value)]);
            let dec = tlv_decode(&enc).unwrap();
            assert_eq!(dec.len(), 1, "len {}", len);
            assert_eq!(dec[0].1, value, "len {}", len);
            // Count encoded pieces by re-walking the raw bytes.
            let mut pieces = 0;
            let mut i = 0;
            while i < enc.len() {
                let l = enc[i + 1] as usize;
                i += 2 + l;
                pieces += 1;
            }
            assert_eq!(pieces, expected_pieces, "len {}", len);
        }
        // Empty value still emits tag + zero length.
        let enc = tlv_encode(&[(TAG_STATE, &[])]);
        assert_eq!(enc, vec![TAG_STATE, 0]);
        assert_eq!(tlv_decode(&enc).unwrap(), vec![(TAG_STATE, vec![])]);
    }

    #[test]
    fn tlv_error_tag_and_truncation() {
        let items = vec![(TAG_STATE, vec![STATE_M4]), (TAG_ERROR, vec![2u8])];
        let refs: Vec<(u8, &[u8])> =
            items.iter().map(|(t, v)| (*t, v.as_slice())).collect();
        let enc = tlv_encode(&refs);
        let dec = tlv_decode(&enc).unwrap();
        let err = tlv_check_no_error(&dec).unwrap_err();
        assert!(format!("{}", err).contains("authentication"));
        // Truncated header and truncated value are hard errors.
        assert!(tlv_decode(&[0x06]).is_err());
        assert!(tlv_decode(&[0x06, 0x05, 0x01]).is_err());
        assert!(tlv_decode(&[0x06, 0x01]).is_err());
    }

    #[test]
    fn pin_normalization() {
        assert_eq!(normalize_setup_code("12345678").unwrap(), "123-45-678");
        assert_eq!(normalize_setup_code("123-45-678").unwrap(), "123-45-678");
        // Same digits, different dash grouping: still the same code.
        assert_eq!(normalize_setup_code("1234-5678").unwrap(), "123-45-678");
        assert!(normalize_setup_code("1234567").is_err());
        assert!(normalize_setup_code("123456789").is_err());
        assert!(normalize_setup_code("1234abcd").is_err());
        assert!(normalize_setup_code("").is_err());
    }

    #[test]
    fn temp_write_quantization() {
        // v1.6.4: the field case — 78 °F converts to 25.555... °C,
        // which the T6 rejects (-70410); a 0.5 °C step turns it into
        // 25.5, the value the unit accepts (and displays as 78 °F).
        assert_eq!(quantize_temp(f_to_c(78.0), Some(0.5), None, None), 25.5);
        assert_eq!(quantize_temp(25.555555555555557, Some(0.5), None, None), 25.5);
        // No step reported: fall back to 0.5 °C steps.
        assert_eq!(quantize_temp(25.555555555555557, None, None, None), 25.5);
        // Non-positive steps are garbage: same fallback.
        assert_eq!(quantize_temp(25.555555555555557, Some(0.0), None, None), 25.5);
        assert_eq!(quantize_temp(25.555555555555557, Some(-1.0), None, None), 25.5);
        // Other steps and on-step values.
        assert_eq!(quantize_temp(25.555555555555557, Some(1.0), None, None), 26.0);
        assert_eq!(quantize_temp(25.5, Some(0.5), None, None), 25.5);
        assert_eq!(quantize_temp(22.0, Some(0.5), None, None), 22.0);
        // Clamping to the reported range (after rounding).
        assert_eq!(quantize_temp(5.0, Some(0.5), Some(10.0), Some(30.0)), 10.0);
        assert_eq!(quantize_temp(35.0, Some(0.5), Some(10.0), Some(30.0)), 30.0);
        assert_eq!(quantize_temp(25.555555555555557, Some(0.5), Some(10.0), Some(30.0)), 25.5);
    }

    #[test]
    fn thermostat_step_range_recorded_from_accessories() {
        // v1.6.4: step/range travel with the write target, and old
        // fixtures without them still load (serde defaults -> None).
        let json = serde_json::json!({
            "aid": 1,
            "services": [{
                "iid": 10,
                "type": "0000004A-0000-1000-8000-0026BB765291",
                "characteristics": [
                    { "iid": 14, "type": "00000035-0000-1000-8000-0026BB765291", "value": 22.0,
                      "minStep": 0.5, "minValue": 10.0, "maxValue": 30.0 },
                    { "iid": 16, "type": "00000012-0000-1000-8000-0026BB765291", "value": 21.0,
                      "minStep": 1.0 },
                    { "iid": 17, "type": "0000000D-0000-1000-8000-0026BB765291", "value": 24.0 }
                ]
            }]
        });
        let acc: AccessoryInfo = serde_json::from_value(json).unwrap();
        let st = state_from_accessories(&[acc]);
        assert_eq!(st.w_target_temp, Some((1, 14)));
        assert_eq!(st.step_target_temp, Some(0.5));
        assert_eq!(st.min_target_temp, Some(10.0));
        assert_eq!(st.max_target_temp, Some(30.0));
        assert_eq!(st.step_heat_to, Some(1.0));
        assert_eq!(st.min_heat_to, None);
        assert_eq!(st.step_cool_to, None);
        assert_eq!(st.min_cool_to, None);
        assert_eq!(st.max_cool_to, None);
    }

    #[test]
    fn optimistic_state_patches_only_written_fields() {
        // v1.6.4: the readback-failure fallback patches exactly the
        // values that were PUT — nothing else, nothing invented.
        let mut st = ThermostatState {
            target_mode: 1,
            target_temp_c: 21.0,
            heat_to_c: Some(20.0),
            cool_to_c: Some(24.0),
            fan_on: Some(false),
            w_target_mode: Some((1, 12)),
            w_target_temp: Some((1, 14)),
            w_heat_to: Some((1, 16)),
            w_cool_to: Some((1, 17)),
            w_fan_active: Some((1, 21)),
            ..Default::default()
        };
        let writes = vec![
            (1u32, 12u32, serde_json::json!(2)),
            (1u32, 14u32, serde_json::json!(25.5)),
            // An address this state has no target for: ignored.
            (1u32, 99u32, serde_json::json!(0)),
        ];
        apply_writes_to_state(&mut st, &writes);
        assert_eq!(st.target_mode, 2);
        assert_eq!(st.target_temp_c, 25.5);
        // No write for these: untouched.
        assert_eq!(st.heat_to_c, Some(20.0));
        assert_eq!(st.cool_to_c, Some(24.0));
        assert_eq!(st.fan_on, Some(false));
    }

    #[test]
    fn tstat_log_ring_cap_evicts_oldest() {
        // Only this test pushes to the shared buffer (it clears
        // first); the assertions are written to survive one foreign
        // line from the unreachable-host test landing anywhere.
        tstat_log_clear();
        for i in 0..205 {
            tstat_log(&format!("entry {i:03}"));
        }
        let text = tstat_log_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 200);
        // The newest entry always survives; the five oldest of this
        // test's own pushes are evicted by its own later pushes no
        // matter where a foreign line interleaves.
        assert!(text.contains("entry 204"));
        for i in 0..5 {
            assert!(!text.contains(&format!("entry {i:03}")));
        }
    }

    #[test]
    fn tstat_log_lines_carry_no_secret_material() {
        // Regression guard for the v1.6.4 logger: the line builders
        // take no code/key parameter at all (structural guarantee),
        // and their output for representative inputs contains
        // neither a setup code nor key material. The call sites pass
        // outcomes, temperatures, modes, ids, and statuses only.
        let pin = "123-45-678";
        let key_hex = "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899";
        let begin = pair_begin_log_line("Tstat-79E7F2");
        assert!(begin.contains("Tstat-79E7F2"));
        assert!(!begin.contains(pin));
        assert!(!begin.contains(key_hex));
        let finish_ok = pair_finish_log_line(true, None);
        let finish_err = pair_finish_log_line(false, Some("accessory proof mismatch -- wrong setup code?"));
        for line in [&finish_ok, &finish_err] {
            assert!(!line.contains(pin));
            assert!(!line.contains(key_hex));
        }
        let set = set_log_line(
            "mode=Cool, target=25.5°C (req 25.555555555555557°C)",
            "PUT rejected: 1.278 -> status -70410",
            "no readback",
            1.2,
        );
        assert!(set.contains("25.5°C"));
        assert!(set.contains("1.278 -> status -70410"));
        assert!(!set.contains(pin));
        assert!(!set.contains(key_hex));
    }

    #[test]
    fn temperature_conversions() {
        assert_eq!(c_to_f(0.0), 32.0);
        assert_eq!(f_to_c(32.0), 0.0);
        assert!((c_to_f(22.2222) - 72.0).abs() < 0.01);
        assert!((f_to_c(72.0) - 22.2222).abs() < 0.01);
        assert!((f_to_c(c_to_f(68.5)) - 68.5).abs() < 1e-9);
    }

    #[test]
    fn mode_names() {
        assert_eq!(mode_name(0), "Off");
        assert_eq!(mode_name(1), "Heat");
        assert_eq!(mode_name(2), "Cool");
        assert_eq!(mode_name(3), "Auto");
        assert_eq!(mode_name(9), "?");
    }

    #[test]
    fn hex_helpers() {
        assert_eq!(hex_encode(&[0x00, 0xAB, 0xFF]), "00abff");
        assert_eq!(hex_decode("00abff").unwrap(), vec![0x00, 0xAB, 0xFF]);
        assert!(hex_decode("abc").is_err());
        assert!(hex_decode("zz").is_err());
        assert!(hex_decode_32(&"ab".repeat(32)).is_ok());
        assert!(hex_decode_32(&"ab".repeat(31)).is_err());
    }

    #[test]
    fn pairing_partial_json_defaults() {
        // An old/partial pairing block must still parse; validation (not
        // parsing) is what rejects it.
        let p: ThermostatPairing =
            serde_json::from_str(r#"{"accessory_ip":"192.168.1.5"}"#).unwrap();
        assert_eq!(p.accessory_ip, "192.168.1.5");
        assert_eq!(p.accessory_port, 0);
        assert!(p.ios_pairing_id.is_empty());
        assert!(p.validate().is_err());
        assert!(ThermostatPairing::default().validate().is_err());
    }

    #[test]
    fn unreachable_host_errors_not_panics() {
        // Port 1 on loopback refuses immediately; must be Err, not panic.
        assert!(Conn::connect("127.0.0.1", 1).is_err());
        assert!(fetch_state(&ThermostatPairing::default()).is_err());
        assert!(set_state(&ThermostatPairing::default(), &ThermostatChanges::default()).is_err());
    }

    #[test]
    fn srp_client_server_consistency() {
        let pin = "123-45-678";
        let salt = [0x42u8; 16];
        let client = SrpClient::new().unwrap();
        let x = SrpClient::compute_x(&salt, pin);
        let n = srp_n();
        let k = srp_k();
        let g = srp_g();
        let v = g.modpow(&x, &n);
        // Server side: pick b, B = (k*v + g^b) mod N.
        let mut bb = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bb);
        let b = BigUint::from_bytes_be(&bb);
        let b_pub = g.modpow(&b, &n);
        let big_b = (&k * &v + &b_pub) % &n;
        let b_raw = pad384(&big_b.to_bytes_be()).unwrap();

        let k_client = client.session(&salt, &b_raw, pin).unwrap();
        // Server: S = (A * v^u)^b mod N, K = SHA512(S padded).
        let mut u_msg = Vec::new();
        u_msg.extend_from_slice(client.public_bytes());
        u_msg.extend_from_slice(&b_raw);
        let u = BigUint::from_bytes_be(&sha512(&u_msg));
        let a_int = BigUint::from_bytes_be(client.public_bytes());
        let s_server = (a_int * v.modpow(&u, &n)).modpow(&b, &n);
        let k_server = sha512(&pad384(&s_server.to_bytes_be()).unwrap());
        assert_eq!(k_client, k_server, "client and server session keys differ");

        // Client proof verified by the server's recomputation.
        let m = client.client_proof(&salt, &b_raw, &k_client);
        let h_n = sha512(&n.to_bytes_be());
        let h_g = sha512(&[0x05u8]);
        let mut xor = [0u8; 64];
        for i in 0..64 {
            xor[i] = h_n[i] ^ h_g[i];
        }
        let mut m_msg = Vec::new();
        m_msg.extend_from_slice(&xor);
        m_msg.extend_from_slice(&sha512(b"Pair-Setup"));
        m_msg.extend_from_slice(&pad16(&salt));
        m_msg.extend_from_slice(client.public_bytes());
        m_msg.extend_from_slice(&b_raw);
        m_msg.extend_from_slice(&k_server);
        assert_eq!(sha512(&m_msg), m);

        // Server proof verified by the client.
        let mut sp_msg = Vec::new();
        sp_msg.extend_from_slice(client.public_bytes());
        sp_msg.extend_from_slice(&m);
        sp_msg.extend_from_slice(&k_server);
        let server_proof = sha512(&sp_msg);
        assert!(client.verify_server_proof(&m, &k_client, &server_proof));

        // Wrong pin must NOT agree with the server.
        let k_wrong = client.session(&salt, &b_raw, "123-45-679").unwrap();
        assert_ne!(k_wrong, k_server);
    }

    #[test]
    fn hkdf_determinism_and_separation() {
        let ikm = [0x77u8; 64];
        let a = hkdf_sha512(&ikm, b"salt", b"info", 32).unwrap();
        let b = hkdf_sha512(&ikm, b"salt", b"info", 32).unwrap();
        assert_eq!(a, b);
        let c = hkdf_sha512(&ikm, b"other", b"info", 32).unwrap();
        assert_ne!(a, c);
        let d = hkdf_sha512(&ikm, b"salt", b"other", 32).unwrap();
        assert_ne!(a, d);
    }

    #[test]
    fn ed25519_seed_roundtrip() {
        let mut seed = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut seed);
        let sk = SigningKey::from_bytes(&seed);
        let pk = sk.verifying_key().to_bytes();
        let msg = b"hello hap";
        let sig = sk.sign(msg).to_bytes();
        let vk = VerifyingKey::from_bytes(&pk).unwrap();
        let sig_obj = Signature::try_from(sig.as_slice()).unwrap();
        vk.verify(msg, &sig_obj).unwrap();
        // Reconstructing from the seed gives the same public key.
        let sk2 = SigningKey::from_bytes(&seed);
        assert_eq!(sk2.verifying_key().to_bytes(), pk);
    }

    #[test]
    fn x25519_dh_symmetry() {
        let a = EphemeralSecret::random_from_rng(rand::thread_rng());
        let b = EphemeralSecret::random_from_rng(rand::thread_rng());
        let pa = XPublicKey::from(&a);
        let pb = XPublicKey::from(&b);
        assert_eq!(
            a.diffie_hellman(&pb).to_bytes(),
            b.diffie_hellman(&pa).to_bytes()
        );
    }

    #[test]
    fn chacha_pairing_nonce_roundtrip() {
        let key = [0x33u8; 32];
        let nonce = pairing_nonce(b"PS-Msg05");
        assert_eq!(&nonce[..4], &[0u8; 4]);
        assert_eq!(&nonce[4..], b"PS-Msg05");
        let pt = b"sub-tlv bytes";
        let ct = chacha_encrypt(&key, &nonce, b"", pt).unwrap();
        assert_eq!(ct.len(), pt.len() + 16);
        let back = chacha_decrypt(&key, &nonce, b"", &ct).unwrap();
        assert_eq!(back, pt);
        // Wrong nonce or aad must fail.
        assert!(chacha_decrypt(&key, &pairing_nonce(b"PS-Msg06"), b"", &ct).is_err());
        assert!(chacha_decrypt(&key, &nonce, b"x", &ct).is_err());
    }

    #[test]
    fn frame_nonce_layout() {
        assert_eq!(frame_nonce(0), [0u8; 12]);
        let n1 = frame_nonce(1);
        assert_eq!(&n1[..4], &[0u8; 4]);
        assert_eq!(&n1[4..], &1u64.to_le_bytes());
        let n = frame_nonce(0x0102030405060708);
        assert_eq!(&n[4..], &[0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]);
    }

    #[test]
    fn secure_session_loopback_roundtrip() {
        // A fake accessory on loopback: reads framed requests (verifying
        // chunk size and sequential counters), skips in an EVENT frame,
        // then answers. Exercises Conn framing + HTTP parsing end to end.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let out_key = [0x11u8; 32];
        let in_key = [0x22u8; 32];
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
            let mut plain = Vec::new();
            let mut ctr = 0u64;
            loop {
                let mut lenb = [0u8; 2];
                s.read_exact(&mut lenb).unwrap();
                let len = u16::from_le_bytes(lenb) as usize;
                assert!(len <= 1024, "chunk exceeds 1024 bytes");
                let mut ct = vec![0u8; len + 16];
                s.read_exact(&mut ct).unwrap();
                let pt = chacha_decrypt(&out_key, &frame_nonce(ctr), &lenb, &ct).unwrap();
                ctr += 1;
                plain.extend_from_slice(&pt);
                if find_subslice(&plain, b"\r\n\r\n").is_some() {
                    break;
                }
            }
            assert!(plain.starts_with(b"GET /accessories"));
            assert!(plain.len() > 2048, "expected a multi-chunk request");
            assert_eq!(ctr, 3, "2500-byte request must take 3 frames");
            // EVENT frame first (nonce 0 on the inbound key), then the
            // real response (nonce 1). The client must skip the EVENT.
            let event = b"EVENT/1.0 200 OK\r\nContent-Length: 2\r\n\r\n{}";
            let elen = (event.len() as u16).to_le_bytes();
            let ect = chacha_encrypt(&in_key, &frame_nonce(0), &elen, event).unwrap();
            let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
            let rlen = (resp.len() as u16).to_le_bytes();
            let rct = chacha_encrypt(&in_key, &frame_nonce(1), &rlen, resp).unwrap();
            let mut frames = elen.to_vec();
            frames.extend_from_slice(&ect);
            frames.extend_from_slice(&rlen);
            frames.extend_from_slice(&rct);
            s.write_all(&frames).unwrap();
        });
        let mut conn = Conn::connect("127.0.0.1", port).unwrap();
        conn.set_secure(out_key, in_key).unwrap();
        // 2500-byte request: header + 'a' padding + terminator.
        let mut req = b"GET /accessories HTTP/1.1\r\nHost: x\r\nX-Pad: ".to_vec();
        req.extend_from_slice(&vec![b'a'; 2500 - req.len() - 4]);
        req.extend_from_slice(b"\r\n\r\n");
        assert_eq!(req.len(), 2500);
        let (code, body) = conn.request_secure(&req).unwrap();
        assert_eq!(code, 200);
        assert_eq!(body, b"{}");
        server.join().unwrap();
    }

    #[test]
    fn thermostat_state_mapping() {
        let json = serde_json::json!({
            "aid": 1,
            "services": [
                {
                    "iid": 1,
                    "type": "0000003E-0000-1000-8000-0026BB765291",
                    "characteristics": [
                        { "iid": 2, "type": "00000023-0000-1000-8000-0026BB765291", "value": "T6 Pro" }
                    ]
                },
                {
                    "iid": 10,
                    "type": "0000004A-0000-1000-8000-0026BB765291",
                    "characteristics": [
                        { "iid": 11, "type": "0000000F-0000-1000-8000-0026BB765291", "value": 1 },
                        { "iid": 12, "type": "00000033-0000-1000-8000-0026BB765291", "value": 1 },
                        { "iid": 13, "type": "00000011-0000-1000-8000-0026BB765291", "value": 21.5 },
                        { "iid": 14, "type": "00000035-0000-1000-8000-0026BB765291", "value": 22.0 },
                        { "iid": 15, "type": "00000036-0000-1000-8000-0026BB765291", "value": 1 },
                        { "iid": 16, "type": "00000012-0000-1000-8000-0026BB765291", "value": 21.0 },
                        { "iid": 17, "type": "0000000D-0000-1000-8000-0026BB765291", "value": 24.0 },
                        { "iid": 18, "type": "00000010-0000-1000-8000-0026BB765291", "value": 45 }
                    ]
                },
                {
                    "iid": 20,
                    "type": "00000040-0000-1000-8000-0026BB765291",
                    "characteristics": [
                        { "iid": 21, "type": "000000B0-0000-1000-8000-0026BB765291", "value": 1 }
                    ]
                }
            ]
        });
        let acc: AccessoryInfo = serde_json::from_value(json).unwrap();
        let st = state_from_accessories(&[acc]);
        assert_eq!(st.name, "T6 Pro");
        assert_eq!(st.current_mode, 1);
        assert_eq!(st.target_mode, 1);
        assert_eq!(st.current_temp_c, 21.5);
        assert_eq!(st.target_temp_c, 22.0);
        assert!(st.temp_units_f);
        assert_eq!(st.heat_to_c, Some(21.0));
        assert_eq!(st.cool_to_c, Some(24.0));
        assert_eq!(st.humidity, Some(45.0));
        assert_eq!(st.fan_on, Some(true));
        assert_eq!(st.w_target_mode, Some((1, 12)));
        assert_eq!(st.w_target_temp, Some((1, 14)));
        assert_eq!(st.w_fan_active, Some((1, 21)));
    }

    // -- Two-step pair-setup tests --------------------------------------
    // A minimal pair-setup accessory on loopback: answers M1..M6 with
    // server-side SRP computed from `correct_pin`. Handles exactly one
    // connection, then exits (EOF on cancel also exits it cleanly).

    fn read_http_request(s: &mut TcpStream) -> Option<Vec<u8>> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..pos]).into_owned();
                let mut content_len = 0usize;
                for line in head.lines().skip(1) {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("content-length") {
                            content_len = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
                let total = pos + 4 + content_len;
                while buf.len() < total {
                    match s.read(&mut chunk) {
                        Ok(0) => return None,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        Err(_) => return None,
                    }
                }
                return Some(buf[pos + 4..total].to_vec());
            }
            match s.read(&mut chunk) {
                Ok(0) => return None,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => return None,
            }
        }
    }

    fn write_http_response(s: &mut TcpStream, body: &[u8]) {
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
        s.write_all(head.as_bytes()).unwrap();
        s.write_all(body).unwrap();
    }

    fn spawn_fake_pair_server(
        correct_pin: &str,
        acc_id: &str,
        seed: [u8; 32],
    ) -> (u16, thread::JoinHandle<()>) {
        let correct_pin = correct_pin.to_string();
        let acc_id = acc_id.to_string();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
            // Server SRP state, filled at M1, consumed at M3.
            let mut srv: Option<(BigUint, Vec<u8>, BigUint, Vec<u8>)> = None;
            let mut k_session: Option<[u8; 64]> = None;
            loop {
                let body = match read_http_request(&mut s) {
                    Some(b) => b,
                    None => return,
                };
                let items = match tlv_decode(&body) {
                    Ok(i) => i,
                    Err(_) => return,
                };
                let state = tlv_get(&items, TAG_STATE)
                    .and_then(|v| v.first().copied())
                    .unwrap_or(0);
                match state {
                    STATE_M1 => {
                        let mut salt = [0u8; 16];
                        rand::thread_rng().fill_bytes(&mut salt);
                        let x = SrpClient::compute_x(&salt, &correct_pin);
                        let n = srp_n();
                        let k = srp_k();
                        let g = srp_g();
                        let v = g.modpow(&x, &n);
                        let mut bb = [0u8; 32];
                        rand::thread_rng().fill_bytes(&mut bb);
                        let b = BigUint::from_bytes_be(&bb);
                        let big_b = (&k * &v + g.modpow(&b, &n)) % &n;
                        let b_raw = pad384(&big_b.to_bytes_be()).unwrap();
                        srv = Some((b, b_raw.to_vec(), v, salt.to_vec()));
                        let refs = [
                            (TAG_STATE, &[STATE_M2][..]),
                            (TAG_SALT, &salt[..]),
                            (TAG_PUBLIC_KEY, &b_raw[..]),
                        ];
                        write_http_response(&mut s, &tlv_encode(&refs));
                    }
                    STATE_M3 => {
                        let (b, b_raw, v, salt) = match srv.take() {
                            Some(t) => t,
                            None => return,
                        };
                        let a_raw = tlv_get(&items, TAG_PUBLIC_KEY).unwrap().to_vec();
                        let m = tlv_get(&items, TAG_PROOF).unwrap().to_vec();
                        let n = srp_n();
                        let mut u_msg = Vec::new();
                        u_msg.extend_from_slice(&a_raw);
                        u_msg.extend_from_slice(&b_raw);
                        let u = BigUint::from_bytes_be(&sha512(&u_msg));
                        let a_int = BigUint::from_bytes_be(&a_raw);
                        let s_val = (a_int * v.modpow(&u, &n)).modpow(&b, &n);
                        let k = sha512(&pad384(&s_val.to_bytes_be()).unwrap());
                        let h_n = sha512(&n.to_bytes_be());
                        let h_g = sha512(&[0x05u8]);
                        let mut xor = [0u8; 64];
                        for i in 0..64 {
                            xor[i] = h_n[i] ^ h_g[i];
                        }
                        let mut m_msg = Vec::new();
                        m_msg.extend_from_slice(&xor);
                        m_msg.extend_from_slice(&sha512(b"Pair-Setup"));
                        m_msg.extend_from_slice(&pad16(&salt));
                        m_msg.extend_from_slice(&a_raw);
                        m_msg.extend_from_slice(&b_raw);
                        m_msg.extend_from_slice(&k);
                        if sha512(&m_msg).as_slice() != m.as_slice() {
                            let refs = [
                                (TAG_STATE, &[STATE_M4][..]),
                                (TAG_ERROR, &[2u8][..]),
                            ];
                            write_http_response(&mut s, &tlv_encode(&refs));
                            return;
                        }
                        k_session = Some(k);
                        let mut sp_msg = Vec::new();
                        sp_msg.extend_from_slice(&a_raw);
                        sp_msg.extend_from_slice(&m);
                        sp_msg.extend_from_slice(&k);
                        let server_proof = sha512(&sp_msg);
                        let refs = [
                            (TAG_STATE, &[STATE_M4][..]),
                            (TAG_PROOF, &server_proof[..]),
                        ];
                        write_http_response(&mut s, &tlv_encode(&refs));
                    }
                    STATE_M5 => {
                        let k = match k_session {
                            Some(k) => k,
                            None => return,
                        };
                        let enc = tlv_get(&items, TAG_ENCRYPTED_DATA).unwrap();
                        let enc_key = hkdf_sha512(
                            &k,
                            b"Pair-Setup-Encrypt-Salt",
                            b"Pair-Setup-Encrypt-Info",
                            32,
                        )
                        .unwrap();
                        let enc_arr: [u8; 32] = enc_key.try_into().unwrap();
                        let inner =
                            chacha_decrypt(&enc_arr, &pairing_nonce(b"PS-Msg05"), b"", enc)
                                .unwrap();
                        let sub = tlv_decode(&inner).unwrap();
                        let cid = tlv_get(&sub, TAG_IDENTIFIER).unwrap();
                        let cltpk = tlv_get(&sub, TAG_PUBLIC_KEY).unwrap();
                        let csig = tlv_get(&sub, TAG_SIGNATURE).unwrap();
                        let ios_x = hkdf_sha512(
                            &k,
                            b"Pair-Setup-Controller-Sign-Salt",
                            b"Pair-Setup-Controller-Sign-Info",
                            32,
                        )
                        .unwrap();
                        let mut info = Vec::new();
                        info.extend_from_slice(&ios_x);
                        info.extend_from_slice(cid);
                        info.extend_from_slice(cltpk);
                        let vk = VerifyingKey::from_bytes(
                            <&[u8; 32]>::try_from(cltpk).unwrap(),
                        )
                        .unwrap();
                        let sig = Signature::try_from(csig).unwrap();
                        vk.verify(&info, &sig).unwrap();
                        let acc_sk = SigningKey::from_bytes(&seed);
                        let acc_ltpk = acc_sk.verifying_key().to_bytes();
                        let acc_x = hkdf_sha512(
                            &k,
                            b"Pair-Setup-Accessory-Sign-Salt",
                            b"Pair-Setup-Accessory-Sign-Info",
                            32,
                        )
                        .unwrap();
                        let mut acc_info = Vec::new();
                        acc_info.extend_from_slice(&acc_x);
                        acc_info.extend_from_slice(acc_id.as_bytes());
                        acc_info.extend_from_slice(&acc_ltpk);
                        let acc_sig = acc_sk.sign(&acc_info).to_bytes();
                        let sub6 = tlv_encode(&[
                            (TAG_IDENTIFIER, acc_id.as_bytes()),
                            (TAG_PUBLIC_KEY, &acc_ltpk[..]),
                            (TAG_SIGNATURE, &acc_sig[..]),
                        ]);
                        let enc6 =
                            chacha_encrypt(&enc_arr, &pairing_nonce(b"PS-Msg06"), b"", &sub6)
                                .unwrap();
                        let refs = [
                            (TAG_STATE, &[STATE_M6][..]),
                            (TAG_ENCRYPTED_DATA, &enc6[..]),
                        ];
                        write_http_response(&mut s, &tlv_encode(&refs));
                        return;
                    }
                    _ => return,
                }
            }
        });
        (port, handle)
    }

    fn test_device(port: u16) -> DiscoveredDevice {
        DiscoveredDevice {
            id: "test-id".to_string(),
            name: "Test T6".to_string(),
            model: "T6".to_string(),
            addresses: vec!["127.0.0.1".to_string()],
            port,
            unpaired: true,
            category: 0,
            feature_flags: 0,
            config_num: 0,
            state_num: 0,
            protocol_version: "2".to_string(),
        }
    }

    #[test]
    fn two_step_pair_setup_success() {
        let pin = "123-45-678";
        let acc_id = "test-accessory-1";
        let seed = [0x5Au8; 32];
        let (port, server) = spawn_fake_pair_server(pin, acc_id, seed);
        let pending = begin_pair_setup(&test_device(port)).unwrap();
        // The pending state holds no pin, by construction and in
        // Debug output (the Debug impl prints identity only).
        let dbg = format!("{:?}", pending);
        assert!(!dbg.contains("123-45-678"), "debug leaks pin: {dbg}");
        assert!(!dbg.contains("12345678"), "debug leaks pin: {dbg}");
        assert!(dbg.contains("Test T6"), "debug: {dbg}");
        let pairing = finish_pair_setup(pending, pin).unwrap();
        assert_eq!(pairing.accessory_pairing_id, acc_id);
        let expect_ltpk =
            hex_encode(&SigningKey::from_bytes(&seed).verifying_key().to_bytes());
        assert_eq!(pairing.accessory_ltpk_hex, expect_ltpk);
        assert_eq!(pairing.accessory_ip, "127.0.0.1");
        assert_eq!(pairing.accessory_port, port);
        assert_eq!(pairing.accessory_name, "Test T6");
        server.join().unwrap();
    }

    #[test]
    fn two_step_pair_setup_wrong_code_fails_with_auth_error() {
        let (port, server) = spawn_fake_pair_server("123-45-678", "acc-1", [0x5Au8; 32]);
        let pending = begin_pair_setup(&test_device(port)).unwrap();
        let err = finish_pair_setup(pending, "123-45-679").unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("authentication"), "wrong-code error: {msg}");
        server.join().unwrap();
    }

    #[test]
    fn pending_drop_then_rebegin_succeeds() {
        // Cancel = drop: the server must see EOF and exit cleanly
        // (no panic on either side), and a fresh begin afterwards
        // must still pair.
        let pin = "123-45-678";
        let (port1, server1) = spawn_fake_pair_server(pin, "acc-1", [0x11u8; 32]);
        let pending = begin_pair_setup(&test_device(port1)).unwrap();
        drop(pending);
        server1.join().unwrap();
        let (port2, server2) = spawn_fake_pair_server(pin, "acc-2", [0x22u8; 32]);
        let pending2 = begin_pair_setup(&test_device(port2)).unwrap();
        let pairing = finish_pair_setup(pending2, pin).unwrap();
        assert_eq!(pairing.accessory_pairing_id, "acc-2");
        server2.join().unwrap();
    }

    // -- Chunked / headerless response tests ------------------------
    // A fake secure accessory for response-framing cases: the client
    // side is a real SecureConn (set_secure + get_accessories), so
    // these exercise the exact call path that failed in the field
    // ("bad accessory response: EOF while parsing a value").

    const TEST_OUT_KEY: [u8; 32] = [0x11u8; 32];
    const TEST_IN_KEY: [u8; 32] = [0x22u8; 32];

    fn secure_test_conn(port: u16) -> SecureConn {
        let mut conn = Conn::connect("127.0.0.1", port).unwrap();
        conn.set_secure(TEST_OUT_KEY, TEST_IN_KEY).unwrap();
        SecureConn { conn }
    }

    /// Server side: read one framed request (client -> server key),
    /// tracking the frame counter across calls.
    fn read_framed_request(
        s: &mut TcpStream,
        key: &[u8; 32],
        ctr: &mut u64,
    ) -> Vec<u8> {
        let mut plain = Vec::new();
        loop {
            let mut lenb = [0u8; 2];
            s.read_exact(&mut lenb).unwrap();
            let len = u16::from_le_bytes(lenb) as usize;
            let mut ct = vec![0u8; len + 16];
            s.read_exact(&mut ct).unwrap();
            let pt = chacha_decrypt(key, &frame_nonce(*ctr), &lenb, &ct).unwrap();
            *ctr += 1;
            plain.extend_from_slice(&pt);
            if find_subslice(&plain, b"\r\n\r\n").is_some() {
                return plain;
            }
        }
    }

    /// Server side: write plaintext as <=1000-byte frames (server ->
    /// client key), tracking the frame counter across calls.
    fn write_framed(s: &mut TcpStream, key: &[u8; 32], ctr: &mut u64, data: &[u8]) {
        for chunk in data.chunks(1000) {
            let lenb = (chunk.len() as u16).to_le_bytes();
            let ct = chacha_encrypt(key, &frame_nonce(*ctr), &lenb, chunk).unwrap();
            *ctr += 1;
            s.write_all(&lenb).unwrap();
            s.write_all(&ct).unwrap();
        }
    }

    /// Spawn a fake secure accessory: reads the first framed request
    /// (asserting it is a GET), then hands the stream plus both frame
    /// counters to `respond`, which may read further requests and
    /// write responses. Returning from `respond` closes the socket.
    fn spawn_secure_responder<F>(respond: F) -> (u16, thread::JoinHandle<()>)
    where
        F: FnOnce(&mut TcpStream, &mut u64, &mut u64) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
            let mut resp_ctr = 0u64;
            let mut req_ctr = 0u64;
            let req = read_framed_request(&mut s, &TEST_OUT_KEY, &mut req_ctr);
            assert!(
                req.starts_with(b"GET /accessories"),
                "unexpected first request"
            );
            respond(&mut s, &mut resp_ctr, &mut req_ctr);
        });
        (port, handle)
    }

    fn test_accessories_json() -> Vec<u8> {
        br#"{"accessories":[{"aid":1,"services":[{"iid":1,"type":"0000003E-0000-1000-8000-0026BB765291","characteristics":[{"iid":2,"type":"00000023-0000-1000-8000-0026BB765291","value":"T6 Pro"}]},{"iid":10,"type":"0000004A-0000-1000-8000-0026BB765291","characteristics":[{"iid":11,"type":"0000000F-0000-1000-8000-0026BB765291","value":1},{"iid":13,"type":"00000011-0000-1000-8000-0026BB765291","value":21.5}]}]}]}"#.to_vec()
    }

    /// Chunked transfer encoding with small chunks, a ;extension on
    /// the first chunk, and a trailer on the terminal chunk.
    fn chunked_encode(body: &[u8], chunk: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut first = true;
        for part in body.chunks(chunk) {
            if first {
                out.extend_from_slice(format!("{:x};ext=1\r\n", part.len()).as_bytes());
                first = false;
            } else {
                out.extend_from_slice(format!("{:x}\r\n", part.len()).as_bytes());
            }
            out.extend_from_slice(part);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"0\r\nX-Trailer: yes\r\n\r\n");
        out
    }

    #[test]
    fn secure_get_accessories_chunked() {
        // The field failure: the T6 answers GET /accessories without
        // Content-Length (chunked here); the old code read an empty
        // body and serde failed with EOF at line 1 column 0.
        let body = test_accessories_json();
        let mut resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        resp.extend_from_slice(&chunked_encode(&body, 13));
        let (port, server) = spawn_secure_responder(move |s, resp_ctr, _| {
            write_framed(s, &TEST_IN_KEY, resp_ctr, &resp);
        });
        let mut sess = secure_test_conn(port);
        let accs = sess.get_accessories().unwrap();
        assert_eq!(accs.len(), 1);
        assert_eq!(accs[0].aid, 1);
        assert_eq!(accs[0].services.len(), 2);
        server.join().unwrap();
    }

    #[test]
    fn secure_get_accessories_close_delimited() {
        // No framing header at all: the body runs to end-of-stream.
        // The server closes after writing; the client must take the
        // buffered remainder as the body, not an empty one.
        let body = test_accessories_json();
        let mut resp = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
        resp.extend_from_slice(&body);
        let (port, server) = spawn_secure_responder(move |s, resp_ctr, _| {
            write_framed(s, &TEST_IN_KEY, resp_ctr, &resp);
            // Returning closes the socket: end-of-stream for the body.
        });
        let mut sess = secure_test_conn(port);
        let accs = sess.get_accessories().unwrap();
        assert_eq!(accs.len(), 1);
        assert_eq!(accs[0].aid, 1);
        server.join().unwrap();
    }

    #[test]
    fn secure_get_accessories_content_length_regression() {
        let body = test_accessories_json();
        let mut resp =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len())
                .into_bytes();
        resp.extend_from_slice(&body);
        let (port, server) = spawn_secure_responder(move |s, resp_ctr, _| {
            write_framed(s, &TEST_IN_KEY, resp_ctr, &resp);
        });
        let mut sess = secure_test_conn(port);
        let accs = sess.get_accessories().unwrap();
        assert_eq!(accs.len(), 1);
        server.join().unwrap();
    }

    #[test]
    fn secure_get_accessories_large_chunked_multi_frame() {
        // ~3.5 KB body: chunked decode must survive the plaintext
        // arriving as several decrypted frames, not just one.
        let name = "x".repeat(3000);
        let body = format!(
            "{{\"accessories\":[{{\"aid\":1,\"services\":[{{\"iid\":1,\"type\":\"0000003E-0000-1000-8000-0026BB765291\",\"characteristics\":[{{\"iid\":2,\"type\":\"00000023-0000-1000-8000-0026BB765291\",\"value\":\"{}\"}}]}}]}}]}}",
            name
        )
        .into_bytes();
        assert!(body.len() > 3000, "test body must span frames");
        let mut resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        resp.extend_from_slice(&chunked_encode(&body, 64));
        let (port, server) = spawn_secure_responder(move |s, resp_ctr, _| {
            write_framed(s, &TEST_IN_KEY, resp_ctr, &resp);
        });
        let mut sess = secure_test_conn(port);
        let accs = sess.get_accessories().unwrap();
        assert_eq!(accs.len(), 1);
        let got = accs[0].services[0].characteristics[0]
            .value
            .as_ref()
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(got.len(), 3000);
        server.join().unwrap();
    }

    #[test]
    fn secure_chunked_then_content_length_keep_alive() {
        // Keep-alive integrity: after a chunked response, leftover
        // bytes (none here) and frame counters must line up so the
        // second request on the same connection still parses.
        let body = test_accessories_json();
        let mut chunked =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        chunked.extend_from_slice(&chunked_encode(&body, 17));
        let mut plain =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len())
                .into_bytes();
        plain.extend_from_slice(&body);
        let (port, server) = spawn_secure_responder(move |s, resp_ctr, req_ctr| {
            write_framed(s, &TEST_IN_KEY, resp_ctr, &chunked);
            let req2 = read_framed_request(s, &TEST_OUT_KEY, req_ctr);
            assert!(req2.starts_with(b"GET /accessories"));
            write_framed(s, &TEST_IN_KEY, resp_ctr, &plain);
        });
        let mut sess = secure_test_conn(port);
        let first = sess.get_accessories().unwrap();
        assert_eq!(first.len(), 1);
        let second = sess.get_accessories().unwrap();
        assert_eq!(second.len(), 1);
        server.join().unwrap();
    }

    #[test]
    fn secure_chunked_event_skipped() {
        // An unsolicited EVENT with a chunked body must be fully
        // consumed (chunks + trailers) before the real response.
        let body = test_accessories_json();
        let mut event = b"EVENT/1.0 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        event.extend_from_slice(&chunked_encode(b"{}", 1));
        let mut real =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len())
                .into_bytes();
        real.extend_from_slice(&body);
        let mut both = event;
        both.extend_from_slice(&real);
        let (port, server) = spawn_secure_responder(move |s, resp_ctr, _| {
            write_framed(s, &TEST_IN_KEY, resp_ctr, &both);
        });
        let mut sess = secure_test_conn(port);
        let accs = sess.get_accessories().unwrap();
        assert_eq!(accs.len(), 1);
        server.join().unwrap();
    }

    #[test]
    fn plain_chunked_response() {
        // The shared reader in the plain phase: a chunked response
        // must parse without waiting for connection close.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
            let _body = read_http_request(&mut s).unwrap();
            let mut resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
            resp.extend_from_slice(&chunked_encode(b"hello", 2));
            s.write_all(&resp).unwrap();
            // Keep the socket open: the client must finish on the
            // terminal chunk, not on EOF.
            std::thread::sleep(Duration::from_millis(200));
        });
        let mut conn = Conn::connect("127.0.0.1", port).unwrap();
        let (_code, body) = conn.post_plain("/x", b"", "text/plain").unwrap();
        assert_eq!(body, b"hello");
        server.join().unwrap();
    }

    // -- Thermostat state mapping: short UUIDs + value overlay -------
    // Field failure (v1.6.2): the tile rendered all defaults (Off,
    // 32°F/32°F = 0°C) while the wall unit showed Cool 77°F/77°F.
    // fetch_state had succeeded, but state_from_accessories matched
    // nothing: either the T6 uses short-form type UUIDs, or its
    // /accessories omits values (or both).

    #[test]
    fn short_type_normalization() {
        assert_eq!(short_type("4A"), "0000004A");
        assert_eq!(short_type("11"), "00000011");
        assert_eq!(short_type("004A"), "0000004A");
        assert_eq!(short_type("4a"), "0000004A");
        assert_eq!(short_type("0000004A"), "0000004A");
        assert_eq!(
            short_type("0000004A-0000-1000-8000-0026BB765291"),
            "0000004A"
        );
        assert_eq!(
            short_type("0000000d-0000-1000-8000-0026BB765291"),
            "0000000D"
        );
        assert_eq!(short_type(""), "00000000");
    }

    fn fetch_char_entry(iid: u32, typ: &str, value: Option<&str>) -> String {
        match value {
            Some(v) => format!(
                "{{\"iid\":{},\"type\":\"{}\",\"value\":{}}}",
                iid, typ, v
            ),
            None => format!("{{\"iid\":{},\"type\":\"{}\"}}", iid, typ),
        }
    }

    /// Full thermostat /accessories record. `short` selects short-form
    /// type UUIDs ("4A") vs full UUIDs; `with_values` includes the
    /// "value" key on every characteristic or omits it entirely.
    /// iid layout: 11 current mode, 12 target mode, 13 current temp,
    /// 14 target temp, 15 heat-to, 16 cool-to, 17 humidity, 18 units,
    /// 21 fan active. Values describe Cool, 25°C/25°C, heat 20°C,
    /// cool 26°C, 45% humidity, Celsius units, fan on.
    fn fetch_test_accessories(short: bool, with_values: bool) -> Vec<u8> {
        let ty = |full: &str, short_form: &str| -> String {
            if short {
                short_form.to_string()
            } else {
                format!("{}-0000-1000-8000-0026BB765291", full)
            }
        };
        let v = |val: &'static str| -> Option<&'static str> {
            if with_values {
                Some(val)
            } else {
                None
            }
        };
        let svc = |iid: u32, typ: &str, chars: Vec<String>| -> String {
            format!(
                "{{\"iid\":{},\"type\":\"{}\",\"characteristics\":[{}]}}",
                iid,
                typ,
                chars.join(",")
            )
        };
        let info = svc(
            1,
            &ty("0000003E", "3E"),
            vec![fetch_char_entry(2, &ty("00000023", "23"), v("\"T6 Pro\""))],
        );
        let tstat = svc(
            10,
            &ty("0000004A", "4A"),
            vec![
                fetch_char_entry(11, &ty("0000000F", "0F"), v("2")),
                fetch_char_entry(12, &ty("00000033", "33"), v("2")),
                fetch_char_entry(13, &ty("00000011", "11"), v("25.0")),
                fetch_char_entry(14, &ty("00000035", "35"), v("25.0")),
                fetch_char_entry(15, &ty("00000012", "12"), v("20.0")),
                fetch_char_entry(16, &ty("0000000D", "0D"), v("26.0")),
                fetch_char_entry(17, &ty("00000010", "10"), v("45")),
                fetch_char_entry(18, &ty("00000036", "36"), v("0")),
            ],
        );
        let fan = svc(
            20,
            &ty("00000040", "40"),
            vec![fetch_char_entry(21, &ty("000000B0", "B0"), v("1"))],
        );
        format!(
            "{{\"accessories\":[{{\"aid\":1,\"services\":[{},{},{}]}}]}}",
            info, tstat, fan
        )
        .into_bytes()
    }

    fn fetch_test_characteristics() -> Vec<u8> {
        br#"{"characteristics":[{"aid":1,"iid":11,"value":2},{"aid":1,"iid":12,"value":2},{"aid":1,"iid":13,"value":25.0},{"aid":1,"iid":14,"value":25.0},{"aid":1,"iid":15,"value":20.0},{"aid":1,"iid":16,"value":26.0},{"aid":1,"iid":17,"value":45},{"aid":1,"iid":18,"value":0},{"aid":1,"iid":21,"value":1}]}"#.to_vec()
    }

    /// Serve one fetch_via round-trip: /accessories (Content-Length),
    /// then the /characteristics overlay request. Asserts the overlay
    /// asks for exactly the nine discovered targets. `char_body` =
    /// None closes the socket on the overlay request instead of
    /// answering (overlay failure: the fetch must still succeed).
    fn spawn_fetch_responder(
        acc_body: Vec<u8>,
        char_body: Option<Vec<u8>>,
    ) -> (u16, thread::JoinHandle<()>) {
        spawn_secure_responder(move |s, resp_ctr, req_ctr| {
            let mut acc_resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                acc_body.len()
            )
            .into_bytes();
            acc_resp.extend_from_slice(&acc_body);
            write_framed(s, &TEST_IN_KEY, resp_ctr, &acc_resp);
            let req = read_framed_request(s, &TEST_OUT_KEY, req_ctr);
            assert!(
                req.starts_with(
                    b"GET /characteristics?id=1.11,1.12,1.13,1.14,1.15,1.16,1.17,1.18,1.21 "
                ),
                "unexpected overlay request: {}",
                String::from_utf8_lossy(&req)
            );
            if let Some(body) = char_body {
                let mut char_resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                char_resp.extend_from_slice(&body);
                write_framed(s, &TEST_IN_KEY, resp_ctr, &char_resp);
            }
            // Returning closes the socket.
        })
    }

    fn assert_cool_25(st: &ThermostatState, expected_name: &str) {
        assert_eq!(st.name, expected_name);
        assert_eq!(st.current_mode, 2);
        assert_eq!(st.target_mode, 2);
        assert_eq!(st.current_temp_c, 25.0);
        assert_eq!(st.target_temp_c, 25.0);
        assert_eq!(st.heat_to_c, Some(20.0));
        assert_eq!(st.cool_to_c, Some(26.0));
        assert_eq!(st.humidity, Some(45.0));
        assert!(!st.temp_units_f);
        assert_eq!(st.fan_on, Some(true));
        assert_eq!(st.r_current_mode, Some((1, 11)));
        assert_eq!(st.w_target_mode, Some((1, 12)));
        assert_eq!(st.r_current_temp, Some((1, 13)));
        assert_eq!(st.w_target_temp, Some((1, 14)));
        assert_eq!(st.w_heat_to, Some((1, 15)));
        assert_eq!(st.w_cool_to, Some((1, 16)));
        assert_eq!(st.r_humidity, Some((1, 17)));
        assert_eq!(st.r_units, Some((1, 18)));
        assert_eq!(st.w_fan_active, Some((1, 21)));
    }

    #[test]
    fn fetch_short_form_uuids_with_values() {
        let (port, server) = spawn_fetch_responder(
            fetch_test_accessories(true, true),
            Some(fetch_test_characteristics()),
        );
        let mut sess = secure_test_conn(port);
        let st = fetch_via(&mut sess).unwrap();
        assert_cool_25(&st, "T6 Pro");
        server.join().unwrap();
    }

    #[test]
    fn fetch_short_form_overlay_failure() {
        // Isolates the /accessories mapping for short forms: with the
        // overlay dead, the state must still come from the short-form
        // record alone (previously "4A" matched nothing -> defaults).
        let (port, server) =
            spawn_fetch_responder(fetch_test_accessories(true, true), None);
        let mut sess = secure_test_conn(port);
        let st = fetch_via(&mut sess).unwrap();
        assert_cool_25(&st, "T6 Pro");
        server.join().unwrap();
    }

    #[test]
    fn fetch_no_accessory_values_overlay() {
        // /accessories carries no values at all: targets are still
        // discovered (value-independent recording) and the overlay
        // supplies every value. The name is not overlaid, so it stays
        // empty here -- the tile does not need it.
        let (port, server) = spawn_fetch_responder(
            fetch_test_accessories(false, false),
            Some(fetch_test_characteristics()),
        );
        let mut sess = secure_test_conn(port);
        let st = fetch_via(&mut sess).unwrap();
        assert_cool_25(&st, "");
        server.join().unwrap();
    }

    #[test]
    fn fetch_full_values_regression() {
        let (port, server) = spawn_fetch_responder(
            fetch_test_accessories(false, true),
            Some(fetch_test_characteristics()),
        );
        let mut sess = secure_test_conn(port);
        let st = fetch_via(&mut sess).unwrap();
        assert_cool_25(&st, "T6 Pro");
        server.join().unwrap();
    }

    #[test]
    fn fetch_overlay_failure_keeps_accessory_values() {
        let (port, server) =
            spawn_fetch_responder(fetch_test_accessories(false, true), None);
        let mut sess = secure_test_conn(port);
        let st = fetch_via(&mut sess).unwrap();
        assert_cool_25(&st, "T6 Pro");
        server.join().unwrap();
    }
}
