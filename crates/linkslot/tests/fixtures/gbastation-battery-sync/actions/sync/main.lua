-- GBAStation battery-save sync (linkSlot manual action).
--
-- This script runs only through the manual timing (`linkSlot mods run`, i.e.
-- long-press Y on a cartridge). It talks to the GBAStation LAN web service:
--   GET  /api/games
--   GET  /api/game/<id>/saves
--   GET  /api/game/<id>/save/export?path=...
--   POST /api/game/<id>/save/start
--   POST /api/upload/chunk?token=...&offset=0
--   POST /api/upload/finish
--   POST /api/upload/cancel (best effort)
--
-- Battery saves (.sav/.srm) and the game playtime are handled. Every host call is protected
-- with pcall; failures end in exactly one mods.report() with a short Chinese
-- single-line detail.

local PLATFORM_BY_ID = { [1] = "GBA", [2] = "GBC", [3] = "GB" }
local ALLOWED_PLATFORMS = { GBA = true, GB = true, GBC = true }
local STRIP_EXTENSIONS = {
  gba = true,
  gb = true,
  gbc = true,
  zip = true,
  ["7z"] = true,
  sav = true,
  srm = true,
}
local MAX_DETAIL_BYTES = 480
local MAX_LOCAL_SAVE_BYTES = 32 * 1024 * 1024
local MAX_REMOTE_SAVE_BYTES = 16 * 1024 * 1024

-- ---------------------------------------------------------------------------
-- Small shared helpers
-- ---------------------------------------------------------------------------

local function trim_string(value)
  if type(value) ~= "string" then
    return ""
  end
  return (value:gsub("^%s+", ""):gsub("%s+$", ""))
end

local function ascii_lower(value)
  return (value:gsub("[A-Z]", function(character)
    return string.char(character:byte() + 32)
  end))
end

-- Host errors and our own texts are UTF-8, but an HTTP error body is raw bytes.
-- Never copy invalid bytes into mods.report (the host expects a UTF-8 String).
local function is_valid_utf8(value)
  if type(utf8) ~= "table" or type(utf8.len) ~= "function" then
    return true
  end
  local ok, length = pcall(utf8.len, value)
  return ok and length ~= nil
end

-- Keep one physical line and stay below the host's 500-character report cap.
-- The cut is made on a UTF-8 character boundary so Chinese text stays valid.
local function one_line(value, limit)
  local message
  if type(value) == "string" then
    message = value
  else
    message = tostring(value or "")
  end
  message = message:gsub("[\r\n\t]+", " ")
  message = message:gsub("%c", " ")
  message = message:gsub("%s+", " ")
  message = message:gsub("^ ", ""):gsub(" $", "")
  limit = limit or MAX_DETAIL_BYTES
  if #message <= limit then
    return message
  end
  local cut = limit - 3
  if cut < 1 then
    return "…"
  end
  local byte = message:byte(cut)
  if byte and byte >= 0x80 then
    if byte < 0xC0 then
      local start = cut
      while start > 1 do
        local previous = message:byte(start)
        if not previous or previous < 0x80 or previous >= 0xC0 then
          break
        end
        start = start - 1
      end
      cut = start - 1
    else
      cut = cut - 1
    end
  end
  if cut < 1 then
    return "…"
  end
  return message:sub(1, cut) .. "…"
end

local function emit_log(level, message)
  if type(log) == "table" and type(log[level]) == "function" then
    pcall(log[level], one_line(message, 400))
  end
end

local function human_size(bytes)
  local value = math.floor(tonumber(bytes) or 0)
  if value < 1024 then
    return string.format("%d B", value)
  end
  if value < 1024 * 1024 then
    return string.format("%.1f KiB", value / 1024)
  end
  return string.format("%.1f MiB", value / (1024 * 1024))
end

local function ok_report(direction, detail)
  return { status = "ok", direction = direction, detail = detail }
end

local function noop_report(detail)
  return { status = "noop", detail = detail }
end

local choice_requested = false
local playtime_game = nil
local playtime_platform = ""
local playtime_key = ""
local playtime_name = ""

-- Ask the device which side wins.  This is a resumable checkpoint: on the
-- first run linkSlot records the request and relaunches with --choice; on the
-- resumed run mods.choose returns "remote" or "local".  Do not wrap it in
-- pcall and do not write any save before it.
local function choose_side(key, title, body)
  if type(mods) ~= "table" or type(mods.choose) ~= "function" then
    return nil, "当前 linkSlot 版本不支持弹窗选择，请升级到 0.0.29 或到 mod 管理页改为 remote/local"
  end
  local choice = mods.choose({
    key = key,
    title = title,
    body = body,
    default = "remote",
    options = {
      { id = "remote", label = "用 GBAStation 覆盖本地" },
      { id = "local", label = "用本地覆盖 GBAStation" },
    },
  })
  if choice ~= "remote" and choice ~= "local" then
    return nil, "弹窗返回了未知选择：" .. tostring(choice)
  end
  return choice, nil
end

local function error_report(kind, detail, direction)
  return { status = "error", error = kind, direction = direction, detail = detail }
end

local function emit_report(result)
  local payload = {
    status = result.status,
    detail = one_line(result.detail or "GBAStation 同步结束"),
  }
  if result.direction ~= nil then
    payload.direction = result.direction
  end
  if result.error ~= nil then
    payload.error = result.error
  end
  if type(mods) == "table" and type(mods.report) == "function" then
    local ok, error_message = pcall(mods.report, payload)
    if not ok then
      emit_log("warn", "mods.report 调用失败：" .. tostring(error_message))
    end
  else
    emit_log("warn", "当前 linkSlot 不支持 mods.report：" .. payload.detail)
  end
end

-- ---------------------------------------------------------------------------
-- Host wrappers (files / JSON / hash / HTTP)
-- ---------------------------------------------------------------------------

local function read_file(path)
  if type(files) ~= "table" or type(files.read) ~= "function" then
    return nil, "当前 linkSlot 不支持 files.read"
  end
  local ok, data = pcall(files.read, path)
  if not ok then
    return nil, tostring(data)
  end
  if data == nil then
    return nil, nil
  end
  if type(data) ~= "string" then
    return nil, "files.read 返回了非字符串数据"
  end
  return data, nil
end

local function write_file(path, data)
  if type(files) ~= "table" or type(files.write) ~= "function" then
    return nil, "当前 linkSlot 不支持 files.write"
  end
  local ok, error_message = pcall(files.write, path, data)
  if not ok then
    return nil, tostring(error_message)
  end
  return true, nil
end

local function sha256(data)
  if type(hash) ~= "table" or type(hash.sha256) ~= "function" then
    return nil, "当前 linkSlot 不支持 hash.sha256"
  end
  local ok, digest = pcall(hash.sha256, data)
  if not ok then
    return nil, tostring(digest)
  end
  if type(digest) ~= "string" or #digest ~= 64 then
    return nil, "hash.sha256 返回了无效结果"
  end
  return digest, nil
end

local function encode_json(value)
  if type(json) ~= "table" or type(json.encode) ~= "function" then
    return nil, "当前 linkSlot 不支持 json.encode"
  end
  local ok, encoded = pcall(json.encode, value)
  if not ok then
    return nil, tostring(encoded)
  end
  if type(encoded) ~= "string" then
    return nil, "json.encode 返回了非字符串结果"
  end
  return encoded, nil
end

local function try_decode_json(value)
  if type(json) ~= "table" or type(json.decode) ~= "function" then
    return nil
  end
  local ok, decoded = pcall(json.decode, value)
  if not ok then
    return nil
  end
  return decoded
end

local function http_error_kind(error_message)
  local text = ascii_lower(tostring(error_message or ""))
  if text:find("[timeout]", 1, true)
    or text:find("timeout", 1, true)
    or text:find("timed out", 1, true)
    or text:find("超时", 1, true)
  then
    return "timeout"
  end
  if text:find("[config]", 1, true)
    or text:find("config.json", 1, true)
    or text:find("base_url", 1, true)
    or text:find("配置", 1, true)
  then
    return "config"
  end
  if text:find("[fs]", 1, true) then
    return "fs"
  end
  if text:find("[verify]", 1, true) then
    return "verify"
  end
  if text:find("[server]", 1, true) then
    return "server"
  end
  return "net"
end

local function validate_http_response(response)
  if type(response) ~= "table" then
    return nil, "net", "http 没有返回 {status, body}"
  end
  if type(response.status) ~= "number" then
    return nil, "net", "http 返回了无效 status"
  end
  if type(response.body) ~= "string" then
    return nil, "net", "http 返回了无效 body"
  end
  return response, nil, nil
end

local function do_get(segments, query)
  if type(http) ~= "table" or type(http.get) ~= "function" then
    return nil, "config", "当前 linkSlot 不支持 http.get"
  end
  local ok, response
  if query ~= nil then
    ok, response = pcall(http.get, segments, query)
  else
    ok, response = pcall(http.get, segments)
  end
  if not ok then
    return nil, http_error_kind(response), "请求 GBAStation 失败：" .. tostring(response)
  end
  return validate_http_response(response)
end

local function do_post(segments, body, content_type, query)
  if type(http) ~= "table" or type(http.post) ~= "function" then
    return nil, "config", "当前 linkSlot 不支持 http.post"
  end
  local ok, response
  if query ~= nil then
    ok, response = pcall(http.post, segments, body, content_type, query)
  else
    ok, response = pcall(http.post, segments, body, content_type)
  end
  if not ok then
    return nil, http_error_kind(response), "请求 GBAStation 失败：" .. tostring(response)
  end
  return validate_http_response(response)
end

local function do_put(segments, body, content_type)
  if type(http) ~= "table" or type(http.request) ~= "function" then
    return nil, "config", "当前 linkSlot 不支持 http.request"
  end
  local ok, response = pcall(http.request, "PUT", segments, {
    body = body,
    content_type = content_type,
  })
  if not ok then
    return nil, http_error_kind(response), "请求 GBAStation 失败：" .. tostring(response)
  end
  return validate_http_response(response)
end

local function is_success(response)
  return response.status >= 200 and response.status < 300
end

local function describe_response(response)
  local description = "HTTP " .. tostring(response.status)
  local body = response.body
  if type(body) == "string" and #body > 0 then
    local decoded = try_decode_json(body)
    local message = ""
    if type(decoded) == "table" and type(decoded.error) == "string" then
      message = trim_string(decoded.error)
    end
    if message == "" and is_valid_utf8(body) then
      message = trim_string(body:sub(1, 160))
    end
    if message ~= "" then
      description = description .. "：" .. message
    end
  end
  return one_line(description, 220)
end

local function read_json_response(response, what)
  if not is_success(response) then
    return nil, "server", what .. "失败（" .. describe_response(response) .. "）"
  end
  local decoded = try_decode_json(response.body)
  if type(decoded) ~= "table" then
    return nil, "server", what .. "返回了无法解析的 JSON"
  end
  if decoded.ok == false then
    local message = trim_string(decoded.error)
    if message == "" then
      message = "GBAStation 返回 ok=false"
    end
    return nil, "server", what .. "被 GBAStation 拒绝：" .. message
  end
  return decoded, nil, nil
end

-- ---------------------------------------------------------------------------
-- Name / platform helpers
-- ---------------------------------------------------------------------------

local function file_name(path)
  local value = trim_string(path)
  local name = value:match("([^/\\]+)$")
  if name == nil then
    return ""
  end
  return name
end

local function strip_known_ext(name)
  local value = trim_string(name)
  local lower = ascii_lower(value)
  for extension in pairs(STRIP_EXTENSIONS) do
    local suffix = "." .. extension
    if #lower > #suffix and lower:sub(-#suffix) == suffix then
      return value:sub(1, #value - #suffix)
    end
  end
  return value
end

local function stem_of(path)
  return strip_known_ext(file_name(path))
end

local function normalize_name(name)
  local value = ascii_lower(strip_known_ext(name))
  value = value:gsub("[%s%-_%.%(%)%[%]{}%,%:;%'\"!%?&%%]+", "")
  return value
end

local function game_platform(game)
  if type(game) ~= "table" then
    return nil
  end
  local value = game.platform
  if type(value) == "number" then
    return PLATFORM_BY_ID[math.floor(value)]
  end
  if type(value) == "string" then
    local upper = string.upper(trim_string(value))
    if ALLOWED_PLATFORMS[upper] then
      return upper
    end
    local numeric = tonumber(upper)
    if numeric ~= nil then
      return PLATFORM_BY_ID[math.floor(numeric)]
    end
  end
  return nil
end

local function platform_allowed(config, platform)
  for _, item in ipairs(config.platforms) do
    if item == platform then
      return true
    end
  end
  return false
end

local function game_key(game)
  return tostring(game.id or "") .. "\n" .. tostring(game.path or "") .. "\n" .. tostring(game.title or "")
end

local function exact_equal(left, right)
  return left == right
end

local function lower_equal(left, right)
  return ascii_lower(left) == ascii_lower(right)
end

local function normalized_equal(left, right)
  return normalize_name(left) == normalize_name(right)
end

local function collect_matches(games, platform, local_name, local_stem, compare)
  local matches = {}
  local seen = {}
  for _, game in ipairs(games) do
    if game_platform(game) == platform then
      local key = game_key(game)
      if not seen[key] then
        local title = strip_known_ext(game.title)
        local path_stem = strip_known_ext(stem_of(game.path))
        local hit = false
        local local_keys = { local_name, local_stem }
        local remote_keys = { title, path_stem }
        for _, left in ipairs(local_keys) do
          for _, right in ipairs(remote_keys) do
            if left ~= "" and right ~= "" and compare(left, right) then
              hit = true
            end
          end
        end
        if hit then
          seen[key] = true
          matches[#matches + 1] = game
        end
      end
    end
  end
  return matches
end

local function mapping_for(games, local_name, local_stem)
  local direct = games[local_name]
  if direct ~= nil then
    return direct, nil
  end
  direct = games[local_stem]
  if direct ~= nil then
    return direct, nil
  end
  local lowered_name = ascii_lower(local_name)
  local lowered_stem = ascii_lower(local_stem)
  local found = nil
  local found_value = nil
  for key, value in pairs(games) do
    local lowered_key = ascii_lower(trim_string(key))
    if lowered_key == lowered_name or lowered_key == lowered_stem then
      if found ~= nil then
        return nil, "games 映射中有多条键与「" .. local_name .. "」匹配，请只保留一条"
      end
      found = key
      found_value = value
    end
  end
  if found ~= nil then
    return found_value, nil
  end
  return nil, nil
end

local function mapping_matches(games, platform, target, compare)
  local matches = {}
  local seen = {}
  local target_stem = strip_known_ext(file_name(target))
  for _, game in ipairs(games) do
    if game_platform(game) == platform then
      local key = game_key(game)
      if not seen[key] then
        local title = strip_known_ext(game.title)
        local path = trim_string(game.path)
        local path_stem = strip_known_ext(stem_of(path))
        local hit = false
        if path ~= "" and compare(target, path) then
          hit = true
        end
        if not hit and title ~= "" and compare(target, title) then
          hit = true
        end
        if not hit and path_stem ~= "" and compare(target, path_stem) then
          hit = true
        end
        if not hit and target_stem ~= "" then
          if title ~= "" and compare(target_stem, title) then
            hit = true
          elseif path_stem ~= "" and compare(target_stem, path_stem) then
            hit = true
          end
        end
        if hit then
          seen[key] = true
          matches[#matches + 1] = game
        end
      end
    end
  end
  return matches
end

local function resolve_mapping(games, platform, mapping)
  local target = trim_string(mapping)
  if target == "" then
    return nil, "games 映射值为空"
  end

  local by_id = {}
  local seen = {}
  for _, game in ipairs(games) do
    if game_platform(game) == platform and tostring(game.id or "") == target then
      local key = game_key(game)
      if not seen[key] then
        seen[key] = true
        by_id[#by_id + 1] = game
      end
    end
  end
  if #by_id == 1 then
    return by_id[1], nil
  end
  if #by_id > 1 then
    return nil, "映射值「" .. target .. "」匹配到多个游戏 id"
  end

  local stages = {
    { label = "精确", compare = exact_equal },
    { label = "忽略大小写", compare = lower_equal },
    { label = "规范化", compare = normalized_equal },
  }
  for _, stage in ipairs(stages) do
    local matches = mapping_matches(games, platform, target, stage.compare)
    if #matches == 1 then
      return matches[1], nil
    end
    if #matches > 1 then
      return nil, "映射值「" .. target .. "」按" .. stage.label .. "匹配到多个游戏，请改为游戏 id"
    end
  end
  return nil, "映射值「" .. target .. "」没有匹配到同平台的 GBAStation 游戏"
end

local function select_game(config, games, platform, local_name)
  local local_stem = strip_known_ext(local_name)
  local mapping_value, mapping_error = mapping_for(config.games, local_name, local_stem)
  if mapping_error ~= nil then
    return nil, mapping_error
  end
  if mapping_value ~= nil then
    local mapped, resolve_error = resolve_mapping(games, platform, mapping_value)
    if mapped ~= nil then
      return mapped, nil
    end
    return nil, "games 映射「" .. local_name .. "」无法使用：" .. tostring(resolve_error)
  end

  local stages = {
    { label = "精确", compare = exact_equal },
    { label = "忽略大小写", compare = lower_equal },
    { label = "规范化", compare = normalized_equal },
  }
  for _, stage in ipairs(stages) do
    local matches = collect_matches(games, platform, local_name, local_stem, stage.compare)
    if #matches == 1 then
      return matches[1], nil
    end
    if #matches > 1 then
      return nil,
        "在 GBAStation 找到多个 " .. platform .. " 游戏与「" .. local_name .. "」" .. stage.label
          .. "匹配；请在 games 映射中指定游戏 id"
    end
  end
  return nil,
    "在 GBAStation 找不到平台 " .. platform .. " 的同名游戏「" .. local_name
      .. "」；请在 mod 管理页的 games 映射中指定游戏 id"
end

-- ---------------------------------------------------------------------------
-- config.json
-- ---------------------------------------------------------------------------

local function load_config()
  if type(ctx) ~= "table" or type(ctx.mod) ~= "table" then
    return nil, "缺少 ctx.mod 上下文"
  end
  local config_error = trim_string(ctx.mod.config_error)
  if config_error ~= "" then
    return nil, "config.json 读取失败：" .. config_error
  end
  local raw = ctx.mod.config
  if raw == nil then
    raw = {}
  end
  if type(raw) ~= "table" then
    return nil, "config.json 必须是 JSON 对象"
  end

  local config = {
    base_url = "",
    mode = "two-way",
    first_sync = "remote",
    on_conflict = "ask",
    backup = true,
    verify = true,
    platforms = { "GBA", "GB", "GBC" },
    games = {},
  }

  if raw.base_url ~= nil then
    if type(raw.base_url) ~= "string" then
      return nil, "base_url 必须是字符串"
    end
    config.base_url = trim_string(raw.base_url)
  end

  if raw.mode ~= nil then
    if type(raw.mode) ~= "string" then
      return nil, "mode 必须是字符串"
    end
    local mode = ascii_lower(trim_string(raw.mode))
    if mode ~= "two-way" and mode ~= "pull" and mode ~= "push" then
      return nil, "mode 只能是 two-way / pull / push"
    end
    config.mode = mode
  end

  if raw.first_sync ~= nil then
    if type(raw.first_sync) ~= "string" then
      return nil, "first_sync 必须是字符串"
    end
    local first_sync = ascii_lower(trim_string(raw.first_sync))
    if first_sync ~= "remote" and first_sync ~= "local" and first_sync ~= "ask" then
      return nil, "first_sync 只能是 remote / local / ask"
    end
    config.first_sync = first_sync
  end

  if raw.on_conflict ~= nil then
    if type(raw.on_conflict) ~= "string" then
      return nil, "on_conflict 必须是字符串"
    end
    local on_conflict = ascii_lower(trim_string(raw.on_conflict))
    if on_conflict ~= "ask" and on_conflict ~= "remote" and on_conflict ~= "local" then
      return nil, "on_conflict 只能是 ask / remote / local"
    end
    config.on_conflict = on_conflict
  end

  if raw.backup ~= nil then
    if type(raw.backup) ~= "boolean" then
      return nil, "backup 必须是布尔值"
    end
    config.backup = raw.backup
  end
  if raw.verify ~= nil then
    if type(raw.verify) ~= "boolean" then
      return nil, "verify 必须是布尔值"
    end
    config.verify = raw.verify
  end

  if raw.platforms ~= nil then
    if type(raw.platforms) ~= "table" then
      return nil, "platforms 必须是数组"
    end
    local platforms = {}
    local seen = {}
    for _, value in ipairs(raw.platforms) do
      if type(value) ~= "string" then
        return nil, "platforms 只能包含字符串"
      end
      local platform = string.upper(trim_string(value))
      if not ALLOWED_PLATFORMS[platform] then
        return nil, "platforms 只能包含 GBA / GB / GBC"
      end
      if not seen[platform] then
        seen[platform] = true
        platforms[#platforms + 1] = platform
      end
    end
    if #platforms == 0 then
      return nil, "platforms 不能为空"
    end
    config.platforms = platforms
  end

  if raw.games ~= nil then
    if type(raw.games) ~= "table" then
      return nil, "games 必须是 JSON 对象"
    end
    local games = {}
    local count = 0
    for key, value in pairs(raw.games) do
      count = count + 1
      if count > 256 then
        return nil, "games 最多 256 项"
      end
      if type(key) ~= "string" or trim_string(key) == "" then
        return nil, "games 的键必须是非空字符串"
      end
      if type(value) ~= "string" or trim_string(value) == "" then
        return nil, "games 的值必须是非空字符串"
      end
      local trimmed_key = trim_string(key)
      if games[trimmed_key] ~= nil then
        return nil, "games 的键重复：" .. trimmed_key
      end
      games[trimmed_key] = trim_string(value)
    end
    config.games = games
  end

  return config, nil
end

-- ---------------------------------------------------------------------------
-- GBAStation remote helpers
-- ---------------------------------------------------------------------------

local function find_remote_battery(remote_game)
  local response, kind, message = do_get({ "api", "game", remote_game.id, "saves" })
  if response == nil then
    return nil, false, kind, message
  end
  if response.status == 404 then
    return nil, true, nil, nil
  end
  local data, data_kind, data_message = read_json_response(response, "读取 GBAStation 存档列表")
  if data == nil then
    return nil, false, data_kind, data_message
  end
  local battery = data.battery
  if battery == nil then
    return nil, true, nil, nil
  end
  if type(battery) ~= "table" then
    return nil, false, "server", "GBAStation 存档列表中的 battery 不是数组"
  end
  if #battery == 0 then
    return nil, true, nil, nil
  end
  if #battery > 1 then
    return nil, false, "server", "GBAStation 返回了多个电池存档，暂不支持自动选择"
  end
  local entry = battery[1]
  if type(entry) ~= "table" then
    return nil, false, "server", "GBAStation 返回的电池存档条目无效"
  end
  local remote_path = trim_string(entry.path)
  if remote_path == "" then
    return nil, false, "server", "GBAStation 返回的电池存档条目缺少 path"
  end
  local remote_name = trim_string(entry.name)
  if remote_name == "" then
    remote_name = file_name(remote_path)
  end
  return {
    id = remote_game.id,
    title = trim_string(remote_game.title),
    platform = game_platform(remote_game),
    path = remote_path,
    name = remote_name,
    size = tonumber(entry.size),
    modified = trim_string(entry.modified),
  }, false, nil, nil
end

local function fetch_remote_data(remote_game, remote_ref)
  local response, kind, message = do_get(
    { "api", "game", remote_game.id, "save", "export" },
    { path = remote_ref.path }
  )
  if response == nil then
    return nil, kind, message, false
  end
  if response.status == 404 then
    return nil, nil, nil, true
  end
  if not is_success(response) then
    return nil, "server", "下载 GBAStation 存档失败（" .. describe_response(response) .. "）", false
  end
  local data = response.body
  if type(data) ~= "string" or #data == 0 then
    return nil, "server", "GBAStation 返回的电池存档为空", false
  end
  if #data > MAX_REMOTE_SAVE_BYTES then
    return nil, "server", "GBAStation 存档超过 16 MiB 读取上限", false
  end
  local expected = tonumber(remote_ref.size)
  if expected ~= nil and expected > 0 and expected ~= #data then
    return nil,
      "server",
      "GBAStation 存档大小与列表不一致（列表 " .. tostring(expected) .. " 字节，实际 " .. tostring(#data) .. " 字节）",
      false
  end
  return data, nil, nil, false
end

-- ---------------------------------------------------------------------------
-- Local save helpers
-- ---------------------------------------------------------------------------

local function local_candidates()
  local candidates = {}
  local seen = {}
  local function add(value)
    local path = trim_string(value)
    if path ~= "" and not seen[path] then
      seen[path] = true
      candidates[#candidates + 1] = path
    end
  end
  if type(ctx.game) == "table" then
    add(ctx.game.sav_path)
    add(ctx.game.srm_path)
  end
  add(ctx.unit_path)
  return candidates
end

local function read_local_save()
  local candidates = local_candidates()
  if #candidates == 0 then
    return nil, "无法确定本地 .sav/.srm 存档路径"
  end
  local empty_path = nil
  for _, path in ipairs(candidates) do
    local data, read_error = read_file(path)
    if read_error ~= nil then
      return nil, "读取本地存档失败：" .. read_error
    end
    if data ~= nil then
      if #data == 0 then
        if empty_path == nil then
          empty_path = path
        end
      else
        if #data > MAX_LOCAL_SAVE_BYTES then
          return nil, "本地存档超过 32 MiB 上传上限"
        end
        local digest, digest_error = sha256(data)
        if digest == nil then
          return nil, "计算本地存档哈希失败：" .. tostring(digest_error)
        end
        return {
          path = path,
          data = data,
          hash = digest,
          size = #data,
        }, nil, nil
      end
    end
  end
  return nil, nil, empty_path
end

local function has_save_extension(path)
  local lower = ascii_lower(path)
  return lower:sub(-4) == ".sav" or lower:sub(-4) == ".srm"
end

local function preferred_download_path(local_save, empty_path)
  local candidates = {}
  if local_save ~= nil then
    candidates[#candidates + 1] = local_save.path
  end
  if empty_path ~= nil then
    candidates[#candidates + 1] = empty_path
  end
  if type(ctx.game) == "table" then
    candidates[#candidates + 1] = trim_string(ctx.game.sav_path)
    candidates[#candidates + 1] = trim_string(ctx.game.srm_path)
  end
  candidates[#candidates + 1] = trim_string(ctx.unit_path)
  for _, path in ipairs(candidates) do
    if path ~= "" and has_save_extension(path) then
      return path
    end
  end
  return nil
end

-- ---------------------------------------------------------------------------
-- State and backups
-- ---------------------------------------------------------------------------

local function state_path(platform, key)
  return ctx.mod.dir .. "/state/" .. platform .. "/" .. key .. ".json"
end

local function load_state(platform, key)
  local data, read_error = read_file(state_path(platform, key))
  if read_error ~= nil then
    emit_log("warn", "读取同步状态失败，按首次同步处理：" .. read_error)
    return nil
  end
  if data == nil then
    return nil
  end
  local decoded = try_decode_json(data)
  if type(decoded) ~= "table" then
    emit_log("warn", "同步状态文件损坏，按首次同步处理")
    return nil
  end
  if type(decoded.local_hash) ~= "string" or type(decoded.remote_hash) ~= "string" then
    emit_log("warn", "同步状态文件缺少哈希，按首次同步处理")
    return nil
  end
  return {
    local_hash = decoded.local_hash,
    remote_hash = decoded.remote_hash,
    remote_path = type(decoded.remote_path) == "string" and decoded.remote_path or nil,
    remote_name = type(decoded.remote_name) == "string" and decoded.remote_name or nil,
    local_path = type(decoded.local_path) == "string" and decoded.local_path or nil,
  }
end

local function save_state(platform, name, key, local_hash, remote_hash, local_path, remote_path, remote_name)
  local payload = {
    version = 1,
    platform = platform,
    name = name,
    key = key,
    local_hash = local_hash,
    remote_hash = remote_hash,
    local_path = local_path or "",
    remote_path = remote_path or "",
    remote_name = remote_name or "",
  }
  local encoded, encode_error = encode_json(payload)
  if encoded == nil then
    return nil, "生成同步状态失败：" .. tostring(encode_error)
  end
  local ok, write_error = write_file(state_path(platform, key), encoded)
  if not ok then
    return nil, write_error
  end
  return true, nil
end

local function write_backup(platform, key, side, data)
  local path = ctx.mod.dir .. "/backups/" .. platform .. "/" .. key .. "-" .. side .. ".sav"
  local ok, write_error = write_file(path, data)
  if not ok then
    return nil, write_error
  end
  return true, nil
end

local function remember_equal_state(platform, key, name, local_save, remote_entry, remote_hash)
  local previous = load_state(platform, key)
  local remote_path = remote_entry and remote_entry.path or (previous and previous.remote_path) or ""
  local remote_name = remote_entry and remote_entry.name or (previous and previous.remote_name) or ""
  if previous ~= nil
    and previous.local_hash == remote_hash
    and previous.remote_hash == remote_hash
    and (previous.remote_path or "") == remote_path
  then
    return true, nil
  end
  return save_state(platform, name, key, remote_hash, remote_hash, local_save.path, remote_path, remote_name)
end

-- ---------------------------------------------------------------------------
-- Transfer operations
-- ---------------------------------------------------------------------------

local function do_download(config, platform, key, game_name, remote_entry, remote_data, remote_hash, local_save, empty_path)
  local target_path = preferred_download_path(local_save, empty_path)
  if target_path == nil then
    return nil, "config", "无法确定本地 .sav/.srm 存档路径"
  end
  if not has_save_extension(target_path) then
    return nil, "config", "本地存档路径不是 .sav 或 .srm，已停止下载"
  end

  if config.backup and local_save ~= nil and #local_save.data > 0 then
    local backed, backup_error = write_backup(platform, key, "local", local_save.data)
    if not backed then
      return nil, "fs", "写入本地备份失败，已停止下载：" .. tostring(backup_error)
    end
  end

  local written, write_error = write_file(target_path, remote_data)
  if not written then
    return nil, "fs", "写入本地存档失败：" .. tostring(write_error)
  end

  local saved, state_error =
    save_state(platform, game_name, key, remote_hash, remote_hash, target_path, remote_entry.path, remote_entry.name)
  if not saved then
    return nil, "fs", "已下载但同步状态写入失败：" .. tostring(state_error)
  end
  return true, target_path
end

local function do_upload(config, platform, key, game_name, remote_game, remote_entry, remote_data, local_save)
  local size = local_save.size
  if size > MAX_LOCAL_SAVE_BYTES then
    return nil, "fs", "本地存档超过 32 MiB 上传上限"
  end

  if config.backup and remote_entry ~= nil and remote_data ~= nil then
    local backed, backup_error = write_backup(platform, key, "remote", remote_data)
    if not backed then
      return nil, "fs", "写入远端备份失败，已停止上传：" .. tostring(backup_error)
    end
  end

  local upload_name = remote_entry and remote_entry.name or file_name(local_save.path)
  if upload_name == "" then
    upload_name = file_name(local_save.path)
  end
  if upload_name == "" then
    upload_name = "save.sav"
  end

  local start_body, start_encode_error = encode_json({
    name = upload_name,
    size = size,
    type = "battery",
    slot = 0,
  })
  if start_body == nil then
    return nil, "server", "生成上传请求失败：" .. tostring(start_encode_error)
  end

  local start_response, start_kind, start_message = do_post(
    { "api", "game", remote_game.id, "save", "start" },
    start_body,
    "application/json"
  )
  if start_response == nil then
    return nil, start_kind, "启动 GBAStation 上传失败：" .. tostring(start_message)
  end
  if not is_success(start_response) then
    return nil, "server", "启动 GBAStation 上传失败（" .. describe_response(start_response) .. "）"
  end
  local start_data, start_data_kind, start_data_message = read_json_response(start_response, "启动 GBAStation 上传")
  if start_data == nil then
    return nil, start_data_kind, start_data_message
  end
  local token = trim_string(start_data.token)
  if token == "" then
    return nil, "server", "GBAStation 未返回上传 token"
  end
  local target_path = trim_string(start_data.targetPath)
  if target_path == "" and remote_entry ~= nil then
    target_path = remote_entry.path
  end

  local finished = false
  local function cancel_upload()
    if finished then
      return
    end
    local cancel_body = encode_json({ token = token })
    if cancel_body ~= nil then
      pcall(do_post, { "api", "upload", "cancel" }, cancel_body, "application/json")
    end
  end

  local chunk_response, chunk_kind, chunk_message = do_post(
    { "api", "upload", "chunk" },
    local_save.data,
    "application/octet-stream",
    { token = token, offset = 0 }
  )
  if chunk_response == nil then
    cancel_upload()
    return nil, chunk_kind, "上传存档数据失败（已尝试取消）：" .. tostring(chunk_message)
  end
  if not is_success(chunk_response) then
    local description = describe_response(chunk_response)
    cancel_upload()
    return nil, "server", "上传存档数据失败（" .. description .. "；已尝试取消）"
  end

  local finish_body, finish_encode_error = encode_json({ token = token })
  if finish_body == nil then
    cancel_upload()
    return nil, "server", "生成上传完成请求失败：" .. tostring(finish_encode_error)
  end
  local finish_response, finish_kind, finish_message = do_post(
    { "api", "upload", "finish" },
    finish_body,
    "application/json"
  )
  if finish_response == nil then
    -- The chunk already wrote the target file. GBAStation's cancel endpoint
    -- deletes that file, so finishing must never cancel here.
    return nil,
      finish_kind,
      "上传完成确认失败（数据可能已写入远端，请检查备份）：" .. tostring(finish_message)
  end
  if not is_success(finish_response) then
    -- Do not cancel: the chunk already wrote the target and cancel deletes it.
    return nil,
      "server",
      "上传完成确认失败（" .. describe_response(finish_response) .. "；数据可能已写入远端，请检查备份）"
  end
  local finish_data, finish_data_kind, finish_data_message =
    read_json_response(finish_response, "完成 GBAStation 上传")
  if finish_data == nil then
    return nil, finish_data_kind, finish_data_message
  end
  finished = true

  local remote_hash = local_save.hash
  local remote_path = target_path
  local remote_name = upload_name
  if config.verify then
    if remote_path == "" then
      local listed, absent, list_kind, list_message = find_remote_battery(remote_game)
      if listed == nil then
        if list_kind ~= nil then
          return nil, list_kind, "上传后校验失败：" .. tostring(list_message)
        end
        if absent then
          return nil, "verify", "上传后校验失败：GBAStation 没有列出电池存档"
        end
        return nil, "verify", "上传后校验失败：无法读取 GBAStation 存档列表"
      end
      remote_path = listed.path
      remote_name = listed.name
    end

    local verify_data, verify_kind, verify_message, verify_absent =
      fetch_remote_data(remote_game, { path = remote_path, size = size })
    if verify_data == nil then
      if verify_absent then
        return nil, "verify", "上传后校验失败：远端没有存档"
      end
      return nil, verify_kind or "verify", "上传后校验失败：" .. tostring(verify_message)
    end
    local digest, digest_error = sha256(verify_data)
    if digest == nil then
      return nil, "verify", "上传后校验哈希失败：" .. tostring(digest_error)
    end
    if digest ~= local_save.hash then
      return nil, "verify", "上传后校验失败：远端哈希与本地不一致"
    end
    remote_hash = digest
  end

  local saved, state_error = save_state(
    platform,
    game_name,
    key,
    local_save.hash,
    remote_hash,
    local_save.path,
    remote_path,
    remote_name
  )
  if not saved then
    return nil, "fs", "已上传但同步状态写入失败：" .. tostring(state_error)
  end
  return true, remote_path
end

-- ---------------------------------------------------------------------------
-- Playtime
-- ---------------------------------------------------------------------------

local MAX_PLAYTIME = 2147483647

local function format_duration(seconds)
  local value = math.floor(tonumber(seconds) or 0)
  if value < 60 then
    return "不到 1 分钟"
  end
  local minutes = math.floor(value / 60)
  local hours = math.floor(minutes / 60)
  minutes = minutes % 60
  if hours > 0 then
    return tostring(hours) .. " 小时 " .. tostring(minutes) .. " 分钟"
  end
  return tostring(minutes) .. " 分钟"
end

local function playtime_state_path(platform, key)
  return ctx.mod.dir .. "/state/playtime/" .. platform .. "/" .. key .. ".json"
end

local function load_playtime_state(platform, key)
  local data, read_error = read_file(playtime_state_path(platform, key))
  if read_error ~= nil then
    emit_log("warn", "读取游玩时间状态失败，按首次同步处理：" .. read_error)
    return nil
  end
  if data == nil then
    return nil
  end
  local decoded = try_decode_json(data)
  if type(decoded) ~= "table" then
    emit_log("warn", "游玩时间状态损坏，按首次同步处理")
    return nil
  end
  local local_total = tonumber(decoded.local_total)
  local remote_total = tonumber(decoded.remote_total)
  local merged = tonumber(decoded.merged)
  if local_total == nil or remote_total == nil or merged == nil then
    return nil
  end
  return {
    local_total = math.max(0, local_total),
    remote_total = math.max(0, remote_total),
    merged = math.max(0, merged),
  }
end

local function save_playtime_state(platform, key, local_total, remote_total, merged)
  local encoded, encode_error = encode_json({
    version = 1,
    local_total = local_total,
    remote_total = remote_total,
    merged = merged,
  })
  if encoded == nil then
    return nil, "生成游玩时间状态失败：" .. tostring(encode_error)
  end
  local ok, write_error = write_file(playtime_state_path(platform, key), encoded)
  if not ok then
    return nil, write_error
  end
  return true, nil
end

local function sync_playtime(remote_game, platform, key, local_name)
  if type(ctx.game) ~= "table" then
    return nil, "缺少 ctx.game"
  end
  local state_dir = trim_string(ctx.game.state_dir)
  if state_dir == "" then
    return nil, "缺少 ctx.game.state_dir，无法定位 playtime.json"
  end
  local path = state_dir .. "/playtime.json"
  local data, read_error = read_file(path)
  if read_error ~= nil then
    return nil, "读取本地 playtime.json 失败：" .. read_error
  end
  local local_data = nil
  if data ~= nil then
    local_data = try_decode_json(data)
    if type(local_data) ~= "table" then
      emit_log("warn", "本地 playtime.json 无法解析，将由本次同步重建")
      local_data = nil
    end
  end

  local local_seconds = 0
  local local_total = 0
  if type(local_data) == "table" then
    local_seconds = tonumber(local_data.seconds) or 0
    local_total = math.max(
      tonumber(local_data.total_seconds) or 0,
      tonumber(local_data.seconds) or 0
    )
  end
  local remote_total = tonumber(remote_game.playTime) or 0
  local_seconds = math.max(0, math.floor(local_seconds))
  local_total = math.max(0, math.floor(local_total))
  remote_total = math.max(0, math.floor(remote_total))

  local baseline = load_playtime_state(platform, key)
  local merged
  if baseline ~= nil then
    local local_delta = math.max(0, local_total - baseline.local_total)
    local remote_delta = math.max(0, remote_total - baseline.remote_total)
    merged = baseline.merged + local_delta + remote_delta
  elseif local_total > 0 and local_total == remote_total then
    -- Both sides already match: assume a prior run converged instead of
    -- adding the same counter twice.
    merged = local_total
  else
    -- First merge: both counters existed independently, so sum them.
    merged = local_total + remote_total
  end
  merged = math.max(merged, local_total, remote_total)
  if merged > MAX_PLAYTIME then
    merged = MAX_PLAYTIME
  end
  if merged <= 0 then
    return nil, nil
  end

  -- Persist the baseline first.  A crash/failure after this point is retried
  -- without counting the same seconds a second time.
  local saved, save_error = save_playtime_state(platform, key, merged, merged, merged)
  if not saved then
    return nil, "保存游玩时间状态失败：" .. tostring(save_error)
  end

  if local_total ~= merged then
    local payload = {
      schema = 1,
      seconds = math.max(local_seconds, merged),
      total_seconds = merged,
      devices = type(local_data) == "table" and type(local_data.devices) == "table"
          and local_data.devices or {},
    }
    local encoded, encode_error = encode_json(payload)
    if encoded == nil then
      return nil, "生成本地 playtime.json 失败：" .. tostring(encode_error)
    end
    local written, write_error = write_file(path, encoded)
    if not written then
      return nil, "写入本地 playtime.json 失败：" .. tostring(write_error)
    end
  end

  if remote_total ~= merged then
    local body, body_error = encode_json({ playTime = merged })
    if body == nil then
      return nil, "生成 GBAStation 游玩时间失败：" .. tostring(body_error)
    end
    local response, put_kind, put_message = do_put(
      { "api", "game", remote_game.id },
      body,
      "application/json"
    )
    if response == nil then
      return nil, tostring(put_message)
    end
    if not is_success(response) then
      return nil, "更新 GBAStation 游玩时间失败（" .. describe_response(response) .. "）"
    end
  end

  return "游玩时间已同步：" .. format_duration(merged) .. "（" .. tostring(merged) .. " 秒）", nil
end

-- ---------------------------------------------------------------------------
-- Main flow
-- ---------------------------------------------------------------------------

local function run()
  if type(ctx) ~= "table" then
    return error_report("config", "缺少 linkSlot mod 上下文")
  end
  if ctx.timing ~= "manual" then
    return noop_report("本动作只在手动同步（长按 Y）时运行")
  end
  if ctx.unit ~= "sav" then
    return noop_report("本动作只处理电池存档（sav），已跳过")
  end
  if type(ctx.game) ~= "table" then
    return error_report("config", "缺少 ctx.game，无法识别当前游戏")
  end
  if type(ctx.mod) ~= "table" or trim_string(ctx.mod.dir) == "" then
    return error_report("config", "缺少 ctx.mod.dir，无法保存同步状态")
  end

  local config, config_error = load_config()
  if config == nil then
    return error_report("config", config_error or "config.json 无效")
  end
  if config.base_url == "" then
    return error_report("config", "尚未配置 GBAStation Web 地址，请打开 mod 管理页填写 base_url")
  end

  local platform = string.upper(trim_string(ctx.game.platform))
  if not ALLOWED_PLATFORMS[platform] then
    return noop_report("平台 " .. (platform ~= "" and platform or "未知") .. " 不在支持范围（仅 GBA / GB / GBC）")
  end
  if not platform_allowed(config, platform) then
    return noop_report("平台 " .. platform .. " 未在 config.json 的 platforms 中启用")
  end

  local local_name = trim_string(ctx.game.name)
  if local_name == "" then
    return error_report("config", "缺少本地游戏名，无法匹配 GBAStation 游戏")
  end

  local key_digest, key_error = sha256(platform .. "\n" .. local_name)
  if key_digest == nil then
    return error_report("fs", "计算同步键失败：" .. tostring(key_error))
  end
  local key = key_digest:sub(1, 16)

  local local_save, local_read_error, empty_local_path = read_local_save()
  if local_read_error ~= nil then
    return error_report("fs", local_read_error)
  end

  local games_response, games_kind, games_message = do_get({ "api", "games" })
  if games_response == nil then
    return error_report(games_kind, games_message)
  end
  if not is_success(games_response) then
    return error_report("server", "读取 GBAStation 游戏列表失败（" .. describe_response(games_response) .. "）")
  end
  local games_data, games_data_kind, games_data_message = read_json_response(games_response, "读取 GBAStation 游戏列表")
  if games_data == nil then
    return error_report(games_data_kind, games_data_message)
  end
  if type(games_data.games) ~= "table" then
    return error_report("server", "GBAStation 游戏列表缺少 games 数组")
  end

  local remote_game, match_error = select_game(config, games_data.games, platform, local_name)
  if remote_game == nil then
    return error_report("config", match_error)
  end
  if remote_game.id == nil then
    return error_report("server", "GBAStation 游戏条目缺少 id，无法定位存档")
  end
  -- Game ids may be CRC numbers or full paths; always pass one string segment.
  remote_game.id = tostring(remote_game.id)
  playtime_game = remote_game
  playtime_platform = platform
  playtime_key = key
  playtime_name = local_name

  local remote_entry, remote_absent, remote_kind, remote_message = find_remote_battery(remote_game)
  if remote_kind ~= nil then
    return error_report(remote_kind, remote_message)
  end
  if remote_absent then
    remote_entry = nil
  end

  local remote_data = nil
  local remote_hash = nil
  if remote_entry ~= nil then
    local grabbed, grab_kind, grab_message, vanished = fetch_remote_data(remote_game, remote_entry)
    if grab_kind ~= nil then
      return error_report(grab_kind, grab_message)
    end
    if vanished then
      remote_entry = nil
    else
      remote_data = grabbed
      local digest, digest_error = sha256(remote_data)
      if digest == nil then
        return error_report("fs", "计算远端存档哈希失败：" .. tostring(digest_error))
      end
      remote_hash = digest
    end
  end

  if local_save == nil and remote_entry == nil then
    if empty_local_path ~= nil then
      return noop_report("本地存档为空且 GBAStation 没有「" .. local_name .. "」的电池存档，请先在游戏中保存")
    end
    return noop_report("本地与 GBAStation 都没有「" .. local_name .. "」的电池存档")
  end

  local state = load_state(platform, key)
  local direction = nil
  local reason = nil

  if remote_entry == nil then
    if config.mode == "pull" then
      return noop_report(
        "GBAStation 没有「" .. local_name
          .. "」的电池存档；当前 mode=pull 不会上传本地存档，请改为 two-way 或 push 后重试"
      )
    end
    direction = "upload"
    reason = "GBAStation 无存档，新建远端电池存档"
  elseif local_save == nil then
    if config.mode == "push" then
      return noop_report(
        "本地没有「" .. local_name
          .. "」的电池存档；当前 mode=push 不会下载，请改为 two-way 或 pull 后重试"
      )
    end
    direction = "download"
    reason = "本地无存档，从 GBAStation 恢复"
  elseif local_save.hash == remote_hash then
    local remembered, remember_error =
      remember_equal_state(platform, key, local_name, local_save, remote_entry, remote_hash)
    if not remembered then
      return noop_report(
        "「" .. local_name .. "」本地与 GBAStation 的存档一致，无需同步；同步状态写入失败："
          .. tostring(remember_error)
      )
    end
    return noop_report("「" .. local_name .. "」本地与 GBAStation 的电池存档一致，无需同步")
  elseif state == nil then
    if config.first_sync == "remote" then
      direction = "download"
      reason = "首次同步（first_sync=remote）"
    elseif config.first_sync == "local" then
      direction = "upload"
      reason = "首次同步（first_sync=local）"
    else
      local choice, choose_error = choose_side(
        "first_sync",
        "首次同步",
        "「" .. local_name .. "」本地和 GBAStation 都有存档且内容不同。"
      )
      if not choice then
        choice_requested = true
        return noop_report(choose_error)
      end
      if choice == "remote" then
        direction = "download"
        reason = "首次同步：用户选择 GBAStation 覆盖本地"
      else
        direction = "upload"
        reason = "首次同步：用户选择本地覆盖 GBAStation"
      end
    end
  else
    local local_changed = local_save.hash ~= state.local_hash
    local remote_changed = remote_hash ~= state.remote_hash
    if local_changed and not remote_changed then
      direction = "upload"
      reason = "仅本地有变化"
    elseif remote_changed and not local_changed then
      direction = "download"
      reason = "仅远端有变化"
    elseif config.on_conflict == "remote" then
      direction = "download"
      reason = "冲突策略 remote（远端覆盖本地）"
    elseif config.on_conflict == "local" then
      direction = "upload"
      reason = "冲突策略 local（本地覆盖远端）"
    else
      local choice, choose_error = choose_side(
        "conflict",
        "存档冲突",
        "「" .. local_name .. "」本地和 GBAStation 都修改了电池存档。"
      )
      if not choice then
        choice_requested = true
        return noop_report(choose_error)
      end
      if choice == "remote" then
        direction = "download"
        reason = "冲突：用户选择 GBAStation 覆盖本地"
      else
        direction = "upload"
        reason = "冲突：用户选择本地覆盖 GBAStation"
      end
    end
  end

  if direction == "upload" and config.mode == "pull" then
    return noop_report(
      "「" .. local_name
        .. "」需要上传本地存档，但当前 mode=pull 只下载；请改为 two-way 或 push 后重试"
    )
  end
  if direction == "download" and config.mode == "push" then
    return noop_report(
      "「" .. local_name
        .. "」需要下载远端存档，但当前 mode=push 只上传；请改为 two-way 或 pull 后重试"
    )
  end

  if direction == "download" then
    if remote_entry == nil or remote_data == nil then
      return error_report("server", "远端存档在读取过程中消失，请稍后重试")
    end
    local downloaded, download_kind, download_message = do_download(
      config,
      platform,
      key,
      local_name,
      remote_entry,
      remote_data,
      remote_hash,
      local_save,
      empty_local_path
    )
    if not downloaded then
      return error_report(download_kind, download_message, "download")
    end
    return ok_report(
      "download",
      "已从 GBAStation 下载「" .. local_name .. "」到本机（" .. human_size(#remote_data) .. "，" .. reason .. "）"
    )
  end

  if direction == "upload" then
    if local_save == nil then
      return error_report("fs", "本地存档在读取过程中消失，请重试")
    end
    local uploaded, upload_kind, upload_message = do_upload(
      config,
      platform,
      key,
      local_name,
      remote_game,
      remote_entry,
      remote_data,
      local_save
    )
    if not uploaded then
      return error_report(upload_kind, upload_message, "upload")
    end
    local suffix = config.verify and "，已校验" or ""
    local prefix = remote_entry ~= nil and "已上传并覆盖" or "已上传"
    return ok_report(
      "upload",
      prefix .. "「" .. local_name .. "」到 GBAStation（" .. human_size(local_save.size) .. "，" .. reason .. suffix .. "）"
    )
  end

  return noop_report("没有需要同步的内容")
end

-- ---------------------------------------------------------------------------
-- Entry point: exactly one report per manual run.
-- ---------------------------------------------------------------------------

local executed, result = pcall(run)
if not executed then
  result = error_report("server", "脚本内部错误：" .. one_line(tostring(result), 240))
end
if type(result) ~= "table" then
  result = error_report("server", "脚本没有产生同步结果")
end

if playtime_game ~= nil
  and not choice_requested
  and (result.status == "ok" or result.status == "noop")
then
  local playtime_detail, playtime_error =
    sync_playtime(playtime_game, playtime_platform, playtime_key, playtime_name)
  if playtime_detail ~= nil then
    result.detail = one_line((result.detail or "") .. "；" .. playtime_detail, 480)
  elseif playtime_error ~= nil then
    emit_log("warn", "游玩时间同步失败：" .. tostring(playtime_error))
  end
end

if result.status == "error" then
  emit_log("error", "GBAStation：" .. tostring(result.detail))
elseif result.status == "noop" then
  emit_log("warn", "GBAStation：" .. tostring(result.detail))
else
  emit_log("info", "GBAStation：" .. tostring(result.detail))
end
emit_report(result)
