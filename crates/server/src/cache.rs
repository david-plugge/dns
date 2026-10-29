//! Caches complete upstream responses for as long as their TTLs allow (RFC 1035, RFC 2308).
//!
//! Concurrent misses for the same question share a single upstream request.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use hickory_proto::{
    op::{Message, ResponseCode},
    rr::{DNSClass, LowerName, RData, Record, RecordType},
};
use moka::{Expiry, future::Cache};

use crate::{
    outcome::AnswerSource,
    upstream::{UpstreamAnswer, UpstreamError},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub name: LowerName,
    pub record_type: RecordType,
    pub class: DNSClass,
    /// Responses to DO queries carry DNSSEC records that other clients must not receive.
    pub dnssec_ok: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct TtlBounds {
    min: Duration,
    max: Duration,
    max_negative: Duration,
}

#[derive(Debug, thiserror::Error)]
#[error(
    "the minimum cache TTL ({min:?}) exceeds the maximum ({max:?}) or the maximum negative \
     TTL ({max_negative:?}); lower DNS_CACHE_MIN_TTL"
)]
pub struct InvalidTtlBounds {
    min: Duration,
    max: Duration,
    max_negative: Duration,
}

impl TtlBounds {
    pub fn new(
        min: Duration,
        max: Duration,
        max_negative: Duration,
    ) -> Result<Self, InvalidTtlBounds> {
        if min > max || min > max_negative {
            return Err(InvalidTtlBounds {
                min,
                max,
                max_negative,
            });
        }
        Ok(Self {
            min,
            max,
            max_negative,
        })
    }
}

/// A response ready to send, and where it came from.
pub struct Resolved {
    pub message: Message,
    pub source: AnswerSource,
}

pub struct DnsCache {
    entries: Cache<CacheKey, Arc<CachedResponse>>,
    ttl_bounds: TtlBounds,
}

struct CachedResponse {
    message: Message,
    stored_at: Instant,
    ttl: Duration,
}

impl DnsCache {
    pub fn new(capacity: u64, ttl_bounds: TtlBounds) -> Self {
        Self {
            entries: Cache::builder()
                .max_capacity(capacity)
                .expire_after(ExpireAfterTtl)
                .build(),
            ttl_bounds,
        }
    }

    /// Returns the cached response for `key`, or awaits `fetch` and caches its result.
    pub async fn get_or_fetch(
        &self,
        key: CacheKey,
        fetch: impl Future<Output = Result<UpstreamAnswer, UpstreamError>>,
    ) -> Result<Resolved, Arc<UpstreamError>> {
        let mut fetched_from = None;
        let entry = self
            .entries
            .entry(key)
            .or_try_insert_with(async {
                let answer = fetch.await?;
                fetched_from = Some(answer.upstream);
                Ok(Arc::new(CachedResponse {
                    ttl: cache_ttl(&answer.message, &self.ttl_bounds),
                    message: answer.message,
                    stored_at: Instant::now(),
                }))
            })
            .await?;

        let cached = entry.value();
        // Only the caller that ran `fetch` reports the upstream; everyone else was
        // served from the cache, even if they waited for the same in-flight request.
        let source = match fetched_from {
            Some(upstream) if entry.is_fresh() => AnswerSource::Upstream(upstream),
            _ => AnswerSource::Cache,
        };
        Ok(Resolved {
            message: with_remaining_ttl(&cached.message, cached.stored_at.elapsed()),
            source,
        })
    }
}

struct ExpireAfterTtl;

impl Expiry<CacheKey, Arc<CachedResponse>> for ExpireAfterTtl {
    fn expire_after_create(
        &self,
        _: &CacheKey,
        value: &Arc<CachedResponse>,
        _: Instant,
    ) -> Option<Duration> {
        Some(value.ttl)
    }
}

/// How long `response` may be cached; zero means not at all.
fn cache_ttl(response: &Message, bounds: &TtlBounds) -> Duration {
    if response.metadata.truncation {
        return Duration::ZERO;
    }
    match response.metadata.response_code {
        ResponseCode::NoError if !response.answers.is_empty() => {
            let ttl = response
                .answers
                .iter()
                .chain(&response.authorities)
                .map(|record| record.ttl)
                .min()
                .unwrap_or(0);
            Duration::from_secs(ttl.into()).clamp(bounds.min, bounds.max)
        }
        // NODATA and NXDOMAIN: RFC 2308 section 5 takes the TTL from the SOA record.
        ResponseCode::NoError | ResponseCode::NXDomain => match negative_ttl(response) {
            Some(ttl) => Duration::from_secs(ttl.into()).clamp(bounds.min, bounds.max_negative),
            // RFC 2308 section 5: negative answers without SOA should not be cached.
            None => Duration::ZERO,
        },
        _ => Duration::ZERO,
    }
}

fn negative_ttl(response: &Message) -> Option<u32> {
    response
        .authorities
        .iter()
        .find_map(|record| match &record.data {
            RData::SOA(soa) => Some(record.ttl.min(soa.minimum)),
            _ => None,
        })
}

fn with_remaining_ttl(message: &Message, age: Duration) -> Message {
    let mut message = message.clone();
    let elapsed = u32::try_from(age.as_secs()).unwrap_or(u32::MAX);
    let sections = [
        &mut message.answers,
        &mut message.authorities,
        &mut message.additionals,
    ];
    sections
        .into_iter()
        .flat_map(|records| records.iter_mut())
        .for_each(|record: &mut Record| {
            record.decrement_ttl(elapsed);
        });
    message
}

#[cfg(test)]
mod tests {
    use std::{net::Ipv4Addr, str::FromStr};

    use hickory_proto::{
        op::Query,
        rr::{Name, rdata::SOA},
    };

    use super::*;

    const BOUNDS: TtlBounds = TtlBounds {
        min: Duration::from_secs(0),
        max: Duration::from_secs(86_400),
        max_negative: Duration::from_secs(3_600),
    };

    fn name(text: &str) -> Name {
        Name::from_str(text).unwrap()
    }

    fn response(code: ResponseCode) -> Message {
        let mut message = Message::query().into_response();
        message.metadata.response_code = code;
        message.add_query(Query::query(name("example.com."), RecordType::A));
        message
    }

    fn a_record(ttl: u32) -> Record {
        Record::from_rdata(
            name("example.com."),
            ttl,
            RData::A(Ipv4Addr::new(192, 0, 2, 1).into()),
        )
    }

    fn soa_record(ttl: u32, minimum: u32) -> Record {
        let soa = SOA::new(
            name("ns.example.com."),
            name("admin.example.com."),
            1,
            7200,
            900,
            1_209_600,
            minimum,
        );
        Record::from_rdata(name("example.com."), ttl, RData::SOA(soa))
    }

    fn key() -> CacheKey {
        CacheKey {
            name: LowerName::from(name("example.com.")),
            record_type: RecordType::A,
            class: DNSClass::IN,
            dnssec_ok: false,
        }
    }

    fn answer(message: Message) -> Result<UpstreamAnswer, UpstreamError> {
        Ok(UpstreamAnswer {
            message,
            upstream: Arc::from("udp://192.0.2.53:53"),
        })
    }

    #[test]
    fn positive_answers_live_as_long_as_their_shortest_ttl() {
        let mut message = response(ResponseCode::NoError);
        message.add_answer(a_record(300));
        message.add_answer(a_record(60));

        assert_eq!(cache_ttl(&message, &BOUNDS), Duration::from_secs(60));
    }

    #[test]
    fn ttls_are_clamped_to_the_bounds() {
        let mut message = response(ResponseCode::NoError);
        message.add_answer(a_record(10));
        let bounds =
            TtlBounds::new(Duration::from_secs(30), BOUNDS.max, BOUNDS.max_negative).unwrap();

        assert_eq!(cache_ttl(&message, &bounds), Duration::from_secs(30));
    }

    #[test]
    fn a_minimum_above_a_maximum_is_rejected() {
        let second = Duration::from_secs(1);

        assert!(TtlBounds::new(second * 2, second, second * 2).is_err());
        assert!(TtlBounds::new(second * 2, second * 2, second).is_err());
        assert!(TtlBounds::new(second, second, second).is_ok());
    }

    #[test]
    fn negative_answers_use_the_soa_minimum_capped_by_max_negative() {
        let mut nxdomain = response(ResponseCode::NXDomain);
        nxdomain.add_authority(soa_record(900, 120));
        assert_eq!(cache_ttl(&nxdomain, &BOUNDS), Duration::from_secs(120));

        let mut nodata = response(ResponseCode::NoError);
        nodata.add_authority(soa_record(86_400, 86_400));
        assert_eq!(cache_ttl(&nodata, &BOUNDS), BOUNDS.max_negative);
    }

    #[test]
    fn errors_truncated_and_soa_less_negative_answers_are_not_cached() {
        let servfail = response(ResponseCode::ServFail);
        let nxdomain_without_soa = response(ResponseCode::NXDomain);
        let mut truncated = response(ResponseCode::NoError);
        truncated.add_answer(a_record(300));
        truncated.metadata.truncation = true;

        for message in [servfail, nxdomain_without_soa, truncated] {
            assert_eq!(cache_ttl(&message, &BOUNDS), Duration::ZERO);
        }
    }

    #[tokio::test]
    async fn second_lookup_is_served_from_cache() {
        let cache = DnsCache::new(100, BOUNDS);
        let mut message = response(ResponseCode::NoError);
        message.add_answer(a_record(300));

        let first = cache
            .get_or_fetch(key(), async { answer(message) })
            .await
            .unwrap();
        let second = cache
            .get_or_fetch(key(), async { panic!("must not fetch again") })
            .await
            .unwrap();

        assert!(matches!(first.source, AnswerSource::Upstream(_)));
        assert_eq!(second.source, AnswerSource::Cache);
        assert_eq!(second.message.answers[0].ttl, 300);
    }

    #[tokio::test]
    async fn uncacheable_responses_are_fetched_every_time() {
        let cache = DnsCache::new(100, BOUNDS);

        for _ in 0..2 {
            let resolved = cache
                .get_or_fetch(key(), async { answer(response(ResponseCode::ServFail)) })
                .await
                .unwrap();
            assert!(matches!(resolved.source, AnswerSource::Upstream(_)));
        }
    }

    #[test]
    fn served_ttls_count_down_with_age() {
        let mut message = response(ResponseCode::NoError);
        message.add_answer(a_record(300));

        let aged = with_remaining_ttl(&message, Duration::from_secs(100));
        assert_eq!(aged.answers[0].ttl, 200);
        let expired = with_remaining_ttl(&message, Duration::from_secs(400));
        assert_eq!(expired.answers[0].ttl, 0);
    }
}
