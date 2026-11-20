//! Rewrites that copy every survey member verbatim (plan 6.5 mode 4 and the
//! Recents rename): the bytes of the last saved file are split into members
//! and written back in canonical order, with only the members being changed
//! replaced. The body is never decoded, so these work on read-only files and
//! on files with an unknown `requires` (the rename excepted, which changes
//! survey content).

use serde_json::value::RawValue;

use super::decode::{sniff_detail, Sniffed};
use super::encode::{write_history, write_local, write_speleodb};
use super::error::{DecodeError, EncodeError};
use super::scan::{members, Member, MAX_SMALL_MEMBERS};
use super::summary::SUMMARY_KEYS;
use super::types::Extras;
use super::write::string;
use super::{KNOWN_FEATURES, VERSION};

/// Rewrite the `speleodb`, `local` and `history` members of `original` (the
/// bytes of the last saved version 2 file) from `extras`, copying every
/// other member verbatim. `summary.speleodb` follows the new link. The
/// generator and `requires` stay as they were: the body is unchanged.
pub fn rewrite_local(original: &[u8], extras: &Extras) -> Result<Vec<u8>, EncodeError> {
    let text = complete_v2(original)?;
    let top = top_members(text)?;
    let mut out = Vec::with_capacity(original.len() + 1024);
    write_header(&mut out, &top, |key, raw, out| {
        if key == "summary" {
            let id = extras.speleodb.as_ref().map(|l| l.project_id.as_str());
            replace_keys(out, raw, Some(&SUMMARY_KEYS), &[("speleodb", id)])
        } else {
            out.extend_from_slice(raw.get().as_bytes());
            Ok(())
        }
    })?;
    if let Some(link) = &extras.speleodb {
        write_speleodb(&mut out, link, &extras.preserved.speleodb);
    }
    write_local(&mut out, &extras.local);
    write_body(&mut out, &top);
    write_history(&mut out, &extras.history.undo, &extras.history.redo);
    out.extend_from_slice(b"\"end\":true}\n");
    Ok(out)
}

/// Rename a closed version 2 file: `project.name` and `summary.name` become
/// `name`, every other member is copied verbatim (Recents rename, plan 6.5).
/// Refused when the file requires a feature this build does not know.
pub fn rename_project(original: &[u8], name: &str) -> Result<Vec<u8>, EncodeError> {
    let text = complete_v2(original)?;
    let top = top_members(text)?;
    if let Some((_, raw)) = top.iter().find(|(k, _)| k == "requires") {
        let requires: Vec<String> = serde_json::from_str(raw.get()).unwrap_or_default();
        if let Some(unknown) = requires
            .into_iter()
            .find(|f| !KNOWN_FEATURES.contains(&f.as_str()))
        {
            return Err(EncodeError::UnknownFeature { name: unknown });
        }
    }
    let mut out = Vec::with_capacity(original.len() + 256);
    write_header(&mut out, &top, |key, raw, out| match key {
        "summary" => replace_keys(out, raw, Some(&SUMMARY_KEYS), &[("name", Some(name))]),
        "project" => replace_keys(out, raw, None, &[("name", Some(name))]),
        _ => {
            out.extend_from_slice(raw.get().as_bytes());
            Ok(())
        }
    })?;
    for key in ["speleodb", "local"] {
        copy(&mut out, &top, key);
    }
    write_body(&mut out, &top);
    copy(&mut out, &top, "history");
    out.extend_from_slice(b"\"end\":true}\n");
    Ok(out)
}

/// The text of `original` when it is a complete version 2 file.
fn complete_v2(original: &[u8]) -> Result<&str, EncodeError> {
    match sniff_detail(original) {
        Sniffed::V2(Some(token)) => match token.parse::<u64>() {
            Ok(VERSION) => {}
            Ok(found) if found > VERSION => {
                return Err(EncodeError::Original(DecodeError::NewerVersion {
                    found,
                    supported: VERSION,
                }))
            }
            _ => {
                return Err(EncodeError::Original(DecodeError::UnsupportedVersion {
                    found: token,
                }))
            }
        },
        Sniffed::Legacy => return Err(EncodeError::Original(DecodeError::Legacy)),
        Sniffed::ByteOrderMark => return Err(EncodeError::Original(DecodeError::ByteOrderMark)),
        Sniffed::V2(None) | Sniffed::Incomplete | Sniffed::NotStalx => {
            return Err(EncodeError::Original(DecodeError::NotStalx))
        }
    }
    std::str::from_utf8(original).map_err(|_| {
        EncodeError::Original(DecodeError::BadValue {
            at: "the file".to_string(),
            message: "not UTF-8 text".to_string(),
        })
    })
}

fn top_members(text: &str) -> Result<Vec<Member<'_>>, EncodeError> {
    let top = members(text, MAX_SMALL_MEMBERS)
        .map_err(|e| EncodeError::Original(e.into_decode("the file", text, text)))?;
    let end = top
        .iter()
        .find(|(k, _)| k == "end")
        .is_some_and(|(_, v)| v.get() == "true");
    if !end {
        return Err(EncodeError::Original(DecodeError::MissingEnd));
    }
    Ok(top)
}

fn find<'a>(top: &'a [Member<'a>], key: &str) -> Option<&'a RawValue> {
    top.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
}

/// `"key":<raw>,\n` when the original has the member.
fn copy(out: &mut Vec<u8>, top: &[Member<'_>], key: &str) {
    if let Some(raw) = find(top, key) {
        string(out, key);
        out.push(b':');
        out.extend_from_slice(raw.get().as_bytes());
        out.extend_from_slice(b",\n");
    }
}

/// The first line through `counters`, each member through `edit`.
fn write_header(
    out: &mut Vec<u8>,
    top: &[Member<'_>],
    mut edit: impl FnMut(&str, &RawValue, &mut Vec<u8>) -> Result<(), EncodeError>,
) -> Result<(), EncodeError> {
    let solver = find(top, "solver").ok_or_else(|| {
        EncodeError::Original(DecodeError::Missing {
            at: "the file".to_string(),
            field: "solver".to_string(),
        })
    })?;
    out.extend_from_slice(super::PREFIX.as_bytes());
    super::write::small(out, VERSION);
    out.extend_from_slice(b",\"solver\":");
    out.extend_from_slice(solver.get().as_bytes());
    out.extend_from_slice(b",\"requires\":");
    match find(top, "requires") {
        Some(raw) => out.extend_from_slice(raw.get().as_bytes()),
        None => out.extend_from_slice(b"[]"),
    }
    out.extend_from_slice(b",\n");
    for key in [
        "conventions",
        "generator",
        "summary",
        "project",
        "settings",
        "counters",
    ] {
        if let Some(raw) = find(top, key) {
            string(out, key);
            out.push(b':');
            edit(key, raw, out)?;
            out.extend_from_slice(b",\n");
        }
    }
    Ok(())
}

/// The survey members, then unknown top-level members in file order.
fn write_body(out: &mut Vec<u8>, top: &[Member<'_>]) {
    const BODY: [&str; 8] = [
        "sections",
        "stations",
        "lines",
        "datums",
        "placements",
        "closures",
        "ignored_warnings",
        "imports",
    ];
    for key in BODY {
        copy(out, top, key);
    }
    const KNOWN: [&str; 22] = [
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
    for (key, raw) in top {
        if !KNOWN.contains(&key.as_ref()) {
            string(out, key);
            out.push(b':');
            out.extend_from_slice(raw.get().as_bytes());
            out.extend_from_slice(b",\n");
        }
    }
}

/// The object `raw` with some keys' values replaced (`None` removes the key),
/// every other member verbatim: with `order`, its keys first in that order
/// and the others after in file order; else in file order. A key missing
/// from the original is added at its place in `order`, else at the end.
fn replace_keys(
    out: &mut Vec<u8>,
    raw: &RawValue,
    order: Option<&[&str]>,
    replace: &[(&str, Option<&str>)],
) -> Result<(), EncodeError> {
    let pairs = members(raw.get(), MAX_SMALL_MEMBERS).map_err(|e| {
        EncodeError::Original(e.into_decode("a header member", raw.get(), raw.get()))
    })?;
    let mut ordered: Vec<(&str, Option<&RawValue>)> = Vec::new();
    if let Some(order) = order {
        for &key in order {
            ordered.push((key, pairs.iter().find(|(k, _)| k == key).map(|(_, v)| *v)));
        }
        for (key, value) in &pairs {
            if !order.contains(&key.as_ref()) {
                ordered.push((key.as_ref(), Some(*value)));
            }
        }
    } else {
        for (key, value) in &pairs {
            ordered.push((key.as_ref(), Some(*value)));
        }
        for (key, _) in replace {
            if !pairs.iter().any(|(k, _)| k == key) {
                ordered.push((key, None));
            }
        }
    }
    out.push(b'{');
    let mut first = true;
    for (key, value) in ordered {
        let replacement = replace.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
        let text: Option<Vec<u8>> = match (replacement, value) {
            (Some(Some(new)), _) => {
                let mut b = Vec::new();
                string(&mut b, new);
                Some(b)
            }
            (Some(None), _) | (None, None) => None,
            (None, Some(v)) => Some(v.get().as_bytes().to_vec()),
        };
        let Some(text) = text else { continue };
        if !first {
            out.push(b',');
        }
        first = false;
        string(out, key);
        out.push(b':');
        out.extend_from_slice(&text);
    }
    out.push(b'}');
    Ok(())
}
