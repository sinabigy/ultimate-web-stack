import { A, useNavigate } from "@solidjs/router";
import { createResource, createSignal, For, type JSX, Show } from "solid-js";
import { ApiError } from "../api/client";
import { api } from "../api/endpoints";
import { reauthenticate, useSession } from "../auth/session";
import { ActivityFeed, AsyncView, DataTable } from "../components/data";
import { Icon } from "../components/icons";
import {
  Alert,
  Avatar,
  Badge,
  Button,
  Card,
  ConfirmDialog,
  EmptyState,
  Field,
  PageHeader,
  Tabs,
} from "../components/ui";
import { deviceLabel, fmtDateTime, fmtRelative } from "../lib/format";
import { setTheme, type Theme, theme } from "../stores/theme";
import { toast } from "../stores/toast";

const crumbs = (label: string) => [{ label: "Account", href: "/account/profile" }, { label }];

export function AccountLayout(props: { children?: JSX.Element }) {
  return (
    <div class="stack">
      <Tabs
        label="Account sections"
        items={[
          { href: "/account/profile", label: "Profile" },
          { href: "/account/security", label: "Security" },
          { href: "/account/passkeys", label: "Passkeys" },
          { href: "/account/mfa", label: "Two-factor" },
          { href: "/account/sessions", label: "Sessions" },
          { href: "/account/preferences", label: "Preferences" },
          { href: "/account/notifications", label: "Notifications" },
          { href: "/account/activity", label: "Activity" },
        ]}
      />
      {props.children}
    </div>
  );
}

function errMsg(e: unknown) {
  return e instanceof ApiError ? e.friendly : "Something went wrong.";
}

export function ProfilePage() {
  const { refetch: refetchSession } = useSession();
  const [profile, { refetch }] = createResource(api.account.profile);
  const [name, setName] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const save = async (e: SubmitEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.account.updateProfile({ display_name: name() ?? profile()?.display_name ?? "" });
      toast("Profile saved", { tone: "success" });
      refetch();
      refetchSession();
    } catch (err) {
      setError(err instanceof ApiError ? (err.fieldErrors[0]?.message ?? err.friendly) : "Could not save");
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <PageHeader
        title="Profile"
        breadcrumbs={crumbs("Profile")}
        description="Your name is managed here; sign-in credentials are managed by the identity provider."
      />
      <AsyncView data={profile} retry={refetch}>
        {(p) => (
          <div class="grid-2">
            <Card title="Personal information">
              <form class="stack" onSubmit={save}>
                <div class="row">
                  <Avatar name={p.display_name} large />
                  <div class="stack-sm">
                    <strong>{p.display_name}</strong>
                    <span class="subtle">Member since {fmtDateTime(p.created_at)}</span>
                  </div>
                </div>
                <Field label="Display name" error={error()}>
                  {(a) => (
                    <input
                      id={a.id}
                      class="input"
                      aria-describedby={a.describedBy}
                      aria-invalid={a.invalid}
                      value={name() ?? p.display_name}
                      onInput={(e) => setName(e.currentTarget.value)}
                      maxLength={80}
                      required
                    />
                  )}
                </Field>
                <div>
                  <Button type="submit" variant="primary" loading={busy()}>
                    Save changes
                  </Button>
                </div>
              </form>
            </Card>
            <Card title="Email & identity">
              <div class="stack">
                <Field
                  label="Email"
                  hint="Change your email at the identity provider; it updates here on your next sign-in."
                >
                  {(a) => (
                    <input
                      id={a.id}
                      class="input"
                      value={p.email}
                      readOnly
                      aria-describedby={a.describedBy}
                    />
                  )}
                </Field>
                <div class="row">
                  <Badge tone={p.email_verified ? "success" : "warning"}>
                    {p.email_verified ? "Verified" : "Not verified"}
                  </Badge>
                  <Show when={!p.email_verified}>
                    <A href="/verify-email">Verify email</A>
                  </Show>
                </div>
                <div class="subtle">
                  Identity provider: <code>{p.identity.provider}</code>
                </div>
                <div class="subtle">Last sign-in {fmtRelative(p.last_login_at)}</div>
              </div>
            </Card>
          </div>
        )}
      </AsyncView>
    </>
  );
}

function SecurityOverviewCard() {
  const [sec, { refetch }] = createResource(api.account.security);
  return (
    <AsyncView data={sec} retry={refetch}>
      {(s) => (
        <div class="stack">
          <div class="grid-cards">
            <Card>
              <div class="stack-sm">
                <span class="metric-label">This session</span>
                <Badge tone={s.session.mfa ? "success" : "warning"}>
                  {s.session.mfa ? "Multi-factor" : "Single factor"}
                </Badge>
                <span class="subtle">Methods: {s.session.amr.join(", ") || "unknown"}</span>
              </div>
            </Card>
            <Card>
              <div class="stack-sm">
                <span class="metric-label">Passkeys</span>
                <span class="metric-value">{s.identity_provider.overview?.passkeys ?? "—"}</span>
              </div>
            </Card>
            <Card>
              <div class="stack-sm">
                <span class="metric-label">Two-factor</span>
                <Badge tone={s.identity_provider.overview?.mfa_enabled ? "success" : "warning"}>
                  {s.identity_provider.overview?.mfa_enabled == null
                    ? "Unknown"
                    : s.identity_provider.overview.mfa_enabled
                      ? "Enabled"
                      : "Not enabled"}
                </Badge>
              </div>
            </Card>
          </div>
          <Show when={!s.identity_provider.available}>
            <Alert tone="warning">
              Credential details are temporarily unavailable from the identity provider.
            </Alert>
          </Show>
          <Card title="Recent security events">
            <ActivityFeed items={s.events} empty="No security events yet" />
          </Card>
        </div>
      )}
    </AsyncView>
  );
}

export function SecurityPage() {
  const { session } = useSession();
  const navigate = useNavigate();
  const [confirm, setConfirm] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const del = async () => {
    setBusy(true);
    try {
      await api.account.deleteAccount(session()?.user?.email ?? "");
      window.location.assign("/login");
    } catch (e) {
      if (e instanceof ApiError && e.code === "reauth_required") {
        toast("Please confirm it's you", {
          body: "You'll come back here after signing in again.",
          tone: "warning",
        });
        reauthenticate();
      } else toast("Account not deleted", { body: errMsg(e), tone: "danger" });
    } finally {
      setBusy(false);
      setConfirm(false);
    }
  };
  return (
    <>
      <PageHeader
        title="Security"
        breadcrumbs={crumbs("Security")}
        description="How you sign in and where you're signed in."
        actions={<Button onClick={() => navigate("/account/sessions")}>Manage sessions</Button>}
      />
      <div class="stack">
        <SecurityOverviewCard />
        <Card title="Danger zone">
          <div class="row-between wrap">
            <div class="stack-sm">
              <strong>Delete account</strong>
              <span class="subtle">
                Removes your profile and signs you out everywhere. Organizations you solely own must be
                transferred first.
              </span>
            </div>
            <Button variant="danger" onClick={() => setConfirm(true)}>
              Delete account
            </Button>
          </div>
        </Card>
      </div>
      <ConfirmDialog
        open={confirm()}
        title="Delete your account?"
        body={<>This cannot be undone. For your security you may be asked to sign in again first.</>}
        confirmLabel="Delete my account"
        danger
        requireText={session()?.user?.email}
        busy={busy()}
        onConfirm={del}
        onClose={() => setConfirm(false)}
      />
    </>
  );
}

function CredentialList(props: { kind: "passkey" | "mfa" }) {
  const [sec, { refetch }] = createResource(api.account.security);
  const kinds = () =>
    props.kind === "passkey" ? ["passkey", "security_key"] : ["totp", "otp_email", "otp_sms"];
  return (
    <AsyncView data={sec} retry={refetch}>
      {(s) => {
        const methods = () =>
          (s.identity_provider.overview?.methods ?? []).filter((m) => kinds().includes(m.kind));
        return (
          <Card
            title={props.kind === "passkey" ? "Your passkeys" : "Two-factor methods"}
            actions={
              <Show when={s.identity_provider.overview?.manage_url}>
                <a
                  class="btn btn-primary btn-sm"
                  href={s.identity_provider.overview?.manage_url ?? "#"}
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  <Icon name="plus" size={14} /> {props.kind === "passkey" ? "Add passkey" : "Set up"}
                </a>
              </Show>
            }
          >
            <Show
              when={methods().length}
              fallback={
                <EmptyState
                  icon={props.kind === "passkey" ? "fingerprint" : "lock"}
                  title={props.kind === "passkey" ? "No passkeys yet" : "Two-factor authentication is off"}
                  body={
                    props.kind === "passkey"
                      ? "Passkeys use your device's biometrics or PIN. They can't be phished or reused."
                      : "Add an authenticator app so a stolen password isn't enough to get in."
                  }
                />
              }
            >
              <ul class="feed">
                <For each={methods()}>
                  {(m) => (
                    <li>
                      <Icon name={m.kind === "passkey" ? "fingerprint" : "lock"} />
                      <span class="grow">{m.label ?? m.kind.replace("_", " ")}</span>
                      <Show when={m.id}>
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={async () => {
                            try {
                              await api.account.removeMethod(m.kind, m.id as string);
                              toast("Removed", { tone: "success" });
                              refetch();
                            } catch (e) {
                              if (e instanceof ApiError && e.code === "reauth_required") reauthenticate();
                              else toast("Not removed", { body: errMsg(e), tone: "danger" });
                            }
                          }}
                        >
                          Remove
                        </Button>
                      </Show>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
            <p class="subtle mt-3">
              Credentials are stored by the identity provider, never by this application. Changes open the
              provider's secure account page.
            </p>
          </Card>
        );
      }}
    </AsyncView>
  );
}

export function PasskeysPage() {
  return (
    <>
      <PageHeader
        title="Passkeys"
        breadcrumbs={crumbs("Passkeys")}
        description="Sign in with your fingerprint, face or device PIN."
      />
      <CredentialList kind="passkey" />
    </>
  );
}

export function MfaPage() {
  return (
    <>
      <PageHeader
        title="Two-factor authentication"
        breadcrumbs={crumbs("Two-factor")}
        description="Authenticator apps and one-time codes."
      />
      <CredentialList kind="mfa" />
    </>
  );
}

export function SessionsPage() {
  const [list, { refetch }] = createResource(api.account.sessions);
  const revoke = async (id: string, current: boolean) => {
    try {
      await api.account.revokeSession(id);
      if (current) window.location.assign("/login");
      else {
        toast("Session signed out", { tone: "success" });
        refetch();
      }
    } catch (e) {
      toast("Couldn't sign out session", { body: errMsg(e), tone: "danger" });
    }
  };
  const others = async () => {
    const r = await api.account.revokeAll(false);
    toast(`Signed out ${r.count} other session${r.count === 1 ? "" : "s"}`, { tone: "success" });
    refetch();
  };
  return (
    <>
      <PageHeader
        title="Sessions & devices"
        breadcrumbs={crumbs("Sessions")}
        description="Everywhere you're currently signed in."
        actions={
          <>
            <Button onClick={others}>Sign out other devices</Button>
            <Button
              variant="danger"
              onClick={async () => {
                await api.account.revokeAll(true);
                window.location.assign("/login");
              }}
            >
              Sign out everywhere
            </Button>
          </>
        }
      />
      <Card flush>
        <AsyncView data={list} retry={refetch}>
          {(l) => (
            <DataTable
              caption="Active sessions"
              rows={l.items}
              rowKey={(s) => s.id}
              columns={[
                {
                  key: "device",
                  header: "Device",
                  cell: (s) => (
                    <span class="row">
                      <Icon name="monitor" />
                      <span class="stack-sm gap-0">
                        <strong>{deviceLabel(s.user_agent)}</strong>
                        <span class="subtle">{s.ip ?? "IP not recorded"}</span>
                      </span>
                      <Show when={s.current}>
                        <Badge tone="brand">This device</Badge>
                      </Show>
                    </span>
                  ),
                },
                {
                  key: "mfa",
                  header: "Sign-in",
                  cell: (s) => (
                    <Badge tone={s.mfa ? "success" : "neutral"}>{s.mfa ? "MFA" : "Single factor"}</Badge>
                  ),
                },
                {
                  key: "seen",
                  header: "Last active",
                  cell: (s) => fmtRelative(s.last_seen_at),
                  sortValue: (s) => s.last_seen_at,
                },
                {
                  key: "created",
                  header: "Signed in",
                  cell: (s) => fmtDateTime(s.created_at),
                  sortValue: (s) => s.created_at,
                },
                {
                  key: "act",
                  header: "",
                  align: "right",
                  cell: (s) => (
                    <Button size="sm" variant="ghost" onClick={() => revoke(s.id, s.current)}>
                      {s.current ? "Sign out" : "Revoke"}
                    </Button>
                  ),
                },
              ]}
            />
          )}
        </AsyncView>
      </Card>
    </>
  );
}

export function PreferencesPage() {
  const save = async (t: Theme) => {
    setTheme(t);
    try {
      await api.account.updatePreferences({ theme: t });
    } catch {
      /* preference still applied locally */
    }
  };
  return (
    <>
      <PageHeader title="Preferences" breadcrumbs={crumbs("Preferences")} />
      <Card title="Appearance">
        <fieldset class="stack-sm fieldset-reset">
          <legend class="label">Theme</legend>
          <For each={["system", "light", "dark"] as Theme[]}>
            {(t) => (
              <label class="checkbox">
                <input type="radio" name="theme" value={t} checked={theme() === t} onChange={() => save(t)} />
                {t === "system" ? "Match system" : t === "light" ? "Light" : "Dark"}
              </label>
            )}
          </For>
        </fieldset>
      </Card>
    </>
  );
}

export function NotificationsPage() {
  const [list, { refetch }] = createResource(() => api.notifications.list());
  return (
    <>
      <PageHeader
        title="Notifications"
        breadcrumbs={crumbs("Notifications")}
        actions={
          <Button
            onClick={async () => {
              await api.notifications.readAll();
              refetch();
            }}
          >
            Mark all read
          </Button>
        }
      />
      <Card flush>
        <AsyncView
          data={list}
          retry={refetch}
          empty={(l) => l.items.length === 0}
          emptyView={<EmptyState icon="bell" title="No notifications" />}
        >
          {(l) => (
            <ul class="feed px-5">
              <For each={l.items}>
                {(n) => (
                  <li>
                    <Icon name={n.read_at ? "check" : "bell"} />
                    <div class="grow">
                      <strong>{n.title}</strong>
                      <div class="subtle">{n.body}</div>
                    </div>
                    <time class="subtle" datetime={n.created_at}>
                      {fmtRelative(n.created_at)}
                    </time>
                    <Show when={!n.read_at}>
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={async () => {
                          await api.notifications.read(n.id);
                          refetch();
                        }}
                      >
                        Mark read
                      </Button>
                    </Show>
                  </li>
                )}
              </For>
            </ul>
          )}
        </AsyncView>
      </Card>
    </>
  );
}

export function ActivityPage() {
  const [page, { refetch }] = createResource(api.account.activity);
  return (
    <>
      <PageHeader
        title="Activity"
        breadcrumbs={crumbs("Activity")}
        description="Security and account events for your user."
      />
      <Card>
        <AsyncView data={page} retry={refetch}>
          {(p) => <ActivityFeed items={p.items} />}
        </AsyncView>
      </Card>
    </>
  );
}
