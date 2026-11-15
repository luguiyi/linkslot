//! Hand-written CLI parser and command dispatch.
//!
//! Argv is passed directly (never through a shell) so Chinese game names and
//! spaces are safe. Commands:
//!
//! ```text
//! linkSlot sync   --platform GBA --emulator mgba --game "合金弹头" [--rom ROM] [--resolve upload|download] [--full] [--net-up]
//! linkSlot push   --platform GBA --emulator mgba --game "合金弹头" [--rom ROM] [--force] [--full] [--net-up]
//! linkSlot pull   --platform GBA --emulator mgba --game "合金弹头" [--rom ROM] [--force] [--full] [--net-up]
//! linkSlot status --platform GBA --emulator mgba --game "合金弹头" [--rom ROM]
//! linkSlot net status
//! linkSlot net up [--timeout 30]
//! linkSlot config init [--force]
//! ```
//!
//! The sync unit is identified by the full-file SHA-256 of the ROM. `--rom`
//! names it explicitly; when omitted the CLI looks for the unique ROM whose
//! stem matches `--game` under `<root>/Games/<platform>/` (recursively,
//! platform extension, case-insensitive) and reports a configuration error
//! when there is none or more than one.
//!
//! Common options: `--root DIR` (else `SLOT_ROOT`, else `/mnt/sdcard`),
//! `--config FILE`, `--verbose`.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::{self, Config};
use crate::error::{ErrorKind, LinkError, Result};
use crate::hash;
use crate::net;
use crate::output::{Outcome, Reporter, Stage};
use crate::sync::{
    self, Ctx, ExpectedVersions, PullOptions, PushOptions, ResolveSpec, SyncOptions,
};
use crate::unit::{Kind, Unit};

pub const DEFAULT_ROOT: &str = slot_model::DEVICE_ROOT;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const USAGE: &str = "\
用法：
  linkSlot sync   --platform GBA --emulator mgba --game <游戏名> [--rom <ROM 路径>] [--only state|sav] [--resolve upload|download|state=upload,sav=download] [--expect state=12,sav=7] [--full] [--net-up] [--timeout 30] [--verbose]
  linkSlot push   --platform GBA --emulator mgba --game <游戏名> [--rom <ROM 路径>] [--force] [--full] [--net-up] [--timeout 30] [--verbose]
  linkSlot pull   --platform GBA --emulator mgba --game <游戏名> [--rom <ROM 路径>] [--force] [--full] [--net-up] [--timeout 30] [--verbose]
  linkSlot status --platform GBA --emulator mgba --game <游戏名> [--rom <ROM 路径>] [--verbose]
  linkSlot net status [--verbose]
  linkSlot net test   [--verbose]
  linkSlot net up     [--timeout 30] [--verbose]
  linkSlot config init [--force] [--verbose]
  linkSlot mods list [--json]
  linkSlot mods check --id <mod id> [--action <action id>]
  linkSlot mods test  --profile <同步方式id>
  linkSlot mods run   --platform GBA --emulator mgba --game <游戏名> [--id <mod id> [--action <action id>]] [--choice key=option] [--net-up] [--timeout 30] [--verbose]
  linkSlot mods boot  --id <mod id> --action <action id> [--root <目录>] [--verbose]

通用参数：
  --root <目录>    存储卡根目录（缺省读 SLOT_ROOT，再缺省 /mnt/sdcard）
  --config <文件>  配置文件路径（缺省 <root>/linkSlot/Config/config.toml）
  --id <脚本id>    mods check / boot / run：mod 目录名（run 时只运行该 mod 的手动动作）
  --action <id>    mods check：动作目录名；省略时检查该 mod 的全部动作；mods boot/run：只执行这一个动作
  --profile <id>   mods test：要测试远程根的同步方式 id
  --choice <k=v>   mods run：预先回答 mods.choose 的选择，可重复
  --json           mods list：输出 JSON
  --rom <文件>     指定游戏 ROM 文件（按全文件 SHA-256 识别游戏）；省略时在 <root>/Games/<平台>/ 下自动查找与 --game 同名的唯一 ROM
  --verbose        输出详细步骤到 stderr，并写入 <root>/Logs/linkSlot.log
  --resolve <方向>  sync 冲突时强制覆盖：upload=覆盖云端，download=覆盖本地；也可按单元指定 state=...,sav=...（覆盖前自动备份被覆盖方）
  --only <单元>     sync 只处理指定单元：state=即时存档，sav=电池存档
  --expect <版本>   sync 提交时校验云端版本：state=<n>,sav=<n>；云端已变化则中止（changed-since）
  --force          push/pull：强制覆盖（先备份被覆盖方）；config init：覆盖已有配置；sync 不支持
  --full           push/pull：忽略“已是最新”，整单元全量重传/重下（修复被外部改动的文件）；sync：需要传输时忽略差量
  --net-up         sync/push/pull：网络不可用时强制按需联网
  --timeout <秒>   net up / sync / push / pull 的联网等待时间上限

退出码：0 成功或无更新；1 网络/WebDAV；2 冲突（sync 未带 --resolve）；3 配置；4 本地文件或校验。
";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Sync,
    Push,
    Pull,
    Status,
    NetStatus,
    NetTest,
    NetUp,
    ConfigInit,
    ModsList,
    ModsCheck,
    ModsTest,
    ModsRun,
    ModsBoot,
    Help,
    Version,
}

#[derive(Clone, Debug)]
struct ParsedArgs {
    command: Command,
    root: PathBuf,
    /// Kept for diagnostics/tests (distinguishes `--root` from the default).
    #[allow(dead_code)]
    explicit_root: bool,
    config: Option<PathBuf>,
    /// Explicit `--rom <path>`; `None` means "discover it under `<root>/Games`".
    rom: Option<PathBuf>,
    unit: Option<Unit>,
    /// `mods check --id`.
    mod_id: Option<String>,
    /// `mods check --action` / `mods boot --action`.
    action_id: Option<String>,
    /// `mods test --profile`.
    profile: Option<String>,
    /// `mods run --choice key=value` (repeatable).
    choices: Vec<(String, String)>,
    /// `mods list --json`.
    json: bool,
    force: bool,
    full: bool,
    net_up: bool,
    verbose: bool,
    timeout: Option<u64>,
    resolve: Option<ResolveSpec>,
    only: Option<Kind>,
    expect: ExpectedVersions,
}

/// Entry point for `main`; returns the process exit code.
pub fn run() -> i32 {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    execute_os(&args, &mut out, &mut err)
}

/// Testable entry point that takes already-parsed OS arguments.
pub fn execute_os(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let mut strings = Vec::with_capacity(args.len());
    for arg in args {
        match arg.clone().into_string() {
            Ok(value) => strings.push(value),
            Err(bad) => {
                let message = format!("参数不是有效 UTF-8：{:?}", bad);
                let mut rep = Reporter::new(std::path::Path::new(DEFAULT_ROOT), false, out, err);
                let outcome = Outcome::error("config", &LinkError::config(message), None, None);
                rep.finish(&outcome);
                return outcome.exit_code();
            }
        }
    }
    execute(&strings, out, err)
}

/// Testable entry point with UTF-8 string arguments.
pub fn execute(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let parsed = match parse(args) {
        Ok(parsed) => parsed,
        Err(parse_error) => {
            let _ = writeln!(err, "{USAGE}");
            let mut rep = Reporter::new(std::path::Path::new(DEFAULT_ROOT), false, out, err);
            let outcome = Outcome::error("config", &parse_error, None, None);
            rep.finish(&outcome);
            return outcome.exit_code();
        }
    };

    match parsed.command {
        Command::Help => {
            let _ = writeln!(out, "{USAGE}");
            let _ = out.flush();
            0
        }
        Command::Version => {
            if cfg!(feature = "mods") {
                let _ = writeln!(out, "linkSlot {VERSION} mods=1");
            } else {
                let _ = writeln!(out, "linkSlot {VERSION}");
            }
            let _ = out.flush();
            0
        }
        _ => dispatch(&parsed, out, err),
    }
}

fn dispatch(parsed: &ParsedArgs, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let mut rep = Reporter::new(&parsed.root, parsed.verbose, out, err);
    let action = action_for(parsed.command);
    let outcome = match parsed.command {
        Command::ConfigInit => run_config_init(parsed),
        Command::NetStatus | Command::NetTest | Command::NetUp => {
            if parsed.command == Command::NetUp {
                rep.stage(Stage::Net);
            }
            run_net(parsed, &mut rep)
        }
        Command::ModsList | Command::ModsCheck | Command::ModsTest | Command::ModsRun
        | Command::ModsBoot => run_mods(parsed, &mut rep),
        Command::Push | Command::Pull | Command::Status | Command::Sync => {
            run_unit(parsed, action, &mut rep)
        }
        Command::Help | Command::Version => unreachable!("handled before dispatch"),
    };
    let code = outcome.exit_code();
    // `mods list --json` is machine-readable: keep stdout pure JSON and skip
    // the human/contract block (the JSON line was already written).
    if parsed.command == Command::ModsList && parsed.json {
        return code;
    }
    rep.finish(&outcome);
    code
}

fn action_for(command: Command) -> &'static str {
    match command {
        Command::Sync => "sync",
        Command::Push => "push",
        Command::Pull => "pull",
        Command::Status => "status",
        Command::NetStatus | Command::NetTest | Command::NetUp => "net",
        Command::ConfigInit => "config",
        Command::ModsList | Command::ModsCheck | Command::ModsTest | Command::ModsRun
        | Command::ModsBoot => "mods",
        Command::Help | Command::Version => "config",
    }
}

fn load_config(parsed: &ParsedArgs) -> Result<Config> {
    Config::load(&parsed.root, parsed.config.as_deref())
}

/// Build the fully identified unit, computing the ROM SHA-256 that names it in
/// the cloud. `--rom` wins; otherwise the ROM is auto-discovered from
/// `<root>/Games/<platform>/`.
fn require_unit(parsed: &ParsedArgs) -> Result<Unit> {
    let unit = parsed
        .unit
        .clone()
        .ok_or_else(|| LinkError::config("缺少 --platform/--emulator/--game 参数"))?;
    let digest = require_rom_digest(parsed, &unit.platform, &unit.game)?;
    unit.with_rom_sha256(&digest)
}

/// ROM SHA-256 for a game identity: `--rom` when given, otherwise the unique
/// auto-discovered ROM under `<root>/Games/<platform>/`.
fn require_rom_digest(parsed: &ParsedArgs, platform: &str, game: &str) -> Result<String> {
    let rom = match &parsed.rom {
        Some(path) => path.clone(),
        None => discover_rom(&parsed.root, platform, game)?,
    };
    // A read failure is a local file error (exit code 4), not a config error.
    hash::sha256_file(&rom)
}

/// ROM file extensions Slot accepts for a platform (compared case-insensitively
/// and mirroring `slot-store`'s cart scanner).
///
/// FC is `.nes` only: the frontend has no disk-swap keys for a `.fds`, so a
/// disk image on the card would sync as a game the shelf can never seat.
/// The catch-all arm that used to answer "gb/gbc" for anything unknown is gone
/// on purpose — a platform this table does not know now finds no ROM rather
/// than silently looking for the wrong extensions and reporting them back.
pub fn rom_extensions(platform: &str) -> &'static [&'static str] {
    // 精确匹配目录名（大小写敏感）：未知机种返回空，而不是猜一组扩展名。
    // 数据来自 `slot-model` 机种表，与前端扫描器同源。
    slot_model::MACHINES
        .iter()
        .find(|m| m.dir == platform)
        .map(|m| m.rom_ext)
        .unwrap_or(&[])
}

/// Find the unique ROM for `game` below `<root>/Games/<platform>/`.
///
/// Subdirectories are searched recursively; hidden files/directories are
/// skipped and directory symlinks are not followed (same rules as the Slot
/// library scanner). Zero or several matches are configuration errors that
/// point at `--rom`.
pub fn discover_rom(root: &Path, platform: &str, game: &str) -> Result<PathBuf> {
    let dir = root.join("Games").join(platform);
    let mut candidates = Vec::new();
    collect_roms(&dir, platform, game, &mut candidates);
    candidates.sort();
    match candidates.len() {
        0 => {
            let extensions: Vec<String> = rom_extensions(platform)
                .iter()
                .map(|ext| format!(".{ext}"))
                .collect();
            Err(LinkError::config(format!(
                "在 {} 下未找到游戏 {game:?} 的 ROM（支持扩展名 {}），请用 --rom 指定 ROM 路径",
                dir.display(),
                extensions.join("/")
            )))
        }
        1 => Ok(candidates.remove(0)),
        count => {
            let names: Vec<String> = candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            Err(LinkError::config(format!(
                "在 {} 下找到 {count} 个名为 {game:?} 的 ROM，无法确定使用哪一个：{}；请用 --rom 指定 ROM 路径",
                dir.display(),
                names.join("；")
            )))
        }
    }
}

fn collect_roms(dir: &Path, platform: &str, game: &str, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // A missing Games/<platform> simply means "no candidate"; unreadable
        // subdirectories are skipped like the Slot library scan does.
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_hidden(&path) {
            continue;
        }
        // Directory symlinks are not followed (avoids cycles / scanning
        // outside Games); file symlinks are accepted.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_roms(&path, platform, game, out);
            continue;
        }
        let is_file = file_type.is_file() || (file_type.is_symlink() && path.is_file());
        if !is_file {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if stem != game {
            continue;
        }
        let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if rom_extensions(platform)
            .iter()
            .any(|ext| extension.eq_ignore_ascii_case(ext))
        {
            out.push(path);
        }
    }
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn run_config_init(parsed: &ParsedArgs) -> Outcome {
    match config::init(&parsed.root, parsed.force) {
        Ok(config::InitResult::Created(path)) => {
            Outcome::ok("config", None, format!("已生成配置模板 {}", path.display()))
        }
        Ok(config::InitResult::Existing(_path)) => Outcome::noop("config", "配置已存在"),
        Err(err) => Outcome::error("config", &err, None, None),
    }
}

fn run_net(parsed: &ParsedArgs, rep: &mut Reporter) -> Outcome {
    let config = match load_config(parsed) {
        Ok(config) => config,
        Err(err) => return Outcome::error("net", &err, None, None),
    };
    for warning in config.warnings() {
        rep.warn(warning);
    }
    match parsed.command {
        Command::NetStatus => match net::status(&parsed.root, &config) {
            Ok(status) => {
                if status.connected {
                    Outcome::ok("net", None, "网络已连接")
                } else {
                    Outcome::error(
                        "net",
                        &LinkError::new(ErrorKind::Net, status.detail),
                        None,
                        None,
                    )
                }
            }
            Err(err) => Outcome::error("net", &err, None, None),
        },
        Command::NetUp => match net::bring_up(&parsed.root, &config, parsed.timeout, rep) {
            Ok(status) => Outcome::ok("net", None, status.detail),
            Err(err) => Outcome::error("net", &err, None, None),
        },
        Command::NetTest => match crate::webdav::DavClient::new(&config, rep) {
            Ok(client) => match client.probe(rep) {
                Ok(status) => Outcome::ok("net", None, format!("WebDAV 连接成功（HTTP {status}）")),
                Err(err) => Outcome::error("net", &err, None, None),
            },
            Err(err) => Outcome::error("net", &err, None, None),
        },
        _ => unreachable!("run_net only handles net commands"),
    }
}

fn run_unit(parsed: &ParsedArgs, action: &'static str, rep: &mut Reporter) -> Outcome {
    let config = match load_config(parsed) {
        Ok(config) => config,
        Err(err) => return Outcome::error(action, &err, None, None),
    };
    for warning in config.warnings() {
        rep.warn(warning);
    }
    let unit = match require_unit(parsed) {
        Ok(unit) => unit,
        Err(err) => return Outcome::error(action, &err, None, None),
    };
    let ctx = Ctx::new(parsed.root.clone());
    match parsed.command {
        Command::Push => sync::push(
            &ctx,
            &config,
            &unit,
            PushOptions {
                force: parsed.force,
                full: parsed.full,
                net_up: parsed.net_up,
                timeout: parsed.timeout,
            },
            rep,
        ),
        Command::Pull => sync::pull(
            &ctx,
            &config,
            &unit,
            PullOptions {
                force: parsed.force,
                full: parsed.full,
                net_up: parsed.net_up,
                timeout: parsed.timeout,
            },
            rep,
        ),
        Command::Status => sync::status(&ctx, &config, &unit, rep),
        Command::Sync => sync::sync(
            &ctx,
            &config,
            &unit,
            SyncOptions {
                only: parsed.only,
                resolve: parsed.resolve.unwrap_or_default(),
                expect: parsed.expect,
                full: parsed.full,
                net_up: parsed.net_up,
                timeout: parsed.timeout,
            },
            rep,
        ),
        _ => unreachable!("run_unit only handles unit commands"),
    }
}

fn run_mods(parsed: &ParsedArgs, rep: &mut Reporter) -> Outcome {
    match parsed.command {
        Command::ModsList => run_mods_list(parsed, rep),
        Command::ModsCheck => run_mods_check(parsed),
        Command::ModsTest => run_mods_test(parsed, rep),
        Command::ModsRun => run_mods_run(parsed, rep),
        Command::ModsBoot => run_mods_boot(parsed, rep),
        _ => unreachable!("run_mods only handles mods commands"),
    }
}

fn run_mods_run(parsed: &ParsedArgs, rep: &mut Reporter) -> Outcome {
    rep.stage(Stage::Mods);
    let config = match crate::config::Config::load_for_mods(&parsed.root, parsed.config.as_deref())
    {
        Ok(config) => config,
        Err(error) => return Outcome::error("mods", &error, None, None),
    };
    let Some(unit) = parsed.unit.as_ref() else {
        return Outcome::error(
            "mods",
            &LinkError::config("mods run 缺少 --platform/--emulator/--game 参数"),
            None,
            None,
        );
    };
    let endpoints = crate::mods::manual_endpoints(&parsed.root);
    if !endpoints.is_empty() {
        rep.stage(Stage::Net);
        let target = endpoints
            .first()
            .map(|(_, endpoint)| (endpoint.socket_host(), endpoint.port, "mod"));
        if let Err(error) = crate::net::ensure_connected_to(
            &parsed.root,
            &config,
            crate::net::NetMode::Real,
            parsed.net_up,
            parsed.timeout,
            target,
            rep,
        ) {
            return Outcome::error("mods", &error, None, None);
        }
        rep.stage(Stage::Mods);
    }
    let _lock = match crate::store::UnitLock::acquire(&parsed.root) {
        Ok(lock) => lock,
        Err(error) => return Outcome::error("mods", &error, None, None),
    };
    let device_id = crate::config::device_id(&parsed.root, &config);
    let summary = crate::mods::run_manual(
        &parsed.root,
        &config,
        unit,
        &device_id,
        &parsed.choices,
        parsed.mod_id.as_deref(),
        parsed.action_id.as_deref(),
        rep,
    );
    match summary.status {
        crate::mods::ManualStatus::Ok => {
            let mut outcome = Outcome::ok("mods", None, summary.detail.clone());
            if let Some(direction) = summary.direction {
                outcome = outcome.with_direction(direction);
            }
            outcome.with_units(vec![crate::output::UnitOutcome {
                kind: Kind::Sav,
                result: crate::output::ResultKind::Ok,
                direction: summary.direction,
                version: None,
                error: crate::error::ErrorKind::None,
                cloud: None,
                local: None,
            }])
        }
        crate::mods::ManualStatus::Noop => Outcome::noop("mods", summary.detail.clone()),
        crate::mods::ManualStatus::Choice => {
            let Some(request) = summary.choice.clone() else {
                return Outcome::error(
                    "mods",
                    &LinkError::new(ErrorKind::Server, "mod 请求了选择但没有提供选项"),
                    None,
                    None,
                );
            };
            let options = request
                .options
                .iter()
                .map(|option| format!("{}={}", request.key, option.id))
                .collect::<Vec<_>>()
                .join(" / ");
            let title = request.title.clone();
            Outcome::choice(
                "mods",
                request,
                format!("需要选择「{title}」；请重新运行并加 {options}"),
            )
        }
        crate::mods::ManualStatus::Error => {
            let error = LinkError::new(summary.error, summary.detail.clone());
            Outcome::error("mods", &error, None, None)
        }
    }
}

fn run_mods_boot(parsed: &ParsedArgs, rep: &mut Reporter) -> Outcome {
    rep.stage(Stage::Mods);
    let id = parsed
        .mod_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let action = parsed
        .action_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let (Some(id), Some(action)) = (id, action) else {
        return Outcome::error(
            "mods",
            &LinkError::config("mods boot 需要 --id <mod id> 和 --action <action id>"),
            None,
            None,
        );
    };
    let config = match crate::config::Config::load_for_mods(&parsed.root, parsed.config.as_deref())
    {
        Ok(config) => config,
        Err(error) => return Outcome::error("mods", &error, None, None),
    };
    let _lock = match crate::store::UnitLock::acquire(&parsed.root) {
        Ok(lock) => lock,
        Err(error) => return Outcome::error("mods", &error, None, None),
    };
    let device_id = crate::config::device_id(&parsed.root, &config);
    let report = match crate::mods::run_boot(
        &parsed.root,
        &config,
        id,
        action,
        &device_id,
        rep,
    ) {
        Ok(report) => report,
        Err(error) => {
            return Outcome::error(
                "mods",
                &LinkError::new(ErrorKind::Config, error),
                None,
                None,
            )
        }
    };
    if report.failed > 0 {
        return Outcome::error(
            "mods",
            &LinkError::new(ErrorKind::Server, "开机后 mod 动作执行失败"),
            None,
            None,
        );
    }
    Outcome::ok("mods", None, "开机后 mod 动作已执行")
}

fn run_mods_list(parsed: &ParsedArgs, rep: &mut Reporter) -> Outcome {
    let entries = match slot_mods::list(&parsed.root) {
        Ok(entries) => entries,
        Err(error) => {
            return Outcome::error("mods", &LinkError::config(error.to_string()), None, None)
        }
    };
    if parsed.json {
        let values: Vec<serde_json::Value> = entries
            .iter()
            .map(|entry| {
                let actions = slot_mods::read_mod(&parsed.root, &entry.id)
                    .map(|detail| {
                        detail
                            .actions
                            .iter()
                            .map(|action| {
                                serde_json::json!({
                                    "id": action.id,
                                    "name": action.name,
                                    "timing": action.timing.map(|timing| timing.as_str()),
                                    "timing_label": action.timing.map(|timing| timing.label()),
                                    "enabled": action.enabled,
                                    "order": action.order,
                                    "boot_delay_minutes": action.boot_delay_minutes,
                                    "error": action.error,
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                serde_json::json!({
                    "id": entry.id,
                    "name": entry.name,
                    "version": entry.version,
                    "author": if entry.author.trim().is_empty() { "无作者信息" } else { entry.author.trim() },
                    "enabled": entry.enabled,
                    "action_count": entry.action_count,
                    "error": entry.error,
                    "updated_at": entry.updated_at,
                    "actions": actions,
                })
            })
            .collect();
        return match serde_json::to_string(&values) {
            Ok(text) => {
                rep.raw_stdout(&text);
                Outcome::ok(
                    "mods",
                    None,
                    format!("{} 个 mod（JSON 已输出）", entries.len()),
                )
            }
            Err(error) => Outcome::error(
                "mods",
                &LinkError::config(format!("无法序列化 mods 列表：{error}")),
                None,
                None,
            ),
        };
    }
    if entries.is_empty() {
        return Outcome::noop("mods", "没有 mod");
    }
    for entry in &entries {
        let author = if entry.author.trim().is_empty() {
            "无作者信息"
        } else {
            entry.author.trim()
        };
        rep.progress(format!(
            "{}  v{}  {}  {}  作者：{}  {} 个动作",
            entry.id,
            entry.version,
            if entry.enabled { "启用" } else { "停用" },
            entry.name,
            author,
            entry.action_count
        ));
        if let Ok(detail) = slot_mods::read_mod(&parsed.root, &entry.id) {
            for action in &detail.actions {
                let delay = if action.timing == Some(slot_mods::Timing::AfterBoot) {
                    format!(" +{} 分", action.boot_delay_minutes)
                } else {
                    String::new()
                };
                rep.progress(format!(
                    "  - {}  {}{}  {}  {}",
                    action.id,
                    action.timing.map(|timing| timing.label()).unwrap_or("无效"),
                    delay,
                    if action.enabled { "启用" } else { "停用" },
                    action.name
                ));
            }
        }
    }
    Outcome::ok("mods", None, format!("{} 个 mod", entries.len()))
}

fn run_mods_check(parsed: &ParsedArgs) -> Outcome {
    let Some(id) = parsed
        .mod_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return Outcome::error(
            "mods",
            &LinkError::config("mods check 需要 --id <mod id>"),
            None,
            None,
        );
    };
    if let Some(action_id) = parsed
        .action_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        let detail = match slot_mods::read_action(&parsed.root, id, action_id) {
            Ok(detail) => detail,
            Err(error) => {
                return Outcome::error("mods", &LinkError::config(error.to_string()), None, None)
            }
        };
        if let Some(error) = &detail.entry.error {
            return Outcome::error(
                "mods",
                &LinkError::config(format!("动作 {id}/{action_id} 无效：{error}")),
                None,
                None,
            );
        }
        return match crate::mods::check_source(&detail.content, &format!("{id}/{action_id}")) {
            Ok(()) => Outcome::ok("mods", None, format!("动作 {id}/{action_id} 语法检查通过")),
            Err(error) => Outcome::error(
                "mods",
                &LinkError::config(format!("动作 {id}/{action_id} 语法错误：{error}")),
                None,
                None,
            ),
        };
    }

    let detail = match slot_mods::read_mod(&parsed.root, id) {
        Ok(detail) => detail,
        Err(error) => {
            return Outcome::error("mods", &LinkError::config(error.to_string()), None, None)
        }
    };
    if detail.actions.is_empty() {
        return Outcome::error(
            "mods",
            &LinkError::config(format!("mod {id} 没有动作")),
            None,
            None,
        );
    }
    let mut checked = 0usize;
    for action in &detail.actions {
        if let Some(error) = &action.error {
            return Outcome::error(
                "mods",
                &LinkError::config(format!("动作 {id}/{} 无效：{error}", action.id)),
                None,
                None,
            );
        }
        let action_detail = match slot_mods::read_action(&parsed.root, id, &action.id) {
            Ok(detail) => detail,
            Err(error) => {
                return Outcome::error("mods", &LinkError::config(error.to_string()), None, None)
            }
        };
        if let Err(error) =
            crate::mods::check_source(&action_detail.content, &format!("{id}/{}", action.id))
        {
            return Outcome::error(
                "mods",
                &LinkError::config(format!("动作 {id}/{} 语法错误：{error}", action.id)),
                None,
                None,
            );
        }
        checked += 1;
    }
    Outcome::ok(
        "mods",
        None,
        format!("mod {id} 的 {checked} 个动作语法检查通过"),
    )
}

fn run_mods_test(parsed: &ParsedArgs, rep: &mut Reporter) -> Outcome {
    let Some(profile_id) = parsed
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return Outcome::error(
            "mods",
            &LinkError::config("mods test 需要 --profile <同步方式id>"),
            None,
            None,
        );
    };
    let config = match load_config(parsed) {
        Ok(config) => config,
        Err(error) => return Outcome::error("mods", &error, None, None),
    };
    for warning in config.warnings() {
        rep.warn(warning);
    }
    let Some(storage) = config
        .all_storages()
        .into_iter()
        .find(|storage| storage.id == profile_id)
    else {
        return Outcome::error(
            "mods",
            &LinkError::config(format!("同步方式「{profile_id}」不存在")),
            None,
            None,
        );
    };
    if storage.kind != "webdav" {
        return Outcome::error(
            "mods",
            &LinkError::config(format!(
                "同步方式「{profile_id}」类型 {} 暂不支持",
                storage.kind
            )),
            None,
            None,
        );
    }
    let endpoint = match crate::webdav::Endpoint::parse(&storage.url) {
        Ok(endpoint) => endpoint,
        Err(error) => return Outcome::error("mods", &error, None, None),
    };
    let target = crate::webdav::DavTarget {
        endpoint: &endpoint,
        remote_root: &storage.remote_root,
        username: &storage.username,
        password: &storage.password,
        ca_cert: &storage.ca_cert,
        insecure: storage.insecure,
        retries: 0,
        timeout_secs: config.sync_cfg().timeout_secs.clamp(1, 10),
        label: "mods",
    };
    let client = match crate::webdav::DavClient::from_target(&target, rep) {
        Ok(client) => client,
        Err(error) => return Outcome::error("mods", &error, None, None),
    };
    match client.probe(rep) {
        Ok(status) => Outcome::ok("mods", None, format!("同步方式可访问（HTTP {status}）")),
        Err(error) => Outcome::error("mods", &error, None, None),
    }
}

fn parse(args: &[String]) -> Result<ParsedArgs> {
    let mut iter = args.iter().peekable();
    let Some(first) = iter.next() else {
        return Err(LinkError::config("缺少命令，使用 --help 查看用法"));
    };
    let command = match first.as_str() {
        "sync" => Command::Sync,
        "push" => Command::Push,
        "pull" => Command::Pull,
        "status" => Command::Status,
        "net" => {
            let Some(sub) = iter.next() else {
                return Err(LinkError::config("net 需要子命令 status 或 up"));
            };
            match sub.as_str() {
                "status" => Command::NetStatus,
                "test" => Command::NetTest,
                "up" => Command::NetUp,
                other => {
                    return Err(LinkError::config(format!(
                        "未知的 net 子命令 {other:?}（支持 status/test/up）"
                    )));
                }
            }
        }
        "config" => {
            let Some(sub) = iter.next() else {
                return Err(LinkError::config("config 需要子命令 init"));
            };
            match sub.as_str() {
                "init" => Command::ConfigInit,
                other => {
                    return Err(LinkError::config(format!(
                        "未知的 config 子命令 {other:?}（支持 init）"
                    )));
                }
            }
        }
        "help" | "--help" | "-h" => Command::Help,
        "version" | "--version" | "-V" => Command::Version,
        "mods" => {
            let Some(sub) = iter.next() else {
                return Err(LinkError::config("mods 需要子命令 list/check/test/run"));
            };
            match sub.as_str() {
                "list" => Command::ModsList,
                "check" => Command::ModsCheck,
                "test" => Command::ModsTest,
                "run" => Command::ModsRun,
                "boot" => Command::ModsBoot,
                other => {
                    return Err(LinkError::config(format!(
                        "未知的 mods 子命令 {other:?}（支持 list/check/test/run/boot）"
                    )));
                }
            }
        }
        other => {
            return Err(LinkError::config(format!(
                "未知命令 {other:?}（支持 sync/push/pull/status/net/config/mods）"
            )));
        }
    };

    let mut root: Option<PathBuf> = None;
    let mut config: Option<PathBuf> = None;
    let mut rom: Option<PathBuf> = None;
    let mut platform: Option<String> = None;
    let mut emulator: Option<String> = None;
    let mut game: Option<String> = None;
    let mut force = false;
    let mut full = false;
    let mut net_up = false;
    let mut verbose = false;
    let mut timeout: Option<u64> = None;
    let mut resolve: Option<ResolveSpec> = None;
    let mut only: Option<Kind> = None;
    let mut expect = ExpectedVersions::default();
    let mut mod_id: Option<String> = None;
    let mut action_id: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut choices: Vec<(String, String)> = Vec::new();
    let mut json = false;

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(take_value(&mut iter, "--root")?)),
            "--config" => config = Some(PathBuf::from(take_value(&mut iter, "--config")?)),
            "--rom" => rom = Some(PathBuf::from(take_value(&mut iter, "--rom")?)),
            "--platform" => platform = Some(take_value(&mut iter, "--platform")?),
            "--emulator" => emulator = Some(take_value(&mut iter, "--emulator")?),
            "--game" => game = Some(take_value(&mut iter, "--game")?),
            "--timeout" => {
                let value = take_value(&mut iter, "--timeout")?;
                let secs = value.parse::<u64>().map_err(|_| {
                    LinkError::config(format!("--timeout 需要整数秒，收到 {value:?}"))
                })?;
                timeout = Some(secs);
            }
            "--resolve" => {
                let value = take_value(&mut iter, "--resolve")?;
                resolve = Some(parse_resolve_spec(&value)?);
            }
            "--only" => {
                let value = take_value(&mut iter, "--only")?;
                only = Some(Kind::parse(&value).ok_or_else(|| {
                    LinkError::config(format!("--only 只支持 state 或 sav，收到 {value:?}"))
                })?);
            }
            "--expect" => {
                let value = take_value(&mut iter, "--expect")?;
                expect = parse_expect_versions(&value)?;
            }
            "--force" => force = true,
            "--full" => full = true,
            "--net-up" => net_up = true,
            "--verbose" | "-v" => verbose = true,
            "--id" => mod_id = Some(take_value(&mut iter, "--id")?),
            "--action" => action_id = Some(take_value(&mut iter, "--action")?),
            "--profile" => profile = Some(take_value(&mut iter, "--profile")?),
            "--choice" => {
                let value = take_value(&mut iter, "--choice")?;
                let Some((key, option)) = value.split_once('=') else {
                    return Err(LinkError::config(
                        "--choice 需要 key=option 形式，例如 --choice conflict=remote",
                    ));
                };
                if key.is_empty() || option.is_empty() {
                    return Err(LinkError::config(
                        "--choice 的 key 和 option 都不能为空",
                    ));
                }
                if choices.iter().any(|(existing, _)| existing == key) {
                    return Err(LinkError::config(format!(
                        "--choice 的 key {key:?} 重复"
                    )));
                }
                choices.push((key.to_string(), option.to_string()));
            }
            "--json" => json = true,
            "--help" | "-h" => {
                return Ok(ParsedArgs {
                    command: Command::Help,
                    root: PathBuf::from(DEFAULT_ROOT),
                    explicit_root: false,
                    config: None,
                    rom: None,
                    unit: None,
                    mod_id: None,
                    action_id: None,
                    profile: None,
                    choices: Vec::new(),
                    json: false,
                    force: false,
                    full: false,
                    net_up: false,
                    verbose: false,
                    timeout: None,
                    resolve: None,
                    only: None,
                    expect: ExpectedVersions::default(),
                });
            }
            other => {
                return Err(LinkError::config(format!("未知参数 {other:?}")));
            }
        }
    }

    let explicit_root = root.is_some();
    let root = slot_model::resolve_root(root, std::env::var_os("SLOT_ROOT").map(PathBuf::from));

    if mod_id.is_some()
        && !matches!(
            command,
            Command::ModsCheck | Command::ModsBoot | Command::ModsRun
        )
    {
        return Err(LinkError::config("--id 仅用于 mods check / boot / run"));
    }
    if action_id.is_some()
        && !matches!(
            command,
            Command::ModsCheck | Command::ModsBoot | Command::ModsRun
        )
    {
        return Err(LinkError::config("--action 仅用于 mods check / boot / run"));
    }
    if profile.is_some() && command != Command::ModsTest {
        return Err(LinkError::config("--profile 仅用于 mods test"));
    }
    if json && command != Command::ModsList {
        return Err(LinkError::config("--json 仅用于 mods list"));
    }

    if net_up
        && !matches!(
            command,
            Command::Sync | Command::Push | Command::Pull | Command::ModsRun
        )
    {
        return Err(LinkError::config("--net-up 仅用于 sync/push/pull/mods run"));
    }
    if full && !matches!(command, Command::Sync | Command::Push | Command::Pull) {
        return Err(LinkError::config("--full 仅用于 sync/push/pull"));
    }
    if force && !matches!(command, Command::Push | Command::Pull | Command::ConfigInit) {
        return Err(LinkError::config(
            "--force 仅用于 push/pull/config init；sync 冲突请用 --resolve upload|download",
        ));
    }
    if resolve.is_some() && command != Command::Sync {
        return Err(LinkError::config("--resolve 仅用于 sync"));
    }
    if !choices.is_empty() && command != Command::ModsRun {
        return Err(LinkError::config("--choice 仅用于 mods run"));
    }
    if only.is_some() && command != Command::Sync {
        return Err(LinkError::config("--only 仅用于 sync"));
    }
    if !expect.is_empty() && command != Command::Sync {
        return Err(LinkError::config("--expect 仅用于 sync"));
    }
    if rom.is_some()
        && !matches!(
            command,
            Command::Sync
                | Command::Push
                | Command::Pull
                | Command::Status
        )
    {
        return Err(LinkError::config(
            "--rom 仅用于 sync/push/pull/status",
        ));
    }
    if timeout.is_some()
        && !matches!(
            command,
            Command::NetUp | Command::Sync | Command::Push | Command::Pull | Command::ModsRun
        )
    {
        return Err(LinkError::config(
            "--timeout 仅用于 net up / sync / push / pull / mods run",
        ));
    }

    let unit = match command {
        Command::Sync | Command::Push | Command::Pull | Command::Status | Command::ModsRun => {
            let platform = platform
                .as_deref()
                .ok_or_else(|| LinkError::config("缺少 --platform 参数"))?;
            let emulator = emulator
                .as_deref()
                .ok_or_else(|| LinkError::config("缺少 --emulator 参数"))?;
            let game = game
                .as_deref()
                .ok_or_else(|| LinkError::config("缺少 --game 参数"))?;
            Some(Unit::new(platform, emulator, game)?)
        }
        _ => {
            if platform.is_some() || emulator.is_some() || game.is_some() {
                return Err(LinkError::config(
                    "--platform/--emulator/--game 仅用于 sync/push/pull/status/mods run",
                ));
            }
            None
        }
    };

    Ok(ParsedArgs {
        command,
        root,
        explicit_root,
        config,
        rom,
        unit,
        mod_id,
        action_id,
        profile,
        choices,
        json,
        force,
        full,
        net_up,
        verbose,
        timeout,
        resolve,
        only,
        expect,
    })
}

/// Parse `--resolve upload|download` or `--resolve state=upload,sav=download`.
///
/// A bare direction fills every unit; `kind=direction` pairs fill only the
/// units named. A unit left out keeps its conflict instead of guessing.
fn parse_resolve_spec(value: &str) -> Result<ResolveSpec> {
    let direction = |text: &str| -> Result<crate::output::Direction> {
        match text {
            "upload" => Ok(crate::output::Direction::Upload),
            "download" => Ok(crate::output::Direction::Download),
            other => Err(LinkError::config(format!(
                "--resolve 方向只支持 upload 或 download，收到 {other:?}"
            ))),
        }
    };
    if value == "upload" || value == "download" {
        return Ok(ResolveSpec::all(direction(value)?));
    }
    let mut spec = ResolveSpec::default();
    for pair in value.split(',') {
        let (kind, dir) = pair.split_once('=').ok_or_else(|| {
            LinkError::config(format!(
                "--resolve 需要 upload|download 或 state=...,sav=...，收到 {value:?}"
            ))
        })?;
        let direction = direction(dir)?;
        match kind {
            "state" => {
                if spec.state.is_some() {
                    return Err(LinkError::config("--resolve 重复指定 state"));
                }
                spec.state = Some(direction);
            }
            "sav" => {
                if spec.sav.is_some() {
                    return Err(LinkError::config("--resolve 重复指定 sav"));
                }
                spec.sav = Some(direction);
            }
            other => {
                return Err(LinkError::config(format!(
                    "--resolve 单元只支持 state 或 sav，收到 {other:?}"
                )));
            }
        }
    }
    if spec.is_empty() {
        return Err(LinkError::config("--resolve 没有指定任何单元"));
    }
    Ok(spec)
}

/// Parse `--expect state=12,sav=7` (one or both units).
fn parse_expect_versions(value: &str) -> Result<ExpectedVersions> {
    let mut expect = ExpectedVersions::default();
    for pair in value.split(',') {
        let (kind, version) = pair.split_once('=').ok_or_else(|| {
            LinkError::config(format!(
                "--expect 需要 state=<版本>,sav=<版本>，收到 {value:?}"
            ))
        })?;
        let version: u64 = version.parse().map_err(|_| {
            LinkError::config(format!("--expect 版本必须是正整数，收到 {version:?}"))
        })?;
        if version == 0 {
            return Err(LinkError::config("--expect 版本必须大于 0"));
        }
        match kind {
            "state" => {
                if expect.state.is_some() {
                    return Err(LinkError::config("--expect 重复指定 state"));
                }
                expect.state = Some(version);
            }
            "sav" => {
                if expect.sav.is_some() {
                    return Err(LinkError::config("--expect 重复指定 sav"));
                }
                expect.sav = Some(version);
            }
            other => {
                return Err(LinkError::config(format!(
                    "--expect 单元只支持 state 或 sav，收到 {other:?}"
                )));
            }
        }
    }
    if expect.is_empty() {
        return Err(LinkError::config("--expect 没有指定任何单元"));
    }
    Ok(expect)
}

fn take_value<'a, I>(iter: &mut I, flag: &str) -> Result<String>
where
    I: Iterator<Item = &'a String>,
{
    iter.next()
        .cloned()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| LinkError::config(format!("{flag} 需要一个值")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(args: &[&str]) -> ParsedArgs {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        parse(&args).unwrap()
    }

    fn parse_err(args: &[&str]) -> LinkError {
        let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        parse(&args).unwrap_err()
    }

    #[test]
    fn parses_push_with_chinese_game() {
        let parsed = parse_ok(&[
            "push",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "合金弹头",
            "--root",
            "D:\\tmp\\sd",
            "--force",
            "--net-up",
            "--verbose",
        ]);
        assert_eq!(parsed.command, Command::Push);
        assert_eq!(parsed.unit.unwrap().game, "合金弹头");
        assert_eq!(parsed.root, PathBuf::from("D:\\tmp\\sd"));
        assert!(parsed.force);
        assert!(parsed.net_up);
        assert!(parsed.verbose);
    }

    #[test]
    fn parses_sync_with_resolve_and_net_options() {
        let parsed = parse_ok(&[
            "sync",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "合金弹头",
            "--resolve",
            "upload",
            "--net-up",
            "--timeout",
            "15",
        ]);
        assert_eq!(parsed.command, Command::Sync);
        assert_eq!(parsed.unit.unwrap().game, "合金弹头");
        assert_eq!(
            parsed.resolve,
            Some(ResolveSpec::all(crate::output::Direction::Upload))
        );
        assert!(parsed.net_up);
        assert!(!parsed.force);
        assert_eq!(parsed.timeout, Some(15));

        let parsed = parse_ok(&[
            "sync",
            "--platform",
            "GB",
            "--emulator",
            "mgba",
            "--game",
            "g",
            "--resolve",
            "download",
        ]);
        assert_eq!(
            parsed.resolve,
            Some(ResolveSpec::all(crate::output::Direction::Download))
        );

        let parsed = parse_ok(&[
            "sync",
            "--platform",
            "GB",
            "--emulator",
            "mgba",
            "--game",
            "g",
        ]);
        assert_eq!(parsed.resolve, None);
    }

    #[test]
    fn parses_full_flag_for_transfer_commands() {
        let parsed = parse_ok(&[
            "push",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "g",
            "--full",
        ]);
        assert!(parsed.full);
        let parsed = parse_ok(&[
            "sync",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "g",
            "--full",
        ]);
        assert!(parsed.full);
        let parsed = parse_ok(&[
            "pull",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "g",
        ]);
        assert!(!parsed.full);
        // `--full` is a transfer option only.
        assert_eq!(
            parse_err(&[
                "status",
                "--platform",
                "GBA",
                "--emulator",
                "mgba",
                "--game",
                "g",
                "--full",
            ])
            .kind(),
            ErrorKind::Config
        );
        assert_eq!(
            parse_err(&["net", "status", "--full"]).kind(),
            ErrorKind::Config
        );
    }

    #[test]
    fn rejects_bad_sync_arguments() {
        // `--resolve` belongs to sync only.
        assert_eq!(
            parse_err(&[
                "push",
                "--platform",
                "GBA",
                "--emulator",
                "mgba",
                "--game",
                "g",
                "--resolve",
                "upload"
            ])
            .kind(),
            ErrorKind::Config
        );
        // Invalid / missing resolve value.
        assert_eq!(
            parse_err(&[
                "sync",
                "--platform",
                "GBA",
                "--emulator",
                "mgba",
                "--game",
                "g",
                "--resolve",
                "sideways"
            ])
            .kind(),
            ErrorKind::Config
        );
        assert_eq!(parse_err(&["sync", "--resolve"]).kind(), ErrorKind::Config);
        // `sync` does not accept `--force` (conflicts use `--resolve`).
        assert_eq!(
            parse_err(&[
                "sync",
                "--platform",
                "GBA",
                "--emulator",
                "mgba",
                "--game",
                "g",
                "--force"
            ])
            .kind(),
            ErrorKind::Config
        );
        // Unit arguments are required for sync too.
        assert_eq!(parse_err(&["sync", "--net-up"]).kind(), ErrorKind::Config);
    }

    #[test]
    fn parses_net_and_config_subcommands() {
        assert_eq!(parse_ok(&["net", "status"]).command, Command::NetStatus);
        assert_eq!(parse_ok(&["net", "test"]).command, Command::NetTest);
        assert_eq!(parse_ok(&["net", "up"]).command, Command::NetUp);
        assert_eq!(
            parse_ok(&["net", "up", "--timeout", "12"]).timeout,
            Some(12)
        );
        assert_eq!(parse_ok(&["config", "init"]).command, Command::ConfigInit);
        assert!(parse_ok(&["config", "init", "--force"]).force);
        assert_eq!(parse_ok(&["--help"]).command, Command::Help);
        assert_eq!(parse_ok(&["version"]).command, Command::Version);
    }

    #[test]
    fn parses_mods_run_with_unit_args() {
        let parsed = parse_ok(&[
            "mods",
            "run",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "合金弹头",
            "--net-up",
            "--timeout",
            "12",
        ]);
        assert_eq!(parsed.command, Command::ModsRun);
        assert_eq!(parsed.unit.as_ref().unwrap().game, "合金弹头");
        assert!(parsed.net_up);
        assert_eq!(parsed.timeout, Some(12));
        assert_eq!(
            parse_err(&["mods", "run", "--net-up"]).kind(),
            ErrorKind::Config
        );
        assert_eq!(
            parse_err(&[
                "mods",
                "run",
                "--platform",
                "GBA",
                "--emulator",
                "mgba",
                "--game",
                "g",
                "--only",
                "sav"
            ])
            .kind(),
            ErrorKind::Config
        );
        // The long-press Y picker narrows a run to one mod action.
        let parsed = parse_ok(&[
            "mods",
            "run",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "g",
            "--id",
            "gbastation-battery-sync",
            "--action",
            "sync",
        ]);
        assert_eq!(parsed.command, Command::ModsRun);
        assert_eq!(parsed.mod_id.as_deref(), Some("gbastation-battery-sync"));
        assert_eq!(parsed.action_id.as_deref(), Some("sync"));
    }

    #[test]
    fn parses_mods_boot_without_a_game_unit() {
        let parsed = parse_ok(&[
            "mods",
            "boot",
            "--id",
            "boot",
            "--action",
            "main",
            "--root",
            "/tmp/sd",
        ]);
        assert_eq!(parsed.command, Command::ModsBoot);
        assert!(parsed.unit.is_none());
        assert_eq!(parsed.mod_id.as_deref(), Some("boot"));
        assert_eq!(parsed.action_id.as_deref(), Some("main"));
        assert_eq!(
            parse_err(&[
                "mods",
                "boot",
                "--platform",
                "GBA",
                "--id",
                "boot",
                "--action",
                "main"
            ])
            .kind(),
            ErrorKind::Config
        );
    }

    #[test]
    fn root_defaults_to_slot_root_then_mnt_sdcard() {
        // Parsing reads the process environment; only assert the explicit and
        // fallback paths here (tests must not mutate global env).
        let parsed = parse_ok(&[
            "pull",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "g",
        ]);
        if std::env::var_os("SLOT_ROOT").is_none() {
            assert_eq!(parsed.root, PathBuf::from(DEFAULT_ROOT));
        }
        assert!(!parsed.explicit_root);
        let parsed = parse_ok(&[
            "pull",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "g",
            "--root",
            "/x",
        ]);
        assert_eq!(parsed.root, PathBuf::from("/x"));
        assert!(parsed.explicit_root);
    }

    #[test]
    fn rejects_bad_arguments() {
        assert_eq!(
            parse_err(&["push", "--platform", "GBA"]).kind(),
            ErrorKind::Config
        );
        assert_eq!(parse_err(&["unknown"]).kind(), ErrorKind::Config);
        assert_eq!(parse_err(&["net"]).kind(), ErrorKind::Config);
        assert_eq!(parse_err(&["config"]).kind(), ErrorKind::Config);
        assert_eq!(
            parse_err(&[
                "push",
                "--platform",
                "GBA",
                "--emulator",
                "mgba",
                "--game",
                "g",
                "--timeout",
                "x"
            ])
            .kind(),
            ErrorKind::Config
        );
        assert_eq!(
            parse_err(&[
                "status",
                "--platform",
                "GBA",
                "--emulator",
                "mgba",
                "--game",
                "g",
                "--force"
            ])
            .kind(),
            ErrorKind::Config
        );
        assert_eq!(
            parse_err(&["net", "status", "--net-up"]).kind(),
            ErrorKind::Config
        );
        assert_eq!(parse_err(&["push", "--wat"]).kind(), ErrorKind::Config);
    }

    #[test]
    fn config_init_dispatch_reports_created_and_existing() {
        let root =
            std::env::temp_dir().join(format!("linkslot-cli-{}", crate::util::unique_suffix()));
        std::fs::create_dir_all(&root).unwrap();
        let args: Vec<OsString> = ["config", "init", "--root"]
            .iter()
            .map(OsString::from)
            .chain(std::iter::once(OsString::from(root.as_os_str())))
            .collect();
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute_os(&args, &mut out, &mut err);
        assert_eq!(code, 0);
        let text = String::from_utf8(std::mem::take(&mut out)).unwrap();
        assert!(text.contains("LINKSLOT_ACTION=config"));
        assert!(text.contains("LINKSLOT_RESULT=ok"));
        assert!(text.contains("LINKSLOT_ERROR=none"));

        out.clear();
        let code = execute_os(&args, &mut out, &mut err);
        assert_eq!(code, 0);
        let text = String::from_utf8(std::mem::take(&mut out)).unwrap();
        assert!(text.contains("LINKSLOT_RESULT=noop"));
        assert!(text.contains("LINKSLOT_DETAIL=配置已存在"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn config_errors_exit_3_with_contract_block() {
        let root =
            std::env::temp_dir().join(format!("linkslot-cli-{}", crate::util::unique_suffix()));
        std::fs::create_dir_all(&root).unwrap();
        let args: Vec<String> = vec![
            "push".to_string(),
            "--platform".to_string(),
            "GBA".to_string(),
            "--emulator".to_string(),
            "mgba".to_string(),
            "--game".to_string(),
            "合金弹头".to_string(),
            "--root".to_string(),
            root.to_string_lossy().to_string(),
        ];
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = execute(&args, &mut out, &mut err);
        assert_eq!(code, 3);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("LINKSLOT_ACTION=push"));
        assert!(text.contains("LINKSLOT_RESULT=error"));
        assert!(text.contains("LINKSLOT_ERROR=config"));
        assert!(text.contains("config init"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- v0.0.10 per-unit sync options -----------------------------------

    #[test]
    fn parses_only_state_and_sav() {
        let parsed = parse_ok(&[
            "sync",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "g",
            "--only",
            "state",
        ]);
        assert_eq!(parsed.only, Some(Kind::State));
        let parsed = parse_ok(&[
            "sync",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "g",
            "--only",
            "sav",
        ]);
        assert_eq!(parsed.only, Some(Kind::Sav));

        assert_eq!(
            parse_err(&["sync", "--platform", "GBA", "--only", "cheats"]).kind(),
            ErrorKind::Config
        );
        // `--only` is sync-only.
        assert_eq!(
            parse_err(&[
                "push",
                "--platform",
                "GBA",
                "--emulator",
                "mgba",
                "--game",
                "g",
                "--only",
                "state"
            ])
            .kind(),
            ErrorKind::Config
        );
    }

    #[test]
    fn parses_resolve_bare_and_per_unit() {
        let parsed = parse_ok(&[
            "sync",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "g",
            "--resolve",
            "upload",
        ]);
        assert_eq!(
            parsed.resolve,
            Some(ResolveSpec::all(crate::output::Direction::Upload))
        );

        let parsed = parse_ok(&[
            "sync",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "g",
            "--resolve",
            "state=upload,sav=download",
        ]);
        assert_eq!(
            parsed.resolve,
            Some(ResolveSpec {
                state: Some(crate::output::Direction::Upload),
                sav: Some(crate::output::Direction::Download),
            })
        );

        let parsed = parse_ok(&[
            "sync",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "g",
            "--resolve",
            "sav=download",
        ]);
        assert_eq!(
            parsed.resolve,
            Some(ResolveSpec {
                state: None,
                sav: Some(crate::output::Direction::Download),
            })
        );

        for bad in [
            "state=sideways",
            "cheats=upload",
            "state=upload,state=download",
            "state=upload,sav=upload,sav=download",
            "state",
        ] {
            assert_eq!(
                parse_resolve_spec(bad).unwrap_err().kind(),
                ErrorKind::Config,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn parses_expect_single_and_both() {
        let parsed = parse_ok(&[
            "sync",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "g",
            "--expect",
            "state=12",
        ]);
        assert_eq!(
            parsed.expect,
            ExpectedVersions {
                state: Some(12),
                sav: None,
            }
        );

        let parsed = parse_ok(&[
            "sync",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "g",
            "--expect",
            "state=12,sav=7",
        ]);
        assert_eq!(
            parsed.expect,
            ExpectedVersions {
                state: Some(12),
                sav: Some(7),
            }
        );

        for bad in ["12", "state=0", "state=x", "state=1,state=2", "cheats=1"] {
            assert_eq!(
                parse_expect_versions(bad).unwrap_err().kind(),
                ErrorKind::Config,
                "{bad:?}"
            );
        }
        // `--expect` is sync-only.
        assert_eq!(
            parse_err(&[
                "pull",
                "--platform",
                "GBA",
                "--emulator",
                "mgba",
                "--game",
                "g",
                "--expect",
                "state=1"
            ])
            .kind(),
            ErrorKind::Config
        );
    }

    #[test]
    fn bad_game_name_is_config_error() {
        let args = vec![
            "push".to_string(),
            "--platform".to_string(),
            "GBA".to_string(),
            "--emulator".to_string(),
            "mgba".to_string(),
            "--game".to_string(),
            "a/b".to_string(),
        ];
        let err = parse(&args).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
    }

    // --rom / ROM discovery -------------------------------------------------

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "linkslot-cli-{tag}-{}",
            crate::util::unique_suffix()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_rom(root: &Path, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn unit_args(root: &Path, game: &str, rom: Option<PathBuf>) -> ParsedArgs {
        ParsedArgs {
            command: Command::Push,
            root: root.to_path_buf(),
            explicit_root: true,
            config: None,
            rom,
            unit: Some(Unit::new("GBA", "mgba", game).unwrap()),
            mod_id: None,
            action_id: None,
            profile: None,
            choices: Vec::new(),
            json: false,
            force: false,
            full: false,
            net_up: false,
            verbose: false,
            timeout: None,
            resolve: None,
            only: None,
            expect: ExpectedVersions::default(),
        }
    }

    #[test]
    fn parses_rom_only_for_unit_commands() {
        let parsed = parse_ok(&[
            "push",
            "--platform",
            "GBA",
            "--emulator",
            "mgba",
            "--game",
            "g",
            "--rom",
            "D:\\roms\\g.gba",
        ]);
        assert_eq!(parsed.rom, Some(PathBuf::from("D:\\roms\\g.gba")));
        assert_eq!(
            parse_ok(&[
                "status",
                "--platform",
                "GBA",
                "--emulator",
                "mgba",
                "--game",
                "g",
                "--rom",
                "/roms/g.gba",
            ])
            .rom,
            Some(PathBuf::from("/roms/g.gba"))
        );
        // `--rom` belongs to the unit commands only.
        for args in [
            vec!["net", "status", "--rom", "x.gba"],
            vec!["net", "up", "--rom", "x.gba"],
            vec!["config", "init", "--rom", "x.gba"],
        ] {
            assert_eq!(parse_err(&args).kind(), ErrorKind::Config, "{args:?}");
        }
        // Missing value is a config error too.
        assert_eq!(
            parse_err(&["push", "--platform", "GBA", "--rom"]).kind(),
            ErrorKind::Config
        );
        assert_eq!(parse_ok(&["--help"]).rom, None);
    }

    #[test]
    fn explicit_rom_hashes_the_given_file() {
        let dir = temp_root("rom-explicit");
        let root = dir.join("sd");
        let rom = write_rom(&dir, "elsewhere/custom.gba", b"explicit-rom-bytes");
        let parsed = unit_args(&root, "合金弹头", Some(rom.clone()));
        let unit = require_unit(&parsed).unwrap();
        let expected = hash::sha256_file(&rom).unwrap();
        assert_eq!(unit.rom_sha256.as_deref(), Some(expected.as_str()));
        // Local directory keeps the game name.
        assert_eq!(unit.local_dir(&root), root.join("States/GBA/mgba/合金弹头"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_discovers_the_unique_matching_rom() {
        let dir = temp_root("rom-auto");
        let root = dir.join("sd");
        let rom = write_rom(
            &root,
            "Games/GBA/sub folder/合金弹头.gba",
            b"auto-rom-bytes",
        );
        let parsed = unit_args(&root, "合金弹头", None);
        let unit = require_unit(&parsed).unwrap();
        assert_eq!(
            unit.rom_sha256.as_deref(),
            Some(hash::sha256_file(&rom).unwrap().as_str())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_rom_is_a_config_error_that_suggests_rom_flag() {
        let dir = temp_root("rom-missing");
        let root = dir.join("sd");
        std::fs::create_dir_all(root.join("Games/GBA")).unwrap();
        let err = require_unit(&unit_args(&root, "合金弹头", None)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.message().contains("未找到"), "{}", err.message());
        assert!(err.message().contains("--rom"), "{}", err.message());
        // A ROM with the wrong extension is ignored as well.
        write_rom(&root, "Games/GBA/合金弹头.gb", b"wrong-ext");
        assert!(require_unit(&unit_args(&root, "合金弹头", None)).is_err());
        // Hidden directories are skipped like the Slot library scan.
        write_rom(&root, "Games/GBA/.trash/合金弹头.gba", b"hidden");
        assert!(require_unit(&unit_args(&root, "合金弹头", None)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_roms_are_a_config_error_listing_candidates() {
        let dir = temp_root("rom-duplicate");
        let root = dir.join("sd");
        write_rom(&root, "Games/GBA/main/合金弹头.gba", b"one");
        write_rom(&root, "Games/GBA/sub/合金弹头.gba", b"two");
        let err = require_unit(&unit_args(&root, "合金弹头", None)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.message().contains("找到 2 个"), "{}", err.message());
        assert!(err.message().contains("main"), "{}", err.message());
        assert!(err.message().contains("sub"), "{}", err.message());
        assert!(err.message().contains("--rom"), "{}", err.message());
        // The same stem with both GB extensions is ambiguous for GB too.
        write_rom(&root, "Games/GB/Tetris.gb", b"gb");
        write_rom(&root, "Games/GB/Tetris.gbC", b"gbc");
        let err = discover_rom(&root, "GB", "Tetris").unwrap_err();
        assert!(err.message().contains("找到 2 个"), "{}", err.message());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discovery_matches_platform_extensions_case_insensitively() {
        let dir = temp_root("rom-ext");
        let root = dir.join("sd");
        let gba = write_rom(&root, "Games/GBA/Game.GBA", b"gba");
        assert_eq!(discover_rom(&root, "GBA", "Game").unwrap(), gba);
        let gb = write_rom(&root, "Games/GB/Tetris.GB", b"gb");
        assert_eq!(discover_rom(&root, "GB", "Tetris").unwrap(), gb);
        let gbc = write_rom(&root, "Games/GBC/Colour.gbC", b"gbc");
        assert_eq!(discover_rom(&root, "GBC", "Colour").unwrap(), gbc);
        // GBA does not accept .gb, GB/GBC do not accept .gba.
        write_rom(&root, "Games/GBA/OnlyGb.gb", b"gb");
        assert!(discover_rom(&root, "GBA", "OnlyGb").is_err());
        write_rom(&root, "Games/GB/OnlyGba.gba", b"gba");
        assert!(discover_rom(&root, "GB", "OnlyGba").is_err());
        // Stem comparison is exact (case-sensitive).
        write_rom(&root, "Games/GBA/Case.gba", b"case");
        assert!(discover_rom(&root, "GBA", "case").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Famicom shelf is `.nes` and nothing else. Without this row a
    /// `--platform FC` sync would look for `.gb/.gbc` (the old catch-all), find
    /// nothing, and report extensions the machine does not even take.
    #[test]
    fn the_famicom_shelf_is_nintendo_entertainment_system_files() {
        let dir = temp_root("rom-ext-fc");
        let root = dir.join("sd");
        let nes = write_rom(&root, "Games/FC/Mario.NES", b"nes");
        assert_eq!(rom_extensions("FC"), ["nes"]);
        assert_eq!(discover_rom(&root, "FC", "Mario").unwrap(), nes);

        // A Game Boy cart in the Famicom folder is not an FC cart.
        write_rom(&root, "Games/FC/Tetris.gb", b"gb");
        assert!(discover_rom(&root, "FC", "Tetris").is_err());
        // And the Famicom's extension does not leak into the handhelds.
        write_rom(&root, "Games/GB/Mario.nes", b"nes");
        assert!(discover_rom(&root, "GB", "Mario").is_err());
        // A platform the table does not know finds nothing rather than
        // borrowing another machine's extensions.
        assert!(rom_extensions("NES").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Mega Drive shelf has all four MD spellings; without this row a
    /// `--platform MD` sync looks for no extension at all and reports a
    /// missing ROM even though the card has one.
    #[test]
    fn the_mega_drive_shelf_has_its_own_extensions() {
        let dir = temp_root("rom-ext-md");
        let root = dir.join("sd");
        let md = write_rom(&root, "Games/MD/Sonic.md", b"md");
        assert_eq!(rom_extensions("MD"), ["md", "gen", "smd", "bin"]);
        assert_eq!(discover_rom(&root, "MD", "Sonic").unwrap(), md);

        write_rom(&root, "Games/MD/Streets.bin", b"bin");
        assert_eq!(
            discover_rom(&root, "MD", "Streets").unwrap(),
            root.join("Games/MD/Streets.bin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_rom_is_a_file_error_with_exit_code_4() {
        let dir = temp_root("rom-unreadable");
        let root = dir.join("sd");
        let missing = dir.join("nope.gba");
        let err = require_unit(&unit_args(&root, "合金弹头", Some(missing))).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Fs);
        assert_eq!(err.kind().exit_code(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
