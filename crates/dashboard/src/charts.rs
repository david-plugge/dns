//! Small chart components drawn with plain SVG and HTML, without a JS charting library.

use dioxus::prelude::*;

const WIDTH: f64 = 600.0;
const HEIGHT: f64 = 160.0;

#[derive(Clone, PartialEq)]
pub struct Series {
    pub label: &'static str,
    pub css_class: &'static str,
    pub values: Vec<u64>,
}

/// Line chart with a shared y-axis; all series must have the same length.
#[component]
pub fn LineChart(series: Vec<Series>, x_labels: [String; 3]) -> Element {
    let max = series
        .iter()
        .flat_map(|s| s.values.iter().copied())
        .max()
        .unwrap_or(0)
        .max(1);

    rsx! {
        div { class: "line-chart",
            div { class: "y-axis",
                span { "{max}" }
                span { "0" }
            }
            svg {
                view_box: "0 0 {WIDTH} {HEIGHT}",
                preserve_aspect_ratio: "none",
                for s in series.iter() {
                    polygon { class: "area {s.css_class}", points: area_points(&s.values, max) }
                    polyline { class: "line {s.css_class}", points: line_points(&s.values, max) }
                }
            }
            div { class: "x-axis",
                for label in x_labels.iter() {
                    span { "{label}" }
                }
            }
            div { class: "legend",
                for s in series.iter() {
                    span { class: "legend-item {s.css_class}", "{s.label}" }
                }
            }
        }
    }
}

fn line_points(values: &[u64], max: u64) -> String {
    let step = WIDTH / (values.len().max(2) - 1) as f64;
    values
        .iter()
        .enumerate()
        .map(|(i, &value)| {
            let x = i as f64 * step;
            let y = HEIGHT - value as f64 / max as f64 * HEIGHT;
            format!("{x:.1},{y:.1}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn area_points(values: &[u64], max: u64) -> String {
    format!("0,{HEIGHT} {} {WIDTH},{HEIGHT}", line_points(values, max))
}

#[derive(Clone, PartialEq)]
pub struct Bar {
    pub label: String,
    pub value: u64,
}

/// Horizontal bars, scaled relative to the largest value.
#[component]
pub fn BarList(bars: Vec<Bar>, empty_text: &'static str) -> Element {
    let max = bars.iter().map(|bar| bar.value).max().unwrap_or(0).max(1);

    rsx! {
        if bars.is_empty() {
            p { class: "empty", "{empty_text}" }
        }
        ul { class: "bar-list",
            for bar in bars.iter() {
                li {
                    div { class: "bar-label",
                        span { class: "name", title: "{bar.label}", "{bar.label}" }
                        span { class: "value", "{bar.value}" }
                    }
                    div { class: "bar-track",
                        div {
                            class: "bar-fill",
                            width: "{bar.value as f64 / max as f64 * 100.0}%",
                        }
                    }
                }
            }
        }
    }
}
