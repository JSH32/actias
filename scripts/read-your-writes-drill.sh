#!/usr/bin/env bash
# A request reads its own writes wherever it lands. A database is
# created and written through worker 1, so worker 1 owns it and worker
# 2 replicates it. Then one request on worker 2 writes a row and reads
# the count in the same handler: the read must see the row, which
# means it went to the owner rather than worker 2's replica copy.
#
#   ./scripts/read-your-writes-drill.sh
set -euo pipefail

cd "$(dirname "$0")/.."

API="${API:-http://127.0.0.1:3001/api}"
OWNER="${OWNER:-http://127.0.0.1:3002}"
PEER="${PEER:-http://127.0.0.1:3003}"
ROUNDS="${ROUNDS:-40}"

publish() {
    SUFFIX=$RANDOM
    curl -sf -X POST "$API/users" -H 'Content-Type: application/json' \
        -d "{\"username\":\"ryw$SUFFIX\",\"email\":\"ryw$SUFFIX@example.com\",\"password\":\"ryw-drill-password\"}" >/dev/null
    TOKEN=$(curl -sf -X POST "$API/auth/login" -H 'Content-Type: application/json' \
        -d "{\"auth\":\"ryw$SUFFIX\",\"password\":\"ryw-drill-password\"}" | jq -r .token)
    AUTH="Authorization: Bearer $TOKEN"
    PROJECT_ID=$(curl -sf -X POST "$API/project" -H "$AUTH" -H 'Content-Type: application/json' \
        -d '{"name":"ryw-drill"}' | jq -r .id)
    IDENT="ryw$SUFFIX"
    SCRIPT_ID=$(curl -sf -X POST "$API/project/$PROJECT_ID/scripts" -H "$AUTH" \
        -H 'Content-Type: application/json' \
        -d "{\"publicIdentifier\":\"$IDENT\"}" | jq -r .id)
    DIR=$(mktemp -d)
    export XDG_CONFIG_HOME="$DIR/config"
    mkdir -p "$XDG_CONFIG_HOME/actias" "$DIR/project/migrations/rows"
    printf '{"apiUrl":"%s","token":"%s"}' "${API%/api}" "$TOKEN" \
        > "$XDG_CONFIG_HOME/actias/settings.json"
    cat > "$DIR/project/script.json" <<EOJ
{"id":"$SCRIPT_ID","entryPoint":"main.lua","includes":["**/*.lua","**/*.sql"],"ignore":[]}
EOJ
    echo "CREATE TABLE rows (at INTEGER NOT NULL);" > "$DIR/project/migrations/rows/0001_rows.sql"
    cat > "$DIR/project/main.lua" <<'LUA'
local db = database "rows" { migrations = "migrations/rows" }

local function count()
    return db:read_one("SELECT COUNT(*) AS n FROM rows").n
end

on "fetch" (function(request)
    if request.path == "/write" then
        db:exec("INSERT INTO rows (at) VALUES (?)", { os.time() })
        return { body = json.stringify({ n = db:query_one("SELECT COUNT(*) AS n FROM rows").n }) }
    elseif request.path == "/write-then-read" then
        db:exec("INSERT INTO rows (at) VALUES (?)", { os.time() })
        return { body = json.stringify({ n = count() }) }
    end
    return { body = json.stringify({ n = count() }) }
end)
LUA
    cargo build -p actias-cli --quiet
    ./target/debug/actias publish "$DIR/project" >/dev/null
    rm -rf "$DIR"
    echo "$IDENT"
}

IDENT=$(publish)
echo "== published $IDENT"
first=$(curl -sf "$OWNER/$IDENT/write" | jq -r .n)
echo "   worker 1 wrote; owner count = $first"
second=$(curl -sf "$PEER/$IDENT/write-then-read" | jq -r .n)
echo "   worker 2 wrote then read in one request; read saw = $second (expected $((first + 1)))"
later=$(curl -sf "$PEER/$IDENT/read" | jq -r .n)
echo "   worker 2 read alone afterwards; saw = $later (from its replica copy, at most one flight behind)"
if [ "$second" -ne $((first + 1)) ]; then
    echo "FAIL: a request did not read its own write"
    exit 1
fi
echo "PASS: a request reads its own writes on a node that does not hold the object"

echo "== across requests: worker 1 writes, worker 2 reads at once, $ROUNDS rounds"
stale=0
for _ in $(seq 1 "${ROUNDS:-40}"); do
    n=$(curl -sf "$OWNER/$IDENT/write" | jq -r .n)
    seen=$(curl -sf "$PEER/$IDENT/read" | jq -r .n)
    if [ "$seen" -ne "$n" ]; then
        stale=$((stale + 1))
        echo "   stale: wrote $n, read $seen"
    fi
done
echo "   reads confirmed on the peer: $(curl -sf "$PEER/_metrics" | awk '$1=="actias_replica_reads_confirmed_total"{print $2}'), waited: $(curl -sf "$PEER/_metrics" | awk '$1=="actias_replica_reads_waited_total"{print $2}'), forwarded: $(curl -sf "$PEER/_metrics" | awk '$1=="actias_replica_reads_forwarded_total"{print $2}')"
if [ "$stale" -ne 0 ]; then
    echo "FAIL: $stale of ${ROUNDS:-40} reads on another node missed a write already answered"
    exit 1
fi
echo "PASS: every read on another node saw the write answered before it"
