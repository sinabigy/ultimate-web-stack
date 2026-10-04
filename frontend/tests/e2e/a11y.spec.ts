import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import { signIn, unique } from "./helpers";

// WCAG 2.2 A/AA automated checks. Automated tools catch a subset of issues; keyboard and
// screen-reader flows are covered by the other specs and manual review.
const scan = async (page: import("@playwright/test").Page) =>
  (
    await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"]).analyze()
  ).violations.filter((v) => v.impact === "serious" || v.impact === "critical");

test("login page has no serious accessibility violations", async ({ page }) => {
  await page.goto("/login");
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
  expect(await scan(page)).toEqual([]);
});

for (const theme of ["light", "dark"] as const) {
  test(`signed-in pages pass axe (${theme} theme)`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page, `${unique("a11y")}@e2e.test`);
    for (const path of [
      "/dashboard",
      "/account/profile",
      "/account/security",
      "/account/sessions",
      "/account/preferences",
    ]) {
      await page.goto(path);
      await page.waitForLoadState("networkidle");
      const v = await scan(page);
      expect(v, `${path}: ${v.map((x) => `${x.id}: ${x.nodes[0]?.target}`).join(", ")}`).toEqual([]);
    }
  });
}

test("keyboard: skip link and focus visibility", async ({ page }) => {
  await signIn(page, `${unique("kb")}@e2e.test`);
  await expect(page.getByRole("heading", { name: /Welcome back/ })).toBeVisible();
  await page.keyboard.press("Tab");
  const skip = page.getByRole("link", { name: "Skip to content" });
  await expect(skip).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.locator("#main")).toBeFocused();
});
