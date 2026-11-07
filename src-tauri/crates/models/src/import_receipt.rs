use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Backend-authoritative summary of what an import command actually did, so the
/// frontend can show a receipt without re-deriving counts the backend already
/// computed (and, for the converter-level warnings below, cannot derive at all).
///
/// These warnings are DISTINCT from `CommandResult::warnings` and must never be
/// pushed through `CommandResult::with_warning`: that path stamps `code:
/// "SYSTEM"`, which the frontend's deferred-save-failure handling
/// (`saveFeedback.ts`) keys on exactly - mixing receipt warnings into it would
/// make an unrelated import-quality note look like the deferred-save failure the
/// #92/#104 contract exists to surface.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ImportReceipt {
    pub source_format: String,
    pub file_name: String,
    pub sections_found: u32,
    pub sections_imported: u32,
    pub shot_count: u32,
    pub closures_requested: u32,
    pub closures_created: u32,
    pub unit_conversion: Option<String>,
    pub warnings: Vec<String>,
    /// Notice-level lines, distinct from `warnings`: they never change the
    /// receipt's toast-vs-dialog routing (`presentImportReceipt`,
    /// `saveFeedback.ts`), unlike a non-empty `warnings`. Currently used for
    /// the "N sections are not connected to a zero datum" line the import
    /// dialogs add when the orphan-placement pass found nothing to suggest
    /// (#446). `#[serde(default)]` so an older cached/replayed payload
    /// without this field still deserializes instead of rejecting the whole
    /// receipt.
    #[serde(default)]
    pub info: Vec<String>,
    /// Name of the section a zero datum was created on during this import
    /// (F12c), or `None` if no zero datum was created. `#[serde(default)]`
    /// for the same replay/cache back-compat reason as `info`.
    #[serde(default)]
    pub zero_datum_created: Option<String>,
    /// How many warnings [`Self::cap_warnings`] has folded into the summary
    /// line so far (#576 C17). A receipt is capped more than once - by the
    /// import, then again each time another source's warnings are put in
    /// front - and each cap must recognise the previous one's summary instead
    /// of counting it as a warning. Bookkeeping only: never sent over IPC.
    #[serde(skip)]
    #[ts(skip)]
    pub hidden_warnings: usize,
}

/// Make a file-controlled string fit to cross IPC and be rendered as UI text: drop
/// control characters and the Unicode bidi overrides (which can visually reorder the
/// sentence, or the dialog title, printed around them), then clamp to `max_chars` on
/// a char boundary - never a raw byte slice.
///
/// Every string this guards comes from a file this app was handed, and one of them
/// (the adoption's receipt `file_name`) comes from a zip entry inside an archive
/// downloaded from a server, so "untrusted" is literal rather than theoretical.
///
/// Lives here, beside [`ImportReceipt`], because the fields it guards are that
/// struct's: `parsers::tml::sanitize_unit_label` is one caller with its own clamp,
/// `decorate_receipt` (every importer, not just Ariane) is the other.
pub fn sanitize_display_text(raw: &str, max_chars: usize) -> String {
    raw.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}')
        })
        .take(max_chars)
        .collect()
}

impl ImportReceipt {
    /// The receipt for an import that landed NOTHING: zero sections, zero
    /// shots, no closures. Every text field is a placeholder the importer's
    /// `decorate_receipt` fills in, including the warnings that say why the
    /// file produced nothing - which is the entire reason an empty import
    /// still carries a receipt.
    pub fn empty() -> Self {
        Self {
            source_format: String::new(),
            file_name: String::new(),
            sections_found: 0,
            sections_imported: 0,
            shot_count: 0,
            closures_requested: 0,
            closures_created: 0,
            unit_conversion: None,
            warnings: Vec::new(),
            info: Vec::new(),
            zero_datum_created: None,
            hidden_warnings: 0,
        }
    }

    /// Keep at most this many individual warnings before folding the rest
    /// into a single summary entry.
    const MAX_WARNINGS: usize = 50;

    /// The entry standing in for `hidden` folded warnings.
    fn summary_line(hidden: usize) -> String {
        format!(
            "... and {hidden} more warning{}",
            if hidden == 1 { "" } else { "s" }
        )
    }
    /// Clamp each warning string to this many characters.
    const MAX_WARNING_LEN: usize = 300;

    /// Cap and clamp `warnings` so a hostile or malformed import file (this
    /// app is open-source, so an untrusted file is a real input, not a
    /// hypothetical) can't inflate the receipt into an unbounded allocation
    /// shipped back over IPC: at most [`Self::MAX_WARNINGS`] entries (the rest
    /// folded into one "... and N more warnings" summary), each clamped to
    /// [`Self::MAX_WARNING_LEN`] chars on a char boundary (never a raw byte
    /// slice - a warning can embed arbitrary user-controlled text).
    ///
    /// Capping again is safe (#576 C17): the summary an earlier cap left is
    /// taken out first and its count carried, so there is only ever one
    /// summary line and it counts everything hidden so far.
    pub fn cap_warnings(&mut self) {
        if self.hidden_warnings > 0 {
            let summary = Self::summary_line(self.hidden_warnings);
            if let Some(at) = self.warnings.iter().rposition(|w| *w == summary) {
                self.warnings.remove(at);
            }
        }
        if self.warnings.len() > Self::MAX_WARNINGS {
            self.hidden_warnings += self.warnings.len() - Self::MAX_WARNINGS;
            self.warnings.truncate(Self::MAX_WARNINGS);
        }
        if self.hidden_warnings > 0 {
            self.warnings.push(Self::summary_line(self.hidden_warnings));
        }
        for warning in &mut self.warnings {
            if let Some((cut, _)) = warning.char_indices().nth(Self::MAX_WARNING_LEN) {
                warning.truncate(cut);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ImportReceipt;

    /// #576 C17: a receipt is capped more than once - by the import itself,
    /// then again when the importer's or the deferred save's warnings are put
    /// in front. Each cap must account for what the earlier ones hid: one
    /// summary line, with the total.
    #[test]
    fn capping_twice_keeps_one_summary_with_the_total() {
        let mut receipt = ImportReceipt::empty();
        receipt.warnings = (0..60).map(|i| format!("shot {i} looks odd")).collect();
        receipt.cap_warnings();
        assert_eq!(
            receipt.warnings.last().map(String::as_str),
            Some("... and 10 more warnings")
        );

        let mut leading = vec!["save note 0".to_string(), "save note 1".to_string()];
        leading.append(&mut receipt.warnings);
        receipt.warnings = leading;
        receipt.cap_warnings();

        assert_eq!(receipt.warnings.len(), 51);
        assert_eq!(receipt.warnings[0], "save note 0");
        assert_eq!(
            receipt.warnings.last().map(String::as_str),
            Some("... and 12 more warnings"),
            "62 warnings, 50 shown"
        );
        assert_eq!(
            receipt
                .warnings
                .iter()
                .filter(|w| w.starts_with("... and "))
                .count(),
            1
        );

        // A cap with nothing new over the limit changes nothing.
        let before = receipt.warnings.clone();
        receipt.cap_warnings();
        assert_eq!(receipt.warnings, before);
    }
}
