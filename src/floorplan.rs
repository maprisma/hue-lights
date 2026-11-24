//! Interactive Visual Floorplan & Spatial Light Studio (`Tab::Map`).
//!
//! Renders a 2D spatial studio over an optional custom floorplan image with
//! resizable Room/Zone boxes, minimalist architectural vector stamps, live
//! radial lamp halos, 12 selectable vector lamp icons, typed Bezier rule wires,
//! collapsible lamp sidebar, and context inspectors (Lamp, Room/Scenes, Stamp, Wire).

use std::collections::{HashMap, HashSet};
use std::f32::consts::{FRAC_PI_2, PI};
use std::path::Path;

use cosmic::applet::menu_button;
use cosmic::iced::alignment;
use cosmic::iced::mouse;
use cosmic::iced::widget::canvas::{
    self, Action, Event, Frame, Geometry, LineCap, LineDash, LineJoin, Path as CanvasPath,
    Program, Stroke, Style, Text as CanvasText,
};
use cosmic::iced::widget::{
    container as iced_container, image as iced_image, stack as iced_stack, text as iced_text,
};
use cosmic::iced::{
    Alignment, Background, Border, Color, ContentFit, Length, Pixels, Point, Radians, Rectangle,
    Size, Vector,
};
use cosmic::widget::{
    button, column, container, divider, flex_row, icon, row, scrollable, slider, space, text,
    text_input, toggler,
};
use cosmic::{Element, theme};
use maprisma_hue_lights::corners;
use maprisma_hue_lights::i18n::{tr, trf};
use serde_json::{Value, json};

use crate::model::{
    BridgeLightInfo, EntertainmentArea, FLOORPLAN_ARCHETYPES, FLOORPLAN_STAMP_KINDS,
    FLOORPLAN_TEMPLATES, FloorplanBox, FloorplanDocument, FloorplanStamp,
    LAMP_ICONS, archetype_glyph, archetype_label, extract_bridge_lights, j_bool, j_list, j_str,
    lamp_icon_glyph, lamp_icon_label, normalize_lamp_icon, parse_entertainment_areas,
    room_active_schedule_badge, skin_style, stamp_kind_label, synth_target_lights, template_label,
};

// ------------------------------------------------------------------------- Visual constants

const WARM_FALLBACK: Color = Color::from_rgb(1.0, 0.824, 0.478);
const WIRE_FOLLOW: Color = Color::from_rgb(0.220, 0.741, 0.973); // #38BDF8
const WIRE_MIRROR: Color = Color::from_rgb(0.133, 0.827, 0.933); // #22D3EE
const WIRE_EXCLUSIVE: Color = Color::from_rgb(0.957, 0.247, 0.369); // #F43F5E
const WIRE_COMPANION: Color = Color::from_rgb(0.961, 0.620, 0.043); // #F59E0B
const SYNTH_MAGENTA: Color = Color::from_rgb(0.851, 0.275, 0.937); // #D946EF

const LAMP_RADIUS_PX: f32 = 16.0;
const HANDLE_SIZE_PX: f32 = 8.0;
const MIN_BOX_DIM: f64 = 60.0;

// ------------------------------------------------------------------------- Modes, Wire Types, Selection

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CanvasMode {
    #[default]
    Select,
    Wire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WireKind {
    #[default]
    Follow,
    Mirror,
    Exclusive,
    Companion,
}

impl WireKind {
    pub const ALL: [WireKind; 4] = [
        WireKind::Follow,
        WireKind::Mirror,
        WireKind::Exclusive,
        WireKind::Companion,
    ];

    pub fn label(self) -> String {
        match self {
            WireKind::Follow => tr("Follow"),
            WireKind::Mirror => tr("Mirror"),
            WireKind::Exclusive => tr("Exclusive"),
            WireKind::Companion => tr("Companion"),
        }
    }

    pub fn color(self) -> Color {
        match self {
            WireKind::Follow => WIRE_FOLLOW,
            WireKind::Mirror => WIRE_MIRROR,
            WireKind::Exclusive => WIRE_EXCLUSIVE,
            WireKind::Companion => WIRE_COMPANION,
        }
    }

    pub fn to_rule_payload(self, trigger_id: &str, trigger_name: &str, target_id: &str, target_name: &str) -> Value {
        let (kind, mirror_off, only_if_off, action) = match self {
            WireKind::Follow => ("follow", false, false, "on"),
            WireKind::Mirror => ("follow", true, false, "on"),
            WireKind::Exclusive => ("exclusive", false, false, "off"),
            WireKind::Companion => ("companion", false, true, "on"),
        };
        let arrow = match self {
            WireKind::Mirror => "↔",
            WireKind::Exclusive => "⊘",
            _ => "→",
        };
        let name = format!("{trigger_name} {arrow} {target_name}");
        json!({
            "cmd": "rule_save",
            "rule": {
                "name": name,
                "enabled": true,
                "kind": kind,
                "trigger_kind": "light",
                "trigger_id": trigger_id,
                "trigger_state": "on",
                "targets": [{
                    "kind": "light",
                    "id": target_id,
                    "action": action,
                    "bri": Value::Null,
                    "scene_id": "",
                }],
                "mirror_off": mirror_off,
                "match_bri": false,
                "time_start": "",
                "time_end": "",
                "only_if_off": only_if_off,
            }
        })
    }
}

pub fn classify_rule_wire(rule: &Value) -> WireKind {
    match j_str(rule, "kind") {
        "exclusive" => WireKind::Exclusive,
        "companion" => WireKind::Companion,
        _ => {
            if j_bool(rule, "mirror_off") {
                WireKind::Mirror
            } else {
                WireKind::Follow
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Lamp(String),
    Box(String),
    Stamp(String),
    Wire(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeHandle {
    NW,
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
}

impl ResizeHandle {
    pub const ALL: [ResizeHandle; 8] = [
        ResizeHandle::NW,
        ResizeHandle::N,
        ResizeHandle::NE,
        ResizeHandle::E,
        ResizeHandle::SE,
        ResizeHandle::S,
        ResizeHandle::SW,
        ResizeHandle::W,
    ];

    pub fn world_point(self, b: &FloorplanBox) -> (f64, f64) {
        let cx = b.x + b.w * 0.5;
        let cy = b.y + b.h * 0.5;
        let rx = b.x + b.w;
        let by = b.y + b.h;
        match self {
            ResizeHandle::NW => (b.x, b.y),
            ResizeHandle::N => (cx, b.y),
            ResizeHandle::NE => (rx, b.y),
            ResizeHandle::E => (rx, cy),
            ResizeHandle::SE => (rx, by),
            ResizeHandle::S => (cx, by),
            ResizeHandle::SW => (b.x, by),
            ResizeHandle::W => (b.x, cy),
        }
    }

    pub fn cursor(self) -> mouse::Interaction {
        match self {
            ResizeHandle::N | ResizeHandle::S => mouse::Interaction::ResizingVertically,
            ResizeHandle::E | ResizeHandle::W => mouse::Interaction::ResizingHorizontally,
            ResizeHandle::NW | ResizeHandle::SE => mouse::Interaction::ResizingDiagonallyDown,
            ResizeHandle::NE | ResizeHandle::SW => mouse::Interaction::ResizingDiagonallyUp,
        }
    }
}

// ------------------------------------------------------------------------- Messages

#[derive(Debug, Clone)]
pub enum MapMsg {
    ToggleSidebar,
    SearchChanged(String),
    ToggleTemplatesMenu,
    ApplyTemplate(String),
    AddBox(String), // "room" | "zone"
    ToggleStampsMenu,
    AddStamp(String),
    SetMode(CanvasMode),
    SetWireKind(WireKind),
    ToggleGlows,
    ToggleFurniture,
    ToggleWires,
    ToggleAutoSync(bool),
    SyncAll,
    ToggleEntMenu,
    SetTopEntertainment(Option<String>),
    // Background popover
    ToggleBgMenu,
    ChooseBgFile,
    BgFilePicked(Option<String>),
    BgPathInput(String),
    BgApplyPath,
    BgOpacity(u8), // 5..=100
    BgOpacityDone,
    BgFit(String),
    BgClear,
    // Sidebar actions
    PlaceLamp(String),
    ArmPlaceLamp(String),
    FocusLamp(String),
    UnplaceLamp(String),
    // Canvas interactions
    Select(Option<Selection>),
    MoveLamp { id: String, x: f64, y: f64, commit: bool },
    MoveBox { id: String, x: f64, y: f64, commit: bool },
    ResizeBox { id: String, x: f64, y: f64, w: f64, h: f64, commit: bool },
    MoveStamp { id: String, x: f64, y: f64, commit: bool },
    PlaceArmedAt { x: f64, y: f64 },
    CreateWire { from_id: String, to_id: String },
    // Lamp inspector
    LampToggle(String, bool),
    LampBri(String, u8),
    LampBriDone(String),
    LampRenameInput(String),
    LampRenameSave(String),
    LampSetIcon(String, String),
    LampStartWire(String),
    // Room / Zone inspector
    BoxRenameInput(String),
    BoxRenameSave(String),
    BoxSetKind(String, String),
    ToggleArchetypeMenu,
    BoxSetArchetype(String, String),
    ToggleBoxEntMenu,
    BoxSetEntertainment(String, Option<String>),
    BoxSyncEntertainment(String, String),
    BoxSyncRoom,
    BoxRemoveFromCanvas(String),
    BoxDeleteOnBridge(String),
    RecallScene(String),
    NewSceneNameInput(String),
    SaveCurrentScene(String),
    ScheduleScene { kind: String, room_id: String, room_name: String, scene_id: String, scene_name: String },
    // Stamp inspector
    StampRotate(String),
    StampDelete(String),
    // Wire inspector
    WireToggle(String, bool),
    WireDelete(String),
}

// ------------------------------------------------------------------------- State

#[derive(Debug, Clone)]
pub struct FloorplanState {
    pub doc: FloorplanDocument,
    pub local_dirty: bool,
    pub sidebar_open: bool,
    pub search: String,
    pub mode: CanvasMode,
    pub wire_kind: WireKind,
    pub selection: Option<Selection>,
    pub armed_place_lamp: Option<String>,
    pub pending_wire_from: Option<String>,
    // Menus / popovers
    pub templates_open: bool,
    pub stamps_open: bool,
    pub bg_open: bool,
    pub ent_menu_open: bool,
    pub archetype_menu_open: bool,
    pub box_ent_menu_open: bool,
    // Inline text inputs
    pub lamp_rename_draft: String,
    pub box_rename_draft: String,
    pub new_scene_name: String,
    pub bg_path_draft: String,
    pub bg_opacity_drag: Option<u8>,
    // Monotonic local ID counter for new boxes/stamps
    pub next_id_seq: u32,
}

impl Default for FloorplanState {
    fn default() -> Self {
        Self {
            doc: FloorplanDocument::default(),
            local_dirty: false,
            sidebar_open: true,
            search: String::new(),
            mode: CanvasMode::Select,
            wire_kind: WireKind::Follow,
            selection: None,
            armed_place_lamp: None,
            pending_wire_from: None,
            templates_open: false,
            stamps_open: false,
            bg_open: false,
            ent_menu_open: false,
            archetype_menu_open: false,
            box_ent_menu_open: false,
            lamp_rename_draft: String::new(),
            box_rename_draft: String::new(),
            new_scene_name: String::new(),
            bg_path_draft: String::new(),
            bg_opacity_drag: None,
            next_id_seq: 1,
        }
    }
}

impl FloorplanState {
    pub fn any_menu_open(&self) -> bool {
        self.templates_open
            || self.stamps_open
            || self.bg_open
            || self.ent_menu_open
            || self.archetype_menu_open
            || self.box_ent_menu_open
    }

    pub fn close_menus(&mut self) {
        self.templates_open = false;
        self.stamps_open = false;
        self.bg_open = false;
        self.ent_menu_open = false;
        self.archetype_menu_open = false;
        self.box_ent_menu_open = false;
    }

    pub fn sync_from_snapshot(&mut self, incoming: FloorplanDocument) {
        if !self.local_dirty {
            self.doc = incoming;
            if self.bg_opacity_drag.is_none() && self.bg_path_draft.is_empty() {
                self.bg_path_draft = self.doc.background.path.clone();
            }
        }
    }

    pub fn select(&mut self, sel: Option<Selection>, stats: &Value) {
        self.selection = sel.clone();
        self.archetype_menu_open = false;
        self.box_ent_menu_open = false;
        match sel {
            Some(Selection::Lamp(ref lid)) => {
                let name = extract_bridge_lights(stats)
                    .into_iter()
                    .find(|l| l.id == *lid)
                    .map(|l| l.name)
                    .unwrap_or_default();
                self.lamp_rename_draft = name;
            }
            Some(Selection::Box(ref bid)) => {
                let name = self
                    .doc
                    .boxes
                    .iter()
                    .find(|b| b.id == *bid)
                    .map(|b| b.name.clone())
                    .unwrap_or_default();
                self.box_rename_draft = name;
                self.new_scene_name.clear();
            }
            _ => {}
        }
    }

    pub fn next_unique_id(&mut self, prefix: &str) -> String {
        loop {
            let candidate = format!("{prefix}-{}", self.next_id_seq);
            self.next_id_seq = self.next_id_seq.wrapping_add(1).max(1);
            let exists = self.doc.boxes.iter().any(|b| b.id == candidate)
                || self.doc.stamps.iter().any(|s| s.id == candidate);
            if !exists {
                return candidate;
            }
        }
    }
}

// ------------------------------------------------------------------------- Geometry, Containment & Diff Detection

pub fn point_in_box(x: f64, y: f64, b: &FloorplanBox) -> bool {
    x >= b.x && x <= b.x + b.w && y >= b.y && y <= b.y + b.h
}

pub fn snap_coord(val: f64, grid: i32, snap: bool) -> f64 {
    if !snap || grid <= 1 {
        return (val * 10.0).round() / 10.0;
    }
    let g = grid as f64;
    ((val / g).round() * g * 10.0).round() / 10.0
}

/// Finds the smallest-area `kind == "room"` box containing `(x, y)`.
/// On shared-wall ties, prefers the box matching `current_room_id` (mirrors `hue-lights.py`).
pub fn containing_room_box<'a>(
    x: f64,
    y: f64,
    boxes: &'a [FloorplanBox],
    current_room_id: &str,
) -> Option<&'a FloorplanBox> {
    let mut best: Option<&FloorplanBox> = None;
    for b in boxes {
        if b.kind != "room" || !point_in_box(x, y, b) {
            continue;
        }
        let area = b.w * b.h;
        let tie = if b.hue_id.as_deref() == Some(current_room_id) && !current_room_id.is_empty() {
            0u8
        } else {
            1u8
        };
        let replace = match best {
            None => true,
            Some(prev) => {
                let prev_area = prev.w * prev.h;
                let prev_tie = if prev.hue_id.as_deref() == Some(current_room_id) && !current_room_id.is_empty() {
                    0u8
                } else {
                    1u8
                };
                (area, tie) < (prev_area, prev_tie)
            }
        };
        if replace {
            best = Some(b);
        }
    }
    best
}

pub fn apply_resize(
    orig: &FloorplanBox,
    handle: ResizeHandle,
    wx: f64,
    wy: f64,
    grid: i32,
    snap: bool,
    max_w: f64,
    max_h: f64,
) -> (f64, f64, f64, f64) {
    let sx = snap_coord(wx, grid, snap).clamp(0.0, max_w);
    let sy = snap_coord(wy, grid, snap).clamp(0.0, max_h);
    let mut left = orig.x;
    let mut top = orig.y;
    let mut right = orig.x + orig.w;
    let mut bottom = orig.y + orig.h;

    match handle {
        ResizeHandle::NW => {
            left = sx.min(right - MIN_BOX_DIM);
            top = sy.min(bottom - MIN_BOX_DIM);
        }
        ResizeHandle::N => {
            top = sy.min(bottom - MIN_BOX_DIM);
        }
        ResizeHandle::NE => {
            right = sx.max(left + MIN_BOX_DIM);
            top = sy.min(bottom - MIN_BOX_DIM);
        }
        ResizeHandle::E => {
            right = sx.max(left + MIN_BOX_DIM);
        }
        ResizeHandle::SE => {
            right = sx.max(left + MIN_BOX_DIM);
            bottom = sy.max(top + MIN_BOX_DIM);
        }
        ResizeHandle::S => {
            bottom = sy.max(top + MIN_BOX_DIM);
        }
        ResizeHandle::SW => {
            left = sx.min(right - MIN_BOX_DIM);
            bottom = sy.max(top + MIN_BOX_DIM);
        }
        ResizeHandle::W => {
            left = sx.min(right - MIN_BOX_DIM);
        }
    }
    let w = (right - left).clamp(MIN_BOX_DIM, max_w);
    let h = (bottom - top).clamp(MIN_BOX_DIM, max_h);
    (left.clamp(0.0, max_w - MIN_BOX_DIM), top.clamp(0.0, max_h - MIN_BOX_DIM), w, h)
}

/// Counts pending Bridge synchronization changes (new rooms/zones, room/zone renames or archetype
/// updates, lamp-to-room moves, zone light membership diffs, and linked entertainment syncs).
pub fn compute_pending_sync_count(doc: &FloorplanDocument, stats: &Value) -> usize {
    let mut pending = 0usize;
    let rooms_val = j_list(stats, "rooms");

    let mut known_rooms: HashMap<&str, &Value> = HashMap::new();
    let mut known_zones: HashMap<&str, &Value> = HashMap::new();
    for r in rooms_val {
        let id = j_str(r, "id");
        if id.is_empty() {
            continue;
        }
        match j_str(r, "kind") {
            "room" => {
                known_rooms.insert(id, r);
            }
            "zone" => {
                known_zones.insert(id, r);
            }
            _ => {}
        }
    }

    // 1. Box creations & metadata diffs (rooms & zones)
    for b in &doc.boxes {
        match b.hue_id.as_deref() {
            None | Some("") => {
                pending += 1;
            }
            Some(hid) => {
                let existing = if b.kind == "zone" {
                    known_zones.get(hid)
                } else {
                    known_rooms.get(hid)
                };
                if let Some(ex) = existing {
                    let ename = j_str(ex, "name").trim();
                    let bname = b.name.trim();
                    let earch = j_str(ex, "archetype");
                    let name_diff = !bname.is_empty() && bname != ename;
                    let arch_diff = !b.archetype.is_empty()
                        && b.archetype != earch
                        && (!earch.is_empty() || b.archetype != "other");
                    if name_diff || arch_diff {
                        pending += 1;
                    }
                    // Zone membership diff
                    if b.kind == "zone" {
                        let current_lights: HashSet<&str> = j_list(ex, "lights")
                            .iter()
                            .map(|l| j_str(l, "id"))
                            .filter(|id| !id.is_empty())
                            .collect();
                        let placed_inside: HashSet<&str> = doc
                            .lamps
                            .iter()
                            .filter(|(_, pos)| point_in_box(pos.x, pos.y, b))
                            .map(|(lid, _)| lid.as_str())
                            .collect();
                        let mut desired: HashSet<&str> = current_lights
                            .iter()
                            .copied()
                            .filter(|lid| !doc.lamps.contains_key(*lid))
                            .collect();
                        desired.extend(placed_inside);
                        if desired != current_lights {
                            pending += 1;
                        }
                    }
                }
            }
        }
    }

    // 2. Room membership moves (lamp inside a room box linked to a different Hue room)
    let lights = extract_bridge_lights(stats);
    let mut moved_devices: HashSet<String> = HashSet::new();
    for light in &lights {
        let Some(pos) = doc.lamps.get(&light.id) else { continue };
        let Some(target_box) = containing_room_box(pos.x, pos.y, &doc.boxes, &light.room_id) else {
            continue;
        };
        let to_room_id = target_box.hue_id.as_deref().unwrap_or("");
        if to_room_id != light.room_id {
            let key = if light.device_id.is_empty() {
                light.id.clone()
            } else {
                light.device_id.clone()
            };
            if moved_devices.insert(key) {
                pending += 1;
            }
        }
    }

    // 3. Linked Entertainment area position drift
    let ent_areas = parse_entertainment_areas(stats);
    let ent_map: HashMap<&str, &EntertainmentArea> = ent_areas.iter().map(|a| (a.id.as_str(), a)).collect();
    let check_ent_drift = |area: &EntertainmentArea, ref_box: &FloorplanBox| -> bool {
        if ref_box.w <= 0.0 || ref_box.h <= 0.0 {
            return false;
        }
        for loc in &area.locations {
            let Some(pos) = doc.lamps.get(&loc.light_id) else { continue };
            if !point_in_box(pos.x, pos.y, ref_box) {
                continue;
            }
            let nx = (((pos.x - ref_box.x) / ref_box.w) * 2.0 - 1.0).clamp(-1.0, 1.0);
            let ny = (1.0 - ((pos.y - ref_box.y) / ref_box.h)).clamp(0.0, 1.0);
            if (nx - loc.x).abs() > 0.08 || (ny - loc.y).abs() > 0.08 {
                return true;
            }
        }
        false
    };
    let mut checked_ent: HashSet<&str> = HashSet::new();
    for b in &doc.boxes {
        if let Some(aid) = b.entertainment_area_id.as_deref()
            && let Some(area) = ent_map.get(aid)
            && checked_ent.insert(aid)
            && check_ent_drift(area, b)
        {
            pending += 1;
        }
    }
    if let Some(aid) = doc.entertainment_id.as_deref()
        && let Some(area) = ent_map.get(aid)
        && checked_ent.insert(aid)
    {
        let fallback_box = FloorplanBox {
            id: String::new(),
            kind: "room".into(),
            hue_id: None,
            name: String::new(),
            archetype: "other".into(),
            x: 0.0,
            y: 0.0,
            w: doc.canvas.width.max(400.0),
            h: doc.canvas.height.max(300.0),
            entertainment_area_id: None,
        };
        let ref_box = if doc.boxes.len() == 1 { &doc.boxes[0] } else { &fallback_box };
        if check_ent_drift(area, ref_box) {
            pending += 1;
        }
    }

    pending
}

// ------------------------------------------------------------------------- Coordinate Transform & Bezier Helpers

#[derive(Debug, Clone, Copy)]
pub struct CanvasTransform {
    pub scale: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}

impl CanvasTransform {
    pub fn fit(bounds: Size, world_w: f64, world_h: f64) -> Self {
        let ww = world_w.max(400.0) as f32;
        let wh = world_h.max(300.0) as f32;
        let pad = 12.0f32;
        let avail_w = (bounds.width - pad * 2.0).max(40.0);
        let avail_h = (bounds.height - pad * 2.0).max(40.0);
        let scale = (avail_w / ww).min(avail_h / wh).clamp(0.12, 3.0);
        let offset_x = (bounds.width - ww * scale) * 0.5;
        let offset_y = (bounds.height - wh * scale) * 0.5;
        Self { scale, offset_x, offset_y }
    }

    pub fn to_screen(self, wx: f64, wy: f64) -> Point {
        Point::new(self.offset_x + (wx as f32) * self.scale, self.offset_y + (wy as f32) * self.scale)
    }

    pub fn to_world(self, p: Point) -> (f64, f64) {
        (
            ((p.x - self.offset_x) / self.scale) as f64,
            ((p.y - self.offset_y) / self.scale) as f64,
        )
    }
}

fn bezier_controls(a: Point, b: Point) -> (Point, Point) {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let dist = (dx * dx + dy * dy).sqrt().max(1.0);
    let lift = (dist * 0.22).clamp(20.0, 90.0);
    let nx = -dy / dist;
    let ny = dx / dist;
    let c1 = Point::new(a.x + dx * 0.33 + nx * lift, a.y + dy * 0.33 + ny * lift);
    let c2 = Point::new(a.x + dx * 0.67 + nx * lift, a.y + dy * 0.67 + ny * lift);
    (c1, c2)
}

fn eval_cubic(p0: Point, p1: Point, p2: Point, p3: Point, t: f32) -> Point {
    let u = 1.0 - t;
    let tt = t * t;
    let uu = u * u;
    let uuu = uu * u;
    let ttt = tt * t;
    Point::new(
        uuu * p0.x + 3.0 * uu * t * p1.x + 3.0 * u * tt * p2.x + ttt * p3.x,
        uuu * p0.y + 3.0 * uu * t * p1.y + 3.0 * u * tt * p2.y + ttt * p3.y,
    )
}

fn eval_cubic_tangent(p0: Point, p1: Point, p2: Point, p3: Point, t: f32) -> Vector {
    let u = 1.0 - t;
    let dx = 3.0 * u * u * (p1.x - p0.x)
        + 6.0 * u * t * (p2.x - p1.x)
        + 3.0 * t * t * (p3.x - p2.x);
    let dy = 3.0 * u * u * (p1.y - p0.y)
        + 6.0 * u * t * (p2.y - p1.y)
        + 3.0 * t * t * (p3.y - p2.y);
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    Vector::new(dx / len, dy / len)
}

fn dist_to_cubic(pt: Point, p0: Point, p1: Point, p2: Point, p3: Point) -> f32 {
    let mut best = f32::MAX;
    for i in 0..=24 {
        let t = i as f32 / 24.0;
        let q = eval_cubic(p0, p1, p2, p3, t);
        let d = ((pt.x - q.x).powi(2) + (pt.y - q.y).powi(2)).sqrt();
        if d < best {
            best = d;
        }
    }
    best
}

fn parse_hex_color(hex: &str) -> Color {
    let ok = hex.len() == 7 && hex.starts_with('#') && hex[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !ok {
        return WARM_FALLBACK;
    }
    let p = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
    Color::from_rgb8(p(1), p(3), p(5))
}

fn with_alpha(mut c: Color, a: f32) -> Color {
    c.a = a;
    c
}

fn contrast_ink(c: Color) -> Color {
    if 0.299 * c.r + 0.587 * c.g + 0.114 * c.b > 0.58 {
        Color::from_rgb8(0x14, 0x16, 0x1c)
    } else {
        Color::WHITE
    }
}

// ------------------------------------------------------------------------- Canvas Program

#[derive(Debug, Clone)]
struct WireRenderItem {
    rule_id: String,
    kind: WireKind,
    enabled: bool,
    from: (f64, f64),
    to: (f64, f64),
}

#[derive(Debug, Clone)]
enum DragOp {
    Lamp { id: String, grab_dx: f64, grab_dy: f64 },
    BoxMove { id: String, grab_dx: f64, grab_dy: f64 },
    BoxResize { id: String, handle: ResizeHandle, orig: FloorplanBox },
    Stamp { id: String, grab_dx: f64, grab_dy: f64 },
    Wire { from_id: String, cursor: Point },
}

#[derive(Default)]
pub struct CanvasInteraction {
    drag: Option<DragOp>,
}

struct FloorplanCanvasProgram<'a> {
    doc: &'a FloorplanDocument,
    lights: HashMap<String, BridgeLightInfo>,
    wires: Vec<WireRenderItem>,
    schedule_badges: HashMap<String, String>,
    synth_running: bool,
    synth_lights: HashSet<String>,
    mode: CanvasMode,
    wire_kind: WireKind,
    selection: Option<Selection>,
    armed_place_lamp: Option<String>,
    pending_wire_from: Option<String>,
    has_bg_image: bool,
}

impl<'a> FloorplanCanvasProgram<'a> {
    fn from_state(state: &'a FloorplanState, stats: &'a Value) -> Self {
        let lights_vec = extract_bridge_lights(stats);
        let lights: HashMap<String, BridgeLightInfo> =
            lights_vec.into_iter().map(|l| (l.id.clone(), l)).collect();

        // Schedule badges per box.id
        let mut schedule_badges = HashMap::new();
        for b in &state.doc.boxes {
            if let Some(hid) = b.hue_id.as_deref()
                && let Some(badge) = room_active_schedule_badge(stats, hid)
            {
                schedule_badges.insert(b.id.clone(), badge);
            }
        }

        // Resolve endpoint world coords for rules (placed lamp or box center)
        let endpoint_pos = |kind: &str, id: &str| -> Option<(f64, f64)> {
            if kind == "light" {
                state.doc.lamps.get(id).map(|l| (l.x, l.y))
            } else if matches!(kind, "room" | "zone") {
                state
                    .doc
                    .boxes
                    .iter()
                    .find(|b| b.hue_id.as_deref() == Some(id))
                    .map(|b| (b.x + b.w * 0.5, b.y + b.h * 0.5))
            } else {
                None
            }
        };

        let mut wires = Vec::new();
        for rule in j_list(stats, "rules") {
            let rid = j_str(rule, "id");
            if rid.is_empty() {
                continue;
            }
            let Some(from) = endpoint_pos(j_str(rule, "trigger_kind"), j_str(rule, "trigger_id")) else {
                continue;
            };
            let wkind = classify_rule_wire(rule);
            let enabled = j_bool(rule, "enabled");
            for t in j_list(rule, "targets") {
                if let Some(to) = endpoint_pos(j_str(t, "kind"), j_str(t, "id")) {
                    wires.push(WireRenderItem {
                        rule_id: rid.to_string(),
                        kind: wkind,
                        enabled,
                        from,
                        to,
                    });
                }
            }
        }

        let synth = stats.get("synth");
        let synth_running = synth.is_some_and(|sy| j_bool(sy, "running"));
        let synth_room = synth.map(|sy| j_str(sy, "room_id")).unwrap_or("");
        let synth_lights: HashSet<String> = if synth_running {
            synth_target_lights(stats, synth_room)
                .into_iter()
                .map(|(id, _)| id)
                .collect()
        } else {
            HashSet::new()
        };

        let has_bg_image =
            !state.doc.background.path.is_empty() && Path::new(&state.doc.background.path).is_file();

        Self {
            doc: &state.doc,
            lights,
            wires,
            schedule_badges,
            synth_running,
            synth_lights,
            mode: state.mode,
            wire_kind: state.wire_kind,
            selection: state.selection.clone(),
            armed_place_lamp: state.armed_place_lamp.clone(),
            pending_wire_from: state.pending_wire_from.clone(),
            has_bg_image,
        }
    }

    fn hit_lamp(&self, pt: Point, tf: CanvasTransform) -> Option<String> {
        let hit_r = LAMP_RADIUS_PX + 6.0;
        let mut best: Option<(f32, String)> = None;
        for (lid, pos) in &self.doc.lamps {
            let sp = tf.to_screen(pos.x, pos.y);
            let d = ((pt.x - sp.x).powi(2) + (pt.y - sp.y).powi(2)).sqrt();
            if d <= hit_r && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, lid.clone()));
            }
        }
        best.map(|(_, id)| id)
    }

    fn hit_selected_box_handle(&self, pt: Point, tf: CanvasTransform) -> Option<(FloorplanBox, ResizeHandle)> {
        let Some(Selection::Box(ref bid)) = self.selection else { return None };
        let b = self.doc.boxes.iter().find(|b| b.id == *bid)?;
        for h in ResizeHandle::ALL {
            let (wx, wy) = h.world_point(b);
            let sp = tf.to_screen(wx, wy);
            if (pt.x - sp.x).abs() <= HANDLE_SIZE_PX + 3.0 && (pt.y - sp.y).abs() <= HANDLE_SIZE_PX + 3.0 {
                return Some((b.clone(), h));
            }
        }
        None
    }

    fn hit_stamp(&self, pt: Point, tf: CanvasTransform) -> Option<String> {
        if !self.doc.show_furniture {
            return None;
        }
        let (wx, wy) = tf.to_world(pt);
        for st in self.doc.stamps.iter().rev() {
            if wx >= st.x - 6.0 && wx <= st.x + st.w + 6.0 && wy >= st.y - 6.0 && wy <= st.y + st.h + 6.0 {
                return Some(st.id.clone());
            }
        }
        None
    }

    fn hit_wire(&self, pt: Point, tf: CanvasTransform) -> Option<String> {
        if !self.doc.show_wires {
            return None;
        }
        let mut best: Option<(f32, String)> = None;
        for w in &self.wires {
            let a = tf.to_screen(w.from.0, w.from.1);
            let b = tf.to_screen(w.to.0, w.to.1);
            let (c1, c2) = bezier_controls(a, b);
            let d = dist_to_cubic(pt, a, c1, c2, b);
            if d <= 10.0 && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, w.rule_id.clone()));
            }
        }
        best.map(|(_, id)| id)
    }

    fn hit_box(&self, pt: Point, tf: CanvasTransform) -> Option<String> {
        let (wx, wy) = tf.to_world(pt);
        let mut best: Option<(f64, String)> = None;
        for b in &self.doc.boxes {
            if point_in_box(wx, wy, b) {
                let area = b.w * b.h;
                if best.as_ref().is_none_or(|(ba, _)| area < *ba) {
                    best = Some((area, b.id.clone()));
                }
            }
        }
        best.map(|(_, id)| id)
    }
}

impl<'a> Program<MapMsg, cosmic::Theme, cosmic::Renderer> for FloorplanCanvasProgram<'a> {
    type State = CanvasInteraction;

    fn update(
        &self,
        state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<MapMsg>> {
        let tf = CanvasTransform::fit(bounds.size(), self.doc.canvas.width, self.doc.canvas.height);

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let pos = cursor.position_in(bounds)?;

                // Click-to-place armed lamp from sidebar
                if self.armed_place_lamp.is_some() {
                    let (wx, wy) = tf.to_world(pos);
                    let x = snap_coord(wx, self.doc.canvas.grid, self.doc.canvas.snap)
                        .clamp(16.0, self.doc.canvas.width - 16.0);
                    let y = snap_coord(wy, self.doc.canvas.grid, self.doc.canvas.snap)
                        .clamp(16.0, self.doc.canvas.height - 16.0);
                    return Some(Action::publish(MapMsg::PlaceArmedAt { x, y }).and_capture());
                }

                // Wire mode: drag or 2-click between lamps
                if self.mode == CanvasMode::Wire {
                    if let Some(lid) = self.hit_lamp(pos, tf) {
                        if let Some(ref from_id) = self.pending_wire_from
                            && from_id != &lid
                        {
                            return Some(
                                Action::publish(MapMsg::CreateWire {
                                    from_id: from_id.clone(),
                                    to_id: lid,
                                })
                                .and_capture(),
                            );
                        }
                        state.drag = Some(DragOp::Wire { from_id: lid, cursor: pos });
                        return Some(Action::request_redraw().and_capture());
                    }
                    if let Some(rid) = self.hit_wire(pos, tf) {
                        return Some(Action::publish(MapMsg::Select(Some(Selection::Wire(rid)))).and_capture());
                    }
                    return Some(Action::publish(MapMsg::Select(None)).and_capture());
                }

                // Select/Move mode: priority = resize handle > lamp > wire > stamp > box
                if let Some((orig, handle)) = self.hit_selected_box_handle(pos, tf) {
                    state.drag = Some(DragOp::BoxResize { id: orig.id.clone(), handle, orig });
                    return Some(Action::capture());
                }
                if let Some(lid) = self.hit_lamp(pos, tf) {
                    if let Some(lpos) = self.doc.lamps.get(&lid) {
                        let (wx, wy) = tf.to_world(pos);
                        state.drag = Some(DragOp::Lamp {
                            id: lid.clone(),
                            grab_dx: lpos.x - wx,
                            grab_dy: lpos.y - wy,
                        });
                    }
                    return Some(Action::publish(MapMsg::Select(Some(Selection::Lamp(lid)))).and_capture());
                }
                if let Some(rid) = self.hit_wire(pos, tf) {
                    return Some(Action::publish(MapMsg::Select(Some(Selection::Wire(rid)))).and_capture());
                }
                if let Some(sid) = self.hit_stamp(pos, tf) {
                    if let Some(st) = self.doc.stamps.iter().find(|s| s.id == sid) {
                        let (wx, wy) = tf.to_world(pos);
                        state.drag = Some(DragOp::Stamp {
                            id: sid.clone(),
                            grab_dx: st.x - wx,
                            grab_dy: st.y - wy,
                        });
                    }
                    return Some(Action::publish(MapMsg::Select(Some(Selection::Stamp(sid)))).and_capture());
                }
                if let Some(bid) = self.hit_box(pos, tf) {
                    if let Some(b) = self.doc.boxes.iter().find(|b| b.id == bid) {
                        let (wx, wy) = tf.to_world(pos);
                        state.drag = Some(DragOp::BoxMove {
                            id: bid.clone(),
                            grab_dx: b.x - wx,
                            grab_dy: b.y - wy,
                        });
                    }
                    return Some(Action::publish(MapMsg::Select(Some(Selection::Box(bid)))).and_capture());
                }
                Some(Action::publish(MapMsg::Select(None)).and_capture())
            }

            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let drag = state.drag.as_mut()?;
                let pos = cursor.position_from(bounds.position())?;
                let (wx, wy) = tf.to_world(pos);
                let grid = self.doc.canvas.grid;
                let snap = self.doc.canvas.snap;
                let max_w = self.doc.canvas.width;
                let max_h = self.doc.canvas.height;
                match drag {
                    DragOp::Lamp { id, grab_dx, grab_dy } => {
                        let x = snap_coord(wx + *grab_dx, grid, snap).clamp(12.0, max_w - 12.0);
                        let y = snap_coord(wy + *grab_dy, grid, snap).clamp(12.0, max_h - 12.0);
                        Some(Action::publish(MapMsg::MoveLamp { id: id.clone(), x, y, commit: false }).and_capture())
                    }
                    DragOp::BoxMove { id, grab_dx, grab_dy } => {
                        let (bw, bh) = self
                            .doc
                            .boxes
                            .iter()
                            .find(|b| b.id == *id)
                            .map(|b| (b.w, b.h))
                            .unwrap_or((120.0, 120.0));
                        let x = snap_coord(wx + *grab_dx, grid, snap).clamp(0.0, (max_w - bw).max(0.0));
                        let y = snap_coord(wy + *grab_dy, grid, snap).clamp(0.0, (max_h - bh).max(0.0));
                        Some(Action::publish(MapMsg::MoveBox { id: id.clone(), x, y, commit: false }).and_capture())
                    }
                    DragOp::BoxResize { id, handle, orig } => {
                        let (x, y, w, h) = apply_resize(orig, *handle, wx, wy, grid, snap, max_w, max_h);
                        Some(
                            Action::publish(MapMsg::ResizeBox {
                                id: id.clone(),
                                x,
                                y,
                                w,
                                h,
                                commit: false,
                            })
                            .and_capture(),
                        )
                    }
                    DragOp::Stamp { id, grab_dx, grab_dy } => {
                        let x = snap_coord(wx + *grab_dx, grid, snap).clamp(0.0, max_w - 16.0);
                        let y = snap_coord(wy + *grab_dy, grid, snap).clamp(0.0, max_h - 16.0);
                        Some(Action::publish(MapMsg::MoveStamp { id: id.clone(), x, y, commit: false }).and_capture())
                    }
                    DragOp::Wire { cursor: c, .. } => {
                        *c = pos;
                        Some(Action::request_redraw().and_capture())
                    }
                }
            }

            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let drag = state.drag.take()?;
                let pos = cursor.position_from(bounds.position()).unwrap_or(Point::ORIGIN);
                let (wx, wy) = tf.to_world(pos);
                let grid = self.doc.canvas.grid;
                let snap = self.doc.canvas.snap;
                let max_w = self.doc.canvas.width;
                let max_h = self.doc.canvas.height;
                match drag {
                    DragOp::Lamp { id, grab_dx, grab_dy } => {
                        let x = snap_coord(wx + grab_dx, grid, snap).clamp(12.0, max_w - 12.0);
                        let y = snap_coord(wy + grab_dy, grid, snap).clamp(12.0, max_h - 12.0);
                        Some(Action::publish(MapMsg::MoveLamp { id, x, y, commit: true }).and_capture())
                    }
                    DragOp::BoxMove { id, grab_dx, grab_dy } => {
                        let (bw, bh) = self
                            .doc
                            .boxes
                            .iter()
                            .find(|b| b.id == id)
                            .map(|b| (b.w, b.h))
                            .unwrap_or((120.0, 120.0));
                        let x = snap_coord(wx + grab_dx, grid, snap).clamp(0.0, (max_w - bw).max(0.0));
                        let y = snap_coord(wy + grab_dy, grid, snap).clamp(0.0, (max_h - bh).max(0.0));
                        Some(Action::publish(MapMsg::MoveBox { id, x, y, commit: true }).and_capture())
                    }
                    DragOp::BoxResize { id, handle, orig } => {
                        let (x, y, w, h) = apply_resize(&orig, handle, wx, wy, grid, snap, max_w, max_h);
                        Some(Action::publish(MapMsg::ResizeBox { id, x, y, w, h, commit: true }).and_capture())
                    }
                    DragOp::Stamp { id, grab_dx, grab_dy } => {
                        let x = snap_coord(wx + grab_dx, grid, snap).clamp(0.0, max_w - 16.0);
                        let y = snap_coord(wy + grab_dy, grid, snap).clamp(0.0, max_h - 16.0);
                        Some(Action::publish(MapMsg::MoveStamp { id, x, y, commit: true }).and_capture())
                    }
                    DragOp::Wire { from_id, .. } => {
                        if let Some(to_id) = self.hit_lamp(pos, tf)
                            && to_id != from_id
                        {
                            return Some(
                                Action::publish(MapMsg::CreateWire { from_id, to_id }).and_capture(),
                            );
                        }
                        // Clicked single lamp in wire mode -> select it as pending source
                        Some(Action::publish(MapMsg::LampStartWire(from_id)).and_capture())
                    }
                }
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &cosmic::Renderer,
        theme: &cosmic::Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry<cosmic::Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let tf = CanvasTransform::fit(bounds.size(), self.doc.canvas.width, self.doc.canvas.height);
        let on_bg = Color::from(theme.cosmic().on_bg_color());
        let accent: Color = theme.cosmic().accent_color().into();

        // 1. Subtle architectural canvas surface & dot grid
        if !self.has_bg_image {
            let world_tl = tf.to_screen(0.0, 0.0);
            let world_sz = Size::new(
                self.doc.canvas.width as f32 * tf.scale,
                self.doc.canvas.height as f32 * tf.scale,
            );
            frame.fill_rectangle(world_tl, world_sz, with_alpha(on_bg, 0.03));

            let step = (self.doc.canvas.grid.max(20) as f64) * 2.0;
            let mut gx = step;
            while gx < self.doc.canvas.width {
                let mut gy = step;
                while gy < self.doc.canvas.height {
                    let p = tf.to_screen(gx, gy);
                    frame.fill_rectangle(Point::new(p.x - 0.8, p.y - 0.8), Size::new(1.6, 1.6), with_alpha(on_bg, 0.14));
                    gy += step;
                }
                gx += step;
            }
        }

        // 2. Room & Zone boxes
        for b in &self.doc.boxes {
            let tl = tf.to_screen(b.x, b.y);
            let sz = Size::new((b.w as f32) * tf.scale, (b.h as f32) * tf.scale);
            let is_zone = b.kind == "zone";
            let selected = matches!(&self.selection, Some(Selection::Box(id)) if id == &b.id);

            // Ambient room tint from active lights inside box
            let mut r_sum = 0.0f32;
            let mut g_sum = 0.0f32;
            let mut b_sum = 0.0f32;
            let mut lit_count = 0u32;
            for (lid, lpos) in &self.doc.lamps {
                if point_in_box(lpos.x, lpos.y, b)
                    && let Some(info) = self.lights.get(lid)
                    && info.on
                {
                    let c = parse_hex_color(&info.color);
                    r_sum += c.r;
                    g_sum += c.g;
                    b_sum += c.b;
                    lit_count += 1;
                }
            }
            let fill_color = if lit_count > 0 {
                let c = Color::from_rgb(
                    r_sum / lit_count as f32,
                    g_sum / lit_count as f32,
                    b_sum / lit_count as f32,
                );
                with_alpha(c, if is_zone { 0.08 } else { 0.13 })
            } else if is_zone {
                with_alpha(accent, 0.05)
            } else {
                with_alpha(on_bg, 0.05)
            };

            let rect_path = CanvasPath::rounded_rectangle(tl, sz, 6.0.into());
            frame.fill(&rect_path, fill_color);

            let border_color = if selected {
                accent
            } else if is_zone {
                with_alpha(accent, 0.55)
            } else {
                with_alpha(on_bg, 0.34)
            };
            let stroke = if is_zone {
                Stroke {
                    style: Style::Solid(border_color),
                    width: if selected { 2.2 } else { 1.4 },
                    line_dash: LineDash { segments: &[6.0, 4.0], offset: 0 },
                    ..Default::default()
                }
            } else {
                Stroke::default()
                    .with_color(border_color)
                    .with_width(if selected { 2.4 } else { 1.5 })
            };
            frame.stroke(&rect_path, stroke);

            // Header label (archetype glyph + name + indicators + active schedule badge)
            let mut label = format!("{} {}", archetype_glyph(&b.archetype), b.name);
            if b.hue_id.is_none() {
                label.push_str(" •");
            }
            if b.entertainment_area_id.is_some() {
                label.push_str(" ♪");
            }
            if let Some(badge) = self.schedule_badges.get(&b.id) {
                label.push_str(&format!("  {badge}"));
            }
            frame.fill_text(CanvasText {
                content: label,
                position: Point::new(tl.x + 8.0, tl.y + 6.0),
                color: with_alpha(on_bg, if selected { 0.98 } else { 0.80 }),
                size: Pixels(11.5),
                ..Default::default()
            });

            // 8 resize handles on selected box
            if selected {
                for h in ResizeHandle::ALL {
                    let (hx, hy) = h.world_point(b);
                    let hp = tf.to_screen(hx, hy);
                    let hrect = CanvasPath::rectangle(
                        Point::new(hp.x - HANDLE_SIZE_PX * 0.5, hp.y - HANDLE_SIZE_PX * 0.5),
                        Size::new(HANDLE_SIZE_PX, HANDLE_SIZE_PX),
                    );
                    frame.fill(&hrect, Color::WHITE);
                    frame.stroke(&hrect, Stroke::default().with_color(accent).with_width(1.6));
                }
            }
        }

        // 3. Minimalist architectural furniture stamps
        if self.doc.show_furniture {
            for st in &self.doc.stamps {
                let selected = matches!(&self.selection, Some(Selection::Stamp(id)) if id == &st.id);
                draw_stamp(&mut frame, st, tf, on_bg, accent, selected);
            }
        }

        // 4. Live radial color/brightness halos around placed lamps
        if self.doc.show_glows {
            for (lid, lpos) in &self.doc.lamps {
                let Some(info) = self.lights.get(lid) else { continue };
                if !info.on {
                    continue;
                }
                let center = tf.to_screen(lpos.x, lpos.y);
                let c = parse_hex_color(&info.color);
                let bri_norm = (info.bri / 100.0).clamp(0.12, 1.0) as f32;
                let max_r = (LAMP_RADIUS_PX + 18.0 + 42.0 * bri_norm) * (tf.scale.clamp(0.55, 1.4));
                for step_i in (1..=6).rev() {
                    let frac = step_i as f32 / 6.0;
                    let r = max_r * frac;
                    let alpha = (1.0 - frac * 0.78) * 0.14 * bri_norm;
                    frame.fill(&CanvasPath::circle(center, r), with_alpha(c, alpha));
                }
            }
        }

        // 5. Typed Bezier dependency wires
        if self.doc.show_wires {
            for w in &self.wires {
                let a = tf.to_screen(w.from.0, w.from.1);
                let b = tf.to_screen(w.to.0, w.to.1);
                let selected = matches!(&self.selection, Some(Selection::Wire(id)) if id == &w.rule_id);
                draw_wire(&mut frame, a, b, w.kind, w.enabled, selected);
            }
        }

        // Live preview wire while dragging in Wire mode
        if let Some(DragOp::Wire { from_id, cursor: cur }) = &state.drag
            && let Some(lpos) = self.doc.lamps.get(from_id)
        {
            let a = tf.to_screen(lpos.x, lpos.y);
            draw_wire(&mut frame, a, *cur, self.wire_kind, true, true);
        } else if self.mode == CanvasMode::Wire
            && let Some(ref from_id) = self.pending_wire_from
            && let Some(lpos) = self.doc.lamps.get(from_id)
            && let Some(cur) = cursor.position_in(bounds)
        {
            let a = tf.to_screen(lpos.x, lpos.y);
            draw_wire(&mut frame, a, cur, self.wire_kind, true, false);
        }

        // 6. Placed lamp nodes with selectable vector icons
        for (lid, lpos) in &self.doc.lamps {
            let center = tf.to_screen(lpos.x, lpos.y);
            let info = self.lights.get(lid);
            let is_on = info.is_some_and(|i| i.on);
            let reachable = info.is_none_or(|i| i.reachable);
            let selected = matches!(&self.selection, Some(Selection::Lamp(id)) if id == lid);
            let is_wire_src = self.pending_wire_from.as_deref() == Some(lid.as_str());
            let is_synth = self.synth_running && self.synth_lights.contains(lid);

            // Synth neon magenta active overlay ring
            if is_synth {
                frame.stroke(
                    &CanvasPath::circle(center, LAMP_RADIUS_PX + 6.5),
                    Stroke::default().with_color(with_alpha(SYNTH_MAGENTA, 0.9)).with_width(2.4),
                );
                frame.stroke(
                    &CanvasPath::circle(center, LAMP_RADIUS_PX + 10.5),
                    Stroke::default().with_color(with_alpha(SYNTH_MAGENTA, 0.35)).with_width(2.0),
                );
            }

            if selected || is_wire_src {
                let ring_col = if is_wire_src { self.wire_kind.color() } else { accent };
                frame.stroke(
                    &CanvasPath::circle(center, LAMP_RADIUS_PX + 4.0),
                    Stroke::default().with_color(ring_col).with_width(2.2),
                );
            }

            let fill = if is_on {
                let mut c = info.map(|i| parse_hex_color(&i.color)).unwrap_or(WARM_FALLBACK);
                if !reachable {
                    c.a = 0.45;
                }
                c
            } else {
                with_alpha(on_bg, 0.16)
            };
            let node_path = CanvasPath::circle(center, LAMP_RADIUS_PX);
            frame.fill(&node_path, fill);
            frame.stroke(
                &node_path,
                Stroke::default()
                    .with_color(if is_on { Color::WHITE } else { with_alpha(on_bg, 0.50) })
                    .with_width(if is_on { 1.8 } else { 1.3 }),
            );

            // Crisp vector lamp icon inside the node
            let ink = if is_on { contrast_ink(fill) } else { with_alpha(on_bg, 0.85) };
            draw_lamp_icon(&mut frame, center, LAMP_RADIUS_PX * 0.56, &lpos.icon, ink);

            // Compact name + brightness caption below node
            if let Some(info) = info {
                let caption = if info.on {
                    format!("{} · {}%", info.name, info.bri.round() as i32)
                } else {
                    info.name.clone()
                };
                frame.fill_text(CanvasText {
                    content: caption,
                    position: Point::new(center.x, center.y + LAMP_RADIUS_PX + 4.0),
                    color: with_alpha(on_bg, if is_on { 0.92 } else { 0.62 }),
                    size: Pixels(10.5),
                    align_x: alignment::Horizontal::Center.into(),
                    align_y: alignment::Vertical::Top,
                    ..Default::default()
                });
            }
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if let Some(drag) = &state.drag {
            return match drag {
                DragOp::BoxResize { handle, .. } => handle.cursor(),
                DragOp::Wire { .. } => mouse::Interaction::Crosshair,
                _ => mouse::Interaction::Grabbing,
            };
        }
        let Some(pos) = cursor.position_in(bounds) else {
            return mouse::Interaction::default();
        };
        if self.armed_place_lamp.is_some() || self.mode == CanvasMode::Wire {
            return mouse::Interaction::Crosshair;
        }
        let tf = CanvasTransform::fit(bounds.size(), self.doc.canvas.width, self.doc.canvas.height);
        if let Some((_, handle)) = self.hit_selected_box_handle(pos, tf) {
            return handle.cursor();
        }
        if self.hit_lamp(pos, tf).is_some() || self.hit_stamp(pos, tf).is_some() {
            return mouse::Interaction::Grab;
        }
        if self.hit_wire(pos, tf).is_some() {
            return mouse::Interaction::Pointer;
        }
        if self.hit_box(pos, tf).is_some() {
            return mouse::Interaction::Grab;
        }
        mouse::Interaction::default()
    }
}

// ------------------------------------------------------------------------- Vector drawing helpers

fn draw_wire(frame: &mut Frame, a: Point, b: Point, kind: WireKind, enabled: bool, selected: bool) {
    let (c1, c2) = bezier_controls(a, b);
    let col = with_alpha(kind.color(), if enabled { 0.92 } else { 0.35 });
    let width = if selected { 3.6 } else { 2.4 };

    let curve = CanvasPath::new(|builder| {
        builder.move_to(a);
        builder.bezier_curve_to(c1, c2, b);
    });

    if selected {
        frame.stroke(&curve, Stroke::default().with_color(with_alpha(kind.color(), 0.28)).with_width(8.0));
    }

    let stroke = match kind {
        WireKind::Follow | WireKind::Mirror => Stroke {
            style: Style::Solid(col),
            width,
            line_cap: LineCap::Round,
            ..Default::default()
        },
        WireKind::Exclusive => Stroke {
            style: Style::Solid(col),
            width,
            line_cap: LineCap::Butt,
            line_dash: LineDash { segments: &[8.0, 5.0], offset: 0 },
            ..Default::default()
        },
        WireKind::Companion => Stroke {
            style: Style::Solid(col),
            width,
            line_cap: LineCap::Round,
            line_dash: LineDash { segments: &[2.5, 5.5], offset: 0 },
            ..Default::default()
        },
    };
    frame.stroke(&curve, stroke);

    // Endcap geometry pulled back to lamp perimeter (t ~ 0.88)
    let tip = eval_cubic(a, c1, c2, b, 0.87);
    let tan = eval_cubic_tangent(a, c1, c2, b, 0.87);
    let perp = Vector::new(-tan.y, tan.x);

    match kind {
        WireKind::Follow => {
            draw_arrowhead(frame, tip, tan, col, 8.0);
        }
        WireKind::Mirror => {
            draw_arrowhead(frame, tip, tan, col, 8.0);
            let start_tip = eval_cubic(a, c1, c2, b, 0.13);
            let start_tan = eval_cubic_tangent(a, c1, c2, b, 0.13);
            draw_arrowhead(frame, start_tip, Vector::new(-start_tan.x, -start_tan.y), col, 8.0);
        }
        WireKind::Exclusive => {
            // Perpendicular T-bar stop
            let tbar = CanvasPath::new(|builder| {
                builder.move_to(Point::new(tip.x + perp.x * 7.5, tip.y + perp.y * 7.5));
                builder.line_to(Point::new(tip.x - perp.x * 7.5, tip.y - perp.y * 7.5));
            });
            frame.stroke(&tbar, Stroke::default().with_color(col).with_width(2.6).with_line_cap(LineCap::Round));
        }
        WireKind::Companion => {
            // Hollow diamond marker
            let d = 5.5;
            let diamond = CanvasPath::new(|builder| {
                builder.move_to(Point::new(tip.x + tan.x * d, tip.y + tan.y * d));
                builder.line_to(Point::new(tip.x + perp.x * d, tip.y + perp.y * d));
                builder.line_to(Point::new(tip.x - tan.x * d, tip.y - tan.y * d));
                builder.line_to(Point::new(tip.x - perp.x * d, tip.y - perp.y * d));
                builder.close();
            });
            frame.stroke(&diamond, Stroke::default().with_color(col).with_width(2.0));
        }
    }
}

fn draw_arrowhead(frame: &mut Frame, tip: Point, dir: Vector, color: Color, size: f32) {
    let perp = Vector::new(-dir.y, dir.x);
    let back = Point::new(tip.x - dir.x * size, tip.y - dir.y * size);
    let p1 = Point::new(back.x + perp.x * (size * 0.58), back.y + perp.y * (size * 0.58));
    let p2 = Point::new(back.x - perp.x * (size * 0.58), back.y - perp.y * (size * 0.58));
    let tri = CanvasPath::new(|builder| {
        builder.move_to(tip);
        builder.line_to(p1);
        builder.line_to(p2);
        builder.close();
    });
    frame.fill(&tri, color);
}

fn draw_stamp(
    frame: &mut Frame,
    st: &FloorplanStamp,
    tf: CanvasTransform,
    on_bg: Color,
    accent: Color,
    selected: bool,
) {
    let cx = tf.offset_x + ((st.x + st.w * 0.5) as f32) * tf.scale;
    let cy = tf.offset_y + ((st.y + st.h * 0.5) as f32) * tf.scale;
    let w = (st.w as f32) * tf.scale;
    let h = (st.h as f32) * tf.scale;
    let hw = w * 0.5;
    let hh = h * 0.5;
    let stroke_col = if selected { accent } else { with_alpha(on_bg, 0.36) };
    let fill_col = with_alpha(if selected { accent } else { on_bg }, 0.06);
    let stroke = Stroke::default()
        .with_color(stroke_col)
        .with_width(if selected { 1.8 } else { 1.2 })
        .with_line_cap(LineCap::Round)
        .with_line_join(LineJoin::Round);

    frame.with_save(|f| {
        f.translate(Vector::new(cx, cy));
        f.rotate(Radians((st.rot as f32).to_radians()));

        match st.kind.as_str() {
            "door" => {
                let r = w.min(h).max(10.0);
                let hinge = Point::new(-r * 0.5, -r * 0.5);
                let leaf = CanvasPath::new(|b| {
                    b.move_to(hinge);
                    b.line_to(Point::new(hinge.x + r, hinge.y));
                });
                let arc = CanvasPath::new(|b| {
                    b.arc(canvas::path::Arc {
                        center: hinge,
                        radius: r,
                        start_angle: Radians(0.0),
                        end_angle: Radians(FRAC_PI_2),
                    });
                });
                f.stroke(&leaf, stroke.with_width(2.0));
                f.stroke(
                    &arc,
                    Stroke {
                        style: Style::Solid(stroke_col),
                        width: 1.1,
                        line_dash: LineDash { segments: &[3.0, 3.0], offset: 0 },
                        ..Default::default()
                    },
                );
            }
            "window" => {
                let p = CanvasPath::rectangle(Point::new(-hw, -hh), Size::new(w, h));
                f.fill(&p, fill_col);
                f.stroke(&p, stroke);
                let mid = CanvasPath::line(Point::new(-hw, 0.0), Point::new(hw, 0.0));
                f.stroke(&mid, stroke.with_width(1.0));
            }
            "sofa" => {
                let body = CanvasPath::rounded_rectangle(Point::new(-hw, -hh), Size::new(w, h), 4.0.into());
                f.fill(&body, fill_col);
                f.stroke(&body, stroke);
                let back_y = -hh + h * 0.28;
                f.stroke(&CanvasPath::line(Point::new(-hw + 3.0, back_y), Point::new(hw - 3.0, back_y)), stroke);
                f.stroke(&CanvasPath::line(Point::new(0.0, back_y), Point::new(0.0, hh)), stroke);
            }
            "tv" => {
                let bar = CanvasPath::rounded_rectangle(Point::new(-hw, -hh), Size::new(w, h), 2.0.into());
                f.fill(&bar, with_alpha(stroke_col, 0.35));
                f.stroke(&bar, stroke);
                let stand = CanvasPath::line(Point::new(-w * 0.18, hh + 3.0), Point::new(w * 0.18, hh + 3.0));
                f.stroke(&stand, stroke);
            }
            "desk" => {
                let top = CanvasPath::rounded_rectangle(Point::new(-hw, -hh), Size::new(w, h * 0.72), 3.0.into());
                f.fill(&top, fill_col);
                f.stroke(&top, stroke);
                let monitor = CanvasPath::line(
                    Point::new(-w * 0.24, -hh + h * 0.22),
                    Point::new(w * 0.24, -hh + h * 0.22),
                );
                f.stroke(&monitor, stroke.with_width(2.0));
                let chair = CanvasPath::circle(Point::new(0.0, hh - h * 0.12), (h * 0.18).max(4.0));
                f.stroke(&chair, stroke);
            }
            "bed" => {
                let frame_r = CanvasPath::rounded_rectangle(Point::new(-hw, -hh), Size::new(w, h), 4.0.into());
                f.fill(&frame_r, fill_col);
                f.stroke(&frame_r, stroke);
                // Two pillows
                let pw = w * 0.36;
                let ph = h * 0.20;
                f.stroke(
                    &CanvasPath::rounded_rectangle(Point::new(-hw + w * 0.08, -hh + h * 0.08), Size::new(pw, ph), 3.0.into()),
                    stroke,
                );
                f.stroke(
                    &CanvasPath::rounded_rectangle(Point::new(hw - w * 0.08 - pw, -hh + h * 0.08), Size::new(pw, ph), 3.0.into()),
                    stroke,
                );
                // Blanket fold line
                f.stroke(&CanvasPath::line(Point::new(-hw, -hh + h * 0.42), Point::new(hw, -hh + h * 0.42)), stroke);
            }
            "dining" => {
                let tbl = CanvasPath::rounded_rectangle(Point::new(-hw, -hh), Size::new(w, h), 4.0.into());
                f.fill(&tbl, fill_col);
                f.stroke(&tbl, stroke);
                // 4 chairs top/bottom
                for sx in [-w * 0.24, w * 0.24] {
                    f.stroke(&CanvasPath::line(Point::new(sx - 6.0, -hh - 4.0), Point::new(sx + 6.0, -hh - 4.0)), stroke);
                    f.stroke(&CanvasPath::line(Point::new(sx - 6.0, hh + 4.0), Point::new(sx + 6.0, hh + 4.0)), stroke);
                }
            }
            "kitchen" => {
                let counter = CanvasPath::rectangle(Point::new(-hw, -hh), Size::new(w, h));
                f.fill(&counter, fill_col);
                f.stroke(&counter, stroke);
                // Sink on left
                let sink = CanvasPath::rounded_rectangle(
                    Point::new(-hw + w * 0.08, -hh + h * 0.18),
                    Size::new(w * 0.26, h * 0.64),
                    3.0.into(),
                );
                f.stroke(&sink, stroke);
                // 4-burner hob on right
                let r = (h * 0.15).clamp(2.5, 6.0);
                for (ox, oy) in [(0.16, -0.18), (0.32, -0.18), (0.16, 0.18), (0.32, 0.18)] {
                    f.stroke(&CanvasPath::circle(Point::new(w * ox, h * oy), r), stroke);
                }
            }
            _ => {
                let rect = CanvasPath::rectangle(Point::new(-hw, -hh), Size::new(w, h));
                f.stroke(&rect, stroke);
            }
        }

        if selected {
            let sel = CanvasPath::rectangle(Point::new(-hw - 3.0, -hh - 3.0), Size::new(w + 6.0, h + 6.0));
            f.stroke(
                &sel,
                Stroke {
                    style: Style::Solid(accent),
                    width: 1.2,
                    line_dash: LineDash { segments: &[4.0, 3.0], offset: 0 },
                    ..Default::default()
                },
            );
        }
    });
}

/// Draws a crisp geometric vector glyph inside a lamp node for all 12 allowlisted `LAMP_ICONS`.
fn draw_lamp_icon(frame: &mut Frame, c: Point, r: f32, icon_key: &str, ink: Color) {
    let s = Stroke::default()
        .with_color(ink)
        .with_width(1.6)
        .with_line_cap(LineCap::Round)
        .with_line_join(LineJoin::Round);

    match normalize_lamp_icon(icon_key) {
        "bulb" => {
            frame.stroke(&CanvasPath::circle(Point::new(c.x, c.y - r * 0.18), r * 0.56), s);
            frame.stroke(
                &CanvasPath::line(Point::new(c.x - r * 0.28, c.y + r * 0.62), Point::new(c.x + r * 0.28, c.y + r * 0.62)),
                s,
            );
            frame.stroke(
                &CanvasPath::line(Point::new(c.x - r * 0.18, c.y + r * 0.88), Point::new(c.x + r * 0.18, c.y + r * 0.88)),
                s,
            );
        }
        "spot" => {
            // Downward spotlight cone
            let p = CanvasPath::new(|b| {
                b.move_to(Point::new(c.x - r * 0.35, c.y - r * 0.7));
                b.line_to(Point::new(c.x + r * 0.35, c.y - r * 0.7));
                b.line_to(Point::new(c.x + r * 0.75, c.y + r * 0.35));
                b.line_to(Point::new(c.x - r * 0.75, c.y + r * 0.35));
                b.close();
            });
            frame.stroke(&p, s);
            frame.stroke(&CanvasPath::line(Point::new(c.x, c.y + r * 0.55), Point::new(c.x, c.y + r * 0.88)), s);
        }
        "strip" => {
            // Horizontal LED strip bar with 3 dots
            let bar = CanvasPath::rounded_rectangle(
                Point::new(c.x - r * 0.85, c.y - r * 0.28),
                Size::new(r * 1.7, r * 0.56),
                2.5.into(),
            );
            frame.stroke(&bar, s);
            for dx in [-0.45, 0.0, 0.45] {
                frame.fill(&CanvasPath::circle(Point::new(c.x + r * dx, c.y), 1.4), ink);
            }
        }
        "floor" => {
            // Tall floor lamp: trapezoid shade + stem + base
            let shade = CanvasPath::new(|b| {
                b.move_to(Point::new(c.x - r * 0.42, c.y - r * 0.82));
                b.line_to(Point::new(c.x + r * 0.42, c.y - r * 0.82));
                b.line_to(Point::new(c.x + r * 0.65, c.y - r * 0.12));
                b.line_to(Point::new(c.x - r * 0.65, c.y - r * 0.12));
                b.close();
            });
            frame.stroke(&shade, s);
            frame.stroke(&CanvasPath::line(Point::new(c.x, c.y - r * 0.12), Point::new(c.x, c.y + r * 0.84)), s);
            frame.stroke(&CanvasPath::line(Point::new(c.x - r * 0.45, c.y + r * 0.84), Point::new(c.x + r * 0.45, c.y + r * 0.84)), s);
        }
        "table" => {
            // Wide cozy dome shade + short base
            let shade = CanvasPath::new(|b| {
                b.move_to(Point::new(c.x - r * 0.52, c.y - r * 0.65));
                b.line_to(Point::new(c.x + r * 0.52, c.y - r * 0.65));
                b.line_to(Point::new(c.x + r * 0.78, c.y + r * 0.10));
                b.line_to(Point::new(c.x - r * 0.78, c.y + r * 0.10));
                b.close();
            });
            frame.stroke(&shade, s);
            frame.stroke(&CanvasPath::line(Point::new(c.x, c.y + r * 0.10), Point::new(c.x, c.y + r * 0.72)), s);
            frame.stroke(&CanvasPath::line(Point::new(c.x - r * 0.5, c.y + r * 0.72), Point::new(c.x + r * 0.5, c.y + r * 0.72)), s);
        }
        "pendant" => {
            // Hanging cord + dome
            frame.stroke(&CanvasPath::line(Point::new(c.x, c.y - r * 0.90), Point::new(c.x, c.y - r * 0.25)), s);
            let dome = CanvasPath::new(|b| {
                b.arc(canvas::path::Arc {
                    center: Point::new(c.x, c.y + r * 0.35),
                    radius: r * 0.65,
                    start_angle: Radians(PI),
                    end_angle: Radians(2.0 * PI),
                });
                b.close();
            });
            frame.stroke(&dome, s);
            frame.fill(&CanvasPath::circle(Point::new(c.x, c.y + r * 0.52), 1.6), ink);
        }
        "ceiling" => {
            // Flush ceiling mount dome + rays
            frame.stroke(&CanvasPath::line(Point::new(c.x - r * 0.75, c.y - r * 0.45), Point::new(c.x + r * 0.75, c.y - r * 0.45)), s);
            let dome = CanvasPath::new(|b| {
                b.arc(canvas::path::Arc {
                    center: Point::new(c.x, c.y - r * 0.45),
                    radius: r * 0.58,
                    start_angle: Radians(0.0),
                    end_angle: Radians(PI),
                });
            });
            frame.stroke(&dome, s);
            frame.stroke(&CanvasPath::line(Point::new(c.x, c.y + r * 0.38), Point::new(c.x, c.y + r * 0.72)), s);
        }
        "wall" => {
            // Wall bracket on left + sconce glow right
            frame.stroke(&CanvasPath::line(Point::new(c.x - r * 0.62, c.y - r * 0.72), Point::new(c.x - r * 0.62, c.y + r * 0.72)), s);
            let cup = CanvasPath::new(|b| {
                b.move_to(Point::new(c.x - r * 0.58, c.y - r * 0.20));
                b.line_to(Point::new(c.x + r * 0.45, c.y - r * 0.58));
                b.line_to(Point::new(c.x + r * 0.45, c.y + r * 0.58));
                b.line_to(Point::new(c.x - r * 0.58, c.y + r * 0.20));
                b.close();
            });
            frame.stroke(&cup, s);
        }
        "bar" => {
            // Vertical Play light bar
            let bar = CanvasPath::rounded_rectangle(
                Point::new(c.x - r * 0.26, c.y - r * 0.84),
                Size::new(r * 0.52, r * 1.68),
                2.5.into(),
            );
            frame.fill(&bar, with_alpha(ink, 0.28));
            frame.stroke(&bar, s);
        }
        "desk" => {
            // Articulated desk arm + angled lamp head + base
            let arm = CanvasPath::new(|b| {
                b.move_to(Point::new(c.x - r * 0.45, c.y + r * 0.75));
                b.line_to(Point::new(c.x - r * 0.15, c.y - r * 0.05));
                b.line_to(Point::new(c.x + r * 0.42, c.y - r * 0.65));
            });
            frame.stroke(&arm, s);
            frame.stroke(&CanvasPath::line(Point::new(c.x + r * 0.15, c.y - r * 0.80), Point::new(c.x + r * 0.72, c.y - r * 0.35)), s.with_width(2.2));
            frame.stroke(&CanvasPath::line(Point::new(c.x - r * 0.72, c.y + r * 0.75), Point::new(c.x + r * 0.25, c.y + r * 0.75)), s);
        }
        "outdoor" => {
            // Lantern / sun post
            frame.stroke(&CanvasPath::circle(c, r * 0.42), s);
            for i in 0..8 {
                let ang = (i as f32) * (PI / 4.0);
                let p1 = Point::new(c.x + ang.cos() * r * 0.62, c.y + ang.sin() * r * 0.62);
                let p2 = Point::new(c.x + ang.cos() * r * 0.88, c.y + ang.sin() * r * 0.88);
                frame.stroke(&CanvasPath::line(p1, p2), s.with_width(1.3));
            }
        }
        "candle" => {
            // Flame teardrop + candle stick
            let flame = CanvasPath::new(|b| {
                b.move_to(Point::new(c.x, c.y - r * 0.85));
                b.quadratic_curve_to(Point::new(c.x + r * 0.55, c.y - r * 0.15), Point::new(c.x, c.y + r * 0.08));
                b.quadratic_curve_to(Point::new(c.x - r * 0.55, c.y - r * 0.15), Point::new(c.x, c.y - r * 0.85));
            });
            frame.stroke(&flame, s);
            let stick = CanvasPath::rectangle(Point::new(c.x - r * 0.24, c.y + r * 0.18), Size::new(r * 0.48, r * 0.68));
            frame.stroke(&stick, s);
        }
        _ => {
            frame.stroke(&CanvasPath::circle(c, r * 0.55), s);
        }
    }
}

// ------------------------------------------------------------------------- Studio UI View

fn dim_caption<'a>(content: impl Into<std::borrow::Cow<'a, str>> + 'a) -> Element<'a, MapMsg> {
    text::caption(content)
        .class(theme::Text::Custom(|theme: &cosmic::Theme| iced_text::Style {
            color: Some(with_alpha(Color::from(theme.cosmic().on_bg_color()), 0.62)),
            ..Default::default()
        }))
        .into()
}

fn studio_card(skin: &str) -> theme::Container<'static> {
    let skin_st = skin_style(skin);
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let base = Color::from(theme.cosmic().on_bg_color());
        let (bg, border, width) = match skin_st {
            Some(st) => (with_alpha(st.bg_tint, 0.12), with_alpha(st.border_tint, 0.34), st.border_width),
            None => (with_alpha(base, 0.045), with_alpha(base, 0.12), 1.0),
        };
        iced_container::Style {
            background: Some(Background::Color(bg)),
            border: Border { radius: corners::now().small.into(), width, color: border },
            ..Default::default()
        }
    })
}

fn color_dot<'a>(hex: &str, on: bool, size: f32) -> Element<'a, MapMsg> {
    let c = parse_hex_color(hex);
    container(space::horizontal())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .class(theme::Container::custom(move |theme: &cosmic::Theme| {
            let base = Color::from(theme.cosmic().on_bg_color());
            let fill = if on { c } else { with_alpha(base, 0.14) };
            iced_container::Style {
                background: Some(Background::Color(fill)),
                border: Border { radius: (size * 0.5).into(), width: 1.0, color: with_alpha(base, 0.32) },
                ..Default::default()
            }
        }))
        .into()
}

pub fn view_map_tab<'a>(
    state: &'a FloorplanState,
    stats: &'a Value,
    skin: &'a str,
    window_mode: bool,
    window_width: f32,
) -> Element<'a, MapMsg> {
    let lights = extract_bridge_lights(stats);
    let ent_areas = parse_entertainment_areas(stats);
    let pending = compute_pending_sync_count(&state.doc, stats);
    let wide = window_mode && window_width >= 760.0;

    let mut root = column![].spacing(8);

    // ----------------------------------------------------------------- Toolbar Row 1
    let sidebar_btn = if state.sidebar_open {
        button::suggested(tr("Lamps"))
    } else {
        button::standard(tr("Lamps"))
    }
    .on_press(MapMsg::ToggleSidebar);

    let tpl_btn = button::standard(format!("{} ▾", template_label(&state.doc.template_id)))
        .on_press(MapMsg::ToggleTemplatesMenu);

    let add_room_btn = button::standard(tr("+ Room")).on_press(MapMsg::AddBox("room".into()));
    let add_zone_btn = button::standard(tr("+ Zone")).on_press(MapMsg::AddBox("zone".into()));
    let stamp_btn = button::standard(format!("{} ▾", tr("+ Stamp"))).on_press(MapMsg::ToggleStampsMenu);
    let bg_btn = if !state.doc.background.path.is_empty() || state.bg_open {
        button::suggested(tr("Background"))
    } else {
        button::standard(tr("Background"))
    }
    .on_press(MapMsg::ToggleBgMenu);

    let sync_label = if pending > 0 {
        trf("Sync All ({0})", &[pending.to_string()])
    } else {
        tr("Sync All to Bridge")
    };
    let sync_btn = if pending > 0 {
        button::suggested(sync_label)
    } else {
        button::standard(sync_label)
    }
    .on_press(MapMsg::SyncAll);

    let mut bar1: Vec<Element<MapMsg>> = vec![
        sidebar_btn.into(),
        tpl_btn.into(),
        add_room_btn.into(),
        add_zone_btn.into(),
        stamp_btn.into(),
        bg_btn.into(),
        sync_btn.into(),
    ];
    if !ent_areas.is_empty() {
        let ent_lbl = state
            .doc
            .entertainment_id
            .as_deref()
            .and_then(|id| ent_areas.iter().find(|a| a.id == id).map(|a| a.name.clone()))
            .unwrap_or_else(|| tr("Entertainment"));
        bar1.push(
            button::standard(format!("♪ {ent_lbl} ▾"))
                .on_press(MapMsg::ToggleEntMenu)
                .into(),
        );
    }

    // ----------------------------------------------------------------- Toolbar Row 2 (Modes & Layers)
    let select_btn = if state.mode == CanvasMode::Select {
        button::suggested(tr("Select / Move"))
    } else {
        button::standard(tr("Select / Move"))
    }
    .on_press(MapMsg::SetMode(CanvasMode::Select));

    let wire_btn = if state.mode == CanvasMode::Wire {
        button::suggested(tr("Wire"))
    } else {
        button::standard(tr("Wire"))
    }
    .on_press(MapMsg::SetMode(CanvasMode::Wire));

    let mut bar2: Vec<Element<MapMsg>> = vec![select_btn.into(), wire_btn.into()];
    if state.mode == CanvasMode::Wire {
        for wk in WireKind::ALL {
            let b = if state.wire_kind == wk {
                button::suggested(wk.label())
            } else {
                button::standard(wk.label())
            };
            bar2.push(b.on_press(MapMsg::SetWireKind(wk)).into());
        }
    }
    bar2.push(
        (if state.doc.show_glows { button::suggested(tr("Glows")) } else { button::standard(tr("Glows")) })
            .on_press(MapMsg::ToggleGlows)
            .into(),
    );
    bar2.push(
        (if state.doc.show_furniture { button::suggested(tr("Furniture")) } else { button::standard(tr("Furniture")) })
            .on_press(MapMsg::ToggleFurniture)
            .into(),
    );
    bar2.push(
        (if state.doc.show_wires { button::suggested(tr("Wires")) } else { button::standard(tr("Wires")) })
            .on_press(MapMsg::ToggleWires)
            .into(),
    );
    bar2.push(
        row![
            dim_caption(tr("Auto-sync")),
            toggler(state.doc.auto_sync).on_toggle(MapMsg::ToggleAutoSync),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into(),
    );

    root = root.push(
        container(
            column![
                flex_row(bar1).row_spacing(6).column_spacing(6),
                flex_row(bar2).row_spacing(6).column_spacing(6),
            ]
            .spacing(6),
        )
        .padding(8)
        .width(Length::Fill)
        .class(studio_card(skin)),
    );

    // ----------------------------------------------------------------- Dropdown Popovers
    if state.templates_open {
        let mut opts = column![dim_caption(tr("Choose Floorplan Prototype"))].spacing(4);
        let btns: Vec<Element<MapMsg>> = FLOORPLAN_TEMPLATES
            .iter()
            .map(|tid| {
                let active = state.doc.template_id == *tid;
                let lbl = if active {
                    format!("✓ {}", template_label(tid))
                } else {
                    template_label(tid)
                };
                let b = if active { button::suggested(lbl) } else { button::standard(lbl) };
                b.on_press(MapMsg::ApplyTemplate((*tid).to_string())).into()
            })
            .collect();
        opts = opts.push(flex_row(btns).row_spacing(6).column_spacing(6));
        root = root.push(container(opts).padding(8).width(Length::Fill).class(studio_card(skin)));
    }

    if state.stamps_open {
        let mut opts = column![dim_caption(tr("Add architectural stamp"))].spacing(4);
        let btns: Vec<Element<MapMsg>> = FLOORPLAN_STAMP_KINDS
            .iter()
            .map(|sk| {
                button::standard(stamp_kind_label(sk))
                    .on_press(MapMsg::AddStamp((*sk).to_string()))
                    .into()
            })
            .collect();
        opts = opts.push(flex_row(btns).row_spacing(6).column_spacing(6));
        root = root.push(container(opts).padding(8).width(Length::Fill).class(studio_card(skin)));
    }

    if state.ent_menu_open && !ent_areas.is_empty() {
        let mut opts = column![dim_caption(tr("Entertainment area"))].spacing(2);
        let none_lbl = if state.doc.entertainment_id.is_none() {
            format!("✓ {}", tr("None"))
        } else {
            tr("None")
        };
        opts = opts.push(menu_button(text::caption(none_lbl)).on_press(MapMsg::SetTopEntertainment(None)));
        for area in &ent_areas {
            let active = state.doc.entertainment_id.as_deref() == Some(area.id.as_str());
            let lbl = if active { format!("✓ {}", area.name) } else { area.name.clone() };
            opts = opts.push(
                menu_button(text::caption(lbl)).on_press(MapMsg::SetTopEntertainment(Some(area.id.clone()))),
            );
        }
        root = root.push(container(opts).padding(8).width(Length::Fill).class(studio_card(skin)));
    }

    if state.bg_open {
        let op_pct = state
            .bg_opacity_drag
            .unwrap_or_else(|| (state.doc.background.opacity * 100.0).round().clamp(5.0, 100.0) as u8);
        let fit_contain = state.doc.background.fit != "cover";
        let bg_panel = column![
            row![
                text::body(tr("Floorplan background")),
                space::horizontal(),
                button::suggested(tr("Choose image…")).on_press(MapMsg::ChooseBgFile),
                button::destructive(tr("Clear"))
                    .on_press_maybe((!state.doc.background.path.is_empty()).then_some(MapMsg::BgClear)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                text_input(tr("Image path (PNG, JPG, WebP)"), &state.bg_path_draft)
                    .on_input(MapMsg::BgPathInput)
                    .on_submit(|_| MapMsg::BgApplyPath)
                    .width(Length::Fill),
                button::standard(tr("Apply")).on_press(MapMsg::BgApplyPath),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            row![
                dim_caption(tr("Opacity")),
                slider(5u8..=100u8, op_pct, MapMsg::BgOpacity)
                    .step(5u8)
                    .on_release(MapMsg::BgOpacityDone)
                    .width(Length::Fill),
                container(text::caption(format!("{op_pct}%"))).width(Length::Fixed(38.0)),
                (if fit_contain { button::suggested(tr("Contain")) } else { button::standard(tr("Contain")) })
                    .on_press(MapMsg::BgFit("contain".into())),
                (if !fit_contain { button::suggested(tr("Cover")) } else { button::standard(tr("Cover")) })
                    .on_press(MapMsg::BgFit("cover".into())),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        ]
        .spacing(8);
        root = root.push(container(bg_panel).padding(10).width(Length::Fill).class(studio_card(skin)));
    }

    // Active mode hint banner (armed click-to-place or wire drawing)
    if let Some(ref armed_id) = state.armed_place_lamp {
        let lname = lights
            .iter()
            .find(|l| l.id == *armed_id)
            .map(|l| l.name.as_str())
            .unwrap_or("");
        root = root.push(
            container(
                row![
                    dim_caption(trf("Click on the canvas to place {0}", &[lname.to_string()])),
                    space::horizontal(),
                    button::text(tr("Cancel")).on_press(MapMsg::Select(None)),
                ]
                .align_y(Alignment::Center),
            )
            .padding([4, 8])
            .width(Length::Fill)
            .class(studio_card(skin)),
        );
    } else if state.mode == CanvasMode::Wire {
        let hint = if let Some(ref fid) = state.pending_wire_from {
            let lname = lights.iter().find(|l| l.id == *fid).map(|l| l.name.as_str()).unwrap_or("");
            trf("Click a target lamp to wire from {0}", &[lname.to_string()])
        } else {
            tr("Drag from one lamp to another to create a dependency wire.")
        };
        root = root.push(
            container(dim_caption(hint))
                .padding([4, 8])
                .width(Length::Fill)
                .class(studio_card(skin)),
        );
    }

    // ----------------------------------------------------------------- Canvas + Optional Background Layer
    let canvas_height = if window_mode { 440.0 } else { 300.0 };
    let program = FloorplanCanvasProgram::from_state(state, stats);
    let canvas_widget = cosmic::widget::canvas(program)
        .width(Length::Fill)
        .height(Length::Fixed(canvas_height));

    let bg_path = state.doc.background.path.trim();
    let layered_canvas: Element<MapMsg> = if !bg_path.is_empty() && Path::new(bg_path).is_file() {
        let fit = if state.doc.background.fit == "cover" {
            ContentFit::Cover
        } else {
            ContentFit::Contain
        };
        let opacity = state
            .bg_opacity_drag
            .map(|p| (p as f32) / 100.0)
            .unwrap_or(state.doc.background.opacity as f32)
            .clamp(0.05, 1.0);
        let bg_img = iced_image(iced_image::Handle::from_path(bg_path))
            .width(Length::Fill)
            .height(Length::Fixed(canvas_height))
            .content_fit(fit)
            .opacity(opacity);
        iced_stack![bg_img, canvas_widget]
            .width(Length::Fill)
            .height(Length::Fixed(canvas_height))
            .into()
    } else {
        canvas_widget.into()
    };

    let canvas_box = container(layered_canvas)
        .width(Length::Fill)
        .height(Length::Fixed(canvas_height))
        .class(studio_card(skin));

    // ----------------------------------------------------------------- Collapsible Left Lamp Sidebar
    let sidebar_el = state.sidebar_open.then(|| {
        let q = state.search.trim().to_lowercase();
        let mut unplaced = Vec::new();
        let mut placed = Vec::new();
        for l in &lights {
            if !q.is_empty()
                && !l.name.to_lowercase().contains(&q)
                && !l.room_name.to_lowercase().contains(&q)
            {
                continue;
            }
            if state.doc.lamps.contains_key(&l.id) {
                placed.push(l);
            } else {
                unplaced.push(l);
            }
        }

        let mut list_col = column![
            text_input(tr("Search lamps…"), &state.search).on_input(MapMsg::SearchChanged),
        ]
        .spacing(6);

        list_col = list_col.push(dim_caption(trf("Unplaced ({0})", &[unplaced.len().to_string()])));
        if unplaced.is_empty() {
            list_col = list_col.push(dim_caption(tr("All lamps placed")));
        } else {
            for l in unplaced {
                let meta = if l.room_name.is_empty() {
                    format!("{}%", l.bri.round() as i32)
                } else {
                    format!("{} · {}%", l.room_name, l.bri.round() as i32)
                };
                list_col = list_col.push(
                    row![
                        color_dot(&l.color, l.on, 10.0),
                        column![text::caption(l.name.clone()), dim_caption(meta)]
                            .spacing(1)
                            .width(Length::Fill),
                        button::standard(tr("Place")).on_press(MapMsg::PlaceLamp(l.id.clone())),
                        button::text("⌖").on_press(MapMsg::ArmPlaceLamp(l.id.clone())),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                );
            }
        }

        list_col = list_col.push(divider::horizontal::light());
        list_col = list_col.push(dim_caption(trf("Placed ({0})", &[placed.len().to_string()])));
        for l in placed {
            let ic = state
                .doc
                .lamps
                .get(&l.id)
                .map(|p| lamp_icon_glyph(&p.icon))
                .unwrap_or("◉");
            let selected = matches!(&state.selection, Some(Selection::Lamp(id)) if id == &l.id);
            let meta = if l.room_name.is_empty() {
                format!("{ic} {}%", l.bri.round() as i32)
            } else {
                format!("{ic} {} · {}%", l.room_name, l.bri.round() as i32)
            };
            let focus_btn = button::custom(
                row![
                    color_dot(&l.color, l.on, 10.0),
                    column![text::caption(l.name.clone()), dim_caption(meta)].spacing(1),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([3, 4])
            .class(if selected { theme::Button::Suggested } else { theme::Button::MenuItem })
            .on_press(MapMsg::FocusLamp(l.id.clone()));

            list_col = list_col.push(
                row![
                    focus_btn,
                    button::text("×").on_press(MapMsg::UnplaceLamp(l.id.clone())),
                ]
                .spacing(2)
                .align_y(Alignment::Center),
            );
        }

        let sc = scrollable(container(list_col).padding([0, 6, 0, 0]))
            .height(if wide { Length::Fixed(canvas_height - 16.0) } else { Length::Shrink });
        let box_c = container(sc)
            .padding(8)
            .class(studio_card(skin));
        if wide {
            box_c.width(Length::Fixed(225.0)).height(Length::Fixed(canvas_height))
        } else {
            box_c.width(Length::Fill).max_height(180.0)
        }
    });

    if let Some(sb) = sidebar_el {
        if wide {
            root = root.push(row![sb, canvas_box].spacing(8).align_y(Alignment::Start));
        } else {
            root = root.push(sb);
            root = root.push(canvas_box);
        }
    } else {
        root = root.push(canvas_box);
    }

    // ----------------------------------------------------------------- Context Inspector Panel
    if let Some(inspector) = view_inspector(state, stats, &lights, &ent_areas, skin) {
        root = root.push(inspector);
    }

    if window_mode {
        root.into()
    } else {
        container(scrollable(container(root).padding([0, 10, 0, 0])).height(Length::Shrink))
            .max_height(560.0)
            .into()
    }
}

fn view_inspector<'a>(
    state: &'a FloorplanState,
    stats: &'a Value,
    lights: &[BridgeLightInfo],
    ent_areas: &[EntertainmentArea],
    skin: &'a str,
) -> Option<Element<'a, MapMsg>> {
    let sel = state.selection.as_ref()?;
    let inner: Element<MapMsg> = match sel {
        Selection::Lamp(lid) => {
            let info = lights.iter().find(|l| l.id == *lid)?;
            let lpos = state.doc.lamps.get(lid)?;
            let current_icon = normalize_lamp_icon(&lpos.icon);
            let bri = info.bri.round().clamp(1.0, 100.0) as u8;
            let lid_toggle = lid.clone();
            let lid_bri = lid.clone();
            let lid_bri_done = lid.clone();
            let lid_rename = lid.clone();

            let icon_buttons: Vec<Element<MapMsg>> = LAMP_ICONS
                .iter()
                .map(|ic| {
                    let active = current_icon == *ic;
                    let lbl = format!("{} {}", lamp_icon_glyph(ic), lamp_icon_label(ic));
                    let b = if active { button::suggested(lbl) } else { button::standard(lbl) };
                    b.on_press(MapMsg::LampSetIcon(lid.clone(), (*ic).to_string())).into()
                })
                .collect();

            column![
                row![
                    color_dot(&info.color, info.on, 14.0),
                    text::heading(info.name.clone()),
                    space::horizontal(),
                    toggler(info.on).on_toggle(move |v| MapMsg::LampToggle(lid_toggle.clone(), v)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                row![
                    dim_caption(tr("Brightness")),
                    slider(1u8..=100u8, bri, move |v| MapMsg::LampBri(lid_bri.clone(), v))
                        .step(5u8)
                        .on_release(MapMsg::LampBriDone(lid_bri_done))
                        .width(Length::Fill),
                    container(text::caption(format!("{bri}%"))).width(Length::Fixed(38.0)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                row![
                    text_input(tr("Lamp name"), &state.lamp_rename_draft)
                        .on_input(MapMsg::LampRenameInput)
                        .on_submit(move |_| MapMsg::LampRenameSave(lid_rename.clone()))
                        .width(Length::Fill),
                    button::suggested(tr("Rename")).on_press(MapMsg::LampRenameSave(lid.clone())),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                column![
                    dim_caption(tr("Lamp icon")),
                    flex_row(icon_buttons).row_spacing(4).column_spacing(4),
                ]
                .spacing(4),
                row![
                    button::standard(tr("Wire from here")).on_press(MapMsg::LampStartWire(lid.clone())),
                    space::horizontal(),
                    button::destructive(tr("Unplace")).on_press(MapMsg::UnplaceLamp(lid.clone())),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(8)
            .into()
        }

        Selection::Box(bid) => {
            let b = state.doc.boxes.iter().find(|b| b.id == *bid)?;
            let bid_rename = bid.clone();
            let is_zone = b.kind == "zone";
            let hue_room = b.hue_id.as_deref().and_then(|hid| {
                j_list(stats, "rooms")
                    .iter()
                    .find(|r| j_str(r, "id") == hid)
            });

            let mut col = column![
                row![
                    text::heading(format!("{} {}", archetype_glyph(&b.archetype), b.name)),
                    space::horizontal(),
                    (if !is_zone { button::suggested(tr("Room")) } else { button::standard(tr("Room")) })
                        .on_press(MapMsg::BoxSetKind(bid.clone(), "room".into())),
                    (if is_zone { button::suggested(tr("Zone")) } else { button::standard(tr("Zone")) })
                        .on_press(MapMsg::BoxSetKind(bid.clone(), "zone".into())),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                row![
                    text_input(tr("Room name"), &state.box_rename_draft)
                        .on_input(MapMsg::BoxRenameInput)
                        .on_submit(move |_| MapMsg::BoxRenameSave(bid_rename.clone()))
                        .width(Length::Fill),
                    button::suggested(tr("Rename")).on_press(MapMsg::BoxRenameSave(bid.clone())),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                row![
                    dim_caption(tr("Archetype")),
                    space::horizontal(),
                    button::text(format!("{} {}", archetype_glyph(&b.archetype), archetype_label(&b.archetype)))
                        .trailing_icon(icon::from_name(if state.archetype_menu_open { "pan-up-symbolic" } else { "pan-down-symbolic" }).size(16))
                        .on_press(MapMsg::ToggleArchetypeMenu),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(8);

            if state.archetype_menu_open {
                let arch_btns: Vec<Element<MapMsg>> = FLOORPLAN_ARCHETYPES
                    .iter()
                    .map(|a| {
                        let active = b.archetype == *a;
                        let lbl = format!("{} {}", archetype_glyph(a), archetype_label(a));
                        let btn = if active { button::suggested(lbl) } else { button::standard(lbl) };
                        btn.on_press(MapMsg::BoxSetArchetype(bid.clone(), (*a).to_string())).into()
                    })
                    .collect();
                col = col.push(flex_row(arch_btns).row_spacing(4).column_spacing(4));
            }

            // Optional per-box Entertainment area linker
            if !ent_areas.is_empty() {
                let linked_name = b
                    .entertainment_area_id
                    .as_deref()
                    .and_then(|aid| ent_areas.iter().find(|a| a.id == aid).map(|a| a.name.clone()))
                    .unwrap_or_else(|| tr("None"));
                let mut ent_row = row![
                    dim_caption(tr("Entertainment area")),
                    space::horizontal(),
                    button::text(linked_name)
                        .trailing_icon(icon::from_name(if state.box_ent_menu_open { "pan-up-symbolic" } else { "pan-down-symbolic" }).size(16))
                        .on_press(MapMsg::ToggleBoxEntMenu),
                ]
                .spacing(6)
                .align_y(Alignment::Center);
                if let Some(aid) = &b.entertainment_area_id {
                    ent_row = ent_row.push(
                        button::standard(tr("Sync 3D"))
                            .on_press(MapMsg::BoxSyncEntertainment(aid.clone(), bid.clone())),
                    );
                }
                col = col.push(ent_row);

                if state.box_ent_menu_open {
                    let mut eopts = column![
                        menu_button(text::caption(tr("None")))
                            .on_press(MapMsg::BoxSetEntertainment(bid.clone(), None)),
                    ]
                    .spacing(2);
                    for a in ent_areas {
                        let active = b.entertainment_area_id.as_deref() == Some(a.id.as_str());
                        let lbl = if active { format!("✓ {}", a.name) } else { a.name.clone() };
                        eopts = eopts.push(
                            menu_button(text::caption(lbl))
                                .on_press(MapMsg::BoxSetEntertainment(bid.clone(), Some(a.id.clone()))),
                        );
                    }
                    col = col.push(eopts);
                }
            }

            // Scenes & Scene Scheduling section (when linked to a Bridge room/zone)
            if let Some(room_val) = hue_room {
                let rid = j_str(room_val, "id").to_string();
                let rkind = j_str(room_val, "kind").to_string();
                let rname = j_str(room_val, "name").to_string();
                let scenes = j_list(room_val, "scenes");

                col = col.push(divider::horizontal::light());
                col = col.push(dim_caption(tr("Scenes")));
                if scenes.is_empty() {
                    col = col.push(dim_caption(tr("No scenes in this room yet.")));
                } else {
                    let scene_pills: Vec<Element<MapMsg>> = scenes
                        .iter()
                        .map(|sc| {
                            let sid = j_str(sc, "id").to_string();
                            let sname = j_str(sc, "name").to_string();
                            let active = j_bool(sc, "active");
                            let recall_btn = if active {
                                button::suggested(sname.clone())
                            } else {
                                button::standard(sname.clone())
                            }
                            .on_press(MapMsg::RecallScene(sid.clone()));
                            let sched_btn = button::standard("⏰").on_press(MapMsg::ScheduleScene {
                                kind: rkind.clone(),
                                room_id: rid.clone(),
                                room_name: rname.clone(),
                                scene_id: sid,
                                scene_name: sname,
                            });
                            row![recall_btn, sched_btn].spacing(2).align_y(Alignment::Center).into()
                        })
                        .collect();
                    col = col.push(flex_row(scene_pills).row_spacing(6).column_spacing(6));
                }

                // Save current lighting as a new Hue Bridge scene
                let rid_sc = rid.clone();
                col = col.push(
                    row![
                        text_input(tr("New scene name"), &state.new_scene_name)
                            .on_input(MapMsg::NewSceneNameInput)
                            .on_submit(move |_| MapMsg::SaveCurrentScene(rid_sc.clone()))
                            .width(Length::Fill),
                        button::suggested(tr("Save as scene"))
                            .on_press_maybe((!state.new_scene_name.trim().is_empty()).then_some(MapMsg::SaveCurrentScene(rid))),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                );
            }

            // Box footer actions: Sync room, Remove from canvas, Delete on Bridge
            let mut footer = row![
                button::standard(tr("Sync rooms")).on_press(MapMsg::BoxSyncRoom),
                space::horizontal(),
                button::standard(tr("Remove box")).on_press(MapMsg::BoxRemoveFromCanvas(bid.clone())),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            if b.hue_id.is_some() {
                footer = footer.push(
                    button::destructive(tr("Delete on Bridge")).on_press(MapMsg::BoxDeleteOnBridge(bid.clone())),
                );
            }
            col = col.push(divider::horizontal::light());
            col = col.push(footer);
            col.into()
        }

        Selection::Stamp(sid) => {
            let st = state.doc.stamps.iter().find(|s| s.id == *sid)?;
            row![
                text::body(format!("{} ({}°)", stamp_kind_label(&st.kind), st.rot)),
                space::horizontal(),
                button::standard(tr("Rotate 90°")).on_press(MapMsg::StampRotate(sid.clone())),
                button::destructive(tr("Delete")).on_press(MapMsg::StampDelete(sid.clone())),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        }

        Selection::Wire(rid) => {
            let rule = j_list(stats, "rules").iter().find(|r| j_str(r, "id") == rid.as_str())?;
            let wkind = classify_rule_wire(rule);
            let enabled = j_bool(rule, "enabled");
            let rname = j_str(rule, "name");
            let title = if rname.is_empty() { wkind.label() } else { rname.to_string() };
            let rid_toggle = rid.clone();
            row![
                text::body(format!("{title} · {}", wkind.label())),
                space::horizontal(),
                toggler(enabled).on_toggle(move |v| MapMsg::WireToggle(rid_toggle.clone(), v)),
                button::destructive(tr("Delete")).on_press(MapMsg::WireDelete(rid.clone())),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        }
    };

    Some(container(inner).padding(10).width(Length::Fill).class(studio_card(skin)).into())
}

// ------------------------------------------------------------------------- Unit Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FloorplanBackground, FloorplanCanvas, FloorplanLamp};

    fn sample_box(id: &str, kind: &str, hue_id: Option<&str>, name: &str, x: f64, y: f64, w: f64, h: f64) -> FloorplanBox {
        FloorplanBox {
            id: id.into(),
            kind: kind.into(),
            hue_id: hue_id.map(str::to_owned),
            name: name.into(),
            archetype: "living_room".into(),
            x,
            y,
            w,
            h,
            entertainment_area_id: None,
        }
    }

    #[test]
    fn containment_hit_testing_prefers_smallest_and_respects_shared_wall_tie() {
        let big = sample_box("b1", "room", Some("r1"), "Living", 0.0, 0.0, 400.0, 400.0);
        let small = sample_box("b2", "room", Some("r2"), "Corner", 100.0, 100.0, 100.0, 100.0);
        let right = sample_box("b3", "room", Some("r3"), "Kitchen", 400.0, 0.0, 400.0, 400.0);
        let zone = sample_box("z1", "zone", Some("z1"), "Downstairs", 0.0, 0.0, 800.0, 400.0);
        let boxes = vec![big.clone(), small.clone(), right.clone(), zone];

        // Inside nested smaller room box -> smallest area wins (ignores zone!)
        let hit = containing_room_box(150.0, 150.0, &boxes, "r1").unwrap();
        assert_eq!(hit.id, "b2");

        // On shared wall (x=400) between equal-area rooms b1(r1) and b3(r3) -> keeps current room
        let tie_r1 = containing_room_box(400.0, 200.0, &boxes, "r1").unwrap();
        assert_eq!(tie_r1.id, "b1");
        let tie_r3 = containing_room_box(400.0, 200.0, &boxes, "r3").unwrap();
        assert_eq!(tie_r3.id, "b3");

        // Outside all rooms -> None
        assert!(containing_room_box(900.0, 500.0, &boxes, "r1").is_none());
    }

    #[test]
    fn wire_classification_distinguishes_all_four_types() {
        assert_eq!(classify_rule_wire(&json!({"kind": "follow", "mirror_off": false})), WireKind::Follow);
        assert_eq!(classify_rule_wire(&json!({"kind": "follow", "mirror_off": true})), WireKind::Mirror);
        assert_eq!(classify_rule_wire(&json!({"kind": "exclusive", "mirror_off": false})), WireKind::Exclusive);
        assert_eq!(classify_rule_wire(&json!({"kind": "companion", "mirror_off": false})), WireKind::Companion);

        let payload = WireKind::Mirror.to_rule_payload("l1", "Desk", "l2", "Strip");
        assert_eq!(payload["cmd"], "rule_save");
        assert_eq!(payload["rule"]["kind"], "follow");
        assert_eq!(payload["rule"]["mirror_off"], true);

        let excl = WireKind::Exclusive.to_rule_payload("l1", "Desk", "l2", "Strip");
        assert_eq!(excl["rule"]["kind"], "exclusive");
        assert_eq!(excl["rule"]["targets"][0]["action"], "off");
    }

    #[test]
    fn diff_detection_counts_moves_renames_new_boxes_and_zones() {
        let stats = json!({
            "rooms": [
                {
                    "id": "r1", "kind": "room", "name": "Living Room", "archetype": "living_room",
                    "lights": [{"id": "l1", "device_id": "d1", "name": "Arc", "on": true, "bri": 80.0, "color": "#ffaa44"}]
                },
                {
                    "id": "r2", "kind": "room", "name": "Office", "archetype": "office",
                    "lights": [{"id": "l2", "device_id": "d2", "name": "Desk", "on": true, "bri": 90.0, "color": "#ffffff"}]
                }
            ]
        });

        let mut doc = FloorplanDocument {
            version: 1,
            bridge_id: String::new(),
            template_id: "auto_bridge".into(),
            entertainment_id: None,
            canvas: FloorplanCanvas::default(),
            show_wires: true,
            show_glows: true,
            show_furniture: true,
            auto_sync: false,
            auto_sync_rooms: false,
            background: FloorplanBackground::default(),
            boxes: vec![
                sample_box("b1", "room", Some("r1"), "Living Room", 80.0, 80.0, 300.0, 260.0),
                FloorplanBox {
                    archetype: "office".into(),
                    ..sample_box("b2", "room", Some("r2"), "Office", 420.0, 80.0, 300.0, 260.0)
                },
            ],
            stamps: vec![],
            lamps: [
                ("l1".to_string(), FloorplanLamp { x: 180.0, y: 180.0, z: 0.0, icon: "floor".into() }),
                ("l2".to_string(), FloorplanLamp { x: 520.0, y: 180.0, z: 0.0, icon: "desk".into() }),
            ]
            .into_iter()
            .collect(),
        };

        // Initially in sync -> 0 pending
        assert_eq!(compute_pending_sync_count(&doc, &stats), 0);

        // Move l1 into Office box -> 1 pending move
        doc.lamps.get_mut("l1").unwrap().x = 500.0;
        assert_eq!(compute_pending_sync_count(&doc, &stats), 1);

        // Rename Living Room box + add a new unlinked Zone -> +2 pending (total 3)
        doc.boxes[0].name = "Cinema".into();
        doc.boxes.push(sample_box("z-new", "zone", None, "Movie Zone", 80.0, 80.0, 640.0, 260.0));
        assert_eq!(compute_pending_sync_count(&doc, &stats), 3);
    }

    #[test]
    fn box_resize_clamps_to_minimum_dimensions() {
        let orig = sample_box("b1", "room", None, "Room", 100.0, 100.0, 200.0, 200.0);
        let (x, y, w, h) = apply_resize(&orig, ResizeHandle::SE, 110.0, 115.0, 20, true, 1600.0, 1000.0);
        assert_eq!(x, 100.0);
        assert_eq!(y, 100.0);
        assert!(w >= MIN_BOX_DIM);
        assert!(h >= MIN_BOX_DIM);
    }
}
