mod api_client;
mod blocklists;
mod charts;
mod format;
mod login;
mod query_log;
mod status;
mod upstreams;
mod visibility;

use api::{QueryPage, Stats, StatsWindow};
use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;

use crate::{
    api_client::{FetchError, LogQuery, ServerStatus, last_page_offset},
    charts::{Bar, BarList, LineChart, Series},
    format::{
        client_name, domain_name, duration_name, percentage, window_name, window_short_name,
        x_labels,
    },
    login::LoginForm,
    query_log::QueryLog,
    status::StatusPanel,
    visibility::use_page_visible,
};

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
        document::Stylesheet { href: STYLESHEET }
        match session() {
            Session::Checking => rsx! {
                header { h1 { "DNS Dashboard" } }
            },
            Session::LoggedOut { check_error } => rsx! {
                header { h1 { "DNS Dashboard" } }
                main {
                    if let Some(message) = check_error {
                        div { class: "error-banner", role: "alert", "{message}" }
                    }
                    LoginForm { on_login: move |_| session.set(Session::LoggedIn) }
                }
            },
            // Mounted only while logged in, so no data outlives the session.
            Session::LoggedIn => rsx! {
                DashboardPage {
                    page_visible,
                    on_logged_out: move |_| session.set(Session::LoggedOut { check_error: None }),
                }
            },
        }
    }
}

#[component]
fn DashboardPage(page_visible: ReadSignal<bool>, on_logged_out: EventHandler<()>) -> Element {
    let mut window = use_signal(StatsWindow::default);
    let mut log_query = use_signal(LogQuery::default);
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

    // Each poll fetches, waits for the answer and only then sleeps, so slow answers are never
    // cancelled by the next poll. Resources restart when a signal read in them changes: on user
    // input and when the page becomes visible again. While it is hidden, they stop.
    use_resource(move || {
        let window = window();
        let is_visible = page_visible();
        async move {
            if !is_visible {
                return;
            }
            loop {
                match api_client::fetch_stats(window).await {
                    Ok(latest) => {
                        stats.set(Some(latest));
                        stats_error.set(None);
                    }
                    Err(error) => report(error, stats_error),
                }
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
    use_resource(move || {
        let query = log_query();
        let is_visible = page_visible();
        async move {
            if !is_visible {
                return;
            }
            loop {
                match api_client::fetch_queries(&query).await {
                    Ok(page) if page.entries.is_empty() && query.offset > 0 => {
                        // The page lies past the end, e.g. after old queries expired.
                        log_query.with_mut(|query| query.offset = last_page_offset(page.total));
                        return;
                    }
                    Ok(page) => {
                        log_page.set(Some(page));
                        log_error.set(None);
                        // Older pages stay as they are, so new queries do not shift them while reading.
                        if query.offset > 0 {
                            return;
                        }
                    }
                    Err(error) => report(error, log_error),
                }
                TimeoutFuture::new(FAST_POLL_INTERVAL_MS).await;
            }
        }
    });

    let log_out = move |_| async move {
        match api_client::logout().await {
            // An expired session is as good as a closed one.
            Ok(()) | Err(FetchError::SessionRequired) => on_logged_out.call(()),
            Err(FetchError::Failed(message)) => logout_error.set(Some(message)),
        }
    };

    rsx! {
        header {
            h1 { "DNS Dashboard" }
            nav { class: "window-picker",
                for option in StatsWindow::ALL {
                    button {
                        class: if option == window() { "active" },
                        "aria-pressed": if option == window() { "true" } else { "false" },
                        onclick: move |_| window.set(option),
                        "{window_short_name(option)}"
                    }
                }
            }
            button {
                class: "icon",
                title: "Log out",
                "aria-label": "Log out",
                onclick: log_out,
                // The "log-out" icon from Feather (MIT).
                svg {
                    view_box: "0 0 24 24",
                    width: "18",
                    height: "18",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "2",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    "aria-hidden": "true",
                    path { d: "M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" }
                    polyline { points: "16 17 21 12 16 7" }
                    line { x1: "21", y1: "12", x2: "9", y2: "12" }
                }
            }
        }
        main {
            // Keep showing the last data we have; the banner tells the user it is stale.
            for message in [logout_error(), stats_error(), status_error(), log_error()].into_iter().flatten() {
                div { class: "error-banner", role: "alert", "{message}" }
            }
            if let Some(status) = server_status() {
                StatusPanel {
                    upstreams: status.upstreams,
                    lists: status.blocklists,
                    system: status.system,
                }
            }
            match stats() {
                Some(stats) => rsx! { StatsOverview { stats } },
                None => rsx! { p { class: "empty", "Loading…" } },
            }
            section { class: "panel",
                h2 { "Query log" }
                QueryLog { query: log_query, page: log_page() }
            }
        }
    }
}

#[component]
fn StatsOverview(stats: Stats) -> Element {
    let failure_rate = percentage(stats.failed_queries, stats.total_queries);

    rsx! {
        section { class: "cards",
            StatCard { label: "Queries ({window_name(stats.window)})", value: "{stats.total_queries}" }
            StatCard { label: "Blocked", value: "{percentage(stats.blocked_queries, stats.total_queries):.1} %" }
            StatCard { label: "Failure rate", value: "{failure_rate:.1} %" }
            StatCard { label: "Cache hit rate", value: "{percentage(stats.cached_queries, stats.total_queries):.1} %" }
            StatCard { label: "Average latency", value: "{stats.average_latency_ms:.1} ms" }
        }
        section { class: "panel",
            h2 { "Queries per {duration_name(stats.bucket_ms)}" }
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
            }
        }
        section { class: "panels",
            div { class: "panel",
                h2 { "Top clients" }
                BarList {
                    bars: stats.clients.iter().map(|c| Bar { label: client_name(c.client, c.name.as_deref()), value: c.queries }).collect(),
                    empty_text: "No queries yet",
                }
            }
            div { class: "panel",
                h2 { "Top domains" }
                BarList {
                    bars: stats.top_domains.iter().map(|d| Bar { label: domain_name(&d.domain).to_owned(), value: d.queries }).collect(),
                    empty_text: "No queries yet",
                }
            }
            div { class: "panel",
                h2 { "Top blocked domains" }
                BarList {
                    bars: stats.top_blocked_domains.iter().map(|d| Bar { label: domain_name(&d.domain).to_owned(), value: d.queries }).collect(),
                    empty_text: "Nothing blocked yet",
                }
            }
        }
    }
}

#[component]
fn StatCard(label: String, value: String) -> Element {
    rsx! {
        div { class: "card",
            span { class: "card-label", "{label}" }
            span { class: "card-value", "{value}" }
        }
    }
}
