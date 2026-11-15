//! Extract readable text and images from an EPUB 2/3 container.
//!
//! The handheld already has a guide reader.  EPUBs are ZIP files with an OPF
//! package: the spine gives the reading order and each XHTML item is flattened
//! to text and image blocks here, so the paginator can render both without a
//! web engine.

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek};

use roxmltree::Document;

/// Guide files are read into memory on the device.  32 MiB accommodates
/// illustrated EPUBs while keeping a malformed upload from exhausting RAM.
pub const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_IMAGES: usize = 512;

/// One readable element in spine order.
#[derive(Debug)]
pub enum Block {
    Text(String),
    Image { name: String, data: Vec<u8> },
}

/// True for a ZIP whose first local header is the usual EPUB container.  The
/// extension check in the caller is still authoritative for a `.epub`.
pub fn is_epub(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

/// Flatten just the text blocks; used by callers that do not render images.
pub fn extract(bytes: &[u8]) -> Result<String, String> {
    let blocks = extract_blocks(bytes)?;
    let mut text = String::new();
    for block in blocks {
        let Block::Text(chunk) = block else {
            continue;
        };
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&chunk);
        if text.len() >= MAX_TEXT_BYTES {
            break;
        }
    }
    if text.trim().is_empty() {
        return Err("EPUB 里没有可显示的文本".into());
    }
    Ok(text)
}

/// Flatten the EPUB spine into text and image blocks in reading order.
pub fn extract_blocks(bytes: &[u8]) -> Result<Vec<Block>, String> {
    if bytes.len() > MAX_BYTES {
        return Err(format!("EPUB 超过 {}MB 上限", MAX_BYTES / (1024 * 1024)));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("无法打开 EPUB：{error}"))?;
    if archive.len() > MAX_ENTRIES {
        return Err("EPUB 文件数量过多".into());
    }
    let container = read_entry(&mut archive, "META-INF/container.xml")?;
    let container =
        std::str::from_utf8(&container).map_err(|_| "EPUB container.xml 不是 UTF-8".to_string())?;
    let document = Document::parse(container)
        .map_err(|error| format!("EPUB container.xml 格式错误：{error}"))?;
    let opf_path = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "rootfile")
        .and_then(|node| node.attribute("full-path"))
        .ok_or_else(|| "EPUB 缺少 OPF 路径".to_string())?;
    let opf_path = normalize_path(opf_path).ok_or_else(|| "EPUB OPF 路径不合法".to_string())?;
    let opf = read_entry(&mut archive, &opf_path)?;
    let opf = std::str::from_utf8(&opf).map_err(|_| "EPUB OPF 不是 UTF-8".to_string())?;
    let document = Document::parse(opf).map_err(|error| format!("EPUB OPF 格式错误：{error}"))?;
    let base = opf_path.rsplit_once('/').map_or("", |(dir, _)| dir);

    let mut manifest: HashMap<String, String> = HashMap::new();
    let mut manifest_order = Vec::new();
    for item in document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
    {
        let Some(id) = item.attribute("id") else {
            continue;
        };
        let Some(href) = item.attribute("href") else {
            continue;
        };
        let media_type = item.attribute("media-type").unwrap_or_default();
        let properties = item.attribute("properties").unwrap_or_default();
        let is_nav = properties
            .split_whitespace()
            .any(|property| property.eq_ignore_ascii_case("nav"));
        let readable = media_type.eq_ignore_ascii_case("application/xhtml+xml")
            || media_type.eq_ignore_ascii_case("text/html")
            || href.to_ascii_lowercase().ends_with(".xhtml")
            || href.to_ascii_lowercase().ends_with(".html");
        if readable && !is_nav {
            manifest.insert(id.to_string(), href.to_string());
            manifest_order.push(id.to_string());
        }
    }

    let mut spine = Vec::new();
    for itemref in document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "itemref")
    {
        if itemref
            .attribute("linear")
            .is_some_and(|linear| linear.eq_ignore_ascii_case("no"))
        {
            continue;
        }
        let Some(idref) = itemref.attribute("idref") else {
            continue;
        };
        if let Some(href) = manifest.get(idref) {
            if let Some(path) = resolve(base, href) {
                spine.push(path);
            }
        }
    }
    if spine.is_empty() {
        for id in manifest_order {
            if let Some(href) = manifest.get(&id) {
                if let Some(path) = resolve(base, href) {
                    spine.push(path);
                }
            }
        }
    }

    let mut blocks = Vec::new();
    let mut text_bytes = 0usize;
    let mut images = 0usize;
    for path in spine {
        let data = match read_entry(&mut archive, &path) {
            Ok(data) => data,
            Err(_) => continue,
        };
        let html = String::from_utf8_lossy(data.as_slice());
        let chapter_dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
        for raw in html_blocks(&html) {
            match raw {
                RawBlock::Text(chunk) => {
                    if chunk.is_empty() {
                        continue;
                    }
                    text_bytes = text_bytes.saturating_add(chunk.len());
                    blocks.push(Block::Text(chunk));
                    if text_bytes >= MAX_TEXT_BYTES {
                        break;
                    }
                }
                RawBlock::Image(src) => {
                    if images >= MAX_IMAGES {
                        continue;
                    }
                    let Some(image_path) = resolve(chapter_dir, &src) else {
                        continue;
                    };
                    let Ok(data) = read_entry_limit(&mut archive, &image_path, MAX_IMAGE_BYTES)
                    else {
                        continue;
                    };
                    images += 1;
                    blocks.push(Block::Image {
                        name: image_path,
                        data,
                    });
                }
            }
        }
        if text_bytes >= MAX_TEXT_BYTES {
            break;
        }
    }
    if blocks.is_empty() {
        return Err("EPUB 里没有可显示的内容".into());
    }
    Ok(blocks)
}

fn read_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>, String> {
    read_entry_limit(archive, name, MAX_BYTES)
}

fn read_entry_limit<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let entry = archive
        .by_name(name)
        .map_err(|error| format!("EPUB 缺少 {name}：{error}"))?;
    if entry.size() > limit as u64 {
        return Err(format!("EPUB 条目 {name} 过大"));
    }
    let mut data = Vec::with_capacity(entry.size().min(1024 * 1024) as usize);
    entry
        .take(limit as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|error| format!("读取 EPUB 条目失败：{error}"))?;
    if data.len() > limit {
        return Err(format!("EPUB 条目 {name} 过大"));
    }
    Ok(data)
}

fn resolve(base: &str, href: &str) -> Option<String> {
    let href = href.split('#').next()?;
    let decoded = percent_decode(href);
    if decoded.is_empty() {
        return None;
    }
    let joined = if decoded.starts_with('/') {
        decoded.trim_start_matches('/').to_string()
    } else if base.is_empty() {
        decoded
    } else {
        format!("{base}/{decoded}")
    };
    normalize_path(&joined)
}

fn normalize_path(path: &str) -> Option<String> {
    let mut parts = Vec::new();
    let path = path.replace('\\', "/");
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            let high = hex(bytes[at + 1]);
            let low = hex(bytes[at + 2]);
            if let (Some(high), Some(low)) = (high, low) {
                out.push(high * 16 + low);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

enum RawBlock {
    Text(String),
    Image(String),
}

fn html_blocks(html: &str) -> Vec<RawBlock> {
    let mut blocks = Vec::new();
    let mut text = String::new();
    let mut at = 0;
    while at < html.len() {
        let rest = &html[at..];
        if rest.starts_with("<!--") {
            at += rest.find("-->").map_or(rest.len(), |end| end + 3);
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            if let Some(end) = rest.find("]]>") {
                text.push_str(&decode_entities(&rest[9..end]));
                at += end + 3;
            } else {
                text.push_str(&decode_entities(&rest[9..]));
                break;
            }
            continue;
        }
        if rest.starts_with('<') {
            let Some(end) = rest.find('>') else {
                break;
            };
            let tag = &rest[1..end];
            let closing = tag.starts_with('/');
            let name = tag
                .trim_start_matches('/')
                .trim_start()
                .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if matches!(name.as_str(), "script" | "style") && !closing {
                let needle = format!("</{name}");
                let lower = html[at + end + 1..].to_ascii_lowercase();
                at = lower.find(&needle).map_or(html.len(), |start| {
                    let close = at + end + 1 + start;
                    html[close..]
                        .find('>')
                        .map_or(html.len(), |offset| close + offset + 1)
                });
                continue;
            }
            if name == "img" && !closing {
                flush_text(&mut blocks, &mut text);
                if let Some(src) = attribute(tag, "src") {
                    blocks.push(RawBlock::Image(src.to_string()));
                }
            } else if name == "br" {
                text.push('\n');
            } else if is_block(&name) {
                if closing {
                    text.push_str("\n\n");
                } else if !text.ends_with('\n') {
                    text.push('\n');
                }
            }
            at += end + 1;
            continue;
        }
        let end = rest.find('<').unwrap_or(rest.len());
        text.push_str(&decode_entities(&rest[..end]));
        at += end;
    }
    flush_text(&mut blocks, &mut text);
    blocks
}

fn flush_text(blocks: &mut Vec<RawBlock>, text: &mut String) {
    let chunk = tidy(text);
    text.clear();
    if !chunk.is_empty() {
        blocks.push(RawBlock::Text(chunk));
    }
}

fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(name) {
        let at = from + found;
        let before = tag[..at].chars().next_back();
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            from = at + name.len();
            continue;
        }
        let mut after = at + name.len();
        let rest = &tag[after..];
        after += rest.len() - rest.trim_start().len();
        if !tag[after..].starts_with('=') {
            from = at + name.len();
            continue;
        }
        after += 1;
        let rest = &tag[after..];
        after += rest.len() - rest.trim_start().len();
        let rest = &tag[after..];
        let Some(first) = rest.chars().next() else {
            return None;
        };
        if first == '"' || first == '\'' {
            let rest = &rest[1..];
            return rest.find(first).map(|end| &rest[..end]);
        }
        return Some(rest.split_whitespace().next().unwrap_or(rest));
    }
    None
}

fn is_block(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "li"
            | "ul"
            | "ol"
            | "tr"
            | "table"
            | "section"
            | "article"
            | "blockquote"
            | "pre"
            | "figure"
            | "figcaption"
    )
}

fn tidy(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::new();
    let mut blank = false;
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank = true;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if blank {
                out.push('\n');
            }
        }
        out.push_str(&line);
        blank = false;
    }
    out.trim().to_string()
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find(';').filter(|end| *end <= 16) else {
            out.push('&');
            rest = after;
            continue;
        };
        let entity = &after[..end];
        if let Some(ch) = entity_char(entity) {
            out.push(ch);
            rest = &after[end + 1..];
        } else {
            out.push('&');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn entity_char(entity: &str) -> Option<char> {
    if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
    }
    if let Some(decimal) = entity.strip_prefix('#') {
        return decimal.parse().ok().and_then(char::from_u32);
    }
    Some(match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "ldquo" => '“',
        "rdquo" => '”',
        "lsquo" => '‘',
        "rsquo" => '’',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn epub(items: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in items {
            zip.start_file(*name, options).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn sample() -> Vec<u8> {
        epub(&[
            (
                "META-INF/container.xml",
                br#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            ),
            (
                "OEBPS/content.opf",
                br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf"><manifest><item id="c2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/><item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#,
            ),
            (
                "OEBPS/text/ch1.xhtml",
                r#"<html><body><h1>第一章</h1><p>你好&amp;世界<br/>第二行</p><img src="../assets/pic.png" alt="插图"/><script>不要出现</script></body></html>"#
                    .as_bytes(),
            ),
            (
                "OEBPS/text/ch2.xhtml",
                "<html><body><p>第二章&#x4E2D;文</p></body></html>".as_bytes(),
            ),
            ("OEBPS/assets/pic.png", b"\x89PNG test"),
        ])
    }

    #[test]
    fn reads_spine_order_and_flattens_xhtml() {
        let text = extract(&sample()).unwrap();
        assert!(text.starts_with("第一章"));
        assert!(text.contains("你好&世界"));
        assert!(text.contains("第二行"));
        assert!(text.contains("第二章中文"));
        assert!(!text.contains("不要出现"));
        assert!(text.find("第一章") < text.find("第二章"));
    }

    #[test]
    fn keeps_images_in_spine_order() {
        let blocks = extract_blocks(&sample()).unwrap();
        let image = blocks
            .iter()
            .position(|block| matches!(block, Block::Image { .. }))
            .expect("image block");
        assert!(matches!(
            &blocks[image],
            Block::Image { name, data } if name == "OEBPS/assets/pic.png" && data == b"\x89PNG test"
        ));
        assert!(blocks[..image]
            .iter()
            .any(|block| matches!(block, Block::Text(text) if text.contains("第二行"))));
    }

    #[test]
    fn a_non_epub_is_rejected() {
        assert!(extract(b"not a zip").is_err());
    }
}
