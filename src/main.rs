// Hue Lights for COSMIC: Philips Hue rooms, zones, lights and scenes from the panel.
// Lit bulb in the accent while any light is on, dim bulb when all are off or no bridge
// is paired. Left click opens the popup; right click switches every light off.
//
// All bridge traffic goes through scripts/hue-lights.py (the same helper as the
// GNOME extension), which gets JSON commands on stdin and streams JSON snapshots
// on stdout (every 2 s while the popup is open, every 20 s while it is closed).
// It talks only to the bridge on the LAN, over pinned HTTPS; its app key stays
// in ~/.config/hue-lights/config.json and never reaches this process.

use maprisma_hue_lights::i18n::{tr, trf, read_pref, write_pref};
use maprisma_hue_lights::{corners, shot};
mod window_size;
use std::collections::{HashMap, HashSet};
use maprisma_hue_lights::{script_path, TITLE, WINDOW_BUS};
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
    Id, autosize, button, column, container, divider, flex_row, icon, mouse_area, row, scrollable, slider, space,
    text, text_input, toggler,
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
        cosmic::app::run::<HueLights<true>>(cosmic::app::Settings::default()
            .size({ let (width, height) = window_start_size(); cosmic::iced::Size::new(width, height) })
            .resizable(Some(8.0)).exit_on_close(true).is_daemon(false), true)
    } else {
        cosmic::applet::run::<HueLights>(false)
    }
}

// The `--window` size: the one the user left it at (layout rule 5a), else the popup's.
fn window_start_size() -> window_size::Size {
    window_size::parse(read_pref(window_size::KEY).as_ref()).unwrap_or((POPUP_WIDTH, 650.0))
}

/// What Esc does (layout rule 5a): the innermost open thing first, then the window or popup.
/// (An open Language / Corners drop-down closes itself and keeps that Esc.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscStep {
    /// Skip the screenshot's note (or close its error).
    Note,
    /// Drop the "Unpair?" question.
    Confirm,
    /// Close the window (`--window`) or the popup.
    Close,
    Nothing,
}

/// `shown`: Esc came from the window or the open popup (not the panel button).
fn esc_step(shown: bool, note: bool, confirm: bool) -> EscStep {
    if !shown {
        EscStep::Nothing
    } else if note {
        EscStep::Note
    } else if confirm {
        EscStep::Confirm
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

fn room_status(room: &Value) -> String {
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

fn swatch<'a>(c: Option<Color>, reachable: bool) -> Element<'a, Message> {
    container(space::horizontal())
        .width(Length::Fixed(12.0))
        .height(Length::Fixed(12.0))
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
    popup: Option<window::Id>,
    rectangle: Rectangle,
    tracker: Option<RectangleTracker<u32>>,
    helper: Option<mpsc::UnboundedSender<String>>,
    stats: Value,
    expanded: HashSet<String>,
    all_scenes: HashSet<String>,
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
    Expand(String),
    AllScenes(String),
    RoomOn(String, bool),
    LightOn(String, bool),
    // "room:<key>" or "light:<id>", the value
    Bri(String, u8),
    BriDone(String),
    Scene(String),
    AllOff,
    Refresh,
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
    Corners(String),
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

// Settings › F12 saves a screenshot (on unless switched off) and Corners, from config.json.
fn shot_key_setting() -> bool {
    read_pref("screenshot_key").and_then(|v| v.as_bool()).unwrap_or(true)
}

fn corners_setting() -> String {
    corners::normalize(read_pref("corners").as_ref().and_then(Value::as_str).unwrap_or("")).to_string()
}

impl<const WINDOW_MODE: bool> HueLights<WINDOW_MODE> {
    fn send(&self, command: Value) {
        if let Some(helper) = &self.helper {
            let _ = helper.unbounded_send(command.to_string());
        }
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
        header.into()
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

    fn room_card<'a>(&'a self, room: &'a Value) -> Element<'a, Message> {
        let key = room_key(room);
        let lit = flag(room, "on");
        let tint = lit.then(|| color(room.get("color").unwrap_or(&Value::Null)));
        let open = self.expanded.contains(&key);

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
        let names = column![text::heading(room_name(room)), dim(room_status(room))].spacing(4);
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
                    let label = if all { tr("less").to_string() } else { trf("+{0} more", &[(scenes.len() - shown).to_string()]) };
                    pills.push(button::text(label).on_press(Message::AllScenes(key.clone())).into());
                }
                card_content =
                    card_content.push(container(flex_row(pills).row_spacing(8).column_spacing(8)).padding([4, 4, 0, 4]));
            }
        }
        container(card_content).padding(12).width(Length::Fill).class(card(tint)).into()
    }

    fn light_row<'a>(&'a self, light: &'a Value) -> Element<'a, Message> {
        let id = s(light, "id").to_string();
        let on = flag(light, "on");
        let reachable = light.get("reachable").and_then(Value::as_bool) != Some(false);
        let c = color(light.get("color").unwrap_or(&Value::Null));
        let mut name = s(light, "name").to_string();
        if !reachable {
            name += &format!(" · {}", tr("unreachable"));
        }
        let name: Element<Message> = if on && reachable { text::body(name).into() } else { dim(name) };
        let mut line = row![swatch(on.then_some(c), reachable), container(name).width(Length::Fill)]
            .spacing(8)
            .align_y(Alignment::Center);
        if flag(light, "dimmable") {
            let target = format!("light:{id}");
            let bri = if on { self.bri_of(&target, num(light, "bri")) } else { 1 };
            let t1 = target.clone();
            line = line.push(
                slider(0..=100, bri, move |v| Message::Bri(t1.clone(), v))
                    .step(5u8)
                    .on_release(Message::BriDone(target))
                    .width(Length::Fixed(110.0)),
            );
        }
        let switch_id = id.clone();
        line = line.push(toggler(on).on_toggle(move |v| Message::LightOn(switch_id.clone(), v)));
        container(line).padding([4, 4, 4, 12]).into()
    }

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
        content = content.push(divider::horizontal::default());
        let rooms = list(&self.stats, "rooms");
        if rooms.is_empty() {
            content = content.push(dim(if self.phase() == "ok" {
                tr("No lights on this bridge yet.")
            } else {
                tr("Connecting to the bridge …")
            }));
            return content.into();
        }
        let mut cards = column![].spacing(12);
        for room in rooms {
            cards = cards.push(self.room_card(room));
        }
        content = content.push(
            container(scrollable(container(cards).padding([0, 12, 0, 0])).height(Length::Shrink))
                .max_height(ROOMS_MAX_HEIGHT),
        );
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

    fn popup_content(&self) -> Element<'_, Message> {
        if let Some(overlay) = self.shot_overlay() {
            return overlay;
        }
        let choices = maprisma_hue_lights::i18n::choices();
        let selected = choices.iter().position(|(code, _)| code == &maprisma_hue_lights::i18n::setting());
        let labels: Vec<String> = choices.iter().map(|(_, label)| label.clone()).collect();
        let mut language_content = column![].spacing(4);
        if self.language_error { language_content = language_content.push(error_text(tr("Could not save language setting."))); }
        let language = row![text::body(tr("Language")), cosmic::widget::dropdown(labels, selected, move |i| Message::Language(choices[i].0.clone()))].spacing(8).align_y(Alignment::Center);
        let screenshot = row![
            text::body(tr("F12 saves a screenshot")),
            space::horizontal().width(Length::Fill),
            toggler(self.shot_key).on_toggle(Message::SetShotKey),
        ]
        .align_y(Alignment::Center);
        let corner_labels = vec![
            trf("Like the desktop ({0} px)", &[corners::desktop_radius().round().to_string()]),
            trf("Slightly rounded ({0} px)", &["6".into()]),
            trf("Rounded ({0} px)", &["12".into()]),
            tr("Square"),
        ];
        let corner_selected = corners::CHOICES.iter().position(|c| *c == self.corners);
        let corners_row = row![
            text::body(tr("Corners")),
            cosmic::widget::dropdown(corner_labels, corner_selected, |i| Message::Corners(corners::CHOICES[i].into())),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        language_content = language_content.push(language).push(screenshot).push(corners_row);
        if self.setting_error { language_content = language_content.push(error_text(tr("Could not save the setting."))); }
        let content = column![padded_control(self.details()).padding([12, 16]), padded_control(language_content), self.actions()]
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
        column![
            divider::horizontal::default(),
            menu_button(text::body(tr("All off"))).on_press_maybe((self.on_count() > 0.0).then_some(Message::AllOff)),
            menu_button(text::body(tr("Refresh"))).on_press(Message::Refresh),
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
        if window_mode { core.set_header_title(TITLE.into()); }
        let mut app = Self { core, window_mode, shot_key: shot_key_setting(), corners: corners_setting(), ..Default::default() };
        let title = if window_mode { app.name_window() } else { Task::none() };
        if window_mode { app.resize = window_size::Debounce::new(window_start_size()); }
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
                }
                self.stats = stats;
                // Settings changed in another interface (window, applet) take effect here too.
                self.shot_key = shot_key_setting();
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
                    self.send(json!({"cmd": "closed"}));
                    return self.popup_closed();
                }
            }
            Message::TogglePopup => {
                if self.window_mode { return Task::none(); }
                if let Some(popup) = self.popup.take() {
                    self.confirm_unpair = false;
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
            Message::AllOff => {
                if self.on_count() > 0.0 {
                    self.send(json!({"cmd": "all_off"}));
                }
            }
            Message::Refresh => self.send(json!({"cmd": "refresh"})),
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
                match esc_step(shown, self.shot.open(), self.confirm_unpair) {
                    EscStep::Note => return self.finish_note(false),
                    EscStep::Confirm => self.confirm_unpair = false,
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
            Message::Corners(choice) => {
                let choice = corners::normalize(&choice);
                // "Like the desktop" is the default: no key in config.json.
                let value = (choice != "desktop").then(|| Value::from(choice));
                self.setting_error = write_pref("corners", value).is_err();
                self.corners = choice.to_string();
                corners::set_current(choice);
                if self.window_mode { return corners::set_app_theme(choice); }
            }
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
        assert_eq!(esc_step(true, true, true), EscStep::Note);
        assert_eq!(esc_step(true, false, true), EscStep::Confirm);
        assert_eq!(esc_step(true, false, false), EscStep::Close);
        // Not from the window or the open popup (e.g. the panel button): nothing.
        assert_eq!(esc_step(false, true, true), EscStep::Nothing);
        assert_eq!(esc_step(false, false, false), EscStep::Nothing);
    }
}
