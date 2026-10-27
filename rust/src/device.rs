use crate::config::{Camera, LayoutCfg};
use crate::convert::{brightness_to_pct, hex_to_hsv, norm_saturation, temp_display_kelvin, temp_reported_to_slider};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub dev_type: String,
    pub kind: String, // "scene" | "light" | "switch" | "other"
    pub icon_kind: String,
    pub is_scene: bool,
    pub online: bool,
    pub state: bool,
    pub has_brightness: bool,
    pub has_temp: bool,
    pub has_color: bool,
    pub brightness_pct: Option<i64>,
    pub temp_slider: Option<i64>,
    pub hue: Option<f64>,
    pub saturation: Option<f64>,
    pub color_mode: Option<String>,
    pub raw: Value,
}

/// A discovery value as a number (v1.3.0, Fix A): the legacy
/// homeassistant API is STRINGLY-TYPED for lights — real payloads
/// carry brightness as "800", not 800 (switches use real numbers;
/// see the census fixtures in the tests) — so a JSON string that
/// parses as a number counts as a number. The pre-1.3.0 reader
/// used as_f64() alone and silently dropped every light reading.
pub fn value_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

pub fn data_f64(data: &Map<String, Value>, key: &str) -> Option<f64> {
    data.get(key).and_then(value_f64)
}

/// A discovery value as a bool (v1.3.0, Fix A): switches report
/// state as a real JSON bool, but LIGHTS report it as the STRING
/// "true"/"false" — the field bug behind every light reading Off
/// while plugs read correctly. The reference implementation only
/// ever checks `state === false`, i.e. it never demanded a bool;
/// this reader accepts bool, "true"/"false" strings and numbers.
pub fn data_bool(data: &Map<String, Value>, key: &str) -> Option<bool> {
    match data.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0),
        _ => None,
    }
}

pub fn parse_device(raw: &Value) -> Device {
    let empty = Map::new();
    let data = raw.get("data").and_then(|v| v.as_object()).unwrap_or(&empty);
    let dev_type = raw
        .get("dev_type")
        .or_else(|| raw.get("devType"))
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    let is_scene = dev_type == "scene";
    let has_brightness = data.contains_key("brightness");
    let has_temp = data.contains_key("color_temp");
    let has_color =
        data.contains_key("color_mode") || data.contains_key("hue") || data.contains_key("saturation");
    let kind = if is_scene {
        "scene"
    } else if has_color || has_temp || has_brightness {
        "light"
    } else if dev_type == "switch" {
        "switch"
    } else {
        "other"
    };
    let name = raw
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| raw.get("id").and_then(|v| v.as_str()).unwrap_or("Device").to_string());
    let icon_kind = if kind == "light" {
        let lower = name.to_lowercase();
        let looks_strip = ["strip", "bar", "led strip", "tape"].iter().any(|p| lower.contains(p))
            || (has_color && !has_temp && !has_brightness);
        if looks_strip {
            "strip".to_string()
        } else {
            "light".to_string()
        }
    } else if ["light", "strip", "switch", "scene", "cover", "climate", "fan", "lock", "sensor"]
        .contains(&dev_type.as_str())
    {
        dev_type.clone()
    } else {
        "default".to_string()
    };
    Device {
        id: raw.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        name,
        dev_type,
        kind: kind.to_string(),
        icon_kind,
        is_scene,
        online: data_bool(data, "online") != Some(false),
        state: data_bool(data, "state") == Some(true),
        has_brightness,
        has_temp,
        has_color,
        // The VALUES additionally require a real reading
        // (fresh_value_counts): a stub payload's placeholder
        // (null / 0) yields None — "no current value" — while the
        // capability flags above stay presence-based, so the
        // control itself never disappears with the value.
        brightness_pct: if has_brightness
            && data.get("brightness").map(|v| fresh_value_counts("brightness", v)).unwrap_or(false)
        {
            data_f64(data, "brightness").map(brightness_to_pct)
        } else {
            None
        },
        temp_slider: if has_temp
            && data.get("color_temp").map(|v| fresh_value_counts("color_temp", v)).unwrap_or(false)
        {
            data_f64(data, "color_temp").map(temp_reported_to_slider)
        } else {
            None
        },
        hue: data_f64(data, "hue"),
        saturation: data_f64(data, "saturation").map(norm_saturation),
        color_mode: data.get("color_mode").and_then(|v| v.as_str()).map(|s| s.to_string()),
        raw: raw.clone(),
    }
}

/// Does this fresh payload value actually REPORT the field
/// (v1.2.2)? A JSON null never does. For brightness / colour
/// temperature a numeric 0 (or a non-numeric junk value) doesn't
/// either: on the reported scales those mean "no reading" — Tuya
/// stubs for lights the account can't fully see carry exactly such
/// placeholders — and treating them as real made colour temp snap
/// to the 2700K floor and brightness read 0 in v1.2.1.
pub fn fresh_value_counts(key: &str, v: &Value) -> bool {
    if v.is_null() {
        return false;
    }
    if key == "brightness" || key == "color_temp" {
        // Tolerant (v1.3.0): a string "800" is a real reading,
        // not a placeholder — judging it by as_f64() alone made
        // the merge overwrite fresh light values with stale ones.
        return value_f64(v).map(|n| n > 0.0).unwrap_or(false);
    }
    true
}

/// Keep previously-known data fields when a fresh discovery omits
/// them — or reports them as placeholders (see fresh_value_counts).
/// Capabilities therefore stick: once any payload has shown a
/// device has brightness / colour temp / colour, later thin
/// payloads can't take the controls away, because the merged data
/// still carries the keys and their last-known values (same merge
/// the web edition performs).
pub fn merge_device_data(fresh: Vec<Value>, previous: &[Value]) -> Vec<Value> {
    if previous.is_empty() {
        return fresh;
    }
    let prev_by_id: HashMap<String, &Map<String, Value>> = previous
        .iter()
        .filter_map(|d| {
            let id = d.get("id")?.as_str()?;
            let data = d.get("data")?.as_object()?;
            Some((id.to_string(), data))
        })
        .collect();
    fresh
        .into_iter()
        .map(|mut d| {
            let id = d.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if let Some(prev_data) = prev_by_id.get(&id) {
                if let Some(data) = d.get_mut("data").and_then(|v| v.as_object_mut()) {
                    for (k, v) in prev_data.iter() {
                        let counts = data
                            .get(k)
                            .map(|fv| fresh_value_counts(k, fv))
                            .unwrap_or(false);
                        if !counts {
                            data.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            d
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Section model (port of the web edition's buildSections / ordering rules)
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize, Debug)]
pub enum SectionKind {
    Pinned,
    Lighting,
    Cameras,
    Group,
    Ungrouped,
    Scenes,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Section {
    pub key: String,
    pub order_key: String,
    pub title: String,
    pub kind: SectionKind,
    pub group_id: Option<String>,
    pub ids: Vec<String>,
}

/// One dashboard card's identity for the section model — a Tuya device
/// or a user-added camera. Sections, ordering and search work on entries;
/// rendering looks the full record up by id.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct CardEntry {
    pub id: String,
    pub name: String,
    pub is_scene: bool,
    /// Light-capable (brightness / colour temp / colour) — such devices
    /// live in the automatic Lighting section unless grouped.
    pub light: bool,
    /// A user-added RTSP camera — lives in the automatic Cameras
    /// section unless grouped.
    pub camera: bool,
}

/// Light-capable = can do brightness, colour temperature or colour
/// (bulbs and RGB strips; cameras never are).
pub fn device_is_light(d: &Device) -> bool {
    !d.is_scene && (d.has_brightness || d.has_temp || d.has_color)
}

pub fn device_entry(d: &Device) -> CardEntry {
    CardEntry {
        id: d.id.clone(),
        name: d.name.clone(),
        is_scene: d.is_scene,
        light: device_is_light(d),
        camera: false,
    }
}

pub fn camera_entry(c: &Camera) -> CardEntry {
    // Item ids are "cam:{record id}" everywhere (layout keys, section
    // ids, camera_index) — the record itself keeps its bare id.
    CardEntry {
        id: format!("cam:{}", c.id),
        name: c.name.clone(),
        is_scene: false,
        light: false,
        camera: true,
    }
}

/// The "no explicit group" label for an item in the group dropdown.
pub fn auto_section_label(e: &CardEntry) -> &'static str {
    if e.camera {
        "Cameras (automatic)"
    } else if e.light {
        "Lighting (automatic)"
    } else {
        "Ungrouped"
    }
}

// ---------------------------------------------------------------------------
// Group controls (broadcast, not binding) — a group action computes a
// plan of per-member Control commands and sends them one by one. No
// group state is stored: header widgets reflect member states
// (any-on, averages), exactly like the web edition.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum GroupKind {
    Toggle,
    Brightness,
    Temp,
    Color,
}

/// Master-toggle display state: ON if ANY (non-scene) member is on.
pub fn group_master_on(members: &[Device]) -> bool {
    members.iter().filter(|d| !d.is_scene).any(|d| d.state)
}

pub fn group_average(values: &[i64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<i64>() as f64 / values.len() as f64)
    }
}

/// The hardware brightness floor on the DEVICE scale (v1.3.1):
/// the user's bulbs switch OFF ENTIRELY below 11% — it is their
/// true minimum, not a UI preference. (v1.2.3 lowered the floor
/// to 1% on the theory that 11% was inherited from the
/// reference slider; the field disproved that within a day.)
pub const BRIGHTNESS_FLOOR_PCT: i64 = 11;

/// Map a UI brightness percent (1..100 — what every slider
/// shows and edits) to the DEVICE percent a brightnessSet
/// sends (11..100), linearly: device = 11 + (ui-1) * 89/99.
/// The slider's bottom therefore displays "1%" while sending
/// exactly the 11% hardware minimum — a lit bulb is never
/// commanded below it — and the top is a true 100%.
pub fn brightness_ui_to_device(ui: i64) -> i64 {
    let ui = ui.clamp(1, 100);
    (BRIGHTNESS_FLOOR_PCT as f64 + (ui - 1) as f64 * 89.0 / 99.0).round() as i64
}

/// Map a reported DEVICE percent back to the UI scale for
/// display (the inverse of brightness_ui_to_device):
/// ui = 1 + (device-11) * 99/89, clamped to 1..100 — a bulb
/// reporting its 11% minimum displays "1%", and anything
/// reported below 11% (while on) displays as 1% too.
pub fn brightness_device_to_ui(dev: i64) -> i64 {
    ((1.0 + (dev - BRIGHTNESS_FLOOR_PCT) as f64 * 99.0 / 89.0).round() as i64).clamp(1, 100)
}

/// The brightnessSet payload for a UI percent: the mapped
/// device value (see brightness_ui_to_device). Brightness is
/// ONLY ever sent to devices that natively report a brightness
/// field (v1.3.0, reversing the v1.2.3 route that dimmed
/// colour-only devices through the V of a colorSet): the legacy
/// API surfaces no brightness/temp for a colour-only strip —
/// on/off + colour only — so the app shows no brightness
/// control for it, exactly like the reference. (The strip
/// hardware itself can dim via other channels; this API simply
/// doesn't expose it. See DESIGN.md.)
pub fn brightness_set_value(ui_pct: i64) -> Value {
    json!(brightness_ui_to_device(ui_pct))
}

/// The fan-out plan for one group action. `num` carries the brightness
/// percent or temperature slider value; `hex` the colour. Offline
/// members are skipped (their cards disable controls too).
pub fn group_plan(
    members: &[Device],
    kind: GroupKind,
    num: i64,
    hex: Option<&str>,
) -> Vec<(String, &'static str, Value)> {
    let devs: Vec<&Device> = members
        .iter()
        .filter(|d| !d.is_scene && d.online)
        .collect();
    let mut plan = Vec::new();
    match kind {
        GroupKind::Toggle => {
            let target: i64 = if group_master_on(members) { 0 } else { 1 };
            for d in devs {
                plan.push((d.id.clone(), "turnOnOff", json!(target)));
            }
        }
        GroupKind::Brightness => {
            // Only members that natively report a brightness
            // field take part (v1.3.0): a colour-only strip is
            // excluded here — it still joins Toggle and Color.
            for d in devs.iter().filter(|d| d.has_brightness) {
                plan.push((d.id.clone(), "brightnessSet", brightness_set_value(num)));
            }
        }
        GroupKind::Temp => {
            let tv = num.clamp(1000, 10000);
            for d in devs.iter().filter(|d| d.has_temp) {
                plan.push((d.id.clone(), "colorTemperatureSet", json!(tv)));
            }
        }
        GroupKind::Color => {
            let (mut hue, mut sat, _) = hex_to_hsv(hex.unwrap_or("#ffffff"));
            if sat < 0.02 {
                hue = 0.0;
                sat = 0.0;
            }
            let sat = (sat * 100.0).round() / 100.0;
            for d in devs.iter().filter(|d| d.has_color) {
                plan.push((
                    d.id.clone(),
                    "colorSet",
                    json!({
                        "hue": hue as i64,
                        "saturation": sat,
                        "brightness": d.brightness_pct.unwrap_or(100)
                    }),
                ));
            }
        }
    }
    plan
}

/// Display name of a device (alias wins) — used for card titles.
pub fn display_name(layout: &LayoutCfg, d: &Device) -> String {
    layout.alias.get(&d.id).cloned().unwrap_or_else(|| d.name.clone())
}

pub fn entry_display_name(layout: &LayoutCfg, e: &CardEntry) -> String {
    layout.alias.get(&e.id).cloned().unwrap_or_else(|| e.name.clone())
}

pub fn compare_by_name(layout: &LayoutCfg, a: &CardEntry, b: &CardEntry) -> std::cmp::Ordering {
    let na = entry_display_name(layout, a).to_lowercase();
    let nb = entry_display_name(layout, b).to_lowercase();
    na.cmp(&nb).then_with(|| a.id.cmp(&b.id))
}

pub fn order_section_items(
    items: &[&CardEntry],
    custom_order: Option<&Vec<String>>,
    layout: &LayoutCfg,
) -> Vec<String> {
    let by_id: HashMap<&str, &CardEntry> = items.iter().map(|d| (d.id.as_str(), *d)).collect();
    let mut used: HashSet<&str> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    if let Some(order) = custom_order {
        for id in order {
            if by_id.contains_key(id.as_str()) && used.insert(id.as_str()) {
                out.push(id.clone());
            }
        }
    }
    let mut rest: Vec<&CardEntry> = items.iter().filter(|d| !used.contains(d.id.as_str())).cloned().collect();
    rest.sort_by(|a, b| compare_by_name(layout, a, b));
    out.extend(rest.iter().map(|d| d.id.clone()));
    out
}

pub fn filter_entries<'a>(entries: &'a [CardEntry], layout: &LayoutCfg, query: &str) -> Vec<&'a CardEntry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return entries.iter().collect();
    }
    entries
        .iter()
        .filter(|d| {
            entry_display_name(layout, d).to_lowercase().contains(&q) || d.name.to_lowercase().contains(&q)
        })
        .collect()
}

/// The sections' DEFAULT order (v1.2.1): Cameras first, then Pinned,
/// Lighting, user groups (creation order, expanded in place),
/// Ungrouped, Scenes. Expressed over a built section list so groups
/// land exactly where they were created relative to each other.
pub fn default_section_seq(sections: &[Section]) -> Vec<String> {
    pub fn slot(s: &Section) -> usize {
        match s.kind {
            SectionKind::Cameras => 0,
            SectionKind::Pinned => 1,
            SectionKind::Lighting => 2,
            SectionKind::Group => 3,
            SectionKind::Ungrouped => 4,
            SectionKind::Scenes => 5,
        }
    }
    let mut idx: Vec<usize> = (0..sections.len()).collect();
    idx.sort_by_key(|&i| slot(&sections[i])); // stable sort: groups keep creation order
    idx.iter().map(|&i| sections[i].order_key.clone()).collect()
}

/// The EFFECTIVE section sequence: the stored user order (filtered
/// to sections that exist) wins; any section missing from the stored
/// list (a new group, a newly-appearing built-in) slots in right
/// after its nearest already-placed default-order predecessor — or
/// at the front when it has none.
pub fn effective_section_seq(sections: &[Section], stored: &[String]) -> Vec<String> {
    let present: HashSet<&str> = sections.iter().map(|s| s.order_key.as_str()).collect();
    let mut eff: Vec<String> = stored
        .iter()
        .filter(|k| present.contains(k.as_str()))
        .cloned()
        .collect();
    let default = default_section_seq(sections);
    for (i, key) in default.iter().enumerate() {
        if eff.contains(key) {
            continue;
        }
        let mut at = 0;
        for prev in default[..i].iter().rev() {
            if let Some(p) = eff.iter().position(|k| k == prev) {
                at = p + 1;
                break;
            }
        }
        eff.insert(at, key.clone());
    }
    eff
}

/// Sort a built section list into the effective sequence.
pub fn order_sections(sections: &mut [Section], stored: &[String]) {
    let eff = effective_section_seq(sections, stored);
    let rank: HashMap<&str, usize> =
        eff.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    sections.sort_by_key(|s| rank[s.order_key.as_str()]);
}

/// The stored sequence after moving `key` one step: start from the
/// effective sequence of the RENDERED sections, swap with the
/// neighbour, then re-append any stored keys for sections that
/// aren't currently rendered (in their stored relative order) so a
/// move never forgets a hidden section's place.
pub fn moved_section_seq(
    stored: &[String],
    rendered: &[Section],
    key: &str,
    dir: i32,
) -> Vec<String> {
    let mut seq = effective_section_seq(rendered, stored);
    if let Some(i) = seq.iter().position(|k| k == key) {
        let j = i as i32 + dir;
        if j >= 0 && (j as usize) < seq.len() {
            seq.swap(i, j as usize);
        }
    }
    for k in stored {
        if !seq.contains(k) {
            seq.push(k.clone());
        }
    }
    seq
}

/// Greedy row packing for the dashboard grid (v1.2.1): walk the
/// items in order, filling each row left to right; an item that
/// doesn't fit in the row's remaining columns starts a new row.
/// Spans are camera card sizes (1 = Normal, 2 = Large, `cols` =
/// Wide); devices, scenes and the drag gap pass their own span.
/// Mirrors what the web edition's CSS grid does with
/// `grid-column: span N`.
pub fn pack_rows<T: Clone>(items: &[T], spans: &[usize], cols: usize) -> Vec<Vec<(T, usize)>> {
    let cols = cols.max(1);
    let mut rows: Vec<Vec<(T, usize)>> = Vec::new();
    let mut cur: Vec<(T, usize)> = Vec::new();
    let mut used = 0usize;
    for (i, item) in items.iter().enumerate() {
        let span = spans.get(i).copied().unwrap_or(1).max(1).min(cols);
        if !cur.is_empty() && used + span > cols {
            rows.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push((item.clone(), span));
        used += span;
    }
    if !cur.is_empty() {
        rows.push(cur);
    }
    rows
}

pub fn build_sections(
    entries: &[CardEntry],
    layout: &LayoutCfg,
    editing: bool,
    query: &str,
) -> Vec<Section> {
    let visible: Vec<&CardEntry> = filter_entries(entries, layout, query)
        .into_iter()
        .filter(|d| !layout.hidden.contains(&d.id))
        .collect();
    let scenes: Vec<&CardEntry> = visible.iter().filter(|d| d.is_scene).cloned().collect();
    let normal: Vec<&CardEntry> = visible.iter().filter(|d| !d.is_scene).cloned().collect();
    let pin_set: HashSet<&str> = layout.pins.iter().map(|s| s.as_str()).collect();
    let so = &layout.section_order;
    let mut sections: Vec<Section> = Vec::new();

    let group_ids: HashSet<&str> = layout.groups.iter().map(|g| g.id.as_str()).collect();
    let assigned = |d: &CardEntry| -> bool {
        match layout.assign.get(&d.id) {
            Some(g) => group_ids.contains(g.as_str()),
            None => false,
        }
    };
    // Sections are CONSTRUCTED in the default order (v1.2.1: Cameras
    // → Pinned → Lighting → groups → Ungrouped → Scenes) and then
    // re-sorted by the user's stored section sequence below. An
    // explicit group assignment always wins; unassigned lights land
    // in Lighting, unassigned cameras in Cameras.
    let cameras_sec: Vec<&CardEntry> = normal
        .iter()
        .filter(|d| !assigned(d) && d.camera)
        .cloned()
        .collect();
    sections.push(Section {
        key: "cameras".into(),
        order_key: "cameras".into(),
        title: "Cameras".into(),
        kind: SectionKind::Cameras,
        group_id: None,
        ids: order_section_items(&cameras_sec, so.get("cameras"), layout),
    });
    let pinned: Vec<&CardEntry> = normal.iter().filter(|d| pin_set.contains(d.id.as_str())).cloned().collect();
    if !pinned.is_empty() {
        sections.push(Section {
            key: "pinned".into(),
            order_key: "pinned".into(),
            title: "Pinned".into(),
            kind: SectionKind::Pinned,
            group_id: None,
            ids: order_section_items(&pinned, so.get("pinned"), layout),
        });
    }
    let lighting: Vec<&CardEntry> = normal
        .iter()
        .filter(|d| !assigned(d) && d.light)
        .cloned()
        .collect();
    sections.push(Section {
        key: "lighting".into(),
        order_key: "lighting".into(),
        title: "Lighting".into(),
        kind: SectionKind::Lighting,
        group_id: None,
        ids: order_section_items(&lighting, so.get("lighting"), layout),
    });
    for g in &layout.groups {
        let devs: Vec<&CardEntry> = normal
            .iter()
            .filter(|d| layout.assign.get(&d.id) == Some(&g.id))
            .cloned()
            .collect();
        let ok = format!("group:{}", g.id);
        sections.push(Section {
            key: g.id.clone(),
            order_key: ok.clone(),
            title: g.name.clone(),
            kind: SectionKind::Group,
            group_id: Some(g.id.clone()),
            ids: order_section_items(&devs, so.get(&ok), layout),
        });
    }
    let ungrouped: Vec<&CardEntry> = normal
        .iter()
        .filter(|d| !assigned(d) && !d.light && !d.camera)
        .cloned()
        .collect();
    sections.push(Section {
        key: "ungrouped".into(),
        order_key: "ungrouped".into(),
        title: "Ungrouped".into(),
        kind: SectionKind::Ungrouped,
        group_id: None,
        ids: order_section_items(&ungrouped, so.get("ungrouped"), layout),
    });
    if !scenes.is_empty() {
        sections.push(Section {
            key: "scenes".into(),
            order_key: "scenes".into(),
            title: "Scenes".into(),
            kind: SectionKind::Scenes,
            group_id: None,
            ids: order_section_items(&scenes, so.get("scenes"), layout),
        });
    }
    order_sections(&mut sections, &layout.section_seq);
    sections.retain(|s| {
        !s.ids.is_empty()
            || (editing
                && (s.kind == SectionKind::Group
                    || s.kind == SectionKind::Lighting
                    || s.kind == SectionKind::Cameras))
    });
    sections
}

pub fn status_text(d: &Device) -> String {
    if !d.online {
        return "Offline".to_string();
    }
    if d.is_scene {
        return "Scene — tap Start to run".to_string();
    }
    let mut parts = vec![if d.state { "On".to_string() } else { "Off".to_string() }];
    if d.state {
        if let Some(b) = d.brightness_pct {
            // Displayed on the UI scale (v1.3.1), like the sliders.
            parts.push(format!("{}%", brightness_device_to_ui(b)));
        }
        if d.color_mode.as_deref() == Some("colour") {
            parts.push("Colour".to_string());
        } else if let Some(t) = d.temp_slider {
            // Displayed at the nearest stop (v1.3.1), like the
            // temp sliders themselves.
            parts.push(format!("{}K", temp_display_kelvin(t)));
        }
    }
    parts.join(" · ")
}
