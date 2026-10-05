import { expect, type Page, test } from "@playwright/test";

const shots = "test-results/journey";
const shot = (page: Page, name: string) => page.screenshot({ path: `${shots}/${name}.png`, fullPage: true });

test("day one: register, use the product, see the evidence, sign out", async ({ page }) => {
  const email = `journey-${Date.now().toString(36)}@example.com`;

  // Registration through the BFF and the identity provider (mock IdP in development).
  await page.goto("/register");
  await shot(page, "00-register");
  await page.getByLabel("Email").fill(email);
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  await expect(page.getByRole("heading", { name: /mock IdP/ })).toBeVisible();
  await page.getByRole("button", { name: "Continue" }).click();
  await page.waitForURL(/\/dashboard/);
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  await shot(page, "01-dashboard");

  const session = await page.evaluate(async () => (await fetch("/api/v1/session")).json());
  expect(session.authenticated).toBe(true);
  expect(session.user.email).toBe(email);

  // Account center.
  for (const [path, name] of [
    ["/account/profile", "02-account-profile"],
    ["/account/security", "03-account-security"],
    ["/account/sessions", "04-account-sessions"],
  ] as const) {
    await page.goto(path);
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    await shot(page, name);
  }

  // A database write that runs as a background job, observed live.
  const slug = session.organizations.find((o: { personal: boolean }) => o.personal).slug as string;
  await page.goto(`/org/${slug}/runs`);
  await page.getByRole("button", { name: "New run" }).click();
  const dialog = page.getByRole("dialog", { name: "New run" });
  await dialog.getByLabel("Label").fill("Journey run");
  await dialog.getByLabel("Calls").fill("25");
  await dialog.getByRole("button", { name: "Start run" }).click();
  const row = page.getByRole("row", { name: /Journey run/ });
  await expect(row.getByText("completed")).toBeVisible({ timeout: 30_000 });
  await shot(page, "05-runs");

  // Evidence: the audit trail recorded it.
  await page.goto(`/org/${slug}/audit`);
  await expect(page.getByText("run.created").first()).toBeVisible();
  await shot(page, "06-audit");

  await page.goto(`/org/${slug}`);
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  await shot(page, "07-org-overview");

  if (session.features.organizations) {
    for (const [path, name] of [
      [`/org/${slug}/members`, "08-org-members"],
      [`/org/${slug}/settings`, "09-org-settings"],
    ] as const) {
      await page.goto(path);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await shot(page, name);
    }
  }

  // The system console is a separate trust level: an ordinary user is refused by the API.
  const admin = await page.evaluate(async () => (await fetch("/api/v1/admin/overview")).status);
  expect([403, 404]).toContain(admin);

  await page.goto("/logout");
  await page.waitForURL(/\/login/);
  const after = await page.evaluate(async () => (await fetch("/api/v1/session")).json());
  expect(after.authenticated).toBe(false);
  await shot(page, "10-signed-out");
});
