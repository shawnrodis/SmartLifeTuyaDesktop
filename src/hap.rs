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
use std::net::TcpStream;
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
            let mut content_len: usize = 0;
            for line in lines {
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_len = v.trim().parse().unwrap_or(0);
                }
            }
            let total = hdr_end + 4 + content_len;
            while self.buf.len() < total {
                self.fill()?;
            }
            let body = self.buf[hdr_end + 4..total].to_vec();
            self.buf.drain(..total);
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
// Pair-setup (M1..M6) on one plain connection. `pin_dashed` is the
// "123-45-678" form; the code itself is never stored anywhere.
// ---------------------------------------------------------------------------

pub fn pair_setup(ip: &str, port: u16, pin_dashed: &str) -> HapResult<ThermostatPairing> {
    let mut conn = Conn::connect(ip, port)?;

    // M1 -> M2: method + state, expect salt + server public key.
    let m1 = tlv_encode(&[(TAG_STATE, &[STATE_M1]), (TAG_METHOD, &[0x00])]);
    let m2 = conn.post_tlv("/pair-setup", &m1)?;
    check_state(&m2, STATE_M2, "pair-setup M2")?;
    let salt = tlv_get(&m2, TAG_SALT)
        .ok_or_else(|| HapError::Msg("pair-setup M2 missing salt".into()))?
        .to_vec();
    let b_raw = tlv_get(&m2, TAG_PUBLIC_KEY)
        .ok_or_else(|| HapError::Msg("pair-setup M2 missing server public key".into()))?
        .to_vec();

    // SRP client math, then M3 (public key + proof).
    let srp = SrpClient::new()?;
    let k = srp.session(&salt, &b_raw, pin_dashed)?;
    let m_proof = srp.client_proof(&salt, &b_raw, &k);
    let m3 = tlv_encode(&[
        (TAG_STATE, &[STATE_M3]),
        (TAG_PUBLIC_KEY, srp.public_bytes()),
        (TAG_PROOF, &m_proof),
    ]);
    let m4 = conn.post_tlv("/pair-setup", &m3)?;
    check_state(&m4, STATE_M4, "pair-setup M4")?;
    let server_proof = tlv_get(&m4, TAG_PROOF)
        .ok_or_else(|| HapError::Msg("pair-setup M4 missing server proof".into()))?;
    if !srp.verify_server_proof(&m_proof, &k, server_proof) {
        return err("accessory proof mismatch -- wrong setup code?");
    }

    // M5: fresh controller identity + signature, encrypted.
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let signing = SigningKey::from_bytes(&seed);
    let ltpk = signing.verifying_key().to_bytes();
    let pairing_id = fresh_pairing_id();
    let ios_x = hkdf_sha512(
        &k,
        b"Pair-Setup-Controller-Sign-Salt",
        b"Pair-Setup-Controller-Sign-Info",
        32,
    )?;
    let enc_key = hkdf_sha512(
        &k,
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
        &k,
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
        ..Default::default()
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
}

fn short_type(full: &str) -> String {
    full.get(..8).unwrap_or("").to_ascii_uppercase()
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
                            st.w_target_mode = Some((acc.aid, ch.iid));
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
                            st.w_target_temp = Some((acc.aid, ch.iid));
                        }
                        CH_TEMP_UNITS => {
                            st.temp_units_f = as_u8(v).map(|u| u == 1).unwrap_or(false);
                        }
                        CH_HEAT_TO => {
                            if let Some(t) = as_f64(v) {
                                st.heat_to_c = Some(t);
                            }
                            st.w_heat_to = Some((acc.aid, ch.iid));
                        }
                        CH_COOL_TO => {
                            if let Some(t) = as_f64(v) {
                                st.cool_to_c = Some(t);
                            }
                            st.w_cool_to = Some((acc.aid, ch.iid));
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
                        if let Some(v) = &ch.value {
                            st.fan_on = as_u8(v).map(|u| u != 0);
                        }
                        st.w_fan_active = Some((acc.aid, ch.iid));
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

fn fetch_state_inner(pairing: &ThermostatPairing) -> HapResult<ThermostatState> {
    let mut sess = SecureConn::open(pairing)?;
    let accs = sess.get_accessories()?;
    Ok(state_from_accessories(&accs))
}

/// Fetch the thermostat state, with one automatic retry on IO failure
/// (e.g. a stale connection) using a fresh session. Counters are never
/// reused across sessions -- the retry opens a brand-new pair-verify.
pub fn fetch_state(pairing: &ThermostatPairing) -> HapResult<ThermostatState> {
    match fetch_state_inner(pairing) {
        Ok(s) => Ok(s),
        Err(HapError::Io(_)) => fetch_state_inner(pairing),
        Err(e) => Err(e),
    }
}

/// Apply changes, then read back exactly the written characteristics and
/// report the actual values (the returned state reflects the readback).
pub fn set_state(
    pairing: &ThermostatPairing,
    changes: &ThermostatChanges,
) -> HapResult<ThermostatState> {
    let mut sess = SecureConn::open(pairing)?;
    let accs = sess.get_accessories()?;
    let mut st = state_from_accessories(&accs);
    let mut writes: Vec<(u32, u32, serde_json::Value)> = Vec::new();
    if let Some(m) = changes.target_mode {
        if let Some(t) = st.w_target_mode {
            writes.push((t.0, t.1, serde_json::json!(m)));
        }
    }
    if let Some(t) = changes.target_temp_c {
        if let Some(w) = st.w_target_temp {
            writes.push((w.0, w.1, serde_json::json!(t)));
        }
    }
    if let Some(t) = changes.heat_to_c {
        if let Some(w) = st.w_heat_to {
            writes.push((w.0, w.1, serde_json::json!(t)));
        }
    }
    if let Some(t) = changes.cool_to_c {
        if let Some(w) = st.w_cool_to {
            writes.push((w.0, w.1, serde_json::json!(t)));
        }
    }
    if let Some(on) = changes.fan_on {
        if let Some(w) = st.w_fan_active {
            writes.push((w.0, w.1, serde_json::json!(if on { 1 } else { 0 })));
        }
    }
    if writes.is_empty() {
        return Ok(st);
    }
    let failures = sess.put_characteristics(&writes)?;
    if !failures.is_empty() {
        let desc: Vec<String> = failures
            .iter()
            .map(|(a, i, s)| format!("{}.{} -> status {}", a, i, s))
            .collect();
        return err(format!("thermostat rejected write(s): {}", desc.join(", ")));
    }
    // Read back the written characteristics and patch the state.
    let ids: Vec<(u32, u32)> = writes.iter().map(|(a, i, _)| (*a, *i)).collect();
    let back = sess.get_characteristics(&ids)?;
    for (aid, iid, _) in &writes {
        if let Some(v) = back.get(&(*aid, *iid)) {
            if Some((*aid, *iid)) == st.w_target_mode {
                if let Some(m) = as_u8(v) {
                    st.target_mode = m;
                }
            } else if Some((*aid, *iid)) == st.w_target_temp {
                if let Some(t) = as_f64(v) {
                    st.target_temp_c = t;
                }
            } else if Some((*aid, *iid)) == st.w_heat_to {
                if let Some(t) = as_f64(v) {
                    st.heat_to_c = Some(t);
                }
            } else if Some((*aid, *iid)) == st.w_cool_to {
                if let Some(t) = as_f64(v) {
                    st.cool_to_c = Some(t);
                }
            } else if Some((*aid, *iid)) == st.w_fan_active {
                st.fan_on = as_u8(v).map(|u| u != 0);
            }
        }
    }
    Ok(st)
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

pub fn discover(timeout: Duration) -> HapResult<Vec<DiscoveredDevice>> {
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

/// Pair with a discovered device. The setup code is normalised, used for
/// this handshake only, and never stored.
pub fn pair_device(device: &DiscoveredDevice, code: &str) -> HapResult<ThermostatPairing> {
    let pin = normalize_setup_code(code)?;
    let ip = device
        .addresses
        .iter()
        .find(|a| a.contains('.'))
        .or_else(|| device.addresses.first())
        .ok_or_else(|| HapError::Msg("discovered device has no address".into()))?;
    let mut pairing = pair_setup(ip, device.port, &pin)?;
    pairing.accessory_name = device.name.clone();
    pairing.accessory_model = device.model.clone();
    Ok(pairing)
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
}
