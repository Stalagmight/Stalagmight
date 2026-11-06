#![expect(clippy::expect_used)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ts_rs::TS;

use stalagmight_lib::commands::cache::{CacheStats, ProviderStats, TileFetchStatsResponse};
use stalagmight_lib::commands::export::DiveProfileRequest;
use stalagmight_lib::commands::history::{FieldChange, PreviewData, UndoHistoryItem};
use stalagmight_lib::commands::memory::{MemoryStats, ProcessMemory};
use stalagmight_lib::commands::project::ProjectState;
use stalagmight_lib::commands::section::{SegmentGeometry, SegmentSectionGeometry};
use stalagmight_lib::commands::survey_geometry::{GeometryShot, SurveyGeometry};
use stalagmight_lib::config::{
    AngleUnit, AppearanceConfig, CacheConfig, ClosureAdjustmentConfig, ColorTheme, Config,
    ConfigPatch, ConfigSaveReport, DebugConfig, DecimalPrecision, DepthGradientConfig,
    DisplayConfig, DpvRig, DpvUnit, ExportConfig, GasConfig, GasUnits, GeneralConfig,
    GlobalViewConfig, GpsOrigin, ImportConfig, LengthUnit, LoggingConfig, MapAnimationConfig,
    MapConfig, MapControlsConfig, MapDebugConfig, MapSnapConfig, OverlapMode, PanelsConfig,
    PlacementSuggestionConfig, ProviderCacheConfig, ReservePreset, ReserveProfile,
    SectionColorsConfig, ShortcutFeedback, ShortcutsConfig, ShotConfig, SpeleoDbConfig,
    StationConfig, SyntacticHighlightLevel, SyntacticHighlightPreset, SyntacticHighlightingConfig,
    ThemeMode, TokenFallback, UnitsConfig, ValidationConfig,
};
use stalagmight_lib::core::command::{
    Command, CommentHolder, ImportReceipt, IntegrityCheckResult, IntegrityRepairResult,
    ReversedSection, SpeleoDoneReport, SpeleoPublishReport, SpeleoPushReport, ValidationWarning,
    WarningSeverity,
};
use stalagmight_lib::core::command_error::{CommandError, CommandErrorCode, HistoryStepFailure};
use stalagmight_lib::core::events::InternalErrorNotice;
use stalagmight_lib::core::loading::{
    CreateProjectFullResult, DisplaySettings, LoadProjectFullResult, LoadingPhase, LoadingProgress,
    OpenFileFlags, SpeleoManagedFile,
};
use stalagmight_lib::core::outcome::{
    BatchOutcome, DeleteProjectOutcome, EditOutcome, HistoryOutcome, ImportOutcome,
    ProjectFileOutcome, RemoteLinkBits, ReverseOutcome, SaveOutcome, SectionCreated, ShotCreated,
};
use stalagmight_lib::core::state::{DisplaySettingTarget, ProjectIdentity, VirtualShotMode};
use stalagmight_lib::core::WorkspaceMode;
use stalagmight_lib::export::{ColorMode, ExportImageOptions, ImageFormat};
use stalagmight_lib::logging::{LogsSince, RingLogEntry};
use stalagmight_lib::models::placement_suggest::{
    CrossSectionSuggestion, ImportSectionForSuggestion, MatchDirection, OrphanPlacementSuggestion,
    PlacementSuggestion, SuggestionConfidence, TargetType,
};
use stalagmight_lib::models::{
    AlongLineResult, AzimuthReference, ClosurePair, ClosurePreview, ClosureRef, ClosureStrategy,
    ClosureUpdate, ConnectivityResult, Coordinates, CorrectionSource, CylRole, Cylinder,
    DeleteShotImpact, DeleteShotRangeImpact, DistanceField, DivePlan, DivePlanMeta,
    DivePlanSnapshot, Diver, ExistingShotUpdate, FrozenShotRef, GasReachField, GpsBounds,
    GpsCoordinates, ImportPlacement, ImportSectionConfig, LoopClosureWire, MarkedPoint,
    NorthReference, PathShot, PlanAnchorSet, PlanMode, PlanSegment, Project, ProjectMetadata,
    ProjectStats, ProjectWarning, RangeDeleteKind, RouteLeg, RouteProfilePoint, RulerEndpoint,
    Section, SectionData, SectionRelationships, SectionStats, SectionUpdates, Shot, ShotData,
    ShotKind, ShotUpdates, SpeleoLink, Station, StationMergeWire, StationPair, StationRef,
    SurveyDirection, TankMaterial, Team, UnrecognisedShotKind, ZeroDatumWire,
};
use stalagmight_lib::parsers::compass::{CompassFile, CompassSurveySummary, InvalidShotCounts};
use stalagmight_lib::parsers::dmp::{DmpDirection, DmpFile, DmpSection, DmpShot, DmpShotType};
use stalagmight_lib::parsers::gpx::{
    GpxFile, GpxRoute, GpxRoutePoint, GpxSectionPlacement, GpxTrack, GpxTrackPoint,
    GpxTrackSegment, GpxWaypoint,
};
use stalagmight_lib::parsers::tml::{
    TmlClosure, TmlFile, TmlGeoCoding, TmlParseAnomalies, TmlRootPlacement, TmlScaledDepthInMarker,
    TmlSection, TmlShot, TmlShotType, TmlUnit,
};
use stalagmight_lib::parsers::walls::{WallsFile, WallsSurveySummary};
use stalagmight_lib::persistence::{
    BackupInfo, BackupKind, RawTableData, RepairResult, SalvagedData,
};
use stalagmight_lib::speleodb::{
    AdoptUpload, DoneResult, DumpCandidateKind, KeychainReport, Permission, PushResult,
    RemoteState, SpeleoAdoptResult, SpeleoCollaborator, SpeleoCollaborators, SpeleoDbAction,
    SpeleoDumpCandidate, SpeleoErrorDto, SpeleoLinkStatus, SpeleoLocalProjectState,
    SpeleoLoginResult, SpeleoOpenResult, SpeleoProjectMetadata, SpeleoProjectMetadataPatch,
    SpeleoProjectSummary, SpeleoPublishParams, SpeleoPublishResult, SpeleoRevision,
    SpeleoRewindTarget, SpeleoSessionStatus, SpeleoTeamGrant, SpeleoTeamOption, SpeleoUnwindTarget,
    SpeleoUserSuggestion, StartEditingResult, TokenStorage, TransportCause, UnpushedLocalEdits,
    UpdateResult, Visibility,
};
use stalagmight_lib::tile_attribution::TileAttribution;
use stalagmight_lib::tile_providers::TileProviderInfo;

#[path = "gen_ipc_types/commands.rs"]
mod commands;
#[path = "gen_ipc_types/events.rs"]
mod events;

/// `(Rust ident, TS name)` of every exported type: the command manifest resolves
/// the types in a `#[tauri::command]` signature against it.
static EXPORTED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

// The manifest declares these id aliases `number`; pin that they are still `u32`.
const _: fn(stalagmight_lib::models::SectionId, stalagmight_lib::models::ShotId) -> (u32, u32) =
    |section, shot| (section, shot);

fn export<T: TS + 'static>(out_dir: &Path) {
    let export_to = T::EXPORT_TO
        .and_then(|path| Path::new(path).file_name())
        .expect("TS type must have an export path");
    let out_path = out_dir.join(export_to);
    T::export_to(&out_path).expect("Failed to export TS type");
    flatten_import_paths(&out_path);
    let rust_path = std::any::type_name::<T>();
    let rust_ident = rust_path
        .split('<')
        .next()
        .and_then(|path| path.rsplit("::").next())
        .unwrap_or(rust_path);
    EXPORTED
        .lock()
        .expect("export registry lock")
        .push((rust_ident.to_string(), T::name()));
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

    // [#576 C4/A6] The one error every fallible command rejects with
    export::<CommandError>(&out_dir);
    export::<CommandErrorCode>(&out_dir);
    export::<HistoryStepFailure>(&out_dir);
    // [#576 A12/D22] The per-command success values that replaced CommandResult
    export::<RemoteLinkBits>(&out_dir);
    export::<EditOutcome>(&out_dir);
    export::<SectionCreated>(&out_dir);
    export::<ShotCreated>(&out_dir);
    export::<ReverseOutcome>(&out_dir);
    export::<BatchOutcome>(&out_dir);
    export::<HistoryOutcome>(&out_dir);
    export::<ImportOutcome>(&out_dir);
    export::<SaveOutcome>(&out_dir);
    export::<DeleteProjectOutcome>(&out_dir);
    export::<ProjectFileOutcome>(&out_dir);
    // Core command/result types
    export::<ReversedSection>(&out_dir);
    export::<ImportReceipt>(&out_dir);
    export::<CommentHolder>(&out_dir);
    export::<IntegrityRepairResult>(&out_dir);
    export::<IntegrityCheckResult>(&out_dir);
    export::<ValidationWarning>(&out_dir);
    export::<WarningSeverity>(&out_dir);
    // [#576 C3] execute_batch's payload and set_workspace_mode's argument
    export::<Command>(&out_dir);
    export::<WorkspaceMode>(&out_dir);

    // Loading/progress types
    export::<DisplaySettings>(&out_dir);
    export::<LoadingPhase>(&out_dir);
    export::<LoadingProgress>(&out_dir);
    // [#576 A11] the internal-error banner event's payload
    export::<InternalErrorNotice>(&out_dir);
    export::<LoadProjectFullResult>(&out_dir);
    export::<OpenFileFlags>(&out_dir);
    // [#576 NF-W8FE-3] save_display_setting's identity of the project a write is for
    export::<DisplaySettingTarget>(&out_dir);
    export::<ProjectIdentity>(&out_dir);
    export::<CreateProjectFullResult>(&out_dir);
    export::<SpeleoManagedFile>(&out_dir);

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
    export::<SectionData>(&out_dir);
    export::<SectionUpdates>(&out_dir);
    export::<SectionStats>(&out_dir);
    export::<SurveyDirection>(&out_dir);
    export::<ImportSectionConfig>(&out_dir);
    export::<ImportPlacement>(&out_dir);
    export::<ExistingShotUpdate>(&out_dir);
    export::<Shot>(&out_dir);
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
    export::<TmlScaledDepthInMarker>(&out_dir);
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

    // Memory probe (debug)
    export::<ProcessMemory>(&out_dir);
    export::<MemoryStats>(&out_dir);

    // Recent-logs ring buffer (debug dialog + agent-harness checkpoint reads)
    export::<RingLogEntry>(&out_dir);
    export::<LogsSince>(&out_dir);

    // Recovery (SPEC 14.4)
    export::<BackupInfo>(&out_dir);
    export::<BackupKind>(&out_dir);
    export::<RepairResult>(&out_dir);
    export::<SalvagedData>(&out_dir);

    // Raw table browsing (data table fullscreen)
    export::<RawTableData>(&out_dir);

    // Config types
    export::<Config>(&out_dir);
    export::<GeneralConfig>(&out_dir);
    export::<UnitsConfig>(&out_dir);
    export::<GasConfig>(&out_dir);
    export::<GasUnits>(&out_dir);
    export::<ReservePreset>(&out_dir);
    export::<ReserveProfile>(&out_dir);
    export::<DpvUnit>(&out_dir);
    export::<DpvRig>(&out_dir);
    export::<DisplayConfig>(&out_dir);
    export::<DepthGradientConfig>(&out_dir);
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
    export::<ThemeMode>(&out_dir);
    export::<ColorTheme>(&out_dir);
    export::<SpeleoDbConfig>(&out_dir);
    export::<TokenFallback>(&out_dir);
    export::<ConfigSaveReport>(&out_dir);
    // [#576 D27-32(c)] one changed setting, for patch_config
    export::<ConfigPatch>(&out_dir);
    // SpeleoDB IPC DTOs (#183; #186/D-B2 added SpeleoDbAction + SpeleoErrorDto)
    export::<Permission>(&out_dir);
    export::<SpeleoProjectSummary>(&out_dir);
    export::<TokenStorage>(&out_dir);
    export::<KeychainReport>(&out_dir);
    export::<SpeleoLoginResult>(&out_dir);
    export::<SpeleoOpenResult>(&out_dir);
    // #214 adopt-Ariane result (import + link + type-flip outcome), and what the
    // adoption's own hand-over to the server managed to do
    export::<AdoptUpload>(&out_dir);
    export::<SpeleoAdoptResult>(&out_dir);
    // #218 revision-history view: one projected commit
    export::<SpeleoRevision>(&out_dir);
    // [#303] The hybrid undo stack: which revision the next Unwind would load, and the
    // per-project policy for whether undo history rides to the server at all.
    export::<SpeleoUnwindTarget>(&out_dir);
    // ...and its forward twin, which a redo-side step would load.
    export::<SpeleoRewindTarget>(&out_dir);
    export::<stalagmight_lib::models::UndoMode>(&out_dir);
    export::<SpeleoSessionStatus>(&out_dir);
    // [#576 C4] No command rejects with these any more (they reject with
    // CommandError), but `SpeleoLinkStatus::server_error` still carries the DTO.
    export::<SpeleoDbAction>(&out_dir);
    export::<SpeleoErrorDto>(&out_dir);
    // #270: why a transport call never reached the server, so the FE can render a
    // translated line instead of a raw OS errno.
    export::<TransportCause>(&out_dir);
    // The un-pushed local work a refused re-pull would have destroyed, carried on
    // SpeleoErrorDto so the FE can offer keep-local vs take-remote-and-discard.
    export::<SpeleoDumpCandidate>(&out_dir);
    // [#258] Which ambiguity the candidates describe, so the picker offers the right
    // verb instead of inferring it from their extensions.
    export::<DumpCandidateKind>(&out_dir);
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
    // [#576 NF-W2SAVE-1] Done / Push answers carrying the pre-push save's warnings
    export::<SpeleoDoneReport>(&out_dir);
    export::<SpeleoPushReport>(&out_dir);
    // SpeleoDB Project Settings metadata (#206) + create-on-SDB publish (#207)
    export::<Visibility>(&out_dir);
    export::<SpeleoProjectMetadata>(&out_dir);
    export::<SpeleoProjectMetadataPatch>(&out_dir);
    export::<SpeleoPublishParams>(&out_dir);
    export::<SpeleoPublishResult>(&out_dir);
    // [#576 NF-W4M-2] The publish answer, carrying its saves' warnings
    export::<SpeleoPublishReport>(&out_dir);
    // #215 full Project Settings: collaborator roster (users + teams) + team picker
    export::<SpeleoCollaborator>(&out_dir);
    export::<SpeleoTeamGrant>(&out_dir);
    export::<SpeleoCollaborators>(&out_dir);
    export::<SpeleoTeamOption>(&out_dir);
    export::<SpeleoUserSuggestion>(&out_dir);

    let export_list = [
        "CommandError",
        "CommandErrorCode",
        "HistoryStepFailure",
        "RemoteLinkBits",
        "EditOutcome",
        "SectionCreated",
        "ShotCreated",
        "ReverseOutcome",
        "BatchOutcome",
        "HistoryOutcome",
        "ImportOutcome",
        "SaveOutcome",
        "DeleteProjectOutcome",
        "ProjectFileOutcome",
        "ReversedSection",
        "ImportReceipt",
        "CommentHolder",
        "IntegrityRepairResult",
        "IntegrityCheckResult",
        "ValidationWarning",
        "WarningSeverity",
        "Command",
        "WorkspaceMode",
        "DisplaySettings",
        "LoadingPhase",
        "LoadingProgress",
        "InternalErrorNotice",
        "LoadProjectFullResult",
        "OpenFileFlags",
        "DisplaySettingTarget",
        "ProjectIdentity",
        "CreateProjectFullResult",
        "SpeleoManagedFile",
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
        "TmlScaledDepthInMarker",
        "TmlRootPlacement",
        "CompassFile",
        "CompassSurveySummary",
        "InvalidShotCounts",
        "WallsFile",
        "WallsSurveySummary",
        "ExportImageOptions",
        "ImageFormat",
        "ColorMode",
        "ProviderStats",
        "CacheStats",
        "TileFetchStatsResponse",
        "TileProviderInfo",
        "TileAttribution",
        "SegmentGeometry",
        "SegmentSectionGeometry",
        "GeometryShot",
        "SurveyGeometry",
        "ProcessMemory",
        "MemoryStats",
        "RingLogEntry",
        "LogsSince",
        "BackupInfo",
        "BackupKind",
        "RepairResult",
        "SalvagedData",
        "RawTableData",
        "Config",
        "GeneralConfig",
        "UnitsConfig",
        "DisplayConfig",
        "DepthGradientConfig",
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
        "GasUnits",
        "ReservePreset",
        "ReserveProfile",
        "DpvUnit",
        "DpvRig",
        "ShortcutFeedback",
        "ThemeMode",
        "ColorTheme",
        "SpeleoDbConfig",
        "TokenFallback",
        "ConfigSaveReport",
        "ConfigPatch",
        "Permission",
        "SpeleoProjectSummary",
        "TokenStorage",
        "KeychainReport",
        "SpeleoLoginResult",
        "SpeleoOpenResult",
        "AdoptUpload",
        "SpeleoAdoptResult",
        "SpeleoRevision",
        "SpeleoUnwindTarget",
        "SpeleoRewindTarget",
        "UndoMode",
        "SpeleoSessionStatus",
        "SpeleoDbAction",
        "SpeleoErrorDto",
        "TransportCause",
        "SpeleoDumpCandidate",
        "DumpCandidateKind",
        "UnpushedLocalEdits",
        "SpeleoLinkStatus",
        "RemoteState",
        "SpeleoLocalProjectState",
        "StartEditingResult",
        "DoneResult",
        "UpdateResult",
        "PushResult",
        "SpeleoDoneReport",
        "SpeleoPushReport",
        "Visibility",
        "SpeleoProjectMetadata",
        "SpeleoProjectMetadataPatch",
        "SpeleoPublishParams",
        "SpeleoPublishResult",
        "SpeleoPublishReport",
        "SpeleoCollaborator",
        "SpeleoTeamGrant",
        "SpeleoCollaborators",
        "SpeleoTeamOption",
        "SpeleoUserSuggestion",
        // [#576 C3] the command manifest (written by `commands::write_manifest`)
        "IpcCommands",
        // [#576 C13] the event manifest (written by `events::write_events`)
        "IpcEvents",
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

    // [#576 C3] The command manifest, typed against everything exported above.
    let exported = EXPORTED.lock().expect("export registry lock").clone();
    commands::write_manifest(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
        &out_dir,
        &commands::TypeTable::new(&exported),
    );
    // [#576 C13] The event manifest.
    events::write_events(
        &out_dir,
        &exported.iter().map(|(_, ts)| ts.clone()).collect(),
    );

    assert_no_bigint(&out_dir);
}

/// [#576 C6] ts-rs 7 declares every `u64`/`i64` as `bigint`, but no IPC path
/// delivers one: `invoke()` results and events are JSON numbers, and the msgpack
/// load envelope is decoded with `useBigInt64: false`, which yields numbers too.
/// A `bigint` declaration is therefore a lie the Playwright mock repeats, so the
/// generator refuses to leave one in the tree. The fix is `#[ts(type = "number")]`
/// on the field - sound because every such value (ids, counters, byte sizes,
/// epoch milliseconds) stays far below 2^53, where a JS number is exact.
fn assert_no_bigint(out_dir: &Path) {
    let mut offenders: Vec<String> = fs::read_dir(out_dir)
        .expect("ipc types directory should be readable")
        .map(|entry| entry.expect("ipc types directory entry").path())
        .filter(|path| {
            fs::read_to_string(path)
                .expect("generated TS file should be readable")
                .contains("bigint")
        })
        .map(|path| path.display().to_string())
        .collect();
    offenders.sort();
    assert!(
        offenders.is_empty(),
        "generated IPC types declare `bigint`, which no IPC path delivers - add \
         #[ts(type = \"number\")] to the u64/i64 field(s) in: {offenders:?}"
    );
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
