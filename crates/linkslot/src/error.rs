//! Error categories and exit codes.
//!
//! Exit codes follow the project book (v0.5 §4.4):
//! 1 = network / WebDAV (including TLS certificate and clock problems),
//! 2 = conflict aborted, 3 = configuration, 4 = local file / verification.
//! `ERROR` in the stdout contract is the lowercase category below.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// No error (`LINKSLOT_ERROR=none`).
    None,
    /// Network unreachable, DNS, Wi-Fi, connection reset.
    Net,
    /// HTTP 401/403: WebDAV credentials rejected.
    Auth,
    /// Server certificate not trusted (`webdav.ca_cert` hint).
    Cert,
    /// Certificate validity window broken by a wrong device clock.
    Clock,
    /// HTTP 4xx/5xx / malformed remote payload / unsupported redirect.
    Server,
    /// Configuration problem (missing file, invalid field, unsupported HTTPS).
    Config,
    /// Local filesystem problem (read-only card, no space, I/O error).
    Fs,
    /// Download verification failed (SHA-256 or size mismatch).
    Verify,
    /// Operation timed out.
    Timeout,
    /// Push refused because the local cache for this unit is missing.
    NoCache,
    /// Push refused because another device pushed a newer version.
    RemoteNewer,
    /// Push refused because cloud and cache versions match but hashes differ.
    Forked,
    /// Push refused because the cloud version went backwards.
    RemoteRollback,
    /// Another linkSlot instance holds the sync lock.
    Busy,
    /// The cloud moved on while the user was choosing a direction
    /// (`--expect` mismatch). Not an error in the data: the caller must show
    /// the new versions and ask again.
    ChangedSince,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::None => "none",
            ErrorKind::Net => "net",
            ErrorKind::Auth => "auth",
            ErrorKind::Cert => "cert",
            ErrorKind::Clock => "clock",
            ErrorKind::Server => "server",
            ErrorKind::Config => "config",
            ErrorKind::Fs => "fs",
            ErrorKind::Verify => "verify",
            ErrorKind::Timeout => "timeout",
            ErrorKind::NoCache => "no-cache",
            ErrorKind::RemoteNewer => "remote-newer",
            ErrorKind::Forked => "forked",
            ErrorKind::RemoteRollback => "remote-rollback",
            ErrorKind::Busy => "busy",
            ErrorKind::ChangedSince => "changed-since",
        }
    }

    /// Process exit code for this category.
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorKind::None => 0,
            ErrorKind::Net
            | ErrorKind::Auth
            | ErrorKind::Cert
            | ErrorKind::Clock
            | ErrorKind::Server
            | ErrorKind::Timeout => 1,
            ErrorKind::NoCache
            | ErrorKind::RemoteNewer
            | ErrorKind::Forked
            | ErrorKind::RemoteRollback
            | ErrorKind::ChangedSince => 2,
            ErrorKind::Config => 3,
            ErrorKind::Fs | ErrorKind::Verify | ErrorKind::Busy => 4,
        }
    }

    pub fn is_conflict(self) -> bool {
        matches!(
            self,
            ErrorKind::NoCache
                | ErrorKind::RemoteNewer
                | ErrorKind::Forked
                | ErrorKind::RemoteRollback
                | ErrorKind::ChangedSince
        )
    }

    /// Whether an HTTP/transport operation with this category may be retried.
    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            ErrorKind::Net | ErrorKind::Timeout | ErrorKind::Server
        )
    }

    /// Short actionable hint appended to the human readable detail.
    pub fn hint(self) -> &'static str {
        match self {
            ErrorKind::None => "",
            ErrorKind::Net => "网络未连接，请检查 Wi-Fi 或配置",
            ErrorKind::Auth => "认证失败，请检查 webdav.username/password",
            ErrorKind::Cert => "证书不受信任，可在配置中设置 webdav.ca_cert",
            ErrorKind::Clock => "设备时钟异常，请校准时间后重试",
            ErrorKind::Server => "服务器返回错误，请稍后重试",
            ErrorKind::Config => "配置错误，请检查 config.toml",
            ErrorKind::Fs => "本地文件操作失败，请检查存储卡",
            ErrorKind::Verify => "文件校验失败，请重新拉取",
            ErrorKind::Timeout => "操作超时，请稍后重试",
            ErrorKind::NoCache => "本地缓存缺失，请先拉取云端存档",
            ErrorKind::RemoteNewer => "云端有更新的版本，请先拉取；确认覆盖可用 --force",
            ErrorKind::Forked => "云端与本地均已修改，请人工处理或使用 --force",
            ErrorKind::RemoteRollback => "云端版本异常回退，请检查云端数据",
            ErrorKind::Busy => "另一个同步任务正在进行，请稍后再试",
            ErrorKind::ChangedSince => "云端已被其它设备更新，请基于最新版本重新选择方向",
        }
    }
}

/// An error carrying both a category for the `LINKSLOT_ERROR` line and a
/// human readable Chinese message for `LINKSLOT_DETAIL` / stderr.
#[derive(Debug, Clone)]
pub struct LinkError {
    kind: ErrorKind,
    message: String,
}

impl LinkError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        LinkError {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn detail(&self) -> String {
        let hint = self.kind.hint();
        if hint.is_empty() || self.message.contains(hint) {
            self.message.clone()
        } else if self.message.is_empty() {
            hint.to_string()
        } else {
            format!("{}；{}", self.message, hint)
        }
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Config, message)
    }

    pub fn fs(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Fs, message)
    }
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LinkError {}

pub type Result<T> = std::result::Result<T, LinkError>;

/// Map an I/O error to a local-filesystem error with a context prefix.
pub fn fs_error(context: impl fmt::Display, err: std::io::Error) -> LinkError {
    LinkError::new(ErrorKind::Fs, format!("{context}：{err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_contract() {
        assert_eq!(ErrorKind::None.exit_code(), 0);
        assert_eq!(ErrorKind::Net.exit_code(), 1);
        assert_eq!(ErrorKind::Auth.exit_code(), 1);
        assert_eq!(ErrorKind::Cert.exit_code(), 1);
        assert_eq!(ErrorKind::Clock.exit_code(), 1);
        assert_eq!(ErrorKind::Server.exit_code(), 1);
        assert_eq!(ErrorKind::Timeout.exit_code(), 1);
        assert_eq!(ErrorKind::NoCache.exit_code(), 2);
        assert_eq!(ErrorKind::RemoteNewer.exit_code(), 2);
        assert_eq!(ErrorKind::Forked.exit_code(), 2);
        assert_eq!(ErrorKind::RemoteRollback.exit_code(), 2);
        assert_eq!(ErrorKind::Config.exit_code(), 3);
        assert_eq!(ErrorKind::Fs.exit_code(), 4);
        assert_eq!(ErrorKind::Verify.exit_code(), 4);
        assert_eq!(ErrorKind::Busy.exit_code(), 4);
    }

    #[test]
    fn error_names_match_contract() {
        assert_eq!(ErrorKind::NoCache.as_str(), "no-cache");
        assert_eq!(ErrorKind::RemoteNewer.as_str(), "remote-newer");
        assert_eq!(ErrorKind::Forked.as_str(), "forked");
        assert_eq!(ErrorKind::RemoteRollback.as_str(), "remote-rollback");
        assert_eq!(ErrorKind::None.as_str(), "none");
    }

    #[test]
    fn detail_appends_hint_once() {
        let e = LinkError::new(ErrorKind::Net, "TCP 连接失败");
        assert!(e.detail().contains("网络未连接"));
        let e = LinkError::new(ErrorKind::Net, ErrorKind::Net.hint());
        assert_eq!(e.detail().matches("网络未连接").count(), 1);
    }
}
