//! Small, bounded ZIP reader/writer used by the generic Lua `zip` API.
//!
//! The implementation deliberately understands only what a mod bridge needs:
//! stored/deflated files, UTF-8 names and the central directory. It never
//! touches the filesystem, so there is no extraction path to escape.

use std::io::{Read, Write};

use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;

pub const MAX_ZIP_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ZIP_ENTRIES: usize = 256;
pub const MAX_ZIP_ENTRY: usize = 32 * 1024 * 1024;
pub const MAX_ZIP_TOTAL: usize = 64 * 1024 * 1024;

const LOCAL_SIG: &[u8; 4] = b"PK\x03\x04";
const CENTRAL_SIG: &[u8; 4] = b"PK\x01\x02";
const EOCD_SIG: &[u8; 4] = b"PK\x05\x06";
const UTF8_FLAG: u16 = 0x0800;
const DEFLATE: u16 = 8;
const STORE: u16 = 0;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub size: usize,
    pub is_dir: bool,
    data: Vec<u8>,
}

impl Entry {
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

#[derive(Clone, Debug, Default)]
pub struct Archive {
    entries: Vec<Entry>,
}

impl Archive {
    pub fn parse(data: &[u8]) -> Result<Archive, String> {
        if data.len() > MAX_ZIP_BYTES {
            return Err(format!(
                "zip 超过 {} MiB 输入上限",
                MAX_ZIP_BYTES / (1024 * 1024)
            ));
        }
        if data.len() < 22 {
            return Err("不是完整的 zip 文件".to_string());
        }

        let eocd = find_eocd(data)?;
        let disk = u16_at(data, eocd + 4)?;
        let cd_disk = u16_at(data, eocd + 6)?;
        let entries_this_disk = u16_at(data, eocd + 8)? as usize;
        let total = u16_at(data, eocd + 10)? as usize;
        if disk != 0 || cd_disk != 0 || entries_this_disk != total {
            return Err("不支持分卷 zip".to_string());
        }
        if total > MAX_ZIP_ENTRIES {
            return Err(format!("zip 条目超过 {MAX_ZIP_ENTRIES} 个"));
        }
        let cd_size = u32_at(data, eocd + 12)? as usize;
        let cd_offset = u32_at(data, eocd + 16)? as usize;
        let cd_end = cd_offset
            .checked_add(cd_size)
            .ok_or_else(|| "zip 中央目录越界".to_string())?;
        if cd_end > eocd {
            return Err("zip 中央目录越界".to_string());
        }

        let mut entries = Vec::with_capacity(total);
        let mut pos = cd_offset;
        let mut total_uncompressed = 0usize;
        for _ in 0..total {
            if pos + 46 > cd_end {
                return Err("zip 中央目录损坏".to_string());
            }
            if &data[pos..pos + 4] != CENTRAL_SIG {
                return Err("zip 中央目录签名错误".to_string());
            }
            let flags = u16_at(data, pos + 8)?;
            let method = u16_at(data, pos + 10)?;
            let crc = u32_at(data, pos + 16)?;
            let compressed_size = u32_at(data, pos + 20)? as usize;
            let uncompressed_size = u32_at(data, pos + 24)? as usize;
            let name_len = u16_at(data, pos + 28)? as usize;
            let extra_len = u16_at(data, pos + 30)? as usize;
            let comment_len = u16_at(data, pos + 32)? as usize;
            let local_offset = u32_at(data, pos + 42)? as usize;

            if flags & 0x1 != 0 {
                return Err("不支持加密 zip".to_string());
            }
            if method != STORE && method != DEFLATE {
                return Err(format!("不支持的 zip 压缩方式 {method}"));
            }
            let name_end = pos + 46 + name_len;
            if name_end > cd_end {
                return Err("zip 条目名称越界".to_string());
            }
            let raw_name = &data[pos + 46..name_end];
            validate_name(raw_name)?;
            let name = String::from_utf8_lossy(raw_name).into_owned();
            let is_dir = raw_name.ends_with(b"/");
            if !is_dir {
                if uncompressed_size > MAX_ZIP_ENTRY {
                    return Err(format!(
                        "zip 单条解压超过 {} MiB 上限",
                        MAX_ZIP_ENTRY / (1024 * 1024)
                    ));
                }
                total_uncompressed = total_uncompressed
                    .checked_add(uncompressed_size)
                    .ok_or_else(|| "zip 解压总量越界".to_string())?;
                if total_uncompressed > MAX_ZIP_TOTAL {
                    return Err(format!(
                        "zip 解压总量超过 {} MiB 上限",
                        MAX_ZIP_TOTAL / (1024 * 1024)
                    ));
                }
            }

            let data_start = local_data_start(data, local_offset, compressed_size)?;
            let data_end = data_start
                .checked_add(compressed_size)
                .ok_or_else(|| "zip 数据越界".to_string())?;
            if data_end > data.len() {
                return Err("zip 数据越界".to_string());
            }

            let entry_data = if is_dir {
                if uncompressed_size != 0 || compressed_size != 0 {
                    return Err("zip 目录条目含有数据".to_string());
                }
                Vec::new()
            } else {
                let bytes = match method {
                    STORE => {
                        if compressed_size != uncompressed_size {
                            return Err("zip 存储条目大小不一致".to_string());
                        }
                        data[data_start..data_end].to_vec()
                    }
                    DEFLATE => {
                        let mut decoder = DeflateDecoder::new(&data[data_start..data_end]);
                        let mut bytes = Vec::new();
                        decoder
                            .by_ref()
                            .take(MAX_ZIP_ENTRY as u64 + 1)
                            .read_to_end(&mut bytes)
                            .map_err(|error| format!("zip 解压失败：{error}"))?;
                        if bytes.len() > MAX_ZIP_ENTRY {
                            return Err(format!(
                                "zip 单条解压超过 {} MiB 上限",
                                MAX_ZIP_ENTRY / (1024 * 1024)
                            ));
                        }
                        bytes
                    }
                    _ => unreachable!(),
                };
                if bytes.len() != uncompressed_size {
                    return Err("zip 条目解压大小不一致".to_string());
                }
                if crc32fast::hash(&bytes) != crc {
                    return Err(format!("zip 条目 {name} CRC 校验失败"));
                }
                bytes
            };

            entries.push(Entry {
                name,
                size: uncompressed_size,
                is_dir,
                data: entry_data,
            });
            pos = pos
                .checked_add(46 + name_len + extra_len + comment_len)
                .ok_or_else(|| "zip 中央目录越界".to_string())?;
        }
        if pos > cd_end {
            return Err("zip 中央目录越界".to_string());
        }
        Ok(Archive { entries })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn read_index(&self, index: usize) -> Option<&[u8]> {
        self.entries.get(index.checked_sub(1)?).map(Entry::data)
    }

    pub fn read_name(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(Entry::data)
    }
}

/// Build a deflated ZIP from file entries. Names are UTF-8 and no filesystem
/// paths are created here.
pub fn build(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    if entries.len() > MAX_ZIP_ENTRIES {
        return Err(format!("zip 条目超过 {MAX_ZIP_ENTRIES} 个"));
    }
    let mut total = 0usize;
    for (_, data) in entries {
        total = total
            .checked_add(data.len())
            .ok_or_else(|| "zip 内容总量越界".to_string())?;
        if data.len() > MAX_ZIP_ENTRY {
            return Err(format!(
                "zip 单条内容超过 {} MiB 上限",
                MAX_ZIP_ENTRY / (1024 * 1024)
            ));
        }
    }
    if total > MAX_ZIP_TOTAL {
        return Err(format!(
            "zip 内容总量超过 {} MiB 上限",
            MAX_ZIP_TOTAL / (1024 * 1024)
        ));
    }

    struct Central {
        name: Vec<u8>,
        crc: u32,
        compressed: usize,
        uncompressed: usize,
        offset: usize,
    }

    let mut out = Vec::new();
    let mut central = Vec::with_capacity(entries.len());
    for (name, data) in entries {
        let name_bytes = name.as_bytes().to_vec();
        validate_name(&name_bytes)?;
        if name_bytes.len() > u16::MAX as usize {
            return Err("zip 条目名称过长".to_string());
        }
        if name_bytes.ends_with(b"/") {
            return Err("zip.build 仅支持文件条目".to_string());
        }

        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(data)
            .map_err(|error| format!("zip 压缩失败：{error}"))?;
        let compressed = encoder
            .finish()
            .map_err(|error| format!("zip 压缩失败：{error}"))?;
        let crc = crc32fast::hash(data);
        let offset = out.len();
        let compressed_size =
            u32::try_from(compressed.len()).map_err(|_| "zip 压缩后过大".to_string())?;
        let uncompressed_size =
            u32::try_from(data.len()).map_err(|_| "zip 内容过大".to_string())?;

        out.extend_from_slice(LOCAL_SIG);
        push_u16(&mut out, 20);
        push_u16(&mut out, UTF8_FLAG);
        push_u16(&mut out, DEFLATE);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0x21);
        push_u32(&mut out, crc);
        push_u32(&mut out, compressed_size);
        push_u32(&mut out, uncompressed_size);
        push_u16(&mut out, name_bytes.len() as u16);
        push_u16(&mut out, 0);
        out.extend_from_slice(&name_bytes);
        out.extend_from_slice(&compressed);

        central.push(Central {
            name: name_bytes,
            crc,
            compressed: compressed.len(),
            uncompressed: data.len(),
            offset,
        });
    }

    let central_offset = out.len();
    for item in &central {
        out.extend_from_slice(CENTRAL_SIG);
        push_u16(&mut out, 20);
        push_u16(&mut out, 20);
        push_u16(&mut out, UTF8_FLAG);
        push_u16(&mut out, DEFLATE);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0x21);
        push_u32(&mut out, item.crc);
        push_u32(&mut out, item.compressed as u32);
        push_u32(&mut out, item.uncompressed as u32);
        push_u16(&mut out, item.name.len() as u16);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u32(&mut out, 0);
        push_u32(&mut out, item.offset as u32);
        out.extend_from_slice(&item.name);
    }
    let central_size = out.len() - central_offset;

    out.extend_from_slice(EOCD_SIG);
    push_u16(&mut out, 0);
    push_u16(&mut out, 0);
    push_u16(&mut out, central.len() as u16);
    push_u16(&mut out, central.len() as u16);
    push_u32(&mut out, central_size as u32);
    push_u32(&mut out, central_offset as u32);
    push_u16(&mut out, 0);
    Ok(out)
}

fn find_eocd(data: &[u8]) -> Result<usize, String> {
    let last = data
        .len()
        .checked_sub(22)
        .ok_or_else(|| "不是完整的 zip 文件".to_string())?;
    let first = data.len().saturating_sub(22 + u16::MAX as usize);
    for pos in (first..=last).rev() {
        if &data[pos..pos + 4] == EOCD_SIG {
            return Ok(pos);
        }
    }
    Err("找不到 zip 结束目录".to_string())
}

fn local_data_start(data: &[u8], local_offset: usize, compressed: usize) -> Result<usize, String> {
    if local_offset + 30 > data.len() {
        return Err("zip 本地头越界".to_string());
    }
    if &data[local_offset..local_offset + 4] != LOCAL_SIG {
        return Err("zip 本地头签名错误".to_string());
    }
    let name_len = u16_at(data, local_offset + 26)? as usize;
    let extra_len = u16_at(data, local_offset + 28)? as usize;
    let start = local_offset
        .checked_add(30 + name_len + extra_len)
        .ok_or_else(|| "zip 本地头越界".to_string())?;
    if start
        .checked_add(compressed)
        .ok_or_else(|| "zip 数据越界".to_string())?
        > data.len()
    {
        return Err("zip 数据越界".to_string());
    }
    Ok(start)
}

fn validate_name(name: &[u8]) -> Result<(), String> {
    if name.is_empty() {
        return Err("zip 条目名不能为空".to_string());
    }
    if name.contains(&0) || name.contains(&b'\\') {
        return Err("zip 条目名包含非法字符".to_string());
    }
    if name[0] == b'/' {
        return Err("zip 条目名不能是绝对路径".to_string());
    }
    for segment in name.split(|byte| *byte == b'/') {
        if segment == b"." || segment == b".." {
            return Err("zip 条目名不能包含路径穿越".to_string());
        }
    }
    Ok(())
}

fn u16_at(data: &[u8], offset: usize) -> Result<u16, String> {
    let bytes: [u8; 2] = data
        .get(offset..offset + 2)
        .ok_or_else(|| "zip 数据越界".to_string())?
        .try_into()
        .map_err(|_| "zip 数据越界".to_string())?;
    Ok(u16::from_le_bytes(bytes))
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32, String> {
    let bytes: [u8; 4] = data
        .get(offset..offset + 4)
        .ok_or_else(|| "zip 数据越界".to_string())?
        .try_into()
        .map_err(|_| "zip 数据越界".to_string())?;
    Ok(u32::from_le_bytes(bytes))
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_files_and_binary_data() {
        let save = vec![0u8, 1, 2, 255, 0, 42];
        let zip = build(&[
            (
                "meta/game.json".to_string(),
                br#"{"version":9,"platform":"GBA"}"#.to_vec(),
            ),
            ("龙珠大冒险.srm".to_string(), save.clone()),
        ])
        .unwrap();
        let archive = Archive::parse(&zip).unwrap();
        assert_eq!(archive.entries().len(), 2);
        assert_eq!(
            archive.read_name("meta/game.json").unwrap(),
            br#"{"version":9,"platform":"GBA"}"#
        );
        assert_eq!(archive.read_name("龙珠大冒险.srm").unwrap(), save);
        assert_eq!(archive.read_index(2).unwrap(), save);
        assert_eq!(archive.entries()[1].name, "龙珠大冒险.srm");
        assert_eq!(archive.entries()[1].size, 6);
    }

    #[test]
    fn rejects_unsafe_names() {
        for name in ["/absolute", "../escape", "a/../b", "a\\b", ""] {
            let error = build(&[(name.to_string(), Vec::new())]).unwrap_err();
            assert!(!error.is_empty(), "expected rejection for {name:?}");
        }
    }

    #[test]
    fn rejects_corrupt_crc_and_truncated_input() {
        let mut zip = build(&[("x.sav".to_string(), b"hello".to_vec())]).unwrap();
        let data_start = zip
            .windows(4)
            .position(|window| window == LOCAL_SIG)
            .map(|pos| pos + 30 + "x.sav".len())
            .unwrap();
        zip[data_start] ^= 0xff;
        assert!(Archive::parse(&zip).is_err());
        assert!(Archive::parse(&zip[..zip.len() / 2]).is_err());
    }

    #[test]
    fn empty_archive_round_trips() {
        let zip = build(&[]).unwrap();
        let archive = Archive::parse(&zip).unwrap();
        assert!(archive.entries().is_empty());
    }
}
