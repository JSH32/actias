#!/usr/bin/env bash
# The durability drill for tail replication: writes answered on a
# replica quorum survive the owner dying before the store catches up.
#
# Against the compose stack (two workers): publish a ledger, write
# through worker 1 and remember the last count it answered, kill worker
# 1 without a drain, then write through worker 2 and check the count
# continues from where the answers left off. Nothing an acked call did
# may be missing, and the takeover must have come from the replica.
#
#   WRITES=50 ./scripts/takeover-drill.sh
set -euo pipefail

cd "$(dirname "$0")/.."

API="${API:-http://127.0.0.1:3001/api}"
OWNER="${OWNER:-http://127.0.0.1:3002}"
PEER="${PEER:-http://127.0.0.1:3003}"
OWNER_SERVICE="${OWNER_SERVICE:-worker_service}"
WRITES="${WRITES:-50}"
# The registry reaps a silent node after NODE_TTL_SECS; the claim from
# worker 2 waits on that.
TAKEOVER_WAIT="${TAKEOVER_WAIT:-90}"

setup() {
    SUFFIX=$RANDOM
    curl -sf -X POST "$API/users" -H 'Content-Type: application/json' \
        -d "{\"username\":\"take$SUFFIX\",\"email\":\"take$SUFFIX@example.com\",\"password\":\"takeover-drill-password\"}" >/dev/null
    TOKEN=$(curl -sf -X POST "$API/auth/login" -H 'Content-Type: application/json' \
        -d "{\"auth\":\"take$SUFFIX\",\"password\":\"takeover-drill-password\"}" | jq -r .token)
    AUTH="Authorization: Bearer $TOKEN"
    PROJECT_ID=$(curl -sf -X POST "$API/project" -H "$AUTH" -H 'Content-Type: application/json' \
        -d '{"name":"takeover-drill"}' | jq -r .id)
    IDENT="take$SUFFIX"
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
    return { body = json.stringify({ n = Ledger:get("led-0"):append() }) }
end)
LUA
    cargo build -p actias-cli --quiet
    ./target/debug/actias publish "$DIR/project" >/dev/null
    rm -rf "$DIR"
    echo "$IDENT"
}

replica_metric() {
    curl -sf "$1/_metrics" 2>/dev/null | awk -v name="$2" '$1 == name { print $2 }'
}

IDENT=$(setup)
echo "== published $IDENT"

echo "== $WRITES gated writes through the owner"
last=0
for _ in $(seq 1 "$WRITES"); do
    n=$(curl -sf "$OWNER/$IDENT/" | jq -r .n)
    last=$n
done
echo "   last answer: n = $last"
echo "   owner quorum releases: $(replica_metric "$OWNER" actias_replica_quorum_releases_total), store releases: $(replica_metric "$OWNER" actias_replica_store_releases_total)"
echo "   peer appends held: $(replica_metric "$PEER" actias_replica_appends_total), bytes held: $(replica_metric "$PEER" actias_replica_bytes_held)"

echo "== killing the owner without a drain"
docker compose kill "$OWNER_SERVICE" >/dev/null

echo "== writing through the peer (waits for the lease to age out, up to ${TAKEOVER_WAIT}s)"
takeovers_before=$(replica_metric "$PEER" actias_replica_takeovers_total)
deadline=$(( $(date +%s) + TAKEOVER_WAIT ))
answer=""
while [ "$(date +%s)" -lt "$deadline" ]; do
    if answer=$(curl -sf "$PEER/$IDENT/" 2>/dev/null); then
        break
    fi
    sleep 2
done
docker compose up -d "$OWNER_SERVICE" >/dev/null

if [ -z "$answer" ]; then
    echo "FAIL: the peer never answered within ${TAKEOVER_WAIT}s"
    exit 1
fi
next=$(echo "$answer" | jq -r .n)
takeovers_after=$(replica_metric "$PEER" actias_replica_takeovers_total)
incidents=$(replica_metric "$PEER" actias_replica_takeover_incidents_total)
echo "   peer answered n = $next (expected $((last + 1)))"
echo "   takeovers from a replica: $((takeovers_after - takeovers_before)), incidents: $incidents"

if [ "$next" -ne $((last + 1)) ]; then
    echo "FAIL: an acknowledged write is missing after takeover"
    exit 1
fi
if [ $((takeovers_after - takeovers_before)) -lt 1 ]; then
    echo "FAIL: the peer restored from the store rather than its replica"
    exit 1
fi
echo "PASS: every acknowledged write survived the owner's death, taken over from the replica"
