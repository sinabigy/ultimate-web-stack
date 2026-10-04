import { type Browser, expect, type Page } from "@playwright/test";

/** Sign in through the real BFF flow and the mock IdP's form. */
export async function signIn(
  page: Page,
  email: string,
  opts: { amr?: "pwd" | "mfa" | "passkey"; roles?: string; returnTo?: string } = {},
) {
  await page.goto(opts.returnTo ? `/login?return_to=${encodeURIComponent(opts.returnTo)}` : "/login");
  await page.getByLabel("Email").fill(email);
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  // Mock identity provider page
  await expect(page.getByRole("heading", { name: /Sign in \(mock IdP\)/ })).toBeVisible();
  await page.getByLabel("Method").selectOption(opts.amr ?? "pwd");
  if (opts.roles) await page.getByLabel(/IdP roles/).fill(opts.roles);
  await page.getByRole("button", { name: "Continue" }).click();
  await page.waitForURL((u) => !u.pathname.startsWith("/authorize") && !u.pathname.startsWith("/auth/"));
}

export async function newUser(browser: Browser, email: string, opts: Parameters<typeof signIn>[2] = {}) {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  await signIn(page, email, opts);
  return { ctx, page };
}

export const unique = (p: string) => `${p}-${Date.now().toString(36)}${Math.floor(Math.random() * 1e4)}`;
