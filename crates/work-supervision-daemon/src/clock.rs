use std::time::{SystemTime, UNIX_EPOCH};

use work_supervision_journal::{JournalError, Timestamp};

/// The current UTC instant as a journal timestamp (`YYYY-MM-DDTHH:MM:SS.mmmZ`).
///
/// A clock before 1970 is reported as the epoch: the journal orders by `seq`,
/// never by `at`, so a wrong clock cannot reorder anything.
///
/// # Errors
///
/// [`JournalError::TimestampInvalid`] for a year beyond 9999.
pub fn now() -> Result<Timestamp, JournalError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format(elapsed.as_secs(), elapsed.subsec_millis())
}

/// Formats seconds since the epoch and milliseconds.
pub(crate) fn format(seconds: u64, millis: u32) -> Result<Timestamp, JournalError> {
    let days =
        i64::try_from(seconds / 86_400).map_err(|_| JournalError::TimestampInvalid { line: 0 })?;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    Timestamp::parse(&format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rest / 3_600,
        (rest % 3_600) / 60,
        rest % 60
    ))
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to (year, month, day).
const fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::format;

    #[test]
    fn formats_known_instants() {
        assert_eq!(format(0, 0).unwrap().as_str(), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            format(951_782_400, 5).unwrap().as_str(),
            "2000-02-29T00:00:00.005Z"
        );
        assert_eq!(
            format(1_791_504_000 + 3_723, 999).unwrap().as_str(),
            "2026-10-09T01:02:03.999Z"
        );
    }
}
