//! State of the configured block- and allowlists.

use api::{BlocklistStatus, ListKind};
use dioxus::prelude::*;

use crate::{
    components::badge::{Badge, BadgeVariant},
    format::{LocalDate, local_time},
    status::LastErrorCell,
};

#[component]
pub fn BlocklistTable(lists: Vec<BlocklistStatus>) -> Element {
    if lists.is_empty() {
        return rsx! {
            p { class: "empty", "No lists configured" }
        };
    }
    let today = LocalDate::today();

    rsx! {
        div { class: "table-scroll",
            table { class: "stack",
                caption { class: "sr-only", "Configured blocklists and allowlists" }
                thead {
                    tr {
                        th { "List" }
                        th { "Kind" }
                        th { "Status" }
                        th { class: "numeric", "Domains" }
                        th { class: "numeric", "Unsupported lines" }
                        th { "Loaded / cached at" }
                        th { "Last error" }
                    }
                }
                tbody {
                    for list in lists.iter() {
                        tr {
                            td { class: "domain", "{list.list}" }
                            td { "data-label": "Kind",
                                if list.kind == ListKind::Block {
                                    "block"
                                } else {
                                    "allow"
                                }
                            }
                            td { "data-label": "Status",
                                if list.last_error.is_some() {
                                    Badge {
                                        variant: BadgeVariant::Outline,
                                        class: "badge-error",
                                        if list.domains > 0 {
                                            "error; rules retained"
                                        } else {
                                            "unavailable"
                                        }
                                    }
                                } else {
                                    Badge {
                                        variant: BadgeVariant::Outline,
                                        class: "badge-ok",
                                        "ok"
                                    }
                                }
                            }
                            td { class: "numeric", "data-label": "Domains", "{list.domains}" }
                            td {
                                class: "numeric",
                                "data-label": "Unsupported lines",
                                "{list.unsupported_lines}"
                            }
                            td { "data-label": "Loaded / cached at",
                                "{list.updated_at_ms.map(|updated| local_time(updated, today)).unwrap_or_else(|| \"–\".to_owned())}"
                            }
                            LastErrorCell { error: list.last_error.clone() }
                        }
                    }
                }
            }
        }
    }
}
