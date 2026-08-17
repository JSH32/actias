//! Outbound egress policy for script-initiated http requests.
//!
//! Scripts are untrusted code running next to internal services, so every
//! outbound request is checked at three layers: the url before the request is
//! built (the only place a literal ip appears), every address dns resolves to
//! (so a hostname cannot smuggle in a private address), and every redirect hop
//! (so a public server cannot bounce the client to a literal private ip).

use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
    time::Duration,
};

/// An outbound destination a script may not reach.
///
/// The message names the rejected destination and nothing else, because it is
/// shown to script authors.
#[derive(Debug)]
pub struct EgressDenied(String);

impl std::fmt::Display for EgressDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "outbound request denied: {}", self.0)
    }
}

impl std::error::Error for EgressDenied {}

/// Which destinations outbound requests may reach.
pub struct EgressPolicy {
    /// Hostnames denied before resolution, lowercase; the platform's own
    /// service names go here.
    denied_hosts: HashSet<String>,
    /// Permits private and local addresses; for local development only.
    allow_private: bool,
}

impl EgressPolicy {
    pub fn new(denied_hosts: impl IntoIterator<Item = String>, allow_private: bool) -> Self {
        Self {
            denied_hosts: denied_hosts
                .into_iter()
                .map(|host| host.to_ascii_lowercase())
                .collect(),
            allow_private,
        }
    }

    /// Checks a url before any request is made and again at each redirect hop.
    ///
    /// Hostnames are re-checked at resolution time; literal ips only pass
    /// through here, because they never reach dns.
    pub fn check_url(&self, url: &url::Url) -> Result<(), EgressDenied> {
        match url.host() {
            Some(url::Host::Domain(host)) => self.check_host(host),
            Some(url::Host::Ipv4(ip)) => self.check_ip(IpAddr::V4(ip)),
            Some(url::Host::Ipv6(ip)) => self.check_ip(IpAddr::V6(ip)),
            None => Err(EgressDenied("the url has no host".into())),
        }
    }

    /// Checks a hostname against the denied list.
    pub fn check_host(&self, host: &str) -> Result<(), EgressDenied> {
        if self.denied_hosts.contains(&host.to_ascii_lowercase()) {
            return Err(EgressDenied(format!("'{host}' is not reachable")));
        }

        Ok(())
    }

    /// Checks one concrete address, wherever it came from.
    pub fn check_ip(&self, ip: IpAddr) -> Result<(), EgressDenied> {
        if !self.allow_private && ip_is_local(ip) {
            return Err(EgressDenied(format!(
                "'{ip}' is a private or local address"
            )));
        }

        Ok(())
    }
}

/// Whether `ip` addresses this machine or a network the platform runs on
/// rather than the public internet.
fn ip_is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4_is_local(v4),
        IpAddr::V6(v6) => {
            // A v4-mapped address connects to the v4 network, so it is judged
            // by the v4 rules, not by its v6 spelling.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return v4_is_local(mapped);
            }

            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Unique local fc00::/7 and link local fe80::/10.
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

fn v4_is_local(ip: Ipv4Addr) -> bool {
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        // Carrier-grade nat, 100.64.0.0/10.
        || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
}

/// Dns resolver that refuses lookups the policy denies.
///
/// Runs for every connection reqwest makes through a hostname, including
/// redirect targets, so a domain resolving to a private address is stopped
/// here no matter how the request reached it.
struct GuardedResolver {
    policy: Arc<EgressPolicy>,
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let policy = self.policy.clone();

        Box::pin(async move {
            policy.check_host(name.as_str())?;

            let addrs: Vec<_> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();

            // One denied address poisons the whole lookup; serving only the
            // public subset would let a half-private domain probe the network.
            for addr in &addrs {
                policy.check_ip(addr.ip())?;
            }

            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// An http client that enforces an [`EgressPolicy`] on everything it sends.
///
/// Built once and cloned everywhere: clones share one connection pool, and
/// the policy rides along for the pre-request literal-ip check.
#[derive(Clone)]
pub struct EgressClient {
    pub client: reqwest::Client,
    pub policy: Arc<EgressPolicy>,
}

impl EgressClient {
    /// # Errors
    /// Returns [`reqwest::Error`] when the underlying client cannot be built.
    pub fn new(policy: EgressPolicy) -> reqwest::Result<Self> {
        let policy = Arc::new(policy);

        let redirects = {
            let policy = policy.clone();
            reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() > 5 {
                    return attempt.error(EgressDenied("too many redirects".into()));
                }

                match policy.check_url(attempt.url()) {
                    Ok(()) => attempt.follow(),
                    Err(denied) => attempt.error(denied),
                }
            })
        };

        let client = reqwest::Client::builder()
            .redirect(redirects)
            .dns_resolver(Arc::new(GuardedResolver {
                policy: policy.clone(),
            }))
            .connect_timeout(Duration::from_secs(10))
            .build()?;

        Ok(Self { client, policy })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deny_all() -> EgressPolicy {
        EgressPolicy::new([], false)
    }

    #[test]
    fn local_and_private_addresses_are_denied() {
        // Every range here is a network the worker itself lives on; reaching
        // any of them from a script is server-side request forgery.
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254", // cloud metadata endpoint
            "100.64.0.1",      // carrier-grade nat
            "0.0.0.0",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:10.0.0.1", // v4-mapped spelling of a private address
        ] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(
                deny_all().check_ip(ip).is_err(),
                "{ip} should have been denied"
            );
        }
    }

    #[test]
    fn public_addresses_are_allowed() {
        for ip in ["1.1.1.1", "8.8.8.8", "93.184.216.34", "2606:4700::1111"] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(deny_all().check_ip(ip).is_ok(), "{ip} should have passed");
        }
    }

    #[test]
    fn allow_private_opens_local_ranges_for_development() {
        let policy = EgressPolicy::new([], true);
        assert!(policy.check_ip("127.0.0.1".parse().unwrap()).is_ok());
        assert!(policy.check_ip("10.0.0.1".parse().unwrap()).is_ok());
    }

    #[test]
    fn denied_hostnames_are_rejected_case_insensitively() {
        let policy = EgressPolicy::new(["script_service".to_owned()], false);

        assert!(policy.check_host("script_service").is_err());
        assert!(policy.check_host("SCRIPT_SERVICE").is_err());
        assert!(policy.check_host("example.com").is_ok());
    }

    #[test]
    fn a_denied_url_never_names_more_than_the_destination() {
        // The message reaches script authors; it must not describe topology.
        let error = deny_all()
            .check_url(&url::Url::parse("http://169.254.169.254/latest").unwrap())
            .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("169.254.169.254"), "unhelpful: {message}");
        assert!(!message.to_lowercase().contains("metadata"), "{message}");
    }
}
