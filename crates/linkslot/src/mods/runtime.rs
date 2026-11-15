//! The actual Lua runtime: per-script state, sandbox and host functions.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use mlua::{HookTriggers, Lua, LuaOptions, LuaSerdeExt, MultiValue, StdLib, Table, Value, VmState};

use super::host_files::{Entry, LocalFiles};
use super::host_http::HostHttp;
use super::host_webdav::RemoteClients;
use super::{
    HookContext, LoadedAction, ManualReportValue, ManualStatus, RemoteProfile, Timing,
    LUA_MEMORY_LIMIT, MANUAL_SCRIPT_TIMEOUT, MAX_LOG_LINES, MAX_REMOTE_REQUESTS, MAX_REMOTE_TOTAL,
    SCRIPT_TIMEOUT,
    SYNC_MAX_REMOTE_REQUESTS, SYNC_MAX_REMOTE_TOTAL,
};
use crate::error::ErrorKind;
use crate::output::Reporter;
use crate::webdav::Endpoint;
use slot_mods::{ChoiceKind, ChoiceOption, ChoiceRequest};

/// Internal sentinel used to unwind a manual action that needs a user choice.
const CHOICE_SENTINEL: &str = "\u{0}LINKSLOT_MOD_CHOICE";

pub(crate) enum ScriptEnd {
    Done(Option<ManualReportValue>),
    Choice(ChoiceRequest),
}

struct LogLine {
    level: &'static str,
    message: String,
}

struct HostIo {
    local: LocalFiles,
    remote: RemoteClients,
    http: Option<HostHttp>,
    http_error: Option<String>,
    mod_config: Option<serde_json::Value>,
    config_error: Option<String>,
    manual_report: Option<ManualReportValue>,
    choices: Vec<(String, String)>,
    pending_choice: Option<ChoiceRequest>,
    ui_seq: usize,
    progress_last: Option<f32>,
    progress_at: Option<Instant>,
    logs: Vec<LogLine>,
}

impl HostIo {
    fn log(&mut self, level: &'static str, message: impl Into<String>) {
        if self.logs.len() >= MAX_LOG_LINES {
            return;
        }
        self.logs.push(LogLine {
            level,
            message: sanitize_log(message.into()),
        });
    }
}

pub fn check_source(source: &str, id: &str) -> Result<(), String> {
    let lua = Lua::new_with(
        StdLib::COROUTINE | StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .map_err(|error| format!("无法创建 Lua 状态：{error}"))?;
    lua.load(source)
        .set_name(format!("@mods/{id}/main.lua"))
        .into_function()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub fn run_one(
    loaded: &LoadedAction,
    ctx: &HookContext<'_>,
    profiles: &[RemoteProfile],
    rep: &mut Reporter,
) -> Result<ScriptEnd, String> {
    let started = Instant::now();
    let script_timeout = if ctx.timing == Timing::Manual {
        MANUAL_SCRIPT_TIMEOUT
    } else {
        SCRIPT_TIMEOUT
    };
    let lua = Lua::new_with(
        StdLib::COROUTINE | StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .map_err(|error| format!("无法创建 Lua 状态：{error}"))?;
    lua.set_memory_limit(LUA_MEMORY_LIMIT)
        .map_err(|error| format!("无法设置 Lua 内存上限：{error}"))?;
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(10_000),
        move |_lua, _debug| {
            if started.elapsed() >= script_timeout {
                Err(mlua::Error::RuntimeError(format!(
                    "动作执行超时（{} 秒）",
                    script_timeout.as_secs()
                )))
            } else {
                Ok(VmState::Continue)
            }
        },
    )
    .map_err(|error| format!("无法设置 Lua 超时钩子：{error}"))?;

    let (mod_config, config_error) = load_mod_config(ctx.root, &loaded.mod_id);
    let (http, http_error) = build_http(mod_config.as_ref(), config_error.as_deref(), ctx, rep);
    let io = Rc::new(RefCell::new(HostIo {
        local: LocalFiles::new(ctx.root, &loaded.mod_id),
        remote: RemoteClients::new(
            ctx.root,
            profiles,
            if ctx.timing == Timing::AfterSync {
                SYNC_MAX_REMOTE_REQUESTS
            } else {
                MAX_REMOTE_REQUESTS
            },
            if ctx.timing == Timing::AfterSync {
                SYNC_MAX_REMOTE_TOTAL
            } else {
                MAX_REMOTE_TOTAL
            },
        ),
        http,
        http_error,
        mod_config,
        config_error,
        manual_report: None,
        choices: ctx.choices.to_vec(),
        pending_choice: None,
        ui_seq: 0,
        progress_last: None,
        progress_at: None,
        logs: Vec::new(),
    }));

    install_sandbox(&lua, &io)?;
    install_webdav(&lua, &io, ctx.active_storage)?;
    install_http(&lua, &io)?;
    install_context(
        &lua,
        ctx,
        &io,
        &loaded.mod_id,
        &loaded.action_id,
        &loaded.action_name,
    )?;
    if ctx.timing == Timing::Manual {
        if let Some(unit) = ctx.unit {
            let library = lua
                .create_table()
                .map_err(|error| format!("无法创建 library 表：{error}"))?;
            install_library(&lua, &library, ctx.root, unit)?;
            lua.globals()
                .set("library", library)
                .map_err(|error| format!("无法设置 library：{error}"))?;
        }
    }
    install_report(&lua, &io, ctx.timing == Timing::Manual)?;
    if ctx.timing == Timing::Manual {
        install_ui(&lua, &io)?;
    }

    let result = lua
        .load(&loaded.source)
        .set_name(format!(
            "@mods/{}/actions/{}/main.lua",
            loaded.mod_id, loaded.action_id
        ))
        .exec();

    let logs = {
        let io = io.borrow();
        io.logs
            .iter()
            .map(|line| (line.level, line.message.clone()))
            .collect::<Vec<_>>()
    };
    for (level, message) in logs {
        match level {
            "info" | "debug" => rep.info(format!(
                "[mods:{}/{}] {message}",
                loaded.mod_id, loaded.action_id
            )),
            _ => rep.warn(format!(
                "[mods:{}/{}] {message}",
                loaded.mod_id, loaded.action_id
            )),
        }
    }

    let report = io.borrow_mut().manual_report.take();
    let choice = io.borrow_mut().pending_choice.take();
    if let Some(choice) = choice {
        return Ok(ScriptEnd::Choice(choice));
    }
    result
        .map(|_| ScriptEnd::Done(report))
        .map_err(|error| error.to_string())
}

fn load_mod_config(root: &Path, mod_id: &str) -> (Option<serde_json::Value>, Option<String>) {
    match super::read_mod_config(root, mod_id) {
        Ok(config) => (config, None),
        Err(error) => (None, Some(error)),
    }
}

fn build_http(
    config: Option<&serde_json::Value>,
    config_error: Option<&str>,
    ctx: &HookContext<'_>,
    rep: &mut Reporter,
) -> (Option<HostHttp>, Option<String>) {
    if let Some(error) = config_error {
        return (None, Some(format!("[config] {error}")));
    }
    let Some(config) = config else {
        return (
            None,
            Some("[config] 未找到 config.json，请先在 mod 管理页配置 GBAStation 地址".to_string()),
        );
    };
    let Some(base_url) = config
        .get("base_url")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|url| !url.is_empty())
    else {
        return (
            None,
            Some("[config] 未配置 base_url，请先在 mod 管理页填写 GBAStation 地址".to_string()),
        );
    };
    let endpoint = match Endpoint::parse(base_url) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            return (
                None,
                Some(format!("[config] base_url 无效：{}", error.message())),
            )
        }
    };
    let timeout = if ctx.timing == Timing::Manual {
        60
    } else {
        ctx.config.sync_cfg().timeout_secs.clamp(1, 10)
    };
    match HostHttp::new(endpoint, timeout, rep) {
        Ok(http) => (Some(http), None),
        Err(error) => (
            None,
            Some(format!("[{}] {}", error.kind().as_str(), error.message())),
        ),
    }
}

fn install_sandbox(lua: &Lua, io: &Rc<RefCell<HostIo>>) -> Result<(), String> {
    let globals = lua.globals();
    for name in [
        "dofile", "loadfile", "load", "require", "package", "io", "os", "debug",
    ] {
        globals
            .set(name, Value::Nil)
            .map_err(|error| format!("无法清理 Lua 全局 {name}：{error}"))?;
    }

    let print_io = io.clone();
    let print = lua
        .create_function(move |_lua, args: MultiValue| {
            let mut parts = Vec::new();
            for value in args {
                parts.push(value_to_string(&value));
            }
            print_io.borrow_mut().log("info", parts.join(" "));
            Ok(())
        })
        .map_err(|error| format!("无法创建 print：{error}"))?;
    globals
        .set("print", print)
        .map_err(|error| format!("无法设置 print：{error}"))?;

    let files = lua
        .create_table()
        .map_err(|error| format!("无法创建 files 表：{error}"))?;
    install_files(lua, &files, io)?;
    globals
        .set("files", files)
        .map_err(|error| format!("无法设置 files：{error}"))?;

    let json = lua
        .create_table()
        .map_err(|error| format!("无法创建 json 表：{error}"))?;
    install_json(lua, &json)?;
    globals
        .set("json", json)
        .map_err(|error| format!("无法设置 json：{error}"))?;

    let hash = lua
        .create_table()
        .map_err(|error| format!("无法创建 hash 表：{error}"))?;
    install_hash(lua, &hash, io)?;
    globals
        .set("hash", hash)
        .map_err(|error| format!("无法设置 hash：{error}"))?;

    let zip = lua
        .create_table()
        .map_err(|error| format!("无法创建 zip 表：{error}"))?;
    install_zip(lua, &zip)?;
    globals
        .set("zip", zip)
        .map_err(|error| format!("无法设置 zip：{error}"))?;

    let log = lua
        .create_table()
        .map_err(|error| format!("无法创建 log 表：{error}"))?;
    for (name, level) in [
        ("info", "info"),
        ("warn", "warn"),
        ("error", "error"),
        ("debug", "debug"),
    ] {
        let log_io = io.clone();
        let function = lua
            .create_function(move |_lua, message: String| {
                log_io.borrow_mut().log(level, message);
                Ok(())
            })
            .map_err(|error| format!("无法创建 log.{name}：{error}"))?;
        log.set(name, function)
            .map_err(|error| format!("无法设置 log.{name}：{error}"))?;
    }
    globals
        .set("log", log)
        .map_err(|error| format!("无法设置 log：{error}"))?;

    Ok(())
}

fn install_files(lua: &Lua, files: &Table, io: &Rc<RefCell<HostIo>>) -> Result<(), String> {
    let read_io = io.clone();
    let read = lua
        .create_function(move |lua, rel: String| {
            let result = { read_io.borrow().local.read(&rel) };
            match result {
                Ok(Some(bytes)) => Ok(Some(lua.create_string(&bytes).map_err(|error| {
                    mlua::Error::RuntimeError(format!("创建字符串失败：{error}"))
                })?)),
                Ok(None) => Ok(None),
                Err(error) => Err(mlua::Error::RuntimeError(error)),
            }
        })
        .map_err(|error| format!("无法创建 files.read：{error}"))?;
    files
        .set("read", read)
        .map_err(|error| format!("无法设置 files.read：{error}"))?;

    let write_io = io.clone();
    let write = lua
        .create_function(move |_lua, (rel, data): (String, mlua::String)| {
            if write_io.borrow().pending_choice.is_some() {
                return Err(mlua::Error::RuntimeError(
                    "mods.choose 之后不能写文件".to_string(),
                ));
            }
            let bytes = data.as_bytes().to_vec();
            { write_io.borrow().local.write(&rel, &bytes) }.map_err(mlua::Error::RuntimeError)?;
            Ok(true)
        })
        .map_err(|error| format!("无法创建 files.write：{error}"))?;
    files
        .set("write", write)
        .map_err(|error| format!("无法设置 files.write：{error}"))?;

    let list_io = io.clone();
    let list = lua
        .create_function(move |lua, rel: String| {
            let entries =
                { list_io.borrow().local.list(&rel) }.map_err(mlua::Error::RuntimeError)?;
            entries_table(lua, entries)
        })
        .map_err(|error| format!("无法创建 files.list：{error}"))?;
    files
        .set("list", list)
        .map_err(|error| format!("无法设置 files.list：{error}"))?;

    let stat_io = io.clone();
    let stat = lua
        .create_function(move |lua, rel: String| {
            let entry = { stat_io.borrow().local.stat(&rel) }.map_err(mlua::Error::RuntimeError)?;
            match entry {
                Some(entry) => Ok(Some(entry_table(lua, entry)?)),
                None => Ok(None),
            }
        })
        .map_err(|error| format!("无法创建 files.stat：{error}"))?;
    files
        .set("stat", stat)
        .map_err(|error| format!("无法设置 files.stat：{error}"))?;

    let exists_io = io.clone();
    let exists = lua
        .create_function(move |_lua, rel: String| {
            exists_io
                .borrow()
                .local
                .exists(&rel)
                .map_err(mlua::Error::RuntimeError)
        })
        .map_err(|error| format!("无法创建 files.exists：{error}"))?;
    files
        .set("exists", exists)
        .map_err(|error| format!("无法设置 files.exists：{error}"))?;

    let mkdir_io = io.clone();
    let mkdir = lua
        .create_function(move |_lua, rel: String| {
            if mkdir_io.borrow().pending_choice.is_some() {
                return Err(mlua::Error::RuntimeError(
                    "mods.choose 之后不能创建目录".to_string(),
                ));
            }
            mkdir_io
                .borrow()
                .local
                .mkdir(&rel)
                .map_err(mlua::Error::RuntimeError)?;
            Ok(true)
        })
        .map_err(|error| format!("无法创建 files.mkdir：{error}"))?;
    files
        .set("mkdir", mkdir)
        .map_err(|error| format!("无法设置 files.mkdir：{error}"))?;

    Ok(())
}

/// Maximum UTF-8 text a manual mod may write into one game-info asset.
const MAX_LIBRARY_TEXT: usize = 1024 * 1024;

fn install_library(
    lua: &Lua,
    library: &Table,
    root: &Path,
    unit: &crate::unit::Unit,
) -> Result<(), String> {
    let root = root.to_path_buf();
    let platform = unit.platform.clone();
    let game = unit.game.clone();

    let exists_root = root.clone();
    let exists_platform = platform.clone();
    let exists_game = game.clone();
    let exists = lua
        .create_function(move |_lua, kind: String| {
            let path = library_target(&exists_root, &exists_platform, &exists_game, &kind)
                .map_err(mlua::Error::RuntimeError)?;
            Ok(path.is_file())
        })
        .map_err(|error| format!("无法创建 library.exists：{error}"))?;
    library
        .set("exists", exists)
        .map_err(|error| format!("无法设置 library.exists：{error}"))?;

    let write_root = root;
    let write_platform = platform;
    let write_game = game;
    let write = lua
        .create_function(move |_lua, (kind, text): (String, mlua::String)| {
            let bytes = text.as_bytes().to_vec();
            if bytes.len() > MAX_LIBRARY_TEXT {
                return Err(mlua::Error::RuntimeError(format!(
                    "游戏资料超过 {} KiB 上限",
                    MAX_LIBRARY_TEXT / 1024
                )));
            }
            let target = library_target(&write_root, &write_platform, &write_game, &kind)
                .map_err(mlua::Error::RuntimeError)?;
            // Create the directory first so canonicalizing the parent can
            // prove that a symlink did not redirect the write outside the card.
            let parent = target
                .parent()
                .ok_or_else(|| mlua::Error::RuntimeError("资料路径没有父目录".to_string()))?;
            std::fs::create_dir_all(parent)
                .map_err(|error| mlua::Error::RuntimeError(format!("创建资料目录失败：{error}")))?;
            let canonical_root = write_root
                .canonicalize()
                .map_err(|error| mlua::Error::RuntimeError(format!("存储根目录不可用：{error}")))?;
            let canonical_parent = parent
                .canonicalize()
                .map_err(|error| mlua::Error::RuntimeError(format!("资料目录不可用：{error}")))?;
            if !canonical_parent.starts_with(&canonical_root) {
                return Err(mlua::Error::RuntimeError("资料路径越界".to_string()));
            }
            crate::store::atomic_write(&target, &bytes)
                .map_err(|error| mlua::Error::RuntimeError(format!("写入游戏资料失败：{error}")))?;
            Ok(true)
        })
        .map_err(|error| format!("无法创建 library.write：{error}"))?;
    library
        .set("write", write)
        .map_err(|error| format!("无法设置 library.write：{error}"))?;
    Ok(())
}

fn library_target(
    root: &Path,
    platform: &str,
    game: &str,
    kind: &str,
) -> Result<std::path::PathBuf, String> {
    crate::unit::validate_platform(platform).map_err(|error| error.message().to_string())?;
    crate::unit::validate_game(game).map_err(|error| error.message().to_string())?;
    let (directory, extension) = match kind {
        "intro" => ("Info", "txt"),
        "guide" => ("Guides", "txt"),
        other => {
            return Err(format!(
                "未知的游戏资料类型 {other:?}（只支持 intro / guide）"
            ))
        }
    };
    Ok(root
        .join(directory)
        .join(platform)
        .join(format!("{game}.{extension}")))
}

fn install_webdav(lua: &Lua, io: &Rc<RefCell<HostIo>>, active: &str) -> Result<(), String> {
    let table = lua
        .create_table()
        .map_err(|error| format!("无法创建 webdav 表：{error}"))?;

    let profiles = profile_tuples(io);
    let profiles_table = lua
        .create_table()
        .map_err(|error| format!("无法创建 webdav.profiles：{error}"))?;
    for (index, (id, name, remote_root)) in profiles.iter().enumerate() {
        let item = lua
            .create_table()
            .map_err(|error| format!("无法创建 profile 表：{error}"))?;
        item.set("id", id.clone())
            .and_then(|_| item.set("name", name.clone()))
            .and_then(|_| item.set("remote_root", remote_root.clone()))
            .map_err(|error| format!("无法填充 profile 表：{error}"))?;
        profiles_table
            .set(index + 1, item)
            .map_err(|error| format!("无法填充 webdav.profiles：{error}"))?;
    }
    let profiles_fn = lua
        .create_function(move |_lua, ()| Ok(profiles_table.clone()))
        .map_err(|error| format!("无法创建 webdav.profiles：{error}"))?;
    table
        .set("profiles", profiles_fn)
        .map_err(|error| format!("无法设置 webdav.profiles：{error}"))?;

    let get_io = io.clone();
    let get_active = active.to_string();
    let get = lua
        .create_function(move |lua, args: MultiValue| {
            let (profile, path) = parse_path_call(args)?;
            let profile = profile.unwrap_or_else(|| get_active.clone());
            let bytes = { get_io.borrow().remote.get(&profile, &path) }
                .map_err(mlua::Error::RuntimeError)?;
            match bytes {
                Some(bytes) => Ok(Some(lua.create_string(&bytes).map_err(|error| {
                    mlua::Error::RuntimeError(format!("创建字符串失败：{error}"))
                })?)),
                None => Ok(None),
            }
        })
        .map_err(|error| format!("无法创建 webdav.get：{error}"))?;
    table
        .set("get", get)
        .map_err(|error| format!("无法设置 webdav.get：{error}"))?;

    let put_io = io.clone();
    let put_active = active.to_string();
    let put = lua
        .create_function(move |_lua, args: MultiValue| {
            let (profile, path, data) = parse_put_call(args)?;
            let profile = profile.unwrap_or_else(|| put_active.clone());
            { put_io.borrow().remote.put(&profile, &path, &data) }
                .map_err(mlua::Error::RuntimeError)?;
            Ok(true)
        })
        .map_err(|error| format!("无法创建 webdav.put：{error}"))?;
    table
        .set("put", put)
        .map_err(|error| format!("无法设置 webdav.put：{error}"))?;

    let put_atomic_io = io.clone();
    let put_atomic_active = active.to_string();
    let put_atomic = lua
        .create_function(move |_lua, args: MultiValue| {
            let (profile, path, data) = parse_put_call(args)?;
            let profile = profile.unwrap_or_else(|| put_atomic_active.clone());
            {
                put_atomic_io
                    .borrow()
                    .remote
                    .put_atomic(&profile, &path, &data)
            }
            .map_err(mlua::Error::RuntimeError)?;
            Ok(true)
        })
        .map_err(|error| format!("无法创建 webdav.put_atomic：{error}"))?;
    table
        .set("put_atomic", put_atomic)
        .map_err(|error| format!("无法设置 webdav.put_atomic：{error}"))?;

    let list_io = io.clone();
    let list_active = active.to_string();
    let list = lua
        .create_function(move |lua, args: MultiValue| {
            let (profile, path) = parse_path_call(args)?;
            let profile = profile.unwrap_or_else(|| list_active.clone());
            let entries = { list_io.borrow().remote.list(&profile, &path) }
                .map_err(mlua::Error::RuntimeError)?;
            match entries {
                Some(entries) => Ok(Some(dav_entries_table(lua, entries)?)),
                None => Ok(None),
            }
        })
        .map_err(|error| format!("无法创建 webdav.list：{error}"))?;
    table
        .set("list", list)
        .map_err(|error| format!("无法设置 webdav.list：{error}"))?;

    let stat_io = io.clone();
    let stat_active = active.to_string();
    let stat = lua
        .create_function(move |lua, args: MultiValue| {
            let (profile, path) = parse_path_call(args)?;
            let profile = profile.unwrap_or_else(|| stat_active.clone());
            let entry = { stat_io.borrow().remote.stat(&profile, &path) }
                .map_err(mlua::Error::RuntimeError)?;
            match entry {
                Some(entry) => Ok(Some(dav_entry_table(lua, entry)?)),
                None => Ok(None),
            }
        })
        .map_err(|error| format!("无法创建 webdav.stat：{error}"))?;
    table
        .set("stat", stat)
        .map_err(|error| format!("无法设置 webdav.stat：{error}"))?;

    let mkdir_io = io.clone();
    let mkdir_active = active.to_string();
    let mkdir = lua
        .create_function(move |_lua, args: MultiValue| {
            let (profile, path) = parse_path_call(args)?;
            let profile = profile.unwrap_or_else(|| mkdir_active.clone());
            { mkdir_io.borrow().remote.mkdir(&profile, &path) }
                .map_err(mlua::Error::RuntimeError)?;
            Ok(true)
        })
        .map_err(|error| format!("无法创建 webdav.mkdir：{error}"))?;
    table
        .set("mkdir", mkdir)
        .map_err(|error| format!("无法设置 webdav.mkdir：{error}"))?;

    lua.globals()
        .set("webdav", table)
        .map_err(|error| format!("无法设置 webdav：{error}"))?;
    Ok(())
}

fn install_http(lua: &Lua, io: &Rc<RefCell<HostIo>>) -> Result<(), String> {
    let http = lua
        .create_table()
        .map_err(|error| format!("无法创建 http 表：{error}"))?;

    let get_io = io.clone();
    let get = lua
        .create_function(move |lua, (segments, query): (Value, Option<Table>)| {
            http_call(lua, &get_io, "GET", segments, query, None, None, None)
        })
        .map_err(|error| format!("无法创建 http.get：{error}"))?;
    http.set("get", get)
        .map_err(|error| format!("无法设置 http.get：{error}"))?;

    let post_io = io.clone();
    let post = lua
        .create_function(
            move |lua,
                  (segments, body, content_type, query): (
                Value,
                mlua::String,
                Option<String>,
                Option<Table>,
            )| {
                http_call(
                    lua,
                    &post_io,
                    "POST",
                    segments,
                    query,
                    Some(body),
                    content_type,
                    None,
                )
            },
        )
        .map_err(|error| format!("无法创建 http.post：{error}"))?;
    http.set("post", post)
        .map_err(|error| format!("无法设置 http.post：{error}"))?;

    let request_io = io.clone();
    let request = lua
        .create_function(
            move |lua, (method, segments, opts): (String, Value, Option<Table>)| {
                let (body, content_type, query, headers) = match opts {
                    Some(table) => (
                        table.get::<Option<mlua::String>>("body")?,
                        table.get::<Option<String>>("content_type")?,
                        table.get::<Option<Table>>("query")?,
                        table.get::<Option<Table>>("headers")?,
                    ),
                    None => (None, None, None, None),
                };
                http_call(
                    lua,
                    &request_io,
                    &method,
                    segments,
                    query,
                    body,
                    content_type,
                    headers,
                )
            },
        )
        .map_err(|error| format!("无法创建 http.request：{error}"))?;
    http.set("request", request)
        .map_err(|error| format!("无法设置 http.request：{error}"))?;

    lua.globals()
        .set("http", http)
        .map_err(|error| format!("无法设置 http：{error}"))?;
    Ok(())
}

fn http_call(
    lua: &Lua,
    io: &Rc<RefCell<HostIo>>,
    method: &str,
    segments: Value,
    query: Option<Table>,
    body: Option<mlua::String>,
    content_type: Option<String>,
    headers: Option<Table>,
) -> mlua::Result<Table> {
    let segments = parse_segments(segments)?;
    let query = parse_query(query)?;
    let headers = parse_headers(headers)?;
    let body = body.map(|value| value.as_bytes().to_vec());
    let result = {
        let mut host = io.borrow_mut();
        if !method.eq_ignore_ascii_case("GET") && host.pending_choice.is_some() {
            return Err(mlua::Error::RuntimeError(
                "mods.choose 之后不能发起写请求".to_string(),
            ));
        }
        let http_error = host.http_error.clone();
        match host.http.as_mut() {
            Some(http) => http.request(
                method,
                &segments,
                &query,
                body.as_deref(),
                content_type.as_deref(),
                &headers,
            ),
            None => Err(http_error.unwrap_or_else(|| "[config] 未配置 HTTP 来源".to_string())),
        }
    };
    let response = result.map_err(mlua::Error::RuntimeError)?;
    let table = lua.create_table()?;
    table.set("status", response.status as i64)?;
    table.set("body", lua.create_string(&response.body)?)?;
    Ok(table)
}

fn parse_headers(table: Option<Table>) -> mlua::Result<Vec<(String, String)>> {
    let Some(table) = table else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for pair in table.pairs::<String, String>() {
        let (name, value) = pair?;
        out.push((name, value));
    }
    Ok(out)
}

fn parse_segments(value: Value) -> mlua::Result<Vec<String>> {
    match value {
        Value::String(text) => {
            let text = text
                .to_str()
                .map_err(|_| mlua::Error::RuntimeError("HTTP 路径必须是 UTF-8 文本".to_string()))?;
            let text = text.trim_matches('/');
            if text.is_empty() {
                return Ok(Vec::new());
            }
            Ok(text.split('/').map(str::to_string).collect())
        }
        Value::Table(table) => {
            let mut out = Vec::new();
            for item in table.sequence_values::<mlua::String>() {
                let item = item?;
                let text = item.to_str().map_err(|_| {
                    mlua::Error::RuntimeError("HTTP 路径段必须是 UTF-8 文本".to_string())
                })?;
                out.push(text.to_string());
            }
            Ok(out)
        }
        _ => Err(mlua::Error::RuntimeError(
            "HTTP 路径必须是字符串或字符串数组".to_string(),
        )),
    }
}

fn parse_query(query: Option<Table>) -> mlua::Result<Vec<(String, String)>> {
    let Some(query) = query else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for pair in query.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let key = match key {
            Value::String(text) => text
                .to_str()
                .map_err(|_| {
                    mlua::Error::RuntimeError("HTTP query 键必须是 UTF-8 文本".to_string())
                })?
                .to_string(),
            _ => {
                return Err(mlua::Error::RuntimeError(
                    "HTTP query 键必须是字符串".to_string(),
                ))
            }
        };
        let value = match value {
            Value::String(text) => text
                .to_str()
                .map_err(|_| {
                    mlua::Error::RuntimeError("HTTP query 值必须是 UTF-8 文本".to_string())
                })?
                .to_string(),
            Value::Integer(number) => number.to_string(),
            Value::Number(number) if number.is_finite() => number.to_string(),
            _ => {
                return Err(mlua::Error::RuntimeError(
                    "HTTP query 值必须是字符串或数字".to_string(),
                ))
            }
        };
        out.push((key, value));
    }
    Ok(out)
}

/// `ui.confirm` / `ui.message`: small declarative dialogs for manual actions.
/// They use the same resumable choice protocol as `mods.choose`, so they keep
/// working across the CLI's exit/relaunch handshake.
fn install_ui(lua: &Lua, io: &Rc<RefCell<HostIo>>) -> Result<(), String> {
    let ui = lua
        .create_table()
        .map_err(|error| format!("无法创建 ui 表：{error}"))?;

    let confirm_io = io.clone();
    let confirm = lua
        .create_function(move |_lua, table: Table| {
            let title = table
                .get::<Option<String>>("title")?
                .unwrap_or_else(|| "确认".to_string());
            let mut body = table
                .get::<Option<String>>("body")?
                .unwrap_or_else(|| "请确认是否继续".to_string());
            if body.is_empty() {
                body = " ".to_string();
            }
            let yes = table
                .get::<Option<String>>("yes")?
                .unwrap_or_else(|| "确定".to_string());
            let no = table
                .get::<Option<String>>("no")?
                .unwrap_or_else(|| "取消".to_string());
            let key = next_ui_key(&confirm_io, "ui_confirm");
            if let Some(option) = choice_answer(&confirm_io, &key) {
                return Ok(Value::Boolean(option == "yes"));
            }
            let request = ChoiceRequest {
                key: key.clone(),
                kind: ChoiceKind::Confirm,
                title,
                body,
                options: vec![
                    ChoiceOption {
                        id: "yes".to_string(),
                        label: yes,
                    },
                    ChoiceOption {
                        id: "no".to_string(),
                        label: no,
                    },
                ],
                default: Some("no".to_string()),
            };
            queue_choice(&confirm_io, request)
                .map_err(mlua::Error::RuntimeError)?;
            Err(mlua::Error::RuntimeError(CHOICE_SENTINEL.to_string()))
        })
        .map_err(|error| format!("无法创建 ui.confirm：{error}"))?;
    ui.set("confirm", confirm)
        .map_err(|error| format!("无法设置 ui.confirm：{error}"))?;

    let message_io = io.clone();
    let message = lua
        .create_function(move |_lua, table: Table| {
            let title = table
                .get::<Option<String>>("title")?
                .unwrap_or_else(|| "提示".to_string());
            let mut body = table.get::<Option<String>>("body")?.unwrap_or_default();
            if body.is_empty() {
                body = " ".to_string();
            }
            let button = table
                .get::<Option<String>>("button")?
                .unwrap_or_else(|| "知道了".to_string());
            let key = next_ui_key(&message_io, "ui_message");
            if let Some(_option) = choice_answer(&message_io, &key) {
                return Ok(Value::Boolean(true));
            }
            let request = ChoiceRequest {
                key: key.clone(),
                kind: ChoiceKind::Message,
                title,
                body,
                options: vec![ChoiceOption {
                    id: "ok".to_string(),
                    label: button,
                }],
                default: Some("ok".to_string()),
            };
            queue_choice(&message_io, request)
                .map_err(mlua::Error::RuntimeError)?;
            Err(mlua::Error::RuntimeError(CHOICE_SENTINEL.to_string()))
        })
        .map_err(|error| format!("无法创建 ui.message：{error}"))?;
    ui.set("message", message)
        .map_err(|error| format!("无法设置 ui.message：{error}"))?;

    lua.globals()
        .set("ui", ui)
        .map_err(|error| format!("无法设置 ui：{error}"))?;
    Ok(())
}

fn next_ui_key(io: &Rc<RefCell<HostIo>>, prefix: &str) -> String {
    let mut host = io.borrow_mut();
    host.ui_seq += 1;
    format!("{prefix}_{}", host.ui_seq)
}

fn choice_answer(io: &Rc<RefCell<HostIo>>, key: &str) -> Option<String> {
    io.borrow()
        .choices
        .iter()
        .find(|(existing, _)| existing == key)
        .map(|(_, answer)| answer.clone())
}

fn queue_choice(io: &Rc<RefCell<HostIo>>, choice: ChoiceRequest) -> Result<(), String> {
    choice.validate()?;
    let mut host = io.borrow_mut();
    if host.pending_choice.is_some() {
        return Err("同一动作不能同时请求多个界面弹窗".to_string());
    }
    host.pending_choice = Some(choice);
    Ok(())
}

fn install_report(lua: &Lua, io: &Rc<RefCell<HostIo>>, manual: bool) -> Result<(), String> {
    let mods = lua
        .create_table()
        .map_err(|error| format!("无法创建 mods 表：{error}"))?;
    let report_io = io.clone();
    let report = lua
        .create_function(move |_lua, table: Table| {
            if !manual {
                return Err(mlua::Error::RuntimeError(
                    "mods.report 仅用于 manual 动作".to_string(),
                ));
            }
            let status_text = table
                .get::<Option<String>>("status")?
                .unwrap_or_else(|| "ok".to_string());
            let status = match status_text.as_str() {
                "ok" => ManualStatus::Ok,
                "noop" => ManualStatus::Noop,
                "error" => ManualStatus::Error,
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "mods.report status 只能是 ok/noop/error，收到 {other:?}"
                    )))
                }
            };
            let direction = match table.get::<Option<String>>("direction")? {
                Some(text) => match text.as_str() {
                    "upload" => Some(crate::output::Direction::Upload),
                    "download" => Some(crate::output::Direction::Download),
                    other => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "mods.report direction 只能是 upload/download，收到 {other:?}"
                        )))
                    }
                },
                None => None,
            };
            let error = match table.get::<Option<String>>("error")? {
                Some(text) if text != "none" => match error_kind_from_str(&text) {
                    Some(kind) => Some(kind),
                    None => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "mods.report error 分类无效：{text:?}"
                        )))
                    }
                },
                _ => None,
            };
            let detail = table
                .get::<Option<String>>("detail")?
                .map(sanitize_detail)
                .filter(|text| !text.is_empty());
            let error = if status == ManualStatus::Error {
                Some(error.unwrap_or(ErrorKind::Server))
            } else {
                None
            };
            report_io.borrow_mut().manual_report = Some(ManualReportValue {
                status,
                direction,
                error,
                detail,
            });
            Ok(())
        })
        .map_err(|error| format!("无法创建 mods.report：{error}"))?;
    mods.set("report", report)
        .map_err(|error| format!("无法设置 mods.report：{error}"))?;
    install_progress(lua, &mods, io, manual)?;
    install_choose(lua, &mods, io, manual)?;
    lua.globals()
        .set("mods", mods)
        .map_err(|error| format!("无法设置 mods：{error}"))?;
    Ok(())
}

fn install_progress(
    lua: &Lua,
    mods: &Table,
    io: &Rc<RefCell<HostIo>>,
    manual: bool,
) -> Result<(), String> {
    let progress_io = io.clone();
    let progress = lua
        .create_function(move |_lua, table: Table| {
            if !manual {
                return Err(mlua::Error::RuntimeError(
                    "mods.progress 仅用于 manual 动作".to_string(),
                ));
            }
            let value = if let Some(value) = table.get::<Option<f64>>("value")? {
                value as f32
            } else {
                let current = table.get::<Option<f64>>("current")?.unwrap_or(0.0) as f32;
                let total = table.get::<Option<f64>>("total")?.unwrap_or(0.0) as f32;
                if total <= 0.0 {
                    return Err(mlua::Error::RuntimeError(
                        "mods.progress 需要 value，或 current/total".to_string(),
                    ));
                }
                current / total
            };
            if !value.is_finite() {
                return Err(mlua::Error::RuntimeError(
                    "mods.progress 的进度不是有效数字".to_string(),
                ));
            }
            let value = value.clamp(0.0, 1.0);
            let label = table
                .get::<Option<String>>("label")?
                .map(|text| {
                    text.chars()
                        .filter(|ch| !ch.is_control())
                        .take(40)
                        .collect::<String>()
                })
                .filter(|text| !text.trim().is_empty());
            let now = Instant::now();
            {
                let mut host = progress_io.borrow_mut();
                if host
                    .progress_at
                    .is_some_and(|at| now.duration_since(at).as_millis() < 100)
                {
                    return Ok(());
                }
                host.progress_at = Some(now);
                let value = host
                    .progress_last
                    .map_or(value, |previous| previous.max(value));
                host.progress_last = Some(value);
            }
            // Progress is a machine-readable stdout line consumed by the
            // device's line reader, just like LINKSLOT_STAGE.  Reporter's
            // output is borrowed inside the CLI, so write the same pipe
            // directly here and flush immediately.
            use std::io::Write as _;
            let mut out = std::io::stdout();
            let percent = (value * 100.0).round() as u32;
            let _ = writeln!(out, "LINKSLOT_PROGRESS={percent}");
            if let Some(label) = label {
                let _ = writeln!(out, "LINKSLOT_PROGRESS_LABEL={label}");
            }
            let _ = out.flush();
            Ok(())
        })
        .map_err(|error| format!("无法创建 mods.progress：{error}"))?;
    mods.set("progress", progress)
        .map_err(|error| format!("无法设置 mods.progress：{error}"))?;
    Ok(())
}

fn install_choose(
    lua: &Lua,
    mods: &Table,
    io: &Rc<RefCell<HostIo>>,
    manual: bool,
) -> Result<(), String> {
    let choose_io = io.clone();
    let choose = lua
        .create_function(move |lua, table: Table| {
            if !manual {
                return Err(mlua::Error::RuntimeError(
                    "mods.choose 仅用于 manual 动作".to_string(),
                ));
            }
            let request = parse_choice_request(&table)?;
            let existing = choose_io
                .borrow()
                .choices
                .iter()
                .find(|(key, _)| key == &request.key)
                .map(|(_, value)| value.clone());
            if let Some(value) = existing {
                if request.option_label(&value).is_none() {
                    return Err(mlua::Error::RuntimeError(format!(
                        "已保存的选择 {:?} 不在 {} 的选项中",
                        value, request.key
                    )));
                }
                return Ok(Value::String(lua.create_string(&value)?));
            }
            {
                let mut host = choose_io.borrow_mut();
                if host.pending_choice.is_some() {
                    return Err(mlua::Error::RuntimeError(
                        "同一动作不能同时请求多个选择".to_string(),
                    ));
                }
                host.pending_choice = Some(request);
            }
            Err(mlua::Error::RuntimeError(CHOICE_SENTINEL.to_string()))
        })
        .map_err(|error| format!("无法创建 mods.choose：{error}"))?;
    mods.set("choose", choose)
        .map_err(|error| format!("无法设置 mods.choose：{error}"))?;
    Ok(())
}

fn parse_choice_request(table: &Table) -> mlua::Result<ChoiceRequest> {
    let key = required_string(table, "key")?;
    let title = required_string(table, "title")?;
    let body = required_string(table, "body")?;
    let options_table = table
        .get::<Table>("options")
        .map_err(|_| mlua::Error::RuntimeError("mods.choose 缺少 options 数组".to_string()))?;
    let mut options = Vec::new();
    for value in options_table.sequence_values::<Table>() {
        let option = value?;
        options.push(slot_mods::ChoiceOption {
            id: required_string(&option, "id")?,
            label: required_string(&option, "label")?,
        });
    }
    let default = table.get::<Option<String>>("default")?;
    let request = ChoiceRequest {
        key,
        kind: ChoiceKind::Choice,
        title,
        body,
        options,
        default,
    };
    request
        .validate()
        .map_err(mlua::Error::RuntimeError)?;
    Ok(request)
}

fn required_string(table: &Table, field: &str) -> mlua::Result<String> {
    table
        .get::<Option<String>>(field)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            mlua::Error::RuntimeError(format!("mods.choose 的 {field} 必须是非空字符串"))
        })
}

fn error_kind_from_str(text: &str) -> Option<ErrorKind> {
    Some(match text {
        "net" => ErrorKind::Net,
        "auth" => ErrorKind::Auth,
        "cert" => ErrorKind::Cert,
        "clock" => ErrorKind::Clock,
        "server" => ErrorKind::Server,
        "config" => ErrorKind::Config,
        "fs" => ErrorKind::Fs,
        "verify" => ErrorKind::Verify,
        "timeout" => ErrorKind::Timeout,
        "busy" => ErrorKind::Busy,
        "no-cache" => ErrorKind::NoCache,
        "remote-newer" => ErrorKind::RemoteNewer,
        "forked" => ErrorKind::Forked,
        "remote-rollback" => ErrorKind::RemoteRollback,
        "changed-since" => ErrorKind::ChangedSince,
        _ => return None,
    })
}

fn sanitize_detail(text: String) -> String {
    text.chars()
        .take(500)
        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
        .collect::<String>()
        .trim()
        .to_string()
}

fn install_json(lua: &Lua, json: &Table) -> Result<(), String> {
    let encode = lua
        .create_function(|lua, value: Value| {
            let value: serde_json::Value = lua.from_value(value).map_err(|error| {
                mlua::Error::RuntimeError(format!("json.encode 输入无效：{error}"))
            })?;
            serde_json::to_string(&value)
                .map_err(|error| mlua::Error::RuntimeError(format!("json.encode 失败：{error}")))
        })
        .map_err(|error| format!("无法创建 json.encode：{error}"))?;
    json.set("encode", encode)
        .map_err(|error| format!("无法设置 json.encode：{error}"))?;

    let decode = lua
        .create_function(|lua, text: String| {
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|error| mlua::Error::RuntimeError(format!("json.decode 失败：{error}")))?;
            lua.to_value(&value).map_err(|error| {
                mlua::Error::RuntimeError(format!("json.decode 转换失败：{error}"))
            })
        })
        .map_err(|error| format!("无法创建 json.decode：{error}"))?;
    json.set("decode", decode)
        .map_err(|error| format!("无法设置 json.decode：{error}"))?;
    Ok(())
}

fn install_hash(lua: &Lua, hash: &Table, io: &Rc<RefCell<HostIo>>) -> Result<(), String> {
    let sha256 = lua
        .create_function(|_lua, data: mlua::String| Ok(crate::hash::sha256_hex(&data.as_bytes())))
        .map_err(|error| format!("无法创建 hash.sha256：{error}"))?;
    hash.set("sha256", sha256)
        .map_err(|error| format!("无法设置 hash.sha256：{error}"))?;

    let file_io = io.clone();
    let sha256_file = lua
        .create_function(move |_lua, rel: String| {
            let bytes = { file_io.borrow().local.read(&rel) }.map_err(mlua::Error::RuntimeError)?;
            Ok(bytes.map(|bytes| crate::hash::sha256_hex(&bytes)))
        })
        .map_err(|error| format!("无法创建 hash.sha256_file：{error}"))?;
    hash.set("sha256_file", sha256_file)
        .map_err(|error| format!("无法设置 hash.sha256_file：{error}"))?;
    Ok(())
}

fn install_zip(lua: &Lua, zip: &Table) -> Result<(), String> {
    let list = lua
        .create_function(|lua, data: mlua::String| {
            let archive =
                super::zip::Archive::parse(&data.as_bytes()).map_err(mlua::Error::RuntimeError)?;
            let table = lua.create_table()?;
            for (index, entry) in archive.entries().iter().enumerate() {
                let item = lua.create_table()?;
                item.set("index", index + 1)?;
                item.set("name", entry.name.clone())?;
                item.set("size", entry.size as i64)?;
                item.set("is_dir", entry.is_dir)?;
                table.set(index + 1, item)?;
            }
            Ok(table)
        })
        .map_err(|error| format!("无法创建 zip.list：{error}"))?;
    zip.set("list", list)
        .map_err(|error| format!("无法设置 zip.list：{error}"))?;

    let read = lua
        .create_function(|lua, (data, key): (mlua::String, Value)| {
            let archive =
                super::zip::Archive::parse(&data.as_bytes()).map_err(mlua::Error::RuntimeError)?;
            let bytes = match key {
                Value::Integer(index) if index > 0 => archive.read_index(index as usize),
                Value::String(name) => {
                    let name = name.to_string_lossy();
                    archive.read_name(&name)
                }
                _ => {
                    return Err(mlua::Error::RuntimeError(
                        "zip.read 的第二个参数必须是条目下标或名称".to_string(),
                    ))
                }
            };
            match bytes {
                Some(bytes) => Ok(Some(lua.create_string(bytes)?)),
                None => Ok(None),
            }
        })
        .map_err(|error| format!("无法创建 zip.read：{error}"))?;
    zip.set("read", read)
        .map_err(|error| format!("无法设置 zip.read：{error}"))?;

    let build = lua
        .create_function(|lua, entries: Table| {
            let mut list = Vec::new();
            for item in entries.sequence_values::<Table>() {
                let item = item?;
                let name: String = item.get("name")?;
                let data: mlua::String = item.get("data")?;
                list.push((name, data.as_bytes().to_vec()));
            }
            let bytes = super::zip::build(&list).map_err(mlua::Error::RuntimeError)?;
            lua.create_string(&bytes)
        })
        .map_err(|error| format!("无法创建 zip.build：{error}"))?;
    zip.set("build", build)
        .map_err(|error| format!("无法设置 zip.build：{error}"))?;
    Ok(())
}

fn install_context(
    lua: &Lua,
    ctx: &HookContext<'_>,
    io: &Rc<RefCell<HostIo>>,
    mod_id: &str,
    action_id: &str,
    action_name: &str,
) -> Result<(), String> {
    let root = ctx.root;

    let table = lua
        .create_table()
        .map_err(|error| format!("无法创建 ctx：{error}"))?;
    table.set("api", 1).map_err(ctx_error("api"))?;
    table
        .set("timing", ctx.timing.as_str())
        .map_err(ctx_error("timing"))?;
    table
        .set("device_id", ctx.device_id)
        .map_err(ctx_error("device_id"))?;
    table
        .set("app_version", env!("CARGO_PKG_VERSION"))
        .map_err(ctx_error("app_version"))?;

    if ctx.timing == Timing::AfterBoot {
        table.set("unit", "boot").map_err(ctx_error("unit"))?;
    }
    if let Some(unit) = ctx.unit {
        let state_dir = unit.local_dir(root);
        let sav_path = unit.local_sav_path(root);
        let srm_path = unit.local_srm_path(root);
        let unit_path = match ctx.kind {
            Some(crate::unit::Kind::State) => state_dir.clone(),
            Some(crate::unit::Kind::Sav) => {
                if sav_path.is_file() {
                    sav_path.clone()
                } else if srm_path.is_file() {
                    srm_path.clone()
                } else {
                    sav_path.clone()
                }
            }
            None => sav_path.clone(),
        };
        table
            .set(
                "unit",
                match ctx.kind {
                    Some(crate::unit::Kind::State) => "state",
                    Some(crate::unit::Kind::Sav) => "sav",
                    None => "sync",
                },
            )
            .map_err(ctx_error("unit"))?;
        table
            .set("unit_path", rel_path(root, &unit_path))
            .map_err(ctx_error("unit_path"))?;

        let game = lua
            .create_table()
            .map_err(|error| format!("无法创建 ctx.game：{error}"))?;
        game.set("name", unit.game.clone())
            .and_then(|_| game.set("platform", unit.platform.clone()))
            .and_then(|_| game.set("core", unit.emulator.clone()))
            .and_then(|_| {
                game.set(
                    "rom_sha256",
                    match &unit.rom_sha256 {
                        Some(digest) => Value::String(
                            lua.create_string(digest)
                                .map_err(|error| mlua::Error::RuntimeError(error.to_string()))?,
                        ),
                        None => Value::Nil,
                    },
                )
            })
            .and_then(|_| game.set("state_dir", rel_path(root, &state_dir)))
            .and_then(|_| game.set("sav_path", rel_path(root, &sav_path)))
            .and_then(|_| game.set("srm_path", rel_path(root, &srm_path)))
            .map_err(|error| format!("无法填充 ctx.game：{error}"))?;
        table.set("game", game).map_err(ctx_error("game"))?;
    }

    let module = lua
        .create_table()
        .map_err(|error| format!("无法创建 ctx.mod：{error}"))?;
    module
        .set("id", mod_id)
        .and_then(|_| module.set("dir", format!("linkSlot/mods/{mod_id}")))
        .map_err(|error| format!("无法填充 ctx.mod：{error}"))?;
    let (mod_config, config_error) = {
        let io = io.borrow();
        (io.mod_config.clone(), io.config_error.clone())
    };
    if let Some(value) = mod_config {
        let value = lua
            .to_value(&value)
            .map_err(|error| format!("无法转换 ctx.mod.config：{error}"))?;
        module
            .set("config", value)
            .map_err(|error| format!("无法填充 ctx.mod.config：{error}"))?;
    }
    if let Some(error) = config_error {
        module
            .set("config_error", error)
            .map_err(|error| format!("无法填充 ctx.mod.config_error：{error}"))?;
    }
    let choices_table = lua
        .create_table()
        .map_err(|error| format!("无法创建 ctx.choices：{error}"))?;
    for (key, value) in ctx.choices {
        choices_table
            .set(key.as_str(), value.as_str())
            .map_err(|error| format!("无法填充 ctx.choices：{error}"))?;
    }
    table
        .set("choices", choices_table)
        .map_err(ctx_error("choices"))?;
    table.set("mod", module).map_err(ctx_error("mod"))?;

    let action = lua
        .create_table()
        .map_err(|error| format!("无法创建 ctx.action：{error}"))?;
    action
        .set("id", action_id)
        .and_then(|_| action.set("name", action_name))
        .and_then(|_| action.set("dir", format!("linkSlot/mods/{mod_id}/actions/{action_id}")))
        .map_err(|error| format!("无法填充 ctx.action：{error}"))?;
    table.set("action", action).map_err(ctx_error("action"))?;

    let profiles = profile_tuples(io);
    let webdav = lua
        .create_table()
        .map_err(|error| format!("无法创建 ctx.webdav：{error}"))?;
    let active = profiles
        .iter()
        .find(|(id, _, _)| id == ctx.active_storage)
        .map(|(id, _, _)| id.clone());
    webdav
        .set(
            "active",
            match active {
                Some(active) => {
                    let text = lua
                        .create_string(active)
                        .map_err(|error| format!("无法创建 ctx.webdav.active：{error}"))?;
                    Value::String(text)
                }
                None => Value::Nil,
            },
        )
        .map_err(ctx_error("webdav.active"))?;
    let profile_table = lua
        .create_table()
        .map_err(|error| format!("无法创建 ctx.webdav.profiles：{error}"))?;
    for (index, (id, name, remote_root)) in profiles.iter().enumerate() {
        let item = lua
            .create_table()
            .map_err(|error| format!("无法创建 ctx profile：{error}"))?;
        item.set("id", id.clone())
            .and_then(|_| item.set("name", name.clone()))
            .and_then(|_| item.set("remote_root", remote_root.clone()))
            .map_err(|error| format!("无法填充 ctx profile：{error}"))?;
        profile_table
            .set(index + 1, item)
            .map_err(|error| format!("无法填充 ctx profiles：{error}"))?;
    }
    webdav
        .set("profiles", profile_table)
        .map_err(ctx_error("webdav.profiles"))?;
    table.set("webdav", webdav).map_err(ctx_error("webdav"))?;

    if let Some(transfer) = ctx.transfer {
        let info = lua
            .create_table()
            .map_err(|error| format!("无法创建 ctx.transfer：{error}"))?;
        info.set("direction", transfer.direction.as_str())
            .and_then(|_| info.set("result", transfer.result.as_str()))
            .and_then(|_| {
                info.set(
                    "version",
                    match transfer.version {
                        Some(version) => Value::Integer(version as i64),
                        None => Value::Nil,
                    },
                )
            })
            .map_err(|error| format!("无法填充 ctx.transfer：{error}"))?;
        table.set("transfer", info).map_err(ctx_error("transfer"))?;
    }

    lua.globals()
        .set("ctx", table)
        .map_err(|error| format!("无法设置 ctx：{error}"))?;
    Ok(())
}

fn ctx_error(key: &'static str) -> impl Fn(mlua::Error) -> String {
    move |error| format!("无法设置 ctx.{key}：{error}")
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn profile_tuples(io: &Rc<RefCell<HostIo>>) -> Vec<(String, String, String)> {
    io.borrow()
        .remote
        .profiles()
        .iter()
        .map(|profile| {
            (
                profile.id.clone(),
                profile.name.clone(),
                profile.remote_root.clone(),
            )
        })
        .collect()
}

fn entries_table(lua: &Lua, entries: Vec<Entry>) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    for (index, entry) in entries.into_iter().enumerate() {
        table.set(index + 1, entry_table(lua, entry)?)?;
    }
    Ok(table)
}

fn entry_table(lua: &Lua, entry: Entry) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("name", entry.name)?;
    table.set("is_dir", entry.is_dir)?;
    table.set("size", entry.size as i64)?;
    table.set("mtime", entry.mtime as i64)?;
    Ok(table)
}

fn dav_entries_table(lua: &Lua, entries: Vec<crate::webdav::DavEntry>) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    for (index, entry) in entries.into_iter().enumerate() {
        table.set(index + 1, dav_entry_table(lua, entry)?)?;
    }
    Ok(table)
}

fn dav_entry_table(lua: &Lua, entry: crate::webdav::DavEntry) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("name", entry.name)?;
    table.set("is_dir", entry.is_dir)?;
    table.set("size", entry.size as i64)?;
    table.set("modified", entry.modified.unwrap_or_default())?;
    Ok(table)
}

fn parse_path_call(args: MultiValue) -> mlua::Result<(Option<String>, String)> {
    let values: Vec<Value> = args.into_iter().collect();
    match values.as_slice() {
        [path] => Ok((None, string_value(path, "path")?)),
        [profile, path] => Ok((
            Some(string_value(profile, "profile")?),
            string_value(path, "path")?,
        )),
        _ => Err(mlua::Error::RuntimeError(
            "webdav 需要 (path) 或 (profile, path) 参数".to_string(),
        )),
    }
}

fn parse_put_call(args: MultiValue) -> mlua::Result<(Option<String>, String, Vec<u8>)> {
    let values: Vec<Value> = args.into_iter().collect();
    match values.as_slice() {
        [path, data] => Ok((None, string_value(path, "path")?, bytes_value(data)?)),
        [profile, path, data] => Ok((
            Some(string_value(profile, "profile")?),
            string_value(path, "path")?,
            bytes_value(data)?,
        )),
        _ => Err(mlua::Error::RuntimeError(
            "webdav 写入需要 (path, data) 或 (profile, path, data) 参数".to_string(),
        )),
    }
}

fn string_value(value: &Value, label: &str) -> mlua::Result<String> {
    match value {
        Value::String(text) => text
            .to_str()
            .map(|text| text.to_string())
            .map_err(|_| mlua::Error::RuntimeError(format!("{label} 必须是 UTF-8 字符串"))),
        _ => Err(mlua::Error::RuntimeError(format!("{label} 必须是字符串"))),
    }
}

fn bytes_value(value: &Value) -> mlua::Result<Vec<u8>> {
    match value {
        Value::String(text) => Ok(text.as_bytes().to_vec()),
        _ => Err(mlua::Error::RuntimeError("data 必须是字符串".to_string())),
    }
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::Nil => "nil".to_string(),
        Value::Boolean(value) => value.to_string(),
        Value::Integer(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value
            .to_str()
            .map(|value| value.to_string())
            .unwrap_or_else(|_| String::from_utf8_lossy(&value.as_bytes()).into_owned()),
        other => format!("{other:?}"),
    }
}

fn sanitize_log(message: String) -> String {
    let message = message.replace(['\n', '\r'], " ");
    let chars: Vec<char> = message.chars().collect();
    if chars.len() <= 500 {
        message
    } else {
        let mut out: String = chars.into_iter().take(500).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_targets_only_the_current_games_intro_and_guide() {
        let root = Path::new("/card");
        let intro = library_target(root, "GBA", "Test Game", "intro").unwrap();
        let guide = library_target(root, "GBA", "Test Game", "guide").unwrap();
        assert!(intro.to_string_lossy().replace('\\', "/").ends_with("Info/GBA/Test Game.txt"));
        assert!(guide.to_string_lossy().replace('\\', "/").ends_with("Guides/GBA/Test Game.txt"));
        assert!(library_target(root, "GBA", "Test Game", "cover").is_err());
        assert!(library_target(root, "GBA", "../escape", "intro").is_err());
    }
}
