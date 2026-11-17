# Work Supervision v0 — flow measures

Crates: `crates/work-supervision-domain` (pure arithmetic, module `flow`),
`crates/work-supervision-store` (one read of the projection),
`crates/work-supervision-daemon` (operation `flow.measures`) and
`crates/work-supervision-cli` (`ws flow`).

`ws flow` answers, read-only, how long accepted missions took and how many of
them were accepted without a rejection. It is a read model: it is derived from
the timeline the projection already keeps (`mission_events`,
`coordination-v0.md`, « Events ») and from the state of the missions. It adds
no journal event, no table and no migration, and it does not read the clock:
two reads of the same root, or a read before and a read after `ws rebuild`
from the same journal, answer the same bytes.

It is not a report of a mission: `ws report <id>` and its schema are unchanged
(`coordination-v0.md`, « Report »).

## What is measured

Only missions in state `accepted` are examined. Four kinds of timeline entries
are read, and no other:

| Kind | Use |
| --- | --- |
| `mission.created` | opens the lead time |
| `mission.result-submitted` | the latest one opens the review time |
| `mission.rejected` | one is enough to lose the first-pass count |
| `mission.accepted` | closes both durations |

For each accepted mission:

- the **lead time** is the duration, in seconds, from its `mission.created`
  entry to its `mission.accepted` entry;
- the **review time** is the duration from its latest
  `mission.result-submitted` entry to its `mission.accepted` entry;
- it is **accepted at the first pass** when its timeline carries no
  `mission.rejected` entry.

Instants are the `at` of the journal entries (`YYYY-MM-DDTHH:MM:SS.mmmZ`),
read to the whole second: the milliseconds are dropped from each instant
before the subtraction.

Over the retained missions (below), with `n` their number:

- **P50** and **P85** of each duration by the nearest-rank method: the
  durations are sorted ascending, the rank is `ceil(p × n / 100)` in integer
  arithmetic, counted from 1, and the percentile is the value at that rank.
  With the lead times 10, 20, 30, 40 and 100 hours, P50 is 30 hours (rank 3)
  and P85 is 100 hours (rank 5); with the values 1 to 20, P85 is 17;
- the **throughput**, in hundredths of a mission per week:
  `n × 100 × 604800 / max(604800, window)` in integer division, where `window`
  is the number of seconds from the earliest retained creation to the latest
  retained acceptance. A window shorter than a week counts as one week: three
  missions inside two days give 300, two missions over fourteen days give 100.

With `n = 0`, P50, P85 and the throughput are `null`, never 0: there is no
measure, which is not a measure of zero.

## Missions that cannot be measured

An accepted mission whose measure is impossible is neither counted nor
ignored: it is left out of `n` and listed with its identifier and one code.
Closed list, evaluated in this order, one code per mission:

| Code | Meaning |
| --- | --- |
| `timeline.missing` | a `mission.created`, `mission.result-submitted` or `mission.accepted` entry is absent from the projected timeline |
| `instant.invalid` | one of the three instants is not in the journal format |
| `duration.negative` | the acceptance precedes the creation or the latest submission |

`timeline.missing` is what a root created before the timeline migration
answers for its earlier missions until `ws rebuild` has filled the timeline
from the journal. `duration.negative` can happen because the journal orders by
sequence number and does not require instants to grow.

The counters sum: `retained` plus the number of entries of `unusable` equals
`examined`.

## Response

Operation `flow.measures` of the daemon, without parameter. The response is a
JSON object of nine keys:

| Key | Value |
| --- | --- |
| `schema` | `libre-ai.work-supervision.flow.v0` |
| `examined` | accepted missions looked at |
| `retained` | missions the measures stand on (`n`) |
| `simulated` | retained missions executed by the fake agent |
| `accepted_first_pass` | retained missions accepted without a rejection |
| `lead_seconds` | `{ "p50", "p85" }`, seconds or `null` |
| `review_seconds` | `{ "p50", "p85" }`, seconds or `null` |
| `throughput_per_week_hundredths` | integer or `null` |
| `unusable` | array of `{ "mission", "code" }`, by mission identifier |

The serialisation is deterministic: object keys are written in one fixed
order (lexicographic), `unusable` by mission identifier.

```json
{
  "accepted_first_pass": 1,
  "examined": 2,
  "lead_seconds": { "p50": 3600, "p85": 7200 },
  "retained": 2,
  "review_seconds": { "p50": 60, "p85": 600 },
  "schema": "libre-ai.work-supervision.flow.v0",
  "simulated": 2,
  "throughput_per_week_hundredths": 200,
  "unusable": []
}
```

## Authority

The operation is the owner's. It is not in the list of operations open to
sessions (`coordination-v0.md`, « Actors and authority »): a request whose
`actor` is `session:<id>` is refused `actor.owner_only` by the existing actor
gate, which is unchanged.

## `ws flow`

`ws flow` takes no option and prints the response on standard output.

| Exit code | Meaning |
| --- | --- |
| 0 | complete measure: `unusable` is empty |
| 1 | refusal, `ws: <code>` on standard error (the daemon's code, or `flow.response_invalid` when the response carries no `unusable` array) |
| 2 | usage error |
| 3 | at least one accepted mission could not be measured; the complete JSON is still printed on standard output |

Exit code 3 exists so that a caller cannot read a partial measure as a
complete one.

## Simulation

Every run of the fake agent is a simulation (`b-prime-rehearsal.md`). A measure taken
on missions executed by the fake agent does not count toward B′: `simulated`
says how many of the retained missions are such, and a measure where
`simulated` equals `retained` describes the test harness, not real work.
