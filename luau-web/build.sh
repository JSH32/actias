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

emcmake cmake -B build -G Ninja -DCMAKE_BUILD_TYPE=Release
cmake --build build

cp build/Actias.Luau.js ../actias-web/public/luau/Actias.Luau.js
echo "-> actias-web/public/luau/Actias.Luau.js ($(du -h ../actias-web/public/luau/Actias.Luau.js | cut -f1))"
