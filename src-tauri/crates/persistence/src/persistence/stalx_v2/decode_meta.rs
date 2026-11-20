//! Readers for the small members and the record lists read one object at a
//! time: the header objects (`conventions`, `project`, `settings`, `counters`,
//! `speleodb`), the lenient `local` and `history`, and the `sections`,
//! `datums`, `placements`, `closures`, `ignored_warnings` and `imports`
//! records.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::de::{self, DeserializeOwned, DeserializeSeed, Deserializer, SeqAccess, Visitor};
use serde_json::value::RawValue;

use crate::models::config::ValidationConfig;
use crate::models::survey::{ClosureStrategy, LoopClosure, Placement, ProjectSettings, ZeroDatum};
use crate::models::{GpsCoordinates, Section};

use super::canon::{canonical_checked, CanonError};
use super::decode_records::{AzimuthSeed, Ctx, Loc};
use super::error::DecodeError;
use super::scan::{json_error, members, Member, MAX_SMALL_MEMBERS};
use super::tokens;
use super::types::{
    CountLimits, DecodeNote, History, IgnoredWarning, Limit, LocalState, LocalSync, RawJson,
    SpeleoDbLink,
};
use super::write::parse_time;
use super::{CONVENTIONS, MAX_INTEGER};

/// Run `seed` over `raw`. Map a failure with [`typed`] once the seed (and its
/// borrow of the context) is gone.
pub(super) fn run<'a, S: DeserializeSeed<'a>>(
    raw: &'a str,
    seed: S,
) -> Result<S::Value, serde_json::Error> {
    let mut de = serde_json::Deserializer::from_str(raw);
    let value = seed.deserialize(&mut de)?;
    de.end()?;
    Ok(value)
}

/// The typed error of a failed [`run`]: the one a reader stashed in the
/// context, else serde's at the context's path (`raw` a slice of `whole`).
pub(super) fn typed(
    error: serde_json::Error,
    ctx: &mut Ctx<'_>,
    whole: &str,
    raw: &str,
) -> DecodeError {
    match ctx.fail.take() {
        Some(typed) => typed,
        None => json_error(error, &ctx.path(), whole, raw),
    }
}

/// An object's members, taken one by one; what is left is unknown.
pub(super) struct Fields<'a, 'w> {
    at: String,
    whole: &'w str,
    pairs: Vec<Option<Member<'a>>>,
}

impl<'a, 'w> Fields<'a, 'w> {
    pub(super) fn of(
        raw: &'a RawValue,
        at: impl Into<String>,
        whole: &'w str,
    ) -> Result<Self, DecodeError> {
        let at = at.into();
        let pairs = members(raw.get(), MAX_SMALL_MEMBERS)
            .map_err(|e| e.into_decode(&at, whole, raw.get()))?;
        Ok(Self {
            at,
            whole,
            pairs: pairs.into_iter().map(Some).collect(),
        })
    }

    pub(super) fn at(&self) -> &str {
        &self.at
    }

    pub(super) fn take(&mut self, key: &str) -> Option<&'a RawValue> {
        self.pairs
            .iter_mut()
            .find(|p| p.as_ref().is_some_and(|(k, _)| k == key))
            .and_then(Option::take)
            .map(|(_, v)| v)
    }

    pub(super) fn opt<T: DeserializeOwned>(&mut self, key: &str) -> Result<Option<T>, DecodeError> {
        let Some(raw) = self.take(key) else {
            return Ok(None);
        };
        serde_json::from_str(raw.get()).map(Some).map_err(|e| {
            json_error(
                e,
                &format!("{}, field {key}", self.at),
                self.whole,
                raw.get(),
            )
        })
    }

    pub(super) fn req<T: DeserializeOwned>(&mut self, key: &str) -> Result<T, DecodeError> {
        self.opt(key)?.ok_or_else(|| DecodeError::Missing {
            at: self.at.clone(),
            field: key.to_string(),
        })
    }

    pub(super) fn time(&mut self, key: &str) -> Result<Option<DateTime<Utc>>, DecodeError> {
        let Some(text) = self.opt::<String>(key)? else {
            return Ok(None);
        };
        parse_time(&text)
            .map(Some)
            .ok_or_else(|| DecodeError::BadValue {
                at: format!("{}, field {key}", self.at),
                message: format!("{text:?} is not an RFC 3339 time"),
            })
    }

    pub(super) fn req_time(&mut self, key: &str) -> Result<DateTime<Utc>, DecodeError> {
        self.time(key)?.ok_or_else(|| DecodeError::Missing {
            at: self.at.clone(),
            field: key.to_string(),
        })
    }

    /// An integer id or counter: at most [`MAX_INTEGER`].
    pub(super) fn big(&mut self, key: &str) -> Result<Option<u64>, DecodeError> {
        let value: Option<u64> = self.opt(key)?;
        match value {
            Some(v) if v > MAX_INTEGER => Err(DecodeError::BadValue {
                at: format!("{}, field {key}", self.at),
                message: format!("{v} is above {MAX_INTEGER}"),
            }),
            other => Ok(other),
        }
    }

    /// The members not taken, in file order.
    pub(super) fn rest(self) -> Vec<Member<'a>> {
        self.pairs.into_iter().flatten().collect()
    }

    /// Note every member not taken as unknown structural content (tier 1).
    pub(super) fn unknown_to(self, ctx: &mut Ctx<'_>) {
        let at = self.at.clone();
        for (key, _) in self.rest() {
            ctx.unknown(|_| format!("{at} key {key:?}"));
        }
    }
}

/// Canonical copies of unknown members (tier 2), counted against the
/// preserved-content cap.
pub(super) fn preserve(
    rest: Vec<Member<'_>>,
    at: &str,
    ctx: &mut Ctx<'_>,
) -> Result<Vec<(String, RawJson)>, DecodeError> {
    let mut out = Vec::with_capacity(rest.len());
    for (key, value) in rest {
        ctx.count_typed(Limit::Preserved, 1)?;
        let text = canonical_checked(value.get()).map_err(|e| match e {
            CanonError::TooDeep => DecodeError::TooDeep {
                at: format!("{at}, member {key:?}"),
            },
            CanonError::Invalid(message) => DecodeError::BadValue {
                at: format!("{at}, member {key:?}"),
                message,
            },
        })?;
        out.push((key.into_owned(), RawJson::from_canonical(text)));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Header members
// ---------------------------------------------------------------------------

pub(super) fn read_conventions(raw: &RawValue, whole: &str) -> Result<(), DecodeError> {
    let mut fields = Fields::of(raw, "the conventions member", whole)?;
    for (key, expected) in CONVENTIONS {
        let found: Option<String> = fields.opt(key)?;
        if found.as_deref() != Some(expected) {
            return Err(DecodeError::Conventions {
                key: key.to_string(),
                found: found.unwrap_or_else(|| "(missing)".to_string()),
            });
        }
    }
    if let Some((key, value)) = fields.rest().into_iter().next() {
        return Err(DecodeError::Conventions {
            key: key.into_owned(),
            found: value.get().to_string(),
        });
    }
    Ok(())
}

/// The `project` member's fields.
pub(super) struct ProjectMeta {
    pub(super) name: String,
    pub(super) location: Option<String>,
    pub(super) created: DateTime<Utc>,
    pub(super) modified: DateTime<Utc>,
}

pub(super) fn read_project(
    raw: &RawValue,
    whole: &str,
    ctx: &mut Ctx<'_>,
) -> Result<(ProjectMeta, Vec<(String, RawJson)>), DecodeError> {
    let mut fields = Fields::of(raw, "the project member", whole)?;
    let meta = ProjectMeta {
        name: fields.req("name")?,
        location: fields.opt("location")?,
        created: fields.req_time("created")?,
        modified: fields.req_time("modified")?,
    };
    let at = fields.at().to_string();
    let preserved = preserve(fields.rest(), &at, ctx)?;
    Ok((meta, preserved))
}

/// `settings` plus its preserved unknown keys and those of `validation`.
pub(super) struct SettingsRead {
    pub(super) settings: ProjectSettings,
    pub(super) preserved: Vec<(String, RawJson)>,
    pub(super) validation: Vec<(String, RawJson)>,
}

pub(super) fn read_settings(
    raw: &RawValue,
    whole: &str,
    ctx: &mut Ctx<'_>,
) -> Result<SettingsRead, DecodeError> {
    let mut fields = Fields::of(raw, "the settings member", whole)?;
    let mut settings = ProjectSettings::default();
    let mut validation_preserved = Vec::new();
    if let Some(raw) = fields.take("validation") {
        let mut v = Fields::of(raw, "settings.validation", whole)?;
        let d = ValidationConfig::default();
        settings.validation = ValidationConfig {
            warn_shot_length_min: v
                .opt("warn_shot_length_min")?
                .unwrap_or(d.warn_shot_length_min),
            warn_shot_length_max: v
                .opt("warn_shot_length_max")?
                .unwrap_or(d.warn_shot_length_max),
            warn_depth_change_per_shot_max: v
                .opt("warn_depth_change_per_shot_max")?
                .unwrap_or(d.warn_depth_change_per_shot_max),
            warn_azimuth_deviation_max: v
                .opt("warn_azimuth_deviation_max")?
                .unwrap_or(d.warn_azimuth_deviation_max),
            azimuth_diff_threshold: v
                .opt("azimuth_diff_threshold")?
                .unwrap_or(d.azimuth_diff_threshold),
            pitch_diff_threshold: v
                .opt("pitch_diff_threshold")?
                .unwrap_or(d.pitch_diff_threshold),
            inclination_depth_diff_threshold: v
                .opt("inclination_depth_diff_threshold")?
                .unwrap_or(d.inclination_depth_diff_threshold),
        };
        validation_preserved = preserve(v.rest(), "settings.validation", ctx)?;
    }
    settings.surface_elevation = fields.opt("surface_elevation")?;
    if let Some(raw) = fields.take("azimuth") {
        ctx.loc = Loc::Member("settings.azimuth");
        ctx.field = "";
        let reference = run(raw.get(), AzimuthSeed { ctx: &mut *ctx })
            .map_err(|e| typed(e, ctx, whole, raw.get()))?;
        settings.default_azimuth_reference = Some(reference);
    }
    let preserved = preserve(fields.rest(), "the settings member", ctx)?;
    Ok(SettingsRead {
        settings,
        preserved,
        validation: validation_preserved,
    })
}

/// Explicit counters, each `None` when the file leaves it out.
#[derive(Debug, Default)]
pub(super) struct RawCounters {
    pub(super) station: Option<u32>,
    pub(super) closure: Option<u64>,
    pub(super) section: Option<u32>,
}

pub(super) fn read_counters(
    raw: &RawValue,
    whole: &str,
    ctx: &mut Ctx<'_>,
) -> Result<RawCounters, DecodeError> {
    let mut fields = Fields::of(raw, "the counters member", whole)?;
    let counters = RawCounters {
        station: fields.opt("station")?,
        closure: fields.big("closure")?,
        section: fields.opt("section")?,
    };
    fields.unknown_to(ctx);
    Ok(counters)
}

pub(super) fn read_speleodb(
    raw: &RawValue,
    whole: &str,
    ctx: &mut Ctx<'_>,
) -> Result<(SpeleoDbLink, Vec<(String, RawJson)>), DecodeError> {
    let mut fields = Fields::of(raw, "the speleodb member", whole)?;
    let link = SpeleoDbLink {
        endpoint: fields.req("endpoint")?,
        project_id: fields.req("project")?,
        project_name: fields.req("name")?,
        undo_mode: fields
            .opt::<String>("undo_mode")?
            .map(|token| tokens::parse_undo_mode(&token)),
    };
    let preserved = preserve(fields.rest(), "the speleodb member", ctx)?;
    Ok((link, preserved))
}

/// The lenient `local` member (tier 3): what cannot be read is dropped with a
/// note and a log line, never refused.
pub(super) fn read_local(raw: &RawValue, limits: &CountLimits) -> (LocalState, Vec<DecodeNote>) {
    let mut notes = Vec::new();
    let mut drop = |what: String| {
        tracing::info!(what = %what, "Dropped unreadable per-machine state from a .stalx file");
        notes.push(DecodeNote::LocalDropped { what });
    };
    let mut local = LocalState::default();
    let Ok(pairs) = members(raw.get(), MAX_SMALL_MEMBERS) else {
        drop("the local member (not an object)".to_string());
        return (local, notes);
    };
    for (key, value) in pairs {
        match key.as_ref() {
            "display" => match members(value.get(), limits.display_settings) {
                Ok(settings) => {
                    for (k, v) in settings {
                        match serde_json::from_str::<String>(v.get()) {
                            Ok(text) => {
                                local.display.insert(k.into_owned(), text);
                            }
                            Err(_) => drop(format!("display setting {k:?}")),
                        }
                    }
                }
                Err(_) => drop("the display settings".to_string()),
            },
            "speleodb" => local.speleodb = read_local_sync(value, &mut drop),
            other => drop(format!("local key {other:?}")),
        }
    }
    (local, notes)
}

fn read_local_sync(raw: &RawValue, drop: &mut impl FnMut(String)) -> LocalSync {
    let mut sync = LocalSync::default();
    let Ok(pairs) = members(raw.get(), MAX_SMALL_MEMBERS) else {
        drop("local.speleodb (not an object)".to_string());
        return sync;
    };
    for (key, value) in pairs {
        let text = value.get();
        let ok = match key.as_ref() {
            "base" => serde_json::from_str(text)
                .map(|v| sync.base_commit = Some(v))
                .is_ok(),
            "unpushed" => serde_json::from_str(text)
                .map(|v| sync.unpushed_edits = v)
                .is_ok(),
            "last_edit" => time_value(text)
                .map(|v| sync.last_edit_at = Some(v))
                .is_some(),
            "last_verify" => time_value(text)
                .map(|v| sync.last_verify_at = Some(v))
                .is_some(),
            "unwound_from" => serde_json::from_str(text)
                .map(|v| sync.unwound_from = Some(v))
                .is_ok(),
            "inherited_through" => time_value(text)
                .map(|v| sync.inherited_through = Some(v))
                .is_some(),
            _ => false,
        };
        if !ok {
            drop(format!("local.speleodb key {key:?}"));
        }
    }
    sync
}

fn time_value(text: &str) -> Option<DateTime<Utc>> {
    serde_json::from_str::<String>(text)
        .ok()
        .and_then(|t| parse_time(&t))
}

/// The lenient `history` member (tier 3): `Err` drops it whole.
pub(super) fn read_history(raw: &RawValue, limits: &CountLimits) -> Result<History, String> {
    let pairs = members(raw.get(), MAX_SMALL_MEMBERS)
        .map_err(|_| "the history member is not an object".to_string())?;
    let mut history = History::default();
    let mut total = 0usize;
    for (key, value) in pairs {
        let list = match key.as_ref() {
            "undo" => &mut history.undo,
            "redo" => &mut history.redo,
            other => {
                tracing::info!(key = %other, "Dropped an unknown history key from a .stalx file");
                continue;
            }
        };
        let entries = collect_raw(value.get(), limits.history_entries.saturating_sub(total))
            .map_err(|e| format!("history.{key}: {e}"))?;
        total += entries.len();
        for (i, entry) in entries.into_iter().enumerate() {
            let text = canonical_checked(entry.get())
                .map_err(|e| format!("history.{key}[{i}]: {}", e.message()))?;
            list.push(RawJson::from_canonical(text));
        }
    }
    Ok(history)
}

/// The elements of an array, at most `max` (else an error).
fn collect_raw(text: &str, max: usize) -> Result<Vec<&RawValue>, String> {
    struct Collect {
        max: usize,
    }
    impl<'de> Visitor<'de> for Collect {
        type Value = Vec<&'de RawValue>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an array")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(v) = seq.next_element::<&'de RawValue>()? {
                if out.len() >= self.max {
                    return Err(de::Error::custom(format!("more than {} entries", self.max)));
                }
                out.push(v);
            }
            Ok(out)
        }
    }
    let mut de = serde_json::Deserializer::from_str(text);
    let out = de
        .deserialize_seq(Collect { max })
        .map_err(|e| e.to_string())?;
    de.end().map_err(|e| e.to_string())?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Record lists read one object at a time
// ---------------------------------------------------------------------------

/// The elements of a record list, counted against `limit` as they arrive.
pub(super) fn records<'a>(
    raw: &'a RawValue,
    member: &'static str,
    limit: Limit,
    ctx: &mut Ctx<'_>,
    whole: &str,
) -> Result<Vec<&'a RawValue>, DecodeError> {
    struct Collect<'c, 'l> {
        ctx: &'c mut Ctx<'l>,
        limit: Limit,
    }
    impl<'de> DeserializeSeed<'de> for Collect<'_, '_> {
        type Value = Vec<&'de RawValue>;
        fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_seq(self)
        }
    }
    impl<'de> Visitor<'de> for Collect<'_, '_> {
        type Value = Vec<&'de RawValue>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an array of records")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(v) = seq.next_element::<&'de RawValue>()? {
                self.ctx.count(self.limit, 1)?;
                out.push(v);
            }
            Ok(out)
        }
    }
    ctx.loc = Loc::Member(member);
    ctx.field = "";
    run(
        raw.get(),
        Collect {
            ctx: &mut *ctx,
            limit,
        },
    )
    .map_err(|e| typed(e, ctx, whole, raw.get()))
}

pub(super) fn read_section(
    raw: &RawValue,
    at: String,
    whole: &str,
    ctx: &mut Ctx<'_>,
) -> Result<Section, DecodeError> {
    let mut f = Fields::of(raw, at, whole)?;
    let section = Section {
        id: f.req("id")?,
        name: f.req("name")?,
        color: f.req("color")?,
    };
    f.unknown_to(ctx);
    Ok(section)
}

pub(super) fn read_datum(
    raw: &RawValue,
    at: String,
    whole: &str,
    ctx: &mut Ctx<'_>,
) -> Result<ZeroDatum, DecodeError> {
    let mut f = Fields::of(raw, at, whole)?;
    let datum = ZeroDatum {
        station: f.req("station")?,
        via: f.opt("via")?,
        gps: GpsCoordinates {
            latitude: f.req("lat")?,
            longitude: f.req("lon")?,
            depth_reference: f.opt("depth")?.unwrap_or(0.0),
        },
        accuracy_m: f.opt("accuracy")?,
        entrance: f.opt("entrance")?,
        correction: f.opt("correction")?,
    };
    f.unknown_to(ctx);
    Ok(datum)
}

pub(super) fn read_placement(
    raw: &RawValue,
    at: String,
    whole: &str,
    ctx: &mut Ctx<'_>,
) -> Result<Placement, DecodeError> {
    let mut f = Fields::of(raw, at, whole)?;
    let placement = Placement {
        station: f.req("station")?,
        x: f.req("x")?,
        y: f.req("y")?,
    };
    f.unknown_to(ctx);
    Ok(placement)
}

pub(super) fn read_closure(
    raw: &RawValue,
    at: String,
    whole: &str,
    ctx: &mut Ctx<'_>,
) -> Result<LoopClosure, DecodeError> {
    let mut f = Fields::of(raw, at, whole)?;
    let id = f.big("id")?.ok_or_else(|| DecodeError::Missing {
        at: f.at().to_string(),
        field: "id".to_string(),
    })?;
    let a = f.req("a")?;
    let a_via = f.opt("a_via")?;
    let b = f.req("b")?;
    let b_via = f.opt("b_via")?;
    let strategy = match f.opt::<String>("strategy")? {
        None => ClosureStrategy::Batch,
        Some(token) => tokens::parse_strategy(&token).unwrap_or_else(|| {
            let at = f.at().to_string();
            ctx.unknown(|_| format!("{at} strategy {token:?}"));
            ClosureStrategy::Batch
        }),
    };
    f.unknown_to(ctx);
    Ok(LoopClosure {
        id,
        a,
        a_via,
        b,
        b_via,
        strategy,
    })
}

pub(super) fn read_ignore(
    raw: &RawValue,
    at: String,
    whole: &str,
    ctx: &mut Ctx<'_>,
) -> Result<IgnoredWarning, DecodeError> {
    let mut f = Fields::of(raw, at, whole)?;
    let warning = IgnoredWarning {
        shot: f.req("shot")?,
        code: f.req("code")?,
        at: f.req_time("at")?,
        comment: f.opt("comment")?,
    };
    f.unknown_to(ctx);
    Ok(warning)
}

pub(super) fn read_imports(
    raw: &RawValue,
    whole: &str,
    ctx: &mut Ctx<'_>,
) -> Result<(Vec<String>, Vec<String>), DecodeError> {
    let mut f = Fields::of(raw, "the imports member", whole)?;
    let files: Vec<String> = f.opt("files")?.unwrap_or_default();
    let lines: Vec<String> = f.opt("lines")?.unwrap_or_default();
    f.unknown_to(ctx);
    ctx.count_typed(Limit::ImportHashes, files.len() + lines.len())?;
    Ok((files, lines))
}
