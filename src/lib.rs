pub mod i18n;
pub mod shot;
pub mod corners;
pub mod look;
use i18n::{tr, trf};
use serde_json::Value;
use std::path::PathBuf;
pub const TITLE: &str = "Maprisma Hue Lights";
pub const ICON: &str = "io.github.ubruckhaus.CosmicMaprismaHueLights-symbolic";
/// The bulb when no light is on (data/icons/hue-bulb-off-symbolic.svg, installed under this name).
pub const ICON_OFF: &str = "io.github.ubruckhaus.CosmicMaprismaHueLights-off-symbolic";

/// The lit bulb in the theme's green (data/icons/hue-bulb-active-symbolic.svg, class
/// ColorScheme-PositiveText): every Maprisma bar icon is "on" in this one colour.
pub const ICON_ACTIVE: &str = "io.github.ubruckhaus.CosmicMaprismaHueLights-active-symbolic";

/// The bar's bulb, as the applet draws it: lit and green while any light is on.
pub fn icon_for(stats: &Value) -> &'static str {
    if number(stats, "on_count") > 0.0 { ICON_ACTIVE } else { ICON_OFF }
}
pub const WINDOW_BUS: &str = "io.github.ubruckhaus.CosmicMaprismaHueLights.Window";
pub fn script_path() -> Option<PathBuf> {
    let name = "hue-lights.py";
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("HUE_LIGHTS_SCRIPTS") {
        dirs.push(dir.into());
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(bin) = exe.parent()
    {
        dirs.push(bin.join("../share/maprisma-hue-lights/scripts"));
    }
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    if let Some(data) = data_home {
        dirs.push(data.join("maprisma-hue-lights/scripts"));
    }
    dirs.push("/usr/share/maprisma-hue-lights/scripts".into());
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

pub fn summary(stats: &Value) -> String {
    match stats["phase"].as_str().unwrap_or("") {
        "unpaired" | "pairing" => tr("No bridge paired yet: click to pair"),
        "error" => match stats["error"].as_str().unwrap_or("") {
            "" => tr("Bridge not reachable"),
            _ => i18n::error(stats),
        },
        _ => trf("{0} of {1} on", &[number(stats, "on_count").to_string(), number(stats, "light_count").to_string()]),
    }
}
fn number(v: &Value, k: &str) -> f64 {
    v[k].as_f64().filter(|v| v.is_finite()).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn tray_bulb_follows_the_lights() {
        assert_eq!(icon_for(&json!({"phase":"ok","on_count":2,"light_count":3})), ICON_ACTIVE);
        assert_eq!(icon_for(&json!({"phase":"ok","on_count":0,"light_count":3})), ICON_OFF);
        assert_eq!(icon_for(&json!({"phase":"error"})), ICON_OFF);
        assert_eq!(icon_for(&Value::Null), ICON_OFF);
    }
    #[test]
    fn tooltip_matches_panel_states() {
        assert_eq!(
            summary(&json!({"phase":"unpaired"})),
            "No bridge paired yet: click to pair"
        );
        assert_eq!(
            summary(&json!({"phase":"pairing"})),
            "No bridge paired yet: click to pair"
        );
        assert_eq!(summary(&json!({"phase":"error"})), "Bridge not reachable");
        assert_eq!(
            summary(&json!({"phase":"error", "error":"Timed out"})),
            "Timed out"
        );
        assert_eq!(
            summary(&json!({"phase":"ok", "on_count":3,"light_count":8})),
            "3 of 8 on"
        );
        assert_eq!(summary(&Value::Null), "0 of 0 on");
    }
}
