//! The clock dates are read from (#1707).
//!
//! What a person sees is local civil time: Insert › Date & Time, the DATE and TIME fields,
//! CREATEDATE, SAVEDATE and PRINTDATE, a cover page's date — the date and time of their zone at
//! that instant, with the offset in force then (a document created in winter and shown in summer
//! shows its winter creation time). Timestamps written into files (document properties, tracked
//! changes, comments, versions) stay UTC, with `Z`.
//!
//! The engine doesn't ask the system for its zone: the app sets the process's local zone at start
//! ([`set_local_zone`] with [`system_zone`]); until then, and in tests, it is UTC. A session can
//! have a clock of its own (`Session::clock`): a fixed instant and zone, as tests use.

use std::sync::RwLock;

use wordcraft_tz::{Civil, Zone};

pub use wordcraft_tz::system as system_zone;

/// The zone local time is read in where a clock has none of its own.
static LOCAL_ZONE: RwLock<Zone> = RwLock::new(Zone::UTC);

/// Set the process's local zone: the app calls this at start with [`system_zone`].
pub fn set_local_zone(zone: Zone) {
    match LOCAL_ZONE.write() {
        Ok(mut z) => *z = zone,
        Err(poisoned) => *poisoned.into_inner() = zone,
    }
}

/// The process's local zone ([`set_local_zone`]; UTC until it is set).
pub fn local_zone() -> Zone {
    match LOCAL_ZONE.read() {
        Ok(z) => z.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// Where "now" and the local zone come from: by default the system clock and the process's
/// local zone; a fixed instant and zone for tests and reproducible runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clock {
    /// A fixed instant, in seconds since 1970-01-01T00:00:00Z; `None` reads the system clock.
    pub now: Option<i64>,
    /// The zone local time is read in; `None` is the process's ([`local_zone`]).
    pub zone: Option<Zone>,
}

impl Clock {
    /// A clock stopped at `now` (seconds since the epoch) in `zone`.
    pub fn fixed(now: i64, zone: Zone) -> Clock {
        Clock { now: Some(now), zone: Some(zone) }
    }

    /// Now, in seconds since 1970-01-01T00:00:00Z.
    pub fn now(&self) -> i64 {
        self.now.unwrap_or_else(|| i64::try_from(crate::cmd::now_unix()).unwrap_or(i64::MAX))
    }

    /// Now as a UTC timestamp, `2026-10-15T21:59:44Z`: what files record.
    pub fn now_iso(&self) -> String {
        crate::cmd::iso_from_unix_secs(u64::try_from(self.now()).unwrap_or(0))
    }

    /// The zone local time is read in.
    pub fn zone(&self) -> Zone {
        self.zone.clone().unwrap_or_else(local_zone)
    }

    /// Local civil time at `t` (seconds since the epoch), with the offset in force then.
    pub fn local(&self, t: i64) -> Civil {
        self.zone().local(t)
    }

    /// Local civil time now.
    pub fn local_now(&self) -> Civil {
        self.local(self.now())
    }

    /// The local date and time at `t`, formatted with a Word date picture (`M/d/yyyy`,
    /// `dddd, MMMM d, yyyy`, `h:mm am/pm`…).
    pub fn format(&self, picture: &str, t: i64) -> String {
        format_civil(picture, &self.local(t))
    }

    /// The local date and time now, formatted with a Word date picture.
    pub fn format_now(&self, picture: &str) -> String {
        self.format(picture, self.now())
    }

    /// A timestamp a document records (W3CDTF, the form of its properties: `2026-01-15T21:59:44Z`)
    /// formatted with a Word date picture: in local time when it gives its zone (`Z`, `+01:00`),
    /// as written when it gives none or only a date. `None` when it holds no date.
    pub fn format_recorded(&self, picture: &str, timestamp: &str) -> Option<String> {
        let (civil, offset) = parse_timestamp(timestamp)?;
        Some(match offset {
            Some(offset) => self.format(picture, civil.days() * 86_400 + civil_secs(&civil) - offset),
            None => format_civil(picture, &civil),
        })
    }
}

/// Word's all-zero date, formatted with a Word date picture: what PRINTDATE shows in a document
/// never printed (ECMA-376 Part 1 §17.16.5.49: `0/0/0000 0:00:00 AM` for the default picture).
/// Every part is 0, the hour too (`h` shows 12 at a real midnight).
pub fn format_never(picture: &str) -> String {
    crate::cmd::insert::format_date_parts_zero(picture)
}

fn civil_secs(c: &Civil) -> i64 {
    i64::from(c.hour) * 3600 + i64::from(c.minute) * 60 + i64::from(c.second)
}

/// `c` formatted with a Word date picture.
fn format_civil(picture: &str, c: &Civil) -> String {
    crate::cmd::insert::format_date_parts(picture, c.year, c.month as usize, c.day, (c.hour, c.minute, c.second), c.days())
}

/// A W3CDTF timestamp: the date and time as written, and its offset from UTC in seconds when it
/// gives one (`Z` is 0). `YYYY-MM-DD` at least; `THH:MM[:SS[.s…]]` and the zone are optional.
pub fn parse_timestamp(s: &str) -> Option<(Civil, Option<i64>)> {
    let num = |r: std::ops::Range<usize>| s.get(r).filter(|x| x.bytes().all(|b| b.is_ascii_digit())).and_then(|x| x.parse::<u32>().ok());
    let (year, month, day) = (i64::from(num(0..4)?), num(5..7)?, num(8..10)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut civil = Civil { year, month, day, hour: 0, minute: 0, second: 0 };
    if !matches!(s.get(10..11), Some("T" | "t" | " ")) {
        return Some((civil, None));
    }
    civil.hour = num(11..13).unwrap_or(0).min(23);
    civil.minute = num(14..16).unwrap_or(0).min(59);
    civil.second = num(17..19).unwrap_or(0).min(59);
    // The zone: after the time and any fraction of a second.
    let rest = s.get(16..).unwrap_or("");
    let offset = rest.find(['Z', 'z', '+', '-']).and_then(|k| {
        let zone = rest.get(k..)?;
        if zone.starts_with(['Z', 'z']) {
            return Some(0);
        }
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let digits: String = zone.chars().skip(1).filter(char::is_ascii_digit).take(4).collect();
        let hours = i64::from(digits.get(0..2)?.parse::<u32>().ok()?.min(23));
        let minutes = digits.get(2..4).and_then(|m| m.parse::<u32>().ok()).map_or(0, |m| i64::from(m.min(59)));
        Some(sign * (hours * 3600 + minutes * 60))
    });
    Some((civil, offset))
}
