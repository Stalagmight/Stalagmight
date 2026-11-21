//! The survey digest: a fingerprint of the survey a file holds, kept in the
//! SpeleoDB sync state as `local.speleodb.base_survey` (the digest of the
//! BASE revision's survey).
//!
//! It answers one question: does this copy hold exactly the survey of the
//! revision it is based on? The dirty mark (`local.speleodb.unpushed`) is
//! made durable when an edit is accepted, the edit itself only by the next
//! save, so a crash or a "Don't save" in between leaves the mark over a body
//! that is still BASE's. The digest of the body on disk, compared with
//! BASE's, tells that apart from un-uploaded work.
//!
//! What it covers: every member an upload carries that is survey content,
//! hashed from the bytes as written (canonical form, `docs/file-format.md`
//! section 8): `requires`, `conventions`, `project` without `modified`,
//! `settings`, `counters`, `speleodb` without `endpoint` and `name`, the body
//! members (`sections` through `imports`) and any unknown top-level member.
//! Left out: `format`, `version`, `solver` and `generator` (which build wrote
//! it), `summary` (derived from the body), `local` (per machine), `history`
//! (cut per push), `end`, the edit time `project.modified` (an edit and the
//! edit back change it and nothing else) and the link's `endpoint` and
//! `name` (the app rewrites them from the session and the server's name
//! without an edit). The revision id the digest belongs to is hashed first,
//! so a digest kept over another BASE never matches.
//!
//! Byte-level on purpose: a link reader answers from the file without
//! decoding the survey (plan 6.1). Two files with the same survey written by
//! this encoder have the same bytes; anything else (another writer's layout,
//! ignores kept for shots only the history can bring back) reads as
//! different, which is the conservative answer: the copy keeps its mark.

use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};

use super::error::EncodeError;
use super::local::{complete_v2, replace_keys, top_members};

/// Members that are not survey content (see the module docs).
const NOT_SURVEY: [&str; 8] = [
    "format",
    "version",
    "solver",
    "generator",
    "summary",
    "local",
    "history",
    "end",
];

/// Keys left out of a member that is otherwise survey content.
const LEFT_OUT: [(&str, &[&str]); 2] = [
    ("project", &["modified"]),
    ("speleodb", &["endpoint", "name"]),
];

/// The version of the digest, written in front of the hex so a later rule
/// never compares equal to this one.
const PREFIX: &str = "sha256:";

/// The domain the hash is taken in.
const DOMAIN: &[u8] = b"stalagmight survey digest 1\n";

/// The digest of the survey in `bytes` (a complete version 2 file) for the
/// BASE revision `base` (see the module docs). Refused like the local-only
/// rewrite refuses: a legacy, truncated or malformed file has no digest.
pub fn survey_digest(bytes: &[u8], base: &str) -> Result<String, EncodeError> {
    let text = complete_v2(bytes)?;
    let top = top_members(text)?;
    let mut members: Vec<(&str, Vec<u8>)> = Vec::with_capacity(top.len());
    for (key, raw) in &top {
        let key = key.as_ref();
        if NOT_SURVEY.contains(&key) {
            continue;
        }
        let value = match LEFT_OUT.iter().find(|(k, _)| *k == key) {
            Some((_, drop)) => {
                let removed: Vec<(&str, Option<&str>)> = drop.iter().map(|k| (*k, None)).collect();
                let mut out = Vec::new();
                replace_keys(&mut out, raw, None, &removed)?;
                out
            }
            None => raw.get().as_bytes().to_vec(),
        };
        members.push((key, value));
    }
    // Keyed, not positional: the digest is a function of the members, so a
    // writer that orders unknown members differently still matches.
    members.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let mut hash = Sha256::new();
    hash.update(DOMAIN);
    field(&mut hash, base.as_bytes());
    for (key, value) in &members {
        field(&mut hash, key.as_bytes());
        field(&mut hash, value);
    }
    let mut out = String::with_capacity(PREFIX.len() + 64);
    out.push_str(PREFIX);
    for byte in hash.finalize() {
        // Writing to a String cannot fail.
        let _ = write!(out, "{byte:02x}");
    }
    Ok(out)
}

/// One length-prefixed field, so no two member lists hash alike.
fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}
