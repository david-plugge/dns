//! Health and latency of the configured upstreams.

use api::{UpstreamHealth, UpstreamStatus};
use dioxus::prelude::*;

use crate::{
    components::badge::{Badge, BadgeVariant},
    status::LastErrorCell,
};

#[component]
pub fn UpstreamTable(upstreams: Vec<UpstreamStatus>) -> Element {
    rsx! {
        div { class: "table-scroll",
            table { class: "stack",
                caption { class: "sr-only", "Upstream health and performance" }
                thead {
                    tr {
                        th { "Upstream" }
                        th { "Status" }
                        th { class: "numeric", "Latency" }
                        th { class: "numeric", "Queries" }
                        th { class: "numeric", "Failures" }
                        th { "Last error" }
                    }
                }
                tbody {
                    for upstream in upstreams.iter() {
                        tr {
                            td { class: "domain", "{upstream.upstream}" }
                            td { "data-label": "Status",
                                Badge {
                                    variant: BadgeVariant::Outline,
                                    class: "{health_class(upstream.health)}",
                                    "{health_label(upstream.health)}"
                                }
                            }
                            td { class: "numeric", "data-label": "Latency",
                                match upstream.smoothed_latency_ms {
                                    Some(latency) => rsx! { "{latency:.1} ms" },
                                    None => rsx! { "–" },
                                }
                            }
                            td { class: "numeric", "data-label": "Queries", "{upstream.queries}" }
                            td { class: "numeric", "data-label": "Failures", "{upstream.failures}" }
                            LastErrorCell { error: upstream.last_error.clone() }
                        }
                    }
                }
            }
        }
    }
}

fn health_label(health: UpstreamHealth) -> &'static str {
    match health {
        UpstreamHealth::Unknown => "unknown",
        UpstreamHealth::Healthy => "healthy",
        UpstreamHealth::Degraded => "degraded",
        UpstreamHealth::Down => "down",
    }
}

fn health_class(health: UpstreamHealth) -> &'static str {
    match health {
        UpstreamHealth::Unknown => "badge-muted",
        UpstreamHealth::Healthy => "badge-ok",
        UpstreamHealth::Degraded => "badge-warn",
        UpstreamHealth::Down => "badge-error",
    }
}
