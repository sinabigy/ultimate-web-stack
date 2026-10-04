// Frontend telemetry hooks. Pluggable sink: by default metrics go to the console in development
// and nowhere in production. Wire a sink (e.g. an /api/v1/telemetry endpoint or an OTLP
// browser exporter) in main.tsx. Never include user content, tokens or URLs with query strings.

export interface Metric {
  name: "LCP" | "CLS" | "INP" | "TTFB" | "route_change" | "api_error" | "js_error";
  value: number;
  detail?: Record<string, string | number | boolean>;
}

type Sink = (m: Metric) => void;
let sink: Sink = import.meta.env.DEV
  ? (m) => console.debug("[telemetry]", m.name, m.value, m.detail ?? "")
  : () => {};

export function setTelemetrySink(s: Sink) {
  sink = s;
}

export function record(m: Metric) {
  try {
    sink(m);
  } catch {
    /* telemetry must never break the app */
  }
}

export function installTelemetry() {
  if (typeof PerformanceObserver === "undefined") return;
  const observe = (type: string, fn: (e: PerformanceEntry) => void) => {
    try {
      new PerformanceObserver((list) => list.getEntries().forEach(fn)).observe({
        type,
        buffered: true,
      } as PerformanceObserverInit);
    } catch {
      /* unsupported entry type */
    }
  };
  observe("largest-contentful-paint", (e) => record({ name: "LCP", value: Math.round(e.startTime) }));
  let cls = 0;
  observe("layout-shift", (e) => {
    const ls = e as PerformanceEntry & { value: number; hadRecentInput: boolean };
    if (!ls.hadRecentInput) cls += ls.value;
  });
  observe("event", (e) => {
    const ev = e as PerformanceEntry & { duration: number };
    if (ev.duration > 200) record({ name: "INP", value: Math.round(ev.duration), detail: { type: e.name } });
  });
  const nav = performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined;
  if (nav) record({ name: "TTFB", value: Math.round(nav.responseStart) });
  addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") record({ name: "CLS", value: Math.round(cls * 1000) / 1000 });
  });
  addEventListener("error", (e) =>
    record({ name: "js_error", value: 1, detail: { message: String(e.message).slice(0, 200) } }),
  );
  addEventListener("unhandledrejection", () =>
    record({ name: "js_error", value: 1, detail: { message: "unhandledrejection" } }),
  );
}
