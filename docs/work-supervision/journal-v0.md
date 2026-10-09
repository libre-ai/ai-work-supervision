# Work Supervision journal, format v0

Schema identifier: `libre-ai.work-supervision.journal.v0`.

The journal is the authority of a Work Supervision v0 root. The SQLite state is
a projection that can be rebuilt from it. The format is the storage of Work
Supervision v0 under ADR-0042 §5 (`libre-ai/project-governance`); it is not a
cross-repository contract and is not published in `libre-ai/schemas-and-contracts`.

Two implementations exist on purpose and share no code:

| Role | Crate | Canonical encoding |
| --- | --- | --- |
| writer | `crates/work-supervision-journal` | `serde_jcs` |
| verifier (`ws-journal-verify`) | `crates/work-supervision-journal-verifier` | own RFC 8785 encoder |

Their tests compare them on every refusal case and on a generated corpus of
values; a disagreement is a failing test.

## File

- One file, UTF-8, one entry per line, each line terminated by `\n` (0x0A).
- A line is **committed** once its `\n` is durable. The writer appends the line
  and its newline in one write, then synchronises the file before returning.
- A last line without `\n` is a **torn tail**: an interrupted append, reported
  apart from corruption.
- A single writer holds an exclusive lock on the file while it is open.

## Entry

Each line is the RFC 8785 (JCS) encoding of an object with exactly these keys:

| Key | Type | Rule |
| --- | --- | --- |
| `at` | string | UTC instant `YYYY-MM-DDTHH:MM:SS.mmmZ`, on a real Gregorian date; supplied by the caller, not required to increase |
| `digest` | string | 64 lowercase hex characters: SHA-256 of the JCS encoding of the entry **without** the `digest` key |
| `event` | object | exactly `{ "data": <object>, "kind": <string> }` |
| `prev` | string or `null` | `null` for `seq = 1`, otherwise the `digest` of the previous entry |
| `schema` | string | `libre-ai.work-supervision.journal.v0` |
| `seq` | integer | 1 for the first entry, then the previous `seq` plus one |

`kind` grammar: one or more dot-separated segments, each `[a-z][a-z0-9-]*`, at
most 64 bytes in total (`mission.note`, `journal.recovered`).

Value domain, everywhere in the entry: strings, booleans, `null`, integers in
`[-(2^53 - 1), 2^53 - 1]`, arrays and objects. Floats are refused, so numbers are
always written in decimal by every implementation. Containers nest at most 32
deep, the entry object being depth 1. A line holds at most 1 MiB (1 048 576
bytes) without its newline.

## Verification order

A verifier applies these checks to each line, in this order, and reports the
first failure with its 1-based line number:

| # | Check | Code |
| --- | --- | --- |
| 1 | line length | `line-too-long` |
| 2 | JSON parse (UTF-8) | `malformed` |
| 3 | value domain, one depth-first pass in key byte order: numbers at leaves, depth at containers | `number-invalid`, `depth-exceeded` |
| 4 | the line equals the JCS encoding of its parsed value (this also refuses duplicate keys) | `non-canonical` |
| 5 | exactly the six envelope keys | `envelope-invalid` |
| 6 | `schema` | `schema-unknown` |
| 7 | `seq` is an integer, `prev` is `null` or a lowercase digest | `envelope-invalid` |
| 8 | chain: genesis, sequence, previous digest | `genesis-invalid`, `sequence-invalid`, `previous-digest-mismatch` |
| 9 | `at` | `envelope-invalid` (not a string), `timestamp-invalid` |
| 10 | `event` shape, then `kind` grammar | `envelope-invalid`, `kind-invalid` |
| 11 | `digest` is a lowercase digest | `envelope-invalid` |
| 12 | recomputed digest | `digest-mismatch` |

## Torn tail and recovery

The writer refuses to open a journal with a torn tail unless asked to recover.
Recovery moves the torn bytes, unchanged, to `<journal file name>.torn-<line>`
next to the journal, truncates the journal to its last committed line, then
appends an entry of kind `journal.recovered` whose data is
`{ "quarantine": <file name>, "torn_bytes": <n>, "torn_digest": <hex> }`.
Nothing is deleted.

## `ws-journal-verify`

```sh
cargo run --locked -p work-supervision-journal-verifier -- <journal-file>
```

| Exit | Meaning | Output |
| --- | --- | --- |
| 0 | valid | stdout: `valid: <n> entries verified; head seq <s> digest <hex>` (or `no head`) |
| 1 | invalid | stderr: `invalid: <code> at line <l>; <v> entries verified before it` |
| 2 | unreadable file or wrong usage | stderr; an unreadable file is never reported as empty |
| 3 | torn tail | stderr: `torn-tail: line <l> has no terminating newline; <v> entries verified before it` |

Output names codes, line numbers and counts only; it never echoes journal content.

## Worked example

The genesis entry below, without its `digest` key, is

```
{"at":"2026-10-09T00:00:00.000Z","event":{"data":{"text":"é\u0001\"x"},"kind":"mission.note"},"prev":null,"schema":"libre-ai.work-supervision.journal.v0","seq":1}
```

and its SHA-256 (computed with `shasum -a 256`, independently of both
implementations) is
`a6f1c5db5d50f88b1156d93d26d094856f99a71084fe18ca10bbff485fbf5f8c`. The test
`encodes_the_hand_computed_vector` pins this value.

## Limits

- The chain detects a modified, removed, inserted or reordered entry. It does not
  detect a consistent rewrite of the whole file; anchoring the head digest outside
  the root is a later slice.
- Opening verifies the whole file: the cost is linear in the journal size.
