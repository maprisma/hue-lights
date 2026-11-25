//! Interactive Visual Floorplan & Spatial Light Studio (`Tab::Map`).
//!
//! Renders a 2D spatial studio over an optional custom floorplan image with
//! resizable Room/Zone boxes, minimalist architectural vector stamps, live
//! radial lamp halos, 12 selectable vector lamp icons, typed Bezier rule wires,
//! collapsible lamp sidebar with per-lamp actions and dialogs, and context inspectors
//! (Room/Scenes, Stamp, Wire).

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
    container as iced_container, image as iced_image, stack as iced_stack, svg as iced_svg,
    text as iced_text,
};
use cosmic::iced::{
    Alignment, Background, Border, Color, ContentFit, Length, Pixels, Point, Radians, Rectangle,
    Size, Vector,
};
use cosmic::widget::{
    button, checkbox, column, container, divider, flex_row, icon, row, scrollable, slider, space,
    text, text_input, toggler,
};
use cosmic::{Element, theme};
use maprisma_hue_lights::corners;
use maprisma_hue_lights::i18n::{tr, trf};
use serde_json::{Value, json};

use crate::model::{
    BridgeLightInfo, EntertainmentArea, FLOORPLAN_ARCHETYPES, FLOORPLAN_ROOM_SHAPES,
    FLOORPLAN_STAMP_KINDS, FLOORPLAN_TEMPLATES, FloorplanBox, FloorplanDocument, FloorplanPoint,
    FloorplanStamp, HueSensor, LAMP_ICONS, MotionArea, PLANNED_LIGHT_PRESETS,
    archetype_glyph, archetype_label, default_trigger_state, extract_bridge_lights,
    is_light_or_room_trigger, j_bool, j_list, j_str, lamp_icon_label,
    lamp_icon_svg, motion_aware_eligible_rooms, normalize_lamp_icon, normalize_trigger_state,
    parse_entertainment_areas, parse_motion_areas, parse_sensors, resolve_name,
    room_active_schedule_badge, should_show_motion_aware_hint, skin_style, stamp_kind_label,
    synth_target_lights, target_lux_for_archetype, template_label, trigger_state_options,
    valid_hhmm,
};

// ------------------------------------------------------------------------- Visual constants

const WARM_FALLBACK: Color = Color::from_rgb(1.0, 0.824, 0.478);
const WIRE_FOLLOW: Color = Color::from_rgb(0.220, 0.741, 0.973); // #38BDF8
const WIRE_MIRROR: Color = Color::from_rgb(0.133, 0.827, 0.933); // #22D3EE
const WIRE_EXCLUSIVE: Color = Color::from_rgb(0.957, 0.247, 0.369); // #F43F5E
const WIRE_COMPANION: Color = Color::from_rgb(0.961, 0.620, 0.043); // #F59E0B
const SYNTH_MAGENTA: Color = Color::from_rgb(0.851, 0.275, 0.937); // #D946EF
const MOTION_ACTIVE_COLOR: Color = Color::from_rgb8(0x10, 0xB9, 0x81); // #10B981
const MOTION_IDLE_COLOR: Color = Color::from_rgb8(0x06, 0xB6, 0xD4); // #06B6D4
const CONTACT_OPEN_COLOR: Color = Color::from_rgb8(0xF5, 0x9E, 0x0B); // #F59E0B
const CONTACT_CLOSED_COLOR: Color = Color::from_rgb8(0x14, 0xB8, 0xA6); // #14B8A6
const BUTTON_COLOR: Color = Color::from_rgb8(0x63, 0x66, 0xF1); // #6366F1

const LAMP_RADIUS_PX: f32 = 18.0;
const SENSOR_HALF_PX: f32 = 14.0;
const HANDLE_SIZE_PX: f32 = 8.0;
const MIN_BOX_DIM: f64 = 60.0;

// ------------------------------------------------------------------------- Modes, Wire Types, Selection

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CanvasMode {
    #[default]
    Select,
    Plan,
    Wire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AmbientMode {
    #[default]
    Day,
    Dusk,
    Night,
}

impl AmbientMode {
    pub fn dim_alpha(self) -> f32 {
        match self {
            AmbientMode::Day => 0.0,
            AmbientMode::Dusk => 0.28,
            AmbientMode::Night => 0.56,
        }
    }
    pub fn glow_boost(self) -> f32 {
        match self {
            AmbientMode::Day => 1.0,
            AmbientMode::Dusk => 1.22,
            AmbientMode::Night => 1.48,
        }
    }
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
    #[allow(dead_code)]
    pub const ALL: [WireKind; 4] = [
        WireKind::Follow,
        WireKind::Mirror,
        WireKind::Exclusive,
        WireKind::Companion,
    ];

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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
    Sensor(String),
    Box(String),
    Stamp(String),
    Planned(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LampDialog {
    Brightness(String),
    Rename(String),
    Icon(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapRuleDialog {
    pub rule_id: Option<String>,
    pub name: String,
    pub enabled: bool,
    pub kind: String,          // "follow" | "exclusive" | "companion"
    pub trigger_kind: String,  // "light" | "room" | "motion" | "contact" | "button" | "motion_area"
    pub trigger_id: String,
    pub trigger_name: String,
    pub trigger_state: String, // "on"|"off"|"motion"|"clear"|"open"|"closed"|"press"|"long_press"
    pub target_kind: String,   // "light" | "room"
    pub target_id: String,
    pub target_name: String,
    pub mirror_off: bool,
    pub match_bri: bool,
    pub only_if_off: bool,
    pub time_start: String,
    pub time_end: String,
    pub error: Option<String>,
}

impl MapRuleDialog {
    pub fn new_between(
        from_kind: String,
        from_id: String,
        from_name: String,
        to_id: String,
        to_name: String,
    ) -> Self {
        let trigger_state = default_trigger_state(&from_kind).into();
        let mirror_off = from_kind != "button";
        Self {
            rule_id: None,
            name: String::new(),
            enabled: true,
            kind: "follow".into(),
            trigger_kind: from_kind,
            trigger_id: from_id,
            trigger_name: from_name,
            trigger_state,
            target_kind: "light".into(),
            target_id: to_id,
            target_name: to_name,
            mirror_off,
            match_bri: false,
            only_if_off: false,
            time_start: String::new(),
            time_end: String::new(),
            error: None,
        }
    }

    #[allow(dead_code)]
    pub fn new_between_lamps(
        from_id: String,
        from_name: String,
        to_id: String,
        to_name: String,
    ) -> Self {
        Self::new_between("light".into(), from_id, from_name, to_id, to_name)
    }

    pub fn from_existing_rule(rule: &Value, stats: &Value) -> Option<Self> {
        let id = j_str(rule, "id");
        if id.is_empty() {
            return None;
        }
        let name = j_str(rule, "name").to_string();
        let enabled = j_bool(rule, "enabled");
        let kind = match j_str(rule, "kind") {
            "exclusive" => "exclusive",
            "companion" => "companion",
            _ => "follow",
        }
        .to_string();
        let trigger_kind = j_str(rule, "trigger_kind").to_string();
        let trigger_id = j_str(rule, "trigger_id").to_string();
        let trigger_state = normalize_trigger_state(&trigger_kind, j_str(rule, "trigger_state")).to_string();
        let first_target = j_list(rule, "targets").first();
        let target_kind = first_target.map(|t| j_str(t, "kind")).unwrap_or("").to_string();
        let target_id = first_target.map(|t| j_str(t, "id")).unwrap_or("").to_string();
        let trigger_name = resolve_name(stats, &trigger_kind, &trigger_id);
        let target_name = resolve_name(stats, &target_kind, &target_id);
        Some(Self {
            rule_id: Some(id.to_string()),
            name,
            enabled,
            kind,
            trigger_kind,
            trigger_id,
            trigger_name,
            trigger_state,
            target_kind,
            target_id,
            target_name,
            mirror_off: j_bool(rule, "mirror_off"),
            match_bri: j_bool(rule, "match_bri"),
            only_if_off: j_bool(rule, "only_if_off"),
            time_start: j_str(rule, "time_start").to_string(),
            time_end: j_str(rule, "time_end").to_string(),
            error: None,
        })
    }

    pub fn target_action(&self) -> &'static str {
        if self.kind == "exclusive" { "off" } else { "on" }
    }

    pub fn to_save_command(&self) -> Result<Value, String> {
        if self.trigger_id.is_empty()
            || self.target_id.is_empty()
            || (self.trigger_kind == self.target_kind && self.trigger_id == self.target_id)
        {
            return Err(tr("Choose trigger and target"));
        }
        let ts = self.time_start.trim();
        let te = self.time_end.trim();
        if (!ts.is_empty() && !valid_hhmm(ts)) || (!te.is_empty() && !valid_hhmm(te)) {
            return Err(tr("Invalid time (use HH:MM)"));
        }
        let supports_bri = is_light_or_room_trigger(&self.trigger_kind);
        let supports_mirror = self.trigger_kind != "button";
        let mirror_off = self.mirror_off && self.kind != "exclusive" && supports_mirror;
        let match_bri = self.match_bri && self.kind != "exclusive" && supports_bri;
        let trigger_state = normalize_trigger_state(&self.trigger_kind, &self.trigger_state);
        let name = if self.name.trim().is_empty() {
            let arrow = if self.kind == "exclusive" {
                "⊘"
            } else if self.kind == "follow" && mirror_off {
                "↔"
            } else {
                "→"
            };
            let a = self.trigger_name.split(" · ").next().unwrap_or(&self.trigger_name);
            let b = self.target_name.split(" · ").next().unwrap_or(&self.target_name);
            format!("{a} {arrow} {b}")
        } else {
            self.name.trim().to_string()
        };
        let mut rule_obj = json!({
            "name": name,
            "enabled": self.enabled,
            "kind": self.kind,
            "trigger_kind": self.trigger_kind,
            "trigger_id": self.trigger_id,
            "trigger_state": trigger_state,
            "targets": [{
                "kind": self.target_kind,
                "id": self.target_id,
                "action": self.target_action(),
                "bri": Value::Null,
                "scene_id": "",
            }],
            "mirror_off": mirror_off,
            "match_bri": match_bri,
            "time_start": ts,
            "time_end": te,
            "only_if_off": self.only_if_off && self.kind != "exclusive",
        });
        if let Some(ref rid) = self.rule_id {
            rule_obj["id"] = json!(rid);
        }
        Ok(json!({ "cmd": "rule_save", "rule": rule_obj }))
    }
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
        self.world_point_rect(b.x, b.y, b.w, b.h)
    }

    pub fn world_point_rect(self, x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let rx = x + w;
        let by = y + h;
        match self {
            ResizeHandle::NW => (x, y),
            ResizeHandle::N => (cx, y),
            ResizeHandle::NE => (rx, y),
            ResizeHandle::E => (rx, cy),
            ResizeHandle::SE => (rx, by),
            ResizeHandle::S => (cx, by),
            ResizeHandle::SW => (x, by),
            ResizeHandle::W => (x, cy),
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
    ToggleAddMenu,
    ToggleViewMenu,
    ToggleTemplatesMenu,
    ToggleGalleryIncludePlan(bool),
    ApplyTemplate(String),
    AddBox(String), // "room" | "zone"
    AddBoxWithShape(String, String), // (kind, shape)
    AddStamp(String),
    SetMode(CanvasMode),
    SetAmbient(AmbientMode),
    // Multi-floor
    SelectFloor(String),
    AddFloor,
    DeleteFloor(String),
    FloorRenameStart,
    FloorRenameInput(String),
    FloorRenameSave,
    FloorRenameCancel,
    UndoFloorplan,
    BoxSetFloor(String, String),
    // Polygon reshaping
    BoxApplyShape(String, String),
    MoveBoxVertex { box_id: String, index: usize, x: f64, y: f64, commit: bool },
    InsertBoxVertex { box_id: String, after_edge: usize, x: f64, y: f64 },
    RemoveBoxVertex { box_id: String, index: usize },
    // Future Lighting Planner (Plan mode)
    TogglePlanAddMenu,
    TogglePlanList,
    AddPlannedLight(String), // preset id
    MovePlannedLight { id: String, x: f64, y: f64, commit: bool },
    MovePlannedStripVertex { id: String, index: usize, x: f64, y: f64, commit: bool },
    TogglePlannedLight(String),
    PlannedSetLumens(String, u32),
    PlannedSetKelvin(String, u32),
    PlannedSetBeam(String, f64),
    PlannedNameInput(String),
    PlannedNameSave(String),
    PlannedPriceInput(String),
    PlannedPriceSave(String),
    BindPlannedToReal { planned_id: String, real_light_id: String },
    DeletePlannedLight(String),
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
    ToggleLampMenu(String),
    OpenLampDialog(LampDialog),
    CloseLampDialog,
    // Sensors & MotionAware
    PlaceSensor(String),
    FocusSensor(String),
    UnplaceSensor(String),
    ToggleSensorMenu(String),
    MoveSensor { id: String, x: f64, y: f64, commit: bool },
    SensorToggle { kind: String, id: String, enabled: bool },
    SensorStartWire { kind: String, id: String },
    MotionAreaToggle(String, bool),
    MotionAreaSensitivity(String, i64),
    DismissMotionHint,
    // Canvas interactions
    Select(Option<Selection>),
    MoveLamp { id: String, x: f64, y: f64, commit: bool },
    MoveBox { id: String, x: f64, y: f64, commit: bool },
    ResizeBox { id: String, x: f64, y: f64, w: f64, h: f64, commit: bool },
    MoveStamp { id: String, x: f64, y: f64, commit: bool },
    PlaceArmedAt { x: f64, y: f64 },
    CreateWire { from_kind: String, from_id: String, to_id: String },
    // Lamp inspector / dialogs
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
    BoxConfirmDeleteOnBridge(String),
    BoxCancelDeleteOnBridge,
    BoxDeleteOnBridge(String),
    RecallScene(String),
    NewSceneNameInput(String),
    SaveCurrentScene(String),
    ScheduleScene { kind: String, room_id: String, room_name: String, scene_id: String, scene_name: String },
    // Stamp inspector
    StampRotate(String),
    StampDelete(String),
    // Rule dialog
    OpenRuleDialog(String),
    CloseRuleDialog,
    RuleDialogName(String),
    RuleDialogEnabled(bool),
    RuleDialogKind(String),
    RuleDialogTriggerState(String),
    RuleDialogMirrorOff(bool),
    RuleDialogMatchBri(bool),
    RuleDialogOnlyIfOff(bool),
    RuleDialogTimeStart(String),
    RuleDialogTimeEnd(String),
    RuleDialogSave,
    RuleDialogDelete(String),
}

// ------------------------------------------------------------------------- State

#[derive(Debug, Clone)]
pub struct FloorplanState {
    pub doc: FloorplanDocument,
    pub undo_doc: Option<FloorplanDocument>,
    pub confirm_template_id: Option<String>,
    pub confirm_delete_bridge_box: Option<String>,
    pub floor_rename_open: bool,
    pub floor_rename_draft: String,
    pub local_dirty: bool,
    pub sidebar_open: bool,
    pub search: String,
    pub mode: CanvasMode,
    pub ambient: AmbientMode,
    pub gallery_include_plan: bool,
    pub plan_add_open: bool,
    pub plan_list_open: bool,
    pub planned_name_draft: String,
    pub planned_price_draft: String,
    pub wire_kind: WireKind,
    pub selection: Option<Selection>,
    pub armed_place_lamp: Option<String>,
    pub pending_wire_from: Option<(String, String)>,
    // Menus / popovers / dialogs
    pub add_menu_open: bool,
    pub view_menu_open: bool,
    pub templates_open: bool,
    pub stamps_open: bool,
    pub bg_open: bool,
    pub ent_menu_open: bool,
    pub archetype_menu_open: bool,
    pub box_ent_menu_open: bool,
    pub open_lamp_menu: Option<String>,
    pub open_sensor_menu: Option<String>,
    pub lamp_dialog: Option<LampDialog>,
    pub rule_dialog: Option<MapRuleDialog>,
    // Inline text inputs
    pub lamp_rename_draft: String,
    pub box_rename_draft: String,
    pub new_scene_name: String,
    pub bg_path_draft: String,
    pub bg_opacity_drag: Option<u8>,
    // Monotonic local ID counter for new boxes/stamps
    pub next_id_seq: u32,
    // Cached background image handle & validated path (avoids per-frame stat/reload)
    pub bg_handle_cache: Option<(String, iced_image::Handle)>,
}

impl Default for FloorplanState {
    fn default() -> Self {
        Self {
            doc: FloorplanDocument::default(),
            undo_doc: None,
            confirm_template_id: None,
            confirm_delete_bridge_box: None,
            floor_rename_open: false,
            floor_rename_draft: String::new(),
            local_dirty: false,
            sidebar_open: true,
            search: String::new(),
            mode: CanvasMode::Select,
            ambient: AmbientMode::Day,
            gallery_include_plan: false,
            plan_add_open: false,
            plan_list_open: false,
            planned_name_draft: String::new(),
            planned_price_draft: String::new(),
            wire_kind: WireKind::Follow,
            selection: None,
            armed_place_lamp: None,
            pending_wire_from: None,
            add_menu_open: false,
            view_menu_open: false,
            templates_open: false,
            stamps_open: false,
            bg_open: false,
            ent_menu_open: false,
            archetype_menu_open: false,
            box_ent_menu_open: false,
            open_lamp_menu: None,
            open_sensor_menu: None,
            lamp_dialog: None,
            rule_dialog: None,
            lamp_rename_draft: String::new(),
            box_rename_draft: String::new(),
            new_scene_name: String::new(),
            bg_path_draft: String::new(),
            bg_opacity_drag: None,
            next_id_seq: 1,
            bg_handle_cache: None,
        }
    }
}

impl FloorplanState {
    pub fn any_menu_open(&self) -> bool {
        self.add_menu_open
            || self.view_menu_open
            || self.templates_open
            || self.plan_add_open
            || self.plan_list_open
            || self.stamps_open
            || self.bg_open
            || self.ent_menu_open
            || self.archetype_menu_open
            || self.box_ent_menu_open
            || self.open_lamp_menu.is_some()
            || self.open_sensor_menu.is_some()
    }

    pub fn close_menus(&mut self) {
        self.add_menu_open = false;
        self.view_menu_open = false;
        self.templates_open = false;
        self.plan_add_open = false;
        self.plan_list_open = false;
        self.stamps_open = false;
        self.bg_open = false;
        self.ent_menu_open = false;
        self.archetype_menu_open = false;
        self.box_ent_menu_open = false;
        self.open_lamp_menu = None;
        self.open_sensor_menu = None;
        self.confirm_delete_bridge_box = None;
    }

    pub fn has_dialog_open(&self) -> bool {
        self.lamp_dialog.is_some() || self.rule_dialog.is_some()
    }

    pub fn close_dialog(&mut self) {
        self.lamp_dialog = None;
        self.rule_dialog = None;
    }

    pub fn open_lamp_dialog(&mut self, dialog: LampDialog, stats: &Value) {
        self.open_lamp_menu = None;
        self.open_sensor_menu = None;
        self.rule_dialog = None;
        if let LampDialog::Rename(ref lid) = dialog {
            let name = extract_bridge_lights(stats)
                .into_iter()
                .find(|l| l.id == *lid)
                .map(|l| l.name)
                .unwrap_or_default();
            self.lamp_rename_draft = name;
        }
        self.lamp_dialog = Some(dialog);
    }

    pub fn open_new_rule_dialog(
        &mut self,
        from_kind: String,
        from_id: String,
        to_id: String,
        stats: &Value,
    ) {
        self.open_lamp_menu = None;
        self.open_sensor_menu = None;
        self.lamp_dialog = None;
        self.pending_wire_from = None;
        let from_name = resolve_name(stats, &from_kind, &from_id);
        let to_name = resolve_name(stats, "light", &to_id);
        self.rule_dialog = Some(MapRuleDialog::new_between(
            from_kind, from_id, from_name, to_id, to_name,
        ));
    }

    pub fn open_existing_rule_dialog(&mut self, rule_id: &str, stats: &Value) {
        self.open_lamp_menu = None;
        self.open_sensor_menu = None;
        self.lamp_dialog = None;
        if let Some(rule) = j_list(stats, "rules").iter().find(|r| j_str(r, "id") == rule_id) {
            self.rule_dialog = MapRuleDialog::from_existing_rule(rule, stats);
        }
    }

    pub fn refresh_bg_cache(&mut self) {
        let trimmed = self.doc.background.path.trim();
        if trimmed.is_empty() {
            self.bg_handle_cache = None;
            return;
        }
        if self
            .bg_handle_cache
            .as_ref()
            .is_some_and(|(cached_path, _)| cached_path == trimmed)
        {
            return;
        }
        if Path::new(trimmed).is_file() {
            self.bg_handle_cache = Some((trimmed.to_string(), iced_image::Handle::from_path(trimmed)));
        } else {
            self.bg_handle_cache = None;
        }
    }

    pub fn sync_from_snapshot(&mut self, incoming: FloorplanDocument) {
        if !self.local_dirty {
            self.doc = incoming;
            if self.bg_opacity_drag.is_none() && self.bg_path_draft.is_empty() {
                self.bg_path_draft = self.doc.background.path.clone();
            }
            self.refresh_bg_cache();
        }
    }

    pub fn select(&mut self, sel: Option<Selection>, stats: &Value) {
        self.selection = sel.clone();
        self.archetype_menu_open = false;
        self.box_ent_menu_open = false;
        self.confirm_delete_bridge_box = None;
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
            Some(Selection::Planned(ref pid)) => {
                if let Some(pl) = self.doc.planned_lights.iter().find(|p| p.id == *pid) {
                    self.planned_name_draft = pl.name.clone();
                    self.planned_price_draft = if pl.price > 0.0 {
                        format!("{:.0}", pl.price)
                    } else {
                        String::new()
                    };
                }
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
    b.contains_point(x, y)
}

pub fn snap_coord(val: f64, grid: i32, snap: bool) -> f64 {
    if !snap || grid <= 1 {
        return (val * 10.0).round() / 10.0;
    }
    let g = grid as f64;
    ((val / g).round() * g * 10.0).round() / 10.0
}

/// Finds the smallest-area `kind == "room"` box on `floor_id` containing `(x, y)`.
/// On shared-wall ties, prefers the box matching `current_room_id` (mirrors `hue-lights.py`).
pub fn containing_room_box_on_floor<'a>(
    x: f64,
    y: f64,
    floor_id: &str,
    boxes: &'a [FloorplanBox],
    current_room_id: &str,
) -> Option<&'a FloorplanBox> {
    let mut best: Option<&FloorplanBox> = None;
    for b in boxes {
        if b.kind != "room" || (!floor_id.is_empty() && b.floor_id != floor_id) || !point_in_box(x, y, b) {
            continue;
        }
        let area = b.area_px();
        let tie = if b.hue_id.as_deref() == Some(current_room_id) && !current_room_id.is_empty() {
            0u8
        } else {
            1u8
        };
        let replace = match best {
            None => true,
            Some(prev) => {
                let prev_area = prev.area_px();
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

#[cfg(test)]
pub fn containing_room_box<'a>(
    x: f64,
    y: f64,
    boxes: &'a [FloorplanBox],
    current_room_id: &str,
) -> Option<&'a FloorplanBox> {
    containing_room_box_on_floor(x, y, "", boxes, current_room_id)
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
    // B2: Without a paired Hue bridge, never report pending Hue Bridge sync changes.
    if stats.get("hue_paired").and_then(Value::as_bool) == Some(false) {
        return 0;
    }
    let mut pending = 0usize;
    let rooms_val = j_list(stats, "rooms");

    let mut known_rooms: HashMap<&str, &Value> = HashMap::new();
    let mut known_zones: HashMap<&str, &Value> = HashMap::new();
    for r in rooms_val {
        let id = j_str(r, "id");
        let prov = j_str(r, "provider");
        // B2: Skip non-Hue provider rooms/zones
        if id.is_empty() || id.contains(':') || (!prov.is_empty() && prov != "hue") {
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

    // 1. Box creations & metadata diffs (Hue rooms & zones only)
    for b in &doc.boxes {
        match b.hue_id.as_deref() {
            None | Some("") => {
                pending += 1;
            }
            Some(hid) => {
                if hid.contains(':') {
                    continue;
                }
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
                    // Zone membership diff (B1: same floor only; B2: Hue lights only)
                    if b.kind == "zone" {
                        let current_lights: HashSet<&str> = j_list(ex, "lights")
                            .iter()
                            .map(|l| j_str(l, "id"))
                            .filter(|id| !id.is_empty() && !id.contains(':'))
                            .collect();
                        let placed_inside: HashSet<&str> = doc
                            .lamps
                            .iter()
                            .filter(|(lid, pos)| {
                                !lid.contains(':')
                                    && pos.floor_id == b.floor_id
                                    && point_in_box(pos.x, pos.y, b)
                            })
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

    // 2. Room membership moves (Hue lights only, same floor only)
    let lights = extract_bridge_lights(stats);
    let mut moved_devices: HashSet<String> = HashSet::new();
    for light in &lights {
        if light.id.contains(':') {
            continue;
        }
        let Some(pos) = doc.lamps.get(&light.id) else { continue };
        let Some(target_box) = containing_room_box_on_floor(pos.x, pos.y, &pos.floor_id, &doc.boxes, &light.room_id) else {
            continue;
        };
        let to_room_id = target_box.hue_id.as_deref().unwrap_or("");
        if to_room_id.contains(':') {
            continue;
        }
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
            points: Vec::new(),
            floor_id: doc.active_floor_id.clone(),
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

/// Computes the clamped floating hover tooltip pill position and size for a lamp label.
/// Centers horizontally above the lamp, flipping below if it would clip near the top edge,
/// and clamps horizontally within `[4.0, (bounds.width - pill_w - 4.0).max(4.0)]`.
pub fn tooltip_rect(center: Point, bounds: Size, label: &str) -> (Point, Size) {
    let pill_w = (label.chars().count() as f32 * 6.8 + 18.0).clamp(44.0, 260.0);
    let pill_h = 24.0f32;
    let max_x = (bounds.width - pill_w - 4.0).max(4.0);
    let pill_x = (center.x - pill_w * 0.5).clamp(4.0, max_x);
    let above_y = center.y - LAMP_RADIUS_PX - 8.0 - pill_h;
    let pill_y = if above_y < 4.0 {
        (center.y + LAMP_RADIUS_PX + 8.0).min((bounds.height - pill_h - 4.0).max(4.0))
    } else {
        above_y
    };
    (Point::new(pill_x, pill_y), Size::new(pill_w, pill_h))
}

// ------------------------------------------------------------------------- Canvas Program

#[derive(Debug, Clone)]
enum WireEndpointRef {
    Lamp(String),
    Sensor(String),
    Box(String),
}

#[derive(Debug, Clone)]
struct WireRenderItem {
    rule_id: String,
    kind: WireKind,
    enabled: bool,
    from: (f64, f64),
    from_ref: WireEndpointRef,
    to: (f64, f64),
    to_ref: WireEndpointRef,
}

#[derive(Debug, Clone)]
enum DragOp {
    Lamp { id: String, grab_dx: f64, grab_dy: f64, preview: Option<(f64, f64)> },
    Sensor { id: String, grab_dx: f64, grab_dy: f64, preview: Option<(f64, f64)> },
    BoxMove { id: String, grab_dx: f64, grab_dy: f64, preview: Option<(f64, f64)> },
    BoxResize { id: String, handle: ResizeHandle, orig: FloorplanBox, preview: Option<(f64, f64, f64, f64)> },
    BoxVertex { box_id: String, index: usize, preview: Option<(f64, f64)> },
    PlannedMove { id: String, grab_dx: f64, grab_dy: f64, preview: Option<(f64, f64)> },
    PlannedStripVertex { id: String, index: usize, preview: Option<(f64, f64)> },
    Stamp { id: String, grab_dx: f64, grab_dy: f64, preview: Option<(f64, f64)> },
    Wire { from_kind: String, from_id: String, cursor: Point },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HoverTarget {
    Lamp(String),
    Sensor(String),
}

#[derive(Default)]
pub struct CanvasInteraction {
    drag: Option<DragOp>,
    hovered: Option<HoverTarget>,
}

struct FloorplanCanvasProgram<'a> {
    doc: &'a FloorplanDocument,
    lights: HashMap<String, BridgeLightInfo>,
    sensors: HashMap<String, HueSensor>,
    motion_areas: Vec<MotionArea>,
    wires: Vec<WireRenderItem>,
    schedule_badges: HashMap<String, String>,
    synth_running: bool,
    synth_lights: HashSet<String>,
    mode: CanvasMode,
    ambient: AmbientMode,
    wire_kind: WireKind,
    selection: Option<Selection>,
    armed_place_lamp: Option<String>,
    pending_wire_from: Option<(String, String)>,
    has_bg_image: bool,
    dialog_open: bool,
}

impl<'a> FloorplanCanvasProgram<'a> {
    fn from_state(state: &'a FloorplanState, stats: &'a Value) -> Self {
        let lights_vec = extract_bridge_lights(stats);
        let lights: HashMap<String, BridgeLightInfo> =
            lights_vec.into_iter().map(|l| (l.id.clone(), l)).collect();
        let sensors: HashMap<String, HueSensor> =
            parse_sensors(stats).into_iter().map(|s| (s.id.clone(), s)).collect();
        let motion_areas = parse_motion_areas(stats);

        // Schedule badges per box.id
        let mut schedule_badges = HashMap::new();
        for b in &state.doc.boxes {
            if let Some(hid) = b.hue_id.as_deref()
                && let Some(badge) = room_active_schedule_badge(stats, hid)
            {
                schedule_badges.insert(b.id.clone(), badge);
            }
        }

        // Resolve endpoint world coords for rules (placed lamp, sensor, or box center)
        let endpoint_pos = |kind: &str, id: &str| -> Option<((f64, f64), WireEndpointRef)> {
            if kind == "light" {
                state
                    .doc
                    .lamps
                    .get(id)
                    .map(|l| ((l.x, l.y), WireEndpointRef::Lamp(id.to_string())))
            } else if matches!(kind, "motion" | "contact" | "button") {
                state
                    .doc
                    .sensors
                    .get(id)
                    .map(|s| ((s.x, s.y), WireEndpointRef::Sensor(id.to_string())))
            } else if matches!(kind, "room" | "zone") {
                state
                    .doc
                    .boxes
                    .iter()
                    .find(|b| b.hue_id.as_deref() == Some(id))
                    .map(|b| ((b.x + b.w * 0.5, b.y + b.h * 0.5), WireEndpointRef::Box(b.id.clone())))
            } else if kind == "motion_area" {
                motion_areas
                    .iter()
                    .find(|a| a.id == id)
                    .and_then(|a| state.doc.boxes.iter().find(|b| b.hue_id.as_deref() == Some(a.room_id.as_str())))
                    .map(|b| ((b.x + b.w * 0.5, b.y + b.h * 0.5), WireEndpointRef::Box(b.id.clone())))
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
            let Some((from, from_ref)) =
                endpoint_pos(j_str(rule, "trigger_kind"), j_str(rule, "trigger_id"))
            else {
                continue;
            };
            let wkind = classify_rule_wire(rule);
            let enabled = j_bool(rule, "enabled");
            for t in j_list(rule, "targets") {
                if let Some((to, to_ref)) = endpoint_pos(j_str(t, "kind"), j_str(t, "id")) {
                    wires.push(WireRenderItem {
                        rule_id: rid.to_string(),
                        kind: wkind,
                        enabled,
                        from,
                        from_ref: from_ref.clone(),
                        to,
                        to_ref,
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

        let has_bg_image = state.bg_handle_cache.is_some();

        Self {
            doc: &state.doc,
            lights,
            sensors,
            motion_areas,
            wires,
            schedule_badges,
            synth_running,
            synth_lights,
            mode: state.mode,
            ambient: state.ambient,
            wire_kind: state.wire_kind,
            selection: state.selection.clone(),
            armed_place_lamp: state.armed_place_lamp.clone(),
            pending_wire_from: state.pending_wire_from.clone(),
            has_bg_image,
            dialog_open: state.has_dialog_open(),
        }
    }

    fn effective_lamp_pos(&self, lid: &str, fallback: (f64, f64), drag: Option<&DragOp>) -> (f64, f64) {
        if let Some(DragOp::Lamp { id, preview: Some(p), .. }) = drag
            && id == lid
        {
            return *p;
        }
        fallback
    }

    fn effective_sensor_pos(&self, sid: &str, fallback: (f64, f64), drag: Option<&DragOp>) -> (f64, f64) {
        if let Some(DragOp::Sensor { id, preview: Some(p), .. }) = drag
            && id == sid
        {
            return *p;
        }
        fallback
    }

    fn effective_box_rect(&self, b: &FloorplanBox, drag: Option<&DragOp>) -> (f64, f64, f64, f64) {
        match drag {
            Some(DragOp::BoxMove { id, preview: Some((px, py)), .. }) if id == &b.id => (*px, *py, b.w, b.h),
            Some(DragOp::BoxResize { id, preview: Some(rect), .. }) if id == &b.id => *rect,
            _ => (b.x, b.y, b.w, b.h),
        }
    }

    fn effective_stamp_pos(&self, st: &FloorplanStamp, drag: Option<&DragOp>) -> (f64, f64) {
        if let Some(DragOp::Stamp { id, preview: Some(p), .. }) = drag
            && id == &st.id
        {
            return *p;
        }
        (st.x, st.y)
    }

    fn effective_wire_endpoint(
        &self,
        fallback: (f64, f64),
        eref: &WireEndpointRef,
        drag: Option<&DragOp>,
    ) -> (f64, f64) {
        match eref {
            WireEndpointRef::Lamp(lid) => self.effective_lamp_pos(lid, fallback, drag),
            WireEndpointRef::Sensor(sid) => self.effective_sensor_pos(sid, fallback, drag),
            WireEndpointRef::Box(bid) => match drag {
                Some(DragOp::BoxMove { id, preview: Some((px, py)), .. }) if id == bid => {
                    if let Some(b) = self.doc.boxes.iter().find(|b| b.id == *bid) {
                        (*px + b.w * 0.5, *py + b.h * 0.5)
                    } else {
                        fallback
                    }
                }
                Some(DragOp::BoxResize { id, preview: Some((px, py, pw, ph)), .. }) if id == bid => {
                    (*px + *pw * 0.5, *py + *ph * 0.5)
                }
                _ => fallback,
            },
        }
    }

    fn active_floor(&self) -> &str {
        if self.doc.active_floor_id.is_empty() { "eg" } else { &self.doc.active_floor_id }
    }

    fn effective_box_points(&self, b: &FloorplanBox, drag: Option<&DragOp>) -> Vec<FloorplanPoint> {
        let mut pts = b.effective_points();
        match drag {
            Some(DragOp::BoxMove { id, preview: Some((px, py)), .. }) if id == &b.id => {
                let dx = *px - b.x;
                let dy = *py - b.y;
                for p in &mut pts {
                    p.x += dx;
                    p.y += dy;
                }
            }
            Some(DragOp::BoxResize { id, preview: Some((nx, ny, nw, nh)), .. }) if id == &b.id => {
                let ow = b.w.max(1.0);
                let oh = b.h.max(1.0);
                for p in &mut pts {
                    p.x = *nx + ((p.x - b.x) / ow) * *nw;
                    p.y = *ny + ((p.y - b.y) / oh) * *nh;
                }
            }
            Some(DragOp::BoxVertex { box_id, index, preview: Some((vx, vy)) }) if box_id == &b.id => {
                if let Some(p) = pts.get_mut(*index) {
                    p.x = *vx;
                    p.y = *vy;
                }
            }
            _ => {}
        }
        pts
    }

    fn hit_selected_box_vertex(&self, pt: Point, tf: CanvasTransform) -> Option<(String, usize)> {
        let Some(Selection::Box(ref bid)) = self.selection else { return None };
        let b = self.doc.boxes.iter().find(|b| b.id == *bid && b.floor_id == self.active_floor())?;
        if !b.is_polygon() {
            return None;
        }
        for (idx, p) in b.points.iter().enumerate() {
            let sp = tf.to_screen(p.x, p.y);
            if ((pt.x - sp.x).powi(2) + (pt.y - sp.y).powi(2)).sqrt() <= 9.5 {
                return Some((b.id.clone(), idx));
            }
        }
        None
    }

    fn hit_selected_box_midpoint(&self, pt: Point, tf: CanvasTransform) -> Option<(String, usize, f64, f64)> {
        let Some(Selection::Box(ref bid)) = self.selection else { return None };
        let b = self.doc.boxes.iter().find(|b| b.id == *bid && b.floor_id == self.active_floor())?;
        // C4: Only polygons use midpoint '+' corner-insert handles; rectangles use their 8 edge/corner resize handles
        if !b.is_polygon() {
            return None;
        }
        let pts = b.effective_points();
        let n = pts.len();
        if n < 3 || n >= 32 {
            return None;
        }
        for i in 0..n {
            let a = &pts[i];
            let c = &pts[(i + 1) % n];
            let mx = (a.x + c.x) * 0.5;
            let my = (a.y + c.y) * 0.5;
            let sp = tf.to_screen(mx, my);
            if ((pt.x - sp.x).powi(2) + (pt.y - sp.y).powi(2)).sqrt() <= 8.5 {
                return Some((b.id.clone(), i, mx, my));
            }
        }
        None
    }

    fn hit_planned_strip_vertex(&self, pt: Point, tf: CanvasTransform) -> Option<(String, usize)> {
        let floor = self.active_floor();
        for pl in &self.doc.planned_lights {
            if pl.floor_id != floor || pl.strip_points.len() < 2 {
                continue;
            }
            for (idx, p) in pl.strip_points.iter().enumerate() {
                let sp = tf.to_screen(p.x, p.y);
                if ((pt.x - sp.x).powi(2) + (pt.y - sp.y).powi(2)).sqrt() <= 8.5 {
                    return Some((pl.id.clone(), idx));
                }
            }
        }
        None
    }

    fn hit_planned_light(&self, pt: Point, tf: CanvasTransform) -> Option<String> {
        let floor = self.active_floor();
        let mut best: Option<(f32, String)> = None;
        for pl in &self.doc.planned_lights {
            if pl.floor_id != floor {
                continue;
            }
            let (ax, ay) = pl.anchor();
            let sp = tf.to_screen(ax, ay);
            let d = ((pt.x - sp.x).powi(2) + (pt.y - sp.y).powi(2)).sqrt();
            if d <= LAMP_RADIUS_PX + 5.0 && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, pl.id.clone()));
            }
        }
        best.map(|(_, id)| id)
    }

    fn hit_lamp(&self, pt: Point, tf: CanvasTransform) -> Option<String> {
        let floor = self.active_floor();
        let hit_r = LAMP_RADIUS_PX + 6.0;
        let mut best: Option<(f32, String)> = None;
        for (lid, pos) in &self.doc.lamps {
            if pos.floor_id != floor {
                continue;
            }
            let sp = tf.to_screen(pos.x, pos.y);
            let d = ((pt.x - sp.x).powi(2) + (pt.y - sp.y).powi(2)).sqrt();
            if d <= hit_r && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, lid.clone()));
            }
        }
        best.map(|(_, id)| id)
    }

    fn hit_sensor(&self, pt: Point, tf: CanvasTransform) -> Option<String> {
        let floor = self.active_floor();
        let hit_r = SENSOR_HALF_PX + 5.0;
        let mut best: Option<(f32, String)> = None;
        for (sid, pos) in &self.doc.sensors {
            if pos.floor_id != floor {
                continue;
            }
            let sp = tf.to_screen(pos.x, pos.y);
            if (pt.x - sp.x).abs() <= hit_r && (pt.y - sp.y).abs() <= hit_r {
                let d = ((pt.x - sp.x).powi(2) + (pt.y - sp.y).powi(2)).sqrt();
                if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                    best = Some((d, sid.clone()));
                }
            }
        }
        best.map(|(_, id)| id)
    }

    fn wire_source_screen_point(&self, kind: &str, id: &str, tf: CanvasTransform) -> Option<Point> {
        if kind == "light" {
            self.doc.lamps.get(id).map(|p| tf.to_screen(p.x, p.y))
        } else {
            self.doc.sensors.get(id).map(|p| tf.to_screen(p.x, p.y))
        }
    }

    fn hit_selected_box_handle(&self, pt: Point, tf: CanvasTransform) -> Option<(FloorplanBox, ResizeHandle)> {
        let Some(Selection::Box(ref bid)) = self.selection else { return None };
        let b = self.doc.boxes.iter().find(|b| b.id == *bid && b.floor_id == self.active_floor())?;
        if b.is_polygon() {
            return None;
        }
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
        let floor = self.active_floor();
        let (wx, wy) = tf.to_world(pt);
        for st in self.doc.stamps.iter().rev() {
            if st.floor_id != floor {
                continue;
            }
            if wx >= st.x - 6.0 && wx <= st.x + st.w + 6.0 && wy >= st.y - 6.0 && wy <= st.y + st.h + 6.0 {
                return Some(st.id.clone());
            }
        }
        None
    }

    fn hit_wire(&self, pt: Point, tf: CanvasTransform) -> Option<String> {
        if !(self.doc.show_wires || self.mode == CanvasMode::Wire) {
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
        let floor = self.active_floor();
        let (wx, wy) = tf.to_world(pt);
        let mut best: Option<(f64, String)> = None;
        for b in &self.doc.boxes {
            if b.floor_id != floor {
                continue;
            }
            if point_in_box(wx, wy, b) {
                let area = b.area_px();
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
        if self.dialog_open {
            if state.hovered.take().is_some() {
                return Some(Action::request_redraw());
            }
            return None;
        }
        let tf = CanvasTransform::fit(bounds.size(), self.doc.canvas.width, self.doc.canvas.height);

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                let pos = cursor.position_in(bounds)?;
                // Right-click a polygon corner vertex to delete it (when >3 corners)
                if let Some((box_id, index)) = self.hit_selected_box_vertex(pos, tf) {
                    return Some(Action::publish(MapMsg::RemoveBoxVertex { box_id, index }).and_capture());
                }
                if let Some(pid) = self.hit_planned_light(pos, tf) {
                    return Some(Action::publish(MapMsg::TogglePlannedLight(pid)).and_capture());
                }
                if let Some(lid) = self.hit_lamp(pos, tf) {
                    let is_on = self.lights.get(&lid).is_some_and(|i| i.on);
                    return Some(Action::publish(MapMsg::LampToggle(lid, !is_on)).and_capture());
                }
                if let Some(sid) = self.hit_sensor(pos, tf)
                    && let Some(sensor) = self.sensors.get(&sid)
                    && matches!(sensor.kind.as_str(), "motion" | "contact")
                {
                    return Some(
                        Action::publish(MapMsg::SensorToggle {
                            kind: sensor.kind.clone(),
                            id: sid,
                            enabled: !sensor.enabled,
                        })
                        .and_capture(),
                    );
                }
                None
            }

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

                // Wire mode: drag or 2-click from any placed lamp OR sensor to any placed lamp
                if self.mode == CanvasMode::Wire {
                    if let Some(lid) = self.hit_lamp(pos, tf) {
                        if let Some((ref fk, ref fid)) = self.pending_wire_from
                            && !(fk == "light" && fid == &lid)
                        {
                            return Some(
                                Action::publish(MapMsg::CreateWire {
                                    from_kind: fk.clone(),
                                    from_id: fid.clone(),
                                    to_id: lid,
                                })
                                .and_capture(),
                            );
                        }
                        state.hovered = None;
                        state.drag = Some(DragOp::Wire {
                            from_kind: "light".into(),
                            from_id: lid,
                            cursor: pos,
                        });
                        return Some(Action::request_redraw().and_capture());
                    }
                    if let Some(sid) = self.hit_sensor(pos, tf) {
                        let skind = self
                            .sensors
                            .get(&sid)
                            .map(|s| s.kind.clone())
                            .unwrap_or_else(|| "motion".into());
                        state.hovered = None;
                        state.drag = Some(DragOp::Wire {
                            from_kind: skind,
                            from_id: sid,
                            cursor: pos,
                        });
                        return Some(Action::request_redraw().and_capture());
                    }
                    if let Some(rid) = self.hit_wire(pos, tf) {
                        return Some(Action::publish(MapMsg::OpenRuleDialog(rid)).and_capture());
                    }
                    return Some(Action::publish(MapMsg::Select(None)).and_capture());
                }

                // C4: Select/Plan mode: resize handles BEFORE polygon midpoints so rectangle edge handles resize cleanly
                if let Some((orig, handle)) = self.hit_selected_box_handle(pos, tf) {
                    state.hovered = None;
                    state.drag = Some(DragOp::BoxResize { id: orig.id.clone(), handle, orig, preview: None });
                    return Some(Action::capture());
                }
                if let Some((box_id, index)) = self.hit_selected_box_vertex(pos, tf) {
                    state.hovered = None;
                    state.drag = Some(DragOp::BoxVertex { box_id, index, preview: None });
                    return Some(Action::capture());
                }
                if let Some((box_id, after_edge, mx, my)) = self.hit_selected_box_midpoint(pos, tf) {
                    return Some(Action::publish(MapMsg::InsertBoxVertex { box_id, after_edge, x: mx, y: my }).and_capture());
                }
                if let Some((pid, index)) = self.hit_planned_strip_vertex(pos, tf) {
                    state.hovered = None;
                    state.drag = Some(DragOp::PlannedStripVertex { id: pid.clone(), index, preview: None });
                    return Some(Action::publish(MapMsg::Select(Some(Selection::Planned(pid)))).and_capture());
                }
                if let Some(pid) = self.hit_planned_light(pos, tf) {
                    if let Some(pl) = self.doc.planned_lights.iter().find(|p| p.id == pid) {
                        let (wx, wy) = tf.to_world(pos);
                        let (ax, ay) = pl.anchor();
                        state.hovered = None;
                        state.drag = Some(DragOp::PlannedMove {
                            id: pid.clone(),
                            grab_dx: ax - wx,
                            grab_dy: ay - wy,
                            preview: None,
                        });
                    }
                    return Some(Action::publish(MapMsg::Select(Some(Selection::Planned(pid)))).and_capture());
                }
                if let Some(lid) = self.hit_lamp(pos, tf) {
                    if let Some(lpos) = self.doc.lamps.get(&lid) {
                        let (wx, wy) = tf.to_world(pos);
                        state.hovered = None;
                        state.drag = Some(DragOp::Lamp {
                            id: lid.clone(),
                            grab_dx: lpos.x - wx,
                            grab_dy: lpos.y - wy,
                            preview: None,
                        });
                    }
                    return Some(Action::publish(MapMsg::Select(Some(Selection::Lamp(lid)))).and_capture());
                }
                if let Some(sid) = self.hit_sensor(pos, tf) {
                    if let Some(spos) = self.doc.sensors.get(&sid) {
                        let (wx, wy) = tf.to_world(pos);
                        state.hovered = None;
                        state.drag = Some(DragOp::Sensor {
                            id: sid.clone(),
                            grab_dx: spos.x - wx,
                            grab_dy: spos.y - wy,
                            preview: None,
                        });
                    }
                    return Some(Action::publish(MapMsg::Select(Some(Selection::Sensor(sid)))).and_capture());
                }
                if let Some(rid) = self.hit_wire(pos, tf) {
                    return Some(Action::publish(MapMsg::OpenRuleDialog(rid)).and_capture());
                }
                if let Some(sid) = self.hit_stamp(pos, tf) {
                    if let Some(st) = self.doc.stamps.iter().find(|s| s.id == sid) {
                        let (wx, wy) = tf.to_world(pos);
                        state.hovered = None;
                        state.drag = Some(DragOp::Stamp {
                            id: sid.clone(),
                            grab_dx: st.x - wx,
                            grab_dy: st.y - wy,
                            preview: None,
                        });
                    }
                    return Some(Action::publish(MapMsg::Select(Some(Selection::Stamp(sid)))).and_capture());
                }
                if let Some(bid) = self.hit_box(pos, tf) {
                    if let Some(b) = self.doc.boxes.iter().find(|b| b.id == bid) {
                        let (wx, wy) = tf.to_world(pos);
                        state.hovered = None;
                        state.drag = Some(DragOp::BoxMove {
                            id: bid.clone(),
                            grab_dx: b.x - wx,
                            grab_dy: b.y - wy,
                            preview: None,
                        });
                    }
                    return Some(Action::publish(MapMsg::Select(Some(Selection::Box(bid)))).and_capture());
                }
                Some(Action::publish(MapMsg::Select(None)).and_capture())
            }

            Event::Mouse(mouse::Event::CursorLeft) => {
                if state.hovered.take().is_some() {
                    return Some(Action::request_redraw());
                }
                None
            }

            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if state.drag.is_none() {
                    let new_hover = cursor.position_in(bounds).and_then(|pos| {
                        self.hit_lamp(pos, tf)
                            .map(HoverTarget::Lamp)
                            .or_else(|| self.hit_sensor(pos, tf).map(HoverTarget::Sensor))
                    });
                    if new_hover != state.hovered {
                        state.hovered = new_hover;
                        return Some(Action::request_redraw());
                    }
                    return None;
                }
                let drag = state.drag.as_mut()?;
                let pos = cursor.position_from(bounds.position())?;
                let (wx, wy) = tf.to_world(pos);
                let grid = self.doc.canvas.grid;
                let snap = self.doc.canvas.snap;
                let max_w = self.doc.canvas.width;
                let max_h = self.doc.canvas.height;
                match drag {
                    DragOp::Lamp { grab_dx, grab_dy, preview, .. }
                    | DragOp::Sensor { grab_dx, grab_dy, preview, .. } => {
                        let x = snap_coord(wx + *grab_dx, grid, snap).clamp(12.0, max_w - 12.0);
                        let y = snap_coord(wy + *grab_dy, grid, snap).clamp(12.0, max_h - 12.0);
                        let next = Some((x, y));
                        if *preview != next {
                            *preview = next;
                            Some(Action::request_redraw().and_capture())
                        } else {
                            Some(Action::capture())
                        }
                    }
                    DragOp::BoxMove { id, grab_dx, grab_dy, preview } => {
                        let (bw, bh) = self
                            .doc
                            .boxes
                            .iter()
                            .find(|b| b.id == *id)
                            .map(|b| (b.w, b.h))
                            .unwrap_or((120.0, 120.0));
                        let x = snap_coord(wx + *grab_dx, grid, snap).clamp(0.0, (max_w - bw).max(0.0));
                        let y = snap_coord(wy + *grab_dy, grid, snap).clamp(0.0, (max_h - bh).max(0.0));
                        let next = Some((x, y));
                        if *preview != next {
                            *preview = next;
                            Some(Action::request_redraw().and_capture())
                        } else {
                            Some(Action::capture())
                        }
                    }
                    DragOp::BoxResize { handle, orig, preview, .. } => {
                        let rect = apply_resize(orig, *handle, wx, wy, grid, snap, max_w, max_h);
                        let next = Some(rect);
                        if *preview != next {
                            *preview = next;
                            Some(Action::request_redraw().and_capture())
                        } else {
                            Some(Action::capture())
                        }
                    }
                    DragOp::BoxVertex { preview, .. } => {
                        let x = snap_coord(wx, grid, snap).clamp(0.0, max_w);
                        let y = snap_coord(wy, grid, snap).clamp(0.0, max_h);
                        let next = Some((x, y));
                        if *preview != next {
                            *preview = next;
                            Some(Action::request_redraw().and_capture())
                        } else {
                            Some(Action::capture())
                        }
                    }
                    DragOp::PlannedMove { grab_dx, grab_dy, preview, .. } => {
                        let x = snap_coord(wx + *grab_dx, grid, snap).clamp(12.0, max_w - 12.0);
                        let y = snap_coord(wy + *grab_dy, grid, snap).clamp(12.0, max_h - 12.0);
                        let next = Some((x, y));
                        if *preview != next {
                            *preview = next;
                            Some(Action::request_redraw().and_capture())
                        } else {
                            Some(Action::capture())
                        }
                    }
                    DragOp::PlannedStripVertex { preview, .. } => {
                        let x = snap_coord(wx, grid, snap).clamp(12.0, max_w - 12.0);
                        let y = snap_coord(wy, grid, snap).clamp(12.0, max_h - 12.0);
                        let next = Some((x, y));
                        if *preview != next {
                            *preview = next;
                            Some(Action::request_redraw().and_capture())
                        } else {
                            Some(Action::capture())
                        }
                    }
                    DragOp::Stamp { grab_dx, grab_dy, preview, .. } => {
                        let x = snap_coord(wx + *grab_dx, grid, snap).clamp(0.0, max_w - 16.0);
                        let y = snap_coord(wy + *grab_dy, grid, snap).clamp(0.0, max_h - 16.0);
                        let next = Some((x, y));
                        if *preview != next {
                            *preview = next;
                            Some(Action::request_redraw().and_capture())
                        } else {
                            Some(Action::capture())
                        }
                    }
                    DragOp::Wire { cursor: c, .. } => {
                        *c = pos;
                        Some(Action::request_redraw().and_capture())
                    }
                }
            }

            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let drag = state.drag.take()?;
                let maybe_pos = cursor.position_from(bounds.position());
                let grid = self.doc.canvas.grid;
                let snap = self.doc.canvas.snap;
                let max_w = self.doc.canvas.width;
                let max_h = self.doc.canvas.height;
                match drag {
                    DragOp::Lamp { id, grab_dx, grab_dy, preview } => {
                        let (x, y) = if let Some(pos) = maybe_pos {
                            let (wx, wy) = tf.to_world(pos);
                            (
                                snap_coord(wx + grab_dx, grid, snap).clamp(12.0, max_w - 12.0),
                                snap_coord(wy + grab_dy, grid, snap).clamp(12.0, max_h - 12.0),
                            )
                        } else if let Some(p) = preview {
                            p
                        } else if let Some(lpos) = self.doc.lamps.get(&id) {
                            (lpos.x, lpos.y)
                        } else {
                            return Some(Action::request_redraw().and_capture());
                        };
                        Some(Action::publish(MapMsg::MoveLamp { id, x, y, commit: true }).and_capture())
                    }
                    DragOp::Sensor { id, grab_dx, grab_dy, preview } => {
                        let (x, y) = if let Some(pos) = maybe_pos {
                            let (wx, wy) = tf.to_world(pos);
                            (
                                snap_coord(wx + grab_dx, grid, snap).clamp(12.0, max_w - 12.0),
                                snap_coord(wy + grab_dy, grid, snap).clamp(12.0, max_h - 12.0),
                            )
                        } else if let Some(p) = preview {
                            p
                        } else if let Some(spos) = self.doc.sensors.get(&id) {
                            (spos.x, spos.y)
                        } else {
                            return Some(Action::request_redraw().and_capture());
                        };
                        Some(Action::publish(MapMsg::MoveSensor { id, x, y, commit: true }).and_capture())
                    }
                    DragOp::BoxMove { id, grab_dx, grab_dy, preview } => {
                        let (bw, bh) = self
                            .doc
                            .boxes
                            .iter()
                            .find(|b| b.id == id)
                            .map(|b| (b.w, b.h))
                            .unwrap_or((120.0, 120.0));
                        let (x, y) = if let Some(pos) = maybe_pos {
                            let (wx, wy) = tf.to_world(pos);
                            (
                                snap_coord(wx + grab_dx, grid, snap).clamp(0.0, (max_w - bw).max(0.0)),
                                snap_coord(wy + grab_dy, grid, snap).clamp(0.0, (max_h - bh).max(0.0)),
                            )
                        } else if let Some(p) = preview {
                            p
                        } else if let Some(b) = self.doc.boxes.iter().find(|b| b.id == id) {
                            (b.x, b.y)
                        } else {
                            return Some(Action::request_redraw().and_capture());
                        };
                        Some(Action::publish(MapMsg::MoveBox { id, x, y, commit: true }).and_capture())
                    }
                    DragOp::BoxResize { id, handle, orig, preview } => {
                        let (x, y, w, h) = if let Some(pos) = maybe_pos {
                            let (wx, wy) = tf.to_world(pos);
                            apply_resize(&orig, handle, wx, wy, grid, snap, max_w, max_h)
                        } else if let Some(rect) = preview {
                            rect
                        } else {
                            (orig.x, orig.y, orig.w, orig.h)
                        };
                        Some(Action::publish(MapMsg::ResizeBox { id, x, y, w, h, commit: true }).and_capture())
                    }
                    DragOp::BoxVertex { box_id, index, preview } => {
                        let (x, y) = if let Some(pos) = maybe_pos {
                            let (wx, wy) = tf.to_world(pos);
                            (snap_coord(wx, grid, snap).clamp(0.0, max_w), snap_coord(wy, grid, snap).clamp(0.0, max_h))
                        } else if let Some(p) = preview {
                            p
                        } else {
                            return Some(Action::request_redraw().and_capture());
                        };
                        Some(Action::publish(MapMsg::MoveBoxVertex { box_id, index, x, y, commit: true }).and_capture())
                    }
                    DragOp::PlannedMove { id, grab_dx, grab_dy, preview } => {
                        let (x, y) = if let Some(pos) = maybe_pos {
                            let (wx, wy) = tf.to_world(pos);
                            (
                                snap_coord(wx + grab_dx, grid, snap).clamp(12.0, max_w - 12.0),
                                snap_coord(wy + grab_dy, grid, snap).clamp(12.0, max_h - 12.0),
                            )
                        } else if let Some(p) = preview {
                            p
                        } else {
                            return Some(Action::request_redraw().and_capture());
                        };
                        Some(Action::publish(MapMsg::MovePlannedLight { id, x, y, commit: true }).and_capture())
                    }
                    DragOp::PlannedStripVertex { id, index, preview } => {
                        let (x, y) = if let Some(pos) = maybe_pos {
                            let (wx, wy) = tf.to_world(pos);
                            (snap_coord(wx, grid, snap).clamp(12.0, max_w - 12.0), snap_coord(wy, grid, snap).clamp(12.0, max_h - 12.0))
                        } else if let Some(p) = preview {
                            p
                        } else {
                            return Some(Action::request_redraw().and_capture());
                        };
                        Some(Action::publish(MapMsg::MovePlannedStripVertex { id, index, x, y, commit: true }).and_capture())
                    }
                    DragOp::Stamp { id, grab_dx, grab_dy, preview } => {
                        let (x, y) = if let Some(pos) = maybe_pos {
                            let (wx, wy) = tf.to_world(pos);
                            (
                                snap_coord(wx + grab_dx, grid, snap).clamp(0.0, max_w - 16.0),
                                snap_coord(wy + grab_dy, grid, snap).clamp(0.0, max_h - 16.0),
                            )
                        } else if let Some(p) = preview {
                            p
                        } else if let Some(st) = self.doc.stamps.iter().find(|s| s.id == id) {
                            (st.x, st.y)
                        } else {
                            return Some(Action::request_redraw().and_capture());
                        };
                        Some(Action::publish(MapMsg::MoveStamp { id, x, y, commit: true }).and_capture())
                    }
                    DragOp::Wire { from_kind, from_id, .. } => {
                        if let Some(pos) = maybe_pos
                            && let Some(to_id) = self.hit_lamp(pos, tf)
                            && !(from_kind == "light" && to_id == from_id)
                        {
                            return Some(
                                Action::publish(MapMsg::CreateWire {
                                    from_kind,
                                    from_id,
                                    to_id,
                                })
                                .and_capture(),
                            );
                        }
                        // Clicked single lamp/sensor in wire mode -> select it as pending source
                        if from_kind == "light" {
                            Some(Action::publish(MapMsg::LampStartWire(from_id)).and_capture())
                        } else {
                            Some(
                                Action::publish(MapMsg::SensorStartWire {
                                    kind: from_kind,
                                    id: from_id,
                                })
                                .and_capture(),
                            )
                        }
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
        let drag = state.drag.as_ref();
        let floor = self.active_floor();

        // 0. Optional Day/Dusk/Night ambient dim simulation
        let dim = self.ambient.dim_alpha();
        if dim > 0.0 {
            frame.fill_rectangle(Point::ORIGIN, bounds.size(), with_alpha(Color::BLACK, dim));
        }

        // 1. Subtle architectural canvas surface & batched dot grid
        if !self.has_bg_image {
            let world_tl = tf.to_screen(0.0, 0.0);
            let world_sz = Size::new(
                self.doc.canvas.width as f32 * tf.scale,
                self.doc.canvas.height as f32 * tf.scale,
            );
            frame.fill_rectangle(world_tl, world_sz, with_alpha(on_bg, 0.03));

            let step = (self.doc.canvas.grid.max(20) as f64) * 2.0;
            let dots = CanvasPath::new(|b| {
                let mut gx = step;
                while gx < self.doc.canvas.width {
                    let mut gy = step;
                    while gy < self.doc.canvas.height {
                        let p = tf.to_screen(gx, gy);
                        b.rectangle(Point::new(p.x - 0.8, p.y - 0.8), Size::new(1.6, 1.6));
                        gy += step;
                    }
                    gx += step;
                }
            });
            frame.fill(&dots, with_alpha(on_bg, 0.14));
        }

        // 2. Polygon & Rectangular Room / Zone boxes on active floor
        for b in &self.doc.boxes {
            if b.floor_id != floor {
                continue;
            }
            let (bx, by, bw, bh) = self.effective_box_rect(b, drag);
            let pts = self.effective_box_points(b, drag);
            let tl = tf.to_screen(bx, by);
            let sz = Size::new((bw as f32) * tf.scale, (bh as f32) * tf.scale);
            let is_zone = b.kind == "zone";
            let selected = matches!(&self.selection, Some(Selection::Box(id)) if id == &b.id);

            // Ambient room tint + Lux calculation from active real + planned lights inside box
            let mut r_sum = 0.0f32;
            let mut g_sum = 0.0f32;
            let mut b_sum = 0.0f32;
            let mut lit_count = 0u32;
            let mut total_lumens = 0.0f64;
            for (lid, lpos) in &self.doc.lamps {
                if lpos.floor_id != floor {
                    continue;
                }
                let (lx, ly) = self.effective_lamp_pos(lid, (lpos.x, lpos.y), drag);
                if b.contains_point(lx, ly)
                    && let Some(info) = self.lights.get(lid)
                    && info.on
                {
                    let c = parse_hex_color(&info.color);
                    r_sum += c.r;
                    g_sum += c.g;
                    b_sum += c.b;
                    lit_count += 1;
                    total_lumens += 800.0 * (info.bri / 100.0).clamp(0.05, 1.0);
                }
            }
            for pl in &self.doc.planned_lights {
                if pl.floor_id != floor || !pl.on {
                    continue;
                }
                let (px, py) = pl.anchor();
                if b.contains_point(px, py) {
                    let c = parse_hex_color(&pl.color);
                    r_sum += c.r;
                    g_sum += c.g;
                    b_sum += c.b;
                    lit_count += 1;
                    total_lumens += pl.lumens as f64;
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

            let room_path = if b.is_polygon() {
                CanvasPath::new(|builder| {
                    for (i, p) in pts.iter().enumerate() {
                        let sp = tf.to_screen(p.x, p.y);
                        if i == 0 { builder.move_to(sp); } else { builder.line_to(sp); }
                    }
                    builder.close();
                })
            } else {
                CanvasPath::rounded_rectangle(tl, sz, 6.0.into())
            };
            frame.fill(&room_path, fill_color);

            let room_motion_area = b.hue_id.as_deref().and_then(|hid| {
                self.motion_areas.iter().find(|a| a.room_id == hid)
            });
            let motion_active = room_motion_area.is_some_and(|a| a.enabled && a.motion);
            if motion_active {
                frame.stroke(
                    &room_path,
                    Stroke::default()
                        .with_color(with_alpha(MOTION_ACTIVE_COLOR, 0.45))
                        .with_width(4.0),
                );
            }

            let border_color = if selected {
                accent
            } else if motion_active {
                MOTION_ACTIVE_COLOR
            } else if is_zone {
                with_alpha(accent, 0.55)
            } else {
                with_alpha(on_bg, 0.36)
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
                    .with_width(if selected || motion_active { 2.4 } else { 1.6 })
            };
            frame.stroke(&room_path, stroke);

            // Header label (archetype glyph + name + m² + Plan-mode lx + badges)
            let area_m2 = b.area_m2(self.doc.px_per_meter).max(0.5);
            let mut label = format!("{} {} · {:.0} m²", archetype_glyph(&b.archetype), b.name, area_m2);
            if self.mode == CanvasMode::Plan {
                let lux = (total_lumens / area_m2).round() as u32;
                let target = target_lux_for_archetype(&b.archetype);
                label.push_str(&format!(" · {lux}/{target} lx"));
            }
            if b.hue_id.is_none() {
                label.push_str(" •");
            }
            if b.entertainment_area_id.is_some() {
                label.push_str(" ♪");
            }
            if let Some(area) = room_motion_area {
                if area.enabled && area.motion {
                    label.push_str(" ◉");
                } else if area.enabled {
                    label.push_str(" ◎");
                }
            }
            if let Some(badge) = self.schedule_badges.get(&b.id) {
                label.push_str(&format!("  {badge}"));
            }
            // Place label inside the first visible upper edge
            let label_pos = if b.is_polygon() {
                let top_pt = pts.iter().min_by(|a, c| a.y.partial_cmp(&c.y).unwrap_or(std::cmp::Ordering::Equal));
                let lp = top_pt.map(|p| tf.to_screen(p.x, p.y)).unwrap_or(tl);
                Point::new((lp.x + 8.0).max(tl.x + 8.0), lp.y + 8.0)
            } else {
                Point::new(tl.x + 8.0, tl.y + 6.0)
            };
            frame.fill_text(CanvasText {
                content: label,
                position: label_pos,
                color: with_alpha(on_bg, if selected { 0.98 } else { 0.82 }),
                size: Pixels(11.0),
                ..Default::default()
            });

            // Selected room handles: polygon corner circles + wall midpoint '+' split handles + wall lengths
            if selected {
                let n = pts.len();
                for i in 0..n {
                    let a = &pts[i];
                    let c = &pts[(i + 1) % n];
                    let seg_px = ((c.x - a.x).powi(2) + (c.y - a.y).powi(2)).sqrt();
                    let mid = tf.to_screen((a.x + c.x) * 0.5, (a.y + c.y) * 0.5);
                    if seg_px * (tf.scale as f64) >= 42.0 {
                        // Wall midpoint '+' split handle
                        let mp = CanvasPath::circle(mid, 6.0);
                        frame.fill(&mp, accent);
                        frame.stroke(&mp, Stroke::default().with_color(Color::WHITE).with_width(1.1));
                        frame.fill_text(CanvasText {
                            content: "+".into(),
                            position: Point::new(mid.x - 3.3, mid.y - 6.0),
                            color: Color::WHITE,
                            size: Pixels(10.0),
                            ..Default::default()
                        });
                        // Wall length in meters
                        let len_m = seg_px / self.doc.px_per_meter.max(10.0);
                        frame.fill_text(CanvasText {
                            content: format!("{len_m:.1}m"),
                            position: Point::new(mid.x + 8.0, mid.y - 5.0),
                            color: with_alpha(on_bg, 0.72),
                            size: Pixels(9.5),
                            ..Default::default()
                        });
                    }
                }
                if b.is_polygon() {
                    for p in &pts {
                        let hp = tf.to_screen(p.x, p.y);
                        let circ = CanvasPath::circle(hp, 5.5);
                        frame.fill(&circ, accent);
                        frame.stroke(&circ, Stroke::default().with_color(Color::WHITE).with_width(1.5));
                    }
                } else {
                    for h in ResizeHandle::ALL {
                        let (hx, hy) = h.world_point_rect(bx, by, bw, bh);
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
        }

        // 3. Minimalist architectural furniture stamps on active floor
        if self.doc.show_furniture {
            for st in &self.doc.stamps {
                if st.floor_id != floor {
                    continue;
                }
                let (sx, sy) = self.effective_stamp_pos(st, drag);
                let selected = matches!(&self.selection, Some(Selection::Stamp(id)) if id == &st.id);
                draw_stamp(&mut frame, st, sx, sy, tf, on_bg, accent, selected);
            }
        }

        // 4. Live radial color/brightness halos around placed real & planned lamps
        let boost = self.ambient.glow_boost();
        if self.doc.show_glows || self.mode == CanvasMode::Plan {
            for (lid, lpos) in &self.doc.lamps {
                if lpos.floor_id != floor {
                    continue;
                }
                let Some(info) = self.lights.get(lid) else { continue };
                if !info.on {
                    continue;
                }
                let (lx, ly) = self.effective_lamp_pos(lid, (lpos.x, lpos.y), drag);
                let center = tf.to_screen(lx, ly);
                let c = parse_hex_color(&info.color);
                let bri_norm = (info.bri / 100.0).clamp(0.12, 1.0) as f32;
                let max_r = (LAMP_RADIUS_PX + 18.0 + 42.0 * bri_norm) * (tf.scale.clamp(0.55, 1.4));
                for step_i in (1..=3).rev() {
                    let frac = step_i as f32 / 3.0;
                    let r = max_r * frac;
                    let alpha = ((1.0 - frac * 0.72) * 0.22 * bri_norm * boost).min(0.85);
                    frame.fill(&CanvasPath::circle(center, r), with_alpha(c, alpha));
                }
            }

            // Planned lights & bendable LED lightstrips glow + ribbons
            for pl in &self.doc.planned_lights {
                if pl.floor_id != floor {
                    continue;
                }
                let c = parse_hex_color(&pl.color);
                let lumen_f = ((pl.lumens as f32) / 1400.0).clamp(0.35, 1.25);
                let max_r = ((pl.beam_radius * self.doc.px_per_meter) as f32 * tf.scale).clamp(24.0, 260.0);

                if pl.strip_points.len() >= 2 {
                    let pts: Vec<Point> = pl.strip_points.iter().enumerate().map(|(i, p)| {
                        if let Some(DragOp::PlannedStripVertex { id, index, preview: Some((vx, vy)) }) = drag
                            && id == &pl.id && *index == i
                        {
                            tf.to_screen(*vx, *vy)
                        } else if let Some(DragOp::PlannedMove { id, preview: Some((mx, my)), .. }) = drag
                            && id == &pl.id
                        {
                            let (ax, ay) = pl.anchor();
                            tf.to_screen(p.x + (*mx - ax), p.y + (*my - ay))
                        } else {
                            tf.to_screen(p.x, p.y)
                        }
                    }).collect();
                    let strip_path = CanvasPath::new(|builder| {
                        for (i, sp) in pts.iter().enumerate() {
                            if i == 0 { builder.move_to(*sp); } else { builder.line_to(*sp); }
                        }
                    });
                    if pl.on {
                        frame.stroke(
                            &strip_path,
                            Stroke::default()
                                .with_color(with_alpha(c, (0.22 * lumen_f * boost).min(0.65)))
                                .with_width((max_r * 0.36).max(16.0))
                                .with_line_cap(LineCap::Round)
                                .with_line_join(LineJoin::Round),
                        );
                        frame.stroke(
                            &strip_path,
                            Stroke::default()
                                .with_color(with_alpha(c, (0.55 * lumen_f * boost).min(0.9)))
                                .with_width(8.0)
                                .with_line_cap(LineCap::Round)
                                .with_line_join(LineJoin::Round),
                        );
                    }
                    frame.stroke(
                        &strip_path,
                        Stroke {
                            style: Style::Solid(if pl.on { c } else { with_alpha(on_bg, 0.5) }),
                            width: 2.6,
                            line_dash: LineDash { segments: &[6.0, 4.0], offset: 0 },
                            line_cap: LineCap::Round,
                            line_join: LineJoin::Round,
                        },
                    );
                    for sp in &pts {
                        let dot = CanvasPath::circle(*sp, 4.5);
                        frame.fill(&dot, accent);
                        frame.stroke(&dot, Stroke::default().with_color(Color::WHITE).with_width(1.2));
                    }
                } else if pl.on {
                    let (ax, ay) = match drag {
                        Some(DragOp::PlannedMove { id, preview: Some(p), .. }) if id == &pl.id => *p,
                        _ => (pl.x, pl.y),
                    };
                    let center = tf.to_screen(ax, ay);
                    for step_i in (1..=3).rev() {
                        let frac = step_i as f32 / 3.0;
                        let r = max_r * frac;
                        let alpha = ((1.0 - frac * 0.72) * 0.24 * lumen_f * boost).min(0.85);
                        frame.fill(&CanvasPath::circle(center, r), with_alpha(c, alpha));
                    }
                }
            }
        }

        // 5. Typed Bezier dependency wires
        if self.doc.show_wires || self.mode == CanvasMode::Wire {
            for w in &self.wires {
                let from_w = self.effective_wire_endpoint(w.from, &w.from_ref, drag);
                let to_w = self.effective_wire_endpoint(w.to, &w.to_ref, drag);
                let a = tf.to_screen(from_w.0, from_w.1);
                let b = tf.to_screen(to_w.0, to_w.1);
                draw_wire(&mut frame, a, b, w.kind, w.enabled, false);
            }
        }

        // Live preview wire while dragging in Wire mode
        if let Some(DragOp::Wire { from_kind, from_id, cursor: cur }) = drag
            && let Some(a) = self.wire_source_screen_point(from_kind, from_id, tf)
        {
            draw_wire(&mut frame, a, *cur, self.wire_kind, true, true);
        } else if self.mode == CanvasMode::Wire
            && let Some((ref fk, ref fid)) = self.pending_wire_from
            && let Some(a) = self.wire_source_screen_point(fk, fid, tf)
            && let Some(cur) = cursor.position_in(bounds)
        {
            draw_wire(&mut frame, a, cur, self.wire_kind, true, false);
        }

        // 6. Placed lamp nodes with selectable vector icons (+ Bridge Pro MotionAware badge) on active floor
        for (lid, lpos) in &self.doc.lamps {
            if lpos.floor_id != floor {
                continue;
            }
            let (lx, ly) = self.effective_lamp_pos(lid, (lpos.x, lpos.y), drag);
            let center = tf.to_screen(lx, ly);
            let info = self.lights.get(lid);
            let is_on = info.is_some_and(|i| i.on);
            let reachable = info.is_none_or(|i| i.reachable);
            let selected = matches!(&self.selection, Some(Selection::Lamp(id)) if id == lid);
            let is_wire_src = matches!(
                &self.pending_wire_from,
                Some((fk, fid)) if fk == "light" && fid == lid
            );
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

            // Crisp vector lamp icon inside the node (matches lamp_icon_svg 1:1)
            let ink = if is_on { contrast_ink(fill) } else { with_alpha(on_bg, 0.90) };
            draw_lamp_icon(&mut frame, center, LAMP_RADIUS_PX * 0.64, &lpos.icon, ink);

            // Bridge Pro MotionAware radar badge at top-right corner of lamp circle
            if info.is_some_and(|i| i.motion_aware) {
                let active = info.is_some_and(|i| i.motion_active);
                let badge_center = Point::new(
                    center.x + LAMP_RADIUS_PX * 0.72,
                    center.y - LAMP_RADIUS_PX * 0.72,
                );
                let badge_r = 5.5f32;
                if active {
                    frame.stroke(
                        &CanvasPath::circle(badge_center, badge_r + 2.8),
                        Stroke::default()
                            .with_color(with_alpha(MOTION_ACTIVE_COLOR, 0.55))
                            .with_width(1.6),
                    );
                }
                let badge_col = if active { MOTION_ACTIVE_COLOR } else { MOTION_IDLE_COLOR };
                let badge_path = CanvasPath::circle(badge_center, badge_r);
                frame.fill(&badge_path, badge_col);
                frame.stroke(
                    &badge_path,
                    Stroke::default().with_color(Color::WHITE).with_width(1.1),
                );
                frame.fill(&CanvasPath::circle(badge_center, 1.4), Color::WHITE);
                let wave = CanvasPath::new(|b| {
                    b.arc(canvas::path::Arc {
                        center: badge_center,
                        radius: 3.2,
                        start_angle: Radians(-0.75),
                        end_angle: Radians(0.75),
                    });
                });
                frame.stroke(
                    &wave,
                    Stroke::default()
                        .with_color(Color::WHITE)
                        .with_width(1.1)
                        .with_line_cap(LineCap::Round),
                );
            }
        }

        // 6b. Placed physical Hue sensors & switches (28x28 rounded-square nodes) on active floor
        for (sid, spos) in &self.doc.sensors {
            if spos.floor_id != floor {
                continue;
            }
            let (sx, sy) = self.effective_sensor_pos(sid, (spos.x, spos.y), drag);
            let center = tf.to_screen(sx, sy);
            let sensor = self.sensors.get(sid);
            let kind = sensor.map(|s| s.kind.as_str()).unwrap_or("motion");
            let enabled = sensor.is_none_or(|s| s.enabled);
            let is_active = match kind {
                "motion" => enabled && sensor.is_some_and(|s| s.motion),
                "contact" => enabled && sensor.is_some_and(|s| s.open),
                _ => false,
            };
            let selected = matches!(&self.selection, Some(Selection::Sensor(id)) if id == sid);
            let is_wire_src = matches!(
                &self.pending_wire_from,
                Some((fk, fid)) if fk != "light" && fid == sid
            );

            // Soft live glow for active motion or open contact
            if is_active {
                let glow_col = if kind == "contact" { CONTACT_OPEN_COLOR } else { MOTION_ACTIVE_COLOR };
                for (dr, alpha) in [(9.0f32, 0.16f32), (5.0f32, 0.28f32)] {
                    let gtl = Point::new(center.x - SENSOR_HALF_PX - dr, center.y - SENSOR_HALF_PX - dr);
                    let gsz = Size::new((SENSOR_HALF_PX + dr) * 2.0, (SENSOR_HALF_PX + dr) * 2.0);
                    frame.fill(
                        &CanvasPath::rounded_rectangle(gtl, gsz, (7.0 + dr * 0.4).into()),
                        with_alpha(glow_col, alpha),
                    );
                }
            }

            if selected || is_wire_src {
                let ring_col = if is_wire_src { self.wire_kind.color() } else { accent };
                let r_pad = 3.5f32;
                let rtl = Point::new(center.x - SENSOR_HALF_PX - r_pad, center.y - SENSOR_HALF_PX - r_pad);
                let rsz = Size::new((SENSOR_HALF_PX + r_pad) * 2.0, (SENSOR_HALF_PX + r_pad) * 2.0);
                frame.stroke(
                    &CanvasPath::rounded_rectangle(rtl, rsz, 9.0.into()),
                    Stroke::default().with_color(ring_col).with_width(2.2),
                );
            }

            let (fill, border_col, ink) = match kind {
                "motion" => {
                    if !enabled {
                        (with_alpha(on_bg, 0.14), with_alpha(on_bg, 0.40), with_alpha(on_bg, 0.65))
                    } else if is_active {
                        (MOTION_ACTIVE_COLOR, Color::WHITE, Color::WHITE)
                    } else {
                        (Color::from_rgb8(0x16, 0x4E, 0x63), with_alpha(MOTION_IDLE_COLOR, 0.90), Color::WHITE)
                    }
                }
                "contact" => {
                    if !enabled {
                        (with_alpha(on_bg, 0.14), with_alpha(on_bg, 0.40), with_alpha(on_bg, 0.65))
                    } else if is_active {
                        (CONTACT_OPEN_COLOR, Color::WHITE, contrast_ink(CONTACT_OPEN_COLOR))
                    } else {
                        (CONTACT_CLOSED_COLOR, Color::WHITE, Color::WHITE)
                    }
                }
                _ => (BUTTON_COLOR, Color::WHITE, Color::WHITE),
            };

            let tl = Point::new(center.x - SENSOR_HALF_PX, center.y - SENSOR_HALF_PX);
            let sz = Size::new(SENSOR_HALF_PX * 2.0, SENSOR_HALF_PX * 2.0);
            let sq = CanvasPath::rounded_rectangle(tl, sz, 7.0.into());
            frame.fill(&sq, fill);
            frame.stroke(
                &sq,
                Stroke::default()
                    .with_color(border_col)
                    .with_width(if is_active || kind == "button" { 1.6 } else { 1.3 }),
            );
            draw_sensor_icon(&mut frame, center, 8.5, kind, ink, is_active);
        }

        // 6c. Planned Future Lighting fixtures (dashed blueprint nodes + lumen tags) on active floor
        let plan_accent = Color::from_rgb8(0x38, 0xBD, 0xF8);
        for pl in &self.doc.planned_lights {
            if pl.floor_id != floor {
                continue;
            }
            let selected = matches!(&self.selection, Some(Selection::Planned(id)) if id == &pl.id);
            let col = parse_hex_color(&pl.color);
            if pl.is_strip() {
                // Draw draggable strip anchor badge at midpoint of strip
                let pts: Vec<FloorplanPoint> = if let Some(DragOp::PlannedStripVertex {
                    id,
                    index,
                    preview: Some((curr_x, curr_y)),
                }) = drag
                    && id == &pl.id
                {
                    let mut p = pl.strip_points.clone();
                    if *index < p.len() {
                        p[*index] = FloorplanPoint { x: *curr_x, y: *curr_y };
                    }
                    p
                } else {
                    pl.strip_points.clone()
                };
                if pts.len() >= 2 {
                    let mid = &pts[pts.len() / 2];
                    let mc = tf.to_screen(mid.x, mid.y);
                    let tag = format!("{} lm", pl.lumens);
                    frame.fill_text(CanvasText {
                        content: tag,
                        position: Point::new(mc.x, mc.y - 12.0),
                        color: if selected { accent } else { plan_accent },
                        size: Pixels(10.0),
                        align_x: alignment::Horizontal::Center.into(),
                        align_y: alignment::Vertical::Bottom.into(),
                        ..Default::default()
                    });
                }
                continue;
            }

            let (px, py) = if let Some(DragOp::PlannedMove {
                id,
                preview: Some((curr_x, curr_y)),
                ..
            }) = drag
                && id == &pl.id
            {
                (*curr_x, *curr_y)
            } else {
                (pl.x, pl.y)
            };
            let center = tf.to_screen(px, py);

            if selected {
                frame.stroke(
                    &CanvasPath::circle(center, LAMP_RADIUS_PX + 5.0),
                    Stroke::default().with_color(accent).with_width(2.2),
                );
            }

            let node_path = CanvasPath::circle(center, LAMP_RADIUS_PX);
            let fill = if pl.on {
                with_alpha(col, 0.85)
            } else {
                with_alpha(on_bg, 0.14)
            };
            frame.fill(&node_path, fill);
            frame.stroke(
                &node_path,
                Stroke {
                    style: Style::Solid(if selected { accent } else { plan_accent }),
                    width: if selected { 2.2 } else { 1.8 },
                    line_dash: LineDash { segments: &[4.0, 3.0], offset: 0 },
                    ..Default::default()
                },
            );

            let ink = if pl.on { contrast_ink(col) } else { with_alpha(on_bg, 0.90) };
            draw_lamp_icon(&mut frame, center, LAMP_RADIUS_PX * 0.62, &pl.icon, ink);

            // Blueprint lumen label underneath node
            frame.fill_text(CanvasText {
                content: format!("{} lm", pl.lumens),
                position: Point::new(center.x, center.y + LAMP_RADIUS_PX + 3.0),
                color: if selected { accent } else { with_alpha(plan_accent, 0.92) },
                size: Pixels(10.0),
                align_x: alignment::Horizontal::Center.into(),
                align_y: alignment::Vertical::Top.into(),
                ..Default::default()
            });
        }

        // 7. Hover tooltip pill (lamp or sensor name only, floats on top of everything)
        if state.drag.is_none()
            && let Some(ref target) = state.hovered
        {
            let tooltip_info: Option<(Point, &str)> = match target {
                HoverTarget::Lamp(hid) => self
                    .doc
                    .lamps
                    .get(hid)
                    .and_then(|lpos| {
                        self.lights
                            .get(hid)
                            .map(|i| i.name.as_str())
                            .filter(|n| !n.is_empty())
                            .map(|n| (tf.to_screen(lpos.x, lpos.y), n))
                    }),
                HoverTarget::Sensor(sid) => self
                    .doc
                    .sensors
                    .get(sid)
                    .and_then(|spos| {
                        self.sensors
                            .get(sid)
                            .map(|s| s.name.as_str())
                            .filter(|n| !n.is_empty())
                            .map(|n| (tf.to_screen(spos.x, spos.y), n))
                    }),
            };
            if let Some((center, name)) = tooltip_info {
                let (pill_pos, pill_sz) = tooltip_rect(center, bounds.size(), name);
                let pill_path = CanvasPath::rounded_rectangle(pill_pos, pill_sz, 6.0.into());
                let bg_fill = with_alpha(Color::from(theme.cosmic().bg_color()), 0.94);
                frame.fill(&pill_path, bg_fill);
                frame.stroke(
                    &pill_path,
                    Stroke::default()
                        .with_color(with_alpha(accent, 0.55))
                        .with_width(1.2),
                );
                frame.fill_text(CanvasText {
                    content: name.to_string(),
                    position: Point::new(pill_pos.x + pill_sz.width * 0.5, pill_pos.y + pill_sz.height * 0.5),
                    color: with_alpha(on_bg, 1.0),
                    size: Pixels(12.0),
                    align_x: alignment::Horizontal::Center.into(),
                    align_y: alignment::Vertical::Center,
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
        if self.dialog_open {
            return mouse::Interaction::default();
        }
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
        let tf = CanvasTransform::fit(bounds.size(), self.doc.canvas.width, self.doc.canvas.height);
        if self.armed_place_lamp.is_some() {
            return mouse::Interaction::Crosshair;
        }
        if self.mode == CanvasMode::Wire {
            if self.hit_lamp(pos, tf).is_some() || self.hit_sensor(pos, tf).is_some() {
                return mouse::Interaction::Crosshair;
            }
            if self.hit_wire(pos, tf).is_some() {
                return mouse::Interaction::Pointer;
            }
            return mouse::Interaction::Crosshair;
        }
        if self.hit_selected_box_vertex(pos, tf).is_some()
            || self.hit_planned_strip_vertex(pos, tf).is_some()
        {
            return mouse::Interaction::Grab;
        }
        if self.hit_selected_box_midpoint(pos, tf).is_some() {
            return mouse::Interaction::Pointer;
        }
        if let Some((_, handle)) = self.hit_selected_box_handle(pos, tf) {
            return handle.cursor();
        }
        if self.hit_planned_light(pos, tf).is_some()
            || self.hit_lamp(pos, tf).is_some()
            || self.hit_sensor(pos, tf).is_some()
            || self.hit_stamp(pos, tf).is_some()
        {
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
    sx: f64,
    sy: f64,
    tf: CanvasTransform,
    on_bg: Color,
    accent: Color,
    selected: bool,
) {
    let cx = tf.offset_x + ((sx + st.w * 0.5) as f32) * tf.scale;
    let cy = tf.offset_y + ((sy + st.h * 0.5) as f32) * tf.scale;
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
            "stairs" => {
                let outer = CanvasPath::rectangle(Point::new(-hw, -hh), Size::new(w, h));
                f.fill(&outer, fill_col);
                f.stroke(&outer, stroke);
                for i in 1..7 {
                    let sy = -hh + (h * (i as f32) / 7.0);
                    f.stroke(&CanvasPath::line(Point::new(-hw, sy), Point::new(hw, sy)), stroke);
                }
                f.stroke(&CanvasPath::line(Point::new(0.0, hh - 5.0), Point::new(0.0, -hh + 5.0)), stroke);
                f.stroke(&CanvasPath::line(Point::new(0.0, -hh + 5.0), Point::new(-5.0, -hh + 11.0)), stroke);
                f.stroke(&CanvasPath::line(Point::new(0.0, -hh + 5.0), Point::new(5.0, -hh + 11.0)), stroke);
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

/// Draws a crisp, bold vector silhouette inside a lamp node matching `lamp_icon_svg` 1:1.
fn draw_lamp_icon(frame: &mut Frame, c: Point, r: f32, icon_key: &str, ink: Color) {
    let s = Stroke::default()
        .with_color(ink)
        .with_width(1.85)
        .with_line_cap(LineCap::Round)
        .with_line_join(LineJoin::Round);

    match normalize_lamp_icon(icon_key) {
        "bulb" => {
            // Solid lightbulb globe + base bars
            frame.fill(&CanvasPath::circle(Point::new(c.x, c.y - r * 0.22), r * 0.56), ink);
            frame.fill(
                &CanvasPath::rectangle(Point::new(c.x - r * 0.28, c.y + r * 0.24), Size::new(r * 0.56, r * 0.26)),
                ink,
            );
            frame.stroke(
                &CanvasPath::line(Point::new(c.x - r * 0.28, c.y + r * 0.68), Point::new(c.x + r * 0.28, c.y + r * 0.68)),
                s,
            );
            frame.stroke(
                &CanvasPath::line(Point::new(c.x - r * 0.18, c.y + r * 0.92), Point::new(c.x + r * 0.18, c.y + r * 0.92)),
                s.with_width(1.6),
            );
        }
        "spot" => {
            // Solid downward spotlight housing + 3 light beam rays
            let housing = CanvasPath::new(|b| {
                b.move_to(Point::new(c.x - r * 0.38, c.y - r * 0.80));
                b.line_to(Point::new(c.x + r * 0.38, c.y - r * 0.80));
                b.line_to(Point::new(c.x + r * 0.76, c.y + r * 0.12));
                b.line_to(Point::new(c.x - r * 0.76, c.y + r * 0.12));
                b.close();
            });
            frame.fill(&housing, ink);
            frame.stroke(&CanvasPath::line(Point::new(c.x, c.y + r * 0.36), Point::new(c.x, c.y + r * 0.86)), s);
            frame.stroke(
                &CanvasPath::line(Point::new(c.x - r * 0.38, c.y + r * 0.34), Point::new(c.x - r * 0.60, c.y + r * 0.80)),
                s.with_width(1.55),
            );
            frame.stroke(
                &CanvasPath::line(Point::new(c.x + r * 0.38, c.y + r * 0.34), Point::new(c.x + r * 0.60, c.y + r * 0.80)),
                s.with_width(1.55),
            );
        }
        "strip" => {
            // Horizontal LED strip bar with 3 bold LED dots
            let bar = CanvasPath::rounded_rectangle(
                Point::new(c.x - r * 0.88, c.y - r * 0.34),
                Size::new(r * 1.76, r * 0.68),
                3.0.into(),
            );
            frame.stroke(&bar, s);
            for dx in [-0.46, 0.0, 0.46] {
                frame.fill(&CanvasPath::circle(Point::new(c.x + r * dx, c.y), 1.8), ink);
            }
        }
        "floor" => {
            // Tall floor lamp: filled shade + tall stem + wide base
            let shade = CanvasPath::new(|b| {
                b.move_to(Point::new(c.x - r * 0.38, c.y - r * 0.86));
                b.line_to(Point::new(c.x + r * 0.38, c.y - r * 0.86));
                b.line_to(Point::new(c.x + r * 0.66, c.y - r * 0.16));
                b.line_to(Point::new(c.x - r * 0.66, c.y - r * 0.16));
                b.close();
            });
            frame.fill(&shade, ink);
            frame.stroke(&CanvasPath::line(Point::new(c.x, c.y - r * 0.16), Point::new(c.x, c.y + r * 0.86)), s);
            frame.stroke(&CanvasPath::line(Point::new(c.x - r * 0.50, c.y + r * 0.86), Point::new(c.x + r * 0.50, c.y + r * 0.86)), s);
        }
        "table" => {
            // Wide filled cozy shade + short thick stem + base
            let shade = CanvasPath::new(|b| {
                b.move_to(Point::new(c.x - r * 0.50, c.y - r * 0.68));
                b.line_to(Point::new(c.x + r * 0.50, c.y - r * 0.68));
                b.line_to(Point::new(c.x + r * 0.82, c.y + r * 0.14));
                b.line_to(Point::new(c.x - r * 0.82, c.y + r * 0.14));
                b.close();
            });
            frame.fill(&shade, ink);
            frame.stroke(&CanvasPath::line(Point::new(c.x, c.y + r * 0.14), Point::new(c.x, c.y + r * 0.76)), s.with_width(2.2));
            frame.stroke(&CanvasPath::line(Point::new(c.x - r * 0.52, c.y + r * 0.76), Point::new(c.x + r * 0.52, c.y + r * 0.76)), s);
        }
        "pendant" => {
            // Hanging cord + filled dome + glowing bulb drop
            frame.stroke(&CanvasPath::line(Point::new(c.x, c.y - r * 0.90), Point::new(c.x, c.y - r * 0.28)), s);
            let dome = CanvasPath::new(|b| {
                b.arc(canvas::path::Arc {
                    center: Point::new(c.x, c.y + r * 0.36),
                    radius: r * 0.68,
                    start_angle: Radians(PI),
                    end_angle: Radians(2.0 * PI),
                });
                b.close();
            });
            frame.fill(&dome, ink);
            frame.fill(&CanvasPath::circle(Point::new(c.x, c.y + r * 0.58), 2.1), ink);
        }
        "ceiling" => {
            // Flush ceiling bar + filled semi-dome + 3 downward rays
            frame.stroke(&CanvasPath::line(Point::new(c.x - r * 0.80, c.y - r * 0.56), Point::new(c.x + r * 0.80, c.y - r * 0.56)), s.with_width(2.0));
            let dome = CanvasPath::new(|b| {
                b.arc(canvas::path::Arc {
                    center: Point::new(c.x, c.y - r * 0.44),
                    radius: r * 0.56,
                    start_angle: Radians(0.0),
                    end_angle: Radians(PI),
                });
                b.close();
            });
            frame.fill(&dome, ink);
            frame.stroke(&CanvasPath::line(Point::new(c.x, c.y + r * 0.38), Point::new(c.x, c.y + r * 0.82)), s);
            frame.stroke(
                &CanvasPath::line(Point::new(c.x - r * 0.42, c.y + r * 0.32), Point::new(c.x - r * 0.62, c.y + r * 0.68)),
                s.with_width(1.5),
            );
            frame.stroke(
                &CanvasPath::line(Point::new(c.x + r * 0.42, c.y + r * 0.32), Point::new(c.x + r * 0.62, c.y + r * 0.68)),
                s.with_width(1.5),
            );
        }
        "wall" => {
            // Wall plate on left + filled sconce cup + forward ray
            frame.stroke(&CanvasPath::line(Point::new(c.x - r * 0.66, c.y - r * 0.74), Point::new(c.x - r * 0.66, c.y + r * 0.74)), s.with_width(2.2));
            let cup = CanvasPath::new(|b| {
                b.move_to(Point::new(c.x - r * 0.52, c.y - r * 0.22));
                b.line_to(Point::new(c.x + r * 0.48, c.y - r * 0.62));
                b.line_to(Point::new(c.x + r * 0.48, c.y + r * 0.62));
                b.line_to(Point::new(c.x - r * 0.52, c.y + r * 0.22));
                b.close();
            });
            frame.fill(&cup, ink);
            frame.stroke(&CanvasPath::line(Point::new(c.x + r * 0.66, c.y), Point::new(c.x + r * 0.88, c.y)), s);
        }
        "bar" => {
            // Solid vertical Play light bar
            let bar = CanvasPath::rounded_rectangle(
                Point::new(c.x - r * 0.32, c.y - r * 0.84),
                Size::new(r * 0.64, r * 1.68),
                3.0.into(),
            );
            frame.fill(&bar, ink);
        }
        "desk" => {
            // Articulated desk arm + solid angled lamp head + base
            let arm = CanvasPath::new(|b| {
                b.move_to(Point::new(c.x - r * 0.40, c.y + r * 0.72));
                b.line_to(Point::new(c.x - r * 0.14, c.y - r * 0.06));
                b.line_to(Point::new(c.x + r * 0.36, c.y - r * 0.56));
            });
            frame.stroke(&arm, s);
            let head = CanvasPath::new(|b| {
                b.move_to(Point::new(c.x + r * 0.20, c.y - r * 0.82));
                b.line_to(Point::new(c.x + r * 0.80, c.y - r * 0.36));
                b.line_to(Point::new(c.x + r * 0.50, c.y - r * 0.04));
                b.line_to(Point::new(c.x + r * 0.06, c.y - r * 0.38));
                b.close();
            });
            frame.fill(&head, ink);
            frame.stroke(&CanvasPath::line(Point::new(c.x - r * 0.74, c.y + r * 0.74), Point::new(c.x + r * 0.24, c.y + r * 0.74)), s.with_width(2.2));
        }
        "outdoor" => {
            // Solid sun center + 8 crisp rays
            frame.fill(&CanvasPath::circle(c, r * 0.42), ink);
            for i in 0..8 {
                let ang = (i as f32) * (PI / 4.0);
                let p1 = Point::new(c.x + ang.cos() * r * 0.62, c.y + ang.sin() * r * 0.62);
                let p2 = Point::new(c.x + ang.cos() * r * 0.90, c.y + ang.sin() * r * 0.90);
                frame.stroke(&CanvasPath::line(p1, p2), s.with_width(1.65));
            }
        }
        "candle" => {
            // Solid flame teardrop + candle body
            let flame = CanvasPath::new(|b| {
                b.move_to(Point::new(c.x, c.y - r * 0.88));
                b.quadratic_curve_to(Point::new(c.x + r * 0.52, c.y - r * 0.14), Point::new(c.x, c.y + r * 0.12));
                b.quadratic_curve_to(Point::new(c.x - r * 0.52, c.y - r * 0.14), Point::new(c.x, c.y - r * 0.88));
            });
            frame.fill(&flame, ink);
            let stick = CanvasPath::rounded_rectangle(
                Point::new(c.x - r * 0.32, c.y + r * 0.24),
                Size::new(r * 0.64, r * 0.64),
                1.5.into(),
            );
            frame.fill(&stick, ink);
        }
        _ => {
            frame.fill(&CanvasPath::circle(c, r * 0.55), ink);
        }
    }
}

/// Draws a crisp vector icon inside a placed sensor node (motion radar, door contact, or 2-button switch).
fn draw_sensor_icon(frame: &mut Frame, c: Point, r: f32, kind: &str, ink: Color, active: bool) {
    let s = Stroke::default()
        .with_color(ink)
        .with_width(1.7)
        .with_line_cap(LineCap::Round)
        .with_line_join(LineJoin::Round);

    match kind {
        "motion" => {
            // Center beacon dot + two symmetrical radar wave arcs
            frame.fill(&CanvasPath::circle(c, r * 0.24), ink);
            for (rad_mul, span) in [(0.56f32, 0.85f32), (0.90f32, 0.95f32)] {
                let right_arc = CanvasPath::new(|b| {
                    b.arc(canvas::path::Arc {
                        center: c,
                        radius: r * rad_mul,
                        start_angle: Radians(-span),
                        end_angle: Radians(span),
                    });
                });
                let left_arc = CanvasPath::new(|b| {
                    b.arc(canvas::path::Arc {
                        center: c,
                        radius: r * rad_mul,
                        start_angle: Radians(PI - span),
                        end_angle: Radians(PI + span),
                    });
                });
                frame.stroke(&right_arc, s.with_width(1.5));
                frame.stroke(&left_arc, s.with_width(1.5));
            }
        }
        "contact" => {
            // Two magnetic contact halves: snug together when closed, parted when open
            let gap = if active { r * 0.34 } else { r * 0.10 };
            let bar_w = r * 0.36;
            let bar_h = r * 1.24;
            let left = CanvasPath::rounded_rectangle(
                Point::new(c.x - gap - bar_w, c.y - bar_h * 0.5),
                Size::new(bar_w, bar_h),
                1.5.into(),
            );
            let right = CanvasPath::rounded_rectangle(
                Point::new(c.x + gap, c.y - bar_h * 0.5),
                Size::new(bar_w, bar_h),
                1.5.into(),
            );
            frame.fill(&left, ink);
            frame.fill(&right, ink);
            if active {
                // Tiny alert bolt/gap marks between parted halves
                frame.stroke(
                    &CanvasPath::line(Point::new(c.x, c.y - r * 0.24), Point::new(c.x, c.y + r * 0.24)),
                    s.with_width(1.4),
                );
            }
        }
        _ => {
            // 2-button Hue dimmer/switch plate
            let plate = CanvasPath::rounded_rectangle(
                Point::new(c.x - r * 0.62, c.y - r * 0.86),
                Size::new(r * 1.24, r * 1.72),
                2.5.into(),
            );
            frame.stroke(&plate, s.with_width(1.5));
            frame.stroke(
                &CanvasPath::line(Point::new(c.x - r * 0.48, c.y), Point::new(c.x + r * 0.48, c.y)),
                s.with_width(1.2),
            );
            frame.fill(&CanvasPath::circle(Point::new(c.x, c.y - r * 0.40), r * 0.18), ink);
            frame.fill(&CanvasPath::circle(Point::new(c.x, c.y + r * 0.40), r * 0.18), ink);
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
            Some(st) => (with_alpha(st.bg_tint, 0.26), with_alpha(st.border_tint, 0.50), st.border_width),
            None => (with_alpha(base, 0.045), with_alpha(base, 0.12), 1.0),
        };
        iced_container::Style {
            background: Some(Background::Color(bg)),
            border: Border { radius: corners::now().small.into(), width, color: border },
            ..Default::default()
        }
    })
}

fn dialog_card(skin: &str) -> theme::Container<'static> {
    let skin_st = skin_style(skin);
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let bg = Color::from(theme.cosmic().bg_color());
        let accent: Color = theme.cosmic().accent_color().into();
        let (border, width) = match skin_st {
            Some(st) => (with_alpha(st.border_tint, 0.78), st.border_width.max(1.25)),
            None => (with_alpha(accent, 0.55), 1.25),
        };
        iced_container::Style {
            background: Some(Background::Color(Color { a: 1.0, ..bg })),
            border: Border { radius: corners::now().radius.into(), width, color: border },
            ..Default::default()
        }
    })
}

fn lamp_icon_handle(icon_key: &str) -> icon::Handle {
    let mut handle = icon::from_svg_bytes(lamp_icon_svg(icon_key));
    handle.symbolic = true;
    handle
}

fn lamp_icon_badge<'a>(hex: &str, on: bool, icon_key: &str, size: f32) -> Element<'a, MapMsg> {
    let c = parse_hex_color(hex);
    let glyph_px = (size * 0.58).round().clamp(11.0, 22.0) as u16;
    let svg_icon = icon::icon(lamp_icon_handle(icon_key))
        .size(glyph_px)
        .class(theme::Svg::custom(move |theme: &cosmic::Theme| iced_svg::Style {
            color: Some(if on {
                contrast_ink(c)
            } else {
                with_alpha(Color::from(theme.cosmic().on_bg_color()), 0.85)
            }),
        }));
    container(svg_icon)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .class(theme::Container::custom(move |theme: &cosmic::Theme| {
            let base = Color::from(theme.cosmic().on_bg_color());
            let fill = if on { c } else { with_alpha(base, 0.16) };
            let border_col = if on { Color::WHITE } else { with_alpha(base, 0.38) };
            iced_container::Style {
                background: Some(Background::Color(fill)),
                border: Border { radius: (size * 0.5).into(), width: 1.2, color: border_col },
                ..Default::default()
            }
        }))
        .into()
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

fn sensor_dot<'a>(sensor: &HueSensor, size: f32) -> Element<'a, MapMsg> {
    let fill = match sensor.kind.as_str() {
        "motion" => {
            if !sensor.enabled {
                Color::from_rgb8(0x64, 0x74, 0x8B)
            } else if sensor.motion {
                MOTION_ACTIVE_COLOR
            } else {
                MOTION_IDLE_COLOR
            }
        }
        "contact" => {
            if !sensor.enabled {
                Color::from_rgb8(0x64, 0x74, 0x8B)
            } else if sensor.open {
                CONTACT_OPEN_COLOR
            } else {
                CONTACT_CLOSED_COLOR
            }
        }
        _ => BUTTON_COLOR,
    };
    container(space::horizontal())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .class(theme::Container::custom(move |theme: &cosmic::Theme| {
            let base = Color::from(theme.cosmic().on_bg_color());
            iced_container::Style {
                background: Some(Background::Color(fill)),
                border: Border { radius: 3.0.into(), width: 1.0, color: with_alpha(base, 0.35) },
                ..Default::default()
            }
        }))
        .into()
}

fn sensor_meta_line(sensor: &HueSensor) -> String {
    let mut parts: Vec<String> = Vec::new();
    match sensor.kind.as_str() {
        "motion" => {
            if !sensor.enabled {
                parts.push(tr("Disable"));
            } else if sensor.motion {
                parts.push(tr("Motion"));
            } else {
                parts.push(tr("No motion"));
            }
            if let Some(t) = sensor.temperature {
                parts.push(format!("{t:.1}°C"));
            }
            if let Some(lux) = sensor.lux {
                parts.push(format!("{:.0} lx", lux));
            }
        }
        "contact" => {
            if !sensor.enabled {
                parts.push(tr("Disable"));
            } else if sensor.open {
                parts.push(tr("Open"));
            } else {
                parts.push(tr("Closed"));
            }
        }
        _ => {
            parts.push(tr("Switch"));
        }
    }
    if let Some(bat) = sensor.battery {
        parts.push(format!("{bat}%"));
    }
    parts.join(" · ")
}

pub fn view_map_tab<'a>(
    state: &'a FloorplanState,
    stats: &'a Value,
    skin: &'a str,
    window_mode: bool,
    window_width: f32,
    pending_sync: usize,
    motion_hint_dismissed: bool,
) -> Element<'a, MapMsg> {
    let lights = extract_bridge_lights(stats);
    let sensors = parse_sensors(stats);
    let motion_areas = parse_motion_areas(stats);
    let ent_areas = parse_entertainment_areas(stats);
    let pending = pending_sync;
    let wide = window_mode && window_width >= 760.0;
    let program = FloorplanCanvasProgram::from_state(state, stats);
    let wire_count = program.wires.len();

    let mut root = column![].spacing(8);

    // ----------------------------------------------------------------- Simplified 1-Row Top Toolbar
    let map_tab_btn = if state.mode == CanvasMode::Select {
        button::suggested(tr("Map"))
    } else {
        button::standard(tr("Map"))
    }
    .on_press(MapMsg::SetMode(CanvasMode::Select));

    let plan_tab_label = if !state.doc.planned_lights.is_empty() {
        format!("{} ({})", tr("Plan"), state.doc.planned_lights.len())
    } else {
        tr("Plan")
    };
    let plan_tab_btn = if state.mode == CanvasMode::Plan {
        button::suggested(plan_tab_label)
    } else {
        button::standard(plan_tab_label)
    }
    .on_press(MapMsg::SetMode(CanvasMode::Plan));

    let rules_tab_label = if wire_count > 0 {
        format!("{} ({wire_count})", tr("Rules"))
    } else {
        tr("Rules")
    };
    let rules_tab_btn = if state.mode == CanvasMode::Wire {
        button::suggested(rules_tab_label)
    } else {
        button::standard(rules_tab_label)
    }
    .on_press(MapMsg::SetMode(CanvasMode::Wire));

    let mode_switcher: Element<MapMsg> = row![map_tab_btn, plan_tab_btn, rules_tab_btn]
        .spacing(2)
        .align_y(Alignment::Center)
        .into();

    let sidebar_btn = if state.sidebar_open {
        button::suggested(tr("Lamps"))
    } else {
        button::standard(tr("Lamps"))
    }
    .on_press(MapMsg::ToggleSidebar);

    let toolbar: Vec<Element<MapMsg>> = if state.mode == CanvasMode::Wire {
        vec![mode_switcher, sidebar_btn.into()]
    } else if state.mode == CanvasMode::Plan {
        let add_fix_label = format!("{} ▾", tr("+ Fixture"));
        let add_fix_btn = if state.plan_add_open {
            button::suggested(add_fix_label)
        } else {
            button::standard(add_fix_label)
        }
        .on_press(MapMsg::TogglePlanAddMenu);

        let list_label = if !state.doc.planned_lights.is_empty() {
            format!("{} ({})", tr("Plan List"), state.doc.planned_lights.len())
        } else {
            tr("Plan List")
        };
        let list_btn = if state.plan_list_open {
            button::suggested(list_label)
        } else {
            button::standard(list_label)
        }
        .on_press(MapMsg::TogglePlanList);

        let gallery_btn = if state.templates_open {
            button::suggested(tr("Gallery…"))
        } else {
            button::standard(tr("Gallery…"))
        }
        .on_press(MapMsg::ToggleTemplatesMenu);

        vec![mode_switcher, sidebar_btn.into(), add_fix_btn.into(), list_btn.into(), gallery_btn.into()]
    } else {
        let add_label = format!("{} ▾", tr("+ Add"));
        let add_btn = if state.add_menu_open {
            button::suggested(add_label)
        } else {
            button::standard(add_label)
        }
        .on_press(MapMsg::ToggleAddMenu);

        let tpl_label = format!("{} ▾", tr("Gallery…"));
        let tpl_btn = if state.templates_open {
            button::suggested(tpl_label)
        } else {
            button::standard(tpl_label)
        }
        .on_press(MapMsg::ToggleTemplatesMenu);

        let view_label = format!("{} ▾", tr("View"));
        let view_btn = if state.view_menu_open || state.bg_open || state.ent_menu_open {
            button::suggested(view_label)
        } else {
            button::standard(view_label)
        }
        .on_press(MapMsg::ToggleViewMenu);

        let hue_paired = stats.get("hue_paired").and_then(Value::as_bool).unwrap_or(true);
        let mut items: Vec<Element<MapMsg>> = vec![
            mode_switcher,
            sidebar_btn.into(),
            add_btn.into(),
            tpl_btn.into(),
            view_btn.into(),
        ];
        if hue_paired {
            let sync_label = if pending > 0 {
                trf("Sync All ({0})", &[pending.to_string()])
            } else {
                tr("Sync All")
            };
            let sync_btn = if pending > 0 {
                button::suggested(sync_label)
            } else {
                button::standard(sync_label)
            }
            .on_press(MapMsg::SyncAll);
            items.push(sync_btn.into());
        }
        items
    };

    root = root.push(
        container(flex_row(toolbar).row_spacing(6).column_spacing(6))
            .padding(8)
            .width(Length::Fill)
            .class(studio_card(skin)),
    );

    // ----------------------------------------------------------------- Multi-Floor Tabs + Day/Dusk/Night Ambient Bar
    let mut floor_items: Vec<Element<MapMsg>> = state
        .doc
        .floors
        .iter()
        .map(|fl| {
            let active = state.doc.active_floor_id == fl.id;
            let lit_on_floor = state
                .doc
                .lamps
                .iter()
                .filter(|(lid, p)| p.floor_id == fl.id && lights.iter().any(|l| l.id == **lid && l.on))
                .count();
            let display_name = tr(&fl.name);
            let lbl = if lit_on_floor > 0 {
                format!("{display_name} · {lit_on_floor}")
            } else {
                display_name
            };
            let b = if active { button::suggested(lbl) } else { button::standard(lbl) };
            b.on_press(MapMsg::SelectFloor(fl.id.clone())).into()
        })
        .collect();
    floor_items.push(button::standard(tr("+ Floor")).on_press(MapMsg::AddFloor).into());
    if state.floor_rename_open {
        floor_items.push(
            row![
                text_input(tr("Floor name"), &state.floor_rename_draft)
                    .on_input(MapMsg::FloorRenameInput)
                    .on_submit(|_| MapMsg::FloorRenameSave)
                    .width(Length::Fixed(130.0)),
                button::suggested(tr("Save")).on_press(MapMsg::FloorRenameSave),
                button::text(tr("Cancel")).on_press(MapMsg::FloorRenameCancel),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .into(),
        );
    } else {
        floor_items.push(button::standard(tr("Rename")).on_press(MapMsg::FloorRenameStart).into());
    }
    if state.doc.floors.len() > 1 {
        floor_items.push(
            button::destructive(tr("Delete Floor"))
                .on_press(MapMsg::DeleteFloor(state.doc.active_floor_id.clone()))
                .into(),
        );
    }
    if state.undo_doc.is_some() {
        floor_items.push(button::standard(tr("Undo")).on_press(MapMsg::UndoFloorplan).into());
    }

    let ambient_row = row![
        (if state.ambient == AmbientMode::Day { button::suggested(tr("Day")) } else { button::standard(tr("Day")) })
            .on_press(MapMsg::SetAmbient(AmbientMode::Day)),
        (if state.ambient == AmbientMode::Dusk { button::suggested(tr("Dusk")) } else { button::standard(tr("Dusk")) })
            .on_press(MapMsg::SetAmbient(AmbientMode::Dusk)),
        (if state.ambient == AmbientMode::Night { button::suggested(tr("Night")) } else { button::standard(tr("Night")) })
            .on_press(MapMsg::SetAmbient(AmbientMode::Night)),
    ]
    .spacing(2)
    .align_y(Alignment::Center);

    root = root.push(
        container(
            row![
                flex_row(floor_items).row_spacing(4).column_spacing(4),
                space::horizontal(),
                ambient_row,
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([5, 8])
        .width(Length::Fill)
        .class(studio_card(skin)),
    );

    // ----------------------------------------------------------------- Dropdown Popovers (Map & Plan sub-tabs)
    if state.mode == CanvasMode::Select && state.add_menu_open {
        let box_row = flex_row(vec![
            button::standard(tr("+ Room")).on_press(MapMsg::AddBox("room".into())).into(),
            button::standard(tr("Add L-Shape Room")).on_press(MapMsg::AddBoxWithShape("room".into(), "l_shape".into())).into(),
            button::standard(tr("Add Bay Window Room")).on_press(MapMsg::AddBoxWithShape("room".into(), "bay_window".into())).into(),
            button::standard(tr("+ Zone")).on_press(MapMsg::AddBox("zone".into())).into(),
        ])
        .row_spacing(6)
        .column_spacing(6);
        let stamp_btns: Vec<Element<MapMsg>> = FLOORPLAN_STAMP_KINDS
            .iter()
            .map(|sk| {
                button::standard(stamp_kind_label(sk))
                    .on_press(MapMsg::AddStamp((*sk).to_string()))
                    .into()
            })
            .collect();
        let opts = column![
            box_row,
            divider::horizontal::light(),
            dim_caption(tr("Add architectural stamp")),
            flex_row(stamp_btns).row_spacing(6).column_spacing(6),
        ]
        .spacing(6);
        root = root.push(container(opts).padding(8).width(Length::Fill).class(studio_card(skin)));
    }

    if (state.mode == CanvasMode::Select || state.mode == CanvasMode::Plan) && state.templates_open {
        let mut opts = column![
            row![
                text::body(tr("Floorplan Gallery")),
                space::horizontal(),
                checkbox(state.gallery_include_plan)
                    .label(tr("Include starter lighting plan"))
                    .on_toggle(MapMsg::ToggleGalleryIncludePlan),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            dim_caption(tr("Pick a realistic apartment or house drawing — every wall & corner is editable.")),
        ]
        .spacing(6);
        let btns: Vec<Element<MapMsg>> = FLOORPLAN_TEMPLATES
            .iter()
            .map(|tid| {
                let active = state.doc.template_id == *tid;
                let confirming = state.confirm_template_id.as_deref() == Some(*tid);
                let lbl = if confirming {
                    tr("Click again to replace map")
                } else if active {
                    format!("✓ {}", template_label(tid))
                } else {
                    template_label(tid)
                };
                let b = if confirming {
                    button::destructive(lbl)
                } else if active {
                    button::suggested(lbl)
                } else {
                    button::standard(lbl)
                };
                b.on_press(MapMsg::ApplyTemplate((*tid).to_string())).into()
            })
            .collect();
        opts = opts.push(flex_row(btns).row_spacing(6).column_spacing(6));
        root = root.push(container(opts).padding(10).width(Length::Fill).class(studio_card(skin)));
    }

    if state.mode == CanvasMode::Plan && state.plan_add_open {
        let preset_btns: Vec<Element<MapMsg>> = PLANNED_LIGHT_PRESETS
            .iter()
            .map(|p| {
                button::standard(format!("{} ({} lm)", tr(p.label), p.lumens))
                    .on_press(MapMsg::AddPlannedLight(p.id.to_string()))
                    .into()
            })
            .collect();
        let opts = column![
            dim_caption(tr("Add virtual future fixture or bendable LED lightstrip")),
            flex_row(preset_btns).row_spacing(6).column_spacing(6),
        ]
        .spacing(6);
        root = root.push(container(opts).padding(8).width(Length::Fill).class(studio_card(skin)));
    }

    if state.mode == CanvasMode::Plan && state.plan_list_open {
        let total_lm: u32 = state.doc.planned_lights.iter().map(|p| p.lumens).sum();
        let total_price: f64 = state.doc.planned_lights.iter().map(|p| p.price).sum();
        let mut list_col = column![
            row![
                text::body(tr("Lighting Plan & Shopping List")),
                space::horizontal(),
                dim_caption(format!("{} · {} lm · {:.0}", state.doc.planned_lights.len(), total_lm, total_price)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(6);
        if state.doc.planned_lights.is_empty() {
            list_col = list_col.push(dim_caption(tr("No planned fixtures yet. Click + Fixture to add virtual lights or LED strips.")));
        } else {
            for pl in &state.doc.planned_lights {
                let pid_t = pl.id.clone();
                let pid_d = pl.id.clone();
                list_col = list_col.push(
                    row![
                        text::caption(format!("{} · {} lm · {} K", tr(&pl.name), pl.lumens, pl.kelvin)),
                        space::horizontal(),
                        dim_caption(if pl.price > 0.0 { format!("{:.0}", pl.price) } else { String::new() }),
                        (if pl.on { button::suggested(tr("On")) } else { button::standard(tr("Off")) })
                            .on_press(MapMsg::TogglePlannedLight(pid_t)),
                        button::destructive("×").on_press(MapMsg::DeletePlannedLight(pid_d)),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                );
            }
        }
        root = root.push(container(list_col).padding(10).width(Length::Fill).class(studio_card(skin)));
    }

    if state.mode == CanvasMode::Select && state.stamps_open && !state.add_menu_open {
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

    if state.mode == CanvasMode::Select && state.view_menu_open {
        let mut layer_btns: Vec<Element<MapMsg>> = vec![
            (if state.doc.show_glows { button::suggested(tr("Glows")) } else { button::standard(tr("Glows")) })
                .on_press(MapMsg::ToggleGlows)
                .into(),
            (if state.doc.show_furniture { button::suggested(tr("Furniture")) } else { button::standard(tr("Furniture")) })
                .on_press(MapMsg::ToggleFurniture)
                .into(),
            (if state.doc.show_wires { button::suggested(tr("Wires")) } else { button::standard(tr("Wires")) })
                .on_press(MapMsg::ToggleWires)
                .into(),
            (if !state.doc.background.path.is_empty() || state.bg_open {
                button::suggested(tr("Background"))
            } else {
                button::standard(tr("Background"))
            })
            .on_press(MapMsg::ToggleBgMenu)
            .into(),
        ];
        if !ent_areas.is_empty() {
            let ent_lbl = state
                .doc
                .entertainment_id
                .as_deref()
                .and_then(|id| ent_areas.iter().find(|a| a.id == id).map(|a| a.name.clone()))
                .unwrap_or_else(|| tr("Entertainment"));
            let ent_text = format!("♪ {ent_lbl} ▾");
            let ent_btn = if state.ent_menu_open || state.doc.entertainment_id.is_some() {
                button::suggested(ent_text)
            } else {
                button::standard(ent_text)
            };
            layer_btns.push(ent_btn.on_press(MapMsg::ToggleEntMenu).into());
        }
        let view_panel = column![
            flex_row(layer_btns).row_spacing(6).column_spacing(6),
            row![
                dim_caption(tr("Auto-sync")),
                space::horizontal(),
                toggler(state.doc.auto_sync).on_toggle(MapMsg::ToggleAutoSync),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(8);
        root = root.push(container(view_panel).padding(8).width(Length::Fill).class(studio_card(skin)));
    }

    if state.mode == CanvasMode::Select && state.ent_menu_open && !ent_areas.is_empty() {
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

    if state.mode == CanvasMode::Select && state.bg_open {
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

    // Active mode hint banner (armed click-to-place or Rules sub-tab)
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
        let hint_row: Element<MapMsg> = if let Some((ref fk, ref fid)) = state.pending_wire_from {
            let src_name = resolve_name(stats, fk, fid);
            row![
                dim_caption(trf("Click a target lamp to wire from {0}", &[src_name])),
                space::horizontal(),
                button::text(tr("Cancel")).on_press(MapMsg::Select(None)),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        } else {
            dim_caption(tr(
                "Drag from one lamp to another to create a rule, or click a line to edit or delete.",
            ))
        };
        root = root.push(
            container(hint_row)
                .padding([6, 8])
                .width(Length::Fill)
                .class(studio_card(skin)),
        );
    }

    // ----------------------------------------------------------------- Canvas + Optional Background Layer
    let canvas_height = if window_mode { 520.0 } else { 340.0 };
    let canvas_widget = cosmic::widget::canvas(program)
        .width(Length::Fill)
        .height(Length::Fixed(canvas_height));

    let layered_canvas: Element<MapMsg> = if let Some((_, ref bg_handle)) = state.bg_handle_cache {
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
        let bg_img = iced_image(bg_handle.clone())
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

        if should_show_motion_aware_hint(stats, motion_hint_dismissed) {
            let eligible = motion_aware_eligible_rooms(stats);
            let mut hint_col = column![
                row![
                    container(space::horizontal())
                        .width(Length::Fixed(8.0))
                        .height(Length::Fixed(8.0))
                        .class(theme::Container::custom(|_| iced_container::Style {
                            background: Some(Background::Color(MOTION_ACTIVE_COLOR)),
                            border: Border { radius: 4.0.into(), ..Default::default() },
                            ..Default::default()
                        })),
                    text::caption(tr("Bridge Pro MotionAware™ Ready")),
                    space::horizontal(),
                    button::custom(text::caption("×"))
                        .padding([1, 5])
                        .class(theme::Button::MenuItem)
                        .on_press(MapMsg::DismissMotionHint),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                dim_caption(tr("1. Open the Philips Hue mobile app → Settings → MotionAware")),
                dim_caption(tr("2. Pick 3+ lights in a room and run the 30-second calibration once")),
                dim_caption(tr("3. Return here — live radar badges, sensitivity & visual rules appear automatically!")),
            ]
            .spacing(4);
            if !eligible.is_empty() {
                hint_col = hint_col.push(
                    text::caption(trf("Ready rooms (3+ lights): {0}", &[eligible.join(", ")])).class(
                        theme::Text::Custom(|_| iced_text::Style {
                            color: Some(MOTION_ACTIVE_COLOR),
                            ..Default::default()
                        }),
                    ),
                );
            }
            list_col = list_col.push(
                container(hint_col)
                    .padding(8)
                    .width(Length::Fill)
                    .class(theme::Container::custom(|_| iced_container::Style {
                        background: Some(Background::Color(with_alpha(MOTION_ACTIVE_COLOR, 0.10))),
                        border: Border {
                            radius: corners::now().small.into(),
                            width: 1.0,
                            color: with_alpha(MOTION_ACTIVE_COLOR, 0.45),
                        },
                        ..Default::default()
                    })),
            );
        }

        list_col = list_col.push(dim_caption(trf("Unplaced ({0})", &[unplaced.len().to_string()])));
        if unplaced.is_empty() {
            list_col = list_col.push(dim_caption(tr("All lamps placed")));
        } else {
            for l in unplaced {
                let menu_open = state.open_lamp_menu.as_deref() == Some(l.id.as_str());
                let meta = if l.room_name.is_empty() {
                    format!("{}%", l.bri.round() as i32)
                } else {
                    format!("{} · {}%", l.room_name, l.bri.round() as i32)
                };
                let main_row = row![
                    color_dot(&l.color, l.on, 10.0),
                    column![text::caption(l.name.clone()), dim_caption(meta)]
                        .spacing(1)
                        .width(Length::Fill),
                    button::standard(tr("Place")).on_press(MapMsg::PlaceLamp(l.id.clone())),
                    button::custom(text::caption("⋯"))
                        .padding([3, 7])
                        .class(if menu_open { theme::Button::Suggested } else { theme::Button::MenuItem })
                        .on_press(MapMsg::ToggleLampMenu(l.id.clone())),
                ]
                .spacing(4)
                .align_y(Alignment::Center);

                if menu_open {
                    let toggle_lbl = if l.on { tr("Turn off") } else { tr("Turn on") };
                    let submenu = container(
                        container(
                            column![
                                menu_button(text::caption(tr("Place")))
                                    .on_press(MapMsg::PlaceLamp(l.id.clone())),
                                menu_button(text::caption(format!("⌖ {}", tr("Place"))))
                                    .on_press(MapMsg::ArmPlaceLamp(l.id.clone())),
                                menu_button(text::caption(toggle_lbl))
                                    .on_press(MapMsg::LampToggle(l.id.clone(), !l.on)),
                                menu_button(text::caption(format!("{}…", tr("Brightness"))))
                                    .on_press(MapMsg::OpenLampDialog(LampDialog::Brightness(l.id.clone()))),
                                menu_button(text::caption(format!("{}…", tr("Rename"))))
                                    .on_press(MapMsg::OpenLampDialog(LampDialog::Rename(l.id.clone()))),
                            ]
                            .spacing(2),
                        )
                        .padding(4)
                        .width(Length::Fill)
                        .class(studio_card(skin)),
                    )
                    .padding([2, 0, 2, 12])
                    .width(Length::Fill);
                    list_col = list_col.push(column![main_row, submenu].spacing(2));
                } else {
                    list_col = list_col.push(main_row);
                }
            }
        }

        list_col = list_col.push(divider::horizontal::light());
        list_col = list_col.push(dim_caption(trf("Placed ({0})", &[placed.len().to_string()])));
        for l in placed {
            let icon_key = state
                .doc
                .lamps
                .get(&l.id)
                .map(|p| normalize_lamp_icon(&p.icon))
                .unwrap_or("bulb");
            let selected = matches!(&state.selection, Some(Selection::Lamp(id)) if id == &l.id);
            let menu_open = state.open_lamp_menu.as_deref() == Some(l.id.as_str());
            let meta = if l.room_name.is_empty() {
                format!("{}%", l.bri.round() as i32)
            } else {
                format!("{} · {}%", l.room_name, l.bri.round() as i32)
            };
            let focus_btn = button::custom(
                row![
                    lamp_icon_badge(&l.color, l.on, icon_key, 24.0),
                    column![text::caption(l.name.clone()), dim_caption(meta)].spacing(1),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([3, 4])
            .class(if selected { theme::Button::Suggested } else { theme::Button::MenuItem })
            .on_press(MapMsg::FocusLamp(l.id.clone()));

            let menu_btn = button::custom(text::caption("⋯"))
                .padding([3, 7])
                .class(if menu_open { theme::Button::Suggested } else { theme::Button::MenuItem })
                .on_press(MapMsg::ToggleLampMenu(l.id.clone()));

            let main_row = row![focus_btn, menu_btn]
                .spacing(2)
                .align_y(Alignment::Center);

            if menu_open {
                let toggle_lbl = if l.on { tr("Turn off") } else { tr("Turn on") };
                let submenu = container(
                    container(
                        column![
                            menu_button(text::caption(toggle_lbl))
                                .on_press(MapMsg::LampToggle(l.id.clone(), !l.on)),
                            menu_button(text::caption(format!("{}…", tr("Brightness"))))
                                .on_press(MapMsg::OpenLampDialog(LampDialog::Brightness(l.id.clone()))),
                            menu_button(text::caption(format!("{}…", tr("Rename"))))
                                .on_press(MapMsg::OpenLampDialog(LampDialog::Rename(l.id.clone()))),
                            menu_button(text::caption(format!("{}…", tr("Lamp icon"))))
                                .on_press(MapMsg::OpenLampDialog(LampDialog::Icon(l.id.clone()))),
                            menu_button(text::caption(tr("Wire from here")))
                                .on_press(MapMsg::LampStartWire(l.id.clone())),
                            menu_button(text::caption(tr("Unplace")))
                                .on_press(MapMsg::UnplaceLamp(l.id.clone())),
                        ]
                        .spacing(2),
                    )
                    .padding(4)
                    .width(Length::Fill)
                    .class(studio_card(skin)),
                )
                .padding([2, 0, 2, 12])
                .width(Length::Fill);
                list_col = list_col.push(column![main_row, submenu].spacing(2));
            } else {
                list_col = list_col.push(main_row);
            }
        }

        // Physical Hue Sensors & Switches section (only shown when sensors exist)
        if !sensors.is_empty() {
            let mut unplaced_sensors = Vec::new();
            let mut placed_sensors = Vec::new();
            for s in &sensors {
                if !q.is_empty() && !s.name.to_lowercase().contains(&q) {
                    continue;
                }
                if state.doc.sensors.contains_key(&s.id) {
                    placed_sensors.push(s);
                } else {
                    unplaced_sensors.push(s);
                }
            }
            if !unplaced_sensors.is_empty() || !placed_sensors.is_empty() {
                list_col = list_col.push(divider::horizontal::default());
                list_col = list_col.push(dim_caption(tr("Sensors")));
                for s in unplaced_sensors {
                    let meta = sensor_meta_line(s);
                    let row_el = row![
                        sensor_dot(s, 11.0),
                        column![text::caption(s.name.clone()), dim_caption(meta)]
                            .spacing(1)
                            .width(Length::Fill),
                        button::standard(tr("Place")).on_press(MapMsg::PlaceSensor(s.id.clone())),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center);
                    list_col = list_col.push(row_el);
                }
                for s in placed_sensors {
                    let selected = matches!(&state.selection, Some(Selection::Sensor(id)) if id == &s.id);
                    let menu_open = state.open_sensor_menu.as_deref() == Some(s.id.as_str());
                    let meta = sensor_meta_line(s);
                    let focus_btn = button::custom(
                        row![
                            sensor_dot(s, 14.0),
                            column![text::caption(s.name.clone()), dim_caption(meta)].spacing(1),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .width(Length::Fill)
                    .padding([3, 4])
                    .class(if selected { theme::Button::Suggested } else { theme::Button::MenuItem })
                    .on_press(MapMsg::FocusSensor(s.id.clone()));

                    let menu_btn = button::custom(text::caption("⋯"))
                        .padding([3, 7])
                        .class(if menu_open { theme::Button::Suggested } else { theme::Button::MenuItem })
                        .on_press(MapMsg::ToggleSensorMenu(s.id.clone()));

                    let main_row = row![focus_btn, menu_btn]
                        .spacing(2)
                        .align_y(Alignment::Center);

                    if menu_open {
                        let mut items = column![].spacing(2);
                        if matches!(s.kind.as_str(), "motion" | "contact") {
                            let lbl = if s.enabled { tr("Disable") } else { tr("Enable") };
                            items = items.push(
                                menu_button(text::caption(lbl)).on_press(MapMsg::SensorToggle {
                                    kind: s.kind.clone(),
                                    id: s.id.clone(),
                                    enabled: !s.enabled,
                                }),
                            );
                        }
                        items = items
                            .push(menu_button(text::caption(tr("Wire from here"))).on_press(
                                MapMsg::SensorStartWire {
                                    kind: s.kind.clone(),
                                    id: s.id.clone(),
                                },
                            ))
                            .push(
                                menu_button(text::caption(tr("Unplace")))
                                    .on_press(MapMsg::UnplaceSensor(s.id.clone())),
                            );
                        let submenu = container(
                            container(items)
                                .padding(4)
                                .width(Length::Fill)
                                .class(studio_card(skin)),
                        )
                        .padding([2, 0, 2, 12])
                        .width(Length::Fill);
                        list_col = list_col.push(column![main_row, submenu].spacing(2));
                    } else {
                        list_col = list_col.push(main_row);
                    }
                }
            }
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
    if let Some(inspector) = view_inspector(state, stats, &ent_areas, &motion_areas, skin) {
        root = root.push(inspector);
    }

    let base_view: Element<'a, MapMsg> = if window_mode {
        root.into()
    } else {
        container(scrollable(container(root).padding([0, 10, 0, 0])).height(Length::Shrink))
            .max_height(560.0)
            .into()
    };

    if let Some(dialog_overlay) = view_rule_dialog(state, &lights, skin) {
        iced_stack![base_view, dialog_overlay]
            .width(Length::Fill)
            .into()
    } else {
        base_view
    }
}

fn view_rule_dialog<'a>(
    state: &'a FloorplanState,
    lights: &[BridgeLightInfo],
    skin: &'a str,
) -> Option<Element<'a, MapMsg>> {
    let d = state.rule_dialog.as_ref()?;
    let title = if d.rule_id.is_some() { tr("Edit rule") } else { tr("+ New rule") };

    let mut header = row![text::heading(title), space::horizontal()]
        .spacing(8)
        .align_y(Alignment::Center);
    if d.rule_id.is_some() {
        header = header.push(toggler(d.enabled).on_toggle(MapMsg::RuleDialogEnabled));
    }
    header = header.push(button::text("×").on_press(MapMsg::CloseRuleDialog));

    let (from_col, from_on) = if d.trigger_kind == "light"
        && let Some(info) = lights.iter().find(|l| l.id == d.trigger_id)
    {
        (info.color.as_str(), info.on)
    } else {
        match d.trigger_kind.as_str() {
            "motion" | "motion_area" => ("#10b981", true),
            "contact" => ("#f59e0b", true),
            "button" => ("#6366f1", true),
            _ => ("#ffd27a", true),
        }
    };
    let (to_col, to_on) = if d.target_kind == "light"
        && let Some(info) = lights.iter().find(|l| l.id == d.target_id)
    {
        (info.color.as_str(), info.on)
    } else {
        ("#ffd27a", true)
    };
    let supports_bri = is_light_or_room_trigger(&d.trigger_kind);
    let supports_mirror = d.trigger_kind != "button";
    let arrow = if d.kind == "exclusive" {
        "⊘"
    } else if d.kind == "follow" && d.mirror_off && supports_mirror {
        "↔"
    } else {
        "→"
    };

    let link_banner = container(
        row![
            color_dot(from_col, from_on, 10.0),
            text::body(d.trigger_name.as_str()),
            dim_caption(arrow),
            color_dot(to_col, to_on, 10.0),
            text::body(d.target_name.as_str()),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([8, 10])
    .width(Length::Fill)
    .class(studio_card(skin));

    let type_pills: Vec<Element<'a, MapMsg>> = [
        ("follow", tr("Follow / Mirror")),
        ("exclusive", tr("Exclusive (turn other off)")),
        ("companion", tr("Companion")),
    ]
    .into_iter()
    .map(|(k, label)| {
        let b = if d.kind == k { button::suggested(label) } else { button::standard(label) };
        b.on_press(MapMsg::RuleDialogKind(k.into())).into()
    })
    .collect();

    let when_pills: Vec<Element<'a, MapMsg>> = trigger_state_options(&d.trigger_kind)
        .into_iter()
        .map(|(k, label)| {
            let b = if d.trigger_state == k { button::suggested(label) } else { button::standard(label) };
            b.on_press(MapMsg::RuleDialogTriggerState(k.into())).into()
        })
        .collect();

    let mut content = column![
        header,
        link_banner,
        column![
            dim_caption(tr("Rule type")),
            flex_row(type_pills).row_spacing(6).column_spacing(6),
        ]
        .spacing(4),
        row![
            dim_caption(tr("When")),
            space::horizontal(),
            flex_row(when_pills).row_spacing(6).column_spacing(6),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(10);

    if d.kind != "exclusive" {
        if supports_mirror {
            content = content.push(
                checkbox(d.mirror_off)
                    .label(tr("Also turn off together"))
                    .on_toggle(MapMsg::RuleDialogMirrorOff),
            );
        }
        if supports_bri {
            content = content.push(
                checkbox(d.match_bri)
                    .label(tr("Match brightness"))
                    .on_toggle(MapMsg::RuleDialogMatchBri),
            );
        }
        content = content.push(
            checkbox(d.only_if_off)
                .label(tr("Only if target is off"))
                .on_toggle(MapMsg::RuleDialogOnlyIfOff),
        );
    }

    content = content
        .push(
            row![
                dim_caption(tr("Active between")),
                space::horizontal(),
                text_input(tr("Optional HH:MM"), &d.time_start)
                    .on_input(MapMsg::RuleDialogTimeStart)
                    .width(Length::Fixed(96.0)),
                dim_caption("–"),
                text_input(tr("Optional HH:MM"), &d.time_end)
                    .on_input(MapMsg::RuleDialogTimeEnd)
                    .width(Length::Fixed(96.0)),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .push(
            text_input(tr("Name (optional)"), &d.name)
                .on_input(MapMsg::RuleDialogName)
                .on_submit(|_| MapMsg::RuleDialogSave)
                .width(Length::Fill),
        );

    if let Some(ref err) = d.error {
        content = content.push(
            text::caption(err.as_str()).class(theme::Text::Custom(|theme: &cosmic::Theme| {
                iced_text::Style {
                    color: Some(theme.cosmic().destructive_color().into()),
                    ..Default::default()
                }
            })),
        );
    }

    let mut footer = row![].spacing(8).align_y(Alignment::Center);
    if let Some(ref rid) = d.rule_id {
        footer = footer.push(
            button::custom(
                row![
                    icon::from_name("user-trash-symbolic").size(16).symbolic(true),
                    text::body(tr("Delete")),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([6, 12])
            .class(theme::Button::Destructive)
            .on_press(MapMsg::RuleDialogDelete(rid.clone())),
        );
    }
    footer = footer
        .push(space::horizontal())
        .push(button::standard(tr("Cancel")).on_press(MapMsg::CloseRuleDialog))
        .push(button::suggested(tr("Save")).on_press(MapMsg::RuleDialogSave));

    content = content.push(footer);

    let card = container(content)
        .padding(16)
        .max_width(440.0)
        .width(Length::Fill)
        .class(dialog_card(skin));

    let scrim = container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .padding(16)
        .class(theme::Container::custom(|_| iced_container::Style {
            background: Some(Background::Color(with_alpha(Color::BLACK, 0.52))),
            ..Default::default()
        }));

    Some(scrim.into())
}

/// Renders the shared centered lamp modal dialog (Brightness, Rename, or 12-icon Lamp Icon picker)
/// so it works identically whether triggered from the Lights tab or the Map sidebar.
pub fn view_lamp_dialog_overlay<'a>(
    state: &'a FloorplanState,
    stats: &'a Value,
    skin: &'a str,
    dragging_bri: Option<u8>,
) -> Option<Element<'a, MapMsg>> {
    let lights = extract_bridge_lights(stats);
    view_lamp_dialog(state, &lights, skin, dragging_bri)
}

fn view_lamp_dialog<'a>(
    state: &'a FloorplanState,
    lights: &[BridgeLightInfo],
    skin: &'a str,
    dragging_bri: Option<u8>,
) -> Option<Element<'a, MapMsg>> {
    let dialog = state.lamp_dialog.as_ref()?;
    let content: Element<'a, MapMsg> = match dialog {
        LampDialog::Brightness(lid) => {
            let info = lights.iter().find(|l| l.id == *lid)?;
            let bri = dragging_bri.unwrap_or_else(|| info.bri.round().clamp(1.0, 100.0) as u8);
            let lid_toggle = lid.clone();
            let lid_bri = lid.clone();
            let lid_bri_done = lid.clone();
            column![
                row![
                    color_dot(&info.color, info.on, 14.0),
                    text::heading(info.name.clone()),
                    space::horizontal(),
                    toggler(info.on).on_toggle(move |v| MapMsg::LampToggle(lid_toggle.clone(), v)),
                    button::text("×").on_press(MapMsg::CloseLampDialog),
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
                    space::horizontal(),
                    button::suggested(tr("Done")).on_press(MapMsg::CloseLampDialog),
                ]
                .align_y(Alignment::Center),
            ]
            .spacing(12)
            .into()
        }
        LampDialog::Rename(lid) => {
            let info = lights.iter().find(|l| l.id == *lid)?;
            let lid_submit = lid.clone();
            let can_save = !state.lamp_rename_draft.trim().is_empty();
            column![
                row![
                    color_dot(&info.color, info.on, 14.0),
                    text::heading(tr("Rename")),
                    space::horizontal(),
                    button::text("×").on_press(MapMsg::CloseLampDialog),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                dim_caption(info.name.clone()),
                text_input(tr("Lamp name"), &state.lamp_rename_draft)
                    .on_input(MapMsg::LampRenameInput)
                    .on_submit(move |_| MapMsg::LampRenameSave(lid_submit.clone()))
                    .width(Length::Fill),
                row![
                    space::horizontal(),
                    button::standard(tr("Cancel")).on_press(MapMsg::CloseLampDialog),
                    button::suggested(tr("Rename"))
                        .on_press_maybe(can_save.then_some(MapMsg::LampRenameSave(lid.clone()))),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(10)
            .into()
        }
        LampDialog::Icon(lid) => {
            let info = lights.iter().find(|l| l.id == *lid)?;
            let current_icon = state
                .doc
                .lamps
                .get(lid)
                .map(|p| normalize_lamp_icon(&p.icon))
                .unwrap_or("bulb");
            let icon_buttons: Vec<Element<'a, MapMsg>> = LAMP_ICONS
                .iter()
                .map(|ic| {
                    let active = current_icon == *ic;
                    let inner = row![
                        icon::icon(lamp_icon_handle(ic)).size(16),
                        text::caption(lamp_icon_label(ic)),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center);
                    button::custom(inner)
                        .padding([6, 10])
                        .class(if active { theme::Button::Suggested } else { theme::Button::Standard })
                        .on_press(MapMsg::LampSetIcon(lid.clone(), (*ic).to_string()))
                        .into()
                })
                .collect();
            column![
                row![
                    lamp_icon_badge(&info.color, info.on, current_icon, 28.0),
                    text::heading(format!("{} — {}", info.name, tr("Lamp icon"))),
                    space::horizontal(),
                    button::text("×").on_press(MapMsg::CloseLampDialog),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                flex_row(icon_buttons).row_spacing(6).column_spacing(6),
                row![
                    space::horizontal(),
                    button::suggested(tr("Done")).on_press(MapMsg::CloseLampDialog),
                ]
                .align_y(Alignment::Center),
            ]
            .spacing(12)
            .into()
        }
    };

    let card = container(content)
        .padding(16)
        .max_width(420.0)
        .width(Length::Fill)
        .class(dialog_card(skin));

    let scrim = container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .padding(16)
        .class(theme::Container::custom(|_| iced_container::Style {
            background: Some(Background::Color(with_alpha(Color::BLACK, 0.52))),
            ..Default::default()
        }));

    Some(scrim.into())
}

fn view_inspector<'a>(
    state: &'a FloorplanState,
    stats: &'a Value,
    ent_areas: &[EntertainmentArea],
    motion_areas: &[MotionArea],
    skin: &'a str,
) -> Option<Element<'a, MapMsg>> {
    let sel = state.selection.as_ref()?;
    let inner: Element<MapMsg> = match sel {
        Selection::Lamp(_) | Selection::Sensor(_) => return None,

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

            // Room Shape starters (convert to L-Shape, Bay Window, T-Shape, Hallway, Angled Wall, or Rectangle)
            let shape_btns: Vec<Element<MapMsg>> = FLOORPLAN_ROOM_SHAPES
                .iter()
                .map(|(sid, slbl)| {
                    button::standard(tr(slbl))
                        .on_press(MapMsg::BoxApplyShape(bid.clone(), (*sid).to_string()))
                        .into()
                })
                .collect();
            col = col.push(dim_caption(tr("Room Shape")));
            col = col.push(flex_row(shape_btns).row_spacing(4).column_spacing(4));
            if b.is_polygon() {
                col = col.push(dim_caption(tr("Drag a midpoint to add a corner, right-click a corner to remove it.")));
            }

            if state.doc.floors.len() > 1 {
                let fl_btns: Vec<Element<MapMsg>> = state
                    .doc
                    .floors
                    .iter()
                    .map(|fl| {
                        let active = b.floor_id == fl.id;
                        let btn = if active { button::suggested(fl.name.clone()) } else { button::standard(fl.name.clone()) };
                        btn.on_press(MapMsg::BoxSetFloor(bid.clone(), fl.id.clone())).into()
                    })
                    .collect();
                col = col.push(dim_caption(tr("Floor")));
                col = col.push(flex_row(fl_btns).row_spacing(4).column_spacing(4));
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

            // Bridge Pro MotionAware™ controls (only when this room has a MotionAware area)
            if let Some(area) = b
                .hue_id
                .as_deref()
                .and_then(|hid| motion_areas.iter().find(|a| a.room_id == hid))
            {
                let area_id_toggle = area.id.clone();
                let status_el: Element<MapMsg> = if area.enabled && area.motion {
                    text::caption(tr("Motion detected"))
                        .class(theme::Text::Custom(|_| iced_text::Style {
                            color: Some(MOTION_ACTIVE_COLOR),
                            ..Default::default()
                        }))
                        .into()
                } else {
                    dim_caption(tr("No motion"))
                };
                let mut ma_row = row![
                    text::caption(format!("◉ {}", tr("MotionAware"))),
                    status_el,
                    space::horizontal(),
                ]
                .spacing(8)
                .align_y(Alignment::Center);

                if let Some(sens) = area.sensitivity {
                    let max_s = area.sensitivity_max.unwrap_or(4).max(1);
                    let aid_dec = area.id.clone();
                    let aid_inc = area.id.clone();
                    ma_row = ma_row
                        .push(dim_caption(tr("Sensitivity")))
                        .push(
                            button::standard("−")
                                .on_press_maybe((sens > 0).then_some(MapMsg::MotionAreaSensitivity(aid_dec, sens - 1))),
                        )
                        .push(text::caption(format!("{sens}/{max_s}")))
                        .push(
                            button::standard("+")
                                .on_press_maybe((sens < max_s).then_some(MapMsg::MotionAreaSensitivity(aid_inc, sens + 1))),
                        );
                }

                ma_row = ma_row.push(
                    toggler(area.enabled)
                        .on_toggle(move |on| MapMsg::MotionAreaToggle(area_id_toggle.clone(), on)),
                );
                col = col.push(divider::horizontal::light());
                col = col.push(ma_row);
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
            let confirming_bridge_delete =
                state.confirm_delete_bridge_box.as_deref() == Some(bid.as_str());
            let mut footer = row![
                button::standard(tr("Sync rooms")).on_press(MapMsg::BoxSyncRoom),
                space::horizontal(),
                button::standard(tr("Remove box")).on_press(MapMsg::BoxRemoveFromCanvas(bid.clone())),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            if b.hue_id.is_some() && !confirming_bridge_delete {
                footer = footer.push(
                    button::destructive(tr("Delete on Bridge"))
                        .on_press(MapMsg::BoxConfirmDeleteOnBridge(bid.clone())),
                );
            }
            col = col.push(divider::horizontal::light());
            col = col.push(footer);
            if b.hue_id.is_some() && confirming_bridge_delete {
                col = col.push(
                    container(
                        column![
                            text::caption(tr("Delete on Bridge?")).class(theme::Text::Custom(
                                |theme: &cosmic::Theme| iced_text::Style {
                                    color: Some(Color::from(theme.cosmic().destructive_color())),
                                    ..Default::default()
                                }
                            )),
                            dim_caption(tr(
                                "This permanently removes the room or zone from your Philips Hue Bridge."
                            )),
                            row![
                                button::standard(tr("Cancel"))
                                    .on_press(MapMsg::BoxCancelDeleteOnBridge),
                                space::horizontal(),
                                button::destructive(tr("Delete"))
                                    .on_press(MapMsg::BoxDeleteOnBridge(bid.clone())),
                            ]
                            .spacing(8)
                            .align_y(Alignment::Center),
                        ]
                        .spacing(6),
                    )
                    .padding(8)
                    .width(Length::Fill)
                    .class(studio_card(skin)),
                );
            }
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

        Selection::Planned(pid) => {
            let pl = state.doc.planned_lights.iter().find(|p| p.id == *pid)?;
            let pid_name = pid.clone();
            let pid_price = pid.clone();
            let real_lights = extract_bridge_lights(stats);
            let mut pcol = column![
                row![
                    text::heading(format!("✦ {} ({} lm · {} K)", pl.name, pl.lumens, pl.kelvin)),
                    space::horizontal(),
                    (if pl.on { button::suggested(tr("On")) } else { button::standard(tr("Off")) })
                        .on_press(MapMsg::TogglePlannedLight(pid.clone())),
                    button::destructive(tr("Delete")).on_press(MapMsg::DeletePlannedLight(pid.clone())),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                row![
                    text_input(tr("Planned Light"), &state.planned_name_draft)
                        .on_input(MapMsg::PlannedNameInput)
                        .on_submit(move |_| MapMsg::PlannedNameSave(pid_name.clone()))
                        .width(Length::Fill),
                    button::suggested(tr("Rename")).on_press(MapMsg::PlannedNameSave(pid.clone())),
                    text_input(tr("Est. Price"), &state.planned_price_draft)
                        .on_input(MapMsg::PlannedPriceInput)
                        .on_submit(move |_| MapMsg::PlannedPriceSave(pid_price.clone()))
                        .width(Length::Fixed(78.0)),
                    button::standard(tr("Save")).on_press(MapMsg::PlannedPriceSave(pid.clone())),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                row![
                    dim_caption(tr("Brightness (Lumens)")),
                    button::standard("−").on_press(MapMsg::PlannedSetLumens(pid.clone(), pl.lumens.saturating_sub(150).max(150))),
                    text::caption(format!("{} lm", pl.lumens)),
                    button::standard("+").on_press(MapMsg::PlannedSetLumens(pid.clone(), (pl.lumens + 150).min(6000))),
                    space::horizontal(),
                    dim_caption(tr("Color Temperature")),
                    button::standard("−").on_press(MapMsg::PlannedSetKelvin(pid.clone(), pl.kelvin.saturating_sub(300).max(2000))),
                    text::caption(format!("{} K", pl.kelvin)),
                    button::standard("+").on_press(MapMsg::PlannedSetKelvin(pid.clone(), (pl.kelvin + 300).min(6500))),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                row![
                    dim_caption(tr("Beam Spread")),
                    button::standard("−").on_press(MapMsg::PlannedSetBeam(pid.clone(), (pl.beam_radius - 0.4).max(1.0))),
                    text::caption(format!("{:.1} m", pl.beam_radius)),
                    button::standard("+").on_press(MapMsg::PlannedSetBeam(pid.clone(), (pl.beam_radius + 0.4).min(6.0))),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            ]
            .spacing(8);

            if !real_lights.is_empty() {
                pcol = pcol.push(divider::horizontal::light());
                pcol = pcol.push(dim_caption(tr("Replace with real paired light")));
                let bind_btns: Vec<Element<MapMsg>> = real_lights
                    .iter()
                    .take(12)
                    .map(|l| {
                        button::standard(l.name.clone())
                            .on_press(MapMsg::BindPlannedToReal {
                                planned_id: pid.clone(),
                                real_light_id: l.id.clone(),
                            })
                            .into()
                    })
                    .collect();
                pcol = pcol.push(flex_row(bind_btns).row_spacing(4).column_spacing(4));
            }
            pcol.into()
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
            points: Vec::new(),
            floor_id: "eg".into(),
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
            px_per_meter: 80.0,
            floors: FloorplanDocument::default().floors,
            active_floor_id: "eg".into(),
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
                ("l1".to_string(), FloorplanLamp { x: 180.0, y: 180.0, z: 0.0, icon: "floor".into(), floor_id: "eg".into() }),
                ("l2".to_string(), FloorplanLamp { x: 520.0, y: 180.0, z: 0.0, icon: "desk".into(), floor_id: "eg".into() }),
            ]
            .into_iter()
            .collect(),
            sensors: std::collections::BTreeMap::new(),
            planned_lights: Vec::new(),
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

    #[test]
    fn tooltip_rect_positions_flips_and_clamps() {
        let bounds = Size::new(600.0, 400.0);

        // Normal center: floats centered above the lamp
        let (pos, sz) = tooltip_rect(Point::new(300.0, 200.0), bounds, "Desk Lamp");
        assert_eq!(sz.height, 24.0);
        let expected_w = (9.0 * 6.8 + 18.0_f32).clamp(44.0, 260.0);
        assert!((sz.width - expected_w).abs() < 0.01);
        assert!((pos.x - (300.0 - sz.width * 0.5)).abs() < 0.01);
        assert!((pos.y - (200.0 - LAMP_RADIUS_PX - 8.0 - 24.0)).abs() < 0.01);

        // Near top edge: flips below the lamp
        let (top_pos, _) = tooltip_rect(Point::new(300.0, 24.0), bounds, "Desk Lamp");
        assert!((top_pos.y - (24.0 + LAMP_RADIUS_PX + 8.0)).abs() < 0.01);

        // Near left edge: clamps to x = 4.0
        let (left_pos, _) = tooltip_rect(Point::new(10.0, 200.0), bounds, "Living Room Ceiling");
        assert_eq!(left_pos.x, 4.0);

        // Near right edge: clamps to bounds.width - width - 4.0
        let (right_pos, right_sz) = tooltip_rect(Point::new(595.0, 200.0), bounds, "Living Room Ceiling");
        assert!((right_pos.x + right_sz.width - (bounds.width - 4.0)).abs() < 0.01);

        // Label width clamping (very short -> 44.0, very long -> 260.0)
        let (_, short_sz) = tooltip_rect(Point::new(300.0, 200.0), bounds, "A");
        assert_eq!(short_sz.width, 44.0);
        let long_name = "Very Long Smart Lamp Name That Exceeds Maximum Pill Width Easily";
        let (_, long_sz) = tooltip_rect(Point::new(300.0, 200.0), bounds, long_name);
        assert_eq!(long_sz.width, 260.0);
    }

    #[test]
    fn lamp_dialog_and_menu_state_helpers() {
        let stats = json!({
            "rooms": [
                {
                    "id": "r1",
                    "kind": "room",
                    "name": "Living Room",
                    "lights": [
                        {"id": "l1", "device_id": "d1", "name": "Arc Floor", "on": true, "bri": 75.0, "color": "#ffaa44"}
                    ]
                }
            ]
        });

        let mut state = FloorplanState::default();
        assert!(!state.any_menu_open());
        assert!(!state.has_dialog_open());

        // Add menu, view menu, and lamp submenu all report via any_menu_open() and clear via close_menus()
        state.add_menu_open = true;
        assert!(state.any_menu_open());
        state.close_menus();
        assert!(!state.add_menu_open);
        assert!(!state.any_menu_open());

        state.view_menu_open = true;
        assert!(state.any_menu_open());
        state.close_menus();
        assert!(!state.view_menu_open);
        assert!(!state.any_menu_open());

        state.open_lamp_menu = Some("l1".into());
        assert!(state.any_menu_open());
        state.close_menus();
        assert!(state.open_lamp_menu.is_none());
        assert!(!state.any_menu_open());

        // Opening Rename dialog closes open_lamp_menu, seeds lamp_rename_draft, and sets has_dialog_open()
        state.open_lamp_menu = Some("l1".into());
        state.open_lamp_dialog(LampDialog::Rename("l1".into()), &stats);
        assert!(state.open_lamp_menu.is_none());
        assert!(state.has_dialog_open());
        assert_eq!(state.lamp_dialog, Some(LampDialog::Rename("l1".into())));
        assert_eq!(state.lamp_rename_draft, "Arc Floor");

        // Closing dialog clears lamp_dialog
        state.close_dialog();
        assert!(!state.has_dialog_open());
        assert!(state.lamp_dialog.is_none());
    }

    #[test]
    fn map_rule_dialog_new_existing_and_save_command() {
        let stats = json!({
            "rooms": [
                {
                    "id": "r1",
                    "kind": "room",
                    "name": "Living Room",
                    "lights": [
                        {"id": "l1", "device_id": "d1", "name": "Arc Floor", "on": true, "bri": 80.0, "color": "#ffaa44"},
                        {"id": "l2", "device_id": "d2", "name": "Desk Strip", "on": false, "bri": 50.0, "color": "#38bdf8"}
                    ]
                }
            ],
            "rules": [
                {
                    "id": "rule-42",
                    "name": "Evening Sync",
                    "enabled": false,
                    "kind": "companion",
                    "trigger_kind": "light",
                    "trigger_id": "l1",
                    "trigger_state": "off",
                    "targets": [{"kind": "light", "id": "l2", "action": "on"}],
                    "mirror_off": false,
                    "match_bri": true,
                    "only_if_off": true,
                    "time_start": "18:00",
                    "time_end": "23:30"
                }
            ]
        });

        // 1. New rule between lamps omits "id" and auto-names with ↔ for follow+mirror_off
        let mut dlg = MapRuleDialog::new_between_lamps(
            "l1".into(),
            "Arc Floor · Living Room".into(),
            "l2".into(),
            "Desk Strip · Living Room".into(),
        );
        assert!(dlg.rule_id.is_none());
        assert!(dlg.enabled);
        assert_eq!(dlg.kind, "follow");
        assert!(dlg.mirror_off);
        assert_eq!(dlg.target_action(), "on");

        let cmd = dlg.to_save_command().expect("valid new rule command");
        assert_eq!(cmd["cmd"], "rule_save");
        assert!(cmd["rule"].get("id").is_none());
        assert_eq!(cmd["rule"]["name"], "Arc Floor ↔ Desk Strip");
        assert_eq!(cmd["rule"]["enabled"], true);
        assert_eq!(cmd["rule"]["mirror_off"], true);
        assert_eq!(cmd["rule"]["targets"][0]["action"], "on");

        // Exclusive rule uses ⊘, action="off", and forces mirror_off/match_bri/only_if_off to false
        dlg.kind = "exclusive".into();
        dlg.match_bri = true;
        dlg.only_if_off = true;
        assert_eq!(dlg.target_action(), "off");
        let excl_cmd = dlg.to_save_command().expect("valid exclusive rule");
        assert_eq!(excl_cmd["rule"]["name"], "Arc Floor ⊘ Desk Strip");
        assert_eq!(excl_cmd["rule"]["targets"][0]["action"], "off");
        assert_eq!(excl_cmd["rule"]["mirror_off"], false);
        assert_eq!(excl_cmd["rule"]["match_bri"], false);
        assert_eq!(excl_cmd["rule"]["only_if_off"], false);

        // Invalid HH:MM returns Err
        dlg.time_start = "25:99".into();
        assert!(dlg.to_save_command().is_err());
        dlg.time_start.clear();

        // Same trigger and target returns Err
        let same_dlg = MapRuleDialog::new_between_lamps("l1".into(), "A".into(), "l1".into(), "A".into());
        assert!(same_dlg.to_save_command().is_err());

        // 2. Existing rule preserves id and fields for in-place update
        let existing = MapRuleDialog::from_existing_rule(&stats["rules"][0], &stats).expect("parsed rule");
        assert_eq!(existing.rule_id.as_deref(), Some("rule-42"));
        assert_eq!(existing.name, "Evening Sync");
        assert!(!existing.enabled);
        assert_eq!(existing.kind, "companion");
        assert_eq!(existing.trigger_state, "off");
        assert_eq!(existing.trigger_name, "Arc Floor");
        assert_eq!(existing.target_name, "Desk Strip");
        assert!(existing.match_bri);
        assert!(existing.only_if_off);
        assert_eq!(existing.time_start, "18:00");
        assert_eq!(existing.time_end, "23:30");

        let upd = existing.to_save_command().expect("valid existing rule update");
        assert_eq!(upd["rule"]["id"], "rule-42");
        assert_eq!(upd["rule"]["name"], "Evening Sync");
        assert_eq!(upd["rule"]["enabled"], false);
        assert_eq!(upd["rule"]["time_start"], "18:00");
        assert_eq!(upd["rule"]["time_end"], "23:30");

        // Missing id returns None
        assert!(MapRuleDialog::from_existing_rule(&json!({"name": "No ID"}), &stats).is_none());

        // 3. FloorplanState dialog lifecycle helpers
        let mut state = FloorplanState::default();
        state.open_lamp_menu = Some("l1".into());
        state.lamp_dialog = Some(LampDialog::Brightness("l1".into()));
        state.pending_wire_from = Some(("light".into(), "l1".into()));
        state.open_new_rule_dialog("light".into(), "l1".into(), "l2".into(), &stats);
        assert!(state.open_lamp_menu.is_none());
        assert!(state.lamp_dialog.is_none());
        assert!(state.pending_wire_from.is_none());
        assert!(state.has_dialog_open());
        let rd = state.rule_dialog.as_ref().expect("rule_dialog open");
        assert!(rd.rule_id.is_none());
        assert_eq!(rd.trigger_name, "Arc Floor");
        assert_eq!(rd.target_name, "Desk Strip");

        state.close_dialog();
        assert!(!state.has_dialog_open());
        assert!(state.rule_dialog.is_none());

        state.open_existing_rule_dialog("rule-42", &stats);
        assert!(state.has_dialog_open());
        assert_eq!(
            state.rule_dialog.as_ref().and_then(|d| d.rule_id.as_deref()),
            Some("rule-42")
        );
        state.close_dialog();
        assert!(!state.has_dialog_open());
    }

    #[test]
    fn map_rule_dialog_handles_motion_contact_and_button_triggers() {
        let stats = json!({
            "rooms": [{
                "id": "r1",
                "kind": "room",
                "name": "Hallway",
                "lights": [{"id": "l1", "device_id": "d1", "name": "Hall Ceiling", "on": false, "bri": 80.0, "color": "#ffd27a"}]
            }],
            "sensors": [
                {"id": "s-mot", "kind": "motion", "name": "Hall Motion", "room_id": "r1", "enabled": true, "motion": false},
                {"id": "s-con", "kind": "contact", "name": "Front Door", "room_id": "r1", "enabled": true, "open": false},
                {"id": "s-btn", "kind": "button", "name": "Wall Switch", "room_id": "r1", "enabled": true}
            ]
        });

        // Motion sensor trigger defaults to trigger_state="motion", mirror_off=true, match_bri forced false
        let mut mot = MapRuleDialog::new_between(
            "motion".into(),
            "s-mot".into(),
            "Hall Motion".into(),
            "l1".into(),
            "Hall Ceiling".into(),
        );
        assert_eq!(mot.trigger_state, "motion");
        assert!(mot.mirror_off);
        mot.match_bri = true;
        let mcmd = mot.to_save_command().expect("motion rule command");
        assert_eq!(mcmd["rule"]["trigger_kind"], "motion");
        assert_eq!(mcmd["rule"]["trigger_state"], "motion");
        assert_eq!(mcmd["rule"]["mirror_off"], true);
        assert_eq!(mcmd["rule"]["match_bri"], false);

        // Contact sensor trigger defaults to trigger_state="open", supports "closed"
        let mut con = MapRuleDialog::new_between(
            "contact".into(),
            "s-con".into(),
            "Front Door".into(),
            "l1".into(),
            "Hall Ceiling".into(),
        );
        assert_eq!(con.trigger_state, "open");
        con.trigger_state = "closed".into();
        let ccmd = con.to_save_command().expect("contact rule command");
        assert_eq!(ccmd["rule"]["trigger_state"], "closed");
        assert_eq!(ccmd["rule"]["match_bri"], false);

        // Button trigger defaults to trigger_state="press", mirror_off forced false
        let mut btn = MapRuleDialog::new_between(
            "button".into(),
            "s-btn".into(),
            "Wall Switch".into(),
            "l1".into(),
            "Hall Ceiling".into(),
        );
        assert_eq!(btn.trigger_state, "press");
        assert!(!btn.mirror_off);
        btn.trigger_state = "long_press".into();
        btn.mirror_off = true;
        let bcmd = btn.to_save_command().expect("button rule command");
        assert_eq!(bcmd["rule"]["trigger_state"], "long_press");
        assert_eq!(bcmd["rule"]["mirror_off"], false);
        assert_eq!(bcmd["rule"]["match_bri"], false);

        // Existing motion rule parses sensor name & "clear" trigger state cleanly
        let existing_json = json!({
            "id": "r-mot-1",
            "name": "Hallway Motion",
            "enabled": true,
            "kind": "follow",
            "trigger_kind": "motion",
            "trigger_id": "s-mot",
            "trigger_state": "clear",
            "targets": [{"kind": "light", "id": "l1", "action": "on"}],
            "mirror_off": true
        });
        let parsed = MapRuleDialog::from_existing_rule(&existing_json, &stats).expect("existing motion rule");
        assert_eq!(parsed.trigger_name, "Hall Motion");
        assert_eq!(parsed.trigger_state, "clear");
    }

    #[test]
    fn floorplan_v2_polygons_multifloor_and_planned_lights() {
        use crate::model::{FloorplanFloor, FloorplanPoint, PlannedLight, kelvin_to_hex, parse_floorplan};

        // 1. L-shape polygon containment excludes the bottom-right notch & computes exact Shoelace area
        let mut lbox = sample_box("b-l", "room", None, "Hallway", 100.0, 100.0, 400.0, 400.0);
        lbox.apply_shape("l_shape");
        assert!(lbox.is_polygon());
        assert_eq!(lbox.points.len(), 6);
        assert!(lbox.contains_point(150.0, 150.0), "top-left inside L");
        assert!(lbox.contains_point(450.0, 150.0), "top-right arm inside L");
        assert!(lbox.contains_point(150.0, 450.0), "bottom-left arm inside L");
        assert!(!lbox.contains_point(450.0, 450.0), "bottom-right notch must be outside L");

        let area_px = lbox.area_px();
        assert!(area_px < 400.0 * 400.0 && area_px > 400.0 * 400.0 * 0.5, "L area={area_px}");
        let area_m2 = lbox.area_m2(80.0);
        assert!((area_m2 - area_px / 6400.0).abs() < 1e-6);

        // 2. Vertex insert, move, translate, remove
        let orig_len = lbox.points.len();
        lbox.insert_vertex(0, 200.0, 100.0);
        assert_eq!(lbox.points.len(), orig_len + 1);
        lbox.move_vertex(1, 220.0, 80.0);
        assert_eq!(lbox.points[1], FloorplanPoint { x: 220.0, y: 80.0 });
        assert!(lbox.y <= 80.0);
        let old_x = lbox.x;
        lbox.translate(50.0, 25.0);
        assert!((lbox.x - (old_x + 50.0)).abs() < 1e-6);
        lbox.remove_vertex(1);
        assert_eq!(lbox.points.len(), orig_len);

        // 3. v1 backward compatibility defaults to 1 floor ("eg") and empty planned_lights
        let v1_stats = json!({
            "floorplan": {
                "version": 1,
                "canvas": {"width": 1200, "height": 800, "grid": 20, "snap": true},
                "boxes": [{"id": "b1", "kind": "room", "name": "Living", "x": 40, "y": 40, "w": 300, "h": 200}],
                "stamps": [],
                "lamps": {"l1": {"x": 120, "y": 120, "z": 0, "icon": "bulb"}},
                "sensors": {}
            }
        });
        let v1_doc = parse_floorplan(&v1_stats).expect("v1 floorplan parses");
        assert_eq!(v1_doc.active_floor_id, "eg");
        assert_eq!(v1_doc.floors.len(), 1);
        assert_eq!(v1_doc.boxes[0].floor_id, "eg");
        assert!(v1_doc.boxes[0].points.is_empty());
        assert!(v1_doc.planned_lights.is_empty());

        // 4. v2 multi-floor + PlannedLight (point & bendable strip) round-trip
        let strip = PlannedLight {
            id: "plan-1".into(),
            floor_id: "og".into(),
            name: "Cove Strip".into(),
            icon: "strip".into(),
            x: 200.0,
            y: 120.0,
            strip_points: vec![
                FloorplanPoint { x: 100.0, y: 120.0 },
                FloorplanPoint { x: 200.0, y: 120.0 },
                FloorplanPoint { x: 300.0, y: 180.0 },
            ],
            lumens: 1600,
            beam_radius: 140.0,
            kelvin: 2700,
            color: kelvin_to_hex(2700),
            on: true,
            price: 89.0,
            model_note: String::new(),
        };
        assert!(strip.is_strip());
        let mut v2_doc = FloorplanDocument::default();
        v2_doc.floors.push(FloorplanFloor { id: "og".into(), name: "Upper Floor (OG)".into(), level: 1 });
        v2_doc.active_floor_id = "og".into();
        v2_doc.boxes.push(lbox);
        v2_doc.planned_lights.push(strip);
        let serialized = json!({"floorplan": v2_doc});
        let parsed = parse_floorplan(&serialized).expect("v2 floorplan round-trips");
        assert_eq!(parsed.floors.len(), 2);
        assert_eq!(parsed.active_floor_id, "og");
        assert_eq!(parsed.planned_lights.len(), 1);
        assert_eq!(parsed.planned_lights[0].strip_points.len(), 3);
        assert_eq!(parsed.planned_lights[0].lumens, 1600);

        // 5. B1 + B2: compute_pending_sync_count respects floor_id, skips non-Hue lights/boxes, and returns 0 when hue_paired=false
        let multi_stats = json!({
            "hue_paired": true,
            "rooms": [
                {
                    "id": "r1", "kind": "room", "name": "Living", "archetype": "living_room", "provider": "hue",
                    "lights": [{"id": "l1", "device_id": "d1", "name": "Lamp 1", "on": true, "bri": 80.0, "color": "#ffd27a"}]
                },
                {
                    "id": "wiz:w1:room", "kind": "room", "name": "WiZ Desk", "provider": "wiz",
                    "lights": [{"id": "wiz:w1:light", "device_id": "", "name": "WiZ", "on": true, "bri": 80.0, "color": "#ffffff"}]
                }
            ]
        });
        let mut sync_doc = FloorplanDocument::default();
        sync_doc.boxes = vec![
            sample_box("b-eg", "room", Some("r1"), "Living", 100.0, 100.0, 300.0, 240.0),
            FloorplanBox {
                floor_id: "og".into(),
                ..sample_box("b-og", "room", Some("r2"), "Upper", 100.0, 100.0, 300.0, 240.0)
            },
            sample_box("b-wiz", "room", Some("wiz:w1:room"), "Renamed WiZ", 500.0, 100.0, 200.0, 200.0),
        ];
        sync_doc.lamps.insert("l1".into(), FloorplanLamp { x: 180.0, y: 180.0, z: 0.0, icon: "bulb".into(), floor_id: "eg".into() });
        sync_doc.lamps.insert("wiz:w1:light".into(), FloorplanLamp { x: 560.0, y: 160.0, z: 0.0, icon: "desk".into(), floor_id: "eg".into() });
        // l1 is in b-eg (r1) on EG; b-og overlaps on OG so must NOT trigger move; b-wiz is non-Hue so must NOT trigger rename!
        assert_eq!(compute_pending_sync_count(&sync_doc, &multi_stats), 0);

        let no_hue_stats = json!({ "hue_paired": false, "rooms": multi_stats["rooms"] });
        sync_doc.boxes.push(sample_box("b-unlinked", "room", None, "New", 750.0, 100.0, 200.0, 200.0));
        assert_eq!(compute_pending_sync_count(&sync_doc, &no_hue_stats), 0);
    }
}
