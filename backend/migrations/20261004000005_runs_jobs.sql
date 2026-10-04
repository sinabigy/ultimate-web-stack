-- Example domain (runs) and the PostgreSQL job queue used by the core profile.

CREATE TABLE runs (
    id              uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    owner_id        uuid REFERENCES users (id) ON DELETE SET NULL,
    label           text NOT NULL,
    provider        text NOT NULL,
    requested       integer NOT NULL CHECK (requested > 0),
    succeeded       integer NOT NULL DEFAULT 0,
    failed          integer NOT NULL DEFAULT 0,
    status          text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'completed', 'failed')),
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    finished_at     timestamptz
);
-- Tenant-scoped listing: WHERE organization_id = $1 ORDER BY created_at DESC LIMIT n
CREATE INDEX runs_org_created_idx ON runs (organization_id, created_at DESC, id DESC);
CREATE TRIGGER runs_updated_at BEFORE UPDATE ON runs FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Durable jobs without extra infrastructure: workers claim with FOR UPDATE SKIP LOCKED.
-- The messaging profile swaps the transport to JetStream behind the same JobQueue trait.
CREATE TABLE jobs (
    id              uuid PRIMARY KEY,
    queue           text NOT NULL,
    kind            text NOT NULL,
    payload         jsonb NOT NULL,
    status          text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'succeeded', 'dead')),
    priority        smallint NOT NULL DEFAULT 0,
    attempts        integer NOT NULL DEFAULT 0,
    max_attempts    integer NOT NULL DEFAULT 5,
    run_at          timestamptz NOT NULL DEFAULT now(),
    locked_until    timestamptz,
    locked_by       text,
    last_error      text,
    -- Producer-supplied key: enqueueing the same logical job twice is a no-op.
    idempotency_key text UNIQUE,
    organization_id uuid REFERENCES organizations (id) ON DELETE CASCADE,
    trace_context   jsonb,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    finished_at     timestamptz
);
CREATE INDEX jobs_ready_idx ON jobs (queue, priority DESC, run_at) WHERE status = 'queued';
CREATE INDEX jobs_running_lease_idx ON jobs (locked_until) WHERE status = 'running';
CREATE INDEX jobs_status_idx ON jobs (status, updated_at DESC);
CREATE TRIGGER jobs_updated_at BEFORE UPDATE ON jobs FOR EACH ROW EXECUTE FUNCTION set_updated_at();
