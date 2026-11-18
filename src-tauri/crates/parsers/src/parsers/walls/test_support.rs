//! Shared Walls test entry points, factored out of `walls::tests` so the root
//! crate's import-pipeline tests can keep calling them: they used to be plain
//! `#[cfg(test)]` items, and a dependency's `cfg(test)` items are invisible to
//! a dependent's test build now that the parsers live in their own crate.

#![expect(
    clippy::unwrap_used,
    reason = "test-only fixture builders fail loudly on setup errors"
)]

use std::collections::HashMap;

use crate::models::line::{ImportClosure, ImportLineConfig};

use super::{
    convert_walls_to_import_configs, parse_srv_contents, ParsedSurvey, ParsedWalls,
    WallsImportOptions, WallsUnits,
};

pub fn parse_srv(content: &str) -> ParsedSurvey {
    let mut warnings = Vec::new();
    parse_srv_contents(
        content,
        "Test Survey",
        "TEST",
        "",
        &WallsUnits::default(),
        &HashMap::new(),
        None,
        true,
        false,
        &mut warnings,
    )
}

pub fn convert_one(
    survey: ParsedSurvey,
) -> (Vec<ImportLineConfig>, Vec<ImportClosure>, Vec<String>) {
    let parsed = ParsedWalls {
        project_name: "Test".to_string(),
        book_count: 0,
        surveys: vec![survey],
        georef: None,
        missing_files: Vec::new(),
        skipped_attachments: 0,
        warnings: Vec::new(),
    };
    convert_walls_to_import_configs(&parsed, &WallsImportOptions::default()).unwrap()
}
