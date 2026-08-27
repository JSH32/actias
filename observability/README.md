# Observability config

One shared layer, one delivery layer per environment. The split is
what makes the same dashboards serve compose today and the helm
chart later without forking them.

## Shared: `dashboards/`

Environment-agnostic Grafana dashboard JSON. Panels reference
datasources by uid (`prometheus`, `tempo`), which every environment
provides under those uids. Nothing in these files names a host, a
port, or a container.

| Dashboard | What it shows |
|---|---|
| `actias-overview` | Platform aggregates: requests/errors/duration by node, residency, replica reads, top scripts |
| `actias-scripts` | One script at a time behind a project, then script picker |
| `actias-cluster` | Service map, hop latencies from trace metrics, api runtime, recent traces |

## Per environment

Each environment supplies three things:

1. **An OTLP endpoint** for every service
   (`OTEL_EXPORTER_OTLP_ENDPOINT`; empty value = exporting off).
2. **A scrape of the workers' `/_metrics`** into a Prometheus that
   Grafana reads under the `prometheus` uid.
3. **Delivery of `dashboards/`** into Grafana.

### compose (`compose/`)

The `grafana/otel-lgtm` all-in-one container: OTLP in on 4317/4318,
Grafana on `ACTIAS_GRAFANA_PORT` (3030). `compose/prometheus.yaml`
is the bundle's own config plus a static scrape of both workers;
`compose/grafana-dashboards.yaml` replaces the bundle's demo
dashboards with a file provider over `dashboards/`, mounted
read-only. The bundle also runs Loki and Pyroscope; nothing ships
logs or profiles to them today and the image has no switch to turn
them off, so they idle. That is a dev-bundle cost, not a design.

### kubernetes (the R1 chart, when it lands)

The chart consumes `dashboards/` verbatim: one ConfigMap per file
via `.Files.Glob`, labeled for the Grafana sidecar (or mounted as a
file provider, same JSON either way). The scrape becomes a
ServiceMonitor (or scrape annotation) on the worker StatefulSet's
`/_metrics`. Only the components the environment needs deploy:
Grafana, Tempo, Prometheus; no Loki or Pyroscope until something
feeds them. Values select the lgtm bundle for kind-sized installs
and external endpoints for real ones, the same shape as every other
chart dependency.
