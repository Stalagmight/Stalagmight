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
    /// One line per section this import lost or could not connect (#814):
    /// sections the project refused and left out, and sections imported
    /// without their placement. Apart from `warnings` because they are the
    /// lines the user most needs: the receipt shows every one, above the
    /// warnings, and never folds them behind Details. `None` when there are
    /// none; optional on the wire so every existing receipt stays valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub section_problems: Option<Vec<String>>,
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
            section_problems: None,
        }
    }

    /// Keep at most this many individual warnings before folding the rest
    /// into a single summary entry.
    const MAX_WARNINGS: usize = 50;
    /// Clamp each warning string to this many characters.
    const MAX_WARNING_LEN: usize = 300;

    /// Cap and clamp `warnings` so a hostile or malformed import file (this
    /// app is open-source, so an untrusted file is a real input, not a
    /// hypothetical) can't inflate the receipt into an unbounded allocation
    /// shipped back over IPC: at most [`Self::MAX_WARNINGS`] entries (the rest
    /// folded into one "... and N more warnings" summary), each clamped to
    /// [`Self::MAX_WARNING_LEN`] chars on a char boundary (never a raw byte
    /// slice - a warning can embed arbitrary user-controlled text).
    pub fn cap_warnings(&mut self) {
        if self.warnings.len() > Self::MAX_WARNINGS {
            let overflow = self.warnings.len() - Self::MAX_WARNINGS;
            self.warnings.truncate(Self::MAX_WARNINGS);
            self.warnings.push(format!(
                "... and {overflow} more warning{}",
                if overflow == 1 { "" } else { "s" }
            ));
        }
        for warning in &mut self.warnings {
            if let Some((cut, _)) = warning.char_indices().nth(Self::MAX_WARNING_LEN) {
                warning.truncate(cut);
            }
        }
        // The same bounds for the per-section lines, which a hostile file can
        // multiply just as easily (one per section).
        if let Some(problems) = self.section_problems.as_mut() {
            if problems.len() > Self::MAX_WARNINGS {
                let overflow = problems.len() - Self::MAX_WARNINGS;
                problems.truncate(Self::MAX_WARNINGS);
                problems.push(format!(
                    "... and {overflow} more section{}",
                    if overflow == 1 { "" } else { "s" }
                ));
            }
            for line in problems.iter_mut() {
                if let Some((cut, _)) = line.char_indices().nth(Self::MAX_WARNING_LEN) {
                    line.truncate(cut);
                }
            }
        }
    }
}
