import { useLocation } from "@solidjs/router";
import { createMemo, createResource } from "solid-js";
import { api } from "../../api/endpoints";
import { useSession } from "../../auth/session";

const LAST_ORG = "app.lastOrg";

export function rememberOrg(slug: string) {
  try {
    localStorage.setItem(LAST_ORG, slug);
  } catch {
    /* ignore */
  }
}

/** The organisation the shell is focused on: from the URL, else last used, else personal. */
export function useCurrentOrg() {
  const location = useLocation();
  const { session } = useSession();
  const slug = createMemo(() => {
    const m = location.pathname.match(/^\/org\/([^/]+)/);
    if (m?.[1]) return decodeURIComponent(m[1]);
    const orgs = session()?.organizations ?? [];
    let last: string | null = null;
    try {
      last = localStorage.getItem(LAST_ORG);
    } catch {
      /* ignore */
    }
    return (
      orgs.find((o) => o.slug === last)?.slug ?? orgs.find((o) => o.personal)?.slug ?? orgs[0]?.slug ?? null
    );
  });
  // Permissions drive what the navigation *shows*; the API still authorizes every call.
  const [detail] = createResource(slug, (s) => api.orgs.get(s).catch(() => undefined));
  const can = (perm: string) => detail()?.permissions.includes(perm) ?? false;
  return { slug, detail, can };
}
