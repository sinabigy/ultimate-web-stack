#!/usr/bin/env bash
# Create a run and wait for it, with an organization API key (curl; python3 only to read JSON).
#   API_URL=http://localhost:8080 API_KEY=... ORG=my-org ./runs.sh
set -euo pipefail
auth=(-H "authorization: Bearer $API_KEY" -H "accept: application/json")
field() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }
# --fail-with-body: an error exits non-zero and still prints the problem details.
curl -sS --fail-with-body "${auth[@]}" "$API_URL/api/v1/orgs/$ORG/runs?limit=5" | field '["items"].__len__()' | sed 's/$/ recent run(s)/'
run=$(curl -sS --fail-with-body "${auth[@]}" -H "content-type: application/json" \
  -d '{"label":"from runs.sh","provider":"simulated","requested":3}' "$API_URL/api/v1/orgs/$ORG/runs")
id=$(field '["id"]' <<<"$run"); status=$(field '["status"]' <<<"$run")
echo "created run $id ($status)"
for _ in $(seq 1 60); do
  case "$status" in queued|running) sleep 1 ;; *) break ;; esac
  run=$(curl -sS --fail-with-body "${auth[@]}" "$API_URL/api/v1/orgs/$ORG/runs/$id"); status=$(field '["status"]' <<<"$run")
done
echo "run $id: $status, $(field '["succeeded"]' <<<"$run")/$(field '["requested"]' <<<"$run") calls succeeded"
[ "$status" = completed ] || exit 2
