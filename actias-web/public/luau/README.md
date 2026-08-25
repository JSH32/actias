# Actias.Luau.js

The workbench's Luau language service: Luau's own analysis library
(release `0.735`) compiled to wasm from `luau-web/` at the repository
root. Vendored so the app builds hermetically; regenerate with
`nix develop -c ./build.sh` in that directory.

- sha256: `cd3a16618127cdcefce63cd1811d55f3db4cffd8b9ee74edd70766d5c3cf31a9`

Three plain C exports, reached through `ccall`; `checker.js` beside this
file is the worker that owns the module and speaks them:

| Export | Returns |
| --- | --- |
| `checkScript(source, strict)` | JSON diagnostics with begin/end positions; lints included |
| `autocompleteScript(source, line, column, strict)` | JSON entries `{name, kind, type?}` |
| `hoverScript(source, line, column, strict)` | `{"type": "..."}` or null |

Positions are one-based both ways. The mode parameter exists because
`actias check` runs nonstrict by default and the editor must never
contradict it; upstream's own web build hardwires strict, which is why
it is not used here.

## Updating

Bump `LUAU_TAG` in `luau-web/build.sh`, rebuild, replace this file's
hash. If the Luau analysis API drifted, `luau-web/Web.cpp` is the only
consumer that has to follow.
