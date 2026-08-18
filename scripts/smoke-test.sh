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

echo "== web pages render"
# Server-side rendering runs the react tree, so a crash in the state layer
# turns these into 500s.
for page in / /login /projects; do
    curl -sf "http://127.0.0.1:3000$page" -o /dev/null \
        || { echo "web page $page did not render"; exit 1; }
done

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

echo "== publishing a revision (actias publish)"
# The handler writes to kv and reads it back, so the response proves the
# worker, script-service, kv-service and scylla all cooperated. Publishing
# through the CLI also runs the declaration pass, so the revision carries
# the capability contract derived from this code.
REPO=$PWD
cargo build -p actias-cli --quiet

DEVDIR=$(mktemp -d)
export XDG_CONFIG_HOME="$DEVDIR/config"
mkdir -p "$XDG_CONFIG_HOME/actias-cli" "$DEVDIR/published" "$DEVDIR/project"
printf '{"apiUrl":"http://127.0.0.1:3001","token":"%s"}' "$TOKEN" \
    > "$XDG_CONFIG_HOME/actias-cli/settings.json"

cat > "$DEVDIR/published/script.json" <<EOF
{"id":"$SCRIPT_ID","entryPoint":"main.lua","includes":["**/*.lua","**/*.txt","**/*.html"],"ignore":[]}
EOF
cat > "$DEVDIR/published/main.lua" <<'LUA'
local ns = kv "smoke"
local token = secret "smoke-token"

on "fetch" (function(request)
    ns:set("visited", true)
    log.info("hello from production")
    -- getfile hands back raw bytes; decode them for the assertion.
    local motd = string.char(table.unpack(getfile("motd.txt")))
    return {
        body = json.stringify({
            ok = true,
            visited = ns:get("visited"),
            secret = token,
            asset = motd,
        }),
        headers = { ["Content-Type"] = "application/json" },
    }
end)
LUA
printf 'hello from an asset' > "$DEVDIR/published/motd.txt"
printf '<h1>served without a vm</h1>' > "$DEVDIR/published/page.html"

echo "== setting a secret (actias secret put)"
"$REPO/target/debug/actias-cli" secret "$PROJECT_ID" put smoke-token hunter2-from-secrets

"$REPO/target/debug/actias-cli" publish "$DEVDIR/published"

echo "== republishing unchanged (incremental publish)"
# Every hash is now stored, so the same tree must upload nothing.
REPUBLISH=$("$REPO/target/debug/actias-cli" publish "$DEVDIR/published")
echo "$REPUBLISH" | grep -q "Uploading 0 of" \
    || { echo "an unchanged republish resent content"; echo "$REPUBLISH"; exit 1; }
echo "republish uploaded zero files"

echo "== checking the stored capability contract"
REV_ID=$(curl -sf "$API/script/$SCRIPT_ID" -H "$AUTH" | jq -r .currentRevisionId)
DECLARED=$(curl -sf "$API/revisions/$REV_ID" -H "$AUTH" | jq -r '.scriptConfig.capabilities.kv[0]')
[ "$DECLARED" = "smoke" ] || { echo "the capability contract was not stored (got '$DECLARED')"; exit 1; }
echo "revision declares kv: $DECLARED"

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
echo "$BODY" | jq -e '.secret == "hunter2-from-secrets"' >/dev/null \
    || { echo "the secret did not decrypt through the worker"; exit 1; }
echo "$BODY" | jq -e '.asset == "hello from an asset"' >/dev/null \
    || { echo "the asset file did not round-trip through the bundle"; exit 1; }

echo "== serving a static asset next to the lua handler"
# The html file publishes as kind: asset, so the worker answers it from the
# bundle without creating a vm: manifest content type, blake3 etag, 304 on
# revalidation.
ASSET_HEADERS=$(curl -sfD - -o "$DEVDIR/page.html.out" "$WORKER/$IDENT/page.html")
diff -q "$DEVDIR/page.html.out" "$DEVDIR/published/page.html" >/dev/null \
    || { echo "the asset body did not survive the blob path"; exit 1; }
echo "$ASSET_HEADERS" | grep -qi '^content-type: text/html' \
    || { echo "the asset lost its content type"; echo "$ASSET_HEADERS"; exit 1; }

ETAG=$(echo "$ASSET_HEADERS" | tr -d '\r' | sed -n 's/^[Ee][Tt][Aa][Gg]: //p')
[ -n "$ETAG" ] || { echo "the asset carried no etag"; echo "$ASSET_HEADERS"; exit 1; }
REVALIDATED=$(curl -s -o /dev/null -w '%{http_code}' -H "If-None-Match: $ETAG" "$WORKER/$IDENT/page.html")
[ "$REVALIDATED" = "304" ] \
    || { echo "a held etag did not revalidate (got $REVALIDATED)"; exit 1; }
echo "asset served with etag $ETAG and a 304 on revalidation"

# The encrypted store is platform-internal: the kv api must not list it.
if curl -sf "$API/project/$PROJECT_ID/kv" -H "$AUTH" | jq -e '.[] | select(.name == "__secrets")' >/dev/null 2>&1; then
    echo "the reserved secrets namespace leaked through the kv api"
    exit 1
fi

echo "== revision preview urls"
# Publish a second version, then hit the old revision at its preview url:
# the path form and the subdomain form (Host header, since nothing here
# resolves *.scripts.localhost) must both serve the old code.
OLD_REV=$(curl -sf "$API/script/$SCRIPT_ID" -H "$AUTH" | jq -r .currentRevisionId)
cat > "$DEVDIR/published/main.lua" <<'LUA'
local ns = kv "smoke"
local token = secret "smoke-token"

on "fetch" (function(request)
    log.info("hello from production")
    return { body = "version two" }
end)
LUA
"$REPO/target/debug/actias-cli" publish "$DEVDIR/published"

# The worker's pointer cache expires within seconds; wait for the flip.
CURRENT=""
for _ in $(seq 1 30); do
    CURRENT=$(curl -sf "$WORKER/$IDENT/" || true)
    [ "$CURRENT" = "version two" ] && break
    sleep 2
done
[ "$CURRENT" = "version two" ] || { echo "the new revision never went live (got '$CURRENT')"; exit 1; }

PREVIEW=$(curl -sf "$WORKER/_rev/$IDENT/$OLD_REV/")
echo "$PREVIEW" | jq -e '.ok == true' >/dev/null \
    || { echo "the path preview did not serve the old revision: $PREVIEW"; exit 1; }

HOST_PREVIEW=$(curl -sf -H "Host: $IDENT--r-$OLD_REV.scripts.localhost" "$WORKER/")
echo "$HOST_PREVIEW" | jq -e '.ok == true' >/dev/null \
    || { echo "the subdomain preview did not serve the old revision: $HOST_PREVIEW"; exit 1; }

# The subdomain form serves the current revision too, straight at the root.
HOST_CURRENT=$(curl -sf -H "Host: $IDENT.scripts.localhost" "$WORKER/")
[ "$HOST_CURRENT" = "version two" ] \
    || { echo "subdomain routing did not serve the script (got '$HOST_CURRENT')"; exit 1; }
echo "old revision previews at /_rev/ and $IDENT--r-<rev>.scripts.localhost while $IDENT.scripts.localhost serves the new one"

echo "== environment aliases (actias alias)"
# An alias is a movable pointer: aim staging at the old revision, see the
# old code at its url, then move it to the current one and see the new.
"$REPO/target/debug/actias-cli" alias "$SCRIPT_ID" set staging "$OLD_REV"

ALIASED=$(curl -sf "$WORKER/_alias/$IDENT/staging/")
echo "$ALIASED" | jq -e '.ok == true' >/dev/null \
    || { echo "the alias path form did not serve the old revision: $ALIASED"; exit 1; }
HOST_ALIASED=$(curl -sf -H "Host: $IDENT--staging.scripts.localhost" "$WORKER/")
echo "$HOST_ALIASED" | jq -e '.ok == true' >/dev/null \
    || { echo "the alias subdomain did not serve the old revision: $HOST_ALIASED"; exit 1; }

"$REPO/target/debug/actias-cli" alias "$SCRIPT_ID" list | grep -q "staging" \
    || { echo "alias list did not show staging"; exit 1; }

# Moving the alias is the rollback primitive; the pointer ttl bounds how
# long the old target keeps serving.
CURRENT_REV=$(curl -sf "$API/script/$SCRIPT_ID" -H "$AUTH" | jq -r .currentRevisionId)
"$REPO/target/debug/actias-cli" alias "$SCRIPT_ID" set staging "$CURRENT_REV"
MOVED=""
for _ in $(seq 1 30); do
    MOVED=$(curl -sf -H "Host: $IDENT--staging.scripts.localhost" "$WORKER/" || true)
    [ "$MOVED" = "version two" ] && break
    sleep 2
done
[ "$MOVED" = "version two" ] || { echo "the moved alias never served the new revision (got '$MOVED')"; exit 1; }
echo "staging alias served the old revision, then the new one after the move"

echo "== live development loop (actias dev)"
# The whole flagship path: the CLI opens a session over the websocket
# gateway, the worker serves the working tree at the live URL, and a file
# save is visible there within seconds.
cat > "$DEVDIR/project/script.json" <<EOF
{"id":"$SCRIPT_ID","entryPoint":"main.lua","includes":["**/*.lua"],"ignore":[]}
EOF
cat > "$DEVDIR/project/main.lua" <<'LUA'
on "fetch" (function(request)
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
