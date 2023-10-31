#!/bin/bash

docker push ghcr.io/jsh32/actias_script_service:latest
docker push ghcr.io/jsh32/actias_script_service_migration:latest

docker push ghcr.io/jsh32/actias_kv_service_migration:latest
docker push ghcr.io/jsh32/actias_kv_service:latest

docker push ghcr.io/jsh32/actias_api_migration:latest
docker push ghcr.io/jsh32/actias_api:latest

docker push ghcr.io/jsh32/actias_web:latest
docker push ghcr.io/jsh32/actias_worker_service:latest
