/**
 * Env-gated OpenTelemetry: nothing here activates unless
 * OTEL_EXPORTER_OTLP_ENDPOINT is set. Imported before anything else in
 * main.ts so the auto-instrumentations can patch fastify, grpc and pg
 * before they load. The service name comes from OTEL_SERVICE_NAME.
 */
import { NodeSDK } from '@opentelemetry/sdk-node';
import { OTLPTraceExporter } from '@opentelemetry/exporter-trace-otlp-proto';
import { getNodeAutoInstrumentations } from '@opentelemetry/auto-instrumentations-node';

if (process.env.OTEL_EXPORTER_OTLP_ENDPOINT) {
  const sdk = new NodeSDK({
    // No keep-alive: a collector restart otherwise leaves the agent
    // holding a dead socket and every later batch fails against it
    // until the api itself restarts. A fresh connection per batch
    // makes a collector outage cost only the batches sent into it.
    traceExporter: new OTLPTraceExporter({ keepAlive: false }),
    instrumentations: [
      getNodeAutoInstrumentations({
        // Every static asset read would otherwise become a span.
        '@opentelemetry/instrumentation-fs': { enabled: false },
      }),
    ],
  });
  sdk.start();
  process.on('SIGTERM', () => {
    sdk.shutdown().catch(() => undefined);
  });
}
