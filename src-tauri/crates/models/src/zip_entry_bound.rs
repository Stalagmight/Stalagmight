//! Bound a ZIP archive's entry count from its raw bytes, BEFORE the `zip`
//! crate parses its central directory (#576 RV19a-4, RV19a-4b, W52-4, RV20-4).
//!
//! `zip::ZipArchive::new` builds a record for every entry the directory
//! declares before it returns, and the crate exposes no hook to bound that: a
//! count checked on `archive.len()` afterwards caps what follows the parse,
//! not the parse (575 MB and 4 s for a million entries). `archive.len()`
//! counts unique names, too, so a million records all named alike were parsed
//! and then let through. The `.tml` reader and the SpeleoDB dump reader both
//! parse through [`parse_bounded`], which bounds the count first.
//!
//! What the reader parses is set by the end record it goes by, and it goes by
//! the LAST one in the file - it searches the whole file backwards for end
//! records - but whenever that one fails it falls back on an earlier one (zip
//! 2.4.2 spec.rs:605-820, read.rs:655-680), so a broken last record could hide
//! any count behind it. That fallback is what [`parse_bounded`] takes away: it
//! hands the reader a view of the file ([`LastEndRecordOnly`]) in which every
//! other end-record signature reads as `PK\x05\x00`, so the last end record is
//! the only one the reader can find. It parses by that one or fails. (Counting
//! every central-directory signature in the file bounded the fallback too, but
//! it also counted the records of an archive STORED inside this one: a dump of
//! three entries holding a ZIP of 10 001 was refused as 10 005 records, RV20-4.)
//! The view is lifted as soon as the parse returns, so an entry's data - a
//! stored archive's own end record included - reads as it is.
//!
//! The one record left bounds the parse by two checks. The count it declares
//! (of the two it holds, the larger) or, when it defers to ZIP64 (the reader's
//! test: its count or directory offset saturated, and a locator right before
//! it, spec.rs:621-699), the count of the ZIP64 end record its locator points
//! at. And because the reader does not stop at the record the locator points
//! at - it searches forward from there for one whose size field ends it
//! exactly where the locator starts (spec.rs:752, 762-790) - and reserves room
//! for every entry that one declares before it reads a directory record
//! (read.rs:692-706, about 200 bytes an entry: 11 million entries reserve 2 GB,
//! RV19a-4b), every ZIP64 end record ending exactly at that locator that
//! declares more than the limit is refused too. The exact end is what keeps a
//! ZIP64 signature that turns up by chance in compressed data from refusing a
//! real archive: its random counts are over the limit, but its size field
//! would have to reach the locator to the byte.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Why an archive is over the entry limit, for the caller's own refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryExcess {
    /// The archive's last end record, or a ZIP64 end record the ZIP reader
    /// could take with it, declares this many entries.
    Declared(u64),
    /// The file holds more end-record signatures besides its last one than
    /// the entry limit. [`parse_bounded`] keeps where each one is, to hide it
    /// from the ZIP reader, and caps that list at the limit: an archive within
    /// the limit holds at most that many archives stored at its top level, one
    /// end record each (a signature turns up by chance only about once in
    /// 4 GiB), so more is a crafted archive, refused outright so the list stays
    /// bounded (8 bytes a position: 80 KB at 10 000).
    EndRecords,
}

/// Parse the archive `reader` holds with `parse` (the ZIP reader's constructor,
/// `zip::ZipArchive::new`) unless it is over `limit` entries, and bound what
/// that parse can do (see the module docs): `parse` is handed `reader` at its
/// start, wrapped so the last end record is the only one it can find, and the
/// wrapper reads the file as it is once `parse` has returned. Reads the file
/// once from its start to the end it has when this is called, plus its last
/// 64 KiB, before `parse` runs.
///
/// `Err(excess)` when the archive is over the limit; `parse` has not run. A
/// missing or garbled end record is the ZIP reader's to refuse.
///
/// `parse` must not read an entry's data: until it returns, every end-record
/// signature but the last reads with a zero for its fourth byte, so an entry
/// that holds one (a stored archive's own end record) would read altered.
/// Read entries through what `parse` returns, once it has returned.
///
/// # Errors
///
/// A read or seek on `reader` failed.
pub fn parse_bounded<R: Read + Seek, T>(
    mut reader: R,
    limit: u64,
    parse: impl FnOnce(LastEndRecordOnly<R>) -> T,
) -> io::Result<Result<T, EntryExcess>> {
    let len = reader.seek(SeekFrom::End(0))?;
    let last = last_end_record(&mut reader, len)?;
    let scan = scan_end_records(&mut reader, len, limit, last.as_ref())?;
    if let Some(declared) = last
        .and_then(|record| record.declared)
        .filter(|&count| count > limit)
        .or(scan.zip64_declared_over)
    {
        return Ok(Err(EntryExcess::Declared(declared)));
    }
    if scan.overflow {
        return Ok(Err(EntryExcess::EndRecords));
    }
    reader.rewind()?;
    let lifted = Arc::new(AtomicBool::new(false));
    let parsed = parse(LastEndRecordOnly {
        inner: reader,
        at: 0,
        hidden: scan.others,
        lifted: Arc::clone(&lifted),
    });
    lifted.store(true, Ordering::SeqCst);
    Ok(Ok(parsed))
}

/// The reader [`parse_bounded`] hands the ZIP reader: `R` as it is, except that
/// until that parse returns, every end-record signature but the archive's last
/// reads as `PK\x05\x00` - a pure function of where a byte is, so every read of
/// it agrees, a signature split across two reads included. A changed fourth
/// byte cannot make another signature: each window it falls in starts with
/// `K`, `\x05` or `\0`, never `P`.
#[derive(Debug)]
pub struct LastEndRecordOnly<R> {
    inner: R,
    /// Where `inner` is.
    at: u64,
    /// Where each hidden signature starts, in file order.
    hidden: Vec<u64>,
    /// Set once the parse has returned: every byte reads as it is.
    lifted: Arc<AtomicBool>,
}

impl<R: Read> Read for LastEndRecordOnly<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        let start = self.at;
        let end = start.saturating_add(read as u64);
        self.at = end;
        if !self.lifted.load(Ordering::SeqCst) {
            // The byte each hidden signature ends with, where this read returned it.
            let first = self
                .hidden
                .partition_point(|&signature| signature.saturating_add(3) < start);
            for &signature in &self.hidden[first..] {
                let last_byte = signature.saturating_add(3);
                if last_byte >= end {
                    break;
                }
                if let Some(byte) = usize::try_from(last_byte - start)
                    .ok()
                    .and_then(|at| buf.get_mut(at))
                {
                    *byte = 0;
                }
            }
        }
        Ok(read)
    }
}

impl<R: Seek> Seek for LastEndRecordOnly<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.at = self.inner.seek(pos)?;
        Ok(self.at)
    }
}

/// The archive's last end record: where it starts, and the entry count the ZIP
/// reader would take from it, if one can be read (see [`last_end_record`]).
#[derive(Debug, Clone, Copy)]
struct LastEndRecord {
    at: u64,
    /// Where the ZIP64 locator right before it starts, when one is there.
    locator_at: Option<u64>,
    declared: Option<u64>,
}

/// What [`scan_end_records`] found.
#[derive(Debug, Default)]
struct EndRecordScan {
    /// Where every end-record signature but the last record's starts.
    others: Vec<u64>,
    /// More of them than the entry limit ([`EntryExcess::EndRecords`]).
    overflow: bool,
    /// The largest count over the limit that a ZIP64 end record ending exactly
    /// at the last record's locator declares, if any.
    zip64_declared_over: Option<u64>,
}

/// Read `reader` once from its start to `len`, where [`parse_bounded`]
/// measured its end, in 64 KiB chunks: find where every
/// end-record signature but `last`'s starts, and every ZIP64 end record
/// declaring more than `limit` entries that ends exactly at `last`'s locator -
/// the shape the ZIP reader takes (see the module docs).
fn scan_end_records<R: Read + Seek>(
    reader: &mut R,
    len: u64,
    limit: u64,
    last: Option<&LastEndRecord>,
) -> io::Result<EndRecordScan> {
    const CHUNK: usize = 64 * 1024;
    // The longest field a signature is read with: a ZIP64 end record's counts
    // end 40 bytes in. The bytes past the last offset whose whole window is in
    // the buffer are carried into the next chunk, so every offset is looked at
    // once, with its window whole wherever the file holds it.
    const WINDOW: usize = 40;
    let last_at = last.map(|record| record.at);
    let locator_at = last.and_then(|record| record.locator_at);
    reader.rewind()?;
    // Only as far as the file was measured, as the end-record search reads it.
    let mut reader = reader.take(len);
    let mut scan = EndRecordScan::default();
    let mut buffer = vec![0u8; WINDOW + CHUNK];
    let mut carried = 0;
    let mut base = 0u64;
    loop {
        let read = match reader.read(&mut buffer[carried..]) {
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let filled = carried + read;
        let at_end = read == 0;
        let looked_at = if at_end {
            filled
        } else {
            filled.saturating_sub(WINDOW - 1)
        };
        for at in 0..looked_at {
            let window = &buffer[at..filled.min(at + WINDOW)];
            if !window.starts_with(b"PK") {
                continue;
            }
            let offset = base + at as u64;
            if window.starts_with(&END_SIGNATURE) {
                if Some(offset) != last_at {
                    if scan.others.len() as u64 >= limit {
                        scan.overflow = true;
                        return Ok(scan);
                    }
                    scan.others.push(offset);
                }
            } else if window.starts_with(&ZIP64_END_SIGNATURE) && window.len() >= WINDOW {
                let declared = u64_at(window, 24).max(u64_at(window, 32));
                let ends_at = offset
                    .checked_add(12)
                    .and_then(|end| end.checked_add(u64_at(window, 4)));
                if declared > limit && ends_at.is_some() && ends_at == locator_at {
                    scan.zip64_declared_over = Some(
                        scan.zip64_declared_over
                            .map_or(declared, |over| over.max(declared)),
                    );
                }
            }
        }
        if at_end {
            break;
        }
        buffer.copy_within(looked_at..filled, 0);
        carried = filled - looked_at;
        base += looked_at as u64;
    }
    Ok(scan)
}

/// `PK\x05\x06`, the signature of the 22-byte end-of-central-directory record.
const END_SIGNATURE: [u8; 4] = *b"PK\x05\x06";
/// The end record's fixed part; a comment of up to 65 535 bytes follows it.
const END_LEN: usize = 22;
/// `PK\x06\x07`, the signature of the 20-byte ZIP64 locator that sits right
/// before the end record and points at the ZIP64 end record.
const ZIP64_LOCATOR_SIGNATURE: [u8; 4] = *b"PK\x06\x07";
const ZIP64_LOCATOR_LEN: u64 = 20;
/// `PK\x06\x06`, the signature of the ZIP64 end record (56 bytes before its
/// extensible data), which carries the 64-bit entry counts.
const ZIP64_END_SIGNATURE: [u8; 4] = *b"PK\x06\x06";
const ZIP64_END_LEN: usize = 56;

/// The little-endian `u64` at `offset` in `bytes`.
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    let mut field = [0; 8];
    field.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(field)
}

/// The archive's last end record - the one the ZIP reader tries first, found by
/// searching back from the end over the longest comment it may carry - or
/// `None` when there is none: a missing record is the ZIP reader's to refuse.
///
/// The count is the one the reader parses that many directory records by
/// (read.rs:500-530, 698). When the record defers to ZIP64 - its count of all
/// entries or its directory offset saturated, with a locator right before it
/// (spec.rs:370, 637-660) - that is the ZIP64 end record's, read where the
/// locator points (the larger of its two counts; `None` when there is no record
/// there: the forward search is [`scan_end_records`]'s to cover). Otherwise it
/// is the larger of the record's two 16-bit counts, taken as they are: the
/// reader parses `0xFFFF` records when the count on this disk says so.
fn last_end_record<R: Read + Seek>(reader: &mut R, len: u64) -> io::Result<Option<LastEndRecord>> {
    let tail_len = len.min((END_LEN + usize::from(u16::MAX)) as u64);
    let tail_at = len - tail_len;
    let tail = read_at(reader, tail_at, tail_len as usize)?;
    let Some(at) = tail
        .windows(END_LEN)
        .rposition(|record| record.starts_with(&END_SIGNATURE))
    else {
        return Ok(None);
    };
    let record = &tail[at..at + END_LEN];
    let at = tail_at + at as u64;
    let u16_at = |offset: usize| u16::from_le_bytes([record[offset], record[offset + 1]]);
    let u32_at = |offset: usize| {
        u32::from_le_bytes([
            record[offset],
            record[offset + 1],
            record[offset + 2],
            record[offset + 3],
        ])
    };
    let locator_at = match at.checked_sub(ZIP64_LOCATOR_LEN) {
        Some(locator_at) => read_at(reader, locator_at, ZIP64_LOCATOR_SIGNATURE.len())?
            .starts_with(&ZIP64_LOCATOR_SIGNATURE)
            .then_some(locator_at),
        None => None,
    };
    let (on_this_disk, in_all) = (u16_at(8), u16_at(10));
    let defers_to_zip64 = in_all == u16::MAX || u32_at(16) == u32::MAX;
    let declared = match locator_at {
        Some(locator_at) if defers_to_zip64 => zip64_entry_count(reader, locator_at)?,
        _ => Some(u64::from(on_this_disk.max(in_all))),
    };
    Ok(Some(LastEndRecord {
        at,
        locator_at,
        declared,
    }))
}

/// The larger entry count of the ZIP64 end record that the locator at
/// `locator_at` points at, or `None` when no ZIP64 end record is there.
fn zip64_entry_count<R: Read + Seek>(reader: &mut R, locator_at: u64) -> io::Result<Option<u64>> {
    let locator = read_at(reader, locator_at, ZIP64_LOCATOR_LEN as usize)?;
    let record_at = u64_at(&locator, 8);
    // The record must end before the locator starts.
    if record_at
        .checked_add(ZIP64_END_LEN as u64)
        .is_none_or(|record_end| record_end > locator_at)
    {
        return Ok(None);
    }
    let record = read_at(reader, record_at, ZIP64_END_LEN)?;
    if !record.starts_with(&ZIP64_END_SIGNATURE) {
        return Ok(None);
    }
    Ok(Some(u64_at(&record, 24).max(u64_at(&record, 32))))
}

/// `len` bytes of `reader` from `offset`.
fn read_at<R: Read + Seek>(reader: &mut R, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    reader.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "test module: a failed setup should panic, and the printed outcome is the \
              evidence a run captures"
)]
mod tests {
    use std::io::{Cursor, Read as _, Seek as _, SeekFrom};

    use super::{parse_bounded, EntryExcess};

    /// A 22-byte end record without a comment, declaring `entries`.
    fn end_record(entries: u16) -> Vec<u8> {
        let mut record = b"PK\x05\x06".to_vec();
        record.extend_from_slice(&[0; 4]);
        record.extend_from_slice(&entries.to_le_bytes());
        record.extend_from_slice(&entries.to_le_bytes());
        record.extend_from_slice(&[0; 10]);
        record
    }

    /// The check works on any seekable reader, an in-memory dump included, and
    /// hands it to the parse at its start; an archive over the limit is refused
    /// without the parse running.
    #[test]
    fn the_parse_gets_the_reader_at_its_start() {
        for (entries, expected) in [(0, Ok(0)), (6, Err(EntryExcess::Declared(6)))] {
            let mut reader = Cursor::new(end_record(entries));
            reader.seek(SeekFrom::End(0)).unwrap();
            let outcome =
                parse_bounded(reader, 5, |mut view| view.stream_position().unwrap()).unwrap();
            assert_eq!(outcome, expected, "an end record declaring {entries}");
        }
    }

    /// The last end record's count of 0xFFFF on this disk is taken as it is: the
    /// ZIP reader parses that many directory records when no ZIP64 locator
    /// stands before it.
    #[test]
    fn a_saturated_count_with_no_zip64_locator_is_taken_as_it_is() {
        let mut record = end_record(1);
        record[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
        let outcome = parse_bounded(Cursor::new(record), 10_000, |_| ()).unwrap();
        assert_eq!(outcome, Err(EntryExcess::Declared(65_535)));
    }

    /// Until the parse returns, every end-record signature but the last reads
    /// as `PK\x05\x00`, through reads of any size (here 7 bytes, so a signature
    /// straddles two of them) and after a seek; once it has returned, the file
    /// reads as it is.
    #[test]
    fn only_the_last_end_record_is_seen_until_the_parse_returns() {
        let mut file = vec![b'.'; 40];
        file[3..7].copy_from_slice(b"PK\x05\x06");
        file[12..16].copy_from_slice(b"PK\x05\x06");
        file.extend(end_record(1));
        let last = file.len() - 22;

        let (seen, mut view) = parse_bounded(Cursor::new(file.clone()), 5, |mut view| {
            let mut seen = Vec::new();
            let mut chunk = [0u8; 7];
            loop {
                let read = view.read(&mut chunk).unwrap();
                if read == 0 {
                    break;
                }
                seen.extend_from_slice(&chunk[..read]);
            }
            view.seek(SeekFrom::Start(12)).unwrap();
            let mut again = [0u8; 4];
            view.read_exact(&mut again).unwrap();
            assert_eq!(&again, b"PK\x05\x00", "after a seek");
            (seen, view)
        })
        .unwrap()
        .unwrap();
        let mut hidden = file.clone();
        hidden[6] = 0;
        hidden[15] = 0;
        assert_eq!(seen, hidden, "the parse sees only the last end record");
        assert!(seen[last..].starts_with(b"PK\x05\x06"), "the last is whole");

        view.rewind().unwrap();
        let mut after = Vec::new();
        view.read_to_end(&mut after).unwrap();
        assert_eq!(after, file, "and the file reads as it is afterwards");
    }

    /// [#576 RV20-4] The hidden end-record signatures are capped at the entry
    /// limit: an archive within it holds at most that many archives stored at
    /// its top level, each with one end record. Here, with a limit of 5, five
    /// others are hidden and the parse runs; a sixth is refused.
    #[test]
    fn end_record_signatures_are_capped_at_the_entry_limit() {
        let outcome = |others: usize| {
            let mut file = b"PK\x05\x06".repeat(others);
            file.extend(end_record(1));
            parse_bounded(Cursor::new(file), 5, |_| ()).unwrap()
        };
        let (at_limit, over) = (outcome(5), outcome(6));
        println!("limit 5: 5 other end records -> {at_limit:?}; 6 -> {over:?}");
        assert_eq!(at_limit, Ok(()), "as many as the limit are hidden");
        assert_eq!(over, Err(EntryExcess::EndRecords), "one more is refused");
    }

    /// A reader that grows by `tail` right after it is first measured
    /// (`seek(End)`), as a file another program appends to does.
    struct GrowsAfterTheMeasure {
        inner: Cursor<Vec<u8>>,
        tail: Option<Vec<u8>>,
    }

    impl std::io::Read for GrowsAfterTheMeasure {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.inner.read(buf)
        }
    }

    impl std::io::Seek for GrowsAfterTheMeasure {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            let at = self.inner.seek(pos)?;
            if matches!(pos, SeekFrom::End(_)) {
                if let Some(tail) = self.tail.take() {
                    self.inner.get_mut().extend(tail);
                }
            }
            Ok(at)
        }
    }

    /// [#576 PR #594 review] The scan reads the file as long as it was
    /// measured, as the end-record search does: bytes past that length are no
    /// part of the archive the checks judge. Here six end-record signatures
    /// land past it, over a limit of 5; the scan read on to the end and
    /// refused the archive for them.
    #[test]
    fn the_scan_stops_at_the_length_the_file_was_measured_at() {
        let reader = GrowsAfterTheMeasure {
            inner: Cursor::new(end_record(1)),
            tail: Some(b"PK\x05\x06".repeat(6)),
        };
        let outcome = parse_bounded(reader, 5, |_| ()).unwrap();
        println!("an end record of 1, then 6 more signatures past the measured end -> {outcome:?}");
        assert_eq!(
            outcome,
            Ok(()),
            "what lies past the measured end is not scanned"
        );
    }
}
