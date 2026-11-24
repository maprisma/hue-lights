//! Help & Feature Guide page for Maprisma Hue Lights.
//!
//! Renders an App-Store-style feature tour listing the 7 major capabilities of the
//! app, each paired with a live, smooth-looping pure-vector animated mini-canvas
//! preview (`120×84 px`) driven by `phase: f32` in `0.0..1.0`.

use std::borrow::Cow;
use std::f32::consts::{PI, TAU};

use cosmic::applet::padded_control;
use cosmic::iced::mouse;
use cosmic::iced::widget::canvas::{
    Frame, Geometry, LineCap, LineDash, LineJoin, Path, Program, Stroke, Style,
};
use cosmic::iced::widget::{container as iced_container, text as iced_text};
use cosmic::iced::{Alignment, Background, Border, Color, Length, Point, Rectangle, Size, Vector};
use cosmic::widget::{button, column, container, divider, icon, row, scrollable, space, text};
use cosmic::{Element, theme};
use maprisma_hue_lights::corners;
use maprisma_hue_lights::i18n::tr;

use crate::model::skin_style;

const POPUP_WIDTH: f32 = 430.0;
const POPUP_LIST_MAX_HEIGHT: f32 = 560.0;
const CANVAS_W: f32 = 120.0;
const CANVAS_H: f32 = 84.0;
const FRAME_W: f32 = 124.0;
const FRAME_H: f32 = 84.0;
const DETAIL_CANVAS_WIN_W: f32 = 320.0;
const DETAIL_CANVAS_WIN_H: f32 = 196.0;
const DETAIL_CANVAS_POPUP_W: f32 = 360.0;
const DETAIL_CANVAS_POPUP_H: f32 = 180.0;

// ------------------------------------------------------------------------- Public Page View

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpAction {
    OpenLights,
    OpenMap,
    OpenMapRules,
    OpenSchedules,
    OpenSynth,
    OpenSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnimKind {
    Lights,
    Floorplan,
    Rules,
    MotionAware,
    Schedules,
    Synth,
    Skins,
    AskAi,
    McpServer,
}

struct FeatureSpec {
    kind: AnimKind,
    title: &'static str,
    desc: &'static str,
    summary: &'static str,
    steps: &'static [&'static str],
    tip: &'static str,
    try_action: Option<HelpAction>,
}

const FEATURES: [FeatureSpec; 9] = [
    FeatureSpec {
        kind: AnimKind::Lights,
        title: "Lights, Color & Themes",
        desc: "Control rooms, zones, and individual lamps with live brightness sliders, Kelvin warmth presets, native Hue effects, and per-lamp Light Themes.",
        summary: "Everything you need for daily lighting is right on the Lights tab. Switch whole rooms or fine-tune any single lamp with one click.",
        steps: &[
            "Toggle a room switch on the right or drag its slider to dim every lamp in that room together.",
            "Click a room card to expand its individual lamps, scenes, and quick buttons.",
            "Click a lamp's icon badge (or choose Color & Warmth from its ⋯ menu) to pick Kelvin warmth, colors, Hue effects, or Light Themes.",
            "Use the ⋯ button on any lamp to rename it, change its icon, pin it as a favorite, or wire a rule on the Map.",
        ],
        tip: "Pin your favorite rooms or lamps so they stay in a 1-click strip at the very top of the Lights tab.",
        try_action: Some(HelpAction::OpenLights),
    },
    FeatureSpec {
        kind: AnimKind::Floorplan,
        title: "Visual Floorplan Map",
        desc: "Arrange your rooms, furniture, and 12 custom lamp icons on a 2D map of your home. Hover any lamp to see its name, right-click to toggle, or use the sidebar menu for quick dialogs.",
        summary: "Build a live, interactive 2D map of your home where every lamp glows in its real color and brightness — and sync room changes back to your Hue Bridge.",
        steps: &[
            "Open the Map tab and pick a starting layout from the template menu (Auto from Hue Bridge places your existing rooms automatically).",
            "Click + Add to add rooms, zones, or furniture stamps like sofas, beds, desks, and doors — then drag corners to resize rooms.",
            "Drag lamps from the Lamps sidebar onto your rooms, or click ⋯ next to a lamp to change its 12 custom icons, brightness, or name.",
            "Hover any lamp to see its name and brightness, click to select, or right-click directly on the canvas to switch it on or off.",
        ],
        tip: "Under View you can upload a photo or sketch of your real floorplan as a faint background image to trace your rooms over.",
        try_action: Some(HelpAction::OpenMap),
    },
    FeatureSpec {
        kind: AnimKind::Rules,
        title: "Visual Rules",
        desc: "Switch to the Map's Rules tab and draw a line between lamps or sensors to link them. Click any line to edit Follow, Mirror, Exclusive, or Companion rules — or delete it.",
        summary: "Automate how lamps and sensors react to each other by simply drawing a glowing line on your map — no code or complex menus required.",
        steps: &[
            "Open the Map tab and click the Rules button in the top-left toolbar to enter wiring mode.",
            "Drag from any trigger lamp or sensor to a target lamp (or pick Wire rule on Map from a lamp's ⋯ menu, then click the target lamp).",
            "Choose a behavior in the popup dialog: Follow (turn on together), Mirror (sync on & off), Exclusive (turn the other off), or Companion (only if off).",
            "Click any colored wire on the canvas anytime to adjust time windows, toggle it on/off, or delete it.",
        ],
        tip: "Use Exclusive rules between bright ceiling lights and cozy mood lamps so turning one on automatically switches the other off.",
        try_action: Some(HelpAction::OpenMapRules),
    },
    FeatureSpec {
        kind: AnimKind::MotionAware,
        title: "Bridge Pro MotionAware & Sensors",
        desc: "See live presence badges on Hue Bridge Pro MotionAware lamps and place physical Hue motion sensors, contact sensors, and switches on your map to trigger automations.",
        summary: "Turn Hue Bridge Pro lights into radar-like motion areas and bring your physical Hue motion sensors, door contacts, and dimmer switches onto the map.",
        steps: &[
            "On a Hue Bridge Pro, open the Philips Hue mobile app → Settings → MotionAware, select 3+ lights in a room, and run the 30-second calibration once.",
            "Return to Maprisma Hue Lights — live green presence badges appear automatically on MotionAware rooms and lamps.",
            "Click a MotionAware room on the Map to toggle detection on/off or adjust its radar sensitivity slider.",
            "Place physical Hue motion sensors, contact sensors, or switches from the Map sidebar onto the canvas and wire them to lamps in Rules mode.",
        ],
        tip: "Sensor and MotionAware rules can include an Active between time window (like 21:00–06:00) so hallway lights only react at night.",
        try_action: Some(HelpAction::OpenMap),
    },
    FeatureSpec {
        kind: AnimKind::Schedules,
        title: "Schedules",
        desc: "Run weekday or sunrise/sunset schedules directly on your Hue Bridge 24/7, or run live PC schedules — including 1-click room scene scheduling from the map.",
        summary: "Wake up to a gentle fade-in or automatically switch lights on at sunset, running either directly on your Hue Bridge 24/7 or live on this PC.",
        steps: &[
            "Open the Schedules tab and click + New schedule.",
            "Pick a room, zone, or single light as the target, then choose a clock time or Sunrise / Sunset with an optional offset.",
            "Select the weekdays, choose On (with brightness & optional fade-in), Off, or Scene, and click Save.",
        ],
        tip: "Click any room on the Map and press the clock button next to a scene to create a 1-click scheduled scene.",
        try_action: Some(HelpAction::OpenSchedules),
    },
    FeatureSpec {
        kind: AnimKind::Synth,
        title: "Audio Light Synth",
        desc: "Turn system audio or your microphone into real-time light shows with 3-band Bass, Mids, and Treble beat detection — analyzed 100% locally in memory.",
        summary: "Sync your Hue lights to music, games, or movies in real time using local 3-band frequency analysis — nothing is ever recorded or sent over the network.",
        steps: &[
            "Open the Synth tab and pick an audio source: System audio, Microphone, or Demo beat to test right away.",
            "Choose a reaction Mode (Club, Chill, Ambient, or Strobe), a color Palette, and the Target room.",
            "Press Start and optionally assign individual lamps to Bass, Mids, or Treble for a multi-layered light show.",
        ],
        tip: "Turn up Smoothness for relaxing ambient color waves, or lower it in Club mode for punchy beat flashes.",
        try_action: Some(HelpAction::OpenSynth),
    },
    FeatureSpec {
        kind: AnimKind::Skins,
        title: "Skins, Icons & Shortcuts",
        desc: "Personalize the whole app with 6 skins, 8 panel icons, custom corners, and 8 languages. Press F12 anytime to save a screenshot, or Esc to close dialogs.",
        summary: "Match Maprisma Hue Lights to your desktop setup with custom skins, panel icons, corner rounding, text scaling, and handy keyboard shortcuts.",
        steps: &[
            "Click the gear icon in the top-right header to open Settings.",
            "Pick an App skin (Native, Synthwave, Cozy, Terminal, Disco, or Zen), a Panel icon, corner style, and language.",
            "Use keyboard shortcuts anywhere: press Esc to step back or close dialogs, and F12 to save a clean screenshot.",
        ],
        tip: "Right-click the Hue panel icon in your top bar anytime to switch every light in your home off immediately.",
        try_action: Some(HelpAction::OpenSettings),
    },
    FeatureSpec {
        kind: AnimKind::AskAi,
        title: "Ask AI (Optional)",
        desc: "Off by default — enable it in Settings if you want natural-language control for lamps, rooms, themes, scenes, rules, and schedules. Works 100% locally with optional local Ollama support.",
        summary: "Control your lighting by typing plain sentences in English or German. Ask AI is completely optional, off by default, and runs locally on your PC.",
        steps: &[
            "Open Settings and turn on the Ask AI toggle — a sparkle button appears in the top header.",
            "Click the sparkle button and type what you want, like \"Dim living room to 40%\" or \"Warm reading light on desk\".",
            "Review the executed actions right below your prompt, or click an example chip to try common commands.",
        ],
        tip: "Works immediately with the built-in local parser, and automatically upgrades if you run a local Ollama model on localhost.",
        try_action: Some(HelpAction::OpenSettings),
    },
    FeatureSpec {
        kind: AnimKind::McpServer,
        title: "MCP Server for AI Agents",
        desc: "Use your Hue Bridge from external AI coding and desktop agents via the built-in local stdio MCP server (maprisma-hue-lights --mcp or maprisma-hue-mcp). Credentials stay private.",
        summary: "Connect external AI assistants and coding agents to your Philips Hue Bridge over local stdio MCP while keeping your bridge key private on this machine.",
        steps: &[
            "Pair your Hue Bridge in Maprisma Hue Lights once so your local configuration is saved.",
            "In your AI agent's MCP server configuration, register the command \"maprisma-hue-lights\" with argument \"--mcp\" (or \"maprisma-hue-mcp\").",
            "Restart your AI client to expose safe tools for reading lights, switching rooms, recalling scenes, and managing schedules.",
        ],
        tip: "Your Hue application key stays inside the local helper process and is never revealed to the AI model.",
        try_action: None,
    },
];

#[cfg(test)]
pub const FEATURE_COUNT: usize = FEATURES.len();

/// Validates a selected feature index against the available help cards.
pub fn resolve_selected_feature(selected: Option<usize>) -> Option<usize> {
    selected.filter(|&idx| idx < FEATURES.len())
}

/// Renders the interactive Help & Feature Guide page:
/// - Overview (`selected == None`): 9 clickable animated feature cards.
/// - Detail (`selected == Some(idx)`): larger animation + beginner step-by-step guide + Quick Tip + Try it now.
pub fn view_help_page<'a, M: Clone + 'static>(
    phase: f32,
    skin: &'a str,
    window_mode: bool,
    selected: Option<usize>,
    on_back: M,
    on_select: impl Fn(Option<usize>) -> M + 'a,
    on_try: impl Fn(HelpAction) -> M + 'a,
) -> Element<'a, M> {
    let norm_phase = wrap_phase(phase);

    if let Some(idx) = resolve_selected_feature(selected) {
        return view_help_detail(
            &FEATURES[idx],
            norm_phase,
            skin,
            window_mode,
            on_select(None),
            on_try,
        );
    }

    let header = row![
        button::text(tr("Back"))
            .leading_icon(icon::from_name("go-previous-symbolic").size(16))
            .on_press(on_back),
        space::horizontal().width(Length::Fill),
        dim(tr("Help").to_uppercase()),
    ]
    .align_y(Alignment::Center);

    let intro = padded_control(dim(tr("Click any card for a bigger visual guide.")));

    let mut cards = column![].spacing(10).width(Length::Fill);
    for (idx, feat) in FEATURES.iter().enumerate() {
        let preview = container(
            cosmic::widget::canvas(HelpAnimProgram { kind: feat.kind, phase: norm_phase })
                .width(Length::Fixed(CANVAS_W))
                .height(Length::Fixed(CANVAS_H)),
        )
        .width(Length::Fixed(FRAME_W))
        .height(Length::Fixed(FRAME_H))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .class(preview_frame_style(skin));

        let hint = text::caption(tr("Tap for step-by-step guide ›")).class(theme::Text::Custom(
            |theme: &cosmic::Theme| iced_text::Style {
                color: Some(Color::from(theme.cosmic().accent_color())),
                ..Default::default()
            },
        ));

        let copy = column![
            text::heading(tr(feat.title)),
            text::body(tr(feat.desc)),
            hint,
        ]
        .spacing(4)
        .width(Length::Fill);

        let chevron = text::title4("›").class(theme::Text::Custom(|theme: &cosmic::Theme| {
            iced_text::Style {
                color: Some(with_alpha(Color::from(theme.cosmic().on_bg_color()), 0.48)),
                ..Default::default()
            }
        }));

        let card_inner = row![preview, copy, chevron]
            .spacing(12)
            .align_y(Alignment::Center)
            .width(Length::Fill);

        let card = button::custom(card_inner)
            .padding(10)
            .width(Length::Fill)
            .class(help_card_button_style(skin))
            .on_press(on_select(Some(idx)));

        cards = cards.push(card);
    }

    let cards_padded = container(cards).padding([4, 16, 12, 16]).width(Length::Fill);

    let list_area: Element<'a, M> = if window_mode {
        scrollable(cards_padded)
            .height(Length::Fill)
            .width(Length::Fill)
            .into()
    } else {
        container(
            scrollable(cards_padded)
                .height(Length::Shrink)
                .width(Length::Fill),
        )
        .max_height(POPUP_LIST_MAX_HEIGHT)
        .width(Length::Fill)
        .into()
    };

    column![
        padded_control(header),
        divider::horizontal::default(),
        intro,
        list_area,
    ]
    .width(if window_mode { Length::Fill } else { Length::Fixed(POPUP_WIDTH) })
    .height(if window_mode { Length::Fill } else { Length::Shrink })
    .into()
}

fn view_help_detail<'a, M: Clone + 'static>(
    feat: &FeatureSpec,
    norm_phase: f32,
    skin: &'a str,
    window_mode: bool,
    on_all_features: M,
    on_try: impl Fn(HelpAction) -> M + 'a,
) -> Element<'a, M> {
    let header = row![
        button::text(tr("All features"))
            .leading_icon(icon::from_name("go-previous-symbolic").size(16))
            .on_press(on_all_features),
        space::horizontal().width(Length::Fill),
        dim(tr("Help").to_uppercase()),
    ]
    .align_y(Alignment::Center);

    let (cw, ch) = if window_mode {
        (DETAIL_CANVAS_WIN_W, DETAIL_CANVAS_WIN_H)
    } else {
        (DETAIL_CANVAS_POPUP_W, DETAIL_CANVAS_POPUP_H)
    };

    let big_preview = container(
        container(
            cosmic::widget::canvas(HelpAnimProgram { kind: feat.kind, phase: norm_phase })
                .width(Length::Fixed(cw))
                .height(Length::Fixed(ch)),
        )
        .padding(8)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .class(preview_frame_style(skin)),
    )
    .width(Length::Fill)
    .align_x(Alignment::Center);

    let mut steps_col = column![text::heading(tr("How to do it"))].spacing(10);
    for (i, step_text) in feat.steps.iter().enumerate() {
        let num_badge = container(
            text::caption((i + 1).to_string()).class(theme::Text::Custom(
                |theme: &cosmic::Theme| iced_text::Style {
                    color: Some(Color::from(theme.cosmic().accent_color())),
                    ..Default::default()
                },
            )),
        )
        .width(Length::Fixed(22.0))
        .height(Length::Fixed(22.0))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .class(step_badge_style(skin));

        steps_col = steps_col.push(
            row![num_badge, text::body(tr(step_text))]
                .spacing(10)
                .align_y(Alignment::Start),
        );
    }

    let steps_card = container(steps_col)
        .padding(12)
        .width(Length::Fill)
        .class(help_card_style(skin));

    let tip_card = container(
        column![
            row![
                text::caption("💡"),
                text::heading(tr("Quick Tip")),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            text::body(tr(feat.tip)),
        ]
        .spacing(6),
    )
    .padding(12)
    .width(Length::Fill)
    .class(tip_card_style(skin));

    let mut detail_body = column![
        text::title4(tr(feat.title)),
        big_preview,
        text::body(tr(feat.summary)),
        steps_card,
        tip_card,
    ]
    .spacing(12)
    .width(Length::Fill);

    if let Some(action) = feat.try_action {
        detail_body = detail_body.push(
            row![
                space::horizontal(),
                button::suggested(tr("Try it now →")).on_press(on_try(action)),
            ]
            .align_y(Alignment::Center),
        );
    }

    let padded = container(detail_body).padding([10, 16, 16, 16]).width(Length::Fill);

    let scroll_area: Element<'a, M> = if window_mode {
        scrollable(padded)
            .height(Length::Fill)
            .width(Length::Fill)
            .into()
    } else {
        container(scrollable(padded).height(Length::Shrink).width(Length::Fill))
            .max_height(POPUP_LIST_MAX_HEIGHT)
            .width(Length::Fill)
            .into()
    };

    column![
        padded_control(header),
        divider::horizontal::default(),
        scroll_area,
    ]
    .width(if window_mode { Length::Fill } else { Length::Fixed(POPUP_WIDTH) })
    .height(if window_mode { Length::Fill } else { Length::Shrink })
    .into()
}

// ------------------------------------------------------------------------- Styles

fn dim<'a, M: 'a>(content: impl Into<Cow<'a, str>> + 'a) -> Element<'a, M> {
    text::caption(content)
        .class(theme::Text::Custom(|theme: &cosmic::Theme| iced_text::Style {
            color: Some(with_alpha(Color::from(theme.cosmic().on_bg_color()), 0.62)),
            ..Default::default()
        }))
        .into()
}

fn help_card_style(skin: &str) -> theme::Container<'static> {
    let skin_st = skin_style(skin);
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let base = Color::from(theme.cosmic().on_bg_color());
        let (bg, border, width) = match skin_st {
            Some(st) => (
                with_alpha(st.bg_tint, 0.26),
                with_alpha(st.border_tint, 0.50),
                st.border_width,
            ),
            None => (with_alpha(base, 0.045), with_alpha(base, 0.12), 1.0),
        };
        iced_container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                radius: corners::now().control.into(),
                width,
                color: border,
            },
            ..Default::default()
        }
    })
}

fn help_card_button_style(skin: &str) -> theme::Button {
    let skin_st = skin_style(skin);
    theme::Button::Custom {
        active: Box::new(move |_focused, theme: &cosmic::Theme| {
            let base = Color::from(theme.cosmic().on_bg_color());
            let (bg, border, width) = match skin_st {
                Some(st) => (
                    with_alpha(st.bg_tint, 0.26),
                    with_alpha(st.border_tint, 0.50),
                    st.border_width,
                ),
                None => (with_alpha(base, 0.045), with_alpha(base, 0.12), 1.0),
            };
            cosmic::widget::button::Style {
                background: Some(Background::Color(bg)),
                border_radius: corners::now().control.into(),
                border_width: width,
                border_color: border,
                text_color: Some(base),
                ..Default::default()
            }
        }),
        disabled: Box::new(move |theme: &cosmic::Theme| {
            let base = Color::from(theme.cosmic().on_bg_color());
            cosmic::widget::button::Style {
                background: Some(Background::Color(with_alpha(base, 0.03))),
                border_radius: corners::now().control.into(),
                border_width: 1.0,
                border_color: with_alpha(base, 0.08),
                text_color: Some(with_alpha(base, 0.5)),
                ..Default::default()
            }
        }),
        hovered: Box::new(move |_focused, theme: &cosmic::Theme| {
            let base = Color::from(theme.cosmic().on_bg_color());
            let accent = Color::from(theme.cosmic().accent_color());
            let (bg, border, width) = match skin_st {
                Some(st) => (
                    with_alpha(st.bg_tint, 0.42),
                    with_alpha(st.accent, 0.78),
                    st.border_width.max(1.25),
                ),
                None => (with_alpha(accent, 0.10), with_alpha(accent, 0.55), 1.25),
            };
            cosmic::widget::button::Style {
                background: Some(Background::Color(bg)),
                border_radius: corners::now().control.into(),
                border_width: width,
                border_color: border,
                text_color: Some(base),
                ..Default::default()
            }
        }),
        pressed: Box::new(move |_focused, theme: &cosmic::Theme| {
            let base = Color::from(theme.cosmic().on_bg_color());
            let accent = Color::from(theme.cosmic().accent_color());
            let (bg, border, width) = match skin_st {
                Some(st) => (
                    with_alpha(st.bg_tint, 0.52),
                    st.accent,
                    st.border_width.max(1.5),
                ),
                None => (with_alpha(accent, 0.16), accent, 1.5),
            };
            cosmic::widget::button::Style {
                background: Some(Background::Color(bg)),
                border_radius: corners::now().control.into(),
                border_width: width,
                border_color: border,
                text_color: Some(base),
                ..Default::default()
            }
        }),
    }
}

fn step_badge_style(skin: &str) -> theme::Container<'static> {
    let skin_st = skin_style(skin);
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let accent = skin_st
            .map(|st| st.accent)
            .unwrap_or_else(|| Color::from(theme.cosmic().accent_color()));
        iced_container::Style {
            background: Some(Background::Color(with_alpha(accent, 0.16))),
            border: Border {
                radius: 11.0.into(),
                width: 1.0,
                color: with_alpha(accent, 0.50),
            },
            ..Default::default()
        }
    })
}

fn tip_card_style(skin: &str) -> theme::Container<'static> {
    let skin_st = skin_style(skin);
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let accent = skin_st
            .map(|st| st.accent)
            .unwrap_or_else(|| Color::from(theme.cosmic().accent_color()));
        iced_container::Style {
            background: Some(Background::Color(with_alpha(accent, 0.11))),
            border: Border {
                radius: corners::now().control.into(),
                width: 1.0,
                color: with_alpha(accent, 0.42),
            },
            ..Default::default()
        }
    })
}

fn preview_frame_style(skin: &str) -> theme::Container<'static> {
    let skin_st = skin_style(skin);
    theme::Container::custom(move |theme: &cosmic::Theme| {
        let on_bg = Color::from(theme.cosmic().on_bg_color());
        let (bg, border) = match skin_st {
            Some(st) => {
                let win = Color::from_rgb8(st.window_bg[0], st.window_bg[1], st.window_bg[2]);
                let dark = lerp_color(Color::from_rgb8(0x0d, 0x11, 0x1a), win, 0.45);
                (with_alpha(dark, 0.96), with_alpha(st.border_tint, 0.42))
            }
            None => (
                Color::from_rgba8(0x0f, 0x13, 0x1e, 0.94),
                with_alpha(on_bg, 0.16),
            ),
        };
        iced_container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                radius: corners::now().control.into(),
                width: 1.0,
                color: border,
            },
            ..Default::default()
        }
    })
}

// ------------------------------------------------------------------------- Animation Math Helpers

fn clamp01(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// Wraps any finite phase into `[0.0, 1.0)`; non-finite values fall back to `0.0`.
fn wrap_phase(phase: f32) -> f32 {
    if phase.is_finite() {
        let p = phase.rem_euclid(1.0);
        if p >= 1.0 { 0.0 } else { p }
    } else {
        0.0
    }
}

/// Smooth `0.0..=1.0` looping sine pulse (0 at phase 0.0 & 1.0, 1 at phase 0.5).
fn pulse(phase: f32) -> f32 {
    clamp01(0.5 - 0.5 * (wrap_phase(phase) * TAU).cos())
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() <= f32::EPSILON {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = clamp01((x - edge0) / (edge1 - edge0));
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    let t = clamp01(t);
    a + (b - a) * t
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = clamp01(t);
    Color::from_rgba(
        clamp01(a.r + (b.r - a.r) * t),
        clamp01(a.g + (b.g - a.g) * t),
        clamp01(a.b + (b.b - a.b) * t),
        clamp01(a.a + (b.a - a.a) * t),
    )
}

fn with_alpha(mut c: Color, a: f32) -> Color {
    c.a = clamp01(a);
    c
}

/// Smoothly cycles through `colors`, wrapping back from the last color to the first.
fn cycle_palette(colors: &[Color], phase: f32) -> Color {
    if colors.is_empty() {
        return Color::WHITE;
    }
    if colors.len() == 1 {
        return colors[0];
    }
    let p = wrap_phase(phase) * colors.len() as f32;
    let idx = (p.floor() as usize) % colors.len();
    let next = (idx + 1) % colors.len();
    let frac = p.fract();
    // Smooth cosine blend between stops for lush transitions.
    let blend = 0.5 - 0.5 * (frac * PI).cos();
    lerp_color(colors[idx], colors[next], blend)
}

fn eval_cubic(p0: Point, p1: Point, p2: Point, p3: Point, t: f32) -> Point {
    let t = clamp01(t);
    let u = 1.0 - t;
    let uu = u * u;
    let tt = t * t;
    let uuu = uu * u;
    let ttt = tt * t;
    Point::new(
        uuu * p0.x + 3.0 * uu * t * p1.x + 3.0 * u * tt * p2.x + ttt * p3.x,
        uuu * p0.y + 3.0 * uu * t * p1.y + 3.0 * u * tt * p2.y + ttt * p3.y,
    )
}

fn eval_cubic_tangent(p0: Point, p1: Point, p2: Point, p3: Point, t: f32) -> Vector {
    let t = clamp01(t);
    let u = 1.0 - t;
    let dx = 3.0 * u * u * (p1.x - p0.x)
        + 6.0 * u * t * (p2.x - p1.x)
        + 3.0 * t * t * (p3.x - p2.x);
    let dy = 3.0 * u * u * (p1.y - p0.y)
        + 6.0 * u * t * (p2.y - p1.y)
        + 3.0 * t * t * (p3.y - p2.y);
    let len = (dx * dx + dy * dy).sqrt().max(1e-4);
    Vector::new(dx / len, dy / len)
}

// ------------------------------------------------------------------------- Vector Mini-Canvas Program

#[derive(Debug, Clone, Copy)]
struct HelpAnimProgram {
    kind: AnimKind,
    phase: f32,
}

impl<M> Program<M, cosmic::Theme, cosmic::Renderer> for HelpAnimProgram {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &cosmic::Renderer,
        _theme: &cosmic::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<cosmic::Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let p = wrap_phase(self.phase);
        let s = (bounds.width / CANVAS_W).min(bounds.height / CANVAS_H).max(0.01);
        let ox = (bounds.width - CANVAS_W * s) * 0.5;
        let oy = (bounds.height - CANVAS_H * s) * 0.5;
        let ctx = DrawCtx { ox, oy, s };

        match self.kind {
            AnimKind::Lights => draw_lights(&mut frame, ctx, p),
            AnimKind::Floorplan => draw_floorplan(&mut frame, ctx, p),
            AnimKind::Rules => draw_rules(&mut frame, ctx, p),
            AnimKind::MotionAware => draw_motion_aware(&mut frame, ctx, p),
            AnimKind::Schedules => draw_schedules(&mut frame, ctx, p),
            AnimKind::Synth => draw_synth(&mut frame, ctx, p),
            AnimKind::Skins => draw_skins(&mut frame, ctx, p),
            AnimKind::AskAi => draw_ask_ai(&mut frame, ctx, p),
            AnimKind::McpServer => draw_mcp_server(&mut frame, ctx, p),
        }

        vec![frame.into_geometry()]
    }
}

#[derive(Clone, Copy)]
struct DrawCtx {
    ox: f32,
    oy: f32,
    s: f32,
}

impl DrawCtx {
    fn pt(self, x: f32, y: f32) -> Point {
        Point::new(self.ox + x * self.s, self.oy + y * self.s)
    }

    fn sz(self, w: f32, h: f32) -> Size {
        Size::new(w * self.s, h * self.s)
    }

    fn r(self, v: f32) -> f32 {
        v * self.s
    }
}

fn draw_soft_glow(frame: &mut Frame, center: Point, max_r: f32, color: Color, strength: f32) {
    let strength = clamp01(strength);
    if strength <= 0.005 || max_r <= 0.5 {
        return;
    }
    const STEPS: usize = 6;
    for i in (1..=STEPS).rev() {
        let frac = i as f32 / STEPS as f32;
        let radius = max_r * frac;
        let falloff = (1.0 - frac * 0.86).powi(2);
        let alpha = strength * 0.24 * falloff;
        frame.fill(&Path::circle(center, radius), with_alpha(color, alpha));
    }
}

fn draw_arrowhead(frame: &mut Frame, tip: Point, dir: Vector, color: Color, size: f32) {
    let perp = Vector::new(-dir.y, dir.x);
    let back = Point::new(tip.x - dir.x * size, tip.y - dir.y * size);
    let p1 = Point::new(back.x + perp.x * (size * 0.56), back.y + perp.y * (size * 0.56));
    let p2 = Point::new(back.x - perp.x * (size * 0.56), back.y - perp.y * (size * 0.56));
    let tri = Path::new(|b| {
        b.move_to(tip);
        b.line_to(p1);
        b.line_to(p2);
        b.close();
    });
    frame.fill(&tri, color);
}

// ------------------------------------------------------------------------- 1. Lights, Color & Themes

fn draw_lights(frame: &mut Frame, c: DrawCtx, p: f32) {
    let palette = [
        Color::from_rgb8(0xff, 0xb0, 0x3a), // warm amber
        Color::from_rgb8(0xff, 0x5c, 0x77), // coral
        Color::from_rgb8(0x22, 0xd3, 0xee), // cyan
        Color::from_rgb8(0xa8, 0x55, 0xf7), // violet
    ];
    let orb_col = cycle_palette(&palette, p);
    let breath = pulse(p * 2.0);
    let bri = lerp(0.32, 0.92, pulse(p));

    let center = c.pt(60.0, 32.0);

    // Subtle pulsing soft radial halo
    let halo_r = c.r(lerp(24.0, 34.0, breath));
    draw_soft_glow(frame, center, halo_r, orb_col, lerp(0.55, 1.0, bri));

    // Tiny theme preset swatches in top-right corner
    for (idx, sw) in palette.iter().enumerate() {
        let sx = 96.0 + (idx % 2) as f32 * 9.5;
        let sy = 12.0 + (idx / 2) as f32 * 9.5;
        let pt = c.pt(sx, sy);
        let active = (p * 4.0).floor() as usize % 4 == idx;
        frame.fill(&Path::circle(pt, c.r(2.7)), with_alpha(*sw, if active { 0.95 } else { 0.42 }));
        if active {
            frame.stroke(
                &Path::circle(pt, c.r(4.1)),
                Stroke::default()
                    .with_color(with_alpha(Color::WHITE, 0.65))
                    .with_width(c.r(0.9)),
            );
        }
    }

    // Outer translucent glass rim
    frame.fill(&Path::circle(center, c.r(15.0)), with_alpha(orb_col, 0.22));
    frame.stroke(
        &Path::circle(center, c.r(15.0)),
        Stroke::default()
            .with_color(with_alpha(orb_col, 0.72))
            .with_width(c.r(1.2)),
    );

    // Bright glowing bulb core
    let core = lerp_color(orb_col, Color::WHITE, 0.28 + 0.18 * bri);
    frame.fill(&Path::circle(center, c.r(11.2)), core);

    // Specular highlight
    frame.fill(
        &Path::circle(c.pt(56.0, 28.0), c.r(3.4)),
        with_alpha(Color::WHITE, 0.68),
    );

    // Minimalist bulb base cap underneath orb
    let cap = Path::rounded_rectangle(c.pt(54.5, 47.0), c.sz(11.0, 4.5), c.r(2.0).into());
    frame.fill(&cap, Color::from_rgba8(0xcb, 0xd5, 0xe1, 0.73));
    let pin = Path::rounded_rectangle(c.pt(56.8, 51.8), c.sz(6.4, 2.2), c.r(1.0).into());
    frame.fill(&pin, Color::from_rgba8(0x94, 0xa3, 0xb8, 0.67));

    // Smooth animated brightness slider underneath
    let track_x = 18.0;
    let track_w = 84.0;
    let track_y = 65.5;
    let track_h = 5.4;

    let rail = Path::rounded_rectangle(
        c.pt(track_x, track_y),
        c.sz(track_w, track_h),
        c.r(track_h * 0.5).into(),
    );
    frame.fill(&rail, with_alpha(Color::WHITE, 0.14));

    let fill_w = (track_w * bri).max(track_h);
    let active_bar = Path::rounded_rectangle(
        c.pt(track_x, track_y),
        c.sz(fill_w, track_h),
        c.r(track_h * 0.5).into(),
    );
    frame.fill(&active_bar, orb_col);

    let knob_x = track_x + track_w * bri;
    let knob = c.pt(knob_x, track_y + track_h * 0.5);
    draw_soft_glow(frame, knob, c.r(9.5), orb_col, 0.65);
    frame.fill(&Path::circle(knob, c.r(5.0)), Color::WHITE);
    frame.stroke(
        &Path::circle(knob, c.r(5.0)),
        Stroke::default()
            .with_color(with_alpha(orb_col, 0.85))
            .with_width(c.r(1.3)),
    );
}

// ------------------------------------------------------------------------- 2. Visual Floorplan Map

fn draw_floorplan(frame: &mut Frame, c: DrawCtx, p: f32) {
    let warm = Color::from_rgb8(0xff, 0xc9, 0x5c);
    let wall = Color::from_rgb8(0xcb, 0xd5, 0xe1);
    let ink = Color::from_rgba8(0x94, 0xa3, 0xb8, 0.44);

    // Subtle architectural dot grid
    let dots = Path::new(|b| {
        let mut gx = 12.0;
        while gx <= 108.0 {
            let mut gy = 12.0;
            while gy <= 72.0 {
                b.rectangle(c.pt(gx - 0.45, gy - 0.45), c.sz(0.9, 0.9));
                gy += 12.0;
            }
            gx += 12.0;
        }
    });
    frame.fill(&dots, with_alpha(Color::WHITE, 0.08));

    // Smooth glide-in, settle + tooltip, smooth return for seamless loop
    let glide = smoothstep(0.06, 0.38, p) * (1.0 - smoothstep(0.82, 0.97, p));
    let tip_alpha = smoothstep(0.34, 0.48, p) * (1.0 - smoothstep(0.76, 0.88, p));

    let left_room = Path::rounded_rectangle(c.pt(10.0, 14.0), c.sz(52.0, 56.0), c.r(5.0).into());
    let right_room = Path::rounded_rectangle(c.pt(64.0, 14.0), c.sz(46.0, 56.0), c.r(5.0).into());

    // Ambient warm fill inside left room intensifies as the lamp glides in
    frame.fill(&left_room, with_alpha(warm, 0.03 + 0.14 * glide));
    frame.fill(&right_room, with_alpha(Color::from_rgb8(0x38, 0xbd, 0xf8), 0.05));

    // Room outlines
    let wall_stroke = Stroke {
        style: Style::Solid(with_alpha(wall, 0.55)),
        width: c.r(1.3),
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };
    frame.stroke(&left_room, wall_stroke);
    frame.stroke(&right_room, wall_stroke);

    // Doorway gap highlight along shared divider wall
    let door = Path::new(|b| {
        b.move_to(c.pt(63.0, 36.0));
        b.line_to(c.pt(63.0, 48.0));
    });
    frame.stroke(
        &door,
        Stroke::default()
            .with_color(Color::from_rgb8(0x10, 0x15, 0x22))
            .with_width(c.r(3.2)),
    );

    // Minimalist vector furniture: sofa in left room, desk in right room
    let sofa = Path::rounded_rectangle(c.pt(15.0, 56.0), c.sz(22.0, 8.5), c.r(2.4).into());
    frame.stroke(&sofa, Stroke::default().with_color(ink).with_width(c.r(1.0)));
    let desk = Path::rounded_rectangle(c.pt(83.0, 20.0), c.sz(20.0, 10.0), c.r(2.0).into());
    frame.stroke(&desk, Stroke::default().with_color(ink).with_width(c.r(1.0)));

    // Secondary soft lamp in right room
    let lamp2 = c.pt(88.0, 50.0);
    let lamp2_col = Color::from_rgb8(0x38, 0xbd, 0xf8);
    draw_soft_glow(frame, lamp2, c.r(14.0), lamp2_col, 0.45);
    frame.fill(&Path::circle(lamp2, c.r(4.6)), lamp2_col);
    frame.stroke(
        &Path::circle(lamp2, c.r(4.6)),
        Stroke::default().with_color(with_alpha(Color::WHITE, 0.8)).with_width(c.r(1.0)),
    );

    // Hero lamp gliding into left room
    let lx = lerp(19.0, 38.0, glide);
    let ly = lerp(52.0, 41.0, glide);
    let lamp = c.pt(lx, ly);

    draw_soft_glow(frame, lamp, c.r(lerp(10.0, 22.0, glide)), warm, 0.35 + 0.65 * glide);
    frame.fill(&Path::circle(lamp, c.r(6.2)), warm);
    frame.fill(&Path::circle(c.pt(lx - 1.6, ly - 1.6), c.r(1.8)), with_alpha(Color::WHITE, 0.72));
    frame.stroke(
        &Path::circle(lamp, c.r(6.2)),
        Stroke::default().with_color(Color::WHITE).with_width(c.r(1.2)),
    );

    // Tiny floating tooltip pill fading in above the settled lamp
    if tip_alpha > 0.02 {
        let ty = ly - 21.5 - (1.0 - tip_alpha) * 3.0;
        let pill = Path::rounded_rectangle(
            c.pt(lx - 17.0, ty),
            c.sz(34.0, 11.0),
            c.r(4.2).into(),
        );
        let tip_bg = Color::from_rgba8(0x0b, 0x0f, 0x1a, 0.92 * tip_alpha);
        frame.fill(&pill, tip_bg);
        frame.stroke(
            &pill,
            Stroke::default()
                .with_color(with_alpha(warm, 0.75 * tip_alpha))
                .with_width(c.r(1.0)),
        );

        // Downward pointer notch
        let notch = Path::new(|b| {
            b.move_to(c.pt(lx - 2.6, ty + 10.6));
            b.line_to(c.pt(lx + 2.6, ty + 10.6));
            b.line_to(c.pt(lx, ty + 13.8));
            b.close();
        });
        frame.fill(&notch, tip_bg);

        // Crisp vector text-line glyph inside tooltip pill (name placeholder + status dot)
        frame.fill(
            &Path::circle(c.pt(lx - 11.5, ty + 5.5), c.r(2.0)),
            with_alpha(warm, tip_alpha),
        );
        let bar = Path::rounded_rectangle(
            c.pt(lx - 7.0, ty + 4.2),
            c.sz(19.0, 2.6),
            c.r(1.3).into(),
        );
        frame.fill(&bar, with_alpha(Color::WHITE, 0.85 * tip_alpha));
    }
}

// ------------------------------------------------------------------------- 3. Visual Rules

fn draw_rules(frame: &mut Frame, c: DrawCtx, p: f32) {
    let cyan = Color::from_rgb8(0x38, 0xbd, 0xf8);
    let warm = Color::from_rgb8(0xfb, 0xbf, 0x24);
    let dim_node = Color::from_rgb8(0x33, 0x41, 0x55);

    let left = c.pt(23.0, 48.0);
    let right = c.pt(97.0, 36.0);

    let p0 = c.pt(31.5, 46.5);
    let p1 = c.pt(52.0, 14.0);
    let p2 = c.pt(68.0, 70.0);
    let p3 = c.pt(88.5, 37.5);

    // Subtle dashed preview guide wire
    let guide = Path::new(|b| {
        b.move_to(p0);
        b.bezier_curve_to(p1, p2, p3);
    });
    frame.stroke(
        &guide,
        Stroke {
            style: Style::Solid(with_alpha(cyan, 0.22)),
            width: c.r(1.4),
            line_cap: LineCap::Round,
            line_dash: LineDash { segments: &[3.0, 3.5], offset: 0 },
            ..Default::default()
        },
    );

    // Progressively trace cubic Bezier curve from left node to right node
    let trace = smoothstep(0.05, 0.62, p);
    let fade_out = 1.0 - smoothstep(0.88, 0.99, p);
    let wire_alpha = smoothstep(0.02, 0.08, p) * fade_out;

    if trace > 0.01 && wire_alpha > 0.01 {
        let steps = 28;
        let partial = Path::new(|b| {
            b.move_to(p0);
            for i in 1..=steps {
                let t = (i as f32 / steps as f32) * trace;
                b.line_to(eval_cubic(p0, p1, p2, p3, t));
            }
        });
        frame.stroke(
            &partial,
            Stroke {
                style: Style::Solid(with_alpha(cyan, 0.28 * wire_alpha)),
                width: c.r(5.4),
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Default::default()
            },
        );
        frame.stroke(
            &partial,
            Stroke {
                style: Style::Solid(with_alpha(cyan, 0.95 * wire_alpha)),
                width: c.r(2.1),
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Default::default()
            },
        );
    }

    // Arrowhead at target end as curve arrives
    let arrow_alpha = smoothstep(0.50, 0.62, p) * fade_out;
    if arrow_alpha > 0.02 {
        let tip = eval_cubic(p0, p1, p2, p3, 0.96);
        let tan = eval_cubic_tangent(p0, p1, p2, p3, 0.96);
        draw_arrowhead(frame, tip, tan, with_alpha(cyan, arrow_alpha), c.r(7.2));
    }

    // Traveling glowing cyan energy dot along curve
    let dot_vis = smoothstep(0.04, 0.10, p) * (1.0 - smoothstep(0.58, 0.68, p));
    if dot_vis > 0.02 {
        let dot_pos = eval_cubic(p0, p1, p2, p3, trace);
        draw_soft_glow(frame, dot_pos, c.r(10.0), cyan, 0.95 * dot_vis);
        frame.fill(&Path::circle(dot_pos, c.r(3.2)), with_alpha(Color::WHITE, dot_vis));
    }

    // Left (trigger) lamp node — always lit with subtle pulse
    let src_pulse = pulse(p * 2.0);
    draw_soft_glow(frame, left, c.r(16.0 + 3.0 * src_pulse), warm, 0.72);
    frame.fill(&Path::circle(left, c.r(8.5)), warm);
    frame.fill(&Path::circle(c.pt(20.6, 45.6), c.r(2.2)), with_alpha(Color::WHITE, 0.72));
    frame.stroke(
        &Path::circle(left, c.r(8.5)),
        Stroke::default().with_color(Color::WHITE).with_width(c.r(1.3)),
    );

    // Right (target) lamp node — lights up vividly on arrival
    let lit = smoothstep(0.58, 0.70, p) * (1.0 - smoothstep(0.88, 0.99, p));
    let target_col = lerp_color(dim_node, warm, lit);
    if lit > 0.02 {
        draw_soft_glow(frame, right, c.r(lerp(11.0, 21.0, lit)), warm, lit);
        // Arrival confirmation ring
        let burst_t = clamp01((p - 0.58) / 0.24);
        if burst_t > 0.0 && burst_t < 1.0 {
            frame.stroke(
                &Path::circle(right, c.r(8.5 + burst_t * 11.0)),
                Stroke::default()
                    .with_color(with_alpha(cyan, (1.0 - burst_t) * 0.85))
                    .with_width(c.r(1.6)),
            );
        }
    }
    frame.fill(&Path::circle(right, c.r(8.5)), target_col);
    frame.stroke(
        &Path::circle(right, c.r(8.5)),
        Stroke::default()
            .with_color(lerp_color(with_alpha(Color::WHITE, 0.35), Color::WHITE, lit))
            .with_width(c.r(1.3)),
    );
}

// ------------------------------------------------------------------------- 4. Bridge Pro MotionAware & Sensors

fn draw_motion_aware(frame: &mut Frame, c: DrawCtx, p: f32) {
    let emerald = Color::from_rgb8(0x10, 0xb9, 0x81);
    let cyan = Color::from_rgb8(0x22, 0xd3, 0xee);
    let warm = Color::from_rgb8(0xfd, 0xd8, 0x6b);

    // Room frame
    let room = Path::rounded_rectangle(c.pt(9.0, 10.0), c.sz(102.0, 64.0), c.r(6.0).into());
    frame.fill(&room, with_alpha(emerald, 0.04));
    frame.stroke(
        &room,
        Stroke::default()
            .with_color(with_alpha(Color::WHITE, 0.20))
            .with_width(c.r(1.2)),
    );

    let sensor_pos = c.pt(28.0, 56.0);
    let lamp_pos = c.pt(84.0, 34.0);
    let dist = ((lamp_pos.x - sensor_pos.x).powi(2) + (lamp_pos.y - sensor_pos.y).powi(2)).sqrt();
    let max_wave_r = c.r(74.0);

    // Three staggered concentric radar wave rings radiating from the sensor
    let mut lamp_hit = 0.0_f32;
    for i in 0..3 {
        let t = wrap_phase(p + i as f32 / 3.0);
        let wave_r = lerp(c.r(9.0), max_wave_r, t);
        let alpha = (1.0 - t).powf(1.3) * 0.78;
        let ring_col = lerp_color(emerald, cyan, t);

        // Partial arc sweep facing into the room (clipped smoothly inside room bounds via arc points)
        let ring_path = Path::new(|b| {
            let segs = 28;
            let start_ang = -PI * 0.62;
            let end_ang = PI * 0.16;
            for k in 0..=segs {
                let a = lerp(start_ang, end_ang, k as f32 / segs as f32);
                let pt = Point::new(
                    sensor_pos.x + wave_r * a.cos(),
                    sensor_pos.y + wave_r * a.sin(),
                );
                if k == 0 {
                    b.move_to(pt);
                } else {
                    b.line_to(pt);
                }
            }
        });
        frame.stroke(
            &ring_path,
            Stroke {
                style: Style::Solid(with_alpha(ring_col, alpha)),
                width: c.r(1.8),
                line_cap: LineCap::Round,
                ..Default::default()
            },
        );

        // Measure how closely this wave hits the MotionAware lamp
        let closeness = (1.0 - ((wave_r - dist).abs() / c.r(14.0))).clamp(0.0, 1.0);
        lamp_hit = lamp_hit.max(closeness);
    }

    // Subtle secondary radar echo ring around the MotionAware lamp itself
    let lamp_wave_t = wrap_phase(p * 2.0);
    frame.stroke(
        &Path::circle(lamp_pos, c.r(lerp(9.0, 22.0, lamp_wave_t))),
        Stroke::default()
            .with_color(with_alpha(emerald, (1.0 - lamp_wave_t) * 0.45))
            .with_width(c.r(1.1)),
    );

    // Physical Hue sensor square (bottom-left)
    let sq = Path::rounded_rectangle(c.pt(20.0, 48.0), c.sz(16.0, 16.0), c.r(3.5).into());
    frame.fill(&sq, Color::from_rgb8(0x16, 0x24, 0x33));
    frame.stroke(
        &sq,
        Stroke::default()
            .with_color(with_alpha(cyan, 0.85))
            .with_width(c.r(1.3)),
    );
    let led_pulse = pulse(p * 3.0);
    draw_soft_glow(frame, sensor_pos, c.r(7.5), emerald, 0.45 + 0.55 * led_pulse);
    frame.fill(
        &Path::circle(sensor_pos, c.r(2.7)),
        lerp_color(emerald, Color::WHITE, 0.35 * led_pulse),
    );

    // MotionAware lamp (upper-right) lighting up as pulses arrive
    let glow_level = 0.32 + 0.68 * lamp_hit;
    draw_soft_glow(frame, lamp_pos, c.r(lerp(13.0, 24.0, lamp_hit)), warm, glow_level);
    let lamp_col = lerp_color(Color::from_rgb8(0x5b, 0x6b, 0x7f), warm, glow_level);
    frame.fill(&Path::circle(lamp_pos, c.r(8.2)), lamp_col);
    frame.stroke(
        &Path::circle(lamp_pos, c.r(8.2)),
        Stroke::default()
            .with_color(lerp_color(with_alpha(Color::WHITE, 0.5), Color::WHITE, lamp_hit))
            .with_width(c.r(1.2)),
    );

    // Tiny MotionAware presence badge on top-right corner of the lamp
    let badge = c.pt(91.5, 26.5);
    let badge_r = c.r(lerp(3.4, 4.4, lamp_hit));
    draw_soft_glow(frame, badge, c.r(8.5), emerald, 0.5 + 0.5 * lamp_hit);
    frame.fill(&Path::circle(badge, badge_r), emerald);
    frame.stroke(
        &Path::circle(badge, badge_r),
        Stroke::default()
            .with_color(Color::from_rgb8(0x0d, 0x13, 0x20))
            .with_width(c.r(1.1)),
    );
}

// ------------------------------------------------------------------------- 5. Schedules

fn draw_schedules(frame: &mut Frame, c: DrawCtx, p: f32) {
    let sun_gold = Color::from_rgb8(0xfb, 0xbf, 0x24);
    let moon_silver = Color::from_rgb8(0x93, 0xc5, 0xfd);
    let dusk_glow = Color::from_rgb8(0xf9, 0x73, 0x16);

    // Night intensity peaks around p = 0.75 (moon overhead), lowest at p = 0.25 (sun overhead)
    let night = pulse(p + 0.25);

    // Subtle twilight horizon glow during transitions (near p = 0.0 and p = 0.5)
    let twilight = (1.0 - (night - 0.5).abs() * 2.0).powi(2);
    if twilight > 0.02 {
        let band = Path::rectangle(c.pt(0.0, 34.0), c.sz(120.0, 26.0));
        frame.fill(&band, with_alpha(dusk_glow, 0.14 * twilight));
    }

    // Tiny twinkling stars during the night half
    let stars = [(18.0, 16.0), (34.0, 12.0), (88.0, 14.0), (104.0, 22.0), (24.0, 29.0)];
    if night > 0.35 {
        let star_vis = smoothstep(0.35, 0.75, night);
        for (i, (sx, sy)) in stars.iter().enumerate() {
            let tw = 0.45 + 0.55 * pulse(p * 3.0 + i as f32 * 0.21);
            frame.fill(
                &Path::circle(c.pt(*sx, *sy), c.r(1.15)),
                with_alpha(Color::WHITE, star_vis * tw * 0.85),
            );
        }
    }

    // Dashed sky arc spanning horizon to horizon
    let arc_cx = 60.0;
    let arc_cy = 58.0;
    let arc_rx = 44.0;
    let arc_ry = 41.0;

    let sky_arc = Path::new(|b| {
        let segs = 32;
        for i in 0..=segs {
            let a = PI - (i as f32 / segs as f32) * PI;
            let pt = c.pt(arc_cx + arc_rx * a.cos(), arc_cy - arc_ry * a.sin());
            if i == 0 {
                b.move_to(pt);
            } else {
                b.line_to(pt);
            }
        }
    });
    frame.stroke(
        &sky_arc,
        Stroke {
            style: Style::Solid(with_alpha(lerp_color(sun_gold, moon_silver, night), 0.34)),
            width: c.r(1.2),
            line_cap: LineCap::Round,
            line_dash: LineDash { segments: &[3.0, 3.5], offset: 0 },
            ..Default::default()
        },
    );

    // Sun travels left->right across the arc during p in 0.0..0.5; Moon during p in 0.5..1.0
    if p < 0.5 {
        let t = p * 2.0;
        let a = PI * (1.0 - t);
        let ox = arc_cx + arc_rx * a.cos();
        let oy = arc_cy - arc_ry * a.sin();
        let orb = c.pt(ox, oy);
        let vis = smoothstep(0.0, 0.14, t) * (1.0 - smoothstep(0.86, 1.0, t));
        draw_soft_glow(frame, orb, c.r(16.0), sun_gold, 0.85 * vis);
        frame.fill(&Path::circle(orb, c.r(6.2)), with_alpha(sun_gold, vis));
        frame.fill(&Path::circle(orb, c.r(3.8)), with_alpha(Color::WHITE, 0.45 * vis));
    } else {
        let t = (p - 0.5) * 2.0;
        let a = PI * (1.0 - t);
        let ox = arc_cx + arc_rx * a.cos();
        let oy = arc_cy - arc_ry * a.sin();
        let orb = c.pt(ox, oy);
        let vis = smoothstep(0.0, 0.14, t) * (1.0 - smoothstep(0.86, 1.0, t));
        draw_soft_glow(frame, orb, c.r(14.0), moon_silver, 0.72 * vis);
        frame.fill(&Path::circle(orb, c.r(5.8)), with_alpha(moon_silver, vis));
        // Crescent cut-out matching backdrop
        frame.fill(
            &Path::circle(c.pt(ox + 2.5, oy - 1.6), c.r(4.6)),
            with_alpha(Color::from_rgb8(0x0f, 0x13, 0x1e), vis),
        );
    }

    // Horizon line
    let horizon = Path::new(|b| {
        b.move_to(c.pt(10.0, 74.0));
        b.line_to(c.pt(110.0, 74.0));
    });
    frame.stroke(
        &horizon,
        Stroke::default()
            .with_color(with_alpha(Color::WHITE, 0.18))
            .with_width(c.r(1.1)),
    );

    // Warm light spill behind the house as evening windows light up
    let window_lit = smoothstep(0.28, 0.72, night);
    draw_soft_glow(frame, c.pt(60.0, 62.0), c.r(24.0), sun_gold, 0.52 * window_lit);

    // Minimalist house silhouette (body + pitched roof)
    let body = Path::rounded_rectangle(c.pt(42.0, 52.0), c.sz(36.0, 22.0), c.r(2.0).into());
    frame.fill(&body, Color::from_rgb8(0x1e, 0x29, 0x3b));
    frame.stroke(
        &body,
        Stroke::default()
            .with_color(with_alpha(Color::WHITE, 0.24))
            .with_width(c.r(1.1)),
    );

    let roof = Path::new(|b| {
        b.move_to(c.pt(37.5, 53.5));
        b.line_to(c.pt(60.0, 38.0));
        b.line_to(c.pt(82.5, 53.5));
        b.close();
    });
    frame.fill(&roof, Color::from_rgb8(0x27, 0x34, 0x49));
    frame.stroke(
        &roof,
        Stroke {
            style: Style::Solid(with_alpha(Color::WHITE, 0.30)),
            width: c.r(1.1),
            line_join: LineJoin::Round,
            ..Default::default()
        },
    );

    // House windows fading smoothly from dim slate to warm lit amber
    let win_col = lerp_color(
        Color::from_rgb8(0x33, 0x41, 0x55),
        Color::from_rgb8(0xfd, 0xe0, 0x47),
        window_lit,
    );
    for (wx, wy, ww, wh) in [
        (48.0, 56.5, 8.0, 6.5),
        (64.0, 56.5, 8.0, 6.5),
        (55.5, 65.5, 9.0, 8.0),
    ] {
        let wpath = Path::rounded_rectangle(c.pt(wx, wy), c.sz(ww, wh), c.r(1.2).into());
        frame.fill(&wpath, win_col);
    }
}

// ------------------------------------------------------------------------- 6. Audio Light Synth

fn draw_synth(frame: &mut Frame, c: DrawCtx, p: f32) {
    let bass_col = Color::from_rgb8(0xec, 0x48, 0x99); // magenta
    let mids_col = Color::from_rgb8(0x22, 0xd3, 0xee); // cyan
    let treb_col = Color::from_rgb8(0xfb, 0xbf, 0x24); // gold

    // Rhythmic seamless 4-beat pulse + harmonic band energy
    let beat_frac = (p * 4.0).fract();
    let beat = (1.0 - beat_frac).powi(3);

    let bass_lvl = clamp01(0.32 + 0.56 * beat + 0.12 * pulse(p * 2.0));
    let mids_lvl = clamp01(0.28 + 0.42 * pulse(p * 3.0 + 0.15) + 0.25 * beat);
    let treb_lvl = clamp01(0.25 + 0.48 * pulse(p * 5.0 + 0.4) + 0.20 * (1.0 - beat));

    // Synchronized glowing mini-lamps above each band
    let lamps = [
        (27.0, bass_col, bass_lvl),
        (60.0, mids_col, mids_lvl),
        (93.0, treb_col, treb_lvl),
    ];
    for (lx, col, lvl) in lamps {
        let pos = c.pt(lx, 18.0);
        draw_soft_glow(frame, pos, c.r(lerp(9.0, 17.0, lvl)), col, 0.35 + 0.65 * lvl);
        frame.fill(&Path::circle(pos, c.r(5.6)), lerp_color(col, Color::WHITE, 0.28 * lvl));
        frame.stroke(
            &Path::circle(pos, c.r(5.6)),
            Stroke::default()
                .with_color(with_alpha(Color::WHITE, 0.55 + 0.4 * lvl))
                .with_width(c.r(1.0)),
        );
    }

    // Pulsing beat indicator circle in upper-center between lamps & EQ bars
    let beat_pt = c.pt(60.0, 35.0);
    frame.stroke(
        &Path::circle(beat_pt, c.r(lerp(4.0, 9.5, 1.0 - beat))),
        Stroke::default()
            .with_color(with_alpha(bass_col, beat * 0.85))
            .with_width(c.r(1.3)),
    );
    frame.fill(
        &Path::circle(beat_pt, c.r(lerp(2.3, 3.8, beat))),
        lerp_color(Color::from_rgb8(0x64, 0x74, 0x8b), Color::WHITE, beat),
    );

    // 9 animated equalizer bars (3 Bass, 3 Mids, 3 Treble)
    let base_y = 75.0;
    let max_bar_h = 30.0;
    let bar_w = 7.4;
    let start_x = 14.0;
    let gap = 10.6;

    for i in 0..9 {
        let band = i / 3;
        let sub = (i % 3) as f32;
        let (col, base_lvl) = match band {
            0 => (bass_col, bass_lvl),
            1 => (mids_col, mids_lvl),
            _ => (treb_col, treb_lvl),
        };
        let wave = pulse(p * (2.0 + band as f32) + sub * 0.27);
        let h_norm = clamp01(0.22 + 0.55 * base_lvl + 0.23 * wave);
        let bh = (max_bar_h * h_norm).max(4.0);
        let bx = start_x + i as f32 * gap;
        let by = base_y - bh;

        // Subtle vertical track slot
        let slot = Path::rounded_rectangle(
            c.pt(bx, base_y - max_bar_h),
            c.sz(bar_w, max_bar_h),
            c.r(2.4).into(),
        );
        frame.fill(&slot, with_alpha(Color::WHITE, 0.05));

        // Lit animated bar
        let bar = Path::rounded_rectangle(c.pt(bx, by), c.sz(bar_w, bh), c.r(2.4).into());
        frame.fill(&bar, with_alpha(col, 0.88));

        // Crisp bright peak cap
        let cap = Path::rounded_rectangle(c.pt(bx, by), c.sz(bar_w, 2.4), c.r(1.2).into());
        frame.fill(&cap, with_alpha(Color::WHITE, 0.80));
    }
}

// ------------------------------------------------------------------------- 7. Skins, Icons & Shortcuts

fn draw_skins(frame: &mut Frame, c: DrawCtx, p: f32) {
    let swatches = [
        Color::from_rgb8(0xff, 0x2e, 0x93), // Synthwave pink
        Color::from_rgb8(0xf0, 0x9a, 0x36), // Cozy amber
        Color::from_rgb8(0x2c, 0xe8, 0x68), // Terminal green
        Color::from_rgb8(0xd9, 0x46, 0xef), // Disco violet
        Color::from_rgb8(0x52, 0xb7, 0x98), // Zen teal
    ];

    let accent = cycle_palette(&swatches, p);

    // Subtle ambient glow behind mini app window card
    draw_soft_glow(frame, c.pt(60.0, 35.0), c.r(38.0), accent, 0.36);

    // Mini app window card
    let win = Path::rounded_rectangle(c.pt(16.0, 9.0), c.sz(88.0, 52.0), c.r(7.0).into());
    frame.fill(&win, Color::from_rgb8(0x15, 0x1a, 0x29));
    frame.stroke(
        &win,
        Stroke::default()
            .with_color(with_alpha(accent, 0.85))
            .with_width(c.r(1.5)),
    );

    // Mini window header bar (3 dots + title pill)
    for (idx, dx) in [23.0, 28.5, 34.0].iter().enumerate() {
        let dot_c = if idx == 0 { accent } else { with_alpha(Color::WHITE, 0.30) };
        frame.fill(&Path::circle(c.pt(*dx, 16.0), c.r(1.8)), dot_c);
    }
    let header_bar = Path::rounded_rectangle(c.pt(44.0, 14.5), c.sz(34.0, 3.0), c.r(1.5).into());
    frame.fill(&header_bar, with_alpha(Color::WHITE, 0.36));

    // Accent pill button in top-right of mini window
    let badge = Path::rounded_rectangle(c.pt(84.0, 13.5), c.sz(14.0, 5.0), c.r(2.5).into());
    frame.fill(&badge, accent);

    // Two mini room cards inside the window reflecting the live skin accent
    for (cy, fill_frac) in [(25.0, 0.72_f32), (42.0, 0.48_f32)] {
        let card = Path::rounded_rectangle(c.pt(22.0, cy), c.sz(76.0, 13.0), c.r(3.5).into());
        frame.fill(&card, with_alpha(accent, 0.13));
        frame.stroke(
            &card,
            Stroke::default()
                .with_color(with_alpha(accent, 0.38))
                .with_width(c.r(0.9)),
        );
        // Mini lamp dot
        frame.fill(&Path::circle(c.pt(29.0, cy + 6.5), c.r(3.0)), accent);
        // Mini slider bar
        let rail = Path::rounded_rectangle(c.pt(36.0, cy + 5.1), c.sz(54.0, 2.8), c.r(1.4).into());
        frame.fill(&rail, with_alpha(Color::WHITE, 0.14));
        let bar = Path::rounded_rectangle(
            c.pt(36.0, cy + 5.1),
            c.sz(54.0 * fill_frac, 2.8),
            c.r(1.4).into(),
        );
        frame.fill(&bar, with_alpha(accent, 0.90));
    }

    // Bottom swatch strip with glowing active indicator
    let active_idx = (p * swatches.len() as f32).floor() as usize % swatches.len();
    for (idx, sw) in swatches.iter().enumerate() {
        let sx = 30.0 + idx as f32 * 15.0;
        let pt = c.pt(sx, 72.5);
        let is_active = idx == active_idx;
        if is_active {
            draw_soft_glow(frame, pt, c.r(9.5), *sw, 0.92);
            frame.fill(&Path::circle(pt, c.r(5.0)), *sw);
            frame.stroke(
                &Path::circle(pt, c.r(5.2)),
                Stroke::default().with_color(Color::WHITE).with_width(c.r(1.4)),
            );
        } else {
            frame.fill(&Path::circle(pt, c.r(3.8)), with_alpha(*sw, 0.72));
        }
    }
}

// ------------------------------------------------------------------------- 8. Ask AI (Optional)

fn draw_ask_ai(frame: &mut Frame, c: DrawCtx, p: f32) {
    let violet = Color::from_rgb8(0xa8, 0x55, 0xf7);
    let amber = Color::from_rgb8(0xfb, 0xbf, 0x24);
    let cyan = Color::from_rgb8(0x22, 0xd3, 0xee);

    // Chat prompt bubble on the left
    let bubble = Path::rounded_rectangle(c.pt(10.0, 18.0), c.sz(54.0, 34.0), c.r(7.0).into());
    frame.fill(&bubble, with_alpha(violet, 0.16));
    frame.stroke(
        &bubble,
        Stroke::default()
            .with_color(with_alpha(violet, 0.75))
            .with_width(c.r(1.3)),
    );

    // Animated typing bars inside the bubble
    let type_progress = smoothstep(0.05, 0.45, p);
    let bar1_w = 34.0 * clamp01(type_progress * 1.4);
    let bar2_w = 24.0 * clamp01((type_progress - 0.35) * 1.8);
    if bar1_w > 1.0 {
        let b1 = Path::rounded_rectangle(c.pt(17.0, 27.0), c.sz(bar1_w, 3.6), c.r(1.8).into());
        frame.fill(&b1, with_alpha(Color::WHITE, 0.85));
    }
    if bar2_w > 1.0 {
        let b2 = Path::rounded_rectangle(c.pt(17.0, 35.0), c.sz(bar2_w, 3.6), c.r(1.8).into());
        frame.fill(&b2, with_alpha(cyan, 0.85));
    }

    // 4-point sparkle pulsing on the bubble corner
    let sp = 0.65 + 0.35 * pulse(p * 2.0);
    let sc = c.pt(59.0, 20.0);
    draw_soft_glow(frame, sc, c.r(10.0), violet, 0.6 * sp);
    let star = Path::new(|b| {
        let r_out = c.r(5.6 * sp);
        let r_in = c.r(1.8 * sp);
        for i in 0..8 {
            let ang = (i as f32) * (PI / 4.0);
            let rad = if i % 2 == 0 { r_out } else { r_in };
            let pt = Point::new(sc.x + ang.cos() * rad, sc.y + ang.sin() * rad);
            if i == 0 { b.move_to(pt); } else { b.line_to(pt); }
        }
        b.close();
    });
    frame.fill(&star, Color::from_rgb8(0xe9, 0xd5, 0xff));

    // Signal dot traveling from prompt to lamp
    let travel = smoothstep(0.32, 0.65, p);
    let line = Path::line(c.pt(66.0, 36.0), c.pt(84.0, 36.0));
    frame.stroke(
        &line,
        Stroke::default()
            .with_color(with_alpha(violet, 0.45))
            .with_width(c.r(1.4)),
    );
    if travel > 0.01 && travel < 0.99 {
        let px = lerp(66.0, 84.0, travel);
        let pt = c.pt(px, 36.0);
        draw_soft_glow(frame, pt, c.r(7.0), cyan, 0.8);
        frame.fill(&Path::circle(pt, c.r(2.5)), Color::WHITE);
    }

    // Lamp orb lighting up warmly after command arrives
    let lit = smoothstep(0.55, 0.82, p) * (1.0 - smoothstep(0.90, 1.0, p));
    let lc = c.pt(96.0, 36.0);
    draw_soft_glow(frame, lc, c.r(19.0), amber, 0.25 + 0.75 * lit);
    let orb_col = lerp_color(Color::from_rgb8(0x3a, 0x42, 0x56), amber, lit);
    frame.fill(&Path::circle(lc, c.r(10.5)), orb_col);
    frame.stroke(
        &Path::circle(lc, c.r(10.5)),
        Stroke::default()
            .with_color(with_alpha(Color::WHITE, 0.35 + 0.45 * lit))
            .with_width(c.r(1.3)),
    );

    // Optional toggle pill at bottom ("OFF by default -> opt-in")
    let track = Path::rounded_rectangle(c.pt(38.0, 62.0), c.sz(44.0, 12.0), c.r(6.0).into());
    frame.fill(&track, with_alpha(violet, 0.22 + 0.35 * lit));
    frame.stroke(
        &track,
        Stroke::default()
            .with_color(with_alpha(violet, 0.65))
            .with_width(c.r(1.0)),
    );
    let knob_x = lerp(45.0, 75.0, smoothstep(0.10, 0.35, p) * (1.0 - smoothstep(0.88, 0.98, p)));
    frame.fill(&Path::circle(c.pt(knob_x, 68.0), c.r(4.4)), Color::WHITE);
}

// ------------------------------------------------------------------------- 9. MCP Server

fn draw_mcp_server(frame: &mut Frame, c: DrawCtx, p: f32) {
    let teal = Color::from_rgb8(0x14, 0xb8, 0xa6);
    let amber = Color::from_rgb8(0xf5, 0x9e, 0x0b);
    let sky = Color::from_rgb8(0x38, 0xbd, 0xf8);

    // Left: Agent terminal box
    let term = Path::rounded_rectangle(c.pt(10.0, 20.0), c.sz(34.0, 44.0), c.r(6.0).into());
    frame.fill(&term, Color::from_rgb8(0x13, 0x1b, 0x2a));
    frame.stroke(
        &term,
        Stroke::default()
            .with_color(with_alpha(sky, 0.78))
            .with_width(c.r(1.3)),
    );
    // Terminal prompt chevron ">_"
    let chev = Path::new(|b| {
        b.move_to(c.pt(17.0, 35.0));
        b.line_to(c.pt(23.0, 40.0));
        b.line_to(c.pt(17.0, 45.0));
    });
    frame.stroke(
        &chev,
        Stroke::default()
            .with_color(sky)
            .with_width(c.r(1.8))
            .with_line_cap(LineCap::Round)
            .with_line_join(LineJoin::Round),
    );
    let cursor_on = if (p * 4.0).fract() < 0.5 { 0.95 } else { 0.25 };
    let cur = Path::rounded_rectangle(c.pt(26.0, 42.5), c.sz(9.0, 2.8), c.r(1.2).into());
    frame.fill(&cur, with_alpha(teal, cursor_on));

    // Bidirectional JSON-RPC stdio tracks
    let top_y = 33.0;
    let bot_y = 51.0;
    frame.stroke(
        &Path::line(c.pt(46.0, top_y), c.pt(76.0, top_y)),
        Stroke::default()
            .with_color(with_alpha(teal, 0.42))
            .with_width(c.r(1.4)),
    );
    frame.stroke(
        &Path::line(c.pt(46.0, bot_y), c.pt(76.0, bot_y)),
        Stroke::default()
            .with_color(with_alpha(amber, 0.42))
            .with_width(c.r(1.4)),
    );

    // Request packet (left -> right)
    let req_x = lerp(47.0, 75.0, (p * 1.5).fract());
    let req_pt = c.pt(req_x, top_y);
    draw_soft_glow(frame, req_pt, c.r(7.5), teal, 0.85);
    frame.fill(&Path::circle(req_pt, c.r(2.8)), Color::WHITE);

    // Response packet (right -> left)
    let res_x = lerp(75.0, 47.0, ((p * 1.5) + 0.5).fract());
    let res_pt = c.pt(res_x, bot_y);
    draw_soft_glow(frame, res_pt, c.r(7.5), amber, 0.85);
    frame.fill(&Path::circle(res_pt, c.r(2.8)), Color::WHITE);

    // Right: Hue Bridge hub + 2 lamps
    let hub = Path::rounded_rectangle(c.pt(78.0, 22.0), c.sz(32.0, 40.0), c.r(8.0).into());
    frame.fill(&hub, Color::from_rgb8(0x18, 0x20, 0x32));
    frame.stroke(
        &hub,
        Stroke::default()
            .with_color(with_alpha(teal, 0.72))
            .with_width(c.r(1.3)),
    );
    let hub_c = c.pt(94.0, 42.0);
    let beat = pulse(p * 1.5);
    draw_soft_glow(frame, hub_c, c.r(15.0), amber, 0.30 + 0.55 * beat);
    frame.fill(&Path::circle(hub_c, c.r(7.5)), lerp_color(teal, amber, beat));
    frame.stroke(
        &Path::circle(hub_c, c.r(11.0)),
        Stroke::default()
            .with_color(with_alpha(Color::WHITE, 0.30 + 0.30 * beat))
            .with_width(c.r(1.1)),
    );
}

// ------------------------------------------------------------------------- Unit Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_and_wrap_stay_within_unit_interval() {
        for &p in &[
            -2.5, -0.25, 0.0, 0.125, 0.25, 0.5, 0.75, 0.9999, 1.0, 1.5, 42.3, f32::NAN,
            f32::INFINITY, f32::NEG_INFINITY,
        ] {
            let wp = wrap_phase(p);
            assert!((0.0..=1.0).contains(&wp), "wrap_phase({p}) = {wp}");
            let v = pulse(p);
            assert!((0.0..=1.0).contains(&v), "pulse({p}) = {v}");
        }
        assert!(pulse(0.0).abs() < 1e-5);
        assert!((pulse(0.5) - 1.0).abs() < 1e-5);
        assert!(pulse(1.0).abs() < 1e-5);
    }

    #[test]
    fn lerp_color_and_cycle_palette_stay_clamped() {
        let a = Color::from_rgba(0.1, 0.9, 0.2, 0.4);
        let b = Color::from_rgba(0.95, 0.05, 0.85, 1.0);
        for &t in &[-1.0, -0.01, 0.0, 0.25, 0.5, 0.75, 1.0, 1.5, f32::NAN] {
            let c = lerp_color(a, b, t);
            for ch in [c.r, c.g, c.b, c.a] {
                assert!((0.0..=1.0).contains(&ch), "lerp_color channel out of range at t={t}: {ch}");
            }
        }

        let palette = [
            Color::from_rgb8(0xff, 0x2e, 0x93),
            Color::from_rgb8(0xf0, 0x9a, 0x36),
            Color::from_rgb8(0x2c, 0xe8, 0x68),
            Color::from_rgb8(0xd9, 0x46, 0xef),
            Color::from_rgb8(0x52, 0xb7, 0x98),
        ];
        for step in 0..=20 {
            let p = step as f32 / 20.0;
            let c = cycle_palette(&palette, p);
            for ch in [c.r, c.g, c.b, c.a] {
                assert!((0.0..=1.0).contains(&ch), "cycle_palette channel out of range at p={p}: {ch}");
            }
        }
        // Seamless loop: phase 0.0 matches phase 1.0
        let c0 = cycle_palette(&palette, 0.0);
        let c1 = cycle_palette(&palette, 1.0);
        assert!((c0.r - c1.r).abs() < 1e-5);
        assert!((c0.g - c1.g).abs() < 1e-5);
        assert!((c0.b - c1.b).abs() < 1e-5);
    }

    #[test]
    fn smoothstep_clamps_and_is_monotonic() {
        assert_eq!(smoothstep(0.2, 0.8, -5.0), 0.0);
        assert_eq!(smoothstep(0.2, 0.8, 0.2), 0.0);
        assert_eq!(smoothstep(0.2, 0.8, 0.8), 1.0);
        assert_eq!(smoothstep(0.2, 0.8, 5.0), 1.0);
        let mut prev = 0.0;
        for i in 0..=20 {
            let v = smoothstep(0.2, 0.8, i as f32 / 20.0);
            assert!((0.0..=1.0).contains(&v));
            assert!(v >= prev - 1e-6);
            prev = v;
        }
    }

    #[test]
    fn all_features_have_beginner_steps_tips_and_valid_selection_bounds() {
        assert_eq!(FEATURE_COUNT, 9);
        assert_eq!(FEATURES.len(), FEATURE_COUNT);
        for (idx, feat) in FEATURES.iter().enumerate() {
            assert!(!feat.title.trim().is_empty(), "feature {idx} missing title");
            assert!(!feat.desc.trim().is_empty(), "feature {idx} missing desc");
            assert!(!feat.summary.trim().is_empty(), "feature {idx} missing summary");
            assert!(
                feat.steps.len() >= 3,
                "feature {} should have at least 3 beginner steps, got {}",
                feat.title,
                feat.steps.len()
            );
            for (sidx, step) in feat.steps.iter().enumerate() {
                assert!(
                    !step.trim().is_empty(),
                    "feature {} step {sidx} is empty",
                    feat.title
                );
            }
            assert!(!feat.tip.trim().is_empty(), "feature {} missing tip", feat.title);
            assert_eq!(resolve_selected_feature(Some(idx)), Some(idx));
        }

        // Bounds checks on detail selection
        assert_eq!(resolve_selected_feature(None), None);
        assert_eq!(resolve_selected_feature(Some(FEATURE_COUNT)), None);
        assert_eq!(resolve_selected_feature(Some(FEATURE_COUNT + 42)), None);
        assert_eq!(resolve_selected_feature(Some(usize::MAX)), None);

        // Actionable scenarios expose a "Try it now" target
        assert_eq!(FEATURES[0].try_action, Some(HelpAction::OpenLights));
        assert_eq!(FEATURES[1].try_action, Some(HelpAction::OpenMap));
        assert_eq!(FEATURES[2].try_action, Some(HelpAction::OpenMapRules));
        assert_eq!(FEATURES[3].try_action, Some(HelpAction::OpenMap));
        assert_eq!(FEATURES[4].try_action, Some(HelpAction::OpenSchedules));
        assert_eq!(FEATURES[5].try_action, Some(HelpAction::OpenSynth));
        assert_eq!(FEATURES[6].try_action, Some(HelpAction::OpenSettings));
        assert_eq!(FEATURES[7].try_action, Some(HelpAction::OpenSettings));
        assert_eq!(FEATURES[8].try_action, None);
    }
}
