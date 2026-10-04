import { A, useNavigate, useParams } from "@solidjs/router";
import {
  createEffect,
  createMemo,
  createResource,
  createSignal,
  For,
  type JSX,
  onCleanup,
  Show,
} from "solid-js";
import { ApiError } from "../api/client";
import { api } from "../api/endpoints";
import type { AuditRow } from "../api/generated/AuditRow";
import type { OrgDetail } from "../api/generated/OrgDetail";
import type { Run } from "../api/generated/Run";
import { reauthenticate, useSession } from "../auth/session";
import {
  ActivityFeed,
  AsyncView,
  DataTable,
  DateRangePicker,
  LineChart,
  LoadMore,
  MetricCard,
  RealtimeIndicator,
  SearchInput,
} from "../components/data";
import { Icon } from "../components/icons";
import {
  Alert,
  Badge,
  Button,
  Card,
  ConfirmDialog,
  Dialog,
  EmptyState,
  ErrorState,
  Field,
  PageHeader,
  Skeleton,
  Tabs,
} from "../components/ui";
import { fmtDateTime, fmtNumber, fmtRelative } from "../lib/format";
import { subscribe } from "../realtime/events";
import { toast } from "../stores/toast";

const errMsg = (e: unknown) =>
  e instanceof ApiError ? (e.fieldErrors[0]?.message ?? e.friendly) : "Something went wrong.";

// ---------------------------------------------------------------- layout

const [currentOrg, setCurrentOrg] = createSignal<OrgDetail | undefined>();
const can = (p: string) => currentOrg()?.permissions.includes(p) ?? false;

export function OrgLayout(props: { children?: JSX.Element }) {
  const params = useParams();
  const [org, { refetch }] = createResource(() => params.slug, api.orgs.get);
  // Publish the loaded org (role + permissions) to child pages for UI decisions only.
  createEffect(() => setCurrentOrg(org()));
  onCleanup(() => setCurrentOrg(undefined));
  const detail = () => org();
  const base = () => `/org/${params.slug}`;
  return (
    <Show when={!org.error} fallback={<ErrorState error={org.error} retry={refetch} />}>
      <Show when={detail()} fallback={<Skeleton lines={6} height="18px" />}>
        {(o) => (
          <div class="stack">
            <div class="row-between wrap">
              <div class="row">
                <span class="avatar" aria-hidden="true">
                  <Icon name="building" size={16} />
                </span>
                <div class="stack-sm gap-0">
                  <strong>{o().organization.name}</strong>
                  <span class="subtle">
                    /{o().organization.slug} · your role:{" "}
                    <Badge tone="brand">{o().role ?? "credential"}</Badge>
                  </span>
                </div>
              </div>
              <RealtimeIndicator />
            </div>
            <Tabs
              label="Organization sections"
              items={[
                { href: base(), label: "Overview", end: true },
                { href: `${base()}/runs`, label: "Runs", hidden: !can("runs:read") },
                { href: `${base()}/members`, label: "Members", hidden: !can("members:read") },
                { href: `${base()}/teams`, label: "Teams", hidden: !can("teams:read") },
                { href: `${base()}/roles`, label: "Roles", hidden: !can("roles:read") },
                { href: `${base()}/api-keys`, label: "API keys", hidden: !can("api_keys:read") },
                { href: `${base()}/audit`, label: "Audit", hidden: !can("audit:read") },
                {
                  href: `${base()}/settings`,
                  label: "Settings",
                  hidden: !(can("org:update") || can("settings:manage")),
                },
                { href: `${base()}/billing`, label: "Billing", hidden: !can("billing:read") },
              ]}
            />
            {props.children}
          </div>
        )}
      </Show>
    </Show>
  );
}

// ---------------------------------------------------------------- overview

export function OrgOverviewPage() {
  const params = useParams();
  const [days, setDays] = createSignal(14);
  const [ov, { refetch }] = createResource(
    () => [params.slug, days()] as const,
    ([s, d]) => api.orgs.overview(s as string, d),
  );
  const off = subscribe((e) => {
    if (e.type === "run_finished" || e.type === "run_created") refetch();
  });
  onCleanup(off);
  return (
    <>
      <PageHeader title="Overview" actions={<DateRangePicker value={days()} onChange={setDays} />} />
      <AsyncView data={ov} retry={refetch}>
        {(o) => (
          <div class="stack">
            <Show when={o.widgets.runs}>
              {(r) => (
                <div class="grid-cards">
                  <MetricCard
                    label="Runs"
                    value={r().total}
                    hint={`last ${o.days} days`}
                    spark={o.widgets.usage?.map((u) => u.succeeded + u.failed)}
                  />
                  <MetricCard label="Calls succeeded" value={r().calls_succeeded} />
                  <MetricCard label="Calls failed" value={r().calls_failed} />
                  <MetricCard label="In progress" value={r().running + r().queued} />
                </div>
              )}
            </Show>
            <div class="grid-2">
              <Show when={o.widgets.usage}>
                {(u) => (
                  <Card title="Usage">
                    <LineChart
                      title="Provider calls per day"
                      labels={u().map((p) => p.date)}
                      series={[
                        { name: "Succeeded", values: u().map((p) => p.succeeded) },
                        { name: "Failed", values: u().map((p) => p.failed), color: "var(--chart-4)" },
                      ]}
                      area
                    />
                  </Card>
                )}
              </Show>
              <Card title="Organization">
                <ul class="feed">
                  <Show when={o.widgets.members != null}>
                    <li>
                      <span class="grow">Members</span>
                      <A href={`/org/${params.slug}/members`}>{fmtNumber(o.widgets.members)}</A>
                    </li>
                  </Show>
                  <Show when={o.widgets.pending_invitations != null}>
                    <li>
                      <span class="grow">Pending invitations</span>
                      <span>{fmtNumber(o.widgets.pending_invitations)}</span>
                    </li>
                  </Show>
                  <li>
                    <span class="grow">Plan</span>
                    <Badge>{o.organization.billing_plan}</Badge>
                  </li>
                  <li>
                    <span class="grow">Created</span>
                    <span class="subtle">{fmtDateTime(o.organization.created_at)}</span>
                  </li>
                </ul>
              </Card>
            </div>
            <Show when={o.widgets.recent_audit}>
              {(a) => (
                <Card title="Recent audit events">
                  <ActivityFeed items={a()} />
                </Card>
              )}
            </Show>
          </div>
        )}
      </AsyncView>
    </>
  );
}

// ---------------------------------------------------------------- runs (example app)

const statusTone = (s: string) =>
  (s === "completed" ? "success" : s === "failed" ? "danger" : s === "running" ? "info" : "neutral") as
    | "success"
    | "danger"
    | "info"
    | "neutral";

export function RunsPage() {
  const params = useParams();
  const [status, setStatus] = createSignal("");
  const [runs, setRuns] = createSignal<Run[]>([]);
  const [cursor, setCursor] = createSignal<string | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);
  const [open, setOpen] = createSignal(false);
  const load = async (append = false) => {
    setLoading(true);
    try {
      const p = await api.orgs.runs(params.slug as string, {
        status: status() || undefined,
        cursor: append ? (cursor() ?? undefined) : undefined,
      });
      setRuns(append ? [...runs(), ...p.items] : p.items);
      setCursor(p.next_cursor);
      setError(null);
    } catch (e) {
      setError(e);
    } finally {
      setLoading(false);
    }
  };
  load();
  // Live progress: patch rows in place from realtime events (no polling).
  const off = subscribe((e) => {
    if (e.type === "run_created") load();
    if (e.type === "run_progress" || e.type === "run_finished") {
      setRuns((list) =>
        list.map((r) =>
          r.id === e.run_id
            ? {
                ...r,
                succeeded: e.succeeded,
                failed: e.failed,
                status: e.type === "run_finished" ? e.status : "running",
              }
            : r,
        ),
      );
    }
  });
  onCleanup(off);
  return (
    <>
      <PageHeader
        title="Runs"
        description="Batches of external API calls executed by background workers through the outbound API engine."
        actions={
          <>
            <label class="sr-only" for="status-filter">
              Status
            </label>
            <select
              id="status-filter"
              class="select w-auto"
              value={status()}
              onChange={(e) => {
                setStatus(e.currentTarget.value);
                load();
              }}
            >
              <option value="">All statuses</option>
              <option value="queued">Queued</option>
              <option value="running">Running</option>
              <option value="completed">Completed</option>
              <option value="failed">Failed</option>
            </select>
            <Show when={can("runs:create")}>
              <Button variant="primary" icon="plus" onClick={() => setOpen(true)}>
                New run
              </Button>
            </Show>
          </>
        }
      />
      <Card flush>
        <Show when={!error()} fallback={<ErrorState error={error()} retry={() => load()} />}>
          <Show
            when={!(loading() && runs().length === 0)}
            fallback={
              <div class="card-body">
                <Skeleton lines={5} />
              </div>
            }
          >
            <DataTable
              caption="Runs"
              rows={runs()}
              rowKey={(r) => r.id}
              empty={
                <EmptyState
                  icon="play"
                  title="No runs yet"
                  body="Start a run to send calls through the provider engine."
                />
              }
              columns={[
                {
                  key: "label",
                  header: "Run",
                  cell: (r) => <strong>{r.label}</strong>,
                  sortValue: (r) => r.label,
                },
                { key: "provider", header: "Provider", cell: (r) => <code>{r.provider}</code> },
                {
                  key: "status",
                  header: "Status",
                  cell: (r) => <Badge tone={statusTone(r.status)}>{r.status}</Badge>,
                  sortValue: (r) => r.status,
                },
                {
                  key: "progress",
                  header: "Progress",
                  cell: (r) => {
                    const done = () => r.succeeded + r.failed;
                    return (
                      <div class="stack-sm gap-1 minw-160">
                        <progress
                          class="w-full"
                          max={r.requested}
                          value={done()}
                          aria-label={`${done()} of ${r.requested} calls done`}
                        />
                        <span class="subtle num">
                          {fmtNumber(r.succeeded)} ok · {fmtNumber(r.failed)} failed ·{" "}
                          {fmtNumber(r.requested)} total
                        </span>
                      </div>
                    );
                  },
                },
                {
                  key: "created",
                  header: "Created",
                  cell: (r) => fmtRelative(r.created_at),
                  sortValue: (r) => r.created_at,
                },
                {
                  key: "act",
                  header: "",
                  align: "right",
                  cell: (r) => (
                    <Button
                      size="sm"
                      variant="ghost"
                      aria-label={`Delete run ${r.label}`}
                      onClick={async () => {
                        try {
                          await api.orgs.deleteRun(params.slug as string, r.id);
                          setRuns(runs().filter((x) => x.id !== r.id));
                        } catch (e) {
                          toast("Not deleted", { body: errMsg(e), tone: "danger" });
                        }
                      }}
                    >
                      <Icon name="trash" size={16} />
                    </Button>
                  ),
                },
              ]}
            />
            <LoadMore hasMore={!!cursor()} loading={loading()} onMore={() => load(true)} />
          </Show>
        </Show>
      </Card>
      <NewRunDialog open={open()} onClose={() => setOpen(false)} onCreated={() => load()} />
    </>
  );
}

function NewRunDialog(props: { open: boolean; onClose: () => void; onCreated: () => void }) {
  const params = useParams();
  const [label, setLabel] = createSignal("Smoke test");
  const [count, setCount] = createSignal(25);
  const [err, setErr] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const submit = async () => {
    setBusy(true);
    setErr(null);
    try {
      await api.orgs.createRun(params.slug as string, {
        label: label(),
        provider: "simulated",
        requested: count(),
      });
      toast("Run queued", { tone: "success" });
      props.onCreated();
      props.onClose();
    } catch (e) {
      setErr(errMsg(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      open={props.open}
      title="New run"
      onClose={props.onClose}
      footer={
        <>
          <Button onClick={props.onClose}>Cancel</Button>
          <Button variant="primary" loading={busy()} onClick={submit}>
            Start run
          </Button>
        </>
      }
    >
      <Show when={err()}>
        <Alert tone="danger">{err()}</Alert>
      </Show>
      <Field label="Label">
        {(a) => (
          <input id={a.id} class="input" value={label()} onInput={(e) => setLabel(e.currentTarget.value)} />
        )}
      </Field>
      <Field label="Calls" hint="Between 1 and 10,000">
        {(a) => (
          <input
            id={a.id}
            class="input"
            type="number"
            min="1"
            max="10000"
            aria-describedby={a.describedBy}
            value={count()}
            onInput={(e) => setCount(Number(e.currentTarget.value))}
          />
        )}
      </Field>
    </Dialog>
  );
}

// ---------------------------------------------------------------- members

export function MembersPage() {
  const params = useParams();
  const { session } = useSession();
  const [members, { refetch }] = createResource(() => params.slug, api.orgs.members);
  const [roles] = createResource(() => params.slug, api.orgs.roles);
  const [invites, { refetch: refetchInvites }] = createResource(
    () => (can("members:invite") ? params.slug : false),
    (s) => api.orgs.invitations(s as string),
  );
  const [q, setQ] = createSignal("");
  const [inviteOpen, setInviteOpen] = createSignal(false);
  const [removing, setRemoving] = createSignal<{ id: string; name: string } | null>(null);
  const me = () => session()?.user?.id;
  const changeRole = async (userId: string, roleId: string) => {
    try {
      await api.orgs.changeRole(params.slug as string, userId, roleId);
      toast("Role updated", { tone: "success" });
    } catch (e) {
      toast("Role not changed", { body: errMsg(e), tone: "danger" });
    }
    refetch();
  };
  return (
    <>
      <PageHeader
        title="Members"
        actions={
          <>
            <SearchInput label="Search members" value={q()} onInput={setQ} />
            <Show when={can("members:invite")}>
              <Button variant="primary" icon="mail" onClick={() => setInviteOpen(true)}>
                Invite
              </Button>
            </Show>
          </>
        }
      />
      <Card flush>
        <AsyncView data={members} retry={refetch}>
          {(m) => (
            <DataTable
              caption="Members"
              rows={m.items}
              rowKey={(r) => r.user_id}
              filterText={q()}
              columns={[
                {
                  key: "name",
                  header: "Member",
                  sortValue: (r) => `${r.display_name} ${r.email}`,
                  cell: (r) => (
                    <span class="stack-sm gap-0">
                      <strong>
                        {r.display_name}{" "}
                        <Show when={r.user_id === me()}>
                          <Badge>you</Badge>
                        </Show>
                      </strong>
                      <span class="subtle">{r.email}</span>
                    </span>
                  ),
                },
                {
                  key: "role",
                  header: "Role",
                  sortValue: (r) => r.role_key,
                  cell: (r) => (
                    <Show
                      when={can("members:update_role") && r.user_id !== me()}
                      fallback={<Badge tone="brand">{r.role_name}</Badge>}
                    >
                      <label class="sr-only" for={`role-${r.user_id}`}>
                        Role for {r.display_name}
                      </label>
                      <select
                        id={`role-${r.user_id}`}
                        class="select w-auto"
                        value={r.role_id}
                        onChange={(e) => changeRole(r.user_id, e.currentTarget.value)}
                      >
                        <For each={roles()?.items}>
                          {(role) => <option value={role.id}>{role.name}</option>}
                        </For>
                      </select>
                    </Show>
                  ),
                },
                {
                  key: "joined",
                  header: "Joined",
                  sortValue: (r) => r.joined_at,
                  cell: (r) => fmtRelative(r.joined_at),
                },
                { key: "active", header: "Last active", cell: (r) => fmtRelative(r.last_login_at) },
                {
                  key: "act",
                  header: "",
                  align: "right",
                  cell: (r) => (
                    <Show when={can("members:remove") && r.user_id !== me()}>
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => setRemoving({ id: r.user_id, name: r.display_name })}
                      >
                        Remove
                      </Button>
                    </Show>
                  ),
                },
              ]}
            />
          )}
        </AsyncView>
      </Card>
      <Show when={(invites()?.items.length ?? 0) > 0}>
        <Card title="Pending invitations" flush>
          <DataTable
            caption="Pending invitations"
            rows={invites()?.items ?? []}
            rowKey={(i) => i.id}
            columns={[
              { key: "email", header: "Email", cell: (i) => i.email },
              { key: "role", header: "Role", cell: (i) => <Badge>{i.role_key}</Badge> },
              { key: "exp", header: "Expires", cell: (i) => fmtRelative(i.expires_at) },
              {
                key: "act",
                header: "",
                align: "right",
                cell: (i) => (
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={async () => {
                      await api.orgs.revokeInvitation(params.slug as string, i.id);
                      refetchInvites();
                    }}
                  >
                    Revoke
                  </Button>
                ),
              },
            ]}
          />
        </Card>
      </Show>
      <InviteDialog
        open={inviteOpen()}
        onClose={() => setInviteOpen(false)}
        roles={roles()?.items ?? []}
        onInvited={refetchInvites}
      />
      <ConfirmDialog
        open={!!removing()}
        title="Remove member?"
        body={<>{removing()?.name} will lose access to this organization immediately.</>}
        confirmLabel="Remove"
        danger
        onConfirm={async () => {
          try {
            await api.orgs.removeMember(params.slug as string, removing()?.id as string);
            toast("Member removed", { tone: "success" });
            refetch();
          } catch (e) {
            toast("Not removed", { body: errMsg(e), tone: "danger" });
          }
          setRemoving(null);
        }}
        onClose={() => setRemoving(null)}
      />
    </>
  );
}

function InviteDialog(props: {
  open: boolean;
  onClose: () => void;
  roles: { id: string; name: string; key: string }[];
  onInvited: () => void;
}) {
  const params = useParams();
  const [email, setEmail] = createSignal("");
  const [role, setRole] = createSignal("");
  const [link, setLink] = createSignal<string | null>(null);
  const [err, setErr] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const roleId = () => role() || props.roles.find((r) => r.key === "member")?.id || props.roles[0]?.id || "";
  const send = async () => {
    setBusy(true);
    setErr(null);
    try {
      const r = await api.orgs.invite(params.slug as string, { email: email(), role_id: roleId() });
      setLink(r.link);
      props.onInvited();
    } catch (e) {
      setErr(errMsg(e));
    } finally {
      setBusy(false);
    }
  };
  const close = () => {
    setLink(null);
    setEmail("");
    props.onClose();
  };
  return (
    <Dialog
      open={props.open}
      title="Invite a member"
      onClose={close}
      footer={
        <Show
          when={!link()}
          fallback={
            <Button variant="primary" onClick={close}>
              Done
            </Button>
          }
        >
          <Button onClick={close}>Cancel</Button>
          <Button variant="primary" loading={busy()} onClick={send}>
            Send invitation
          </Button>
        </Show>
      }
    >
      <Show
        when={!link()}
        fallback={
          <div class="stack-sm">
            <Alert tone="success" title="Invitation created">
              Share this one-time link with {email()}. It's shown only once.
            </Alert>
            <input
              class="input mono"
              readOnly
              value={link() ?? ""}
              aria-label="Invitation link"
              onFocus={(e) => e.currentTarget.select()}
            />
          </div>
        }
      >
        <Show when={err()}>
          <Alert tone="danger">{err()}</Alert>
        </Show>
        <Field label="Email">
          {(a) => (
            <input
              id={a.id}
              class="input"
              type="email"
              value={email()}
              onInput={(e) => setEmail(e.currentTarget.value)}
            />
          )}
        </Field>
        <Field label="Role" hint="You can only grant roles with permissions you hold.">
          {(a) => (
            <select
              id={a.id}
              class="select"
              aria-describedby={a.describedBy}
              value={roleId()}
              onChange={(e) => setRole(e.currentTarget.value)}
            >
              <For each={props.roles}>{(r) => <option value={r.id}>{r.name}</option>}</For>
            </select>
          )}
        </Field>
      </Show>
    </Dialog>
  );
}

// ---------------------------------------------------------------- teams

export function TeamsPage() {
  const params = useParams();
  const [teams, { refetch }] = createResource(() => params.slug, api.orgs.teams);
  const [name, setName] = createSignal("");
  const create = async (e: SubmitEvent) => {
    e.preventDefault();
    try {
      await api.orgs.createTeam(params.slug as string, { name: name() });
      setName("");
      refetch();
    } catch (err) {
      toast("Team not created", { body: errMsg(err), tone: "danger" });
    }
  };
  return (
    <>
      <PageHeader title="Teams" description="Group members for ownership and notifications." />
      <Show when={can("teams:manage")}>
        <Card>
          <form class="row wrap" onSubmit={create}>
            <label class="sr-only" for="team-name">
              Team name
            </label>
            <input
              id="team-name"
              class="input maxw-320"
              placeholder="New team name"
              value={name()}
              onInput={(e) => setName(e.currentTarget.value)}
              required
            />
            <Button type="submit" variant="primary" icon="plus">
              Create team
            </Button>
          </form>
        </Card>
      </Show>
      <Card flush>
        <AsyncView
          data={teams}
          retry={refetch}
          empty={(t) => t.items.length === 0}
          emptyView={<EmptyState icon="users" title="No teams yet" />}
        >
          {(t) => (
            <DataTable
              caption="Teams"
              rows={t.items}
              rowKey={(r) => r.id}
              columns={[
                {
                  key: "name",
                  header: "Team",
                  cell: (r) => <strong>{r.name}</strong>,
                  sortValue: (r) => r.name,
                },
                {
                  key: "members",
                  header: "Members",
                  cell: (r) => fmtNumber(r.member_count),
                  sortValue: (r) => r.member_count,
                },
                { key: "created", header: "Created", cell: (r) => fmtRelative(r.created_at) },
                {
                  key: "act",
                  header: "",
                  align: "right",
                  cell: (r) => (
                    <Show when={can("teams:manage")}>
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={async () => {
                          await api.orgs.deleteTeam(params.slug as string, r.id);
                          refetch();
                        }}
                      >
                        Delete
                      </Button>
                    </Show>
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

// ---------------------------------------------------------------- roles

export function RolesPage() {
  const params = useParams();
  const [roles, { refetch }] = createResource(() => params.slug, api.orgs.roles);
  const [catalog] = createResource(api.permissions);
  const [open, setOpen] = createSignal(false);
  return (
    <>
      <PageHeader
        title="Roles & permissions"
        description="Built-in roles are fixed; custom roles can combine any permissions you hold (except owner-only powers)."
        actions={
          <Show when={can("roles:manage")}>
            <Button variant="primary" icon="plus" onClick={() => setOpen(true)}>
              Custom role
            </Button>
          </Show>
        }
      />
      <AsyncView data={roles} retry={refetch}>
        {(r) => (
          <div class="grid-2">
            <For each={r.items}>
              {(role) => (
                <Card
                  title={
                    <span class="row">
                      {role.name}{" "}
                      <Badge tone={role.builtin ? "neutral" : "brand"}>
                        {role.builtin ? "built-in" : "custom"}
                      </Badge>
                    </span>
                  }
                  actions={
                    <Show when={!role.builtin && can("roles:manage")}>
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={async () => {
                          try {
                            await api.orgs.deleteRole(params.slug as string, role.id);
                            refetch();
                          } catch (e) {
                            toast("Not deleted", { body: errMsg(e), tone: "danger" });
                          }
                        }}
                      >
                        Delete
                      </Button>
                    </Show>
                  }
                >
                  <p class="subtle">{role.description || "—"}</p>
                  <p class="subtle">{role.member_count} member(s)</p>
                  <div class="row wrap gap-6 mt-2">
                    <For each={role.permissions}>{(p) => <code class="badge">{p}</code>}</For>
                  </div>
                </Card>
              )}
            </For>
          </div>
        )}
      </AsyncView>
      <CustomRoleDialog
        open={open()}
        onClose={() => setOpen(false)}
        catalog={catalog()?.items ?? []}
        onCreated={refetch}
      />
    </>
  );
}

function CustomRoleDialog(props: {
  open: boolean;
  onClose: () => void;
  catalog: { key: string; description: string; owner_only: boolean }[];
  onCreated: () => void;
}) {
  const params = useParams();
  const [name, setName] = createSignal("");
  const [perms, setPerms] = createSignal<string[]>([]);
  const [err, setErr] = createSignal<string | null>(null);
  const available = () => props.catalog.filter((p) => !p.owner_only && can(p.key));
  const create = async () => {
    try {
      const key = name()
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "_")
        .replace(/^_|_$/g, "");
      await api.orgs.createRole(params.slug as string, { key, name: name(), permissions: perms() });
      props.onCreated();
      props.onClose();
      setName("");
      setPerms([]);
    } catch (e) {
      setErr(errMsg(e));
    }
  };
  return (
    <Dialog
      open={props.open}
      title="Create custom role"
      onClose={props.onClose}
      footer={
        <>
          <Button onClick={props.onClose}>Cancel</Button>
          <Button variant="primary" onClick={create}>
            Create role
          </Button>
        </>
      }
    >
      <Show when={err()}>
        <Alert tone="danger">{err()}</Alert>
      </Show>
      <Field label="Role name">
        {(a) => (
          <input id={a.id} class="input" value={name()} onInput={(e) => setName(e.currentTarget.value)} />
        )}
      </Field>
      <fieldset class="stack-sm fieldset-reset scroll-260">
        <legend class="label">Permissions</legend>
        <For each={available()}>
          {(p) => (
            <label class="checkbox">
              <input
                type="checkbox"
                checked={perms().includes(p.key)}
                onChange={(e) =>
                  setPerms(e.currentTarget.checked ? [...perms(), p.key] : perms().filter((x) => x !== p.key))
                }
              />
              <span>
                <code>{p.key}</code> <span class="subtle">{p.description}</span>
              </span>
            </label>
          )}
        </For>
      </fieldset>
    </Dialog>
  );
}

// ---------------------------------------------------------------- settings

export function OrgSettingsPage() {
  const params = useParams();
  const navigate = useNavigate();
  const { refetch: refetchSession } = useSession();
  const [name, setName] = createSignal<string | null>(null);
  const [confirmDelete, setConfirmDelete] = createSignal(false);
  const org = () => currentOrg()?.organization;
  const save = async (e: SubmitEvent) => {
    e.preventDefault();
    try {
      await api.orgs.update(params.slug as string, { name: name() ?? org()?.name });
      toast("Saved", { tone: "success" });
      refetchSession();
    } catch (err) {
      toast("Not saved", { body: errMsg(err), tone: "danger" });
    }
  };
  const leave = async () => {
    try {
      await api.orgs.leave(params.slug as string);
      refetchSession();
      navigate("/dashboard");
    } catch (e) {
      toast("Couldn't leave", { body: errMsg(e), tone: "danger" });
    }
  };
  const remove = async () => {
    try {
      await api.orgs.remove(params.slug as string);
      refetchSession();
      navigate("/dashboard");
    } catch (e) {
      if (e instanceof ApiError && e.code === "reauth_required") reauthenticate();
      else toast("Not deleted", { body: errMsg(e), tone: "danger" });
    }
    setConfirmDelete(false);
  };
  return (
    <>
      <PageHeader title="Settings" />
      <div class="stack">
        <Show when={can("org:update")}>
          <Card title="General">
            <form class="stack maxw-480" onSubmit={save}>
              <Field label="Organization name">
                {(a) => (
                  <input
                    id={a.id}
                    class="input"
                    value={name() ?? org()?.name ?? ""}
                    onInput={(e) => setName(e.currentTarget.value)}
                  />
                )}
              </Field>
              <Field label="URL" hint="The organization URL cannot be changed.">
                {(a) => (
                  <input
                    id={a.id}
                    class="input mono"
                    value={`/org/${org()?.slug ?? ""}`}
                    readOnly
                    aria-describedby={a.describedBy}
                  />
                )}
              </Field>
              <div>
                <Button type="submit" variant="primary">
                  Save
                </Button>
              </div>
            </form>
          </Card>
        </Show>
        <Show when={!org()?.personal}>
          <Card title="Danger zone">
            <div class="stack">
              <div class="row-between wrap">
                <div class="stack-sm">
                  <strong>Leave organization</strong>
                  <span class="subtle">You'll lose access. The last owner can't leave.</span>
                </div>
                <Button onClick={leave}>Leave</Button>
              </div>
              <Show when={can("org:delete")}>
                <hr class="divider" />
                <div class="row-between wrap">
                  <div class="stack-sm">
                    <strong>Delete organization</strong>
                    <span class="subtle">All members lose access. Requires recent sign-in.</span>
                  </div>
                  <Button variant="danger" onClick={() => setConfirmDelete(true)}>
                    Delete
                  </Button>
                </div>
              </Show>
            </div>
          </Card>
        </Show>
      </div>
      <ConfirmDialog
        open={confirmDelete()}
        title="Delete organization?"
        body={<>This permanently removes access for every member.</>}
        confirmLabel="Delete organization"
        danger
        requireText={org()?.slug}
        onConfirm={remove}
        onClose={() => setConfirmDelete(false)}
      />
    </>
  );
}

// ---------------------------------------------------------------- audit

export function AuditLogView(props: {
  fetch: (q: {
    action?: string;
    outcome?: string;
    cursor?: string;
  }) => Promise<{ items: AuditRow[]; next_cursor: string | null }>;
}) {
  const [items, setItems] = createSignal<AuditRow[]>([]);
  const [cursor, setCursor] = createSignal<string | null>(null);
  const [action, setAction] = createSignal("");
  const [outcome, setOutcome] = createSignal("");
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);
  const load = async (append = false) => {
    setLoading(true);
    try {
      const p = await props.fetch({
        action: action() || undefined,
        outcome: outcome() || undefined,
        cursor: append ? (cursor() ?? undefined) : undefined,
      });
      setItems(append ? [...items(), ...p.items] : p.items);
      setCursor(p.next_cursor);
      setError(null);
    } catch (e) {
      setError(e);
    } finally {
      setLoading(false);
    }
  };
  load();
  return (
    <div class="stack">
      <div class="row wrap">
        <SearchInput
          label="Filter by action prefix"
          placeholder="e.g. security. or role."
          value={action()}
          onInput={(v) => {
            setAction(v);
            load();
          }}
        />
        <label class="sr-only" for="outcome-filter">
          Outcome
        </label>
        <select
          id="outcome-filter"
          class="select w-auto"
          value={outcome()}
          onChange={(e) => {
            setOutcome(e.currentTarget.value);
            load();
          }}
        >
          <option value="">All outcomes</option>
          <option value="success">Success</option>
          <option value="denied">Denied</option>
          <option value="failure">Failure</option>
        </select>
      </div>
      <Card flush>
        <Show when={!error()} fallback={<ErrorState error={error()} retry={() => load()} />}>
          <DataTable
            caption="Audit events"
            rows={items()}
            rowKey={(e) => e.id}
            pageSize={1000}
            empty={<EmptyState icon="list" title={loading() ? "Loading…" : "No matching events"} />}
            columns={[
              {
                key: "time",
                header: "Time",
                cell: (e) => <time datetime={e.occurred_at}>{fmtDateTime(e.occurred_at)}</time>,
              },
              {
                key: "actor",
                header: "Actor",
                cell: (e) => (
                  <span class="stack-sm gap-0">
                    <span>{e.actor_label ?? "—"}</span>
                    <span class="subtle">{e.actor_type}</span>
                  </span>
                ),
              },
              { key: "action", header: "Action", cell: (e) => <code>{e.action}</code> },
              {
                key: "outcome",
                header: "Outcome",
                cell: (e) => (
                  <Badge
                    tone={e.outcome === "success" ? "success" : e.outcome === "denied" ? "danger" : "warning"}
                  >
                    {e.outcome}
                  </Badge>
                ),
              },
              {
                key: "target",
                header: "Target",
                cell: (e) =>
                  e.target_type ? (
                    <span class="subtle mono">
                      {e.target_type}:{e.target_id?.slice(0, 8)}
                    </span>
                  ) : (
                    "—"
                  ),
              },
              {
                key: "req",
                header: "Request",
                cell: (e) => <span class="subtle mono">{e.request_id?.slice(0, 8) ?? "—"}</span>,
              },
            ]}
          />
          <LoadMore hasMore={!!cursor()} loading={loading()} onMore={() => load(true)} />
        </Show>
      </Card>
    </div>
  );
}

export function OrgAuditPage() {
  const params = useParams();
  return (
    <>
      <PageHeader
        title="Audit log"
        description="Append-only record of security-relevant actions in this organization."
      />
      <AuditLogView fetch={(q) => api.orgs.audit(params.slug as string, q)} />
    </>
  );
}

// ---------------------------------------------------------------- API keys

export function ApiKeysPage() {
  const params = useParams();
  const [keys, { refetch }] = createResource(() => params.slug, api.orgs.apiKeys);
  const [catalog] = createResource(api.permissions);
  const [open, setOpen] = createSignal(false);
  const [secret, setSecret] = createSignal<string | null>(null);
  const [name, setName] = createSignal("");
  const [scopes, setScopes] = createSignal<string[]>(["runs:read"]);
  const [err, setErr] = createSignal<string | null>(null);
  const assignable = createMemo(() =>
    (catalog()?.items ?? []).filter((p) => p.credential_assignable && can(p.key)),
  );
  const create = async () => {
    setErr(null);
    try {
      const r = await api.orgs.createApiKey(params.slug as string, { name: name(), scopes: scopes() });
      setSecret(r.key);
      refetch();
    } catch (e) {
      setErr(errMsg(e));
    }
  };
  const close = () => {
    setOpen(false);
    setSecret(null);
    setName("");
  };
  return (
    <>
      <PageHeader
        title="API keys"
        description="Organization credentials for programmatic access. Keys never exceed their creator's permissions."
        actions={
          <Show when={can("api_keys:manage")}>
            <Button variant="primary" icon="plus" onClick={() => setOpen(true)}>
              Create key
            </Button>
          </Show>
        }
      />
      <Card flush>
        <AsyncView
          data={keys}
          retry={refetch}
          empty={(k) => k.items.length === 0}
          emptyView={<EmptyState icon="key" title="No API keys" />}
        >
          {(k) => (
            <DataTable
              caption="API keys"
              rows={k.items}
              rowKey={(r) => r.id}
              columns={[
                {
                  key: "name",
                  header: "Name",
                  cell: (r) => <strong>{r.name}</strong>,
                  sortValue: (r) => r.name,
                },
                { key: "id", header: "Key ID", cell: (r) => <code>{r.key_id}</code> },
                {
                  key: "scopes",
                  header: "Scopes",
                  cell: (r) => (
                    <span class="row wrap gap-1">
                      <For each={r.scopes}>{(s) => <code class="badge">{s}</code>}</For>
                    </span>
                  ),
                },
                { key: "used", header: "Last used", cell: (r) => fmtRelative(r.last_used_at) },
                {
                  key: "state",
                  header: "Status",
                  cell: (r) =>
                    r.revoked_at ? (
                      <Badge tone="danger">revoked</Badge>
                    ) : r.expires_at && new Date(r.expires_at) < new Date() ? (
                      <Badge tone="warning">expired</Badge>
                    ) : (
                      <Badge tone="success">active</Badge>
                    ),
                },
                {
                  key: "act",
                  header: "",
                  align: "right",
                  cell: (r) => (
                    <Show when={can("api_keys:manage") && !r.revoked_at}>
                      <span class="row">
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={async () => {
                            try {
                              const x = await api.orgs.rotateApiKey(params.slug as string, r.id);
                              setSecret(x.key);
                              setOpen(true);
                              refetch();
                            } catch (e) {
                              toast("Not rotated", { body: errMsg(e), tone: "danger" });
                            }
                          }}
                        >
                          Rotate
                        </Button>
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={async () => {
                            await api.orgs.revokeApiKey(params.slug as string, r.id);
                            refetch();
                          }}
                        >
                          Revoke
                        </Button>
                      </span>
                    </Show>
                  ),
                },
              ]}
            />
          )}
        </AsyncView>
      </Card>
      <Dialog
        open={open()}
        title={secret() ? "Copy your API key" : "Create API key"}
        onClose={close}
        footer={
          <Show
            when={!secret()}
            fallback={
              <Button variant="primary" onClick={close}>
                I've stored it safely
              </Button>
            }
          >
            <Button onClick={close}>Cancel</Button>
            <Button variant="primary" onClick={create}>
              Create
            </Button>
          </Show>
        }
      >
        <Show
          when={!secret()}
          fallback={
            <div class="stack-sm">
              <Alert tone="warning" title="Shown only once">
                Store this key in a secret manager now. You won't be able to see it again.
              </Alert>
              <input
                class="input mono"
                readOnly
                value={secret() ?? ""}
                aria-label="API key"
                onFocus={(e) => e.currentTarget.select()}
              />
            </div>
          }
        >
          <Show when={err()}>
            <Alert tone="danger">{err()}</Alert>
          </Show>
          <Field label="Name">
            {(a) => (
              <input
                id={a.id}
                class="input"
                value={name()}
                onInput={(e) => setName(e.currentTarget.value)}
                placeholder="CI deploys"
              />
            )}
          </Field>
          <fieldset class="stack-sm fieldset-reset">
            <legend class="label">Scopes</legend>
            <For each={assignable()}>
              {(p) => (
                <label class="checkbox">
                  <input
                    type="checkbox"
                    checked={scopes().includes(p.key)}
                    onChange={(e) =>
                      setScopes(
                        e.currentTarget.checked ? [...scopes(), p.key] : scopes().filter((x) => x !== p.key),
                      )
                    }
                  />
                  <code>{p.key}</code>
                </label>
              )}
            </For>
          </fieldset>
        </Show>
      </Dialog>
    </>
  );
}

// ---------------------------------------------------------------- billing

export function BillingPage() {
  const params = useParams();
  const [b, { refetch }] = createResource(() => params.slug, api.orgs.billing);
  return (
    <>
      <PageHeader title="Billing" />
      <AsyncView data={b} retry={refetch}>
        {(x) => (
          <Card title="Plan">
            <div class="stack">
              <div class="row">
                <span class="metric-value capitalize">{x.plan}</span>
                <Badge>{x.provider ?? "no billing provider connected"}</Badge>
              </div>
              <p class="subtle">{x.note}</p>
            </div>
          </Card>
        )}
      </AsyncView>
    </>
  );
}
