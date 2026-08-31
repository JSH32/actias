#!/usr/bin/env bash
# Object write load against a running stack, and the numbers that decide
# how shipping should be bounded: ack latency percentiles, plus the
# worker's own shipping gauges before and after.
#
# What it drives is deliberately the gated path: every request calls an
# object method that writes, so every response waited for its frames to
# reach the store. OBJECTS spread the load across shippers (each object
# ships independently); WRITERS is the concurrency against them.
#
# Run against the compose stack (just up), or point it at anything:
#   OBJECTS=50 WRITERS=20 REQUESTS=500 ./scripts/load-drill.sh
set -euo pipefail

cd "$(dirname "$0")/.."

API="${API:-http://127.0.0.1:3001/api}"
WORKER="${WORKER:-http://127.0.0.1:3002}"
OBJECTS="${OBJECTS:-20}"
WRITERS="${WRITERS:-10}"
REQUESTS="${REQUESTS:-200}"
OUT="${OUT:-$(pwd)/scratch/load-drill-$(date +%Y%m%d-%H%M%S)}"

mkdir -p "$OUT"
echo "== load drill: $OBJECTS objects, $WRITERS writers, $REQUESTS requests each"
echo "== output: $OUT"

metrics() {
    curl -sf "$WORKER/_metrics" 2>/dev/null | grep -E \
        "^actias_(ships_in_flight|objects_dirty|ships_total|ship_failures_total|ship_duration_ms_total|ack_gate_waits_total|ack_gate_wait_ms_total|ack_gate_expired_total|objects_resident) " \
        || true
}

# A published script whose method writes on every call, so the response
# is an acknowledged durable write and nothing else.
setup() {
    SUFFIX=$RANDOM
    curl -sf -X POST "$API/users" -H 'Content-Type: application/json' \
        -d "{\"username\":\"load$SUFFIX\",\"email\":\"load$SUFFIX@example.com\",\"password\":\"load-drill-password\"}" >/dev/null
    TOKEN=$(curl -sf -X POST "$API/auth/login" -H 'Content-Type: application/json' \
        -d "{\"auth\":\"load$SUFFIX\",\"password\":\"load-drill-password\"}" | jq -r .token)
    AUTH="Authorization: Bearer $TOKEN"
    PROJECT_ID=$(curl -sf -X POST "$API/project" -H "$AUTH" -H 'Content-Type: application/json' \
        -d '{"name":"load-drill"}' | jq -r .id)
    IDENT="load$SUFFIX"
    SCRIPT_ID=$(curl -sf -X POST "$API/project/$PROJECT_ID/scripts" -H "$AUTH" \
        -H 'Content-Type: application/json' \
        -d "{\"publicIdentifier\":\"$IDENT\"}" | jq -r .id)

    DIR=$(mktemp -d)
    export XDG_CONFIG_HOME="$DIR/config"
    mkdir -p "$XDG_CONFIG_HOME/actias" "$DIR/project"
    printf '{"apiUrl":"%s","token":"%s"}' "${API%/api}" "$TOKEN" \
        > "$XDG_CONFIG_HOME/actias/settings.json"
    cat > "$DIR/project/script.json" <<EOF
{"id":"$SCRIPT_ID","entryPoint":"main.lua","includes":["**/*.lua"],"ignore":[]}
EOF
    cat > "$DIR/project/main.lua" <<'LUA'
-- One write per call, nothing else: the response time IS the gated
-- write path, so the percentiles below are the durability cost.
local Ledger = object "Ledger" {
    append = function(state)
        local n = (state.store:get("n") or 0) + 1
        state.store:set("n", n)
        return n
    end,
}

on "fetch" (function(request)
    local which = request.uri:match("/(%d+)$") or "0"
    return { body = json.stringify({ n = Ledger:get("led-" .. which):append() }) }
end)
LUA
    cargo build -p actias-cli --quiet
    ./target/debug/actias publish "$DIR/project" >/dev/null
    rm -rf "$DIR"
    echo "$IDENT"
}

IDENT=$(setup)
echo "== published $IDENT; warming the objects"
for i in $(seq 0 $((OBJECTS - 1))); do
    curl -sf -o /dev/null "$WORKER/$IDENT/$i" || true
done

metrics > "$OUT/metrics-before.txt"

echo "== driving"
started=$(date +%s.%N)
# Each writer walks its own slice of the object space; curl reports the
# total time per request, which for this script is the gated write.
seq 0 $((WRITERS - 1)) | xargs -P "$WRITERS" -I{} bash -c '
    writer=$1
    for r in $(seq 1 '"$REQUESTS"'); do
        obj=$(( (writer + r) % '"$OBJECTS"' ))
        curl -sf -o /dev/null -w "%{time_total}\n" "'"$WORKER"'/'"$IDENT"'/$obj" \
            || echo "ERR"
    done
' _ {} > "$OUT/times.txt"
finished=$(date +%s.%N)

metrics > "$OUT/metrics-after.txt"

python3 - "$OUT" "$started" "$finished" <<'PY'
import sys, pathlib
out, started, finished = pathlib.Path(sys.argv[1]), float(sys.argv[2]), float(sys.argv[3])
raw = (out / "times.txt").read_text().split()
errors = sum(1 for line in raw if line == "ERR")
times = sorted(float(line) * 1000 for line in raw if line != "ERR")

def pct(p):
    return times[min(int(len(times) * p / 100), len(times) - 1)] if times else float("nan")

def series(path):
    return {
        k: float(v)
        for k, v in (line.split() for line in path.read_text().splitlines() if " " in line)
    }

before, after = series(out / "metrics-before.txt"), series(out / "metrics-after.txt")
def delta(name):
    return after.get(name, 0) - before.get(name, 0)

ships = delta("actias_ships_total")
gates = delta("actias_ack_gate_waits_total")
elapsed = finished - started

report = f"""requests   {len(times)} ok, {errors} failed, {elapsed:.1f}s, {len(times)/elapsed:.0f}/s
ack ms     p50 {pct(50):.0f}  p90 {pct(90):.0f}  p99 {pct(99):.0f}  max {times[-1] if times else 0:.0f}
gate ms    mean {delta('actias_ack_gate_wait_ms_total')/gates:.0f} over {gates:.0f} waits
           expired {delta('actias_ack_gate_expired_total'):.0f} (writes told the outcome is unknown)
ships      {ships:.0f} flights, {delta('actias_ship_failures_total'):.0f} failed
           mean {delta('actias_ship_duration_ms_total')/ships:.0f} ms each
           {len(times)/ships:.1f} acked writes per flight (coalescing)
at rest    in flight {after.get('actias_ships_in_flight', 0):.0f}, dirty {after.get('actias_objects_dirty', 0):.0f}, resident {after.get('actias_objects_resident', 0):.0f}
"""
print(report)
(out / "report.txt").write_text(report)
PY

echo "== full output in $OUT"
