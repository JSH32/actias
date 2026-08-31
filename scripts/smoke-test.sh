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
# Tight object timings so the cold-alarm section fits a test run: fast
# sweeps, and leases free quickly after the worker restart.
export OBJECT_SWEEP_SECS=3
export NODE_TTL_SECS=10
export OBJECT_REPLICA_TTL_SECS=2
# The smoke stack publishes on its own port range so it can run beside a
# live dev stack; the compose file reads the same variables.
PORT_BASE="${SMOKE_PORT_BASE:-13000}"
export ACTIAS_WEB_PORT=$PORT_BASE
export ACTIAS_API_PORT=$((PORT_BASE + 1))
export ACTIAS_WORKER_PORT=$((PORT_BASE + 2))
export ACTIAS_WORKER2_PORT=$((PORT_BASE + 3))
export ACTIAS_WORKER_DATA_PORT=$((PORT_BASE + 102))
export ACTIAS_WORKER2_DATA_PORT=$((PORT_BASE + 103))
export ACTIAS_GRAFANA_PORT=$((PORT_BASE + 30))
# Tight hibernation so the connection checkpoint fits a test run.
export CONNECTION_HIBERNATE_SECS=3
WEB=http://127.0.0.1:$ACTIAS_WEB_PORT
API=http://127.0.0.1:$ACTIAS_API_PORT/api
WORKER=http://127.0.0.1:$ACTIAS_WORKER_PORT
WORKER2=http://127.0.0.1:$ACTIAS_WORKER2_PORT
FAILED=1

compose() { docker compose -p "$PROJECT" "$@"; }

cleanup() {
    if [ "$FAILED" = 1 ]; then
        # The full logs go to a file for post-mortems; the terminal gets
        # only the lines that carry a verdict, because debug noise from
        # h2 and the aws sdk buries errors in any bounded tail.
        FULL_LOG="${SMOKE_LOG_FILE:-$(pwd)/smoke-logs.txt}"
        compose logs --no-color > "$FULL_LOG" 2>/dev/null || true
        echo "== error lines from all services (full logs: $FULL_LOG)"
        grep -aiE " error |panicked|error handling request" "$FULL_LOG" | tail -40 || true
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
for page in / /login /projects /settings /download /script/shell /project/shell; do
    curl -sf "$WEB$page" -o /dev/null \
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
printf '{"apiUrl":"http://127.0.0.1:%s","token":"%s"}' "$ACTIAS_API_PORT" "$TOKEN" \
    > "$XDG_CONFIG_HOME/actias-cli/settings.json"

cat > "$DEVDIR/published/script.json" <<EOF
{"id":"$SCRIPT_ID","entryPoint":"main.lua","includes":["**/*.lua","**/*.txt","**/*.html","migrations/**/*.sql"],"ignore":[]}
EOF
cat > "$DEVDIR/published/main.lua" <<'LUA'
local ns = kv "smoke"
local token = secret "smoke-token"

-- A durable object: one pinned vm owns this state, every request routes
-- its call through the same mailbox.
-- The sql product face: durable rows behind one declaration. The
-- migrations body is what puts the ladder into the contract; without
-- it the tables below never exist and every request fails.
local db = database "main" { migrations = "migrations/main" }

local Hits = object "Hits" {
    publishes = { poked = "public" },

    bump = function(state)
        state.sql:exec("CREATE TABLE IF NOT EXISTS hits (at INTEGER)")
        state.sql:exec("INSERT INTO hits VALUES (?)", { 1 })
        return state.sql:query_one("SELECT COUNT(*) AS n FROM hits").n
    end,

    poke = function(state)
        state:publish("poked", { at = state.now() })
        return true
    end,
}

-- The lifecycle class: a declared lifespan, an admission gate, and a
-- method that ends the object from inside. The counter rides the
-- store face, so a recreation starting back at 1 proves the storage
-- was reclaimed, not just the row.
local Visit = object "Visit" {
    expire = "10s",
    admit = function(name)
        return #name > 3
    end,
    touch = function(state)
        local n = (state.store:get("touched") or 0) + 1
        state.store:set("touched", n)
        return n
    end,
    finish = function(state)
        local n = state.store:get("touched") or 0
        state:destroy()
        return n
    end,
}

-- The connection checkpoint's subject: echoes count into conn.state,
-- a followed publish lands as an event, and the blob must survive
-- the vm dropping in between.
local Live = connection "Live" {
    open = function(conn)
        conn:follow(Hits("global"), "poked")
        conn:send({ kind = "hello" })
    end,
    frame = function(conn, data)
        conn.state.frames = (conn.state.frames or 0) + 1
        conn:send({ kind = "echo", n = conn.state.frames })
    end,
    event = function(conn, event)
        conn:send({ kind = "poked", frames = conn.state.frames })
    end,
}

-- The heartbeat: ticks must arrive while the hibernate threshold
-- passes underneath, because a timered connection stays warm.
local Pulse = connection "Pulse" {
    timer = { every = "2s", run = function(conn, missed)
        conn.state.beats = (conn.state.beats or 0) + 1
        conn:send({ kind = "beat", n = conn.state.beats, missed = missed })
    end },
}

-- Armed once before the worker restarts and never touched again: only
-- the cold-alarm sweep can fire this, and the mark it writes into the
-- shared database is the proof.
local AlarmKeeper = object "AlarmKeeper" {
    arm = function(state, duration)
        state:set_alarm(duration)
        return true
    end,
    alarm = function(state)
        db:exec("INSERT INTO alarm_marks VALUES (?)", { state.now() })
    end,
}

-- Every two seconds, forever, from the script's first touch on.
on "cron:*/2 * * * * *" (function(event)
    db:exec("INSERT INTO cron_marks VALUES (?)", { event.scheduled_at })
end)

-- A durable queue: send enqueues into the queue object's own sqlite, the
-- alarm loop delivers to this listener, which records the receipt where
-- the smoke test can count it.
local jobs = queue "jobs"
on "queue:jobs" (function(message)
    db:exec("INSERT INTO queue_done VALUES (?)", { message.n })
end)

on "fetch" (function(request)
    if request.upgrade and string.find(request.context_uri or "", "/live") then
        return request:upgrade(Live, Hits("watcher"))
    end
    if request.upgrade and string.find(request.context_uri or "", "/pulse") then
        return request:upgrade(Pulse, Hits("watcher"))
    end
    if string.find(request.context_uri or "", "/poke") then
        Hits:get("global"):poke()
    end
    if string.find(request.context_uri or "", "/arm") then
        AlarmKeeper:get("watchdog"):arm("15s")
    end
    if string.find(request.context_uri or "", "/enqueue") then
        jobs:send({ n = 1 })
    end
    if string.find(request.context_uri or "", "/visit") then
        local q = request.query or {}
        local visit = Visit(q.name or "guest")
        if q.act == "finish" then
            return { body = json.stringify({ finished = visit:finish() }) }
        end
        return { body = json.stringify({ touched = visit:touch() }) }
    end
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
            hits = Hits:get("global"):bump(),
            marks = db:read_one("SELECT COUNT(*) AS n FROM alarm_marks").n,
            crons = db:read_one("SELECT COUNT(*) AS n FROM cron_marks").n,
            queued = db:read_one("SELECT COUNT(*) AS n FROM queue_done").n,
            db_rows = (function()
                -- The visits table exists because the migration applied at
                -- the database's first touch; nothing creates it here.
                db:exec("INSERT INTO visits VALUES (?)", { 1 })
                -- read_one exercises the mailbox bypass end to end.
                return db:read_one("SELECT COUNT(*) AS n FROM visits").n
            end)(),
        }),
        headers = { ["Content-Type"] = "application/json" },
    }
end)
LUA
printf 'hello from an asset' > "$DEVDIR/published/motd.txt"

# Scaffold the migration through the cli, then give it content. The bare
# CREATE (no IF NOT EXISTS) doubles as the reapply detector: a second
# application would fail every db request.
"$REPO/target/debug/actias" sql main create visits --directory "$DEVDIR/published"
cat > "$DEVDIR/published/migrations/main/0001_visits.sql" <<'SQL'
CREATE TABLE visits (at INTEGER);
SQL
"$REPO/target/debug/actias" sql main create alarm_marks --directory "$DEVDIR/published"
cat > "$DEVDIR/published/migrations/main/0002_alarm_marks.sql" <<'SQL'
CREATE TABLE alarm_marks (at INTEGER);
SQL
"$REPO/target/debug/actias" sql main create cron_marks --directory "$DEVDIR/published"
cat > "$DEVDIR/published/migrations/main/0003_cron_marks.sql" <<'SQL'
CREATE TABLE cron_marks (at INTEGER);
SQL
"$REPO/target/debug/actias" sql main create queue_done --directory "$DEVDIR/published"
cat > "$DEVDIR/published/migrations/main/0004_queue_done.sql" <<'SQL'
CREATE TABLE queue_done (n INTEGER);
SQL
printf '<h1>served without a vm</h1>' > "$DEVDIR/published/page.html"

echo "== setting a secret (actias secret put)"
"$REPO/target/debug/actias" secret "$PROJECT_ID" put smoke-token hunter2-from-secrets

"$REPO/target/debug/actias" publish "$DEVDIR/published"

echo "== republishing unchanged (incremental publish)"
# Every hash is now stored, so the same tree must upload nothing.
REPUBLISH=$("$REPO/target/debug/actias" publish "$DEVDIR/published")
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
STATUS=000
for _ in $(seq 1 30); do
    STATUS=$(curl -s -o /tmp/smoke-body.$$ -w '%{http_code}' "$WORKER/$IDENT/") || STATUS=000
    BODY=$(cat /tmp/smoke-body.$$ 2>/dev/null || true)
    if [ "$STATUS" = 200 ] && [ -n "$BODY" ]; then
        break
    fi
    sleep 2
done
rm -f /tmp/smoke-body.$$

echo "worker responded ($STATUS): $BODY"
echo "$BODY" | jq -e '.ok == true and .visited == true' >/dev/null \
    || { echo "response did not round-trip through kv"; exit 1; }
echo "$BODY" | jq -e '.secret == "hunter2-from-secrets"' >/dev/null \
    || { echo "the secret did not decrypt through the worker"; exit 1; }
echo "$BODY" | jq -e '.asset == "hello from an asset"' >/dev/null \
    || { echo "the asset file did not round-trip through the bundle"; exit 1; }

echo "== durable object state across requests"
# Each request runs in a fresh vm; the counter lives in the object's own
# pinned vm, so consecutive requests must observe each other's bumps.
H1=$(curl -sf "$WORKER/$IDENT/" | jq .hits)
H2=$(curl -sf "$WORKER/$IDENT/" | jq .hits)
[ "$H2" -gt "$H1" ] 2>/dev/null \
    || { echo "object state did not carry across requests ($H1 -> $H2)"; exit 1; }
OBJ_DECLARED=$(curl -sf "$API/revisions/$REV_ID" -H "$AUTH" | jq -r '.scriptConfig.capabilities.objects[0]')
[ "$OBJ_DECLARED" = "Hits" ] \
    || { echo "the object class was not in the stored contract (got '$OBJ_DECLARED')"; exit 1; }
DB_DECLARED=$(curl -sf "$API/revisions/$REV_ID" -H "$AUTH" | jq -r '.scriptConfig.capabilities.databases[0]')
[ "$DB_DECLARED" = "main=migrations/main" ] \
    || { echo "the database was not in the stored contract (got '$DB_DECLARED')"; exit 1; }
LIFE_DECLARED=$(curl -sf "$API/revisions/$REV_ID" -H "$AUTH" | jq -r '.scriptConfig.capabilities.lifecycle | join(",")')
[ "$LIFE_DECLARED" = "Visit:expire=10s,Visit:admit" ] \
    || { echo "the lifecycle was not in the stored contract (got '$LIFE_DECLARED')"; exit 1; }
CONN_DECLARED=$(curl -sf "$API/revisions/$REV_ID" -H "$AUTH" | jq -r '.scriptConfig.capabilities.connections | join(",")')
[ "$CONN_DECLARED" = "Live,Pulse" ] \
    || { echo "the connection classes were not in the stored contract (got '$CONN_DECLARED')"; exit 1; }
D1=$(curl -sf "$WORKER/$IDENT/" | jq .db_rows)
D2=$(curl -sf "$WORKER/$IDENT/" | jq .db_rows)
[ "$D2" -gt "$D1" ] 2>/dev/null \
    || { echo "database rows did not accumulate across requests ($D1 -> $D2)"; exit 1; }
echo "database counted $D1 then $D2 across requests"
echo "object counted $H1 then $H2; contract declares class $OBJ_DECLARED"

# Arm the watchdog now: its 15s alarm outlives the restart below, and
# nothing after this line touches AlarmKeeper again.
curl -sf "$WORKER/$IDENT/arm" -o /dev/null

echo "== object rows survive a worker restart"
# The counter lives in the object's own sqlite file on a volume; kill the
# worker and the next bump must continue the same count, not restart it.
compose restart worker_service >/dev/null 2>&1
H3=""
for _ in $(seq 1 30); do
    H3=$(curl -sf "$WORKER/$IDENT/" | jq .hits 2>/dev/null) && [ -n "$H3" ] && break
    sleep 2
done
[ -n "$H3" ] && [ "$H3" -gt "$H2" ] 2>/dev/null \
    || { echo "object state did not survive the restart ($H2 -> '$H3')"; exit 1; }
D3=$(curl -sf "$WORKER/$IDENT/" | jq .db_rows)
[ -n "$D3" ] && [ "$D3" -gt "$D2" ] 2>/dev/null \
    || { echo "the migrated database broke across the restart ($D2 -> '$D3'), migrations may have reapplied"; exit 1; }

echo "== cold alarm fires through the sweep"
# The watchdog was armed before the restart and nothing touches it after;
# the sweep must revive it (once the dead node's lease ages out) and its
# alarm writes the mark we poll for.
MARKS=0
for _ in $(seq 1 30); do
    MARKS=$(curl -sf "$WORKER/$IDENT/" | jq .marks 2>/dev/null || echo 0)
    [ "$MARKS" -ge 1 ] 2>/dev/null && break
    sleep 2
done
[ "$MARKS" -ge 1 ] 2>/dev/null \
    || { echo "the cold alarm never fired (marks: '$MARKS')"; exit 1; }
echo "cold alarm fired via the sweep; $MARKS mark(s) written"

echo "== lifecycle: destroy from inside, recreate fresh, expire, refuse"
# The directory speaks for every residue location the platform keeps in
# sql; the epoch fence is the ONE row deletion leaves on purpose.
pg() { compose exec -T postgres psql -U actias -d actias_script_service -tA -c "$1"; }

T1=$(curl -sf "$WORKER/$IDENT/visit?name=roomy" | jq .touched)
T2=$(curl -sf "$WORKER/$IDENT/visit?name=roomy" | jq .touched)
[ "$T2" = 2 ] 2>/dev/null \
    || { echo "the store-face counter did not accumulate ($T1 -> '$T2')"; exit 1; }
VISIT_HASH=$(pg "SELECT object_id FROM object_instances WHERE class='Visit' AND name='roomy'")
[ -n "$VISIT_HASH" ] || { echo "the directory never learned the identity hash"; exit 1; }
EPOCH_BEFORE=$(pg "SELECT COALESCE(MAX(epoch),0) FROM object_epochs WHERE object_id='$VISIT_HASH'")

# Destroy answers first: the reply carries the state the object died with.
FIN=$(curl -sf "$WORKER/$IDENT/visit?name=roomy&act=finish" | jq .finished)
[ "$FIN" = 2 ] 2>/dev/null \
    || { echo "destroy did not answer with the final state (got '$FIN')"; exit 1; }

# The janitor finishes within a sweep: directory, lease and alarm rows
# empty, and only the epoch fence outlives the object, bumped.
LEFT=1
for _ in $(seq 1 20); do
    LEFT=$(pg "SELECT count(*) FROM object_instances WHERE class='Visit' AND name='roomy'")
    [ "$LEFT" = 0 ] && break
    sleep 2
done
[ "$LEFT" = 0 ] || { echo "the destroyed instance never left the directory"; exit 1; }
[ "$(pg "SELECT count(*) FROM leases WHERE object_id='$VISIT_HASH'")" = 0 ] \
    || { echo "a lease outlived the object"; exit 1; }
[ "$(pg "SELECT count(*) FROM object_alarms WHERE object_id='$VISIT_HASH'")" = 0 ] \
    || { echo "an alarm outlived the object"; exit 1; }
EPOCH_AFTER=$(pg "SELECT COALESCE(MAX(epoch),0) FROM object_epochs WHERE object_id='$VISIT_HASH'")
[ "$EPOCH_AFTER" -gt "$EPOCH_BEFORE" ] 2>/dev/null \
    || { echo "deletion did not bump the epoch fence ($EPOCH_BEFORE -> '$EPOCH_AFTER')"; exit 1; }

# The name is legal again and starts fresh: forget, never a ban.
T3=$(curl -sf "$WORKER/$IDENT/visit?name=roomy" | jq .touched)
[ "$T3" = 1 ] 2>/dev/null \
    || { echo "recreation did not start fresh (touched '$T3')"; exit 1; }

# An untouched instance ages out through the sweep (expire=10s, sweep=3s).
curl -sf "$WORKER/$IDENT/visit?name=brief" -o /dev/null
SWEPT=1
for _ in $(seq 1 20); do
    SWEPT=$(pg "SELECT count(*) FROM object_instances WHERE class='Visit' AND name='brief'")
    [ "$SWEPT" = 0 ] && break
    sleep 2
done
[ "$SWEPT" = 0 ] || { echo "the untouched instance never expired"; exit 1; }

# A refused name leaves nothing at all: no row, no epoch fence.
EPOCHS_TOTAL=$(pg "SELECT count(*) FROM object_epochs")
REFUSED_CODE=$(curl -s -o /dev/null -w '%{http_code}' "$WORKER/$IDENT/visit?name=zz")
[ "$REFUSED_CODE" != 200 ] \
    || { echo "the admission gate admitted a two-letter name"; exit 1; }
[ "$(pg "SELECT count(*) FROM object_instances WHERE class='Visit' AND name='zz'")" = 0 ] \
    || { echo "a refused name reached the directory"; exit 1; }
[ "$(pg "SELECT count(*) FROM object_epochs")" = "$EPOCHS_TOTAL" ] \
    || { echo "a refused name left an epoch fence"; exit 1; }
echo "lifecycle round-tripped: destroy answered $FIN, epoch $EPOCH_BEFORE -> $EPOCH_AFTER, recreation fresh, expiry swept, refusal residue-free"

echo "== a connection hibernates with the socket open and revives on delivery"
# The whole arc's claim in one sequence: declared handlers run, the vm
# falls while the socket stays open (the gauges say so), a delivery
# revives it with conn.state intact, and closing severs the edges.
WORKER_URL="$WORKER" IDENT="$IDENT" node -e '
const http = require("http");
const base = process.env.WORKER_URL.replace("http://", "");
const [host, port] = base.split(":");
const fail = (why) => { console.error("connection smoke: " + why); process.exit(1); };
setTimeout(() => fail("timeout"), 30000);
const metric = (name) => new Promise((res) => {
  http.get({ host, port, path: "/_metrics" }, (r) => {
    let b = ""; r.on("data", (c) => b += c);
    r.on("end", () => {
      const line = b.split("\n").find((l) => l.startsWith(name + " "));
      res(line ? Number(line.split(" ")[1]) : NaN);
    });
  }).on("error", () => res(NaN));
});
const ws = new WebSocket("ws://" + base + "/" + process.env.IDENT + "/live");
ws.addEventListener("message", async (event) => {
  const msg = JSON.parse(String(event.data));
  if (msg.kind === "hello") {
    ws.send(JSON.stringify({ nudge: true }));
  } else if (msg.kind === "echo") {
    if (msg.n !== 1) fail("echo count wrong: " + msg.n);
    await new Promise((r) => setTimeout(r, 5500));
    const hib = await metric("actias_connections_hibernated");
    const warm = await metric("actias_connections_warm");
    if (!(hib >= 1)) fail("the vm never hibernated (gauge " + hib + ")");
    if (warm !== 0) fail("a vm survived the idle threshold (warm " + warm + ")");
    http.get({ host, port, path: "/" + process.env.IDENT + "/poke" }, () => {});
  } else if (msg.kind === "poked") {
    if (msg.frames !== 1) fail("conn.state did not survive hibernation: " + msg.frames);
    const wakes = await metric("actias_connection_wakes_total");
    if (!(wakes >= 1)) fail("the wake was not counted: " + wakes);
    console.log("hibernated with the socket open; delivery revived with state intact; wakes=" + wakes);
    ws.close();
    process.exit(0);
  }
});
ws.addEventListener("error", () => fail("socket error"));
' || { echo "the connection checkpoint failed"; exit 1; }

# Closing severed the edges: the publisher-side follower table drains.
EDGES=1
for _ in $(seq 1 20); do
    EDGES=$(curl -sf "$API/project/$PROJECT_ID/objects/Hits/global/followers" -H "$AUTH" | jq '.edges | length')
    [ "$EDGES" = 0 ] && break
    sleep 2
done
[ "$EDGES" = 0 ] || { echo "the closed connection left $EDGES edge(s)"; exit 1; }
echo "close severed the edges; the follower table is empty"

echo "== a timered connection beats and stays warm"
WORKER_URL="$WORKER" IDENT="$IDENT" node -e '
const base = process.env.WORKER_URL.replace("http://", "");
const fail = (why) => { console.error("timer smoke: " + why); process.exit(1); };
setTimeout(() => fail("timeout"), 15000);
const ws = new WebSocket("ws://" + base + "/" + process.env.IDENT + "/pulse");
ws.addEventListener("message", (event) => {
  const msg = JSON.parse(String(event.data));
  if (msg.kind !== "beat") return;
  if (msg.n === 2) {
    if (msg.missed !== 0) fail("an idle timer missed ticks: " + msg.missed);
    console.log("two beats on schedule across the hibernate threshold");
    ws.close();
    process.exit(0);
  }
});
ws.addEventListener("error", () => fail("socket error"));
' || { echo "the timer checkpoint failed"; exit 1; }

echo "== cron handler runs on schedule"
# Armed at the script's first touch, firing every two seconds since; two
# consecutive reads must show the count still climbing.
C1=$(curl -sf "$WORKER/$IDENT/" | jq .crons)
sleep 5
C2=$(curl -sf "$WORKER/$IDENT/" | jq .crons)
[ -n "$C1" ] && [ "$C2" -gt "$C1" ] 2>/dev/null \
    || { echo "cron did not keep firing ($C1 -> '$C2')"; exit 1; }
echo "cron fired: $C1 then $C2 marks"

echo "== queue delivers to its consumer"
# Three sends through the producer handle; the queue object's alarm loop
# delivers each to the listener, whose receipts the poll counts.
Q_DECLARED=$(curl -sf "$API/revisions/$REV_ID" -H "$AUTH" | jq -r '.scriptConfig.capabilities.queues[0]')
[ "$Q_DECLARED" = "jobs" ] \
    || { echo "the queue was not in the stored contract (got '$Q_DECLARED')"; exit 1; }
curl -sf "$WORKER/$IDENT/enqueue" -o /dev/null
curl -sf "$WORKER/$IDENT/enqueue" -o /dev/null
curl -sf "$WORKER/$IDENT/enqueue" -o /dev/null
QN=0
for _ in $(seq 1 15); do
    QN=$(curl -sf "$WORKER/$IDENT/" | jq .queued 2>/dev/null || echo 0)
    [ "$QN" -ge 3 ] 2>/dev/null && break
    sleep 1
done
[ "$QN" -ge 3 ] 2>/dev/null \
    || { echo "queue deliveries did not land ($QN of 3)"; exit 1; }
echo "queue delivered $QN message(s) to the consumer"

echo "== a second script produces into the same project-scoped queue"
# Identity is (project, class, name): a sibling script declaring
# `queue "jobs"` reaches the SAME queue, and its send is delivered by the
# consumer script's code. A second consumer would be refused at publish.
PRODUCER_IDENT="smokeprod$SUFFIX"
PRODUCER_ID=$(curl -sf -X POST "$API/project/$PROJECT_ID/scripts" -H "$AUTH" -H 'Content-Type: application/json' \
    -d "{\"publicIdentifier\":\"$PRODUCER_IDENT\"}" | jq -r .id)
mkdir -p "$DEVDIR/producer"
cat > "$DEVDIR/producer/script.json" <<EOF
{"id":"$PRODUCER_ID","entryPoint":"main.lua","includes":["**/*.lua"],"ignore":[]}
EOF
cat > "$DEVDIR/producer/main.lua" <<'LUA'
local jobs = queue "jobs"

on "fetch" (function(request)
    jobs:send({ n = 99 })
    return { body = "sent", headers = {} }
end)
LUA
"$REPO/target/debug/actias" publish "$DEVDIR/producer"
curl -sf "$WORKER/$PRODUCER_IDENT/" -o /dev/null
QN2=0
for _ in $(seq 1 15); do
    QN2=$(curl -sf "$WORKER/$IDENT/" | jq .queued 2>/dev/null || echo 0)
    [ "$QN2" -ge 4 ] 2>/dev/null && break
    sleep 1
done
[ "$QN2" -ge 4 ] 2>/dev/null \
    || { echo "the sibling producer's message did not reach the consumer ($QN2 of 4)"; exit 1; }
echo "sibling script produced into the shared queue; consumer count $QN2"

echo "== dashboard resources speak for the platform's own storage"
# The union listing knows the queue and database from the contract; the
# stats and tables come off the worker's sqlite through the api proxy;
# the console reads through the same transport scripts use. Identity is
# the name alone, scoped to the project.
RQ=$(curl -sf "$API/project/$PROJECT_ID/queues" -H "$AUTH")
echo "$RQ" | jq -e '.[0].name == "jobs" and .[0].orphaned == false and (.[0].declaredBy | length) > 0' >/dev/null \
    || { echo "queue listing did not surface the contract queue: $RQ"; exit 1; }
RD=$(curl -sf "$API/project/$PROJECT_ID/databases" -H "$AUTH")
echo "$RD" | jq -e 'map(.name) | index("main") != null' >/dev/null \
    || { echo "database listing missed main: $RD"; exit 1; }
QST=$(curl -sf "$API/project/$PROJECT_ID/queues/jobs/stats" -H "$AUTH")
echo "$QST" | jq -e '.depth >= 0 and .deadLetters >= 0' >/dev/null \
    || { echo "queue stats did not read: $QST"; exit 1; }
TBL=$(curl -sf "$API/project/$PROJECT_ID/databases/main/overview" -H "$AUTH")
echo "$TBL" | jq -e '(.tables | map(.name) | index("visits") != null) and .sizeBytes > 0' >/dev/null \
    || { echo "database overview missed visits or its size: $TBL"; exit 1; }
CONSOLE=$(curl -sf -X POST "$API/project/$PROJECT_ID/databases/main/query" \
    -H "$AUTH" -H 'Content-Type: application/json' \
    -d '{"sql":"SELECT COUNT(*) AS n FROM visits"}')
# Rows come back as objects; indexing one with a number aborts jq, so
# the shape probe uses `?` instead of `or`.
echo "$CONSOLE" | jq -e '(.rows[0].n? // .rows[0][0].n? // 0) >= 1' >/dev/null \
    || { echo "the query console did not read visits: $CONSOLE"; exit 1; }
echo "resources listing, stats, tables and console all answered"

echo "== object state survives losing the data volume"
# The disk is a leased cache; the blob store is the truth. Wipe every
# object file while the worker is down, and the counters must continue
# from their shipped snapshots, not restart from zero.
HB=$(curl -sf "$WORKER/$IDENT/" | jq .hits)
compose stop worker_service >/dev/null 2>&1
docker run --rm -v "${PROJECT}_objects_data:/d" busybox sh -c 'rm -rf /d/*' >/dev/null 2>&1
compose start worker_service >/dev/null 2>&1
HV=""
for _ in $(seq 1 30); do
    HV=$(curl -sf "$WORKER/$IDENT/" | jq .hits 2>/dev/null) && [ -n "$HV" ] && break
    sleep 2
done
[ -n "$HV" ] && [ "$HV" -gt "$HB" ] 2>/dev/null \
    || { echo "object state did not survive the volume loss ($HB -> '$HV')"; exit 1; }
echo "counter continued at $HV after the volume was wiped"

echo "== two nodes serve one object"
# The object is homed on worker one; a request through worker two must
# forward its calls there and continue the same count.
HA=$(curl -sf "$WORKER/$IDENT/" | jq .hits)
HB=$(curl -sf "$WORKER2/$IDENT/" | jq .hits)
[ -n "$HB" ] && [ "$HB" -gt "$HA" ] 2>/dev/null \
    || { echo "the second node did not continue the count ($HA -> '$HB'); forwarding broke"; exit 1; }
echo "worker two forwarded into the same object: $HA then $HB"

echo "== the survivor takes over when the holder dies"
# Kill the holder outright. Once its lease ages out, the survivor claims,
# restores the shipped snapshot onto its own empty volume, and the count
# continues; failover and rehoming are the same code path.
compose stop worker_service >/dev/null 2>&1
HC=""
for _ in $(seq 1 40); do
    HC=$(curl -sf "$WORKER2/$IDENT/" | jq .hits 2>/dev/null) || { sleep 2; continue; }
    [ -n "$HC" ] && [ "$HC" -gt "$HB" ] 2>/dev/null && break
    sleep 2
done
[ -n "$HC" ] && [ "$HC" -gt "$HB" ] 2>/dev/null \
    || { echo "the survivor never took the object over ($HB -> '$HC')"; exit 1; }
echo "survivor took over and continued: $HC"

# The old holder returns as a stranger: its stale local file must lose to
# the lease, so its requests forward to the new home.
compose start worker_service >/dev/null 2>&1
HD=""
for _ in $(seq 1 30); do
    HD=$(curl -sf "$WORKER/$IDENT/" | jq .hits 2>/dev/null) || { sleep 2; continue; }
    [ -n "$HD" ] && [ "$HD" -gt "$HC" ] 2>/dev/null && break
    sleep 2
done
[ -n "$HD" ] && [ "$HD" -gt "$HC" ] 2>/dev/null \
    || { echo "the returned node did not forward to the new holder ($HC -> '$HD')"; exit 1; }
echo "returned node forwards to the new home: $HD"

echo "== reads on the non-holder serve from the replica"
# The object now lives on worker two; worker one's db reads must answer
# locally from a restored snapshot, never entering the owner's mailbox.
# The request above already exercised them; the counter is the witness.
REPLICA_READS=$(curl -sf "$WORKER/_metrics" | sed -n 's/^actias_replica_reads_total //p')
[ -n "$REPLICA_READS" ] && [ "$REPLICA_READS" -ge 1 ] 2>/dev/null \
    || { echo "the non-holder served no replica reads (got '$REPLICA_READS')"; exit 1; }
echo "worker one answered $REPLICA_READS read(s) from its replica"

echo "object resumed at $H3 after the worker restart"

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
# Version two keeps every declaration: objects run the owner's CURRENT
# revision whoever calls them, so the old revision's preview handler
# still reaches Hits/AlarmKeeper/jobs through this code. Dropping a
# class from the current revision correctly breaks calls to it, which is
# its own (deliberate) platform behavior, not this section's subject.
cat > "$DEVDIR/published/main.lua" <<'LUA'
local ns = kv "smoke"
local token = secret "smoke-token"
local db = database "main" { migrations = "migrations/main" }

local Hits = object "Hits" {
    bump = function(state)
        state.sql:exec("CREATE TABLE IF NOT EXISTS hits (at INTEGER)")
        state.sql:exec("INSERT INTO hits VALUES (?)", { 1 })
        return state.sql:query_one("SELECT COUNT(*) AS n FROM hits").n
    end,
}

local AlarmKeeper = object "AlarmKeeper" {
    arm = function(state, duration)
        state:set_alarm(duration)
        return true
    end,
    alarm = function(state)
        db:exec("INSERT INTO alarm_marks VALUES (?)", { state.now() })
    end,
}

on "cron:*/2 * * * * *" (function(event)
    db:exec("INSERT INTO cron_marks VALUES (?)", { event.scheduled_at })
end)

local jobs = queue "jobs"
on "queue:jobs" (function(message)
    db:exec("INSERT INTO queue_done VALUES (?)", { message.n })
end)

on "fetch" (function(request)
    log.info("hello from production")
    return { body = "version two" }
end)
LUA
"$REPO/target/debug/actias" publish "$DEVDIR/published"

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
"$REPO/target/debug/actias" alias "$SCRIPT_ID" set staging "$OLD_REV"

ALIASED=$(curl -sf "$WORKER/_alias/$IDENT/staging/")
echo "$ALIASED" | jq -e '.ok == true' >/dev/null \
    || { echo "the alias path form did not serve the old revision: $ALIASED"; exit 1; }
HOST_ALIASED=$(curl -sf -H "Host: $IDENT--staging.scripts.localhost" "$WORKER/")
echo "$HOST_ALIASED" | jq -e '.ok == true' >/dev/null \
    || { echo "the alias subdomain did not serve the old revision: $HOST_ALIASED"; exit 1; }

"$REPO/target/debug/actias" alias "$SCRIPT_ID" list | grep -q "staging" \
    || { echo "alias list did not show staging"; exit 1; }

# Moving the alias is the rollback primitive; the pointer ttl bounds how
# long the old target keeps serving.
CURRENT_REV=$(curl -sf "$API/script/$SCRIPT_ID" -H "$AUTH" | jq -r .currentRevisionId)
"$REPO/target/debug/actias" alias "$SCRIPT_ID" set staging "$CURRENT_REV"
MOVED=""
for _ in $(seq 1 30); do
    MOVED=$(curl -sf -H "Host: $IDENT--staging.scripts.localhost" "$WORKER/" || true)
    [ "$MOVED" = "version two" ] && break
    sleep 2
done
[ "$MOVED" = "version two" ] || { echo "the moved alias never served the new revision (got '$MOVED')"; exit 1; }
echo "staging alias served the old revision, then the new one after the move"

echo "== service tokens (machine deploys)"
# A machine credential deploys like a member and dies by deletion: the
# secret is shown once, its hash is all the api keeps.
TOKEN_JSON=$(curl -sf -X POST "$API/project/$PROJECT_ID/tokens" -H "$AUTH" \
    -H 'Content-Type: application/json' -d '{"name":"smoke deploy"}')
SVC_TOKEN=$(echo "$TOKEN_JSON" | jq -r .token)
SVC_ID=$(echo "$TOKEN_JSON" | jq -r .id)
[ -n "$SVC_TOKEN" ] && [ "$SVC_TOKEN" != "null" ] \
    || { echo "token creation returned no secret: $TOKEN_JSON"; exit 1; }

# The CLI authenticates with whatever bearer its settings carry; hand it
# the machine token instead of the user session.
MACHINE_CONFIG="$DEVDIR/machine-config"
mkdir -p "$MACHINE_CONFIG/actias-cli"
printf '{"apiUrl":"http://127.0.0.1:%s","token":"%s"}' "$ACTIAS_API_PORT" "$SVC_TOKEN" \
    > "$MACHINE_CONFIG/actias-cli/settings.json"

XDG_CONFIG_HOME="$MACHINE_CONFIG" "$REPO/target/debug/actias" publish "$DEVDIR/published" \
    || { echo "a service token could not publish"; exit 1; }

curl -sf -X DELETE "$API/project/$PROJECT_ID/tokens/$SVC_ID" -H "$AUTH" >/dev/null
if XDG_CONFIG_HOME="$MACHINE_CONFIG" "$REPO/target/debug/actias" publish "$DEVDIR/published" >/dev/null 2>&1; then
    echo "a revoked token still published"
    exit 1
fi
echo "service token published; revoked token refused"

echo "== local tests (actias test)"
# Fully local: the template's shipped test runs on the embedded runtime
# with in-memory fakes, no stack and no login involved.
UNIT="$DEVDIR/unit"
mkdir -p "$UNIT"
cp -r "$REPO/actias-cli/template/templates/basic/." "$UNIT/"
"$REPO/target/debug/actias" test "$UNIT" \
    || { echo "the template's shipped test failed"; exit 1; }
"$REPO/target/debug/actias" check "$UNIT" \
    || { echo "the template failed actias check"; exit 1; }
echo "template test and typed check passed on the local runtime"

echo "== worker metrics expose the traffic"
curl -sf "$WORKER/_metrics" | grep -q "actias_requests_total{.*script=\"$IDENT\"}" \
    || { echo "the script's requests are missing from /_metrics"; exit 1; }
echo "metrics show $IDENT traffic"

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
(cd "$DEVDIR" && exec "$REPO/target/debug/actias" dev project --worker-url "$WORKER" > "$DEV_LOG" 2>&1) &
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
"$REPO/target/debug/actias" tail "$SCRIPT_ID" > "$TAIL_LOG" 2>&1 &
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

echo "== websocket tail authenticates via query token (browser path)"
# Browsers cannot set upgrade headers; the dashboard's live tail rides a
# ?token= query instead. Prove the whole handshake: ready, tail, tailing.
WS_URL="ws://127.0.0.1:$ACTIAS_API_PORT/liveScript?token=$TOKEN" SID="$SCRIPT_ID" \
node -e '
const ws = new WebSocket(process.env.WS_URL);
let ok = false;
ws.addEventListener("message", (event) => {
  const message = JSON.parse(String(event.data));
  if (message.status === "ready")
    ws.send(JSON.stringify({ event: "tail", data: { scriptId: process.env.SID } }));
  if (message.status === "tailing") { ok = true; ws.close(); }
});
ws.addEventListener("close", () => process.exit(ok ? 0 : 1));
ws.addEventListener("error", () => process.exit(1));
setTimeout(() => process.exit(1), 10000);
' || { echo "query-token websocket tail failed"; exit 1; }
echo "browser tail handshake completed"

echo "== playground protocol round-trips (browser live session)"
# The workbench page speaks exactly this: start a session with a base64
# bundle over the query-token socket, then the live url serves it. The
# old standalone playground page folded into the workbench.
curl -sf "$WEB/script/shell/workbench" -o /dev/null \
    || { echo "the workbench page did not render"; exit 1; }
PLAY_SESSION=$(WS_URL="ws://127.0.0.1:$ACTIAS_API_PORT/liveScript?token=$TOKEN" SID="$SCRIPT_ID" \
node -e '
const ws = new WebSocket(process.env.WS_URL);
const source = Buffer.from(
  `on "fetch" (function(r) return { body = "from the playground" } end)`
).toString("base64");
const payload = {
  scriptId: process.env.SID,
  revision: {
    scriptConfig: { id: process.env.SID, entryPoint: "main.lua", includes: ["**/*.lua"], ignore: [] },
    bundle: { entryPoint: "main.lua", files: [{ filePath: "main.lua", content: source }] },
  },
};
ws.addEventListener("message", (event) => {
  const message = JSON.parse(String(event.data));
  if (message.status === "ready") ws.send(JSON.stringify({ event: "start", data: payload }));
  if (message.status === "created") { console.log(message.sessionId); process.exit(0); }
});
ws.addEventListener("error", () => process.exit(1));
setTimeout(() => process.exit(1), 10000);
') || { echo "the playground session did not start"; exit 1; }
PLAY_BODY=$(curl -sf "$WORKER/_live/$IDENT/$PLAY_SESSION/")
[ "$PLAY_BODY" = "from the playground" ] \
    || { echo "the playground session did not serve (got '$PLAY_BODY')"; exit 1; }
echo "playground session served: $PLAY_BODY"

echo "== regenerating clients against the live api (drift coverage)"
# Both artifacts regenerate against THIS stack's api, not whatever sits
# on the default port; the spec pipes through jq because that is the
# checked-in snapshot's canonical formatting.
( cd actias-web && OPENAPI_URL="$API/docs/openapi.json" npm run generateClient >/dev/null 2>&1 )
curl -sf "$API/docs/openapi.json" | jq . > actias-cli/src/actias-api.json

if ! git diff --quiet -- actias-web/src/client actias-cli/src/actias-api.json; then
    echo "generated clients are out of date with the api:"
    git --no-pager diff --stat -- actias-web/src/client actias-cli/src/actias-api.json
    exit 1
fi

FAILED=0
echo "SMOKE TEST PASSED"
