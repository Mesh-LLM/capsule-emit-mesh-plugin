//! A referee's signed verdict, on the chains it concerns.
//!
//! This node has no referee yet: it neither issues nor delivers verdicts,
//! and it refuses a verdict delivered to it (`record_push_bridge`), signed,
//! with `adjudication_unavailable`. A twin pair whose answers differ stays
//! recorded and reads "Not adjudicated: this node has no referee yet".
//!
//! What remains here reads what an earlier run of this node holds: the
//! verdict files beside the ledger, and the `adjudication_issued` /
//! `adjudication_received` records on its chain, for the page's verdict view.
use std::sync::Arc;

use mesh_llm_plugin::{PluginContext, PluginError, PluginResult};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::capsule_emit::CapsuleState;

/// The member of a record-push body that marks a delivered verdict: such a
/// body is refused (`record_push_bridge`), never received as a record.
pub const DELIVERY_MARKER: &str = "adjudication_delivery";
pub const DELIVER_OPERATION: &str = "deliver_adjudication";

pub fn is_delivery_body(body: &Value) -> bool {
    body.get(DELIVERY_MARKER).is_some()
}

/// The tool's input, kept so its schema stays published; the call is
/// refused before any field is read.
#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
pub struct DeliverAdjudicationArgs {
    /// The node the verdict would go to. Never read: the call is refused.
    pub peer_id: String,
    /// The referee's signed verdict record, as it issued it.
    pub verdict_capsule: Value,
}

/// The `deliver_adjudication` tool: refused. This node has no referee yet,
/// so it has no verdict of its own to deliver and no way to check anyone
/// else's; a caller gets that answer, never a silent success.
pub async fn deliver(
    _args: DeliverAdjudicationArgs,
    _context: &mut PluginContext<'_>,
    _capsules: Arc<CapsuleState>,
    _self_id: Option<String>,
) -> PluginResult<Value> {
    Err(PluginError::invalid_request(
        "not available: this node has no referee yet, so it neither issues nor delivers verdicts",
    ))
}

/// The held verdict files beside the ledger, as an earlier run may have
/// left them.
pub const ISSUED_ADJUDICATIONS_FILENAME: &str = "issued-adjudications.jsonl";
pub const RECEIVED_ADJUDICATIONS_FILENAME: &str = "received-adjudications.jsonl";

fn held_verdict(ledger_dir: &std::path::Path, verdict_capsule_id: &str) -> Option<Value> {
    for file in [
        ISSUED_ADJUDICATIONS_FILENAME,
        RECEIVED_ADJUDICATIONS_FILENAME,
    ] {
        let Ok(text) = std::fs::read_to_string(ledger_dir.join(file)) else {
            continue;
        };
        for line in text.lines() {
            let Ok(held) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if held.get("verdict_capsule_id").and_then(Value::as_str) == Some(verdict_capsule_id) {
                if let Some(capsule) = held.get("verdict_capsule") {
                    return Some(capsule.clone());
                }
            }
        }
    }
    None
}

/// `issued` / `received` when this node's chain records the verdict.
fn recorded_as(ledger_dir: &std::path::Path, verdict_capsule_id: &str) -> Option<&'static str> {
    let text = std::fs::read_to_string(ledger_dir.join("capsules.jsonl")).ok()?;
    for line in text.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(attestation) = record.pointer("/model_attestation/compute_attestation") else {
            continue;
        };
        for (block, kind) in [
            (
                crate::producer::capsule::ADJUDICATION_ISSUED_BLOCK,
                "issued",
            ),
            (
                crate::producer::capsule::ADJUDICATION_RECEIVED_BLOCK,
                "received",
            ),
        ] {
            if attestation
                .pointer(&format!("/{block}/verdict_capsule_id"))
                .and_then(Value::as_str)
                == Some(verdict_capsule_id)
            {
                return Some(kind);
            }
        }
    }
    None
}

/// `http/ledger/verdict?capsule_id=`: one held verdict, as an earlier run of
/// this node left it. Nothing in this plugin issues or holds verdicts now,
/// and it does not check who signed one against the referee's announced
/// key, so a held verdict is always `"legacy": true` and never `verify_ok`.
/// The signature's own check (`signed_by_key_id` / `signature_error`) and
/// how this node's chain records it (`recorded_as`) are reported as facts,
/// not as a verdict on the verdict. `null` capsule when none is held.
pub fn verdict_json(ledger_dir: &std::path::Path, verdict_capsule_id: &str) -> Value {
    let Some(capsule) = held_verdict(ledger_dir, verdict_capsule_id) else {
        return json!({ "capsule": null, "signed_by_key_id": null, "verify_ok": false });
    };
    let signature = crate::producer::cose::verify_producer_envelope(&capsule);
    let recorded = recorded_as(ledger_dir, verdict_capsule_id);
    let referee = capsule
        .pointer("/model_attestation/compute_attestation/adjudication/referee_node_id")
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "legacy": true,
        "signed_by_key_id": signature.as_ref().ok(),
        "verify_ok": false,
        "signature_error": signature.as_ref().err(),
        "recorded_as": recorded,
        "referee_node_id": referee,
        "capsule": capsule,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/referee-verdict.json")).unwrap()
    }

    #[test]
    fn a_verdict_the_reference_referee_signed_verifies_here() {
        let fx = fixture();
        let key = crate::producer::cose::verify_producer_envelope(&fx["verdict_capsule"])
            .expect("verifies");
        assert_eq!(json!(key), fx["referee_key_id"]);
    }

    fn held_dir(recorded: bool) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let capsule = fixture()["verdict_capsule"].clone();
        let id = capsule["capsule_id"].as_str().unwrap().to_string();
        let held = json!({ "verdict_capsule_id": id, "verdict_capsule": capsule });
        std::fs::write(
            dir.path().join(RECEIVED_ADJUDICATIONS_FILENAME),
            format!("{held}\n"),
        )
        .unwrap();
        if recorded {
            let record = json!({ "model_attestation": { "compute_attestation": {
                "adjudication_received": { "verdict_capsule_id": id } } } });
            std::fs::write(dir.path().join("capsules.jsonl"), format!("{record}\n")).unwrap();
        }
        (dir, id)
    }

    #[test]
    fn a_held_verdict_is_legacy_and_never_verified() {
        let (dir, id) = held_dir(true);
        let out = verdict_json(dir.path(), &id);
        assert_eq!(out["legacy"], json!(true));
        assert_eq!(
            out["verify_ok"],
            json!(false),
            "its signer is not checked against an announced key"
        );
        assert_eq!(out["recorded_as"], json!("received"));
        assert_eq!(out["signed_by_key_id"], fixture()["referee_key_id"]);

        let (unrecorded, id) = held_dir(false);
        assert_eq!(
            verdict_json(unrecorded.path(), &id)["verify_ok"],
            json!(false),
            "held but never recorded"
        );
    }

    #[test]
    fn an_altered_held_verdict_does_not_verify() {
        let (dir, id) = held_dir(true);
        let path = dir.path().join(RECEIVED_ADJUDICATIONS_FILENAME);
        let altered = std::fs::read_to_string(&path)
            .unwrap()
            .replace("contradicted:", "contradicted:x");
        std::fs::write(&path, altered).unwrap();
        let out = verdict_json(dir.path(), &id);
        assert_eq!(out["verify_ok"], json!(false));
        assert!(out["signature_error"].is_string());
    }

    #[test]
    fn an_unknown_verdict_is_null_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            verdict_json(dir.path(), &"0".repeat(64))["capsule"],
            Value::Null
        );
    }
}
