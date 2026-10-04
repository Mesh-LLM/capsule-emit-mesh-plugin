# Contributing

Thanks for helping. This file covers how changes are made and what CI checks.

## Ground rules

- **Apache-2.0.** By contributing you agree your work is licensed under the
  repository's license.
- **DCO sign-off on every commit** (`git commit -s`). The `dco` check fails a
  pull request with any commit that lacks a `Signed-off-by:` line.
- **Pull requests only.** `main` is protected; every change needs a green CI
  run and one approving review.
- **No secrets in the tree.** Never commit private keys, tokens, node ledgers
  or data directories. CI fails on key or token material (`scripts/no-keys.sh`).
- **The plugin's id stays `capsules`.** Nodes key their data directory
  and installed plugin on it. (It was `capsule-emit-mesh` until 0.1.3; that
  rename keeps a node's existing data directory in place, see `INSTALL.md`.)

## Building and testing locally

```bash
# The plugin
cd crates/evidence-plugin
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked -- --test-threads=1

# The Evidence page
cd web-ui
pnpm install --frozen-lockfile
pnpm typecheck && pnpm test && pnpm build   # writes ../bundle/register-mesh-plugin-ui.js

# Repository checks
scripts/no-keys.sh
node --test scripts/*.test.mjs
(cd vectors && shasum -a 256 -c SHA256SUMS)
```

The interop tests run the compiled plugin against a stand-in host over
mesh-llm's plugin wire protocol, and share ports and directories, so run them
with `--test-threads=1`. Tests that need a real `mesh-llm` binary are ignored by
default; [docs/REAL-HOST-VERIFICATION.md](docs/REAL-HOST-VERIFICATION.md)
explains how to run them.

## CI

### Tier 1: every pull request (target: under 10 minutes)

On Linux x86_64, Linux arm64 and macOS arm64 (`.github/workflows/ci.yml`):

- `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test` for the
  plugin, including the conformance vectors in `vectors/`.
- The web UI: typecheck, unit tests, and the bundle build.

Once, on Linux:

- `scripts/no-keys.sh`: no private key or token material in any tracked file,
  and no file that never belongs here (Python, demo or red-team material, key
  or token files, node ledgers or data directories, a NOTICE file).
- The vector checksums in `vectors/SHA256SUMS`.
- The release packaging, run twice on one build; the two archives must be
  byte-identical.
- The neutrality check (`.github/workflows/neutrality.yml`): the repository
  carries none of a reserved vocabulary, supplied as a repository secret. On a
  pull request from a fork, a failing run says only that it failed, with no
  term, file or line; a maintainer re-runs it on a trusted event to see the
  details. The scanner's own tests (`node --test scripts/*.test.mjs`) run on every pull
  request in `ci.yml`. The neutrality check runs on
  `pull_request_target` so that fork pull requests get the secret; that is
  safe only because the workflow never runs, builds or installs anything
  from the pull request (it reads the files as text with the base branch's
  scanner). The rules are written at the top of the workflow file.

Changes to the sealing crates (`capsule-emit`, `checkpointed-local-log`,
`evidencebook`) are tested in their own repositories and reach this one as
version bumps.

### Tier 2: nightly, and on the `run-e2e` label

`.github/workflows/e2e.yml` runs two nodes of a pinned, unmodified mesh-llm
release on one Linux runner, each with this plugin: one serves a small model,
the other joins its mesh and sends it three chat completions.
`scripts/e2e-two-node.sh` runs the nodes; once they have stopped, the checks
in `crates/evidence-plugin/src/two_node_e2e.rs` run over what each node wrote:

- each side sealed its own record of every exchange, and both logs verify;
- a forged record is refused (changed after signing, signed by another key,
  or from a sender with no announced key);
- attack A, a tampered record, and attack C, a dropped one, break the
  provider's log;
- attack B, a record naming another server, and attack D, a record naming
  other model weights, are refused, and the run repeats both against a
  receiver built without its claim checks, which must fail them.

Confirming an exchange (each side holding the other's record, the row
CLOSED) needs the exchange event to name the other side, which no mesh-llm
release does yet. That check runs as "expected-blocked until the host names the other side": it
asserts the honest one-sided state, and fails once the pinned release names
the other side, so it can become the confirmed / CLOSED check.

The mesh-llm release and the model are pinned by SHA-256 in the workflow and
cached. Add the `run-e2e` label to a pull request to run it before merge.

## Test vectors

`vectors/` holds pinned copies of external test vectors. Don't edit them here:
regenerate at the source, copy unchanged, update the pin in
`vectors/SOURCES.md`, and regenerate `vectors/SHA256SUMS`.

## Reporting a security issue

See [SECURITY.md](SECURITY.md). Please don't open a public issue for a
vulnerability.
