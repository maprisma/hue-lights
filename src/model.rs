//! Pure UI model helpers for Maprisma Hue Lights: tabs, room filters, pinned favorites,
//! All-On room exclusions, color/warmth/effect presets, schedule/rule draft builders,
//! formatting helpers, and unit tests.

use std::collections::{BTreeMap, HashSet};

use cosmic::iced::Color;
use maprisma_hue_lights::i18n::{tr, trf};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const FAVORITES_KEY: &str = "favorites";
pub const EXCLUDE_ALL_ON_KEY: &str = "exclude_all_on";
pub const CONFIRM_ALL_ON_KEY: &str = "confirm_all_on";
pub const SCHEDULE_MODE_KEY: &str = "schedule_mode";
pub const SKIN_KEY: &str = "skin";
pub const PANEL_ICON_KEY: &str = "panel_icon";
pub const SHOW_COUNT_KEY: &str = "show_count";
pub const FONT_SCALE_KEY: &str = "font_scale";

pub const DAYS_ORDER: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
pub const SCHEDULE_MODES: [&str; 2] = ["bridge", "local"];
pub const SKINS: [&str; 6] = ["native", "synthwave", "cozy", "terminal", "disco", "zen"];
pub const PANEL_ICONS: [&str; 8] = ["bulb", "lamp", "sun", "sparkle", "bolt", "flame", "moon", "dot"];
pub const FONT_SCALES: [&str; 3] = ["small", "normal", "large"];

pub const SYNTH_SOURCES: [&str; 3] = ["system", "mic", "demo"];
pub const SYNTH_MODES: [&str; 4] = ["club", "chill", "ambient", "strobe"];
pub const SYNTH_PALETTES: [&str; 6] = ["neon", "sunset", "ocean", "aurora", "fire", "pastel"];
pub const SYNTH_BANDS: [&str; 4] = ["all", "bass", "mids", "treble"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Lights,
    Map,
    Schedules,
    Rules,
    Synth,
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
    pub action: String, // "on" | "off" | "scene"
    pub bri: u8,
    pub scene_id: String,
    pub scene_name: String,
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
            scene_id: String::new(),
            scene_name: String::new(),
            fade_min: 0,
            error: None,
        }
    }

    pub fn for_scene(
        target_kind: String,
        target_id: String,
        target_name: String,
        scene_id: String,
        scene_name: String,
    ) -> Self {
        let mut draft = Self::new(Some(TargetOption {
            kind: target_kind,
            id: target_id,
            label: target_name.clone(),
        }));
        draft.action = "scene".into();
        draft.scene_id = scene_id;
        draft.scene_name = scene_name.clone();
        if !scene_name.is_empty() {
            draft.name = format!("{target_name} · {scene_name}");
        }
        draft.time = "20:00".into();
        draft
    }

    pub fn to_command(&self) -> Result<Value, String> {
        if self.target_id.is_empty() || self.target_kind.is_empty() {
            return Err(tr("Choose a target first"));
        }
        if self.trigger_kind == "time" && !valid_hhmm(&self.time) {
            return Err(tr("Invalid time (use HH:MM)"));
        }
        if self.action == "scene" && self.scene_id.is_empty() {
            return Err(tr("Choose a target first"));
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
        let scene_id = if self.action == "scene" { self.scene_id.as_str() } else { "" };
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
            "scene_id": scene_id,
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

// ------------------------------------------------------------------------- Light theme draft

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeDraft {
    pub light_id: String,
    pub name: String,
    pub hex: String,
    pub mirek: Option<u16>,
    pub bri: u8,
    pub effect: String,
    pub error: Option<String>,
}

fn is_valid_hex(hex: &str) -> bool {
    let s = hex.trim();
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

impl ThemeDraft {
    /// Captures a light's current color, warmth (mirek), brightness, and effect as a theme draft.
    pub fn from_light(light: &Value) -> Self {
        let light_id = j_str(light, "id").to_string();
        let raw_hex = j_str(light, "color").trim();
        let hex = if is_valid_hex(raw_hex) {
            raw_hex.to_ascii_lowercase()
        } else {
            "#ffd27a".to_string()
        };
        let mirek = light
            .get("mirek")
            .and_then(Value::as_i64)
            .filter(|m| (153..=500).contains(m))
            .map(|m| m as u16);
        let bri = light
            .get("bri")
            .and_then(Value::as_f64)
            .filter(|b| b.is_finite())
            .map(|b| b.round().clamp(1.0, 100.0) as u8)
            .unwrap_or(80);
        let raw_effect = j_str(light, "effect");
        let effect = if raw_effect.is_empty() { "no_effect".to_string() } else { raw_effect.to_string() };
        Self { light_id, name: String::new(), hex, mirek, bri, effect, error: None }
    }

    pub fn to_command(&self) -> Result<Value, String> {
        let trimmed = self.name.trim();
        if trimmed.is_empty() {
            return Err(tr("Enter a theme name"));
        }
        let hex = if is_valid_hex(&self.hex) { self.hex.to_ascii_lowercase() } else { "#ffd27a".to_string() };
        let mirek_val = self.mirek.map(|m| json!(m.clamp(153, 500))).unwrap_or(Value::Null);
        let eff = if self.effect.trim().is_empty() { "no_effect" } else { self.effect.trim() };
        Ok(json!({
            "cmd": "theme_save",
            "theme": {
                "name": trimmed,
                "hex": hex,
                "mirek": mirek_val,
                "bri": self.bri.clamp(1, 100),
                "effect": eff,
            }
        }))
    }
}

// ------------------------------------------------------------------------- Schedule mode

pub fn normalize_schedule_mode(mode: &str) -> &'static str {
    SCHEDULE_MODES.iter().find(|m| **m == mode).copied().unwrap_or("bridge")
}

pub fn schedule_mode_label(mode: &str) -> String {
    match normalize_schedule_mode(mode) {
        "local" => tr("On this PC (live)"),
        _ => tr("On Hue Bridge"),
    }
}

// ------------------------------------------------------------------------- Skins, panel icons & font scale

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinStyle {
    pub accent: Color,
    pub border_tint: Color,
    pub bg_tint: Color,
    pub border_width: f32,
}

pub fn normalize_skin(skin: &str) -> &'static str {
    SKINS.iter().find(|s| **s == skin).copied().unwrap_or("native")
}

/// Visual accent palette for non-native skins; `None` for `"native"` so the desktop theme stays untouched.
pub fn skin_style(skin: &str) -> Option<SkinStyle> {
    match normalize_skin(skin) {
        "synthwave" => Some(SkinStyle {
            accent: Color::from_rgb(1.0, 0.18, 0.62),
            border_tint: Color::from_rgb(0.0, 0.88, 1.0),
            bg_tint: Color::from_rgb(0.38, 0.10, 0.54),
            border_width: 1.5,
        }),
        "cozy" => Some(SkinStyle {
            accent: Color::from_rgb(0.98, 0.65, 0.24),
            border_tint: Color::from_rgb(0.95, 0.56, 0.22),
            bg_tint: Color::from_rgb(0.48, 0.26, 0.08),
            border_width: 1.25,
        }),
        "terminal" => Some(SkinStyle {
            accent: Color::from_rgb(0.20, 0.92, 0.42),
            border_tint: Color::from_rgb(0.18, 0.85, 0.38),
            bg_tint: Color::from_rgb(0.06, 0.28, 0.12),
            border_width: 1.25,
        }),
        "disco" => Some(SkinStyle {
            accent: Color::from_rgb(0.86, 0.30, 1.0),
            border_tint: Color::from_rgb(1.0, 0.78, 0.18),
            bg_tint: Color::from_rgb(0.40, 0.14, 0.52),
            border_width: 1.5,
        }),
        "zen" => Some(SkinStyle {
            accent: Color::from_rgb(0.42, 0.78, 0.66),
            border_tint: Color::from_rgb(0.48, 0.76, 0.65),
            bg_tint: Color::from_rgb(0.16, 0.32, 0.27),
            border_width: 1.0,
        }),
        _ => None,
    }
}

pub fn skin_label(skin: &str) -> String {
    match normalize_skin(skin) {
        "synthwave" => tr("Synthwave"),
        "cozy" => tr("Cozy"),
        "terminal" => tr("Terminal"),
        "disco" => tr("Disco"),
        "zen" => tr("Zen"),
        _ => tr("Native"),
    }
}

pub fn normalize_panel_icon(icon_key: &str) -> &'static str {
    PANEL_ICONS.iter().find(|k| **k == icon_key).copied().unwrap_or("bulb")
}

pub fn panel_icon_label(icon_key: &str) -> String {
    match normalize_panel_icon(icon_key) {
        "lamp" => tr("Lamp"),
        "sun" => tr("icon_sun"),
        "sparkle" => tr("Sparkle"),
        "bolt" => tr("Bolt"),
        "flame" => tr("Flame"),
        "moon" => tr("Moon"),
        "dot" => tr("Dot"),
        _ => tr("Bulb"),
    }
}

pub fn panel_icon_glyph(icon_key: &str) -> &'static str {
    match normalize_panel_icon(icon_key) {
        "lamp" => "⌅",
        "sun" => "☀",
        "sparkle" => "✦",
        "bolt" => "⚡",
        "flame" => "▲",
        "moon" => "☾",
        "dot" => "●",
        _ => "◉",
    }
}

/// Symbolic 16×16 SVG bytes for non-bulb panel icons (`None` for `"bulb"`, which uses the bundled bulb SVGs).
pub fn panel_icon_svg(icon_key: &str) -> Option<&'static [u8]> {
    match normalize_panel_icon(icon_key) {
        "lamp" => Some(br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path fill="currentColor" d="M4.5 2h7l2.5 7H2L4.5 2zm2.75 8h1.5v3h2.25v1.5h-6V13h2.25v-3z"/></svg>"#),
        "sun" => Some(br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path fill="currentColor" d="M8 4.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7zM7.25 1h1.5v2h-1.5V1zm0 12h1.5v2h-1.5v-2zM1 7.25h2v1.5H1v-1.5zm12 0h2v1.5h-2v-1.5zM2.5 3.56l1.06-1.06 1.41 1.41-1.06 1.06L2.5 3.56zm8.53 8.53 1.06-1.06 1.41 1.41-1.06 1.06-1.41-1.41zM12.44 2.5l1.06 1.06-1.41 1.41-1.06-1.06 1.41-1.41zM3.91 11.03l1.06 1.06-1.41 1.41-1.06-1.06 1.41-1.41z"/></svg>"#),
        "sparkle" => Some(br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path fill="currentColor" d="M8 1l1.8 5.2L15 8l-5.2 1.8L8 15l-1.8-5.2L1 8l5.2-1.8L8 1z"/></svg>"#),
        "bolt" => Some(br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path fill="currentColor" d="M9.5 1L3 9h4.5L6.5 15 13 7H8.5L9.5 1z"/></svg>"#),
        "flame" => Some(br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path fill="currentColor" d="M8.5 1.2C10.8 3.6 13 6 13 9.2A5 5 0 0 1 3 9.2c0-1.9.9-3.6 2.1-4.8.1 1.2.6 2 1.4 2.4-.3-2 .7-4.1 2-5.6z"/></svg>"#),
        "moon" => Some(br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><path fill="currentColor" d="M10.8 1.5A6.5 6.5 0 1 0 14.5 11a5.3 5.3 0 0 1-3.7-9.5z"/></svg>"#),
        "dot" => Some(br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16"><circle fill="currentColor" cx="8" cy="8" r="5"/></svg>"#),
        _ => None,
    }
}

pub fn normalize_font_scale(scale: &str) -> &'static str {
    FONT_SCALES.iter().find(|s| **s == scale).copied().unwrap_or("normal")
}

pub fn font_scale_factor(scale: &str) -> f32 {
    match normalize_font_scale(scale) {
        "small" => 0.9,
        "large" => 1.15,
        _ => 1.0,
    }
}

pub fn font_scale_label(scale: &str) -> String {
    match normalize_font_scale(scale) {
        "small" => tr("Small"),
        "large" => tr("Large"),
        _ => tr("Normal"),
    }
}

// ------------------------------------------------------------------------- Synth helpers

pub fn synth_palette_swatch(palette: &str) -> Color {
    match palette {
        "sunset" => Color::from_rgb(1.0, 0.35, 0.25),
        "ocean" => Color::from_rgb(0.0, 0.70, 0.86),
        "aurora" => Color::from_rgb(0.0, 0.90, 0.55),
        "fire" => Color::from_rgb(1.0, 0.42, 0.0),
        "pastel" => Color::from_rgb(0.88, 0.73, 1.0),
        _ => Color::from_rgb(1.0, 0.0, 0.50), // neon
    }
}

pub fn synth_source_label(source: &str) -> String {
    match source {
        "mic" => tr("Microphone"),
        "demo" => tr("Demo beat"),
        _ => tr("System audio"),
    }
}

pub fn synth_mode_label(mode: &str) -> String {
    match mode {
        "chill" => tr("Chill"),
        "ambient" => tr("Ambient"),
        "strobe" => tr("Strobe"),
        _ => tr("Club"),
    }
}

pub fn synth_palette_label(palette: &str) -> String {
    match palette {
        "sunset" => tr("Sunset"),
        "ocean" => tr("Ocean"),
        "aurora" => tr("Aurora"),
        "fire" => tr("Fire"),
        "pastel" => tr("Pastel"),
        _ => tr("Neon"),
    }
}

pub fn synth_band_label(band: &str) -> String {
    match band {
        "bass" => tr("Bass"),
        "mids" => tr("Mids"),
        "treble" => tr("Treble"),
        _ => tr("All"),
    }
}

/// Returns `(light_id, light_name)` pairs targeted by the synth for `room_id` (`""` = all lights).
pub fn synth_target_lights(stats: &Value, room_id: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for room in j_list(stats, "rooms") {
        if !room_id.is_empty() && j_str(room, "id") != room_id {
            continue;
        }
        for light in j_list(room, "lights") {
            let lid = j_str(light, "id");
            if lid.is_empty() || !seen.insert(lid.to_string()) {
                continue;
            }
            out.push((lid.to_string(), j_str(light, "name").to_string()));
        }
    }
    out
}

// ------------------------------------------------------------------------- Floorplan Studio & Bridge Sync Model

pub const FLOORPLAN_TEMPLATES: [&str; 6] = [
    "auto_bridge",
    "studio_loft",
    "one_bed_apt",
    "three_room_apt",
    "family_house",
    "desk_studio",
];

pub const FLOORPLAN_STAMP_KINDS: [&str; 8] = [
    "door", "window", "sofa", "tv", "desk", "bed", "dining", "kitchen",
];

pub const LAMP_ICONS: [&str; 12] = [
    "bulb", "spot", "strip", "floor", "table", "pendant",
    "ceiling", "wall", "bar", "desk", "outdoor", "candle",
];

pub const FLOORPLAN_ARCHETYPES: [&str; 18] = [
    "living_room", "bedroom", "kitchen", "dining", "office", "bathroom",
    "hallway", "balcony", "terrace", "kids_bedroom", "toilet", "lounge",
    "tv", "reading", "studio", "garage", "garden", "other",
];

fn default_version() -> u32 { 1 }
fn default_template_id() -> String { "auto_bridge".to_string() }
fn default_true() -> bool { true }
fn default_canvas_w() -> f64 { 1600.0 }
fn default_canvas_h() -> f64 { 1000.0 }
fn default_grid() -> i32 { 20 }
fn default_zoom() -> f64 { 1.0 }
fn default_bg_opacity() -> f64 { 0.35 }
fn default_bg_fit() -> String { "contain".to_string() }
fn default_box_kind() -> String { "room".to_string() }
fn default_archetype() -> String { "other".to_string() }
pub fn default_lamp_icon() -> String { "bulb".to_string() }

pub fn normalize_lamp_icon(icon: &str) -> &'static str {
    let mapped = match icon {
        "floor_lamp" => "floor",
        "table_lamp" => "table",
        "desk_lamp" => "desk",
        "wall_lamp" => "wall",
        "ceiling_lamp" => "ceiling",
        "pendant_lamp" => "pendant",
        "lightstrip" => "strip",
        "spotlight" => "spot",
        other => other,
    };
    LAMP_ICONS.iter().find(|k| **k == mapped).copied().unwrap_or("bulb")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloorplanCanvas {
    #[serde(default = "default_canvas_w")]
    pub width: f64,
    #[serde(default = "default_canvas_h")]
    pub height: f64,
    #[serde(default = "default_grid")]
    pub grid: i32,
    #[serde(default = "default_true")]
    pub snap: bool,
    #[serde(default = "default_zoom")]
    pub zoom: f64,
    #[serde(default)]
    pub pan_x: f64,
    #[serde(default)]
    pub pan_y: f64,
}

impl Default for FloorplanCanvas {
    fn default() -> Self {
        Self {
            width: 1600.0,
            height: 1000.0,
            grid: 20,
            snap: true,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloorplanBackground {
    #[serde(default)]
    pub path: String,
    #[serde(default = "default_bg_opacity")]
    pub opacity: f64,
    #[serde(default = "default_bg_fit")]
    pub fit: String,
}

impl Default for FloorplanBackground {
    fn default() -> Self {
        Self { path: String::new(), opacity: 0.35, fit: "contain".to_string() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloorplanBox {
    pub id: String,
    #[serde(default = "default_box_kind")]
    pub kind: String, // "room" | "zone"
    #[serde(default)]
    pub hue_id: Option<String>,
    pub name: String,
    #[serde(default = "default_archetype")]
    pub archetype: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default)]
    pub entertainment_area_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloorplanStamp {
    pub id: String,
    pub kind: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[serde(default)]
    pub rot: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloorplanLamp {
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub z: f64,
    #[serde(default = "default_lamp_icon")]
    pub icon: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FloorplanDocument {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub bridge_id: String,
    #[serde(default = "default_template_id")]
    pub template_id: String,
    #[serde(default)]
    pub entertainment_id: Option<String>,
    #[serde(default)]
    pub canvas: FloorplanCanvas,
    #[serde(default = "default_true")]
    pub show_wires: bool,
    #[serde(default = "default_true")]
    pub show_glows: bool,
    #[serde(default = "default_true")]
    pub show_furniture: bool,
    #[serde(default)]
    pub auto_sync: bool,
    #[serde(default)]
    pub auto_sync_rooms: bool,
    #[serde(default)]
    pub background: FloorplanBackground,
    #[serde(default)]
    pub boxes: Vec<FloorplanBox>,
    #[serde(default)]
    pub stamps: Vec<FloorplanStamp>,
    #[serde(default)]
    pub lamps: BTreeMap<String, FloorplanLamp>,
}

impl Default for FloorplanDocument {
    fn default() -> Self {
        Self {
            version: 1,
            bridge_id: String::new(),
            template_id: "auto_bridge".to_string(),
            entertainment_id: None,
            canvas: FloorplanCanvas::default(),
            show_wires: true,
            show_glows: true,
            show_furniture: true,
            auto_sync: false,
            auto_sync_rooms: false,
            background: FloorplanBackground::default(),
            boxes: Vec::new(),
            stamps: Vec::new(),
            lamps: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntertainmentLocation {
    #[serde(default)]
    pub service_id: String,
    #[serde(default)]
    pub service_type: String,
    #[serde(default)]
    pub light_id: String,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub z: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntertainmentArea {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub configuration_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub locations: Vec<EntertainmentLocation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BridgeLightInfo {
    pub id: String,
    pub device_id: String,
    pub name: String,
    pub on: bool,
    pub bri: f64,
    pub color: String,
    pub reachable: bool,
    pub room_id: String,
    pub room_name: String,
}

pub fn parse_floorplan(stats: &Value) -> Option<FloorplanDocument> {
    let raw = stats.get("floorplan")?;
    if !raw.is_object() {
        return None;
    }
    let mut doc: FloorplanDocument = serde_json::from_value(raw.clone()).ok()?;
    for lamp in doc.lamps.values_mut() {
        lamp.icon = normalize_lamp_icon(&lamp.icon).to_string();
    }
    if doc.background.fit != "cover" {
        doc.background.fit = "contain".to_string();
    }
    doc.background.opacity = doc.background.opacity.clamp(0.05, 1.0);
    Some(doc)
}

pub fn parse_entertainment_areas(stats: &Value) -> Vec<EntertainmentArea> {
    let Some(arr) = stats.get("entertainment_areas").and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| serde_json::from_value::<EntertainmentArea>(item.clone()).ok())
        .filter(|a| !a.id.is_empty())
        .collect()
}

/// Extracts every unique light from `stats["rooms"]` (preferring room ownership over zone/other),
/// preserving `device_id` for Bridge 1-room-per-device sync checks.
pub fn extract_bridge_lights(stats: &Value) -> Vec<BridgeLightInfo> {
    let mut by_id: BTreeMap<String, BridgeLightInfo> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for room in j_list(stats, "rooms") {
        let rkind = j_str(room, "kind");
        let rid = if rkind == "room" { j_str(room, "id") } else { "" };
        let rname = if rkind == "other" {
            tr("Other lights")
        } else {
            j_str(room, "name").to_string()
        };
        for light in j_list(room, "lights") {
            let lid = j_str(light, "id");
            if lid.is_empty() {
                continue;
            }
            let dev_id = j_str(light, "device_id").to_string();
            let info = BridgeLightInfo {
                id: lid.to_string(),
                device_id: dev_id,
                name: j_str(light, "name").to_string(),
                on: j_bool(light, "on"),
                bri: light.get("bri").and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(0.0),
                color: j_str(light, "color").to_string(),
                reachable: light.get("reachable").and_then(Value::as_bool) != Some(false),
                room_id: rid.to_string(),
                room_name: if rkind == "zone" { String::new() } else { rname.clone() },
            };
            match by_id.get_mut(lid) {
                Some(existing) => {
                    if existing.room_id.is_empty() && !info.room_id.is_empty() {
                        existing.room_id = info.room_id;
                        existing.room_name = info.room_name;
                    }
                    if existing.device_id.is_empty() && !info.device_id.is_empty() {
                        existing.device_id = info.device_id;
                    }
                }
                None => {
                    order.push(lid.to_string());
                    by_id.insert(lid.to_string(), info);
                }
            }
        }
    }
    order.into_iter().filter_map(|id| by_id.remove(&id)).collect()
}

/// Returns a compact schedule badge (e.g. `"⏰ 20:00"` or `"⏰"`) if the room/zone has an active schedule.
pub fn room_active_schedule_badge(stats: &Value, hue_id: &str) -> Option<String> {
    if hue_id.is_empty() {
        return None;
    }
    for sch in j_list(stats, "schedules") {
        if !j_bool(sch, "enabled") || j_str(sch, "target_id") != hue_id {
            continue;
        }
        if !matches!(j_str(sch, "target_kind"), "room" | "zone") {
            continue;
        }
        let kind = j_str(sch, "trigger_kind");
        let label = match kind {
            "sunrise" => format!("⏰ {}", tr("Sunrise")),
            "sunset" => format!("⏰ {}", tr("Sunset")),
            _ => {
                let t = j_str(sch, "time");
                if t.is_empty() { "⏰".to_string() } else { format!("⏰ {t}") }
            }
        };
        return Some(label);
    }
    None
}

// ------------------------------------------------------------------------- Bridge IPC command builders

pub fn cmd_floorplan_save(doc: &FloorplanDocument) -> Value {
    let mut normalized = doc.clone();
    normalized.auto_sync_rooms = normalized.auto_sync;
    json!({
        "cmd": "floorplan_save",
        "floorplan": serde_json::to_value(&normalized).unwrap_or(Value::Null),
    })
}

pub fn cmd_floorplan_template(template_id: &str) -> Value {
    json!({ "cmd": "floorplan_template", "template_id": template_id })
}

pub fn cmd_floorplan_bg_set(path: &str, opacity: f64, fit: &str) -> Value {
    json!({
        "cmd": "floorplan_bg_set",
        "path": path,
        "opacity": opacity.clamp(0.05, 1.0),
        "fit": if fit == "cover" { "cover" } else { "contain" },
    })
}

pub fn cmd_floorplan_bg_update(opacity: f64, fit: &str) -> Value {
    json!({
        "cmd": "floorplan_bg_update",
        "opacity": opacity.clamp(0.05, 1.0),
        "fit": if fit == "cover" { "cover" } else { "contain" },
    })
}

pub fn cmd_floorplan_bg_clear() -> Value {
    json!({ "cmd": "floorplan_bg_clear" })
}

pub fn cmd_light_rename(id: &str, name: &str) -> Value {
    json!({ "cmd": "light_rename", "id": id, "name": name })
}

#[allow(dead_code)]
pub fn cmd_room_create(kind: &str, name: &str, archetype: &str, child_ids: &[String]) -> Value {
    let key = if kind == "zone" { "light_ids" } else { "device_ids" };
    json!({
        "cmd": "room_create",
        "kind": if kind == "zone" { "zone" } else { "room" },
        "name": name,
        "archetype": archetype,
        key: child_ids,
    })
}

pub fn cmd_room_update(
    id: &str,
    kind: &str,
    name: Option<&str>,
    archetype: Option<&str>,
    children: Option<&[String]>,
) -> Value {
    let mut obj = json!({
        "cmd": "room_update",
        "id": id,
        "kind": if kind == "zone" { "zone" } else { "room" },
    });
    if let Some(n) = name {
        obj["name"] = json!(n);
    }
    if let Some(a) = archetype {
        obj["archetype"] = json!(a);
    }
    if let Some(c) = children {
        obj["children"] = json!(c);
    }
    obj
}

pub fn cmd_room_delete(id: &str, kind: &str) -> Value {
    json!({
        "cmd": "room_delete",
        "id": id,
        "kind": if kind == "zone" { "zone" } else { "room" },
    })
}

pub fn cmd_scene_create(room_id: &str, name: &str) -> Value {
    json!({ "cmd": "scene_create", "room_id": room_id, "name": name })
}

pub fn cmd_floorplan_sync_rooms() -> Value {
    json!({ "cmd": "floorplan_sync_rooms" })
}

pub fn cmd_floorplan_sync_all() -> Value {
    json!({ "cmd": "floorplan_sync_all" })
}

pub fn cmd_entertainment_sync(area_id: &str, box_id: &str) -> Value {
    json!({ "cmd": "entertainment_sync", "area_id": area_id, "box_id": box_id })
}

// ------------------------------------------------------------------------- Floorplan localization labels

pub fn template_label(id: &str) -> String {
    match id {
        "studio_loft" => tr("Studio / Loft"),
        "one_bed_apt" => tr("1-Bedroom Apartment"),
        "three_room_apt" => tr("3-Room Apartment"),
        "family_house" => tr("Family House + Terrace"),
        "desk_studio" => tr("Desk / Gaming Studio"),
        _ => tr("Auto from Hue Bridge"),
    }
}

pub fn stamp_kind_label(kind: &str) -> String {
    match kind {
        "door" => tr("Door"),
        "window" => tr("Window"),
        "sofa" => tr("Sofa"),
        "tv" => tr("TV"),
        "desk" => tr("Desk"),
        "bed" => tr("Bed"),
        "dining" => tr("Dining table"),
        "kitchen" => tr("Kitchen counter"),
        other => other.to_string(),
    }
}

pub fn lamp_icon_label(icon: &str) -> String {
    match normalize_lamp_icon(icon) {
        "spot" => tr("Spotlight"),
        "strip" => tr("Lightstrip"),
        "floor" => tr("Floor lamp"),
        "table" => tr("Table lamp"),
        "pendant" => tr("Pendant"),
        "ceiling" => tr("Ceiling"),
        "wall" => tr("Wall sconce"),
        "bar" => tr("Play bar"),
        "desk" => tr("Desk lamp"),
        "outdoor" => tr("Outdoor"),
        "candle" => tr("Candle"),
        _ => tr("Bulb"),
    }
}

pub fn lamp_icon_glyph(icon: &str) -> &'static str {
    match normalize_lamp_icon(icon) {
        "spot" => "◎",
        "strip" => "═",
        "floor" => "⌅",
        "table" => "⏅",
        "pendant" => "⏚",
        "ceiling" => "◠",
        "wall" => "◧",
        "bar" => "▬",
        "desk" => "⌙",
        "outdoor" => "☀",
        "candle" => "▲",
        _ => "◉",
    }
}

pub fn archetype_label(arch: &str) -> String {
    match arch {
        "living_room" => tr("Living Room"),
        "bedroom" => tr("Bedroom"),
        "kitchen" => tr("Kitchen"),
        "dining" => tr("Dining"),
        "office" => tr("Office"),
        "bathroom" => tr("Bathroom"),
        "hallway" => tr("Hallway"),
        "balcony" => tr("Balcony"),
        "terrace" => tr("Terrace"),
        "kids_bedroom" => tr("Kids Bedroom"),
        "toilet" => tr("Toilet"),
        "lounge" => tr("Lounge"),
        "tv" => tr("TV"),
        "reading" => tr("Reading"),
        "studio" => tr("Studio"),
        "garage" => tr("Garage"),
        "garden" => tr("Garden"),
        _ => tr("Other"),
    }
}

pub fn archetype_glyph(arch: &str) -> &'static str {
    match arch {
        "living_room" | "lounge" | "tv" => "▢",
        "bedroom" | "kids_bedroom" | "guest_room" | "nursery" => "☾",
        "kitchen" | "dining" => "◈",
        "office" | "computer" | "studio" | "reading" => "⌘",
        "bathroom" | "toilet" => "◎",
        "balcony" | "terrace" | "garden" | "porch" => "✿",
        "hallway" | "staircase" => "⇄",
        "garage" | "carport" => "⌂",
        _ => "◇",
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

    #[test]
    fn skins_normalize_and_style() {
        assert_eq!(normalize_skin(""), "native");
        assert_eq!(normalize_skin("bogus"), "native");
        assert_eq!(skin_style("native"), None);
        for skin in SKINS {
            assert_eq!(normalize_skin(skin), skin);
            assert!(!skin_label(skin).is_empty());
            if skin == "native" {
                assert!(skin_style(skin).is_none());
            } else {
                let st = skin_style(skin).expect("non-native skin should have a style");
                assert!(st.border_width >= 1.0);
            }
        }
    }

    #[test]
    fn panel_icons_normalize_glyphs_and_svgs() {
        assert_eq!(normalize_panel_icon(""), "bulb");
        assert_eq!(normalize_panel_icon("invalid"), "bulb");
        assert!(panel_icon_svg("bulb").is_none());
        for icon in PANEL_ICONS {
            assert_eq!(normalize_panel_icon(icon), icon);
            assert!(!panel_icon_label(icon).is_empty());
            assert!(!panel_icon_glyph(icon).is_empty());
            if icon != "bulb" {
                let svg = panel_icon_svg(icon).expect("non-bulb panel icon must have symbolic SVG");
                let text = std::str::from_utf8(svg).expect("valid utf8 svg");
                assert!(text.contains("<svg") && text.contains("currentColor"));
            }
        }
    }

    #[test]
    fn font_scale_and_schedule_mode_helpers() {
        assert_eq!(normalize_font_scale(""), "normal");
        assert_eq!(normalize_font_scale("huge"), "normal");
        assert_eq!(normalize_font_scale("small"), "small");
        assert_eq!(normalize_font_scale("large"), "large");
        assert!(font_scale_factor("small") < font_scale_factor("normal"));
        assert!(font_scale_factor("normal") < font_scale_factor("large"));
        for sc in FONT_SCALES {
            assert!(!font_scale_label(sc).is_empty());
        }

        assert_eq!(normalize_schedule_mode(""), "bridge");
        assert_eq!(normalize_schedule_mode("cloud"), "bridge");
        assert_eq!(normalize_schedule_mode("bridge"), "bridge");
        assert_eq!(normalize_schedule_mode("local"), "local");
        assert!(!schedule_mode_label("bridge").is_empty());
        assert!(!schedule_mode_label("local").is_empty());
    }

    #[test]
    fn theme_draft_captures_light_and_builds_command() {
        let ct_light = json!({
            "id": "l-1",
            "color": "#FFD9A0",
            "mirek": 320,
            "bri": 72.4,
            "effect": "candle"
        });
        let mut draft = ThemeDraft::from_light(&ct_light);
        assert_eq!(draft.light_id, "l-1");
        assert_eq!(draft.hex, "#ffd9a0");
        assert_eq!(draft.mirek, Some(320));
        assert_eq!(draft.bri, 72);
        assert_eq!(draft.effect, "candle");
        assert!(draft.to_command().is_err()); // empty name rejected

        draft.name = "  Cozy Desk  ".into();
        let cmd = draft.to_command().unwrap();
        assert_eq!(cmd["cmd"], "theme_save");
        assert_eq!(cmd["theme"]["name"], "Cozy Desk");
        assert_eq!(cmd["theme"]["hex"], "#ffd9a0");
        assert_eq!(cmd["theme"]["mirek"], 320);
        assert_eq!(cmd["theme"]["bri"], 72);
        assert_eq!(cmd["theme"]["effect"], "candle");

        // Color-only light without mirek or effect falls back cleanly.
        let color_light = json!({"id": "l-2", "color": "#ff2e93", "bri": 100});
        let mut draft2 = ThemeDraft::from_light(&color_light);
        draft2.name = "Neon Pink".into();
        let cmd2 = draft2.to_command().unwrap();
        assert_eq!(cmd2["theme"]["hex"], "#ff2e93");
        assert_eq!(cmd2["theme"]["mirek"], Value::Null);
        assert_eq!(cmd2["theme"]["bri"], 100);
        assert_eq!(cmd2["theme"]["effect"], "no_effect");
    }

    #[test]
    fn synth_target_lights_filters_and_deduplicates() {
        let stats = json!({
            "rooms": [
                {"id": "r1", "kind": "room", "lights": [{"id": "l1", "name": "Desk"}, {"id": "l2", "name": "Floor"}]},
                {"id": "z1", "kind": "zone", "lights": [{"id": "l1", "name": "Desk"}, {"id": "l3", "name": "Strip"}]}
            ]
        });
        let all = synth_target_lights(&stats, "");
        assert_eq!(all.len(), 3);
        let r1 = synth_target_lights(&stats, "r1");
        assert_eq!(r1, vec![("l1".into(), "Desk".into()), ("l2".into(), "Floor".into())]);
        let missing = synth_target_lights(&stats, "r999");
        assert!(missing.is_empty());
    }

    #[test]
    fn floorplan_deserialization_icons_and_ipc_commands() {
        let stats = json!({
            "rooms": [{
                "id": "11111111-1111-1111-1111-111111111111",
                "kind": "room",
                "name": "Living Room",
                "lights": [{
                    "id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                    "device_id": "dddddddd-dddd-dddd-dddd-dddddddddddd",
                    "name": "Arc Lamp",
                    "on": true,
                    "bri": 85.0,
                    "color": "#ff7a3c",
                    "reachable": true
                }]
            }],
            "entertainment_areas": [{
                "id": "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee",
                "name": "TV Sync",
                "configuration_type": "screen",
                "status": "inactive",
                "locations": [{"service_id": "1234", "service_type": "entertainment", "light_id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "x": -0.5, "y": 0.8, "z": 0.0}]
            }],
            "floorplan": {
                "version": 1,
                "bridge_id": "001788FFFE123456",
                "template_id": "family_house",
                "auto_sync": true,
                "background": {"path": "/tmp/bg.png", "opacity": 0.4, "fit": "cover"},
                "boxes": [{
                    "id": "box-1", "kind": "room", "hue_id": "11111111-1111-1111-1111-111111111111",
                    "name": "Living Room", "archetype": "living_room", "x": 80.0, "y": 80.0, "w": 420.0, "h": 300.0
                }],
                "stamps": [{"id": "st-1", "kind": "sofa", "x": 150.0, "y": 200.0, "w": 160.0, "h": 70.0, "rot": 90}],
                "lamps": {
                    "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa": {"x": 160.0, "y": 140.0, "z": 0.25, "icon": "pendant"},
                    "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb": {"x": 200.0, "y": 180.0}
                }
            }
        });

        let doc = parse_floorplan(&stats).expect("floorplan should deserialize");
        assert_eq!(doc.template_id, "family_house");
        assert!(doc.auto_sync);
        assert_eq!(doc.background.fit, "cover");
        assert_eq!(doc.boxes.len(), 1);
        assert_eq!(doc.stamps.len(), 1);
        assert_eq!(doc.lamps["aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"].icon, "pendant");
        // Missing icon defaults to "bulb"
        assert_eq!(doc.lamps["bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"].icon, "bulb");

        let ent = parse_entertainment_areas(&stats);
        assert_eq!(ent.len(), 1);
        assert_eq!(ent[0].name, "TV Sync");

        let lights = extract_bridge_lights(&stats);
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].device_id, "dddddddd-dddd-dddd-dddd-dddddddddddd");

        // All 12 lamp icons normalize & have labels/glyphs
        assert_eq!(LAMP_ICONS.len(), 12);
        for ic in LAMP_ICONS {
            assert_eq!(normalize_lamp_icon(ic), ic);
            assert!(!lamp_icon_label(ic).is_empty());
            assert!(!lamp_icon_glyph(ic).is_empty());
        }
        assert_eq!(normalize_lamp_icon("floor_lamp"), "floor");
        assert_eq!(normalize_lamp_icon("bogus"), "bulb");

        // IPC builders match hue-lights.py command names
        assert_eq!(cmd_floorplan_save(&doc)["cmd"], "floorplan_save");
        assert_eq!(cmd_floorplan_template("studio_loft")["cmd"], "floorplan_template");
        assert_eq!(cmd_floorplan_bg_set("/a.png", 0.5, "contain")["cmd"], "floorplan_bg_set");
        assert_eq!(cmd_floorplan_bg_update(0.5, "cover")["cmd"], "floorplan_bg_update");
        assert_eq!(cmd_floorplan_bg_clear()["cmd"], "floorplan_bg_clear");
        assert_eq!(cmd_light_rename("l1", "Arc")["cmd"], "light_rename");
        assert_eq!(cmd_room_create("room", "Office", "office", &["d1".into()])["device_ids"][0], "d1");
        assert_eq!(cmd_room_create("zone", "Downstairs", "other", &["l1".into()])["light_ids"][0], "l1");
        assert_eq!(cmd_room_update("r1", "room", Some("New"), None, None)["cmd"], "room_update");
        assert_eq!(cmd_room_delete("r1", "room")["cmd"], "room_delete");
        assert_eq!(cmd_scene_create("r1", "Sunset")["cmd"], "scene_create");
        assert_eq!(cmd_floorplan_sync_rooms()["cmd"], "floorplan_sync_rooms");
        assert_eq!(cmd_floorplan_sync_all()["cmd"], "floorplan_sync_all");
        assert_eq!(cmd_entertainment_sync("e1", "box-1")["cmd"], "entertainment_sync");

        // Scene schedule draft
        let sd = ScheduleDraft::for_scene("room".into(), "r1".into(), "Living".into(), "sc1".into(), "Sunset".into());
        let scmd = sd.to_command().unwrap();
        assert_eq!(scmd["action"], "scene");
        assert_eq!(scmd["scene_id"], "sc1");
    }
}
