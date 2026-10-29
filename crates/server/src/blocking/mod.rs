//! Blocks domains listed in blocklists, unless an allowlist permits them.
//!
//! URL lists are cached on disk, so blocking works right after a restart even without
//! internet access. The cache is refreshed once it is older than the refresh interval;
//! local files are reread when they change.

mod blocklist;
mod download;
mod rules;
mod source;
mod state;

use std::{
    mem,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use api::{BlocklistStatus, ListKind};
use reqwest::Url;
use tracing::{error, info, warn};

use self::{
    blocklist::{Blocklist, BlocklistBuilder},
    rules::{ParsedList, Rule},
    state::{ListError, Probe, SourceState, due_sources},
};
pub use self::{download::Downloader, rules::parse_domain, source::ListLocation};
use crate::error::ErrorCode;

/// How often stale or failed downloads and changed files are checked for.
const CHECK_INTERVAL: Duration = Duration::from_secs(600);

pub struct BlockingConfig {
    pub blocklists: Vec<ListLocation>,
    pub allowlists: Vec<ListLocation>,
    pub blocked_domains: Vec<Box<str>>,
    pub allowed_domains: Vec<Box<str>>,
    pub cache_dir: PathBuf,
    pub refresh_interval: Duration,
}

pub struct Blocker {
    sources: Vec<ListSource>,
    cache_dir: PathBuf,
    refresh_interval: Duration,
    downloader: Downloader,
    current: RwLock<Arc<Blocklist>>,
    /// One entry per source, in the same order.
    states: Mutex<Vec<SourceState>>,
}

struct ListSource {
    name: Arc<str>,
    kind: ListKind,
    content: SourceContent,
}

enum SourceContent {
    Location(ListLocation),
    /// Domains given directly in the configuration.
    Inline(Vec<Box<str>>),
}

impl Blocker {
    /// Blocks nothing until [`Blocker::load`] is called.
    pub fn new(config: BlockingConfig, downloader: Downloader) -> Self {
        let lists = |locations: Vec<ListLocation>, kind| {
            locations.into_iter().map(move |location| ListSource {
                name: Arc::from(location.to_string()),
                kind,
                content: SourceContent::Location(location),
            })
        };
        let inline = |name: &str, domains: Vec<Box<str>>, kind| {
            (!domains.is_empty()).then(|| ListSource {
                name: Arc::from(name),
                kind,
                content: SourceContent::Inline(domains),
            })
        };
        let sources: Vec<ListSource> = lists(config.blocklists, ListKind::Block)
            .chain(inline(
                "DNS_BLOCKED_DOMAINS",
                config.blocked_domains,
                ListKind::Block,
            ))
            .chain(lists(config.allowlists, ListKind::Allow))
            .chain(inline(
                "DNS_ALLOWED_DOMAINS",
                config.allowed_domains,
                ListKind::Allow,
            ))
            .collect();
        let states = sources.iter().map(|_| SourceState::default()).collect();

        Self {
            sources,
            cache_dir: config.cache_dir,
            refresh_interval: config.refresh_interval,
            downloader,
            current: RwLock::new(Arc::new(Blocklist::default())),
            states: Mutex::new(states),
        }
    }

    /// The list blocking `domain` (lowercase), if it is blocked.
    pub fn blocked_by(&self, domain: &str) -> Option<Arc<str>> {
        let current = self
            .current
            .read()
            .expect("blocklist lock poisoned")
            .clone();
        current.blocked_by(domain).cloned()
    }

    pub fn status(&self) -> Vec<BlocklistStatus> {
        let states = self.states.lock().expect("status lock poisoned");
        self.sources
            .iter()
            .zip(states.iter())
            .map(|(source, state)| BlocklistStatus {
                list: source.name.to_string(),
                kind: source.kind,
                domains: state.parsed.map_or(0, |parsed| parsed.domains as u64),
                unsupported_lines: state
                    .parsed
                    .map_or(0, |parsed| parsed.unsupported_lines as u64),
                updated_at_ms: state.parsed.and(state.modified).and_then(unix_ms),
                last_error: state.last_error.as_ref().map(ErrorCode::report),
            })
            .collect()
    }

    /// Builds the blocklist from the cached and local lists, without downloading.
    pub async fn load(self: &Arc<Self>) {
        self.rebuild(no_errors(self.sources.len())).await;
    }

    /// Keeps the lists current; never returns.
    pub async fn keep_refreshed(self: Arc<Self>) {
        // `interval` panics on a zero period.
        let period = CHECK_INTERVAL
            .min(self.refresh_interval)
            .max(Duration::from_secs(1));
        let mut checks = tokio::time::interval(period);
        loop {
            checks.tick().await;
            self.refresh().await;
        }
    }

    /// Downloads URL lists whose cached copy is stale and rebuilds the blocklist if any list
    /// changed.
    async fn refresh(self: &Arc<Self>) {
        let probes: Vec<Probe> = self
            .sources
            .iter()
            .map(|source| self.probe(source))
            .collect();
        let due = {
            let states = self.states.lock().expect("status lock poisoned");
            due_sources(&probes, &states, self.refresh_interval)
        };

        let mut changed = false;
        let mut download_errors = no_errors(self.sources.len());
        for index in due {
            let source = &self.sources[index];
            let SourceContent::Location(ListLocation::Url(url)) = &source.content else {
                changed = true;
                continue;
            };
            match self.download(source, url).await {
                Ok(()) => changed = true,
                Err(error) => {
                    warn!(%error, "could not download list");
                    download_errors[index] = Some(error);
                }
            }
        }

        if changed {
            self.rebuild(download_errors).await;
            return;
        }
        let mut states = self.states.lock().expect("status lock poisoned");
        for (state, error) in states.iter_mut().zip(download_errors) {
            if let Some(error) = error {
                state.last_error = Some(error);
            }
        }
    }

    fn probe(&self, source: &ListSource) -> Probe {
        match &source.content {
            SourceContent::Inline(_) => Probe::Inline,
            SourceContent::Location(ListLocation::Url(url)) => Probe::Url {
                cache_age: modified(&self.cache_path(url))
                    .and_then(|modified| modified.elapsed().ok()),
            },
            SourceContent::Location(ListLocation::File(path)) => Probe::File {
                modified: modified(path),
            },
        }
    }

    async fn download(&self, source: &ListSource, url: &Url) -> Result<(), ListError> {
        let body = self
            .downloader
            .fetch(url)
            .await
            .map_err(|error| ListError::Download {
                list: source.name.clone(),
                source: error,
            })?;
        let cached = self.cache_path(url);
        let write = async {
            tokio::fs::create_dir_all(&self.cache_dir).await?;
            // Write to a temporary file first, so a crash never leaves a truncated list.
            let partial = cached.with_extension("partial");
            tokio::fs::write(&partial, &body).await?;
            tokio::fs::rename(&partial, &cached).await
        };
        write.await.map_err(|error| ListError::CacheWrite {
            list: source.name.clone(),
            path: self.cache_dir.clone(),
            source: error,
        })?;
        info!(list = %url, bytes = body.len(), "downloaded list");
        Ok(())
    }

    /// Rereads all lists and replaces the blocklist. `download_errors` holds, per source,
    /// why its latest download failed.
    async fn rebuild(self: &Arc<Self>, download_errors: Vec<Option<ListError>>) {
        let blocker = self.clone();
        let built = tokio::task::spawn_blocking(move || blocker.build(download_errors)).await;
        let (blocklist, states) = match built {
            Ok(built) => built,
            Err(error) => {
                error!(%error, "could not build the blocklist; keeping the previous one");
                return;
            }
        };
        let previous = mem::replace(
            &mut *self.current.write().expect("blocklist lock poisoned"),
            Arc::new(blocklist),
        );
        *self.states.lock().expect("status lock poisoned") = states;
        // Freeing the previous blocklist is blocking work, and must not hold the lock.
        tokio::task::spawn_blocking(move || drop(previous));
    }

    fn build(&self, download_errors: Vec<Option<ListError>>) -> (Blocklist, Vec<SourceState>) {
        let mut builder = BlocklistBuilder::default();
        let states = self
            .sources
            .iter()
            .zip(download_errors)
            .map(|(source, download_error)| {
                let mut state = self.read(source, |rule| add_rule(&mut builder, source, rule));
                if let Some(error) = &state.last_error {
                    warn!(%error, "could not load list");
                }
                // The download error explains why the list is missing or outdated.
                if download_error.is_some() {
                    state.last_error = download_error;
                }
                state
            })
            .collect();

        let blocklist = builder.build();
        info!(
            blocked = blocklist.blocked_count(),
            allowed = blocklist.allowed_count(),
            "blocklist loaded"
        );
        (blocklist, states)
    }

    /// Passes the rules of the list to `on_rule`.
    fn read(&self, source: &ListSource, mut on_rule: impl FnMut(Rule)) -> SourceState {
        let path = match &source.content {
            SourceContent::Inline(domains) => {
                for domain in domains {
                    on_rule(Rule::Block(domain));
                }
                let parsed = ParsedList {
                    domains: domains.len(),
                    unsupported_lines: 0,
                };
                return SourceState {
                    parsed: Some(parsed),
                    ..SourceState::default()
                };
            }
            SourceContent::Location(ListLocation::File(path)) => path.clone(),
            SourceContent::Location(ListLocation::Url(url)) => {
                let cached = self.cache_path(url);
                if !cached.exists() {
                    let error = ListError::NotDownloaded {
                        list: source.name.clone(),
                    };
                    return SourceState {
                        last_error: Some(error),
                        ..SourceState::default()
                    };
                }
                cached
            }
        };
        // Taken before reading, so that a change while reading is noticed by the next check.
        let modified = modified(&path);
        let content = match std::fs::read(&path) {
            Ok(content) => content,
            Err(error) => {
                return SourceState {
                    modified,
                    parsed: None,
                    last_error: Some(ListError::Read {
                        path,
                        source: error,
                    }),
                };
            }
        };
        SourceState {
            modified,
            parsed: Some(rules::parse(&String::from_utf8_lossy(&content), on_rule)),
            last_error: None,
        }
    }

    fn cache_path(&self, url: &Url) -> PathBuf {
        self.cache_dir.join(ListLocation::cache_file_name(url))
    }
}

/// Every entry of an allowlist allows, whatever syntax it uses.
fn add_rule(builder: &mut BlocklistBuilder, source: &ListSource, rule: Rule) {
    match (source.kind, rule) {
        (ListKind::Block, Rule::Block(domain)) => builder.block(domain, &source.name),
        (ListKind::Block, Rule::Allow(domain))
        | (ListKind::Allow, Rule::Block(domain) | Rule::Allow(domain)) => builder.allow(domain),
    }
}

fn no_errors(sources: usize) -> Vec<Option<ListError>> {
    (0..sources).map(|_| None).collect()
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn unix_ms(time: SystemTime) -> Option<u64> {
    let since_epoch = time.duration_since(UNIX_EPOCH).ok()?;
    Some(since_epoch.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upstream::UpstreamPool;

    fn blocker(config: BlockingConfig) -> Arc<Blocker> {
        let upstreams = Arc::new(UpstreamPool::new(Vec::new(), Duration::from_secs(1)));
        Arc::new(Blocker::new(config, Downloader::new(upstreams).unwrap()))
    }

    fn config(cache_dir: &Path) -> BlockingConfig {
        BlockingConfig {
            blocklists: Vec::new(),
            allowlists: Vec::new(),
            blocked_domains: Vec::new(),
            allowed_domains: Vec::new(),
            cache_dir: cache_dir.to_owned(),
            refresh_interval: Duration::from_secs(86_400),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dns-blocking-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn loads_files_cached_urls_and_inline_domains() {
        let dir = temp_dir("load");
        let file = dir.join("own.txt");
        std::fs::write(&file, "||ads.example.com^\n@@||ok.ads.example.com^\n").unwrap();
        let url = Url::parse("https://lists.example/hosts").unwrap();
        std::fs::write(
            dir.join(ListLocation::cache_file_name(&url)),
            "0.0.0.0 tracker.example.net\n",
        )
        .unwrap();
        let blocker = blocker(BlockingConfig {
            blocklists: vec![
                ListLocation::File(file.clone()),
                ListLocation::Url(url.clone()),
            ],
            allowlists: vec![],
            blocked_domains: vec![Box::from("custom.example")],
            allowed_domains: vec![Box::from("www.tracker.example.net")],
            ..config(&dir)
        });

        blocker.load().await;

        let blocked_by = |domain| blocker.blocked_by(domain).map(|list| list.to_string());
        assert_eq!(
            blocked_by("x.ads.example.com."),
            Some(file.display().to_string())
        );
        assert_eq!(blocked_by("ok.ads.example.com."), None);
        assert_eq!(blocked_by("tracker.example.net."), Some(url.to_string()));
        assert_eq!(blocked_by("www.tracker.example.net."), None);
        assert_eq!(
            blocked_by("custom.example."),
            Some("DNS_BLOCKED_DOMAINS".to_owned())
        );
        let domains: Vec<u64> = blocker
            .status()
            .iter()
            .map(|status| status.domains)
            .collect();
        assert_eq!(domains, [2, 1, 1, 1]);
    }

    #[tokio::test]
    async fn missing_lists_are_reported_and_do_not_stop_the_others() {
        let dir = temp_dir("missing");
        let blocker = blocker(BlockingConfig {
            blocklists: vec![
                ListLocation::File(dir.join("missing.txt")),
                ListLocation::Url(Url::parse("https://lists.example/never-downloaded").unwrap()),
            ],
            blocked_domains: vec![Box::from("custom.example")],
            ..config(&dir)
        });

        blocker.load().await;

        let codes: Vec<Option<String>> = blocker
            .status()
            .into_iter()
            .map(|status| status.last_error.map(|error| error.code))
            .collect();
        assert_eq!(
            codes,
            [
                Some("read_failed".to_owned()),
                Some("not_downloaded".to_owned()),
                None
            ]
        );
        assert!(blocker.blocked_by("custom.example").is_some());
    }

    #[tokio::test]
    async fn unreadable_files_are_not_reread_until_they_change() {
        let dir = temp_dir("unreadable");
        // Reading a directory fails, yet it has a modification time like a file.
        let unreadable = dir.join("list.txt");
        std::fs::create_dir(&unreadable).unwrap();
        let blocker = blocker(BlockingConfig {
            blocklists: vec![ListLocation::File(unreadable)],
            ..config(&dir)
        });

        blocker.load().await;

        let probes: Vec<Probe> = blocker
            .sources
            .iter()
            .map(|source| blocker.probe(source))
            .collect();
        let states = blocker.states.lock().unwrap();
        assert!(matches!(states[0].last_error, Some(ListError::Read { .. })));
        assert!(due_sources(&probes, &states, blocker.refresh_interval).is_empty());
    }
}
