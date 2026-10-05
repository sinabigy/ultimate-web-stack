import { type Browser, type BrowserContext, expect, type Page, test } from "@playwright/test";
import { signIn, unique } from "../e2e/helpers";

// Browser acceptance against a RUNNING full stack (`./dev up`, started with
// APP__AUTH__SYSTEM_ROLES_FROM_IDP=true so the mock IdP can assert system roles). One test per
// role; every boundary is checked in the browser AND at the API, because hiding a control is
// never the evidence. Screenshots: test-results/journey/acceptance-*.png.
//   ./dev test --journey

test.describe.configure({ mode: "serial" });
const shot = (page: Page, name: string) =>
  page.screenshot({ path: `test-results/journey/acceptance-${name}.png`, fullPage: true });

async function user(browser: Browser, email: string, opts: Parameters<typeof signIn>[2] = {}) {
  const ctx: BrowserContext = await browser.newContext();
  const page = await ctx.newPage();
  await signIn(page, email, opts);
  return { ctx, page };
}
const status = (page: Page, path: string, method = "GET") =>
  page.evaluate(
    async ([p, m]) => {
      const s = await (await fetch("/api/v1/session")).json();
      const r = await fetch(p, {
        method: m,
        headers: { "Content-Type": "application/json", "X-CSRF-Token": s.csrf_token ?? "" },
        body: m === "GET" ? undefined : "{}",
      });
      return r.status;
    },
    [path, method] as const,
  );
const session = (page: Page) => page.evaluate(async () => (await fetch("/api/v1/session")).json());

const orgSlug = unique("accept-a");

test("anonymous: public page, protected and admin routes refused", async ({ page }) => {
  await page.goto("/login");
  await expect(page.getByRole("button", { name: "Continue", exact: true })).toBeVisible();
  await shot(page, "00-login");
  for (const path of ["/dashboard", "/admin/users", `/org/${orgSlug}`]) {
    await page.goto(path);
    await expect(page).toHaveURL(/\/login/);
  }
  for (const path of ["/api/v1/dashboard", "/api/v1/admin/overview", "/api/v1/orgs"]) {
    expect(await page.evaluate(async (p) => (await fetch(p)).status, path), path).toBe(401);
  }
});

test("session: sign in, CSRF enforced, reload keeps the session, sign out, sign in again", async ({
  page,
}) => {
  const email = `${unique("session")}@example.com`;
  await signIn(page, email);
  await expect(page).toHaveURL(/\/dashboard/);
  expect((await session(page)).authenticated).toBe(true);
  // A state-changing request without the CSRF token is refused by the server.
  const noCsrf = await page.evaluate(
    async () => (await fetch("/api/v1/orgs", { method: "POST", body: "{}" })).status,
  );
  expect(noCsrf).toBe(403);
  await page.reload();
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  expect((await session(page)).authenticated, "session survives a reload").toBe(true);
  // Logout regression (CSRF race): /logout opened directly, before the SPA has a token.
  for (let i = 0; i < 3; i++) {
    await page.goto("/logout");
    await page.waitForURL(/\/login/);
    expect((await session(page)).authenticated, `signed out (attempt ${i + 1})`).toBe(false);
    await page.goto("/dashboard");
    await expect(page).toHaveURL(/\/login/);
    await signIn(page, email);
    expect((await session(page)).authenticated, "signed in again").toBe(true);
  }
  await shot(page, "01-signed-in-again");
});

test("ordinary user, organization member, organization admin and cross-tenant outsider", async ({
  browser,
}) => {
  const owner = await user(browser, `${unique("owner")}@example.com`);
  const created = await owner.page.evaluate(async (s) => {
    const sess = await (await fetch("/api/v1/session")).json();
    const r = await fetch("/api/v1/orgs", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-CSRF-Token": sess.csrf_token },
      body: JSON.stringify({ name: "Acceptance A", slug: s }),
    });
    return r.status;
  }, orgSlug);
  expect(created).toBe(201);

  // Ordinary user: dashboard, account, own workspace; no system admin.
  const memberEmail = `${unique("member")}@example.com`;
  const member = await user(browser, memberEmail);
  await member.page.goto("/dashboard");
  await expect(member.page.getByRole("heading", { level: 1 })).toBeVisible();
  await shot(member.page, "02-user-dashboard");
  await member.page.goto("/account/profile");
  await expect(member.page.getByRole("heading", { level: 1 })).toBeVisible();
  await member.page.goto("/admin/users");
  await expect(member.page.getByText("restricted to system administrators")).toBeVisible();
  await shot(member.page, "03-user-admin-refused");
  expect(await status(member.page, "/api/v1/admin/users")).toBe(403);

  // Invite → accept → organization member.
  await owner.page.goto(`/org/${orgSlug}/members`);
  await owner.page.getByRole("button", { name: "Invite" }).click();
  const dialog = owner.page.getByRole("dialog", { name: "Invite a member" });
  await dialog.getByLabel("Email").fill(memberEmail);
  await dialog.getByRole("button", { name: "Send invitation" }).click();
  const link = await dialog.getByLabel("Invitation link").inputValue();
  await dialog.getByRole("button", { name: "Done" }).click();
  await member.page.goto(new URL(link).pathname);
  await member.page.getByRole("button", { name: "Accept invitation" }).click();
  await expect(member.page).toHaveURL(new RegExp(`/org/${orgSlug}$`));
  await shot(member.page, "04-member-org-overview");

  // Member: own-tenant resources allowed, organization administration refused (API, not UI).
  expect(await status(member.page, `/api/v1/orgs/${orgSlug}/runs`)).toBe(200);
  expect(await status(member.page, `/api/v1/orgs/${orgSlug}/members`)).toBe(200);
  expect(await status(member.page, `/api/v1/orgs/${orgSlug}/audit`)).toBe(403);
  expect(await status(member.page, `/api/v1/orgs/${orgSlug}`, "PATCH")).toBe(403);

  // Organization admin (the owner): administration allowed, platform refused.
  await owner.page.goto(`/org/${orgSlug}/settings`);
  await expect(owner.page.getByRole("heading", { level: 1 })).toBeVisible();
  await shot(owner.page, "05-org-admin-settings");
  await owner.page.goto(`/org/${orgSlug}/audit`);
  await expect(owner.page.getByRole("cell", { name: "organization.member_added" })).toBeVisible();
  await shot(owner.page, "06-org-audit");
  expect(await status(owner.page, `/api/v1/orgs/${orgSlug}/audit`)).toBe(200);
  expect(await status(owner.page, "/api/v1/admin/overview")).toBe(403);

  // Tenant B user → tenant A: not found, in the browser and at the API.
  const outsider = await user(browser, `${unique("outsider")}@example.com`);
  await outsider.page.goto(`/org/${orgSlug}`);
  await expect(outsider.page.getByText("Not found", { exact: true })).toBeVisible();
  await shot(outsider.page, "07-cross-tenant-not-found");
  for (const p of [
    `/api/v1/orgs/${orgSlug}`,
    `/api/v1/orgs/${orgSlug}/runs`,
    `/api/v1/orgs/${orgSlug}/members`,
  ]) {
    expect(await status(outsider.page, p), p).toBe(404);
  }
  for (const u of [owner, member, outsider]) await u.ctx.close();
});

test("system auditor: every console page readable, mutations refused", async ({ browser }) => {
  const target = await user(browser, `${unique("target")}@example.com`);
  const targetId = (await session(target.page)).user.id as string;
  const auditor = await user(browser, `${unique("auditor")}@example.com`, {
    roles: "system_auditor",
    amr: "mfa",
  });
  for (const path of ["/admin", "/admin/users", "/admin/organizations", "/admin/jobs", "/admin/audit"]) {
    await auditor.page.goto(path);
    await expect(auditor.page.getByRole("heading", { level: 1 })).toBeVisible();
  }
  await shot(auditor.page, "08-auditor-audit");
  expect(await status(auditor.page, "/api/v1/admin/users")).toBe(200);
  expect(await status(auditor.page, `/api/v1/admin/users/${targetId}`, "PATCH")).toBe(403);
  expect(await status(auditor.page, `/api/v1/admin/users/${targetId}/revoke-sessions`, "POST")).toBe(403);
  expect((await session(target.page)).authenticated, "the auditor changed nothing").toBe(true);
  for (const u of [target, auditor]) await u.ctx.close();
});

test("system admin (MFA): overview, users, organizations, jobs, audit, providers, health", async ({
  browser,
}) => {
  const admin = await user(browser, `${unique("root")}@example.com`, { roles: "system_admin", amr: "mfa" });
  const pages: [string, string][] = [
    ["/admin", "09-admin-overview"],
    ["/admin/users", "10-admin-users"],
    ["/admin/organizations", "11-admin-organizations"],
    ["/admin/jobs", "12-admin-jobs"],
    ["/admin/audit", "13-admin-audit"],
    ["/admin/providers", "14-admin-providers"],
    ["/admin/system", "15-admin-system-health"],
  ];
  for (const [path, name] of pages) {
    await admin.page.goto(path);
    await expect(admin.page.getByRole("heading", { level: 1 })).toBeVisible();
    await shot(admin.page, name);
  }
  await admin.page.goto("/admin/system");
  await expect(admin.page.getByText("postgres").first()).toBeVisible();
  for (const p of ["overview", "users", "organizations", "jobs", "audit", "providers", "system"]) {
    expect(await status(admin.page, `/api/v1/admin/${p}`), p).toBe(200);
  }
  // Without MFA the same role is asked to step up: the server, not the UI, decides.
  const pwd = await user(browser, `${unique("root-pwd")}@example.com`, { roles: "system_admin" });
  expect(await status(pwd.page, "/api/v1/admin/users")).toBe(403);
  for (const u of [admin, pwd]) await u.ctx.close();
});
