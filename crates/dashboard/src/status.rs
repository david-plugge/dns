//! One line summarizing the health of the server, with upstreams and lists unfolding below.

use api::{BlocklistStatus, ErrorReport, SystemStatus, UpstreamHealth, UpstreamStatus};
use dioxus::prelude::*;

use crate::{
    blocklists::BlocklistTable,
    components::{
        badge::{Badge, BadgeVariant},
        button::{Button, ButtonSize, ButtonVariant},
        card::{Card, CardContent},
    },
    upstreams::UpstreamTable,
};

#[component]
pub fn StatusPanel(
    upstreams: Vec<UpstreamStatus>,
    lists: Vec<BlocklistStatus>,
    system: SystemStatus,
    refreshing_lists: bool,
    refresh_feedback: Option<String>,
    on_refresh: EventHandler<MouseEvent>,
) -> Element {
    // Unknown upstreams have not been asked yet, which is no reason to worry.
    let working_upstreams = upstreams
        .iter()
        .filter(|upstream| {
            matches!(
                upstream.health,
                UpstreamHealth::Healthy | UpstreamHealth::Unknown
            )
        })
        .count();
    let failing_lists = lists
        .iter()
        .filter(|list| list.last_error.is_some())
        .count();
    let is_healthy = working_upstreams == upstreams.len() && failing_lists == 0;

    rsx! {
        Card { class: "status-card",
            CardContent {
                details { class: "status",
                    summary { class: if is_healthy { "healthy" } else { "unhealthy" },
                        Badge {
                            variant: BadgeVariant::Outline,
                            class: if is_healthy { "badge-ok" } else { "badge-warn" },
                            if is_healthy {
                                "System healthy"
                            } else {
                                "Needs attention"
                            }
                        }
                        span { "{working_upstreams}/{upstreams.len()} upstreams ok" }
                        if failing_lists == 0 {
                            span { "{lists.len()} lists ok" }
                        } else {
                            span { "{failing_lists}/{lists.len()} lists failing" }
                        }
                        if let Some(bytes) = system.resident_memory_bytes {
                            span { "{bytes / 1_000_000} MB memory" }
                        }
                    }
                    h2 { "Upstreams" }
                    UpstreamTable { upstreams }
                    h2 { "Lists" }
                    p { class: "muted log-hint",
                        "Sources and rules are managed by deployment configuration. Refresh reloads the existing sources and their cache; it does not change configuration."
                    }
                    Button {
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Sm,
                        disabled: refreshing_lists,
                        onclick: on_refresh,
                        if refreshing_lists {
                            "Refreshing lists…"
                        } else {
                            "Refresh lists"
                        }
                    }
                    if let Some(message) = refresh_feedback {
                        p { class: "log-hint", role: "status", "{message}" }
                    }
                    BlocklistTable { lists }
                }
            }
        }
    }
}

/// The last error of an upstream or list; its code shows on hover.
#[component]
pub fn LastErrorCell(error: Option<ErrorReport>) -> Element {
    let Some(error) = error else {
        return rsx! {
            td { class: "muted", "data-label": "Last error", "–" }
        };
    };
    rsx! {
        td {
            class: "muted",
            "data-label": "Last error",
            title: "{error.code}",
            "{error.message}"
        }
    }
}
