//! Turns API values into text for people.

use std::net::IpAddr;

use api::StatsWindow;
use js_sys::Date;

pub fn window_short_name(window: StatsWindow) -> &'static str {
    match window {
        StatsWindow::LastHour => "1 h",
        StatsWindow::LastDay => "24 h",
        StatsWindow::LastWeek => "7 d",
    }
}

pub fn window_name(window: StatsWindow) -> &'static str {
    match window {
        StatsWindow::LastHour => "last hour",
        StatsWindow::LastDay => "last 24 hours",
        StatsWindow::LastWeek => "last 7 days",
    }
}

pub fn x_labels(window: StatsWindow) -> [String; 3] {
    let [start, middle] = match window {
        StatsWindow::LastHour => ["-60 min", "-30 min"],
        StatsWindow::LastDay => ["-24 h", "-12 h"],
        StatsWindow::LastWeek => ["-7 d", "-3.5 d"],
    };
    [start.to_owned(), middle.to_owned(), "now".to_owned()]
}

pub fn duration_name(duration_ms: u64) -> String {
    const HOUR_MS: u64 = 3_600_000;
    match duration_ms {
        HOUR_MS => "hour".to_owned(),
        ms if ms % HOUR_MS == 0 => format!("{} hours", ms / HOUR_MS),
        60_000 => "minute".to_owned(),
        ms => format!("{} minutes", ms / 60_000),
    }
}

/// The device name if the local network knows one, otherwise the address.
pub fn client_name(client: IpAddr, name: Option<&str>) -> String {
    name.map_or_else(|| client.to_string(), str::to_owned)
}

/// Domains as people write them, without the trailing dot of the fully qualified name.
pub fn domain_name(domain: &str) -> &str {
    domain.strip_suffix('.').unwrap_or(domain)
}

pub fn percentage(part: u64, total: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    part as f64 / total as f64 * 100.0
}

/// A calendar day in the browser's time zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalDate {
    year: u32,
    month: u32,
    day: u32,
}

impl LocalDate {
    pub fn today() -> Self {
        Self::of(&Date::new_0())
    }

    fn of(date: &Date) -> Self {
        Self {
            year: date.get_full_year(),
            month: date.get_month() + 1,
            day: date.get_date(),
        }
    }
}

/// The time of day, prefixed with the date for timestamps from before `today`.
pub fn local_time(timestamp_ms: u64, today: LocalDate) -> String {
    let date = Date::new(&(timestamp_ms as f64).into());
    let time = format!(
        "{:02}:{:02}:{:02}",
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds()
    );
    let day = LocalDate::of(&date);
    if day == today {
        return time;
    }
    format!("{}-{:02}-{:02} {time}", day.year, day.month, day.day)
}
