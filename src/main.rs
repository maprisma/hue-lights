// Hue Lights for COSMIC: Philips Hue rooms, zones, lights and scenes from the panel.
// Lit bulb in green while any light is on, dim bulb when all are off or no bridge
// is paired. Left click opens the popup; right click switches every light off.
//
// All bridge traffic goes through scripts/hue-lights.py (the same helper as the
// GNOME extension), which gets JSON commands on stdin and streams JSON snapshots
// on stdout (every 2 s while the popup is open, every 20 s while it is closed).
// It talks only to the bridge on the LAN, over pinned HTTPS; its app key stays
// in ~/.config/hue-lights/config.json and never reaches this process.

use maprisma_hue_lights::i18n::{tr, trf};
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
static AUTOSIZE_ID: LazyLock<Id> = LazyLock::new(|| Id::new("hue-lights-autosize"));
static BULB_ON: &[u8] = include_bytes!("../data/icons/io.github.ubruckhaus.CosmicMaprismaHueLights-symbolic.svg");
static BULB_OFF: &[u8] = include_bytes!("../data/icons/hue-bulb-off-symbolic.svg");

const RESTART_DELAY: Duration = Duration::from_secs(5); // before a helper that exited starts again
const SEND_EVERY: Duration = Duration::from_millis(150); // between brightness commands while dragging
const SCENE_LIMIT: usize = 6; // scenes shown before "+N more"
const POPUP_WIDTH: f32 = 430.0;
const ROOMS_MAX_HEIGHT: f32 = 540.0;
const LIT: Color = Color::from_rgb(0x33 as f32 / 255.0, 0xd1 as f32 / 255.0, 0x7a as f32 / 255.0);
const WARM: Color = Color::from_rgb(1.0, 0xd2 as f32 / 255.0, 0x7a as f32 / 255.0);

fn main() -> cosmic::iced::Result {
    if std::env::args().any(|arg| arg == "--window") {
        let runtime = tokio::runtime::Runtime::new().expect("window D-Bus runtime");
        let connection = runtime.block_on(async {
            let c = zbus::connection::Builder::session()?.serve_at("/Window", WindowControl)?.build().await?;
            c.request_name_with_flags(WINDOW_BUS, zbus::fdo::RequestNameFlags::DoNotQueue.into()).await?;
            Ok::<_, zbus::Error>(c)
        });
        let _connection = match connection { Ok(c) => c, Err(e) => { eprintln!("Window unavailable or already open: {e}"); return Ok(()); } };
        cosmic::app::run::<HueLights>(cosmic::app::Settings::default()
            .size(cosmic::iced::Size::new(POPUP_WIDTH, 650.0))
            .resizable(Some(8.0)).exit_on_close(true).is_daemon(false), true)
    } else {
        cosmic::applet::run::<HueLights>(false)
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
    text::caption(content).class(theme::Text::Color(Color::from_rgb8(0xe0, 0x1b, 0x24))).into()
}

fn tinted_svg(c: Color) -> theme::Svg {
    theme::Svg::custom(move |_| iced_svg::Style { color: Some(c) })
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
            border: Border { radius: 12.0.into(), width: 1.0, color: border },
            ..Default::default()
        }
    })
}

fn badge(tint: Option<Color>) -> theme::Container<'static> {
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let bg = tint.unwrap_or_else(|| with_alpha(Color::from(theme.cosmic().on_bg_color()), 0.1));
        iced_container::Style {
            background: Some(Background::Color(bg)),
            border: Border { radius: 16.0.into(), ..Default::default() },
            ..Default::default()
        }
    })
}

fn pill_style() -> theme::Container<'static> {
    theme::Container::custom(|theme: &cosmic::Theme| iced_container::Style {
        background: Some(Background::Color(with_alpha(Color::from(theme.cosmic().on_bg_color()), 0.12))),
        border: Border { radius: 10.0.into(), ..Default::default() },
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
                border: Border { radius: 6.0.into(), width: 1.0, color: with_alpha(base, 0.3) },
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
struct HueLights {
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
}

#[derive(Debug, Clone)]
enum Message {
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
}

impl HueLights {
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
            bulb = bulb.class(tinted_svg(LIT));
        } else {
            bulb = bulb.opacity(0.6);
        }
        bulb.into()
    }

    fn header(&self) -> Element<'_, Message> {
        let lit = self.on_count() > 0.0;
        let mut hero = icon::icon(bulb(lit)).size(32);
        hero = if lit { hero.class(tinted_svg(LIT)) } else { hero.opacity(0.6) };
        let meta = if self.paired() { bridge_text(&self.stats) } else { tr("No bridge paired") };
        let mut header = row![hero, column![text::title4(tr("Maprisma Hue Lights")), dim(meta)].spacing(2), space::horizontal()]
            .spacing(12)
            .align_y(Alignment::Center);
        if self.phase() == "ok" {
            let pill = trf("{0} of {1} on", &[self.on_count().to_string(), num(&self.stats, "light_count").to_string()]);
            header = header.push(container(text::caption(pill)).padding([2, 10]).class(pill_style()));
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
        let names = column![text::heading(room_name(room)), dim(room_status(room))].spacing(1);
        let expander = button::custom(
            row![badge, names, space::horizontal(), chevron].spacing(10).align_y(Alignment::Center),
        )
        .padding(4)
        .width(Length::Fill)
        .class(theme::Button::MenuItem)
        .on_press(Message::Expand(key.clone()));
        let switch_key = key.clone();
        let head = row![expander, toggler(lit).on_toggle(move |on| Message::RoomOn(switch_key.clone(), on))]
            .spacing(8)
            .align_y(Alignment::Center);
        let mut card_content = column![head].spacing(6);

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
                    card_content.push(container(flex_row(pills).row_spacing(6).column_spacing(6)).padding([4, 4, 0, 4]));
            }
        }
        container(card_content).padding(8).width(Length::Fill).class(card(tint)).into()
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
            .spacing(10)
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
        container(line).padding([2, 4, 2, 12]).into()
    }

    fn details(&self) -> Element<'_, Message> {
        let mut content = column![self.header()].spacing(10);
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
        let mut cards = column![].spacing(8);
        for room in rooms {
            cards = cards.push(self.room_card(room));
        }
        content = content.push(
            container(scrollable(container(cards).padding([0, 12, 0, 0])).height(Length::Shrink))
                .max_height(ROOMS_MAX_HEIGHT),
        );
        content.into()
    }

    fn popup_content(&self) -> Element<'_, Message> {
        let choices = maprisma_hue_lights::i18n::choices();
        let selected = choices.iter().position(|(code, _)| code == &maprisma_hue_lights::i18n::setting());
        let labels: Vec<String> = choices.iter().map(|(_, label)| label.clone()).collect();
        let mut language_content = column![].spacing(4);
        if self.language_error { language_content = language_content.push(error_text(tr("Could not save language setting."))); }
        let language = row![text::body(tr("Language")), cosmic::widget::dropdown(labels, selected, move |i| Message::Language(choices[i].0.clone()))].spacing(8);
        let content = column![padded_control(self.details()).padding([12, 16]), padded_control(language_content.push(language)), self.actions()]
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
                    .spacing(10)
                    .align_y(Alignment::Center)
            ),
        ]
        .into()
    }
}

impl cosmic::Application for HueLights {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = bool;
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(mut core: Core, window_mode: bool) -> (Self, Task<Message>) {
        if window_mode { core.set_header_title(TITLE.into()); }
        let title = if window_mode { core.set_title(core.main_window_id(), TITLE.into()) } else { Task::none() };
        (Self { core, window_mode, ..Default::default() }, title)
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::CloseRequested(id))
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
        Subscription::batch(vec![close, rectangles,
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
            Message::Quit => return cosmic::iced::exit(),
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
            }
            Message::Rectangle(update) => match update {
                RectangleUpdate::Rectangle(r) => self.rectangle = r.1,
                RectangleUpdate::Init(tracker) => self.tracker = Some(tracker),
            },
            Message::CloseRequested(id) => {
                if self.window_mode { return cosmic::iced::exit(); }
                if self.popup == Some(id) {
                    self.popup = None;
                    self.confirm_unpair = false;
                    self.send(json!({"cmd": "closed"}));
                }
            }
            Message::TogglePopup => {
                if self.window_mode { return Task::none(); }
                if let Some(popup) = self.popup.take() {
                    self.confirm_unpair = false;
                    self.send(json!({"cmd": "closed"}));
                    return destroy_popup(popup);
                }
                self.send(json!({"cmd": "open"}));
                return cosmic::surface::surface_task(cosmic::surface::action::app_popup(
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
                ));
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
        if self.window_mode { return self.popup_content(); }
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
        if self.window_mode { return self.popup_content(); }
        self.core.applet.popup_container(self.popup_content()).into()
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
