//! Typed refusals of the encoder and the decoder. Every variant names what is
//! wrong and where, in words a user (or a third-party writer) can act on.

use std::fmt;

use crate::models::survey::Violation;
use crate::models::StationId;

use super::types::{Limit, ReadOnly};

/// Why a file was not decoded.
#[derive(Debug, Clone, PartialEq)]
pub enum DecodeError {
    /// Neither a version 2 file nor a legacy SQLite file.
    NotStalx,
    /// A legacy SQLite `.stalx`: it opens through the migration.
    Legacy,
    /// The file starts with a UTF-8 byte order mark; the format has none.
    ByteOrderMark,
    /// Larger than [`super::DecodeOptions::max_file_bytes`].
    TooLarge { size: u64, max: u64 },
    /// Written by a newer Stalagmight (a higher major version).
    NewerVersion { found: u64, supported: u64 },
    /// A version this build never wrote (below 2, or not an integer).
    UnsupportedVersion { found: String },
    /// `conventions` is not the fixed version 2 object.
    Conventions { key: String, found: String },
    /// Over a [`super::CountLimits`] cap.
    TooComplex { limit: Limit, max: usize },
    /// The bytes end before the document does: a torn or truncated file.
    Truncated { line: usize, column: usize },
    /// Not well-formed JSON at this place (a `NaN` literal, a stray byte...).
    Syntax {
        at: String,
        line: usize,
        column: usize,
        message: String,
    },
    /// A member or field has the wrong type or an out-of-range value.
    BadValue { at: String, message: String },
    /// A required member or field is missing.
    Missing { at: String, field: String },
    /// A key appears twice in one object.
    DuplicateKey { at: String, key: String },
    /// Nesting deeper than [`super::MAX_DEPTH`] in preserved content.
    TooDeep { at: String },
    /// No `"end":true` member: the file is not complete.
    MissingEnd,
    /// Two station records share an id.
    DuplicateStation { id: StationId },
    /// The survey breaks the model invariants (plan section 2).
    Invalid { violations: Vec<Violation> },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotStalx => f.write_str("this is not a Stalagmight project file"),
            Self::Legacy => f.write_str(
                "this is a project file in the old (SQLite) format; it opens through the \
                 conversion",
            ),
            Self::ByteOrderMark => {
                f.write_str("the file starts with a byte order mark, which a .stalx file never has")
            }
            Self::TooLarge { size, max } => {
                write!(f, "the file is {size} bytes, over the {max}-byte limit")
            }
            Self::NewerVersion { found, supported } => write!(
                f,
                "the file was made with a newer Stalagmight (format version {found}; this \
                 version reads {supported})"
            ),
            Self::UnsupportedVersion { found } => {
                write!(
                    f,
                    "format version {found} is not one Stalagmight ever wrote"
                )
            }
            Self::Conventions { key, found } => write!(
                f,
                "the file declares conventions this version does not use ({key}: {found})"
            ),
            Self::TooComplex { limit, max } => {
                write!(f, "the file holds more than {max} {limit}")
            }
            Self::Truncated { line, column } => write!(
                f,
                "the file ends early (line {line}, column {column}): it is incomplete"
            ),
            Self::Syntax {
                at,
                line,
                column,
                message,
            } => write!(
                f,
                "the file is malformed in {at} (line {line}, column {column}): {message}"
            ),
            Self::BadValue { at, message } => write!(f, "{at}: {message}"),
            Self::Missing { at, field } => write!(f, "{at} has no {field}"),
            Self::DuplicateKey { at, key } => write!(f, "{at} has the key {key:?} twice"),
            Self::TooDeep { at } => write!(f, "{at} is nested too deeply"),
            Self::MissingEnd => f.write_str(
                "the file has no end marker: it is incomplete or was not written by Stalagmight",
            ),
            Self::DuplicateStation { id } => write!(f, "two stations have id {id}"),
            Self::Invalid { violations } => {
                write!(f, "the survey is inconsistent")?;
                for (i, violation) in violations.iter().take(5).enumerate() {
                    write!(f, "{}{violation}", if i == 0 { ": " } else { "; " })?;
                }
                if violations.len() > 5 {
                    write!(f, " (and {} more)", violations.len() - 5)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// Why a project was not encoded. Nothing was written.
#[derive(Debug, Clone, PartialEq)]
pub enum EncodeError {
    /// The model breaks its invariants; the in-memory state is kept.
    Invalid { violations: Vec<Violation> },
    /// A number has no JSON form.
    NonFinite { at: String, value: f64 },
    /// An integer is above [`super::MAX_INTEGER`].
    IntegerTooLarge { at: String, value: u64 },
    /// The project was opened read-only; only a local-only rewrite is allowed.
    ReadOnly(ReadOnly),
    /// `requires` names a feature this build does not know.
    UnknownFeature { name: String },
    /// A push does not fit its byte cap even without history.
    TooLarge { size: usize, cap: usize },
    /// The history cannot be cut at the watermark (unreadable or out-of-order
    /// entry times): a push refuses rather than ship inherited entries.
    History { reason: String },
    /// A local-only rewrite was given bytes that are not a complete version 2
    /// file.
    Original(DecodeError),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { violations } => {
                write!(f, "the project cannot be saved: it is inconsistent")?;
                for (i, violation) in violations.iter().take(5).enumerate() {
                    write!(f, "{}{violation}", if i == 0 { ": " } else { "; " })?;
                }
                if violations.len() > 5 {
                    write!(f, " (and {} more)", violations.len() - 5)?;
                }
                Ok(())
            }
            Self::NonFinite { at, value } => {
                write!(f, "{at} is {value}, which a .stalx file cannot hold")
            }
            Self::IntegerTooLarge { at, value } => {
                write!(
                    f,
                    "{at} is {value}, above the largest integer a .stalx file holds"
                )
            }
            Self::ReadOnly(reason) => write!(f, "the project is read-only: {reason}"),
            Self::UnknownFeature { name } => write!(
                f,
                "the project uses the feature {name:?}, which this version cannot save"
            ),
            Self::TooLarge { size, cap } => write!(
                f,
                "the project is {size} bytes without its history, over the {cap}-byte limit"
            ),
            Self::History { reason } => {
                write!(f, "the undo history cannot be cut for the upload: {reason}")
            }
            Self::Original(error) => write!(f, "the saved file cannot be rewritten: {error}"),
        }
    }
}

impl std::error::Error for EncodeError {}
