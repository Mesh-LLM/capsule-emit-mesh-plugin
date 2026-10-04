<!-- SPDX-License-Identifier: Apache-2.0 -->
# Sharing policy

Every sharing decision the plugin makes is one of four switches, and every
default keys on **relationship** (whether this node has exchanged with the
other node), never on proximity, latency, or any computed standing. Nothing
here is a score, and nothing here ranks anyone.

```
share_record_at_completion: counterparty | off                           # default: counterparty
share_history_segments:     counterparties | prospective | peers | off   # default: prospective
share_adjudications:        deliver_to_subjects | off                    # default: on
witness:                    off | <url>                                  # default: off
```

The plugin declares the four switches in its `config_schema`
(`crates/evidence-plugin/src/share_policy.rs`), so the console shows them under
Configuration › Plugins › Sharing policy. mesh-llm does not yet pass a
plugin's configured values back to the running plugin, so today each value is
read from the plugin's environment: `CAPSULES_SHARE_RECORD_AT_COMPLETION`,
`CAPSULES_SHARE_HISTORY_SEGMENTS`, `CAPSULES_SHARE_ADJUDICATIONS`
and `CAPSULES_WITNESS`. An unset or unknown value means the default.

## The record at completion (`share_record_at_completion`)

When an exchange ends, each side sends its own **sealed record** to the other
side over the plugin's `record-push/1` mesh stream: the provider's record to
the requester, and the requester's to the provider. With the checkpoint
cadence on (the default), the record goes out as a **bundle**: the record, a
signed checkpoint of the sender's log, and the record's inclusion proof under
it.

The record contains nothing the other side can't already compute (they hold
the bytes, so both digests are theirs to derive). What it adds is the time it
was sealed, the signature, and, in a bundle, the size and root of the sender's
log and the record's position in it. No other record's content leaves: the
proof is hashes only. With both records exchanged, each side holds the other's
signed statement about the same exchange.

**Symmetric by default:** a node with this switch `off` neither sends its
record nor accepts the other side's.

**In this build:** the plugin sends its record when an exchange ends and
mesh-llm has told it who the other side is. A pushed record is received and
checked in-process (the sender's announced key, the signature, the claims, and
a bundle's proof and checkpoint) and held only if every check passes; anything
else gets a signed refusal. The sender's key must be configured
(`CAPSULES_PEER_KEYS`, see INSTALL.md): nodes do not exchange keys yet.

## Reading records back (`share_history_segments`)

A peer can ask this node for one of its records by id, over the plugin's
`ledger-fetch/1` mesh stream. The switch decides who gets an answer:

| value | counterparty (a node this record names as the other side) | any other peer |
| --- | :---: | :---: |
| `off` | | |
| `counterparties` | ✓ | |
| `prospective` (default) | ✓ | |
| `peers` | ✓ | ✓ |

A local block and an owner-maintenance record are never served. A declined
request is answered `not_authorized`. The requester's identity is the peer id
the asking node states for itself: mesh-llm does not yet tell the plugin which
peer opened a stream. This is a scope filter against casual reads, not access
control: a record id is not guessable, and `peers` hands out the same records
anyway.

**This trusts the id the request states.** Under `prospective` and
`counterparties` a peer that lies about its id receives the records the peer
it names would receive: the switch scopes what this node discloses, it is not
access control.

**Evidence requests** (`evidence-request/1`, draft-mih-agent-evidence-request-00)
are answered in-process by `evidence_answer.rs`, with the protocol from the
`capsule-emit-evidence-request` crate. The same rule applies to every record
an answer would carry, for every subject that carries record bodies (`record`,
`range`, `full_history`, `correlation`, `exchange`). The asker's id is the
request's own optional `requester_id` member, as it states it. An answer is
served whole or refused `not_authorized` whole, never filtered per asker,
because an artifact is the same for every requester. The checkpoint list, the
`history_card/1` derivation and the `served_summary/1` derivation carry no
record bodies and are answered under every tier. The golden answers are in
`vectors/parity/evidence_request/`.

## Verdicts (`share_adjudications`)

When a referee's verdict about an exchange exists, it is delivered to every
node it concerns, and each seals its own record citing it. A node holds a
delivered verdict only when the referee it names signed it with its announced
key and the verdict concerns that node (it asked for it, or it judges one of
that node's own records). With `share_adjudications: off` the node that asked
still holds the verdict, and delivers it to no one. How a referee is chosen:
[TWIN-REFEREE-SELECTION.md](TWIN-REFEREE-SELECTION.md).

## Witness

`witness` is a URL, or unset for off (the default). With no URL set there is
no witness and no network call to one.

## The two mesh streams are plugin-internal

`record-push/1` (a record sent at completion) and `ledger-fetch/1` (a record
read back by id) are this plugin's own behaviour between two copies of
itself. No public specification defines them. They are not a standardized
wire, other implementations should not target them, and they may change
between releases. Neither stores and forwards a record, and neither promises
delivery.

## Three rules that keep this from becoming a score

1. **Filter, never rank.** Every switch answers or refuses; none orders or
   weights a peer.
2. **Counts, not sums.** Where counts are reported, they are counts; nothing
   is totalled or averaged into a standing.
3. **Relationship, never proximity.** Every default keys on whether this node
   has actually exchanged with the other node.
