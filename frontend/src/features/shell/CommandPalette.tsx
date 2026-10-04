import { useNavigate } from "@solidjs/router";
import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import { useSession } from "../../auth/session";
import { Icon, type IconName } from "../../components/icons";

interface Command {
  id: string;
  label: string;
  group: string;
  icon: IconName;
  href: string;
}

/** ⌘K / Ctrl+K command palette: combobox + listbox semantics, fully keyboard driven. */
export function CommandPalette(props: { open: boolean; onClose: () => void; orgSlug: string | null }) {
  const navigate = useNavigate();
  const { session, isSystem } = useSession();
  const [q, setQ] = createSignal("");
  const [active, setActive] = createSignal(0);
  let input: HTMLInputElement | undefined;
  let dialog: HTMLDialogElement | undefined;

  const commands = createMemo<Command[]>(() => {
    const list: Command[] = [
      { id: "dash", label: "Dashboard", group: "Navigate", icon: "home", href: "/dashboard" },
      { id: "profile", label: "Profile", group: "Account", icon: "user", href: "/account/profile" },
      { id: "security", label: "Security", group: "Account", icon: "shield", href: "/account/security" },
      { id: "passkeys", label: "Passkeys", group: "Account", icon: "fingerprint", href: "/account/passkeys" },
      {
        id: "sessions",
        label: "Sessions & devices",
        group: "Account",
        icon: "monitor",
        href: "/account/sessions",
      },
      { id: "prefs", label: "Preferences", group: "Account", icon: "settings", href: "/account/preferences" },
    ];
    const s = props.orgSlug;
    if (s) {
      for (const [id, label, icon, path] of [
        ["o-overview", "Organization overview", "building", ""],
        ["o-runs", "Runs", "play", "/runs"],
        ["o-members", "Members", "users", "/members"],
        ["o-teams", "Teams", "users", "/teams"],
        ["o-roles", "Roles", "shield", "/roles"],
        ["o-keys", "API keys", "key", "/api-keys"],
        ["o-audit", "Audit log", "list", "/audit"],
        ["o-settings", "Organization settings", "settings", "/settings"],
      ] as const)
        list.push({ id, label, group: "Organization", icon, href: `/org/${s}${path}` });
    }
    for (const o of session()?.organizations ?? [])
      list.push({
        id: `switch-${o.slug}`,
        label: `Switch to ${o.name}`,
        group: "Organizations",
        icon: "briefcase",
        href: `/org/${o.slug}`,
      });
    if (isSystem())
      for (const [id, label, path] of [
        ["a-overview", "Admin overview", ""],
        ["a-users", "Admin: users", "/users"],
        ["a-orgs", "Admin: organizations", "/organizations"],
        ["a-audit", "Admin: audit", "/audit"],
        ["a-jobs", "Admin: jobs", "/jobs"],
        ["a-system", "Admin: system", "/system"],
      ] as const)
        list.push({ id, label, group: "Administration", icon: "server", href: `/admin${path}` });
    return list;
  });

  const results = createMemo(() => {
    const t = q().trim().toLowerCase();
    return t ? commands().filter((c) => c.label.toLowerCase().includes(t)) : commands();
  });

  const run = (c: Command | undefined) => {
    if (!c) return;
    props.onClose();
    navigate(c.href);
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(results().length - 1, a + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(0, a - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      run(results()[active()]);
    }
  };

  // Effects run after refs are bound, so the native dialog can be driven reactively.
  createEffect(() => {
    // Read the reactive input *before* any early return so the effect always tracks it.
    const open = props.open;
    if (!dialog) return;
    if (open && !dialog.open) {
      setQ("");
      setActive(0);
      dialog.showModal?.();
      queueMicrotask(() => input?.focus());
    } else if (!open && dialog.open) {
      dialog.close();
    }
  });

  return (
    <dialog
      ref={dialog}
      class="dialog palette-dialog"
      aria-label="Command palette"
      onClose={() => props.onClose()}
    >
      <div class="dialog-body stack-sm">
        <div class="row">
          <Icon name="search" />
          <input
            ref={input}
            class="input input-bare"
            role="combobox"
            aria-expanded="true"
            aria-controls="palette-list"
            aria-activedescendant={results()[active()] ? `cmd-${results()[active()]?.id}` : undefined}
            aria-label="Search commands"
            placeholder="Search pages, organizations, actions…"
            value={q()}
            onInput={(e) => {
              setQ(e.currentTarget.value);
              setActive(0);
            }}
            onKeyDown={onKey}
          />
        </div>
        <hr class="divider" />
        <div class="scroll-360" id="palette-list" role="listbox" aria-label="Commands">
          <Show when={results().length > 0} fallback={<p class="subtle p-3">No matches</p>}>
            <For each={results()}>
              {(c, i) => (
                // Options are not tab stops: focus stays in the combobox input and
                // aria-activedescendant points at the highlighted option (WAI-ARIA combobox pattern).
                <div
                  id={`cmd-${c.id}`}
                  role="option"
                  tabIndex={-1}
                  aria-selected={i() === active()}
                  class="menu-item"
                  data-active={i() === active()}
                  onMouseEnter={() => setActive(i())}
                  onClick={() => run(c)}
                  onKeyDown={(e) => e.key === "Enter" && run(c)}
                >
                  <Icon name={c.icon} size={16} />
                  <span class="grow">{c.label}</span>
                  <span class="subtle">{c.group}</span>
                </div>
              )}
            </For>
          </Show>
        </div>
      </div>
    </dialog>
  );
}
