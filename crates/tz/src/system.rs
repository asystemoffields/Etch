//! The zone the machine is set to.

use std::path::{Path, PathBuf};

use crate::{Error, Zone};

/// Where Unix systems keep their zone files (after `$TZDIR`): Linux, macOS and the BSDs use the
/// first; the others are older layouts.
const ZONE_DIRS: &[&str] = &["/usr/share/zoneinfo", "/usr/lib/zoneinfo", "/usr/share/lib/zoneinfo"];

/// The zone file of the zone a Unix system is set to.
const LOCALTIME: &str = "/etc/localtime";

/// The zone the machine is set to: the browser's on the web, Windows' on Windows, and elsewhere
/// the one `TZ` or `/etc/localtime` gives ([`system_from`]). UTC when it can't be read.
pub fn system() -> Zone {
    #[cfg(target_arch = "wasm32")]
    {
        Zone(crate::Kind::Browser)
    }
    #[cfg(windows)]
    {
        Zone(crate::Kind::Windows)
    }
    #[cfg(not(any(target_arch = "wasm32", windows)))]
    {
        system_from(|name| std::env::var(name).ok(), read_zone_file).unwrap_or(Zone::UTC)
    }
}

/// The zone a Unix system is set to, as its C library reads it, with the environment (`var`)
/// and the zone files (`read`: a file's bytes, `None` when it can't be read) given:
///
/// - `TZ` not set: the zone file `/etc/localtime`.
/// - `TZ` empty: UTC.
/// - `TZ=:name` or `TZ=name`: the zone file `name`, an absolute path or a name such as
///   `Pacific/Auckland` under `$TZDIR` or the system's zone directory; without the colon, when
///   there is no such file, a POSIX TZ string such as `NZST-12NZDT,M9.5.0,M4.1.0/3`.
pub fn system_from(var: impl Fn(&str) -> Option<String>, read: impl Fn(&Path) -> Option<Vec<u8>>) -> Result<Zone, Error> {
    let file = |path: &Path| read(path).ok_or(Error("zone file not readable")).and_then(|b| Zone::from_tzif(&b));
    let Some(tz) = var("TZ") else { return file(Path::new(LOCALTIME)) };
    if tz.is_empty() {
        return Ok(Zone::UTC);
    }
    let (only_file, name) = match tz.strip_prefix(':') {
        Some(name) => (true, name),
        None => (false, tz.as_str()),
    };
    if name.is_empty() {
        return file(Path::new(LOCALTIME));
    }
    let mut err = Error("TZ names no zone");
    for path in zone_paths(name, var("TZDIR")) {
        match file(&path) {
            Ok(zone) => return Ok(zone),
            Err(e) => err = e,
        }
    }
    if !only_file && let Ok(zone) = Zone::from_posix(name) {
        return Ok(zone);
    }
    Err(err)
}

/// The files `TZ=name` may name: `name` itself when it is an absolute path, else `name` under
/// `tzdir` and the system's zone directories, for a name that stays inside them (no `..`).
fn zone_paths(name: &str, tzdir: Option<String>) -> Vec<PathBuf> {
    if name.starts_with('/') {
        return vec![PathBuf::from(name)];
    }
    if name.len() > 255 || name.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
        return Vec::new();
    }
    let dirs = tzdir.filter(|d| d.starts_with('/')).into_iter().chain(ZONE_DIRS.iter().map(|d| d.to_string()));
    dirs.map(|d| Path::new(&d).join(name)).collect()
}

/// A zone file's bytes, for [`system_from`]: only a regular file (a pipe or a device could
/// block or never end), and only up to the size a zone file can have.
#[cfg(not(any(target_arch = "wasm32", windows)))]
fn read_zone_file(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let max = crate::tzif::MAX_LEN;
    let mut out = Vec::new();
    std::fs::File::open(path).ok()?.take(u64::try_from(max).ok()? + 1).read_to_end(&mut out).ok()?;
    (out.len() <= max).then_some(out)
}
