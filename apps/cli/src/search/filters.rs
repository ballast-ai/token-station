//! Domain constraints apply to decoded source URLs, independently of search-engine query hints.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_matching_uses_dns_label_boundaries() {
        let filter = DomainFilter::new(&["Example.COM.".into()], &[]).unwrap();
        for url in ["https://example.com/a", "https://docs.example.com/b"] {
            assert!(filter.allows(url));
        }
        for url in [
            "https://notexample.com",
            "https://example.com.evil.org",
            "https://evil.org/?u=https://example.com",
            "https://example.com@evil.org",
        ] {
            assert!(!filter.allows(url), "{url}");
        }
        let blocked = DomainFilter::new(&[], &["example.com".into()]).unwrap();
        assert!(!blocked.allows("https://docs.example.com/a"));
        assert!(blocked.allows("https://other.org/a"));
    }

    #[test]
    fn invalid_domains_and_ambiguous_constraints_are_rejected() {
        for domain in [
            "",
            "https://example.com",
            "*.example.com",
            "example.com/path",
            "example.com:443",
            "user@example.com",
            "example.com?x",
            "example.com#x",
            "127.0.0.1",
            "localhost",
            "printer.local",
            "foo..com",
            "-foo.com",
            "foo_.com",
            "foo.com..",
            "foo.com OR site:evil.com",
        ] {
            assert!(
                DomainFilter::new(&[domain.into()], &[]).is_err(),
                "{domain}"
            );
        }
        assert!(DomainFilter::new(&vec!["example.com".into(); 21], &[]).is_err());
        assert!(DomainFilter::new(&["example.com".into()], &["other.org".into()]).is_err());
        let unicode = DomainFilter::new(&["bücher.de".into()], &[]).unwrap();
        assert!(unicode.allows("https://xn--bcher-kva.de"));
    }

    #[test]
    fn site_hints_keep_the_original_query_bounded() {
        let filter = DomainFilter::new(&["example.com".into(), "docs.rs".into()], &[]).unwrap();
        assert_eq!(
            filter.query("rust & tools").unwrap(),
            "(rust & tools) (site:example.com OR site:docs.rs)"
        );
        assert!(filter.query(&"a".repeat(501)).is_err());
        let blocked = DomainFilter::new(&[], &["example.com".into()]).unwrap();
        assert_eq!(blocked.query("rust").unwrap(), "(rust) -site:example.com");
    }
}

use url::{Host, Url};

#[derive(Default)]
pub(crate) struct DomainFilter {
    allowed: Vec<String>,
    blocked: Vec<String>,
}

impl DomainFilter {
    pub(crate) fn new(allowed: &[String], blocked: &[String]) -> Result<Self, String> {
        if allowed.len() > 20 || blocked.len() > 20 {
            return Err("Browser search supports at most 20 domains per filter.".into());
        }
        if !allowed.is_empty() && !blocked.is_empty() {
            return Err("Use either allowed domains or blocked domains.".into());
        }
        Ok(Self {
            allowed: allowed
                .iter()
                .map(|domain| normalize(domain))
                .collect::<Result<_, _>>()?,
            blocked: blocked
                .iter()
                .map(|domain| normalize(domain))
                .collect::<Result<_, _>>()?,
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.allowed.is_empty() && self.blocked.is_empty()
    }

    pub(crate) fn allows(&self, raw: &str) -> bool {
        let Some(url) = super::public_url(raw).and_then(|url| Url::parse(&url).ok()) else {
            return false;
        };
        let Some(host) = url.host_str() else {
            return false;
        };
        let host = host.trim_end_matches('.');
        let matches = |domain: &String| {
            host == domain
                || host
                    .strip_suffix(domain)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        };
        (self.allowed.is_empty() || self.allowed.iter().any(matches))
            && !self.blocked.iter().any(matches)
    }

    pub(crate) fn query(&self, query: &str) -> Result<String, String> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 500 {
            return Err("Enter a search query with 1 to 500 characters.".into());
        }
        if !self.allowed.is_empty() {
            Ok(format!(
                "({query}) ({})",
                self.allowed
                    .iter()
                    .map(|domain| format!("site:{domain}"))
                    .collect::<Vec<_>>()
                    .join(" OR ")
            ))
        } else if !self.blocked.is_empty() {
            Ok(format!(
                "({query}) {}",
                self.blocked
                    .iter()
                    .map(|domain| format!("-site:{domain}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ))
        } else {
            Ok(query.to_owned())
        }
    }
}

fn normalize(raw: &str) -> Result<String, String> {
    let invalid = || {
        "Search filters require public DNS domain names without schemes, paths, ports, or wildcards.".to_owned()
    };
    let raw = raw.strip_suffix('.').unwrap_or(raw);
    if raw.is_empty()
        || raw.chars().any(|c| {
            c.is_whitespace() || matches!(c, '/' | '\\' | ':' | '*' | '@' | '?' | '#' | '%')
        })
    {
        return Err(invalid());
    }
    let url = Url::parse(&format!("https://{raw}")).map_err(|_| invalid())?;
    let Some(Host::Domain(host)) = url.host() else {
        return Err(invalid());
    };
    if host.len() > 253
        || super::public_url(url.as_str()).is_none()
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(invalid());
    }
    Ok(host.to_owned())
}
