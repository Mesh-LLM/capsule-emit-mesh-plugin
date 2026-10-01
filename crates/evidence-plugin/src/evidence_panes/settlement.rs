//! Settlement records in the Evidence panes: the payer node's own sealed
//! observations of `payment.lifecycle.v1`, joined to exchanges by
//! `exchange_id`.
//!
//! The plugin seals one record per lifecycle event it observed, carrying the
//! event's fields verbatim under
//! `model_attestation.compute_attestation["x-mesh-settlement-v1"]`. This
//! module only reads those records; it never computes a sum, a balance or a
//! rate, and it never reaches into the wallet.
//!
//! What one node's records can say, and what they cannot:
//!
//! - A payer book is built from this node's records as the paying side only.
//!   The provider of the same exchange keeps its book on its own node, so a
//!   payer book always reports it as `not_available`.
//! - Records this node sealed as a provider (`observed_by: "provider"`) are
//!   its own book. They never enter a payer book; the host's provider-side
//!   ids do not join this node's exchange records yet, so they are counted
//!   (`settlement_provider_records`).
//! - The provider's side of a payment is not something the payer's events
//!   can see, so nothing about it is sent: per-peer counts carry only this
//!   node's own facts, beside `provider_book: "not_available"`.
//! - An exchange with no settlement records has no payment summary at all
//!   (`settlement: null`). That covers a free exchange, a node with payments
//!   off, and a paid request that failed before authorization (the host emits
//!   no lifecycle phase for it). None of those is "unpaid".
//! - The payer's invoice record can arrive after the first token (the host
//!   authorizes at the first canonical token), so nothing here orders
//!   settlement records against the exchange's own record.

use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};

/// Where the plugin records the observed event.
const SETTLEMENT_BLOCK: &str = "/model_attestation/compute_attestation/x-mesh-settlement-v1";

/// The provider's book of a payer's exchange is kept on the provider's node,
/// so it is never available to this reader.
pub(super) const PROVIDER_BOOK_NOT_AVAILABLE: &str = "not_available";

/// Every invoice this node saw issued has a settlement its own wallet
/// reported, under the same payment hash and segment.
const PAYER_SETTLED: &str = "settled";
/// At least one invoice has no settlement reported by this node's wallet.
/// The payer's events cannot tell why (not yet paid, never paid, or paid and
/// not reported), so this is a fact about the payer's book, not a lapse.
const PAYER_NO_SETTLEMENT_SEEN: &str = "no_settlement_seen";
/// Terms were accepted but no invoice was recorded.
const PAYER_TERMS_ONLY: &str = "terms_only";
/// A settlement names a payment hash no invoice of this exchange named.
const PAYER_UNMATCHED_SETTLEMENT: &str = "unmatched_settlement";

pub(super) fn settlement_block(record: &Value) -> Option<&Value> {
    record.pointer(SETTLEMENT_BLOCK)
}

/// A settlement record is identified by its block alone.
pub(super) fn is_settlement_record(record: &Value) -> bool {
    settlement_block(record).is_some()
}

fn block_str<'a>(record: &'a Value, key: &str) -> Option<&'a str> {
    settlement_block(record)?
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// The settlement records this node holds, keyed by the `exchange_id` each
/// one records, in ledger order.
#[derive(Default)]
pub(super) struct SettlementIndex {
    by_exchange: HashMap<String, Vec<Value>>,
    /// Ledger order of first appearance, so unjoined ids are reported
    /// deterministically.
    order: Vec<String>,
    /// Settlement records carrying no usable `exchange_id`: nothing can join
    /// them, so they are counted rather than dropped.
    missing_exchange_id: usize,
    /// Records this node sealed as the provider of a paid exchange. They are
    /// its own book, never part of a payer book, and the host's provider-side
    /// ids do not join its exchange records, so they are counted.
    provider_records: usize,
}

impl SettlementIndex {
    pub(super) fn push(&mut self, record: Value) {
        if block_str(&record, "observed_by") == Some("provider") {
            self.provider_records += 1;
            return;
        }
        let Some(exchange_id) = block_str(&record, "exchange_id").map(str::to_string) else {
            self.missing_exchange_id += 1;
            return;
        };
        if !self.by_exchange.contains_key(&exchange_id) {
            self.order.push(exchange_id.clone());
        }
        self.by_exchange
            .entry(exchange_id)
            .or_default()
            .push(record);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.by_exchange.is_empty() && self.missing_exchange_id == 0 && self.provider_records == 0
    }

    pub(super) fn provider_records(&self) -> usize {
        self.provider_records
    }

    pub(super) fn missing_exchange_id(&self) -> usize {
        self.missing_exchange_id
    }

    /// The payer-book summary for the exchange ids one row's records carry,
    /// or `None` when none of them has a settlement record.
    ///
    /// Each exchange id is its own book: one exchange's settlement never
    /// settles another's invoice. A row carrying more than one id shows the
    /// WORST of their states, with every entry, so a merged row can never
    /// read "settled" while one of its exchanges is not.
    pub(super) fn summary_for<'a>(
        &self,
        exchange_ids: impl IntoIterator<Item = &'a str>,
    ) -> Option<Value> {
        let mut seen = BTreeSet::new();
        let mut books: Vec<(&str, Value)> = Vec::new();
        for id in exchange_ids {
            if !seen.insert(id) {
                continue;
            }
            if let Some(records) = self.by_exchange.get(id) {
                let entries: Vec<&Value> = records.iter().collect();
                books.push((id, payer_book(&entries)));
            }
        }
        match books.len() {
            0 => None,
            1 => books.pop().map(|(id, mut book)| {
                book["exchange_ids"] = json!([id]);
                book
            }),
            _ => Some(worst_book(books)),
        }
    }

    /// Settlement exchange ids no pane row carries, so the page can say the
    /// records exist instead of dropping them.
    pub(super) fn unjoined<'a>(&self, joined: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let joined: BTreeSet<&str> = joined.into_iter().collect();
        self.order
            .iter()
            .filter(|id| !joined.contains(id.as_str()))
            .cloned()
            .collect()
    }
}

/// How bad a payer-book state is, for a row that carries several exchanges:
/// the row shows the worst. A state this reader does not know ranks worst.
fn state_rank(state: &str) -> u8 {
    match state {
        PAYER_TERMS_ONLY => 0,
        PAYER_SETTLED => 1,
        PAYER_NO_SETTLEMENT_SEEN => 2,
        _ => 3,
    }
}

/// Several exchanges' books as one row summary: the worst state, every entry
/// in book order, every terms digest, and the exchange ids it covers.
fn worst_book(books: Vec<(&str, Value)>) -> Value {
    let mut state = PAYER_TERMS_ONLY;
    let mut matched_by_segment_only = false;
    let mut terms_digests: BTreeSet<String> = BTreeSet::new();
    let mut entries: Vec<Value> = Vec::new();
    let mut exchange_ids: Vec<&str> = Vec::new();
    for (id, book) in &books {
        let book_state = book["state"].as_str().unwrap_or_default();
        if state_rank(book_state) > state_rank(state) {
            state = match book_state {
                PAYER_SETTLED => PAYER_SETTLED,
                PAYER_NO_SETTLEMENT_SEEN => PAYER_NO_SETTLEMENT_SEEN,
                _ => PAYER_UNMATCHED_SETTLEMENT,
            };
        }
        // "No reference" describes a settled book; it carries to the row only
        // while the row still reads settled.
        matched_by_segment_only |=
            book_state == PAYER_SETTLED && book["matched_by_segment_only"].as_bool() == Some(true);
        if let Some(digests) = book["terms_digests"].as_array() {
            terms_digests.extend(digests.iter().filter_map(Value::as_str).map(str::to_string));
        }
        if let Some(rows) = book["entries"].as_array() {
            entries.extend(rows.iter().cloned());
        }
        exchange_ids.push(id);
    }
    json!({
        "observed_by": "payer",
        "state": state,
        "terms_digests": terms_digests.into_iter().collect::<Vec<_>>(),
        "entries": entries,
        "matched_by_segment_only": state == PAYER_SETTLED && matched_by_segment_only,
        "provider_book": PROVIDER_BOOK_NOT_AVAILABLE,
        "exchange_ids": exchange_ids,
    })
}

/// The payer's book for one exchange, from its settlement records.
///
/// `state` compares invoices with settlements by `(segment, payment_hash)`:
/// the one identifier both wallets share, and the one the host's events carry.
/// The host copies a settlement's hash from the wallet's transaction, which
/// may not carry one; such a settlement can only be matched to its segment's
/// invoice, and the summary says so (`matched_by_segment_only`).
/// Amounts are copied from each record and never added up.
fn payer_book(entries: &[&Value]) -> Value {
    let mut invoices: BTreeSet<(u64, &str)> = BTreeSet::new();
    let mut settlements: BTreeSet<(u64, &str)> = BTreeSet::new();
    let mut hashless_settlement_segments: BTreeSet<u64> = BTreeSet::new();
    let mut terms_digests: BTreeSet<&str> = BTreeSet::new();
    let mut rows: Vec<Value> = Vec::with_capacity(entries.len());
    for record in entries {
        let Some(block) = settlement_block(record) else {
            continue;
        };
        let phase = block
            .get("phase")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let segment = block.get("segment").and_then(Value::as_u64);
        let payment_hash = block
            .get("payment_hash")
            .and_then(Value::as_str)
            .filter(|h| !h.is_empty());
        // Only the wallet's own report settles an invoice. A settlement record
        // stated by anyone else stays an entry on the row and never reads
        // "settled".
        let wallet_settlement = phase.ends_with("_settlement_observed")
            && block.get("source").and_then(Value::as_str) == Some("wallet_reported");
        match (segment, payment_hash) {
            (Some(segment), Some(hash)) if phase.ends_with("_invoice_issued") => {
                invoices.insert((segment, hash));
            }
            (Some(segment), Some(hash)) if wallet_settlement => {
                settlements.insert((segment, hash));
            }
            (Some(segment), None) if wallet_settlement => {
                hashless_settlement_segments.insert(segment);
            }
            _ => {}
        }
        if let Some(digest) = block
            .get("terms_digest")
            .and_then(Value::as_str)
            .filter(|d| !d.is_empty())
        {
            terms_digests.insert(digest);
        }
        rows.push(json!({
            "capsule_id": record.get("capsule_id").cloned().unwrap_or(Value::Null),
            "timestamp": record.get("timestamp").cloned().unwrap_or(Value::Null),
            "phase": phase,
            "source": block.get("source").cloned().unwrap_or(Value::Null),
            "segment": segment,
            "payment_hash": payment_hash,
            "amount_msat": block.get("amount_msat").cloned().unwrap_or(Value::Null),
        }));
    }
    let invoice_segments: BTreeSet<u64> = invoices.iter().map(|(segment, _)| *segment).collect();
    let mut invoices_per_segment: HashMap<u64, usize> = HashMap::new();
    for (segment, _) in &invoices {
        *invoices_per_segment.entry(*segment).or_default() += 1;
    }
    let mut matched_by_segment_only = false;
    let all_invoices_settled = invoices.iter().all(|invoice| {
        if settlements.contains(invoice) {
            return true;
        }
        // A settlement with no hash can only name its segment, so it settles
        // an invoice only when that segment has exactly one: with two (a
        // retry under the same exchange id), it cannot say which was paid.
        let by_segment = hashless_settlement_segments.contains(&invoice.0)
            && invoices_per_segment.get(&invoice.0) == Some(&1);
        matched_by_segment_only |= by_segment;
        by_segment
    });
    let state = if !settlements.is_subset(&invoices)
        || !hashless_settlement_segments.is_subset(&invoice_segments)
    {
        PAYER_UNMATCHED_SETTLEMENT
    } else if invoices.is_empty() {
        PAYER_TERMS_ONLY
    } else if all_invoices_settled {
        PAYER_SETTLED
    } else {
        PAYER_NO_SETTLEMENT_SEEN
    };
    json!({
        "observed_by": "payer",
        "state": state,
        // One digest when every record agrees; all of them otherwise, so a
        // disagreement is shown rather than resolved here.
        "terms_digests": terms_digests.into_iter().collect::<Vec<_>>(),
        "entries": rows,
        "matched_by_segment_only": matched_by_segment_only,
        "provider_book": PROVIDER_BOOK_NOT_AVAILABLE,
    })
}

/// Per-peer settlement counts from the payer-book summaries of the peer's
/// exchanges. Counts only: no amounts, no rates, and nothing about the
/// provider's side, which this node cannot observe (`provider_book`).
pub(super) fn peer_counts(summaries: &[Value]) -> Value {
    let count = |state: &str| {
        summaries
            .iter()
            .filter(|s| s.get("state").and_then(Value::as_str) == Some(state))
            .count()
    };
    // An exchange whose terms were accepted but that never got an invoice is
    // not paid: it is counted on its own.
    json!({
        "paid_exchanges": summaries.len() - count(PAYER_TERMS_ONLY),
        "terms_only": count(PAYER_TERMS_ONLY),
        "settled_payer_observed": count(PAYER_SETTLED),
        "no_settlement_seen": count(PAYER_NO_SETTLEMENT_SEEN),
        "provider_book": PROVIDER_BOOK_NOT_AVAILABLE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(
        exchange_id: &str,
        phase: &str,
        source: &str,
        segment: Option<u64>,
        hash: Option<&str>,
        amount: u64,
    ) -> Value {
        let mut block = json!({
            "v": 1,
            "observed_by": "payer",
            "exchange_id": exchange_id,
            "event_ref": format!("{exchange_id}-{phase}"),
            "terms_digest": "t".repeat(64),
            "phase": phase,
            "source": source,
            "amount_msat": amount,
        });
        if let Some(segment) = segment {
            block["segment"] = json!(segment);
        }
        if let Some(hash) = hash {
            block["payment_hash"] = json!(hash);
        }
        json!({
            "capsule_id": format!("cap-{exchange_id}-{phase}"),
            "timestamp": "2026-09-27T00:00:00Z",
            "model_attestation": { "compute_attestation": { "x-mesh-settlement-v1": block } },
        })
    }

    fn paid_and_settled(exchange_id: &str) -> Vec<Value> {
        vec![
            event(
                exchange_id,
                "terms_accepted",
                "payer_asserted",
                None,
                None,
                900,
            ),
            event(
                exchange_id,
                "input_invoice_issued",
                "provider_asserted",
                Some(0),
                Some("aa"),
                120,
            ),
            event(
                exchange_id,
                "input_settlement_observed",
                "wallet_reported",
                Some(0),
                Some("aa"),
                120,
            ),
            event(
                exchange_id,
                "output_invoice_issued",
                "provider_asserted",
                Some(1),
                Some("bb"),
                457,
            ),
            event(
                exchange_id,
                "output_settlement_observed",
                "wallet_reported",
                Some(1),
                Some("bb"),
                457,
            ),
            event(
                exchange_id,
                "final_accounted",
                "payer_asserted",
                None,
                None,
                577,
            ),
        ]
    }

    fn index(records: Vec<Value>) -> SettlementIndex {
        let mut index = SettlementIndex::default();
        for record in records {
            index.push(record);
        }
        index
    }

    #[test]
    fn every_invoice_settled_by_this_wallet_reads_settled_with_amounts_verbatim() {
        let index = index(paid_and_settled("ex-1"));
        let summary = index.summary_for(["ex-1"]).expect("records exist");
        assert_eq!(summary["state"], "settled");
        assert_eq!(summary["observed_by"], "payer");
        assert_eq!(summary["provider_book"], "not_available");
        let entries = summary["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 6);
        // Recorded values, never a computed total.
        assert_eq!(entries[3]["amount_msat"], 457);
        assert_eq!(entries[5]["amount_msat"], 577);
        assert!(summary.get("total_msat").is_none());
    }

    /// Two paid exchanges with the same request body. Exchange 1
    /// has invoice seg0 `aa` and a hash-less wallet settlement for seg0;
    /// exchange 2 has invoice seg0 `bb` and nothing paid. A row carrying both
    /// must not read settled: each id is its own book, the row shows the worst.
    #[test]
    fn one_exchange_settlement_never_settles_another_exchange_invoice() {
        let records = vec![
            event(
                "ex-1",
                "input_invoice_issued",
                "provider_asserted",
                Some(0),
                Some("aa"),
                120,
            ),
            event(
                "ex-1",
                "input_settlement_observed",
                "wallet_reported",
                Some(0),
                None,
                120,
            ),
            event(
                "ex-2",
                "input_invoice_issued",
                "provider_asserted",
                Some(0),
                Some("bb"),
                120,
            ),
        ];
        let index = index(records);
        let merged = index.summary_for(["ex-1", "ex-2"]).unwrap();
        assert_eq!(merged["state"], "no_settlement_seen");
        assert_eq!(merged["matched_by_segment_only"], false);
        assert_eq!(merged["exchange_ids"], json!(["ex-1", "ex-2"]));
        assert_eq!(merged["entries"].as_array().unwrap().len(), 3);
        // Each on its own: exchange 1 settled (by segment), exchange 2 not.
        let one = index.summary_for(["ex-1"]).unwrap();
        assert_eq!(one["state"], "settled");
        assert_eq!(one["matched_by_segment_only"], true);
        assert_eq!(
            index.summary_for(["ex-2"]).unwrap()["state"],
            "no_settlement_seen"
        );
    }

    /// The retry: a paid retry reuses the exchange id, so one
    /// book holds two seg0 invoices. A hash-less settlement cannot say which
    /// was paid, so it settles neither.
    #[test]
    fn a_hashless_settlement_under_a_retried_segment_settles_nothing() {
        let records = vec![
            event(
                "ex-r",
                "input_invoice_issued",
                "provider_asserted",
                Some(0),
                Some("aa"),
                120,
            ),
            event(
                "ex-r",
                "input_invoice_issued",
                "provider_asserted",
                Some(0),
                Some("bb"),
                120,
            ),
            event(
                "ex-r",
                "input_settlement_observed",
                "wallet_reported",
                Some(0),
                None,
                120,
            ),
        ];
        let summary = index(records).summary_for(["ex-r"]).unwrap();
        assert_eq!(summary["state"], "no_settlement_seen");
        assert_eq!(summary["matched_by_segment_only"], false);
    }

    #[test]
    fn a_row_of_settled_and_terms_only_exchanges_reads_settled() {
        let mut records = paid_and_settled("ex-a");
        records.push(event(
            "ex-b",
            "terms_accepted",
            "payer_asserted",
            None,
            None,
            5,
        ));
        let merged = index(records).summary_for(["ex-a", "ex-b"]).unwrap();
        assert_eq!(merged["state"], "settled");
    }

    #[test]
    fn a_settlement_not_reported_by_the_wallet_never_reads_settled() {
        for hash in [Some("bb"), None] {
            let mut records = paid_and_settled("ex-src");
            records[4] = event(
                "ex-src",
                "output_settlement_observed",
                "provider_asserted",
                Some(1),
                hash,
                457,
            );
            let summary = index(records).summary_for(["ex-src"]).unwrap();
            assert_eq!(summary["state"], "no_settlement_seen", "hash {hash:?}");
            // The record is still shown, as what it is.
            assert_eq!(summary["entries"][4]["source"], "provider_asserted");
        }
    }

    #[test]
    fn an_output_invoice_without_its_settlement_is_no_settlement_seen() {
        let mut records = paid_and_settled("ex-2");
        records.remove(4);
        let summary = index(records).summary_for(["ex-2"]).unwrap();
        assert_eq!(summary["state"], "no_settlement_seen");
    }

    #[test]
    fn a_settlement_under_another_segment_does_not_settle_the_invoice() {
        let records = vec![
            event(
                "ex-3",
                "input_invoice_issued",
                "provider_asserted",
                Some(0),
                Some("aa"),
                1,
            ),
            event(
                "ex-3",
                "input_settlement_observed",
                "wallet_reported",
                Some(1),
                Some("aa"),
                1,
            ),
        ];
        let summary = index(records).summary_for(["ex-3"]).unwrap();
        assert_eq!(summary["state"], "unmatched_settlement");
    }

    #[test]
    fn a_settlement_naming_no_invoice_is_unmatched_even_when_the_invoices_settled() {
        let mut records = paid_and_settled("ex-4");
        records.push(event(
            "ex-4",
            "output_settlement_observed",
            "wallet_reported",
            Some(2),
            Some("cc"),
            3,
        ));
        let summary = index(records).summary_for(["ex-4"]).unwrap();
        assert_eq!(summary["state"], "unmatched_settlement");
    }

    #[test]
    fn a_settlement_with_no_hash_settles_its_segment_and_says_how_it_matched() {
        let mut records = paid_and_settled("ex-7");
        records[2] = event(
            "ex-7",
            "input_settlement_observed",
            "wallet_reported",
            Some(0),
            None,
            120,
        );
        let summary = index(records).summary_for(["ex-7"]).unwrap();
        assert_eq!(summary["state"], "settled");
        assert_eq!(summary["matched_by_segment_only"], true);
        let full = index(paid_and_settled("ex-8"))
            .summary_for(["ex-8"])
            .unwrap();
        assert_eq!(full["matched_by_segment_only"], false);
    }

    #[test]
    fn a_hashless_settlement_for_a_segment_with_no_invoice_is_unmatched() {
        let records = vec![
            event(
                "ex-9",
                "input_invoice_issued",
                "provider_asserted",
                Some(0),
                Some("aa"),
                1,
            ),
            event(
                "ex-9",
                "input_settlement_observed",
                "wallet_reported",
                Some(0),
                Some("aa"),
                1,
            ),
            event(
                "ex-9",
                "output_settlement_observed",
                "wallet_reported",
                Some(1),
                None,
                1,
            ),
        ];
        let summary = index(records).summary_for(["ex-9"]).unwrap();
        assert_eq!(summary["state"], "unmatched_settlement");
    }

    #[test]
    fn terms_without_an_invoice_read_terms_only() {
        let records = vec![event(
            "ex-5",
            "terms_accepted",
            "payer_asserted",
            None,
            None,
            900,
        )];
        let summary = index(records).summary_for(["ex-5"]).unwrap();
        assert_eq!(summary["state"], "terms_only");
    }

    #[test]
    fn an_exchange_with_no_settlement_records_has_no_summary() {
        // Free, payments off, or failed before authorization: nothing to say,
        // and in particular never "unpaid".
        let index = index(paid_and_settled("ex-paid"));
        assert!(index.summary_for(["ex-free"]).is_none());
    }

    #[test]
    fn provider_records_are_counted_and_never_enter_a_payer_book() {
        let mut provider = event("ex-a", "terms_accepted", "provider_asserted", None, None, 0);
        provider["model_attestation"]["compute_attestation"]["x-mesh-settlement-v1"]
            ["observed_by"] = json!("provider");
        let index = index(vec![provider]);
        assert_eq!(index.provider_records(), 1);
        assert!(!index.is_empty());
        assert!(index.summary_for(["ex-a"]).is_none());
        assert!(index.unjoined([]).is_empty());
    }

    #[test]
    fn records_for_ids_no_row_carries_are_reported_as_unjoined() {
        let mut records = paid_and_settled("ex-a");
        records.extend(paid_and_settled("ex-b"));
        let index = index(records);
        assert_eq!(index.unjoined(["ex-a"]), vec!["ex-b".to_string()]);
        assert!(index.unjoined(["ex-a", "ex-b"]).is_empty());
    }

    #[test]
    fn differing_terms_digests_are_all_listed() {
        let mut records = paid_and_settled("ex-6");
        records[1]["model_attestation"]["compute_attestation"]["x-mesh-settlement-v1"]
            ["terms_digest"] = json!("u".repeat(64));
        let summary = index(records).summary_for(["ex-6"]).unwrap();
        assert_eq!(summary["terms_digests"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn peer_counts_are_counts_and_leave_provider_states_unavailable() {
        let mut records = paid_and_settled("ex-p1");
        let mut unsettled = paid_and_settled("ex-p2");
        unsettled.remove(2);
        records.extend(unsettled);
        let index = index(records);
        let summaries: Vec<Value> = ["ex-p1", "ex-p2"]
            .iter()
            .filter_map(|id| index.summary_for([*id]))
            .collect();
        let counts = peer_counts(&summaries);
        assert_eq!(counts["paid_exchanges"], 2);
        assert_eq!(counts["settled_payer_observed"], 1);
        assert_eq!(counts["no_settlement_seen"], 1);
        assert_eq!(counts["provider_book"], "not_available");
        // Nothing about the provider's side is sent, not even as null.
        for key in ["lapsed", "debt", "settled_both_books"] {
            assert!(counts.get(key).is_none(), "{key}");
        }
    }
}
