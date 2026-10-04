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
//! directory carried that name. A node that has one keeps using it, in place,
//! under its old name (`existing_old_default`): nothing is moved or copied, so
//! an upgrade can never race a writer that is still running. A node with both
//! directories refuses to start and says why, rather than pick one or merge
//! them.
//!
//! Before using a directory whose ledger exists, the plugin refuses if another
//! process has that ledger open (Linux, from `/proc`): a plugin from before
//! 0.1.3 takes no lock, so the lock alone cannot keep it out.
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
/// The lock in the ledger's own directory (see [`lock`]).
const LEDGER_LOCK_FILE: &str = ".capsules-writer.lock";

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

/// The old default directory to keep using, when this node has one: only
/// for the default location (a directory the operator chose is used as is),
/// only when it holds a node, and never when the new one holds one too.
fn existing_old_default(dir: &Path, chosen: bool) -> anyhow::Result<Option<PathBuf>> {
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
    Ok(Some(old))
}

/// `lsof -t path`, from the system's own `/usr/sbin/lsof` (never one found on
/// PATH), given at most five seconds; anything else is "cannot be told".
fn lsof_holders(path: &Path) -> Vec<u32> {
    use std::io::Read;
    let me = std::process::id();
    let Ok(mut child) = std::process::Command::new("/usr/sbin/lsof")
        .arg("-t")
        .arg(path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    out.lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .filter(|pid| *pid != me)
        .collect()
}

/// The other processes that hold `path` open: from `/proc` on Linux, from
/// `lsof -t` elsewhere (macOS). Empty when it cannot be told. A writer that
/// opens the ledger only after this check is not seen; the lock keeps out
/// every capsules process, so that can only be a plugin from before 0.1.3.
fn other_holders(path: &Path) -> Vec<u32> {
    if !cfg!(target_os = "linux") {
        return lsof_holders(path);
    }
    let Ok(target) = std::fs::canonicalize(path) else {
        return Vec::new();
    };
    let me = std::process::id();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut holders = Vec::new();
    for entry in procs.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == me {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        if fds
            .flatten()
            .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|p| p == target))
        {
            holders.push(pid);
        }
    }
    holders
}

/// Refuse a directory whose ledger another process has open: two writers on
/// one hash chain break it, and a plugin from before 0.1.3 takes no lock.
fn check_no_other_writer(dir: &Path) -> anyhow::Result<()> {
    let holders = other_holders(&dir.join(LEDGER));
    if let Some(pid) = holders.first() {
        bail!(
            "{} is open in another process (pid {pid}); refusing to write to a ledger another \
             process is writing. Stop that process, or give this node its own {ENV_DATA_DIR}",
            dir.join(LEDGER).display()
        );
    }
    Ok(())
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
    let dir = match existing_old_default(&dir, chosen)? {
        Some(old) => {
            tracing::info!(
                dir = %old.display(),
                "using this node's existing data directory under the plugin's old name"
            );
            old
        }
        None => dir,
    };
    check_no_second_key(&dir, chosen, &cwd)?;
    Ok(dir)
}

/// This process's exclusive hold on the data directory, for its lifetime.
#[must_use = "the directory is locked only while this is held"]
pub struct DataDirLock {
    _files: Vec<File>,
}

/// One advisory lock on the file at `path` (never through a symlink), or why
/// not: `Ok(None)` when another process holds it, with its pid.
fn take(path: &Path, label: &str) -> anyhow::Result<Result<File, String>> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!(
            "{} is a symbolic link; refusing to lock through it",
            path.display()
        );
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    // Never through a link, even one made between the check above and here.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => {
            file.set_len(0)
                .and_then(|()| writeln!(file, "{}", std::process::id()))
                .with_context(|| format!("write {}", path.display()))?;
            Ok(Ok(file))
        }
        Err(TryLockError::WouldBlock) => {
            let mut holder = String::new();
            let _ = file.read_to_string(&mut holder);
            let holder = match holder.trim() {
                "" => "unknown",
                pid => pid,
            };
            Ok(Err(format!(
                "{label} is in use by another capsules plugin process (pid {holder}); give each node its own {ENV_DATA_DIR}"
            )))
        }
        Err(TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("lock {}", path.display()))
        }
    }
}

/// Take the data directory for this process alone.
///
/// An advisory OS lock (`flock` on Unix, `LockFileEx` on Windows) on
/// `<dir>/capsules.lock`, and a second one in the ledger's own directory as
/// it really is (a `ledger/` that is a link to a directory another data
/// directory also uses is the same ledger). The OS releases a lock when the
/// process exits or crashes, so a lock file a crash left behind never blocks
/// the next start; it is not a pid file. A lock file that is a symbolic link
/// is refused. A second process gets one plain line naming the directory and
/// the holder's pid. A writer from before 0.1.3 takes no lock, so the ledger
/// itself is checked too (`check_no_other_writer`).
pub fn lock(dir: &Path) -> anyhow::Result<DataDirLock> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let dir_lock = take(
        &dir.join(LOCK_FILE),
        &format!("data directory {}", dir.display()),
    )?
    .map_err(anyhow::Error::msg)?;
    let ledger = dir.join("ledger");
    std::fs::create_dir_all(&ledger).with_context(|| format!("create {}", ledger.display()))?;
    let real_ledger =
        std::fs::canonicalize(&ledger).with_context(|| format!("resolve {}", ledger.display()))?;
    let ledger_lock = take(
        &real_ledger.join(LEDGER_LOCK_FILE),
        &format!("ledger {}", real_ledger.display()),
    )?
    .map_err(anyhow::Error::msg)?;
    let mut files = vec![dir_lock, ledger_lock];
    // The log of requests made of this node can live elsewhere
    // (CAPSULES_RECEIVED_LOG_DIR); two nodes must not share it either.
    if let Some(lock_path) =
        received_log_lock_path(crate::settings::var_os(crate::evidence_routes::ENV_RECEIVED_LOG_DIR), &real_ledger)
    {
        match take(
            &lock_path,
            &format!("received-request log {}", lock_path.parent().unwrap_or(&lock_path).display()),
        ) {
            Ok(Ok(file)) => files.push(file),
            // Another process writes it: refused, as for the ledger.
            Ok(Err(held)) => anyhow::bail!(held),
            // The log is best-effort; it never stops the plugin starting.
            Err(e) => tracing::warn!(
                path = %lock_path.display(),
                error = %e,
                "cannot lock the received-request log; starting without that lock"
            ),
        }
    }
    check_no_other_writer(dir)?;
    Ok(DataDirLock { _files: files })
}

/// The lock file for the received-request log, when it needs one of its own:
/// the log is opt-in and written only to a directory that already exists, so
/// none is created here; a directory that cannot be resolved is left alone
/// (warned, not fatal); and a directory that is the ledger itself is already
/// locked by this process.
fn received_log_lock_path(env: Option<std::ffi::OsString>, real_ledger: &Path) -> Option<PathBuf> {
    let raw = env.filter(|v| !v.is_empty())?;
    let given = PathBuf::from(raw.to_string_lossy().trim());
    if !given.is_dir() {
        return None;
    }
    let real = match std::fs::canonicalize(&given) {
        Ok(real) => real,
        Err(e) => {
            tracing::warn!(path = %given.display(), error = %e, "cannot resolve the received-request log directory");
            return None;
        }
    };
    (real != real_ledger).then(|| real.join(LEDGER_LOCK_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_received_log_is_locked_only_when_it_exists_apart_from_the_ledger() {
        let base = tmp("received-lock");
        let ledger = base.join("ledger");
        std::fs::create_dir_all(&ledger).unwrap();
        let ledger = std::fs::canonicalize(&ledger).unwrap();
        let os = |p: &Path| Some(p.as_os_str().to_os_string());
        // Unset or empty: no log, no lock.
        assert_eq!(received_log_lock_path(None, &ledger), None);
        assert_eq!(received_log_lock_path(Some("".into()), &ledger), None);
        // Not there (or impossible to make): no lock, and nothing created,
        // because the log is written only to a directory that exists.
        let missing = base.join("not-yet");
        assert_eq!(received_log_lock_path(os(&missing), &ledger), None);
        assert!(!missing.exists());
        let under_a_file = base.join("a-file");
        std::fs::write(&under_a_file, b"x").unwrap();
        assert_eq!(received_log_lock_path(os(&under_a_file.join("log")), &ledger), None);
        // The ledger itself, directly or through a link: already locked.
        assert_eq!(received_log_lock_path(os(&ledger), &ledger), None);
        #[cfg(unix)]
        {
            let link = base.join("ledger-link");
            std::os::unix::fs::symlink(&ledger, &link).unwrap();
            assert_eq!(received_log_lock_path(os(&link), &ledger), None);
        }
        // A directory of its own: its own lock.
        let own = base.join("received");
        std::fs::create_dir_all(&own).unwrap();
        assert_eq!(
            received_log_lock_path(os(&own), &ledger),
            Some(std::fs::canonicalize(&own).unwrap().join(LEDGER_LOCK_FILE))
        );
    }

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
    fn a_fresh_node_uses_the_new_name() {
        let parent = tmp("old-fresh");
        assert!(existing_old_default(&parent.join(APP_DIR), false)
            .unwrap()
            .is_none());
    }

    /// An upgraded node keeps its directory where it is: nothing moves, so a
    /// writer still running there is never raced.
    #[test]
    fn an_existing_node_keeps_its_old_directory_in_place() {
        let (old, new) = old_node("old-in-place", 3);
        let key = std::fs::read(old.join(NODE_KEY)).unwrap();
        let ledger = std::fs::read(old.join(LEDGER)).unwrap();
        assert_eq!(
            existing_old_default(&new, false).unwrap(),
            Some(old.clone())
        );
        assert!(!new.exists(), "nothing created under the new name");
        assert_eq!(std::fs::read(old.join(NODE_KEY)).unwrap(), key);
        assert_eq!(std::fs::read(old.join(LEDGER)).unwrap(), ledger);
    }

    #[test]
    fn a_node_under_both_names_is_refused() {
        let (old, new) = old_node("old-both", 1);
        crate::capsule_emit::CapsuleState::open(&new, "other").unwrap();
        let message = existing_old_default(&new, false).unwrap_err().to_string();
        assert!(message.contains("both"), "{message}");
        assert!(message.contains(&old.display().to_string()), "{message}");
        assert!(message.contains(&new.display().to_string()), "{message}");
    }

    #[test]
    fn a_directory_the_operator_chose_is_used_as_is() {
        let (_old, new) = old_node("old-chosen", 1);
        assert!(existing_old_default(&new, true).unwrap().is_none());
    }

    /// A writer from before 0.1.3 takes no lock; one that has the ledger open
    /// is found and refused.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_ledger_another_process_has_open_is_refused() {
        let (old, _new) = old_node("old-held", 1);
        let mut holder = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "exec 3>>'{}'; exec sleep 30",
                old.join(LEDGER).display()
            ))
            .spawn()
            .unwrap();
        let mut refused = None;
        for _ in 0..50 {
            if let Err(error) = check_no_other_writer(&old) {
                refused = Some(error.to_string());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = holder.kill();
        let _ = holder.wait();
        let message = refused.expect("refused while another process held the ledger");
        assert!(
            message.contains(&format!("pid {}", holder.id())),
            "{message}"
        );
        assert!(check_no_other_writer(&old).is_ok(), "free once it let go");
    }

    /// Two data directories whose `ledger/` is the same directory (a link)
    /// share one ledger: the second is refused.
    #[cfg(unix)]
    #[test]
    fn a_ledger_shared_through_a_link_is_one_ledger() {
        let shared = tmp("lock-shared-ledger");
        let (a, b) = (tmp("lock-link-a"), tmp("lock-link-b"));
        std::os::unix::fs::symlink(&shared, a.join("ledger")).unwrap();
        std::os::unix::fs::symlink(&shared, b.join("ledger")).unwrap();
        let _held = lock(&a).expect("the first");
        let message = lock(&b).err().expect("the second refused").to_string();
        assert!(message.contains("ledger"), "{message}");
        assert!(
            message.contains(&format!("pid {}", std::process::id())),
            "{message}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_file_that_is_a_link_is_refused() {
        let dir = tmp("lock-link-file");
        let elsewhere = tmp("lock-link-target").join("important");
        std::fs::write(&elsewhere, "keep me").unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.join(LOCK_FILE)).unwrap();
        assert!(lock(&dir).is_err());
        assert_eq!(
            std::fs::read_to_string(&elsewhere).unwrap(),
            "keep me",
            "never truncated through the link"
        );
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
