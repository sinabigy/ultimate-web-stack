#!/usr/bin/env python3
"""Configure a local/self-hosted ZITADEL for this application (idempotent).

Creates (or reuses):
  * project "app" with roles system_admin / system_auditor (system trust level only;
    organisation roles stay in the application database),
  * OIDC web application: authorization code + PKCE, confidential client (client_secret_basic),
    redirect + post-logout URIs for the dev SPA and the backend (infra/dev-ports.env),
    ID token carries email/profile claims and project roles,
  * a service account (client credentials, JWT access tokens) for machine-to-machine tests,
  * development users (password login; passkeys/MFA can be added in the ZITADEL login UI).

Writes .env.zitadel (gitignored) consumed by `./dev up --identity zitadel`.

Usage: python3 scripts/zitadel_bootstrap.py [--issuer URL] [--pat-file var/zitadel/pat.txt]
Development only: users get fixed passwords. For ZITADEL Cloud or production, create the same
objects in the console or with Terraform and inject secrets via your secret manager.
"""
from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

import dev_ports

ROOT = Path(__file__).resolve().parents[1]


class Zitadel:
    def __init__(self, issuer: str, pat: str):
        self.base = issuer.rstrip("/")
        self.pat = pat

    def call(self, method: str, path: str, body: dict | None = None, ok404: bool = False):
        req = urllib.request.Request(
            self.base + path,
            method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={"Authorization": f"Bearer {self.pat}", "Content-Type": "application/json", "Accept": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=20) as r:
                raw = r.read()
                return json.loads(raw) if raw else {}
        except urllib.error.HTTPError as e:
            text = e.read().decode(errors="replace")
            if ok404 and e.code == 404:
                return None
            if e.code == 409 or "AlreadyExists" in text or "already exists" in text.lower():
                return {"_exists": True, "_detail": text}
            raise SystemExit(f"ZITADEL API {method} {path} failed: HTTP {e.code}: {text[:400]}")


def wait_ready(issuer: str, seconds: int = 120) -> None:
    url = issuer.rstrip("/") + "/.well-known/openid-configuration"
    deadline = time.time() + seconds
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=5) as r:
                if r.status == 200:
                    return
        except Exception:
            pass
        time.sleep(2)
    raise SystemExit(f"ZITADEL not reachable at {url}")


def ensure_project(z: Zitadel, name: str) -> str:
    found = z.call("POST", "/management/v1/projects/_search",
                   {"queries": [{"nameQuery": {"name": name, "method": "TEXT_QUERY_METHOD_EQUALS"}}]})
    for p in found.get("result", []):
        return p["id"]
    created = z.call("POST", "/management/v1/projects", {"name": name, "projectRoleAssertion": True})
    return created["id"]


def ensure_roles(z: Zitadel, project: str) -> None:
    for key, label in [("system_admin", "System administrator"), ("system_auditor", "System auditor")]:
        z.call("POST", f"/management/v1/projects/{project}/roles", {"roleKey": key, "displayName": label, "group": "system"})


def ensure_oidc_app(z: Zitadel, project: str, name: str, origins: list[str]) -> tuple[str, str | None]:
    found = z.call("POST", f"/management/v1/projects/{project}/apps/_search", {})
    for app in found.get("result", []):
        if app.get("name") == name and "oidcConfig" in app:
            # Secret is only returned at creation; regenerate so .env.zitadel is complete.
            sec = z.call("POST", f"/management/v1/projects/{project}/apps/{app['id']}/oidc_config/_generate_client_secret", {})
            return app["oidcConfig"]["clientId"], sec.get("clientSecret")
    created = z.call("POST", f"/management/v1/projects/{project}/apps/oidc", {
        "name": name,
        "redirectUris": [f"{o}/auth/callback" for o in origins],
        "postLogoutRedirectUris": [f"{o}/login" for o in origins],
        "responseTypes": ["OIDC_RESPONSE_TYPE_CODE"],
        "grantTypes": ["OIDC_GRANT_TYPE_AUTHORIZATION_CODE"],
        "appType": "OIDC_APP_TYPE_WEB",
        "authMethodType": "OIDC_AUTH_METHOD_TYPE_BASIC",
        "devMode": True,
        "accessTokenType": "OIDC_TOKEN_TYPE_BEARER",
        "idTokenRoleAssertion": True,
        "idTokenUserinfoAssertion": True,
        "accessTokenRoleAssertion": False,
    })
    return created["clientId"], created.get("clientSecret")


def ensure_user(z: Zitadel, email: str, given: str, family: str, password: str) -> str:
    found = z.call("POST", "/v2/users", {"queries": [{"emailQuery": {"emailAddress": email}}]})
    for u in found.get("result", []):
        return u["userId"]
    created = z.call("POST", "/v2/users/human", {
        "username": email,
        "profile": {"givenName": given, "familyName": family, "displayName": f"{given} {family}"},
        "email": {"email": email, "isVerified": True},
        "password": {"password": password, "changeRequired": False},
    })
    return created["userId"]


def grant(z: Zitadel, user: str, project: str, roles: list[str]) -> None:
    z.call("POST", f"/management/v1/users/{user}/grants", {"projectId": project, "roleKeys": roles})


def ensure_service_account(z: Zitadel, username: str) -> tuple[str, str, str]:
    found = z.call("POST", "/management/v1/users/_search",
                   {"queries": [{"userNameQuery": {"userName": username, "method": "TEXT_QUERY_METHOD_EQUALS"}}]})
    uid = next((u["id"] for u in found.get("result", [])), None)
    if uid is None:
        uid = z.call("POST", "/management/v1/users/machine",
                     {"userName": username, "name": "App reporting service", "accessTokenType": "ACCESS_TOKEN_TYPE_JWT"})["userId"]
    sec = z.call("PUT", f"/management/v1/users/{uid}/secret", {})
    return uid, sec["clientId"], sec["clientSecret"]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ports = dev_ports.load(ROOT)
    ap.add_argument("--issuer", default=f"http://localhost:{ports['DEV_ZITADEL_PORT']}")
    ap.add_argument("--pat-file", default=str(ROOT / "var/zitadel/pat.txt"))
    ap.add_argument("--origins", default=f"http://localhost:{ports['DEV_WEB_PORT']},http://localhost:{ports['DEV_API_PORT']}")
    ap.add_argument("--out", default=str(ROOT / ".env.zitadel"))
    a = ap.parse_args()
    wait_ready(a.issuer)
    pat = Path(a.pat_file).read_text().strip()
    z = Zitadel(a.issuer, pat)
    project = ensure_project(z, "app")
    ensure_roles(z, project)
    client_id, client_secret = ensure_oidc_app(z, project, "app-web", [o.strip() for o in a.origins.split(",")])
    users = {
        "alice@example.com": ("Alice", "Admin", ["system_admin"]),
        "bob@example.com": ("Bob", "Member", []),
    }
    dev_password = "Password1!Password1!"
    for email, (g, f, roles) in users.items():
        uid = ensure_user(z, email, g, f, dev_password)
        if roles:
            grant(z, uid, project, roles)
    sa_id, sa_client, sa_secret = ensure_service_account(z, "app-reporting")
    lines = [
        "# Generated by scripts/zitadel_bootstrap.py — local development only. Do not commit.",
        "APP__AUTH__PROVIDER=zitadel",
        f"APP__AUTH__ISSUER_URL={a.issuer}",
        f"APP__AUTH__CLIENT_ID={client_id}",
        f"APP__AUTH__CLIENT_SECRET={client_secret or ''}",
        f"APP__AUTH__ZITADEL_API_TOKEN={pat}",
        "APP__AUTH__SYSTEM_ROLES_FROM_IDP=true",
        f"APP__AUTH__SERVICE_AUDIENCES=[{project}]",
        # ZITADEL adds the project id to ID-token audiences; trust exactly that one.
        f"APP__AUTH__ID_TOKEN_TRUSTED_AUDIENCES=[{project}]",
        f"ZITADEL_PROJECT_ID={project}",
        f"ZITADEL_SERVICE_ACCOUNT_ID={sa_id}",
        f"ZITADEL_SERVICE_CLIENT_ID={sa_client}",
        f"ZITADEL_SERVICE_CLIENT_SECRET={sa_secret}",
        f"ZITADEL_DEV_USERS={','.join(users)}",
        f"ZITADEL_DEV_PASSWORD={dev_password}",
    ]
    Path(a.out).write_text("\n".join(lines) + "\n")
    Path(a.out).chmod(0o600)
    print(json.dumps({"project": project, "client_id": client_id, "users": list(users), "service_account": sa_client, "env_file": a.out}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
