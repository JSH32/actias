# Actias task runner. Every verb here is what CI runs, so local and CI agree.
# Enter the toolchain with `nix develop` (or direnv) before running these.
#
# Codegen note: only `generate` runs offline and is gated by `ci`. The web api
# client and the CLI's OpenAPI snapshot are produced from a running api, so
# they live in `generate-clients` and stay outside the drift check for now.

# Show available tasks.
default:
    @just --list

# Install node dependencies for api and web from their lockfiles.
deps:
    cd actias-api && npm ci
    cd actias-web && npm ci

# Build everything: rust workspace, api, web.
build: build-rust build-api build-web

# Build the rust workspace (worker, kv, script-service, cli, common).
build-rust:
    cargo build --workspace

# Build the NestJS api.
build-api:
    cd actias-api && npm run build

# Build the Next.js web app.
build-web:
    cd actias-web && npm run build

# Rebuild the workbench's Luau wasm from luau-web and vendor it:
# artifact plus the sha in the README beside it. The dev shell carries
# the emscripten toolchain; output is byte-stable under its pin.
wasm:
    cd luau-web && emcmake cmake -B build -G Ninja -DCMAKE_BUILD_TYPE=Release > /dev/null && cmake --build build
    cp luau-web/build/Actias.Luau.js actias-web/public/luau/Actias.Luau.js
    sha=$(sha256sum actias-web/public/luau/Actias.Luau.js | cut -d' ' -f1) && \
        sed -i "s/sha256: \`[0-9a-f]*\`/sha256: \`$sha\`/" actias-web/public/luau/README.md
    @echo "vendored: $(du -h actias-web/public/luau/Actias.Luau.js | cut -f1)"

# Fail when the vendored wasm drifted from a fresh build of luau-web.
wasm-check:
    cd luau-web && emcmake cmake -B build -G Ninja -DCMAKE_BUILD_TYPE=Release > /dev/null && cmake --build build
    test "$(sha256sum luau-web/build/Actias.Luau.js | cut -d' ' -f1)" = "$(sha256sum actias-web/public/luau/Actias.Luau.js | cut -d' ' -f1)" || \
        (echo "Vendored Actias.Luau.js drifted from luau-web; run 'just wasm'." && exit 1)

# Run all tests. The rust suites need docker for testcontainers.
test: test-rust test-api

# Run the rust workspace tests.
test-rust:
    cargo test --workspace

# Run the api's jest suite.
test-api:
    cd actias-api && npm test

# Check formatting and lint everything. Never rewrites files.
lint: lint-rust lint-api lint-web

# rustfmt in check mode plus clippy with warnings denied.
lint-rust:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings

# eslint without --fix, so a violation fails instead of being rewritten.
lint-api:
    cd actias-api && npx eslint "{src,apps,libs,test}/**/*.ts"

# Lint the web app.
lint-web:
    cd actias-web && npx next lint

# Compiler-grade typecheck of the api, spec files included; `nest build` uses
# tsconfig.build.json, which excludes them.
typecheck-api:
    cd actias-api && npx tsc --noEmit

# Compiler-grade typecheck of the web app, everything the tsconfig includes.
typecheck-web:
    cd actias-web && npx tsc --noEmit

# Apply formatters in place.
fmt:
    cargo fmt --all
    cd actias-api && npx prettier --write "src/**/*.ts"
    cd actias-web && npx prettier --write "src/**/*.{ts,tsx}"

# Regenerate the api's protobuf types; the only codegen that runs offline.
generate:
    cd actias-api && npm run generateProto

# Regenerate the web api client; needs a running api, so `ci` cannot gate on it.
generate-clients:
    cd actias-web && npm run generateClient

# Fail if checked-in generated code differs from a fresh generation.
check-generated: generate
    #!/usr/bin/env bash
    set -euo pipefail
    # Two ways to be stale: regeneration modified a tracked file, or it produced a
    # file nobody added. Staged output counts as up to date, so a review-before-commit
    # workflow is not blocked; CI checks out clean, where the distinction cannot arise.
    untracked="$(git ls-files --others --exclude-standard -- actias-api/src/protobufs)"
    if ! git diff --quiet -- actias-api/src/protobufs || [ -n "$untracked" ]; then
        echo "generated protobuf sources are out of date, run 'just generate' and commit the result"
        git --no-pager diff -- actias-api/src/protobufs
        [ -n "$untracked" ] && echo "untracked generated files: $untracked"
        exit 1
    fi

# Bring the local stack up in the background.
up:
    docker compose up -d --build

# Full-stack smoke test: compose up, publish a script, request it through the
# worker, verify the checked-in clients match the live api. Needs docker.
smoke:
    ./scripts/smoke-test.sh

# Tear the local stack down.
down:
    docker compose down

# Follow logs for the whole stack, or one service: `just logs worker_service`.
logs service="":
    docker compose logs -f {{ service }}

# Copy the shared dashboards into the chart. A chart cannot read files
# above its own directory, so the copy is what lets one set of json
# serve both compose and helm; it is generated, gitignored, and every
# chart verb below runs this first.
chart-sync:
    rm -rf charts/actias/dashboards
    mkdir -p charts/actias/dashboards
    cp observability/dashboards/*.json charts/actias/dashboards/

# Lint the chart the way CI does. Needs `nix develop .#kube`.
chart-lint: chart-sync
    helm lint charts/actias -f charts/actias/values-kind.yaml
    ct lint --charts charts/actias
    helm template actias charts/actias -f charts/actias/values-kind.yaml | kubeconform -strict -summary

# Install the chart into a kind cluster and prove it serves a script.
# Creates the cluster if it is missing. Needs `nix develop .#kube`.
chart-install: chart-sync
    ./scripts/chart-smoke.sh

# Everything CI gates on.
ci: deps lint-rust test-rust build-rust lint-api typecheck-api test-api build-api lint-web typecheck-web build-web check-generated

# Cut a release: one version everywhere (workspace, api, web), one
# commit, one tag. Pushing the tag is the release act; the release
# workflow gates on ci and publishes every image from it.
release version:
    python3 -c "import re; p='Cargo.toml'; s=open(p).read(); open(p,'w').write(re.sub(r'(\[workspace\.package\][^\[]*?version = )\"[^\"]+\"', r'\g<1>\"{{version}}\"', s, count=1, flags=re.S))"
    python3 -c "import re; [open(p,'w').write(re.sub(r'\"version\": \"[^\"]+\"', '\"version\": \"{{version}}\"', open(p).read(), count=1)) for p in ['actias-api/package.json','actias-web/package.json']]"
    cargo update --workspace --quiet
    git add Cargo.toml Cargo.lock actias-api/package.json actias-web/package.json
    git commit -m "chore(release): v{{version}}"
    git tag "v{{version}}"
    @echo "release v{{version}} committed and tagged; push with: git push && git push origin v{{version}}"
