//! In-app updater IPC surface (#579).
//!
//! Rust drives the whole cycle rather than the plugin's JS guest API, for one
//! concrete reason: the guest `check()` cannot override the endpoint per call,
//! and the endpoint is exactly what changes between the stable and weekly
//! release tracks. So the endpoint is computed here from the user's configured
//! track and handed to a per-call [`tauri_plugin_updater::UpdaterBuilder`].
//!
//! The cycle is deliberately FOUR commands, not one:
//!
//! - [`check_for_app_update`] asks the manifest and parks the answer,
//! - [`download_app_update`] fetches and signature-verifies the bytes,
//! - [`arm_app_update_restart`] takes the safety catch off,
//! - [`install_app_update_and_restart`] writes them and relaunches.
//!
//! `Update::download_and_install` would collapse the middle two, and must not
//! be used: on Windows the install step hands control to the NSIS installer and
//! exits this process, so anything the frontend still had to ask the user -
//! unsaved work, a held SpeleoDB write lock - would never be asked. Splitting
//! the download from the install is what keeps the point of no return behind
//! the existing quit guards.
//!
//! ## What is trusted, and where
//!
//! The manifest is fetched over https but is NOT itself signed - only the
//! bundle is, and the plugin verifies that signature against the baked-in
//! minisign public key at the END of `Update::download`, before this module is
//! handed any bytes. So everything the manifest says is UNTRUSTED input until
//! then: the download URL, its size, and how long it takes to arrive. Three
//! guards follow from that, none of which the plugin provides:
//!
//! - [`MAX_UPDATE_BYTES`] with a real mid-stream abort (the plugin buffers the
//!   whole body in RAM and enforces no length at all),
//! - an explicit request timeout on both the check and the download,
//! - a generation stamp pairing bytes with the check that offered them.
//!
//! Two risks remain, and are known:
//!
//! - **The version is not signed.** The signature covers the bundle's bytes
//!   and nothing else: the version an update is offered as is the manifest's
//!   own, unsigned, and the plugin installs any signed bundle whose manifest
//!   version is higher than the running one (tauri-plugin-updater 2.9.0,
//!   `updater.rs`: `release.version > self.current_version` in `check`,
//!   `verify_signature(&buffer, ...)` in `download`). So whoever controls the
//!   `updater` release's manifest can pair an OLD, validly signed bundle with
//!   a higher version, and the app downgrades itself to it - to a release with
//!   a flaw since fixed, say. What stands in the way is who can write that
//!   manifest: a release build fetches it only from an https `github.com` URL
//!   ([`UPDATE_URL_ENV`], [`is_trusted_manifest_url`]), and changing it takes
//!   write access to the repository's `updater` release. Signing the version
//!   too, and checking it against the offer, would close it; not done.
//! - **The size cap can be overshot.** [`MAX_UPDATE_BYTES`] is checked in the
//!   plugin's per-chunk progress callback, and the plugin appends each chunk to
//!   its buffer right after calling it. The callback can only raise a flag, so
//!   the download is dropped when the poll that ran it returns ([`abortable`]),
//!   and every chunk the stream already had ready is buffered before then: the
//!   memory held can pass the cap by what arrived in that one poll before the
//!   abort is seen. The cap bounds the download near 512 MiB, not at it.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::task::Poll;
use std::time::Duration;

use serde::Serialize;
use tauri::utils::config::BundleType;
use tauri::{AppHandle, Manager, Runtime, Url};
use tauri_plugin_updater::{Update, UpdaterExt};
use ts_rs::TS;

use super::event_sink::emit_event;
use super::pool;
use crate::config::UpdateTrack;
use crate::core::events::AppUpdateDownloadProgress;
pub use crate::core::events::AppUpdateProgress;
use crate::core::{AppState, CommandError, CommandErrorCode};
use crate::models::poison_policy::RecoveringMutex;

/// Endpoint override for the update manifest.
///
/// Honoured unconditionally in a DEBUG build - that is the manual test recipe
/// in docs/releasing.md, where a local `python3 -m http.server` stands in for
/// GitHub. In a release build it is restricted to https `github.com` URLs
/// ([`is_trusted_manifest_url`]): the manifest decides which binary an
/// installed app downloads, so an attacker who can set one environment
/// variable must not be able to redirect it. (The plugin's own
/// `validate_endpoints` already refuses non-https in release; this narrows the
/// host as well.)
///
/// Read at CALL time, so exporting it and re-checking is enough - no restart.
pub const UPDATE_URL_ENV: &str = "STALAGMIGHT_UPDATE_URL";

/// Per-track manifests, both rolling assets of the `updater` release.
const STABLE_ENDPOINT: &str =
    "https://github.com/Stalagmight/Stalagmight/releases/download/updater/latest-stable.json";
const WEEKLY_ENDPOINT: &str =
    "https://github.com/Stalagmight/Stalagmight/releases/download/updater/latest-weekly.json";

/// The only host a release build will fetch a manifest from.
const TRUSTED_MANIFEST_HOST: &str = "github.com";

/// Release-notes landing page for an offered version. Tags are `v<version>`.
const RELEASE_TAG_URL_PREFIX: &str = "https://github.com/Stalagmight/Stalagmight/releases/tag/v";

/// Emit at most one progress event per this many bytes.
///
/// A release bundle arrives in chunks of a few KiB, so emitting per chunk would
/// put thousands of messages through the IPC channel for one download - enough
/// to compete with the UI it is meant to be updating. A quarter-megabyte step
/// is still ~40 updates for a 10 MB bundle, which is finer than a progress bar
/// can show.
const PROGRESS_EMIT_BYTES: u64 = 256 * 1024;

/// Hard ceiling on an update bundle, enforced mid-stream.
///
/// The plugin accumulates the entire response into a `Vec<u8>` and checks no
/// length whatsoever, so without this a manifest pointing at an endless body
/// grows the process until the OS kills it - and the manifest is exactly the
/// part of the chain nothing has signed yet. 512 MiB is roughly an order of
/// magnitude above the largest bundle this app ships (a Windows NSIS setup is
/// tens of MB), so it cannot be reached by a legitimate release; raise it here
/// rather than discovering it as a failed update if that stops being true.
const MAX_UPDATE_BYTES: u64 = 512 * 1024 * 1024;

/// Whole-request timeout for the manifest fetch.
///
/// A JSON document of a few hundred bytes. Without this the check inherits
/// reqwest's default of NO timeout, so an endpoint that accepts the connection
/// and then stalls leaves the check pending for the life of the process - and
/// the frontend's launch check would never resolve.
const CHECK_TIMEOUT: Duration = Duration::from_mins(1);

/// Whole-request timeout for the bundle download.
///
/// Set on the `Update` itself, NOT inherited from the builder:
/// `Updater::check` constructs the `Update` with `timeout: None` hard-coded
/// (tauri-plugin-updater 2.9.0, `updater.rs`), so the builder's timeout covers
/// the manifest fetch alone. Ten minutes is deliberately generous - it is a
/// backstop against a stalled connection, not a bandwidth policy, and a user
/// on a slow link must still be able to finish.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(10);

/// The answer to one update check.
#[derive(Debug, Clone, Serialize, TS)]
pub struct AppUpdateCheck {
    pub status: AppUpdateStatus,
    /// The version the RUNNING app was built as, so the UI can say "you have X"
    /// without a second round trip.
    pub current_version: String,
}

/// Whether an update is on offer.
///
/// A discriminated union rather than an `Option<info>`: "no update" and "there
/// is one" are different states in the banner, and the payload must not be able
/// to travel without the arm that says it is real.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppUpdateStatus {
    /// Nothing newer is published for this platform on this track. Also the
    /// answer when the manifest is missing entirely (see
    /// [`check_for_app_update`]).
    UpToDate,
    Available {
        version: String,
        notes_url: String,
    },
}

/// Where in the check -> download -> install cycle we are.
///
/// The [`Update`] cannot be rebuilt from anything the frontend holds (it
/// carries the signature and the resolved per-platform download URL), so it is
/// parked here between commands, and the verified bytes with it.
#[derive(Default)]
struct Pending {
    /// Bumped by every check. Everything downstream carries the generation it
    /// was produced under, so a download that finishes after a LATER check
    /// cannot have its bytes paired with that later check's `Update` - both
    /// would be validly signed, and the user would silently install a version
    /// nobody offered them. See [`store_downloaded_bytes`].
    generation: u64,
    update: Option<Update>,
    /// The verified bundle and the generation it was downloaded for.
    bytes: Option<(u64, Vec<u8>)>,
    /// One-shot consent for [`install_app_update_and_restart`].
    ///
    /// The frontend's exit guards - unsaved changes, unpushed SpeleoDB work -
    /// are frontend code, and any script running in the webview can `invoke`
    /// the install command directly and walk straight past them. This flag is
    /// the backend half of that contract: the install refuses unless the quit
    /// path armed it, and it is cleared by the install, by a new check, and by
    /// [`unarm_app_update_restart`] on every abandoned restart.
    restart_armed: bool,
}

/// Tauri-managed state for the update cycle.
///
/// Its own managed struct rather than a field on `AppState`: `AppState` is the
/// survey document and its undo stack, and this outlives every project the user
/// opens. `PendingLaunchFiles` sets the same precedent.
///
/// A [`RecoveringMutex`] like every other lock in the backend (#576 A11,
/// docs/architecture.md "Mutex Poisoning"): a poisoned cell is recovered and
/// raises the internal-error banner. The install alone refuses one instead
/// ([`take_for_install`]), because it would hand whatever a crashed holder left
/// to the installer.
pub struct AppUpdateState(RecoveringMutex<Pending>);

impl Default for AppUpdateState {
    fn default() -> Self {
        Self(RecoveringMutex::new("app_update", Pending::default()))
    }
}

impl AppUpdateState {
    fn with<T>(&self, f: impl FnOnce(&mut Pending) -> T) -> T {
        f(&mut self.0.lock())
    }
}

/// Does this build have an updater public key configured?
///
/// Read from the checked-in `src-tauri/tauri.conf.json` - the SAME fact, from
/// the SAME file, that `.github/workflows/release.yml` reads with
/// `jq -r '.plugins.updater.pubkey // empty'` to decide whether to enable
/// updater artifacts. CI's `--config` merge adds only
/// `bundle.createUpdaterArtifacts`, never the pubkey, so `include_str!` and the
/// merged config cannot disagree about this key.
///
/// It gates plugin registration in `lib.rs`, and it has to: the plugin's own
/// `Config` has a REQUIRED `pubkey` and no `Default`, so with `plugins.updater`
/// absent Tauri fails to deserialize it and `Builder::build()` returns an
/// error, so the app never starts. The checked-in config carries the key
/// (committed for #579), so every build of this tree answers `true`; the gate
/// is what lets a config without the block - a fork that drops it, or the key
/// taken out again - still start, with the updater off, instead of not at all.
pub fn updater_configured() -> bool {
    static CONFIGURED: OnceLock<bool> = OnceLock::new();
    *CONFIGURED.get_or_init(|| pubkey_present(include_str!("../../tauri.conf.json")))
}

/// [`updater_configured`]'s decision, over an explicit document so a test can
/// drive both answers without a second `tauri.conf.json`.
fn pubkey_present(conf_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(conf_json)
        .ok()
        .and_then(|conf| {
            conf.get("plugins")?
                .get("updater")?
                .get("pubkey")?
                .as_str()
                .map(|key| !key.trim().is_empty())
        })
        .unwrap_or(false)
}

/// The error every command answers with while the updater has no public key.
fn updater_not_configured() -> String {
    "This build has no update signing key, so it cannot check for updates".to_owned()
}

/// Why THIS install cannot replace itself with what the manifest offers, or
/// `None` when it can.
///
/// A manifest carries one entry per OS/arch - the AppImage for Linux, the NSIS
/// `-setup.exe` for Windows (`scripts/gen-updater-manifest.mjs`) - and
/// tauri-plugin-updater 2.9 looks entries up by `{os}-{arch}` alone, never by
/// how the running copy was installed. Without this gate a `.deb` install would
/// download an AppImage its `install_deb` path rejects, an `.rpm` install would
/// try to overwrite its package-owned binary with an AppImage, and an `.msi`
/// install would run the NSIS installer and end up with a second, separately
/// registered copy. Those installs update through their package instead.
///
/// `bundle` comes from the marker the bundler patches into each packaged
/// binary. An unmarked binary (a bare `cargo build`, or a packaging step that
/// stopped patching) is refused in a release build - fail closed, the updater
/// goes quiet - but allowed in a debug build so the dev recipe
/// (`STALAGMIGHT_UPDATE_URL`) keeps working.
fn self_update_refusal(bundle: Option<BundleType>, release_build: bool) -> Option<String> {
    let package = match bundle {
        Some(BundleType::AppImage | BundleType::Nsis | BundleType::App | BundleType::Dmg) => {
            return None
        }
        None if !release_build => return None,
        None => {
            return Some(
                "This copy of Stalagmight was not installed from a release package, so it cannot update itself"
                    .to_owned(),
            )
        }
        Some(BundleType::Deb) => ".deb",
        Some(BundleType::Rpm) => ".rpm",
        Some(BundleType::Msi) => ".msi",
    };
    Some(format!(
        "Stalagmight was installed from a {package} package, which the in-app updater cannot replace; install the new version the same way"
    ))
}

/// Is this a manifest URL a release build may fetch?
///
/// https, and `github.com` exactly - not a suffix match, which would accept
/// `github.com.evil.test`. Release assets live on `objects.githubusercontent.com`,
/// but that host is only ever reached by following a redirect the manifest
/// names, never configured here.
fn is_trusted_manifest_url(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    parsed.scheme() == "https" && parsed.host_str() == Some(TRUSTED_MANIFEST_HOST)
}

/// Decide what [`UPDATE_URL_ENV`] means, given the raw value and whether the
/// host allowlist is being enforced.
///
/// Split out and parameterised because `cfg!(debug_assertions)` is ALWAYS true
/// under `cargo test`: the release branch - the one that matters for security -
/// would otherwise have no test that can reach it.
fn resolve_override(raw: Option<&str>, enforce_allowlist: bool) -> Option<String> {
    // An exported-then-emptied variable (`export STALAGMIGHT_UPDATE_URL=`) is
    // present but blank; honouring it would send the check at an unparseable URL.
    let value = raw.map(str::trim).filter(|v| !v.is_empty())?;
    if enforce_allowlist && !is_trusted_manifest_url(value) {
        tracing::warn!(
            purpose = "override the update manifest endpoint",
            "Ignoring {UPDATE_URL_ENV}: a release build only accepts an https \
             {TRUSTED_MANIFEST_HOST} URL"
        );
        return None;
    }
    Some(value.to_owned())
}

/// The manifest URL for `track`, or the [`UPDATE_URL_ENV`] override.
///
/// Pure apart from the environment, which is read here rather than cached so a
/// support session can point a running app somewhere else between two checks.
pub fn endpoint_for_track(track: UpdateTrack) -> String {
    let raw = std::env::var(UPDATE_URL_ENV).ok();
    if let Some(overridden) = resolve_override(raw.as_deref(), !cfg!(debug_assertions)) {
        return overridden;
    }
    match track {
        UpdateTrack::Stable => STABLE_ENDPOINT,
        UpdateTrack::Weekly => WEEKLY_ENDPOINT,
    }
    .to_owned()
}

/// Why a download must stop before the body ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DownloadAbort {
    /// More bytes than any bundle we ship could plausibly be.
    OverCap,
    /// More bytes than the server itself said it would send.
    OverDeclaredLength,
}

impl DownloadAbort {
    fn message(self) -> String {
        match self {
            Self::OverCap => format!(
                "The update download exceeded the {} MiB limit and was stopped",
                MAX_UPDATE_BYTES / 1024 / 1024
            ),
            Self::OverDeclaredLength => {
                "The update download sent more data than it declared and was stopped".to_owned()
            }
        }
    }
}

/// Should the in-flight download be abandoned?
///
/// Pure, so the three ways a body can be hostile are testable without a server.
/// The declared-length arm is checked BEFORE any bytes arrive as well as
/// against the running total, so a manifest advertising a 4 GiB bundle fails on
/// the first chunk instead of after half a gigabyte of RAM.
fn download_abort_reason(downloaded: u64, content_length: Option<u64>) -> Option<DownloadAbort> {
    if downloaded > MAX_UPDATE_BYTES {
        return Some(DownloadAbort::OverCap);
    }
    match content_length {
        Some(declared) if declared > MAX_UPDATE_BYTES => Some(DownloadAbort::OverCap),
        Some(declared) if downloaded > declared => Some(DownloadAbort::OverDeclaredLength),
        _ => None,
    }
}

/// The parked update and the generation it belongs to.
///
/// Cloned out rather than borrowed: the caller awaits a download afterwards,
/// and holding the lock across that await is exactly what `await_holding_lock`
/// forbids.
fn checked_update(update_state: &AppUpdateState) -> Result<(u64, Update), String> {
    update_state
        .with(|pending| pending.update.clone().map(|u| (pending.generation, u)))
        .ok_or_else(|| "No update to download; check for updates first".to_owned())
}

/// Park downloaded bytes, but only if no check has happened since the download
/// started.
///
/// The refusal is the point: a second check while a download is in flight has
/// already replaced `pending.update`, so storing here would pair one version's
/// bytes with another version's `Update`, and the install would apply a build
/// nobody was offered.
fn store_downloaded_bytes(
    update_state: &AppUpdateState,
    generation: u64,
    bytes: Vec<u8>,
) -> Result<(), String> {
    update_state.with(|pending| {
        if pending.generation != generation {
            return Err(
                "A newer update check replaced this one while it was downloading; check again"
                    .to_owned(),
            );
        }
        pending.bytes = Some((generation, bytes));
        Ok(())
    })
}

/// Arm the one-shot install consent. [`arm_app_update_restart`]'s whole body,
/// as a plain function so the state machine is reachable from a unit test.
fn arm_restart(update_state: &AppUpdateState) -> Result<(), String> {
    update_state.with(|pending| {
        match pending.bytes {
            Some((generation, _)) if generation == pending.generation => {}
            _ => {
                return Err(
                    "There is no downloaded update to restart into; download it first".to_owned(),
                )
            }
        }
        pending.restart_armed = true;
        Ok(())
    })
}

/// Spend the install consent and take everything the install needs, in ONE
/// critical section.
///
/// Taking rather than peeking is what makes the arm a one-shot: a second
/// install cannot run off consent the first one already spent, and a failed
/// install has to be re-armed through the guarded path like any other restart.
/// The arm is checked FIRST, so an unarmed `invoke` from the webview learns
/// nothing about whether an update is even pending.
///
/// One acquisition, not two, because the pair is the whole point. Taking the
/// bytes under one lock and cloning the `Update` under a second left a window
/// in which a check could complete: the bytes came from the old offer and the
/// `Update` (with its signature and download URL) from the new one, which is
/// exactly the mispairing the generation exists to prevent.
///
/// Takes `&mut Pending` rather than the state, so the state machine is
/// reachable from a unit test without a live plugin behind `Update`.
fn take_installable(pending: &mut Pending) -> Result<(u64, Update, Vec<u8>), String> {
    if !std::mem::take(&mut pending.restart_armed) {
        return Err(
            "The update restart was not armed; use Restart to update in the app".to_owned(),
        );
    }
    let Some((generation, bytes)) = pending.bytes.take() else {
        return Err("The update has not been downloaded yet; download it first".to_owned());
    };
    if generation != pending.generation {
        return Err(
            "The downloaded update no longer matches the latest check; check again".to_owned(),
        );
    }
    // Unreachable in practice - bytes only ever land through
    // `store_downloaded_bytes`, which runs after a check parked an `Update` of
    // the same generation - but "cannot happen" is not a reason to unwrap.
    let Some(update) = pending.update.clone() else {
        return Err("No update to install; check for updates first".to_owned());
    };
    Ok((generation, update, bytes))
}

/// The authorised bytes, the [`Update`] that will apply them, and the
/// generation both belong to.
fn installable_update(update_state: &AppUpdateState) -> Result<(u64, Update, Vec<u8>), String> {
    update_state.with(take_installable)
}

/// What the install takes, refused outright when the cell is poisoned.
///
/// The one updater operation that does not recover a poisoned cell (#576 A11):
/// a crash while the cell was held may have left the bytes, the `Update` and
/// the generation half-paired, and this is the step that hands them to the
/// installer - exactly what the generation exists to prevent. Every other step
/// only reads or replaces the offer, and a re-check rebuilds it from scratch.
fn take_for_install(update_state: &AppUpdateState) -> Result<(u64, Update, Vec<u8>), CommandError> {
    update_state.0.refuse_if_poisoned().map_err(|refused| {
        CommandError::new(
            CommandErrorCode::RefusedAfterInternalError,
            format!("The update was not installed: {refused}; restart the app and update again"),
        )
    })?;
    installable_update(update_state).map_err(CommandError::failed)
}

/// Put bytes back after a failed install - but only if they are still the
/// current offer.
///
/// A check that completed while the install was running has already replaced
/// the parked `Update`, so restoring unconditionally would re-create the
/// mispairing: old bytes filed under the new generation, ready for the next
/// arm to hand them to a different version's installer. They are dropped
/// instead, and the banner's re-check offers the new download.
fn restore_bytes_if_current(update_state: &AppUpdateState, generation: u64, bytes: Vec<u8>) {
    update_state.with(|pending| {
        if pending.generation == generation {
            pending.bytes = Some((generation, bytes));
        }
    });
}

/// Ask this track's manifest whether a newer build exists.
///
/// Deliberately does NOT consult `update_check_enabled`: the caller decides
/// whether to ask at all (the launch check is gated on the setting; the About
/// dialog's button is an explicit request and must work regardless). Making
/// this refuse would silently break the manual button.
///
/// "There is no manifest" and "the manifest has no entry for this platform" map
/// to [`AppUpdateStatus::UpToDate`], not to an error: both are the ordinary
/// state of a track that has not published for this platform yet (macOS stays
/// out of the manifests until Developer ID signing is live), and neither is
/// something the user can act on. A transport failure IS an error, so the
/// frontend can stay quiet about it rather than claim the app is current.
///
/// [#576 C2/C4] `async` (the network wait runs on the async runtime, never on the
/// main thread) and answers a [`CommandError`]: `FAILED`, with the message
/// below. The frontend routes on none of them - a failed check goes quietly to
/// idle - so no update code of its own is needed.
#[tauri::command]
pub async fn check_for_app_update<R: Runtime>(
    app: AppHandle<R>,
) -> Result<AppUpdateCheck, CommandError> {
    check(&app).await.map_err(CommandError::failed)
}

/// [`check_for_app_update`]'s body.
async fn check<R: Runtime>(app: &AppHandle<R>) -> Result<AppUpdateCheck, String> {
    let update_state = app.state::<AppUpdateState>();
    // The plugin is not registered without a pubkey (see `updater_configured`),
    // and `updater_builder()` PANICS on its unmanaged state - so this guard is
    // load-bearing, not defensive.
    if !updater_configured() {
        return Err(updater_not_configured());
    }
    // Before any state is touched, for the same reason as the transport-failure
    // arm below. Gating the check is enough: download and install refuse
    // without the `Update` only a successful check parks.
    if let Some(refusal) = self_update_refusal(
        tauri::utils::platform::bundle_type(),
        !cfg!(debug_assertions),
    ) {
        return Err(refusal);
    }
    let current_version = app.package_info().version.to_string();
    let track = app.state::<AppState>().get_config().general.update_track;
    let endpoint = endpoint_for_track(track);
    let url = Url::parse(&endpoint)
        .map_err(|e| format!("Update endpoint '{endpoint}' is not a valid URL: {e}"))?;

    // [#576] What a quit does on its way out, on the one install path that skips
    // it: on Windows `Update::install` hands over to the installer and calls
    // `std::process::exit` itself, so `RunEvent::ExitRequested` - where a quit
    // releases a held SpeleoDB write lock - never runs. This hook runs right
    // before that exit (replacing the plugin's default, which only cleaned up).
    // Elsewhere the install returns and `app.restart()`, called off the main
    // thread, goes through `ExitRequested` like a quit.
    let exiting = app.clone();
    let updater = app
        .updater_builder()
        .on_before_exit(move || {
            crate::release_held_lock_on_exit(&exiting);
            exiting.cleanup_before_exit();
        })
        .timeout(CHECK_TIMEOUT)
        .endpoints(vec![url])
        .map_err(|e| format!("Update endpoint was rejected: {e}"))?
        .build()
        .map_err(|e| format!("Could not build the updater: {e}"))?;

    let found = match updater.check().await {
        Ok(found) => found,
        // `ReleaseNotFound` is what a 404 (or an unparseable body) on every
        // endpoint reduces to; `TargetNotFound` is a manifest with no entry for
        // this OS/arch.
        Err(
            tauri_plugin_updater::Error::ReleaseNotFound
            | tauri_plugin_updater::Error::TargetNotFound(_),
        ) => None,
        // Returns BEFORE any of the state mutations below, and that ordering is
        // load-bearing: a check that never reached the server knows nothing
        // about the offer already on the table, so it must not bump the
        // generation. Bumping here would invalidate a COMPLETED download - the
        // user's finished "Restart to update" would start refusing on the
        // strength of one failed background poll. The frontend store mirrors
        // this by leaving an actionable banner alone on a failed re-check.
        Err(e) => return Err(format!("Update check failed: {e}")),
    };

    // Every check invalidates the previous one, found or not: an update the
    // server has since withdrawn must not stay installable, and consent given
    // for the old offer must not carry over to a new one.
    let Some(mut update) = found else {
        update_state.with(|pending| {
            pending.generation = pending.generation.wrapping_add(1);
            pending.update = None;
            pending.bytes = None;
            pending.restart_armed = false;
        });
        return Ok(AppUpdateCheck {
            status: AppUpdateStatus::UpToDate,
            current_version,
        });
    };

    // See `DOWNLOAD_TIMEOUT`: the builder's timeout never reaches the body.
    update.timeout = Some(DOWNLOAD_TIMEOUT);
    let version = update.version.clone();

    update_state.with(|pending| {
        pending.generation = pending.generation.wrapping_add(1);
        pending.update = Some(update);
        pending.bytes = None;
        pending.restart_armed = false;
    });

    Ok(AppUpdateCheck {
        status: AppUpdateStatus::Available {
            notes_url: format!("{RELEASE_TAG_URL_PREFIX}{version}"),
            version,
        },
        current_version,
    })
}

/// Download the update the last check found, and verify its signature.
///
/// Download ONLY - see the module docs for why this never installs. The bytes
/// are held in memory until [`install_app_update_and_restart`] asks for them;
/// the plugin has already checked them against the baked-in minisign public key
/// by the time this returns.
///
/// [#576 C2/C4] `async`, the download awaited on the async runtime; failures are
/// `FAILED` [`CommandError`]s.
#[tauri::command]
pub async fn download_app_update<R: Runtime>(app: AppHandle<R>) -> Result<(), CommandError> {
    download(&app).await.map_err(CommandError::failed)
}

/// [`download_app_update`]'s body.
async fn download<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let update_state = app.state::<AppUpdateState>();
    let (generation, update) = checked_update(&update_state)?;

    // Shared with the progress closure, which runs inside the download future
    // and therefore cannot return anything to us.
    let abort = AtomicBool::new(false);
    let mut reason: Option<DownloadAbort> = None;
    let mut downloaded: u64 = 0;
    let mut emitted_at: u64 = 0;
    let mut total: Option<u64> = None;

    // The whole borrow of the counters above lives in this block: the closure
    // holds them for as long as the future does, so the final progress emit and
    // the result handling have to happen after it is dropped.
    let outcome = {
        let abort = &abort;
        let mut download = Box::pin(update.download(
            |chunk, content_length| {
                downloaded += chunk as u64;
                if let Some(why) = download_abort_reason(downloaded, content_length) {
                    reason = Some(why);
                    abort.store(true, Ordering::Relaxed);
                    return;
                }
                // Emit on the byte step OR when the announced total changes -
                // the latter is what makes the first chunk emit, so the UI
                // learns the size instead of waiting a quarter megabyte for it.
                if downloaded - emitted_at >= PROGRESS_EMIT_BYTES || content_length != total {
                    emitted_at = downloaded;
                    total = content_length;
                    emit_progress(app, downloaded, total);
                }
            },
            || {},
        ));

        abortable(download.as_mut(), abort).await
    };

    match outcome {
        Some(Ok(bytes)) => {
            // The throttle can swallow the tail, and a bar stuck at 97% reads
            // as a hang; the finished size is also the one the frontend can
            // assert on.
            emit_progress(app, downloaded, total);
            store_downloaded_bytes(&update_state, generation, bytes)
        }
        Some(Err(e)) => Err(format!("Update download failed: {e}")),
        // The abort path. `reason` is always set alongside the flag; the
        // fallback keeps the failure honest rather than unwrapping.
        None => Err(reason.unwrap_or(DownloadAbort::OverCap).message()),
    }
}

/// Drive `fut` to completion, abandoning it the moment `abort` is set.
///
/// The plugin's `download` has no way to be cancelled from the progress
/// callback, so it is cancelled from the outside: stop polling and drop the
/// future, which frees the partially-accumulated buffer. The callback runs
/// INSIDE `poll`, so the flag is current the instant that poll returns.
///
/// The order of the two checks is load-bearing. The flag is tested BEFORE the
/// poll's own result, because the poll that receives the final chunk is also
/// the poll that can return `Ready` - a body whose last chunk crosses the cap
/// sets the flag and completes in the same call. Honouring `Ready` first
/// accepted exactly the fully-buffered payload the cap exists to refuse.
async fn abortable<F: Future>(mut fut: Pin<&mut F>, abort: &AtomicBool) -> Option<F::Output> {
    std::future::poll_fn(|cx| {
        let polled = fut.as_mut().poll(cx);
        if abort.load(Ordering::Relaxed) {
            return Poll::Ready(None);
        }
        match polled {
            Poll::Ready(value) => Poll::Ready(Some(value)),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

/// [#576 C13] Through the event manifest: the name and payload are
/// [`AppUpdateDownloadProgress`]'s, so the frontend listens with
/// `listenIpc(IPC_EVENTS.APP_UPDATE_DOWNLOAD_PROGRESS, ...)`.
fn emit_progress<R: Runtime>(app: &AppHandle<R>, downloaded: u64, total: Option<u64>) {
    if let Err(e) =
        emit_event::<AppUpdateDownloadProgress, R>(app, AppUpdateProgress { downloaded, total })
    {
        // A dropped progress tick is cosmetic; the download itself carries on.
        tracing::warn!(
            error = %e,
            purpose = "report update download progress",
            "Emit failed"
        );
    }
}

/// Take the safety catch off the install, for exactly one attempt.
///
/// Called by `requestUpdateRestart` immediately before it asks for a window
/// close, so the install command can only fire on a restart the quit path
/// actually started. Refuses unless a download for the current check is sitting
/// ready - arming with nothing to install would be consent for whatever the
/// NEXT download turns out to be.
///
/// ## Threat model, and what this deliberately does NOT claim
///
/// A reasonable objection is that exposing the arm as its own IPC command lets
/// anything that can call the install also call the arm, so the guarantee looks
/// circular. It is worth stating plainly what the nonce is and is not for.
///
/// It does NOT defend against a fully compromised webview. It cannot: the
/// unsaved-changes and unpushed-SpeleoDB conditions live in the frontend
/// stores, so "have the exit guards passed?" is not a question Rust can answer
/// without moving that whole model across the boundary. And an attacker who can
/// run arbitrary JS here already has commands with a far worse blast radius
/// than a signed, in-place upgrade - `save_project`, `delete_project`, the
/// SpeleoDB push.
///
/// What it does defend against is narrower and real: a SINGLE stray
/// `invoke('install_app_update_and_restart')` - an XSS payload firing one call,
/// a frontend bug, a devtools invocation replayed out of context - can no
/// longer restart the app out from under unsaved work. Reaching the installer
/// now takes a deliberate two-step against state the backend maintains, which
/// is the difference between an accident and an intent.
///
/// Refusing to arm without a current download is the other half: the consent is
/// bound to one specific, already-verified bundle, not to "whatever is next".
///
/// [#576 C2] `async`, the body on the blocking pool like every command's.
#[tauri::command]
pub async fn arm_app_update_restart<R: Runtime>(app: AppHandle<R>) -> Result<(), CommandError> {
    pool::run_stateless(move || {
        arm_restart(app.state::<AppUpdateState>().inner()).map_err(CommandError::failed)
    })
    .await
}

/// Put the safety catch back.
///
/// Idempotent, and called on every path that abandons a restart - the user
/// cancels a guard dialog, the window close is refused, the install rejects -
/// so a restart the user walked away from cannot be completed by a later,
/// unrelated window close.
#[tauri::command]
pub async fn unarm_app_update_restart<R: Runtime>(app: AppHandle<R>) -> Result<(), CommandError> {
    pool::run_stateless(move || {
        app.state::<AppUpdateState>()
            .with(|pending| pending.restart_armed = false);
        Ok(())
    })
    .await
}

/// Install the downloaded update and relaunch into it.
///
/// The point of no return. It requires [`arm_app_update_restart`] to have run,
/// which is the backend's own guarantee that this arrived through the quit path
/// and not from a bare `invoke` in the webview; the frontend's exit guards
/// cannot provide that guarantee themselves.
///
/// On Windows `install` hands over to the installer and exits this process, so
/// nothing after it is guaranteed to run - which is also why `app.restart()`
/// (never-returning by signature) sits directly behind it rather than any
/// cleanup.
///
/// On failure the bytes are put back, so the restart can be retried - through
/// the guarded path, which re-arms - without downloading again.
///
/// [#576 C2] `async`, the body on the blocking pool - which is also what makes
/// the restart exit like a quit. `AppHandle::restart` on the MAIN thread (where
/// Tauri runs a non-`async` command) cleans up and re-executes at once, without
/// `RunEvent::ExitRequested`, so the exit-time release of a held SpeleoDB write
/// lock that every quit gets was skipped; off the main thread it requests the
/// exit, and the event loop runs `ExitRequested` before it restarts (tauri 2.9
/// `AppHandle::restart`). Windows, where the install exits the process itself,
/// gets the same release from the `on_before_exit` hook set at the check.
#[tauri::command]
pub async fn install_app_update_and_restart<R: Runtime>(
    app: AppHandle<R>,
) -> Result<(), CommandError> {
    pool::run_stateless(move || install_and_restart(&app)).await
}

/// [`install_app_update_and_restart`]'s body. Never returns on success.
fn install_and_restart<R: Runtime>(app: &AppHandle<R>) -> Result<(), CommandError> {
    let update_state = app.state::<AppUpdateState>();
    let (generation, update, bytes) = take_for_install(&update_state)?;

    if let Err(e) = update.install(&bytes) {
        // Filed under the generation they were DOWNLOADED for, not whatever is
        // current now - see `restore_bytes_if_current`.
        restore_bytes_if_current(&update_state, generation, bytes);
        return Err(CommandError::failed(format!("Update install failed: {e}")));
    }
    app.restart();
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test fixtures fail loudly; one test panics a holder to POISON the cell on purpose"
)]
mod tests {
    use super::*;

    /// The endpoint override is process-global, so the tests that touch it take
    /// this first. Without it they race each other and read one another's value.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Restores whatever the variable held (including "unset") on drop, so a
    /// developer running the suite with a real override exported does not have
    /// it silently cleared.
    struct EnvVarGuard(Option<String>);

    impl EnvVarGuard {
        fn set(value: Option<&str>) -> Self {
            let original = std::env::var(UPDATE_URL_ENV).ok();
            match value {
                Some(value) => std::env::set_var(UPDATE_URL_ENV, value),
                None => std::env::remove_var(UPDATE_URL_ENV),
            }
            Self(original)
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(original) => std::env::set_var(UPDATE_URL_ENV, original),
                None => std::env::remove_var(UPDATE_URL_ENV),
            }
        }
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `Update` has no `Debug`, so `expect_err` is unavailable on these.
    fn refusal<T>(result: Result<T, String>) -> String {
        result.err().expect("the step must refuse")
    }

    #[test]
    fn each_track_has_its_own_manifest() {
        let _lock = env_lock();
        let _env = EnvVarGuard::set(None);

        let stable = endpoint_for_track(UpdateTrack::Stable);
        let weekly = endpoint_for_track(UpdateTrack::Weekly);
        assert!(stable.ends_with("/latest-stable.json"), "{stable}");
        assert!(weekly.ends_with("/latest-weekly.json"), "{weekly}");
        assert_ne!(
            stable, weekly,
            "the two tracks must not resolve to one manifest"
        );
        // Both defaults must satisfy the release allowlist, or a release build
        // would refuse its own endpoints if they were ever routed through it.
        assert!(is_trusted_manifest_url(&stable), "{stable}");
        assert!(is_trusted_manifest_url(&weekly), "{weekly}");
    }

    #[test]
    fn the_env_override_wins_over_both_tracks() {
        let _lock = env_lock();
        let _env = EnvVarGuard::set(Some("http://127.0.0.1:8000/latest.json"));

        for track in [UpdateTrack::Stable, UpdateTrack::Weekly] {
            // Debug build (which every test binary is), so the local recipe URL
            // is honoured verbatim.
            assert_eq!(
                endpoint_for_track(track),
                "http://127.0.0.1:8000/latest.json"
            );
        }
    }

    #[test]
    fn a_blank_override_is_treated_as_unset() {
        let _lock = env_lock();
        let _env = EnvVarGuard::set(Some("   "));

        assert_eq!(endpoint_for_track(UpdateTrack::Stable), STABLE_ENDPOINT);
        assert_eq!(endpoint_for_track(UpdateTrack::Weekly), WEEKLY_ENDPOINT);
    }

    /// The override is read on every call, not captured once. A support session
    /// exports it against a running app and re-checks; caching would make that
    /// silently do nothing.
    #[test]
    fn the_override_is_re_read_on_every_call() {
        let _lock = env_lock();

        let _env = EnvVarGuard::set(None);
        assert_eq!(endpoint_for_track(UpdateTrack::Stable), STABLE_ENDPOINT);
        std::env::set_var(UPDATE_URL_ENV, "https://github.com/x/latest.json");
        assert_eq!(
            endpoint_for_track(UpdateTrack::Stable),
            "https://github.com/x/latest.json"
        );
        std::env::remove_var(UPDATE_URL_ENV);
        assert_eq!(endpoint_for_track(UpdateTrack::Stable), STABLE_ENDPOINT);
    }

    /// The security half of the override: in a RELEASE build anything that is
    /// not an https github.com URL is ignored, so one environment variable
    /// cannot redirect an installed app at an attacker's manifest.
    /// `cfg!(debug_assertions)` is always true under `cargo test`, which is why
    /// `resolve_override` takes the flag rather than reading it.
    #[test]
    fn a_release_build_only_accepts_a_github_https_override() {
        let allowed = "https://github.com/Stalagmight/Stalagmight/releases/download/updater/x.json";
        assert_eq!(
            resolve_override(Some(allowed), true).as_deref(),
            Some(allowed)
        );

        for rejected in [
            // The dev recipe - fine in debug, never in release.
            "http://127.0.0.1:8000/latest.json",
            // Plain http on the right host is still rewritable in transit.
            "http://github.com/x.json",
            // Suffix-matching the host would accept this.
            "https://github.com.evil.test/x.json",
            "https://evil.test/github.com/x.json",
            // A userinfo field that reads as github.com to a human.
            "https://github.com@evil.test/x.json",
            "not a url at all",
        ] {
            assert_eq!(
                resolve_override(Some(rejected), true),
                None,
                "release builds must reject {rejected}"
            );
            // ...and a debug build still honours it, so the recipe works.
            assert_eq!(
                resolve_override(Some(rejected), false).as_deref(),
                Some(rejected),
                "debug builds must honour {rejected}"
            );
        }
    }

    #[test]
    fn the_notes_url_points_at_the_offered_versions_tag() {
        // Mirrors what `check_for_app_update` builds, which is the one place the
        // `v` prefix and the tag shape are decided.
        assert_eq!(
            format!("{RELEASE_TAG_URL_PREFIX}0.5.1-weekly.20260817"),
            "https://github.com/Stalagmight/Stalagmight/releases/tag/v0.5.1-weekly.20260817"
        );
    }

    // ── Download bounds ──────────────────────────────────────────────────────

    #[test]
    fn an_ordinary_download_is_never_aborted() {
        assert_eq!(download_abort_reason(0, None), None);
        assert_eq!(download_abort_reason(40 * 1024 * 1024, None), None);
        assert_eq!(
            download_abort_reason(40 * 1024 * 1024, Some(40 * 1024 * 1024)),
            None
        );
        // Exactly at the cap is still allowed; only past it is not.
        assert_eq!(download_abort_reason(MAX_UPDATE_BYTES, None), None);
    }

    /// A body with no `Content-Length` that just keeps coming - the shape the
    /// plugin has no defence against at all, since it buffers everything.
    #[test]
    fn an_endless_body_is_aborted_at_the_cap() {
        assert_eq!(
            download_abort_reason(MAX_UPDATE_BYTES + 1, None),
            Some(DownloadAbort::OverCap)
        );
    }

    /// A manifest that ADMITS to an enormous bundle is refused on the first
    /// chunk, rather than after half a gigabyte of it has been accumulated.
    #[test]
    fn an_oversized_declared_length_is_refused_immediately() {
        assert_eq!(
            download_abort_reason(1, Some(MAX_UPDATE_BYTES + 1)),
            Some(DownloadAbort::OverCap)
        );
    }

    /// A server that declares a small body and then sends more is lying about
    /// one of the two; either way the extra bytes are not accumulated.
    #[test]
    fn overrunning_a_declared_length_is_aborted() {
        assert_eq!(
            download_abort_reason(1_001, Some(1_000)),
            Some(DownloadAbort::OverDeclaredLength)
        );
        assert_eq!(download_abort_reason(1_000, Some(1_000)), None);
    }

    #[test]
    fn each_abort_reason_names_itself() {
        assert!(DownloadAbort::OverCap.message().contains("512 MiB"));
        assert!(DownloadAbort::OverDeclaredLength
            .message()
            .contains("more data than it declared"));
    }

    // ── Updater configuration ────────────────────────────────────────────────

    #[test]
    fn a_pubkey_is_only_present_when_it_is_a_non_empty_string() {
        assert!(pubkey_present(
            r#"{"plugins":{"updater":{"pubkey":"abc"}}}"#
        ));
        assert!(!pubkey_present(r#"{"plugins":{"updater":{"pubkey":""}}}"#));
        assert!(!pubkey_present(
            r#"{"plugins":{"updater":{"pubkey":"  "}}}"#
        ));
        assert!(!pubkey_present(r#"{"plugins":{"updater":{}}}"#));
        assert!(!pubkey_present(r#"{"plugins":{}}"#));
        assert!(!pubkey_present("{}"));
        assert!(!pubkey_present("not json"));
    }

    /// `updater_configured` and CI's `jq -r '.plugins.updater.pubkey // empty'`
    /// must always agree - CI decides whether to build updater artifacts from
    /// it, this decides whether the app registers the plugin at all, and a
    /// disagreement means either signed artifacts nothing can install or an app
    /// that refuses to start.
    #[test]
    fn the_plugin_gate_matches_the_committed_conf() {
        let conf = include_str!("../../tauri.conf.json");
        assert_eq!(
            updater_configured(),
            pubkey_present(conf),
            "the cached gate disagrees with tauri.conf.json"
        );
    }

    /// Only the installs the manifest's artifacts can replace may check; every
    /// package-managed install is refused by name, and an unmarked binary only
    /// in a release build.
    #[test]
    fn only_self_replaceable_installs_may_check() {
        for allowed in [
            BundleType::AppImage,
            BundleType::Nsis,
            BundleType::App,
            BundleType::Dmg,
        ] {
            assert_eq!(
                self_update_refusal(Some(allowed.clone()), true),
                None,
                "{allowed:?}"
            );
        }
        for (refused, package) in [
            (BundleType::Deb, ".deb"),
            (BundleType::Rpm, ".rpm"),
            (BundleType::Msi, ".msi"),
        ] {
            for release_build in [true, false] {
                // An allowed install yields "", which names no package.
                let message =
                    self_update_refusal(Some(refused.clone()), release_build).unwrap_or_default();
                assert!(
                    message.contains(package),
                    "{refused:?} must be refused naming {package}, got {message:?}"
                );
            }
        }
        assert!(self_update_refusal(None, true).is_some());
        assert_eq!(self_update_refusal(None, false), None);
    }

    // ── State machine ────────────────────────────────────────────────────────

    /// Nothing checked yet: every later step must refuse, and name the step the
    /// user is missing rather than surfacing a lock or unwrap failure.
    #[test]
    fn every_step_refuses_an_unchecked_state() {
        let state = AppUpdateState::default();

        let download = refusal(checked_update(&state));
        assert!(download.contains("check for updates"), "{download}");

        let arm = refusal(arm_restart(&state));
        assert!(arm.contains("download it first"), "{arm}");

        // Install refuses on the ARM first - the safety catch is checked before
        // anything else, so a bare `invoke` gets no information about state.
        let install = refusal(installable_update(&state));
        assert!(install.contains("not armed"), "{install}");
    }

    /// The install cannot run on consent that was never given, even with a
    /// perfectly good download sitting ready. This is the guard that a webview
    /// script calling `invoke('install_app_update_and_restart')` hits.
    #[test]
    fn install_refuses_without_an_arm() {
        let state = AppUpdateState::default();
        seed_download(&state, 1);

        let err = refusal(installable_update(&state));
        assert!(err.contains("not armed"), "{err}");
        // ...and the refusal did not consume the download.
        assert!(state.with(|pending| pending.bytes.is_some()));
    }

    /// The arm is one-shot: consumed by the install attempt it authorised, so a
    /// second attempt has to be re-armed through the guarded path.
    #[test]
    fn the_arm_is_spent_by_one_install_attempt() {
        let state = AppUpdateState::default();
        seed_download(&state, 1);
        arm_restart(&state).expect("arming a ready download should work");

        assert!(state.with(|pending| pending.restart_armed));

        // An `Update` cannot be constructed outside a live plugin, so the
        // attempt gets all the way to the LAST check and stops there - which is
        // itself the proof that the arm and the bytes were accepted and taken.
        let attempt = refusal(installable_update(&state));
        assert!(attempt.contains("No update to install"), "{attempt}");
        assert!(!state.with(|pending| pending.restart_armed));
        assert!(state.with(|pending| pending.bytes.is_none()));

        let second = refusal(installable_update(&state));
        assert!(second.contains("not armed"), "{second}");
    }

    /// Unarming is what every abandoned restart does - a cancelled guard
    /// dialog, a refused close - and it must actually disarm.
    #[test]
    fn unarming_revokes_the_consent() {
        let state = AppUpdateState::default();
        seed_download(&state, 1);
        arm_restart(&state).expect("arming a ready download should work");
        state.with(|pending| pending.restart_armed = false);

        let err = refusal(installable_update(&state));
        assert!(err.contains("not armed"), "{err}");
    }

    /// The pairing race, from the download's side: a check that lands while the
    /// download is in flight bumps the generation, and the finishing download
    /// must NOT write its bytes against the new check's update. Both bundles
    /// carry a valid signature, so nothing downstream would notice.
    #[test]
    fn a_check_during_a_download_refuses_the_late_bytes() {
        let state = AppUpdateState::default();
        let generation = state.with(|pending| {
            pending.generation = 7;
            pending.generation
        });

        // A second check lands mid-download.
        state.with(|pending| {
            pending.generation += 1;
            pending.bytes = None;
        });

        let err = refusal(store_downloaded_bytes(&state, generation, vec![0u8; 8]));
        assert!(err.contains("newer update check"), "{err}");
        assert!(state.with(|pending| pending.bytes.is_none()));
    }

    /// ...and from the install's side, for any way a stale pair could still be
    /// sitting in the state.
    #[test]
    fn install_refuses_a_mismatched_pair() {
        let state = AppUpdateState::default();
        seed_download(&state, 3);
        arm_restart(&state).expect("arming a ready download should work");
        // The check that invalidates it lands after the arm.
        state.with(|pending| pending.generation = 4);

        let err = refusal(installable_update(&state));
        assert!(err.contains("no longer matches"), "{err}");
    }

    /// A failed install hands the bytes back so "Restart to update" can be
    /// retried without re-downloading - but only while they are still the offer
    /// on the table.
    #[test]
    fn a_failed_install_restores_bytes_while_the_offer_stands() {
        let state = AppUpdateState::default();
        state.with(|pending| pending.generation = 5);

        restore_bytes_if_current(&state, 5, vec![7u8; 3]);

        assert_eq!(
            state.with(|pending| pending.bytes.clone()),
            Some((5, vec![7u8; 3]))
        );
    }

    /// ...and drops them when a check completed while the install was running.
    /// Restoring unconditionally would file the OLD bytes under the NEW
    /// generation, re-creating exactly the mispairing the generation exists to
    /// prevent - the next arm would hand a different version's installer a
    /// bundle it was never offered.
    #[test]
    fn a_failed_install_drops_bytes_a_newer_check_superseded() {
        let state = AppUpdateState::default();
        state.with(|pending| pending.generation = 5);

        // The install was running under generation 5; a check landed and made
        // it 6 before the failure came back.
        state.with(|pending| pending.generation = 6);
        restore_bytes_if_current(&state, 5, vec![7u8; 3]);

        assert!(
            state.with(|pending| pending.bytes.is_none()),
            "bytes from a superseded offer must not be filed under the new one"
        );
    }

    // ── Download cancellation ────────────────────────────────────────────────

    /// Poll `fut` once through [`abortable`] with a no-op waker, which is all
    /// the ordering question needs: does one poll that both sets the flag and
    /// returns `Ready` count as an abort, or as a success?
    fn poll_abortable_once<F: Future>(fut: F, abort: &AtomicBool) -> Poll<Option<F::Output>> {
        let mut fut = std::pin::pin!(fut);
        let mut wrapper = std::pin::pin!(abortable(fut.as_mut(), abort));
        let waker = std::task::Waker::noop();
        wrapper
            .as_mut()
            .poll(&mut std::task::Context::from_waker(waker))
    }

    /// [review] The cap was checked AFTER the poll's own result, so a body
    /// whose FINAL chunk crosses the limit was accepted: the callback sets the
    /// flag and the very same poll returns `Ready` with the fully-buffered
    /// payload - the exact thing the cap exists to refuse, already in memory.
    #[test]
    fn a_ready_result_from_the_poll_that_tripped_the_cap_is_still_aborted() {
        let abort = AtomicBool::new(false);
        let tripped = &abort;
        let ready_but_over_cap = std::future::poll_fn(move |_| {
            // Stands in for the last chunk arriving: the callback trips the cap
            // and the download completes, both inside this one poll.
            tripped.store(true, Ordering::Relaxed);
            Poll::Ready("the whole oversized body")
        });

        assert_eq!(
            poll_abortable_once(ready_but_over_cap, &abort),
            Poll::Ready(None),
            "a result produced by the poll that tripped the cap must not be accepted"
        );
    }

    /// ...while an untripped download still delivers its result.
    #[test]
    fn an_untripped_download_is_handed_back_normally() {
        let abort = AtomicBool::new(false);
        assert_eq!(
            poll_abortable_once(std::future::ready("body"), &abort),
            Poll::Ready(Some("body"))
        );
    }

    /// A download still in flight when the cap trips ends immediately, without
    /// waiting for a body that would never stop arriving.
    #[test]
    fn a_pending_download_is_abandoned_as_soon_as_the_cap_trips() {
        let abort = AtomicBool::new(true);
        assert_eq!(
            poll_abortable_once(std::future::pending::<&str>(), &abort),
            Poll::Ready(None)
        );
    }

    #[test]
    fn a_download_for_the_current_check_is_stored() {
        let state = AppUpdateState::default();
        let generation = state.with(|pending| pending.generation);
        store_downloaded_bytes(&state, generation, vec![1u8; 4]).expect("should store");
        assert_eq!(
            state.with(|pending| pending.bytes.clone()),
            Some((generation, vec![1u8; 4]))
        );
    }

    /// Put the state where a real check + download would leave it, without an
    /// `Update` (which needs a live plugin). Anything that needs the `Update`
    /// itself is covered by the command-level paths, not here.
    fn seed_download(state: &AppUpdateState, generation: u64) {
        state.with(|pending| {
            pending.generation = generation;
            pending.bytes = Some((generation, vec![0u8; 16]));
        });
    }

    /// Poison the cell the way a crash would: a holder panics.
    fn poison(state: &AppUpdateState) {
        let _ = std::thread::scope(|s| {
            s.spawn(|| {
                let _guard = state.0.lock();
                panic!("poison the update state on purpose");
            })
            .join()
        });
        assert!(state.0.is_poisoned(), "precondition: the cell is poisoned");
    }

    /// [#576 A11] The install is the one step that refuses a poisoned cell, with
    /// the code that says why - it would hand the installer whatever a crashed
    /// holder left. Master refused every step there; the rest now recover.
    #[test]
    fn a_poisoned_state_refuses_the_install_and_recovers_for_the_rest() {
        let state = AppUpdateState::default();
        seed_download(&state, 3);
        state.with(|pending| pending.restart_armed = true);
        poison(&state);

        let refused = take_for_install(&state)
            .err()
            .expect("the install must refuse");
        assert_eq!(refused.code, CommandErrorCode::RefusedAfterInternalError);
        assert!(
            state.with(|pending| pending.restart_armed && pending.bytes.is_some()),
            "a refused install spends nothing"
        );

        // The other steps read the recovered state and answer as they always do.
        assert!(
            arm_restart(&state).is_ok(),
            "arming a ready download still works"
        );
        let err = refusal(checked_update(&state).map(|_| ()));
        assert!(err.contains("check for updates first"), "{err}");
    }

    /// [#579 x #576] What a quit gets on its way out, the restart gets too. The
    /// two halves are wiring, so they are pinned on the source: the install body
    /// runs on the blocking pool (off the main thread, so `AppHandle::restart`
    /// requests the exit and `RunEvent::ExitRequested` runs, as on a quit), and the
    /// Windows installer hand-over, which exits without that event, runs the same
    /// release first. Both go through the one function `RunEvent::ExitRequested`
    /// calls.
    #[test]
    fn the_update_restart_exits_through_the_quit_path() {
        fn body<'a>(src: &'a str, signature: &str) -> &'a str {
            let start = src.find(signature).expect("the function is there");
            let rest = &src[start..];
            &rest[..rest.find("\n}\n").expect("the function ends")]
        }
        let src = include_str!("app_update.rs");
        let install = body(src, "pub async fn install_app_update_and_restart<");
        assert!(
            install.contains("pool::run_stateless(move || install_and_restart(&app))"),
            "{install}"
        );
        let restart = body(src, "fn install_and_restart<");
        assert!(restart.contains("app.restart()"), "{restart}");
        let check = body(src, "async fn check<");
        assert!(
            check.contains(".on_before_exit(move || {")
                && check.contains("crate::release_held_lock_on_exit(&exiting);")
                && check.contains("exiting.cleanup_before_exit();"),
            "{check}"
        );
        let lib = include_str!("../lib.rs");
        let on_exit = body(lib, "pub(crate) fn release_held_lock_on_exit<");
        assert!(
            on_exit.contains("speleodb_release_held_lock_bounded(EXIT_LOCK_RELEASE_TIMEOUT)"),
            "{on_exit}"
        );
        // [MAC-CMDQ-D1] The run loop hands every event to `on_run_event`, whose
        // exit stages (`ExitRequested`, and `Exit` as the backstop) call the same
        // release; `exit_release_tests` drives that through the mock run loop.
        assert!(
            lib.contains("app.run(|app_handle, event| on_run_event(app_handle, &event));"),
            "the run loop goes through on_run_event"
        );
        let on_run = body(lib, "pub(crate) fn on_run_event<");
        assert!(
            on_run.contains(
                "if exit_releases_held_lock(event) {\n        release_held_lock_on_exit(app_handle);"
            ),
            "the quit path calls the same release: {on_run}"
        );
        let stages = body(lib, "fn exit_releases_held_lock(");
        assert!(
            stages.contains("tauri::RunEvent::ExitRequested { .. }"),
            "{stages}"
        );
    }
}
