//! Downloads lists over HTTP(S), resolving their hosts through our own upstreams.
//!
//! The host running this server often uses it as its own DNS server, so relying on the
//! system resolver would make list downloads depend on the server being up already.

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use futures_util::future::join;
use hickory_proto::rr::{Name, RData, RecordType};
use reqwest::{
    Client, StatusCode, Url,
    dns::{Addrs, Name as HostName, Resolve, Resolving},
};

use crate::{
    error::ErrorCode,
    upstream::{UpstreamError, UpstreamPool},
};

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// Large public lists are a few tens of megabytes; anything far beyond is not a blocklist.
const MAX_LIST_SIZE: usize = 128 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("could not set up the HTTP client: {0}")]
    Setup(String),
    #[error("{}", with_causes(.0))]
    Http(#[from] reqwest::Error),
    #[error("server answered with HTTP {0}")]
    Status(StatusCode),
    #[error("list is larger than {} MiB", MAX_LIST_SIZE / 1024 / 1024)]
    TooLarge,
}

impl ErrorCode for DownloadError {
    fn code(&self) -> &'static str {
        match self {
            DownloadError::Setup(_) => "download_setup_failed",
            DownloadError::Http(_) => "download_failed",
            DownloadError::Status(_) => "http_status",
            DownloadError::TooLarge => "list_too_large",
        }
    }
}

pub struct Downloader {
    client: Client,
}

impl Downloader {
    pub fn new(upstreams: Arc<UpstreamPool>) -> Result<Self, DownloadError> {
        let tls_config = hickory_net::tls::client_config()
            .map_err(|error| DownloadError::Setup(error.to_string()))?;
        let client = Client::builder()
            .tls_backend_preconfigured(tls_config)
            .dns_resolver(Arc::new(UpstreamResolver { upstreams }))
            .timeout(DOWNLOAD_TIMEOUT)
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|error| DownloadError::Setup(error.to_string()))?;
        Ok(Self { client })
    }

    pub async fn fetch(&self, url: &Url) -> Result<Vec<u8>, DownloadError> {
        // The URL is the list's name, which callers already report next to the error.
        let mut response = self
            .client
            .get(url.clone())
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        if !response.status().is_success() {
            return Err(DownloadError::Status(response.status()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(reqwest::Error::without_url)?
        {
            if body.len() + chunk.len() > MAX_LIST_SIZE {
                return Err(DownloadError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

/// reqwest's messages omit the cause, e.g. a DNS or TLS failure, which is what users need.
fn with_causes(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut cause = error.source();
    while let Some(current) = cause {
        message += &format!(": {current}");
        cause = current.source();
    }
    message
}

struct UpstreamResolver {
    upstreams: Arc<UpstreamPool>,
}

impl Resolve for UpstreamResolver {
    fn resolve(&self, host: HostName) -> Resolving {
        let upstreams = self.upstreams.clone();
        let host = host.as_str().to_owned();
        Box::pin(async move {
            let mut name = Name::from_ascii(&host)?;
            // Host names in URLs are absolute; upstreams answer with the fully qualified name.
            name.set_fqdn(true);
            let (ipv4, ipv6) = join(
                lookup(&upstreams, &name, RecordType::A),
                lookup(&upstreams, &name, RecordType::AAAA),
            )
            .await;
            if let (Err(error), Err(_)) = (&ipv4, &ipv6) {
                return Err(format!("could not resolve {host}: {error}").into());
            }
            let addresses: Vec<SocketAddr> = ipv4
                .into_iter()
                .chain(ipv6)
                .flatten()
                .map(|ip| SocketAddr::new(ip, 0))
                .collect();
            if addresses.is_empty() {
                return Err(format!("{host} has no IP addresses").into());
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

/// The addresses in the answer; CNAME chains are followed by the upstream.
async fn lookup(
    upstreams: &UpstreamPool,
    name: &Name,
    record_type: RecordType,
) -> Result<Vec<IpAddr>, UpstreamError> {
    let records = upstreams.lookup(name.clone(), record_type).await?;
    let addresses = records
        .into_iter()
        .filter_map(|record| match record {
            RData::A(address) => Some(IpAddr::V4(address.0)),
            RData::AAAA(address) => Some(IpAddr::V6(address.0)),
            _ => None,
        })
        .collect();
    Ok(addresses)
}
