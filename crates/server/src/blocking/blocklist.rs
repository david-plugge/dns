//! Decides whether a domain is blocked, given the merged block and allow rules.
//!
//! Lists hold up to millions of domains and the server runs on devices with 1 GB of RAM, so
//! all domain text lives in one string and the tables hold packed positions into it, instead
//! of one allocation per domain.

use std::{hash::BuildHasher, ops::Range, sync::Arc};

use hashbrown::{DefaultHashBuilder, HashTable};
use tracing::warn;

/// An immutable snapshot of all lists; replaced as a whole when lists are refreshed.
#[derive(Default)]
pub struct Blocklist {
    /// All domains, back to back.
    text: String,
    blocked: HashTable<Entry>,
    allowed: HashTable<Entry>,
    /// Names of the blocking lists, indexed by [`Entry::list`].
    lists: Vec<Arc<str>>,
    hasher: DefaultHashBuilder,
}

/// A domain in [`Blocklist::text`] and, for blocked domains, the first list blocking it,
/// packed as `offset:32 | len:16 | list:16`.
#[derive(Clone, Copy)]
struct Entry(u64);

impl Entry {
    /// `None` if a part does not fit its bits.
    fn new(offset: usize, len: usize, list: usize) -> Option<Self> {
        let offset = u64::from(u32::try_from(offset).ok()?);
        let len = u64::from(u16::try_from(len).ok()?);
        let list = u64::from(u16::try_from(list).ok()?);
        Some(Self(offset << 32 | len << 16 | list))
    }

    fn range(self) -> Range<usize> {
        let offset = (self.0 >> 32) as usize;
        let len = (self.0 >> 16 & 0xffff) as usize;
        offset..offset + len
    }

    fn list(self) -> usize {
        (self.0 & 0xffff) as usize
    }
}

impl Blocklist {
    pub fn blocked_count(&self) -> usize {
        self.blocked.len()
    }

    pub fn allowed_count(&self) -> usize {
        self.allowed.len()
    }

    /// The list blocking `domain` or one of its parents, unless any of them is allowed.
    /// `domain` must be lowercase; a trailing dot is ignored.
    pub fn blocked_by(&self, domain: &str) -> Option<&Arc<str>> {
        let domain = domain.strip_suffix('.').unwrap_or(domain);
        if domain.is_empty()
            || suffixes(domain).any(|suffix| self.find(&self.allowed, suffix).is_some())
        {
            return None;
        }
        suffixes(domain)
            .find_map(|suffix| self.find(&self.blocked, suffix))
            .map(|entry| &self.lists[entry.list()])
    }

    fn find(&self, table: &HashTable<Entry>, domain: &str) -> Option<Entry> {
        let hash = self.hasher.hash_one(domain);
        table
            .find(hash, |entry| &self.text[entry.range()] == domain)
            .copied()
    }
}

/// Collects the rules of all lists into a [`Blocklist`].
#[derive(Default)]
pub struct BlocklistBuilder {
    blocklist: Blocklist,
    /// Domains that did not fit an [`Entry`].
    dropped: usize,
}

impl BlocklistBuilder {
    /// Blocks `domain` and its subdomains, unless an earlier list already blocks it.
    pub fn block(&mut self, domain: &str, list: &Arc<str>) {
        let lists = &mut self.blocklist.lists;
        if !lists.last().is_some_and(|last| Arc::ptr_eq(last, list)) {
            lists.push(list.clone());
        }
        let list = lists.len() - 1;
        self.insert(Table::Blocked, domain, list);
    }

    pub fn allow(&mut self, domain: &str) {
        self.insert(Table::Allowed, domain, 0);
    }

    pub fn build(mut self) -> Blocklist {
        if self.dropped > 0 {
            warn!(
                dropped = self.dropped,
                "the lists hold more domains than supported; some domains are ignored"
            );
        }
        let Blocklist {
            text,
            blocked,
            allowed,
            lists,
            hasher,
        } = &mut self.blocklist;
        let rehash = |entry: &Entry| hasher.hash_one(&text[entry.range()]);
        blocked.shrink_to_fit(rehash);
        allowed.shrink_to_fit(rehash);
        text.shrink_to_fit();
        lists.shrink_to_fit();
        self.blocklist
    }

    fn insert(&mut self, table: Table, domain: &str, list: usize) {
        let Blocklist {
            text,
            blocked,
            allowed,
            hasher,
            ..
        } = &mut self.blocklist;
        let table = match table {
            Table::Blocked => blocked,
            Table::Allowed => allowed,
        };
        let hash = hasher.hash_one(domain);
        if table
            .find(hash, |entry| &text[entry.range()] == domain)
            .is_some()
        {
            return;
        }
        let Some(entry) = Entry::new(text.len(), domain.len(), list) else {
            self.dropped += 1;
            return;
        };
        text.push_str(domain);
        table.insert_unique(hash, entry, |entry| hasher.hash_one(&text[entry.range()]));
    }
}

#[derive(Clone, Copy)]
enum Table {
    Blocked,
    Allowed,
}

/// `a.b.c`, `b.c`, `c`.
fn suffixes(domain: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(Some(domain), |current| {
        current.split_once('.').map(|(_, parent)| parent)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocklist(blocked: &[(&str, &Arc<str>)], allowed: &[&str]) -> Blocklist {
        let mut builder = BlocklistBuilder::default();
        for (domain, list) in blocked {
            builder.block(domain, list);
        }
        for domain in allowed {
            builder.allow(domain);
        }
        builder.build()
    }

    #[test]
    fn blocks_domains_and_their_subdomains() {
        let ads: Arc<str> = Arc::from("ads-list");
        let blocklist = blocklist(&[("ads.example.com", &ads)], &[]);

        assert_eq!(blocklist.blocked_by("ads.example.com."), Some(&ads));
        assert_eq!(blocklist.blocked_by("x.y.ads.example.com."), Some(&ads));
        assert_eq!(blocklist.blocked_by("example.com."), None);
        assert_eq!(blocklist.blocked_by("bads.example.com."), None);
        assert_eq!(blocklist.blocked_by("."), None);
    }

    #[test]
    fn allowed_domains_win_over_blocked_parents_and_children() {
        let list: Arc<str> = Arc::from("list");
        let blocklist = blocklist(
            &[("example.com", &list), ("ads.good.example.org", &list)],
            &["cdn.example.com", "good.example.org"],
        );

        assert_eq!(blocklist.blocked_by("cdn.example.com."), None);
        assert_eq!(blocklist.blocked_by("img.cdn.example.com."), None);
        assert_eq!(blocklist.blocked_by("ads.good.example.org."), None);
        assert_eq!(blocklist.blocked_by("www.example.com."), Some(&list));
    }

    #[test]
    fn the_first_list_blocking_a_domain_is_reported() {
        let (first, second): (Arc<str>, Arc<str>) = (Arc::from("first"), Arc::from("second"));
        let blocklist = blocklist(
            &[
                ("ads.example.com", &first),
                ("ads.example.com", &second),
                ("tracker.example.com", &second),
            ],
            &[],
        );

        assert_eq!(blocklist.blocked_by("ads.example.com"), Some(&first));
        assert_eq!(blocklist.blocked_by("tracker.example.com"), Some(&second));
        assert_eq!(blocklist.blocked_count(), 2);
    }

    #[test]
    fn domains_that_do_not_fit_an_entry_are_ignored() {
        let list: Arc<str> = Arc::from("list");
        let too_long = "a".repeat(usize::from(u16::MAX) + 1);
        let blocklist = blocklist(&[(&too_long, &list), ("ads.example.com", &list)], &[]);

        assert_eq!(blocklist.blocked_by(&too_long), None);
        assert_eq!(blocklist.blocked_by("ads.example.com"), Some(&list));
        assert_eq!(blocklist.blocked_count(), 1);
    }
}
