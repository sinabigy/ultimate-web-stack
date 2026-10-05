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
            // Protocol: the endpoint answered garbage (a proxy or port forwarder in front of a
            // stopped server), which is an outage, not a bug in the request.
            sqlx::Error::PoolTimedOut
            | sqlx::Error::PoolClosed
            | sqlx::Error::Io(_)
            | sqlx::Error::Tls(_)
            | sqlx::Error::Protocol(_) => DbError::Unavailable(e),
            sqlx::Error::Database(d) => match d.code().as_deref() {
                // unique_violation, foreign_key_violation, check_violation, exclusion_violation
                Some("23505" | "23503" | "23514" | "23P01") => {
                    DbError::Conflict(d.constraint().unwrap_or("constraint").to_string())
                }
                Some(code) if unavailable_code(code) => DbError::Unavailable(e),
                _ => DbError::Other(e),
            },
            _ => DbError::Other(e),
        }
    }
}

/// SQLSTATEs that mean "the database is (temporarily) not serving", so the request may be retried:
/// class 08 (connection exception), statement_timeout, admin/crash shutdown (sent to open
/// connections when the server stops), cannot_connect_now, too_many_connections.
fn unavailable_code(code: &str) -> bool {
    code.starts_with("08") || matches!(code, "57014" | "57P01" | "57P02" | "57P03" | "53300")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outages_are_unavailable_not_internal() {
        for code in ["08006", "08001", "57P01", "57P02", "57P03", "53300", "57014"] {
            assert!(unavailable_code(code), "{code}");
        }
        for code in ["23505", "42P01", "22P02"] {
            assert!(!unavailable_code(code), "{code}");
        }
        assert!(matches!(DbError::from(sqlx::Error::Protocol("bad SSLRequest".into())), DbError::Unavailable(_)));
        assert!(matches!(DbError::from(sqlx::Error::PoolTimedOut), DbError::Unavailable(_)));
        assert!(matches!(DbError::from(sqlx::Error::RowNotFound), DbError::NotFound));
    }
}
