// Session state for the SPA. The source of truth is the backend (/api/v1/session); the UI uses
// it to decide what to *show*. Every action is still authorized by the server.
import { useLocation, useNavigate } from "@solidjs/router";
import {
  type Accessor,
  createContext,
  createEffect,
  createResource,
  type JSX,
  on,
  onCleanup,
  type Resource,
  Show,
  useContext,
} from "solid-js";
import { type ApiError, onAuthError, setCsrfToken } from "../api/client";
import { api, authUrls } from "../api/endpoints";
import type { SessionResponse } from "../api/generated/SessionResponse";
import { toast } from "../stores/toast";

interface SessionCtx {
  session: Resource<SessionResponse>;
  refetch: () => void;
  isSystem: Accessor<boolean>;
}

const Ctx = createContext<SessionCtx>();

export function SessionProvider(props: { children: JSX.Element }) {
  const [session, { refetch }] = createResource(api.session);
  createEffect(() => setCsrfToken(session()?.csrf_token ?? null));
  const isSystem = () => {
    const r = session()?.user?.system_role;
    return r === "system_admin" || r === "system_auditor";
  };
  const off = onAuthError((e: ApiError) => {
    if (e.status === 401) {
      toast("Signed out", { body: e.friendly, tone: "warning" });
      refetch();
    } else if (e.code === "csrf_failed") {
      refetch();
    }
  });
  onCleanup(off);
  return <Ctx.Provider value={{ session, refetch, isSystem }}>{props.children}</Ctx.Provider>;
}

export function useSession(): SessionCtx {
  const c = useContext(Ctx);
  if (!c) throw new Error("useSession outside SessionProvider");
  return c;
}

/** Signed-in area: redirects anonymous visitors to /login with a safe return path. */
export function RequireAuth(props: { children: JSX.Element }) {
  const { session } = useSession();
  const navigate = useNavigate();
  const location = useLocation();
  createEffect(
    on(
      () => session.state === "ready" && !session()?.authenticated,
      (anon) => {
        if (anon)
          navigate(`/login?return_to=${encodeURIComponent(location.pathname + location.search)}`, {
            replace: true,
          });
      },
    ),
  );
  return (
    <Show when={session()?.authenticated} fallback={<FullPageLoading />}>
      {props.children}
    </Show>
  );
}

/** Visible only to system staff. UX only: the admin API independently enforces access. */
export function RequireSystem(props: { children: JSX.Element }) {
  const { isSystem } = useSession();
  return (
    <Show
      when={isSystem()}
      fallback={
        <div class="card card-body" role="alert">
          <h1>Not available</h1>
          <p class="muted">This area is restricted to system administrators.</p>
        </div>
      }
    >
      {props.children}
    </Show>
  );
}

export function FullPageLoading() {
  return (
    <div class="auth-page" aria-busy="true">
      <div class="row muted">
        <span class="spinner" aria-hidden="true" /> Loading…
      </div>
    </div>
  );
}

/** Start a fresh-authentication round trip (step-up), returning here afterwards. */
export function reauthenticate() {
  window.location.assign(authUrls.reauth(window.location.pathname + window.location.search));
}
