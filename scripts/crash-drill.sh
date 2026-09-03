#!/usr/bin/env bash
# The crash-point matrix for tail replication, on the compose stack.
#
# For each point, the owner worker is restarted with ACTIAS_DRILL_FAULT
# naming it, so the process exits the instant a flight reaches that
# point. Only a worker built with the `drill` cargo feature has the
# points; the compose image is (CARGO_FLAGS in docker-compose.yml). Writes go through the owner until one fails; the last count it
# answered is the last acknowledged write. Then the peer takes the
# object over (after the registry reaps the dead node id) and the next
# write must continue from there: every acknowledged write present,
# and at most the one unacknowledged write in flight.
#
#   POINTS="after-commit after-quorum" ./scripts/crash-drill.sh
set -euo pipefail

cd "$(dirname "$0")/.."

API="${API:-http://127.0.0.1:3001/api}"
OWNER="${OWNER:-http://127.0.0.1:3002}"
PEER="${PEER:-http://127.0.0.1:3003}"
OWNER_SERVICE="${OWNER_SERVICE:-worker_service}"
POINTS="${POINTS:-after-commit after-quorum after-segment after-manifest}"
TAKEOVER_WAIT="${TAKEOVER_WAIT:-90}"

publish() {
    SUFFIX=$RANDOM
    curl -sf -X POST "$API/users" -H 'Content-Type: application/json' \
        -d "{\"username\":\"crash$SUFFIX\",\"email\":\"crash$SUFFIX@example.com\",\"password\":\"crash-drill-password\"}" >/dev/null
    TOKEN=$(curl -sf -X POST "$API/auth/login" -H 'Content-Type: application/json' \
        -d "{\"auth\":\"crash$SUFFIX\",\"password\":\"crash-drill-password\"}" | jq -r .token)
    AUTH="Authorization: Bearer $TOKEN"
    PROJECT_ID=$(curl -sf -X POST "$API/project" -H "$AUTH" -H 'Content-Type: application/json' \
        -d '{"name":"crash-drill"}' | jq -r .id)
    IDENT="crash$SUFFIX"
    SCRIPT_ID=$(curl -sf -X POST "$API/project/$PROJECT_ID/scripts" -H "$AUTH" \
        -H 'Content-Type: application/json' \
        -d "{\"publicIdentifier\":\"$IDENT\"}" | jq -r .id)
    DIR=$(mktemp -d)
    export XDG_CONFIG_HOME="$DIR/config"
    mkdir -p "$XDG_CONFIG_HOME/actias" "$DIR/project"
    printf '{"apiUrl":"%s","token":"%s"}' "${API%/api}" "$TOKEN" \
        > "$XDG_CONFIG_HOME/actias/settings.json"
    cat > "$DIR/project/script.json" <<EOJ
{"id":"$SCRIPT_ID","entryPoint":"main.lua","includes":["**/*.lua"],"ignore":[]}
EOJ
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

wait_up() {
    for _ in $(seq 1 60); do
        curl -sf -o /dev/null "$1/_metrics" 2>/dev/null && return 0
        sleep 1
    done
    return 1
}

failed=0
for point in $POINTS; do
    echo "== point: $point"
    ACTIAS_DRILL_FAULT="$point" docker compose up -d "$OWNER_SERVICE" >/dev/null 2>&1
    wait_up "$OWNER"
    IDENT=$(publish)
    last=0
    for _ in $(seq 1 200); do
        if n=$(curl -sf --max-time 15 "$OWNER/$IDENT/" 2>/dev/null | jq -r .n 2>/dev/null) && [ -n "$n" ]; then
            last=$n
        else
            break
        fi
    done
    echo "   owner died after answering n = $last"
    # A dead machine stays dead: compose would otherwise restart the
    # owner at once, with the fault still set, as a new node at the same
    # address.
    docker compose stop "$OWNER_SERVICE" >/dev/null 2>&1

    deadline=$(( $(date +%s) + TAKEOVER_WAIT ))
    answer=""
    while [ "$(date +%s)" -lt "$deadline" ]; do
        if answer=$(curl -sf --max-time 15 "$PEER/$IDENT/" 2>/dev/null); then
            break
        fi
        sleep 2
    done
    ACTIAS_DRILL_FAULT= docker compose up -d "$OWNER_SERVICE" >/dev/null 2>&1
    wait_up "$OWNER"
    # The dead incarnation's node id leaves the registry after its ttl;
    # the next point starts on a settled membership.
    sleep "${SETTLE:-50}"
    if [ -z "$answer" ]; then
        echo "   FAIL: the peer never answered"
        failed=1
        continue
    fi
    next=$(echo "$answer" | jq -r .n)
    if [ "$next" -ge $((last + 1)) ] && [ "$next" -le $((last + 2)) ]; then
        echo "   PASS: peer answered n = $next (every acknowledged write present; at most one in flight)"
    else
        echo "   FAIL: peer answered n = $next, expected $((last + 1)) or $((last + 2))"
        failed=1
    fi
done
exit $failed
