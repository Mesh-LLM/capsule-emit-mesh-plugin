//! Where the plugin keeps its signing key, ledger and logs.
//!
//! The host starts the plugin with no data directory of its own, and a
//! service manager starts the host with any working directory (often `/`).
//! A directory relative to the working directory would then move with it,
//! and a node started from somewhere else would mint a NEW key and a second,
//! forked chain. So the directory is always absolute:
//!
//! 1. `ADMISSION_POLICY_DATA_DIR`, made absolute at startup;
//! 2. else `$XDG_DATA_HOME/capsule-emit-mesh`;
//! 3. else `$HOME/.local/share/capsule-emit-mesh`.
//!
//! With none of these the plugin refuses to start. It also refuses to mint a
//! key where one is expected: a ledger with records but no key, or a key left
//! in the old default (`./admission-policy-data`) while the new directory has
//! none.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

pub const ENV_DATA_DIR: &str = "ADMISSION_POLICY_DATA_DIR";
const APP_DIR: &str = "capsule-emit-mesh";
/// The default before the directory was absolute, relative to the working
/// directory.
const LEGACY_RELATIVE_DIR: &str = "admission-policy-data";
const NODE_KEY: &str = "keys/node-key.pem";
const LEDGER: &str = "ledger/capsules.jsonl";

/// The data directory for these inputs, and whether the operator chose it.
fn resolve(
    env_dir: Option<&str>,
    xdg_data_home: Option<&str>,
    home: Option<&str>,
    cwd: &Path,
) -> anyhow::Result<(PathBuf, bool)> {
    let non_empty = |v: Option<&str>| {
        v.map(str::trim)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = non_empty(env_dir) {
        let dir = if dir.is_absolute() {
            dir
        } else {
            cwd.join(dir)
        };
        return Ok((dir, true));
    }
    if let Some(xdg) = non_empty(xdg_data_home).filter(|p| p.is_absolute()) {
        return Ok((xdg.join(APP_DIR), false));
    }
    if let Some(home) = non_empty(home).filter(|p| p.is_absolute()) {
        return Ok((home.join(".local/share").join(APP_DIR), false));
    }
    bail!("no data directory: set {ENV_DATA_DIR} to an absolute path (HOME is not set)")
}

/// Refuse to start where starting would silently mint a second key.
fn check_no_second_key(dir: &Path, chosen: bool, cwd: &Path) -> anyhow::Result<()> {
    if dir.join(NODE_KEY).exists() {
        return Ok(());
    }
    let ledger_has_records = std::fs::metadata(dir.join(LEDGER)).is_ok_and(|m| m.len() > 0);
    if ledger_has_records {
        bail!(
            "{} holds a ledger but no signing key ({NODE_KEY}); refusing to start with a new key, \
             which would fork this node's chain. Restore the key, or point {ENV_DATA_DIR} elsewhere.",
            dir.display()
        );
    }
    let legacy = cwd.join(LEGACY_RELATIVE_DIR);
    if !chosen && legacy.join(NODE_KEY).exists() {
        bail!(
            "found this node's key under {} (the old default, relative to the working directory) \
             and none under {}; refusing to start with a new key. Set {ENV_DATA_DIR}={} to keep it.",
            legacy.display(),
            dir.display(),
            legacy.display()
        );
    }
    Ok(())
}

/// The plugin's data directory, absolute, checked.
pub fn data_dir() -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir().context("read the working directory")?;
    let (dir, chosen) = resolve(
        std::env::var(ENV_DATA_DIR).ok().as_deref(),
        std::env::var("XDG_DATA_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        &cwd,
    )?;
    check_no_second_key(&dir, chosen, &cwd)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("data-dir-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_directory_is_absolute_whatever_the_working_directory() {
        let root = Path::new("/");
        let (d, chosen) = resolve(None, None, Some("/home/op"), root).unwrap();
        assert_eq!(d, PathBuf::from("/home/op/.local/share/capsule-emit-mesh"));
        assert!(!chosen);
        let (d, _) = resolve(None, Some("/var/xdg"), Some("/home/op"), root).unwrap();
        assert_eq!(d, PathBuf::from("/var/xdg/capsule-emit-mesh"));
        let (d, chosen) =
            resolve(Some("state"), None, Some("/home/op"), Path::new("/srv")).unwrap();
        assert_eq!(d, PathBuf::from("/srv/state"));
        assert!(chosen);
        let (d, _) = resolve(Some("/data/cem"), None, None, root).unwrap();
        assert_eq!(d, PathBuf::from("/data/cem"));
        // A relative XDG_DATA_HOME is ignored, as the spec says.
        let (d, _) = resolve(None, Some("rel"), Some("/home/op"), root).unwrap();
        assert_eq!(d, PathBuf::from("/home/op/.local/share/capsule-emit-mesh"));
    }

    #[test]
    fn no_home_and_no_setting_refuses_rather_than_using_the_working_directory() {
        assert!(resolve(None, None, None, Path::new("/")).is_err());
        assert!(resolve(Some("  "), None, Some(""), Path::new("/")).is_err());
    }

    #[test]
    fn a_ledger_without_its_key_refuses_to_mint_a_new_one() {
        let dir = tmp("ledger-no-key");
        std::fs::create_dir_all(dir.join("ledger")).unwrap();
        std::fs::write(dir.join(LEDGER), b"{}\n").unwrap();
        assert!(check_no_second_key(&dir, true, Path::new("/")).is_err());
        std::fs::create_dir_all(dir.join("keys")).unwrap();
        std::fs::write(dir.join(NODE_KEY), b"pem").unwrap();
        assert!(check_no_second_key(&dir, true, Path::new("/")).is_ok());
    }

    #[test]
    fn a_key_left_in_the_old_default_is_never_silently_replaced() {
        let cwd = tmp("legacy-cwd");
        std::fs::create_dir_all(cwd.join(LEGACY_RELATIVE_DIR).join("keys")).unwrap();
        std::fs::write(cwd.join(LEGACY_RELATIVE_DIR).join(NODE_KEY), b"pem").unwrap();
        let fresh = tmp("legacy-new");
        assert!(check_no_second_key(&fresh, false, &cwd).is_err());
        // An operator who chose the directory has decided.
        assert!(check_no_second_key(&fresh, true, &cwd).is_ok());
        // A fresh node with nothing anywhere starts.
        assert!(check_no_second_key(&fresh, false, &tmp("empty-cwd")).is_ok());
    }
}
