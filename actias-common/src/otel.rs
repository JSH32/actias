//! Env-gated OpenTelemetry: span export over OTLP and trace-context
//! propagation across grpc and http hops. Nothing here activates unless
//! OTEL_EXPORTER_OTLP_ENDPOINT is set; without it the layer is [`None`],
//! the propagator stays the no-op default, and both helpers cost a
//! header lookup at most. The service name comes from OTEL_SERVICE_NAME,
//! read by the sdk's own resource detection.

use opentelemetry::propagation::{Extractor, Injector};
use opentelemetry::trace::TracerProvider as _;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Whether this process exports; the same switch the sdk exporter
/// reads. An empty value counts as unplugged, so a compose override of
/// `OTEL_ENDPOINT=` turns the whole thing off without editing code.
pub fn enabled() -> bool {
    std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok_and(|value| !value.is_empty())
}

/// The export layer for [`crate::setup_tracing`], [`None`] when the
/// endpoint is not configured. Installing it also installs the W3C
/// trace-context propagator, so injection and extraction light up with
/// the exporter, never separately.
pub fn layer<S>()
-> Result<Option<impl tracing_subscriber::Layer<S>>, Box<dyn std::error::Error + Send + Sync>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    if !enabled() {
        return Ok(None);
    }

    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()?;
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .build();
    opentelemetry::global::set_tracer_provider(provider.clone());

    Ok(Some(
        tracing_opentelemetry::layer().with_tracer(provider.tracer("actias")),
    ))
}

/// Steady-state chatter that never becomes a span, client or server
/// side: the metrics scrape, liveness beats, health probes and the
/// alarm bookkeeping, each at its own cadence forever.
const UNTRACED: [&str; 6] = [
    "/_metrics",
    "/node_registry.NodeRegistryService/Heartbeat",
    "/node_registry.NodeRegistryService/DueAlarms",
    "/node_registry.NodeRegistryService/SetAlarm",
    "/node_registry.NodeRegistryService/ClearAlarm",
    "/grpc.health.v1.Health/Check",
];

/// A grpc transport channel that wraps every call in a client-kind span
/// and injects that span's context into the outgoing headers. The
/// client span is what lets a trace backend pair the hop with the
/// server's span, so service graphs draw caller to callee instead of
/// attributing everything to an anonymous user.
#[derive(Clone)]
pub struct TracedChannel(tonic::transport::Channel);

/// Wraps a transport channel; the platform grpc clients run over this.
pub fn traced_channel(channel: tonic::transport::Channel) -> TracedChannel {
    TracedChannel(channel)
}

struct HeaderInjector<'a>(&'a mut http::HeaderMap);

impl Injector for HeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let (Ok(key), Ok(value)) = (
            key.parse::<http::HeaderName>(),
            http::HeaderValue::from_str(&value),
        ) {
            self.0.insert(key, value);
        }
    }
}

impl<B> tower::Service<http::Request<B>> for TracedChannel
where
    tonic::transport::Channel: tower::Service<http::Request<B>>,
    <tonic::transport::Channel as tower::Service<http::Request<B>>>::Future: Send + 'static,
{
    type Response = <tonic::transport::Channel as tower::Service<http::Request<B>>>::Response;
    type Error = <tonic::transport::Channel as tower::Service<http::Request<B>>>::Error;
    type Future =
        std::pin::Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        tower::Service::poll_ready(&mut self.0, cx)
    }

    fn call(&mut self, mut request: http::Request<B>) -> Self::Future {
        use tracing::Instrument;

        if UNTRACED.contains(&request.uri().path()) {
            return Box::pin(tower::Service::call(&mut self.0, request));
        }

        let span = tracing::info_span!(
            "grpc",
            otel.name = %request.uri().path(),
            otel.kind = "client",
        );
        let context = span.context();
        opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&context, &mut HeaderInjector(request.headers_mut()));
        });
        Box::pin(tower::Service::call(&mut self.0, request).instrument(span))
    }
}

struct HeaderExtractor<'a>(&'a http::HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(http::HeaderName::as_str).collect()
    }
}

/// Tower layer for servers, grpc and http alike: every request runs
/// inside a span whose parent comes from the incoming traceparent
/// header, which is what links one request's spans across services.
#[derive(Clone, Default)]
pub struct TraceExtract;

impl<S> tower::Layer<S> for TraceExtract {
    type Service = TraceExtractService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        TraceExtractService(inner)
    }
}

#[derive(Clone)]
pub struct TraceExtractService<S>(S);

impl<S, B> tower::Service<http::Request<B>> for TraceExtractService<S>
where
    S: tower::Service<http::Request<B>>,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future =
        std::pin::Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send + 'static>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        use tracing::Instrument;

        if UNTRACED.contains(&request.uri().path()) {
            return Box::pin(self.0.call(request));
        }

        let parent = opentelemetry::global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(request.headers()))
        });
        let span = tracing::info_span!(
            "request",
            otel.name = %request.uri().path(),
            otel.kind = "server",
        );
        span.set_parent(parent);
        Box::pin(self.0.call(request).instrument(span))
    }
}
