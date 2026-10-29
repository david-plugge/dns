//! Whether the browser shows the page, so polling can pause while the tab is in the background.

use dioxus::prelude::*;
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::Document;

/// Follows the page's visibility. Use it only in the root component: its listener stays
/// registered for the lifetime of the page.
pub fn use_page_visible() -> ReadSignal<bool> {
    let mut is_visible = use_signal(|| !document().hidden());
    use_hook(move || {
        let listener = Closure::<dyn FnMut()>::new(move || is_visible.set(!document().hidden()));
        // Cannot fail: the callback is a function.
        let _ = document().add_event_listener_with_callback(
            "visibilitychange",
            listener.as_ref().unchecked_ref(),
        );
        listener.forget();
    });
    is_visible.into()
}

fn document() -> Document {
    web_sys::window()
        .and_then(|window| window.document())
        .expect("the dashboard runs in a browser page")
}
