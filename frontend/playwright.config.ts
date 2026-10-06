import { defineConfig, devices } from "@playwright/test";
import { devPort } from "./scripts/dev-ports.mjs";

// Full-stack E2E: real app-server (serving the built SPA, same origin), real PostgreSQL,
// mock OIDC provider, simulated provider. Its ports (DEV_E2E_* in infra/dev-ports.env) differ from
// the `./dev up` ones, so E2E (and therefore `./dev check`) runs while a dev stack is up.
const APP = `http://localhost:${devPort("DEV_E2E_API_PORT")}`;
const IDP = `http://127.0.0.1:${devPort("DEV_E2E_IDP_PORT")}`;
const UPSTREAM_PORT = String(devPort("DEV_E2E_UPSTREAM_PORT"));

const backendEnv = {
  APP__ENVIRONMENT: "test",
  APP__DATABASE__URL: `postgres://app:app-dev-only@localhost:${devPort("DEV_PG_PORT")}/app_e2e`,
  APP__DATABASE__MIGRATE_ON_START: "true",
  APP__HTTP__PORT: String(devPort("DEV_E2E_API_PORT")),
  APP__HTTP__STATIC_DIR: "../frontend/dist",
  APP__RATE_LIMIT__ENABLED: "false",
  APP__AUTH__PROVIDER: "oidc",
  APP__PROVIDERS__DEFINITIONS__SIMULATED__BASE_URL: `http://127.0.0.1:${UPSTREAM_PORT}`,
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
      command: `../backend/target/debug/fake-upstream ${UPSTREAM_PORT}`,
      url: `http://127.0.0.1:${UPSTREAM_PORT}/healthz`,
      reuseExistingServer: false,
      stdout: "ignore",
    },
    {
      command: "../backend/target/debug/mock-oidc",
      url: `${IDP}/.well-known/openid-configuration`,
      env: {
        MOCK_OIDC_PORT: String(devPort("DEV_E2E_IDP_PORT")),
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
