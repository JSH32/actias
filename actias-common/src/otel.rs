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

/// Grpc client interceptor: stamps the current span's context into the
/// outgoing metadata. A plain `fn` so client types stay nameable (see
/// the worker-core `Grpc` alias). No-op until the propagator installs.
// The Err size is tonic's Interceptor contract, not a choice here.
#[allow(clippy::result_large_err)]
pub fn trace_inject(mut request: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
    let context = tracing::Span::current().context();
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut MetadataInjector(request.metadata_mut()));
    });
    Ok(request)
}

struct MetadataInjector<'a>(&'a mut tonic::metadata::MetadataMap);

impl Injector for MetadataInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let (Ok(key), Ok(value)) = (
            key.parse::<tonic::metadata::MetadataKey<tonic::metadata::Ascii>>(),
            value.parse(),
        ) {
            self.0.insert(key, value);
        }
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
