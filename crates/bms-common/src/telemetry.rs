//! Logs and distributed traces.
//!
//! Logs always go to stdout. When `OTEL_EXPORTER_OTLP_ENDPOINT` is set
//! (e.g. `http://localhost:4318`), spans are also exported over OTLP to a tracing
//! backend such as Jaeger, where one event can be followed across all services.
//!
//! A trace crosses service boundaries inside the event itself: the envelope
//! carries the W3C trace context (`traceparent`) of the span that created it,
//! and the consumer continues the same trace from it.

use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::SpanExporter;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use std::collections::HashMap;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::filter::{EnvFilter, filter_fn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, fmt};

/// Log level used when `RUST_LOG` is not set.
const DEFAULT_FILTER: &str = "info";

/// W3C trace context of a span, e.g. `{"traceparent": "00-<trace id>-<span id>-01"}`.
pub type TraceContext = HashMap<String, String>;

/// Keeps the trace exporter alive; dropping it sends the remaining spans.
#[must_use = "dropping the guard stops exporting traces"]
pub struct Telemetry {
    provider: Option<SdkTracerProvider>,
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take()
            && let Err(error) = provider.shutdown()
        {
            eprintln!("Failed to flush traces: {error}");
        }
    }
}

/// Initializes logging and, if configured, trace export.
///
/// The log level comes from `RUST_LOG` (e.g. `debug` or `bms_state=trace`),
/// defaulting to `info`. `service_name` is how the service appears in traces.
pub fn init(service_name: &'static str) -> Telemetry {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer());

    let provider = match tracer_provider(service_name) {
        Ok(provider) => provider,
        Err(error) => {
            registry.init();
            tracing::error!(%error, "Trace export disabled: exporter failed to start");
            return Telemetry { provider: None };
        },
    };

    if let Some(provider) = &provider {
        global::set_text_map_propagator(TraceContextPropagator::new());
        let otel = tracing_opentelemetry::layer()
            .with_tracer(provider.tracer(service_name))
            // Only our own spans: exporting the HTTP client's spans would
            // also trace the export requests themselves.
            .with_filter(filter_fn(|metadata| metadata.target().starts_with("bms_")));
        registry.with(otel).init();
        tracing::info!("Exporting traces over OTLP");
    } else {
        registry.init();
    }
    Telemetry { provider }
}

/// Trace export is enabled by `OTEL_EXPORTER_OTLP_ENDPOINT`; the exporter reads
/// it (and the other standard `OTEL_*` variables) itself.
fn tracer_provider(
    service_name: &'static str,
) -> Result<Option<SdkTracerProvider>, opentelemetry_otlp::ExporterBuildError> {
    if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_none() {
        return Ok(None);
    }
    let exporter = SpanExporter::builder().with_http().build()?;
    let provider = SdkTracerProvider::builder()
        // Spans are collected in memory and sent in batches by a background thread.
        .with_batch_exporter(exporter)
        .with_resource(Resource::builder().with_service_name(service_name).build())
        .build();
    Ok(Some(provider))
}

/// Trace context of the current span, to be carried by an outgoing event.
/// Empty when trace export is disabled.
pub fn current_context() -> TraceContext {
    let mut carrier = TraceContext::new();
    let context = Span::current().context();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut carrier);
    });
    carrier
}

/// Makes `span` continue the trace described by `carrier` (from an incoming event).
pub fn continue_trace(span: &Span, carrier: &TraceContext) {
    if carrier.is_empty() {
        return;
    }
    let context = global::get_text_map_propagator(|propagator| propagator.extract(carrier));
    // Fails only if the span was already started or is disabled; then it simply
    // stays the root of its own trace.
    let _ = span.set_parent(context);
}
