// Prepare the E2E stack: build backend binaries and the SPA, and recreate the E2E database.
// Requires PostgreSQL from `./dev up` (port 55432) and a Rust toolchain.
import { execSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { homedir } from "node:os";

const env = { ...process.env, PATH: `${homedir()}/.cargo/bin:${process.env.PATH}` };
const run = (cmd, cwd = ".") => execSync(cmd, { stdio: "inherit", cwd, env });
// Compose project name: COMPOSE_PROJECT_NAME, else the default in infra/docker/compose.yaml.
const composeDefault =
  /name: \$\{COMPOSE_PROJECT_NAME:-([^}]+)\}/.exec(readFileSync("../infra/docker/compose.yaml", "utf8"))?.[1] ?? "app";
const pg = process.env.E2E_PG_CONTAINER ?? `${process.env.COMPOSE_PROJECT_NAME ?? composeDefault}-postgres-1`;

run("cargo build -q -p app-server -p mock-oidc -p fake-upstream", "../backend");
run("npm run -s build");
run(`docker exec ${pg} psql -U app -d app -qc "DROP DATABASE IF EXISTS app_e2e WITH (FORCE)"`);
run(`docker exec ${pg} psql -U app -d app -qc "CREATE DATABASE app_e2e"`);
