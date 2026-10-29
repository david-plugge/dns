//! Asks for the master password.

use dioxus::prelude::*;

use crate::api_client::{self, LoginError};

#[component]
pub fn LoginForm(on_login: EventHandler<()>) -> Element {
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| None::<LoginError>);
    let mut pending = use_signal(|| false);

    let submit = move |event: FormEvent| async move {
        event.prevent_default();
        pending.set(true);
        match api_client::login(password()).await {
            Ok(()) => {
                password.set(String::new());
                error.set(None);
                on_login.call(());
            }
            Err(failure) => error.set(Some(failure)),
        }
        pending.set(false);
    };

    rsx! {
        form { class: "panel login", onsubmit: submit,
            h2 { "Log in" }
            input {
                r#type: "password",
                placeholder: "Password",
                "aria-label": "Password",
                autofocus: true,
                value: "{password}",
                oninput: move |event| password.set(event.value()),
            }
            button { r#type: "submit", disabled: pending() || password().is_empty(), "Log in" }
            match error() {
                None => rsx! {},
                Some(LoginError::WrongPassword) => rsx! { p { class: "failed", role: "alert", "Wrong password" } },
                Some(LoginError::Failed(message)) => rsx! { p { class: "failed", role: "alert", "{message}" } },
            }
        }
    }
}
