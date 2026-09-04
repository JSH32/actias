#!/usr/bin/env bash
# The fair-share drill, on the compose stack: two projects on one worker,
# one saturating it, and the numbers that say whether the small one
# still gets its share.
#
# Each project publishes the load drill's script (one gated write per
# call). The big project drives BIG_WRITERS concurrent writers, the
# small one SMALL_WRITERS, both for REQUESTS calls per writer, at the
# same time. What is measured is each project's ack percentiles and
# its 429 count, plus the worker's share gauges. The claim under test:
# the small project's p99 stays near its solo number while the big one
# is refused at its share, and the big project alone afterwards holds
# the whole node.
#
# Set REQUEST_CONCURRENCY low on the worker to see the bound bite at
# these sizes (the compose default, 1024, is far above two drills):
#   REQUEST_CONCURRENCY=8 just up
#   BIG_WRITERS=16 SMALL_WRITERS=2 ./scripts/fair-share-drill.sh
set -euo pipefail

cd "$(dirname "$0")/.."

API="${API:-http://127.0.0.1:3001/api}"
WORKER="${WORKER:-http://127.0.0.1:3002}"
BIG_WRITERS="${BIG_WRITERS:-16}"
SMALL_WRITERS="${SMALL_WRITERS:-2}"
REQUESTS="${REQUESTS:-100}"
OBJECTS="${OBJECTS:-20}"
OUT="${OUT:-$(pwd)/scratch/fair-share-drill-$(date +%Y%m%d-%H%M%S)}"

mkdir -p "$OUT"
echo "== fair-share drill: big $BIG_WRITERS writers, small $SMALL_WRITERS writers, $REQUESTS each"
echo "== output: $OUT"

shares() {
    curl -sf "$WORKER/_metrics" 2>/dev/null | grep -E '^actias_share_' || true
}

# A project of its own with the load drill's script published under it;
# prints the script's identifier.
setup() {
    local tag=$1
    SUFFIX=$RANDOM
    curl -sf -X POST "$API/users" -H 'Content-Type: application/json' \
        -d "{\"username\":\"share$tag$SUFFIX\",\"email\":\"share$tag$SUFFIX@example.com\",\"password\":\"fair-share-password\"}" >/dev/null
    TOKEN=$(curl -sf -X POST "$API/auth/login" -H 'Content-Type: application/json' \
        -d "{\"auth\":\"share$tag$SUFFIX\",\"password\":\"fair-share-password\"}" | jq -r .token)
    AUTH="Authorization: Bearer $TOKEN"
    PROJECT_ID=$(curl -sf -X POST "$API/project" -H "$AUTH" -H 'Content-Type: application/json' \
        -d "{\"name\":\"share-$tag\"}" | jq -r .id)
    IDENT="share$tag$SUFFIX"
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
    ./target/debug/actias publish "$DIR/project" >/dev/null
    rm -rf "$DIR"
    echo "$IDENT"
}

# One project's writers; each line is a total time or the status that
# refused it.
drive() {
    local ident=$1 writers=$2 file=$3
    seq 0 $((writers - 1)) | xargs -P "$writers" -I{} bash -c '
        writer=$1
        for r in $(seq 1 '"$REQUESTS"'); do
            obj=$(( (writer + r) % '"$OBJECTS"' ))
            curl -s -o /dev/null -w "%{http_code} %{time_total}\n" "'"$WORKER"'/'"$ident"'/$obj" \
                || echo "000 0"
        done
    ' _ {} > "$file"
}

report() {
    python3 - "$1" "$2" <<'PY'
import sys, pathlib
label, path = sys.argv[1], pathlib.Path(sys.argv[2])
rows = [line.split() for line in path.read_text().splitlines() if line.strip()]
ok = sorted(float(t) * 1000 for code, t in rows if code == "200")
refused = sum(1 for code, _ in rows if code == "429")
failed = sum(1 for code, _ in rows if code not in ("200", "429"))
def pct(p):
    return ok[min(int(len(ok) * p / 100), len(ok) - 1)] if ok else float("nan")
print(f"{label:<12} ok {len(ok):>5}  refused {refused:>5}  failed {failed:>3}  "
      f"p50 {pct(50):6.0f}  p90 {pct(90):6.0f}  p99 {pct(99):6.0f} ms")
PY
}

cargo build -p actias-cli --quiet
BIG=$(setup big)
SMALL=$(setup small)
echo "== published big=$BIG small=$SMALL; warming"
for i in $(seq 0 $((OBJECTS - 1))); do
    curl -sf -o /dev/null "$WORKER/$BIG/$i" || true
    curl -sf -o /dev/null "$WORKER/$SMALL/$i" || true
done

echo "== small alone"
drive "$SMALL" "$SMALL_WRITERS" "$OUT/small-alone.txt"

echo "== both at once"
shares > "$OUT/shares-before.txt"
drive "$BIG" "$BIG_WRITERS" "$OUT/big-contended.txt" &
big_pid=$!
drive "$SMALL" "$SMALL_WRITERS" "$OUT/small-contended.txt"
wait "$big_pid"
shares > "$OUT/shares-contended.txt"

echo "== big alone"
drive "$BIG" "$BIG_WRITERS" "$OUT/big-alone.txt"
shares > "$OUT/shares-after.txt"

{
    report "small alone" "$OUT/small-alone.txt"
    report "small+big" "$OUT/small-contended.txt"
    report "big+small" "$OUT/big-contended.txt"
    report "big alone" "$OUT/big-alone.txt"
    echo
    echo "shares while contended:"
    grep -E 'refused_total|active_scopes|current' "$OUT/shares-contended.txt" | grep requests
} | tee "$OUT/report.txt"
