# Changelog

## Unreleased

### Changed

- **The plugin reads and hashes its own executable once per process**, at the first record it
  seals, instead of for every record. Each record still carries that measurement, signed with the
  node key and stamped with the record's own time, so records are unchanged. On macOS the kernel's
  code-signing status is still asked for every record, because it can change while the process
  runs. Replacing the plugin's executable on disk while it runs is measured only after a restart,
  which is also when the new binary starts running.

## 0.1.3

### Changed

- **Nothing reaches a peer on its own by default.** `share_record_at_completion` (push this node's
  record to the counterparty), `share_adjudications` (deliver verdicts) and
  `adjudicate_differing_twins` (ask a referee, which sends the twins' request to a third node) are
  **off by default**; each turns on only on its explicit value. Answering a peer that asks for a
  record (`share_history_segments`) is unchanged.
- **The stop-routing rule asks the host through the plugin protocol** (`PeerBlockRequest`), so the
  host records the block as this plugin's and refuses it unless the operator set
  `allow_peer_blocks = true` for this plugin. The plugin no longer calls the console's
  `/api/peer-blocks` route (as the operator) for this or for choosing a referee; it learns blocks
  from the host's `routing.choice.v1` publications, and seals its own record of a rule block.
- **The twin pair says what happened:** "A client marked these two requests as one pair", not that
  a comparison ran automatically.
- **Renamed to `capsules`** (plugin id, repository `Mesh-LLM/capsules`, release assets
  `capsules-<version>-<target>.tar.gz`, page `/plugins/capsules/evidence`). Settings are
  `CAPSULES_*`; the old `CAPSULE_EMIT_MESH_*` names are read for this release, with a warning. The
  `ADMISSION_POLICY_*` names are no longer read.
- **An upgraded node keeps its data directory in place** under the old name
  (`capsule-emit-mesh`); nothing is moved, so an upgrade never races a writer that is still
  running. A node with both directories refuses to start and says why. Existing logs keep their
  log id. On Linux the plugin refuses a log another process has open.

### Added

- **A list of witnesses.** The console's `witness` setting is a list of `{ endpoint, public_key }`
  rows, the key optional (the environment's `CAPSULES_CHECKPOINT_WITNESS_URLS` stays a comma list,
  with keys in `CAPSULES_CHECKPOINT_WITNESS_KEYS`, a JSON object from URL to key). There is still no default witness, and no
  witness is contacted until one is named. The Evidence page lists each witness by name with what
  it holds.
- **Receipts are checked before they count.** A witness's receipt counts only when it verifies
  offline: the entry recomputed from the checkpoint's signed fields, and the receipt's inclusion
  proof and signature under the witness's key: the one configured, else one fetched once from that
  witness and kept, which the page labels "pinned on first contact, not configured". An
  unchecked or failing receipt is shown, with the reason, and never counted. A witness that does
  not hold the latest checkpoint (or whose receipt for it does not check) says why: timed out,
  refused, an HTTP error, or a different key.
- **The Evidence page can be a primary tab** (mesh-llm 0.78 and later). The page asks for primary
  placement; the operator turns it on in the console's plugin settings ("Primary tab"), and with it
  off the page stays a navigation item. The page draws its own header, so the host's header is
  turned off for it.
- **Under a chat answer and in the Logs request inspector** (mesh-llm 0.78 and later): whether
  this node sealed that exchange, with a link to its record on the Evidence page. The lookup is
  local (`GET /lookup?exchange_id=` or `?client_nonce=`); nothing is shown when it fails or finds
  no record. It works on the node that asked a peer for the answer. For an exchange this node
  served itself, mesh-llm 0.78 does not give the plugin the ids those views hold, so nothing is
  shown there. On mesh-llm 0.77 the plugin installs and runs as before, without these.

### Fixed

- **The log of requests made of this node never stops the plugin starting.** It is written only to
  a directory that exists when the plugin starts (`CAPSULES_RECEIVED_LOG_DIR`, else
  `<data dir>/received-log`) and that this process holds a lock on; never the ledger directory. A
  directory another node holds, or one that cannot be locked, turns that log off for the run, with
  a warning.
- **A witness is dialled at the URL given.** With capsule-emit 0.0.4, one public witness URL
  (`https://witness.agentactioncapsule.org`) was sent to a different host
  (`anchor.agentactioncapsule.org`). capsule-emit 0.0.6, now required, dials every witness exactly as configured and
  follows no redirect, and the plugin's own witness reads do the same.
- **Settings saved in the console reach the plugin.** The plugin reads its `[plugin.settings]`
  from mesh-llm's config file (`MESH_LLM_CONFIG`, or `~/.mesh-llm/config.toml`); the environment
  still wins. The console's `witness` is the checkpoint witness, and the witness client uses it.
- **One plugin process per data directory.** A second process on the same directory refuses to
  start instead of interleaving appends and breaking the chain.

## 0.1.2

### Fixed

- **The plugin serves no models unless asked.** 0.1.1 registered an OpenAI-compatible inference
  provider on every node and, with `CAPSULE_EMIT_MESH_BLOCKED_MODELS` unset or empty, advertised
  `blocked-test-model` through it. That model always answers 403, so a client that takes the first
  model `/v1/models` lists, or "Mesh automatic" routing, could pick it. From 0.1.2 the admission
  endpoint is opt-in: only when `CAPSULE_EMIT_MESH_BLOCKED_MODELS` names models does the plugin
  register the provider and declare `admission_policy.v1`. Unset, empty, `none` or `off`: no
  provider, no admission capability, no models. The plugin seals records and serves the Evidence
  page.

## 0.1.1

### Added

- **Settlement-record legs for paid exchanges** (draft-mih-agent-settlement-records-00). For each
  invoice of a paid exchange, both sides now seal the draft's legs beside the existing payment
  lifecycle trail. The payer seals the terms leg (invoice amount, `ln.payment_hash`) and pushes it
  to the serving peer, then seals `payer_observed` (the amount, plus `routing_fee` when the host
  passes the wallet's fee) and a delivered leg with the response digest. The provider seals
  `payee_observed` (`received`, `receive_fee`), citing the payer's terms leg.
  - **Host requirement:** the provider's leg needs a mesh-llm host whose settlement events carry
    the wallet's credited amount and fee (Mesh-LLM/mesh-llm#2169). On a host without it, the
    provider seals no payee leg; the payer's legs and pushed terms work either way.
  - `routing_fee` and `receive_fee` are new on ledger records. Peers may read them back under the
    default share policy, like the amounts the trail already carried.

### Fixed

- **A node-unique default log id.** Every node's checkpoints used the same log id, so a witness
  accepted the first node and refused the rest as a fork. The default is now
  `capsule-emit-mesh/<key id>`, chosen at first start and kept in `<data dir>/log_id`;
  `CAPSULE_EMIT_MESH_LOG_ID` overrides it. A log that already holds checkpoints keeps its id, and
  starting a new log moves the node to its own id (see INSTALL.md, "The log's id").
- **Finding a served half by its digests.** When the host forwarded no nonce, the serving node's
  half couldn't be found by the requester's nonce, so the exchange showed as open. The responder
  now also indexes each record by its exchange's two effect digests, and the page asks by those
  after a signed "no such record". Both nodes need 0.1.1 for this.
- **The witness indicator.** The Evidence page could say "witness off" on a node whose checkpoints
  carried witness receipts. It now reads how many checkpoints a witness holds and whether a
  witness is configured, and says "off" only when no witness is set.

## 0.1.0

- First release.
