//! The reader: bytes to a [`Decoded`] project.
//!
//! Order of checks: size, byte order mark, the sniff (legacy SQLite, version 2,
//! or neither) and the major version (before the body is parsed, since a newer
//! body may not parse at all), then the top-level members one by one, then the
//! model is assembled exactly as written and `Project::validate` runs. Load
//! never mutates the model: no heals, no re-minting, no re-sorting of
//! semantic lists.

use std::collections::BTreeSet;

use crate::models::survey::{
    Counters, Line, LoopClosure, Placement, Project, ProjectSettings, Station, ZeroDatum,
};
use crate::models::Section;

use super::canon::{canonical_checked, CanonError};
use super::decode_meta::{
    preserve, read_closure, read_conventions, read_counters, read_datum, read_history, read_ignore,
    read_imports, read_local, read_placement, read_project, read_section, read_settings,
    read_speleodb, read_strings, records, run, typed, ProjectMeta, RawCounters,
};
use super::decode_records::{Ctx, LineBuild, LinesSeed, Loc, StationsSeed};
use super::error::DecodeError;
use super::salvage::{self, SalvageReport};
use super::scan::{json_error, members, ScanError, MAX_SMALL_MEMBERS};
use super::summary::SUMMARY_KEYS;
use super::types::{
    DecodeNote, DecodeOptions, Decoded, Extras, IgnoredWarning, Limit, RawJson, ReadOnly,
};
use super::{KNOWN_FEATURES, SOLVER, SQLITE_MAGIC, VERSION};

/// Decode a version 2 file.
pub fn decode(bytes: &[u8], options: &DecodeOptions) -> Result<Decoded, DecodeError> {
    let doc = parse(bytes, options)?;
    let (project, extras, notes) = doc.assemble(None)?;
    let violations = project.validate();
    if !violations.is_empty() {
        return Err(DecodeError::Invalid { violations });
    }
    Ok(Decoded {
        project,
        extras,
        notes,
    })
}

/// Best-effort repair of a file that parses but breaks the model invariants
/// (plan 6.6): the repaired project (in memory only; the caller saves it to a
/// copy) and a report of every repair. `None` when the file does not parse or
/// the repairs do not converge.
pub fn decode_salvage(bytes: &[u8]) -> (Option<Project>, SalvageReport) {
    salvage::run(bytes)
}

/// What the sniff found in the first bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Sniffed {
    Legacy,
    ByteOrderMark,
    /// The version 2 prefix, with the version token when the bytes reach it.
    V2(Option<String>),
    /// Too short to tell: a prefix of the version 2 prefix, with how many of
    /// its tokens are complete.
    Incomplete(usize),
    NotStalx,
}

/// Sniff the first bytes. The version 2 prefix is `{"format":"stalx",
/// "version":`; insignificant whitespace between its tokens is tolerated (a
/// pretty-printed file), a byte order mark is not.
pub(super) fn sniff_detail(bytes: &[u8]) -> Sniffed {
    if bytes.starts_with(SQLITE_MAGIC) {
        return Sniffed::Legacy;
    }
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Sniffed::ByteOrderMark;
    }
    let mut i = 0;
    let ws = |i: &mut usize| {
        while bytes
            .get(*i)
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        {
            *i += 1;
        }
    };
    let tokens = [
        &b"{"[..],
        b"\"format\"",
        b":",
        b"\"stalx\"",
        b",",
        b"\"version\"",
        b":",
    ];
    for (matched, token) in tokens.into_iter().enumerate() {
        ws(&mut i);
        let rest = bytes.get(i..).unwrap_or_default();
        if rest.len() < token.len() {
            return if token.starts_with(rest) {
                Sniffed::Incomplete(matched)
            } else {
                Sniffed::NotStalx
            };
        }
        if !rest.starts_with(token) {
            return Sniffed::NotStalx;
        }
        i += token.len();
    }
    ws(&mut i);
    let start = i;
    while bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E'))
    {
        i += 1;
    }
    if i == start || i == bytes.len() {
        // No token yet, or it may continue past the bytes we have.
        return Sniffed::V2(None);
    }
    Sniffed::V2(Some(String::from_utf8_lossy(&bytes[start..i]).into_owned()))
}

/// How many prefix tokens (`{`, `"format"`, `:`, `"stalx"`) make a too-short
/// file a truncated version 2 file rather than something else.
pub(super) const INCOMPLETE_IS_TRUNCATED: usize = 4;

/// The checks before the body is parsed: size, BOM, sniff, major version.
fn preflight(bytes: &[u8], options: &DecodeOptions) -> Result<(), DecodeError> {
    let size = bytes.len() as u64;
    if size > options.max_file_bytes {
        return Err(DecodeError::TooLarge {
            size,
            max: options.max_file_bytes,
        });
    }
    match sniff_detail(bytes) {
        Sniffed::Legacy => Err(DecodeError::Legacy),
        Sniffed::ByteOrderMark => Err(DecodeError::ByteOrderMark),
        // Cut after `"stalx"`: a torn version 2 file, not some other file.
        Sniffed::Incomplete(matched) if matched >= INCOMPLETE_IS_TRUNCATED => {
            Err(DecodeError::Truncated { line: 1, column: 1 })
        }
        Sniffed::NotStalx | Sniffed::Incomplete(_) => Err(DecodeError::NotStalx),
        Sniffed::V2(None) => Err(DecodeError::Truncated { line: 1, column: 1 }),
        Sniffed::V2(Some(token)) => match token.parse::<u64>() {
            Ok(VERSION) => Ok(()),
            Ok(found) if found > VERSION => Err(DecodeError::NewerVersion {
                found,
                supported: VERSION,
            }),
            _ => Err(DecodeError::UnsupportedVersion { found: token }),
        },
    }
}

/// Everything a file holds, as read, before the model is assembled.
pub(super) struct RawDoc {
    pub(super) meta: ProjectMeta,
    pub(super) settings: ProjectSettings,
    pub(super) counters: RawCounters,
    pub(super) sections: Vec<Section>,
    pub(super) stations: Vec<Station>,
    pub(super) lines: Vec<Line>,
    pub(super) datums: Vec<ZeroDatum>,
    pub(super) placements: Vec<Placement>,
    pub(super) closures: Vec<LoopClosure>,
    pub(super) file_hashes: Vec<String>,
    pub(super) line_hashes: Vec<String>,
    pub(super) extras: Extras,
    pub(super) notes: Vec<DecodeNote>,
}

/// Parse every member (nothing assembled, nothing validated yet).
pub(super) fn parse(bytes: &[u8], options: &DecodeOptions) -> Result<RawDoc, DecodeError> {
    preflight(bytes, options)?;
    let whole = std::str::from_utf8(bytes).map_err(|e| {
        let before = &bytes[..e.valid_up_to()];
        let line = before.split(|&b| b == b'\n').count();
        let column = before.len()
            - before
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |i| i + 1)
            + 1;
        if e.error_len().is_none() {
            // The bytes end inside a character: a cut, not bad text.
            return DecodeError::Truncated { line, column };
        }
        DecodeError::Syntax {
            at: "the file".to_string(),
            line,
            column,
            message: "the file is not UTF-8 text".to_string(),
        }
    })?;
    let top =
        members(whole, MAX_SMALL_MEMBERS).map_err(|e| e.into_decode("the file", whole, whole))?;

    let mut ctx = Ctx::new(&options.limits);
    let mut notes = Vec::new();
    let mut extras = Extras {
        generator: String::new(),
        ..Extras::default()
    };
    let mut meta = None;
    let mut settings = ProjectSettings::default();
    let mut counters = RawCounters::default();
    let mut sections = Vec::new();
    let mut stations = Vec::new();
    let mut lines: Vec<LineBuild> = Vec::new();
    let mut datums = Vec::new();
    let mut placements = Vec::new();
    let mut closures = Vec::new();
    let mut file_hashes = Vec::new();
    let mut line_hashes = Vec::new();
    let (mut solver, mut conventions, mut end) = (false, false, false);

    for (index, (key, raw)) in top.into_iter().enumerate() {
        let text = raw.get();
        let at = |what: &str| format!("the {what} member");
        let simple = |e: serde_json::Error, what: &str| json_error(e, &at(what), whole, text);
        match key.as_ref() {
            "format" | "version" => {
                // The sniff checked both, in this order, at the start.
                if index > 1 {
                    return Err(DecodeError::DuplicateKey {
                        at: "the file".to_string(),
                        key: key.into_owned(),
                    });
                }
            }
            "solver" => {
                let found: u64 = serde_json::from_str(text).map_err(|e| simple(e, "solver"))?;
                if found != SOLVER {
                    notes.push(DecodeNote::OtherSolver { found });
                }
                solver = true;
            }
            "requires" => {
                extras.requires = read_strings(raw, "requires", Limit::Preserved, &mut ctx, whole)?;
            }
            "conventions" => {
                read_conventions(raw, whole)?;
                conventions = true;
            }
            "generator" => {
                extras.generator =
                    serde_json::from_str(text).map_err(|e| simple(e, "generator"))?;
            }
            "summary" => {
                // A cache: the values are never read here, but unknown keys
                // are kept. Not an object: nothing to keep.
                match members(text, MAX_SMALL_MEMBERS) {
                    Ok(pairs) => {
                        let rest = pairs
                            .into_iter()
                            .filter(|(k, _)| !SUMMARY_KEYS.contains(&k.as_ref()))
                            .collect();
                        extras.preserved.summary = preserve(rest, &at("summary"), &mut ctx)?;
                    }
                    Err(ScanError::DuplicateKey(key)) => {
                        return Err(DecodeError::DuplicateKey {
                            at: at("summary"),
                            key,
                        })
                    }
                    Err(_) => {}
                }
            }
            "project" => {
                let (read, preserved) = read_project(raw, whole, &mut ctx)?;
                meta = Some(read);
                extras.preserved.project = preserved;
            }
            "settings" => {
                let read = read_settings(raw, whole, &mut ctx)?;
                settings = read.settings;
                extras.preserved.settings = read.preserved;
                extras.preserved.validation = read.validation;
            }
            "counters" => counters = read_counters(raw, whole, &mut ctx)?,
            "speleodb" => {
                let (link, preserved) = read_speleodb(raw, whole, &mut ctx)?;
                extras.speleodb = Some(link);
                extras.preserved.speleodb = preserved;
            }
            "local" => {
                let (local, dropped) = read_local(raw, &options.limits);
                extras.local = local;
                notes.extend(dropped);
            }
            "sections" => {
                for (i, record) in records(raw, "sections", Limit::Sections, &mut ctx, whole)?
                    .into_iter()
                    .enumerate()
                {
                    sections.push(read_section(
                        record,
                        format!("sections[{i}]"),
                        whole,
                        &mut ctx,
                    )?);
                }
            }
            "stations" => {
                ctx.loc = Loc::Member("stations");
                run(
                    text,
                    StationsSeed {
                        ctx: &mut ctx,
                        out: &mut stations,
                    },
                )
                .map_err(|e| typed(e, &mut ctx, whole, text))?;
            }
            "lines" => {
                ctx.loc = Loc::Member("lines");
                run(
                    text,
                    LinesSeed {
                        ctx: &mut ctx,
                        out: &mut lines,
                    },
                )
                .map_err(|e| typed(e, &mut ctx, whole, text))?;
            }
            "datums" => {
                for (i, record) in records(raw, "datums", Limit::Datums, &mut ctx, whole)?
                    .into_iter()
                    .enumerate()
                {
                    datums.push(read_datum(record, format!("datums[{i}]"), whole, &mut ctx)?);
                }
            }
            "placements" => {
                for (i, record) in records(raw, "placements", Limit::Placements, &mut ctx, whole)?
                    .into_iter()
                    .enumerate()
                {
                    placements.push(read_placement(
                        record,
                        format!("placements[{i}]"),
                        whole,
                        &mut ctx,
                    )?);
                }
            }
            "closures" => {
                for (i, record) in records(raw, "closures", Limit::Closures, &mut ctx, whole)?
                    .into_iter()
                    .enumerate()
                {
                    closures.push(read_closure(
                        record,
                        format!("closures[{i}]"),
                        whole,
                        &mut ctx,
                    )?);
                }
            }
            "ignored_warnings" => {
                for (i, record) in records(
                    raw,
                    "ignored_warnings",
                    Limit::IgnoredWarnings,
                    &mut ctx,
                    whole,
                )?
                .into_iter()
                .enumerate()
                {
                    extras.ignored_warnings.push(read_ignore(
                        record,
                        format!("ignored_warnings[{i}]"),
                        whole,
                        &mut ctx,
                    )?);
                }
            }
            "imports" => (file_hashes, line_hashes) = read_imports(raw, whole, &mut ctx)?,
            "history" => match read_history(raw, &options.limits) {
                Ok(history) => extras.history = history,
                Err(reason) => {
                    tracing::warn!(reason = %reason, "Dropped an unreadable undo history from a .stalx file");
                    notes.push(DecodeNote::HistoryDropped { reason });
                }
            },
            "end" => {
                end = serde_json::from_str::<bool>(text).map_err(|e| simple(e, "end"))?;
                if !end {
                    return Err(DecodeError::MissingEnd);
                }
            }
            _ => {
                ctx.count_typed(Limit::Preserved, 1)?;
                let canonical = canonical_checked(text).map_err(|e| match e {
                    CanonError::TooDeep => DecodeError::TooDeep {
                        at: format!("the member {key:?}"),
                    },
                    CanonError::Invalid(message) => DecodeError::BadValue {
                        at: format!("the member {key:?}"),
                        message,
                    },
                })?;
                extras
                    .preserved
                    .top
                    .push((key.into_owned(), RawJson::from_canonical(canonical)));
            }
        }
    }
    if !end {
        return Err(DecodeError::MissingEnd);
    }
    for (present, field) in [(solver, "solver"), (conventions, "conventions")] {
        if !present {
            return Err(DecodeError::Missing {
                at: "the file".to_string(),
                field: field.to_string(),
            });
        }
    }
    let meta = meta.ok_or_else(|| DecodeError::Missing {
        at: "the file".to_string(),
        field: "project".to_string(),
    })?;

    let unknown_features: Vec<String> = extras
        .requires
        .iter()
        .filter(|name| !KNOWN_FEATURES.contains(&name.as_str()))
        .cloned()
        .collect();
    if !unknown_features.is_empty() || ctx.unknown_total > 0 {
        extras.read_only = Some(ReadOnly {
            unknown_features,
            unknown_content: std::mem::take(&mut ctx.unknown),
            unknown_content_total: ctx.unknown_total,
        });
    }
    Ok(RawDoc {
        meta,
        settings,
        counters,
        sections,
        stations,
        lines: lines.into_iter().map(|b| b.line).collect(),
        datums,
        placements,
        closures,
        file_hashes,
        line_hashes,
        extras,
        notes,
    })
}

impl RawDoc {
    /// Assemble the model exactly as written. With `repairs` (salvage), a
    /// repeated station id keeps the first record and is reported; without,
    /// it refuses.
    pub(super) fn assemble(
        self,
        mut repairs: Option<&mut Vec<super::Repair>>,
    ) -> Result<(Project, Extras, Vec<DecodeNote>), DecodeError> {
        let Self {
            meta,
            settings,
            counters,
            sections,
            stations,
            lines,
            datums,
            placements,
            closures,
            file_hashes,
            line_hashes,
            mut extras,
            mut notes,
        } = self;
        let mut project = Project::new_at(meta.name, meta.location, meta.created);
        project.modified_at = meta.modified;
        for station in stations {
            let id = station.id;
            if project.stations.contains_key(&id) {
                match repairs.as_deref_mut() {
                    Some(list) => list.push(super::Repair::DroppedDuplicateStation { id }),
                    None => return Err(DecodeError::DuplicateStation { id }),
                }
                continue;
            }
            project.stations.insert(id, station);
        }
        project.lines_mut().extend(lines);
        project.assign_session_shot_ids();
        project.datums = datums;
        project.placements = placements;
        project.closures = closures;
        project.sections = sections;
        project.settings = settings;
        project.imported_file_hashes = file_hashes;
        project.imported_line_hashes = line_hashes;
        project.counters = counters_of(&project, &counters);

        let before = extras.ignored_warnings.len();
        extras.ignored_warnings =
            kept_ignores(&project, std::mem::take(&mut extras.ignored_warnings));
        let dropped = before - extras.ignored_warnings.len();
        if dropped > 0 {
            notes.push(DecodeNote::IgnoresDropped { count: dropped });
        }
        Ok((project, extras, notes))
    }
}

/// The counters as written, or derived (one above every id in use) where the
/// file leaves one out.
fn counters_of(project: &Project, raw: &RawCounters) -> Counters {
    let defaults = Counters::default();
    let next_station = project
        .stations
        .values()
        .flat_map(|s| std::iter::once(s.id).chain(s.aliases.iter().map(|a| a.id)))
        .max()
        .map_or(defaults.next_station_id, |m| m.saturating_add(1));
    let next_closure = project
        .closures
        .iter()
        .map(|c| c.id)
        .max()
        .map_or(defaults.next_closure_id, |m| m.saturating_add(1));
    let next_section = project
        .sections
        .iter()
        .map(|s| s.id)
        .max()
        .map_or(defaults.next_section_id, |m| m.saturating_add(1));
    Counters {
        next_station_id: raw.station.unwrap_or(next_station),
        next_closure_id: raw.closure.unwrap_or(next_closure),
        next_section_id: raw.section.unwrap_or(next_section),
    }
}

/// The ignores that name a shot of the project, the first of each
/// (shot, code) pair, in file order.
fn kept_ignores(project: &Project, ignores: Vec<IgnoredWarning>) -> Vec<IgnoredWarning> {
    let uuids = project.shot_uuids();
    let mut seen = BTreeSet::new();
    ignores
        .into_iter()
        .filter(|w| uuids.contains(&w.shot) && seen.insert((w.shot.clone(), w.code.clone())))
        .collect()
}
