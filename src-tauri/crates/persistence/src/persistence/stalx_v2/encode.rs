//! The writer: a survey project plus its [`Extras`] to canonical bytes.
//!
//! Hand-written rather than derived: the format fixes the key order, puts one
//! record per physical line, leaves out what is unset and spells every scalar
//! one way (`write.rs`), none of which a derived `Serialize` gives.

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::models::survey::{Line, Project, Shot, Station};
use crate::models::{AzimuthReference, Person, SurveyCredit};

use super::error::EncodeError;
use super::summary::Summary;
use super::tokens;
use super::types::{Extras, History, IgnoredWarning, LocalState, Mode, PushOptions, RawJson};
use super::write::{
    boolean, date, float, integer, normalized_date, signed, small, string, time, Obj,
};
use super::{CONVENTIONS, DEFAULT_GENERATOR, KNOWN_FEATURES, PREFIX, SOLVER, VERSION};
use crate::models::UndoMode;

/// What an encode did beside writing the survey.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EncodeReport {
    /// The size of the bytes returned.
    pub size: usize,
    pub history: HistoryCut,
    /// Ignored warnings left out: they name no shot of the project, or repeat
    /// a (shot, code) pair.
    pub ignores_dropped: usize,
}

/// How the history was cut for the mode.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryCut {
    pub undo_kept: usize,
    /// Undo entries at or before the watermark (inherited from the link).
    pub inherited_cut: usize,
    /// The oldest undo entries left out to fit a push's byte cap.
    pub size_cut: usize,
    /// The whole undo history was left out (the mode, or fail closed).
    pub undo_dropped: bool,
    pub redo_dropped: usize,
}

/// Encode `project` and `extras` as a version 2 file in `mode`.
pub fn encode(project: &Project, extras: &Extras, mode: Mode) -> Result<Vec<u8>, EncodeError> {
    encode_with_report(project, extras, mode).map(|(bytes, _)| bytes)
}

/// [`encode`], also reporting what was left out.
pub fn encode_with_report(
    project: &Project,
    extras: &Extras,
    mode: Mode,
) -> Result<(Vec<u8>, EncodeReport), EncodeError> {
    if let Some(reason) = &extras.read_only {
        return Err(EncodeError::ReadOnly(reason.clone()));
    }
    if let Some(name) = extras
        .requires
        .iter()
        .find(|name| !KNOWN_FEATURES.contains(&name.as_str()))
    {
        return Err(EncodeError::UnknownFeature { name: name.clone() });
    }
    let violations = project.validate();
    if !violations.is_empty() {
        return Err(EncodeError::Invalid { violations });
    }

    let link = match mode {
        Mode::DetachedCopy => None,
        Mode::Normal | Mode::ForPush(_) => extras.speleodb.as_ref(),
    };
    let mut report = EncodeReport::default();
    let mut head = Vec::with_capacity(4096);
    write_head(&mut head, project, extras, link)?;
    if let Some(link) = link {
        write_speleodb(&mut head, link, &extras.preserved.speleodb);
    }
    match mode {
        Mode::Normal => write_local(&mut head, &extras.local),
        Mode::ForPush(_) => {}
        Mode::DetachedCopy => write_local(
            &mut head,
            &LocalState {
                display: extras.local.display.clone(),
                ..LocalState::default()
            },
        ),
    }
    // The history this mode writes (before a push's byte cut): an ignore of a
    // shot it can bring back is kept (5.13).
    let (undo, redo) = history_for(mode, extras, &mut report.history)?;
    let mut body = Vec::with_capacity(estimate(project));
    report.ignores_dropped = write_body(&mut body, project, extras, &[undo, redo])?;
    const END: &[u8] = b"\"end\":true}\n";

    let undo = match mode {
        Mode::ForPush(PushOptions { byte_cap, .. }) => {
            let base = head.len() + body.len() + END.len();
            if base > byte_cap {
                return Err(EncodeError::TooLarge {
                    size: base,
                    cap: byte_cap,
                });
            }
            let kept = fit_newest(undo, byte_cap - base);
            report.history.size_cut = undo.len() - kept.len();
            kept
        }
        Mode::Normal | Mode::DetachedCopy => undo,
    };
    report.history.undo_kept = undo.len();

    let mut out = head;
    out.extend_from_slice(&body);
    write_history(&mut out, undo, redo);
    out.extend_from_slice(END);
    report.size = out.len();
    Ok((out, report))
}

/// A rough size, to allocate once.
fn estimate(project: &Project) -> usize {
    let shots: usize = project.lines().iter().map(|l| l.shots.len()).sum();
    4096 + shots * 120 + project.stations.len() * 16
}

// ---------------------------------------------------------------------------
// Header members
// ---------------------------------------------------------------------------

fn write_head(
    out: &mut Vec<u8>,
    project: &Project,
    extras: &Extras,
    link: Option<&super::SpeleoDbLink>,
) -> Result<(), EncodeError> {
    out.extend_from_slice(PREFIX.as_bytes());
    small(out, VERSION);
    out.extend_from_slice(b",\"solver\":");
    small(out, SOLVER);
    out.extend_from_slice(b",\"requires\":");
    let mut requires: Vec<&str> = extras.requires.iter().map(String::as_str).collect();
    requires.sort_unstable();
    requires.dedup();
    strings(out, requires);
    out.extend_from_slice(b",\n");

    string(out, "conventions");
    out.push(b':');
    let mut o = Obj::open(out);
    for (key, value) in CONVENTIONS {
        string(o.key(key), value);
    }
    o.close();
    out.extend_from_slice(b",\n");

    string(out, "generator");
    out.push(b':');
    let generator = if extras.generator.is_empty() {
        DEFAULT_GENERATOR
    } else {
        extras.generator.as_str()
    };
    string(out, generator);
    out.extend_from_slice(b",\n");

    Summary::of(project, link).write(out, &extras.preserved.summary)?;
    out.extend_from_slice(b",\n");

    string(out, "project");
    out.push(b':');
    let mut o = Obj::open(out);
    string(o.key("name"), &project.name);
    if let Some(location) = &project.location {
        string(o.key("location"), location);
    }
    time(o.key("created"), project.created_at);
    time(o.key("modified"), project.modified_at);
    o.preserved(&extras.preserved.project);
    o.close();
    out.extend_from_slice(b",\n");

    write_settings(out, project, extras)?;

    string(out, "counters");
    out.push(b':');
    let mut o = Obj::open(out);
    small(
        o.key("station"),
        u64::from(project.counters.next_station_id),
    );
    integer(o.key("closure"), project.counters.next_closure_id, || {
        "the closure counter".to_string()
    })?;
    small(
        o.key("section"),
        u64::from(project.counters.next_section_id),
    );
    o.close();
    out.extend_from_slice(b",\n");
    Ok(())
}

fn write_settings(
    out: &mut Vec<u8>,
    project: &Project,
    extras: &Extras,
) -> Result<(), EncodeError> {
    let settings = &project.settings;
    string(out, "settings");
    out.push(b':');
    let mut o = Obj::open(out);
    {
        let v = &settings.validation;
        let mut vo = Obj::open(o.key("validation"));
        for (key, value) in [
            ("warn_shot_length_min", v.warn_shot_length_min),
            ("warn_shot_length_max", v.warn_shot_length_max),
            (
                "warn_depth_change_per_shot_max",
                v.warn_depth_change_per_shot_max,
            ),
            ("warn_azimuth_deviation_max", v.warn_azimuth_deviation_max),
            ("azimuth_diff_threshold", v.azimuth_diff_threshold),
            ("pitch_diff_threshold", v.pitch_diff_threshold),
            (
                "inclination_depth_diff_threshold",
                v.inclination_depth_diff_threshold,
            ),
        ] {
            float(vo.key(key), value, || format!("validation threshold {key}"))?;
        }
        vo.preserved(&extras.preserved.validation);
        vo.close();
    }
    if let Some(elevation) = settings.surface_elevation {
        float(o.key("surface_elevation"), elevation, || {
            "the surface elevation".to_string()
        })?;
    }
    if let Some(reference) = &settings.default_azimuth_reference {
        azimuth(o.key("azimuth"), reference, "the default azimuth reference")?;
    }
    o.preserved(&extras.preserved.settings);
    o.close();
    out.extend_from_slice(b",\n");
    Ok(())
}

pub(super) fn write_speleodb(
    out: &mut Vec<u8>,
    link: &super::SpeleoDbLink,
    preserved: &[(String, RawJson)],
) {
    string(out, "speleodb");
    out.push(b':');
    let mut o = Obj::open(out);
    string(o.key("endpoint"), &link.endpoint);
    string(o.key("project"), &link.project_id);
    string(o.key("name"), &link.project_name);
    if let Some(mode) = link.undo_mode {
        string(o.key("undo_mode"), tokens::undo_mode(mode));
    }
    o.preserved(preserved);
    o.close();
    out.extend_from_slice(b",\n");
}

pub(super) fn write_local(out: &mut Vec<u8>, local: &LocalState) {
    if local.is_empty() {
        return;
    }
    string(out, "local");
    out.push(b':');
    let mut o = Obj::open(out);
    if !local.display.is_empty() {
        let mut d = Obj::open(o.key("display"));
        for (key, value) in &local.display {
            // A toggle is a JSON boolean (demo finding I-16); any other
            // setting (a spacing, a mode, a list) is its stored text. The
            // decoder reads either back as the same text.
            match value.as_str() {
                "true" => boolean(d.key(key), true),
                "false" => boolean(d.key(key), false),
                _ => string(d.key(key), value),
            }
        }
        d.close();
    }
    let sync = &local.speleodb;
    if !sync.is_empty() {
        let mut s = Obj::open(o.key("speleodb"));
        if let Some(base) = &sync.base_commit {
            string(s.key("base"), base);
        }
        if let Some(digest) = &sync.base_survey {
            string(s.key("base_survey"), digest);
        }
        if let Some(at) = sync.base_survey_edit {
            time(s.key("base_survey_edit"), at);
        }
        if sync.unpushed_edits {
            boolean(s.key("unpushed"), true);
        }
        if let Some(at) = sync.last_edit_at {
            time(s.key("last_edit"), at);
        }
        if let Some(at) = sync.last_verify_at {
            time(s.key("last_verify"), at);
        }
        if let Some(sha) = &sync.unwound_from {
            string(s.key("unwound_from"), sha);
        }
        if let Some(at) = sync.inherited_through {
            time(s.key("inherited_through"), at);
        }
        s.preserved(&sync.preserved);
        s.close();
    }
    o.preserved(&local.preserved);
    o.close();
    out.extend_from_slice(b",\n");
}

// ---------------------------------------------------------------------------
// Survey records
// ---------------------------------------------------------------------------

/// Write the survey members (sections through imports) and the preserved
/// unknown top-level members. Returns how many ignores were left out.
fn write_body(
    out: &mut Vec<u8>,
    project: &Project,
    extras: &Extras,
    history: &[&[RawJson]],
) -> Result<usize, EncodeError> {
    records(out, "sections", &project.sections, |out, section| {
        let mut o = Obj::open(out);
        small(o.key("id"), u64::from(section.id));
        string(o.key("name"), &section.name);
        string(o.key("color"), &section.color);
        o.close();
        Ok(())
    })?;
    let stations: Vec<&Station> = project.stations.values().collect();
    records(out, "stations", &stations, |out, station| {
        write_station(out, station);
        Ok(())
    })?;
    write_lines(out, project)?;
    records(out, "datums", &project.datums, |out, datum| {
        let at = |what: &str| format!("datum at station {} {what}", datum.station);
        let mut o = Obj::open(out);
        small(o.key("station"), u64::from(datum.station));
        if let Some(via) = datum.via {
            small(o.key("via"), u64::from(via));
        }
        float(o.key("lat"), datum.gps.latitude, || at("latitude"))?;
        float(o.key("lon"), datum.gps.longitude, || at("longitude"))?;
        float(o.key("depth"), datum.gps.depth_reference, || at("depth"))?;
        if let Some(accuracy) = datum.accuracy_m {
            float(o.key("accuracy"), accuracy, || at("accuracy"))?;
        }
        if let Some(entrance) = datum.entrance {
            boolean(o.key("entrance"), entrance);
        }
        if let Some([dx, dy]) = datum.correction {
            let b = o.key("correction");
            b.push(b'[');
            float(b, dx, || at("correction"))?;
            b.push(b',');
            float(b, dy, || at("correction"))?;
            b.push(b']');
        }
        o.close();
        Ok(())
    })?;
    records(out, "placements", &project.placements, |out, placement| {
        let at = || format!("placement at station {}", placement.station);
        let mut o = Obj::open(out);
        small(o.key("station"), u64::from(placement.station));
        float(o.key("x"), placement.x, at)?;
        float(o.key("y"), placement.y, at)?;
        o.close();
        Ok(())
    })?;
    records(out, "closures", &project.closures, |out, closure| {
        let mut o = Obj::open(out);
        integer(o.key("id"), closure.id, || {
            format!("closure id {}", closure.id)
        })?;
        small(o.key("a"), u64::from(closure.a));
        if let Some(via) = closure.a_via {
            small(o.key("a_via"), u64::from(via));
        }
        small(o.key("b"), u64::from(closure.b));
        if let Some(via) = closure.b_via {
            small(o.key("b_via"), u64::from(via));
        }
        string(o.key("strategy"), tokens::strategy(closure.strategy));
        o.close();
        Ok(())
    })?;
    let (ignores, dropped) = canonical_ignores(project, &extras.ignored_warnings, history);
    records(out, "ignored_warnings", &ignores, |out, warning| {
        let mut o = Obj::open(out);
        string(o.key("shot"), &warning.shot);
        string(o.key("code"), &warning.code);
        time(o.key("at"), warning.at);
        if let Some(comment) = &warning.comment {
            string(o.key("comment"), comment);
        }
        o.close();
        Ok(())
    })?;
    if !project.imported_file_hashes.is_empty() || !project.imported_line_hashes.is_empty() {
        string(out, "imports");
        out.push(b':');
        let mut o = Obj::open(out);
        if !project.imported_file_hashes.is_empty() {
            strings(
                o.key("files"),
                project.imported_file_hashes.iter().map(String::as_str),
            );
        }
        if !project.imported_line_hashes.is_empty() {
            strings(
                o.key("lines"),
                project.imported_line_hashes.iter().map(String::as_str),
            );
        }
        o.close();
        out.extend_from_slice(b",\n");
    }
    for (key, value) in &extras.preserved.top {
        string(out, key);
        out.push(b':');
        out.extend_from_slice(value.as_str().as_bytes());
        out.extend_from_slice(b",\n");
    }
    Ok(dropped)
}

/// The ignores the file holds: those naming a shot of the project or one
/// `history` can bring back ([`super::ignores`]), sorted by (shot, code,
/// time, comment), one per (shot, code).
pub(super) fn canonical_ignores<'a>(
    project: &Project,
    ignores: &'a [IgnoredWarning],
    history: &[&[RawJson]],
) -> (Vec<&'a IgnoredWarning>, usize) {
    let mut kept = super::ignores::reachable(project, ignores, history);
    kept.sort();
    kept.dedup_by(|b, a| a.shot == b.shot && a.code == b.code);
    let dropped = ignores.len() - kept.len();
    (kept, dropped)
}

/// `"key":[\n rec,\n rec\n],\n`, omitted when empty.
fn records<T>(
    out: &mut Vec<u8>,
    key: &str,
    items: &[T],
    mut write: impl FnMut(&mut Vec<u8>, &T) -> Result<(), EncodeError>,
) -> Result<(), EncodeError> {
    if items.is_empty() {
        return Ok(());
    }
    string(out, key);
    out.extend_from_slice(b":[\n");
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(b",\n");
        }
        write(out, item)?;
    }
    out.extend_from_slice(b"\n],\n");
    Ok(())
}

fn write_station(out: &mut Vec<u8>, station: &Station) {
    out.push(b'[');
    small(out, u64::from(station.id));
    out.push(b',');
    string(out, &station.name);
    if station.comment.is_some() || !station.aliases.is_empty() || station.external.is_some() {
        out.push(b',');
        let mut o = Obj::open(out);
        if let Some(comment) = &station.comment {
            string(o.key("comment"), comment);
        }
        if !station.aliases.is_empty() {
            let b = o.key("aliases");
            b.push(b'[');
            for (i, alias) in station.aliases.iter().enumerate() {
                if i > 0 {
                    b.push(b',');
                }
                // `[id]`, `[id, name]`, or with the line that knew the
                // station by it `[id, name, line]` (`null` for no name).
                b.push(b'[');
                small(b, u64::from(alias.id));
                match (&alias.name, alias.line) {
                    (Some(name), line) => {
                        b.push(b',');
                        string(b, name);
                        if let Some(line) = line {
                            b.push(b',');
                            small(b, u64::from(line));
                        }
                    }
                    (None, Some(line)) => {
                        b.extend_from_slice(b",null,");
                        small(b, u64::from(line));
                    }
                    (None, None) => {}
                }
                b.push(b']');
            }
            b.push(b']');
        }
        if let Some(external) = &station.external {
            string(o.key("external"), external);
        }
        o.close();
    }
    out.push(b']');
}

fn write_lines(out: &mut Vec<u8>, project: &Project) -> Result<(), EncodeError> {
    if project.lines().is_empty() {
        return Ok(());
    }
    string(out, "lines");
    out.extend_from_slice(b":[\n");
    for (i, line) in project.lines().iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(b",\n");
        }
        let credits = line_credit(line);
        write_line_header(out, line, credits.shared.as_deref())?;
        for (index, shot) in line.shots.iter().enumerate() {
            out.extend_from_slice(b",\n");
            let credit_differs = credits.differs.get(index).copied().unwrap_or(true);
            write_shot(out, line, index, shot, credit_differs)?;
        }
    }
    out.extend_from_slice(b"\n],\n");
    Ok(())
}

fn write_line_header(
    out: &mut Vec<u8>,
    line: &Line,
    shared: Option<&SurveyCredit>,
) -> Result<(), EncodeError> {
    let mut o = Obj::open(out);
    small(o.key("id"), u64::from(line.id));
    if let Some(name) = &line.name {
        string(o.key("name"), name);
    }
    string(o.key("color"), &line.color);
    string(o.key("direction"), tokens::direction(line.direction));
    float(o.key("quality"), line.quality, || {
        format!("quality of line {}", line.id)
    })?;
    if let Some(section) = line.section_id {
        small(o.key("section"), u64::from(section));
    }
    if let Some(reference) = &line.azimuth_reference {
        azimuth(
            o.key("azimuth"),
            reference,
            &format!("azimuth reference of line {}", line.id),
        )?;
    }
    if let Some(surveyor) = &line.legacy_surveyor {
        string(o.key("surveyor"), surveyor);
    }
    if let Some(day) = line.legacy_date {
        date(o.key("date"), day);
    }
    if let Some(credit) = shared {
        write_credit(o.key("credit"), credit);
    }
    small(o.key("start"), u64::from(line.start));
    if !line.hand_moves.is_empty() {
        let b = o.key("hand");
        b.push(b'[');
        for (i, (station, [dx, dy, dz])) in line.hand_moves.iter().enumerate() {
            if i > 0 {
                b.push(b',');
            }
            b.push(b'[');
            small(b, u64::from(*station));
            for value in [dx, dy, dz] {
                b.push(b',');
                float(b, *value, || {
                    format!("hand move of line {} at station {station}", line.id)
                })?;
            }
            b.push(b']');
        }
        b.push(b']');
    }
    o.close();
    Ok(())
}

fn write_shot(
    out: &mut Vec<u8>,
    line: &Line,
    index: usize,
    shot: &Shot,
    credit_differs: bool,
) -> Result<(), EncodeError> {
    let at = |field: &str| format!("{field} of shot {index} of line {}", line.id);
    out.push(b'[');
    string(out, &shot.uuid);
    out.push(b',');
    small(out, u64::from(shot.to));
    for (field, value) in [
        ("length", shot.length),
        ("azimuth_in", shot.azimuth_in),
        ("azimuth_out", shot.azimuth_out),
        ("depth_in", shot.depth_in),
        ("depth_out", shot.depth_out),
    ] {
        out.push(b',');
        float(out, value, || at(field))?;
    }
    let lrud = [shot.left, shot.right, shot.up, shot.down];
    let keep = lrud.iter().rposition(Option::is_some).map_or(0, |p| p + 1);
    for (field, value) in ["left", "right", "up", "down"].iter().zip(&lrud[..keep]) {
        out.push(b',');
        match value {
            Some(v) => float(out, *v, || at(field))?,
            None => out.extend_from_slice(b"null"),
        }
    }
    let own = shot.credit.as_deref();
    if tokens::kind(shot.kind).is_some()
        || shot.pitch_in.is_some()
        || shot.pitch_out.is_some()
        || shot.temperature.is_some()
        || shot.marker_index.is_some()
        || shot.comment.is_some()
        || credit_differs
    {
        out.push(b',');
        let mut o = Obj::open(out);
        if let Some(token) = tokens::kind(shot.kind) {
            string(o.key("kind"), token);
        }
        if let Some(v) = shot.pitch_in {
            float(o.key("pitch_in"), v, || at("pitch_in"))?;
        }
        if let Some(v) = shot.pitch_out {
            float(o.key("pitch_out"), v, || at("pitch_out"))?;
        }
        if let Some(v) = shot.temperature {
            float(o.key("temperature"), v, || at("temperature"))?;
        }
        if let Some(marker) = shot.marker_index {
            small(o.key("marker"), u64::from(marker));
        }
        if let Some(comment) = &shot.comment {
            string(o.key("comment"), comment);
        }
        if credit_differs {
            match own {
                Some(credit) => write_credit(o.key("credit"), credit),
                None => o.key("credit").extend_from_slice(b"null"),
            }
        }
        o.close();
    }
    out.push(b']');
    Ok(())
}

/// The credit most of the line's shots carry, as written (absence counts as
/// a value); a tie goes to the one that comes first. Written once in the line
/// header, so a row writes a credit only where it differs.
pub(super) struct LineCredits<'a> {
    pub(super) shared: Option<Cow<'a, SurveyCredit>>,
    /// Per shot: its credit is not the header's.
    differs: Vec<bool>,
}

/// A credit as the file holds it: its date clamped like every date. Borrowed
/// unless the date needed clamping (only an out-of-range date does).
fn written_credit(credit: &SurveyCredit) -> Cow<'_, SurveyCredit> {
    match credit.date {
        Some(day) if normalized_date(day) != day => Cow::Owned(SurveyCredit {
            date: Some(normalized_date(day)),
            ..credit.clone()
        }),
        _ => Cow::Borrowed(credit),
    }
}

/// Group the shots of `line` by credit as written, in one pass. Shots that
/// share an allocation are grouped by pointer (the decoder shares one
/// allocation per distinct credit, so this is the usual case); a new
/// allocation is hashed once and compared by value only with the groups of
/// its hash bucket. A save therefore costs one look at each credit's people,
/// not one per shot.
pub(super) fn line_credit(line: &Line) -> LineCredits<'_> {
    let mut by_pointer: HashMap<*const SurveyCredit, usize> = HashMap::new();
    let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
    // (credit as written, count, first shot index)
    let mut groups: Vec<(Option<Cow<'_, SurveyCredit>>, usize, usize)> = Vec::new();
    let mut none_group = None;
    let mut shot_group = Vec::with_capacity(line.shots.len());
    for (index, shot) in line.shots.iter().enumerate() {
        let group = match &shot.credit {
            None => *none_group.get_or_insert_with(|| {
                groups.push((None, 0, index));
                groups.len() - 1
            }),
            Some(shared) => {
                let pointer = Arc::as_ptr(shared);
                if let Some(&group) = by_pointer.get(&pointer) {
                    group
                } else {
                    let written = written_credit(shared);
                    let mut hasher = DefaultHasher::new();
                    written.hash(&mut hasher);
                    let bucket = buckets.entry(hasher.finish()).or_default();
                    let found = bucket
                        .iter()
                        .copied()
                        .find(|&g| groups[g].0.as_deref() == Some(&*written));
                    let group = if let Some(g) = found {
                        g
                    } else {
                        groups.push((Some(written), 0, index));
                        bucket.push(groups.len() - 1);
                        groups.len() - 1
                    };
                    by_pointer.insert(pointer, group);
                    group
                }
            }
        };
        groups[group].1 += 1;
        shot_group.push(group);
    }
    let header = groups
        .iter()
        .enumerate()
        .max_by(|(_, (_, ca, ia)), (_, (_, cb, ib))| ca.cmp(cb).then(ib.cmp(ia)))
        .map(|(g, _)| g);
    let differs = shot_group.iter().map(|&g| Some(g) != header).collect();
    LineCredits {
        shared: header.and_then(|g| groups.swap_remove(g).0),
        differs,
    }
}

fn write_credit(out: &mut Vec<u8>, credit: &SurveyCredit) {
    let mut o = Obj::open(out);
    for (key, people) in [
        ("surveyors", &credit.surveyors),
        ("explorers", &credit.explorers),
    ] {
        if people.is_empty() {
            continue;
        }
        let b = o.key(key);
        b.push(b'[');
        for (i, person) in people.iter().enumerate() {
            if i > 0 {
                b.push(b',');
            }
            write_person(b, person);
        }
        b.push(b']');
    }
    if let Some(day) = credit.date {
        date(o.key("date"), day);
    }
    o.close();
}

fn write_person(out: &mut Vec<u8>, person: &Person) {
    let mut o = Obj::open(out);
    string(o.key("name"), &person.name);
    if let Some(email) = &person.email {
        string(o.key("email"), email);
    }
    o.close();
}

fn azimuth(out: &mut Vec<u8>, reference: &AzimuthReference, what: &str) -> Result<(), EncodeError> {
    let mut o = Obj::open(out);
    string(o.key("north"), tokens::north(reference.north));
    float(o.key("correction"), reference.correction, || {
        format!("correction of {what}")
    })?;
    let (source, epoch) = tokens::source(reference.source);
    string(o.key("source"), source);
    if let Some(epoch) = epoch {
        signed(o.key("epoch"), i64::from(epoch));
    }
    o.close();
    Ok(())
}

fn strings<'a>(out: &mut Vec<u8>, items: impl IntoIterator<Item = &'a str>) {
    out.push(b'[');
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        string(out, item);
    }
    out.push(b']');
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

/// The undo and redo entries `mode` writes (before a push's byte cut).
fn history_for<'a>(
    mode: Mode,
    extras: &'a Extras,
    cut: &mut HistoryCut,
) -> Result<(&'a [RawJson], &'a [RawJson]), EncodeError> {
    let History { undo, redo } = &extras.history;
    let sync = &extras.local.speleodb;
    match mode {
        Mode::Normal => Ok((undo, redo)),
        Mode::ForPush(options) => {
            cut.redo_dropped = redo.len();
            // The stored link state backs the options up: the watermark the
            // file holds, and a link whose undo mode is Disabled.
            let watermark = options.watermark.or(sync.inherited_through);
            let allowed = extras
                .speleodb
                .as_ref()
                .and_then(|l| l.undo_mode)
                .is_none_or(UndoMode::pushes_history);
            if !options.keep_undo
                || !allowed
                || (watermark.is_none() && sync.unwound_from.is_some())
            {
                // Without a watermark, a recorded Unwind step leaves the
                // inherited extent unknown: ship no history rather than
                // duplicate a loaded revision's operations (#303).
                cut.undo_dropped = !undo.is_empty();
                return Ok((&[], &[]));
            }
            let inherited = match watermark {
                Some(watermark) => inherited_prefix(undo, watermark)
                    .map_err(|reason| EncodeError::History { reason })?,
                None => 0,
            };
            cut.inherited_cut = inherited;
            Ok((&undo[inherited..], &[]))
        }
        Mode::DetachedCopy => {
            let inherited = match (sync.inherited_through, &sync.unwound_from) {
                (Some(watermark), _) => inherited_prefix(undo, watermark).ok(),
                (None, Some(_)) => None,
                (None, None) => Some(0),
            };
            if let Some(n) = inherited {
                cut.inherited_cut = n;
                Ok((&undo[n..], redo))
            } else {
                // An ambiguous extent: the copy keeps no undo (fail closed).
                cut.undo_dropped = !undo.is_empty();
                Ok((&[], redo))
            }
        }
    }
}

/// How many of the oldest undo entries are inherited: at or before
/// `watermark`, or untimed before the first timed one. Refuses an untimed
/// entry after a timed one and times out of order (today's SQLite rule).
pub(super) fn inherited_prefix(
    undo: &[RawJson],
    watermark: DateTime<Utc>,
) -> Result<usize, String> {
    #[derive(serde::Deserialize)]
    struct Stamp {
        #[serde(default)]
        t: Option<String>,
    }
    let mut previous: Option<DateTime<Utc>> = None;
    let mut inherited = 0;
    for (index, entry) in undo.iter().enumerate() {
        let stamp = serde_json::from_str::<Stamp>(entry.as_str())
            .ok()
            .and_then(|s| s.t);
        let is_inherited = match stamp {
            None if previous.is_some() => {
                return Err(format!(
                    "undo entry {index} has no time after a timed entry"
                ))
            }
            None => true,
            Some(text) => {
                let Some(at) = super::write::parse_time(&text) else {
                    return Err(format!("undo entry {index} has a malformed time {text:?}"));
                };
                if previous.is_some_and(|p| at < p) {
                    return Err(format!(
                        "undo entry {index} is older than the one before it"
                    ));
                }
                previous = Some(at);
                at <= watermark
            }
        };
        if is_inherited {
            if inherited != index {
                return Err("inherited undo entries are not the oldest ones".to_string());
            }
            inherited += 1;
        }
    }
    Ok(inherited)
}

/// The newest entries whose `history` member fits `room` bytes.
fn fit_newest(undo: &[RawJson], room: usize) -> &[RawJson] {
    const FIXED: usize = "\"history\":{\"undo\":[\n".len() + "\n]},\n".len();
    let mut used = FIXED;
    let mut start = undo.len();
    for (i, entry) in undo.iter().enumerate().rev() {
        let cost = entry.as_str().len() + if start == undo.len() { 0 } else { 2 };
        if used + cost > room {
            break;
        }
        used += cost;
        start = i;
    }
    &undo[start..]
}

pub(super) fn write_history(out: &mut Vec<u8>, undo: &[RawJson], redo: &[RawJson]) {
    if undo.is_empty() && redo.is_empty() {
        return;
    }
    string(out, "history");
    out.push(b':');
    let mut o = Obj::open(out);
    for (key, entries) in [("undo", undo), ("redo", redo)] {
        if entries.is_empty() {
            continue;
        }
        let b = o.key(key);
        b.extend_from_slice(b"[\n");
        for (i, entry) in entries.iter().enumerate() {
            if i > 0 {
                b.extend_from_slice(b",\n");
            }
            b.extend_from_slice(entry.as_str().as_bytes());
        }
        b.extend_from_slice(b"\n]");
    }
    o.close();
    out.extend_from_slice(b",\n");
}
