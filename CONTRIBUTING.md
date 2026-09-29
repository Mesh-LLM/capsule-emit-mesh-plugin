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
- **The plugin's id stays `capsule-emit-mesh`.** Nodes key their data directory
  and installed plugin on it.

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

- `scripts/no-keys.sh`: no private key or token material in any tracked file.
- The vector checksums in `vectors/SHA256SUMS`.
- The release packaging, run twice on one build; the two archives must be
  byte-identical.
- The neutrality check (`.github/workflows/neutrality.yml`): the repository
  carries none of a reserved vocabulary, supplied as a repository secret. On a
  pull request from a fork, the log names the file and line of a hit but not
  the term; a maintainer can re-run it on `main` to see the term. It runs on
  `pull_request_target` so that fork pull requests get the secret; that is
  safe only because the workflow never runs, builds or installs anything
  from the pull request (it reads the files as text with the base branch's
  scanner). The rules are written at the top of the workflow file.

Changes to the sealing crates (`capsule-emit`, `checkpointed-local-log`,
`evidencebook`) are tested in their own repositories and reach this one as
version bumps.

### Tier 2: nightly, and on the `run-e2e` label

A two-node integration test on a pinned, unmodified mesh-llm release with a
small model: an exchange sealed on both sides, a forged record refused, and the
self-contained attack cases. Checks that need host support not yet in a
mesh-llm release are reported as blocked on that host change, not as passes.
Add the `run-e2e` label to a pull request to run it before merge. The workflow
is added in its own pull request.

## Test vectors

`vectors/` holds pinned copies of external test vectors. Don't edit them here:
regenerate at the source, copy unchanged, update the pin in
`vectors/SOURCES.md`, and regenerate `vectors/SHA256SUMS`.

## Reporting a security issue

See [SECURITY.md](SECURITY.md). Please don't open a public issue for a
vulnerability.
