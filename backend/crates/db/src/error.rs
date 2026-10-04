#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("not found")]
    NotFound,
    /// Unique/foreign-key/check violation; carries the constraint name when known.
    #[error("conflict on {0}")]
    Conflict(String),
    /// The database could not be reached or the pool is exhausted; the request may be retried.
    #[error("database unavailable")]
    Unavailable(#[source] sqlx::Error),
    #[error(transparent)]
    Other(sqlx::Error),
}

pub type DbResult<T> = Result<T, DbError>;

impl From<sqlx::Error> for DbError {
    fn from(e: sqlx::Error) -> Self {
        match &e {
            sqlx::Error::RowNotFound => DbError::NotFound,
            sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_) | sqlx::Error::Tls(_) => {
                DbError::Unavailable(e)
            }
            sqlx::Error::Database(d) => match d.code().as_deref() {
                // unique_violation, foreign_key_violation, check_violation, exclusion_violation
                Some("23505" | "23503" | "23514" | "23P01") => {
                    DbError::Conflict(d.constraint().unwrap_or("constraint").to_string())
                }
                // statement_timeout / cannot_connect_now / too_many_connections
                Some("57014" | "57P03" | "53300") => DbError::Unavailable(e),
                _ => DbError::Other(e),
            },
            _ => DbError::Other(e),
        }
    }
}
