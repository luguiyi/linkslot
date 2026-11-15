// Browser play: load the ROM and the battery save from the device, hand them to
// the WASM emulator, and write the save back through the device's own save path.
//
// The page never emulates anything itself: EmulatorJS (vendored under
// /play/data/) is the runtime, this file is the glue that keeps the .sav in
// sync with the card and owns the pause menu.
(function () {
  'use strict';

  var params = new URLSearchParams(window.location.search);
  var rom = params.get('rom') || '';
  var romUrl = '/api/play/rom?rom=' + encodeURIComponent(rom);
  var saveUrl = '/api/play/save?rom=' + encodeURIComponent(rom);
  var title = rom.split('/').pop().replace(/\.[^.]+$/, '') || '游戏';
  var titleFromRom = !!rom.split('/').pop().replace(/\.[^.]+$/, '');
  // Games/<机种>/<文件>: mGBA for Advance, Gambatte for Game Boy / Color,
  // FCEUmm (system name "nes") for the Famicom.
  var platform = (rom.split('/')[1] || 'GBA').toUpperCase();
  var core = platform === 'GBA' ? 'mgba' : platform === 'FC' ? 'nes' : 'gb';
  // Battery saves rarely change inside a session; a slow sweep is plenty.
  var SAVE_SYNC_MS = 5 * 60 * 1000;

  // ---------------------------------------------------------------- 文案
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
    var titles = document.querySelectorAll('[data-i18n-title]');
    for (var ti = 0; ti < titles.length; ti++) {
      titles[ti].setAttribute('title', t(titles[ti].getAttribute('data-i18n-title')));
    }
    document.title = t('play.documentTitle');
  }

  // EmulatorJS ships a player-1 keyboard map; a second Famicom pad needs its
  // own.  The two-player toggle only shows for FC, where a shared screen is
  // the real use case.
  var DEFAULT_P1 = {
  0: {
      0: {
          "value": "x",
          "value2": "BUTTON_2"
      },
      1: {
          "value": "s",
          "value2": "BUTTON_4"
      },
      2: {
          "value": "v",
          "value2": "SELECT"
      },
      3: {
          "value": "enter",
          "value2": "START"
      },
      4: {
          "value": "up arrow",
          "value2": "DPAD_UP"
      },
      5: {
          "value": "down arrow",
          "value2": "DPAD_DOWN"
      },
      6: {
          "value": "left arrow",
          "value2": "DPAD_LEFT"
      },
      7: {
          "value": "right arrow",
          "value2": "DPAD_RIGHT"
      },
      8: {
          "value": "z",
          "value2": "BUTTON_1"
      },
      9: {
          "value": "a",
          "value2": "BUTTON_3"
      },
      10: {
          "value": "q",
          "value2": "LEFT_TOP_SHOULDER"
      },
      11: {
          "value": "e",
          "value2": "RIGHT_TOP_SHOULDER"
      },
      12: {
          "value": "tab",
          "value2": "LEFT_BOTTOM_SHOULDER"
      },
      13: {
          "value": "r",
          "value2": "RIGHT_BOTTOM_SHOULDER"
      },
      14: {
          "value": "",
          "value2": "LEFT_STICK",
      },
      15: {
          "value": "",
          "value2": "RIGHT_STICK",
      },
      16: {
          "value": "h",
          "value2": "LEFT_STICK_X:+1"
      },
      17: {
          "value": "f",
          "value2": "LEFT_STICK_X:-1"
      },
      18: {
          "value": "g",
          "value2": "LEFT_STICK_Y:+1"
      },
      19: {
          "value": "t",
          "value2": "LEFT_STICK_Y:-1"
      },
      20: {
          "value": "l",
          "value2": "RIGHT_STICK_X:+1"
      },
      21: {
          "value": "j",
          "value2": "RIGHT_STICK_X:-1"
      },
      22: {
          "value": "k",
          "value2": "RIGHT_STICK_Y:+1"
      },
      23: {
          "value": "i",
          "value2": "RIGHT_STICK_Y:-1"
      },
      24: {
          "value": "1"
      },
      25: {
          "value": "2"
      },
      26: {
          "value": "3"
      },
      27: {},
      28: {},
      29: {},
  },
  1: {},
  2: {},
  3: {}
        };

  // Two-player Famicom: P1 gets WASD + JKL (U/I are Select/Start), P2 gets
  // the arrow keys + the number row. Non-two-player mode keeps EmulatorJS's
  // default single-player map.
  var DEFAULT_P2 = {
    0: { value: '1', value2: 'BUTTON_2' }, // B
    1: { value: '2', value2: 'BUTTON_1' }, // A
    2: { value: '5', value2: 'SELECT' },
    3: { value: '6', value2: 'START' },
    4: { value: 'up arrow', value2: 'DPAD_UP' },
    5: { value: 'down arrow', value2: 'DPAD_DOWN' },
    6: { value: 'left arrow', value2: 'DPAD_LEFT' },
    7: { value: 'right arrow', value2: 'DPAD_RIGHT' },
    8: { value: '3', value2: 'BUTTON_3' }, // turbo A
    9: { value: '4', value2: 'BUTTON_4' }, // turbo B
  };

  /// Two-player Famicom layout: both players get a separate, non-overlapping
  /// block. P1 uses WASD + JKL (plus U/I for Select/Start), P2 uses the arrow
  /// keys + the number row, which is much closer to two people at one keyboard
  /// than the shared single-player defaults.
  var TWO_PLAYER_P1 = {
    0: { value: 'j', value2: 'BUTTON_2' }, // B
    1: { value: 'k', value2: 'BUTTON_1' }, // A
    2: { value: 'u', value2: 'SELECT' },
    3: { value: 'i', value2: 'START' },
    4: { value: 'w', value2: 'DPAD_UP' },
    5: { value: 's', value2: 'DPAD_DOWN' },
    6: { value: 'a', value2: 'DPAD_LEFT' },
    7: { value: 'd', value2: 'DPAD_RIGHT' },
    8: { value: 'l', value2: 'BUTTON_3' }, // turbo A
    9: { value: '', value2: 'BUTTON_4' },
    10: { value: 'q', value2: 'LEFT_TOP_SHOULDER' },
    11: { value: 'e', value2: 'RIGHT_TOP_SHOULDER' },
  };

  function buildControllers(twoPlayer) {
    if (!twoPlayer) {
      return JSON.parse(JSON.stringify(DEFAULT_P1));
    }
    return {
      0: JSON.parse(JSON.stringify(TWO_PLAYER_P1)),
      1: JSON.parse(JSON.stringify(DEFAULT_P2)),
      2: {},
      3: {},
    };
  }

  function twoPlayerAvailable() {
    return platform === 'FC';
  }

  /// EmulatorJS hands the first connected pad to player 1 and the second to
  /// player 2, but only while a second player mapping exists (two-player
  /// mode).  Surface the count so nobody wonders why pad 2 does nothing.
  function connectedPads() {
    if (!navigator.getGamepads) return [];
    try {
      return Array.prototype.filter.call(navigator.getGamepads(), function (pad) {
        return !!pad && pad.connected;
      });
    } catch (error) {
      return [];
    }
  }

  function updatePadState() {
    var line = $('pad-state');
    if (!line) return;
    var pads = connectedPads().length;
    if (!pads) {
      line.hidden = true;
      return;
    }
    line.hidden = false;
    line.textContent =
      pads > 1 ? t('play.padTwo') : t('play.padOne');
  }

  var state = {
    emu: null,
    saveBytes: null,
    saveStamp: '',
    lastWritten: null,
    injected: false,
    started: false,
    timer: null,
    loading: true,
    twoPlayer: false,
  };

  var $ = function (id) { return document.getElementById(id); };

  var pillTimer = null;

  /// The battery save is a background detail: show a short note while it is
  /// being read or written, keep errors on screen, and disappear when idle.
  function setPill(text, kind, sticky) {
    var pill = $('save-pill');
    if (!pill) return;
    if (pillTimer) {
      clearTimeout(pillTimer);
      pillTimer = null;
    }
    if (!text) {
      pill.hidden = true;
      pill.textContent = '';
      return;
    }
    pill.hidden = false;
    pill.textContent = text;
    pill.className = 'pill' + (kind ? ' ' + kind : '');
    if (!sticky) {
      pillTimer = setTimeout(function () {
        pill.hidden = true;
        pillTimer = null;
      }, 4000);
    }
  }

  function setLoading(text) {
    var box = $('loading');
    var line = $('loading-text');
    if (line) line.textContent = text;
    if (box) box.hidden = false;
    state.loading = true;
  }

  function hideLoading() {
    var box = $('loading');
    if (box) box.hidden = true;
    state.loading = false;
  }

  function bytesEqual(a, b) {
    if (!a || !b) return a === b;
    if (a.length !== b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (a[i] !== b[i]) return false;
    }
    return true;
  }

  function savePathLabel() {
    var parts = rom.split('/');
    var platform = parts.length > 1 ? parts[1] : 'GBA';
    return 'Saves/' + platform + '/' + title + '.sav';
  }

  // ---------------------------------------------------------------- save io
  function readSave() {
    return fetch(saveUrl, { headers: { 'X-Slot-Web': '1' } }).then(function (response) {
      if (response.status === 204) return null;
      if (!response.ok) throw new Error('HTTP ' + response.status);
      state.saveStamp = response.headers.get('X-Save-Stamp') || '';
      return response.arrayBuffer();
    }).then(function (buffer) {
      state.saveBytes = buffer ? new Uint8Array(buffer) : null;
    });
  }

  function writeBack(force) {
    if (!state.emu) return Promise.resolve(true);
    var manager = state.emu.gameManager;
    if (!manager) return Promise.resolve(true);
    var bytes;
    try {
      bytes = manager.getSaveFile();
    } catch (error) {
      return Promise.resolve(true);
    }
    if (!bytes || !bytes.length) return Promise.resolve(true);
    if (!force && state.lastWritten && bytesEqual(bytes, state.lastWritten)) {
      return Promise.resolve(true);
    }
    setPill(t('play.saveSaving'), 'warn');
    return fetch(saveUrl, {
      method: 'PUT',
      headers: {
        'X-Slot-Web': '1',
        'Content-Type': 'application/octet-stream',
        'X-Save-Stamp': state.saveStamp,
      },
      body: bytes,
    }).then(function (response) {
      if (response.status === 409) {
        return response.json().then(function (data) {
          throw new Error(data.error || t('play.saveChanged'));
        });
      }
      if (!response.ok) throw new Error('HTTP ' + response.status);
      return response.json();
    }).then(function (data) {
      state.saveStamp = String((data && data.stamp) || state.saveStamp || '');
      state.lastWritten = bytes.slice();
      setPill(
        t('play.savedAt', {
          time: new Date().toLocaleTimeString(i18n.lang, {
            hour: '2-digit',
            minute: '2-digit'
          })
        }),
        'ok'
      );
      return true;
    }).catch(function (error) {
      // Losing progress is the one thing worth interrupting for.
      setPill(t('play.saveWriteFailed', { message: error.message }), 'bad', true);
      return false;
    });
  }

  // ---------------------------------------------------------------- runtime
  function injectSave() {
    if (state.injected || !state.saveBytes || !state.emu) return false;
    var manager = state.emu.gameManager;
    if (!manager || !manager.FS) return false;
    try {
      var path = manager.getSaveFilePath();
      manager.FS.writeFile(path, state.saveBytes);
      manager.loadSaveFiles();
      state.injected = true;
      state.lastWritten = state.saveBytes.slice();
      // The core has already booted the empty save; one restart picks ours up.
      setTimeout(function () {
        try { manager.restart(); } catch (error) { /* already booting */ }
      }, 120);
      return true;
    } catch (error) {
      console.warn('play: could not inject save', error);
      return false;
    }
  }

  function bootEmulator() {
    window.EJS_player = '#game';
    window.EJS_core = core;
    window.EJS_gameName = title;
    window.EJS_gameUrl = romUrl;
    window.EJS_pathtodata = '/play/data/';
    window.EJS_startOnLoaded = true;
    window.EJS_color = '#00d4aa';
    window.EJS_backgroundColor = '#0c0c14';
    // One localisation file is shipped with the offline bundle; asking for it
    // by name keeps the loader from guessing at the browser's language.
    window.EJS_language = i18n.lang;
    window.EJS_disableDatabases = true;
    window.EJS_defaultControls = buildControllers(state.twoPlayer);
    window.EJS_Buttons = {
      playPause: false,
      restart: false,
      settings: true,
      fullscreen: true,
      saveState: false,
      loadState: false,
      saveSavFiles: true,
      loadSavFiles: false,
      screenshot: false,
      cacheManager: false,
      exitEmulation: false,
    };
    window.EJS_onGameStart = function () {
      state.emu = window.EJS_emulator;
      state.started = true;
      hideLoading();
      if (state.saveBytes && !state.injected) {
        injectSave();
      }
      if (!state.timer) {
        state.timer = setInterval(function () { writeBack(false); }, SAVE_SYNC_MS);
      }
    };
    var script = document.createElement('script');
    script.src = '/play/data/loader.js';
    script.onerror = function () {
      setLoading(t('play.resourcesFailed'));
    };
    document.body.appendChild(script);
  }

  // ---------------------------------------------------------------- input safety
  /// Let go of every button on every port.  A browser can swallow the keyup
  /// (focus moved to the side panel, the tab was hidden, the window blurred),
  /// and the libretro core would keep the direction held forever.
  function releaseAllInputs() {
    if (!state.emu || !state.emu.gameManager) return;
    try {
      for (var player = 0; player < 4; player++) {
        for (var index = 0; index < 30; index++) {
          state.emu.gameManager.simulateInput(player, index, 0);
        }
      }
    } catch (error) { /* input layer not ready yet */ }
  }

  /// EmulatorJS listens on the player element; keys pressed while a button on
  /// this page has focus would otherwise never reach the game.
  function forwardKeyToGame(event) {
    if (!state.emu || typeof state.emu.keyChange !== 'function') return;
    var game = $('game');
    if (!game) return;
    if (event.target && game.contains(event.target)) return;
    try { state.emu.keyChange(event); } catch (error) { /* ignore */ }
  }

  function focusGame() {
    var game = $('game');
    if (game && game.focus) {
      try { game.focus({ preventScroll: true }); } catch (error) { /* ignore */ }
    }
  }

  /// Switch the pad mapping on a running emulator.  EmulatorJS converts the
  /// string map into key codes in setupKeys(), so re-run it after replacing
  /// the controls.
  function applyTwoPlayer(enabled) {
    state.twoPlayer = !!enabled;
    try {
      localStorage.setItem('slot-play-2p', state.twoPlayer ? '1' : '0');
    } catch (error) { /* private mode */ }
    releaseAllInputs();
    if (!state.emu) return;
    var controls = buildControllers(state.twoPlayer);
    state.emu.defaultControllers = JSON.parse(JSON.stringify(controls));
    state.emu.controls = JSON.parse(JSON.stringify(controls));
    try {
      if (state.emu.setupKeys) state.emu.setupKeys();
    } catch (error) { /* ignore */ }
    var hint = $('p2-keys');
    if (hint) hint.hidden = !state.twoPlayer;
    var footer = $('p2-footer');
    if (footer) footer.hidden = !state.twoPlayer;
    updatePadState();
  }

  // ---------------------------------------------------------------- pause menu
  function emulatorRunning() {
    return !!(state.emu && state.started);
  }

  function pauseGame() {
    if (!emulatorRunning()) return;
    writeBack(false);
    releaseAllInputs();
    try { if (state.emu.pause) state.emu.pause(); } catch (error) { /* ignore */ }
    var box = $('pause');
    if (box) box.hidden = false;
  }

  function resumeGame() {
    var box = $('pause');
    if (box) box.hidden = true;
    focusGame();
    if (!emulatorRunning()) return;
    try { if (state.emu.play) state.emu.play(); } catch (error) { /* ignore */ }
  }

  function restartGame() {
    if (!emulatorRunning()) return;
    try { if (state.emu.gameManager) state.emu.gameManager.restart(); } catch (error) { /* ignore */ }
    resumeGame();
  }

  function bindKeys() {
    window.addEventListener('keydown', forwardKeyToGame);
    window.addEventListener('keyup', forwardKeyToGame);
    window.addEventListener('blur', releaseAllInputs);
    document.addEventListener('visibilitychange', function () {
      if (document.visibilityState === 'hidden') releaseAllInputs();
    });
    // A cancelled touch (the browser starts scrolling, a call comes in) never
    // sends the matching pointerup, so the virtual pad would stay pressed.
    window.addEventListener('pointercancel', releaseAllInputs);
    window.addEventListener('touchcancel', releaseAllInputs);
    window.addEventListener('keydown', function (event) {
      var tag = (event.target && event.target.tagName) || '';
      if (tag === 'INPUT' || tag === 'TEXTAREA') return;
      var key = event.key;
      if (key === 'p' || key === 'P' || key === 'Escape') {
        var box = $('pause');
        if (box && box.hidden) { event.preventDefault(); pauseGame(); }
      } else if (key === 'f' || key === 'F') {
        if (state.emu && state.emu.toggleFastForward) {
          try { state.emu.toggleFastForward(!state.fastForward); } catch (error) { /* ignore */ }
          state.fastForward = !state.fastForward;
        }
      } else if (key === 'F11') {
        event.preventDefault();
        if (document.fullscreenElement) document.exitFullscreen();
        else document.documentElement.requestFullscreen().catch(function () {});
      }
    });
  }

  function bindButtons() {
    var pause = $('pause');
    if (pause) {
      pause.addEventListener('click', function (event) {
        var act = event.target && event.target.getAttribute('data-act');
        if (!act) return;
        if (act === 'resume') resumeGame();
        else if (act === 'restart') restartGame();
        else if (act === 'exit') {
          event.preventDefault();
          writeBack(true).then(function () { window.location.href = '/'; });
        }
      });
    }

    var twoPlayer = $('two-player');
    if (twoPlayer) {
      twoPlayer.addEventListener('change', function () {
        applyTwoPlayer(this.checked);
      });
    }

    var speed = $('speed-row');
    if (speed) {
      speed.addEventListener('click', function (event) {
        var value = event.target && event.target.getAttribute('data-speed');
        if (!value) return;
        Array.prototype.forEach.call(speed.querySelectorAll('.chip'), function (chip) {
          chip.classList.toggle('on', chip === event.target);
        });
        if (state.emu) {
          try {
            if (Number(value) > 1 && state.emu.setFastForwardRatio) state.emu.setFastForwardRatio(Number(value));
            if (Number(value) > 1 && state.emu.toggleFastForward) state.emu.toggleFastForward(true);
            if (Number(value) === 1 && state.emu.toggleFastForward) state.emu.toggleFastForward(false);
          } catch (error) { /* ignore */ }
        }
      });
    }

    var scale = $('scale-row');
    if (scale) {
      scale.addEventListener('click', function (event) {
        var value = event.target && event.target.getAttribute('data-scale');
        if (value === null) return;
        Array.prototype.forEach.call(scale.querySelectorAll('.chip'), function (chip) {
          chip.classList.toggle('on', chip === event.target);
        });
        var canvas = document.querySelector('#game canvas');
        if (!canvas) return;
        if (Number(value) === 0) {
          canvas.style.width = '';
          canvas.style.height = '';
        } else {
          canvas.style.width = 240 * Number(value) + 'px';
          canvas.style.height = 160 * Number(value) + 'px';
        }
      });
    }
  }

  function boot() {
    if (!rom) {
      setLoading(t('play.romMissing'));
      return;
    }
    $('title').textContent = title;
    $('platform').textContent = platform;
    if (twoPlayerAvailable()) {
      var row = $('two-player-row');
      if (row) row.hidden = false;
      var saved = null;
      try { saved = localStorage.getItem('slot-play-2p'); } catch (error) { /* ignore */ }
      state.twoPlayer = saved === '1';
      var box = $('two-player');
      if (box) box.checked = state.twoPlayer;
      var hint = $('p2-keys');
      if (hint) hint.hidden = !state.twoPlayer;
      var footer = $('p2-footer');
      if (footer) footer.hidden = !state.twoPlayer;
    }
    $('host').textContent = window.location.host;
    setLoading(t('play.readingSave'));
    readSave().then(function () {
      if (state.saveBytes) setPill(t('play.saveLoaded'), 'ok');
      setLoading(t('play.starting'));
      bootEmulator();
    }).catch(function (error) {
      console.warn('play: save read failed', error);
      setPill(t('play.saveReadFailed', { message: error.message }), 'bad', true);
      bootEmulator();
    });
  }

  window.addEventListener('gamepadconnected', updatePadState);
  window.addEventListener('gamepaddisconnected', function () {
    updatePadState();
    releaseAllInputs();
  });
  window.addEventListener('pagehide', function () { releaseAllInputs(); writeBack(true); });
  document.addEventListener('visibilitychange', function () {
    if (document.visibilityState === 'hidden') writeBack(true);
  });

  bindKeys();
  bindButtons();
  loadI18n().then(function () {
    if (!titleFromRom) title = t('play.titleFallback');
    applyI18n();
    boot();
  });
})();
