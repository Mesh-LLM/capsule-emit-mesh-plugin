# capsule-emit-mesh-plugin

A [mesh-llm](https://github.com/Mesh-LLM/mesh-llm) plugin that keeps a signed,
hash-chained record of every exchange a node serves or asks for, on that
node's own disk, and adds an **Evidence** page to the mesh-llm console. The
page shows the peers this node exchanged with, each exchange, and whether the
node's log still verifies.

Records follow the Agent Action Capsule format: JCS-canonical JSON, Ed25519
COSE_Sign1 signatures, and a checkpointed Merkle log. The plugin is Rust; the
page is a TypeScript bundle the plugin serves through mesh-llm's plugin web UI.

## The plugin's id vs. this repository's name

The plugin's id is **`capsule-emit-mesh`**: the name it installs under, its
data directory, and its page route (`/plugins/capsule-emit-mesh/evidence`).
Nodes already running it keep their records across upgrades because the id
does not change. This repository is `capsule-emit-mesh-plugin`.

The two differ, and that matters for one install form. `mesh-llm plugins
install <owner>/<repo>` takes the plugin name from the repository name, so it
would look for `capsule-emit-mesh-plugin-…` release assets, which don't exist.
Install from the archive with `--name capsule-emit-mesh`, as
[`INSTALL.md`](INSTALL.md) shows, or from a plugin catalog entry named
`capsule-emit-mesh`.

## Install

[`INSTALL.md`](INSTALL.md): download a release package for your platform,
check it, and install it with `mesh-llm plugins install --archive`. Release
packages are built by `.github/workflows/release.yml` from a tag in this
repository, for macOS arm64 and Linux x86_64/arm64.

## What works today

- The plugin installs on an unmodified mesh-llm, starts with the node, and adds
  the Evidence page.
- Each node seals its own record of every exchange, and the log's checkpoints
  and integrity checks work.
- When mesh-llm identifies the other side of an exchange, the node sends it
  its own record at completion.
- A record pushed by the other side is received and checked in-process: the
  sender's announced key, the signature, the claims, and a bundle's proof and
  checkpoint. It is held only if every check passes; anything else gets a
  signed refusal. Record requests are answered in-process too, under the
  sharing policy (see [docs/SHARING-POLICY.md](docs/SHARING-POLICY.md)).
- Twins that answered the same request differently get an independent check:
  a referee, chosen by yes/no eligibility rules, re-answers and signs a
  verdict ([docs/TWIN-REFEREE-SELECTION.md](docs/TWIN-REFEREE-SELECTION.md)).
  Every pair with no verdict reads "Not adjudicated" with the reason, and is
  never counted against either twin. This needs mesh-llm to mark twins (a
  twin bracket id); until it does, the page reads "Not adjudicated: this host
  does not mark twins" and nothing is called.

**Not yet:** an exchange confirmed by both sides. That needs mesh-llm to tell
the plugin which peer asked and which peer served (see
[Host changes](#host-changes)). Until then each node holds only its own
record.

## Host changes

Changes to mesh-llm this plugin depends on or would use. Each is a generic
host feature; none names this plugin.

| Change | What it enables | Status |
| --- | --- | --- |
| Provider-side `payment.lifecycle.v1` events | the serving node's record of a paid exchange | mesh-llm#2108, merged; not in a release yet |
| The exchange event on the paid serving path | paid exchanges are visible to plugins | mesh-llm#2109, merged; not in a release yet |
| `requested_by_node_id` on the served side; `served_by_node_id` with request/response digests on the routed side | matching the two sides' records | proposed |
| An operator's "stop routing to this peer" as a core router feature a plugin can request, plus a routing-choice event | acting on evidence without the plugin touching the router | proposed |
| A plugin page can ask to be a primary console tab; the operator decides | a stable place for the Evidence page | mesh-llm#2130, merged; not in a release yet |
| A built-in or default plugin can serve its web UI bundle | shipping this plugin by default | proposed |
| Per-request `skippy.stage.v1` events from split-inference stages | records for multi-stage requests | proposed |

## Repository layout

| Path | What |
| --- | --- |
| `crates/evidence-plugin/` | the plugin (Rust, `mesh-llm-plugin`) |
| `web-ui/` | the Evidence page, built into one ES module (`bundle/register-mesh-plugin-ui.js`) |
| `vectors/` | pinned test vectors; `vectors/SOURCES.md` says where each set comes from |
| `docs/` | sharing policy, verification chain, split-stage records, protocol notes, real-host tests |
| `plugin.toml`, `plugin.package.json` | the package marker and the reviewed plugin manifest |
| `scripts/` | packaging and repository checks |

The records are sealed by the [`capsule-emit`](https://crates.io/crates/capsule-emit)
crate on top of [`checkpointed-local-log`](https://crates.io/crates/checkpointed-local-log)
and [`evidencebook`](https://crates.io/crates/evidencebook); those crates are
developed and tested in their own repositories.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Apache-2.0; every commit needs a DCO
sign-off (`git commit -s`).

## License

Apache-2.0. See [LICENSE](LICENSE).
