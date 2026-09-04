{{/*
Shared plumbing for every template: names, labels, image refs and the
env blocks that assemble a service's view of the stores. Templates
never learn whether a store is bundled or external, or whether a
secret was rendered or referenced; these helpers are where that
knowledge lives, once.
*/}}

{{- define "actias.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "actias.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name (include "actias.name" .) | trunc 63 | trimSuffix "-" | replace (printf "%s-%s" .Chart.Name .Chart.Name) .Chart.Name -}}
{{- end -}}
{{- end -}}

{{- define "actias.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/*
Labels for one component's resources; the selector labels are the
subset a Deployment or Service matches on. The split matters because
selectors are immutable after creation, so only the stable identity
lives in them while the rest may grow. Usage, both forms:
  {{ include "actias.labels" (dict "root" . "component" "api") }}
  {{ include "actias.selectorLabels" (dict "root" . "component" "api") }}
*/}}
{{- define "actias.labels" -}}
helm.sh/chart: {{ include "actias.chart" .root }}
{{ include "actias.selectorLabels" . }}
app.kubernetes.io/version: {{ .root.Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .root.Release.Service }}
{{- end -}}

{{- define "actias.selectorLabels" -}}
app.kubernetes.io/name: {{ include "actias.name" .root }}
app.kubernetes.io/instance: {{ .root.Release.Name }}
app.kubernetes.io/component: {{ .component }}
{{- end -}}

{{/*
Image reference for one component. Usage:
  {{ include "actias.image" (dict "root" . "component" .Values.api) }}
The tag falls back to appVersion so chart version and image tags move
together.
*/}}
{{- define "actias.image" -}}
{{- $tag := default .root.Chart.AppVersion .root.Values.image.tag -}}
{{- printf "%s/%s:%s" .root.Values.image.registry .component.repository $tag -}}
{{- end -}}

{{/*
The postgres host a service dials: the bundled StatefulSet's service,
or the external host. Port likewise.
*/}}
{{- define "actias.postgresHost" -}}
{{- if .Values.postgres.bundled -}}
{{- printf "%s-postgres" (include "actias.fullname" .) -}}
{{- else -}}
{{- required "externalPostgres.host is required when postgres.bundled=false" .Values.externalPostgres.host -}}
{{- end -}}
{{- end -}}

{{- define "actias.postgresPort" -}}
{{- if .Values.postgres.bundled -}}5432{{- else -}}{{ .Values.externalPostgres.port }}{{- end -}}
{{- end -}}

{{/*
The Secret carrying postgres credentials (keys: username, password):
the operator's existingSecret when set, else the chart's own rendered
one.
*/}}
{{- define "actias.postgresSecretName" -}}
{{- if .Values.postgres.bundled -}}
{{- default (printf "%s-postgres" (include "actias.fullname" .)) .Values.postgres.auth.existingSecret -}}
{{- else -}}
{{- required "externalPostgres.existingSecret is required when postgres.bundled=false" .Values.externalPostgres.existingSecret -}}
{{- end -}}
{{- end -}}

{{/*
DATABASE_URL env entries for one logical database. Credentials stay in
their Secret: the username/password land as PG_USER/PG_PASSWORD via
secretKeyRef and the url composes them with kubernetes $(VAR)
expansion, which only substitutes variables defined EARLIER in the env
list, so order here is load-bearing. Usage:
  {{ include "actias.databaseUrlEnv" (dict "root" . "database" "actias_api") }}
*/}}
{{- define "actias.databaseUrlEnv" -}}
- name: PG_USER
  valueFrom:
    secretKeyRef:
      name: {{ include "actias.postgresSecretName" .root }}
      key: username
- name: PG_PASSWORD
  valueFrom:
    secretKeyRef:
      name: {{ include "actias.postgresSecretName" .root }}
      key: password
- name: DATABASE_URL
  value: "postgresql://$(PG_USER):$(PG_PASSWORD)@{{ include "actias.postgresHost" .root }}:{{ include "actias.postgresPort" .root }}/{{ .database }}"
{{- if .root.Values.postgres.readReplicaHost }}
- name: READ_DATABASE_URL
  value: "postgresql://$(PG_USER):$(PG_PASSWORD)@{{ .root.Values.postgres.readReplicaHost }}:{{ include "actias.postgresPort" .root }}/{{ .database }}"
{{- end }}
{{- end -}}

{{/*
The redis url: the bundled service or the external endpoint.
*/}}
{{- define "actias.redisUrl" -}}
{{- if .Values.redis.bundled -}}
{{- printf "redis://%s-redis:6379" (include "actias.fullname" .) -}}
{{- else -}}
{{- required "externalRedis.url is required when redis.bundled=false" .Values.externalRedis.url -}}
{{- end -}}
{{- end -}}

{{/*
S3 env entries: endpoint, bucket and credentials from whichever store
is active. Bundled minio's credentials come from its Secret (keys:
root-user, root-password); an external store's from its existingSecret
(keys: access-key, secret-key).
*/}}
{{- define "actias.s3Env" -}}
{{- if .Values.minio.bundled -}}
- name: S3_ENDPOINT
  value: "http://{{ include "actias.fullname" . }}-minio:9000"
- name: S3_BUCKET
  value: {{ .Values.externalS3.bucket | quote }}
- name: S3_ACCESS_KEY
  valueFrom:
    secretKeyRef:
      name: {{ default (printf "%s-minio" (include "actias.fullname" .)) .Values.minio.auth.existingSecret }}
      key: root-user
- name: S3_SECRET_KEY
  valueFrom:
    secretKeyRef:
      name: {{ default (printf "%s-minio" (include "actias.fullname" .)) .Values.minio.auth.existingSecret }}
      key: root-password
{{- else -}}
- name: S3_ENDPOINT
  value: {{ required "externalS3.endpoint is required when minio.bundled=false" .Values.externalS3.endpoint | quote }}
- name: S3_BUCKET
  value: {{ .Values.externalS3.bucket | quote }}
- name: S3_ACCESS_KEY
  valueFrom:
    secretKeyRef:
      name: {{ required "externalS3.existingSecret is required when minio.bundled=false" .Values.externalS3.existingSecret }}
      key: access-key
- name: S3_SECRET_KEY
  valueFrom:
    secretKeyRef:
      name: {{ .Values.externalS3.existingSecret }}
      key: secret-key
{{- end -}}
{{- end -}}

{{/*
OTLP env for one service; renders nothing when the endpoint it needs is
empty, which is how exporting stays off.

Two endpoints, because the exporters do not share a wire protocol: the
rust services export OTLP over grpc (a collector's 4317) and the api
exports OTLP over http/protobuf (its 4318). Handing either one the
other's port drops every span silently, so the api asks for
`otelHttp` and everything else takes the default. Usage:
  {{ include "actias.otelEnv" (dict "root" . "service" "actias_api" "http" true) }}
*/}}
{{- define "actias.otelEnv" -}}
{{- $endpoint := .root.Values.otelEndpoint -}}
{{- if .http -}}
{{- $endpoint = .root.Values.otelHttpEndpoint -}}
{{- end -}}
{{- if $endpoint -}}
- name: OTEL_EXPORTER_OTLP_ENDPOINT
  value: {{ $endpoint | quote }}
- name: OTEL_SERVICE_NAME
  value: {{ .service | quote }}
{{- end -}}
{{- end -}}

{{/*
One platform secret as an env var. Resolution order: the entry's
existingSecret wins; else an explicit value routes through the chart's
own rendered Secret (templates/secret.yaml, same key names); neither is
a refused install, which is the never-defaulted rule enforced. Usage:
  {{ include "actias.secretEnv" (dict "root" . "env" "JWT_KEY" "entry" .Values.secrets.jwtKey "name" "secrets.jwtKey") }}
*/}}
{{- define "actias.secretEnv" -}}
- name: {{ .env }}
  valueFrom:
    secretKeyRef:
      {{- if .entry.existingSecret }}
      name: {{ .entry.existingSecret }}
      {{- else if .entry.value }}
      name: {{ include "actias.fullname" .root }}-secrets
      {{- else }}
      name: {{ required (printf "%s needs a value or an existingSecret; the platform's security rests on it and the chart will not default it" .name) nil }}
      {{- end }}
      key: {{ .entry.key }}
{{- end -}}

{{/*
Browser-facing origins. With ingress these are the configured hosts;
without it they are the port-forwards NOTES.txt prints, which is what
makes an ingressless install usable for CI and local clusters.
*/}}
{{- define "actias.consoleOrigin" -}}
{{- if and .Values.ingress.enabled .Values.ingress.console -}}
https://{{ .Values.ingress.console }}
{{- else -}}
http://localhost:3000
{{- end -}}
{{- end -}}

{{- define "actias.apiOrigin" -}}
{{- if and .Values.ingress.enabled .Values.ingress.api -}}
https://{{ .Values.ingress.api }}
{{- else -}}
http://localhost:3001
{{- end -}}
{{- end -}}

{{/*
Where the console sends visitors to reach a script. Under a baseDomain
scripts answer on their own subdomain and revisions on a "--r-" label;
without one, the path-prefix forms on a worker. The placeholders are
the console's own: it substitutes _IDENTIFIER_ and _REVISION_.
*/}}
{{- define "actias.workerBase" -}}
{{- if .Values.baseDomain -}}
https://_IDENTIFIER_.{{ .Values.baseDomain }}
{{- else -}}
http://localhost:3002/_IDENTIFIER_
{{- end -}}
{{- end -}}

{{- define "actias.workerRevisionBase" -}}
{{- if .Values.baseDomain -}}
https://_IDENTIFIER_--r-_REVISION_.{{ .Values.baseDomain }}
{{- else -}}
http://localhost:3002/_rev/_IDENTIFIER_/_REVISION_
{{- end -}}
{{- end -}}

{{/*
An initContainer that blocks until a dependency accepts connections.

The services fail fast when a dependency is missing, which is right for
a process (a worker that cannot reach script-service has nothing to do)
but wrong for a rollout: on a fresh install the whole stack starts at
once, and a pod that exits repeatedly lands in CrashLoopBackOff, whose
backoff can outlast the dependency's startup and make helm call the
release failed. Waiting here turns that race into ordering.

It runs the component's OWN image, so no third image joins the install
and nothing has to be pulled into a cluster that was handed its images.
The pull policy travels with it for that same reason: a container that
does not set one defaults to Always on a `:latest` tag, which pulls a
published image over the loaded one and silently tests the wrong build.
Usage, at pod-spec indent:
  {{- include "actias.waitFor" (dict "root" . "image" $img "name" "postgres" "host" $h "port" 5432) | nindent 6 }}
*/}}
{{- define "actias.waitFor" -}}
- name: wait-for-{{ .name }}
  image: {{ .image | quote }}
  imagePullPolicy: {{ .root.Values.image.pullPolicy }}
  command:
    - bash
    - -c
    - |
      until (exec 3<>/dev/tcp/{{ .host }}/{{ .port }}) 2>/dev/null; do
        echo "waiting for {{ .name }} at {{ .host }}:{{ .port }}"
        sleep 2
      done
{{- end -}}

{{/*
Scheduling hygiene for one component's pod spec: everything an
operator tunes without our involvement. Usage, at pod-spec indent:
  {{- include "actias.scheduling" (dict "root" . "component" .Values.api) | nindent 6 }}
*/}}
{{- define "actias.scheduling" -}}
{{- with .component.nodeSelector }}
nodeSelector: {{- toYaml . | nindent 2 }}
{{- end }}
{{- with .component.tolerations }}
tolerations: {{- toYaml . | nindent 2 }}
{{- end }}
{{- with .component.affinity }}
affinity: {{- toYaml . | nindent 2 }}
{{- end }}
{{- with .component.topologySpreadConstraints }}
topologySpreadConstraints: {{- toYaml . | nindent 2 }}
{{- end }}
{{- with .component.priorityClassName }}
priorityClassName: {{ . }}
{{- end }}
{{- with .root.Values.imagePullSecrets }}
imagePullSecrets: {{- toYaml . | nindent 2 }}
{{- end }}
{{- end -}}

{{/*
The placement service's store: the bundled postgres by default, or the
scylla cluster the values name, replicated in this datacenter alone.
*/}}
{{- define "actias.placementStoreEnv" -}}
{{- if eq .Values.placement.backend "scylla" }}
- name: PLACEMENT_BACKEND
  value: "scylla"
- name: SCYLLA_NODES
  value: {{ required "placement.scyllaNodes names the cluster when placement.backend is scylla" .Values.placement.scyllaNodes | quote }}
- name: SCYLLA_DC
  value: {{ .Values.placement.scyllaDc | quote }}
- name: SCYLLA_REPLICATION_FACTOR
  value: {{ .Values.placement.replicationFactor | quote }}
{{- else }}
- name: PLACEMENT_BACKEND
  value: "postgres"
{{ include "actias.databaseUrlEnv" (dict "root" . "database" "actias_placement") }}
{{- end }}
{{- end }}
