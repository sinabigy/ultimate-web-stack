// System administration UI. Visibility here is convenience only: every endpoint independently
// requires a system permission (and MFA when configured). 403 mfa_required triggers step-up.
import { A, useNavigate, useParams } from "@solidjs/router";
import { createResource, createSignal, For, Show } from "solid-js";
import { ApiError } from "../api/client";
import { api } from "../api/endpoints";
import { AsyncView, BarChart, DataTable, MetricCard, Pagination, SearchInput } from "../components/data";
import { Alert, Badge, Button, Card, ConfirmDialog, PageHeader } from "../components/ui";
import { fmtDateTime, fmtNumber, fmtRelative } from "../lib/format";
import { toast } from "../stores/toast";
import { AuditLogView } from "./org";

const crumbs = (label?: string) => [
  { label: "Administration", href: "/admin" },
  ...(label ? [{ label }] : []),
];
const errMsg = (e: unknown) => (e instanceof ApiError ? e.friendly : "Something went wrong.");

export function AdminOverviewPage() {
  const [o, { refetch }] = createResource(api.admin.overview);
  return (
    <>
      <PageHeader
        title="Administration"
        breadcrumbs={crumbs()}
        description="Platform-wide state. Organization admins do not have access to this area."
      />
      <AsyncView data={o} retry={refetch}>
        {(d) => (
          <div class="stack">
            <div class="grid-cards">
              <MetricCard
                label="Users"
                value={d.counts.users}
                hint={`${fmtNumber(d.counts.active_users_7d)} active (7d)`}
              />
              <MetricCard label="Organizations" value={d.counts.organizations} />
              <MetricCard label="Active sessions" value={d.counts.active_sessions} />
              <MetricCard label="Denied requests (24h)" value={d.counts.denied_24h} />
              <MetricCard label="System admins" value={d.counts.system_admins} />
              <MetricCard
                label="DB pool"
                value={`${d.pool.size - d.pool.idle}/${d.pool.max}`}
                hint="in use / max"
              />
            </div>
            <Card title="Job queues">
              <Show when={d.jobs.length} fallback={<p class="subtle">No jobs yet.</p>}>
                <BarChart
                  title="Queued jobs per queue"
                  items={d.jobs.map((q) => ({ label: q.queue, value: Number(q.queued) }))}
                />
              </Show>
            </Card>
          </div>
        )}
      </AsyncView>
    </>
  );
}

export function AdminUsersPage() {
  const navigate = useNavigate();
  const [search, setSearch] = createSignal("");
  const [status, setStatus] = createSignal("");
  const [page, setPage] = createSignal(1);
  const [data, { refetch }] = createResource(
    () => ({ search: search(), status: status(), page: page() }),
    (q) =>
      api.admin.users({
        search: q.search || undefined,
        status: q.status || undefined,
        page: q.page,
        per_page: 25,
      }),
  );
  return (
    <>
      <PageHeader
        title="Users"
        breadcrumbs={crumbs("Users")}
        actions={
          <>
            <SearchInput
              label="Search users"
              value={search()}
              onInput={(v) => {
                setSearch(v);
                setPage(1);
              }}
            />
            <label class="sr-only" for="user-status">
              Status
            </label>
            <select
              id="user-status"
              class="select w-auto"
              value={status()}
              onChange={(e) => {
                setStatus(e.currentTarget.value);
                setPage(1);
              }}
            >
              <option value="">All statuses</option>
              <option value="active">Active</option>
              <option value="suspended">Suspended</option>
              <option value="deleted">Deleted</option>
            </select>
          </>
        }
      />
      <Card flush>
        <AsyncView data={data} retry={refetch}>
          {(d) => (
            <>
              <DataTable
                caption="Users"
                rows={d.items}
                rowKey={(u) => u.id}
                onRowClick={(u) => navigate(`/admin/users/${u.id}`)}
                pageSize={1000}
                columns={[
                  {
                    key: "user",
                    header: "User",
                    cell: (u) => (
                      <A href={`/admin/users/${u.id}`} class="stack-sm gap-0">
                        <strong>{u.display_name}</strong>
                        <span class="subtle">{u.email}</span>
                      </A>
                    ),
                  },
                  {
                    key: "status",
                    header: "Status",
                    cell: (u) => (
                      <Badge tone={u.status === "active" ? "success" : "danger"}>{u.status}</Badge>
                    ),
                  },
                  {
                    key: "role",
                    header: "System role",
                    cell: (u) =>
                      u.system_role === "none" ? (
                        <span class="subtle">—</span>
                      ) : (
                        <Badge tone="brand">{u.system_role}</Badge>
                      ),
                  },
                  { key: "orgs", header: "Orgs", align: "right", cell: (u) => fmtNumber(u.org_count) },
                  { key: "login", header: "Last sign-in", cell: (u) => fmtRelative(u.last_login_at) },
                ]}
              />
              <Pagination
                page={d.page}
                pages={Math.max(1, Math.ceil(d.total / d.per_page))}
                total={d.total}
                onPage={setPage}
              />
            </>
          )}
        </AsyncView>
      </Card>
    </>
  );
}

export function AdminUserDetailPage() {
  const params = useParams();
  const [d, { refetch }] = createResource(() => params.id, api.admin.user);
  const [confirm, setConfirm] = createSignal<null | "suspend" | "activate" | "sessions">(null);
  const act = async () => {
    try {
      if (confirm() === "sessions") {
        const r = await api.admin.revokeUserSessions(params.id as string);
        toast(`Revoked ${r.count} session(s)`, { tone: "success" });
      } else {
        await api.admin.updateUser(params.id as string, {
          status: confirm() === "suspend" ? "suspended" : "active",
        });
        toast("User updated", { tone: "success" });
      }
      refetch();
    } catch (e) {
      toast("Action failed", { body: errMsg(e), tone: "danger" });
    }
    setConfirm(null);
  };
  const setRole = async (role: string) => {
    try {
      await api.admin.updateUser(params.id as string, { system_role: role });
      toast("System role updated; the user must sign in again", { tone: "success" });
      refetch();
    } catch (e) {
      toast("Role not changed", { body: errMsg(e), tone: "danger" });
    }
  };
  return (
    <AsyncView data={d} retry={refetch}>
      {(x) => (
        <>
          <PageHeader
            title={x.user.display_name}
            breadcrumbs={[
              ...crumbs("Users").map((c) =>
                c.label === "Users" ? { label: "Users", href: "/admin/users" } : c,
              ),
              { label: x.user.email },
            ]}
            actions={
              <>
                <Button onClick={() => setConfirm("sessions")}>Revoke sessions</Button>
                <Show
                  when={x.user.status === "active"}
                  fallback={
                    <Button variant="primary" onClick={() => setConfirm("activate")}>
                      Reactivate
                    </Button>
                  }
                >
                  <Button variant="danger" onClick={() => setConfirm("suspend")}>
                    Suspend
                  </Button>
                </Show>
              </>
            }
          />
          <div class="grid-2">
            <Card title="Identity">
              <ul class="feed">
                <li>
                  <span class="grow">Email</span>
                  {x.user.email}{" "}
                  <Badge tone={x.user.email_verified ? "success" : "warning"}>
                    {x.user.email_verified ? "verified" : "unverified"}
                  </Badge>
                </li>
                <li>
                  <span class="grow">Status</span>
                  <Badge tone={x.user.status === "active" ? "success" : "danger"}>{x.user.status}</Badge>
                </li>
                <li>
                  <span class="grow">Identity provider</span>
                  <code class="subtle">{x.user.identity_provider}</code>
                </li>
                <li>
                  <span class="grow">Created</span>
                  {fmtDateTime(x.user.created_at)}
                </li>
                <li>
                  <span class="grow">Last sign-in</span>
                  {fmtRelative(x.user.last_login_at)}
                </li>
                <li>
                  <span class="grow">Active sessions</span>
                  {x.active_sessions}
                </li>
              </ul>
            </Card>
            <Card title="System role">
              <div class="stack">
                <Alert tone="info">
                  System roles are a separate trust level from organization roles. Changing one signs the user
                  out everywhere.
                </Alert>
                <label class="sr-only" for="sysrole">
                  System role
                </label>
                <select
                  id="sysrole"
                  class="select"
                  value={x.user.system_role}
                  onChange={(e) => setRole(e.currentTarget.value)}
                >
                  <option value="none">None</option>
                  <option value="system_auditor">System auditor (read-only)</option>
                  <option value="system_admin">System admin</option>
                </select>
              </div>
            </Card>
          </div>
          <Card title="Organizations" flush>
            <DataTable
              caption="Organizations"
              rows={x.organizations}
              rowKey={(o) => o.id}
              columns={[
                { key: "name", header: "Organization", cell: (o) => <strong>{o.name}</strong> },
                { key: "role", header: "Role", cell: (o) => <Badge>{o.role}</Badge> },
                { key: "members", header: "Members", align: "right", cell: (o) => fmtNumber(o.member_count) },
              ]}
            />
          </Card>
          <ConfirmDialog
            open={!!confirm()}
            title={
              confirm() === "suspend"
                ? "Suspend user?"
                : confirm() === "activate"
                  ? "Reactivate user?"
                  : "Revoke all sessions?"
            }
            body={
              confirm() === "suspend" ? (
                <>They will be signed out everywhere and unable to sign in.</>
              ) : confirm() === "sessions" ? (
                <>They will be signed out of every device.</>
              ) : (
                <>They will be able to sign in again.</>
              )
            }
            confirmLabel="Confirm"
            danger={confirm() !== "activate"}
            onConfirm={act}
            onClose={() => setConfirm(null)}
          />
        </>
      )}
    </AsyncView>
  );
}

export function AdminOrgsPage() {
  const [search, setSearch] = createSignal("");
  const [page, setPage] = createSignal(1);
  const [data, { refetch }] = createResource(
    () => ({ s: search(), p: page() }),
    (q) => api.admin.orgs({ search: q.s || undefined, page: q.p, per_page: 25 }),
  );
  return (
    <>
      <PageHeader
        title="Organizations"
        breadcrumbs={crumbs("Organizations")}
        actions={
          <SearchInput
            label="Search organizations"
            value={search()}
            onInput={(v) => {
              setSearch(v);
              setPage(1);
            }}
          />
        }
      />
      <Card flush>
        <AsyncView data={data} retry={refetch}>
          {(d) => (
            <>
              <DataTable
                caption="Organizations"
                rows={d.items}
                rowKey={(o) => o.id}
                pageSize={1000}
                columns={[
                  {
                    key: "name",
                    header: "Organization",
                    cell: (o) => (
                      <span class="stack-sm gap-0">
                        <strong>{o.name}</strong>
                        <span class="subtle">/{o.slug}</span>
                      </span>
                    ),
                  },
                  {
                    key: "type",
                    header: "Type",
                    cell: (o) => <Badge>{o.personal ? "personal" : "team"}</Badge>,
                  },
                  { key: "plan", header: "Plan", cell: (o) => o.billing_plan },
                  {
                    key: "members",
                    header: "Members",
                    align: "right",
                    cell: (o) => fmtNumber(o.member_count),
                  },
                  {
                    key: "state",
                    header: "State",
                    cell: (o) =>
                      o.deleted_at ? (
                        <Badge tone="danger">deleted</Badge>
                      ) : (
                        <Badge tone="success">active</Badge>
                      ),
                  },
                  { key: "created", header: "Created", cell: (o) => fmtRelative(o.created_at) },
                ]}
              />
              <Pagination
                page={d.page}
                pages={Math.max(1, Math.ceil(d.total / d.per_page))}
                total={d.total}
                onPage={setPage}
              />
            </>
          )}
        </AsyncView>
      </Card>
    </>
  );
}

export function AdminRolesPage() {
  const [m, { refetch }] = createResource(api.admin.roles);
  return (
    <>
      <PageHeader
        title="Roles & permissions"
        breadcrumbs={crumbs("Roles")}
        description="The authorization model in effect (read-only)."
      />
      <AsyncView data={m} retry={refetch}>
        {(r) => (
          <div class="stack">
            <Alert tone="info">
              Engine: <strong>{r.engine}</strong> · System roles managed by{" "}
              <strong>{r.system_roles_source.replace("_", " ")}</strong>
            </Alert>
            <Card title="Organization roles" flush>
              <div class="table-wrap">
                <table class="table">
                  <caption class="sr-only">Permission matrix by organization role</caption>
                  <thead>
                    <tr>
                      <th scope="col">Permission</th>
                      <For each={r.organization_roles}>{(role) => <th scope="col">{role.key}</th>}</For>
                    </tr>
                  </thead>
                  <tbody>
                    <For each={r.permissions}>
                      {(p) => (
                        <tr>
                          <th class="th-plain" scope="row">
                            <code>{p.key}</code>
                          </th>
                          <For each={r.organization_roles}>
                            {(role) => (
                              <td>
                                {role.permissions.includes(p.key) ? (
                                  <>
                                    <span aria-hidden="true">✓</span>
                                    <span class="sr-only">granted</span>
                                  </>
                                ) : (
                                  <>
                                    <span class="subtle" aria-hidden="true">
                                      —
                                    </span>
                                    <span class="sr-only">not granted</span>
                                  </>
                                )}
                              </td>
                            )}
                          </For>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
            </Card>
            <div class="grid-2">
              <For each={r.system_roles}>
                {(s) => (
                  <Card title={s.key}>
                    <div class="row wrap gap-6">
                      <For each={s.permissions}>{(p) => <code class="badge">{p}</code>}</For>
                    </div>
                  </Card>
                )}
              </For>
            </div>
          </div>
        )}
      </AsyncView>
    </>
  );
}

export function AdminAuditPage() {
  return (
    <>
      <PageHeader
        title="Audit log"
        breadcrumbs={crumbs("Audit")}
        description="All organizations and system actions."
      />
      <AuditLogView fetch={(q) => api.admin.audit(q)} />
    </>
  );
}

export function AdminJobsPage() {
  const [status, setStatus] = createSignal("");
  const [data, { refetch }] = createResource(status, (s) => api.admin.jobs(s || undefined));
  return (
    <>
      <PageHeader
        title="Background jobs"
        breadcrumbs={crumbs("Jobs")}
        actions={
          <>
            <label class="sr-only" for="job-status">
              Status
            </label>
            <select
              id="job-status"
              class="select w-auto"
              value={status()}
              onChange={(e) => setStatus(e.currentTarget.value)}
            >
              <option value="">All</option>
              <option value="queued">Queued</option>
              <option value="running">Running</option>
              <option value="succeeded">Succeeded</option>
              <option value="dead">Dead (failed permanently)</option>
            </select>
            <Button icon="refresh" onClick={refetch}>
              Refresh
            </Button>
          </>
        }
      />
      <AsyncView data={data} retry={refetch}>
        {(d) => (
          <div class="stack">
            <div class="grid-cards">
              <For each={d.stats}>
                {(q) => (
                  <MetricCard
                    label={`Queue: ${q.queue}`}
                    value={Number(q.queued)}
                    hint={`${q.running} running · ${q.dead} dead · ${q.succeeded_24h} ok/24h`}
                  />
                )}
              </For>
            </div>
            <Card flush>
              <DataTable
                caption="Jobs"
                rows={d.items}
                rowKey={(j) => j.id}
                columns={[
                  {
                    key: "kind",
                    header: "Job",
                    cell: (j) => (
                      <span class="stack-sm gap-0">
                        <code>{j.kind}</code>
                        <span class="subtle">{j.queue}</span>
                      </span>
                    ),
                  },
                  {
                    key: "status",
                    header: "Status",
                    cell: (j) => (
                      <Badge
                        tone={j.status === "succeeded" ? "success" : j.status === "dead" ? "danger" : "info"}
                      >
                        {j.status}
                      </Badge>
                    ),
                  },
                  {
                    key: "attempts",
                    header: "Attempts",
                    align: "right",
                    cell: (j) => `${j.attempts}/${j.max_attempts}`,
                  },
                  {
                    key: "err",
                    header: "Last error",
                    cell: (j) => (
                      <span class="subtle truncate maxw-260 inline-block">{j.last_error ?? "—"}</span>
                    ),
                  },
                  { key: "created", header: "Created", cell: (j) => fmtRelative(j.created_at) },
                  {
                    key: "act",
                    header: "",
                    align: "right",
                    cell: (j) => (
                      <Show when={j.status === "dead"}>
                        <Button
                          size="sm"
                          onClick={async () => {
                            try {
                              await api.admin.retryJob(j.id);
                              refetch();
                            } catch (e) {
                              toast("Retry failed", { body: errMsg(e), tone: "danger" });
                            }
                          }}
                        >
                          Retry
                        </Button>
                      </Show>
                    ),
                  },
                ]}
              />
            </Card>
          </div>
        )}
      </AsyncView>
    </>
  );
}

export function AdminProvidersPage() {
  const [data, { refetch }] = createResource(api.admin.providers);
  return (
    <>
      <PageHeader
        title="External providers"
        breadcrumbs={crumbs("Providers")}
        description="Outbound API engine: limits, adaptive concurrency and health."
      />
      <AsyncView data={data} retry={refetch}>
        {(d) => (
          <div class="grid-2">
            <For each={d.items}>
              {(p) => (
                <Card
                  title={p.name}
                  actions={
                    p.health ? (
                      <Badge tone={p.health.circuit === "closed" ? "success" : "danger"}>
                        circuit {p.health.circuit}
                      </Badge>
                    ) : (
                      <Badge>no live data</Badge>
                    )
                  }
                >
                  <ul class="feed">
                    <li>
                      <span class="grow">Endpoint</span>
                      <code class="subtle">{p.base_url}</code>
                    </li>
                    <li>
                      <span class="grow">Concurrency limit</span>
                      {p.health
                        ? `${p.health.concurrency_limit} (max ${p.max_concurrency})`
                        : p.max_concurrency}
                    </li>
                    <Show when={p.health}>
                      {(h) => (
                        <>
                          <li>
                            <span class="grow">In flight / queued</span>
                            {h().inflight} / {h().queued}
                          </li>
                          <li>
                            <span class="grow">Success rate</span>
                            {(h().success_rate * 100).toFixed(1)}%
                          </li>
                          <li>
                            <span class="grow">Latency p50 / p95 / p99</span>
                            {h().p50_ms.toFixed(0)} / {h().p95_ms.toFixed(0)} / {h().p99_ms.toFixed(0)} ms
                          </li>
                          <li>
                            <span class="grow">429 rate</span>
                            {(h().rate_429 * 100).toFixed(1)}%
                          </li>
                        </>
                      )}
                    </Show>
                    <li>
                      <span class="grow">Rate limit</span>
                      {p.requests_per_second > 0 ? `${p.requests_per_second}/s` : "unlimited"}
                    </li>
                  </ul>
                </Card>
              )}
            </For>
          </div>
        )}
      </AsyncView>
    </>
  );
}

export function AdminSystemPage() {
  const [s, { refetch }] = createResource(api.admin.system);
  return (
    <>
      <PageHeader
        title="System"
        breadcrumbs={crumbs("System")}
        actions={
          <Button icon="refresh" onClick={refetch}>
            Refresh
          </Button>
        }
      />
      <AsyncView data={s} retry={refetch}>
        {(x) => (
          <div class="grid-2">
            <Card title="Health checks">
              <ul class="feed">
                <For each={x.checks}>
                  {(c) => (
                    <li>
                      <span class="grow">
                        {c.name}{" "}
                        <Show when={!c.critical}>
                          <span class="subtle">(non-critical)</span>
                        </Show>
                      </span>
                      <span class="subtle num">{c.latency_ms.toFixed(1)} ms</span>
                      <Badge tone={c.ok ? "success" : c.critical ? "danger" : "warning"}>
                        {c.ok ? "ok" : "failing"}
                      </Badge>
                    </li>
                  )}
                </For>
              </ul>
            </Card>
            <Card title="Build">
              <ul class="feed">
                <li>
                  <span class="grow">Version</span>
                  {x.build.version}
                </li>
                <li>
                  <span class="grow">Commit</span>
                  <code>{x.build.git_sha}</code>
                </li>
                <li>
                  <span class="grow">Profile</span>
                  {x.build.profile}
                </li>
                <li>
                  <span class="grow">Environment</span>
                  <Badge>{x.environment}</Badge>
                </li>
              </ul>
            </Card>
            <Card title="Modules">
              <ul class="feed">
                <li>
                  <span class="grow">Cache</span>
                  {x.modules.cache}
                </li>
                <li>
                  <span class="grow">Messaging (NATS)</span>
                  {x.modules.messaging ? "enabled" : "off"}
                </li>
                <li>
                  <span class="grow">Analytics (ClickHouse)</span>
                  {x.modules.analytics ? "enabled" : "off"}
                </li>
                <li>
                  <span class="grow">Organizations</span>
                  {x.modules.organizations ? "enabled" : "personal only"}
                </li>
                <li>
                  <span class="grow">Authorization engine</span>
                  {x.modules.authorization_engine}
                </li>
              </ul>
            </Card>
            <Card title="Identity">
              <ul class="feed">
                <li>
                  <span class="grow">Provider</span>
                  {x.identity.provider}
                </li>
                <li>
                  <span class="grow">Issuer</span>
                  <code class="subtle">{x.identity.issuer}</code>
                </li>
                <li>
                  <span class="grow">Auth profile</span>
                  {x.identity.profile}
                </li>
                <li>
                  <span class="grow">MFA required for admins</span>
                  {x.identity.require_mfa_for_system_admin ? "yes" : "no"}
                </li>
                <Show when={x.pool}>
                  {(p) => (
                    <li>
                      <span class="grow">DB pool</span>
                      {p().size - p().idle} in use / {p().max}
                    </li>
                  )}
                </Show>
              </ul>
            </Card>
          </div>
        )}
      </AsyncView>
    </>
  );
}
