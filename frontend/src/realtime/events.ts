// Realtime client: Server-Sent Events from /api/v1/events (authenticated by the session
// cookie; the server filters events by verified membership). Reconnects with jittered
// exponential backoff and exposes connection state for the realtime indicator.
import { createSignal } from "solid-js";
import type { RealtimeEvent } from "../api/generated/RealtimeEvent";

export type ConnState = "idle" | "connecting" | "open" | "closed";
type Handler = (e: RealtimeEvent) => void;

const [state, setState] = createSignal<ConnState>("idle");
export const connectionState = state;

const handlers = new Set<Handler>();
let source: EventSource | null = null;
let retries = 0;
let timer: ReturnType<typeof setTimeout> | undefined;
const KINDS: RealtimeEvent["type"][] = [
  "run_created",
  "run_progress",
  "run_finished",
  "notification",
  "provider_health",
  "heartbeat",
];

export function subscribe(fn: Handler): () => void {
  handlers.add(fn);
  if (!source && typeof EventSource !== "undefined") connect();
  return () => {
    handlers.delete(fn);
    if (handlers.size === 0) disconnect();
  };
}

function connect() {
  setState("connecting");
  const es = new EventSource("/api/v1/events", { withCredentials: true });
  source = es;
  es.onopen = () => {
    retries = 0;
    setState("open");
  };
  const dispatch = (ev: MessageEvent) => {
    try {
      const data = JSON.parse(ev.data) as RealtimeEvent;
      for (const h of handlers) h(data);
    } catch {
      /* ignore malformed events */
    }
  };
  for (const k of KINDS) es.addEventListener(k, dispatch as EventListener);
  es.onerror = () => {
    es.close();
    source = null;
    setState("closed");
    if (handlers.size === 0) return;
    const delay = Math.min(30_000, 500 * 2 ** retries) * (0.5 + Math.random() / 2);
    retries++;
    timer = setTimeout(connect, delay);
  };
}

function disconnect() {
  clearTimeout(timer);
  source?.close();
  source = null;
  setState("idle");
}
