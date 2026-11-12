//! Per-shot survey credit: who surveyed a shot, who explored it, and when.
//!
//! Surveyors, explorers and the survey date live on every SHOT
//! ([`crate::Shot::credit`]). A section holds none of them; it only aggregates
//! its shots ([`crate::Section::credit_summary`]).
//!
//! People are stored BY VALUE - a name and an optional email, no registry ids -
//! so a shot copied between projects carries its credit with it.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{SectionId, ShotId};

/// One person credited on a shot.
///
/// Identity is [`Person::same_as`]: the names after casefolding and collapsing
/// whitespace, AND the emails lowercased, where `None` only matches `None`.
/// The derived `PartialEq` is structural (exact spelling), which is what a
/// snapshot comparison wants; list operations use the identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
pub struct Person {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub email: Option<String>,
}

impl Person {
    /// A person from user or file text: the name trimmed, the email trimmed and
    /// dropped when empty. `None` when the name is empty after trimming.
    pub fn new(name: &str, email: Option<&str>) -> Option<Self> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let email = email
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(str::to_string);
        Some(Self {
            name: name.to_string(),
            email,
        })
    }

    /// A person with a name and no email (see [`Person::new`]).
    pub fn named(name: &str) -> Option<Self> {
        Self::new(name, None)
    }

    /// The name as compared: casefolded, runs of whitespace collapsed to one
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
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

    /// The same credit with every list deduplicated and invalid people dropped.
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.surveyors = dedupe_people(self.surveyors);
        self.explorers = dedupe_people(self.explorers);
        self
    }

    /// The surveyors' names joined with `", "`, or `None` when there are none.
    /// What the legacy `sections.surveyor` column and the plain-text exports
    /// carry.
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

/// Whether `list` holds someone who is [`Person::same_as`] `person`.
pub fn contains_person(list: &[Person], person: &Person) -> bool {
    list.iter().any(|p| p.same_as(person))
}

/// Append `person` to `list` unless it is already there or invalid. Returns
/// whether the list changed.
pub fn add_person(list: &mut Vec<Person>, person: Person) -> bool {
    if !person.is_valid() || contains_person(list, &person) {
        return false;
    }
    list.push(person);
    true
}

/// Drop every entry of `list` that is [`Person::same_as`] `person`. Returns
/// whether the list changed.
pub fn remove_person(list: &mut Vec<Person>, person: &Person) -> bool {
    let before = list.len();
    list.retain(|p| !p.same_as(person));
    list.len() != before
}

/// `list` in order with duplicates (by identity) and invalid people removed;
/// the first spelling of each person wins.
pub fn dedupe_people(list: Vec<Person>) -> Vec<Person> {
    let mut out: Vec<Person> = Vec::with_capacity(list.len());
    for person in list {
        add_person(&mut out, person);
    }
    out
}

/// Placeholder texts that name nobody, compared casefolded.
const PLACEHOLDERS: [&str; 3] = ["unknown", "none", "?"];

/// Split a legacy free-text surveyor into `(explorers, surveyors)`.
///
/// * Text with exactly two `" / "` halves came from the Ariane importer, which
///   joined the file's explorer and surveyor halves that way: the first half is
///   the explorers, the second the surveyors. Any other text is surveyors only.
/// * Each half is split on `;`. Each piece is then split on `,`, unless the
///   whole half looks like `Last, First` (exactly two comma parts, each one
///   word), which is one person.
/// * Pieces are trimmed. Empty pieces and the placeholders `unknown`, `none`
///   and `?` are dropped, and duplicates are removed.
pub fn legacy_people(text: &str) -> (Vec<Person>, Vec<Person>) {
    let halves: Vec<&str> = text.split(" / ").collect();
    if let [explorers, surveyors] = halves.as_slice() {
        (split_people(explorers), split_people(surveyors))
    } else {
        (Vec::new(), split_people(text))
    }
}

/// Split one role's free text into people (the per-half rule of
/// [`legacy_people`]).
pub fn split_people(half: &str) -> Vec<Person> {
    let half = half.trim();
    let mut people = Vec::new();
    if looks_like_last_first(half) {
        push_piece(&mut people, half);
        return people;
    }
    for piece in half.split(';') {
        for name in piece.split(',') {
            push_piece(&mut people, name);
        }
    }
    people
}

/// Split free text on `,` and `;` alone (no `Last, First` rule): the importers'
/// rule for a field that holds only names, such as an Ariane explorer list.
pub fn split_names(text: &str) -> Vec<Person> {
    let mut people = Vec::new();
    for name in text.split([',', ';']) {
        push_piece(&mut people, name);
    }
    people
}

fn push_piece(people: &mut Vec<Person>, piece: &str) {
    let piece = piece.trim();
    if piece.is_empty() || PLACEHOLDERS.iter().any(|p| piece.eq_ignore_ascii_case(p)) {
        return;
    }
    if let Some(person) = Person::named(piece) {
        add_person(people, person);
    }
}

/// `Last, First`: exactly two comma parts, each a single non-empty word, and no
/// `;` anywhere.
fn looks_like_last_first(half: &str) -> bool {
    if half.contains(';') {
        return false;
    }
    let parts: Vec<&str> = half.split(',').map(str::trim).collect();
    parts.len() == 2
        && parts
            .iter()
            .all(|p| !p.is_empty() && !p.contains(char::is_whitespace))
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
    /// The person as a credit edit stores it: trimmed, clamped to
    /// [`MAX_PERSON_TEXT_CHARS`], an empty email dropped. `None` for an empty
    /// name.
    pub fn sanitized(&self) -> Option<Person> {
        Person::new(
            clamp_chars(self.name.trim()),
            self.email.as_deref().map(|e| clamp_chars(e.trim())),
        )
    }
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
    let mut list: Vec<Person> = match set {
        Some(set) => dedupe_people(set.iter().filter_map(Person::sanitized).collect()),
        None => current.to_vec(),
    };
    for person in remove.unwrap_or_default() {
        remove_person(&mut list, person);
    }
    for person in add.unwrap_or_default().iter().filter_map(Person::sanitized) {
        if list.len() >= MAX_PEOPLE_PER_ROLE {
            break;
        }
        add_person(&mut list, person);
    }
    list.truncate(MAX_PEOPLE_PER_ROLE);
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
    pub fn from_credits<'a>(credits: impl IntoIterator<Item = &'a SurveyCredit>) -> Self {
        let mut surveyors = Tally::default();
        let mut explorers = Tally::default();
        let mut summary = Self::default();
        for credit in credits {
            summary.shot_count = summary.shot_count.saturating_add(1);
            surveyors.add_shot(&credit.surveyors);
            explorers.add_shot(&credit.explorers);
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
    fn add_shot(&mut self, people: &[Person]) {
        // A person listed twice on one shot counts once for it.
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
                    shots: 1,
                });
            } else if let Some(entry) = self.people.get_mut(index) {
                entry.shots = entry.shots.saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests;
