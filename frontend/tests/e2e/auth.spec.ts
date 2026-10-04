import { expect, test } from "@playwright/test";
import { signIn, unique } from "./helpers";

test("anonymous visitors are sent to login and returned afterwards", async ({ page }) => {
  await page.goto("/account/sessions");
  await expect(page).toHaveURL(/\/login\?return_to=%2Faccount%2Fsessions/);
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
  // Only configured methods are offered: passkey on; no social providers configured.
  await expect(page.getByRole("button", { name: "Continue with passkey" })).toBeVisible();
  await expect(page.getByRole("button", { name: /Continue with Google/ })).toHaveCount(0);
  await signIn(page, `${unique("ret")}@e2e.test`, { returnTo: "/account/sessions" });
  await expect(page).toHaveURL(/\/account\/sessions$/);
  await expect(page.getByRole("heading", { name: "Sessions & devices" })).toBeVisible();
  await expect(page.getByText("This device")).toBeVisible();
});

test("login, dashboard, sign out", async ({ page, context }) => {
  const email = `${unique("ada")}@e2e.test`;
  await signIn(page, email);
  await expect(page).toHaveURL(/\/dashboard$/);
  await expect(page.getByRole("heading", { name: /Welcome back/ })).toBeVisible();
  // The browser holds only an HttpOnly session cookie; nothing auth-related in web storage.
  const cookies = await context.cookies();
  const session = cookies.find((c) => c.name === "app_session");
  expect(session?.httpOnly).toBe(true);
  expect(session?.sameSite).toBe("Lax");
  const storage = await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }));
  expect(storage).not.toMatch(/token|eyJ/i);
  await page.getByRole("button", { name: "Account menu" }).click();
  await page.getByRole("menuitem", { name: "Sign out" }).click();
  await expect(page).toHaveURL(/\/login/);
  await page.goto("/dashboard");
  await expect(page).toHaveURL(/\/login/);
});

test("IdP errors are explained on the login page", async ({ page }) => {
  await page.goto("/login?error=login_rejected");
  await expect(page.getByRole("alert")).toContainText("couldn't verify");
});

test("command palette is keyboard operable", async ({ page }) => {
  await signIn(page, `${unique("kbd")}@e2e.test`);
  // Wait for the signed-in shell (which owns the shortcut) before using it.
  await expect(page.getByRole("button", { name: "Search (Ctrl+K)" })).toBeVisible();
  await page.keyboard.press("Control+k");
  const input = page.getByRole("combobox", { name: "Search commands" });
  await expect(input).toBeFocused();
  await input.fill("sessions");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/\/account\/sessions$/);
});
