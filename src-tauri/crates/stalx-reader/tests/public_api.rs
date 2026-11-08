//! Compile-time pin of the crate's public API, for the servers that depend on it.
//!
//! Every public function, builder, field, enum variant and const is named here
//! with its exact type, and so are the trait impls a consumer relies on. Removing
//! one, or changing its type, fails this file's build. Additions stay allowed:
//! the types are `#[non_exhaustive]`, so every pattern ends in `..`, every match
//! has a wildcard arm, and nothing is built with a struct literal.
//!
//! A failure here is a breaking change for library users. Restore the item, or
//! make the break deliberately (a major version, a changelog entry) and then
//! update the pin.

use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Duration;

use stalx_reader::{
    CorrectionSource, CountLimits, Direction, InvalidKind, Limit, LoopClosure, North, ReadError,
    ReadOptions, Section, Shot, ShotKind, StationComment, StationId, StationMerge, StationPosition,
    Survey, ZeroDatum, DEFAULT_DEADLINE, DEFAULT_MAX_FILE_BYTES, SUPPORTED_SCHEMA_VERSION,
};

/// Implemented for `T` alone, so `U: Exactly<T>` holds only when `U` is `T`.
#[diagnostic::on_unimplemented(
    message = "public API changed: this is a `{Self}`, the pin expects a `{T}`",
    label = "type changed"
)]
trait Exactly<T> {}
impl<T> Exactly<T> for T {}

/// Compiles only when the argument is exactly a `&T`. `U` is inferred from the
/// argument, so no deref or unsizing coercion can let another type through.
fn is<T, U: Exactly<T>>(_: &U) {}

#[test]
fn entry_points_keep_their_signatures() {
    let _: fn(&Path) -> Result<Survey, ReadError> = stalx_reader::read_file;
    let _: fn(&Path, &ReadOptions) -> Result<Survey, ReadError> = stalx_reader::read_file_with;
    let _: fn(&[u8], &ReadOptions) -> Result<Survey, ReadError> = stalx_reader::read_bytes;
}

#[test]
fn builders_keep_their_signatures_and_set_their_field() {
    let _: fn() -> ReadOptions = ReadOptions::default;
    let _: fn(ReadOptions, u64) -> ReadOptions = ReadOptions::max_file_bytes;
    let _: fn(ReadOptions, Duration) -> ReadOptions = ReadOptions::deadline;
    let _: fn(ReadOptions, bool) -> ReadOptions = ReadOptions::with_stations;
    let _: fn(ReadOptions, CountLimits) -> ReadOptions = ReadOptions::count_limits;
    // `impl Into<PathBuf>`: owned paths here, borrowed ones in the calls below.
    let _: fn(ReadOptions, PathBuf) -> ReadOptions = ReadOptions::temp_dir;
    let _: fn(ReadOptions, String) -> ReadOptions = ReadOptions::temp_dir;
    let _: fn() -> CountLimits = CountLimits::default;
    let _: fn(CountLimits, usize) -> CountLimits = CountLimits::sections;
    let _: fn(CountLimits, usize) -> CountLimits = CountLimits::shots;
    let _: fn(CountLimits, usize) -> CountLimits = CountLimits::station_merges;
    let _: fn(CountLimits, usize) -> CountLimits = CountLimits::loop_closures;

    let limits = CountLimits::default()
        .sections(1)
        .shots(2)
        .station_merges(3)
        .loop_closures(4);
    assert_eq!(
        (
            limits.sections,
            limits.shots,
            limits.station_merges,
            limits.loop_closures
        ),
        (1, 2, 3, 4)
    );
    let options = ReadOptions::default()
        .max_file_bytes(5)
        .deadline(Duration::from_millis(6))
        .with_stations(true)
        .count_limits(limits)
        .temp_dir(Path::new("/stalx-reader-pin/a"));
    assert_eq!(options.max_file_bytes, 5);
    assert_eq!(options.deadline, Duration::from_millis(6));
    assert!(options.with_stations);
    assert_eq!(options.count_limits, limits);
    assert_eq!(options.temp_dir, Some(PathBuf::from("/stalx-reader-pin/a")));
    let options = options.temp_dir("/stalx-reader-pin/b");
    assert_eq!(options.temp_dir, Some(PathBuf::from("/stalx-reader-pin/b")));
}

#[test]
fn consts_and_defaults_keep_their_values() {
    is::<i32, _>(&SUPPORTED_SCHEMA_VERSION);
    is::<u64, _>(&DEFAULT_MAX_FILE_BYTES);
    is::<Duration, _>(&DEFAULT_DEADLINE);
    // A schema bump changes this one on purpose: update it with the bump.
    assert_eq!(SUPPORTED_SCHEMA_VERSION, 18);
    assert_eq!(DEFAULT_MAX_FILE_BYTES, 64 * 1024 * 1024);
    assert_eq!(DEFAULT_DEADLINE, Duration::from_secs(30));

    let options = ReadOptions::default();
    read_options_fields(&options);
    assert_eq!(options.max_file_bytes, DEFAULT_MAX_FILE_BYTES);
    assert_eq!(options.deadline, DEFAULT_DEADLINE);
    assert!(!options.with_stations);
    assert_eq!(options.count_limits, CountLimits::default());
    assert_eq!(options.temp_dir, None);

    let limits = CountLimits::default();
    count_limits_fields(&limits);
    assert_eq!(
        (
            limits.sections,
            limits.shots,
            limits.station_merges,
            limits.loop_closures
        ),
        (100_000, 2_000_000, 200_000, 50_000)
    );
}

#[test]
fn every_public_field_and_variant_keeps_its_type() {
    // Type-checked at build time. Only a real file yields these values, so the
    // pins are not run here.
    let _: fn(&Survey) = survey_fields;
    let _: fn(&Section) = section_fields;
    let _: fn(&Shot) = shot_fields;
    let _: fn(&StationId) = station_id_fields;
    let _: fn(&StationMerge) = station_merge_fields;
    let _: fn(&ZeroDatum) = zero_datum_fields;
    let _: fn(&LoopClosure) = loop_closure_fields;
    let _: fn(&StationComment) = station_comment_fields;
    let _: fn(&StationPosition) = station_position_fields;
    let _: fn(&ReadError) -> &'static str = read_error_variants;
    let _: fn(&InvalidKind) -> &'static str = invalid_kind_variants;
    let _: fn(&Limit) -> &'static str = limit_variants;
    let _: fn(&CorrectionSource) -> &'static str = correction_source_variants;
    let _: fn(&ShotKind) -> &'static str = shot_kind_variants;
    let _: fn(&Direction) -> &'static str = direction_variants;
    let _: fn(&North) -> &'static str = north_variants;
}

fn read_options_fields(o: &ReadOptions) {
    is::<u64, _>(&o.max_file_bytes);
    is::<Duration, _>(&o.deadline);
    is::<bool, _>(&o.with_stations);
    is::<CountLimits, _>(&o.count_limits);
    is::<Option<PathBuf>, _>(&o.temp_dir);
}

fn count_limits_fields(l: &CountLimits) {
    is::<usize, _>(&l.sections);
    is::<usize, _>(&l.shots);
    is::<usize, _>(&l.station_merges);
    is::<usize, _>(&l.loop_closures);
}

fn survey_fields(s: &Survey) {
    is::<i32, _>(&s.schema_version);
    is::<String, _>(&s.name);
    is::<Option<String>, _>(&s.location);
    is::<String, _>(&s.created_at);
    is::<String, _>(&s.modified_at);
    is::<Vec<Section>, _>(&s.sections);
    is::<Vec<StationMerge>, _>(&s.station_merges);
    is::<Vec<ZeroDatum>, _>(&s.zero_datums);
    is::<Vec<LoopClosure>, _>(&s.loop_closures);
    is::<Vec<StationComment>, _>(&s.station_comments);
    is::<Option<StationId>, _>(&s.frame_datum);
    is::<Option<Vec<StationPosition>>, _>(&s.stations);
    is::<usize, _>(&s.unresolved_references);
}

fn section_fields(s: &Section) {
    is::<u32, _>(&s.id);
    is::<Option<String>, _>(&s.name);
    is::<Option<String>, _>(&s.surveyor);
    is::<Option<String>, _>(&s.date);
    is::<Direction, _>(&s.direction);
    is::<f64, _>(&s.quality);
    is::<String, _>(&s.color);
    is::<North, _>(&s.north);
    is::<f64, _>(&s.azimuth_correction_deg);
    is::<CorrectionSource, _>(&s.azimuth_correction_source);
    is::<Vec<Shot>, _>(&s.shots);
}

fn shot_fields(s: &Shot) {
    is::<String, _>(&s.uuid);
    is::<bool, _>(&s.uuid_derived);
    is::<ShotKind, _>(&s.kind);
    is::<f64, _>(&s.length);
    is::<f64, _>(&s.azimuth_in);
    is::<f64, _>(&s.azimuth_out);
    is::<f64, _>(&s.depth_in);
    is::<f64, _>(&s.depth_out);
    is::<Option<f64>, _>(&s.pitch_in);
    is::<Option<f64>, _>(&s.pitch_out);
    is::<Option<f64>, _>(&s.left);
    is::<Option<f64>, _>(&s.right);
    is::<Option<f64>, _>(&s.up);
    is::<Option<f64>, _>(&s.down);
    is::<Option<f64>, _>(&s.temperature);
    is::<Option<u32>, _>(&s.marker_index);
    is::<Option<String>, _>(&s.comment);
}

fn station_id_fields(s: &StationId) {
    is::<u32, _>(&s.section_id);
    is::<u32, _>(&s.index);
}

fn station_merge_fields(m: &StationMerge) {
    is::<StationId, _>(&m.a);
    is::<StationId, _>(&m.b);
}

fn zero_datum_fields(z: &ZeroDatum) {
    is::<StationId, _>(&z.station);
    is::<f64, _>(&z.latitude);
    is::<f64, _>(&z.longitude);
    is::<f64, _>(&z.depth);
    is::<Option<f64>, _>(&z.residual_m);
}

fn loop_closure_fields(c: &LoopClosure) {
    is::<u64, _>(&c.id);
    is::<StationId, _>(&c.a);
    is::<StationId, _>(&c.b);
}

fn station_comment_fields(c: &StationComment) {
    is::<StationId, _>(&c.station);
    is::<String, _>(&c.text);
}

fn station_position_fields(p: &StationPosition) {
    is::<StationId, _>(&p.station);
    is::<f64, _>(&p.x);
    is::<f64, _>(&p.y);
    is::<f64, _>(&p.z);
    is::<Option<StationId>, _>(&p.anchor);
    is::<Option<f64>, _>(&p.latitude);
    is::<Option<f64>, _>(&p.longitude);
}

fn read_error_variants(e: &ReadError) -> &'static str {
    match e {
        ReadError::TooLarge { max, .. } => {
            is::<u64, _>(max);
            "TooLarge"
        }
        ReadError::TooComplex {
            what, count, max, ..
        } => {
            is::<Limit, _>(what);
            is::<usize, _>(count);
            is::<usize, _>(max);
            "TooComplex"
        }
        ReadError::NewerSchema {
            found, supported, ..
        } => {
            is::<i32, _>(found);
            is::<i32, _>(supported);
            "NewerSchema"
        }
        ReadError::Io(source, ..) => {
            is::<std::io::Error, _>(source);
            "Io"
        }
        ReadError::Invalid { kind, detail, .. } => {
            is::<InvalidKind, _>(kind);
            is::<String, _>(detail);
            "Invalid"
        }
        ReadError::Internal(message, ..) => {
            is::<String, _>(message);
            "Internal"
        }
        _ => "added later",
    }
}

fn invalid_kind_variants(k: &InvalidKind) -> &'static str {
    match k {
        InvalidKind::NotSqlite => "NotSqlite",
        InvalidKind::Corrupt => "Corrupt",
        InvalidKind::RefusedSchemaObject => "RefusedSchemaObject",
        InvalidKind::Deadline => "Deadline",
        InvalidKind::NoProjectData => "NoProjectData",
        InvalidKind::InvalidValue => "InvalidValue",
        InvalidKind::Other => "Other",
        _ => "added later",
    }
}

fn limit_variants(l: &Limit) -> &'static str {
    match l {
        Limit::Sections => "Sections",
        Limit::Shots => "Shots",
        Limit::StationMerges => "StationMerges",
        Limit::LoopClosures => "LoopClosures",
        _ => "added later",
    }
}

fn correction_source_variants(c: &CorrectionSource) -> &'static str {
    match c {
        CorrectionSource::NotRecorded => "NotRecorded",
        CorrectionSource::Unknown => "Unknown",
        CorrectionSource::FileDeclared => "FileDeclared",
        CorrectionSource::Model { epoch_year, .. } => {
            is::<i32, _>(epoch_year);
            "Model"
        }
        CorrectionSource::Manual => "Manual",
        _ => "added later",
    }
}

fn shot_kind_variants(k: &ShotKind) -> &'static str {
    match k {
        ShotKind::Normal => "Normal",
        ShotKind::Jump => "Jump",
        ShotKind::Reference => "Reference",
        _ => "added later",
    }
}

fn direction_variants(d: &Direction) -> &'static str {
    match d {
        Direction::In => "In",
        Direction::Out => "Out",
        _ => "added later",
    }
}

fn north_variants(n: &North) -> &'static str {
    match n {
        North::Magnetic => "Magnetic",
        North::True => "True",
        North::Grid => "Grid",
        _ => "added later",
    }
}

fn error<T: std::error::Error + Send + Sync + 'static>() {}
fn data<
    T: Debug + Clone + PartialEq + Send + Sync + Unpin + UnwindSafe + RefUnwindSafe + 'static,
>() {
}
fn copy<T: Copy>() {}
fn eq<T: Eq>() {}
fn defaulted<T: Default>() {}
fn displayed<T: Display>() {}

#[test]
fn trait_impls_consumers_rely_on() {
    // `?` into `Box<dyn Error + Send + Sync>` or `anyhow`, across threads.
    // (Not UnwindSafe: `std::io::Error` inside `ReadError::Io` is not.)
    error::<ReadError>();
    fn from_io<T: From<std::io::Error> + Debug + Display + Unpin>() {}
    from_io::<ReadError>();

    fn options<T: Debug + Clone + Send + Sync + Unpin + UnwindSafe + RefUnwindSafe + 'static>() {}
    options::<ReadOptions>();
    defaulted::<ReadOptions>();
    data::<CountLimits>();
    defaulted::<CountLimits>();
    data::<Limit>();
    displayed::<Limit>();
    data::<InvalidKind>();

    data::<Survey>();
    data::<Section>();
    data::<Shot>();
    data::<StationId>();
    data::<StationMerge>();
    data::<ZeroDatum>();
    data::<LoopClosure>();
    data::<StationComment>();
    data::<StationPosition>();
    data::<ShotKind>();
    data::<Direction>();
    data::<North>();
    data::<CorrectionSource>();

    copy::<CountLimits>();
    copy::<Limit>();
    copy::<InvalidKind>();
    copy::<StationId>();
    copy::<StationMerge>();
    copy::<ZeroDatum>();
    copy::<LoopClosure>();
    copy::<StationPosition>();
    copy::<ShotKind>();
    copy::<Direction>();
    copy::<North>();
    copy::<CorrectionSource>();

    eq::<CountLimits>();
    eq::<Limit>();
    eq::<InvalidKind>();
    eq::<StationId>();
    eq::<StationMerge>();
    eq::<LoopClosure>();
    eq::<StationComment>();
    eq::<ShotKind>();
    eq::<Direction>();
    eq::<North>();
    eq::<CorrectionSource>();

    // Keyed in maps and sorted by consumers.
    fn key<T: Ord + Hash>() {}
    key::<StationId>();
    // The one public impl that names an app-model type.
    fn from_station_ref<T: From<stalagmight_models::station::StationRef>>() {}
    from_station_ref::<StationId>();
}
