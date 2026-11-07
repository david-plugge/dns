mod api_client;
mod blocklists;
mod charts;
mod client_filter;
mod clients;
mod components;
mod format;
mod login;
mod query_details;
mod query_log;
mod status;
mod upstreams;
mod view;
mod visibility;

use api::{ObservedClient, QueryPage, QueryStatus, Stats, StatsWindow};
use dioxus::prelude::*;
use dioxus_icons::lucide::{LogOut, Network};
use gloo_timers::future::TimeoutFuture;
use std::net::IpAddr;

use crate::{
    api_client::{FetchError, ServerStatus, last_page_offset},
    charts::{Bar, BarList, LineChart, Series},
    clients::ObservedClients,
    components::{
        button::{Button, ButtonSize, ButtonVariant},
        card::{Card, CardContent, CardDescription, CardHeader, CardTitle},
    },
    format::{
        client_name, domain_name, duration_name, percentage, window_name, window_short_name,
        x_labels,
    },
    login::LoginForm,
    query_log::QueryLog,
    status::StatusPanel,
    view::{ViewState, scroll_to_queries, use_view},
    visibility::use_page_visible,
};

const THEME: Asset = asset!("/assets/dx-components-theme.css");
const STYLESHEET: Asset = asset!("/assets/main.css");
/// For the stats and the newest page of the query log.
const FAST_POLL_INTERVAL_MS: u32 = 2_000;
/// For upstreams, lists and the system, which change slowly.
const STATUS_POLL_INTERVAL_MS: u32 = 10_000;

fn main() {
    dioxus::launch(App);
}

#[derive(Debug, Clone, PartialEq)]
enum Session {
    /// Asking the server, so neither the login form nor the dashboard flickers up.
    Checking,
    LoggedIn,
    LoggedOut {
        /// Why the session could not be checked on load, if that failed.
        check_error: Option<String>,
    },
}

#[component]
fn App() -> Element {
    let mut session = use_signal(|| Session::Checking);
    let page_visible = use_page_visible();

    use_future(move || async move {
        let checked = match api_client::check_session().await {
            Ok(()) => Session::LoggedIn,
            Err(FetchError::SessionRequired) => Session::LoggedOut { check_error: None },
            Err(FetchError::Failed(message)) => Session::LoggedOut {
                check_error: Some(message),
            },
        };
        session.set(checked);
    });

    rsx! {
        document::Title { "DNS Dashboard" }
        document::Stylesheet { href: THEME }
        document::Stylesheet { href: STYLESHEET }
        match session() {
            Session::Checking => rsx! {
                header { Brand {} }
                main {
                    p { class: "empty", role: "status", "Checking your session…" }
                }
            },
            Session::LoggedOut { check_error } => rsx! {
                header { Brand {} }
                main { class: "login-page",
                    if let Some(message) = check_error {
                        div { class: "error-banner", role: "alert", "{message}" }
                    }
                    LoginForm { on_login: move |_| session.set(Session::LoggedIn) }
                }
            },
            Session::LoggedIn => rsx! {
                DashboardPage {
                    page_visible,
                    on_logged_out: move |_| {
                        session
                            .set(Session::LoggedOut {
                                check_error: None,
                            })
                    },
                }
            },
        }
    }
}

#[component]
fn Brand() -> Element {
    rsx! {
        div { class: "brand",
            span { class: "brand-mark", "aria-hidden": "true",
                Network { size: "20px" }
            }
            span {
                "DNS"
                span { class: "brand-caption", "Dashboard" }
            }
        }
    }
}

#[component]
fn DashboardPage(page_visible: ReadSignal<bool>, on_logged_out: EventHandler<()>) -> Element {
    let mut view = use_view();
    let query_view = use_memo(move || view().query_state());
    let mut observed_clients = use_signal(|| None::<(u64, Vec<ObservedClient>)>);
    let mut clients_error = use_signal(|| None::<String>);
    let mut refreshing_lists = use_signal(|| false);
    let mut refresh_feedback = use_signal(|| None::<String>);
    let mut loaded_view = use_signal(|| None::<ViewState>);
    let mut stats = use_signal(|| None::<Stats>);
    let mut stats_error = use_signal(|| None::<String>);
    let mut server_status = use_signal(|| None::<ServerStatus>);
    let mut status_error = use_signal(|| None::<String>);
    let mut log_page = use_signal(|| None::<QueryPage>);
    let mut log_error = use_signal(|| None::<String>);
    // Separate from the others, so the next successful poll does not hide it.
    let mut logout_error = use_signal(|| None::<String>);
    let report = move |error: FetchError, mut target: Signal<Option<String>>| match error {
        FetchError::SessionRequired => on_logged_out.call(()),
        FetchError::Failed(message) => target.set(Some(message)),
    };

    // Use server-derived bounds for every drill-down; browser clock skew cannot change the range.
    use_resource(move || {
        let selected = query_view();
        let is_visible = page_visible();
        async move {
            if !is_visible {
                return;
            }
            let mut cached_selection = loaded_view.peek().clone();
            if let Some(cached) = &mut cached_selection {
                cached.until_ms = selected.until_ms;
            }
            let freeze_existing = selected.until_ms.is_some()
                && cached_selection.as_ref() == Some(&selected)
                && log_page
                    .peek()
                    .as_ref()
                    .and_then(|page| page.range)
                    .map(|range| range.until_ms)
                    == selected.until_ms;
            if freeze_existing {
                loaded_view.set(Some(selected.clone()));
            }
            if stats
                .peek()
                .as_ref()
                .is_some_and(|stats| stats.window != selected.window)
            {
                stats.set(None);
                observed_clients.set(None);
            }
            let mut read_log = !freeze_existing;
            let mut client_poll = 0;
            loop {
                let latest_stats = match api_client::fetch_stats(selected.window).await {
                    Ok(latest) => {
                        stats_error.set(None);
                        Some(latest)
                    }
                    Err(error) => {
                        report(error, stats_error);
                        None
                    }
                };
                let until_ms = latest_stats.as_ref().map(|stats| stats.range.until_ms);
                if let Some(latest) = latest_stats {
                    stats.set(Some(latest));
                }
                if read_log || selected.until_ms.is_none() {
                    match api_client::fetch_queries(&selected, until_ms).await {
                        Ok(page) if page.entries.is_empty() && selected.offset > 0 => {
                            view.with_mut(|view| view.offset = last_page_offset(page.total));
                            return;
                        }
                        Ok(page) => {
                            let cutoff = page.range.map(|range| range.until_ms);
                            log_page.set(Some(page));
                            loaded_view.set(Some(selected.clone()));
                            log_error.set(None);
                            if selected.offset > 0
                                && selected.until_ms.is_none()
                                && let Some(cutoff) = cutoff
                            {
                                view.with_mut(|view| view.until_ms = Some(cutoff));
                                return;
                            }
                            read_log = false;
                        }
                        Err(error) => report(error, log_error),
                    }
                }
                if client_poll == 0
                    && let Some(until) = until_ms
                {
                    match api_client::fetch_clients(selected.window, until).await {
                        Ok(clients) => {
                            observed_clients.set(Some((until, clients)));
                            clients_error.set(None);
                        }
                        Err(error) => report(error, clients_error),
                    }
                }
                client_poll = (client_poll + 1) % 5;
                TimeoutFuture::new(FAST_POLL_INTERVAL_MS).await;
            }
        }
    });
    use_resource(move || {
        let is_visible = page_visible();
        async move {
            if !is_visible {
                return;
            }
            loop {
                match api_client::fetch_server_status().await {
                    Ok(latest) => {
                        server_status.set(Some(latest));
                        status_error.set(None);
                    }
                    Err(error) => report(error, status_error),
                }
                TimeoutFuture::new(STATUS_POLL_INTERVAL_MS).await;
            }
        }
    });

    let refresh_lists = move |_| async move {
        if refreshing_lists() {
            return;
        }
        refreshing_lists.set(true);
        refresh_feedback.set(None);
        match api_client::refresh_blocklists().await {
            Ok(lists) => {
                let failures = lists
                    .iter()
                    .filter(|list| list.last_error.is_some())
                    .count();
                server_status.with_mut(|status| {
                    if let Some(status) = status {
                        status.blocklists = lists;
                    }
                });
                refresh_feedback.set(Some(if failures == 0 { "List refresh completed.".into() } else { format!("Refresh completed with {failures} list errors. Check the list details below.") }));
            }
            Err(FetchError::SessionRequired) => on_logged_out.call(()),
            Err(FetchError::Failed(message)) => refresh_feedback.set(Some(message)),
        }
        refreshing_lists.set(false);
    };

    let log_out = move |_| async move {
        match api_client::logout().await {
            // An expired session is as good as a closed one.
            Ok(()) | Err(FetchError::SessionRequired) => on_logged_out.call(()),
            Err(FetchError::Failed(message)) => logout_error.set(Some(message)),
        }
    };

    rsx! {
        header {
            Brand {}
            Button {
                variant: ButtonVariant::Ghost,
                size: ButtonSize::Sm,
                onclick: log_out,
                span { "aria-hidden": "true",
                    LogOut { size: "16px" }
                }
                "Log out"
            }
        }
        main {
            div { class: "page-heading",
                div {
                    p { class: "eyebrow", "YOUR NETWORK, AT A GLANCE" }
                    h1 { "Overview" }
                    p { class: "muted", "DNS activity, protection and performance." }
                }
                nav {
                    class: "window-picker",
                    "aria-label": "Dashboard time range",
                    for option in StatsWindow::ALL {
                        Button {
                            variant: if option == view().window { ButtonVariant::Secondary } else { ButtonVariant::Ghost },
                            size: ButtonSize::Sm,
                            "aria-pressed": if option == view().window { "true" } else { "false" },
                            onclick: move |_| {
                                view
                                    .with_mut(|view| {
                                    view.window = option;
                                    view.offset = 0;
                                    view.until_ms = None;
                                })
                            },
                            "{window_short_name(option)}"
                        }
                    }
                }
            }
            // Keep showing the last data we have; the banner tells the user it is stale.
            for message in [logout_error(), stats_error(), status_error(), log_error(), clients_error()]
                .into_iter()
                .flatten()
            {
                div { class: "error-banner", role: "alert", "{message}" }
            }
            if let Some(status) = server_status() {
                StatusPanel {
                    upstreams: status.upstreams,
                    lists: status.blocklists,
                    system: status.system,
                    refreshing_lists: refreshing_lists(),
                    refresh_feedback: refresh_feedback(),
                    on_refresh: refresh_lists,
                }
            }
            match stats() {
                Some(stats) => rsx! {
                    StatsOverview { stats, view }
                },
                None => rsx! {
                    p { class: "empty", "Loading…" }
                },
            }
            section { id: "queries", "aria-label": "Query investigation",
                Card { class: "query-panel",
                    CardHeader {
                        CardTitle {
                            h2 { "Query log" }
                        }
                        CardDescription {
                            "Inspect requests in the selected dashboard period. Open a domain for its recorded details."
                        }
                    }
                    CardContent {
                        QueryLog {
                            view,
                            page: log_page(),
                            updating: loaded_view().as_ref() != Some(&query_view()),
                            clients: observed_clients().map(|(_, clients)| clients).unwrap_or_default(),
                        }
                    }
                }
            }
            ObservedClients {
                clients: observed_clients().map(|(_, clients)| clients),
                window: view().window,
                on_select: move |client| {
                    if let Some((until_ms, _)) = observed_clients.peek().as_ref() {
                        drill_down(view, *until_ms, Selection::Client(client));
                    }
                },
            }
        }
    }
}

enum Selection {
    All,
    Client(IpAddr),
    Domain(String),
    BlockedDomain(String),
    Status(QueryStatus),
    Failed,
}

fn drill_down(mut view: Signal<ViewState>, until_ms: u64, selection: Selection) {
    let (client, domain, status, servfail) = match selection {
        Selection::All => (None, None, None, false),
        Selection::Client(client) => (Some(client), None, None, false),
        Selection::Domain(domain) => (None, Some(domain), None, false),
        Selection::BlockedDomain(domain) => (None, Some(domain), Some(QueryStatus::Blocked), false),
        Selection::Status(status) => (None, None, Some(status), false),
        Selection::Failed => (None, None, None, true),
    };
    let mut selected = view.peek().drilldown(client, domain, status, until_ms);
    selected.servfail = servfail;
    view.set(selected);
    scroll_to_queries();
}

#[component]
fn StatsOverview(stats: Stats, view: Signal<ViewState>) -> Element {
    let failure_rate = percentage(stats.failed_queries, stats.total_queries);
    let until_ms = stats.range.until_ms;
    let clients = stats.clients.clone();
    let domains = stats.top_domains.clone();
    let blocked_domains = stats.top_blocked_domains.clone();

    rsx! {
        section { class: "cards",
            StatCard {
                label: "Queries ({window_name(stats.window)})",
                value: "{stats.total_queries}",
                on_select: Some(EventHandler::new(move |_| drill_down(view, until_ms, Selection::All))),
            }
            StatCard {
                label: "Blocked",
                value: "{percentage(stats.blocked_queries, stats.total_queries):.1} %",
                on_select: Some(
                    EventHandler::new(move |_| drill_down(
                        view,
                        until_ms,
                        Selection::Status(QueryStatus::Blocked),
                    )),
                ),
            }
            StatCard {
                label: "Failure rate",
                value: "{failure_rate:.1} %",
                on_select: Some(EventHandler::new(move |_| drill_down(view, until_ms, Selection::Failed))),
            }
            StatCard {
                label: "Cache hit rate",
                value: "{percentage(stats.cached_queries, stats.total_queries):.1} %",
                on_select: Some(
                    EventHandler::new(move |_| drill_down(
                        view,
                        until_ms,
                        Selection::Status(QueryStatus::Cache),
                    )),
                ),
            }
            StatCard {
                label: "Average latency",
                value: "{stats.average_latency_ms:.1} ms",
            }
        }
        Card {
            CardHeader {
                CardTitle {
                    h2 { "Query activity" }
                }
                CardDescription { "Queries per {duration_name(stats.bucket_ms)}" }
            }
            CardContent {
                LineChart {
                    series: vec![
                        Series {
                            label: "All queries",
                            css_class: "total",
                            values: stats.timeline.iter().map(|b| b.queries).collect(),
                        },
                        Series {
                            label: "Blocked",
                            css_class: "blocked",
                            values: stats.timeline.iter().map(|b| b.blocked).collect(),
                        },
                        Series {
                            label: "Failed",
                            css_class: "failed",
                            values: stats.timeline.iter().map(|b| b.failed).collect(),
                        },
                    ],
                    x_labels: x_labels(stats.window),
                    start_ms: stats.range.from_ms,
                    bucket_ms: stats.bucket_ms,
                    on_select: move |index| drill_down(
                        view,
                        until_ms,
                        match index {
                            1 => Selection::Status(QueryStatus::Blocked),
                            2 => Selection::Failed,
                            _ => Selection::All,
                        },
                    ),
                }
            }
        }
        section { class: "panels",
            Card {
                CardHeader {
                    CardTitle {
                        h2 { "Top clients" }
                    }
                }
                CardContent {
                    BarList {
                        bars: stats
                            .clients
                            .iter()
                            .map(|c| Bar {
                                label: client_name(c.client, c.name.as_deref()),
                                value: c.queries,
                            })
                            .collect(),
                        empty_text: "No queries yet",
                        on_select: move |index: usize| drill_down(
                            view,
                            until_ms,
                            Selection::Client(clients[index].client),
                        ),
                    }
                }
            }
            Card {
                CardHeader {
                    CardTitle {
                        h2 { "Top domains" }
                    }
                }
                CardContent {
                    BarList {
                        bars: stats
                            .top_domains
                            .iter()
                            .map(|d| Bar {
                                label: domain_name(&d.domain).to_owned(),
                                value: d.queries,
                            })
                            .collect(),
                        empty_text: "No queries yet",
                        on_select: move |index: usize| drill_down(
                            view,
                            until_ms,
                            Selection::Domain(domains[index].domain.clone()),
                        ),
                    }
                }
            }
            Card {
                CardHeader {
                    CardTitle {
                        h2 { "Top blocked domains" }
                    }
                }
                CardContent {
                    BarList {
                        bars: stats
                            .top_blocked_domains
                            .iter()
                            .map(|d| Bar {
                                label: domain_name(&d.domain).to_owned(),
                                value: d.queries,
                            })
                            .collect(),
                        empty_text: "Nothing blocked yet",
                        on_select: move |index: usize| drill_down(
                            view,
                            until_ms,
                            Selection::BlockedDomain(blocked_domains[index].domain.clone()),
                        ),
                    }
                }
            }
        }
    }
}

#[component]
fn StatCard(
    label: String,
    value: String,
    #[props(default)] on_select: Option<EventHandler<()>>,
) -> Element {
    rsx! {
        Card { class: "stat-card",
            CardHeader {
                if let Some(on_select) = on_select {
                    button {
                        class: "stat-action",
                        r#type: "button",
                        "aria-label": "Show {label} queries",
                        onclick: move |_| on_select.call(()),
                        span { class: "muted", "{label}" }
                        span { class: "card-value", "{value}" }
                    }
                } else {
                    CardDescription { "{label}" }
                    p { class: "card-value", "{value}" }
                }
            }
        }
    }
}
