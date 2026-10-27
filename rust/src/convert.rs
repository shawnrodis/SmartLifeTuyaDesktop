pub fn clampf(v: f64, lo: f64, hi: f64) -> f64 {
    v.min(hi).max(lo)
}

pub fn brightness_to_pct(reported: f64) -> i64 {
    clampf((reported / 10.0).round(), 0.0, 100.0) as i64
}

pub fn temp_reported_to_slider(reported: f64) -> i64 {
    clampf(((reported - 1000.0) / 4.033 + 1000.0).round(), 1000.0, 10000.0) as i64
}

pub fn temp_slider_to_reported(slider: i64) -> i64 {
    (((slider - 1000) as f64) * 4.033 + 1000.0).round() as i64
}

/// Neutral colour-temperature slider position (≈4600K, mid-scale)
/// shown when a temp-capable device or group currently has no
/// reading — the control stays, at a sane middle value, instead of
/// snapping to the 2700K floor or vanishing (v1.2.2).
pub const TEMP_NEUTRAL_SLIDER: i64 = 5500;

pub fn temp_slider_to_kelvin(slider: i64) -> i64 {
    ((2700.0 + (clampf(slider as f64, 1000.0, 10000.0) - 1000.0) * (3800.0 / 9000.0)) / 10.0).round() as i64 * 10
}

/// The five colour-temperature stops (v1.3.1), in Kelvin —
/// the standard temps from the user's reference chart. Every
/// colour-temperature slider in the app has exactly these
/// detents: its value is a stop INDEX (0..=4), and anything
/// displayed or sent is one of these five values (snapped in
/// Kelvin space, before the API-scale conversion).
pub const TEMP_STOPS_K: [i64; 5] = [2700, 3000, 4000, 5000, 6500];

/// Index of the stop nearest a Kelvin value (ties go to the
/// lower stop). A device reporting an in-between value (3080K,
/// 3780K, 3970K...) DISPLAYS at the nearest stop — its stored
/// reading is not rewritten until the user actually drags.
pub fn temp_stop_index(kelvin: i64) -> usize {
    let mut best = 0;
    for (i, &s) in TEMP_STOPS_K.iter().enumerate() {
        if (kelvin - s).abs() < (kelvin - TEMP_STOPS_K[best]).abs() {
            best = i;
        }
    }
    best
}

/// The Kelvin stop at a slider index (clamped to the range).
pub fn temp_stop_kelvin(index: i64) -> i64 {
    TEMP_STOPS_K[(index.max(0) as usize).min(TEMP_STOPS_K.len() - 1)]
}

/// Kelvin -> the API slider scale (the inverse of
/// temp_slider_to_kelvin): what a colorTemperatureSet sends
/// for an exact stop.
pub fn temp_kelvin_to_slider(kelvin: i64) -> i64 {
    (1000.0 + (kelvin - 2700) as f64 * 9000.0 / 3800.0).round() as i64
}

/// The Kelvin a temp reading DISPLAYS as: the nearest stop
/// (v1.3.1) — used by the card status line.
pub fn temp_display_kelvin(temp_slider: i64) -> i64 {
    TEMP_STOPS_K[temp_stop_index(temp_slider_to_kelvin(temp_slider))]
}

pub fn hex_to_hsv(hex: &str) -> (f64, f64, f64) {
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

pub fn hsv_to_hex(h: f64, s: f64, v: f64) -> String {
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

pub fn norm_saturation(s: f64) -> f64 {
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
pub fn pct_encode_component(s: &str) -> String {
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

