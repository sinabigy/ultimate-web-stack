import { expect, test } from "@playwright/test";
import { signIn, unique } from "./helpers";

// The example application end to end: UI → API → PostgreSQL job → worker → outbound engine →
// simulated provider → progress over SSE → completion + notification, with no page reload.
test("a run executes in the background and streams live progress", async ({ page }) => {
  await signIn(page, `${unique("runner")}@e2e.test`, { roles: "system_admin", amr: "mfa" });
  const session = await page.evaluate(async () => (await fetch("/api/v1/session")).json());
  const slug = session.organizations[0].slug as string;
  await page.goto(`/org/${slug}/runs`);
  // SSE connected (the page's own indicator; the header shows another one).
  await expect(page.locator("#main").getByRole("status", { name: "Realtime connection" })).toHaveText(/Live/);
  await page.getByRole("button", { name: "New run" }).click();
  const dialog = page.getByRole("dialog", { name: "New run" });
  await dialog.getByLabel("Label").fill("E2E batch");
  await dialog.getByLabel("Calls").fill("300");
  await dialog.getByRole("button", { name: "Start run" }).click();
  const row = page.getByRole("row", { name: /E2E batch/ });
  await expect(row).toBeVisible();
  // Live updates arrive via SSE (no reload) until the run completes.
  await expect(row.getByText("completed")).toBeVisible({ timeout: 20_000 });
  await expect(row.getByText(/300 ok/)).toBeVisible();
  // Completion notification (realtime toast + bell count)
  await expect(page.getByRole("button", { name: /Notifications, \d+ unread/ })).toBeVisible();
  // Overview reflects the work
  await page.goto(`/org/${slug}`);
  await expect(page.getByText("Calls succeeded")).toBeVisible();
  // Live provider health from the outbound engine
  await page.goto("/admin/providers");
  await expect(page.getByText("circuit closed")).toBeVisible();
  await expect(page.getByText(/Success rate/)).toBeVisible();
});
