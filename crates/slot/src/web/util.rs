//! Small helpers shared by the Web service: process timeouts, log tails,
//! percent-encoding and time formatting.  Everything here is std-only.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

/// How much output a child process may buffer before we start discarding the
/// excess (the reader keeps draining the pipe so the child can never block).
const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

const POLL_INTERVAL: Duration = Duration::from_millis(40);

/// Result of a child process run that was bounded by a timeout.
#[derive(Debug, Default)]
pub struct RunOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl RunOutput {
    /// stdout followed by stderr, trimmed; used for CLI result panels.
    pub fn combined(&self) -> String {
        let mut text = String::new();
        if !self.stdout.trim().is_empty() {
            text.push_str(self.stdout.trim_end());
        }
        if !self.stderr.trim().is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(self.stderr.trim_end());
        }
        text
    }
}

/// Run `command` (stdout/stderr captured) and kill it once `timeout` elapses.
///
/// The pipe readers run on their own threads, so a chatty child can never
/// deadlock against a full pipe buffer while we poll `try_wait`.
pub fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<RunOutput, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A killed `/bin/sh` can leave its children (`linkSlot`, `sleep`, ...)
    // holding the pipes; put the child in its own process group so the
    // timeout can kill the whole tree and the reader threads finish promptly.
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|err| format!("无法启动命令：{err}"))?;

    let out_reader = child.stdout.take().map(|pipe| {
        std::thread::Builder::new()
            .name("slot-web-out".into())
            .spawn(move || read_capped(pipe))
    });
    let err_reader = child.stderr.take().map(|pipe| {
        std::thread::Builder::new()
            .name("slot-web-err".into())
            .spawn(move || read_capped(pipe))
    });

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let mut status = None;
    loop {
        match child.try_wait() {
            Ok(Some(done)) => {
                status = Some(done);
                break;
            }
            Ok(None) => {}
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            timed_out = true;
            #[cfg(unix)]
            {
                // process_group(0) above made the child a group leader, so the
                // negative pid targets the whole subtree (sh + linkSlot + ...).
                let _ = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            }
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    let stdout = join_reader(out_reader);
    let stderr = join_reader(err_reader);
    Ok(RunOutput {
        code: status.and_then(|status| status.code()),
        stdout,
        stderr,
        timed_out,
    })
}

fn read_capped(mut pipe: impl Read) -> Vec<u8> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                if buffer.len() < MAX_CAPTURE_BYTES {
                    let room = (MAX_CAPTURE_BYTES - buffer.len()).min(read);
                    buffer.extend_from_slice(&chunk[..room]);
                }
                // Keep draining after the cap so the child never blocks.
            }
            Err(_) => break,
        }
    }
    buffer
}

fn join_reader(handle: Option<std::io::Result<std::thread::JoinHandle<Vec<u8>>>>) -> String {
    match handle {
        Some(Ok(handle)) => handle
            .join()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// File modification time in seconds since the Unix epoch (0 when unknown).
pub fn mtime_secs(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Read the last `max_lines` lines of a file without slurping huge logs.
pub fn tail_lines(path: &Path, max_lines: usize, max_bytes: u64) -> Vec<String> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes);
    if start > 0 && file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect();
    // A chunked tail starts in the middle of a line: drop that fragment.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    // Keep `tail` semantics close to the shell tool: no phantom empty last line
    // for files that end with a newline.
    if lines.last().map(|line| line.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    if lines.len() > max_lines {
        lines.drain(..lines.len() - max_lines);
    }
    lines
}

/// `encodeURIComponent`-compatible escaping for query-string values.
pub fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 16);
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            push_percent(&mut out, byte);
        }
    }
    out
}

/// RFC 5987 `attr-char` escaping for `filename*=UTF-8''...`.
pub fn encode_rfc5987(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 16);
    for byte in value.bytes() {
        let attr = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'!' | b'#'
                    | b'$'
                    | b'&'
                    | b'+'
                    | b'-'
                    | b'.'
                    | b'^'
                    | b'_'
                    | b'`'
                    | b'|'
                    | b'~'
            );
        if attr {
            out.push(byte as char);
        } else {
            push_percent(&mut out, byte);
        }
    }
    out
}

fn push_percent(out: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push('%');
    out.push(HEX[(byte >> 4) as usize] as char);
    out.push(HEX[(byte & 0x0f) as usize] as char);
}

/// Plain-ASCII fallback for the legacy `filename="..."` parameter.
pub fn ascii_filename(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() || out.chars().all(|ch| ch == '_') {
        out = "download".to_string();
    }
    out
}

/// Short human-readable digest of a CLI error (last meaningful line, capped).
pub fn summarize(text: &str, max_chars: usize) -> String {
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("未知错误");
    truncate_chars(line, max_chars)
}

/// Truncate a string to at most `max_chars` characters (char-safe).
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_encoding_matches_uri_component() {
        assert_eq!(
            encode_query("States/GBA/宝可梦.sav"),
            "States%2FGBA%2F%E5%AE%9D%E5%8F%AF%E6%A2%A6.sav"
        );
        assert_eq!(encode_query("a b+c&d"), "a%20b%2Bc%26d");
    }

    #[test]
    fn rfc5987_keeps_attr_chars_and_escapes_utf8() {
        assert_eq!(encode_rfc5987("存档.state"), "%E5%AD%98%E6%A1%A3.state");
        assert_eq!(encode_rfc5987("ok-name_1.sav"), "ok-name_1.sav");
    }

    #[test]
    fn ascii_fallback_never_quotes() {
        assert_eq!(ascii_filename("存档\"x\".state"), "___x_.state");
        assert_eq!(ascii_filename("宝可梦.sav"), "___.sav");
    }

    #[test]
    fn summarize_uses_last_non_empty_line() {
        assert_eq!(summarize("第一行\n\n最后错误\n", 100), "最后错误");
        assert_eq!(summarize("", 100), "未知错误");
    }

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert_eq!(truncate_chars("abcdef", 3), "abc…");
    }

    #[test]
    fn tail_lines_reads_only_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.log");
        let mut text = String::new();
        for index in 0..500 {
            text.push_str(&format!("line-{index}\n"));
        }
        std::fs::write(&path, text).unwrap();
        let lines = tail_lines(&path, 5, 64 * 1024);
        assert_eq!(
            lines,
            vec!["line-495", "line-496", "line-497", "line-498", "line-499"]
        );
        // A tail that starts mid-line must drop the fragmented first line.
        let lines = tail_lines(&path, 3, 40);
        assert!(!lines.is_empty() && lines.len() <= 3);
        assert_eq!(lines.last().unwrap(), "line-499");
    }

    #[test]
    fn tail_lines_of_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(tail_lines(&dir.path().join("nope.log"), 10, 1024).is_empty());
    }

    #[test]
    fn run_with_timeout_captures_output() {
        #[cfg(unix)]
        {
            let mut command = Command::new("/bin/sh");
            command.arg("-c").arg("printf 'hello'; printf 'oops' >&2; exit 4");
            let out = run_with_timeout(command, Duration::from_secs(10)).unwrap();
            assert_eq!(out.code, Some(4));
            assert_eq!(out.stdout, "hello");
            assert_eq!(out.stderr, "oops");
            assert!(!out.timed_out);
        }
    }

    #[test]
    fn run_with_timeout_kills_runaway_process() {
        #[cfg(unix)]
        {
            let mut command = Command::new("/bin/sh");
            command.arg("-c").arg("sleep 30");
            let started = Instant::now();
            let out = run_with_timeout(command, Duration::from_millis(300)).unwrap();
            assert!(out.timed_out);
            assert!(started.elapsed() < Duration::from_secs(10));
        }
    }
}
