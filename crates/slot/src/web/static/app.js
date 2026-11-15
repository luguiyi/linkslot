/* ============================================================================
   linkSlot · Slot 掌机内置 Web 管理页
   ----------------------------------------------------------------------------
   原生 JavaScript：无第三方库、无外部字体 / CDN。所有写请求（上传、保存 WebDAV
   设置、执行同步、连接 WiFi）都带 X-Slot-Web: 1，服务端据此拒绝其它来源的误触。
   ========================================================================== */
(function () {
  'use strict';

  var WRITE_HEADERS = { 'X-Slot-Web': '1' };
  var MAX_UPLOAD_BYTES = 32 * 1024 * 1024;
  var MAX_UPLOAD_MB = 32;
  var POLL_MS = 5000;
  var TABS = ['overview', 'roms', 'saves', 'files', 'sync', 'mods', 'network', 'settings', 'about'];
  var ROM_BATCH = 40;
  var SAVE_BATCH = 10;

  var state = {
    modGroupsClosed: {},
    status: null,
    saves: null,
    files: null,
    filesBusy: false,
    sync: null,
    networks: null,
    savedNetworks: [],
    networkStatus: null,
    roms: null,
    romFilter: '',
    romQuery: '',
    romView: null,
    addGameFile: null,
    savesView: null,
    selectedRom: null,
    frontEditor: null,
    coverGame: null,
    coverImage: null,
    coverZoom: 1,
    coverX: 0,
    coverY: 0,
    coverPointer: null,
    coverBusy: false,
    upload: null,
    scanBusy: false,
    formFilled: false,
    profiles: [],
    activeStorage: '',
    selectedStorage: '',
    syncSwitchBusy: false,
    logLevel: 'info',
    mods: null,
    selectedMod: null,
    selectedAction: null,
    modUiMod: null,
    modsBusy: false,
    upgrade: null,
    upgradeBusy: false,
    upgradeXhr: null,
    uploadBusy: false,
    uploadInline: false,
    // A single-game export downloads itself; a platform export keeps its link.
    gameExportPending: null,
    uploadXhr: null,
    toastTimer: null,
    offlineNotified: false
  };

  // ------------------------------------------------------------------ i18n
  // Config/language.txt is the source of truth.  This page fetches the same
  // dictionary the device renders from and only swaps the text it owns.
  var i18n = { lang: 'zh-CN', strings: {}, failed: false };

  function t(key, params) {
    var text = i18n.strings[key];
    if (typeof text !== 'string') return key;
    if (!params) return text;
    return text.replace(/\{(\w+)\}/g, function (whole, name) {
      return Object.prototype.hasOwnProperty.call(params, name)
        ? String(params[name])
        : whole;
    });
  }

  function loadI18n() {
    return fetch('/api/i18n', { cache: 'no-store' })
      .then(function (response) {
        if (!response.ok) throw new Error('HTTP ' + response.status);
        return response.json();
      })
      .then(function (data) {
        if (!data || !data.strings || typeof data.strings !== 'object') {
          throw new Error('bad payload');
        }
        i18n.strings = data.strings;
        i18n.lang = data.language === 'en-US' ? 'en-US' : 'zh-CN';
        i18n.failed = false;
      })
      .catch(function () {
        i18n.failed = true;
      });
  }

  function applyI18n() {
    if (i18n.failed) return;
    document.documentElement.lang = i18n.lang;
    var nodes = document.querySelectorAll('[data-i18n]');
    for (var i = 0; i < nodes.length; i++) {
      nodes[i].textContent = t(nodes[i].getAttribute('data-i18n'));
    }
    var placeholders = document.querySelectorAll('[data-i18n-placeholder]');
    for (var p = 0; p < placeholders.length; p++) {
      placeholders[p].setAttribute(
        'placeholder',
        t(placeholders[p].getAttribute('data-i18n-placeholder'))
      );
    }
    var titles = document.querySelectorAll('[data-i18n-title]');
    for (var ti = 0; ti < titles.length; ti++) {
      titles[ti].setAttribute('title', t(titles[ti].getAttribute('data-i18n-title')));
    }
    var arias = document.querySelectorAll('[data-i18n-aria]');
    for (var a = 0; a < arias.length; a++) {
      arias[a].setAttribute('aria-label', t(arias[a].getAttribute('data-i18n-aria')));
    }
    var alts = document.querySelectorAll('[data-i18n-alt]');
    for (var ai = 0; ai < alts.length; ai++) {
      alts[ai].setAttribute('alt', t(alts[ai].getAttribute('data-i18n-alt')));
    }
    var langSelect = $('webLanguage');
    if (langSelect) langSelect.value = i18n.lang;
    var zhDocs = $('modsApiCard');
    var enDocs = $('modsApiCardEn');
    var internalDocs = $('modsInternalApiCard');
    if (zhDocs) zhDocs.hidden = i18n.lang === 'en-US';
    if (enDocs) enDocs.hidden = i18n.lang !== 'en-US';
    if (internalDocs) internalDocs.hidden = i18n.lang === 'en-US';
    var brand = document.querySelector('.brand-sub');
    if (brand) brand.textContent = t('web.page.brandSub');
    var title = document.querySelector('title');
    if (title) title.textContent = t('web.page.title');
  }

  function setLanguage(lang) {
    var status = $('webLanguageStatus');
    if (status) status.textContent = t('web.settings.languageSaving');
    return apiWrite('/api/settings', { language: lang })
      .then(function () {
        return loadI18n();
      })
      .then(function () {
        applyI18n();
        showTab(currentTab());
        if (status) status.textContent = t('web.settings.languageSaved');
      })
      .catch(function (error) {
        if (status) {
          status.textContent = t('web.settings.languageFailed', {
            message: error && error.message ? error.message : String(error)
          });
        }
      });
  }

  // --------------------------------------------------------------- 基础工具

  function $(id) {
    return document.getElementById(id);
  }

  function value(id) {
    var element = $(id);
    return element ? element.value : '';
  }

  function checked(id) {
    var element = $(id);
    return !!(element && element.checked);
  }

  function esc(value) {
    return String(value === null || value === undefined ? '' : value).replace(
      /[&<>"']/g,
      function (ch) {
        return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[ch];
      }
    );
  }

  function encodePath(path) {
    return encodeURIComponent(path);
  }

  function cacheBust(url) {
    return url + (String(url).indexOf('?') >= 0 ? '&' : '?') + 't=' + Date.now();
  }

  function pad2(number) {
    return number < 10 ? '0' + number : String(number);
  }

  function fmtBytes(bytes) {
    if (!bytes && bytes !== 0) return '—';
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
    return (bytes / (1024 * 1024)).toFixed(2) + ' MB';
  }

  function uploadLimitText() {
    return MAX_UPLOAD_MB + ' MB';
  }

  function useUploadLimit(mb) {
    if (!mb || mb < 1) return;
    MAX_UPLOAD_MB = mb;
    MAX_UPLOAD_BYTES = mb * 1024 * 1024;
  }

  function fmtTime(seconds) {
    if (!seconds) return '—';
    var date = new Date(seconds * 1000);
    return (
      date.getFullYear() +
      '-' +
      pad2(date.getMonth() + 1) +
      '-' +
      pad2(date.getDate()) +
      ' ' +
      pad2(date.getHours()) +
      ':' +
      pad2(date.getMinutes()) +
      ':' +
      pad2(date.getSeconds())
    );
  }

  // Uptime chip on the overview ("已运行 1 小时 2 分") keeps the seconds-level
  // detail of the boot duration.
  function fmtUptime(seconds) {
    seconds = Math.max(0, Math.floor(seconds || 0));
    var days = Math.floor(seconds / 86400);
    var hours = Math.floor((seconds % 86400) / 3600);
    var minutes = Math.floor((seconds % 3600) / 60);
    if (days > 0) return t('web.time.days_hours', { days: days, hours: hours });
    if (hours > 0) return t('web.time.hours_minutes', { hours: hours, minutes: minutes });
    if (minutes > 0) return t('web.time.minutes_seconds', { minutes: minutes, seconds: seconds % 60 });
    return t('web.time.seconds', { seconds: seconds });
  }

  // Game play time, from `playtime.json` via `/api/status` + `/api/saves`.
  // Always rendered as the requested "X 小时 Y 分"; 0 means the unit has never
  // recorded a play session ("暂无"), e.g. 3725 s -> "1 小时 2 分".
  function fmtDuration(seconds) {
    seconds = Math.max(0, Math.floor(seconds || 0));
    if (seconds <= 0) return t('web.time.none');
    var hours = Math.floor(seconds / 3600);
    var minutes = Math.floor((seconds % 3600) / 60);
    return t('web.time.hours_minutes', { hours: hours, minutes: minutes });
  }

  function fmtSyncTime(iso) {
    if (!iso) return '—';
    return String(iso).replace('T', ' ').replace(/\.\d+/, '').replace(/Z$/, '');
  }

  function toast(message, kind, duration) {
    var element = $('toast');
    if (!element) return;
    element.textContent = message;
    element.className = 'toast ' + (kind || 'ok');
    if (state.toastTimer) {
      clearTimeout(state.toastTimer);
      state.toastTimer = null;
    }
    var shown = duration || 3200;
    state.toastTimer = setTimeout(function () {
      element.classList.add('hidden');
    }, shown);
  }

  function showError(error, fallback) {
    var message = error && error.message ? error.message : fallback || t('web.error.operation');
    toast(message, 'error', 6000);
  }

  // --------------------------------------------------------------- 网络请求

  function api(path, options) {
    options = options || {};
    options.cache = 'no-store';
    return fetch(path, options).then(function (response) {
      return response.text().then(function (text) {
        var data = null;
        if (text) {
          try {
            data = JSON.parse(text);
          } catch (error) {
            data = null;
          }
        }
        if (!response.ok) {
          var message =
            data && data.error
              ? data.error
              : t('web.error.http', { status: response.status });
          var failure = new Error(message);
          failure.status = response.status;
          throw failure;
        }
        return data;
      });
    });
  }

  function apiWrite(path, body, method) {
    var options = {
      method: method || 'POST',
      headers: { 'Content-Type': 'application/json', 'X-Slot-Web': '1' }
    };
    if (body !== undefined) options.body = JSON.stringify(body);
    return api(path, options);
  }

  function putFile(path, file) {
    return api('/api/file?path=' + encodePath(path), {
      method: 'PUT',
      headers: { 'Content-Type': 'application/octet-stream', 'X-Slot-Web': '1' },
      body: file
    });
  }

  function putBinary(url, body, contentType) {
    return api(url, {
      method: 'PUT',
      headers: {
        'Content-Type': contentType || 'application/octet-stream',
        'X-Slot-Web': '1'
      },
      body: body
    });
  }

  // fetch() cannot report upload progress, so the version package goes through
  // XHR: `onProgress(loaded, total)` runs as the browser pushes the bytes, and
  // the same { error } shape as api() comes out the other end.
  // fetch() cannot report upload progress; every raw-file upload goes through
  // XHR so the browser can show how much has actually been sent.
  function uploadWithProgress(method, path, file, contentType, onProgress) {
    return new Promise(function (resolve, reject) {
      var xhr = new XMLHttpRequest();
      xhr.open(method, path, true);
      xhr.setRequestHeader('Content-Type', contentType || 'application/octet-stream');
      xhr.setRequestHeader('X-Slot-Web', '1');
      if (xhr.upload && onProgress) {
        xhr.upload.onprogress = function (event) {
          if (!event.lengthComputable) return;
          onProgress(event.loaded, event.total);
        };
      }
      xhr.onload = function () {
        var data = null;
        try {
          data = xhr.responseText ? JSON.parse(xhr.responseText) : null;
        } catch (error) {
          data = null;
        }
        if (xhr.status >= 200 && xhr.status < 300) {
          resolve(data);
          return;
        }
        var message =
          data && data.error ? data.error : t('web.error.http', { status: xhr.status });
        var failure = new Error(message);
        failure.status = xhr.status;
        reject(failure);
      };
      xhr.onerror = function () {
        reject(new Error(t('web.error.upload_interrupted')));
      };
      xhr.onabort = function () {
        reject(new Error(t('web.error.upload_cancelled')));
      };
      state.uploadXhr = xhr;
      xhr.send(file);
    }).then(
      function (data) {
        state.uploadXhr = null;
        return data;
      },
      function (error) {
        state.uploadXhr = null;
        throw error;
      }
    );
  }

  function uploadPercent(loaded, total) {
    return total > 0 ? Math.min(100, Math.floor((loaded / total) * 100)) : 0;
  }

  // Uploads started from a modal dialog draw their progress inside that dialog:
  // a modal <dialog> lives in the browser's top layer, so the full-screen
  // overlay can never paint above it.
  function inlineUploadRow() {
    var dialog = document.querySelector('dialog[open]');
    return dialog ? dialog.querySelector('[data-upload]') : null;
  }

  function uploadsInline() {
    return !!inlineUploadRow();
  }

  function showUploadTargets(inline, title, fileLabel) {
    var row = inline ? inlineUploadRow() : null;
    if (row) {
      row.classList.remove('hidden');
      var label = row.querySelector('[data-upload-title]');
      if (label) label.textContent = title || t('web.upload.uploading');
      var file = row.querySelector('[data-upload-file]');
      if (file) file.textContent = fileLabel || '';
      var bar = row.querySelector('[data-upload-bar]');
      if (bar) bar.style.width = '0%';
      var text = row.querySelector('[data-upload-text]');
      if (text) text.textContent = '0%';
      if ($('uploadProgressOverlay')) $('uploadProgressOverlay').classList.add('hidden');
    } else {
      var stale = document.querySelector('[data-upload]');
      if (stale) stale.classList.add('hidden');
      if ($('uploadProgressOverlay')) $('uploadProgressOverlay').classList.remove('hidden');
      if ($('uploadProgressTitle')) $('uploadProgressTitle').textContent = title || t('web.upload.uploading');
      if ($('uploadProgressFile')) $('uploadProgressFile').textContent = fileLabel || '';
    }
  }

  function beginUpload(title, fileLabel) {
    if (state.uploadBusy) {
      toast(t('web.upload.busy'), 'warn');
      return false;
    }
    if (state.upgradeBusy) {
      toast(t('web.upload.upgrade_busy'), 'warn');
      return false;
    }
    state.uploadBusy = true;
    state.uploadInline = uploadsInline();
    showUploadTargets(state.uploadInline, title, fileLabel);
    setUploadProgress(0, 0);
    return true;
  }

  function setUploadProgress(loaded, total) {
    var percent = uploadPercent(loaded, total);
    var text =
      total > 0 ? percent + '% · ' + fmtBytes(loaded) + ' / ' + fmtBytes(total) : '0%';
    // The dialog can be closed mid-upload; fall back to the overlay so the
    // progress never disappears entirely.
    if (state.uploadInline && !uploadsInline()) {
      state.uploadInline = false;
      var stale = document.querySelector('[data-upload]');
      if (stale) stale.classList.add('hidden');
      if ($('uploadProgressOverlay')) $('uploadProgressOverlay').classList.remove('hidden');
    }
    if (state.uploadInline) {
      var row = inlineUploadRow();
      if (row) {
        var bar = row.querySelector('[data-upload-bar]');
        if (bar) bar.style.width = percent + '%';
        var label = row.querySelector('[data-upload-text]');
        if (label) label.textContent = text;
      }
    } else {
      if ($('uploadProgressBar')) $('uploadProgressBar').style.width = percent + '%';
      if ($('uploadProgressText')) $('uploadProgressText').textContent = text;
    }
  }

  function endUpload() {
    var inline = state.uploadInline;
    state.uploadBusy = false;
    state.uploadXhr = null;
    state.uploadInline = false;
    var hide = function () {
      if (state.uploadBusy) return;
      var row = document.querySelector('[data-upload]');
      if (row) row.classList.add('hidden');
      if ($('uploadProgressOverlay')) $('uploadProgressOverlay').classList.add('hidden');
    };
    // Keep the finished bar visible for a beat, whichever row it was drawn in.
    void inline;
    setTimeout(hide, 450);
  }

  function isWritable() {
    return !!(state.status && state.status.writable);
  }

  // ------------------------------------------------------------------- Tab

  function currentTab() {
    var hash = (window.location.hash || '').replace(/^#/, '');
    return TABS.indexOf(hash) >= 0 ? hash : 'overview';
  }

  function showTab(tab) {
    if (tab !== 'mods') { closeModDetail(); closeModUi(); }
    if (tab !== 'files') closeFilePreview();
    var tabs = document.querySelectorAll('.tab');
    for (var index = 0; index < tabs.length; index += 1) {
      tabs[index].classList.toggle('active', tabs[index].getAttribute('data-tab') === tab);
    }
    var pages = document.querySelectorAll('.page');
    for (var pageIndex = 0; pageIndex < pages.length; pageIndex += 1) {
      pages[pageIndex].hidden = pages[pageIndex].getAttribute('data-page') !== tab;
    }
    if (tab === 'overview') {
      refreshStatus().catch(function () {});
      loadRecentLog();
    } else if (tab === 'roms') {
      loadRoms();
    } else if (tab === 'saves') {
      loadSaves();
    } else if (tab === 'files') {
      loadFiles();
    } else if (tab === 'sync') {
      loadSync();
    } else if (tab === 'mods') {
      loadMods();
    } else if (tab === 'network') {
      loadNetworkStatus();
    } else if (tab === 'about') {
      renderAbout();
      loadUpgradeStatus();
    }
  }

  // ---------------------------------------------------------------- 概览页

  function refreshStatus() {
    return api('/api/status')
      .then(function (status) {
        state.status = status;
        state.offlineNotified = false;
        renderStatus();
        applyWritable();
        return status;
      })
      .catch(function (error) {
        state.status = null;
        var chip = $('statusChip');
        if (chip) chip.className = 'status-chip offline';
        var text = $('statusText');
        if (text) text.textContent = t('web.status.connection_lost');
        if (!state.offlineNotified) {
          state.offlineNotified = true;
          showError(error, t('web.error.device_status'));
        }
        throw error;
      });
  }

  function renderStatus() {
    var status = state.status;
    if (!status) return;

    var chip = $('statusChip');
    var chipText = $('statusText');
    if (chip && chipText) {
      if (!status.writable) {
        chip.className = 'status-chip readonly';
        chipText.textContent = t('web.overview.readonly_busy');
      } else if (status.network && status.network.connected) {
        chip.className = 'status-chip online';
        chipText.textContent = t('web.overview.online', {
          ssid: status.network.ssid || t('web.overview.wifi_connected')
        });
      } else {
        chip.className = 'status-chip offline';
        chipText.textContent = t('web.overview.wifi_off');
      }
    }

    $('slotVersion').textContent = 'v' + status.slot;
    $('linkslotVersion').textContent = 'CLI ' + (status.linkslot || 'unknown');

    var network = status.network || {};
    var storage = status.storage || {};
    var web = status.web || {};

    $('ovWifi').textContent = network.connected ? network.ssid || t('web.overview.connected') : t('web.overview.disconnected');
    $('ovIp').textContent = network.ip || t('web.overview.no_ipv4');
    $('ovUrl').textContent = web.url || '—';
    $('ovUrl2').textContent = web.url || '—';
    $('ovGames').textContent = String(storage.games || 0);
    $('ovCounts').textContent = t('web.overview.states_saves', { states: storage.states || 0, battery: storage.battery || 0 });
    $('ovWritable').textContent = status.writable ? t('web.overview.writable') : t('web.overview.readonly_game');
    $('ovWritable').className = 'v ' + (status.writable ? 'ok' : 'warn');
    $('ovUptime').textContent = t('web.overview.running', { duration: fmtUptime(status.uptime_secs) });
    $('ovPlaytime').textContent = fmtDuration(storage.playtime_seconds);
    $('ovVersion').textContent = status.slot || '—';
    $('ovLinkVersion').textContent = status.linkslot || 'unknown';
    $('ovStates').textContent = String(storage.states || 0);
    $('ovBattery').textContent = String(storage.battery || 0);

    renderAbout();
  }

  function renderAbout() {
    var status = state.status;
    if (!status) return;
    var web = status.web || {};
    $('aboutSlot').textContent = status.slot || '—';
    $('aboutLinkSlot').textContent = status.linkslot || 'unknown';
    $('aboutWeb').textContent = t('web.overview.web_port', { port: web.port || '—' });
    $('aboutUrl').textContent = web.url || '—';
    renderUpgradeStatus();
  }

  function renderUpgradeStatus() {
    var element = $('upgradeStatus');
    if (!element) return;
    var upgrade = state.upgrade || {};
    var stateName = upgrade.state || 'idle';
    if (stateName === 'staged') {
      element.className = 'hint ok';
      element.textContent = upgrade.same_version
        ? t('web.upgrade.staged_same', { version: upgrade.target || '?' })
        : t('web.upgrade.staged', {
            version: upgrade.target || '?',
            files: upgrade.files || 0
          });
    } else if (stateName === 'failed') {
      element.className = 'hint error';
      element.textContent = t('web.upgrade.failed', {
        message: upgrade.message || t('web.error.unknown')
      });
    } else if (stateName === 'applying' || stateName === 'rebooting') {
      element.className = 'hint warn';
      element.textContent = t('web.upgrade.applying');
    } else {
      element.className = 'hint';
      element.textContent = t('web.upgrade.empty');
    }
  }

  function loadUpgradeStatus() {
    return api('/api/upgrade/status')
      .then(function (upgrade) {
        state.upgrade = upgrade;
        renderUpgradeStatus();
      })
      .catch(function (error) {
        var element = $('upgradeStatus');
        if (element) {
          element.className = 'hint error';
          element.textContent = t('web.error.upgrade_status', {
            message: error.message || error
          });
        }
      });
  }

  function uploadUpgrade() {
    if (state.upgradeBusy) return;
    if (state.uploadBusy) {
      toast(t('web.upload.busy'), 'warn');
      return;
    }
    if (!isWritable()) {
      toast(t('web.upgrade.readonly'), 'warn');
      return;
    }
    var input = $('upgradeFile');
    var file = input && input.files && input.files[0] ? input.files[0] : null;
    if (!file) {
      toast(t('web.upgrade.choose'), 'warn');
      return;
    }
    if (!/\.zip$/i.test(file.name || '')) {
      toast(t('web.upgrade.not_zip'), 'error');
      return;
    }
    if (file.size > MAX_UPLOAD_BYTES) {
      toast(t('web.upgrade.too_large', { limit: uploadLimitText() }), 'error');
      return;
    }
    var button = $('upgradeUpload');
    state.upgradeBusy = true;
    if (button) {
      button.disabled = true;
      button.textContent = t('web.upgrade.uploading');
    }
    var element = $('upgradeStatus');
    if (element) {
      element.className = 'hint';
      element.textContent = t('web.upgrade.verifying');
    }
    showUpgradeProgress(0, file.size);
    uploadWithProgress('POST', '/api/upgrade', file, 'application/zip', function (loaded, total) {
      showUpgradeProgress(loaded, total);
    })
      .then(function (result) {
        state.upgrade = {
          state: 'staged',
          target: result.target,
          files: result.files,
          bytes: result.bytes,
          build: result.build,
          message: result.message
        };
        renderUpgradeStatus();
        toast(t('web.upgrade.uploaded', { version: result.target || '?' }), 'ok', 7000);
      })
      .catch(function (error) {
        showError(error, t('web.upgrade.upload_failed'));
        loadUpgradeStatus();
      })
      .then(function () {
        state.upgradeBusy = false;
        hideUpgradeProgress();
        if (button) {
          button.disabled = !isWritable();
          button.textContent = t('web.about.upgradeUpload');
        }
        if (input) input.value = '';
      });
  }

  // The bar fills as the bytes go out, then shimmers while the device unpacks
  // and verifies the package (the upload itself is already done).
  function showUpgradeProgress(loaded, total) {
    var box = $('upgradeProgress');
    var bar = $('upgradeProgressBar');
    var text = $('upgradeProgressText');
    if (!box || !bar || !text) return;
    box.classList.remove('hidden');
    var percent = total > 0 ? Math.min(100, Math.floor((loaded / total) * 100)) : 0;
    box.setAttribute('aria-valuenow', String(percent));
    var done = total > 0 && loaded >= total;
    box.classList.toggle('is-verifying', done);
    bar.style.width = percent + '%';
    if (done) {
      text.textContent = t('web.upgrade.sent', { size: fmtBytes(total) });
    } else {
      text.textContent =
        percent + '% · ' + fmtBytes(loaded) + ' / ' + fmtBytes(total);
    }
  }

  function hideUpgradeProgress() {
    var box = $('upgradeProgress');
    if (!box) return;
    box.classList.add('hidden');
    box.classList.remove('is-verifying');
    var bar = $('upgradeProgressBar');
    if (bar) bar.style.width = '0%';
    var text = $('upgradeProgressText');
    if (text) text.textContent = '0%';
  }

  function applyWritable() {
    var writable = isWritable();
    var banner = $('writeBanner');
    if (banner) banner.classList.toggle('hidden', writable);
    document.body.classList.toggle('readonly', !writable);
    var saveButton = $('saveWebdav');
    if (saveButton) saveButton.disabled = !writable;
    var testButton = $('testWebdav');
    if (testButton) testButton.disabled = !writable;
    var connectButton = $('connectBtn');
    if (connectButton) connectButton.disabled = !writable;
    var romCover = $('romReplaceCover');
    if (romCover) romCover.disabled = !writable;
    var introPick = $('romIntroPick');
    if (introPick) introPick.disabled = !writable;
    var videoPick = $('romVideoPick');
    if (videoPick) videoPick.disabled = !writable;
    var romReplace = $('romReplaceRom');
    if (romReplace) romReplace.disabled = !writable;
    var romRename = $('romRename');
    if (romRename) romRename.disabled = !writable;
    var romDelete = $('romDelete');
    if (romDelete) romDelete.disabled = !writable;
    applyPlayWebGate(writable);
    var romExport = $('romExport');
    if (romExport) romExport.disabled = !writable || collectionBusy;
    ['romGuidePath', 'romGuideSave', 'romGuidePick'].forEach(function (id) {
      var control = $(id);
      if (control) control.disabled = !writable;
    });
    ['romColor', 'romColorHex', 'romColorRandom', 'romColorSave', 'romCheatPath', 'romCheatSave', 'romModel', 'romModelSave', 'frontAdd', 'frontNew', 'frontImport', 'frontModel'].forEach(function (id) {
      var control = $(id);
      if (control) control.disabled = !writable;
    });
    var addGame = $('addGame');
    if (addGame) addGame.disabled = !writable || collectionBusy;
    var collectionImport = $('collectionImport');
    if (collectionImport) collectionImport.disabled = !writable || collectionBusy;
    var collectionExport = $('collectionExport');
    if (collectionExport) {
      collectionExport.disabled = !writable || collectionBusy || !state.romFilter;
    }
    var coverSave = $('coverSave');
    if (coverSave) coverSave.disabled = !writable || state.coverBusy;
    var dynamic = document.querySelectorAll('[data-action="upload"],[data-action="sync"],[data-action="delete-save"]');
    for (var index = 0; index < dynamic.length; index += 1) {
      dynamic[index].disabled = !writable;
    }
    var fileWrites = document.querySelectorAll('[data-file-write]');
    for (var fileIndex = 0; fileIndex < fileWrites.length; fileIndex += 1) {
      fileWrites[fileIndex].disabled = !writable || state.filesBusy;
    }
    if ($('fileUpload')) $('fileUpload').disabled = !writable || state.filesBusy;
    if ($('upgradeUpload')) {
      $('upgradeUpload').disabled = !writable || state.upgradeBusy;
    }
    var storageRadios = document.querySelectorAll('input[name="storageActive"]');
    for (var radioIndex = 0; radioIndex < storageRadios.length; radioIndex += 1) {
      storageRadios[radioIndex].disabled = !writable || state.syncSwitchBusy;
    }
    if (document.getElementById('modList')) renderModsControls();
  }

  function loadRecentLog() {
    return api('/api/sync')
      .then(function (info) {
        state.sync = info;
        var lines = (info && info.log_tail) || [];
        var recent = lines.slice(-12);
        $('recentLog').textContent = recent.length ? recent.join('\n') : t('web.common.noLog');
      })
      .catch(function () {
        $('recentLog').textContent = t('web.common.noLog');
      });
  }

  // ---------------------------------------------------------------- 游戏库

  function loadRoms() {
    var summary = $('romsSummary');
    if (summary) summary.textContent = t('web.common.reading');
    return api('/api/roms')
      .then(function (data) {
        state.roms = (data && data.games) || [];
        renderRoms();
      })
      .catch(function (error) {
        detachListMore(state.romView);
        state.romView = null;
        if (summary) summary.textContent = t('web.overview.read_failed');
        $('romList').innerHTML =
          '<div class="empty">' + esc(t('web.error.library_retry')) + '</div>';
        showError(error, t('web.error.library_read'));
      });
  }

  function filteredRoms() {
    var query = state.romQuery.trim().toLowerCase();
    return (state.roms || []).filter(function (game) {
      if (state.romFilter && game.platform !== state.romFilter) return false;
      if (!query) return true;
      return (
        String(game.stem || '').toLowerCase().indexOf(query) >= 0 ||
        String(game.file || '').toLowerCase().indexOf(query) >= 0
      );
    });
  }

  function listMoreHtml(label, shown, total) {
    return (
      '<div class="list-more">' +
      '<button class="btn ghost" type="button">' +
      esc(label) +
      ' <span class="muted small">' +
      shown +
      ' / ' +
      total +
      '</span></button>' +
      '</div>'
    );
  }

  function detachListMore(view) {
    if (!view) return;
    if (view.observer) {
      view.observer.disconnect();
      view.observer = null;
    }
    if (view.onScroll) {
      window.removeEventListener('scroll', view.onScroll);
      view.onScroll = null;
    }
    if (view.onResize) {
      window.removeEventListener('resize', view.onResize);
      view.onResize = null;
    }
  }

  function watchListMore(container, view, loadMore) {
    detachListMore(view);
    var sentinel = container.querySelector('.list-more');
    if (!sentinel) return;
    var button = sentinel.querySelector('button');
    if (button) {
      button.addEventListener('click', function () {
        loadMore(view);
      });
    }
    // The scroll listener is the compatibility path: some WebViews expose
    // IntersectionObserver but do not deliver callbacks reliably.
    view.onScroll = function () {
      var current = container.querySelector('.list-more');
      if (!current) return;
      var rect = current.getBoundingClientRect();
      if (rect.top <= window.innerHeight + 800) loadMore(view);
    };
    window.addEventListener('scroll', view.onScroll, { passive: true });
    view.onResize = view.onScroll;
    window.addEventListener('resize', view.onResize, { passive: true });
    if (!('IntersectionObserver' in window)) {
      window.setTimeout(view.onScroll, 0);
      return;
    }
    view.observer = new IntersectionObserver(
      function (entries) {
        for (var index = 0; index < entries.length; index += 1) {
          if (entries[index].isIntersecting) {
            loadMore(view);
            return;
          }
        }
      },
      { root: null, rootMargin: '800px 0px', threshold: 0 }
    );
    view.observer.observe(sentinel);
    window.setTimeout(view.onScroll, 0);
  }

  function romCardHtml(game, index) {
    var ratio = (game.cover_width || 532) + ' / ' + (game.cover_height || 276);
    var cover = game.has_cover
      ? '<img loading="lazy" decoding="async" alt="' +
        esc(t('web.roms.cover_alt', { name: game.stem })) +
        '" src="' +
        esc(game.cover) +
        '&amp;t=' +
        encodeURIComponent(String(game.mtime || 0)) +
        '">'
      : '<span class="rom-cover-empty">' + esc(game.platform) + '</span>';
    return (
      '<button class="rom-card" type="button" data-rom-index="' +
      index +
      '">' +
      '<span class="rom-cover" style="aspect-ratio:' +
      ratio +
      '">' +
      cover +
      '</span>' +
      '<span class="rom-card-info">' +
      '<strong>' +
      esc(game.stem || t('web.roms.unnamed')) +
      '</strong>' +
      '<span class="rom-meta">' +
      '<span class="pill">' +
      esc(game.platform) +
      '</span>' +
      '<span>' +
      fmtBytes(game.size) +
      '</span>' +
      '</span>' +
      '</span>' +
      '</button>'
    );
  }

  function romSummary(all, view) {
    var text = t('web.roms.count', { total: all.length, matched: view.total });
    if (view.rendered < view.total) {
      text += ' · ' + t('web.roms.shown', { shown: view.rendered });
    }
    $('romsSummary').textContent = text;
  }

  function renderRomBatch(view) {
    if (state.romView !== view) return;
    var container = $('romList');
    if (!container) return;
    detachListMore(view);
    var end = Math.min(view.rendered + ROM_BATCH, view.total);
    var html = [];
    for (var index = view.rendered; index < end; index += 1) {
      var game = view.list[index];
      html.push(romCardHtml(game, view.indexMap.get(game)));
    }
    var more = container.querySelector('.list-more');
    if (more) more.insertAdjacentHTML('beforebegin', html.join(''));
    else container.insertAdjacentHTML('beforeend', html.join(''));
    if (more) more.remove();
    view.rendered = end;
    romSummary(view.all, view);
    if (view.rendered < view.total) {
      container.insertAdjacentHTML(
        'beforeend',
        listMoreHtml(t('web.roms.load_more'), view.rendered, view.total)
      );
      watchListMore(container, view, renderRomBatch);
    }
    applyWritable();
  }

  function renderRoms() {
    var container = $('romList');
    if (!container) return;
    var all = state.roms || [];
    var list = filteredRoms();
    detachListMore(state.romView);
    state.romView = null;
    var counts = { '': all.length, GBA: 0, GB: 0, GBC: 0, FC: 0, SFC: 0, MD: 0 };
    for (var n = 0; n < all.length; n += 1) {
      if (counts[all[n].platform] !== undefined) counts[all[n].platform] += 1;
    }
    var filters = $('romFilters');
    var exportButton = $('collectionExport');
    if (exportButton) {
      exportButton.textContent = state.romFilter
        ? t('web.roms.export_named', { platform: state.romFilter })
        : t('web.roms.export_disabled');
      exportButton.disabled = !state.romFilter || collectionBusy || !isWritable();
      exportButton.title = state.romFilter
        ? t('web.roms.export_hint', { platform: state.romFilter })
        : t('web.roms.export_none');
    }
    if (filters) {
      var buttons = filters.querySelectorAll('[data-platform]');
      for (var b = 0; b < buttons.length; b += 1) {
        var key = buttons[b].getAttribute('data-platform');
        buttons[b].classList.toggle('active', key === state.romFilter);
        buttons[b].textContent =
          (key || t('web.common.all')) + (counts[key] !== undefined ? ' ' + counts[key] : '');
      }
    }
    if (!all.length) {
      $('romsSummary').textContent = t('web.roms.empty_count');
      container.innerHTML =
        '<div class="empty">' + esc(t('web.roms.no_roms')) + '</div>';
      applyWritable();
      return;
    }
    if (!list.length) {
      $('romsSummary').textContent = t('web.roms.count_none', { total: all.length });
      container.innerHTML = '<div class="empty">' + esc(t('web.roms.none_match')) + '</div>';
      applyWritable();
      return;
    }
    var indexMap = new Map();
    for (var index = 0; index < all.length; index += 1) {
      indexMap.set(all[index], index);
    }
    var view = {
      all: all,
      list: list,
      indexMap: indexMap,
      total: list.length,
      rendered: 0,
      observer: null
    };
    state.romView = view;
    container.innerHTML = '';
    renderRomBatch(view);
  }

  function renderRomDialogCover(game) {
    var image = $('romDialogCover');
    var empty = $('romDialogNoCover');
    if (!image || !empty) return;
    if (game && game.has_cover) {
      image.hidden = false;
      empty.hidden = true;
      image.src = cacheBust(game.cover);
    } else {
      image.hidden = true;
      empty.hidden = false;
      image.removeAttribute('src');
    }
  }

  function renderRomAssets(game) {
    var intro = $('romIntroState');
    var video = $('romVideoState');
    var guide = $('romGuideState');
    if (intro) intro.textContent = game && game.has_intro ? t('web.rom.uploaded') : t('web.rom.noDescription');
    if (video) video.textContent = game && game.has_video ? t('web.rom.uploaded') : t('web.rom.noVideo');
    if (guide) guide.textContent = game && game.has_guide ? t('web.rom.configured') : t('web.rom.noGuide');
    loadRomIntro(game);
  }

  /// The intro text lives beside the cover as a .txt; show it in the dialog so
  /// the player does not need to boot the game to read it.
  function loadRomIntro(game) {
    var body = $('romIntroBody');
    if (!body) return;
    body.hidden = true;
    body.textContent = '';
    body.classList.remove('is-loading');
    if (!game || !game.has_intro) return;
    body.hidden = false;
    body.classList.add('is-loading');
    body.textContent = t('web.rom.reading_intro');
    api('/api/rom/intro?rom=' + encodePath(game.rom))
      .then(function (data) {
        body.classList.remove('is-loading');
        var text = (data && data.text) || '';
        body.textContent = text.trim() ? text : t('web.rom.intro_empty');
        body.scrollTop = 0;
      })
      .catch(function () {
        body.hidden = true;
        body.classList.remove('is-loading');
        body.textContent = '';
      });
  }

  function normalizeRomColor(value) {
    var hex = String(value || '').trim();
    if (hex.charAt(0) !== '#') hex = '#' + hex;
    return /^#[0-9a-fA-F]{6}$/.test(hex) ? hex.toLowerCase() : null;
  }

  function setRomColor(value) {
    var hex = normalizeRomColor(value) || '#8fd6c4';
    var picker = $('romColor');
    var input = $('romColorHex');
    if (picker) picker.value = hex;
    if (input) input.value = hex;
  }

  function randomRomColor() {
    var bytes = new Uint8Array(3);
    if (window.crypto && window.crypto.getRandomValues) {
      window.crypto.getRandomValues(bytes);
    } else {
      for (var index = 0; index < 3; index += 1) {
        bytes[index] = Math.floor(Math.random() * 256);
      }
    }
    var parts = [];
    for (var channel = 0; channel < 3; channel += 1) {
      parts.push((bytes[channel] + 0x100).toString(16).slice(-2));
    }
    return '#' + parts.join('');
  }

  function saveRomColor() {
    var game = state.selectedRom;
    if (!game) return;
    if (!isWritable()) {
      toast(t('web.rom.readonly_color'), 'warn');
      return;
    }
    var hex = normalizeRomColor($('romColorHex').value);
    if (!hex) {
      toast(t('web.rom.color_invalid'), 'warn');
      return;
    }
    apiWrite('/api/rom/color?rom=' + encodePath(game.rom), { color: hex }, 'PUT')
      .then(function (data) {
        game.color = (data && data.color) || hex;
        setRomColor(game.color);
        toast(t('web.rom.color_saved'));
      })
      .catch(function (error) {
        toast((error && error.message) || t('web.rom.color_save_failed'), 'warn');
      });
  }

  function saveRomModel() {
    var game = state.selectedRom;
    if (!game) return;
    if (!isWritable()) {
      toast(t('web.rom.modelReadonly'), 'warn');
      return;
    }
    var model = $('romModel').value === 'nes' ? 'nes' : 'famicom';
    apiWrite('/api/rom/model?rom=' + encodePath(game.rom), { model: model }, 'PUT')
      .then(function (data) {
        game.model = (data && data.model) || model;
        toast(t('web.rom.modelSaved'));
        loadFronts(game);
        loadRoms();
      })
      .catch(function (error) {
        showError(error, t('web.rom.modelFailed'));
      });
  }

  function loadFronts(game) {
    var box = $('frontList');
    if (!box || !game) return;
    box.textContent = t('web.common.reading');
    fetch('/api/rom/fronts?rom=' + encodePath(game.rom), { cache: 'no-store' })
      .then(function (response) {
        if (!response.ok) throw new Error('HTTP ' + response.status);
        return response.json();
      })
      .then(function (data) {
        box.textContent = '';
        var faces = (data && data.faces) || [];
        faces = faces.filter(function (face) {
          if ((game.platform || '') !== 'FC') return face.model === 'any' || !face.model;
          if (face.model === 'any' || !face.model) return true;
          return face.model === (game.model === 'nes' ? 'nes' : 'famicom');
        });
        state.frontCount = faces.length;
        state.frontCountRom = game.rom;
        frontQuickUpdateCount();
        frontQuickRefreshPreview(game);
        if (!faces.length) {
          var empty = document.createElement('p');
          empty.className = 'hint';
          empty.textContent = t('web.rom.frontNone');
          box.appendChild(empty);
          return;
        }
        faces.forEach(function (face) {
          var card = document.createElement('div');
          card.className = 'front-card' + (face.id === data.selected ? ' selected' : '');
          var image = document.createElement('img');
          image.src = face.url;
          image.alt = face.name;
          card.appendChild(image);
          var title = document.createElement('strong');
          title.textContent = face.name;
          card.appendChild(title);
          var meta = document.createElement('span');
          meta.className = 'muted small';
          meta.textContent =
            face.id === data.selected
              ? t('web.rom.frontSelected')
              : (face.model === 'any' ? '' : face.model);
          card.appendChild(meta);
          var actions = document.createElement('div');
          actions.className = 'row-actions';
          var edit = document.createElement('button');
          edit.type = 'button';
          edit.className = 'btn ghost small';
          edit.textContent = t('web.rom.frontEdit');
          edit.addEventListener('click', function () {
            openFrontEditor(game, face);
          });
          actions.appendChild(edit);
          var copy = document.createElement('button');
          copy.type = 'button';
          copy.className = 'btn ghost small';
          copy.textContent = t('web.rom.frontCopy');
          copy.addEventListener('click', function () {
            copyFront(game, face);
          });
          actions.appendChild(copy);
          var exportButton = document.createElement('button');
          exportButton.type = 'button';
          exportButton.className = 'btn ghost small';
          exportButton.textContent = t('web.rom.frontExport');
          exportButton.addEventListener('click', function () {
            exportFront(game, face);
          });
          actions.appendChild(exportButton);
          if (face.id !== data.selected) {
            var use = document.createElement('button');
            use.type = 'button';
            use.className = 'btn ghost small';
            use.textContent = t('web.rom.frontSelect');
            use.addEventListener('click', function () {
              selectFront(game, face.id);
            });
            actions.appendChild(use);
          }
          var remove = document.createElement('button');
          remove.type = 'button';
          remove.className = 'btn ghost small';
          remove.textContent = t('web.rom.frontDelete');
          remove.addEventListener('click', function () {
            if (!window.confirm(t('web.rom.frontDelete') + '?')) return;
            deleteFront(game, face.id);
          });
          actions.appendChild(remove);
          card.appendChild(actions);
          box.appendChild(card);
        });
      })
      .catch(function (error) {
        box.textContent = (error && error.message) || String(error);
      });
  }

  function exportFront(game, face) {
    fetch(
      '/api/rom/fronts/export?rom=' + encodePath(game.rom) + '&id=' + encodeURIComponent(face.id),
      { cache: 'no-store' }
    )
      .then(function (response) {
        if (!response.ok) throw new Error('HTTP ' + response.status);
        return response.blob();
      })
      .then(function (blob) {
        var url = URL.createObjectURL(blob);
        var link = document.createElement('a');
        link.href = url;
        link.download = (face.name || 'front').replace(/[\\/:*?"<>|]+/g, '_') + '.front.json';
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);
        setTimeout(function () { URL.revokeObjectURL(url); }, 10000);
        toast(t('web.rom.frontExported'));
      })
      .catch(function (error) {
        showError(error, t('web.rom.frontUploadFailed'));
      });
  }

  function importFront(game, file) {
    if (!file) return;
    if (file.size > 24 * 1024 * 1024) {
      toast(t('web.rom.frontTooLarge'), 'error');
      return;
    }
    var reader = new FileReader();
    reader.onload = function () {
      fetch('/api/rom/fronts/import?rom=' + encodePath(game.rom), {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-Slot-Web': '1' },
        body: reader.result
      })
        .then(function (response) {
          return response.json().then(function (body) {
            if (!response.ok) throw new Error((body && body.error) || 'HTTP ' + response.status);
            return body;
          });
        })
        .then(function () {
          toast(t('web.rom.frontImported'));
          loadFronts(game);
          loadRoms();
        })
        .catch(function (error) {
          showError(error, t('web.rom.frontUploadFailed'));
        });
    };
    reader.onerror = function () { toast(t('web.rom.frontUploadFailed'), 'error'); };
    reader.readAsText(file);
  }

  function copyFront(game, face) {    if (!isWritable()) {
      toast(t('web.rom.frontReadonly'), 'warn');
      return;
    }
    apiWrite(
      '/api/rom/fronts/copy?rom=' + encodePath(game.rom) +
        '&id=' + encodeURIComponent(face.id) +
        '&name=' + encodeURIComponent((face.name || game.stem || '') + ' ' + t('web.frontEditor.copySuffix')),
      {},
      'POST'
    )
      .then(function () {
        toast(t('web.rom.frontCopied'));
        loadFronts(game);
        loadRoms();
      })
      .catch(function (error) {
        showError(error, t('web.rom.frontUploadFailed'));
      });
  }

  function selectFront(game, id) {    if (!isWritable()) {
      toast(t('web.rom.frontReadonly'), 'warn');
      return;
    }
    apiWrite(
      '/api/rom/fronts/select?rom=' + encodePath(game.rom) + '&id=' + encodeURIComponent(id),
      {},
      'PUT'
    )
      .then(function () {
        loadFronts(game);
        loadRoms();
      })
      .catch(function (error) {
        showError(error, t('web.rom.frontUploadFailed'));
      });
  }

  function deleteFront(game, id) {
    if (!isWritable()) {
      toast(t('web.rom.frontReadonly'), 'warn');
      return;
    }
    apiWrite(
      '/api/rom/fronts/delete?rom=' + encodePath(game.rom) + '&id=' + encodeURIComponent(id),
      {},
      'POST'
    )
      .then(function () {
        toast(t('web.rom.frontDeleted'));
        loadFronts(game);
        loadRoms();
      })
      .catch(function (error) {
        showError(error, t('web.rom.frontUploadFailed'));
      });
  }

  function frontCanvasSize(game) {
    var platform = (game && game.platform) || 'GBA';
    if (platform === 'GBA') return [360, 202];
    if (platform === 'GB' || platform === 'GBC') return [240, 276];
    if (platform === 'FC') return [360, 299];
    if (platform === 'SFC') return [360, 252];
    if (platform === 'MD') return [360, 244];
    return [360, 299];
  }

  function frontQuickModel(game) {
    var select = $('frontModel');
    if (select && !select.hidden && select.offsetParent !== null) return select.value;
    if ((game.platform || '') === 'FC') return game.model === 'nes' ? 'nes' : 'famicom';
    return 'any';
  }

  function frontQuickRoundRect(ctx, x, y, w, h, r) {
    r = Math.max(0, Math.min(r, w / 2, h / 2));
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.lineTo(x + w - r, y);
    ctx.quadraticCurveTo(x + w, y, x + w, y + r);
    ctx.lineTo(x + w, y + h - r);
    ctx.quadraticCurveTo(x + w, y + h, x + w - r, y + h);
    ctx.lineTo(x + r, y + h);
    ctx.quadraticCurveTo(x, y + h, x, y + h - r);
    ctx.lineTo(x, y + r);
    ctx.quadraticCurveTo(x, y, x + r, y);
    ctx.closePath();
  }

  /// Compose a cover-only sticker in the current model's label window.  The
  /// shell is deliberately not baked into this PNG: the device keeps drawing
  /// its own shell and only the label/cover layer changes.
  function frontQuickSticker(game, image, fit) {
    var size = frontCanvasSize(game);
    var guide = frontGuideRect(game);
    var canvas = document.createElement('canvas');
    canvas.width = size[0];
    canvas.height = size[1];
    var ctx = canvas.getContext('2d');
    var x = guide[0], y = guide[1], w = guide[2], h = guide[3];
    var radius = Math.max(4, Math.min(12, Math.min(w, h) * 0.06));
    frontQuickRoundRect(ctx, x, y, w, h, radius);
    ctx.fillStyle = '#f3ead9';
    ctx.fill();
    ctx.save();
    frontQuickRoundRect(ctx, x, y, w, h, radius);
    ctx.clip();
    if (fit === 'fill') {
      var scale = Math.max(w / image.width, h / image.height);
      var sw = w / scale;
      var sh = h / scale;
      ctx.drawImage(image, (image.width - sw) / 2, (image.height - sh) / 2, sw, sh, x, y, w, h);
    } else {
      var pad = 5;
      var scale2 = Math.min((w - pad * 2) / image.width, (h - pad * 2) / image.height);
      var iw = Math.max(1, image.width * scale2);
      var ih = Math.max(1, image.height * scale2);
      ctx.drawImage(image, x + (w - iw) / 2, y + (h - ih) / 2, iw, ih);
    }
    ctx.restore();
    frontQuickRoundRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
    ctx.strokeStyle = 'rgba(25, 22, 18, 0.7)';
    ctx.lineWidth = 2;
    ctx.stroke();
    return canvas;
  }

  function frontQuickImage(src) {
    return new Promise(function (resolve, reject) {
      var image = new Image();
      image.onload = function () { resolve(image); };
      image.onerror = function () { reject(new Error('image decode failed')); };
      image.src = src;
    });
  }

  function frontQuickDataUrl(blob) {
    if (blob.size > 4 * 1024 * 1024) {
      toast(t('web.frontEditor.imageTooLarge'), 'error');
      return Promise.reject(new Error('image too large'));
    }
    return new Promise(function (resolve) {
      frontFileToDataUrl(blob, resolve);
    });
  }

  function frontQuickRefreshPreview(game) {
    var img = $('frontQuickPreview');
    if (!img || !game) return;
    var token = (state.quickPreviewToken || 0) + 1;
    state.quickPreviewToken = token;
    var size = frontCanvasSize(game);
    var fit = $('frontQuickFit') ? $('frontQuickFit').value : 'contain';
    var template = fetch('/api/rom/front/template?rom=' + encodePath(game.rom), { cache: 'no-store' })
      .then(function (response) {
        if (!response.ok) throw new Error('HTTP ' + response.status);
        return response.blob();
      })
      .then(frontQuickDataUrl)
      .then(frontQuickImage);
    var cover = fetch('/api/rom/cover?rom=' + encodePath(game.rom), { cache: 'no-store' })
      .then(function (response) {
        if (!response.ok) return null;
        return response.blob();
      })
      .then(function (blob) { return blob ? frontQuickDataUrl(blob) : null; })
      .then(function (src) { return src ? frontQuickImage(src) : null; })
      .catch(function () { return null; });
    Promise.all([template, cover])
      .then(function (result) {
        if (state.quickPreviewToken !== token || state.frontEditor) return;
        var shell = result[0];
        var artwork = result[1];
        var canvas = document.createElement('canvas');
        canvas.width = size[0];
        canvas.height = size[1];
        var ctx = canvas.getContext('2d');
        ctx.drawImage(shell, 0, 0, size[0], size[1]);
        if (artwork) {
          ctx.drawImage(frontQuickSticker(game, artwork, fit), 0, 0);
        }
        img.src = canvas.toDataURL('image/png');
      })
      .catch(function () {
        if (state.quickPreviewToken === token) img.removeAttribute('src');
      });
  }

  function frontQuickProject(game, name, sticker, model) {
    var size = frontCanvasSize(game);
    var project = frontNewProject(game);
    project.name = name;
    project.model = model;
    project.background = 'rgba(0,0,0,0)';
    project.layers = [{
      id: frontNewId('img'),
      type: 'image',
      name: name,
      src: sticker,
      x: 0,
      y: 0,
      w: size[0],
      h: size[1],
      fit: 'stretch',
      rotation: 0,
      opacity: 1,
      visible: true,
      locked: false
    }];
    return project;
  }

  function frontQuickSave(game, name, sticker, model) {
    var project = frontQuickProject(game, name, sticker, model);
    var query = '?rom=' + encodePath(game.rom) +
      '&name=' + encodeURIComponent(name) +
      '&model=' + encodeURIComponent(model);
    return fetch('/api/rom/fronts/project' + query, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json', 'X-Slot-Web': '1' },
      body: JSON.stringify({ revision: 0, project: project, render: sticker })
    }).then(function (response) {
      return response.json().then(function (body) {
        if (!response.ok) throw new Error((body && body.error) || 'HTTP ' + response.status);
        return body;
      });
    });
  }

  function frontQuickSelect(game, id) {
    return fetch('/api/rom/fronts/select?rom=' + encodePath(game.rom) + '&id=' + encodeURIComponent(id), {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json', 'X-Slot-Web': '1' },
      body: '{}'
    }).then(function (response) {
      if (!response.ok) throw new Error('HTTP ' + response.status);
    });
  }

  function frontQuickRemaining(game) {
    if (state.frontCountRom !== game.rom) return 5;
    return Math.max(0, 5 - (state.frontCount || 0));
  }

  function frontQuickBusy(busy) {
    state.frontQuickBusy = !!busy;
    var game = state.selectedRom;
    var remaining = game ? frontQuickRemaining(game) : 5;
    ['frontQuickCover', 'frontQuickBatch'].forEach(function (id) {
      var button = $(id);
      if (button) button.disabled = !!busy || remaining <= 0;
    });
  }

  function frontQuickUpdateCount() {
    var count = state.frontCountRom === (state.selectedRom && state.selectedRom.rom)
      ? (state.frontCount || 0) : 0;
    var label = $('frontQuickCount');
    if (label) label.textContent = count + ' / 5';
    frontQuickBusy(state.frontQuickBusy || false);
  }

  function frontQuickFinish(game, count) {
    frontQuickBusy(false);
    loadFronts(game);
    loadRoms();
    if (count > 1) toast(t('web.rom.quickFrontBatchDone', { count: count }));
    else toast(t('web.rom.quickFrontCreated'));
  }

  function frontQuickCreateOne(game, image, name, fit, model) {
    var sticker = frontQuickSticker(game, image, fit).toDataURL('image/png');
    return frontQuickSave(game, name, sticker, model);
  }

  function frontQuickAddCover(game) {
    if (!game || state.frontQuickBusy) return;
    if (!isWritable()) {
      toast(t('web.rom.frontReadonly'), 'warn');
      return;
    }
    if (frontQuickRemaining(game) <= 0) {
      toast(t('web.rom.quickFrontLimit'), 'warn');
      return;
    }
    frontQuickBusy(true);
    var fit = $('frontQuickFit') ? $('frontQuickFit').value : 'contain';
    var model = frontQuickModel(game);
    var name = (t('web.rom.quickFrontName') + ' - ' + (game.stem || '')).slice(0, 80);
    fetch('/api/rom/cover?rom=' + encodePath(game.rom), { cache: 'no-store' })
      .then(function (response) {
        if (response.status === 404) throw new Error(t('web.rom.quickFrontNoCover'));
        if (!response.ok) throw new Error('HTTP ' + response.status);
        return response.blob();
      })
      .then(frontQuickDataUrl)
      .then(frontQuickImage)
      .then(function (image) {
        return frontQuickCreateOne(game, image, name, fit, model);
      })
      .then(function (data) {
        return frontQuickSelect(game, data.id).then(function () {
          frontQuickFinish(game, 1);
        });
      })
      .catch(function (error) {
        frontQuickBusy(false);
        showError(error, t('web.rom.quickFrontFailed'));
      });
  }

  function frontQuickAddFiles(game, files) {
    if (!game || state.frontQuickBusy) return;
    if (!isWritable()) {
      toast(t('web.rom.frontReadonly'), 'warn');
      return;
    }
    var list = Array.prototype.slice.call(files || []);
    if (!list.length) return;
    var remaining = frontQuickRemaining(game);
    if (remaining <= 0) {
      toast(t('web.rom.quickFrontLimit'), 'warn');
      return;
    }
    if (list.length > remaining) {
      list = list.slice(0, remaining);
      toast(t('web.rom.quickFrontLimit'), 'warn');
    }
    var fit = $('frontQuickFit') ? $('frontQuickFit').value : 'contain';
    var model = frontQuickModel(game);
    var created = 0;
    var lastId = null;
    var failed = 0;
    frontQuickBusy(true);
    var chain = Promise.resolve();
    list.forEach(function (file) {
      chain = chain.then(function () {
        return frontQuickDataUrl(file)
          .then(frontQuickImage)
          .then(function (image) {
            var name = file.name.replace(/\.[^.]+$/, '').slice(0, 80) || t('web.rom.quickFrontName');
            return frontQuickCreateOne(game, image, name, fit, model);
          })
          .then(function (data) {
            created += 1;
            lastId = data.id;
          })
          .catch(function () { failed += 1; });
      });
    });
    chain.then(function () {
      if (lastId) {
        return frontQuickSelect(game, lastId).then(function () {
          frontQuickFinish(game, created);
        });
      }
      frontQuickBusy(false);
      if (failed) toast(t('web.rom.quickFrontFailed'), 'error');
    });
  }

  function frontBlob(file, game) {
    return new Promise(function (resolve, reject) {
      var size = frontCanvasSize(game);
      var url = URL.createObjectURL(file);
      var image = new Image();
      image.onload = function () {
        try {
          var canvas = document.createElement('canvas');
          canvas.width = size[0];
          canvas.height = size[1];
          var ctx = canvas.getContext('2d');
          var scale = Math.min(size[0] / image.width, size[1] / image.height);
          var w = Math.round(image.width * scale);
          var h = Math.round(image.height * scale);
          ctx.drawImage(image, Math.round((size[0] - w) / 2), Math.round((size[1] - h) / 2), w, h);
          URL.revokeObjectURL(url);
          canvas.toBlob(function (blob) {
            if (blob) resolve(blob);
            else reject(new Error('PNG conversion failed'));
          }, 'image/png');
        } catch (error) {
          URL.revokeObjectURL(url);
          reject(error);
        }
      };
      image.onerror = function () {
        URL.revokeObjectURL(url);
        reject(new Error('image decode failed'));
      };
      image.src = url;
    });
  }

  function uploadFront(game) {
    if (!isWritable()) {
      toast(t('web.rom.frontReadonly'), 'warn');
      return;
    }
    var input = $('frontFile');
    if (!input || !input.files || !input.files[0]) return;
    var file = input.files[0];
    input.value = '';
    var name = window.prompt(t('web.rom.frontNamePrompt'), file.name.replace(/\.[^.]+$/, ''));
    if (name === null) return;
    name = name.trim();
    if (!name) return;
    var model = $('frontModel') && !$('frontModel').hidden ? $('frontModel').value : 'any';
    var query =
      '?rom=' + encodePath(game.rom) +
      '&name=' + encodeURIComponent(name) +
      '&model=' + encodeURIComponent(model);
    frontBlob(file, game)
      .then(function (blob) {
        return fetch('/api/rom/fronts' + query, {
          method: 'POST',
          headers: { 'Content-Type': 'image/png', 'X-Slot-Web': '1' },
          body: blob
        });
      })
      .then(function (response) {
        return response.json().then(function (body) {
          if (!response.ok) throw new Error((body && body.error) || 'HTTP ' + response.status);
          return body;
        });
      })
      .then(function () {
        toast(t('web.rom.frontSaved'));
        loadFronts(game);
        loadRoms();
      })
      .catch(function (error) {
        showError(error, t('web.rom.frontUploadFailed'));
      });
  }

  // ---------------------------------------------------------- front editor
  function frontNewProject(game) {
    var size = frontCanvasSize(game);
    var model = (game.platform || '') === 'FC'
      ? (game.model === 'nes' ? 'nes' : 'famicom')
      : 'any';
    return {
      schema: 1,
      revision: 0,
      canvas: { width: size[0], height: size[1] },
      platform: game.platform,
      model: model,
      name: (game.stem || '') + ' 前盖',
      background: '#202028',
      layers: []
    };
  }

  function frontNewId(prefix) {
    return prefix + '-' + Date.now().toString(36) + '-' + Math.floor(Math.random() * 10000).toString(36);
  }

  function frontRemember(editor) {
    if (!editor) return;
    var snapshot = JSON.stringify(editor.project);
    if (editor.historyIndex >= 0 && editor.history[editor.historyIndex] === snapshot) return;
    editor.history = editor.history.slice(0, editor.historyIndex + 1);
    editor.history.push(snapshot);
    if (editor.history.length > 30) editor.history.shift();
    editor.historyIndex = editor.history.length - 1;
  }

  function frontRestore(editor, snapshot) {
    try {
      editor.project = JSON.parse(snapshot);
      editor.selected = null;
      editor.dirty = true;
      editor.images = {};
      editor.project.layers.forEach(function (layer) {
        if (layer.type === 'image' || layer.type === 'label') {
          frontLoadLayerImage(editor, layer);
        }
      });
      renderFrontEditor(editor);
    } catch (error) {
      showError(error, t('web.frontEditor.loadFailed'));
    }
  }

  function frontUndo() {
    var editor = state.frontEditor;
    if (!editor || editor.historyIndex <= 0) return;
    editor.historyIndex -= 1;
    frontRestore(editor, editor.history[editor.historyIndex]);
  }

  function frontRedo() {
    var editor = state.frontEditor;
    if (!editor || editor.historyIndex >= editor.history.length - 1) return;
    editor.historyIndex += 1;
    frontRestore(editor, editor.history[editor.historyIndex]);
  }

  function frontLoadLayerImage(editor, layer) {
    if (!layer || !layer.src) return;
    if (editor.images[layer.id]) return;
    var image = new Image();
    image.onload = function () {
      editor.images[layer.id] = image;
      renderFrontEditor(editor);
    };
    image.onerror = function () {
      toast(t('web.frontEditor.imageFailed'), 'error');
    };
    image.src = layer.src;
  }

  function frontContainRect(image, canvas) {
    var scale = Math.min(canvas.width / image.width, canvas.height / image.height);
    var width = Math.max(1, Math.round(image.width * scale));
    var height = Math.max(1, Math.round(image.height * scale));
    return {
      x: Math.round((canvas.width - width) / 2),
      y: Math.round((canvas.height - height) / 2),
      w: width,
      h: height
    };
  }

  function frontAddImageLayer(editor, src, image, name, locked) {
    var rect = frontContainRect(image, editor.project.canvas);
    frontRemember(editor);
    var layer = {
      id: frontNewId('img'),
      type: 'image',
      name: name || t('web.frontEditor.imageLayer'),
      src: src,
      x: rect.x,
      y: rect.y,
      w: rect.w,
      h: rect.h,
      fit: 'contain',
      rotation: 0,
      opacity: 1,
      visible: true,
      locked: !!locked
    };
    editor.images[layer.id] = image;
    editor.scaleBases[layer.id] = { w: layer.w, h: layer.h };
    editor.project.layers.push(layer);
    editor.selected = layer.id;
    editor.dirty = true;
    renderFrontEditor(editor);
  }

  function frontAddShapeLayer(editor, kind) {
    frontRemember(editor);
    var canvas = editor.project.canvas;
    var width = kind === 'line' ? Math.round(canvas.width * 0.55) : Math.round(canvas.width * 0.28);
    var height = kind === 'line' ? 0 : Math.round(canvas.height * 0.22);
    var layer = {
      id: frontNewId('shape'),
      type: 'shape',
      name: t('web.frontEditor.shape_' + kind),
      kind: kind,
      x: Math.round((canvas.width - width) / 2),
      y: Math.round((canvas.height - height) / 2),
      w: width,
      h: height,
      colour: '#00b894',
      stroke: '#00b894',
      stroke_width: kind === 'line' ? 3 : 0,
      fill: kind !== 'line',
      rotation: 0,
      opacity: 1,
      visible: true,
      locked: false
    };
    editor.project.layers.push(layer);
    editor.selected = layer.id;
    editor.dirty = true;
    renderFrontEditor(editor);
  }

  function frontApplyTemplate(editor, kind) {
    frontRemember(editor);
    var canvas = editor.project.canvas;
    if (kind === 'title') {
      editor.project.background = {
        type: 'linear',
        from: '#1b2330',
        to: '#080b12',
        angle: 90
      };
      var text = String(editor.game.stem || editor.project.name || '').slice(0, 40);
      editor.project.layers.push({
        id: frontNewId('text'),
        type: 'text',
        name: t('web.frontEditor.titleTemplateLayer'),
        text: text,
        x: Math.round(canvas.width * 0.08),
        y: Math.round(canvas.height * 0.68),
        w: Math.round(canvas.width * 0.84),
        h: 44,
        size: Math.max(16, Math.round(canvas.height / 13)),
        weight: 800,
        colour: '#f1eddf',
        stroke: '#080b12',
        stroke_width: 2,
        align: 'center',
        rotation: 0,
        opacity: 1,
        visible: true,
        locked: false
      });
    } else if (kind === 'frame') {
      editor.project.background = '#101014';
      editor.project.layers.push({
        id: frontNewId('shape'),
        type: 'shape',
        name: t('web.frontEditor.frameTemplateLayer'),
        kind: 'rect',
        x: 8,
        y: 8,
        w: canvas.width - 16,
        h: canvas.height - 16,
        colour: '#00b894',
        stroke: '#00b894',
        stroke_width: 4,
        fill: false,
        rotation: 0,
        opacity: 1,
        visible: true,
        locked: false
      });
    }
    editor.selected = null;
    editor.dirty = true;
    renderFrontEditor(editor);
  }

  function frontAddTextLayer(editor) {
    var text = window.prompt(t('web.frontEditor.textPrompt'), t('web.frontEditor.defaultText'));
    if (text === null) return;
    text = text.trim();
    if (!text) return;
    frontRemember(editor);
    var canvas = editor.project.canvas;
    var size = Math.max(16, Math.round(canvas.height / 11));
    var layer = {
      id: frontNewId('text'),
      type: 'text',
      name: t('web.frontEditor.textLayer'),
      text: text,
      x: Math.round(canvas.width * 0.16),
      y: Math.round(canvas.height * 0.55),
      w: Math.round(canvas.width * 0.68),
      h: Math.round(size * 1.4),
      size: size,
      weight: 700,
      colour: '#f1eddf',
      stroke: '#101014',
      stroke_width: 2,
      align: 'center',
      rotation: 0,
      opacity: 1,
      visible: true,
      locked: false
    };
    editor.project.layers.push(layer);
    editor.selected = layer.id;
    editor.dirty = true;
    renderFrontEditor(editor);
  }

  function frontSelectedLayer(editor) {
    if (!editor || !editor.selected) return null;
    for (var i = 0; i < editor.project.layers.length; i++) {
      if (editor.project.layers[i].id === editor.selected) return editor.project.layers[i];
    }
    return null;
  }

  function frontLoadImages(editor) {
    if (!editor.scaleBases) editor.scaleBases = {};
    editor.project.layers.forEach(function (layer) {
      if (layer.type === 'image' || layer.type === 'label') {
        if (!editor.scaleBases[layer.id]) {
          editor.scaleBases[layer.id] = {
            w: Math.max(1, Number(layer.w) || 1),
            h: Math.max(1, Number(layer.h) || 1)
          };
        }
        frontLoadLayerImage(editor, layer);
      }
    });
  }

  function frontLoadTemplate(editor) {
    fetch('/api/rom/front/template?rom=' + encodePath(editor.game.rom), { cache: 'no-store' })
      .then(function (response) {
        if (!response.ok) throw new Error('HTTP ' + response.status);
        return response.blob();
      })
      .then(function (blob) {
        frontFileToDataUrl(blob, function (src) {
          var image = new Image();
          image.onload = function () {
            if (state.frontEditor !== editor) return;
            editor.templateImage = image;
            renderFrontEditor(editor);
          };
          image.onerror = function () {
            if (state.frontEditor === editor) toast(t('web.frontEditor.templateFailed'), 'error');
          };
          image.src = src;
        });
      })
      .catch(function () {
        if (state.frontEditor === editor) toast(t('web.frontEditor.templateFailed'), 'error');
      });
  }

  function frontApplyMask(ctx, editor) {
    if (!editor.templateImage || !editor.templateImage.complete) return false;
    var canvas = editor.project.canvas;
    var mask = document.createElement('canvas');
    mask.width = canvas.width;
    mask.height = canvas.height;
    var maskContext = mask.getContext('2d');
    maskContext.drawImage(editor.templateImage, 0, 0, canvas.width, canvas.height);
    ctx.save();
    ctx.globalCompositeOperation = 'destination-in';
    ctx.drawImage(mask, 0, 0);
    ctx.restore();
    return true;
  }

  function frontRenderContent(editor) {
    var canvas = document.createElement('canvas');
    canvas.width = editor.project.canvas.width;
    canvas.height = editor.project.canvas.height;
    var ctx = canvas.getContext('2d');
    frontDrawLayers(ctx, editor);
    frontApplyMask(ctx, editor);
    return canvas;
  }

  function frontGuideRect(game) {
    var platform = (game && game.platform) || 'GBA';
    if (platform === 'GBA') return [25, 20, 310, 160];
    if (platform === 'GB' || platform === 'GBC') return [30, 32, 180, 180];
    if (platform === 'FC') {
      if (game.model === 'nes') return [154, 2, 121, 200];
      return [31, 31, 296, 202];
    }
    if (platform === 'SFC') return [80, 30, 200, 190];
    if (platform === 'MD') return [50, 20, 260, 200];
    return [20, 20, 320, 240];
  }

  function frontBackgroundFields(editor) {
    var background = editor.project.background;
    if (background && typeof background === 'object') {
      return {
        gradient: true,
        from: background.from || '#202028',
        to: background.to || '#08080c',
        angle: Number(background.angle) || 90
      };
    }
    return {
      gradient: false,
      from: background || '#202028',
      to: '#08080c',
      angle: 90
    };
  }

  function frontSetBackgroundControls(editor) {
    var fields = frontBackgroundFields(editor);
    if ($('frontBackground')) $('frontBackground').value = fields.from;
    if ($('frontGradient')) $('frontGradient').checked = fields.gradient;
    if ($('frontBackground2')) $('frontBackground2').value = fields.to;
    if ($('frontGradientAngle')) $('frontGradientAngle').value = String(fields.angle);
  }

  function frontPaintBackground(ctx, editor) {
    var canvas = editor.project.canvas;
    var background = editor.project.background;
    var fields = frontBackgroundFields(editor);
    if (background && typeof background === 'object') {
      var angle = (fields.angle || 90) * Math.PI / 180;
      var cx = canvas.width / 2;
      var cy = canvas.height / 2;
      var half = Math.max(canvas.width, canvas.height);
      var dx = Math.cos(angle) * half;
      var dy = Math.sin(angle) * half;
      var gradient = ctx.createLinearGradient(cx - dx, cy - dy, cx + dx, cy + dy);
      gradient.addColorStop(0, fields.from);
      gradient.addColorStop(1, fields.to);
      ctx.fillStyle = gradient;
      ctx.fillRect(0, 0, canvas.width, canvas.height);
    } else {
      ctx.fillStyle = fields.from;
      ctx.fillRect(0, 0, canvas.width, canvas.height);
    }
  }

  function frontDrawLayers(ctx, editor) {
    var canvas = editor.project.canvas;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    frontPaintBackground(ctx, editor);
    editor.project.layers.forEach(function (layer) {
      if (layer.visible === false) return;
      var alpha = typeof layer.opacity === 'number' ? layer.opacity : 1;
      ctx.save();
      ctx.globalAlpha = Math.max(0, Math.min(1, alpha));
      var rotation = Number(layer.rotation) || 0;
      if (rotation) {
        var centreX = Number(layer.x) + (Number(layer.w) || 0) / 2;
        var centreY = Number(layer.y) + (Number(layer.h) || 0) / 2;
        ctx.translate(centreX, centreY);
        ctx.rotate(rotation * Math.PI / 180);
        ctx.translate(-centreX, -centreY);
      }
      if (layer.type === 'image' || layer.type === 'label') {
        var image = editor.images[layer.id];
        if (image && image.complete) {
          ctx.drawImage(image, layer.x, layer.y, layer.w, layer.h);
        }
      } else if (layer.type === 'text') {
        var size = Math.max(6, Number(layer.size) || 24);
        ctx.font = (layer.weight || 700) + ' ' + size + 'px ' + (layer.font || 'system-ui, sans-serif');
        ctx.textAlign = layer.align || 'center';
        ctx.textBaseline = 'top';
        var lines = String(layer.text || '').split('\n');
        var lineHeight = size * 1.18;
        lines.forEach(function (line, index) {
          var tx = layer.x;
          if (ctx.textAlign === 'center') tx = layer.x + layer.w / 2;
          if (ctx.textAlign === 'right') tx = layer.x + layer.w;
          var ty = layer.y + index * lineHeight;
          if (layer.stroke_width > 0 && layer.stroke) {
            ctx.lineWidth = Number(layer.stroke_width) || 1;
            ctx.strokeStyle = layer.stroke;
            ctx.strokeText(line, tx, ty);
          }
          ctx.fillStyle = layer.colour || '#f1eddf';
          ctx.fillText(line, tx, ty);
        });
      } else if (layer.type === 'shape') {
        var sx = Number(layer.x) || 0;
        var sy = Number(layer.y) || 0;
        var sw = Number(layer.w) || 1;
        var sh = Number(layer.h) || 1;
        var stroke = Number(layer.stroke_width) || 0;
        ctx.fillStyle = layer.colour || '#00d4aa';
        ctx.strokeStyle = layer.stroke || layer.colour || '#00d4aa';
        ctx.lineWidth = Math.max(1, stroke);
        ctx.beginPath();
        if (layer.kind === 'ellipse') {
          ctx.ellipse(sx + sw / 2, sy + sh / 2, Math.abs(sw) / 2, Math.abs(sh) / 2, 0, 0, Math.PI * 2);
          if (layer.fill !== false) ctx.fill();
          if (stroke > 0) ctx.stroke();
        } else if (layer.kind === 'line') {
          ctx.moveTo(sx, sy);
          ctx.lineTo(sx + sw, sy + sh);
          ctx.stroke();
        } else {
          if (layer.fill !== false) ctx.fillRect(sx, sy, sw, sh);
          if (stroke > 0) ctx.strokeRect(sx, sy, sw, sh);
        }
      }
      ctx.restore();
    });
  }

  function renderFrontCanvas(editor) {
    var canvas = $('frontCanvas');
    if (!canvas) return;
    var size = editor.project.canvas;
    if (canvas.width !== size.width || canvas.height !== size.height) {
      canvas.width = size.width;
      canvas.height = size.height;
    }
    var ctx = canvas.getContext('2d');
    ctx.clearRect(0, 0, size.width, size.height);
    var content = frontRenderContent(editor);
    if (editor.showMask !== false && editor.templateImage) {
      ctx.save();
      ctx.globalAlpha = 0.55;
      ctx.drawImage(editor.templateImage, 0, 0, size.width, size.height);
      ctx.restore();
    }
    ctx.drawImage(content, 0, 0);
    if (editor.showGuides) {
      var guide = frontGuideRect(editor.game);
      ctx.save();
      ctx.setLineDash([4, 4]);
      ctx.strokeStyle = 'rgba(255, 210, 90, 0.8)';
      ctx.lineWidth = 1;
      ctx.strokeRect(guide[0], guide[1], guide[2], guide[3]);
      ctx.restore();
    }
    var selected = frontSelectedLayer(editor);
    if (selected && selected.type !== 'background') {
      // A full-bleed image's selection rectangle sits on the canvas edge and
      // visually competes with the cartridge outline.  The layer list already
      // shows what is selected, so keep the artwork and its contour clear.
      var bleed = selected.type === 'image' &&
        selected.x <= 10 && selected.y <= 10 &&
        selected.x + (selected.w || 0) >= size.width - 10 &&
        selected.y + (selected.h || 0) >= size.height - 10;
      if (!bleed) {
        ctx.save();
        ctx.strokeStyle = '#00d4aa';
        ctx.lineWidth = 1;
        var centreX = selected.x + (selected.w || 0) / 2;
        var centreY = selected.y + (selected.h || 0) / 2;
        if (selected.rotation) {
          ctx.translate(centreX, centreY);
          ctx.rotate(Number(selected.rotation) * Math.PI / 180);
          ctx.translate(-centreX, -centreY);
        }
        ctx.strokeRect(selected.x - 1, selected.y - 1, (selected.w || 0) + 2, (selected.h || 0) + 2);
        ctx.restore();
      }
    }
  }

  function renderFrontLayers(editor) {
    var box = $('frontLayerList');
    if (!box) return;
    box.textContent = '';
    if (!editor.project.layers.length) {
      var empty = document.createElement('p');
      empty.className = 'hint';
      empty.textContent = t('web.frontEditor.noLayers');
      box.appendChild(empty);
      return;
    }
    var layers = editor.project.layers.slice().reverse();
    layers.forEach(function (layer) {
      var row = document.createElement('div');
      row.className = 'front-layer-row' + (layer.id === editor.selected ? ' selected' : '');
      row.addEventListener('click', function () {
        editor.selected = layer.id;
        renderFrontEditor(editor);
      });
      var kind = document.createElement('span');
      kind.className = 'front-layer-kind';
      kind.textContent = layer.type === 'text' ? 'T' : '▣';
      row.appendChild(kind);
      var name = document.createElement('span');
      name.className = 'front-layer-name';
      name.textContent = layer.name || layer.type;
      row.appendChild(name);
      var eye = document.createElement('button');
      eye.type = 'button';
      eye.textContent = layer.visible === false ? '○' : '●';
      eye.title = t('web.frontEditor.visible');
      eye.addEventListener('click', function (event) {
        event.stopPropagation();
        frontRemember(editor);
        layer.visible = layer.visible === false;
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      row.appendChild(eye);
      var lock = document.createElement('button');
      lock.type = 'button';
      lock.textContent = layer.locked ? '🔒' : '🔓';
      lock.title = t('web.frontEditor.locked');
      lock.addEventListener('click', function (event) {
        event.stopPropagation();
        frontRemember(editor);
        layer.locked = !layer.locked;
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      row.appendChild(lock);
      var duplicate = document.createElement('button');
      duplicate.type = 'button';
      duplicate.textContent = '⧉';
      duplicate.title = t('web.frontEditor.duplicate');
      duplicate.addEventListener('click', function (event) {
        event.stopPropagation();
        frontDuplicateLayer(editor, layer);
      });
      row.appendChild(duplicate);
      var up = document.createElement('button');
      up.type = 'button';
      up.textContent = '↑';
      up.title = t('web.frontEditor.up');
      up.addEventListener('click', function (event) {
        event.stopPropagation();
        frontReorderLayer(editor, layer, 1);
      });
      row.appendChild(up);
      var down = document.createElement('button');
      down.type = 'button';
      down.textContent = '↓';
      down.title = t('web.frontEditor.down');
      down.addEventListener('click', function (event) {
        event.stopPropagation();
        frontReorderLayer(editor, layer, -1);
      });
      row.appendChild(down);
      var remove = document.createElement('button');
      remove.type = 'button';
      remove.textContent = '×';
      remove.title = t('web.frontEditor.delete');
      remove.addEventListener('click', function (event) {
        event.stopPropagation();
        frontDeleteLayer(editor, layer);
      });
      row.appendChild(remove);
      box.appendChild(row);
    });
  }

  function frontReorderLayer(editor, layer, direction) {
    var index = editor.project.layers.indexOf(layer);
    var target = index + direction;
    if (index < 0 || target < 0 || target >= editor.project.layers.length) return;
    frontRemember(editor);
    editor.project.layers.splice(index, 1);
    editor.project.layers.splice(target, 0, layer);
    editor.dirty = true;
    renderFrontEditor(editor);
  }

  function frontDuplicateLayer(editor, layer) {
    frontRemember(editor);
    var copy = JSON.parse(JSON.stringify(layer));
    copy.id = frontNewId(copy.type === 'shape' ? 'shape' : copy.type === 'text' ? 'text' : 'img');
    copy.name = (layer.name || layer.type) + ' ' + t('web.frontEditor.copySuffix');
    copy.x = (Number(layer.x) || 0) + 8;
    copy.y = (Number(layer.y) || 0) + 8;
    if (layer.type === 'image' || layer.type === 'label') {
      var image = editor.images[layer.id];
      if (image) editor.images[copy.id] = image;
      editor.scaleBases[copy.id] = { w: copy.w, h: copy.h };
    }
    editor.project.layers.push(copy);
    editor.selected = copy.id;
    editor.dirty = true;
    renderFrontEditor(editor);
  }

  function frontDeleteLayer(editor, layer) {
    frontRemember(editor);
    editor.project.layers = editor.project.layers.filter(function (item) {
      return item.id !== layer.id;
    });
    if (editor.selected === layer.id) editor.selected = null;
    editor.dirty = true;
    renderFrontEditor(editor);
  }

  function frontField(labelText, input) {
    var label = document.createElement('label');
    var span = document.createElement('span');
    span.textContent = labelText;
    label.appendChild(span);
    label.appendChild(input);
    return label;
  }

  function frontNumber(editor, layer, key, labelText) {
    var input = document.createElement('input');
    input.type = 'number';
    input.step = '1';
    input.value = Math.round(Number(layer[key]) || 0);
    input.addEventListener('change', function () {
      frontRemember(editor);
      layer[key] = Number(input.value) || 0;
      if ((key === 'w' || key === 'h') && editor.scaleBases && editor.scaleBases[layer.id]) {
        editor.scaleBases[layer.id] = {
          w: Math.max(1, Number(layer.w) || 1),
          h: Math.max(1, Number(layer.h) || 1)
        };
      }
      editor.dirty = true;
      renderFrontEditor(editor);
    });
    return frontField(labelText, input);
  }

  function frontScaleField(editor, layer) {
    if (!editor.scaleBases) editor.scaleBases = {};
    var base = editor.scaleBases[layer.id];
    if (!base) {
      base = {
        w: Math.max(1, Number(layer.w) || 1),
        h: Math.max(1, Number(layer.h) || 1)
      };
      editor.scaleBases[layer.id] = base;
    }
    var group = document.createElement('div');
    group.className = 'front-scale-group';

    var rows = {};
    var currentPercent = function (value, total) {
      return Math.round(Math.max(1, Number(value) || 1) / Math.max(1, Number(total) || 1) * 100);
    };
    var updateReadouts = function () {
      rows.scale.range.value = String(Math.max(10, Math.min(500, currentPercent(layer.w, base.w))));
      rows.scale.value.textContent = rows.scale.range.value + '%';
      rows.x.range.value = String(Math.max(10, Math.min(500, currentPercent(layer.w, base.w))));
      rows.x.value.textContent = rows.x.range.value + '%';
      rows.y.range.value = String(Math.max(10, Math.min(500, currentPercent(layer.h, base.h))));
      rows.y.value.textContent = rows.y.range.value + '%';
    };
    var addRow = function (key, labelKey, onChange) {
      var wrap = document.createElement('label');
      var head = document.createElement('span');
      head.className = 'front-scale-head';
      var title = document.createElement('span');
      title.textContent = t(labelKey);
      var value = document.createElement('b');
      var range = document.createElement('input');
      range.type = 'range';
      range.min = '10';
      range.max = '500';
      range.step = '1';
      head.appendChild(title);
      head.appendChild(value);
      wrap.appendChild(head);
      wrap.appendChild(range);
      range.addEventListener('pointerdown', function () { frontRemember(editor); });
      range.addEventListener('focus', function () { frontRemember(editor); });
      range.addEventListener('input', function () {
        var scale = Math.max(10, Math.min(500, Number(range.value) || 100)) / 100;
        onChange(scale);
        value.textContent = Math.round(scale * 100) + '%';
        updateReadouts();
        editor.dirty = true;
        renderFrontCanvas(editor);
      });
      range.addEventListener('change', function () {
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      group.appendChild(wrap);
      rows[key] = { range: range, value: value };
    };

    addRow('scale', 'web.frontEditor.layerScale', function (scale) {
      var cx = (Number(layer.x) || 0) + (Number(layer.w) || 0) / 2;
      var cy = (Number(layer.y) || 0) + (Number(layer.h) || 0) / 2;
      layer.w = Math.max(8, Math.round(base.w * scale));
      layer.h = Math.max(8, Math.round(base.h * scale));
      layer.x = Math.round(cx - layer.w / 2);
      layer.y = Math.round(cy - layer.h / 2);
    });
    addRow('x', 'web.frontEditor.stretchX', function (scale) {
      var cx = (Number(layer.x) || 0) + (Number(layer.w) || 0) / 2;
      layer.w = Math.max(8, Math.round(base.w * scale));
      layer.x = Math.round(cx - layer.w / 2);
    });
    addRow('y', 'web.frontEditor.stretchY', function (scale) {
      var cy = (Number(layer.y) || 0) + (Number(layer.h) || 0) / 2;
      layer.h = Math.max(8, Math.round(base.h * scale));
      layer.y = Math.round(cy - layer.h / 2);
    });
    updateReadouts();
    return group;
  }

  function renderFrontInspector(editor) {
    var box = $('frontInspector');
    if (!box) return;
    box.textContent = '';
    var layer = frontSelectedLayer(editor);
    if (!layer) {
      var empty = document.createElement('p');
      empty.className = 'front-inspector-empty';
      empty.textContent = t('web.frontEditor.pickLayer');
      box.appendChild(empty);
      return;
    }
    var name = document.createElement('input');
    name.type = 'text';
    name.maxLength = 80;
    name.value = layer.name || '';
    name.addEventListener('change', function () {
      frontRemember(editor);
      layer.name = name.value;
      editor.dirty = true;
      renderFrontEditor(editor);
    });
    box.appendChild(frontField(t('web.frontEditor.layerName'), name));
    if (layer.type === 'image' || layer.type === 'label') {
      var fit = document.createElement('select');
      ['contain', 'cover', 'stretch'].forEach(function (key) {
        var option = document.createElement('option');
        option.value = key;
        option.textContent = t('web.frontEditor.fit_' + key);
        if ((layer.fit || 'contain') === key) option.selected = true;
        fit.appendChild(option);
      });
      fit.addEventListener('change', function () {
        frontRemember(editor);
        layer.fit = fit.value;
        frontApplyFit(editor, layer);
        if (editor.scaleBases) {
          editor.scaleBases[layer.id] = {
            w: Math.max(1, Number(layer.w) || 1),
            h: Math.max(1, Number(layer.h) || 1)
          };
        }
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      box.appendChild(frontField(t('web.frontEditor.fit'), fit));
      var grid = document.createElement('div');
      grid.className = 'front-grid-2';
      grid.appendChild(frontNumber(editor, layer, 'x', 'X'));
      grid.appendChild(frontNumber(editor, layer, 'y', 'Y'));
      box.appendChild(grid);
      var grid2 = document.createElement('div');
      grid2.className = 'front-grid-2';
      grid2.appendChild(frontNumber(editor, layer, 'w', 'W'));
      grid2.appendChild(frontNumber(editor, layer, 'h', 'H'));
      box.appendChild(grid2);
      box.appendChild(frontScaleField(editor, layer));
    } else if (layer.type === 'text') {
      var text = document.createElement('textarea');
      text.maxLength = 160;
      text.value = layer.text || '';
      text.addEventListener('change', function () {
        frontRemember(editor);
        layer.text = text.value;
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      box.appendChild(frontField(t('web.frontEditor.text'), text));
      var size = document.createElement('input');
      size.type = 'number';
      size.min = '6';
      size.max = '300';
      size.value = layer.size || 24;
      size.addEventListener('change', function () {
        frontRemember(editor);
        layer.size = Number(size.value) || 24;
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      box.appendChild(frontField(t('web.frontEditor.size'), size));
      var colour = document.createElement('input');
      colour.type = 'color';
      colour.value = layer.colour || '#f1eddf';
      colour.addEventListener('change', function () {
        frontRemember(editor);
        layer.colour = colour.value;
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      box.appendChild(frontField(t('web.frontEditor.textColour'), colour));
      var weight = document.createElement('select');
      [400, 700, 800].forEach(function (value) {
        var option = document.createElement('option');
        option.value = String(value);
        option.textContent = String(value);
        if (Number(layer.weight || 700) === value) option.selected = true;
        weight.appendChild(option);
      });
      weight.addEventListener('change', function () {
        frontRemember(editor);
        layer.weight = Number(weight.value) || 700;
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      box.appendChild(frontField(t('web.frontEditor.weight'), weight));
      var align = document.createElement('select');
      ['left', 'center', 'right'].forEach(function (value) {
        var option = document.createElement('option');
        option.value = value;
        option.textContent = t('web.frontEditor.align_' + value);
        if ((layer.align || 'center') === value) option.selected = true;
        align.appendChild(option);
      });
      align.addEventListener('change', function () {
        frontRemember(editor);
        layer.align = align.value;
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      box.appendChild(frontField(t('web.frontEditor.align'), align));
      var textStroke = document.createElement('input');
      textStroke.type = 'color';
      textStroke.value = layer.stroke || '#101014';
      textStroke.addEventListener('change', function () {
        frontRemember(editor);
        layer.stroke = textStroke.value;
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      box.appendChild(frontField(t('web.frontEditor.textStroke'), textStroke));
      var textStrokeWidth = document.createElement('input');
      textStrokeWidth.type = 'number';
      textStrokeWidth.min = '0';
      textStrokeWidth.max = '20';
      textStrokeWidth.value = layer.stroke_width || 0;
      textStrokeWidth.addEventListener('change', function () {
        frontRemember(editor);
        layer.stroke_width = Math.max(0, Number(textStrokeWidth.value) || 0);
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      box.appendChild(frontField(t('web.frontEditor.strokeWidth'), textStrokeWidth));
      var grid = document.createElement('div');
      grid.className = 'front-grid-2';
      grid.appendChild(frontNumber(editor, layer, 'x', 'X'));
      grid.appendChild(frontNumber(editor, layer, 'y', 'Y'));
      box.appendChild(grid);
    } else if (layer.type === 'shape') {
      var kind = document.createElement('select');
      ['rect', 'ellipse', 'line'].forEach(function (value) {
        var option = document.createElement('option');
        option.value = value;
        option.textContent = t('web.frontEditor.shape_' + value);
        if ((layer.kind || 'rect') === value) option.selected = true;
        kind.appendChild(option);
      });
      kind.addEventListener('change', function () {
        frontRemember(editor);
        layer.kind = kind.value;
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      box.appendChild(frontField(t('web.frontEditor.shape'), kind));
      var grid = document.createElement('div');
      grid.className = 'front-grid-2';
      grid.appendChild(frontNumber(editor, layer, 'x', 'X'));
      grid.appendChild(frontNumber(editor, layer, 'y', 'Y'));
      grid.appendChild(frontNumber(editor, layer, 'w', 'W'));
      grid.appendChild(frontNumber(editor, layer, 'h', 'H'));
      box.appendChild(grid);
      var shapeColour = document.createElement('input');
      shapeColour.type = 'color';
      shapeColour.value = layer.colour || '#00b894';
      shapeColour.addEventListener('change', function () {
        frontRemember(editor);
        layer.colour = shapeColour.value;
        layer.stroke = shapeColour.value;
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      box.appendChild(frontField(t('web.frontEditor.shapeColour'), shapeColour));
      var strokeWidth = document.createElement('input');
      strokeWidth.type = 'number';
      strokeWidth.min = '0';
      strokeWidth.max = '20';
      strokeWidth.value = layer.stroke_width || 0;
      strokeWidth.addEventListener('change', function () {
        frontRemember(editor);
        layer.stroke_width = Math.max(0, Number(strokeWidth.value) || 0);
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      box.appendChild(frontField(t('web.frontEditor.strokeWidth'), strokeWidth));
      var fill = document.createElement('label');
      fill.className = 'front-check';
      var fillInput = document.createElement('input');
      fillInput.type = 'checkbox';
      fillInput.checked = layer.fill !== false;
      fillInput.addEventListener('change', function () {
        frontRemember(editor);
        layer.fill = fillInput.checked;
        editor.dirty = true;
        renderFrontEditor(editor);
      });
      var fillText = document.createElement('span');
      fillText.textContent = t('web.frontEditor.fill');
      fill.appendChild(fillInput);
      fill.appendChild(fillText);
      box.appendChild(fill);
    }
    var opacity = document.createElement('input');
    opacity.type = 'range';
    opacity.min = '0';
    opacity.max = '1';
    opacity.step = '0.01';
    opacity.value = String(typeof layer.opacity === 'number' ? layer.opacity : 1);
    opacity.addEventListener('change', function () {
      frontRemember(editor);
      layer.opacity = Number(opacity.value);
      editor.dirty = true;
      renderFrontEditor(editor);
    });
    box.appendChild(frontField(t('web.frontEditor.opacity'), opacity));
    box.appendChild(frontNumber(editor, layer, 'rotation', t('web.frontEditor.rotation')));
  }

  function renderFrontEditor(editor) {
    if (!editor) return;
    var meta = $('frontEditorMeta');
    if (meta) {
      meta.textContent = (editor.game.stem || '') + ' · ' +
        editor.project.canvas.width + ' × ' + editor.project.canvas.height +
        (editor.faceId ? ' · ' + t('web.frontEditor.saved') : ' · ' + t('web.frontEditor.unsaved'));
    }
    $('frontNameInput').value = editor.project.name || editor.name || '';
    frontSetBackgroundControls(editor);
    renderFrontLayers(editor);
    renderFrontInspector(editor);
    renderFrontCanvas(editor);
  }

  function frontApplyFit(editor, layer) {
    var image = editor.images[layer.id];
    var canvas = editor.project.canvas;
    if (!image) return;
    if (layer.fit === 'stretch') {
      layer.x = 0;
      layer.y = 0;
      layer.w = canvas.width;
      layer.h = canvas.height;
      return;
    }
    var scale = layer.fit === 'cover'
      ? Math.max(canvas.width / image.width, canvas.height / image.height)
      : Math.min(canvas.width / image.width, canvas.height / image.height);
    layer.w = Math.max(1, Math.round(image.width * scale));
    layer.h = Math.max(1, Math.round(image.height * scale));
    layer.x = Math.round((canvas.width - layer.w) / 2);
    layer.y = Math.round((canvas.height - layer.h) / 2);
  }

  function frontFileToDataUrl(file, callback) {
    if (file.size > 4 * 1024 * 1024) {
      toast(t('web.frontEditor.imageTooLarge'), 'error');
      return;
    }
    var reader = new FileReader();
    reader.onload = function () { callback(reader.result); };
    reader.onerror = function () { toast(t('web.frontEditor.imageFailed'), 'error'); };
    reader.readAsDataURL(file);
  }

  function frontAddImageFile(editor, file) {
    if (!file) return;
    frontFileToDataUrl(file, function (src) {
      var image = new Image();
      image.onload = function () {
        frontAddImageLayer(editor, src, image, file.name.replace(/\.[^.]+$/, ''));
      };
      image.onerror = function () { toast(t('web.frontEditor.imageFailed'), 'error'); };
      image.src = src;
    });
  }

  /// Insert the bare cartridge shell (the platform/current-model front with no
  /// cover artwork) as the bottom image layer.  It gives users a neutral shell
  /// to paint on, without having to upload an external image first.
  function frontAddShellLayer(editor) {
    if (!editor.templateImage || !editor.templateImage.complete) {
      toast(t('web.frontEditor.templatePending'), 'warn');
      return;
    }
    var canvas = editor.project.canvas;
    var shell = document.createElement('canvas');
    shell.width = canvas.width;
    shell.height = canvas.height;
    var ctx = shell.getContext('2d');
    ctx.drawImage(editor.templateImage, 0, 0, canvas.width, canvas.height);
    var src = shell.toDataURL('image/png');
    var image = new Image();
    image.onload = function () {
      if (state.frontEditor === editor) {
        frontAddImageLayer(editor, src, image, t('web.frontEditor.shellLayer'));
      }
    };
    image.onerror = function () { toast(t('web.frontEditor.imageFailed'), 'error'); };
    image.src = src;
  }

  function frontAddCurrentCover(editor) {
    fetch('/api/rom/cover?rom=' + encodePath(editor.game.rom), { cache: 'no-store' })
      .then(function (response) {
        if (!response.ok) throw new Error('HTTP ' + response.status);
        return response.blob();
      })
      .then(function (blob) {
        frontFileToDataUrl(blob, function (src) {
          var image = new Image();
          image.onload = function () {
            frontAddImageLayer(editor, src, image, t('web.frontEditor.coverLayer'));
          };
          image.onerror = function () { toast(t('web.frontEditor.imageFailed'), 'error'); };
          image.src = src;
        });
      })
      .catch(function () {
        toast(t('web.frontEditor.noCover'), 'warn');
      });
  }

  function frontCanvasPoint(event) {
    var canvas = $('frontCanvas');
    var rect = canvas.getBoundingClientRect();
    return {
      x: (event.clientX - rect.left) * canvas.width / Math.max(1, rect.width),
      y: (event.clientY - rect.top) * canvas.height / Math.max(1, rect.height)
    };
  }

  function frontHitLayer(editor, point) {
    var layers = editor.project.layers;
    for (var i = layers.length - 1; i >= 0; i--) {
      var layer = layers[i];
      if (layer.visible === false || layer.locked) continue;
      if (layer.type !== 'image' && layer.type !== 'label' &&
          layer.type !== 'text' && layer.type !== 'shape') continue;
      var cx = layer.x + (layer.w || 0) / 2;
      var cy = layer.y + (layer.h || 0) / 2;
      var dx = point.x - cx;
      var dy = point.y - cy;
      var angle = -(Number(layer.rotation) || 0) * Math.PI / 180;
      var cos = Math.cos(angle);
      var sin = Math.sin(angle);
      var localX = cx + dx * cos - dy * sin;
      var localY = cy + dx * sin + dy * cos;
      if (localX >= layer.x && localX <= layer.x + layer.w &&
          localY >= layer.y && localY <= layer.y + layer.h) {
        return layer;
      }
    }
    return null;
  }

  function bindFrontCanvas() {
    var canvas = $('frontCanvas');
    if (!canvas) return;
    canvas.addEventListener('pointerdown', function (event) {
      var editor = state.frontEditor;
      if (!editor) return;
      event.preventDefault();
      var point = frontCanvasPoint(event);
      var layer = frontHitLayer(editor, point);
      if (!layer) {
        editor.selected = null;
        renderFrontEditor(editor);
        return;
      }
      editor.selected = layer.id;
      frontRemember(editor);
      editor.drag = { layer: layer, dx: point.x - layer.x, dy: point.y - layer.y };
      renderFrontEditor(editor);
      canvas.setPointerCapture(event.pointerId);
    });
    canvas.addEventListener('pointermove', function (event) {
      var editor = state.frontEditor;
      if (!editor || !editor.drag) return;
      event.preventDefault();
      var point = frontCanvasPoint(event);
      editor.drag.layer.x = Math.round(point.x - editor.drag.dx);
      editor.drag.layer.y = Math.round(point.y - editor.drag.dy);
      editor.dirty = true;
      renderFrontCanvas(editor);
    });
    var release = function () {
      var editor = state.frontEditor;
      if (editor) editor.drag = null;
    };
    canvas.addEventListener('pointerup', release);
    canvas.addEventListener('pointercancel', release);
  }

  function frontFitZoom() {
    var canvas = $('frontCanvas');
    var wrap = document.querySelector('.front-canvas-wrap');
    if (!canvas || !wrap || !canvas.width || !canvas.height) return 1;
    // Leave the stage padding, artboard border/padding and a little breathing
    // room on every side.  The canvas may still be zoomed beyond this and
    // scrolled.
    var width = Math.max(1, wrap.clientWidth - 68);
    var height = Math.max(1, wrap.clientHeight - 68);
    return Math.max(0.4, Math.min(5.0, Math.min(
      width / canvas.width,
      height / canvas.height
    )));
  }

  function setFrontZoom(value) {
    var editor = state.frontEditor;
    if (!editor) return;
    editor.zoom = Math.max(0.4, Math.min(5.0, Number(value) || 1));
    var canvas = $('frontCanvas');
    // Grow the canvas itself and let .front-canvas-wrap scroll.  Previously
    // the width was clipped to the wrapper, and the wrapper was sized by the
    // canvas: after zooming out, the wrapper shrank too, so zooming back in
    // could never pass the new, smaller wrapper width.
    var width = editor.project.canvas.width * editor.zoom;
    canvas.style.width = Math.round(width) + 'px';
    canvas.style.height = 'auto';
    $('frontZoom').value = String(editor.zoom);
    $('frontZoomText').textContent = Math.round(editor.zoom * 100) + '%';
  }

  function frontEditorClose(force) {
    var editor = state.frontEditor;
    if (!editor) return;
    if (!force && editor.dirty && !window.confirm(t('web.frontEditor.discard'))) return;
    var dialog = $('frontEditorDialog');
    if (dialog && dialog.open) dialog.close();
    state.frontEditor = null;
  }

  function frontRenderPng(editor) {
    return frontRenderContent(editor).toDataURL('image/png');
  }

  function saveFrontEditor() {
    var editor = state.frontEditor;
    if (!editor) return;
    if (!isWritable()) {
      toast(t('web.frontEditor.readonly'), 'warn');
      return;
    }
    var name = $('frontNameInput').value.trim();
    if (!name) {
      toast(t('web.frontEditor.nameRequired'), 'warn');
      return;
    }
    if (!editor.project.layers.length) {
      toast(t('web.frontEditor.empty'), 'warn');
      return;
    }
    if (!editor.templateImage) {
      toast(t('web.frontEditor.templatePending'), 'warn');
      return;
    }
    editor.project.name = name;
    editor.project.layers.forEach(function (layer) {
      if (layer.type === 'text') layer.text = String(layer.text || '').slice(0, 160);
    });
    var render = frontRenderPng(editor);
    var query = '?rom=' + encodePath(editor.game.rom) +
      '&name=' + encodeURIComponent(name) +
      '&model=' + encodeURIComponent(editor.project.model || 'any');
    if (editor.faceId) query += '&id=' + encodeURIComponent(editor.faceId);
    var body = {
      revision: editor.revision || 0,
      project: editor.project,
      render: render
    };
    editor.busy = true;
    $('frontEditorSave').disabled = true;
    fetch('/api/rom/fronts/project' + query, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json', 'X-Slot-Web': '1' },
      body: JSON.stringify(body)
    })
      .then(function (response) {
        return response.json().then(function (data) {
          if (!response.ok) {
            var error = new Error((data && data.error) || 'HTTP ' + response.status);
            error.status = response.status;
            throw error;
          }
          return data;
        });
      })
      .then(function (data) {
        editor.faceId = data.id;
        editor.revision = data.revision;
        editor.dirty = false;
        toast(t('web.frontEditor.savedToast'));
        frontEditorClose(true);
        if (state.selectedRom) {
          loadFronts(state.selectedRom);
          loadRoms();
        }
      })
      .catch(function (error) {
        showError(error, t('web.frontEditor.saveFailed'));
      })
      .finally(function () {
        editor.busy = false;
        $('frontEditorSave').disabled = false;
      });
  }

  function openFrontEditor(game, face) {
    if (!isWritable()) {
      toast(t('web.rom.frontReadonly'), 'warn');
      return;
    }
    var editor = {
      game: game,
      faceId: null,
      project: frontNewProject(game),
      revision: 0,
      selected: null,
      images: {},
      scaleBases: {},
      history: [],
      historyIndex: 0,
      dirty: false,
      busy: false,
      showMask: true,
      showGuides: false,
      zoom: 1,
      origin: face ? (face.source === 'project' ? 'project' : 'copy') : 'new'
    };
    state.frontEditor = editor;
    editor.history = [JSON.stringify(editor.project)];
    editor.historyIndex = 0;
    frontLoadTemplate(editor);
    var dialog = $('frontEditorDialog');
    if (dialog) {
      dialog.classList.remove('is-maximized');
      var focus = $('frontEditorFocus');
      if (focus) {
        focus.textContent = t('web.frontEditor.focus');
        focus.setAttribute('aria-pressed', 'false');
      }
      if (!dialog.open) dialog.showModal();
    }
    renderFrontEditor(editor);
    setFrontZoom(frontFitZoom());
    if (!face) {
      editor.project.name = (game.stem || '') + ' 前盖';
      $('frontNameInput').value = editor.project.name;
      return;
    }
    if (face.source === 'project' || face.project) {
      editor.faceId = face.id;
      fetch('/api/rom/fronts/project?rom=' + encodePath(game.rom) + '&id=' + encodeURIComponent(face.id), {
        cache: 'no-store'
      })
        .then(function (response) {
          if (!response.ok) throw new Error('HTTP ' + response.status);
          return response.json();
        })
        .then(function (data) {
          var project = data.project || {};
          editor.project = project;
          editor.revision = project.revision || 0;
          editor.project.revision = editor.revision;
          editor.origin = 'project';
          editor.dirty = false;
          editor.history = [JSON.stringify(editor.project)];
          editor.historyIndex = 0;
          $('frontNameInput').value = editor.project.name || face.name || '';
          frontLoadImages(editor);
          renderFrontEditor(editor);
        })
        .catch(function (error) {
          showError(error, t('web.frontEditor.loadFailed'));
        });
    } else {
      // Phase A uploads have no layer project.  Open a baked copy and save it
      // as a new front so the original upload stays untouched.
      editor.faceId = null;
      editor.revision = 0;
      editor.project.name = (face.name || game.stem || '') + ' ' + t('web.frontEditor.copySuffix');
      $('frontNameInput').value = editor.project.name;
      fetch(face.url, { cache: 'no-store' })
        .then(function (response) {
          if (!response.ok) throw new Error('HTTP ' + response.status);
          return response.blob();
        })
        .then(function (blob) {
          frontFileToDataUrl(blob, function (src) {
            var image = new Image();
            image.onload = function () {
              editor.project.layers = [];
              editor.images = {};
              frontAddImageLayer(editor, src, image, t('web.frontEditor.bakedLayer'), true);
              editor.project.layers[0].locked = false;
              editor.dirty = true;
              renderFrontEditor(editor);
            };
            image.onerror = function () { toast(t('web.frontEditor.imageFailed'), 'error'); };
            image.src = src;
          });
        })
        .catch(function (error) {
          showError(error, t('web.frontEditor.loadFailed'));
        });
    }
  }

  function bindFrontEditor() {
    var close = $('frontEditorClose');
    if (close) close.addEventListener('click', function () { frontEditorClose(false); });
    var cancel = $('frontEditorCancel');
    if (cancel) cancel.addEventListener('click', function () { frontEditorClose(false); });
    var save = $('frontEditorSave');
    if (save) save.addEventListener('click', saveFrontEditor);
    var undo = $('frontUndo');
    if (undo) undo.addEventListener('click', frontUndo);
    var redo = $('frontRedo');
    if (redo) redo.addEventListener('click', frontRedo);
    var frontFilePickerOpenUntil = 0;
    var addImage = $('frontAddImage');
    if (addImage) addImage.addEventListener('click', function () {
      var input = $('frontImageInput');
      if (input) {
        // Cancelling the native file picker produces a `cancel` event that can
        // bubble as a dialog cancel; keep the editor open until the picker is
        // really done.
        frontFilePickerOpenUntil = Date.now() + 2000;
        input.value = '';
        input.click();
      }
    });
    var file = $('frontImageInput');
    if (file) {
      file.addEventListener('click', function (event) { event.stopPropagation(); });
      file.addEventListener('cancel', function (event) {
        // Do not clear the guard immediately: the dialog's own `cancel` event
        // can arrive right after this one.  It expires shortly anyway.
        event.stopPropagation();
        setTimeout(function () { frontFilePickerOpenUntil = 0; }, 500);
      });
      file.addEventListener('change', function () {
        frontFilePickerOpenUntil = 0;
        if (state.frontEditor) frontAddImageFile(state.frontEditor, file.files[0]);
      });
    }
    var addCover = $('frontAddCover');
    if (addCover) addCover.addEventListener('click', function () {
      if (state.frontEditor) frontAddCurrentCover(state.frontEditor);
    });
    var addShell = $('frontAddShell');
    if (addShell) addShell.addEventListener('click', function () {
      if (state.frontEditor) frontAddShellLayer(state.frontEditor);
    });
    var addText = $('frontAddText');
    if (addText) addText.addEventListener('click', function () {
      if (state.frontEditor) frontAddTextLayer(state.frontEditor);
    });
    var addRect = $('frontAddRect');
    if (addRect) addRect.addEventListener('click', function () {
      if (state.frontEditor) frontAddShapeLayer(state.frontEditor, 'rect');
    });
    var addEllipse = $('frontAddEllipse');
    if (addEllipse) addEllipse.addEventListener('click', function () {
      if (state.frontEditor) frontAddShapeLayer(state.frontEditor, 'ellipse');
    });
    var addLine = $('frontAddLine');
    if (addLine) addLine.addEventListener('click', function () {
      if (state.frontEditor) frontAddShapeLayer(state.frontEditor, 'line');
    });
    var templateTitle = $('frontTemplateTitle');
    if (templateTitle) templateTitle.addEventListener('click', function () {
      if (state.frontEditor) frontApplyTemplate(state.frontEditor, 'title');
    });
    var templateFrame = $('frontTemplateFrame');
    if (templateFrame) templateFrame.addEventListener('click', function () {
      if (state.frontEditor) frontApplyTemplate(state.frontEditor, 'frame');
    });
    var updateBackground = function () {
      var editor = state.frontEditor;
      if (!editor) return;
      frontRemember(editor);
      var from = $('frontBackground').value || '#202028';
      var to = $('frontBackground2').value || '#08080c';
      var angle = Math.max(0, Math.min(360, Number($('frontGradientAngle').value) || 90));
      editor.project.background = $('frontGradient').checked
        ? { type: 'linear', from: from, to: to, angle: angle }
        : from;
      editor.dirty = true;
      renderFrontEditor(editor);
    };
    var bg = $('frontBackground');
    if (bg) bg.addEventListener('change', updateBackground);
    var bg2 = $('frontBackground2');
    if (bg2) bg2.addEventListener('change', updateBackground);
    var bgAngle = $('frontGradientAngle');
    if (bgAngle) bgAngle.addEventListener('change', updateBackground);
    var gradient = $('frontGradient');
    if (gradient) gradient.addEventListener('change', updateBackground);
    var mask = $('frontShowMask');
    if (mask) mask.addEventListener('change', function () {
      var editor = state.frontEditor;
      if (!editor) return;
      editor.showMask = mask.checked;
      renderFrontCanvas(editor);
    });
    var guides = $('frontShowGuides');
    if (guides) guides.addEventListener('change', function () {
      var editor = state.frontEditor;
      if (!editor) return;
      editor.showGuides = guides.checked;
      renderFrontCanvas(editor);
    });
    var zoom = $('frontZoom');
    if (zoom) zoom.addEventListener('input', function () { setFrontZoom(zoom.value); });
    var focus = $('frontEditorFocus');
    if (focus) focus.addEventListener('click', function () {
      var dialog = $('frontEditorDialog');
      if (!dialog) return;
      var on = !dialog.classList.contains('is-maximized');
      dialog.classList.toggle('is-maximized', on);
      focus.textContent = t(on ? 'web.frontEditor.focusOff' : 'web.frontEditor.focus');
      focus.setAttribute('aria-pressed', on ? 'true' : 'false');
      // The stage changes width/height class-side; wait for layout before
      // recalculating the artboard fit.
      requestAnimationFrame(function () { setFrontZoom(frontFitZoom()); });
    });
    var name = $('frontNameInput');
    if (name) name.addEventListener('change', function () {
      var editor = state.frontEditor;
      if (!editor) return;
      editor.project.name = name.value.trim();
      editor.dirty = true;
      renderFrontEditor(editor);
    });
    bindFrontCanvas();
    var dialog = $('frontEditorDialog');
    if (dialog) dialog.addEventListener('cancel', function (event) {
      event.preventDefault();
      if (frontFilePickerOpenUntil && Date.now() < frontFilePickerOpenUntil) {
        frontFilePickerOpenUntil = 0;
        return;
      }
      frontEditorClose(false);
    });
    document.addEventListener('keydown', function (event) {
      if (!state.frontEditor) return;
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'z') {
        event.preventDefault();
        if (event.shiftKey) frontRedo(); else frontUndo();
      } else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'y') {
        event.preventDefault();
        frontRedo();
      }
    });
  }

  function saveRomCheat() {    var game = state.selectedRom;
    if (!game) return;
    if (!isWritable()) {
      toast(t('web.rom.readonly_cheat'), 'warn');
      return;
    }
    var input = $('romCheatPath');
    var path = input ? input.value.trim() : '';
    apiWrite('/api/rom/cheat?rom=' + encodePath(game.rom), { cheat: path }, 'PUT')
      .then(function (data) {
        game.cheat = (data && data.cheat) || null;
        game.has_cheat = !!game.cheat;
        if (input) input.value = game.cheat || '';
        toast(game.cheat ? t('web.rom.cheat_saved') : t('web.rom.cheat_reset'));
      })
      .catch(function (error) {
        toast((error && error.message) || t('web.rom.cheat_save_failed'), 'warn');
      });
  }

  function uploadRomAsset(kind) {
    var game = state.selectedRom;
    if (!game) return;
    if (!isWritable()) {
      toast(t('web.rom.readonly_asset'), 'warn');
      return;
    }
    var isVideo = kind === 'video';
    var input = $(isVideo ? 'romVideoInput' : 'romIntroInput');
    if (!input || !input.files || !input.files.length) return;
    var file = input.files[0];
    input.value = '';
    var name = file.name || '';
    if (isVideo) {
      if (!/\.mp4$/i.test(name)) {
        toast(t('web.rom.video_must_mp4'), 'warn');
        return;
      }
      if (file.size > MAX_UPLOAD_BYTES) {
        toast(t('web.rom.video_too_large', { limit: uploadLimitText() }), 'warn');
        return;
      }
    } else {
      if (!/\.txt$/i.test(name)) {
        toast(t('web.rom.intro_must_txt'), 'warn');
        return;
      }
      if (file.size > 256 * 1024) {
        toast(t('web.rom.intro_too_large'), 'warn');
        return;
      }
    }
    var url = '/api/rom/' + (isVideo ? 'video' : 'intro') + '?rom=' + encodePath(game.rom);
    if (!beginUpload(isVideo ? t('web.rom.uploading_video') : t('web.rom.uploading_intro'), file.name)) return;
    uploadWithProgress(
      'PUT',
      url,
      file,
      isVideo ? 'video/mp4' : 'text/plain; charset=utf-8',
      setUploadProgress
    )
      .then(function () {
        if (isVideo) {
          game.has_video = true;
        } else {
          game.has_intro = true;
        }
        renderRomAssets(game);
        toast(isVideo ? t('web.rom.video_uploaded') : t('web.rom.intro_uploaded'));
      })
      .catch(function (error) {
        toast((error && error.message) || (isVideo ? t('web.rom.video_upload_failed') : t('web.rom.intro_upload_failed')), 'warn');
      })
      .then(endUpload);
  }


  function saveRomGuide() {
    var game = state.selectedRom;
    if (!game) return;
    if (!isWritable()) {
      toast(t('web.rom.readonly_guide'), 'warn');
      return;
    }
    var input = $('romGuidePath');
    var path = input ? input.value.trim() : '';
    apiWrite('/api/rom/guide?rom=' + encodePath(game.rom), { guide: path })
      .then(function (data) {
        game.guide = (data && data.guide) || null;
        game.has_guide = !!game.guide;
        if (input) input.value = game.guide || '';
        renderRomAssets(game);
        toast(game.guide ? t('web.rom.guide_saved') : t('web.rom.guide_reset'));
      })
      .catch(function (error) {
        toast((error && error.message) || t('web.rom.guide_save_failed'), 'warn');
      });
  }

  function uploadRomGuide() {
    var game = state.selectedRom;
    var input = $('romGuideInput');
    if (!game || !input || !input.files || !input.files.length) return;
    if (!isWritable()) {
      toast(t('web.rom.readonly_guide_upload'), 'warn');
      return;
    }
    var file = input.files[0];
    input.value = '';
    var match = /\.(txt|epub)$/i.exec(file.name || '');
    if (!match) {
      toast(t('web.rom.guide_must_txt_epub'), 'warn');
      return;
    }
    var extension = match[1].toLowerCase();
    if (file.size > 32 * 1024 * 1024) {
      toast(t('web.rom.guide_too_large'), 'warn');
      return;
    }
    if (!beginUpload(t('web.rom.uploading_guide'), file.name)) return;
    uploadWithProgress(
      'PUT',
      '/api/rom/guide?rom=' + encodePath(game.rom) + '&ext=' + extension,
      file,
      extension === 'epub' ? 'application/epub+zip' : 'text/plain; charset=utf-8',
      setUploadProgress
    )
      .then(function (data) {
        game.guide = (data && data.guide) || null;
        game.has_guide = !!game.guide;
        var pathInput = $('romGuidePath');
        if (pathInput) pathInput.value = game.guide || '';
        renderRomAssets(game);
        toast(t('web.rom.guide_uploaded'));
      })
      .catch(function (error) {
        toast((error && error.message) || t('web.rom.guide_upload_failed'), 'warn');
      })
      .then(endUpload);
  }

  // ------------------------------------------------------------ 添加游戏
  var ADD_GAME_PLATFORMS = {
    GBA: { accept: '.gba', dir: 'GBA', hint: '.gba' },
    GB: { accept: '.gb,.gbc', dir: 'GB', hint: '.gb / .gbc' },
    GBC: { accept: '.gb,.gbc', dir: 'GBC', hint: '.gb / .gbc' },
    FC: { accept: '.nes', dir: 'FC', hint: '.nes' },
    SFC: { accept: '.sfc,.smc,.fig', dir: 'SFC', hint: '.sfc / .smc / .fig' },
    MD: { accept: '.md,.gen,.smd,.bin', dir: 'MD', hint: '.md / .gen / .smd / .bin' }
  };

  function addGamePlatform() {
    var select = $('addGamePlatform');
    var value = select ? select.value : 'GBA';
    return ADD_GAME_PLATFORMS[value] ? value : 'GBA';
  }

  function syncAddGamePlatform() {
    var platform = addGamePlatform();
    var spec = ADD_GAME_PLATFORMS[platform];
    var input = $('addGameFile');
    if (input) input.accept = spec.accept;
    var hint = $('addGameHint');
    if (hint) {
      hint.textContent = t('web.addGame.hint_for', {
        ext: spec.hint,
        dir: spec.dir
      });
    }
    // A file picked for the previous platform may not fit the new one.
    state.addGameFile = null;
    if ($('addGameName')) $('addGameName').value = '';
    if (input) input.value = '';
    if ($('addGameSubmit')) $('addGameSubmit').disabled = true;
  }

  function openAddGame() {
    if (!isWritable()) {
      toast(t('web.addGame.readonly'), 'warn');
      return;
    }
    var dialog = $('addGameDialog');
    syncAddGamePlatform();
    if (dialog && dialog.showModal) dialog.showModal();
  }

  function closeAddGame() {
    var dialog = $('addGameDialog');
    if (dialog && dialog.open) dialog.close();
  }

  function pickAddGameFile(file) {
    state.addGameFile = file || null;
    if ($('addGameName')) $('addGameName').value = file ? file.name : '';
    if ($('addGameSubmit')) $('addGameSubmit').disabled = !file;
  }

  function submitAddGame() {
    var file = state.addGameFile;
    if (!file) return;
    if (file.size > MAX_UPLOAD_BYTES) {
      toast(t('web.addGame.too_large', { limit: uploadLimitText() }), 'warn');
      return;
    }
    var platform = addGamePlatform();
    if (!beginUpload(t('web.addGame.uploading', { platform: platform }), file.name)) return;
    if ($('addGameSubmit')) $('addGameSubmit').disabled = true;
    uploadWithProgress(
      'POST',
      '/api/rom/add?platform=' + encodeURIComponent(platform) + '&name=' + encodePath(file.name),
      file,
      'application/octet-stream',
      setUploadProgress
    )
      .then(function (data) {
        closeAddGame();
        toast(t('web.addGame.added', {
          name: (data && (data.stem || data.rom)) || file.name
        }));
        state.romFilter = '';
        return loadRoms();
      })
      .catch(function (error) {
        toast((error && error.message) || t('web.addGame.failed'), 'warn');
        if ($('addGameSubmit')) $('addGameSubmit').disabled = !state.addGameFile;
      })
      .then(endUpload);
  }

  var collectionPoll = null;
  var collectionBusy = false;
  var collectionRefreshed = false;
  var collectionUploadCancelled = false;

  function fmtBytesShort(bytes) {
    bytes = Number(bytes) || 0;
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
    if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
    return (bytes / (1024 * 1024 * 1024)).toFixed(2) + ' GB';
  }

  function setCollectionBusy(busy) {
    collectionBusy = busy;
    var importButton = $('collectionImport');
    if (importButton) importButton.disabled = busy || !isWritable();
    var exportButton = $('collectionExport');
    if (exportButton) {
      exportButton.disabled = busy || !state.romFilter || !isWritable();
      exportButton.textContent = state.romFilter
        ? t('web.roms.export_named', { platform: state.romFilter })
        : t('web.roms.export_disabled');
    }
    var romExport = $('romExport');
    if (romExport) romExport.disabled = busy || !isWritable();
  }

  function showCollectionPanel() {
    var panel = $('collectionStatus');
    if (panel) panel.hidden = false;
  }

  function renderCollectionStatus(data) {
    data = data || {};
    showCollectionPanel();
    var stage = $('collectionStage');
    var count = $('collectionCount');
    var bar = $('collectionBar');
    var cancel = $('collectionCancel');
    var download = $('collectionDownload');
    var errors = $('collectionError');
    if (stage) stage.textContent = data.stage || '—';
    if (count) {
      var text = '';
      if (data.total) text = (data.done || 0) + ' / ' + data.total;
      if (data.kind === 'export' && data.bytes) text += ' · ' + fmtBytesShort(data.bytes);
      if (data.kind === 'import' && data.imported) text += ' · ' + t('web.collection.imported', { count: data.imported });
      if (data.kind === 'import' && data.duplicates) text += ' · ' + t('web.collection.duplicates', { count: data.duplicates });
      if (data.kind === 'import' && data.skipped) text += ' · ' + t('web.collection.skipped', { count: data.skipped });
      count.textContent = text;
    }
    if (bar) {
      var pct = data.total ? Math.min(100, ((data.done || 0) / data.total) * 100) : 0;
      bar.style.width = pct + '%';
    }
    if (cancel) cancel.disabled = !data.running;
    if (download) {
      if (data.file && !data.running && data.kind === 'export') {
        download.hidden = false;
        download.href = '/api/files/download?path=' + encodePath(data.file);
        download.setAttribute('download', data.file.split('/').pop());
        if (state.gameExportPending) {
          // One click on 导出游戏包 is enough: the browser gets the ZIP as soon
          // as the card has finished packing it.
          state.gameExportPending = null;
          downloadPackage(download.href, data.file.split('/').pop());
          toast(t('web.collection.download_started'));
        }
      } else {
        download.hidden = true;
        download.removeAttribute('href');
        if (!data.running) state.gameExportPending = null;
      }
    }
    if (errors) {
      errors.textContent = (data.errors || []).join('\n');
    }
    if (!data.running && data.finished) {
      if (collectionPoll) {
        clearInterval(collectionPoll);
        collectionPoll = null;
      }
      setCollectionBusy(false);
      if (data.kind === 'import' && !collectionRefreshed) {
        collectionRefreshed = true;
        loadRoms();
      }
    }
  }

  function pollCollection() {
    api('/api/collection/status')
      .then(renderCollectionStatus)
      .catch(function (error) {
        showError(error, t('web.collection.read_failed'));
      });
  }

  function startCollectionPolling() {
    if (collectionPoll) clearInterval(collectionPoll);
    collectionRefreshed = false;
    collectionPoll = setInterval(pollCollection, 900);
    pollCollection();
  }

  /// Hand a finished archive to the browser without another click.
  function downloadPackage(url, name) {
    var link = document.createElement('a');
    link.href = url;
    if (name) link.download = name;
    link.rel = 'noopener';
    link.style.display = 'none';
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
  }

  function startCollectionExport(rom) {
    var single = typeof rom === 'string' && rom.length > 0;
    if (!single && !state.romFilter) {
      toast(t('web.collection.export_pick'), 'warn');
      return;
    }
    if (!isWritable()) {
      toast(t('web.collection.readonly_export'), 'warn');
      return;
    }
    setCollectionBusy(true);
    showCollectionPanel();
    if (single) {
      var panel = $('collectionStatus');
      if (panel && panel.scrollIntoView) panel.scrollIntoView({ block: 'nearest' });
    }
    state.gameExportPending = single ? single : null;
    $('collectionStage').textContent = single
      ? t('web.collection.exporting_game')
      : t('web.collection.starting_export');
    $('collectionCount').textContent = '';
    $('collectionError').textContent = '';
    $('collectionDownload').hidden = true;
    apiWrite('/api/collection/export', single ? { rom: rom } : { platform: state.romFilter })
      .then(function (data) {
        renderCollectionStatus(data);
        startCollectionPolling();
      })
      .catch(function (error) {
        setCollectionBusy(false);
        $('collectionStage').textContent = t('web.collection.export_failed');
        $('collectionError').textContent = (error && error.message) || String(error);
      });
  }

  function cancelCollection() {
    collectionUploadCancelled = true;
    var upload = $('collectionImportFile');
    if (upload) upload.value = '';
    setCollectionBusy(false);
    apiWrite('/api/collection/upload/cancel', {})
      .catch(function () {})
      .then(function () {
        return apiWrite('/api/collection/cancel', {});
      })
      .then(function (data) {
        renderCollectionStatus(data);
      })
      .catch(function (error) {
        showError(error, t('web.collection.cancel_failed'));
      });
  }

  function uploadCollection(file) {
    if (!file) return;
    if (!/\.zip$/i.test(file.name || '')) {
      toast(t('web.collection.must_zip'), 'warn');
      return;
    }
    if (!isWritable()) {
      toast(t('web.collection.readonly_import'), 'warn');
      return;
    }
    if (
      !window.confirm(t('web.collection.import_confirm'))
    ) {
      return;
    }
    setCollectionBusy(true);
    collectionRefreshed = false;
    collectionUploadCancelled = false;
    showCollectionPanel();
    $('collectionStage').textContent = t('web.upload.collection');
    $('collectionCount').textContent = '0%';
    $('collectionError').textContent = '';
    $('collectionDownload').hidden = true;
    var chunkSize = 8 * 1024 * 1024;
    var offset = 0;
    function chunk() {
      if (collectionUploadCancelled) return Promise.reject(new Error(t('web.collection.cancelled')));
      if (offset >= file.size) return Promise.resolve();
      var end = Math.min(file.size, offset + chunkSize);
      var blob = file.slice(offset, end);
      return api('/api/collection/upload?offset=' + offset, {
        method: 'PUT',
        headers: {
          'Content-Type': 'application/octet-stream',
          'X-Slot-Web': '1'
        },
        body: blob
      }).then(function (data) {
        offset = (data && data.offset) || end;
        var pct = Math.round((offset / file.size) * 100);
        $('collectionCount').textContent = pct + '%';
        $('collectionBar').style.width = pct + '%';
        return chunk();
      });
    }

    apiWrite('/api/collection/upload/start', {})
      .then(function () {
        return chunk();
      })
      .then(function () {
        $('collectionStage').textContent = t('web.upload.importing');
        return apiWrite('/api/collection/upload/finish', { overwrite: true });
      })
      .then(function (data) {
        renderCollectionStatus(data);
        startCollectionPolling();
      })
      .catch(function (error) {
        setCollectionBusy(false);
        $('collectionStage').textContent = t('web.collection.import_failed');
        $('collectionError').textContent = (error && error.message) || String(error);
        apiWrite('/api/collection/upload/cancel', {}).catch(function () {});
      });
  }

  // Browser play needs a writable card only for the save write-back; the ROM
  // itself is read-only.  GBA first: it is the one core shipped in the page.
  function applyPlayWebGate(writable) {
    var play = $('romPlayWeb');
    if (!play) return;
    var game = state.selectedRom;
    var name = game ? String(game.platform || '').toUpperCase() : '';
    var playable =
      name === 'GBA' || name === 'GB' || name === 'GBC' || name === 'FC';
    var ready = !!game && playable && !!writable;
    play.disabled = !ready;
    play.title = !game
      ? t('web.rom.play_select')
      : !playable
        ? t('web.rom.play_unsupported')
        : !writable
          ? t('web.rom.play_readonly')
          : t('web.rom.playTitle');
  }

  function playInBrowser() {
    var game = state.selectedRom;
    if (!game || !game.rom) return;
    if (!isWritable()) {
      toast(t('web.rom.readonly_play'), 'warn');
      return;
    }
    window.open('/play?rom=' + encodePath(game.rom), '_blank', 'noopener');
  }

  function renameSelectedRom() {
    var game = state.selectedRom;
    if (!game || !game.rom) return;
    if (!isWritable()) {
      toast(t('web.rom.renameReadonly'), 'warn');
      return;
    }
    var current = game.stem || '';
    var name = window.prompt(t('web.rom.renamePrompt'), current);
    if (name === null) return;
    name = name.trim();
    if (!name || name === current) return;
    apiWrite('/api/rom/rename', { rom: game.rom, name: name })
      .then(function (data) {
        toast(t('web.rom.renamed', { name: (data && data.stem) || name }));
        closeRomDialog();
        loadRoms();
      })
      .catch(function (error) {
        showError(error, t('web.rom.renameFailed'));
      });
  }

  function deleteSelectedRom() {
    var game = state.selectedRom;
    if (!game || !game.rom) return;
    if (!isWritable()) {
      toast(t('web.rom.deleteReadonly'), 'warn');
      return;
    }
    var name = game.stem || game.file || game.rom;
    if (!window.confirm(t('web.rom.deleteConfirm', { name: name }))) return;
    apiWrite('/api/rom/delete?rom=' + encodePath(game.rom), {})
      .then(function () {
        toast(t('web.rom.deleted', { name: name }), 'ok');
        closeRomDialog();
        loadRoms();
      })
      .catch(function (error) {
        showError(error, t('web.rom.deleteFailed'));
      });
  }

  function openRomDialog(game) {
    if (!game) return;
    state.selectedRom = game;
    $('romDialogPlatform').textContent = game.platform;
    $('romDialogTitle').textContent = game.stem || t('web.rom.detail');
    $('romDialogPlatformText').value = game.platform;
    $('romDialogFile').value = game.file || '';
    $('romDialogPath').value = game.rom || '';
    $('romDialogSize').value = fmtBytes(game.size);
    var isFc = String(game.platform || '').toUpperCase() === 'FC';
    var modelField = $('romModelField');
    if (modelField) modelField.hidden = !isFc;
    if (isFc && $('romModel')) $('romModel').value = game.model === 'nes' ? 'nes' : 'famicom';
    var frontModel = $('frontModel');
    if (frontModel) {
      frontModel.hidden = !isFc;
      frontModel.value = isFc && game.model === 'nes' ? 'nes' : 'any';
    }
    loadFronts(game);
    setRomColor(game.color || '#8fd6c4');
    var cheatInput = $('romCheatPath');
    if (cheatInput) cheatInput.value = game.cheat || '';
    var guideInput = $('romGuidePath');
    if (guideInput) guideInput.value = game.guide || '';
    renderRomDialogCover(game);
    renderRomAssets(game);
    applyWritable();
    applyPlayWebGate(isWritable());
    $('romDialog').showModal();
  }

  function closeRomDialog() {
    var dialog = $('romDialog');
    if (dialog && dialog.open) dialog.close();
    state.selectedRom = null;
  }

  function openCoverEditor() {
    var game = state.selectedRom;
    if (!game) return;
    if (!isWritable()) {
      toast(t('web.cover.readonly_edit'), 'warn');
      return;
    }
    state.coverGame = game;
    state.coverImage = null;
    state.coverZoom = 1;
    state.coverX = 0;
    state.coverY = 0;
    state.coverPointer = null;
    state.coverBusy = false;
    var canvas = $('coverCanvas');
    canvas.width = game.cover_width || 532;
    canvas.height = game.cover_height || 276;
    canvas.style.backgroundColor = game.color || '#8fd6c4';
    $('coverTargetHint').textContent =
      (game.stem || '') + ' · ' + canvas.width + ' × ' + canvas.height;
    $('coverZoom').value = '1';
    $('coverZoomText').textContent = '100%';
    $('coverEmpty').hidden = !game.has_cover;
    if (game.has_cover) {
      loadCoverImage(cacheBust(game.cover));
    } else {
      state.coverImage = null;
      $('coverEmpty').hidden = false;
      drawCover();
    }
    $('coverDialog').showModal();
  }

  function closeCoverEditor() {
    var dialog = $('coverDialog');
    if (dialog && dialog.open) dialog.close();
    state.coverGame = null;
    state.coverImage = null;
    state.coverPointer = null;
  }

  function loadCoverImage(source) {
    var img = new Image();
    img.onload = function () {
      state.coverImage = img;
      state.coverZoom = 1;
      state.coverX = 0;
      state.coverY = 0;
      $('coverEmpty').hidden = true;
      $('coverZoom').value = '1';
      $('coverZoomText').textContent = '100%';
      coverApplyZoomBounds();
      drawCover();
    };
    img.onerror = function () {
      state.coverImage = null;
      $('coverEmpty').hidden = false;
      drawCover();
    };
    img.src = source;
  }

  function loadCoverFile(file) {
    if (!file) return;
    if (file.size > 16 * 1024 * 1024) {
      toast(t('web.cover.too_large'), 'error', 5000);
      return;
    }
    var url = URL.createObjectURL(file);
    var img = new Image();
    img.onload = function () {
      state.coverImage = img;
      state.coverZoom = 1;
      state.coverX = 0;
      state.coverY = 0;
      $('coverEmpty').hidden = true;
      $('coverZoom').value = '1';
      $('coverZoomText').textContent = '100%';
      coverApplyZoomBounds();
      drawCover();
      URL.revokeObjectURL(url);
    };
    img.onerror = function () {
      URL.revokeObjectURL(url);
      toast(t('web.cover.unreadable'), 'error');
    };
    img.src = url;
  }

  /// The scale that fills the canvas (the old baseline), and the one that
  /// fits the whole picture inside it.  The slider is a multiplier of the
  /// fill scale, so fitting is always <= 100%.
  function coverBaseScale() {
    var canvas = $('coverCanvas');
    var img = state.coverImage;
    if (!canvas || !img) return 1;
    return Math.max(canvas.width / img.width, canvas.height / img.height);
  }

  function coverFitZoom() {
    var canvas = $('coverCanvas');
    var img = state.coverImage;
    if (!canvas || !img) return 1;
    return Math.min(canvas.width / img.width, canvas.height / img.height) /
      coverBaseScale();
  }

  /// The slider bottom follows the picture, so it can always reach "整图显示".
  function coverApplyZoomBounds() {
    var zoom = $('coverZoom');
    if (!zoom) return;
    var fit = Math.floor(coverFitZoom() * 100) / 100;
    zoom.min = String(Math.min(1, Math.max(0.05, fit)));
  }

  function fitCover() {
    if (!state.coverImage) return;
    state.coverX = 0;
    state.coverY = 0;
    var zoom = Math.min(1, coverFitZoom());
    state.coverZoom = zoom;
    $('coverZoom').value = String(zoom);
    $('coverZoomText').textContent = Math.round(zoom * 100) + '%';
    drawCover();
  }

  function coverClamp() {
    var canvas = $('coverCanvas');
    var img = state.coverImage;
    if (!img) return;
    var scale =
      Math.max(canvas.width / img.width, canvas.height / img.height) *
      state.coverZoom;
    var maxX = Math.max(0, (img.width * scale - canvas.width) / 2);
    var maxY = Math.max(0, (img.height * scale - canvas.height) / 2);
    state.coverX = Math.max(-maxX, Math.min(maxX, state.coverX));
    state.coverY = Math.max(-maxY, Math.min(maxY, state.coverY));
  }

  function drawCover() {
    var canvas = $('coverCanvas');
    if (!canvas) return;
    var ctx = canvas.getContext('2d');
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    // Keep the uncovered part transparent: the cartridge colour is painted by
    // CSS behind the canvas, and the saved PNG stays transparent so the device
    // shows the shell colour through it too.  Never paint a black matte here.
    if (state.coverGame && state.coverGame.color) {
      canvas.style.backgroundColor = state.coverGame.color;
    }
    var img = state.coverImage;
    if (!img) {
      $('coverEmpty').hidden = false;
      return;
    }
    $('coverEmpty').hidden = true;
    coverClamp();
    var scale =
      Math.max(canvas.width / img.width, canvas.height / img.height) *
      state.coverZoom;
    ctx.save();
    ctx.translate(canvas.width / 2 + state.coverX, canvas.height / 2 + state.coverY);
    ctx.drawImage(
      img,
      (-img.width * scale) / 2,
      (-img.height * scale) / 2,
      img.width * scale,
      img.height * scale
    );
    ctx.restore();
  }

  function setCoverZoom(value) {
    var zoom = $('coverZoom');
    var min = zoom ? Number(zoom.min) || 0.05 : 0.2;
    state.coverZoom = Math.max(Math.min(1, min), Math.min(3, Number(value) || 1));
    $('coverZoom').value = String(state.coverZoom);
    $('coverZoomText').textContent = Math.round(state.coverZoom * 100) + '%';
    drawCover();
  }

  function coverPointerPosition(event) {
    var canvas = $('coverCanvas');
    var rect = canvas.getBoundingClientRect();
    return {
      x: (event.clientX - rect.left) * canvas.width / Math.max(1, rect.width),
      y: (event.clientY - rect.top) * canvas.height / Math.max(1, rect.height)
    };
  }

  function bindCoverPointer() {
    var canvas = $('coverCanvas');
    if (!canvas) return;
    canvas.addEventListener('pointerdown', function (event) {
      if (!state.coverImage) return;
      event.preventDefault();
      canvas.setPointerCapture(event.pointerId);
      state.coverPointer = coverPointerPosition(event);
    });
    canvas.addEventListener('pointermove', function (event) {
      if (!state.coverPointer) return;
      event.preventDefault();
      var point = coverPointerPosition(event);
      state.coverX += point.x - state.coverPointer.x;
      state.coverY += point.y - state.coverPointer.y;
      state.coverPointer = point;
      drawCover();
    });
    var release = function () {
      state.coverPointer = null;
    };
    canvas.addEventListener('pointerup', release);
    canvas.addEventListener('pointercancel', release);
  }

  function saveCover() {
    var game = state.coverGame;
    if (!game || state.coverBusy) return;
    if (!state.coverImage) {
      toast(t('web.cover.select_first'), 'warn');
      return;
    }
    if (!isWritable()) {
      toast(t('web.cover.readonly_save'), 'warn');
      return;
    }
    var canvas = $('coverCanvas');
    var button = $('coverSave');
    state.coverBusy = true;
    if (button) button.disabled = true;
    canvas.toBlob(function (blob) {
      if (!blob) {
        state.coverBusy = false;
        if (button) button.disabled = !isWritable();
        toast(t('web.cover.generate_failed'), 'error');
        return;
      }
      if (blob.size > 8 * 1024 * 1024) {
        state.coverBusy = false;
        if (button) button.disabled = !isWritable();
        toast(t('web.cover.file_too_large'), 'error', 5000);
        return;
      }
      putBinary('/api/rom/cover?rom=' + encodePath(game.rom), blob, 'image/png')
        .then(function () {
          game.has_cover = true;
          game.mtime = Math.floor(Date.now() / 1000);
          renderRomDialogCover(game);
          renderRoms();
          closeCoverEditor();
          toast(t('web.cover.saved'), 'ok');
        })
        .catch(function (error) {
          showError(error, t('web.cover.save_failed'));
        })
        .then(function () {
          state.coverBusy = false;
          if (button) button.disabled = !isWritable();
        });
    }, 'image/png');
  }

  function chooseRomFile() {
    var game = state.selectedRom;
    if (!game) return;
    if (!isWritable()) {
      toast(t('web.rom.readonly_replace'), 'warn');
      return;
    }
    var input = $('romFileInput');
    input.accept = romAccept(game.platform);
    input.value = '';
    input.click();
  }

  // One table for the cart types each shelf takes, so the file picker, the
  // extension check and the empty-library hint cannot disagree about what a
  // platform reads. FC is fceumm's `.nes`; its `.fds` is deliberately absent
  // (the frontend ships no disk-swap keys yet).
  var ROM_EXTENSIONS = {
    GBA: ['gba'],
    GB: ['gb', 'gbc'],
    GBC: ['gb', 'gbc'],
    FC: ['nes'],
    SFC: ['sfc', 'smc', 'fig'],
    MD: ['md', 'gen', 'smd', 'bin'],
  };

  function romExtensions(platform) {
    return ROM_EXTENSIONS[platform] || [];
  }

  function romAccept(platform) {
    var exts = romExtensions(platform);
    if (!exts.length) return '';
    var out = [];
    for (var i = 0; i < exts.length; i += 1) out.push('.' + exts[i]);
    return out.join(',');
  }

  function replaceRom(file) {
    var game = state.selectedRom;
    if (!game || !file) return;
    if (file.size > MAX_UPLOAD_BYTES) {
      toast(t('web.rom.replace_too_large', { limit: uploadLimitText() }), 'error', 5000);
      return;
    }
    var extension = String(file.name || '').split('.').pop().toLowerCase();
    var allowed = romExtensions(game.platform);
    if (allowed.indexOf(extension) < 0) {
      toast(t('web.rom.type_mismatch', { platform: game.platform }), 'error', 5000);
      return;
    }
    if (
      !window.confirm(
        t('web.rom.replace_confirm', {
          file: file.name,
          target: game.file || game.stem
        })
      )
    ) {
      return;
    }
    if (!beginUpload(t('web.upload.replacing'), file.name)) return;
    var button = $('romReplaceRom');
    if (button) {
      button.disabled = true;
      button.textContent = t('web.rom.replacing');
    }
    uploadWithProgress(
      'PUT',
      '/api/rom?rom=' + encodePath(game.rom),
      file,
      'application/octet-stream',
      setUploadProgress
    )
      .then(function (result) {
        game.size = (result && result.size) || file.size;
        $('romDialogSize').value = fmtBytes(game.size);
        renderRoms();
        toast(t('web.rom.replaced'), 'ok');
      })
      .catch(function (error) {
        showError(error, t('web.rom.replace_failed'));
      })
      .then(function () {
        if (button) {
          button.disabled = !isWritable();
          button.textContent = t('web.rom.replaceRom');
        }
        endUpload();
      });
  }

  function bindRoms() {
    var refresh = $('refreshRoms');
    if (!refresh) return;
    $('refreshRoms').addEventListener('click', function () {
      loadRoms();
    });
    var addGame = $('addGame');
    if (addGame) addGame.addEventListener('click', openAddGame);
    var closeAdd = $('closeAddGame');
    if (closeAdd) closeAdd.addEventListener('click', closeAddGame);
    var addCancel = $('addGameCancel');
    if (addCancel) addCancel.addEventListener('click', closeAddGame);
    var addPlatform = $('addGamePlatform');
    if (addPlatform) addPlatform.addEventListener('change', syncAddGamePlatform);
    var addPick = $('addGamePick');
    if (addPick) {
      addPick.addEventListener('click', function () {
        var input = $('addGameFile');
        if (input) {
          input.value = '';
          input.click();
        }
      });
    }
    var addFile = $('addGameFile');
    if (addFile) {
      addFile.addEventListener('change', function () {
        var file = this.files && this.files[0] ? this.files[0] : null;
        this.value = '';
        pickAddGameFile(file);
      });
    }
    var addSubmit = $('addGameSubmit');
    if (addSubmit) addSubmit.addEventListener('click', submitAddGame);
    var collectionImport = $('collectionImport');
    if (collectionImport) {
      collectionImport.addEventListener('click', function () {
        $('collectionImportFile').click();
      });
    }
    var collectionExport = $('collectionExport');
    if (collectionExport) {
      collectionExport.addEventListener('click', function () {
        startCollectionExport();
      });
    }
    var collectionImportFile = $('collectionImportFile');
    if (collectionImportFile) {
      collectionImportFile.addEventListener('change', function () {
        var file = this.files && this.files[0];
        this.value = '';
        uploadCollection(file);
      });
    }
    var collectionCancel = $('collectionCancel');
    if (collectionCancel) {
      collectionCancel.addEventListener('click', cancelCollection);
    }
    $('romSearch').addEventListener('input', function () {
      state.romQuery = this.value;
      renderRoms();
    });
    $('romFilters').addEventListener('click', function (event) {
      var button = event.target.closest('[data-platform]');
      if (!button) return;
      state.romFilter = button.getAttribute('data-platform') || '';
      renderRoms();
    });
    $('romList').addEventListener('click', function (event) {
      var card = event.target.closest('[data-rom-index]');
      if (!card) return;
      var index = Number(card.getAttribute('data-rom-index'));
      if (index >= 0 && state.roms && state.roms[index]) openRomDialog(state.roms[index]);
    });
    $('closeRomDialog').addEventListener('click', closeRomDialog);
    $('romDialog').addEventListener('close', function () {
      state.selectedRom = null;
    });
    var romColor = $('romColor');
    if (romColor) {
      romColor.addEventListener('input', function () {
        setRomColor(this.value);
      });
    }
    var romColorHex = $('romColorHex');
    if (romColorHex) {
      romColorHex.addEventListener('input', function () {
        var hex = normalizeRomColor(this.value);
        if (hex) setRomColor(hex);
      });
    }
    var romColorRandom = $('romColorRandom');
    if (romColorRandom) {
      romColorRandom.addEventListener('click', function () {
        setRomColor(randomRomColor());
      });
    }
    var romColorSave = $('romColorSave');
    if (romColorSave) {
      romColorSave.addEventListener('click', saveRomColor);
    }
    var romModelSave = $('romModelSave');
    if (romModelSave) {
      romModelSave.addEventListener('click', saveRomModel);
    }
    var frontAdd = $('frontAdd');
    if (frontAdd) {
      frontAdd.addEventListener('click', function () {
        var input = $('frontFile');
        if (input) {
          input.value = '';
          input.click();
        }
      });
    }
    var frontQuickCover = $('frontQuickCover');
    if (frontQuickCover) {
      frontQuickCover.addEventListener('click', function () {
        if (state.selectedRom) frontQuickAddCover(state.selectedRom);
      });
    }
    var frontQuickBatch = $('frontQuickBatch');
    if (frontQuickBatch) {
      frontQuickBatch.addEventListener('click', function () {
        var input = $('frontQuickFile');
        if (input) {
          input.value = '';
          input.click();
        }
      });
    }
    var frontQuickFit = $('frontQuickFit');
    if (frontQuickFit) {
      frontQuickFit.addEventListener('change', function () {
        if (state.selectedRom) frontQuickRefreshPreview(state.selectedRom);
      });
    }
    var frontQuickFile = $('frontQuickFile');
    if (frontQuickFile) {
      frontQuickFile.addEventListener('change', function () {
        var files = frontQuickFile.files ? Array.prototype.slice.call(frontQuickFile.files) : [];
        frontQuickFile.value = '';
        if (state.selectedRom && files.length) frontQuickAddFiles(state.selectedRom, files);
      });
    }
    var frontNew = $('frontNew');
    if (frontNew) {
      frontNew.addEventListener('click', function () {
        if (state.selectedRom) openFrontEditor(state.selectedRom, null);
      });
    }
    var frontImport = $('frontImport');
    if (frontImport) {
      frontImport.addEventListener('click', function () {
        var input = $('frontImportFile');
        if (input) { input.value = ''; input.click(); }
      });
    }
    var frontImportFile = $('frontImportFile');
    if (frontImportFile) {
      frontImportFile.addEventListener('change', function () {
        if (state.selectedRom) importFront(state.selectedRom, frontImportFile.files[0]);
      });
    }
    var frontFile = $('frontFile');
    if (frontFile) {
      frontFile.addEventListener('change', function () {
        if (state.selectedRom) uploadFront(state.selectedRom);
      });
    }
    var romCheatSave = $('romCheatSave');
    if (romCheatSave) romCheatSave.addEventListener('click', saveRomCheat);
    var romGuideSave = $('romGuideSave');
    if (romGuideSave) romGuideSave.addEventListener('click', saveRomGuide);
    var romGuidePick = $('romGuidePick');
    if (romGuidePick) {
      romGuidePick.addEventListener('click', function () {
        $('romGuideInput').click();
      });
    }
    var romGuideInput = $('romGuideInput');
    if (romGuideInput) romGuideInput.addEventListener('change', uploadRomGuide);
    $('romReplaceCover').addEventListener('click', openCoverEditor);
    $('romReplaceRom').addEventListener('click', chooseRomFile);
    var rename = $('romRename');
    if (rename) rename.addEventListener('click', renameSelectedRom);
    var remove = $('romDelete');
    if (remove) remove.addEventListener('click', deleteSelectedRom);
    var playWeb = $('romPlayWeb');
    if (playWeb) playWeb.addEventListener('click', playInBrowser);
    var romExport = $('romExport');
    if (romExport) {
      romExport.addEventListener('click', function () {
        var game = state.selectedRom;
        if (!game) return;
        closeRomDialog();
        startCollectionExport(game.rom);
      });
    }
    $('romFileInput').addEventListener('change', function () {
      var file = this.files && this.files[0] ? this.files[0] : null;
      this.value = '';
      if (file) replaceRom(file);
    });
    $('closeCoverDialog').addEventListener('click', closeCoverEditor);
    $('coverCancel').addEventListener('click', closeCoverEditor);
    $('coverPick').addEventListener('click', function () {
      var input = $('coverFileInput');
      input.value = '';
      input.click();
    });
    $('coverFileInput').addEventListener('change', function () {
      var file = this.files && this.files[0] ? this.files[0] : null;
      this.value = '';
      if (file) loadCoverFile(file);
    });
    $('coverZoom').addEventListener('input', function () {
      setCoverZoom(this.value);
    });
    $('coverReset').addEventListener('click', function () {
      state.coverZoom = 1;
      state.coverX = 0;
      state.coverY = 0;
      $('coverZoom').value = '1';
      $('coverZoomText').textContent = '100%';
      drawCover();
    });
    var coverFit = $('coverFit');
    if (coverFit) coverFit.addEventListener('click', fitCover);
    var introPick = $('romIntroPick');
    if (introPick) {
      introPick.addEventListener('click', function () {
        $('romIntroInput').click();
      });
    }
    var videoPick = $('romVideoPick');
    if (videoPick) {
      videoPick.addEventListener('click', function () {
        $('romVideoInput').click();
      });
    }
    var introInput = $('romIntroInput');
    if (introInput) {
      introInput.addEventListener('change', function () {
        uploadRomAsset('intro');
      });
    }
    var videoInput = $('romVideoInput');
    if (videoInput) {
      videoInput.addEventListener('change', function () {
        uploadRomAsset('video');
      });
    }

    $('coverSave').addEventListener('click', saveCover);
    bindCoverPointer();
  }

  // ---------------------------------------------------------------- 存档页

  function loadSaves() {
    return api('/api/saves')
      .then(function (data) {
        state.saves = data;
        renderSaves();
      })
      .catch(function (error) {
        detachListMore(state.savesView);
        state.savesView = null;
        $('savesList').innerHTML =
          '<div class="empty">' + esc(t('web.error.saves_retry')) + '</div>';
        $('savesSummary').textContent = t('web.overview.read_failed');
        showError(error, t('web.error.saves_read'));
      });
  }

  // ---------------------------------------------------------------- 文件页

  var FILE_IMAGE_EXT = { png: 1, jpg: 1, jpeg: 1, gif: 1, webp: 1, bmp: 1, svg: 1 };
  var FILE_TEXT_EXT = {
    txt: 1, md: 1, log: 1, json: 1, lua: 1, ini: 1, toml: 1, cfg: 1,
    csv: 1, xml: 1, yml: 1, yaml: 1, js: 1, css: 1, html: 1
  };

  function fileExt(entry) {
    return (entry && entry.ext ? String(entry.ext) : '').toLowerCase();
  }

  function fileIsImage(entry) {
    return !!(entry && !entry.is_dir && FILE_IMAGE_EXT[fileExt(entry)]);
  }

  function fileIsText(entry) {
    return !!(entry && !entry.is_dir && FILE_TEXT_EXT[fileExt(entry)]);
  }

  function fileDownloadUrl(path) {
    return '/api/files/download?path=' + encodePath(path);
  }

  function findFileEntry(path) {
    var entries = state.files && state.files.entries ? state.files.entries : [];
    for (var index = 0; index < entries.length; index += 1) {
      if (entries[index].path === path) return entries[index];
    }
    return null;
  }

  function loadFiles(path) {
    var target = path === undefined ? (state.files ? state.files.path : '') : path;
    return api('/api/files?path=' + encodePath(target || ''))
      .then(function (data) {
        state.files = data;
        renderFiles();
      })
      .catch(function (error) {
        var list = $('fileList');
        if (list) {
          list.innerHTML =
            '<tr><td colspan="4" class="empty">' + esc(t('web.error.files_retry')) + '</td></tr>';
        }
        if ($('fileSummary')) $('fileSummary').textContent = t('web.overview.read_failed');
        showError(error, t('web.error.files_read'));
      });
  }

  function renderFileBreadcrumb() {
    var path = state.files && state.files.path ? state.files.path : '';
    var html =
      '<button class="file-crumb" type="button" data-file-path="">' +
      esc(t('web.files.card')) +
      '</button>';
    var parts = path ? path.split('/') : [];
    var accumulated = '';
    for (var index = 0; index < parts.length; index += 1) {
      accumulated = accumulated ? accumulated + '/' + parts[index] : parts[index];
      html += '<span class="file-crumb-sep">/</span>';
      html +=
        '<button class="file-crumb" type="button" data-file-path="' +
        esc(accumulated) +
        '">' +
        esc(parts[index]) +
        '</button>';
    }
    $('fileBreadcrumb').innerHTML = html;
  }

  function renderFiles() {
    var data = state.files;
    if (!data) return;
    renderFileBreadcrumb();
    var entries = data.entries || [];
    $('fileUp').disabled = data.parent === null || data.parent === undefined;
    $('fileSummary').textContent = t('web.files.summary', {
      count: entries.length,
      path: data.path || t('web.files.root')
    });
    $('fileNotice').hidden = !data.truncated;
    $('fileNotice').textContent = data.truncated
      ? t('web.files.too_many', { count: data.max_entries || entries.length })
      : '';
    var root = $('fileList');
    if (!entries.length) {
      root.innerHTML =
        '<tr><td colspan="4" class="empty">' + esc(t('web.files.empty')) + '</td></tr>';
      applyWritable();
      return;
    }
    var html = '';
    for (var index = 0; index < entries.length; index += 1) {
      var entry = entries[index];
      var size = entry.is_dir ? esc(t('web.files.folder')) : fmtBytes(entry.size);
      var actions =
        '<button class="btn ghost small" type="button" data-file-open="' +
        esc(entry.path) +
        '">' +
        esc(entry.is_dir ? t('web.files.open') : t('web.files.preview_download')) +
        '</button>';
      if (!entry.is_dir) {
        actions +=
          '<button class="btn ghost small" type="button" data-file-download="' +
          esc(entry.path) +
          '">' + esc(t('web.files.download')) + '</button>';
      }
      actions +=
        '<button class="btn ghost small" type="button" data-file-write data-file-rename="' +
        esc(entry.path) +
        '">' + esc(t('web.files.rename')) + '</button>' +
        '<button class="btn ghost small" type="button" data-file-write data-file-move="' +
        esc(entry.path) +
        '">' + esc(t('web.files.move')) + '</button>' +
        '<button class="btn ghost small danger" type="button" data-file-write data-file-delete="' +
        esc(entry.path) +
        '">' + esc(t('web.files.delete')) + '</button>';
      html +=
        '<tr class="' +
        (entry.is_dir ? 'is-dir' : '') +
        '">' +
        '<td><button class="file-open" type="button" data-file-open="' +
        esc(entry.path) +
        '"><span class="file-icon">' +
        (entry.is_dir ? '📁' : '📄') +
        '</span>' +
        esc(entry.name) +
        '</button></td>' +
        '<td class="mono small">' +
        size +
        '</td>' +
        '<td class="muted small">' +
        (entry.modified ? fmtTime(entry.modified) : '—') +
        '</td>' +
        '<td><div class="file-actions">' +
        actions +
        '</div></td>' +
        '</tr>';
    }
    root.innerHTML = html;
    applyWritable();
  }

  function downloadBrowserFile(path) {
    var link = document.createElement('a');
    link.href = fileDownloadUrl(path);
    link.download = path.split('/').pop() || 'file';
    document.body.appendChild(link);
    link.click();
    link.remove();
  }

  function closeFilePreview() {
    var dialog = $('filePreviewDialog');
    if (dialog && dialog.open) dialog.close();
  }

  function openFilePreview(entry) {
    if (!entry || entry.is_dir) return;
    var dialog = $('filePreviewDialog');
    if (!dialog) return;
    $('filePreviewTitle').textContent = entry.name || t('web.files.preview_title');
    $('filePreviewMeta').textContent =
      entry.path + ' · ' + fmtBytes(entry.size) + ' · ' + (entry.modified ? fmtTime(entry.modified) : '—');
    $('filePreviewDownload').href = fileDownloadUrl(entry.path);
    $('filePreviewDownload').setAttribute('download', entry.name || 'file');
    var image = $('filePreviewImage');
    var text = $('filePreviewText');
    image.hidden = true;
    image.removeAttribute('src');
    text.hidden = true;
    text.textContent = '';
    if (fileIsImage(entry)) {
      image.hidden = false;
      image.alt = entry.name || '';
      image.src = cacheBust('/api/files/view?path=' + encodePath(entry.path));
    } else {
      text.hidden = false;
      text.textContent = t('web.common.reading');
      api('/api/files/text?path=' + encodePath(entry.path))
        .then(function (data) {
          text.textContent = data.binary
            ? t('web.files.binary')
            : data.content || '';
        })
        .catch(function (error) {
          text.textContent = t('web.files.preview_failed', {
            message: error.message || t('web.error.unknown')
          });
        });
    }
    if (!dialog.open) {
      if (dialog.showModal) dialog.showModal();
      else dialog.setAttribute('open', 'open');
    }
  }

  function openBrowserFile(path) {
    var entry = findFileEntry(path);
    if (!entry) {
      loadFiles(path);
      return;
    }
    if (entry.is_dir) {
      loadFiles(entry.path);
    } else if (fileIsImage(entry) || fileIsText(entry)) {
      openFilePreview(entry);
    } else {
      downloadBrowserFile(entry.path);
    }
  }

  function renameBrowserFile(path) {
    var entry = findFileEntry(path);
    if (!entry) return;
    var name = window.prompt(t('web.files.rename_prompt'), entry.name || '');
    if (!name || name === entry.name) return;
    apiWrite('/api/files/rename', { path: entry.path, name: name })
      .then(function () {
        toast(t('web.files.renamed'), 'ok');
        loadFiles();
      })
      .catch(function (error) {
        showError(error, t('web.error.rename_failed'));
      });
  }

  function moveBrowserFile(path) {
    var entry = findFileEntry(path);
    if (!entry) return;
    var dest = window.prompt(t('web.files.move_prompt'), state.files.path || '');
    if (dest === null || dest === undefined) return;
    dest = String(dest).replace(/^\/+|\/+$/g, '');
    apiWrite('/api/files/move', { path: entry.path, dest: dest })
      .then(function () {
        toast(t('web.files.moved'), 'ok');
        loadFiles();
      })
      .catch(function (error) {
        showError(error, t('web.error.move_failed'));
      });
  }

  function deleteBrowserFile(path) {
    var entry = findFileEntry(path);
    if (!entry) return;
    var message = entry.is_dir
      ? t('web.files.delete_confirm_dir', { name: entry.name })
      : t('web.files.delete_confirm', { name: entry.name });
    if (!window.confirm(message)) return;
    apiWrite('/api/files/delete', { path: entry.path, recursive: !!entry.is_dir })
      .then(function () {
        toast(t('web.files.deleted'), 'ok');
        loadFiles();
      })
      .catch(function (error) {
        showError(error, t('web.error.delete_failed'));
      });
  }

  function createBrowserFolder() {
    if (!isWritable()) {
      toast(t('web.files.readonly_mkdir'), 'warn');
      return;
    }
    var name = window.prompt(t('web.files.mkdir_prompt'));
    if (!name) return;
    apiWrite('/api/files/mkdir', { path: state.files.path || '', name: name })
      .then(function () {
        toast(t('web.files.created'), 'ok');
        loadFiles();
      })
      .catch(function (error) {
        showError(error, t('web.error.mkdir_failed'));
      });
  }

  function uploadBrowserFile(file, onProgress) {
    if (file.size > MAX_UPLOAD_BYTES) {
      return Promise.reject(new Error(t('web.files.too_large', {
        name: file.name,
        limit: uploadLimitText()
      })));
    }
    var target = (state.files.path ? state.files.path + '/' : '') + file.name;
    return uploadWithProgress(
      'POST',
      '/api/files/upload?path=' + encodePath(target),
      file,
      'application/octet-stream',
      onProgress
    );
  }

  function uploadBrowserFiles(files) {
    if (!isWritable()) {
      toast(t('web.files.readonly_upload'), 'warn');
      return;
    }
    var list = Array.prototype.slice.call(files || []).filter(function (file) {
      return file && file.name;
    });
    if (!list.length) return;
    var current = (state.files && state.files.entries) || [];
    var conflicts = list.filter(function (file) {
      for (var index = 0; index < current.length; index += 1) {
        if (!current[index].is_dir && current[index].name === file.name) return true;
      }
      return false;
    });
    if (conflicts.length && !window.confirm(
      t('web.files.conflict_confirm', {
        names: conflicts
          .map(function (file) { return file.name; })
          .join(t('web.common.name_separator'))
      })
    )) {
      $('fileUploadInput').value = '';
      return;
    }
    if (!beginUpload(
      t('web.files.uploading_files'),
      t('web.files.file_count', { count: list.length })
    )) {
      $('fileUploadInput').value = '';
      return;
    }
    state.filesBusy = true;
    applyWritable();
    var totalBytes = list.reduce(function (sum, file) {
      return sum + (file.size || 0);
    }, 0);
    var doneBytes = 0;
    var index = 0;
    var chain = Promise.resolve();
    list.forEach(function (file) {
      chain = chain.then(function () {
        index += 1;
        if ($('uploadProgressTitle')) {
          $('uploadProgressTitle').textContent = t('web.files.uploading_index', {
            index: index,
            total: list.length
          });
        }
        if ($('uploadProgressFile')) $('uploadProgressFile').textContent = file.name;
        return uploadBrowserFile(file, function (loaded, total) {
          setUploadProgress(doneBytes + loaded, totalBytes || total);
        }).then(function (data) {
          doneBytes += file.size || 0;
          setUploadProgress(doneBytes, totalBytes);
          return data;
        });
      });
    });
    chain
      .then(function () {
        toast(t('web.files.uploaded_count', { count: list.length }), 'ok');
        loadFiles();
      })
      .catch(function (error) {
        showError(error, t('web.error.upload_failed'));
      })
      .then(function () {
        state.filesBusy = false;
        applyWritable();
        $('fileUploadInput').value = '';
        endUpload();
      });
  }

  function bindFileBrowser() {
    $('fileList').addEventListener('click', function (event) {
      var target = event.target;
      while (target && target !== this && !target.getAttribute) target = target.parentNode;
      if (!target || target === this) return;
      var open = target.getAttribute('data-file-open');
      var download = target.getAttribute('data-file-download');
      var rename = target.getAttribute('data-file-rename');
      var move = target.getAttribute('data-file-move');
      var remove = target.getAttribute('data-file-delete');
      if (open) openBrowserFile(open);
      else if (download) downloadBrowserFile(download);
      else if (rename) renameBrowserFile(rename);
      else if (move) moveBrowserFile(move);
      else if (remove) deleteBrowserFile(remove);
    });
    $('fileBreadcrumb').addEventListener('click', function (event) {
      var target = event.target;
      while (target && target !== this && !target.getAttribute) target = target.parentNode;
      if (!target || target === this) return;
      var path = target.getAttribute('data-file-path');
      if (path !== null) loadFiles(path);
    });
    $('fileUp').addEventListener('click', function () {
      if (state.files && state.files.parent !== null && state.files.parent !== undefined) {
        loadFiles(state.files.parent);
      }
    });    $('fileRefresh').addEventListener('click', function () {
      loadFiles();
    });
    $('fileNewFolder').addEventListener('click', createBrowserFolder);
    $('fileUpload').addEventListener('click', function () {
      if (!isWritable()) {
        toast(t('web.files.readonly_upload'), 'warn');
        return;
      }
      $('fileUploadInput').click();
    });
    $('fileUploadInput').addEventListener('change', function () {
      uploadBrowserFiles(this.files);
    });
    $('filePreviewClose').addEventListener('click', closeFilePreview);
    $('filePreviewDialog').addEventListener('click', function (event) {
      if (event.target === this) closeFilePreview();
    });
  }

  function stateUrl(unit, name, extension) {
    return (
      '/api/file?path=' +
      encodePath(
        'States/' +
          unit.platform +
          '/' +
          unit.emulator +
          '/' +
          unit.game +
          '/' +
          name +
          '.' +
          extension
      )
    );
  }

  function deleteSave(path, label) {
    if (!isWritable()) {
      toast(t('web.saves.readonly_delete'), 'warn');
      return;
    }
    if (!path) return;
    var message = t('web.saves.delete_confirm', { label: label });
    if (!window.confirm(message)) return;
    apiWrite('/api/saves/delete?path=' + encodePath(path))
      .then(function () {
        toast(t('web.saves.deleted', { label: label }), 'ok', 6000);
        return loadSaves();
      })
      .catch(function (error) {
        showError(error, t('web.saves.delete_failed'));
      });
  }

  function savesSummary(view) {
    var text = t('web.saves.summary_rendered', {
      rendered: view.rendered,
      total: view.total,
      states: view.states,
      battery: view.battery
    });
    if (view.rendered >= view.total) {
      text = t('web.saves.summary', {
        total: view.total,
        states: view.states,
        battery: view.battery
      });
    }
    $('savesSummary').textContent = text;
  }

  function renderSavesBatch(view) {
    if (state.savesView !== view) return;
    var container = $('savesList');
    if (!container) return;
    detachListMore(view);
    var end = Math.min(view.rendered + SAVE_BATCH, view.total);
    var html = [];
    for (var index = view.rendered; index < end; index += 1) {
      html.push(renderUnit(view.units[index]));
    }
    var more = container.querySelector('.list-more');
    if (more) more.insertAdjacentHTML('beforebegin', html.join(''));
    else container.insertAdjacentHTML('beforeend', html.join(''));
    if (more) more.remove();
    view.rendered = end;
    savesSummary(view);
    if (view.rendered < view.total) {
      container.insertAdjacentHTML(
        'beforeend',
        listMoreHtml(t('web.saves.load_more'), view.rendered, view.total)
      );
      watchListMore(container, view, renderSavesBatch);
    }
    applyWritable();
  }

  function renderSaves() {
    var container = $('savesList');
    var units = (state.saves && state.saves.units) || [];
    detachListMore(state.savesView);
    state.savesView = null;
    if (!units.length) {
      container.innerHTML =
        '<div class="empty">' + esc(t('web.saves.none')) + '</div>';
      $('savesSummary').textContent = t('web.saves.summary_zero');
      return;
    }
    var states = 0;
    var battery = 0;
    for (var index = 0; index < units.length; index += 1) {
      states += (units[index].states || []).length;
      battery += (units[index].battery || []).length;
    }
    var view = {
      units: units,
      total: units.length,
      rendered: 0,
      states: states,
      battery: battery,
      observer: null
    };
    state.savesView = view;
    container.innerHTML = '';
    renderSavesBatch(view);
  }

  function renderUnit(unit) {
    var writable = isWritable();
    var disabled = writable ? '' : ' disabled';
    var emulator = unit.emulator || t('web.saves.unknown_core');
    var playtimeLabel = fmtDuration(unit.playtime_total_seconds);
    var playtimeTitle = t('web.saves.playtime_title', {
      local: fmtDuration(unit.playtime_seconds),
      total: fmtDuration(unit.playtime_total_seconds)
    });
    var head =
      '<header class="unit-head">' +
      '<div class="unit-title">' +
      '<b>' +
      esc(unit.game) +
      '</b>' +
      '<span class="pill">' +
      esc(unit.platform) +
      ' · ' +
      esc(emulator) +
      '</span>' +
      '<span class="unit-playtime" title="' +
      esc(playtimeTitle) +
      '">' +
      esc(t('web.saves.playtime', { duration: playtimeLabel })) +
      '</span>' +
      '</div>' +
      '<div class="row-actions" style="margin:0 0 0 auto">' +
      '<button class="btn primary small" type="button" data-action="sync" ' +
      'data-platform="' +
      esc(unit.platform) +
      '" data-emulator="' +
      esc(unit.emulator) +
      '" data-game="' +
      esc(unit.game) +
      '"' +
      disabled +
      '>' + esc(t('web.saves.sync_now')) + '</button>' +
      '</div>' +
      '</header>';

    var stateCards = (unit.states || [])
      .map(function (item) {
        return renderStateCard(unit, item, writable);
      })
      .join('');
    if (!stateCards) {
      stateCards = '<div class="empty">' + esc(t('web.saves.no_states')) + '</div>';
    }

    var batteryBlock = '';
    if ((unit.battery || []).length) {
      var rows = unit.battery
        .map(function (file) {
          return (
            '<div class="battery-item">' +
            '<span class="pill battery">' + esc(t('web.saves.battery')) + '</span>' +
            '<div class="grow"><b>' +
            esc(file.name) +
            '</b><div class="muted">' +
            fmtBytes(file.size) +
            ' · ' +
            fmtTime(file.mtime) +
            '</div></div>' +
            '<a class="btn small" href="/api/file?path=' +
            encodePath(file.path) +
            '" download>' + esc(t('web.files.download')) + '</a>' +
            '<button class="btn small" type="button" data-action="upload" ' +
            'data-path="' +
            esc(file.path) +
            '" data-label="' +
            esc(file.name) +
            '"' +
            disabled +
            '>' + esc(t('web.saves.upload')) + '</button>' +
            '<button class="btn danger small" type="button" data-action="delete-save" ' +
            'data-path="' +
            esc(file.path) +
            '" data-label="' +
            esc(file.name) +
            '"' +
            disabled +
            '>' + esc(t('web.files.delete')) + '</button>' +
            '</div>'
          );
        })
        .join('');
      batteryBlock =
        '<div class="battery-block"><h3>' + esc(t('web.overview.battery')) + '</h3><div class="battery-list">' +
        rows +
        '</div></div>';
    }

    return (
      '<article class="unit">' +
      head +
      '<div class="unit-body"><div class="states">' +
      stateCards +
      '</div>' +
      batteryBlock +
      '</div></article>'
    );
  }

  function renderStateCard(unit, item, writable) {
    var disabled = writable ? '' : ' disabled';
    var thumb;
    if (item.thumb) {
      thumb =
        '<img class="thumb" loading="lazy" alt="' +
        esc(t('web.saves.thumb_alt', { name: item.name })) +
        '" src="' +
        esc(item.thumb) +
        '">';
    } else {
      thumb = '<div class="thumb-placeholder">' + esc(t('web.saves.no_thumb')) + '</div>';
    }
    var name = item.resume ? 'resume' : item.name;
    var tag = item.resume
      ? '<span class="pill resume">' + esc(t('web.saves.resume')) + '</span>'
      : '<span class="pill dim">' + esc(t('web.saves.history')) + '</span>';
    var statePath = stateUrl(unit, item.name, 'state');
    return (
      '<div class="state-card">' +
      thumb +
      '<div class="state-info">' +
      '<div class="state-name"><b>' +
      esc(name) +
      '</b>' +
      tag +
      '</div>' +
      '<div class="state-meta">' +
      fmtBytes(item.size) +
      ' · ' +
      fmtTime(item.mtime) +
      '</div>' +
      '<div class="state-actions">' +
      '<a class="btn small" href="' +
      statePath +
      '" download>' + esc(t('web.files.download')) + '</a>' +
      '<button class="btn small" type="button" data-action="upload" ' +
      'data-path="' +
      esc('States/' + unit.platform + '/' + unit.emulator + '/' + unit.game + '/' + item.name + '.state') +
      '" data-label="' +
      esc(unit.game + ' / ' + name + '.state') +
      '"' +
      disabled +
      '>' + esc(t('web.saves.upload')) + '</button>' +
      '<button class="btn danger small" type="button" data-action="delete-save" ' +
      'data-path="' +
      esc('States/' + unit.platform + '/' + unit.emulator + '/' + unit.game + '/' + item.name + '.state') +
      '" data-label="' +
      esc(unit.game + ' / ' + name + '.state') +
      '"' +
      disabled +
      '>' + esc(t('web.files.delete')) + '</button>' +
      '</div>' +
      '</div>' +
      '</div>'
    );
  }

  // -------------------------------------------------------------- 云同步页

  function loadSync() {
    return api('/api/sync')
      .then(function (info) {
        state.sync = info;
        if (!state.formFilled) {
          fillStorages(info);
          state.formFilled = true;
        }
        renderCaches(info);
        renderSyncLog(info.log_tail || []);
        if (info && info.log_level) {
          state.logLevel = info.log_level;
          var select = $('logLevel');
          if (select) select.value = info.log_level;
        }
      })
      .catch(function (error) {
        $('cacheList').innerHTML =
          '<div class="empty">' + esc(t('web.sync.read_failed')) + '</div>';
        showError(error, t('web.sync.read_failed'));
      });
  }

  function fillStorages(info) {
    var profiles = (info && info.storages) || [];
    if (!profiles.length) {
      var webdav = (info && info.webdav) || {};
      profiles = [{
        id: 'default',
        name: t('web.sync.default_name'),
        kind: 'webdav',
        url: webdav.url || '',
        remote_root: webdav.remote_root || '',
        username: webdav.username || '',
        password_set: !!webdav.password_set,
        ca_cert: webdav.ca_cert || '',
        insecure: !!webdav.insecure
      }];
    }
    state.profiles = profiles.map(function (profile) {
      return {
        id: profile.id,
        name: profile.name || profile.id,
        kind: profile.kind || 'webdav',
        url: profile.url || '',
        remote_root: profile.remote_root || '',
        username: profile.username || '',
        password_set: !!profile.password_set,
        ca_cert: profile.ca_cert || '',
        insecure: !!profile.insecure,
        password: '',
        clear_password: false
      };
    });
    state.activeStorage = (info && info.active_storage) || state.profiles[0].id;
    if (!byStorageId(state.activeStorage)) state.activeStorage = state.profiles[0].id;
    state.selectedStorage = state.activeStorage;
    $('wdKeepRemote').checked = !!(info && info.keep_remote);
    renderStorageList();
    fillWebdavForm(byStorageId(state.selectedStorage));
    var warning = $('storageWarning');
    if (warning) {
      warning.hidden = !(info && info.storages_warning);
      warning.textContent = (info && info.storages_warning) || '';
    }
  }

  function byStorageId(id) {
    for (var index = 0; index < state.profiles.length; index += 1) {
      if (state.profiles[index].id === id) return state.profiles[index];
    }
    return null;
  }

  function selectedProfile() {
    var profile = byStorageId(state.selectedStorage);
    return profile || state.profiles[0] || null;
  }

  function syncConfigPayload() {
    return {
      active: state.activeStorage,
      keep_remote: checked('wdKeepRemote'),
      profiles: state.profiles.map(function (profile) {
        var item = {
          id: profile.id,
          name: profile.name,
          kind: profile.kind || 'webdav',
          url: profile.url,
          remote_root: profile.remote_root,
          username: profile.username,
          ca_cert: profile.ca_cert,
          insecure: !!profile.insecure,
          clear_password: !!profile.clear_password
        };
        if (profile.password) item.password = profile.password;
        return item;
      })
    };
  }

  function switchActiveStorage(id) {
    if (state.syncSwitchBusy || !id) return;
    if (id === state.activeStorage) return;
    if (!isWritable()) {
      toast(t('web.sync.readonly_switch'), 'warn');
      renderStorageList();
      return;
    }
    captureWebdavForm();
    var previousActive = state.activeStorage;
    var previousSelected = state.selectedStorage;
    var profile = byStorageId(id);
    if (!profile) return;
    state.activeStorage = id;
    state.selectedStorage = id;
    renderStorageList();
    fillWebdavForm(profile);
    state.syncSwitchBusy = true;
    applyWritable();
    apiWrite('/api/sync/config', syncConfigPayload())
      .then(function () {
        toast(t('web.sync.switched', { name: profile.name || id }), 'ok', 5000);
        state.formFilled = false;
        return loadSync();
      })
      .catch(function (error) {
        state.activeStorage = previousActive;
        state.selectedStorage = previousSelected;
        renderStorageList();
        fillWebdavForm(byStorageId(previousSelected));
        showError(error, t('web.sync.switch_failed'));
      })
      .then(function () {
        state.syncSwitchBusy = false;
        applyWritable();
      });
  }

  function captureWebdavForm() {
    var profile = selectedProfile();
    if (!profile) return;
    profile.name = value('wdName').trim();
    profile.url = value('wdUrl').trim();
    profile.remote_root = value('wdRemoteRoot').trim();
    profile.username = value('wdUsername').trim();
    profile.ca_cert = value('wdCaCert').trim();
    profile.insecure = checked('wdInsecure');
    profile.clear_password = checked('wdClearPassword');
    var password = value('wdPassword');
    if (password) profile.password = password;
  }

  function fillWebdavForm(profile) {
    profile = profile || selectedProfile();
    if (!profile) return;
    $('wdName').value = profile.name || '';
    $('wdUrl').value = profile.url || '';
    $('wdRemoteRoot').value = profile.remote_root || '';
    $('wdUsername').value = profile.username || '';
    $('wdCaCert').value = profile.ca_cert || '';
    $('wdInsecure').checked = !!profile.insecure;
    $('wdClearPassword').checked = !!profile.clear_password;
    $('wdPassword').value = '';
    profile.clear_password = !!profile.clear_password;
    $('wdPasswordState').textContent = profile.password_set
      ? t('web.sync.password_saved')
      : t('web.sync.password_empty');
  }

  function storageId() {
    return 'p-' + Date.now().toString(36) + '-' + Math.floor(Math.random() * 1679616).toString(36);
  }

  function renderStorageList() {
    var container = $('storageList');
    if (!container) return;
    if (!state.profiles.length) {
      container.innerHTML =
        '<div class="empty">' + esc(t('web.sync.no_storages')) + '</div>';
      return;
    }
    var rows = [];
    for (var index = 0; index < state.profiles.length; index += 1) {
      var profile = state.profiles[index];
      var active = profile.id === state.activeStorage;
      var selected = profile.id === state.selectedStorage;
      rows.push(
        '<div class="storage-item' + (selected ? ' selected' : '') + '" data-storage-id="' + esc(profile.id) + '">' +
          '<label class="check storage-active"><input type="radio" name="storageActive" value="' +
          esc(profile.id) + '"' + (active ? ' checked' : '') + '><span>' +
          (active ? esc(t('web.sync.active')) : esc(t('web.sync.use_this'))) + '</span></label>' +
          '<button class="btn ghost small" type="button" data-edit-storage="' + esc(profile.id) + '">' +
          esc(t('web.sync.edit_button')) + '</button>' +
          '<div class="storage-meta"><strong>' + esc(profile.name || profile.id) + '</strong>' +
          '<span class="storage-kind">' + esc(profile.kind || 'webdav') + '</span>' +
          '<span class="storage-url">' + esc(profile.url || t('web.sync.no_url')) + '</span></div>' +
        '</div>'
      );
    }
    container.innerHTML = rows.join('');
  }

  function renderCaches(info) {
    var caches = (info && info.caches) || [];
    var container = $('cacheList');
    if (!caches.length) {
      container.innerHTML =
        '<div class="empty">' + esc(t('web.sync.no_caches')) + '</div>';
      return;
    }
    var rows = [];
    for (var index = 0; index < caches.length; index += 1) {
      var cache = caches[index];
      var unitLabel =
        esc(cache.game || t('web.sync.unknown_unit')) +
        ' <span class="muted">· ' +
        esc((cache.platform || '?') + ' / ' + (cache.emulator || t('web.saves.unknown_core'))) +
        '</span>';
      var remote = cache.remote_match
        ? '<span class="pill resume">' + esc(t('web.sync.remote_match')) + '</span>'
        : '<span class="pill warn">' + esc(t('web.sync.remote_moved')) + '</span>';
      var action = '';
      if (cache.platform && cache.emulator && cache.game) {
        action =
          '<button class="btn small" type="button" data-action="sync" ' +
          'data-platform="' +
          esc(cache.platform) +
          '" data-emulator="' +
          esc(cache.emulator) +
          '" data-game="' +
          esc(cache.game) +
          '"' +
          (isWritable() ? '' : ' disabled') +
          '>' + esc(t('web.saves.sync_now')) + '</button>';
      }
      rows.push(
        '<tr><td class="unit-cell">' +
          unitLabel +
          '</td><td>' +
          esc(String(cache.version || 0)) +
          '</td><td>' +
          esc(fmtSyncTime(cache.synced_at)) +
          '</td><td>' +
          remote +
          '</td><td>' +
          action +
          '</td></tr>'
      );
    }
    container.innerHTML =
      '<table><thead><tr><th>' + esc(t('web.sync.table_unit')) +
      '</th><th>' + esc(t('web.sync.table_version')) +
      '</th><th>' + esc(t('web.sync.table_synced')) +
      '</th><th>' + esc(t('web.sync.table_remote')) +
      '</th><th></th></tr></thead><tbody>' +
      rows.join('') +
      '</tbody></table>';
    applyWritable();
  }

  function renderSyncLog(lines) {
    $('syncLog').textContent = lines.length ? lines.join('\n') : t('web.common.noLog');
  }

  function showSyncOutput(result) {
    var lines = [];
    if (result && typeof result.code === 'number') {
      lines.push(result.ok
        ? t('web.sync.exit_ok', { code: result.code })
        : t('web.sync.exit_fail', { code: result.code }));
    }
    if (result && result.hint) lines.push(result.hint);
    if (result && result.output) lines.push('', result.output);
    $('syncOutput').textContent = lines.length ? lines.join('\n') : '—';
    var card = $('syncOutput');
    if (card && card.scrollIntoView) card.scrollIntoView({ block: 'nearest' });
  }

  function runSync(unit, button) {
    if (!isWritable()) {
      toast(t('web.sync.readonly_run'), 'warn');
      return;
    }
    if (!unit || !unit.platform || !unit.emulator || !unit.game) {
      toast(t('web.sync.missing_unit'), 'error');
      return;
    }
    var label = unit.game;
    if (button) {
      button.disabled = true;
      button.textContent = t('web.sync.syncing');
    }
    toast(t('web.sync.running', { name: label }), 'ok', 1800);
    apiWrite('/api/sync/run', {
      platform: unit.platform,
      emulator: unit.emulator,
      game: unit.game
    })
      .then(function (result) {
        showSyncOutput(result);
        if (result && result.ok) {
          toast(t('web.sync.done', { name: label }), 'ok');
        } else if (result && result.code === 2) {
          toast(result.hint || t('web.sync.conflict'), 'warn', 8000);
        } else if (result && result.code === 3) {
          toast(result.hint || t('web.sync.config_error'), 'error', 8000);
        } else {
          toast(t('web.sync.failed', { code: result ? result.code : '?' }), 'error', 6000);
        }
        loadSync();
      })
      .catch(function (error) {
        showSyncOutput({ hint: error.message });
        showError(error, t('web.sync.request_failed'));
      })
      .then(function () {
        if (button) {
          button.disabled = !isWritable();
          button.textContent = t('web.saves.sync_now');
        }
      });
  }

  function modsList() {
    return (state.mods && state.mods.mods) || [];
  }

  function modsTimestamp() {
    function pad(value) {
      return (value < 10 ? '0' : '') + value;
    }
    var now = new Date();
    return (
      now.getFullYear() +
      pad(now.getMonth() + 1) +
      pad(now.getDate()) +
      '-' +
      pad(now.getHours()) +
      pad(now.getMinutes()) +
      pad(now.getSeconds())
    );
  }

  function downloadBlob(filename, blob) {
    var url = URL.createObjectURL(blob);
    var anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = filename;
    document.body.appendChild(anchor);
    anchor.click();
    document.body.removeChild(anchor);
    window.setTimeout(function () {
      URL.revokeObjectURL(url);
    }, 1500);
  }

  var MOD_TIMINGS = [
    'before_upload',
    'after_upload',
    'before_download',
    'after_download',
    'after_sync',
    'after_boot',
    'manual'
  ];
  var MOD_IMPORT_MAX_BYTES = 24 * 1024 * 1024;
  var MOD_MAX_SCRIPT_BYTES = 262144;
  var MOD_VERSION_PATTERN = /^[A-Za-z0-9._+-]{1,32}$/;
  var MOD_ACTION_ID_PATTERN = /^[a-z0-9][a-z0-9_-]{0,63}$/;

  function modVersionProblem(version) {
    var text = String(version || '').trim();
    if (!text) return t('web.mod.version_empty');
    if (!MOD_VERSION_PATTERN.test(text)) {
      return t('web.mod.version_invalid');
    }
    return '';
  }

  function utf8Bytes(text) {
    if (window.Blob) {
      try {
        return new Blob([text]).size;
      } catch (error) {
        /* fall through */
      }
    }
    try {
      return unescape(encodeURIComponent(text)).length;
    } catch (error) {
      return String(text).length;
    }
  }

  function loadMods() {
    var summary = $('modsSummary');
    if (summary) summary.textContent = t('web.common.reading');
    return api('/api/mods')
      .then(function (data) {
        state.mods = data || {};
        renderMods();
        renderModsRuntime();
        renderModsLog();
        if (state.selectedMod && state.selectedMod.id) {
          openModDetail(state.selectedMod.id, true).catch(function () {});
        }
      })
      .catch(function (error) {
        if (summary) summary.textContent = t('web.overview.read_failed');
        var list = $('modList');
        if (list) {
          list.innerHTML =
            '<tr><td colspan="2" class="empty">' + esc(t('web.mod.read_retry')) + '</td></tr>';
        }
        showError(error, t('web.mod.read_failed'));
      });
  }

  function findMod(id) {
    var list = modsList();
    for (var index = 0; index < list.length; index += 1) {
      if (list[index].id === id) return list[index];
    }
    return null;
  }

  var MOD_GROUPS = [
    { key: 'sync', labelKey: 'web.mod.category.sync' },
    { key: 'info', labelKey: 'web.mod.category.info' },
    { key: 'tool', labelKey: 'web.mod.category.tool' },
    { key: 'theme', labelKey: 'web.mod.category.theme' },
    { key: 'other', labelKey: 'web.mod.category.other' }
  ];

  function modGroups(list) {
    var buckets = {};
    list.forEach(function (entry) {
      var key = entry.category || 'other';
      if (!buckets[key]) buckets[key] = [];
      buckets[key].push(entry);
    });
    var out = [];
    MOD_GROUPS.forEach(function (group) {
      if (buckets[group.key] && buckets[group.key].length) {
        out.push({
          key: group.key,
          label: t(group.labelKey),
          entries: buckets[group.key]
        });
      }
    });
    return out;
  }

  function renderMods() {
    var container = $('modList');
    if (!container) return;
    var list = modsList();
    var enabled = 0;
    for (var n = 0; n < list.length; n += 1) {
      if (list[n].enabled) enabled += 1;
    }
    var summary = $('modsSummary');
    if (summary) {
      summary.textContent = list.length
        ? t('web.mods.summary', { total: list.length, enabled: enabled })
        : t('web.mods.empty_summary');
    }
    if (!list.length) {
      container.innerHTML =
        '<tr><td colspan="3" class="empty">' + esc(t('web.mods.empty')) + '</td></tr>';
      renderModsControls();
      return;
    }
    var groups = modGroups(list);
    var rows = [];
    for (var g = 0; g < groups.length; g += 1) {
      var group = groups[g];
      var closed = state.modGroupsClosed[group.key] === true;
      rows.push(
        '<tr class="mod-group-row" data-mod-group="' +
          esc(group.key) +
          '"><td colspan="3"><span class="mod-group-toggle">' +
          (closed ? '\u25b6' : '\u25bc') +
          '</span><span class="mod-group-name">' +
          esc(group.label) +
          '</span><span class="pill dim">' +
          group.entries.length +
          '</span></td></tr>'
      );
      if (closed) continue;
      for (var index = 0; index < group.entries.length; index += 1) {
        var entry = group.entries[index];
      var warnings = [];
      if (entry.error) {
        warnings.push(
          '<span class="pill warn" title="' +
            esc(entry.error) +
            '">' + esc(t('web.mod.needs_fix')) + '</span>'
        );
      }
      if (!entry.has_ui) {
        warnings.push(
          '<span class="pill warn" title="' + esc(t('web.mod.no_ui_title')) + '">' +
            esc(t('web.mod.no_ui')) + '</span>'
        );
      }
      if (entry.legacy_actions > 0) {
        warnings.push(
          '<span class="pill warn" title="' + esc(t('web.mod.legacy_caps_title')) + '">' +
            esc(t('web.mod.legacy_caps', { count: entry.legacy_actions })) + '</span>'
        );
      } else if (entry.action_count > 0) {
        warnings.push(
          '<span class="pill ok" title="' + esc(t('web.mod.caps_declared_title')) + '">' +
            esc(t('web.mod.caps_declared')) + '</span>'
        );
      }
      var warning = warnings.length
        ? '<div class="mod-sub mod-error">' + warnings.join(' ') + '</div>'
        : '';
      rows.push(
        '<tr data-mod-id="' +
          esc(entry.id) +
          '"' +
          (entry.error ? ' data-mod-error="1"' : '') +
          (!entry.has_ui ? ' data-mod-no-ui="1"' : '') +
          '>' +
          '<td class="unit-cell mod-name"><strong>' +
          esc(entry.name || entry.id) +
          '</strong><div class="mod-sub mono">v' +
          esc(entry.version || '1.0.0') +
          '</div>' +
          warning +
          '</td>' +
          '<td class="mod-author muted small">' +
          esc(entry.author || t('web.mod.noAuthor')) +
          '</td>' +
          '<td><label class="check mod-enable"><input type="checkbox" data-mod-toggle="' +
          esc(entry.id) +
          '"' +
          (entry.enabled ? ' checked' : '') +
          (entry.error ? ' data-mod-error="1"' : '') +
          (!entry.has_ui ? ' data-mod-no-ui="1"' : '') +
          '><span>' +
          (entry.enabled ? esc(t('web.mod.enabled')) : esc(t('web.mod.disabled'))) +
          '</span></label></td>' +
          '</tr>'
        );
      }
    }
    container.innerHTML = rows.join('');
    renderModsControls();
  }

  function renderModsRuntime() {
    var data = state.mods || {};
    var runtime = $('modsRuntimeBanner');
    if (runtime) {
      runtime.hidden = !!data.runtime_supported;
      if (!data.runtime_supported) {
        runtime.textContent =
          data.runtime_hint || t('web.mods.runtime_missing');
      }
    }
  }

  function renderModsLog() {
    var log = $('modsLog');
    if (!log) return;
    var lines = (state.mods && state.mods.log_tail) || [];
    log.textContent = lines.length ? lines.join('\n') : t('web.common.noLog');
  }

  function openModDetail(id, quiet) {
    return api('/api/mods/item?id=' + encodeURIComponent(id))
      .then(function (detail) {
        state.selectedMod = detail;
        fillModDetail(detail);
        var dialog = $('modDetailDialog');
        if (dialog && !dialog.open) dialog.showModal();
        return loadModDetailViews(detail);
      })
      .catch(function (error) {
        showError(error, t('web.mod.detail_failed'));
        if (error && error.status === 404) {
          state.selectedMod = null;
          closeModDetail();
          loadMods().catch(function () {});
        }
        if (!quiet) throw error;
      });
  }

  function fillModDetail(detail) {
    detail = detail || {};
    $('modDetailTitle').textContent = detail.name || t('web.mod.detail');
    $('modDetailId').textContent = detail.id || '';
    $('modDetailVersion').textContent = 'v' + (detail.version || '1.0.0');
    $('modDetailAuthor').textContent = detail.author || t('web.mod.noAuthor');
    var status = detail.has_ui ? t('web.mod.ui_ready') : t('web.mod.ui_missing');
    if (detail.error) {
      status = t('web.mod.status_error', { status: status, message: detail.error });
    }
    $('modDetailStatus').textContent = status;
    $('modReadme').textContent = t('web.mod.readingReadme');
    renderModsControls();
  }

  function loadModDetailViews(detail) {
    var id = detail.id;
    // The file browser already owns directory listing.  Read only the README
    // here so opening a mod detail is one small request instead of a full tree
    // listing plus a second file request.
    return api(
      '/api/mods/file?id=' +
        encodeURIComponent(id) +
        '&path=' +
        encodeURIComponent('README.md')
    )
      .then(function (file) {
        $('modReadme').textContent =
          (file && file.content) || t('web.mod.no_readme');
      })
      .catch(function (error) {
        if (error && error.status === 404) {
          $('modReadme').textContent = t('web.mod.no_readme');
          return;
        }
        $('modReadme').textContent = t('web.mod.readme_failed');
        showError(error, t('web.mod.readme_read_failed'));
      });
  }

  function closeModDetail() {
    var dialog = $('modDetailDialog');
    if (dialog && dialog.open) dialog.close();
    state.selectedMod = null;
  }

  function uninstallMod() {
    if (!state.selectedMod) return;
    if (!isWritable()) {
      toast(t('web.mod.readonly_uninstall'), 'warn');
      return;
    }
    if (state.modsBusy) return;
    var name = state.selectedMod.name || state.selectedMod.id;
    if (
      !window.confirm(t('web.mod.uninstall_confirm', {
        name: name,
        id: state.selectedMod.id
      }))
    ) {
      return;
    }
    state.modsBusy = true;
    renderModsControls();
    apiWrite('/api/mods/uninstall', { id: state.selectedMod.id })
      .then(function () {
        state.selectedMod = null;
        closeModDetail();
        toast(t('web.mod.uninstalled'), 'ok', 5000);
        return loadMods();
      })
      .catch(function (error) {
        showError(error, t('web.mod.uninstall_failed'));
      })
      .then(function () {
        state.modsBusy = false;
        renderModsControls();
      });
  }

  function toggleMod(id, enabled, checkbox) {
    if (!isWritable()) {
      toast(t('web.mod.readonly_toggle'), 'warn');
      return;
    }
    if (state.modsBusy) return;
    state.modsBusy = true;
    if (checkbox) checkbox.disabled = true;
    renderModsControls();
    apiWrite('/api/mods/toggle', { id: id, enabled: enabled })
      .then(function (detail) {
        toast(enabled ? t('web.mod.enabled_toast') : t('web.mod.disabled_toast'), 'ok');
        if (state.selectedMod && state.selectedMod.id === id) {
          state.selectedMod = detail;
          fillModDetail(detail);
          loadModDetailViews(detail).catch(function () {});
        }
        return loadMods();
      })
      .catch(function (error) {
        if (checkbox) checkbox.checked = !enabled;
        showError(error, t('web.mod.toggle_failed'));
      })
      .then(function () {
        state.modsBusy = false;
        renderModsControls();
      });
  }

  function openModUi() {
    if (!state.selectedMod) return;
    if (!state.selectedMod.has_ui) {
      toast(t('web.mod.ui_missing_toast'), 'warn');
      return;
    }
    state.modUiMod = state.selectedMod;
    $('modUiTitle').textContent = t('web.mod.ui_title', {
      name: state.selectedMod.name || state.selectedMod.id
    });
    $('modUiStatus').textContent = t('web.mod.ui_loading');
    var frame = $('modUiFrame');
    frame.src =
      '/mod-ui/' +
      encodeURIComponent(state.selectedMod.id) +
      '/web/index.html';
    var dialog = $('modUiDialog');
    if (dialog && !dialog.open) dialog.showModal();
  }

  function closeModUi() {
    var frame = $('modUiFrame');
    if (frame) frame.src = 'about:blank';
    var dialog = $('modUiDialog');
    if (dialog && dialog.open) dialog.close();
    state.modUiMod = null;
  }

  function sendModUiInit() {
    if (!state.modUiMod) return;
    var frame = $('modUiFrame');
    if (!frame || !frame.contentWindow) return;
    frame.contentWindow.postMessage(
      {
        source: 'linkslot-host',
        type: 'init',
        info: {
          id: state.modUiMod.id,
          name: state.modUiMod.name,
          version: state.modUiMod.version,
          locale: i18n.lang
        }
      },
      '*'
    );
    $('modUiStatus').textContent = t('web.mod.ui_connected');
  }

  function handleModUiMessage(event) {
    var frame = $('modUiFrame');
    if (!frame || event.source !== frame.contentWindow) return;
    var message = event.data;
    if (!message || message.source !== 'linkslot-mod' || message.type !== 'request') {
      return;
    }
    var mod = state.modUiMod;
    if (!mod || !state.selectedMod || state.selectedMod.id !== mod.id) return;
    dispatchModUiRequest(mod, message.method, message.params || {})
      .then(function (data) {
        replyModUi(message.id, true, data, null);
      })
      .catch(function (error) {
        replyModUi(
          message.id,
          false,
          null,
          (error && error.message) || t('web.mod.api_failed')
        );
      });
  }

  function replyModUi(id, ok, data, error) {
    var frame = $('modUiFrame');
    if (!frame || !frame.contentWindow) return;
    frame.contentWindow.postMessage(
      {
        source: 'linkslot-host',
        type: 'response',
        id: id,
        ok: ok,
        data: data === undefined ? null : data,
        error: error
      },
      '*'
    );
  }

  function dispatchModUiRequest(mod, method, params) {
    var id = mod.id;
    if (method === 'files.list') {
      return api('/api/mods/files?id=' + encodeURIComponent(id));
    }
    if (method === 'files.read') {
      return api(
        '/api/mods/file?id=' +
          encodeURIComponent(id) +
          '&path=' +
          encodeURIComponent(params.path || '')
      );
    }
    if (method === 'files.write') {
      return apiWrite('/api/mods/file', {
        id: id,
        path: params.path || '',
        content: params.content || '',
        revision: params.revision || undefined
      });
    }
    if (method === 'actions.list') {
      return api('/api/mods/item?id=' + encodeURIComponent(id)).then(function (detail) {
        return { actions: (detail && detail.actions) || [] };
      });
    }
    if (method === 'actions.get') {
      return api(
        '/api/mods/action?id=' +
          encodeURIComponent(id) +
          '&action=' +
          encodeURIComponent(params.action || '')
      );
    }
    if (method === 'actions.save') {
      return apiWrite('/api/mods/action', {
        id: id,
        action_id: params.action_id || undefined,
        name: params.name || '',
        timing: params.timing || '',
        enabled: params.enabled !== false,
        order: params.order,
        boot_delay_minutes: params.boot_delay_minutes || 0,
        content: params.content || '',
        revision: params.revision || undefined
      });
    }
    if (method === 'actions.toggle') {
      return apiWrite('/api/mods/action/toggle', {
        id: id,
        action: params.action || '',
        enabled: !!params.enabled
      });
    }
    if (method === 'actions.delete') {
      return apiWrite('/api/mods/action/delete', {
        id: id,
        action: params.action || ''
      });
    }
    if (method === 'actions.check') {
      return apiWrite('/api/mods/check', {
        id: id,
        action: params.action || undefined
      });
    }
    if (id === 'gba-cover-fronts' && method === 'fronts.games') {
      return api('/api/roms').then(function (data) {
        return {
          games: ((data && data.games) || [])
            .filter(function (game) { return game.platform === 'GBA'; })
            .map(function (game) {
              return {
                rom: game.rom,
                stem: game.stem,
                name: game.name || game.stem || game.rom
              };
            })
        };
      });
    }
    if (id === 'gba-cover-fronts' && method === 'fronts.list') {
      return api('/api/rom/fronts?rom=' + encodePath(params.rom || ''));
    }
    if (id === 'gba-cover-fronts' && method === 'fronts.index') {
      return api('/api/rom/fronts/index');
    }
    if (id === 'gba-cover-fronts' && method === 'fronts.save') {
      if (!isWritable()) return Promise.reject(new Error(t('web.files.readonly_upload')));
      var png = String(params.png || '');
      var pngMatch = /^data:image\/png;base64,([A-Za-z0-9+/=]+)$/.exec(png);
      if (!pngMatch) return Promise.reject(new Error('invalid PNG'));
      var binary = atob(pngMatch[1]);
      var bytes = new Uint8Array(binary.length);
      for (var i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
      var body = new Blob([bytes], { type: 'image/png' });
      var query = '?rom=' + encodePath(params.rom || '') +
        '&name=' + encodeURIComponent(String(params.name || '封面').slice(0, 80)) +
        '&model=any';
      return fetch('/api/rom/fronts' + query, {
        method: 'POST',
        headers: { 'Content-Type': 'image/png', 'X-Slot-Web': '1' },
        body: body
      }).then(function (response) {
        return response.json().then(function (data) {
          if (!response.ok) throw new Error((data && data.error) || ('HTTP ' + response.status));
          return data;
        });
      });
    }
    if (id === 'gba-cover-fronts' && method === 'fronts.select') {
      return apiWrite(
        '/api/rom/fronts/select?rom=' + encodePath(params.rom || '') +
          '&id=' + encodeURIComponent(params.id || ''),
        {},
        'PUT'
      );
    }
    if (id === 'gba-zip-import' && method === 'zip.list') {
      return api('/api/rom/zip/list');
    }
    if (id === 'gba-zip-import' && method === 'zip.extract') {
      return apiWrite('/api/rom/zip/extract', { path: params.path || '' });
    }
    if (method === 'context.saves') {
      return api('/api/saves');
    }
    if (method === 'library.migrateSidecars') {
      return apiWrite('/api/library/migrate', {});
    }
    if (method === 'context.storages') {
      return api('/api/mods').then(function (data) {
        return {
          storages: (data && data.storages) || [],
          timings: (data && data.timings) || []
        };
      });
    }
    if (method === 'tianma.start') {
      return apiWrite('/api/tianma/start', {
        path: params.path || 'Imports/TianmaG',
        overwrite: !!params.overwrite
      });
    }
    if (method === 'tianma.status') {
      return api('/api/tianma/status');
    }
    if (method === 'tianma.cancel') {
      return apiWrite('/api/tianma/cancel', {});
    }
    if (method.indexOf('gbastation.') === 0 && id !== 'gbastation-rom-sync') {
      return Promise.reject(new Error(t('web.mod.gbastation_only')));
    }
    if (method === 'gbastation.games') {
      return apiWrite('/api/gbastation/games', {
        base_url: params.base_url || ''
      });
    }
    if (method === 'gbastation.start') {
      return apiWrite('/api/gbastation/start', {
        base_url: params.base_url || '',
        keys: Array.isArray(params.keys) ? params.keys : [],
        overwrite: !!params.overwrite
      });
    }
    if (method === 'gbastation.status') {
      return api('/api/gbastation/status');
    }
    if (method === 'gbastation.cancel') {
      return apiWrite('/api/gbastation/cancel', {});
    }
    return Promise.reject(new Error(t('web.mod.api_unknown', { method: method })));
  }

  function showModsOutput(elementId, result) {
    var element = $(elementId);
    if (!element) return;
    var lines = [];
    if (result && typeof result.code === 'number') {
      lines.push(result.ok
        ? t('web.sync.exit_ok', { code: result.code })
        : t('web.sync.exit_fail', { code: result.code }));
    }
    if (result && result.timed_out) lines.push(t('web.mod.timed_out'));
    if (result && result.hint) lines.push(result.hint);
    if (result && result.detail) lines.push(t('web.mod.detail_line', { detail: result.detail }));
    if (result && result.output) lines.push('', result.output);
    element.textContent = lines.length ? lines.join('\n') : '—';
  }

  function renderModsControls() {
    var writable = isWritable();
    var busy = state.modsBusy;
    var importButton = $('modImport');
    if (importButton) importButton.disabled = !writable || busy;
    var exportButton = $('modExport');
    if (exportButton) exportButton.disabled = busy || !modsList().length;
    var toggles = document.querySelectorAll('[data-mod-toggle]');
    for (var i = 0; i < toggles.length; i += 1) {
      toggles[i].disabled =
        !writable ||
        busy ||
        toggles[i].getAttribute('data-mod-error') === '1' ||
        toggles[i].getAttribute('data-mod-no-ui') === '1';
    }
    var openButton = $('openModUi');
    if (openButton) {
      openButton.disabled =
        !state.selectedMod || !state.selectedMod.has_ui || busy;
    }
    var uninstall = $('uninstallMod');
    if (uninstall) uninstall.disabled = !writable || busy || !state.selectedMod;
  }

  function exportMods() {
    if (state.modsBusy) return;
    var list = modsList();
    if (!list.length) {
      toast(t('web.mods.nothing_to_export'), 'warn');
      return;
    }
    state.modsBusy = true;
    renderModsControls();
    var entries = [];
    var manifestMods = [];
    var usedFolders = {};
    var skipped = 0;

    function modUrl(id) {
      return '/api/mods/item?id=' + encodeURIComponent(id);
    }

    function actionUrl(modId, actionId) {
      return (
        '/api/mods/action?id=' +
        encodeURIComponent(modId) +
        '&action=' +
        encodeURIComponent(actionId)
      );
    }

    function fileUrl(modId, path) {
      return (
        '/api/mods/file?id=' +
        encodeURIComponent(modId) +
        '&path=' +
        encodeURIComponent(path)
      );
    }

    var chain = Promise.resolve();
    list.forEach(function (entry, modIndex) {
      if (entry.error || !entry.has_ui) {
        skipped += 1;
        return;
      }
      chain = chain.then(function () {
        return api(modUrl(entry.id)).then(function (mod) {
          var folder = zipModFolder(mod.id, modIndex, usedFolders);
          var root = 'mods/' + folder;
          entries.push({
            name: root + '/mod.json',
            data: JSON.stringify(
              {
                schema: 2,
                id: mod.id,
                name: mod.name,
                version: mod.version || '1.0.0',
                enabled: !!mod.enabled
              },
              null,
              2
            )
          });
          var actions = mod.actions || [];
          var manifestActions = [];
          var usedActions = {};
          var actionChain = Promise.resolve();
          actions.forEach(function (action, actionIndex) {
            actionChain = actionChain.then(function () {
              return api(actionUrl(mod.id, action.id)).then(function (detail) {
                var actionFolder = zipModFolder(
                  detail.id,
                  actionIndex,
                  usedActions
                );
                var actionRoot = root + '/actions/' + actionFolder;
                entries.push({
                  name: actionRoot + '/action.json',
                  data: JSON.stringify(
                    {
                      schema: 1,
                      name: detail.name,
                      timing: detail.timing,
                      enabled: !!detail.enabled,
                      order: detail.order
                    },
                    null,
                    2
                  )
                });
                entries.push({
                  name: actionRoot + '/main.lua',
                  data: detail.content || ''
                });
                manifestActions.push({
                  id: detail.id,
                  path: actionRoot,
                  name: detail.name,
                  timing: detail.timing,
                  enabled: !!detail.enabled,
                  order: detail.order
                });
              });
            });
          });

          var manifestFiles = [];
          var fileChain = api(
            '/api/mods/files?id=' + encodeURIComponent(mod.id)
          ).then(function (data) {
            var files = (data && data.files) || [];
            var textFiles = files.filter(function (file) {
              return (
                !file.is_dir &&
                file.editable &&
                file.path !== 'mod.json' &&
                file.path.indexOf('actions/') !== 0
              );
            });
            var hasIndex = textFiles.some(function (file) {
              return file.path === 'web/index.html';
            });
            if (!hasIndex) {
              throw new Error(
                t('web.mod.export_missing_index', { id: mod.id })
              );
            }
            var fileListChain = Promise.resolve();
            textFiles.forEach(function (file) {
              fileListChain = fileListChain.then(function () {
                return api(fileUrl(mod.id, file.path)).then(function (detail) {
                  entries.push({
                    name: root + '/' + file.path,
                    data: detail.content || ''
                  });
                  manifestFiles.push({ path: file.path });
                });
              });
            });
            return fileListChain;
          });

          return Promise.all([actionChain, fileChain]).then(function () {
            manifestMods.push({
              id: mod.id,
              path: root,
              name: mod.name,
              version: mod.version || '1.0.0',
              author: mod.author || t('web.mod.noAuthor'),
              enabled: !!mod.enabled,
              actions: manifestActions,
              files: manifestFiles
            });
          });
        }).catch(function (error) {
          skipped += 1;
          showError(error, t('web.mods.export_one_failed', { id: entry.id }));
        });
      });
    });

    chain
      .then(function () {
        if (!manifestMods.length) {
          toast(t('web.mods.nothing_complete'), 'warn', 9000);
          return;
        }
        var manifest = {
          schema: 2,
          kind: 'linkSlot.mods.zip',
          exported_at: new Date().toISOString(),
          mods: manifestMods
        };
        entries.unshift({
          name: 'manifest.json',
          data: JSON.stringify(manifest, null, 2)
        });
        downloadBlob(
          'linkSlot-mods-' + modsTimestamp() + '.zip',
          buildStoreZip(entries)
        );
        var extra = skipped
          ? t('web.mods.export_skipped', { count: skipped })
          : '';
        toast(
          t('web.mods.exported', { count: manifestMods.length }) + extra,
          'ok',
          6000
        );
      })
      .catch(function (error) {
        showError(error, t('web.mods.export_failed'));
      })
      .then(function () {
        state.modsBusy = false;
        renderModsControls();
      });
  }

  function manifestFromBareModZip(map) {
    var mods = [];
    Object.keys(map).forEach(function (entryName) {
      var match = /^mods\/([^/]+)\/mod\.json$/.exec(entryName);
      if (!match || !zipSafePath(entryName)) return;
      var path = 'mods/' + match[1];
      var meta;
      try {
        meta = JSON.parse(zipText(map[entryName]));
      } catch (error) {
        return;
      }
      var actions = [];
      Object.keys(map).forEach(function (actionName) {
        var actionMatch = new RegExp(
          '^' +
            path.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') +
            '/actions/([^/]+)/action\\.json$'
        ).exec(actionName);
        if (!actionMatch) return;
        var actionPath = path + '/actions/' + actionMatch[1];
        if (!map[actionPath + '/main.lua']) return;
        actions.push({
          path: actionPath,
          id: MOD_ACTION_ID_PATTERN.test(actionMatch[1]) ? actionMatch[1] : undefined
        });
      });
      mods.push({
        path: path,
        name: meta.name,
        version: meta.version,
        enabled: meta.enabled === true,
        actions: actions,
        files: []
      });
    });
    if (!mods.length) {
      throw new Error(t('web.mod.zip_no_manifest'));
    }
    return {
      schema: 2,
      kind: 'linkSlot.mods.zip',
      mods: mods
    };
  }

  // Import writes web/ files through /api/files/upload, whose parent must
  // already exist.  Create every intermediate mod directory first; an existing
  // directory (409) is fine.
  function ensureModDirs(id, files) {
    var dirs = {};
    (files || []).forEach(function (file) {
      var parts = String(file.path || '').split('/');
      parts.pop();
      for (var i = 1; i <= parts.length; i += 1) {
        dirs[parts.slice(0, i).join('/')] = true;
      }
    });
    var chain = Promise.resolve();
    Object.keys(dirs)
      .sort()
      .forEach(function (relative) {
        var parts = relative.split('/');
        var parent =
          'linkSlot/mods/' +
          id +
          (parts.length > 1 ? '/' + parts.slice(0, -1).join('/') : '');
        chain = chain.then(function () {
          return apiWrite('/api/files/mkdir', {
            path: parent,
            name: parts[parts.length - 1]
          }).catch(function () {});
        });
      });
    return chain;
  }

  function importMods(file) {
    if (!isWritable()) {
      toast(t('web.mod.readonly_import'), 'warn');
      return;
    }
    if (state.modsBusy || !file) return;
    if (file.size > MOD_IMPORT_MAX_BYTES) {
      toast(t('web.mod.import_too_big'), 'warn');
      return;
    }
    var name = String(file.name || '').toLowerCase();
    if (name.slice(-5) === '.json' || file.type === 'application/json') {
      toast(t('web.mod.legacy_json'), 'error', 9000);
      return;
    }
    var read = file.arrayBuffer
      ? file.arrayBuffer()
      : new Promise(function (resolve, reject) {
          var reader = new FileReader();
          reader.onload = function () {
            resolve(reader.result);
          };
          reader.onerror = function () {
            reject(new Error(t('web.mod.zip_read_failed')));
          };
          reader.readAsArrayBuffer(file);
        });

    read
      .then(readZipEntries)
      .then(function (entries) {
        var map = zipEntryMap(entries);
        var used = {};
        used['manifest.json'] = true;
        var manifestEntry = map['manifest.json'];
        var manifest;
        if (!manifestEntry) {
          manifest = manifestFromBareModZip(map);
        } else {
          try {
            manifest = JSON.parse(zipText(manifestEntry));
          } catch (error) {
            throw new Error(t('web.mod.manifest_invalid'));
          }
          if (manifest.schema !== 2 || manifest.kind !== 'linkSlot.mods.zip') {
            throw new Error(t('web.mod.manifest_schema'));
          }
          if (!Array.isArray(manifest.mods)) {
            throw new Error(t('web.mod.manifest_no_mods'));
          }
          if (manifest.mods.length > 64) {
            throw new Error(t('web.mod.import_too_many'));
          }
        }

        var imported = [];
        var invalid = 0;
        manifest.mods.forEach(function (raw) {
          var path =
            typeof raw.path === 'string' ? raw.path.replace(/\/+$/, '') : '';
          if (!zipSafePath(path) || path.indexOf('mods/') !== 0) {
            invalid += 1;
            return;
          }
          var modEntry = map[path + '/mod.json'];
          if (!modEntry) {
            invalid += 1;
            return;
          }
          var modMeta;
          try {
            modMeta = JSON.parse(zipText(modEntry));
          } catch (error) {
            invalid += 1;
            return;
          }
          var modName =
            typeof modMeta.name === 'string' ? modMeta.name.trim() : '';
          var modVersion =
            typeof modMeta.version === 'string' && modMeta.version.trim()
              ? modMeta.version.trim()
              : '1.0.0';
          var modAuthor =
            typeof modMeta.author === 'string' ? modMeta.author.trim() : '';
          if (
            !modName ||
            modName.length > 64 ||
            Number(modMeta.schema) !== 2 ||
            modVersionProblem(modVersion)
          ) {
            invalid += 1;
            return;
          }
          if (!map[path + '/web/index.html']) {
            invalid += 1;
            return;
          }
          var declaredId =
            typeof modMeta.id === 'string'
              ? modMeta.id.trim()
              : typeof raw.id === 'string'
                ? raw.id.trim()
                : '';
          var pathId = path.slice('mods/'.length);
          if (declaredId && !MOD_ACTION_ID_PATTERN.test(declaredId)) {
            invalid += 1;
            return;
          }
          var modId = declaredId || pathId;
          if (!MOD_ACTION_ID_PATTERN.test(modId)) {
            invalid += 1;
            return;
          }
          var existing = null;
          var knownMods = state.mods || [];
          for (var known = 0; known < knownMods.length; known += 1) {
            if (knownMods[known].id === modId) {
              existing = knownMods[known];
              break;
            }
          }

          var rawActions = Array.isArray(raw.actions) ? raw.actions : [];
          var actions = [];
          rawActions.forEach(function (actionRaw) {
            var actionPath =
              typeof actionRaw.path === 'string'
                ? actionRaw.path.replace(/\/+$/, '')
                : '';
            if (
              !zipSafePath(actionPath) ||
              actionPath.indexOf(path + '/actions/') !== 0
            ) {
              invalid += 1;
              return;
            }
            var actionEntry = map[actionPath + '/action.json'];
            var mainEntry = map[actionPath + '/main.lua'];
            if (!actionEntry || !mainEntry) {
              invalid += 1;
              return;
            }
            var actionId =
              typeof actionRaw.id === 'string' ? actionRaw.id.trim() : '';
            if (!MOD_ACTION_ID_PATTERN.test(actionId)) {
              actionId = actionPath.split('/').pop();
            }
            if (!MOD_ACTION_ID_PATTERN.test(actionId)) {
              invalid += 1;
              return;
            }
            var actionMeta;
            try {
              actionMeta = JSON.parse(zipText(actionEntry));
            } catch (error) {
              invalid += 1;
              return;
            }
            var actionName =
              typeof actionMeta.name === 'string'
                ? actionMeta.name.trim()
                : '';
            var timing = actionMeta.timing;
            var content = zipText(mainEntry);
            var order = Number(actionMeta.order);
            if (!isFinite(order) || Math.floor(order) !== order) order = 100;
            var bootDelay = Number(actionMeta.boot_delay_minutes);
            if (
              !isFinite(bootDelay) ||
              Math.floor(bootDelay) !== bootDelay ||
              bootDelay < 0 ||
              bootDelay > 1440
            ) {
              bootDelay = 0;
            }
            if (
              !actionName ||
              actionName.length > 64 ||
              MOD_TIMINGS.indexOf(timing) < 0 ||
              Number(actionMeta.schema) !== 1 ||
              utf8Bytes(content) > MOD_MAX_SCRIPT_BYTES ||
              order < -9999 ||
              order > 9999
            ) {
              invalid += 1;
              return;
            }
            used[actionPath + '/action.json'] = true;
            used[actionPath + '/main.lua'] = true;
            actions.push({
              id: actionId,
              name: actionName,
              timing: timing,
              enabled: actionMeta.enabled !== false,
              order: order,
              boot_delay_minutes: bootDelay,
              content: content,
              ui: actionMeta.ui || null,
              capabilities: Array.isArray(actionMeta.capabilities)
                ? actionMeta.capabilities
                : undefined
            });
          });

          var declaredFiles = Array.isArray(raw.files) ? raw.files : [];
          var filePaths = [];
          declaredFiles.forEach(function (fileRaw) {
            if (!fileRaw || typeof fileRaw.path !== 'string') return;
            if (fileRaw.path === 'config.json' && existing) return;
            filePaths.push(fileRaw.path);
          });
          Object.keys(map).forEach(function (entryName) {
            if (entryName.slice(-1) === '/') return;
            if (entryName.indexOf(path + '/') !== 0) return;
            var relative = entryName.slice(path.length + 1);
            if (relative === 'mod.json') {
              return;
            }
            if (isBinaryRelative(relative)) {
              filePaths.push(relative);
              return;
            }
            if (relative.indexOf('actions/') === 0) {
              return;
            }
            if (relative === 'config.json') {
              // Keep the user's settings when upgrading an existing id.  A new
              // mod still gets the package's initial config.
              if (!existing) filePaths.push(relative);
              return;
            }
            if (
              relative === 'web/index.html' ||
              relative.indexOf('web/') === 0 ||
              relative === 'README.md' ||
              relative === 'readme.md' ||
              relative === 'sync-chip.png' ||
              relative === 'theme.txt' ||
              isBinaryRelative(relative)
            ) {
              filePaths.push(relative);
            }
          });
          var files = [];
          var seen = {};
          filePaths.forEach(function (relative) {
            if (!zipSafePath(relative) || seen[relative]) return;
            seen[relative] = true;
            var entry = map[path + '/' + relative];
            if (!entry) return;
            used[path + '/' + relative] = true;
            if (isBinaryRelative(relative)) {
              files.push({ path: relative, image: entry.data });
            } else {
              files.push({ path: relative, content: zipText(entry) });
            }
          });
          used[path + '/mod.json'] = true;
          imported.push({
            id: modId,
            existing: existing,
            name: modName,
            version: modVersion,
            author: modAuthor,
            enabled: modMeta.enabled === true,
            actions: actions,
            files: files
          });
        });

        if (!imported.length) {
          throw new Error(t('web.mod.zip_no_mod'));
        }
        var updates = 0;
        for (var updateIndex = 0; updateIndex < imported.length; updateIndex += 1) {
          if (imported[updateIndex].existing) updates += 1;
        }
        var intro = updates
          ? t('web.mod.import_confirm_update', {
              count: imported.length,
              updates: updates
            })
          : t('web.mod.import_confirm_new', { count: imported.length });
        if (invalid) {
          intro += t('web.mod.import_confirm_invalid', { count: invalid });
        }
        if (!window.confirm(intro)) return;

        state.modsBusy = true;
        renderModsControls();
        var ok = 0;
        var failed = 0;
        var actionOk = 0;
        var fileOk = 0;
        var stickerOk = 0;
        var firstError = null;
        var chain = Promise.resolve();
        imported.forEach(function (mod) {
          chain = chain.then(function () {
            return apiWrite('/api/mods/item', {
              id: mod.id,
              allow_create: true,
              name: mod.name,
              version: mod.version,
              author: mod.author || '',
              enabled: mod.existing ? mod.existing.enabled : mod.enabled
            })
              .then(function (created) {
                var childChain = Promise.resolve();
                mod.actions.forEach(function (action) {
                  childChain = childChain
                    .then(function () {
                      return apiWrite('/api/mods/action', {
                        id: created.id,
                        action_id: action.id || undefined,
                        name: action.name,
                        timing: action.timing,
                        enabled: action.enabled,
                        order: action.order,
                        boot_delay_minutes: action.boot_delay_minutes || 0,
                        content: action.content,
                        capabilities: action.capabilities
                      }).then(function () {
                        actionOk += 1;
                      });
                    })
                    .catch(function (error) {
                      failed += 1;
                      if (!firstError) firstError = error;
                    });
                });
                childChain = childChain.then(function () {
                  return ensureModDirs(created.id, mod.files);
                });
                mod.files.forEach(function (file) {
                  childChain = childChain
                    .then(function () {
                      // The raw upload endpoint overwrites an existing page or
                      // sticker without requiring a revision.  The text-file
                      // revision path is for the editable Files page, and using
                      // it here broke every upgrade of an existing mod.
                      var body = file.image
                        ? file.image
                        : new TextEncoder().encode(file.content || '');
                      return fetch(
                        '/api/files/upload?path=' +
                          encodePath('linkSlot/mods/' + created.id + '/' + file.path),
                        {
                          method: 'POST',
                          headers: { 'X-Slot-Web': '1' },
                          body: body
                        }
                      ).then(function (response) {
                        if (!response.ok) {
                          return response.text().then(function (text) {
                            throw new Error(text || t('web.mod.file_save_failed'));
                          });
                        }
                        if (file.image) stickerOk += 1;
                        fileOk += 1;
                      });
                    })
                    .catch(function (error) {
                      failed += 1;
                      if (!firstError) firstError = error;
                    });
                });
                return childChain.then(function () {
                  ok += 1;
                });
              })
              .catch(function (error) {
                failed += 1;
                if (!firstError) firstError = error;
              });
          });
        });
        return chain
          .then(function () {
            var parts = [
              t('web.mod.import_ok', { count: ok }),
              t('web.mod.import_actions', { count: actionOk }),
              t('web.mod.import_files', { count: fileOk }),
              t('web.mod.import_stickers', { count: stickerOk })
            ];
            if (invalid) parts.push(t('web.mod.import_skipped', { count: invalid }));
            if (failed) parts.push(t('web.mod.import_failed', { count: failed }));
            toast(
              t('web.mod.import_done', {
                parts: parts.join(t('web.common.list_separator'))
              }),
              failed ? 'warn' : 'ok',
              9000
            );
            if (failed && firstError) {
              showError(firstError, t('web.mod.import_partial_failed'));
            }
            return loadMods();
          })
          .then(function () {
            state.modsBusy = false;
            renderModsControls();
          });
      })
      .catch(function (error) {
        state.modsBusy = false;
        renderModsControls();
        showError(error, t('web.mod.zip_import_failed'));
      });
  }


  var MOD_ZIP_MAX_ENTRIES = 512;
  var MOD_ZIP_MAX_ENTRY = 16 * 1024 * 1024;
  var MOD_ZIP_MAX_TOTAL = 64 * 1024 * 1024;
  var zipCrcTable = null;

  function zipCrc32(bytes) {
    if (!zipCrcTable) {
      zipCrcTable = new Uint32Array(256);
      for (var n = 0; n < 256; n += 1) {
        var c = n;
        for (var k = 0; k < 8; k += 1) {
          c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
        }
        zipCrcTable[n] = c >>> 0;
      }
    }
    var crc = 0xffffffff;
    for (var i = 0; i < bytes.length; i += 1) {
      crc = zipCrcTable[(crc ^ bytes[i]) & 0xff] ^ (crc >>> 8);
    }
    return (crc ^ 0xffffffff) >>> 0;
  }

  function zipDosTime(date) {
    var year = date.getFullYear();
    if (year < 1980) return { time: 0, date: 0x21 };
    if (year > 2107) year = 2107;
    return {
      time:
        (date.getHours() << 11) |
        (date.getMinutes() << 5) |
        Math.floor(date.getSeconds() / 2),
      date: ((year - 1980) << 9) | ((date.getMonth() + 1) << 5) | date.getDate()
    };
  }

  function zipUtf8(text) {
    return new TextEncoder().encode(String(text));
  }

  function buildStoreZip(entries) {
    var encoder = new TextEncoder();
    var parts = [];
    var central = [];
    var offset = 0;
    var stamp = zipDosTime(new Date());
    entries.forEach(function (entry) {
      var name = encoder.encode(entry.name);
      var data =
        entry.data instanceof Uint8Array ? entry.data : encoder.encode(String(entry.data || ''));
      var crc = zipCrc32(data);
      var local = new Uint8Array(30 + name.length + data.length);
      var view = new DataView(local.buffer);
      view.setUint32(0, 0x04034b50, true);
      view.setUint16(4, 20, true);
      view.setUint16(6, 0x0800, true);
      view.setUint16(8, 0, true);
      view.setUint16(10, stamp.time, true);
      view.setUint16(12, stamp.date, true);
      view.setUint32(14, crc, true);
      view.setUint32(18, data.length, true);
      view.setUint32(22, data.length, true);
      view.setUint16(26, name.length, true);
      view.setUint16(28, 0, true);
      local.set(name, 30);
      local.set(data, 30 + name.length);
      parts.push(local);
      central.push({ name: name, crc: crc, size: data.length, offset: offset });
      offset += local.length;
    });

    var centralOffset = offset;
    var centralParts = [];
    central.forEach(function (item) {
      var entry = new Uint8Array(46 + item.name.length);
      var view = new DataView(entry.buffer);
      view.setUint32(0, 0x02014b50, true);
      view.setUint16(4, 20, true);
      view.setUint16(6, 20, true);
      view.setUint16(8, 0x0800, true);
      view.setUint16(10, 0, true);
      view.setUint16(12, stamp.time, true);
      view.setUint16(14, stamp.date, true);
      view.setUint32(16, item.crc, true);
      view.setUint32(20, item.size, true);
      view.setUint32(24, item.size, true);
      view.setUint16(28, item.name.length, true);
      view.setUint16(30, 0, true);
      view.setUint16(32, 0, true);
      view.setUint16(34, 0, true);
      view.setUint16(36, 0, true);
      view.setUint32(38, 0, true);
      view.setUint32(42, item.offset, true);
      entry.set(item.name, 46);
      centralParts.push(entry);
    });

    var centralSize = centralParts.reduce(function (sum, part) {
      return sum + part.length;
    }, 0);
    var eocd = new Uint8Array(22);
    var end = new DataView(eocd.buffer);
    end.setUint32(0, 0x06054b50, true);
    end.setUint16(4, 0, true);
    end.setUint16(6, 0, true);
    end.setUint16(8, central.length, true);
    end.setUint16(10, central.length, true);
    end.setUint32(12, centralSize, true);
    end.setUint32(16, centralOffset, true);
    end.setUint16(20, 0, true);

    return new Blob(parts.concat(centralParts, [eocd]), { type: 'application/zip' });
  }

  function zipU16(view, pos) {
    if (pos + 2 > view.byteLength) throw new Error(t('web.mod.zip_damaged'));
    return view.getUint16(pos, true);
  }

  function zipU32(view, pos) {
    if (pos + 4 > view.byteLength) throw new Error(t('web.mod.zip_damaged'));
    return view.getUint32(pos, true);
  }

  function zipSafePath(name) {
    if (!name || name.charAt(0) === '/' || name.indexOf('\\') >= 0 || name.indexOf('\0') >= 0) {
      return false;
    }
    var parts = name.split('/');
    for (var i = 0; i < parts.length; i += 1) {
      if (parts[i] === '.' || parts[i] === '..') return false;
    }
    return true;
  }

  function zipInflateEntry(bytes) {
    return new Promise(function (resolve, reject) {
      if (typeof DecompressionStream !== 'function') {
        reject(
          new Error(
            t('web.mod.zip_deflate_unsupported')
          )
        );
        return;
      }
      var stream;
      try {
        stream = new Blob([bytes])
          .stream()
          .pipeThrough(new DecompressionStream('deflate-raw'));
      } catch (error) {
        reject(
          new Error(
            t('web.mod.zip_raw_unsupported')
          )
        );
        return;
      }
      new Response(stream)
        .arrayBuffer()
        .then(function (buffer) {
          resolve(new Uint8Array(buffer));
        })
        .catch(function () {
          reject(new Error(t('web.mod.zip_inflate_failed')));
        });
    });
  }

  function readZipEntries(buffer) {
    try {
      var bytes = new Uint8Array(buffer);
      if (bytes.length > MOD_IMPORT_MAX_BYTES) {
        throw new Error(t('web.mod.zip_too_big'));
      }
      if (bytes.length < 22) throw new Error(t('web.mod.zip_not_zip'));
      var view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
      var eocd = -1;
      var first = Math.max(0, bytes.length - 22 - 0xffff);
      for (var pos = bytes.length - 22; pos >= first; pos -= 1) {
        if (zipU32(view, pos) === 0x06054b50) {
          eocd = pos;
          break;
        }
      }
      if (eocd < 0) throw new Error(t('web.mod.zip_no_eocd'));
      var disk = zipU16(view, eocd + 4);
      var cdDisk = zipU16(view, eocd + 6);
      var entriesThisDisk = zipU16(view, eocd + 8);
      var total = zipU16(view, eocd + 10);
      var cdSize = zipU32(view, eocd + 12);
      var cdOffset = zipU32(view, eocd + 16);
      if (disk !== 0 || cdDisk !== 0 || entriesThisDisk !== total) {
        throw new Error(t('web.mod.zip_multi_volume'));
      }
      if (total > MOD_ZIP_MAX_ENTRIES) throw new Error(t('web.mod.zip_too_many_entries'));
      if (cdOffset + cdSize > eocd) throw new Error(t('web.mod.zip_central_out_of_bounds'));

      var metas = [];
      var centralPos = cdOffset;
      var totalSize = 0;
      for (var i = 0; i < total; i += 1) {
        if (zipU32(view, centralPos) !== 0x02014b50) {
          throw new Error(t('web.mod.zip_central_signature'));
        }
        var flags = zipU16(view, centralPos + 8);
        var method = zipU16(view, centralPos + 10);
        var crc = zipU32(view, centralPos + 16);
        var compressedSize = zipU32(view, centralPos + 20);
        var uncompressedSize = zipU32(view, centralPos + 24);
        var nameLength = zipU16(view, centralPos + 28);
        var extraLength = zipU16(view, centralPos + 30);
        var commentLength = zipU16(view, centralPos + 32);
        var localOffset = zipU32(view, centralPos + 42);
        if (flags & 1) throw new Error(t('web.mod.zip_encrypted'));
        if (method !== 0 && method !== 8) throw new Error(t('web.mod.zip_method_unsupported'));
        if (uncompressedSize > MOD_ZIP_MAX_ENTRY) throw new Error(t('web.mod.zip_entry_too_big'));
        totalSize += uncompressedSize;
        if (totalSize > MOD_ZIP_MAX_TOTAL) throw new Error(t('web.mod.zip_total_too_big'));
        var nameStart = centralPos + 46;
        var nameEnd = nameStart + nameLength;
        if (nameEnd > bytes.length) throw new Error(t('web.mod.zip_name_out_of_bounds'));
        var name = new TextDecoder('utf-8', { fatal: false }).decode(
          bytes.subarray(nameStart, nameEnd)
        );
        if (!zipSafePath(name)) throw new Error(t('web.mod.zip_unsafe_path'));

        if (localOffset + 30 > bytes.length) throw new Error(t('web.mod.zip_local_out_of_bounds'));
        if (zipU32(view, localOffset) !== 0x04034b50) {
          throw new Error(t('web.mod.zip_local_signature'));
        }
        var localNameLength = zipU16(view, localOffset + 26);
        var localExtraLength = zipU16(view, localOffset + 28);
        var dataStart = localOffset + 30 + localNameLength + localExtraLength;
        var dataEnd = dataStart + compressedSize;
        if (dataEnd > bytes.length) throw new Error(t('web.mod.zip_data_out_of_bounds'));
        metas.push({
          name: name,
          method: method,
          crc: crc,
          size: uncompressedSize,
          data: bytes.subarray(dataStart, dataEnd)
        });
        centralPos += 46 + nameLength + extraLength + commentLength;
      }

      var chain = Promise.resolve([]);
      metas.forEach(function (meta) {
        chain = chain.then(function (out) {
          var content =
            meta.method === 0 ? Promise.resolve(meta.data) : zipInflateEntry(meta.data);
          return content.then(function (data) {
            if (data.length !== meta.size) throw new Error(t('web.mod.zip_size_mismatch'));
            if (zipCrc32(data) !== meta.crc) throw new Error(t('web.mod.zip_crc_mismatch'));
            out.push({ name: meta.name, data: data });
            return out;
          });
        });
      });
      return chain;
    } catch (error) {
      return Promise.reject(error);
    }
  }

  function zipText(entry) {
    return new TextDecoder('utf-8', { fatal: false }).decode(entry.data);
  }

  function isBinaryRelative(relative) {
    return (
      relative === 'sticker.png' ||
      relative === 'sync-chip.png' ||
      relative === 'wallpaper.png' ||
      /^slot_back_[a-z0-9]+\.png$/.test(relative) ||
      /^font\.(otf|ttf|ttc)$/.test(relative) ||
      /^actions\/[^/]+\/sticker\.png$/.test(relative)
    );
  }

  function zipEntryMap(entries) {
    var map = {};
    entries.forEach(function (entry) {
      map[entry.name] = entry;
    });
    return map;
  }

  function zipModFolder(id, index, used) {
    var base = String(id || '')
      .replace(/[^A-Za-z0-9._-]/g, '_')
      .replace(/^\.+$/, '');
    if (!base) base = 'mod-' + (index + 1);
    var folder = base;
    var suffix = 1;
    while (used[folder]) {
      suffix += 1;
      folder = base + '-' + suffix;
    }
    used[folder] = true;
    return folder;
  }

  function bindMods() {
    if (!$('modList')) return;

    $('closeModDetail').addEventListener('click', closeModDetail);
    $('uninstallMod').addEventListener('click', uninstallMod);
    $('openModUi').addEventListener('click', openModUi);
    $('closeModUi').addEventListener('click', closeModUi);
    var uiDialog = $('modUiDialog');
    if (uiDialog) {
      uiDialog.addEventListener('click', function (event) {
        if (event.target === uiDialog) closeModUi();
      });
      uiDialog.addEventListener('close', function () {
        state.modUiMod = null;
      });
    }
    var uiFrame = $('modUiFrame');
    if (uiFrame) {
      uiFrame.addEventListener('load', sendModUiInit);
    }
    var detailDialog = $('modDetailDialog');
    if (detailDialog) {
      detailDialog.addEventListener('click', function (event) {
        if (event.target === detailDialog) closeModDetail();
      });
      detailDialog.addEventListener('close', function () {
        state.selectedMod = null;
        closeModUi();
      });
    }

    $('modImport').addEventListener('click', function () {
      if (state.modsBusy) return;
      $('modImportFile').click();
    });
    $('modImportFile').addEventListener('change', function () {
      var file = this.files && this.files[0];
      this.value = '';
      if (file) importMods(file);
    });
    $('modExport').addEventListener('click', exportMods);
    $('refreshMods').addEventListener('click', function () {
      loadMods();
    });
    $('refreshModsLog').addEventListener('click', function () {
      loadMods();
    });

    $('modList').addEventListener('click', function (event) {
      // Group headers fold their section instead of opening a mod.
      var group = event.target.closest('[data-mod-group]');
      if (group) {
        var key = group.getAttribute('data-mod-group');
        state.modGroupsClosed[key] = !state.modGroupsClosed[key];
        renderMods();
        return;
      }
      if (event.target.closest('input[data-mod-toggle]')) return;
      if (event.target.closest('label')) return;
      var row = event.target.closest('[data-mod-id]');
      if (!row) return;
      openModDetail(row.getAttribute('data-mod-id')).catch(function () {});
    });
    $('modList').addEventListener('change', function (event) {
      var checkbox = event.target.closest('[data-mod-toggle]');
      if (!checkbox) return;
      toggleMod(
        checkbox.getAttribute('data-mod-toggle'),
        checkbox.checked,
        checkbox
      );
    });

    window.addEventListener('message', handleModUiMessage);
    renderModsControls();
  }


  // ---------------------------------------------------------------- 网络页

  function loadNetworkStatus() {
    return api('/api/network')
      .then(function (network) {
        renderNetworkStatus(network);
        state.networkStatus = network;
        state.savedNetworks = (network && network.saved) || [];
        renderSavedNetworks();
        if (state.networks) renderNetworks();
      })
      .catch(function (error) {
        renderNetworkStatus({ connected: false, ssid: null, ip: null });
        $('netState').textContent = t('web.overview.read_failed');
        $('netState').className = 'danger';
        showError(error, t('web.network.read_failed'));
      });
  }

  function renderNetworkStatus(network) {
    network = network || {};
    if (network.connected) {
      $('netState').textContent = t('web.overview.connected');
      $('netState').className = 'ok';
      $('netSsid').textContent = network.ssid || '—';
      $('netIp').textContent = network.ip || t('web.overview.no_ipv4');
    } else {
      $('netState').textContent = t('web.overview.disconnected');
      $('netState').className = 'warn';
      $('netSsid').textContent = '—';
      $('netIp').textContent = '—';
    }
  }

  function signalBars(signal) {
    var strength = 1;
    if (signal >= -55) strength = 5;
    else if (signal >= -65) strength = 4;
    else if (signal >= -75) strength = 3;
    else if (signal >= -85) strength = 2;
    var color = strength <= 2 ? ' bad' : strength === 3 ? ' weak' : '';
    var html = '<span class="signal" title="' + esc(String(signal)) + ' dBm">';
    for (var index = 1; index <= 5; index += 1) {
      html +=
        '<i class="' + (index <= strength ? 'on' + color : '') + '"></i>';
    }
    return html + '</span>';
  }

  function renderSavedNetworks() {
    var saved = state.savedNetworks || [];
    var container = $('savedList');
    if (!container) return;
    if (!saved.length) {
      container.innerHTML =
        '<div class="empty">' + esc(t('web.network.savedEmpty')) + '</div>';
      return;
    }
    var currentSsid =
      state.networkStatus && state.networkStatus.connected
        ? state.networkStatus.ssid
        : '';
    var html = [];
    for (var index = 0; index < saved.length; index += 1) {
      var network = saved[index];
      var current =
        network.ssid && network.ssid === currentSsid
          ? ' · ' + esc(t('web.network.connected_tag'))
          : '';
      html.push(
        '<div class="net-item' +
          (current ? ' connected' : '') +
          '"><div class="grow"><b>' +
          esc(network.ssid || '') +
          '</b><div class="net-sub">' + esc(t('web.network.saved_label')) + ' · ' +
          esc(network.password_set ? t('web.network.secure') : t('web.network.open')) +
          current +
          '</div></div><button class="btn small" type="button" data-action="pick-saved-wifi" data-ssid="' +
          esc(network.ssid || '') +
          '">' + esc(t('web.network.connect')) + '</button></div>'
      );
    }
    container.innerHTML = html.join('');
  }

  function renderNetworks() {    var networks = state.networks || [];
    var container = $('netList');
    if (!networks.length) {
      container.innerHTML =
        '<div class="empty">' + esc(t('web.network.none')) + '</div>';
      return;
    }
    var currentSsid = state.status && state.status.network ? state.status.network.ssid : '';
    var html = [];
    for (var index = 0; index < networks.length; index += 1) {
      var network = networks[index];
      var connectedClass = network.ssid && network.ssid === currentSsid ? ' connected' : '';
      html.push(
        '<div class="net-item' +
          connectedClass +
          '">' +
          signalBars(network.signal) +
          '<div class="grow"><b>' +
          esc(network.ssid || t('web.network.hidden')) +
          '</b><div class="net-sub">' +
          esc(network.secured ? t('web.network.secured') : t('web.network.open_network')) +
          ' · ' +
          esc(String(network.signal)) +
          ' dBm' +
          (connectedClass ? ' · ' + esc(t('web.network.connected_tag')) : '') +
          '</div></div>' +
          '<button class="btn small" type="button" data-action="pick-wifi" data-ssid="' +
          esc(network.ssid || '') +
          '">' + esc(t('web.network.choose')) + '</button>' +
          '</div>'
      );
    }
    container.innerHTML = html.join('');
  }

  function bindScan() {
    $('scanBtn').addEventListener('click', function () {
      if (state.scanBusy) return;
      var button = $('scanBtn');
      state.scanBusy = true;
      button.disabled = true;
      button.textContent = t('web.network.scanning');
      $('scanHint').textContent = t('web.network.scanning_hint');
      api('/api/network/scan')
        .then(function (data) {
          state.networks = (data && data.networks) || [];
          renderNetworks();
          $('scanHint').textContent = t('web.network.found', {
            count: state.networks.length
          });
        })
        .catch(function (error) {
          $('scanHint').textContent = t('web.network.scan_failed');
          showError(error, t('web.network.scan_error'));
        })
        .then(function () {
          state.scanBusy = false;
          button.disabled = false;
          button.textContent = t('web.network.scan');
        });
    });
  }

  // ---------------------------------------------------------------- 事件

  function loadWebSettings() {
    return api('/api/settings')
      .then(function (data) {
        if (!data) return;
        useUploadLimit(Number(data.max_upload_mb));
        if ($('webUploadMb')) $('webUploadMb').value = String(data.max_upload_mb);
        if ($('webSettingsStatus')) {
          $('webSettingsStatus').textContent = t('web.settings.current_limit', {
            max: data.max_upload_mb,
            min: data.min_upload_mb,
            maxLimit: data.max_limit_mb
          });
        }
      })
      .catch(function (error) {
        if ($('webSettingsStatus')) {
          $('webSettingsStatus').textContent = t('web.settings.read_failed', {
            message: error.message
          });
        }
      });
  }

  function saveWebSettings(mb) {
    var status = $('webSettingsStatus');
    if (!isWritable()) {
      toast(t('web.settings.readonly'), 'warn');
      return Promise.resolve();
    }
    if (!Number.isInteger(mb) || mb < 1 || mb > 256) {
      toast(t('web.settings.invalid_limit'), 'warn');
      return Promise.resolve();
    }
    if (status) status.textContent = t('web.settings.saving');
    return apiWrite('/api/settings', { max_upload_mb: mb })
      .then(function (data) {
        useUploadLimit(Number(data.max_upload_mb));
        if ($('webUploadMb')) $('webUploadMb').value = String(data.max_upload_mb);
        if (status) {
          status.textContent = t('web.settings.saved', {
            max: data.max_upload_mb
          });
        }
        toast(t('web.settings.saved_toast', { max: data.max_upload_mb }), 'ok');
      })
      .catch(function (error) {
        if (status) {
          status.textContent = t('web.settings.save_failed', {
            message: error.message
          });
        }
        showError(error, t('web.settings.save_error'));
      });
  }

  function bindSettings() {
    var form = $('webSettingsForm');
    if (form) {
      form.addEventListener('submit', function (event) {
        event.preventDefault();
        saveWebSettings(parseInt(value('webUploadMb'), 10));
      });
    }
    var reset = $('resetWebSettings');
    if (reset) {
      reset.addEventListener('click', function () {
        if ($('webUploadMb')) $('webUploadMb').value = '32';
        saveWebSettings(32);
      });
    }
    var langSelect = $('webLanguage');
    if (langSelect) {
      langSelect.addEventListener('change', function () {
        setLanguage(this.value);
      });
    }
  }

  function bindTabs() {
    window.addEventListener('hashchange', function () {
      showTab(currentTab());
    });
    if (!window.location.hash) {
      window.location.replace('#overview');
    }
  }

  function bindUpload() {
    $('uploadInput').addEventListener('change', function () {
      var input = this;
      var file = input.files && input.files[0] ? input.files[0] : null;
      var target = state.upload;
      state.upload = null;
      input.value = '';
      if (!file || !target) return;
      if (!isWritable()) {
        toast(t('web.files.readonly_upload'), 'warn');
        return;
      }
      if (file.size > MAX_UPLOAD_BYTES) {
        toast(
          t('web.saves.upload_too_large', { limit: uploadLimitText() }),
          'error',
          5000
        );
        return;
      }
      if (!window.confirm(t('web.saves.overwrite_confirm', {
        file: file.name,
        target: target.label
      }))) {
        return;
      }
      if (!beginUpload(t('web.saves.uploading'), file.name)) return;
      uploadWithProgress(
        'PUT',
        '/api/file?path=' + encodePath(target.path),
        file,
        'application/octet-stream',
        setUploadProgress
      )
        .then(function () {
          toast(t('web.saves.overwritten', { file: file.name }), 'ok');
          loadSaves();
          if (currentTab() === 'sync') loadSync();
        })
        .catch(function (error) {
          showError(error, t('web.error.upload_failed'));
        })
        .then(endUpload);
    });
  }

  function bindDelegatedActions() {
    document.addEventListener('click', function (event) {
      var target = event.target;
      while (target && target !== document) {
        if (target.getAttribute && target.getAttribute('data-action')) break;
        target = target.parentNode;
      }
      if (!target || target === document || !target.getAttribute) return;
      var action = target.getAttribute('data-action');
      if (action === 'upload') {
        if (!isWritable()) {
          toast(t('web.files.readonly_upload'), 'warn');
          return;
        }
        state.upload = {
          path: target.getAttribute('data-path') || '',
          label: target.getAttribute('data-label') || target.getAttribute('data-path') || ''
        };
        $('uploadInput').click();
      } else if (action === 'sync') {
        runSync(
          {
            platform: target.getAttribute('data-platform') || '',
            emulator: target.getAttribute('data-emulator') || '',
            game: target.getAttribute('data-game') || ''
          },
          target
        );
      } else if (action === 'delete-save') {
        deleteSave(
          target.getAttribute('data-path') || '',
          target.getAttribute('data-label') || target.getAttribute('data-path') || ''
        );
      } else if (action === 'pick-wifi') {
        $('ssidInput').value = target.getAttribute('data-ssid') || '';
        $('pskInput').focus();
      } else if (action === 'pick-saved-wifi') {
        connectSavedWifi(target.getAttribute('data-ssid') || '', target);
      }
    });
  }

  function bindWebdavForm() {
    $('addStorage').addEventListener('click', function () {
      captureWebdavForm();
      var profile = {
        id: storageId(),
        name: t('web.sync.new_name'),
        kind: 'webdav',
        url: '',
        remote_root: '/linkSlot',
        username: '',
        password_set: false,
        ca_cert: '',
        insecure: false,
        password: '',
        clear_password: false
      };
      state.profiles.push(profile);
      state.selectedStorage = profile.id;
      if (!state.activeStorage) state.activeStorage = profile.id;
      renderStorageList();
      fillWebdavForm(profile);
      $('wdName').focus();
    });
    $('removeStorage').addEventListener('click', function () {
      if (state.profiles.length <= 1) {
        toast(t('web.sync.keep_one'), 'warn');
        return;
      }
      captureWebdavForm();
      var at = -1;
      for (var index = 0; index < state.profiles.length; index += 1) {
        if (state.profiles[index].id === state.selectedStorage) at = index;
      }
      if (at < 0) return;
      var removed = state.profiles[at].id;
      state.profiles.splice(at, 1);
      if (state.activeStorage === removed) state.activeStorage = state.profiles[0].id;
      state.selectedStorage = state.activeStorage;
      renderStorageList();
      fillWebdavForm(selectedProfile());
    });
    $('storageList').addEventListener('click', function (event) {
      var button = event.target.closest('[data-edit-storage]');
      if (!button) return;
      captureWebdavForm();
      state.selectedStorage = button.getAttribute('data-edit-storage');
      renderStorageList();
      fillWebdavForm(selectedProfile());
    });
    $('storageList').addEventListener('change', function (event) {
      if (!event.target.matches('input[name="storageActive"]')) return;
      switchActiveStorage(event.target.value);
    });
    $('testWebdav').addEventListener('click', function () {
      if (!isWritable()) {
        toast(t('web.sync.readonly_test'), 'warn');
        return;
      }
      var button = $('testWebdav');
      if (button) {
        button.disabled = true;
        button.textContent = t('web.sync.testing');
      }
      captureWebdavForm();
      var current = selectedProfile();
      if (!current || !current.url) {
        if (button) {
          button.disabled = !isWritable();
          button.textContent = t('web.sync.test');
        }
        toast(t('web.sync.test_required'), 'warn');
        return;
      }
      toast(t('web.sync.testing_toast'), 'ok', 1800);
      apiWrite('/api/sync/test', {
        id: current.id,
        url: current.url,
        remote_root: current.remote_root,
        username: current.username,
        password: current.password || '',
        clear_password: !!current.clear_password,
        ca_cert: current.ca_cert,
        insecure: !!current.insecure
      })
        .then(function (result) {
          showSyncOutput(result);
          if (result && result.ok) {
            toast(result.hint || t('web.sync.test_ok'), 'ok', 5000);
          } else {
            toast((result && result.hint) || t('web.sync.test_failed'), 'error', 8000);
          }
        })
        .catch(function (error) {
          showError(error, t('web.sync.test_error'));
        })
        .then(function () {
          if (button) {
            button.disabled = !isWritable();
            button.textContent = t('web.sync.test');
          }
        });
    });
    $('webdavForm').addEventListener('submit', function (event) {
      event.preventDefault();
      if (!isWritable()) {
        toast(t('web.sync.readonly_save'), 'warn');
        return;
      }
      var url = value('wdUrl').trim();
      if (!url) {
        toast(t('web.sync.url_required'), 'warn');
        return;
      }
      captureWebdavForm();
      var selected = selectedProfile();
      if (selected) selected.password = value('wdPassword');
      var payload = syncConfigPayload();
      var button = $('saveWebdav');
      if (button) {
        button.disabled = true;
        button.textContent = t('web.sync.saving');
      }
      apiWrite('/api/sync/config', payload)
        .then(function () {
          toast(t('web.sync.saved'), 'ok');
          state.formFilled = false;
          loadSync();
        })
        .catch(function (error) {
          showError(error, t('web.sync.save_failed'));
        })
        .then(function () {
          if (button) {
            button.disabled = !isWritable();
            button.textContent = t('web.common.save');
          }
        });
    });
  }

  function connectSavedWifi(ssid, button) {
    if (!ssid) return;
    if (!isWritable()) {
      toast(t('web.network.readonly_switch'), 'warn');
      return;
    }
    var current = state.networkStatus || {};
    if (current.connected && current.ssid !== ssid) {
      if (!window.confirm(t('web.network.switch_confirm'))) {
        return;
      }
    }
    if (button) {
      button.disabled = true;
      button.textContent = t('web.network.connecting');
    }
    toast(t('web.network.connecting_saved', { ssid: ssid }), 'ok', 2000);
    apiWrite('/api/network/connect', { ssid: ssid, saved: true })
      .then(function (result) {
        toast(t('web.network.connected_to', {
          ssid: (result && result.ssid) || ssid
        }), 'ok', 4000);
        setTimeout(function () {
          refreshStatus().catch(function () {});
          loadNetworkStatus().catch(function () {});
        }, 2500);
      })
      .catch(function (error) {
        showError(error, t('web.network.connect_saved_failed'));
      })
      .then(function () {
        if (button) {
          button.disabled = !isWritable();
          button.textContent = t('web.network.connect');
        }
      });
  }

  function bindWifiForm() {
    $('wifiForm').addEventListener('submit', function (event) {
      event.preventDefault();
      if (!isWritable()) {
        toast(t('web.network.readonly_switch'), 'warn');
        return;
      }
      var ssid = value('ssidInput').trim();
      if (!ssid) {
        toast(t('web.network.ssid_required'), 'warn');
        return;
      }
      var current = state.status && state.status.network ? state.status.network : null;
      if (current && current.connected && current.ssid === ssid) {
        if (!window.confirm(t('web.network.reconnect_confirm', { ssid: ssid }))) return;
      } else if (current && current.connected) {
        if (!window.confirm(t('web.network.switch_confirm'))) {
          return;
        }
      }
      var button = $('connectBtn');
      if (button) {
        button.disabled = true;
        button.textContent = t('web.network.connecting');
      }
      apiWrite('/api/network/connect', { ssid: ssid, psk: value('pskInput') })
        .then(function (result) {
          $('pskInput').value = '';
          toast(t('web.network.connected_to', {
          ssid: (result && result.ssid) || ssid
        }), 'ok', 4000);
          setTimeout(function () {
            refreshStatus().catch(function () {});
            loadNetworkStatus().catch(function () {});
          }, 2500);
        })
        .catch(function (error) {
          showError(error, t('web.network.connect_failed'));
        })
        .then(function () {
          if (button) {
            button.disabled = !isWritable();
            button.textContent = t('web.network.connect');
          }
        });
    });
  }

  function bindLogLevel() {
    var button = $('saveLogLevel');
    if (!button) return;
    button.addEventListener('click', function () {
      if (!isWritable()) {
        toast(t('web.sync.readonly_level'), 'warn');
        return;
      }
      var level = $('logLevel').value;
      button.disabled = true;
      apiWrite('/api/log/level', { level: level })
        .then(function (result) {
          state.logLevel = (result && result.level) || level;
          toast(t('web.sync.level_saved', { level: state.logLevel }), 'ok');
          loadSync();
        })
        .catch(function (error) {
          showError(error, t('web.sync.level_failed'));
        })
        .then(function () {
          button.disabled = !isWritable();
        });
    });
  }

  function bindRefreshButtons() {
    $('refreshSaves').addEventListener('click', function () {
      $('savesSummary').textContent = t('web.common.reading');
      loadSaves();
    });
    $('ovRefreshLog').addEventListener('click', function () {
      loadRecentLog();
    });
    $('refreshSync').addEventListener('click', function () {
      state.formFilled = false;
      loadSync();
    });
  }

  function startPolling() {
    setInterval(function () {
      if (document.hidden) return;
      refreshStatus().catch(function () {});
      if (currentTab() === 'network') loadNetworkStatus().catch(function () {});
    }, POLL_MS);
    document.addEventListener('visibilitychange', function () {
      if (!document.hidden) refreshStatus().catch(function () {});
    });
  }

  // ---------------------------------------------------------------- mod 文档

  function copyText(text) {
    if (navigator.clipboard && window.isSecureContext) {
      return navigator.clipboard.writeText(text);
    }
    return new Promise(function (resolve, reject) {
      var area = document.createElement('textarea');
      area.value = text;
      area.setAttribute('readonly', 'readonly');
      area.style.position = 'fixed';
      area.style.left = '-9999px';
      document.body.appendChild(area);
      area.select();
      try {
        if (document.execCommand('copy')) resolve();
        else reject(new Error('copy failed'));
      } catch (error) {
        reject(error);
      } finally {
        document.body.removeChild(area);
      }
    });
  }

  function bindModDocs() {
    var buttons = document.querySelectorAll('[data-copy]');
    for (var i = 0; i < buttons.length; i += 1) {
      buttons[i].addEventListener('click', function () {
        var target = document.getElementById(this.getAttribute('data-copy'));
        if (!target) return;
        copyText(target.textContent)
          .then(function () {
            toast(t('web.mods.copied'), 'ok');
          })
          .catch(function () {
            toast(t('web.mods.copy_failed'), 'warn');
          });
      });
    }
    var links = document.querySelectorAll('.docs-toc a');
    for (var j = 0; j < links.length; j += 1) {
      links[j].addEventListener('click', function (event) {
        var target = document.getElementById(this.getAttribute('href').slice(1));
        if (!target) return;
        event.preventDefault();
        if (target.tagName === 'DETAILS') target.open = true;
        target.scrollIntoView({ behavior: 'smooth', block: 'start' });
      });
    }
  }

  // ---------------------------------------------------------------- 启动

  function bindUpgrade() {
    if ($('upgradeUpload')) $('upgradeUpload').addEventListener('click', uploadUpgrade);
  }

  // One zip with Logs/ (including crash/) and version.json, named after the
  // machine and the local time, so a report can be handed over in one file.
  function exportLogs() {
    var status = $('exportLogsStatus');
    if (status) status.textContent = t('web.about.export_packing');
    fetch('/api/logs/export', { cache: 'no-store' })
      .then(function (response) {
        if (!response.ok) {
          return response.json().then(
            function (body) {
              throw new Error((body && body.error) || 'HTTP ' + response.status);
            },
            function () {
              throw new Error('HTTP ' + response.status);
            }
          );
        }
        var disposition = response.headers.get('Content-Disposition') || '';
        var match = /filename="([^"]+)"/.exec(disposition);
        return response.blob().then(function (blob) {
          return { blob: blob, name: match ? match[1] : 'logs.zip' };
        });
      })
      .then(function (got) {
        var url = URL.createObjectURL(got.blob);
        var link = document.createElement('a');
        link.href = url;
        link.download = got.name;
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);
        setTimeout(function () {
          URL.revokeObjectURL(url);
        }, 10000);
        if (status) status.textContent = t('web.about.exported', { name: got.name });
      })
      .catch(function (error) {
        if (status) status.textContent = t('web.about.export_failed', {
          message: error.message
        });
      });
  }

  function bindLogExport() {
    if ($('exportLogs')) $('exportLogs').addEventListener('click', exportLogs);
  }

  function start() {
    if (!window.fetch) {
      var banner = $('writeBanner');
      if (banner) {
        banner.className = 'banner danger';
        banner.textContent = t('web.shell.old_browser');
      }
      return;
    }
    bindTabs();
    bindRoms();
    bindFrontEditor();
    bindUpload();
    bindFileBrowser();
    bindDelegatedActions();
    bindWebdavForm();
    bindMods();
    bindModDocs();
    bindWifiForm();
    bindLogLevel();
    bindRefreshButtons();
    bindScan();
    bindUpgrade();
    bindLogExport();
    bindSettings();
    loadI18n().then(function () {
      applyI18n();
      showTab(currentTab());
      refreshStatus().catch(function () {});
      loadWebSettings();
      loadUpgradeStatus();
      startPolling();
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', start);
  } else {
    start();
  }
})();
