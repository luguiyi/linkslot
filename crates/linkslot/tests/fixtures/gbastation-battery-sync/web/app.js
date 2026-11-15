(function () {
  'use strict';

  var api = window.LinkSlotMod;
  var DEFAULT_PLATFORMS = ['GBA', 'GB', 'GBC'];
  var ALLOWED_MODES = ['two-way', 'pull', 'push'];
  var ALLOWED_FIRST_SYNC = ['remote', 'local', 'ask'];
  var ALLOWED_CONFLICT = ['ask', 'remote', 'local'];
  var state = {
    config: null,
    revision: null,
    dirty: false,
    saving: false
  };

  function q(id) {
    return document.getElementById(id);
  }

  function defaultConfig() {
    return {
      schema: 1,
      kind: 'linkSlot.gbastation',
      base_url: '',
      mode: 'two-way',
      first_sync: 'remote',
      on_conflict: 'ask',
      backup: true,
      verify: true,
      platforms: DEFAULT_PLATFORMS.slice(),
      games: {}
    };
  }

  function normalizeLoadedConfig(raw) {
    var config = {};
    if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
      Object.keys(raw).forEach(function (key) {
        config[key] = raw[key];
      });
    }
    var base = defaultConfig();
    config.schema = 1;
    config.kind = 'linkSlot.gbastation';
    config.base_url = typeof raw.base_url === 'string' ? raw.base_url.trim() : '';
    config.mode = ALLOWED_MODES.indexOf(raw.mode) >= 0 ? raw.mode : base.mode;
    config.first_sync = ALLOWED_FIRST_SYNC.indexOf(raw.first_sync) >= 0 ? raw.first_sync : base.first_sync;
    config.on_conflict = ALLOWED_CONFLICT.indexOf(raw.on_conflict) >= 0 ? raw.on_conflict : base.on_conflict;
    config.backup = raw.backup !== false;
    config.verify = raw.verify !== false;
    if (Array.isArray(raw.platforms)) {
      var platforms = [];
      raw.platforms.forEach(function (item) {
        var platform = typeof item === 'string' ? item.trim().toUpperCase() : '';
        if (DEFAULT_PLATFORMS.indexOf(platform) >= 0 && platforms.indexOf(platform) < 0) {
          platforms.push(platform);
        }
      });
      config.platforms = platforms.length ? platforms : base.platforms;
    } else {
      config.platforms = base.platforms;
    }
    if (raw.games && typeof raw.games === 'object' && !Array.isArray(raw.games)) {
      config.games = raw.games;
    } else {
      config.games = {};
    }
    return config;
  }

  function request(method, params) {
    if (!api || typeof api.request !== 'function') {
      return Promise.reject(new Error('linkSlot mod bridge 不可用'));
    }
    return api.request(method, params || {});
  }

  function setConfigStatus(text, kind) {
    var element = q('configStatus');
    if (!element) return;
    element.textContent = text;
    element.className = 'status' + (kind ? ' ' + kind : '');
  }

  function setActionStatus(text, kind) {
    var element = q('actionStatus');
    if (!element) return;
    element.textContent = text;
    element.className = 'hint' + (kind ? ' ' + kind : '');
  }

  function setDirty(dirty) {
    state.dirty = dirty;
    var pill = q('dirty');
    if (!pill) return;
    pill.textContent = dirty ? '有未保存修改' : '未修改';
    pill.className = 'pill' + (dirty ? ' on' : '');
  }

  function markDirty() {
    if (!state.saving) setDirty(true);
  }

  function parseBaseUrl(value) {
    var raw = String(value == null ? '' : value).trim();
    if (!raw) {
      throw new Error('请填写 GBAStation Web 地址，例如 http://192.168.1.23:8080');
    }
    var parsed;
    if (typeof URL === 'function') {
      try {
        parsed = new URL(raw);
      } catch (error) {
        throw new Error('地址格式不正确，请填写 http://IP:端口');
      }
    } else {
      var match = raw.match(/^(https?):\/\/([^\/\s]+)(\/.*)?$/i);
      if (!match) {
        throw new Error('地址格式不正确，请填写 http://IP:端口');
      }
      parsed = {
        protocol: match[1].toLowerCase() + ':',
        hostname: match[2],
        pathname: match[3] || '/',
        search: '',
        hash: '',
        origin: match[1].toLowerCase() + '://' + match[2],
        username: '',
        password: ''
      };
    }
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
      throw new Error('地址必须以 http:// 或 https:// 开头');
    }
    if (!parsed.hostname) {
      throw new Error('地址缺少主机名或 IP');
    }
    if (parsed.username || parsed.password) {
      throw new Error('地址不能带用户名或密码；GBAStation Web 服务不需要登录');
    }
    var hadPath = (parsed.pathname && parsed.pathname !== '/') || !!parsed.search || !!parsed.hash;
    return {
      origin: parsed.origin,
      hadPath: hadPath,
      insecure: parsed.protocol === 'http:'
    };
  }

  function refreshUrlHint() {
    var hint = q('urlHint');
    if (!hint) return;
    var raw = q('baseUrl').value.trim();
    if (!raw) {
      hint.textContent = '例如 http://192.168.1.23:8080；只填协议、IP 和端口。';
      hint.className = 'hint';
      return;
    }
    try {
      var parsed = parseBaseUrl(raw);
      var text = '将使用 ' + parsed.origin + '。';
      if (parsed.hadPath) {
        text += '地址里的路径不会被保存，将只保存来源。';
      }
      if (parsed.insecure) {
        text += ' HTTP 仅建议在可信局域网使用。';
      }
      hint.textContent = text;
      hint.className = 'hint';
    } catch (error) {
      hint.textContent = error.message;
      hint.className = 'hint error';
    }
  }

  function addGameRow(key, value) {
    var editor = q('gamesEditor');
    if (!editor) return;
    var row = document.createElement('div');
    row.className = 'game-row';

    var keyInput = document.createElement('input');
    keyInput.type = 'text';
    keyInput.className = 'game-key';
    keyInput.placeholder = 'linkSlot 游戏名，例如：龙珠大冒险';
    keyInput.value = key == null ? '' : String(key);
    keyInput.addEventListener('input', markDirty);

    var arrow = document.createElement('span');
    arrow.className = 'game-arrow';
    arrow.textContent = '→';

    var valueInput = document.createElement('input');
    valueInput.type = 'text';
    valueInput.className = 'game-value';
    valueInput.placeholder = 'GBAStation id / 标题 / ROM 文件名';
    valueInput.value = value == null ? '' : String(value);
    valueInput.addEventListener('input', markDirty);

    var remove = document.createElement('button');
    remove.type = 'button';
    remove.className = 'btn ghost game-remove';
    remove.textContent = '删除';
    remove.addEventListener('click', function () {
      row.parentNode.removeChild(row);
      if (!editor.querySelector('.game-row')) addGameRow('', '');
      markDirty();
    });

    row.appendChild(keyInput);
    row.appendChild(arrow);
    row.appendChild(valueInput);
    row.appendChild(remove);
    editor.appendChild(row);
  }

  function renderGames(games) {
    var editor = q('gamesEditor');
    if (!editor) return;
    editor.innerHTML = '';
    var entries = games && typeof games === 'object' && !Array.isArray(games) ? games : {};
    Object.keys(entries).forEach(function (key) {
      addGameRow(key, entries[key]);
    });
    if (!Object.keys(entries).length) addGameRow('', '');
  }

  function collectGames() {
    var editor = q('gamesEditor');
    var result = {};
    var count = 0;
    var rows = editor ? editor.querySelectorAll('.game-row') : [];
    for (var index = 0; index < rows.length; index += 1) {
      var fields = rows[index].querySelectorAll('input');
      var name = fields[0] ? fields[0].value.trim() : '';
      var target = fields[1] ? fields[1].value.trim() : '';
      if (!name && !target) continue;
      if (!name) {
        throw new Error('第 ' + (index + 1) + ' 行缺少 linkSlot 游戏名');
      }
      if (!target) {
        throw new Error('游戏「' + name + '」缺少 GBAStation 的 id / 标题 / ROM 文件名');
      }
      count += 1;
      if (count > 256) {
        throw new Error('游戏映射最多 256 项');
      }
      if (Object.prototype.hasOwnProperty.call(result, name)) {
        throw new Error('linkSlot 游戏名「' + name + '」重复，请合并成一行');
      }
      result[name] = target;
    }
    return result;
  }

  function collectConfig(parsedUrl) {
    var previous = state.config || defaultConfig();
    var config = {};
    Object.keys(previous).forEach(function (key) {
      config[key] = previous[key];
    });
    config.schema = 1;
    config.kind = 'linkSlot.gbastation';
    config.base_url = parsedUrl.origin;
    config.mode = q('mode').value;
    config.first_sync = q('firstSync').value;
    config.on_conflict = q('onConflict').value;
    config.backup = q('backup').checked;
    config.verify = q('verify').checked;
    config.platforms = Array.isArray(previous.platforms) && previous.platforms.length
      ? previous.platforms.slice()
      : DEFAULT_PLATFORMS.slice();
    config.games = collectGames();
    return config;
  }

  function renderConfig() {
    var config = state.config || defaultConfig();
    q('baseUrl').value = config.base_url || '';
    q('mode').value = ALLOWED_MODES.indexOf(config.mode) >= 0 ? config.mode : 'two-way';
    q('firstSync').value = ALLOWED_FIRST_SYNC.indexOf(config.first_sync) >= 0 ? config.first_sync : 'remote';
    q('onConflict').value = ALLOWED_CONFLICT.indexOf(config.on_conflict) >= 0 ? config.on_conflict : 'ask';
    q('backup').checked = config.backup !== false;
    q('verify').checked = config.verify !== false;
    renderGames(config.games || {});
    refreshUrlHint();
    setDirty(false);
  }

  function looksMissing(error) {
    var message = String((error && error.message) || error || '');
    return /404|not found|不存在|没有找到/i.test(message);
  }

  function loadConfig() {
    setConfigStatus('正在读取 config.json…');
    return request('files.read', { path: 'config.json' })
      .then(function (file) {
        state.revision = file && file.revision ? file.revision : null;
        var content = file && typeof file.content === 'string' ? file.content : '';
        if (!content.trim()) {
          state.config = defaultConfig();
          renderConfig();
          setConfigStatus('config.json 为空，页面显示默认值；保存后会写入配置。', 'warn');
          return;
        }
        var raw;
        try {
          raw = JSON.parse(content);
        } catch (error) {
          state.config = defaultConfig();
          renderConfig();
          setConfigStatus('config.json 不是有效 JSON；页面显示默认值，保存会覆盖原文件。错误：' + error.message, 'error');
          return;
        }
        state.config = normalizeLoadedConfig(raw || {});
        renderConfig();
        setConfigStatus('已读取 config.json；修改后点「保存配置」，下一次长按 Y 生效。', 'ok');
      })
      .catch(function (error) {
        if (looksMissing(error)) {
          state.config = defaultConfig();
          state.revision = null;
          renderConfig();
          setConfigStatus('还没有 config.json；填写地址后保存会创建。', 'warn');
          return;
        }
        setConfigStatus('读取 config.json 失败：' + error.message, 'error');
      });
  }

  function saveConfig() {
    if (state.saving) return;
    var parsedUrl;
    var config;
    try {
      parsedUrl = parseBaseUrl(q('baseUrl').value);
      config = collectConfig(parsedUrl);
    } catch (error) {
      setConfigStatus(error.message, 'error');
      return;
    }
    var content = JSON.stringify(config, null, 2) + '\n';
    state.saving = true;
    var button = q('save');
    if (button) {
      button.disabled = true;
      button.textContent = '保存中…';
    }
    setConfigStatus('正在保存 config.json…');
    var payload = { path: 'config.json', content: content };
    if (state.revision) {
      payload.revision = state.revision;
    }
    request('files.write', payload)
      .then(function (file) {
        state.config = normalizeLoadedConfig(config);
        state.revision = file && file.revision ? file.revision : state.revision;
        renderConfig();
        var message = '已保存；下一次长按 Y 使用新配置。';
        if (parsedUrl.hadPath) {
          message += ' 地址已规范化为来源 ' + parsedUrl.origin + '。';
        }
        setConfigStatus(message, 'ok');
      })
      .catch(function (error) {
        var message = String((error && error.message) || error || '');
        if (/409|冲突|conflict/i.test(message)) {
          setConfigStatus('保存失败：配置已被其他页面修改，请先点「重新读取」再编辑。', 'error');
        } else {
          setConfigStatus('保存失败：' + message, 'error');
        }
      })
      .then(function () {
        state.saving = false;
        if (button) {
          button.disabled = false;
          button.textContent = '保存配置';
        }
      });
  }

  function timingLabel(timing) {
    if (timing === 'manual') return '手动（长按 Y）';
    return timing || '未知时机';
  }

  function loadActionStatus() {
    setActionStatus('正在读取动作状态…');
    return request('actions.list')
      .then(function (data) {
        var actions = (data && data.actions) || [];
        var sync = null;
        var manual = [];
        actions.forEach(function (action) {
          if (!action) return;
          if (action.id === 'sync') sync = action;
          if (action.timing === 'manual') manual.push(action);
        });
        // Older Web imports used to generate an action id (for example
        // a-18e...); the actual manual action still works.  Accept it instead
        // of showing a false "missing sync" warning.
        if (!sync) {
          manual.forEach(function (action) {
            var name = String(action.name || '');
            if (!sync && (name.indexOf('电池') >= 0 || name.indexOf('同步') >= 0)) {
              sync = action;
            }
          });
          if (!sync && manual.length) sync = manual[0];
        }
        if (!sync) {
          setActionStatus('没有找到可执行的 manual 动作；请确认 mod 包完整（actions/sync/action.json）。', 'error');
          return;
        }
        var enabled = sync.enabled !== false;
        var parts = [];
        parts.push('动作「' + (sync.name || sync.id) + '」：' + (sync.timing_label || timingLabel(sync.timing)));
        parts.push('顺序 ' + sync.order);
        parts.push(enabled ? '已启用' : '已停用');
        if (!enabled) parts.push('动作已停用，长按 Y 不会执行本 mod 的同步');
        if (sync.error) parts.push('动作错误：' + sync.error);
        setActionStatus(parts.join(' · '), enabled ? null : 'error');
      })
      .catch(function (error) {
        setActionStatus('读取动作状态失败：' + error.message, 'error');
      });
  }

  function bind() {
    q('save').addEventListener('click', saveConfig);
    q('reload').addEventListener('click', function () {
      loadActionStatus();
      loadConfig();
    });
    q('baseUrl').addEventListener('input', function () {
      refreshUrlHint();
      markDirty();
    });
    ['mode', 'firstSync', 'onConflict'].forEach(function (id) {
      q(id).addEventListener('change', markDirty);
    });
    ['backup', 'verify'].forEach(function (id) {
      q(id).addEventListener('change', markDirty);
    });
    q('addGame').addEventListener('click', function () {
      addGameRow('', '');
      markDirty();
      var rows = q('gamesEditor').querySelectorAll('.game-key');
      if (rows.length) rows[rows.length - 1].focus();
    });
  }
  function start() {
    bind();
    if (!api || typeof api.ready !== 'object' || typeof api.ready.then !== 'function') {
      setConfigStatus('linkSlot mod bridge 不可用：请在 linkSlot Web 的 mod 详情页打开本页面。', 'error');
      setActionStatus('无法读取动作状态：bridge 不可用。', 'error');
      return;
    }
    api.ready
      .then(function (info) {
        var name = (info && info.name) || 'GBAStation 电池存档同步';
        var version = (info && info.version) || '1.0.0';
        q('modInfo').textContent = name + ' · v' + version + ' · 管理页在 linkSlot 内运行，不会直接访问网络';
        return Promise.all([loadConfig(), loadActionStatus()]);
      })
      .catch(function (error) {
        setConfigStatus('连接 linkSlot 失败：' + ((error && error.message) || error), 'error');
      });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', start);
  } else {
    start();
  }
})();
