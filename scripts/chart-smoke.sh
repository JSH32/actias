#!/usr/bin/env bash
# Installs the chart into a kind cluster and proves the wiring: every
# workload becomes ready, the api answers through its own service, and a
# published script runs on a worker and round-trips a value through kv.
#
# This is a wiring test, not a behavioural one. The compose smoke owns
# the platform's surface; what can only break in kubernetes is service
# discovery, probes, migration hooks, volumes and the registration
# address, and that is what this exercises.
#
# Requires docker on the host and the kube devshell. Run as:
#   nix develop .#kube -c just chart-install
set -euo pipefail

cd "$(dirname "$0")/.."

CLUSTER="${CHART_SMOKE_CLUSTER:-actias-chart}"
NS="${CHART_SMOKE_NAMESPACE:-actias}"
RELEASE=actias
# Pinned so resource names are predictable regardless of how helm
# collapses a release name that matches the chart name.
FULLNAME=actias
KEEP="${CHART_SMOKE_KEEP:-0}"
# Images are loaded into the cluster rather than pulled, so a local
# build is what gets tested. Point at published tags to test those.
IMAGE_TAG="${CHART_SMOKE_TAG:-latest}"
REGISTRY="${CHART_SMOKE_REGISTRY:-ghcr.io/jsh32}"
# Ingress needs a controller in the cluster; set 0 to skip that phase
# and test only the path-prefix and port-forward routes.
INGRESS="${CHART_SMOKE_INGRESS:-1}"
INGRESS_MANIFEST="https://raw.githubusercontent.com/kubernetes/ingress-nginx/controller-v1.12.0/deploy/static/provider/kind/deploy.yaml"
# The host port kind maps to the node's :80, so ingress rules can be
# reached by Host header without touching dns.
EDGE="http://127.0.0.1:8080"
CONSOLE_HOST=console.actias.test
API_HOST=api.actias.test
SCRIPTS_DOMAIN=scripts.actias.test
FAILED=1
API_PID=""
WORKER_PID=""
DEVDIR=""
REPO=$PWD

cleanup() {
    [ -n "$API_PID" ] && kill "$API_PID" 2>/dev/null || true
    [ -n "$WORKER_PID" ] && kill "$WORKER_PID" 2>/dev/null || true
    [ -n "$DEVDIR" ] && rm -rf "$DEVDIR"
    if [ "$FAILED" = 1 ]; then
        echo "== workloads"
        kubectl -n "$NS" get pods -o wide 2>/dev/null || true
        echo "== recent events"
        kubectl -n "$NS" get events --sort-by=.lastTimestamp 2>/dev/null | tail -30 || true
        echo "== logs from pods that are not running"
        for pod in $(kubectl -n "$NS" get pods -o jsonpath='{range .items[?(@.status.phase!="Running")]}{.metadata.name}{"\n"}{end}' 2>/dev/null); do
            echo "-- $pod"
            kubectl -n "$NS" logs "$pod" --tail=40 --all-containers 2>/dev/null || true
        done
    fi
    if [ "$KEEP" = 0 ]; then
        kind delete cluster --name "$CLUSTER" >/dev/null 2>&1 || true
    else
        echo "== cluster kept: kubectl --context kind-$CLUSTER -n $NS get pods"
    fi
}
trap cleanup EXIT

if ! kind get clusters 2>/dev/null | grep -qx "$CLUSTER"; then
    echo "== creating kind cluster $CLUSTER"
    kind create cluster --name "$CLUSTER" --config scripts/kind-cluster.yaml --wait 120s
fi
kubectl config use-context "kind-$CLUSTER" >/dev/null

echo "== loading images into the cluster"
# Every image the chart references, so nothing is pulled from a registry
# the CI runner may not reach.
for image in actias_script_service actias_secret_service actias_kv_service actias_placement_service \
             actias_api actias_web actias_worker_service; do
    ref="$REGISTRY/$image:$IMAGE_TAG"
    docker image inspect "$ref" >/dev/null 2>&1 \
        || { echo "missing image $ref; build the stack first (just up or docker compose build)"; exit 1; }
    kind load docker-image "$ref" --name "$CLUSTER" >/dev/null
done

if [ "$INGRESS" = 1 ]; then
    echo "== installing the ingress controller"
    kubectl apply -f "$INGRESS_MANIFEST" >/dev/null
    # Wait on the POD, not the Deployment: the deployment reports
    # available before the admission webhook inside it is listening.
    kubectl -n ingress-nginx wait --for=condition=ready pod \
        --selector=app.kubernetes.io/component=controller --timeout=5m
    # The patch job installs the webhook's CA bundle. Until it finishes
    # the apiserver has nothing to trust the endpoint with.
    kubectl -n ingress-nginx wait --for=condition=complete job \
        --selector=app.kubernetes.io/component=admission-webhook --timeout=5m 2>/dev/null || true

    # None of the above proves the apiserver can actually reach the
    # webhook: an endpoint is published before it is ready, and
    # kube-proxy programs the service a moment later still. So ask the
    # real question instead of proxying for it. A server-side dry run
    # runs admission and creates nothing, so it fails exactly as the
    # release would and costs nothing when it succeeds.
    admitted=0
    for _ in $(seq 1 90); do
        if kubectl create --dry-run=server -f - >/dev/null 2>&1 <<'PROBE'; then
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: admission-probe
  namespace: default
spec:
  ingressClassName: nginx
  rules:
    - host: probe.invalid
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: probe
                port:
                  number: 80
PROBE
            admitted=1
            break
        fi
        sleep 2
    done
    [ "$admitted" = 1 ] || {
        echo "the admission webhook never accepted an ingress"
        kubectl -n ingress-nginx get pod,svc,endpointslices
        exit 1
    }
fi

echo "== installing the chart"
ingress_values=(--set ingress.enabled=false)
if [ "$INGRESS" = 1 ]; then
    # The hosts are reached by Host header against the mapped node port,
    # so they need no dns entry to be real ingress traffic.
    ingress_values=(
        --set ingress.enabled=true
        --set ingress.console="$CONSOLE_HOST"
        --set ingress.api="$API_HOST"
        --set baseDomain="$SCRIPTS_DOMAIN"
    )
fi
helm upgrade --install "$RELEASE" charts/actias \
    --namespace "$NS" --create-namespace \
    -f charts/actias/values-kind.yaml \
    --set fullnameOverride="$FULLNAME" \
    --set image.tag="$IMAGE_TAG" \
    "${ingress_values[@]}" \
    --wait --timeout 10m

echo "== every workload is ready"
kubectl -n "$NS" rollout status "statefulset/$FULLNAME-worker" --timeout=5m
kubectl -n "$NS" rollout status "deployment/$FULLNAME-api" --timeout=5m

echo "== the api answers"
# Migrations are not asserted by looking at the Jobs: on an upgrade
# they are hooks and helm deletes them once they succeed, so their
# absence proves nothing either way. What proves the schema is the
# registration below, which needs tables no probe touches.
kubectl -n "$NS" port-forward "svc/$FULLNAME-api" 18080:3000 >/dev/null 2>&1 &
API_PID=$!
API=http://127.0.0.1:18080/api
ready=0
for _ in $(seq 1 60); do
    if curl -sf "$API/health" -o /dev/null; then ready=1; break; fi
    sleep 2
done
[ "$ready" = 1 ] || { echo "api never answered /api/health"; exit 1; }

echo "== workers registered with their stable addresses"
# The registration address is the pod's own dns name, so a worker that
# registered a wrong one shows up here as the wrong host.
kubectl -n "$NS" port-forward "svc/$FULLNAME-worker" 18081:3000 >/dev/null 2>&1 &
WORKER_PID=$!
worker_ready=0
for _ in $(seq 1 60); do
    if curl -sf "http://127.0.0.1:18081/_metrics" -o /dev/null; then worker_ready=1; break; fi
    sleep 2
done
[ "$worker_ready" = 1 ] || { echo "worker never served /_metrics"; exit 1; }

echo "== registering a user (proves the schema migrations applied)"
SUFFIX=$RANDOM
curl -sf -X POST "$API/users" -H 'Content-Type: application/json' \
    -d "{\"username\":\"chart$SUFFIX\",\"email\":\"chart$SUFFIX@example.com\",\"password\":\"chart-smoke-password\"}" >/dev/null

TOKEN=$(curl -sf -X POST "$API/auth/login" -H 'Content-Type: application/json' \
    -d "{\"auth\":\"chart$SUFFIX\",\"password\":\"chart-smoke-password\"}" | jq -r .token)
AUTH="Authorization: Bearer $TOKEN"

PROJECT_ID=$(curl -sf -X POST "$API/project" -H "$AUTH" -H 'Content-Type: application/json' \
    -d '{"name":"chart-smoke"}' | jq -r .id)
IDENT="chart$SUFFIX"
SCRIPT_ID=$(curl -sf -X POST "$API/project/$PROJECT_ID/scripts" -H "$AUTH" -H 'Content-Type: application/json' \
    -d "{\"publicIdentifier\":\"$IDENT\"}" | jq -r .id)

DEVDIR=$(mktemp -d)
export XDG_CONFIG_HOME="$DEVDIR/config"
mkdir -p "$XDG_CONFIG_HOME/actias" "$DEVDIR/project"
printf '{"apiUrl":"http://127.0.0.1:18080","token":"%s"}' "$TOKEN" \
    > "$XDG_CONFIG_HOME/actias/settings.json"

cat > "$DEVDIR/project/script.json" <<EOF
{"id":"$SCRIPT_ID","entryPoint":"main.lua","includes":["**/*.lua"],"ignore":[]}
EOF
# kv proves the worker reaches kv-service and its database; the object
# proves script-service leases and the worker's own volume.
cat > "$DEVDIR/project/main.lua" <<'LUA'
local ns = kv "chart"

local Counter = object "Counter" {
    bump = function(state)
        local n = (state.store:get("n") or 0) + 1
        state.store:set("n", n)
        return n
    end,
}

on "fetch" (function(request)
    ns:set("hello", "world")
    return {
        status = 200,
        body = json.stringify({
            kv = ns:get("hello"),
            count = Counter:get("smoke"):bump(),
        }),
    }
end)
LUA

cargo build -p actias-cli --quiet
"$REPO/target/debug/actias" publish "$DEVDIR/project" >/dev/null

echo "== the worker serves the script"
body=""
for _ in $(seq 1 30); do
    body=$(curl -sf "http://127.0.0.1:18081/$IDENT/" || true)
    [ -n "$body" ] && break
    sleep 2
done
echo "$body" | jq -e '.kv == "world"' >/dev/null \
    || { echo "kv round trip failed: $body"; exit 1; }
echo "$body" | jq -e '.count >= 1' >/dev/null \
    || { echo "object call failed: $body"; exit 1; }

echo "== objects survive a worker restart"
before=$(curl -sf "http://127.0.0.1:18081/$IDENT/" | jq -r .count)
# Every worker, not just the first: the object is homed on whichever one
# served it, so deleting a single pod might leave it untouched and prove
# nothing. Taking them all guarantees its host went away and the count
# below comes from restored state.
kubectl -n "$NS" delete pod -l app.kubernetes.io/component=worker --wait=true >/dev/null
kubectl -n "$NS" rollout status "statefulset/$FULLNAME-worker" --timeout=5m
kill "$WORKER_PID" 2>/dev/null || true
kubectl -n "$NS" port-forward "svc/$FULLNAME-worker" 18081:3000 >/dev/null 2>&1 &
WORKER_PID=$!
after=""
for _ in $(seq 1 60); do
    after=$(curl -sf "http://127.0.0.1:18081/$IDENT/" | jq -r .count 2>/dev/null || true)
    [ -n "$after" ] && [ "$after" != "null" ] && break
    sleep 2
done
[ -n "$after" ] && [ "$after" -gt "$before" ] \
    || { echo "the object lost its state across a restart: $before then $after"; exit 1; }

if [ "$INGRESS" = 1 ]; then
    echo "== the three hosts route through the ingress"
    # The console and api answer on their own hosts, and the script
    # answers on its label under the wildcard, which is the routing the
    # console's own links depend on.
    for _ in $(seq 1 30); do
        curl -sf -H "Host: $API_HOST" "$EDGE/api/health" -o /dev/null && break
        sleep 2
    done
    curl -sf -H "Host: $API_HOST" "$EDGE/api/health" -o /dev/null \
        || { echo "the api host did not route"; exit 1; }
    # Server-rendered pages, so a crash in the console's data layer is a
    # 500 here rather than a blank screen someone finds later.
    for page in / /login /projects /download; do
        curl -sf -H "Host: $CONSOLE_HOST" "$EDGE$page" -o /dev/null \
            || { echo "the console did not render $page"; exit 1; }
    done

    # The wildcard rule sends every label to a worker, which resolves
    # the script from the label itself rather than from a path.
    wild=""
    for _ in $(seq 1 30); do
        wild=$(curl -sf -H "Host: $IDENT.$SCRIPTS_DOMAIN" "$EDGE/" || true)
        [ -n "$wild" ] && break
        sleep 2
    done
    echo "$wild" | jq -e '.kv == "world"' >/dev/null \
        || { echo "the script did not serve under its subdomain: $wild"; exit 1; }

    # A revision preview is the same wildcard with a "--r-" label, and
    # the console builds exactly this url.
    REV=$(curl -sf -H "$AUTH" "$API/script/$SCRIPT_ID/revisions?page=1" \
        | jq -r '.items[0].id')
    prev=$(curl -sf -H "Host: $IDENT--r-$REV.$SCRIPTS_DOMAIN" "$EDGE/" || true)
    echo "$prev" | jq -e '.kv == "world"' >/dev/null \
        || { echo "the revision preview host did not serve: $prev"; exit 1; }

    # A label with no script behind it must 404 rather than reach some
    # default backend, which is what proves the wildcard resolves the
    # script from the host rather than serving whatever it has.
    unknown=$(curl -s -o /dev/null -w '%{http_code}' \
        -H "Host: nosuchscript.$SCRIPTS_DOMAIN" "$EDGE/")
    [ "$unknown" = 404 ] \
        || { echo "an unknown script host answered $unknown, expected 404"; exit 1; }

    echo "== websocket annotations are on the api and worker rules"
    kubectl -n "$NS" get ingress "$FULLNAME" -o jsonpath='{.metadata.annotations}' \
        | grep -q "proxy-read-timeout" \
        || { echo "the websocket annotations are missing"; exit 1; }
fi

echo "== chart smoke passed (kv round trip, object call, restart continuity$([ "$INGRESS" = 1 ] && echo ", three-host ingress"))"
FAILED=0
