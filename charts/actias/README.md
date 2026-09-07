# actias

Helm chart for the Actias platform: the API, the web console, the
script/secret/kv services, and the worker fleet that runs scripts and
durable objects.

## Installing

```sh
helm install actias oci://ghcr.io/jsh32/charts/actias \
  --set ingress.enabled=true \
  --set ingress.console=console.example.com \
  --set ingress.api=api.example.com \
  --set baseDomain=scripts.example.com \
  -f my-secrets.yaml
```

The chart has no dependencies. By default it brings its own
single-replica postgres, redis and minio, which is enough for kind or
an evaluation; for production you point each at your own
infrastructure (below).

## Secrets

Four values have no default and the install fails if they are missing:

| Value | Used for |
|---|---|
| `secrets.jwtKey` | session signing in the API |
| `secrets.internalToken` | API-to-worker and worker-to-worker calls |
| `secrets.masterKey` | the key-encryption key in secret-service (base64, 32 bytes) |
| `secrets.masterKeyId` | names the current master key version |

Each takes either a literal `value` (the chart creates a Secret) or an
`existingSecret` name pointing at a Secret you manage, in which case
the chart never sees the value. Database and S3 passwords work the
same way inside their store blocks. `values-kind.yaml` sets throwaway
values for CI and local clusters.

## Stores

Each store is a pair of blocks: the bundled one and the external one.

```yaml
postgres:
  bundled: false
externalPostgres:
  host: db.internal
  port: 5432
  existingSecret: pg-credentials   # keys: username, password
```

Anything that speaks the protocol works: RDS or CloudNativePG for
postgres, S3 or R2 for `externalS3`, ElastiCache for redis. Redis
carries live log tails and pub/sub only, so losing it loses no data.

An external postgres needs either a role that may `CREATE DATABASE`
(each service creates its own database on first connect) or the four
databases pre-created: `actias_script_service`,
`actias_secret_service`, `actias_kv`, `actias_api`.

## Migrations

Each migration Job uses the same image as the service it migrates, so
the schema and the binary are built together and cannot drift. The Jobs
receive only `DATABASE_URL`; in particular, the secret-service
migration never holds the master key.

When they run depends on the operation. On an **upgrade** they are
`pre-upgrade` hooks: the database holds data and pods are serving, so
the new schema lands before the new pods roll and a migration that
cannot apply fails the release. On an **install** they are ordinary
resources that come up alongside the stores and retry until the
database answers, because a pre-install hook would run before the
release's own objects exist and a bundled store would not be there to
migrate.

## Ingress

Two values produce the three hosts:

```yaml
ingress:
  enabled: true
  console: console.example.com   # web console
  api: api.example.com           # REST API
baseDomain: scripts.example.com  # user scripts, *.scripts.example.com
```

Scripts are served by subdomain: `<name>.scripts.example.com`, with
revision previews at `<name>--r-<revision>.scripts.example.com`. The
console builds its links from the same value.

The wildcard host needs a wildcard certificate. With ACME that means a
DNS01 solver; the chart does not install cert-manager. The API and
worker rules disable proxy buffering and raise timeouts, because both
hold long-lived websockets.

With `ingress.enabled=false` scripts remain reachable by path prefix
on any worker (`/<name>/`), and the console via port-forward.

## Workers

Workers run as a StatefulSet with a headless service, because each pod
registers a stable address that peers and the API dial back. Each pod
gets a volume for its object databases. The volume is a cache: if it
is lost, objects restore from the blob store, and a stale volume on a
returning pod is detected and re-fetched.

Scaling down is ordinary pod deletion. A stopping worker deregisters
and its objects move to another node on the next call; there is no
drain procedure. The PodDisruptionBudget limits node drains to one
worker at a time.

`workers.tuning` exposes the commonly adjusted settings (hibernation
timers, sweep cadence, database size cap, WAL shipping thresholds,
the durability ack budget). Anything else goes through
`workers.extraEnv`.

Every service scales with `replicas`. The stateless ones (api, web,
kv, secret, script) can also autoscale on CPU through `autoscaling`;
workers are left out because CPU says little about what makes a worker
busy. The script service belongs in that list because it keeps nothing
in memory: Postgres decides who owns a lease, Redis holds live script
sessions, and its one background job deletes aged-out node rows, which
is harmless to run from several replicas. The default is 1 because the
registry is rarely the busy part.

## The placement store

`placement.backend` chooses what holds leases, membership, alarms and
the instance directory for the region: `postgres` or `scylla`. Both
pass the same conformance suite, and nothing above the store can tell
them apart.

Postgres is one primary with transactions. A claim is one statement;
failover is the database's own, a replica promoted by an operator or a
managed service; the store is a database already being run for the
control plane. It is the right choice for one region on one node or a
few, which is every small deployment.

Scylla is leaderless. Every row lives on `placement.replicationFactor`
nodes of one datacenter and a claim is a lightweight transaction
across them, so the store survives the loss of a node with no failover
step and scales with the region's workers. That benefit exists only
when the store spans several nodes. A one-node scylla is a slower
postgres that uses more memory and has no transactions across rows.
Choose scylla for a region that must survive node loss without an
operator, or whose claim rate outgrows one primary; choose postgres
otherwise. Either way the store is per region and never talks to
another region's; scylla gives a region a highly available store, not
a store shared between regions.

`scylla.bundled: true` with `placement.backend: scylla` renders a
one-node scylla (developer mode, one shard) so the backend can be
tried on kind or a single region. It carries none of the benefit
above and is for evaluation only; a real scylla region names its
cluster in `placement.scyllaNodes`. The placement service and its
migration wait on the store they use, so a scylla region need not run
a postgres of its own.

## Regions

One release is one control plane and one region. The control plane
(api, web, script, secret and kv services, their databases, redis) is
shared; a region is a placement service, an object bucket and workers
under one name, `placement.region`. `region.bucket` names the object
bucket; empty means the blob bucket, the single-region layout.

A second region is another release of the chart with
`controlPlane.enabled: false` and the shared control plane's services
in `controlPlane.external`. Such a release renders the placement
service, the workers, the stores it uses and the registration job;
no api, web, script, secret or kv service, no redis, and only the
placement migration. `values-region.yaml` is the worked example. The
region's S3 endpoint must hold both the control plane's bundle bucket
(workers pull bundles by hash from it) and the region's own object
bucket.

`region.register` names the region to the control plane: a Job after
install and upgrade logs in to the api as the instance admin and PUTs
`/regions/<name>` with the addresses other regions and the control
plane reach this one on (`dataPlaneAddr`, the worker service's grpc
port, and `placementAddr`), the bucket and the S3 credentials. The
control plane offers homes, forwards calls and moves projects only
among registered regions. The control plane's own release may set it
too, so its region is registered the same way rather than by hand.

The one secret a region carries is `secrets.internalToken`, the same
value as the control plane's; `jwtKey` and `masterKey` belong to the
control plane and are not asked for.

## Observability

Set `otelEndpoint` to an OTLP collector and every service exports
traces there; leave it empty and exporting is off.
`observability.dashboards.enabled` installs the repo's Grafana
dashboards as ConfigMaps with the `grafana_dashboard: "1"` label for a
sidecar-provisioned Grafana; they expect datasources named
`prometheus` and `tempo`. Workers expose request metrics on
`/_metrics`; the worker StatefulSet carries a commented PodMonitor
showing the shape, since scraping belongs to whatever collector you
run.

## Developing the chart

The dashboards live at `observability/dashboards/` and are shared with
the compose stack. A chart cannot read files above its own directory,
so `just chart-sync` copies them in; `chart-lint`, `chart-install` and
the publish workflow all run it first, and the copy is gitignored.

```sh
nix develop .#kube -c just chart-lint      # helm lint, ct lint, kubeconform
nix develop .#kube -c just chart-install   # kind cluster, install, probe
```

`chart-install` builds nothing: it loads the images already on your
docker daemon into the cluster, so build them first (`docker compose
build`).

It installs ingress-nginx and exercises the three hosts by Host header
against the node port kind maps to 8080, so the routing is real
traffic rather than a rendered rule. Useful switches:

| Variable | Effect |
|---|---|
| `CHART_SMOKE_KEEP=1` | leave the cluster up for inspection |
| `CHART_SMOKE_INGRESS=0` | skip the controller and the host routing |
| `CHART_SMOKE_TAG` | install a different image tag (default `latest`) |
