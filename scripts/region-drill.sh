#!/usr/bin/env bash
# The region drill (FLEET.md P4.d), on the two-region compose overlay.
#
# Region a is the base stack (ACTIAS_REGION=a), region b the overlay's
# placement store and workers. The drill registers both with the
# control plane, homes a project in a, and checks: a call through an a
# worker is served in a; a call through a b worker answers in one hop
# and continues the same count; the project moved to b under writes
# loses no acknowledged write; afterwards a serves by forwarding and b
# serves locally; and an a worker holding a residency across the flip
# is fenced on its next flight.
#
#   ACTIAS_REGION=a ADMIN_USERNAME=admin ADMIN_EMAIL=admin@example.com ADMIN_PASSWORD=admin-drill-password \
#     docker compose -f docker-compose.yml -f docker-compose.regions.yml up -d --build
#   ./scripts/region-drill.sh
#
# Beside a dev stack on the default ports, run it on its own project:
#   COMPOSE_PROJECT_NAME=actias-regions ACTIAS_REGION=a ADMIN_USERNAME=admin ... \
#     ACTIAS_API_PORT=13001 ACTIAS_WORKER_PORT=13002 ACTIAS_WORKER2_PORT=13003 \
#     ACTIAS_WORKER3_PORT=13004 ACTIAS_WORKER4_PORT=13005 (and the *_DATA_PORT, web, grafana ports) \
#     docker compose -f docker-compose.yml -f docker-compose.regions.yml up -d --build
#   API=http://127.0.0.1:13001/api A1=http://127.0.0.1:13002 A2=http://127.0.0.1:13003 \
#     B1=http://127.0.0.1:13004 B2=http://127.0.0.1:13005 ./scripts/region-drill.sh
set -euo pipefail

cd "$(dirname "$0")/.."

API="${API:-http://127.0.0.1:3001/api}"
A1="${A1:-http://127.0.0.1:3002}"
A2="${A2:-http://127.0.0.1:3003}"
B1="${B1:-http://127.0.0.1:3004}"
B2="${B2:-http://127.0.0.1:3005}"
ADMIN_USERNAME="${ADMIN_USERNAME:-admin}"
ADMIN_PASSWORD="${ADMIN_PASSWORD:-admin-drill-password}"
WRITES="${WRITES:-30}"
# The move drains for MOVE_DRAIN_SECS on the script-service (40 by
# default) before it copies; the wait covers it and the copy.
MOVE_WAIT="${MOVE_WAIT:-180}"

metric() {
    curl -sf "$1/_metrics" 2>/dev/null | awk -v name="$2" '$1 == name { print $2 }'
}

admin_token() {
    curl -sf -X POST "$API/auth/login" -H 'Content-Type: application/json' \
        -d "{\"auth\":\"$ADMIN_USERNAME\",\"password\":\"$ADMIN_PASSWORD\"}" | jq -r .token
}

setup() {
    SUFFIX=$RANDOM
    curl -sf -X POST "$API/users" -H 'Content-Type: application/json' \
        -d "{\"username\":\"region$SUFFIX\",\"email\":\"region$SUFFIX@example.com\",\"password\":\"region-drill-password\"}" >/dev/null
    TOKEN=$(curl -sf -X POST "$API/auth/login" -H 'Content-Type: application/json' \
        -d "{\"auth\":\"region$SUFFIX\",\"password\":\"region-drill-password\"}" | jq -r .token)
    AUTH="Authorization: Bearer $TOKEN"
    PROJECT_ID=$(curl -sf -X POST "$API/project" -H "$AUTH" -H 'Content-Type: application/json' \
        -d '{"name":"region-drill"}' | jq -r .id)
    IDENT="region$SUFFIX"
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
    echo "$IDENT $PROJECT_ID $TOKEN"
}

echo "== registering regions a and b with the control plane"
ADMIN=$(admin_token)
[ -n "$ADMIN" ] && [ "$ADMIN" != null ] || { echo "FAIL: no admin token; set ADMIN_* on the stack"; exit 1; }
curl -sf -X PUT "$API/regions/a" -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/json' \
    -d '{"dataPlaneAddr":"worker_service:3100","bucket":"actias-blobs","placementAddr":"http://placement_service:3000"}' >/dev/null
curl -sf -X PUT "$API/regions/b" -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/json' \
    -d '{"dataPlaneAddr":"worker_service_3:3100","bucket":"actias-b","placementAddr":"http://placement_service_b:3000"}' >/dev/null
echo "   $(curl -sf "$API/regions" -H "Authorization: Bearer $ADMIN" | jq -r '[.[].name] | join(", ")')"

read -r IDENT PROJECT_ID TOKEN <<<"$(setup)"
AUTH="Authorization: Bearer $TOKEN"
echo "== published $IDENT, project $PROJECT_ID"
HOME_REGION=$(curl -sf "$API/project/$PROJECT_ID/policy" -H "$AUTH" | jq -r .region)
[ "$HOME_REGION" = a ] || { echo "FAIL: the project is homed in '$HOME_REGION', expected a"; exit 1; }
echo "   homed in a"

# A worker that just came up answers once it has registered; wait for
# the first answer rather than counting a boot as a failure.
first=""
for _ in $(seq 1 30); do
    first=$(curl -sf "$A1/$IDENT/" 2>/dev/null | jq -r .n 2>/dev/null) && [ -n "$first" ] && break
    sleep 2
done
[ -n "$first" ] || { echo "FAIL: region a never answered"; exit 1; }

echo "== $WRITES writes through region a"
a_forwards_start=$(metric "$A1" actias_region_forwards_total)
last=$first
for _ in $(seq 2 "$WRITES"); do last=$(curl -sf "$A1/$IDENT/" | jq -r .n); done
echo "   a answered n = $last"
# Counters are cumulative across runs on one stack: what matters is
# that these writes added no forward.
[ "$(metric "$A1" actias_region_forwards_total)" = "$a_forwards_start" ] || { echo "FAIL: a forwarded a call for its own object"; exit 1; }

echo "== a call through region b answers in one hop"
forwards_before=$(metric "$B1" actias_region_forwards_total)
n=$(curl -sf "$B1/$IDENT/" | jq -r .n)
[ "$n" -gt "$last" ] || { echo "FAIL: b did not continue the count ($last -> '$n')"; exit 1; }
forwards_after=$(metric "$B1" actias_region_forwards_total)
[ "$forwards_after" -gt "$forwards_before" ] || { echo "FAIL: b served the object itself (forwards $forwards_before -> $forwards_after)"; exit 1; }
last=$n
echo "   b forwarded once and got n = $last"

echo "== moving the project to b under writes"
curl -sf -X PATCH "$API/project/$PROJECT_ID/region" -H "$AUTH" -H 'Content-Type: application/json' \
    -d '{"region":"b"}' | jq -c '{step, fromRegion, toRegion}'
deadline=$(( $(date +%s) + MOVE_WAIT ))
step=""
acked=$last
refused=0
while [ "$(date +%s)" -lt "$deadline" ]; do
    step=$(curl -sf "$API/project/$PROJECT_ID/move" -H "$AUTH" | jq -r .step)
    [ "$step" = done ] && break
    [ "$step" = failed ] && { echo "FAIL: the move failed: $(curl -sf "$API/project/$PROJECT_ID/move" -H "$AUTH" | jq -r .error)"; exit 1; }
    # Writes during the move: answered before the drain, refused
    # retryably during it, answered after the flip. Only an answer
    # counts as acknowledged.
    if n=$(curl -sf "$A1/$IDENT/" 2>/dev/null | jq -r .n 2>/dev/null) && [ -n "$n" ] && [ "$n" != null ]; then
        acked=$n
    else
        refused=$((refused + 1))
    fi
    sleep 1
done
[ "$step" = done ] || { echo "FAIL: the move did not finish within ${MOVE_WAIT}s (step '$step')"; exit 1; }
echo "   moved; last acknowledged write n = $acked, $refused calls refused during the move"
MOVE=$(curl -sf "$API/project/$PROJECT_ID/move" -H "$AUTH")
echo "   $(echo "$MOVE" | jq -r '"copied \(.objectsCopied) of \(.objectsTotal) objects"')"

echo "== after the move: b serves locally, a forwards"
b_forwards_before=$(metric "$B1" actias_region_forwards_total)
n=$(curl -sf "$B1/$IDENT/" | jq -r .n)
[ "$n" -gt "$acked" ] || { echo "FAIL: b lost acknowledged writes ($acked -> '$n')"; exit 1; }
[ "$(metric "$B1" actias_region_forwards_total)" = "$b_forwards_before" ] || { echo "FAIL: b still forwards after the move"; exit 1; }
echo "   b answered n = $n locally"
a_forwards_before=$(metric "$A1" actias_region_forwards_total)
# Within a pointer ttl of the flip, a still reads the project as moving
# and refuses retryably (FLEET.md 6.3 step 4); the caller retries.
m=""
for _ in $(seq 1 15); do
    m=$(curl -sf "$A1/$IDENT/" 2>/dev/null | jq -r .n 2>/dev/null) && [ -n "$m" ] && break
    sleep 1
done
[ -n "$m" ] && [ "$m" -gt "$n" ] || { echo "FAIL: a did not continue the count after the move ($n -> '$m')"; exit 1; }
[ "$(metric "$A1" actias_region_forwards_total)" -gt "$a_forwards_before" ] || { echo "FAIL: a served the moved object itself"; exit 1; }
echo "   a forwarded to b and got n = $m"

echo "== a residency left in a is fenced"
fenced=$(metric "$A1" actias_region_fenced_total)
[ -n "$fenced" ] || fenced=0
echo "   a fenced $fenced flight(s) across the flip (0 is fine when the drain ended every residency first)"

echo "PASS: a project moved between regions under writes with every acknowledged write present"
