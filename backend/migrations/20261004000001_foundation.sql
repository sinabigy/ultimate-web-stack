-- Foundation: conventions shared by every table.
--   * Primary keys are application-generated UUIDv7 (time-ordered; see app_domain::new_id).
--   * Timestamps are timestamptz, stored in UTC; `updated_at` is maintained by trigger.
--   * Tenant-owned tables carry `organization_id NOT NULL` and every query filters on it.

CREATE OR REPLACE FUNCTION set_updated_at() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
  NEW.updated_at := now();
  RETURN NEW;
END $$;
