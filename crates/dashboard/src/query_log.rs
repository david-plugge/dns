//! The query log: filters, the table of queries and paging.

use api::{AnswerSource, ObservedClient, QueryLogEntry, QueryOutcome, QueryPage, QueryStatus};
use dioxus::{core::Task, prelude::*};
use gloo_timers::future::TimeoutFuture;

use crate::{
    api_client::PAGE_SIZE,
    client_filter::ClientFilter,
    components::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonSize, ButtonVariant},
        input::Input,
    },
    format::{LocalDate, client_name, domain_name, local_time, window_name},
    query_details::QueryDetails,
    view::ViewState,
};

const DOMAIN_FILTER_DELAY_MS: u32 = 300;

#[component]
pub fn QueryLog(
    mut view: Signal<ViewState>,
    page: Option<QueryPage>,
    updating: bool,
    clients: Vec<ObservedClient>,
) -> Element {
    let mut client_filter_reset = use_signal(|| 0_u64);
    let mut domain_text = use_signal(|| view.peek().domain.clone().unwrap_or_default());
    let mut pending_domain_filter = use_signal(|| None::<Task>);
    let mut selected = use_signal(|| None::<QueryLogEntry>);
    use_effect(move || {
        let query = view();
        if let Some(task) = pending_domain_filter.write().take() {
            task.cancel();
        }
        domain_text.set(query.domain.unwrap_or_default());
    });
    let commit_domain = use_callback(move |()| {
        if let Some(task) = pending_domain_filter.write().take() {
            task.cancel();
        }
        let text = domain_text.peek().trim().to_owned();
        let domain = (!text.is_empty()).then_some(text);
        view.with_mut(|query| {
            if query.domain != domain {
                query.domain = domain;
                query.offset = 0;
            }
        });
    });
    let query = view();
    let until_ms = page
        .as_ref()
        .and_then(|page| page.range)
        .map(|range| range.until_ms);
    let today = LocalDate::today();

    rsx! {
        div { class: "log-toolbar",
            div {
                p { "Queries: {window_name(query.window)}" }
                if let Some(range) = page.as_ref().and_then(|page| page.range) {
                    p { class: "muted",
                        "{local_time(range.from_ms, today)} – {local_time(range.until_ms, today)}"
                    }
                }
            }
            Button {
                variant: ButtonVariant::Outline,
                size: ButtonSize::Sm,
                disabled: updating || (until_ms.is_none() && query.until_ms.is_none()),
                onclick: move |_| {
                    view
                        .with_mut(|view| {
                        view.until_ms = if view.until_ms.is_some() { None } else { until_ms };
                        view.offset = 0;
                    })
                },
                if query.until_ms.is_some() {
                    "Resume live log"
                } else {
                    "Pause live log"
                }
            }
        }
        p { class: "muted log-hint", role: "status",
            if query.until_ms.is_some() {
                "Log snapshot — automatic log updates paused. "
            } else {
                "Live log. "
            }
            "DNS service and filtering are unchanged."
        }
        div { class: "log-filters",
            ClientFilter {
                client: query.client,
                clients: clients.clone(),
                reset: client_filter_reset(),
                on_select: move |client| {
                    commit_domain.call(());
                    view.with_mut(|query| {
                        query.client = client;
                        query.offset = 0;
                    });
                },
            }
            div { class: "domain-filter",
                select {
                    "aria-label": "Domain match",
                    value: if query.domain_exact { "exact" } else { "contains" },
                    onchange: move |event: FormEvent| {
                        commit_domain.call(());
                        view.with_mut(|query| {
                            let exact = event.value() == "exact";
                            if query.domain.is_some() && query.domain_exact != exact {
                                query.offset = 0;
                            }
                            query.domain_exact = exact;
                        });
                    },
                    option { value: "contains", "Contains" }
                    option { value: "exact", "Exact" }
                }
                Input {
                    value: "{domain_text}",
                    placeholder: if query.domain_exact { "Exact domain" } else { "Domain contains" },
                    "aria-label": "Domain",
                    oninput: move |event: FormEvent| {
                        let text = event.value();
                        domain_text.set(text.clone());
                        let domain = (!text.trim().is_empty()).then(|| text.trim().to_owned());
                        if let Some(task) = pending_domain_filter.write().take() {
                            task.cancel();
                        }
                        pending_domain_filter
                            .set(
                                Some(
                                    spawn(async move {
                                        TimeoutFuture::new(DOMAIN_FILTER_DELAY_MS).await;
                                        view.with_mut(|query| {
                                            query.domain = domain;
                                            query.offset = 0;
                                        });
                                    }),
                                ),
                            );
                    },
                }
            }
            select {
                "aria-label": "Result",
                value: query.status.map(|status| status.as_str()).unwrap_or_default(),
                onchange: move |event: FormEvent| {
                    commit_domain.call(());
                    let status = QueryStatus::ALL
                        .into_iter()
                        .find(|status| status.as_str() == event.value());
                    view.with_mut(|query| {
                        query.status = status;
                        query.offset = 0;
                    });
                },
                option { value: "", "All results" }
                for status in QueryStatus::ALL {
                    option { value: status.as_str(), "{status_label(status)}" }
                }
            }
            select {
                "aria-label": "DNS response",
                value: if query.servfail { "SERVFAIL" } else { "" },
                onchange: move |event: FormEvent| {
                    commit_domain.call(());
                    view.with_mut(|query| {
                        query.servfail = event.value() == "SERVFAIL";
                        query.offset = 0;
                    });
                },
                option { value: "", "All DNS responses" }
                option { value: "SERVFAIL", "SERVFAIL" }
            }
            Button {
                class: "filter-reset",
                variant: ButtonVariant::Ghost,
                size: ButtonSize::Sm,
                onclick: move |_| {
                    if let Some(task) = pending_domain_filter.write().take() {
                        task.cancel();
                    }
                    client_filter_reset += 1;
                    domain_text.set(String::new());
                    view.with_mut(ViewState::clear_filters);
                },
                "Reset filters"
            }
        }
        match page {
            None => rsx! {
                p { class: "empty", role: "status", "Loading queries…" }
            },
            Some(page) if page.entries.is_empty() => rsx! {
                p { class: "empty", role: "status",
                    if updating {
                        "Updating queries…"
                    } else {
                        "No queries match this period and these filters. Clear filters or choose a wider period."
                    }
                }
            },
            Some(page) => rsx! {
                p { class: "muted result-count", role: "status",
                    if updating {
                        "Updating queries…"
                    } else {
                        "{page.total} matching queries"
                    }
                }
                div {
                    class: "query-results",
                    "aria-busy": if updating { "true" } else { "false" },
                    inert: updating.then_some(""),
                    QueryTable {
                        entries: page.entries.clone(),
                        on_select: move |entry| selected.set(Some(entry)),
                    }
                    Pager {
                        view,
                        shown: page.entries.len(),
                        total: page.total,
                        until_ms,
                    }
                }
            },
        }
        if let Some(entry) = selected() {
            QueryDetails { entry, on_close: move |_| selected.set(None) }
        }
    }
}

#[component]
fn Pager(mut view: Signal<ViewState>, shown: usize, total: u64, until_ms: Option<u64>) -> Element {
    let offset = view.read().offset;
    let last_shown = offset.saturating_add(shown);
    rsx! {
        div { class: "pager",
            Button {
                variant: ButtonVariant::Outline,
                size: ButtonSize::Sm,
                disabled: offset == 0,
                onclick: move |_| {
                    view
                        .with_mut(|query| {
                        query.offset = query.offset.saturating_sub(PAGE_SIZE);
                        query.until_ms = query.until_ms.or(until_ms);
                    })
                },
                "Newer"
            }
            span { "{offset.saturating_add(1)}–{last_shown} of {total}" }
            Button {
                variant: ButtonVariant::Outline,
                size: ButtonSize::Sm,
                disabled: last_shown as u64 >= total,
                onclick: move |_| {
                    view
                        .with_mut(|query| {
                        query.offset = query.offset.saturating_add(PAGE_SIZE);
                        query.until_ms = query.until_ms.or(until_ms);
                    })
                },
                "Older"
            }
        }
    }
}

#[component]
fn QueryTable(entries: Vec<QueryLogEntry>, on_select: EventHandler<QueryLogEntry>) -> Element {
    let today = LocalDate::today();
    rsx! {
        div { class: "table-scroll",
            table { class: "query-table",
                caption { class: "sr-only", "DNS query log" }
                thead {
                    tr {
                        th { "Time" }
                        th { "Client" }
                        th { "Domain / details" }
                        th { "Type" }
                        th { "Protocol" }
                        th { "Result" }
                        th { "Source" }
                        th { class: "numeric", "Latency" }
                    }
                }
                tbody {
                    for entry in entries.iter() {
                        tr {
                            td { class: "time", "{local_time(entry.timestamp_ms, today)}" }
                            td { class: "client", title: "{entry.client}",
                                "{client_name(entry.client, entry.client_name.as_deref())}"
                            }
                            td { class: "domain",
                                button {
                                    class: "text-link",
                                    r#type: "button",
                                    "aria-label": "Details for {entry.domain}",
                                    onclick: {
                                        let entry = entry.clone();
                                        move |_| on_select.call(entry.clone())
                                    },
                                    "{domain_name(&entry.domain)}"
                                }
                            }
                            td { class: "type", "{entry.record_type}" }
                            td { class: "protocol", "{entry.protocol.to_uppercase()}" }
                            td { class: "result {result_class(&entry.outcome, &entry.response_code)}",
                                Badge {
                                    variant: BadgeVariant::Outline,
                                    class: "query-result {result_class(&entry.outcome, &entry.response_code)}",
                                    "{result_label(entry)}"
                                }
                            }
                            td { class: "source", "{answer_source(&entry.outcome)}" }
                            td { class: "latency numeric", "{entry.latency_ms:.1} ms" }
                        }
                    }
                }
            }
        }
    }
}

pub fn status_label(status: QueryStatus) -> &'static str {
    match status {
        QueryStatus::Cache => "Cache",
        QueryStatus::Upstream => "Upstream",
        QueryStatus::UpstreamFailed => "Upstream failed",
        QueryStatus::Blocked => "Blocked",
    }
}

fn answer_source(outcome: &QueryOutcome) -> &str {
    match outcome {
        QueryOutcome::Answered {
            source: AnswerSource::Cache,
        } => "cache",
        QueryOutcome::Answered {
            source: AnswerSource::Upstream { upstream },
        } => upstream,
        QueryOutcome::UpstreamFailed { .. } => "–",
        QueryOutcome::Blocked { list } => list,
    }
}

fn result_label(entry: &QueryLogEntry) -> &str {
    match entry.outcome {
        QueryOutcome::Blocked { .. } => "BLOCKED",
        _ => &entry.response_code,
    }
}

fn result_class(outcome: &QueryOutcome, response_code: &str) -> &'static str {
    match outcome {
        QueryOutcome::Answered { .. } => {
            if response_code == "SERVFAIL" {
                "failed"
            } else {
                ""
            }
        }
        QueryOutcome::UpstreamFailed { .. } => "failed",
        QueryOutcome::Blocked { .. } => "blocked",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_servfail_is_visually_distinct_from_a_successful_answer() {
        let answered = QueryOutcome::Answered {
            source: AnswerSource::Cache,
        };
        assert_eq!(result_class(&answered, "SERVFAIL"), "failed");
        assert_eq!(result_class(&answered, "NOERROR"), "");
        assert_eq!(result_class(&answered, "NXDOMAIN"), "");
        let blocked = QueryOutcome::Blocked {
            list: "configured-list".into(),
        };
        assert_eq!(result_class(&blocked, "NOERROR"), "blocked");
    }
}
