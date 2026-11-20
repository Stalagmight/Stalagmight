//! Canonical text of a JSON value the format keeps without interpreting it
//! (preserved unknown content, history entries): insignificant whitespace
//! removed, strings re-escaped the canonical way, numbers, literals and key
//! order exactly as written.
//!
//! Iterative, so a deeply nested hostile value cannot overflow the stack; the
//! depth is capped at [`MAX_DEPTH`] all the same.

use super::write::string;
use super::MAX_DEPTH;

/// Why a value could not be canonicalised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CanonError {
    TooDeep,
    Invalid(String),
}

impl CanonError {
    pub(super) fn message(&self) -> String {
        match self {
            Self::TooDeep => format!("nested deeper than {MAX_DEPTH} levels"),
            Self::Invalid(message) => message.clone(),
        }
    }
}

/// The canonical text of `text`, one well-formed JSON value (as a
/// `serde_json::value::RawValue` holds it).
pub(super) fn canonical(text: &str) -> Result<String, String> {
    canonical_checked(text).map_err(|e| e.message())
}

pub(super) fn canonical_checked(text: &str) -> Result<String, CanonError> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut depth = 0usize;
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match b {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'{' | b'[' => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(CanonError::TooDeep);
                }
                out.push(b);
                i += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                out.push(b);
                i += 1;
            }
            b',' | b':' => {
                out.push(b);
                i += 1;
            }
            b'"' => {
                let end = string_end(bytes, i)
                    .ok_or_else(|| CanonError::Invalid("an unterminated string".to_string()))?;
                let token = text
                    .get(i..end)
                    .ok_or_else(|| CanonError::Invalid("a string is not UTF-8".to_string()))?;
                if token.as_bytes().contains(&b'\\') {
                    let decoded: String = serde_json::from_str(token)
                        .map_err(|e| CanonError::Invalid(e.to_string()))?;
                    string(&mut out, &decoded);
                } else {
                    out.extend_from_slice(token.as_bytes());
                }
                i = end;
            }
            _ => {
                let start = i;
                while let Some(&c) = bytes.get(i) {
                    if matches!(
                        c,
                        b' ' | b'\t'
                            | b'\n'
                            | b'\r'
                            | b','
                            | b':'
                            | b'{'
                            | b'}'
                            | b'['
                            | b']'
                            | b'"'
                    ) {
                        break;
                    }
                    i += 1;
                }
                out.extend_from_slice(&bytes[start..i]);
            }
        }
    }
    String::from_utf8(out).map_err(|e| CanonError::Invalid(e.to_string()))
}

/// The index just past the closing quote of the string that opens at `start`.
fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}
