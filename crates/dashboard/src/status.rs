//! One line summarizing the health of the server, with upstreams and lists unfolding below.

use api::{BlocklistStatus, ErrorReport, SystemStatus, UpstreamHealth, UpstreamStatus};
use dioxus::prelude::*;

use crate::{blocklists::BlocklistTable, upstreams::UpstreamTable};

#[component]
pub fn StatusPanel(
    upstreams: Vec<UpstreamStatus>,
    lists: Vec<BlocklistStatus>,
    system: SystemStatus,
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
        details { class: "panel status",
            summary { class: if is_healthy { "healthy" } else { "unhealthy" },
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
            BlocklistTable { lists }
        }
    }
}

/// The last error of an upstream or list; its code shows on hover.
#[component]
pub fn LastErrorCell(error: Option<ErrorReport>) -> Element {
    let Some(error) = error else {
        return rsx! { td { class: "muted", "data-label": "Last error", "–" } };
    };
    rsx! {
        td { class: "muted", "data-label": "Last error", title: "{error.code}", "{error.message}" }
    }
}
