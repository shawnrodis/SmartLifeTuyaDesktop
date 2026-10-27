pub fn region_base(region: &str) -> &'static str {
    match region {
        "US" => "https://px1.tuyaus.com/homeassistant/",
        "CN" => "https://px1.tuyacn.com/homeassistant/",
        _ => "https://px1.tuyaeu.com/homeassistant/",
    }
}

pub fn region_key_from_code(code: &str) -> &'static str {
    match code {
        "1" => "US",
        "86" => "CN",
        _ => "EU",
    }
}

pub fn region_from_token(token: &str) -> Option<&'static str> {
    match token.get(0..2) {
        Some("EU") => Some("EU"),
        Some("AY") => Some("CN"),
        Some("AZ") => Some("US"),
        _ => None,
    }
}

pub const SMARTATHOME_PROXY: &str = "https://cors.smartathome.co.uk/";

pub const PROXY_PRESETS: [(&str, &str); 6] = [
    ("auto", "Auto — direct for US/EU, community proxy for CN (recommended)"),
    ("smartathome", "SmartAtHome community proxy (all requests)"),
    ("corsproxy", "corsproxy.io (encoded URL)"),
    ("cors-anywhere", "cors-anywhere.herokuapp.com (may need activation)"),
    ("none", "None — direct connection"),
    ("custom", "Custom…"),
];

pub fn url_encode(s: &str) -> String {
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
pub fn apply_proxy(url: &str, preset: &str, custom: &str, region: &str) -> String {
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

