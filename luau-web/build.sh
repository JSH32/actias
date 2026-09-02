#!/usr/bin/env bash
# Builds Actias.Luau.js, the workbench's Luau language service, and puts
# it where the web app serves it. Needs the emscripten toolchain from
# the dev shell: run as `nix develop -c ./build.sh` from this directory.
set -euo pipefail
cd "$(dirname "$0")"

LUAU_TAG=0.735

if [ ! -d vendor/luau ]; then
    mkdir -p vendor
    curl -sL "https://github.com/luau-lang/luau/archive/refs/tags/${LUAU_TAG}.tar.gz" |
        tar -xz -C vendor
    mv "vendor/luau-${LUAU_TAG}" vendor/luau
fi

# --native builds the checker command `actias check` prefers, which needs
# cmake and a C++ compiler but not emscripten. Without it, the wasm the
# workbench loads is what gets built.
if [ "${1:-}" = "--native" ]; then
    cmake -B build-native -G Ninja -DCMAKE_BUILD_TYPE=Release
    cmake --build build-native --target actias-luau
    echo "-> luau-web/build-native/actias-luau"
    echo "   put it on PATH, or point ACTIAS_LUAU at it, and 'actias check'"
    echo "   uses it instead of stock luau-analyze."
    exit 0
fi

emcmake cmake -B build -G Ninja -DCMAKE_BUILD_TYPE=Release
cmake --build build --target Actias.Luau

cp build/Actias.Luau.js ../actias-web/public/luau/Actias.Luau.js
echo "-> actias-web/public/luau/Actias.Luau.js ($(du -h ../actias-web/public/luau/Actias.Luau.js | cut -f1))"
