-- Organisations (tenants), roles, memberships, teams, invitations.

CREATE TABLE organizations (
    id                   uuid PRIMARY KEY,
    slug                 text NOT NULL UNIQUE,
    name                 text NOT NULL,
    personal             boolean NOT NULL DEFAULT false,
    created_by           uuid REFERENCES users (id) ON DELETE SET NULL,
    settings             jsonb NOT NULL DEFAULT '{}'::jsonb,
    -- Billing hooks: the billing provider owns subscriptions; we keep references only.
    billing_plan         text NOT NULL DEFAULT 'free',
    billing_customer_ref text,
    created_at           timestamptz NOT NULL DEFAULT now(),
    updated_at           timestamptz NOT NULL DEFAULT now(),
    deleted_at           timestamptz
);
CREATE TRIGGER organizations_updated_at BEFORE UPDATE ON organizations FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Permission vocabulary. Rows are synchronised from code (app-authz) at startup; code is
-- authoritative, the table exists for referential integrity of custom roles.
CREATE TABLE permissions (
    key         text PRIMARY KEY,
    description text NOT NULL
);

-- Roles: built-in (organization_id IS NULL, permissions defined in code) or custom per org.
CREATE TABLE roles (
    id              uuid PRIMARY KEY,
    organization_id uuid REFERENCES organizations (id) ON DELETE CASCADE,
    key             text NOT NULL,
    name            text NOT NULL,
    description     text NOT NULL DEFAULT '',
    builtin         boolean NOT NULL DEFAULT false,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT roles_scope_key UNIQUE NULLS NOT DISTINCT (organization_id, key),
    CONSTRAINT roles_builtin_global CHECK (builtin = (organization_id IS NULL))
);
CREATE TRIGGER roles_updated_at BEFORE UPDATE ON roles FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE role_permissions (
    role_id    uuid NOT NULL REFERENCES roles (id) ON DELETE CASCADE,
    permission text NOT NULL REFERENCES permissions (key) ON DELETE CASCADE,
    PRIMARY KEY (role_id, permission)
);

-- Built-in role ids are fixed so code and data agree across every deployment.
INSERT INTO roles (id, organization_id, key, name, description, builtin) VALUES
    ('00000000-0000-7000-8000-000000000001', NULL, 'owner',   'Owner',   'Full control including deletion and ownership transfer', true),
    ('00000000-0000-7000-8000-000000000002', NULL, 'admin',   'Admin',   'Manage members, roles, settings and all resources', true),
    ('00000000-0000-7000-8000-000000000003', NULL, 'manager', 'Manager', 'Manage resources and invite members', true),
    ('00000000-0000-7000-8000-000000000004', NULL, 'member',  'Member',  'Create and manage own resources', true),
    ('00000000-0000-7000-8000-000000000005', NULL, 'viewer',  'Viewer',  'Read-only access', true);

CREATE TABLE organization_memberships (
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    user_id         uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role_id         uuid NOT NULL REFERENCES roles (id),
    invited_by      uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, user_id)
);
CREATE INDEX memberships_user_idx ON organization_memberships (user_id);
CREATE TRIGGER memberships_updated_at BEFORE UPDATE ON organization_memberships FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- A custom role may only be assigned within its own organisation.
CREATE FUNCTION check_membership_role_scope() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE role_org uuid;
BEGIN
  SELECT organization_id INTO role_org FROM roles WHERE id = NEW.role_id;
  IF role_org IS NOT NULL AND role_org <> NEW.organization_id THEN
    RAISE EXCEPTION 'role % belongs to another organization', NEW.role_id USING ERRCODE = 'check_violation';
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER memberships_role_scope BEFORE INSERT OR UPDATE ON organization_memberships
    FOR EACH ROW EXECUTE FUNCTION check_membership_role_scope();

CREATE TABLE teams (
    id              uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    name            text NOT NULL,
    description     text NOT NULL DEFAULT '',
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (organization_id, name)
);
CREATE TRIGGER teams_updated_at BEFORE UPDATE ON teams FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Team members must be members of the team's organisation (composite FK enforces it).
CREATE TABLE team_members (
    team_id         uuid NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    organization_id uuid NOT NULL,
    user_id         uuid NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (team_id, user_id),
    FOREIGN KEY (organization_id, user_id) REFERENCES organization_memberships (organization_id, user_id) ON DELETE CASCADE
);

CREATE TABLE invitations (
    id              uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    email           text NOT NULL,
    role_id         uuid NOT NULL REFERENCES roles (id),
    -- SHA-256 of the random token embedded in the invitation link.
    token_hash      bytea NOT NULL UNIQUE,
    invited_by      uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    expires_at      timestamptz NOT NULL,
    accepted_at     timestamptz,
    accepted_by     uuid REFERENCES users (id) ON DELETE SET NULL,
    revoked_at      timestamptz
);
CREATE INDEX invitations_org_idx ON invitations (organization_id, created_at DESC);
CREATE UNIQUE INDEX invitations_pending_email_idx ON invitations (organization_id, lower(email))
    WHERE accepted_at IS NULL AND revoked_at IS NULL;
