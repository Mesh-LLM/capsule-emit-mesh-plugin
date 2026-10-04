# Changelog

## 0.1.3 (unreleased)

### Changed

- **Renamed to `capsules`** (plugin id, repository `Mesh-LLM/capsules`, release assets
  `capsules-<version>-<target>.tar.gz`, page `/plugins/capsules/evidence`). Settings are
  `CAPSULES_*`; the old `CAPSULE_EMIT_MESH_*` names are read for this release, with a warning. The
  `ADMISSION_POLICY_*` names are no longer read.
- **The data directory moves once** from `capsule-emit-mesh` to `capsules`, as a single rename,
  with the log verified before and after. A node with both directories refuses to start and says
  why. Existing logs keep their log id.

### Added

- **A list of witnesses.** The console's `witness` setting is a list of URLs (the environment's
  `CAPSULES_CHECKPOINT_WITNESS_URLS` stays a comma list). There is still no default witness, and no
  witness is contacted until one is named. The Evidence page lists each witness by name with what
  it holds.
- **Receipts are checked before they count.** A witness's receipt counts only when it verifies
  offline: the entry recomputed from the checkpoint's signed fields, and the receipt's inclusion
  proof and signature under the witness's key (fetched once from that witness and kept). An
  unchecked or failing receipt is shown, with the reason, and never counted.

### Fixed

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
