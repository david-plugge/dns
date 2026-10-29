//! Parses upstream definitions such as `tls://9.9.9.9#dns.quad9.net`.
//!
//! Upstreams are always addressed by IP so that reaching them never requires DNS.
//! Encrypted upstreams name the certificate's server name after `#`.

use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

use rustls_pki_types::ServerName;

const PLAIN_PORT: u16 = 53;
const TLS_PORT: u16 = 853;
const HTTPS_PORT: u16 = 443;
const DEFAULT_HTTPS_PATH: &str = "/dns-query";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamSpec {
    /// UDP, retried over TCP when the response is truncated.
    Plain { address: SocketAddr },
    /// DNS over TLS (RFC 7858).
    Tls {
        address: SocketAddr,
        server_name: ServerName<'static>,
    },
    /// DNS over HTTPS (RFC 8484).
    Https {
        address: SocketAddr,
        server_name: ServerName<'static>,
        path: String,
    },
}

impl fmt::Display for UpstreamSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpstreamSpec::Plain { address } => write!(f, "udp://{address}"),
            UpstreamSpec::Tls {
                address,
                server_name,
            } => write!(f, "tls://{address}#{}", server_name.to_str()),
            UpstreamSpec::Https {
                address,
                server_name,
                path,
            } => write!(f, "https://{address}{path}#{}", server_name.to_str()),
        }
    }
}

impl FromStr for UpstreamSpec {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        let (scheme, rest) = input.split_once("://").unwrap_or(("udp", input));
        let (location, server_name) = match rest.split_once('#') {
            Some((location, name)) => (location, Some(parse_server_name(name, input)?)),
            None => (rest, None),
        };

        match (scheme, server_name) {
            ("udp", None) => Ok(UpstreamSpec::Plain {
                address: parse_address(location, PLAIN_PORT, input)?,
            }),
            ("udp", Some(_)) => Err(format!(
                "'{input}': plain DNS has no server name; remove the '#…' part"
            )),
            ("tls", Some(server_name)) => Ok(UpstreamSpec::Tls {
                address: parse_address(location, TLS_PORT, input)?,
                server_name,
            }),
            ("https", Some(server_name)) => {
                let (authority, path) = match location.find('/') {
                    Some(index) => location.split_at(index),
                    None => (location, DEFAULT_HTTPS_PATH),
                };
                Ok(UpstreamSpec::Https {
                    address: parse_address(authority, HTTPS_PORT, input)?,
                    server_name,
                    path: path.to_owned(),
                })
            }
            ("tls" | "https", None) => Err(format!(
                "'{input}': encrypted upstreams need the certificate's server name, e.g. '{scheme}://9.9.9.9#dns.quad9.net'"
            )),
            _ => Err(format!(
                "'{input}': unknown scheme '{scheme}', expected udp://, tls:// or https://"
            )),
        }
    }
}

fn parse_server_name(name: &str, input: &str) -> Result<ServerName<'static>, String> {
    if name.is_empty() {
        return Err(format!("'{input}': the server name after '#' is empty"));
    }
    ServerName::try_from(name.to_owned())
        .map_err(|_| format!("'{input}': '{name}' is not a valid server name"))
}

/// Accepts `1.2.3.4`, `1.2.3.4:53`, `[2620:fe::fe]` and `[2620:fe::fe]:53`.
fn parse_address(authority: &str, default_port: u16, input: &str) -> Result<SocketAddr, String> {
    if let Ok(address) = authority.parse::<SocketAddr>() {
        return Ok(address);
    }
    let host = authority
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(authority);
    host.parse::<IpAddr>()
        .map(|ip| SocketAddr::new(ip, default_port))
        .map_err(|_| {
            format!("'{input}': '{authority}' is not an IP address (hostnames are not supported)")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> UpstreamSpec {
        input.parse().unwrap()
    }

    fn server_name(name: &str) -> ServerName<'static> {
        ServerName::try_from(name.to_owned()).unwrap()
    }

    #[test]
    fn parses_plain_upstreams_with_and_without_scheme() {
        let expected = UpstreamSpec::Plain {
            address: "192.168.1.1:53".parse().unwrap(),
        };
        assert_eq!(parse("192.168.1.1"), expected);
        assert_eq!(parse("udp://192.168.1.1:53"), expected);
    }

    #[test]
    fn parses_tls_with_default_port_and_ipv6() {
        assert_eq!(
            parse("tls://[2620:fe::fe]#dns.quad9.net"),
            UpstreamSpec::Tls {
                address: "[2620:fe::fe]:853".parse().unwrap(),
                server_name: server_name("dns.quad9.net"),
            }
        );
    }

    #[test]
    fn parses_https_with_default_and_custom_path() {
        assert_eq!(
            parse("https://1.1.1.1#cloudflare-dns.com"),
            UpstreamSpec::Https {
                address: "1.1.1.1:443".parse().unwrap(),
                server_name: server_name("cloudflare-dns.com"),
                path: "/dns-query".to_owned(),
            }
        );
        assert_eq!(
            parse("https://9.9.9.9:5053/custom#dns.quad9.net"),
            UpstreamSpec::Https {
                address: "9.9.9.9:5053".parse().unwrap(),
                server_name: server_name("dns.quad9.net"),
                path: "/custom".to_owned(),
            }
        );
    }

    #[test]
    fn display_round_trips() {
        for input in [
            "udp://192.168.1.1:53",
            "tls://9.9.9.9:853#dns.quad9.net",
            "https://1.1.1.1:443/dns-query#cloudflare-dns.com",
        ] {
            assert_eq!(parse(input).to_string(), input);
        }
    }

    #[test]
    fn rejects_invalid_definitions() {
        for input in [
            "tls://9.9.9.9",
            "https://1.1.1.1",
            "tls://9.9.9.9#",
            "tls://9.9.9.9#not a name",
            "udp://1.1.1.1#name",
            "quic://9.9.9.9#dns.quad9.net",
            "tls://dns.quad9.net#dns.quad9.net",
        ] {
            assert!(
                input.parse::<UpstreamSpec>().is_err(),
                "{input} should be rejected"
            );
        }
    }
}
