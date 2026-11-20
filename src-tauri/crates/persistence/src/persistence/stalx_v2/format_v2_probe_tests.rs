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
