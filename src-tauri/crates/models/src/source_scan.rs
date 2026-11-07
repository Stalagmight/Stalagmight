//! [#576 A8 / E9(b)] The one way a test reads production Rust source.
//!
//! A source scan is a test that `include_str!`s a production file and asserts
//! something about its text. The workspace keeps them only for a policy the type
//! system cannot express - "this call appears at exactly these sanctioned sites" -
//! and never for an ordering or for the presence of a log line (see
//! docs/testing-recipes.md, "Source scans"). Every scan goes through
//! [`prod_region`], which closes the failure mode the hand-rolled cuts kept
//! reopening: a cut on the BARE `#[cfg(test)]` string stops at the first
//! test-only `use`, indented test-only method, or comment that merely mentions
//! the attribute, and every absence assertion over the truncated text then
//! passes while guarding nothing.

/// The production region of a Rust source file - everything above its top-level
/// inline test module - after checking that `anchor` is in it.
///
/// The cut is the column-0 `#[cfg(test)]` that gates an INLINE module (`mod tests
/// {`, after any further attribute, comment or blank lines) and is followed by
/// nothing but test-gated items to the end of the file - the trailing test region.
/// A gated `use`, a gated method, an out-of-line `mod x_tests;` declaration, the
/// attribute's text inside a comment, or a test-only inline module with
/// production code after it (`repair.rs`'s fault injection) does not end
/// production. A file with no such region is production to the end.
///
/// `anchor` is text the scan's subject must contain - the signature of the
/// function it slices, or a line known to sit near the end of the file. Its
/// absence fails the calling test: the subject was renamed, moved out of the
/// file, or cut off, and a scan that cannot see its subject must fail rather
/// than pass vacuously.
///
/// # Panics
///
/// When `anchor` is not in the production region (that is the point).
#[track_caller]
pub fn prod_region<'a>(src: &'a str, anchor: &str) -> &'a str {
    let end = test_module_start(src).unwrap_or(src.len());
    let region = src.get(..end).unwrap_or(src);
    assert!(
        region.contains(anchor),
        "source scan: the anchor `{anchor}` is not in the production region ({end} of \
         {} bytes, cut at the top-level test module). The scanned code was renamed or \
         moved, or something now ends the region above it - fix the scan; a scan that \
         cannot see its subject guards nothing",
        src.len()
    );
    region
}

/// Byte offset of the line that starts the file's trailing test region, if it has
/// one. See [`prod_region`] for what counts.
pub fn test_module_start(src: &str) -> Option<usize> {
    let lines: Vec<&str> = src.split_inclusive('\n').collect();
    let mut offset = 0;
    for (at, line) in lines.iter().enumerate() {
        if line.trim_end() == "#[cfg(test)]" {
            if let Some((item, true)) = gated_item(&lines, at) {
                if only_gated_items_from(&lines, item_end(&lines, item) + 1) {
                    return Some(offset);
                }
            }
        }
        offset += line.len();
    }
    None
}

/// The item a column-0 attribute line at `gate` applies to - past any further
/// attribute lines (bracket-balanced: lint waivers run over several lines),
/// comments and blank lines - and whether that item is an INLINE module (a
/// declaration, `mod x_tests;`, is not: its body is another file).
fn gated_item(lines: &[&str], gate: usize) -> Option<(usize, bool)> {
    let bracket_delta = |line: &str| -> i64 {
        let opens = i64::try_from(line.matches('[').count()).unwrap_or(0);
        let closes = i64::try_from(line.matches(']').count()).unwrap_or(0);
        opens - closes
    };
    let mut depth = bracket_delta(lines.get(gate)?);
    for (at, line) in lines.iter().enumerate().skip(gate + 1) {
        if depth > 0 {
            depth += bracket_delta(line);
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if trimmed.starts_with("#[") {
            depth += bracket_delta(line);
            continue;
        }
        let is_mod = line.starts_with("mod ")
            || line.starts_with("pub mod ")
            || line.starts_with("pub(crate) mod ")
            || line.starts_with("pub(super) mod ");
        return Some((at, is_mod && !line.trim_end().ends_with(';')));
    }
    None
}

/// The last line of the item starting at `item`: its column-0 closing brace when
/// the item opens a block (rustfmt layout), else the item line itself.
fn item_end(lines: &[&str], item: usize) -> usize {
    let opens_block = lines.get(item).is_some_and(|l| l.trim_end().ends_with('{'));
    if !opens_block {
        return item;
    }
    lines
        .iter()
        .enumerate()
        .skip(item + 1)
        .find(|(_, l)| l.starts_with('}'))
        .map_or(lines.len(), |(at, _)| at)
}

/// Whether everything from line `from` to the end is test-gated items, blank
/// lines and comments - i.e. no production follows.
fn only_gated_items_from(lines: &[&str], from: usize) -> bool {
    let mut at = from;
    while let Some(line) = lines.get(at) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            at += 1;
            continue;
        }
        if line.trim_end() != "#[cfg(test)]" {
            return false;
        }
        let Some((item, _)) = gated_item(lines, at) else {
            return true;
        };
        at = item_end(lines, item) + 1;
    }
    true
}

/// The text of the one function in `region` whose signature contains
/// `signature`: from the signature to the closing brace at the signature's own
/// indentation (rustfmt layout), both included.
///
/// `signature` must name exactly one item in `region` (pass `"fn name("`, not
/// a prefix another function shares). A second match - an overload in another
/// `impl`, a doc comment quoting the signature - fails the test instead of
/// letting the scan silently read the wrong body.
///
/// # Panics
///
/// When `signature` does not occur exactly once, or its body has no closing
/// brace at the signature's indentation.
#[track_caller]
pub fn fn_body<'a>(region: &'a str, signature: &str) -> &'a str {
    let hits: Vec<usize> = region.match_indices(signature).map(|(at, _)| at).collect();
    assert!(
        hits.len() == 1,
        "source scan: `{signature}` must occur exactly once in the scanned region, found \
         {} - name the function unambiguously",
        hits.len()
    );
    let at = hits.first().copied().unwrap_or(0);
    let line_start = region
        .get(..at)
        .and_then(|s| s.rfind('\n'))
        .map_or(0, |n| n + 1);
    let indent: String = region
        .get(line_start..at)
        .unwrap_or("")
        .chars()
        .take_while(|c| *c == ' ')
        .collect();
    let close = format!("\n{indent}}}\n");
    let rest = region.get(at..).unwrap_or("");
    let end = rest.find(&close);
    assert!(
        end.is_some(),
        "source scan: `{signature}` has no closing brace at its own indentation - the \
         file is not rustfmt-formatted, or the signature is not an item's"
    );
    let end = end.unwrap_or(rest.len());
    rest.get(..end + close.len() - 1).unwrap_or(rest)
}

/// [#576 RV9a-9] `src` with every comment and literal blanked to spaces, so a
/// scan that looks for CODE (a call, a path) is not answered by prose or data.
///
/// Blanks `//` comments, `/* */` comments (nested, as Rust nests them), string
/// literals with their escapes, raw strings (`r"..."`, `r#"..."#`, any number of
/// `#`), the byte forms of both (`b"..."`, `br#"..."#`), and char and byte
/// literals (`'"'`, `'\''`, `b'{'`) - a lifetime (`'a`) is code and is kept.
/// Every blanked character becomes as many spaces as it has UTF-8 bytes and every
/// newline stays, so byte offsets and line numbers still line up with `src`.
#[must_use]
pub fn code_only(src: &str) -> String {
    fn blank(out: &mut String, c: char) {
        if c == '\n' {
            out.push('\n');
        } else {
            out.push_str(&" ".repeat(c.len_utf8()));
        }
    }
    let chars: Vec<char> = src.chars().collect();
    let at = |i: usize| chars.get(i).copied();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while let Some(c) = at(i) {
        let prev_ident = i > 0 && at(i - 1).is_some_and(|p| p.is_alphanumeric() || p == '_');
        // A `//` comment, to the end of the line.
        if c == '/' && at(i + 1) == Some('/') {
            while let Some(c) = at(i) {
                if c == '\n' {
                    break;
                }
                blank(&mut out, c);
                i += 1;
            }
            continue;
        }
        // A `/* */` comment, nested.
        if c == '/' && at(i + 1) == Some('*') {
            let mut depth = 0usize;
            while let Some(c) = at(i) {
                if c == '/' && at(i + 1) == Some('*') {
                    depth += 1;
                    blank(&mut out, '/');
                    blank(&mut out, '*');
                    i += 2;
                } else if c == '*' && at(i + 1) == Some('/') {
                    depth -= 1;
                    blank(&mut out, '*');
                    blank(&mut out, '/');
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    blank(&mut out, c);
                    i += 1;
                }
            }
            continue;
        }
        // A raw (byte) string: `r`/`br`, any `#`s, then `"`.
        if !prev_ident && (c == 'r' || (c == 'b' && at(i + 1) == Some('r'))) {
            let mut j = if c == 'b' { i + 2 } else { i + 1 };
            let mut hashes = 0usize;
            while at(j) == Some('#') {
                hashes += 1;
                j += 1;
            }
            if at(j) == Some('"') {
                let close: Vec<char> = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                let mut k = j + 1;
                while k < chars.len() && chars.get(k..k + close.len()) != Some(&close[..]) {
                    k += 1;
                }
                let end = (k + close.len()).min(chars.len());
                for &c in chars.get(i..end).unwrap_or_default() {
                    blank(&mut out, c);
                }
                i = end;
                continue;
            }
        }
        // A (byte) string with escapes.
        if c == '"' || (c == 'b' && !prev_ident && at(i + 1) == Some('"')) {
            let start = if c == 'b' { i + 1 } else { i };
            for k in i..=start {
                blank(&mut out, at(k).unwrap_or(' '));
            }
            let mut k = start + 1;
            while let Some(c) = at(k) {
                blank(&mut out, c);
                k += 1;
                if c == '\\' {
                    if let Some(escaped) = at(k) {
                        blank(&mut out, escaped);
                        k += 1;
                    }
                } else if c == '"' {
                    break;
                }
            }
            i = k;
            continue;
        }
        // A char (or byte) literal - not a lifetime: `'x'`, `'\n'`, `'\''`, `'\u{..}'`.
        if c == '\'' || (c == 'b' && !prev_ident && at(i + 1) == Some('\'')) {
            let q = if c == 'b' { i + 1 } else { i };
            let end = if at(q + 1) == Some('\\') {
                (q + 3..chars.len()).find(|&k| at(k) == Some('\''))
            } else if at(q + 2) == Some('\'') {
                Some(q + 2)
            } else {
                None
            };
            if let Some(end) = end {
                for &c in chars.get(i..=end).unwrap_or_default() {
                    blank(&mut out, c);
                }
                i = end + 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{code_only, fn_body, prod_region, test_module_start};

    /// A file whose production region holds every way the attribute's TEXT can
    /// appear without ending production, then the real module.
    const SAMPLE: &str = "\
use std::path::Path;

#[cfg(test)]
use std::fmt as _probe;

#[cfg(test)]
pub(super) mod fault {
    pub(crate) fn inject() {}
}

// A comment that quotes #[cfg(test)] mid-line.
pub fn first() {
    let _ = 1;
}

impl Thing {
    #[cfg(test)]
    fn test_only_method(&self) {}

    pub fn second(&self) -> u8 {
        if true {
            return 1;
        }
        2
    }
}

#[cfg(test)]
mod declared_tests;

pub fn last_production_item() {}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = \"a multi-line [waiver] like the crate's own\"
)]
// and a comment between the waiver and the module
mod tests {
    fn in_the_test_module() {}
}
";

    #[test]
    fn code_only_blanks_comments_and_literals_and_keeps_code_and_offsets() {
        let src = "call(1); // call(2)\n\
                   /* call(3) /* nested call(4) */ still call(5) */ call(6);\n\
                   let s = \"call(7) \\\" call(8)\"; let r = r#\"call(9) \" call(10)\"#;\n\
                   let b = b\"call(11)\"; let br = br\"call(12)\";\n\
                   let q = '\"'; let e = '\\''; let l: &'a str = x; call(13);\n";
        let code = code_only(src);
        assert_eq!(code.len(), src.len(), "byte offsets line up");
        assert_eq!(code.lines().count(), src.lines().count(), "lines line up");
        let kept: Vec<&str> = ["call(1)", "call(6)", "call(13)", "&'a str"]
            .into_iter()
            .filter(|needle| code.contains(needle))
            .collect();
        assert_eq!(
            kept,
            ["call(1)", "call(6)", "call(13)", "&'a str"],
            "{code}"
        );
        for blanked in (2..=12).filter(|n| *n != 6) {
            assert!(
                !code.contains(&format!("call({blanked})")),
                "call({blanked}) is in a comment or a literal: {code}"
            );
        }
    }

    #[test]
    fn the_cut_lands_at_the_inline_test_module_and_nowhere_earlier() {
        let region = prod_region(SAMPLE, "pub fn last_production_item()");
        assert!(region.contains("pub fn first()"));
        assert!(
            region.contains("mod fault"),
            "a test-only module with production after it is not the test region"
        );
        assert!(region.contains("fn test_only_method"));
        assert!(region.contains("pub fn second"));
        assert!(
            region.contains("mod declared_tests;"),
            "an out-of-line declaration is not the test module"
        );
        assert!(
            !region.contains("in_the_test_module"),
            "the inline test module is cut off"
        );
        assert!(region
            .trim_end()
            .ends_with("pub fn last_production_item() {}"));
    }

    #[test]
    fn a_file_without_a_test_module_is_production_to_the_end() {
        let src = "pub fn a() {}\n#[cfg(test)]\nfn helper() {}\n";
        assert_eq!(test_module_start(src), None);
        assert_eq!(prod_region(src, "fn helper"), src);
    }

    #[test]
    #[should_panic(expected = "is not in the production region")]
    fn an_anchor_cut_off_by_the_test_module_fails_the_scan() {
        // The vacuous-pass case: the subject sits in (or below) the test module.
        let _ = prod_region(SAMPLE, "fn in_the_test_module");
    }

    #[test]
    fn fn_body_takes_one_whole_function_at_its_own_indentation() {
        let region = prod_region(SAMPLE, "pub fn second");
        let body = fn_body(region, "pub fn second(");
        assert!(body.starts_with("pub fn second("));
        assert!(
            body.ends_with("\n    }"),
            "ends at the method's own brace: {body:?}"
        );
        assert!(body.contains("return 1;"), "an inner brace does not end it");
        assert!(!body.contains("last_production_item"));

        let free = fn_body(region, "pub fn first(");
        assert!(free.ends_with("\n}"));
        assert!(!free.contains("impl Thing"));
    }

    #[test]
    #[should_panic(expected = "must occur exactly once")]
    fn an_ambiguous_signature_fails_the_scan() {
        // `fn s` prefixes `fn second` and `fn test_only_method`'s neighbours alike.
        let _ = fn_body(SAMPLE, "fn ");
    }
}
