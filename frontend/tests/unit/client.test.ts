import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, http, onAuthError, qs, setCsrfToken } from "../../src/api/client";

function mockFetch(status: number, body: unknown, headers: Record<string, string> = {}) {
  const fn = vi.fn(
    async (_url: string, _init?: RequestInit) =>
      new Response(body === undefined ? null : JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json", ...headers },
      }),
  );
  vi.stubGlobal("fetch", fn);
  return fn;
}

afterEach(() => {
  vi.unstubAllGlobals();
  setCsrfToken(null);
});

describe("api client", () => {
  it("sends the CSRF token on unsafe methods only, with same-origin credentials", async () => {
    setCsrfToken("tok-123");
    const f = mockFetch(200, { ok: true });
    await http.get("/api/v1/x");
    await http.post("/api/v1/y", { a: 1 });
    const [, getInit] = f.mock.calls[0] as [string, RequestInit];
    const [, postInit] = f.mock.calls[1] as [string, RequestInit];
    expect((getInit.headers as Record<string, string>)["X-CSRF-Token"]).toBeUndefined();
    expect((postInit.headers as Record<string, string>)["X-CSRF-Token"]).toBe("tok-123");
    expect(postInit.credentials).toBe("same-origin");
    expect(postInit.body).toBe('{"a":1}');
  });

  it("parses problem+json into ApiError with friendly text and request id", async () => {
    mockFetch(403, { code: "mfa_required", status: 403, title: "Forbidden" }, { "x-request-id": "rid-1" });
    const err = (await http.get("/api/v1/admin/users").catch((e) => e)) as ApiError;
    expect(err).toBeInstanceOf(ApiError);
    expect(err.code).toBe("mfa_required");
    expect(err.requestId).toBe("rid-1");
    expect(err.friendly).toMatch(/multi-factor/i);
  });

  it("exposes field validation errors", async () => {
    mockFetch(422, { code: "validation_failed", errors: [{ field: "email", message: "must contain @" }] });
    const err = (await http.post("/x", {}).catch((e) => e)) as ApiError;
    expect(err.fieldErrors[0]?.field).toBe("email");
  });

  it("notifies auth listeners on 401 unless quiet", async () => {
    const seen: string[] = [];
    const off = onAuthError((e) => seen.push(e.code));
    mockFetch(401, { code: "unauthenticated" });
    await http.get("/a").catch(() => {});
    await http.get("/b", { quiet401: true }).catch(() => {});
    off();
    expect(seen).toEqual(["unauthenticated"]);
  });

  it("returns undefined for 204", async () => {
    mockFetch(204, undefined);
    expect(await http.del("/x")).toBeUndefined();
  });

  it("builds query strings without empty values", () => {
    expect(qs({ a: 1, b: "", c: undefined, d: null, e: "x y" })).toBe("?a=1&e=x+y");
    expect(qs({})).toBe("");
  });
});
