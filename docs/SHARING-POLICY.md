<!-- SPDX-License-Identifier: Apache-2.0 -->
# Sharing policy

Every sharing decision the plugin makes is one of four switches, and every
default keys on **relationship** (whether this node has exchanged with the
other node), never on proximity, latency, or any computed standing. Nothing
here is a score, and nothing here ranks anyone.

```
share_record_at_completion: counterparty | off                           # default: off
share_history_segments:     counterparties | prospective | peers | off   # default: prospective
share_adjudications:        deliver_to_subjects | off                    # default: off
witness:                    [] | [{endpoint, public_key?}, ...]          # default: [] (off)
```

With these defaults a node sends nothing to a peer on its own: it pushes no
record and delivers no verdict, and it only answers a peer that asks for one
of its records (`share_history_segments`). The referee
(`adjudicate_differing_twins`) is off by default too.

The plugin declares the four switches in its `config_schema`
(`crates/evidence-plugin/src/share_policy.rs`), so the console shows them under
Configuration › Plugins › Sharing policy. mesh-llm keeps what you save there in
its config file and does not pass it to the plugin, so the plugin reads it
there (`~/.mesh-llm/config.toml`, or the file `MESH_LLM_CONFIG` names). The
same settings can be set in the plugin's environment, which wins over the
console: `CAPSULES_SHARE_RECORD_AT_COMPLETION`,
`CAPSULES_SHARE_HISTORY_SEGMENTS`, `CAPSULES_SHARE_ADJUDICATIONS` and
`CAPSULES_CHECKPOINT_WITNESS_URLS` (a comma list of witness URLs) with
`CAPSULES_CHECKPOINT_WITNESS_KEYS` (a JSON object from witness URL to its key
in hex). An unset or unknown value means the default.

## The record at completion (`share_record_at_completion`)

Off by default. When it is on (`counterparty`) and an exchange ends, each side
sends its own **sealed record** to the other side over the plugin's
`record-push/1` mesh stream: the provider's record to
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

Off by default. When it is on (`deliver_to_subjects`) and a referee's verdict
about an exchange exists, it is delivered to every node it concerns, and each
seals its own record citing it. A node holds a
delivered verdict only when the referee it names signed it with its announced
key and the verdict concerns that node (it asked for it, or it judges one of
that node's own records). With `share_adjudications: off` the node that asked
still holds the verdict, and delivers it to no one. How a referee is chosen:
[TWIN-REFEREE-SELECTION.md](TWIN-REFEREE-SELECTION.md).

**Asking a referee sends a prompt to a third node.** With
`adjudicate_differing_twins` on (off by default), when two twins of a pair a
client marked (`x-mesh-twin-bracket`) answered differently, this node sends the
twins' own request, including the prompt's `messages`, to one eligible peer
(`x-mesh-target`), which answers it with its own inference; that answer and
both twins' halves then go to it to adjudicate. It needs the exchange text
kept on this node, a separate setting (`CAPSULES_KEEP_EXCHANGE_TEXT`, off)
that keeping text locally does not imply the other way round: keeping text
never sends it to a peer. The operator's "Ask again" is the same switch.

## Witness

`witness` is a list of witnesses, empty for off (the default). Each is an
`endpoint` (the witness's URL) and, if you have it, its `public_key` (its
Ed25519 key in hex, as its operator publishes it): the same endpoint-and-key
pair capsule-cli takes. There is no default witness: with the list empty the
plugin contacts no witness at all.

Each checkpoint is offered to every witness on the list. A witness that
accepts it returns a receipt, and the Evidence page lists every witness by
name with what it holds: the latest checkpoint, an earlier one, or none yet.

A receipt counts only once the plugin has **checked** it, offline. It
recomputes the checkpoint's digest from the checkpoint's own signed fields,
confirms the receipt is for exactly that entry, and verifies the receipt's
inclusion proof and signature under the witness's key: the key you gave, or,
when you gave none, a key fetched from that witness the first time it is
needed and kept (`<ledger>/witness-keys.json`). A different key later from the
same URL is not taken. The page says which: "key configured", or "key pinned
on first contact, not configured". A key you configure is the stronger choice:
you got it from the witness's operator, not from the witness's own answer.
"Witnessed" on the page means at least one checked receipt, from a witness it
names.

A named witness that does not hold the latest checkpoint is asked for its key
again, at most once a minute and only that witness, so its row can say why:
it did not answer in time, refused the connection, answered with an HTTP
error, or presents a different key than the one configured or pinned. A
witness that holds the latest checkpoint is not contacted for this.

A witness holding a checkpoint means one thing: a later rewrite of this log is
detectable by someone other than this node. It does not make the records true.

## The two mesh streams are plugin-internal

`record-push/1` (a record sent at completion) and `ledger-fetch/1` (a record
read back by id) are this plugin's own behaviour between two copies of
itself. No public specification defines them. They are not a standardized
wire, other implementations should not target them, and they may change
between releases. Neither stores and forwards a record, and neither promises
delivery.

## Three rules every switch follows

1. **Filter, never rank.** Every switch answers or refuses; none orders or
   weights a peer.
2. **Counts, not sums.** Where counts are reported, they are counts; nothing
   is totalled or averaged into a standing.
3. **Relationship, never proximity.** Every default keys on whether this node
   has actually exchanged with the other node.
