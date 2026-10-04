import { readFileSync } from "node:fs";
import { defineConfig, devices } from "@playwright/test";

// E2E against a real self-hosted ZITADEL (./dev up --identity zitadel; scripts/zitadel_bootstrap.py).
// The backend runs with the generated .env.zitadel (confidential OIDC client, project roles).
const APP = "http://localhost:8080";
const zitadelEnv = Object.fromEntries(
  readFileSync("../.env.zitadel", "utf8")
    .split("\n")
    .filter((l) => l && !l.startsWith("#") && l.includes("="))
    .map((l) => [l.slice(0, l.indexOf("=")), l.slice(l.indexOf("=") + 1)]),
);

export default defineConfig({
  testDir: "tests/zitadel",
  timeout: 60_000,
  expect: { timeout: 10_000 },
  workers: 1,
  reporter: [["list"]],
  use: { baseURL: APP, trace: "retain-on-failure" },
  projects: [{ name: "zitadel", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: "../backend/target/debug/app-server",
    cwd: "../backend",
    url: `${APP}/readyz`,
    timeout: 60_000,
    reuseExistingServer: false,
    stdout: "pipe",
    stderr: "pipe",
    env: {
      ...zitadelEnv,
      APP__ENVIRONMENT: "test",
      APP__DATABASE__URL: "postgres://app:app-dev-only@localhost:55432/app_e2e",
      APP__DATABASE__MIGRATE_ON_START: "true",
      APP__HTTP__PORT: "8080",
      APP__HTTP__STATIC_DIR: "../frontend/dist",
      APP__RATE_LIMIT__ENABLED: "false",
      APP__AUTH__PUBLIC_ORIGIN: APP,
      APP__AUTH__REDIRECT_URL: `${APP}/auth/callback`,
      APP__AUTH__POST_LOGOUT_REDIRECT_URL: `${APP}/login`,
      RUST_LOG: "warn",
    },
  },
});
