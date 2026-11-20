//! Canonicalisation: a non-canonical spelling of a file (shuffled keys,
//! arbitrary whitespace, CRLF, escaped characters, other float spellings,
//! `-0`) decodes to the same model and re-encodes to the canonical bytes.

use std::fmt::Write as _;

use serde_json::Value;

use super::test_helpers::{generate, text_of, Rng, MEDIUM, SMALL};
use super::{decode, encode, DecodeOptions, Mode};

/// Re-spell a JSON document: keys shuffled (the first two top-level members
/// stay first), random whitespace, random escapes, other float spellings.
/// History entries keep their keys and numbers (they are opaque and kept as
/// written); only their whitespace and escapes change.
struct Respeller {
    rng: Rng,
}

impl Respeller {
    fn ws(&mut self, out: &mut String) {
        for _ in 0..self.rng.below(3) {
            out.push(*self.rng.pick(&[' ', '\t', '\n', '\r']));
        }
    }

    fn string(&mut self, out: &mut String, s: &str) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '/' if self.rng.chance(0.5) => out.push_str("\\/"),
                c if (c as u32) < 0x20 || self.rng.chance(0.2) => {
                    let mut units = [0u16; 2];
                    for unit in c.encode_utf16(&mut units) {
                        let _ = if self.rng.chance(0.5) {
                            write!(out, "\\u{unit:04X}")
                        } else {
                            write!(out, "\\u{unit:04x}")
                        };
                    }
                }
                c => out.push(c),
            }
        }
        out.push('"');
    }

    fn number(&mut self, out: &mut String, n: &serde_json::Number, opaque: bool) {
        let Some(v) = n.as_f64().filter(|_| n.is_f64() && !opaque) else {
            out.push_str(&n.to_string());
            return;
        };
        let spelled = match self.rng.below(6) {
            0 => format!("{v:e}"),
            1 => format!("{v:.17e}").replace('e', "E+").replace("E+-", "E-"),
            2 if v == 0.0 && v.is_sign_negative() => "-0".to_string(),
            3 if v.fract() == 0.0 && v.abs() < 1e15 && !(v == 0.0 && v.is_sign_negative()) => {
                format!("{}", v as i64)
            }
            4 => {
                let s = format!("{v:?}");
                if s.contains('e') || s.contains('E') {
                    s
                } else {
                    format!("{s}000")
                }
            }
            _ => serde_json::to_string(&v).unwrap(),
        };
        let back: f64 = serde_json::from_str(&spelled).unwrap();
        assert_eq!(
            back.to_bits(),
            v.to_bits(),
            "test respelling {spelled} of {v}"
        );
        out.push_str(&spelled);
    }

    fn value(&mut self, out: &mut String, v: &Value, opaque: bool, top: bool) {
        self.ws(out);
        match v {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => self.number(out, n, opaque),
            Value::String(s) => self.string(out, s),
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    self.value(out, item, opaque, false);
                }
                self.ws(out);
                out.push(']');
            }
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                if !opaque {
                    // Shuffle (Fisher-Yates), then pin the sniffed prefix.
                    for i in (1..keys.len()).rev() {
                        let j = self.rng.below(i + 1);
                        keys.swap(i, j);
                    }
                    if top {
                        keys.retain(|k| *k != "format" && *k != "version");
                        keys.insert(0, map.keys().find(|k| *k == "version").unwrap());
                        keys.insert(0, map.keys().find(|k| *k == "format").unwrap());
                    }
                }
                out.push('{');
                for (i, key) in keys.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    self.ws(out);
                    // The sniffed prefix is spelled literally (no escapes).
                    let literal = top && (key == "format" || key == "version");
                    if literal {
                        let _ = write!(out, "\"{key}\"");
                    } else {
                        self.string(out, key);
                    }
                    self.ws(out);
                    out.push(':');
                    let child_opaque = opaque || (top && key == "history");
                    if literal {
                        self.ws(out);
                        out.push_str(&map[key.as_str()].to_string());
                    } else {
                        self.value(out, &map[key.as_str()], child_opaque, false);
                    }
                }
                self.ws(out);
                out.push('}');
            }
        }
        self.ws(out);
    }
}

/// History entries are opaque: their key order is kept. `serde_json::Value`
/// sorts object keys, so a test file needs entries whose keys are sorted.
fn sorted_history(extras: &mut super::Extras) {
    for list in [&mut extras.history.undo, &mut extras.history.redo] {
        for entry in list.iter_mut() {
            let value: Value = serde_json::from_str(entry.as_str()).unwrap();
            *entry = super::RawJson::parse(&serde_json::to_string(&value).unwrap()).unwrap();
        }
    }
}

#[test]
fn respelled_files_decode_to_the_same_model_and_re_encode_canonically() {
    for seed in 0..60 {
        let (p, mut extras) = generate(seed, if seed % 5 == 0 { MEDIUM } else { SMALL });
        sorted_history(&mut extras);
        let canonical = encode(&p, &extras, Mode::Normal).unwrap();
        let value: Value = serde_json::from_slice(&canonical).unwrap();
        let mut respelled = String::new();
        Respeller {
            rng: Rng(seed + 1000),
        }
        .value(&mut respelled, &value, false, true);
        assert_ne!(respelled.as_bytes(), &canonical[..]);
        let original = decode(&canonical, &DecodeOptions::default()).unwrap();
        let decoded = match decode(respelled.as_bytes(), &DecodeOptions::default()) {
            Ok(d) => d,
            Err(e) => panic!("seed {seed}: {e}\n{respelled}"),
        };
        assert!(
            decoded.project == original.project,
            "seed {seed}: model differs"
        );
        assert_eq!(decoded.extras, original.extras, "seed {seed}");
        let again = encode(&decoded.project, &decoded.extras, Mode::Normal).unwrap();
        assert!(again == canonical, "seed {seed}: not canonical again");
    }
}

#[test]
fn crlf_line_ends_and_indentation_are_accepted() {
    let (p, extras) = generate(11, SMALL);
    let canonical = encode(&p, &extras, Mode::Normal).unwrap();
    let crlf = text_of(&canonical).replace('\n', "\r\n  ");
    let decoded = decode(crlf.as_bytes(), &DecodeOptions::default()).unwrap();
    assert_eq!(
        encode(&decoded.project, &decoded.extras, Mode::Normal).unwrap(),
        canonical
    );
    // Whitespace inside the sniffed prefix too (a pretty-printer's output).
    let pretty = format!(
        "\n {{ \"format\" : \"stalx\" ,\n  \"version\" : 2 ,{}",
        &text_of(&canonical)[super::PREFIX.len() + 2..]
    );
    let decoded = decode(pretty.as_bytes(), &DecodeOptions::default()).unwrap();
    assert_eq!(
        encode(&decoded.project, &decoded.extras, Mode::Normal).unwrap(),
        canonical
    );
}

#[test]
fn minus_zero_and_integer_spellings_read_as_doubles() {
    let (p, extras) = generate(5, SMALL);
    let canonical = encode(&p, &extras, Mode::Normal).unwrap();
    let text = text_of(&canonical);
    // The quality of every line is a double; spell 1.0 as 1 and 1e0.
    let respelled = text
        .replacen("\"quality\":1.0", "\"quality\":1", 1)
        .replacen("\"quality\":1.0", "\"quality\":1e0", 1);
    let decoded = decode(respelled.as_bytes(), &DecodeOptions::default()).unwrap();
    assert_eq!(
        encode(&decoded.project, &decoded.extras, Mode::Normal).unwrap(),
        canonical
    );
    for (spelled, bits) in [
        ("-0", (-0.0f64).to_bits()),
        ("-0.0", (-0.0f64).to_bits()),
        ("-0e7", (-0.0f64).to_bits()),
        ("0", 0),
        ("1", 1.0f64.to_bits()),
        ("10e-1", 1.0f64.to_bits()),
        ("0.1e1", 1.0f64.to_bits()),
        ("100000000000000000000", 1e20f64.to_bits()),
    ] {
        let v: f64 = serde_json::from_str(spelled).unwrap();
        assert_eq!(v.to_bits(), bits, "{spelled}");
    }
}

#[test]
fn escaped_strings_come_back_canonical() {
    let (p, extras) = generate(9, SMALL);
    let canonical = encode(&p, &extras, Mode::Normal).unwrap();
    // `A` for `A`, `\/` for `/`, and a surrogate pair for an astral char.
    let text = text_of(&canonical)
        .replace("https://", "https:\\/\\/")
        .replace("\"Synthetic", "\"\\u0053ynthetic")
        .replace('\u{1f987}', "\\ud83e\\udd87");
    let decoded = decode(text.as_bytes(), &DecodeOptions::default()).unwrap();
    assert_eq!(
        encode(&decoded.project, &decoded.extras, Mode::Normal).unwrap(),
        canonical
    );
}
