// HTTP client for the Rust BFF. The browser holds only an HttpOnly session cookie; this module
// never sees or stores tokens. Unsafe requests carry the per-session CSRF token header.
import type { FieldError } from "./generated/FieldError";

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    readonly detail: string | undefined,
    readonly fieldErrors: FieldError[] = [],
    readonly requestId: string | null = null,
  ) {
    super(detail ?? code);
    this.name = "ApiError";
  }
  /** User-facing message for well-known codes. */
  get friendly(): string {
    return friendlyMessages[this.code] ?? this.detail ?? "Something went wrong. Please try again.";
  }
}

const friendlyMessages: Record<string, string> = {
  unauthenticated: "Your session has ended. Please sign in again.",
  missing_permission: "You don't have permission to do that.",
  system_privilege_required: "This area is for system administrators.",
  mfa_required:
    "This action requires multi-factor authentication. Sign in again with a passkey or authenticator.",
  reauth_required: "For your security, please confirm it's you by signing in again.",
  csrf_failed: "Your session token is out of date. Reload the page and try again.",
  escalation: "You can't grant permissions you don't have.",
  last_owner: "An organization must keep at least one owner.",
  rate_limited: "Too many requests. Please wait a moment.",
  unavailable: "The service is temporarily unavailable. Please try again shortly.",
  not_found: "Not found.",
};

let csrfToken: string | null = null;
export function setCsrfToken(t: string | null) {
  csrfToken = t;
}

type Method = "GET" | "POST" | "PUT" | "PATCH" | "DELETE";

export interface RequestOptions {
  signal?: AbortSignal;
  /** Do not trigger the global "session expired" handler on 401. */
  quiet401?: boolean;
}

const listeners = new Set<(e: ApiError) => void>();
/** Subscribe to authentication/authorization failures (used by the session layer). */
export function onAuthError(fn: (e: ApiError) => void) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export async function request<T>(
  method: Method,
  path: string,
  body?: unknown,
  opts: RequestOptions = {},
): Promise<T> {
  const headers: Record<string, string> = { Accept: "application/json" };
  if (body !== undefined) headers["Content-Type"] = "application/json";
  if (method !== "GET" && csrfToken) headers["X-CSRF-Token"] = csrfToken;
  const res = await fetch(path, {
    method,
    headers,
    credentials: "same-origin",
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: opts.signal,
  });
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  const data = text ? safeJson(text) : undefined;
  if (!res.ok) {
    const p = (data ?? {}) as { code?: string; detail?: string; errors?: FieldError[] };
    const err = new ApiError(
      res.status,
      p.code ?? `http_${res.status}`,
      p.detail,
      p.errors ?? [],
      res.headers.get("x-request-id"),
    );
    if (!(res.status === 401 && opts.quiet401)) for (const l of listeners) l(err);
    throw err;
  }
  return data as T;
}

function safeJson(t: string): unknown {
  try {
    return JSON.parse(t);
  } catch {
    return undefined;
  }
}

export const http = {
  get: <T>(p: string, o?: RequestOptions) => request<T>("GET", p, undefined, o),
  post: <T>(p: string, b?: unknown, o?: RequestOptions) => request<T>("POST", p, b ?? {}, o),
  put: <T>(p: string, b?: unknown, o?: RequestOptions) => request<T>("PUT", p, b ?? {}, o),
  patch: <T>(p: string, b?: unknown, o?: RequestOptions) => request<T>("PATCH", p, b ?? {}, o),
  del: <T>(p: string, o?: RequestOptions) => request<T>("DELETE", p, undefined, o),
};

/** Build a query string, skipping empty values. */
export function qs(params: Record<string, string | number | boolean | null | undefined>): string {
  const u = new URLSearchParams();
  for (const [k, v] of Object.entries(params))
    if (v !== undefined && v !== null && v !== "") u.set(k, String(v));
  const s = u.toString();
  return s ? `?${s}` : "";
}
