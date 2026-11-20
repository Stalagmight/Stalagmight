//! The cheap header read: the sniff, and the members before the survey body
//! (`summary`, `project`, `speleodb`, `local`), read from the first bytes with
//! a reader that stops at the first body member. For Recents, the
//! foreign-copy probe, logout's forget-recents, the cached-copies listing and
//! every other link reader (plan 6.1).

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
    /// The read reached the first survey member (or the end of the file), so
    /// every header member the file has was seen. The probe trusts the
    /// canonical member order; the full decode does not.
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

/// The header of the bytes given, which may be only a prefix of the file.
/// Never fails: what cannot be read is left unset.
pub fn probe_bytes(bytes: &[u8]) -> Header {
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
    });
    header.complete = complete || result.is_ok();
    header
}

/// The header of the file at `path`, reading as few bytes as it can (64 KiB
/// first, doubling up to 16 MiB while the header is not complete).
pub fn probe_path(path: &Path) -> std::io::Result<Header> {
    const FIRST: usize = 64 * 1024;
    const MOST: usize = 16 * 1024 * 1024;
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    let mut want = FIRST;
    loop {
        let have = bytes.len();
        let read = (&mut file)
            .take((want - have) as u64)
            .read_to_end(&mut bytes)?;
        let header = probe_bytes(&bytes);
        let at_end = read < want - have;
        if header.complete || at_end || want >= MOST || header.format != Format::V2 {
            return Ok(header);
        }
        want *= 2;
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

struct HeaderVisitor<'h> {
    header: &'h mut Header,
    complete: &'h mut bool,
}

impl<'de> Visitor<'de> for HeaderVisitor<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a stalx file")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<Key<'de>>()? {
            let key = key.0;
            if BODY.contains(&key.as_ref()) {
                *self.complete = true;
                return Err(de::Error::custom("header complete"));
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

impl<'de> DeserializeSeed<'de> for HeaderVisitor<'_> {
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
