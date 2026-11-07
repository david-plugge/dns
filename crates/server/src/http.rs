//! HTTP API for the dashboard, plus the dashboard's static files.

use std::{
    net::IpAddr,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use api::{
    BlocklistStatus, ErrorReport, LoginRequest, ObservedClient, QueryPage, Stats, StatsWindow,
    SystemStatus, UpstreamStatus,
};
use axum::{
    Json, Router,
    extract::{Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use tower_http::services::{ServeDir, ServeFile};
use tracing::error;

use crate::{
    auth::Authenticator,
    blocking::Blocker,
    error::ErrorCode,
    local_network::ClientNames,
    metrics::{self, Metrics},
    outcome::QueryStatus,
    process,
    query_store::{QueryFilter, QueryReader, QueryRecorder, StoreError, time_range},
    upstream::UpstreamPool,
};

const MAX_QUERIES_PER_REQUEST: usize = 1000;
const SESSION_COOKIE: &str = "dns_session";

#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<Authenticator>,
    pub query_reader: Arc<QueryReader>,
    pub client_names: Arc<ClientNames>,
    pub upstreams: Arc<UpstreamPool>,
    pub blocker: Arc<Blocker>,
    pub metrics: Arc<Metrics>,
    pub query_recorder: QueryRecorder,
}

pub fn router(state: AppState, dashboard_dir: &Path) -> Router {
    // Unknown paths fall back to index.html so the SPA can route them itself. The dashboard's
    // files contain no data, so they are public; it asks for the password when the API does.
    let dashboard =
        ServeDir::new(dashboard_dir).fallback(ServeFile::new(dashboard_dir.join("index.html")));

    let protected_api = Router::new()
        .route("/api/session", get(session))
        .route("/api/logout", post(logout))
        .route("/api/stats", get(stats))
        .route("/api/queries", get(queries))
        .route("/api/clients", get(clients))
        .route("/api/upstreams", get(upstream_status))
        .route("/api/blocklists", get(blocklist_status))
        .route("/api/blocklists/refresh", post(refresh_blocklists))
        .route("/api/system", get(system_status))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_session,
        ));

    Router::new()
        .route("/health", get(health))
        .route("/metrics", get(prometheus_metrics))
        .route("/api/login", post(login))
        .merge(protected_api)
        .with_state(state)
        .fallback_service(dashboard)
}

/// An error response: the status plus an [`ErrorReport`] body.
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let report = ErrorReport {
            code: self.code.to_owned(),
            message: self.message.to_owned(),
        };
        (self.status, Json(report)).into_response()
    }
}

const SESSION_REQUIRED: ApiError = ApiError {
    status: StatusCode::UNAUTHORIZED,
    code: "session_required",
    message: "Log in to use the dashboard.",
};

const WRONG_PASSWORD: ApiError = ApiError {
    status: StatusCode::UNAUTHORIZED,
    code: "wrong_password",
    message: "The password is wrong.",
};

async fn require_session(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let is_logged_in =
        session_token(request.headers()).is_some_and(|token| state.auth.is_valid(token));
    if !is_logged_in {
        return SESSION_REQUIRED.into_response();
    }
    next.run(request).await
}

async fn login(State(state): State<AppState>, Json(request): Json<LoginRequest>) -> Response {
    let Some(session) = state.auth.login(&request.password).await else {
        return WRONG_PASSWORD.into_response();
    };
    // Not `Secure`: the dashboard is usually served over plain HTTP in the local network.
    let cookie = format!(
        "{SESSION_COOKIE}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        session.token,
        session.lifetime.as_secs()
    );
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response()
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = session_token(&headers) {
        state.auth.logout(token);
    }
    let expired = format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0");
    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, expired)]).into_response()
}

/// Reaching this handler means the session is valid.
async fn session() -> StatusCode {
    StatusCode::NO_CONTENT
}

fn session_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .find_map(|cookie| {
            cookie
                .trim()
                .strip_prefix(SESSION_COOKIE)?
                .strip_prefix('=')
        })
}

async fn health() -> &'static str {
    "ok"
}

async fn prometheus_metrics(State(state): State<AppState>) -> Response {
    let snapshot = metrics::Snapshot {
        upstreams: state.upstreams.status(),
        blocklists: state.blocker.status(),
        query_log_dropped: state.query_recorder.dropped_total(),
        resident_memory_bytes: process::resident_memory_bytes(),
    };
    (
        [(header::CONTENT_TYPE, metrics::CONTENT_TYPE)],
        state.metrics.render(&snapshot),
    )
        .into_response()
}

/// Status filter of the query log, as named in the API.
fn query_status(status: api::QueryStatus) -> QueryStatus {
    match status {
        api::QueryStatus::Cache => QueryStatus::Cache,
        api::QueryStatus::Upstream => QueryStatus::Upstream,
        api::QueryStatus::UpstreamFailed => QueryStatus::UpstreamFailed,
        api::QueryStatus::Blocked => QueryStatus::Blocked,
    }
}

#[derive(Deserialize)]
struct StatsParams {
    #[serde(default)]
    window: StatsWindow,
    until_ms: Option<u64>,
}

fn requested_until(until_ms: Option<u64>) -> Result<SystemTime, ApiError> {
    let now = SystemTime::now();
    let Some(until_ms) = until_ms else {
        return Ok(now);
    };
    let until = UNIX_EPOCH.checked_add(Duration::from_millis(until_ms));
    match until {
        Some(until) if until <= now && until_ms <= i64::MAX as u64 => Ok(until),
        _ => Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_time_range",
            message: "The end of the query range must be a valid time in the past. Reset the filters to return to live data.",
        }),
    }
}

async fn stats(
    State(state): State<AppState>,
    Query(params): Query<StatsParams>,
) -> Result<Json<Stats>, ApiError> {
    let until = requested_until(params.until_ms)?;
    let reader = state.query_reader.clone();
    let Json(mut stats) = read(move || reader.stats(params.window, until)).await?;
    let names = state
        .client_names
        .names_of(stats.clients.iter().map(|count| count.client))
        .await;
    for (count, name) in stats.clients.iter_mut().zip(names) {
        count.name = name;
    }
    Ok(Json(stats))
}

async fn clients(
    State(state): State<AppState>,
    Query(params): Query<StatsParams>,
) -> Result<Json<Vec<ObservedClient>>, ApiError> {
    let range = time_range(params.window, requested_until(params.until_ms)?);
    let reader = state.query_reader.clone();
    let Json(mut clients) = read(move || reader.clients(range)).await?;
    let names = state
        .client_names
        .names_of(clients.iter().map(|client| client.client))
        .await;
    for (client, name) in clients.iter_mut().zip(names) {
        client.name = name;
    }
    Ok(Json(clients))
}

async fn upstream_status(State(state): State<AppState>) -> Json<Vec<UpstreamStatus>> {
    Json(state.upstreams.status())
}

async fn blocklist_status(State(state): State<AppState>) -> Json<Vec<BlocklistStatus>> {
    Json(state.blocker.status())
}

async fn refresh_blocklists(
    State(state): State<AppState>,
    Json(()): Json<()>,
) -> Result<Json<Vec<BlocklistStatus>>, ApiError> {
    if !state.blocker.refresh_now().await {
        return Err(ApiError {
            status: StatusCode::CONFLICT,
            code: "list_refresh_running",
            message: "A list refresh is already running. Check the list status shortly.",
        });
    }
    Ok(Json(state.blocker.status()))
}

async fn system_status() -> Json<SystemStatus> {
    Json(SystemStatus {
        resident_memory_bytes: process::resident_memory_bytes(),
    })
}

#[derive(Deserialize)]
struct QueriesParams {
    limit: Option<usize>,
    #[serde(default)]
    offset: usize,
    client: Option<IpAddr>,
    domain: Option<String>,
    status: Option<api::QueryStatus>,
    #[serde(default)]
    domain_exact: bool,
    #[serde(default)]
    servfail: bool,
    window: Option<StatsWindow>,
    until_ms: Option<u64>,
}

async fn queries(
    State(state): State<AppState>,
    Query(params): Query<QueriesParams>,
) -> Result<Json<QueryPage>, ApiError> {
    let until = requested_until(params.until_ms)?;
    if params.until_ms.is_some() && params.window.is_none() {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_time_range",
            message: "A snapshot cutoff requires a window: 1h, 24h or 7d.",
        });
    }
    if params.offset > i64::MAX as usize {
        return Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_offset",
            message: "The query offset is too large. Reset the filters.",
        });
    }
    let limit = params.limit.unwrap_or(100).min(MAX_QUERIES_PER_REQUEST);
    let filter = QueryFilter {
        client: params.client,
        domain: params.domain,
        status: params.status.map(query_status),
        domain_exact: params.domain_exact,
        servfail: params.servfail,
        range: params.window.map(|window| time_range(window, until)),
    };
    let reader = state.query_reader.clone();
    let Json(mut page) = read(move || reader.queries(&filter, limit, params.offset)).await?;
    let names = state
        .client_names
        .names_of(page.entries.iter().map(|entry| entry.client))
        .await;
    for (entry, name) in page.entries.iter_mut().zip(names) {
        entry.client_name = name;
    }
    Ok(Json(page))
}

/// Runs a database read off the async runtime and turns failures into a 500 response.
/// Database messages may reveal internals, so the details only go to the server log.
async fn read<T: Send + 'static>(
    query: impl FnOnce() -> Result<T, StoreError> + Send + 'static,
) -> Result<Json<T>, ApiError> {
    let failed = |code| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code,
        message: "Reading the query log failed; the server log has the details.",
    };
    match tokio::task::spawn_blocking(query).await {
        Ok(Ok(value)) => Ok(Json(value)),
        Ok(Err(error)) => {
            error!(%error, "reading the query log failed");
            Err(failed(error.code()))
        }
        Err(error) => {
            error!(%error, "reading the query log panicked");
            Err(failed("query_store_failed"))
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn rejects_future_and_overflowing_snapshot_times() {
        assert!(requested_until(None).is_ok());
        assert!(requested_until(Some(1_000)).is_ok());
        assert!(requested_until(Some(u64::MAX)).is_err());
        let future = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 60_000;
        let error = requested_until(Some(future)).err().unwrap();
        assert_eq!(error.code, "invalid_time_range");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn finds_the_session_cookie_among_others() {
        let mut headers = HeaderMap::new();
        headers.append(header::COOKIE, HeaderValue::from_static("theme=dark"));
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("dns_session_old=x; dns_session=abc"),
        );

        assert_eq!(session_token(&headers), Some("abc"));
        assert_eq!(session_token(&HeaderMap::new()), None);
    }
}
