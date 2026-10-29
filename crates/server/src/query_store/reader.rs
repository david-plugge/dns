//! Statistics and query log pages, read on their own connection.

use std::{net::IpAddr, sync::Mutex, time::SystemTime};

use api::{
    AnswerSource, ClientCount, DomainCount, ErrorReport, QueryLogEntry, QueryOutcome, QueryPage,
    Stats, StatsWindow, TimelineBucket,
};
use duckdb::{Connection, Row, params};
use hickory_proto::op::ResponseCode;

use super::{StoreError, unix_ms};
use crate::outcome::{QueryStatus, response_code_name};

const TOP_ENTRIES: usize = 10;
const MINUTE_MS: i64 = 60_000;

/// Narrows the query log; unset fields match everything.
#[derive(Debug, Default)]
pub struct QueryFilter {
    pub client: Option<IpAddr>,
    /// Matches every domain containing this text, case-insensitively.
    pub domain: Option<String>,
    pub status: Option<QueryStatus>,
}

pub struct QueryReader {
    connection: Mutex<Connection>,
}

impl QueryReader {
    pub fn new(connection: Connection) -> Self {
        Self {
            connection: Mutex::new(connection),
        }
    }

    pub fn stats(&self, window: StatsWindow, now: SystemTime) -> Result<Stats, StoreError> {
        let connection = self.connection.lock().expect("query reader lock poisoned");
        let timeline = Timeline::ending_at(window, unix_ms(now));
        let blocked = QueryStatus::Blocked.name();
        // Failed counts every SERVFAIL, including those the upstream sent, e.g. for DNSSEC
        // validation failures.
        let servfail = response_code_name(ResponseCode::ServFail);
        let (total_queries, failed_queries, cached_queries, blocked_queries, average_latency_us) =
            connection.query_row(
                "SELECT count(*),
                        count(*) FILTER (WHERE response_code = $5),
                        count(*) FILTER (WHERE status = $3),
                        count(*) FILTER (WHERE status = $4),
                        coalesce(avg(latency_us), 0)
                 FROM queries WHERE timestamp_ms BETWEEN $1 AND $2",
                params![
                    timeline.first_ms,
                    timeline.last_ms,
                    QueryStatus::Cache.name(),
                    blocked,
                    servfail
                ],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, u64>(2)?,
                        row.get::<_, u64>(3)?,
                        row.get::<_, f64>(4)?,
                    ))
                },
            )?;

        let mut buckets = timeline.empty_buckets();
        let mut statement = connection.prepare(
            "SELECT (timestamp_ms - $1) // $3 AS bucket,
                    count(*),
                    count(*) FILTER (WHERE response_code = $5),
                    count(*) FILTER (WHERE status = $4)
             FROM queries WHERE timestamp_ms BETWEEN $1 AND $2
             GROUP BY bucket",
        )?;
        let mut rows = statement.query(params![
            timeline.first_ms,
            timeline.last_ms,
            timeline.bucket_ms,
            blocked,
            servfail
        ])?;
        while let Some(row) = rows.next()? {
            let index: usize = row.get(0)?;
            let bucket = &mut buckets[index];
            bucket.queries = row.get(1)?;
            bucket.failed = row.get(2)?;
            bucket.blocked = row.get(3)?;
        }

        let clients = top(&connection, "client", &timeline, None)?
            .into_iter()
            .map(|(client, queries)| {
                let client = client.parse().map_err(|_| {
                    StoreError::InvalidRow(format!("client {client:?} is not an IP address"))
                })?;
                Ok(ClientCount {
                    client,
                    name: None,
                    queries,
                })
            })
            .collect::<Result<_, StoreError>>()?;
        let domain_counts = |status| -> Result<Vec<DomainCount>, StoreError> {
            let counts = top(&connection, "domain", &timeline, status)?;
            Ok(counts
                .into_iter()
                .map(|(domain, queries)| DomainCount { domain, queries })
                .collect())
        };
        let top_domains = domain_counts(None)?;
        let top_blocked_domains = domain_counts(Some(blocked))?;

        Ok(Stats {
            window,
            total_queries,
            failed_queries,
            cached_queries,
            blocked_queries,
            average_latency_ms: average_latency_us / 1000.0,
            bucket_ms: timeline.bucket_ms as u64,
            timeline: buckets,
            clients,
            top_domains,
            top_blocked_domains,
        })
    }

    /// Matching queries, newest first, skipping the `offset` newest.
    pub fn queries(
        &self,
        filter: &QueryFilter,
        limit: usize,
        offset: usize,
    ) -> Result<QueryPage, StoreError> {
        let connection = self.connection.lock().expect("query reader lock poisoned");
        let client = filter.client.map(|client| client.to_string());
        let domain = filter.domain.as_ref().map(|domain| domain.to_lowercase());
        let status = filter.status.map(QueryStatus::name);
        const MATCHES_FILTER: &str = "($1 IS NULL OR client = $1)
            AND ($2 IS NULL OR contains(domain, $2))
            AND ($3 IS NULL OR status = $3)";

        let total = connection.query_row(
            &format!("SELECT count(*) FROM queries WHERE {MATCHES_FILTER}"),
            params![client, domain, status],
            |row| row.get(0),
        )?;

        let mut statement = connection.prepare(&format!(
            "SELECT timestamp_ms, client, protocol, domain, record_type, response_code,
                    status, upstream, error, latency_us, blocked_by, error_code
             FROM queries WHERE {MATCHES_FILTER}
             ORDER BY timestamp_ms DESC, rowid DESC
             LIMIT $4 OFFSET $5"
        ))?;
        let mut rows = statement.query(params![client, domain, status, limit, offset])?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next()? {
            entries.push(log_entry(row)?);
        }
        Ok(QueryPage { entries, total })
    }
}

/// The buckets of a stats window: whole buckets, the last one containing now.
struct Timeline {
    first_ms: i64,
    last_ms: i64,
    bucket_ms: i64,
    buckets: usize,
}

impl Timeline {
    fn ending_at(window: StatsWindow, now_ms: i64) -> Self {
        let (bucket_ms, buckets) = match window {
            StatsWindow::LastHour => (MINUTE_MS, 60),
            StatsWindow::LastDay => (15 * MINUTE_MS, 96),
            StatsWindow::LastWeek => (120 * MINUTE_MS, 84),
        };
        let current_bucket = now_ms - now_ms % bucket_ms;
        Self {
            first_ms: current_bucket - (buckets as i64 - 1) * bucket_ms,
            last_ms: now_ms,
            bucket_ms,
            buckets,
        }
    }

    fn empty_buckets(&self) -> Vec<TimelineBucket> {
        vec![
            TimelineBucket {
                queries: 0,
                failed: 0,
                blocked: 0,
            };
            self.buckets
        ]
    }
}

/// The most frequent values of `column` in the time range, among queries with `status` if
/// given, descending by count. Ties are broken by value so the order is stable between polls.
fn top(
    connection: &Connection,
    column: &'static str,
    timeline: &Timeline,
    status: Option<&str>,
) -> Result<Vec<(String, u64)>, StoreError> {
    let mut statement = connection.prepare(&format!(
        "SELECT {column}, count(*) AS queries FROM queries
         WHERE timestamp_ms BETWEEN $1 AND $2 AND ($3 IS NULL OR status = $3)
         GROUP BY {column} ORDER BY queries DESC, {column} LIMIT {TOP_ENTRIES}"
    ))?;
    let counts = statement
        .query_map(
            params![timeline.first_ms, timeline.last_ms, status],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
        .collect::<Result<_, _>>()?;
    Ok(counts)
}

fn log_entry(row: &Row) -> Result<QueryLogEntry, StoreError> {
    let client: String = row.get(1)?;
    let status: String = row.get(6)?;
    let upstream: Option<String> = row.get(7)?;
    let error: Option<String> = row.get(8)?;
    let latency_us: i64 = row.get(9)?;
    let blocked_by: Option<String> = row.get(10)?;
    let error_code: Option<String> = row.get(11)?;

    let outcome = match (
        QueryStatus::from_name(&status),
        upstream,
        error,
        error_code,
        blocked_by,
    ) {
        (Some(QueryStatus::Cache), None, None, None, None) => QueryOutcome::Answered {
            source: AnswerSource::Cache,
        },
        (Some(QueryStatus::Upstream), Some(upstream), None, None, None) => QueryOutcome::Answered {
            source: AnswerSource::Upstream { upstream },
        },
        (Some(QueryStatus::UpstreamFailed), None, Some(message), Some(code), None) => {
            QueryOutcome::UpstreamFailed {
                error: ErrorReport { code, message },
            }
        }
        (Some(QueryStatus::Blocked), None, None, None, Some(list)) => {
            QueryOutcome::Blocked { list }
        }
        _ => {
            return Err(StoreError::InvalidRow(format!(
                "status {status:?} does not match its upstream, error and blocked_by columns"
            )));
        }
    };

    Ok(QueryLogEntry {
        timestamp_ms: row.get(0)?,
        client: client.parse().map_err(|_| {
            StoreError::InvalidRow(format!("client {client:?} is not an IP address"))
        })?,
        client_name: None,
        protocol: row.get(2)?,
        domain: row.get(3)?,
        record_type: row.get(4)?,
        response_code: row.get(5)?,
        latency_ms: latency_us as f64 / 1000.0,
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        sync::Arc,
        time::{Duration, UNIX_EPOCH},
    };

    use hickory_proto::rr::RecordType;

    use super::*;
    use crate::{
        outcome,
        query_store::{QueryEvent, open_in_memory, writer::append},
        upstream::{TransportError, UpstreamError},
    };

    const NOW: u64 = 1_800_000_000; // a minute boundary

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn event(
        at: SystemTime,
        client: [u8; 4],
        domain: &str,
        outcome: outcome::QueryOutcome,
    ) -> QueryEvent {
        QueryEvent {
            at,
            client: IpAddr::from(client),
            protocol: "udp",
            domain: domain.to_owned(),
            record_type: RecordType::A,
            latency: Duration::from_millis(10),
            outcome,
        }
    }

    const UPSTREAM: &str = "udp://192.0.2.53:53";
    const LIST: &str = "https://lists.example/ads.txt";

    fn answered(response_code: ResponseCode) -> outcome::QueryOutcome {
        outcome::QueryOutcome::Answered {
            response_code,
            source: outcome::AnswerSource::Upstream(Arc::from(UPSTREAM)),
        }
    }

    fn cached() -> outcome::QueryOutcome {
        outcome::QueryOutcome::Answered {
            response_code: ResponseCode::NoError,
            source: outcome::AnswerSource::Cache,
        }
    }

    fn failed() -> outcome::QueryOutcome {
        outcome::QueryOutcome::UpstreamFailed(Arc::new(UpstreamError {
            attempts: vec![(Arc::from(UPSTREAM), TransportError::Timeout)],
        }))
    }

    fn blocked() -> outcome::QueryOutcome {
        outcome::QueryOutcome::Blocked {
            list: Arc::from(LIST),
        }
    }

    fn reader_with(events: &[QueryEvent]) -> QueryReader {
        let connection = open_in_memory();
        append(&connection, events).unwrap();
        QueryReader::new(connection)
    }

    #[test]
    fn counts_queries_per_client_and_domain() {
        let reader = reader_with(&[
            event(
                at(NOW - 5),
                [10, 0, 0, 1],
                "a.example.",
                answered(ResponseCode::NoError),
            ),
            event(
                at(NOW - 4),
                [10, 0, 0, 1],
                "b.example.",
                answered(ResponseCode::NXDomain),
            ),
            event(at(NOW - 3), [10, 0, 0, 2], "a.example.", failed()),
            event(at(NOW - 2), [10, 0, 0, 3], "c.example.", cached()),
            event(at(NOW - 1), [10, 0, 0, 1], "ads.example.", blocked()),
        ]);

        let stats = reader.stats(StatsWindow::LastHour, at(NOW)).unwrap();

        assert_eq!(stats.total_queries, 5);
        assert_eq!(stats.failed_queries, 1);
        assert_eq!(stats.cached_queries, 1);
        assert_eq!(stats.blocked_queries, 1);
        assert_eq!(stats.timeline[58].blocked, 1);
        assert_eq!(
            stats.top_blocked_domains,
            [DomainCount {
                domain: "ads.example.".to_owned(),
                queries: 1
            }]
        );
        assert_eq!(
            stats.clients[0],
            ClientCount {
                client: IpAddr::from([10, 0, 0, 1]),
                name: None,
                queries: 3
            }
        );
        assert_eq!(
            stats.top_domains[0],
            DomainCount {
                domain: "a.example.".to_owned(),
                queries: 2
            }
        );
        assert_eq!(stats.average_latency_ms, 10.0);
    }

    #[test]
    fn timeline_covers_the_window_and_places_queries_in_their_bucket() {
        let reader = reader_with(&[
            event(
                at(NOW - 61),
                [10, 0, 0, 1],
                "a.example.",
                answered(ResponseCode::NoError),
            ),
            event(at(NOW), [10, 0, 0, 1], "a.example.", failed()),
        ]);

        let stats = reader.stats(StatsWindow::LastHour, at(NOW)).unwrap();
        let buckets = &stats.timeline;

        assert_eq!((buckets.len(), stats.bucket_ms), (60, 60_000));
        assert_eq!((buckets[59].queries, buckets[59].failed), (1, 1));
        assert_eq!((buckets[57].queries, buckets[57].failed), (1, 0));
    }

    #[test]
    fn longer_windows_include_older_queries_in_wider_buckets() {
        let reader = reader_with(&[
            event(
                at(NOW - 3700),
                [10, 0, 0, 1],
                "old.example.",
                answered(ResponseCode::NoError),
            ),
            event(
                at(NOW),
                [10, 0, 0, 1],
                "new.example.",
                answered(ResponseCode::NoError),
            ),
        ]);

        let hour = reader.stats(StatsWindow::LastHour, at(NOW)).unwrap();
        let day = reader.stats(StatsWindow::LastDay, at(NOW)).unwrap();
        let week = reader.stats(StatsWindow::LastWeek, at(NOW)).unwrap();

        assert_eq!(hour.total_queries, 1);
        assert_eq!((day.total_queries, day.timeline.len()), (2, 96));
        assert_eq!((week.total_queries, week.timeline.len()), (2, 84));
    }

    #[test]
    fn log_pages_are_newest_first_and_filtered() {
        let reader = reader_with(&[
            event(
                at(NOW - 3),
                [10, 0, 0, 1],
                "a.example.",
                answered(ResponseCode::NoError),
            ),
            event(at(NOW - 2), [10, 0, 0, 2], "b.example.", failed()),
            event(at(NOW - 1), [10, 0, 0, 1], "ab.example.", cached()),
        ]);
        let domains = |page: QueryPage| -> Vec<String> {
            page.entries.into_iter().map(|entry| entry.domain).collect()
        };

        let all = reader.queries(&QueryFilter::default(), 2, 0).unwrap();
        assert_eq!(all.total, 3);
        assert_eq!(all.entries[1].response_code, "SERVFAIL");
        assert!(matches!(
            &all.entries[1].outcome,
            QueryOutcome::UpstreamFailed { error } if error.code == "timeout"
        ));
        assert_eq!(domains(all), ["ab.example.", "b.example."]);

        let second_page = reader.queries(&QueryFilter::default(), 2, 2).unwrap();
        assert_eq!(domains(second_page), ["a.example."]);

        let by_client = QueryFilter {
            client: Some(IpAddr::from([10, 0, 0, 1])),
            domain: Some("B.EX".to_owned()),
            ..QueryFilter::default()
        };
        assert_eq!(
            domains(reader.queries(&by_client, 10, 0).unwrap()),
            ["ab.example."]
        );

        let blocked_reader =
            reader_with(&[event(at(NOW), [10, 0, 0, 1], "ads.example.", blocked())]);
        let blocked_page = blocked_reader
            .queries(&QueryFilter::default(), 10, 0)
            .unwrap();
        assert_eq!(blocked_page.entries[0].response_code, "NOERROR");
        assert_eq!(
            blocked_page.entries[0].outcome,
            QueryOutcome::Blocked {
                list: LIST.to_owned()
            }
        );

        let by_status = QueryFilter {
            status: Some(QueryStatus::Cache),
            ..QueryFilter::default()
        };
        let cached_page = reader.queries(&by_status, 10, 0).unwrap();
        assert_eq!(cached_page.total, 1);
        assert_eq!(
            cached_page.entries[0].outcome,
            QueryOutcome::Answered {
                source: AnswerSource::Cache
            }
        );
    }
}
