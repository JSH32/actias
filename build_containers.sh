#!/bin/bash

function build_container {
    docker build --build-arg CRATE_NAME=$1 -t ghcr.io/jsh32/$2:latest .
}

build_container "ephermal-script-service" "script_service"
build_container "ephermal-worker" "worker_service"