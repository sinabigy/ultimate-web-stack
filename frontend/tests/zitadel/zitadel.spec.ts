import { expect, type Page, test } from "@playwright/test";
import { devPort } from "../../scripts/dev-ports.mjs";

// Requires: ./dev up --identity zitadel && python3 scripts/zitadel_bootstrap.py
const PASSWORD = "Password1!Password1!";
const ZITADEL = `localhost:${devPort("DEV_ZITADEL_PORT")}`;
const APP = `localhost:${devPort("DEV_API_PORT")}`;

async function zitadelLogin(page: Page, email: string) {
  await page.goto("/login");
  await page.getByLabel("Email").fill(email);
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  // ZITADEL's hosted login (login_hint skips the username step)
  await page.waitForURL(new RegExp(`${ZITADEL}/ui/login`));
  await page.locator("#password").fill(PASSWORD);
  await page.locator("#submit-button").click();
  // ZITADEL may offer MFA enrolment after password login; skip it for this flow.
  const skip = page.getByRole("button", { name: /skip/i });
  if (await skip.isVisible({ timeout: 3000 }).catch(() => false)) await skip.click();
  await page.waitForURL(new RegExp(`${APP}/dashboard`));
}

test("real ZITADEL: login, IdP-managed system role, MFA step-up, IdP admin API, RP logout", async ({
  page,
}) => {
  await zitadelLogin(page, "alice@example.com");
  await expect(page.getByRole("heading", { name: /Welcome back, Alice/ })).toBeVisible();

  const session = await page.evaluate(async () => (await fetch("/api/v1/session")).json());
  expect(session.user.email).toBe("alice@example.com");
  expect(session.user.email_verified).toBe(true);
  expect(session.user.system_role).toBe("system_admin"); // from the ZITADEL project-role claim
  expect(session.session.mfa).toBe(false); // password only
  expect(session.session.amr).toContain("pwd");

  // System admin by role, but MFA is required for administration: step-up demanded.
  const admin = await page.evaluate(async () => {
    const r = await fetch("/api/v1/admin/system");
    return [r.status, (await r.json()).code];
  });
  expect(admin).toEqual([403, "mfa_required"]);

  // Credential state comes from ZITADEL's management API through the IdentityAdmin adapter.
  const sec = await page.evaluate(async () => (await fetch("/api/v1/account/security")).json());
  expect(sec.identity_provider.available).toBe(true);
  expect(sec.identity_provider.overview.methods.map((m: { kind: string }) => m.kind)).toContain("password");
  expect(sec.identity_provider.overview.manage_url).toContain(ZITADEL);

  // RP-initiated logout ends the ZITADEL session too and returns to /login.
  await page.getByRole("button", { name: "Account menu" }).click();
  await page.getByRole("menuitem", { name: "Sign out" }).click();
  await page.waitForURL(new RegExp(`${APP}/login`));
  expect(
    await page.evaluate(async () => (await fetch("/api/v1/session")).json().then((s) => s.authenticated)),
  ).toBe(false);
});

test("real ZITADEL: ordinary user has no system access", async ({ page }) => {
  await zitadelLogin(page, "bob@example.com");
  const s = await page.evaluate(async () => (await fetch("/api/v1/session")).json());
  expect(s.user.system_role).toBe("none");
  expect(await page.evaluate(async () => (await fetch("/api/v1/admin/users")).status)).toBe(403);
});

test("real ZITADEL: service account (client credentials JWT) calls the tenant API", async ({
  page,
  request,
}) => {
  const env = Object.fromEntries(
    (await import("node:fs"))
      .readFileSync("../.env.zitadel", "utf8")
      .split("\n")
      .filter((l) => l.includes("="))
      .map((l) => [l.slice(0, l.indexOf("=")), l.slice(l.indexOf("=") + 1)]),
  ) as Record<string, string>;
  await zitadelLogin(page, "bob@example.com");
  const s = await page.evaluate(async () => (await fetch("/api/v1/session")).json());
  const slug = s.organizations[0].slug as string;
  // Bob (owner of his workspace) registers the ZITADEL service account as an org credential.
  const reg = await page.evaluate(
    async ([slug, subject, csrf]) =>
      (
        await fetch(`/api/v1/orgs/${slug}/service-clients`, {
          method: "POST",
          headers: { "Content-Type": "application/json", "X-CSRF-Token": csrf as string },
          body: JSON.stringify({ subject, name: "Reporting", scopes: ["runs:read"] }),
        })
      ).status,
    [slug, env.ZITADEL_SERVICE_ACCOUNT_ID, s.csrf_token],
  );
  expect(reg).toBe(201);
  // Client credentials grant at ZITADEL; the project-audience scope puts the project id in `aud`.
  const tok = await request.post(`http://${ZITADEL}/oauth/v2/token`, {
    form: {
      grant_type: "client_credentials",
      scope: `openid urn:zitadel:iam:org:project:id:${env.ZITADEL_PROJECT_ID}:aud`,
    },
    headers: {
      Authorization: `Basic ${Buffer.from(`${env.ZITADEL_SERVICE_CLIENT_ID}:${env.ZITADEL_SERVICE_CLIENT_SECRET}`).toString("base64")}`,
    },
  });
  expect(tok.status()).toBe(200);
  const access = (await tok.json()).access_token as string;
  expect(access.split(".")).toHaveLength(3); // JWT access token
  const ok = await request.get(`/api/v1/orgs/${slug}/runs`, {
    headers: { Authorization: `Bearer ${access}` },
  });
  expect(ok.status()).toBe(200);
  // Not a user: no account endpoints, and no other tenants.
  expect(
    (await request.get("/api/v1/dashboard", { headers: { Authorization: `Bearer ${access}` } })).status(),
  ).toBe(403);
  expect(
    (
      await request.get("/api/v1/orgs/does-not-exist/runs", {
        headers: { Authorization: `Bearer ${access}` },
      })
    ).status(),
  ).toBe(404);
  // Tampered token is rejected.
  expect(
    (
      await request.get(`/api/v1/orgs/${slug}/runs`, {
        headers: { Authorization: `Bearer ${access.slice(0, -4)}AAAA` },
      })
    ).status(),
  ).toBe(401);
});
