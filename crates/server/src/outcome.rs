//! How a query was handled, as recorded in the query log and the metrics.

use std::{borrow::Cow, sync::Arc};

use hickory_proto::op::ResponseCode;

use crate::upstream::UpstreamError;

#[derive(Debug, Clone)]
pub enum QueryOutcome {
    /// The client received an answer from the cache or an upstream.
    Answered {
        response_code: ResponseCode,
        source: AnswerSource,
    },
    /// No upstream answered; the client received SERVFAIL.
    UpstreamFailed(Arc<UpstreamError>),
    /// A blocklist contains the domain; the client received a null address or no records.
    Blocked { list: Arc<str> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerSource {
    Cache,
    Upstream(Arc<str>),
}

/// How a query was handled, without the details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryStatus {
    Cache,
    Upstream,
    UpstreamFailed,
    Blocked,
}

impl QueryOutcome {
    /// The response code the client received.
    pub fn response_code(&self) -> ResponseCode {
        match self {
            QueryOutcome::Answered { response_code, .. } => *response_code,
            QueryOutcome::UpstreamFailed(_) => ResponseCode::ServFail,
            QueryOutcome::Blocked { .. } => ResponseCode::NoError,
        }
    }

    pub fn status(&self) -> QueryStatus {
        match self {
            QueryOutcome::Answered {
                source: AnswerSource::Cache,
                ..
            } => QueryStatus::Cache,
            QueryOutcome::Answered {
                source: AnswerSource::Upstream(_),
                ..
            } => QueryStatus::Upstream,
            QueryOutcome::UpstreamFailed(_) => QueryStatus::UpstreamFailed,
            QueryOutcome::Blocked { .. } => QueryStatus::Blocked,
        }
    }
}

impl QueryStatus {
    pub const ALL: [QueryStatus; 4] = [
        Self::Cache,
        Self::Upstream,
        Self::UpstreamFailed,
        Self::Blocked,
    ];

    /// Stable name, stored in the query log and used as metrics label.
    pub fn name(self) -> &'static str {
        match self {
            QueryStatus::Cache => "cache",
            QueryStatus::Upstream => "upstream",
            QueryStatus::UpstreamFailed => "upstream_failed",
            QueryStatus::Blocked => "blocked",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.name() == name)
    }
}

/// The mnemonic used by RFC 1035 and tools like dig, e.g. `NXDOMAIN`.
pub fn response_code_name(code: ResponseCode) -> Cow<'static, str> {
    let name = match code {
        ResponseCode::NoError => "NOERROR",
        ResponseCode::FormErr => "FORMERR",
        ResponseCode::ServFail => "SERVFAIL",
        ResponseCode::NXDomain => "NXDOMAIN",
        ResponseCode::NotImp => "NOTIMP",
        ResponseCode::Refused => "REFUSED",
        other => return Cow::Owned(format!("RCODE{}", u16::from(other))),
    };
    Cow::Borrowed(name)
}
