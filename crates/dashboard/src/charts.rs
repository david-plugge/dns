//! Small chart components drawn with plain SVG and HTML, without a JS charting library.

use dioxus::prelude::*;

use crate::format::{LocalDate, local_time};

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
pub fn LineChart(
    series: Vec<Series>,
    x_labels: [String; 3],
    start_ms: u64,
    bucket_ms: u64,
    on_select: EventHandler<usize>,
) -> Element {
    let max = series
        .iter()
        .flat_map(|s| s.values.iter().copied())
        .max()
        .unwrap_or(0)
        .max(1);

    let bucket_count = series.first().map_or(0, |series| series.values.len());
    let today = LocalDate::today();
    rsx! {
        div { class: "line-chart",
            div { class: "y-axis",
                span { "{max}" }
                span { "0" }
            }
            svg {
                view_box: "0 0 {WIDTH} {HEIGHT}",
                preserve_aspect_ratio: "none",
                role: "img",
                "aria-label": "Query activity. Exact values are available in View chart data.",
                for s in series.iter() {
                    polygon {
                        class: "area {s.css_class}",
                        points: area_points(&s.values, max),
                    }
                    polyline {
                        class: "line {s.css_class}",
                        points: line_points(&s.values, max),
                    }
                }
                for index in 0..bucket_count {
                    rect {
                        x: "{index as f64 * WIDTH / bucket_count as f64}",
                        y: "0",
                        width: "{WIDTH / bucket_count as f64}",
                        height: "{HEIGHT}",
                        fill: "transparent",
                        title {
                            "{local_time(start_ms + index as u64 * bucket_ms, today)}"
                            for series in series.iter() {
                                " · {series.label}: {series.values[index]}"
                            }
                        }
                    }
                }
            }
            div { class: "x-axis",
                for label in x_labels.iter() {
                    span { "{label}" }
                }
            }
            div { class: "legend",
                for (index, s) in series.iter().enumerate() {
                    button {
                        class: "text-link legend-item {s.css_class}",
                        r#type: "button",
                        onclick: move |_| on_select.call(index),
                        "{s.label}"
                    }
                }
            }
        }
        details { class: "chart-data",
            summary { "View chart data" }
            div { class: "table-scroll",
                table {
                    caption { class: "sr-only", "Exact query activity values" }
                    thead {
                        tr {
                            th { "Bucket start" }
                            for series in series.iter() {
                                th { "{series.label}" }
                            }
                        }
                    }
                    tbody {
                        for index in 0..bucket_count {
                            tr {
                                td { "{local_time(start_ms + index as u64 * bucket_ms, today)}" }
                                for series in series.iter() {
                                    td { "{series.values[index]}" }
                                }
                            }
                        }
                    }
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
pub fn BarList(
    bars: Vec<Bar>,
    empty_text: &'static str,
    on_select: EventHandler<usize>,
) -> Element {
    let max = bars.iter().map(|bar| bar.value).max().unwrap_or(0).max(1);

    rsx! {
        if bars.is_empty() {
            p { class: "empty", "{empty_text}" }
        }
        ul { class: "bar-list",
            for (index, bar) in bars.iter().enumerate() {
                li {
                    button {
                        class: "bar-action",
                        r#type: "button",
                        onclick: move |_| on_select.call(index),
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
}
