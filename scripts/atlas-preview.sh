#!/usr/bin/env bash
# Fast local preview of the hiking dashboard against a real-data copy.
#
# Boots the Rust API server (:8080) and the vite dev server (:5173) with the
# correct env, then self-checks that the hiking data actually loads before
# handing you the URL. Frontend edits hot-reload instantly — no deploy needed
# to see design changes.
#
#   scripts/atlas-preview.sh          # (re)start the stack
#   scripts/atlas-preview.sh stop     # stop both servers
#   scripts/atlas-preview.sh status   # show what's running
#
# Env overrides: ATLAS_DATA_DIR (default /tmp/atlas-real), ATLAS_PW (default hiking)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA_DIR="${ATLAS_DATA_DIR:-/tmp/atlas-real}"
PW="${ATLAS_PW:-hiking}"
CARGO="$HOME/.cargo/bin/cargo"
API=http://localhost:8080
UI=http://localhost:5173

die() { echo "✗ $*" >&2; exit 1; }

free_port() { lsof -ti "tcp:$1" 2>/dev/null | xargs -r kill 2>/dev/null || true; }

stop_stack() {
  free_port 8080
  free_port 5173
}

case "${1:-start}" in
  stop)
    stop_stack; echo "✓ stopped"; exit 0 ;;
  status)
    printf 'backend :8080  '; lsof -ti tcp:8080 >/dev/null 2>&1 && echo "up" || echo "down"
    printf 'vite    :5173  '; lsof -ti tcp:5173 >/dev/null 2>&1 && echo "up" || echo "down"
    exit 0 ;;
  start) ;;
  *) die "usage: atlas-preview.sh [start|stop|status]" ;;
esac

[ -f "$DATA_DIR/garmin.db" ]           || die "no garmin.db in $DATA_DIR (copy prod data there first)"
[ -f "$DATA_DIR/fit-dashboard.duckdb" ] || die "no fit-dashboard.duckdb in $DATA_DIR"

echo "→ restarting stack against $DATA_DIR"
stop_stack
sleep 1

# --- backend --------------------------------------------------------------
FIT_DASHBOARD_GARMIN_DB="$DATA_DIR/garmin.db" \
FIT_DASHBOARD_DATA_DIR="$DATA_DIR" \
  "$CARGO" run --features web --manifest-path "$ROOT/src-tauri/Cargo.toml" \
  >"$DATA_DIR/backend.log" 2>&1 &
backend_pid=$!

printf '→ backend compiling/starting'
for _ in $(seq 1 240); do
  kill -0 "$backend_pid" 2>/dev/null || { echo; tail -20 "$DATA_DIR/backend.log"; die "backend exited (see $DATA_DIR/backend.log)"; }
  curl -sf "$API/api/status" >/dev/null 2>&1 && break
  printf '.'; sleep 1
done
echo " up"

# --- self-check: the exact failure mode that showed an empty map ----------
token=$(curl -s -X POST "$API/api/unlock" -H 'Content-Type: application/json' \
  -d "{\"password\":\"$PW\"}" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("token",""))' 2>/dev/null || true)
[ -n "$token" ] || die "unlock failed for password '$PW' — set ATLAS_PW or reset it (examples/reset_password.rs)"

tracks=$(curl -s -H "X-Session: $token" "$API/api/hiking/tracks" \
  | python3 -c 'import sys,json;print(len(json.load(sys.stdin)))' 2>/dev/null || echo 0)
[ "$tracks" -gt 0 ] || die "tracks endpoint returned $tracks — backend has no garmin data (wrong env?)"
echo "✓ data OK: $tracks hiking tracks"

# --- frontend -------------------------------------------------------------
( cd "$ROOT" && npm run dev >"$DATA_DIR/vite.log" 2>&1 & )
for _ in $(seq 1 30); do curl -sf "$UI" >/dev/null 2>&1 && break; sleep 1; done

cat <<EOF

  ✓ preview ready
    open      $UI
    password  $PW      (Hiking tab → The Atlas)
    logs      $DATA_DIR/backend.log · $DATA_DIR/vite.log
    stop      scripts/atlas-preview.sh stop
EOF
