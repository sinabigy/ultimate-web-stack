# Troubleshooting

Start with `./dev doctor`. It checks every required tool and the Docker daemon, and lists the
enabled modules.

## Setup and startup
| symptom | cause and fix |
|---|---|
| `docker compose` not found | `./dev` falls back to `docker-compose`; install either one (Docker Desktop, or colima plus the compose plugin) |
| a service container exits with code 137 | out of memory in the Docker VM: give it ≥ 6 GiB (`colima start --memory 6`), or `./dev down` before release builds. ClickHouse is the usual victim |
| PostgreSQL exits with `configuration file "/etc/postgresql/postgresql.conf" contains errors` | the project lives outside the directories your Docker VM shares (colima shares only your home directory by default), so the config bind mount is empty. Keep projects under `$HOME`, or add the path to the VM's mounts |
| PostgreSQL refuses connections with `no pg_hba.conf entry for host …` | the database volume was created by an earlier failed start (for example after the shared-directory problem above) and never finished initialising. Run `./dev down --volumes`, then `./dev up` |
| `cannot start: ports this project needs are in use` | `./dev` checks every port before starting anything and names what holds it (another project, a container, an old `./dev up`). Stop that, or give this project other ports (see [Ports](#ports-development) below). `port is already allocated` from Docker itself means an older `./dev` |
| the first `./dev up` is slow | the first Rust build of a fresh `target/` takes several minutes; later starts are incremental |
| `set DATABASE_URL to use query macros online` | you added or changed a SQL query. Start the database (`./dev up --no-app`), export the dev URL, build, then `./dev db prepare` ([CONVENTIONS](../.ai/knowledge/CONVENTIONS.md)) |
| `invalid configuration: …` at startup | run `cargo run -p app-server -- check-config` in `backend/`; it names the key. Production requires HTTPS URLs, `cookie_secure` and real secrets |
| the disk fills up | `backend/target` grows large: run `cargo clean` occasionally. Each generated project has its own `target/` |

## Sign-in and access
| symptom | cause and fix |
|---|---|
| no admin console | in the mock sign-in, set *IdP roles* to `system_admin` **and** method *Password + TOTP (MFA)*. System administration requires MFA |
| signed out after clicking a link | expected after logout or session revocation; sessions live server-side |
| 404 on another organization | by design: non-members cannot even confirm that an organization exists |
| 403 `csrf_failed` from a script | state-changing requests need the `X-CSRF-Token` from `GET /api/v1/session` |

## Tests and validation
| symptom | cause and fix |
|---|---|
| `./dev check` "hangs" | it takes 6–10 minutes warm, because it builds the release image and runs a live systemd test. `tools/ai-validate` prints its summary at the end |
| one `sqlx::test` fails with `unexpected response from SSLRequest: 0x00` | intermittent on colima's port forwarder under connection bursts. It passes on rerun ([KNOWN_ISSUES](../.ai/knowledge/KNOWN_ISSUES.md)) |
| `rust-test-cache`, `-nats` or `-clickhouse` fail with "connection refused" | run them through `./dev check`, which starts the services they need, or run `./dev up --no-app` first |
| `systemd-live` fails | it needs privileged containers (works on Docker Desktop, colima and GitHub-hosted runners) |
| `./dev test --journey` admin tests fail | the stack must run with the mock IdP (`./dev up`, the default), which enables IdP-asserted system roles in development |

## Ports (development)
Every development port is declared once, in `infra/dev-ports.env`. `./dev`, compose, Vite,
Playwright, the validation commands and the smoke scripts all read it, and `./dev ports` lists the
ports with what currently holds each one. The blueprint uses:

| port | service |
|---|---|
| 5190 / 8080 | SPA (Vite) / API |
| 55432 | PostgreSQL |
| 56379 / 56380 | Redis / Dragonfly (cache module) |
| 54222, 58222 | NATS client, monitoring |
| 58123 | ClickHouse HTTP |
| 59081 / 59090 | mock OIDC / simulated provider |
| 58081 | ZITADEL (`./dev up --identity zitadel`) |
| 53000, 59190, 53100, 53200, 54318 | Grafana, Prometheus, Loki, Tempo, OTLP (`--with observability`) |
| 18080, 59082, 59091, 18299 | E2E test servers and the release smoke test (`./dev check`) |

A generated project gets its own block of consecutive ports (20000–29999, chosen from its name, or
`create-project --port-base N`). Several projects and the blueprint can therefore run at the same
time. To move a project, edit `infra/dev-ports.env`; to move one port for one run, set the variable
(`DEV_PG_PORT=55433 ./dev up`). Browsers keep cookies per host rather than per port, so `./dev up`
names the session cookie after the project, and signing in to one project leaves the others signed in.

Still stuck? Check [`.ai/knowledge/KNOWN_ISSUES.md`](../.ai/knowledge/KNOWN_ISSUES.md), then ask
with `./dev doctor` output and the exact failing command.
