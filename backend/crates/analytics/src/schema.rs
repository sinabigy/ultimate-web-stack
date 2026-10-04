//! ClickHouse schema migrations (forward-only, idempotent, recorded in `schema_migrations`).

use crate::AnalyticsError;

/// (version, statements). Append new versions; never edit an applied one.
pub const MIGRATIONS: &[(u32, &[&str])] = &[(
    1,
    &[
        // Raw events. Monthly partitions make TTL drops and backfills cheap; the sort key serves
        // the dominant query shape (one organisation, one event, a time range).
        "CREATE TABLE IF NOT EXISTS events (
            ts DateTime64(3, 'UTC') CODEC(Delta, ZSTD(1)),
            event LowCardinality(String),
            organization_id UUID,
            user_id Nullable(UUID),
            request_id String CODEC(ZSTD(1)),
            properties String CODEC(ZSTD(3)),
            value Float64
        ) ENGINE = MergeTree
        PARTITION BY toYYYYMM(ts)
        ORDER BY (organization_id, event, ts)
        TTL toDateTime(ts) + INTERVAL 400 DAY DELETE",
        // Daily rollup kept for three years; dashboards read this, not raw events.
        "CREATE TABLE IF NOT EXISTS events_daily (
            day Date,
            organization_id UUID,
            event LowCardinality(String),
            events UInt64,
            value Float64
        ) ENGINE = SummingMergeTree
        PARTITION BY toYYYYMM(day)
        ORDER BY (organization_id, event, day)
        TTL day + INTERVAL 1100 DAY DELETE",
        "CREATE MATERIALIZED VIEW IF NOT EXISTS events_daily_mv TO events_daily AS
            SELECT toDate(ts) AS day, organization_id, event, count() AS events, sum(value) AS value
            FROM events GROUP BY day, organization_id, event",
    ],
)];

/// Apply pending migrations. Safe to run concurrently from several instances: every statement
/// is idempotent (`IF NOT EXISTS`), and the version table is a ReplacingMergeTree.
pub async fn migrate(client: &clickhouse::Client) -> Result<Vec<u32>, AnalyticsError> {
    client
        .query(
            "CREATE TABLE IF NOT EXISTS schema_migrations (version UInt32, applied_at DateTime DEFAULT now()) \
             ENGINE = ReplacingMergeTree ORDER BY version",
        )
        .execute()
        .await?;
    let applied: Vec<u32> = client.query("SELECT DISTINCT version FROM schema_migrations").fetch_all::<u32>().await?;
    let mut ran = Vec::new();
    for (version, statements) in MIGRATIONS {
        if applied.contains(version) {
            continue;
        }
        for sql in *statements {
            client.query(sql).execute().await?;
        }
        client.query("INSERT INTO schema_migrations (version) VALUES (?)").bind(*version).execute().await?;
        ran.push(*version);
    }
    Ok(ran)
}
