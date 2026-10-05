import { defineConfig, devices } from "@playwright/test";

// Product journey against a RUNNING stack (`./dev up`), not a test-managed one: what a person
// does on day one. Screenshots of every screen go to test-results/journey/.
//   ./dev test --journey            (JOURNEY_URL defaults to http://localhost:5190)
export default defineConfig({
  testDir: "tests/journey",
  timeout: 90_000,
  expect: { timeout: 15_000 },
  workers: 1,
  reporter: [["list"]],
  use: { baseURL: process.env.JOURNEY_URL ?? "http://localhost:5190", viewport: { width: 1360, height: 860 } },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"], viewport: { width: 1360, height: 860 } } }],
});
