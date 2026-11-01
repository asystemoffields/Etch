//! TZif zone files (RFC 8536): `/etc/localtime` and the files under `/usr/share/zoneinfo`.
//!
//! A version 1 file has a header and a data block with 32-bit times. Versions 2 to 4 follow it
//! with a second header and data block with 64-bit times, then a footer: a POSIX TZ string for
//! the instants after the last transition. Only the transitions and the local time types' offsets
//! matter here; abbreviations, leap seconds and the standard/wall and UT/local indicators are
//! checked for size and skipped.

use crate::{Error, MAX_OFFSET, posix};

/// The largest zone file read: real ones are under 4 KiB.
pub const MAX_LEN: usize = 1 << 20;

/// A header's size: magic, version, 15 reserved bytes, six counts.
const HEADER: usize = 44;

const TRUNCATED: Error = Error("TZif file truncated");

/// A zone file's rules.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tzif {
    /// When the offset changes, seconds since the epoch, ascending.
    times: Vec<i64>,
    /// The local time type each change starts (an index into `offsets`).
    types: Vec<u8>,
    /// Each local time type's offset, seconds east of UTC.
    offsets: Vec<i32>,
    /// The rule after the last change.
    footer: Option<posix::Rule>,
}

/// A header's counts.
struct Counts {
    isut: usize,
    isstd: usize,
    leap: usize,
    time: usize,
    types: usize,
    chars: usize,
}

impl Counts {
    /// The size of the data block that follows the header, with `time_size`-byte times.
    fn block_len(&self, time_size: usize) -> Option<usize> {
        self.time
            .checked_mul(time_size + 1)?
            .checked_add(self.types.checked_mul(6)?)?
            .checked_add(self.chars)?
            .checked_add(self.leap.checked_mul(time_size + 4)?)?
            .checked_add(self.isstd)?
            .checked_add(self.isut)
    }
}

fn be32(b: &[u8], at: usize) -> Option<[u8; 4]> {
    b.get(at..at.checked_add(4)?)?.try_into().ok()
}

fn be64(b: &[u8], at: usize) -> Option<[u8; 8]> {
    b.get(at..at.checked_add(8)?)?.try_into().ok()
}

/// A header: the version byte and the counts.
fn header(b: &[u8]) -> Result<(u8, Counts), Error> {
    if b.get(0..4) != Some(b"TZif".as_slice()) {
        return Err(Error("not a TZif file"));
    }
    let version = *b.get(4).ok_or(TRUNCATED)?;
    // Version 1 is NUL; later versions are ASCII digits and read as version 2 is.
    if version != 0 && !(b'2'..=b'9').contains(&version) {
        return Err(Error("unknown TZif version"));
    }
    let count = |k: usize| be32(b, 20 + 4 * k).map(u32::from_be_bytes).and_then(|n| usize::try_from(n).ok()).ok_or(TRUNCATED);
    let counts = Counts { isut: count(0)?, isstd: count(1)?, leap: count(2)?, time: count(3)?, types: count(4)?, chars: count(5)? };
    Ok((version, counts))
}

/// Parse a TZif file.
pub(crate) fn parse(b: &[u8]) -> Result<Tzif, Error> {
    if b.len() > MAX_LEN {
        return Err(Error("TZif file too large"));
    }
    let (version, v1) = header(b)?;
    let v1_end = v1.block_len(4).and_then(|n| n.checked_add(HEADER)).ok_or(TRUNCATED)?;
    let (counts, data, time_size, rest) = if version == 0 {
        (v1, b.get(HEADER..v1_end).ok_or(TRUNCATED)?, 4, &[][..])
    } else {
        // The 64-bit data after the 32-bit data, which is only there for old readers.
        let second = b.get(v1_end..).ok_or(TRUNCATED)?;
        let (_, v2) = header(second)?;
        let end = v2.block_len(8).and_then(|n| n.checked_add(HEADER)).ok_or(TRUNCATED)?;
        (v2, second.get(HEADER..end).ok_or(TRUNCATED)?, 8, second.get(end..).ok_or(TRUNCATED)?)
    };
    // RFC 8536 §3.1: at least one local time type; the indicators are absent or one per type.
    if counts.types == 0 || counts.types > 256 {
        return Err(Error("TZif file without valid local time types"));
    }
    if (counts.isstd != 0 && counts.isstd != counts.types) || (counts.isut != 0 && counts.isut != counts.types) {
        return Err(Error("TZif indicator counts don't match its types"));
    }
    let time_at = |k: usize| -> Option<i64> {
        let at = k.checked_mul(time_size)?;
        if time_size == 4 { be32(data, at).map(|x| i64::from(i32::from_be_bytes(x))) } else { be64(data, at).map(i64::from_be_bytes) }
    };
    let times = (0..counts.time).map(time_at).collect::<Option<Vec<i64>>>().ok_or(TRUNCATED)?;
    let types_at = counts.time * time_size;
    let types = data.get(types_at..types_at + counts.time).ok_or(TRUNCATED)?.to_vec();
    let records_at = types_at + counts.time;
    let offsets = (0..counts.types).map(|k| be32(data, records_at + 6 * k).map(i32::from_be_bytes)).collect::<Option<Vec<i32>>>().ok_or(TRUNCATED)?;
    if offsets.iter().any(|o| o.unsigned_abs() > MAX_OFFSET.unsigned_abs()) {
        return Err(Error("TZif offset out of range"));
    }
    if types.iter().any(|t| usize::from(*t) >= counts.types) {
        return Err(Error("TZif transition to an unknown type"));
    }
    if times.windows(2).any(|w| matches!(w, [a, b] if a > b)) {
        return Err(Error("TZif transitions out of order"));
    }
    // The footer: a TZ string between newlines, empty when the last type lasts for ever. One that
    // can't be read leaves the last transition's type in force, as for a version 1 file.
    let footer = rest
        .strip_prefix(b"\n")
        .and_then(|r| r.split(|c| *c == b'\n').next())
        .and_then(|s| std::str::from_utf8(s).ok())
        .filter(|s| !s.is_empty())
        .and_then(|s| posix::parse(s).ok());
    Ok(Tzif { times, types, offsets, footer })
}

impl Tzif {
    /// The offset in force at `t` (seconds since the epoch).
    pub(crate) fn offset_at(&self, t: i64) -> i32 {
        // How many changes have happened by `t`.
        let n = self.times.partition_point(|x| *x <= t);
        if n == self.times.len()
            && let Some(rule) = &self.footer
        {
            return rule.offset_at(t);
        }
        // RFC 8536 §3.2: before the first transition, local time type 0.
        let ty = n.checked_sub(1).and_then(|k| self.types.get(k)).map_or(0, |t| usize::from(*t));
        self.offsets.get(ty).copied().unwrap_or(0)
    }
}
