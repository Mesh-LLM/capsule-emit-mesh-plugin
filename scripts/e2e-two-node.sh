#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Tier 2: two unmodified mesh-llm nodes on this machine, each running the
# plugin. The provider serves the model; the requester serves nothing, joins
# the provider's mesh, and sends it chat completions over the mesh. Once both
# nodes have stopped, the scenario checks (crates/evidence-plugin/src/
# two_node_e2e.rs) run over what each node wrote.
#
# Usage: scripts/e2e-two-node.sh MESH_LLM_BIN PLUGIN_BIN MODEL_GGUF OUT_DIR
#
# Run from the repository root, on a machine that may load the model. The node
# logs go to OUT_DIR/logs with the mesh's invite token removed. The nodes'
# homes (their keys and the plugin's data) stay under OUT_DIR/nodes and are
# never printed.
set -euo pipefail

if [ "$#" -ne 4 ]; then
  echo "usage: $0 MESH_LLM_BIN PLUGIN_BIN MODEL_GGUF OUT_DIR" >&2
  exit 2
fi
mesh_llm=$(realpath "$1")
plugin=$(realpath "$2")
model=$(realpath "$3")
out=$4
repo=$(pwd)

EXCHANGES=${E2E_EXCHANGES:-3}
MODEL_NAME=e2e-model
PROVIDER_API=9401 PROVIDER_CONSOLE=9411
REQUESTER_API=9402 REQUESTER_CONSOLE=9412

mkdir -p "$out/logs" "$out/nodes"
out=$(realpath "$out")
logs=$out/logs
pids=()
token=

node_home() { echo "$out/nodes/$1"; }
plugin_data() { echo "$(node_home "$1")/.local/share/capsules"; }

# The token lets anyone join the mesh while it runs; it never reaches a log
# anyone reads. Matched by value, wherever it appears.
redact_logs() {
  [ -n "$token" ] || return 0
  TOKEN=$token perl -pi -e 's/\Q$ENV{TOKEN}\E/<invite-token>/g' "$logs"/*.log
}

stop_nodes() {
  for pid in "${pids[@]}"; do kill -TERM "$pid" 2>/dev/null || true; done
  # The plugin writes a checkpoint on shutdown: give it time.
  for _ in $(seq 60); do
    alive=0
    for pid in "${pids[@]}"; do kill -0 "$pid" 2>/dev/null && alive=1; done
    [ "$alive" = 0 ] && break
    sleep 0.5
  done
  for pid in "${pids[@]}"; do kill -KILL "$pid" 2>/dev/null || true; done
  pids=()
}

on_exit() {
  status=$?
  stop_nodes
  redact_logs
  if [ "$status" -ne 0 ]; then
    for log in "$logs"/*.log; do
      echo "::group::$(basename "$log") (last 80 lines)"
      tail -n 80 "$log"
      echo "::endgroup::"
    done
  fi
  exit "$status"
}
trap on_exit EXIT

# One node: its own HOME (mesh-llm keeps its key, config and sockets under
# it), and the plugin's data under the same HOME.
start_node() {
  local name=$1 api=$2 console=$3
  shift 3
  local home
  home=$(node_home "$name")
  mkdir -p "$home"
  cat > "$home/config.toml" <<EOF
version = 1

[[plugin]]
name = "capsules"
enabled = true
command = "$plugin"
args = []
EOF
  HOME=$home XDG_DATA_HOME=$home/.local/share \
    "$mesh_llm" serve --config "$home/config.toml" \
    --port "$api" --console "$console" --headless \
    --disable-iroh-relays --log-format json "$@" \
    > "$logs/$name.log" 2>&1 &
  pids+=("$!")
}

# wait_for SECONDS DESCRIPTION COMMAND...: retry COMMAND until it succeeds.
wait_for() {
  local seconds=$1 what=$2
  shift 2
  local deadline=$((SECONDS + seconds))
  until "$@" > /dev/null 2>&1; do
    for pid in "${pids[@]}"; do
      kill -0 "$pid" 2>/dev/null || { echo "a node exited while waiting for: $what" >&2; return 1; }
    done
    if [ "$SECONDS" -ge "$deadline" ]; then
      echo "timed out after ${seconds}s waiting for: $what" >&2
      return 1
    fi
    sleep 2
  done
  echo "ready: $what"
}

status_field() { curl -fsS "http://127.0.0.1:$1/api/status" | jq -er "$2"; }
lists_model() { curl -fsS "http://127.0.0.1:$1/v1/models" | jq -e --arg m "$MODEL_NAME" 'any(.data[]; .id == $m)'; }
halves() {
  local file
  file=$(plugin_data "$1")/ledger/capsules.jsonl
  [ -f "$file" ] || { echo 0; return; }
  jq -c --arg r "$2" 'select(.model_attestation.compute_attestation["x-mesh-poc-v1"].role? == $r)' "$file" | wc -l
}
has_halves() { [ "$(halves "$1" "$2")" -ge "$EXCHANGES" ]; }

# --- the provider: serves the model -----------------------------------------
start_node provider "$PROVIDER_API" "$PROVIDER_CONSOLE" --gguf "$model" --model "$MODEL_NAME"
wait_for 600 "the provider has loaded the model" status_field "$PROVIDER_CONSOLE" '.llama_ready == true'
token=$(status_field "$PROVIDER_CONSOLE" '.token')
provider_id=$(curl -fsS "http://127.0.0.1:$PROVIDER_CONSOLE/api/diagnostics/network" | jq -er '.node_id')
echo "provider node id: $provider_id"

# --- the requester: serves nothing, joins the provider's mesh ---------------
start_node requester "$REQUESTER_API" "$REQUESTER_CONSOLE" --client --join "$token"
wait_for 300 "the requester can route to the provider's model" lists_model "$REQUESTER_API"

# --- the exchanges: each asked of the provider by id ------------------------
for n in $(seq "$EXCHANGES"); do
  body=$(jq -cn --arg m "$MODEL_NAME" --arg p "Reply with the number $n and nothing else." \
    '{model: $m, messages: [{role: "user", content: $p}], max_tokens: 8, temperature: 0}')
  code=$(curl -sS -o "$out/response-$n.json" -D "$out/headers-$n.txt" -w '%{http_code}' \
    --max-time 120 -H 'content-type: application/json' -H "x-mesh-target: $provider_id" \
    -d "$body" "http://127.0.0.1:$REQUESTER_API/v1/chat/completions")
  served_by=$(tr -d '\r' < "$out/headers-$n.txt" | awk -F': ' 'tolower($1) == "x-mesh-served-by" {print $2}')
  echo "exchange $n: HTTP $code, served by ${served_by:-<not said>}"
  [ "$code" = 200 ] || { echo "exchange $n failed" >&2; exit 1; }
  [ "$served_by" = "$provider_id" ] || { echo "exchange $n was not served by the provider" >&2; exit 1; }
done

# --- each side seals its own half -------------------------------------------
wait_for 120 "the requester sealed $EXCHANGES requested halves" has_halves requester requested
wait_for 120 "the provider sealed $EXCHANGES served halves" has_halves provider served

stop_nodes

# --- the scenario checks, over copies of what each node wrote ---------------
mkdir -p "$out/data"
cp -a "$(plugin_data requester)" "$out/data/requester"
cp -a "$(plugin_data provider)" "$out/data/provider"
cd "$repo/crates/evidence-plugin"
E2E_REQUESTER_DIR=$out/data/requester E2E_PROVIDER_DIR=$out/data/provider E2E_EXCHANGES=$EXCHANGES \
  cargo test --locked --bin capsules -- --ignored --test-threads=1 two_node_e2e::
