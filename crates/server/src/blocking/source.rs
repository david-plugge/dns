//! Where a list comes from: an HTTP(S) URL, cached on disk, or a local file.

use std::{fmt, path::PathBuf, str::FromStr};

use reqwest::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListLocation {
    Url(Url),
    File(PathBuf),
}

impl ListLocation {
    /// File name of the cached copy of a URL list, stable across restarts and versions.
    pub fn cache_file_name(url: &Url) -> String {
        // FNV-1a: the standard library's hasher is not guaranteed to be stable.
        let hash = url
            .as_str()
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            });
        format!("{hash:016x}.txt")
    }
}

impl fmt::Display for ListLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ListLocation::Url(url) => write!(f, "{url}"),
            ListLocation::File(path) => write!(f, "{}", path.display()),
        }
    }
}

impl FromStr for ListLocation {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if text.is_empty() {
            return Err("expected a URL or a file path, got nothing".to_owned());
        }
        if !(text.starts_with("http://") || text.starts_with("https://")) {
            return Ok(ListLocation::File(PathBuf::from(text)));
        }
        Url::parse(text)
            .map(ListLocation::Url)
            .map_err(|error| format!("invalid list URL {text:?}: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urls_and_file_paths() {
        let url: ListLocation = "https://example.com/hosts.txt".parse().unwrap();
        let file: ListLocation = "/lists/own.txt".parse().unwrap();

        assert!(matches!(url, ListLocation::Url(_)));
        assert_eq!(file, ListLocation::File(PathBuf::from("/lists/own.txt")));
        assert!("https://".parse::<ListLocation>().is_err());
        assert!("".parse::<ListLocation>().is_err());
    }

    #[test]
    fn cache_file_names_differ_per_url() {
        let first = Url::parse("https://example.com/a.txt").unwrap();
        let second = Url::parse("https://example.com/b.txt").unwrap();

        assert_ne!(
            ListLocation::cache_file_name(&first),
            ListLocation::cache_file_name(&second)
        );
    }
}
