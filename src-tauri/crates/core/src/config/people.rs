//! The `[people]` config table: the people recently credited as surveyors or
//! explorers, offered as autocomplete suggestions in every project.
//!
//! Global on purpose, like `[map_palette]`: the list follows the user across
//! projects, so it lives in `config.toml` and never in a survey's `.stalx` or
//! anything sent to SpeleoDB. People are stored by value (a name and an
//! optional email), the same [`Person`] a shot's credit holds, and two entries
//! are the same person by [`Person::same_as`].

use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

use crate::models::{contains_person, dedupe_people, Person};

/// Most people the recent list keeps; the oldest fall off the end.
pub const MAX_RECENT_PEOPLE: usize = 200;

/// The `[people]` table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PeopleConfig {
    /// People recently added to a shot's credit, most recent first, distinct by
    /// identity and at most [`MAX_RECENT_PEOPLE`].
    #[serde(default, deserialize_with = "lenient_people")]
    pub recent: Vec<Person>,
}

/// A malformed entry must not fail the whole config parse:
/// `Config::load().unwrap_or_default()` would then reset every other setting
/// because one remembered person was hand-edited badly (the same reason
/// `lenient_column_widths` exists). Entries without a string `name` are
/// skipped; a non-string `email` is dropped.
fn lenient_people<'de, D>(deserializer: D) -> Result<Vec<Person>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let Some(items) = value.as_array() else {
        return Ok(Vec::new());
    };
    Ok(items
        .iter()
        .filter_map(|item| {
            let obj = item.as_object()?;
            let name = obj.get("name")?.as_str()?;
            let email = obj.get("email").and_then(serde_json::Value::as_str);
            Some(Person {
                name: name.to_string(),
                email: email.map(str::to_string),
            })
        })
        .collect())
}

impl PeopleConfig {
    /// Every entry sanitized ([`Person::sanitized`]: trimmed, clamped, an empty
    /// email dropped), invalid ones dropped, duplicates by identity removed (the
    /// first, most recent, wins) and the list capped at [`MAX_RECENT_PEOPLE`].
    pub fn normalize(&mut self) {
        let mut kept: Vec<Person> = Vec::with_capacity(self.recent.len().min(MAX_RECENT_PEOPLE));
        for person in self.recent.iter().filter_map(Person::sanitized) {
            if kept.len() >= MAX_RECENT_PEOPLE {
                break;
            }
            if !contains_person(&kept, &person) {
                kept.push(person);
            }
        }
        self.recent = kept;
    }

    /// Move `people` to the front of the list, in the given order (`people[0]`
    /// becomes the most recent). An entry already listed under the same identity
    /// is replaced, so the new spelling wins. Then [`Self::normalize`] (which
    /// caps the list). Empty or invalid input changes nothing.
    pub fn remember(&mut self, people: &[Person]) {
        // Cut to what the list can hold BEFORE sanitizing or deduplicating.
        let fresh = dedupe_people(
            people
                .iter()
                .take(MAX_RECENT_PEOPLE)
                .filter_map(Person::sanitized)
                .collect(),
        );
        if fresh.is_empty() {
            return;
        }
        self.recent
            .retain(|person| !contains_person(&fresh, person));
        let older = std::mem::take(&mut self.recent);
        self.recent = fresh;
        self.recent.extend(older);
        self.normalize();
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn person(name: &str, email: Option<&str>) -> Person {
        Person {
            name: name.to_string(),
            email: email.map(str::to_string),
        }
    }

    fn names(config: &PeopleConfig) -> Vec<&str> {
        config.recent.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn remember_takes_the_first_entries_before_deduplicating() {
        let mut config = PeopleConfig::default();
        let mut people = vec![person("A", None); MAX_RECENT_PEOPLE];
        people.push(person("Past The Cap", None));
        config.remember(&people);
        assert_eq!(names(&config), ["A"]);
    }

    #[test]
    fn normalize_sanitizes_dedupes_and_drops_invalid_people() {
        let mut config = PeopleConfig {
            recent: vec![
                person("  Ann  ", Some("  ")),
                person("   ", Some("ghost@x.org")),
                person("ann", None),
                person("Bob", Some("Bob@X.org")),
                person("bob", Some("bob@x.org")),
                person("Bob", None),
            ],
        };
        config.normalize();
        assert_eq!(
            config.recent,
            vec![
                person("Ann", None),
                person("Bob", Some("Bob@X.org")),
                person("Bob", None),
            ]
        );
    }

    #[test]
    fn normalize_caps_the_list_keeping_the_most_recent() {
        let mut config = PeopleConfig {
            recent: (0..MAX_RECENT_PEOPLE + 25)
                .map(|i| person(&format!("Person {i}"), None))
                .collect(),
        };
        config.normalize();
        assert_eq!(config.recent.len(), MAX_RECENT_PEOPLE);
        assert_eq!(config.recent.first().unwrap().name, "Person 0");
        assert_eq!(
            config.recent.last().unwrap().name,
            format!("Person {}", MAX_RECENT_PEOPLE - 1)
        );
    }

    #[test]
    fn remember_puts_the_given_people_first_in_order() {
        let mut config = PeopleConfig {
            recent: vec![person("Old", None), person("Older", None)],
        };
        config.remember(&[person("New A", None), person("New B", None)]);
        assert_eq!(names(&config), vec!["New A", "New B", "Old", "Older"]);
    }

    #[test]
    fn remember_moves_a_known_person_to_the_front_with_the_new_spelling() {
        let mut config = PeopleConfig {
            recent: vec![
                person("Ann", None),
                person("john  smith", Some("JS@caves.org")),
                person("Bob", None),
            ],
        };
        config.remember(&[person("John Smith", Some("js@caves.org"))]);
        assert_eq!(
            config.recent,
            vec![
                person("John Smith", Some("js@caves.org")),
                person("Ann", None),
                person("Bob", None),
            ]
        );
    }

    #[test]
    fn remember_keeps_a_same_named_person_with_another_email() {
        let mut config = PeopleConfig {
            recent: vec![person("Ann", Some("a@x.org"))],
        };
        config.remember(&[person("Ann", None)]);
        assert_eq!(
            config.recent,
            vec![person("Ann", None), person("Ann", Some("a@x.org"))]
        );
    }

    #[test]
    fn remember_dedupes_its_input_and_caps_the_list() {
        let mut config = PeopleConfig {
            recent: (0..MAX_RECENT_PEOPLE)
                .map(|i| person(&format!("Person {i}"), None))
                .collect(),
        };
        config.remember(&[
            person("Fresh", None),
            person(" fresh ", None),
            person("Person 5", None),
        ]);
        assert_eq!(config.recent.len(), MAX_RECENT_PEOPLE);
        assert_eq!(&names(&config)[..3], &["Fresh", "Person 5", "Person 0"]);
        // Person 5 moved up rather than being added twice, so only the single
        // oldest entry falls off the end.
        assert_eq!(
            config.recent.last().unwrap().name,
            format!("Person {}", MAX_RECENT_PEOPLE - 2)
        );
        assert_eq!(
            names(&config).iter().filter(|n| **n == "Person 5").count(),
            1
        );
    }

    #[test]
    fn remember_with_nothing_valid_changes_nothing() {
        let before = PeopleConfig {
            recent: vec![person("Ann", None)],
        };
        let mut config = before.clone();
        config.remember(&[]);
        config.remember(&[person("   ", Some("x@y.org"))]);
        assert_eq!(config, before);
    }

    #[test]
    fn a_malformed_entry_does_not_fail_the_parse() {
        let config: PeopleConfig = toml::from_str(
            r#"recent = [{ name = "Ann", email = "ann@x.org" }, { email = "nameless@x.org" }, 7, { name = "Bob", email = 3 }]"#,
        )
        .unwrap();
        assert_eq!(
            config.recent,
            vec![person("Ann", Some("ann@x.org")), person("Bob", None)]
        );
        let not_a_list: PeopleConfig = toml::from_str(r#"recent = "Ann""#).unwrap();
        assert!(not_a_list.recent.is_empty());
    }
}
