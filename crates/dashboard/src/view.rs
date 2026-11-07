use std::net::IpAddr;

use api::{QueryStatus, StatsWindow};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, closure::Closure};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewState {
    pub window: StatsWindow,
    pub client: Option<IpAddr>,
    pub domain: Option<String>,
    pub status: Option<QueryStatus>,
    pub domain_exact: bool,
    pub servfail: bool,
    pub offset: usize,
    pub until_ms: Option<u64>,
}

impl ViewState {
    pub fn from_search(search: &str) -> Self {
        let mut view: Self =
            serde_urlencoded::from_str(search.trim_start_matches('?')).unwrap_or_default();
        view.domain = view.domain.filter(|domain| !domain.trim().is_empty());
        if view.domain.is_none() {
            view.domain_exact = false;
        }
        view
    }

    pub fn search(&self) -> String {
        format!(
            "?{}",
            serde_urlencoded::to_string(self).expect("view fields are URL-safe")
        )
    }

    pub fn query_state(&self) -> Self {
        let mut query = self.clone();
        if query.domain.is_none() {
            query.domain_exact = false;
        }
        query
    }

    pub fn clear_filters(&mut self) {
        *self = Self {
            window: self.window,
            ..Self::default()
        };
    }

    pub fn drilldown(
        &self,
        client: Option<IpAddr>,
        domain: Option<String>,
        status: Option<QueryStatus>,
        until_ms: u64,
    ) -> Self {
        Self {
            window: self.window,
            client,
            domain_exact: domain.is_some(),
            domain,
            status,
            until_ms: Some(until_ms),
            ..Self::default()
        }
    }
}

fn current_view() -> ViewState {
    web_sys::window()
        .and_then(|window| window.location().search().ok())
        .map(|search| ViewState::from_search(&search))
        .unwrap_or_default()
}

pub fn use_view() -> Signal<ViewState> {
    let mut view = use_signal(current_view);
    use_effect(move || {
        let selected = view();
        if selected == current_view() {
            return;
        }
        if let Some(window) = web_sys::window() {
            let location = window.location();
            if let (Ok(path), Ok(hash), Ok(history)) =
                (location.pathname(), location.hash(), window.history())
            {
                let url = format!("{path}{}{hash}", selected.search());
                let _ = history.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
            }
        }
    });
    let listener = use_hook(move || {
        let callback = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| view.set(current_view()));
        if let Some(window) = web_sys::window() {
            let _ = window
                .add_event_listener_with_callback("popstate", callback.as_ref().unchecked_ref());
        }
        std::rc::Rc::new(callback)
    });
    use_drop(move || {
        if let Some(window) = web_sys::window() {
            let _ = window.remove_event_listener_with_callback(
                "popstate",
                listener.as_ref().as_ref().unchecked_ref(),
            );
        }
    });
    view
}

pub fn scroll_to_queries() {
    if let Some(element) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("queries"))
    {
        element.scroll_into_view();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_roundtrip_preserves_ipv6_special_characters_time_and_paging() {
        let view = ViewState {
            window: StatsWindow::LastWeek,
            client: Some("2001:db8::1".parse().unwrap()),
            domain: Some("a+b&c.example.".into()),
            status: Some(QueryStatus::Blocked),
            domain_exact: true,
            servfail: false,
            offset: 50,
            until_ms: Some(1800000000000),
        };
        assert_eq!(ViewState::from_search(&view.search()), view);
    }

    #[test]
    fn invalid_url_values_fall_back_to_safe_defaults() {
        assert_eq!(
            ViewState::from_search("?client=invalid&window=bad"),
            ViewState::default()
        );
        assert!(!ViewState::from_search("?domain=&domain_exact=true").domain_exact);
    }

    #[test]
    fn domain_match_only_affects_queries_when_a_domain_is_present() {
        let contains = ViewState {
            offset: 50,
            until_ms: Some(123),
            ..ViewState::default()
        };
        let mut exact = contains.clone();
        exact.domain_exact = true;
        assert_eq!(contains.query_state(), exact.query_state());
        assert_eq!(exact.query_state().offset, 50);
        assert_eq!(exact.query_state().until_ms, Some(123));

        let mut contains = contains;
        contains.domain = Some("example.test".into());
        exact.domain = contains.domain.clone();
        assert_ne!(contains.query_state(), exact.query_state());
    }

    #[test]
    fn clearing_and_drilling_down_keep_the_statistics_window() {
        let selected = ViewState {
            window: StatsWindow::LastDay,
            ..ViewState::default()
        };
        let mut view = selected.drilldown(
            None,
            Some("ads.example.".into()),
            Some(QueryStatus::Blocked),
            123,
        );
        assert!(view.domain_exact);
        assert_eq!(view.until_ms, Some(123));
        view.clear_filters();
        assert_eq!(view, selected);
    }
}
