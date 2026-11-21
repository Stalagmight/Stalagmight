//! The cheap header read: the sniff, and the header members (`summary`,
//! `project`, `speleodb`, `local`, ...), read without decoding the survey
//! body. For Recents, the foreign-copy probe, logout's forget-recents, the
//! cached-copies listing and every other link reader (plan 6.1).
//!
//! The format does not fix the member order (`docs/file-format.md` section
//! 4), so [`probe_bytes`] and [`probe_path`] stop at the first body member
//! only when nothing they read can still follow it:
//!
//! - every header member they read came before it, or
//! - the header before it is in the canonical form Stalagmight writes
//!   (section 8: the known header members in table order, the first four on
//!   the first line and every later one on a line of its own, `requires`,
//!   `generator`, `summary` and `project` among them) and the `summary`
//!   names the same SpeleoDB project as the `speleodb` member (or, like it,
//!   none). A writer that keeps that layout keeps the order with it; the
//!   summary check catches a link moved below the body by hand (review
//!   finding O1 of package U5: an unlinked file, or one without local
//!   state, used to be read to its end).
//!
//! Otherwise they skip the body to the end of the file (review finding
//! C-file-format-11). [`probe_path_listing`] trusts the canonical order and
//! always stops there: for display-only listings.

use std::fmt;
use std::io::Read as _;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::de::{self, DeserializeSeed, Deserializer as _, IgnoredAny, MapAccess, Visitor};
use serde_json::value::RawValue;

use super::decode::{sniff_detail, Sniffed};
use super::decode_meta::read_local;
use super::scan::{members, Key, MAX_SMALL_MEMBERS};
use super::summary::Summary;
use super::tokens;
use super::types::{CountLimits, LocalState, SpeleoDbLink};
use super::write::parse_time;

/// What a file is, from its first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A legacy SQLite `.stalx` (read its header through the legacy reader).
    Legacy,
    /// A version 2 (or newer) JSON `.stalx`.
    V2,
    /// Not a Stalagmight project (or too short to tell).
    Unknown,
}

/// Sniff the first bytes (plan 6.1).
pub fn sniff(bytes: &[u8]) -> Format {
    match sniff_detail(bytes) {
        Sniffed::Legacy => Format::Legacy,
        Sniffed::V2(_) => Format::V2,
        Sniffed::ByteOrderMark | Sniffed::Incomplete(_) | Sniffed::NotStalx => Format::Unknown,
    }
}

/// The `project` member, as the header read gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub name: String,
    pub location: Option<String>,
    pub created: Option<DateTime<Utc>>,
    pub modified: Option<DateTime<Utc>>,
}

/// The cheap header of a file.
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    pub format: Format,
    /// The major version, when the bytes reach it.
    pub version: Option<u64>,
    pub requires: Vec<String>,
    pub generator: Option<String>,
    pub summary: Option<Summary>,
    pub project: Option<ProjectInfo>,
    pub speleodb: Option<SpeleoDbLink>,
    pub local: Option<LocalState>,
    /// Every header member the file has was seen: the read reached the first
    /// survey member with nothing it reads still to come (see the module
    /// docs), or the end of the file. ([`probe_path_listing`] sets it at the
    /// first survey member: it trusts the canonical order.)
    pub complete: bool,
}

impl Header {
    fn empty(format: Format) -> Self {
        Self {
            format,
            version: None,
            requires: Vec::new(),
            generator: None,
            summary: None,
            project: None,
            speleodb: None,
            local: None,
            complete: false,
        }
    }
}

/// How far a header read goes past the first survey member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scan {
    /// Until every header member the file has was seen, wherever it is.
    Whole,
    /// Up to the first survey member: the canonical order.
    UntilBody,
}

/// The header of the bytes given, which may be only a prefix of the file.
/// Never fails: what cannot be read is left unset. Member order does not
/// matter (see the module docs).
pub fn probe_bytes(bytes: &[u8]) -> Header {
    probe_bytes_with(bytes, Scan::Whole)
}

fn probe_bytes_with(bytes: &[u8], scan: Scan) -> Header {
    let format = sniff(bytes);
    let mut header = Header::empty(format);
    if format != Format::V2 {
        return header;
    }
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let mut complete = false;
    let result = de.deserialize_map(HeaderVisitor {
        header: &mut header,
        complete: &mut complete,
        scan,
        seen: 0,
        bytes,
        canonical: true,
        last: None,
    });
    header.complete = complete || result.is_ok();
    header
}

/// The header of the file at `path`, reading as few bytes as it can (64 KiB
/// first, doubling while the header is not complete, up to the decoder's
/// size cap). Member order does not matter (see the module docs): a file
/// Stalagmight wrote is read from its first 64 KiB whatever its header
/// holds, and a file whose header is neither canonical nor whole before its
/// body is read to its end.
pub fn probe_path(path: &Path) -> std::io::Result<Header> {
    probe_path_with(path, Scan::Whole).map(|(header, _)| header)
}

/// [`probe_path`], with how many bytes of the file it read.
#[cfg(test)]
pub(super) fn probe_path_counting(path: &Path) -> std::io::Result<(Header, usize)> {
    probe_path_with(path, Scan::Whole)
}

/// [`probe_path`] for a display-only listing (Recents): it stops at the first
/// survey member, trusting the canonical member order, so it reads only the
/// first bytes of any file. A file whose header members follow its body is
/// listed without them. Never for a decision about a link or sync state:
/// those read [`probe_path`].
pub fn probe_path_listing(path: &Path) -> std::io::Result<Header> {
    probe_path_with(path, Scan::UntilBody).map(|(header, _)| header)
}

/// The header, and how many bytes of the file the read took.
fn probe_path_with(path: &Path, scan: Scan) -> std::io::Result<(Header, usize)> {
    const FIRST: usize = 64 * 1024;
    // The decoder's own cap: a larger file opens nowhere.
    const MOST: usize = super::MAX_V2_FILE_BYTES as usize;
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    let mut want = FIRST;
    loop {
        let have = bytes.len();
        let read = (&mut file)
            .take((want - have) as u64)
            .read_to_end(&mut bytes)?;
        let header = probe_bytes_with(&bytes, scan);
        let at_end = read < want - have;
        if header.complete || at_end || want >= MOST || header.format != Format::V2 {
            return Ok((header, bytes.len()));
        }
        want = want.saturating_mul(2).min(MOST);
    }
}

/// Members after which the header is over (the canonical order).
const BODY: [&str; 10] = [
    "sections",
    "stations",
    "lines",
    "datums",
    "placements",
    "closures",
    "ignored_warnings",
    "imports",
    "history",
    "end",
];

/// The header members the probe reads, as bits of [`HeaderVisitor::seen`].
const READ: [&str; 6] = [
    "requires",
    "generator",
    "summary",
    "project",
    "speleodb",
    "local",
];

/// The members of [`READ`] Stalagmight always writes (`requires`,
/// `generator`, `summary`, `project`): a canonical header without one of them
/// is not one the canonical stop trusts.
const ALWAYS: u8 = 0b1111;

/// The header members in the order of the canonical form
/// (`docs/file-format.md` sections 4 and 8).
const HEAD: [&str; 12] = [
    "format",
    "version",
    "solver",
    "requires",
    "conventions",
    "generator",
    "summary",
    "project",
    "settings",
    "counters",
    "speleodb",
    "local",
];

/// How many of [`HEAD`] the canonical form puts on the first line.
const FIRST_LINE: usize = 4;

struct HeaderVisitor<'h, 'b> {
    header: &'h mut Header,
    complete: &'h mut bool,
    scan: Scan,
    /// Which of [`READ`] the read has met.
    seen: u8,
    /// The bytes read, for where each key sits.
    bytes: &'b [u8],
    /// Whether the members met so far are a canonical header: none out of
    /// order, unknown or not where the layout puts it.
    canonical: bool,
    /// The index in [`HEAD`] of the last member met.
    last: Option<usize>,
}

impl HeaderVisitor<'_, '_> {
    /// Note the header member `key`: it keeps the header canonical when it
    /// is a known header member after the previous one, at its place in the
    /// layout.
    fn note_header_key(&mut self, key: &str) {
        if !self.canonical {
            return;
        }
        let at = HEAD.iter().position(|k| *k == key);
        self.canonical = match at {
            Some(at) if self.last.is_none_or(|last| at > last) => {
                self.last = Some(at);
                if at < FIRST_LINE {
                    key_follows(self.bytes, key, b"{") || key_follows(self.bytes, key, b",")
                } else {
                    key_follows(self.bytes, key, b",\n")
                }
            }
            _ => false,
        };
    }

    /// Whether the first body member, `key`, ends a canonical header that
    /// nothing the probe reads can still follow (see the module docs).
    fn canonical_header_ends_at(&self, key: &str) -> bool {
        let link_agrees = self.header.summary.as_ref().is_some_and(|summary| {
            summary.speleodb.as_deref()
                == self.header.speleodb.as_ref().map(|l| l.project_id.as_str())
        });
        self.canonical
            && self.seen & ALWAYS == ALWAYS
            && link_agrees
            && key_follows(self.bytes, key, b",\n")
    }
}

/// Whether the key `key`, a slice of `bytes`, directly follows `before` and
/// its opening quote. A key that is not a slice of `bytes` (an escaped one,
/// which the parser unescapes into a string of its own) never does: no
/// canonical key has an escape.
fn key_follows(bytes: &[u8], key: &str, before: &[u8]) -> bool {
    let Some(start) = (key.as_ptr() as usize).checked_sub(bytes.as_ptr() as usize) else {
        return false;
    };
    start
        .checked_sub(before.len() + 1)
        .and_then(|from| bytes.get(from..start))
        .is_some_and(|prefix| prefix.ends_with(b"\"") && prefix.starts_with(before))
}

impl<'de> Visitor<'de> for HeaderVisitor<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a stalx file")
    }

    fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> Result<(), A::Error> {
        const ALL: u8 = (1 << READ.len()) - 1;
        while let Some(key) = map.next_key::<Key<'de>>()? {
            let key = key.0;
            if BODY.contains(&key.as_ref()) {
                if self.scan == Scan::UntilBody
                    || self.seen == ALL
                    || self.canonical_header_ends_at(&key)
                {
                    // Every member read is in (a later repeat is a duplicate
                    // key, which no reader accepts), or the canonical form
                    // puts none of the missing ones later.
                    *self.complete = true;
                    return Err(de::Error::custom("header complete"));
                }
                self.canonical = false;
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            self.note_header_key(&key);
            if let Some(bit) = READ.iter().position(|k| *k == key.as_ref()) {
                self.seen |= 1 << bit;
            }
            match key.as_ref() {
                "version" => self.header.version = map.next_value::<u64>().ok(),
                "requires" => {
                    let raw: &RawValue = map.next_value()?;
                    self.header.requires = serde_json::from_str(raw.get()).unwrap_or_default();
                }
                "generator" => {
                    let raw: &RawValue = map.next_value()?;
                    self.header.generator = serde_json::from_str(raw.get()).ok();
                }
                "summary" => {
                    let raw: &RawValue = map.next_value()?;
                    self.header.summary = Summary::read(raw.get());
                }
                "project" => {
                    let raw: &RawValue = map.next_value()?;
                    self.header.project = project_info(raw.get());
                }
                "speleodb" => {
                    let raw: &RawValue = map.next_value()?;
                    self.header.speleodb = speleodb(raw.get());
                }
                "local" => {
                    let raw: &RawValue = map.next_value()?;
                    self.header.local = Some(read_local(raw, &CountLimits::default()).0);
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

impl<'de> DeserializeSeed<'de> for HeaderVisitor<'_, '_> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

fn project_info(text: &str) -> Option<ProjectInfo> {
    let mut info = ProjectInfo {
        name: String::new(),
        location: None,
        created: None,
        modified: None,
    };
    let mut named = false;
    for (key, value) in members(text, MAX_SMALL_MEMBERS).ok()? {
        let raw = value.get();
        match key.as_ref() {
            "name" => {
                info.name = serde_json::from_str(raw).ok()?;
                named = true;
            }
            "location" => info.location = serde_json::from_str(raw).ok(),
            "created" => info.created = time(raw),
            "modified" => info.modified = time(raw),
            _ => {}
        }
    }
    named.then_some(info)
}

fn time(raw: &str) -> Option<DateTime<Utc>> {
    serde_json::from_str::<String>(raw)
        .ok()
        .and_then(|t| parse_time(&t))
}

fn speleodb(text: &str) -> Option<SpeleoDbLink> {
    let mut endpoint = None;
    let mut project_id = None;
    let mut project_name = None;
    let mut undo_mode = None;
    for (key, value) in members(text, MAX_SMALL_MEMBERS).ok()? {
        let raw = value.get();
        match key.as_ref() {
            "endpoint" => endpoint = serde_json::from_str(raw).ok(),
            "project" => project_id = serde_json::from_str(raw).ok(),
            "name" => project_name = serde_json::from_str(raw).ok(),
            "undo_mode" => {
                undo_mode = serde_json::from_str::<String>(raw)
                    .ok()
                    .map(|t| tokens::parse_undo_mode(&t));
            }
            _ => {}
        }
    }
    Some(SpeleoDbLink {
        endpoint: endpoint?,
        project_id: project_id?,
        project_name: project_name?,
        undo_mode,
    })
}
