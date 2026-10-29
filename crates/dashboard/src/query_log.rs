//! The query log: filters, the table of queries and paging.

use std::net::IpAddr;

use api::{AnswerSource, QueryLogEntry, QueryOutcome, QueryPage, QueryStatus};
use dioxus::{core::Task, prelude::*};
use gloo_timers::future::TimeoutFuture;

use crate::{
    api_client::{LogQuery, PAGE_SIZE},
    format::{LocalDate, client_name, domain_name, local_time},
};

/// How long the domain filter waits for typing to pause, so not every keystroke fetches a page.
const DOMAIN_FILTER_DELAY_MS: u32 = 300;

#[component]
pub fn QueryLog(query: Signal<LogQuery>, page: Option<QueryPage>) -> Element {
    let mut client_is_valid = use_signal(|| true);
    let mut pending_domain_filter = use_signal(|| None::<Task>);

    rsx! {
        div { class: "log-filters",
            input {
                class: if !client_is_valid() { "invalid" },
                placeholder: "Client IP",
                "aria-label": "Client IP",
                oninput: move |event| {
                    let text = event.value();
                    let text = text.trim();
                    let client = if text.is_empty() { None } else { text.parse::<IpAddr>().ok() };
                    let is_valid = text.is_empty() || client.is_some();
                    client_is_valid.set(is_valid);
                    // Keep the last valid filter while the user is still typing an address.
                    if is_valid {
                        query.with_mut(|query| {
                            query.client = client;
                            query.offset = 0;
                        });
                    }
                },
            }
            input {
                placeholder: "Domain contains",
                "aria-label": "Domain contains",
                oninput: move |event| {
                    let text = event.value().trim().to_owned();
                    let domain = (!text.is_empty()).then_some(text);
                    if let Some(task) = pending_domain_filter.write().take() {
                        task.cancel();
                    }
                    let task = spawn(async move {
                        TimeoutFuture::new(DOMAIN_FILTER_DELAY_MS).await;
                        query.with_mut(|query| {
                            query.domain = domain;
                            query.offset = 0;
                        });
                    });
                    pending_domain_filter.set(Some(task));
                },
            }
            select {
                "aria-label": "Result",
                onchange: move |event| {
                    let status = QueryStatus::ALL.into_iter().find(|status| status.as_str() == event.value());
                    query.with_mut(|query| {
                            query.status = status;
                            query.offset = 0;
                        });
                },
                option { value: "", "All results" }
                for status in QueryStatus::ALL {
                    option { value: status.as_str(), "{status_label(status)}" }
                }
            }
        }
        match page {
            None => rsx! { p { class: "empty", "Loading…" } },
            Some(page) if page.entries.is_empty() => rsx! { p { class: "empty", "No matching queries" } },
            Some(page) => rsx! {
                QueryTable { entries: page.entries.clone() }
                Pager { query, shown: page.entries.len(), total: page.total }
            },
        }
    }
}

#[component]
fn Pager(query: Signal<LogQuery>, shown: usize, total: u64) -> Element {
    let offset = query.read().offset;
    let last_shown = offset + shown;

    rsx! {
        div { class: "pager",
            button {
                disabled: offset == 0,
                onclick: move |_| query.with_mut(|query| query.offset = query.offset.saturating_sub(PAGE_SIZE)),
                "Newer"
            }
            span { "{offset + 1}–{last_shown} of {total}" }
            button {
                disabled: last_shown as u64 >= total,
                onclick: move |_| query.with_mut(|query| query.offset += PAGE_SIZE),
                "Older"
            }
        }
    }
}

#[component]
fn QueryTable(entries: Vec<QueryLogEntry>) -> Element {
    let today = LocalDate::today();

    rsx! {
        table { class: "query-table",
            thead {
                tr {
                    th { "Time" }
                    th { "Client" }
                    th { "Domain" }
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
                        td { class: "domain", "{domain_name(&entry.domain)}" }
                        td { class: "type", "{entry.record_type}" }
                        td { class: "protocol", "{entry.protocol.to_uppercase()}" }
                        td {
                            class: "result {result_class(&entry.outcome)}",
                            title: if let QueryOutcome::UpstreamFailed { error } = &entry.outcome { "{error.message}" },
                            "{result_label(entry)}"
                        }
                        td { class: "source", "{answer_source(&entry.outcome)}" }
                        td { class: "latency numeric", "{entry.latency_ms:.1} ms" }
                    }
                }
            }
        }
    }
}

fn status_label(status: QueryStatus) -> &'static str {
    match status {
        QueryStatus::Cache => "Cache",
        QueryStatus::Upstream => "Upstream",
        QueryStatus::UpstreamFailed => "Failed",
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

fn result_class(outcome: &QueryOutcome) -> &'static str {
    match outcome {
        QueryOutcome::Answered { .. } => "",
        QueryOutcome::UpstreamFailed { .. } => "failed",
        QueryOutcome::Blocked { .. } => "blocked",
    }
}
