// Prepare the E2E stack: build backend binaries and the SPA, and recreate the E2E database.
// Requires PostgreSQL from `./dev up` (port 55432) and a Rust toolchain.
import { execSync } from "node:child_process";
import { homedir } from "node:os";

const env = { ...process.env, PATH: `${homedir()}/.cargo/bin:${process.env.PATH}` };
const run = (cmd, cwd = ".") => execSync(cmd, { stdio: "inherit", cwd, env });
const pg = process.env.E2E_PG_CONTAINER ?? "uwsb-postgres-1";

run("cargo build -q -p app-server -p mock-oidc", "../backend");
run("npm run -s build");
run(`docker exec ${pg} psql -U app -d app -qc "DROP DATABASE IF EXISTS app_e2e WITH (FORCE)"`);
run(`docker exec ${pg} psql -U app -d app -qc "CREATE DATABASE app_e2e"`);
