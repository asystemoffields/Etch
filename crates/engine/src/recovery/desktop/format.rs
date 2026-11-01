//! The on-disk vocabulary: record ids, file names, hashes, size bounds and the checksum line
//! of a plain generation.
use super::{MAX_HEADER, SEALED_MAGIC};
use sha2::{Digest, Sha256};
use std::path::Path;

pub(super) fn id_ok(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
pub(super) fn new_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(hex(&bytes))
}
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub(super) fn unhex16(s: &str) -> Option<[u8; 16]> {
    let mut out = [0u8; 16];
    if s.len() != 32 {
        return None;
    }
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}
pub(super) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
/// The generation of `snapshot-N.json` (plain) or `snapshot-N.enc` (protected).
pub(super) fn generation(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?.strip_prefix("snapshot-")?;
    name.strip_suffix(".json").or_else(|| name.strip_suffix(".enc"))?.parse().ok()
}
pub(super) fn sealed(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "enc")
}
/// A side file's name: `part-` and 64 lowercase hex digits, `.bin` or `.enc`. Anything else (a
/// path, `..`) named by a hostile copy is refused.
pub(super) fn part_ok(name: &str) -> bool {
    name.strip_prefix("part-")
        .and_then(|n| n.strip_suffix(".bin").or_else(|| n.strip_suffix(".enc")))
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}
pub(super) fn pending_ok(name: &str) -> bool {
    name.strip_prefix("pending-").and_then(|n| n.strip_suffix(".tmp")).is_some_and(id_ok)
}
pub(super) fn part_file(hash: &[u8; 32], salt: Option<&[u8; 16]>) -> String {
    match salt {
        None => format!("part-{}.bin", hex(hash)),
        Some(salt) => {
            let mut h = Sha256::new();
            h.update(salt);
            h.update(hash);
            let named: [u8; 32] = h.finalize().into();
            format!("part-{}.enc", hex(&named))
        }
    }
}
/// At least the size of `len` bytes in the encrypted container (sectors, allocation tables,
/// padding, the encryption information).
pub(super) fn sealed_size(len: u64) -> u64 {
    len.saturating_add(len / 64).saturating_add(64 << 10)
}
/// The most a protected generation file may take whose copy is `main` bytes of JSON: the copy
/// encrypted, after the magic line and the header line.
pub(super) fn sealed_file_size(main: u64) -> u64 {
    sealed_size(main).saturating_add((SEALED_MAGIC.len() + MAX_HEADER + 1) as u64)
}
/// `sha256=<64 hex digits> length=<20 decimal digits>\n`: the checksum line of a plain
/// generation whose JSON is `json` ([`PLAIN_MAGIC`]).
pub(super) fn check_line(json: &[u8]) -> String {
    format!("sha256={} length={:020}\n", hex(&sha256(json)), json.len())
}
/// The SHA-256 (hex) and length a plain generation's checksum line gives; `None` when it isn't
/// one.
pub(super) fn parse_check_line(line: &[u8]) -> Option<(&str, u64)> {
    let line = std::str::from_utf8(line).ok()?.strip_suffix('\n')?;
    let (digest, len) = line.strip_prefix("sha256=")?.split_once(" length=")?;
    let digest_ok = digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let len_ok = len.len() == 20 && len.bytes().all(|b| b.is_ascii_digit());
    (digest_ok && len_ok).then_some(())?;
    Some((digest, len.parse().ok()?))
}
