//! Splitting an object into its members without interpreting them: ordered
//! `(key, raw value)` pairs borrowed from the input, duplicate keys refused.
//! The decoder reads the top level this way and then each member with its own
//! parser; the local-only rewrite copies members verbatim from it.

use std::borrow::Cow;
use std::fmt;

use serde::de::{self, Deserializer as _, MapAccess, Visitor};
use serde_json::value::RawValue;

use super::error::DecodeError;

/// The most members a small object (a header member, a record) may have:
/// its known keys plus the preserved-content cap.
pub(super) const MAX_SMALL_MEMBERS: usize = 10_100;

/// One member: its key and its value's text, borrowed from the input.
pub(super) type Member<'a> = (Cow<'a, str>, &'a RawValue);

/// Why an object could not be split.
pub(super) enum ScanError {
    /// Not an object, or not well-formed JSON (serde's error).
    Json(serde_json::Error),
    DuplicateKey(String),
    /// More members than the caller allows.
    TooMany(usize),
}

impl ScanError {
    /// The typed decode error, `at` naming the object, positions made
    /// absolute in `whole` (the text `object` is a slice of).
    pub(super) fn into_decode(self, at: &str, whole: &str, object: &str) -> DecodeError {
        match self {
            Self::DuplicateKey(key) => DecodeError::DuplicateKey {
                at: at.to_string(),
                key,
            },
            Self::Json(error) => json_error(error, at, whole, object),
            Self::TooMany(max) => DecodeError::TooComplex {
                limit: super::types::Limit::Preserved,
                max,
            },
        }
    }
}

/// A serde error raised while parsing `part` (a slice of `whole`), as a typed
/// decode error with absolute line and column.
pub(super) fn json_error(error: serde_json::Error, at: &str, whole: &str, part: &str) -> DecodeError {
    let (line, column) = absolute(whole, part, error.line(), error.column());
    match error.classify() {
        serde_json::error::Category::Eof => DecodeError::Truncated { line, column },
        serde_json::error::Category::Syntax | serde_json::error::Category::Io => {
            DecodeError::Syntax {
                at: at.to_string(),
                line,
                column,
                message: strip_position(&error.to_string()),
            }
        }
        serde_json::error::Category::Data => DecodeError::BadValue {
            at: at.to_string(),
            message: strip_position(&error.to_string()),
        },
    }
}

/// serde_json appends " at line L column C" (relative to the slice it read);
/// the typed error carries the absolute position instead.
fn strip_position(message: &str) -> String {
    match message.rfind(" at line ") {
        Some(cut) => message[..cut].to_string(),
        None => message.to_string(),
    }
}

/// Line and column (1-based) in `whole` of a position given relative to
/// `part`, a slice of `whole`.
pub(super) fn absolute(whole: &str, part: &str, line: usize, column: usize) -> (usize, usize) {
    let offset = (part.as_ptr() as usize).saturating_sub(whole.as_ptr() as usize);
    let Some(before) = whole.get(..offset.min(whole.len())) else {
        return (line, column);
    };
    let base_line = before.bytes().filter(|&b| b == b'\n').count();
    let base_column = before.len() - before.rfind('\n').map_or(0, |i| i + 1);
    if line <= 1 {
        (base_line + 1, base_column + column)
    } else {
        (base_line + line, column)
    }
}

/// The members of the object `text` holds, in file order, at most `max`.
pub(super) fn members(text: &str, max: usize) -> Result<Vec<Member<'_>>, ScanError> {
    let mut de = serde_json::Deserializer::from_str(text);
    let mut too_many = false;
    let pairs = de.deserialize_map(MembersVisitor {
        max,
        too_many: &mut too_many,
    });
    if too_many {
        return Err(ScanError::TooMany(max));
    }
    let pairs = pairs.map_err(ScanError::Json)?;
    de.end().map_err(ScanError::Json)?;
    let mut seen = std::collections::BTreeSet::new();
    for (key, _) in &pairs {
        if !seen.insert(key.as_ref()) {
            return Err(ScanError::DuplicateKey(key.to_string()));
        }
    }
    Ok(pairs)
}

struct MembersVisitor<'f> {
    max: usize,
    too_many: &'f mut bool,
}

impl<'de> Visitor<'de> for MembersVisitor<'_> {
    type Value = Vec<Member<'de>>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some(key) = map.next_key::<Key<'de>>()? {
            if out.len() >= self.max {
                *self.too_many = true;
                return Err(de::Error::custom("too many members"));
            }
            let value: &'de RawValue = map.next_value()?;
            out.push((key.0, value));
        }
        Ok(out)
    }
}

/// A key, borrowed when it has no escapes.
pub(super) struct Key<'a>(pub(super) Cow<'a, str>);

impl<'de> de::Deserialize<'de> for Key<'de> {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_str(KeyVisitor)
    }
}

struct KeyVisitor;

impl<'de> Visitor<'de> for KeyVisitor {
    type Value = Key<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a key")
    }

    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Key<'de>, E> {
        Ok(Key(Cow::Borrowed(v)))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Key<'de>, E> {
        Ok(Key(Cow::Owned(v.to_string())))
    }
}
