-- Append-only audit trail. UPDATE and DELETE are rejected by trigger; retention is handled by
-- dropping whole time partitions or archiving (docs/operations/audit-retention.md), never by
-- editing rows.
CREATE TABLE audit_events (
    id              uuid PRIMARY KEY,
    occurred_at     timestamptz NOT NULL DEFAULT now(),
    actor_type      text NOT NULL CHECK (actor_type IN ('user', 'service', 'api_key', 'system', 'anonymous')),
    actor_id        uuid,
    actor_label     text,
    action          text NOT NULL,
    outcome         text NOT NULL CHECK (outcome IN ('success', 'denied', 'failure')),
    target_type     text,
    target_id       text,
    organization_id uuid,
    request_id      text,
    ip              inet,
    user_agent      text,
    metadata        jsonb NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX audit_org_time_idx ON audit_events (organization_id, occurred_at DESC);
CREATE INDEX audit_actor_time_idx ON audit_events (actor_id, occurred_at DESC);
CREATE INDEX audit_time_idx ON audit_events (occurred_at DESC);
CREATE INDEX audit_action_idx ON audit_events (action, occurred_at DESC);

CREATE FUNCTION audit_events_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  RAISE EXCEPTION 'audit_events is append-only' USING ERRCODE = 'insufficient_privilege';
END $$;
CREATE TRIGGER audit_events_no_update BEFORE UPDATE OR DELETE ON audit_events
    FOR EACH ROW EXECUTE FUNCTION audit_events_immutable();
CREATE TRIGGER audit_events_no_truncate BEFORE TRUNCATE ON audit_events
    FOR EACH STATEMENT EXECUTE FUNCTION audit_events_immutable();

-- API keys: `<prefix>_<env>_<key_id>_<secret>`. Only HMAC-SHA256(pepper, secret) is stored.
CREATE TABLE api_keys (
    id              uuid PRIMARY KEY,
    key_id          text NOT NULL UNIQUE,
    secret_hash     bytea NOT NULL,
    name            text NOT NULL,
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    created_by      uuid REFERENCES users (id) ON DELETE SET NULL,
    scopes          text[] NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    expires_at      timestamptz,
    last_used_at    timestamptz,
    revoked_at      timestamptz,
    rotated_from    uuid REFERENCES api_keys (id) ON DELETE SET NULL
);
CREATE INDEX api_keys_org_idx ON api_keys (organization_id, created_at DESC);

-- Machine-to-machine clients registered at the IdP (OAuth client credentials / JWT profile).
-- They are service principals, never users.
CREATE TABLE service_clients (
    id              uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    -- `sub` (or client_id) of tokens issued to this client by the IdP.
    subject         text NOT NULL UNIQUE,
    name            text NOT NULL,
    scopes          text[] NOT NULL,
    created_by      uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    disabled_at     timestamptz
);

CREATE TABLE notifications (
    id              uuid PRIMARY KEY,
    user_id         uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    organization_id uuid REFERENCES organizations (id) ON DELETE CASCADE,
    kind            text NOT NULL,
    title           text NOT NULL,
    body            text NOT NULL DEFAULT '',
    link            text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    read_at         timestamptz
);
CREATE INDEX notifications_user_idx ON notifications (user_id, created_at DESC);
CREATE INDEX notifications_unread_idx ON notifications (user_id) WHERE read_at IS NULL;
