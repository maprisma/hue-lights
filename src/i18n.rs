//! Bundled gettext-style catalogs. UI preferences are separate from bridge credentials.
use serde_json::Value;
use std::{path::PathBuf, sync::LazyLock};
pub const LANGUAGES: [(&str, &str); 8] = [("en", "English"), ("de", "Deutsch"), ("fr", "Français"), ("es", "Español"), ("it", "Italiano"), ("pt", "Português"), ("nl", "Nederlands"), ("pl", "Polski")];
static CATALOGS: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("../translations/bundled.json")).expect("bundled catalogs"));
pub fn system_language(values: [&str; 3]) -> &'static str {
    let first = values.into_iter().find(|v| !v.is_empty()).unwrap_or("");
    let code = first.split(['_', '.', '@', '-']).next().unwrap_or("").to_lowercase();
    LANGUAGES.iter().find(|(c, _)| *c == code).map_or("en", |(c, _)| c)
}
pub fn system() -> &'static str {
    let values = ["LC_ALL", "LC_MESSAGES", "LANG"].map(|k| std::env::var(k).unwrap_or_default());
    system_language([&values[0], &values[1], &values[2]])
}
fn path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
        .join("maprisma-hue-lights/config.json")
}
pub fn setting() -> String {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["language"].as_str().map(str::to_owned)).filter(|c| c == "auto" || LANGUAGES.iter().any(|(code, _)| code == c)).unwrap_or_else(|| "auto".into())
}
pub fn save(code: &str) -> std::io::Result<()> {
    if code != "auto" && !LANGUAGES.iter().any(|(c, _)| *c == code) { return Err(std::io::ErrorKind::InvalidInput.into()); }
    let path = path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut config = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).filter(Value::is_object).unwrap_or_else(|| serde_json::json!({}));
    config["language"] = code.into();
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temp, serde_json::to_vec_pretty(&config)?)?;
    std::fs::rename(temp, path)
}
/// One interface setting from config.json (language, screenshot_key, corners); bridge
/// credentials live elsewhere (the helper's own file).
pub fn read_pref(key: &str) -> Option<Value> {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v.get(key).cloned())
}
/// Sets (Some) or removes (None) one setting; the other keys stay as they are.
pub fn write_pref(key: &str, value: Option<Value>) -> std::io::Result<()> {
    let path = path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut config = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).filter(Value::is_object).unwrap_or_else(|| serde_json::json!({}));
    match value {
        Some(value) => config[key] = value,
        None => { config.as_object_mut().map(|o| o.remove(key)); }
    }
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temp, serde_json::to_vec_pretty(&config)?)?;
    std::fs::rename(temp, path)
}
pub fn translate(code: &str, key: &str) -> String {
    let value = &CATALOGS[code][key];
    value.as_str().or_else(|| value.as_array().and_then(|a| a.last()).and_then(Value::as_str)).unwrap_or(key).to_owned()
}
pub fn tr(key: &str) -> String {
    let setting = setting();
    translate(if setting == "auto" { system() } else { &setting }, key)
}
pub fn trf(key: &str, args: &[String]) -> String {
    let setting = setting();
    let code = if setting == "auto" { system() } else { &setting };
    let value = &CATALOGS[code][key];
    let text = if code == "pl" && value.is_array() {
        let n = args.first().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        let few = (2..=4).contains(&(n % 10)) && !(12..=14).contains(&(n % 100));
        value[if few { 0 } else { 1 }].as_str().unwrap_or(key).to_owned()
    } else { translate(code, key) };
    format(&text, args)
}
fn format(text: &str, args: &[String]) -> String {
    // One pass: inserted values containing placeholders must remain unchanged.
    let mut result = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        result.push_str(&rest[..start]);
        rest = &rest[start..];
        if let Some(end) = rest.find('}') {
            if let Ok(i) = rest[1..end].parse::<usize>() {
                if let Some(arg) = args.get(i) { result.push_str(arg); rest = &rest[end + 1..]; continue; }
            }
        }
        result.push('{'); rest = &rest[1..];
    }
    result.push_str(rest); result
}
pub fn choices() -> Vec<(String, String)> {
    let name = LANGUAGES.iter().find(|(c, _)| *c == system()).unwrap().1;
    let mut choices = vec![("auto".into(), trf("System ({0})", &[name.into()]))];
    choices.extend(LANGUAGES.iter().map(|(c, n)| (c.to_string(), n.to_string())));
    choices
}
pub fn error(stats: &Value) -> String {
    let args: Vec<String> = stats["error_args"].as_array().map(|a| a.iter().map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).collect()).unwrap_or_default();
    let key = match stats["error_code"].as_str().unwrap_or("") {
        "errPairFailed" => trf("Pairing failed: {0}", &args),
        "errCannotReach" => trf("Cannot reach {0} ({1})", &args),
        "errReply" => trf("The bridge sent a reply this app does not understand.", &args),
        "errInternal" => trf("Something went wrong ({0})", &args),
        "errCommand" => trf("Command failed ({0})", &args),
        "errEnterIp" => trf("Enter the bridge's local IPv4 address", &args),
        "errNoPress" => trf("No button press seen. Press the round button on the bridge, then try again.", &args),
        "errCert" => trf("The bridge certificate changed. Unpair and pair again if you replaced the bridge.", &args),
        "errRejected" => trf("The bridge no longer accepts this app. Unpair and pair again.", &args),
        "errUnreachable" => trf("Bridge not reachable ({0})", &args),
        "errNotHue" => trf("{0} does not look like a Hue bridge", &args),
        _ => tr(stats["error"].as_str().unwrap_or("")),
    };
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locale_precedence_and_fallback() {
        for (values, expected) in [
            (["de_DE.UTF-8", "fr", "pl"], "de"),
            (["", "fr-FR", "de"], "fr"),
            (["", "", "pt_BR.UTF-8"], "pt"),
            (["zz_ZZ", "de", "fr"], "en"),
            (["C.UTF-8", "de", "fr"], "en"),
            (["POSIX", "de", "fr"], "en"),
            (["", "", ""], "en"),
            (["PL_pl@x", "", ""], "pl"),
        ] { assert_eq!(system_language(values), expected); }
    }
    #[test]
    fn fallback_and_values_are_preserved() {
        assert_eq!(translate("unknown", "Unlisted"), "Unlisted");
        assert_eq!(translate("de", "Language"), "Sprache");
        assert_eq!(format("{0} {1}", &["{1}".into(), "Hue".into()]), "{1} Hue");
    }
}
