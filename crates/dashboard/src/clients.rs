use api::{ObservedClient, StatsWindow};
use dioxus::prelude::*;
use std::net::IpAddr;

use crate::{
    components::card::{Card, CardContent, CardDescription, CardHeader, CardTitle},
    format::{LocalDate, client_name, local_time, window_name},
};

#[component]
pub fn ObservedClients(
    clients: Option<Vec<ObservedClient>>,
    window: StatsWindow,
    on_select: EventHandler<IpAddr>,
) -> Element {
    let today = LocalDate::today();
    rsx! {
        Card {
            CardHeader {
                CardTitle {
                    h2 { "Observed clients" }
                }
                CardDescription {
                    "Most active clients during the {window_name(window)}. Up to 1,000 addresses, derived from retained queries — not a permanent device inventory."
                }
            }
            CardContent {
                p { class: "muted log-hint",
                    "Names come from your configured local resolver. A forwarding router can hide individual devices behind its address. This view does not edit client settings."
                }
                match clients {
                    None => rsx! {
                        p { class: "empty", "Loading clients…" }
                    },
                    Some(clients) if clients.is_empty() => rsx! {
                        p { class: "empty", "No clients observed in this period" }
                    },
                    Some(clients) => rsx! {
                        div { class: "table-scroll",
                            table { class: "stack client-table",
                                caption { class: "sr-only", "Clients observed in the selected period" }
                                thead {
                                    tr {
                                        th { "Client / queries" }
                                        th { "IP address" }
                                        th { "Last seen" }
                                        th { class: "numeric", "Queries" }
                                    }
                                }
                                tbody {
                                    for client in clients {
                                        tr {
                                            td {
                                                button {
                                                    class: "text-link",
                                                    r#type: "button",
                                                    onclick: move |_| on_select.call(client.client),
                                                    "{client_name(client.client, client.name.as_deref())}"
                                                }
                                            }
                                            td { "data-label": "IP address", "{client.client}" }
                                            td { "data-label": "Last seen", "{local_time(client.last_seen_ms, today)}" }
                                            td { class: "numeric", "data-label": "Queries", "{client.queries}" }
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }
        }
    }
}
