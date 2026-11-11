//! Transport policy for the SpeleoDB client (#187).
//!
//! `https://` is always allowed: TLS protects the credentials whatever the peer.
//! Plain `http://` sends the sign-in and the survey data unencrypted, so it is
//! allowed only to the automatic exceptions, compared as written (no DNS):
//!
//! - the exact host `localhost` (any letter case),
//! - the exact IPv4 literal `127.0.0.1`,
//! - an IPv4 literal in `192.168.0.0/16` (four decimal octets, no leading zeros),
//!
//! on any valid port. Every other `http://` address (a hostname, a public
//! address, the other private ranges, IPv6 literals, `localhost` aliases or
//! subdomains) is refused before any byte is dialed. A missing or unknown
//! scheme, a missing host, user info (`user@host`) or an invalid port is refused
//! as an unusable address (D-B19: require an explicit http/https, never
//! silently rewrite).
//!
//! The exception set is defined once, in [`is_automatic_http_exception`], and
//! described once, in [`PLAIN_HTTP_RULE`] (the frontend's translated line has
//! its own single description, `speleodb.httpExceptions`).
//!
//! `insecure_http` (the persisted `SpeleoDbConfig::insecure_http` override, or
//! the value a sign-in passes) lifts ONLY the http scheme rule - it never
//! disables TLS certificate verification, so self-signed / internal-CA
//! `https://` endpoints stay rejected by the TLS stack, and it never accepts an
//! unusable address. It is a DEVELOPMENT-BUILD option: a release build
//! (betas and release candidates included) treats it as `false` whatever the
//! config file or the caller says ([`INSECURE_HTTP_HONOURED`]). The policy is
//! the one place the value takes effect, so that is where it is ignored: no
//! caller has to remember to.
//!
//! [`enforce`] runs in `client::SdkClient::new`, before the SDK client exists,
//! so a new client method physically cannot forget the check. Sign-in calls
//! [`validate_endpoint`] (the same policy, without DNS) before it uses an
//! address, and a Settings save calls it whenever the save changes the server
//! or the `insecure_http` choice (`AppState::update_config`), so a refused
//! address is reported before it is saved. An address already saved is not
//! re-checked by a save that changes neither; [`enforce`] still refuses it on
//! every call.
//!
//! A hostname the override allowed over `http://` is resolved before the client
//! dials it: a host that does not resolve is UNREACHABLE, not insecure, so
//! [`enforce`] reports it as a `Transport { cause: Dns }` failure, and that typed
//! cause gets the offline handling (the cached open, sdk#6). An `https://`
//! request is not resolved here: its DNS failure surfaces from the SDK's send,
//! which `client::api_error` currently classifies, like a refused or reset
//! connection, as `TransportCause::Unknown` - and an unknown cause does not fall
//! back to the cached copy. So today an offline `https://` endpoint falls back
//! only on a timeout; the DNS and refused cases are the known gap F1, tracked in
//! #669.
//!
//! NOTE (#187): this is a guard against accidental plaintext misconfiguration,
//! NOT an adversarial SSRF / DNS-rebinding defense.

use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};

use http::Uri;

use super::{EndpointRefusal, SpeleoDbError, TransportCause};

/// Characters that copy-paste carries around an address without the user seeing
/// them: zero-width space/joiners, word joiner, BOM, soft hyphen and the bidi
/// marks/embeddings/isolates. None of them is Unicode `White_Space`, so
/// `str::trim` keeps them.
fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// Clean an endpoint as the user entered it (typed or pasted, #675): strip
/// leading and trailing whitespace and invisible format characters. `None` when
/// nothing is left. Interior characters are kept, so a genuinely malformed
/// address still fails [`validate_endpoint`] - this only removes what the user
/// cannot see and did not mean. Every place a configured endpoint enters the app
/// (startup load, settings save, settings reload, login override,
/// `DEV_SPELEODB_URL`) runs it before validating, and the cleaned value is what
/// gets stored. An endpoint read from a project file's SpeleoDB link is only
/// ever compared, through [`super::auth::normalize_endpoint`], which applies the
/// same cleaning. [`validate_endpoint`] itself stays strict: it is the transport
/// boundary and does not clean its input.
pub fn clean_endpoint_input(input: &str) -> Option<String> {
    let cleaned = input.trim_matches(|c: char| c.is_whitespace() || is_invisible_format(c));
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

/// `true` for the automatic plain-HTTP exceptions (see the module docs). Pure:
/// the host is compared as written, never resolved. The ONE definition of the
/// set: [`PLAIN_HTTP_RULE`] describes it, and changes with it.
fn is_automatic_http_exception(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || Ipv4Addr::from_str(host).is_ok_and(|ip| ip.octets()[..2] == [192, 168])
}

/// The plain-HTTP rule in English, naming [`is_automatic_http_exception`]'s set:
/// the Display text of a `PlainHttp` refusal (logs, and callers that show the
/// backend message as is).
pub(crate) const PLAIN_HTTP_RULE: &str =
    "HTTPS is required. HTTP is allowed only on localhost, 127.0.0.1 or 192.168.x.x.";

/// Whether this build honours an `insecure_http` choice at all. Only a
/// development build does (`debug_assertions`, like the other dev-only gates in
/// docs/debug-gates.md). A release build - betas and release candidates
/// included - offers no control for it and ignores a stored or requested value.
pub(crate) const INSECURE_HTTP_HONOURED: bool = cfg!(debug_assertions);

/// The `insecure_http` choice a build applies: `requested` in a development
/// build (`dev_build`), always `false` in a release one. Pure, so a test in a
/// debug build pins what a release build does.
const fn honoured_insecure_http(requested: bool, dev_build: bool) -> bool {
    requested && dev_build
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
             trusted local network; prefer https otherwise."
        );
    }
}

/// Enforce the transport policy for `endpoint` before credentials or requests
/// leave the application. See the module docs.
///
/// - `https://` -> `Ok(())`.
/// - `http://` -> allowed for the automatic exceptions, or anywhere when
///   `insecure_http` is set in a development build; the first allowed http
///   connection of the session emits a one-time notice. Otherwise
///   `Err(InsecureEndpoint { PlainHttp })`.
/// - missing / unknown scheme, no host, user info or an invalid port ->
///   `Err(InsecureEndpoint { InvalidAddress })`.
pub fn validate_endpoint(endpoint: &str, insecure_http: bool) -> Result<(), SpeleoDbError> {
    validate_endpoint_in(endpoint, insecure_http, INSECURE_HTTP_HONOURED)
}

/// [`validate_endpoint`] as a development (`dev_build`) or a release build
/// applies it.
fn validate_endpoint_in(
    endpoint: &str,
    insecure_http: bool,
    dev_build: bool,
) -> Result<(), SpeleoDbError> {
    let insecure_http = honoured_insecure_http(insecure_http, dev_build);
    let refuse = |refusal| SpeleoDbError::InsecureEndpoint {
        endpoint: endpoint.to_owned(),
        refusal,
    };
    let invalid = || refuse(EndpointRefusal::InvalidAddress);
    // The client dials a `reqwest::Url`: an address it cannot parse (a port out
    // of range or not a number, ...) is unusable whatever the scheme.
    reqwest::Url::parse(endpoint).map_err(|_| invalid())?;
    let uri = Uri::from_str(endpoint).map_err(|_| invalid())?;
    let authority = uri.authority().ok_or_else(invalid)?;
    // `user@host` hides the real host from whoever reads the address.
    if authority.as_str().contains('@') {
        return Err(invalid());
    }
    let host = uri.host().ok_or_else(invalid)?;
    match uri.scheme_str() {
        Some("https") => Ok(()),
        Some("http") if insecure_http || is_automatic_http_exception(host) => {
            // Any allowed http is plaintext - notice once per session (E3).
            notice_unencrypted_once(endpoint);
            Ok(())
        }
        Some("http") => Err(refuse(EndpointRefusal::PlainHttp)),
        // D-B19: require an explicit, known scheme - never silently rewrite.
        _ => Err(invalid()),
    }
}

/// [`validate_endpoint`], plus the resolution an allowed `http://` hostname
/// gets before the client dials it (see the module docs). The client's check.
pub(crate) fn enforce(endpoint: &str, insecure_http: bool) -> Result<(), SpeleoDbError> {
    enforce_with(
        endpoint,
        insecure_http,
        INSECURE_HTTP_HONOURED,
        resolve_addrs,
    )
}

/// Resolve `host:port` to the address set the client will dial, with the
/// system resolver (the one the transport uses).
fn resolve_addrs(host: &str, port: u16) -> std::io::Result<Vec<IpAddr>> {
    Ok((host, port).to_socket_addrs()?.map(|sa| sa.ip()).collect())
}

/// The DNS failure an unresolvable `http://` host reports: the same typed
/// transport error the request itself would have produced, never the policy
/// refusal. `detail` names the cause for the log; no URL rides in it.
fn unresolved(detail: impl std::fmt::Display) -> SpeleoDbError {
    SpeleoDbError::Transport {
        cause: TransportCause::Dns,
        detail: format!("could not resolve the SpeleoDB host: {detail}"),
    }
}

/// [`enforce`] with the build (`dev_build`, see [`validate_endpoint_in`]) and the
/// resolver injected, so the offline path is testable without DNS.
fn enforce_with(
    endpoint: &str,
    insecure_http: bool,
    dev_build: bool,
    resolve: impl FnOnce(&str, u16) -> std::io::Result<Vec<IpAddr>>,
) -> Result<(), SpeleoDbError> {
    validate_endpoint_in(endpoint, insecure_http, dev_build)?;
    // Validated above, so these only skip what needs no resolving: `https://`,
    // the automatic exceptions and IP literals.
    let Ok(uri) = Uri::from_str(endpoint) else {
        return Ok(());
    };
    let Some(host) = uri.host() else {
        return Ok(());
    };
    let literal = host.starts_with('[') || IpAddr::from_str(host).is_ok();
    if uri.scheme_str() != Some("http") || literal || is_automatic_http_exception(host) {
        return Ok(());
    }
    let addrs = resolve(host, uri.port_u16().unwrap_or(80)).map_err(unresolved)?;
    if addrs.is_empty() {
        return Err(unresolved("no addresses"));
    }
    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "unit test fixtures fail loudly on an unexpected answer"
)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::{
        clean_endpoint_input, enforce_with, honoured_insecure_http, latch_once, validate_endpoint,
        validate_endpoint_in, INSECURE_HTTP_HONOURED,
    };
    use crate::speleodb::{EndpointRefusal, SpeleoDbError, TransportCause};

    fn refusal(endpoint: &str, insecure_http: bool) -> EndpointRefusal {
        match validate_endpoint(endpoint, insecure_http) {
            Err(SpeleoDbError::InsecureEndpoint { refusal, .. }) => refusal,
            other => panic!("{endpoint}: expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn automatic_exceptions_and_https_are_allowed() {
        for endpoint in [
            "http://localhost",
            "http://LOCALHOST:8000/",
            "http://127.0.0.1:80",
            "http://127.0.0.1:8000",
            "http://192.168.0.0",
            "http://192.168.1.50",
            "http://192.168.255.255:8000/",
            "https://example.com",
            "https://8.8.8.8",
        ] {
            assert!(validate_endpoint(endpoint, false).is_ok(), "{endpoint}");
        }
    }

    #[test]
    fn other_http_addresses_are_refused_as_plain_http() {
        for endpoint in [
            "http://10.0.0.1",
            "http://10.10.0.220:8000",
            "http://172.16.0.1",
            "http://127.0.0.2",
            "http://[::1]",
            "http://[::1]:8000",
            "http://169.254.1.1",
            "http://8.8.8.8",
            "http://localhost.evil.com",
            "http://sub.localhost",
            "http://localhost.",
            "http://192.168.example.com",
            "http://192.169.0.1",
            "http://192.167.255.255",
            "http://example.com",
            "http://this-host-does-not-exist.invalid:8000",
        ] {
            assert_eq!(
                refusal(endpoint, false),
                EndpointRefusal::PlainHttp,
                "{endpoint}"
            );
        }
    }

    #[test]
    fn look_alikes_of_the_exceptions_are_refused() {
        // Short, decimal, octal-looking or out-of-range spellings of an exception
        // are not the exception, whichever parser rejects them first.
        for endpoint in [
            "http://127.1",
            "http://2130706433",
            "http://192.168.001.1",
            "http://192.168.1.256",
        ] {
            assert!(
                matches!(
                    validate_endpoint(endpoint, false),
                    Err(SpeleoDbError::InsecureEndpoint { .. })
                ),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn unusable_addresses_are_refused_as_invalid() {
        // D-B19: no or an unknown scheme is refused rather than guessed, as are
        // user info and a port the client could not dial.
        for endpoint in [
            "localhost:8000",
            "10.10.0.220:8000",
            "ftp://localhost",
            "http://localhost@evil.com",
            "https://user@example.com",
            "http://localhost:65536",
            "http://localhost:bad",
        ] {
            assert_eq!(
                refusal(endpoint, false),
                EndpointRefusal::InvalidAddress,
                "{endpoint}"
            );
        }
    }

    #[test]
    fn insecure_http_lifts_only_the_http_rule() {
        // In a development build, with the override set, any http host is
        // allowed - but an unusable address is STILL refused (the override
        // relaxes the scheme rule only).
        for endpoint in ["http://8.8.8.8", "http://example.com", "http://[::1]:8000"] {
            assert!(
                validate_endpoint_in(endpoint, true, true).is_ok(),
                "{endpoint}"
            );
        }
        for endpoint in [
            "10.10.0.220:8000",
            "ftp://example.com",
            "http://u@example.com",
        ] {
            assert!(
                matches!(
                    validate_endpoint_in(endpoint, true, true),
                    Err(SpeleoDbError::InsecureEndpoint {
                        refusal: EndpointRefusal::InvalidAddress,
                        ..
                    })
                ),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn a_release_build_ignores_insecure_http() {
        // Owner decision: the option exists only in development builds. A
        // release build (betas and release candidates included) applies the
        // automatic rules even when the config file or the caller asks for the
        // override, so an old config with `insecure_http = true` cannot reopen
        // plaintext to a public host.
        assert!(!honoured_insecure_http(true, false));
        assert!(!honoured_insecure_http(false, false));
        assert!(honoured_insecure_http(true, true));
        assert!(!honoured_insecure_http(false, true));
        for endpoint in [
            "http://8.8.8.8",
            "http://example.com",
            "http://10.0.0.5:8000",
        ] {
            assert!(
                matches!(
                    validate_endpoint_in(endpoint, true, false),
                    Err(SpeleoDbError::InsecureEndpoint {
                        refusal: EndpointRefusal::PlainHttp,
                        ..
                    })
                ),
                "{endpoint}"
            );
        }
        // The transport check refuses it too, before any resolution.
        let must_not_resolve = |_: &str, _: u16| Err(std::io::Error::other("must not resolve"));
        assert!(matches!(
            enforce_with("http://speleodb.lan", true, false, must_not_resolve),
            Err(SpeleoDbError::InsecureEndpoint {
                refusal: EndpointRefusal::PlainHttp,
                ..
            })
        ));
        // The automatic exceptions and https are unaffected.
        for endpoint in [
            "http://localhost:8000",
            "http://127.0.0.1",
            "http://192.168.1.50",
            "https://example.com",
        ] {
            assert!(
                validate_endpoint_in(endpoint, true, false).is_ok(),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn only_a_development_build_honours_insecure_http() {
        assert_eq!(INSECURE_HTTP_HONOURED, cfg!(debug_assertions));
        // `validate_endpoint` is the policy of THIS build.
        assert_eq!(
            validate_endpoint("http://8.8.8.8", true).is_ok(),
            cfg!(debug_assertions)
        );
    }

    #[test]
    fn an_unresolvable_http_host_the_override_allows_is_an_offline_dns_failure() {
        // sdk#6: a host that cannot resolve is never dialed, and it is reported as
        // the DNS transport failure, not as a fatal InsecureEndpoint - so an
        // offline `http://` endpoint gets the cached open an offline `https://`
        // one gets. `.invalid` never resolves (RFC 6761).
        let err = enforce_with(
            "http://this-host-does-not-exist.invalid:8000",
            true,
            true,
            super::resolve_addrs,
        )
        .expect_err("unresolvable http host must not be allowed");
        assert!(
            matches!(
                err,
                SpeleoDbError::Transport {
                    cause: TransportCause::Dns,
                    ..
                }
            ),
            "got {err:?}"
        );
        // Same through the injected resolver, including an empty answer.
        let not_found = || std::io::Error::new(std::io::ErrorKind::NotFound, "offline");
        for resolved in [Err(not_found()), Ok(Vec::new())] {
            let err = enforce_with("http://speleodb.lan:8000", true, true, |host, port| {
                assert_eq!((host, port), ("speleodb.lan", 8000));
                resolved
            })
            .expect_err("nothing to dial");
            assert!(matches!(
                err,
                SpeleoDbError::Transport {
                    cause: TransportCause::Dns,
                    ..
                }
            ));
        }
    }

    #[test]
    fn only_an_allowed_http_hostname_is_resolved() {
        let must_not_resolve = |_: &str, _: u16| Err(std::io::Error::other("must not resolve"));
        // Refused without the override: the policy, not DNS, answers.
        assert!(matches!(
            enforce_with("http://speleodb.lan", false, true, must_not_resolve),
            Err(SpeleoDbError::InsecureEndpoint {
                refusal: EndpointRefusal::PlainHttp,
                ..
            })
        ));
        // Nothing to resolve: https, the exceptions and IP literals.
        for (endpoint, insecure_http) in [
            ("https://speleodb.example", false),
            ("http://localhost:8000", false),
            ("http://192.168.1.50", false),
            ("http://10.0.0.5:8000", true),
            ("http://[::1]:8000", true),
        ] {
            assert!(
                enforce_with(endpoint, insecure_http, true, must_not_resolve).is_ok(),
                "{endpoint}"
            );
        }
        // A hostname that resolves goes ahead.
        let resolved = enforce_with("http://speleodb.lan", true, true, |_, port| {
            assert_eq!(port, 80);
            Ok(vec!["10.10.0.220".parse().expect("test IP literal")])
        });
        assert!(resolved.is_ok());
    }

    #[test]
    fn the_refusal_names_the_rule_and_the_endpoint() {
        let err = validate_endpoint("http://10.0.0.1:8000", false).expect_err("refused");
        let message = err.to_string();
        assert!(message.contains("HTTPS is required"), "{message}");
        assert!(message.contains("http://10.0.0.1:8000"), "{message}");
    }

    #[test]
    fn clean_strips_only_what_the_user_cannot_see_at_the_ends() {
        for (input, expected) in [
            ("https://www.speleodb.org", Some("https://www.speleodb.org")),
            (
                "  https://www.speleodb.org \r\n",
                Some("https://www.speleodb.org"),
            ),
            (
                "\u{00A0}https://www.speleodb.org\u{3000}",
                Some("https://www.speleodb.org"),
            ),
            (
                "\u{200B}\u{FEFF}https://www.speleodb.org\u{2060}",
                Some("https://www.speleodb.org"),
            ),
            (
                "\u{200E}https://www.speleodb.org\u{202C}\u{00AD}",
                Some("https://www.speleodb.org"),
            ),
            (
                "\u{0085}https://www.speleodb.org\u{0085}",
                Some("https://www.speleodb.org"),
            ),
            // Interior characters are the user's; validation decides about them.
            (
                "https://www.speleo db.org",
                Some("https://www.speleo db.org"),
            ),
            (
                "https://www.speleo\u{200B}db.org",
                Some("https://www.speleo\u{200B}db.org"),
            ),
            ("", None),
            (" \t\n\u{200B}\u{FEFF} ", None),
        ] {
            assert_eq!(
                clean_endpoint_input(input).as_deref(),
                expected,
                "{input:?}"
            );
        }
    }

    #[test]
    fn a_cleaned_paste_is_valid_and_an_interior_space_is_still_refused() {
        let pasted = "\u{FEFF} https://www.speleodb.org\u{200B}\n";
        assert_eq!(refusal(pasted, false), EndpointRefusal::InvalidAddress);
        let cleaned = clean_endpoint_input(pasted).expect("non-empty");
        assert!(validate_endpoint(&cleaned, false).is_ok(), "{cleaned:?}");
        assert_eq!(
            refusal("https://www.speleo db.org", false),
            EndpointRefusal::InvalidAddress
        );
    }

    #[test]
    fn the_refusal_shows_an_invisible_character_escaped() {
        let message = validate_endpoint("https://www.speleodb.org\u{200B}", false)
            .expect_err("refused")
            .to_string();
        assert!(
            message.ends_with("Rejected endpoint: \"https://www.speleodb.org\\u{200b}\""),
            "{message}"
        );
    }

    #[test]
    fn normalize_endpoint_treats_a_pasted_spelling_as_the_same_server() {
        // Project-file links and a config loaded before any save reach
        // `normalize_endpoint` uncleaned; it must still see one server.
        use crate::speleodb::auth::{account_key, normalize_endpoint};
        let pasted = "\u{200B} https://SDB.example.com:443/ \u{FEFF}";
        assert_eq!(normalize_endpoint(pasted), "https://sdb.example.com");
        assert_eq!(
            account_key(pasted, "diver@example.com"),
            account_key("https://sdb.example.com", "diver@example.com")
        );
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
}
