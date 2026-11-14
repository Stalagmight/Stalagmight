#![expect(clippy::expect_used)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use ts_rs::TS;

use stalagmight_lib::commands::app_update::{
    AppUpdateCheck, AppUpdateInstallError, AppUpdateProgress, AppUpdateStatus,
};
use stalagmight_lib::commands::cache::{CacheStats, ProviderStats, TileFetchStatsResponse};
use stalagmight_lib::commands::export::DiveProfileRequest;
use stalagmight_lib::commands::history::{FieldChange, PreviewData, UndoHistoryItem};
use stalagmight_lib::commands::memory::{MemoryStats, ProcessMemory};
use stalagmight_lib::commands::mnemo::{MnemoError, MnemoPort, MnemoPullProgress, MnemoPullResult};
use stalagmight_lib::commands::project::ProjectState;
use stalagmight_lib::commands::section::{SegmentGeometry, SegmentSectionGeometry};
use stalagmight_lib::commands::survey_geometry::{
    GeometryShot, SurveyGeometry, SurveyGeometryError,
};
use stalagmight_lib::config::{
    AngleUnit, AppearanceConfig, CacheConfig, ClosureAdjustmentConfig, ColorTheme, Config,
    DebugConfig, DecimalPrecision, DisplayConfig, DpvRig, DpvUnit, ExportConfig, GasConfig,
    GasUnits, GeneralConfig, GlobalViewConfig, GpsOrigin, ImportConfig, IssueCombine, LengthUnit,
    LoggingConfig, MapAnimationConfig, MapConfig, MapControlsConfig, MapDebugConfig,
    MapPaletteConfig, MapSnapConfig, OverlapMode, PaletteChoiceConfig, PanelsConfig, PeopleConfig,
    PlacementSuggestionConfig, ProviderCacheConfig, ReservePreset, ReserveProfile,
    SectionColorsConfig, ShortcutFeedback, ShortcutsConfig, ShotConfig, SpeleoDbConfig,
    StationConfig, SyntacticHighlightLevel, SyntacticHighlightPreset, SyntacticHighlightingConfig,
    ThemeMode, UnitsConfig, UpdateTrack, ValidationConfig,
};
use stalagmight_lib::core::command::{
    CommandResult, CommentHolder, ImportReceipt, IntegrityCheckResult, IntegrityRepairResult,
    LockReleaseOutcome, ReversedSection, ValidationWarning, WarningSeverity,
};
use stalagmight_lib::core::loading::{
    CreateProjectFullResult, DisplaySettings, LoadProjectFullResult, LoadingPhase, LoadingProgress,
    SpeleoManagedFile,
};
use stalagmight_lib::core::state::{
    SpeleoCacheOwnership, SpeleoCachedProject, SpeleoPurgeReport, VirtualShotMode,
};
use stalagmight_lib::export::color_scale::{
    ExportColorScale, ExportLevels, ExportValueKind, ValueHistogram,
};
use stalagmight_lib::export::{ColorMode, ExportImageOptions, ImageFormat};
use stalagmight_lib::logging::{LogsSince, RingLogEntry};
use stalagmight_lib::models::placement_suggest::{
    CrossSectionSuggestion, ImportSectionForSuggestion, MatchDirection, OrphanPlacementSuggestion,
    PlacementSuggestion, SuggestionConfidence, TargetType,
};
use stalagmight_lib::models::{
    AlongLineResult, AzimuthReference, ClosurePair, ClosurePreview, ClosureRef, ClosureStrategy,
    ClosureUpdate, CombineCandidate, CombineJoinEnds, CombineJoinKind, CombinePick, CombinePlan,
    CombineSectionsOptions, ConnectivityResult, Coordinates, CorrectionSource, CreditEdit,
    CreditTargets, CreditedPerson, CylRole, Cylinder, DeleteShotImpact, DeleteShotRangeImpact,
    DistanceField, DivePlan, DivePlanMeta, DivePlanSnapshot, Diver, ExistingShotUpdate,
    FrozenShotRef, GasReachField, GpsBounds, GpsCoordinates, ImportPlacement, ImportSectionConfig,
    KeptMovesReason, LoopClosureWire, MarkedPoint, NorthReference, PathShot, Person, PlanAnchorSet,
    PlanMode, PlanSegment, Project, ProjectMetadata, ProjectStats, ProjectWarning, RangeDeleteKind,
    RouteLeg, RouteProfilePoint, RulerEndpoint, Section, SectionCreditSummary, SectionData,
    SectionEnd, SectionRelationships, SectionStats, SectionUpdates, Shot, ShotData, ShotKind,
    ShotUpdates, SpeleoLink, SplitAt, SplitPreview, SplitSectionOptions, SplitSide, Station,
    StationMergeWire, StationPair, StationRef, SurveyCredit, SurveyDirection, TankMaterial, Team,
    TornTie, TornTieKind, TornTieReason, UnrecognisedShotKind, ZeroDatumWire,
};
use stalagmight_lib::parsers::compass::{CompassFile, CompassSurveySummary, InvalidShotCounts};
use stalagmight_lib::parsers::dmp::{DmpDirection, DmpFile, DmpSection, DmpShot, DmpShotType};
use stalagmight_lib::parsers::gpx::{
    GpxFile, GpxRoute, GpxRoutePoint, GpxSectionPlacement, GpxTrack, GpxTrackPoint,
    GpxTrackSegment, GpxWaypoint,
};
use stalagmight_lib::parsers::tml::{
    TmlClosure, TmlFile, TmlGeoCoding, TmlParseAnomalies, TmlRootPlacement, TmlSection, TmlShot,
    TmlShotType, TmlUnit,
};
use stalagmight_lib::parsers::walls::{WallsFile, WallsSurveySummary};
use stalagmight_lib::persistence::{BackupInfo, RawTableData, RepairResult, SalvagedData};
use stalagmight_lib::speleodb::{
    DoneResult, EndpointRefusal, Permission, PushResult, RemoteState, SpeleoAccessVerification,
    SpeleoCollaborator, SpeleoDbAction, SpeleoErrorDto, SpeleoFileAccess, SpeleoLinkStatus,
    SpeleoLocalProjectState, SpeleoLoginResult, SpeleoOpenResult, SpeleoProjectSummary,
    SpeleoPublishParams, SpeleoPublishResult, SpeleoReadOnlyOpen, SpeleoReleaseContext,
    SpeleoRevision, SpeleoSessionStatus, StartEditingResult, TransportCause, UnpushedLocalEdits,
    UpdateResult, Visibility,
};
use stalagmight_lib::tile_attribution::TileAttribution;
use stalagmight_lib::tile_providers::TileProviderInfo;

fn export<T: TS + 'static>(out_dir: &Path) {
    let export_to = T::EXPORT_TO
        .and_then(|path| Path::new(path).file_name())
        .expect("TS type must have an export path");
    let out_path = out_dir.join(export_to);
    T::export_to(&out_path).expect("Failed to export TS type");
    flatten_import_paths(&out_path);
}

/// Rewrite every `import type { X } from "<somewhere>/X";` to the sibling form
/// `"./X"`. ts-rs derives import specifiers from each dependency's `EXPORT_TO`
/// relative to the importing type's own, and those are crate-relative - so a
/// dependency that lives in a workspace member crate (whose exports point at
/// `../../bindings/`) comes out as `../../../bindings/X` when imported by a
/// root-crate type. That relative dance is right for the `src-tauri/bindings/`
/// freshness tree, but everything here is flattened into the single
/// `src/types/ipc/` directory, where the only correct specifier is `./X`.
fn flatten_import_paths(file: &Path) {
    let content = fs::read_to_string(file).expect("exported TS file should be readable");
    let mut changed = false;
    let flattened: Vec<String> = content
        .split('\n')
        .map(|line| {
            let from = " from \"";
            let Some(open) = line
                .starts_with("import type ")
                .then(|| line.find(from))
                .flatten()
            else {
                return line.to_owned();
            };
            let path_start = open + from.len();
            let Some(close) = line[path_start..].find('"') else {
                return line.to_owned();
            };
            let spec = &line[path_start..path_start + close];
            let stem = spec.rsplit('/').next().unwrap_or(spec);
            let flat = format!("./{stem}");
            if spec == flat {
                return line.to_owned();
            }
            changed = true;
            format!(
                "{}{flat}{}",
                &line[..path_start],
                &line[path_start + close..]
            )
        })
        .collect();
    if changed {
        fs::write(file, flattened.join("\n")).expect("Failed to rewrite TS imports");
    }
}

fn main() {
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/types/ipc");
    fs::create_dir_all(&out_dir).expect("Failed to create ipc types directory");
    // Remove generated files whose source types were retired with native transfer.
    for name in [
        "SpeleoProjectMetadataPatch",
        "SpeleoProjectMetadata",
        "SpeleoUnwindTarget",
        "SpeleoRewindTarget",
        "KeychainReport",
        "TokenFallback",
        "TokenStorage",
        "ConfigSaveReport",
        "AdoptUpload",
        "SpeleoAdoptResult",
        "DumpCandidateKind",
        "SpeleoDumpCandidate",
        "SpeleoCollaborator",
        "SpeleoCollaborators",
        "SpeleoTeamGrant",
        "SpeleoTeamOption",
        "SpeleoUserSuggestion",
        // Retired with the dead Settings > Display "Depth Gradient" section (#380).
        "DepthGradientConfig",
    ] {
        for directory in [
            &out_dir,
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bindings"),
        ] {
            let path = directory.join(format!("{name}.ts"));
            if path.exists() {
                fs::remove_file(path).expect("remove retired generated type");
            }
        }
    }

    // Core command/result types
    export::<CommandResult>(&out_dir);
    export::<LockReleaseOutcome>(&out_dir);
    export::<ReversedSection>(&out_dir);
    export::<ImportReceipt>(&out_dir);
    export::<CommentHolder>(&out_dir);
    export::<IntegrityRepairResult>(&out_dir);
    export::<IntegrityCheckResult>(&out_dir);
    export::<ValidationWarning>(&out_dir);
    export::<WarningSeverity>(&out_dir);

    // Loading/progress types
    export::<DisplaySettings>(&out_dir);
    export::<LoadingPhase>(&out_dir);
    export::<LoadingProgress>(&out_dir);
    export::<LoadProjectFullResult>(&out_dir);
    export::<CreateProjectFullResult>(&out_dir);
    export::<SpeleoManagedFile>(&out_dir);
    // [#649] A file a newer Stalagmight saved: carried on LoadError and SpeleoErrorDto.
    export::<stalagmight_lib::models::NewerSchema>(&out_dir);

    // Project/model types
    export::<Project>(&out_dir);
    export::<ProjectMetadata>(&out_dir);
    // [Track B] SpeleoDB hand-off link (#189)
    export::<SpeleoLink>(&out_dir);
    export::<ProjectStats>(&out_dir);
    export::<DistanceField>(&out_dir);
    // Gas planning (#38)
    export::<TankMaterial>(&out_dir);
    export::<CylRole>(&out_dir);
    export::<PlanMode>(&out_dir);
    export::<Diver>(&out_dir);
    export::<Cylinder>(&out_dir);
    export::<Team>(&out_dir);
    export::<PlanSegment>(&out_dir);
    export::<GasReachField>(&out_dir);
    // Dive-plan persistence (#244)
    export::<MarkedPoint>(&out_dir);
    export::<DivePlanSnapshot>(&out_dir);
    export::<DivePlanMeta>(&out_dir);
    export::<PlanAnchorSet>(&out_dir);
    export::<DivePlan>(&out_dir);
    export::<RouteLeg>(&out_dir);
    export::<RouteProfilePoint>(&out_dir);
    export::<DiveProfileRequest>(&out_dir);
    export::<RulerEndpoint>(&out_dir);
    export::<PathShot>(&out_dir);
    export::<AlongLineResult>(&out_dir);
    export::<AzimuthReference>(&out_dir);
    export::<CorrectionSource>(&out_dir);
    export::<NorthReference>(&out_dir);
    export::<Section>(&out_dir);
    // Per-shot survey credit (surveyors, explorers, date as people)
    export::<Person>(&out_dir);
    export::<SurveyCredit>(&out_dir);
    export::<CreditTargets>(&out_dir);
    export::<CreditEdit>(&out_dir);
    export::<CreditedPerson>(&out_dir);
    export::<SectionCreditSummary>(&out_dir);
    export::<SectionData>(&out_dir);
    export::<SectionUpdates>(&out_dir);
    export::<SectionStats>(&out_dir);
    export::<SurveyDirection>(&out_dir);
    export::<ImportSectionConfig>(&out_dir);
    export::<ImportPlacement>(&out_dir);
    export::<ExistingShotUpdate>(&out_dir);
    export::<Shot>(&out_dir);
    export::<CombineCandidate>(&out_dir);
    export::<CombinePick>(&out_dir);
    export::<CombineSectionsOptions>(&out_dir);
    export::<CombineJoinEnds>(&out_dir);
    export::<CombineJoinKind>(&out_dir);
    export::<CombinePlan>(&out_dir);
    export::<TornTie>(&out_dir);
    export::<TornTieKind>(&out_dir);
    export::<TornTieReason>(&out_dir);
    export::<KeptMovesReason>(&out_dir);
    export::<SectionEnd>(&out_dir);
    export::<SplitAt>(&out_dir);
    export::<SplitPreview>(&out_dir);
    export::<SplitSectionOptions>(&out_dir);
    export::<SplitSide>(&out_dir);
    export::<ShotData>(&out_dir);
    export::<ShotKind>(&out_dir);
    export::<UnrecognisedShotKind>(&out_dir);
    export::<VirtualShotMode>(&out_dir);
    export::<ShotUpdates>(&out_dir);
    export::<Station>(&out_dir);
    export::<StationRef>(&out_dir);
    export::<StationMergeWire>(&out_dir);
    export::<ZeroDatumWire>(&out_dir);
    export::<LoopClosureWire>(&out_dir);
    export::<ClosureRef>(&out_dir);
    export::<ClosureStrategy>(&out_dir);
    export::<Coordinates>(&out_dir);
    export::<GpsCoordinates>(&out_dir);
    export::<GpsBounds>(&out_dir);
    export::<SectionRelationships>(&out_dir);
    export::<StationPair>(&out_dir);
    export::<ClosurePair>(&out_dir);
    export::<ClosureUpdate>(&out_dir);
    export::<DeleteShotImpact>(&out_dir);
    export::<RangeDeleteKind>(&out_dir);
    export::<DeleteShotRangeImpact>(&out_dir);
    export::<ConnectivityResult>(&out_dir);
    export::<ClosurePreview>(&out_dir);
    export::<ProjectWarning>(&out_dir);
    export::<FrozenShotRef>(&out_dir);
    export::<ProjectState>(&out_dir);

    // Placement suggestions
    export::<MatchDirection>(&out_dir);
    export::<SuggestionConfidence>(&out_dir);
    export::<PlacementSuggestion>(&out_dir);
    export::<TargetType>(&out_dir);
    export::<CrossSectionSuggestion>(&out_dir);
    export::<ImportSectionForSuggestion>(&out_dir);
    export::<OrphanPlacementSuggestion>(&out_dir);

    // Undo history
    export::<UndoHistoryItem>(&out_dir);
    export::<PreviewData>(&out_dir);
    export::<FieldChange>(&out_dir);

    // DMP import
    export::<DmpFile>(&out_dir);
    export::<DmpSection>(&out_dir);
    export::<DmpShot>(&out_dir);
    export::<DmpDirection>(&out_dir);
    export::<DmpShotType>(&out_dir);

    // GPX import
    export::<GpxFile>(&out_dir);
    export::<GpxWaypoint>(&out_dir);
    export::<GpxTrack>(&out_dir);
    export::<GpxTrackSegment>(&out_dir);
    export::<GpxTrackPoint>(&out_dir);
    export::<GpxRoute>(&out_dir);
    export::<GpxRoutePoint>(&out_dir);
    export::<GpxSectionPlacement>(&out_dir);

    // TML/TMLU import
    export::<TmlFile>(&out_dir);
    export::<TmlSection>(&out_dir);
    export::<TmlShot>(&out_dir);
    export::<TmlShotType>(&out_dir);
    export::<TmlUnit>(&out_dir);
    export::<TmlGeoCoding>(&out_dir);
    export::<TmlClosure>(&out_dir);
    export::<TmlParseAnomalies>(&out_dir);
    export::<TmlRootPlacement>(&out_dir);

    // Compass import
    export::<CompassFile>(&out_dir);
    export::<CompassSurveySummary>(&out_dir);
    export::<InvalidShotCounts>(&out_dir);

    // Walls import
    export::<WallsFile>(&out_dir);
    export::<WallsSurveySummary>(&out_dir);

    // Export/image types
    export::<ExportImageOptions>(&out_dir);
    export::<ImageFormat>(&out_dir);
    export::<ColorMode>(&out_dir);
    export::<ExportColorScale>(&out_dir);
    export::<ExportLevels>(&out_dir);
    export::<ExportValueKind>(&out_dir);
    export::<ValueHistogram>(&out_dir);

    // Cache + tiles
    export::<ProviderStats>(&out_dir);
    export::<CacheStats>(&out_dir);
    export::<TileFetchStatsResponse>(&out_dir);
    export::<TileProviderInfo>(&out_dir);
    export::<TileAttribution>(&out_dir);

    // Segment geometry (for import preview)
    export::<SegmentGeometry>(&out_dir);
    export::<SegmentSectionGeometry>(&out_dir);

    // Survey geometry
    export::<GeometryShot>(&out_dir);
    export::<SurveyGeometry>(&out_dir);
    export::<SurveyGeometryError>(&out_dir);

    // Memory probe (debug)
    export::<ProcessMemory>(&out_dir);
    export::<MemoryStats>(&out_dir);

    // Recent-logs ring buffer (debug dialog + agent-harness checkpoint reads)
    export::<RingLogEntry>(&out_dir);
    export::<LogsSince>(&out_dir);

    // Recovery (SPEC 14.4)
    export::<BackupInfo>(&out_dir);
    export::<RepairResult>(&out_dir);
    export::<SalvagedData>(&out_dir);

    // Raw table browsing (data table fullscreen)
    export::<RawTableData>(&out_dir);

    // Config types
    export::<Config>(&out_dir);
    export::<GeneralConfig>(&out_dir);
    export::<UnitsConfig>(&out_dir);
    export::<GasConfig>(&out_dir);
    export::<MapPaletteConfig>(&out_dir);
    export::<PaletteChoiceConfig>(&out_dir);
    export::<IssueCombine>(&out_dir);
    export::<PeopleConfig>(&out_dir);
    export::<GasUnits>(&out_dir);
    export::<ReservePreset>(&out_dir);
    export::<ReserveProfile>(&out_dir);
    export::<DpvUnit>(&out_dir);
    export::<DpvRig>(&out_dir);
    export::<DisplayConfig>(&out_dir);
    export::<SectionColorsConfig>(&out_dir);
    export::<GpsOrigin>(&out_dir);
    export::<StationConfig>(&out_dir);
    export::<ShotConfig>(&out_dir);
    export::<MapSnapConfig>(&out_dir);
    export::<MapAnimationConfig>(&out_dir);
    export::<DebugConfig>(&out_dir);
    export::<MapDebugConfig>(&out_dir);
    export::<MapConfig>(&out_dir);
    export::<MapControlsConfig>(&out_dir);
    export::<ValidationConfig>(&out_dir);
    export::<ClosureAdjustmentConfig>(&out_dir);
    export::<PlacementSuggestionConfig>(&out_dir);
    export::<ExportConfig>(&out_dir);
    export::<LoggingConfig>(&out_dir);
    export::<ShortcutsConfig>(&out_dir);
    export::<PanelsConfig>(&out_dir);
    export::<CacheConfig>(&out_dir);
    export::<ProviderCacheConfig>(&out_dir);
    export::<GlobalViewConfig>(&out_dir);
    export::<AppearanceConfig>(&out_dir);
    export::<ImportConfig>(&out_dir);
    export::<SyntacticHighlightingConfig>(&out_dir);
    export::<SyntacticHighlightLevel>(&out_dir);
    export::<SyntacticHighlightPreset>(&out_dir);
    export::<OverlapMode>(&out_dir);
    export::<DecimalPrecision>(&out_dir);
    export::<LengthUnit>(&out_dir);
    export::<AngleUnit>(&out_dir);
    export::<ShortcutFeedback>(&out_dir);
    export::<UpdateTrack>(&out_dir);
    export::<ThemeMode>(&out_dir);
    export::<ColorTheme>(&out_dir);
    export::<SpeleoDbConfig>(&out_dir);
    // In-app updater (#579): the check answer, its status union, and the
    // download-progress event payload.
    export::<AppUpdateCheck>(&out_dir);
    export::<AppUpdateStatus>(&out_dir);
    export::<AppUpdateProgress>(&out_dir);
    export::<AppUpdateInstallError>(&out_dir);
    // Import from MNemo over USB (#725).
    export::<MnemoPort>(&out_dir);
    export::<MnemoPullProgress>(&out_dir);
    export::<MnemoPullResult>(&out_dir);
    export::<MnemoError>(&out_dir);
    // SpeleoDB IPC DTOs (#183; #186/D-B2 added SpeleoDbAction + SpeleoErrorDto)
    export::<Permission>(&out_dir);
    export::<SpeleoProjectSummary>(&out_dir);
    export::<SpeleoFileAccess>(&out_dir);
    export::<SpeleoAccessVerification>(&out_dir);
    export::<SpeleoReleaseContext>(&out_dir);
    export::<SpeleoReleaseContext>(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bindings"));
    export::<SpeleoLoginResult>(&out_dir);
    export::<SpeleoOpenResult>(&out_dir);
    // Why an open returned the cached copy read-only without a session.
    export::<SpeleoReadOnlyOpen>(&out_dir);
    // Cached SpeleoDB copies on the home screen (owner-aware) and the cache purge.
    export::<SpeleoCacheOwnership>(&out_dir);
    export::<SpeleoCachedProject>(&out_dir);
    export::<SpeleoPurgeReport>(&out_dir);
    // #214 adopt-Ariane result (import + link + type-flip outcome), and what the
    // adoption's own hand-over to the server managed to do
    // #218 revision-history view: one projected commit
    export::<SpeleoRevision>(&out_dir);
    // [#303] The per-project policy for whether undo history rides to the server at all.
    export::<stalagmight_lib::models::UndoMode>(&out_dir);
    export::<SpeleoSessionStatus>(&out_dir);
    export::<SpeleoDbAction>(&out_dir);
    export::<SpeleoErrorDto>(&out_dir);
    // #270: why a transport call never reached the server, so the FE can render a
    // translated line instead of a raw OS errno.
    export::<TransportCause>(&out_dir);
    // Why the endpoint policy refused a server address, so the FE can point at the
    // setting to change instead of showing the backend's refusal text.
    export::<EndpointRefusal>(&out_dir);
    // The un-pushed local work a refused re-pull would have destroyed, carried on
    // SpeleoErrorDto so the FE can offer keep-local vs take-remote-and-discard.
    // [#258] Which ambiguity the candidates describe, so the picker offers the right
    // verb instead of inferring it from their extensions.
    export::<UnpushedLocalEdits>(&out_dir);
    // SpeleoDB linked-project status for the action shell (#204) + remote-state (T2c)
    export::<SpeleoLinkStatus>(&out_dir);
    export::<RemoteState>(&out_dir);
    // Per-project LOCAL state (cached copy + un-pushed edits) for the held-lock /
    // GlobalView project joins
    export::<SpeleoLocalProjectState>(&out_dir);
    // SpeleoDB single-writer hand-off DTOs (#194/#197) + push-without-unlock (T2d)
    export::<StartEditingResult>(&out_dir);
    export::<DoneResult>(&out_dir);
    export::<UpdateResult>(&out_dir);
    export::<PushResult>(&out_dir);
    // SpeleoDB Project Settings metadata (#206) + create-on-SDB publish (#207)
    export::<Visibility>(&out_dir);
    export::<SpeleoCollaborator>(&out_dir);
    export::<SpeleoPublishParams>(&out_dir);
    export::<SpeleoPublishResult>(&out_dir);
    // #215 full Project Settings: collaborator roster (users + teams) + team picker

    let export_list = [
        "CommandResult",
        "LockReleaseOutcome",
        "ReversedSection",
        "ImportReceipt",
        "CommentHolder",
        "IntegrityRepairResult",
        "IntegrityCheckResult",
        "ValidationWarning",
        "WarningSeverity",
        "DisplaySettings",
        "LoadingPhase",
        "LoadingProgress",
        "LoadProjectFullResult",
        "CreateProjectFullResult",
        "SpeleoManagedFile",
        "NewerSchema",
        "Project",
        "ProjectMetadata",
        "ProjectStats",
        "DistanceField",
        "TankMaterial",
        "CylRole",
        "PlanMode",
        "Diver",
        "Cylinder",
        "Team",
        "PlanSegment",
        "GasReachField",
        "MarkedPoint",
        "DivePlanSnapshot",
        "DivePlanMeta",
        "PlanAnchorSet",
        "DivePlan",
        "RouteLeg",
        "RouteProfilePoint",
        "DiveProfileRequest",
        "RulerEndpoint",
        "PathShot",
        "AlongLineResult",
        "AzimuthReference",
        "CorrectionSource",
        "NorthReference",
        "Section",
        "Person",
        "SurveyCredit",
        "CreditTargets",
        "CreditEdit",
        "CreditedPerson",
        "SectionCreditSummary",
        "SectionData",
        "SectionUpdates",
        "SectionStats",
        "SurveyDirection",
        "ImportSectionConfig",
        "ImportPlacement",
        "Shot",
        "ShotData",
        "ShotKind",
        "UnrecognisedShotKind",
        "VirtualShotMode",
        "ShotUpdates",
        "SplitAt",
        "SplitPreview",
        "SplitSectionOptions",
        "SplitSide",
        "CombineCandidate",
        "CombinePick",
        "CombineSectionsOptions",
        "CombineJoinEnds",
        "CombineJoinKind",
        "CombinePlan",
        "TornTie",
        "TornTieKind",
        "TornTieReason",
        "KeptMovesReason",
        "SectionEnd",
        "Station",
        "StationRef",
        "StationMerge",
        "ZeroDatum",
        "LoopClosure",
        "ClosureRef",
        "ClosureStrategy",
        "Coordinates",
        "GpsCoordinates",
        "GpsBounds",
        "SectionRelationships",
        "StationPair",
        "ClosurePair",
        "ClosureUpdate",
        "DeleteShotImpact",
        "RangeDeleteKind",
        "DeleteShotRangeImpact",
        "ConnectivityResult",
        "ClosurePreview",
        "ProjectWarning",
        "FrozenShotRef",
        "ProjectState",
        "MatchDirection",
        "SuggestionConfidence",
        "PlacementSuggestion",
        "TargetType",
        "CrossSectionSuggestion",
        "ImportSectionForSuggestion",
        "OrphanPlacementSuggestion",
        "UndoHistoryItem",
        "PreviewData",
        "FieldChange",
        "DmpFile",
        "DmpSection",
        "DmpShot",
        "DmpDirection",
        "DmpShotType",
        "GpxFile",
        "GpxWaypoint",
        "GpxTrack",
        "GpxTrackSegment",
        "GpxTrackPoint",
        "GpxRoute",
        "GpxRoutePoint",
        "GpxSectionPlacement",
        "TmlFile",
        "TmlSection",
        "TmlShot",
        "TmlShotType",
        "TmlUnit",
        "TmlGeoCoding",
        "TmlClosure",
        "TmlParseAnomalies",
        "TmlRootPlacement",
        "CompassFile",
        "CompassSurveySummary",
        "InvalidShotCounts",
        "WallsFile",
        "WallsSurveySummary",
        "ExportImageOptions",
        "ImageFormat",
        "ColorMode",
        "ExportColorScale",
        "ExportLevels",
        "ExportValueKind",
        "ValueHistogram",
        "ProviderStats",
        "CacheStats",
        "TileFetchStatsResponse",
        "TileProviderInfo",
        "TileAttribution",
        "SegmentGeometry",
        "SegmentSectionGeometry",
        "GeometryShot",
        "SurveyGeometry",
        "SurveyGeometryError",
        "ProcessMemory",
        "MemoryStats",
        "RingLogEntry",
        "LogsSince",
        "BackupInfo",
        "RepairResult",
        "SalvagedData",
        "RawTableData",
        "Config",
        "GeneralConfig",
        "UnitsConfig",
        "DisplayConfig",
        "SectionColorsConfig",
        "GpsOrigin",
        "StationConfig",
        "ShotConfig",
        "MapSnapConfig",
        "MapAnimationConfig",
        "DebugConfig",
        "MapDebugConfig",
        "MapConfig",
        "MapControlsConfig",
        "ValidationConfig",
        "ClosureAdjustmentConfig",
        "PlacementSuggestionConfig",
        "ExportConfig",
        "LoggingConfig",
        "ShortcutsConfig",
        "PanelsConfig",
        "CacheConfig",
        "ProviderCacheConfig",
        "GlobalViewConfig",
        "AppearanceConfig",
        "ImportConfig",
        "SyntacticHighlightingConfig",
        "SyntacticHighlightLevel",
        "SyntacticHighlightPreset",
        "OverlapMode",
        "DecimalPrecision",
        "LengthUnit",
        "AngleUnit",
        "GasConfig",
        "MapPaletteConfig",
        "PaletteChoiceConfig",
        "IssueCombine",
        "PeopleConfig",
        "GasUnits",
        "ReservePreset",
        "ReserveProfile",
        "DpvUnit",
        "DpvRig",
        "ShortcutFeedback",
        "UpdateTrack",
        "ThemeMode",
        "ColorTheme",
        "SpeleoDbConfig",
        "AppUpdateCheck",
        "AppUpdateStatus",
        "AppUpdateProgress",
        "AppUpdateInstallError",
        "MnemoPort",
        "MnemoPullProgress",
        "MnemoPullResult",
        "MnemoError",
        "Permission",
        "SpeleoProjectSummary",
        "SpeleoFileAccess",
        "SpeleoAccessVerification",
        "SpeleoReleaseContext",
        "SpeleoLoginResult",
        "SpeleoOpenResult",
        "SpeleoReadOnlyOpen",
        "SpeleoCacheOwnership",
        "SpeleoCachedProject",
        "SpeleoPurgeReport",
        "SpeleoRevision",
        "UndoMode",
        "SpeleoSessionStatus",
        "SpeleoDbAction",
        "SpeleoErrorDto",
        "TransportCause",
        "EndpointRefusal",
        "UnpushedLocalEdits",
        "SpeleoLinkStatus",
        "RemoteState",
        "SpeleoLocalProjectState",
        "StartEditingResult",
        "DoneResult",
        "UpdateResult",
        "PushResult",
        "Visibility",
        "SpeleoCollaborator",
        "SpeleoPublishParams",
        "SpeleoPublishResult",
    ];

    let mut generated =
        String::from("// This file is generated by `pnpm gen:ipc-types`. Do not edit by hand.\n");
    for name in export_list {
        let _ = writeln!(generated, "export * from './ipc/{name}';");
    }

    let generated_path = out_dir
        .parent()
        .expect("ipc types directory should have a parent")
        .join("ipc.generated.ts");
    fs::write(generated_path, generated).expect("Failed to write ipc.generated.ts");

    // `Project`'s station_merges/zero_datums/loop_closures fields are declared
    // with `#[ts(type = "Array<...>")]` so the TS shape stays the positional wire
    // form while the in-memory fields are StationKey-based. ts-rs 7 treats a
    // `type` override as an opaque string and emits no import for it, so inject
    // the imports the interface body references. The wire structs export those
    // names (via `#[ts(rename = ...)]`) to `bindings/<Name>.ts`, flattened to
    // `ipc/<Name>.ts` here, so a sibling `./<Name>` import resolves.
    ensure_imports(
        &out_dir.join("Project.ts"),
        &[
            ("StationMerge", "./StationMerge"),
            ("ZeroDatum", "./ZeroDatum"),
            ("LoopClosure", "./LoopClosure"),
        ],
    );

    write_default_config(&out_dir);
}

/// Emit `Config::default()`, serialized through the same `serde_json` path the
/// real `get_config` IPC command uses, as a typed TS module. This is the single
/// source of truth for the frontend's `defaultConfig` (`src/types/config.ts`
/// imports and re-exports it) so the two can never drift the way a hand-written
/// TS literal can.
fn write_default_config(out_dir: &Path) {
    use stalagmight_lib::config::Config;

    let json = serde_json::to_string_pretty(&Config::default())
        .expect("Failed to serialize Config::default()");
    let contents = format!(
        "// This file is generated by `pnpm gen:ipc-types` (Config::default(), serialized). Do not edit by hand.\nimport type {{ Config }} from './Config';\n\nexport const defaultConfig: Config = {json};\n"
    );
    fs::write(out_dir.join("defaultConfig.ts"), contents)
        .expect("Failed to write defaultConfig.ts");
}

/// Ensure each `(name, path)` is imported in `file`. Missing imports are added
/// and ALL `import type` lines are re-sorted alphabetically to match ts-rs's own
/// deterministic ordering (so the output is byte-identical regardless of which
/// imports ts-rs emitted vs. which we injected).
fn ensure_imports(file: &Path, imports: &[(&str, &str)]) {
    let content = fs::read_to_string(file).expect("Project.ts should exist after export");
    let lines: Vec<&str> = content.split('\n').collect();

    let header: Vec<String> = lines
        .iter()
        .take_while(|l| l.starts_with("//"))
        .map(|s| s.to_string())
        .collect();
    let mut import_lines: Vec<String> = lines
        .iter()
        .filter(|l| l.starts_with("import type "))
        .map(|s| s.to_string())
        .collect();
    let body: Vec<String> = lines
        .iter()
        .skip_while(|l| l.starts_with("//"))
        .filter(|l| !l.starts_with("import type "))
        .map(|s| s.to_string())
        .collect();

    for (name, path) in imports {
        let line = format!("import type {{ {name} }} from \"{path}\";");
        if !import_lines
            .iter()
            .any(|l| l.contains(&format!("{{ {name} }}")))
        {
            import_lines.push(line);
        }
    }
    import_lines.sort();

    let mut out = header;
    out.extend(import_lines);
    out.extend(body);
    fs::write(file, out.join("\n")).expect("Failed to rewrite Project.ts imports");
}
