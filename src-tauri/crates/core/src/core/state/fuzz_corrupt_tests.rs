//! `FORMAT_CORRUPT=1`: damage ONE persisted history entry of the `.stalx` v2
//! file between a save and the reopen, and hold the load to the rule
//! docs/undo-redo.md and docs/file-format.md (5.15) state for it.
//!
//! A history entry is one JSON object per physical line of the file's
//! `history` member (`{"v":1,"t":...,"k":...,"label":...,"refs":...,
//! "preview":...,"ops":...}`). Every edit below keeps the FILE valid JSON, so
//! the survey still opens, and has a DETERMINATE right answer:
//!
//! | kind | what it does | the load must |
//! |---|---|---|
//! | `version` | `"v":1` becomes `"v":2` (a later build's entry) | drop the whole history (both stacks), with the banner |
//! | `unknown-member` | an extra member in the entry | drop it whole: the layout is strict |
//! | `not-an-object` | the entry becomes `0` | drop it whole |
//! | `bad-time` | `"t"` is not an RFC 3339 time | drop it whole |
//! | `line-hash` | one line side's content hash changed by one bit | KEEP it (nothing in the entry alone is wrong) - and then a step through it is REFUSED, changes nothing and keeps the entry, in either direction, for good |
//!
//! Corruptions with no determinate answer are not generated: a changed
//! value inside a run is a valid entry to a DIFFERENT state, which nothing
//! short of a checksum over the whole survey could tell from a real one.
//!
//! Every draw comes from the coverage stream (`Fuzzer::aux`).
//!
//! RETIRED with the SQLite history (each pinned the gzip-row format the
//! redesign deleted): `line-count`, `edit-id`, `copy-past-end`,
//! `repeated-keep`, `keep-past-end`, `arr-length`, `bag-absent-del`,
//! `healed-member-del` (delta-engine runs and bags), `v3-envelope`,
//! `garbage-bytes`, `truncated-gzip` (the row envelope and its compression).
//! The engine-level byte flips and truncations of an entry are
//! `models::survey::changeset::entry_tests`.

use std::fmt;
use std::path::Path;

use super::Rng;
use crate::core::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Version,
    UnknownMember,
    NotAnObject,
    BadTime,
    LineHash,
}

const KINDS: [Kind; 5] = [
    Kind::Version,
    Kind::UnknownMember,
    Kind::NotAnObject,
    Kind::BadTime,
    Kind::LineHash,
];

impl Kind {
    const fn name(self) -> &'static str {
        match self {
            Self::Version => "version",
            Self::UnknownMember => "unknown-member",
            Self::NotAnObject => "not-an-object",
            Self::BadTime => "bad-time",
            Self::LineHash => "line-hash",
        }
    }
}

/// Which stack of the file an entry is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stack {
    Undo,
    Redo,
}

/// One rewritten entry, named for the move log and the failure payload.
#[derive(Clone, Debug)]
pub(super) struct Corruption {
    kind: Kind,
    stack: Stack,
    /// The entry's position in its list (file order: oldest first), and how
    /// many the list held.
    index: usize,
    count: usize,
}

impl fmt::Display for Corruption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} entry {} of {} ({})",
            match self.stack {
                Stack::Undo => "undo",
                Stack::Redo => "redo",
            },
            self.index,
            self.count,
            self.kind.name()
        )
    }
}

impl Corruption {
    /// The entry still decodes, so the load keeps the history.
    pub(super) fn must_be_kept(&self) -> bool {
        self.kind == Kind::LineHash
    }

    pub(super) const fn kind_name(&self) -> &'static str {
        self.kind.name()
    }

    /// The depth the damaged entry leads INTO, given the window the reopen
    /// left (`undo_kept` entries below `cursor`, `redo_kept` above it).
    pub(super) fn depth(&self, cursor: usize, undo_kept: usize, redo_kept: usize) -> Option<usize> {
        match self.stack {
            Stack::Undo => {
                let from_top = self.count.checked_sub(1 + self.index)?;
                (self.count == undo_kept).then(|| cursor - from_top)
            }
            Stack::Redo => {
                let from_cursor = self.count.checked_sub(self.index)?;
                (self.count == redo_kept).then(|| cursor + from_cursor)
            }
        }
    }
}

/// Where the history entries sit in a v2 file: `(line number, stack, index
/// in the list)`, plus each list's length.
fn entry_lines(lines: &[&str]) -> (Vec<(usize, Stack, usize)>, usize, usize) {
    let mut out = Vec::new();
    let mut stack = None;
    let (mut undo, mut redo) = (0, 0);
    for (at, line) in lines.iter().enumerate() {
        match *line {
            "\"history\":{\"undo\":[" => stack = Some(Stack::Undo),
            "\"history\":{\"redo\":[" | "],\"redo\":[" => stack = Some(Stack::Redo),
            _ if line.starts_with(']') => stack = None,
            _ => {
                if let Some(which) = stack {
                    let index = match which {
                        Stack::Undo => &mut undo,
                        Stack::Redo => &mut redo,
                    };
                    out.push((at, which, *index));
                    *index += 1;
                }
            }
        }
    }
    (out, undo, redo)
}

/// The entry text with `kind` applied, or `None` when it does not apply (no
/// line side to change a hash of).
fn damage(entry: &str, kind: Kind) -> Option<String> {
    use crate::models::survey::changeset::HISTORY_FORMAT_VERSION as VERSION;
    match kind {
        Kind::Version => entry
            .strip_prefix(&format!("{{\"v\":{VERSION},"))
            .map(|rest| format!("{{\"v\":{},{rest}", VERSION + 1)),
        Kind::UnknownMember => entry
            .strip_prefix(&format!("{{\"v\":{VERSION},"))
            .map(|rest| format!("{{\"v\":{VERSION},\"zz\":0,{rest}")),
        Kind::NotAnObject => Some("0".to_string()),
        Kind::BadTime => {
            let start = entry.find("\"t\":\"")? + 5;
            let end = start + entry[start..].find('"')?;
            Some(format!("{}yesterday{}", &entry[..start], &entry[end..]))
        }
        Kind::LineHash => {
            // A side's `"h":<digits>` (list hunks are `"h":[`).
            let mut from = 0;
            while let Some(found) = entry[from..].find("\"h\":") {
                let start = from + found + 4;
                let digits = entry[start..]
                    .bytes()
                    .take_while(u8::is_ascii_digit)
                    .count();
                if digits > 0 {
                    let value: u64 = entry[start..start + digits].parse().ok()?;
                    return Some(format!(
                        "{}{}{}",
                        &entry[..start],
                        value ^ 1,
                        &entry[start + digits..]
                    ));
                }
                from = start;
            }
            None
        }
    }
}

/// Damage one history entry of the file at `path`, chosen from the coverage
/// stream. `None` when the file holds no history.
pub(super) fn corrupt_one_entry(path: &Path, aux: &mut Rng) -> Option<Corruption> {
    let text = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = text.split('\n').collect();
    let (entries, undo, redo) = entry_lines(&lines);
    if entries.is_empty() {
        return None;
    }
    let (at, stack, index) = entries[aux.below(entries.len())];
    let line = lines[at];
    let (entry, comma) = line
        .strip_suffix(',')
        .map_or((line, ""), |entry| (entry, ","));
    let mut kind = KINDS[aux.below(KINDS.len())];
    let damaged = damage(entry, kind).or_else(|| {
        kind = Kind::Version;
        damage(entry, kind)
    })?;
    let mut out: Vec<String> = lines.iter().map(|l| (*l).to_string()).collect();
    out[at] = format!("{damaged}{comma}");
    std::fs::write(path, out.join("\n")).ok()?;
    Some(Corruption {
        kind,
        stack,
        index,
        count: match stack {
            Stack::Undo => undo,
            Stack::Redo => redo,
        },
    })
}

/// Each kind, pinned on its own: the three refused kinds drop the whole
/// history (both stacks) and say so; a changed line hash keeps it, and a step
/// through that entry is refused, changes nothing, and keeps it.
#[test]
fn each_damaged_entry_kind_gets_its_determinate_answer() {
    let _config = crate::test_helpers::isolated_config();
    for kind in KINDS {
        let fuzzer = super::Fuzzer::new(2, false);
        // Three length edits; the newest is undone (a redo entry too).
        for length in [3.5, 4.5, 5.5] {
            let targets = super::read_targets(&fuzzer.state);
            let line = targets
                .lines
                .iter()
                .find(|(_, shots)| !shots.is_empty())
                .unwrap();
            let r = fuzzer.state.execute(Command::UpdateShot {
                line_id: line.0,
                shot_id: line.1[0],
                updates: crate::models::ShotUpdates {
                    length: Some(length),
                    ..crate::models::ShotUpdates::default()
                },
            });
            assert!(r.success);
        }
        assert!(fuzzer.state.undo().success);
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("damaged.stalx");
        assert!(fuzzer.state.save_project(Some(path.clone())).success);
        assert!(fuzzer.state.execute(Command::CloseProject).success);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.split('\n').collect();
        let (entries, undo, redo) = entry_lines(&lines);
        assert_eq!((undo, redo), (2, 1), "{kind:?}: the file's history");
        // The newest undo entry: the one the next undo steps through.
        let (at, _, _) = entries[1];
        let (entry, comma) = lines[at]
            .strip_suffix(',')
            .map_or((lines[at], ""), |entry| (entry, ","));
        let damaged = damage(entry, kind).unwrap();
        let mut out: Vec<String> = lines.iter().map(|l| (*l).to_string()).collect();
        out[at] = format!("{damaged}{comma}");
        std::fs::write(&path, out.join("\n")).unwrap();
        let opened = fuzzer.state.execute(Command::OpenProject { path });
        assert!(opened.success, "{kind:?}: {:?}", opened.error);
        if kind == Kind::LineHash {
            assert!(!fuzzer.state.history_discarded(), "{kind:?} was dropped");
            assert_eq!((fuzzer.undo_len(), fuzzer.redo_len()), (2, 1));
            let before = fuzzer.live();
            let r = fuzzer.state.undo();
            assert!(!r.success, "a step through a damaged hash succeeded");
            assert_eq!(fuzzer.live(), before, "the refused step changed the survey");
            assert_eq!((fuzzer.undo_len(), fuzzer.redo_len()), (2, 1));
            // The redo side is intact and still steps.
            assert!(fuzzer.state.redo().success);
            assert!(fuzzer.state.undo().success);
            assert!(!fuzzer.state.undo().success, "refused for good");
        } else {
            assert!(fuzzer.state.history_discarded(), "{kind:?} was kept");
            assert_eq!(
                (fuzzer.undo_len(), fuzzer.redo_len()),
                (0, 0),
                "{kind:?}: part of the history survived"
            );
        }
        assert!(fuzzer.live().validate().is_empty());
    }
}
