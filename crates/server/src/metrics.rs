//! Prometheus metrics in the OpenMetrics text format.
//!
//! Only aggregates are exported, never client addresses or queried domains, so the endpoint
//! can stay unauthenticated.

use std::time::Duration;

use api::{BlocklistStatus, UpstreamHealth, UpstreamStatus};
use prometheus_client::{
    encoding::{EncodeLabelSet, text},
    metrics::{
        counter::Counter,
        family::Family,
        gauge::Gauge,
        histogram::{Histogram, exponential_buckets},
    },
    registry::Registry,
};

use crate::outcome::QueryOutcome;

pub const CONTENT_TYPE: &str = "application/openmetrics-text; version=1.0.0; charset=utf-8";
const PREFIX: &str = "dns";

/// Counts handled queries; everything else is read from the components when scraped.
pub struct Metrics {
    registry: Registry,
    queries: Family<QueryLabels, Counter>,
    durations: Family<StatusLabels, Histogram>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct QueryLabels {
    status: &'static str,
    protocol: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct StatusLabels {
    status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct UpstreamLabels {
    upstream: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct UpstreamHealthLabels {
    upstream: String,
    health: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, EncodeLabelSet)]
struct ListLabels {
    list: String,
}

/// What the components report at scrape time.
pub struct Snapshot {
    pub upstreams: Vec<UpstreamStatus>,
    pub blocklists: Vec<BlocklistStatus>,
    pub query_log_dropped: u64,
    pub resident_memory_bytes: Option<u64>,
}

impl Metrics {
    pub fn new() -> Self {
        let mut registry = Registry::with_prefix(PREFIX);
        let queries = Family::default();
        registry.register(
            "queries",
            "Answered queries by how they were handled",
            queries.clone(),
        );
        let durations = Family::new_with_constructor(duration_histogram as fn() -> Histogram);
        registry.register(
            "query_duration_seconds",
            "Time from receiving a query to sending the answer",
            durations.clone(),
        );
        Self {
            registry,
            queries,
            durations,
        }
    }

    pub fn record_query(&self, outcome: &QueryOutcome, protocol: &'static str, duration: Duration) {
        let status = outcome.status().name();
        self.queries
            .get_or_create(&QueryLabels { status, protocol })
            .inc();
        self.durations
            .get_or_create(&StatusLabels { status })
            .observe(duration.as_secs_f64());
    }

    pub fn render(&self, snapshot: &Snapshot) -> String {
        let mut output = String::new();
        text::encode_registry(&mut output, &self.registry).expect("writing to a String");
        text::encode_registry(&mut output, &component_registry(snapshot))
            .expect("writing to a String");
        text::encode_eof(&mut output).expect("writing to a String");
        output
    }
}

/// 0.1 ms to 3.3 s: cache hits take microseconds, upstream timeouts seconds.
fn duration_histogram() -> Histogram {
    Histogram::new(exponential_buckets(0.000_1, 2.0, 16))
}

fn component_registry(snapshot: &Snapshot) -> Registry {
    let mut registry = Registry::with_prefix(PREFIX);

    let health = Family::<UpstreamHealthLabels, Gauge>::default();
    let rtt = Family::<UpstreamLabels, Gauge<f64, std::sync::atomic::AtomicU64>>::default();
    let upstream_queries = Family::<UpstreamLabels, Counter>::default();
    let upstream_failures = Family::<UpstreamLabels, Counter>::default();
    for upstream in &snapshot.upstreams {
        let labels = UpstreamLabels {
            upstream: upstream.upstream.clone(),
        };
        // A state set: one series per health state, 1 for the current one.
        for state in [
            UpstreamHealth::Unknown,
            UpstreamHealth::Healthy,
            UpstreamHealth::Degraded,
            UpstreamHealth::Down,
        ] {
            let labels = UpstreamHealthLabels {
                upstream: upstream.upstream.clone(),
                health: health_name(state),
            };
            health
                .get_or_create(&labels)
                .set(i64::from(upstream.health == state));
        }
        if let Some(latency_ms) = upstream.smoothed_latency_ms {
            rtt.get_or_create(&labels).set(latency_ms / 1000.0);
        }
        upstream_queries
            .get_or_create(&labels)
            .inc_by(upstream.queries);
        upstream_failures
            .get_or_create(&labels)
            .inc_by(upstream.failures);
    }
    registry.register("upstream_health", "Current health of each upstream", health);
    registry.register(
        "upstream_rtt_seconds",
        "Smoothed round-trip time of each upstream",
        rtt,
    );
    registry.register(
        "upstream_queries",
        "Queries sent to each upstream",
        upstream_queries,
    );
    registry.register(
        "upstream_failures",
        "Queries to each upstream that failed or timed out",
        upstream_failures,
    );

    let domains = Family::<ListLabels, Gauge>::default();
    let updated = Family::<ListLabels, Gauge>::default();
    let failing = Family::<ListLabels, Gauge>::default();
    for list in &snapshot.blocklists {
        let labels = ListLabels {
            list: list.list.clone(),
        };
        domains
            .get_or_create(&labels)
            .set(i64::try_from(list.domains).unwrap_or(i64::MAX));
        if let Some(updated_at_ms) = list.updated_at_ms {
            updated
                .get_or_create(&labels)
                .set(i64::try_from(updated_at_ms / 1000).unwrap_or(i64::MAX));
        }
        failing
            .get_or_create(&labels)
            .set(i64::from(list.last_error.is_some()));
    }
    registry.register(
        "blocklist_domains",
        "Domains in each block- or allowlist",
        domains,
    );
    registry.register(
        "blocklist_updated_timestamp_seconds",
        "When each list was last downloaded or changed",
        updated,
    );
    registry.register(
        "blocklist_failing",
        "1 if the latest download or read of the list failed",
        failing,
    );

    let dropped = Counter::<u64>::default();
    dropped.inc_by(snapshot.query_log_dropped);
    registry.register(
        "query_log_dropped",
        "Queries not recorded because the query log could not keep up",
        dropped,
    );

    if let Some(bytes) = snapshot.resident_memory_bytes {
        let memory = Gauge::<i64>::default();
        memory.set(i64::try_from(bytes).unwrap_or(i64::MAX));
        registry.register(
            "process_resident_memory_bytes",
            "Memory the server process occupies",
            memory,
        );
    }

    registry
}

fn health_name(health: UpstreamHealth) -> &'static str {
    match health {
        UpstreamHealth::Unknown => "unknown",
        UpstreamHealth::Healthy => "healthy",
        UpstreamHealth::Degraded => "degraded",
        UpstreamHealth::Down => "down",
    }
}

#[cfg(test)]
mod tests {
    use api::{ErrorReport, ListKind};
    use hickory_proto::op::ResponseCode;

    use super::*;
    use crate::outcome::AnswerSource;

    #[test]
    fn renders_query_counts_and_component_state() {
        let metrics = Metrics::new();
        let cached = QueryOutcome::Answered {
            response_code: ResponseCode::NoError,
            source: AnswerSource::Cache,
        };
        metrics.record_query(&cached, "udp", Duration::from_micros(50));
        metrics.record_query(&cached, "udp", Duration::from_micros(70));

        let output = metrics.render(&Snapshot {
            upstreams: vec![UpstreamStatus {
                upstream: "tls://9.9.9.9:853#dns.quad9.net".to_owned(),
                health: UpstreamHealth::Degraded,
                smoothed_latency_ms: Some(20.0),
                queries: 10,
                failures: 2,
                last_error: None,
            }],
            blocklists: vec![BlocklistStatus {
                list: "https://lists.example/ads.txt".to_owned(),
                kind: ListKind::Block,
                domains: 1234,
                unsupported_lines: 0,
                updated_at_ms: Some(1_800_000_000_000),
                last_error: Some(ErrorReport {
                    code: "http_status".to_owned(),
                    message: "server answered with HTTP 404 Not Found".to_owned(),
                }),
            }],
            query_log_dropped: 3,
            resident_memory_bytes: Some(64 * 1024 * 1024),
        });

        for expected in [
            r#"dns_queries_total{status="cache",protocol="udp"} 2"#,
            r#"dns_query_duration_seconds_count{status="cache"} 2"#,
            r#"dns_upstream_health{upstream="tls://9.9.9.9:853#dns.quad9.net",health="degraded"} 1"#,
            r#"dns_upstream_health{upstream="tls://9.9.9.9:853#dns.quad9.net",health="healthy"} 0"#,
            r#"dns_upstream_rtt_seconds{upstream="tls://9.9.9.9:853#dns.quad9.net"} 0.02"#,
            r#"dns_upstream_failures_total{upstream="tls://9.9.9.9:853#dns.quad9.net"} 2"#,
            r#"dns_blocklist_domains{list="https://lists.example/ads.txt"} 1234"#,
            r#"dns_blocklist_updated_timestamp_seconds{list="https://lists.example/ads.txt"} 1800000000"#,
            r#"dns_blocklist_failing{list="https://lists.example/ads.txt"} 1"#,
            "dns_query_log_dropped_total 3",
            "dns_process_resident_memory_bytes 67108864",
        ] {
            assert!(
                output.contains(expected),
                "missing {expected:?} in\n{output}"
            );
        }
        assert!(output.ends_with("# EOF\n"));
    }
}
