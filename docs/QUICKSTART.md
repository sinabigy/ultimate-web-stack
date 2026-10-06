# Quickstart

From zero to a running, secure application in about ten minutes. The first Rust build is the
slow part.

## 1. Prerequisites
- **Rust**: rustup, which installs the pinned toolchain (1.99.0, from `backend/rust-toolchain.toml`).
- **Node ≥ 22**, **Python ≥ 3.11**, **git**.
- **Docker** with compose. On macOS, colima or Docker Desktop works; give the VM **at least 6 GiB**.
- `sqlx-cli` is installed by `./dev setup`. `oha` is needed only for benchmarks.

## 2. Generate your product
From a clone of this repository:

```sh
scripts/create-project ../my-product --name "My Product"
cd ../my-product
```

That is the default: the `core` profile (PostgreSQL only) with `b2b` auth (organizations, admin
console). Other options:

```sh
scripts/create-project ../my-saas  --name "My SaaS"  --profile performance --cedar   # + Redis cache, Cedar policies
scripts/create-project ../my-scale --name "My Scale" --profile distributed            # + NATS/JetStream, ClickHouse
scripts/create-project --help                                                          # every flag
```

Run without flags on a terminal to be asked interactively. The new directory is an independent
git repository and never refers back to the blueprint.

## 3. Run it

```sh
python3 tools/ai-check   # protocol state is valid
./dev setup              # toolchains, dependencies, git hooks
./dev up                 # database (+ selected modules), API, worker, SPA
```

Open the app URL that `./dev up` prints (the project's README names it too). Each generated
project has its own ports, so it runs next to the blueprint and other projects; `./dev ports` lists
them. The development identity provider is a mock: sign in with **any email**. For the admin console, enter `system_admin` in *IdP roles* and choose the method
*Password + TOTP (MFA)*.

## 4. Prove it works

```sh
./dev check            # every validation command: lint, audit, tests, E2E, release image, systemd
./dev test --system    # one live journey through every component (needs ./dev up)
./dev test --journey   # browser acceptance per role, with screenshots (needs ./dev up)
./dev down             # stop everything
```

## 5. Build your first feature
Follow [examples/adding-a-feature.md](examples/adding-a-feature.md). It covers permissions,
database, API, audit, UI and tests, in the order the project's conventions require. If you work
with an AI coding agent, point it at `AI_PROTOCOL.md`; see [AI_AGENT_GUIDE.md](AI_AGENT_GUIDE.md).

## 6. Deploy
Pick a tier in [deployment/README.md](deployment/README.md):
- a single VPS (systemd + Caddy);
- containers (production compose);
- Kubernetes (base manifests; statically validated only).

Before going live, work through [PRODUCTION_CHECKLIST.md](PRODUCTION_CHECKLIST.md).

Stuck? See [TROUBLESHOOTING.md](TROUBLESHOOTING.md).
