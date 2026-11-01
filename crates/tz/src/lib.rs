//! Local time (#1707): what a person reads as today's date and the time now is the civil time
//! of their zone at that instant, while timestamps written into files stay UTC.
//!
//! A [`Zone`] gives the offset from UTC in force at any instant ([`Zone::offset_at`]), so a date
//! recorded in winter and shown in summer is shown with the winter offset. It is a fixed offset,
//! the rules of a TZif zone file (RFC 8536) with its POSIX TZ footer, a POSIX TZ string
//! (POSIX.1-2017 §8.3, with RFC 8536's extensions), or the platform's own zone: the browser's on
//! the web, Windows' on Windows. [`system`] finds the zone the machine is set to.
//!
//! Zone files and `TZ` values are input like any other: sizes are capped, lengths checked and
//! arithmetic bounded; a zone that can't be read is an error, never a panic.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod civil;
mod posix;
mod system;
mod tzif;

use std::sync::Arc;

pub use civil::{Civil, civil_from_days, days_from_civil};
pub use system::{system, system_from};

/// The largest offset from UTC a zone may have, east or west, in seconds: 25:59:59, the most
/// RFC 8536 §3.2 allows (no zone has had more than 15 hours).
pub const MAX_OFFSET: i32 = 93_599;

/// The first instant local time is computed for, 0001-01-01T00:00:00Z: earlier ones count as it.
pub const MIN_TIME: i64 = -62_135_596_800;

/// The last instant local time is computed for, 9999-12-31T23:59:59Z: later ones count as it.
pub const MAX_TIME: i64 = 253_402_300_799;

/// Why a zone couldn't be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error(&'static str);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Error {}

/// A time zone: the offset from UTC in force at each instant.
#[derive(Clone, Debug, PartialEq)]
pub struct Zone(Kind);

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    /// The same offset at every instant (seconds east of UTC).
    Fixed(i32),
    /// A zone file's transitions, then its footer's rule.
    Tzif(Arc<tzif::Tzif>),
    /// A POSIX TZ string's rule.
    Posix(posix::Rule),
    /// The browser's zone.
    #[cfg(target_arch = "wasm32")]
    Browser,
    /// The zone Windows is set to.
    #[cfg(windows)]
    Windows,
}

impl Default for Zone {
    fn default() -> Self {
        Zone::UTC
    }
}

impl Zone {
    /// Coordinated Universal Time.
    pub const UTC: Zone = Zone(Kind::Fixed(0));

    /// A zone `offset` seconds east of UTC at every instant (clamped to ±[`MAX_OFFSET`]).
    pub fn fixed(offset: i32) -> Zone {
        Zone(Kind::Fixed(offset.clamp(-MAX_OFFSET, MAX_OFFSET)))
    }

    /// The zone a TZif file describes (RFC 8536, versions 1 to 4), such as `/etc/localtime`.
    pub fn from_tzif(bytes: &[u8]) -> Result<Zone, Error> {
        tzif::parse(bytes).map(|z| Zone(Kind::Tzif(Arc::new(z))))
    }

    /// The zone a POSIX TZ string describes: `UTC0`, `EST5EDT,M3.2.0,M11.1.0`,
    /// `<+1245>-12:45<+1345>,M9.5.0/2:45,M4.1.0/3:45`.
    pub fn from_posix(tz: &str) -> Result<Zone, Error> {
        posix::parse(tz).map(|r| Zone(Kind::Posix(r)))
    }

    /// The offset from UTC in force at `t` (seconds since 1970-01-01T00:00:00Z), in seconds east
    /// of UTC: what local civil time adds to UTC then.
    pub fn offset_at(&self, t: i64) -> i32 {
        let t = t.clamp(MIN_TIME, MAX_TIME);
        let offset = match &self.0 {
            Kind::Fixed(o) => *o,
            Kind::Tzif(z) => z.offset_at(t),
            Kind::Posix(r) => r.offset_at(t),
            #[cfg(target_arch = "wasm32")]
            Kind::Browser => browser_offset(t),
            #[cfg(windows)]
            Kind::Windows => windows_offset(t).unwrap_or(0),
        };
        offset.clamp(-MAX_OFFSET, MAX_OFFSET)
    }

    /// Local civil time at `t` (seconds since 1970-01-01T00:00:00Z), the date and time a clock
    /// on the wall shows then.
    pub fn local(&self, t: i64) -> Civil {
        let t = t.clamp(MIN_TIME, MAX_TIME);
        Civil::from_unix(t + i64::from(self.offset_at(t)))
    }
}

/// The browser's offset at `t`: `Date.getTimezoneOffset()` of that instant, which JavaScript
/// gives in minutes west of UTC.
#[cfg(target_arch = "wasm32")]
fn browser_offset(t: i64) -> i32 {
    let date = js_sys::Date::new_0();
    // Years 1 to 9999 in milliseconds are exact in a double and inside JavaScript's range.
    if !date.set_time(t as f64 * 1000.0).is_finite() {
        return 0;
    }
    let west = date.get_timezone_offset();
    let east = (-west * 60.0).round();
    if east.is_finite() && east.abs() <= f64::from(MAX_OFFSET) { east as i32 } else { 0 }
}

/// Windows' offset at `t`: the local date and time the Windows Runtime calendar (Gregorian, 24
/// hours) shows for that instant, less the instant. Windows applies its own zone and daylight
/// saving rules, historical ones too.
#[cfg(windows)]
fn windows_offset(t: i64) -> Option<i32> {
    use windows::Globalization::{Calendar, CalendarIdentifiers, ClockIdentifiers};
    // `DateTime` counts 100-nanosecond ticks from 1601-01-01T00:00:00Z.
    let ticks = t.checked_add(11_644_473_600)?.checked_mul(10_000_000)?;
    let c = Calendar::new().ok()?;
    c.ChangeCalendarSystem(&CalendarIdentifiers::Gregorian().ok()?).ok()?;
    c.ChangeClock(&ClockIdentifiers::TwentyFourHour().ok()?).ok()?;
    c.SetDateTime(windows::Foundation::DateTime { UniversalTime: ticks }).ok()?;
    let part = |v: windows::core::Result<i32>| v.ok().and_then(|v| u32::try_from(v).ok());
    let (year, month, day) = (i64::from(c.Year().ok()?), part(c.Month())?, part(c.Day())?);
    let (hour, minute, second) = (i64::from(part(c.Hour())?), i64::from(part(c.Minute())?), i64::from(part(c.Second())?));
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let local = days_from_civil(year, month, day).checked_mul(86_400)?.checked_add(hour * 3600 + minute * 60 + second)?;
    i32::try_from(local.checked_sub(t)?).ok().filter(|o| o.abs() <= MAX_OFFSET)
}

#[cfg(test)]
mod tests;
