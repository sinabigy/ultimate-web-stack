// Design-system primitives. Accessible by construction: labelled controls, described errors,
// native <dialog> modals (focus trap + Escape), keyboard-operable menus.
import { A } from "@solidjs/router";
import {
  createEffect,
  createSignal,
  createUniqueId,
  For,
  type JSX,
  onCleanup,
  type ParentProps,
  Show,
  splitProps,
} from "solid-js";
import { ApiError } from "../api/client";
import { reauthenticate } from "../auth/session";
import { initials } from "../lib/format";
import { Icon, type IconName } from "./icons";

type Variant = "primary" | "secondary" | "ghost" | "danger";

export function Button(
  props: JSX.ButtonHTMLAttributes<HTMLButtonElement> & {
    variant?: Variant;
    size?: "sm" | "md" | "lg";
    loading?: boolean;
    icon?: IconName;
    block?: boolean;
  },
) {
  const [own, rest] = splitProps(props, [
    "variant",
    "size",
    "loading",
    "icon",
    "block",
    "class",
    "children",
    "disabled",
  ]);
  const cls = () =>
    [
      "btn",
      own.variant && own.variant !== "secondary" ? `btn-${own.variant}` : "",
      own.size && own.size !== "md" ? `btn-${own.size}` : "",
      own.block ? "btn-block" : "",
      own.class ?? "",
    ]
      .filter(Boolean)
      .join(" ");
  return (
    <button
      type="button"
      class={cls()}
      disabled={own.disabled || own.loading}
      aria-busy={own.loading ? "true" : undefined}
      {...rest}
    >
      <Show when={own.loading} fallback={own.icon ? <Icon name={own.icon} size={16} /> : null}>
        <span class="spinner" aria-hidden="true" />
      </Show>
      {own.children}
    </button>
  );
}

export function LinkButton(props: {
  href: string;
  variant?: Variant;
  size?: "sm" | "md";
  icon?: IconName;
  children: JSX.Element;
  external?: boolean;
}) {
  const cls = () =>
    [
      "btn",
      props.variant && props.variant !== "secondary" ? `btn-${props.variant}` : "",
      props.size === "sm" ? "btn-sm" : "",
    ].join(" ");
  return props.external ? (
    <a class={cls()} href={props.href}>
      {props.icon && <Icon name={props.icon} size={16} />}
      {props.children}
    </a>
  ) : (
    <A class={cls()} href={props.href}>
      {props.icon && <Icon name={props.icon} size={16} />}
      {props.children}
    </A>
  );
}

/** Label + control + hint + error, wired with aria-describedby / aria-invalid. */
export function Field(props: {
  label: string;
  hint?: string;
  error?: string | null;
  children: (a: { id: string; describedBy: string | undefined; invalid: boolean }) => JSX.Element;
}) {
  const id = createUniqueId();
  const hintId = `${id}-hint`;
  const errId = `${id}-err`;
  const describedBy = () =>
    [props.hint ? hintId : "", props.error ? errId : ""].filter(Boolean).join(" ") || undefined;
  return (
    <div class="field">
      <label class="label" for={id}>
        {props.label}
      </label>
      {props.children({ id, describedBy: describedBy(), invalid: !!props.error })}
      <Show when={props.hint}>
        <span class="hint" id={hintId}>
          {props.hint}
        </span>
      </Show>
      <Show when={props.error}>
        <span class="field-error" id={errId} role="alert">
          {props.error}
        </span>
      </Show>
    </div>
  );
}

export type Tone = "neutral" | "success" | "warning" | "danger" | "info" | "brand";
export function Badge(props: ParentProps<{ tone?: Tone }>) {
  return (
    <span class={`badge ${props.tone && props.tone !== "neutral" ? `badge-${props.tone}` : ""}`}>
      {props.children}
    </span>
  );
}

export function Card(
  props: ParentProps<{
    title?: JSX.Element;
    actions?: JSX.Element;
    footer?: JSX.Element;
    flush?: boolean;
    class?: string;
  }>,
) {
  return (
    <section class={`card ${props.class ?? ""}`}>
      <Show when={props.title || props.actions}>
        <header class="card-header">
          <Show when={props.title}>
            <h2>{props.title}</h2>
          </Show>
          <div class="row">{props.actions}</div>
        </header>
      </Show>
      <Show when={!props.flush} fallback={props.children}>
        <div class="card-body">{props.children}</div>
      </Show>
      <Show when={props.footer}>
        <footer class="card-footer">{props.footer}</footer>
      </Show>
    </section>
  );
}

export function Avatar(props: { name: string; large?: boolean }) {
  return (
    <span class={`avatar ${props.large ? "avatar-lg" : ""}`} aria-hidden="true">
      {initials(props.name)}
    </span>
  );
}

export function Alert(
  props: ParentProps<{ tone?: "info" | "warning" | "danger" | "success"; title?: string }>,
) {
  return (
    <div class={`alert alert-${props.tone ?? "info"}`} role={props.tone === "danger" ? "alert" : "status"}>
      <Icon name={props.tone === "danger" || props.tone === "warning" ? "alert" : "check"} />
      <div class="stack-sm">
        <Show when={props.title}>
          <strong>{props.title}</strong>
        </Show>
        <div>{props.children}</div>
      </div>
    </div>
  );
}

export function EmptyState(props: { icon?: IconName; title: string; body?: string; action?: JSX.Element }) {
  return (
    <div class="empty">
      <div class="empty-icon">
        <Icon name={props.icon ?? "list"} size={22} />
      </div>
      <strong>{props.title}</strong>
      <Show when={props.body}>
        <p class="subtle">{props.body}</p>
      </Show>
      <Show when={props.action}>
        <div class="mt-2">{props.action}</div>
      </Show>
    </div>
  );
}

/** Error state that understands API error codes (permission, step-up, outage). */
export function ErrorState(props: { error: unknown; retry?: () => void }) {
  const e = () => (props.error instanceof ApiError ? props.error : null);
  return (
    <div class="empty" role="alert">
      <div class="empty-icon">
        <Icon name={e()?.status === 403 ? "lock" : "alert"} size={22} />
      </div>
      <strong>
        {e()?.status === 403 ? "Access denied" : e()?.status === 404 ? "Not found" : "Something went wrong"}
      </strong>
      <p class="subtle">{e()?.friendly ?? "Please try again."}</p>
      <div class="row">
        <Show when={e()?.code === "mfa_required" || e()?.code === "reauth_required"}>
          <Button variant="primary" onClick={reauthenticate}>
            Verify it's you
          </Button>
        </Show>
        <Show when={props.retry && (!e() || (e()?.status ?? 0) >= 500)}>
          <Button icon="refresh" onClick={() => props.retry?.()}>
            Retry
          </Button>
        </Show>
      </div>
      <Show when={e()?.requestId}>
        <p class="subtle mono">Reference: {e()?.requestId}</p>
      </Show>
    </div>
  );
}

export function Skeleton(props: { width?: string; height?: string; lines?: number }) {
  return (
    <div class="stack-sm" aria-hidden="true">
      <For each={Array.from({ length: props.lines ?? 1 })}>
        {() => (
          <div class="skeleton" style={{ width: props.width ?? "100%", height: props.height ?? "14px" }} />
        )}
      </For>
    </div>
  );
}

export function PageHeader(props: {
  title: string;
  description?: string;
  actions?: JSX.Element;
  breadcrumbs?: { label: string; href?: string }[];
}) {
  createEffect(() => {
    document.title = `${props.title} · App`;
  });
  return (
    <div class="page-header">
      <div class="stack-sm">
        <Show when={props.breadcrumbs?.length}>
          <nav aria-label="Breadcrumb" class="breadcrumbs">
            <ol>
              <For each={props.breadcrumbs}>
                {(b, i) => (
                  <li>
                    <Show
                      when={b.href && i() < (props.breadcrumbs?.length ?? 0) - 1}
                      fallback={<span aria-current="page">{b.label}</span>}
                    >
                      <A href={b.href ?? "#"}>{b.label}</A>
                    </Show>
                  </li>
                )}
              </For>
            </ol>
          </nav>
        </Show>
        <h1>{props.title}</h1>
        <Show when={props.description}>
          <p class="muted">{props.description}</p>
        </Show>
      </div>
      <div class="row wrap">{props.actions}</div>
    </div>
  );
}

export function Tabs(props: {
  label: string;
  items: { href: string; label: string; end?: boolean; hidden?: boolean }[];
}) {
  return (
    <nav class="tabs" aria-label={props.label}>
      <For each={props.items.filter((i) => !i.hidden)}>
        {(i) => (
          <A class="tab" href={i.href} end={i.end} activeClass="active">
            {i.label}
          </A>
        )}
      </For>
    </nav>
  );
}

/** Native modal dialog: the browser provides focus containment, Escape and inertness. */
export function Dialog(
  props: ParentProps<{
    open: boolean;
    title: string;
    onClose: () => void;
    footer?: JSX.Element;
    describedBy?: string;
  }>,
) {
  let el: HTMLDialogElement | undefined;
  const titleId = createUniqueId();
  createEffect(() => {
    const open = props.open; // track before any early return
    if (!el) return;
    if (open && !el.open) el.showModal?.() ?? el.setAttribute("open", "");
    if (!open && el.open) el.close?.() ?? el.removeAttribute("open");
  });
  return (
    <dialog
      ref={el}
      class="dialog"
      aria-labelledby={titleId}
      onClose={() => props.onClose()}
      onCancel={() => props.onClose()}
    >
      <Show when={props.open}>
        <div class="dialog-body stack">
          <div class="row-between">
            <h2 id={titleId}>{props.title}</h2>
            <Button
              variant="ghost"
              class="btn-icon"
              aria-label="Close dialog"
              onClick={() => props.onClose()}
            >
              <Icon name="x" />
            </Button>
          </div>
          {props.children}
        </div>
        <Show when={props.footer}>
          <div class="dialog-footer">{props.footer}</div>
        </Show>
      </Show>
    </dialog>
  );
}

/** Confirmation for destructive actions; optionally requires typing a phrase. */
export function ConfirmDialog(props: {
  open: boolean;
  title: string;
  body: JSX.Element;
  confirmLabel: string;
  danger?: boolean;
  requireText?: string;
  busy?: boolean;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const [typed, setTyped] = createSignal("");
  const ok = () => !props.requireText || typed().trim().toLowerCase() === props.requireText.toLowerCase();
  return (
    <Dialog
      open={props.open}
      title={props.title}
      onClose={() => {
        setTyped("");
        props.onClose();
      }}
      footer={
        <>
          <Button onClick={() => props.onClose()}>Cancel</Button>
          <Button
            variant={props.danger ? "danger" : "primary"}
            disabled={!ok()}
            loading={props.busy}
            onClick={() => props.onConfirm()}
          >
            {props.confirmLabel}
          </Button>
        </>
      }
    >
      <div class="muted">{props.body}</div>
      <Show when={props.requireText}>
        <Field label={`Type ${props.requireText} to confirm`}>
          {(a) => (
            <input
              id={a.id}
              class="input"
              autocomplete="off"
              value={typed()}
              onInput={(e) => setTyped(e.currentTarget.value)}
            />
          )}
        </Field>
      </Show>
    </Dialog>
  );
}

/** Dropdown menu: button + popup list; arrow keys move, Escape closes and restores focus. */
export function Menu(props: {
  label: string;
  trigger: JSX.Element;
  children: JSX.Element;
  align?: "left" | "right";
  triggerClass?: string;
}) {
  const [open, setOpen] = createSignal(false);
  const id = createUniqueId();
  let wrap: HTMLDivElement | undefined;
  let btn: HTMLButtonElement | undefined;
  const items = () => Array.from(wrap?.querySelectorAll<HTMLElement>("[role=menuitem]") ?? []);
  const close = (focus = true) => {
    setOpen(false);
    if (focus) btn?.focus();
  };
  const onDoc = (e: MouseEvent) => {
    if (open() && wrap && !wrap.contains(e.target as Node)) close(false);
  };
  document.addEventListener("mousedown", onDoc);
  onCleanup(() => document.removeEventListener("mousedown", onDoc));
  const onKey = (e: KeyboardEvent) => {
    const list = items();
    const idx = list.indexOf(document.activeElement as HTMLElement);
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      list[(idx + 1) % list.length]?.focus();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      list[(idx - 1 + list.length) % list.length]?.focus();
    } else if (e.key === "Tab") {
      close(false);
    }
  };
  return (
    <div class="relative" ref={wrap}>
      <button
        ref={btn}
        type="button"
        class={props.triggerClass ?? "btn btn-ghost"}
        aria-haspopup="menu"
        aria-expanded={open()}
        aria-controls={id}
        aria-label={props.label}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" && !open()) {
            e.preventDefault();
            setOpen(true);
            queueMicrotask(() => items()[0]?.focus());
          }
        }}
        onClick={() => {
          setOpen(!open());
          if (open()) queueMicrotask(() => items()[0]?.focus());
        }}
      >
        {props.trigger}
      </button>
      <Show when={open()}>
        <div
          id={id}
          role="menu"
          class={`menu ${props.align === "left" ? "menu-left" : "menu-right"}`}
          onKeyDown={onKey}
          onClick={(e) => {
            if ((e.target as HTMLElement).closest("[role=menuitem]")) close(false);
          }}
        >
          {props.children}
        </div>
      </Show>
    </div>
  );
}

export function MenuItem(props: {
  onSelect?: () => void;
  href?: string;
  icon?: IconName;
  children: JSX.Element;
  danger?: boolean;
}) {
  const content = (
    <>
      {props.icon && <Icon name={props.icon} size={16} />}
      <span style={props.danger ? { color: "var(--danger)" } : undefined}>{props.children}</span>
    </>
  );
  return props.href ? (
    <A role="menuitem" tabIndex={-1} class="menu-item" href={props.href}>
      {content}
    </A>
  ) : (
    <button type="button" role="menuitem" tabIndex={-1} class="menu-item" onClick={() => props.onSelect?.()}>
      {content}
    </button>
  );
}

export function Kbd(props: ParentProps) {
  return <kbd class="kbd">{props.children}</kbd>;
}
