//! Per-shot survey credit: who surveyed a shot, who explored it, and when.
//!
//! Surveyors, explorers and the survey date live on every SHOT
//! ([`crate::Shot::credit`]). A section holds none of them; it only aggregates
//! its shots ([`crate::Section::credit_summary`]).
//!
//! People are stored BY VALUE - a name and an optional email, no registry ids -
//! so a shot copied between projects carries its credit with it.

use std::collections::HashSet;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{SectionId, ShotId};

/// One person credited on a shot.
///
/// Identity is [`Person::same_as`]: the names after lowercasing and collapsing
/// whitespace, AND the emails lowercased, where `None` only matches `None`.
/// The derived `PartialEq` is structural (exact spelling), which is what a
/// snapshot comparison wants; list operations use the identity.
///
/// `export_to` without `export`, like `ImportPlacement`: the file is written by
/// the gen_ipc_types example, but the member-crate path lets the parsers'
/// `TmlShot` export test compute its sibling import.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export_to = "../../bindings/Person.ts")]
pub struct Person {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub email: Option<String>,
}

impl Person {
    /// A person from user or file text: the name and email cleaned of control
    /// characters ([`clean_text`]) and trimmed, an empty email dropped. `None`
    /// when the name is empty after that.
    pub fn new(name: &str, email: Option<&str>) -> Option<Self> {
        let name = clean_text(name);
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let email = email
            .map(clean_text)
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty());
        Some(Self {
            name: name.to_string(),
            email,
        })
    }

    /// A person with a name and no email (see [`Person::new`]).
    pub fn named(name: &str) -> Option<Self> {
        Self::new(name, None)
    }

    /// The name as compared: lowercased, runs of whitespace collapsed to one
    /// space, ends trimmed.
    pub fn name_key(&self) -> String {
        self.name
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }

    /// The email as compared: trimmed and lowercased; an empty email is none.
    pub fn email_key(&self) -> Option<String> {
        self.email
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(str::to_lowercase)
    }

    /// The identity key: `(name_key, email_key)`.
    pub fn identity(&self) -> (String, Option<String>) {
        (self.name_key(), self.email_key())
    }

    /// Whether two people are the same person: equal names AND equal emails,
    /// both compared loosely, with a missing email equal only to another
    /// missing email.
    pub fn same_as(&self, other: &Person) -> bool {
        self.identity() == other.identity()
    }

    /// False for a person whose name is empty after trimming. Such a value can
    /// only arrive from a hand-built payload; every list operation drops it.
    pub fn is_valid(&self) -> bool {
        !self.name.trim().is_empty()
    }
}

/// The survey credit of one shot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export_to = "../../bindings/SurveyCredit.ts")]
pub struct SurveyCredit {
    /// Who surveyed the shot, in order.
    #[serde(default)]
    pub surveyors: Vec<Person>,
    /// Who explored (first laid line through) the shot, in order.
    #[serde(default)]
    pub explorers: Vec<Person>,
    /// The day the shot was surveyed.
    #[serde(default)]
    #[ts(type = "string | null")]
    pub date: Option<NaiveDate>,
}

/// The empty credit, for accessors that hand out a reference to a shot's
/// credit when the shot stores none.
pub static EMPTY_CREDIT: SurveyCredit = SurveyCredit {
    surveyors: Vec::new(),
    explorers: Vec::new(),
    date: None,
};

impl SurveyCredit {
    /// True when the credit names nobody and no date.
    pub fn is_empty(&self) -> bool {
        self.surveyors.is_empty() && self.explorers.is_empty() && self.date.is_none()
    }

    /// The credit the legacy free-text surveyor and date describe: the text
    /// split by [`legacy_people`] into explorers and surveyors.
    pub fn from_legacy(surveyor: Option<&str>, date: Option<NaiveDate>) -> Self {
        let (explorers, surveyors) = surveyor.map(legacy_people).unwrap_or_default();
        Self {
            surveyors,
            explorers,
            date,
        }
    }

    /// A survey team as the cave-survey text formats write it (Compass
    /// `SURVEY TEAM:`, a Walls `team:` comment): surveyors, split on `,` and
    /// `;` ([`split_role`] as a [`NameSource::NameList`]), with the survey's
    /// date.
    pub fn from_team(team: Option<&str>, date: Option<NaiveDate>) -> Self {
        Self {
            surveyors: team
                .map(|team| split_role(team, NameSource::NameList))
                .unwrap_or_default(),
            explorers: Vec::new(),
            date,
        }
    }

    /// `ShotData::credit` of a whole imported run: `None` for an empty credit.
    pub fn stored(self) -> Option<Self> {
        (!self.is_empty()).then_some(self)
    }

    /// Whether the credit is already as a shot stores it: every person
    /// sanitized ([`Person::is_sanitized`]) and no role longer than
    /// [`MAX_PEOPLE_PER_ROLE`]. Allocation-free.
    pub fn is_storable(&self) -> bool {
        [&self.surveyors, &self.explorers]
            .iter()
            .all(|role| role.len() <= MAX_PEOPLE_PER_ROLE && role.iter().all(Person::is_sanitized))
    }

    /// The credit as a shot stores it (`Shot::set_credit`, the one way in):
    /// every person sanitized - control characters out, trimmed, clamped -
    /// each role deduplicated and cut to [`MAX_PEOPLE_PER_ROLE`]. Returned as
    /// it is, with no allocation, when it already is that ([`Self::is_storable`]).
    #[must_use]
    pub fn into_storable(self) -> Self {
        if self.is_storable() {
            return self;
        }
        Self {
            surveyors: merge_people([self.surveyors]),
            explorers: merge_people([self.explorers]),
            date: self.date,
        }
    }

    /// The same credit with every list deduplicated and invalid people dropped.
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.surveyors = dedupe_people(self.surveyors);
        self.explorers = dedupe_people(self.explorers);
        self
    }

    /// The surveyors' names joined with `", "`, or `None` when there are none.
    /// What the plain-text exports carry (the legacy `sections.surveyor`
    /// column is [`legacy_credit_text`]).
    pub fn surveyor_names(&self) -> Option<String> {
        join_names(&self.surveyors)
    }

    /// The explorers' names joined with `", "`, or `None` when there are none.
    pub fn explorer_names(&self) -> Option<String> {
        join_names(&self.explorers)
    }
}

/// The names of `people` joined with `", "`, or `None` for an empty list.
pub fn join_names(people: &[Person]) -> Option<String> {
    if people.is_empty() {
        return None;
    }
    Some(
        people
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// The names of `people` joined with `", "`, each DISPLAY name once (compared
/// as [`Person::name_key`] compares them, the first spelling kept), or `None`
/// for an empty list.
///
/// For the legacy `sections.surveyor` text ([`legacy_credit_text`]), which
/// older builds and SpeleoDB show as-is: two different people sharing a name
/// (different emails) are one name there, not "Jo Diver, Jo Diver". The
/// per-shot tables still keep both people.
pub fn join_distinct_names(people: &[Person]) -> Option<String> {
    let mut seen = HashSet::new();
    let names: Vec<&str> = people
        .iter()
        .filter(|p| seen.insert(p.name_key()))
        .map(|p| p.name.as_str())
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

/// The legacy `sections.surveyor` text of a section's people: the only credit
/// SpeleoDB reads, and all an older build keeps, since a released build saves
/// by writing a fresh file and drops the per-shot tables (#718).
///
/// - No explorers: the surveyors as free text, their names joined with
///   `", "` (the text every build before #718 wrote), or `None` when there are
///   none either. The one exception is a role of exactly two one-word names,
///   joined with `"; "` (`"Ann; Bo"`): `"Ann, Bo"` would read back as one
///   `Last, First` person ([`NameSource::FreeText`]), also after an older
///   build's save.
/// - Explorers and surveyors: the Ariane importer's `"explorers / surveyors"`
///   form, each half joined with `", "` - read back as name lists
///   ([`NameSource::NameList`]), which have no `Last, First` rule.
/// - Explorers only: `"Ida Ice /"`, with no space after the `/`, so the text
///   survives trimming: an older build's Edit Section trims the surveyor on
///   every save, and [`legacy_halves`] reads both `"Ida Ice /"` and
///   `"Ida Ice / "` as explorers only.
///
/// Each half names each display name once, compared like
/// [`Person::name_key`]. A `/` with a space or a name edge on both sides is
/// written `-` in this text only (`"A / B"` becomes `"A - B"`), so no name can
/// hold or complete the `" / "` that splits the roles. A `,` or `;` inside one
/// name cannot come back as one person once an older build has saved the file
/// (it splits there); the per-shot tables keep every real name in this build.
pub fn legacy_credit_text(explorers: &[Person], surveyors: &[Person]) -> Option<String> {
    let explorers = legacy_names(explorers);
    let surveyors = legacy_names(surveyors);
    match (explorers.is_empty(), surveyors.is_empty()) {
        (true, true) => None,
        (true, false) => {
            let commas = surveyors.join(", ");
            Some(if looks_like_last_first(&commas) {
                surveyors.join("; ")
            } else {
                commas
            })
        }
        (false, true) => Some(format!("{} /", explorers.join(", "))),
        (false, false) => Some(format!(
            "{} / {}",
            explorers.join(", "),
            surveyors.join(", ")
        )),
    }
}

/// One role of [`legacy_credit_text`]: each written name once.
fn legacy_names(people: &[Person]) -> Vec<String> {
    let mut seen = HashSet::new();
    people
        .iter()
        // Trimmed first: a name's edge is where it meets the separator, and
        // a raw name ("A /\t") may still carry whitespace there.
        .filter_map(|p| Person::named(&legacy_name(p.name.trim())))
        .filter(|p| seen.insert(p.name_key()))
        .map(|p| p.name)
        .collect()
}

/// A name as [`legacy_credit_text`] writes it: every `/` whose neighbours are
/// both a space or the edge of the name becomes `-`. In the joined text a name
/// edge meets `", "`, `" / "` or the end, so this is exactly the set of `/`
/// that could form a `" / "` with what surrounds them.
fn legacy_name(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let open = |at: Option<usize>| at.and_then(|i| chars.get(i)).is_none_or(|c| *c == ' ');
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            if c == '/' && open(i.checked_sub(1)) && open(i.checked_add(1)) {
                '-'
            } else {
                c
            }
        })
        .collect()
}

/// Whether `list` holds someone who is [`Person::same_as`] `person`. The
/// needle's identity is built once; to merge whole lists use [`merge_people`],
/// which keys a hash set instead of scanning.
pub fn contains_person(list: &[Person], person: &Person) -> bool {
    let identity = person.identity();
    list.iter().any(|p| p.identity() == identity)
}

/// Append `person` to `list` unless it is already there, invalid, or the list
/// already holds [`MAX_PEOPLE_PER_ROLE`] people. Returns whether the list
/// changed.
pub fn add_person(list: &mut Vec<Person>, person: Person) -> bool {
    if list.len() >= MAX_PEOPLE_PER_ROLE || !person.is_valid() || contains_person(list, &person) {
        return false;
    }
    list.push(person);
    true
}

/// The people of `lists`, in order, each once (by identity, the first
/// spelling kept), sanitized ([`Person::sanitized`]) and never more than
/// [`MAX_PEOPLE_PER_ROLE`]. Linear: one hash set of identities, and it stops
/// reading once the role is full.
pub fn merge_people(lists: impl IntoIterator<Item = Vec<Person>>) -> Vec<Person> {
    let mut role = RoleBuilder::new();
    for person in lists.into_iter().flatten() {
        if role.is_full() {
            break;
        }
        role.push_person(&person);
    }
    role.people
}

/// Drop every entry of `list` that is [`Person::same_as`] `person`. Returns
/// whether the list changed.
pub fn remove_person(list: &mut Vec<Person>, person: &Person) -> bool {
    let before = list.len();
    list.retain(|p| !p.same_as(person));
    list.len() != before
}

/// `list` in order with duplicates (by identity) and invalid people removed;
/// the first spelling of each person wins. Linear: identities go through a
/// hash set, never a pairwise scan.
pub fn dedupe_people(list: Vec<Person>) -> Vec<Person> {
    let mut seen: HashSet<(String, Option<String>)> = HashSet::with_capacity(list.len());
    list.into_iter()
        .filter(|person| person.is_valid() && seen.insert(person.identity()))
        .collect()
}

/// Most pieces one role's free text is read for. A role keeps at most
/// [`MAX_PEOPLE_PER_ROLE`] people; this bounds the work on text that repeats
/// the same few names (or empty pieces) a million times, which no survey does
/// and a crafted file can.
pub const MAX_PIECES_PER_ROLE: usize = MAX_PEOPLE_PER_ROLE * 16;

/// One role's people as they are read from text: every piece sanitized
/// ([`Person::sanitized`]: trimmed, clamped to [`MAX_PERSON_TEXT_CHARS`]),
/// placeholders and empty pieces dropped, duplicates removed through a hash set
/// of identities, and never more than [`MAX_PEOPLE_PER_ROLE`] people.
struct RoleBuilder {
    people: Vec<Person>,
    seen: HashSet<(String, Option<String>)>,
}

impl RoleBuilder {
    fn new() -> Self {
        Self {
            people: Vec::new(),
            seen: HashSet::new(),
        }
    }

    fn is_full(&self) -> bool {
        self.people.len() >= MAX_PEOPLE_PER_ROLE
    }

    fn push_person(&mut self, person: &Person) {
        if let Some(person) = person.sanitized() {
            if self.seen.insert(person.identity()) {
                self.people.push(person);
            }
        }
    }

    fn push_piece(&mut self, piece: &str) {
        let piece = piece.trim();
        if self.is_full()
            || piece.is_empty()
            || PLACEHOLDERS.iter().any(|p| piece.eq_ignore_ascii_case(p))
        {
            return;
        }
        if let Some(person) = Person::named(clamp_chars(piece)) {
            if self.seen.insert(person.identity()) {
                self.people.push(person);
            }
        }
    }

    /// Read up to [`MAX_PIECES_PER_ROLE`] pieces, stopping once the role is full.
    fn extend<'a>(&mut self, pieces: impl Iterator<Item = &'a str>) {
        for piece in pieces.take(MAX_PIECES_PER_ROLE) {
            if self.is_full() {
                break;
            }
            self.push_piece(piece);
        }
    }
}

/// Placeholder texts that name nobody, compared ignoring ASCII case.
const PLACEHOLDERS: [&str; 3] = ["unknown", "none", "?"];

/// Where a text naming people comes from, which decides how one role of it is
/// split ([`split_role`], [`split_roles`]): only text a person typed freely
/// gets the `Last, First` rule (#719 review).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameSource {
    /// A field known to list names: anything parsed from an Ariane file (the
    /// plain `Explorer` field, the element form's halves, `XMLExplorer` /
    /// `XMLSurveyor`), either half of a legacy text in the Ariane `" / "`
    /// form, and a Compass or Walls team line. Split on `,` and `;` only:
    /// `ds, kb` is two people.
    NameList,
    /// Free text a person typed: a legacy `sections.surveyor` text that is NOT
    /// in the Ariane form. Split on `;` and `,`, except that exactly two comma
    /// parts, each one word, are one `Last, First` person (`Doe, John`).
    FreeText,
}

/// Split a text naming people into `(explorers, surveyors)`: the one splitter
/// every path shares - the TML import (`source` [`NameSource::NameList`]), a
/// `.stalx` legacy column ([`legacy_people`], [`NameSource::FreeText`]), and
/// the frontend's copies (`legacyPeople` in `src/utils/sectionCredit.ts`, the
/// Playwright mock).
///
/// * Text in the Ariane form ([`legacy_halves`]) gives the explorers from its
///   first half and the surveyors from its second, each split as a
///   [`NameSource::NameList`], whatever `source`: that form is the Ariane
///   importer's own join.
/// * Any other text names SURVEYORS only, split by `source`'s rule: a single
///   unseparated name is a surveyor on every path.
/// * Pieces are trimmed. Empty pieces and the placeholders `unknown`, `none`
///   and `?` are dropped, and duplicates are removed.
pub fn split_roles(text: &str, source: NameSource) -> (Vec<Person>, Vec<Person>) {
    match legacy_halves(text) {
        Some((explorers, surveyors)) => (
            split_role(explorers, NameSource::NameList),
            split_role(surveyors, NameSource::NameList),
        ),
        None => (Vec::new(), split_role(text, source)),
    }
}

/// A `.stalx` legacy `sections.surveyor` text as `(explorers, surveyors)`:
/// [`split_roles`] with text outside the Ariane form read as
/// [`NameSource::FreeText`].
pub fn legacy_people(text: &str) -> (Vec<Person>, Vec<Person>) {
    split_roles(text, NameSource::FreeText)
}

/// The `(explorers, surveyors)` halves of legacy text in the Ariane form, or
/// `None` for text that names surveyors only:
///
/// * exactly two `" / "` halves: the Ariane importer's join of a file's
///   explorer and surveyor halves, and what [`legacy_credit_text`] writes for
///   a section with both roles;
/// * no `" / "` at all, and the text ends in `" /"` once trimmed: explorers
///   only, the second half empty. [`legacy_credit_text`] writes `"Ida Ice /"`;
///   `"Ida Ice / "` (what it wrote before) reads the same, as does either
///   after an older build's Edit Section trimmed it on save. A written name
///   never ends in `" /"` ([`legacy_credit_text`] writes that `/` as `-`).
pub fn legacy_halves(text: &str) -> Option<(&str, &str)> {
    // Lazily: a crafted text can hold a million separators.
    let mut halves = text.split(" / ");
    match (halves.next(), halves.next(), halves.next()) {
        (Some(explorers), Some(surveyors), None) => Some((explorers, surveyors)),
        (Some(_), None, _) => text
            .trim_end()
            .strip_suffix(" /")
            .map(|explorers| (explorers, "")),
        _ => None,
    }
}

/// Split one role's text into people by its source's rule ([`NameSource`]).
///
/// Bounded like every splitter here (see [`RoleBuilder`]): at most
/// [`MAX_PEOPLE_PER_ROLE`] people, each clamped to [`MAX_PERSON_TEXT_CHARS`].
pub fn split_role(text: &str, source: NameSource) -> Vec<Person> {
    let text = text.trim();
    let mut role = RoleBuilder::new();
    if source == NameSource::FreeText && looks_like_last_first(text) {
        role.push_piece(text);
    } else {
        role.extend(text.split([',', ';']));
    }
    role.people
}

/// `Last, First`: exactly two comma parts, each a single non-empty word, and no
/// `;` anywhere.
fn looks_like_last_first(half: &str) -> bool {
    if half.contains(';') {
        return false;
    }
    let mut parts = half.split(',').map(str::trim);
    match (parts.next(), parts.next(), parts.next()) {
        (Some(last), Some(first), None) => [last, first]
            .iter()
            .all(|p| !p.is_empty() && !p.contains(char::is_whitespace)),
        _ => false,
    }
}

/// Longest name or email a credit edit stores, in chars. Longer text is cut on
/// a char boundary: a person is a label, and an unbounded one would ride into
/// every save, undo entry and export.
pub const MAX_PERSON_TEXT_CHARS: usize = 256;

/// Most people one role of one shot can list.
pub const MAX_PEOPLE_PER_ROLE: usize = 200;

fn clamp_chars(text: &str) -> &str {
    match text.char_indices().nth(MAX_PERSON_TEXT_CHARS) {
        Some((cut, _)) => &text[..cut],
        None => text,
    }
}

impl Person {
    /// The person as every stored credit holds it: cleaned of control
    /// characters ([`clean_text`]), trimmed, clamped to
    /// [`MAX_PERSON_TEXT_CHARS`], an empty email dropped. `None` for an empty
    /// name.
    pub fn sanitized(&self) -> Option<Person> {
        let name = clean_text(&self.name);
        let email = self.email.as_deref().map(clean_text);
        Person::new(
            clamp_chars(name.trim()),
            email.as_deref().map(|e| clamp_chars(e.trim())),
        )
    }

    /// Whether the person is already as [`Person::sanitized`] would make it.
    /// Allocation-free: the check every stored credit passes on its way in.
    pub fn is_sanitized(&self) -> bool {
        let clean = |text: &str| {
            !text.is_empty()
                && text.trim() == text
                && !text.chars().any(needs_cleaning)
                && text.chars().nth(MAX_PERSON_TEXT_CHARS).is_none()
        };
        clean(&self.name) && self.email.as_deref().is_none_or(clean)
    }
}

/// Whether `c` cannot stay in stored text: a control character (it would
/// make an exported XML document invalid, or a label unreadable) or one of
/// the two noncharacters XML 1.0 forbids.
fn needs_cleaning(c: char) -> bool {
    c.is_control() || c == '\u{fffe}' || c == '\u{ffff}'
}

/// `text` with every control character a space if it is whitespace (tab,
/// newline) and dropped otherwise, and the XML-forbidden noncharacters
/// dropped. Borrowed when there is nothing to change.
pub fn clean_text(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.chars().any(needs_cleaning) {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(
        text.chars()
            .filter_map(|c| match c {
                c if !needs_cleaning(c) => Some(c),
                c if c.is_whitespace() => Some(' '),
                _ => None,
            })
            .collect(),
    )
}

/// Which shots a [`CreditEdit`] applies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum CreditTargets {
    /// Every shot of one section.
    Section { section_id: SectionId },
    /// Explicit shots, as `(section_id, shot_id)` pairs (in-session ids).
    Shots { shots: Vec<(SectionId, ShotId)> },
}

/// One edit of the survey credit of a set of shots
/// (`Command::EditShotCredit`).
///
/// Per role the order is: `set_*` replaces the list, then `remove_*` drops
/// people, then `add_*` appends those not already there. `set_date` sets
/// (`Some(Some(d))`) or clears (`Some(None)`) the date; absent leaves it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CreditEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub add_surveyors: Option<Vec<Person>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub remove_surveyors: Option<Vec<Person>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub set_surveyors: Option<Vec<Person>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub add_explorers: Option<Vec<Person>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub remove_explorers: Option<Vec<Person>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub set_explorers: Option<Vec<Person>>,
    #[serde(
        default,
        deserialize_with = "double_option_date",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(type = "string | null")]
    #[ts(optional)]
    pub set_date: Option<Option<NaiveDate>>,
}

/// `absent -> None`, `null -> Some(None)`, a date -> `Some(Some(d))`.
fn double_option_date<'de, D>(de: D) -> Result<Option<Option<NaiveDate>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<NaiveDate>::deserialize(de).map(Some)
}

/// What a [`CreditEdit`] changes, for labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreditEditKind {
    Surveyors,
    Explorers,
    Date,
    Mixed,
    Nothing,
}

impl CreditEdit {
    fn touches_surveyors(&self) -> bool {
        self.add_surveyors.is_some()
            || self.remove_surveyors.is_some()
            || self.set_surveyors.is_some()
    }

    fn touches_explorers(&self) -> bool {
        self.add_explorers.is_some()
            || self.remove_explorers.is_some()
            || self.set_explorers.is_some()
    }

    /// Which part of the credit this edit touches.
    pub fn kind(&self) -> CreditEditKind {
        match (
            self.touches_surveyors(),
            self.touches_explorers(),
            self.set_date.is_some(),
        ) {
            (false, false, false) => CreditEditKind::Nothing,
            (true, false, false) => CreditEditKind::Surveyors,
            (false, true, false) => CreditEditKind::Explorers,
            (false, false, true) => CreditEditKind::Date,
            _ => CreditEditKind::Mixed,
        }
    }

    /// The history label of this edit: "Edit surveyors", "Edit explorers",
    /// "Edit survey date" or "Edit survey credit".
    pub fn label(&self) -> &'static str {
        match self.kind() {
            CreditEditKind::Surveyors => "Edit surveyors",
            CreditEditKind::Explorers => "Edit explorers",
            CreditEditKind::Date => "Edit survey date",
            CreditEditKind::Mixed | CreditEditKind::Nothing => "Edit survey credit",
        }
    }

    /// The credit `before` becomes under this edit.
    pub fn apply(&self, before: &SurveyCredit) -> SurveyCredit {
        SurveyCredit {
            surveyors: edit_role(
                &before.surveyors,
                self.set_surveyors.as_deref(),
                self.remove_surveyors.as_deref(),
                self.add_surveyors.as_deref(),
            ),
            explorers: edit_role(
                &before.explorers,
                self.set_explorers.as_deref(),
                self.remove_explorers.as_deref(),
                self.add_explorers.as_deref(),
            ),
            date: self.set_date.unwrap_or(before.date),
        }
    }
}

fn edit_role(
    current: &[Person],
    set: Option<&[Person]>,
    remove: Option<&[Person]>,
    add: Option<&[Person]>,
) -> Vec<Person> {
    // Every incoming list is cut to what a role can hold BEFORE anything is
    // sanitized or deduplicated: an IPC payload is not otherwise bounded.
    let base: Vec<Person> = match set {
        Some(set) => set
            .iter()
            .take(MAX_PEOPLE_PER_ROLE)
            .filter_map(Person::sanitized)
            .collect(),
        None => current.to_vec(),
    };
    let removed: HashSet<(String, Option<String>)> = remove
        .unwrap_or_default()
        .iter()
        .take(MAX_PIECES_PER_ROLE)
        .map(Person::identity)
        .collect();
    let mut seen: HashSet<(String, Option<String>)> = HashSet::new();
    let mut list: Vec<Person> = Vec::new();
    let added = add
        .unwrap_or_default()
        .iter()
        .take(MAX_PEOPLE_PER_ROLE)
        .filter_map(Person::sanitized);
    let kept = base
        .into_iter()
        .filter(|person| !removed.contains(&person.identity()));
    for person in kept.chain(added) {
        if list.len() >= MAX_PEOPLE_PER_ROLE {
            break;
        }
        if person.is_valid() && seen.insert(person.identity()) {
            list.push(person);
        }
    }
    list
}

/// One person in a [`SectionCreditSummary`], with how many of the section's
/// shots credit them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CreditedPerson {
    pub person: Person,
    pub shots: u32,
}

/// What a section's shots say about who surveyed it and when
/// ([`crate::Section::credit_summary`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SectionCreditSummary {
    /// Every surveyor of any shot, in first-appearance order, with a count.
    pub surveyors: Vec<CreditedPerson>,
    /// Every explorer of any shot, in first-appearance order, with a count.
    pub explorers: Vec<CreditedPerson>,
    /// The earliest shot date.
    #[ts(type = "string | null")]
    pub date_min: Option<NaiveDate>,
    /// The latest shot date.
    #[ts(type = "string | null")]
    pub date_max: Option<NaiveDate>,
    /// How many shots the section has (the denominator of every count).
    pub shot_count: u32,
}

impl SectionCreditSummary {
    /// Aggregate a run of shot credits.
    ///
    /// Shots that share ONE credit (every shot a section's legacy text
    /// credits shares one allocation) are keyed once, by address, and only
    /// counted after that: a long legacy list over many shots costs one pass
    /// over its people, not one per shot.
    pub fn from_credits<'a>(credits: impl IntoIterator<Item = &'a SurveyCredit>) -> Self {
        let mut surveyors = Tally::default();
        let mut explorers = Tally::default();
        let mut summary = Self::default();
        let mut keyed: std::collections::HashMap<*const SurveyCredit, (Vec<usize>, Vec<usize>)> =
            std::collections::HashMap::new();
        for credit in credits {
            summary.shot_count = summary.shot_count.saturating_add(1);
            let (surveyed, explored) =
                keyed.entry(std::ptr::from_ref(credit)).or_insert_with(|| {
                    (
                        surveyors.index_shot(&credit.surveyors),
                        explorers.index_shot(&credit.explorers),
                    )
                });
            surveyors.count(surveyed);
            explorers.count(explored);
            if let Some(date) = credit.date {
                summary.date_min = Some(summary.date_min.map_or(date, |d| d.min(date)));
                summary.date_max = Some(summary.date_max.map_or(date, |d| d.max(date)));
            }
        }
        summary.surveyors = surveyors.people;
        summary.explorers = explorers.people;
        summary
    }

    /// The surveyors alone, in first-appearance order.
    pub fn surveyor_people(&self) -> Vec<Person> {
        self.surveyors.iter().map(|c| c.person.clone()).collect()
    }

    /// The explorers alone, in first-appearance order.
    pub fn explorer_people(&self) -> Vec<Person> {
        self.explorers.iter().map(|c| c.person.clone()).collect()
    }
}

/// People counted per shot, in first-appearance order, with their identity
/// keys alongside so a long section is not re-keyed on every comparison.
#[derive(Default)]
struct Tally {
    keys: std::collections::HashMap<(String, Option<String>), usize>,
    people: Vec<CreditedPerson>,
}

impl Tally {
    /// The indices of one shot's people, each once (a person listed twice on
    /// one shot counts once for it), adding the people not seen yet at the
    /// end with no shots counted.
    fn index_shot(&mut self, people: &[Person]) -> Vec<usize> {
        let mut seen_here: Vec<usize> = Vec::with_capacity(people.len());
        for person in people {
            if !person.is_valid() {
                continue;
            }
            let next = self.people.len();
            let index = *self.keys.entry(person.identity()).or_insert(next);
            if seen_here.contains(&index) {
                continue;
            }
            seen_here.push(index);
            if index == next {
                self.people.push(CreditedPerson {
                    person: person.clone(),
                    shots: 0,
                });
            }
        }
        seen_here
    }

    /// Count one more shot for each of `indices` ([`Self::index_shot`]).
    fn count(&mut self, indices: &[usize]) {
        for &index in indices {
            if let Some(entry) = self.people.get_mut(index) {
                entry.shots = entry.shots.saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests;
