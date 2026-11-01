use std::path::Path;

use super::*;

const H: i32 = 3600;

/// A TZif file: `version` (0, b'2', b'3'…), transitions (time, type), types' offsets, footer.
fn tzif(version: u8, transitions: &[(i64, u8)], offsets: &[i32], footer: &str) -> Vec<u8> {
    fn header(out: &mut Vec<u8>, version: u8, times: usize, types: usize) {
        out.extend_from_slice(b"TZif");
        out.push(version);
        out.extend_from_slice(&[0; 15]);
        // isut, isstd, leap, time, type, char
        for n in [types, types, 0, times, types, 4] {
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
    }
    fn block(out: &mut Vec<u8>, transitions: &[(i64, u8)], offsets: &[i32], wide: bool) {
        for (t, _) in transitions {
            if wide {
                out.extend_from_slice(&t.to_be_bytes());
            } else {
                out.extend_from_slice(&(*t as i32).to_be_bytes());
            }
        }
        out.extend(transitions.iter().map(|(_, k)| *k));
        for o in offsets {
            out.extend_from_slice(&o.to_be_bytes());
            out.extend_from_slice(&[0, 0]);
        }
        out.extend_from_slice(b"XYZ\0");
        out.extend(std::iter::repeat_n(0, offsets.len() * 2));
    }
    let mut out = Vec::new();
    let narrow: Vec<(i64, u8)> = transitions.iter().copied().filter(|(t, _)| i32::try_from(*t).is_ok()).collect();
    header(&mut out, version, narrow.len(), offsets.len());
    block(&mut out, &narrow, offsets, false);
    if version != 0 {
        header(&mut out, version, transitions.len(), offsets.len());
        block(&mut out, transitions, offsets, true);
        out.push(b'\n');
        out.extend_from_slice(footer.as_bytes());
        out.push(b'\n');
    }
    out
}

/// Seconds since the epoch of a UTC date and time.
fn utc(y: i64, mo: u32, d: u32, h: i64, mi: i64, s: i64) -> i64 {
    days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s
}

#[test]
fn civil_dates_round_trip() {
    for days in (-800_000..3_000_000).step_by(997) {
        let (y, m, d) = civil_from_days(days);
        assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
    }
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
    let c = Civil::from_unix(utc(2026, 10, 31, 12, 34, 56));
    assert_eq!((c.year, c.month, c.day, c.hour, c.minute, c.second), (2026, 10, 31, 12, 34, 56));
    assert_eq!(c.weekday(), 6, "Saturday");
    assert_eq!(Civil::from_unix(0).weekday(), 4, "Thursday");
    assert_eq!(Civil::from_unix(-1), Civil { year: 1969, month: 12, day: 31, hour: 23, minute: 59, second: 59 });
    // Hostile numbers are clamped, not overflowed.
    let _ = civil_from_days(i64::MAX);
    let _ = civil_from_days(i64::MIN);
    let _ = days_from_civil(i64::MAX, u32::MAX, u32::MAX);
    let _ = days_from_civil(i64::MIN, 0, 0);
    let _ = Civil::from_unix(i64::MIN);
    let _ = Civil::from_unix(i64::MAX);
}

#[test]
fn the_new_zealand_noon_of_the_issue_is_the_31st() {
    // #1707: at noon on 31 October in Auckland (UTC+13) it is 23:00 on the 30th in UTC.
    let t = utc(2026, 10, 30, 23, 0, 0);
    let c = Zone::fixed(13 * H).local(t);
    assert_eq!((c.year, c.month, c.day, c.hour), (2026, 10, 31, 12));
    let nz = Zone::from_posix("NZST-12NZDT,M9.5.0,M4.1.0/3").unwrap();
    assert_eq!(nz.offset_at(t), 13 * H);
    assert_eq!(nz.local(t).day, 31);
    // Just after midnight there, still the previous morning in UTC.
    let c = nz.local(utc(2026, 10, 30, 11, 0, 0));
    assert_eq!((c.month, c.day, c.hour), (10, 31, 0));
}

#[test]
fn posix_rules_change_at_the_right_instants() {
    // Reference instants and offsets from the IANA database (tzdata 2025).
    let cases: &[(&str, &[(i64, i32)])] = &[
        (
            "NZST-12NZDT,M9.5.0,M4.1.0/3",
            &[(1790431199, 12 * H), (1790431200, 13 * H), (1775311199, 13 * H), (1775311200, 12 * H), (1782864000, 12 * H), (1768435200, 13 * H)],
        ),
        ("EST5EDT,M3.2.0,M11.1.0", &[(1772953199, -5 * H), (1772953200, -4 * H), (1793512799, -4 * H), (1793512800, -5 * H), (4118083200, -4 * H)]),
        ("CET-1CEST,M3.5.0,M10.5.0/3", &[(1774745999, H), (1774746000, 2 * H), (1792889999, 2 * H), (1792890000, H), (1768514384, H)]),
        // Negative times of day (RFC 8536 §3.3.1).
        ("<-02>2<-01>,M3.5.0/-1,M10.5.0/0", &[(1774742399, -2 * H), (1774746000, -H), (1792886399, -H), (1792890000, -2 * H), (1782864000, -H)]),
        // Half an hour of daylight saving, quoted names, minutes in offsets and times.
        ("<+1030>-10:30<+11>-11,M10.1.0,M4.1.0", &[(1767225600, 11 * H), (1782864000, 10 * H + 1800)]),
        ("<+1245>-12:45<+1345>,M9.5.0/2:45,M4.1.0/3:45", &[(1767225600, 13 * H + 2700), (1782864000, 12 * H + 2700)]),
        // No daylight saving time.
        ("<-03>3", &[(1767225600, -3 * H), (1514764800, -3 * H)]),
        ("UTC0", &[(0, 0), (1782864000, 0)]),
        // A daylight saving time with no rule takes the United States' (as the C libraries do).
        ("EST5EDT", &[(1772953199, -5 * H), (1772953200, -4 * H)]),
        // All year round, written as a change at the start and one after the end of the year.
        ("EST5EDT,0/0,J365/25", &[(1767225600, -4 * H), (1782864000, -4 * H), (1798761599, -4 * H)]),
    ];
    for (tz, instants) in cases {
        let zone = Zone::from_posix(tz).unwrap_or_else(|e| panic!("{tz}: {e}"));
        for (t, want) in *instants {
            assert_eq!(zone.offset_at(*t), *want, "{tz} at {t}");
        }
    }
}

#[test]
fn posix_julian_dates_count_february_29_or_not() {
    // `Jn` never counts February 29 (J60 is always 1 March); `n` counts it (59 is 29 February in a leap year).
    let julian = Zone::from_posix("AAA0BBB,J60/0,J300/0").unwrap();
    assert_eq!(julian.offset_at(utc(2024, 2, 29, 23, 59, 59)), 0);
    assert_eq!(julian.offset_at(utc(2024, 3, 1, 0, 0, 0)), H);
    assert_eq!(julian.offset_at(utc(2026, 2, 28, 23, 59, 59)), 0);
    assert_eq!(julian.offset_at(utc(2026, 3, 1, 0, 0, 0)), H);
    let zero = Zone::from_posix("AAA0BBB,59/0,300/0").unwrap();
    assert_eq!(zero.offset_at(utc(2024, 2, 28, 23, 59, 59)), 0);
    assert_eq!(zero.offset_at(utc(2024, 2, 29, 0, 0, 0)), H);
    assert_eq!(zero.offset_at(utc(2026, 2, 28, 23, 59, 59)), 0);
    assert_eq!(zero.offset_at(utc(2026, 3, 1, 0, 0, 0)), H);
    // Week 5 is the last such day: the fourth Sunday of February 2026, the fifth of March.
    let last = Zone::from_posix("AAA0BBB,M2.5.0/0,M3.5.0/0").unwrap();
    assert_eq!(last.offset_at(utc(2026, 2, 21, 23, 59, 59)), 0);
    assert_eq!(last.offset_at(utc(2026, 2, 22, 0, 0, 0)), H);
    // It ends at midnight of daylight saving time, 23:00 the day before in UTC.
    assert_eq!(last.offset_at(utc(2026, 3, 28, 22, 59, 59)), H);
    assert_eq!(last.offset_at(utc(2026, 3, 28, 23, 0, 0)), 0);
}

#[test]
fn bad_posix_strings_are_errors() {
    let long = format!("{}5", "A".repeat(300));
    for tz in [
        "",
        "E",
        "ES5",
        "EST",
        "EST+",
        "EST5:",
        "EST5:60",
        "EST25",
        "EST5EDT,",
        "EST5EDT,M3.2.0",
        "EST5EDT,M3.2.0,",
        "EST5EDT,M13.1.0,M11.1.0",
        "EST5EDT,M3.6.0,M11.1.0",
        "EST5EDT,M3.2.7,M11.1.0",
        "EST5EDT,J0,J365",
        "EST5EDT,J366,J1",
        "EST5EDT,366,1",
        "EST5EDT,M3.2.0/168,M11.1.0",
        "EST5EDT,M3.2.0,M11.1.0x",
        "<EST5",
        "<AB>5",
        "<A B>5",
        "EST5EDT4x",
        "EST5EDT,M3.2,M11.1.0",
        "ÉST5",
        long.as_str(),
    ] {
        assert!(Zone::from_posix(tz).is_err(), "{tz:?}");
    }
}

#[test]
fn hostile_posix_strings_never_panic() {
    let alphabet = b"AZ<>+-0123456789:,./JM";
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    for _ in 0..20_000 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let len = (seed % 40) as usize;
        let s: String = (0..len).map(|k| alphabet[((seed >> (k % 60)) as usize + k * 7) % alphabet.len()] as char).collect();
        if let Ok(z) = Zone::from_posix(&s) {
            for t in [MIN_TIME, -1, 0, 1_790_431_200, MAX_TIME, i64::MIN, i64::MAX] {
                assert!(z.offset_at(t).abs() <= MAX_OFFSET, "{s}");
                let _ = z.local(t);
            }
        }
    }
    // The largest offsets a string can give stay in range.
    let z = Zone::from_posix("<-25>-24:59:59<-26>,M1.1.0/-167,M12.5.6/167").unwrap();
    for t in [MIN_TIME, 0, MAX_TIME] {
        assert!(z.offset_at(t).abs() <= MAX_OFFSET);
        let _ = z.local(t);
    }
}

#[test]
fn tzif_files_give_the_offset_in_force() {
    // 1990 to 2000 at +1, then +2 until 2010, then the footer's rule.
    let t1 = utc(1990, 1, 1, 0, 0, 0);
    let t2 = utc(2000, 1, 1, 0, 0, 0);
    let t3 = utc(2010, 1, 1, 0, 0, 0);
    for version in *b"234" {
        let z = Zone::from_tzif(&tzif(version, &[(t1, 1), (t2, 2), (t3, 1)], &[0, H, 2 * H], "CET-1CEST,M3.5.0,M10.5.0/3")).unwrap();
        // Before the first transition: type 0 (RFC 8536 §3.2).
        assert_eq!(z.offset_at(t1 - 1), 0);
        assert_eq!(z.offset_at(t1), H);
        assert_eq!(z.offset_at(t2 - 1), H);
        assert_eq!(z.offset_at(t2), 2 * H);
        // After the last: the footer, so summer and winter differ.
        assert_eq!(z.offset_at(1768514384), H);
        assert_eq!(z.offset_at(1784116800), 2 * H);
    }
    // Version 1: no footer, the last transition's type lasts.
    let v1 = Zone::from_tzif(&tzif(0, &[(t1, 1), (t2, 2)], &[0, H, 2 * H], "")).unwrap();
    assert_eq!(v1.offset_at(t1 - 1), 0);
    assert_eq!(v1.offset_at(1784116800), 2 * H);
    // A "slim" file: no transitions at all, everything in the footer.
    let slim = Zone::from_tzif(&tzif(b'2', &[], &[12 * H], "NZST-12NZDT,M9.5.0,M4.1.0/3")).unwrap();
    assert_eq!(slim.offset_at(1793401200), 13 * H);
    assert_eq!(slim.offset_at(1782864000), 12 * H);
    // An empty footer: the last type lasts. An unreadable one is ignored the same way.
    for footer in ["", "not a rule"] {
        let z = Zone::from_tzif(&tzif(b'2', &[(t1, 1)], &[0, -5 * H], footer)).unwrap();
        assert_eq!(z.offset_at(1784116800), -5 * H, "{footer:?}");
    }
    // 64-bit times beyond 2038 are read from the second block.
    let far = utc(2100, 1, 1, 0, 0, 0);
    let z = Zone::from_tzif(&tzif(b'2', &[(t1, 1), (far, 0)], &[0, H], "")).unwrap();
    assert_eq!(z.offset_at(far - 1), H);
    assert_eq!(z.offset_at(far), 0);
}

#[test]
fn bad_tzif_files_are_errors_and_never_panic() {
    let good = tzif(b'2', &[(0, 1), (1_000_000_000, 0)], &[0, H], "CET-1CEST,M3.5.0,M10.5.0/3");
    assert!(Zone::from_tzif(&good).is_ok());
    // Every truncation of the data is an error (the footer's end may go: it is read leniently).
    let footer_at = good.len() - "CET-1CEST,M3.5.0,M10.5.0/3\n".len() - 1;
    for n in 0..footer_at {
        assert!(Zone::from_tzif(&good[..n]).is_err(), "truncated at {n}");
    }
    for n in footer_at..good.len() {
        let _ = Zone::from_tzif(&good[..n]);
    }
    // Every single-byte corruption: an error or a zone, never a panic, offsets in range.
    for at in 0..good.len() {
        for v in [0x00, 0x01, 0x7f, 0x80, 0xff] {
            let mut b = good.clone();
            b[at] = v;
            if let Ok(z) = Zone::from_tzif(&b) {
                for t in [i64::MIN, MIN_TIME, -1, 0, 999_999_999, 1_000_000_000, MAX_TIME, i64::MAX] {
                    assert!(z.offset_at(t).abs() <= MAX_OFFSET);
                    let _ = z.local(t);
                }
            }
        }
    }
    let with = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut b = good.clone();
        f(&mut b);
        Zone::from_tzif(&b)
    };
    // Not TZif, an unknown version.
    assert!(with(&|b| b[0] = b'X').is_err());
    assert!(with(&|b| b[4] = b'1').is_err());
    // Huge counts.
    assert!(with(&|b| b[32..36].copy_from_slice(&u32::MAX.to_be_bytes())).is_err());
    assert!(with(&|b| b[36..40].copy_from_slice(&u32::MAX.to_be_bytes())).is_err());
    // No types; a transition to a type that isn't there; transitions out of order; an offset out of range.
    assert!(Zone::from_tzif(&tzif(b'2', &[], &[], "")).is_err());
    assert!(Zone::from_tzif(&tzif(b'2', &[(0, 2)], &[0, H], "")).is_err());
    assert!(Zone::from_tzif(&tzif(b'2', &[(10, 0), (5, 1)], &[0, H], "")).is_err());
    assert!(Zone::from_tzif(&tzif(b'2', &[], &[100 * H], "")).is_err());
    assert!(Zone::from_tzif(&tzif(b'2', &[], &[i32::MIN], "")).is_err());
    // Too large.
    let mut big = good.clone();
    big.resize(tzif::MAX_LEN + 1, b'\n');
    assert!(Zone::from_tzif(&big).is_err());
    // Random bytes behind a valid header.
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..2000 {
        let mut b = good[..44].to_vec();
        for _ in 0..(seed % 300) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            b.push(seed as u8);
        }
        let _ = Zone::from_tzif(&b).map(|z| z.local(0));
    }
}

#[test]
fn the_system_zone_follows_tz_as_the_c_library_does() {
    let auckland = tzif(b'2', &[], &[12 * H], "NZST-12NZDT,M9.5.0,M4.1.0/3");
    let berlin = tzif(b'2', &[], &[H], "CET-1CEST,M3.5.0,M10.5.0/3");
    let files = |path: &Path| -> Option<Vec<u8>> {
        match path.to_str()? {
            "/etc/localtime" => Some(berlin.clone()),
            // Any path to it, as a file system would resolve `..` (the names that climb out of the
            // zone directory must be refused before they get here).
            p if p.ends_with("/Pacific/Auckland") || p == "/opt/auckland" => Some(auckland.clone()),
            "/usr/share/zoneinfo/Broken" => Some(b"TZif2 not really".to_vec()),
            _ => None,
        }
    };
    let summer_in_nz = 1793401200;
    let zone = |tz: Option<&str>, tzdir: Option<&str>| {
        system_from(
            |name| match name {
                "TZ" => tz.map(str::to_string),
                "TZDIR" => tzdir.map(str::to_string),
                _ => None,
            },
            files,
        )
    };
    let offset = |tz: Option<&str>, tzdir: Option<&str>| zone(tz, tzdir).map(|z| z.offset_at(summer_in_nz));
    // Not set: /etc/localtime. Empty: UTC.
    assert_eq!(offset(None, None), Ok(H));
    assert_eq!(offset(Some(""), None), Ok(0));
    assert_eq!(offset(Some(":"), None), Ok(H));
    // A zone name, with or without the colon, under the zone directory or $TZDIR; an absolute path.
    assert_eq!(offset(Some("Pacific/Auckland"), None), Ok(13 * H));
    assert_eq!(offset(Some(":Pacific/Auckland"), None), Ok(13 * H));
    assert_eq!(offset(Some("Pacific/Auckland"), Some("/zones")), Ok(13 * H));
    assert_eq!(offset(Some(":/opt/auckland"), None), Ok(13 * H));
    // A relative $TZDIR is ignored, and names can't climb out of the directory.
    assert_eq!(offset(Some(":Pacific/Auckland"), Some("zones")), Ok(13 * H));
    assert!(zone(Some(":../share/zoneinfo/Pacific/Auckland"), None).is_err());
    assert!(zone(Some(":Pacific//Auckland"), None).is_err());
    // A POSIX string when no file has the name, but not after a colon.
    assert_eq!(offset(Some("NZST-12NZDT,M9.5.0,M4.1.0/3"), None), Ok(13 * H));
    assert!(zone(Some(":NZST-12NZDT,M9.5.0,M4.1.0/3"), None).is_err());
    // Unknown names and broken files are errors (the caller falls back to UTC).
    assert!(zone(Some("Nowhere/Special"), None).is_err());
    assert!(zone(Some("Broken"), None).is_err());
    assert!(system_from(|_| None, |_| None).is_err());
}

#[test]
fn the_system_zone_files_read_like_the_iana_database_says() {
    // Real zone files when the machine has them (most Unix systems do).
    let dir = Path::new("/usr/share/zoneinfo");
    if !dir.join("Pacific/Auckland").is_file() {
        eprintln!("no zone files here; skipped");
        return;
    }
    let read = |name: &str| Zone::from_tzif(&std::fs::read(dir.join(name)).unwrap()).unwrap();
    let cases: &[(&str, &[(i64, i32)])] = &[
        ("Pacific/Auckland", &[(1793401200, 13 * H), (1782864000, 12 * H), (1790431199, 12 * H), (1790431200, 13 * H)]),
        // Local mean time before the first transition.
        ("America/New_York", &[(-5364662400, -17762), (-618105600, -4 * H), (1772953200, -4 * H), (4118083200, -4 * H)]),
        ("Europe/Berlin", &[(-2208988800, H), (1768514384, H), (1784116800, 2 * H)]),
        ("America/Nuuk", &[(1774742399, -2 * H), (1774746000, -H)]),
        ("Pacific/Chatham", &[(1767225600, 13 * H + 2700)]),
        ("America/Sao_Paulo", &[(1767225600, -3 * H), (1514764800, -2 * H)]),
    ];
    for (name, instants) in cases {
        let z = read(name);
        for (t, want) in *instants {
            assert_eq!(z.offset_at(*t), *want, "{name} at {t}");
        }
    }
    // Every zone file there reads.
    let mut stack = vec![dir.to_path_buf()];
    let mut read_files = 0;
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(b) = std::fs::read(&p)
                && b.starts_with(b"TZif")
            {
                assert!(Zone::from_tzif(&b).is_ok(), "{}", p.display());
                read_files += 1;
            }
        }
    }
    assert!(read_files > 100, "{read_files}");
}
