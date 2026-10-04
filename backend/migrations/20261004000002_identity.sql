-- Identity: application-side references to identity-provider accounts, plus sessions.
-- The identity provider (ZITADEL by default) owns credentials, MFA, passkeys, email
-- verification and linked identities. See docs/authentication/data-ownership.md.

CREATE TABLE users (
    id                uuid PRIMARY KEY,
    -- Exact OIDC issuer URL + `sub`: the stable identity key. Email is NOT an identity key.
    identity_provider text NOT NULL,
    external_subject  text NOT NULL,
    -- Cached from ID-token claims at each login; the IdP is the source of truth.
    email             text NOT NULL,
    email_verified    boolean NOT NULL DEFAULT false,
    -- Application-owned profile data.
    display_name      text NOT NULL,
    avatar_url        text,
    preferences       jsonb NOT NULL DEFAULT '{}'::jsonb,
    status            text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended', 'deleted')),
    -- System trust level; independent of organisation roles.
    system_role       text NOT NULL DEFAULT 'none' CHECK (system_role IN ('none', 'system_auditor', 'system_admin')),
    last_login_at     timestamptz,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    deleted_at        timestamptz,
    CONSTRAINT users_identity_key UNIQUE (identity_provider, external_subject)
);
CREATE INDEX users_email_idx ON users (lower(email));
CREATE INDEX users_created_idx ON users (created_at DESC);
CREATE TRIGGER users_updated_at BEFORE UPDATE ON users FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Browser sessions (BFF). The cookie carries a random 256-bit token; only its SHA-256 is stored,
-- so a database leak does not yield usable session tokens.
CREATE TABLE sessions (
    id                   uuid PRIMARY KEY,
    user_id              uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash           bytea NOT NULL UNIQUE,
    -- After rotation the previous token stays valid briefly so concurrent in-flight requests
    -- do not log the user out.
    previous_token_hash  bytea UNIQUE,
    previous_valid_until timestamptz,
    csrf_token           text NOT NULL,
    created_at           timestamptz NOT NULL DEFAULT now(),
    last_seen_at         timestamptz NOT NULL DEFAULT now(),
    rotated_at           timestamptz NOT NULL DEFAULT now(),
    expires_at           timestamptz NOT NULL,   -- absolute lifetime
    idle_expires_at      timestamptz NOT NULL,   -- sliding inactivity limit
    auth_time            timestamptz NOT NULL,   -- when the user authenticated at the IdP
    amr                  text[] NOT NULL DEFAULT '{}',
    mfa                  boolean NOT NULL DEFAULT false,
    ip                   inet,
    user_agent           text,
    -- ID token (AES-256-GCM) kept only for RP-initiated logout (id_token_hint).
    id_token_enc         bytea,
    revoked_at           timestamptz,
    revoked_reason       text
);
CREATE INDEX sessions_user_active_idx ON sessions (user_id, last_seen_at DESC) WHERE revoked_at IS NULL;
CREATE INDEX sessions_expiry_idx ON sessions (expires_at);

-- In-flight OIDC authorization requests (state → nonce, PKCE verifier, return path).
-- Short-lived; deleted on use. Keyed by SHA-256(state).
CREATE TABLE oidc_flows (
    state_hash    bytea PRIMARY KEY,
    nonce         text NOT NULL,
    pkce_verifier text NOT NULL,
    return_to     text NOT NULL,
    intent        text NOT NULL CHECK (intent IN ('login', 'register', 'reauth')),
    created_at    timestamptz NOT NULL DEFAULT now(),
    expires_at    timestamptz NOT NULL
);
CREATE INDEX oidc_flows_expiry_idx ON oidc_flows (expires_at);
