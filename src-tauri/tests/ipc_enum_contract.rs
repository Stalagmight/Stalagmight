//! The IPC enum contract (RS-FX6, P2-3): every enum the frontend reads as a
//! string literal (a unit enum, `"old_format" | "earlier_version_2"`, or the
//! tag of a tagged one, `{ "kind": "up_to_date" } | ...`) must serialise to
//! exactly the literal ts-rs generated for it. serde and ts-rs convert
//! `rename_all` cases with different code: serde's `snake_case` writes
//! `EarlierVersion2` as `earlier_version2`, ts-rs (Inflector) as
//! `earlier_version_2`, and an acronym (`HTTPServer`) differs too. Nothing
//! else notices: `tsc` believes the declaration, the e2e mock speaks it, and
//! the frontend's lookup by the TS literal silently misses (the earlier
//! version 2 banner lost its lead that way).
//!
//! Each literal of each type's ts-rs declaration is deserialised into the
//! type and serialised back: an unknown variant, or a round trip that
//! writes another string, fails. A tagged variant with fields cannot be
//! built from its tag alone; serde then says which field is missing, which
//! proves the tag was recognised. The second test keeps the list complete:
//! every generated `src/types/ipc/*.ts` that declares a literal union must
//! be one of the types checked here, so a new IPC enum cannot skip it.
#![expect(clippy::expect_used, reason = "test setup reads the generated types")]

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use ts_rs::TS;

use stalagmight_lib::commands::app_update::{AppUpdateInstallError, AppUpdateStatus};
use stalagmight_lib::commands::mnemo::MnemoError;
use stalagmight_lib::config::{
    AngleUnit, ColorTheme, GasUnits, IssueCombine, LengthUnit, OverlapMode, ReservePreset,
    ShortcutFeedback, SyntacticHighlightLevel, SyntacticHighlightPreset, ThemeMode, UpdateTrack,
};
use stalagmight_lib::core::command::{CommentHolder, LockReleaseOutcome, WarningSeverity};
use stalagmight_lib::core::loading::LoadingPhase;
use stalagmight_lib::core::state::{
    ConversionFile, ConversionSource, SpeleoCacheOwnership, VirtualShotMode,
};
use stalagmight_lib::export::color_scale::ExportValueKind;
use stalagmight_lib::export::{ColorMode, ImageFormat};
use stalagmight_lib::models::placement_suggest::{
    MatchDirection, SuggestionConfidence, TargetType,
};
use stalagmight_lib::models::{
    ClosureEndWire, ClosureStrategy, CombineJoinKind, CombinePick, CombineRemovedKind,
    CombineRenameReason, CombineReplacedKind, CorrectionSource, CreditTargets, CylRole, DatumRole,
    ImportPlacement, KeepRefusalReason, KeptMovesReason, LineEnd, NorthReference, PlanMode,
    RangeDeleteKind, ShotKind, SplitAt, SplitSide, SurveyDirection, TankMaterial, TornTieKind,
    TornTieReason, UndoMode,
};
use stalagmight_lib::parsers::dmp::{DmpDirection, DmpShotType};
use stalagmight_lib::parsers::tml::{TmlRootPlacement, TmlShotType, TmlUnit};
use stalagmight_lib::persistence::BackupKind;
use stalagmight_lib::speleodb::{
    DoneResult, EditingStopped, EndpointRefusal, Permission, PushResult, RemoteState,
    SpeleoAccessVerification, SpeleoDbAction, SpeleoReadOnlyOpen, StartEditingResult,
    TransportCause, UpdateResult, Visibility,
};

/// One literal of a declaration and the JSON that names it on the wire.
struct Probe {
    literal: String,
    /// How the literal sits in the declaration: a unit literal, the value of
    /// a tag key (`{ "kind": "x", .. }`), or an externally tagged variant
    /// name (`{ "x": { .. } }`).
    shape: Shape,
}

enum Shape {
    Unit,
    Tag(String),
    External,
}

/// The literals of a ts-rs declaration: every quoted string, each a unit
/// literal, a tag's value, or a variant name used as an object key.
fn probes(decl: &str) -> Vec<Probe> {
    let mut quoted: Vec<(String, usize)> = Vec::new();
    let bytes = decl.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'"' {
                end += 1;
            }
            quoted.push((decl[start..end].to_owned(), end + 1));
            i = end + 1;
        } else {
            i += 1;
        }
    }
    let next_char = |at: usize| decl[at..].trim_start().chars().next();
    let mut out = Vec::new();
    let mut k = 0;
    while k < quoted.len() {
        let (text, after) = &quoted[k];
        if next_char(*after) == Some(':') {
            let colon = after + decl[*after..].find(':').expect("a colon");
            if next_char(colon + 1) == Some('"') {
                if let Some((value, _)) = quoted.get(k + 1) {
                    out.push(Probe {
                        literal: value.clone(),
                        shape: Shape::Tag(text.clone()),
                    });
                    k += 2;
                    continue;
                }
            }
            out.push(Probe {
                literal: text.clone(),
                shape: Shape::External,
            });
        } else {
            out.push(Probe {
                literal: text.clone(),
                shape: Shape::Unit,
            });
        }
        k += 1;
    }
    out
}

/// Every literal of `T`'s declaration names a variant serde reads, and
/// writes back as the same literal. The mismatches, worded.
fn mismatches<T: TS + Serialize + DeserializeOwned>() -> Vec<String> {
    let decl = T::decl();
    let probes = probes(&decl);
    if probes.is_empty() {
        return vec![format!("{}: no literal in `{decl}`", T::name())];
    }
    let mut out = Vec::new();
    for probe in probes {
        let wire = match &probe.shape {
            Shape::Unit => Value::String(probe.literal.clone()),
            Shape::Tag(key) => json!({ key.as_str(): probe.literal }),
            Shape::External => json!({ probe.literal.as_str(): null }),
        };
        match serde_json::from_value::<T>(wire.clone()) {
            Ok(value) => {
                let back = serde_json::to_value(&value).expect("serialises");
                let same = match &probe.shape {
                    Shape::Unit => back == wire,
                    Shape::Tag(key) => back.get(key) == Some(&Value::String(probe.literal.clone())),
                    Shape::External => {
                        back.get(&probe.literal).is_some()
                            || back == Value::String(probe.literal.clone())
                    }
                };
                if !same {
                    out.push(format!(
                        "{}: `{}` reads back as {back}",
                        T::name(),
                        probe.literal
                    ));
                }
            }
            Err(error) if error.to_string().contains("unknown variant") => out.push(format!(
                "{}: serde does not know the TS literal `{}` ({error})",
                T::name(),
                probe.literal
            )),
            // A tagged variant with fields: serde recognised the tag and
            // asks for the fields.
            Err(_) => {}
        }
    }
    out
}

type Check = (String, fn() -> Vec<String>);

macro_rules! checks {
    ($($t:ty),* $(,)?) => {
        vec![$((<$t as TS>::name(), mismatches::<$t> as fn() -> Vec<String>)),*]
    };
}

/// Every IPC enum the frontend reads as string literals.
fn checked() -> Vec<Check> {
    checks![
        AngleUnit,
        AppUpdateInstallError,
        AppUpdateStatus,
        BackupKind,
        ClosureEndWire,
        ClosureStrategy,
        ColorMode,
        ColorTheme,
        CombineJoinKind,
        CombinePick,
        CombineRemovedKind,
        CombineRenameReason,
        CombineReplacedKind,
        CommentHolder,
        ConversionFile,
        ConversionSource,
        CorrectionSource,
        CreditTargets,
        CylRole,
        DatumRole,
        DmpDirection,
        DmpShotType,
        DoneResult,
        EditingStopped,
        EndpointRefusal,
        ExportValueKind,
        GasUnits,
        ImageFormat,
        ImportPlacement,
        IssueCombine,
        KeepRefusalReason,
        KeptMovesReason,
        LengthUnit,
        LineEnd,
        LoadingPhase,
        LockReleaseOutcome,
        MatchDirection,
        MnemoError,
        NorthReference,
        OverlapMode,
        Permission,
        PlanMode,
        PushResult,
        RangeDeleteKind,
        RemoteState,
        ReservePreset,
        ShortcutFeedback,
        ShotKind,
        SpeleoAccessVerification,
        SpeleoCacheOwnership,
        SpeleoDbAction,
        SpeleoReadOnlyOpen,
        SplitAt,
        SplitSide,
        StartEditingResult,
        SuggestionConfidence,
        SurveyDirection,
        SyntacticHighlightLevel,
        SyntacticHighlightPreset,
        TankMaterial,
        TargetType,
        ThemeMode,
        TmlRootPlacement,
        TmlShotType,
        TmlUnit,
        TornTieKind,
        TornTieReason,
        TransportCause,
        UndoMode,
        UpdateResult,
        UpdateTrack,
        VirtualShotMode,
        Visibility,
        WarningSeverity,
    ]
}

/// Injection: drop `#[serde(rename = "earlier_version_2")]` from
/// `ConversionSource::EarlierVersion2` and this fails naming it.
#[test]
fn every_ipc_enum_literal_is_the_string_serde_writes() {
    let failures: Vec<String> = checked().iter().flat_map(|(_, check)| check()).collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The parser reads the three shapes ts-rs writes.
#[test]
fn the_literal_parser_reads_every_shape() {
    let shapes = |decl: &str| {
        probes(decl)
            .into_iter()
            .map(|p| match p.shape {
                Shape::Unit => format!("unit {}", p.literal),
                Shape::Tag(key) => format!("{key}={}", p.literal),
                Shape::External => format!("external {}", p.literal),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        shapes(r#"type A = "old_format" | "earlier_version_2";"#),
        ["unit old_format", "unit earlier_version_2"]
    );
    assert_eq!(
        shapes(r#"type B = { "kind": "failed", message: string, } | { "kind": "not_editing" };"#),
        ["kind=failed", "kind=not_editing"]
    );
    assert_eq!(
        shapes(r#"type C = "unknown" | { "igrf_auto": { epoch_year: number, } };"#),
        ["unit unknown", "external igrf_auto"]
    );
}

/// The generated types the frontend reads (`src/types/ipc/`): every file
/// that declares a literal union is a type the contract checks, so a new
/// IPC enum cannot skip it. Injection: drop a type from `checked`.
#[test]
fn every_generated_literal_union_is_checked() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/types/ipc");
    let checked: Vec<String> = checked().into_iter().map(|(name, _)| name).collect();
    let mut missing = Vec::new();
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).expect("the generated IPC types") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_none_or(|ext| ext != "ts") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a generated file");
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("export type ") else {
                continue;
            };
            let Some((name, body)) = rest.split_once(" = ") else {
                continue;
            };
            if body.starts_with('"') || body.starts_with("{ \"") {
                seen += 1;
                if !checked.iter().any(|c| c == name) {
                    missing.push(name.to_owned());
                }
            }
        }
    }
    assert!(seen > 0, "no literal union found in {}", dir.display());
    missing.sort();
    assert!(
        missing.is_empty(),
        "generated literal unions the contract does not check (add them to `checked`): {missing:?}"
    );
}
