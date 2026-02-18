use crate::net::{
    DnsResolver, HttpMeta, NetworkPolicy, TokioDnsResolver, WebsocketMeta, is_private_ip,
};
use http::Method;
use std::{net::IpAddr, sync::Arc, time::Duration};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
    Ws,
    Wss,
}

impl Scheme {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "http" => Some(Self::Http),
            "https" => Some(Self::Https),
            "ws" => Some(Self::Ws),
            "wss" => Some(Self::Wss),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum HostMatch {
    /// Pre-normalized: trimmed of trailing dots and lowercased.
    Exact(String),
    /// Pre-normalized: trimmed of leading/trailing dots and lowercased.
    Suffix(String),
}

impl HostMatch {
    /// Create an exact host match. Normalizes at construction time.
    fn new_exact(host: impl Into<String>) -> Self {
        Self::Exact(host.into().trim_end_matches('.').to_ascii_lowercase())
    }

    /// Create a suffix host match. Normalizes at construction time.
    fn new_suffix(suffix: impl Into<String>) -> Self {
        Self::Suffix(
            suffix
                .into()
                .trim_start_matches('.')
                .trim_end_matches('.')
                .to_ascii_lowercase(),
        )
    }

    fn matches(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        match self {
            Self::Exact(expected) => host == *expected,
            Self::Suffix(bare) => {
                host == *bare
                    || host
                        .strip_suffix(bare.as_str())
                        .is_some_and(|prefix| prefix.ends_with('.'))
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PortRange {
    pub start: u16,
    pub end: u16,
}

impl PortRange {
    #[must_use]
    pub const fn single(port: u16) -> Self {
        Self {
            start: port,
            end: port,
        }
    }

    #[must_use]
    pub const fn range(start: u16, end: u16) -> Self {
        Self { start, end }
    }

    const fn contains(self, port: u16) -> bool {
        self.start <= port && port <= self.end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Allow,
    Deny,
}

#[derive(Debug, Clone)]
pub struct Rule {
    action: Action,
    schemes: Vec<Scheme>,
    methods: Vec<Method>,
    host: Option<HostMatch>,
    ports: Vec<PortRange>,
}

impl Rule {
    #[must_use]
    pub const fn allow() -> Self {
        Self::new(Action::Allow)
    }

    #[must_use]
    pub const fn deny() -> Self {
        Self::new(Action::Deny)
    }

    const fn new(action: Action) -> Self {
        Self {
            action,
            schemes: Vec::new(),
            methods: Vec::new(),
            host: None,
            ports: Vec::new(),
        }
    }

    #[must_use]
    pub fn schemes(mut self, schemes: impl IntoIterator<Item = Scheme>) -> Self {
        self.schemes = schemes.into_iter().collect();
        self
    }

    #[must_use]
    pub fn methods(mut self, methods: impl IntoIterator<Item = Method>) -> Self {
        self.methods = methods.into_iter().collect();
        self
    }

    #[must_use]
    pub fn host_exact(mut self, host: impl Into<String>) -> Self {
        self.host = Some(HostMatch::new_exact(host));
        self
    }

    #[must_use]
    pub fn host_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.host = Some(HostMatch::new_suffix(suffix));
        self
    }

    #[must_use]
    pub fn ports(mut self, ports: impl IntoIterator<Item = PortRange>) -> Self {
        self.ports = ports.into_iter().collect();
        self
    }

    fn matches(&self, scheme: Scheme, host: &str, port: u16, method: Option<&Method>) -> bool {
        if !self.schemes.is_empty() && !self.schemes.contains(&scheme) {
            return false;
        }
        if let Some(m) = method {
            if !self.methods.is_empty() && !self.methods.iter().any(|x| x == m) {
                return false;
            }
        } else if !self.methods.is_empty() {
            return false;
        }

        if let Some(host_match) = &self.host
            && !host_match.matches(host)
        {
            return false;
        }

        if !self.ports.is_empty() && !self.ports.iter().copied().any(|r| r.contains(port)) {
            return false;
        }

        true
    }
}

#[derive(Clone)]
pub struct AclPolicy {
    rules: Arc<Vec<Rule>>,
    deny_private_ranges: bool,
    resolver: Arc<dyn DnsResolver>,
    dns_timeout: Duration,
    dns_max_addrs: usize,
}

impl core::fmt::Debug for AclPolicy {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AclPolicy")
            .field("rules", &self.rules)
            .field("deny_private_ranges", &self.deny_private_ranges)
            .field("dns_timeout", &self.dns_timeout)
            .field("dns_max_addrs", &self.dns_max_addrs)
            .finish_non_exhaustive()
    }
}

pub struct AclPolicyBuilder {
    rules: Vec<Rule>,
    deny_private_ranges: bool,
    resolver: Arc<dyn DnsResolver>,
    dns_timeout: Duration,
    dns_max_addrs: usize,
}

impl Default for AclPolicyBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AclPolicyBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            deny_private_ranges: true,
            resolver: Arc::new(TokioDnsResolver),
            dns_timeout: Duration::from_secs(1),
            dns_max_addrs: 16,
        }
    }

    #[must_use]
    pub fn push(mut self, rule: Rule) -> Self {
        self.rules.push(rule);
        self
    }

    #[must_use]
    pub const fn deny_private_ranges(mut self, deny: bool) -> Self {
        self.deny_private_ranges = deny;
        self
    }

    #[must_use]
    pub const fn dns_timeout(mut self, timeout: Duration) -> Self {
        self.dns_timeout = timeout;
        self
    }

    #[must_use]
    pub const fn dns_max_addrs(mut self, max: usize) -> Self {
        self.dns_max_addrs = max;
        self
    }

    #[must_use]
    pub fn resolver(mut self, resolver: Arc<dyn DnsResolver>) -> Self {
        self.resolver = resolver;
        self
    }

    #[must_use]
    pub fn build(self) -> AclPolicy {
        AclPolicy {
            rules: Arc::new(self.rules),
            deny_private_ranges: self.deny_private_ranges,
            resolver: self.resolver,
            dns_timeout: self.dns_timeout,
            dns_max_addrs: self.dns_max_addrs,
        }
    }
}

struct ParsedUrl {
    scheme: Scheme,
    host: String,
    port: u16,
}

fn parse_url(uri_str: &str) -> Result<ParsedUrl, String> {
    let url = Url::parse(uri_str).map_err(|e| format!("invalid url: {e}"))?;
    let scheme = Scheme::parse(url.scheme()).ok_or_else(|| "unsupported scheme".to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "missing host".to_string())?
        .to_string();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "missing port".to_string())?;
    Ok(ParsedUrl { scheme, host, port })
}

impl AclPolicy {
    async fn check_private(&self, host: &str, port: u16) -> Result<(), String> {
        if let Ok(ip) = host.parse::<IpAddr>() {
            if is_private_ip(ip) {
                return Err(format!("destination ip prohibited: {ip}"));
            }
            return Ok(());
        }

        let addrs = tokio::time::timeout(self.dns_timeout, self.resolver.resolve(host, port))
            .await
            .map_err(|_e| "dns timeout".to_string())?
            .map_err(|e| format!("dns error: {e}"))?;

        for ip in addrs.into_iter().take(self.dns_max_addrs) {
            if is_private_ip(ip) {
                return Err(format!("destination ip prohibited: {ip}"));
            }
        }

        Ok(())
    }

    async fn check(
        &self,
        uri_str: &str,
        method: Option<&Method>,
    ) -> core::result::Result<(), String> {
        let ParsedUrl { scheme, host, port } = parse_url(uri_str)?;

        if self.deny_private_ranges {
            self.check_private(&host, port).await?;
        }

        for (idx, rule) in self.rules.iter().enumerate() {
            if rule.matches(scheme, &host, port, method) {
                return match rule.action {
                    Action::Allow => Ok(()),
                    Action::Deny => Err(format!("denied by rule #{idx}")),
                };
            }
        }

        Err("no ACL rule matched".to_string())
    }
}

#[async_trait::async_trait]
impl NetworkPolicy for AclPolicy {
    async fn check_http(&self, meta: &HttpMeta) -> core::result::Result<(), String> {
        self.check(&meta.uri.to_string(), Some(&meta.method)).await
    }

    async fn check_websocket(&self, meta: &WebsocketMeta) -> core::result::Result<(), String> {
        self.check(&meta.uri.to_string(), None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::DnsResolver;
    use std::io;

    #[derive(Debug)]
    struct StaticResolver(Vec<IpAddr>);

    #[async_trait::async_trait]
    impl DnsResolver for StaticResolver {
        async fn resolve(&self, _host: &str, _port: u16) -> io::Result<Vec<IpAddr>> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn deny_private_ip_literal() {
        let policy = AclPolicyBuilder::new()
            .deny_private_ranges(true)
            .push(Rule::allow().schemes([Scheme::Http, Scheme::Https]))
            .build();

        let meta = HttpMeta {
            method: Method::GET,
            uri: "http://127.0.0.1/".parse().unwrap(),
        };

        assert!(policy.check_http(&meta).await.is_err());
    }

    #[tokio::test]
    async fn deny_private_ip_via_dns() {
        let policy = AclPolicyBuilder::new()
            .deny_private_ranges(true)
            .resolver(Arc::new(StaticResolver(vec!["10.0.0.1".parse().unwrap()])))
            .push(Rule::allow().schemes([Scheme::Http, Scheme::Https]))
            .build();

        let meta = HttpMeta {
            method: Method::GET,
            uri: "http://example.com/".parse().unwrap(),
        };

        assert!(policy.check_http(&meta).await.is_err());
    }

    #[tokio::test]
    async fn allow_by_rule() {
        let policy = AclPolicyBuilder::new()
            .deny_private_ranges(true)
            .resolver(Arc::new(StaticResolver(vec![
                "93.184.216.34".parse().unwrap(),
            ])))
            .push(
                Rule::allow()
                    .schemes([Scheme::Http, Scheme::Https])
                    .methods([Method::GET])
                    .host_exact("example.com")
                    .ports([PortRange::single(80)]),
            )
            .build();

        let meta = HttpMeta {
            method: Method::GET,
            uri: "http://example.com/".parse().unwrap(),
        };

        assert!(policy.check_http(&meta).await.is_ok());
    }

    #[tokio::test]
    async fn defaults_deny_private_ranges() {
        let policy = AclPolicyBuilder::new()
            .push(Rule::allow().schemes([Scheme::Http, Scheme::Https]))
            .build();

        let meta = HttpMeta {
            method: Method::GET,
            uri: "http://127.0.0.1/".parse().unwrap(),
        };

        assert!(policy.check_http(&meta).await.is_err());
    }

    #[test]
    fn host_suffix_matches_subdomain() {
        let m = HostMatch::new_suffix("example.com");
        assert!(m.matches("example.com"));
        assert!(m.matches("foo.example.com"));
        assert!(m.matches("bar.foo.example.com"));
        assert!(!m.matches("notexample.com"));
        assert!(!m.matches("example.com.evil.com"));
    }

    #[test]
    fn host_suffix_with_leading_dot() {
        let m = HostMatch::new_suffix(".example.com");
        assert!(m.matches("example.com"));
        assert!(m.matches("foo.example.com"));
        assert!(!m.matches("notexample.com"));
    }

    #[test]
    fn host_suffix_trailing_dot() {
        let m = HostMatch::new_suffix("example.com.");
        assert!(m.matches("example.com"));
        assert!(m.matches("example.com."));
        assert!(m.matches("foo.example.com"));
    }
}
