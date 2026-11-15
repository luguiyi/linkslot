//! SHA-256 helpers and the aggregate hash used by `meta.json`.
//!
//! Aggregate rule: take every whitelisted file, sort by file name (byte
//! order), feed `name`, a NUL byte, the lowercase hex digest and a newline for
//! each file into SHA-256. The result is stored as `sha256:<hex>`.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{fs_error, ErrorKind, LinkError, Result};

pub const AGGREGATE_PREFIX: &str = "sha256:";

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    to_hex(&hasher.finalize())
}

/// Lowercase hex SHA-256 of a file, streamed (states are ~400 KiB each).
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).map_err(|err| fs_error(format!("无法读取 {}", path.display()), err))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|err| fs_error(format!("无法读取 {}", path.display()), err))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// Aggregate hash over `(name, sha256_hex)` pairs. Order of the input does not
/// matter; files are sorted by name internally.
pub fn aggregate_hash<'a, I>(files: I) -> String
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut entries: Vec<(&str, &str)> = files.into_iter().collect();
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut hasher = Sha256::new();
    for (name, digest) in entries {
        hasher.update(name.as_bytes());
        hasher.update([0u8]);
        hasher.update(digest.as_bytes());
        hasher.update(b"\n");
    }
    format!("{AGGREGATE_PREFIX}{}", to_hex(&hasher.finalize()))
}

/// Accept both `sha256:<hex>` (what `meta.json` uses for the aggregate) and a
/// bare hex digest (what individual `files[*].sha256` entries use).
pub fn normalize_digest(digest: &str) -> &str {
    digest.strip_prefix(AGGREGATE_PREFIX).unwrap_or(digest)
}

/// Compare two digests, ignoring case and the optional `sha256:` prefix.
pub fn digest_eq(a: &str, b: &str) -> bool {
    normalize_digest(a).eq_ignore_ascii_case(normalize_digest(b))
}

/// Validate that `value` looks like a hex SHA-256 digest (with or without the
/// `sha256:` prefix).
pub fn is_valid_digest(value: &str) -> bool {
    let hex = normalize_digest(value);
    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Verify an in-memory payload against a size + hex digest pair.
pub fn verify_bytes(bytes: &[u8], size: u64, sha256: &str) -> std::result::Result<(), LinkError> {
    if bytes.len() as u64 != size {
        return Err(LinkError::new(
            ErrorKind::Verify,
            format!("文件大小不符：期望 {size} 字节，实际 {} 字节", bytes.len()),
        ));
    }
    let actual = sha256_hex(bytes);
    if !digest_eq(&actual, sha256) {
        return Err(LinkError::new(
            ErrorKind::Verify,
            "文件 SHA-256 校验失败".to_string(),
        ));
    }
    Ok(())
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn aggregate_is_order_independent_and_sorted() {
        let a = aggregate_hash([("b.state", "11"), ("a.state", "22")]);
        let b = aggregate_hash([("a.state", "22"), ("b.state", "11")]);
        assert_eq!(a, b);
        assert!(a.starts_with("sha256:"));
        assert!(is_valid_digest(&a));
        assert_ne!(a, aggregate_hash([("a.state", "22")]));
    }

    #[test]
    fn aggregate_includes_names() {
        assert_ne!(
            aggregate_hash([("a.state", "11")]),
            aggregate_hash([("b.state", "11")])
        );
    }

    #[test]
    fn digest_helpers() {
        assert!(digest_eq("sha256:ABC", "abc"));
        assert!(digest_eq("abc", "ABC"));
        assert!(!digest_eq("abc", "abd"));
        assert!(is_valid_digest(&"0".repeat(64)));
        assert!(is_valid_digest(&format!("sha256:{}", "f".repeat(64))));
        assert!(!is_valid_digest("abc"));
        assert!(!is_valid_digest(&"z".repeat(64)));
    }

    #[test]
    fn verify_bytes_checks_size_and_digest() {
        let digest = sha256_hex(b"data");
        assert!(verify_bytes(b"data", 4, &digest).is_ok());
        assert!(verify_bytes(b"data", 5, &digest).is_err());
        assert!(verify_bytes(b"dat", 4, &digest).is_err());
        assert!(verify_bytes(b"data", 4, &"0".repeat(64)).is_err());
    }
}
