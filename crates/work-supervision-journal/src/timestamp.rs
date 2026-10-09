use crate::JournalError;

/// A UTC instant written `YYYY-MM-DDTHH:MM:SS.mmmZ` (24 characters).
///
/// The journal records the instant its caller supplies; it does not read the
/// host clock and does not require instants to increase, because wall clocks
/// step backwards. Ordering is carried by `seq`, never by `at`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Timestamp(String);

impl Timestamp {
    /// Validates the exact format and that the date exists in the proleptic Gregorian calendar.
    ///
    /// # Errors
    ///
    /// [`JournalError::TimestampInvalid`] (line 0) for any other text.
    pub fn parse(text: &str) -> Result<Self, JournalError> {
        if is_valid(text.as_bytes()) {
            Ok(Self(text.to_owned()))
        } else {
            Err(JournalError::TimestampInvalid { line: 0 })
        }
    }

    /// The timestamp as written in the journal.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_valid(bytes: &[u8]) -> bool {
    let Ok(
        [
            y1,
            y2,
            y3,
            y4,
            b'-',
            m1,
            m2,
            b'-',
            d1,
            d2,
            b'T',
            h1,
            h2,
            b':',
            n1,
            n2,
            b':',
            s1,
            s2,
            b'.',
            f1,
            f2,
            f3,
            b'Z',
        ],
    ) = <[u8; 24]>::try_from(bytes)
    else {
        return false;
    };
    let Some(year) = number(&[y1, y2, y3, y4]) else {
        return false;
    };
    let (Some(month), Some(day), Some(hour), Some(minute), Some(second), Some(_)) = (
        number(&[m1, m2]),
        number(&[d1, d2]),
        number(&[h1, h2]),
        number(&[n1, n2]),
        number(&[s1, s2]),
        number(&[f1, f2, f3]),
    ) else {
        return false;
    };
    (1..=12).contains(&month)
        && day >= 1
        && day <= days_in_month(year, month)
        && hour <= 23
        && minute <= 59
        && second <= 59
}

fn number(digits: &[u8]) -> Option<u32> {
    digits.iter().try_fold(0_u32, |value, digit| {
        digit
            .is_ascii_digit()
            .then(|| value * 10 + u32::from(digit - b'0'))
    })
}

const fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
