-- Local development bootstrap (runs once, on an empty data directory).
-- Application database `app` is created by POSTGRES_DB. Test databases are created by tests.
-- ZITADEL (identity profile) uses its own database and role, created by ZITADEL itself
-- using the admin credentials passed in its environment.
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
