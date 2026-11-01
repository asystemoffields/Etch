//! Civil (proleptic Gregorian) dates and times from seconds since the Unix epoch.

/// A date and time of day in the proleptic Gregorian calendar, as a clock on the wall shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    /// 1–12.
    pub month: u32,
    /// 1–31.
    pub day: u32,
    /// 0–23.
    pub hour: u32,
    /// 0–59.
    pub minute: u32,
    /// 0–59.
    pub second: u32,
}

impl Civil {
    /// The civil date and time `t` seconds after 1970-01-01T00:00:00 (before it when negative).
    pub fn from_unix(t: i64) -> Civil {
        let (year, month, day) = civil_from_days(t.div_euclid(86_400));
        let secs = t.rem_euclid(86_400);
        let part = |v: i64| u32::try_from(v).unwrap_or(0);
        Civil { year, month, day, hour: part(secs / 3600), minute: part(secs / 60 % 60), second: part(secs % 60) }
    }

    /// Days since 1970-01-01 of this date.
    pub fn days(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    /// The day of the week: 0 for Sunday to 6 for Saturday.
    pub fn weekday(&self) -> u32 {
        weekday(self.days())
    }
}

/// The furthest from 1970 [`civil_from_days`] and [`days_from_civil`] go, in days (about 2.7
/// million years): further ones count as this, so the arithmetic can't overflow.
const MAX_DAYS: i64 = 1_000_000_000;

/// Days since 1970-01-01 → (year, month, day) (Howard Hinnant's `civil_from_days`).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days.clamp(-MAX_DAYS, MAX_DAYS) + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// (year, month, day) → days since 1970-01-01 (Howard Hinnant's `days_from_civil`). A month
/// outside 1–12 counts as the nearest; a day past the month's end runs on into the next.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let max_year = MAX_DAYS / 366;
    let m = i64::from(month.clamp(1, 12));
    let y = year.clamp(-max_year, max_year) - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + i64::from(day.min(1_000_000)) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The day of the week of `days` since 1970-01-01 (a Thursday): 0 for Sunday to 6 for Saturday.
pub(crate) fn weekday(days: i64) -> u32 {
    u32::try_from((days.rem_euclid(7) + 4) % 7).unwrap_or(0)
}

/// Is `year` a leap year in the Gregorian calendar?
pub(crate) fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The number of days in `month` (1–12) of `year`.
pub(crate) fn month_len(year: i64, month: u32) -> u32 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
