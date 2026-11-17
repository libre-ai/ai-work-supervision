#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! Structure of `harness-adapters-v0.md` and `steering-surfaces-v0.md`.
//!
//! `docs/cahier-des-charges.md` and `docs/prd.md` cite the first document by
//! line numbers, so its first 155 lines may only be followed, never shifted.

use std::fs;
use std::path::Path;

use work_supervision_daemon::Config;

const ADAPTERS: &str = "harness-adapters-v0.md";
const SURFACES: &str = "steering-surfaces-v0.md";

/// Last line of the text that other documents cite by line number.
const CITED_LINES: usize = 155;
/// Line of the heading of §2, the last `##` heading of the document.
const CONTRACT_HEADING_LINE: usize = 114;

const PROTOCOL_REPOSITORY: &str = "libre-ai/project-governance@";
const PROTOCOL_PATH: &str = ":docs/architecture/forge/EXECUTION-PROTOCOL.md";

/// A specification is a declared source: one that cannot be read fails the
/// test, it is never read as an empty document.
fn document(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/work-supervision")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{name} is unreadable: {error}"))
}

/// The line numbered `number` as an editor and a citation count it (from 1).
fn line<'a>(lines: &[&'a str], number: usize) -> &'a str {
    lines
        .get(number - 1)
        .copied()
        .unwrap_or_else(|| panic!("line {number} is missing"))
}

fn is_heading(line: &str) -> bool {
    line.starts_with('#')
}

/// The lines under the `###` heading `title`, up to the next heading.
fn section<'a>(lines: &[&'a str], title: &str) -> Vec<&'a str> {
    let heading = format!("### {title}");
    let start = lines
        .iter()
        .position(|line| *line == heading)
        .unwrap_or_else(|| panic!("heading {heading:?} is missing"));
    lines[start + 1..]
        .iter()
        .copied()
        .take_while(|line| !is_heading(line))
        .collect()
}

/// The number of bullets of each bullet list, in order. A list is a run of
/// `- ` bullets and their indented continuation lines.
fn bullet_lists(lines: &[&str]) -> Vec<usize> {
    let mut lists = Vec::new();
    let mut current = 0_usize;
    for line in lines {
        if line.starts_with("- ") {
            current += 1;
        } else if current > 0 && !line.starts_with("  ") {
            lists.push(current);
            current = 0;
        }
    }
    if current > 0 {
        lists.push(current);
    }
    lists
}

/// The first cell of every data row of the tables found in `lines`: the
/// header row and the `---` separator row are left out.
fn first_cells(lines: &[&str]) -> Vec<String> {
    let rows: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with('|'))
        .collect();
    rows.iter()
        .skip(2)
        .map(|row| {
            row.trim_matches('|')
                .split('|')
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned()
        })
        .collect()
}

/// Whether `line` carries the pinned reference to the execution protocol:
/// the governance repository, a full lowercase commit digest, the path.
fn has_pinned_protocol_reference(line: &str) -> bool {
    line.match_indices(PROTOCOL_REPOSITORY).any(|(at, found)| {
        let rest = &line[at + found.len()..];
        let digest: String = rest.chars().take(40).collect();
        digest.len() == 40
            && digest
                .chars()
                .all(|character| matches!(character, '0'..='9' | 'a'..='f'))
            && rest[digest.len()..].starts_with(PROTOCOL_PATH)
    })
}

/// Whether a number is followed by the word `lines` (a size cap).
fn has_line_cap(text: &str) -> bool {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|character: char| !character.is_alphanumeric())
                .to_lowercase()
        })
        .collect();
    words.windows(2).any(|pair| {
        !pair[0].is_empty()
            && pair[0].chars().all(|character| character.is_ascii_digit())
            && pair[1] == "lines"
    })
}

#[test]
fn the_cited_line_anchors_stay_at_their_numbers() {
    let text = document(ADAPTERS);
    let lines: Vec<&str> = text.lines().collect();

    assert_eq!(line(&lines, 23), "## 1. The bridge: `ws hook <harness>`");
    assert_eq!(
        line(&lines, CONTRACT_HEADING_LINE),
        "## 2. The executor adapter contract (after C0)"
    );
    assert!(line(&lines, 129).starts_with("The profile, its argument template"));
    assert_eq!(line(&lines, 136), "### Vocabulary of a run");
    assert!(
        line(&lines, CITED_LINES)
            .ends_with("goes to a decision request before the run, not during it.")
    );
}

#[test]
fn the_contract_table_keeps_its_seven_elements_in_order() {
    let text = document(ADAPTERS);
    let lines: Vec<&str> = text.lines().collect();
    let vocabulary = lines
        .iter()
        .position(|line| *line == "### Vocabulary of a run")
        .expect("the vocabulary heading");

    assert_eq!(
        first_cells(&lines[CONTRACT_HEADING_LINE..vocabulary]),
        [
            "Launch",
            "Environment",
            "Waiting for input",
            "Approvals",
            "Result",
            "Identity",
            "Delegation"
        ]
    );
}

#[test]
fn three_subsections_follow_the_cited_lines_and_no_higher_heading_does() {
    let text = document(ADAPTERS);
    let lines: Vec<&str> = text.lines().collect();

    let appended: Vec<&str> = lines[CITED_LINES..]
        .iter()
        .copied()
        .filter(|line| is_heading(line))
        .collect();
    assert_eq!(
        appended,
        [
            "### Run end without a deliverable",
            "### Stall",
            "### Adapter code is translation only"
        ]
    );

    let higher: Vec<&str> = lines[CONTRACT_HEADING_LINE..]
        .iter()
        .copied()
        .filter(|line| line.starts_with("# ") || line.starts_with("## "))
        .collect();
    assert_eq!(higher, Vec::<&str>::new());
}

#[test]
fn adapter_code_has_two_closed_lists_and_no_line_cap() {
    let text = document(ADAPTERS);
    let lines: Vec<&str> = text.lines().collect();
    let section = section(&lines, "Adapter code is translation only");

    assert_eq!(bullet_lists(&section), [3, 6]);
    assert!(!has_line_cap(&section.join(" ")));
}

#[test]
fn stall_names_settings_the_daemon_accepts() {
    let text = document(ADAPTERS);
    let lines: Vec<&str> = text.lines().collect();
    let section = section(&lines, "Stall").join("\n");

    for setting in [
        "idle_after_ms",
        "grace_ms",
        "max_duration_seconds",
        "max_output_bytes",
    ] {
        assert!(
            section.contains(&format!("`{setting}`")),
            "`{setting}` is not named in « Stall »"
        );
    }
    assert!(Config::parse("[runs]\nidle_after_ms = 1\ngrace_ms = 1\n").is_ok());
}

#[test]
fn the_execution_protocol_is_named_only_by_a_pinned_reference() {
    for name in [ADAPTERS, SURFACES] {
        let text = document(name);
        for word in ["executor_unavailable", "recoverable_failure"] {
            assert!(!text.contains(word), "{name} restates {word}");
        }
        for (index, line) in text.lines().enumerate() {
            if line.contains("ExecutionProtocol") || line.contains("EXECUTION-PROTOCOL") {
                assert!(
                    has_pinned_protocol_reference(line),
                    "{name}:{} names the execution protocol without its pinned reference",
                    index + 1
                );
            }
        }
    }
}

#[test]
fn the_steering_surfaces_are_specified_as_s1_to_s8() {
    let text = document(SURFACES);
    let lines: Vec<&str> = text.lines().collect();

    assert_eq!(line(&lines, 1), "# Work Supervision v0 — steering surfaces");
    assert!(text.contains("specified, not enabled"));
    assert_eq!(
        first_cells(&lines),
        ["S1", "S2", "S3", "S4", "S5", "S6", "S7", "S8"]
    );
}
