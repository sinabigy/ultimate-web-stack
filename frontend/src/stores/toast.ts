import { createSignal } from "solid-js";

export type ToastTone = "info" | "success" | "warning" | "danger";
export interface Toast {
  id: number;
  title: string;
  body?: string;
  tone: ToastTone;
}

const [toasts, setToasts] = createSignal<Toast[]>([]);
let next = 1;

export { toasts };

export function toast(title: string, opts: { body?: string; tone?: ToastTone; ms?: number } = {}) {
  const t: Toast = { id: next++, title, body: opts.body, tone: opts.tone ?? "info" };
  setToasts((list) => [...list.slice(-4), t]);
  const ms = opts.ms ?? (t.tone === "danger" ? 8000 : 4000);
  setTimeout(() => dismiss(t.id), ms);
  return t.id;
}

export function dismiss(id: number) {
  setToasts((list) => list.filter((t) => t.id !== id));
}
