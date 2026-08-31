# Actias.Luau.js

The workbench's Luau language service: Luau's own analysis library
(release `0.735`) compiled to wasm from `luau-web/` at the repository
root. Vendored so the app builds hermetically; regenerate with
`nix develop -c ./build.sh` in that directory.

- sha256: `e5f090ddedbfbb7e4ba34d67f26ac93ed6a321be560e4d294813e2f8208ddb22`

The build runs the new solver, which is what `luau-analyze` defaults
to and therefore what `actias check` runs; the editor must never
contradict the check.

Plain C exports, reached through `ccall`; `checker.js` beside this
file is the worker that owns the module and speaks them. The module
holds the project: `setFile`/`removeFile` feed it, the query exports
take a module path into that state. Positions are one-based both ways;
line shifting for the shadow prologue is the caller's.

| Export | Returns |
| --- | --- |
| `setFile(name, text)` / `removeFile(name)` | nothing; marks the module dirty |
| `checkScript(module)` | JSON diagnostics with begin/end positions; lints included |
| `autocompleteScript(module, line, column)` | JSON entries `{name, kind, type?, wrongIndexType?, indexedWithSelf?}` |
| `hoverScript(module, line, column)` | `{"type": "..."}` or null |
| `definitionScript(module, line, column)` | `{module, line, column, endColumn}` or null |
| `signatureScript(module, line, column)` | `{parameters, active, returns?}` or null |
| `semanticScript(module)` | JSON `[line, column, length, type]` rows, zero-based |

## Updating

Bump `LUAU_TAG` in `luau-web/build.sh`, rebuild, replace this file's
hash. If the Luau analysis API drifted, `luau-web/Web.cpp` is the only
consumer that has to follow.
