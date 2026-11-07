use dioxus::prelude::*;
use dioxus_primitives::{dioxus_attributes::attributes, merge_attributes};

#[css_module("/src/components/badge/style.css")]
struct Styles;

#[derive(Copy, Clone, PartialEq, Default)]
#[non_exhaustive]
pub enum BadgeVariant {
    #[default]
    Primary,
    Outline,
}

impl BadgeVariant {
    pub fn class(&self) -> &'static str {
        match self {
            BadgeVariant::Primary => "primary",
            BadgeVariant::Outline => "outline",
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct BadgeProps {
    #[props(default)]
    pub variant: BadgeVariant,

    #[props(extends = GlobalAttributes)]
    pub attributes: Vec<Attribute>,

    pub children: Element,
}

#[component]
pub fn Badge(props: BadgeProps) -> Element {
    let base = attributes!(span {
        class: Styles::dx_badge,
        "data-style": props.variant.class(),
        "padding": "true",
    });
    let merged = merge_attributes(vec![base, props.attributes]);

    rsx! {
        span {
            ..merged,
            {props.children}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_class_preserves_registry_class() {
        let html = dioxus_ssr::render_element(rsx! {
            Badge { class: "custom-badge", "Content" }
        });
        let classes = html
            .split("class=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();

        assert!(
            classes
                .split_whitespace()
                .any(|class| class == "custom-badge")
        );
        assert_eq!(classes.split_whitespace().count(), 2);
        assert!(html.contains("Content"));
        assert!(html.contains("padding=\"true\""));
    }
}
