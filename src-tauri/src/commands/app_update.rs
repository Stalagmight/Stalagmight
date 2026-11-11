//! In-app updater IPC surface (#579).
//!
//! Rust drives the whole cycle rather than the plugin's JS guest API, for one
//! concrete reason: the guest `check()` cannot override the endpoint per call,
//! and the endpoint is exactly what changes between the stable and beta
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
//! A download nobody restarts into is installed as the app exits instead
//! ([`install_downloaded_update_on_exit`], #631), so an ordinary quit is
//! enough to update and nothing ever forces a restart. Not on Windows, nor
//! where it would need an admin prompt - see [`installs_on_quit`].
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

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::task::Poll;
use std::time::Duration;

use semver::{Prerelease, Version};
use serde::Serialize;
use tauri::utils::config::BundleType;
use tauri::{AppHandle, Emitter, Manager, State, Url};
use tauri_plugin_updater::{Update, UpdaterExt};
use ts_rs::TS;

use crate::config::UpdateTrack;
use crate::core::AppState;

/// Endpoint override for the update manifest.
///
/// Honoured unconditionally in a DEBUG build - that is the manual test recipe
/// in docs/releasing.md, where a local `python3 -m http.server` stands in for
/// GitHub - and REQUIRED there: without it a debug build does not update at
/// all ([`dev_build_refusal`], #644). In a release build it is restricted to
/// https `github.com` URLs ([`is_trusted_manifest_url`]): the manifest decides
/// which binary an installed app downloads, so an attacker who can set one
/// environment variable must not be able to redirect it. (The plugin's own
/// `validate_endpoints` already refuses non-https in release; this narrows the
/// host as well.)
///
/// Read at CALL time, so exporting it and re-checking is enough - no restart.
pub const UPDATE_URL_ENV: &str = "STALAGMIGHT_UPDATE_URL";

/// Per-track manifests, both rolling assets of the `updater` release.
const STABLE_ENDPOINT: &str =
    "https://github.com/Stalagmight/Stalagmight/releases/download/updater/latest-stable.json";
const BETA_ENDPOINT: &str =
    "https://github.com/Stalagmight/Stalagmight/releases/download/updater/latest-beta.json";

/// The pre-release part of THIS build's version, without its leading hyphen
/// (#650): `rc.2` for a release candidate, unset or empty for a final release.
/// Baked in by CI at compile time. The package version - and with it every
/// artifact - stays a plain `X.Y.Z`, so this is the only place a candidate
/// learns that it is one.
const BAKED_PRERELEASE: Option<&str> = option_env!("STALAGMIGHT_PRERELEASE");

/// The only host a release build will fetch a manifest from.
const TRUSTED_MANIFEST_HOST: &str = "github.com";

/// Release-notes landing page for an offered version. Tags are `v<version>`.
const RELEASE_TAG_URL_PREFIX: &str = "https://github.com/Stalagmight/Stalagmight/releases/tag/v";

/// Progress event the frontend listens on during [`download_app_update`].
pub const DOWNLOAD_PROGRESS_EVENT: &str = "app-update:download-progress";

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
    /// without a second round trip. The EFFECTIVE version
    /// ([`effective_version`]): `0.5.2-rc.2` for a candidate, the same version
    /// the offer was compared against.
    pub current_version: String,
    /// Whether a downloaded update is installed when the app quits
    /// ([`installs_on_quit`]), so the banner can promise it - or not.
    pub installs_on_quit: bool,
}

/// Whether [`install_downloaded_update_on_exit`] can run on this platform.
///
/// Not on Windows: the plugin's NSIS install passes `/R` in the passive mode
/// this app uses (`WindowsUpdateInstallMode::nsis_args`, tauri-plugin-updater
/// 2.9.0), so the installer reopens the app when it finishes. Installing on
/// quit there would bring back the window the user just closed, which is a
/// restart they never asked for. Windows keeps "Restart to update".
const PLATFORM_INSTALLS_ON_QUIT: bool = !cfg!(windows);

/// Whether an update downloaded now would be installed when the app quits.
///
/// On macOS this also asks whether the folder holding the app is writable.
/// When it is not (a non-admin account, an app copied in by another user), the
/// plugin falls back to an administrator password prompt, and raising that
/// from an app whose window has already closed, on every quit, is not
/// acceptable. Such installs keep "Restart to update", where the prompt follows
/// a click.
pub fn installs_on_quit() -> bool {
    PLATFORM_INSTALLS_ON_QUIT && install_dir_is_writable()
}

/// Can the install replace the app without elevation? Probed by creating and
/// removing a directory next to the bundle, since permission bits alone do not
/// answer it (ACLs, group membership). Only macOS has an elevation fallback;
/// elsewhere a failed write is just a logged failure.
fn install_dir_is_writable() -> bool {
    if !cfg!(target_os = "macos") {
        return true;
    }
    let Ok(exe) = tauri::utils::platform::current_exe() else {
        return false;
    };
    let Ok(bundle) = tauri_plugin_updater::extract_path_from_executable(&exe) else {
        return false;
    };
    let Some(dir) = bundle.parent() else {
        return false;
    };
    let probe = dir.join(format!(".stalagmight-update-probe-{}", std::process::id()));
    match std::fs::create_dir(&probe) {
        Ok(()) => {
            if let Err(e) = std::fs::remove_dir(&probe) {
                tracing::warn!(error = %e, path = %probe.display(), "Could not remove the update write probe");
            }
            true
        }
        Err(_) => false,
    }
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

/// Payload of [`DOWNLOAD_PROGRESS_EVENT`].
///
/// `total` is `None` when the server sent no `Content-Length`; the frontend
/// then has bytes-so-far but no percentage, which is a real state and not an
/// error.
#[derive(Debug, Clone, Copy, Serialize, TS)]
pub struct AppUpdateProgress {
    /// `number` in TS, not ts-rs's default `bigint` for `u64`: serde_json puts a
    /// plain JSON number on the wire, so a `bigint` declaration would typecheck
    /// against arithmetic that throws at runtime. Bounded by
    /// [`MAX_UPDATE_BYTES`], nowhere near 2^53.
    #[ts(type = "number")]
    pub downloaded: u64,
    #[ts(type = "number | null")]
    pub total: Option<u64>,
}

/// Why [`install_app_update_and_restart`] did not restart.
///
/// Typed rather than a bare string because the two need different answers from
/// the frontend: a transfer in flight keeps the download (Restart works again once
/// the transfer has finished), while any other failure drops the banner back to
/// Download, since the bytes may be what failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppUpdateInstallError {
    /// A SpeleoDB transfer the user started is still running and the user did
    /// not choose "Quit now" over it. Nothing was installed; the download is kept.
    TransferInFlight { message: String },
    /// Not armed, nothing downloaded, or the install itself failed.
    Failed { message: String },
}

impl From<String> for AppUpdateInstallError {
    fn from(message: String) -> Self {
        Self::Failed { message }
    }
}

/// The message of [`AppUpdateInstallError::TransferInFlight`] (logs, and a
/// caller that shows the backend text as is; the app shows its own translation).
const RESTART_WAITS_FOR_TRANSFER: &str = "Stalagmight did not restart to update: a SpeleoDB \
     transfer is still running. Restart again once it has finished.";

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
    ///
    /// Also bumped by [`discard_download`] (#645), which keeps the parked
    /// `Update` but not the bytes. The pairing still holds: bytes are only ever
    /// installed under the generation they were stored for, and the `Update`
    /// parked at that generation is the one they were downloaded from.
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
#[derive(Default)]
pub struct AppUpdateState(Mutex<Pending>);

impl AppUpdateState {
    fn with<T>(&self, f: impl FnOnce(&mut Pending) -> T) -> Result<T, String> {
        let mut pending = self
            .0
            .lock()
            .map_err(|_| "The update state is unusable; restart the app".to_owned())?;
        Ok(f(&mut pending))
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
/// error, so the app never starts. The block is deliberately absent today (the
/// minisign keypair is still being provisioned), so registering unconditionally
/// would brick every build, local and released, until the key lands.
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

/// Why a DEBUG build must not update itself, or `None` (#644).
///
/// A debug build is a developer's `target/debug` binary, not an install: left
/// to the real manifest, `./scripts/dev.sh` on a branch older than the
/// published stable would download that release and, on window close, have the
/// plugin replace `target/debug/stalagmight` (Linux) or move `target/debug`
/// away (macOS). So a debug build checks, downloads and installs ONLY when
/// [`UPDATE_URL_ENV`] names a manifest - the local test recipe - and refuses
/// otherwise. "Names" means what [`endpoint_for_track`] would honour in a debug
/// build, so a blank value refuses here exactly as it falls back there.
///
/// Parameterised for the same reason as [`resolve_override`]: under
/// `cargo test` `cfg!(debug_assertions)` is always true, so the release branch
/// would otherwise be unreachable from a test.
fn dev_build_refusal(debug_build: bool, override_url: Option<&str>) -> Option<String> {
    if !debug_build || resolve_override(override_url, false).is_some() {
        return None;
    }
    Some(format!(
        "This is a development build, which does not update itself; set {UPDATE_URL_ENV} to a manifest URL to test the updater"
    ))
}

/// [`dev_build_refusal`] for the running build and the current environment.
/// Read at call time, like the endpoint override itself.
fn this_build_refusal() -> Option<String> {
    let raw = std::env::var(UPDATE_URL_ENV).ok();
    dev_build_refusal(cfg!(debug_assertions), raw.as_deref())
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
        UpdateTrack::Beta => BETA_ENDPOINT,
    }
    .to_owned()
}

/// The version this build compares offers against (#650): the package version
/// with the baked pre-release, so `0.5.2` built with `rc.2` is `0.5.2-rc.2`.
///
/// Unset or blank is a final release and yields the package version as it is.
/// `Err` says why a non-blank value was not used; [`running_version`] logs it
/// and falls back to the package version.
///
/// A leading hyphen is refused although semver would accept it: `-rc.2` parses
/// as the identifier `-rc` and makes `0.5.2--rc.2`, which sorts BELOW
/// `0.5.2-rc.1` - so the build would be offered, and install, itself on every
/// check.
fn effective_version(package: &Version, prerelease: Option<&str>) -> Result<Version, String> {
    let Some(pre) = prerelease.map(str::trim).filter(|p| !p.is_empty()) else {
        return Ok(package.clone());
    };
    // Checked here because `Prerelease::new` accepts it (`-` is an identifier
    // character in semver-1.0.27 parse.rs `identifier`); see above for why it
    // must not be.
    if pre.starts_with('-') {
        return Err(format!(
            "STALAGMIGHT_PRERELEASE {pre:?} must not start with a hyphen"
        ));
    }
    if !package.pre.is_empty() {
        return Err(format!(
            "the package version {package} already has a pre-release"
        ));
    }
    let pre = Prerelease::new(pre)
        .map_err(|e| format!("STALAGMIGHT_PRERELEASE {pre:?} is not a semver pre-release: {e}"))?;
    Ok(Version {
        pre,
        ..package.clone()
    })
}

/// [`effective_version`], falling back to the plain package version - with a
/// warning - when the baked pre-release is unusable. Comparing against the
/// plain version is what the plugin did before #650, so the fallback can miss
/// later candidates of the same release but can never downgrade.
fn running_version(package: &Version, prerelease: Option<&str>) -> Version {
    effective_version(package, prerelease).unwrap_or_else(|why| {
        tracing::warn!(
            %why,
            purpose = "compare update offers against this build's version",
            "Ignoring the baked pre-release; using the plain package version"
        );
        package.clone()
    })
}

/// Is `offered` an update for a build whose effective version is `running`?
///
/// Strictly greater by semver PRECEDENCE: `0.5.2-rc.1 < 0.5.2-rc.2 <
/// 0.5.2-rc.10 < 0.5.2`. An equal version is not an update, and neither is a
/// lower one - which is what makes switching from Beta back to Stable stay on
/// the candidate instead of downgrading. Precedence ignores build metadata,
/// which `Version`'s `Ord` does not.
///
/// Handed to the plugin as its `version_comparator`. Without one the plugin
/// compares against the PACKAGE version, a plain `X.Y.Z` even in a candidate,
/// so a candidate would never be offered the candidates after it, nor its own
/// final release.
fn is_newer(offered: &Version, running: &Version) -> bool {
    offered.cmp_precedence(running) == std::cmp::Ordering::Greater
}

/// Why a download must stop before the body ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DownloadAbort {
    /// More bytes than any bundle we ship could plausibly be.
    OverCap,
    /// More bytes than the server itself said it would send.
    OverDeclaredLength,
    /// A later check replaced the update this download is for, so its bytes
    /// could never be stored (see [`store_downloaded_bytes`]).
    Superseded,
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
            Self::Superseded => SUPERSEDED_MESSAGE.to_owned(),
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

const SUPERSEDED_MESSAGE: &str =
    "A newer update check replaced this download, or it was cancelled; check again";

/// Should the download for `generation` stop at this chunk?
///
/// Adds the one stateful reason to [`download_abort_reason`]: a newer check
/// (a track switch mid-download) has replaced the offer. Its bytes would be
/// refused at the end anyway, so stopping now is what frees the bandwidth and
/// the partial buffer instead of holding both until the body ends or
/// `DOWNLOAD_TIMEOUT` fires. A poisoned lock also stops it: nothing could be
/// stored under one.
fn chunk_abort_reason(
    update_state: &AppUpdateState,
    generation: u64,
    downloaded: u64,
    content_length: Option<u64>,
) -> Option<DownloadAbort> {
    let current = update_state
        .with(|pending| pending.generation == generation)
        .unwrap_or(false);
    if !current {
        return Some(DownloadAbort::Superseded);
    }
    download_abort_reason(downloaded, content_length)
}

/// The parked update and the generation it belongs to.
///
/// Cloned out rather than borrowed: the caller awaits a download afterwards,
/// and holding the lock across that await is exactly what `await_holding_lock`
/// forbids.
fn checked_update(update_state: &AppUpdateState) -> Result<(u64, Update), String> {
    update_state
        .with(|pending| pending.update.clone().map(|u| (pending.generation, u)))?
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
            return Err(SUPERSEDED_MESSAGE.to_owned());
        }
        pending.bytes = Some((generation, bytes));
        Ok(())
    })?
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
    })?
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
    update_state.with(take_installable)?
}

/// Put bytes back after a failed install - but only if they are still the
/// current offer.
///
/// A check that completed while the install was running has already replaced
/// the parked `Update`, so restoring unconditionally would re-create the
/// mispairing: old bytes filed under the new generation, ready for the next
/// arm to hand them to a different version's installer. They are dropped
/// instead, and the banner's re-check offers the new download.
fn restore_bytes_if_current(
    update_state: &AppUpdateState,
    generation: u64,
    bytes: Vec<u8>,
) -> Result<(), String> {
    update_state.with(|pending| {
        if pending.generation == generation {
            pending.bytes = Some((generation, bytes));
        }
    })
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
#[tauri::command]
pub async fn check_for_app_update(
    app: AppHandle,
    state: State<'_, AppState>,
    update_state: State<'_, AppUpdateState>,
) -> Result<AppUpdateCheck, String> {
    // The plugin is not registered without a pubkey (see `updater_configured`),
    // and `updater_builder()` PANICS on its unmanaged state - so this guard is
    // load-bearing, not defensive.
    if !updater_configured() {
        return Err(updater_not_configured());
    }
    // A development build never asks the real manifest (#644). Before any
    // state is touched, like the bundle gate below.
    if let Some(refusal) = this_build_refusal() {
        return Err(refusal);
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
    let running = running_version(&app.package_info().version, BAKED_PRERELEASE);
    let current_version = running.to_string();
    let track = state.get_config().general.update_track;
    let endpoint = endpoint_for_track(track);
    let url = Url::parse(&endpoint)
        .map_err(|e| format!("Update endpoint '{endpoint}' is not a valid URL: {e}"))?;

    let updater = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        // The plugin passes its own idea of the current version, the package
        // version; it is ignored in favour of the effective one (#650).
        .version_comparator(move |_package, release| is_newer(&release.version, &running))
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
        })?;
        return Ok(AppUpdateCheck {
            status: AppUpdateStatus::UpToDate,
            current_version,
            installs_on_quit: installs_on_quit(),
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
    })?;

    Ok(AppUpdateCheck {
        status: AppUpdateStatus::Available {
            notes_url: format!("{RELEASE_TAG_URL_PREFIX}{version}"),
            version,
        },
        current_version,
        installs_on_quit: installs_on_quit(),
    })
}

/// Download the update the last check found, and verify its signature.
///
/// Download ONLY - see the module docs for why this never installs. The bytes
/// are held in memory until [`install_app_update_and_restart`] asks for them;
/// the plugin has already checked them against the baked-in minisign public key
/// by the time this returns.
#[tauri::command]
pub async fn download_app_update(
    app: AppHandle,
    update_state: State<'_, AppUpdateState>,
) -> Result<(), String> {
    // Unreachable through the check, which refuses first; repeated so the
    // download does not depend on that ordering (#644).
    if let Some(refusal) = this_build_refusal() {
        return Err(refusal);
    }
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
                if let Some(why) =
                    chunk_abort_reason(&update_state, generation, downloaded, content_length)
                {
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
                    emit_progress(&app, downloaded, total);
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
            emit_progress(&app, downloaded, total);
            store_downloaded_bytes(&update_state, generation, bytes)?;
            Ok(())
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

fn emit_progress(app: &AppHandle, downloaded: u64, total: Option<u64>) {
    if let Err(e) = app.emit(
        DOWNLOAD_PROGRESS_EVENT,
        AppUpdateProgress { downloaded, total },
    ) {
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
#[tauri::command]
pub fn arm_app_update_restart(update_state: State<AppUpdateState>) -> Result<(), String> {
    arm_restart(update_state.inner())
}

/// Put the safety catch back.
///
/// Idempotent, and called on every path that abandons a restart - the user
/// cancels a guard dialog, the window close is refused, the install rejects -
/// so a restart the user walked away from cannot be completed by a later,
/// unrelated window close.
#[tauri::command]
pub fn unarm_app_update_restart(update_state: State<AppUpdateState>) -> Result<(), String> {
    update_state.with(|pending| pending.restart_armed = false)
}

/// Drop the download in hand, keeping the offer it was for (#645).
///
/// Bumping the generation is what reaches a download still IN FLIGHT: its
/// next chunk sees the change and stops ([`chunk_abort_reason`]), and if the
/// body ends first, [`store_downloaded_bytes`] refuses it. Finished bytes are
/// dropped, so [`install_downloaded_update_on_exit`] finds nothing to install.
/// The arm goes with them: it was consent for those bytes.
///
/// The parked `Update` stays, so a later [`download_app_update`] works without
/// a new check - the frontend goes back to offering Download.
fn discard_download(pending: &mut Pending) {
    pending.generation = pending.generation.wrapping_add(1);
    pending.bytes = None;
    pending.restart_armed = false;
}

/// Abandon an automatic download because "Update automatically" was turned
/// off (#645).
///
/// Unconditional on purpose: which download was automatic is the frontend's
/// knowledge (the backend sees one `download_app_update` either way), and the
/// frontend only calls this for a download a check started, never for one the
/// user asked for. Idempotent, and harmless from a stray `invoke`: the worst
/// it does is make the user download the update again.
#[tauri::command]
pub fn discard_app_update_download(update_state: State<AppUpdateState>) -> Result<(), String> {
    update_state.with(discard_download)
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
/// Releases a held SpeleoDB write lock FIRST, bounded by the same deadline the
/// ordinary quit uses. The Windows installer's own exit never raises
/// `RunEvent::ExitRequested`, which is where a quit releases (nor does an
/// `app.restart()` on the main thread; the one here runs off it and does, but
/// only after the install), so without this the lock was simply never handed
/// back before the install. The release keeps the
/// lock over un-pushed edits (the invariant in `speleodb_lock_release`), which is
/// the restart's documented "keep the lock and the local changes" answer; the
/// frontend has already saved and closed such a project with the lock kept.
///
/// Never cuts a SpeleoDB transfer the user started (lock#6): Tauri cannot prevent
/// a restart's exit, so this refuses with
/// [`AppUpdateInstallError::TransferInFlight`] while one is in flight, unless the
/// user chose "Quit now" over it (`exit_wait::arm_forced_exit`). See
/// [`install_unless_a_transfer_runs`] for where the check runs. The frontend's
/// quit gate normally waited for the transfer already; this does not rely on it.
///
/// Async, the work on a blocking thread: the bounded release can take up to
/// [`crate::EXIT_LOCK_RELEASE_TIMEOUT`], and a sync command runs on the main
/// thread, which would freeze the UI for it.
#[tauri::command]
pub async fn install_app_update_and_restart(app: AppHandle) -> Result<(), AppUpdateInstallError> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), AppUpdateInstallError> {
        let update_state = app.state::<AppUpdateState>();
        let taken = installable_update(&update_state)?;
        let exit_wait = app.try_state::<super::exit_wait::ExitWait>();
        install_unless_a_transfer_runs(
            &update_state,
            taken,
            || {
                exit_wait
                    .as_ref()
                    .is_some_and(|wait| wait.restart_blocked())
            },
            || {
                if let Some(state) = app.try_state::<AppState>() {
                    state.speleodb_release_held_lock_bounded(crate::EXIT_LOCK_RELEASE_TIMEOUT);
                }
            },
            |update: &Update, bytes: &[u8]| update.install(bytes).map_err(|e| e.to_string()),
        )?;
        // Off the main thread, this raises `ExitRequested` with the restart code,
        // which the exit hook lets through (`exit_wait`).
        app.restart();
    })
    .await
    .map_err(|e| AppUpdateInstallError::from(format!("Update install failed: {e}")))?
}

/// [`install_app_update_and_restart`] up to the restart, generic over the
/// installer so the order is testable without a live updater plugin.
///
/// `transfer_blocks` is asked twice: before the release, so a transfer in flight
/// is refused at once instead of the release queuing behind it, and again right
/// before the install, so one that started while the release ran is not cut
/// either. A refusal (or a failed install) puts the bytes back
/// ([`restore_bytes_if_current`]), so the restart can be retried without
/// downloading again.
fn install_unless_a_transfer_runs<U>(
    update_state: &AppUpdateState,
    (generation, update, bytes): (u64, U, Vec<u8>),
    transfer_blocks: impl Fn() -> bool,
    release: impl FnOnce(),
    install: impl FnOnce(&U, &[u8]) -> Result<(), String>,
) -> Result<(), AppUpdateInstallError> {
    // Filed under the generation they were DOWNLOADED for, not whatever is
    // current now - see `restore_bytes_if_current`.
    let refuse = |bytes: Vec<u8>| -> Result<(), AppUpdateInstallError> {
        tracing::info!(
            purpose = "never cut a SpeleoDB transfer the user started",
            "update restart refused: a SpeleoDB transfer is in flight"
        );
        restore_bytes_if_current(update_state, generation, bytes)?;
        Err(AppUpdateInstallError::TransferInFlight {
            message: RESTART_WAITS_FOR_TRANSFER.to_owned(),
        })
    };
    if transfer_blocks() {
        return refuse(bytes);
    }
    release();
    if transfer_blocks() {
        return refuse(bytes);
    }
    if let Err(e) = install(&update, &bytes) {
        restore_bytes_if_current(update_state, generation, bytes)?;
        return Err(format!("Update install failed: {e}").into());
    }
    Ok(())
}

/// Take a verified download for the exit-time install, if one is waiting for
/// the CURRENT check.
///
/// Needs no arm, unlike [`take_installable`]. The arm exists so a stray
/// `invoke` cannot restart the app out from under unsaved work; this runs only
/// from the `RunEvent::Exit` handler, after the process has already committed
/// to exiting, so there is no work left to protect and no webview path to it.
///
/// Taking the bytes (not peeking) also keeps the restart path from installing
/// twice: `install_app_update_and_restart` has already taken them by the time
/// its `app.restart()` reaches the exit event.
fn take_for_exit(pending: &mut Pending) -> Option<(Update, Vec<u8>)> {
    let (generation, _) = pending.bytes.as_ref()?;
    if *generation != pending.generation {
        return None;
    }
    let (_, bytes) = pending.bytes.take()?;
    pending.update.clone().map(|update| (update, bytes))
}

/// Install a downloaded update as the app exits, without relaunching (#631).
///
/// Called from the `RunEvent::Exit` handler in `lib.rs`, so it runs after every
/// exit guard has passed and the window is gone: the user quit normally, and
/// the next launch starts the new version. Nothing is restarted.
///
/// Best-effort: a failure is logged and the exit goes ahead. The next launch
/// re-checks and downloads the update again. Skipped wherever
/// [`installs_on_quit`] says no, which on macOS includes a location the user
/// cannot write - the plugin would otherwise raise an admin password prompt
/// after the window has gone.
pub fn install_downloaded_update_on_exit(app: &AppHandle) {
    if !installs_on_quit() {
        return;
    }
    // Defense in depth for #644: the check already refuses in a development
    // build, but this is the step that overwrites `target/debug`, so it does
    // not rely on that.
    if let Some(refusal) = this_build_refusal() {
        tracing::info!(reason = %refusal, "Not installing an update on quit");
        return;
    }
    let Some(update_state) = app.try_state::<AppUpdateState>() else {
        return;
    };
    let taken = match update_state.with(take_for_exit) {
        Ok(taken) => taken,
        Err(e) => {
            tracing::warn!(error = %e, "Update state unusable at exit; not installing");
            return;
        }
    };
    let Some((update, bytes)) = taken else {
        return;
    };
    let version = update.version.clone();
    match update.install(&bytes) {
        Ok(()) => tracing::info!(%version, "Installed the downloaded update on quit"),
        Err(e) => tracing::warn!(%version, error = %e, "Installing the update on quit failed"),
    }
}

#[cfg(test)]
#[expect(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_update_restart_releases_the_speleodb_lock_before_installing() {
        // `app.restart()` and the Windows installer's own exit both skip
        // `RunEvent::ExitRequested`, the only other place a quit releases. A release
        // placed after `update.install` never runs on Windows (the installer has
        // already exited the process), so ORDER is the contract, not presence.
        let src = include_str!("app_update.rs");
        let start = src
            .find("pub async fn install_app_update_and_restart(")
            .expect("the install command is present");
        let body = &src[start..];
        let body = &body[..body.find("\n}\n").expect("fn body terminates")];
        let release = body
            .find("speleodb_release_held_lock_bounded(")
            .expect("the install path releases a held lock");
        let install = body
            .find("update.install(bytes)")
            .expect("the install path installs");
        assert!(release < install, "the release must run BEFORE the install");
        assert!(
            body.contains("EXIT_LOCK_RELEASE_TIMEOUT"),
            "bounded by the same deadline the quit path uses"
        );
        // The command hands the two steps to `install_unless_a_transfer_runs` as
        // its `release` and `install`; that it calls them in this order is pinned
        // behaviourally below.
        assert!(
            body.contains("install_unless_a_transfer_runs("),
            "the install path runs through the transfer gate"
        );
    }

    /// What `install_unless_a_transfer_runs` did, in order.
    fn attempt(
        state: &AppUpdateState,
        blocked: &[bool],
        install: Result<(), String>,
    ) -> (Result<(), AppUpdateInstallError>, Vec<&'static str>) {
        let steps = std::cell::RefCell::new(Vec::new());
        let asks = std::cell::Cell::new(0);
        let result = install_unless_a_transfer_runs(
            state,
            (1, "synthetic update", vec![9u8; 4]),
            || {
                let i = asks.get();
                asks.set(i + 1);
                steps.borrow_mut().push("check");
                blocked.get(i).copied().unwrap_or(false)
            },
            || steps.borrow_mut().push("release"),
            |update: &&str, bytes: &[u8]| {
                assert_eq!((*update, bytes), ("synthetic update", &[9u8; 4][..]));
                steps.borrow_mut().push("install");
                install
            },
        );
        (result, steps.into_inner())
    }

    fn kept_bytes(state: &AppUpdateState) -> Option<(u64, Vec<u8>)> {
        state
            .with(|pending| pending.bytes.clone())
            .expect("lockable")
    }

    /// lock#6: Tauri ignores `prevent_exit` for a restart, so the install command
    /// itself refuses while a SpeleoDB transfer the user started is in flight: no
    /// release queued behind it, no install, and the download is kept for a
    /// Restart once the transfer has finished.
    #[test]
    fn a_transfer_in_flight_refuses_the_restart_and_keeps_the_download() {
        let state = AppUpdateState::default();
        state
            .with(|pending| pending.generation = 1)
            .expect("lockable");
        let (result, steps) = attempt(&state, &[true], Ok(()));
        assert!(
            matches!(result, Err(AppUpdateInstallError::TransferInFlight { .. })),
            "{result:?}"
        );
        assert_eq!(steps, ["check"], "nothing released, nothing installed");
        assert_eq!(kept_bytes(&state), Some((1, vec![9u8; 4])));
    }

    /// A transfer that starts while the release runs is not cut either: the gate
    /// is asked again right before the install.
    #[test]
    fn a_transfer_started_during_the_release_still_refuses_the_install() {
        let state = AppUpdateState::default();
        state
            .with(|pending| pending.generation = 1)
            .expect("lockable");
        let (result, steps) = attempt(&state, &[false, true], Ok(()));
        assert!(
            matches!(result, Err(AppUpdateInstallError::TransferInFlight { .. })),
            "{result:?}"
        );
        assert_eq!(steps, ["check", "release", "check"]);
        assert_eq!(kept_bytes(&state), Some((1, vec![9u8; 4])));
    }

    /// With nothing in flight (or Quit now armed, which `restart_blocked` answers
    /// false for), the lock is released and only then the update installed.
    #[test]
    fn with_nothing_in_flight_the_release_runs_before_the_install() {
        let state = AppUpdateState::default();
        let (result, steps) = attempt(&state, &[], Ok(()));
        assert_eq!(result, Ok(()));
        assert_eq!(steps, ["check", "release", "check", "install"]);
    }

    /// A failed install is a plain failure (the banner re-offers Download), with
    /// the bytes put back while the offer stands.
    #[test]
    fn a_failed_install_is_not_reported_as_a_transfer_refusal() {
        let state = AppUpdateState::default();
        state
            .with(|pending| pending.generation = 1)
            .expect("lockable");
        let (result, _) = attempt(&state, &[], Err("synthetic disk full".to_owned()));
        assert!(
            matches!(
                &result,
                Err(AppUpdateInstallError::Failed { message }) if message.contains("synthetic disk full")
            ),
            "expected a plain failure, got {result:?}"
        );
        assert_eq!(kept_bytes(&state), Some((1, vec![9u8; 4])));
    }

    /// The endpoint override is process-global, so the tests that touch it take
    /// this first. Without it they race each other and read one another's value.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        let beta = endpoint_for_track(UpdateTrack::Beta);
        assert!(stable.ends_with("/latest-stable.json"), "{stable}");
        // Not the legacy `latest-weekly.json`, which the release pipeline only
        // mirrors for 0.5.0/0.5.1 installs (#650).
        assert!(beta.ends_with("/latest-beta.json"), "{beta}");
        assert_ne!(
            stable, beta,
            "the two tracks must not resolve to one manifest"
        );
        // Both defaults must satisfy the release allowlist, or a release build
        // would refuse its own endpoints if they were ever routed through it.
        assert!(is_trusted_manifest_url(&stable), "{stable}");
        assert!(is_trusted_manifest_url(&beta), "{beta}");
    }

    #[test]
    fn the_env_override_wins_over_both_tracks() {
        let _lock = env_lock();
        let _env = EnvVarGuard::set(Some("http://127.0.0.1:8000/latest.json"));

        for track in [UpdateTrack::Stable, UpdateTrack::Beta] {
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
        assert_eq!(endpoint_for_track(UpdateTrack::Beta), BETA_ENDPOINT);
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
            format!("{RELEASE_TAG_URL_PREFIX}0.5.2-rc.2"),
            "https://github.com/Stalagmight/Stalagmight/releases/tag/v0.5.2-rc.2"
        );
    }

    // ── Effective version and ordering (#650) ───────────────────────────────

    fn v(text: &str) -> Version {
        Version::parse(text).expect("a test version")
    }

    /// The release chain a Beta user walks, in order: each step is offered to
    /// the one before it, and never the other way round.
    #[test]
    fn candidates_sort_below_their_release_and_numerically_among_themselves() {
        let chain = ["0.5.1", "0.5.2-rc.1", "0.5.2-rc.2", "0.5.2-rc.10", "0.5.2"];
        for pair in chain.windows(2) {
            let [older, newer] = [pair[0], pair[1]];
            assert!(
                is_newer(&v(newer), &v(older)),
                "{newer} must be offered to {older}"
            );
            assert!(
                !is_newer(&v(older), &v(newer)),
                "{older} must not be offered to {newer}"
            );
        }
        // Numeric, not lexical: "10" < "2" as text.
        assert!(is_newer(&v("0.5.2-rc.10"), &v("0.5.2-rc.9")));
    }

    #[test]
    fn an_equal_version_is_not_offered() {
        for same in ["0.5.1", "0.5.2-rc.2", "0.5.2"] {
            assert!(!is_newer(&v(same), &v(same)), "{same}");
        }
        // Build metadata does not make an otherwise equal version newer.
        assert!(!is_newer(&v("0.5.2+abc"), &v("0.5.2")));
    }

    /// A candidate switched back to Stable reads a manifest that is BEHIND it.
    /// It stays where it is until the final release of its version arrives.
    #[test]
    fn switching_beta_to_stable_never_downgrades() {
        let candidate = running_version(&v("0.5.2"), Some("rc.2"));
        assert_eq!(candidate, v("0.5.2-rc.2"));
        assert!(
            !is_newer(&v("0.5.1"), &candidate),
            "the old stable is a downgrade"
        );
        assert!(
            is_newer(&v("0.5.2"), &candidate),
            "its own final release is an update"
        );
    }

    /// Without the comparator the plugin compares against the package version.
    /// Pinned so the reason for overriding it stays visible: a candidate built
    /// as `0.5.2` would see `0.5.2` as nothing new.
    #[test]
    fn the_effective_version_is_what_makes_the_final_release_an_update() {
        let package = v("0.5.2");
        assert!(!is_newer(&v("0.5.2"), &package));
        assert!(is_newer(
            &v("0.5.2"),
            &running_version(&package, Some("rc.2"))
        ));
    }

    #[test]
    fn the_baked_prerelease_makes_the_effective_version() {
        assert_eq!(
            effective_version(&v("0.5.2"), Some("rc.2")),
            Ok(v("0.5.2-rc.2"))
        );
        assert_eq!(
            effective_version(&v("0.5.2"), Some(" rc.10 ")),
            Ok(v("0.5.2-rc.10"))
        );
    }

    /// A final release bakes nothing (or an empty value): its version is the
    /// package version, and that is not an error worth a warning.
    #[test]
    fn a_final_build_is_its_plain_package_version() {
        for unset in [None, Some(""), Some("   ")] {
            assert_eq!(
                effective_version(&v("0.5.2"), unset),
                Ok(v("0.5.2")),
                "{unset:?}"
            );
        }
    }

    /// Anything that is not a plain pre-release falls back to the package
    /// version rather than producing a version that sorts somewhere surprising.
    #[test]
    fn an_invalid_prerelease_falls_back_to_the_plain_version() {
        for (invalid, why) in [
            ("rc.02", "a leading zero in a numeric identifier"),
            ("rc..2", "an empty identifier"),
            ("rc 2", "a space"),
            (
                "rc.2+abc",
                "build metadata, which is not part of a pre-release",
            ),
            ("-rc.2", "a leading hyphen"),
        ] {
            assert!(
                effective_version(&v("0.5.2"), Some(invalid)).is_err(),
                "{invalid:?} must be refused: {why}"
            );
            assert_eq!(
                running_version(&v("0.5.2"), Some(invalid)),
                v("0.5.2"),
                "{invalid:?}"
            );
        }
        // A package version that already carries a pre-release is not extended.
        assert!(effective_version(&v("0.5.2-alpha"), Some("rc.2")).is_err());
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
        assert!(DownloadAbort::Superseded
            .message()
            .contains("newer update check"));
    }

    /// A track switch mid-download replaces the offer; the old transfer stops
    /// at its next chunk instead of running to the end of the body.
    #[test]
    fn a_download_a_newer_check_replaced_stops_at_the_next_chunk() {
        let state = AppUpdateState::default();
        let started = state.with(|pending| pending.generation).expect("lock");
        assert_eq!(
            chunk_abort_reason(&state, started, 1_000, Some(10_000)),
            None
        );

        state
            .with(|pending| pending.generation = pending.generation.wrapping_add(1))
            .expect("lock");
        assert_eq!(
            chunk_abort_reason(&state, started, 2_000, Some(10_000)),
            Some(DownloadAbort::Superseded)
        );
    }

    /// The generation check is added to the size bounds, not swapped in for them.
    #[test]
    fn a_current_download_still_obeys_the_size_bounds() {
        let state = AppUpdateState::default();
        let current = state.with(|pending| pending.generation).expect("lock");
        assert_eq!(
            chunk_abort_reason(&state, current, MAX_UPDATE_BYTES + 1, None),
            Some(DownloadAbort::OverCap)
        );
        assert_eq!(
            chunk_abort_reason(&state, current, 1_001, Some(1_000)),
            Some(DownloadAbort::OverDeclaredLength)
        );
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

    /// [#644] A debug build left on the real manifest downloaded the published
    /// release and replaced `target/debug` on quit. Without an override it now
    /// refuses, by name, and says how to test the updater instead.
    #[test]
    fn a_debug_build_refuses_without_an_override() {
        for unset in [None, Some(""), Some("   ")] {
            let message = dev_build_refusal(true, unset).unwrap_or_default();
            assert!(
                message.contains("development build") && message.contains(UPDATE_URL_ENV),
                "{unset:?} must refuse naming the override, got {message:?}"
            );
        }
    }

    /// ...while the local recipe (a debug build pointed at a local manifest)
    /// keeps working, and a release build is never refused on these grounds.
    #[test]
    fn the_override_and_a_release_build_pass_the_dev_gate() {
        assert_eq!(
            dev_build_refusal(true, Some("http://127.0.0.1:8000/latest.json")),
            None
        );
        for raw in [None, Some(""), Some("http://127.0.0.1:8000/latest.json")] {
            assert_eq!(dev_build_refusal(false, raw), None, "{raw:?}");
        }
    }

    /// The gate reads the same variable the endpoint does, at call time.
    #[test]
    fn the_dev_gate_follows_the_environment() {
        let _lock = env_lock();
        let _env = EnvVarGuard::set(None);
        // Every test binary is a debug build.
        assert!(this_build_refusal().is_some());
        std::env::set_var(UPDATE_URL_ENV, "http://127.0.0.1:8000/latest.json");
        assert_eq!(this_build_refusal(), None);
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
        assert!(state
            .with(|pending| pending.bytes.is_some())
            .expect("lockable"));
    }

    /// The arm is one-shot: consumed by the install attempt it authorised, so a
    /// second attempt has to be re-armed through the guarded path.
    #[test]
    fn the_arm_is_spent_by_one_install_attempt() {
        let state = AppUpdateState::default();
        seed_download(&state, 1);
        arm_restart(&state).expect("arming a ready download should work");

        assert!(state
            .with(|pending| pending.restart_armed)
            .expect("lockable"));

        // An `Update` cannot be constructed outside a live plugin, so the
        // attempt gets all the way to the LAST check and stops there - which is
        // itself the proof that the arm and the bytes were accepted and taken.
        let attempt = refusal(installable_update(&state));
        assert!(attempt.contains("No update to install"), "{attempt}");
        assert!(!state
            .with(|pending| pending.restart_armed)
            .expect("lockable"));
        assert!(state
            .with(|pending| pending.bytes.is_none())
            .expect("lockable"));

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
        state
            .with(|pending| pending.restart_armed = false)
            .expect("lockable");

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
        let generation = state
            .with(|pending| {
                pending.generation = 7;
                pending.generation
            })
            .expect("lockable");

        // A second check lands mid-download.
        state
            .with(|pending| {
                pending.generation += 1;
                pending.bytes = None;
            })
            .expect("lockable");

        let err = refusal(store_downloaded_bytes(&state, generation, vec![0u8; 8]));
        assert!(err.contains("newer update check"), "{err}");
        assert!(state
            .with(|pending| pending.bytes.is_none())
            .expect("lockable"));
    }

    /// ...and from the install's side, for any way a stale pair could still be
    /// sitting in the state.
    #[test]
    fn install_refuses_a_mismatched_pair() {
        let state = AppUpdateState::default();
        seed_download(&state, 3);
        arm_restart(&state).expect("arming a ready download should work");
        // The check that invalidates it lands after the arm.
        state
            .with(|pending| pending.generation = 4)
            .expect("lockable");

        let err = refusal(installable_update(&state));
        assert!(err.contains("no longer matches"), "{err}");
    }

    /// A failed install hands the bytes back so "Restart to update" can be
    /// retried without re-downloading - but only while they are still the offer
    /// on the table.
    #[test]
    fn a_failed_install_restores_bytes_while_the_offer_stands() {
        let state = AppUpdateState::default();
        state
            .with(|pending| pending.generation = 5)
            .expect("lockable");

        restore_bytes_if_current(&state, 5, vec![7u8; 3]).expect("lockable");

        assert_eq!(
            state
                .with(|pending| pending.bytes.clone())
                .expect("lockable"),
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
        state
            .with(|pending| pending.generation = 5)
            .expect("lockable");

        // The install was running under generation 5; a check landed and made
        // it 6 before the failure came back.
        state
            .with(|pending| pending.generation = 6)
            .expect("lockable");
        restore_bytes_if_current(&state, 5, vec![7u8; 3]).expect("lockable");

        assert!(
            state
                .with(|pending| pending.bytes.is_none())
                .expect("lockable"),
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
        let generation = state.with(|pending| pending.generation).expect("lockable");
        store_downloaded_bytes(&state, generation, vec![1u8; 4]).expect("should store");
        assert_eq!(
            state
                .with(|pending| pending.bytes.clone())
                .expect("lockable"),
            Some((generation, vec![1u8; 4]))
        );
    }

    // ── Install on quit (#631) ───────────────────────────────────────────────

    /// Bytes from a check a later one superseded must not be installed: they
    /// would be applied by the newer `Update`, a version nobody downloaded.
    /// They are left in place, which is what makes the refusal observable here.
    #[test]
    fn the_exit_install_skips_bytes_a_newer_check_superseded() {
        let state = AppUpdateState::default();
        seed_download(&state, 1);
        state
            .with(|pending| pending.generation = 2)
            .expect("lockable");

        assert!(state.with(take_for_exit).expect("lockable").is_none());
        assert!(state
            .with(|pending| pending.bytes.is_some())
            .expect("lockable"));
    }

    /// The exit install needs no arm, but it does take the bytes, so the
    /// restart path (which takes them first) can never install a second time
    /// from the exit event its own `app.restart()` raises.
    #[test]
    fn the_exit_install_takes_a_current_download_without_an_arm() {
        let state = AppUpdateState::default();
        seed_download(&state, 1);
        assert!(!state
            .with(|pending| pending.restart_armed)
            .expect("lockable"));

        // An `Update` cannot be constructed outside a live plugin, so the take
        // stops at the last step; the bytes being gone proves it got there.
        assert!(state.with(take_for_exit).expect("lockable").is_none());
        assert!(state
            .with(|pending| pending.bytes.is_none())
            .expect("lockable"));
    }

    // ── Discarding an automatic download (#645) ─────────────────────────────

    /// A finished download is dropped, so nothing installs at quit and the
    /// restart cannot be armed for it.
    #[test]
    fn a_discard_drops_finished_bytes_so_nothing_installs_at_quit() {
        let state = AppUpdateState::default();
        seed_download(&state, 1);
        arm_restart(&state).expect("arming a ready download should work");

        state.with(discard_download).expect("lockable");

        assert!(state
            .with(|pending| pending.bytes.is_none() && !pending.restart_armed)
            .expect("lockable"));
        assert!(state.with(take_for_exit).expect("lockable").is_none());
        let arm = refusal(arm_restart(&state));
        assert!(arm.contains("download it first"), "{arm}");
    }

    /// A download still in flight stops at its next chunk, and if its body
    /// ends first, its bytes are refused rather than stored.
    #[test]
    fn a_discard_stops_an_in_flight_download_and_refuses_its_bytes() {
        let state = AppUpdateState::default();
        let started = state.with(|pending| pending.generation).expect("lockable");

        state.with(discard_download).expect("lockable");

        assert_eq!(
            chunk_abort_reason(&state, started, 1_000, Some(10_000)),
            Some(DownloadAbort::Superseded)
        );
        let err = refusal(store_downloaded_bytes(&state, started, vec![0u8; 8]));
        assert!(err.contains("cancelled"), "{err}");
        assert!(state
            .with(|pending| pending.bytes.is_none())
            .expect("lockable"));
    }

    /// The offer survives: a download started AFTER the discard - the user
    /// pressing Download on the banner - is stored and can be armed, so the
    /// discard cannot leave the update unreachable until the next check.
    #[test]
    fn a_download_after_a_discard_is_stored_and_armable() {
        let state = AppUpdateState::default();
        seed_download(&state, 1);
        state.with(discard_download).expect("lockable");

        let restarted = state.with(|pending| pending.generation).expect("lockable");
        store_downloaded_bytes(&state, restarted, vec![2u8; 4]).expect("should store");
        arm_restart(&state).expect("a download after the discard should arm");
    }

    /// Put the state where a real check + download would leave it, without an
    /// `Update` (which needs a live plugin). Anything that needs the `Update`
    /// itself is covered by the command-level paths, not here.
    fn seed_download(state: &AppUpdateState, generation: u64) {
        state
            .with(|pending| {
                pending.generation = generation;
                pending.bytes = Some((generation, vec![0u8; 16]));
            })
            .expect("lockable");
    }
}
