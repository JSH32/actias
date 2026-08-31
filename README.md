<p align="center">
	<picture>
		<source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/JSH32/actias/master/.github/assets/banner-dark.png">
		<img width="460" alt="Actias" src="https://raw.githubusercontent.com/JSH32/actias/master/.github/assets/banner-light.png">
	</picture><br>
	<img src="https://img.shields.io/badge/license-AGPL--3.0%20%7C%20MIT-a3e6b4.svg">
	<img src="https://img.shields.io/badge/contributions-welcome-orange.svg">
	<img src="https://img.shields.io/badge/Made%20with-%E2%9D%A4-ff69b4?logo=love">
</p>

# Actias

> **Under construction.** Actias is in heavy development: surfaces
> change without notice, nothing is deployed anywhere yet, and proper
> documentation is still being written. Poke around, but do not build
> on it today.

Actias is an open-source serverless platform for Luau. You publish a
script; the platform runs it across a fleet and gives it durable
building blocks. Code is the manifest: what a script declares is what
it may touch, and the console can show all of it.

```lua
local visits = kv "visits"

on "fetch" (function(request)
    local count = (visits:get("count") or 0) + 1
    visits:set("count", count)
    return {
        headers = { ["content-type"] = "application/json" },
        body = json.stringify({ hello = "actias", visits = count }),
    }
end)
```

Actias is for both people learning to ship server-side code and
developers running real infrastructure. The same platform serves both,
so nothing here is a toy version of something else.

## The shape of it

Durable objects are the spine: named entities that take one call at a
time, own their own SQLite file, and publish streams other objects and
browsers can follow.

```lua
-- Identity-only: connections speak AS a Viewer; the class is free
-- until someone calls it.
local Viewer = object "Viewer" {}

local Auction = object "Auction" {
    migrations = "migrations/Auction",
    publishes = { bids = "public" },

    bid = function(state, user, amount)
        local high = state.sql:query_one("SELECT MAX(amount) AS amount FROM bids")
        if high.amount and amount <= high.amount then
            return { ok = false, refusal = "does not beat " .. high.amount }
        end
        state.sql:exec("INSERT INTO bids (user, amount) VALUES (?, ?)",
            { user, amount })
        state:publish("bids", { user = user, amount = amount })
        return { ok = true }
    end,
}

on "fetch" (function(request)
    local lot = request.query.lot
    if request.upgrade then
        -- A live bid feed: the socket follows the auction's stream.
        return request:upgrade(function(sock)
            sock:follow(Auction(lot), "bids")
            sock:each(function(item) sock:send(item.event.data) end)
        end, Viewer(request.query.user or "anon"))
    end
    local body = json.parse(request.body)
    return { body = json.stringify(Auction(lot):bid(body.user, body.amount)) }
end)
```

Two simultaneous bids are fair because the instance is single-writer:
one mailbox, one call at a time, no locks in user code. Names scope to
the project, so any script in it reaches the same `Auction("lot-42")`.

## What works today

- **HTTP handlers** with a typed request (path, query, headers, body)
  and static assets served straight from the bundle
- **Key-value storage**
- **SQL databases** with CLI-generated migrations, applied at first touch
- **Durable objects**: one instance, one writer, its own SQLite file,
  alarms it arms itself, methods callable from any script in the project
- **Queues** with retries, dead letters, and console-driven requeue
- **Workflows**: journaled and replayable, steps with retries, signals,
  race/all, runs that park for days and survive restarts
- **Streams**: publisher-gated pub/sub between objects, delivered to
  browsers over websockets with server-controlled connection programs
- **Versioned secrets** with rotation, and **cron schedules**
- **A typed Luau surface**: `actias check` and shipped luau-lsp
  definitions read the same declarations
- **A browser workbench**: the full editor with Luau's own analyzer
  compiled to wasm (diagnostics, completions, hover, cross-file jumps),
  editing a live session that serves at a url while you type
- **A web console** that inspects all of it live: object storage, queue
  journals, workflow runs, stream edges

## Running it

`docker-compose up -d` boots the whole stack locally (api, console,
workers, storage). The `actias` binary creates projects
(`actias init`), type-checks them (`actias check`, with luau-lsp
editor support wired by the generated project files), and publishes
them (`actias publish`). None of this is production-ready yet.

On kubernetes, one chart installs the same stack:

```sh
helm install actias oci://ghcr.io/jsh32/charts/actias \
  --set ingress.enabled=true \
  --set ingress.console=console.example.com \
  --set ingress.api=api.example.com \
  --set baseDomain=scripts.example.com \
  -f my-secrets.yaml
```

It brings its own postgres, redis and object storage for evaluation,
and points at external ones for anything real. See
[charts/actias](./charts/actias/README.md).

## How it is built

A Rust workspace does the serving: `actias-worker` runs scripts and
objects, `actias-script-service` owns revisions and contracts,
`actias-kv` stores keys, and they speak gRPC. `actias-api` (NestJS) is
the public REST gateway, and `actias-web` (Next.js) is the console and
workbench. `actias-cli` talks to the api and embeds the same Luau
analysis the workbench runs.

## Licensing

Actias is licensed under two licenses.

| Components | License |
| --- | --- |
| worker, worker-core, script-service, secret-service, kv, common, api, web | [AGPL-3.0-only](./LICENSE) |
| `actias-cli` and its Luau definitions, `actias-declarations`, `luau-web` | [MIT](./LICENSE-MIT) |

Modifying an AGPL component and offering it to others over a network
requires offering those users the corresponding source. AGPL section 13.

Lua and Luau you publish to Actias is not a derivative work of Actias,
in the same way a Python script is not a derivative of CPython. The
AGPL binds modification and hosting of Actias itself.

A commercial license is available. [NOTICE](./NOTICE) has the
component-by-component breakdown.
