#![expect(clippy::expect_used)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use ts_rs::TS;

use stalagmight_lib::commands::cache::{CacheStats, ProviderStats, TileFetchStatsResponse};
use stalagmight_lib::commands::history::{FieldChange, PreviewData, UndoHistoryItem};
use stalagmight_lib::commands::memory::{MemoryStats, ProcessMemory};
use stalagmight_lib::commands::project::ProjectState;
use stalagmight_lib::commands::section::{SegmentGeometry, SegmentSectionGeometry};
use stalagmight_lib::commands::survey_geometry::{
    GeometryShot, SurveyGeometry, SurveyGeometryError,
};
use stalagmight_lib::config::{
    AngleUnit, AppearanceConfig, CacheConfig, ClosureAdjustmentConfig, ColorTheme, Config,
    DebugConfig, DecimalPrecision, DepthGradientConfig, DisplayConfig, DpvProfile, ExportConfig,
    GasConfig, GasUnits, GeneralConfig, GlobalViewConfig, GpsOrigin, ImportConfig, LengthUnit,
    LoggingConfig, MapAnimationConfig, MapConfig, MapControlsConfig, MapDebugConfig, MapSnapConfig,
    OverlapMode, PanelsConfig, PlacementSuggestionConfig, ProviderCacheConfig, ReservePreset,
    ReserveProfile, SectionColorsConfig, ShortcutFeedback, ShortcutsConfig, ShotConfig,
    SpeleoDbConfig, StationConfig, SyntacticHighlightLevel, SyntacticHighlightPreset,
    SyntacticHighlightingConfig, ThemeMode, TokenFallback, UnitsConfig, ValidationConfig,
};
use stalagmight_lib::core::command::{
    CommandResult, CommentHolder, IntegrityCheckResult, IntegrityRepairResult, ValidationWarning,
    WarningSeverity,
};
use stalagmight_lib::core::loading::{
    CreateProjectFullResult, DisplaySettings, LoadProjectFullResult, LoadingPhase, LoadingProgress,
};
use stalagmight_lib::core::reconcile::{
    AddedSection, AddedShot, CollabChangeSet, CollabFieldChange, CollabWarning, EditedShot,
    MatchQuality, RemovedShot, SectionMetaChange,
};
use stalagmight_lib::export::{ColorMode, ExportImageOptions, ImageFormat};
use stalagmight_lib::logging::{LogsSince, RingLogEntry};
use stalagmight_lib::models::placement_suggest::{
    CrossSectionSuggestion, ImportSectionForSuggestion, MatchDirection, OrphanPlacementSuggestion,
    PlacementSuggestion, SuggestionConfidence, TargetType,
};
use stalagmight_lib::models::{
    AlongLineResult, ClosurePair, ClosurePreview, ClosureRef, ClosureStrategy, ClosureUpdate,
    CollabState, ConnectivityResult, Coordinates, CylRole, Cylinder, DeleteShotImpact,
<<<<<<< HEAD
    DeleteShotRangeImpact, DistanceField, DivePlan, DivePlanMeta, DivePlanSnapshot, Diver,
    ExistingShotUpdate, FrozenShotRef, GasReachField, GpsBounds, GpsCoordinates, ImportPlacement,
    ImportSectionConfig, LoopClosureWire, Marker, MarkerDirection, MarkerType, MarkerUpdates,
    PathShot, PlanMode, Project, ProjectMetadata,
    ProjectStats, ProjectWarning, RangeDeleteKind, RouteLeg, RulerEndpoint, Section, SectionData,
    SectionRelationships, SectionStats, SectionUpdates, Shot, ShotBaselineEntry, ShotData,
    ShotUpdates, SpeleoLink, Station, StationMergeWire, StationPair, StationRef, SurveyDirection,
    TankMaterial, Team, ZeroDatumWire,
};
use stalagmight_lib::parsers::compass::{CompassFile, CompassSurveySummary};
use stalagmight_lib::parsers::dmp::{DmpDirection, DmpFile, DmpSection, DmpShot, DmpShotType};
use stalagmight_lib::parsers::gpx::{
    GpxFile, GpxRoute, GpxRoutePoint, GpxSectionPlacement, GpxTrack, GpxTrackPoint,
    GpxTrackSegment, GpxWaypoint,
};
use stalagmight_lib::parsers::tml::{
    TmlClosure, TmlFile, TmlGeoCoding, TmlSection, TmlShot, TmlShotType, TmlUnit,
};
use stalagmight_lib::parsers::walls::{WallsFile, WallsSurveySummary};
use stalagmight_lib::persistence::{BackupInfo, RawTableData, RepairResult, SalvagedData};
use stalagmight_lib::speleodb::{
    DoneResult, Permission, PushResult, RemoteState, SpeleoAdoptResult, SpeleoCollaborator,
    SpeleoCollaborators, SpeleoDbAction, SpeleoErrorDto, SpeleoLinkStatus, SpeleoLoginResult,
    SpeleoOpenResult, SpeleoProjectMetadata, SpeleoProjectMetadataPatch, SpeleoProjectSummary,
    SpeleoPublishParams, SpeleoPublishResult, SpeleoRevision, SpeleoSessionStatus, SpeleoTeamGrant,
    SpeleoTeamOption, SpeleoUserSuggestion, StartEditingResult, UpdateResult, Visibility,
};
use stalagmight_lib::tile_providers::TileProviderInfo;

fn export<T: TS + 'static>(out_dir: &Path) {
    let export_to = T::EXPORT_TO
        .and_then(|path| Path::new(path).file_name())
        .expect("TS type must have an export path");
    let out_path = out_dir.join(export_to);
    T::export_to(out_path).expect("Failed to export TS type");
}

fn main() {
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/types/ipc");
    fs::create_dir_all(&out_dir).expect("Failed to create ipc types directory");

    // Core command/result types
    export::<CommandResult>(&out_dir);
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

    // Project/model types
    export::<Project>(&out_dir);
    // Collaboration mode
    export::<CollabState>(&out_dir);
    export::<ShotBaselineEntry>(&out_dir);
    export::<CollabChangeSet>(&out_dir);
    export::<CollabFieldChange>(&out_dir);
    export::<AddedShot>(&out_dir);
    export::<EditedShot>(&out_dir);
    export::<RemovedShot>(&out_dir);
    export::<AddedSection>(&out_dir);
    export::<SectionMetaChange>(&out_dir);
    export::<CollabWarning>(&out_dir);
    export::<MatchQuality>(&out_dir);
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
    export::<GasReachField>(&out_dir);
    // Dive-plan persistence (#244)
    export::<DivePlanSnapshot>(&out_dir);
    export::<DivePlanMeta>(&out_dir);
    export::<DivePlan>(&out_dir);
    export::<RouteLeg>(&out_dir);
    export::<RulerEndpoint>(&out_dir);
    export::<PathShot>(&out_dir);
    export::<AlongLineResult>(&out_dir);
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
    // Markers (#256)
    export::<Marker>(&out_dir);
    export::<MarkerType>(&out_dir);
    export::<MarkerDirection>(&out_dir);
    export::<MarkerUpdates>(&out_dir);

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

    // Compass import
    export::<CompassFile>(&out_dir);
    export::<CompassSurveySummary>(&out_dir);

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
    export::<GasUnits>(&out_dir);
    export::<ReservePreset>(&out_dir);
    export::<ReserveProfile>(&out_dir);
    export::<DpvProfile>(&out_dir);
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
    // SpeleoDB IPC DTOs (#183; #186/D-B2 added SpeleoDbAction + SpeleoErrorDto)
    export::<Permission>(&out_dir);
    export::<SpeleoProjectSummary>(&out_dir);
    export::<SpeleoLoginResult>(&out_dir);
    export::<SpeleoOpenResult>(&out_dir);
    // #214 adopt-Ariane result (import + link + type-flip outcome)
    export::<SpeleoAdoptResult>(&out_dir);
    // #218 revision-history view: one projected commit
    export::<SpeleoRevision>(&out_dir);
    export::<SpeleoSessionStatus>(&out_dir);
    export::<SpeleoDbAction>(&out_dir);
    export::<SpeleoErrorDto>(&out_dir);
    // SpeleoDB linked-project status for the action shell (#204) + remote-state (T2c)
    export::<SpeleoLinkStatus>(&out_dir);
    export::<RemoteState>(&out_dir);
    // SpeleoDB single-writer hand-off DTOs (#194/#197) + push-without-unlock (T2d)
    export::<StartEditingResult>(&out_dir);
    export::<DoneResult>(&out_dir);
    export::<UpdateResult>(&out_dir);
    export::<PushResult>(&out_dir);
    // SpeleoDB Project Settings metadata (#206) + create-on-SDB publish (#207)
    export::<Visibility>(&out_dir);
    export::<SpeleoProjectMetadata>(&out_dir);
    export::<SpeleoProjectMetadataPatch>(&out_dir);
    export::<SpeleoPublishParams>(&out_dir);
    export::<SpeleoPublishResult>(&out_dir);
    // #215 full Project Settings: collaborator roster (users + teams) + team picker
    export::<SpeleoCollaborator>(&out_dir);
    export::<SpeleoTeamGrant>(&out_dir);
    export::<SpeleoCollaborators>(&out_dir);
    export::<SpeleoTeamOption>(&out_dir);
    export::<SpeleoUserSuggestion>(&out_dir);

    let export_list = [
        "CommandResult",
        "CommentHolder",
        "IntegrityRepairResult",
        "IntegrityCheckResult",
        "ValidationWarning",
        "WarningSeverity",
        // Collaboration mode
        "CollabState",
        "ShotBaselineEntry",
        "CollabChangeSet",
        "CollabFieldChange",
        "AddedShot",
        "EditedShot",
        "RemovedShot",
        "AddedSection",
        "SectionMetaChange",
        "CollabWarning",
        "MatchQuality",
        "DisplaySettings",
        "LoadingPhase",
        "LoadingProgress",
        "LoadProjectFullResult",
        "CreateProjectFullResult",
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
        "GasReachField",
        "DivePlanSnapshot",
        "DivePlanMeta",
        "DivePlan",
        "RouteLeg",
        "RulerEndpoint",
        "PathShot",
        "AlongLineResult",
        "Section",
        "SectionData",
        "SectionUpdates",
        "SectionStats",
        "SurveyDirection",
        "ImportSectionConfig",
        "ImportPlacement",
        "Shot",
        "ShotData",
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
        "Marker",
        "MarkerType",
        "MarkerDirection",
        "MarkerUpdates",
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
        "CompassFile",
        "CompassSurveySummary",
        "WallsFile",
        "WallsSurveySummary",
        "ExportImageOptions",
        "ImageFormat",
        "ColorMode",
        "ProviderStats",
        "CacheStats",
        "TileFetchStatsResponse",
        "TileProviderInfo",
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
        "DpvProfile",
        "ShortcutFeedback",
        "ThemeMode",
        "ColorTheme",
        "SpeleoDbConfig",
        "TokenFallback",
        "Permission",
        "SpeleoProjectSummary",
        "SpeleoLoginResult",
        "SpeleoOpenResult",
        "SpeleoAdoptResult",
        "SpeleoRevision",
        "SpeleoSessionStatus",
        "SpeleoDbAction",
        "SpeleoErrorDto",
        "SpeleoLinkStatus",
        "RemoteState",
        "StartEditingResult",
        "DoneResult",
        "UpdateResult",
        "PushResult",
        "Visibility",
        "SpeleoProjectMetadata",
        "SpeleoProjectMetadataPatch",
        "SpeleoPublishParams",
        "SpeleoPublishResult",
        "SpeleoCollaborator",
        "SpeleoTeamGrant",
        "SpeleoCollaborators",
        "SpeleoTeamOption",
        "SpeleoUserSuggestion",
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
