import { expect, test } from "@playwright/test";
import { signIn, unique } from "./helpers";

// Every application route renders real data from the backend (no error state, no blank page).
test("every org and admin route renders", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  // A system admin with MFA so admin routes are reachable; they also own an organization.
  await signIn(page, `${unique("tour")}@e2e.test`, { roles: "system_admin", amr: "mfa" });
  const slug = unique("tour");
  const status = await page.evaluate(async (s) => {
    const sess = await (await fetch("/api/v1/session")).json();
    const r = await fetch("/api/v1/orgs", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-CSRF-Token": sess.csrf_token },
      body: JSON.stringify({ name: "Tour Org", slug: s }),
    });
    return r.status;
  }, slug);
  expect(status).toBe(201);
  const routes: [string, RegExp][] = [
    ["/dashboard", /Welcome back/],
    [`/org/${slug}`, /Overview/],
    [`/org/${slug}/runs`, /Runs/],
    [`/org/${slug}/members`, /Members/],
    [`/org/${slug}/teams`, /Teams/],
    [`/org/${slug}/roles`, /Roles & permissions/],
    [`/org/${slug}/settings`, /Settings/],
    [`/org/${slug}/audit`, /Audit log/],
    [`/org/${slug}/api-keys`, /API keys/],
    [`/org/${slug}/billing`, /Billing/],
    ["/account/profile", /Profile/],
    ["/account/security", /Security/],
    ["/account/passkeys", /Passkeys/],
    ["/account/mfa", /Two-factor/],
    ["/account/sessions", /Sessions & devices/],
    ["/account/preferences", /Preferences/],
    ["/account/notifications", /Notifications/],
    ["/account/activity", /Activity/],
    ["/admin", /Administration/],
    ["/admin/users", /Users/],
    ["/admin/organizations", /Organizations/],
    ["/admin/roles", /Roles & permissions/],
    ["/admin/audit", /Audit log/],
    ["/admin/jobs", /Background jobs/],
    ["/admin/providers", /External providers/],
    ["/admin/system", /System/],
    ["/verify-email", /Verify your email/],
    ["/forgot-password", /Reset your password/],
  ];
  for (const [path, heading] of routes) {
    await page.goto(path);
    await expect(page.getByRole("heading", { level: 1, name: heading }).first(), path).toBeVisible();
    await expect(page.getByText("Something went wrong"), path).toHaveCount(0);
    await expect(page.getByText("Access denied"), path).toHaveCount(0);
  }
  // admin user detail via the users table
  await page.goto("/admin/users");
  await page.getByRole("link", { name: /tour-/ }).first().click();
  await expect(page.getByRole("heading", { name: "System role" })).toBeVisible();
  expect(errors).toEqual([]);
});
