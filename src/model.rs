//! Pure UI model helpers for Maprisma Hue Lights: tabs, room filters, pinned favorites,
//! All-On room exclusions, color/warmth/effect presets, schedule/rule draft builders,
//! formatting helpers, and unit tests.

use std::collections::HashSet;

use cosmic::iced::Color;
use maprisma_hue_lights::i18n::{tr, trf};
use serde_json::{Value, json};

pub const FAVORITES_KEY: &str = "favorites";
pub const EXCLUDE_ALL_ON_KEY: &str = "exclude_all_on";
pub const CONFIRM_ALL_ON_KEY: &str = "confirm_all_on";

pub const DAYS_ORDER: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Lights,
    Schedules,
    Rules,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoomFilter {
    #[default]
    All,
    On,
    Rooms,
    Zones,
    Favorites,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Favorite {
    pub kind: String, // "room" | "light"
    pub id: String,
}

impl Favorite {
    pub fn new(kind: impl Into<String>, id: impl Into<String>) -> Self {
        Self { kind: kind.into(), id: id.into() }
    }
}

/// Parses persisted favorites from config.json (`[{"kind":"room"|"light","id":"..."}]`
/// or legacy `"room:<id>"` / `"light:<id>"` strings), dropping malformed entries and duplicates.
pub fn parse_favorites(value: Option<&Value>) -> Vec<Favorite> {
    let Some(arr) = value.and_then(Value::as_array) else { return Vec::new() };
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for item in arr {
        let fav = if let Some(obj) = item.as_object() {
            let kind = obj.get("kind").and_then(Value::as_str).unwrap_or("");
            let id = obj.get("id").and_then(Value::as_str).unwrap_or("");
            if matches!(kind, "room" | "light") && !id.is_empty() {
                Some(Favorite::new(kind, id))
            } else {
                None
            }
        } else if let Some(raw) = item.as_str() {
            raw.split_once(':').and_then(|(k, id)| {
                (matches!(k, "room" | "light") && !id.is_empty()).then(|| Favorite::new(k, id))
            })
        } else {
            None
        };
        if let Some(f) = fav && seen.insert(f.clone()) {
            out.push(f);
        }
    }
    out
}

pub fn favorites_to_value(items: &[Favorite]) -> Value {
    Value::Array(items.iter().map(|f| json!({"kind": f.kind, "id": f.id})).collect())
}

pub fn is_favorite(items: &[Favorite], kind: &str, id: &str) -> bool {
    items.iter().any(|f| f.kind == kind && f.id == id)
}

/// Toggles a favorite in place; returns `true` if added, `false` if removed.
pub fn toggle_favorite(items: &mut Vec<Favorite>, kind: &str, id: &str) -> bool {
    if !matches!(kind, "room" | "light") || id.is_empty() {
        return false;
    }
    if let Some(pos) = items.iter().position(|f| f.kind == kind && f.id == id) {
        items.remove(pos);
        false
    } else {
        items.push(Favorite::new(kind, id));
        true
    }
}

pub fn parse_string_set(value: Option<&Value>) -> HashSet<String> {
    value
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub fn string_set_to_value(set: &HashSet<String>) -> Value {
    let mut list: Vec<&str> = set.iter().map(String::as_str).collect();
    list.sort_unstable();
    Value::Array(list.into_iter().map(Value::from).collect())
}

pub fn sorted_exclude_list(set: &HashSet<String>) -> Vec<String> {
    let mut list: Vec<String> = set.iter().cloned().collect();
    list.sort_unstable();
    list
}

// ------------------------------------------------------------------------- JSON helpers

pub fn j_str<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn j_bool(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool) == Some(true)
}

pub fn j_i64(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

pub fn j_list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

pub fn room_key_of(room: &Value) -> String {
    let id = j_str(room, "id");
    if id.is_empty() { j_str(room, "kind").to_string() } else { id.to_string() }
}

/// Whether a room/zone matches the selected filter pill.
pub fn room_matches_filter(room: &Value, filter: RoomFilter, favorites: &[Favorite]) -> bool {
    match filter {
        RoomFilter::All => true,
        RoomFilter::On => j_bool(room, "on"),
        RoomFilter::Rooms => j_str(room, "kind") != "zone",
        RoomFilter::Zones => j_str(room, "kind") == "zone",
        RoomFilter::Favorites => {
            let key = room_key_of(room);
            is_favorite(favorites, "room", &key)
                || j_list(room, "lights")
                    .iter()
                    .any(|l| is_favorite(favorites, "light", j_str(l, "id")))
        }
    }
}

/// Whether the given target (`"light"`, `"room"`, `"zone"`) has an enabled schedule.
pub fn has_active_schedule(stats: &Value, kind: &str, id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    j_list(stats, "schedules").iter().any(|sch| {
        if !j_bool(sch, "enabled") || j_str(sch, "target_id") != id {
            return false;
        }
        let tk = j_str(sch, "target_kind");
        tk == kind || (kind == "room" && tk == "zone")
    })
}

/// Whether the given light or room is referenced as trigger or target in any rule.
pub fn belongs_to_rule(stats: &Value, kind: &str, id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    let kind_eq = |k: &str| k == kind || (matches!(kind, "room" | "zone") && matches!(k, "room" | "zone"));
    j_list(stats, "rules").iter().any(|rule| {
        if kind_eq(j_str(rule, "trigger_kind")) && j_str(rule, "trigger_id") == id {
            return true;
        }
        j_list(rule, "targets")
            .iter()
            .any(|t| kind_eq(j_str(t, "kind")) && j_str(t, "id") == id)
    })
}

// ------------------------------------------------------------------------- presets

#[derive(Debug, Clone, Copy)]
pub struct WarmthPreset {
    pub label_key: &'static str,
    pub mirek: u16,
    pub swatch: Color,
}

pub const WARMTH_PRESETS: [WarmthPreset; 4] = [
    WarmthPreset { label_key: "Candle 2000K", mirek: 500, swatch: Color::from_rgb(1.0, 0.58, 0.16) },
    WarmthPreset { label_key: "Warm 2700K", mirek: 370, swatch: Color::from_rgb(1.0, 0.79, 0.48) },
    WarmthPreset { label_key: "Neutral 4000K", mirek: 250, swatch: Color::from_rgb(1.0, 0.93, 0.85) },
    WarmthPreset { label_key: "Daylight 6500K", mirek: 154, swatch: Color::from_rgb(0.85, 0.92, 1.0) },
];

#[derive(Debug, Clone, Copy)]
pub struct ColorPreset {
    pub xy: [f64; 2],
    pub swatch: Color,
}

pub const COLOR_PRESETS: [ColorPreset; 8] = [
    ColorPreset { xy: [0.675, 0.322], swatch: Color::from_rgb(0.90, 0.22, 0.21) }, // Red
    ColorPreset { xy: [0.585, 0.388], swatch: Color::from_rgb(0.98, 0.55, 0.00) }, // Orange
    ColorPreset { xy: [0.505, 0.445], swatch: Color::from_rgb(1.00, 0.70, 0.00) }, // Amber
    ColorPreset { xy: [0.215, 0.700], swatch: Color::from_rgb(0.26, 0.63, 0.28) }, // Green
    ColorPreset { xy: [0.165, 0.360], swatch: Color::from_rgb(0.00, 0.67, 0.76) }, // Cyan
    ColorPreset { xy: [0.153, 0.048], swatch: Color::from_rgb(0.12, 0.53, 0.90) }, // Blue
    ColorPreset { xy: [0.272, 0.118], swatch: Color::from_rgb(0.56, 0.14, 0.67) }, // Purple
    ColorPreset { xy: [0.460, 0.215], swatch: Color::from_rgb(0.91, 0.12, 0.39) }, // Pink
];

pub fn effect_label(effect: &str) -> String {
    match effect {
        "no_effect" => tr("None"),
        "candle" => tr("Candle"),
        "fire" => tr("Fire"),
        "sparkle" => tr("Sparkle"),
        "prism" => tr("Prism"),
        "opal" => tr("Opal"),
        "glisten" => tr("Glisten"),
        "underwater" => tr("Underwater"),
        "cosmos" => tr("Cosmos"),
        "sunbeam" => tr("Sunbeam"),
        "enchant" => tr("Enchant"),
        other => {
            let clean = other.replace('_', " ");
            let mut chars = clean.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => tr("None"),
            }
        }
    }
}

pub fn clamp_mirek(light: &Value, mirek: u16) -> u16 {
    let min = j_i64(light, "mirek_min").unwrap_or(153).clamp(100, 500) as u16;
    let max = j_i64(light, "mirek_max").unwrap_or(500).clamp(min as i64, 1000) as u16;
    mirek.clamp(min, max)
}

// ------------------------------------------------------------------------- target lookup

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetOption {
    pub kind: String, // "room" | "zone" | "light"
    pub id: String,
    pub label: String,
}

/// Collects all addressable rooms, zones, and individual lights from the snapshot.
pub fn all_targets(stats: &Value) -> Vec<TargetOption> {
    let mut out = Vec::new();
    let mut seen_lights = HashSet::new();
    for room in j_list(stats, "rooms") {
        let id = j_str(room, "id");
        let kind = j_str(room, "kind");
        if !id.is_empty() && matches!(kind, "room" | "zone") {
            let name = j_str(room, "name");
            let label = if kind == "zone" {
                format!("{name} ({})", tr("Zone"))
            } else {
                name.to_string()
            };
            out.push(TargetOption { kind: kind.to_string(), id: id.to_string(), label });
        }
    }
    for room in j_list(stats, "rooms") {
        let rname = if j_str(room, "kind") == "other" {
            tr("Other lights")
        } else {
            j_str(room, "name").to_string()
        };
        for light in j_list(room, "lights") {
            let lid = j_str(light, "id");
            if lid.is_empty() || !seen_lights.insert(lid.to_string()) {
                continue;
            }
            let lname = j_str(light, "name");
            let label = if rname.is_empty() {
                lname.to_string()
            } else {
                format!("{lname} · {rname}")
            };
            out.push(TargetOption { kind: "light".into(), id: lid.to_string(), label });
        }
    }
    out
}

/// Resolves a `(kind, id)` to its human-readable name from the snapshot.
pub fn resolve_name(stats: &Value, kind: &str, id: &str) -> String {
    if kind == "desktop" {
        return match id {
            "lock" => "Lock".into(),
            "unlock" => "Unlock".into(),
            other if !other.is_empty() => other.to_string(),
            _ => "Desktop".into(),
        };
    }
    if matches!(kind, "room" | "zone") {
        for room in j_list(stats, "rooms") {
            if room_key_of(room) == id {
                return if j_str(room, "kind") == "other" {
                    tr("Other lights")
                } else {
                    j_str(room, "name").to_string()
                };
            }
        }
    }
    for room in j_list(stats, "rooms") {
        for light in j_list(room, "lights") {
            if j_str(light, "id") == id {
                return j_str(light, "name").to_string();
            }
        }
    }
    id.to_string()
}

// ------------------------------------------------------------------------- time & day formatting

pub fn valid_hhmm(s: &str) -> bool {
    let s = s.trim();
    if s.len() != 5 || s.as_bytes()[2] != b':' {
        return false;
    }
    let Ok(h) = s[0..2].parse::<u8>() else { return false };
    let Ok(m) = s[3..5].parse::<u8>() else { return false };
    h < 24 && m < 60
}

pub fn day_short_toggle(code: &str) -> String {
    match code {
        "mon" => tr("day_mon"),
        "tue" => tr("day_tue"),
        "wed" => tr("day_wed"),
        "thu" => tr("day_thu"),
        "fri" => tr("day_fri"),
        "sat" => tr("day_sat"),
        "sun" => tr("day_sun"),
        other => other.to_string(),
    }
}

pub fn day_abbrev(code: &str) -> String {
    match code {
        "mon" => tr("Mon"),
        "tue" => tr("Tue"),
        "wed" => tr("Wed"),
        "thu" => tr("Thu"),
        "fri" => tr("Fri"),
        "sat" => tr("Sat"),
        "sun" => tr("Sun"),
        other => other.to_string(),
    }
}

pub fn format_days(days: &[String]) -> String {
    let set: HashSet<&str> = days.iter().map(String::as_str).collect();
    if set.is_empty() || DAYS_ORDER.iter().all(|d| set.contains(d)) {
        return tr("Every day");
    }
    let weekdays = ["mon", "tue", "wed", "thu", "fri"];
    let weekend = ["sat", "sun"];
    if set.len() == 5 && weekdays.iter().all(|d| set.contains(d)) {
        return format!("{}–{}", tr("Mon"), tr("Fri"));
    }
    if set.len() == 2 && weekend.iter().all(|d| set.contains(d)) {
        return format!("{}, {}", tr("Sat"), tr("Sun"));
    }
    DAYS_ORDER
        .iter()
        .filter(|d| set.contains(*d))
        .map(|d| day_abbrev(d))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn schedule_trigger_summary(sch: &Value) -> String {
    let kind = j_str(sch, "trigger_kind");
    let offset = j_i64(sch, "offset_min").unwrap_or(0);
    let head = match kind {
        "sunrise" => format_solar(&tr("Sunrise"), offset),
        "sunset" => format_solar(&tr("Sunset"), offset),
        _ => {
            let t = j_str(sch, "time");
            if t.is_empty() { "00:00".to_string() } else { t.to_string() }
        }
    };
    let days: Vec<String> = j_list(sch, "days")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    format!("{head} · {}", format_days(&days))
}

fn format_solar(label: &str, offset_min: i64) -> String {
    if offset_min > 0 {
        format!("{label} +{offset_min}m")
    } else if offset_min < 0 {
        format!("{label} {offset_min}m")
    } else {
        label.to_string()
    }
}

pub fn schedule_action_summary(sch: &Value) -> String {
    let action = j_str(sch, "action");
    let fade = j_i64(sch, "fade_min").unwrap_or(0);
    let mut parts = Vec::new();
    match action {
        "off" => parts.push(tr("Off")),
        "scene" => parts.push(tr("Scene")),
        _ => {
            if let Some(bri) = j_i64(sch, "bri") {
                parts.push(format!("{} · {bri}%", tr("On")));
            } else {
                parts.push(tr("On"));
            }
        }
    }
    if fade > 0 {
        parts.push(trf("Fade {0} min", &[fade.to_string()]));
    }
    parts.join(" · ")
}

pub fn format_unix_ts(ts: i64) -> String {
    if ts <= 0 {
        return "--:--".into();
    }
    // Safe POSIX localtime_r wrapper via libc (already a workspace dependency).
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = ts as libc::time_t;
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if ok {
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    } else {
        let secs = ts.rem_euclid(86_400);
        format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    }
}

// ------------------------------------------------------------------------- Schedule draft

#[derive(Debug, Clone)]
pub struct ScheduleDraft {
    pub name: String,
    pub target_kind: String, // "light" | "room" | "zone"
    pub target_id: String,
    pub target_name: String,
    pub target_picker_open: bool,
    pub trigger_kind: String, // "time" | "sunrise" | "sunset"
    pub time: String,         // "HH:MM"
    pub offset_min: i32,
    pub days: HashSet<String>,
    pub action: String, // "on" | "off"
    pub bri: u8,
    pub fade_min: u16,
    pub error: Option<String>,
}

impl ScheduleDraft {
    pub fn new(target: Option<TargetOption>) -> Self {
        let (target_kind, target_id, target_name) = match target {
            Some(t) => (t.kind, t.id, t.label),
            None => (String::new(), String::new(), String::new()),
        };
        Self {
            name: String::new(),
            target_kind,
            target_id,
            target_name,
            target_picker_open: false,
            trigger_kind: "time".into(),
            time: "07:00".into(),
            offset_min: 0,
            days: DAYS_ORDER.iter().map(|d| (*d).to_string()).collect(),
            action: "on".into(),
            bri: 80,
            fade_min: 0,
            error: None,
        }
    }

    pub fn to_command(&self) -> Result<Value, String> {
        if self.target_id.is_empty() || self.target_kind.is_empty() {
            return Err(tr("Choose a target first"));
        }
        if self.trigger_kind == "time" && !valid_hhmm(&self.time) {
            return Err(tr("Invalid time (use HH:MM)"));
        }
        let days: Vec<&str> = DAYS_ORDER
            .iter()
            .copied()
            .filter(|d| self.days.contains(*d))
            .collect();
        let days = if days.is_empty() { DAYS_ORDER.to_vec() } else { days };
        let name = if self.name.trim().is_empty() {
            let short_target = self.target_name.split(" · ").next().unwrap_or(&self.target_name);
            if self.trigger_kind == "time" {
                format!("{short_target} {}", self.time.trim())
            } else {
                format!("{short_target} {}", self.trigger_kind)
            }
        } else {
            self.name.trim().to_string()
        };
        let bri = if self.action == "on" { json!(self.bri.clamp(1, 100)) } else { Value::Null };
        Ok(json!({
            "cmd": "schedule_create",
            "name": name,
            "target_kind": self.target_kind,
            "target_id": self.target_id,
            "trigger_kind": self.trigger_kind,
            "time": if self.trigger_kind == "time" { self.time.trim() } else { "" },
            "offset_min": if self.trigger_kind == "time" { 0 } else { self.offset_min },
            "days": days,
            "action": self.action,
            "bri": bri,
            "scene_id": "",
            "fade_min": self.fade_min,
        }))
    }
}

// ------------------------------------------------------------------------- Rule draft

#[derive(Debug, Clone)]
pub struct RuleDraft {
    pub name: String,
    pub kind: String,          // "follow" | "exclusive" | "companion"
    pub trigger_kind: String,  // "light" | "room"
    pub trigger_id: String,
    pub trigger_name: String,
    pub trigger_picker_open: bool,
    pub trigger_state: String, // "on" | "off"
    pub target_kind: String,   // "light" | "room"
    pub target_id: String,
    pub target_name: String,
    pub target_picker_open: bool,
    pub mirror_off: bool,
    pub match_bri: bool,
    pub only_if_off: bool,
    pub time_start: String,
    pub time_end: String,
    pub error: Option<String>,
}

impl RuleDraft {
    pub fn new(trigger: Option<TargetOption>, fallback_target: Option<TargetOption>) -> Self {
        let norm_kind = |k: String| if k == "zone" { "room".into() } else { k };
        let (trigger_kind, trigger_id, trigger_name) = match trigger {
            Some(t) => (norm_kind(t.kind), t.id, t.label),
            None => (String::new(), String::new(), String::new()),
        };
        let (target_kind, target_id, target_name) = match fallback_target {
            Some(t) => (norm_kind(t.kind), t.id, t.label),
            None => (String::new(), String::new(), String::new()),
        };
        Self {
            name: String::new(),
            kind: "follow".into(),
            trigger_kind,
            trigger_id,
            trigger_name,
            trigger_picker_open: false,
            trigger_state: "on".into(),
            target_kind,
            target_id,
            target_name,
            target_picker_open: false,
            mirror_off: true,
            match_bri: false,
            only_if_off: false,
            time_start: String::new(),
            time_end: String::new(),
            error: None,
        }
    }

    pub fn target_action(&self) -> &'static str {
        if self.kind == "exclusive" { "off" } else { "on" }
    }

    pub fn to_command(&self) -> Result<Value, String> {
        if self.trigger_id.is_empty() || self.target_id.is_empty() {
            return Err(tr("Choose trigger and target"));
        }
        let ts = self.time_start.trim();
        let te = self.time_end.trim();
        if (!ts.is_empty() && !valid_hhmm(ts)) || (!te.is_empty() && !valid_hhmm(te)) {
            return Err(tr("Invalid time (use HH:MM)"));
        }
        let name = if self.name.trim().is_empty() {
            let a = self.trigger_name.split(" · ").next().unwrap_or(&self.trigger_name);
            let b = self.target_name.split(" · ").next().unwrap_or(&self.target_name);
            format!("{a} → {b}")
        } else {
            self.name.trim().to_string()
        };
        let action = self.target_action();
        let mirror_off = if self.kind == "exclusive" { false } else { self.mirror_off };
        let match_bri = if self.kind == "exclusive" { false } else { self.match_bri };
        Ok(json!({
            "cmd": "rule_save",
            "rule": {
                "name": name,
                "enabled": true,
                "kind": self.kind,
                "trigger_kind": self.trigger_kind,
                "trigger_id": self.trigger_id,
                "trigger_state": self.trigger_state,
                "targets": [{
                    "kind": self.target_kind,
                    "id": self.target_id,
                    "action": action,
                    "bri": Value::Null,
                    "scene_id": "",
                }],
                "mirror_off": mirror_off,
                "match_bri": match_bri,
                "time_start": ts,
                "time_end": te,
                "only_if_off": self.only_if_off,
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorite_serialization_round_trip_and_dedup() {
        let mut favs = Vec::new();
        assert!(toggle_favorite(&mut favs, "room", "r1"));
        assert!(toggle_favorite(&mut favs, "light", "l1"));
        assert!(is_favorite(&favs, "room", "r1"));
        assert!(is_favorite(&favs, "light", "l1"));
        assert!(!is_favorite(&favs, "light", "l2"));

        let val = favorites_to_value(&favs);
        let parsed = parse_favorites(Some(&val));
        assert_eq!(parsed, favs);

        // Removing an existing favorite works cleanly.
        assert!(!toggle_favorite(&mut favs, "room", "r1"));
        assert_eq!(favs, vec![Favorite::new("light", "l1")]);

        // Ignores malformed entries and deduplicates mixed formats.
        let messy = json!([
            {"kind": "room", "id": "r1"},
            "room:r1",
            {"kind": "bogus", "id": "x"},
            {"kind": "light", "id": ""},
            "light:l2",
            42
        ]);
        assert_eq!(
            parse_favorites(Some(&messy)),
            vec![Favorite::new("room", "r1"), Favorite::new("light", "l2")]
        );
        assert_eq!(parse_favorites(None), Vec::<Favorite>::new());
    }

    #[test]
    fn room_filtering_handles_all_modes() {
        let room_on = json!({
            "id": "r-living",
            "kind": "room",
            "name": "Living Room",
            "on": true,
            "lights": [{"id": "l-lamp", "name": "Lamp"}]
        });
        let room_off = json!({
            "id": "r-bed",
            "kind": "room",
            "name": "Bedroom",
            "on": false,
            "lights": [{"id": "l-bed", "name": "Bedside"}]
        });
        let zone_on = json!({
            "id": "z-down",
            "kind": "zone",
            "name": "Downstairs",
            "on": true,
            "lights": [{"id": "l-lamp", "name": "Lamp"}]
        });

        let favs = vec![Favorite::new("light", "l-bed")];

        assert!(room_matches_filter(&room_on, RoomFilter::All, &favs));
        assert!(room_matches_filter(&room_off, RoomFilter::All, &favs));
        assert!(room_matches_filter(&zone_on, RoomFilter::All, &favs));

        assert!(room_matches_filter(&room_on, RoomFilter::On, &favs));
        assert!(!room_matches_filter(&room_off, RoomFilter::On, &favs));
        assert!(room_matches_filter(&zone_on, RoomFilter::On, &favs));

        assert!(room_matches_filter(&room_on, RoomFilter::Rooms, &favs));
        assert!(room_matches_filter(&room_off, RoomFilter::Rooms, &favs));
        assert!(!room_matches_filter(&zone_on, RoomFilter::Rooms, &favs));

        assert!(!room_matches_filter(&room_on, RoomFilter::Zones, &favs));
        assert!(room_matches_filter(&zone_on, RoomFilter::Zones, &favs));

        // Favorites matches room containing a favorited light OR a favorited room.
        assert!(!room_matches_filter(&room_on, RoomFilter::Favorites, &favs));
        assert!(room_matches_filter(&room_off, RoomFilter::Favorites, &favs));

        let fav_room = vec![Favorite::new("room", "z-down")];
        assert!(room_matches_filter(&zone_on, RoomFilter::Favorites, &fav_room));
    }

    #[test]
    fn exclude_all_on_set_serialization() {
        let mut set = HashSet::new();
        set.insert("r-bed".to_string());
        set.insert("r-kids".to_string());
        let val = string_set_to_value(&set);
        assert_eq!(val, json!(["r-bed", "r-kids"]));
        assert_eq!(parse_string_set(Some(&val)), set);
        assert_eq!(sorted_exclude_list(&set), vec!["r-bed", "r-kids"]);
    }

    #[test]
    fn schedule_and_rule_draft_commands() {
        let target = TargetOption { kind: "light".into(), id: "l-1".into(), label: "Desk".into() };
        let mut sd = ScheduleDraft::new(Some(target.clone()));
        sd.time = "25:00".into();
        assert!(sd.to_command().is_err());

        sd.time = "07:30".into();
        sd.bri = 75;
        sd.fade_min = 10;
        let cmd = sd.to_command().unwrap();
        assert_eq!(cmd["cmd"], "schedule_create");
        assert_eq!(cmd["target_kind"], "light");
        assert_eq!(cmd["target_id"], "l-1");
        assert_eq!(cmd["time"], "07:30");
        assert_eq!(cmd["bri"], 75);
        assert_eq!(cmd["fade_min"], 10);

        let target2 = TargetOption { kind: "room".into(), id: "r-2".into(), label: "Hall".into() };
        let mut rd = RuleDraft::new(Some(target), Some(target2));
        rd.kind = "exclusive".into();
        rd.only_if_off = false;
        let rcmd = rd.to_command().unwrap();
        assert_eq!(rcmd["cmd"], "rule_save");
        assert_eq!(rcmd["rule"]["kind"], "exclusive");
        assert_eq!(rcmd["rule"]["targets"][0]["action"], "off");
        assert_eq!(rcmd["rule"]["mirror_off"], false);
    }

    #[test]
    fn status_badge_lookup() {
        let stats = json!({
            "schedules": [
                {"id": "s1", "enabled": true, "target_kind": "light", "target_id": "l1"},
                {"id": "s2", "enabled": false, "target_kind": "light", "target_id": "l2"}
            ],
            "rules": [
                {
                    "id": "ru1",
                    "enabled": true,
                    "trigger_kind": "light",
                    "trigger_id": "l1",
                    "targets": [{"kind": "light", "id": "l3", "action": "on"}]
                }
            ]
        });
        assert!(has_active_schedule(&stats, "light", "l1"));
        assert!(!has_active_schedule(&stats, "light", "l2"));
        assert!(belongs_to_rule(&stats, "light", "l1"));
        assert!(!belongs_to_rule(&stats, "light", "l2"));
        assert!(belongs_to_rule(&stats, "light", "l3"));
    }

    #[test]
    fn time_validation_and_mirek_clamp() {
        assert!(valid_hhmm("00:00"));
        assert!(valid_hhmm("23:59"));
        assert!(!valid_hhmm("24:00"));
        assert!(!valid_hhmm("7:00"));
        assert!(!valid_hhmm("ab:cd"));

        let light = json!({"mirek_min": 200, "mirek_max": 450});
        assert_eq!(clamp_mirek(&light, 154), 200);
        assert_eq!(clamp_mirek(&light, 500), 450);
        assert_eq!(clamp_mirek(&light, 370), 370);
    }
}
