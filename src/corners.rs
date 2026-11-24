//! Settings › Corners, in AI Playground's three sizes. Shared copy: the same file in every
//! Maprisma COSMIC app; change it in all of them.
//!
//! "desktop" follows the desktop's window corners: the COSMIC theme's medium radius (16 Round, 8
//! Slightly round, 2 Square in COSMIC Settings), Hyprland's rounding, KDE's 8 (look.rs); "slight"
//! is 6, "rounded" 12, "square" 0. The colours come from look.rs too (the desktop's theme outside
//! COSMIC). Cards and popup containers take the radius, buttons,
//! drop-downs and fields at most 8, small parts (check boxes, tracks, badges, swatches) at most
//! 4, pills stay round (0 in Square).

use std::sync::{Arc, RwLock};

pub const CHOICES: [&str; 4] = ["desktop", "slight", "rounded", "square"];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sizes {
    pub radius: f32,
    pub control: f32,
    pub small: f32,
    pub pill: f32,
}

impl Sizes {
    pub fn new(radius: f32) -> Self {
        let radius = radius.max(0.0);
        Self { radius, control: radius.min(8.0), small: radius.min(4.0), pill: if radius > 0.0 { 999.0 } else { 0.0 } }
    }
}

/// An unknown or missing setting is "desktop".
pub fn normalize(setting: &str) -> &'static str {
    CHOICES.iter().find(|c| **c == setting).copied().unwrap_or("desktop")
}

pub fn radius(setting: &str, desktop: f32) -> f32 {
    match normalize(setting) {
        "slight" => 6.0,
        "rounded" => 12.0,
        "square" => 0.0,
        _ => desktop,
    }
}

/// Cached COSMIC/system fallback radius (updated in [system_theme_update] or lazily from [system]).
static SYSTEM_RADIUS: RwLock<Option<f32>> = RwLock::new(None);

fn system_radius() -> f32 {
    if let Some(r) = SYSTEM_RADIUS.read().ok().and_then(|r| *r) {
        return r;
    }
    let r = system().cosmic().corner_radii.radius_m[0];
    if let Ok(mut cached) = SYSTEM_RADIUS.write() {
        *cached = Some(r);
    }
    r
}

/// The desktop's window corners (what "Like the desktop" follows).
pub fn desktop_radius() -> f32 {
    crate::look::window_radius().unwrap_or_else(system_radius)
}

/// The last system (COSMIC Settings) theme libcosmic reported (see [system_theme_update]).
static SYSTEM: RwLock<Option<cosmic::Theme>> = RwLock::new(None);

/// The theme our corners are put on where the desktop's look has none (COSMIC): libcosmic's
/// active theme while it is the system's, else the system theme last reported, else read now.
/// Never the custom theme set_app_theme installed: libcosmic doesn't update a custom theme when
/// COSMIC's colours or light/dark mode change, so it would keep the old colours.
fn system() -> cosmic::Theme {
    let active = cosmic::theme::active();
    if !matches!(active.theme_type, cosmic::theme::ThemeType::Custom(_)) {
        return active;
    }
    if let Some(theme) = SYSTEM.read().ok().and_then(|s| s.clone()) {
        return theme;
    }
    let theme = cosmic::theme::system_preference();
    if let Ok(mut stored) = SYSTEM.write() {
        *stored = Some(theme.clone());
    }
    theme
}

/// For the app's `system_theme_update` (libcosmic calls it when COSMIC's theme or light/dark
/// mode changes): keeps the new system theme and, in `--window` mode with our own theme in
/// effect, sets it again on the new colours. The applet popup draws with [apply], which follows
/// by itself.
#[allow(dead_code)] // apps with a system_theme_update of their own don't call it
pub fn system_theme_update<M: Send + 'static>(
    setting: &str,
    new_theme: &cosmic::cosmic_theme::Theme,
    window_mode: bool,
) -> cosmic::Task<cosmic::Action<M>> {
    if let Ok(mut stored) = SYSTEM.write() {
        *stored = Some(cosmic::Theme::system(Arc::new(new_theme.clone())));
    }
    if let Ok(mut cached) = SYSTEM_RADIUS.write() {
        *cached = Some(new_theme.corner_radii.radius_m[0]);
    }
    if let Ok(mut cached) = THEME_CACHE.write() {
        *cached = None;
    }
    if let Ok(mut current) = CURRENT.write()
        && current.setting == "desktop"
    {
        current.sizes = None;
    }
    match theme(setting) {
        Some(theme) if window_mode => cosmic::command::set_theme(theme),
        _ => cosmic::Task::none(),
    }
}

pub fn sizes(setting: &str) -> Sizes {
    let r = match normalize(setting) {
        "slight" => 6.0,
        "rounded" => 12.0,
        "square" => 0.0,
        _ => desktop_radius(),
    };
    Sizes::new(r)
}

#[derive(Clone, Copy)]
struct CurrentState {
    setting: &'static str,
    look_radius: Option<f32>,
    sizes: Option<Sizes>,
}

static CURRENT: RwLock<CurrentState> = RwLock::new(CurrentState {
    setting: "desktop",
    look_radius: None,
    sizes: None,
});

/// The setting in effect, for the app's own drawing (bars, swatches, dots, badges).
pub fn set_current(setting: &str) {
    let norm = normalize(setting);
    let (look_radius, computed) = if norm == "desktop" {
        let lr = crate::look::window_radius();
        (lr, Sizes::new(lr.unwrap_or_else(system_radius)))
    } else {
        (None, sizes(norm))
    };
    if let Ok(mut current) = CURRENT.write() {
        *current = CurrentState { setting: norm, look_radius, sizes: Some(computed) };
    }
}

pub fn current() -> &'static str {
    CURRENT.read().map(|c| c.setting).unwrap_or("desktop")
}

/// The sizes in effect (see set_current).
pub fn now() -> Sizes {
    let cur = CURRENT.read().map(|c| *c).unwrap_or(CurrentState {
        setting: "desktop",
        look_radius: None,
        sizes: None,
    });
    if cur.setting != "desktop"
        && let Some(s) = cur.sizes
    {
        return s;
    }
    let lr = if cur.setting == "desktop" { crate::look::window_radius() } else { None };
    if let Some(s) = cur.sizes
        && cur.look_radius == lr
    {
        return s;
    }
    let computed = if cur.setting == "desktop" {
        Sizes::new(lr.unwrap_or_else(system_radius))
    } else {
        sizes(cur.setting)
    };
    if let Ok(mut current) = CURRENT.write()
        && current.setting == cur.setting
    {
        current.look_radius = lr;
        current.sizes = Some(computed);
    }
    computed
}

/// The theme's corner radii set to these sizes, so libcosmic's own widgets (buttons, menu rows,
/// toggles, fields, the popup container) follow too.
pub fn radii(sizes: Sizes) -> cosmic::cosmic_theme::CornerRadii {
    cosmic::cosmic_theme::CornerRadii {
        radius_0: [0.0; 4],
        radius_xs: [sizes.small; 4],
        radius_s: [sizes.control; 4],
        radius_m: [sizes.radius; 4],
        radius_l: [sizes.radius; 4],
        radius_xl: [sizes.pill; 4],
    }
}

struct CachedTheme {
    /// Kept alive so the pointer address cannot be reused.
    base: cosmic::Theme,
    sizes: Sizes,
    theme: cosmic::Theme,
}

static THEME_CACHE: RwLock<Option<CachedTheme>> = RwLock::new(None);

fn radii_eq(a: &cosmic::cosmic_theme::CornerRadii, b: &cosmic::cosmic_theme::CornerRadii) -> bool {
    a.radius_0 == b.radius_0
        && a.radius_xs == b.radius_xs
        && a.radius_s == b.radius_s
        && a.radius_m == b.radius_m
        && a.radius_l == b.radius_l
        && a.radius_xl == b.radius_xl
}

/// The desktop's theme (look.rs) with the chosen corners; None for "desktop" on COSMIC (the
/// theme libcosmic has stays as it is).
pub fn theme(setting: &str) -> Option<cosmic::Theme> {
    let norm = normalize(setting);
    let desktop = crate::look::theme();
    if norm == "desktop" && desktop.is_none() {
        return None;
    }
    let s = sizes(norm);
    let base = desktop.unwrap_or_else(system);
    let base_ptr: *const cosmic::cosmic_theme::Theme = base.cosmic();
    if let Ok(guard) = THEME_CACHE.read()
        && let Some(cached) = guard.as_ref()
        && std::ptr::eq(cached.base.cosmic(), base_ptr)
        && cached.sizes == s
    {
        return Some(cached.theme.clone());
    }
    let built = if matches!(base.theme_type, cosmic::theme::ThemeType::Custom(_))
        && radii_eq(&base.cosmic().corner_radii, &radii(s))
    {
        base.clone()
    } else {
        with_corners(base.clone(), s)
    };
    if let Ok(mut guard) = THEME_CACHE.write() {
        *guard = Some(CachedTheme { base, sizes: s, theme: built.clone() });
    }
    Some(built)
}

/// `base` with these corners, as a custom theme.
fn with_corners(mut theme: cosmic::Theme, sizes: Sizes) -> cosmic::Theme {
    let mut cosmic = theme.cosmic().clone();
    cosmic.corner_radii = radii(sizes);
    theme.theme_type = cosmic::theme::ThemeType::Custom(Arc::new(cosmic));
    theme
}

/// `content` drawn with the desktop's colours and the chosen corners (an applet popup: its
/// container draws the background).
pub fn apply<'a, M: 'a>(setting: &str, content: impl Into<cosmic::Element<'a, M>>) -> cosmic::Element<'a, M> {
    match theme(setting) {
        Some(theme) => cosmic::iced::widget::themer(Some(theme), content).into(),
        None => content.into(),
    }
}

/// The whole app's theme (a `--window`'s title bar too): the desktop's theme with the chosen
/// corners, else the system's; and the window's zoom for the desktop's text size
/// (look::text_scale). On start and whenever corners or the desktop's look change.
pub fn set_app_theme<M: Send + 'static>(setting: &str) -> cosmic::Task<cosmic::Action<M>> {
    cosmic::Task::batch([
        cosmic::command::set_theme(theme(setting).unwrap_or_else(cosmic::theme::system_preference)),
        cosmic::command::set_scaling_factor(crate::look::text_scale()),
    ])
}

/// [apply] for a whole window (`--window`): the window's background in the theme's colour too.
pub fn apply_window<'a, M: 'a>(setting: &str, content: impl Into<cosmic::Element<'a, M>>) -> cosmic::Element<'a, M> {
    match theme(setting) {
        Some(theme) => cosmic::iced::widget::themer(Some(theme), content)
            .background(crate::look::window_background)
            .text_color(|t: &cosmic::Theme| t.cosmic().on_bg_color().into())
            .into(),
        None => content.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_sizes() {
        assert_eq!(Sizes::new(16.0), Sizes { radius: 16.0, control: 8.0, small: 4.0, pill: 999.0 });
        assert_eq!(Sizes::new(6.0), Sizes { radius: 6.0, control: 6.0, small: 4.0, pill: 999.0 });
        assert_eq!(Sizes::new(12.0), Sizes { radius: 12.0, control: 8.0, small: 4.0, pill: 999.0 });
        assert_eq!(Sizes::new(2.0), Sizes { radius: 2.0, control: 2.0, small: 2.0, pill: 999.0 });
        assert_eq!(Sizes::new(0.0), Sizes { radius: 0.0, control: 0.0, small: 0.0, pill: 0.0 });
    }

    #[test]
    fn choices() {
        assert_eq!(radius("desktop", 16.0), 16.0);
        assert_eq!(radius("slight", 16.0), 6.0);
        assert_eq!(radius("rounded", 2.0), 12.0);
        assert_eq!(radius("square", 16.0), 0.0);
        assert_eq!(radius("bogus", 8.0), 8.0);
        assert_eq!(normalize(""), "desktop");
        let r = radii(Sizes::new(0.0));
        assert_eq!((r.radius_xs, r.radius_s, r.radius_m, r.radius_xl), ([0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4]));
        let r = radii(Sizes::new(12.0));
        assert_eq!((r.radius_xs[0], r.radius_s[0], r.radius_m[0], r.radius_l[0]), (4.0, 8.0, 12.0, 12.0));
        // (theme("desktop") depends on the session's desktop: look.rs's tests cover the colours)
        assert_eq!(theme("rounded").unwrap().cosmic().corner_radii.radius_m, [12.0; 4]);
    }

    #[test]
    fn corners_follow_a_new_system_theme() {
        // A custom theme with our corners is never the base: a COSMIC theme change (light → dark,
        // a new accent) reaches the window through system_theme_update.
        let light = cosmic::cosmic_theme::Theme::light_default();
        let accent = *light.accent_color();
        let _: cosmic::Task<cosmic::Action<()>> = system_theme_update("square", &light, true);
        let stored = SYSTEM.read().unwrap().clone().unwrap();
        assert!(matches!(stored.theme_type, cosmic::theme::ThemeType::System { .. }));
        let t = with_corners(stored, Sizes::new(0.0));
        assert!(!t.cosmic().is_dark);
        assert_eq!(*t.cosmic().accent_color(), accent);
        assert_eq!(t.cosmic().corner_radii.radius_m, [0.0; 4]);
        // The corners replace, the colours stay.
        let again = with_corners(t.clone(), Sizes::new(12.0));
        assert_eq!(again.cosmic().corner_radii.radius_m, [12.0; 4]);
        assert_eq!(again.cosmic().bg_color(), t.cosmic().bg_color());
    }

    #[test]
    fn current_sizes_cache_tracks_setting() {
        set_current("rounded");
        assert_eq!(current(), "rounded");
        assert_eq!(now(), Sizes::new(12.0));
        set_current("slight");
        assert_eq!(now(), Sizes::new(6.0));
        set_current("square");
        assert_eq!(now(), Sizes::new(0.0));
        set_current("desktop");
        assert_eq!(current(), "desktop");
        assert_eq!(now(), sizes("desktop"));
    }
}
