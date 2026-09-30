//! Answering an evidence request in-process
//! (draft-mih-agent-evidence-request-00), for every subject. A referee's
//! `adjudicate` request is not a -00 subject and is refused, signed: this
//! node has no referee yet.
//!
//! The protocol is the `capsule-emit-evidence-request` crate's: this module
//! parses with `request::parse_json`, resolves with `resolve::resolve`,
//! builds with `answer::build` and signs refusals with `refusal::sign`. What
//! stays here is this node's own glue: the limits, the sharing-policy gate,
//! the served summary (a derivation the crate does not build), and the wire.
//!
//! **Order, cheapest first.** Every step that can refuse runs before the
//! work after it, so a request is turned away before it costs anything:
//!
//! 1. the request's size ([`MAX_REQUEST_BYTES`]) → `policy_declined`, before
//!    it is parsed;
//! 2. parsing → `request_malformed` / `coverage_unsatisfiable`;
//! 3. a deadline already past → `deadline_unmet`;
//! 4. the sharing policy switched `off` for any request that carries record
//!    bodies → `not_authorized`, before anything is looked up;
//! 5. resolution against the index → the crate's reason;
//! 6. the number of records (or checkpoints) the answer would carry, counted
//!    from the index without reading one → `policy_declined` over
//!    [`MAX_ANSWER_RECORDS`] / [`MAX_ANSWER_CHECKPOINTS`];
//! 7. the sharing-policy gate on each of those records → `not_authorized`;
//! 8. building and signing; an answer larger than [`MAX_ANSWER_BYTES`] on
//!    the wire → `policy_declined`.
//!
//! Every refusal is signed by this node's key and binds the digest of the
//! request bytes as received (`refusal::sign`). Nothing here unwraps or
//! panics on the request.
//!
//! **The sharing policy** (`share_policy::history_segments`) decides who gets
//! record bodies, by the same rule the plugin's `ledger-fetch/1` channel uses:
//! `off` serves none; a local block or an owner-maintenance record is never
//! served; under `peers` every other record goes to anyone; otherwise a
//! record goes only to the node it names as the other side of its exchange.
//! An answer is served whole or refused whole (`not_authorized`): an
//! artifact is the same for every requester (§5), so it is never filtered
//! per asker. The requester's id is the request's own optional
//! `requester_id` member: its word, which narrows who is answered and is not
//! access control. Checkpoints, the history card and the served summary
//! carry no record bodies and are not gated.
//!
//! **The wire.** An artifact answer is `{"artifact", "material", "envelope"}`:
//! the artifact and the verification material as the exact RFC 8785 text the
//! envelope's digest and signature cover (strings, so no re-serialization
//! can change a byte), and the signed envelope. A refusal is the crate's
//! refusal object. The carrier is plugin-internal, not a standardized wire.

use capsule_emit_evidence_request::answer::{self, BuildError, MAX_RECORDS};
use capsule_emit_evidence_request::digest::request_digest;
use capsule_emit_evidence_request::jcs;
use capsule_emit_evidence_request::refusal;
use capsule_emit_evidence_request::registry::Reason;
use capsule_emit_evidence_request::request::{self, Coverage, Derivation, Request, Subject};
use capsule_emit_evidence_request::resolve::{self, Resolution, ResolvedAnchor};
use capsule_emit_evidence_request::time::parse_utc;
use cll::mmr::leaf_count;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::evidence_log::{self, LedgerIndex, LogView};
use crate::served_summary::{self, SERVED_SUMMARY_DERIVATION_TOKEN};

/// The largest request answered. A -00 request is a few hundred bytes; this
/// turns a large one away before it is parsed.
pub const MAX_REQUEST_BYTES: usize = 16 * 1024;

/// The most records one answer carries. A larger subject is refused
/// `policy_declined`: ask for a range.
pub const MAX_ANSWER_RECORDS: u64 = 256;

/// The most checkpoints one answer carries (`checkpoints`, `history_card/1`).
pub const MAX_ANSWER_CHECKPOINTS: u64 = 1024;

/// The largest answer on the wire. The requester reads at most
/// `mesh_evidence_bridge::MAX_SIDE_STREAM_BYTES`, so a larger answer could
/// never arrive; it is refused here instead.
pub const MAX_ANSWER_BYTES: usize = crate::mesh_evidence_bridge::MAX_SIDE_STREAM_BYTES as usize;

const _: () = assert!(MAX_ANSWER_RECORDS <= MAX_RECORDS && MAX_ANSWER_CHECKPOINTS <= MAX_RECORDS);

/// What one request is answered against.
pub struct Responder<'a> {
    pub ledger_dir: &'a Path,
    pub signing_key: &'a SigningKey,
    /// Now, RFC 3339 UTC (`YYYY-MM-DDTHH:MM:SSZ`): every `issued_at`.
    pub now: &'a str,
    /// `share_policy::history_segments()`.
    pub history_segments: &'a str,
}

/// The one outcome of a request.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// An artifact answer, on the wire.
    Answered {
        wire: Value,
        subject_kind: &'static str,
    },
    /// A signed refusal, on the wire.
    Refused {
        wire: Value,
        reason: Reason,
        subject_kind: Option<&'static str>,
    },
    /// No answer can be given (this node cannot sign one: its clock or its
    /// ledger could not be read). Nothing is sent; the requester records an
    /// absence, never a refusal this node did not sign.
    Unanswerable(String),
}

impl Outcome {
    /// The `received_log` status and reason.
    pub fn log_status(&self) -> (&'static str, Option<&'static str>) {
        match self {
            Outcome::Answered { .. } => ("answered", None),
            Outcome::Refused { reason, .. } => ("refused", Some(reason.token())),
            Outcome::Unanswerable(_) => ("unanswered", None),
        }
    }

    pub fn subject_kind(&self) -> Option<&'static str> {
        match self {
            Outcome::Answered { subject_kind, .. } => Some(subject_kind),
            Outcome::Refused { subject_kind, .. } => *subject_kind,
            Outcome::Unanswerable(_) => None,
        }
    }

    /// The bytes to send, if any.
    pub fn wire_bytes(&self) -> Option<Vec<u8>> {
        match self {
            Outcome::Answered { wire, .. } | Outcome::Refused { wire, .. } => {
                serde_json::to_vec(wire).ok()
            }
            Outcome::Unanswerable(_) => None,
        }
    }
}

/// The requester's self-declared id: the request's optional top-level
/// `requester_id` text member. Read only from a request within
/// [`MAX_REQUEST_BYTES`].
pub fn requester_id(request_bytes: &[u8]) -> Option<String> {
    if request_bytes.len() > MAX_REQUEST_BYTES {
        return None;
    }
    let value: Value = serde_json::from_slice(request_bytes).ok()?;
    value.get("requester_id")?.as_str().map(str::to_string)
}

/// Answer `request_bytes` (exactly as received) from `requester_id`.
pub fn answer(
    request_bytes: &[u8],
    requester_id: Option<&str>,
    responder: &Responder<'_>,
) -> Outcome {
    let digest = request_digest(request_bytes);
    let refuse = |reason: Reason, subject_kind: Option<&'static str>| match refusal::sign(
        &digest,
        reason,
        responder.now,
        responder.signing_key,
    ) {
        Ok(r) => Outcome::Refused {
            wire: r.to_json(),
            reason,
            subject_kind,
        },
        Err(e) => Outcome::Unanswerable(format!("could not sign a {reason} refusal: {e}")),
    };

    // 1. Size, before parsing.
    if request_bytes.len() > MAX_REQUEST_BYTES {
        return refuse(Reason::PolicyDeclined, None);
    }
    // 2. Parse.
    let req = match request::parse_json(request_bytes) {
        Ok(req) => req,
        Err(e) => return refuse(e.reason(), None),
    };
    let kind = Some(req.subject.form().token());
    // 3. Deadline.
    let Some(now) = parse_utc(responder.now) else {
        return Outcome::Unanswerable("this node's clock is not an RFC 3339 UTC time".into());
    };
    if req.deadline.is_some_and(|deadline| deadline < now) {
        return refuse(Reason::DeadlineUnmet, kind);
    }
    // 4. Sharing switched off: no record bodies to anyone.
    if carries_records(&req) && responder.history_segments == "off" && policy_gate_applies() {
        return refuse(Reason::NotAuthorized, kind);
    }

    let key_id = hex::encode(responder.signing_key.verifying_key().to_bytes());
    let built = evidence_log::with_index(responder.ledger_dir, &key_id, |index| {
        let view = index.view()?;
        Ok::<_, std::io::Error>(answer_from(&req, &digest, requester_id, responder, &view))
    });
    match built {
        Ok(Ok(Ok(wire))) => match serde_json::to_vec(&wire) {
            Ok(bytes) if bytes.len() <= MAX_ANSWER_BYTES => Outcome::Answered {
                wire,
                subject_kind: req.subject.form().token(),
            },
            _ => refuse(Reason::PolicyDeclined, kind),
        },
        Ok(Ok(Err(reason))) => refuse(reason, kind),
        Ok(Err(e)) | Err(e) => Outcome::Unanswerable(format!("the ledger could not be read: {e}")),
    }
}

/// Whether the answer to `req` carries record bodies.
fn carries_records(req: &Request) -> bool {
    req.derivation.is_none() && !matches!(req.subject, Subject::Checkpoints)
}

/// The sharing-policy gate, except in the deliberately broken build CI uses
/// to show the parity run catches a responder that drops it.
fn policy_gate_applies() -> bool {
    !cfg!(feature = "mutant-evidence-skips-policy-gate")
}

/// Steps 5 to 8, over the index.
fn answer_from(
    req: &Request,
    digest: &str,
    requester_id: Option<&str>,
    responder: &Responder<'_>,
    view: &LogView<'_>,
) -> Result<Value, Reason> {
    // 5. Resolve.
    let anchor = match resolve::resolve(req, view) {
        Resolution::Artifact(anchor) => anchor,
        Resolution::Refuse(reason) => return Err(reason),
    };
    let index = view.index;
    if matches!(&req.derivation, Some(Derivation::Token(t)) if t == SERVED_SUMMARY_DERIVATION_TOKEN)
    {
        return served_summary_answer(req, digest, &anchor, responder, index);
    }

    // 6. Size, from the index.
    let anchor_position = anchor_position(index, &anchor).ok_or(Reason::CoverageUnsatisfiable)?;
    let covered = leaf_count(index.chain[anchor_position].mmr_size)
        .map_err(|_| Reason::CoverageUnsatisfiable)?;
    let records: Vec<u64> = if carries_records(req) {
        let count = match &req.subject {
            Subject::Range(a, b) => b.saturating_sub(*a).saturating_add(1),
            Subject::FullHistory => covered,
            Subject::Record(_) => 1,
            Subject::Correlation(id) => index.correlated(id).len() as u64,
            Subject::Exchange(half) => index.citing(half).len() as u64,
            Subject::Checkpoints => 0,
        };
        if count > MAX_ANSWER_RECORDS {
            return Err(Reason::PolicyDeclined);
        }
        match &req.subject {
            Subject::Range(a, b) => (*a..=*b).collect(),
            Subject::FullHistory => (0..covered).collect(),
            // Held, but after the anchor: not yet committed under it.
            Subject::Record(d) => match index.leaf_index_of(d) {
                Some(i) if i >= covered => return Err(Reason::CoverageUnsatisfiable),
                found => found.into_iter().collect(),
            },
            Subject::Correlation(id) => index.correlated(id).to_vec(),
            Subject::Exchange(half) => index.citing(half).to_vec(),
            Subject::Checkpoints => Vec::new(),
        }
        .into_iter()
        .filter(|&i| i < covered)
        .collect()
    } else {
        if anchor_position as u64 + 1 > MAX_ANSWER_CHECKPOINTS {
            return Err(Reason::PolicyDeclined);
        }
        Vec::new()
    };

    // 7. Who may have these records.
    if policy_gate_applies()
        && !may_serve_all(index, &records, requester_id, responder.history_segments)
    {
        return Err(Reason::NotAuthorized);
    }

    // 8. Build and sign.
    let limit = if carries_records(req) {
        MAX_ANSWER_RECORDS
    } else {
        MAX_ANSWER_CHECKPOINTS
    };
    let built = answer::build(
        req,
        digest,
        &anchor,
        view,
        limit,
        responder.signing_key,
        responder.now,
    )
    .map_err(|e| {
        if !matches!(
            e,
            BuildError::OverLimit | BuildError::RecordNotFound | BuildError::NoRecords
        ) {
            tracing::warn!(error = %e, "an evidence answer could not be built -- declining");
        }
        build_error_reason(&e)
    })?;
    wire(built.artifact, built.material, built.envelope)
}

/// The refusal for a build that did not complete.
fn build_error_reason(e: &BuildError) -> Reason {
    match e {
        BuildError::RecordNotFound | BuildError::NoRecords => Reason::NoSuchSubject,
        BuildError::AnchorNotFound => Reason::CoverageUnsatisfiable,
        BuildError::DerivationUnsupported => Reason::DerivationUnsupported,
        BuildError::OverLimit | BuildError::Malformed | BuildError::Proof(_) => {
            Reason::PolicyDeclined
        }
    }
}

fn anchor_position(index: &LedgerIndex, anchor: &ResolvedAnchor) -> Option<usize> {
    match anchor {
        ResolvedAnchor::Anchor(a) => index.chain.iter().position(|cp| cp.digest() == a.digest),
        ResolvedAnchor::ExchangeHalf(_) => index.chain.len().checked_sub(1),
    }
}

/// Whether every record in `records` may go to `requester_id` under
/// `history_segments` (see the module doc).
fn may_serve_all(
    index: &LedgerIndex,
    records: &[u64],
    requester_id: Option<&str>,
    history_segments: &str,
) -> bool {
    if records.is_empty() {
        return true;
    }
    if history_segments == "off" {
        return false;
    }
    let asker = requester_id.map(str::trim).filter(|id| !id.is_empty());
    records.iter().all(|&i| {
        let Some(leaf) = index.leaves.get(i as usize) else {
            return false;
        };
        !leaf.never_leaves
            && (history_segments == "peers"
                || (!leaf.padding && asker.is_some() && leaf.counterparty.as_deref() == asker))
    })
}

/// The `served_summary/1` derivation: a static export, answered only under
/// an `expected_pin` naming this node's latest checkpoint (the Python
/// reference's rule). A `min_freshness` request asks for an on-demand export, which this
/// node declines.
fn served_summary_answer(
    req: &Request,
    digest: &str,
    anchor: &ResolvedAnchor,
    responder: &Responder<'_>,
    index: &LedgerIndex,
) -> Result<Value, Reason> {
    if !matches!(req.coverage, Coverage::ExpectedPin(_)) {
        return Err(Reason::PolicyDeclined);
    }
    let latest = index.chain.last().ok_or(Reason::CoverageUnsatisfiable)?;
    if anchor_position(index, anchor) != Some(index.chain.len() - 1) {
        return Err(Reason::CoverageUnsatisfiable);
    }
    let covered = leaf_count(latest.mmr_size).map_err(|_| Reason::CoverageUnsatisfiable)?;
    let facts: Vec<_> = index.leaves.iter().map(|l| &l.facts).collect();
    let summary = served_summary::summary_value(
        &facts,
        &served_summary::Coverage {
            checkpoint: latest,
            covered,
        },
        &|id| index.leaf_index_of(id).is_some(),
    );
    let anchor_digest = latest.digest();
    let artifact = json!({
        "anchor": anchor_digest,
        "evidence_stream": latest.log_id,
        "subject": answer::subject_json(&req.subject),
        "derivation": SERVED_SUMMARY_DERIVATION_TOKEN,
        "served_summary": summary,
    });
    let material = json!({ "anchor_checkpoint": serde_json::to_value(latest).map_err(|_| Reason::PolicyDeclined)? });
    let artifact = jcs::to_string(&artifact).map_err(|_| Reason::PolicyDeclined)?;
    let material = jcs::to_string(&material).map_err(|_| Reason::PolicyDeclined)?;
    let envelope = sign_envelope(digest, &anchor_digest, artifact.as_bytes(), responder)?;
    wire(artifact.into_bytes(), material.into_bytes(), envelope)
}

/// The envelope of `answer::build`, over a derivation the crate does not
/// build: Ed25519 over RFC 8785 of `{request_digest, anchor,
/// artifact_digest, issued_at}`, with `key_id` and `sig` beside them.
fn sign_envelope(
    digest: &str,
    anchor: &str,
    artifact: &[u8],
    responder: &Responder<'_>,
) -> Result<Value, Reason> {
    let mut signed = Map::new();
    signed.insert("request_digest".into(), json!(digest));
    signed.insert("anchor".into(), json!(anchor));
    signed.insert(
        "artifact_digest".into(),
        json!(hex::encode(Sha256::digest(artifact))),
    );
    signed.insert("issued_at".into(), json!(responder.now));
    let body =
        jcs::to_string(&Value::Object(signed.clone())).map_err(|_| Reason::PolicyDeclined)?;
    let sig = responder.signing_key.sign(body.as_bytes());
    signed.insert(
        "key_id".into(),
        json!(hex::encode(
            responder.signing_key.verifying_key().to_bytes()
        )),
    );
    signed.insert("sig".into(), json!(hex::encode(sig.to_bytes())));
    Ok(Value::Object(signed))
}

fn wire(artifact: Vec<u8>, material: Vec<u8>, envelope: Value) -> Result<Value, Reason> {
    let text = |b: Vec<u8>| String::from_utf8(b).map_err(|_| Reason::PolicyDeclined);
    Ok(json!({
        "artifact": text(artifact)?,
        "material": text(material)?,
        "envelope": envelope,
    }))
}

/// Split a received artifact answer into what `answer::verify` takes:
/// `(envelope, artifact bytes, material bytes)`. `None` for anything else.
pub fn split_wire(answer: &Value) -> Option<(&Value, &[u8], &[u8])> {
    Some((
        answer.get("envelope")?,
        answer.get("artifact")?.as_str()?.as_bytes(),
        answer.get("material")?.as_str()?.as_bytes(),
    ))
}

/// Answer `request_bytes` (or, when `busy`, decline it unread), and log the
/// outcome to the "asked of you" log in `log_dir`. The one path the mesh
/// responder takes.
pub fn answer_and_log(
    request_bytes: &[u8],
    busy: bool,
    responder: &Responder<'_>,
    log_dir: Option<&Path>,
) -> Outcome {
    let requester_id = requester_id(request_bytes);
    let outcome = if busy {
        decline_busy(request_bytes, responder)
    } else {
        answer(request_bytes, requester_id.as_deref(), responder)
    };
    let (status, reason) = outcome.log_status();
    crate::received_log::append(
        log_dir,
        &crate::received_log::Entry {
            ts: responder.now,
            path: "evidence-request",
            requester_id: requester_id.as_deref(),
            subject_kind: outcome.subject_kind(),
            status,
            reason,
        },
    );
    outcome
}

/// Now, as every `issued_at` is written.
pub fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// A signed `policy_declined` refusal of `request_bytes`, given without
/// reading the ledger: this node is already answering as many requests as it
/// takes at once.
pub fn decline_busy(request_bytes: &[u8], responder: &Responder<'_>) -> Outcome {
    let digest = request_digest(request_bytes);
    match refusal::sign(
        &digest,
        Reason::PolicyDeclined,
        responder.now,
        responder.signing_key,
    ) {
        Ok(r) => Outcome::Refused {
            wire: r.to_json(),
            reason: Reason::PolicyDeclined,
            subject_kind: None,
        },
        Err(e) => Outcome::Unanswerable(format!("could not sign a policy_declined refusal: {e}")),
    }
}

// ---------------------------------------------------------------------------
// The requester's side: is what came back evidence?
// ---------------------------------------------------------------------------

/// What a requester makes of a response to the request it sent (`sent`,
/// exactly the bytes written) from the responder whose key it expects:
///
/// - `{"state": "refusal", "reason"}`: a refusal that key signed, of this
///   request (`refusal::verify_for`);
/// - `{"state": "artifact", "anchor", "records", "checkpoints"}`: an answer
///   that verifies (`answer::verify`, or for `served_summary/1` its envelope,
///   artifact digest and anchor checkpoint);
/// - `{"state": "not_evidence", "why"}`: anything else. It is not a refusal
///   and not an answer (§4.1).
pub fn verify_response(sent: &[u8], response: &Value, key: &ed25519_dalek::VerifyingKey) -> Value {
    let not_evidence = |why: String| json!({"state": "not_evidence", "why": why});
    let digest = request_digest(sent);
    if response.get("reason").is_some() {
        return match refusal::verify_for(response, key, &digest) {
            Ok(reason) => json!({"state": "refusal", "reason": reason.token()}),
            Err(e) => not_evidence(e.to_string()),
        };
    }
    let Ok(req) = request::parse_json(sent) else {
        return not_evidence("the request sent is not a -00 request".into());
    };
    let Some((envelope, artifact, material)) = split_wire(response) else {
        return not_evidence("neither a refusal nor an artifact answer".into());
    };
    if matches!(&req.derivation, Some(Derivation::Token(t)) if t == SERVED_SUMMARY_DERIVATION_TOKEN)
    {
        return match verify_served_summary(envelope, artifact, material, &req, &digest, key) {
            Ok(anchor) => {
                json!({"state": "artifact", "anchor": anchor, "records": 0, "checkpoints": 0})
            }
            Err(why) => not_evidence(why.into()),
        };
    }
    match answer::verify(
        envelope,
        artifact,
        material,
        &req,
        &digest,
        key,
        &evidence_log::record_digest,
    ) {
        Ok(v) => json!({
            "state": "artifact",
            "anchor": v.anchor.digest(),
            "records": v.records.iter().map(|r| r.leaf_index).collect::<Vec<_>>(),
            "checkpoints": v.checkpoints.len(),
        }),
        Err(e) => not_evidence(e.to_string()),
    }
}

/// The checks `answer::verify` makes of any answer, for the served-summary
/// derivation it does not know: the envelope is `key`'s and names this
/// request, the artifact is the one it signed, and the anchor checkpoint is
/// `key`'s, is the one named, and is the pinned one. Returns the anchor.
fn verify_served_summary(
    envelope: &Value,
    artifact: &[u8],
    material: &[u8],
    req: &Request,
    digest: &str,
    key: &ed25519_dalek::VerifyingKey,
) -> Result<String, &'static str> {
    let text = |k: &str| {
        envelope
            .get(k)
            .and_then(Value::as_str)
            .ok_or("the envelope is malformed")
    };
    let (env_request, env_anchor, env_artifact, issued_at) = (
        text("request_digest")?,
        text("anchor")?,
        text("artifact_digest")?,
        text("issued_at")?,
    );
    if text("key_id")? != hex::encode(key.to_bytes()) {
        return Err("the envelope is not signed by the expected responder key");
    }
    let body = jcs::to_string(&json!({
        "request_digest": env_request, "anchor": env_anchor,
        "artifact_digest": env_artifact, "issued_at": issued_at,
    }))
    .map_err(|_| "the envelope is malformed")?;
    let sig: [u8; 64] = hex::decode(text("sig")?)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or("the envelope is malformed")?;
    key.verify_strict(body.as_bytes(), &ed25519_dalek::Signature::from_bytes(&sig))
        .map_err(|_| "the envelope is not signed by the expected responder key")?;
    if env_request != digest {
        return Err("the envelope names a different request");
    }
    if hex::encode(Sha256::digest(artifact)) != env_artifact {
        return Err("the artifact does not match the envelope's artifact digest");
    }
    let art: Value = serde_json::from_slice(artifact).map_err(|_| "the artifact is malformed")?;
    if art.get("anchor").and_then(Value::as_str) != Some(env_anchor)
        || art.get("subject") != Some(&answer::subject_json(&req.subject))
        || art.get("derivation").and_then(Value::as_str) != Some(SERVED_SUMMARY_DERIVATION_TOKEN)
        || !art.get("served_summary").is_some_and(Value::is_object)
    {
        return Err("the artifact is not for the requested subject or derivation");
    }
    let mat: Value = serde_json::from_slice(material).map_err(|_| "the material is malformed")?;
    let cp: cll::checkpoint::CheckpointRecord = mat
        .get("anchor_checkpoint")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .ok_or("the anchor checkpoint is malformed")?;
    if cp.digest() != env_anchor
        || cp.key_id != hex::encode(key.to_bytes())
        || !cp.verify_signature_offline()
    {
        return Err("the anchor checkpoint is not the responder's, or not the one named");
    }
    match &req.coverage {
        Coverage::ExpectedPin(pin) if pin == env_anchor => Ok(env_anchor.to_string()),
        _ => Err("the anchor does not satisfy the requested coverage"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence_request_parity::{
        corpus_checkpoints, corpus_ledger, sealed, write_jsonl, NODE_KEY_SEED,
    };

    const NOW: &str = "2026-09-29T00:00:00Z";

    fn key() -> SigningKey {
        SigningKey::from_bytes(&NODE_KEY_SEED)
    }

    /// A ledger directory holding `ledger` and checkpoints cut at `cuts`
    /// (the corpus's own cutter, which cuts after 16 and 300 lines).
    fn node(ledger: &[Value]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write_jsonl(&dir.path().join("capsules.jsonl"), ledger);
        let cps: Vec<Value> = corpus_checkpoints(ledger, &key())
            .into_iter()
            .map(|cp| serde_json::to_value(cp).unwrap())
            .collect();
        write_jsonl(&dir.path().join("checkpoints.jsonl"), &cps);
        dir
    }

    fn ask(dir: &Path, bytes: &[u8], segments: &str) -> Outcome {
        let key = key();
        answer(
            bytes,
            requester_id(bytes).as_deref(),
            &Responder {
                ledger_dir: dir,
                signing_key: &key,
                now: NOW,
                history_segments: segments,
            },
        )
    }

    fn refused(outcome: &Outcome) -> Option<Reason> {
        match outcome {
            Outcome::Refused { reason, .. } => Some(*reason),
            _ => None,
        }
    }

    /// No request, however formed, panics the responder or goes unanswered:
    /// each is a signed refusal bound to its own bytes.
    #[test]
    fn hostile_requests_are_signed_refusals_never_panics() {
        let dir = node(&corpus_ledger());
        let deep = format!("{}{}", "[".repeat(5000), "]".repeat(5000));
        let long_id = format!(
            r#"{{"subject":{{"correlation":"{}"}},"coverage":{{"min_freshness":1}}}}"#,
            "x".repeat(15_000)
        );
        let inputs: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"null".to_vec(),
            b"[]".to_vec(),
            deep.into_bytes(),
            vec![0xff, 0xfe, 0x00],
            br#"{"subject":{"range":[0,18446744073709551615]},"coverage":{"min_freshness":1}}"#.to_vec(),
            br#"{"subject":{"range":[5,2]},"coverage":{"min_freshness":1}}"#.to_vec(),
            br#"{"subject":{"range":[-1,2]},"coverage":{"min_freshness":1}}"#.to_vec(),
            br#"{"subject":{"record":"a","range":[0,1]},"coverage":{"min_freshness":1}}"#.to_vec(),
            br#"{"subject":{"checkpoints":null},"coverage":{"min_freshness":18446744073709551615}}"#.to_vec(),
            br#"{"subject":{"checkpoints":null},"coverage":{"min_freshness":"not a time"}}"#.to_vec(),
            long_id.into_bytes(),
        ];
        for bytes in inputs {
            let outcome = ask(dir.path(), &bytes, "peers");
            let Outcome::Refused { wire, reason, .. } = &outcome else {
                panic!(
                    "{:?} was not refused: {outcome:?}",
                    String::from_utf8_lossy(&bytes)
                );
            };
            assert_eq!(
                refusal::verify_for(wire, &key().verifying_key(), &request_digest(&bytes)),
                Ok(*reason),
                "signed and bound to the request"
            );
        }
    }

    /// A `requester_id` that is not text is no identity at all: ignored, not
    /// a reason to refuse what needs none.
    #[test]
    fn a_requester_id_that_is_not_text_is_ignored() {
        let dir = node(&corpus_ledger());
        let bytes =
            br#"{"subject":{"checkpoints":null},"coverage":{"min_freshness":1},"requester_id":5}"#;
        assert_eq!(requester_id(bytes), None);
        assert!(matches!(
            ask(dir.path(), bytes, "peers"),
            Outcome::Answered { .. }
        ));
    }

    /// A refusal names the request it refuses: it does not verify as the
    /// answer to any other request.
    #[test]
    fn a_refusal_is_bound_to_its_request_digest() {
        let dir = node(&corpus_ledger());
        let bytes = br#"{"subject":{"record":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"coverage":{"min_freshness":1}}"#;
        let Outcome::Refused { wire, .. } = ask(dir.path(), bytes, "peers") else {
            panic!("refused");
        };
        let other = request_digest(b"another request");
        assert_eq!(
            refusal::verify_for(&wire, &key().verifying_key(), &other),
            Err(refusal::VerifyError::WrongRequest)
        );
    }

    /// Size and the `off` switch are decided before the ledger is read: with
    /// no ledger at all, the answer is still the policy refusal, not a
    /// failure to read one.
    #[test]
    fn cheap_refusals_come_before_the_ledger_is_read() {
        let missing = tempfile::tempdir().unwrap();
        let nowhere = missing.path().join("no-such-ledger");
        let big = vec![b' '; MAX_REQUEST_BYTES + 1];
        assert_eq!(
            refused(&ask(&nowhere, &big, "peers")),
            Some(Reason::PolicyDeclined)
        );
        let record = format!(
            r#"{{"subject":{{"record":"{}"}},"coverage":{{"min_freshness":1}}}}"#,
            "a".repeat(64)
        );
        assert_eq!(
            refused(&ask(&nowhere, record.as_bytes(), "off")),
            Some(Reason::NotAuthorized)
        );
    }

    /// An answer that would not fit the stream the requester reads is
    /// declined, not sent to be cut off.
    #[test]
    fn an_answer_over_the_stream_cap_is_declined() {
        let mut ledger = vec![sealed(
            json!({"action_type": "fyi", "blob": "x".repeat(MAX_ANSWER_BYTES)}),
        )];
        let rest = corpus_ledger();
        ledger.extend(rest.into_iter().skip(1).take(15));
        let dir = node(&ledger);
        let record = format!(
            r#"{{"subject":{{"record":"{}"}},"coverage":{{"min_freshness":1}}}}"#,
            ledger[0]["capsule_id"].as_str().unwrap()
        );
        assert_eq!(
            refused(&ask(dir.path(), record.as_bytes(), "peers")),
            Some(Reason::PolicyDeclined)
        );
    }

    /// A requester that is too busy gets a signed refusal, never a wait.
    #[test]
    fn a_busy_node_declines_with_a_signed_refusal() {
        let key = key();
        let responder = Responder {
            ledger_dir: Path::new("/nonexistent"),
            signing_key: &key,
            now: NOW,
            history_segments: "peers",
        };
        let bytes = br#"{"subject":{"checkpoints":null},"coverage":{"min_freshness":1}}"#;
        let Outcome::Refused { wire, reason, .. } = decline_busy(bytes, &responder) else {
            panic!("declined");
        };
        assert_eq!(reason, Reason::PolicyDeclined);
        assert!(refusal::verify_for(&wire, &key.verifying_key(), &request_digest(bytes)).is_ok());
    }

    /// A node whose clock cannot be written as an `issued_at` signs nothing.
    #[test]
    fn a_bad_clock_answers_nothing() {
        let key = key();
        let responder = Responder {
            ledger_dir: Path::new("/nonexistent"),
            signing_key: &key,
            now: "yesterday",
            history_segments: "peers",
        };
        assert!(matches!(
            answer(b"{}", None, &responder),
            Outcome::Unanswerable(_)
        ));
        assert_eq!(answer(b"{}", None, &responder).wire_bytes(), None);
    }

    /// This node has no referee: a referee's `adjudicate` request is answered
    /// in-process with a signed refusal, never a verdict, never silence.
    #[test]
    fn an_adjudicate_request_is_refused_signed_never_a_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let request = json!({
            "v": 1,
            "subject": {"kind": "adjudicate", "halves": ["a".repeat(64), "b".repeat(64)]},
            "requester_id": "peer-x",
        })
        .to_string();
        let outcome = ask(dir.path(), request.as_bytes(), "peers");
        assert!(refused(&outcome).is_some(), "refused");
        let wire = outcome.wire_bytes().expect("something signed is sent");
        let reply: Value = serde_json::from_slice(&wire).unwrap();
        assert!(reply.get("adjudication_verdict").is_none());
        let key_id = hex::encode(key().verifying_key().to_bytes());
        assert_eq!(reply["key_id"], json!(key_id));
        let check = refusal::verify_for(
            &reply,
            &key().verifying_key(),
            &request_digest(request.as_bytes()),
        );
        assert!(
            check.is_ok(),
            "the refusal verifies under this node's key: {check:?}"
        );
    }
}
