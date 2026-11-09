//! Where the fuzzer may send requests, and with which accounts.
//!
//! The fuzzer only ever talks to a private SpeleoDB test instance on the machine
//! it runs on. It refuses to start, before it sends a single request, unless:
//!
//! - `STALAGMIGHT_SPELEODB_FUZZ_URL` is a plain `http://` URL whose host is a
//!   loopback IP LITERAL (`127.x.y.z` or `[::1]`) with an explicit port and no
//!   path, query, fragment or userinfo. No name is resolved, so no DNS answer can
//!   point the run anywhere else, and a public SpeleoDB is never on loopback.
//! - every account email it is given is under the reserved `.invalid` top-level
//!   domain (RFC 2606), which no real server can hold a verified account for, so a
//!   misdirected run could not authenticate anywhere real either.
//! - `DEV_SPELEODB_URL` is unset: the app resolves its endpoint from it ahead of
//!   its own config, so a stray value would steer a rehydrate past the proxy.
//!
//! The in-process fault proxy and the other-server stub are loopback too; every
//! endpoint the app session can hold is one of the two, and the driver checks
//! that after every step.

use std::net::{IpAddr, SocketAddr};

/// The env var naming the instance's base URL (e.g. `http://127.0.0.1:18000/`).
pub(super) const URL_VAR: &str = "STALAGMIGHT_SPELEODB_FUZZ_URL";

/// The optional read-only account's variables: both set, or neither.
const RO_EMAIL_VAR: &str = "STALAGMIGHT_SPELEODB_FUZZ_RO_EMAIL";
const RO_PASSWORD_VAR: &str = "STALAGMIGHT_SPELEODB_FUZZ_RO_PASSWORD";

/// One account on the instance.
#[derive(Clone)]
pub(super) struct Account {
    pub(super) email: String,
    pub(super) password: String,
}

/// Everything a run needs from the environment, validated.
#[derive(Clone)]
pub(super) struct FuzzTarget {
    /// The instance base URL, normalised to end in `/`.
    pub(super) url: String,
    /// The same, as a socket address for the proxy's upstream.
    pub(super) upstream: SocketAddr,
    /// The app's own account: a normal user.
    pub(super) owner: Account,
    /// The second client: a normal user who shares projects with `owner`.
    pub(super) collab: Account,
    /// A Django superuser: the oracle's eyes (granted READ_ONLY on every run
    /// project, since a superuser has no implicit API access).
    pub(super) admin: Account,
    /// An optional read-only account (`_RO_EMAIL` / `_RO_PASSWORD`, both or
    /// neither). `None` when neither is set.
    pub(super) ro: Option<Account>,
}

/// Validate `url` as the fuzz instance: plain http on a loopback IP literal.
/// Returns the URL normalised to end in `/` and its socket address.
pub(super) fn check_target_url(url: &str) -> Result<(String, SocketAddr), String> {
    let refuse = |why: &str| Err(format!("{URL_VAR}={url:?} refused: {why}"));
    let Some(rest) = url.strip_prefix("http://") else {
        return refuse("only a plain http:// URL to a loopback instance is allowed");
    };
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.contains(['/', '?', '#', '@']) {
        return refuse("no path, query, fragment or userinfo");
    }
    let Ok(addr) = authority.parse::<SocketAddr>() else {
        return refuse("the host must be an IP literal with a port (127.0.0.1:PORT or [::1]:PORT)");
    };
    if !is_loopback(addr.ip()) {
        return refuse("the host is not a loopback address");
    }
    if addr.port() == 0 {
        return refuse("port 0");
    }
    Ok((format!("http://{authority}/"), addr))
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// Validate an account email: it must be under the reserved `.invalid` TLD.
/// `var` names the variable it came from, for the refusal text.
pub(super) fn check_account_email(var: &str, email: &str) -> Result<(), String> {
    let lower = email.trim().to_ascii_lowercase();
    let domain_ok = lower
        .rsplit_once('@')
        .is_some_and(|(local, domain)| !local.is_empty() && domain.ends_with(".invalid"));
    if domain_ok {
        Ok(())
    } else {
        Err(format!(
            "{var} refused: fuzz accounts must be under the reserved .invalid domain"
        ))
    }
}

fn account(
    get: &dyn Fn(&str) -> Option<String>,
    email_var: &str,
    password_var: &str,
) -> Result<Account, String> {
    let email = get(email_var).ok_or_else(|| format!("{email_var} is not set"))?;
    check_account_email(email_var, &email)?;
    let password = get(password_var).ok_or_else(|| format!("{password_var} is not set"))?;
    Ok(Account { email, password })
}

/// The optional read-only account: `None` when neither variable is set, an
/// error naming the missing one when only one is.
fn optional_ro_account(get: &dyn Fn(&str) -> Option<String>) -> Result<Option<Account>, String> {
    match (get(RO_EMAIL_VAR), get(RO_PASSWORD_VAR)) {
        (None, None) => Ok(None),
        (Some(_), None) => Err(format!(
            "{RO_PASSWORD_VAR} is not set but {RO_EMAIL_VAR} is: the read-only account needs both or neither"
        )),
        (None, Some(_)) => Err(format!(
            "{RO_EMAIL_VAR} is not set but {RO_PASSWORD_VAR} is: the read-only account needs both or neither"
        )),
        (Some(_), Some(_)) => account(get, RO_EMAIL_VAR, RO_PASSWORD_VAR).map(Some),
    }
}

/// Read and validate the whole target from `get` (the process environment in a
/// run; a map in the tests). Nothing here touches the network. Every error
/// names the variable that is missing or refused.
pub(super) fn target_from(get: &dyn Fn(&str) -> Option<String>) -> Result<FuzzTarget, String> {
    if let Some(dev) = get("DEV_SPELEODB_URL") {
        return Err(format!(
            "DEV_SPELEODB_URL={dev:?} is set; the app would resolve its endpoint from it. Unset it."
        ));
    }
    let raw = get(URL_VAR).ok_or_else(|| format!("{URL_VAR} is not set"))?;
    let (url, upstream) = check_target_url(raw.trim())?;
    Ok(FuzzTarget {
        url,
        upstream,
        owner: account(
            get,
            "STALAGMIGHT_SPELEODB_FUZZ_EMAIL",
            "STALAGMIGHT_SPELEODB_FUZZ_PASSWORD",
        )?,
        collab: account(
            get,
            "STALAGMIGHT_SPELEODB_FUZZ_EMAIL2",
            "STALAGMIGHT_SPELEODB_FUZZ_PASSWORD2",
        )?,
        admin: account(
            get,
            "STALAGMIGHT_SPELEODB_FUZZ_ADMIN_EMAIL",
            "STALAGMIGHT_SPELEODB_FUZZ_ADMIN_PASSWORD",
        )?,
        ro: optional_ro_account(get)?,
    })
}

/// [`target_from`] over the process environment (a blank value counts as unset).
pub(super) fn target_from_env() -> Result<FuzzTarget, String> {
    target_from(&|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
}

/// Is `endpoint` (as the app session holds it) one of `allowed`? Compared after
/// trimming a trailing `/`, which is the only spelling difference the app makes.
pub(super) fn endpoint_allowed(endpoint: &str, allowed: &[&str]) -> bool {
    let norm = |s: &str| s.trim().trim_end_matches('/').to_ascii_lowercase();
    allowed.iter().any(|a| norm(a) == norm(endpoint))
}

#[cfg(test)]
mod tests {
    use super::{
        check_account_email, check_target_url, endpoint_allowed, target_from, RO_EMAIL_VAR,
        RO_PASSWORD_VAR,
    };
    use std::collections::HashMap;

    fn base_env() -> HashMap<&'static str, &'static str> {
        [
            ("STALAGMIGHT_SPELEODB_FUZZ_URL", "http://127.0.0.1:18000/"),
            ("STALAGMIGHT_SPELEODB_FUZZ_EMAIL", "owner@fuzz.invalid"),
            ("STALAGMIGHT_SPELEODB_FUZZ_PASSWORD", "p1"),
            ("STALAGMIGHT_SPELEODB_FUZZ_EMAIL2", "collab@fuzz.invalid"),
            ("STALAGMIGHT_SPELEODB_FUZZ_PASSWORD2", "p2"),
            (
                "STALAGMIGHT_SPELEODB_FUZZ_ADMIN_EMAIL",
                "admin@fuzz.invalid",
            ),
            ("STALAGMIGHT_SPELEODB_FUZZ_ADMIN_PASSWORD", "p3"),
        ]
        .into_iter()
        .collect()
    }

    fn get(map: &HashMap<&str, &str>, name: &str) -> Option<String> {
        map.get(name).map(|v| (*v).to_owned())
    }

    #[test]
    fn only_a_loopback_ip_literal_over_http_is_a_target() {
        for ok in [
            "http://127.0.0.1:18000/",
            "http://127.0.0.1:18000",
            "http://127.1.2.3:8000/",
            "http://[::1]:18000/",
        ] {
            assert!(check_target_url(ok).is_ok(), "{ok} should be allowed");
        }
        for bad in [
            "https://127.0.0.1:18000/",
            "http://localhost:18000/",
            "http://www.speleodb.org/",
            "https://www.speleodb.org/",
            "http://stage.speleodb.org:80/",
            "http://10.0.0.5:8000/",
            "http://192.168.1.2:8000/",
            "http://0.0.0.0:8000/",
            "http://127.0.0.1/",
            "http://127.0.0.1:0/",
            "http://127.0.0.1:18000/api/v2/",
            "http://user@127.0.0.1:18000/",
            "http://127.0.0.1:18000/?x=1",
            "http://127.0.0.1.nip.io:18000/",
            "ftp://127.0.0.1:18000/",
            "",
        ] {
            assert!(check_target_url(bad).is_err(), "{bad} must be refused");
        }
        assert_eq!(
            check_target_url("http://127.0.0.1:18000")
                .expect("allowed")
                .0,
            "http://127.0.0.1:18000/"
        );
    }

    #[test]
    fn accounts_must_be_under_the_reserved_invalid_domain() {
        assert!(check_account_email("E", "owner@fuzz.invalid").is_ok());
        assert!(check_account_email("E", "Owner@Fuzz.INVALID").is_ok());
        for bad in [
            "owner@example.com",
            "owner@invalid",
            "@fuzz.invalid",
            "owner",
            "",
        ] {
            assert!(
                check_account_email("E", bad).is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn the_target_needs_every_var_and_refuses_a_dev_override() {
        let full = base_env();
        let target = target_from(&|n| get(&full, n)).expect("a full env is a target");
        assert!(target.ro.is_none(), "no read-only account configured");
        for missing in full.keys() {
            let mut partial = full.clone();
            partial.remove(missing);
            let err = target_from(&|n| get(&partial, n))
                .err()
                .expect("a missing var is refused");
            assert!(err.contains(missing), "{err} names {missing}");
        }
        let mut dev = full.clone();
        dev.insert("DEV_SPELEODB_URL", "http://127.0.0.1:18000/");
        assert!(target_from(&|n| get(&dev, n)).is_err());
        let mut public = full;
        public.insert("STALAGMIGHT_SPELEODB_FUZZ_URL", "https://www.speleodb.org/");
        assert!(target_from(&|n| get(&public, n)).is_err());
    }

    #[test]
    fn every_account_email_is_checked() {
        for var in [
            "STALAGMIGHT_SPELEODB_FUZZ_EMAIL",
            "STALAGMIGHT_SPELEODB_FUZZ_EMAIL2",
            "STALAGMIGHT_SPELEODB_FUZZ_ADMIN_EMAIL",
        ] {
            let mut env = base_env();
            env.insert(var, "someone@example.com");
            let err = target_from(&|n| get(&env, n))
                .err()
                .expect("a real-looking email is refused");
            assert!(err.contains(var) && err.contains(".invalid"), "{err}");
        }
    }

    #[test]
    fn the_read_only_account_is_both_or_neither_and_its_email_is_checked() {
        let mut both = base_env();
        both.insert(RO_EMAIL_VAR, "ro@fuzz.invalid");
        both.insert(RO_PASSWORD_VAR, "p4");
        let ro = target_from(&|n| get(&both, n))
            .expect("both set is a target")
            .ro
            .expect("the read-only account is present");
        assert_eq!(
            (ro.email.as_str(), ro.password.as_str()),
            ("ro@fuzz.invalid", "p4")
        );
        for (present, missing) in [
            (RO_EMAIL_VAR, RO_PASSWORD_VAR),
            (RO_PASSWORD_VAR, RO_EMAIL_VAR),
        ] {
            let mut half = base_env();
            half.insert(present, "ro@fuzz.invalid");
            let err = target_from(&|n| get(&half, n))
                .err()
                .expect("half a read-only account is refused");
            assert!(err.starts_with(missing), "{err} names {missing} first");
        }
        let mut real = both;
        real.insert(RO_EMAIL_VAR, "ro@example.com");
        let err = target_from(&|n| get(&real, n))
            .err()
            .expect("the read-only email is checked like the others");
        assert!(
            err.contains(RO_EMAIL_VAR) && err.contains(".invalid"),
            "{err}"
        );
    }

    #[test]
    fn endpoints_compare_without_the_trailing_slash() {
        assert!(endpoint_allowed(
            "http://127.0.0.1:5/",
            &["http://127.0.0.1:5", "http://127.0.0.1:6/"]
        ));
        assert!(!endpoint_allowed(
            "http://127.0.0.1:7/",
            &["http://127.0.0.1:5/"]
        ));
    }
}
