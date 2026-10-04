import { expect, test } from "@playwright/test";
import { signIn, unique } from "./helpers";

// The backend serves the SPA with a strict CSP (no inline scripts or styles). Any violation means
// a build or component regression (e.g. a static style={{}} prop compiled into template HTML).
test("no Content-Security-Policy violations across the main pages", async ({ page }) => {
  const violations: string[] = [];
  page.on("console", (m) => {
    if (m.type() === "error" && /Content Security Policy/i.test(m.text()))
      violations.push(`${page.url()}: ${m.text().slice(0, 140)}`);
  });
  await page.goto("/login");
  await signIn(page, `${unique("csp")}@e2e.test`);
  for (const path of [
    "/dashboard",
    "/account/profile",
    "/account/security",
    "/account/passkeys",
    "/account/sessions",
    "/account/preferences",
    "/account/notifications",
  ]) {
    await page.goto(path);
    await page.waitForLoadState("networkidle");
  }
  await page.keyboard.press("Control+k");
  await page.keyboard.press("Escape");
  expect(violations).toEqual([]);
  const csp = (await page.request.get("/dashboard")).headers()["content-security-policy"];
  expect(csp).toContain("script-src 'self'");
  expect(csp).not.toContain("unsafe-inline");
});
