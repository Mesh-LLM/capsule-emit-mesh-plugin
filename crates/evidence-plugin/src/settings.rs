//! The plugin's environment settings. Each is named `CAPSULE_EMIT_MESH_<NAME>`.
//!
//! For one release the old name, `ADMISSION_POLICY_<NAME>`, is still read when
//! the new one is unset, and a log line names the setting that was used. When
//! both are set the new name wins and the log line says the old one was
//! ignored. Each setting is logged at most once per process.

use std::collections::HashSet;
use std::env::VarError;
use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};

/// The prefix every setting's name carries.
pub const PREFIX: &str = "CAPSULE_EMIT_MESH_";

/// The prefix the same settings carried before; read for one release only.
pub const LEGACY_PREFIX: &str = "ADMISSION_POLICY_";

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

/// `std::env::var` for a plugin setting: `name` is the setting's current
/// name; its old name is read when `name` is unset.
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
        None => Err(VarError::NotPresent),
    }
}

/// `std::env::var_os` for a plugin setting, with the same fallback as [`var`].
pub fn var_os(name: &str) -> Option<OsString> {
    let Some(legacy) = legacy_name(name) else {
        return std::env::var_os(name);
    };
    let (value, source) = pick(std::env::var_os(name), std::env::var_os(&legacy))?;
    log_once(name, &legacy, source);
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test uses its own setting names, so tests running in parallel never
    // see each other's variables.

    #[test]
    fn the_current_name_is_read() {
        let name = "CAPSULE_EMIT_MESH_TEST_SETTINGS_CURRENT";
        std::env::set_var(name, "new");
        assert_eq!(var(name).as_deref(), Ok("new"));
        assert_eq!(var_os(name), Some(OsString::from("new")));
    }

    #[test]
    fn the_old_name_is_read_when_the_current_one_is_unset() {
        let name = "CAPSULE_EMIT_MESH_TEST_SETTINGS_LEGACY";
        std::env::set_var("ADMISSION_POLICY_TEST_SETTINGS_LEGACY", "old");
        assert_eq!(var(name).as_deref(), Ok("old"));
        assert_eq!(var_os(name), Some(OsString::from("old")));
    }

    #[test]
    fn the_current_name_wins_over_the_old_one() {
        let name = "CAPSULE_EMIT_MESH_TEST_SETTINGS_BOTH";
        std::env::set_var(name, "new");
        std::env::set_var("ADMISSION_POLICY_TEST_SETTINGS_BOTH", "old");
        assert_eq!(var(name).as_deref(), Ok("new"));
        assert_eq!(var_os(name), Some(OsString::from("new")));
    }

    #[test]
    fn neither_name_set_is_not_present() {
        let name = "CAPSULE_EMIT_MESH_TEST_SETTINGS_NEITHER";
        assert_eq!(var(name), Err(VarError::NotPresent));
        assert_eq!(var_os(name), None);
    }

    #[test]
    fn a_name_outside_the_plugin_prefix_has_no_fallback() {
        assert_eq!(legacy_name("HOME"), None);
        assert_eq!(
            legacy_name("CAPSULE_EMIT_MESH_PEER_KEYS").as_deref(),
            Some("ADMISSION_POLICY_PEER_KEYS")
        );
    }

    #[test]
    fn pick_names_its_source() {
        assert_eq!(pick(Some(1), None), Some((1, Source::Current)));
        assert_eq!(pick(None, Some(2)), Some((2, Source::Legacy)));
        assert_eq!(pick(Some(1), Some(2)), Some((1, Source::CurrentOverLegacy)));
        assert_eq!(pick::<i32>(None, None), None);
    }
}
