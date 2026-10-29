//! Forwards queries to the configured upstreams, preferring the fastest healthy one
//! and failing over to the others on errors.

mod encrypted;
mod plain;
mod spec;

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use api::{ErrorReport, UpstreamHealth, UpstreamStatus};
use futures_util::future::join_all;
use hickory_proto::{
    ProtoError,
    op::{Message, MessageType, Query},
    rr::{Name, RData, RecordType},
};
use tracing::{info, warn};

use crate::error::ErrorCode;

pub use spec::UpstreamSpec;

use self::{
    encrypted::{EncryptedProtocol, EncryptedTransport},
    plain::PlainTransport,
};

/// After this many failures in a row an upstream counts as down and is tried last.
const DOWN_AFTER_FAILURES: u32 = 3;
/// Weight of the newest sample in the smoothed round-trip time.
const RTT_SMOOTHING: f64 = 0.3;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("no answer within the timeout")]
    Timeout,
    #[error("network error: {0}")]
    Io(#[from] std::io::Error),
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("could not encode the query: {0}")]
    EncodeQuery(ProtoError),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("response does not match the query")]
    MismatchedResponse,
}

impl ErrorCode for TransportError {
    fn code(&self) -> &'static str {
        match self {
            TransportError::Timeout => "timeout",
            TransportError::Io(_) => "network_error",
            TransportError::Connection(_) => "connection_failed",
            TransportError::EncodeQuery(_) => "invalid_query",
            TransportError::InvalidResponse(_) => "invalid_response",
            TransportError::MismatchedResponse => "mismatched_response",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("all upstreams failed: {}", .attempts.iter().map(|(upstream, error)| format!("{upstream}: {error}")).collect::<Vec<_>>().join("; "))]
pub struct UpstreamError {
    pub attempts: Vec<(Arc<str>, TransportError)>,
}

impl ErrorCode for UpstreamError {
    /// The code all attempts share, e.g. `timeout` when the network is down; otherwise the
    /// upstreams failed for different reasons.
    fn code(&self) -> &'static str {
        let mut codes = self.attempts.iter().map(|(_, error)| error.code());
        let first = codes.next();
        match first {
            Some(code) if codes.all(|other| other == code) => code,
            _ => "upstreams_failed",
        }
    }
}

/// A response together with the upstream that sent it.
#[derive(Debug, Clone)]
pub struct UpstreamAnswer {
    pub message: Message,
    pub upstream: Arc<str>,
}

pub struct UpstreamPool {
    upstreams: Vec<Upstream>,
    timeout: Duration,
}

impl UpstreamPool {
    pub fn new(specs: Vec<UpstreamSpec>, timeout: Duration) -> Self {
        let upstreams = specs
            .into_iter()
            .map(|spec| Upstream::new(spec, timeout))
            .collect();
        Self { upstreams, timeout }
    }

    /// Sends `query` (which must have a random ID) to the upstreams in order of preference
    /// until one answers.
    pub async fn forward(&self, query: &Message) -> Result<UpstreamAnswer, UpstreamError> {
        let mut attempts = Vec::new();
        for upstream in self.by_preference() {
            match upstream.exchange(query, self.timeout).await {
                Ok(message) => {
                    return Ok(UpstreamAnswer {
                        message,
                        upstream: upstream.name.clone(),
                    });
                }
                Err(error) => attempts.push((upstream.name.clone(), error)),
            }
        }
        Err(UpstreamError { attempts })
    }

    /// The answer records for `name`; CNAME chains are followed by the upstream.
    pub async fn lookup(
        &self,
        name: Name,
        record_type: RecordType,
    ) -> Result<Vec<RData>, UpstreamError> {
        let answer = self.forward(&recursive_query(name, record_type)).await?;
        Ok(answer
            .message
            .answers
            .into_iter()
            .map(|record| record.data)
            .collect())
    }

    /// Queries every upstream once so the health of idle upstreams stays current.
    pub async fn probe_all(&self) {
        // `. NS` is answered by every recursive resolver and is almost always cached there.
        let probe = recursive_query(Name::root(), RecordType::NS);
        let probes = self
            .upstreams
            .iter()
            .map(|upstream| upstream.exchange(&probe, self.timeout));
        for (upstream, result) in self.upstreams.iter().zip(join_all(probes).await) {
            if let Err(error) = result {
                warn!(upstream = %upstream.name, %error, "upstream probe failed");
            }
        }
    }

    pub fn status(&self) -> Vec<UpstreamStatus> {
        self.upstreams.iter().map(Upstream::status).collect()
    }

    fn by_preference(&self) -> Vec<&Upstream> {
        let mut ordered: Vec<_> = self.upstreams.iter().collect();
        ordered.sort_by_cached_key(|upstream| upstream.preference());
        ordered
    }
}

struct Upstream {
    name: Arc<str>,
    transport: Transport,
    health: Mutex<HealthRecord>,
}

enum Transport {
    Plain(PlainTransport),
    Encrypted(EncryptedTransport),
}

#[derive(Default)]
struct HealthRecord {
    smoothed_rtt: Option<Duration>,
    consecutive_failures: u32,
    total_queries: u64,
    total_failures: u64,
    last_error: Option<ErrorReport>,
}

impl Upstream {
    fn new(spec: UpstreamSpec, timeout: Duration) -> Self {
        let transport = match &spec {
            UpstreamSpec::Plain { address } => Transport::Plain(PlainTransport::new(*address)),
            UpstreamSpec::Tls {
                address,
                server_name,
            } => Transport::Encrypted(EncryptedTransport::new(
                *address,
                server_name.clone(),
                EncryptedProtocol::Tls,
                timeout,
            )),
            UpstreamSpec::Https {
                address,
                server_name,
                path,
            } => Transport::Encrypted(EncryptedTransport::new(
                *address,
                server_name.clone(),
                EncryptedProtocol::Https {
                    path: path.as_str().into(),
                },
                timeout,
            )),
        };
        Self {
            name: spec.to_string().into(),
            transport,
            health: Mutex::new(HealthRecord::default()),
        }
    }

    async fn exchange(
        &self,
        query: &Message,
        timeout: Duration,
    ) -> Result<Message, TransportError> {
        let started = Instant::now();
        let exchange = async {
            match &self.transport {
                Transport::Plain(transport) => transport.exchange(query).await,
                Transport::Encrypted(transport) => transport.exchange(query).await,
            }
        };
        let result = tokio::time::timeout(timeout, exchange)
            .await
            .unwrap_or(Err(TransportError::Timeout));
        self.record(&result, started.elapsed());
        result
    }

    fn record(&self, result: &Result<Message, TransportError>, rtt: Duration) {
        let mut health = self.health.lock().expect("upstream health lock poisoned");
        let was_down = health.consecutive_failures >= DOWN_AFTER_FAILURES;
        health.total_queries += 1;
        match result {
            Ok(_) => {
                health.consecutive_failures = 0;
                health.smoothed_rtt = Some(match health.smoothed_rtt {
                    Some(previous) => {
                        previous.mul_f64(1.0 - RTT_SMOOTHING) + rtt.mul_f64(RTT_SMOOTHING)
                    }
                    None => rtt,
                });
                if was_down {
                    info!(upstream = %self.name, "upstream recovered");
                }
            }
            Err(error) => {
                health.consecutive_failures += 1;
                health.total_failures += 1;
                health.last_error = Some(error.report());
                if health.consecutive_failures == DOWN_AFTER_FAILURES {
                    warn!(upstream = %self.name, %error, "upstream is down");
                }
            }
        }
    }

    /// Sort key: healthy before degraded before down, then fastest first.
    /// Upstreams without measurements yet come first so they get measured.
    fn preference(&self) -> (u8, Duration) {
        let health = self.health.lock().expect("upstream health lock poisoned");
        let rank = match health_of(&health) {
            UpstreamHealth::Unknown | UpstreamHealth::Healthy => 0,
            UpstreamHealth::Degraded => 1,
            UpstreamHealth::Down => 2,
        };
        (rank, health.smoothed_rtt.unwrap_or(Duration::ZERO))
    }

    fn status(&self) -> UpstreamStatus {
        let health = self.health.lock().expect("upstream health lock poisoned");
        UpstreamStatus {
            upstream: self.name.to_string(),
            health: health_of(&health),
            smoothed_latency_ms: health.smoothed_rtt.map(|rtt| rtt.as_secs_f64() * 1000.0),
            queries: health.total_queries,
            failures: health.total_failures,
            last_error: health.last_error.clone(),
        }
    }
}

fn health_of(record: &HealthRecord) -> UpstreamHealth {
    match record.consecutive_failures {
        0 if record.total_queries == 0 => UpstreamHealth::Unknown,
        0 => UpstreamHealth::Healthy,
        n if n < DOWN_AFTER_FAILURES => UpstreamHealth::Degraded,
        _ => UpstreamHealth::Down,
    }
}

/// Checks the ID as well as the question; the ID alone is easy to guess for an off-path attacker.
fn is_response_to(response: &Message, query: &Message) -> bool {
    response.metadata.id == query.metadata.id && answers_question_of(response, query)
}

fn answers_question_of(response: &Message, query: &Message) -> bool {
    response.metadata.message_type == MessageType::Response && response.queries == query.queries
}

/// A query the server sends on its own behalf, with a random ID and RD set.
fn recursive_query(name: Name, record_type: RecordType) -> Message {
    let mut query = Message::query();
    query.metadata.recursion_desired = true;
    query.add_query(Query::query(name, record_type));
    query
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use hickory_proto::rr::Record;
    use tokio::net::UdpSocket;

    use super::*;

    const TIMEOUT: Duration = Duration::from_millis(200);

    /// Answers every query with 192.0.2.1.
    async fn answering_upstream() -> SocketAddr {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buffer = vec![0; 512];
            loop {
                let (length, client) = socket.recv_from(&mut buffer).await.unwrap();
                let mut response = Message::from_vec(&buffer[..length])
                    .unwrap()
                    .into_response();
                let name = response.queries[0].name().clone();
                response.add_answer(Record::from_rdata(
                    name,
                    60,
                    RData::A(Ipv4Addr::new(192, 0, 2, 1).into()),
                ));
                socket
                    .send_to(&response.to_vec().unwrap(), client)
                    .await
                    .unwrap();
            }
        });
        address
    }

    /// Accepts queries but never answers.
    async fn silent_upstream() -> (UdpSocket, SocketAddr) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        (socket, address)
    }

    fn query() -> Message {
        recursive_query(Name::from_ascii("example.com.").unwrap(), RecordType::A)
    }

    #[tokio::test]
    async fn fails_over_and_then_prefers_the_working_upstream() {
        let (_silent, silent_address) = silent_upstream().await;
        let working_address = answering_upstream().await;
        let pool = UpstreamPool::new(
            vec![
                UpstreamSpec::Plain {
                    address: silent_address,
                },
                UpstreamSpec::Plain {
                    address: working_address,
                },
            ],
            TIMEOUT,
        );

        let answer = pool.forward(&query()).await.unwrap();
        assert_eq!(&*answer.upstream, format!("udp://{working_address}"));
        assert_eq!(pool.status()[0].health, UpstreamHealth::Degraded);

        // The failed upstream is now tried last, so the next query is answered without waiting.
        let started = Instant::now();
        pool.forward(&query()).await.unwrap();
        assert!(started.elapsed() < TIMEOUT);
    }

    #[tokio::test]
    async fn lookups_return_the_answer_records() {
        let pool = UpstreamPool::new(
            vec![UpstreamSpec::Plain {
                address: answering_upstream().await,
            }],
            TIMEOUT,
        );

        let records = pool
            .lookup(Name::from_ascii("example.com.").unwrap(), RecordType::A)
            .await
            .unwrap();

        assert_eq!(records, [RData::A(Ipv4Addr::new(192, 0, 2, 1).into())]);
    }

    #[test]
    fn failures_report_the_code_all_attempts_share() {
        let failed = |errors: Vec<TransportError>| UpstreamError {
            attempts: errors
                .into_iter()
                .map(|error| (Arc::from("udp://192.0.2.53:53"), error))
                .collect(),
        };

        assert_eq!(
            failed(vec![TransportError::Timeout, TransportError::Timeout]).code(),
            "timeout"
        );
        assert_eq!(
            failed(vec![
                TransportError::Timeout,
                TransportError::MismatchedResponse
            ])
            .code(),
            "upstreams_failed"
        );
    }

    #[tokio::test]
    async fn reports_every_attempt_when_all_upstreams_fail() {
        let (_silent, silent_address) = silent_upstream().await;
        let pool = UpstreamPool::new(
            vec![UpstreamSpec::Plain {
                address: silent_address,
            }],
            TIMEOUT,
        );

        for _ in 0..DOWN_AFTER_FAILURES {
            let error = pool.forward(&query()).await.unwrap_err();
            assert_eq!(error.attempts.len(), 1);
            assert!(matches!(error.attempts[0].1, TransportError::Timeout));
            assert_eq!(error.code(), "timeout");
        }
        assert_eq!(pool.status()[0].health, UpstreamHealth::Down);
    }
}
