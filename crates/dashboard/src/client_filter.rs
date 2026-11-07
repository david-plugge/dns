use std::net::IpAddr;

use api::ObservedClient;
use dioxus::prelude::*;

use crate::{components::input::Input, view::ViewState};

#[component]
pub fn ClientFilter(
    mut view: Signal<ViewState>,
    clients: Vec<ObservedClient>,
    reset: u64,
) -> Element {
    let mut text = use_signal(String::new);
    let mut applied = use_signal(|| (None::<IpAddr>, String::new(), reset));
    let mut feedback = use_signal(|| ClientMatch::All);
    let known_clients = clients.clone();
    use_effect(use_reactive!(|(known_clients, reset)| {
        let client = view().client;
        let label = client
            .map(|ip| client_label(ip, &known_clients))
            .unwrap_or_default();
        if *applied.peek() != (client, label.clone(), reset) {
            text.set(label.clone());
            applied.set((client, label, reset));
            feedback.set(ClientMatch::All);
        }
    }));

    let commit = use_callback({
        let clients = clients.clone();
        move |value: String| {
            let matched = match_client(&value, &clients);
            text.set(match matched {
                ClientMatch::All => String::new(),
                ClientMatch::Client(ip) => client_label(ip, &clients),
                ClientMatch::Choose | ClientMatch::Unknown => value,
            });
            feedback.set(matched);
            let client = match matched {
                ClientMatch::All => None,
                ClientMatch::Client(ip) => Some(ip),
                ClientMatch::Choose | ClientMatch::Unknown => return,
            };
            view.with_mut(|query| {
                query.client = client;
                query.offset = 0;
            });
        }
    });

    rsx! {
        div { class: "client-filter",
            Input {
                value: "{text}",
                list: "client-filter-options",
                autocomplete: "off",
                placeholder: "Client name or IP",
                "aria-label": "Client name or IP",
                "aria-invalid": if feedback() == ClientMatch::Unknown { "true" } else { "false" },
                "aria-describedby": if matches!(feedback(), ClientMatch::Choose | ClientMatch::Unknown) { "client-filter-feedback" },
                oninput: {
                    let clients = clients.clone();
                    move |event: FormEvent| {
                        let value = event.value();
                        if let ClientMatch::Client(ip) = match_client(&value, &clients)
                            && value.trim().parse::<IpAddr>().is_err()
                            && !client_label(ip, &clients).eq_ignore_ascii_case(value.trim())
                        {
                            text.set(value);
                            feedback.set(ClientMatch::Choose);
                            return;
                        }
                        commit.call(value);
                    }
                },
                onchange: move |event: FormEvent| commit.call(event.value()),
                onkeydown: move |event: KeyboardEvent| {
                    if event.key() == Key::Enter {
                        commit.call(text());
                    }
                },
            }
            datalist { id: "client-filter-options",
                for client in &clients {
                    option { value: device_label(client) }
                }
            }
            if matches!(feedback(), ClientMatch::Choose | ClientMatch::Unknown) {
                p {
                    id: "client-filter-feedback",
                    class: if feedback() == ClientMatch::Unknown { "failed" } else { "muted" },
                    role: "status",
                    if feedback() == ClientMatch::Unknown {
                        "Enter a valid IP address or select a known device."
                    } else {
                        "Select a device or press Enter on its full name."
                    }
                }
            }
        }
    }
}

fn client_label(ip: IpAddr, clients: &[ObservedClient]) -> String {
    clients
        .iter()
        .find(|client| client.client == ip)
        .map(device_label)
        .unwrap_or_else(|| ip.to_string())
}

fn device_label(client: &ObservedClient) -> String {
    let address = client.client.to_string();
    match client.name.as_deref() {
        Some(name) if !name.is_empty() && name != address => format!("{name} ({address})"),
        _ => address,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ClientMatch {
    All,
    Client(IpAddr),
    Choose,
    Unknown,
}

fn match_client(text: &str, clients: &[ObservedClient]) -> ClientMatch {
    let text = text.trim();
    if text.is_empty() {
        return ClientMatch::All;
    }
    if let Ok(ip) = text.parse() {
        return ClientMatch::Client(ip);
    }

    let mut exact = clients.iter().filter(|client| {
        client
            .name
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(text))
            || device_label(client).eq_ignore_ascii_case(text)
    });
    if let Some(client) = exact.next() {
        return if exact.next().is_none() {
            ClientMatch::Client(client.client)
        } else {
            ClientMatch::Choose
        };
    }
    let text = text.to_lowercase();
    if clients
        .iter()
        .any(|client| device_label(client).to_lowercase().contains(&text))
    {
        ClientMatch::Choose
    } else {
        ClientMatch::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(ip: &str, name: Option<&str>) -> ObservedClient {
        ObservedClient {
            client: ip.parse().unwrap(),
            name: name.map(str::to_owned),
            queries: 1,
            last_seen_ms: 0,
        }
    }

    #[test]
    fn resolves_names_and_labels_without_guessing_partial_or_duplicate_names() {
        let clients = [
            device("192.168.1.10", Some("Laptop")),
            device("2001:db8::1", Some("Phone")),
        ];
        assert!(
            matches!(match_client("laptop", &clients), ClientMatch::Client(ip) if ip == clients[0].client)
        );
        assert!(
            matches!(match_client("Phone (2001:db8::1)", &clients), ClientMatch::Client(ip) if ip == clients[1].client)
        );
        assert!(matches!(match_client("Lap", &clients), ClientMatch::Choose));
        assert!(matches!(
            match_client("192.168.1", &clients),
            ClientMatch::Choose
        ));
        let duplicates = [
            device("192.168.1.10", Some("Laptop")),
            device("192.168.1.11", Some("Laptop")),
        ];
        assert!(matches!(
            match_client("Laptop", &duplicates),
            ClientMatch::Choose
        ));
        assert!(
            matches!(match_client("Laptop (192.168.1.11)", &duplicates), ClientMatch::Client(ip) if ip == duplicates[1].client)
        );
    }

    #[test]
    fn accepts_unknown_ips_and_clearing_but_not_unknown_names() {
        assert!(matches!(
            match_client(" 10.0.0.42 ", &[]),
            ClientMatch::Client(_)
        ));
        assert!(matches!(
            match_client("2001:db8::42", &[]),
            ClientMatch::Client(_)
        ));
        assert!(matches!(match_client(" ", &[]), ClientMatch::All));
        assert!(matches!(match_client("unknown", &[]), ClientMatch::Unknown));
        assert_eq!(client_label("10.0.0.42".parse().unwrap(), &[]), "10.0.0.42");
    }
}
