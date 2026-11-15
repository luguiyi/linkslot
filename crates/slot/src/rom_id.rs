//! SHA-256 content identity for ROM files.
//!
//! This is the same digest the linkSlot CLI uses for cloud units: lower-case
//! hex over the full ROM bytes.  The local link protocol uses the 32-byte
//! digest directly; SQLite and TSV files use the hex form.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use sha2::Sha256;

const BUFFER: usize = 1024 * 1024;

/// Raw SHA-256 of a ROM, used on the cable wire.
pub type Digest = [u8; 32];

pub fn sha256_hex(bytes: &[u8]) -> String {
    digest_hex(&digest_bytes(bytes))
}

pub fn digest_bytes(bytes: &[u8]) -> Digest {
    use sha2::Digest as _;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

pub fn digest_file(path: &Path) -> std::io::Result<Digest> {
    let file = File::open(path)?;
    digest_reader(file)
}

pub fn digest_reader<R: Read>(mut reader: R) -> std::io::Result<Digest> {
    use sha2::Digest as _;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; BUFFER];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    Ok(digest_hex(&digest_file(path)?))
}

pub fn sha256_reader<R: Read>(reader: R) -> std::io::Result<String> {
    Ok(digest_hex(&digest_reader(reader)?))
}

pub fn digest_hex(digest: &Digest) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub fn digest_from_hex(value: &str) -> Option<Digest> {
    if value.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = hex(pair[0])?;
        let low = hex(pair[1])?;
        out[index] = high * 16 + low;
    }
    Some(out)
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

pub fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_standard_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn digest_hex_round_trips() {
        let digest = digest_bytes(b"abc");
        assert_eq!(digest_from_hex(&digest_hex(&digest)), Some(digest));
    }
}
