import { A } from "@solidjs/router";
import { createResource, For, Show } from "solid-js";
import { api } from "../api/endpoints";
import type { DashboardWidgets } from "../api/generated/DashboardWidgets";
import { useSession } from "../auth/session";
import { ActivityFeed, AsyncView, MetricCard } from "../components/data";
import { Alert, Badge, Card, EmptyState, LinkButton, PageHeader, Skeleton } from "../components/ui";
import { fmtRelative } from "../lib/format";

/** Personal dashboard. Each widget is independent so projects can add/remove them freely. */
export function DashboardPage() {
  const { session } = useSession();
  const [data, { refetch }] = createResource(api.dashboard);
  return (
    <>
      <PageHeader
        title={`Welcome back${session()?.user?.display_name ? `, ${session()?.user?.display_name.split(" ")[0]}` : ""}`}
        description="Your account at a glance."
      />
      <AsyncView data={data} retry={refetch} loading={<Skeleton lines={8} height="18px" />}>
        {(d) => <Widgets w={d.widgets} />}
      </AsyncView>
    </>
  );
}

function Widgets(props: { w: DashboardWidgets }) {
  const w = () => props.w;
  return (
    <div class="stack">
      <Show when={!w().account.email_verified}>
        <Alert tone="warning" title="Verify your email">
          Some features need a verified email. <A href="/verify-email">Verify now</A>.
        </Alert>
      </Show>
      <div class="grid-cards">
        <MetricCard label="Organizations" value={w().account.organizations} />
        <MetricCard label="Unread notifications" value={w().notifications.unread} />
        <MetricCard label="Active sessions" value={w().security.active_sessions} />
        <MetricCard
          label="This session"
          value={w().security.mfa_this_session ? "MFA" : "Single factor"}
          hint={w().security.mfa_this_session ? "Strong" : "Add a passkey"}
        />
      </div>
      <div class="grid-2">
        <Card
          title="Organizations"
          actions={
            <LinkButton href="/account/profile" size="sm" variant="ghost">
              Manage
            </LinkButton>
          }
          flush
        >
          <Show
            when={w().organizations.length}
            fallback={<EmptyState icon="building" title="No organizations" />}
          >
            <ul class="feed px-5">
              <For each={w().organizations}>
                {(o) => (
                  <li>
                    <div class="grow">
                      <A href={`/org/${o.slug}`}>
                        <strong>{o.name}</strong>
                      </A>
                      <div class="subtle">
                        {o.member_count} member{o.member_count === 1 ? "" : "s"}
                      </div>
                    </div>
                    <Badge tone={o.role === "owner" ? "brand" : "neutral"}>{o.role}</Badge>
                    <Show when={o.personal}>
                      <Badge>personal</Badge>
                    </Show>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </Card>
        <Card
          title="Security"
          actions={
            <LinkButton href="/account/security" size="sm" variant="ghost">
              Review
            </LinkButton>
          }
        >
          <ul class="feed">
            <li>
              <span class="grow">Email verification</span>
              <Badge tone={w().security.email_verified ? "success" : "warning"}>
                {w().security.email_verified ? "Verified" : "Pending"}
              </Badge>
            </li>
            <li>
              <span class="grow">Multi-factor in this session</span>
              <Badge tone={w().security.mfa_this_session ? "success" : "warning"}>
                {w().security.mfa_this_session ? "Yes" : "No"}
              </Badge>
            </li>
            <li>
              <span class="grow">Signed-in devices</span>
              <A href="/account/sessions">{w().security.active_sessions}</A>
            </li>
          </ul>
        </Card>
      </div>
      <div class="grid-2">
        <Card
          title="Recent activity"
          actions={
            <LinkButton href="/account/activity" size="sm" variant="ghost">
              All activity
            </LinkButton>
          }
        >
          <ActivityFeed items={w().activity} />
        </Card>
        <Card
          title="Notifications"
          actions={
            <LinkButton href="/account/notifications" size="sm" variant="ghost">
              View all
            </LinkButton>
          }
        >
          <Show
            when={w().notifications.recent.length}
            fallback={<EmptyState icon="bell" title="You're all caught up" />}
          >
            <ul class="feed">
              <For each={w().notifications.recent}>
                {(n) => (
                  <li>
                    <div class="grow">
                      <strong>{n.title}</strong>
                      <div class="subtle">{n.body}</div>
                    </div>
                    <time class="subtle" datetime={n.created_at}>
                      {fmtRelative(n.created_at)}
                    </time>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </Card>
      </div>
    </div>
  );
}
