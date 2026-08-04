//! GitHub timestamps (`2026-08-04T14:32:41Z`, optionally fractional) as
//! epoch seconds, without pulling a date dependency into the workspace.

/// Parses GitHub's fixed UTC layout to epoch seconds; anything else is
/// `None`, never a panic.
#[must_use]
pub fn parse_iso8601(text: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let digits = text.get(range)?;
        if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    };
    let separator = |index: usize, expected: u8| text.as_bytes().get(index) == Some(&expected);
    if !(separator(4, b'-')
        && separator(7, b'-')
        && separator(10, b'T')
        && separator(13, b':')
        && separator(16, b':'))
    {
        return None;
    }
    let tail = text.get(19..)?;
    let fractional_z = tail.len() > 2
        && tail.starts_with('.')
        && tail.ends_with('Z')
        && tail[1..tail.len() - 1]
            .bytes()
            .all(|byte| byte.is_ascii_digit());
    if !(tail == "Z" || fractional_z) {
        return None;
    }
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !((1..=12).contains(&month)
        && (1..=31).contains(&day)
        && (0..=23).contains(&hour)
        && (0..=59).contains(&minute)
        && (0..=59).contains(&second))
    {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Days since 1970-01-01 for a proleptic Gregorian date
/// (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let adjusted_year = if month <= 2 { year - 1 } else { year };
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let month_index = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::parse_iso8601;

    #[test]
    fn github_timestamps_parse_to_epoch_seconds() {
        // date -u -d "2026-08-04T14:32:41Z" +%s
        assert_eq!(parse_iso8601("2026-08-04T14:32:41Z"), Some(1_785_853_961));
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(
            parse_iso8601("2026-08-04T14:32:41.1234567Z"),
            Some(1_785_853_961),
            "log-line fractions are ignored"
        );
    }

    #[test]
    fn foreign_shapes_are_none_not_panics() {
        for bad in [
            "",
            "not a date",
            "2026-08-04",
            "2026-08-04 14:32:41",
            "2026-13-04T14:32:41Z",
            "2026-08-04T25:32:41Z",
            "2026-08-04T14:32:41+02:00",
        ] {
            assert_eq!(parse_iso8601(bad), None, "{bad:?}");
        }
    }
}
