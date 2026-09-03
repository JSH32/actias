#!/usr/bin/env bash
# The outbound-connection drill, on the compose stack: the assistant
# session from docs/OUTBOUND-CONNECTIONS.md against a fake provider
# that is itself an Actias script.
#
# Two scripts. The provider accepts an inbound connection and, per
# prompt frame, streams five token frames and a done frame; a prompt
# carrying `drop` makes it close the wire mid-answer instead. The
# assistant holds a Chat object with the history, one outbound wire to
# the provider (dialled with Upstream:open), and an inbound Session for
# the tab. A tab asks, sees the tokens and the done; asks with `drop`,
# sees `retry` and then a whole answer over a reopened wire. The
# console's listing must show both directions meanwhile.
#
# Workers must allow private egress for the dial to reach the peer
# worker on the compose network; the drill restarts them with it set
# and restores the default after.
#
#   ./scripts/outbound-drill.sh
set -euo pipefail

cd "$(dirname "$0")/.."

API="${API:-http://127.0.0.1:3001/api}"
OWNER="${OWNER:-http://127.0.0.1:3002}"
# The address the owner dials the provider at, inside the compose network.
PROVIDER_ADDR="${PROVIDER_ADDR:-worker_service_2:3000}"

login() {
    SUFFIX=$RANDOM
    curl -sf -X POST "$API/users" -H 'Content-Type: application/json' \
        -d "{\"username\":\"out$SUFFIX\",\"email\":\"out$SUFFIX@example.com\",\"password\":\"outbound-drill-password\"}" >/dev/null
    TOKEN=$(curl -sf -X POST "$API/auth/login" -H 'Content-Type: application/json' \
        -d "{\"auth\":\"out$SUFFIX\",\"password\":\"outbound-drill-password\"}" | jq -r .token)
    AUTH="Authorization: Bearer $TOKEN"
    PROJECT_ID=$(curl -sf -X POST "$API/project" -H "$AUTH" -H 'Content-Type: application/json' \
        -d '{"name":"outbound-drill"}' | jq -r .id)
    DIR=$(mktemp -d)
    export XDG_CONFIG_HOME="$DIR/config"
    mkdir -p "$XDG_CONFIG_HOME/actias"
    printf '{"apiUrl":"%s","token":"%s"}' "${API%/api}" "$TOKEN" \
        > "$XDG_CONFIG_HOME/actias/settings.json"
}

# publish <ident> <main.lua path>
publish() {
    local ident="$1" source="$2"
    local script_id
    script_id=$(curl -sf -X POST "$API/project/$PROJECT_ID/scripts" -H "$AUTH" \
        -H 'Content-Type: application/json' \
        -d "{\"publicIdentifier\":\"$ident\"}" | jq -r .id)
    local project="$DIR/$ident"
    mkdir -p "$project"
    cat > "$project/script.json" <<EOJ
{"id":"$script_id","entryPoint":"main.lua","includes":["**/*.lua"],"ignore":[]}
EOJ
    cp "$source" "$project/main.lua"
    ./target/debug/actias publish "$project" >/dev/null
}

login
cargo build -p actias-cli --quiet
PROVIDER="prov$SUFFIX"
ASSIST="asst$SUFFIX"

cat > "$DIR/provider.lua" <<'LUA'
-- A model provider, as a script: five tokens and a done per prompt, or
-- a dropped wire when the prompt says so.
local User = objects "User"

local Provider = connection "Provider" {
    frame = function(conn, data)
        if data.drop then
            conn:send({ type = "response.output_text.delta", response_id = data.id, delta = "half" })
            conn:close()
            return
        end
        for i = 1, 5 do
            conn:send({ type = "response.output_text.delta", response_id = data.id, delta = "t" .. i })
        end
        conn:send({ type = "response.done", response = { id = data.id } })
    end,
}

on "fetch" (function(request)
    if request.upgrade then
        return request:upgrade(Provider, {}, User("provider"))
    end
    return { status_code = 404 }
end)
LUA

cat > "$DIR/assistant.lua" <<LUA
local User = objects "User"

-- Declared ahead: the connection handlers below name Chat, and a local
-- declared after them would be a nil global when they run.
local Chat

local Session = connection "Session" {
    open = function(conn) conn:follow(Chat(conn.state.room), "tokens") end,
    frame = function(conn, data) Chat(conn.state.room):say(conn.name, data.text, data.drop) end,
    event = "forward",
}

local Upstream = connection "Upstream" {
    -- Following happens here, after whatever call opened the wire has
    -- returned; the object hears the wire is up and sends what waits.
    open = function(conn)
        conn:follow(Chat(conn.state.room), "upstream")
        Chat(conn.state.room):wire_up()
    end,
    event = function(conn, event) conn:send(event.data) end,
    frame = function(conn, data)
        local chat = Chat(conn.state.room)
        if data.type == "response.output_text.delta" then
            chat:token(data.response_id, data.delta)
        elseif data.type == "response.done" then
            chat:done(data.response.id)
        end
    end,
    close = function(conn) Chat(conn.state.room):upstream_closed(conn.closed) end,
}

-- Opens the upstream wire when none is recorded; the object calls one
-- thing at a time, so two calls cannot open two.
local function ensure_upstream(state)
    if state.store:get("upstream") then return end
    local name = Upstream:open("ws://$PROVIDER_ADDR/$PROVIDER/", { room = state.name })
    state.store:set("upstream", name)
end

Chat = object "Chat" {
    publishes = { tokens = "public", "upstream" },
    hooks = {
        follow = function(state, topic, follower)
            return topic == "tokens" or follower:is(Upstream)
        end,
        init = function(state)
            if state.store:get("pending") then state:set_alarm("1s") end
        end,
        alarm = function(state)
            local pending = state.store:get("pending")
            if not pending then return end
            state.store:delete("draft")
            -- The retry never asks for the drop again; the reopened
            -- wire sends it when it reports up.
            pending.drop = false
            state.store:set("pending", pending)
            state:publish("tokens", { retry = true })
            ensure_upstream(state)
        end,
    },
    say = function(state, user, text, drop)
        local n = (state.store:get("n") or 0) + 1
        state.store:set("n", n)
        state.store:set("pending", { id = "r" .. n, text = text, drop = drop or false })
        ensure_upstream(state)
        if state.store:get("wire_up") then
            state:publish("upstream", state.store:get("pending"))
        end
    end,
    -- The wire is following now; whatever waited goes up it.
    wire_up = function(state)
        state.store:set("wire_up", true)
        local pending = state.store:get("pending")
        if pending then state:publish("upstream", pending) end
    end,
    token = function(state, id, delta)
        local draft = state.store:get("draft") or { id = id, body = "" }
        draft.body = draft.body .. delta
        state.store:set("draft", draft)
        state:publish("tokens", { delta = delta })
    end,
    done = function(state, id)
        local draft = state.store:get("draft")
        state.store:delete("draft")
        state.store:delete("pending")
        state:publish("tokens", { done = true, body = draft and draft.body or "" })
    end,
    upstream_closed = function(state, why)
        state.store:delete("upstream")
        state.store:delete("wire_up")
        state:publish("tokens", { closed = why and why.by or "unknown", reason = why and why.reason or nil })
        if state.store:get("pending") then state:set_alarm("1s") end
    end,
}

on "fetch" (function(request)
    if request.upgrade then
        return request:upgrade(Session, { room = "lobby" }, User("tab"))
    end
    return { status_code = 404 }
end)
LUA

echo "== workers allow private egress for the run"
EGRESS_ALLOW_PRIVATE=true docker compose up -d worker_service worker_service_2 >/dev/null 2>&1
for _ in $(seq 1 60); do curl -sf -o /dev/null "$OWNER/_metrics" && break; sleep 1; done
# Both nodes register and see each other before anything is placed.
sleep "${SETTLE:-50}"

publish "$PROVIDER" "$DIR/provider.lua"
publish "$ASSIST" "$DIR/assistant.lua"

echo "== a tab asks, a wire is dialled, tokens stream back; then a drop and a retry"
WS_URL="ws://${OWNER#http://}/$ASSIST/" API="$API" AUTH_TOKEN="$TOKEN" PROJECT_ID="$PROJECT_ID" \
node -e '
const fail = (why) => { console.error("outbound drill: " + why); process.exit(1); };
setTimeout(() => fail("timeout"), 40000);
const ws = new WebSocket(process.env.WS_URL);
let phase = "first", tokens = 0, sawRetry = false, sawClosed = false;
const listConnections = async () => {
  const res = await fetch(process.env.API + "/project/" + process.env.PROJECT_ID + "/connections",
    { headers: { Authorization: "Bearer " + process.env.AUTH_TOKEN } });
  return res.json();
};
ws.addEventListener("open", () => { if (process.env.VERBOSE) console.log("   tab socket open"); ws.send(JSON.stringify({ text: "hello" })); });
ws.addEventListener("close", (event) => { if (process.env.VERBOSE) console.log("   tab socket closed", event.code, event.reason); });
ws.addEventListener("message", async (event) => {
  if (process.env.VERBOSE) console.log("   frame", String(event.data));
  const msg = JSON.parse(String(event.data));
  const data = msg.data || {};
  if (data.delta) tokens += 1;
  if (data.retry) sawRetry = true;
  if (data.closed) sawClosed = true;
  if (data.done) {
    if (phase === "first") {
      if (tokens !== 5) fail("first answer had " + tokens + " tokens");
      const rows = await listConnections();
      const inbound = rows.filter((r) => r.direction === "inbound").length;
      const outbound = rows.filter((r) => r.direction === "outbound");
      if (inbound < 2 || outbound.length !== 1) fail("listing: " + JSON.stringify(rows));
      if (!outbound[0].peer || outbound[0].connectionClass !== "Upstream") fail("outbound row: " + JSON.stringify(outbound[0]));
      console.log("   first answer: 5 tokens; listing shows " + inbound + " inbound, 1 outbound to " + outbound[0].peer);
      phase = "drop"; tokens = 0;
      ws.send(JSON.stringify({ text: "again", drop: true }));
    } else {
      if (!sawClosed) fail("the close hook never reported the drop");
      if (!sawRetry) fail("no retry was announced");
      if (tokens < 5) fail("the retried answer had " + tokens + " tokens");
      console.log("   dropped mid-answer: close hook said peer, retry announced, whole answer over a reopened wire");
      console.log("PASS: outbound wires open, report their end, and reopen under the object");
      ws.close();
      process.exit(0);
    }
  }
});
ws.addEventListener("error", () => fail("socket error"));
' || {
    if [ -n "${VERBOSE:-}" ]; then
        echo "== worker log tail"
        docker compose logs --since 3m worker_service worker_service_2 2>&1 \
            | grep -v "did not ship\|heartbeat\|_metrics" | grep -i "error\|warn\|ended\|denied" | cut -c1-400 | tail -12
    fi
    docker compose up -d worker_service worker_service_2 >/dev/null 2>&1
    rm -rf "$DIR"
    exit 1
}

docker compose up -d worker_service worker_service_2 >/dev/null 2>&1
rm -rf "$DIR"
