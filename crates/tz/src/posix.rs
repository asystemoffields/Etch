//! POSIX TZ strings (POSIX.1-2017 §8.3, with the extensions of RFC 8536 §3.3.1): the `TZ`
//! environment variable and the footer of a TZif file.
//!
//! `std offset [dst [offset] [,start[/time],end[/time]]]`: names of three or more letters, or
//! quoted in `<…>`; offsets `[+|-]hh[:mm[:ss]]` west of UTC (so `CET-1` is an hour east); dates
//! `Jn` (1–365, February 29 never counted), `n` (0–365) or `Mm.w.d` (day `d` of week `w` of month
//! `m`, week 5 the last); times of day in local time, up to ±167 hours, 02:00 when not given.

use crate::Error;
use crate::civil::{Civil, days_from_civil, is_leap, month_len, weekday};

/// The longest TZ string read: real ones are under 60 bytes.
const MAX_LEN: usize = 256;

/// The time of day a change of offset happens when the string gives none: 02:00.
const DEFAULT_TIME: i32 = 2 * 3600;

const BAD: Error = Error("not a POSIX TZ string");

/// A zone's rule: standard time, and daylight saving time when the zone has it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Rule {
    /// Standard time's offset, seconds east of UTC.
    std: i32,
    dst: Option<Dst>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Dst {
    /// Daylight saving time's offset, seconds east of UTC.
    offset: i32,
    /// When it starts, in local standard time.
    start: Change,
    /// When it ends, in local daylight saving time.
    end: Change,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Change {
    date: Date,
    /// Seconds after the date's midnight (may be negative, or more than a day).
    time: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Date {
    /// `Jn`: day 1–365 of the year, February 29 never counted.
    Julian(u32),
    /// `n`: day 0–365 of the year, February 29 counted.
    Zero(u32),
    /// `Mm.w.d`: day `day` (0 Sunday) of week `week` (1–5, 5 the last) of `month` (1–12).
    Month { month: u32, week: u32, day: u32 },
}

struct Cursor<'a> {
    s: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.at).copied()
    }

    fn eat(&mut self, c: u8) -> bool {
        let hit = self.peek() == Some(c);
        if hit {
            self.at += 1;
        }
        hit
    }

    fn done(&self) -> bool {
        self.at >= self.s.len()
    }

    /// A decimal number of 1 to `digits` digits.
    fn number(&mut self, digits: usize) -> Result<u32, Error> {
        let from = self.at;
        let mut n: u32 = 0;
        while self.at - from < digits
            && let Some(d) = self.peek().filter(u8::is_ascii_digit)
        {
            n = n * 10 + u32::from(d - b'0');
            self.at += 1;
        }
        if self.at == from { Err(BAD) } else { Ok(n) }
    }
}

/// Parse a POSIX TZ string.
pub(crate) fn parse(tz: &str) -> Result<Rule, Error> {
    if tz.len() > MAX_LEN {
        return Err(Error("TZ string too long"));
    }
    let mut c = Cursor { s: tz.as_bytes(), at: 0 };
    name(&mut c)?;
    // TZ offsets count west of UTC.
    let std = -offset(&mut c, 24)?;
    if c.done() {
        return Ok(Rule { std, dst: None });
    }
    name(&mut c)?;
    let dst = if matches!(c.peek(), Some(b'+' | b'-' | b'0'..=b'9')) { -offset(&mut c, 24)? } else { std + 3600 };
    let (start, end) = if c.done() {
        // A daylight saving time with no rule: the United States' rule since 2007, which the
        // reference implementation and the GNU C library assume too.
        let at = |month, week| Change { date: Date::Month { month, week, day: 0 }, time: DEFAULT_TIME };
        (at(3, 2), at(11, 1))
    } else {
        if !c.eat(b',') {
            return Err(BAD);
        }
        let start = change(&mut c)?;
        if !c.eat(b',') {
            return Err(BAD);
        }
        (start, change(&mut c)?)
    };
    if !c.done() {
        return Err(BAD);
    }
    Ok(Rule { std, dst: Some(Dst { offset: dst, start, end }) })
}

/// A zone abbreviation: three or more letters, or any letters, digits, `+` and `-` in `<…>`.
fn name(c: &mut Cursor) -> Result<(), Error> {
    let quoted = c.eat(b'<');
    let from = c.at;
    while let Some(b) = c.peek() {
        let ok = if quoted { b.is_ascii_alphanumeric() || b == b'+' || b == b'-' } else { b.is_ascii_alphabetic() };
        if !ok {
            break;
        }
        c.at += 1;
    }
    let len = c.at - from;
    if quoted && !c.eat(b'>') {
        return Err(BAD);
    }
    if len < 3 { Err(BAD) } else { Ok(()) }
}

/// `[+|-]hh[:mm[:ss]]` in seconds, the hours at most `max_hours`.
fn offset(c: &mut Cursor, max_hours: u32) -> Result<i32, Error> {
    let negative = c.eat(b'-');
    if !negative {
        c.eat(b'+');
    }
    let hours = c.number(3)?;
    if hours > max_hours {
        return Err(BAD);
    }
    let mut secs = hours * 3600;
    if c.eat(b':') {
        let minutes = c.number(2)?;
        if minutes > 59 {
            return Err(BAD);
        }
        secs += minutes * 60;
        if c.eat(b':') {
            let seconds = c.number(2)?;
            if seconds > 59 {
                return Err(BAD);
            }
            secs += seconds;
        }
    }
    let secs = i32::try_from(secs).map_err(|_| BAD)?;
    Ok(if negative { -secs } else { secs })
}

/// A change of offset: `date[/time]`.
fn change(c: &mut Cursor) -> Result<Change, Error> {
    let date = if c.eat(b'J') {
        let n = c.number(3)?;
        if !(1..=365).contains(&n) {
            return Err(BAD);
        }
        Date::Julian(n)
    } else if c.eat(b'M') {
        let month = c.number(2)?;
        let week = if c.eat(b'.') { c.number(1)? } else { return Err(BAD) };
        let day = if c.eat(b'.') { c.number(1)? } else { return Err(BAD) };
        if !(1..=12).contains(&month) || !(1..=5).contains(&week) || day > 6 {
            return Err(BAD);
        }
        Date::Month { month, week, day }
    } else {
        let n = c.number(3)?;
        if n > 365 {
            return Err(BAD);
        }
        Date::Zero(n)
    };
    // RFC 8536 §3.3.1: the time may be negative and up to 167 hours.
    let time = if c.eat(b'/') { offset(c, 167)? } else { DEFAULT_TIME };
    Ok(Change { date, time })
}

impl Date {
    /// Days since 1970-01-01 of this date in `year`.
    fn day(self, year: i64) -> i64 {
        let jan1 = days_from_civil(year, 1, 1);
        match self {
            Date::Julian(n) => jan1 + i64::from(n) - 1 + i64::from(is_leap(year) && n >= 60),
            Date::Zero(n) => jan1 + i64::from(n),
            Date::Month { month, week, day } => {
                let first = days_from_civil(year, month, 1);
                let mut d = (i64::from(day) - i64::from(weekday(first))).rem_euclid(7) + 7 * (i64::from(week) - 1);
                // Week 5 is the last such day, in the fourth week when the month has no fifth.
                while d >= i64::from(month_len(year, month)) {
                    d -= 7;
                }
                first + d
            }
        }
    }
}

impl Rule {
    /// The offset in force at `t` (seconds since 1970-01-01T00:00:00Z, years 1 to 9999).
    pub(crate) fn offset_at(&self, t: i64) -> i32 {
        let Some(dst) = self.dst else { return self.std };
        let year = Civil::from_unix(t + i64::from(self.std)).year;
        // Daylight saving time starts at a local standard time, and ends at a local daylight one.
        let start = dst.start.date.day(year) * 86_400 + i64::from(dst.start.time) - i64::from(self.std);
        let end = dst.end.date.day(year) * 86_400 + i64::from(dst.end.time) - i64::from(dst.offset);
        // South of the equator it starts late in the year and ends early in the next.
        let in_dst = if start <= end { start <= t && t < end } else { t < end || t >= start };
        if in_dst { dst.offset } else { self.std }
    }
}
