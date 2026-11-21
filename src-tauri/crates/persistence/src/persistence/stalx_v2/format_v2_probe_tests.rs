//! The header probe and the sniff: every prefix of a file probes without a
//! panic, what a prefix reports never contradicts the whole file, and the
//! probe of a file on disk reads only its first bytes.

use super::test_helpers::{generate, MEDIUM, SMALL};
use super::{encode, probe_bytes, probe_path, sniff, Format, Mode};

#[test]
fn every_prefix_probes_consistently() {
    for seed in [1, 2, 3] {
        let (p, extras) = generate(seed, SMALL);
        let bytes = encode(&p, &extras, Mode::Normal).unwrap();
        let full = probe_bytes(&bytes);
        assert!(full.complete);
        assert_eq!(full.format, Format::V2);
        assert_eq!(full.version, Some(2));
        assert_eq!(full.project.as_ref().unwrap().name, p.name);
        assert_eq!(full.speleodb, extras.speleodb);
        assert!(full.local.as_ref().is_none_or(|l| *l == extras.local));
        let sniff_len = super::PREFIX.len();
        for cut in 0..=bytes.len() {
            let header = probe_bytes(&bytes[..cut]);
            if cut >= sniff_len {
                assert_eq!(header.format, Format::V2, "cut {cut}");
            } else {
                assert_eq!(header.format, Format::Unknown, "cut {cut}");
            }
            // A prefix reports a member only once it holds the whole member,
            // and then exactly as the whole file does.
            for (part, whole) in [
                (header.summary.is_some(), full.summary.is_some()),
                (header.project.is_some(), full.project.is_some()),
                (header.speleodb.is_some(), full.speleodb.is_some()),
                (header.local.is_some(), full.local.is_some()),
            ] {
                assert!(!part || whole, "cut {cut}");
            }
            if let Some(summary) = &header.summary {
                assert_eq!(Some(summary), full.summary.as_ref(), "cut {cut}");
            }
            if let Some(project) = &header.project {
                assert_eq!(Some(project), full.project.as_ref(), "cut {cut}");
            }
            if header.speleodb.is_some() {
                assert_eq!(header.speleodb, full.speleodb, "cut {cut}");
            }
            if header.local.is_some() {
                assert_eq!(header.local, full.local, "cut {cut}");
            }
            if header.complete {
                assert_eq!(header, full, "cut {cut}");
            }
        }
    }
}

#[test]
fn the_probe_of_a_file_reads_its_header_only() {
    let dir = tempfile::tempdir().unwrap();
    let (p, extras) = generate(4, MEDIUM);
    let bytes = encode(&p, &extras, Mode::Normal).unwrap();
    let path = dir.path().join("cave.stalx");
    std::fs::write(&path, &bytes).unwrap();
    let header = probe_path(&path).unwrap();
    assert!(header.complete);
    assert_eq!(header, probe_bytes(&bytes));
    // The header of a canonical file sits in its first few kilobytes.
    let first_body = bytes
        .windows(b"\n\"stations\":[".len())
        .position(|w| w == b"\n\"stations\":[")
        .unwrap_or(bytes.len());
    assert!(first_body < 64 * 1024);

    let legacy = dir.path().join("old.stalx");
    let mut sqlite = b"SQLite format 3\0".to_vec();
    sqlite.resize(4096, 0);
    std::fs::write(&legacy, &sqlite).unwrap();
    assert_eq!(probe_path(&legacy).unwrap().format, Format::Legacy);
    let other = dir.path().join("notes.stalx");
    std::fs::write(&other, b"hello").unwrap();
    assert_eq!(probe_path(&other).unwrap().format, Format::Unknown);
}

#[test]
fn the_sniff() {
    assert_eq!(sniff(b"SQLite format 3\0rest"), Format::Legacy);
    assert_eq!(sniff(b"{\"format\":\"stalx\",\"version\":2,"), Format::V2);
    assert_eq!(sniff(b"{\"format\":\"stalx\",\"version\":9"), Format::V2);
    assert_eq!(
        sniff(b" {\n \"format\" : \"stalx\" , \"version\" : 2"),
        Format::V2
    );
    assert_eq!(
        sniff(b"\xef\xbb\xbf{\"format\":\"stalx\",\"version\":2"),
        Format::Unknown
    );
    assert_eq!(
        sniff(b"{\"version\":2,\"format\":\"stalx\"}"),
        Format::Unknown
    );
    assert_eq!(sniff(b"{\"format\":\"stalx\""), Format::Unknown);
    assert_eq!(sniff(b""), Format::Unknown);
    assert_eq!(sniff(b"PK\x03\x04"), Format::Unknown);
}

/// The header read does not depend on member order (`docs/file-format.md`
/// section 4): a file that puts its link and per-machine state after the
/// survey body probes with them, from bytes and from disk, exactly as the
/// full decode reads them; only the display-only listing read, which trusts
/// the canonical order, stops at the body (review finding C-file-format-11).
/// Injection: stopping `probe_bytes` at the first body member again reads the
/// moved file as unlinked, which the replace guard and the foreign-copy
/// probe would act on.
#[test]
fn the_probe_finds_header_members_after_the_body() {
    let (p, extras) = super::format_v2_doc_tests::example();
    let text = String::from_utf8(encode(&p, &extras, Mode::Normal).unwrap()).unwrap();
    let mut moved = text.clone();
    for key in ["\"speleodb\":{", "\"local\":{"] {
        let start = moved.find(&format!("\n{key}")).unwrap() + 1;
        let end = start + moved[start..].find('\n').unwrap() + 1;
        let member = moved[start..end].to_string();
        moved.replace_range(start..end, "");
        let at = moved.find("\n\"end\":true").unwrap() + 1;
        moved.insert_str(at, &member);
    }
    assert_ne!(moved, text);
    let decoded = super::decode(moved.as_bytes(), &super::DecodeOptions::default()).unwrap();
    assert!(decoded.extras.speleodb.is_some());

    let header = probe_bytes(moved.as_bytes());
    assert!(header.complete);
    assert_eq!(header.speleodb, decoded.extras.speleodb);
    assert_eq!(header.local.as_ref(), Some(&decoded.extras.local));
    assert_eq!(header.project.unwrap().name, p.name);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moved.stalx");
    std::fs::write(&path, &moved).unwrap();
    let from_disk = probe_path(&path).unwrap();
    assert_eq!(from_disk, probe_bytes(moved.as_bytes()));
    // The listing read stops at the body, by design.
    let listed = super::probe_path_listing(&path).unwrap();
    assert!(listed.complete && listed.speleodb.is_none() && listed.summary.is_some());

    // A canonical file with a link and local state is complete at the key
    // of its first body member.
    let canonical = probe_bytes(text.as_bytes());
    let key = "\n\"sections\":";
    let body = text.find(key).unwrap() + key.len();
    assert_eq!(probe_bytes(&text.as_bytes()[..body]), canonical);
}

/// Every link and local-state shape a file Stalagmight writes can hold, the
/// push mode's revision (a link, never local state) included, encoded around
/// `p`.
fn header_shapes(p: &crate::models::survey::Project) -> Vec<(&'static str, Vec<u8>)> {
    use super::{Extras, LocalState, LocalSync, PushOptions, SpeleoDbLink};
    let link = SpeleoDbLink {
        endpoint: "https://speleodb.example/".to_string(),
        project_id: "11111111-1111-1111-1111-111111111111".to_string(),
        project_name: "Synthetic".to_string(),
        undo_mode: None,
    };
    let display = LocalState {
        display: [("showGrid".to_string(), "true".to_string())].into(),
        ..LocalState::default()
    };
    let sync = LocalState {
        speleodb: LocalSync {
            base_commit: Some("abc1234".to_string()),
            unpushed_edits: true,
            ..LocalSync::default()
        },
        ..display.clone()
    };
    let shape = |speleodb: Option<&SpeleoDbLink>, local: &LocalState| Extras {
        speleodb: speleodb.cloned(),
        local: local.clone(),
        ..Extras::default()
    };
    let encoded = |extras: &Extras, mode: Mode| encode(p, extras, mode).unwrap();
    vec![
        (
            "unlinked",
            encoded(&shape(None, &LocalState::default()), Mode::Normal),
        ),
        (
            "unlinked, with display settings",
            encoded(&shape(None, &display), Mode::Normal),
        ),
        (
            "linked, without local state",
            encoded(&shape(Some(&link), &LocalState::default()), Mode::Normal),
        ),
        (
            "linked, with local state",
            encoded(&shape(Some(&link), &sync), Mode::Normal),
        ),
        (
            "a pushed revision",
            encoded(
                &shape(Some(&link), &sync),
                Mode::ForPush(PushOptions::default()),
            ),
        ),
        (
            "a detached copy",
            encoded(&shape(Some(&link), &sync), Mode::DetachedCopy),
        ),
    ]
}

/// Review finding O1 (package U5): a file Stalagmight wrote is read from its
/// first 64 KiB whatever its header holds. The early stop used to need all
/// six header members, so an unlinked file, or one without local state (a
/// downloaded revision is written without it), was read and parsed to its
/// end by every link check. The read is the same first chunk for a file many
/// times larger, and the header is the whole file's.
/// Injection: stop at the body only when every header member was seen
/// (`self.seen == ALL`, as before) and every shape but "linked, with local
/// state" reads the whole file.
#[test]
fn a_canonical_file_is_read_from_its_first_bytes_whatever_its_header_holds() {
    const FIRST: usize = 64 * 1024;
    let dir = tempfile::tempdir().unwrap();
    let p = super::format_v2_diff_tests::survey(8, 625, 7);
    for (shape, bytes) in header_shapes(&p) {
        assert!(bytes.len() > 8 * FIRST, "{shape}: {} bytes", bytes.len());
        let path = dir.path().join("cave.stalx");
        std::fs::write(&path, &bytes).unwrap();
        let (header, read) = super::probe::probe_path_counting(&path).unwrap();
        assert_eq!(read, FIRST, "{shape}: the first chunk only");
        assert!(header.complete, "{shape}");
        assert_eq!(
            header,
            probe_bytes(&bytes),
            "{shape}: the whole file's header"
        );
        let decoded = super::decode(&bytes, &super::DecodeOptions::default()).unwrap();
        assert_eq!(header.speleodb, decoded.extras.speleodb, "{shape}");
        assert_eq!(
            header.local.unwrap_or_default(),
            decoded.extras.local,
            "{shape}"
        );
        assert_eq!(header.project.unwrap().name, p.name, "{shape}");
    }
}

/// The timing check of O1: a link check runs for every Recents entry at
/// logout and for every cached copy, so the check of a large unlinked file
/// must cost what its header costs, not what its survey costs. 200 probes of
/// a 3.5 MB file take about 6 ms in a debug build with the canonical stop
/// (each reads 64 KiB); reading each to its end, as before, took 17 s.
#[test]
fn probing_a_large_unlinked_file_many_times_stays_fast() {
    let dir = tempfile::tempdir().unwrap();
    let p = super::format_v2_diff_tests::survey(24, 1250, 11);
    let bytes = encode(&p, &super::Extras::default(), Mode::Normal).unwrap();
    assert!(bytes.len() > 3 * 1024 * 1024, "{} bytes", bytes.len());
    let path = dir.path().join("large.stalx");
    std::fs::write(&path, &bytes).unwrap();
    let started = std::time::Instant::now();
    for _ in 0..200 {
        let header = probe_path(&path).unwrap();
        assert!(header.complete && header.speleodb.is_none());
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "200 probes of a {} byte file took {elapsed:?}",
        bytes.len()
    );
}

/// The slow path stays correct (`docs/file-format.md` section 4): a header
/// that is not in the canonical form is read to its end, so a member a
/// writer put after the body is still found. Three shapes: the link and
/// local state moved below the body of a canonically laid out file (the
/// summary names the link the header lacks); the link alone moved; and the
/// local state of an unlinked file moved below the body of a file written on
/// one line (a JSON library's compact output).
/// Injection: drop the summary check from `canonical_header_ends_at` and the
/// moved link reads as unlinked; drop the layout check and the one-line
/// file's local state is lost.
#[test]
fn a_header_out_of_canonical_form_is_still_read_to_its_end() {
    use super::{Extras, LocalState};
    let (p, extras) = super::format_v2_doc_tests::example();
    let move_below_body = |text: &str, keys: &[&str]| {
        let mut moved = text.to_string();
        for key in keys {
            let start = moved.find(&format!("\n\"{key}\":{{")).unwrap() + 1;
            let end = start + moved[start..].find('\n').unwrap() + 1;
            let member = moved[start..end].to_string();
            moved.replace_range(start..end, "");
            let at = moved.find("\n\"end\":true").unwrap() + 1;
            moved.insert_str(at, &member);
        }
        moved
    };
    let linked = String::from_utf8(encode(&p, &extras, Mode::Normal).unwrap()).unwrap();
    let unlinked_extras = Extras {
        speleodb: None,
        local: LocalState {
            display: [("showGrid".to_string(), "true".to_string())].into(),
            ..LocalState::default()
        },
        ..extras
    };
    let unlinked = String::from_utf8(encode(&p, &unlinked_extras, Mode::Normal).unwrap()).unwrap();
    let one_line = move_below_body(&unlinked, &["local"]).replace('\n', "");
    let dir = tempfile::tempdir().unwrap();
    for (shape, text) in [
        (
            "link and local state below the body",
            move_below_body(&linked, &["speleodb", "local"]),
        ),
        (
            "link below the body",
            move_below_body(&linked, &["speleodb"]),
        ),
        ("one line, local state below the body", one_line),
    ] {
        let decoded = super::decode(text.as_bytes(), &super::DecodeOptions::default());
        assert!(decoded.is_ok(), "{shape}: {:?}", decoded.err());
        let decoded = decoded.unwrap();
        let header = probe_bytes(text.as_bytes());
        assert!(header.complete, "{shape}");
        assert_eq!(header.speleodb, decoded.extras.speleodb, "{shape}");
        assert_eq!(
            header.local.as_ref(),
            Some(&decoded.extras.local),
            "{shape}"
        );
        let path = dir.path().join("moved.stalx");
        std::fs::write(&path, &text).unwrap();
        assert_eq!(probe_path(&path).unwrap(), header, "{shape}");
    }
}
