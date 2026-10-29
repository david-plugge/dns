//! State of the configured block- and allowlists.

use api::{BlocklistStatus, ListKind};
use dioxus::prelude::*;

use crate::{
    format::{LocalDate, local_time},
    status::LastErrorCell,
};

#[component]
pub fn BlocklistTable(lists: Vec<BlocklistStatus>) -> Element {
    if lists.is_empty() {
        return rsx! { p { class: "empty", "No lists configured" } };
    }
    let today = LocalDate::today();

    rsx! {
        table { class: "stack",
            thead {
                tr {
                    th { "List" }
                    th { "Kind" }
                    th { "Status" }
                    th { class: "numeric", "Domains" }
                    th { class: "numeric", "Unsupported lines" }
                    th { "Updated" }
                    th { "Last error" }
                }
            }
            tbody {
                for list in lists.iter() {
                    tr {
                        td { class: "domain", "{list.list}" }
                        td { "data-label": "Kind", if list.kind == ListKind::Block { "block" } else { "allow" } }
                        td { "data-label": "Status",
                            if list.last_error.is_some() {
                                span { class: "badge badge-error", "error" }
                            } else {
                                span { class: "badge badge-ok", "ok" }
                            }
                        }
                        td { class: "numeric", "data-label": "Domains", "{list.domains}" }
                        td { class: "numeric", "data-label": "Unsupported lines", "{list.unsupported_lines}" }
                        td { "data-label": "Updated", "{list.updated_at_ms.map(|updated| local_time(updated, today)).unwrap_or_else(|| \"–\".to_owned())}" }
                        LastErrorCell { error: list.last_error.clone() }
                    }
                }
            }
        }
    }
}
