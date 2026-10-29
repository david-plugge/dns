//! Server configuration from command-line flags or the matching `DNS_*` environment variables.

use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use clap::Parser;

use crate::{
    auth::MasterPassword,
    blocking::{BlockingConfig, ListLocation, parse_domain},
    cache::{InvalidTtlBounds, TtlBounds},
    upstream::UpstreamSpec,
};

const PLAIN_DNS_PORT: u16 = 53;

#[derive(Debug, Parser)]
#[command(version, about = "DNS forwarder with a query dashboard")]
#[group(id = "master_password", required = true, multiple = false)]
pub struct Config {
    /// Address for DNS over UDP and TCP. `[::]` accepts IPv4 and IPv6.
    #[arg(long, env = "DNS_LISTEN", default_value = "[::]:53")]
    pub dns_listen: SocketAddr,

    /// Address for the dashboard and the HTTP API.
    #[arg(long, env = "DNS_HTTP_LISTEN", default_value = "[::]:8053")]
    pub http_listen: SocketAddr,

    /// Master password for the dashboard. Prefer the environment variable: command-line
    /// arguments are visible to every user of the host.
    #[arg(long, env = "DNS_PASSWORD", hide_env_values = true, value_parser = password, group = "master_password")]
    pub password: Option<MasterPassword>,

    /// Argon2id hash of the master password in PHC format (`$argon2id$v=19$…`), instead of
    /// `DNS_PASSWORD`, so deployment files need not contain the password.
    #[arg(long, env = "DNS_PASSWORD_HASH", hide_env_values = true, value_parser = MasterPassword::parse_argon2id, group = "master_password")]
    pub password_hash: Option<MasterPassword>,

    /// How long a dashboard login lasts.
    #[arg(long, env = "DNS_SESSION_LIFETIME", default_value = "7days", value_parser = positive_duration)]
    pub session_lifetime: Duration,

    /// Upstream resolvers, by IP: `udp://192.168.1.1`, `tls://9.9.9.9#dns.quad9.net`,
    /// `https://1.1.1.1/dns-query#cloudflare-dns.com`. Repeat the flag or separate with commas.
    #[arg(
        long = "upstream",
        env = "DNS_UPSTREAMS",
        value_delimiter = ',',
        required = true
    )]
    pub upstreams: Vec<UpstreamSpec>,

    /// How long to wait for one upstream before trying the next.
    #[arg(long, env = "DNS_UPSTREAM_TIMEOUT", default_value = "2s", value_parser = positive_duration)]
    pub upstream_timeout: Duration,

    /// Resolver of the local network, usually the router, asked over plain DNS on port 53.
    /// It names the clients in the dashboard and answers reverse lookups of private
    /// addresses, which otherwise go to the upstreams.
    #[arg(long, env = "DNS_LOCAL_RESOLVER")]
    pub local_resolver: Option<IpAddr>,

    /// How often every upstream is probed, so the health of idle upstreams stays current.
    #[arg(long, env = "DNS_UPSTREAM_PROBE_INTERVAL", default_value = "30s", value_parser = positive_duration)]
    pub upstream_probe_interval: Duration,

    /// Maximum number of cached responses; 0 disables the cache.
    #[arg(long, env = "DNS_CACHE_SIZE", default_value_t = 10_000)]
    pub cache_size: u64,

    /// Lower bound for how long responses are cached, overriding shorter TTLs.
    #[arg(long, env = "DNS_CACHE_MIN_TTL", default_value = "0s", value_parser = humantime::parse_duration)]
    pub cache_min_ttl: Duration,

    /// Upper bound for how long positive responses are cached.
    #[arg(long, env = "DNS_CACHE_MAX_TTL", default_value = "1day", value_parser = humantime::parse_duration)]
    pub cache_max_ttl: Duration,

    /// Upper bound for how long NXDOMAIN and empty responses are cached.
    #[arg(long, env = "DNS_CACHE_MAX_NEGATIVE_TTL", default_value = "1h", value_parser = humantime::parse_duration)]
    pub cache_max_negative_ttl: Duration,

    /// Lists of domains to block, as HTTP(S) URLs or file paths, in hosts, plain domain or
    /// adblock (`||domain^`) format. Repeat the flag or separate with commas.
    #[arg(long = "blocklist", env = "DNS_BLOCKLISTS", value_delimiter = ',')]
    pub blocklists: Vec<ListLocation>,

    /// Lists of domains that are never blocked, in the same formats as blocklists.
    #[arg(long = "allowlist", env = "DNS_ALLOWLISTS", value_delimiter = ',')]
    pub allowlists: Vec<ListLocation>,

    /// Single domains to block, in addition to the blocklists.
    #[arg(long = "blocked-domain", env = "DNS_BLOCKED_DOMAINS", value_delimiter = ',', value_parser = domain)]
    pub blocked_domains: Vec<Box<str>>,

    /// Single domains that are never blocked, in addition to the allowlists.
    #[arg(long = "allowed-domain", env = "DNS_ALLOWED_DOMAINS", value_delimiter = ',', value_parser = domain)]
    pub allowed_domains: Vec<Box<str>>,

    /// How often lists from URLs are downloaded again.
    #[arg(long, env = "DNS_BLOCKLIST_REFRESH_INTERVAL", default_value = "1day", value_parser = positive_duration)]
    pub blocklist_refresh_interval: Duration,

    /// Directory where downloaded lists are kept, so blocking works right after a restart.
    #[arg(
        long,
        env = "DNS_BLOCKLIST_CACHE_DIR",
        default_value = "/data/blocklists"
    )]
    pub blocklist_cache_dir: PathBuf,

    /// TTL of the null addresses returned for blocked domains.
    #[arg(long, env = "DNS_BLOCKED_TTL", default_value = "10s", value_parser = humantime::parse_duration)]
    pub blocked_ttl: Duration,

    /// DuckDB file holding the query log; created if missing.
    #[arg(long, env = "DNS_DATABASE", default_value = "/data/dns.duckdb")]
    pub database: PathBuf,

    /// Memory DuckDB may use, e.g. `64MB` or `1GB`.
    #[arg(long, env = "DNS_DATABASE_MEMORY_LIMIT", default_value = "64MB")]
    pub database_memory_limit: String,

    /// How long queries are kept in the query log.
    #[arg(long, env = "DNS_QUERY_LOG_RETENTION", default_value = "7days", value_parser = positive_duration)]
    pub query_log_retention: Duration,

    /// Directory containing the built dashboard (`index.html` and assets).
    #[arg(
        long,
        env = "DNS_DASHBOARD_DIR",
        default_value = "target/dx/dashboard/release/web/public"
    )]
    pub dashboard_dir: PathBuf,
}

impl Config {
    pub fn blocking(&self) -> BlockingConfig {
        BlockingConfig {
            blocklists: self.blocklists.clone(),
            allowlists: self.allowlists.clone(),
            blocked_domains: self.blocked_domains.clone(),
            allowed_domains: self.allowed_domains.clone(),
            cache_dir: self.blocklist_cache_dir.clone(),
            refresh_interval: self.blocklist_refresh_interval,
        }
    }

    pub fn master_password(&self) -> MasterPassword {
        self.password
            .clone()
            .or_else(|| self.password_hash.clone())
            .expect("clap requires DNS_PASSWORD or DNS_PASSWORD_HASH")
    }

    pub fn cache_ttl_bounds(&self) -> Result<TtlBounds, InvalidTtlBounds> {
        TtlBounds::new(
            self.cache_min_ttl,
            self.cache_max_ttl,
            self.cache_max_negative_ttl,
        )
    }

    pub fn local_resolver_spec(&self) -> Option<UpstreamSpec> {
        let address = self.local_resolver?;
        Some(UpstreamSpec::Plain {
            address: SocketAddr::new(address, PLAIN_DNS_PORT),
        })
    }
}

/// Intervals and timeouts of zero would make timers panic or every query fail.
fn positive_duration(text: &str) -> Result<Duration, String> {
    let duration = humantime::parse_duration(text).map_err(|error| error.to_string())?;
    if duration.is_zero() {
        return Err("must be longer than zero".to_owned());
    }
    Ok(duration)
}

fn password(text: &str) -> Result<MasterPassword, String> {
    if text.is_empty() {
        return Err("the password must not be empty".to_owned());
    }
    Ok(MasterPassword::Plain(text.into()))
}

fn domain(text: &str) -> Result<Box<str>, String> {
    parse_domain(text.trim()).ok_or_else(|| format!("{text:?} is not a valid domain name"))
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn flag_definitions_are_consistent() {
        Config::command().debug_assert();
    }

    #[test]
    fn zero_intervals_are_rejected() {
        assert!(positive_duration("0s").is_err());
        assert!(positive_duration("soon").is_err());
        assert_eq!(positive_duration("30s"), Ok(Duration::from_secs(30)));
    }
}
