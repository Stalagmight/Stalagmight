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
