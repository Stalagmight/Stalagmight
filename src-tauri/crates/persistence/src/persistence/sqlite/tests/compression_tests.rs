//! Gzip compression of history blobs and the decompression ceiling (#473).
//!
//! Lines 3979-4722 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

// ── Gzip compression unit tests ─────────────────────────────────────

#[test]
fn test_compress_decompress_round_trip() {
    let original = r#"{"type":"AddSection","data":{"name":"Main"}}"#;
    let compressed = compress_json(original);
    let decompressed = decompress_json(&compressed).unwrap();
    assert_eq!(decompressed, original);
}

#[test]
fn test_compress_produces_gzip_magic_bytes() {
    let compressed = compress_json("hello");
    assert!(
        compressed.len() >= 2,
        "compressed output should have at least 2 bytes"
    );
    assert_eq!(compressed[0], 0x1f, "first magic byte");
    assert_eq!(compressed[1], 0x8b, "second magic byte");
}

#[test]
fn test_decompress_plain_utf8_backward_compat() {
    // Old uncompressed TEXT entries should be returned as-is
    let plain = b"plain JSON string";
    let result = decompress_json(plain).unwrap();
    assert_eq!(result, "plain JSON string");
}

#[test]
fn test_decompress_empty_data() {
    let result = decompress_json(b"").unwrap();
    assert_eq!(result, "");
}

#[test]
fn test_compress_empty_string() {
    let compressed = compress_json("");
    let decompressed = decompress_json(&compressed).unwrap();
    assert_eq!(decompressed, "");
}

#[test]
fn test_compress_large_json() {
    // Large repetitive JSON compresses well
    let large = format!(
        r#"{{"data":[{}]}}"#,
        "1,".repeat(10_000).trim_end_matches(',')
    );
    let compressed = compress_json(&large);
    let decompressed = decompress_json(&compressed).unwrap();
    assert_eq!(decompressed, large);
    // Compression should be significantly smaller
    assert!(
        compressed.len() < large.len() / 2,
        "compressed ({}) should be much smaller than original ({})",
        compressed.len(),
        large.len()
    );
}

#[test]
fn test_compress_unicode() {
    let unicode = r#"{"name":"Grotte de l'Émeraude","探検":"洞窟"}"#;
    let compressed = compress_json(unicode);
    let decompressed = decompress_json(&compressed).unwrap();
    assert_eq!(decompressed, unicode);
}

#[test]
fn test_decompress_single_byte_not_gzip() {
    // Single byte can't be gzip - should return as lossy UTF-8
    let result = decompress_json(&[0x41]).unwrap();
    assert_eq!(result, "A");
}

#[test]
fn test_undo_stack_round_trips_through_compression() {
    // Verify the full pipeline: save → (compress) → load → (decompress) matches
    let db = Database::in_memory().unwrap();
    let json_with_special_chars = r##"{"color":"#ff0000","value":"hello\nworld"}"##;
    let entries = vec![(
        json_with_special_chars.to_string(),
        r#"{"undo":"data"}"#.to_string(),
    )];
    db.save_undo_stack(entries).unwrap();
    let loaded = db.load_undo_stack().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].1, json_with_special_chars);
    assert_eq!(loaded[0].2, r#"{"undo":"data"}"#);
}

#[test]
fn test_migrate_undo_redo_to_blob_on_fresh_db() {
    // Fresh in-memory DB already has BLOB columns (schema v3).
    // Migration should be idempotent and not error.
    let db = Database::in_memory().unwrap();
    db.migrate_undo_redo_to_blob().unwrap();
    // Save and load should still work after migration
    let entries = vec![("cmd".to_string(), "undo".to_string())];
    db.save_undo_stack(entries).unwrap();
    let loaded = db.load_undo_stack().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].1, "cmd");
}

// ── Decompression ceiling (#473) ────────────────────────────────────

/// The bomb shape itself: a tiny BLOB whose inflated size is enormous.
///
/// The assertion on the compressed length is the point of the test - it states the
/// amplification (8 MiB out of well under 64 KiB in), which is why a row-count bound
/// alone cannot contain this and an output ceiling is needed.
#[test]
fn a_gzip_bomb_is_rejected_by_the_ceiling() {
    let bomb = compress_json(&"a".repeat(8 * 1024 * 1024));
    assert!(
        bomb.len() < 64 * 1024,
        "8 MiB of one repeated byte should compress to well under 64 KiB, got {} bytes",
        bomb.len()
    );

    let err = decompress_json_capped(&bomb, 1024).unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("decompression ceiling"),
        "the bomb should be refused by the ceiling, got: {rendered}"
    );
}

/// Overwrite a gzip blob's ISIZE trailer so it CLAIMS `declared` bytes.
///
/// The stream is left otherwise intact and deliberately inconsistent with the claim -
/// which is the whole point, since the claim is attacker-controlled and the tests below
/// exist to pin down what is done with a lie in each direction.
fn claiming_inflated_size(mut blob: Vec<u8>, declared: u32) -> Vec<u8> {
    let at = blob.len() - 4;
    blob[at..].copy_from_slice(&declared.to_le_bytes());
    blob
}

/// [#473] A blob is refused on its DECLARED size, before a byte is inflated.
///
/// The property `extract_entry_capped` (speleodb/dump.rs) gets from the ZIP central
/// directory, and the one the first attempt at this fix lacked: it inflated all 256 MiB
/// before refusing, which is the exact cost the ceiling exists to avoid. Proven at the
/// REAL constant and for a few bytes of work, which is only possible because nothing
/// here decompresses.
#[test]
fn a_blob_that_declares_more_than_the_ceiling_is_refused_without_inflating() {
    let honest = compress_json("{}");
    let liar = claiming_inflated_size(honest, u32::MAX);
    assert!(
        liar.len() < 128,
        "the point is that a hostile CLAIM costs nothing to make, got {} bytes",
        liar.len()
    );

    let budget = InflationBudget::new(MAX_HISTORY_LOAD_BYTES);
    let err = decompress_json_under(&liar, &budget).unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("declares") && rendered.contains("decompression ceiling"),
        "the refusal must name the declared size, got: {rendered}"
    );
}

/// [#473] A legacy blob is priced at what it will ACTUALLY produce, not always 3x.
///
/// `from_utf8_lossy` returns valid input unchanged and only substitutes a 3-byte U+FFFD
/// per invalid subsequence, so the two cases differ by 3x - and the per-blob ceiling is
/// compared against this number. Answering `3 * len` unconditionally therefore refused a
/// perfectly valid 90 MB legacy row for a 270 MB output it would never produce: an
/// effective ceiling of 85 MiB where the constant says 256, discarding the whole stack of
/// a file this codebase itself wrote.
#[test]
fn a_legacy_blob_is_priced_by_its_real_utf8_expansion() {
    let valid = b"{\"type\":\"close_project\"}";
    assert_eq!(
        declared_inflated_size(valid),
        valid.len() as u64,
        "valid UTF-8 passes through from_utf8_lossy unchanged"
    );

    // Every 0xFF is its own invalid subsequence, so each becomes a 3-byte U+FFFD.
    let invalid = [0xFF_u8; 16];
    assert_eq!(
        declared_inflated_size(&invalid),
        48,
        "invalid input is still priced at the 3x worst case"
    );
    assert_eq!(
        String::from_utf8_lossy(&invalid).len(),
        48,
        "...and that worst case is exact, not a guess"
    );
}

/// [#473] THE invariant every bound rests on: a blob never produces more than it declared.
///
/// The pre-scan approves a window by summing `declared_inflated_size`, and the running
/// budget reserves the same number per row, so both are sound only while actual <=
/// declared for every possible input. Asserted over a spread of byte patterns rather than
/// the handful of shapes the other tests happen to use - lone invalid bytes, truncated
/// multi-byte prefixes (which collapse SEVERAL bytes into one U+FFFD, so they expand by
/// less than 3x), valid ASCII and valid multi-byte text.
#[test]
fn a_legacy_blob_never_produces_more_than_it_declares() {
    let cases: Vec<Vec<u8>> = vec![
        vec![],
        b"{}".to_vec(),
        b"plain ascii json".to_vec(),
        "\u{e9}\u{4e2d}\u{1f600}".repeat(8).into_bytes(), // valid, 2/3/4-byte chars
        vec![0xFF; 32],                                   // every byte its own bad subsequence
        vec![0x80; 32],                                   // stray continuation bytes
        b"\xE0\xA0".repeat(16),                           // truncated 3-byte prefixes
        b"\xF0\x9F\x98".repeat(16),                       // truncated 4-byte prefixes
        {
            let mut mixed = b"valid text ".to_vec();
            mixed.extend_from_slice(&[0xFF, 0xFE, 0x80]);
            mixed.extend_from_slice(b" more valid text");
            mixed
        },
    ];

    for data in cases {
        // Only the legacy branch is under test; a gzip-magic prefix would route elsewhere.
        assert!(!is_gzip(&data), "fixture must take the legacy branch");
        let declared = declared_inflated_size(&data);
        let produced = String::from_utf8_lossy(&data).len() as u64;
        assert!(
            produced <= declared,
            "declared {declared} but produced {produced} for {data:?}"
        );
    }
}

/// [#473] The per-blob ceiling binds the LEGACY branch too, not only gzip.
///
/// It used to sit below the `is_gzip` branch, which silently exempted legacy blobs from
/// the one bound that names a single row. Invalid bytes, so the 3x worst case is the real
/// output and the ceiling is what has to refuse it.
#[test]
fn a_legacy_blob_past_the_ceiling_is_refused_by_the_per_blob_bound() {
    // 3x this is just past 256 MiB, so it is the per-blob ceiling that must refuse it -
    // not the 1 GiB load budget, which it fits inside.
    let oversized = vec![0xFF_u8; (MAX_DECOMPRESSED_BLOB_BYTES as usize / 3) + 1024];
    assert!(!oversized.starts_with(&[0x1f, 0x8b]), "not a gzip blob");
    assert!(
        declared_inflated_size(&oversized) > MAX_DECOMPRESSED_BLOB_BYTES,
        "the 3x worst case must exceed the per-blob ceiling for this to test anything"
    );
    assert!(
        declared_inflated_size(&oversized) < MAX_HISTORY_LOAD_BYTES,
        "...and must fit the load budget, or the wrong bound would refuse it"
    );

    let budget = InflationBudget::new(MAX_HISTORY_LOAD_BYTES);
    let err = decompress_json_under(&oversized, &budget).unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("decompression ceiling"),
        "the per-blob ceiling must be what refuses a legacy blob, got: {rendered}"
    );
    assert_eq!(
        budget.remaining(),
        MAX_HISTORY_LOAD_BYTES,
        "a blob refused before reserving must not touch the budget"
    );

    // ...and the SAME length of valid text is admitted, because its real output is `len`.
    // This is the half the unconditional 3x got wrong.
    let valid_same_size = vec![b'x'; oversized.len()];
    assert!(
        declared_inflated_size(&valid_same_size) < MAX_DECOMPRESSED_BLOB_BYTES,
        "a valid legacy row of the same length must fit the ceiling it really uses"
    );
}

/// [#473] ...and a blob that UNDERSTATES its size is stopped by the backstop.
///
/// The declared size is attacker-controlled, so it is only ever trusted in the refusing
/// direction. A blob claiming 1 KiB and carrying 8 MiB is capped at what it claimed, so
/// lying buys nothing: the `take` trips one byte past the claim.
#[test]
fn a_blob_that_understates_its_size_is_stopped_at_what_it_claimed() {
    let bomb = compress_json(&"a".repeat(8 * 1024 * 1024));
    let liar = claiming_inflated_size(bomb, 1024);

    let budget = InflationBudget::new(MAX_HISTORY_LOAD_BYTES);
    let err = decompress_json_under(&liar, &budget).unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("decompression ceiling"),
        "an understated blob must still trip the read ceiling, got: {rendered}"
    );
}

/// [#473] A very large ceiling does not wrap `max_bytes + 1` into `take(0)`.
///
/// Debug builds panicked on the overflow; release builds have no `overflow-checks`, so
/// the wrap turned into an EMPTY string reported as SUCCESS - a caller asking for "no
/// practical limit" silently losing its payload, in the build where that is hardest to
/// notice.
#[test]
fn an_enormous_ceiling_returns_the_payload_rather_than_wrapping_to_empty() {
    let payload = "a".repeat(4096);
    let compressed = compress_json(&payload);

    assert_eq!(
        decompress_json_capped(&compressed, u64::MAX).unwrap(),
        payload
    );
}

/// The boundary is inclusive: `max_bytes` bytes out is fine, and comes back intact.
#[test]
fn a_payload_exactly_at_the_ceiling_still_decompresses() {
    let payload = "a".repeat(1024);
    let compressed = compress_json(&payload);

    let decompressed = decompress_json_capped(&compressed, 1024).unwrap();
    assert_eq!(decompressed.len(), 1024);
    assert_eq!(decompressed, payload);
}

/// And one byte past it is not - this is what `take(max_bytes + 1)` buys.
#[test]
fn one_byte_past_the_ceiling_is_rejected() {
    let payload = "a".repeat(1025);
    let compressed = compress_json(&payload);

    let err = decompress_json_capped(&compressed, 1024).unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("decompression ceiling"),
        "1025 bytes under a 1024-byte ceiling should name the ceiling, got: {rendered}"
    );
}

/// A size trip is reported as a size trip even when the cut lands mid-codepoint.
///
/// The payload is 2 bytes per character, so the `take` boundary at `max_bytes + 1`
/// = 1025 falls on the FIRST byte of a character. Reading through `read_to_string`
/// would surface that as invalid UTF-8 and blame the wrong thing; reading into a
/// `Vec<u8>` and checking the length first keeps the diagnosis honest.
#[test]
fn a_multibyte_payload_past_the_ceiling_reports_the_ceiling_not_utf8() {
    let payload = "é".repeat(600); // 1200 bytes, 2 per char
    assert_eq!(payload.len(), 1200, "two bytes per character");
    let compressed = compress_json(&payload);

    let err = decompress_json_capped(&compressed, 1024).unwrap_err();
    let rendered = format!("{err:#} | {err:?}");
    assert!(
        rendered.contains("decompression ceiling"),
        "a mid-codepoint cut is still a size trip, got: {rendered}"
    );
    assert!(
        !rendered.contains("UTF-8"),
        "and must not be misreported as an encoding problem, got: {rendered}"
    );
}

/// The ceiling does not get in the way of a realistically large history blob.
#[test]
fn ordinary_history_blobs_still_round_trip_under_the_default_ceiling() {
    let entry = r#"{"type":"MoveStation","section":3,"station":17,"from":[12.5,-3.25,44.0],"to":[12.75,-3.5,44.25]},"#;
    let mut json = String::from(r#"{"entries":["#);
    while json.len() < 2 * 1024 * 1024 {
        json.push_str(entry);
    }
    json.push_str("{}]}");

    let compressed = compress_json(&json);
    let decompressed = decompress_json(&compressed).unwrap();
    assert_eq!(decompressed, json);

    // The constant's own rationale: >10x the ~17 MB a 20,000-shot ProjectSnapshotPair
    // measures at (the table on UNDO_BYTE_BUDGET_BYTES). Shrinking it below that margin
    // invalidates the justification, so it has to fail here rather than silently start
    // refusing legitimate surveys. (The measurement is a binding rather than a literal
    // so the comparison is not const-folded into an `assertions_on_constants` warning.)
    let largest_measured_legitimate_blob: u64 = 17 * 1024 * 1024;
    assert!(
        MAX_DECOMPRESSED_BLOB_BYTES >= largest_measured_legitimate_blob * 10,
        "the ceiling must stay >10x the largest measured legitimate payload"
    );
}

/// End to end: a bomb row in `undo_stack` fails the load at the REAL constant.
///
/// The only test that builds a genuine 256 MiB payload, and what that buys is one thing
/// precisely: that the constant which SHIPS is the constant that binds, on a blob nobody
/// forged. Be exact about which bound catches it, because the answer changed - the row's
/// honest ISIZE declares ~257 MiB, so `decompress_json_under` refuses it on the DECLARED
/// size and `decompress_json_capped` is never reached. It does not exercise the inflate
/// path; `a_blob_that_understates_its_size_is_stopped_at_what_it_claimed` does that.
///
/// Cost is dominated by BUILDING the payload, not by loading it (nothing inflates):
/// ~86 MB peak RSS and ~0.8 s, measured with `/usr/bin/time -v` on the test binary, not
/// on `cargo test`, which measures the compiler. Cheap enough to run unguarded next to
/// the other 3,374 tests.
#[test]
fn a_bomb_row_fails_the_history_load_instead_of_inflating() {
    let db = Database::in_memory().unwrap();

    // Stream the bomb rather than materialising 256 MiB of input: one 1 MiB chunk of a
    // repeated byte, written until the encoder has consumed more than the ceiling.
    let chunk = vec![b'a'; 1024 * 1024];
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let mut written: u64 = 0;
    while written <= MAX_DECOMPRESSED_BLOB_BYTES {
        encoder.write_all(&chunk).unwrap();
        written += chunk.len() as u64;
    }
    let bomb = encoder.finish().unwrap();
    assert!(
        written > MAX_DECOMPRESSED_BLOB_BYTES,
        "the row must inflate past the ceiling to be a bomb"
    );

    let tiny = compress_json(r#"{"undo":"data"}"#);
    db.conn
        .execute(
            "INSERT INTO undo_stack (sequence, command_json, undo_data_json, executed_at) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![0_i64, bomb, tiny, Utc::now().to_rfc3339()],
        )
        .unwrap();

    let err = db.load_undo_stack_with_timestamps_capped(10).unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("decompression ceiling"),
        "the timestamped loader should fail naming the ceiling, got: {rendered}"
    );

    let legacy_err = db.load_undo_stack_capped(10).unwrap_err();
    let legacy_rendered = format!("{legacy_err:#}");
    assert!(
        legacy_rendered.contains("decompression ceiling"),
        "and so should the legacy loader, got: {legacy_rendered}"
    );
}

/// [#473] THE regression: many rows, each individually legal, refused in AGGREGATE.
///
/// This is the shape the per-blob ceiling alone did not stop, and it is not theoretical.
/// Measured on the first version of this fix: six rows whose blobs each declared 250 MiB
/// - every one comfortably inside the 256 MiB per-blob ceiling - came out of a 14 MB
/// file as 3.1 GB of `String` in memory, and the load returned `Ok`. At the default
/// 100-row count window the same file shape reaches ~50 GiB, which is the OOM #473 was
/// filed about.
///
/// Runs at the REAL constants and costs nothing, because the rows are refused on their
/// DECLARED sizes before anything inflates - the forged trailer is what makes a
/// 51-GiB-shaped file expressible in a few hundred bytes.
#[test]
fn a_history_table_of_individually_legal_rows_is_refused_in_aggregate() {
    let db = Database::in_memory().unwrap();

    // 250 MiB per blob: under the per-blob ceiling, so ONLY the aggregate bound can
    // catch it. Six rows x two blobs = 3 GiB claimed, past the 1 GiB load budget.
    const ROWS: i64 = 6;
    let quarter_gig = claiming_inflated_size(compress_json("{}"), 250 * 1024 * 1024);
    assert!(
        declared_inflated_size(&quarter_gig) < MAX_DECOMPRESSED_BLOB_BYTES,
        "each row must be INSIDE the per-blob ceiling, or this tests the wrong bound"
    );
    for seq in 0..ROWS {
        db.conn
            .execute(
                "INSERT INTO undo_stack (sequence, command_json, undo_data_json, executed_at) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    seq,
                    quarter_gig.clone(),
                    quarter_gig.clone(),
                    Utc::now().to_rfc3339()
                ],
            )
            .unwrap();
    }

    let err = db.load_undo_stack_with_timestamps_capped(100).unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("load budget"),
        "the aggregate bound must be what refuses it, got: {rendered}"
    );

    // The legacy loader answers to the same bound - it shares the helper precisely so
    // that this assertion cannot start passing for one loader and not the other.
    let legacy = format!("{:#}", db.load_undo_stack_capped(100).unwrap_err());
    assert!(
        legacy.contains("load budget"),
        "the legacy loader is bounded in aggregate too, got: {legacy}"
    );
}

/// [#473] The window pre-scan is spent ACROSS rows, not reset per row.
///
/// Named for the pre-scan on purpose. An earlier version of this test claimed to
/// exercise the running `InflationBudget` and did not: the rows declare exactly what they
/// carry, so the pre-scan refused first, and BOTH errors said "load budget" - which meant
/// the assertion could not tell them apart and the whole budget could have been stubbed
/// out with `Ok(())` while this still passed. The two messages no longer share a phrase,
/// and this one asserts on the pre-scan's.
#[test]
fn the_window_prescan_is_spent_across_rows_not_reset_per_row() {
    use super::undo::decompress_history_rows_within;

    let row = |seq: i64| {
        (
            seq,
            compress_json(&"a".repeat(1024)),
            compress_json(&"b".repeat(1024)),
            None,
        )
    };
    let rows: Vec<_> = (0..4).map(row).collect();

    // 8 KiB of payload across 4 rows. A budget of 8 KiB fits exactly...
    let ok = decompress_history_rows_within(rows.clone(), "undo", &InflationBudget::new(8 * 1024))
        .unwrap();
    assert_eq!(ok.len(), 4);
    assert_eq!(ok[0].1.len(), 1024);

    // ...and one byte less does not, which is only true if the window is priced as a
    // WHOLE. A per-row check would see 2 KiB against 8 KiB four times over and pass.
    let err =
        decompress_history_rows_within(rows.clone(), "undo", &InflationBudget::new(8 * 1024 - 1))
            .unwrap_err();
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("declares") && rendered.contains("across 4 rows"),
        "the pre-scan must be what refuses it, got: {rendered}"
    );

    // A budget that fits one row proves it directly: the bound is not per row.
    assert!(decompress_history_rows_within(rows, "undo", &InflationBudget::new(2 * 1024)).is_err());
}

/// [#473] A budget already partly spent binds the NEXT stack of the same open.
///
/// This is the property that makes the ceiling per-open rather than per-stack. Giving
/// each stack a fresh budget - which is what the first version of this fix did - bounds a
/// project open at 2x the constant while every doc says 1x.
#[test]
fn a_second_stack_only_gets_what_the_first_one_left() {
    use super::undo::decompress_history_rows_within;

    let rows = |tag: char| {
        vec![(
            0_i64,
            compress_json(&tag.to_string().repeat(1024)),
            compress_json(&tag.to_string().repeat(1024)),
            None,
        )]
    };
    let budget = InflationBudget::new(3 * 1024);

    // The undo stack takes 2 KiB of the 3 KiB.
    assert_eq!(
        decompress_history_rows_within(rows('a'), "undo", &budget)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(budget.remaining(), 1024);

    // The redo stack needs 2 KiB and only 1 KiB is left, so the open is refused. With a
    // per-stack budget this would succeed and the pair would hold 4 KiB under a 3 KiB cap.
    let err = decompress_history_rows_within(rows('b'), "redo", &budget).unwrap_err();
    assert!(
        format!("{err:#}").contains("redo history declares"),
        "the second stack must see the first one's spending, got: {err:#}"
    );
}

/// [#473] `InflationBudget`'s own arithmetic, tested directly.
///
/// It needs its own test because on the load path it is unreachable by construction: the
/// pre-scan establishes `sum(declared) <= remaining` before any row reserves its own
/// `declared`, so `reserve` cannot fail there. That makes it defense in depth - which is
/// worth keeping and worth testing, but only a direct test can do it.
#[test]
fn the_inflation_budget_reserves_refunds_and_refuses() {
    let budget = InflationBudget::new(1000);

    budget.reserve(400).unwrap();
    assert_eq!(budget.remaining(), 600);

    // A refund puts the unused part back for the rows behind it.
    budget.refund(150);
    assert_eq!(budget.remaining(), 750);

    // Reserving more than is left fails and takes NOTHING - a partial deduction would
    // shrink the budget on every refusal.
    let err = budget.reserve(751).unwrap_err();
    assert!(
        format!("{err:#}").contains("while inflating"),
        "the budget's own message must be distinguishable from the pre-scan's, got: {err:#}"
    );
    assert_eq!(budget.remaining(), 750, "a failed reserve must not deduct");

    // The boundary is inclusive.
    budget.reserve(750).unwrap();
    assert_eq!(budget.remaining(), 0);
    assert!(budget.reserve(1).is_err());
}

/// [#473] The legacy plain-UTF-8 branch reserves its worst case, then refunds.
///
/// Two properties in one, because they only hold together. It RESERVES BEFORE it
/// allocates (`from_utf8_lossy(..).into_owned()` runs after the reserve, not before) -
/// charging afterwards is the ordering the budget exists to forbid, and with several rows
/// inflating at once is how a bound gets multiplied. And it REFUNDS, which is what makes
/// reserving the 3x worst case affordable: without the refund a 1 KiB legacy row would
/// cost 3 KiB forever and two of them would not fit in 4 KiB.
#[test]
fn the_legacy_branch_reserves_its_worst_case_then_refunds_the_difference() {
    // A MIXED blob - mostly valid text with a few invalid bytes - because that is the
    // only shape where the reservation and the output differ, and so the only shape that
    // can prove the refund happens at all. All-valid is priced exactly (no difference to
    // refund) and all-invalid produces exactly its 3x worst case (likewise); a version of
    // this test using those left `declared == produced` in both cases, so deleting the
    // refund entirely kept it green. It is also the realistic corrupt-legacy-row shape.
    let mut mixed = vec![b'x'; 1020];
    mixed.extend_from_slice(&[0xFF; 4]);
    assert_eq!(mixed.len(), 1024);
    // Reserved: 3 * 1024 = 3072 (the worst case, since the blob is not valid UTF-8).
    // Produced: 1020 valid bytes + 4 x 3-byte U+FFFD = 1032. Refund: 2040.
    assert_eq!(declared_inflated_size(&mixed), 3 * 1024);
    assert_eq!(String::from_utf8_lossy(&mixed).len(), 1032);

    // 5 KiB, sized so the refund is the ONLY thing that lets row 2 in: with it 4088
    // remains (>= the 3072 reservation), without it 2048 (short by 1024).
    let budget = InflationBudget::new(5 * 1024);
    assert_eq!(
        decompress_json_under(&mixed, &budget).unwrap().len(),
        1032,
        "the produced string is the lossy expansion, not the reservation"
    );
    assert_eq!(
        budget.remaining(),
        5 * 1024 - 1032,
        "the unused 2040 bytes of the worst-case reservation must come back"
    );

    // THE discriminating step: a second row needs another 3072-byte reservation. It fits
    // only because the first row refunded - without the refund just 1024 would remain and
    // this would be refused, which is what makes the assertion above load-bearing rather
    // than decorative.
    assert!(
        decompress_json_under(&mixed, &budget).is_ok(),
        "the second row fits only because the first refunded its unused reservation"
    );
    assert_eq!(budget.remaining(), 5 * 1024 - 2 * 1032);

    // ...and it is still genuinely charged: a third row's 3072-byte reservation exceeds
    // what is left, so the PEAK reservation binds, and a refusal deducts nothing.
    let left = budget.remaining();
    let err = decompress_json_under(&mixed, &budget).unwrap_err();
    assert!(
        format!("{err:#}").contains("while inflating"),
        "the legacy branch must spend the shared budget, got: {err:#}"
    );
    assert_eq!(budget.remaining(), left, "a refusal must not deduct");
}

/// [#473] A blob that fails to inflate hands its reservation back.
///
/// Harmless while every caller aborts the whole load on the first error - but "skip the
/// bad row and keep the rest" is a live proposal for this loader, and under it a leaked
/// reservation would drain the budget permanently and refuse honest rows behind a corrupt
/// one.
#[test]
fn a_failed_inflate_does_not_leak_its_reservation() {
    let budget = InflationBudget::new(64 * 1024);
    let before = budget.remaining();

    // Truncated mid-stream: the header and trailer survive, the deflate body does not.
    let mut corrupt = compress_json(&"a".repeat(4096));
    let trailer = corrupt.split_off(corrupt.len() - 8);
    corrupt.truncate(corrupt.len() / 2);
    corrupt.extend_from_slice(&trailer);

    assert!(decompress_json_under(&corrupt, &budget).is_err());
    assert_eq!(
        budget.remaining(),
        before,
        "a failed inflate must refund what it reserved"
    );
}

/// [#473] ...and a stack that fails PART WAY refunds every row that already decoded.
///
/// The single-blob case above was never the whole story: a load decodes many rows, and
/// one bad row late in the stack propagates out with the earlier rows' bytes still
/// charged. Those `String`s are dropped - the caller substitutes `Vec::new()` - so the
/// charge is a lie, and an expensive one, because the budget is SHARED with the other
/// stack of the same open: a clean redo stack would be refused for memory that is
/// already free. Enough rows to span several decompression batches, so both the
/// whole-earlier-batch and partial-failing-batch cases are covered.
#[test]
fn a_stack_that_fails_part_way_refunds_the_rows_that_already_decoded() {
    use super::undo::decompress_history_rows_within;

    let budget = InflationBudget::new(1024 * 1024);
    let before = budget.remaining();

    let good = |seq: i64| {
        (
            seq,
            compress_json(&"a".repeat(4096)),
            compress_json(&"b".repeat(4096)),
            None,
        )
    };
    // Truncated mid-stream, but its trailer still declares a small size - so it passes
    // the window pre-scan and fails during inflation, which is the only shape that
    // reaches the partial-decode path at all.
    let mut corrupt = compress_json(&"c".repeat(4096));
    let trailer = corrupt.split_off(corrupt.len() - 8);
    corrupt.truncate(corrupt.len() / 2);
    corrupt.extend_from_slice(&trailer);

    let mut rows: Vec<_> = (0..8).map(good).collect();
    rows.push((8, corrupt.clone(), corrupt, None));

    let err = decompress_history_rows_within(rows, "undo", &budget).unwrap_err();
    assert!(
        !format!("{err:#}").contains("load budget"),
        "the failure must come from the corrupt row, not from running out of budget"
    );
    assert_eq!(
        budget.remaining(),
        before,
        "a stack that fails charges nothing - the rows it did decode are dropped"
    );
}
