//! The local network's own resolver, usually the router. It knows the names of the devices
//! and answers reverse lookups of private addresses, which public resolvers cannot.

use std::{net::IpAddr, sync::Arc, time::Duration};

use futures_util::future::join_all;
use hickory_proto::rr::{Name, RData, RecordType};
use moka::future::Cache;

use crate::upstream::UpstreamPool;

/// Device names rarely change; a new or renamed device shows up after at most this long.
const NAME_TTL: Duration = Duration::from_secs(600);
const MAX_CACHED_NAMES: u64 = 4096;

pub struct ClientNames {
    resolver: Option<Arc<UpstreamPool>>,
    cache: Cache<IpAddr, Option<Arc<str>>>,
}

impl ClientNames {
    /// Without a resolver no client has a name.
    pub fn new(resolver: Option<Arc<UpstreamPool>>) -> Self {
        let cache = Cache::builder()
            .max_capacity(MAX_CACHED_NAMES)
            .time_to_live(NAME_TTL)
            .build();
        Self { resolver, cache }
    }

    /// Names in the same order as `clients`; failed lookups count as unknown names.
    pub async fn names_of(&self, clients: impl IntoIterator<Item = IpAddr>) -> Vec<Option<String>> {
        join_all(clients.into_iter().map(|client| self.name_of(client))).await
    }

    async fn name_of(&self, client: IpAddr) -> Option<String> {
        let resolver = self.resolver.as_ref()?;
        let name = self.cache.get_with(client, lookup(resolver, client)).await;
        name.map(|name| name.to_string())
    }
}

async fn lookup(resolver: &UpstreamPool, client: IpAddr) -> Option<Arc<str>> {
    let records = resolver
        .lookup(Name::from(client), RecordType::PTR)
        .await
        .ok()?;
    records.iter().find_map(|record| match record {
        RData::PTR(ptr) => device_name(&ptr.0),
        _ => None,
    })
}

/// The host name without the local domain: `laptop` for `laptop.fritz.box.`.
fn device_name(name: &Name) -> Option<Arc<str>> {
    let host = name.iter().next()?;
    Some(String::from_utf8_lossy(host).into())
}

/// Whether `name` lies in a reverse zone of private, loopback or link-local addresses
/// (RFC 6303). Only the local network knows these names.
pub fn is_private_reverse_name(name: &str) -> bool {
    let name = name.strip_suffix('.').unwrap_or(name);
    if let Some(octets) = name.strip_suffix(".in-addr.arpa") {
        let octets: Vec<&str> = octets.rsplit('.').collect();
        return match octets.as_slice() {
            ["10", ..] | ["127", ..] | ["169", "254", ..] | ["192", "168", ..] => true,
            ["172", second, ..] => second
                .parse::<u8>()
                .is_ok_and(|second| (16..=31).contains(&second)),
            _ => false,
        };
    }
    if let Some(nibbles) = name.strip_suffix(".ip6.arpa") {
        let nibbles: Vec<&str> = nibbles.rsplit('.').collect();
        return matches!(
            nibbles.as_slice(),
            // fc00::/7 unique local and fe80::/10 link-local addresses.
            ["f", "c" | "d", ..] | ["f", "e", "8" | "9" | "a" | "b", ..]
        );
    }
    false
}

#[cfg(test)]
mod tests {
    use std::net::Ipv6Addr;

    use super::*;

    fn reverse_name(address: IpAddr) -> String {
        Name::from(address).to_string()
    }

    #[test]
    fn recognizes_reverse_names_of_private_addresses() {
        for private in [
            IpAddr::from([192, 168, 178, 27]),
            IpAddr::from([10, 1, 2, 3]),
            IpAddr::from([172, 20, 0, 1]),
            IpAddr::from([169, 254, 1, 1]),
            IpAddr::from([127, 0, 0, 1]),
            "fdc5:3e05:45f2::1".parse::<Ipv6Addr>().unwrap().into(),
            "fe80::1".parse::<Ipv6Addr>().unwrap().into(),
        ] {
            assert!(is_private_reverse_name(&reverse_name(private)), "{private}");
        }
        assert!(is_private_reverse_name("168.192.in-addr.arpa."));
    }

    #[test]
    fn leaves_reverse_names_of_public_addresses_alone() {
        for public in [
            IpAddr::from([8, 8, 8, 8]),
            IpAddr::from([172, 32, 0, 1]),
            IpAddr::from([192, 169, 0, 1]),
            "2001:db8::1".parse::<Ipv6Addr>().unwrap().into(),
        ] {
            assert!(!is_private_reverse_name(&reverse_name(public)), "{public}");
        }
        assert!(!is_private_reverse_name("192.in-addr.arpa."));
        assert!(!is_private_reverse_name("example.com."));
    }

    #[test]
    fn device_names_drop_the_local_domain() {
        let name = Name::from_ascii("S22-Ultra.fritz.box.").unwrap();

        assert_eq!(device_name(&name).as_deref(), Some("S22-Ultra"));
        assert_eq!(device_name(&Name::root()), None);
    }
}
