// Public authentication pages. Credentials are never entered here: every method hands off to the
// identity provider through the BFF (/auth/login, /auth/register). This page is the branded
// entry point that shows only the methods the deployment has configured.
import { A, useNavigate, useParams, useSearchParams } from "@solidjs/router";
import { createEffect, createResource, createSignal, For, type JSX, Show } from "solid-js";
import { ApiError, setCsrfToken } from "../api/client";
import { api, authUrls } from "../api/endpoints";
import { FullPageLoading, reauthenticate, useSession } from "../auth/session";
import { Icon } from "../components/icons";
import { Alert, Button, Card, ErrorState, Field } from "../components/ui";
import { toast } from "../stores/toast";

const ERRORS: Record<string, string> = {
  idp_unavailable: "The sign-in service is temporarily unavailable. Please try again in a moment.",
  login_rejected: "We couldn't verify that sign-in. Please try again.",
  state_mismatch: "That sign-in link expired or was opened in another browser. Please start again.",
  flow_expired: "That sign-in attempt expired. Please start again.",
  account_inactive: "This account is suspended or deleted. Contact support if you think this is a mistake.",
  email_required: "Your identity provider did not share an email address.",
  registration_closed: "Registration is closed for this application.",
  idp_error: "The sign-in was cancelled or failed at the identity provider.",
  unknown_method: "That sign-in method is not available.",
};

const SOCIAL_LABEL: Record<string, string> = {
  google: "Google",
  apple: "Apple",
  github: "GitHub",
  microsoft: "Microsoft",
};

function AuthFrame(props: { title: string; subtitle?: string; children: JSX.Element; footer?: JSX.Element }) {
  createEffect(() => {
    document.title = `${props.title} · App`;
  });
  return (
    <main class="auth-page">
      <div class="auth-card stack">
        <div class="row justify-center">
          <span class="brand-mark size-36" aria-hidden="true">
            <Icon name="activity" size={20} />
          </span>
        </div>
        <Card>
          <div class="stack">
            <div class="stack-sm text-center">
              <h1>{props.title}</h1>
              <Show when={props.subtitle}>
                <p class="muted">{props.subtitle}</p>
              </Show>
            </div>
            {props.children}
          </div>
        </Card>
        <Show when={props.footer}>
          <p class="subtle text-center">{props.footer}</p>
        </Show>
      </div>
    </main>
  );
}

function useRedirectIfSignedIn() {
  const { session } = useSession();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  createEffect(() => {
    if (session()?.authenticated)
      navigate(safeReturn(params.return_to as string | undefined), { replace: true });
  });
}

function safeReturn(p: string | undefined): string {
  return p?.startsWith("/") && !p.startsWith("//") ? p : "/dashboard";
}

function MethodButtons(props: { mode: "login" | "register"; returnTo: string }) {
  const { session } = useSession();
  const cfg = () => session()?.login;
  const go = (method?: string) =>
    window.location.assign(
      props.mode === "login"
        ? authUrls.login({ returnTo: props.returnTo, method })
        : authUrls.register({ returnTo: props.returnTo }),
    );
  return (
    <div class="stack-sm">
      <Show when={cfg()?.passkey}>
        <Button variant="primary" size="lg" block icon="fingerprint" onClick={() => go("passkey")}>
          Continue with passkey
        </Button>
      </Show>
      <For each={cfg()?.social ?? []}>
        {(name) => (
          <Button size="lg" block onClick={() => go(name)}>
            Continue with {SOCIAL_LABEL[name] ?? name}
          </Button>
        )}
      </For>
      <Show when={cfg()?.enterprise_sso}>
        <Button size="lg" block icon="building" onClick={() => go("sso")}>
          Continue with SSO
        </Button>
      </Show>
    </div>
  );
}

function EmailContinue(props: { mode: "login" | "register"; returnTo: string }) {
  const [email, setEmail] = createSignal("");
  const [err, setErr] = createSignal<string | null>(null);
  const submit = (e: SubmitEvent) => {
    e.preventDefault();
    const v = email().trim();
    if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(v)) {
      setErr("Enter a valid email address.");
      return;
    }
    window.location.assign(
      props.mode === "login"
        ? authUrls.login({ returnTo: props.returnTo, method: "email", loginHint: v })
        : authUrls.register({ returnTo: props.returnTo, loginHint: v }),
    );
  };
  return (
    <form class="stack-sm" onSubmit={submit} noValidate>
      <Field label="Email" error={err()}>
        {(a) => (
          <input
            id={a.id}
            class="input"
            type="email"
            autocomplete="username webauthn"
            aria-describedby={a.describedBy}
            aria-invalid={a.invalid}
            value={email()}
            onInput={(e) => {
              setEmail(e.currentTarget.value);
              setErr(null);
            }}
          />
        )}
      </Field>
      <Button type="submit" size="lg" block>
        Continue
      </Button>
    </form>
  );
}

export function LoginPage() {
  useRedirectIfSignedIn();
  const { session } = useSession();
  const [params] = useSearchParams();
  const returnTo = () => safeReturn(params.return_to as string | undefined);
  const hasButtons = () =>
    !!(session()?.login.passkey || session()?.login.social.length || session()?.login.enterprise_sso);
  return (
    <Show when={session.state === "ready"} fallback={<FullPageLoading />}>
      <AuthFrame
        title="Sign in"
        subtitle="Welcome back"
        footer={
          <Show when={session()?.login.registration}>
            New here? <A href={`/register?return_to=${encodeURIComponent(returnTo())}`}>Create an account</A>
          </Show>
        }
      >
        <Show when={params.error}>
          <Alert tone="danger">{ERRORS[params.error as string] ?? "Sign-in failed. Please try again."}</Alert>
        </Show>
        <MethodButtons mode="login" returnTo={returnTo()} />
        <Show when={hasButtons()}>
          <div class="or-divider">or</div>
        </Show>
        <EmailContinue mode="login" returnTo={returnTo()} />
        <Show when={session()?.login.password}>
          <A href="/forgot-password" class="subtle text-center">
            Forgot your password?
          </A>
        </Show>
      </AuthFrame>
    </Show>
  );
}

export function RegisterPage() {
  useRedirectIfSignedIn();
  const { session } = useSession();
  const [params] = useSearchParams();
  const returnTo = () => safeReturn(params.return_to as string | undefined);
  return (
    <Show when={session.state === "ready"} fallback={<FullPageLoading />}>
      <AuthFrame
        title="Create your account"
        subtitle="Passkeys are the fastest and most secure way in."
        footer={
          <>
            Already registered? <A href="/login">Sign in</A>
          </>
        }
      >
        <Show
          when={session()?.login.registration}
          fallback={<Alert tone="warning">Registration is by invitation only.</Alert>}
        >
          <MethodButtons mode="register" returnTo={returnTo()} />
          <div class="or-divider">or</div>
          <EmailContinue mode="register" returnTo={returnTo()} />
          <p class="subtle text-center">
            You'll verify your email and can add a passkey and two-factor authentication at any time.
          </p>
        </Show>
      </AuthFrame>
    </Show>
  );
}

export function ForgotPasswordPage() {
  const [email, setEmail] = createSignal("");
  return (
    <AuthFrame
      title="Reset your password"
      subtitle="Password recovery is handled securely by our identity provider."
      footer={<A href="/login">Back to sign in</A>}
    >
      <form
        class="stack-sm"
        onSubmit={(e) => {
          e.preventDefault();
          window.location.assign(authUrls.login({ method: "password", loginHint: email().trim() }));
        }}
      >
        <Field label="Email" hint="On the next screen choose “Forgot password” to receive a reset link.">
          {(a) => (
            <input
              id={a.id}
              class="input"
              type="email"
              autocomplete="username"
              aria-describedby={a.describedBy}
              value={email()}
              onInput={(e) => setEmail(e.currentTarget.value)}
              required
            />
          )}
        </Field>
        <Button type="submit" variant="primary" block>
          Continue to password reset
        </Button>
      </form>
      <p class="subtle">Tip: with a passkey you won't need a password at all.</p>
    </AuthFrame>
  );
}

export function ResetPasswordPage() {
  return (
    <AuthFrame
      title="Password updated"
      subtitle="If you just reset your password at the identity provider, sign in with it now."
    >
      <Button variant="primary" block onClick={() => window.location.assign(authUrls.login())}>
        Sign in
      </Button>
    </AuthFrame>
  );
}

export function VerifyEmailPage() {
  const { session } = useSession();
  const [busy, setBusy] = createSignal(false);
  const resend = async () => {
    setBusy(true);
    try {
      await api.account.resendVerification();
      toast("Verification email sent", { tone: "success" });
    } catch (e) {
      toast("Couldn't send email", { body: e instanceof ApiError ? e.friendly : undefined, tone: "danger" });
    } finally {
      setBusy(false);
    }
  };
  return (
    <AuthFrame title="Verify your email">
      <Show
        when={session()?.authenticated}
        fallback={
          <Button block onClick={() => window.location.assign(authUrls.login({ returnTo: "/verify-email" }))}>
            Sign in to continue
          </Button>
        }
      >
        <Show
          when={!session()?.user?.email_verified}
          fallback={
            <Alert tone="success" title="Email verified">
              {session()?.user?.email} is verified. <A href="/dashboard">Continue to your dashboard</A>.
            </Alert>
          }
        >
          <p class="muted">
            We sent a verification link to <strong>{session()?.user?.email}</strong>. Some features (accepting
            invitations, administrator access) need a verified email.
          </p>
          <div class="stack-sm">
            <Button variant="primary" block loading={busy()} onClick={resend}>
              Resend verification email
            </Button>
            <Button block onClick={reauthenticate}>
              I've verified it — refresh
            </Button>
          </div>
        </Show>
      </Show>
    </AuthFrame>
  );
}

/** The backend owns /auth/callback; this page only renders if a deployment routes it to the SPA. */
export function AuthCallbackPage() {
  const navigate = useNavigate();
  createEffect(() => navigate("/dashboard", { replace: true }));
  return <FullPageLoading />;
}

export function LogoutPage() {
  const { session } = useSession();
  let started = false;
  // Logout is a CSRF-protected POST: wait for the session (which carries the token) instead of
  // racing it, set the token explicitly, and only then post. Anonymous visitors go to /login.
  createEffect(async () => {
    if (session.loading || started) return;
    started = true;
    const s = session.error ? undefined : session();
    if (!s?.authenticated) {
      window.location.assign("/login");
      return;
    }
    setCsrfToken(s.csrf_token ?? null);
    try {
      const r = await api.logout();
      window.location.assign(r.redirect);
    } catch {
      window.location.assign("/login");
    }
  });
  return <FullPageLoading />;
}

export function InvitationPage() {
  const params = useParams();
  const navigate = useNavigate();
  const { refetch } = useSession();
  const [inv] = createResource(() => params.token, api.invitation);
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<unknown>(null);
  const accept = async () => {
    setBusy(true);
    try {
      const r = await api.acceptInvitation(params.token as string);
      toast(`Joined ${r.organization.name}`, { tone: "success" });
      refetch();
      navigate(`/org/${r.organization.slug}`);
    } catch (e) {
      setErr(e);
    } finally {
      setBusy(false);
    }
  };
  return (
    <AuthFrame title="Join organization">
      <Show when={!inv.error} fallback={<ErrorState error={inv.error} />}>
        <Show when={inv()} fallback={<p class="muted">Loading invitation…</p>}>
          {(i) => (
            <div class="stack">
              <p>
                You've been invited to join <strong>{i().organization.name}</strong> as{" "}
                <strong>{i().role}</strong>.
              </p>
              <p class="subtle">Invitation for {i().email}</p>
              <Show when={err()}>
                <Alert tone="danger">
                  {err() instanceof ApiError
                    ? (err() as ApiError).code === "invitation_email_mismatch"
                      ? "This invitation was sent to a different email address than the one you're signed in with."
                      : (err() as ApiError).friendly
                    : "Couldn't accept the invitation."}
                </Alert>
              </Show>
              <Button variant="primary" block loading={busy()} onClick={accept}>
                Accept invitation
              </Button>
            </div>
          )}
        </Show>
      </Show>
    </AuthFrame>
  );
}
