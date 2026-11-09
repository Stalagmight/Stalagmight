//! TLS / private-network transport policy for the SpeleoDB client (#187).
//!
//! The `http://` scheme is a plaintext-credential hazard, so it is **blocked
//! unless every resolved address is loopback / private / link-local** - the
//! dev-box case (`http://10.10.0.220:8000`) is allowed, a public `http://` host
//! is refused before any byte is dialed. `https://` short-circuits before any
//! resolution (TLS protects the credentials regardless of the peer address). A
//! missing or unknown scheme is rejected outright (D-B19: require explicit
//! http/https, never silently rewrite).
//!
//! `insecure_http` (the persisted override, D-B10, owned by Track C config but
//! consumed here) relaxes **only** the public-http scheme block - it never
//! disables TLS certificate verification, so self-signed / internal-CA `https://`
//! endpoints stay rejected by ureq's TLS stack.
//!
//! [`enforce`] is called once per request at the top of `client::with_retry`
//! (#185 step 5), so a new client method physically cannot forget the check.
//!
//! NOTE (#187/D-187, risk 2): this is a guard against accidental
//! plaintext-to-public misconfiguration, NOT an adversarial SSRF / DNS-rebinding
//! defense. The address set is resolved + classified at check-time; a hostname
//! that resolves private at check-time and public at connect-time (TOCTOU /
//! DNS rebinding) is explicitly accepted as out of scope.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};

use http::Uri;

use super::SpeleoDbError;

/// Classification of a single resolved IP address for the http policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddrClass {
    Loopback,
    Private,
    LinkLocal,
    Public,
}

/// `true` for the three classes that are safe to reach over plaintext `http://`
/// (a private network the user already trusts); `false` for `Public`.
fn is_private(class: AddrClass) -> bool {
    !matches!(class, AddrClass::Public)
}

/// Classify an IP address into the #187 policy buckets. Pure (no DNS or socket
/// I/O), so it is the unit-test seam (D-T-unit). Delegates the actual range
/// checks to the standard library's audited predicates rather than hand-rolled
/// bit math, so this security-sensitive boundary can't drift from the RFC set.
///
/// IPv4: `is_loopback` (127/8) -> Loopback; `is_private` (10/8, 172.16/12,
/// 192.168/16) -> Private; `is_link_local` (169.254/16) -> LinkLocal; else Public.
/// IPv6: `is_loopback` (::1) -> Loopback; `is_unique_local` (fc00::/7) -> Private;
/// `is_unicast_link_local` (fe80::/10) -> LinkLocal; else Public.
/// An IPv4-mapped IPv6 address is unwrapped to its v4 form first, closing the
/// `::ffff:8.8.8.8` bypass.
fn classify_addr(ip: IpAddr) -> AddrClass {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        // IPv4-mapped (`::ffff:a.b.c.d`) must be classified by its v4 range,
        // not treated as an opaque public v6 address.
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(mapped) => classify_v4(mapped),
            None => classify_v6(v6),
        },
    }
}

fn classify_v4(v4: Ipv4Addr) -> AddrClass {
    if v4.is_loopback() {
        AddrClass::Loopback
    } else if v4.is_private() {
        AddrClass::Private
    } else if v4.is_link_local() {
        AddrClass::LinkLocal
    } else {
        AddrClass::Public
    }
}

fn classify_v6(v6: Ipv6Addr) -> AddrClass {
    if v6.is_loopback() {
        AddrClass::Loopback
    } else if v6.is_unique_local() {
        AddrClass::Private
    } else if v6.is_unicast_link_local() {
        AddrClass::LinkLocal
    } else {
        AddrClass::Public
    }
}

/// Resolve the URI's authority to the address set the agent will dial.
///
/// Uses [`DefaultResolver::host_and_port`] to assemble the exact `host:port`
/// ureq dials, then `ToSocketAddrs` to resolve it (the D-187-permitted fallback
/// - wiring the full `&Config`/`NextTimeout` resolver path is awkward and buys
/// nothing for a check that only needs the IP set). Returns an `InsecureEndpoint`
/// error on a missing authority or a resolution failure (fail closed).
fn resolve_addrs(uri: &Uri) -> Result<Vec<IpAddr>, SpeleoDbError> {
    let endpoint = uri.to_string();
    let host = uri.host().ok_or_else(|| SpeleoDbError::InsecureEndpoint {
        endpoint: endpoint.clone(),
    })?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let host_and_port = (host, uri.port_u16().unwrap_or(80));
    let addrs = host_and_port
        .to_socket_addrs()
        .map_err(|_| SpeleoDbError::InsecureEndpoint { endpoint })?;
    Ok(addrs.map(|sa| sa.ip()).collect())
}

/// `true` for `localhost` and any `*.localhost` label (case-insensitive). These
/// resolve to loopback by convention; special-casing them keeps the box-less
/// localhost dev path working without depending on a hosts-file entry.
fn is_localhost(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

/// Process-wide one-shot latch behind the unencrypted-connection notice: `true`
/// the first time any `http://` connection is allowed this session, `false`
/// forever after. A `static`, so the notice fires once per run regardless of
/// which endpoint or how many requests.
static UNENCRYPTED_NOTICE_EMITTED: AtomicBool = AtomicBool::new(false);

/// Return `true` exactly once - on the first call for a given `flag` - and
/// `false` on every later call. Factored out so the "once" semantics are
/// unit-testable with a fresh local flag, without a global or a log capture.
fn latch_once(flag: &AtomicBool) -> bool {
    !flag.swap(true, Ordering::Relaxed)
}

/// Emit the one-time unencrypted-connection notice (§4.3 E3). Called at the
/// FIRST allowed `http://` connection of the session: every `http://` is
/// plaintext (credentials + survey data in the clear), so the user gets a single
/// heads-up. Later allowed http connections are silent (the latch is spent).
fn notice_unencrypted_once(endpoint: &str) {
    if latch_once(&UNENCRYPTED_NOTICE_EMITTED) {
        tracing::warn!(
            endpoint,
            "Using an unencrypted http connection to SpeleoDB: credentials and \
             survey data are sent in plaintext. This is allowed only for a \
             trusted local/private network; prefer https otherwise."
        );
    }
}

/// Enforce the http/https transport policy for `endpoint`. See the module docs.
///
/// - `https://` → `Ok(())` immediately (no resolution, no notice - it is encrypted).
/// - `http://` → allowed only if every resolved address is private/loopback/
///   link-local (or `insecure_http` is set, which lifts the scheme block only);
///   the first allowed http connection of the session emits a one-time notice.
/// - missing / unknown scheme → `Err(InsecureEndpoint)`.
pub(crate) fn enforce(endpoint: &str, insecure_http: bool) -> Result<(), SpeleoDbError> {
    let uri = Uri::from_str(endpoint).map_err(|_| SpeleoDbError::InsecureEndpoint {
        endpoint: endpoint.to_owned(),
    })?;

    match uri.scheme_str() {
        // TLS protects the credentials regardless of peer address; never resolve.
        Some("https") => Ok(()),
        Some("http") => {
            let allowed = if insecure_http {
                // The persisted override lifts ONLY the public-http scheme block
                // (D-B10) - TLS cert verification on https is untouched.
                true
            } else if uri.host().is_some_and(is_localhost) {
                // localhost / *.localhost are loopback by convention (no DNS).
                true
            } else {
                // Empty resolution or any public address fails closed (D-187);
                // `resolve_addrs` returns Err (fail closed) on a resolution
                // failure, propagated here before the private-only check.
                let addrs = resolve_addrs(&uri)?;
                !addrs.is_empty()
                    && addrs
                        .iter()
                        .copied()
                        .all(|ip| is_private(classify_addr(ip)))
            };
            if allowed {
                // Any allowed http is plaintext - notice once per session (E3).
                notice_unencrypted_once(endpoint);
                Ok(())
            } else {
                Err(SpeleoDbError::InsecureEndpoint {
                    endpoint: endpoint.to_owned(),
                })
            }
        }
        // D-B19: require an explicit, known scheme - never silently rewrite.
        _ => Err(SpeleoDbError::InsecureEndpoint {
            endpoint: endpoint.to_owned(),
        }),
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
mod tests {
    use std::net::IpAddr;
    use std::str::FromStr;
    use std::sync::atomic::AtomicBool;

    use super::{classify_addr, enforce, latch_once, AddrClass};
    use crate::speleodb::SpeleoDbError;

    fn ip(s: &str) -> IpAddr {
        IpAddr::from_str(s).expect("test IP literal")
    }

    #[test]
    fn classify_addr_private_v4_ranges() {
        assert_eq!(classify_addr(ip("10.10.0.220")), AddrClass::Private);
        assert_eq!(classify_addr(ip("127.0.0.1")), AddrClass::Loopback);
        assert_eq!(classify_addr(ip("10.0.0.1")), AddrClass::Private);
        assert_eq!(classify_addr(ip("172.16.5.4")), AddrClass::Private);
        assert_eq!(classify_addr(ip("172.31.255.255")), AddrClass::Private);
        assert_eq!(classify_addr(ip("192.168.1.1")), AddrClass::Private);
        assert_eq!(classify_addr(ip("169.254.10.10")), AddrClass::LinkLocal);
    }

    #[test]
    fn classify_addr_public_v4_blocks() {
        assert_eq!(classify_addr(ip("8.8.8.8")), AddrClass::Public);
        // 172.32.0.1 is just past the 172.16/12 private block.
        assert_eq!(classify_addr(ip("172.32.0.1")), AddrClass::Public);
        assert_eq!(classify_addr(ip("172.15.0.1")), AddrClass::Public);
        assert_eq!(classify_addr(ip("1.1.1.1")), AddrClass::Public);
        // 192.169 is NOT 192.168.
        assert_eq!(classify_addr(ip("192.169.0.1")), AddrClass::Public);
    }

    #[test]
    fn classify_addr_v6() {
        assert_eq!(classify_addr(ip("::1")), AddrClass::Loopback);
        assert_eq!(classify_addr(ip("fc00::1")), AddrClass::Private);
        assert_eq!(classify_addr(ip("fd12:3456::1")), AddrClass::Private);
        assert_eq!(classify_addr(ip("fe80::1")), AddrClass::LinkLocal);
        assert_eq!(classify_addr(ip("2001:db8::1")), AddrClass::Public);
    }

    #[test]
    fn classify_addr_v4_mapped_stripped() {
        // ::ffff:8.8.8.8 must classify as the public v4 8.8.8.8, not as an
        // opaque public v6 (the result is the same here, but the mapped path is
        // what closes the ::ffff:10.0.0.1 → "looks public" bypass).
        assert_eq!(classify_addr(ip("::ffff:8.8.8.8")), AddrClass::Public);
        assert_eq!(classify_addr(ip("::ffff:10.0.0.1")), AddrClass::Private);
        assert_eq!(classify_addr(ip("::ffff:127.0.0.1")), AddrClass::Loopback);
    }

    #[test]
    fn enforce_https_short_circuits() {
        // A public https host is fine - TLS protects the credentials; enforce
        // must not resolve or block it.
        assert!(enforce("https://example.com", false).is_ok());
        assert!(enforce("https://8.8.8.8", false).is_ok());
    }

    #[test]
    fn enforce_http_public_blocked() {
        let err = enforce("http://8.8.8.8", false).expect_err("public http must be blocked");
        assert!(matches!(err, SpeleoDbError::InsecureEndpoint { .. }));
    }

    #[test]
    fn enforce_http_private_allowed() {
        // A literal private IP resolves to itself - no DNS needed. Acceptance
        // (#187): `10.10.0.220` AND `[::1]` are both accepted under the policy.
        assert!(enforce("http://10.10.0.220:8000", false).is_ok());
        assert!(enforce("http://127.0.0.1:8000", false).is_ok());
        assert!(enforce("http://192.168.1.50", false).is_ok());
        // IPv6 loopback literal, with and without a port - the bracketed
        // authority must resolve to `::1` and classify as Loopback.
        assert!(enforce("http://[::1]:8000", false).is_ok());
        assert!(enforce("http://[::1]", false).is_ok());
    }

    #[test]
    fn latch_once_fires_exactly_once() {
        // The one-time unencrypted-connection notice fires on the first allowed
        // http connection only. Tested with a fresh local flag so it does not
        // depend on the process-wide static or on capturing the log.
        let flag = AtomicBool::new(false);
        assert!(latch_once(&flag), "first call must fire");
        assert!(!latch_once(&flag), "second call must be silent");
        assert!(!latch_once(&flag), "and every call after");
    }

    #[test]
    fn enforce_localhost_resolution_branch() {
        // localhost / *.localhost are loopback by convention (no DNS).
        assert!(enforce("http://localhost:8000", false).is_ok());
        assert!(enforce("http://LOCALHOST:8000", false).is_ok());
        assert!(enforce("http://foo.localhost", false).is_ok());
        // And the classifier agrees the loopback literal is Loopback distinctly.
        assert_eq!(classify_addr(ip("127.0.0.1")), AddrClass::Loopback);
    }

    #[test]
    fn enforce_missing_scheme_rejected() {
        // No scheme: refuse rather than guess http/https (D-B19).
        let err = enforce("10.10.0.220:8000", false).expect_err("missing scheme must be rejected");
        assert!(matches!(err, SpeleoDbError::InsecureEndpoint { .. }));
        // An unknown scheme is likewise rejected.
        assert!(matches!(
            enforce("ftp://10.10.0.220", false),
            Err(SpeleoDbError::InsecureEndpoint { .. })
        ));
    }

    #[test]
    fn enforce_empty_resolution_fails_closed() {
        // A host that cannot resolve to any address fails closed with
        // InsecureEndpoint (never silently allowed).
        let err = enforce("http://this-host-does-not-exist.invalid", false)
            .expect_err("unresolvable http host must fail closed");
        assert!(matches!(err, SpeleoDbError::InsecureEndpoint { .. }));
    }

    #[test]
    fn enforce_insecure_http_bypasses_public_block() {
        // With the override set, a public http host is allowed (scheme block
        // lifted) - but a missing scheme is STILL rejected (the override only
        // relaxes the http policy, not the require-explicit-scheme rule).
        assert!(enforce("http://8.8.8.8", true).is_ok());
        assert!(enforce("http://example.com", true).is_ok());
        assert!(matches!(
            enforce("10.10.0.220:8000", true),
            Err(SpeleoDbError::InsecureEndpoint { .. })
        ));
    }
}
