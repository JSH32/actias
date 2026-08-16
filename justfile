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

# Tear the local stack down.
down:
    docker compose down

# Follow logs for the whole stack, or one service: `just logs worker_service`.
logs service="":
    docker compose logs -f {{ service }}

# Everything CI gates on.
ci: deps lint-rust test-rust build-rust lint-api test-api build-api lint-web build-web check-generated
