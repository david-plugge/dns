//! Fetches data from the server's HTTP API.

use std::net::IpAddr;

use api::{
    BlocklistStatus, ErrorReport, LoginRequest, QueryPage, QueryStatus, Stats, StatsWindow,
    SystemStatus, UpstreamStatus,
};
use futures_util::future::try_join3;
use gloo_net::http::{Request, Response};
use serde::de::DeserializeOwned;

pub const PAGE_SIZE: usize = 50;
const SESSION_REQUIRED: &str = "session_required";
const WRONG_PASSWORD: &str = "wrong_password";

#[derive(Debug, Clone, PartialEq)]
pub enum FetchError {
    /// The session is missing or expired; the user has to log in.
    SessionRequired,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoginError {
    WrongPassword,
    Failed(String),
}

/// A request that got no successful answer.
struct Failure {
    /// The code of the server's `ErrorReport`, if it sent one.
    code: Option<String>,
    message: String,
}

impl From<Failure> for FetchError {
    fn from(failure: Failure) -> Self {
        if failure.code.as_deref() == Some(SESSION_REQUIRED) {
            return Self::SessionRequired;
        }
        Self::Failed(failure.message)
    }
}

impl From<Failure> for LoginError {
    fn from(failure: Failure) -> Self {
        if failure.code.as_deref() == Some(WRONG_PASSWORD) {
            return Self::WrongPassword;
        }
        Self::Failed(failure.message)
    }
}

/// Succeeds while the browser holds a valid session.
pub async fn check_session() -> Result<(), FetchError> {
    let url = "/api/session";
    send(url, Request::get(url).build()).await?;
    Ok(())
}

pub async fn login(password: String) -> Result<(), LoginError> {
    let url = "/api/login";
    send(url, Request::post(url).json(&LoginRequest { password })).await?;
    Ok(())
}

pub async fn logout() -> Result<(), FetchError> {
    let url = "/api/logout";
    send(url, Request::post(url).build()).await?;
    Ok(())
}

pub async fn fetch_stats(window: StatsWindow) -> Result<Stats, FetchError> {
    get_json(&format!("/api/stats?window={}", window.as_str())).await
}

/// The parts of the server's state that change slowly.
#[derive(Clone, PartialEq)]
pub struct ServerStatus {
    pub upstreams: Vec<UpstreamStatus>,
    pub blocklists: Vec<BlocklistStatus>,
    pub system: SystemStatus,
}

pub async fn fetch_server_status() -> Result<ServerStatus, FetchError> {
    let (upstreams, blocklists, system) = try_join3(
        get_json("/api/upstreams"),
        get_json("/api/blocklists"),
        get_json("/api/system"),
    )
    .await?;
    Ok(ServerStatus {
        upstreams,
        blocklists,
        system,
    })
}

/// Which part of the query log to show.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LogQuery {
    pub client: Option<IpAddr>,
    pub domain: Option<String>,
    pub status: Option<QueryStatus>,
    pub offset: usize,
}

/// The offset of the last page that holds any of `total` queries.
pub fn last_page_offset(total: u64) -> usize {
    let last_index = usize::try_from(total.saturating_sub(1)).unwrap_or(usize::MAX);
    last_index / PAGE_SIZE * PAGE_SIZE
}

pub async fn fetch_queries(query: &LogQuery) -> Result<QueryPage, FetchError> {
    let mut url = format!("/api/queries?limit={PAGE_SIZE}&offset={}", query.offset);
    if let Some(client) = query.client {
        url += &format!("&client={client}");
    }
    if let Some(domain) = &query.domain {
        url += &format!("&domain={}", js_sys::encode_uri_component(domain));
    }
    if let Some(status) = query.status {
        url += &format!("&status={}", status.as_str());
    }
    get_json(&url).await
}

async fn get_json<T: DeserializeOwned>(url: &str) -> Result<T, FetchError> {
    let response = send(url, Request::get(url).build()).await?;
    response
        .json()
        .await
        .map_err(|error| FetchError::Failed(format!("Unexpected response from {url}: {error}")))
}

async fn send(url: &str, request: Result<Request, gloo_net::Error>) -> Result<Response, Failure> {
    let response = match request {
        Ok(request) => request.send().await,
        Err(error) => Err(error),
    };
    let response = response.map_err(|error| Failure {
        code: None,
        message: format!("Server not reachable ({url}): {error}"),
    })?;
    if !response.ok() {
        return Err(rejection(url, response).await);
    }
    Ok(response)
}

async fn rejection(url: &str, response: Response) -> Failure {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    // Requests the server turns down before handling them, e.g. for malformed parameters,
    // come with a plain text body instead of an `ErrorReport`.
    let (code, detail) = match serde_json::from_str::<ErrorReport>(&body) {
        Ok(report) => (Some(report.code), report.message),
        Err(_) => (None, body),
    };
    Failure {
        code,
        message: format!("Server answered {url} with HTTP {status}: {detail}"),
    }
}
