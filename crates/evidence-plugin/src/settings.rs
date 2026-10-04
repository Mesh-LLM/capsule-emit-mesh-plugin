//! The plugin's environment settings. Each is named `CAPSULES_<NAME>`.
//!
//! A setting the environment does not set is read from the value the
//! operator saved in mesh's console (Configuration > Plugins). mesh-llm keeps
//! that value in its own config file, under this plugin's `[[plugin]]` entry
//! (`[plugin.settings]`), and does not pass it to the plugin process, so the
//! plugin reads it there itself: the file `MESH_LLM_CONFIG` names, or
//! `~/.mesh-llm/config.toml`. The console key is the setting's name without
//! the prefix, lower-cased (`CAPSULES_SHARE_HISTORY_SEGMENTS` is
//! `share_history_segments`), except the witness: the console's `witness` is
//! `CAPSULES_CHECKPOINT_WITNESS_URLS`. The environment wins over the
//! console. A setting read once at start (the witness, the checkpoint
//! cadence) takes effect when mesh-llm is restarted.
//!
//! The plugin was called `capsule-emit-mesh` until 0.1.3. For one release the
//! old name, `CAPSULE_EMIT_MESH_<NAME>`, is still read when the new one is
//! unset, and a log line names the setting that was used; console settings
//! saved under the old plugin name are read the same way. When
//! both are set the new name wins and the log line says the old one was
//! ignored. Each setting is logged at most once per process.

use std::collections::HashSet;
use std::env::VarError;
use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};

/// The prefix every setting's name carries.
pub const PREFIX: &str = "CAPSULES_";

/// The prefix the same settings carried before; read for one release only.
pub const LEGACY_PREFIX: &str = "CAPSULE_EMIT_MESH_";

/// The old name of `name`, or `None` when `name` is not a plugin setting.
pub fn legacy_name(name: &str) -> Option<String> {
    name.strip_prefix(PREFIX)
        .map(|rest| format!("{LEGACY_PREFIX}{rest}"))
}

/// Which name a setting's value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Current,
    Legacy,
    /// Both names were set; the current one was used.
    CurrentOverLegacy,
}

/// Pick a value from the current and the old name's values.
fn pick<T>(current: Option<T>, legacy: Option<T>) -> Option<(T, Source)> {
    match (current, legacy) {
        (Some(value), Some(_)) => Some((value, Source::CurrentOverLegacy)),
        (Some(value), None) => Some((value, Source::Current)),
        (None, Some(value)) => Some((value, Source::Legacy)),
        (None, None) => None,
    }
}

fn log_once(name: &str, legacy: &str, source: Source) {
    static LOGGED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    if source == Source::Current {
        return;
    }
    let mut logged = LOGGED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !logged.insert(name.to_string()) {
        return;
    }
    match source {
        Source::Legacy => tracing::warn!(
            setting = legacy,
            renamed_to = name,
            "read the setting from its old name; rename it, the old name is read for one release only"
        ),
        Source::CurrentOverLegacy => tracing::warn!(
            setting = name,
            ignored = legacy,
            "both the setting and its old name are set; using the new name and ignoring the old one"
        ),
        Source::Current => {}
    }
}

/// This plugin's name in mesh's config file (`[[plugin]] name = ...`).
pub const PLUGIN_NAME: &str = "capsules";

/// The plugin's name before 0.1.3. Console settings saved under it are read
/// for one release, under the ones saved under [`PLUGIN_NAME`].
pub const OLD_PLUGIN_NAME: &str = "capsule-emit-mesh";

/// The console key a setting is saved under in mesh's config file.
pub fn console_key(name: &str) -> Option<String> {
    let rest = name.strip_prefix(PREFIX)?;
    Some(match rest {
        "CHECKPOINT_WITNESS_URLS" => "witness".to_string(),
        // Read from the same console list: each witness's `public_key`.
        "CHECKPOINT_WITNESS_KEYS" => "witness_keys".to_string(),
        other => other.to_ascii_lowercase(),
    })
}

/// mesh's config file: the one `MESH_LLM_CONFIG` names, or the default.
fn host_config_path() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("MESH_LLM_CONFIG") {
        return Some(path.into());
    }
    // Tests never read the developer's own mesh config.
    if cfg!(test) {
        return None;
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(
        std::path::Path::new(&home)
            .join(".mesh-llm")
            .join("config.toml"),
    )
}

/// The settings saved in the console for this plugin, from mesh's config
/// file. Read again whenever the file changes, so a setting read per use
/// follows the console without a restart.
fn console_settings() -> std::collections::BTreeMap<String, String> {
    type Cached = (
        std::path::PathBuf,
        Option<std::time::SystemTime>,
        u64,
        std::collections::BTreeMap<String, String>,
    );
    static CACHE: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();
    let Some(path) = host_config_path() else {
        return Default::default();
    };
    let meta = std::fs::metadata(&path).ok();
    let stamp = (
        meta.as_ref().and_then(|m| m.modified().ok()),
        meta.as_ref().map_or(0, |m| m.len()),
    );
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((p, modified, len, values)) = cache.as_ref() {
        if *p == path && (*modified, *len) == stamp {
            return values.clone();
        }
    }
    let values = std::fs::read_to_string(&path)
        .ok()
        .map(|raw| settings_from_config(&raw))
        .unwrap_or_default();
    *cache = Some((path, stamp.0, stamp.1, values.clone()));
    values
}

/// This plugin's `[plugin.settings]` from a mesh config file, as strings. A
/// file that does not parse yields nothing (the defaults stand).
fn settings_from_config(raw: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let Ok(doc) = raw.parse::<toml::Table>() else {
        tracing::warn!(
            "mesh's config file did not parse; the console's plugin settings are not read"
        );
        return out;
    };
    let Some(plugins) = doc.get("plugin").and_then(|p| p.as_array()) else {
        return out;
    };
    // The old name's entry first, so the current one's values win.
    let named = |name: &'static str| {
        plugins
            .iter()
            .filter(move |entry| entry.get("name").and_then(|n| n.as_str()) == Some(name))
    };
    for entry in named(OLD_PLUGIN_NAME).chain(named(PLUGIN_NAME)) {
        let Some(settings) = entry.get("settings").and_then(|s| s.as_table()) else {
            continue;
        };
        for (key, value) in settings {
            let text = match value {
                toml::Value::String(s) => s.clone(),
                toml::Value::Integer(n) => n.to_string(),
                toml::Value::Boolean(b) => b.to_string(),
                toml::Value::Float(f) => f.to_string(),
                // The witnesses: `{ endpoint, public_key }` rows (or bare
                // URLs), read as the environment's comma list of URLs plus a
                // JSON object of the keys given.
                toml::Value::Array(items) if key == "witness" => {
                    let (urls, keys) = witness_rows(items);
                    if !keys.is_empty() {
                        out.insert(
                            "witness_keys".to_string(),
                            serde_json::to_string(&keys).unwrap_or_default(),
                        );
                    }
                    urls.join(",")
                }
                toml::Value::Array(items) => items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .collect::<Vec<_>>()
                    .join(","),
                _ => continue,
            };
            out.insert(key.clone(), text);
        }
    }
    out
}

/// The console's witness rows: their endpoints in order, and the public key of
/// each row that gives one. A bare string row is an endpoint with no key.
fn witness_rows(
    items: &[toml::Value],
) -> (Vec<String>, std::collections::BTreeMap<String, String>) {
    let mut urls = Vec::new();
    let mut keys = std::collections::BTreeMap::new();
    for item in items {
        let (endpoint, key) = match item {
            toml::Value::String(url) => (url.as_str(), None),
            toml::Value::Table(row) => (
                row.get("endpoint").and_then(|v| v.as_str()).unwrap_or(""),
                row.get("public_key").and_then(|v| v.as_str()),
            ),
            _ => continue,
        };
        let endpoint = endpoint.trim();
        if endpoint.is_empty() {
            continue;
        }
        urls.push(endpoint.to_string());
        if let Some(key) = key.map(str::trim).filter(|k| !k.is_empty()) {
            keys.insert(endpoint.to_string(), key.to_string());
        }
    }
    (urls, keys)
}

/// The console's value for a setting, when the environment sets neither its
/// name nor its old name.
fn console_value(name: &str) -> Option<String> {
    let key = console_key(name)?;
    let value = console_settings().get(&key).cloned()?;
    if value.trim().is_empty() {
        return None;
    }
    static LOGGED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mut logged = LOGGED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if logged.insert(name.to_string()) {
        tracing::info!(setting = name, console_key = %key, "read the setting from the console (mesh's config file)");
    }
    Some(value)
}

/// Where a setting's value comes from: "env", "console", or `None` (unset).
pub fn origin(name: &str) -> Option<&'static str> {
    let in_env = std::env::var_os(name).is_some()
        || legacy_name(name).is_some_and(|l| std::env::var_os(l).is_some());
    if in_env {
        Some("env")
    } else if console_value(name).is_some() {
        Some("console")
    } else {
        None
    }
}

/// `std::env::var` for a plugin setting: `name` is the setting's current
/// name; its old name is read when `name` is unset, then the console's value.
pub fn var(name: &str) -> Result<String, VarError> {
    let Some(legacy) = legacy_name(name) else {
        return std::env::var(name);
    };
    let current = match std::env::var(name) {
        Ok(value) => Some(Ok(value)),
        Err(VarError::NotPresent) => None,
        Err(error) => Some(Err(error)),
    };
    let old = match std::env::var(&legacy) {
        Ok(value) => Some(Ok(value)),
        Err(VarError::NotPresent) => None,
        Err(error) => Some(Err(error)),
    };
    match pick(current, old) {
        Some((value, source)) => {
            log_once(name, &legacy, source);
            value
        }
        None => console_value(name).ok_or(VarError::NotPresent),
    }
}

/// `std::env::var_os` for a plugin setting, with the same fallback as [`var`].
pub fn var_os(name: &str) -> Option<OsString> {
    let Some(legacy) = legacy_name(name) else {
        return std::env::var_os(name);
    };
    match pick(std::env::var_os(name), std::env::var_os(&legacy)) {
        Some((value, source)) => {
            log_once(name, &legacy, source);
            Some(value)
        }
        None => console_value(name).map(OsString::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test uses its own setting names, so tests running in parallel never
    // see each other's variables.

    #[test]
    fn the_current_name_is_read() {
        let name = "CAPSULES_TEST_SETTINGS_CURRENT";
        std::env::set_var(name, "new");
        assert_eq!(var(name).as_deref(), Ok("new"));
        assert_eq!(var_os(name), Some(OsString::from("new")));
    }

    #[test]
    fn the_old_name_is_read_when_the_current_one_is_unset() {
        let name = "CAPSULES_TEST_SETTINGS_LEGACY";
        std::env::set_var("CAPSULE_EMIT_MESH_TEST_SETTINGS_LEGACY", "old");
        assert_eq!(var(name).as_deref(), Ok("old"));
        assert_eq!(var_os(name), Some(OsString::from("old")));
    }

    /// Reading a setting from its old name says so, once, naming both names.
    #[test]
    fn reading_the_old_name_is_logged_once_naming_both() {
        #[derive(Clone, Default)]
        struct Captured(std::sync::Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Captured {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        let name = "CAPSULES_TEST_SETTINGS_LOGGED";
        std::env::set_var("CAPSULE_EMIT_MESH_TEST_SETTINGS_LOGGED", "old");
        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(var(name).as_deref(), Ok("old"));
            assert_eq!(var(name).as_deref(), Ok("old"));
        });
        let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        assert_eq!(log.lines().count(), 1, "{log}");
        assert!(log.contains("CAPSULE_EMIT_MESH_TEST_SETTINGS_LOGGED"), "{log}");
        assert!(log.contains(name), "{log}");
        assert!(log.contains("one release"), "{log}");
    }

    #[test]
    fn the_current_name_wins_over_the_old_one() {
        let name = "CAPSULES_TEST_SETTINGS_BOTH";
        std::env::set_var(name, "new");
        std::env::set_var("CAPSULE_EMIT_MESH_TEST_SETTINGS_BOTH", "old");
        assert_eq!(var(name).as_deref(), Ok("new"));
        assert_eq!(var_os(name), Some(OsString::from("new")));
    }

    #[test]
    fn neither_name_set_is_not_present() {
        let name = "CAPSULES_TEST_SETTINGS_NEITHER";
        assert_eq!(var(name), Err(VarError::NotPresent));
        assert_eq!(var_os(name), None);
    }

    #[test]
    fn a_name_outside_the_plugin_prefix_has_no_fallback() {
        assert_eq!(legacy_name("HOME"), None);
        assert_eq!(
            legacy_name("CAPSULES_PEER_KEYS").as_deref(),
            Some("CAPSULE_EMIT_MESH_PEER_KEYS")
        );
    }

    #[test]
    fn pick_names_its_source() {
        assert_eq!(pick(Some(1), None), Some((1, Source::Current)));
        assert_eq!(pick(None, Some(2)), Some((2, Source::Legacy)));
        assert_eq!(pick(Some(1), Some(2)), Some((1, Source::CurrentOverLegacy)));
        assert_eq!(pick::<i32>(None, None), None);
    }

    #[test]
    fn console_keys_are_the_lower_cased_suffix_and_the_witness() {
        assert_eq!(
            console_key("CAPSULES_SHARE_HISTORY_SEGMENTS").as_deref(),
            Some("share_history_segments")
        );
        assert_eq!(
            console_key("CAPSULES_REFEREE_BAR_DAYS").as_deref(),
            Some("referee_bar_days")
        );
        assert_eq!(
            console_key("CAPSULES_CHECKPOINT_WITNESS_URLS").as_deref(),
            Some("witness")
        );
        assert_eq!(console_key("HOME"), None);
    }

    #[test]
    fn only_this_plugins_settings_are_read_from_the_config_file() {
        let raw = r#"
[[plugin]]
name = "other"
[plugin.settings]
witness = "https://other.example"

[[plugin]]
name = "capsules"
enabled = true
[plugin.settings]
witness = "https://witness.example"
share_history_segments = "peers"
referee_bar_days = 7
adjudicate_differing_twins = "off"
"#;
        let s = settings_from_config(raw);
        assert_eq!(
            s.get("witness").map(String::as_str),
            Some("https://witness.example")
        );
        assert_eq!(
            s.get("share_history_segments").map(String::as_str),
            Some("peers")
        );
        assert_eq!(s.get("referee_bar_days").map(String::as_str), Some("7"));
        assert_eq!(
            s.get("adjudicate_differing_twins").map(String::as_str),
            Some("off")
        );
        assert!(settings_from_config("not [valid").is_empty());
        assert!(settings_from_config("").is_empty());
    }

    #[test]
    fn console_settings_saved_under_the_old_plugin_name_are_read_under_the_new_ones() {
        let raw = r#"
[[plugin]]
name = "capsule-emit-mesh"
[plugin.settings]
witness = "https://old.example"
referee_bar_days = 7

[[plugin]]
name = "capsules"
[plugin.settings]
witness = "https://new.example"
"#;
        let s = settings_from_config(raw);
        assert_eq!(s.get("witness").map(String::as_str), Some("https://new.example"));
        assert_eq!(s.get("referee_bar_days").map(String::as_str), Some("7"));
        let only_old = settings_from_config(
            "[[plugin]]\nname = \"capsule-emit-mesh\"\n[plugin.settings]\nwitness = \"https://old.example\"\n",
        );
        assert_eq!(only_old.get("witness").map(String::as_str), Some("https://old.example"));
    }

    #[test]
    fn a_list_in_the_console_is_read_as_the_comma_list() {
        let raw = r#"
[[plugin]]
name = "capsules"
[plugin.settings]
witness = ["https://a.example", " https://b.example ", ""]
"#;
        assert_eq!(
            settings_from_config(raw).get("witness").map(String::as_str),
            Some("https://a.example,https://b.example")
        );
    }

    #[test]
    fn witness_rows_give_the_url_list_and_the_keys_given() {
        let raw = r#"
[[plugin]]
name = "capsules"
[plugin.settings]
witness = [
  { endpoint = "https://a.example", public_key = "AB01" },
  { endpoint = "https://b.example" },
  "https://c.example",
  { public_key = "no endpoint" },
]
"#;
        let s = settings_from_config(raw);
        assert_eq!(
            s.get("witness").map(String::as_str),
            Some("https://a.example,https://b.example,https://c.example")
        );
        assert_eq!(
            s.get("witness_keys").map(String::as_str),
            Some(r#"{"https://a.example":"AB01"}"#)
        );
        assert_eq!(
            console_key("CAPSULES_CHECKPOINT_WITNESS_KEYS").as_deref(),
            Some("witness_keys")
        );
    }
}
