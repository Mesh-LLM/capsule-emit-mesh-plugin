# Install the capsule-emit-mesh plugin on a mesh-llm node

`capsule-emit-mesh` is a mesh-llm plugin. It keeps a signed, hash-chained
record of every exchange this node serves or asks for, on this node's disk,
and adds an **Evidence** page to the console: the peers you exchanged with,
each exchange, and whether this node's log still verifies. It installs and
runs on an unmodified mesh-llm release (0.77 or newer). No other runtime is
needed.

## What it sends, and where it listens

Records are kept on this node. With the default settings:

| What | To whom | Default | Turn it off |
| --- | --- | --- | --- |
| This node's own sealed record of a completed exchange | the peer in that exchange | on | `share_record_at_completion = "off"` |
| One of this node's records, when asked for it by id | the peer that record names as the other side | on | `share_history_segments = "off"` |
| A signed checkpoint of the log | a witness service | **off**: only if you set `witness` | leave `witness` empty |

All peer traffic uses mesh-llm's own peer connections. Nothing goes to any
third party unless you set a witness. The switches are under Configuration ›
Plugins › Sharing policy, and [`docs/SHARING-POLICY.md`](docs/SHARING-POLICY.md)
explains each one.

The plugin listens only on `127.0.0.1`, at a random port the node reaches it
through.

## Install

One path, the same on macOS 11 or newer (Apple Silicon) and on Linux with
glibc 2.35 or newer (x86_64 or arm64).

**1. Download the package for your machine and the checksum file** from the
[releases page](https://github.com/Mesh-LLM/capsule-emit-mesh-plugin/releases),
and set `VERSION` to the release (without the leading `v`):

```bash
VERSION=0.2.0
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)  TARGET=aarch64-apple-darwin ;;
  Linux-x86_64)  TARGET=x86_64-unknown-linux-gnu ;;
  Linux-aarch64) TARGET=aarch64-unknown-linux-gnu ;;
  *) echo "unsupported platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac
BASE=https://github.com/Mesh-LLM/capsule-emit-mesh-plugin/releases/download/v$VERSION
curl -fLO "$BASE/capsule-emit-mesh-$VERSION-$TARGET.tar.gz"
curl -fLO "$BASE/SHA256SUMS"
```

**2. Check the download.** This must print `OK` for your file:

```bash
shasum -a 256 -c SHA256SUMS --ignore-missing
```

With the GitHub CLI you can also check where it was built:

```bash
gh attestation verify "capsule-emit-mesh-$VERSION-$TARGET.tar.gz" \
  --repo Mesh-LLM/capsule-emit-mesh-plugin
```

The checksum proves your file is the one published. The attestation proves it
was built by this repository's release workflow from the tagged commit.
Neither says the code is safe to run; that is a judgement about the source,
which is public here.

**3. Install it.**

```bash
mesh-llm plugins install --archive "capsule-emit-mesh-$VERSION-$TARGET.tar.gz" \
  --name capsule-emit-mesh --version "$VERSION"
```

Pass `--name capsule-emit-mesh`: the plugin's id is `capsule-emit-mesh`, while
this repository is `capsule-emit-mesh-plugin` (see the README), so installing
by repository name does not find the release assets. The command unpacks the
package into the node's plugin directory (`~/.mesh-llm/plugins`, or
`MESH_LLM_PLUGIN_DIR`) and enables it. `mesh-llm plugins info
capsule-emit-mesh` shows what was installed.

**4. Restart the node.** An installed, enabled plugin starts with the node; no
config entry is needed.

**5. Open the console.** The plugin adds a page labelled **Evidence**. After
the node serves or asks for an exchange, the page shows the peer, the
exchange, and the log's integrity.

## What works today

| | |
| --- | --- |
| Install, start, the Evidence page | works |
| Each node seals its own record of each exchange | works |
| An exchange confirmed by the other side | **not yet** |

A confirmed exchange is the serving node's record and the requesting node's
record, matched and each held by the other side. The plugin already receives
and checks the other side's record. What is not there yet is **two fields from
mesh-llm**: `requested_by_node_id` on the exchange event the serving node sees
(which peer asked), and `served_by_node_id` with the request and response
digests on the event the requesting node sees for a request another peer
served. Without them neither side knows whom to match.

Until they exist, each node holds only its own record, and nothing is shown as
confirmed.

**Each node must know the other node's public key.** Nodes do not exchange
keys yet. The plugin accepts a record only from a peer whose key you have
configured, and refuses the rest. Set `CAPSULE_EMIT_MESH_PEER_KEYS` in the
node's environment to a JSON object mapping each peer's id to its raw Ed25519
public key in hex. A node writes its own id to `<data dir>/self-peer-id` and
its public key to `<data dir>/keys/node-key.pub.pem`.

## Where the records are kept

Under the plugin's data directory: `CAPSULE_EMIT_MESH_DATA_DIR` if set in the
node's environment (an absolute path), else `$XDG_DATA_HOME/capsule-emit-mesh`,
else `~/.local/share/capsule-emit-mesh`. The sealed log is
`<data dir>/ledger/capsules.jsonl` and its checkpoints are
`<data dir>/ledger/checkpoints.jsonl`. The node's signing key is under
`<data dir>/keys/`; it never leaves the node.

## Settings were renamed

Every setting in the node's environment is named `CAPSULE_EMIT_MESH_*`. Until
the next release the plugin still reads the old `ADMISSION_POLICY_*` name of a
setting whose new name is unset, and logs which one it used; when both are
set, the new name wins. Rename yours now: the old names stop working after
this release.

## Turn it off or remove it

```bash
mesh-llm plugins disable capsule-emit-mesh   # keeps it installed
mesh-llm plugins delete capsule-emit-mesh    # removes the installed files
```

Removing the plugin does not delete its data directory.

## What is in the package

```text
capsule-emit-mesh/
  capsule-emit-mesh                   the plugin executable
  plugin.toml                         package marker (name, version)
  plugin-manifest.json                settings schema and web UI declaration
  bundle/register-mesh-plugin-ui.js   the Evidence page
  README.md                           this file
  LICENSE
```

## Build it yourself

Only packages built by `.github/workflows/release.yml` are published. The
packaging step (`scripts/package.sh`) is deterministic, so the same executable
always gives the same archive digest. To build a package for your own machine
from a checkout of a release tag (the packaging step needs GNU tar):

```bash
cargo build --locked --release --manifest-path crates/evidence-plugin/Cargo.toml
(cd web-ui && pnpm install --frozen-lockfile && pnpm build)
scripts/package.sh "$VERSION" "$TARGET" crates/evidence-plugin/target/release/capsule-emit-mesh dist
```

A different compiler or build machine can give a different executable and so a
different digest; `SHA256SUMS` covers the published packages.
