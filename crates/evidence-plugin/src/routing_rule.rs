//! The opt-in operator rule "stop routing to a peer after N referee-signed
//! contradictions within D days". Off unless the operator sets N.
//!
//! The rule never routes anything itself. When it fires it asks the host's own
//! stop-routing path, the one the console's Stop routing button uses
//! (`POST /api/peer-blocks`, loopback-only), for an until-undone block. The
//! host saves the block, enforces it in its router, and asks this plugin to
//! seal the record (`mesh_local_routing_choice`). The rule leaves a pending
//! citation for that peer first, so the sealed record names the rule and cites
//! the verdicts that met it (by commitment; see
//! `crate::producer::capsule::RoutingRuleCitation`). Undo is the host's
//! unblock, exactly as for a manual block.
//!
//! Inputs: only [`crate::verdict_counts::fold`] over this node's own chain:
//! verdicts whose referee signature was verified before they were recorded, from a referee
//! this node itself asked about that pair of halves, once per (referee, pair).
//! On top of that, one referee alone never fires the rule when N is 2 or more:
//! it contributes at most N - 1 contradictions ([`per_referee_cap`]).
//!
//! A verdict that met the rule is written to [`CITED_FILENAME`] whenever the
//! host holds a block of that peer afterwards (a new block, sealed or not, or
//! one already in place), and never counts again. So undoing a block holds:
//! the rule fires again only on new contradictions.
//!
//! A host without `/api/peer-blocks` (stock mesh-llm today) answers 404: the
//! rule logs that this host has no stop-routing hook and does nothing.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};

use crate::capsule_emit::CapsuleState;
use crate::verdict_counts::{read_requested, PeerVerdict, CONTRADICTED};

pub const RULE_NAME: &str = "stop_routing_after_contradictions";
/// N. Unset, empty, `0` or not a number: the rule is off.
pub const ENV_AFTER: &str = "CAPSULES_STOP_ROUTING_AFTER_CONTRADICTIONS";
/// D, in days. Unset: [`DEFAULT_WINDOW_DAYS`]. `0` or not a number: off.
pub const ENV_WINDOW_DAYS: &str = "CAPSULES_STOP_ROUTING_WINDOW_DAYS";
pub const DEFAULT_WINDOW_DAYS: u32 = 30;
/// The host's local API. Unset: [`DEFAULT_HOST_API`].
pub const ENV_HOST_API: &str = "CAPSULES_HOST_API_URL";
pub const DEFAULT_HOST_API: &str = "http://127.0.0.1:3131";
const PEER_BLOCKS_PATH: &str = "/api/peer-blocks";
/// Beside the ledger: one line per rule block, the verdicts it cited, in
/// clear. Local; the sealed record carries only their commitments.
pub const CITED_FILENAME: &str = "routing-rule-cited.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub after: u32,
    pub window_days: u32,
}

impl Rule {
    pub fn from_env() -> Option<Self> {
        rule_for(
            crate::settings::var(ENV_AFTER).ok().as_deref(),
            crate::settings::var(ENV_WINDOW_DAYS).ok().as_deref(),
        )
    }
}

/// A value that isn't a positive whole number turns the rule off rather than
/// being guessed at.
pub(crate) fn rule_for(after: Option<&str>, window_days: Option<&str>) -> Option<Rule> {
    let positive = |raw: &str| {
        let raw = raw.trim();
        (!raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()))
            .then(|| raw.parse::<u32>().ok())
            .flatten()
            .filter(|n| *n > 0)
    };
    let after = positive(after?)?;
    let window_days = match window_days {
        None => DEFAULT_WINDOW_DAYS,
        Some(raw) => positive(raw)?,
    };
    Some(Rule { after, window_days })
}

/// The rule met for one peer: the verdicts that met it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firing {
    pub peer_id: String,
    pub verdict_capsule_ids: Vec<String>,
}

/// The most contradictions one referee can contribute toward N: N - 1, so
/// that with N of 2 or more no single referee blocks a peer alone. N = 1 is
/// the operator's explicit choice to act on one referee's word.
pub fn per_referee_cap(rule: Rule) -> usize {
    (rule.after as usize).saturating_sub(1).max(1)
}

/// Every peer with at least `rule.after` contradictions recorded within the
/// last `rule.window_days` days, not counting verdicts an earlier rule block
/// already cited, and at most [`per_referee_cap`] from any one referee. A
/// verdict with no readable time counts nowhere.
pub fn due(
    rule: Rule,
    by_peer: &BTreeMap<String, Vec<PeerVerdict>>,
    cited: &HashSet<String>,
    now: DateTime<Utc>,
) -> Vec<Firing> {
    let since = now - Duration::days(i64::from(rule.window_days));
    by_peer
        .iter()
        .filter_map(|(peer, verdicts)| {
            let mut per_referee: HashMap<&str, usize> = HashMap::new();
            let ids: Vec<String> = verdicts
                .iter()
                .filter(|v| v.bucket == CONTRADICTED && !cited.contains(&v.verdict_capsule_id))
                .filter(|v| {
                    v.recorded_at
                        .as_deref()
                        .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
                        .is_some_and(|at| at >= since && at <= now)
                })
                .filter(|v| {
                    let taken = per_referee.entry(v.referee_node_id.as_str()).or_default();
                    *taken += 1;
                    *taken <= per_referee_cap(rule)
                })
                .map(|v| v.verdict_capsule_id.clone())
                .collect();
            (ids.len() >= rule.after as usize).then(|| Firing {
                peer_id: peer.clone(),
                verdict_capsule_ids: ids,
            })
        })
        .collect()
}

/// What the rule asks of the host for the firings of one evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planned {
    /// This host has no stop-routing hook: nothing is asked.
    NoHostHook,
    /// The peer is already blocked: nothing is asked, and its verdicts are
    /// spent.
    AlreadyBlocked(Firing),
    /// Ask the host to block the peer.
    Block(Firing),
}

/// The steps for `firings`, given whether the host already blocks a peer
/// (`None`: the host has no stop-routing hook).
pub fn plan(firings: &[Firing], blocked: Option<&dyn Fn(&str) -> bool>) -> Vec<Planned> {
    if firings.is_empty() {
        return Vec::new();
    }
    let Some(blocked) = blocked else {
        return vec![Planned::NoHostHook];
    };
    firings
        .iter()
        .map(|firing| {
            if blocked(&firing.peer_id) {
                Planned::AlreadyBlocked(firing.clone())
            } else {
                Planned::Block(firing.clone())
            }
        })
        .collect()
}

/// A rule block waiting for the host to ask for its seal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCitation {
    pub rule: Rule,
    pub verdict_capsule_ids: Vec<String>,
}

fn pending() -> &'static Mutex<HashMap<String, PendingCitation>> {
    static PENDING: OnceLock<Mutex<HashMap<String, PendingCitation>>> = OnceLock::new();
    PENDING.get_or_init(Mutex::default)
}

fn set_pending(peer_id: &str, citation: PendingCitation) {
    pending()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(peer_id.to_string(), citation);
}

/// The seal path takes the citation for a block of `peer_id`, if the rule
/// asked for it. A manual block finds none.
pub fn take_pending(peer_id: &str) -> Option<PendingCitation> {
    pending()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(peer_id)
}

/// Every verdict id a rule block has cited.
pub fn read_cited(ledger_dir: &Path) -> HashSet<String> {
    let Ok(text) = std::fs::read_to_string(ledger_dir.join(CITED_FILENAME)) else {
        return HashSet::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|line| {
            line.get("verdict_capsule_ids")
                .and_then(Value::as_array)
                .cloned()
        })
        .flatten()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

/// Note `verdict_capsule_ids` as acted on, with the host's routing-choice
/// record id when it reported one.
pub fn record_cited(
    ledger_dir: &Path,
    routing_choice_capsule_id: Option<&str>,
    verdict_capsule_ids: &[String],
) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ledger_dir.join(CITED_FILENAME))?;
    let line = json!({
        "routing_choice_capsule_id": routing_choice_capsule_id,
        "verdict_capsule_ids": verdict_capsule_ids,
    });
    writeln!(file, "{line}")
}

/// What one evaluation did, per peer, for the log and the tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The host blocked the peer; `sealed` is the host's word on the record.
    Blocked { peer_id: String, sealed: bool },
    /// The peer was already blocked; nothing asked.
    AlreadyBlocked { peer_id: String },
    /// This host has no `/api/peer-blocks` (stock mesh-llm).
    NoHostHook,
    /// The host refused or could not be reached.
    Failed { peer_id: String, error: String },
}

async fn active_blocks(client: &reqwest::Client, host: &str) -> Result<HashSet<String>, Outcome> {
    let response = client
        .get(format!("{host}{PEER_BLOCKS_PATH}"))
        .send()
        .await
        .map_err(|error| Outcome::Failed {
            peer_id: String::new(),
            error: error.to_string(),
        })?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(Outcome::NoHostHook);
    }
    let body: Value = response
        .error_for_status()
        .map_err(|error| Outcome::Failed {
            peer_id: String::new(),
            error: error.to_string(),
        })?
        .json()
        .await
        .map_err(|error| Outcome::Failed {
            peer_id: String::new(),
            error: error.to_string(),
        })?;
    Ok(body
        .get("blocks")
        .and_then(Value::as_object)
        .map(|blocks| blocks.keys().cloned().collect())
        .unwrap_or_default())
}

/// Asks the host to block; on success also returns the record id the host
/// reports, if it sealed one.
async fn block(client: &reqwest::Client, host: &str, firing: &Firing) -> (Outcome, Option<String>) {
    let peer_id = firing.peer_id.clone();
    let sent = client
        .post(format!("{host}{PEER_BLOCKS_PATH}"))
        .json(&json!({ "peer": peer_id, "length": "until_undone" }))
        .send()
        .await;
    let response = match sent.and_then(reqwest::Response::error_for_status) {
        Ok(response) => response,
        Err(error) => {
            return (
                Outcome::Failed {
                    peer_id,
                    error: error.to_string(),
                },
                None,
            )
        }
    };
    match response.json::<Value>().await {
        Ok(body) => {
            let sealed = body.get("sealed").and_then(Value::as_bool) == Some(true);
            let record = body
                .pointer("/choice/capsule_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            (Outcome::Blocked { peer_id, sealed }, record)
        }
        Err(error) => (
            Outcome::Failed {
                peer_id,
                error: error.to_string(),
            },
            None,
        ),
    }
}

/// Run the rule once against this node's chain and the host at `host`.
pub async fn evaluate(
    capsules: &Arc<CapsuleState>,
    rule: Rule,
    host: &str,
    now: DateTime<Utc>,
) -> Vec<Outcome> {
    let ledger_dir = capsules.ledger_dir().to_path_buf();
    let (by_peer, cited) = match tokio::task::spawn_blocking(move || {
        let records = crate::evidence_panes::read_capsule_records(&ledger_dir);
        (
            crate::verdict_counts::fold(&records, &read_requested(&ledger_dir)),
            read_cited(&ledger_dir),
        )
    })
    .await
    {
        Ok(read) => read,
        Err(error) => {
            return vec![Outcome::Failed {
                peer_id: String::new(),
                error: error.to_string(),
            }];
        }
    };
    let firings = due(rule, &by_peer, &cited, now);
    if firings.is_empty() {
        return Vec::new();
    }
    let client = reqwest::Client::new();
    let blocked = match active_blocks(&client, host).await {
        Ok(blocked) => blocked,
        Err(outcome) => return vec![outcome],
    };
    let mut outcomes = Vec::new();
    let holds = |peer: &str| blocked.contains(peer);
    for planned in plan(&firings, Some(&holds)) {
        let (firing, block_it) = match planned {
            Planned::NoHostHook => {
                outcomes.push(Outcome::NoHostHook);
                continue;
            }
            Planned::AlreadyBlocked(firing) => (firing, false),
            Planned::Block(firing) => (firing, true),
        };
        let (outcome, record) = if !block_it {
            (
                Outcome::AlreadyBlocked {
                    peer_id: firing.peer_id.clone(),
                },
                None,
            )
        } else {
            set_pending(
                &firing.peer_id,
                PendingCitation {
                    rule,
                    verdict_capsule_ids: firing.verdict_capsule_ids.clone(),
                },
            );
            let answer = block(&client, host, &firing).await;
            // A citation the seal did not take must not ride a later manual block.
            take_pending(&firing.peer_id);
            answer
        };
        // Whenever the host now holds a block of this peer, these verdicts
        // have been acted on: note them, so an undo is not undone by the
        // same verdicts on the next run. Only a host that did not block
        // (unreachable, refused) leaves them to count again.
        if matches!(
            outcome,
            Outcome::Blocked { .. } | Outcome::AlreadyBlocked { .. }
        ) {
            if let Err(error) = record_cited(
                capsules.ledger_dir(),
                record.as_deref(),
                &firing.verdict_capsule_ids,
            ) {
                tracing::warn!(%error, peer_id = %firing.peer_id, "rule verdicts not noted as cited; an undo could re-fire on them");
            }
        }
        outcomes.push(outcome);
    }
    outcomes
}

/// Run the rule in the background, one evaluation at a time, when it is on.
/// Called when a verdict record lands on the chain, and once at start.
pub fn spawn_evaluate(capsules: Arc<CapsuleState>) {
    let Some(rule) = Rule::from_env() else {
        return;
    };
    let host = crate::settings::var(ENV_HOST_API)
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_HOST_API.to_string());
    tokio::spawn(async move {
        static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _one = ONE_AT_A_TIME.lock().await;
        for outcome in evaluate(&capsules, rule, host.trim_end_matches('/'), Utc::now()).await {
            match outcome {
                Outcome::Blocked { peer_id, sealed } => {
                    tracing::info!(%peer_id, sealed, rule = RULE_NAME, "operator rule stopped routing to a peer")
                }
                Outcome::AlreadyBlocked { peer_id } => {
                    tracing::info!(%peer_id, rule = RULE_NAME, "rule met for a peer already blocked; nothing asked")
                }
                Outcome::NoHostHook => {
                    tracing::warn!(rule = RULE_NAME, "rule met, but this host has no stop-routing hook (/api/peer-blocks); nothing blocked")
                }
                Outcome::Failed { peer_id, error } => {
                    tracing::warn!(%peer_id, %error, rule = RULE_NAME, "rule met, but the host did not block")
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict_counts::{CORROBORATED, INCONCLUSIVE, REQUESTED_FILENAME};

    const NOW: &str = "2026-09-28T16:00:00Z";
    const RULE: Rule = Rule {
        after: 3,
        window_days: 7,
    };

    fn now() -> DateTime<Utc> {
        NOW.parse().unwrap()
    }

    /// A counted verdict about peer `p`, from a referee of its own.
    fn verdict(id: &str, bucket: &'static str, days_ago: i64) -> PeerVerdict {
        by_referee(id, &format!("referee-{id}"), bucket, days_ago)
    }

    fn by_referee(id: &str, referee: &str, bucket: &'static str, days_ago: i64) -> PeerVerdict {
        PeerVerdict {
            verdict_capsule_id: id.to_string(),
            referee_node_id: referee.to_string(),
            bucket,
            recorded_at: Some((now() - Duration::days(days_ago)).to_rfc3339()),
            model_hash: None,
        }
    }

    fn peer(verdicts: Vec<PeerVerdict>) -> BTreeMap<String, Vec<PeerVerdict>> {
        BTreeMap::from([("p".to_string(), verdicts)])
    }

    #[test]
    fn off_unless_the_operator_sets_a_positive_n() {
        assert_eq!(rule_for(None, None), None);
        assert_eq!(rule_for(Some(""), None), None);
        assert_eq!(rule_for(Some("0"), None), None);
        assert_eq!(rule_for(Some("three"), Some("7")), None);
        assert_eq!(
            rule_for(Some("3"), Some("0")),
            None,
            "a zero window is off, not 'forever'"
        );
        assert_eq!(rule_for(Some("3"), Some("x")), None);
        assert_eq!(
            rule_for(Some("3"), None),
            Some(Rule {
                after: 3,
                window_days: DEFAULT_WINDOW_DAYS
            })
        );
        assert_eq!(rule_for(Some(" 3 "), Some("7")), Some(RULE));
    }

    #[test]
    fn fires_at_exactly_n_within_d() {
        let by_peer = peer(vec![
            verdict("a", CONTRADICTED, 0),
            verdict("b", CONTRADICTED, 3),
            verdict("c", CONTRADICTED, 7),
        ]);
        assert_eq!(
            due(RULE, &by_peer, &HashSet::new(), now()),
            vec![Firing {
                peer_id: "p".into(),
                verdict_capsule_ids: vec!["a".into(), "b".into(), "c".into()]
            }]
        );
    }

    #[test]
    fn does_not_fire_at_n_minus_one() {
        let by_peer = peer(vec![
            verdict("a", CONTRADICTED, 0),
            verdict("b", CONTRADICTED, 1),
            // Not contradictions: they never count toward N.
            verdict("c", CORROBORATED, 1),
            verdict("d", INCONCLUSIVE, 1),
        ]);
        assert!(due(RULE, &by_peer, &HashSet::new(), now()).is_empty());
    }

    #[test]
    fn a_contradiction_outside_the_window_does_not_count() {
        let by_peer = peer(vec![
            verdict("a", CONTRADICTED, 0),
            verdict("b", CONTRADICTED, 6),
            verdict("c", CONTRADICTED, 8),
        ]);
        assert!(due(RULE, &by_peer, &HashSet::new(), now()).is_empty());
    }

    #[test]
    fn a_contradiction_without_a_time_does_not_count() {
        let mut undated = verdict("c", CONTRADICTED, 0);
        undated.recorded_at = None;
        let by_peer = peer(vec![
            verdict("a", CONTRADICTED, 0),
            verdict("b", CONTRADICTED, 1),
            undated,
        ]);
        assert!(due(RULE, &by_peer, &HashSet::new(), now()).is_empty());
    }

    #[test]
    fn verdicts_an_earlier_rule_block_cited_never_count_again() {
        let by_peer = peer(vec![
            verdict("a", CONTRADICTED, 0),
            verdict("b", CONTRADICTED, 1),
            verdict("c", CONTRADICTED, 2),
        ]);
        let cited: HashSet<String> = ["a".to_string()].into();
        assert!(due(RULE, &by_peer, &cited, now()).is_empty());
    }

    /// With N of 2 or more, one referee alone never fires the rule: it
    /// contributes at most N - 1.
    #[test]
    fn one_referee_alone_never_reaches_n() {
        assert_eq!(
            per_referee_cap(Rule {
                after: 1,
                window_days: 7
            }),
            1
        );
        assert_eq!(
            per_referee_cap(Rule {
                after: 2,
                window_days: 7
            }),
            1
        );
        assert_eq!(per_referee_cap(RULE), 2);
        let one_referee: Vec<PeerVerdict> = ["a", "b", "c", "d"]
            .iter()
            .map(|id| by_referee(id, "r1", CONTRADICTED, 0))
            .collect();
        assert!(due(RULE, &peer(one_referee.clone()), &HashSet::new(), now()).is_empty());
        let mut two_referees = one_referee;
        two_referees.push(by_referee("e", "r2", CONTRADICTED, 0));
        assert_eq!(
            due(RULE, &peer(two_referees), &HashSet::new(), now()),
            vec![Firing {
                peer_id: "p".into(),
                verdict_capsule_ids: vec!["a".into(), "b".into(), "e".into()]
            }]
        );
        let n_one = Rule {
            after: 1,
            window_days: 7,
        };
        assert_eq!(
            due(
                n_one,
                &peer(vec![by_referee("a", "r1", CONTRADICTED, 0)]),
                &HashSet::new(),
                now()
            )
            .len(),
            1
        );
    }

    /// "Not for unsigned verdicts": the rule reads only the fold, and the fold
    /// reads only the records sealed after a referee's signature verified. A bare
    /// adjudication block claiming three contradictions fires nothing.
    #[test]
    fn unsigned_verdicts_never_fire_the_rule() {
        let bare = |id: &str| {
            json!({ "model_attestation": { "compute_attestation": { "adjudication": {
                "verdict": "contradicted:p", "verdict_capsule_id": id, "referee_node_id": format!("r-{id}"),
                "halves": [format!("{id}-a"), format!("{id}-b")],
                "half_node_ids": ["p", "q"], "received_at": NOW,
            }}}})
        };
        let by_peer =
            crate::verdict_counts::fold(&[bare("a"), bare("b"), bare("c")], &HashSet::new());
        assert!(due(RULE, &by_peer, &HashSet::new(), now()).is_empty());
    }

    /// A stand-in for the fork host's `/api/peer-blocks`: a block is saved,
    /// then the plugin's own seal operation is asked for the record, as
    /// `api/routes/peer_blocks.rs` does. `stock` answers 404 like upstream;
    /// with `seal_fails` set the block holds but the answer is `sealed: false`.
    mod host {
        use std::collections::HashSet;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};

        use axum::extract::State;
        use axum::http::StatusCode;
        use axum::routing::get;
        use axum::{Json, Router};
        use serde_json::{json, Value};

        use crate::capsule_emit::CapsuleState;
        use crate::routing_choice_bridge::{handle_local_routing_choice, LocalRoutingChoiceArgs};

        #[derive(Clone)]
        pub struct Host {
            pub capsules: Arc<CapsuleState>,
            pub blocked: Arc<Mutex<HashSet<String>>>,
            pub sealed: Arc<Mutex<Vec<String>>>,
            pub seal_fails: Arc<AtomicBool>,
        }

        impl Host {
            pub fn new(capsules: &Arc<CapsuleState>) -> Self {
                Self {
                    capsules: capsules.clone(),
                    blocked: Arc::default(),
                    sealed: Arc::default(),
                    seal_fails: Arc::default(),
                }
            }
        }

        fn seal(host: &Host, change: &str, peer: &str) -> Value {
            let args: LocalRoutingChoiceArgs = serde_json::from_value(json!({
                "change": change, "peer_id": peer, "salt": "09".repeat(32),
            }))
            .unwrap();
            let out = handle_local_routing_choice(&host.capsules, args).unwrap();
            host.sealed
                .lock()
                .unwrap()
                .push(out["capsule_id"].as_str().unwrap().to_string());
            out
        }

        pub fn unblock(host: &Host, peer: &str) {
            assert!(
                host.blocked.lock().unwrap().remove(peer),
                "undo only what is blocked"
            );
            seal(host, "unblock", peer);
        }

        /// The operator's own Stop routing, with no rule involved.
        pub fn manual_block(host: &Host, peer: &str) {
            host.blocked.lock().unwrap().insert(peer.to_string());
            seal(host, "block", peer);
        }

        async fn list(State(host): State<Host>) -> Json<Value> {
            let blocks: serde_json::Map<String, Value> = host
                .blocked
                .lock()
                .unwrap()
                .iter()
                .map(|p| (p.clone(), json!({})))
                .collect();
            Json(json!({ "blocks": blocks, "choices": [] }))
        }

        async fn block(State(host): State<Host>, Json(body): Json<Value>) -> Json<Value> {
            assert_eq!(body["length"], json!("until_undone"));
            let peer = body["peer"].as_str().unwrap().to_string();
            host.blocked.lock().unwrap().insert(peer.clone());
            if host.seal_fails.load(Ordering::SeqCst) {
                return Json(
                    json!({ "choice": { "capsule_id": null }, "sealed": false, "seal_error": "down" }),
                );
            }
            let record = seal(&host, "block", &peer);
            Json(json!({ "choice": { "capsule_id": record["capsule_id"] }, "sealed": true }))
        }

        pub async fn serve(host: Host, stock: bool) -> String {
            let app = if stock {
                Router::new().fallback(|| async { StatusCode::NOT_FOUND })
            } else {
                Router::new()
                    .route(super::PEER_BLOCKS_PATH, get(list).post(block))
                    .with_state(host)
            };
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            url
        }
    }

    /// A verdict from `referee` contradicting `peer` (against its twin `q`),
    /// about a pair of halves of its own, recorded on the chain the way the
    /// verified-verdict path records one. With `asked`, this node's own request
    /// to that referee about that pair is noted too. Each test uses its own
    /// `peer`: the pending citations are one map per process.
    fn contradiction(
        capsules: &CapsuleState,
        peer: &str,
        id: &str,
        referee: &str,
        asked: bool,
        at: DateTime<Utc>,
    ) {
        let verdict = format!("contradicted:{peer}");
        let halves = [format!("{id}-a"), format!("{id}-b")];
        let facts = crate::capsule_emit::VerdictFacts {
            verdict: &verdict,
            verdict_capsule_id: id,
            referee_node_id: referee,
            halves: [&halves[0], &halves[1]],
            half_node_ids: [peer, "q"],
            twin_bracket_id: None,
            model_hash: None,
        };
        capsules
            .emit_adjudication_received(&facts, &halves[1], "q", &at.to_rfc3339())
            .unwrap()
            .expect("a new verdict seals a record");
        if asked {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(capsules.ledger_dir().join(REQUESTED_FILENAME))
                .unwrap();
            writeln!(file, "{}", json!({ "referee": referee, "halves": halves })).unwrap();
        }
    }

    fn rule_record(capsules: &CapsuleState, capsule_id: &str) -> Value {
        crate::evidence_panes::read_capsule_records(capsules.ledger_dir())
            .into_iter()
            .find(|r| r["capsule_id"] == json!(capsule_id))
            .expect("the sealed record is on the chain")["model_attestation"]["compute_attestation"]
            ["local_routing_choice"]
            .clone()
    }

    fn open(dir: &tempfile::TempDir) -> Arc<CapsuleState> {
        Arc::new(CapsuleState::open(dir.path(), "node-under-test").unwrap())
    }

    const N2: Rule = Rule {
        after: 2,
        window_days: 7,
    };

    #[tokio::test]
    async fn fires_through_the_hosts_stop_routing_path_and_undo_works() {
        let dir = tempfile::tempdir().unwrap();
        let capsules = open(&dir);
        let host = host::Host::new(&capsules);
        let url = host::serve(host.clone(), false).await;
        let now = Utc::now();
        let (v1, v2, v3, v4) = (
            "1".repeat(64),
            "2".repeat(64),
            "3".repeat(64),
            "4".repeat(64),
        );

        // N-1: nothing asked of the host.
        contradiction(&capsules, "p", &v1, "r1", true, now);
        assert!(evaluate(&capsules, N2, &url, now).await.is_empty());
        assert!(host.sealed.lock().unwrap().is_empty());

        // N within D, from two referees this node asked: the host blocks, and
        // the record names the rule and cites both verdicts by commitment.
        contradiction(&capsules, "p", &v2, "r2", true, now);
        assert_eq!(
            evaluate(&capsules, N2, &url, now).await,
            vec![Outcome::Blocked {
                peer_id: "p".into(),
                sealed: true
            }]
        );
        let block_id = host.sealed.lock().unwrap()[0].clone();
        let fact = rule_record(&capsules, &block_id);
        assert_eq!(fact["change"], json!("block"));
        assert_eq!(fact["rule"]["rule"], json!(RULE_NAME));
        assert_eq!(
            fact["rule"]["verdict_commitments"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            read_cited(capsules.ledger_dir()),
            [v1.clone(), v2.clone()].into()
        );
        assert!(
            take_pending("p").is_none(),
            "no citation is left to ride a later manual block"
        );

        // Undo, the same way as a manual block: a plain unblock record.
        host::unblock(&host, "p");
        let unblock_id = host.sealed.lock().unwrap()[1].clone();
        assert_eq!(rule_record(&capsules, &unblock_id)["rule"], Value::Null);

        // The verdicts that already met the rule don't fire it again...
        assert!(evaluate(&capsules, N2, &url, now).await.is_empty());
        // ...one new contradiction is N-1 again...
        contradiction(&capsules, "p", &v3, "r1", true, now);
        assert!(evaluate(&capsules, N2, &url, now).await.is_empty());
        // ...and N new ones fire it.
        contradiction(&capsules, "p", &v4, "r3", true, now);
        assert_eq!(
            evaluate(&capsules, N2, &url, now).await,
            vec![Outcome::Blocked {
                peer_id: "p".into(),
                sealed: true
            }]
        );
        assert_eq!(host.sealed.lock().unwrap().len(), 3);
    }

    /// The adversarial case end to end: a peer with an announced key signs
    /// many contradictions of `f`, each with its own id and a pair this node
    /// never asked it about. The verified records exist, but nothing is
    /// asked of the host.
    #[tokio::test]
    async fn verdicts_this_node_never_asked_for_block_nobody() {
        let dir = tempfile::tempdir().unwrap();
        let capsules = open(&dir);
        let host = host::Host::new(&capsules);
        let url = host::serve(host.clone(), false).await;
        let now = Utc::now();
        for i in 0..6 {
            contradiction(
                &capsules,
                "f",
                &format!("{i}").repeat(64),
                &format!("x{}", i % 2),
                false,
                now,
            );
        }
        assert!(evaluate(&capsules, N2, &url, now).await.is_empty());
        assert!(host.blocked.lock().unwrap().is_empty());
        assert!(host.sealed.lock().unwrap().is_empty());
    }

    /// Undo holds on every path where the host ends up holding a block:
    /// here the host blocked but could not seal the record.
    #[tokio::test]
    async fn undo_holds_when_the_host_blocked_without_a_sealed_record() {
        let dir = tempfile::tempdir().unwrap();
        let capsules = open(&dir);
        let host = host::Host::new(&capsules);
        host.seal_fails
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let url = host::serve(host.clone(), false).await;
        let now = Utc::now();
        contradiction(&capsules, "u", &"1".repeat(64), "r1", true, now);
        contradiction(&capsules, "u", &"2".repeat(64), "r2", true, now);
        assert_eq!(
            evaluate(&capsules, N2, &url, now).await,
            vec![Outcome::Blocked {
                peer_id: "u".into(),
                sealed: false
            }]
        );
        assert!(take_pending("u").is_none());
        host.seal_fails
            .store(false, std::sync::atomic::Ordering::SeqCst);
        host::unblock(&host, "u");
        assert!(
            evaluate(&capsules, N2, &url, now).await.is_empty(),
            "the undo is not undone"
        );
        assert!(host.blocked.lock().unwrap().is_empty());
    }

    /// ...and here the operator had already blocked the peer by hand when the
    /// rule was met, then undid that block.
    #[tokio::test]
    async fn undo_holds_when_the_peer_was_already_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let capsules = open(&dir);
        let host = host::Host::new(&capsules);
        let url = host::serve(host.clone(), false).await;
        let now = Utc::now();
        host::manual_block(&host, "w");
        contradiction(&capsules, "w", &"1".repeat(64), "r1", true, now);
        contradiction(&capsules, "w", &"2".repeat(64), "r2", true, now);
        assert_eq!(
            evaluate(&capsules, N2, &url, now).await,
            vec![Outcome::AlreadyBlocked {
                peer_id: "w".into()
            }]
        );
        host::unblock(&host, "w");
        assert!(
            evaluate(&capsules, N2, &url, now).await.is_empty(),
            "the undo is not undone"
        );
        assert!(host.blocked.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_manual_block_carries_no_rule() {
        let dir = tempfile::tempdir().unwrap();
        let capsules = open(&dir);
        let host = host::Host::new(&capsules);
        let url = host::serve(host.clone(), false).await;
        reqwest::Client::new()
            .post(format!("{url}{PEER_BLOCKS_PATH}"))
            .json(&json!({ "peer": "m", "length": "until_undone" }))
            .send()
            .await
            .unwrap();
        let id = host.sealed.lock().unwrap()[0].clone();
        assert_eq!(rule_record(&capsules, &id)["rule"], Value::Null);
        assert!(read_cited(capsules.ledger_dir()).is_empty());
    }

    #[tokio::test]
    async fn a_stock_host_without_the_hook_blocks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let capsules = open(&dir);
        let host = host::Host::new(&capsules);
        let url = host::serve(host.clone(), true).await;
        let now = Utc::now();
        contradiction(&capsules, "s", &"1".repeat(64), "r1", true, now);
        assert_eq!(
            evaluate(
                &capsules,
                Rule {
                    after: 1,
                    window_days: 7
                },
                &url,
                now
            )
            .await,
            vec![Outcome::NoHostHook]
        );
        assert!(
            read_cited(capsules.ledger_dir()).is_empty(),
            "nothing blocked, so nothing is used up"
        );
        assert!(take_pending("s").is_none());
        assert!(host.sealed.lock().unwrap().is_empty());
    }

    #[test]
    fn the_cited_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_cited(dir.path()).is_empty());
        record_cited(dir.path(), Some("r1"), &["a".into(), "b".into()]).unwrap();
        record_cited(dir.path(), None, &["c".into()]).unwrap();
        assert_eq!(
            read_cited(dir.path()),
            ["a", "b", "c"].map(String::from).into()
        );
    }
}
