import { For } from "solid-js";
import { dismiss, toasts } from "../stores/toast";
import { Icon } from "./icons";

/** Toast region: announced politely to assistive technology. */
export function Toasts() {
  return (
    <section class="toasts" aria-live="polite" aria-label="Notifications">
      <For each={toasts()}>
        {(t) => (
          <div class={`toast alert-${t.tone}`} role={t.tone === "danger" ? "alert" : "status"}>
            <div class="row-between">
              <strong>{t.title}</strong>
              <button
                type="button"
                class="btn btn-ghost btn-sm btn-icon"
                aria-label="Dismiss"
                onClick={() => dismiss(t.id)}
              >
                <Icon name="x" size={14} />
              </button>
            </div>
            {t.body && <p class="subtle">{t.body}</p>}
          </div>
        )}
      </For>
    </section>
  );
}
