//! Data types exchanged between the server's HTTP API and the dashboard.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// Aggregated statistics over the most recent time window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub window: StatsWindow,
    pub total_queries: u64,
    pub failed_queries: u64,
    pub cached_queries: u64,
    pub blocked_queries: u64,
    pub average_latency_ms: f64,
    /// Width of each timeline bucket.
    pub bucket_ms: u64,
    /// Oldest first, covering the whole window; the last bucket contains now.
    pub timeline: Vec<TimelineBucket>,
    /// Sorted by count, descending.
    pub clients: Vec<ClientCount>,
    /// Sorted by count, descending.
    pub top_domains: Vec<DomainCount>,
    /// Sorted by count, descending.
    pub top_blocked_domains: Vec<DomainCount>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatsWindow {
    #[default]
    #[serde(rename = "1h")]
    LastHour,
    #[serde(rename = "24h")]
    LastDay,
    #[serde(rename = "7d")]
    LastWeek,
}

impl StatsWindow {
    pub const ALL: [StatsWindow; 3] = [Self::LastHour, Self::LastDay, Self::LastWeek];

    /// The value used in URLs, matching the serde representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LastHour => "1h",
            Self::LastDay => "24h",
            Self::LastWeek => "7d",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineBucket {
    pub queries: u64,
    pub failed: u64,
    pub blocked: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientCount {
    pub client: IpAddr,
    /// The device name the local network knows the client by, if any.
    pub name: Option<String>,
    pub queries: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainCount {
    pub domain: String,
    pub queries: u64,
}

/// One page of the query log, newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryPage {
    pub entries: Vec<QueryLogEntry>,
    /// Number of queries matching the filter, across all pages.
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryLogEntry {
    pub timestamp_ms: u64,
    pub client: IpAddr,
    /// The device name the local network knows the client by, if any.
    pub client_name: Option<String>,
    /// How the query arrived, e.g. `udp` or `tcp`.
    pub protocol: String,
    pub domain: String,
    pub record_type: String,
    /// The response code the client received, e.g. `NXDOMAIN`.
    pub response_code: String,
    pub latency_ms: f64,
    pub outcome: QueryOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QueryOutcome {
    /// The client received an answer from the cache or an upstream.
    Answered { source: AnswerSource },
    /// The upstream could not be reached; the client received SERVFAIL.
    UpstreamFailed { error: ErrorReport },
    /// A blocklist contains the domain; the client received a null address or no records.
    Blocked { list: String },
}

/// How a query was handled, for filtering the query log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryStatus {
    Cache,
    Upstream,
    UpstreamFailed,
    Blocked,
}

impl QueryStatus {
    pub const ALL: [QueryStatus; 4] = [
        Self::Cache,
        Self::Upstream,
        Self::UpstreamFailed,
        Self::Blocked,
    ];

    /// The value used in URLs, matching the serde representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cache => "cache",
            Self::Upstream => "upstream",
            Self::UpstreamFailed => "upstream_failed",
            Self::Blocked => "blocked",
        }
    }
}

impl QueryOutcome {
    pub fn status(&self) -> QueryStatus {
        match self {
            QueryOutcome::Answered {
                source: AnswerSource::Cache,
                ..
            } => QueryStatus::Cache,
            QueryOutcome::Answered {
                source: AnswerSource::Upstream { .. },
                ..
            } => QueryStatus::Upstream,
            QueryOutcome::UpstreamFailed { .. } => QueryStatus::UpstreamFailed,
            QueryOutcome::Blocked { .. } => QueryStatus::Blocked,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnswerSource {
    Cache,
    Upstream { upstream: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpstreamStatus {
    pub upstream: String,
    pub health: UpstreamHealth,
    /// Smoothed round-trip time; `None` until the first successful query.
    pub smoothed_latency_ms: Option<f64>,
    pub queries: u64,
    pub failures: u64,
    pub last_error: Option<ErrorReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamHealth {
    /// Not queried yet.
    Unknown,
    Healthy,
    /// Failed recently, but not often enough in a row to count as down.
    Degraded,
    Down,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlocklistStatus {
    /// The URL, file path or setting the list comes from.
    pub list: String,
    pub kind: ListKind,
    pub domains: u64,
    /// Lines in a syntax that is not supported, e.g. cosmetic filters; they are ignored.
    pub unsupported_lines: u64,
    /// When the list was last downloaded or the file changed; `None` if never loaded.
    pub updated_at_ms: Option<u64>,
    pub last_error: Option<ErrorReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListKind {
    Block,
    Allow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemStatus {
    /// Memory the server process occupies; `None` where the platform does not report it.
    pub resident_memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

/// A failure: a stable code programs can react to, and a message for people. Also the body
/// of every API error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorReport {
    /// Stable snake_case identifier, e.g. `timeout` or `wrong_password`.
    pub code: String,
    pub message: String,
}
