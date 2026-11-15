-- SaveDeck battery-save bridge.
--
-- All SaveDeck-specific knowledge lives in this mod:
--   * the SaveDeck zip layout (_savedeck/game.json + .srm/.sav)
--   * matching SaveDeck archives to local battery saves
--   * conflict policy, backups and the sync state machine.
-- linkSlot itself only exposes generic files/webdav/zip/json/hash/log APIs.
--
-- This mod's action runs at `manual` (long-press Y), for the current game.
-- A pulled save is uploaded to the linkSlot cloud by the next normal sync.

local CONFIG = {
  -- A WebDAV profile from the user's sync-method list. The mod can only
  -- access that profile's remote_root; choose a dedicated SaveDeck profile.
  profile = "",

  -- "pull" = SaveDeck -> linkSlot, "push" = linkSlot -> SaveDeck,
  -- "two-way" = both directions, "diagnose" = only log matching keys.
  mode = "two-way",

  -- When true, also match remote archives by game.name when game.romStem does
  -- not match. Explicit aliases below always win.
  match_by_name = true,

  -- "newest" uses the WebDAV modified time vs. the local file mtime.
  -- "local" / "remote" force a direction when both sides changed.
  -- "ask" prompts on the device in manual mode.
  on_conflict = "ask",

  -- Safe default: never create a new SaveDeck archive from scratch. Create the
  -- first backup in SaveDeck, then this mod can keep it in sync.
  allow_create = false,

  -- At most this many archive downloads/index reads and this many pull/push
  -- actions per sync. Remaining work continues on the next sync.
  batch = 8,

  platforms = { "GBA", "GB", "GBC" },

  -- Map a SaveDeck platform name to a local Saves/<platform> directory.
  -- Example: GBA ROM hacks are stored under Saves/GBA but SaveDeck may label
  -- them as "GBA hack"; map that label to GBA.
  platform_map = { ["GBA hack"] = "GBA" },

  -- Optional aliases from the local save stem to the SaveDeck romStem/name:
  -- ["GBA/Local Name"] = "SaveDeck Name".
  aliases = {},
}

local STATE_PATH = ctx.mod.dir .. "/state.json"
local CONFIG_PATH = ctx.mod.dir .. "/config.json"
local BACKUP_DIR = ctx.mod.dir .. "/backup"
local MONTHS = {
  Jan = 1,
  Feb = 2,
  Mar = 3,
  Apr = 4,
  May = 5,
  Jun = 6,
  Jul = 7,
  Aug = 8,
  Sep = 9,
  Oct = 10,
  Nov = 11,
  Dec = 12,
}

local function trim(text)
  return (text:gsub("^%s+", ""):gsub("%s+$", ""))
end

local function safe_segment(text)
  if type(text) ~= "string" or text == "" or text == "." or text == ".." then
    return false
  end
  if text:find("/", 1, true) or text:find("\\", 1, true) then
    return false
  end
  return not text:find(string.char(0), 1, true)
end

local function allowed_platform(platform)
  if CONFIG.platform_map[platform] then
    return true
  end
  for _, item in ipairs(CONFIG.platforms) do
    if item == platform then
      return true
    end
  end
  return false
end

local function local_platform(platform)
  local mapped = CONFIG.platform_map[platform]
  if type(mapped) == "string" and mapped ~= "" then
    return mapped
  end
  return platform
end

local function local_platform_dirs()
  local seen = {}
  local out = {}
  local function add(remote)
    local local_name = local_platform(remote)
    if safe_segment(local_name) and not seen[local_name] then
      seen[local_name] = true
      out[#out + 1] = local_name
    end
  end
  for _, platform in ipairs(CONFIG.platforms) do
    add(platform)
  end
  for remote in pairs(CONFIG.platform_map) do
    add(remote)
  end
  return out
end

local function safe_file_name(text)
  local value = trim(tostring(text or "game")):gsub("[^%w%-%._]", "_")
  if value == "" then
    return "game"
  end
  return value
end

local function remote_stem(game)
  local value = ""
  if type(game.romStem) == "string" then
    value = game.romStem
  elseif type(game.name) == "string" then
    value = game.name
  end
  value = trim(value)
  value = value:gsub("%.[zZ][iI][pP]$", "")
  return value
end

local function remote_name(game)
  if type(game.name) ~= "string" then
    return ""
  end
  local value = trim(game.name)
  return value:gsub("%.[zZ][iI][pP]$", "")
end

local function valid_mode(value)
  return value == "pull" or value == "push" or value == "two-way" or value == "diagnose"
end

local function valid_conflict(value)
  return value == "newest" or value == "local" or value == "remote" or value == "ask"
end

local function validate_config(value)
  if type(value) ~= "table" then
    return nil, "必须是 JSON 对象"
  end
  local next_config = {
    profile = CONFIG.profile,
    mode = CONFIG.mode,
    match_by_name = CONFIG.match_by_name,
    on_conflict = CONFIG.on_conflict,
    allow_create = CONFIG.allow_create,
    batch = CONFIG.batch,
    platforms = CONFIG.platforms,
    platform_map = CONFIG.platform_map,
    aliases = CONFIG.aliases,
  }
  if value.profile ~= nil then
    if type(value.profile) ~= "string" or trim(value.profile) == "" then
      return nil, "profile 必须是非空字符串"
    end
    next_config.profile = trim(value.profile)
  end
  if value.mode ~= nil then
    if type(value.mode) ~= "string" or not valid_mode(value.mode) then
      return nil, "mode 只能是 pull / push / two-way / diagnose"
    end
    next_config.mode = value.mode
  end
  if value.match_by_name ~= nil then
    if type(value.match_by_name) ~= "boolean" then
      return nil, "match_by_name 必须是布尔值"
    end
    next_config.match_by_name = value.match_by_name
  end
  if value.on_conflict ~= nil then
    if type(value.on_conflict) ~= "string" or not valid_conflict(value.on_conflict) then
      return nil, "on_conflict 只能是 newest / local / remote / ask"
    end
    next_config.on_conflict = value.on_conflict
  end
  if value.allow_create ~= nil then
    if type(value.allow_create) ~= "boolean" then
      return nil, "allow_create 必须是布尔值"
    end
    next_config.allow_create = value.allow_create
  end
  if value.batch ~= nil then
    if type(value.batch) ~= "number" or value.batch ~= math.floor(value.batch) then
      return nil, "batch 必须是整数"
    end
    if value.batch < 1 or value.batch > 64 then
      return nil, "batch 必须在 1..64 之间"
    end
    next_config.batch = value.batch
  end
  if value.platforms ~= nil then
    if type(value.platforms) ~= "table" or #value.platforms == 0 then
      return nil, "platforms 必须是非空数组"
    end
    local platforms = {}
    for _, platform in ipairs(value.platforms) do
      if type(platform) ~= "string" or not safe_segment(platform) then
        return nil, "platforms 包含非法平台名"
      end
      platforms[#platforms + 1] = platform
    end
    next_config.platforms = platforms
  end
  if value.platform_map ~= nil then
    if type(value.platform_map) ~= "table" then
      return nil, "platform_map 必须是对象"
    end
    local platform_map = {}
    local count = 0
    for remote, local_name in pairs(value.platform_map) do
      count = count + 1
      if count > 64 then
        return nil, "platform_map 最多 64 项"
      end
      if type(remote) ~= "string" or type(local_name) ~= "string" then
        return nil, "platform_map 的键和值必须是字符串"
      end
      remote = trim(remote)
      local_name = trim(local_name)
      if not safe_segment(remote) or not safe_segment(local_name) then
        return nil, "platform_map 包含非法平台名"
      end
      platform_map[remote] = local_name
    end
    next_config.platform_map = platform_map
  end
  if value.aliases ~= nil then
    if type(value.aliases) ~= "table" then
      return nil, "aliases 必须是对象"
    end
    local aliases = {}
    local count = 0
    for local_key, remote_key in pairs(value.aliases) do
      count = count + 1
      if count > 256 then
        return nil, "aliases 最多 256 项"
      end
      if type(local_key) ~= "string" or type(remote_key) ~= "string" then
        return nil, "aliases 的键和值必须是字符串"
      end
      local local_key = trim(local_key)
      local remote_key = trim(remote_key)
      if local_key == "" or remote_key == "" then
        return nil, "aliases 的键和值不能为空"
      end
      if not local_key:find("/", 1, true) then
        return nil, "aliases 键必须是 平台/本地存档名"
      end
      aliases[local_key] = remote_key
    end
    next_config.aliases = aliases
  end
  return next_config, nil
end

local function load_config()
  local text = files.read(CONFIG_PATH)
  if not text then
    return
  end
  local decoded, value = pcall(json.decode, text)
  if not decoded then
    log.warn("SaveDeck：config.json 解析失败，使用默认配置")
    return
  end
  local next_config, error_message = validate_config(value)
  if not next_config then
    log.warn("SaveDeck：config.json 无效（" .. tostring(error_message) .. "），使用默认配置")
    return
  end
  CONFIG = next_config
end

local function local_stem(name)
  local lower = name:lower()
  if lower:sub(-4) == ".sav" or lower:sub(-4) == ".srm" then
    return name:sub(1, -5)
  end
  return nil
end

local function load_state()
  local text = files.read(STATE_PATH)
  local state = { version = 1, games = {}, index = {} }
  if not text then
    return state
  end
  local ok, value = pcall(json.decode, text)
  if not ok or type(value) ~= "table" then
    log.warn("SaveDeck：state.json 损坏，已放弃旧 state；备份将在本次决策后写入 state.json.bad")
    return { version = 1, games = {}, index = {}, bad_state = text }
  end
  value.version = 1
  if type(value.games) ~= "table" then
    value.games = {}
  end
  if type(value.index) ~= "table" then
    value.index = {}
  end
  return value
end

local function save_state(state)
  local bad_state = state.bad_state
  state.bad_state = nil
  if bad_state then
    local backed_up, backup_error = pcall(files.write, STATE_PATH .. ".bad", bad_state)
    if not backed_up then
      log.warn("SaveDeck：保存 state.json.bad 失败：" .. tostring(backup_error))
    end
  end
  local ok, error_message = pcall(files.write, STATE_PATH, json.encode(state))
  if not ok then
    log.warn("SaveDeck：保存 state.json 失败：" .. tostring(error_message))
  end
end

local function find_profile()
  for _, profile in ipairs(ctx.webdav.profiles or {}) do
    if profile.id == CONFIG.profile then
      return profile
    end
  end
  return nil
end

local function days_from_civil(year, month, day)
  if month <= 2 then
    year = year - 1
  end
  local era = math.floor(year / 400)
  local yoe = year - era * 400
  local mp = (month + 9) % 12
  local doy = math.floor((153 * mp + 2) / 5) + day - 1
  local doe = yoe * 365 + math.floor(yoe / 4) - math.floor(yoe / 100) + doy
  return era * 146097 + doe - 719468
end

local function http_epoch(text)
  if type(text) ~= "string" then
    return 0
  end
  local day, month_name, year, hour, minute, second =
    text:match("(%d%d?)%s+(%a+)%s+(%d%d%d%d)%s+(%d%d):(%d%d):(%d%d)")
  local month = MONTHS[month_name]
  if not day or not month or not year then
    return 0
  end
  local epoch = days_from_civil(tonumber(year), month, tonumber(day)) * 86400
  return epoch + tonumber(hour) * 3600 + tonumber(minute) * 60 + tonumber(second)
end

local function list_remote()
  local ok, entries = pcall(webdav.list, CONFIG.profile, "")
  if not ok then
    return nil, tostring(entries)
  end
  local result = {}
  for _, entry in ipairs(entries or {}) do
    if not entry.is_dir and type(entry.name) == "string" and entry.name:lower():sub(-4) == ".zip" then
      result[#result + 1] = entry
    end
  end
  return result
end

local function cache_valid(cache, entry)
  return type(cache) == "table"
    and cache.size == entry.size
    and cache.modified == entry.modified
    and type(cache.game) == "table"
    and type(cache.stem) == "string"
    and type(cache.platform) == "string"
    and type(cache.save_entry) == "string"
    and type(cache.save_hash) == "string"
end

local function read_remote_archive(entry)
  local ok, zip_bytes = pcall(webdav.get, CONFIG.profile, entry.name)
  if not ok or not zip_bytes then
    return nil, "下载 archive 失败"
  end
  local game_text = zip.read(zip_bytes, "_savedeck/game.json")
  if not game_text then
    return nil, "archive 缺少 _savedeck/game.json"
  end
  local decoded, game = pcall(json.decode, game_text)
  if not decoded or type(game) ~= "table" then
    return nil, "game.json 不是有效 JSON"
  end
  local remote_platform = game.platform
  if not allowed_platform(remote_platform) then
    return nil, "不支持的平台 " .. tostring(remote_platform)
  end
  local platform = local_platform(remote_platform)
  local stem = remote_stem(game)
  if not safe_segment(stem) then
    return nil, "romStem 不是安全的存档名"
  end
  local count = tonumber(game.saveFileCount or 1)
  if count and count > 1 then
    return nil, "多存档文件暂不支持"
  end

  local entries = zip.list(zip_bytes)
  local save_entry = nil
  for _, item in ipairs(entries or {}) do
    local lower = item.name:lower()
    if not item.is_dir
      and lower:sub(1, 10) ~= "_savedeck/"
      and (lower:sub(-4) == ".srm" or lower:sub(-4) == ".sav")
    then
      save_entry = item
      break
    end
  end
  if not save_entry then
    return nil, "archive 没有 .srm/.sav 存档"
  end
  local save_data = zip.read(zip_bytes, save_entry.index)
  if not save_data or #save_data == 0 then
    return nil, "读取 archive 存档失败"
  end
  local cache = {
    size = entry.size,
    modified = entry.modified,
    game = game,
    platform = platform,
    remote_platform = remote_platform,
    stem = stem,
    save_entry = save_entry.name,
    save_hash = hash.sha256(save_data),
    archive = entry.name,
  }
  return {
    archive = entry.name,
    size = entry.size,
    modified = entry.modified,
    game = game,
    platform = platform,
    remote_platform = remote_platform,
    stem = stem,
    name = remote_name(game),
    save_entry = save_entry.name,
    save_hash = cache.save_hash,
    zip_bytes = zip_bytes,
    save_data = save_data,
  }, nil, cache
end

local function ensure_remote_data(item)
  if item.save_data then
    return item.save_data
  end
  local ok, zip_bytes = pcall(webdav.get, CONFIG.profile, item.archive)
  if not ok or not zip_bytes then
    return nil, "下载 archive 失败"
  end
  local save_data = zip.read(zip_bytes, item.save_entry)
  if not save_data then
    local entries = zip.list(zip_bytes)
    for _, entry in ipairs(entries or {}) do
      local lower = entry.name:lower()
      if not entry.is_dir and (lower:sub(-4) == ".srm" or lower:sub(-4) == ".sav") then
        save_data = zip.read(zip_bytes, entry.index)
        item.save_entry = entry.name
        break
      end
    end
  end
  if not save_data or #save_data == 0 then
    return nil, "读取 archive 存档失败"
  end
  item.zip_bytes = zip_bytes
  item.save_data = save_data
  item.save_hash = hash.sha256(save_data)
  return save_data
end

local function scan_local()
  local result = {}
  for _, platform in ipairs(local_platform_dirs()) do
    local ok, entries = pcall(files.list, "Saves/" .. platform)
    if ok then
      for _, entry in ipairs(entries or {}) do
        if not entry.is_dir then
          local stem = local_stem(entry.name)
          if stem and safe_segment(stem) then
            local key = platform .. "/" .. stem
            local lower = entry.name:lower()
            local current = result[key]
            if not current or (lower:sub(-4) == ".sav" and current.ext ~= ".sav") then
              result[key] = {
                platform = platform,
                stem = stem,
                key = key,
                name = entry.name,
                ext = lower:sub(-4),
                path = "Saves/" .. platform .. "/" .. entry.name,
                mtime = entry.mtime or 0,
              }
            end
          end
        end
      end
    end
  end
  return result
end

local function backup(name, data)
  if not data then
    return true
  end
  local ok, error_message = pcall(files.write, BACKUP_DIR .. "/" .. safe_file_name(name), data)
  if not ok then
    log.warn("SaveDeck：写备份失败：" .. tostring(error_message))
    return false
  end
  return true
end

local function local_path_for(platform, stem)
  if not safe_segment(platform) or not safe_segment(stem) then
    return nil
  end
  return "Saves/" .. platform .. "/" .. stem .. ".sav"
end

local function remember(state, key, local_hash, remote_hash, item, local_path)
  state.games[key] = {
    local_hash = local_hash,
    remote_hash = remote_hash,
    archive = item.archive,
    save_entry = item.save_entry,
    local_path = local_path,
  }
  state.index[item.archive] = {
    size = item.size,
    modified = item.modified,
    game = item.game,
    platform = item.platform,
    stem = item.stem,
    save_entry = item.save_entry,
    save_hash = remote_hash,
    archive = item.archive,
  }
  save_state(state)
end

local function pull(state, key, local_item, item, reason)
  local remote_data, error_message = ensure_remote_data(item)
  if not remote_data then
    log.warn("SaveDeck：" .. tostring(error_message))
    return false
  end
  local path = local_item and local_item.path or local_path_for(item.platform, item.stem)
  if not path then
    log.warn("SaveDeck：无法为 " .. item.stem .. " 生成本地路径")
    return false
  end
  local existing = local_item and files.read(local_item.path) or nil
  if existing then
    local backup_name = item.platform .. "-" .. item.stem .. ".local.sav"
    if not backup(backup_name, existing) then
      return false
    end
  end
  local ok, write_error = pcall(files.write, path, remote_data)
  if not ok then
    log.warn("SaveDeck：写入本地存档失败：" .. tostring(write_error))
    return false
  end
  local remote_hash = hash.sha256(remote_data)
  item.save_hash = remote_hash
  remember(state, key, remote_hash, remote_hash, item, path)
  log.info(string.format(
    "SaveDeck：拉取 %s（%s）→ %s",
    item.stem,
    reason,
    path
  ))
  return true
end

local function push(state, key, local_item, item, local_data, local_hash, reason)
  if not item then
    log.info("SaveDeck：本地有 " .. local_item.stem .. "，但 SaveDeck 没有对应 archive；跳过（不自动新建）")
    return false
  end
  local remote_data, error_message = ensure_remote_data(item)
  if not remote_data then
    log.warn("SaveDeck：" .. tostring(error_message))
    return false
  end
  local backup_base = item.platform .. "-" .. item.stem
  if not backup(backup_base .. ".local.sav", local_data) then
    return false
  end
  if not backup(backup_base .. ".remote.sav", remote_data) then
    return false
  end

  local save_name = item.save_entry
  if type(save_name) ~= "string" or save_name == "" then
    save_name = item.stem .. ".srm"
  end
  local game = item.game
  game.archive = item.archive
  game.saveFileCount = 1
  game.version = 9
  local zip_ok, zip_bytes = pcall(zip.build, {
    { name = "_savedeck/game.json", data = json.encode(game) },
    { name = save_name, data = local_data },
  })
  if not zip_ok then
    log.warn("SaveDeck：生成 archive 失败：" .. tostring(zip_bytes))
    return false
  end
  local put_ok, put_error = pcall(webdav.put_atomic, CONFIG.profile, item.archive, zip_bytes)
  if not put_ok then
    log.warn("SaveDeck：上传 archive 失败：" .. tostring(put_error))
    return false
  end
  item.save_entry = save_name
  item.save_hash = local_hash
  item.save_data = local_data
  item.zip_bytes = zip_bytes
  remember(state, key, local_hash, local_hash, item, local_item.path)
  log.info(string.format(
    "SaveDeck：推送 %s（%s）→ %s",
    item.stem,
    reason,
    item.archive
  ))
  return true
end

local function run()
  load_config()
  local profile = find_profile()
  if not profile then
    log.warn("SaveDeck：未找到 profile，请在 mod 管理页选择 WebDAV 同步方式")
    return
  end
  if
    CONFIG.mode ~= "pull"
    and CONFIG.mode ~= "push"
    and CONFIG.mode ~= "two-way"
    and CONFIG.mode ~= "diagnose"
  then
    log.warn("SaveDeck：mode 只能是 pull / push / two-way / diagnose")
    return
  end
  local diagnostic = CONFIG.mode == "diagnose"
  local can_pull = not diagnostic and CONFIG.mode ~= "push"
  local can_push = not diagnostic and CONFIG.mode ~= "pull"

  local state = load_state()
  local remote_entries, list_error = list_remote()
  if not remote_entries then
    log.warn("SaveDeck：列出远端失败：" .. tostring(list_error))
    return
  end

  local listed = {}
  local remote_by_key = {}
  local remote_by_name = {}
  local indexed = 0
  local function index_remote(map, key, item)
    local current = map[key]
    if not current or (current.modified or "") < (item.modified or "") then
      map[key] = item
    end
  end
  for _, entry in ipairs(remote_entries) do
    listed[entry.name] = true
    local cache = state.index[entry.name]
    local item = nil
    if cache_valid(cache, entry) then
      local cached_platform = cache.platform
      item = {
        archive = entry.name,
        size = entry.size,
        modified = entry.modified,
        game = cache.game,
        platform = local_platform(cached_platform),
        remote_platform = cache.remote_platform or cached_platform,
        stem = cache.stem,
        name = remote_name(cache.game),
        save_entry = cache.save_entry,
        save_hash = cache.save_hash,
      }
    elseif indexed < CONFIG.batch then
      local read_ok, loaded, error_message, cache_entry = pcall(read_remote_archive, entry)
      indexed = indexed + 1
      if read_ok and loaded then
        state.index[entry.name] = cache_entry
        item = loaded
      else
        if not read_ok then
          error_message = loaded
        end
        log.debug("SaveDeck：跳过 archive " .. entry.name .. "：" .. tostring(error_message))
      end
    end
    if item then
      local key = item.platform .. "/" .. item.stem
      index_remote(remote_by_key, key, item)
      if CONFIG.match_by_name and item.name ~= "" and item.name ~= item.stem then
        index_remote(remote_by_name, item.platform .. "/" .. item.name, item)
      end
    end
  end
  for archive in pairs(state.index) do
    if not listed[archive] then
      state.index[archive] = nil
    end
  end

  local local_by_key = scan_local()
  if diagnostic then
    for key, item in pairs(remote_by_key) do
      log.info(
        "SaveDeck：诊断 remote "
          .. key
          .. "（archive "
          .. item.archive
          .. "，name "
          .. tostring(item.name)
          .. "）"
      )
    end
    for key, item in pairs(remote_by_name) do
      log.info("SaveDeck：诊断 remote-name " .. key .. "（archive " .. item.archive .. "）")
    end
    for key, local_item in pairs(local_by_key) do
      local alias = CONFIG.aliases[key]
      local remote_stem = alias or local_item.stem
      local remote_key = local_item.platform .. "/" .. remote_stem
      local item = remote_by_key[remote_key]
      local via = alias and "alias" or "romStem"
      if not item and CONFIG.match_by_name then
        item = remote_by_name[remote_key]
        via = "name"
      end
      if item then
        log.info(
          "SaveDeck：诊断 match "
            .. key
            .. " → "
            .. remote_key
            .. "（"
            .. via
            .. "："
            .. item.archive
            .. "）"
        )
      else
        log.info("SaveDeck：诊断 unmatched " .. key .. "（尝试 remote key " .. remote_key .. "）")
      end
    end
    log.info("SaveDeck：诊断模式只读，未修改本地或远端存档；alias 键格式为 <平台>/<本地存档名>")
    return
  end

  local consumed_remote = {}
  local stats = { pull = 0, push = 0, conflict = 0, skip = 0 }
  local actions = 0

  local function process_local(key, local_item)
    if actions >= CONFIG.batch then
      return
    end
    local alias = CONFIG.aliases[key]
    local remote_stem = alias or local_item.stem
    local remote_key = local_item.platform .. "/" .. remote_stem
    local item = remote_by_key[remote_key]
    local matched_by = alias and "alias" or "romStem"
    if not item and CONFIG.match_by_name then
      item = remote_by_name[remote_key]
      matched_by = "name"
    end
    if not item then
      stats.skip = stats.skip + 1
      log.debug("SaveDeck：本地 " .. key .. " 没有匹配 archive（可能未索引）")
      return
    end
    consumed_remote[item.archive] = true
    log.debug(
      "SaveDeck：匹配 "
        .. key
        .. " → "
        .. remote_key
        .. "（"
        .. matched_by
        .. "："
        .. item.archive
        .. "）"
    )
    local read_ok, local_data = pcall(files.read, local_item.path)
    if not read_ok or not local_data then
      log.warn("SaveDeck：读取本地 " .. local_item.path .. " 失败：" .. tostring(local_data))
      return
    end
    local local_hash = hash.sha256(local_data)
    local last = state.games[key]
    local local_changed = last and local_hash ~= last.local_hash or not last
    local remote_changed = last and item.save_hash ~= last.remote_hash or not last

    if local_hash == item.save_hash then
      remember(state, key, local_hash, item.save_hash, item, local_item.path)
      return
    end
    if last and not local_changed and not remote_changed then
      remember(state, key, local_hash, item.save_hash, item, local_item.path)
      return
    end

    local direction
    local reason
    if last and local_changed and not remote_changed then
      direction = "push"
      reason = "仅本地变化"
    elseif last and remote_changed and not local_changed then
      direction = "pull"
      reason = "仅远端变化"
    else
      stats.conflict = stats.conflict + 1
      if CONFIG.on_conflict == "local" then
        direction = "push"
        reason = "冲突策略 local"
      elseif CONFIG.on_conflict == "remote" then
        direction = "pull"
        reason = "冲突策略 remote"
      elseif CONFIG.on_conflict == "ask" then
        stats.skip = stats.skip + 1
        log.info("SaveDeck：检测到冲突，但 on_conflict=ask 需要长按 Y 手动选择；本次跳过")
        return
      else
        local remote_time = http_epoch(item.modified)
        local local_time = local_item.mtime or 0
        direction = remote_time > local_time and "pull" or "push"
        reason = "冲突策略 newest"
      end
    end

    if direction == "pull" then
      if not can_pull then
        stats.skip = stats.skip + 1
        log.info("SaveDeck：需要拉取 " .. key .. "，但当前 mode 不允许")
        return
      end
      if pull(state, key, local_item, item, reason) then
        stats.pull = stats.pull + 1
        actions = actions + 1
      end
    elseif direction == "push" then
      if not can_push then
        stats.skip = stats.skip + 1
        log.info("SaveDeck：需要推送 " .. key .. "，但当前 mode 不允许")
        return
      end
      if push(state, key, local_item, item, local_data, local_hash, reason) then
        stats.push = stats.push + 1
        actions = actions + 1
      end
    end
  end

  for key, local_item in pairs(local_by_key) do
    if actions < CONFIG.batch then
      process_local(key, local_item)
    end
  end

  if can_pull then
    for key, item in pairs(remote_by_key) do
      if actions >= CONFIG.batch then
        break
      end
      if not local_by_key[key] and not consumed_remote[item.archive] then
        if pull(state, key, nil, item, "远端独有") then
          stats.pull = stats.pull + 1
          actions = actions + 1
        end
      end
    end
  end

  save_state(state)
  log.info(string.format(
    "SaveDeck：索引 %d/%d，拉取 %d，推送 %d，冲突 %d，跳过 %d",
    indexed,
    #remote_entries,
    stats.pull,
    stats.push,
    stats.conflict,
    stats.skip
  ))
end

local function cached_remote_item(cache, entry)
  local cached_platform = cache.platform
  return {
    archive = entry.name,
    size = entry.size,
    modified = entry.modified,
    game = cache.game,
    platform = local_platform(cached_platform),
    remote_platform = cache.remote_platform or cached_platform,
    stem = cache.stem,
    name = remote_name(cache.game),
    save_entry = cache.save_entry,
    save_hash = cache.save_hash,
  }
end

local function current_local_item()
  if type(ctx.game) ~= "table" then
    return nil, "缺少 ctx.game，无法识别当前游戏"
  end
  local platform = ctx.game.platform
  if not safe_segment(platform) then
    return nil, "当前游戏的平台名无效"
  end
  local candidates = {}
  if type(ctx.game.sav_path) == "string" and ctx.game.sav_path ~= "" then
    candidates[#candidates + 1] = ctx.game.sav_path
  end
  if type(ctx.game.srm_path) == "string"
    and ctx.game.srm_path ~= ""
    and ctx.game.srm_path ~= ctx.game.sav_path
  then
    candidates[#candidates + 1] = ctx.game.srm_path
  end
  for _, path in ipairs(candidates) do
    local ok, data = pcall(files.read, path)
    if ok and data then
      local name = path:match("([^/]+)$") or ""
      local stem = local_stem(name)
      if stem and safe_segment(stem) then
        local stat_ok, entry = pcall(files.stat, path)
        return {
          path = path,
          platform = platform,
          stem = stem,
          data = data,
          hash = hash.sha256(data),
          mtime = (stat_ok and entry and entry.mtime) or 0,
        }
      end
    end
  end
  local stem = trim(tostring(ctx.game.name or ""))
  if not safe_segment(stem) then
    return nil, "当前游戏没有可用的本地存档名"
  end
  local path = ctx.game.sav_path
  if type(path) ~= "string" or path == "" then
    path = local_path_for(platform, stem)
  end
  if not path then
    return nil, "无法生成本地存档路径"
  end
  return {
    path = path,
    platform = platform,
    stem = stem,
    data = nil,
    hash = nil,
    mtime = 0,
  }
end

local function find_remote_for(state, local_item, entries)
  local key = local_item.platform .. "/" .. local_item.stem
  local alias = CONFIG.aliases[key]
  local wanted_stem = alias or local_item.stem
  local function matches(item)
    if item.platform ~= local_item.platform then
      return false
    end
    return item.stem == wanted_stem
      or (CONFIG.match_by_name and item.name == wanted_stem)
  end
  local function from_cache(entry)
    local cache = state.index[entry.name]
    if cache_valid(cache, entry) then
      local item = cached_remote_item(cache, entry)
      if matches(item) then
        return item
      end
    end
    return nil
  end
  local function read_entry(entry)
    local read_ok, item, error_message, cache_entry = pcall(read_remote_archive, entry)
    if not read_ok or not item then
      log.debug("SaveDeck：跳过 archive " .. entry.name .. "：" .. tostring(read_ok and error_message or item))
      return nil
    end
    state.index[entry.name] = cache_entry
    if matches(item) then
      return item
    end
    return nil
  end

  local preferred = state.games[key] and state.games[key].archive
  local preferred_entry = nil
  for _, entry in ipairs(entries) do
    if entry.name == preferred then
      preferred_entry = entry
      break
    end
  end
  if preferred_entry then
    local item = from_cache(preferred_entry) or read_entry(preferred_entry)
    if item then
      return item, nil, false
    end
  end
  local reads = 0
  local limited = false
  for _, entry in ipairs(entries) do
    local lower = entry.name:lower()
    if not entry.is_dir and lower:sub(-4) == ".zip" and entry.name ~= preferred then
      local item = from_cache(entry)
      if not item then
        if reads >= CONFIG.batch then
          limited = true
          break
        end
        reads = reads + 1
        item = read_entry(entry)
      end
      if item then
        return item, nil, false
      end
    end
  end
  return nil, nil, limited
end

local function manual_report(status, detail, direction, error_kind)
  if type(mods) == "table" and type(mods.report) == "function" then
    local payload = { status = status, detail = detail }
    if direction then
      payload.direction = direction
    end
    if error_kind then
      payload.error = error_kind
    end
    local ok, error_message = pcall(mods.report, payload)
    if not ok then
      log.warn("SaveDeck：mods.report 失败：" .. tostring(error_message))
    end
  else
    log.info("SaveDeck：" .. tostring(detail))
  end
end

local function choose_conflict(stem)
  if type(mods) ~= "table" or type(mods.choose) ~= "function" then
    return nil, "当前 linkSlot 版本不支持冲突弹窗，请升级到 0.0.29+，或在页面改为 local/remote/newest"
  end
  local choice = mods.choose({
    key = "savedeck_conflict",
    title = "SaveDeck 存档冲突",
    body = "「" .. stem .. "」本地和 SaveDeck 都有改动，请选择保留哪一边。",
    default = "local",
    options = {
      { id = "local", label = "用本地覆盖 SaveDeck" },
      { id = "remote", label = "用 SaveDeck 覆盖本地" },
    },
  })
  if choice ~= "local" and choice ~= "remote" then
    return nil, "弹窗返回了未知选择：" .. tostring(choice)
  end
  return choice, nil
end

-- SaveDeck 里还没有这个游戏时给的下一步建议。横幅用它，弹窗也用它的一半。
local MISSING_ARCHIVE_KEY = "savedeck_missing_archive"

local function missing_archive_hint(stem)
  return "请先在 SaveDeck 里打开并保存一次「" .. stem .. "」，再回来长按 Y 同步"
end

-- 本地玩过、SaveDeck 里还没有对应 archive 时，弹一次说明。
--
-- 用户在下一次长按 Y 之前很可能没意识到"SaveDeck 那边要先有文件"，只报一句
-- noop 等于没提示。首次调用会中止脚本、由设备显示 modal；用户确认后脚本重跑，
-- 这里直接返回所选 id（两个 id 都是终态，不再重复弹窗）。
-- 调用点在任何写操作之前，所以"只弹窗、不写文件"天然成立。
local function advise_missing_archive(stem)
  if type(mods) ~= "table" or type(mods.choose) ~= "function" then
    return
  end
  mods.choose({
    key = MISSING_ARCHIVE_KEY,
    title = "SaveDeck 里还没有这个游戏",
    body = "「" .. stem .. "」在 SaveDeck 里还没有存档。",
    default = "ok",
    options = {
      { id = "ok", label = "知道了" },
      { id = "later", label = "稍后再说" },
    },
  })
end

local function run_manual()
  if ctx.timing ~= "manual" then
    manual_report("noop", "本动作只在手动同步（长按 Y）时运行")
    return
  end
  if ctx.unit ~= "sav" then
    manual_report("noop", "本动作只处理电池存档")
    return
  end
  load_config()
  if not find_profile() then
    manual_report("error", "没有找到所选 WebDAV 同步方式，请先打开 mod 管理页配置", nil, "config")
    return
  end
  if
    CONFIG.mode ~= "pull"
    and CONFIG.mode ~= "push"
    and CONFIG.mode ~= "two-way"
    and CONFIG.mode ~= "diagnose"
  then
    manual_report("error", "mode 只能是 pull / push / two-way / diagnose", nil, "config")
    return
  end
  local local_item, local_error = current_local_item()
  if not local_item then
    manual_report("error", local_error, nil, "config")
    return
  end
  local key = local_item.platform .. "/" .. local_item.stem
  local state = load_state()
  local entries, list_error = list_remote()
  if not entries then
    manual_report("error", "列出远端失败：" .. tostring(list_error), nil, "server")
    return
  end
  local item, find_error, index_limited = find_remote_for(state, local_item, entries)
  if not item then
    if index_limited then
      manual_report(
        "error",
        "远端 archive 较多，索引达到 batch 上限；请在设置中提高 batch 后重试",
        nil,
        "config"
      )
    elseif not local_item.data then
      manual_report("noop", "本地没有「" .. local_item.stem .. "」存档，SaveDeck 也没有对应 archive")
    else
      -- 本地玩过、SaveDeck 没有对应 archive：多半是这个游戏还没在 SaveDeck 里
      -- 保存过。诊断模式只读、不打扰，其余模式弹一次说明，否则用户只会看到
      -- 一句"没同步"却不知道下一步该做什么。
      if CONFIG.mode ~= "diagnose" then
        advise_missing_archive(local_item.stem)
      end
      manual_report(
        "noop",
        "SaveDeck 里还没有「" .. local_item.stem .. "」对应的 archive；"
          .. missing_archive_hint(local_item.stem)
      )
    end
    return
  end
  if CONFIG.mode == "diagnose" then
    manual_report(
      "noop",
      "诊断：本地 " .. key .. " 匹配 archive " .. item.archive .. "；未写入任何文件"
    )
    return
  end

  local local_hash = local_item.hash
  local remote_hash = item.save_hash
  local last = state.games[key]
  if local_hash and local_hash == remote_hash then
    remember(state, key, local_hash, remote_hash, item, local_item.path)
    manual_report("noop", "「" .. local_item.stem .. "」本地与 SaveDeck 一致，无需同步")
    return
  end
  if last and local_hash and local_hash == last.local_hash and remote_hash == last.remote_hash then
    remember(state, key, local_hash, remote_hash, item, local_item.path)
    manual_report("noop", "「" .. local_item.stem .. "」没有新的变化")
    return
  end

  local direction
  local reason
  if not local_item.data then
    direction = "pull"
    reason = "本地无存档"
  elseif not last then
    direction = nil
    reason = "首次同步"
  elseif local_hash ~= last.local_hash and remote_hash == last.remote_hash then
    direction = "push"
    reason = "仅本地变化"
  elseif local_hash == last.local_hash and remote_hash ~= last.remote_hash then
    direction = "pull"
    reason = "仅远端变化"
  else
    direction = nil
    reason = "本地和 SaveDeck 都修改过"
  end

  if not direction then
    if CONFIG.on_conflict == "local" then
      direction = "push"
      reason = "冲突策略 local"
    elseif CONFIG.on_conflict == "remote" then
      direction = "pull"
      reason = "冲突策略 remote"
    elseif CONFIG.on_conflict == "newest" then
      local remote_time = http_epoch(item.modified)
      direction = (remote_time > (local_item.mtime or 0)) and "pull" or "push"
      reason = "冲突策略 newest"
    else
      local choice, choose_error = choose_conflict(local_item.stem)
      if not choice then
        manual_report("error", choose_error, nil, "config")
        return
      end
      direction = choice == "local" and "push" or "pull"
      reason = choice == "local" and "用户选择本地" or "用户选择 SaveDeck"
    end
  end

  if direction == "pull" and CONFIG.mode == "push" then
    manual_report("noop", "需要从 SaveDeck 拉取，但 mode=push 只允许推送")
    return
  end
  if direction == "push" and CONFIG.mode == "pull" then
    manual_report("noop", "需要推送到 SaveDeck，但 mode=pull 只允许拉取")
    return
  end

  if direction == "pull" then
    if not pull(state, key, local_item, item, reason) then
      manual_report("error", "从 SaveDeck 拉取「" .. local_item.stem .. "」失败", "download", "server")
      return
    end
    manual_report("ok", "已从 SaveDeck 拉取「" .. local_item.stem .. "」", "download")
    return
  end

  if not local_item.data then
    manual_report("error", "本地存档在读取过程中消失", nil, "fs")
    return
  end
  if not push(state, key, local_item, item, local_item.data, local_hash, reason) then
    manual_report("error", "上传「" .. local_item.stem .. "」到 SaveDeck 失败", "upload", "server")
    return
  end
  manual_report("ok", "已把「" .. local_item.stem .. "」上传到 SaveDeck", "upload")
end

if ctx.timing == "manual" then
  run_manual()
else
  local ok, error_message = pcall(run)
  if not ok then
    log.error("SaveDeck：执行失败：" .. tostring(error_message))
  end
end
