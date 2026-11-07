//! Live data-source attribution for tile providers.
//!
//! Esri requires the *names of the sources* behind its imagery to stay visible
//! wherever that imagery is drawn, and tells integrators to read those names
//! from the service at runtime rather than hard-coding them, because the list
//! changes. See
//! <https://developers.arcgis.com/documentation/esri-and-data-attribution/interactive-maps/>.
//!
//! This module does exactly that: it GETs the provider's service-metadata
//! endpoint (`service_info_url`) and reads `copyrightText`. The result is cached
//! in memory with a long TTL - the string changes on the order of once a year,
//! so re-asking more often than daily is pure noise.
//!
//! **It never fails outward.** Offline, timed out, or malformed, the caller gets
//! the registry's own `attribution` string with `live: false`. Attribution that
//! degrades to a slightly stale-but-correct list beats attribution that
//! disappears when the network does, and this app is expected to run offline in
//! the field.
//!
//! The separate "Powered by Esri" notice is carried here too, but only so the
//! frontend can fetch both in one call - that one is allowed to live behind a
//! menu or button, and the app credits it in the About dialog.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::models::poison_policy::RecoveringMutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::tile_providers;

/// HTTP request timeout for the service-metadata fetch.
///
/// Deliberately short: this is decoration on a map that is already drawing, and
/// the fallback is a correct string. Blocking a worker thread for longer buys
/// nothing.
const ATTRIBUTION_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a fetched attribution string is trusted before it is re-fetched.
const ATTRIBUTION_TTL: Duration = Duration::from_hours(24);

/// How long a *failed* fetch suppresses further attempts.
///
/// Without this, a map panning offline re-attempts (and re-blocks for the
/// timeout) on every call. The fallback string is already being displayed, so
/// there is nothing to gain from retrying tightly.
const ATTRIBUTION_FAILURE_BACKOFF: Duration = Duration::from_mins(5);

/// Most bytes the service-metadata response may decode to (#576 RV19a-11).
/// Esri's real document is about 11 KB, so 1 MiB is some ninety times that,
/// while a body streaming far more is refused here instead of being read up
/// to ureq's silent 10 MiB default - a default that counts the gzip bytes on
/// the wire, so it never bounded the decoded body at all.
const ATTRIBUTION_BODY_MAX_BYTES: u64 = 1024 * 1024;

/// Characters removed from a remote attribution before it is displayed.
///
/// `char::is_control` alone is Unicode category Cc only, which leaves the
/// format and separator characters that matter most for a credit line: U+202E
/// RIGHT-TO-LEFT OVERRIDE can reverse the rendered source names, and U+2028
/// breaks the layout. Neither is an injection - React escapes the string - but
/// both defeat the display integrity this module exists to provide.
fn is_display_hostile(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{200B}'..='\u{200F}'   // zero-width + LRM/RLM
            | '\u{202A}'..='\u{202E}' // bidi embedding/override
            | '\u{2060}'..='\u{206F}' // word joiner, invisible ops, bidi isolates
            | '\u{2028}' | '\u{2029}' // line / paragraph separator
            | '\u{FEFF}'               // BOM / zero-width no-break space
        )
}

/// Ceiling on the attribution string, in characters.
///
/// `copyrightText` is remote text that ends up in a DOM node and a `title=`
/// tooltip. React escapes it, so this is not an injection guard - it is a size
/// guard: a compromised or misconfigured endpoint should not be able to put
/// megabytes into the layout, or into the `split(',')` the collapsed chip runs
/// over it. Esri's own line is ~66 characters, so the cap is ~8x the real thing.
const MAX_ATTRIBUTION_CHARS: usize = 512;

/// Attribution for one tile provider, as handed to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TileAttribution {
    /// Provider this describes (e.g. "esri").
    pub provider: String,
    /// Data-source names. MUST be displayed wherever the imagery is drawn.
    pub text: String,
    /// Provider notice ("Powered by Esri"), which may live behind a button.
    pub notice: Option<String>,
    /// Brand URL for `notice`.
    pub notice_url: Option<String>,
    /// `true` when `text` came from the provider's live service metadata,
    /// `false` when it is the compiled-in fallback.
    pub live: bool,
}

/// Service-metadata response. ArcGIS returns a large document; the only field
/// that matters here is the copyright line.
#[derive(Deserialize)]
struct ServiceInfo {
    #[serde(rename = "copyrightText")]
    copyright_text: Option<String>,
}

/// What the cache remembers about one provider.
enum CacheEntry {
    /// A live string, good until `fetched_at + ATTRIBUTION_TTL`.
    Live { text: String, fetched_at: Instant },
    /// A failed attempt; do not retry until `failed_at + ATTRIBUTION_FAILURE_BACKOFF`.
    Failed { failed_at: Instant },
}

static CACHE: OnceLock<RecoveringMutex<HashMap<String, CacheEntry>>> = OnceLock::new();

fn cache() -> &'static RecoveringMutex<HashMap<String, CacheEntry>> {
    CACHE.get_or_init(|| RecoveringMutex::new("tile_attribution_cache", HashMap::new()))
}

/// Read `copyrightText` from a provider's service-metadata endpoint.
fn fetch_copyright_text(url: &str) -> Result<String, String> {
    tracing::debug!(url = %url, "Fetching tile attribution");

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(ATTRIBUTION_FETCH_TIMEOUT))
        .build()
        .new_agent();

    let response = agent
        .get(url)
        .call()
        .map_err(|e| format!("Attribution network error: {e}"))?;

    // Over the ceiling this is ureq's `BodyExceedsLimit`, whose message names
    // the limit; either way the caller falls back to the compiled-in string.
    let body =
        stalagmight_tiles::read_body_capped(response.into_body(), ATTRIBUTION_BODY_MAX_BYTES)
            .map_err(|e| format!("Failed to read attribution response: {e}"))?;

    parse_copyright_text(&body)
}

/// Parse `copyrightText` out of a service-metadata body.
///
/// Split from the network call so the parse is testable without a server.
fn parse_copyright_text(body: &[u8]) -> Result<String, String> {
    let info: ServiceInfo = serde_json::from_slice(body)
        .map_err(|e| format!("Failed to parse attribution JSON: {e}"))?;

    // Hostile characters are REPLACED with a space, not deleted. Deleting them
    // merges the names either side: a plausible multi-line `copyrightText` of
    // "Source: Esri\nVantor" became "Source: EsriVantor", i.e. a fabricated
    // source name in a credit that is supposed to be exact - and it is the name
    // `primarySourceName` then puts on the collapsed chip.
    let cleaned: String = info
        .copyright_text
        .unwrap_or_default()
        .chars()
        .map(|c| if is_display_hostile(c) { ' ' } else { c })
        .collect();

    // Collapse the runs that replacement can create, then bound the length.
    let mut text: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");

    if text.chars().count() > MAX_ATTRIBUTION_CHARS {
        // Marked, not silently clipped: a truncated credit displayed as though
        // complete is worse than one that says it was cut.
        text = text
            .chars()
            .take(MAX_ATTRIBUTION_CHARS - 1)
            .collect::<String>()
            .trim_end()
            .to_string();
        text.push('\u{2026}');
    }

    if text.is_empty() {
        // An empty copyright line is not an attribution - fall back rather than
        // display nothing where names are required.
        return Err("Service metadata carried no copyrightText".to_string());
    }

    Ok(text)
}

/// Resolve the attribution for `provider_name`.
///
/// Blocking: run it off the UI thread. Returns the compiled-in fallback for an
/// unknown provider, an offline machine, or a provider with no metadata
/// endpoint - never an error.
pub fn get_attribution(provider_name: &str) -> TileAttribution {
    let Some(provider) = tile_providers::get_provider(provider_name) else {
        // Unknown provider: nothing to attribute, but the caller still gets a
        // well-formed answer rather than an error to render around.
        return TileAttribution {
            provider: provider_name.to_string(),
            text: String::new(),
            notice: None,
            notice_url: None,
            live: false,
        };
    };

    let fallback = |live_text: Option<String>| TileAttribution {
        provider: provider.name.clone(),
        live: live_text.is_some(),
        text: live_text.unwrap_or_else(|| provider.attribution.clone()),
        notice: provider.attribution_notice.clone(),
        notice_url: provider.attribution_url.clone(),
    };

    let Some(url) = provider.service_info_url.as_ref() else {
        return fallback(None);
    };

    // Cache hit, or a recent failure we agreed not to retry yet.
    {
        // A poisoned lock is recovered (#576 A11): the cache holds nothing that
        // cannot be re-fetched, and an entry is written whole under the lock.
        {
            let guard = cache().lock();
            match guard.get(&provider.name) {
                Some(CacheEntry::Live { text, fetched_at })
                    if fetched_at.elapsed() < ATTRIBUTION_TTL =>
                {
                    return fallback(Some(text.clone()));
                }
                Some(CacheEntry::Failed { failed_at })
                    if failed_at.elapsed() < ATTRIBUTION_FAILURE_BACKOFF =>
                {
                    return fallback(None);
                }
                _ => {}
            }
        }
    }

    match fetch_copyright_text(url) {
        Ok(text) => {
            cache().lock().insert(
                provider.name.clone(),
                CacheEntry::Live {
                    text: text.clone(),
                    fetched_at: Instant::now(),
                },
            );
            tracing::debug!(provider = %provider.name, attribution = %text, "Live tile attribution");
            fallback(Some(text))
        }
        Err(e) => {
            cache().lock().insert(
                provider.name.clone(),
                CacheEntry::Failed {
                    failed_at: Instant::now(),
                },
            );
            tracing::debug!(
                provider = %provider.name,
                error = %e,
                "Tile attribution fetch failed; using compiled-in source names"
            );
            fallback(None)
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod tests {
    use super::*;

    #[test]
    fn parses_copyright_text() {
        let body = br#"{"currentVersion":11.4,"copyrightText":"Source: Esri, Vantor, Earthstar Geographics, and the GIS User Community","serviceDescription":"..."}"#;
        assert_eq!(
            parse_copyright_text(body).unwrap(),
            "Source: Esri, Vantor, Earthstar Geographics, and the GIS User Community"
        );
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let body = br#"{"copyrightText":"  Source: Esri  "}"#;
        assert_eq!(parse_copyright_text(body).unwrap(), "Source: Esri");
    }

    #[test]
    fn rejects_missing_copyright_text() {
        // Well-formed metadata that simply carries no copyright line must fall
        // back, not display an empty attribution.
        let body = br#"{"currentVersion":11.4}"#;
        assert!(parse_copyright_text(body).is_err());
    }

    #[test]
    fn rejects_empty_copyright_text() {
        let body = br#"{"copyrightText":"   "}"#;
        assert!(parse_copyright_text(body).is_err());
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(parse_copyright_text(b"not json at all").is_err());
    }

    #[test]
    fn unknown_provider_yields_empty_attribution_not_an_error() {
        let attribution = get_attribution("no_such_provider_12345");
        assert_eq!(attribution.provider, "no_such_provider_12345");
        assert!(attribution.text.is_empty());
        assert!(attribution.notice.is_none());
        assert!(!attribution.live);
    }

    /// The fallback is the thing that renders offline, so it has to be a real
    /// attribution on its own - not a placeholder waiting for the network.
    #[test]
    fn esri_fallback_names_the_sources() {
        let provider = tile_providers::get_provider("esri").unwrap();
        assert!(provider.attribution.contains("Esri"));
        assert!(provider.attribution.starts_with("Source:"));
        assert_eq!(
            provider.attribution_notice.as_deref(),
            Some("Powered by Esri")
        );
        assert_eq!(
            provider.attribution_url.as_deref(),
            Some("https://www.esri.com")
        );
        assert!(provider
            .service_info_url
            .as_ref()
            .is_some_and(|u| u.contains("f=json")));
    }

    #[test]
    fn attribution_serializes_with_every_field_the_ui_reads() {
        let attribution = TileAttribution {
            provider: "esri".to_string(),
            text: "Source: Esri".to_string(),
            notice: Some("Powered by Esri".to_string()),
            notice_url: Some("https://www.esri.com".to_string()),
            live: true,
        };
        let json = serde_json::to_value(&attribution).unwrap();
        assert_eq!(json["provider"], "esri");
        assert_eq!(json["text"], "Source: Esri");
        assert_eq!(json["notice"], "Powered by Esri");
        assert_eq!(json["notice_url"], "https://www.esri.com");
        assert_eq!(json["live"], true);
    }

    #[test]
    fn a_huge_copyright_line_is_capped_before_it_reaches_the_ui() {
        // Remote text that ends up in a DOM node and a title tooltip. React
        // escapes it, so this is a size guard, not an injection one.
        let huge = "A".repeat(100_000);
        let body = format!(r#"{{"copyrightText":"{huge}"}}"#);
        let text = parse_copyright_text(body.as_bytes()).unwrap();
        assert_eq!(text.chars().count(), MAX_ATTRIBUTION_CHARS);
    }

    #[test]
    fn control_characters_are_neutralised_without_fusing_the_names() {
        let body = "{\"copyrightText\":\"Source: Esri\\u0000\\u0007Vantor\"}";
        let text = parse_copyright_text(body.as_bytes()).unwrap();
        assert!(!text.chars().any(char::is_control));
        // Replaced with a separator, not deleted - deleting would have produced
        // the single fabricated source name "EsriVantor".
        assert_eq!(text, "Source: Esri Vantor");
    }

    #[test]
    fn a_line_of_only_control_characters_is_not_an_attribution() {
        // Stripping must not turn junk into an empty string we then display.
        let body = "{\"copyrightText\":\"\\u0000\\u0001\\u0002\"}";
        assert!(parse_copyright_text(body.as_bytes()).is_err());
    }

    #[test]
    fn a_bidi_override_cannot_reverse_the_rendered_source_names() {
        // U+202E survives `char::is_control` (category Cc only) and would flip
        // the display order of a legally required credit.
        let body = "{\"copyrightText\":\"Source: Esri\\u202E rotnaV\"}";
        let text = parse_copyright_text(body.as_bytes()).unwrap();
        assert!(!text.contains('\u{202E}'), "got: {text:?}");
        assert!(text.contains("Esri"));
    }

    #[test]
    fn a_multi_line_credit_does_not_fuse_two_source_names() {
        // Deleting control characters merged the names either side of them:
        // "Source: Esri\nVantor" became "EsriVantor", inventing a source.
        let body = "{\"copyrightText\":\"Source: Esri\\nVantor\\tEarthstar\"}";
        assert_eq!(
            parse_copyright_text(body.as_bytes()).unwrap(),
            "Source: Esri Vantor Earthstar"
        );
    }

    #[test]
    fn an_over_long_credit_says_it_was_cut_rather_than_reading_as_complete() {
        let huge = format!("Source: {}", "Somebody, ".repeat(2_000));
        let body = format!(r#"{{"copyrightText":"{huge}"}}"#);
        let text = parse_copyright_text(body.as_bytes()).unwrap();
        assert_eq!(text.chars().count(), MAX_ATTRIBUTION_CHARS);
        assert!(
            text.ends_with('\u{2026}'),
            "a clipped credit must not present itself as the whole list"
        );
    }

    /// A loopback stub answering every request with `200 OK` and `body` as
    /// JSON. Returns a service-metadata URL on it.
    fn serve_service_info(body: Vec<u8>) -> String {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/MapServer?f=json", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    match stream.read(&mut byte) {
                        Ok(1) => head.extend_from_slice(&byte),
                        _ => break,
                    }
                }
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                // The client hangs up once it hits its ceiling; a failed
                // write is that, not a stub fault.
                let _ = stream.write_all(reply.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        url
    }

    /// #576 RV19a-11: the service metadata is read under an explicit ceiling.
    /// A valid document padded to over 2 MiB (JSON allows trailing
    /// whitespace) was read whole under ureq's silent 10 MiB default.
    #[test]
    fn a_service_info_body_over_the_ceiling_is_refused() {
        let mut body = br#"{"copyrightText":"Source: Esri"}"#.to_vec();
        body.resize(body.len() + 2 * 1024 * 1024, b' ');
        let len = body.len();
        let result = fetch_copyright_text(&serve_service_info(body));
        println!("{len}-byte service-info body: {result:?}");
        assert!(
            result.is_err(),
            "a body over the ceiling must be refused, got {result:?}"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("larger than") && err.contains("1048576"),
            "the refusal must name the limit, got {err}"
        );
    }

    /// An ordinary document is still read and parsed.
    #[test]
    fn a_service_info_body_under_the_ceiling_is_parsed() {
        let body = br#"{"currentVersion":11.4,"copyrightText":"Source: Esri"}"#.to_vec();
        let result = fetch_copyright_text(&serve_service_info(body));
        println!("ordinary service-info body: {result:?}");
        assert_eq!(result.unwrap(), "Source: Esri");
    }
}
