//! The SQLite (pre-version-2) path: the reader's own hostile-input checks on
//! the copy, then the frozen 0.6.0 open (`stalagmight_legacy`), then the app's
//! conversion to the station-graph model (`persistence::migrate`).

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use stalagmight_legacy::persistence::refused_schema_object;
use stalagmight_legacy::{LegacyError, LegacyProject, LegacySurvey, OpenOptions};
use stalagmight_models::config::ClosureAdjustmentConfig;
use stalagmight_persistence::persistence::migrate::{migrate, MigrateError, Migrated};

use crate::survey::{FileFormat, ReadMeta};
use crate::{
    after_load, check_counts, remaining, validate_datums, CountLimits, InvalidKind, Limit,
    LoadedCounts, ReadError, ReadOptions, Survey, SUPPORTED_SCHEMA_VERSION,
};

/// Read an SQLite `.stalx` copy.
pub(crate) fn read(
    copy: &Path,
    options: &ReadOptions,
    expires: Instant,
) -> Result<Survey, ReadError> {
    let max_value_bytes = i32::try_from(options.max_file_bytes).unwrap_or(i32::MAX);
    let schema_version = inspect(
        copy,
        max_value_bytes,
        options.count_limits,
        remaining(expires)?,
    )?;
    if schema_version > SUPPORTED_SCHEMA_VERSION {
        return Err(ReadError::NewerSchema {
            found: schema_version,
            supported: SUPPORTED_SCHEMA_VERSION,
        });
    }

    // The 0.6.0 open: integrity check, migrations, load and the load's heals
    // (shot uuids derived in this crate's namespace, closure ids, the offset
    // re-drive, the pending kind classification). Its SQLite statements get
    // only the time that is left.
    let open = OpenOptions {
        max_file_bytes: options.max_file_bytes,
        with_adjusted: false,
        temp_dir: options.temp_dir.clone(),
        sqlite_deadline: remaining(expires)?,
        ..OpenOptions::default()
    };
    let legacy =
        stalagmight_legacy::open_path(copy, &open).map_err(|e| legacy_error(e, expires))?;
    // A statement interrupted by the deadline can surface as a plain refusal,
    // so check the clock before trusting what was loaded.
    remaining(expires)?;
    after_load();
    let lp = &legacy.project;
    check_counts(
        &LoadedCounts {
            sections: lp.lines.len(),
            shots: lp.lines.iter().map(|l| l.shots.len()).sum(),
            station_merges: lp.station_merges.len(),
            loop_closures: lp.loop_closures.len(),
        },
        options.count_limits,
    )?;
    validate_datums(lp.zero_datums.iter().map(|d| {
        (
            d.gps.latitude,
            d.gps.longitude,
            format!("section {}", d.station_ref.line_id),
        )
    }))?;
    let unresolved = unresolved_references(lp);

    remaining(expires)?;
    // The conversion the app runs on open. The legacy Closures Computed View
    // was not computed (`with_adjusted: false`), so the conversion verifies
    // raw positions only; the config is then unused.
    let migrated = migrate(&legacy, &ClosureAdjustmentConfig::default()).map_err(migrate_error)?;
    remaining(expires)?;
    let derived = derived_uuids(&legacy, &migrated);
    Ok(Survey::from_model(
        &migrated.project,
        &ReadMeta {
            format: FileFormat::Sqlite,
            format_version: 1,
            schema_version,
            derived_uuids: &derived,
            unresolved_references: unresolved,
            newer_content: None,
        },
        options.with_stations,
    ))
}

/// Cross-references of the loaded legacy project whose station no longer
/// exists (counted once per record, as the reader always did). The
/// conversion drops them.
fn unresolved_references(project: &LegacyProject) -> usize {
    let dangling = |key| project.resolve_key(key).is_none();
    project
        .station_merges
        .iter()
        .filter(|m| dangling(&m.station_a) || dangling(&m.station_b))
        .count()
        + project
            .zero_datums
            .iter()
            .filter(|d| dangling(&d.station_ref))
            .count()
        + project
            .loop_closures
            .iter()
            .filter(|c| dangling(&c.station_a) || dangling(&c.station_b))
            .count()
        + project
            .station_comments
            .keys()
            .filter(|k| dangling(k))
            .count()
        + project.station_names.keys().filter(|k| dangling(k)).count()
}

/// The shots whose uuid the file did not give: derived or renamed by the
/// legacy load, or (re-)minted by the conversion, as `(section id, uuid)`.
fn derived_uuids(legacy: &LegacySurvey, migrated: &Migrated) -> HashSet<(u32, String)> {
    let mut out: HashSet<(u32, String)> = legacy
        .load
        .derived_shot_uuids
        .iter()
        .cloned()
        .chain(
            legacy
                .load
                .renamed_shot_uuids
                .iter()
                .map(|(line, _, new)| (*line, new.clone())),
        )
        .collect();
    for line in migrated.project.lines() {
        let Some(old) = legacy.project.lines.iter().find(|l| l.id == line.id) else {
            continue;
        };
        for (index, shot) in line.shots.iter().enumerate() {
            let was = old.shots.get(index).and_then(|s| s.uuid.as_deref());
            if was != Some(shot.uuid.as_str()) {
                out.insert((line.id, shot.uuid.clone()));
            }
        }
    }
    // Keep only ids the converted survey holds: a load-derived id the
    // conversion replaced is not one a consumer can meet.
    let held: HashSet<(u32, &str)> = migrated
        .project
        .lines()
        .iter()
        .flat_map(|l| l.shots.iter().map(move |s| (l.id, s.uuid.as_str())))
        .collect();
    out.retain(|(line, uuid)| held.contains(&(*line, uuid.as_str())));
    out
}

/// Classify a refusal of the frozen open. Its typed variants map one to one;
/// a [`LegacyError::Corrupt`] carries only text, so the deadline is read off
/// the clock (the open's own SQLite deadline is what is left of the read's,
/// so an interrupt there means the read's has passed too) and the rest off
/// the messages the frozen persistence layer attaches (pinned by tests).
pub(crate) fn legacy_error(e: LegacyError, expires: Instant) -> ReadError {
    let detail = match e {
        LegacyError::NotSqlite => {
            return ReadError::invalid(InvalidKind::NotSqlite, "not an SQLite database");
        }
        LegacyError::NewerSchema { found, supported } => {
            return ReadError::NewerSchema { found, supported };
        }
        LegacyError::TooLarge { max } => return ReadError::TooLarge { max },
        LegacyError::Io(e) => return ReadError::Io(e),
        LegacyError::Corrupt(detail) => detail,
    };
    let kind = if remaining(expires).is_err() || detail.contains("interrupted") {
        InvalidKind::Deadline
    } else if detail.contains("refused schema object") {
        InvalidKind::RefusedSchemaObject
    } else if detail.contains("does not contain a valid Stalagmight project") {
        InvalidKind::NoProjectData
    } else if detail.contains("Database file may be corrupted")
        || detail.contains("malformed")
        || detail.contains("negative schema version")
    {
        InvalidKind::Corrupt
    } else {
        InvalidKind::Other
    };
    ReadError::invalid(kind, detail)
}

/// Classify a refusal of the conversion.
fn migrate_error(e: MigrateError) -> ReadError {
    match e {
        // `migrate` takes an opened survey, so this cannot happen; mapped for
        // completeness.
        MigrateError::Legacy(e) => legacy_error(e, Instant::now() + Duration::from_secs(1)),
        e @ (MigrateError::LineIdOutOfRange(_)
        | MigrateError::DuplicateLineId(_)
        | MigrateError::StationIdsExhausted) => {
            ReadError::invalid(InvalidKind::InvalidValue, e.to_string())
        }
        // The conversion produced a survey that breaks the model's
        // invariants: a bug in the conversion, not in the file.
        e @ MigrateError::Invalid(_) => ReadError::Internal(e.to_string()),
    }
}

/// Refuse a file whose tables hold more rows than `limits` allow, before the
/// load builds them in memory. A table the file lacks counts as empty.
fn check_row_counts(conn: &rusqlite::Connection, limits: CountLimits) -> Result<(), ReadError> {
    for (table, what, max) in [
        ("sections", Limit::Sections, limits.sections),
        ("shots", Limit::Shots, limits.shots),
        (
            "station_merges",
            Limit::StationMerges,
            limits.station_merges,
        ),
        ("loop_closures", Limit::LoopClosures, limits.loop_closures),
        ("station_names", Limit::StationNames, limits.station_names),
        ("shot_credits", Limit::ShotCredits, limits.shot_credits),
        ("credit_people", Limit::CreditPeople, limits.credit_people),
        (
            "zero_datum_extras",
            Limit::ZeroDatumExtras,
            limits.zero_datum_extras,
        ),
        ("station_ids", Limit::StationIds, limits.station_ids),
        ("cave_sections", Limit::CaveSections, limits.cave_sections),
        (
            "cave_section_lines",
            Limit::CaveSectionLines,
            limits.cave_section_lines,
        ),
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .map_err(sql_invalid)?;
        if exists == 0 {
            continue;
        }
        // `table` is one of the fixed names above, never file content.
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                row.get(0)
            })
            .map_err(sql_invalid)?;
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        if count > max {
            return Err(ReadError::TooComplex { what, count, max });
        }
    }
    Ok(())
}

/// Refuse schema objects that would run file-chosen SQL, refuse tables over
/// the row caps, and return the file's schema version, all read before
/// anything migrates the copy.
///
/// The version follows the app's own rule: `PRAGMA user_version`, or when that
/// is 0 (files from before the pragma), the `project.schema_version` column.
fn inspect(
    path: &Path,
    max_value_bytes: i32,
    limits: CountLimits,
    deadline: Duration,
) -> Result<i32, ReadError> {
    use rusqlite::config::DbConfig;
    use rusqlite::limits::Limit;

    let sql_err = sql_invalid;
    let mut conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(sql_err)?;
    // Same hardening as the load connection: this one parses the file's schema too.
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
        .map_err(sql_err)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)
        .map_err(sql_err)?;
    conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, max_value_bytes)
        .map_err(sql_err)?;
    let expires = Instant::now() + deadline;
    conn.progress_handler(1_000, Some(move || Instant::now() > expires))
        .map_err(sql_err)?;
    conn.execute_batch("PRAGMA mmap_size = 0; PRAGMA cell_size_check = ON;")
        .map_err(sql_err)?;

    // The frozen app's hostile-schema check (`refused_schema_object`):
    // pragma-shadowing names, unexpected types, virtual tables, and any schema
    // expression the file chose (generated columns, expression or partial
    // indexes, expression defaults, CHECKs other than the app's).
    if let Some(refused) = refused_schema_object(&conn).map_err(sql_err)? {
        return Err(ReadError::invalid(
            InvalidKind::RefusedSchemaObject,
            refused.to_string(),
        ));
    }
    check_row_counts(&conn, limits)?;

    let stamped: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(sql_err)?;
    if stamped < 0 {
        return Err(ReadError::invalid(
            InvalidKind::Corrupt,
            format!("negative schema version {stamped}"),
        ));
    }
    if stamped > 0 {
        return Ok(stamped);
    }
    let has_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('project') WHERE name = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .map_err(sql_err)?;
    if has_column == 0 {
        return Ok(0);
    }
    let claimed: Option<i32> = conn
        .query_row("SELECT schema_version FROM project LIMIT 1", [], |row| {
            row.get(0)
        })
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })
        .map_err(sql_err)?;
    Ok(claimed.unwrap_or(0).max(0))
}

fn sqlite_kind(e: &rusqlite::Error) -> Option<InvalidKind> {
    use rusqlite::ErrorCode;
    match e.sqlite_error_code()? {
        ErrorCode::NotADatabase => Some(InvalidKind::NotSqlite),
        ErrorCode::DatabaseCorrupt => Some(InvalidKind::Corrupt),
        ErrorCode::OperationInterrupted => Some(InvalidKind::Deadline),
        _ => None,
    }
}

fn sql_invalid(e: rusqlite::Error) -> ReadError {
    let kind = sqlite_kind(&e).unwrap_or(InvalidKind::Other);
    ReadError::invalid(kind, e.to_string())
}
