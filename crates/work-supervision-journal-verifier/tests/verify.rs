#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use work_supervision_journal::{Event, Journal, OpenMode, SCHEMA, Timestamp};
use work_supervision_journal_verifier::{Code, Outcome, canonical, verify};

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_journal(path: &Path, data: &[Value]) {
    let (mut journal, _) = Journal::open(path, OpenMode::Strict).unwrap();
    for (index, item) in data.iter().enumerate() {
        let Value::Object(map) = item.clone() else {
            panic!("event data must be an object");
        };
        let at = format!("2026-10-09T00:{:02}:00.000Z", index % 60);
        journal
            .append(
                Timestamp::parse(&at).unwrap(),
                Event::new("mission.note", map).unwrap(),
            )
            .unwrap();
    }
}

fn lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn joined(lines: &[String]) -> Vec<u8> {
    let mut text = lines.join("\n");
    text.push('\n');
    text.into_bytes()
}

/// Seals an arbitrary envelope with a correct digest, to reach checks past the digest.
fn seal(mut body: Map<String, Value>) -> String {
    body.remove("digest");
    let digest = sha256_hex(&serde_jcs::to_vec(&body).unwrap());
    body.insert("digest".into(), json!(digest));
    String::from_utf8(serde_jcs::to_vec(&body).unwrap()).unwrap()
}

fn genesis_body(data: Value) -> Map<String, Value> {
    let mut body = Map::new();
    body.insert("at".into(), json!("2026-10-09T00:00:00.000Z"));
    body.insert(
        "event".into(),
        json!({ "data": data, "kind": "mission.note" }),
    );
    body.insert("prev".into(), Value::Null);
    body.insert("schema".into(), json!(SCHEMA));
    body.insert("seq".into(), json!(1));
    body
}

fn tricky_data() -> Vec<Value> {
    vec![
        json!({ "text": "\u{e9}\u{1}\"x", "slash": "a/b\\c" }),
        json!({ "\u{fb01}": 1, "\u{1f600}": 2, "a": 3, "Z": 4, "\u{e9}": 5 }),
        json!({ "controls": "\u{0}\u{8}\u{9}\u{a}\u{c}\u{d}\u{1f}\u{7f}", "separators": "\u{2028}\u{2029}" }),
        json!({ "max": 9_007_199_254_740_991_i64, "min": -9_007_199_254_740_991_i64, "zero": 0 }),
        json!({ "nested": { "list": [true, false, null, [], {}], "deeper": { "k": "v" } } }),
        json!({}),
    ]
}

#[test]
fn a_journal_written_by_the_writer_verifies_and_reports_its_volume() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    let mut data = tricky_data();
    for index in 0..44 {
        data.push(json!({ "index": index }));
    }
    write_journal(&path, &data);
    let outcome = verify(fs::File::open(&path).unwrap()).unwrap();
    let (writer, _) = Journal::open(&path, OpenMode::Strict).unwrap();
    let writer_head = writer.head().unwrap();
    match outcome {
        Outcome::Valid { entries, head } => {
            assert_eq!(entries, 50);
            let head = head.unwrap();
            assert_eq!(head.seq, 50);
            assert_eq!(head.digest, writer_head.digest().to_hex());
        }
        other => panic!("expected a valid journal, got {other:?}"),
    }
}

#[test]
fn an_empty_journal_is_valid_with_zero_entries() {
    assert_eq!(
        verify(&b""[..]).unwrap(),
        Outcome::Valid {
            entries: 0,
            head: None
        }
    );
}

struct Case {
    name: &'static str,
    bytes: Vec<u8>,
    code: Code,
    line: u64,
}

fn corrupted_cases(dir: &Path) -> Vec<Case> {
    let path = dir.join("base.jsonl");
    write_journal(
        &path,
        &[
            json!({ "text": "one" }),
            json!({ "text": "two" }),
            json!({ "text": "three" }),
        ],
    );
    let base = lines(&path);
    let other_path = dir.join("other.jsonl");
    write_journal(
        &other_path,
        &[json!({ "text": "uno" }), json!({ "text": "dos" })],
    );
    let other = lines(&other_path);

    let mut deep = json!("leaf");
    for _ in 0..40 {
        deep = json!([deep]);
    }
    let mut bad_time = genesis_body(json!({}));
    bad_time.insert("at".into(), json!("2026-02-30T00:00:00.000Z"));
    let mut bad_kind = genesis_body(json!({}));
    bad_kind.insert("event".into(), json!({ "data": {}, "kind": "Mission" }));
    let mut extra = genesis_body(json!({}));
    extra.insert("extra".into(), json!(1));
    let mut uppercase = base[0].clone();
    let digest_at = uppercase.find("\"digest\":\"").unwrap() + 10;
    uppercase.replace_range(
        digest_at..digest_at + 64,
        &uppercase[digest_at..digest_at + 64].to_uppercase(),
    );

    vec![
        Case {
            name: "modified byte",
            bytes: joined(&[
                base[0].clone(),
                base[1].replace("two", "twO"),
                base[2].clone(),
            ]),
            code: Code::DigestMismatch,
            line: 2,
        },
        Case {
            name: "removed entry",
            bytes: joined(&[base[0].clone(), base[2].clone()]),
            code: Code::SequenceInvalid,
            line: 2,
        },
        Case {
            name: "reordered",
            bytes: joined(&[base[1].clone(), base[0].clone()]),
            code: Code::GenesisInvalid,
            line: 1,
        },
        Case {
            name: "spliced",
            bytes: joined(&[base[0].clone(), other[1].clone()]),
            code: Code::PreviousDigestMismatch,
            line: 2,
        },
        Case {
            name: "spacing",
            bytes: joined(&[base[0].replacen("\"seq\":1", "\"seq\": 1", 1)]),
            code: Code::NonCanonical,
            line: 1,
        },
        Case {
            name: "duplicate key",
            bytes: joined(&[base[0].replacen("{\"at\"", "{\"at\":\"x\",\"at\"", 1)]),
            code: Code::NonCanonical,
            line: 1,
        },
        Case {
            name: "unknown field",
            bytes: joined(&[seal(extra)]),
            code: Code::EnvelopeInvalid,
            line: 1,
        },
        Case {
            name: "unknown schema",
            bytes: joined(&[base[0].replace(SCHEMA, "libre-ai.work-supervision.journal.v9")]),
            code: Code::SchemaUnknown,
            line: 1,
        },
        Case {
            name: "float",
            bytes: joined(&[seal(genesis_body(json!({ "ratio": 1.5 })))]),
            code: Code::NumberInvalid,
            line: 1,
        },
        Case {
            name: "unsafe integer",
            bytes: joined(&[seal(genesis_body(
                json!({ "n": 9_007_199_254_740_992_u64 }),
            ))]),
            code: Code::NumberInvalid,
            line: 1,
        },
        Case {
            name: "too deep",
            bytes: joined(&[seal(genesis_body(json!({ "deep": deep })))]),
            code: Code::DepthExceeded,
            line: 1,
        },
        Case {
            name: "bad timestamp",
            bytes: joined(&[seal(bad_time)]),
            code: Code::TimestampInvalid,
            line: 1,
        },
        Case {
            name: "bad kind",
            bytes: joined(&[seal(bad_kind)]),
            code: Code::KindInvalid,
            line: 1,
        },
        Case {
            name: "uppercase digest",
            bytes: joined(&[uppercase]),
            code: Code::EnvelopeInvalid,
            line: 1,
        },
        Case {
            name: "not json",
            bytes: b"not json\n".to_vec(),
            code: Code::Malformed,
            line: 1,
        },
        Case {
            name: "invalid utf-8",
            bytes: b"{\"at\":\"\xff\"}\n".to_vec(),
            code: Code::Malformed,
            line: 1,
        },
        Case {
            name: "too long",
            bytes: {
                let mut long = vec![b' '; (1 << 20) + 1];
                long.push(b'\n');
                long
            },
            code: Code::LineTooLong,
            line: 1,
        },
    ]
}

#[test]
fn every_corruption_is_refused_with_its_code_and_line() {
    let dir = tempfile::tempdir().unwrap();
    for case in corrupted_cases(dir.path()) {
        let outcome = verify(&case.bytes[..]).unwrap();
        assert_eq!(
            outcome,
            Outcome::Invalid {
                line: case.line,
                code: case.code,
                verified: case.line - 1
            },
            "case {}",
            case.name
        );
    }
}

#[test]
fn the_writer_and_the_verifier_refuse_each_corruption_identically() {
    let dir = tempfile::tempdir().unwrap();
    for (index, case) in corrupted_cases(dir.path()).into_iter().enumerate() {
        let path = dir.path().join(format!("case-{index}.jsonl"));
        fs::write(&path, &case.bytes).unwrap();
        let writer = Journal::open(&path, OpenMode::Strict).unwrap_err();
        assert_eq!(writer.code(), case.code.code(), "case {}", case.name);
        assert_eq!(writer.line(), Some(case.line), "case {}", case.name);
    }
}

#[test]
fn a_torn_tail_is_reported_apart_from_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.v0.jsonl");
    write_journal(&path, &[json!({ "text": "one" }), json!({ "text": "two" })]);
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(b"{\"at\":\"20");
    assert_eq!(
        verify(&bytes[..]).unwrap(),
        Outcome::TornTail {
            line: 3,
            verified: 2
        }
    );
}

#[test]
fn keys_are_ordered_by_utf16_code_units_not_utf8_bytes() {
    // U+1F600 is D83D DE00 in UTF-16 and sorts before U+FB01; in UTF-8 it sorts after.
    let value = json!({ "\u{fb01}": 1, "\u{1f600}": 2 });
    let bytes = canonical::encode(&value).unwrap();
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "{\"\u{1f600}\":2,\"\u{fb01}\":1}"
    );
}

#[test]
fn canonical_encoding_refuses_floats() {
    assert_eq!(canonical::encode(&json!({ "x": 0.5 })), None);
}

/// Deterministic pseudo-random values (no extra dependency), exercising the
/// characters and key orders where JCS implementations diverge.
struct Generator(u64);

impl Generator {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn string(&mut self) -> String {
        const ALPHABET: [char; 16] = [
            'a',
            'Z',
            '0',
            '"',
            '\\',
            '/',
            '\u{0}',
            '\u{1f}',
            '\u{7f}',
            '\u{e9}',
            '\u{2028}',
            '\u{fb01}',
            '\u{ffff}',
            '\u{10000}',
            '\u{1f600}',
            ' ',
        ];
        let length = self.next() % 6;
        (0..length)
            .map(|_| ALPHABET[(self.next() % 16) as usize])
            .collect()
    }

    fn value(&mut self, depth: u32) -> Value {
        match self.next() % if depth > 3 { 4 } else { 6 } {
            0 => Value::Null,
            1 => Value::Bool(self.next().is_multiple_of(2)),
            2 => json!((self.next() as i64) - (1 << 30)),
            3 => Value::String(self.string()),
            4 => Value::Array(
                (0..self.next() % 4)
                    .map(|_| self.value(depth + 1))
                    .collect(),
            ),
            _ => Value::Object(
                (0..self.next() % 5)
                    .map(|_| (self.string(), self.value(depth + 1)))
                    .collect(),
            ),
        }
    }
}

#[test]
fn the_independent_canonical_encoding_matches_serde_jcs_on_a_generated_corpus() {
    let mut generator = Generator(0x5eed);
    for _ in 0..5_000 {
        let value = generator.value(0);
        assert_eq!(
            canonical::encode(&value).unwrap(),
            serde_jcs::to_vec(&value).unwrap(),
            "value {value:?}"
        );
    }
}
