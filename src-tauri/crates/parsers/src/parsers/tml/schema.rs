//! The Ariane TML/TMLU schema, stated once for both directions (#576 B4).
//!
//! The importer (`parsers::tml`) and the exporter (`export::tml`) used to carry
//! their own copies of every element name, the shot-type strings, the `DepthIn`
//! sentinel and the inclination and colour conversions. The copies agreed, but
//! nothing made them: a typo in one `.tmlu` short name would have shipped a
//! field that our own reader then read back as `0.0`, and no round trip
//! asserted the LRUD or colour fields closely enough to notice. Everything here
//! is the single definition both sides use.
//!
//! `.tml` files (a ZIP holding `Data.xml`) spell elements long (`Azimut`),
//! `.tmlu` files (plain XML) spell them short (`AZ`); the file-level metadata
//! elements are spelled the same in both.

/// Which of the two spellings a document uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spelling {
    /// `.tml`: `SurveyData`, `Azimut`, `Length`, ...
    Long,
    /// `.tmlu`: `SRVD`, `AZ`, `LG`, ...
    Short,
}

/// One element, spelled both ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementName {
    pub long: &'static str,
    pub short: &'static str,
}

impl ElementName {
    /// The element's name in a document of the given spelling.
    pub const fn spelled(self, spelling: Spelling) -> &'static str {
        match spelling {
            Spelling::Long => self.long,
            Spelling::Short => self.short,
        }
    }
}

/// The first literal of a list: an element's short spelling when it has one,
/// its long spelling otherwise.
macro_rules! first_literal {
    ($first:literal $(, $rest:literal)*) => {
        $first
    };
}

/// One module per element, holding its [`ElementName`] and the byte forms the
/// importer's hot loop matches against (`const` items, so they are usable as
/// `match` patterns, which a field of a struct constant is not). An element
/// spelled the same in both formats gets no `SHORT`: matching `LONG | SHORT`
/// on two equal constants would be an unreachable pattern.
macro_rules! elements {
    ($( $(#[$doc:meta])* $module:ident => $long:literal $(| $short:literal)?; )*) => {
        $(
            $(#[$doc])*
            pub mod $module {
                use super::super::ElementName;

                pub const NAME: ElementName = ElementName {
                    long: $long,
                    short: first_literal!($($short,)? $long),
                };
                pub const LONG: &[u8] = $long.as_bytes();
                $( pub const SHORT: &[u8] = $short.as_bytes(); )?
            }
        )*

        /// Every element, in declaration order.
        pub const ALL: &[super::ElementName] = &[$($module::NAME),*];
    };
}

/// Element names. Long spelling first, then the `.tmlu` short one where it
/// differs.
pub mod element {
    elements! {
        // ---- File level ----
        /// The document root.
        cave_file => "CaveFile";
        cave_name => "caveName";
        /// Length unit of the whole file.
        unit => "unit";
        /// File-level north reference flag (#438).
        use_magnetic_azimuth => "useMagneticAzimuth";
        /// Entrance elevation above sea level (#431).
        first_start_absolute_elevation => "firstStartAbsoluteElevation";
        /// Holds the shot rows.
        data => "Data";
        /// One shot row.
        survey_data => "SurveyData" | "SRVD";
        /// Per-shot cross-section polygons; the importer skips the subtree and
        /// the exporter writes none.
        shape => "Shape" | "SH";

        // ---- Per shot, in the alphabetical order Ariane writes them ----
        azimuth => "Azimut" | "AZ";
        closure_to_id => "ClosureToID" | "CID";
        color => "Color" | "CL";
        comment => "Comment" | "CM";
        date => "Date" | "DT";
        depth => "Depth" | "DP";
        /// Start depth, or [`super::DEPTH_IN_SAME_AS_PREVIOUS`].
        depth_in => "DepthIn" | "DPI";
        down => "Down" | "D";
        excluded => "Excluded" | "EXC";
        explorer => "Explorer" | "EX";
        from_id => "FromID" | "FRID";
        /// All caps in both spellings; see [`super::ID_LEGACY_SPELLING`].
        id => "ID";
        inclination => "Inclination" | "INC";
        latitude => "Latitude" | "LT";
        left => "Left" | "L";
        length => "Length" | "LG";
        /// Written by the exporter; the importer does not read it.
        locked => "Locked" | "LCK";
        longitude => "Longitude" | "LGT";
        /// Ariane's per-row marker ([`super::NAME_START`],
        /// [`super::NAME_CLOSURE`], else empty). Written by the exporter; the
        /// importer takes the shot type from `Type` and does not read it.
        name => "Name" | "NM";
        /// Written by the exporter; the importer does not read it.
        profiletype => "Profiletype" | "PT";
        right => "Right" | "R";
        section => "Section" | "SC";
        /// `<Type>`: one of the `TYPE_*` values below.
        shot_type => "Type" | "TY";
        up => "Up" | "U";
        /// Ariane 26+'s stable per-row id (#423).
        uuid => "UUID" | "UU";
    }
}

/// A spelling of `ID` some third-party files use. Read, never written.
pub const ID_LEGACY_SPELLING: &[u8] = b"Id";

/// `<Type>` values. Ariane writes them upper case; the reader matches any case.
pub const TYPE_START: &str = "START";
pub const TYPE_REAL: &str = "REAL";
pub const TYPE_VIRTUAL: &str = "VIRTUAL";
pub const TYPE_CLOSURE: &str = "CLOSURE";

/// `<Name>` on a START row.
pub const NAME_START: &str = "START";
/// `<Name>` on a CLOSURE row.
pub const NAME_CLOSURE: &str = "CLOSURE:CLOSURE";

/// The only `<Profiletype>` the exporter writes.
pub const PROFILE_TYPE_VERTICAL: &str = "VERTICAL";

/// The `<Date>` written for a section with no survey date.
///
/// A date, not an empty element: SpeleoDB rejects the whole file when `<Date>`
/// is missing or empty, and whether Ariane opens an empty one is unverified.
/// Our reader takes EVERY 2000-01-01 as "no date", marked or not (#576 B2,
/// B2-MARKER), so a survey genuinely dated 1 January 2000 imports undated.
pub const UNDATED_PLACEHOLDER: &str = "2000-01-01";

/// Text of the XML comment the exporter writes right after a `<Date>` that is
/// [`UNDATED_PLACEHOLDER`] standing for "no date" (#576 B2), so a person
/// reading the file can tell the placeholder from a date. Our reader does not
/// need it: it reads every placeholder as undated, because the comment does
/// not survive everywhere (Ariane drops it on re-save, older exports and the
/// SpeleoDB sidecar never carry it).
///
/// A comment rather than an element because nothing in the TML schema is a
/// field Ariane ignores: `Name`, `Locked`, `Profiletype` and `Excluded` all
/// mean something to it, and `UUID` is a shot's identity. A comment is not
/// document content at all under the XML specification, so every XML reader -
/// Ariane's, SpeleoDB's - skips it, and Ariane shows nothing.
pub const UNDATED_MARKER: &str = "stalagmight:no-survey-date";

/// Ariane's spelling of "nobody filled in this person". The exporter writes it
/// for a section with no surveyor; the reader drops it per half of the field
/// (#427), together with `None` and the empty string.
pub const EXPLORER_PLACEHOLDER: &str = "unknown";

/// Ariane's `DepthIn` sentinel: "this shot starts where the previous one
/// ended". The format's marker, not ours: a literal `-1.0` in a file means
/// "inherit" even though -1 m is a legal depth, so the exporter nudges a
/// genuine -1.0 off it before writing (`export::tml::emitted_depth_in`).
pub const DEPTH_IN_SAME_AS_PREVIOUS: f64 = -1.0;

/// Whether a `DepthIn` value read from a file is the sentinel. Exact: the
/// sentinel is a spelling (`-1.0`), not a measurement with a tolerance.
///
/// A METRIC file can also spell it as -1 FOOT in metres (#576 HFI-1): see
/// [`DEPTH_IN_SAME_AS_PREVIOUS_SCALED`]. That reading is the parser's, per file
/// (`parsers::tml::restore_scaled_depth_in_marker`), not this per-value test's.
pub fn is_depth_in_sentinel(depth_in: f64) -> bool {
    depth_in == DEPTH_IN_SAME_AS_PREVIOUS
}

/// [#576 HFI-1] The sentinel after a feet-to-metre conversion that did not
/// except it: -1.0 ft x 0.3048 = -0.3048, kept to 1-4 decimals, rounded or
/// truncated. Ariane writes lengths with two decimals and trims a trailing zero,
/// so its own conversion leaves `-0.3` (-0.30); the other spellings are the same
/// value at the other precisions a converter can keep. Each is matched to a
/// micrometre - the finest precision any writer in this format uses (our export
/// writes six decimals) - so a nearby real depth (-0.31, -0.2) is never one.
pub const DEPTH_IN_SAME_AS_PREVIOUS_SCALED: [f64; 4] = [-0.3, -0.304, -0.305, -0.3048];

/// Whether `depth_in` is one of [`DEPTH_IN_SAME_AS_PREVIOUS_SCALED`].
pub fn is_scaled_depth_in_sentinel(depth_in: f64) -> bool {
    scaled_depth_in_spelling(depth_in).is_some()
}

/// The spelling of [`DEPTH_IN_SAME_AS_PREVIOUS_SCALED`] that `depth_in` is, if any.
pub fn scaled_depth_in_spelling(depth_in: f64) -> Option<f64> {
    DEPTH_IN_SAME_AS_PREVIOUS_SCALED
        .iter()
        .copied()
        .find(|spelling| (depth_in - spelling).abs() < 1e-6)
}

/// Whether `c` is in XML 1.0's `Char` production -
/// `#x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]`.
///
/// Anything outside it cannot appear in a well-formed document at all, escaped
/// or not: quick-xml's `BytesText::new` escapes only `<>&"'` and writes the rest
/// as raw bytes, and one such byte makes Ariane and SpeleoDB reject the WHOLE
/// file. Surrogates are the production's other exclusion and cannot occur in a
/// Rust `char`. C1 controls (U+0080-U+009F) ARE in the production.
pub fn is_xml_char(c: char) -> bool {
    matches!(c,
        '\u{9}' | '\u{A}' | '\u{D}'
        | '\u{20}'..='\u{D7FF}'
        | '\u{E000}'..='\u{FFFD}'
        | '\u{10000}'..='\u{10FFFF}')
}

/// `text` with every character XML 1.0 cannot carry replaced by U+FFFD, and how
/// many were replaced (#576 B11).
///
/// Replaced, not dropped: the file always opens, and the change is visible in
/// Ariane rather than silently closing up the text around it. Line breaks
/// (`\n`, `\r`, tab) and the U+200B shot-comment marker are legal XML and pass
/// through untouched - a broader "strip control characters" filter would merge
/// multi-line comments and destroy the station-comment co-encoding. Borrowed
/// when nothing needed replacing, which is every ordinary string.
pub fn xml_text(text: &str) -> (std::borrow::Cow<'_, str>, usize) {
    let replaced = text.chars().filter(|&c| !is_xml_char(c)).count();
    if replaced == 0 {
        return (std::borrow::Cow::Borrowed(text), 0);
    }
    let safe = text
        .chars()
        .map(|c| if is_xml_char(c) { c } else { '\u{FFFD}' })
        .collect();
    (std::borrow::Cow::Owned(safe), replaced)
}

/// Colour Ariane gets for a section whose own colour is not `#RRGGBB`.
pub const FALLBACK_ARIANE_COLOR: &str = "0x4ecdc4ff";
/// The model colour a file row without a usable `<Color>` imports as.
pub const FALLBACK_MODEL_COLOR: &str = "#4ecdc4";

/// Convert a model pitch into Ariane's `<Inclination>`.
///
/// The model is +down (positive descends, matching `depth_in` / `depth_out`).
/// Ariane is taken to be +up, like every other survey format we read: Walls
/// and Compass both store +up in-file, and the mNemo device reports +up. That
/// could not be confirmed from data. Across every Ariane file available, all
/// 5,686 shots carry `Inclination` 0, because these surveys are depth-gauge
/// based and Ariane never populates the field. So this is an inference from the
/// surrounding ecosystem, not a measurement, and it is unobservable on real
/// files; it only changes what we emit. `None` and `0` both export as a plain
/// `0.0` rather than `-0.0`. Inverse of [`inclination_to_pitch`].
pub fn pitch_to_inclination(pitch: Option<f64>) -> f64 {
    match pitch {
        Some(p) if p != 0.0 => -p,
        _ => 0.0,
    }
}

/// Convert Ariane's `<Inclination>` into a model pitch.
///
/// The sign flips for the reason [`pitch_to_inclination`] gives, and could not
/// be confirmed against data either. `0` maps to a plain `0.0`, never `-0.0`,
/// so a round-trip through Ariane does not show up as a phantom edit.
///
/// Applied to every `.tml` regardless of who wrote it, deliberately: export
/// writes the same convention, so a file Stalagmight produces re-imports
/// correctly as though Ariane had written it. A standalone `.tml` from a
/// pre-#296 build holds raw +down pitch here and so imports inverted, with no
/// migration to repair it; that is accepted rather than sniffed for, since real
/// Ariane files carry `Inclination` 0. See `docs/inclination.md`.
pub fn inclination_to_pitch(inclination: f64) -> f64 {
    if inclination == 0.0 {
        0.0
    } else {
        -inclination
    }
}

/// Convert a model `#RRGGBB` colour to Ariane's `0xRRGGBBff`, falling back to
/// [`FALLBACK_ARIANE_COLOR`] for anything else.
pub fn color_to_ariane(hex: &str) -> String {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        format!("0x{hex}ff")
    } else {
        FALLBACK_ARIANE_COLOR.to_string()
    }
}

/// Convert Ariane's `0xRRGGBBAA` to a model `#RRGGBB` (the alpha is dropped),
/// falling back to [`FALLBACK_MODEL_COLOR`]. Inverse of [`color_to_ariane`] for
/// every colour that function writes.
pub fn color_from_ariane(color: Option<&str>) -> String {
    let Some(s) = color.map(str::trim).filter(|s| !s.is_empty()) else {
        return FALLBACK_MODEL_COLOR.to_string();
    };
    let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) else {
        return FALLBACK_MODEL_COLOR.to_string();
    };
    if hex.len() != 8 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return FALLBACK_MODEL_COLOR.to_string();
    }
    format!("#{}", &hex[..6])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both spellings of every element are distinct across elements, so a
    /// reader keying on the name can never confuse two fields.
    #[test]
    fn no_two_elements_share_a_spelling() {
        let mut seen = std::collections::HashSet::new();
        for name in element::ALL {
            assert!(seen.insert(name.long), "{} spelled twice", name.long);
            if name.short != name.long {
                assert!(seen.insert(name.short), "{} spelled twice", name.short);
            }
        }
    }

    /// Spelling selects the right half.
    #[test]
    fn spelled_picks_the_documents_spelling() {
        assert_eq!(element::azimuth::NAME.spelled(Spelling::Long), "Azimut");
        assert_eq!(element::azimuth::NAME.spelled(Spelling::Short), "AZ");
        assert_eq!(element::id::NAME.spelled(Spelling::Short), "ID");
        assert_eq!(element::azimuth::LONG, b"Azimut");
        assert_eq!(element::azimuth::SHORT, b"AZ");
    }

    /// The sign flip is its own inverse, and zero stays a positive zero.
    #[test]
    fn inclination_and_pitch_invert() {
        for pitch in [30.0_f64, -30.0, 90.0, -90.0, 0.5, -0.5, 1e-9] {
            let back = inclination_to_pitch(pitch_to_inclination(Some(pitch)));
            assert!(
                (back - pitch).abs() < f64::EPSILON,
                "{pitch} came back {back}"
            );
        }
        for zero in [
            pitch_to_inclination(None),
            pitch_to_inclination(Some(0.0)),
            pitch_to_inclination(Some(-0.0)),
            inclination_to_pitch(0.0),
            inclination_to_pitch(-0.0),
        ] {
            assert!(zero == 0.0 && zero.is_sign_positive());
        }
    }

    /// Every colour the exporter writes reads back as itself, and the
    /// fallbacks are each other's image.
    #[test]
    fn colors_invert() {
        for model in ["#000000", "#ffffff", "#336699", "#AbCdEf"] {
            assert_eq!(color_from_ariane(Some(&color_to_ariane(model))), model);
        }
        assert_eq!(
            color_from_ariane(Some(&color_to_ariane("not-a-colour"))),
            FALLBACK_MODEL_COLOR
        );
        assert_eq!(color_from_ariane(Some("0X8F6112FF")), "#8F6112");
        for junk in [None, Some(""), Some("  "), Some("8f6112ff"), Some("0x8f61")] {
            assert_eq!(color_from_ariane(junk), FALLBACK_MODEL_COLOR);
        }
    }

    /// Exactly the characters outside XML 1.0's `Char` production become
    /// U+FFFD; line breaks, tab, the U+200B comment marker, C1 controls and
    /// astral characters survive as themselves.
    #[test]
    fn xml_text_replaces_exactly_what_xml_cannot_carry() {
        let (safe, replaced) = xml_text("a\u{1}b\u{B}c\u{FFFE}d\u{FFFF}e\u{0}");
        assert_eq!(safe, "a\u{FFFD}b\u{FFFD}c\u{FFFD}d\u{FFFD}e\u{FFFD}");
        assert_eq!(replaced, 5);

        let legal = "line one\nline two\r\n\ttabbed \u{200B}marked \u{85}c1 \u{1F600} \u{FFFD}";
        let (kept, none) = xml_text(legal);
        assert_eq!(none, 0);
        assert!(matches!(kept, std::borrow::Cow::Borrowed(_)));
        assert_eq!(kept, legal);

        for c in [
            '\u{0}', '\u{8}', '\u{B}', '\u{C}', '\u{1F}', '\u{FFFE}', '\u{FFFF}',
        ] {
            assert!(!is_xml_char(c), "{c:?} is not XML");
        }
        for c in [
            '\t',
            '\n',
            '\r',
            ' ',
            '\u{D7FF}',
            '\u{E000}',
            '\u{FFFD}',
            '\u{10000}',
        ] {
            assert!(is_xml_char(c), "{c:?} is XML");
        }
    }

    #[test]
    fn the_depth_in_sentinel_is_exact() {
        assert!(is_depth_in_sentinel(-1.0));
        assert!(!is_depth_in_sentinel(-1.000_001));
        assert!(!is_depth_in_sentinel(-0.999_999));
        assert!(!is_depth_in_sentinel(1.0));
    }

    /// [#576 HFI-1] -1 ft in metres at the precisions a converter keeps, and
    /// nothing a micrometre or more away from one of them.
    #[test]
    fn the_scaled_depth_in_sentinel_is_its_spellings_only() {
        for spelling in [-0.3, -0.30, -0.304, -0.305, -0.3048, -0.304_800_000_4] {
            assert!(is_scaled_depth_in_sentinel(spelling), "{spelling}");
        }
        for real in [-0.31, -0.29, -0.2, -0.3049, -0.303, -0.300_002, 0.3, -1.0] {
            assert!(!is_scaled_depth_in_sentinel(real), "{real}");
        }
    }
}
