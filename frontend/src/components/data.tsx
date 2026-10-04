// Dashboard primitives. Generic and data-agnostic: pages compose them with typed API data.
import { createMemo, createSignal, For, type JSX, onCleanup, type Resource, Show } from "solid-js";
import type { AuditRow } from "../api/generated/AuditRow";
import { describeAction, fmtCompact, fmtNumber, fmtRelative } from "../lib/format";
import { connectionState } from "../realtime/events";
import { Icon } from "./icons";
import { Badge, Button, EmptyState, ErrorState, Skeleton } from "./ui";

// ---------------------------------------------------------------- async wrapper

/** Renders loading (skeleton), error (with retry) and empty states around a resource. */
export function AsyncView<T>(props: {
  data: Resource<T>;
  retry?: () => void;
  loading?: JSX.Element;
  empty?: (v: T) => boolean;
  emptyView?: JSX.Element;
  children: (v: T) => JSX.Element;
}) {
  return (
    <Show when={!props.data.error} fallback={<ErrorState error={props.data.error} retry={props.retry} />}>
      <Show
        when={props.data.state !== "pending" || props.data.latest}
        fallback={props.loading ?? <Skeleton lines={4} />}
      >
        <Show
          when={!(props.empty && props.data.latest !== undefined && props.empty(props.data.latest as T))}
          fallback={props.emptyView ?? <EmptyState title="Nothing here yet" />}
        >
          {props.children(props.data.latest as T)}
        </Show>
      </Show>
    </Show>
  );
}

// ---------------------------------------------------------------- table

export interface Column<T> {
  key: string;
  header: string;
  cell: (row: T) => JSX.Element;
  /** Value used for client-side sorting/filtering. */
  sortValue?: (row: T) => string | number;
  align?: "left" | "right";
  width?: string;
}

export function DataTable<T>(props: {
  caption: string;
  columns: Column<T>[];
  rows: T[];
  rowKey: (row: T) => string;
  filterText?: string;
  pageSize?: number;
  empty?: JSX.Element;
  onRowClick?: (row: T) => void;
}) {
  const [sortKey, setSortKey] = createSignal<string | null>(null);
  const [dir, setDir] = createSignal<1 | -1>(1);
  const [page, setPage] = createSignal(0);
  const filtered = createMemo(() => {
    const q = (props.filterText ?? "").trim().toLowerCase();
    if (!q) return props.rows;
    return props.rows.filter((r) =>
      props.columns.some((c) => c.sortValue && String(c.sortValue(r)).toLowerCase().includes(q)),
    );
  });
  const sorted = createMemo(() => {
    const k = sortKey();
    const col = props.columns.find((c) => c.key === k);
    if (!col?.sortValue) return filtered();
    const sv = col.sortValue;
    return [...filtered()].sort((a, b) => {
      const x = sv(a);
      const y = sv(b);
      return (x < y ? -1 : x > y ? 1 : 0) * dir();
    });
  });
  const size = () => props.pageSize ?? 25;
  const pages = () => Math.max(1, Math.ceil(sorted().length / size()));
  const visible = () => sorted().slice(page() * size(), page() * size() + size());
  const toggle = (k: string) => {
    if (sortKey() === k) setDir(dir() === 1 ? -1 : 1);
    else {
      setSortKey(k);
      setDir(1);
    }
    setPage(0);
  };
  return (
    <Show
      when={sorted().length > 0}
      fallback={props.empty ?? <EmptyState title="No results" body="Try a different search." />}
    >
      <div class="table-wrap">
        <table class="table">
          <caption class="sr-only">{props.caption}</caption>
          <thead>
            <tr>
              <For each={props.columns}>
                {(c) => (
                  <th
                    scope="col"
                    style={{ "text-align": c.align ?? "left", width: c.width }}
                    aria-sort={
                      sortKey() === c.key
                        ? dir() === 1
                          ? "ascending"
                          : "descending"
                        : c.sortValue
                          ? "none"
                          : undefined
                    }
                  >
                    <Show when={c.sortValue} fallback={c.header}>
                      <button type="button" class="th-sort" onClick={() => toggle(c.key)}>
                        {c.header}
                        <Show when={sortKey() === c.key}>
                          <span aria-hidden="true">{dir() === 1 ? "↑" : "↓"}</span>
                        </Show>
                      </button>
                    </Show>
                  </th>
                )}
              </For>
            </tr>
          </thead>
          <tbody>
            <For each={visible()}>
              {(row) => (
                <tr
                  data-key={props.rowKey(row)}
                  style={props.onRowClick ? { cursor: "pointer" } : undefined}
                  onClick={() => props.onRowClick?.(row)}
                >
                  <For each={props.columns}>
                    {(c) => <td style={{ "text-align": c.align ?? "left" }}>{c.cell(row)}</td>}
                  </For>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
      <Show when={pages() > 1}>
        <Pagination
          page={page() + 1}
          pages={pages()}
          total={sorted().length}
          onPage={(p) => setPage(p - 1)}
        />
      </Show>
    </Show>
  );
}

export function Pagination(props: {
  page: number;
  pages: number;
  total?: number;
  onPage: (p: number) => void;
}) {
  return (
    <nav class="pagination" aria-label="Pagination">
      <span class="subtle">
        Page {props.page} of {props.pages}
        {props.total != null ? ` · ${fmtNumber(props.total)} total` : ""}
      </span>
      <div class="row">
        <Button size="sm" disabled={props.page <= 1} onClick={() => props.onPage(props.page - 1)}>
          Previous
        </Button>
        <Button size="sm" disabled={props.page >= props.pages} onClick={() => props.onPage(props.page + 1)}>
          Next
        </Button>
      </div>
    </nav>
  );
}

/** Cursor pagination ("Load more") for keyset APIs. */
export function LoadMore(props: { hasMore: boolean; loading?: boolean; onMore: () => void }) {
  return (
    <Show when={props.hasMore}>
      <div class="pagination justify-center">
        <Button size="sm" loading={props.loading} onClick={() => props.onMore()}>
          Load more
        </Button>
      </div>
    </Show>
  );
}

export function SearchInput(props: {
  label: string;
  value: string;
  onInput: (v: string) => void;
  placeholder?: string;
  debounceMs?: number;
}) {
  let t: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(t));
  return (
    <div class="row relative minw-220">
      <label class="sr-only" for="search-input">
        {props.label}
      </label>
      <input
        id="search-input"
        class="input"
        type="search"
        placeholder={props.placeholder ?? "Search…"}
        value={props.value}
        onInput={(e) => {
          const v = e.currentTarget.value;
          clearTimeout(t);
          t = setTimeout(() => props.onInput(v), props.debounceMs ?? 200);
        }}
      />
    </div>
  );
}

// ---------------------------------------------------------------- date range

export interface DateRange {
  days: number;
  label: string;
}
export const RANGES: DateRange[] = [
  { days: 7, label: "Last 7 days" },
  { days: 14, label: "Last 14 days" },
  { days: 30, label: "Last 30 days" },
  { days: 90, label: "Last 90 days" },
];

export function DateRangePicker(props: { value: number; onChange: (days: number) => void }) {
  return (
    <div class="field field-inline">
      <label class="sr-only" for="range">
        Date range
      </label>
      <select
        id="range"
        class="select w-auto"
        value={props.value}
        onChange={(e) => props.onChange(Number(e.currentTarget.value))}
      >
        <For each={RANGES}>{(r) => <option value={r.days}>{r.label}</option>}</For>
      </select>
    </div>
  );
}

// ---------------------------------------------------------------- metrics

export function MetricCard(props: {
  label: string;
  value: number | string | null | undefined;
  hint?: string;
  delta?: number;
  spark?: number[];
  loading?: boolean;
}) {
  return (
    <div class="card metric">
      <span class="metric-label">{props.label}</span>
      <Show when={!props.loading} fallback={<Skeleton height="30px" width="60%" />}>
        <span class="metric-value">
          {typeof props.value === "number" ? fmtCompact(props.value) : (props.value ?? "—")}
        </span>
      </Show>
      <div class="row-between">
        <Show when={props.delta != null}>
          <span class={`subtle ${(props.delta ?? 0) >= 0 ? "metric-delta-up" : "metric-delta-down"}`}>
            {(props.delta ?? 0) >= 0 ? "▲" : "▼"} {Math.abs(props.delta ?? 0).toFixed(1)}%
          </span>
        </Show>
        <Show when={props.hint}>
          <span class="subtle">{props.hint}</span>
        </Show>
        <Show when={props.spark && props.spark.length > 1}>
          <Sparkline values={props.spark ?? []} label={`${props.label} trend`} />
        </Show>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- charts (SVG)

export interface Series {
  name: string;
  values: number[];
  color?: string;
}

function niceMax(v: number) {
  if (v <= 0) return 1;
  const p = 10 ** Math.floor(Math.log10(v));
  return Math.ceil(v / p) * p;
}

/** Accessible line chart: role=img with a summary, plus a visually hidden data table. */
export function LineChart(props: {
  title: string;
  labels: string[];
  series: Series[];
  height?: number;
  area?: boolean;
}) {
  const W = 640;
  const H = () => props.height ?? 220;
  const pad = { l: 40, r: 12, t: 12, b: 26 };
  const max = createMemo(() => niceMax(Math.max(0, ...props.series.flatMap((s) => s.values))));
  const x = (i: number) => pad.l + (i * (W - pad.l - pad.r)) / Math.max(1, props.labels.length - 1);
  const y = (v: number) => pad.t + (H() - pad.t - pad.b) * (1 - v / max());
  const path = (vals: number[]) =>
    vals.map((v, i) => `${i ? "L" : "M"}${x(i).toFixed(1)},${y(v).toFixed(1)}`).join(" ");
  const colors = ["var(--chart-1)", "var(--chart-2)", "var(--chart-3)", "var(--chart-4)"];
  const summary = () =>
    `${props.title}. ${props.series.map((s) => `${s.name}: total ${fmtNumber(s.values.reduce((a, b) => a + b, 0))}`).join("; ")}.`;
  const tickIdx = () => {
    const n = props.labels.length;
    const step = Math.max(1, Math.ceil(n / 7));
    return props.labels.map((_, i) => i).filter((i) => i % step === 0 || i === n - 1);
  };
  return (
    <figure class="stack-sm m-0">
      <svg class="chart" viewBox={`0 0 ${W} ${H()}`} role="img" aria-label={summary()}>
        <For each={[0, 0.25, 0.5, 0.75, 1]}>
          {(f) => (
            <>
              <line class="gridline" x1={pad.l} x2={W - pad.r} y1={y(max() * f)} y2={y(max() * f)} />
              <text class="tick" x={pad.l - 6} y={y(max() * f) + 4} text-anchor="end">
                {fmtCompact(max() * f)}
              </text>
            </>
          )}
        </For>
        <For each={tickIdx()}>
          {(i) => (
            <text class="tick" x={x(i)} y={H() - 6} text-anchor="middle">
              {props.labels[i]?.slice(5)}
            </text>
          )}
        </For>
        <For each={props.series}>
          {(s, i) => (
            <>
              <Show when={props.area}>
                <path
                  d={`${path(s.values)} L${x(s.values.length - 1)},${y(0)} L${x(0)},${y(0)} Z`}
                  fill={s.color ?? colors[i() % 4]}
                  opacity="0.08"
                />
              </Show>
              <path
                d={path(s.values)}
                fill="none"
                stroke={s.color ?? colors[i() % 4]}
                stroke-width="2"
                stroke-linejoin="round"
              />
            </>
          )}
        </For>
      </svg>
      <figcaption class="legend">
        <For each={props.series}>
          {(s, i) => (
            <span>
              <span class="legend-swatch" style={{ background: s.color ?? colors[i() % 4] }} />
              {s.name}
            </span>
          )}
        </For>
      </figcaption>
      <table class="sr-only">
        <caption>{props.title}</caption>
        <thead>
          <tr>
            <th scope="col">Date</th>
            <For each={props.series}>{(s) => <th scope="col">{s.name}</th>}</For>
          </tr>
        </thead>
        <tbody>
          <For each={props.labels}>
            {(l, i) => (
              <tr>
                <th scope="row">{l}</th>
                <For each={props.series}>{(s) => <td>{s.values[i()]}</td>}</For>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </figure>
  );
}

export function BarChart(props: {
  title: string;
  items: { label: string; value: number }[];
  height?: number;
}) {
  const W = 640;
  const H = () => props.height ?? 200;
  const max = createMemo(() => niceMax(Math.max(0, ...props.items.map((i) => i.value))));
  const bw = () => (W - 40) / Math.max(1, props.items.length);
  return (
    <svg
      class="chart"
      viewBox={`0 0 ${W} ${H()}`}
      role="img"
      aria-label={`${props.title}: ${props.items.map((i) => `${i.label} ${i.value}`).join(", ")}`}
    >
      <For each={props.items}>
        {(it, i) => {
          const h = () => ((H() - 30) * it.value) / max();
          return (
            <>
              <rect
                x={30 + i() * bw() + 4}
                y={H() - 20 - h()}
                width={Math.max(2, bw() - 8)}
                height={h()}
                rx="3"
                fill="var(--chart-1)"
              />
              <text class="tick" x={30 + i() * bw() + bw() / 2} y={H() - 6} text-anchor="middle">
                {it.label}
              </text>
            </>
          );
        }}
      </For>
    </svg>
  );
}

export function Sparkline(props: { values: number[]; label: string }) {
  const W = 80;
  const H = 24;
  const max = () => Math.max(1, ...props.values);
  const d = () =>
    props.values
      .map(
        (v, i) =>
          `${i ? "L" : "M"}${((i * W) / Math.max(1, props.values.length - 1)).toFixed(1)},${(H - (v / max()) * H).toFixed(1)}`,
      )
      .join(" ");
  return (
    <svg width={W} height={H} viewBox={`0 0 ${W} ${H}`} role="img" aria-label={props.label}>
      <path d={d()} fill="none" stroke="var(--chart-1)" stroke-width="1.5" />
    </svg>
  );
}

// ---------------------------------------------------------------- activity & realtime

export function ActivityFeed(props: { items: AuditRow[]; empty?: string }) {
  return (
    <Show
      when={props.items.length > 0}
      fallback={<EmptyState icon="activity" title={props.empty ?? "No recent activity"} />}
    >
      <ul class="feed">
        <For each={props.items}>
          {(e) => (
            <li>
              <span class="avatar icon-28" aria-hidden="true">
                <Icon
                  name={
                    e.outcome === "denied"
                      ? "lock"
                      : e.action.startsWith("security") || e.action.startsWith("user.login")
                        ? "shield"
                        : "activity"
                  }
                  size={14}
                />
              </span>
              <div class="grow">
                <div class="row-between">
                  <span>
                    <strong>{describeAction(e.action)}</strong>{" "}
                    <span class="subtle">{e.actor_label ?? e.actor_type}</span>
                  </span>
                  <time class="subtle" datetime={e.occurred_at} title={e.occurred_at}>
                    {fmtRelative(e.occurred_at)}
                  </time>
                </div>
                <div class="row">
                  <code class="subtle">{e.action}</code>
                  <Show when={e.outcome !== "success"}>
                    <Badge tone={e.outcome === "denied" ? "danger" : "warning"}>{e.outcome}</Badge>
                  </Show>
                </div>
              </div>
            </li>
          )}
        </For>
      </ul>
    </Show>
  );
}

export function RealtimeIndicator() {
  const label = () =>
    ({ open: "Live", connecting: "Connecting…", closed: "Reconnecting…", idle: "Offline" })[
      connectionState()
    ];
  return (
    <span class="row subtle" role="status" aria-live="polite" title="Realtime connection">
      <span class="live-dot" data-state={connectionState()} aria-hidden="true" />
      {label()}
    </span>
  );
}
