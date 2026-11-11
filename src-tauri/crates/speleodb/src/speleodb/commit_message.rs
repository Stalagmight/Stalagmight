//! The longest upload description SpeleoDB can store (sdk#2).
//!
//! SpeleoDB records every revision's description in `ProjectCommit.message`, a
//! `CharField(max_length=1024)`, but only AFTER it has pushed the git commit. A
//! longer description lands in git, then fails the database insert (HTTP 500),
//! and every later upload of the project rebuilds that history row and fails the
//! same way until a server admin steps in. So the client refuses a description
//! that would not fit, with some headroom below the column.
//!
//! The length that matters is the one the server STORES. Its upload view strips
//! the description and turns every run of line breaks, together with the
//! whitespace around them, into ". "
//! (`re.sub(r"(?:\s*\n\s*)+", ". ", message.strip())`). A single line break
//! therefore costs two characters, not one, and the limit is measured on the
//! normalised text, in characters (Unicode scalar values), as Postgres counts
//! them.
//!
//! The frontend mirrors this in `src/utils/commitMessage.ts`; keep them in step.

/// The longest description, in characters after the server's normalisation,
/// that an upload may carry. The server column holds 1024; the rest is headroom.
pub const MAX_COMMIT_MESSAGE_CHARS: usize = 1000;

/// Why a description cannot be sent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommitMessageError {
    /// Nothing but whitespace.
    #[error("the upload description must not be empty")]
    Empty,
    /// Longer than [`MAX_COMMIT_MESSAGE_CHARS`] once normalised like the server.
    #[error(
        "the upload description is too long: {length} characters, and SpeleoDB accepts at most {max}. Line breaks count as two characters. Shorten it and try again."
    )]
    TooLong { length: usize, max: usize },
}

/// Python's `str.isspace`, which is what the server's `strip()` and `\s` use:
/// Unicode `White_Space` plus the four information separators U+001C..U+001F.
fn is_server_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// The description exactly as the server will store it: stripped, with every
/// whitespace run that contains a line feed replaced by ". ".
///
/// The server's pattern `(?:\s*\n\s*)+` matches, at the first whitespace
/// character of a run, the WHOLE run whenever it holds at least one `\n` (the
/// greedy `\s*` swallows what precedes the last line feed, the trailing `\s*`
/// what follows it). Runs without a line feed (spaces, tabs, a lone `\r`) are
/// left as they are.
pub fn normalize_like_server(message: &str) -> String {
    let trimmed = message.trim_matches(is_server_space);
    let mut out = String::with_capacity(trimmed.len());
    let mut run = String::new();
    for c in trimmed.chars() {
        if is_server_space(c) {
            run.push(c);
            continue;
        }
        if run.contains('\n') {
            out.push_str(". ");
        } else {
            out.push_str(&run);
        }
        run.clear();
        out.push(c);
    }
    // `trimmed` ends on a non-space character, so no run is left over.
    out
}

/// The stored length of `message`, in characters.
pub fn stored_length(message: &str) -> usize {
    normalize_like_server(message).chars().count()
}

/// Refuse a description the server could not store (or that is empty).
pub fn check_commit_message(message: &str) -> Result<(), CommitMessageError> {
    let length = stored_length(message);
    if length == 0 {
        return Err(CommitMessageError::Empty);
    }
    if length > MAX_COMMIT_MESSAGE_CHARS {
        return Err(CommitMessageError::TooLong {
            length,
            max: MAX_COMMIT_MESSAGE_CHARS,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_breaks_become_a_full_stop_and_a_space() {
        assert_eq!(
            normalize_like_server("north passage\nsump 2"),
            "north passage. sump 2"
        );
        // A run of breaks with whitespace around them collapses to ONE separator.
        assert_eq!(normalize_like_server("a  \r\n\n \t b"), "a. b");
        assert_eq!(normalize_like_server("a\n\n\nb\nc"), "a. b. c");
    }

    #[test]
    fn whitespace_without_a_line_feed_is_kept() {
        assert_eq!(normalize_like_server("a \t b"), "a \t b");
        // A lone carriage return is not a line feed to the server's `\n`.
        assert_eq!(normalize_like_server("a\rb"), "a\rb");
    }

    #[test]
    fn the_ends_are_stripped_like_python() {
        assert_eq!(normalize_like_server("\n\n  survey \n"), "survey");
        // U+001F is whitespace to Python's strip(), not to Rust's trim().
        assert_eq!(normalize_like_server("\u{1f}x\u{1f}"), "x");
        assert_eq!(normalize_like_server(" \n\t "), "");
    }

    #[test]
    fn length_is_counted_in_characters_not_bytes() {
        assert_eq!(stored_length("caf\u{e9} \u{1f30a}"), 6);
    }

    #[test]
    fn a_single_line_break_costs_two_characters() {
        // 999 visible characters and one line break store as 1001.
        let message = format!("{}\n{}", "a".repeat(500), "b".repeat(499));
        assert_eq!(message.chars().count(), 1000);
        assert_eq!(stored_length(&message), 1001);
        assert_eq!(
            check_commit_message(&message),
            Err(CommitMessageError::TooLong {
                length: 1001,
                max: MAX_COMMIT_MESSAGE_CHARS
            })
        );
    }

    #[test]
    fn the_limit_itself_is_accepted() {
        assert_eq!(
            check_commit_message(&"x".repeat(MAX_COMMIT_MESSAGE_CHARS)),
            Ok(())
        );
        // Surrounding whitespace is stripped before counting.
        let padded = format!("\n  {}  \n", "x".repeat(MAX_COMMIT_MESSAGE_CHARS));
        assert_eq!(check_commit_message(&padded), Ok(()));
        assert!(check_commit_message(&"x".repeat(MAX_COMMIT_MESSAGE_CHARS + 1)).is_err());
    }

    #[test]
    fn blank_is_empty() {
        assert_eq!(check_commit_message(" \n "), Err(CommitMessageError::Empty));
    }

    #[test]
    fn the_message_names_the_numbers() {
        let text = CommitMessageError::TooLong {
            length: 1200,
            max: 1000,
        }
        .to_string();
        assert!(text.contains("1200") && text.contains("1000"));
    }
}
