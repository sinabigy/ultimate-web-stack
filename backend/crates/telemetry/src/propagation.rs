//! W3C Trace Context propagation between HTTP headers, job rows and `tracing` spans.
//!
//! All functions are no-ops when no OpenTelemetry layer is installed (the span has no
//! OpenTelemetry context), so callers never need to check configuration.

use std::collections::HashMap;

use http::{HeaderMap, HeaderName, HeaderValue};
use opentelemetry::{
    global,
    propagation::{Extractor, Injector},
    trace::TraceContextExt as _,
};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

struct HeaderExtractor<'a>(&'a HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }
    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(HeaderName::as_str).collect()
    }
}

struct HeaderInjector<'a>(&'a mut HeaderMap);

impl Injector for HeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let (Ok(k), Ok(v)) = (HeaderName::from_bytes(key.as_bytes()), HeaderValue::from_str(&value)) {
            self.0.insert(k, v);
        }
    }
}

/// Make `span` a child of the trace described by inbound `traceparent`/`tracestate` headers.
pub fn set_parent_from_headers(span: &tracing::Span, headers: &HeaderMap) {
    if !headers.contains_key("traceparent") {
        return;
    }
    let cx = global::get_text_map_propagator(|p| p.extract(&HeaderExtractor(headers)));
    let _ = span.set_parent(cx);
}

/// Add `traceparent` (and `tracestate`) for `span` to outgoing headers.
pub fn inject_headers(span: &tracing::Span, headers: &mut HeaderMap) {
    let cx = span.context();
    global::get_text_map_propagator(|p| p.inject_context(&cx, &mut HeaderInjector(headers)));
}

/// The current span's trace context as a string map (stored with queued jobs).
pub fn current_context_map() -> HashMap<String, String> {
    let cx = tracing::Span::current().context();
    let mut map = HashMap::new();
    global::get_text_map_propagator(|p| p.inject_context(&cx, &mut map));
    map
}

/// Resume a trace stored by [`current_context_map`] (e.g. in a job row).
pub fn set_parent_from_map(span: &tracing::Span, map: &HashMap<String, String>) {
    if !map.contains_key("traceparent") {
        return;
    }
    let cx = global::get_text_map_propagator(|p| p.extract(map));
    let _ = span.set_parent(cx);
}

/// Hex trace id of `span`, if it belongs to a valid trace (for log correlation).
pub fn trace_id(span: &tracing::Span) -> Option<String> {
    let cx = span.context();
    let sc = cx.span().span_context().clone();
    sc.is_valid().then(|| sc.trace_id().to_string())
}
