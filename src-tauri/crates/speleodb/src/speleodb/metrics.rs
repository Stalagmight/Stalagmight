//! Per-endpoint traffic instrumentation for the SpeleoDB client (#210, F1).
//!
//! Every SpeleoDB HTTP call funnels through `client::SdkClient::call`, the
//! single choke point a new method cannot forget. This module turns each
//! attempt through that choke point into three no-PII telemetry dimensions -
//! **count**, **latency**, and **outcome** - so the backend can size the server
//! load Stalagmight causes and spot excessive requests or retries (§4.11 F1).
//! The payload never carries a host, project id, email,
//! token, or file bytes: only the endpoint *kind*, the outcome class, and a
//! latency bucket.
//!
//! Wire shape (following the flat `{name: count}` payload and the `dwell.<state>`
//! precedent - the wire cannot carry a computed percentile, so latency is a
//! histogram of bucket counters the backend reconstructs p50/p95 from):
//! - `speleodb-api.<kind>.<outcome>` - one increment per attempt; the five
//!   outcome counters sum to the call count for that endpoint.
//! - `speleodb-api.<kind>.latency.<bucket>` - one increment per attempt, in the
//!   bucket the attempt's wall-clock duration falls into.
//!
//! Recorded **per HTTP attempt**, not per logical call: an idempotent read that
//! retries a transient 5xx hits the server more than once, and each hit is real
//! load - so a retry storm shows up as inflated counts + latency, which is the
//! whole point (abuse detection). Backoff sleeps between attempts are excluded
//! (the timer wraps only the attempt itself).

use std::sync::OnceLock;
use std::time::Duration;

use crate::telemetry::TelemetryCollector;

/// The SpeleoDB API endpoint kinds, one label per traffic class. Every SDK
/// operation records its outcome through `SdkClient::call`.
///
/// [`Endpoint::ALL`] backs the `every_speleodb_endpoint_has_traffic_events`
/// registry coverage test: add a variant here and CI fails until its eleven event
/// names are registered - the "a new method cannot forget" guarantee (F1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Endpoint {
    Login,
    ListProjects,
    GetProject,
    GetRevisions,
    FetchUser,
    DownloadStalx,
    Upload,
    Acquire,
    Release,
    PatchProject,
    CreateProject,
}

impl Endpoint {
    /// Every endpoint kind, for the registry coverage test and event enumeration.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "enumerated only by the registry/metrics coverage tests"
        )
    )]
    pub(crate) const ALL: &'static [Endpoint] = &[
        Endpoint::Login,
        Endpoint::ListProjects,
        Endpoint::GetProject,
        Endpoint::GetRevisions,
        Endpoint::FetchUser,
        Endpoint::DownloadStalx,
        Endpoint::Upload,
        Endpoint::Acquire,
        Endpoint::Release,
        Endpoint::PatchProject,
        Endpoint::CreateProject,
    ];

    /// The stable wire label for this endpoint (the `<kind>` in the event name).
    /// Kebab-case, no PII - purely the endpoint class.
    pub(crate) fn kind(self) -> &'static str {
        match self {
            Endpoint::Login => "login",
            Endpoint::ListProjects => "list-projects",
            Endpoint::GetProject => "get-project",
            Endpoint::GetRevisions => "get-revisions",
            Endpoint::FetchUser => "fetch-user",
            Endpoint::DownloadStalx => "download-stalx",
            Endpoint::Upload => "upload",
            Endpoint::Acquire => "acquire",
            Endpoint::Release => "release",
            Endpoint::PatchProject => "patch-project",
            Endpoint::CreateProject => "create-project",
        }
    }
}

/// The outcome class of a single attempt. The five labels partition every
/// attempt: a completed HTTP exchange is `ok` (2xx/3xx, incl. the benign 304
/// unchanged), a client/validation status is `client-error` (4xx), a server
/// status is `server-error` (5xx), a reply that does not match the SDK
/// contract (an undecodable 2xx body, an unknown enum value, an oversized
/// body) is `parse-error`, and a transport failure (DNS, timeout, connection
/// reset, TLS, mid-body read) is `network-error`.
///
/// `parse-error` is separate from `ok` on purpose (sdk#8): the server answered,
/// but the app could not use the answer, and counting that as `ok` hid every
/// contract drift from the traffic metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Ok,
    ClientError,
    ServerError,
    ParseError,
    NetworkError,
}

impl Outcome {
    /// The outcome labels, for the registry coverage test.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "enumerated only by the registry/metrics coverage tests"
        )
    )]
    pub(crate) const ALL: &'static [&'static str] = &[
        "ok",
        "client-error",
        "server-error",
        "parse-error",
        "network-error",
    ];

    /// Classify a completed HTTP response by status class. A transport failure
    /// never reaches here - it is [`Outcome::NetworkError`] at the call site.
    pub(crate) fn from_status(status: u16) -> Self {
        match status {
            // 2xx success and 3xx (incl. 304 unchanged) are a completed exchange,
            // not an error condition - count them as `ok`.
            200..=399 => Outcome::Ok,
            400..=499 => Outcome::ClientError,
            // 5xx and anything else non-transport is a server-side failure.
            _ => Outcome::ServerError,
        }
    }

    /// The stable wire label for this outcome.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::ClientError => "client-error",
            Outcome::ServerError => "server-error",
            Outcome::ParseError => "parse-error",
            Outcome::NetworkError => "network-error",
        }
    }
}

/// Latency bucket labels (the `<bucket>` in `…latency.<bucket>`), coarsest last.
/// Boundaries span a fast local reply (`le-100`) through a near-read-timeout
/// stall (`le-15000`, vs the 30s read / 120s upload budgets) to the pathological
/// tail (`gt-15000`). The backend reconstructs per-endpoint p50/p95 from the
/// bucket distribution.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "enumerated only by the registry/metrics coverage tests"
    )
)]
pub(crate) const LATENCY_BUCKETS: &[&str] = &[
    "le-100", "le-500", "le-1000", "le-5000", "le-15000", "gt-15000",
];

/// Bucket an attempt duration (milliseconds) into one [`LATENCY_BUCKETS`] label.
fn latency_bucket(ms: u128) -> &'static str {
    match ms {
        0..=100 => "le-100",
        101..=500 => "le-500",
        501..=1000 => "le-1000",
        1001..=5000 => "le-5000",
        5001..=15000 => "le-15000",
        _ => "gt-15000",
    }
}

/// The process-wide telemetry sink the client emits through. Set once at startup
/// ([`install`]) from the app's collector; the free-function client has no
/// `AppHandle`, so this mirrors the tile-fetch `app_handle.state::<…>()` idiom
/// as a write-once handle instead. Absent (unset) in unit tests and headless
/// tooling, where [`record`] is a no-op.
static SINK: OnceLock<TelemetryCollector> = OnceLock::new();

/// Install the telemetry sink the SpeleoDB client records through. Called once at
/// app setup; a second call is ignored (the first handle wins).
pub fn install(collector: TelemetryCollector) {
    let _ = SINK.set(collector);
}

/// Record one attempt: its outcome counter and its latency bucket. No-op unless a
/// sink is installed and telemetry is enabled (the enabled check also avoids the
/// two `format!` allocations on the disabled path).
pub(crate) fn record(endpoint: Endpoint, outcome: Outcome, elapsed: Duration) {
    let Some(sink) = SINK.get() else {
        return;
    };
    if !sink.is_enabled() {
        return;
    }
    let kind = endpoint.kind();
    sink.track(&format!("speleodb-api.{kind}.{}", outcome.label()));
    sink.track(&format!(
        "speleodb-api.{kind}.latency.{}",
        latency_bucket(elapsed.as_millis())
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_endpoint_kind_is_unique_and_kebab() {
        let mut seen = std::collections::HashSet::new();
        for ep in Endpoint::ALL {
            let kind = ep.kind();
            assert!(seen.insert(kind), "duplicate endpoint kind: {kind}");
            assert!(
                kind.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "endpoint kind must be kebab-case: {kind}"
            );
        }
    }

    #[test]
    fn status_classes_map_to_outcomes() {
        assert_eq!(Outcome::from_status(200), Outcome::Ok);
        assert_eq!(Outcome::from_status(201), Outcome::Ok);
        // 304 (unchanged) is a benign completed exchange, not an error.
        assert_eq!(Outcome::from_status(304), Outcome::Ok);
        assert_eq!(Outcome::from_status(400), Outcome::ClientError);
        assert_eq!(Outcome::from_status(403), Outcome::ClientError);
        assert_eq!(Outcome::from_status(404), Outcome::ClientError);
        assert_eq!(Outcome::from_status(500), Outcome::ServerError);
        assert_eq!(Outcome::from_status(503), Outcome::ServerError);
        // Pin the inner range upper edges so a narrowed bound (e.g. 400..=498)
        // can't silently misclassify the boundary status.
        assert_eq!(Outcome::from_status(399), Outcome::Ok);
        assert_eq!(Outcome::from_status(499), Outcome::ClientError);
    }

    #[test]
    fn outcome_labels_match_the_registry_list() {
        // The five labels the registry coverage test enumerates, in order.
        let labels: Vec<&str> = [
            Outcome::Ok,
            Outcome::ClientError,
            Outcome::ServerError,
            Outcome::ParseError,
            Outcome::NetworkError,
        ]
        .iter()
        .map(|o| o.label())
        .collect();
        assert_eq!(labels, Outcome::ALL);
    }

    #[test]
    fn latency_buckets_cover_the_boundaries() {
        assert_eq!(latency_bucket(0), "le-100");
        assert_eq!(latency_bucket(100), "le-100");
        assert_eq!(latency_bucket(101), "le-500");
        assert_eq!(latency_bucket(500), "le-500");
        assert_eq!(latency_bucket(501), "le-1000");
        assert_eq!(latency_bucket(1000), "le-1000");
        assert_eq!(latency_bucket(1001), "le-5000");
        assert_eq!(latency_bucket(5000), "le-5000");
        assert_eq!(latency_bucket(5001), "le-15000");
        assert_eq!(latency_bucket(15000), "le-15000");
        assert_eq!(latency_bucket(15001), "gt-15000");
        assert_eq!(latency_bucket(120_000), "gt-15000");
    }

    #[test]
    fn every_bucket_label_is_reachable() {
        // Each declared bucket label is produced by some duration (no dead label).
        let produced: std::collections::HashSet<&str> = [0, 101, 501, 1001, 5001, 15001]
            .iter()
            .map(|&ms| latency_bucket(ms))
            .collect();
        for &label in LATENCY_BUCKETS {
            assert!(
                produced.contains(label),
                "unreachable bucket label: {label}"
            );
        }
        assert_eq!(produced.len(), LATENCY_BUCKETS.len());
    }
}

// Relocated from `telemetry::registry` ahead of the crate split: `speleodb`
// depends on `telemetry` non-test, so the coverage test that joins
// `Endpoint::ALL` with `EVENT_NAMES` lives on the upper side of that edge.
// Gated exactly as the registry test module it came from: the registry
// (and `EVENT_NAMES`) only exists with the `telemetry` feature.
#[cfg(all(test, feature = "telemetry"))]
mod telemetry_coverage_tests {
    use std::collections::HashSet;

    use super::{Endpoint, Outcome, LATENCY_BUCKETS};
    use crate::telemetry::registry::EVENT_NAMES;

    /// Every SpeleoDB endpoint kind (#210, F1) must have its full traffic-event
    /// set registered: five outcome counters + one counter per latency bucket.
    /// Mirrors `every_tile_provider_has_consumption_events` - add an `Endpoint`
    /// variant and this fails until its eleven names land in `EVENT_NAMES`. Also
    /// asserts the reverse: no orphan `speleodb-api.*` name the generator cannot
    /// produce (a typo, or a renamed/removed kind), so the vocabulary can't drift
    /// away from `speleodb::metrics::Endpoint`.
    #[test]
    fn every_speleodb_endpoint_has_traffic_events() {
        let registered: HashSet<&str> = EVENT_NAMES.iter().copied().collect();
        let mut expected: HashSet<String> = HashSet::new();
        for ep in Endpoint::ALL {
            let kind = ep.kind();
            for outcome in Outcome::ALL {
                let event = format!("speleodb-api.{kind}.{outcome}");
                assert!(
                    registered.contains(event.as_str()),
                    "endpoint '{kind}' missing outcome event '{event}' - add it to EVENT_NAMES"
                );
                expected.insert(event);
            }
            for bucket in LATENCY_BUCKETS {
                let event = format!("speleodb-api.{kind}.latency.{bucket}");
                assert!(
                    registered.contains(event.as_str()),
                    "endpoint '{kind}' missing latency event '{event}' - add it to EVENT_NAMES"
                );
                expected.insert(event);
            }
        }
        for name in EVENT_NAMES {
            if name.starts_with("speleodb-api.") {
                assert!(
                    expected.contains(*name),
                    "orphan speleodb-api event '{name}' - not produced by Endpoint::ALL \
                     x Outcome::ALL x LATENCY_BUCKETS; remove it or fix the kind/label"
                );
            }
        }
    }
}
