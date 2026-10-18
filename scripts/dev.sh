#!/usr/bin/env bash
set -euo pipefail

repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo"
dev_root=$(realpath -m -- "${ANCHOR_DEV_ROOT:-$repo/.local/rust}")
state="$dev_root/dev"
mkdir -p "$state"
exec 9>"$state/lifecycle.lock"
flock 9

export ANCHOR_RUNNER_CATALOG_ROOT="${ANCHOR_RUNNER_CATALOG_ROOT:-$dev_root/catalog}"
export ANCHOR_RUNNER_STATE_ROOT="${ANCHOR_RUNNER_STATE_ROOT:-$dev_root/state}"
export ANCHOR_RUNNER_WORKSPACE_ROOT="${ANCHOR_RUNNER_WORKSPACE_ROOT:-$dev_root/workspaces}"
export ANCHOR_RUNNER_LIBRARY_ROOT="${ANCHOR_RUNNER_LIBRARY_ROOT:-$dev_root/library}"
# Platform development starts without a deployment-owned entry Graph. Ordinary
# Graphs belong to the editable catalog. An explicitly configured bundle/name
# retains the deployment-owned entry Graph contract.
if [[ -n "${ANCHOR_RUNNER_BUNDLE_ROOT:-}" ]]; then
  export ANCHOR_RUNNER_BUNDLE_ROOT
  export ANCHOR_RUNNER_GRAPH_NAME="${ANCHOR_RUNNER_GRAPH_NAME:-$(basename -- "$ANCHOR_RUNNER_BUNDLE_ROOT")}"
elif [[ -n "${ANCHOR_RUNNER_GRAPH_NAME:-}" ]]; then
  export ANCHOR_RUNNER_GRAPH_NAME
  export ANCHOR_RUNNER_BUNDLE_ROOT="$ANCHOR_RUNNER_CATALOG_ROOT/$ANCHOR_RUNNER_GRAPH_NAME"
else
  unset ANCHOR_RUNNER_GRAPH_NAME ANCHOR_RUNNER_BUNDLE_ROOT
fi
export ANCHOR_RUNNER_SCHEDULES_PATH="${ANCHOR_RUNNER_SCHEDULES_PATH:-$ANCHOR_RUNNER_STATE_ROOT/schedules.json}"
export ANCHOR_RUNNER_WEB_ROOT="${ANCHOR_RUNNER_WEB_ROOT:-$repo/apps/web/dist}"
export ANCHOR_RUNNER_LISTEN="${ANCHOR_RUNNER_LISTEN:-127.0.0.1:8077}"
export ANCHOR_RUNNER_ALLOWED_COMMANDS="${ANCHOR_RUNNER_ALLOWED_COMMANDS:-sh,git,cat,printf,true}"
api_address=${ANCHOR_RUNNER_LISTEN/0.0.0.0/127.0.0.1}
export ANCHOR_WEB_API_URL="${ANCHOR_WEB_API_URL:-http://$api_address}"
web_port=${ANCHOR_WEB_PORT:-5173}
web_url="http://127.0.0.1:$web_port"

api_pid="$state/native-host.pid"
web_pid="$state/vite.pid"

api_key=$(node <<'AUTH'
const configured = process.env.ANCHOR_API_KEYS || '';
try {
  const keys = configured.trim().startsWith('[') ? JSON.parse(configured) : configured.split(',').map(key => key.trim()).filter(Boolean);
  if (!Array.isArray(keys) || keys.some(key => typeof key !== 'string')) process.exit(1);
  process.stdout.write(keys[0] || '');
} catch { process.exit(1); }
AUTH
)

endpoint_open() {
  node - "$1" <<'PORT'
const net = require('node:net');
const url = new URL(process.argv[2]);
const socket = net.createConnection({ host: url.hostname.replace(/^\[|\]$/g, ''), port: url.port || 80 });
socket.setTimeout(1000);
socket.on('connect', () => { socket.destroy(); process.exit(0); });
socket.on('error', () => process.exit(1));
socket.on('timeout', () => { socket.destroy(); process.exit(1); });
PORT
}

api_response() {
  if [[ -n "$api_key" ]]; then
    printf 'Authorization: Bearer %s\n' "$api_key" |
      curl -fsS --max-time 1 --header @- "$ANCHOR_WEB_API_URL/$1" 2>/dev/null
  else
    curl -fsS --max-time 1 "$ANCHOR_WEB_API_URL/$1" 2>/dev/null
  fi
}

json_status() {
  node -e 'let body=""; process.stdin.on("data", chunk => body += chunk); process.stdin.on("end", () => {
    try { const data=JSON.parse(body); process.exit(data && !Array.isArray(data) && data.status === process.argv[1] ? 0 : 1); }
    catch { process.exit(1); }
  });' "$1"
}

api_ready() {
  api_response health | json_status ok && api_response ready | json_status ready
}

process_identity() {
  local pid=$1 process_stat
  local -a fields
  [[ "$pid" =~ ^[0-9]+$ && -r "/proc/$pid/stat" ]] || return 1
  process_stat=$(<"/proc/$pid/stat")
  read -r -a fields <<< "${process_stat##*) }"
  [[ "${fields[0]:-Z}" != Z && -n "${fields[19]:-}" ]] || return 1
  echo "${fields[19]}"
}

alive() {
  local pid identity current
  [[ -s "$1" ]] || return 1
  read -r pid identity < "$1"
  [[ -n "$identity" ]] || return 1
  current=$(process_identity "$pid") || return 1
  [[ "$current" == "$identity" ]] && kill -0 "$pid" 2>/dev/null
}

record_pid() {
  local pid=$1 pid_file=$2 identity
  identity=$(process_identity "$pid") || return 1
  printf '%s %s\n' "$pid" "$identity" > "$pid_file"
}

ready() {
  local kind=$1 pid_file=$2 log=$3
  for _ in {1..200}; do
    if [[ "$kind" == api ]]; then
      api_ready && return 0
    else
      curl -fsS --max-time 1 "$web_url/" >/dev/null 2>&1 && return 0
    fi
    alive "$pid_file" || break
    sleep 0.1
  done
  echo "Failed to start $kind; see $log" >&2
  return 1
}

start_api() {
  if alive "$api_pid"; then
    ready api "$api_pid" "$state/native-host.log"
    echo "Rust Host ready at $ANCHOR_WEB_API_URL (managed pid $(cut -d ' ' -f 1 "$api_pid"))"
  elif endpoint_open "$ANCHOR_WEB_API_URL"; then
    echo "External service at $ANCHOR_WEB_API_URL (not managed by dev.sh)"
    api_ready || { echo "External service does not satisfy Rust Host health/readiness" >&2; return 1; }
  else
    local binary target_dir
    target_dir=$(realpath -m -- "${CARGO_TARGET_DIR:-$repo/rust/target}")
    if [[ -n "${ANCHOR_RUNNER_BINARY:-}" ]]; then
      binary=$(realpath -m -- "$ANCHOR_RUNNER_BINARY")
      [[ -x "$binary" ]] || { echo "Rust Host binary is not executable: $binary" >&2; return 1; }
    elif [[ -x "$target_dir/release/anchor-runner-host" ]]; then
      binary="$target_dir/release/anchor-runner-host"
    else
      binary="$target_dir/debug/anchor-runner-host"
      if [[ ! -x "$binary" ]]; then
        CARGO_TARGET_DIR="$target_dir" cargo build --manifest-path "$repo/rust/Cargo.toml" -p anchor-runner-host --locked
      fi
    fi
    # Fixed build outputs supply runtime tools; startup never sources a configuration file.
    export ANCHOR_GOOSE_BINARY="${ANCHOR_GOOSE_BINARY:-$repo/.local/bin/goose}"
    export ANCHOR_GOOSE_RELAY_BINARY="${ANCHOR_GOOSE_RELAY_BINARY:-$(dirname -- "$binary")/anchor-net-relay}"
    if [[ ! -f "$ANCHOR_RUNNER_WEB_ROOT/index.html" && "$ANCHOR_RUNNER_WEB_ROOT" == "$repo/apps/web/dist" ]]; then
      npm --prefix "$repo/apps/web" run build
    fi
    mkdir -p "$ANCHOR_RUNNER_CATALOG_ROOT" "$ANCHOR_RUNNER_STATE_ROOT" \
      "$ANCHOR_RUNNER_WORKSPACE_ROOT" "$ANCHOR_RUNNER_LIBRARY_ROOT/plugins"
    nohup setsid "$binary" serve >"$state/native-host.log" 2>&1 < /dev/null 9>&- &
    record_pid "$!" "$api_pid"
    ready api "$api_pid" "$state/native-host.log"
    echo "Rust Host ready at $ANCHOR_WEB_API_URL (managed pid $(cut -d ' ' -f 1 "$api_pid"))"
  fi
}

start_web() {
  if alive "$web_pid"; then
    ready web "$web_pid" "$state/vite.log"
    echo "Web UI process already exists (pid $(cut -d ' ' -f 1 "$web_pid"))"
  elif endpoint_open "$web_url"; then
    echo "External service at $web_url (not managed by dev.sh)"
  else
    nohup setsid "$repo/apps/web/node_modules/.bin/vite" \
      "$repo/apps/web" --host 127.0.0.1 --port "$web_port" --strictPort \
      >"$state/vite.log" 2>&1 < /dev/null 9>&- &
    record_pid "$!" "$web_pid"
    ready web "$web_pid" "$state/vite.log"
  fi
}

stop_one() {
  local label=$1 pid_file=$2
  if alive "$pid_file"; then
    local pid identity group
    read -r pid identity < "$pid_file"
    group=$(ps -o pgid= -p "$pid" | tr -d ' ')
    if [[ "$group" == "$pid" ]]; then
      kill -- "-$pid" 2>/dev/null || true
    else
      kill "$pid" 2>/dev/null || true
    fi
    for _ in {1..20}; do
      alive "$pid_file" || break
      sleep 0.1
    done
    if alive "$pid_file"; then
      echo "$label is still stopping (pid $pid)"
      return
    fi
    echo "$label stopped"
  fi
  rm -f "$pid_file"
}

status() {
  if alive "$api_pid"; then
    api_ready && echo "Rust Host: ready ($ANCHOR_WEB_API_URL)" || echo "Rust Host: not ready ($ANCHOR_WEB_API_URL)"
  elif endpoint_open "$ANCHOR_WEB_API_URL"; then
    echo "Anchor API: external service ($ANCHOR_WEB_API_URL)"
  else
    echo "Anchor API: down"
  fi
  if alive "$web_pid"; then
    curl -fsS --max-time 1 "$web_url/" >/dev/null 2>&1 && echo "Web UI: up ($web_url)" || echo "Web UI: not ready ($web_url)"
  elif endpoint_open "$web_url"; then
    echo "Web UI: external service ($web_url)"
  else
    echo "Web UI: down"
  fi
}

case "${1:-start}" in
  start)
    start_api
    start_web
    echo "Logs: $state/"
    ;;
  stop)
    stop_one "Web UI" "$web_pid"
    stop_one "Anchor API" "$api_pid"
    ;;
  restart)
    stop_one "Web UI" "$web_pid"
    stop_one "Anchor API" "$api_pid"
    start_api
    start_web
    ;;
  status)
    status
    ;;
  *)
    echo "usage: $0 [start|stop|restart|status]" >&2
    exit 2
    ;;
esac
