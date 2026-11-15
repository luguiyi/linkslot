(function () {
  'use strict';

  var api = window.LinkSlotMod;
  var state = {
    info: null,
    config: null,
    configRevision: null,
    actions: [],
    selectedAction: null
  };

  function q(id) {
    return document.getElementById(id);
  }

  function esc(text) {
    return String(text == null ? '' : text)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  function request(method, params) {
    if (!api || !api.request) {
      return Promise.reject(new Error('linkSlot mod bridge 不可用'));
    }
    return api.request(method, params || {});
  }

  function status(id, text, kind) {
    var el = q(id);
    if (!el) return;
    el.textContent = text;
    el.className = 'status' + (kind ? ' ' + kind : '');
  }

  function setActionStatus(text, kind) {
    status('actionStatus', text, kind);
  }

  function setConfigStatus(text, kind) {
    status('configStatus', text, kind);
  }

  function defaultConfig() {
    return {
      schema: 1,
      kind: 'linkSlot.savedeck',
      profile: '',
      mode: 'two-way',
      match_by_name: true,
      on_conflict: 'ask',
      allow_create: false,
      batch: 8,
      platforms: ['GBA', 'GB', 'GBC'],
      platform_map: { 'GBA hack': 'GBA' },
      aliases: {}
    };
  }

  function loadContext() {
    return Promise.all([
      request('context.storages'),
      request('context.saves').catch(function () {
        return null;
      })
    ])
      .then(function (values) {
        var context = values[0] || {};
        renderProfiles((context.storages || []).filter(function (item) {
          return item && item.id && (!item.kind || item.kind === 'webdav');
        }));
        state.localSaves = values[1] || null;
      })
      .catch(function (error) {
        setConfigStatus('读取 linkSlot 上下文失败：' + error.message, 'error');
      });
  }

  function renderProfiles(profiles) {
    var select = q('configProfile');
    var current = (state.config && state.config.profile) || select.value || '';
    select.innerHTML = '';
    if (!profiles.length) {
      var empty = document.createElement('option');
      empty.value = '';
      empty.textContent = '没有可用的 WebDAV 同步方式';
      select.appendChild(empty);
      return;
    }
    profiles.forEach(function (profile) {
      var option = document.createElement('option');
      option.value = profile.id;
      option.textContent = profile.name
        ? profile.name + '（' + profile.id + '）'
        : profile.id;
      select.appendChild(option);
    });
    var found = false;
    for (var index = 0; index < select.options.length; index += 1) {
      if (select.options[index].value === current) found = true;
    }
    select.value = found ? current : profiles[0].id;
  }

  function loadConfig() {
    setConfigStatus('正在读取 config.json…');
    return request('files.list')
      .then(function (data) {
        var files = (data && data.files) || [];
        var configFile = null;
        files.forEach(function (file) {
          if (!file.is_dir && file.path === 'config.json') configFile = file;
        });
        if (!configFile) {
          state.config = defaultConfig();
          state.configRevision = null;
          renderConfig();
          setConfigStatus('还没有 config.json，保存时会新建。');
          return;
        }
        return request('files.read', { path: 'config.json' }).then(function (file) {
          state.configRevision = file.revision;
          try {
            state.config = JSON.parse(file.content);
            var defaults = defaultConfig();
            if (!Array.isArray(state.config.platforms)) {
              state.config.platforms = defaults.platforms;
            }
            if (!state.config.platform_map || typeof state.config.platform_map !== 'object') {
              state.config.platform_map = defaults.platform_map;
            }
          } catch (error) {
            state.config = defaultConfig();
            setConfigStatus('config.json 不是有效 JSON，当前显示默认值；修复后请重新读取。', 'error');
            renderConfig();
            return;
          }
          renderConfig();
          if (!q('configProfile').value) {
            setConfigStatus('没有可用的 WebDAV 同步方式，请先在「云同步」页添加。', 'error');
          } else if (state.config.profile && !profileExists(state.config.profile)) {
            setConfigStatus(
              'config.json 的同步方式「' +
                state.config.profile +
                '」不在当前列表，已选中第一个可用方式；请保存配置。',
              'error'
            );
          } else {
            setConfigStatus('已读取 config.json。');
          }
        });
      })
      .catch(function (error) {
        setConfigStatus('读取 config.json 失败：' + error.message, 'error');
      });
  }

  function renderConfig() {
    var config = state.config || defaultConfig();
    var profileSelect = q('configProfile');
    if (config.profile) {
      for (var i = 0; i < profileSelect.options.length; i += 1) {
        if (profileSelect.options[i].value === config.profile) {
          profileSelect.value = config.profile;
          break;
        }
      }
    }
    q('configMode').value = config.mode || 'two-way';
    q('configConflict').value = config.on_conflict || 'ask';
    q('configMatchName').checked = config.match_by_name !== false;
    q('configCreate').checked = config.allow_create === true;
    renderPlatformMap(config.platform_map || {});
    renderAliases(config.aliases || {});
  }

  function profileExists(profileId) {
    if (!profileId) return false;
    var select = q('configProfile');
    for (var index = 0; index < select.options.length; index += 1) {
      if (select.options[index].value === profileId) return true;
    }
    return false;
  }

  function aliasRow(localKey, remoteKey) {
    var row = document.createElement('div');
    row.className = 'alias-row';
    var local = document.createElement('input');
    local.type = 'text';
    local.placeholder = 'GBA/龙珠大冒险';
    local.value = localKey || '';
    local.setAttribute('data-alias-local', '1');
    var arrow = document.createElement('span');
    arrow.textContent = '→';
    arrow.className = 'muted';
    var remote = document.createElement('input');
    remote.type = 'text';
    remote.placeholder = 'Dragon Ball - Advanced Adventure';
    remote.value = remoteKey || '';
    remote.setAttribute('data-alias-remote', '1');
    var remove = document.createElement('button');
    remove.type = 'button';
    remove.className = 'btn ghost';
    remove.textContent = '删除';
    remove.addEventListener('click', function () {
      row.remove();
    });
    row.appendChild(local);
    row.appendChild(arrow);
    row.appendChild(remote);
    row.appendChild(remove);
    return row;
  }

  function renderAliases(aliases) {
    var list = q('aliasList');
    list.innerHTML = '';
    var keys = Object.keys(aliases || {}).sort();
    keys.forEach(function (key) {
      list.appendChild(aliasRow(key, aliases[key]));
    });
  }

  function collectAliases() {
    var aliases = {};
    var rows = q('aliasList').querySelectorAll('.alias-row');
    for (var i = 0; i < rows.length; i += 1) {
      var local = rows[i].querySelector('[data-alias-local]').value.trim();
      var remote = rows[i].querySelector('[data-alias-remote]').value.trim();
      if (!local && !remote) continue;
      if (!local || !remote) {
        throw new Error('映射行的本地键和 SaveDeck 键都必须填写');
      }
      if (!local || local === '/' || local.indexOf('/') < 1 || local.indexOf('..') >= 0) {
        throw new Error('本地键格式必须是 平台/本地存档名，例如 GBA/龙珠大冒险');
      }
      if (aliases[local]) {
        throw new Error('映射键重复：' + local);
      }
      aliases[local] = remote;
    }
    return aliases;
  }

  function platformMapRow(remoteName, localName) {
    var row = document.createElement('div');
    row.className = 'alias-row platform-map-row';
    var remote = document.createElement('input');
    remote.type = 'text';
    remote.placeholder = 'GBA hack';
    remote.value = remoteName || '';
    remote.setAttribute('data-platform-remote', '1');
    var arrow = document.createElement('span');
    arrow.textContent = '→';
    arrow.className = 'muted';
    var local = document.createElement('input');
    local.type = 'text';
    local.placeholder = 'GBA';
    local.value = localName || '';
    local.setAttribute('data-platform-local', '1');
    var remove = document.createElement('button');
    remove.type = 'button';
    remove.className = 'btn ghost';
    remove.textContent = '删除';
    remove.addEventListener('click', function () {
      row.remove();
    });
    row.appendChild(remote);
    row.appendChild(arrow);
    row.appendChild(local);
    row.appendChild(remove);
    return row;
  }

  function renderPlatformMap(platformMap) {
    var list = q('platformMapList');
    list.innerHTML = '';
    var keys = Object.keys(platformMap || {}).sort();
    keys.forEach(function (remoteName) {
      list.appendChild(platformMapRow(remoteName, platformMap[remoteName]));
    });
  }

  function collectPlatformMap() {
    var platformMap = {};
    var rows = q('platformMapList').querySelectorAll('.platform-map-row');
    for (var index = 0; index < rows.length; index += 1) {
      var remote = rows[index].querySelector('[data-platform-remote]').value.trim();
      var local = rows[index].querySelector('[data-platform-local]').value.trim();
      if (!remote && !local) continue;
      if (!remote || !local) {
        throw new Error('平台映射两边的平台名都必须填写');
      }
      if (
        remote === '.' ||
        remote === '..' ||
        local === '.' ||
        local === '..' ||
        /[\/\\\u0000-\u001f]/.test(remote) ||
        /[\/\\\u0000-\u001f]/.test(local)
      ) {
        throw new Error('平台名不能包含路径分隔符或控制字符');
      }
      if (platformMap[remote]) {
        throw new Error('平台映射重复：' + remote);
      }
      platformMap[remote] = local;
    }
    return platformMap;
  }

  function collectConfig() {
    var aliases = collectAliases();
    var platformMap = collectPlatformMap();
    var batch = Number(state.config && state.config.batch);
    if (!isFinite(batch) || Math.floor(batch) !== batch || batch < 1) batch = 8;
    return {
      schema: 1,
      kind: 'linkSlot.savedeck',
      profile: q('configProfile').value,
      mode: q('configMode').value,
      match_by_name: q('configMatchName').checked,
      on_conflict: q('configConflict').value,
      allow_create: q('configCreate').checked,
      batch: batch,
      platforms: (state.config && state.config.platforms) || ['GBA', 'GB', 'GBC'],
      platform_map: platformMap,
      aliases: aliases
    };
  }

  function saveConfig(event) {
    event.preventDefault();
    var config;
    try {
      config = collectConfig();
    } catch (error) {
      setConfigStatus(error.message, 'error');
      return;
    }
    if (!config.profile) {
      setConfigStatus('请选择 WebDAV 同步方式。', 'error');
      return;
    }
    var content = JSON.stringify(config, null, 2) + '\n';
    setConfigStatus('正在保存…');
    var payload = { path: 'config.json', content: content };
    if (state.configRevision) payload.revision = state.configRevision;
    request('files.write', payload)
      .then(function (file) {
        state.config = config;
        state.configRevision = file.revision;
        renderConfig();
        setConfigStatus('已保存；下一次 linkSlot sync 生效。', 'ok');
      })
      .catch(function (error) {
        setConfigStatus('保存失败：' + error.message, 'error');
      });
  }

  function actionTimingLabel(timing) {
    var labels = {
      before_upload: '上传前',
      after_upload: '上传后',
      before_download: '下载前',
      after_download: '下载后',
      after_sync: '同步完成后',
      manual: '手动（长按 Y）'
    };
    return labels[timing] || timing || '无效';
  }

  function loadActions() {
    return request('actions.list')
      .then(function (data) {
        state.actions = (data && data.actions) || [];
        renderActions();
      })
      .catch(function (error) {
        q('actionList').innerHTML =
          '<div class="empty">读取动作失败：' + esc(error.message) + '</div>';
      });
  }

  function renderActions() {
    var list = q('actionList');
    if (!state.actions.length) {
      list.innerHTML = '<div class="empty">还没有动作。请新建一个 manual 动作。</div>';
      return;
    }
    list.innerHTML = '';
    state.actions.forEach(function (action) {
      var row = document.createElement('div');
      row.className = 'list-row';
      var grow = document.createElement('div');
      grow.className = 'grow';
      var title = document.createElement('strong');
      title.textContent = action.name || action.id;
      var sub = document.createElement('div');
      sub.className = 'muted';
      sub.textContent = actionTimingLabel(action.timing) + ' · 顺序 ' + action.order;
      grow.appendChild(title);
      grow.appendChild(sub);
      var pill = document.createElement('span');
      pill.className = 'pill' + (action.enabled ? ' on' : '');
      pill.textContent = action.enabled ? '启用' : '停用';
      var edit = document.createElement('button');
      edit.type = 'button';
      edit.className = 'btn';
      edit.textContent = '编辑';
      edit.addEventListener('click', function () {
        editAction(action.id);
      });
      var toggle = document.createElement('button');
      toggle.type = 'button';
      toggle.className = 'btn ghost';
      toggle.textContent = action.enabled ? '停用' : '启用';
      toggle.addEventListener('click', function () {
        request('actions.toggle', {
          action: action.id,
          enabled: !action.enabled
        })
          .then(loadActions)
          .catch(function (error) {
            setActionStatus('修改动作状态失败：' + error.message, 'error');
          });
      });
      var remove = document.createElement('button');
      remove.type = 'button';
      remove.className = 'btn danger';
      remove.textContent = '删除';
      remove.addEventListener('click', function () {
        if (!window.confirm('删除动作「' + (action.name || action.id) + '」？')) {
          return;
        }
        request('actions.delete', { action: action.id })
          .then(function () {
            setActionStatus('动作已删除。', 'ok');
            return loadActions();
          })
          .catch(function (error) {
            setActionStatus('删除动作失败：' + error.message, 'error');
          });
      });
      row.appendChild(grow);
      row.appendChild(pill);
      row.appendChild(edit);
      row.appendChild(toggle);
      row.appendChild(remove);
      list.appendChild(row);
    });
  }

  function showActionEditor(show) {
    q('actionForm').hidden = !show;
    if (show) {
      q('actionForm').scrollIntoView({ block: 'nearest' });
    }
  }

  function newAction() {
    state.selectedAction = null;
    q('actionId').value = '';
    q('actionRevision').value = '';
    q('actionName').value = '';
    q('actionOrder').value = '100';
    q('actionTiming').value = 'manual';
    q('actionEnabled').checked = true;
    q('actionContent').value = '-- linkSlot mod 动作\nreturn\n';
    setActionStatus('新动作；保存后才会生效。');
    showActionEditor(true);
    q('actionName').focus();
  }

  function editAction(id) {
    request('actions.get', { action: id })
      .then(function (detail) {
        state.selectedAction = detail;
        q('actionId').value = detail.id || '';
        q('actionRevision').value = detail.revision || '';
        q('actionName').value = detail.name || '';
        q('actionOrder').value = String(detail.order == null ? 100 : detail.order);
        q('actionTiming').value = detail.timing || 'manual';
        q('actionEnabled').checked = !!detail.enabled;
        q('actionContent').value = detail.content || '';
        setActionStatus('已读取动作。');
        showActionEditor(true);
      })
      .catch(function (error) {
        setActionStatus('读取动作失败：' + error.message, 'error');
      });
  }

  function captureAction() {
    var name = q('actionName').value.trim();
    var order = Number(q('actionOrder').value);
    if (!name) throw new Error('动作名称不能为空');
    if (!isFinite(order) || Math.floor(order) !== order) {
      throw new Error('执行顺序必须是整数');
    }
    return {
      action_id: q('actionId').value || undefined,
      name: name,
      order: order,
      timing: q('actionTiming').value,
      enabled: q('actionEnabled').checked,
      content: q('actionContent').value,
      revision: q('actionRevision').value || undefined
    };
  }

  function saveAction(event) {
    event.preventDefault();
    var payload;
    try {
      payload = captureAction();
    } catch (error) {
      setActionStatus(error.message, 'error');
      return;
    }
    setActionStatus('正在保存…');
    request('actions.save', payload)
      .then(function (detail) {
        state.selectedAction = detail;
        q('actionId').value = detail.id || '';
        q('actionRevision').value = detail.revision || '';
        setActionStatus('动作已保存。', 'ok');
        return loadActions();
      })
      .catch(function (error) {
        setActionStatus('保存失败：' + error.message, 'error');
      });
  }

  function checkAction() {
    var payload;
    try {
      payload = captureAction();
    } catch (error) {
      setActionStatus(error.message, 'error');
      return;
    }
    setActionStatus('正在保存并检查…');
    request('actions.save', payload)
      .then(function (detail) {
        state.selectedAction = detail;
        q('actionId').value = detail.id || '';
        q('actionRevision').value = detail.revision || '';
        return request('actions.check', { action: detail.id || undefined });
      })
      .then(function (result) {
        var text = [];
        if (result && result.detail) text.push(result.detail);
        if (result && result.output) text.push(result.output);
        if (result && result.ok) {
          setActionStatus('语法检查通过。' + (text.length ? '\n' + text.join('\n') : ''), 'ok');
        } else {
          setActionStatus('语法检查失败。' + (text.length ? '\n' + text.join('\n') : ''), 'error');
        }
        return loadActions();
      })
      .catch(function (error) {
        setActionStatus('语法检查失败：' + error.message, 'error');
      });
  }

  function bind() {
    q('configForm').addEventListener('submit', saveConfig);
    q('saveConfig').addEventListener('click', saveConfig);
    q('reloadConfig').addEventListener('click', loadConfig);
    q('addAlias').addEventListener('click', function () {
      q('aliasList').appendChild(aliasRow('', ''));
    });
    q('addPlatformMap').addEventListener('click', function () {
      q('platformMapList').appendChild(platformMapRow('', ''));
    });
    q('newAction').addEventListener('click', newAction);
    q('actionForm').addEventListener('submit', saveAction);
    q('saveAction').addEventListener('click', saveAction);
    q('checkAction').addEventListener('click', checkAction);
    q('cancelAction').addEventListener('click', function () {
      showActionEditor(false);
    });
  }

  function start() {
    bind();
    api.ready.then(function (info) {
      state.info = info || {};
      q('modInfo').textContent =
        (state.info.name || state.info.id || 'SaveDeck 电池存档桥') +
        ' · v' +
        (state.info.version || '1.2.0');
      return loadContext().then(function () {
        return Promise.all([loadConfig(), loadActions()]);
      });
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', start);
  } else {
    start();
  }
})();
