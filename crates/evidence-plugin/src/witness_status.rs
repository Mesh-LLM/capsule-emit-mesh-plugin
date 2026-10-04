//! Where each witness stands with this node's checkpoints, and whether its
//! receipts check.
//!
//! The operator names witnesses in one setting, a list (the console's
//! `witness`, or a comma list in `CAPSULES_CHECKPOINT_WITNESS_URLS`). There
//! is no default witness, and the plugin contacts none until one is named.
//!
//! A witness that accepts a checkpoint returns a receipt, which the
//! checkpoint cadence stores on the checkpoint's line in `checkpoints.jsonl`.
//! A receipt counts only once it is **checked** here, offline:
//!
//! 1. the checkpoint's digest is recomputed from the line's own signed fields
//!    (`sha256` of the sorted, compact JSON of `v, kind, log_id, mmr_size,
//!    root, prev_size, prev_root, key_id, timestamp`), and the entry the
//!    witness logged is `sha256` of that digest;
//! 2. the receipt's `entry_hash` must equal that entry;
//! 3. the COSE receipt must verify for that entry under the witness's key
//!    (`scitt_cose_receipt::verify_receipt`: the inclusion proof and the
//!    witness's signature over the tree head).
//!
//! The witness's key: the operator gives it with the URL (the console's
//! `public_key` on the witness's row, or `CAPSULES_CHECKPOINT_WITNESS_KEYS`),
//! the same endpoint-plus-key shape capsule-cli takes. Only when none is given
//! is the key fetched from that witness, the first time it is needed, and
//! pinned in `<ledger>/witness-keys.json`; the page then says the key was
//! pinned on first contact, not configured. A later different key from the
//! same URL is not taken, and its receipts stay unchecked. A witness is never
//! trusted for more than this: it holds a checkpoint, so a later rewrite of
//! the log is detectable by someone else.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The pinned witness keys, beside `checkpoints.jsonl`.
const KEYS_FILE: &str = "witness-keys.json";
/// The fields a checkpoint's signature covers, and the witness's digest.
const SIGNED_FIELDS: [&str; 9] = [
    "v",
    "kind",
    "log_id",
    "mmr_size",
    "root",
    "prev_size",
    "prev_root",
    "key_id",
    "timestamp",
];

/// One witness's key, as first fetched from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedKey {
    /// The raw 32-byte Ed25519 public key, hex.
    pub pubkey_hex: String,
    pub key_id: String,
    pub pinned_at: String,
}

fn load_keys(ledger_dir: &Path) -> BTreeMap<String, PinnedKey> {
    std::fs::read_to_string(ledger_dir.join(KEYS_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_keys(ledger_dir: &Path, keys: &BTreeMap<String, PinnedKey>) -> anyhow::Result<()> {
    let path = ledger_dir.join(KEYS_FILE);
    let tmp = ledger_dir.join(format!("{KEYS_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(keys)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// A PEM SubjectPublicKeyInfo for a raw Ed25519 key.
fn ed25519_pem(pubkey_hex: &str) -> Option<String> {
    let raw = hex::decode(pubkey_hex).ok().filter(|k| k.len() == 32)?;
    let mut der = hex::decode("302a300506032b6570032100").ok()?;
    der.extend_from_slice(&raw);
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    Some(format!(
        "-----BEGIN PUBLIC KEY-----\n{b64}\n-----END PUBLIC KEY-----\n"
    ))
}

/// The last contact with each witness, beside the pinned keys.
const REACH_FILE: &str = "witness-reach.json";
/// How often a lagging witness is contacted again.
const CONTACT_EVERY: Duration = Duration::from_secs(60);

/// What the last contact with a witness found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reach {
    pub at: String,
    /// `ok`, `timeout`, `refused`, `dns`, `http`, `not_a_witness`,
    /// `key_mismatch` or `unreachable`.
    pub outcome: String,
    /// The reason in words, for the page.
    pub reason: String,
}

fn load_reach(ledger_dir: &Path) -> BTreeMap<String, Reach> {
    std::fs::read_to_string(ledger_dir.join(REACH_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Why a witness could not be reached, from the client's error.
fn classify(err: &crate::producer::anchor::AnchorError) -> (&'static str, String) {
    use crate::producer::anchor::AnchorError;
    match err {
        AnchorError::Status { status, .. } => ("http", format!("answered HTTP {status}")),
        AnchorError::Decode(_) => ("not_a_witness", "answered, but not as a witness".into()),
        AnchorError::Transport(msg) => {
            let m = msg.to_ascii_lowercase();
            if m.contains("timed out") || m.contains("timeout") {
                ("timeout", "did not answer in time".into())
            } else if m.contains("refused") {
                ("refused", "refused the connection".into())
            } else if m.contains("dns") || m.contains("lookup") || m.contains("resolve") {
                ("dns", "its host name does not resolve".into())
            } else {
                ("unreachable", format!("unreachable: {msg}"))
            }
        }
    }
}

/// The URLs the latest checkpoint line carries a receipt from.
fn latest_holders(ledger_dir: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(ledger_dir.join("checkpoints.jsonl"))
        .ok()
        .and_then(|text| {
            text.lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .and_then(|line| serde_json::from_str::<Value>(line).ok())
        })
        .and_then(|cp| cp.get("witnesses").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|w| w.get("ts_url").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// Contact each named witness that needs it, to that witness only: one with
/// no key (configured or pinned) has its key fetched and pinned; one that
/// does not hold the latest checkpoint is asked for its key, which tells
/// whether it answers and whether it still presents the key its receipts are
/// checked under. The outcome is kept in `<ledger>/witness-reach.json` for
/// its row on the page. A witness holding the latest checkpoint is not
/// contacted, and none is when none is named. Blocking; call it off the
/// async runtime. Each witness is contacted at most every [`CONTACT_EVERY`].
pub fn refresh_witnesses(
    ledger_dir: &Path,
    witness_urls: &[String],
    configured_keys: &BTreeMap<String, String>,
) {
    static LAST_TRY: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
    if witness_urls.is_empty() {
        return;
    }
    let mut keys = load_keys(ledger_dir);
    let mut reach = load_reach(ledger_dir);
    let holders = latest_holders(ledger_dir);
    let (mut keys_changed, mut reach_changed) = (false, false);
    for url in witness_urls {
        let expected = configured_keys
            .get(url)
            .map(|k| (k.clone(), "configured"))
            .or_else(|| keys.get(url).map(|k| (k.pubkey_hex.clone(), "pinned on first contact")));
        if expected.is_some() && holders.contains(url) {
            continue;
        }
        {
            let mut tries = LAST_TRY.lock().unwrap_or_else(|e| e.into_inner());
            let tries = tries.get_or_insert_with(HashMap::new);
            if tries.get(url).is_some_and(|at| at.elapsed() < CONTACT_EVERY) {
                continue;
            }
            tries.insert(url.clone(), Instant::now());
        }
        let base = crate::producer::anchor::dispatch_base_for(url).to_string();
        let (outcome, reason) = match crate::producer::anchor::AnchorClient::new(base).authority_pubkey() {
            Ok(key) if ed25519_pem(&key.pubkey_hex).is_none() => (
                "not_a_witness",
                "answered with a key that is not a 32-byte Ed25519 key".to_string(),
            ),
            Ok(key) => {
                let presented = key.pubkey_hex.to_ascii_lowercase();
                match &expected {
                    Some((want, source)) if *want != presented => (
                        "key_mismatch",
                        format!("presents a different key than the one {source} (key id {})", key.key_id),
                    ),
                    Some(_) => ("ok", "answers".to_string()),
                    None => {
                        tracing::info!(witness = %display_url(url), key_id = %key.key_id, "pinned the witness's key");
                        keys.insert(
                            url.clone(),
                            PinnedKey {
                                pubkey_hex: presented,
                                key_id: key.key_id,
                                pinned_at: chrono::Utc::now().to_rfc3339(),
                            },
                        );
                        keys_changed = true;
                        ("ok", "answers".to_string())
                    }
                }
            }
            Err(err) => {
                let (outcome, reason) = classify(&err);
                tracing::warn!(witness = %display_url(url), %err, "{reason}");
                (outcome, reason)
            }
        };
        reach.insert(
            url.clone(),
            Reach {
                at: chrono::Utc::now().to_rfc3339(),
                outcome: outcome.to_string(),
                reason,
            },
        );
        reach_changed = true;
    }
    if keys_changed {
        if let Err(err) = save_keys(ledger_dir, &keys) {
            tracing::warn!(%err, "could not save the pinned witness keys");
        }
    }
    if reach_changed {
        let path = ledger_dir.join(REACH_FILE);
        let tmp = ledger_dir.join(format!("{REACH_FILE}.tmp"));
        let written = serde_json::to_vec_pretty(&reach)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| std::fs::write(&tmp, bytes).map_err(Into::into))
            .and_then(|()| std::fs::rename(&tmp, &path).map_err(Into::into));
        if let Err(err) = written {
            tracing::warn!(%err, "could not save the witness contact record");
        }
    }
}

/// The entry a witness logs for this checkpoint line: `sha256(digest)`, hex.
fn expected_entry_hash(cp: &Value) -> Option<String> {
    let mut body = BTreeMap::new();
    for field in SIGNED_FIELDS {
        body.insert(field, cp.get(field)?.clone());
    }
    let digest = Sha256::digest(serde_json::to_vec(&body).ok()?);
    Some(hex::encode(Sha256::digest(digest)))
}

/// Whether one receipt on one checkpoint line checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    Checked,
    /// Not checked yet: no key pinned for this witness.
    NoKey,
    /// Checked and refused, with the reason.
    Failed(String),
}

fn check_receipt(cp: &Value, receipt: &Value, key_hex: Option<&str>) -> Check {
    if receipt.get("is_stub").and_then(Value::as_bool) == Some(true) {
        return Check::Failed("a placeholder, not a receipt".into());
    }
    let Some(expected) = expected_entry_hash(cp) else {
        return Check::Failed("the checkpoint line lacks a signed field".into());
    };
    if receipt.get("entry_hash").and_then(Value::as_str) != Some(expected.as_str()) {
        return Check::Failed("the receipt is for a different entry than this checkpoint".into());
    }
    let Some(pem) = key_hex.and_then(ed25519_pem) else {
        return Check::NoKey;
    };
    let Some(bytes) = receipt
        .get("receipt_b64")
        .and_then(Value::as_str)
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
    else {
        return Check::Failed("the receipt is not base64".into());
    };
    let Ok(leaf) = hex::decode(&expected) else {
        return Check::Failed("the entry is not hex".into());
    };
    let result = scitt_cose_receipt::verify_receipt(&bytes, &leaf, &pem);
    if result.ok {
        Check::Checked
    } else {
        Check::Failed(format!(
            "the receipt does not verify under the witness's key: {}",
            result.errors.join("; ")
        ))
    }
}

/// The key a witness's receipts are checked under, and where it came from:
/// the operator's, else one pinned on first contact.
fn key_for<'a>(
    url: &str,
    configured: &'a BTreeMap<String, String>,
    pinned: &'a BTreeMap<String, PinnedKey>,
) -> Option<(&'a str, &'static str)> {
    configured
        .get(url)
        .map(|k| (k.as_str(), "configured"))
        .or_else(|| {
            pinned
                .get(url)
                .map(|k| (k.pubkey_hex.as_str(), "pinned_on_first_contact"))
        })
}

/// A witness URL as shown: no userinfo, query or fragment.
pub fn display_url(url: &str) -> String {
    crate::owner_maintenance::without_credentials(url)
}

/// A short name for a witness: its host.
fn witness_name(url: &str) -> String {
    let shown = display_url(url);
    let rest = shown.split_once("://").map_or(shown.as_str(), |(_, r)| r);
    rest.split('/').next().unwrap_or(rest).to_string()
}

/// The checkpoint card's witness fields, from `checkpoints.jsonl`'s lines,
/// the configured witness URLs and the pinned keys.
///
/// - Each receipt on a checkpoint gains `checked` (true only when it
///   verified) and, when not, `check` (why).
/// - `witness_status` lists every configured witness, and every witness a
///   receipt names, with what it holds.
pub fn annotate(
    checkpoints: &mut [Value],
    configured: &[String],
    configured_keys: &BTreeMap<String, String>,
    ledger_dir: &Path,
) -> Value {
    let pinned = load_keys(ledger_dir);
    let reach = load_reach(ledger_dir);
    let mut urls: Vec<String> = configured.to_vec();
    let mut seen: BTreeSet<String> = configured.iter().cloned().collect();
    #[derive(Default)]
    struct Tally {
        held: usize,
        checked: usize,
        latest_checked: Option<(Value, Value)>,
        holds_latest: bool,
        problem: Option<String>,
    }
    let mut tally: HashMap<String, Tally> = HashMap::new();
    let last = checkpoints.len().saturating_sub(1);
    for (index, cp) in checkpoints.iter_mut().enumerate() {
        let snapshot = cp.clone();
        let Some(receipts) = cp.get_mut("witnesses").and_then(Value::as_array_mut) else {
            continue;
        };
        for receipt in receipts.iter_mut() {
            let Some(url) = receipt
                .get("ts_url")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            if seen.insert(url.clone()) {
                urls.push(url.clone());
            }
            let check = check_receipt(
                &snapshot,
                receipt,
                key_for(&url, configured_keys, &pinned).map(|(k, _)| k),
            );
            let t = tally.entry(url).or_default();
            t.held += 1;
            match &check {
                Check::Checked => {
                    t.checked += 1;
                    t.latest_checked = Some((
                        snapshot.get("mmr_size").cloned().unwrap_or(Value::Null),
                        snapshot.get("timestamp").cloned().unwrap_or(Value::Null),
                    ));
                    if index == last {
                        t.holds_latest = true;
                    }
                    receipt["checked"] = json!(true);
                }
                Check::NoKey => {
                    t.problem = Some("its key is not fetched yet".into());
                    receipt["checked"] = json!(false);
                    receipt["check"] = json!("its key is not fetched yet");
                }
                Check::Failed(why) => {
                    t.problem = Some(why.clone());
                    receipt["checked"] = json!(false);
                    receipt["check"] = json!(why);
                }
            }
        }
    }
    let rows: Vec<Value> = urls
        .iter()
        .map(|url| {
            let t = tally.remove(url).unwrap_or_default();
            let is_configured = configured.contains(url);
            // A witness no longer named says so first; its checked
            // receipts still show in the counts.
            let state = if !is_configured {
                "removed"
            } else if t.holds_latest {
                "latest"
            } else if t.checked > 0 {
                "earlier"
            } else if t.held > 0 {
                "unchecked"
            } else {
                "pending"
            };
            json!({
                "name": witness_name(url),
                "url": display_url(url),
                "configured": is_configured,
                "state": state,
                "held_count": t.held,
                "checked_count": t.checked,
                "latest_checked": t.latest_checked.map(|(size, ts)| json!({"mmr_size": size, "timestamp": ts})),
                "key_source": key_for(url, configured_keys, &pinned).map_or(Value::Null, |(_, source)| Value::from(source)),
                // Why it does not hold the latest checkpoint: what the last
                // contact found, when that was not a plain answer; else why
                // its receipts did not check.
                "problem": if t.holds_latest {
                    Value::Null
                } else {
                    reach
                        .get(url)
                        .filter(|r| r.outcome != "ok")
                        .map(|r| r.reason.clone())
                        .or(t.problem)
                        .map_or(Value::Null, Value::from)
                },
                "last_contact": reach.get(url).map_or(Value::Null, |r| json!({"at": r.at, "outcome": r.outcome})),
            })
        })
        .collect();
    Value::Array(rows)
}

/// A real receipt for a test checkpoint line, minted with the scitt-cose
/// Python reference (`build_receipt`, EdDSA, leaf 1 of 3) under a fixed test
/// key, so the check is exercised end to end.
#[cfg(test)]
pub(crate) mod fixture {
    use serde_json::{json, Value};

    pub const WITNESS: &str = "https://witness.example";
    pub const PUBKEY_HEX: &str = "03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8";
    pub const ENTRY_HASH: &str = "7806d5f2218209cdbb7d39731ba7660adeac388a7e81b0b2b4b6816501f0384e";
    pub const RECEIPT_B64: &str = "0oRHohkBiwEBJ6EZAYyhIIFYSIMDAYJYIK62o1zzpFj3BEvbbsO75spXLmTortiof8oJ6bdE+jkFWCCutqNc86RY9wRL227Du+bKVy5k6K7YqH/KCem3RPo5BfZYQERw5RI05xQ/90nrG+KgrgVUbv0wnLkelwjF/Zlc+jYQSA4Fm48N/pb0vS8peh5gwnF8Yi8VbANtuwrUSiDocgc=";

    /// The checkpoint line the receipt was minted for (mmr_size 3).
    pub fn held_line() -> Value {
        json!({"v": 1, "kind": "mmr_checkpoint", "log_id": "capsules/abc", "mmr_size": 3,
               "root": "aa", "prev_size": 0, "prev_root": "", "key_id": "k1",
               "timestamp": "2026-10-04T00:00:00Z",
               "witnesses": [{"ts_url": WITNESS, "entry_hash": ENTRY_HASH,
                              "receipt_b64": RECEIPT_B64, "leaf_index": 1, "tree_size": 3}]})
    }

    /// `witness-keys.json` pinning the test key for [`WITNESS`].
    pub fn keys_json() -> String {
        json!({WITNESS: {"pubkey_hex": PUBKEY_HEX, "key_id": "test", "pinned_at": "t"}}).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_receipt_under_the_pinned_key_checks_and_counts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(KEYS_FILE), fixture::keys_json()).unwrap();
        let mut lines = vec![fixture::held_line()];
        let rows = annotate(
            &mut lines,
            &[fixture::WITNESS.to_string()],
            &BTreeMap::new(),
            dir.path(),
        );
        assert_eq!(rows[0]["state"], json!("latest"));
        assert_eq!(rows[0]["key_source"], json!("pinned_on_first_contact"));
        assert_eq!(rows[0]["checked_count"], json!(1));
        assert_eq!(rows[0]["latest_checked"]["mmr_size"], json!(3));
        assert_eq!(rows[0]["problem"], Value::Null);
        assert_eq!(lines[0]["witnesses"][0]["checked"], json!(true));
    }

    #[test]
    fn the_same_receipt_under_another_key_or_on_another_checkpoint_does_not_check() {
        let cp = fixture::held_line();
        let receipt = cp["witnesses"][0].clone();
        let other_key = "11".repeat(32);
        assert!(matches!(
            check_receipt(&cp, &receipt, Some(&other_key)),
            Check::Failed(_)
        ));
        let mut moved = cp.clone();
        moved["root"] = json!("bb");
        assert_eq!(
            check_receipt(&cp, &receipt, Some(fixture::PUBKEY_HEX)),
            Check::Checked
        );
        assert!(matches!(
            check_receipt(&moved, &receipt, Some(fixture::PUBKEY_HEX)),
            Check::Failed(_)
        ));
    }

    fn line(mmr: u64, witnesses: Value) -> Value {
        json!({"v": 1, "kind": "mmr_checkpoint", "log_id": "capsules/abc", "mmr_size": mmr,
               "root": "aa", "prev_size": 0, "prev_root": "", "key_id": "k1",
               "timestamp": "2026-10-04T00:00:00Z", "witnesses": witnesses})
    }

    #[test]
    fn the_entry_is_sha256_of_the_sorted_compact_signed_body() {
        // The same bytes capsule-anchor's _checkpoint_digest hashes:
        // json.dumps(body, sort_keys=True, separators=(",", ":")).
        let cp = line(3, json!([]));
        let body = r#"{"key_id":"k1","kind":"mmr_checkpoint","log_id":"capsules/abc","mmr_size":3,"prev_root":"","prev_size":0,"root":"aa","timestamp":"2026-10-04T00:00:00Z","v":1}"#;
        let digest = Sha256::digest(body.as_bytes());
        assert_eq!(
            expected_entry_hash(&cp).unwrap(),
            hex::encode(Sha256::digest(digest))
        );
    }

    #[test]
    fn a_pem_is_built_for_a_32_byte_key_only() {
        let pem = ed25519_pem(&"11".repeat(32)).unwrap();
        assert!(pem.starts_with("-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEA"));
        assert!(ed25519_pem("11").is_none());
        assert!(ed25519_pem("zz").is_none());
    }

    #[test]
    fn a_receipt_for_another_entry_or_without_a_key_does_not_count() {
        let dir = tempfile::tempdir().unwrap();
        let cp = line(3, json!([]));
        let wrong = json!({"ts_url": "https://w.example", "entry_hash": "00", "receipt_b64": ""});
        assert!(matches!(check_receipt(&cp, &wrong, None), Check::Failed(_)));
        let right = json!({"ts_url": "https://w.example",
                           "entry_hash": expected_entry_hash(&cp).unwrap(), "receipt_b64": ""});
        assert_eq!(check_receipt(&cp, &right, None), Check::NoKey);
        let stub = json!({"ts_url": "https://w.example", "is_stub": true});
        assert!(matches!(check_receipt(&cp, &stub, None), Check::Failed(_)));
        // A key and garbage receipt bytes: refused, never counted.
        let key = "11".repeat(32);
        let garbage = json!({"ts_url": "https://w.example",
                             "entry_hash": expected_entry_hash(&cp).unwrap(), "receipt_b64": "AAEC"});
        assert!(matches!(
            check_receipt(&cp, &garbage, Some(&key)),
            Check::Failed(_)
        ));
        drop(dir);
    }

    #[test]
    fn every_configured_witness_has_a_row_and_none_counts_without_a_checked_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let mut lines = vec![
            line(
                3,
                json!([{"ts_url": "https://a.example/log", "entry_hash": "00", "receipt_b64": ""}]),
            ),
            line(7, json!([])),
        ];
        let configured = vec![
            "https://a.example/log".to_string(),
            "https://user:pw@b.example".to_string(),
        ];
        let rows = annotate(&mut lines, &configured, &BTreeMap::new(), dir.path());
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], json!("a.example"));
        assert_eq!(rows[0]["state"], json!("unchecked"));
        assert_eq!(rows[0]["held_count"], json!(1));
        assert_eq!(rows[0]["checked_count"], json!(0));
        assert_eq!(rows[1]["name"], json!("b.example"));
        assert_eq!(
            rows[1]["url"],
            json!("https://b.example"),
            "no credentials shown"
        );
        assert_eq!(rows[1]["state"], json!("pending"));
        assert_eq!(lines[0]["witnesses"][0]["checked"], json!(false));
    }

    #[test]
    fn a_witness_no_longer_configured_still_shows_what_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        let mut lines = vec![line(
            3,
            json!([{"ts_url": "https://old.example", "entry_hash": "00", "receipt_b64": ""}]),
        )];
        let rows = annotate(&mut lines, &[], &BTreeMap::new(), dir.path());
        assert_eq!(rows[0]["configured"], json!(false));
        assert_eq!(rows[0]["state"], json!("removed"));
        assert_eq!(rows[0]["held_count"], json!(1));
    }

    #[test]
    fn with_no_witness_configured_nothing_is_contacted() {
        let dir = tempfile::tempdir().unwrap();
        refresh_witnesses(dir.path(), &[], &BTreeMap::new());
        assert!(!dir.path().join(KEYS_FILE).exists());
        assert!(!dir.path().join(REACH_FILE).exists());
    }

    /// A witness that is down: its row says why, in words, and a configured
    /// key is never replaced by a fetch.
    #[test]
    fn a_down_witness_row_says_why() {
        let dir = tempfile::tempdir().unwrap();
        // Nothing listens on port 1: the connection is refused at once.
        let down = "http://127.0.0.1:1".to_string();
        let given = BTreeMap::from([(down.clone(), fixture::PUBKEY_HEX.to_string())]);
        refresh_witnesses(dir.path(), &[down.clone()], &given);
        assert!(!dir.path().join(KEYS_FILE).exists(), "a configured key is never fetched over");
        let reach = load_reach(dir.path());
        assert_eq!(reach[&down].outcome, "refused");
        let mut lines = vec![line(3, json!([]))];
        let rows = annotate(&mut lines, &[down.clone()], &given, dir.path());
        assert_eq!(rows[0]["state"], json!("pending"));
        assert_eq!(rows[0]["problem"], json!("refused the connection"));
        assert_eq!(rows[0]["last_contact"]["outcome"], json!("refused"));
        // Contacted at most once a minute: a second call right away does not
        // contact it again (the record keeps its first time).
        let first = reach[&down].at.clone();
        refresh_witnesses(dir.path(), &[down.clone()], &given);
        assert_eq!(load_reach(dir.path())[&down].at, first);
    }

    /// A one-shot local HTTP server that answers every request with `body`.
    fn serve_once(status: &'static str, body: String) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        url
    }

    #[test]
    fn a_witness_presenting_another_key_or_an_error_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let other = json!({"pubkey_hex": "22".repeat(32), "key_id": "other-id"}).to_string();
        let swapped = serve_once("200 OK", other);
        let failing = serve_once("503 Service Unavailable", "{}".to_string());
        let given = BTreeMap::from([(swapped.clone(), fixture::PUBKEY_HEX.to_string())]);
        refresh_witnesses(dir.path(), &[swapped.clone(), failing.clone()], &given);
        let reach = load_reach(dir.path());
        assert_eq!(reach[&swapped].outcome, "key_mismatch");
        assert!(reach[&swapped].reason.contains("different key than the one configured"), "{}", reach[&swapped].reason);
        assert!(reach[&swapped].reason.contains("other-id"));
        assert_eq!(reach[&failing].outcome, "http");
        assert_eq!(reach[&failing].reason, "answered HTTP 503");
        assert!(!dir.path().join(KEYS_FILE).exists(), "nothing pinned from a failing witness");
    }

    #[test]
    fn client_errors_read_as_reasons() {
        use crate::producer::anchor::AnchorError;
        let words = |e: AnchorError| classify(&e).1;
        assert_eq!(words(AnchorError::Status { status: 503, body: String::new() }), "answered HTTP 503");
        assert_eq!(words(AnchorError::Transport("…: timed out reading response".into())), "did not answer in time");
        assert_eq!(words(AnchorError::Transport("Connection refused (os error 111)".into())), "refused the connection");
        assert_eq!(words(AnchorError::Transport("Dns Failed: failed to lookup address".into())), "its host name does not resolve");
        assert_eq!(words(AnchorError::Decode("x".into())), "answered, but not as a witness");
    }

    #[test]
    fn a_configured_key_is_used_and_wins_over_a_pinned_one() {
        let dir = tempfile::tempdir().unwrap();
        // A wrong key pinned on first contact, the right one configured.
        std::fs::write(
            dir.path().join(KEYS_FILE),
            json!({fixture::WITNESS: {"pubkey_hex": "11".repeat(32), "key_id": "x", "pinned_at": "t"}}).to_string(),
        )
        .unwrap();
        let given = BTreeMap::from([(
            fixture::WITNESS.to_string(),
            fixture::PUBKEY_HEX.to_string(),
        )]);
        let mut lines = vec![fixture::held_line()];
        let rows = annotate(
            &mut lines,
            &[fixture::WITNESS.to_string()],
            &given,
            dir.path(),
        );
        assert_eq!(rows[0]["state"], json!("latest"));
        assert_eq!(rows[0]["key_source"], json!("configured"));
        // Without the configured key, the wrong pinned one leaves it unchecked.
        let mut lines = vec![fixture::held_line()];
        let rows = annotate(
            &mut lines,
            &[fixture::WITNESS.to_string()],
            &BTreeMap::new(),
            dir.path(),
        );
        assert_eq!(rows[0]["state"], json!("unchecked"));
        assert_eq!(rows[0]["key_source"], json!("pinned_on_first_contact"));
    }
}
