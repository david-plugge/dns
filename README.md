# dns

A DNS forwarder with a query dashboard, in the spirit of Pi-hole and AdGuard Home.

## Layout

| Crate | Purpose |
|---|---|
| `crates/server` | DNS forwarder (UDP/TCP clients; UDP, DoT and DoH upstreams), blocking and HTTP API |
| `crates/api` | Types shared by the HTTP API and the dashboard |
| `crates/dashboard` | Dioxus web SPA, served by the server |

## Running locally

Tools and the development environment (`DNS_*` variables, using Quad9 and Cloudflare as
upstreams) are defined in `mise.toml`.

```sh
mise exec -- dx build --release --web -p dashboard   # build the dashboard
mise exec -- cargo run -p server                      # DNS on 127.0.0.1:5300, HTTP on 127.0.0.1:8053
dig @127.0.0.1 -p 5300 example.com
```

Open <http://127.0.0.1:8053>. For dashboard development with hot reload, run
`mise exec -- dx serve -p dashboard --port 8055` next to the server; it proxies `/api` to port 8053.

## Dashboard UI

The dashboard uses source-owned [Dioxus Components](https://dioxuslabs.github.io/dioxus-components/)
with local CSS modules; no Node or Tailwind build is required. Component provenance and update
notes are in [`crates/dashboard/src/components/README.md`](crates/dashboard/src/components/README.md).
Layout, chart colors and the dark theme are customized in `crates/dashboard/assets/main.css`.

The selected 1h/24h/7d period applies to statistics, queries and observed clients. Counters,
chart legends and top lists open a filtered query snapshot. Query details show recorded data,
not a re-evaluation of today's rules. Pausing the live log freezes only the displayed query
view; DNS service and filtering continue. Paging uses the same server-derived time cutoff,
but late log writes and retention can still affect subsequent pages.

Filters, period, page and a frozen log's cutoff are kept in the URL, not browser storage.
These URLs contain client addresses and domain searches; treat shared links accordingly.
The client filter suggests observed devices by name or IP and shows both after selection.
Enter a full device name and press Enter, or choose a suggestion; duplicate names require
selecting a specific IP. Shared URLs retain the IP rather than the device name.
The client inventory is derived from retained queries (up to 1,000 addresses per selected
period), so it is not permanent and cannot identify devices hidden behind a forwarding router.

Client naming uses the configured local resolver; filtering rules and list sources remain
deployment-managed. **Refresh lists** reloads
existing sources and updates the existing list cache without editing configuration. A failed
download can leave previously loaded rules active; the list table reports retained rules and
errors separately. No new persistent state or configuration store is introduced.

The optional filter-layout regression requires Node.js and Playwright with Chromium. It checks
that pending filter requests do not collapse the table or move the viewport:

```sh
DNS_DASHBOARD_URL=http://127.0.0.1:18153 DNS_PASSWORD='<local-dashboard-password>' \
  node crates/dashboard/tests/filter-layout.cjs
```

Set `PLAYWRIGHT_MODULE` to the module path if Playwright is installed outside Node's search path.

## Configuration

Every setting is a command-line flag or the matching environment variable; there is no
configuration file. `server --help` lists all of them with their defaults.

Variables holding several values take them separated by commas or newlines, so long lists can
be written one per line, e.g. in Compose:

```yaml
DNS_BLOCKLISTS: |
  https://big.oisd.nl
  https://cdn.jsdelivr.net/gh/hagezi/dns-blocklists@latest/adblock/pro.txt
```

| Variable | Default | |
|---|---|---|
| `DNS_UPSTREAMS` | – (required) | Upstreams, see below |
| `DNS_PASSWORD` | – | Master password for the dashboard; this or `DNS_PASSWORD_HASH` is required |
| `DNS_PASSWORD_HASH` | – | Argon2id hash of the master password (PHC format), so deployment files need not hold the password |
| `DNS_SESSION_LIFETIME` | `7days` | How long a dashboard login lasts |
| `DNS_LISTEN` | `[::]:53` | DNS over UDP and TCP; `[::]` accepts IPv4 and IPv6 |
| `DNS_HTTP_LISTEN` | `[::]:8053` | Dashboard and HTTP API |
| `DNS_UPSTREAM_TIMEOUT` | `2s` | Per upstream, before failing over to the next |
| `DNS_UPSTREAM_PROBE_INTERVAL` | `30s` | Health probe of every upstream |
| `DNS_LOCAL_RESOLVER` | – | Router IP, e.g. `192.168.178.1`: names clients in the dashboard and answers reverse lookups of private addresses |
| `DNS_CACHE_SIZE` | `10000` | Cached responses; `0` disables the cache |
| `DNS_CACHE_MIN_TTL` / `DNS_CACHE_MAX_TTL` | `0s` / `1day` | Bounds for positive responses |
| `DNS_CACHE_MAX_NEGATIVE_TTL` | `1h` | Bound for NXDOMAIN and empty responses |
| `DNS_BLOCKLISTS` | – | Lists to block, see below |
| `DNS_ALLOWLISTS` | – | Lists that are never blocked |
| `DNS_BLOCKED_DOMAINS` / `DNS_ALLOWED_DOMAINS` | – | Single domains to block or allow |
| `DNS_BLOCKLIST_REFRESH_INTERVAL` | `1day` | How often lists from URLs are downloaded again |
| `DNS_BLOCKLIST_CACHE_DIR` | `/data/blocklists` | Downloaded lists, used right after a restart |
| `DNS_BLOCKED_TTL` | `10s` | TTL of the answers for blocked domains |
| `DNS_DATABASE` | `/data/dns.duckdb` | DuckDB file holding the query log |
| `DNS_DATABASE_MEMORY_LIMIT` | `64MB` | Memory DuckDB may use |
| `DNS_QUERY_LOG_RETENTION` | `7days` | Queries older than this are deleted hourly |
| `DNS_DASHBOARD_DIR` | `target/dx/dashboard/release/web/public` | Built dashboard |

Upstreams are addressed by IP so that reaching them needs no DNS. Encrypted upstreams name
the certificate's server name after `#`:

```
udp://192.168.1.1                               plain DNS, port 53, TCP on truncation
tls://9.9.9.9#dns.quad9.net                     DNS over TLS, port 853
https://1.1.1.1/dns-query#cloudflare-dns.com    DNS over HTTPS, port 443
tls://[2620:fe::fe]:853#dns.quad9.net           IPv6 and explicit ports work for all schemes
```

The fastest healthy upstream is used; on errors or timeouts the next one takes over.

## Blocking

Block- and allowlists are HTTP(S) URLs or file paths. They may be in hosts format
(`0.0.0.0 ads.example.com`), plain domain lists, or adblock syntax (`||ads.example.com^`,
exceptions as `@@||ads.example.com^`); other adblock rules are skipped and counted on the
dashboard. Every entry also covers its subdomains, and allowing always wins over blocking.

Blocked A and AAAA queries are answered with `0.0.0.0` and `::`, all other types with an
empty answer. Downloaded lists are kept in `DNS_BLOCKLIST_CACHE_DIR`, so blocking is active
from the first query after a restart, even without internet access. List hosts are resolved
through the configured upstreams, not the system resolver.

## Dashboard login and metrics

The dashboard and `/api/*` require logging in with the master password. Sessions are kept in
memory, so a restart logs everyone out; failed logins are slowed down to about one per
second. `/health` and `/metrics` stay open.

To keep the password out of deployment files, set `DNS_PASSWORD_HASH` instead, e.g. created
with the `argon2` CLI (in Compose files, write every `$` as `$$`):

```sh
printf '%s' "$PASSWORD" | argon2 "$(openssl rand -base64 16)" -id -e
```

API errors are JSON `{"code": "...", "message": "..."}`. The code is stable (e.g.
`session_required`, `wrong_password`, `query_store_failed`), so clients branch on it; the
message is for people. Upstream and list errors on the dashboard carry codes the same way
(e.g. `timeout`, `connection_failed`).

`/metrics` serves Prometheus metrics in the OpenMetrics format: queries and their duration
by status, upstream health, latency and failures, blocklist sizes, update times and
failures, and dropped query log entries. It contains no client addresses or domains.

Log verbosity is controlled with `RUST_LOG` (default `info,hickory_server=warn`).
