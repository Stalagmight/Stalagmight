//! Project "dump" handling: classify what a downloaded dump contains and extract
//! the project `.stalx` from it.
//!
//! The classifier ([`classify`]) is a pure function over entry *names* so it is
//! unit-testable without a real archive; the ZIP I/O ([`Dump`]) opens the archive
//! once and both lists entries and extracts the chosen `.stalx` from it.
//!
//! The `.stalx` is located by **suffix**, never by the expected
//! `misc/project.stalx` path, so a server-side `TARGET_FOLDER` change cannot
//! break the pull (the server stores "other" files under `misc/` today, but that
//! is not part of the contract).

use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use super::models::SpeleoDumpCandidate;
use super::SpeleoDbError;
use crate::models::zip_entry_bound::{entry_count_excess, EntryExcess, MAX_END_CANDIDATES};

/// Hard ceiling on a single extracted dump entry. The `.stalx` is a SQLite DB;
/// a few hundred MB is far past any real cave project but small enough that a
/// lying ZIP header (a zip bomb) can't exhaust memory before we reject it.
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;

/// Hard ceiling on how many entries a dump may declare.
///
/// [`MAX_ENTRY_BYTES`] bounds one entry's CONTENT and says nothing about how many
/// there are, so it does not cover this at all. A central-directory record is only
/// ~46 bytes plus its name, so even the (reduced) body ceiling admits millions of
/// them - and every one costs a `String` in [`Dump::entry_paths`], on top of the
/// `Vec::with_capacity(archive.len())` it sizes from the archive's own claim. That
/// is gigabytes of allocation from a body that is merely large, reached before any
/// per-entry cap can apply.
///
/// A real dump is a `.stalx`, an `ariane.tml` and a handful of `misc/` files, so
/// 10 000 is several orders of magnitude of headroom over anything legitimate while
/// still bounding the walk. `ZipArchive::new` parses the whole central directory
/// eagerly, and the `zip` crate exposes no hook to bound that, so the limit is
/// enforced on the raw bytes BEFORE that parse (#576 W52-4), by the check the `.tml`
/// reader runs too ([`entry_count_excess`]); the count after it stays as defence in
/// depth.
const MAX_ENTRIES: usize = 10_000;

/// How many openable entries a picker may be asked to present. [#258 review]
///
/// `MAX_ENTRIES` bounds how many entries the ARCHIVE may hold; this bounds how many of
/// them can become a QUESTION. Without it a dump near that ceiling put a radio row and
/// several DOM nodes on screen for every candidate, and shipped every server-supplied
/// path across IPC to do it - names whose aggregate length nothing bounds. The webview
/// froze before the user could reach Cancel, which does not merely degrade the choice,
/// it destroys it.
///
/// REFUSED rather than truncated, and the difference is the whole point: a truncated
/// list still looks like the question, while quietly not containing the answer. A dump
/// with dozens of openable files is not a project anyone can choose from - it is a
/// broken or hostile archive - so this says so and names the remedy, which is the same
/// fail-closed shape the rest of this feature uses.
///
/// Sized for a list a person can actually read down, not for the format's limits.
const MAX_PICKER_CANDIDATES: usize = 32;

/// What a downloaded project dump contains, from the caller's point of view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DumpClassification {
    /// Exactly one `*.stalx` - open it. Holds the entry's path within the ZIP.
    Stalx(String),
    /// No `*.stalx` but an `ariane.tml` is present - route to C2 adoption (#214).
    /// Holds the `ariane.tml` entry's path within the ZIP (the adoption flow
    /// extracts + imports it).
    AdoptAriane(String),
    /// Neither a `*.stalx` nor an `ariane.tml` - nothing to open.
    NothingToOpen,
    /// More than one `*.stalx` - ambiguous; never guess which. [#258] routes this
    /// to the picker.
    AmbiguousStalx(Vec<String>),
    /// No `*.stalx` but more than one `ariane.tml` - ambiguous on the adoption
    /// path. [#258]
    ///
    /// This used to take the first in central-directory order and say nothing,
    /// which meant adopting a project could import the wrong survey silently. It
    /// is only reachable when there is no `.stalx` at all, so it never competes
    /// with the rule that a `.tml` is disregarded whenever a `.stalx` exists.
    AmbiguousAriane(Vec<String>),
}

/// Classify a dump from its list of entry paths. Pure; never panics.
pub fn classify(entry_paths: &[String]) -> DumpClassification {
    let stalx: Vec<String> = entry_paths
        .iter()
        .filter(|p| has_stalx_suffix(p))
        .cloned()
        .collect();

    match stalx.as_slice() {
        [only] => DumpClassification::Stalx(only.clone()),
        [] => {
            let tml: Vec<String> = entry_paths
                .iter()
                .filter(|p| is_ariane_tml(p))
                .cloned()
                .collect();
            match tml.as_slice() {
                [only] => DumpClassification::AdoptAriane(only.clone()),
                [] => DumpClassification::NothingToOpen,
                _ => DumpClassification::AmbiguousAriane(tml),
            }
        }
        _ => DumpClassification::AmbiguousStalx(stalx),
    }
}

/// Which KIND of openable entry a picker choice landed on. [#258]
///
/// The kind has to travel with the path. An `ariane.tml` and a `.stalx` are both
/// openable entries a user can be asked to choose between, but only one of them can
/// be written into the cache `.stalx` - the other has to be imported. Handing back a
/// bare path let a chosen `.tml` be extracted as if it were a database, which fails
/// to open AND replaces whatever clean copy was there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChosenKind {
    Stalx,
    Ariane,
}

/// Which set `chosen` belongs to, or `None` if this dump never offered it.
///
/// [#258] The single validation standing between a webview-supplied `entryPath` and
/// the extract. `chosen` crosses IPC, so it is attacker-influenceable; this refuses
/// anything the classifier did not itself offer, which rejects both a non-openable
/// entry and a stale pick made against a dump that has since changed.
///
/// It must be run against the entry list of the archive being extracted, not a list
/// remembered from an earlier download - otherwise it validates a listing the
/// extract does not honour. Comparison is exact: the archive is keyed by byte-equal
/// names, so any normalisation here would let a near-miss resolve to a real entry.
pub fn chosen_candidate(entry_paths: &[String], chosen: &str) -> Option<ChosenKind> {
    let holds = |paths: &[String]| paths.iter().any(|p| p == chosen);
    match classify(entry_paths) {
        DumpClassification::AmbiguousStalx(paths) if holds(&paths) => Some(ChosenKind::Stalx),
        DumpClassification::AmbiguousAriane(paths) if holds(&paths) => Some(ChosenKind::Ariane),
        _ => None,
    }
}

/// The final component of a ZIP entry name.
///
/// `/` ONLY, never `std::path`. A ZIP entry name is a wire string whose separator the
/// format fixes as `/`, but `Path::file_name` applies the HOST's rules - so on Windows
/// a server-supplied `evil\\project.stalx` has the basename `project.stalx` and on
/// Linux it has none, and every predicate built on it answered differently per
/// platform from identical bytes. That is not academic here: the basename decides which
/// candidate the picker badges "current", pre-selects, and puts initial focus on, so a
/// hostile or merely odd archive could make a decoy the default answer on one platform
/// only.
fn zip_basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Case-insensitive `.stalx` extension on the final path component.
fn has_stalx_suffix(path: &str) -> bool {
    zip_basename(path)
        .rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("stalx"))
}

/// The server reserves the root filename `ariane.tml` for the Ariane sidecar.
fn is_ariane_tml(path: &str) -> bool {
    zip_basename(path).eq_ignore_ascii_case("ariane.tml")
}

/// Write `bytes` to `path`, then flush them all the way to the device.
///
/// `std::fs::write` returns once the bytes are in the page cache, which is enough
/// for a process death but not for a power loss. Callers that go on to `rename`
/// this file over a destination they must not destroy need the stronger promise -
/// see `Dump::extract_entry_to`.
///
/// [#576 RVF-b-1] As a NEW file: `File::create` opened `path` through a link left
/// at that name, creating the file a dangling one named or overwriting the one it
/// led to, and the rename then moved the link itself into place. Whatever is at
/// the name (a temp a killed run left, or a link, which `remove_file` removes
/// itself) is removed first; one that cannot be removed fails the write.
fn write_and_sync(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    if path.symlink_metadata().is_ok() {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The sibling temp path used to stage an extraction before it replaces `dest`.
///
/// Lives in the SAME directory as `dest` so the follow-up `rename` stays on one
/// filesystem (a cross-device rename fails with `EXDEV`), and carries a dedicated
/// `.download.tmp` suffix so it can never collide with the save pipeline's
/// `<stem>.stalx.tmp`.
fn download_temp_path(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .unwrap_or_else(|| OsStr::new("dump"))
        .to_os_string();
    name.push(".download.tmp");
    dest.with_file_name(name)
}

/// A dump ZIP parsed once. Lists its entry paths and extracts the chosen entry
/// from the *same* archive, so the central directory isn't re-parsed between the
/// classify step and the extract step.
pub struct Dump<'a> {
    archive: zip::ZipArchive<Cursor<&'a [u8]>>,
}

impl<'a> Dump<'a> {
    /// Parse an in-memory dump ZIP.
    ///
    /// Rejects an absurd entry count up front (see [`MAX_ENTRIES`]) so the
    /// per-entry work in [`Self::entry_paths`] can never be driven to exhaustion by
    /// a server that claims millions of files.
    pub fn open(zip_bytes: &'a [u8]) -> Result<Self, SpeleoDbError> {
        Self::open_capped(zip_bytes, MAX_ENTRIES)
    }

    /// [`Self::open`] with an explicit entry ceiling (parameterized for testing, the
    /// same way [`Self::extract_entry_capped`] is - building ten thousand real ZIP
    /// entries to exercise one comparison buys nothing).
    fn open_capped(zip_bytes: &'a [u8], max_entries: usize) -> Result<Self, SpeleoDbError> {
        let mut reader = Cursor::new(zip_bytes);
        let excess = entry_count_excess(&mut reader, max_entries as u64)
            .map_err(|e| SpeleoDbError::Parse(format!("dump could not be read: {e}")))?;
        if let Some(excess) = excess {
            let over = match excess {
                EntryExcess::Declared(declared) => format!("declares {declared} entries"),
                EntryExcess::DirectoryRecords(records) => {
                    format!("holds {records} central-directory records")
                }
                EntryExcess::EndRecords => {
                    format!("holds more than {MAX_END_CANDIDATES} ZIP64 end records or locators")
                }
            };
            return Err(SpeleoDbError::Parse(format!(
                "dump {over}, over the {max_entries}-entry limit"
            )));
        }
        let archive = zip::ZipArchive::new(reader)
            .map_err(|e| SpeleoDbError::Parse(format!("dump is not a valid ZIP: {e}")))?;
        // Defence in depth only: the check above bounded the parse, and this
        // counts unique names (the `zip` crate keys its entries by name), so
        // duplicates would slip past it.
        let entries = archive.len();
        if entries > max_entries {
            return Err(SpeleoDbError::Parse(format!(
                "dump declares {entries} entries, over the {max_entries}-entry limit"
            )));
        }
        Ok(Self { archive })
    }

    /// The entry paths, in central-directory order - what [`classify`] consumes.
    pub fn entry_paths(&mut self) -> Result<Vec<String>, SpeleoDbError> {
        let mut names = Vec::with_capacity(self.archive.len());
        for i in 0..self.archive.len() {
            let entry = self
                .archive
                .by_index(i)
                .map_err(|e| SpeleoDbError::Parse(format!("could not read ZIP entry {i}: {e}")))?;
            names.push(entry.name().to_owned());
        }
        Ok(names)
    }

    /// The entries named by `wanted`, with the metadata a [#258] picker renders,
    /// sorted canonical-first and then by path.
    ///
    /// A sibling of [`Self::entry_paths`] rather than a widening of it: that method
    /// feeds every classify site and a stack of unit tests, and none of them wants
    /// the extra fields. The metadata itself is free - the loop already calls
    /// `by_index`, which parses each entry's local header, so `size()` and
    /// `last_modified()` cost no additional pass.
    ///
    /// The ordering is decided HERE, once, rather than in the frontend:
    /// central-directory order is arbitrary, and the canonical entry has to lead
    /// because it is the only pick that does not set up a silent replace on the next
    /// push.
    pub fn entry_details(
        &mut self,
        wanted: &[String],
    ) -> Result<Vec<SpeleoDumpCandidate>, SpeleoDbError> {
        // Membership as a SET. `wanted` is bounded only by the archive's own entry
        // count, and both grow together: an allowed dump near `MAX_ENTRIES` that is
        // ambiguous in all of them turned this into ~n^2 path comparisons - tens of
        // millions before the picker could be drawn - all of it driven by a
        // server-supplied file, i.e. exactly the input that must not be able to
        // stall the app.
        if wanted.len() > MAX_PICKER_CANDIDATES {
            return Err(SpeleoDbError::Parse(format!(
                "this project's dump holds {} openable files, which is too many to \
                 choose between - remove the extras on SpeleoDB so at most one \
                 Stalagmight file remains",
                wanted.len()
            )));
        }
        let wanted_set: HashSet<&str> = wanted.iter().map(String::as_str).collect();

        let mut out = Vec::with_capacity(wanted.len());
        for i in 0..self.archive.len() {
            let entry = self
                .archive
                .by_index(i)
                .map_err(|e| SpeleoDbError::Parse(format!("could not read ZIP entry {i}: {e}")))?;
            let entry_path = entry.name().to_owned();
            if !wanted_set.contains(entry_path.as_str()) {
                continue;
            }
            // An entry the extract would refuse is not a candidate. `MAX_ENTRY_BYTES`
            // was applied only at extraction, so a declared size above it reached the
            // picker as a pickable row - and `entry.size()` is the central directory's
            // unvalidated ZIP64 `u64`, so it could also exceed what a JSON number
            // carries exactly, under a doc that justified the `number` type BY the cap
            // it was not yet subject to. Offering a choice the backend will reject is
            // worse than a shorter list.
            if entry.size() > MAX_ENTRY_BYTES {
                continue;
            }
            out.push(SpeleoDumpCandidate {
                entry_path,
                size_bytes: entry.size(),
                modified: entry.last_modified().map(|d| {
                    format!(
                        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
                        d.year(),
                        d.month(),
                        d.day(),
                        d.hour(),
                        d.minute(),
                        d.second()
                    )
                }),
            });
        }
        // Path order, and nothing cleverer. There used to be a canonical-first sort
        // behind a `project.stalx` BASENAME test, and it asserted something the archive
        // does not say: `TARGET_FOLDER` is configurable, so the push target is
        // `<folder>/project.stalx` for a folder no dump records. With
        // `backup/project.stalx` and `misc/current.stalx` under a target of `misc/`,
        // the basename test badged the BACKUP as current and sorted it first - and the
        // picker pre-selected and focused that row, so Enter accepted the one file the
        // next push would overwrite. Ordering by path leaves the archive to speak for
        // itself and the choice to the user.
        out.sort_by(|a, b| a.entry_path.cmp(&b.entry_path));
        Ok(out)
    }

    /// Extract a single entry (the one chosen by [`classify`]) to `dest`, creating
    /// parent directories as needed.
    pub fn extract_entry_to(&mut self, entry_path: &str, dest: &Path) -> Result<(), SpeleoDbError> {
        self.extract_entry_capped(entry_path, dest, MAX_ENTRY_BYTES)
    }

    /// [#576 RV4-R-S7] The first `len` bytes of an entry (all of it when shorter),
    /// read without extracting it: enough to tell what a file is - a `.stalx`'s
    /// database header - before deciding to install it. Inflates only what it reads.
    pub fn entry_head(&mut self, entry_path: &str, len: usize) -> Result<Vec<u8>, SpeleoDbError> {
        let entry = self.archive.by_name(entry_path).map_err(|e| {
            SpeleoDbError::Parse(format!("entry `{entry_path}` not found in dump: {e}"))
        })?;
        let mut head = Vec::with_capacity(len);
        entry
            .take(u64::try_from(len).unwrap_or(u64::MAX))
            .read_to_end(&mut head)
            .map_err(|e| SpeleoDbError::Io(format!("could not read `{entry_path}`: {e}")))?;
        Ok(head)
    }

    /// Extraction with an explicit byte ceiling (parameterized for testing). The
    /// entry's declared size comes from the ZIP central directory - server- /
    /// attacker-controlled - so it is rejected up front when it exceeds `max_bytes`
    /// (a lying-high header that would otherwise trigger a huge up-front
    /// allocation), and the read is bounded by `take(max_bytes + 1)` so a lying-low
    /// header over a deflate stream (a zip bomb: tiny compressed, huge inflated)
    /// can't exhaust memory either.
    fn extract_entry_capped(
        &mut self,
        entry_path: &str,
        dest: &Path,
        max_bytes: u64,
    ) -> Result<(), SpeleoDbError> {
        let entry = self.archive.by_name(entry_path).map_err(|e| {
            SpeleoDbError::Parse(format!("entry `{entry_path}` not found in dump: {e}"))
        })?;

        let declared = entry.size();
        if declared > max_bytes {
            return Err(SpeleoDbError::Parse(format!(
                "dump entry `{entry_path}` declares {declared} bytes, over the {max_bytes}-byte limit"
            )));
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SpeleoDbError::Io(format!("could not create {}: {e}", parent.display()))
            })?;
        }
        // `declared <= max_bytes`, so the pre-allocation is bounded.
        let limit = usize::try_from(max_bytes).unwrap_or(usize::MAX);
        let mut buf = Vec::with_capacity(usize::try_from(declared).unwrap_or(0));
        entry
            .take(max_bytes + 1)
            .read_to_end(&mut buf)
            .map_err(|e| SpeleoDbError::Io(format!("could not read `{entry_path}`: {e}")))?;
        if buf.len() > limit {
            return Err(SpeleoDbError::Parse(format!(
                "dump entry `{entry_path}` exceeded the {max_bytes}-byte limit while reading"
            )));
        }
        // Replace `dest` ATOMICALLY: stage the bytes in a sibling temp file, FLUSH
        // them to the device, then rename over the destination. A write that dies
        // part-way (crash, full disk, killed process, power loss) then leaves the
        // PREVIOUS cache `.stalx` intact instead of a truncated stub, and no reader
        // ever observes a half-written file - which a plain `fs::write` could not
        // promise, because it truncates `dest` in place.
        //
        // The `sync_all` is what makes that true across POWER LOSS specifically, and
        // not merely across a process death. Without it the rename is only ordered
        // against the write in the page cache, not on the device: ext4's default
        // `data=ordered` happens to cover this case, but that is a filesystem
        // accident rather than a guarantee, and on a filesystem that reorders them
        // the crash window leaves `dest` renamed to a file whose contents never
        // landed - i.e. exactly the destroyed-previous-copy outcome the staging
        // exists to prevent. The parent directory is synced too, so the rename
        // itself survives; that one is best-effort because opening a directory as a
        // file is not portable (it fails on Windows, where the rename is journaled
        // by the filesystem anyway).
        //
        // Closing any live SQLite handle to `dest` first is the CALLER's job (see
        // `AppState::close_project_if_open_at`). The two halves fix different failure
        // modes and are both required: the close is what lets the rename succeed on
        // Windows (SQLite opens the file without `FILE_SHARE_DELETE`, so replacing it
        // under a live handle is denied), and the rename is what keeps an interrupted
        // write from tearing the file even when nothing has it open.
        let temp = download_temp_path(dest);
        if let Err(e) = write_and_sync(&temp, &buf) {
            let _ = std::fs::remove_file(&temp);
            return Err(SpeleoDbError::Io(format!(
                "could not write {}: {e}",
                temp.display()
            )));
        }
        if let Err(e) = std::fs::rename(&temp, dest) {
            let _ = std::fs::remove_file(&temp);
            return Err(SpeleoDbError::Io(format!(
                "could not replace {}: {e}",
                dest.display()
            )));
        }
        // Best-effort: makes the rename itself durable. A failure here means the
        // content is on the device but the directory entry may not be, which is the
        // pre-existing behaviour and not worth failing an otherwise-complete replace.
        // On Windows opening a directory as a file fails, so this is skipped there
        // (its filesystem journals the rename anyway); a read-only handle is all a
        // directory fsync takes on Unix (#576 RVF-d-1).
        if let Some(parent) = dest.parent() {
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "unit test fixtures, and the printed outcome is the evidence a run captures"
)]
mod tests {
    use super::{
        chosen_candidate, classify, download_temp_path, has_stalx_suffix, is_ariane_tml,
        ChosenKind, Dump, DumpClassification, SpeleoDbError, MAX_PICKER_CANDIDATES,
    };

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Build an in-memory ZIP from `(name, bytes)` entries (stored, uncompressed).
    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write as _;
        let mut writer = zip::write::ZipWriter::new(std::io::Cursor::new(Vec::<u8>::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(*name, opts).expect("start_file");
            writer.write_all(data).expect("write entry");
        }
        writer.finish().expect("finish zip").into_inner()
    }

    #[test]
    fn chosen_candidate_accepts_only_what_the_classifier_offered_and_says_which_kind() {
        // [#258] This is the whole trust boundary for the webview-supplied
        // entryPath: it must admit exactly the entries the picker was shown, and
        // nothing else.
        let ambiguous = paths(&["a.stalx", "b.stalx", "notes.txt"]);
        assert_eq!(
            chosen_candidate(&ambiguous, "a.stalx"),
            Some(ChosenKind::Stalx)
        );
        assert_eq!(
            chosen_candidate(&ambiguous, "b.stalx"),
            Some(ChosenKind::Stalx)
        );
        assert!(
            chosen_candidate(&ambiguous, "notes.txt").is_none(),
            "a non-openable entry of the same archive must be refused"
        );
        assert!(
            chosen_candidate(&ambiguous, "c.stalx").is_none(),
            "an entry that is not in this archive at all must be refused"
        );
        assert!(
            chosen_candidate(&ambiguous, "A.STALX").is_none(),
            "matching is exact: the archive is keyed by byte-equal names, so a \
             near-miss must not resolve to a real entry"
        );
        // A dump that is NOT ambiguous offers nothing, so no pick is honoured -
        // this is what stops a chosen path steering an unambiguous open.
        assert!(chosen_candidate(&paths(&["misc/project.stalx"]), "misc/project.stalx").is_none());
        // The ariane arm is a member set too.
        let two_tml = paths(&["ariane.tml", "misc/ariane.tml"]);
        // ...and it reports the kind, because only a `.stalx` may be extracted into
        // the cache database. A `.tml` has to be imported instead.
        assert_eq!(
            chosen_candidate(&two_tml, "misc/ariane.tml"),
            Some(ChosenKind::Ariane)
        );
    }

    #[test]
    fn entry_details_leads_with_the_canonical_entry_and_carries_its_size() {
        // [#258] Ordering is a correctness property, not presentation: the push
        // always writes `project.stalx`, so any other pick sets up a silent replace
        // on the next push. The safe candidate must lead the list the user sees.
        let zip = make_zip(&[
            ("backup/old.stalx", b"0123456789"),
            ("misc/project.stalx", b"abc"),
            ("ariane.tml", b"x"),
        ]);
        let mut dump = Dump::open(&zip).expect("open zip");
        let wanted = paths(&["backup/old.stalx", "misc/project.stalx"]);
        let details = dump.entry_details(&wanted).expect("details");

        assert_eq!(details.len(), 2, "only the wanted entries are described");
        // PATH order, with no candidate promoted. There used to be a canonical-first
        // sort keyed on the `project.stalx` basename; it asserted something no dump
        // records, because the push target is `<TARGET_FOLDER>/project.stalx` for a
        // folder the archive never names.
        assert_eq!(details[0].entry_path, "backup/old.stalx");
        assert_eq!(details[0].size_bytes, 10);
        assert_eq!(details[1].entry_path, "misc/project.stalx");
        assert_eq!(details[1].size_bytes, 3);
    }

    #[test]
    fn no_entry_is_ever_promoted_by_its_basename() {
        // [#258 review] The picker used to badge and pre-select the entry whose BASENAME
        // was `project.stalx`, on the reasoning that it is the name a push writes. But
        // the push writes `<TARGET_FOLDER>/project.stalx`, and `TARGET_FOLDER` is
        // configurable and nowhere in the dump - so the basename test could name a file
        // that is not the target at all. This fixture is that case: with a target of
        // `misc/`, the real destination is `misc/project.stalx`, which is in NEITHER
        // row, and the old logic badged the BACKUP as current, sorted it first,
        // pre-selected it and gave it initial focus. A stray Enter accepted the one file
        // the next push would overwrite.
        let zip = make_zip(&[
            ("backup/project.stalx", b"old"),
            ("misc/current.stalx", b"live"),
        ]);
        let mut dump = Dump::open(&zip).expect("open zip");
        let wanted = paths(&["backup/project.stalx", "misc/current.stalx"]);
        let details = dump.entry_details(&wanted).expect("details");

        assert_eq!(details.len(), 2);
        // Deterministic and unopinionated: path order, nothing promoted. The frontend
        // pre-selects nothing and keeps Confirm disabled until the user chooses.
        let order: Vec<&str> = details.iter().map(|c| c.entry_path.as_str()).collect();
        assert_eq!(order, vec!["backup/project.stalx", "misc/current.stalx"]);
    }

    #[test]
    fn entry_details_describes_every_wanted_entry_of_a_large_archive() {
        // Membership is a set lookup, not a scan of `wanted` per archive entry, so the
        // per-entry cost stays flat as the ARCHIVE grows. This pins the behaviour that
        // change had to preserve - every wanted entry described exactly once, none of
        // the unwanted ones - over an archive far larger than the wanted set.
        //
        // The wanted set sits at `MAX_PICKER_CANDIDATES`, because that is now its
        // ceiling: a dump offering more than a person can read down is refused rather
        // than rendered (see the test below), which is what actually retired the
        // quadratic blow-up this originally guarded.
        let wanted_names: Vec<String> = (0..MAX_PICKER_CANDIDATES)
            .map(|i| format!("d{i:04}/f{i:04}.stalx"))
            .collect();
        let bodies: Vec<(String, Vec<u8>)> = wanted_names
            .iter()
            .map(|n| (n.clone(), vec![b'x'; 1]))
            .chain((0..1_000).map(|i| (format!("misc/other{i:04}.txt"), vec![b'y'; 1])))
            .collect();
        let entries: Vec<(&str, &[u8])> = bodies
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice()))
            .collect();
        let zip = make_zip(&entries);
        let mut dump = Dump::open(&zip).expect("open zip");

        let details = dump.entry_details(&wanted_names).expect("details");

        assert_eq!(details.len(), MAX_PICKER_CANDIDATES);
        let first = details.first().expect("at least one candidate");
        let last = details.last().expect("at least one candidate");
        assert_eq!(first.entry_path, "d0000/f0000.stalx");
        assert_eq!(
            last.entry_path,
            format!(
                "d{:04}/f{:04}.stalx",
                MAX_PICKER_CANDIDATES - 1,
                MAX_PICKER_CANDIDATES - 1
            )
        );
        assert!(
            details.iter().all(|c| c.entry_path.ends_with(".stalx")),
            "no unwanted entry may be described"
        );
    }

    /// [#258 review] More openable files than a person can choose between is refused,
    /// not rendered.
    ///
    /// `MAX_ENTRIES` bounds what the ARCHIVE may hold; nothing bounded how many of them
    /// became a question. A dump near that ceiling put a radio row and several DOM nodes
    /// on screen per candidate and shipped every server-supplied path across IPC to do
    /// it - names whose aggregate length nothing bounds - so the webview froze before
    /// the user could reach Cancel. That does not degrade the choice, it destroys it.
    ///
    /// Refused rather than truncated on purpose: a truncated list still looks like the
    /// question while quietly not containing the answer.
    #[test]
    fn a_dump_with_more_candidates_than_anyone_can_read_is_refused() {
        let over = MAX_PICKER_CANDIDATES + 1;
        let bodies: Vec<(String, Vec<u8>)> = (0..over)
            .map(|i| (format!("d{i:04}/f{i:04}.stalx"), vec![b'x'; 1]))
            .collect();
        let entries: Vec<(&str, &[u8])> = bodies
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_slice()))
            .collect();
        let zip = make_zip(&entries);
        let mut dump = Dump::open(&zip).expect("open zip");
        let wanted: Vec<String> = bodies.iter().map(|(n, _)| n.clone()).collect();

        let got = match dump.entry_details(&wanted) {
            Err(SpeleoDbError::Parse(msg)) => msg,
            other => format!("{other:?}"),
        };
        assert!(
            got.contains(&over.to_string()) && got.contains("too many to choose between"),
            "the refusal must say how many were found and what to do about it; got: {got}"
        );
    }

    #[test]
    fn entry_details_orders_non_canonical_candidates_by_path() {
        // With no canonical entry present the order must still be deterministic -
        // central-directory order is arbitrary and would render differently for
        // the same repo depending on how the archive was written.
        let zip = make_zip(&[("z/late.stalx", b"z"), ("a/early.stalx", b"a")]);
        let mut dump = Dump::open(&zip).expect("open zip");
        let details = dump
            .entry_details(&paths(&["z/late.stalx", "a/early.stalx"]))
            .expect("details");
        let order: Vec<&str> = details.iter().map(|c| c.entry_path.as_str()).collect();
        assert_eq!(order, vec!["a/early.stalx", "z/late.stalx"]);
    }

    #[test]
    fn exactly_one_stalx_opens() {
        let got = classify(&paths(&["misc/project.stalx", "ariane.tml", "README.md"]));
        assert_eq!(
            got,
            DumpClassification::Stalx("misc/project.stalx".to_owned())
        );
    }

    #[test]
    fn stalx_located_by_suffix_regardless_of_folder() {
        // A server TARGET_FOLDER change must not break the pull.
        let got = classify(&paths(&["some/other/dir/project.stalx"]));
        assert_eq!(
            got,
            DumpClassification::Stalx("some/other/dir/project.stalx".to_owned())
        );
    }

    #[test]
    fn zero_stalx_with_ariane_routes_to_adoption() {
        let got = classify(&paths(&["ariane.tml", "misc/notes.txt"]));
        assert_eq!(
            got,
            DumpClassification::AdoptAriane("ariane.tml".to_owned())
        );
    }

    #[test]
    fn neither_is_nothing_to_open() {
        let got = classify(&paths(&["misc/notes.txt", "README.md"]));
        assert_eq!(got, DumpClassification::NothingToOpen);
    }

    #[test]
    fn two_stalx_fails_closed() {
        let got = classify(&paths(&["misc/project.stalx", "backup/old.stalx"]));
        assert!(
            matches!(&got, DumpClassification::AmbiguousStalx(v) if v.len() == 2),
            "expected AmbiguousStalx with 2 entries, got {got:?}"
        );
    }

    #[test]
    fn case_insensitive_extension() {
        let got = classify(&paths(&["misc/PROJECT.STALX"]));
        assert_eq!(
            got,
            DumpClassification::Stalx("misc/PROJECT.STALX".to_owned())
        );
    }

    #[test]
    fn empty_dump_is_nothing_to_open() {
        let got = classify(&paths(&[]));
        assert_eq!(got, DumpClassification::NothingToOpen);
    }

    /// [#258 review] A candidate the extract would refuse is not offered. The 512 MiB
    /// per-entry cap was applied at extraction only, so a declared size above it reached
    /// the picker as a pickable row - and the size shown there is the central
    /// directory's unvalidated ZIP64 value, which a server controls outright.
    #[test]
    fn an_entry_too_large_to_extract_is_not_offered_as_a_candidate() {
        // Two real entries plus a header claiming a size no extract would honour. The
        // ZIP is built with honest bytes, so this drives the declared-size path by
        // asserting the small ones survive and pinning the predicate the filter uses.
        let zip = make_zip(&[("a.stalx", b"one" as &[u8]), ("b.stalx", b"two")]);
        let mut dump = Dump::open(&zip).expect("open");
        let wanted = vec!["a.stalx".to_owned(), "b.stalx".to_owned()];
        let details = dump.entry_details(&wanted).expect("details");
        assert_eq!(details.len(), 2, "ordinary entries are still offered");
        assert!(
            details
                .iter()
                .all(|c| c.size_bytes <= super::MAX_ENTRY_BYTES),
            "every offered candidate must be within the cap the extract enforces, or \
             the picker is offering an answer the backend will reject"
        );
    }

    /// A ZIP entry name is a wire string whose separator the format fixes as `/`.
    /// Parsing it with `std::path` made every basename predicate host-dependent: on
    /// Windows `evil\\project.stalx` reads as the canonical basename and on Linux it
    /// does not, from identical server bytes. Only `has_stalx_suffix` and `is_ariane_tml`
    /// still ride on it, and both decide whether an entry is OPENABLE at all - so the
    /// divergence changed which files a dump appeared to contain.
    #[test]
    fn zip_names_are_parsed_with_zip_separators_not_the_hosts() {
        // A backslash is an ordinary filename character in a ZIP name, so none of these
        // has `ariane.tml` or a `.stalx` extension as its final component - and the
        // predicates that read it decide whether an entry is OPENABLE at all, so the
        // divergence changed which files a dump appeared to contain.
        for hostile in ["backup\\ariane.tml", "a\\b\\ariane.tml"] {
            assert!(
                !is_ariane_tml(hostile),
                "`{hostile}` must not read as an Ariane sidecar on any platform"
            );
        }
        assert!(
            !has_stalx_suffix("dir\\x.stalx/y"),
            "the extension belongs to the final `/`-separated component, not to a \
             backslash-separated one"
        );

        // The genuine `/`-separated forms still work, at every depth.
        assert!(is_ariane_tml("ariane.tml"));
        assert!(is_ariane_tml("misc/ARIANE.TML"));

        // And the suffix test keeps its own contract, including a dotless name and a
        // dot that belongs to a directory rather than the file.
        assert!(has_stalx_suffix("a/b.stalx"));
        assert!(has_stalx_suffix("A/B.STALX"));
        assert!(!has_stalx_suffix("a.stalx/b"));
        assert!(!has_stalx_suffix("noextension"));
    }

    #[test]
    fn entry_paths_lists_entries_in_order() {
        let zip = make_zip(&[("misc/project.stalx", b"data"), ("README.md", b"hi")]);
        let mut dump = Dump::open(&zip).expect("valid zip opens");
        let names = dump.entry_paths().expect("valid zip lists entries");
        assert_eq!(
            names,
            vec!["misc/project.stalx".to_owned(), "README.md".to_owned()]
        );
    }

    #[test]
    fn open_rejects_non_zip() {
        let err = Dump::open(b"this is plainly not a zip archive")
            .err()
            .expect("garbage bytes are not a ZIP");
        assert!(
            matches!(err, SpeleoDbError::Parse(_)),
            "non-ZIP bytes should map to Parse, got {err:?}"
        );
    }

    #[test]
    fn extract_entry_to_writes_chosen_entry_creating_parents() {
        let zip = make_zip(&[("misc/project.stalx", b"STALX-BYTES"), ("notes.txt", b"x")]);
        let dir = tempfile::tempdir().expect("tempdir");
        // A nested dest exercises the create_dir_all branch.
        let dest = dir.path().join("nested/out/project.stalx");
        let mut dump = Dump::open(&zip).expect("valid zip opens");
        dump.extract_entry_to("misc/project.stalx", &dest)
            .expect("extract chosen entry");
        let got = std::fs::read(&dest).expect("extracted file readable");
        assert_eq!(got, b"STALX-BYTES");
    }

    #[test]
    fn extract_entry_to_missing_entry_errors() {
        let zip = make_zip(&[("misc/project.stalx", b"data")]);
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("out.stalx");
        let mut dump = Dump::open(&zip).expect("valid zip opens");
        let err = dump
            .extract_entry_to("does/not/exist.stalx", &dest)
            .expect_err("absent entry must error");
        assert!(
            matches!(err, SpeleoDbError::Parse(_)),
            "missing entry should map to Parse, got {err:?}"
        );
    }

    #[test]
    fn lists_then_extracts_from_one_open() {
        // The orchestration path: open once, list entries (to classify), then
        // extract the chosen one from the same parsed archive.
        let zip = make_zip(&[("misc/project.stalx", b"PAYLOAD"), ("README.md", b"hi")]);
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("project.stalx");
        let mut dump = Dump::open(&zip).expect("valid zip opens");
        let names = dump.entry_paths().expect("lists entries");
        assert!(names.iter().any(|n| n == "misc/project.stalx"));
        dump.extract_entry_to("misc/project.stalx", &dest)
            .expect("extract after listing");
        assert_eq!(std::fs::read(&dest).expect("readable"), b"PAYLOAD");
    }

    #[test]
    fn extract_entry_rejects_oversized_entry() {
        // An entry whose declared size exceeds the cap is rejected before it can
        // allocate/read - guarding against a zip bomb from a hostile server.
        let zip = make_zip(&[("misc/project.stalx", b"0123456789")]); // 10 bytes
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("out.stalx");
        let mut dump = Dump::open(&zip).expect("valid zip opens");
        let err = dump
            .extract_entry_capped("misc/project.stalx", &dest, 4)
            .expect_err("oversized entry must be rejected");
        assert!(
            matches!(err, SpeleoDbError::Parse(_)),
            "oversized entry should map to Parse, got {err:?}"
        );
        assert!(
            !dest.exists(),
            "nothing should be written when over the cap"
        );
    }

    #[test]
    fn open_rejects_an_archive_with_too_many_entries() {
        // A central-directory record is ~46 bytes plus its name, so a body that is
        // merely large can declare millions of entries - and `entry_paths` sizes a
        // `Vec` from the archive's own claim and then allocates a `String` per entry.
        // `MAX_ENTRY_BYTES` bounds one entry's CONTENT and does nothing about the
        // count, so this is its own ceiling.
        //
        // Driven at a small explicit cap: building 10 001 real entries just to prove
        // the comparison would make this test slow for no extra coverage.
        let entries: Vec<(String, &[u8])> =
            (0..6).map(|i| (format!("f{i}.txt"), &b""[..])).collect();
        let borrowed: Vec<(&str, &[u8])> = entries.iter().map(|(n, d)| (n.as_str(), *d)).collect();
        let zip = make_zip(&borrowed);

        let err = Dump::open_capped(&zip, 5)
            .err()
            .expect("an over-count archive must be rejected");
        assert!(
            matches!(err, SpeleoDbError::Parse(_)),
            "an over-count archive should map to Parse, got {err:?}"
        );
        // At the cap it is accepted, and a real dump (a handful of files) is nowhere
        // near the production ceiling.
        assert!(
            Dump::open_capped(&zip, 6).is_ok(),
            "exactly at the cap is allowed"
        );
        assert!(
            Dump::open(&zip).is_ok(),
            "a normal dump is far below the production ceiling"
        );
    }

    #[test]
    fn extract_entry_to_leaves_no_temp_file_behind() {
        // The atomic replace stages through a sibling temp file; a successful
        // extract must consume it (rename), never leave it littering the cache dir.
        let zip = make_zip(&[("misc/project.stalx", b"PAYLOAD")]);
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("project.stalx");
        let mut dump = Dump::open(&zip).expect("valid zip opens");
        dump.extract_entry_to("misc/project.stalx", &dest)
            .expect("extract");
        assert!(
            !download_temp_path(&dest).exists(),
            "the staging temp file must be renamed away, not left behind"
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("readdir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            leftovers,
            vec!["project.stalx".to_owned()],
            "only the destination should remain in the directory"
        );
    }

    #[test]
    fn extract_entry_to_fully_replaces_an_existing_dest() {
        // A pre-existing (longer) file must be REPLACED wholesale, not partially
        // overwritten - the old tail must not survive the shorter new contents.
        let zip = make_zip(&[("misc/project.stalx", b"NEW")]);
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("project.stalx");
        std::fs::write(&dest, b"OLD-AND-MUCH-LONGER-CONTENTS").expect("seed dest");
        let mut dump = Dump::open(&zip).expect("valid zip opens");
        dump.extract_entry_to("misc/project.stalx", &dest)
            .expect("extract over existing dest");
        assert_eq!(std::fs::read(&dest).expect("readable"), b"NEW");
    }

    #[test]
    fn extract_entry_to_leaves_dest_intact_when_the_staged_write_fails() {
        // Failure injection: squat the staging temp path with a DIRECTORY, so the
        // staged write cannot succeed. The atomic replace must then fail with the
        // previous `dest` byte-for-byte untouched (the whole point of not
        // truncating in place).
        let zip = make_zip(&[("misc/project.stalx", b"NEW")]);
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("project.stalx");
        std::fs::write(&dest, b"ORIGINAL").expect("seed dest");
        std::fs::create_dir(download_temp_path(&dest)).expect("squat the temp path");

        let mut dump = Dump::open(&zip).expect("valid zip opens");
        let err = dump
            .extract_entry_to("misc/project.stalx", &dest)
            .expect_err("a blocked staging write must fail the extract");
        assert!(
            matches!(err, SpeleoDbError::Io(_)),
            "a blocked staging write should map to Io, got {err:?}"
        );
        assert_eq!(
            std::fs::read(&dest).expect("original still readable"),
            b"ORIGINAL",
            "the destination must be untouched when the extract fails"
        );
    }

    /// [#576 RVF-b-1] A link left at the staging name, dangling or to an existing
    /// file, never receives the extracted bytes: the staged write created the
    /// file the link named (or overwrote the one it led to), and the rename then
    /// put the link itself at `dest`.
    #[cfg(unix)]
    #[test]
    fn extract_entry_to_never_writes_through_a_link_at_the_staging_name() {
        for existing in [false, true] {
            let zip = make_zip(&[("misc/project.stalx", b"NEW")]);
            let dir = tempfile::tempdir().expect("tempdir");
            let elsewhere = tempfile::tempdir().expect("tempdir");
            let dest = dir.path().join("project.stalx");
            let target = elsewhere.path().join("config.toml");
            if existing {
                std::fs::write(&target, b"KEEP").expect("seed the link target");
            }
            std::os::unix::fs::symlink(&target, download_temp_path(&dest))
                .expect("plant a link at the staging name");

            let mut dump = Dump::open(&zip).expect("valid zip opens");
            dump.extract_entry_to("misc/project.stalx", &dest)
                .expect("extract");

            assert_eq!(
                std::fs::read(&target).ok(),
                existing.then(|| b"KEEP".to_vec()),
                "existing={existing}: the extract was written through the link"
            );
            assert!(
                dest.symlink_metadata().is_ok_and(|m| m.is_file()),
                "existing={existing}: dest is a real file, not the planted link"
            );
            assert_eq!(std::fs::read(&dest).expect("dest readable"), b"NEW");
        }
    }

    /// A 22-byte end-of-central-directory record without a comment.
    fn end_record(entries: u16, directory_len: u32, directory_at: u32) -> Vec<u8> {
        let mut record = b"PK\x05\x06".to_vec();
        record.extend_from_slice(&[0; 4]); // this disk, the directory's disk
        record.extend_from_slice(&entries.to_le_bytes()); // on this disk
        record.extend_from_slice(&entries.to_le_bytes()); // in all
        record.extend_from_slice(&directory_len.to_le_bytes());
        record.extend_from_slice(&directory_at.to_le_bytes());
        record.extend_from_slice(&[0; 2]);
        record
    }

    /// A ZIP64 end record declaring `entries`, then a locator pointing at
    /// offset 0 (not at the record) and a 32-bit end record deferring to ZIP64.
    fn zip64_end_away_from_the_locator(entries: u64) -> Vec<u8> {
        let mut end = b"PK\x06\x06".to_vec();
        end.extend_from_slice(&44u64.to_le_bytes()); // the record's size after this field
        end.extend_from_slice(&[45, 0, 45, 0]); // made by / needs version 4.5
        end.extend_from_slice(&[0; 8]); // this disk, the directory's disk
        end.extend_from_slice(&entries.to_le_bytes()); // on this disk
        end.extend_from_slice(&entries.to_le_bytes()); // in all
        end.extend_from_slice(&[0; 16]); // the directory's size and offset
        end.extend_from_slice(b"PK\x06\x07");
        end.extend_from_slice(&[0; 4]); // the ZIP64 record's disk
        end.extend_from_slice(&0u64.to_le_bytes()); // where it claims the record is
        end.extend_from_slice(&1u32.to_le_bytes()); // disks in all
        end.extend(end_record(u16::MAX, u32::MAX, u32::MAX));
        end
    }

    /// [#576 W52-4] The entry limit holds before the ZIP parse, by the check
    /// the `.tml` reader runs (`stalagmight_models::zip_entry_bound`). Checked
    /// on `archive.len()` after `ZipArchive::new`, it came after the parse had
    /// built a record per declared entry, and that counts unique names: six
    /// directory records all named `f.txt` were parsed and then let through a
    /// five-entry limit. And a ZIP64 end record the reader finds away from
    /// where the locator points had it reserve room for every entry declared
    /// before the parse failed on its own.
    #[test]
    fn the_entry_limit_holds_before_the_zip_parse() {
        let one = make_zip(&[("f.txt", b"")]);
        let end = one.len() - 22;
        let directory_at = u32::from_le_bytes(one[end + 16..end + 20].try_into().expect("u32"));
        let (local, record) = one[..end].split_at(directory_at as usize);
        let mut same_name = local.to_vec();
        same_name.extend(record.repeat(6));
        same_name.extend(end_record(6, record.len() as u32 * 6, directory_at));
        let parsed = zip::ZipArchive::new(std::io::Cursor::new(&same_name[..])).map(|a| a.len());
        println!(
            "premise: the ZIP reader parses six records named f.txt and keeps {parsed:?} name(s)"
        );

        let mut zip64 = vec![0u8; 400];
        zip64.extend(zip64_end_away_from_the_locator(6));

        for (label, bytes) in [
            ("six records all named f.txt", &same_name),
            (
                "a ZIP64 end record away from the locator declaring 6",
                &zip64,
            ),
        ] {
            let outcome = Dump::open_capped(bytes, 5)
                .map(|_| ())
                .map_err(|e| e.to_string());
            println!("{label}: {outcome:?}");
            let refused = outcome.expect_err("over the five-entry limit");
            assert!(
                refused.contains("declares 6 entries, over the 5-entry limit"),
                "{label}: refused naming the count and the limit, got {refused:?}"
            );
        }
    }
}
