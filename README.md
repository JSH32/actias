<p align="center">
	<img width="550" src="https://raw.githubusercontent.com/JSH32/actias/master/.github/assets/banner.png"><br>
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
building blocks: key-value storage, SQL databases with migrations,
single-writer durable objects with their own storage and alarms,
queues with retries and dead letters, replayable multi-day workflows,
and publish/subscribe streams that reach browsers over websockets.
Code is the manifest: what a script declares is what it may touch, and
the console can show all of it.

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

It exists to make deploying server-side code approachable: education
is the wedge, production scale is the ceiling, and neither audience is
sacrificed for the other.

## Running it

`docker-compose up -d` boots the whole stack locally (api, console,
workers, storage). The `actias-cli` binary creates projects
(`actias init`), type-checks them (`actias check`, with luau-lsp
editor support wired by the generated project files), and publishes
them (`actias publish`). None of this is production-ready yet.
