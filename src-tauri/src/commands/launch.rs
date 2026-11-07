//! Launch-with-file plumbing (#424).
//!
//! Three OS paths can hand this app a file to open:
//!
//! - **Cold launch with argv** (Windows Explorer, a Linux `.desktop` handler):
//!   the path is an ordinary command-line argument.
//! - **Apple events** (macOS): Finder never passes argv, it sends an open-URLs
//!   event that Tauri surfaces as [`tauri::RunEvent::Opened`] - for both a cold
//!   launch and a double-click while the app is already running.
//! - **A second process** (Windows/Linux): `tauri-plugin-single-instance` kills
//!   the newcomer and forwards its argv to the running instance.
//!
//! All three converge here. Paths are pushed onto [`PendingLaunchFiles`] and the
//! frontend is nudged with [`LaunchFileQueued`]; the frontend drains the queue
//! via [`take_pending_launch_files`] once it is ready and routes each path
//! through its own unified open/import router. The queue - rather than an event
//! payload - is what makes a cold launch work at all: the file is known long
//! before the webview exists to receive an event.
//!
//! Everything arriving here is UNTRUSTED. The forwarding path in particular is
//! remote control: anything that can run a program as this user can hand the
//! running app a file. So each decode step below is a filter, not a conversion,
//! and the count is capped - the frontend then asks before it acts on anything
//! that would displace open work.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime, Url};

use crate::models::poison_policy::RecoveringMutex;

/// Payload-free nudge telling the frontend to drain the pending-launch queue
/// (`core::events::LaunchFileQueued`, #576 C13).
///
/// Deliberately carries no path: the queue is the single source of truth, so a
/// listener that misses this event (or is not mounted yet) still gets the file
/// on its next drain, and no path can be handled twice.
pub use crate::core::events::LaunchFileQueued;

/// Extensions the OS may hand us, lowercase and without the leading dot.
///
/// Must stay set-equal to `bundle.fileAssociations[].ext` in `tauri.conf.json`;
/// `associations_match_launchable_extensions` below pins that, so the OS's idea
/// of what we handle and ours cannot drift.
///
/// Deliberately NARROWER than what `routeOpenedFile` can import: our own
/// project format and Ariane's, nothing else. `role`/`rank` - the keys that say
/// "we are only an alternate viewer of this" - are macOS-only. On Windows an
/// association is a handler claim with no such hedge, and there `.dmp` is the
/// system minidump extension while `.dat` is about as generic as an extension
/// gets; claiming those would take handlers away from software that owns them.
/// Those formats stay importable through the picker and drag-drop, which is
/// where the user asked for them by name.
///
/// The `.tml`/`.tmlu` entry in `tauri.conf.json` deliberately has NO
/// `exportedType`: that maps to `UTExportedTypeDeclarations`, which CLAIMS
/// ownership of a Uniform Type Identifier, and Ariane already owns these -
/// its installed `Info.plist` exports `com.arianesline.ariane.tml` /
/// `.tmlu` as `LSHandlerRank: Owner`, `LSIsAppleDefaultForType: true`.
/// Exporting a competing UTI of our own would not make Finder treat the two
/// apps as agreeing about the same type, and referencing Ariane's identifier
/// in OUR `exportedType` would be a straight ownership collision (two apps
/// each claiming to be the source of truth for the identical UTI). Instead
/// the entry sets `contentTypes` (`LSItemContentTypes`) to Ariane's own
/// identifiers - the macOS construct for "I can open a type I do not own":
/// it references an EXISTING UTI without exporting one, cooperating with
/// Ariane's `Owner`/default-app declaration rather than contesting it. This
/// was verified against the real, installed `Ariane.app` on the maintainer's
/// machine, not guessed.
pub const LAUNCHABLE_EXTENSIONS: &[&str] = &["stalx", "tml", "tmlu"];

/// Most files we will accept from one launch or forward.
///
/// A "select 20000 files, press Enter" accident - or a deliberate argv flood
/// from a second process - must not become 20000 queued open requests. The
/// surplus is dropped with a warning rather than truncated silently.
pub const MAX_LAUNCH_PATHS: usize = 16;

fn has_launchable_extension(path: &str) -> bool {
    let Some(ext) = path.rsplit_once('.').map(|(_, ext)| ext) else {
        return false;
    };
    // A dot in a directory component is not an extension.
    if ext.contains('/') || ext.contains('\\') {
        return false;
    }
    let ext = ext.to_ascii_lowercase();
    LAUNCHABLE_EXTENSIONS.contains(&ext.as_str())
}

/// Resolve to an absolute path, or reject.
///
/// A forwarded argv is relative to the SENDING process's working directory, not
/// ours, so `base` is that directory (the `cwd` the single-instance plugin
/// forwards, or our own on a cold launch). With no usable base a relative path
/// is dropped rather than guessed at: resolving it against the wrong directory
/// opens the wrong file, or nothing.
fn absolutize(path: &str, base: Option<&Path>) -> Option<String> {
    let candidate = Path::new(path);
    let resolved: PathBuf = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        // `join` with an absolute argument replaces the base, so this is right
        // for both shapes; the check below catches what is left (e.g. a bare
        // `\foo` on Windows, which has a root but no drive prefix).
        base?.join(candidate)
    };
    if !resolved.is_absolute() {
        return None;
    }
    resolved.to_str().map(str::to_owned)
}

/// Decode one `file://` URL into a local path.
///
/// Rejects a non-empty host. `Url::to_file_path` turns `file://evil.example/x`
/// into the UNC path `\\evil.example\x` on Windows, so opening it makes an
/// outbound SMB connection - leaking an NTLM handshake to an attacker-chosen
/// host from nothing but a crafted argument. `localhost` is the one spelling of
/// "this machine" the file scheme defines.
fn file_url_to_path(url: &Url) -> Option<String> {
    if url.scheme() != "file" {
        return None;
    }
    match url.host_str() {
        None | Some("" | "localhost") => {}
        Some(_) => return None,
    }
    let path = url.to_file_path().ok()?;
    path.to_str().map(str::to_owned)
}

/// Decode one command-line argument into a launchable absolute path.
///
/// Returns `None` for flags, unsupported extensions, `file://` URLs that don't
/// resolve to a local path, and anything still relative after `base`. Only an
/// argument that already looks like a URL is parsed as one: a Windows path
/// (`C:\dive\cave.stalx`) parses as a URL with scheme `c`, so blanket URL
/// parsing would mangle it.
pub fn launch_path_from_arg(arg: &str, base: Option<&Path>) -> Option<String> {
    if arg.starts_with('-') {
        return None;
    }
    let path = if arg.starts_with("file://") {
        file_url_to_path(&Url::parse(arg).ok()?)?
    } else {
        arg.to_owned()
    };
    if !has_launchable_extension(&path) {
        return None;
    }
    absolutize(&path, base)
}

/// Decode one opened URL (macOS Apple event) into a launchable local path.
pub fn launch_path_from_url(url: &Url) -> Option<String> {
    let path = file_url_to_path(url)?;
    if !has_launchable_extension(&path) {
        return None;
    }
    absolutize(&path, None)
}

/// Collect at most [`MAX_LAUNCH_PATHS`] launchable paths, first-seen order, no
/// duplicates. The shared tail of both argv and URL decoding.
fn collect_capped<I, F>(items: I, decode: F) -> Vec<String>
where
    I: IntoIterator,
    F: Fn(I::Item) -> Option<String>,
{
    collect_capped_with_seen(items, decode).0
}

/// [`collect_capped`]'s actual work, also returning the dedup set - the only
/// way to observe from a test that it really does stay capped (see
/// `dedup_set_does_not_grow_past_the_cap`) rather than retaining one string
/// per surplus path regardless of `MAX_LAUNCH_PATHS` (#445 review).
fn collect_capped_with_seen<I, F>(items: I, decode: F) -> (Vec<String>, HashSet<String>)
where
    I: IntoIterator,
    F: Fn(I::Item) -> Option<String>,
{
    let mut seen: HashSet<String> = HashSet::new();
    let mut paths: Vec<String> = Vec::new();
    let mut dropped = 0usize;
    for item in items {
        let Some(path) = decode(item) else {
            continue;
        };
        if paths.len() >= MAX_LAUNCH_PATHS {
            // The cap is already full - check `seen` WITHOUT inserting into it.
            // Inserting one string per surplus path here (as before) retained
            // memory proportional to the flood regardless of the cap - exactly
            // the case MAX_LAUNCH_PATHS exists to bound (#445 review). A
            // genuine duplicate of something already collected still isn't
            // counted as dropped launchable content, matching the accounting
            // below.
            if !seen.contains(&path) {
                dropped += 1;
            }
            continue;
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        paths.push(path);
    }
    if dropped > 0 {
        tracing::warn!(
            dropped,
            cap = MAX_LAUNCH_PATHS,
            purpose = "accept files the OS asked us to open",
            "Too many files in one launch; ignoring the surplus"
        );
    }
    (paths, seen)
}

/// Collect the launchable paths out of a full argv, skipping `argv[0]`.
///
/// Takes `OsString`s, not `String`s: `std::env::args()` PANICS on a non-Unicode
/// argument, so a latin-1 filename on Linux would crash the app at launch - a
/// one-line local denial of service. Such an argument is dropped instead; it
/// could not survive the JSON IPC hop to the frontend in any case.
pub fn launch_paths_from_args<I>(args: I, base: Option<&Path>) -> Vec<String>
where
    I: IntoIterator<Item = OsString>,
{
    collect_capped(args.into_iter().skip(1), |arg| {
        launch_path_from_arg(arg.to_str()?, base)
    })
}

/// Same, for the URL list of a macOS open event.
pub fn launch_paths_from_urls(urls: &[Url]) -> Vec<String> {
    collect_capped(urls, launch_path_from_url)
}

/// Files the OS asked us to open that the frontend has not consumed yet.
///
/// Managed Tauri state. Holding them here (instead of emitting a path with the
/// event) is what lets a cold launch work: the OS tells us the file before the
/// webview exists, and the frontend picks it up when its stores are ready.
#[derive(Debug)]
pub struct PendingLaunchFiles(RecoveringMutex<Vec<String>>);

impl Default for PendingLaunchFiles {
    fn default() -> Self {
        Self(RecoveringMutex::new("pending_launch_files", Vec::new()))
    }
}

impl PendingLaunchFiles {
    /// Queue paths not already queued; returns how many were newly added.
    ///
    /// Capped at [`MAX_LAUNCH_PATHS`] across the whole queue, not per call: a
    /// frontend that is slow (or waiting on the user's confirm) must not let
    /// repeated forwards grow it without bound. A poisoned lock is recovered
    /// like every lock (#576 A11), never a panic: a crashed event loop is not
    /// recoverable.
    pub fn push_all(&self, paths: Vec<String>) -> usize {
        let mut queue = self.0.lock();
        let before = queue.len();
        let mut dropped = 0usize;
        for path in paths {
            if queue.contains(&path) {
                continue;
            }
            if queue.len() >= MAX_LAUNCH_PATHS {
                dropped += 1;
                continue;
            }
            queue.push(path);
        }
        if dropped > 0 {
            tracing::warn!(
                dropped,
                cap = MAX_LAUNCH_PATHS,
                purpose = "queue files the OS asked us to open",
                "Pending-launch queue is full; ignoring the surplus"
            );
        }
        queue.len() - before
    }

    /// Remove and return everything queued.
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock())
    }
}

/// Queue `paths` and nudge the frontend, unless nothing new was queued.
pub fn queue_and_notify<R: Runtime>(app: &AppHandle<R>, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    let Some(pending) = app.try_state::<PendingLaunchFiles>() else {
        tracing::warn!(
            purpose = "queue an OS-opened file",
            "PendingLaunchFiles state is not managed; dropping the request"
        );
        return;
    };
    if pending.push_all(paths) == 0 {
        return;
    }
    if let Err(e) = super::event_sink::emit_event::<LaunchFileQueued, _>(app, ()) {
        tracing::warn!(
            error = %e,
            purpose = "notify the frontend of an OS-opened file",
            "Emit failed; the file stays queued for the next drain"
        );
    }
}

/// Bring the main window forward.
///
/// Called when a SECOND launch is folded into this one. Without it "Open with"
/// on a backgrounded app looks like nothing happened - the prompt (and then the
/// file) lands in a window the user cannot see. Raising the window is also what
/// makes the forwarded open observable rather than silent, which is half of why
/// it isn't unconsented remote control; the frontend's confirm is the other.
///
/// `unminimize` runs before `set_focus`: on Windows and Linux, `set_focus` alone
/// does not restore a MINIMIZED window - it would leave the confirm this
/// function exists to make visible waiting in a window the user still cannot
/// see. Best-effort like the focus call below it: a window that was never
/// minimized (or a backend that no-ops the call) is not an error.
pub fn focus_main_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let Err(e) = window.unminimize() {
        tracing::warn!(
            error = %e,
            purpose = "raise the window for a forwarded file open",
            "Could not unminimize the main window"
        );
    }
    if let Err(e) = window.set_focus() {
        tracing::warn!(
            error = %e,
            purpose = "raise the window for a forwarded file open",
            "Could not focus the main window"
        );
    }
}

/// Whether this process registers `tauri-plugin-single-instance`.
///
/// Always, except in a DEBUG build launched by the agent harness
/// (`STALAGMIGHT_TEST_HOOKS=1`, which `e2e/agent/session.ts` sets for every
/// session it starts and a normal launch never does). The plugin claims one
/// name on the D-Bus session bus per app identifier, so with it registered two
/// harness sessions on one machine kill each other: the second app is
/// terminated and its argv forwarded to the first, and a parallel
/// `agent:fuzz-matrix` lost 9 of 12 cells that way (#576 HFI-7). The same
/// mechanism forwarded a harness launch into the user's own open app.
///
/// Two layers, the same rule `commands/debug.rs` applies to its hooks: the
/// environment variable alone is not enough, because `debug_build` is
/// `cfg!(debug_assertions)` at the call site, so a release binary ignores the
/// variable and keeps the plugin whatever its environment says. (The release
/// HARNESS build drops the plugin at compile time instead, through `--features
/// e2e`.) Pure so both answers are testable from a debug test build.
pub fn single_instance_enabled(debug_build: bool, test_hooks: Option<&OsStr>) -> bool {
    !(debug_build && test_hooks == Some(OsStr::new("1")))
}

/// Drain the files the OS asked us to open since the last call.
#[tauri::command]
pub async fn take_pending_launch_files<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<String>, crate::core::CommandError> {
    super::pool::run_stateless(move || Ok(app.state::<PendingLaunchFiles>().take())).await
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// An absolute directory, spelled for the running platform, so the
    /// absolute/relative cases mean the same thing on Windows and Unix.
    fn base() -> &'static Path {
        Path::new(if cfg!(windows) { r"C:\dives" } else { "/dives" })
    }

    /// An absolute path under [`base`].
    fn abs(name: &str) -> String {
        base().join(name).to_str().unwrap().to_owned()
    }

    #[test]
    fn accepts_every_associated_extension() {
        for ext in LAUNCHABLE_EXTENSIONS {
            let arg = abs(&format!("cave.{ext}"));
            assert_eq!(
                launch_path_from_arg(&arg, None),
                Some(arg.clone()),
                "{ext} should be launchable"
            );
        }
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        let arg = abs("Cave.STALX");
        assert_eq!(launch_path_from_arg(&arg, None), Some(arg));
    }

    #[test]
    fn rejects_flags_and_unsupported_extensions() {
        assert_eq!(launch_path_from_arg("--webdriver", None), None);
        assert_eq!(launch_path_from_arg("-v", None), None);
        assert_eq!(launch_path_from_arg(&abs("notes.txt"), None), None);
        assert_eq!(launch_path_from_arg(&abs("cave"), None), None);
    }

    /// Importable, but deliberately NOT associated - so the OS never sends them
    /// and a forwarded argv naming one is refused. See LAUNCHABLE_EXTENSIONS.
    #[test]
    fn rejects_importable_formats_we_do_not_claim() {
        for ext in ["dmp", "mak", "dat", "wpj", "srv", "gpx"] {
            assert_eq!(
                launch_path_from_arg(&abs(&format!("cave.{ext}")), None),
                None,
                "{ext} is importable but must not be launchable"
            );
        }
    }

    #[test]
    fn keeps_windows_paths_intact() {
        // `C:\...` parses as a URL with scheme "c"; it must not be URL-decoded.
        // It is only ABSOLUTE on Windows, which is where the round trip holds.
        let arg = r"C:\dives\cave.stalx";
        let expected = if cfg!(windows) {
            Some(arg.to_owned())
        } else {
            None
        };
        assert_eq!(launch_path_from_arg(arg, None), expected);
    }

    #[test]
    fn decodes_file_urls() {
        let url = Url::from_file_path(base().join("deep cave.stalx")).unwrap();
        // Percent-encoding really is decoded, not passed through.
        assert!(url.as_str().contains("%20"));
        assert_eq!(launch_path_from_url(&url), Some(abs("deep cave.stalx")));
        assert_eq!(
            launch_path_from_arg(url.as_str(), None),
            Some(abs("deep cave.stalx"))
        );
    }

    #[test]
    fn rejects_non_file_urls() {
        let url = Url::parse("https://example.com/cave.stalx").unwrap();
        assert_eq!(launch_path_from_url(&url), None);
    }

    /// A remote host in a `file://` URL becomes a UNC path on Windows, so
    /// opening it makes an outbound SMB connection to a host the ARGUMENT
    /// chose, handing over an NTLM handshake. Rejected on every platform.
    #[test]
    fn rejects_file_urls_with_a_remote_host() {
        let evil = Url::parse("file://evil.example/share/cave.stalx").unwrap();
        assert_eq!(evil.host_str(), Some("evil.example"));
        assert_eq!(launch_path_from_url(&evil), None);
        assert_eq!(launch_path_from_arg(evil.as_str(), None), None);

        // The "this machine" spelling still works.
        let localhost = Url::parse("file://localhost/dives/cave.stalx").unwrap();
        assert!(launch_path_from_url(&localhost).is_some());
    }

    #[test]
    fn resolves_a_relative_arg_against_the_forwarded_cwd() {
        assert_eq!(
            launch_path_from_arg("cave.stalx", Some(base())),
            Some(abs("cave.stalx"))
        );
    }

    #[test]
    fn rejects_a_relative_arg_with_no_base() {
        // Resolving against OUR cwd would open a different file than the one the
        // sending process named.
        assert_eq!(launch_path_from_arg("cave.stalx", None), None);
        assert_eq!(launch_path_from_arg("./cave.stalx", None), None);
    }

    #[test]
    fn skips_argv_zero() {
        // argv[0] can itself end in a launchable extension; it is the
        // executable, never a file to open.
        let args: Vec<OsString> = vec![
            abs("app.stalx").into(),
            abs("one.tml").into(),
            "--flag".into(),
            abs("one.tml").into(),
            abs("two.tmlu").into(),
        ];
        assert_eq!(
            launch_paths_from_args(args, None),
            vec![abs("one.tml"), abs("two.tmlu")]
        );
    }

    /// `std::env::args()` panics on this. A file named in latin-1 on Linux must
    /// not be able to crash the app at launch.
    #[cfg(unix)]
    #[test]
    fn a_non_unicode_arg_is_dropped_not_panicked_on() {
        use std::os::unix::ffi::OsStringExt;
        let args: Vec<OsString> = vec![
            OsString::from("/apps/stalagmight"),
            OsString::from_vec(vec![0xff, 0xfe]),
            OsString::from_vec(b"/dives/caf\xe9.stalx".to_vec()),
            OsString::from(abs("ok.stalx")),
        ];
        assert_eq!(launch_paths_from_args(args, None), vec![abs("ok.stalx")]);
    }

    #[test]
    fn urls_are_filtered_and_deduped() {
        let urls = vec![
            Url::from_file_path(base().join("one.stalx")).unwrap(),
            Url::from_file_path(base().join("one.stalx")).unwrap(),
            Url::from_file_path(base().join("notes.txt")).unwrap(),
        ];
        assert_eq!(launch_paths_from_urls(&urls), vec![abs("one.stalx")]);
    }

    #[test]
    fn an_argv_flood_is_capped() {
        let mut args: Vec<OsString> = vec![OsString::from("/apps/stalagmight")];
        args.extend((0..10_000).map(|i| OsString::from(abs(&format!("cave{i}.stalx")))));
        let paths = launch_paths_from_args(args, None);
        assert_eq!(paths.len(), MAX_LAUNCH_PATHS);
        assert_eq!(paths[0], abs("cave0.stalx"));
    }

    /// #445 review: the cap used to be checked AFTER inserting into the dedup
    /// set, so a flood of unique surplus paths retained one string per surplus
    /// file - defeating the cap's memory purpose even though `paths` itself
    /// stayed capped. `an_argv_flood_is_capped` above pins the OUTPUT cap; this
    /// pins the internal set that could not otherwise be observed from a test.
    #[test]
    fn dedup_set_does_not_grow_past_the_cap() {
        let paths: Vec<String> = (0..10_000)
            .map(|i| abs(&format!("cave{i}.stalx")))
            .collect();
        let (collected, seen) = collect_capped_with_seen(paths, Some);
        assert_eq!(collected.len(), MAX_LAUNCH_PATHS);
        assert_eq!(
            seen.len(),
            MAX_LAUNCH_PATHS,
            "the dedup set must not retain a surplus path once the cap is full"
        );
    }

    #[test]
    fn queue_holds_until_taken_and_dedupes() {
        let pending = PendingLaunchFiles::default();
        assert_eq!(pending.push_all(vec![abs("one.stalx")]), 1);
        // Still queued: nothing has drained it yet.
        assert_eq!(pending.push_all(vec![abs("one.stalx"), abs("two.tml")]), 1);
        assert_eq!(pending.take(), vec![abs("one.stalx"), abs("two.tml")]);
        assert!(pending.take().is_empty());
    }

    #[test]
    fn the_queue_is_capped_across_calls() {
        let pending = PendingLaunchFiles::default();
        let full: Vec<String> = (0..MAX_LAUNCH_PATHS)
            .map(|i| abs(&format!("cave{i}.stalx")))
            .collect();
        assert_eq!(pending.push_all(full), MAX_LAUNCH_PATHS);
        // A frontend that never drains cannot be made to accumulate.
        assert_eq!(pending.push_all(vec![abs("overflow.stalx")]), 0);
        assert_eq!(pending.take().len(), MAX_LAUNCH_PATHS);
    }

    /// `tauri.conf.json` tells the OS which extensions we handle; this module
    /// decides which it will accept. If those drift we either claim a type we
    /// then refuse to open, or accept one no OS will ever send.
    #[test]
    fn associations_match_launchable_extensions() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let associations = config["bundle"]["fileAssociations"]
            .as_array()
            .expect("bundle.fileAssociations must exist");
        let declared: HashSet<String> = associations
            .iter()
            .flat_map(|entry| {
                entry["ext"]
                    .as_array()
                    .expect("every association needs an ext list")
                    .iter()
                    .map(|ext| {
                        ext.as_str()
                            .expect("ext entries are strings")
                            .trim_start_matches('.')
                            .to_ascii_lowercase()
                    })
            })
            .collect();
        let accepted: HashSet<String> = LAUNCHABLE_EXTENSIONS
            .iter()
            .map(|ext| (*ext).to_owned())
            .collect();
        assert_eq!(
            declared, accepted,
            "tauri.conf.json fileAssociations and LAUNCHABLE_EXTENSIONS disagree"
        );
    }

    #[test]
    fn single_instance_stays_on_in_release_even_with_test_hooks() {
        // A release binary ignores the variable: the harness switch must never
        // reach a production build, whatever its environment says.
        assert!(single_instance_enabled(false, Some(OsStr::new("1"))));
        assert!(single_instance_enabled(false, None));
    }

    #[test]
    fn harness_debug_build_drops_single_instance() {
        // #576 HFI-7: a debug build started by the agent harness
        // (STALAGMIGHT_TEST_HOOKS=1) must not claim the session-bus name, or
        // parallel harness sessions kill each other.
        assert!(!single_instance_enabled(true, Some(OsStr::new("1"))));
    }

    #[test]
    fn plain_debug_build_keeps_single_instance() {
        // `./scripts/dev.sh` and every other debug launch without the harness
        // switch behave exactly as before.
        assert!(single_instance_enabled(true, None));
        for other in ["0", "true", "", "1 "] {
            assert!(
                single_instance_enabled(true, Some(OsStr::new(other))),
                "STALAGMIGHT_TEST_HOOKS={other:?} is not the harness switch"
            );
        }
    }
}
