import { defineConfig, devices } from "@playwright/test";

// Full-stack E2E: real app-server (serving the built SPA, same origin), real PostgreSQL,
// mock OIDC provider. Ports are dedicated so a running dev stack is not disturbed.
const APP = "http://localhost:18080";
const IDP = "http://127.0.0.1:59082";

const backendEnv = {
  APP__ENVIRONMENT: "test",
  APP__DATABASE__URL: "postgres://app:app-dev-only@localhost:55432/app_e2e",
  APP__DATABASE__MIGRATE_ON_START: "true",
  APP__HTTP__PORT: "18080",
  APP__HTTP__STATIC_DIR: "../frontend/dist",
  APP__RATE_LIMIT__ENABLED: "false",
  APP__AUTH__PROVIDER: "oidc",
  APP__AUTH__ISSUER_URL: IDP,
  APP__AUTH__CLIENT_ID: "app-web",
  APP__AUTH__PUBLIC_ORIGIN: APP,
  APP__AUTH__REDIRECT_URL: `${APP}/auth/callback`,
  APP__AUTH__POST_LOGOUT_REDIRECT_URL: `${APP}/login`,
  APP__AUTH__SYSTEM_ROLES_FROM_IDP: "true",
  APP__AUTH__REQUIRE_MFA_FOR_SYSTEM_ADMIN: "true",
  // E2E exercises the full feature set whatever profile config/app.toml selects.
  APP__TENANCY__ORGANIZATIONS: "true",
  APP__TENANCY__ALLOW_ORG_CREATION: "true",
  APP__ADMIN__ENABLED: "true",
  APP__AUTH__METHODS__PASSKEY: "true",
  APP__AUTH__METHODS__PASSWORD: "true",
  APP__LOG__FORMAT: "json",
  RUST_LOG: "warn",
};

export default defineConfig({
  testDir: "tests/e2e",
  timeout: 30_000,
  expect: { timeout: 7_000 },
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 1 : 0,
  reporter: [["list"], ["html", { open: "never", outputFolder: "playwright-report" }]],
  use: { baseURL: APP, trace: "retain-on-failure" },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: [
    {
      command: "../backend/target/debug/fake-upstream 59090",
      url: "http://127.0.0.1:59090/healthz",
      reuseExistingServer: false,
      stdout: "ignore",
    },
    {
      command: "../backend/target/debug/mock-oidc",
      url: `${IDP}/.well-known/openid-configuration`,
      env: {
        MOCK_OIDC_PORT: "59082",
        MOCK_OIDC_REDIRECT_URIS: `${APP}/auth/callback`,
        MOCK_OIDC_POST_LOGOUT_URIS: `${APP}/login`,
      },
      reuseExistingServer: false,
      stdout: "ignore",
    },
    {
      command: "../backend/target/debug/app-server",
      cwd: "../backend",
      url: `${APP}/readyz`,
      env: backendEnv,
      reuseExistingServer: false,
      timeout: 60_000,
    },
  ],
});
