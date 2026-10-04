import { expect, test } from "@playwright/test";
import { newUser, unique } from "./helpers";

test("organization lifecycle: create, invite, accept, roles, run, isolation", async ({ browser }) => {
  const owner = await newUser(browser, `${unique("owner")}@e2e.test`);
  const memberEmail = `${unique("member")}@e2e.test`;
  const member = await newUser(browser, memberEmail);
  const outsider = await newUser(browser, `${unique("outsider")}@e2e.test`);
  const slug = unique("acme");

  // owner creates an organization via the API the UI uses (no UI for creation yet in this test)
  const created = await owner.page.evaluate(async (s) => {
    const sess = await (await fetch("/api/v1/session")).json();
    const r = await fetch("/api/v1/orgs", {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-CSRF-Token": sess.csrf_token },
      body: JSON.stringify({ name: "Acme Corp", slug: s }),
    });
    return r.status;
  }, slug);
  expect(created).toBe(201);

  // invite via the members page
  await owner.page.goto(`/org/${slug}/members`);
  await owner.page.getByRole("button", { name: "Invite" }).click();
  const dialog = owner.page.getByRole("dialog", { name: "Invite a member" });
  await dialog.getByLabel("Email").fill(memberEmail);
  await dialog.getByRole("button", { name: "Send invitation" }).click();
  const link = await dialog.getByLabel("Invitation link").inputValue();
  expect(link).toContain("/invitations/");
  await dialog.getByRole("button", { name: "Done" }).click();

  // the invited member accepts; an outsider with the same link is refused
  const path = new URL(link).pathname;
  await outsider.page.goto(path);
  await outsider.page.getByRole("button", { name: "Accept invitation" }).click();
  await expect(outsider.page.getByText(/different email address/)).toBeVisible();
  await member.page.goto(path);
  await member.page.getByRole("button", { name: "Accept invitation" }).click();
  await expect(member.page).toHaveURL(new RegExp(`/org/${slug}$`));

  // member (role: member) can create runs but sees no admin-only navigation
  await member.page.goto(`/org/${slug}/runs`);
  await member.page.getByRole("button", { name: "New run" }).click();
  await member.page.getByRole("dialog").getByRole("button", { name: "Start run" }).click();
  await expect(member.page.getByRole("cell", { name: "Smoke test", exact: true })).toBeVisible();
  await expect(member.page.getByRole("link", { name: "Audit log" })).toHaveCount(0);

  // ...and the server enforces it even if the UI is bypassed
  const auditStatus = await member.page.evaluate(
    async (s) => (await fetch(`/api/v1/orgs/${s}/audit`)).status,
    slug,
  );
  expect(auditStatus).toBe(403);

  // the outsider cannot see the tenant at all (404, same as a non-existent org)
  await outsider.page.goto(`/org/${slug}`);
  await expect(outsider.page.getByText("Not found", { exact: true })).toBeVisible();
  const status = await outsider.page.evaluate(
    async (s) => (await fetch(`/api/v1/orgs/${s}/runs`)).status,
    slug,
  );
  expect(status).toBe(404);

  // owner sees the audit trail of all this
  await owner.page.goto(`/org/${slug}/audit`);
  await expect(owner.page.getByRole("cell", { name: "organization.member_added" })).toBeVisible();
  await expect(owner.page.getByRole("cell", { name: "run.created" })).toBeVisible();
  for (const u of [owner, member, outsider]) await u.ctx.close();
});

test("admin area: hidden and denied for normal users, MFA step-up for system admins", async ({ browser }) => {
  const normal = await newUser(browser, `${unique("user")}@e2e.test`);
  await expect(normal.page.getByRole("link", { name: "Users" })).toHaveCount(0);
  await normal.page.goto("/admin/users");
  await expect(normal.page.getByText("restricted to system administrators")).toBeVisible();
  expect(await normal.page.evaluate(async () => (await fetch("/api/v1/admin/users")).status)).toBe(403);

  // system admin without MFA: UI visible, server demands step-up
  const pwdAdmin = await newUser(browser, `${unique("root")}@e2e.test`, { roles: "system_admin" });
  await pwdAdmin.page.goto("/admin/users");
  await expect(pwdAdmin.page.getByRole("button", { name: "Verify it's you" })).toBeVisible();

  // with MFA: full access
  const mfaAdmin = await newUser(browser, `${unique("root")}@e2e.test`, {
    roles: "system_admin",
    amr: "mfa",
  });
  await mfaAdmin.page.goto("/admin/users");
  await expect(mfaAdmin.page.getByRole("heading", { name: "Users" })).toBeVisible();
  await expect(mfaAdmin.page.getByRole("table", { name: "Users" })).toBeVisible();
  await mfaAdmin.page.goto("/admin/system");
  await expect(mfaAdmin.page.getByText("postgres")).toBeVisible();
  for (const u of [normal, pwdAdmin, mfaAdmin]) await u.ctx.close();
});
