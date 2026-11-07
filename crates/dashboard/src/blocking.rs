use std::net::IpAddr;

use api::{BlockingPause, BlockingScope, Device, PauseBlockingRequest};
use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;

use crate::{
    api_client::{self, FetchError},
    format::client_name,
};

#[component]
pub fn BlockingPanel(page_visible: ReadSignal<bool>, on_logged_out: EventHandler<()>) -> Element {
    let mut devices = use_signal(Vec::<Device>::new);
    let mut pause = use_signal(|| None::<BlockingPause>);
    let mut loaded = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut all = use_signal(|| true);
    let mut selected = use_signal(Vec::<IpAddr>::new);
    let mut minutes = use_signal(|| "5".to_owned());
    let mut busy = use_signal(|| false);
    let mut now = use_signal(js_sys::Date::now);
    let mut report = move |failure| match failure {
        FetchError::SessionRequired => on_logged_out.call(()),
        FetchError::Failed(message) => error.set(Some(message)),
    };

    use_resource(move || {
        let visible = page_visible();
        let changing = busy();
        async move {
            if !visible || changing {
                return;
            }
            loop {
                match api_client::fetch_blocking_pause().await {
                    Ok(latest) => {
                        pause.set(latest);
                        loaded.set(true);
                    }
                    Err(failure) => report(failure),
                }
                TimeoutFuture::new(2_000).await;
            }
        }
    });
    use_resource(move || {
        let visible = page_visible();
        async move {
            if !visible {
                return;
            }
            loop {
                match api_client::fetch_devices().await {
                    Ok(latest) => devices.set(latest),
                    Err(failure) => report(failure),
                }
                TimeoutFuture::new(10_000).await;
            }
        }
    });
    use_future(move || async move {
        loop {
            now.set(js_sys::Date::now());
            TimeoutFuture::new(1_000).await;
        }
    });

    let duration = minutes()
        .parse::<u64>()
        .ok()
        .filter(|value| (1..=1440).contains(value));
    let active = pause().filter(|pause| pause.until_ms as f64 > now());
    let submit = move |_| async move {
        let Some(minutes) = duration else {
            return;
        };
        let target = if all() {
            BlockingScope::All
        } else {
            BlockingScope::Clients(selected())
        };
        busy.set(true);
        error.set(None);
        match api_client::pause_blocking(PauseBlockingRequest {
            duration_seconds: minutes * 60,
            target,
        })
        .await
        {
            Ok(()) => {}
            Err(failure) => report(failure),
        }
        busy.set(false);
    };
    let resume = move |_| async move {
        busy.set(true);
        error.set(None);
        match api_client::resume_blocking().await {
            Ok(()) => pause.set(None),
            Err(failure) => report(failure),
        }
        busy.set(false);
    };

    rsx! {
        section { class: "panel blocking-panel",
            h2 { "Blocking" }
            if let Some(message) = error() {
                div { class: "error-banner", role: "alert", "{message}" }
            }
            if let Some(active) = active {
                {
                    let seconds = ((active.until_ms as f64 - now()) / 1000.0).ceil() as u64;
                    let target = match active.target {
                        BlockingScope::All => "all devices".to_owned(),
                        BlockingScope::Clients(clients) => clients.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
                    };
                    rsx! {
                        p { class: "pause-status", role: "status", "Blocking paused for {target} · {seconds / 60}m {seconds % 60}s remaining" }
                        button { disabled: busy(), onclick: resume, "Resume blocking now" }
                    }
                }
            } else if loaded() {
                p { role: "status", "Blocking is active for all devices." }
            } else {
                p { "Loading blocking status…" }
            }
            div { class: "blocking-controls",
                label {
                    "Pause for "
                    select {
                        value: if all() { "all" } else { "selected" },
                        disabled: busy(),
                        onchange: move |event| all.set(event.value() == "all"),
                        option { value: "all", "All devices" }
                        option { value: "selected", "Selected devices" }
                    }
                }
                label {
                    "Duration (minutes) "
                    input {
                        r#type: "number", min: "1", max: "1440", step: "1",
                        value: minutes(), disabled: busy(),
                        oninput: move |event| minutes.set(event.value()),
                    }
                }
                button {
                    disabled: busy() || !loaded() || duration.is_none() || (!all() && selected().is_empty()),
                    onclick: submit,
                    if pause().is_some() { "Replace pause" } else { "Pause blocking" }
                }
            }
            if !all() {
                fieldset { class: "device-picker", disabled: busy(),
                    legend { "Devices seen in the retained query log" }
                    if devices().is_empty() { p { class: "empty", "No devices seen yet." } }
                    for device in devices() {
                        label { key: "{device.client}",
                            input {
                                r#type: "checkbox",
                                checked: selected().contains(&device.client),
                                onchange: move |event| selected.with_mut(|clients| {
                                    clients.retain(|client| *client != device.client);
                                    if event.checked() { clients.push(device.client); }
                                }),
                            }
                            "{client_name(device.client, device.name.as_deref())}"
                        }
                    }
                }
            }
            p { class: "muted", "A new pause replaces the current pause. Devices are identified by IP; select each address a device uses. Restarting the server ends the pause. Cached DNS answers on devices may take time to expire." }
        }
    }
}
