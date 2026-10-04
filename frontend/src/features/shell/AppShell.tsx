import { A, useLocation, useNavigate } from "@solidjs/router";
import {
  createEffect,
  createResource,
  createSignal,
  For,
  type JSX,
  on,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import { api } from "../../api/endpoints";
import { useSession } from "../../auth/session";
import { RealtimeIndicator } from "../../components/data";
import { Icon, type IconName } from "../../components/icons";
import { Avatar, Badge, Kbd, Menu, MenuItem } from "../../components/ui";
import { fmtRelative } from "../../lib/format";
import { subscribe } from "../../realtime/events";
import { setTheme, theme } from "../../stores/theme";
import { toast } from "../../stores/toast";
import { CommandPalette } from "./CommandPalette";
import { rememberOrg, useCurrentOrg } from "./orgContext";

function NavLink(props: { href: string; icon: IconName; label: string; end?: boolean }) {
  return (
    <A href={props.href} class="nav-link" activeClass="active" end={props.end}>
      <Icon name={props.icon} />
      <span>{props.label}</span>
    </A>
  );
}

function Sidebar(props: { open: boolean; onNavigate: () => void }) {
  const { session, isSystem } = useSession();
  const org = useCurrentOrg();
  const navigate = useNavigate();
  const location = useLocation();
  // Close the mobile drawer whenever navigation happens (links, palette, back button).
  createEffect(
    on(
      () => location.pathname,
      () => props.onNavigate(),
      { defer: true },
    ),
  );
  const base = () => `/org/${org.slug() ?? ""}`;
  return (
    <aside class="sidebar" data-open={props.open} aria-label="Primary">
      <div class="sidebar-brand">
        <span class="brand-mark" aria-hidden="true">
          <Icon name="activity" size={16} />
        </span>
        App
      </div>
      <Show when={session()?.features.organizations && (session()?.organizations.length ?? 0) > 0}>
        <div class="org-switch">
          <label class="sr-only" for="org-switcher">
            Organization
          </label>
          <select
            id="org-switcher"
            class="select"
            value={org.slug() ?? ""}
            onChange={(e) => {
              rememberOrg(e.currentTarget.value);
              navigate(`/org/${e.currentTarget.value}`);
            }}
          >
            <For each={session()?.organizations}>
              {(o) => <option value={o.slug}>{o.personal ? `${o.name} (personal)` : o.name}</option>}
            </For>
          </select>
        </div>
      </Show>
      <nav class="nav" aria-label="Main">
        <NavLink href="/dashboard" icon="home" label="Dashboard" />
        <Show when={org.slug()}>
          <div class="nav-section">Organization</div>
          <NavLink href={base()} icon="building" label="Overview" end />
          <Show when={org.can("runs:read")}>
            <NavLink href={`${base()}/runs`} icon="play" label="Runs" />
          </Show>
          <Show when={org.can("members:read")}>
            <NavLink href={`${base()}/members`} icon="users" label="Members" />
          </Show>
          <Show when={org.can("teams:read")}>
            <NavLink href={`${base()}/teams`} icon="users" label="Teams" />
          </Show>
          <Show when={org.can("roles:read")}>
            <NavLink href={`${base()}/roles`} icon="shield" label="Roles" />
          </Show>
          <Show when={org.can("api_keys:read")}>
            <NavLink href={`${base()}/api-keys`} icon="key" label="API keys" />
          </Show>
          <Show when={org.can("audit:read")}>
            <NavLink href={`${base()}/audit`} icon="list" label="Audit log" />
          </Show>
          <Show when={org.can("org:update") || org.can("settings:manage")}>
            <NavLink href={`${base()}/settings`} icon="settings" label="Settings" />
          </Show>
          <Show when={org.can("billing:read")}>
            <NavLink href={`${base()}/billing`} icon="briefcase" label="Billing" />
          </Show>
        </Show>
        <div class="nav-section">Account</div>
        <NavLink href="/account/profile" icon="user" label="Profile" />
        <NavLink href="/account/security" icon="shield" label="Security" />
        <NavLink href="/account/sessions" icon="monitor" label="Sessions" />
        <NavLink href="/account/notifications" icon="bell" label="Notifications" />
        <Show when={isSystem()}>
          <div class="nav-section">Administration</div>
          <NavLink href="/admin" icon="server" label="Overview" end />
          <NavLink href="/admin/users" icon="users" label="Users" />
          <NavLink href="/admin/organizations" icon="building" label="Organizations" />
          <NavLink href="/admin/roles" icon="shield" label="Roles" />
          <NavLink href="/admin/audit" icon="list" label="Audit" />
          <NavLink href="/admin/jobs" icon="clock" label="Jobs" />
          <NavLink href="/admin/providers" icon="activity" label="Providers" />
          <NavLink href="/admin/system" icon="settings" label="System" />
        </Show>
      </nav>
    </aside>
  );
}

function Notifications() {
  const { session, refetch } = useSession();
  const [list, { refetch: reload }] = createResource(() => api.notifications.list());
  const unread = () => list()?.unread ?? session()?.unread_notifications ?? 0;
  const off = subscribe((e) => {
    if (e.type === "notification") {
      toast(e.title, { tone: "info" });
      reload();
    }
  });
  onCleanup(off);
  return (
    <Menu
      label={`Notifications${unread() ? `, ${unread()} unread` : ""}`}
      triggerClass="btn btn-ghost btn-icon"
      trigger={
        <span class="relative inline-flex">
          <Icon name="bell" />
          <Show when={unread() > 0}>
            <span class="badge badge-danger badge-count">{unread()}</span>
          </Show>
        </span>
      }
    >
      <div class="row-between menu-pad">
        <strong>Notifications</strong>
        <button
          type="button"
          class="btn btn-ghost btn-sm"
          onClick={async () => {
            await api.notifications.readAll();
            reload();
            refetch();
          }}
        >
          Mark all read
        </button>
      </div>
      <Show
        when={(list()?.items.length ?? 0) > 0}
        fallback={<p class="subtle menu-pad">You're all caught up.</p>}
      >
        <For each={list()?.items.slice(0, 6)}>
          {(n) => (
            <MenuItem href={n.link ?? "/account/notifications"} icon={n.read_at ? "check" : "bell"}>
              <span class="stack-sm gap-0">
                <span>{n.title}</span>
                <span class="subtle">{fmtRelative(n.created_at)}</span>
              </span>
            </MenuItem>
          )}
        </For>
      </Show>
      <MenuItem href="/account/notifications">View all</MenuItem>
    </Menu>
  );
}

function UserMenu() {
  const { session } = useSession();
  const signOut = async () => {
    try {
      const r = await api.logout();
      window.location.assign(r.redirect);
    } catch {
      window.location.assign("/login");
    }
  };
  return (
    <Menu
      label="Account menu"
      triggerClass="btn btn-ghost"
      trigger={
        <span class="row gap-2">
          <Avatar name={session()?.user?.display_name ?? "?"} />
          <span class="truncate maxw-140">{session()?.user?.display_name}</span>
          <Icon name="chevronDown" size={14} />
        </span>
      }
    >
      <div class="stack-sm menu-pad">
        <strong class="truncate">{session()?.user?.display_name}</strong>
        <span class="subtle truncate">{session()?.user?.email}</span>
        <Show when={session()?.user?.system_role !== "none"}>
          <Badge tone="brand">{session()?.user?.system_role?.replace("_", " ")}</Badge>
        </Show>
      </div>
      <hr class="divider" />
      <MenuItem href="/account/profile" icon="user">
        Profile
      </MenuItem>
      <MenuItem href="/account/security" icon="shield">
        Security
      </MenuItem>
      <MenuItem icon="settings" onSelect={() => setTheme(theme() === "dark" ? "light" : "dark")}>
        {theme() === "dark" ? "Light theme" : "Dark theme"}
      </MenuItem>
      <hr class="divider" />
      <MenuItem icon="logout" onSelect={signOut}>
        Sign out
      </MenuItem>
    </Menu>
  );
}

export function AppShell(props: { children?: JSX.Element }) {
  const [navOpen, setNavOpen] = createSignal(false);
  const [palette, setPalette] = createSignal(false);
  const org = useCurrentOrg();
  const location = useLocation();
  createEffect(() => {
    const s = org.slug();
    if (s && location.pathname.startsWith("/org/")) rememberOrg(s);
  });
  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette(true);
      }
    };
    document.addEventListener("keydown", onKey);
    const off = subscribe(() => {}); // keep the realtime connection open while signed in
    onCleanup(() => {
      document.removeEventListener("keydown", onKey);
      off();
    });
  });
  return (
    <div class="shell">
      {/* biome-ignore lint/a11y/useValidAnchor: a skip link is in-page navigation (WCAG G1); the handler only restores the focus move the SPA router suppresses */}
      <a
        class="skip-link"
        href="#main"
        // The router intercepts in-app anchors (no native focus move), so focus explicitly;
        // this handler runs before the router's document-level listener.
        onClick={() => document.getElementById("main")?.focus()}
      >
        Skip to content
      </a>
      <Sidebar open={navOpen()} onNavigate={() => setNavOpen(false)} />
      <Show when={navOpen()}>
        <div class="scrim mobile-only" aria-hidden="true" onClick={() => setNavOpen(false)} />
      </Show>
      <div class="grow min-w-0">
        <header class="topbar">
          <button
            type="button"
            class="btn btn-ghost btn-icon mobile-only"
            aria-label="Open navigation"
            aria-expanded={navOpen()}
            onClick={() => setNavOpen(true)}
          >
            <Icon name="menu" />
          </button>
          <button
            type="button"
            class="search-trigger"
            onClick={() => setPalette(true)}
            aria-label="Search (Ctrl+K)"
          >
            <Icon name="search" size={16} />
            <span class="search-label grow text-left">Search…</span>
            <span class="search-label">
              <Kbd>⌘</Kbd> <Kbd>K</Kbd>
            </span>
          </button>
          <div class="grow" />
          <RealtimeIndicator />
          <Notifications />
          <UserMenu />
        </header>
        <main id="main" class="main" tabIndex={-1}>
          {props.children}
        </main>
      </div>
      <CommandPalette open={palette()} onClose={() => setPalette(false)} orgSlug={org.slug()} />
    </div>
  );
}
