//! Parses blocklists in hosts, plain domain and adblock (`||domain^`, `@@||domain^`) format.
//!
//! Every rule applies to the domain and all its subdomains. Lines in other syntax, such as
//! cosmetic filters, wildcards or rules with `$` modifiers other than `$important`, are
//! counted and skipped.

use std::net::IpAddr;

/// Names in hosts files that map local addresses rather than block anything.
const LOCAL_HOSTNAMES: &[&str] = &[
    "localhost",
    "localhost.localdomain",
    "local",
    "broadcasthost",
    "ip6-localhost",
    "ip6-loopback",
    "ip6-localnet",
    "ip6-mcastprefix",
    "ip6-allnodes",
    "ip6-allrouters",
    "ip6-allhosts",
];
const MAX_DOMAIN_LENGTH: usize = 253;
const MAX_LABEL_LENGTH: usize = 63;

/// A normalized domain and what the list says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule<'a> {
    Block(&'a str),
    /// From `@@||domain^` exception rules.
    Allow(&'a str),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ParsedList {
    /// Rules found, including duplicates.
    pub domains: usize,
    pub unsupported_lines: usize,
}

/// Passes every rule to `on_rule`, in list order. Domains are normalized into one reused
/// buffer, so parsing allocates nothing per domain.
pub fn parse(text: &str, mut on_rule: impl FnMut(Rule)) -> ParsedList {
    let mut domains = 0;
    let mut unsupported_lines = 0;
    let mut buffer = String::new();
    let mut count_and_pass = |rule: Rule| {
        domains += 1;
        on_rule(rule);
    };
    for line in text.lines() {
        let line = without_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        if !parse_line(line, &mut buffer, &mut count_and_pass) {
            unsupported_lines += 1;
        }
    }
    ParsedList {
        domains,
        unsupported_lines,
    }
}

/// Normalizes a single domain: lowercase, without trailing dot. `None` if it is not a
/// valid domain name.
pub fn parse_domain(text: &str) -> Option<Box<str>> {
    normalize(text, &mut String::new()).map(Box::from)
}

fn normalize<'a>(text: &str, buffer: &'a mut String) -> Option<&'a str> {
    buffer.clear();
    buffer.push_str(text.strip_suffix('.').unwrap_or(text));
    buffer.make_ascii_lowercase();
    let is_valid_label = |label: &str| {
        !label.is_empty()
            && label.len() <= MAX_LABEL_LENGTH
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    };
    if buffer.len() > MAX_DOMAIN_LENGTH || !buffer.split('.').all(is_valid_label) {
        return None;
    }
    Some(buffer)
}

/// Returns whether the whole line was understood.
fn parse_line(line: &str, buffer: &mut String, on_rule: &mut impl FnMut(Rule)) -> bool {
    if let Some(rule) = line.strip_prefix("@@||") {
        return pass(adblock_domain(rule, buffer).map(Rule::Allow), on_rule);
    }
    if let Some(rule) = line.strip_prefix("||") {
        return pass(adblock_domain(rule, buffer).map(Rule::Block), on_rule);
    }

    let mut tokens = line.split_whitespace();
    let Some(first) = tokens.next() else {
        return true;
    };
    if first.parse::<IpAddr>().is_ok() {
        let mut understood = true;
        for host in tokens.filter(|host| !LOCAL_HOSTNAMES.contains(host)) {
            understood &= pass(normalize(host, buffer).map(Rule::Block), on_rule);
        }
        return understood;
    }
    if tokens.next().is_some() {
        return false;
    }
    pass(normalize(first, buffer).map(Rule::Block), on_rule)
}

/// The domain of `domain^`. A trailing `|` and the `$important` modifier change nothing for
/// whole domains, so they are ignored; other modifiers and wildcards are not supported.
fn adblock_domain<'a>(rule: &str, buffer: &'a mut String) -> Option<&'a str> {
    let rule = rule.strip_suffix("$important").unwrap_or(rule);
    let rule = rule.strip_suffix('|').unwrap_or(rule);
    normalize(rule.strip_suffix('^')?, buffer)
}

fn pass(rule: Option<Rule>, on_rule: &mut impl FnMut(Rule)) -> bool {
    let Some(rule) = rule else {
        return false;
    };
    on_rule(rule);
    true
}

/// Strips `#` and `!` comments. In adblock syntax `#` also starts cosmetic filters like
/// `example.com##.ad`, so mid-line `#` only counts after whitespace, as in hosts files.
fn without_comment(line: &str) -> &str {
    let line = line.trim_start();
    if line.starts_with('#') || line.starts_with('!') {
        return "";
    }
    match line
        .find([' ', '\t'])
        .and_then(|space| line[space..].find('#').map(|hash| space + hash))
    {
        Some(comment) => &line[..comment],
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct Collected {
        blocked: Vec<String>,
        allowed: Vec<String>,
        list: ParsedList,
    }

    fn collect(text: &str) -> Collected {
        let mut collected = Collected::default();
        collected.list = parse(text, |rule| match rule {
            Rule::Block(domain) => collected.blocked.push(domain.to_owned()),
            Rule::Allow(domain) => collected.allowed.push(domain.to_owned()),
        });
        collected
    }

    #[test]
    fn parses_hosts_files() {
        let list = collect(
            "# comment\n\
             127.0.0.1 localhost\n\
             ::1 ip6-localhost ip6-loopback\n\
             0.0.0.0 ads.example.com tracker.example.net # inline comment\n\
             0.0.0.0\tTelemetry.Example.org.\n",
        );

        assert_eq!(
            list.blocked,
            [
                "ads.example.com",
                "tracker.example.net",
                "telemetry.example.org"
            ]
        );
        assert_eq!(
            list.list,
            ParsedList {
                domains: 3,
                unsupported_lines: 0
            }
        );
    }

    #[test]
    fn takes_every_valid_host_of_a_hosts_line_with_invalid_ones() {
        let list = collect("0.0.0.0 ads.example.com *.bad.example tracker.example.net\n");

        assert_eq!(list.blocked, ["ads.example.com", "tracker.example.net"]);
        assert_eq!(list.list.unsupported_lines, 1);
    }

    #[test]
    fn parses_plain_domain_lists() {
        let list = collect("ads.example.com\n\n  tracker.example.net  \n");

        assert_eq!(list.blocked, ["ads.example.com", "tracker.example.net"]);
    }

    #[test]
    fn parses_adblock_domain_rules_and_exceptions() {
        let list = collect(
            "[Adblock Plus 2.0]\n\
             ! Title: example\n\
             ||ads.example.com^\n\
             @@||good.ads.example.com^\n\
             ||important.example^$important\n\
             @@||anchored.example^|\n\
             ||third-party.example^$third-party\n\
             example.com##.banner\n\
             ||*.wildcard.example^\n\
             /regex/\n",
        );

        assert_eq!(list.blocked, ["ads.example.com", "important.example"]);
        assert_eq!(list.allowed, ["good.ads.example.com", "anchored.example"]);
        assert_eq!(
            list.list,
            ParsedList {
                domains: 4,
                unsupported_lines: 5
            }
        );
    }

    #[test]
    fn rejects_invalid_domains() {
        for invalid in ["", "a..b", "exa mple.com", "*.example.com", "ex/ample"] {
            assert_eq!(parse_domain(invalid), None, "{invalid:?}");
        }
        assert_eq!(
            parse_domain("_dmarc.Example.COM.").as_deref(),
            Some("_dmarc.example.com")
        );
    }
}
