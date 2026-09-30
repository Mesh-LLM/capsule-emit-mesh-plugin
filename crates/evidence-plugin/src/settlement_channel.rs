//! The host's `payment.lifecycle.v1` mesh channel: payer-side payment
//! lifecycle events the host broadcasts for a paid exchange, parsed and
//! checked here before the plugin seals one settlement record per event
//! (`CapsuleState::emit_settlement_record`).
//!
//! A settlement record is the payer node's own sealed observation of what its
//! host broadcast. The event's `source` says who asserted each value: the
//! payer (`payer_asserted`), the provider's invoice (`provider_asserted`), or
//! the payer's wallet (`wallet_reported`). A record is not a claim that money
//! moved beyond what a `wallet_reported` event says, and never a claim about
//! the provider's books. No records for an exchange means "no payment
//! lifecycle observed" (a free exchange, payments off, or a failure before
//! authorization), never "unpaid".
//!
//! [`parse_and_check`] refuses an event whose own `event_ref` does not
//! recompute, and any phase/source/segment/hash combination the host's
//! emitter cannot produce. The combination rules mirror the emitter exactly
//! and are no stricter: in particular a settlement event may carry a `null`
//! `payment_hash`, because the emitter copies the wallet transaction's
//! optional hash as-is.

use crate::producer::capsule::{SettlementObservation, SETTLEMENT_CHANNEL};
use crate::producer::jcs;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The channel name this plugin declares and dispatches on.
pub const PAYMENT_LIFECYCLE_CHANNEL: &str = SETTLEMENT_CHANNEL;

/// Lifecycle phase, with the host's exact wire strings. An unknown phase
/// fails deserialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    TermsAccepted,
    InputInvoiceIssued,
    OutputInvoiceIssued,
    InputSettlementObserved,
    OutputSettlementObserved,
    FinalAccounted,
}

impl Phase {
    pub fn wire(self) -> &'static str {
        match self {
            Phase::TermsAccepted => "terms_accepted",
            Phase::InputInvoiceIssued => "input_invoice_issued",
            Phase::OutputInvoiceIssued => "output_invoice_issued",
            Phase::InputSettlementObserved => "input_settlement_observed",
            Phase::OutputSettlementObserved => "output_settlement_observed",
            Phase::FinalAccounted => "final_accounted",
        }
    }
}

/// Who asserted the event's values, with the host's exact wire strings. An
/// unknown source fails deserialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    PayerAsserted,
    ProviderAsserted,
    WalletReported,
}

impl Source {
    pub fn wire(self) -> &'static str {
        match self {
            Source::PayerAsserted => "payer_asserted",
            Source::ProviderAsserted => "provider_asserted",
            Source::WalletReported => "wallet_reported",
        }
    }
}

/// The only `settlement` value the host emits (for a wallet-reported event).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Settlement {
    Terminal,
}

impl Settlement {
    pub fn wire(self) -> &'static str {
        match self {
            Settlement::Terminal => "terminal",
        }
    }
}

/// One `payment.lifecycle.v1` event. Unknown extra fields are tolerated (a
/// newer host may add some); every listed field is required, including the
/// nullable ones -- `deserialize_with = "Option::deserialize"` turns off
/// serde's missing-`Option`-means-`None` default, so an absent key is an
/// error while an explicit `null` is `None`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PaymentLifecycleEvent {
    pub exchange_id: String,
    pub event_ref: String,
    pub terms_digest: String,
    pub phase: Phase,
    pub source: Source,
    #[serde(deserialize_with = "Option::deserialize")]
    pub settlement: Option<Settlement>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub segment: Option<u32>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub payment_hash: Option<String>,
    pub amount_msat: u64,
}

impl PaymentLifecycleEvent {
    /// Borrow this event as the observation `seal_settlement_record` copies
    /// verbatim.
    pub fn observation(&self) -> SettlementObservation<'_> {
        SettlementObservation {
            exchange_id: &self.exchange_id,
            event_ref: &self.event_ref,
            terms_digest: &self.terms_digest,
            phase: self.phase.wire(),
            source: self.source.wire(),
            settlement: self.settlement.map(Settlement::wire),
            segment: self.segment,
            payment_hash: self.payment_hash.as_deref(),
            amount_msat: self.amount_msat,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettlementEventError {
    #[error("not a JSON object: {0}")]
    NotJson(String),
    #[error("event shape: {0}")]
    Shape(#[from] serde_json::Error),
    #[error("empty exchange_id")]
    EmptyExchangeId,
    #[error("impossible event: {0}")]
    Impossible(&'static str),
    #[error("event_ref cannot be recomputed: {0}")]
    Digest(#[from] jcs::JcsError),
    #[error("event_ref {claimed} does not recompute (got {recomputed})")]
    EventRefMismatch { claimed: String, recomputed: String },
}

/// Parse one channel body and check it before anything is sealed:
/// 1. JSON object, typed into [`PaymentLifecycleEvent`] (unknown phase/source
///    refused, missing field refused, extra fields tolerated);
/// 2. non-empty `exchange_id`;
/// 3. the phase/source/settlement/segment/payment_hash combination is one the
///    host's emitter produces (see [`check_combination`]);
/// 4. `event_ref` recomputes: lowercase-hex SHA-256 of the plugin's own JCS
///    over the received object (extra fields included, as the host digested
///    them) with `event_ref` set to `""`. A float anywhere, or an integer
///    above 2^53-1, makes the JCS refuse and the event is refused.
///
/// Duplicate JSON keys are not detected: `serde_json` keeps the last one.
pub fn parse_and_check(bytes: &[u8]) -> Result<PaymentLifecycleEvent, SettlementEventError> {
    let value: Value = serde_json::from_slice(bytes)?;
    let Value::Object(mut object) = value else {
        return Err(SettlementEventError::NotJson(
            "channel body is not a JSON object".into(),
        ));
    };
    let event = PaymentLifecycleEvent::deserialize(Value::Object(object.clone()))?;
    if event.exchange_id.is_empty() {
        return Err(SettlementEventError::EmptyExchangeId);
    }
    check_combination(&event)?;
    object.insert("event_ref".into(), Value::String(String::new()));
    let recomputed = jcs::json_digest(&Value::Object(object))?;
    if recomputed != event.event_ref {
        return Err(SettlementEventError::EventRefMismatch {
            claimed: event.event_ref,
            recomputed,
        });
    }
    Ok(event)
}

/// The combinations the host's emitter (`paid_events.rs`) produces, and no
/// stricter:
/// - `settlement` is `"terminal"` exactly when `source` is `wallet_reported`;
/// - `terms_accepted` / `final_accounted`: `payer_asserted`, no segment, no
///   payment hash;
/// - `*_invoice_issued`: `provider_asserted` with a segment and a payment hash;
/// - `*_settlement_observed`: `wallet_reported` with a segment; the payment
///   hash may be `null`;
/// - `input_*` phases are segment 0, `output_*` phases a non-zero segment.
fn check_combination(event: &PaymentLifecycleEvent) -> Result<(), SettlementEventError> {
    use Phase::*;
    let impossible = |why| Err(SettlementEventError::Impossible(why));
    if (event.source == Source::WalletReported) != (event.settlement == Some(Settlement::Terminal))
    {
        return impossible("settlement is \"terminal\" exactly when source is wallet_reported");
    }
    let expected_source = match event.phase {
        TermsAccepted | FinalAccounted => Source::PayerAsserted,
        InputInvoiceIssued | OutputInvoiceIssued => Source::ProviderAsserted,
        InputSettlementObserved | OutputSettlementObserved => Source::WalletReported,
    };
    if event.source != expected_source {
        return impossible("source does not match phase");
    }
    match event.phase {
        TermsAccepted | FinalAccounted => {
            if event.segment.is_some() || event.payment_hash.is_some() {
                return impossible("payer-asserted phase carries no segment or payment_hash");
            }
        }
        InputInvoiceIssued
        | OutputInvoiceIssued
        | InputSettlementObserved
        | OutputSettlementObserved => {
            let Some(segment) = event.segment else {
                return impossible("invoice/settlement phase requires a segment");
            };
            let is_input = matches!(event.phase, InputInvoiceIssued | InputSettlementObserved);
            if is_input != (segment == 0) {
                return impossible("input phases are segment 0, output phases non-zero");
            }
            if matches!(event.phase, InputInvoiceIssued | OutputInvoiceIssued)
                && event.payment_hash.is_none()
            {
                return impossible("invoice phase requires a payment_hash");
            }
        }
    }
    Ok(())
}

/// Build a checked event body the way the host does: blank `event_ref`,
/// digest the JCS, fill it in. Shared by this module's tests and the
/// capsule_emit tests/fixture generator.
/// Whether a channel message is the host's own local broadcast, never a frame
/// relayed from a mesh peer.
///
/// The host's local broadcast (`PluginManager::broadcast_channel_message`,
/// `plugin/channel_broadcast.rs`) sends with an empty `source_peer_id` and
/// `target_peer_id` set to the receiving plugin's name. A frame from a peer is
/// delivered to local plugins only when its `target_peer_id` is empty or this
/// node's own peer id, a 64-hex endpoint id (`handle_plugin_channel_stream`,
/// `mesh/plugin_mesh.rs`), and its `source_peer_id` is whatever the sender
/// wrote. So a message with an empty source and a non-empty target that is not
/// a peer id can only be the local broadcast. Payment lifecycle events are
/// this node's own observations; anything else is refused before parsing.
pub(crate) fn is_local_host_broadcast(source_peer_id: &str, target_peer_id: &str) -> bool {
    let target_is_peer_id =
        target_peer_id.len() == 64 && target_peer_id.bytes().all(|b| b.is_ascii_hexdigit());
    source_peer_id.is_empty() && !target_peer_id.is_empty() && !target_is_peer_id
}

#[cfg(test)]
pub(crate) fn host_shaped_event(
    exchange_id: &str,
    phase: Phase,
    segment: Option<u32>,
    payment_hash: Option<&str>,
    amount_msat: u64,
) -> Value {
    let source = match phase {
        Phase::TermsAccepted | Phase::FinalAccounted => Source::PayerAsserted,
        Phase::InputInvoiceIssued | Phase::OutputInvoiceIssued => Source::ProviderAsserted,
        _ => Source::WalletReported,
    };
    let mut event = serde_json::json!({
        "exchange_id": exchange_id,
        "event_ref": "",
        "terms_digest": "7d".repeat(32),
        "phase": phase,
        "source": source,
        "settlement": (source == Source::WalletReported).then_some("terminal"),
        "segment": segment,
        "payment_hash": payment_hash,
        "amount_msat": amount_msat,
    });
    event["event_ref"] = Value::String(jcs::json_digest(&event).unwrap());
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_hosts_local_broadcast_is_accepted() {
        // The host's local broadcast: empty source, target = plugin name.
        assert!(is_local_host_broadcast("", "capsule-emit-mesh"));
        assert!(is_local_host_broadcast("", "capsule-emit-mesh"));
        // A peer's frame delivered here: target empty or our own peer id.
        let our_peer_id = "ab".repeat(32);
        assert!(!is_local_host_broadcast("", ""));
        assert!(!is_local_host_broadcast("", &our_peer_id));
        assert!(!is_local_host_broadcast("", &our_peer_id.to_uppercase()));
        // A named sender is never the local broadcast.
        assert!(!is_local_host_broadcast(
            &"cd".repeat(32),
            "capsule-emit-mesh"
        ));
    }
    use serde_json::json;

    fn bytes(v: &Value) -> Vec<u8> {
        serde_json::to_vec(v).unwrap()
    }

    fn settled_input() -> Value {
        host_shaped_event(
            "ex-1",
            Phase::InputSettlementObserved,
            Some(0),
            Some(&"aa".repeat(32)),
            123457,
        )
    }

    /// Re-digest after a test mutates a field, so only the guard under test
    /// can refuse the body.
    fn redigest(mut v: Value) -> Value {
        v["event_ref"] = json!("");
        v["event_ref"] = Value::String(jcs::json_digest(&v).unwrap());
        v
    }

    #[test]
    fn channel_name_is_the_hosts() {
        assert_eq!(PAYMENT_LIFECYCLE_CHANNEL, "payment.lifecycle.v1");
    }

    /// Every phase, with the segment/hash the emitter gives it, passes. The
    /// upstream `Event` serializes every field with `null`s present, as
    /// `host_shaped_event` does; JCS sorts keys, so field order is irrelevant.
    #[test]
    fn every_phase_the_host_emits_is_accepted() {
        let hash = "aa".repeat(32);
        for (phase, segment, payment_hash) in [
            (Phase::TermsAccepted, None, None),
            (Phase::InputInvoiceIssued, Some(0), Some(hash.as_str())),
            (Phase::InputSettlementObserved, Some(0), Some(hash.as_str())),
            (Phase::OutputInvoiceIssued, Some(1), Some(hash.as_str())),
            (
                Phase::OutputSettlementObserved,
                Some(1),
                Some(hash.as_str()),
            ),
            (Phase::FinalAccounted, None, None),
        ] {
            let body = host_shaped_event("ex-1", phase, segment, payment_hash, 5);
            let event = parse_and_check(&bytes(&body)).unwrap_or_else(|e| panic!("{phase:?}: {e}"));
            assert_eq!(event.phase, phase);
        }
    }

    /// Pinned vector: the upstream event shape, serialized with its `null`s
    /// as the host's `Event` does, with the `event_ref` an independent
    /// computation gives (Python `hashlib.sha256` over `json.dumps(e,
    /// sort_keys=True, separators=(",", ":"))`, which equals JCS for this
    /// ASCII-only, integer-only body).
    #[test]
    fn event_ref_matches_an_independent_digest() {
        let body = br#"{"exchange_id":"exchange-one","event_ref":"f7c6847a6fbfcb6bc6ccc26bdb748b333e22946c70a4aa5684a60ba1c882b634","terms_digest":"7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d7d","phase":"terms_accepted","source":"payer_asserted","settlement":null,"segment":null,"payment_hash":null,"amount_msat":100}"#;
        let event = parse_and_check(body).expect("independently digested event checks");
        assert_eq!(event.amount_msat, 100);
    }

    #[test]
    fn amount_and_fields_parse_verbatim() {
        let event = parse_and_check(&bytes(&settled_input())).unwrap();
        assert_eq!(event.amount_msat, 123457);
        assert_eq!(
            event.payment_hash.as_deref(),
            Some("aa".repeat(32).as_str())
        );
        assert_eq!(event.segment, Some(0));
        assert_eq!(event.settlement, Some(Settlement::Terminal));
        let obs = event.observation();
        assert_eq!(obs.phase, "input_settlement_observed");
        assert_eq!(obs.source, "wallet_reported");
        assert_eq!(obs.settlement, Some("terminal"));
    }

    #[test]
    fn extra_unknown_field_is_tolerated() {
        let mut body = settled_input();
        body["newer_host_field"] = json!("x");
        let body = redigest(body);
        assert!(parse_and_check(&bytes(&body)).is_ok());
    }

    /// Follows upstream: the emitter copies the wallet transaction's optional
    /// payment hash, so a settlement event with a `null` hash is one it can
    /// produce and is accepted.
    #[test]
    fn settlement_observed_with_null_payment_hash_is_accepted() {
        let mut body = settled_input();
        body["payment_hash"] = Value::Null;
        let body = redigest(body);
        assert!(parse_and_check(&bytes(&body)).is_ok());
    }

    #[test]
    fn event_ref_mismatch_is_refused() {
        let mut body = settled_input();
        body["amount_msat"] = json!(999);
        assert!(matches!(
            parse_and_check(&bytes(&body)),
            Err(SettlementEventError::EventRefMismatch { .. })
        ));
    }

    #[test]
    fn unknown_phase_is_refused() {
        let mut body = settled_input();
        body["phase"] = json!("refund_issued");
        let body = redigest(body);
        assert!(matches!(
            parse_and_check(&bytes(&body)),
            Err(SettlementEventError::Shape(_))
        ));
    }

    #[test]
    fn unknown_source_is_refused() {
        let mut body = settled_input();
        body["source"] = json!("oracle_asserted");
        let body = redigest(body);
        assert!(matches!(
            parse_and_check(&bytes(&body)),
            Err(SettlementEventError::Shape(_))
        ));
    }

    #[test]
    fn missing_nullable_field_is_refused() {
        let mut body = settled_input();
        body.as_object_mut().unwrap().remove("payment_hash");
        let body = redigest(body);
        assert!(matches!(
            parse_and_check(&bytes(&body)),
            Err(SettlementEventError::Shape(_))
        ));
    }

    #[test]
    fn empty_exchange_id_is_refused() {
        let mut body = settled_input();
        body["exchange_id"] = json!("");
        let body = redigest(body);
        assert!(matches!(
            parse_and_check(&bytes(&body)),
            Err(SettlementEventError::EmptyExchangeId)
        ));
    }

    #[test]
    fn invoice_without_payment_hash_is_refused() {
        let body = host_shaped_event("ex-1", Phase::InputInvoiceIssued, Some(0), None, 5);
        assert!(matches!(
            parse_and_check(&bytes(&body)),
            Err(SettlementEventError::Impossible(_))
        ));
    }

    #[test]
    fn settlement_observed_without_segment_is_refused() {
        let hash = "aa".repeat(32);
        let body = host_shaped_event("ex-1", Phase::InputSettlementObserved, None, Some(&hash), 5);
        assert!(matches!(
            parse_and_check(&bytes(&body)),
            Err(SettlementEventError::Impossible(_))
        ));
    }

    #[test]
    fn output_phase_on_segment_zero_is_refused() {
        let hash = "bb".repeat(32);
        let body = host_shaped_event(
            "ex-1",
            Phase::OutputSettlementObserved,
            Some(0),
            Some(&hash),
            5,
        );
        assert!(matches!(
            parse_and_check(&bytes(&body)),
            Err(SettlementEventError::Impossible(_))
        ));
    }

    #[test]
    fn source_not_matching_phase_is_refused() {
        let mut body = host_shaped_event("ex-1", Phase::TermsAccepted, None, None, 5);
        body["source"] = json!("provider_asserted");
        let body = redigest(body);
        assert!(matches!(
            parse_and_check(&bytes(&body)),
            Err(SettlementEventError::Impossible(_))
        ));
    }

    #[test]
    fn wallet_reported_without_terminal_settlement_is_refused() {
        let mut body = settled_input();
        body["settlement"] = Value::Null;
        let body = redigest(body);
        assert!(matches!(
            parse_and_check(&bytes(&body)),
            Err(SettlementEventError::Impossible(_))
        ));
    }

    #[test]
    fn payer_phase_with_payment_hash_is_refused() {
        let hash = "aa".repeat(32);
        let body = host_shaped_event("ex-1", Phase::FinalAccounted, None, Some(&hash), 5);
        assert!(matches!(
            parse_and_check(&bytes(&body)),
            Err(SettlementEventError::Impossible(_))
        ));
    }

    /// Above 2^53-1 the host drops the event; if one arrives anyway the JCS
    /// recompute refuses it.
    #[test]
    fn unsafe_amount_is_refused() {
        let mut body = settled_input();
        body["amount_msat"] = json!(1u64 << 53);
        assert!(matches!(
            parse_and_check(&bytes(&body)),
            Err(SettlementEventError::Digest(_))
        ));
    }
}
