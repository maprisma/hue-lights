// Maprisma Lights: multi-brand local smart lighting & Floorplan Studio for COSMIC, KDE and Hyprland.
// Lit bulb in the accent while any light is on, dim bulb when all are off or no bridge
// is paired. Left click opens the popup; right click switches every light off.
//
// All bridge traffic goes through scripts/hue-lights.py (the same helper as the
// GNOME extension), which gets JSON commands on stdin and streams JSON snapshots
// on stdout (every 2 s while the popup is open, every 20 s while it is closed).
// It talks only to the bridge on the LAN, over pinned HTTPS; its app key stays
// in ~/.config/hue-lights/config.json and never reaches this process.

use maprisma_hue_lights::i18n::{read_pref, tr, trf, write_pref};
use maprisma_kit::{AppInfo, corners, look, shot, window_size};
mod floorplan;
mod help;
mod model;
use floorplan::{
    CanvasMode, FloorplanState, LampDialog, MapMsg, Selection, compute_pending_sync_count,
    view_lamp_dialog_overlay, view_map_tab,
};
use model::{
    ASK_AI_KEY, COLOR_PRESETS, CONFIRM_ALL_ON_KEY, DAYS_ORDER, EXCLUDE_ALL_ON_KEY, FAVORITES_KEY, FONT_SCALES,
    FONT_SCALE_KEY, Favorite, FloorplanBox, FloorplanFloor, FloorplanLamp, FloorplanPoint, FloorplanSensor,
    FloorplanStamp, PANEL_ICONS, PANEL_ICON_KEY, PLANNED_LIGHT_PRESETS, PlannedLight, RoomFilter,
    RuleDraft, SCHEDULE_MODES, SCHEDULE_MODE_KEY, SHOW_COUNT_KEY, SKINS, SKIN_KEY, SYNTH_BANDS, SYNTH_MODES,
    SYNTH_PALETTES, SYNTH_SOURCES, ScheduleDraft, Tab, TargetOption, ThemeDraft, WARMTH_PRESETS, all_rule_triggers,
    all_targets, belongs_to_rule, clamp_mirek, cmd_entertainment_sync, cmd_floorplan_bg_clear, cmd_floorplan_bg_set,
    cmd_floorplan_bg_update, cmd_floorplan_save, cmd_floorplan_sync_all, cmd_floorplan_sync_rooms,
    cmd_floorplan_template_with_plan, cmd_light_rename, cmd_motion_area_update, cmd_room_delete, cmd_room_update,
    cmd_scene_create, cmd_sensor_toggle, day_short_toggle, default_trigger_state, effect_label, extract_bridge_lights,
    favorites_to_value, font_scale_factor, font_scale_label, format_unix_ts, has_active_schedule, is_favorite,
    is_light_or_room_trigger, kelvin_to_hex, lamp_icon_svg, motion_aware_eligible_rooms, normalize_font_scale,
    normalize_lamp_icon, normalize_panel_icon, normalize_schedule_mode, normalize_skin, panel_icon_glyph,
    panel_icon_label, panel_icon_svg, parse_favorites, parse_floorplan, parse_motion_areas, parse_sensors,
    parse_string_set, resolve_name, room_matches_filter, schedule_action_summary, schedule_mode_label,
    schedule_trigger_summary, should_show_motion_aware_hint, skin_label, skin_style, sorted_exclude_list,
    string_set_to_value, synth_band_label, synth_mode_label, synth_palette_label, synth_palette_swatch,
    synth_source_label, synth_target_lights, toggle_favorite, trigger_state_label, trigger_state_options,
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
use cosmic::iced::widget::{container as iced_container, stack as iced_stack, svg as iced_svg, text as iced_text};
use cosmic::iced::{Alignment, Background, Border, Color, Length, Rectangle, Subscription, stream, window};
use cosmic::widget::rectangle_tracker::{RectangleTracker, RectangleUpdate, rectangle_tracker_subscription};
use cosmic::widget::{
    Id, autosize, button, checkbox, column, container, divider, flex_row, icon, mouse_area, row, scrollable, slider,
    space, text, text_input, toggler,
};
use cosmic::{Element, theme};
use serde::Deserialize;
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
static NOTE_ID: LazyLock<Id> = LazyLock::new(Id::unique);
static ASK_AI_ID: LazyLock<Id> = LazyLock::new(|| Id::new("hue-lights-ask-ai"));
// F12 pictures go to ~/Pictures/<this>/ (the app's name, the same in every language).
const SHOT_APP: AppInfo = AppInfo::new(APP_ID, TITLE);
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
    if std::env::args().any(|arg| arg == "--mcp") {
        if let Some(script) = maprisma_hue_lights::mcp_script_path() {
            let status = std::process::Command::new("python3")
                .arg("-I")
                .arg(&script)
                .status();
            std::process::exit(status.ok().and_then(|s| s.code()).unwrap_or(1));
        }
        eprintln!("MCP server script (hue-mcp.py) not found.");
        std::process::exit(1);
    }
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
        cosmic::app::run::<HueLights<true>>(cosmic::app::Settings::default().default_font(look::default_font())
            .size({ let (width, height) = window_start_size(); cosmic::iced::Size::new(width, height) })
            .resizable(Some(8.0)).exit_on_close(true).is_daemon(false), true)
    } else {
        look::keep_fonts();
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
    /// Step back from a Help detail guide to the Help overview.
    HelpOverview,
    /// Drop the "Unpair?" or "Turn all lights on?" confirmation.
    Confirm,
    /// Close an open Language / Corners list, light `⋯` submenu, or inline target picker.
    Menus,
    /// Close an open inline schedule/rule builder, light color drawer, or centered modal dialog.
    Drawer,
    /// Leave the settings, help, or Ask AI page.
    Back,
    /// Close the window (`--window`) or the popup.
    Close,
    Nothing,
}

/// `shown`: Esc came from the window or the open popup (not the panel button); `note`: the
/// screenshot card shows; `help_detail`: a single Help feature guide is open; `confirm`:
/// "Really unpair?" or "Turn all lights on?" shows; `menus`: a Language / Corners / light `⋯`
/// submenu / target picker list is open; `drawer`: an inline builder, modal dialog, or light
/// color drawer is open; `settings`: the settings, help overview, or Ask AI page shows.
fn esc_step(
    shown: bool,
    note: bool,
    help_detail: bool,
    confirm: bool,
    menus: bool,
    drawer: bool,
    settings: bool,
) -> EscStep {
    if !shown {
        EscStep::Nothing
    } else if note {
        EscStep::Note
    } else if help_detail {
        EscStep::HelpOverview
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

// The lit bulb/icon: skin accent if active, else the theme accent.
fn lit_svg_for(skin: &str) -> theme::Svg {
    let custom = skin_style(skin).map(|st| st.accent);
    theme::Svg::custom(move |theme: &cosmic::Theme| iced_svg::Style {
        color: Some(custom.unwrap_or_else(|| theme.cosmic().accent_color().into())),
    })
}

fn active_theme(skin: &str, corners_choice: &str) -> Option<cosmic::Theme> {
    if let Some(st) = skin_style(skin) {
        let radius = corners::radius(corners_choice, corners::desktop_radius());
        Some(look::theme_from(&st.palette(), Some(radius)))
    } else {
        corners::theme(corners_choice)
    }
}

fn card(tint: Option<Color>, skin: &str) -> theme::Container<'static> {
    let skin_st = skin_style(skin);
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let base = Color::from(theme.cosmic().on_bg_color());
        let (bg, border, width) = match (tint, skin_st) {
            (Some(t), Some(st)) => (with_alpha(t, 0.16), with_alpha(st.border_tint, 0.55), st.border_width),
            (Some(t), None) => (with_alpha(t, 0.12), with_alpha(t, 0.4), 1.0),
            (None, Some(st)) => (with_alpha(st.bg_tint, 0.28), with_alpha(st.border_tint, 0.48), st.border_width),
            (None, None) => (with_alpha(base, 0.05), with_alpha(base, 0.1), 1.0),
        };
        iced_container::Style {
            background: Some(Background::Color(bg)),
            border: Border { radius: corners::now().radius.into(), width, color: border },
            ..Default::default()
        }
    })
}

fn sub_card(skin: &str) -> theme::Container<'static> {
    let skin_st = skin_style(skin);
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let base = Color::from(theme.cosmic().on_bg_color());
        let (bg, border) = match skin_st {
            Some(st) => (with_alpha(st.bg_tint, 0.22), with_alpha(st.border_tint, 0.40)),
            None => (with_alpha(base, 0.04), with_alpha(base, 0.12)),
        };
        iced_container::Style {
            background: Some(Background::Color(bg)),
            border: Border { radius: corners::now().small.into(), width: 1.0, color: border },
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

fn pill_style(skin: &str) -> theme::Container<'static> {
    let skin_st = skin_style(skin);
    theme::Container::custom(move |theme: &cosmic::Theme| match skin_st {
        Some(st) => iced_container::Style {
            background: Some(Background::Color(with_alpha(st.accent, 0.24))),
            border: Border { radius: corners::now().pill.into(), width: 1.0, color: with_alpha(st.border_tint, 0.48) },
            ..Default::default()
        },
        None => iced_container::Style {
            background: Some(Background::Color(with_alpha(Color::from(theme.cosmic().on_bg_color()), 0.12))),
            border: Border { radius: corners::now().pill.into(), ..Default::default() },
            ..Default::default()
        },
    })
}

fn beat_pill_style(active: bool) -> theme::Container<'static> {
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let bg = if active {
            Color::from_rgb(0.95, 0.25, 0.45)
        } else {
            with_alpha(Color::from(theme.cosmic().on_bg_color()), 0.10)
        };
        iced_container::Style {
            background: Some(Background::Color(bg)),
            border: Border { radius: corners::now().pill.into(), ..Default::default() },
            ..Default::default()
        }
    })
}

fn meter_bar<'a>(label: String, value: f64, fill: Color) -> Element<'a, Message> {
    let clamped = value.clamp(0.0, 1.0);
    let pct = (clamped * 100.0).round() as u16;
    let fill_el = container(space::horizontal())
        .height(Length::Fill)
        .class(theme::Container::custom(move |_| iced_container::Style {
            background: Some(Background::Color(fill)),
            border: Border { radius: corners::now().pill.into(), ..Default::default() },
            ..Default::default()
        }));
    let track_inner: Element<Message> = if pct == 0 {
        space::horizontal().into()
    } else if pct >= 100 {
        fill_el.width(Length::Fill).into()
    } else {
        row![
            fill_el.width(Length::FillPortion(pct)),
            space::horizontal().width(Length::FillPortion(100 - pct)),
        ]
        .height(Length::Fill)
        .into()
    };
    let track = container(track_inner)
        .width(Length::Fill)
        .height(Length::Fixed(10.0))
        .class(theme::Container::custom(|theme: &cosmic::Theme| iced_container::Style {
            background: Some(Background::Color(with_alpha(Color::from(theme.cosmic().on_bg_color()), 0.12))),
            border: Border { radius: corners::now().pill.into(), ..Default::default() },
            ..Default::default()
        }));
    row![
        container(dim(label)).width(Length::Fixed(54.0)),
        track,
        container(dim(format!("{pct}%"))).width(Length::Fixed(36.0)).align_x(Alignment::End),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn segmented_bar_style(skin: &str) -> theme::Container<'static> {
    let skin_st = skin_style(skin);
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let base = Color::from(theme.cosmic().on_bg_color());
        let (bg, border, width) = match skin_st {
            Some(st) => (with_alpha(st.bg_tint, 0.28), with_alpha(st.border_tint, 0.48), st.border_width),
            None => (with_alpha(base, 0.06), with_alpha(base, 0.12), 1.0),
        };
        iced_container::Style {
            background: Some(Background::Color(bg)),
            border: Border { radius: corners::now().small.into(), width, color: border },
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

fn mini_badge<'a>(label: String, skin: &str) -> Element<'a, Message> {
    container(dim(label)).padding([1, 6]).class(pill_style(skin)).into()
}

fn bulb(on: bool) -> icon::Handle {
    let mut handle = icon::from_svg_bytes(if on { BULB_ON } else { BULB_OFF });
    handle.symbolic = true;
    handle
}

fn lamp_icon_handle(icon_key: &str) -> icon::Handle {
    let mut handle = icon::from_svg_bytes(lamp_icon_svg(icon_key));
    handle.symbolic = true;
    handle
}

fn lamp_icon_badge<'a>(icon_key: &str, tint: Option<Color>, reachable: bool, size: f32) -> Element<'a, Message> {
    let glyph_px = (size * 0.58).round().clamp(11.0, 22.0) as u16;
    let svg_icon = icon::icon(lamp_icon_handle(icon_key))
        .size(glyph_px)
        .class(theme::Svg::custom(move |theme: &cosmic::Theme| iced_svg::Style {
            color: Some(match tint {
                Some(c) => with_alpha(on_tint(c), if reachable { 1.0 } else { 0.5 }),
                None => with_alpha(Color::from(theme.cosmic().on_bg_color()), if reachable { 0.85 } else { 0.4 }),
            }),
        }));
    container(svg_icon)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .class(theme::Container::custom(move |theme: &cosmic::Theme| {
            let base = Color::from(theme.cosmic().on_bg_color());
            let fill = match tint {
                Some(c) => with_alpha(c, if reachable { 1.0 } else { 0.4 }),
                None => with_alpha(base, 0.14),
            };
            let border_col = if tint.is_some() && reachable {
                Color::WHITE
            } else {
                with_alpha(base, 0.35)
            };
            iced_container::Style {
                background: Some(Background::Color(fill)),
                border: Border { radius: (size * 0.5).into(), width: 1.2, color: border_col },
                ..Default::default()
            }
        }))
        .into()
}

fn panel_icon_handle(icon_key: &str, lit: bool) -> icon::Handle {
    match panel_icon_svg(icon_key) {
        Some(bytes) => {
            let mut handle = icon::from_svg_bytes(bytes);
            handle.symbolic = true;
            handle
        }
        None => bulb(lit),
    }
}

// ------------------------------------------------------------------------- applet

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct AiState {
    #[serde(alias = "running")]
    busy: bool,
    last_prompt: String,
    last_reply: String,
    last_actions: Vec<String>,
    engine: String,
    error: String,
}

impl AiState {
    fn from_stats(stats: &Value) -> Self {
        stats
            .get("ai")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ProviderDraft {
    id: String,
    kind: String,
    name: String,
    host: String,
    port: String,
    room_name: String,
    token: String,
    pair_id: String,
    use_https: bool,
    has_saved_token: bool,
    has_cert_fingerprint: bool,
    original_kind: String,
    original_host: String,
    original_port: String,
    awaiting_save: bool,
}

const EXTRA_PROVIDERS: [(&str, &str, &str, u16, bool, bool); 8] = [
    ("home_assistant", "Home Assistant", "HA", 8123, true, false),
    ("wled", "WLED", "WLED", 80, false, false),
    ("wiz", "WiZ", "WiZ", 38899, false, false),
    ("lifx", "LIFX", "LIFX", 56700, false, false),
    ("govee", "Govee LAN", "Govee", 4003, false, false),
    ("nanoleaf", "Nanoleaf", "Nanoleaf", 16021, true, true),
    ("shelly", "Shelly", "Shelly", 80, false, false),
    ("dirigera", "IKEA DIRIGERA", "IKEA", 8443, true, true),
];

fn provider_meta(kind: &str) -> Option<(&'static str, &'static str, &'static str, u16, bool, bool)> {
    EXTRA_PROVIDERS.iter().copied().find(|(k, _, _, _, _, _)| *k == kind)
}

fn provider_badge_color(kind: &str) -> Color {
    match kind {
        "home_assistant" => Color::from_rgb(3.0 / 255.0, 169.0 / 255.0, 244.0 / 255.0),
        "wled" => Color::from_rgb(236.0 / 255.0, 72.0 / 255.0, 153.0 / 255.0),
        "wiz" => Color::from_rgb(139.0 / 255.0, 92.0 / 255.0, 246.0 / 255.0),
        "lifx" => Color::from_rgb(16.0 / 255.0, 185.0 / 255.0, 129.0 / 255.0),
        "govee" => Color::from_rgb(59.0 / 255.0, 130.0 / 255.0, 246.0 / 255.0),
        "nanoleaf" => Color::from_rgb(34.0 / 255.0, 197.0 / 255.0, 94.0 / 255.0),
        "shelly" => Color::from_rgb(6.0 / 255.0, 182.0 / 255.0, 212.0 / 255.0),
        "dirigera" => Color::from_rgb(234.0 / 255.0, 179.0 / 255.0, 8.0 / 255.0),
        _ => Color::from_rgb(245.0 / 255.0, 158.0 / 255.0, 11.0 / 255.0),
    }
}

fn provider_pill<'a>(kind: &str) -> Option<Element<'a, Message>> {
    let (_, _, badge, _, _, _) = provider_meta(kind)?;
    let tint = provider_badge_color(kind);
    Some(
        container(text::caption(badge).class(theme::Text::Color(tint)))
            .padding([1, 6])
            .class(theme::Container::custom(move |_| iced_container::Style {
                background: Some(Background::Color(with_alpha(tint, 0.14))),
                border: Border {
                    radius: corners::now().pill.into(),
                    width: 1.0,
                    color: with_alpha(tint, 0.35),
                },
                ..Default::default()
            }))
            .into(),
    )
}

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
    floorplan: FloorplanState,
    filter: RoomFilter,
    expanded: HashSet<String>,
    all_scenes: HashSet<String>,
    open_light_drawer: Option<String>,
    open_lights_menu: Option<String>,
    motion_hint_dismissed: bool,
    favorites: Vec<Favorite>,
    exclude_all_on: HashSet<String>,
    confirm_all_on_pref: bool,
    confirm_all_on: bool,
    schedule_draft: Option<ScheduleDraft>,
    rule_draft: Option<RuleDraft>,
    theme_draft: Option<ThemeDraft>,
    provider_draft: Option<ProviderDraft>,
    confirm_remove_provider: Option<String>,
    confirm_delete_schedule: Option<String>,
    confirm_delete_rule: Option<String>,
    activity_open: bool,
    // A slider being dragged: its value, and when it was last sent.
    dragging: HashMap<String, (u8, Instant)>,
    synth_sens_drag: Option<(f32, Instant)>,
    synth_smooth_drag: Option<(f32, Instant)>,
    synth_room_picker_open: bool,
    ip: String,
    confirm_unpair: bool,
    language_error: bool,
    // F12: the note's state, whether a capture is on its way, the settings.
    shot: shot::Phase,
    capturing: bool,
    shot_key: bool,
    corners: String,
    schedule_mode: String,
    skin: String,
    panel_icon: String,
    show_count: bool,
    font_scale: String,
    setting_error: bool,
    resize: window_size::Debounce,
    /// The settings page is shown instead of the tabs.
    settings_open: bool,
    /// The animated Help & Feature Guide page is shown instead of the tabs.
    help_open: bool,
    help_selected: Option<usize>,
    help_phase: f32,
    /// Optional local Ask AI (OFF by default — opt-in in Settings).
    ask_ai_enabled: bool,
    ask_ai_open: bool,
    ask_ai_input: String,
    ai_state: AiState,
    language_open: bool,
    corners_open: bool,
    skin_open: bool,
    panel_icon_open: bool,
    font_scale_open: bool,
    schedule_mode_open: bool,
    cached_theme: Option<cosmic::Theme>,
    look_refreshing: bool,
    pending_sync_count: usize,
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
    Map(MapMsg),
    SetFilter(RoomFilter),
    Expand(String),
    AllScenes(String),
    ToggleLightDrawer(String),
    ToggleLightsMenu(String),
    LightsMenuBrightness(String),
    LightsMenuColor(String),
    LightsMenuRename(String),
    LightsMenuIcon(String),
    LightsMenuWireOnMap(String),
    DismissMotionHint,
    ToggleFavorite(String, String),
    ToggleExcludeAllOn(String, bool),
    LightMirek(String, u16),
    LightXy(String, [f64; 2]),
    LightEffect(String, String),
    // Light themes
    ThemeApply(String, String),
    ThemeDelete(String),
    StartSaveTheme(String),
    ThemeDraftName(String),
    ThemeDraftSave,
    ThemeDraftCancel,
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
    ScheduleConfirmDelete(Option<String>),
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
    RuleConfirmDelete(Option<String>),
    RuleDelete(String),
    ToggleActivity,
    // Synth
    SynthStart,
    SynthStop,
    SynthSource(String),
    SynthMode(String),
    SynthPalette(String),
    SynthToggleRoomPicker,
    SynthPickRoom(String),
    SynthSensitivity(u16),
    SynthSensitivityDone,
    SynthSmoothness(u16),
    SynthSmoothnessDone,
    SynthLightBand(String, String),
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
    SetScheduleMode(String),
    SetSkin(String),
    SetPanelIcon(String),
    SetShowCount(bool),
    SetFontScale(String),
    SetAskAi(bool),
    ToggleSettings,
    ToggleHelp,
    HelpSelect(Option<usize>),
    HelpTry(help::HelpAction),
    ToggleAskAi,
    AskAiInput(String),
    AskAiSubmit,
    AskAiExample(String),
    AskAiClear,
    HelpTick,
    ToggleLanguage,
    ToggleCorners,
    ToggleSkin,
    TogglePanelIcon,
    ToggleFontScale,
    ToggleScheduleMode,
    LookRefreshed(bool),
    OpenAddProvider(Option<String>),
    EditProvider(String),
    CloseProviderDraft,
    ProviderKind(String),
    ProviderName(String),
    ProviderHost(String),
    ProviderPort(String),
    ProviderRoomName(String),
    ProviderToken(String),
    ProviderHttps(bool),
    ProviderUseDiscovered(String, String, String, u16),
    DiscoverProviders,
    TestProvider,
    PairProvider,
    SaveProvider,
    ToggleProvider(String, bool),
    RemoveProvider(String),
    CancelRemoveProvider,
    TrustProviderCert(String),
    AddAllDiscoveredProviders,
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

fn off_thread_value<T: Default + Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: fn(T) -> Message,
) -> Task<Message> {
    let (sender, receiver) = cosmic::iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    Task::perform(receiver, move |result| cosmic::Action::App(done(result.unwrap_or_default())))
}

// Settings from config.json.
fn shot_key_setting() -> bool {
    read_pref("screenshot_key").and_then(|v| v.as_bool()).unwrap_or(true)
}

fn confirm_all_on_setting() -> bool {
    read_pref(CONFIRM_ALL_ON_KEY).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn ask_ai_setting() -> bool {
    read_pref(ASK_AI_KEY).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn corners_setting() -> String {
    corners::normalize(read_pref("corners").as_ref().and_then(Value::as_str).unwrap_or("")).to_string()
}

fn schedule_mode_setting() -> String {
    normalize_schedule_mode(read_pref(SCHEDULE_MODE_KEY).as_ref().and_then(Value::as_str).unwrap_or("")).to_string()
}

fn skin_setting() -> String {
    normalize_skin(read_pref(SKIN_KEY).as_ref().and_then(Value::as_str).unwrap_or("")).to_string()
}

fn panel_icon_setting() -> String {
    normalize_panel_icon(read_pref(PANEL_ICON_KEY).as_ref().and_then(Value::as_str).unwrap_or("")).to_string()
}

fn show_count_setting() -> bool {
    read_pref(SHOW_COUNT_KEY).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn font_scale_setting() -> String {
    normalize_font_scale(read_pref(FONT_SCALE_KEY).as_ref().and_then(Value::as_str).unwrap_or("")).to_string()
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

    fn refresh_pending_sync(&mut self) {
        self.pending_sync_count = compute_pending_sync_count(&self.floorplan.doc, &self.stats);
    }

    fn save_floorplan(&mut self, trigger_auto_sync: bool) {
        self.floorplan.local_dirty = false;
        self.refresh_pending_sync();
        self.send(cmd_floorplan_save(&self.floorplan.doc));
        if trigger_auto_sync && self.floorplan.doc.auto_sync {
            self.send(cmd_floorplan_sync_all());
        }
    }

    fn handle_map_msg(&mut self, msg: MapMsg) -> Task<Message> {
        match msg {
            MapMsg::ToggleSidebar => {
                self.floorplan.sidebar_open = !self.floorplan.sidebar_open;
            }
            MapMsg::SearchChanged(q) => {
                self.floorplan.search = q.chars().take(60).collect();
            }
            MapMsg::ToggleAddMenu => {
                let next = !self.floorplan.add_menu_open;
                self.floorplan.close_menus();
                self.floorplan.add_menu_open = next;
            }
            MapMsg::ToggleViewMenu => {
                let next = !self.floorplan.view_menu_open;
                self.floorplan.close_menus();
                self.floorplan.view_menu_open = next;
            }
            MapMsg::ToggleLampMenu(lid) => {
                let next = if self.floorplan.open_lamp_menu.as_deref() == Some(lid.as_str()) {
                    None
                } else {
                    Some(lid)
                };
                self.floorplan.open_sensor_menu = None;
                self.floorplan.open_lamp_menu = next;
            }
            MapMsg::ToggleSensorMenu(sid) => {
                let next = if self.floorplan.open_sensor_menu.as_deref() == Some(sid.as_str()) {
                    None
                } else {
                    Some(sid)
                };
                self.floorplan.open_lamp_menu = None;
                self.floorplan.open_sensor_menu = next;
            }
            MapMsg::PlaceSensor(sid) => {
                self.floorplan.armed_place_lamp = None;
                self.floorplan.open_sensor_menu = None;
                let sensors = parse_sensors(&self.stats);
                let room_id = sensors
                    .iter()
                    .find(|s| s.id == sid)
                    .map(|s| s.room_id.as_str())
                    .unwrap_or("");
                let (x, y, floor_id) = self
                    .floorplan
                    .doc
                    .boxes
                    .iter()
                    .find(|b| !room_id.is_empty() && b.hue_id.as_deref() == Some(room_id))
                    .map(|b| (b.x + (b.w * 0.5 + 36.0).min(b.w - 24.0), b.y + (b.h * 0.5).max(32.0), b.floor_id.clone()))
                    .unwrap_or((
                        self.floorplan.doc.canvas.width * 0.5,
                        self.floorplan.doc.canvas.height * 0.5,
                        self.floorplan.doc.active_floor_id.clone(),
                    ));
                self.floorplan.doc.sensors.insert(sid.clone(), FloorplanSensor { x, y, floor_id });
                self.floorplan.select(Some(Selection::Sensor(sid)), &self.stats);
                self.save_floorplan(false);
            }
            MapMsg::FocusSensor(sid) => {
                self.floorplan.armed_place_lamp = None;
                self.floorplan.select(Some(Selection::Sensor(sid)), &self.stats);
            }
            MapMsg::UnplaceSensor(sid) => {
                self.floorplan.open_sensor_menu = None;
                self.floorplan.doc.sensors.remove(&sid);
                if matches!(&self.floorplan.selection, Some(Selection::Sensor(id)) if id == &sid) {
                    self.floorplan.selection = None;
                }
                self.save_floorplan(false);
            }
            MapMsg::MoveSensor { id, x, y, commit } => {
                if let Some(pos) = self.floorplan.doc.sensors.get_mut(&id) {
                    pos.x = x;
                    pos.y = y;
                }
                self.floorplan.local_dirty = !commit;
                if commit {
                    self.save_floorplan(false);
                }
            }
            MapMsg::SensorToggle { kind, id, enabled } => {
                if self.floorplan.open_sensor_menu.as_deref() == Some(id.as_str()) {
                    self.floorplan.open_sensor_menu = None;
                }
                self.send(cmd_sensor_toggle(&kind, &id, enabled));
            }
            MapMsg::SensorStartWire { kind, id } => {
                self.floorplan.open_lamp_menu = None;
                self.floorplan.open_sensor_menu = None;
                self.floorplan.close_dialog();
                self.floorplan.mode = CanvasMode::Wire;
                self.floorplan.pending_wire_from = Some((kind, id));
            }
            MapMsg::MotionAreaToggle(area_id, enabled) => {
                self.send(cmd_motion_area_update(&area_id, Some(enabled), None));
            }
            MapMsg::MotionAreaSensitivity(area_id, val) => {
                self.send(cmd_motion_area_update(&area_id, None, Some(val)));
            }
            MapMsg::DismissMotionHint => {
                self.motion_hint_dismissed = true;
            }
            MapMsg::OpenLampDialog(dialog) => {
                self.floorplan.open_lamp_dialog(dialog, &self.stats);
            }
            MapMsg::CloseLampDialog => {
                self.floorplan.close_dialog();
            }
            MapMsg::ToggleTemplatesMenu => {
                let next = !self.floorplan.templates_open;
                self.floorplan.close_menus();
                self.floorplan.templates_open = next;
            }
            MapMsg::ToggleGalleryIncludePlan(on) => {
                self.floorplan.gallery_include_plan = on;
            }
            MapMsg::ApplyTemplate(tid) => {
                // E3: Require 2-click confirm when replacing an existing non-empty map + save Undo snapshot
                if !self.floorplan.doc.boxes.is_empty()
                    && self.floorplan.confirm_template_id.as_deref() != Some(tid.as_str())
                {
                    self.floorplan.confirm_template_id = Some(tid);
                } else {
                    self.floorplan.confirm_template_id = None;
                    self.floorplan.undo_doc = Some(self.floorplan.doc.clone());
                    self.floorplan.close_menus();
                    self.floorplan.close_dialog();
                    self.floorplan.local_dirty = false;
                    self.floorplan.selection = None;
                    self.refresh_pending_sync();
                    self.send(cmd_floorplan_template_with_plan(&tid, self.floorplan.gallery_include_plan));
                }
            }
            MapMsg::AddBox(kind) => {
                self.floorplan.close_menus();
                let is_zone = kind == "zone";
                let id = self.floorplan.next_unique_id(if is_zone { "zone" } else { "box" });
                let offset = ((self.floorplan.doc.boxes.len() % 5) as f64) * 30.0;
                let bx = (120.0 + offset).min((self.floorplan.doc.canvas.width - 300.0).max(40.0));
                let by = (120.0 + offset).min((self.floorplan.doc.canvas.height - 240.0).max(40.0));
                let name = if is_zone { tr("New Zone") } else { tr("New Room") };
                let floor_id = self.floorplan.doc.active_floor_id.clone();
                self.floorplan.doc.boxes.push(FloorplanBox {
                    id: id.clone(),
                    kind: if is_zone { "zone".into() } else { "room".into() },
                    hue_id: None,
                    name,
                    archetype: if is_zone { "other".into() } else { "living_room".into() },
                    x: bx,
                    y: by,
                    w: 320.0,
                    h: 220.0,
                    points: Vec::new(),
                    floor_id,
                    entertainment_area_id: None,
                });
                self.floorplan.select(Some(Selection::Box(id)), &self.stats);
                self.save_floorplan(true);
            }
            MapMsg::AddBoxWithShape(kind, shape) => {
                self.floorplan.close_menus();
                let is_zone = kind == "zone";
                let id = self.floorplan.next_unique_id(if is_zone { "zone" } else { "box" });
                let offset = ((self.floorplan.doc.boxes.len() % 5) as f64) * 30.0;
                let bx = (120.0 + offset).min((self.floorplan.doc.canvas.width - 360.0).max(40.0));
                let by = (120.0 + offset).min((self.floorplan.doc.canvas.height - 280.0).max(40.0));
                let name = if is_zone { tr("New Zone") } else { tr("New Room") };
                let floor_id = self.floorplan.doc.active_floor_id.clone();
                let mut b = FloorplanBox {
                    id: id.clone(),
                    kind: if is_zone { "zone".into() } else { "room".into() },
                    hue_id: None,
                    name,
                    archetype: if is_zone { "other".into() } else { "living_room".into() },
                    x: bx,
                    y: by,
                    w: 340.0,
                    h: 260.0,
                    points: Vec::new(),
                    floor_id,
                    entertainment_area_id: None,
                };
                b.apply_shape(&shape);
                self.floorplan.doc.boxes.push(b);
                self.floorplan.select(Some(Selection::Box(id)), &self.stats);
                self.save_floorplan(true);
            }
            MapMsg::AddStamp(kind) => {
                self.floorplan.close_menus();
                let id = self.floorplan.next_unique_id("st");
                let (w, h) = match kind.as_str() {
                    "door" => (52.0, 52.0),
                    "window" => (180.0, 12.0),
                    "sofa" => (170.0, 72.0),
                    "tv" => (150.0, 18.0),
                    "desk" => (160.0, 72.0),
                    "bed" => (190.0, 155.0),
                    "dining" => (150.0, 90.0),
                    "kitchen" => (240.0, 54.0),
                    "stairs" => (96.0, 130.0),
                    _ => (100.0, 60.0),
                };
                let cx = (self.floorplan.doc.canvas.width * 0.5 - w * 0.5).clamp(20.0, 3600.0);
                let cy = (self.floorplan.doc.canvas.height * 0.5 - h * 0.5).clamp(20.0, 2600.0);
                let floor_id = self.floorplan.doc.active_floor_id.clone();
                self.floorplan.doc.stamps.push(FloorplanStamp {
                    id: id.clone(),
                    kind,
                    x: cx,
                    y: cy,
                    w,
                    h,
                    rot: 0,
                    floor_id,
                });
                self.floorplan.select(Some(Selection::Stamp(id)), &self.stats);
                self.save_floorplan(false);
            }
            MapMsg::SetAmbient(mode) => {
                self.floorplan.ambient = mode;
            }
            MapMsg::SelectFloor(fid) => {
                if self.floorplan.doc.floors.iter().any(|f| f.id == fid) {
                    self.floorplan.doc.active_floor_id = fid;
                    self.floorplan.selection = None;
                    self.save_floorplan(false);
                }
            }
            MapMsg::AddFloor => {
                let next_level = self
                    .floorplan
                    .doc
                    .floors
                    .iter()
                    .map(|f| f.level)
                    .max()
                    .unwrap_or(0)
                    + 1;
                let mut idx = self.floorplan.doc.floors.len() + 1;
                let fid = loop {
                    let c = format!("fl-{idx}");
                    if !self.floorplan.doc.floors.iter().any(|f| f.id == c) {
                        break c;
                    }
                    idx += 1;
                };
                let name = match next_level {
                    1 => tr("Upper Floor"),
                    2 => tr("Attic"),
                    n => trf("Floor {0}", &[n.to_string()]),
                };
                self.floorplan.doc.floors.push(FloorplanFloor {
                    id: fid.clone(),
                    name,
                    level: next_level,
                });
                self.floorplan.doc.active_floor_id = fid;
                self.floorplan.floor_rename_open = false;
                self.floorplan.selection = None;
                self.save_floorplan(false);
            }
            MapMsg::FloorRenameStart => {
                if let Some(fl) = self
                    .floorplan
                    .doc
                    .floors
                    .iter()
                    .find(|f| f.id == self.floorplan.doc.active_floor_id)
                {
                    self.floorplan.floor_rename_draft = tr(&fl.name);
                    self.floorplan.floor_rename_open = true;
                }
            }
            MapMsg::FloorRenameInput(v) => {
                self.floorplan.floor_rename_draft = v.chars().take(32).collect();
            }
            MapMsg::FloorRenameSave => {
                let clean = self.floorplan.floor_rename_draft.trim().to_string();
                if !clean.is_empty()
                    && let Some(fl) = self
                        .floorplan
                        .doc
                        .floors
                        .iter_mut()
                        .find(|f| f.id == self.floorplan.doc.active_floor_id)
                {
                    fl.name = clean;
                    self.floorplan.floor_rename_open = false;
                    self.save_floorplan(false);
                }
            }
            MapMsg::FloorRenameCancel => {
                self.floorplan.floor_rename_open = false;
            }
            MapMsg::UndoFloorplan => {
                if let Some(prev) = self.floorplan.undo_doc.take() {
                    self.floorplan.doc = prev;
                    self.floorplan.selection = None;
                    self.floorplan.floor_rename_open = false;
                    self.save_floorplan(false);
                }
            }
            MapMsg::DeleteFloor(fid) => {
                if self.floorplan.doc.floors.len() > 1 {
                    self.floorplan.undo_doc = Some(self.floorplan.doc.clone());
                    self.floorplan.doc.floors.retain(|f| f.id != fid);
                    let fallback = self
                        .floorplan
                        .doc
                        .floors
                        .first()
                        .map(|f| f.id.clone())
                        .unwrap_or_else(|| "eg".into());
                    self.floorplan.doc.boxes.retain(|b| b.floor_id != fid);
                    self.floorplan.doc.stamps.retain(|s| s.floor_id != fid);
                    self.floorplan.doc.planned_lights.retain(|p| p.floor_id != fid);
                    for l in self.floorplan.doc.lamps.values_mut() {
                        if l.floor_id == fid {
                            l.floor_id = fallback.clone();
                        }
                    }
                    for s in self.floorplan.doc.sensors.values_mut() {
                        if s.floor_id == fid {
                            s.floor_id = fallback.clone();
                        }
                    }
                    if self.floorplan.doc.active_floor_id == fid {
                        self.floorplan.doc.active_floor_id = fallback;
                    }
                    self.floorplan.selection = None;
                    self.save_floorplan(false);
                }
            }
            MapMsg::BoxSetFloor(bid, fid) => {
                if self.floorplan.doc.floors.iter().any(|f| f.id == fid)
                    && let Some(b_snap) = self.floorplan.doc.boxes.iter().find(|b| b.id == bid).cloned()
                {
                    let old_floor = b_snap.floor_id.clone();
                    if old_floor != fid {
                        for l in self.floorplan.doc.lamps.values_mut() {
                            if l.floor_id == old_floor && b_snap.contains_point(l.x, l.y) {
                                l.floor_id = fid.clone();
                            }
                        }
                        for s in self.floorplan.doc.sensors.values_mut() {
                            if s.floor_id == old_floor && b_snap.contains_point(s.x, s.y) {
                                s.floor_id = fid.clone();
                            }
                        }
                        for pl in &mut self.floorplan.doc.planned_lights {
                            let (px, py) = pl.anchor();
                            if pl.floor_id == old_floor && b_snap.contains_point(px, py) {
                                pl.floor_id = fid.clone();
                            }
                        }
                        if let Some(b) = self.floorplan.doc.boxes.iter_mut().find(|b| b.id == bid) {
                            b.floor_id = fid;
                        }
                        self.floorplan.selection = None;
                        self.save_floorplan(false);
                    }
                }
            }
            MapMsg::BoxApplyShape(bid, shape) => {
                if let Some(b) = self.floorplan.doc.boxes.iter_mut().find(|b| b.id == bid) {
                    b.apply_shape(&shape);
                    self.save_floorplan(true);
                }
            }
            MapMsg::MoveBoxVertex { box_id, index, x, y, commit } => {
                if let Some(b) = self.floorplan.doc.boxes.iter_mut().find(|b| b.id == box_id) {
                    b.move_vertex(index, x, y);
                }
                self.floorplan.local_dirty = !commit;
                if commit {
                    self.save_floorplan(true);
                }
            }
            MapMsg::InsertBoxVertex { box_id, after_edge, x, y } => {
                if let Some(b) = self.floorplan.doc.boxes.iter_mut().find(|b| b.id == box_id) {
                    b.insert_vertex(after_edge, x, y);
                    self.save_floorplan(true);
                }
            }
            MapMsg::RemoveBoxVertex { box_id, index } => {
                if let Some(b) = self.floorplan.doc.boxes.iter_mut().find(|b| b.id == box_id) {
                    b.remove_vertex(index);
                    self.save_floorplan(true);
                }
            }
            MapMsg::TogglePlanAddMenu => {
                let next = !self.floorplan.plan_add_open;
                self.floorplan.close_menus();
                self.floorplan.plan_add_open = next;
            }
            MapMsg::TogglePlanList => {
                let next = !self.floorplan.plan_list_open;
                self.floorplan.close_menus();
                self.floorplan.plan_list_open = next;
            }
            MapMsg::AddPlannedLight(preset_id) => {
                self.floorplan.close_menus();
                let preset = PLANNED_LIGHT_PRESETS
                    .iter()
                    .find(|p| p.id == preset_id)
                    .unwrap_or(&PLANNED_LIGHT_PRESETS[0]);
                let id = self.floorplan.next_unique_id("plan");
                let offset = ((self.floorplan.doc.planned_lights.len() % 6) as f64) * 26.0;
                let cx = (self.floorplan.doc.canvas.width * 0.5 - 60.0 + offset)
                    .clamp(60.0, self.floorplan.doc.canvas.width - 60.0);
                let cy = (self.floorplan.doc.canvas.height * 0.5 - 40.0 + offset)
                    .clamp(60.0, self.floorplan.doc.canvas.height - 60.0);
                let strip_points = if preset.is_strip {
                    vec![
                        FloorplanPoint { x: cx - 70.0, y: cy },
                        FloorplanPoint { x: cx, y: cy },
                        FloorplanPoint { x: cx + 70.0, y: cy },
                    ]
                } else {
                    Vec::new()
                };
                let floor_id = self.floorplan.doc.active_floor_id.clone();
                self.floorplan.doc.planned_lights.push(PlannedLight {
                    id: id.clone(),
                    name: tr(preset.label),
                    icon: preset.icon.to_string(),
                    floor_id,
                    x: cx,
                    y: cy,
                    strip_points,
                    lumens: preset.lumens,
                    beam_radius: preset.beam_radius,
                    kelvin: preset.kelvin,
                    color: kelvin_to_hex(preset.kelvin),
                    on: true,
                    price: preset.price,
                    model_note: String::new(),
                });
                self.floorplan.select(Some(Selection::Planned(id)), &self.stats);
                self.save_floorplan(false);
            }
            MapMsg::MovePlannedLight { id, x, y, commit } => {
                if let Some(pl) = self.floorplan.doc.planned_lights.iter_mut().find(|p| p.id == id) {
                    pl.x = x;
                    pl.y = y;
                }
                self.floorplan.local_dirty = !commit;
                if commit {
                    self.save_floorplan(false);
                }
            }
            MapMsg::MovePlannedStripVertex { id, index, x, y, commit } => {
                if let Some(pl) = self.floorplan.doc.planned_lights.iter_mut().find(|p| p.id == id)
                    && index < pl.strip_points.len()
                {
                    pl.strip_points[index] = FloorplanPoint { x, y };
                    let (ax, ay) = pl.anchor();
                    pl.x = ax;
                    pl.y = ay;
                }
                self.floorplan.local_dirty = !commit;
                if commit {
                    self.save_floorplan(false);
                }
            }
            MapMsg::TogglePlannedLight(id) => {
                if let Some(pl) = self.floorplan.doc.planned_lights.iter_mut().find(|p| p.id == id) {
                    pl.on = !pl.on;
                    self.save_floorplan(false);
                }
            }
            MapMsg::PlannedSetLumens(id, lm) => {
                if let Some(pl) = self.floorplan.doc.planned_lights.iter_mut().find(|p| p.id == id) {
                    pl.lumens = lm.clamp(50, 6000);
                    self.save_floorplan(false);
                }
            }
            MapMsg::PlannedSetKelvin(id, k) => {
                if let Some(pl) = self.floorplan.doc.planned_lights.iter_mut().find(|p| p.id == id) {
                    let ck = k.clamp(2000, 6500);
                    pl.kelvin = ck;
                    pl.color = kelvin_to_hex(ck);
                    self.save_floorplan(false);
                }
            }
            MapMsg::PlannedSetBeam(id, r) => {
                if let Some(pl) = self.floorplan.doc.planned_lights.iter_mut().find(|p| p.id == id) {
                    pl.beam_radius = r.clamp(0.8, 10.0);
                    self.save_floorplan(false);
                }
            }
            MapMsg::PlannedNameInput(s) => {
                self.floorplan.planned_name_draft = s.chars().take(48).collect();
            }
            MapMsg::PlannedNameSave(id) => {
                let name = self.floorplan.planned_name_draft.trim().to_string();
                if !name.is_empty()
                    && let Some(pl) = self.floorplan.doc.planned_lights.iter_mut().find(|p| p.id == id)
                {
                    pl.name = name;
                    self.save_floorplan(false);
                }
            }
            MapMsg::PlannedPriceInput(s) => {
                self.floorplan.planned_price_draft = s.chars().take(12).collect();
            }
            MapMsg::PlannedPriceSave(id) => {
                let cleaned = self.floorplan.planned_price_draft.trim().replace(',', ".");
                if let Ok(val) = cleaned.parse::<f64>()
                    && let Some(pl) = self.floorplan.doc.planned_lights.iter_mut().find(|p| p.id == id)
                {
                    pl.price = val.clamp(0.0, 99999.0);
                    self.save_floorplan(false);
                }
            }
            MapMsg::BindPlannedToReal { planned_id, real_light_id } => {
                if let Some(pos) = self.floorplan.doc.planned_lights.iter().position(|p| p.id == planned_id) {
                    let pl = self.floorplan.doc.planned_lights.remove(pos);
                    let (ax, ay) = pl.anchor();
                    self.floorplan.doc.lamps.insert(
                        real_light_id.clone(),
                        FloorplanLamp {
                            x: ax,
                            y: ay,
                            z: 0.0,
                            icon: normalize_lamp_icon(&pl.icon).to_string(),
                            floor_id: pl.floor_id,
                        },
                    );
                    self.floorplan.select(Some(Selection::Lamp(real_light_id)), &self.stats);
                    self.save_floorplan(true);
                }
            }
            MapMsg::DeletePlannedLight(id) => {
                self.floorplan.doc.planned_lights.retain(|p| p.id != id);
                if matches!(&self.floorplan.selection, Some(Selection::Planned(pid)) if pid == &id) {
                    self.floorplan.selection = None;
                }
                self.save_floorplan(false);
            }
            MapMsg::SetMode(mode) => {
                self.floorplan.close_menus();
                self.floorplan.mode = mode;
                self.floorplan.armed_place_lamp = None;
                if mode != CanvasMode::Wire {
                    self.floorplan.pending_wire_from = None;
                }
            }
            MapMsg::ToggleGlows => {
                self.floorplan.doc.show_glows = !self.floorplan.doc.show_glows;
                self.save_floorplan(false);
            }
            MapMsg::ToggleFurniture => {
                self.floorplan.doc.show_furniture = !self.floorplan.doc.show_furniture;
                self.save_floorplan(false);
            }
            MapMsg::ToggleWires => {
                self.floorplan.doc.show_wires = !self.floorplan.doc.show_wires;
                self.save_floorplan(false);
            }
            MapMsg::ToggleAutoSync(on) => {
                self.floorplan.doc.auto_sync = on;
                self.floorplan.doc.auto_sync_rooms = on;
                self.save_floorplan(on);
            }
            MapMsg::SyncAll => {
                self.save_floorplan(false);
                self.send(cmd_floorplan_sync_all());
            }
            MapMsg::ToggleEntMenu => {
                let next = !self.floorplan.ent_menu_open;
                self.floorplan.close_menus();
                self.floorplan.ent_menu_open = next;
            }
            MapMsg::SetTopEntertainment(ent_id) => {
                self.floorplan.ent_menu_open = false;
                self.floorplan.doc.entertainment_id = ent_id;
                self.save_floorplan(true);
            }
            // Background image controls
            MapMsg::ToggleBgMenu => {
                let next = !self.floorplan.bg_open;
                self.floorplan.close_menus();
                self.floorplan.bg_open = next;
                if next && self.floorplan.bg_path_draft.is_empty() {
                    self.floorplan.bg_path_draft = self.floorplan.doc.background.path.clone();
                }
            }
            MapMsg::ChooseBgFile => {
                let title = tr("Choose floorplan image");
                let filter_lbl = tr("Images");
                return cosmic::task::future(async move {
                    use cosmic::dialog::file_chooser;
                    let filter = file_chooser::FileFilter::new(&filter_lbl)
                        .glob("*.png")
                        .glob("*.jpg")
                        .glob("*.jpeg")
                        .glob("*.webp");
                    let dialog = file_chooser::open::Dialog::new().title(title).filter(filter);
                    let picked = match dialog.open_file().await {
                        Ok(resp) => resp.url().to_file_path().ok().map(|p| p.to_string_lossy().into_owned()),
                        Err(_) => None,
                    };
                    cosmic::Action::App(Message::Map(MapMsg::BgFilePicked(picked)))
                });
            }
            MapMsg::BgFilePicked(Some(path)) => {
                self.floorplan.bg_path_draft = path.clone();
                let op = self.floorplan.doc.background.opacity;
                let fit = self.floorplan.doc.background.fit.clone();
                self.floorplan.local_dirty = false;
                self.send(cmd_floorplan_bg_set(&path, op, &fit));
            }
            MapMsg::BgFilePicked(None) => {}
            MapMsg::BgPathInput(p) => {
                self.floorplan.bg_path_draft = p.chars().take(512).collect();
            }
            MapMsg::BgApplyPath => {
                let p = self.floorplan.bg_path_draft.trim().to_string();
                if !p.is_empty() {
                    let op = self.floorplan.doc.background.opacity;
                    let fit = self.floorplan.doc.background.fit.clone();
                    self.floorplan.local_dirty = false;
                    self.send(cmd_floorplan_bg_set(&p, op, &fit));
                }
            }
            MapMsg::BgOpacity(pct) => {
                let clamped = pct.clamp(5, 100);
                self.floorplan.bg_opacity_drag = Some(clamped);
                self.floorplan.doc.background.opacity = (clamped as f64) / 100.0;
            }
            MapMsg::BgOpacityDone => {
                if let Some(pct) = self.floorplan.bg_opacity_drag.take() {
                    let op = (pct.clamp(5, 100) as f64) / 100.0;
                    self.floorplan.doc.background.opacity = op;
                    let fit = self.floorplan.doc.background.fit.clone();
                    self.send(cmd_floorplan_bg_update(op, &fit));
                }
            }
            MapMsg::BgFit(fit) => {
                let f = if fit == "cover" { "cover".to_string() } else { "contain".to_string() };
                self.floorplan.doc.background.fit = f.clone();
                let op = self.floorplan.doc.background.opacity;
                self.send(cmd_floorplan_bg_update(op, &f));
            }
            MapMsg::BgClear => {
                self.floorplan.doc.background.path.clear();
                self.floorplan.bg_path_draft.clear();
                self.floorplan.local_dirty = false;
                self.floorplan.refresh_bg_cache();
                self.send(cmd_floorplan_bg_clear());
            }
            // Sidebar lamp actions
            MapMsg::PlaceLamp(lid) => {
                self.floorplan.armed_place_lamp = None;
                self.floorplan.open_lamp_menu = None;
                let lights = extract_bridge_lights(&self.stats);
                let room_id = lights
                    .iter()
                    .find(|l| l.id == lid)
                    .map(|l| l.room_id.as_str())
                    .unwrap_or("");
                let (x, y, floor_id) = self
                    .floorplan
                    .doc
                    .boxes
                    .iter()
                    .find(|b| !room_id.is_empty() && b.hue_id.as_deref() == Some(room_id))
                    .map(|b| (b.x + b.w * 0.5, b.y + b.h * 0.5, b.floor_id.clone()))
                    .unwrap_or((
                        self.floorplan.doc.canvas.width * 0.5,
                        self.floorplan.doc.canvas.height * 0.5,
                        self.floorplan.doc.active_floor_id.clone(),
                    ));
                self.floorplan.doc.lamps.insert(
                    lid.clone(),
                    FloorplanLamp { x, y, z: 0.0, icon: "bulb".into(), floor_id },
                );
                self.floorplan.select(Some(Selection::Lamp(lid)), &self.stats);
                self.save_floorplan(true);
            }
            MapMsg::ArmPlaceLamp(lid) => {
                self.floorplan.armed_place_lamp = Some(lid);
                self.floorplan.open_lamp_menu = None;
                self.floorplan.mode = CanvasMode::Select;
            }
            MapMsg::FocusLamp(lid) => {
                self.floorplan.armed_place_lamp = None;
                self.floorplan.select(Some(Selection::Lamp(lid)), &self.stats);
            }
            MapMsg::UnplaceLamp(lid) => {
                self.floorplan.open_lamp_menu = None;
                if matches!(&self.floorplan.lamp_dialog, Some(LampDialog::Icon(id)) if id == &lid) {
                    self.floorplan.close_dialog();
                }
                self.floorplan.doc.lamps.remove(&lid);
                if matches!(&self.floorplan.selection, Some(Selection::Lamp(id)) if id == &lid) {
                    self.floorplan.selection = None;
                }
                self.save_floorplan(false);
            }
            // Canvas drag & click interactions
            MapMsg::Select(sel) => {
                self.floorplan.armed_place_lamp = None;
                if sel.is_none() {
                    self.floorplan.pending_wire_from = None;
                }
                self.floorplan.select(sel, &self.stats);
            }
            MapMsg::PlaceArmedAt { x, y } => {
                if let Some(lid) = self.floorplan.armed_place_lamp.take() {
                    let floor_id = self.floorplan.doc.active_floor_id.clone();
                    self.floorplan.doc.lamps.insert(
                        lid.clone(),
                        FloorplanLamp { x, y, z: 0.0, icon: "bulb".into(), floor_id },
                    );
                    self.floorplan.select(Some(Selection::Lamp(lid)), &self.stats);
                    self.save_floorplan(true);
                }
            }
            MapMsg::MoveLamp { id, x, y, commit } => {
                if let Some(pos) = self.floorplan.doc.lamps.get_mut(&id) {
                    pos.x = x;
                    pos.y = y;
                }
                self.floorplan.local_dirty = !commit;
                if commit {
                    self.save_floorplan(true);
                }
            }
            MapMsg::MoveBox { id, x, y, commit } => {
                if let Some(orig) = self.floorplan.doc.boxes.iter().find(|b| b.id == id).cloned() {
                    let dx = x - orig.x;
                    let dy = y - orig.y;
                    if let Some(b) = self.floorplan.doc.boxes.iter_mut().find(|b| b.id == id) {
                        b.translate(dx, dy);
                    }
                    // Carry enclosed lights, sensors & planned lights on the same floor when dragging a room
                    if orig.kind == "room" && (dx.abs() > 0.01 || dy.abs() > 0.01) {
                        let max_w = self.floorplan.doc.canvas.width;
                        let max_h = self.floorplan.doc.canvas.height;
                        for l in self.floorplan.doc.lamps.values_mut() {
                            if l.floor_id == orig.floor_id && orig.contains_point(l.x, l.y) {
                                l.x = (l.x + dx).clamp(0.0, max_w);
                                l.y = (l.y + dy).clamp(0.0, max_h);
                            }
                        }
                        for s in self.floorplan.doc.sensors.values_mut() {
                            if s.floor_id == orig.floor_id && orig.contains_point(s.x, s.y) {
                                s.x = (s.x + dx).clamp(0.0, max_w);
                                s.y = (s.y + dy).clamp(0.0, max_h);
                            }
                        }
                        for pl in &mut self.floorplan.doc.planned_lights {
                            let (px, py) = pl.anchor();
                            if pl.floor_id == orig.floor_id && orig.contains_point(px, py) {
                                pl.translate(dx, dy);
                                pl.x = pl.x.clamp(0.0, max_w);
                                pl.y = pl.y.clamp(0.0, max_h);
                                for pt in &mut pl.strip_points {
                                    pt.x = pt.x.clamp(0.0, max_w);
                                    pt.y = pt.y.clamp(0.0, max_h);
                                }
                            }
                        }
                    }
                }
                self.floorplan.local_dirty = !commit;
                if commit {
                    self.save_floorplan(true);
                }
            }
            MapMsg::ResizeBox { id, x, y, w, h, commit } => {
                if let Some(b) = self.floorplan.doc.boxes.iter_mut().find(|b| b.id == id) {
                    b.scale_to(x, y, w, h);
                }
                self.floorplan.local_dirty = !commit;
                if commit {
                    self.save_floorplan(true);
                }
            }
            MapMsg::MoveStamp { id, x, y, commit } => {
                if let Some(st) = self.floorplan.doc.stamps.iter_mut().find(|s| s.id == id) {
                    st.x = x;
                    st.y = y;
                }
                self.floorplan.local_dirty = !commit;
                if commit {
                    self.save_floorplan(false);
                }
            }
            MapMsg::CreateWire { from_kind, from_id, to_id } => {
                self.floorplan.pending_wire_from = None;
                if !(from_kind == "light" && from_id == to_id) {
                    self.floorplan
                        .open_new_rule_dialog(from_kind, from_id, to_id, &self.stats);
                }
            }
            // Lamp Inspector
            MapMsg::LampToggle(id, on) => {
                if self.floorplan.open_lamp_menu.as_deref() == Some(id.as_str()) {
                    self.floorplan.open_lamp_menu = None;
                }
                self.send(json!({"cmd": "light", "id": id, "on": on}));
            }
            MapMsg::LampBri(id, val) => {
                let target = format!("light:{id}");
                let value = val.max(1);
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
            MapMsg::LampBriDone(id) => {
                let target = format!("light:{id}");
                if let Some((value, _)) = self.dragging.remove(&target) {
                    self.send_bri(&target, value);
                }
            }
            MapMsg::LampRenameInput(v) => {
                self.floorplan.lamp_rename_draft = v.chars().take(32).collect();
            }
            MapMsg::LampRenameSave(id) => {
                let name = self.floorplan.lamp_rename_draft.trim().to_string();
                if !name.is_empty() {
                    self.send(cmd_light_rename(&id, &name));
                    self.floorplan.close_dialog();
                }
            }
            MapMsg::LampSetIcon(id, icon_key) => {
                let norm = normalize_lamp_icon(&icon_key).to_string();
                if let Some(pos) = self.floorplan.doc.lamps.get_mut(&id) {
                    pos.icon = norm;
                    self.save_floorplan(false);
                } else {
                    let lights = extract_bridge_lights(&self.stats);
                    let room_id = lights
                        .iter()
                        .find(|l| l.id == id)
                        .map(|l| l.room_id.as_str())
                        .unwrap_or("");
                    let (x, y, floor_id) = self
                        .floorplan
                        .doc
                        .boxes
                        .iter()
                        .find(|b| !room_id.is_empty() && b.hue_id.as_deref() == Some(room_id))
                        .map(|b| (b.x + b.w * 0.5, b.y + b.h * 0.5, b.floor_id.clone()))
                        .unwrap_or((
                            self.floorplan.doc.canvas.width * 0.5,
                            self.floorplan.doc.canvas.height * 0.5,
                            self.floorplan.doc.active_floor_id.clone(),
                        ));
                    self.floorplan.doc.lamps.insert(
                        id,
                        FloorplanLamp { x, y, z: 0.0, icon: norm, floor_id },
                    );
                    self.save_floorplan(false);
                }
            }
            MapMsg::LampStartWire(id) => {
                self.floorplan.open_lamp_menu = None;
                self.floorplan.open_sensor_menu = None;
                self.floorplan.close_dialog();
                self.floorplan.mode = CanvasMode::Wire;
                self.floorplan.pending_wire_from = Some(("light".into(), id));
            }
            // Room / Zone Inspector
            MapMsg::BoxRenameInput(v) => {
                self.floorplan.box_rename_draft = v.chars().take(40).collect();
            }
            MapMsg::BoxRenameSave(bid) => {
                let name = self.floorplan.box_rename_draft.trim().to_string();
                if !name.is_empty()
                    && let Some(b) = self.floorplan.doc.boxes.iter_mut().find(|b| b.id == bid)
                {
                    b.name = name.clone();
                    let linked = b.hue_id.clone();
                    let bkind = b.kind.clone();
                    self.save_floorplan(false);
                    if let Some(hid) = linked {
                        self.send(cmd_room_update(&hid, &bkind, Some(&name), None, None));
                    }
                }
            }
            MapMsg::BoxSetKind(bid, kind) => {
                if let Some(b) = self.floorplan.doc.boxes.iter_mut().find(|b| b.id == bid) {
                    let norm = if kind == "zone" { "zone" } else { "room" };
                    if b.kind != norm {
                        b.kind = norm.to_string();
                        // Unlink if switching Room <-> Zone so Bridge resource types stay consistent
                        b.hue_id = None;
                        self.save_floorplan(true);
                    }
                }
            }
            MapMsg::ToggleArchetypeMenu => {
                self.floorplan.archetype_menu_open = !self.floorplan.archetype_menu_open;
            }
            MapMsg::BoxSetArchetype(bid, arch) => {
                self.floorplan.archetype_menu_open = false;
                if let Some(b) = self.floorplan.doc.boxes.iter_mut().find(|b| b.id == bid) {
                    b.archetype = arch.clone();
                    let linked = b.hue_id.clone();
                    let bkind = b.kind.clone();
                    self.save_floorplan(false);
                    if let Some(hid) = linked {
                        self.send(cmd_room_update(&hid, &bkind, None, Some(&arch), None));
                    }
                }
            }
            MapMsg::ToggleBoxEntMenu => {
                self.floorplan.box_ent_menu_open = !self.floorplan.box_ent_menu_open;
            }
            MapMsg::BoxSetEntertainment(bid, area_id) => {
                self.floorplan.box_ent_menu_open = false;
                if let Some(b) = self.floorplan.doc.boxes.iter_mut().find(|b| b.id == bid) {
                    b.entertainment_area_id = area_id;
                    self.save_floorplan(true);
                }
            }
            MapMsg::BoxSyncEntertainment(area_id, box_id) => {
                self.save_floorplan(false);
                self.send(cmd_entertainment_sync(&area_id, &box_id));
            }
            MapMsg::BoxSyncRoom => {
                self.save_floorplan(false);
                self.send(cmd_floorplan_sync_rooms());
            }
            MapMsg::BoxRemoveFromCanvas(bid) => {
                self.floorplan.doc.boxes.retain(|b| b.id != bid);
                self.floorplan.selection = None;
                self.floorplan.confirm_delete_bridge_box = None;
                self.save_floorplan(false);
            }
            MapMsg::BoxConfirmDeleteOnBridge(bid) => {
                self.floorplan.confirm_delete_bridge_box = Some(bid);
            }
            MapMsg::BoxCancelDeleteOnBridge => {
                self.floorplan.confirm_delete_bridge_box = None;
            }
            MapMsg::BoxDeleteOnBridge(bid) => {
                self.floorplan.confirm_delete_bridge_box = None;
                if let Some(pos) = self.floorplan.doc.boxes.iter().position(|b| b.id == bid) {
                    let b = self.floorplan.doc.boxes.remove(pos);
                    self.floorplan.selection = None;
                    self.save_floorplan(false);
                    if let Some(hid) = b.hue_id {
                        self.send(cmd_room_delete(&hid, &b.kind));
                    }
                }
            }
            MapMsg::RecallScene(sid) => {
                self.send(json!({"cmd": "scene", "id": sid}));
            }
            MapMsg::NewSceneNameInput(v) => {
                self.floorplan.new_scene_name = v.chars().take(32).collect();
            }
            MapMsg::SaveCurrentScene(room_id) => {
                let name = self.floorplan.new_scene_name.trim().to_string();
                if !name.is_empty() {
                    self.floorplan.new_scene_name.clear();
                    self.send(cmd_scene_create(&room_id, &name));
                }
            }
            MapMsg::ScheduleScene { kind, room_id, room_name, scene_id, scene_name } => {
                self.tab = Tab::Schedules;
                self.open_light_drawer = None;
                self.schedule_draft = Some(ScheduleDraft::for_scene(
                    kind, room_id, room_name, scene_id, scene_name,
                ));
            }
            // Stamp Inspector
            MapMsg::StampRotate(sid) => {
                if let Some(st) = self.floorplan.doc.stamps.iter_mut().find(|s| s.id == sid) {
                    st.rot = (st.rot + 90) % 360;
                    self.save_floorplan(false);
                }
            }
            MapMsg::StampDelete(sid) => {
                self.floorplan.doc.stamps.retain(|s| s.id != sid);
                self.floorplan.selection = None;
                self.save_floorplan(false);
            }
            // Map Rule Dialog
            MapMsg::OpenRuleDialog(rid) => {
                self.floorplan.open_existing_rule_dialog(&rid, &self.stats);
            }
            MapMsg::CloseRuleDialog => {
                self.floorplan.close_dialog();
            }
            MapMsg::RuleDialogName(v) => {
                if let Some(d) = &mut self.floorplan.rule_dialog {
                    d.name = v.chars().take(60).collect();
                    d.error = None;
                }
            }
            MapMsg::RuleDialogEnabled(v) => {
                if let Some(d) = &mut self.floorplan.rule_dialog {
                    d.enabled = v;
                }
            }
            MapMsg::RuleDialogKind(kind) => {
                if let Some(d) = &mut self.floorplan.rule_dialog {
                    if kind == "companion" {
                        d.only_if_off = true;
                    }
                    d.kind = kind;
                    d.error = None;
                }
            }
            MapMsg::RuleDialogTriggerState(st) => {
                if let Some(d) = &mut self.floorplan.rule_dialog {
                    d.trigger_state = st;
                }
            }
            MapMsg::RuleDialogMirrorOff(v) => {
                if let Some(d) = &mut self.floorplan.rule_dialog {
                    d.mirror_off = v;
                }
            }
            MapMsg::RuleDialogMatchBri(v) => {
                if let Some(d) = &mut self.floorplan.rule_dialog {
                    d.match_bri = v;
                }
            }
            MapMsg::RuleDialogOnlyIfOff(v) => {
                if let Some(d) = &mut self.floorplan.rule_dialog {
                    d.only_if_off = v;
                }
            }
            MapMsg::RuleDialogTimeStart(v) => {
                if let Some(d) = &mut self.floorplan.rule_dialog {
                    d.time_start = v.chars().take(5).collect();
                    d.error = None;
                }
            }
            MapMsg::RuleDialogTimeEnd(v) => {
                if let Some(d) = &mut self.floorplan.rule_dialog {
                    d.time_end = v.chars().take(5).collect();
                    d.error = None;
                }
            }
            MapMsg::RuleDialogSave => {
                let res = self.floorplan.rule_dialog.as_ref().map(|d| d.to_save_command());
                if let Some(res) = res {
                    match res {
                        Ok(cmd) => {
                            self.send(cmd);
                            self.floorplan.close_dialog();
                        }
                        Err(err) => {
                            if let Some(d) = &mut self.floorplan.rule_dialog {
                                d.error = Some(err);
                            }
                        }
                    }
                }
            }
            MapMsg::RuleDialogDelete(rid) => {
                self.floorplan.close_dialog();
                self.send(json!({"cmd": "rule_delete", "id": rid}));
            }
        }
        Task::none()
    }

    fn any_picker_open(&self) -> bool {
        self.synth_room_picker_open
            || self.open_lights_menu.is_some()
            || self.floorplan.any_menu_open()
            || self.schedule_draft.as_ref().is_some_and(|d| d.target_picker_open)
            || self.rule_draft.as_ref().is_some_and(|d| d.trigger_picker_open || d.target_picker_open)
    }

    fn close_pickers(&mut self) {
        self.synth_room_picker_open = false;
        self.open_lights_menu = None;
        self.floorplan.close_menus();
        if let Some(d) = &mut self.schedule_draft {
            d.target_picker_open = false;
        }
        if let Some(d) = &mut self.rule_draft {
            d.trigger_picker_open = false;
            d.target_picker_open = false;
        }
    }

    fn close_settings_menus(&mut self) {
        self.language_open = false;
        self.corners_open = false;
        self.skin_open = false;
        self.panel_icon_open = false;
        self.font_scale_open = false;
        self.schedule_mode_open = false;
    }

    fn any_settings_menu_open(&self) -> bool {
        self.language_open
            || self.corners_open
            || self.skin_open
            || self.panel_icon_open
            || self.font_scale_open
            || self.schedule_mode_open
    }

    fn any_drawer_open(&self) -> bool {
        self.open_light_drawer.is_some()
            || self.theme_draft.is_some()
            || self.schedule_draft.is_some()
            || self.rule_draft.is_some()
            || self.floorplan.has_dialog_open()
    }

    fn rebuild_cached_theme(&mut self) {
        self.cached_theme = active_theme(&self.skin, &self.corners);
    }

    fn apply_window_theme(&self) -> Task<Message> {
        if !self.window_mode {
            return Task::none();
        }
        let scale = look::text_scale() * font_scale_factor(&self.font_scale);
        let theme = self.cached_theme.clone().unwrap_or_else(cosmic::theme::system_preference);
        Task::batch([
            cosmic::command::set_theme(theme),
            cosmic::command::set_scaling_factor(scale),
        ])
    }

    fn themed_window_content(&self) -> Element<'_, Message> {
        match self.cached_theme.clone() {
            Some(theme) => cosmic::iced::widget::themer(Some(theme), self.popup_content())
                .background(look::window_background)
                .text_color(|t: &cosmic::Theme| t.cosmic().on_bg_color().into())
                .into(),
            None => self.popup_content(),
        }
    }

    fn panel_content(&self) -> Element<'_, Message> {
        let lit = self.on_count() > 0.0;
        let size = self.core.applet.suggested_size(true).0;
        let mut glyph = icon::icon(panel_icon_handle(&self.panel_icon, lit)).size(size);
        if lit {
            glyph = glyph.class(lit_svg_for(&self.skin));
        } else {
            glyph = glyph.opacity(0.6);
        }
        if self.show_count && self.phase() == "ok" {
            let count = (self.on_count().round().max(0.0) as u64).to_string();
            let label: Element<Message> = if lit { text::caption(count).into() } else { dim(count) };
            let horizontal = matches!(self.core.applet.anchor, PanelAnchor::Top | PanelAnchor::Bottom);
            if horizontal {
                row![glyph, label].spacing(4).align_y(Alignment::Center).into()
            } else {
                column![glyph, label].spacing(2).align_x(Alignment::Center).into()
            }
        } else {
            glyph.into()
        }
    }

    fn header(&self) -> Element<'_, Message> {
        let lit = self.on_count() > 0.0;
        let mut hero = icon::icon(panel_icon_handle(&self.panel_icon, lit)).size(32);
        hero = if lit { hero.class(lit_svg_for(&self.skin)) } else { hero.opacity(0.6) };
        let meta = if self.paired() { bridge_text(&self.stats) } else { tr("No bridge paired") };
        let mut header = row![hero, column![text::title4(tr("Maprisma Lights")), dim(meta)].spacing(4), space::horizontal()]
            .spacing(12)
            .align_y(Alignment::Center);
        if self.phase() == "ok" {
            let pill = trf("{0} of {1} on", &[self.on_count().to_string(), num(&self.stats, "light_count").to_string()]);
            header = header.push(container(text::caption(pill)).padding([4, 8]).class(pill_style(&self.skin)));
        }
        // Optional Ask AI button (only rendered when explicitly enabled in Settings).
        if self.ask_ai_enabled {
            let label = if self.window_mode && self.window_width >= 520.0 {
                format!("✨ {}", tr("Ask AI"))
            } else {
                "✨".to_string()
            };
            header = header.push(cosmic::widget::tooltip(
                button::text(label).on_press(Message::ToggleAskAi),
                text::body(tr("Ask AI")),
                cosmic::widget::tooltip::Position::Bottom,
            ));
        }
        // Help and Settings at the header's right end, as in Maprisma Launcher.
        header = header.push(cosmic::widget::tooltip(
            button::icon(icon::from_name("help-about-symbolic").symbolic(true)).on_press(Message::ToggleHelp),
            text::body(tr("Help")),
            cosmic::widget::tooltip::Position::Bottom,
        ));
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
        let map_pending = self.pending_sync_count;
        let paused = flag(&self.stats, "rules_paused");
        let synth_running = self.stats.get("synth").is_some_and(|sy| flag(sy, "running"));

        let tab_btn = |tab: Tab, label: String, count: usize, dimmed: bool, live: bool| -> Element<'_, Message> {
            let active = self.tab == tab;
            let text_el: Element<Message> = if dimmed && !active {
                dim(label)
            } else {
                text::caption(label).into()
            };
            let mut inner = row![text_el].spacing(4).align_y(Alignment::Center);
            if count > 0 {
                inner = inner.push(container(text::caption(count.to_string())).padding([1, 5]).class(pill_style(&self.skin)));
            }
            if live {
                inner = inner.push(swatch_sized(Some(Color::from_rgb(0.95, 0.25, 0.45)), true, 7.0));
            }
            if dimmed {
                inner = inner.push(dim("⏸"));
            }
            button::custom(container(inner).width(Length::Fill).align_x(Alignment::Center))
                .width(Length::FillPortion(1))
                .padding([6, 6])
                .class(if active { theme::Button::Suggested } else { theme::Button::MenuItem })
                .on_press(Message::SelectTab(tab))
                .into()
        };

        container(
            row![
                tab_btn(Tab::Lights, tr("Lights"), 0, false, false),
                tab_btn(Tab::Map, tr("Map"), map_pending, false, false),
                tab_btn(Tab::Schedules, tr("Schedules"), sch_count, false, false),
                tab_btn(Tab::Rules, tr("Rules"), rule_count, paused, false),
                tab_btn(Tab::Synth, tr("Synth"), 0, false, synth_running),
            ]
            .spacing(4),
        )
        .padding(4)
        .width(Length::Fill)
        .class(segmented_bar_style(&self.skin))
        .into()
    }

    fn pairing(&self) -> Element<'_, Message> {
        let stats = &self.stats;
        let pairing = self.phase() == "pairing";
        let mut content = column![text::heading(tr("Connect Your Lights"))].spacing(8);
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
        content = content.push(divider::horizontal::default());
        content = content.push(self.extra_devices_section(true));
        content.into()
    }

    fn extra_devices_section(&self, compact_onboarding: bool) -> Element<'_, Message> {
        let mut sec = column![].spacing(8);
        let providers = list(&self.stats, "providers");
        if compact_onboarding {
            sec = sec.push(text::heading(tr("Other smart lights & hubs")));
            sec = sec.push(dim(tr("Connect Home Assistant, WLED, WiZ, LIFX, Govee LAN, Nanoleaf, Shelly, or IKEA DIRIGERA directly over your local network — with or without a Hue Bridge.")));
            let mut row1 = row![].spacing(6);
            let mut row2 = row![].spacing(6);
            for (idx, (kind, _, badge, _, _, _)) in EXTRA_PROVIDERS.iter().enumerate() {
                let b = button::standard(*badge).on_press(Message::OpenAddProvider(Some((*kind).to_string())));
                if idx < 4 {
                    row1 = row1.push(b);
                } else {
                    row2 = row2.push(b);
                }
            }
            sec = sec.push(row1);
            sec = sec.push(row2);
        } else {
            sec = sec.push(
                row![
                    column![
                        text::body(tr("Other smart lights & hubs")),
                        dim(tr("Mix Philips Hue with Home Assistant, WLED, WiZ, LIFX, Govee LAN, Nanoleaf, Shelly, or IKEA DIRIGERA.")),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    button::suggested(tr("Add Device…")).on_press(Message::OpenAddProvider(None)),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
            if providers.is_empty() {
                sec = sec.push(dim(tr("No extra devices added yet.")));
            } else {
                for p in providers {
                    let pid = s(p, "id").to_string();
                    let pkind = s(p, "kind");
                    let pname = s(p, "name").to_string();
                    let phost = s(p, "host");
                    let pport = num(p, "port") as u16;
                    let penabled = p.get("enabled").and_then(Value::as_bool) != Some(false);
                    let pconn = flag(p, "connected");
                    let lcount = num(p, "light_count") as u64;
                    let perr = s(p, "error");
                    let dot = if !penabled { "○" } else if pconn { "●" } else { "◐" };
                    let mut title_row = row![text::body(format!("{dot} {pname}"))].spacing(6).align_y(Alignment::Center);
                    if let Some(pill) = provider_pill(pkind) {
                        title_row = title_row.push(pill);
                    }
                    let subtitle = if lcount > 0 {
                        format!("{phost}:{pport} · {lcount}")
                    } else {
                        format!("{phost}:{pport}")
                    };
                    let mut info_col = column![title_row, dim(subtitle)].spacing(2).width(Length::Fill);
                    if penabled && !perr.is_empty() {
                        info_col = info_col.push(error_text(tr(perr)));
                    }
                    let pid_tog = pid.clone();
                    let pid_edit = pid.clone();
                    let pid_del = pid.clone();
                    let confirming_del = self.confirm_remove_provider.as_deref() == Some(pid.as_str());
                    let mut action_row = row![
                        info_col,
                        toggler(penabled).on_toggle(move |v| Message::ToggleProvider(pid_tog.clone(), v)),
                        button::standard(tr("Edit")).on_press(Message::EditProvider(pid_edit)),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center);
                    if confirming_del {
                        action_row = action_row
                            .push(button::destructive(tr("Confirm")).on_press(Message::RemoveProvider(pid_del)))
                            .push(button::text(tr("Cancel")).on_press(Message::CancelRemoveProvider));
                    } else {
                        action_row = action_row
                            .push(button::destructive(tr("Remove")).on_press(Message::RemoveProvider(pid_del)));
                    }
                    sec = sec.push(
                        container(action_row)
                            .padding(8)
                            .class(card(None, &self.skin)),
                    );
                }
            }
        }
        if let Some(draft) = &self.provider_draft {
            sec = sec.push(self.provider_editor(draft));
        }
        sec.into()
    }

    fn provider_editor<'a>(&'a self, draft: &'a ProviderDraft) -> Element<'a, Message> {
        let (_, disp_name, _, default_port, req_tok, supports_pair) =
            provider_meta(&draft.kind).unwrap_or(EXTRA_PROVIDERS[0]);
        let mut form = column![
            row![
                text::heading(if draft.id.is_empty() { tr("Add Lighting Device") } else { tr("Edit Lighting Device") }),
                space::horizontal(),
                button::text(tr("Cancel")).on_press(Message::CloseProviderDraft),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(8);

        if draft.id.is_empty() {
            let mut r1 = row![].spacing(4);
            let mut r2 = row![].spacing(4);
            for (idx, (k, _, badge, _, _, _)) in EXTRA_PROVIDERS.iter().enumerate() {
                let active = draft.kind == *k;
                let btn = if active {
                    button::suggested(*badge).on_press(Message::ProviderKind((*k).to_string()))
                } else {
                    button::standard(*badge).on_press(Message::ProviderKind((*k).to_string()))
                };
                if idx < 4 { r1 = r1.push(btn); } else { r2 = r2.push(btn); }
            }
            form = form.push(r1).push(r2);

            // E4: Per-brand setup help
            let setup_hint = match draft.kind.as_str() {
                "home_assistant" => tr("In Home Assistant: Profile → Security → Long-lived access tokens."),
                "nanoleaf" => tr("Hold the Nanoleaf power button for 5–7 seconds until LEDs flash, then click Pair via Power Button."),
                "govee" => tr("Enable LAN Control in the Govee Home app first, then enter the light's local IP address."),
                "dirigera" => tr("Press the Action Button underneath your IKEA DIRIGERA hub when prompted to pair securely."),
                "wiz" => tr("Direct local UDP control on port 38899 — no pairing or cloud required."),
                "lifx" => tr("Direct local LAN protocol on port 56700 — no hub required."),
                "wled" => tr("Direct local HTTP control for WLED strips — no hub required."),
                "shelly" => tr("Direct local HTTP control for Shelly bulbs, dimmers & light switches."),
                _ => String::new(),
            };
            if !setup_hint.is_empty() {
                form = form.push(dim(setup_hint));
            }

            let disc_busy = flag(&self.stats, "discovering_providers");
            let discovered = list(&self.stats, "discovered_providers");
            let existing_targets: HashSet<String> = list(&self.stats, "providers")
                .iter()
                .map(|p| format!("{}|{}|{}", s(p, "kind"), s(p, "host").to_lowercase(), num(p, "port") as u16))
                .collect();
            let addable_free = discovered
                .iter()
                .filter(|dev| {
                    let dk = s(dev, "kind");
                    let req_t = provider_meta(dk).is_some_and(|m| m.4);
                    let key = format!("{dk}|{}|{}", s(dev, "host").to_lowercase(), num(dev, "port") as u16);
                    !req_t && !existing_targets.contains(&key)
                })
                .count();
            let mut disc_hdr = row![
                dim(tr("Discovered on local network")),
                space::horizontal(),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            if addable_free >= 2 {
                disc_hdr = disc_hdr.push(
                    button::suggested(tr("Add all found")).on_press(Message::AddAllDiscoveredProviders),
                );
            }
            disc_hdr = disc_hdr.push(
                button::standard(if disc_busy { tr("Scanning…") } else { tr("Scan LAN") })
                    .on_press_maybe((!disc_busy).then_some(Message::DiscoverProviders)),
            );
            form = form.push(disc_hdr);
            if discovered.is_empty() && !disc_busy {
                form = form.push(dim(tr("Nothing found — find the IP in your router or the brand's app.")));
            }
            for dev in discovered {
                let dk = s(dev, "kind").to_string();
                let dn = s(dev, "name").to_string();
                let dh = s(dev, "host").to_string();
                let dp = num(dev, "port") as u16;
                let badge = s(dev, "badge").to_string();
                let key = format!("{dk}|{}|{dp}", dh.to_lowercase());
                let already = existing_targets.contains(&key);
                let act_btn = if already {
                    button::standard(tr("Added"))
                } else {
                    button::standard(tr("Use")).on_press(Message::ProviderUseDiscovered(dk, dn.clone(), dh.clone(), dp))
                };
                form = form.push(
                    row![
                        text::caption(format!("[{badge}] {dn} · {dh}:{dp}")),
                        space::horizontal(),
                        act_btn,
                    ]
                    .align_y(Alignment::Center),
                );
            }
        }

        form = form.push(
            text_input(disp_name, &draft.name)
                .on_input(Message::ProviderName)
                .width(Length::Fill),
        );
        form = form.push(
            row![
                text_input(tr("Local IP or .local host"), &draft.host)
                    .on_input(Message::ProviderHost)
                    .width(Length::Fill),
                text_input(default_port.to_string(), &draft.port)
                    .on_input(Message::ProviderPort)
                    .width(Length::Fixed(80.0)),
            ]
            .spacing(8),
        );
        if draft.kind != "home_assistant" && draft.kind != "dirigera" {
            form = form.push(
                text_input(tr("Room name (optional)"), &draft.room_name)
                    .on_input(Message::ProviderRoomName)
                    .width(Length::Fill),
            );
        }
        if draft.kind == "home_assistant" {
            form = form.push(
                row![
                    text::caption(tr("Use HTTPS (self-signed LAN cert allowed)")),
                    space::horizontal(),
                    toggler(draft.use_https).on_toggle(Message::ProviderHttps),
                ]
                .align_y(Alignment::Center),
            );
        }
        let same_saved_target = draft.has_saved_token
            && draft.kind == draft.original_kind
            && draft.host.trim().eq_ignore_ascii_case(draft.original_host.trim())
            && draft.port.trim() == draft.original_port.trim();
        if req_tok {
            let placeholder = if !draft.pair_id.is_empty() {
                tr("Paired — press Save")
            } else if same_saved_target {
                tr("Saved (leave blank to keep)")
            } else if draft.kind == "home_assistant" {
                tr("Paste long-lived access token")
            } else {
                tr("Access Token")
            };
            form = form.push(
                text_input(placeholder, &draft.token)
                    .on_input(Message::ProviderToken)
                    .password()
                    .width(Length::Fill),
            );
            if !draft.id.is_empty() && !same_saved_target && draft.token.trim().is_empty() && draft.pair_id.is_empty() {
                form = form.push(dim(tr("Host or port changed — enter a token or re-pair before saving.")));
            }
        }
        // B4: Allow trusting a renewed TLS certificate while keeping the connection ID intact
        let has_cert_err = !draft.id.is_empty()
            && list(&self.stats, "providers")
                .iter()
                .find(|p| s(p, "id") == draft.id)
                .is_some_and(|p| s(p, "error").to_lowercase().contains("certificate"));
        if !draft.id.is_empty() && (draft.kind == "dirigera" || draft.use_https) && (draft.has_cert_fingerprint || has_cert_err) {
            form = form.push(
                button::standard(tr("Trust new certificate"))
                    .on_press(Message::TrustProviderCert(draft.id.clone())),
            );
        }
        if supports_pair {
            let pair_lbl = if draft.kind == "nanoleaf" {
                tr("Pair via Power Button")
            } else {
                tr("Pair via Action Button")
            };
            form = form.push(
                button::standard(pair_lbl)
                    .on_press_maybe((!draft.host.trim().is_empty()).then_some(Message::PairProvider)),
            );
        }

        if let Some(act) = self.stats.get("provider_action").and_then(Value::as_object) {
            let running = act.get("running").and_then(Value::as_bool) == Some(true);
            let ok = act.get("ok").and_then(Value::as_bool) == Some(true);
            let msg = act.get("message").and_then(Value::as_str).unwrap_or("");
            if running {
                form = form.push(dim(tr("Working…")));
            } else if !msg.is_empty() {
                let localized = tr(msg);
                if ok {
                    form = form.push(dim(format!("✓ {localized}")));
                } else {
                    form = form.push(error_text(localized));
                }
            }
        }

        let host_ok = !draft.host.trim().is_empty();
        let tok_ok = !req_tok || !draft.token.trim().is_empty() || !draft.pair_id.is_empty() || same_saved_target;
        form = form.push(
            row![
                button::standard(tr("Test Connection"))
                    .on_press_maybe((host_ok && tok_ok).then_some(Message::TestProvider)),
                space::horizontal(),
                button::suggested(tr("Save Device"))
                    .on_press_maybe((host_ok && tok_ok).then_some(Message::SaveProvider)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );

        container(form).padding(10).class(card(None, &self.skin)).into()
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

    fn motion_aware_hint_card(&self) -> Option<Element<'_, Message>> {
        if !should_show_motion_aware_hint(&self.stats, self.motion_hint_dismissed) {
            return None;
        }
        const EMERALD: Color = Color::from_rgb(16.0 / 255.0, 185.0 / 255.0, 129.0 / 255.0);
        let eligible = motion_aware_eligible_rooms(&self.stats);
        let dot = container(space::horizontal())
            .width(Length::Fixed(10.0))
            .height(Length::Fixed(10.0))
            .class(theme::Container::custom(|_| iced_container::Style {
                background: Some(Background::Color(EMERALD)),
                border: Border { radius: 5.0.into(), ..Default::default() },
                ..Default::default()
            }));
        let header = row![
            dot,
            text::heading(tr("Bridge Pro MotionAware™ Ready")),
            space::horizontal(),
            button::custom(text::caption("×"))
                .padding([2, 7])
                .class(theme::Button::MenuItem)
                .on_press(Message::DismissMotionHint),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let mut body = column![
            header,
            dim(tr("1. Open the Philips Hue mobile app → Settings → MotionAware")),
            dim(tr("2. Pick 3+ lights in a room and run the 30-second calibration once")),
            dim(tr("3. Return here — live radar badges, sensitivity & visual rules appear automatically!")),
        ]
        .spacing(4);

        if !eligible.is_empty() {
            body = body.push(
                text::caption(trf("Ready rooms (3+ lights): {0}", &[eligible.join(", ")])).class(
                    theme::Text::Custom(|_| iced_text::Style {
                        color: Some(EMERALD),
                        ..Default::default()
                    }),
                ),
            );
        }

        Some(
            container(body)
                .padding(10)
                .width(Length::Fill)
                .class(theme::Container::custom(|_| iced_container::Style {
                    background: Some(Background::Color(with_alpha(EMERALD, 0.10))),
                    border: Border {
                        radius: corners::now().radius.into(),
                        width: 1.0,
                        color: with_alpha(EMERALD, 0.45),
                    },
                    ..Default::default()
                }))
                .into(),
        )
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
        if let Some(hint) = self.motion_aware_hint_card() {
            content = content.push(hint);
        }
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
        let motion_live = parse_motion_areas(&self.stats)
            .iter()
            .any(|a| a.room_id == key && a.enabled && a.motion);
        let status_el: Element<Message> = if motion_live {
            const EMERALD: Color = Color::from_rgb(16.0 / 255.0, 185.0 / 255.0, 129.0 / 255.0);
            let live_pill = container(
                text::caption(format!("◉ {}", tr("Motion"))).class(theme::Text::Custom(|_| {
                    iced_text::Style { color: Some(EMERALD), ..Default::default() }
                })),
            )
            .padding([1, 6])
            .class(theme::Container::custom(|_| iced_container::Style {
                background: Some(Background::Color(with_alpha(EMERALD, 0.16))),
                border: Border {
                    radius: corners::now().pill.into(),
                    width: 1.0,
                    color: with_alpha(EMERALD, 0.45),
                },
                ..Default::default()
            }));
            row![dim(room_status(room, excluded)), live_pill]
                .spacing(6)
                .align_y(Alignment::Center)
                .into()
        } else {
            dim(room_status(room, excluded))
        };
        let mut title_row = row![text::heading(rname.clone())].spacing(6).align_y(Alignment::Center);
        if let Some(pill) = provider_pill(s(room, "provider")) {
            title_row = title_row.push(pill);
        }
        let rlights = list(room, "lights");
        let all_offline = !rlights.is_empty()
            && rlights.iter().all(|l| l.get("reachable").and_then(Value::as_bool) == Some(false));
        if all_offline {
            const ORANGE: Color = Color::from_rgb(249.0 / 255.0, 115.0 / 255.0, 22.0 / 255.0);
            let offline_pill = container(
                text::caption(tr("Offline")).class(theme::Text::Color(ORANGE)),
            )
            .padding([1, 6])
            .class(theme::Container::custom(|_| iced_container::Style {
                background: Some(Background::Color(with_alpha(ORANGE, 0.14))),
                border: Border {
                    radius: corners::now().pill.into(),
                    width: 1.0,
                    color: with_alpha(ORANGE, 0.35),
                },
                ..Default::default()
            }));
            title_row = title_row.push(offline_pill);
        }
        let names = column![title_row, status_el].spacing(4);
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
                if self.open_lights_menu.as_deref() == Some(s(light, "id")) {
                    card_content = card_content.push(self.light_submenu(light));
                }
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
        container(card_content).padding(12).width(Length::Fill).class(card(tint, &self.skin)).into()
    }

    fn light_row<'a>(&'a self, light: &'a Value) -> Element<'a, Message> {
        let id = s(light, "id").to_string();
        let on = flag(light, "on");
        let reachable = light.get("reachable").and_then(Value::as_bool) != Some(false);
        let c = color(light.get("color").unwrap_or(&Value::Null));
        let drawer_open = self.open_light_drawer.as_deref() == Some(id.as_str());
        let menu_open = self.open_lights_menu.as_deref() == Some(id.as_str());
        let icon_key = self
            .floorplan
            .doc
            .lamps
            .get(&id)
            .map(|p| normalize_lamp_icon(&p.icon))
            .unwrap_or("bulb");
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
                badges = badges.push(mini_badge(tr("Schedule"), &self.skin));
            }
            if has_rule {
                badges = badges.push(mini_badge(tr("Rule"), &self.skin));
            }
            column![name_el, badges].spacing(2).into()
        } else {
            name_el
        };

        let dot_btn = cosmic::widget::tooltip(
            button::custom(lamp_icon_badge(icon_key, on.then_some(c), reachable, 24.0))
                .padding(2)
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
            let slider_w = if self.window_mode { 104.0 } else { 86.0 };
            line = line.push(
                slider(0..=100, bri, move |v| Message::Bri(t1.clone(), v))
                    .step(5u8)
                    .on_release(Message::BriDone(target))
                    .width(Length::Fixed(slider_w)),
            );
        }
        let switch_id = id.clone();
        line = line.push(toggler(on).on_toggle(move |v| Message::LightOn(switch_id.clone(), v)));
        line = line.push(
            button::custom(text::caption("⋯"))
                .padding([3, 7])
                .class(if menu_open { theme::Button::Suggested } else { theme::Button::MenuItem })
                .on_press(Message::ToggleLightsMenu(id)),
        );
        container(line).padding([4, 4, 4, 8]).into()
    }

    fn light_submenu<'a>(&'a self, light: &'a Value) -> Element<'a, Message> {
        let id = s(light, "id").to_string();
        let pinned = is_favorite(&self.favorites, "light", &id);
        let pin_label = if pinned { tr("Unpin favorite") } else { tr("Pin favorite") };
        let items = column![
            menu_button(text::caption(format!("{}…", tr("Brightness"))))
                .on_press(Message::LightsMenuBrightness(id.clone())),
            menu_button(text::caption(tr("Color & Warmth")))
                .on_press(Message::LightsMenuColor(id.clone())),
            menu_button(text::caption(format!("{}…", tr("Rename"))))
                .on_press(Message::LightsMenuRename(id.clone())),
            menu_button(text::caption(format!("{}…", tr("Lamp icon"))))
                .on_press(Message::LightsMenuIcon(id.clone())),
            menu_button(text::caption(tr("Wire rule on Map")))
                .on_press(Message::LightsMenuWireOnMap(id.clone())),
            menu_button(text::caption(pin_label))
                .on_press(Message::ToggleFavorite("light".into(), id)),
        ]
        .spacing(2);
        container(
            container(items)
                .padding(4)
                .width(Length::Fill)
                .class(sub_card(&self.skin)),
        )
        .padding([2, 4, 4, 24])
        .width(Length::Fill)
        .into()
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

        // Per-lamp Light themes
        let themes = list(&self.stats, "themes");
        if !themes.is_empty() || self.theme_draft.as_ref().is_some_and(|d| d.light_id == id) {
            let mut theme_chips: Vec<Element<Message>> = Vec::new();
            for th in themes {
                let tid = s(th, "id").to_string();
                let tname = s(th, "name").to_string();
                let tcol = color(th.get("hex").unwrap_or(&Value::Null));
                let built_in = flag(th, "built_in");
                let apply_btn = button::custom(
                    row![swatch_sized(Some(tcol), true, 10.0), text::caption(tname)]
                        .spacing(6)
                        .align_y(Alignment::Center),
                )
                .padding([4, 8])
                .class(theme::Button::Standard)
                .on_press(Message::ThemeApply(tid.clone(), id.clone()));
                if built_in {
                    theme_chips.push(apply_btn.into());
                } else {
                    let del_btn = button::custom(text::caption("×"))
                        .padding([4, 6])
                        .class(theme::Button::Destructive)
                        .on_press(Message::ThemeDelete(tid));
                    theme_chips.push(row![apply_btn, del_btn].spacing(2).align_y(Alignment::Center).into());
                }
            }
            let mut theme_sec = column![dim(tr("Light themes"))].spacing(4);
            if !theme_chips.is_empty() {
                theme_sec = theme_sec.push(flex_row(theme_chips).row_spacing(6).column_spacing(6));
            }
            if let Some(draft) = self.theme_draft.as_ref().filter(|d| d.light_id == id) {
                let mut form = column![
                    row![
                        text_input(tr("Theme name"), &draft.name)
                            .on_input(Message::ThemeDraftName)
                            .on_submit(|_| Message::ThemeDraftSave)
                            .width(Length::Fill),
                        button::suggested(tr("Save")).on_press(Message::ThemeDraftSave),
                        button::standard(tr("Cancel")).on_press(Message::ThemeDraftCancel),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                ]
                .spacing(4);
                if let Some(err) = &draft.error {
                    form = form.push(error_text(err.clone()));
                }
                theme_sec = theme_sec.push(form);
            } else {
                theme_sec = theme_sec.push(
                    button::standard(tr("Save current as theme")).on_press(Message::StartSaveTheme(id.clone())),
                );
            }
            drawer = drawer.push(theme_sec);
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

        container(container(drawer).padding(10).width(Length::Fill).class(sub_card(&self.skin)))
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
        match source {
            "bridge" => head_left = head_left.push(mini_badge(tr("Bridge"), &self.skin)),
            "local" => head_left = head_left.push(mini_badge(tr("Local"), &self.skin)),
            _ => {}
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
        let confirming = self.confirm_delete_schedule.as_deref() == Some(id.as_str());
        let mut foot = row![
            dim(action),
            space::horizontal(),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if confirming {
            foot = foot
                .push(button::standard(tr("Cancel")).on_press(Message::ScheduleConfirmDelete(None)))
                .push(button::destructive(tr("Delete")).on_press(Message::ScheduleDelete(id)));
        } else {
            foot = foot.push(
                button::destructive(tr("Delete")).on_press(Message::ScheduleConfirmDelete(Some(id))),
            );
        }

        container(column![head, dim(meta), foot].spacing(6))
            .padding(12)
            .width(Length::Fill)
            .class(card(None, &self.skin))
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
                    .class(sub_card(&self.skin)),
            );
        }

        // Trigger kind pills (local PC schedules support clock time only; Bridge supports solar too)
        let local_mode = self.schedule_mode == "local";
        let effective_trigger = if local_mode { "time" } else { draft.trigger_kind.as_str() };
        if !local_mode {
            let trigger_pills: Vec<Element<Message>> = [
                ("time", tr("Time")),
                ("sunrise", tr("Sunrise")),
                ("sunset", tr("Sunset")),
            ]
            .into_iter()
            .map(|(k, label)| {
                let b = if effective_trigger == k { button::suggested(label) } else { button::standard(label) };
                b.on_press(Message::ScheduleTriggerKind(k.into())).into()
            })
            .collect();
            form = form.push(column![dim(tr("Trigger")), flex_row(trigger_pills).row_spacing(6).column_spacing(6)].spacing(4));
        }

        // Time or solar offset
        if effective_trigger == "time" {
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

        // Action & brightness / scene
        let mut action_opts: Vec<(&str, String)> = vec![("on", tr("On")), ("off", tr("Off"))];
        if !draft.scene_id.is_empty() {
            action_opts.push(("scene", tr("Scene")));
        }
        let action_pills: Vec<Element<Message>> = action_opts
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

        if draft.action == "scene" && !draft.scene_id.is_empty() {
            let scene_lbl = if draft.scene_name.is_empty() {
                draft.scene_id.clone()
            } else {
                draft.scene_name.clone()
            };
            form = form.push(
                row![dim(tr("Scene")), space::horizontal(), text::body(scene_lbl)]
                    .spacing(8)
                    .align_y(Alignment::Center),
            );
        } else if draft.action == "on" {
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

        container(form).padding(12).width(Length::Fill).class(card(None, &self.skin)).into()
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
            act_head = act_head.push(mini_badge(log.len().to_string(), &self.skin));
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
                        .class(sub_card(&self.skin)),
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

        let trig_kind = s(rule, "trigger_kind");
        let trig_name = resolve_name(&self.stats, trig_kind, s(rule, "trigger_id"));
        let trig_state = trigger_state_label(trig_kind, s(rule, "trigger_state"));
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
            mini_badge(kind_label, &self.skin),
            space::horizontal(),
            toggler(enabled).on_toggle(move |v| Message::RuleToggle(toggle_id.clone(), v)),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let mut col = column![head, text::caption(summary_line)].spacing(6);
        if !conds.is_empty() {
            col = col.push(dim(conds.join(" · ")));
        }
        let confirming = self.confirm_delete_rule.as_deref() == Some(id.as_str());
        let mut foot = row![space::horizontal()].spacing(8).align_y(Alignment::Center);
        if confirming {
            foot = foot
                .push(button::standard(tr("Cancel")).on_press(Message::RuleConfirmDelete(None)))
                .push(button::destructive(tr("Delete")).on_press(Message::RuleDelete(id)));
        } else {
            foot = foot.push(
                button::destructive(tr("Delete")).on_press(Message::RuleConfirmDelete(Some(id))),
            );
        }
        col = col.push(foot);

        container(col).padding(12).width(Length::Fill).class(card(None, &self.skin)).into()
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
            for opt in all_rule_triggers(&self.stats) {
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
                    .class(sub_card(&self.skin)),
            );
        }

        let state_pills: Vec<Element<Message>> = trigger_state_options(&draft.trigger_kind)
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
                    .class(sub_card(&self.skin)),
            );
        }

        // Option checkboxes
        if draft.kind != "exclusive" {
            if draft.trigger_kind != "button" {
                form = form.push(
                    checkbox(draft.mirror_off)
                        .label(tr("Also turn off together"))
                        .on_toggle(Message::RuleMirrorOff),
                );
            }
            if is_light_or_room_trigger(&draft.trigger_kind) {
                form = form.push(
                    checkbox(draft.match_bri)
                        .label(tr("Match brightness"))
                        .on_toggle(Message::RuleMatchBri),
                );
            }
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

        container(form).padding(12).width(Length::Fill).class(card(None, &self.skin)).into()
    }

    // --------------------------------------------------------------------- Synth tab

    fn synth_tab(&self) -> Element<'_, Message> {
        let null = Value::Null;
        let synth = self.stats.get("synth").unwrap_or(&null);
        let running = flag(synth, "running");
        let source = match s(synth, "source") {
            "" => "system",
            v => v,
        };
        let mode = match s(synth, "mode") {
            "" => "club",
            v => v,
        };
        let palette = match s(synth, "palette") {
            "" => "neon",
            v => v,
        };
        let room_id = s(synth, "room_id");
        let sensitivity = self
            .synth_sens_drag
            .map(|(v, _)| v as f64)
            .unwrap_or_else(|| {
                synth.get("sensitivity").and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(1.0)
            })
            .clamp(0.4, 2.5);
        let smoothness = self
            .synth_smooth_drag
            .map(|(v, _)| v as f64)
            .unwrap_or_else(|| {
                synth.get("smoothness").and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(0.45)
            })
            .clamp(0.0, 0.9);
        let bass = num(synth, "bass");
        let mids = num(synth, "mids");
        let treble = num(synth, "treble");
        let beat = flag(synth, "beat") && running;
        let synth_err = s(synth, "error");
        let light_bands = synth.get("light_bands").and_then(Value::as_object);

        // Live visualizer & transport card
        let beat_label: Element<Message> = if beat {
            text::caption(tr("Beat"))
                .class(theme::Text::Custom(|_| iced_text::Style { color: Some(Color::WHITE), ..Default::default() }))
                .into()
        } else {
            dim(tr("Beat"))
        };
        let beat_pill = container(beat_label).padding([3, 10]).class(beat_pill_style(beat));
        let transport_btn = if running {
            button::destructive(tr("Stop")).on_press(Message::SynthStop)
        } else {
            button::suggested(tr("Start")).on_press(Message::SynthStart)
        };
        let mut live_col = column![
            row![text::heading(tr("Audio-reactive lights")), beat_pill, space::horizontal(), transport_btn]
                .spacing(8)
                .align_y(Alignment::Center),
            meter_bar(tr("Bass"), bass, Color::from_rgb(0.95, 0.25, 0.45)),
            meter_bar(tr("Mids"), mids, Color::from_rgb(0.15, 0.78, 0.95)),
            meter_bar(tr("Treble"), treble, Color::from_rgb(1.0, 0.78, 0.18)),
        ]
        .spacing(8);
        if !synth_err.is_empty() {
            live_col = live_col.push(error_text(synth_err.to_string()));
        }
        let live_tint = running.then(|| synth_palette_swatch(palette));
        let live_card = container(live_col)
            .padding(12)
            .width(Length::Fill)
            .class(card(live_tint, &self.skin));

        // Controls card
        let source_pills: Vec<Element<Message>> = SYNTH_SOURCES
            .iter()
            .map(|src| {
                let label = synth_source_label(src);
                let b = if source == *src { button::suggested(label) } else { button::standard(label) };
                b.on_press(Message::SynthSource((*src).to_string())).into()
            })
            .collect();

        let mode_pills: Vec<Element<Message>> = SYNTH_MODES
            .iter()
            .map(|m| {
                let label = synth_mode_label(m);
                let b = if mode == *m { button::suggested(label) } else { button::standard(label) };
                b.on_press(Message::SynthMode((*m).to_string())).into()
            })
            .collect();

        let palette_pills: Vec<Element<Message>> = SYNTH_PALETTES
            .iter()
            .map(|p| {
                let inner = row![
                    swatch_sized(Some(synth_palette_swatch(p)), true, 10.0),
                    text::caption(synth_palette_label(p)),
                ]
                .spacing(6)
                .align_y(Alignment::Center);
                button::custom(inner)
                    .padding([4, 8])
                    .class(if palette == *p { theme::Button::Suggested } else { theme::Button::Standard })
                    .on_press(Message::SynthPalette((*p).to_string()))
                    .into()
            })
            .collect();

        let room_label = if room_id.is_empty() {
            tr("All lights")
        } else {
            list(&self.stats, "rooms")
                .iter()
                .find(|r| s(r, "id") == room_id)
                .map(room_name)
                .unwrap_or_else(|| tr("All lights"))
        };

        let mut controls = column![
            column![
                dim(tr("Source")),
                flex_row(source_pills).row_spacing(6).column_spacing(6),
                dim(tr("Audio is analyzed strictly locally in memory and never recorded or sent anywhere.")),
            ]
            .spacing(4),
            column![dim(tr("Mode")), flex_row(mode_pills).row_spacing(6).column_spacing(6)].spacing(4),
            column![dim(tr("Palette")), flex_row(palette_pills).row_spacing(6).column_spacing(6)].spacing(4),
            row![
                dim(tr("Target room")),
                space::horizontal(),
                button::text(room_label)
                    .trailing_icon(
                        icon::from_name(if self.synth_room_picker_open { "pan-up-symbolic" } else { "pan-down-symbolic" })
                            .size(16),
                    )
                    .on_press(Message::SynthToggleRoomPicker),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(10);

        if self.synth_room_picker_open {
            let mut opts = column![].spacing(2);
            let all_lbl = if room_id.is_empty() { format!("✓ {}", tr("All lights")) } else { tr("All lights") };
            opts = opts.push(menu_button(text::caption(all_lbl)).on_press(Message::SynthPickRoom(String::new())));
            for room in list(&self.stats, "rooms") {
                let rid = s(room, "id");
                if rid.is_empty() || !matches!(s(room, "kind"), "room" | "zone") {
                    continue;
                }
                let rname = room_name(room);
                let lbl = if room_id == rid { format!("✓ {rname}") } else { rname };
                opts = opts.push(menu_button(text::caption(lbl)).on_press(Message::SynthPickRoom(rid.to_string())));
            }
            controls = controls.push(
                container(scrollable(opts).height(Length::Shrink))
                    .max_height(150.0)
                    .padding(4)
                    .width(Length::Fill)
                    .class(sub_card(&self.skin)),
            );
        }

        let sens_val = (sensitivity * 100.0).round().clamp(40.0, 250.0) as u16;
        let smooth_val = (smoothness * 100.0).round().clamp(0.0, 90.0) as u16;
        controls = controls.push(
            row![
                container(dim(tr("Sensitivity"))).width(Length::Fixed(86.0)),
                slider(40u16..=250u16, sens_val, Message::SynthSensitivity)
                    .step(5u16)
                    .on_release(Message::SynthSensitivityDone)
                    .width(Length::Fill),
                container(text::caption(format!("{:.2}×", sens_val as f32 / 100.0)))
                    .width(Length::Fixed(44.0))
                    .align_x(Alignment::End),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
        controls = controls.push(
            row![
                container(dim(tr("Smoothness"))).width(Length::Fixed(86.0)),
                slider(0u16..=90u16, smooth_val, Message::SynthSmoothness)
                    .step(5u16)
                    .on_release(Message::SynthSmoothnessDone)
                    .width(Length::Fill),
                container(text::caption(format!("{smooth_val}%")))
                    .width(Length::Fixed(44.0))
                    .align_x(Alignment::End),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );

        let controls_card = container(controls)
            .padding(12)
            .width(Length::Fill)
            .class(card(None, &self.skin));

        let mut body = column![live_card, controls_card].spacing(10);

        // Per-light frequency band routing
        let target_lights = synth_target_lights(&self.stats, room_id);
        if !target_lights.is_empty() {
            let mut routing = column![dim(tr("Light bands"))].spacing(6);
            for (lid, lname) in target_lights {
                let active_band = light_bands
                    .and_then(|m| m.get(&lid))
                    .and_then(Value::as_str)
                    .unwrap_or("all");
                let band_pills: Vec<Element<Message>> = SYNTH_BANDS
                    .iter()
                    .map(|b| {
                        let lbl = synth_band_label(b);
                        let btn = if active_band == *b {
                            button::suggested(lbl)
                        } else {
                            button::standard(lbl)
                        };
                        btn.on_press(Message::SynthLightBand(lid.clone(), (*b).to_string())).into()
                    })
                    .collect();
                routing = routing.push(
                    column![
                        text::caption(lname),
                        flex_row(band_pills).row_spacing(4).column_spacing(4),
                    ]
                    .spacing(4),
                );
            }
            body = body.push(
                container(routing)
                    .padding(12)
                    .width(Length::Fill)
                    .class(card(None, &self.skin)),
            );
        }

        if self.window_mode {
            body.into()
        } else {
            container(scrollable(container(body).padding([0, 12, 0, 0])).height(Length::Shrink))
                .max_height(ROOMS_MAX_HEIGHT)
                .into()
        }
    }

    // --------------------------------------------------------------------- Main details

    fn details(&self) -> Element<'_, Message> {
        let mut content = column![self.header()].spacing(12);
        let error = s(&self.stats, "error");
        if !error.is_empty() {
            content = content.push(
                row![
                    error_text(maprisma_hue_lights::i18n::error(&self.stats)),
                    space::horizontal(),
                    button::standard(tr("Retry")).on_press(Message::Refresh),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        if !self.paired() {
            content = content.push(divider::horizontal::default());
            content = content.push(self.pairing());
            return content.into();
        }
        content = content.push(self.tab_bar());
        content = content.push(match self.tab {
            Tab::Lights => self.lights_tab(),
            Tab::Map => view_map_tab(
                &self.floorplan,
                &self.stats,
                &self.skin,
                self.window_mode,
                self.window_width,
                self.pending_sync_count,
                self.motion_hint_dismissed,
            )
            .map(Message::Map),
            Tab::Schedules => self.schedules_tab(),
            Tab::Rules => self.rules_tab(),
            Tab::Synth => self.synth_tab(),
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
                        move || shot::save_note_for(&SHOT_APP, &path, &text).map(|_| ()).map_err(|e| e.to_string()),
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
        if !self.window_mode {
            return;
        }
        let sharp = self.core.window.sharp_corners;
        let ignore = || window_size::not_users_size(sharp);
        let size = if closing { self.resize.flush(ignore) } else { self.resize.due(Instant::now(), ignore) };
        if let Some(size) = size {
            self.setting_error = write_pref(window_size::KEY, Some(window_size::to_value(size))).is_err();
        }
        // Hyprland: where the user left it, for the next open (saved on resize settle and on close/quit).
        if (closing || size.is_some())
            && let Some(position) = window_size::current_position()
        {
            self.setting_error |=
                write_pref(window_size::POSITION_KEY, Some(window_size::position_value(position))).is_err();
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
        let labels = shot::ShotLabels {
            saved_heading: tr("Screenshot saved"),
            saved_as_template: tr("Saved as {0}"),
            note_placeholder: tr("What is wrong? (optional)"),
            done_button: tr("Done"),
            save_error_template: tr("The screenshot could not be saved: {0}"),
            note_error_template: tr("The note could not be saved: {0}"),
        };
        let width = if self.window_mode { Length::Fill } else { Length::Fixed(POPUP_WIDTH) };
        shot::note_overlay(
            &self.shot,
            NOTE_ID.clone(),
            &labels,
            theme::Container::Card,
            width,
            Message::NoteInput,
            Message::NoteDone,
        )
    }

    /// Settings (as Maprisma Launcher's page): ← Back and the title, then one row per setting,
    /// words + short description at the left and the control at the right; a list opens under its row.
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
        let setting = |label: String, desc: String, control: Element<'static, Message>| {
            padded_control(
                row![
                    column![text::body(label), dim(desc)].spacing(2).width(Length::Fill),
                    control,
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
        };
        let mut page = column![padded_control(header), divider::horizontal::default()];

        // App skin (6 fun skins)
        page = page.push(setting(
            tr("App skin"),
            tr("Visual accent for cards and borders; Native keeps your desktop theme untouched."),
            choice(skin_label(&self.skin), self.skin_open, Message::ToggleSkin),
        ));
        if self.skin_open {
            for skin in SKINS {
                let name = skin_label(skin);
                let label = if self.skin == skin { format!("✓ {name}") } else { name };
                let row_inner: Element<Message> = if let Some(st) = skin_style(skin) {
                    row![swatch_sized(Some(st.accent), true, 10.0), text::body(label)]
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .into()
                } else {
                    text::body(label).into()
                };
                page = page.push(menu_button(row_inner).on_press(Message::SetSkin(skin.into())));
            }
        }

        // Panel icon (8 selectable icons)
        let current_icon = format!("{} {}", panel_icon_glyph(&self.panel_icon), panel_icon_label(&self.panel_icon));
        page = page.push(setting(
            tr("Panel icon"),
            tr("Choose the icon shown in your desktop panel."),
            choice(current_icon, self.panel_icon_open, Message::TogglePanelIcon),
        ));
        if self.panel_icon_open {
            for icon_key in PANEL_ICONS {
                let text_lbl = format!("{} {}", panel_icon_glyph(icon_key), panel_icon_label(icon_key));
                let label = if self.panel_icon == icon_key { format!("✓ {text_lbl}") } else { text_lbl };
                page = page.push(menu_button(text::body(label)).on_press(Message::SetPanelIcon(icon_key.into())));
            }
        }

        // Show lit count in panel
        page = page.push(setting(
            tr("Show lit count in panel"),
            tr("Show the number of active lights next to the panel icon."),
            toggler(self.show_count).on_toggle(Message::SetShowCount).into(),
        ));

        // Text size (font scale)
        page = page.push(setting(
            tr("Text size"),
            tr("Adjust interface text scale in window mode."),
            choice(font_scale_label(&self.font_scale), self.font_scale_open, Message::ToggleFontScale),
        ));
        if self.font_scale_open {
            for sc in FONT_SCALES {
                let name = font_scale_label(sc);
                let label = if self.font_scale == sc { format!("✓ {name}") } else { name };
                page = page.push(menu_button(text::body(label)).on_press(Message::SetFontScale(sc.into())));
            }
        }

        // Schedule mode (On Hue Bridge vs On this PC (live))
        page = page.push(setting(
            tr("Schedule mode"),
            tr("Bridge schedules run 24/7 even when your PC is off; PC schedules run while Maprisma Lights is open."),
            choice(schedule_mode_label(&self.schedule_mode), self.schedule_mode_open, Message::ToggleScheduleMode),
        ));
        if self.schedule_mode_open {
            for mode in SCHEDULE_MODES {
                let name = schedule_mode_label(mode);
                let label = if self.schedule_mode == mode { format!("✓ {name}") } else { name };
                page = page.push(menu_button(text::body(label)).on_press(Message::SetScheduleMode(mode.into())));
            }
        }

        // Confirm before All On
        page = page.push(setting(
            tr("Confirm before All On"),
            tr("Ask for confirmation before turning on every light."),
            toggler(self.confirm_all_on_pref).on_toggle(Message::SetConfirmAllOnPref).into(),
        ));

        // Corners
        let corner_choices = [
            ("desktop", trf("Like the desktop ({0} px)", &[corners::desktop_radius().round().to_string()])),
            ("slight", trf("Slightly rounded ({0} px)", &["6".into()])),
            ("rounded", trf("Rounded ({0} px)", &["12".into()])),
            ("square", tr("Square")),
        ];
        let current_corner = corner_choices.iter().find(|(c, _)| *c == self.corners).map(|(_, l)| l.clone()).unwrap_or_default();
        page = page.push(setting(
            tr("Corners"),
            tr("Adjust corner rounding for cards and buttons."),
            choice(current_corner, self.corners_open, Message::ToggleCorners),
        ));
        if self.corners_open {
            for (c, label) in corner_choices {
                let label = if self.corners == c { format!("✓ {label}") } else { label };
                page = page.push(menu_button(text::body(label)).on_press(Message::Corners(c.into())));
            }
        }

        // Language
        let language = maprisma_hue_lights::i18n::setting();
        let choices = maprisma_hue_lights::i18n::choices();
        let current_lang = choices.iter().find(|(code, _)| *code == language).map(|(_, l)| l.clone()).unwrap_or_default();
        page = page.push(setting(
            tr("Language"),
            tr("Choose the display language for Maprisma Lights."),
            choice(current_lang, self.language_open, Message::ToggleLanguage),
        ));
        if self.language_open {
            for (code, label) in choices {
                let label = if code == language { format!("✓ {label}") } else { label };
                page = page.push(menu_button(text::body(label)).on_press(Message::Language(code)));
            }
        }

        // Ask AI (Optional, off by default)
        page = page.push(setting(
            tr("Ask AI (Optional)"),
            tr("Optional, off by default. Control lamps, rules, and schedules in plain language — runs locally on this PC."),
            toggler(self.ask_ai_enabled).on_toggle(Message::SetAskAi).into(),
        ));

        // F12 saves a screenshot
        page = page.push(setting(
            tr("F12 saves a screenshot"),
            tr("Press F12 to save a screenshot to your Pictures folder."),
            toggler(self.shot_key).on_toggle(Message::SetShotKey).into(),
        ));

        page = page.push(divider::horizontal::default());
        if !flag(&self.stats, "hue_paired") && self.paired() {
            let ip = self.ip.trim().to_string();
            page = page.push(padded_control(
                column![
                    text::body(tr("Pair a Hue bridge")),
                    row![
                        text_input(tr("Bridge IP, e.g. 192.168.178.106"), &self.ip)
                            .on_input(Message::Ip)
                            .on_submit(|value: String| Message::Pair(value.trim().to_string()))
                            .width(Length::Fill),
                        button::standard(tr("Pair")).on_press_maybe((!ip.is_empty()).then_some(Message::Pair(ip)))
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                ]
                .spacing(6),
            ));
            page = page.push(divider::horizontal::default());
        }
        page = page.push(padded_control(self.extra_devices_section(false)));

        if self.language_error { page = page.push(padded_control(error_text(tr("Could not save language setting.")))); }
        if self.setting_error { page = page.push(padded_control(error_text(tr("Could not save the setting.")))); }
        page = page.push(padded_control(dim(format!("Maprisma Lights v{}", env!("CARGO_PKG_VERSION")))));
        let page = page.width(if self.window_mode { Length::Fill } else { Length::Fixed(POPUP_WIDTH) }).padding([0, 0, 8, 0]);
        scrollable(page).height(if self.window_mode { Length::Fill } else { Length::Shrink }).into()
    }

    fn ask_ai_page(&self) -> Element<'_, Message> {
        let header = row![
            button::text(tr("Back"))
                .leading_icon(icon::from_name("go-previous-symbolic").size(16))
                .on_press(Message::ToggleAskAi),
            space::horizontal().width(Length::Fill),
            dim(tr("Ask AI").to_uppercase()),
        ]
        .align_y(Alignment::Center);

        let mut submit_btn = button::suggested(tr("Ask"));
        if !self.ai_state.busy && !self.ask_ai_input.trim().is_empty() {
            submit_btn = submit_btn.on_press(Message::AskAiSubmit);
        }

        let input_row = row![
            text_input(tr("Ask to control lights, scenes, rules, or schedules…"), &self.ask_ai_input)
                .id(ASK_AI_ID.clone())
                .on_input(Message::AskAiInput)
                .on_submit(|_| Message::AskAiSubmit)
                .width(Length::Fill),
            submit_btn,
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let examples = [
            tr("Warm living room 40%"),
            tr("Turn off all lights"),
            tr("When motion in hall turn on lamp"),
            tr("Sunrise warm white"),
        ];
        let chips: Vec<Element<Message>> = examples
            .into_iter()
            .map(|ex| {
                let mut chip = button::standard(ex.clone());
                if !self.ai_state.busy {
                    chip = chip.on_press(Message::AskAiExample(ex));
                }
                chip.into()
            })
            .collect();

        let mut resp_col = column![].spacing(8).width(Length::Fill);
        if self.ai_state.busy {
            resp_col = resp_col.push(text::body(tr("Thinking…")));
            if !self.ai_state.last_prompt.is_empty() {
                resp_col = resp_col.push(dim(format!("“{}”", self.ai_state.last_prompt)));
            }
        } else if !self.ai_state.last_reply.is_empty()
            || !self.ai_state.error.is_empty()
            || !self.ai_state.last_actions.is_empty()
        {
            let mut top = row![].spacing(8).align_y(Alignment::Center);
            if !self.ai_state.last_prompt.is_empty() {
                top = top.push(dim(format!("“{}”", self.ai_state.last_prompt)));
            }
            top = top.push(space::horizontal().width(Length::Fill));
            if !self.ai_state.engine.is_empty() && self.ai_state.engine != "none" {
                top = top.push(mini_badge(self.ai_state.engine.clone(), &self.skin));
            }
            top = top.push(button::text(tr("Clear")).on_press(Message::AskAiClear));
            resp_col = resp_col.push(top);

            if !self.ai_state.last_reply.is_empty() {
                resp_col = resp_col.push(text::body(&self.ai_state.last_reply));
            }
            if !self.ai_state.last_actions.is_empty() {
                let mut acts = column![dim(tr("Actions"))].spacing(4);
                for act in &self.ai_state.last_actions {
                    acts = acts.push(text::caption(format!("• {act}")));
                }
                resp_col = resp_col.push(acts);
            }
            if !self.ai_state.error.is_empty() {
                resp_col = resp_col.push(error_text(self.ai_state.error.clone()));
            }
        } else {
            resp_col = resp_col.push(dim(tr("Try an example above or type any command in English or German.")));
        }

        let response_card = container(resp_col)
            .padding(12)
            .width(Length::Fill)
            .class(card(None, &self.skin));

        let body = column![
            dim(tr("Runs 100% locally on this PC (with optional local Ollama on localhost).")),
            input_row,
            column![
                dim(tr("Examples")),
                flex_row(chips).row_spacing(6).column_spacing(6),
            ]
            .spacing(6),
            response_card,
        ]
        .spacing(12)
        .width(Length::Fill);

        let page = column![
            padded_control(header),
            divider::horizontal::default(),
            container(body).padding([12, 16]).width(Length::Fill),
        ]
        .width(if self.window_mode { Length::Fill } else { Length::Fixed(POPUP_WIDTH) })
        .padding([0, 0, 8, 0]);

        scrollable(page).height(if self.window_mode { Length::Fill } else { Length::Shrink }).into()
    }

    fn popup_content(&self) -> Element<'_, Message> {
        if let Some(overlay) = self.shot_overlay() {
            return overlay;
        }
        if self.help_open {
            return help::view_help_page(
                self.help_phase,
                &self.skin,
                self.window_mode,
                self.help_selected,
                Message::ToggleHelp,
                Message::HelpSelect,
                Message::HelpTry,
            );
        }
        if self.ask_ai_enabled && self.ask_ai_open {
            return self.ask_ai_page();
        }
        if self.settings_open {
            return self.settings_page();
        }
        let content = column![padded_control(self.details()).padding([12, 16]), self.actions()]
            .width(if self.window_mode { Length::Fill } else { Length::Fixed(POPUP_WIDTH) })
            .padding([0, 0, 8, 0]);
        let base: Element<Message> = if self.window_mode {
            // Keep every action reachable when the normal window is resized smaller.
            scrollable(content).height(Length::Fill).into()
        } else {
            content.into()
        };
        let dragging_bri = match &self.floorplan.lamp_dialog {
            Some(LampDialog::Brightness(lid)) => self.dragging.get(&format!("light:{lid}")).map(|(v, _)| *v),
            _ => None,
        };
        if let Some(dialog) = view_lamp_dialog_overlay(&self.floorplan, &self.stats, &self.skin, dragging_bri) {
            iced_stack![base, dialog.map(Message::Map)]
                .width(if self.window_mode { Length::Fill } else { Length::Fixed(POPUP_WIDTH) })
                .height(if self.window_mode { Length::Fill } else { Length::Shrink })
                .into()
        } else {
            base
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
        look::solid(&mut core);
        if window_mode { core.set_header_title(TITLE.into()); }
        let start_size = window_start_size();
        let mut app = Self {
            core,
            window_mode,
            window_width: if window_mode { start_size.0 } else { POPUP_WIDTH },
            shot_key: shot_key_setting(),
            confirm_all_on_pref: confirm_all_on_setting(),
            ask_ai_enabled: ask_ai_setting(),
            corners: corners_setting(),
            schedule_mode: schedule_mode_setting(),
            skin: skin_setting(),
            panel_icon: panel_icon_setting(),
            show_count: show_count_setting(),
            font_scale: font_scale_setting(),
            favorites: favorites_setting(),
            exclude_all_on: exclude_all_on_setting(),
            ..Default::default()
        };
        // Debug builds only: start on a specific page/tab (for off-screen screenshots).
        if cfg!(debug_assertions) {
            match std::env::var("MAPRISMA_SHOT_PAGE").as_deref() {
                Ok("settings") => app.settings_open = true,
                Ok("ask_ai") => {
                    app.ask_ai_enabled = true;
                    app.ask_ai_open = true;
                }
                Ok("map") => app.tab = Tab::Map,
                Ok("schedules") => app.tab = Tab::Schedules,
                Ok("rules") => app.tab = Tab::Rules,
                Ok("synth") => app.tab = Tab::Synth,
                _ => {}
            }
        }
        let title = if window_mode { app.name_window() } else { Task::none() };
        if window_mode {
            app.resize = window_size::Debounce::new(start_size);
            // Hyprland: back where the user left it (the tray rule first puts it under the bar).
            window_size::restore_position(window_size::parse_position(read_pref(window_size::POSITION_KEY).as_ref()), start_size);
        }
        corners::set_current(&app.corners);
        app.rebuild_cached_theme();
        // The desktop's or skin's theme for the whole window (title bar too), see look.rs.
        let theme = app.apply_window_theme();
        (app, Task::batch([title, theme]))
    }

    /// COSMIC's colours or light/dark mode changed: a window with its own corners (a custom
    /// theme, which libcosmic leaves alone) takes the new colours too.
    fn system_theme_update(&mut self, _keys: &[&'static str], new_theme: &cosmic::cosmic_theme::Theme) -> Task<Message> {
        let _ = corners::system_theme_update::<Message>(&self.corners, new_theme, false);
        self.rebuild_cached_theme();
        self.apply_window_theme()
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
        let help_anim = if self.help_open {
            cosmic::iced::time::every(Duration::from_millis(50)).map(|_| Message::HelpTick)
        } else { Subscription::none() };
        Subscription::batch(vec![close, rectangles, keys, save_size, help_anim,
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
                self.send(json!({"cmd": "schedule_mode", "mode": self.schedule_mode}));
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
                if let Some(act) = self.stats.get("provider_action").and_then(Value::as_object) {
                    let act_kind = act.get("kind").and_then(Value::as_str).unwrap_or("");
                    let act_ok = act.get("ok").and_then(Value::as_bool) == Some(true);
                    let act_running = act.get("running").and_then(Value::as_bool) == Some(true);
                    let pair_id = act.get("pair_id").and_then(Value::as_str).unwrap_or("");
                    if let Some(draft) = &mut self.provider_draft {
                        if act_ok && !pair_id.is_empty() && act_kind == format!("pair_{}", draft.kind) {
                            draft.pair_id = pair_id.to_string();
                        }
                        if draft.awaiting_save && act_kind == "save" && !act_running {
                            draft.awaiting_save = false;
                            if act_ok {
                                self.provider_draft = None;
                            }
                        }
                    }
                }
                self.ai_state = AiState::from_stats(&self.stats);
                if let Some(fp) = parse_floorplan(&self.stats) {
                    self.floorplan.sync_from_snapshot(fp);
                }
                self.refresh_pending_sync();
                // Single config.json read per snapshot; all settings below come from the in-memory cache.
                maprisma_hue_lights::i18n::reload_prefs();
                // Settings changed in another interface (window, applet) take effect here too.
                self.shot_key = shot_key_setting();
                self.confirm_all_on_pref = confirm_all_on_setting();
                self.ask_ai_enabled = ask_ai_setting();
                if !self.ask_ai_enabled {
                    self.ask_ai_open = false;
                }
                self.schedule_mode = schedule_mode_setting();
                let skin = skin_setting();
                let skin_changed = skin != self.skin;
                if skin_changed {
                    self.skin = skin;
                }
                self.panel_icon = panel_icon_setting();
                self.show_count = show_count_setting();
                let font_scale = font_scale_setting();
                let font_changed = font_scale != self.font_scale;
                if font_changed {
                    self.font_scale = font_scale;
                }
                self.favorites = favorites_setting();
                self.exclude_all_on = exclude_all_on_setting();
                if self.filter == RoomFilter::Favorites && self.favorites.is_empty() {
                    self.filter = RoomFilter::All;
                }
                let corners = corners_setting();
                let corners_changed = corners != self.corners;
                if corners_changed {
                    corners::set_current(&corners);
                    self.corners = corners;
                }
                let theme_changed = skin_changed || corners_changed;
                if theme_changed {
                    self.rebuild_cached_theme();
                }
                let restyle = theme_changed || font_changed;
                // The desktop's colours and corners (Maprisma Theme can change them any time), off the UI thread.
                let refresh_look = if !self.look_refreshing && look::is_refresh_due() {
                    self.look_refreshing = true;
                    off_thread_value(look::refresh, Message::LookRefreshed)
                } else {
                    Task::none()
                };
                let theme_task = if restyle && self.window_mode {
                    self.apply_window_theme()
                } else {
                    Task::none()
                };
                return Task::batch([theme_task, refresh_look]);
            }
            Message::LookRefreshed(changed) => {
                self.look_refreshing = false;
                if changed {
                    self.rebuild_cached_theme();
                    return self.apply_window_theme();
                }
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
                self.floorplan.close_dialog();
            }
            Message::Map(m) => {
                return self.handle_map_msg(m);
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
                self.open_lights_menu = None;
                if self.open_light_drawer.as_deref() == Some(id.as_str()) {
                    self.open_light_drawer = None;
                    self.theme_draft = None;
                } else {
                    self.open_light_drawer = Some(id);
                    self.theme_draft = None;
                }
            }
            Message::ToggleLightsMenu(id) => {
                if self.open_lights_menu.as_deref() == Some(id.as_str()) {
                    self.open_lights_menu = None;
                } else {
                    self.open_lights_menu = Some(id);
                }
            }
            Message::LightsMenuBrightness(id) => {
                self.open_lights_menu = None;
                self.floorplan.open_lamp_dialog(LampDialog::Brightness(id), &self.stats);
            }
            Message::LightsMenuColor(id) => {
                self.open_lights_menu = None;
                self.open_light_drawer = Some(id);
                self.theme_draft = None;
            }
            Message::LightsMenuRename(id) => {
                self.open_lights_menu = None;
                self.floorplan.open_lamp_dialog(LampDialog::Rename(id), &self.stats);
            }
            Message::LightsMenuIcon(id) => {
                self.open_lights_menu = None;
                self.floorplan.open_lamp_dialog(LampDialog::Icon(id), &self.stats);
            }
            Message::LightsMenuWireOnMap(id) => {
                self.open_lights_menu = None;
                self.open_light_drawer = None;
                self.floorplan.close_menus();
                self.floorplan.close_dialog();
                self.tab = Tab::Map;
                self.floorplan.mode = CanvasMode::Wire;
                if self.floorplan.doc.lamps.contains_key(&id) {
                    self.floorplan.pending_wire_from = Some(("light".into(), id));
                } else {
                    self.floorplan.pending_wire_from = None;
                }
            }
            Message::DismissMotionHint => {
                self.motion_hint_dismissed = true;
            }
            Message::ToggleFavorite(kind, id) => {
                self.open_lights_menu = None;
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
            Message::ThemeApply(theme_id, light_id) => {
                self.send(json!({"cmd": "theme_apply", "theme_id": theme_id, "light_id": light_id}));
            }
            Message::ThemeDelete(theme_id) => {
                self.send(json!({"cmd": "theme_delete", "id": theme_id}));
            }
            Message::StartSaveTheme(light_id) => {
                if let Some(light) = self.find_light(&light_id) {
                    self.theme_draft = Some(ThemeDraft::from_light(light));
                }
            }
            Message::ThemeDraftName(name) => {
                if let Some(d) = &mut self.theme_draft {
                    d.name = name.chars().take(60).collect();
                    d.error = None;
                }
            }
            Message::ThemeDraftSave => {
                if let Some(d) = &mut self.theme_draft {
                    match d.to_command() {
                        Ok(cmd) => {
                            self.send(cmd);
                            self.theme_draft = None;
                        }
                        Err(err) => d.error = Some(err),
                    }
                }
            }
            Message::ThemeDraftCancel => {
                self.theme_draft = None;
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
                    if self.schedule_mode == "local" {
                        d.trigger_kind = "time".into();
                    }
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
            Message::ScheduleConfirmDelete(id) => {
                self.confirm_delete_schedule = id;
            }
            Message::ScheduleDelete(id) => {
                self.confirm_delete_schedule = None;
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
                    let kind_changed = d.trigger_kind != kind;
                    d.trigger_kind = kind.clone();
                    d.trigger_id = id;
                    d.trigger_name = label;
                    if kind_changed {
                        d.trigger_state = default_trigger_state(&kind).into();
                    }
                    if !is_light_or_room_trigger(&kind) {
                        d.match_bri = false;
                    }
                    if kind == "button" {
                        d.mirror_off = false;
                    }
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
            Message::RuleConfirmDelete(id) => {
                self.confirm_delete_rule = id;
            }
            Message::RuleDelete(id) => {
                self.confirm_delete_rule = None;
                self.send(json!({"cmd": "rule_delete", "id": id}));
            }
            Message::ToggleActivity => {
                self.activity_open = !self.activity_open;
            }

            // Synth
            Message::SynthStart => {
                self.send(json!({"cmd": "synth_start"}));
            }
            Message::SynthStop => {
                self.send(json!({"cmd": "synth_stop"}));
            }
            Message::SynthSource(source) => {
                self.send(json!({"cmd": "synth_config", "source": source}));
            }
            Message::SynthMode(mode) => {
                self.send(json!({"cmd": "synth_config", "mode": mode}));
            }
            Message::SynthPalette(palette) => {
                self.send(json!({"cmd": "synth_config", "palette": palette}));
            }
            Message::SynthToggleRoomPicker => {
                self.synth_room_picker_open = !self.synth_room_picker_open;
            }
            Message::SynthPickRoom(room_id) => {
                self.synth_room_picker_open = false;
                self.send(json!({"cmd": "synth_config", "room_id": room_id}));
            }
            Message::SynthSensitivity(raw) => {
                let val = (raw.clamp(40, 250) as f32) / 100.0;
                let now = Instant::now();
                let due = self.synth_sens_drag.is_none_or(|(_, sent)| now.duration_since(sent) >= SEND_EVERY);
                let sent = if due {
                    self.send(json!({"cmd": "synth_config", "sensitivity": val}));
                    now
                } else {
                    self.synth_sens_drag.map_or(now, |(_, s)| s)
                };
                self.synth_sens_drag = Some((val, sent));
            }
            Message::SynthSensitivityDone => {
                if let Some((val, _)) = self.synth_sens_drag.take() {
                    self.send(json!({"cmd": "synth_config", "sensitivity": val}));
                }
            }
            Message::SynthSmoothness(raw) => {
                let val = (raw.clamp(0, 90) as f32) / 100.0;
                let now = Instant::now();
                let due = self.synth_smooth_drag.is_none_or(|(_, sent)| now.duration_since(sent) >= SEND_EVERY);
                let sent = if due {
                    self.send(json!({"cmd": "synth_config", "smoothness": val}));
                    now
                } else {
                    self.synth_smooth_drag.map_or(now, |(_, s)| s)
                };
                self.synth_smooth_drag = Some((val, sent));
            }
            Message::SynthSmoothnessDone => {
                if let Some((val, _)) = self.synth_smooth_drag.take() {
                    self.send(json!({"cmd": "synth_config", "smoothness": val}));
                }
            }
            Message::SynthLightBand(light_id, band) => {
                self.send(json!({"cmd": "synth_config", "light_id": light_id, "band": band}));
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
                return off_thread(move || shot::save_screenshot_for(&SHOT_APP, &picture.rgba, width, height), Message::ShotSaved);
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
                let help_detail = self.help_open && self.help_selected.is_some();
                let confirm = !self.help_open && (self.confirm_unpair || self.confirm_all_on);
                let menus = !self.help_open && (self.any_settings_menu_open() || self.any_picker_open());
                let drawer = !self.help_open && self.any_drawer_open();
                match esc_step(
                    shown,
                    self.shot.open(),
                    help_detail,
                    confirm,
                    menus,
                    drawer,
                    self.settings_open || self.help_open || self.ask_ai_open,
                ) {
                    EscStep::Note => return self.finish_note(false),
                    EscStep::HelpOverview => {
                        self.help_selected = None;
                    }
                    EscStep::Confirm => {
                        self.confirm_unpair = false;
                        self.confirm_all_on = false;
                    }
                    EscStep::Menus => {
                        self.close_settings_menus();
                        self.close_pickers();
                    }
                    EscStep::Drawer => {
                        self.open_light_drawer = None;
                        self.theme_draft = None;
                        self.schedule_draft = None;
                        self.rule_draft = None;
                        self.floorplan.close_dialog();
                    }
                    EscStep::Back => {
                        self.settings_open = false;
                        self.help_open = false;
                        self.help_selected = None;
                        self.ask_ai_open = false;
                    }
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
                self.rebuild_cached_theme();
                if self.window_mode { return self.apply_window_theme(); }
            }
            Message::SetScheduleMode(mode) => {
                let mode = normalize_schedule_mode(&mode);
                let value = (mode != "bridge").then(|| Value::from(mode));
                self.setting_error = write_pref(SCHEDULE_MODE_KEY, value).is_err();
                self.schedule_mode = mode.to_string();
                self.schedule_mode_open = false;
                if mode == "local" && let Some(d) = &mut self.schedule_draft {
                    d.trigger_kind = "time".into();
                }
                self.send(json!({"cmd": "schedule_mode", "mode": mode}));
            }
            Message::SetSkin(skin) => {
                let skin = normalize_skin(&skin);
                let value = (skin != "native").then(|| Value::from(skin));
                self.setting_error = write_pref(SKIN_KEY, value).is_err();
                self.skin = skin.to_string();
                self.skin_open = false;
                self.rebuild_cached_theme();
                if self.window_mode { return self.apply_window_theme(); }
            }
            Message::SetPanelIcon(icon_key) => {
                let icon_key = normalize_panel_icon(&icon_key);
                let value = (icon_key != "bulb").then(|| Value::from(icon_key));
                self.setting_error = write_pref(PANEL_ICON_KEY, value).is_err();
                self.panel_icon = icon_key.to_string();
                self.panel_icon_open = false;
            }
            Message::SetShowCount(on) => {
                let value = on.then(|| Value::from(true));
                self.setting_error = write_pref(SHOW_COUNT_KEY, value).is_err();
                self.show_count = on;
            }
            Message::SetFontScale(scale) => {
                let scale = normalize_font_scale(&scale);
                let value = (scale != "normal").then(|| Value::from(scale));
                self.setting_error = write_pref(FONT_SCALE_KEY, value).is_err();
                self.font_scale = scale.to_string();
                self.font_scale_open = false;
                if self.window_mode { return self.apply_window_theme(); }
            }
            Message::SetAskAi(on) => {
                let value = on.then(|| Value::from(true));
                self.setting_error = write_pref(ASK_AI_KEY, value).is_err();
                self.ask_ai_enabled = on;
                if !on {
                    self.ask_ai_open = false;
                }
            }
            Message::ToggleSettings => {
                self.settings_open = !self.settings_open;
                self.help_open = false;
                self.help_selected = None;
                self.ask_ai_open = false;
                self.close_settings_menus();
                self.close_pickers();
                self.confirm_unpair = false;
                self.confirm_all_on = false;
            }
            Message::ToggleHelp => {
                self.help_open = !self.help_open;
                self.help_selected = None;
                self.settings_open = false;
                self.ask_ai_open = false;
                self.close_settings_menus();
                self.close_pickers();
            }
            Message::HelpSelect(idx) => {
                self.help_selected = help::resolve_selected_feature(idx);
            }
            Message::HelpTry(action) => {
                self.help_open = false;
                self.help_selected = None;
                self.ask_ai_open = false;
                self.close_settings_menus();
                self.close_pickers();
                self.floorplan.close_dialog();
                match action {
                    help::HelpAction::OpenLights => {
                        self.settings_open = false;
                        self.tab = Tab::Lights;
                    }
                    help::HelpAction::OpenMap => {
                        self.settings_open = false;
                        self.tab = Tab::Map;
                        self.floorplan.mode = CanvasMode::Select;
                    }
                    help::HelpAction::OpenMapRules => {
                        self.settings_open = false;
                        self.tab = Tab::Map;
                        self.floorplan.mode = CanvasMode::Wire;
                    }
                    help::HelpAction::OpenSchedules => {
                        self.settings_open = false;
                        self.tab = Tab::Schedules;
                    }
                    help::HelpAction::OpenSynth => {
                        self.settings_open = false;
                        self.tab = Tab::Synth;
                    }
                    help::HelpAction::OpenSettings => {
                        self.settings_open = true;
                    }
                }
            }
            Message::ToggleAskAi => {
                if self.ask_ai_enabled {
                    self.ask_ai_open = !self.ask_ai_open;
                    self.settings_open = false;
                    self.help_open = false;
                    self.help_selected = None;
                    self.close_settings_menus();
                    self.close_pickers();
                    if self.ask_ai_open {
                        return cosmic::widget::text_input::focus(ASK_AI_ID.clone());
                    }
                } else {
                    self.ask_ai_open = false;
                }
            }
            Message::AskAiInput(value) => {
                self.ask_ai_input = value.chars().take(500).collect();
            }
            Message::AskAiSubmit => {
                let prompt = self.ask_ai_input.trim().to_string();
                if self.ask_ai_enabled && !self.ai_state.busy && !prompt.is_empty() {
                    self.send(json!({"cmd": "ai_ask", "prompt": prompt}));
                }
            }
            Message::AskAiExample(example) => {
                if self.ask_ai_enabled && !self.ai_state.busy {
                    self.ask_ai_input = example.clone();
                    self.send(json!({"cmd": "ai_ask", "prompt": example}));
                }
            }
            Message::AskAiClear => {
                self.ask_ai_input.clear();
                self.send(json!({"cmd": "ai_clear"}));
            }
            Message::HelpTick => {
                self.help_phase = (self.help_phase + 0.0125).rem_euclid(1.0);
            }
            Message::ToggleLanguage => {
                let next = !self.language_open;
                self.close_settings_menus();
                self.language_open = next;
            }
            Message::ToggleCorners => {
                let next = !self.corners_open;
                self.close_settings_menus();
                self.corners_open = next;
            }
            Message::ToggleSkin => {
                let next = !self.skin_open;
                self.close_settings_menus();
                self.skin_open = next;
            }
            Message::TogglePanelIcon => {
                let next = !self.panel_icon_open;
                self.close_settings_menus();
                self.panel_icon_open = next;
            }
            Message::ToggleFontScale => {
                let next = !self.font_scale_open;
                self.close_settings_menus();
                self.font_scale_open = next;
            }
            Message::ToggleScheduleMode => {
                let next = !self.schedule_mode_open;
                self.close_settings_menus();
                self.schedule_mode_open = next;
            }
            Message::Unpair => {
                if self.confirm_unpair {
                    self.confirm_unpair = false;
                    self.send(json!({"cmd": "unpair"}));
                } else {
                    self.confirm_unpair = true;
                }
            }
            Message::OpenAddProvider(pre) => {
                let kind = pre.unwrap_or_else(|| "home_assistant".to_string());
                let (_, _, _, default_port, _, _) = provider_meta(&kind).unwrap_or(EXTRA_PROVIDERS[0]);
                self.provider_draft = Some(ProviderDraft {
                    id: String::new(),
                    kind: kind.clone(),
                    name: String::new(),
                    host: String::new(),
                    port: default_port.to_string(),
                    room_name: String::new(),
                    token: String::new(),
                    pair_id: String::new(),
                    use_https: kind == "dirigera",
                    has_saved_token: false,
                    has_cert_fingerprint: false,
                    original_kind: String::new(),
                    original_host: String::new(),
                    original_port: String::new(),
                    awaiting_save: false,
                });
                // E7: Clear stale provider status message when opening Add/Edit
                self.send(json!({"cmd": "clear_provider_action"}));
                self.send(json!({"cmd": "discover_providers"}));
            }
            Message::EditProvider(cid) => {
                if let Some(p) = list(&self.stats, "providers").iter().find(|v| s(v, "id") == cid) {
                    let pkind = s(p, "kind").to_string();
                    let phost = s(p, "host").to_string();
                    let pport = (num(p, "port") as u16).to_string();
                    self.provider_draft = Some(ProviderDraft {
                        id: cid,
                        kind: pkind.clone(),
                        name: s(p, "name").to_string(),
                        host: phost.clone(),
                        port: pport.clone(),
                        room_name: s(p, "room_name").to_string(),
                        token: String::new(),
                        pair_id: String::new(),
                        use_https: flag(p, "use_https"),
                        has_saved_token: flag(p, "has_token"),
                        has_cert_fingerprint: flag(p, "has_cert_fingerprint"),
                        original_kind: pkind,
                        original_host: phost,
                        original_port: pport,
                        awaiting_save: false,
                    });
                    self.send(json!({"cmd": "clear_provider_action"}));
                }
            }
            Message::CloseProviderDraft => {
                self.provider_draft = None;
            }
            Message::ProviderKind(k) => {
                if let Some(draft) = &mut self.provider_draft {
                    let (_, _, _, default_port, _, _) = provider_meta(&k).unwrap_or(EXTRA_PROVIDERS[0]);
                    draft.kind = k.clone();
                    draft.port = default_port.to_string();
                    draft.use_https = k == "dirigera";
                    draft.token.clear();
                    draft.pair_id.clear();
                    draft.has_saved_token = false;
                }
            }
            Message::ProviderName(v) => {
                if let Some(d) = &mut self.provider_draft { d.name = v; }
            }
            Message::ProviderHost(v) => {
                if let Some(d) = &mut self.provider_draft {
                    d.host = v;
                    d.pair_id.clear();
                }
            }
            Message::ProviderPort(v) => {
                if let Some(d) = &mut self.provider_draft {
                    d.port = v;
                    d.pair_id.clear();
                }
            }
            Message::ProviderRoomName(v) => {
                if let Some(d) = &mut self.provider_draft { d.room_name = v; }
            }
            Message::ProviderToken(v) => {
                if let Some(d) = &mut self.provider_draft { d.token = v; }
            }
            Message::ProviderHttps(v) => {
                if let Some(d) = &mut self.provider_draft { d.use_https = v; }
            }
            Message::ProviderUseDiscovered(k, n, h, p) => {
                if let Some(d) = &mut self.provider_draft {
                    d.kind = k.clone();
                    d.name = n;
                    d.host = h;
                    d.port = p.to_string();
                    d.use_https = k == "dirigera";
                    d.token.clear();
                    d.pair_id.clear();
                    d.has_saved_token = false;
                }
            }
            Message::DiscoverProviders => {
                self.send(json!({"cmd": "discover_providers"}));
            }
            Message::TestProvider => {
                if let Some(d) = &self.provider_draft {
                    let port_num: Option<u16> = d.port.trim().parse().ok();
                    self.send(json!({
                        "cmd": "test_provider",
                        "provider": {
                            "id": d.id,
                            "kind": d.kind,
                            "name": d.name.trim(),
                            "host": d.host.trim(),
                            "port": port_num,
                            "room_name": d.room_name.trim(),
                            "token": d.token.trim(),
                            "pair_id": d.pair_id,
                            "use_https": d.use_https,
                        }
                    }));
                }
            }
            Message::PairProvider => {
                if let Some(d) = &self.provider_draft {
                    let port_num: Option<u16> = d.port.trim().parse().ok();
                    self.send(json!({
                        "cmd": "pair_provider",
                        "kind": d.kind,
                        "host": d.host.trim(),
                        "port": port_num,
                    }));
                }
            }
            Message::SaveProvider => {
                if let Some(d) = &mut self.provider_draft {
                    let port_num: Option<u16> = d.port.trim().parse().ok();
                    d.awaiting_save = true;
                    let payload = json!({
                        "cmd": "save_provider",
                        "provider": {
                            "id": d.id,
                            "kind": d.kind,
                            "name": d.name.trim(),
                            "host": d.host.trim(),
                            "port": port_num,
                            "room_name": d.room_name.trim(),
                            "token": d.token.trim(),
                            "pair_id": d.pair_id,
                            "use_https": d.use_https,
                            "enabled": true,
                        }
                    });
                    self.send(payload);
                }
            }
            Message::ToggleProvider(cid, en) => {
                self.send(json!({"cmd": "toggle_provider", "id": cid, "enabled": en}));
            }
            Message::RemoveProvider(cid) => {
                if self.confirm_remove_provider.as_deref() == Some(cid.as_str()) {
                    self.confirm_remove_provider = None;
                    self.send(json!({"cmd": "remove_provider", "id": cid}));
                } else {
                    self.confirm_remove_provider = Some(cid);
                }
            }
            Message::CancelRemoveProvider => {
                self.confirm_remove_provider = None;
            }
            Message::TrustProviderCert(cid) => {
                if let Some(d) = &mut self.provider_draft {
                    d.has_cert_fingerprint = false;
                }
                self.send(json!({"cmd": "trust_provider_cert", "id": cid}));
            }
            Message::AddAllDiscoveredProviders => {
                let existing: HashSet<String> = list(&self.stats, "providers")
                    .iter()
                    .map(|p| format!("{}|{}|{}", s(p, "kind"), s(p, "host").to_lowercase(), num(p, "port") as u16))
                    .collect();
                let room_override = self.provider_draft.as_ref().map(|d| d.room_name.trim().to_string()).unwrap_or_default();
                let mut added_any = false;
                for dev in list(&self.stats, "discovered_providers") {
                    let dk = s(dev, "kind");
                    if provider_meta(dk).is_some_and(|m| m.4) {
                        continue;
                    }
                    let dh = s(dev, "host");
                    let dp = num(dev, "port") as u16;
                    let key = format!("{dk}|{}|{dp}", dh.to_lowercase());
                    if existing.contains(&key) {
                        continue;
                    }
                    self.send(json!({
                        "cmd": "save_provider",
                        "provider": {
                            "id": "",
                            "kind": dk,
                            "name": s(dev, "name"),
                            "host": dh,
                            "port": dp,
                            "room_name": room_override,
                            "token": "",
                            "pair_id": "",
                            "use_https": false,
                            "enabled": true,
                        }
                    }));
                    added_any = true;
                }
                if added_any {
                    self.provider_draft = None;
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        look::keep_fonts();
        if self.window_mode {
            return self.themed_window_content();
        }
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
        look::keep_fonts();
        if self.window_mode {
            return self.themed_window_content();
        }
        match self.cached_theme.clone() {
            Some(theme) => {
                let inner: Element<Message> = if skin_style(&self.skin).is_some() {
                    let bg = Color::from(theme.cosmic().bg_color());
                    let fg = Color::from(theme.cosmic().on_bg_color());
                    container(self.popup_content())
                        .class(theme::Container::custom(move |_| iced_container::Style {
                            background: Some(Background::Color(bg)),
                            text_color: Some(fg),
                            border: Border { radius: corners::now().radius.into(), ..Default::default() },
                            ..Default::default()
                        }))
                        .into()
                } else {
                    self.popup_content()
                };
                cosmic::iced::widget::themer(Some(theme), self.core.applet.popup_container(inner))
                    .text_color(|t: &cosmic::Theme| t.cosmic().on_bg_color().into())
                    .into()
            }
            None => self.core.applet.popup_container(self.popup_content()).into(),
        }
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
        assert_eq!(esc_step(true, true, true, true, true, true, true), EscStep::Note);
        assert_eq!(esc_step(true, false, true, true, true, true, true), EscStep::HelpOverview);
        assert_eq!(esc_step(true, false, false, true, true, true, true), EscStep::Confirm);
        assert_eq!(esc_step(true, false, false, false, true, true, true), EscStep::Menus);
        assert_eq!(esc_step(true, false, false, false, false, true, true), EscStep::Drawer);
        assert_eq!(esc_step(true, false, false, false, false, false, true), EscStep::Back);
        assert_eq!(esc_step(true, false, false, false, false, false, false), EscStep::Close);
        // Not from the window or the open popup (e.g. the panel button): nothing.
        assert_eq!(esc_step(false, true, true, true, true, true, true), EscStep::Nothing);
        assert_eq!(esc_step(false, false, false, false, false, false, false), EscStep::Nothing);
    }

    #[test]
    fn help_esc_steps_from_detail_to_overview_to_closed() {
        use cosmic::Application;
        let mut app = HueLights::<false>::default();
        let popup_id = cosmic::iced::window::Id::unique();
        app.popup = Some(popup_id);

        // Open Help -> select feature 2 (Visual Rules)
        let _ = app.update(Message::ToggleHelp);
        assert!(app.help_open);
        assert_eq!(app.help_selected, None);

        let _ = app.update(Message::HelpSelect(Some(2)));
        assert_eq!(app.help_selected, Some(2));

        // Out-of-bounds selection is rejected safely
        let _ = app.update(Message::HelpSelect(Some(999)));
        assert_eq!(app.help_selected, None);

        // Select feature 1, then press Esc -> goes back to Help overview
        let _ = app.update(Message::HelpSelect(Some(1)));
        assert_eq!(app.help_selected, Some(1));
        let _ = app.update(Message::Escape(popup_id));
        assert!(app.help_open);
        assert_eq!(app.help_selected, None);

        // Press Esc again -> closes Help completely
        let _ = app.update(Message::Escape(popup_id));
        assert!(!app.help_open);
        assert_eq!(app.help_selected, None);

        // Try-it-now jumps straight to the target mode and closes Help
        let _ = app.update(Message::ToggleHelp);
        let _ = app.update(Message::HelpSelect(Some(2)));
        let _ = app.update(Message::HelpTry(help::HelpAction::OpenMapRules));
        assert!(!app.help_open);
        assert_eq!(app.help_selected, None);
        assert_eq!(app.tab, Tab::Map);
        assert_eq!(app.floorplan.mode, CanvasMode::Wire);
    }

    #[test]
    fn lights_tab_submenu_modals_and_esc_flow() {
        use cosmic::Application;
        let mut app = HueLights::<false>::default();
        let popup_id = cosmic::iced::window::Id::unique();
        app.popup = Some(popup_id);
        app.stats = json!({
            "phase": "ok",
            "bridge": {"is_pro": true, "model_id": "BSB003"},
            "motion_areas": [],
            "rooms": [{
                "id": "r1",
                "kind": "room",
                "name": "Living Room",
                "lights": [{
                    "id": "l1",
                    "device_id": "d1",
                    "name": "Floor Lamp",
                    "on": true,
                    "bri": 75.0,
                    "color": "#ffd27a",
                    "reachable": true
                }]
            }]
        });
        app.floorplan.doc.lamps.insert(
            "l1".into(),
            FloorplanLamp { x: 100.0, y: 100.0, z: 0.0, icon: "bulb".into(), floor_id: "eg".into() },
        );

        // Toggle Lights tab `⋯` submenu open & closed
        let _ = app.update(Message::ToggleLightsMenu("l1".into()));
        assert_eq!(app.open_lights_menu.as_deref(), Some("l1"));
        assert!(app.any_picker_open());

        // Esc closes submenu cleanly without closing the popup
        let _ = app.update(Message::Escape(popup_id));
        assert_eq!(app.open_lights_menu, None);

        // Brightness modal opens from Lights submenu and closes on Esc
        let _ = app.update(Message::ToggleLightsMenu("l1".into()));
        let _ = app.update(Message::LightsMenuBrightness("l1".into()));
        assert_eq!(app.open_lights_menu, None);
        assert_eq!(app.floorplan.lamp_dialog, Some(LampDialog::Brightness("l1".into())));
        let _ = app.update(Message::Escape(popup_id));
        assert_eq!(app.floorplan.lamp_dialog, None);

        // Color & Warmth opens the light drawer
        let _ = app.update(Message::LightsMenuColor("l1".into()));
        assert_eq!(app.open_light_drawer.as_deref(), Some("l1"));

        // Rename modal pre-fills current lamp name
        let _ = app.update(Message::LightsMenuRename("l1".into()));
        assert_eq!(app.floorplan.lamp_dialog, Some(LampDialog::Rename("l1".into())));
        assert_eq!(app.floorplan.lamp_rename_draft, "Floor Lamp");
        let _ = app.update(Message::Escape(popup_id));
        assert_eq!(app.floorplan.lamp_dialog, None);

        // Lamp icon modal syncs 1:1 with floorplan
        let _ = app.update(Message::LightsMenuIcon("l1".into()));
        assert_eq!(app.floorplan.lamp_dialog, Some(LampDialog::Icon("l1".into())));
        let _ = app.update(Message::Map(MapMsg::LampSetIcon("l1".into(), "pendant".into())));
        assert_eq!(app.floorplan.doc.lamps["l1"].icon, "pendant");

        // Wire rule on Map jumps to Map tab in Rules mode with lamp armed
        let _ = app.update(Message::LightsMenuWireOnMap("l1".into()));
        assert_eq!(app.tab, Tab::Map);
        assert_eq!(app.floorplan.mode, CanvasMode::Wire);
        assert_eq!(app.floorplan.pending_wire_from, Some(("light".into(), "l1".into())));

        // MotionAware hint dismissal
        assert!(should_show_motion_aware_hint(&app.stats, app.motion_hint_dismissed));
        let _ = app.update(Message::DismissMotionHint);
        assert!(app.motion_hint_dismissed);
        assert!(!should_show_motion_aware_hint(&app.stats, app.motion_hint_dismissed));
    }

    #[test]
    fn ask_ai_defaults_off_and_parses_ai_state() {
        let app = HueLights::<false>::default();
        assert!(!app.ask_ai_enabled);
        assert!(!app.ask_ai_open);

        let empty = AiState::from_stats(&json!({}));
        assert_eq!(empty, AiState::default());

        let populated = AiState::from_stats(&json!({
            "ai": {
                "running": true,
                "last_prompt": "Turn off all lights",
                "last_reply": "Turned off all lights.",
                "last_actions": ["all_off"],
                "engine": "builtin",
                "error": ""
            }
        }));
        assert!(populated.busy);
        assert_eq!(populated.last_prompt, "Turn off all lights");
        assert_eq!(populated.last_reply, "Turned off all lights.");
        assert_eq!(populated.last_actions, vec!["all_off".to_string()]);
        assert_eq!(populated.engine, "builtin");
        assert_eq!(populated.error, "");
    }
}
