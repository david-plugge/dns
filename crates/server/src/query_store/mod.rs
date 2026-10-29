//! Persistent query log in DuckDB, and the statistics derived from it.
//!
//! Queries are recorded without blocking the DNS path: events go through a bounded channel
//! to a writer thread that appends them in batches and deletes queries past the retention.
//! Reads use a separate connection, so the dashboard never waits for the writer.

mod reader;
mod writer;

use std::{
    net::IpAddr,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use duckdb::{Config, Connection};
use hickory_proto::rr::RecordType;

pub use reader::{QueryFilter, QueryReader};
pub use writer::{QueryRecorder, QueryWriter};

use crate::{error::ErrorCode, outcome::QueryOutcome};

#[derive(Debug, Clone)]
pub struct QueryEvent {
    pub at: SystemTime,
    pub client: IpAddr,
    pub protocol: &'static str,
    pub domain: String,
    pub record_type: RecordType,
    pub latency: Duration,
    pub outcome: QueryOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("query database failed: {0}")]
    Database(#[from] duckdb::Error),
    #[error("query database contains an invalid row: {0}")]
    InvalidRow(String),
}

impl ErrorCode for StoreError {
    fn code(&self) -> &'static str {
        match self {
            StoreError::Database(_) => "query_store_failed",
            StoreError::InvalidRow(_) => "query_store_invalid_row",
        }
    }
}

/// Applied in order; each entry upgrades the schema by one version. Never edit an entry
/// that has been released, append a new one instead.
const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE queries (
        timestamp_ms  BIGINT  NOT NULL,
        client        VARCHAR NOT NULL,
        protocol      VARCHAR NOT NULL,
        domain        VARCHAR NOT NULL,
        record_type   VARCHAR NOT NULL,
        response_code VARCHAR NOT NULL,
        status        VARCHAR NOT NULL,
        upstream      VARCHAR,
        error         VARCHAR,
        latency_us    BIGINT  NOT NULL
    );
",
    "
    ALTER TABLE queries ADD COLUMN blocked_by VARCHAR;
",
    "
    ALTER TABLE queries ADD COLUMN error_code VARCHAR;
    UPDATE queries SET error_code = 'upstreams_failed' WHERE status = 'upstream_failed';
",
];

/// Opens the database at `path`, creating it if needed, and brings its schema up to date.
pub fn open(path: &Path, memory_limit: &str) -> Result<Connection, StoreError> {
    let config = Config::default().max_memory(memory_limit)?;
    let connection = Connection::open_with_flags(path, config)?;
    migrate(&connection)?;
    Ok(connection)
}

fn migrate(connection: &Connection) -> Result<(), StoreError> {
    connection
        .execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")?;
    let current: usize = connection.query_row(
        "SELECT coalesce(max(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    )?;
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(current) {
        let version = index + 1;
        connection.execute_batch(&format!(
            "BEGIN; {migration}; INSERT INTO schema_version VALUES ({version}); COMMIT;"
        ))?;
    }
    Ok(())
}

fn unix_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

#[cfg(test)]
fn open_in_memory() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    migrate(&connection).unwrap();
    connection
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrating_twice_keeps_the_schema() {
        let connection = open_in_memory();
        migrate(&connection).unwrap();

        let version: usize = connection
            .query_row("SELECT max(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, MIGRATIONS.len());
    }
}
