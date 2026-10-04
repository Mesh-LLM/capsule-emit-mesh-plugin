//! Where the plugin keeps its signing key, ledger and logs.
//!
//! The host starts the plugin with no data directory of its own, and a
//! service manager starts the host with any working directory (often `/`).
//! A directory relative to the working directory would then move with it,
//! and a node started from somewhere else would mint a NEW key and a second,
//! forked chain. So the directory is always absolute:
//!
//! 1. `CAPSULES_DATA_DIR`, made absolute at startup;
//! 2. else `$XDG_DATA_HOME/capsules`;
//! 3. else `$HOME/.local/share/capsules`.
//!
//! The plugin was called `capsule-emit-mesh` before 0.1.3, and its default
//! directory carried that name. A node that has one is moved to the new name
//! once (`move_old_default`): a single rename, with the ledger's chain verified
//! before and after. A node with both directories refuses to start and says
//! why, rather than pick one or merge them.
//!
//! With none of these the plugin refuses to start. It also refuses to mint a
//! key where one is expected: a ledger with records but no key, or a key left
//! in the old default (`./admission-policy-data`) while the new directory has
//! none.
//!
//! One process per directory (`lock`): the ledger is a hash chain with a
//! single writer, so a second plugin process on the same directory refuses to
//! start rather than interleave appends and break the chain.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

pub const ENV_DATA_DIR: &str = "CAPSULES_DATA_DIR";
const APP_DIR: &str = "capsules";
/// The default directory's name before the plugin was renamed `capsules`.
const OLD_APP_DIR: &str = "capsule-emit-mesh";
/// The default before the directory was absolute, relative to the working
/// directory.
const LEGACY_RELATIVE_DIR: &str = "admission-policy-data";
const NODE_KEY: &str = "keys/node-key.pem";
const LEDGER: &str = "ledger/capsules.jsonl";
/// Holds the advisory lock; its contents (the holder's pid) are only for the
/// refusal message.
const LOCK_FILE: &str = "capsules.lock";

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

/// Whether `dir` holds a node: its signing key or its ledger.
fn holds_node(dir: &Path) -> bool {
    dir.join(NODE_KEY).exists() || dir.join(LEDGER).exists()
}

/// What a ledger's chain verifies to: its entry count and its head.
#[derive(Debug, PartialEq, Eq)]
struct ChainCheck {
    entries: u64,
    head: Option<String>,
}

/// Open the ledger under `dir`, which checks every record's id, its link to
/// the record before it and its signed statement. `None` when `dir` has no
/// ledger.
fn verify_chain(dir: &Path) -> anyhow::Result<Option<ChainCheck>> {
    if !dir.join(LEDGER).exists() {
        return Ok(None);
    }
    let ledger_dir = dir.join(LEDGER).parent().map(Path::to_path_buf).unwrap_or_default();
    let (ledger, _) = crate::producer::index::open_ledger(&ledger_dir)
        .map_err(|e| anyhow::anyhow!("{e:?}"))
        .with_context(|| format!("verify the ledger in {}", dir.display()))?;
    Ok(Some(ChainCheck {
        entries: ledger.entries(),
        head: ledger.chain_head().map(str::to_string),
    }))
}

/// A directory move this start made, for the log.
#[derive(Debug)]
struct Moved {
    from: PathBuf,
    chain: Option<ChainCheck>,
}

/// Move the default directory from the plugin's old name to `dir`, once.
///
/// Only the default directory moves: one the operator chose
/// (`CAPSULES_DATA_DIR`, or the old setting name) is used where it is. The
/// move is a single `rename` within one parent directory, so there is never a
/// second copy of the key or the ledger. The ledger's chain is verified
/// before the move and again after it; if the two disagree the move is undone
/// and the plugin refuses to start.
fn move_old_default(dir: &Path, chosen: bool) -> anyhow::Result<Option<Moved>> {
    if chosen {
        return Ok(None);
    }
    let Some(old) = dir.parent().map(|parent| parent.join(OLD_APP_DIR)) else {
        return Ok(None);
    };
    if !holds_node(&old) {
        return Ok(None);
    }
    if holds_node(dir) {
        bail!(
            "found a node under both {} (the plugin's old name) and {}; refusing to pick one. \
             Keep the one whose key and records are this node's, move the other out of the way, \
             and start again",
            old.display(),
            dir.display()
        );
    }
    if dir.exists() {
        // An empty directory is in the way of the rename; anything else is not
        // ours to remove.
        std::fs::remove_dir(dir).map_err(|_| {
            anyhow::anyhow!(
                "{} exists and is not empty, so {} cannot be moved there; \
                 move {} out of the way and start again",
                dir.display(),
                old.display(),
                dir.display()
            )
        })?;
    }
    // A second process doing the same move waits on this lock, then finds
    // nothing left to move.
    let held = lock(&old)?;
    let before = verify_chain(&old)?;
    std::fs::rename(&old, dir)
        .with_context(|| format!("move {} to {}", old.display(), dir.display()))?;
    drop(held);
    let after = match verify_chain(dir) {
        Ok(after) if after == before => after,
        result => {
            let undo = std::fs::rename(dir, &old);
            bail!(
                "moved {} to {}, but the ledger no longer verifies the same \
                 (before {before:?}, after {result:?}); {}",
                old.display(),
                dir.display(),
                match undo {
                    Ok(()) => format!("moved it back to {}", old.display()),
                    Err(e) => format!("could not move it back ({e}); it is at {}", dir.display()),
                }
            );
        }
    };
    Ok(Some(Moved {
        from: old,
        chain: after,
    }))
}

/// The plugin's data directory, absolute, checked.
pub fn data_dir() -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir().context("read the working directory")?;
    let (dir, chosen) = resolve(
        crate::settings::var(ENV_DATA_DIR).ok().as_deref(),
        std::env::var("XDG_DATA_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        &cwd,
    )?;
    if let Some(moved) = move_old_default(&dir, chosen)? {
        let (entries, head) = moved
            .chain
            .map_or((0, None), |c| (c.entries, c.head));
        tracing::warn!(
            from = %moved.from.display(),
            to = %dir.display(),
            ledger_entries = entries,
            chain_head = head.as_deref().unwrap_or("none"),
            "moved the data directory to the plugin's new name; the ledger verified the same after the move"
        );
    }
    check_no_second_key(&dir, chosen, &cwd)?;
    Ok(dir)
}

/// This process's exclusive hold on the data directory, for its lifetime.
#[must_use = "the directory is locked only while this is held"]
pub struct DataDirLock {
    _file: File,
}

/// Take the data directory for this process alone.
///
/// An advisory OS lock (`flock` on Unix, `LockFileEx` on Windows) on
/// `<dir>/capsules.lock`. The OS releases it when the process exits
/// or crashes, so a lock file a crash left behind never blocks the next start;
/// it is not a pid file. A second process on the same directory gets one plain
/// line naming the directory and the holder's pid.
pub fn lock(dir: &Path) -> anyhow::Result<DataDirLock> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join(LOCK_FILE);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => {
            file.set_len(0)
                .and_then(|()| writeln!(file, "{}", std::process::id()))
                .with_context(|| format!("write {}", path.display()))?;
            Ok(DataDirLock { _file: file })
        }
        Err(TryLockError::WouldBlock) => {
            let mut holder = String::new();
            let _ = file.read_to_string(&mut holder);
            let holder = match holder.trim() {
                "" => "unknown",
                pid => pid,
            };
            bail!(
                "data directory {} is in use by another capsules plugin process (pid {holder}); give each node its own {ENV_DATA_DIR}",
                dir.display()
            )
        }
        Err(TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("lock {}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_lock_on_the_same_directory_is_refused_naming_it_and_the_holder() {
        let dir = tmp("lock-second");
        let _held = lock(&dir).expect("first lock");
        let message = lock(&dir).err().expect("second lock refused").to_string();
        assert!(message.contains(&dir.display().to_string()), "{message}");
        assert!(
            message.contains(&format!("pid {}", std::process::id())),
            "{message}"
        );
        assert!(!message.contains('\n'), "one line: {message}");
    }

    #[test]
    fn the_directory_is_free_again_once_the_holder_lets_go() {
        let dir = tmp("lock-release");
        drop(lock(&dir).expect("first lock"));
        let _again = lock(&dir).expect("a new holder after the first let go");
    }

    #[test]
    fn a_lock_file_left_by_a_crash_does_not_block() {
        let dir = tmp("lock-stale");
        std::fs::write(dir.join(LOCK_FILE), "999999\n").unwrap();
        let _held = lock(&dir).expect("a stale lock file is not a lock");
        assert_eq!(
            std::fs::read_to_string(dir.join(LOCK_FILE)).unwrap(),
            format!("{}\n", std::process::id())
        );
    }

    /// `<parent>/capsule-emit-mesh` holding a node with `records` sealed
    /// records, as 0.1.2 left it; `<parent>/capsules` is where it belongs.
    fn old_node(label: &str, records: usize) -> (PathBuf, PathBuf) {
        let parent = tmp(label);
        let old = parent.join(OLD_APP_DIR);
        let state = crate::capsule_emit::CapsuleState::open(&old, "node-under-test").unwrap();
        for i in 0..records {
            let id = format!("e-{i}");
            state
                .emit_for_exchange(&crate::capsule_emit::ExchangeRecord {
                    model: "m",
                    client_nonce: None,
                    request_bytes: b"{}",
                    response_bytes: b"{}",
                    latency_ms: 1.0,
                    exchange_id: Some(&id),
                    requesting_party: Some("party"),
                    host_provenance: None,
                })
                .unwrap();
        }
        (old, parent.join(APP_DIR))
    }

    #[test]
    fn a_fresh_node_moves_nothing() {
        let parent = tmp("move-fresh");
        assert!(move_old_default(&parent.join(APP_DIR), false).unwrap().is_none());
        assert!(!parent.join(OLD_APP_DIR).exists());
    }

    #[test]
    fn the_old_default_moves_whole_and_its_chain_verifies_the_same() {
        let (old, new) = old_node("move-ledger", 3);
        let key = std::fs::read(old.join(NODE_KEY)).unwrap();
        let before = verify_chain(&old).unwrap().expect("a ledger");
        assert_eq!(before.entries, 3);

        let moved = move_old_default(&new, false).unwrap().expect("moved");
        assert_eq!(moved.from, old);
        assert_eq!(moved.chain.as_ref(), Some(&before));
        assert!(!old.exists(), "one copy only: the old directory is gone");
        assert_eq!(std::fs::read(new.join(NODE_KEY)).unwrap(), key, "the same key");
        assert_eq!(verify_chain(&new).unwrap(), Some(before));
        // The next start finds nothing to move.
        assert!(move_old_default(&new, false).unwrap().is_none());
    }

    #[test]
    fn an_empty_new_directory_does_not_block_the_move() {
        let (old, new) = old_node("move-empty-new", 1);
        std::fs::create_dir_all(&new).unwrap();
        move_old_default(&new, false).unwrap().expect("moved");
        assert!(!old.exists());
        assert!(new.join(NODE_KEY).exists());
    }

    #[test]
    fn a_node_under_both_names_is_refused_and_neither_is_touched() {
        let (old, new) = old_node("move-both", 1);
        crate::capsule_emit::CapsuleState::open(&new, "other").unwrap();
        let old_key = std::fs::read(old.join(NODE_KEY)).unwrap();
        let new_key = std::fs::read(new.join(NODE_KEY)).unwrap();
        let message = move_old_default(&new, false).unwrap_err().to_string();
        assert!(message.contains("both"), "{message}");
        assert!(message.contains(&old.display().to_string()), "{message}");
        assert!(message.contains(&new.display().to_string()), "{message}");
        assert_eq!(std::fs::read(old.join(NODE_KEY)).unwrap(), old_key);
        assert_eq!(std::fs::read(new.join(NODE_KEY)).unwrap(), new_key);
    }

    #[test]
    fn a_new_directory_with_other_files_is_refused_and_nothing_moves() {
        let (old, new) = old_node("move-busy-new", 1);
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("notes.txt"), "mine").unwrap();
        let message = move_old_default(&new, false).unwrap_err().to_string();
        assert!(message.contains("not empty"), "{message}");
        assert!(old.join(NODE_KEY).exists());
        assert!(new.join("notes.txt").exists());
    }

    #[test]
    fn a_directory_the_operator_chose_is_never_moved() {
        let (old, new) = old_node("move-chosen", 1);
        assert!(move_old_default(&new, true).unwrap().is_none());
        assert!(old.join(NODE_KEY).exists());
        assert!(!new.exists());
    }

    #[test]
    fn a_broken_chain_is_refused_before_anything_moves() {
        let (old, new) = old_node("move-broken", 2);
        let ledger = old.join(LEDGER);
        let text = std::fs::read_to_string(&ledger).unwrap();
        let first_line_end = text.find('\n').unwrap() + 1;
        std::fs::write(&ledger, &text[first_line_end..]).unwrap();
        assert!(move_old_default(&new, false).is_err());
        assert!(old.join(NODE_KEY).exists(), "the old directory stays where it was");
        assert!(!new.exists());
    }

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
        assert_eq!(d, PathBuf::from("/home/op/.local/share/capsules"));
        assert!(!chosen);
        let (d, _) = resolve(None, Some("/var/xdg"), Some("/home/op"), root).unwrap();
        assert_eq!(d, PathBuf::from("/var/xdg/capsules"));
        let (d, chosen) =
            resolve(Some("state"), None, Some("/home/op"), Path::new("/srv")).unwrap();
        assert_eq!(d, PathBuf::from("/srv/state"));
        assert!(chosen);
        let (d, _) = resolve(Some("/data/cem"), None, None, root).unwrap();
        assert_eq!(d, PathBuf::from("/data/cem"));
        // A relative XDG_DATA_HOME is ignored, as the spec says.
        let (d, _) = resolve(None, Some("rel"), Some("/home/op"), root).unwrap();
        assert_eq!(d, PathBuf::from("/home/op/.local/share/capsules"));
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
