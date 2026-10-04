import { createSignal } from "solid-js";

export type Theme = "system" | "light" | "dark";
const KEY = "app.theme";

function read(): Theme {
  try {
    const v = localStorage.getItem(KEY);
    return v === "light" || v === "dark" ? v : "system";
  } catch {
    return "system";
  }
}

const [theme, setThemeSignal] = createSignal<Theme>(read());

export { theme };

export function applyTheme(t: Theme = theme()) {
  const root = document.documentElement;
  if (t === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", t);
}

/** UI preference only (not a secret); stored locally and mirrored to account preferences. */
export function setTheme(t: Theme) {
  setThemeSignal(t);
  try {
    localStorage.setItem(KEY, t);
  } catch {
    /* storage unavailable: preference lasts for this tab */
  }
  applyTheme(t);
}
