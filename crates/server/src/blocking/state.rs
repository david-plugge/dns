//! What is known about each list between refreshes, and which lists a check has to work on.

use std::{
    io,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use super::{download::DownloadError, rules::ParsedList};
use crate::error::ErrorCode;

#[derive(Debug, thiserror::Error)]
pub enum ListError {
    #[error("{list} has not been downloaded yet")]
    NotDownloaded { list: Arc<str> },
    #[error("could not read {}: {source}", path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("could not download {list}: {source}")]
    Download {
        list: Arc<str>,
        source: DownloadError,
    },
    #[error("could not cache {list} in {}: {source}", path.display())]
    CacheWrite {
        list: Arc<str>,
        path: PathBuf,
        source: io::Error,
    },
}

impl ErrorCode for ListError {
    fn code(&self) -> &'static str {
        match self {
            ListError::NotDownloaded { .. } => "not_downloaded",
            ListError::Read { .. } => "read_failed",
            ListError::Download { source, .. } => source.code(),
            ListError::CacheWrite { .. } => "cache_write_failed",
        }
    }
}

#[derive(Debug, Default)]
pub struct SourceState {
    /// Modification time of the list file when it was last read, successfully or not, so that
    /// an unchanged file is not read again. `None` for inline lists and missing files.
    pub modified: Option<SystemTime>,
    /// `None` if the list could not be read.
    pub parsed: Option<ParsedList>,
    pub last_error: Option<ListError>,
}

/// What a check finds out about a list without reading it.
#[derive(Debug, Clone, Copy)]
pub enum Probe {
    /// Configured domains, which never change.
    Inline,
    /// Age of the cached copy; `None` if there is none or its age is unknown.
    Url { cache_age: Option<Duration> },
    /// `None` if the file is missing.
    File { modified: Option<SystemTime> },
}

/// Indexes of the lists to download (URLs) or to read again (files).
///
/// A file is only read again once its modification time changes, also after a failed read:
/// retrying an unchanged file would rebuild the whole blocklist on every check.
pub fn due_sources(
    probes: &[Probe],
    states: &[SourceState],
    refresh_interval: Duration,
) -> Vec<usize> {
    let is_due = |(probe, state): (&Probe, &SourceState)| match *probe {
        Probe::Inline => false,
        Probe::Url { cache_age } => cache_age.is_none_or(|age| age >= refresh_interval),
        Probe::File { modified } => modified != state.modified,
    };
    probes
        .iter()
        .zip(states)
        .enumerate()
        .filter(|&(_, source)| is_due(source))
        .map(|(index, _)| index)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: Duration = Duration::from_secs(86_400);

    fn read_at(modified: Option<SystemTime>, last_error: Option<ListError>) -> SourceState {
        SourceState {
            modified,
            parsed: last_error.is_none().then(ParsedList::default),
            last_error,
        }
    }

    fn read_error(path: &str) -> ListError {
        ListError::Read {
            path: PathBuf::from(path),
            source: io::ErrorKind::PermissionDenied.into(),
        }
    }

    #[test]
    fn urls_are_due_without_a_fresh_cached_copy() {
        let probes = [
            Probe::Url { cache_age: None },
            Probe::Url {
                cache_age: Some(DAY),
            },
            Probe::Url {
                cache_age: Some(DAY / 2),
            },
            Probe::Inline,
        ];
        let states: Vec<SourceState> = probes.iter().map(|_| SourceState::default()).collect();

        assert_eq!(due_sources(&probes, &states, DAY), [0, 1]);
    }

    #[test]
    fn files_are_due_when_their_modification_time_changes() {
        let earlier = SystemTime::UNIX_EPOCH + DAY;
        let later = earlier + DAY;
        let probes = [
            Probe::File {
                modified: Some(earlier),
            },
            Probe::File {
                modified: Some(later),
            },
            Probe::File { modified: None },
            Probe::File {
                modified: Some(earlier),
            },
        ];
        let states = [
            read_at(Some(earlier), None),
            read_at(Some(earlier), None),
            read_at(Some(earlier), None),
            // Appeared since the last check.
            read_at(None, Some(read_error("/lists/new.txt"))),
        ];

        assert_eq!(due_sources(&probes, &states, DAY), [1, 2, 3]);
    }

    #[test]
    fn unreadable_files_are_only_retried_once_they_change() {
        let modified = SystemTime::UNIX_EPOCH + DAY;
        let probes = [
            Probe::File {
                modified: Some(modified),
            },
            Probe::File { modified: None },
        ];
        // Fixing only the permissions keeps the modification time; touching the file or a
        // restart reloads it.
        let states = [
            read_at(Some(modified), Some(read_error("/lists/unreadable.txt"))),
            read_at(None, Some(read_error("/lists/missing.txt"))),
        ];

        assert!(due_sources(&probes, &states, DAY).is_empty());
    }

    #[test]
    fn download_errors_report_the_download_code() {
        let error = ListError::Download {
            list: Arc::from("https://lists.example/hosts"),
            source: DownloadError::TooLarge,
        };

        let report = error.report();
        assert_eq!(report.code, "list_too_large");
        assert_eq!(
            report.message,
            "could not download https://lists.example/hosts: list is larger than 128 MiB"
        );
    }
}
