//! A version 2 file an earlier build of version 2 wrote (no
//! `archived-stations` feature, U1 per-line aliases) in the app (plan
//! 2026-11-23 RS 8.2): it opens upgraded and then behaves as a 0.6.0 file
//! does. The status bar says it is the older file, the notice leads with
//! "Upgraded from an earlier version 2 file" and says what lines show
//! differently, its history is dropped with the banner, a local-only rewrite
//! keeps it pending, and the first save keeps the original in Recovery. A
//! SpeleoDB revision of that kind is converted on install. Synthetic files
//! only, written in the earlier form by hand.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::conversion_notice::{ConversionFile, ConversionSource};
use super::speleodb_cache::{InheritedUndo, LocalEditsPolicy};
use super::stalx_fixtures::{fixture_link, FIXTURE_ID};
use super::AppState;
use crate::core::Command;
use crate::persistence::stalx_v2::{self, DecodeOptions};

const CONVENTIONS: &str = "\"conventions\":{\"length\":\"m\",\"angles\":\"deg\",\
    \"azimuth\":\"cw-from-north\",\"inclination\":\"+down\",\"depth\":\"+down\",\
    \"local\":\"enu-m\",\"gps\":\"wgs84-deg\",\"temperature\":\"degC\"}";

/// An earlier file: Main M0 -> M1 -> M2 (10 m), Side from M2 (40 m) which
/// knew the junction as S0 (alias 9), and one undo entry of the earlier
/// history format.
fn earlier_text(name: &str) -> String {
    format!(
        "{{\"format\":\"stalx\",\"version\":2,\"solver\":1,\"requires\":[],\n{CONVENTIONS},\n\
         \"project\":{{\"name\":\"{name}\",\"created\":\"2026-11-01T00:00:00Z\",\
         \"modified\":\"2026-11-01T00:00:00Z\"}},\n\
         \"counters\":{{\"station\":20,\"closure\":1,\"section\":0}},\n\
         \"stations\":[\n[0,\"M0\"],\n[1,\"M1\"],\n\
         [2,\"M2\",{{\"comment\":\"sump\",\"aliases\":[[9,\"S0\",1]]}}],\n[3,\"S1\"],\n[4,\"S2\"]\n],\n\
         \"lines\":[\n\
         {{\"id\":0,\"name\":\"Main\",\"color\":\"#f00\",\"start\":0}},\n\
         [\"u1\",1,5.0,0.0,0.0,0.0,0.0],\n[\"u2\",2,5.0,0.0,0.0,0.0,0.0],\n\
         {{\"id\":1,\"name\":\"Side\",\"color\":\"#0f0\",\"start\":2}},\n\
         [\"u3\",3,20.0,90.0,90.0,0.0,0.0],\n[\"u4\",4,20.0,90.0,90.0,0.0,0.0]\n],\n\
         \"placements\":[\n{{\"station\":0,\"x\":0.0,\"y\":0.0}}\n],\n\
         \"history\":{{\"undo\":[\n{{\"t\":\"2026-11-01T00:00:00.000000000Z\",\"v\":1,\
         \"k\":\"update_shot\",\"label\":\"Edit shot\"}}\n]}},\n\
         \"end\":true}}\n"
    )
}

fn write_earlier(dir: &Path, stem: &str) -> PathBuf {
    let path = dir.join(format!("{stem}.stalx"));
    std::fs::write(&path, earlier_text(stem)).unwrap();
    path
}

fn open(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    let opened = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(opened.success, "{:?}", opened.error);
    state
}

/// The copies of `stem`'s originals the upgrades kept (Recovery lists them).
fn kept_originals(stem: &str) -> Vec<PathBuf> {
    let Some(folder) = super::project_io::upgrade_backup_dir() else {
        return Vec::new();
    };
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(stem))
        })
        .collect()
}

fn requires_feature(path: &Path) -> bool {
    String::from_utf8(std::fs::read(path).unwrap())
        .unwrap()
        .contains("\"requires\":[\"archived-stations\"]")
}

/// Open: upgraded in memory (the junction keeps the identity on its longest
/// line, Side), the file still the earlier one on disk, the notice with its
/// own lead, the earlier history dropped with the banner.
#[test]
fn an_earlier_file_opens_upgraded_and_says_so() {
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let stem = format!("earlier-open-{}", uuid::Uuid::new_v4());
    let path = write_earlier(dir.path(), &stem);
    let state = open(&path);

    let project = state.get_project_state().unwrap();
    let main = project.lines()[0].id;
    let junction = project.line_stations(main).unwrap()[2];
    assert_eq!(junction, 9, "the node took the kept record's Station ID");
    assert_eq!(project.shown_name(junction), Some("S0"));
    assert!(
        state.file_is_legacy(),
        "the file on disk is the earlier one"
    );
    assert!(!state.has_unsaved_changes());
    assert!(state.history_discarded_from_earlier_version());

    let notice = state.take_conversion_notice().expect("a notice");
    assert_eq!(notice.source, ConversionSource::EarlierVersion2);
    assert_eq!(notice.changes, ["Main / M2 is now shown as Side / S0"]);
    assert!(notice.brings_back);
    assert_eq!(notice.file, ConversionFile::NotYetSaved);
    assert_eq!(state.take_conversion_notice(), None, "offered once");
    assert!(!requires_feature(&path), "opening never writes");
}

/// Until the first save the file stays the earlier one: a display toggle's
/// local-only rewrite copies `requires` and the body. The first save writes
/// the upgraded survey with the feature and keeps the original byte for byte
/// in Recovery; the save says it converted the file.
#[test]
fn the_first_save_writes_the_upgrade_and_keeps_the_original() {
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let stem = format!("earlier-save-{}", uuid::Uuid::new_v4());
    let path = write_earlier(dir.path(), &stem);
    let original = std::fs::read(&path).unwrap();
    let state = open(&path);

    state.with_file(|file| {
        file.extras
            .local
            .display
            .insert("showGrid".to_string(), "false".to_string());
    });
    state.persist_display_state().unwrap();
    assert!(
        !requires_feature(&path),
        "a local-only rewrite keeps it earlier"
    );
    assert!(state.file_is_legacy());
    assert!(
        stalx_v2::decode(&std::fs::read(&path).unwrap(), &DecodeOptions::default())
            .unwrap()
            .earlier
            .is_some()
    );
    let before_save = std::fs::read(&path).unwrap();

    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "{:?}", saved.error);
    assert_eq!(saved.legacy_converted, Some(ConversionFile::Saved));
    assert!(!state.file_is_legacy());
    assert!(requires_feature(&path));
    let reopened = open(&path);
    assert!(
        !reopened.file_is_legacy(),
        "a file with the feature never is"
    );
    assert_eq!(reopened.take_conversion_notice(), None);

    let kept = kept_originals(&stem);
    assert_eq!(kept.len(), 1, "Recovery lists the original: {kept:?}");
    assert_eq!(std::fs::read(&kept[0]).unwrap(), before_save);
    assert_ne!(before_save, original, "the display toggle was written");
    for copy in kept {
        let _ = std::fs::remove_file(copy);
    }
}

/// A Save As elsewhere writes the upgraded survey to the new file and leaves
/// the earlier file as it was.
#[test]
fn a_save_as_leaves_the_earlier_file_as_it_was() {
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let stem = format!("earlier-save-as-{}", uuid::Uuid::new_v4());
    let path = write_earlier(dir.path(), &stem);
    let original = std::fs::read(&path).unwrap();
    let state = open(&path);
    let target = dir.path().join(format!("{stem}-new.stalx"));
    let saved = state.execute(Command::SaveProject {
        path: Some(target.clone()),
    });
    assert!(saved.success, "{:?}", saved.error);
    assert_eq!(saved.legacy_converted, Some(ConversionFile::SavedAsNewFile));
    assert!(!state.file_is_legacy());
    assert!(requires_feature(&target));
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

/// Add .stalx reads a source the way an open does: upgraded, never written.
#[test]
fn an_earlier_source_is_read_upgraded() {
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let path = write_earlier(dir.path(), "earlier-source");
    let original = std::fs::read(&path).unwrap();
    let state = AppState::new().unwrap();
    let source = state
        .read_source_project(&path, "earlier-source", "added")
        .unwrap();
    assert!(source.project.stations.contains_key(&9));
    assert!(!source.project.stations.contains_key(&2));
    assert_eq!(source.project.validate(), Vec::new());
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

/// A SpeleoDB revision an earlier build wrote is converted on install, as a
/// 0.6.0 revision is: the cache copy has the feature, and the open of it
/// says so with the downloaded sentence.
#[test]
fn an_earlier_revision_is_converted_on_install() {
    crate::test_helpers::with_isolated_config(|| {
        crate::app_dirs::init_for_test();
        let dir = TempDir::new().unwrap();
        let source = write_earlier(dir.path(), "download");
        let dest = dir.path().join("cache").join("project.stalx");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let state = AppState::new().unwrap();
        let generation = state.reserve_download_generation();
        let prepared = state
            .prepare_download(
                &source,
                fixture_link("https://example.com", false),
                InheritedUndo::Drop,
                &DecodeOptions::default(),
            )
            .unwrap();
        state
            .replace_cache_for_session(
                prepared,
                &dest,
                FIXTURE_ID,
                LocalEditsPolicy::Discard,
                generation,
            )
            .unwrap();
        assert!(requires_feature(&dest), "the cache copy is converted");
        let opened = state.execute(Command::OpenProject { path: dest.clone() });
        assert!(opened.success, "{:?}", opened.error);
        assert!(!state.file_is_legacy());
        let notice = state.take_conversion_notice().expect("a notice");
        assert_eq!(notice.source, ConversionSource::EarlierVersion2);
        assert_eq!(notice.file, ConversionFile::Downloaded);
    });
}
