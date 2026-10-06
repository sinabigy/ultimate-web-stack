# API clients

Three small clients for the same task: list recent runs, create a run, and wait for it to finish.
They use an organization **API key**. `./dev test --system` runs all three against the live
stack, with a fresh key that it revokes afterwards, so they are tested and not only documented.

| file | needs |
|---|---|
| [`runs.sh`](runs.sh) | bash, curl, python3 (to read JSON) |
| [`runs.py`](runs.py) | Python 3.11+, standard library only |
| [`runs.mjs`](runs.mjs) | Node.js 22+, no dependencies |

```sh
export API_URL=http://localhost:8080   # the API (`./dev ports` shows DEV_API_PORT)
export API_KEY=...                     # Organization → API keys, scopes runs:read + runs:create
export ORG=my-org                      # the organization's slug
python3 runs.py
```

## What the examples show

- **Authentication:** `Authorization: Bearer <key>`. An API key belongs to one organization, and
  its scopes can only narrow what its creator may do. A present but invalid credential is always a
  401; it never falls back to another method. Backend services can use a service account instead:
  a JWT from the identity provider, registered with the organization
  ([authentication](../../docs/authentication/README.md)).
- **No CSRF token:** bearer requests do not use cookies, so they skip the browser's CSRF check.
- **Errors** are [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457) problem details
  (`application/problem+json`) with `code`, `detail` and, for validation, field `errors`. Every
  response has an `x-request-id`; quote it when you report a problem, because it finds the log
  lines and the trace.
- **Rate limits:** a 429 carries `Retry-After`. Back off for that long.
- **Compression:** send `Accept-Encoding: gzip, br` (curl `--compressed`; Python's `urllib` does
  not decompress, so these examples do not ask for it). Responses from 1 KiB up are compressed.

The examples use the simulated provider that `./dev up` starts. In your product, replace runs with
your own resources: every organization route follows the same pattern.
