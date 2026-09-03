#!/usr/bin/env bash
# The large-object drill for chunked bases, on the compose stack.
#
# One object is grown to SIZE_MB by inserting 64 KB rows, then a write
# workload touches rows from a hot set for TOUCHES calls. The drill
# reports what the store took per rotation against the file's size
# (the amplification chunking exists to remove), the touch latency
# across the rotations (the stall), what the replica received, and then
# kills the owner and checks the peer takes the object over with every
# row present.
#
#   SIZE_MB=256 TOUCHES=1200 HOT_PCT=5 ./scripts/big-object-drill.sh
#
# Pass: chunk bytes put during the touches are under a quarter of the
# file per rotation, at least one rotation happened, the takeover
# answers with the grown row count, and no touch failed.
set -euo pipefail

cd "$(dirname "$0")/.."

API="${API:-http://127.0.0.1:3001/api}"
OWNER="${OWNER:-http://127.0.0.1:3002}"
PEER="${PEER:-http://127.0.0.1:3003}"
OWNER_SERVICE="${OWNER_SERVICE:-worker_service}"
SIZE_MB="${SIZE_MB:-256}"
TOUCHES="${TOUCHES:-1200}"
HOT_PCT="${HOT_PCT:-5}"
TAKEOVER_WAIT="${TAKEOVER_WAIT:-90}"
# Rows per grow call: 64 rows of 64 KB is 4 MB, one segment's worth.
ROWS_PER_GROW=64
ROW_KB=64

metric() {
    curl -sf "$1/_metrics" 2>/dev/null | awk -v name="$2" '$1 == name { print $2 }'
}

publish() {
    SUFFIX=$RANDOM
    curl -sf -X POST "$API/users" -H 'Content-Type: application/json' \
        -d "{\"username\":\"big$SUFFIX\",\"email\":\"big$SUFFIX@example.com\",\"password\":\"big-object-drill-password\"}" >/dev/null
    TOKEN=$(curl -sf -X POST "$API/auth/login" -H 'Content-Type: application/json' \
        -d "{\"auth\":\"big$SUFFIX\",\"password\":\"big-object-drill-password\"}" | jq -r .token)
    AUTH="Authorization: Bearer $TOKEN"
    PROJECT_ID=$(curl -sf -X POST "$API/project" -H "$AUTH" -H 'Content-Type: application/json' \
        -d '{"name":"big-object-drill"}' | jq -r .id)
    IDENT="big$SUFFIX"
    SCRIPT_ID=$(curl -sf -X POST "$API/project/$PROJECT_ID/scripts" -H "$AUTH" \
        -H 'Content-Type: application/json' \
        -d "{\"publicIdentifier\":\"$IDENT\"}" | jq -r .id)

    DIR=$(mktemp -d)
    export XDG_CONFIG_HOME="$DIR/config"
    mkdir -p "$XDG_CONFIG_HOME/actias" "$DIR/project" "$DIR/project/migrations/Blob"
    printf '{"apiUrl":"%s","token":"%s"}' "${API%/api}" "$TOKEN" \
        > "$XDG_CONFIG_HOME/actias/settings.json"
    cat > "$DIR/project/script.json" <<EOJ
{"id":"$SCRIPT_ID","entryPoint":"main.lua","includes":["**/*.lua","migrations/**"],"ignore":[]}
EOJ
    cat > "$DIR/project/migrations/Blob/0001_rows.sql" <<'SQL'
CREATE TABLE rows (k INTEGER PRIMARY KEY, v BLOB NOT NULL);
SQL
    cat > "$DIR/project/main.lua" <<LUA
local Blob = object "Blob" {
    migrations = "migrations/Blob",

    -- Appends n rows of row_kb kilobytes; answers the row count.
    grow = function(state, n, row_kb)
        local v = string.rep("x", row_kb * 1024)
        for _ = 1, n do
            state.sql:exec("INSERT INTO rows (v) VALUES (?)", { v })
        end
        return state.sql:query_one("SELECT count(*) AS n FROM rows").n
    end,

    -- Rewrites one row; the page images the checkpoint will fold.
    touch = function(state, k)
        state.sql:exec("UPDATE rows SET v = ? WHERE k = ?", { string.rep("y", $ROW_KB * 1024), k })
        return k
    end,

    count = function(state)
        return state.sql:query_one("SELECT count(*) AS n FROM rows").n
    end,
}

on "fetch" (function(request)
    local verb, arg = request.uri:match("/(%a+)/?(%d*)$")
    local blob = Blob:get("big-0")
    if verb == "grow" then
        return { body = json.stringify({ n = blob:grow($ROWS_PER_GROW, $ROW_KB) }) }
    elseif verb == "touch" then
        return { body = json.stringify({ k = blob:touch(tonumber(arg)) }) }
    else
        return { body = json.stringify({ n = blob:count() }) }
    end
end)
LUA
    cargo build -p actias-cli --quiet
    ./target/debug/actias publish "$DIR/project" >/dev/null
    rm -rf "$DIR"
    echo "$IDENT"
}

wait_up() {
    for _ in $(seq 1 60); do
        curl -sf -o /dev/null "$1/_metrics" 2>/dev/null && return 0
        sleep 1
    done
    return 1
}

IDENT=$(publish)
OUT=$(mktemp -d)
grows=$(( SIZE_MB * 1024 / (ROWS_PER_GROW * ROW_KB) ))
echo "== big-object drill: growing to ${SIZE_MB} MB in $grows calls"
rows=0
for _ in $(seq 1 "$grows"); do
    rows=$(curl -sf --max-time 120 "$OWNER/$IDENT/grow" | jq -r .n)
done
file_bytes=$(( rows * ROW_KB * 1024 ))
echo "   rows $rows, about $(( file_bytes / 1024 / 1024 )) MB on disk"

puts_before=$(metric "$OWNER" actias_store_chunk_puts_total)
bytes_before=$(metric "$OWNER" actias_store_chunk_bytes_put_total)
lay_before=$(metric "$PEER" actias_replica_lay_bytes_total)
lays_before=$(metric "$PEER" actias_replica_lays_total)

echo "== touching $TOUCHES rows from the hottest ${HOT_PCT}%"
hot=$(( rows * HOT_PCT / 100 ))
[ "$hot" -lt 1 ] && hot=1
: > "$OUT/times.txt"
failed=0
for _ in $(seq 1 "$TOUCHES"); do
    k=$(( (RANDOM % hot) + 1 ))
    if t=$(curl -sf -o /dev/null -w "%{time_total}" --max-time 60 "$OWNER/$IDENT/touch/$k"); then
        echo "$t" >> "$OUT/times.txt"
    else
        failed=$(( failed + 1 ))
    fi
done
# The last flight lands before the gauges are read.
sleep 3

puts=$(( $(metric "$OWNER" actias_store_chunk_puts_total) - puts_before ))
bytes=$(( $(metric "$OWNER" actias_store_chunk_bytes_put_total) - bytes_before ))
lay_bytes=$(( $(metric "$PEER" actias_replica_lay_bytes_total) - lay_before ))
lays=$(( $(metric "$PEER" actias_replica_lays_total) - lays_before ))

python3 - "$OUT/times.txt" "$file_bytes" "$puts" "$bytes" "$lay_bytes" "$lays" "$failed" <<'PY'
import sys
times = sorted(float(t) * 1000 for t in open(sys.argv[1]).read().split())
file_bytes, puts, bytes_put, lay_bytes, lays, failed = (int(x) for x in sys.argv[2:8])
def pct(p):
    return times[min(int(len(times) * p / 100), len(times) - 1)] if times else float("nan")
rotations = max(lays, 1)
share = bytes_put / file_bytes / rotations if file_bytes else float("nan")
print(f"touches    {len(times)} ok, {failed} failed")
print(f"touch ms   p50 {pct(50):.0f}  p90 {pct(90):.0f}  p99 {pct(99):.0f}  max {times[-1] if times else 0:.0f}")
print(f"rotations  {lays} (replica lays), {puts} chunks put, {bytes_put / 1048576:.1f} MB to the store")
print(f"           {share * 100:.1f}% of the file per rotation to the store, {lay_bytes / 1048576:.1f} MB to the replica")
ok = lays >= 1 and share < 0.25 and failed == 0
print("PASS: rotations ship the dirty chunks, not the file" if ok else "FAIL: see above")
sys.exit(0 if ok else 1)
PY
touch_ok=$?

echo "== killing the owner"
docker compose stop "$OWNER_SERVICE" >/dev/null 2>&1
deadline=$(( $(date +%s) + TAKEOVER_WAIT ))
answer=""
took=$(date +%s)
while [ "$(date +%s)" -lt "$deadline" ]; do
    if answer=$(curl -sf --max-time 60 "$PEER/$IDENT/count" 2>/dev/null); then
        break
    fi
    sleep 2
done
docker compose up -d "$OWNER_SERVICE" >/dev/null 2>&1
wait_up "$OWNER"
n=$(echo "$answer" | jq -r .n 2>/dev/null || echo "")
takeovers=$(metric "$PEER" actias_replica_takeovers_total)
if [ "$n" = "$rows" ]; then
    echo "   PASS: peer answered $n rows after $(( $(date +%s) - took )) s (takeovers so far: $takeovers)"
    takeover_ok=0
else
    echo "   FAIL: peer answered '$n', expected $rows"
    takeover_ok=1
fi
rm -rf "$OUT"
exit $(( touch_ok + takeover_ok ))
