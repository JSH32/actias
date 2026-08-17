#!/usr/bin/env bash
# End-to-end smoke test: the whole compose stack comes up, a user registers,
# publishes a script whose handler round-trips a value through kv, and the
# worker serves it. If this passes, every service and both datastores took
# part in one request path, and the checked-in clients match the live api.
#
# Requires docker on the host and jq/curl from the devshell. Run as:
#   just smoke
set -euo pipefail

cd "$(dirname "$0")/.."

PROJECT=actias-smoke
API=http://127.0.0.1:3001/api
WORKER=http://127.0.0.1:3002
FAILED=1

compose() { docker compose -p "$PROJECT" "$@"; }

cleanup() {
    if [ "$FAILED" = 1 ]; then
        echo "== last service logs"
        compose logs --tail 25 actias_api worker_service kv_service script_service 2>/dev/null || true
        [ -f "${DEV_LOG:-}" ] && { echo "== actias dev log"; cat "$DEV_LOG"; }
    fi
    [ -n "${DEV_PID:-}" ] && kill "$DEV_PID" 2>/dev/null || true
    rm -rf "${DEVDIR:-}"
    compose down -v --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "== building and starting the stack"
compose up -d --build --quiet-pull

echo "== waiting for the api"
ready=0
for _ in $(seq 1 120); do
    if curl -sf "$API/docs/openapi.json" -o /dev/null; then
        ready=1
        break
    fi
    sleep 2
done
[ "$ready" = 1 ] || { echo "api never became ready"; exit 1; }

echo "== registering and logging in"
SUFFIX=$RANDOM
curl -sf -X POST "$API/users" -H 'Content-Type: application/json' \
    -d "{\"username\":\"smokeuser$SUFFIX\",\"email\":\"smoke$SUFFIX@example.com\",\"password\":\"smoketest-password\"}" >/dev/null

TOKEN=$(curl -sf -X POST "$API/auth/login" -H 'Content-Type: application/json' \
    -d "{\"auth\":\"smokeuser$SUFFIX\",\"password\":\"smoketest-password\"}" | jq -r .token)
AUTH="Authorization: Bearer $TOKEN"

echo "== creating project and script"
PROJECT_ID=$(curl -sf -X POST "$API/project" -H "$AUTH" -H 'Content-Type: application/json' \
    -d '{"name":"smoke-project"}' | jq -r .id)

IDENT="smoke$SUFFIX"
SCRIPT_ID=$(curl -sf -X POST "$API/project/$PROJECT_ID/scripts" -H "$AUTH" -H 'Content-Type: application/json' \
    -d "{\"publicIdentifier\":\"$IDENT\"}" | jq -r .id)

echo "== publishing a revision"
# The handler writes to kv and reads it back, so the response proves the
# worker, script-service, kv-service and scylla all cooperated.
MAIN_LUA=$(base64 -w0 <<'LUA'
add_event_listener("fetch", function(request)
    local ns = kv.get_namespace("smoke")
    ns:set("visited", true)
    log.info("hello from production")
    return {
        body = json.stringify({ ok = true, visited = ns:get("visited") }),
        headers = { ["Content-Type"] = "application/json" },
    }
end)
LUA
)

curl -sf -X PUT "$API/script/$SCRIPT_ID/revisions" -H "$AUTH" -H 'Content-Type: application/json' -d @- >/dev/null <<EOF
{
    "bundle": {
        "entryPoint": "main.lua",
        "files": [{"fileName": "main.lua", "filePath": "main.lua", "content": "$MAIN_LUA"}]
    },
    "scriptConfig": {"id": "$SCRIPT_ID", "entryPoint": "main.lua", "includes": ["**/*.lua"], "ignore": []}
}
EOF

echo "== requesting the script through the worker"
BODY=""
for _ in $(seq 1 30); do
    if BODY=$(curl -sf "$WORKER/$IDENT/") && [ -n "$BODY" ]; then
        break
    fi
    sleep 2
done

echo "worker responded: $BODY"
echo "$BODY" | jq -e '.ok == true and .visited == true' >/dev/null \
    || { echo "response did not round-trip through kv"; exit 1; }

echo "== live development loop (actias dev)"
# The whole flagship path: the CLI opens a session over the websocket
# gateway, the worker serves the working tree at the live URL, and a file
# save is visible there within seconds.
REPO=$PWD
cargo build -p actias-cli --quiet

DEVDIR=$(mktemp -d)
export XDG_CONFIG_HOME="$DEVDIR/config"
mkdir -p "$XDG_CONFIG_HOME/actias-cli" "$DEVDIR/project"
printf '{"apiUrl":"http://127.0.0.1:3001","token":"%s"}' "$TOKEN" \
    > "$XDG_CONFIG_HOME/actias-cli/settings.json"

cat > "$DEVDIR/project/script.json" <<EOF
{"id":"$SCRIPT_ID","entryPoint":"main.lua","includes":["**/*.lua"],"ignore":[]}
EOF
cat > "$DEVDIR/project/main.lua" <<'LUA'
add_event_listener("fetch", function(request)
    log.info("hello from the live session")
    return { body = "live version one" }
end)
LUA

DEV_LOG="$DEVDIR/dev.log"
(cd "$DEVDIR" && exec "$REPO/target/debug/actias-cli" dev project --worker-url "$WORKER" > "$DEV_LOG" 2>&1) &
DEV_PID=$!

LIVE_URL=""
for _ in $(seq 1 30); do
    LIVE_URL=$(grep -oE "$WORKER/_live/[^ ]+/" "$DEV_LOG" | head -1 || true)
    [ -n "$LIVE_URL" ] && break
    sleep 1
done
[ -n "$LIVE_URL" ] || { echo "actias dev never printed a live URL"; exit 1; }

LIVE_BODY=$(curl -sf "$LIVE_URL")
echo "live session responded: $LIVE_BODY"
[ "$LIVE_BODY" = "live version one" ] || { echo "live URL did not serve the working tree"; exit 1; }

sed -i 's/live version one/live version two/' "$DEVDIR/project/main.lua"
for _ in $(seq 1 20); do
    sleep 1
    LIVE_BODY=$(curl -sf "$LIVE_URL" || true)
    [ "$LIVE_BODY" = "live version two" ] && break
done
echo "after save: $LIVE_BODY"
[ "$LIVE_BODY" = "live version two" ] || { echo "the save never reached the live URL"; exit 1; }

# The requests above ran log.info, so the line must have crossed
# worker -> redis -> script-service stream -> gateway -> CLI by now.
LOGGED=0
for _ in $(seq 1 15); do
    if grep -q "hello from the live session" "$DEV_LOG"; then
        LOGGED=1
        break
    fi
    sleep 1
done
[ "$LOGGED" = 1 ] || { echo "the log line never reached the CLI"; exit 1; }
echo "log line reached the CLI"

kill "$DEV_PID" 2>/dev/null || true
DEV_PID=""

echo "== tailing the published script (actias tail)"
TAIL_LOG="$DEVDIR/tail.log"
"$REPO/target/debug/actias-cli" tail "$SCRIPT_ID" > "$TAIL_LOG" 2>&1 &
DEV_PID=$!

for _ in $(seq 1 15); do
    grep -q "Tailing" "$TAIL_LOG" && break
    sleep 1
done

# A fresh request makes the published script log, which must arrive in the
# tail's terminal.
curl -sf "$WORKER/$IDENT/" -o /dev/null
TAILED=0
for _ in $(seq 1 15); do
    if grep -q "hello from production" "$TAIL_LOG"; then
        TAILED=1
        break
    fi
    curl -sf "$WORKER/$IDENT/" -o /dev/null || true
    sleep 1
done
[ "$TAILED" = 1 ] || { echo "the production log line never reached the tail"; cat "$TAIL_LOG"; exit 1; }
echo "production log line reached the tail"

kill "$DEV_PID" 2>/dev/null || true
DEV_PID=""

echo "== regenerating clients against the live api (drift coverage)"
( cd actias-web && npm run generateClient >/dev/null 2>&1 )
curl -sf "$API/docs/openapi.json" -o actias-cli/src/actias-api.json

if ! git diff --quiet -- actias-web/src/client actias-cli/src/actias-api.json; then
    echo "generated clients are out of date with the api:"
    git --no-pager diff --stat -- actias-web/src/client actias-cli/src/actias-api.json
    exit 1
fi

FAILED=0
echo "SMOKE TEST PASSED"
