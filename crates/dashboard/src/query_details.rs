use api::{AnswerSource, QueryLogEntry, QueryOutcome};
use dioxus::prelude::*;
use wasm_bindgen::JsCast;

use crate::components::button::{Button, ButtonSize, ButtonVariant};

fn dialog() -> Option<web_sys::HtmlDialogElement> {
    web_sys::window()?
        .document()?
        .get_element_by_id("query-details")?
        .dyn_into()
        .ok()
}

#[component]
pub fn QueryDetails(entry: QueryLogEntry, on_close: EventHandler<()>) -> Element {
    let timestamp = js_sys::Date::new(&(entry.timestamp_ms as f64).into()).to_iso_string();
    rsx! {
        dialog {
            id: "query-details",
            "aria-labelledby": "query-details-title",
            onmounted: move |_| {
                if let Some(dialog) = dialog() {
                    let _ = dialog.show_modal();
                }
            },
            oncancel: move |_| {
                if let Some(dialog) = dialog() {
                    dialog.close();
                }
                on_close.call(());
            },
            div { class: "details-heading",
                h2 { id: "query-details-title", "Query details" }
                Button {
                    variant: ButtonVariant::Ghost,
                    onclick: move |_| {
                        if let Some(dialog) = dialog() {
                            dialog.close();
                        }
                        on_close.call(());
                    },
                    "Close"
                }
            }
            dl { class: "query-details",
                dt { "Domain" }
                dd {
                    CopyValue { value: entry.domain.clone(), label: "domain" }
                }
                dt { "Client IP" }
                dd {
                    CopyValue { value: entry.client.to_string(), label: "client IP" }
                }
                if let Some(name) = entry.client_name {
                    dt { "Client name" }
                    dd { "{name}" }
                }
                dt { "Timestamp (UTC)" }
                dd { "{timestamp}" }
                dt { "Record type / transport" }
                dd { "{entry.record_type} / {entry.protocol.to_uppercase()}" }
                dt { "DNS response code" }
                dd { "{entry.response_code}" }
                dt { "Latency" }
                dd { "{entry.latency_ms:.1} ms" }
                match entry.outcome {
                    QueryOutcome::Blocked { list } => rsx! {
                        dt { "Decision" }
                        dd { "Blocked by configured filtering" }
                        dt { "Recorded source" }
                        dd {
                            CopyValue { value: list, label: "source" }
                        }
                    },
                    QueryOutcome::Answered { source: AnswerSource::Cache } => rsx! {
                        dt { "Source" }
                        dd { "DNS cache (not evidence of an allowlist match)" }
                    },
                    QueryOutcome::Answered { source: AnswerSource::Upstream { upstream } } => {
                        rsx! {
                            dt { "Upstream" }
                            dd {
                                CopyValue { value: upstream, label: "upstream" }
                            }
                        }
                    }
                    QueryOutcome::UpstreamFailed { error } => rsx! {
                        dt { "Error code" }
                        dd { "{error.code}" }
                        dt { "Error" }
                        dd { "{error.message}" }
                    },
                }
            }
            p { class: "muted",
                "Recorded query result. Exact matching rules, answer records and TTLs are not retained."
            }
        }
    }
}

#[component]
fn CopyValue(value: String, label: String) -> Element {
    let mut feedback = use_signal(String::new);
    rsx! {
        div { class: "copy-value",
            code { "{value}" }
            Button {
                variant: ButtonVariant::Outline,
                size: ButtonSize::Sm,
                "aria-label": "Copy {label}",
                onclick: move |_| {
                    let value = value.clone();
                    async move {
                        feedback
                            .set(
                                match copy_text(value).await {
                                    true => "Copied".into(),
                                    false => {
                                        "Copy unavailable. Select and copy the value manually."
                                            .into()
                                    }
                                },
                            );
                    }
                },
                "Copy"
            }
            span { class: "copy-feedback muted", role: "status", "{feedback}" }
        }
    }
}

async fn copy_text(value: String) -> bool {
    let evaluation = document::eval(
        r#"
        const text = await dioxus.recv();
        try {
            if (navigator.clipboard) {
                await navigator.clipboard.writeText(text);
                return true;
            }
        } catch (_) {}
        const focused = document.activeElement;
        const input = document.createElement('textarea');
        input.value = text;
        input.style.cssText = 'position:fixed;left:-9999px';
        (document.querySelector('dialog[open]') ?? document.body).appendChild(input);
        let copied = false;
        try {
            input.focus();
            input.select();
            copied = document.execCommand('copy');
        } catch (_) {}
        input.remove();
        focused?.focus();
        return copied;
    "#,
    );
    if evaluation.send(value).is_err() {
        return false;
    }
    evaluation
        .await
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}
