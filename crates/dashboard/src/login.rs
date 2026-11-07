//! Asks for the master password.

use dioxus::prelude::*;

use crate::{
    api_client::{self, LoginError},
    components::{
        button::Button,
        card::{Card, CardContent, CardDescription, CardHeader, CardTitle},
        input::Input,
    },
};

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
        Card { class: "login",
            CardHeader {
                CardTitle {
                    h1 { "Welcome back" }
                }
                CardDescription { "Log in to your DNS dashboard." }
            }
            CardContent {
                form { onsubmit: submit,
                    label { r#for: "password", "Master password" }
                    Input {
                        id: "password",
                        r#type: "password",
                        autocomplete: "current-password",
                        placeholder: "Enter your password",
                        "autofocus": true,
                        value: "{password}",
                        oninput: move |event: FormEvent| password.set(event.value()),
                    }
                    Button {
                        r#type: "submit",
                        disabled: pending() || password().is_empty(),
                        if pending() {
                            "Logging in…"
                        } else {
                            "Log in"
                        }
                    }
                    match error() {
                        None => rsx! {},
                        Some(LoginError::WrongPassword) => rsx! {
                            p { class: "failed", role: "alert", "Wrong password" }
                        },
                        Some(LoginError::Failed(message)) => rsx! {
                            p { class: "failed", role: "alert", "{message}" }
                        },
                    }
                }
            }
        }
    }
}
