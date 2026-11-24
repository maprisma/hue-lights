// Hue Lights for COSMIC: Philips Hue rooms, zones, lights, scenes, schedules and rules from the panel.
// Lit bulb in the accent while any light is on, dim bulb when all are off or no bridge
// is paired. Left click opens the popup; right click switches every light off.
//
// All bridge traffic goes through scripts/hue-lights.py (the same helper as the
// GNOME extension), which gets JSON commands on stdin and streams JSON snapshots
// on stdout (every 2 s while the popup is open, every 20 s while it is closed).
// It talks only to the bridge on the LAN, over pinned HTTPS; its app key stays
// in ~/.config/hue-lights/config.json and never reaches this process.

use maprisma_hue_lights::i18n::{read_pref, tr, trf, write_pref};
use maprisma_hue_lights::{corners, shot};
mod model;
mod window_size;
use model::{
    COLOR_PRESETS, CONFIRM_ALL_ON_KEY, DAYS_ORDER, EXCLUDE_ALL_ON_KEY, FAVORITES_KEY, Favorite, RoomFilter, RuleDraft,
    ScheduleDraft, Tab, TargetOption, WARMTH_PRESETS, all_targets, belongs_to_rule, clamp_mirek, day_short_toggle,
    effect_label, favorites_to_value, format_unix_ts, has_active_schedule, is_favorite, parse_favorites,
    parse_string_set, resolve_name, room_matches_filter, schedule_action_summary, schedule_trigger_summary,
    sorted_exclude_list, string_set_to_value, toggle_favorite,
};
use std::collections::{HashMap, HashSet};
use maprisma_hue_lights::{TITLE, WINDOW_BUS, script_path};
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use cosmic::app::{Core, Task};
use cosmic::applet::cosmic_panel_config::PanelAnchor;
use cosmic::applet::{menu_button, padded_control};
use cosmic::iced::futures::channel::mpsc;
use cosmic::iced::futures::{SinkExt, StreamExt};
use cosmic::iced::platform_specific::shell::wayland::commands::popup::destroy_popup;
use cosmic::iced::widget::{container as iced_container, svg as iced_svg, text as iced_text};
use cosmic::iced::{Alignment, Background, Border, Color, Length, Rectangle, Subscription, stream, window};
use cosmic::widget::rectangle_tracker::{RectangleTracker, RectangleUpdate, rectangle_tracker_subscription};
use cosmic::widget::{
    Id, autosize, button, checkbox, column, container, divider, flex_row, icon, mouse_area, row, scrollable, slider,
    space, text, text_input, toggler,
};
use cosmic::{Element, theme};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// Window controls also work for windows launched independently of the tray.
static WINDOW_ACTION: LazyLock<tokio::sync::watch::Sender<Option<bool>>> = LazyLock::new(|| tokio::sync::watch::channel(None).0);
struct WindowControl;
#[zbus::interface(name = "io.github.ubruckhaus.HueLights.Window")]
impl WindowControl {
    fn close(&self) { let _ = WINDOW_ACTION.send_replace(Some(false)); }
    fn show(&self) { let _ = WINDOW_ACTION.send_replace(Some(true)); }
}

// The installed desktop entry for this Wayland app ID supplies the window icon.
const APP_ID: &str = "io.github.ubruckhaus.CosmicMaprismaHueLights";
const WINDOW_APP_ID: &str = "io.github.ubruckhaus.CosmicMaprismaHueLights.Window";
static AUTOSIZE_ID: LazyLock<Id> = LazyLock::new(|| Id::new("hue-lights-autosize"));
static NOTE_ID: LazyLock<Id> = LazyLock::new(|| Id::new("hue-lights-shot-note"));
// F12 pictures go to ~/Pictures/<this>/ (the app's name, the same in every language).
const SHOT_FOLDER: &str = TITLE;
static BULB_ON: &[u8] = include_bytes!("../data/icons/io.github.ubruckhaus.CosmicMaprismaHueLights-symbolic.svg");
static BULB_OFF: &[u8] = include_bytes!("../data/icons/hue-bulb-off-symbolic.svg");

const RESTART_DELAY: Duration = Duration::from_secs(5); // before a helper that exited starts again
const SEND_EVERY: Duration = Duration::from_millis(150); // between brightness commands while dragging
const SCENE_LIMIT: usize = 6; // scenes shown before "+N more"
const POPUP_WIDTH: f32 = 430.0;
const TWO_COL_MIN_WIDTH: f32 = 720.0;
const ROOMS_MAX_HEIGHT: f32 = 540.0;
// A light's own colour when the bridge reports none: data (what the bulb shows), not the theme's.
const WARM: Color = Color::from_rgb(1.0, 0xd2 as f32 / 255.0, 0x7a as f32 / 255.0);

/// A second `--window` start toggles (layout rule 5a): the open window is told to close, the way
/// its tray does, then this one ends. Every entry point (tray, Maprisma Shell's glyph, launcher)
/// gets the same toggle.
async fn close_running_window(bus: &str) {
    if let Ok(connection) = zbus::Connection::session().await {
        let _ = connection.call_method(Some(bus), "/Window", Some("io.github.ubruckhaus.HueLights.Window"), "Close", &()).await;
    }
}

fn main() -> cosmic::iced::Result {
    if std::env::args().any(|arg| arg == "--window") {
        let runtime = tokio::runtime::Runtime::new().expect("window D-Bus runtime");
        let connection = runtime.block_on(async {
            let c = zbus::connection::Builder::session()?.serve_at("/Window", WindowControl)?.build().await?;
            c.request_name_with_flags(WINDOW_BUS, zbus::fdo::RequestNameFlags::DoNotQueue.into()).await?;
            Ok::<_, zbus::Error>(c)
        });
        let _connection = match connection {
            Ok(c) => c,
            Err(zbus::Error::NameTaken) => { runtime.block_on(close_running_window(WINDOW_BUS)); return Ok(()); }
            Err(e) => { eprintln!("Window unavailable: {e}"); return Ok(()); }
        };
        cosmic::app::run::<HueLights<true>>(cosmic::app::Settings::default().default_font(maprisma_hue_lights::look::default_font())
            .size({ let (width, height) = window_start_size(); cosmic::iced::Size::new(width, height) })
            .resizable(Some(8.0)).exit_on_close(true).is_daemon(false), true)
    } else {
        maprisma_hue_lights::look::keep_fonts();
        cosmic::applet::run::<HueLights>(false)
    }
}

// The `--window` size: the one the user left it at (layout rule 5a), else the popup's.
fn window_start_size() -> window_size::Size {
    window_size::parse(read_pref(window_size::KEY).as_ref()).unwrap_or((POPUP_WIDTH, 650.0))
}

/// What Esc does (layout rule 5a): the innermost open thing first, then the window or popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscStep {
    /// Skip the screenshot's note (or close its error).
    Note,
    /// Drop the "Unpair?" or "Turn all lights on?" confirmation.
    Confirm,
    /// Close an open Language / Corners list or inline target picker.
    Menus,
    /// Close an open inline schedule/rule builder or light color drawer.
    Drawer,
    /// Leave the settings page.
    Back,
    /// Close the window (`--window`) or the popup.
    Close,
    Nothing,
}

/// `shown`: Esc came from the window or the open popup (not the panel button); `note`: the
/// screenshot card shows; `confirm`: "Really unpair?" or "Turn all lights on?" shows;
/// `menus`: a Language / Corners / target picker list is open; `drawer`: an inline builder or
/// light color drawer is open; `settings`: the settings page shows.
fn esc_step(shown: bool, note: bool, confirm: bool, menus: bool, drawer: bool, settings: bool) -> EscStep {
    if !shown {
        EscStep::Nothing
    } else if note {
        EscStep::Note
    } else if confirm {
        EscStep::Confirm
    } else if menus {
        EscStep::Menus
    } else if drawer {
        EscStep::Drawer
    } else if settings {
        EscStep::Back
    } else {
        EscStep::Close
    }
}

// ------------------------------------------------------------------------- data

fn num(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(0.0)
}

fn s<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool) == Some(true)
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

// Colours come from the bridge: only "#rrggbb" is used, else the warm white.
fn color(value: &Value) -> Color {
    let hex = value.as_str().unwrap_or("");
    let ok = hex.len() == 7 && hex.starts_with('#') && hex[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !ok {
        return WARM;
    }
    let part = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
    Color::from_rgb8(part(1), part(3), part(5))
}

// Dark or light glyph on top of a light colour, whichever reads better.
fn on_tint(c: Color) -> Color {
    if 0.299 * c.r + 0.587 * c.g + 0.114 * c.b > 0.6 { Color::from_rgb8(0x1a, 0x1a, 0x1a) } else { Color::WHITE }
}

fn with_alpha(mut c: Color, a: f32) -> Color {
    c.a = a;
    c
}

fn room_key(room: &Value) -> String {
    let id = s(room, "id");
    if id.is_empty() { s(room, "kind").to_string() } else { id.to_string() }
}

fn room_name(room: &Value) -> String {
    if s(room, "kind") == "other" { tr("Other lights") } else { s(room, "name").to_string() }
}

// Room type (Hue archetype) to an icon. Many rooms keep the Hue app's default
// archetype, so the name wins when it is telling. None: the bulb.
fn room_icon(room: &Value) -> Option<&'static str> {
    match s(room, "kind") {
        "zone" => return Some("view-grid-symbolic"),
        "other" => return None,
        _ => {}
    }
    let name = s(room, "name").to_lowercase();
    let by_name: [(&[&str], &str); 8] = [
        (&["schlaf", "bed", "kinder", "gäste", "guest", "nursery"], "weather-clear-night-symbolic"),
        (&["küche", "kueche", "kitchen", "ess", "dining"], "emoji-food-symbolic"),
        (&["bad", "bath", "wc", "toilet", "dusche", "shower"], "weather-showers-symbolic"),
        (&["büro", "buero", "office", "arbeit", "studio"], "computer-symbolic"),
        (&["flur", "diele", "treppe", "hall", "stair", "eingang"], "go-home-symbolic"),
        (&["garage", "carport"], "emoji-travel-symbolic"),
        (
            &["garten", "terrasse", "balkon", "garden", "terrace", "balcony", "patio", "außen", "outdoor"],
            "emoji-nature-symbolic",
        ),
        (&["wohn", "living", "lounge", "tv"], "tv-symbolic"),
    ];
    for (words, icon) in by_name {
        if words.iter().any(|w| name.contains(w)) {
            return Some(icon);
        }
    }
    Some(match s(room, "archetype") {
        "living_room" | "lounge" | "tv" => "tv-symbolic",
        "kitchen" | "dining" => "emoji-food-symbolic",
        "bedroom" | "kids_bedroom" | "guest_room" | "nursery" => "weather-clear-night-symbolic",
        "bathroom" | "toilet" => "weather-showers-symbolic",
        "office" | "computer" | "studio" => "computer-symbolic",
        "hallway" | "staircase" | "top_floor" | "upstairs" | "downstairs" => "go-home-symbolic",
        "garage" | "carport" | "driveway" => "emoji-travel-symbolic",
        "garden" | "terrace" | "balcony" | "porch" | "barbecue" | "pool" | "front_door" => "emoji-nature-symbolic",
        _ => "user-home-symbolic",
    })
}

fn room_status(room: &Value, excluded: bool) -> String {
    let lights = list(room, "lights");
    let total = lights.len();
    let mut parts: Vec<String> = Vec::new();
    if s(room, "kind") == "zone" {
        parts.push(tr("Zone"));
    }
    if flag(room, "on") {
        parts.push(trf("{0} of {1} on", &[num(room, "on_count").to_string(), total.to_string()]));
    } else if total == 1 {
        parts.push(tr("Off · 1 light"));
    } else {
        parts.push(trf("Off · {0} lights", &[total.to_string()]));
    }
    let unreachable = lights.iter().filter(|l| l.get("reachable").and_then(Value::as_bool) == Some(false)).count();
    if unreachable > 0 {
        parts.push(trf("{0} unreachable", &[unreachable.to_string()]));
    }
    if excluded {
        parts.push(tr("Excluded"));
    }
    parts.join(" · ")
}

fn bridge_text(stats: &Value) -> String {
    let null = Value::Null;
    let bridge = stats.get("bridge").unwrap_or(&null);
    let name = match s(bridge, "name") {
        "" => "Hue Bridge",
        name => name,
    };
    match s(bridge, "ip") {
        "" => name.to_string(),
        ip => format!("{name} · {ip}"),
    }
}

// Brightness 1..100 % in steps of 5 (1 at the bottom).
fn step(value: f64) -> u8 {
    ((value / 5.0).round() * 5.0).clamp(1.0, 100.0) as u8
}

// ------------------------------------------------------------------------- styles

fn dim_text(theme: &cosmic::Theme) -> iced_text::Style {
    iced_text::Style { color: Some(with_alpha(Color::from(theme.cosmic().on_bg_color()), 0.6)), ..Default::default() }
}

fn dim<'a>(content: impl Into<std::borrow::Cow<'a, str>> + 'a) -> Element<'a, Message> {
    text::caption(content).class(theme::Text::Custom(dim_text)).into()
}

fn error_text<'a>(content: String) -> Element<'a, Message> {
    text::caption(content)
        .class(theme::Text::Custom(|theme: &cosmic::Theme| iced_text::Style {
            color: Some(theme.cosmic().destructive_color().into()),
            ..Default::default()
        }))
        .into()
}

fn tinted_svg(c: Color) -> theme::Svg {
    theme::Svg::custom(move |_| iced_svg::Style { color: Some(c) })
}

// The lit bulb: the theme accent.
fn lit_svg() -> theme::Svg {
    theme::Svg::custom(|theme: &cosmic::Theme| iced_svg::Style { color: Some(theme.cosmic().accent_color().into()) })
}

fn card(tint: Option<Color>) -> theme::Container<'static> {
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let base = Color::from(theme.cosmic().on_bg_color());
        let (bg, border) = match tint {
            Some(t) => (with_alpha(t, 0.12), with_alpha(t, 0.4)),
            None => (with_alpha(base, 0.05), with_alpha(base, 0.1)),
        };
        iced_container::Style {
            background: Some(Background::Color(bg)),
            border: Border { radius: corners::now().radius.into(), width: 1.0, color: border },
            ..Default::default()
        }
    })
}

fn sub_card() -> theme::Container<'static> {
    theme::Container::custom(|theme: &cosmic::Theme| {
        let base = Color::from(theme.cosmic().on_bg_color());
        iced_container::Style {
            background: Some(Background::Color(with_alpha(base, 0.04))),
            border: Border { radius: corners::now().small.into(), width: 1.0, color: with_alpha(base, 0.12) },
            ..Default::default()
        }
    })
}

fn badge(tint: Option<Color>) -> theme::Container<'static> {
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let bg = tint.unwrap_or_else(|| with_alpha(Color::from(theme.cosmic().on_bg_color()), 0.1));
        iced_container::Style {
            background: Some(Background::Color(bg)),
            border: Border { radius: corners::now().small.into(), ..Default::default() },
            ..Default::default()
        }
    })
}

fn pill_style() -> theme::Container<'static> {
    theme::Container::custom(|theme: &cosmic::Theme| iced_container::Style {
        background: Some(Background::Color(with_alpha(Color::from(theme.cosmic().on_bg_color()), 0.12))),
        border: Border { radius: corners::now().pill.into(), ..Default::default() },
        ..Default::default()
    })
}

fn segmented_bar_style() -> theme::Container<'static> {
    theme::Container::custom(|theme: &cosmic::Theme| {
        let base = Color::from(theme.cosmic().on_bg_color());
        iced_container::Style {
            background: Some(Background::Color(with_alpha(base, 0.06))),
            border: Border { radius: corners::now().small.into(), width: 1.0, color: with_alpha(base, 0.12) },
            ..Default::default()
        }
    })
}

fn swatch_sized<'a>(c: Option<Color>, reachable: bool, size: f32) -> Element<'a, Message> {
    container(space::horizontal())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .class(theme::Container::custom(move |theme: &cosmic::Theme| {
            let base = Color::from(theme.cosmic().on_bg_color());
            let fill = c.map_or(with_alpha(base, 0.08), |c| with_alpha(c, if reachable { 1.0 } else { 0.4 }));
            iced_container::Style {
                background: Some(Background::Color(fill)),
                border: Border { radius: corners::now().small.into(), width: 1.0, color: with_alpha(base, 0.3) },
                ..Default::default()
            }
        }))
        .into()
}

fn mini_badge<'a>(label: String) -> Element<'a, Message> {
    container(dim(label)).padding([1, 6]).class(pill_style()).into()
}

fn bulb(on: bool) -> icon::Handle {
    let mut handle = icon::from_svg_bytes(if on { BULB_ON } else { BULB_OFF });
    handle.symbolic = true;
    handle
}

// ------------------------------------------------------------------------- applet

#[derive(Default)]
struct HueLights<const WINDOW_MODE: bool = false> {
    core: Core,
    window_mode: bool,
    window_width: f32,
    popup: Option<window::Id>,
    rectangle: Rectangle,
    tracker: Option<RectangleTracker<u32>>,
    helper: Option<mpsc::UnboundedSender<String>>,
    stats: Value,
    tab: Tab,
    filter: RoomFilter,
    expanded: HashSet<String>,
    all_scenes: HashSet<String>,
    open_light_drawer: Option<String>,
    favorites: Vec<Favorite>,
    exclude_all_on: HashSet<String>,
    confirm_all_on_pref: bool,
    confirm_all_on: bool,
    schedule_draft: Option<ScheduleDraft>,
    rule_draft: Option<RuleDraft>,
    activity_open: bool,
    // A slider being dragged: its value, and when it was last sent.
    dragging: HashMap<String, (u8, Instant)>,
    ip: String,
    confirm_unpair: bool,
    language_error: bool,
    // F12: the note's state, whether a capture is on its way, the settings.
    shot: shot::Phase,
    capturing: bool,
    shot_key: bool,
    corners: String,
    setting_error: bool,
    resize: window_size::Debounce,
    /// The settings page (Language, F12, Corners, Confirm All On) is shown instead of the tabs.
    settings_open: bool,
    language_open: bool,
    corners_open: bool,
}

#[derive(Debug, Clone)]
enum Message {
    SaveSize,
    Helper(mpsc::UnboundedSender<String>),
    Stats(Value),
    TogglePopup,
    Quit,
    Show,
    CloseRequested(window::Id),
    Rectangle(RectangleUpdate<u32>),
    SelectTab(Tab),
    SetFilter(RoomFilter),
    Expand(String),
    AllScenes(String),
    ToggleLightDrawer(String),
    ToggleFavorite(String, String),
    ToggleExcludeAllOn(String, bool),
    LightMirek(String, u16),
    LightXy(String, [f64; 2]),
    LightEffect(String, String),
    RoomOn(String, bool),
    LightOn(String, bool),
    // "room:<key>" or "light:<id>", the value
    Bri(String, u8),
    BriDone(String),
    Scene(String),
    AllOn,
    ConfirmAllOn,
    CancelAllOn,
    AllOff,
    Refresh,
    // Schedules
    StartSchedule,
    StartScheduleFor(String, String, String),
    CancelSchedule,
    ScheduleName(String),
    ScheduleToggleTargetPicker,
    SchedulePickTarget(String, String, String),
    ScheduleTriggerKind(String),
    ScheduleTime(String),
    ScheduleOffsetDelta(i32),
    ScheduleToggleDay(String),
    ScheduleAction(String),
    ScheduleBri(u8),
    ScheduleFade(u16),
    ScheduleSave,
    ScheduleToggle(String, bool),
    ScheduleDelete(String),
    // Rules
    RulesPause(bool),
    StartRule,
    StartRuleFor(String, String, String),
    CancelRule,
    RuleName(String),
    RuleKind(String),
    RuleToggleTriggerPicker,
    RulePickTrigger(String, String, String),
    RuleTriggerState(String),
    RuleToggleTargetPicker,
    RulePickTarget(String, String, String),
    RuleMirrorOff(bool),
    RuleMatchBri(bool),
    RuleOnlyIfOff(bool),
    RuleTimeStart(String),
    RuleTimeEnd(String),
    RuleSave,
    RuleToggle(String, bool),
    RuleDelete(String),
    ToggleActivity,
    // Pairing & settings
    Ip(String),
    Pair(String),
    Discover,
    CancelPair,
    Unpair,
    Language(String),
    ShotKey(window::Id),
    Escape(window::Id),
    Captured(window::Screenshot),
    ShotSaved(Result<shot::Saved, String>),
    NoteInput(String),
    NoteDone,
    NoteSaved(Result<(), String>),
    SetShotKey(bool),
    SetConfirmAllOnPref(bool),
    Corners(String),
    ToggleSettings,
    ToggleLanguage,
    ToggleCorners,
}

// Runs `work` on its own thread (saving must not hold up the popup) and answers with `done`.
fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: fn(Result<T, String>) -> Message,
) -> Task<Message> {
    let (sender, receiver) = cosmic::iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    Task::perform(receiver, move |result| {
        cosmic::Action::App(done(result.unwrap_or_else(|_| Err("the file could not be written".into()))))
    })
}

// Settings from config.json.
fn shot_key_setting() -> bool {
    read_pref("screenshot_key").and_then(|v| v.as_bool()).unwrap_or(true)
}

fn confirm_all_on_setting() -> bool {
    read_pref(CONFIRM_ALL_ON_KEY).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn corners_setting() -> String {
    corners::normalize(read_pref("corners").as_ref().and_then(Value::as_str).unwrap_or("")).to_string()
}

fn favorites_setting() -> Vec<Favorite> {
    parse_favorites(read_pref(FAVORITES_KEY).as_ref())
}

fn exclude_all_on_setting() -> HashSet<String> {
    parse_string_set(read_pref(EXCLUDE_ALL_ON_KEY).as_ref())
}

impl<const WINDOW_MODE: bool> HueLights<WINDOW_MODE> {
    fn send(&self, command: Value) {
        if let Some(helper) = &self.helper {
            let _ = helper.unbounded_send(command.to_string());
        }
    }

    fn send_all_on(&self) {
        self.send(json!({
            "cmd": "all_on",
            "exclude_rooms": sorted_exclude_list(&self.exclude_all_on),
        }));
    }

    fn phase(&self) -> &str {
        match s(&self.stats, "phase") {
            "" => "connecting",
            phase => phase,
        }
    }

    fn paired(&self) -> bool {
        matches!(self.phase(), "ok" | "error" | "connecting")
    }

    fn on_count(&self) -> f64 {
        num(&self.stats, "on_count")
    }

    fn room(&self, key: &str) -> Option<&Value> {
        list(&self.stats, "rooms").iter().find(|r| room_key(r) == key)
    }

    fn find_light(&self, id: &str) -> Option<&Value> {
        list(&self.stats, "rooms")
            .iter()
            .flat_map(|r| list(r, "lights").iter())
            .find(|l| s(l, "id") == id)
    }

    // Rooms with a grouped light are switched with one bridge command; tr("Other lights") per light.
    fn set_room(&self, key: &str, field: &str, value: Value) {
        let Some(room) = self.room(key) else { return };
        let grouped = s(room, "grouped_light");
        if !grouped.is_empty() {
            let mut command = json!({"cmd": "group", "id": grouped});
            command[field] = value;
            self.send(command);
            return;
        }
        for light in list(room, "lights") {
            let mut command = json!({"cmd": "light", "id": s(light, "id")});
            command[field] = value.clone();
            self.send(command);
        }
    }

    fn send_bri(&self, target: &str, bri: u8) {
        if let Some(key) = target.strip_prefix("room:") {
            self.set_room(key, "bri", json!(bri));
        } else if let Some(id) = target.strip_prefix("light:") {
            self.send(json!({"cmd": "light", "id": id, "bri": bri}));
        }
    }

    // The slider's value: the dragged one, else the bridge's.
    fn bri_of(&self, target: &str, bridge: f64) -> u8 {
        self.dragging.get(target).map_or_else(|| step(bridge.max(1.0)), |(v, _)| *v)
    }

    fn summary(&self) -> String {
        maprisma_hue_lights::summary(&self.stats)
    }

    fn any_picker_open(&self) -> bool {
        self.schedule_draft.as_ref().is_some_and(|d| d.target_picker_open)
            || self.rule_draft.as_ref().is_some_and(|d| d.trigger_picker_open || d.target_picker_open)
    }

    fn close_pickers(&mut self) {
        if let Some(d) = &mut self.schedule_draft {
            d.target_picker_open = false;
        }
        if let Some(d) = &mut self.rule_draft {
            d.trigger_picker_open = false;
            d.target_picker_open = false;
        }
    }

    fn any_drawer_open(&self) -> bool {
        self.open_light_drawer.is_some() || self.schedule_draft.is_some() || self.rule_draft.is_some()
    }

    fn panel_content(&self) -> Element<'_, Message> {
        let lit = self.on_count() > 0.0;
        let size = self.core.applet.suggested_size(true).0;
        let mut bulb = icon::icon(bulb(lit)).size(size);
        if lit {
            bulb = bulb.class(lit_svg());
        } else {
            bulb = bulb.opacity(0.6);
        }
        bulb.into()
    }

    fn header(&self) -> Element<'_, Message> {
        let lit = self.on_count() > 0.0;
        let mut hero = icon::icon(bulb(lit)).size(32);
        hero = if lit { hero.class(lit_svg()) } else { hero.opacity(0.6) };
        let meta = if self.paired() { bridge_text(&self.stats) } else { tr("No bridge paired") };
        let mut header = row![hero, column![text::title4(tr("Maprisma Hue Lights")), dim(meta)].spacing(4), space::horizontal()]
            .spacing(12)
            .align_y(Alignment::Center);
        if self.phase() == "ok" {
            let pill = trf("{0} of {1} on", &[self.on_count().to_string(), num(&self.stats, "light_count").to_string()]);
            header = header.push(container(text::caption(pill)).padding([4, 8]).class(pill_style()));
        }
        // Settings at the header's right end, as in Maprisma Launcher.
        header = header.push(cosmic::widget::tooltip(
            button::icon(icon::from_name("emblem-system-symbolic").symbolic(true)).on_press(Message::ToggleSettings),
            text::body(tr("Settings")),
            cosmic::widget::tooltip::Position::Bottom,
        ));
        header.into()
    }

    fn tab_bar(&self) -> Element<'_, Message> {
        let sch_count = list(&self.stats, "schedules").len();
        let rule_count = list(&self.stats, "rules").len();
        let paused = flag(&self.stats, "rules_paused");

        let tab_btn = |tab: Tab, label: String, count: usize, dimmed: bool| -> Element<'_, Message> {
            let active = self.tab == tab;
            let text_el: Element<Message> = if dimmed && !active {
                dim(label)
            } else {
                text::caption(label).into()
            };
            let mut inner = row![text_el].spacing(6).align_y(Alignment::Center);
            if count > 0 {
                inner = inner.push(container(text::caption(count.to_string())).padding([1, 6]).class(pill_style()));
            }
            if dimmed {
                inner = inner.push(dim("⏸"));
            }
            button::custom(container(inner).width(Length::Fill).align_x(Alignment::Center))
                .width(Length::FillPortion(1))
                .padding([6, 8])
                .class(if active { theme::Button::Suggested } else { theme::Button::MenuItem })
                .on_press(Message::SelectTab(tab))
                .into()
        };

        container(
            row![
                tab_btn(Tab::Lights, tr("Lights"), 0, false),
                tab_btn(Tab::Schedules, tr("Schedules"), sch_count, false),
                tab_btn(Tab::Rules, tr("Rules"), rule_count, paused),
            ]
            .spacing(4),
        )
        .padding(4)
        .width(Length::Fill)
        .class(segmented_bar_style())
        .into()
    }

    fn pairing(&self) -> Element<'_, Message> {
        let stats = &self.stats;
        let pairing = self.phase() == "pairing";
        let mut content = column![text::heading(tr("Pair a Hue bridge"))].spacing(8);
        if pairing {
            content = content.push(text::body(trf("Press the round link button on top of the bridge now. Waiting {0} s for {1} …", &[num(stats, "pair_remaining").to_string(), s(stats, "pair_ip").to_string()])));
            content = content.push(button::standard(tr("Cancel")).on_press(Message::CancelPair));
            return content.into();
        }
        content = content.push(dim(tr("Click Pair, then press the round link button on top of the bridge within 30 seconds.")));
        let candidates = list(stats, "candidates");
        for candidate in candidates {
            let name = match s(candidate, "name") {
                "" => "Hue Bridge",
                n => n,
            };
            let ip = s(candidate, "ip").to_string();
            content = content.push(
                row![
                    text::body(format!("{name}  ·  {ip}")),
                    space::horizontal(),
                    button::suggested(tr("Pair")).on_press(Message::Pair(ip))
                ]
                .align_y(Alignment::Center),
            );
        }
        let discovering = flag(stats, "discovering");
        if candidates.is_empty() {
            content = content.push(dim(if discovering {
                tr("Searching the network for Hue bridges …")
            } else {
                tr("No bridge found automatically. Enter its IP address:")
            }));
        }
        let ip = self.ip.trim().to_string();
        content = content.push(
            row![
                text_input(tr("Bridge IP, e.g. 192.168.178.106"), &self.ip)
                    .on_input(Message::Ip)
                    .on_submit(|value: String| Message::Pair(value.trim().to_string()))
                    .width(Length::Fill),
                button::standard(tr("Pair")).on_press_maybe((!ip.is_empty()).then_some(Message::Pair(ip)))
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
        content = content.push(button::standard(tr("Search again")).on_press_maybe((!discovering).then_some(Message::Discover)));
        content.into()
    }

    // --------------------------------------------------------------------- Lights tab

    fn favorites_strip(&self) -> Option<Element<'_, Message>> {
        if self.favorites.is_empty() {
            return None;
        }
        let mut pills: Vec<Element<Message>> = Vec::new();
        for fav in &self.favorites {
            if fav.kind == "room" {
                if let Some(room) = self.room(&fav.id) {
                    let lit = flag(room, "on");
                    let c = color(room.get("color").unwrap_or(&Value::Null));
                    let name = room_name(room);
                    let id = fav.id.clone();
                    let inner = row![swatch_sized(lit.then_some(c), true, 10.0), text::caption(name)]
                        .spacing(6)
                        .align_y(Alignment::Center);
                    let b = button::custom(inner)
                        .padding([5, 10])
                        .class(if lit { theme::Button::Suggested } else { theme::Button::Standard })
                        .on_press(Message::RoomOn(id, !lit));
                    pills.push(b.into());
                }
            } else if fav.kind == "light" {
                if let Some(light) = self.find_light(&fav.id) {
                    let lit = flag(light, "on");
                    let reachable = light.get("reachable").and_then(Value::as_bool) != Some(false);
                    let c = color(light.get("color").unwrap_or(&Value::Null));
                    let name = s(light, "name").to_string();
                    let id = fav.id.clone();
                    let inner = row![swatch_sized(lit.then_some(c), reachable, 10.0), text::caption(name)]
                        .spacing(6)
                        .align_y(Alignment::Center);
                    let b = button::custom(inner)
                        .padding([5, 10])
                        .class(if lit { theme::Button::Suggested } else { theme::Button::Standard })
                        .on_press(Message::LightOn(id, !lit));
                    pills.push(b.into());
                }
            }
        }
        if pills.is_empty() {
            return None;
        }
        Some(
            column![
                dim(tr("Pinned")),
                flex_row(pills).row_spacing(6).column_spacing(6),
            ]
            .spacing(6)
            .into(),
        )
    }

    fn filter_bar(&self) -> Element<'_, Message> {
        let mut filters: Vec<(RoomFilter, String)> = vec![
            (RoomFilter::All, tr("All")),
            (RoomFilter::On, tr("On")),
            (RoomFilter::Rooms, tr("Rooms")),
            (RoomFilter::Zones, tr("Zones")),
        ];
        if !self.favorites.is_empty() {
            filters.push((RoomFilter::Favorites, tr("Favorites")));
        }
        let pills: Vec<Element<Message>> = filters
            .into_iter()
            .map(|(f, label)| {
                let b = if self.filter == f { button::suggested(label) } else { button::standard(label) };
                b.on_press(Message::SetFilter(f)).into()
            })
            .collect();
        flex_row(pills).row_spacing(6).column_spacing(6).into()
    }

    fn lights_tab(&self) -> Element<'_, Message> {
        let rooms = list(&self.stats, "rooms");
        if rooms.is_empty() {
            return dim(if self.phase() == "ok" {
                tr("No lights on this bridge yet.")
            } else {
                tr("Connecting to the bridge …")
            });
        }
        let mut content = column![].spacing(10);
        if let Some(strip) = self.favorites_strip() {
            content = content.push(strip);
        }
        content = content.push(self.filter_bar());

        let filtered: Vec<&Value> = rooms
            .iter()
            .filter(|r| room_matches_filter(r, self.filter, &self.favorites))
            .collect();

        if filtered.is_empty() {
            content = content.push(container(dim(tr("No lights match this filter."))).padding([12, 4]));
            return content.into();
        }

        let two_cols = self.window_mode && self.window_width >= TWO_COL_MIN_WIDTH;
        let cards_area: Element<Message> = if two_cols {
            let mut left = column![].spacing(12).width(Length::FillPortion(1));
            let mut right = column![].spacing(12).width(Length::FillPortion(1));
            for (i, room) in filtered.iter().enumerate() {
                if i % 2 == 0 {
                    left = left.push(self.room_card(room));
                } else {
                    right = right.push(self.room_card(room));
                }
            }
            row![left, right].spacing(12).align_y(Alignment::Start).into()
        } else {
            let mut cards = column![].spacing(12);
            for room in filtered {
                cards = cards.push(self.room_card(room));
            }
            cards.into()
        };

        if self.window_mode {
            content = content.push(cards_area);
        } else {
            content = content.push(
                container(scrollable(container(cards_area).padding([0, 12, 0, 0])).height(Length::Shrink))
                    .max_height(ROOMS_MAX_HEIGHT),
            );
        }
        content.into()
    }

    fn room_card<'a>(&'a self, room: &'a Value) -> Element<'a, Message> {
        let key = room_key(room);
        let kind = s(room, "kind").to_string();
        let lit = flag(room, "on");
        let tint = lit.then(|| color(room.get("color").unwrap_or(&Value::Null)));
        let open = self.expanded.contains(&key);
        let excluded = self.exclude_all_on.contains(&key);
        let pinned = is_favorite(&self.favorites, "room", &key);
        let rname = room_name(room);

        let glyph: Element<Message> = match room_icon(room) {
            Some(name) => {
                let mut i = icon::from_name(name).size(18).symbolic(true).icon();
                i = match tint {
                    Some(t) => i.class(tinted_svg(on_tint(t))),
                    None => i.opacity(0.6),
                };
                i.into()
            }
            None => {
                let mut i = icon::icon(bulb(true)).size(18);
                i = match tint {
                    Some(t) => i.class(tinted_svg(on_tint(t))),
                    None => i.opacity(0.6),
                };
                i.into()
            }
        };
        let badge = container(glyph)
            .width(Length::Fixed(32.0))
            .height(Length::Fixed(32.0))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .class(badge(tint));
        let chevron = icon::from_name(if open { "pan-down-symbolic" } else { "pan-end-symbolic" }).size(16).symbolic(true);
        let names = column![text::heading(rname.clone()), dim(room_status(room, excluded))].spacing(4);
        let expander = button::custom(
            row![badge, names, space::horizontal(), chevron].spacing(12).align_y(Alignment::Center),
        )
        .padding(4)
        .width(Length::Fill)
        .class(theme::Button::MenuItem)
        .on_press(Message::Expand(key.clone()));
        let switch_key = key.clone();
        let head = row![expander, toggler(lit).on_toggle(move |on| Message::RoomOn(switch_key.clone(), on))]
            .spacing(8)
            .align_y(Alignment::Center);
        let mut card_content = column![head].spacing(8);

        if lit {
            let target = format!("room:{key}");
            let bri = self.bri_of(&target, num(room, "bri"));
            let t1 = target.clone();
            card_content = card_content.push(
                row![
                    slider(0..=100, bri, move |v| Message::Bri(t1.clone(), v))
                        .step(5u8)
                        .on_release(Message::BriDone(target))
                        .width(Length::Fill),
                    container(text::caption(format!("{bri}%"))).width(Length::Fixed(40.0)).align_x(Alignment::End),
                ]
                .spacing(8)
                .padding([0, 4])
                .align_y(Alignment::Center),
            );
        }

        if open {
            for light in list(room, "lights") {
                card_content = card_content.push(self.light_row(light));
                if self.open_light_drawer.as_deref() == Some(s(light, "id")) {
                    card_content = card_content.push(self.light_drawer(light));
                }
            }
            let scenes = list(room, "scenes");
            if !scenes.is_empty() {
                let all = self.all_scenes.contains(&key);
                let mut pills: Vec<Element<Message>> = scenes
                    .iter()
                    .enumerate()
                    .filter(|(i, scene)| all || *i < SCENE_LIMIT || flag(scene, "active"))
                    .map(|(_, scene)| {
                        let name = s(scene, "name").to_string();
                        let b = if flag(scene, "active") { button::suggested(name) } else { button::standard(name) };
                        b.on_press(Message::Scene(s(scene, "id").to_string())).into()
                    })
                    .collect();
                if scenes.len() > SCENE_LIMIT {
                    let shown = pills.len();
                    let label = if all { tr("less") } else { trf("+{0} more", &[(scenes.len() - shown).to_string()]) };
                    pills.push(button::text(label).on_press(Message::AllScenes(key.clone())).into());
                }
                card_content =
                    card_content.push(container(flex_row(pills).row_spacing(8).column_spacing(8)).padding([4, 4, 0, 4]));
            }
            // Room quick actions: Pin, Schedule, Link rule, Exclude from All On.
            let mut room_actions: Vec<Element<Message>> = vec![
                button::standard(if pinned { tr("Unpin") } else { tr("Pin") })
                    .on_press(Message::ToggleFavorite("room".into(), key.clone()))
                    .into(),
            ];
            if matches!(kind.as_str(), "room" | "zone") {
                room_actions.push(
                    button::standard(tr("Schedule"))
                        .on_press(Message::StartScheduleFor(kind.clone(), key.clone(), rname.clone()))
                        .into(),
                );
                room_actions.push(
                    button::standard(tr("Link rule"))
                        .on_press(Message::StartRuleFor(kind.clone(), key.clone(), rname.clone()))
                        .into(),
                );
            }
            card_content = card_content.push(divider::horizontal::light());
            card_content = card_content.push(
                container(flex_row(room_actions).row_spacing(6).column_spacing(6)).padding([2, 4, 0, 4]),
            );
            if matches!(kind.as_str(), "room" | "zone") {
                let ex_key = key.clone();
                card_content = card_content.push(
                    container(
                        checkbox(excluded)
                            .label(tr("Exclude from All On"))
                            .on_toggle(move |v| Message::ToggleExcludeAllOn(ex_key.clone(), v)),
                    )
                    .padding([0, 4]),
                );
            }
        }
        container(card_content).padding(12).width(Length::Fill).class(card(tint)).into()
    }

    fn light_row<'a>(&'a self, light: &'a Value) -> Element<'a, Message> {
        let id = s(light, "id").to_string();
        let on = flag(light, "on");
        let reachable = light.get("reachable").and_then(Value::as_bool) != Some(false);
        let c = color(light.get("color").unwrap_or(&Value::Null));
        let drawer_open = self.open_light_drawer.as_deref() == Some(id.as_str());
        let mut name = s(light, "name").to_string();
        if !reachable {
            name += &format!(" · {}", tr("unreachable"));
        }
        let name_el: Element<Message> = if on && reachable { text::body(name).into() } else { dim(name) };

        let has_sched = has_active_schedule(&self.stats, "light", &id);
        let has_rule = belongs_to_rule(&self.stats, "light", &id);
        let label_col: Element<Message> = if has_sched || has_rule {
            let mut badges = row![].spacing(4).align_y(Alignment::Center);
            if has_sched {
                badges = badges.push(mini_badge(tr("Schedule")));
            }
            if has_rule {
                badges = badges.push(mini_badge(tr("Rule")));
            }
            column![name_el, badges].spacing(2).into()
        } else {
            name_el
        };

        let dot_btn = cosmic::widget::tooltip(
            button::custom(swatch_sized(on.then_some(c), reachable, 14.0))
                .padding(4)
                .class(if drawer_open { theme::Button::Suggested } else { theme::Button::MenuItem })
                .on_press(Message::ToggleLightDrawer(id.clone())),
            text::body(tr("Color, warmth & effects")),
            cosmic::widget::tooltip::Position::Top,
        );

        let mut line = row![dot_btn, container(label_col).width(Length::Fill)]
            .spacing(6)
            .align_y(Alignment::Center);
        if flag(light, "dimmable") {
            let target = format!("light:{id}");
            let bri = if on { self.bri_of(&target, num(light, "bri")) } else { 1 };
            let t1 = target.clone();
            line = line.push(
                slider(0..=100, bri, move |v| Message::Bri(t1.clone(), v))
                    .step(5u8)
                    .on_release(Message::BriDone(target))
                    .width(Length::Fixed(104.0)),
            );
        }
        let switch_id = id.clone();
        line = line.push(toggler(on).on_toggle(move |v| Message::LightOn(switch_id.clone(), v)));
        container(line).padding([4, 4, 4, 8]).into()
    }

    fn light_drawer<'a>(&'a self, light: &'a Value) -> Element<'a, Message> {
        let id = s(light, "id").to_string();
        let name = s(light, "name").to_string();
        let supports_ct = flag(light, "supports_ct");
        let supports_color = flag(light, "supports_color");
        let current_mirek = light.get("mirek").and_then(Value::as_i64);
        let current_effect = s(light, "effect");
        let effects = list(light, "available_effects");
        let pinned = is_favorite(&self.favorites, "light", &id);

        let mut drawer = column![].spacing(8);

        if supports_ct {
            let pills: Vec<Element<Message>> = WARMTH_PRESETS
                .iter()
                .map(|preset| {
                    let target = clamp_mirek(light, preset.mirek);
                    let active = current_mirek.is_some_and(|m| (m - target as i64).abs() <= 25);
                    let inner = row![
                        swatch_sized(Some(preset.swatch), true, 10.0),
                        text::caption(tr(preset.label_key)),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center);
                    button::custom(inner)
                        .padding([4, 8])
                        .class(if active { theme::Button::Suggested } else { theme::Button::Standard })
                        .on_press(Message::LightMirek(id.clone(), target))
                        .into()
                })
                .collect();
            drawer = drawer.push(column![dim(tr("Warmth")), flex_row(pills).row_spacing(6).column_spacing(6)].spacing(4));
        }

        if supports_color {
            let swatches: Vec<Element<Message>> = COLOR_PRESETS
                .iter()
                .map(|preset| {
                    let lid = id.clone();
                    let xy = preset.xy;
                    button::custom(swatch_sized(Some(preset.swatch), true, 18.0))
                        .padding(4)
                        .class(theme::Button::Standard)
                        .on_press(Message::LightXy(lid, xy))
                        .into()
                })
                .collect();
            drawer = drawer.push(column![dim(tr("Color")), flex_row(swatches).row_spacing(6).column_spacing(6)].spacing(4));
        }

        if !effects.is_empty() {
            let chips: Vec<Element<Message>> = effects
                .iter()
                .filter_map(Value::as_str)
                .map(|eff| {
                    let active = eff == current_effect || (current_effect.is_empty() && eff == "no_effect");
                    let b = if active { button::suggested(effect_label(eff)) } else { button::standard(effect_label(eff)) };
                    b.on_press(Message::LightEffect(id.clone(), eff.to_string())).into()
                })
                .collect();
            if !chips.is_empty() {
                drawer = drawer.push(column![dim(tr("Effects")), flex_row(chips).row_spacing(6).column_spacing(6)].spacing(4));
            }
        }

        let quick: Vec<Element<Message>> = vec![
            button::standard(if pinned { tr("Unpin") } else { tr("Pin") })
                .on_press(Message::ToggleFavorite("light".into(), id.clone()))
                .into(),
            button::standard(tr("Schedule"))
                .on_press(Message::StartScheduleFor("light".into(), id.clone(), name.clone()))
                .into(),
            button::standard(tr("Link rule"))
                .on_press(Message::StartRuleFor("light".into(), id, name))
                .into(),
        ];
        drawer = drawer.push(flex_row(quick).row_spacing(6).column_spacing(6));

        container(container(drawer).padding(10).width(Length::Fill).class(sub_card()))
            .padding([0, 4, 4, 16])
            .into()
    }

    // --------------------------------------------------------------------- Schedules tab

    fn schedules_tab(&self) -> Element<'_, Message> {
        let mut content = column![].spacing(10);
        if self.schedule_draft.is_none() {
            content = content.push(
                row![
                    space::horizontal(),
                    button::suggested(tr("+ New schedule")).on_press(Message::StartSchedule),
                ]
                .align_y(Alignment::Center),
            );
        }

        if let Some(draft) = &self.schedule_draft {
            content = content.push(self.schedule_builder(draft));
        }

        let schedules = list(&self.stats, "schedules");
        let list_view: Element<Message> = if schedules.is_empty() {
            container(dim(tr("No schedules yet."))).padding([8, 4]).into()
        } else {
            let mut items = column![].spacing(10);
            for sch in schedules {
                items = items.push(self.schedule_card(sch));
            }
            items.into()
        };

        if self.window_mode {
            content = content.push(list_view);
        } else {
            content = content.push(
                container(scrollable(container(list_view).padding([0, 12, 0, 0])).height(Length::Shrink))
                    .max_height(ROOMS_MAX_HEIGHT),
            );
        }
        content.into()
    }

    fn schedule_card<'a>(&'a self, sch: &'a Value) -> Element<'a, Message> {
        let id = s(sch, "id").to_string();
        let enabled = flag(sch, "enabled");
        let name = match s(sch, "name") {
            "" => tr("Schedule"),
            n => n.to_string(),
        };
        let target_name = match s(sch, "target_name") {
            "" => resolve_name(&self.stats, s(sch, "target_kind"), s(sch, "target_id")),
            n => n.to_string(),
        };
        let trigger = schedule_trigger_summary(sch);
        let action = schedule_action_summary(sch);
        let source = s(sch, "source");

        let mut head_left = row![text::heading(name)].spacing(8).align_y(Alignment::Center);
        if source == "bridge" {
            head_left = head_left.push(mini_badge(tr("Bridge")));
        }

        let toggle_id = id.clone();
        let head = row![
            head_left,
            space::horizontal(),
            toggler(enabled).on_toggle(move |v| Message::ScheduleToggle(toggle_id.clone(), v)),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let meta = format!("{target_name} · {trigger}");
        let foot = row![
            dim(action),
            space::horizontal(),
            button::destructive(tr("Delete")).on_press(Message::ScheduleDelete(id)),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        container(column![head, dim(meta), foot].spacing(6))
            .padding(12)
            .width(Length::Fill)
            .class(card(None))
            .into()
    }

    fn schedule_builder<'a>(&'a self, draft: &'a ScheduleDraft) -> Element<'a, Message> {
        let mut form = column![
            text::heading(tr("+ New schedule")),
            text_input(tr("Name (optional)"), &draft.name).on_input(Message::ScheduleName),
        ]
        .spacing(8);

        // Target picker
        let target_label = if draft.target_name.is_empty() { tr("Select target") } else { draft.target_name.clone() };
        form = form.push(
            row![
                dim(tr("Target")),
                space::horizontal(),
                button::text(target_label)
                    .trailing_icon(icon::from_name(if draft.target_picker_open { "pan-up-symbolic" } else { "pan-down-symbolic" }).size(16))
                    .on_press(Message::ScheduleToggleTargetPicker),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
        if draft.target_picker_open {
            let mut opts = column![].spacing(2);
            for opt in all_targets(&self.stats) {
                let selected = opt.kind == draft.target_kind && opt.id == draft.target_id;
                let label = if selected { format!("✓ {}", opt.label) } else { opt.label.clone() };
                opts = opts.push(
                    menu_button(text::caption(label)).on_press(Message::SchedulePickTarget(opt.kind, opt.id, opt.label)),
                );
            }
            form = form.push(
                container(scrollable(opts).height(Length::Shrink))
                    .max_height(160.0)
                    .padding(4)
                    .width(Length::Fill)
                    .class(sub_card()),
            );
        }

        // Trigger kind pills
        let trigger_pills: Vec<Element<Message>> = [
            ("time", tr("Time")),
            ("sunrise", tr("Sunrise")),
            ("sunset", tr("Sunset")),
        ]
        .into_iter()
        .map(|(k, label)| {
            let b = if draft.trigger_kind == k { button::suggested(label) } else { button::standard(label) };
            b.on_press(Message::ScheduleTriggerKind(k.into())).into()
        })
        .collect();
        form = form.push(column![dim(tr("Trigger")), flex_row(trigger_pills).row_spacing(6).column_spacing(6)].spacing(4));

        // Time or solar offset
        if draft.trigger_kind == "time" {
            form = form.push(
                row![
                    dim(tr("Time")),
                    space::horizontal(),
                    text_input("07:00", &draft.time)
                        .on_input(Message::ScheduleTime)
                        .width(Length::Fixed(100.0)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        } else {
            let offset_text = if draft.offset_min > 0 {
                format!("+{}", trf("{0} min", &[draft.offset_min.to_string()]))
            } else {
                trf("{0} min", &[draft.offset_min.to_string()])
            };
            form = form.push(
                row![
                    dim(tr("Offset")),
                    space::horizontal(),
                    button::standard("-15").on_press(Message::ScheduleOffsetDelta(-15)),
                    button::standard("-5").on_press(Message::ScheduleOffsetDelta(-5)),
                    text::body(offset_text),
                    button::standard("+5").on_press(Message::ScheduleOffsetDelta(5)),
                    button::standard("+15").on_press(Message::ScheduleOffsetDelta(15)),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            );
        }

        // Weekday toggles
        let day_btns: Vec<Element<Message>> = DAYS_ORDER
            .iter()
            .map(|code| {
                let active = draft.days.contains(*code);
                let label = day_short_toggle(code);
                let b = if active { button::suggested(label) } else { button::standard(label) };
                b.on_press(Message::ScheduleToggleDay((*code).to_string())).into()
            })
            .collect();
        form = form.push(column![dim(tr("Days")), flex_row(day_btns).row_spacing(4).column_spacing(4)].spacing(4));

        // Action & brightness
        let action_pills: Vec<Element<Message>> = [("on", tr("On")), ("off", tr("Off"))]
            .into_iter()
            .map(|(k, label)| {
                let b = if draft.action == k { button::suggested(label) } else { button::standard(label) };
                b.on_press(Message::ScheduleAction(k.into())).into()
            })
            .collect();
        form = form.push(
            row![
                dim(tr("Action")),
                space::horizontal(),
                flex_row(action_pills).row_spacing(6).column_spacing(6),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );

        if draft.action == "on" {
            form = form.push(
                row![
                    dim(tr("Brightness")),
                    slider(1..=100, draft.bri, Message::ScheduleBri).step(5u8).width(Length::Fill),
                    container(text::caption(format!("{}%", draft.bri))).width(Length::Fixed(40.0)).align_x(Alignment::End),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }

        // Fade minutes
        let fade_pills: Vec<Element<Message>> = [0u16, 5, 10, 15, 30]
            .into_iter()
            .map(|m| {
                let label = if m == 0 { tr("None") } else { trf("{0} min", &[m.to_string()]) };
                let b = if draft.fade_min == m { button::suggested(label) } else { button::standard(label) };
                b.on_press(Message::ScheduleFade(m)).into()
            })
            .collect();
        form = form.push(column![dim(tr("Fade")), flex_row(fade_pills).row_spacing(6).column_spacing(6)].spacing(4));

        if let Some(err) = &draft.error {
            form = form.push(error_text(err.clone()));
        }

        form = form.push(
            row![
                space::horizontal(),
                button::standard(tr("Cancel")).on_press(Message::CancelSchedule),
                button::suggested(tr("Save")).on_press(Message::ScheduleSave),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );

        container(form).padding(12).width(Length::Fill).class(card(None)).into()
    }

    // --------------------------------------------------------------------- Rules tab

    fn rules_tab(&self) -> Element<'_, Message> {
        let paused = flag(&self.stats, "rules_paused");
        let rules = list(&self.stats, "rules");
        let log = list(&self.stats, "rule_log");

        let mut head = row![
            text::body(tr("Pause all rules")),
            toggler(paused).on_toggle(Message::RulesPause),
            space::horizontal(),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if self.rule_draft.is_none() {
            head = head.push(button::suggested(tr("+ New rule")).on_press(Message::StartRule));
        }

        let mut content = column![head].spacing(10);
        if paused {
            content = content.push(dim(tr("Rules are paused")));
        }

        if let Some(draft) = &self.rule_draft {
            content = content.push(self.rule_builder(draft));
        }

        let mut body = column![].spacing(10);
        if rules.is_empty() {
            body = body.push(container(dim(tr("No rules yet."))).padding([8, 4]));
        } else {
            for rule in rules {
                body = body.push(self.rule_card(rule, paused));
            }
        }

        // Expandable Recent activity section
        let chevron = icon::from_name(if self.activity_open { "pan-down-symbolic" } else { "pan-end-symbolic" })
            .size(16)
            .symbolic(true);
        let mut act_head = row![text::body(tr("Recent activity"))].spacing(8).align_y(Alignment::Center);
        if !log.is_empty() {
            act_head = act_head.push(mini_badge(log.len().to_string()));
        }
        act_head = act_head.push(space::horizontal()).push(chevron);
        body = body.push(divider::horizontal::default());
        body = body.push(
            button::custom(act_head)
                .padding([6, 4])
                .width(Length::Fill)
                .class(theme::Button::MenuItem)
                .on_press(Message::ToggleActivity),
        );

        if self.activity_open {
            if log.is_empty() {
                body = body.push(container(dim(tr("No recent activity."))).padding([4, 8]));
            } else {
                let mut entries = column![].spacing(6);
                for entry in log.iter().rev() {
                    let ts = entry.get("ts").and_then(Value::as_i64).unwrap_or(0);
                    let rname = s(entry, "rule_name");
                    let detail = s(entry, "detail");
                    entries = entries.push(
                        container(
                            row![
                                dim(format_unix_ts(ts)),
                                column![text::caption(rname.to_string()), dim(detail.to_string())].spacing(2),
                            ]
                            .spacing(10)
                            .align_y(Alignment::Start),
                        )
                        .padding([6, 8])
                        .width(Length::Fill)
                        .class(sub_card()),
                    );
                }
                body = body.push(entries);
            }
        }

        if self.window_mode {
            content = content.push(body);
        } else {
            content = content.push(
                container(scrollable(container(body).padding([0, 12, 0, 0])).height(Length::Shrink))
                    .max_height(ROOMS_MAX_HEIGHT),
            );
        }
        content.into()
    }

    fn rule_card<'a>(&'a self, rule: &'a Value, all_paused: bool) -> Element<'a, Message> {
        let id = s(rule, "id").to_string();
        let enabled = flag(rule, "enabled");
        let kind = s(rule, "kind");
        let name = match s(rule, "name") {
            "" => tr("Rule"),
            n => n.to_string(),
        };
        let kind_label = match kind {
            "exclusive" => tr("Exclusive"),
            "companion" => tr("Companion"),
            _ => tr("Follow"),
        };

        let trig_name = resolve_name(&self.stats, s(rule, "trigger_kind"), s(rule, "trigger_id"));
        let trig_state = if s(rule, "trigger_state") == "off" { tr("Turns off") } else { tr("Turns on") };
        let when_part = format!("{trig_name} ({trig_state})");

        let targets_str: String = list(rule, "targets")
            .iter()
            .map(|t| {
                let tname = resolve_name(&self.stats, s(t, "kind"), s(t, "id"));
                let act = if s(t, "action") == "off" { tr("Turn off") } else { tr("Turn on") };
                format!("{tname} ({act})")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let summary_line = trf("WHEN {0} → {1}", &[when_part, targets_str]);

        let mut conds: Vec<String> = Vec::new();
        if flag(rule, "mirror_off") {
            conds.push(tr("Also turn off together"));
        }
        if flag(rule, "match_bri") {
            conds.push(tr("Match brightness"));
        }
        if flag(rule, "only_if_off") {
            conds.push(tr("Only if target is off"));
        }
        let ts = s(rule, "time_start");
        let te = s(rule, "time_end");
        if !ts.is_empty() || !te.is_empty() {
            conds.push(format!("{ts}–{te}"));
        }

        let toggle_id = id.clone();
        let title_el: Element<Message> = if all_paused || !enabled {
            dim(name)
        } else {
            text::heading(name).into()
        };
        let head = row![
            title_el,
            mini_badge(kind_label),
            space::horizontal(),
            toggler(enabled).on_toggle(move |v| Message::RuleToggle(toggle_id.clone(), v)),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let mut col = column![head, text::caption(summary_line)].spacing(6);
        if !conds.is_empty() {
            col = col.push(dim(conds.join(" · ")));
        }
        col = col.push(
            row![
                space::horizontal(),
                button::destructive(tr("Delete")).on_press(Message::RuleDelete(id)),
            ]
            .align_y(Alignment::Center),
        );

        container(col).padding(12).width(Length::Fill).class(card(None)).into()
    }

    fn rule_builder<'a>(&'a self, draft: &'a RuleDraft) -> Element<'a, Message> {
        let mut form = column![
            text::heading(tr("+ New rule")),
            text_input(tr("Name (optional)"), &draft.name).on_input(Message::RuleName),
        ]
        .spacing(8);

        // Rule type pills
        let types: Vec<Element<Message>> = [
            ("follow", tr("Follow / Mirror")),
            ("exclusive", tr("Exclusive (turn other off)")),
            ("companion", tr("Companion")),
        ]
        .into_iter()
        .map(|(k, label)| {
            let b = if draft.kind == k { button::suggested(label) } else { button::standard(label) };
            b.on_press(Message::RuleKind(k.into())).into()
        })
        .collect();
        form = form.push(column![dim(tr("Rule type")), flex_row(types).row_spacing(6).column_spacing(6)].spacing(4));

        // Trigger picker + state
        let trig_label = if draft.trigger_name.is_empty() { tr("Select target") } else { draft.trigger_name.clone() };
        form = form.push(
            row![
                dim(tr("When")),
                space::horizontal(),
                button::text(trig_label)
                    .trailing_icon(icon::from_name(if draft.trigger_picker_open { "pan-up-symbolic" } else { "pan-down-symbolic" }).size(16))
                    .on_press(Message::RuleToggleTriggerPicker),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
        if draft.trigger_picker_open {
            let mut opts = column![].spacing(2);
            for opt in all_targets(&self.stats) {
                let norm = if opt.kind == "zone" { "room" } else { opt.kind.as_str() };
                let selected = norm == draft.trigger_kind && opt.id == draft.trigger_id;
                let label = if selected { format!("✓ {}", opt.label) } else { opt.label.clone() };
                opts = opts.push(
                    menu_button(text::caption(label)).on_press(Message::RulePickTrigger(norm.into(), opt.id, opt.label)),
                );
            }
            form = form.push(
                container(scrollable(opts).height(Length::Shrink))
                    .max_height(150.0)
                    .padding(4)
                    .width(Length::Fill)
                    .class(sub_card()),
            );
        }

        let state_pills: Vec<Element<Message>> = [("on", tr("Turns on")), ("off", tr("Turns off"))]
            .into_iter()
            .map(|(k, label)| {
                let b = if draft.trigger_state == k { button::suggested(label) } else { button::standard(label) };
                b.on_press(Message::RuleTriggerState(k.into())).into()
            })
            .collect();
        form = form.push(flex_row(state_pills).row_spacing(6).column_spacing(6));

        // Target picker
        let target_label = if draft.target_name.is_empty() { tr("Select target") } else { draft.target_name.clone() };
        let action_hint = if draft.target_action() == "off" { tr("Turn off") } else { tr("Turn on") };
        form = form.push(
            row![
                dim(format!("{} ({action_hint})", tr("Then"))),
                space::horizontal(),
                button::text(target_label)
                    .trailing_icon(icon::from_name(if draft.target_picker_open { "pan-up-symbolic" } else { "pan-down-symbolic" }).size(16))
                    .on_press(Message::RuleToggleTargetPicker),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
        if draft.target_picker_open {
            let mut opts = column![].spacing(2);
            for opt in all_targets(&self.stats) {
                let norm = if opt.kind == "zone" { "room" } else { opt.kind.as_str() };
                let selected = norm == draft.target_kind && opt.id == draft.target_id;
                let label = if selected { format!("✓ {}", opt.label) } else { opt.label.clone() };
                opts = opts.push(
                    menu_button(text::caption(label)).on_press(Message::RulePickTarget(norm.into(), opt.id, opt.label)),
                );
            }
            form = form.push(
                container(scrollable(opts).height(Length::Shrink))
                    .max_height(150.0)
                    .padding(4)
                    .width(Length::Fill)
                    .class(sub_card()),
            );
        }

        // Option checkboxes
        if draft.kind != "exclusive" {
            form = form.push(checkbox(draft.mirror_off).label(tr("Also turn off together")).on_toggle(Message::RuleMirrorOff));
            form = form.push(checkbox(draft.match_bri).label(tr("Match brightness")).on_toggle(Message::RuleMatchBri));
        }
        form = form.push(checkbox(draft.only_if_off).label(tr("Only if target is off")).on_toggle(Message::RuleOnlyIfOff));

        // Optional time window
        form = form.push(
            row![
                dim(tr("Active between")),
                space::horizontal(),
                text_input(tr("Optional HH:MM"), &draft.time_start)
                    .on_input(Message::RuleTimeStart)
                    .width(Length::Fixed(96.0)),
                dim("–"),
                text_input(tr("Optional HH:MM"), &draft.time_end)
                    .on_input(Message::RuleTimeEnd)
                    .width(Length::Fixed(96.0)),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );

        if let Some(err) = &draft.error {
            form = form.push(error_text(err.clone()));
        }

        form = form.push(
            row![
                space::horizontal(),
                button::standard(tr("Cancel")).on_press(Message::CancelRule),
                button::suggested(tr("Save")).on_press(Message::RuleSave),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );

        container(form).padding(12).width(Length::Fill).class(card(None)).into()
    }

    // --------------------------------------------------------------------- Main details

    fn details(&self) -> Element<'_, Message> {
        let mut content = column![self.header()].spacing(12);
        let error = s(&self.stats, "error");
        if !error.is_empty() {
            content = content.push(error_text(maprisma_hue_lights::i18n::error(&self.stats)));
        }
        if !self.paired() {
            content = content.push(divider::horizontal::default());
            content = content.push(self.pairing());
            return content.into();
        }
        content = content.push(self.tab_bar());
        content = content.push(match self.tab {
            Tab::Lights => self.lights_tab(),
            Tab::Schedules => self.schedules_tab(),
            Tab::Rules => self.rules_tab(),
        });
        content.into()
    }

    // Done, Enter or the popup closing (keep: the note typed so far is saved), Esc (skip).
    fn finish_note(&mut self, keep: bool) -> Task<Message> {
        match std::mem::take(&mut self.shot) {
            shot::Phase::Note { path, text, .. } => {
                if keep && shot::note_text(&text).is_some() {
                    // Busy until the note is written, so a new F12 cannot replace it.
                    self.shot = shot::Phase::Noting;
                    return off_thread(
                        move || shot::save_note(&path, SHOT_FOLDER, &text).map(|_| ()).map_err(|e| e.to_string()),
                        Message::NoteSaved,
                    );
                }
            }
            shot::Phase::Error(_) => {}
            other => self.shot = other,
        }
        Task::none()
    }

    // A capture still on its way is dropped with its popup; a typed note is kept (saved).
    // A resize that has settled (or, on `closing`, any waiting one) goes to config.json; not
    // while maximised, fullscreen or tiled (libcosmic's sharp corners).
    fn save_size(&mut self, closing: bool) {
        let sharp = self.core.window.sharp_corners;
        let ignore = || window_size::not_users_size(sharp);
        let size = if closing { self.resize.flush(ignore) } else { self.resize.due(Instant::now(), ignore) };
        if let Some(size) = size {
            self.setting_error = write_pref(window_size::KEY, Some(window_size::to_value(size))).is_err();
        }
        // Hyprland: where the user left it, for the next open (a move sends no event, so on close).
        if closing && let Some(position) = window_size::current_position() {
            self.setting_error |= write_pref(window_size::POSITION_KEY, Some(window_size::position_value(position))).is_err();
        }
    }

    fn popup_closed(&mut self) -> Task<Message> {
        if std::mem::take(&mut self.capturing) && self.shot == shot::Phase::Busy {
            self.shot = shot::Phase::Idle;
        }
        if matches!(self.shot, shot::Phase::Note { .. }) { self.finish_note(true) } else { Task::none() }
    }

    // After F12: "Screenshot saved", where, the optional note and Done (or the error and Done).
    fn shot_overlay(&self) -> Option<Element<'_, Message>> {
        let body: Element<Message> = match &self.shot {
            shot::Phase::Note { shown, text: note, .. } => column![
                text::heading(tr("Screenshot saved")),
                dim(trf("Saved as {0}", &[shown.clone()])),
                text_input(tr("What is wrong? (optional)"), note.as_str())
                    .id(NOTE_ID.clone())
                    .on_input(Message::NoteInput)
                    .on_submit(|_| Message::NoteDone),
                row![space::horizontal().width(Length::Fill), button::suggested(tr("Done")).on_press(Message::NoteDone)],
            ]
            .spacing(12)
            .into(),
            shot::Phase::Error(reason) => column![
                text::body(reason.clone()),
                row![space::horizontal().width(Length::Fill), button::suggested(tr("Done")).on_press(Message::NoteDone)],
            ]
            .spacing(12)
            .into(),
            _ => return None,
        };
        let card = container(body).padding(16).width(Length::Fill).class(theme::Container::Card);
        let card = container(card).padding(12).width(if self.window_mode { Length::Fill } else { Length::Fixed(POPUP_WIDTH) });
        Some(card.into())
    }

    /// Settings (as Maprisma Launcher's page): ← Back and the title, then one row per setting,
    /// words at the left and the control at the right; a list opens under its row.
    fn settings_page(&self) -> Element<'_, Message> {
        let header = row![
            button::text(tr("Back")).leading_icon(icon::from_name("go-previous-symbolic").size(16)).on_press(Message::ToggleSettings),
            space::horizontal().width(Length::Fill),
            dim(tr("Settings").to_uppercase()),
        ]
        .align_y(Alignment::Center);
        let choice = |label: String, open: bool, message: Message| -> Element<'static, Message> {
            button::text(label)
                .trailing_icon(icon::from_name(if open { "pan-up-symbolic" } else { "pan-down-symbolic" }).size(16))
                .on_press(message)
                .into()
        };
        let setting = |label: String, control: Element<'static, Message>| {
            padded_control(row![text::body(label), space::horizontal().width(Length::Fill), control].spacing(12).align_y(Alignment::Center))
        };
        let language = maprisma_hue_lights::i18n::setting();
        let choices = maprisma_hue_lights::i18n::choices();
        let current = choices.iter().find(|(code, _)| *code == language).map(|(_, l)| l.clone()).unwrap_or_default();
        let mut page = column![padded_control(header), divider::horizontal::default()];
        page = page.push(setting(tr("Language"), choice(current, self.language_open, Message::ToggleLanguage)));
        if self.language_open {
            for (code, label) in choices {
                let label = if code == language { format!("✓ {label}") } else { label };
                page = page.push(menu_button(text::body(label)).on_press(Message::Language(code)));
            }
        }
        page = page.push(setting(
            tr("Confirm before All On"),
            toggler(self.confirm_all_on_pref).on_toggle(Message::SetConfirmAllOnPref).into(),
        ));
        page = page.push(setting(tr("F12 saves a screenshot"), toggler(self.shot_key).on_toggle(Message::SetShotKey).into()));
        let corner_choices = [
            ("desktop", trf("Like the desktop ({0} px)", &[corners::desktop_radius().round().to_string()])),
            ("slight", trf("Slightly rounded ({0} px)", &["6".into()])),
            ("rounded", trf("Rounded ({0} px)", &["12".into()])),
            ("square", tr("Square")),
        ];
        let current = corner_choices.iter().find(|(c, _)| *c == self.corners).map(|(_, l)| l.clone()).unwrap_or_default();
        page = page.push(setting(tr("Corners"), choice(current, self.corners_open, Message::ToggleCorners)));
        if self.corners_open {
            for (choice, label) in corner_choices {
                let label = if self.corners == choice { format!("✓ {label}") } else { label };
                page = page.push(menu_button(text::body(label)).on_press(Message::Corners(choice.into())));
            }
        }
        if self.language_error { page = page.push(padded_control(error_text(tr("Could not save language setting.")))); }
        if self.setting_error { page = page.push(padded_control(error_text(tr("Could not save the setting.")))); }
        let page = page.width(if self.window_mode { Length::Fill } else { Length::Fixed(POPUP_WIDTH) }).padding([0, 0, 8, 0]);
        scrollable(page).height(if self.window_mode { Length::Fill } else { Length::Shrink }).into()
    }

    fn popup_content(&self) -> Element<'_, Message> {
        if let Some(overlay) = self.shot_overlay() {
            return overlay;
        }
        if self.settings_open {
            return self.settings_page();
        }
        let content = column![padded_control(self.details()).padding([12, 16]), self.actions()]
            .width(if self.window_mode { Length::Fill } else { Length::Fixed(POPUP_WIDTH) })
            .padding([0, 0, 8, 0]);
        if self.window_mode {
            // Keep every action reachable when the normal window is resized smaller.
            scrollable(content).height(Length::Fill).into()
        } else {
            content.into()
        }
    }

    fn actions(&self) -> Element<'_, Message> {
        if !self.paired() {
            return space::vertical().height(Length::Fixed(0.0)).into();
        }
        let unpair = if self.confirm_unpair {
            button::destructive(tr("Really unpair?")).on_press(Message::Unpair)
        } else {
            button::text(tr("Unpair")).on_press(Message::Unpair)
        };
        let master_row: Element<Message> = if self.confirm_all_on {
            padded_control(
                row![
                    text::body(tr("Turn all lights on?")),
                    space::horizontal(),
                    button::suggested(tr("Yes, all on")).on_press(Message::ConfirmAllOn),
                    button::text(tr("Cancel")).on_press(Message::CancelAllOn),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .into()
        } else {
            padded_control(
                row![
                    button::standard(tr("All on")).on_press(Message::AllOn),
                    button::standard(tr("All off")).on_press_maybe((self.on_count() > 0.0).then_some(Message::AllOff)),
                    space::horizontal(),
                    button::standard(tr("Refresh")).on_press(Message::Refresh),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .into()
        };
        column![
            divider::horizontal::default(),
            master_row,
            padded_control(
                row![dim(tr("Bridge")), text::caption(bridge_text(&self.stats)), space::horizontal(), unpair]
                    .spacing(8)
                    .align_y(Alignment::Center)
            ),
        ]
        .into()
    }
}

impl<const WINDOW_MODE: bool> HueLights<WINDOW_MODE> {
    /// The `--window` toplevel's title (taskbars, Alt+Tab, window rules): libcosmic takes it from
    /// the title it keeps per window (`set_window_title`); `Core::set_title` alone sends nothing,
    /// which left the toplevel without a title on KDE and COSMIC.
    fn name_window(&mut self) -> Task<Message> {
        match self.core.main_window_id() {
            Some(id) => cosmic::app::ApplicationExt::set_window_title(self, TITLE.into(), id),
            None => Task::none(),
        }
    }
}

impl<const WINDOW_MODE: bool> cosmic::Application for HueLights<WINDOW_MODE> {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = bool;
    type Message = Message;
    // Wayland's app id: the panel applet's, or the window's own (`<id>.Window`, the id of the
    // `.Window.desktop` entry install.sh writes, so taskbars and window rules find it).
    const APP_ID: &'static str = if WINDOW_MODE { WINDOW_APP_ID } else { APP_ID };

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(mut core: Core, window_mode: bool) -> (Self, Task<Message>) {
        maprisma_hue_lights::look::solid(&mut core);
        if window_mode { core.set_header_title(TITLE.into()); }
        let start_size = window_start_size();
        let mut app = Self {
            core,
            window_mode,
            window_width: if window_mode { start_size.0 } else { POPUP_WIDTH },
            shot_key: shot_key_setting(),
            confirm_all_on_pref: confirm_all_on_setting(),
            corners: corners_setting(),
            favorites: favorites_setting(),
            exclude_all_on: exclude_all_on_setting(),
            ..Default::default()
        };
        // Debug builds only: start on the settings page (for off-screen screenshots).
        app.settings_open = cfg!(debug_assertions) && std::env::var("MAPRISMA_SHOT_PAGE").as_deref() == Ok("settings");
        let title = if window_mode { app.name_window() } else { Task::none() };
        if window_mode {
            app.resize = window_size::Debounce::new(start_size);
            // Hyprland: back where the user left it (the tray rule first puts it under the bar).
            window_size::restore_position(window_size::parse_position(read_pref(window_size::POSITION_KEY).as_ref()), start_size);
        }
        corners::set_current(&app.corners);
        // The desktop's theme for the whole window (title bar too), see look.rs.
        let theme = if window_mode { corners::set_app_theme(&app.corners) } else { Task::none() };
        (app, Task::batch([title, theme]))
    }

    /// COSMIC's colours or light/dark mode changed: a window with its own corners (a custom
    /// theme, which libcosmic leaves alone) takes the new colours too.
    fn system_theme_update(&mut self, _keys: &[&'static str], new_theme: &cosmic::cosmic_theme::Theme) -> Task<Message> {
        corners::system_theme_update(&self.corners, new_theme, self.window_mode)
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::CloseRequested(id))
    }

    fn on_window_resize(&mut self, id: window::Id, width: f32, height: f32) {
        if self.window_mode && self.core.main_window_id() == Some(id) {
            self.window_width = width;
            self.resize.resized(width, height, Instant::now());
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let close = if self.window_mode {
            Subscription::run_with("window-close", |_| stream::channel(1, |mut output: mpsc::Sender<Message>| async move {
                let mut rx = WINDOW_ACTION.subscribe();
                loop {
                    let action = *rx.borrow_and_update();
                    if let Some(show) = action {
                        let _ = output.send(if show { Message::Show } else { Message::Quit }).await;
                    }
                    if rx.changed().await.is_err() { break; }
                }
            }))
        } else { Subscription::none() };
        let rectangles = if self.window_mode { Subscription::none() } else {
            rectangle_tracker_subscription(0).map(|update| Message::Rectangle(update.1))
        };
        // F12 (no modifiers) in the popup or window; Esc closes the innermost thing (esc_step).
        let keys = cosmic::iced::event::listen_with(|event, status, id| match event {
            cosmic::iced::Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                if shot::is_shot_key(&key, modifiers) {
                    Some(Message::ShotKey(id))
                } else if status == cosmic::iced::event::Status::Ignored
                    && matches!(key, cosmic::iced::keyboard::Key::Named(cosmic::iced::keyboard::key::Named::Escape))
                {
                    // Not one an open drop-down list took to close itself.
                    Some(Message::Escape(id))
                } else {
                    None
                }
            }
            _ => None,
        });
        let save_size = if self.window_mode && self.resize.waiting() {
            cosmic::iced::time::every(Duration::from_millis(100)).map(|_| Message::SaveSize)
        } else { Subscription::none() };
        Subscription::batch(vec![close, rectangles, keys, save_size,
            Subscription::run_with("hue-lights-helper", |_| {
                stream::channel(8, |mut output| async move {
                    run_helper(&mut output).await;
                })
            }),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Show => {
                if let Some(id) = self.core.main_window_id() {
                    return Task::batch([window::set_mode(id, window::Mode::Windowed), window::gain_focus(id)]);
                }
            }
            Message::Quit => { self.save_size(true); return cosmic::iced::exit(); }
            Message::SaveSize => self.save_size(false),
            Message::Helper(sender) => {
                self.helper = Some(sender);
                if self.window_mode || self.popup.is_some() {
                    self.send(json!({"cmd": "open"}));
                }
            }
            Message::Stats(stats) => {
                if !matches!(s(&stats, "phase"), "ok" | "error" | "connecting") {
                    self.confirm_unpair = false;
                    self.confirm_all_on = false;
                }
                self.stats = stats;
                // Settings changed in another interface (window, applet) take effect here too.
                self.shot_key = shot_key_setting();
                self.confirm_all_on_pref = confirm_all_on_setting();
                self.favorites = favorites_setting();
                self.exclude_all_on = exclude_all_on_setting();
                if self.filter == RoomFilter::Favorites && self.favorites.is_empty() {
                    self.filter = RoomFilter::All;
                }
                let corners = corners_setting();
                let mut restyle = corners != self.corners;
                if restyle { corners::set_current(&corners); self.corners = corners; }
                // The desktop's colours and corners (Maprisma Theme can change them any time).
                restyle |= maprisma_hue_lights::look::refresh();
                if restyle && self.window_mode { return corners::set_app_theme(&self.corners); }
            }
            Message::Rectangle(update) => match update {
                RectangleUpdate::Rectangle(r) => self.rectangle = r.1,
                RectangleUpdate::Init(tracker) => self.tracker = Some(tracker),
            },
            Message::CloseRequested(id) => {
                if self.window_mode { self.save_size(true); return cosmic::iced::exit(); }
                if self.popup == Some(id) {
                    self.popup = None;
                    self.confirm_unpair = false;
                    self.confirm_all_on = false;
                    self.send(json!({"cmd": "closed"}));
                    return self.popup_closed();
                }
            }
            Message::TogglePopup => {
                if self.window_mode { return Task::none(); }
                if let Some(popup) = self.popup.take() {
                    self.confirm_unpair = false;
                    self.confirm_all_on = false;
                    self.send(json!({"cmd": "closed"}));
                    return Task::batch([self.popup_closed(), destroy_popup(popup)]);
                }
                self.send(json!({"cmd": "open"}));
                let focus = if matches!(self.shot, shot::Phase::Note { .. }) {
                    cosmic::widget::text_input::focus(NOTE_ID.clone())
                } else {
                    Task::none()
                };
                return Task::batch([cosmic::surface::surface_task(cosmic::surface::action::app_popup(
                    |_| Default::default(),
                    |app: &mut Self| {
                        let id = window::Id::unique();
                        app.popup = Some(id);
                        let mut settings =
                            app.core.applet.get_popup_settings(app.core.main_window_id().unwrap(), id, None, None, None);
                        let Rectangle { x, y, width, height } = app.rectangle;
                        settings.positioner.anchor_rect = Rectangle::<i32> {
                            x: x.max(1.0) as i32,
                            y: y.max(1.0) as i32,
                            width: width.max(1.0) as i32,
                            height: height.max(1.0) as i32,
                        };
                        settings.positioner.size = None;
                        settings
                    },
                    None,
                )), focus]);
            }
            Message::SelectTab(tab) => {
                self.tab = tab;
                self.close_pickers();
            }
            Message::SetFilter(filter) => {
                self.filter = filter;
            }
            Message::Expand(key) => {
                if !self.expanded.remove(&key) {
                    self.expanded.insert(key);
                }
            }
            Message::AllScenes(key) => {
                if !self.all_scenes.remove(&key) {
                    self.all_scenes.insert(key);
                }
            }
            Message::ToggleLightDrawer(id) => {
                if self.open_light_drawer.as_deref() == Some(id.as_str()) {
                    self.open_light_drawer = None;
                } else {
                    self.open_light_drawer = Some(id);
                }
            }
            Message::ToggleFavorite(kind, id) => {
                toggle_favorite(&mut self.favorites, &kind, &id);
                let val = (!self.favorites.is_empty()).then(|| favorites_to_value(&self.favorites));
                self.setting_error = write_pref(FAVORITES_KEY, val).is_err();
                if self.filter == RoomFilter::Favorites && self.favorites.is_empty() {
                    self.filter = RoomFilter::All;
                }
            }
            Message::ToggleExcludeAllOn(key, excluded) => {
                if excluded {
                    self.exclude_all_on.insert(key);
                } else {
                    self.exclude_all_on.remove(&key);
                }
                let val = (!self.exclude_all_on.is_empty()).then(|| string_set_to_value(&self.exclude_all_on));
                self.setting_error = write_pref(EXCLUDE_ALL_ON_KEY, val).is_err();
            }
            Message::LightMirek(id, mirek) => {
                self.send(json!({"cmd": "light", "id": id, "mirek": mirek}));
            }
            Message::LightXy(id, xy) => {
                self.send(json!({"cmd": "light", "id": id, "xy": xy}));
            }
            Message::LightEffect(id, effect) => {
                self.send(json!({"cmd": "light", "id": id, "effect": effect}));
            }
            Message::RoomOn(key, on) => self.set_room(&key, "on", json!(on)),
            Message::LightOn(id, on) => self.send(json!({"cmd": "light", "id": id, "on": on})),
            Message::Bri(target, value) => {
                let value = value.max(1);
                let now = Instant::now();
                let due = self.dragging.get(&target).is_none_or(|(_, sent)| now.duration_since(*sent) >= SEND_EVERY);
                let sent = if due {
                    self.send_bri(&target, value);
                    now
                } else {
                    self.dragging[&target].1
                };
                self.dragging.insert(target, (value, sent));
            }
            Message::BriDone(target) => {
                if let Some((value, _)) = self.dragging.remove(&target) {
                    self.send_bri(&target, value);
                }
            }
            Message::Scene(id) => self.send(json!({"cmd": "scene", "id": id})),
            Message::AllOn => {
                if self.confirm_all_on_pref {
                    self.confirm_all_on = true;
                } else {
                    self.send_all_on();
                }
            }
            Message::ConfirmAllOn => {
                self.confirm_all_on = false;
                self.send_all_on();
            }
            Message::CancelAllOn => {
                self.confirm_all_on = false;
            }
            Message::AllOff => {
                if self.on_count() > 0.0 {
                    self.send(json!({"cmd": "all_off"}));
                }
            }
            Message::Refresh => self.send(json!({"cmd": "refresh"})),

            // Schedules
            Message::StartSchedule => {
                let first = all_targets(&self.stats).into_iter().next();
                self.schedule_draft = Some(ScheduleDraft::new(first));
            }
            Message::StartScheduleFor(kind, id, name) => {
                self.tab = Tab::Schedules;
                self.open_light_drawer = None;
                self.schedule_draft = Some(ScheduleDraft::new(Some(TargetOption { kind, id, label: name })));
            }
            Message::CancelSchedule => {
                self.schedule_draft = None;
            }
            Message::ScheduleName(v) => {
                if let Some(d) = &mut self.schedule_draft {
                    d.name = v.chars().take(60).collect();
                }
            }
            Message::ScheduleToggleTargetPicker => {
                if let Some(d) = &mut self.schedule_draft {
                    d.target_picker_open = !d.target_picker_open;
                }
            }
            Message::SchedulePickTarget(kind, id, label) => {
                if let Some(d) = &mut self.schedule_draft {
                    d.target_kind = kind;
                    d.target_id = id;
                    d.target_name = label;
                    d.target_picker_open = false;
                    d.error = None;
                }
            }
            Message::ScheduleTriggerKind(kind) => {
                if let Some(d) = &mut self.schedule_draft {
                    d.trigger_kind = kind;
                    d.error = None;
                }
            }
            Message::ScheduleTime(t) => {
                if let Some(d) = &mut self.schedule_draft {
                    d.time = t.chars().take(5).collect();
                    d.error = None;
                }
            }
            Message::ScheduleOffsetDelta(delta) => {
                if let Some(d) = &mut self.schedule_draft {
                    d.offset_min = (d.offset_min + delta).clamp(-120, 120);
                }
            }
            Message::ScheduleToggleDay(day) => {
                if let Some(d) = &mut self.schedule_draft {
                    if !d.days.remove(&day) {
                        d.days.insert(day);
                    }
                }
            }
            Message::ScheduleAction(act) => {
                if let Some(d) = &mut self.schedule_draft {
                    d.action = act;
                }
            }
            Message::ScheduleBri(bri) => {
                if let Some(d) = &mut self.schedule_draft {
                    d.bri = bri.clamp(1, 100);
                }
            }
            Message::ScheduleFade(fade) => {
                if let Some(d) = &mut self.schedule_draft {
                    d.fade_min = fade;
                }
            }
            Message::ScheduleSave => {
                if let Some(d) = &mut self.schedule_draft {
                    match d.to_command() {
                        Ok(cmd) => {
                            self.send(cmd);
                            self.schedule_draft = None;
                        }
                        Err(err) => d.error = Some(err),
                    }
                }
            }
            Message::ScheduleToggle(id, enabled) => {
                self.send(json!({"cmd": "schedule_toggle", "id": id, "enabled": enabled}));
            }
            Message::ScheduleDelete(id) => {
                self.send(json!({"cmd": "schedule_delete", "id": id}));
            }

            // Rules
            Message::RulesPause(paused) => {
                self.send(json!({"cmd": "rules_pause", "paused": paused}));
            }
            Message::StartRule => {
                let targets = all_targets(&self.stats);
                let first = targets.first().cloned();
                let second = targets.get(1).cloned().or_else(|| first.clone());
                self.rule_draft = Some(RuleDraft::new(first, second));
            }
            Message::StartRuleFor(kind, id, name) => {
                self.tab = Tab::Rules;
                self.open_light_drawer = None;
                let second = all_targets(&self.stats).into_iter().find(|t| t.id != id);
                self.rule_draft = Some(RuleDraft::new(Some(TargetOption { kind, id, label: name }), second));
            }
            Message::CancelRule => {
                self.rule_draft = None;
            }
            Message::RuleName(v) => {
                if let Some(d) = &mut self.rule_draft {
                    d.name = v.chars().take(60).collect();
                }
            }
            Message::RuleKind(kind) => {
                if let Some(d) = &mut self.rule_draft {
                    d.kind = kind;
                }
            }
            Message::RuleToggleTriggerPicker => {
                if let Some(d) = &mut self.rule_draft {
                    d.trigger_picker_open = !d.trigger_picker_open;
                    d.target_picker_open = false;
                }
            }
            Message::RulePickTrigger(kind, id, label) => {
                if let Some(d) = &mut self.rule_draft {
                    d.trigger_kind = kind;
                    d.trigger_id = id;
                    d.trigger_name = label;
                    d.trigger_picker_open = false;
                    d.error = None;
                }
            }
            Message::RuleTriggerState(state) => {
                if let Some(d) = &mut self.rule_draft {
                    d.trigger_state = state;
                }
            }
            Message::RuleToggleTargetPicker => {
                if let Some(d) = &mut self.rule_draft {
                    d.target_picker_open = !d.target_picker_open;
                    d.trigger_picker_open = false;
                }
            }
            Message::RulePickTarget(kind, id, label) => {
                if let Some(d) = &mut self.rule_draft {
                    d.target_kind = kind;
                    d.target_id = id;
                    d.target_name = label;
                    d.target_picker_open = false;
                    d.error = None;
                }
            }
            Message::RuleMirrorOff(v) => {
                if let Some(d) = &mut self.rule_draft {
                    d.mirror_off = v;
                }
            }
            Message::RuleMatchBri(v) => {
                if let Some(d) = &mut self.rule_draft {
                    d.match_bri = v;
                }
            }
            Message::RuleOnlyIfOff(v) => {
                if let Some(d) = &mut self.rule_draft {
                    d.only_if_off = v;
                }
            }
            Message::RuleTimeStart(v) => {
                if let Some(d) = &mut self.rule_draft {
                    d.time_start = v.chars().take(5).collect();
                    d.error = None;
                }
            }
            Message::RuleTimeEnd(v) => {
                if let Some(d) = &mut self.rule_draft {
                    d.time_end = v.chars().take(5).collect();
                    d.error = None;
                }
            }
            Message::RuleSave => {
                if let Some(d) = &mut self.rule_draft {
                    match d.to_command() {
                        Ok(cmd) => {
                            self.send(cmd);
                            self.rule_draft = None;
                        }
                        Err(err) => d.error = Some(err),
                    }
                }
            }
            Message::RuleToggle(id, enabled) => {
                self.send(json!({"cmd": "rule_toggle", "id": id, "enabled": enabled}));
            }
            Message::RuleDelete(id) => {
                self.send(json!({"cmd": "rule_delete", "id": id}));
            }
            Message::ToggleActivity => {
                self.activity_open = !self.activity_open;
            }

            // Pairing & settings
            Message::Ip(ip) => self.ip = ip.chars().take(64).collect(),
            Message::Pair(ip) => {
                if !ip.is_empty() {
                    self.send(json!({"cmd": "pair", "ip": ip}));
                }
            }
            Message::Discover => self.send(json!({"cmd": "discover"})),
            Message::CancelPair => self.send(json!({"cmd": "cancel_pair"})),
            Message::Language(code) => {
                self.language_error = maprisma_hue_lights::i18n::save(&code).is_err();
                self.language_open = false;
            }
            Message::ShotKey(id) => {
                let shown = if self.window_mode { self.core.main_window_id() == Some(id) } else { self.popup == Some(id) };
                if self.shot_key && shown && self.shot == shot::Phase::Idle {
                    self.shot = shot::Phase::Busy;
                    self.capturing = true;
                    return window::screenshot(id).map(|picture| cosmic::Action::App(Message::Captured(picture)));
                }
            }
            Message::Captured(picture) => {
                if !std::mem::take(&mut self.capturing) { return Task::none(); }
                let (width, height) = (picture.size.width, picture.size.height);
                return off_thread(move || shot::save_screenshot(SHOT_FOLDER, &picture.rgba, width, height), Message::ShotSaved);
            }
            Message::ShotSaved(result) => {
                self.shot = match result {
                    Ok(saved) => shot::Phase::Note { path: saved.path, shown: saved.shown, text: String::new() },
                    Err(reason) => shot::Phase::Error(trf("The screenshot could not be saved: {0}", &[reason])),
                };
                if matches!(self.shot, shot::Phase::Note { .. }) {
                    return cosmic::widget::text_input::focus(NOTE_ID.clone());
                }
            }
            Message::NoteInput(value) => {
                if let shot::Phase::Note { text, .. } = &mut self.shot {
                    *text = value.chars().take(shot::NOTE_LIMIT).collect();
                }
            }
            Message::NoteDone => return self.finish_note(true),
            Message::Escape(id) => {
                let shown = if self.window_mode { self.core.main_window_id() == Some(id) } else { self.popup == Some(id) };
                let confirm = self.confirm_unpair || self.confirm_all_on;
                let menus = self.language_open || self.corners_open || self.any_picker_open();
                let drawer = self.any_drawer_open();
                match esc_step(shown, self.shot.open(), confirm, menus, drawer, self.settings_open) {
                    EscStep::Note => return self.finish_note(false),
                    EscStep::Confirm => {
                        self.confirm_unpair = false;
                        self.confirm_all_on = false;
                    }
                    EscStep::Menus => {
                        self.language_open = false;
                        self.corners_open = false;
                        self.close_pickers();
                    }
                    EscStep::Drawer => {
                        self.open_light_drawer = None;
                        self.schedule_draft = None;
                        self.rule_draft = None;
                    }
                    EscStep::Back => self.settings_open = false,
                    // Like the window's own close (the tray keeps running); the popup like a click on the icon.
                    EscStep::Close if self.window_mode => return self.update(Message::CloseRequested(id)),
                    EscStep::Close => return self.update(Message::TogglePopup),
                    EscStep::Nothing => {}
                }
            }
            Message::NoteSaved(result) => match result {
                Ok(()) => { if self.shot == shot::Phase::Noting { self.shot = shot::Phase::Idle; } }
                Err(reason) => self.shot = shot::Phase::Error(trf("The note could not be saved: {0}", &[reason])),
            },
            Message::SetShotKey(on) => {
                self.setting_error = write_pref("screenshot_key", Some(on.into())).is_err();
                self.shot_key = on;
            }
            Message::SetConfirmAllOnPref(on) => {
                let val = on.then(|| Value::from(true));
                self.setting_error = write_pref(CONFIRM_ALL_ON_KEY, val).is_err();
                self.confirm_all_on_pref = on;
                if !on {
                    self.confirm_all_on = false;
                }
            }
            Message::Corners(choice) => {
                let choice = corners::normalize(&choice);
                // "Like the desktop" is the default: no key in config.json.
                let value = (choice != "desktop").then(|| Value::from(choice));
                self.setting_error = write_pref("corners", value).is_err();
                self.corners = choice.to_string();
                corners::set_current(choice);
                self.corners_open = false;
                if self.window_mode { return corners::set_app_theme(choice); }
            }
            Message::ToggleSettings => {
                self.settings_open = !self.settings_open;
                self.language_open = false;
                self.corners_open = false;
                self.confirm_unpair = false;
                self.confirm_all_on = false;
            }
            Message::ToggleLanguage => { self.language_open = !self.language_open; self.corners_open = false; }
            Message::ToggleCorners => { self.corners_open = !self.corners_open; self.language_open = false; }
            Message::Unpair => {
                if self.confirm_unpair {
                    self.confirm_unpair = false;
                    self.send(json!({"cmd": "unpair"}));
                } else {
                    self.confirm_unpair = true;
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        maprisma_hue_lights::look::keep_fonts();
        if self.window_mode { return corners::apply_window(&self.corners, self.popup_content()); }
        let horizontal = matches!(self.core.applet.anchor, PanelAnchor::Top | PanelAnchor::Bottom);
        let padding = self.core.applet.suggested_padding(true).0;
        let button = button::custom(self.panel_content())
            .padding(if horizontal { [0, padding] } else { [padding, 0] })
            .on_press_down(Message::TogglePopup)
            .class(theme::Button::AppletIcon);
        // Right click: every light off.
        let button = mouse_area(button).on_right_press(Message::AllOff);
        let button = cosmic::widget::tooltip(button, text::body(self.summary()), cosmic::widget::tooltip::Position::Bottom);
        let content: Element<Message> = match self.tracker.as_ref() {
            Some(tracker) => tracker.container(0, button).ignore_bounds(true).into(),
            None => button.into(),
        };
        autosize::autosize(content, AUTOSIZE_ID.clone()).into()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
        maprisma_hue_lights::look::keep_fonts();
        if self.window_mode { return corners::apply_window(&self.corners, self.popup_content()); }

        corners::apply(&self.corners, self.core.applet.popup_container(self.popup_content()))
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        if self.window_mode { None } else { Some(cosmic::applet::style()) }
    }
}

// ------------------------------------------------------------------------- helper process

// Runs the helper: hands the app a sender for commands (one JSON per line), streams
// its snapshots, and starts it again RESTART_DELAY after it exits.
async fn run_helper(output: &mut mpsc::Sender<Message>) {
    loop {
        if let Some(script) = script_path() {
            let child = tokio::process::Command::new("python3")
                .arg("-I")
                .arg(&script)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn();
            if let Ok(mut child) = child
                && let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take())
            {
                let (sender, mut commands) = mpsc::unbounded::<String>();
                if output.send(Message::Helper(sender)).await.is_err() {
                    return;
                }
                let mut lines = BufReader::new(stdout).lines();
                loop {
                    tokio::select! {
                        line = lines.next_line() => match line {
                            Ok(Some(line)) => {
                                // A partial line while the helper exits is skipped.
                                if let Ok(stats) = serde_json::from_str::<Value>(&line)
                                    && stats.is_object()
                                    && output.send(Message::Stats(stats)).await.is_err()
                                {
                                    return;
                                }
                            }
                            _ => break,
                        },
                        command = commands.next() => match command {
                            Some(command) => {
                                if stdin.write_all(format!("{command}\n").as_bytes()).await.is_err()
                                    || stdin.flush().await.is_err()
                                {
                                    break;
                                }
                            }
                            None => break,
                        },
                    }
                }
                let _ = child.kill().await;
            }
        }
        tokio::time::sleep(RESTART_DELAY).await;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn window_mode_names_its_toplevel() {
        use cosmic::app::ApplicationExt;
        let mut app = HueLights::<false>::default();
        let id = cosmic::iced::window::Id::unique();
        app.core.set_main_window_id(Some(id));
        assert_eq!(app.title(id), "");
        let _ = app.name_window();
        assert_eq!(app.title(id), TITLE);
    }

    use super::*;

    #[test]
    fn esc_closes_the_innermost_thing_first() {
        assert_eq!(esc_step(true, true, true, true, true, true), EscStep::Note);
        assert_eq!(esc_step(true, false, true, true, true, true), EscStep::Confirm);
        assert_eq!(esc_step(true, false, false, true, true, true), EscStep::Menus);
        assert_eq!(esc_step(true, false, false, false, true, true), EscStep::Drawer);
        assert_eq!(esc_step(true, false, false, false, false, true), EscStep::Back);
        assert_eq!(esc_step(true, false, false, false, false, false), EscStep::Close);
        // Not from the window or the open popup (e.g. the panel button): nothing.
        assert_eq!(esc_step(false, true, true, true, true, true), EscStep::Nothing);
        assert_eq!(esc_step(false, false, false, false, false, false), EscStep::Nothing);
    }
}
