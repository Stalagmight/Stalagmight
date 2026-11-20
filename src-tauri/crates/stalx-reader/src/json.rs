//! The JSON (version 2) path: the app's own decoder, with the reader's caps.

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use stalagmight_persistence::persistence::stalx_v2::{
    self, decode, DecodeError, DecodeOptions, Decoded,
};

use crate::survey::{FileFormat, ReadMeta};
use crate::{
    after_load, check_counts, remaining, validate_datums, CountLimits, InvalidKind, Limit,
    LoadedCounts, ReadError, ReadOptions, Survey,
};

/// Read a JSON `.stalx` copy (already sniffed as one).
pub(crate) fn read(
    copy: &Path,
    options: &ReadOptions,
    expires: Instant,
) -> Result<Survey, ReadError> {
    remaining(expires)?;
    // The copy is already capped at `max_file_bytes`.
    let bytes = std::fs::read(copy)?;
    let decoded = decode(
        &bytes,
        &DecodeOptions {
            max_file_bytes: options.max_file_bytes,
            limits: decoder_limits(options.count_limits),
        },
    )
    .map_err(decode_error)?;
    drop(bytes);
    remaining(expires)?;
    after_load();
    survey_of(&decoded, options, expires)
}

fn survey_of(
    decoded: &Decoded,
    options: &ReadOptions,
    expires: Instant,
) -> Result<Survey, ReadError> {
    let project = &decoded.project;
    check_counts(
        &LoadedCounts {
            sections: project.lines().len(),
            shots: project.lines().iter().map(|l| l.shots.len()).sum(),
            station_merges: crate::survey::junction_pair_count(project),
            loop_closures: project.closures.len(),
        },
        options.count_limits,
    )?;
    validate_datums(project.datums.iter().map(|d| {
        (
            d.gps.latitude,
            d.gps.longitude,
            format!("station {}", d.station),
        )
    }))?;
    remaining(expires)?;
    let none = HashSet::default();
    Ok(Survey::from_model(
        project,
        &ReadMeta {
            format: FileFormat::Json,
            format_version: stalx_v2::VERSION,
            schema_version: 0,
            derived_uuids: &none,
            unresolved_references: 0,
            newer_content: decoded.read_only().map(ToString::to_string),
        },
        options.with_stations,
    ))
}

/// The decoder's caps from the reader's (`CountLimits`' field docs say which
/// is which). History entries and display settings over the decoder's own
/// caps are dropped by the decoder, never refused, and the reader reads
/// neither.
fn decoder_limits(limits: CountLimits) -> stalx_v2::CountLimits {
    stalx_v2::CountLimits {
        stations: limits.station_ids,
        aliases: limits.aliases,
        lines: limits.sections,
        shots: limits.shots,
        hand_moves: limits.hand_moves,
        datums: limits.zero_datum_extras,
        placements: limits.placements,
        closures: limits.loop_closures,
        sections: limits.cave_sections,
        ignored_warnings: limits.ignored_warnings,
        import_hashes: limits.import_hashes,
        credit_people: limits.credit_people,
        preserved: limits.preserved_content,
        ..stalx_v2::CountLimits::default()
    }
}

/// The reader's name for a decoder cap.
fn reader_limit(limit: stalx_v2::Limit) -> Limit {
    use stalx_v2::Limit as L;
    match limit {
        L::Stations => Limit::StationIds,
        L::Aliases => Limit::Aliases,
        L::Lines => Limit::Sections,
        L::Shots => Limit::Shots,
        L::HandMoves => Limit::HandMoves,
        L::Datums => Limit::ZeroDatumExtras,
        L::Placements => Limit::Placements,
        L::Closures => Limit::LoopClosures,
        L::Sections => Limit::CaveSections,
        L::IgnoredWarnings => Limit::IgnoredWarnings,
        L::ImportHashes => Limit::ImportHashes,
        L::CreditPeople => Limit::CreditPeople,
        L::Preserved => Limit::PreservedContent,
    }
}

/// Classify a refusal of the decoder.
pub(crate) fn decode_error(e: DecodeError) -> ReadError {
    let detail = e.to_string();
    match e {
        DecodeError::NotStalx | DecodeError::Legacy => {
            ReadError::invalid(InvalidKind::NotSqlite, detail)
        }
        DecodeError::TooLarge { max, .. } => ReadError::TooLarge { max },
        DecodeError::NewerVersion { found, supported } => {
            ReadError::NewerFormat { found, supported }
        }
        DecodeError::TooComplex { limit, max } => ReadError::TooComplex {
            what: reader_limit(limit),
            count: max.saturating_add(1),
            max,
        },
        DecodeError::ByteOrderMark
        | DecodeError::UnsupportedVersion { .. }
        | DecodeError::Conventions { .. }
        | DecodeError::Truncated { .. }
        | DecodeError::Syntax { .. }
        | DecodeError::BadValue { .. }
        | DecodeError::Missing { .. }
        | DecodeError::DuplicateKey { .. }
        | DecodeError::TooDeep { .. }
        | DecodeError::MissingEnd
        | DecodeError::DuplicateStation { .. }
        | DecodeError::Invalid { .. } => ReadError::invalid(InvalidKind::Corrupt, detail),
    }
}
