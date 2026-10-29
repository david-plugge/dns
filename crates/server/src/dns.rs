//! Answers client queries from the blocklist, the cache or the upstreams and records them in
//! the query log.

use std::{
    net::{Ipv4Addr, Ipv6Addr},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use hickory_proto::{
    op::{Edns, Header, HeaderCounts, Message, Metadata, OpCode, Query, ResponseCode},
    rr::{RData, Record, RecordType},
};
use hickory_server::{
    net::runtime::Time,
    server::{Request, RequestHandler, ResponseHandler, ResponseInfo},
    zone_handler::MessageResponseBuilder,
};
use tracing::{debug, warn};

use crate::{
    blocking::Blocker,
    cache::{CacheKey, DnsCache, Resolved},
    local_network::is_private_reverse_name,
    metrics::Metrics,
    outcome::{AnswerSource, QueryOutcome},
    query_store::{QueryEvent, QueryRecorder},
    upstream::{UpstreamError, UpstreamPool},
};

/// UDP payload size recommended by DNS Flag Day 2020 to avoid IP fragmentation.
const MAX_UDP_PAYLOAD: u16 = 1232;
const SUPPORTED_EDNS_VERSION: u8 = 0;

pub struct ForwardingHandler {
    upstreams: Arc<UpstreamPool>,
    /// Answers reverse lookups of private addresses, if configured.
    local_resolver: Option<Arc<UpstreamPool>>,
    cache: DnsCache,
    blocker: Arc<Blocker>,
    blocked_ttl: Duration,
    query_log: QueryRecorder,
    metrics: Arc<Metrics>,
}

impl ForwardingHandler {
    pub fn new(
        upstreams: Arc<UpstreamPool>,
        local_resolver: Option<Arc<UpstreamPool>>,
        cache: DnsCache,
        blocker: Arc<Blocker>,
        blocked_ttl: Duration,
        query_log: QueryRecorder,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            upstreams,
            local_resolver,
            cache,
            blocker,
            blocked_ttl,
            query_log,
            metrics,
        }
    }

    /// Answers from the cache or the upstreams, or with SERVFAIL if no upstream answers.
    async fn forward(
        &self,
        request: &Request,
        domain: &str,
        response_handle: impl ResponseHandler,
    ) -> (ResponseInfo, QueryOutcome) {
        match self.resolve(request, domain).await {
            Ok(resolved) => {
                let outcome = QueryOutcome::Answered {
                    response_code: resolved.message.metadata.response_code,
                    source: resolved.source,
                };
                (
                    relay(request, &resolved.message, response_handle).await,
                    outcome,
                )
            }
            Err(error) => {
                // Upstream health and the query log report this; a warning per query would
                // flood the log while the network is down.
                debug!(%error, query = domain, "forwarding failed");
                (
                    reply_error(request, ResponseCode::ServFail, response_handle).await,
                    QueryOutcome::UpstreamFailed(error),
                )
            }
        }
    }

    async fn resolve(
        &self,
        request: &Request,
        domain: &str,
    ) -> Result<Resolved, Arc<UpstreamError>> {
        let query = upstream_query(
            request
                .queries
                .queries()
                .iter()
                .map(|q| q.original().clone()),
            request.metadata.checking_disabled,
            client_wants_dnssec(request),
        );
        let upstreams = self.upstreams_for(domain);
        // With CD set the client wants unvalidated data, which must neither come from
        // nor end up in the cache shared with validating clients.
        if request.metadata.checking_disabled {
            let answer = upstreams.forward(&query).await?;
            return Ok(Resolved {
                message: answer.message,
                source: AnswerSource::Upstream(answer.upstream),
            });
        }
        let question = &query.queries[0];
        let key = CacheKey {
            name: question.name().into(),
            record_type: question.query_type(),
            class: question.query_class(),
            dnssec_ok: client_wants_dnssec(request),
        };
        self.cache
            .get_or_fetch(key, upstreams.forward(&query))
            .await
    }

    fn upstreams_for(&self, name: &str) -> &UpstreamPool {
        match &self.local_resolver {
            Some(local_resolver) if is_private_reverse_name(name) => local_resolver,
            _ => &self.upstreams,
        }
    }
}

#[async_trait::async_trait]
impl RequestHandler for ForwardingHandler {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        response_handle: R,
    ) -> ResponseInfo {
        let received_at = SystemTime::now();
        let started = Instant::now();

        if request.metadata.op_code != OpCode::Query {
            return reply_error(request, ResponseCode::NotImp, response_handle).await;
        }
        if request
            .edns
            .as_ref()
            .is_some_and(|edns| edns.version() > SUPPORTED_EDNS_VERSION)
        {
            return reply_error(request, ResponseCode::BADVERS, response_handle).await;
        }
        let Ok(info) = request.request_info() else {
            return reply_error(request, ResponseCode::FormErr, response_handle).await;
        };

        // Lowercase, so clients randomizing the case (DNS 0x20) do not split the statistics.
        let domain = info.query.name().to_lowercase().to_string();
        let (response_info, outcome) = match self.blocker.blocked_by(&domain) {
            Some(list) => (
                reply_blocked(request, self.blocked_ttl, response_handle).await,
                QueryOutcome::Blocked { list },
            ),
            None => self.forward(request, &domain, response_handle).await,
        };

        let latency = started.elapsed();
        self.metrics
            .record_query(&outcome, info.protocol.as_str(), latency);
        self.query_log.record(QueryEvent {
            at: received_at,
            // On a dual-stack socket IPv4 clients arrive as `::ffff:a.b.c.d`.
            client: info.src.ip().to_canonical(),
            protocol: info.protocol.as_str(),
            domain,
            record_type: info.query.query_type(),
            latency,
            outcome,
        });
        response_info
    }
}

/// Builds the query sent upstream: a fresh random ID, the client's questions, and our own
/// EDNS options so the client's hop-by-hop options (e.g. cookies) are not leaked.
///
/// RD and AD are always set: the response is cached for all clients, and AD asks the
/// upstream to report whether it validated the answer (RFC 6840 5.7).
fn upstream_query(
    questions: impl IntoIterator<Item = Query>,
    checking_disabled: bool,
    dnssec_ok: bool,
) -> Message {
    let mut query = Message::query();
    query.metadata.recursion_desired = true;
    query.metadata.authentic_data = true;
    query.metadata.checking_disabled = checking_disabled;
    query.add_queries(questions);

    let mut edns = Edns::new();
    edns.set_max_payload(MAX_UDP_PAYLOAD);
    edns.set_dnssec_ok(dnssec_ok);
    query.set_edns(edns);
    query
}

/// The header of the response relayed to the client, taking the flags that describe the
/// answer from the upstream's response.
fn response_metadata(
    request: &Metadata,
    client_wants_dnssec: bool,
    upstream: &Metadata,
) -> Metadata {
    let mut metadata = Metadata::response_from_request(request);
    metadata.authoritative = upstream.authoritative;
    metadata.recursion_available = upstream.recursion_available;
    // RFC 6840 5.8: only clients that signalled DNSSEC awareness get the AD bit.
    metadata.authentic_data =
        upstream.authentic_data && (request.authentic_data || client_wants_dnssec);
    metadata.response_code = upstream.response_code;
    metadata
}

async fn relay(
    request: &Request,
    upstream: &Message,
    mut response_handle: impl ResponseHandler,
) -> ResponseInfo {
    let metadata = response_metadata(
        &request.metadata,
        client_wants_dnssec(request),
        &upstream.metadata,
    );
    let edns = response_edns(request);
    let response = MessageResponseBuilder::new(&request.queries, edns.as_ref()).build(
        metadata,
        upstream.answers.iter(),
        upstream.authorities.iter(),
        std::iter::empty(),
        upstream.additionals.iter(),
    );
    match response_handle.send_response(response).await {
        Ok(info) => info,
        Err(error) => send_failed(request, error),
    }
}

/// A null address for A and AAAA queries, so clients fail fast instead of retrying, and an
/// empty answer for every other type.
async fn reply_blocked(
    request: &Request,
    ttl: Duration,
    mut response_handle: impl ResponseHandler,
) -> ResponseInfo {
    let question = request.queries.queries()[0].original();
    let null_address = match question.query_type() {
        RecordType::A => Some(RData::A(Ipv4Addr::UNSPECIFIED.into())),
        RecordType::AAAA => Some(RData::AAAA(Ipv6Addr::UNSPECIFIED.into())),
        _ => None,
    };
    let ttl = u32::try_from(ttl.as_secs()).unwrap_or(u32::MAX);
    let answers: Vec<Record> = null_address
        .into_iter()
        .map(|data| Record::from_rdata(question.name().clone(), ttl, data))
        .collect();

    let mut metadata = Metadata::response_from_request(&request.metadata);
    metadata.recursion_available = true;
    metadata.response_code = ResponseCode::NoError;
    let edns = response_edns(request);
    let response = MessageResponseBuilder::new(&request.queries, edns.as_ref()).build(
        metadata,
        answers.iter(),
        std::iter::empty(),
        std::iter::empty(),
        std::iter::empty(),
    );
    match response_handle.send_response(response).await {
        Ok(info) => info,
        Err(error) => send_failed(request, error),
    }
}

async fn reply_error(
    request: &Request,
    code: ResponseCode,
    mut response_handle: impl ResponseHandler,
) -> ResponseInfo {
    let edns = response_edns(request);
    let response = MessageResponseBuilder::new(&request.queries, edns.as_ref())
        .error_msg(&request.metadata, code);
    match response_handle.send_response(response).await {
        Ok(info) => info,
        Err(error) => send_failed(request, error),
    }
}

fn send_failed(request: &Request, error: impl std::fmt::Display) -> ResponseInfo {
    warn!(%error, client = %request.src(), "sending response failed");
    let mut metadata = Metadata::response_from_request(&request.metadata);
    metadata.response_code = ResponseCode::ServFail;
    ResponseInfo::from(Header {
        metadata,
        counts: HeaderCounts::default(),
    })
}

/// RFC 6891: a response carries an OPT record only if the query did. The payload size
/// also bounds how large the encoded UDP response may get before it is truncated.
fn response_edns(request: &Request) -> Option<Edns> {
    let request_edns = request.edns.as_ref()?;
    let mut edns = Edns::new();
    edns.set_max_payload(request_edns.max_payload().clamp(512, MAX_UDP_PAYLOAD));
    edns.set_dnssec_ok(request_edns.flags().dnssec_ok);
    Some(edns)
}

fn client_wants_dnssec(request: &Request) -> bool {
    request
        .edns
        .as_ref()
        .is_some_and(|edns| edns.flags().dnssec_ok)
}

#[cfg(test)]
mod tests {
    use hickory_proto::rr::Name;

    use super::*;

    fn question() -> Query {
        Query::query(Name::from_ascii("example.com.").unwrap(), RecordType::A)
    }

    #[test]
    fn upstream_queries_ask_for_recursion_and_validation() {
        let query = upstream_query([question()], true, true);

        assert!(query.metadata.recursion_desired);
        assert!(query.metadata.authentic_data);
        assert!(query.metadata.checking_disabled);
        assert_eq!(query.queries, [question()]);
        let edns = query.edns.as_ref().unwrap();
        assert!(edns.flags().dnssec_ok);
        assert_eq!(edns.max_payload(), MAX_UDP_PAYLOAD);
        assert!(
            !upstream_query([question()], false, false)
                .metadata
                .checking_disabled
        );
    }

    #[test]
    fn responses_keep_the_request_id_and_take_the_upstream_flags() {
        let mut request = Message::query();
        request.metadata.id = 4711;
        let mut upstream = Message::query().into_response();
        upstream.metadata.authoritative = true;
        upstream.metadata.recursion_available = true;
        upstream.metadata.response_code = ResponseCode::NXDomain;

        let metadata = response_metadata(&request.metadata, false, &upstream.metadata);

        assert_eq!(metadata.id, 4711);
        assert!(metadata.authoritative && metadata.recursion_available);
        assert_eq!(metadata.response_code, ResponseCode::NXDomain);
    }

    #[test]
    fn only_dnssec_aware_clients_get_the_ad_bit() {
        let plain_client = Message::query().metadata;
        let mut ad_client = plain_client;
        ad_client.authentic_data = true;
        let mut validated = Message::query().into_response().metadata;
        validated.authentic_data = true;

        assert!(!response_metadata(&plain_client, false, &validated).authentic_data);
        assert!(response_metadata(&plain_client, true, &validated).authentic_data);
        assert!(response_metadata(&ad_client, false, &validated).authentic_data);
        let unvalidated = Message::query().into_response().metadata;
        assert!(!response_metadata(&ad_client, true, &unvalidated).authentic_data);
    }
}
