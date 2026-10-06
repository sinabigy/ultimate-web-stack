#!/usr/bin/env node
// Create a run and wait for it, with an organization API key (Node.js 22+, no dependencies).
//
//   API_URL=http://localhost:8080 API_KEY=... ORG=my-org node runs.mjs
//
// The key needs the scopes `runs:read` and `runs:create`. Exit codes as in runs.py.
const { API_URL, API_KEY, ORG } = process.env;
const api = API_URL.replace(/\/$/, "");

async function call(method, path, body) {
  const res = await fetch(`${api}${path}`, {
    method,
    headers: { authorization: `Bearer ${API_KEY}`, accept: "application/json", "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) {
    // RFC 9457 problem details; quote x-request-id when reporting a problem.
    const p = await res.json().catch(() => ({}));
    console.error(`${method} ${path}: ${res.status} ${p.code}: ${p.detail ?? p.title}`, p.errors ?? "", `(request ${res.headers.get("x-request-id")})`);
    process.exit(1);
  }
  return res.json();
}

const recent = (await call("GET", `/api/v1/orgs/${ORG}/runs?limit=5`)).items;
console.log(`${recent.length} recent run(s)`);
let run = await call("POST", `/api/v1/orgs/${ORG}/runs`, { label: "from runs.mjs", provider: "simulated", requested: 3 });
console.log(`created run ${run.id} (${run.status})`);
const deadline = Date.now() + 60_000;
while (["queued", "running"].includes(run.status) && Date.now() < deadline) {
  await new Promise((r) => setTimeout(r, 1000));
  run = await call("GET", `/api/v1/orgs/${ORG}/runs/${run.id}`);
}
console.log(`run ${run.id}: ${run.status}, ${run.succeeded}/${run.requested} calls succeeded`);
process.exit(run.status === "completed" ? 0 : 2);
