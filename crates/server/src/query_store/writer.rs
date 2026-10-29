//! Appends recorded queries in batches on a dedicated thread and enforces the retention.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel},
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime},
};

use duckdb::{Connection, params};
use tracing::{error, info, warn};

use super::{QueryEvent, StoreError, unix_ms};
use crate::{
    error::ErrorCode,
    outcome::{AnswerSource, QueryOutcome, response_code_name},
};

/// Queries buffered for the writer. When it falls this far behind, new queries are dropped
/// instead of slowing down DNS.
const CHANNEL_CAPACITY: usize = 10_000;
const MAX_BATCH: usize = 1_000;
/// How long a query may wait before it is written, and so before the dashboard shows it.
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);
const RETENTION_INTERVAL: Duration = Duration::from_secs(3600);

enum Command {
    Record(QueryEvent),
    Shutdown,
}

/// Records queries from the DNS path; never blocks.
#[derive(Clone)]
pub struct QueryRecorder {
    commands: SyncSender<Command>,
    dropped: Arc<AtomicU64>,
}

impl QueryRecorder {
    /// Queries dropped since the start because the writer could not keep up.
    pub fn dropped_total(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn record(&self, event: QueryEvent) {
        match self.commands.try_send(Command::Record(event)) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// The writer thread; shut it down to write the queries still buffered.
pub struct QueryWriter {
    commands: SyncSender<Command>,
    thread: JoinHandle<()>,
}

impl QueryWriter {
    /// Starts writing to `connection`, deleting queries older than `retention`.
    pub fn spawn(connection: Connection, retention: Duration) -> (QueryWriter, QueryRecorder) {
        let (commands, received) = sync_channel(CHANNEL_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let thread = {
            let dropped = dropped.clone();
            std::thread::Builder::new()
                .name("query-writer".to_owned())
                .spawn(move || run(&connection, &received, &dropped, retention))
                .expect("could not start the query writer thread")
        };
        let recorder = QueryRecorder {
            commands: commands.clone(),
            dropped,
        };
        (QueryWriter { commands, thread }, recorder)
    }

    /// Writes the buffered queries and stops the thread. Blocks until it is done.
    pub fn shutdown(self) {
        // The writer is draining the channel, so this waits at most one batch.
        if self.commands.send(Command::Shutdown).is_ok() && self.thread.join().is_err() {
            error!("query writer panicked during shutdown");
        }
    }
}

fn run(
    connection: &Connection,
    commands: &Receiver<Command>,
    dropped: &AtomicU64,
    retention: Duration,
) {
    let mut batch = Vec::with_capacity(MAX_BATCH);
    let mut next_cleanup = Instant::now();
    let mut reported_dropped = 0;
    loop {
        let stop = fill_batch(commands, &mut batch);
        if let Err(error) = append(connection, &batch) {
            error!(%error, queries = batch.len(), "could not write queries to the query log");
        }
        batch.clear();

        let dropped_total = dropped.load(Ordering::Relaxed);
        if dropped_total > reported_dropped {
            warn!(
                dropped = dropped_total - reported_dropped,
                "query log could not keep up; queries were not recorded"
            );
            reported_dropped = dropped_total;
        }

        if Instant::now() >= next_cleanup {
            next_cleanup = Instant::now() + RETENTION_INTERVAL;
            delete_past_retention(connection, retention);
        }

        if stop {
            return;
        }
    }
}

/// Collects queries until the batch is full or the flush interval has passed. Returns
/// whether the writer has to stop.
fn fill_batch(commands: &Receiver<Command>, batch: &mut Vec<QueryEvent>) -> bool {
    let deadline = Instant::now() + FLUSH_INTERVAL;
    while batch.len() < MAX_BATCH {
        match commands.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Command::Record(event)) => batch.push(event),
            Err(RecvTimeoutError::Timeout) => return false,
            Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => return true,
        }
    }
    false
}

pub(super) fn append(connection: &Connection, events: &[QueryEvent]) -> Result<(), StoreError> {
    if events.is_empty() {
        return Ok(());
    }
    let mut appender = connection.appender("queries")?;
    for event in events {
        let (upstream, error, blocked_by) = match &event.outcome {
            QueryOutcome::Answered {
                source: AnswerSource::Upstream(upstream),
                ..
            } => (Some(&**upstream), None, None),
            QueryOutcome::Answered {
                source: AnswerSource::Cache,
                ..
            } => (None, None, None),
            QueryOutcome::UpstreamFailed(error) => (None, Some(error), None),
            QueryOutcome::Blocked { list } => (None, None, Some(&**list)),
        };
        appender.append_row(params![
            unix_ms(event.at),
            event.client.to_string(),
            event.protocol,
            event.domain,
            event.record_type.to_string(),
            response_code_name(event.outcome.response_code()),
            event.outcome.status().name(),
            upstream,
            error.map(ToString::to_string),
            i64::try_from(event.latency.as_micros()).unwrap_or(i64::MAX),
            blocked_by,
            error.map(|error| error.code()),
        ])?;
    }
    appender.flush()?;
    Ok(())
}

fn delete_past_retention(connection: &Connection, retention: Duration) {
    // A retention reaching back before 1970 keeps everything.
    let Some(cutoff) = SystemTime::now().checked_sub(retention) else {
        return;
    };
    match delete_older_than(connection, cutoff) {
        Ok(0) => {}
        Ok(deleted) => info!(deleted, "deleted queries past the retention"),
        Err(error) => error!(%error, "could not delete queries past the retention"),
    }
}

pub(super) fn delete_older_than(
    connection: &Connection,
    cutoff: SystemTime,
) -> Result<usize, StoreError> {
    let deleted = connection.execute(
        "DELETE FROM queries WHERE timestamp_ms < ?",
        params![unix_ms(cutoff)],
    )?;
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use api::QueryStatus;
    use hickory_proto::{op::ResponseCode, rr::RecordType};

    use super::*;
    use crate::query_store::{QueryFilter, QueryReader, open_in_memory};

    fn event(at: SystemTime) -> QueryEvent {
        QueryEvent {
            at,
            client: IpAddr::from([10, 0, 0, 1]),
            protocol: "udp",
            domain: "example.com.".to_owned(),
            record_type: RecordType::A,
            latency: Duration::from_millis(10),
            outcome: QueryOutcome::Answered {
                response_code: ResponseCode::NoError,
                source: AnswerSource::Cache,
            },
        }
    }

    #[test]
    fn shutdown_writes_buffered_queries() {
        let connection = open_in_memory();
        let reader = QueryReader::new(connection.try_clone().unwrap());
        let (writer, recorder) = QueryWriter::spawn(connection, Duration::from_secs(3600));

        for _ in 0..3 {
            recorder.record(event(SystemTime::now()));
        }
        writer.shutdown();

        let page = reader.queries(&QueryFilter::default(), 10, 0).unwrap();
        assert_eq!(page.total, 3);
    }

    #[test]
    fn retention_deletes_only_older_queries() {
        let connection = open_in_memory();
        let now = SystemTime::now();
        let old = now - Duration::from_secs(8 * 86_400);
        append(&connection, &[event(old), event(now)]).unwrap();

        let deleted = delete_older_than(&connection, now - Duration::from_secs(7 * 86_400));

        assert_eq!(deleted.unwrap(), 1);
        let reader = QueryReader::new(connection);
        let remaining = reader.queries(&QueryFilter::default(), 10, 0).unwrap();
        assert_eq!(remaining.total, 1);
        assert_eq!(remaining.entries[0].outcome.status(), QueryStatus::Cache);
    }
}
