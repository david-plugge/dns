# DNS dashboard: AdGuard Home and Pi-hole usability research

Research date: 2026-11-07. Status: research and proposals, not an implementation plan or accepted feature scope.

## Summary

The most useful pattern to borrow is **overview → filtered queries → explanation → narrowly scoped action → verification**. Our dashboard already covers much of the observation side. Its main opportunity is connecting the information and making troubleshooting actionable, rather than adding more charts.

Recommended order:

1. Improve query investigation using existing data: visible filters, reset, live-update pause, query details and copy actions.
2. Add consistent time filtering and overview-to-query drill-down.
3. Decide configuration ownership, then add persistent domain exceptions and list-management actions.
4. Add a client inventory and effective-policy views when there is a concrete device-management need.
5. Consider local DNS records and privacy controls separately. Defer DHCP, parental-control suites and certificate administration.

These are qualitative recommendations, not measured usability results. The separate timed-blocking-pause branch remains deferred; this research does not reintroduce it into the UI branch.

## Scope and evidence

- **AdGuard Home**, the self-hosted DNS server, not hosted AdGuard DNS or a browser extension. UI sources are pinned to **v0.107.80**, released 2026-11-03. [A0]
- **Pi-hole v6**, with web/admin UI sources pinned to **v6.6.1**, released 2026-10-17. This identifies the web component, not a unified version for all Pi-hole components. [P0]
- Official documentation, release metadata and first-party UI/source were inspected. The products were **not installed or exercised in a browser**. Source establishes controls and intended workflows, not runtime accessibility, reliability or measured ease of use.
- Living documentation and the separately versioned FTL API can differ from tagged UI source. API/configuration capabilities are not automatically dashboard features.
- Local baseline: `feat/dioxus-components`, based on `main` at `9fc1a16`, including the uncommitted dashboard modernization. No application changes were made for this research.

## How the products organize the interface

### AdGuard Home

Top-level navigation separates **Dashboard, Settings, Filters, Query log and Setup guide**. Settings separates general behavior, DNS, encryption, clients and DHCP. Filters separates subscriptions, custom filtering rules, rewrites and blocked services. [A1]

Its most transferable workflow is contextual: counters and top clients/domains lead into query searches; log details explain the result; row actions can create global or client-specific domain rules. Mobile rows use a details modal rather than requiring desktop hover interactions. [A2–A4]

### Pi-hole

A sidebar exposes monitoring, query investigation, group-based policy management, settings and tools. Basic/expert disclosure keeps many advanced settings out of the default view. The official UI README illustrates current v6 query filters and settings, but also contains some older screenshots; tagged templates and JavaScript are stronger evidence for exact behavior. [P1, P2]

Its notable workflow is drill-down: dashboard top items carry filters into the query log, and chart clicks open a time interval around the selected point. Query investigation distinguishes live monitoring from historical disk queries. [P3, P4]

**Lesson for us:** borrow connected tasks and progressive disclosure, not either product's full navigation tree. A permanent sidebar is not needed merely to add a few actions.

## Feature and workflow comparison

“Not established” means the inspected evidence did not verify a capability; it is not a claim that the product cannot provide it.

| Task | AdGuard Home | Pi-hole v6 | Our current baseline |
|---|---|---|---|
| Understand activity | Traffic timeline, filtering/security counters, top clients/domains and upstream response/latency summaries; links into query searches. [A2] | Traffic/client timelines, query-type and upstream charts, top lists and contextual log links. [P3] | Traffic, blocking, failures, cache rate, latency, top clients/domains and upstream health. Charts/top lists are not interactive. [L1, L2] |
| Investigate a query | Domain/client search, response categories, compact/detailed views; matching rules/lists, upstream/cache and answers where available. [A3] | Domain, client, upstream, type, status, reply and DNSSEC filters; time picker and expandable details, including CNAME blocking information. [P4] | Client IP, domain-substring and result filters; pagination. Rows already contain response code, upstream/list/error and latency, but not answer records, TTLs or exact matched-rule provenance. [L3, L4] |
| Allow or block from the evidence | Row actions generate custom domain rules globally or for a client. Entire-client DNS-access denial is a different action, not a filtering pause. [A3, A4] | Row actions add domain rules. The row handler does not pass client/group scope; group policy is managed separately. [P4, P5] | Domains can be configured through environment variables/CLI, not changed through the dashboard. [L5, L6] |
| Temporarily suspend filtering | Global indefinite/timed protection controls with remaining time and re-enable. Client settings support overrides, but an equivalent timed per-client pause was not established. [A2, A5] | Global indefinite/timed blocking controls and re-enable. The blocking API has timer semantics but no client/group selector. [P2, P15] | Not on the UI branch. Work exists on a separate, deferred branch; it must not be counted as available here. |
| Recognize and configure clients | Discovered versus persistent clients; names, multiple identifiers, tags, inheritance, per-client protection, upstreams and logging exclusions. MAC identification requires Home's DHCP server. [A5, A6] | Client identifiers and multiple group assignments. Identifier precedence and delayed discovery matter; MAC recognition is limited to a networking hop. [P6] | Router-based reverse-DNS names plus client IPs in logs/top lists; no editable client inventory or policy. [L4, L7] |
| Target different policies | Client settings and `$client`/`$ctag` rules. A first-class reusable group-policy editor was not established. [A5, A6] | Explicit groups associate clients, subscriptions and domain rules. Default-group membership is important and not removed automatically when another group is added. [P5] | One global filtering policy. [L5, L8] |
| Maintain lists and rules | Separate block/allow subscriptions with enable/edit/delete/update; custom-rule editor and hostname/client/type checking tool. [A7] | Block/allow subscriptions, exact/regex domain rules, comments, groups and download-state distinctions. Domain search and list-update tools are exposed. [P1, P7] | Block/allow subscriptions and inline domains; cached downloads, refreshes and list/error status. Dashboard is read-only. Entries also cover subdomains; allowing wins. [L5, L6, L8] |
| Provide local DNS names | Dedicated domain/answer rewrite editor, including wildcard examples; advanced rewrite syntax is separate. [A7, A10] | Local domain/IP records and CNAME records in a dedicated settings page. Its help documents target-resolution and restart constraints. [P8] | Router-backed private reverse lookups; no arbitrary local-record editor. [L5, L7] |
| Configure upstreams | Separate upstream/fallback/bootstrap/private-reverse settings; **Test upstreams** can test unsaved values before Save. File-managed upstream configuration disables editing. [A4, A8, A10] | Preset/custom upstreams, DNSSEC and conditional forwarding. Per-upstream synthetic test/health monitoring was not established from inspected UI. [P9] | Native UDP/DoT/DoH upstreams, selection/failover, active probes, health, latency and errors. Configuration is deployment-managed. [L5, L6] |
| Control logging and privacy | Logging and statistics have separate controls/retention/reset; client exclusions and IP anonymization. A dedicated query-export UI was not established. [A5, A8, A10] | Privacy levels, logging/storage controls and retention. Database retention is distinct from the in-memory/API history horizon. [P10] | Query retention defaults to seven days and is deployment-configured. No dashboard logging/anonymization/delete/export controls. [L5, L6] |
| Back up configuration | No dedicated configuration-backup UI was established by this research; API/file access should not be described as a verified backup workflow. [A9, A10] | Teleporter exports/imports configuration, rules, clients and other selected state; excludes long-term query history and warns that exports contain secrets. [P11] | No application-level import/export workflow. [L5, L6] |
| Set up devices and advanced networking | Initial browser wizard plus ongoing setup guide; native encrypted-DNS serving, certificate settings, DHCP, Safe Search/parental controls and service schedules. [A1, A5, A7, A8, A11] | Post-install router/device DNS instructions; DHCP/leases/static assignments and advanced settings. The inspected encrypted-upstream guide uses an external proxy and now deprecates its particular recipe. [P12–P14] | CLI/environment setup; plain UDP/TCP client DNS and encrypted upstreams. No DHCP, certificate UI, encrypted client-facing DNS listener or parental-control suite. [L5, L6] |

## Important distinctions to preserve

1. **DNS health is not filtering state.** A resolver can be reachable while filtering is paused, or filtering can remain active while a list download fails. Show resolver health, protection state and data freshness separately. Cached-list/download-state distinctions in both products support this separation. [A2, A7, P7]
2. **A DNS reply is not an allow-rule match.** Our `Answered` outcome only identifies cache/upstream service. Do not label it “allowlisted” without additional policy evidence. `NXDOMAIN` is not automatically a local block. [L4]
3. **Domain exception, client policy and DNS-access denial have different scope.** AdGuard Home exposes distinct actions; Pi-hole's group workflow requires explicit membership awareness. Avoid a generic “Unblock” action whose reach is hidden. [A3–A6, P5]
4. **Time context must be honest.** Our 1h/24h/7d selection currently changes statistics, not the query log. Until a log time filter exists, label the log as all retained queries; a drill-down must not imply a time constraint it does not enforce. [L1, L3, L6]
5. **Historical evidence and current policy checks are different.** A recorded blocklist identifies the historical source; testing today's configuration cannot reconstruct an exact historical rule that was never recorded. [L4, L8]
6. **Client identity has limits.** Names aid recognition but are not stable authorization identifiers. A DNS-forwarding router may hide devices behind its own address; MAC-based approaches also have topology/DHCP constraints. Explain observed IP versus configured device identity. [A6, P6, L7]
7. **DNS changes do not erase client caches.** Confirmation should explain verification and cache/TTL effects rather than promise that a website immediately works. Also distinguish traffic using this resolver from clients using another DNS path.
8. **Configuration backup is not query-history export.** State which data is included and whether secrets or client/domain history are present. Pi-hole's Teleporter makes this distinction explicit. [P11]

## Proposed backlog

Priorities and effort are relative judgments for this project, not delivery estimates. Nothing below is authorized for implementation by this document.

| Priority | Improvement | Scope and dependency |
|---|---|---|
| **P0** | Query-detail drawer/modal | Full domain and client name/IP, recorded decision, response code, list/upstream, readable errors, latency and copy buttons. Existing row data is sufficient; returned records, TTLs and exact matching rules are explicitly out of scope. Ensure keyboard and touch access. |
| **P0** | Understandable filtering | Visible active-filter chips, Clear all, filter counts, useful empty states and URL-persisted filters/back navigation. Clearly label the independent statistics period and log retention scope. Existing read APIs suffice for current filters. |
| **P0** | Freeze live inspection | Pause/resume **log updates**, visibly distinct from pausing DNS protection. Freeze the rendered rows while inspecting. This alone does not create a consistent historical snapshot across offset-paginated pages. |
| **P0** | More useful existing summaries | Accessible chart-value tooltips/focus information, readable number formatting and click-to-filter for existing client/domain/status filters. Explicitly disclose all-retained log scope until time filtering is implemented. |
| **P1** | Consistent time-based investigation | Add query time-range support to the API/store, then carry period/client/domain/status through KPI, chart and top-list drill-down. Choose deliberate live versus fixed historical semantics; do not just filter the currently loaded 50 rows. |
| **P1** | Persistent domain exceptions | Add scoped, understandable allow/block actions with confirmation, provenance, undo and a small rule-management view. **Blocked on configuration-ownership and matching-semantics decisions below.** Do not edit downloaded lists to create exceptions. |
| **P1, deferred work** | Visible timed protection pause | If the separate pause work is resumed, retain explicit global/selected-client scope, authoritative remaining duration, persistent resume access and an explanation of client caching. Do not pull it into this branch implicitly. |
| **P2** | Client inventory | List observed clients, name/IP, last seen and links to queries. Reuse router names first. Manual names and per-device policies require persistent identity/configuration design; do not call an IP list a stable device inventory. |
| **P2** | List management and diagnostics | Expose effective source/configuration ownership, last successful state versus latest error, and unsupported syntax. Add manual refresh, domain checking and narrowly scoped editing only with corresponding backend support. |
| **P2** | Privacy, export and recovery | Show effective retention/logging settings; design logging exclusions/anonymization/deletion deliberately. Treat safe configuration backup and bounded query export as separate features. New read/write APIs and lifecycle behavior need explicit design. |
| **P2, demand-driven** | Simple local DNS records | Dedicated records/aliases workflow if internal naming is a real need. Specify record types, precedence, caching and validation rather than building a general zone editor. |
| **Later / separate scope** | DHCP, encrypted client-facing DNS, schedules and parental suites | These add network-service ownership, certificate lifecycle or a much larger policy model. They should not be prerequisites for a usable DNS dashboard. Native encrypted **upstreams already exist** and are not a missing feature. |

### Configuration ownership: decide before adding mutation buttons

Today every setting comes from CLI/environment values; there is no writable application configuration file. Adding a form without deciding persistence and precedence creates settings that disappear or conflict with deployment. [L5, L6]

Two reasonable options:

- **Deployment remains authoritative:** show effective configuration read-only, label it as externally managed, and help generate changes for the deployment. Predictable for GitOps, but no immediate persistent “Allow this domain” action.
- **Application-owned runtime rules:** keep deployment-managed infrastructure separate from a persisted rule/exception store. Define precedence, restart behavior, backup and conflict handling. Enables immediate exceptions, but creates new durable state and operating responsibilities.

Pi-hole documents that environment-defined values become read-only elsewhere; AdGuard Home disables upstream editing for a file-backed configuration. These are useful ownership cues, not evidence that their persistence models should be copied wholesale. [P10, A8, A10]

A second decision is rule meaning: our current domain entries include subdomains. A button labeled “Only this hostname” would require new semantics; it must not silently create a suffix-wide exception. Also decide whether an action is global or client-scoped and how it interacts with inherited policy. [L5, L8]

## Suggested next slice and acceptance scenarios

Start with **query details, visible/resettable filters and live-log freeze**, plus honest statistics/log scope labels. This improves diagnosis without introducing mutable DNS configuration or reviving the deferred pause branch.

Suggested future acceptance checks, not tests already performed:

- Open a blocked query and identify its recorded source list without relying on hover, on desktop and mobile.
- Copy the complete domain/IP and inspect an error without truncation.
- Set client/domain/result filters, inspect the active criteria, and clear them in one action.
- Reload or navigate Back and recover the same URL-encoded filter state.
- Freeze the log; rows remain stable across at least two polling intervals. Resume returns to live updates, and DNS protection is unaffected.
- Selecting a statistics period cannot falsely suggest that the log is filtered to that period.
- After a later time-filtering slice, a dashboard drill-down reproduces its time/client/domain/status context in the query view.

Grow navigation only as functionality exists: **Overview** and **Queries** first; add **Clients**, **Filtering** and **System/settings** when they own actual workflows. Keep low-frequency infrastructure controls progressively disclosed.

## Primary sources

### AdGuard Home

- [A0] [v0.107.80 release](https://github.com/AdguardTeam/AdGuardHome/releases/tag/v0.107.80); publication metadata checked via the GitHub releases API.
- [A1] [Release-tagged navigation](https://github.com/AdguardTeam/AdGuardHome/blob/v0.107.80/client/src/components/Header/Menu.tsx).
- [A2] [Dashboard, counters, top tables and protection controls](https://github.com/AdguardTeam/AdGuardHome/tree/v0.107.80/client/src/components/Dashboard).
- [A3] [Query-log filters, detail views and row actions](https://github.com/AdguardTeam/AdGuardHome/tree/v0.107.80/client/src/components/Logs).
- [A4] [Action implementations: generated rules and upstream testing](https://github.com/AdguardTeam/AdGuardHome/blob/v0.107.80/client/src/actions/index.tsx).
- [A5] [Client forms: inheritance, protection, exclusions and schedules](https://github.com/AdguardTeam/AdGuardHome/tree/v0.107.80/client/src/components/Settings/Clients/Form).
- [A6] [Official client documentation](https://adguard-dns.io/kb/adguard-home/clients/).
- [A7] [Subscriptions, custom rules/checking, rewrites and blocked services](https://github.com/AdguardTeam/AdGuardHome/tree/v0.107.80/client/src/components/Filters).
- [A8] [Settings UI: DNS, logging, statistics, encryption and DHCP](https://github.com/AdguardTeam/AdGuardHome/tree/v0.107.80/client/src/components/Settings).
- [A9] [Release-tagged OpenAPI](https://github.com/AdguardTeam/AdGuardHome/blob/v0.107.80/openapi/openapi.yaml): query retrieval is not evidence of a download/backup UI.
- [A10] [Official configuration reference](https://adguard-dns.io/kb/adguard-home/configuration/).
- [A11] Official [getting started](https://adguard-dns.io/kb/adguard-home/getting-started/), [encryption](https://adguard-dns.io/kb/adguard-home/encryption/) and [DHCP](https://adguard-dns.io/kb/adguard-home/dhcp/) guides.

### Pi-hole

- [P0] [Web/admin v6.6.1 release](https://github.com/pi-hole/web/releases/tag/v6.6.1); publication metadata checked via the GitHub releases API.
- [P1] [Release-tagged UI README and screenshots](https://github.com/pi-hole/web/blob/v6.6.1/README.md).
- [P2] [Navigation and blocking controls](https://github.com/pi-hole/web/blob/v6.6.1/scripts/lua/sidebar.lp).
- [P3] [Dashboard charts and drill-down links](https://github.com/pi-hole/web/blob/v6.6.1/scripts/js/index.js).
- [P4] [Query filters, details, row actions and disk/live behavior](https://github.com/pi-hole/web/blob/v6.6.1/scripts/js/queries.js).
- [P5] [Official group-policy examples and Default membership](https://docs.pi-hole.net/group_management/example/). Explanatory screenshots can predate v6.
- [P6] [Client-management identifiers and constraints](https://github.com/pi-hole/web/blob/v6.6.1/groups-clients.lp).
- [P7] [Subscribed lists and download states](https://github.com/pi-hole/web/blob/v6.6.1/groups-lists.lp); [exact/regex domain rules](https://github.com/pi-hole/web/blob/v6.6.1/groups-domains.lp).
- [P8] [Local DNS and CNAME settings](https://github.com/pi-hole/web/blob/v6.6.1/settings-dnsrecords.lp).
- [P9] [DNS/upstream settings](https://github.com/pi-hole/web/blob/v6.6.1/settings-dns.lp).
- [P10] [Official FTL configuration reference](https://docs.pi-hole.net/ftldns/configfile/): environment precedence, privacy, retention and history.
- [P11] [Teleporter import/export scope and warnings](https://github.com/pi-hole/web/blob/v6.6.1/settings-teleporter.lp).
- [P12] [Post-install network setup](https://docs.pi-hole.net/main/post-install/).
- [P13] [DHCP settings](https://github.com/pi-hole/web/blob/v6.6.1/settings-dhcp.lp).
- [P14] [Official cloudflared guide and deprecation warning](https://docs.pi-hole.net/guides/dns/cloudflared/). This is evidence about that external integration, not an exhaustive proof of absent encrypted-DNS features.
- [P15] [Separately versioned FTL blocking-control API](https://github.com/pi-hole/FTL/blob/master/src/api/docs/content/specs/dns.yaml): timer and scope semantics, not UI evidence.

### Local source baseline

- [L1] [Dashboard state, polling and statistics controls](../../crates/dashboard/src/main.rs).
- [L2] [Non-interactive chart components](../../crates/dashboard/src/charts.rs).
- [L3] [Current log controls and row presentation](../../crates/dashboard/src/query_log.rs); [filter state/API client](../../crates/dashboard/src/api_client.rs).
- [L4] [Shared query/status data types](../../crates/api/src/lib.rs).
- [L5] [Documented configuration and filtering semantics](../../README.md).
- [L6] [Configuration inputs](../../crates/server/src/config.rs); [HTTP API surface](../../crates/server/src/http.rs); [query filters/store](../../crates/server/src/query_store/reader.rs).
- [L7] [Router-based client naming and private reverse DNS](../../crates/server/src/local_network.rs).
- [L8] [Blocking configuration and source reporting](../../crates/server/src/blocking/mod.rs); [allow/block matching](../../crates/server/src/blocking/blocklist.rs).
