mod auth;
mod blocking;
mod cache;
mod config;
mod dns;
mod error;
mod http;
mod local_network;
mod metrics;
mod outcome;
mod process;
mod query_store;
mod upstream;

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use clap::Parser;
use hickory_server::Server;
use tokio::{
    net::{TcpListener, UdpSocket},
    signal::unix::{SignalKind, signal},
};
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::{
    auth::Authenticator,
    blocking::{Blocker, Downloader},
    cache::DnsCache,
    config::Config,
    dns::ForwardingHandler,
    local_network::ClientNames,
    metrics::Metrics,
    query_store::{QueryReader, QueryWriter},
    upstream::UpstreamPool,
};

/// Idle TCP connections are closed after this long.
const TCP_TIMEOUT: Duration = Duration::from_secs(10);
const TCP_RESPONSE_BUFFER: usize = 32;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,hickory_server=warn")),
        )
        .init();

    let config = Config::parse();
    let cache_ttl_bounds = config.cache_ttl_bounds()?;
    let database = query_store::open(&config.database, &config.database_memory_limit)
        .with_context(|| format!("could not open the database {}", config.database.display()))?;
    let query_reader = Arc::new(QueryReader::new(database.try_clone()?));
    let (query_writer, query_recorder) = QueryWriter::spawn(database, config.query_log_retention);
    let upstreams = Arc::new(UpstreamPool::new(
        config.upstreams.clone(),
        config.upstream_timeout,
    ));
    let local_resolver = config
        .local_resolver_spec()
        .map(|spec| Arc::new(UpstreamPool::new(vec![spec], config.upstream_timeout)));
    let cache = DnsCache::new(config.cache_size, cache_ttl_bounds);
    let downloader =
        Downloader::new(upstreams.clone()).context("could not set up list downloads")?;
    let blocker = Arc::new(Blocker::new(config.blocking(), downloader));
    // Lists from the cache and local files are loaded before serving, so nothing slips through.
    blocker.load().await;
    tokio::spawn(blocker.clone().keep_refreshed());
    tokio::spawn(probe_upstreams(
        upstreams.clone(),
        config.upstream_probe_interval,
    ));

    let metrics = Arc::new(Metrics::new());
    let mut dns_server = Server::new(ForwardingHandler::new(
        upstreams.clone(),
        local_resolver.clone(),
        cache,
        blocker.clone(),
        config.blocked_ttl,
        query_recorder.clone(),
        metrics.clone(),
    ));
    let udp = UdpSocket::bind(config.dns_listen)
        .await
        .with_context(|| format!("could not bind DNS over UDP to {}", config.dns_listen))?;
    let tcp = TcpListener::bind(config.dns_listen)
        .await
        .with_context(|| format!("could not bind DNS over TCP to {}", config.dns_listen))?;
    dns_server.register_socket(udp);
    dns_server.register_listener(tcp, TCP_TIMEOUT, TCP_RESPONSE_BUFFER);
    let upstream_names: Vec<_> = config.upstreams.iter().map(ToString::to_string).collect();
    info!(
        listen = %config.dns_listen,
        upstreams = ?upstream_names,
        local_resolver = ?config.local_resolver,
        "DNS server started"
    );

    let http_listener = TcpListener::bind(config.http_listen)
        .await
        .with_context(|| format!("could not bind HTTP to {}", config.http_listen))?;
    let app = http::router(
        http::AppState {
            auth: Arc::new(Authenticator::new(
                config.master_password(),
                config.session_lifetime,
            )),
            query_reader,
            client_names: Arc::new(ClientNames::new(local_resolver)),
            upstreams,
            blocker,
            metrics,
            query_recorder,
        },
        &config.dashboard_dir,
    );
    let http_server = tokio::spawn(
        axum::serve(http_listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .into_future(),
    );
    info!(listen = %config.http_listen, "HTTP server started");

    shutdown_signal().await;
    info!("shutting down");
    dns_server
        .shutdown_gracefully()
        .await
        .context("DNS server failed")?;
    http_server.await?.context("HTTP server failed")?;
    tokio::task::spawn_blocking(|| query_writer.shutdown()).await?;
    Ok(())
}

async fn probe_upstreams(upstreams: Arc<UpstreamPool>, interval: Duration) {
    let mut ticks = tokio::time::interval(interval);
    loop {
        ticks.tick().await;
        upstreams.probe_all().await;
    }
}

async fn shutdown_signal() {
    let mut terminate = signal(SignalKind::terminate()).expect("could not install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}
