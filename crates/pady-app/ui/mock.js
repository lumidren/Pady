/*
 * Pady — browser preview backend.
 *
 * Only active when `window.__TAURI__` is absent (the page was opened in a
 * normal browser). It exposes `window.PadyMock = { invoke, listen }` with the
 * same contract as the Rust side, keeps state in memory, and runs a scripted
 * "virtual hand" on a loop that emits `pady://frame` (while live),
 * `pady://gesture` and `pady://level` events so every screen can be previewed.
 *
 * Binding resolution mirrors pady-core: a pretend foreground app changes every
 * few seconds, and per-app profiles override global bindings for it (a
 * profile binding of type `none` switches the gesture off in that app).
 *
 * Preview switches (query string):
 *   ?platform=linux   Linux labels / presets
 *   ?nodevice         no touchpad + input_error
 *   ?noconflicts      empty conflicts list
 *   ?noprofiles       start without the sample "Browser" profile
 *   ?theme=light|dark force a theme regardless of the OS setting
 *   ?brightfail       brightness actions emit pady://action-failed
 *   ?detectnull       detect_app times out (resolves null)
 *   ?nopermission     Linux without access to the touchpad (shows Fix permissions)
 *   ?onboarding       start with settings.onboarded = false (shows the tutorial); the
 *                     virtual hand then loops the tutorial's practice gestures
 *   ?slow             add 600ms latency to every command
 *   ?update           pretend a newer version is available
 *   ?still            the virtual hand rests (for static screenshots)
 */
(function () {
  'use strict';
  if (window.__TAURI__) return;

  const q = new URLSearchParams(location.search);
  const platform = q.get('platform') === 'linux' ? 'linux' : 'windows';
  const linux = platform === 'linux';
  if (q.get('theme')) document.documentElement.dataset.theme = q.get('theme');

  const clone = (v) => (v === undefined ? null : JSON.parse(JSON.stringify(v)));

  const DEFAULT_SETTINGS = {
    enabled: true, tap_max_ms: 220, tap_slop_mm: 4.0, multi_tap_ms: 300, swipe_threshold_mm: 10.0, swipe_step_mm: 7.0,
    pinch_threshold_mm: 8.0, hold_ms: 600, drag_speed: 9.0, drag_acceleration: 1.0, drag_release_ms: 350,
    show_overlay: linux, pause_in_fullscreen: true, show_copy_notice: true, onboarded: false, check_updates: true, autostart_configured: true,
    animations: true, anim_swipe_glow: true, anim_hints: false, anim_pinch: true, anim_notices: true, anim_touches: false,
    anim_previews: true, anim_mirror: true, anim_celebrate: true,
  };
  const DEFAULT_BINDINGS = [
    { trigger: 'drag:3', action: { type: 'drag' } },
    { trigger: 'tap:3', action: { type: 'media', key: 'play_pause' } },
    { trigger: 'doubletap:3', action: { type: 'media', key: 'next' } },
    { trigger: 'swipe:4:up', action: { type: 'volume', step: 4 }, repeat: true },
    { trigger: 'swipe:4:down', action: { type: 'volume', step: -4 }, repeat: true },
    { trigger: 'swipe:4:right', action: { type: 'brightness', step: 10 }, repeat: true },
    { trigger: 'swipe:4:left', action: { type: 'brightness', step: -10 }, repeat: true },
    { trigger: 'tap:4', action: { type: 'keys', combo: 'win+d' } },
    { trigger: 'hold:4', action: { type: 'lock' } },
    { trigger: 'pinch:4:out', action: { type: 'keys', combo: 'ctrl+equal' }, repeat: true },
    { trigger: 'pinch:4:in', action: { type: 'keys', combo: 'ctrl+minus' }, repeat: true },
    { trigger: 'pinch:3:in', action: { type: 'keys', combo: 'ctrl+c' } },
    { trigger: 'pinch:3:out', action: { type: 'keys', combo: 'ctrl+v' } },
  ];
  const SAMPLE_PROFILE = {
    name: 'Browser',
    apps: linux ? ['google-chrome', 'firefox'] : ['chrome.exe', 'msedge.exe'],
    binding: [
      { trigger: 'tap:3', action: { type: 'keys', combo: 'ctrl+t' } },
      { trigger: 'swipe:4:left', action: { type: 'keys', combo: 'ctrl+shift+tab' } },
      { trigger: 'swipe:4:right', action: { type: 'keys', combo: 'ctrl+tab' } },
      { trigger: 'hold:4', action: { type: 'none' } },
    ],
  };
  // Starter sets for apply_preset (the real backend owns these; plausible stand-ins here).
  const keys = (combo) => ({ type: 'keys', combo });
  // Mirrors crates/pady-core/src/presets.rs (names, descriptions and bindings).
  const SHORTCUT = linux
    ? { taskView: 'win', showDesktop: 'win+d', desktopLeft: 'win+pageup', desktopRight: 'win+pagedown', screenshot: 'printscreen' }
    : { taskView: 'win+tab', showDesktop: 'win+d', desktopLeft: 'win+ctrl+left', desktopRight: 'win+ctrl+right', screenshot: 'win+shift+s' };
  const DRAG = { trigger: 'drag:3', action: { type: 'drag' } };
  const ZOOM = [
    { trigger: 'pinch:4:out', action: keys('ctrl+equal'), repeat: true },
    { trigger: 'pinch:4:in', action: keys('ctrl+minus'), repeat: true },
  ];
  const PRESETS = [
    { id: 'recommended', name: 'Recommended', description: 'Three-finger drag, taps for play/pause and next track, four-finger swipes for volume and brightness, pinch to zoom.', binding: DEFAULT_BINDINGS },
    {
      id: 'macos', name: 'Like a Mac', description: 'Feels like a MacBook trackpad: three-finger drag, four-finger swipes between desktops and to task view, pinch to zoom.',
      binding: [
        DRAG,
        { trigger: 'swipe:4:up', action: keys(SHORTCUT.taskView) },
        { trigger: 'swipe:4:down', action: keys(SHORTCUT.showDesktop) },
        { trigger: 'swipe:4:left', action: keys(SHORTCUT.desktopRight) },
        { trigger: 'swipe:4:right', action: keys(SHORTCUT.desktopLeft) },
        { trigger: 'tap:3', action: { type: 'media', key: 'play_pause' } },
        ...ZOOM,
      ],
    },
    {
      id: 'media', name: 'Media', description: 'Built for music and video: taps control playback, four-finger swipes scrub the volume and skip tracks.',
      binding: [
        DRAG,
        { trigger: 'tap:3', action: { type: 'media', key: 'play_pause' } },
        { trigger: 'doubletap:3', action: { type: 'media', key: 'next' } },
        { trigger: 'tripletap:3', action: { type: 'media', key: 'previous' } },
        { trigger: 'swipe:4:up', action: { type: 'volume', step: 4 }, repeat: true },
        { trigger: 'swipe:4:down', action: { type: 'volume', step: -4 }, repeat: true },
        { trigger: 'swipe:4:left', action: { type: 'media', key: 'previous' } },
        { trigger: 'swipe:4:right', action: { type: 'media', key: 'next' } },
        { trigger: 'tap:4', action: { type: 'mute' } },
      ],
    },
    {
      id: 'productivity', name: 'Productivity', description: 'Window and desktop management: task view, show desktop, switching desktops, quick screenshots and lock.',
      binding: [
        DRAG,
        { trigger: 'swipe:4:left', action: keys(SHORTCUT.desktopRight) },
        { trigger: 'swipe:4:right', action: keys(SHORTCUT.desktopLeft) },
        { trigger: 'swipe:4:up', action: keys(SHORTCUT.taskView) },
        { trigger: 'swipe:4:down', action: keys(SHORTCUT.showDesktop) },
        { trigger: 'tap:4', action: keys(SHORTCUT.screenshot) },
        { trigger: 'hold:4', action: { type: 'lock' } },
        { trigger: 'tap:3', action: { type: 'media', key: 'play_pause' } },
        ...ZOOM,
      ],
    },
    { id: 'minimal', name: 'Minimal', description: 'Just three-finger drag. Start small and add your own.', binding: [DRAG] },
  ];

  // The pretend app in front cycles through these (one of them is in the sample profile).
  const FOREGROUND = linux ? ['code', 'google-chrome', 'nautilus'] : ['Code.exe', 'chrome.exe', 'explorer.exe'];

  const noDevice = q.has('nodevice');
  const noPermission = q.has('nopermission');
  const state = {
    config: {
      // The preview skips the tutorial unless ?onboarding asks for it.
      settings: Object.assign(clone(DEFAULT_SETTINGS), { onboarded: !q.has('onboarding') }),
      // The preview starts with a few extras so the "shadowed" badge, an
      // open_url chip and a per-app profile are visible out of the box.
      binding: clone(DEFAULT_BINDINGS).concat([
        { trigger: 'swipe:3:up', action: { type: 'keys', combo: 'win+tab' } },
        { trigger: 'tap:5', action: { type: 'open_url', url: 'https://youtube.com', browser: 'chrome' } },
      ]),
      profile: q.has('noprofiles') ? [] : [clone(SAMPLE_PROFILE)],
    },
    devices: noDevice || noPermission ? [] : [{ name: 'Precision Touchpad', width_mm: 105.0, height_mm: 68.0, max_contacts: 5 }],
    input_error: noPermission
      ? 'permission denied reading touchpad: click Fix permissions in Pady or run pady-cli fix-permissions'
      : noDevice
      ? (linux
        ? 'No touchpad found under /dev/input. Add your user to the "input" group and log in again.'
        : 'No Precision Touchpad was found. Pady needs a touchpad that uses the Windows Precision driver.')
      : null,
    conflicts: (noPermission ? [{
      message: "Pady isn't allowed to read your touchpad yet.",
      fix: 'Click Fix permissions and enter your password once.',
      can_open_settings: false,
      can_fix: true,
    }] : []).concat(q.has('noconflicts') ? [] : [{
      message: linux ? 'GNOME also handles three-finger swipes.' : 'Windows also handles three-finger swipes.',
      fix: linux
        ? 'Disable workspace gestures with a GNOME extension, or bind only 4- and 5-finger swipes.'
        : 'Set three-finger swipes to "Nothing" in Touchpad settings.',
      can_open_settings: !linux,
      can_fix: false,
    }]),
    shadowed: [],
    autostart: false,
    config_path: linux ? '/home/me/.config/pady/config.toml' : 'C:\\Users\\me\\AppData\\Roaming\\Pady\\config.toml',
    platform,
    version: '0.2.0',
    presets: PRESETS.map(({ id, name, description }) => ({ id, name, description })),
  };

  /* ---------- Validation / normalisation (mirrors pady-core) ---------- */

  const KEY_ALIASES = {
    return: 'enter', escape: 'esc', del: 'delete', ins: 'insert', pgup: 'pageup', pgdn: 'pagedown',
    prtsc: 'printscreen', print: 'printscreen', '-': 'minus', plus: 'equal', '=': 'equal', ',': 'comma',
    '.': 'period', '/': 'slash', '\\': 'backslash', ';': 'semicolon', "'": 'quote', '[': 'bracketleft',
    ']': 'bracketright', backtick: 'grave', '`': 'grave',
  };
  const KEYS = new Set('abcdefghijklmnopqrstuvwxyz0123456789'.split('').concat(
    Array.from({ length: 12 }, (_, i) => 'f' + (i + 1)),
    ['enter', 'esc', 'tab', 'space', 'backspace', 'delete', 'insert', 'home', 'end', 'pageup', 'pagedown',
      'up', 'down', 'left', 'right', 'printscreen', 'minus', 'equal', 'comma', 'period', 'slash', 'backslash',
      'semicolon', 'quote', 'bracketleft', 'bracketright', 'grave']));
  const MODS = { ctrl: 'ctrl', control: 'ctrl', alt: 'alt', option: 'alt', shift: 'shift', win: 'win', super: 'win', meta: 'win', cmd: 'win', windows: 'win' };

  function checkCombo(combo) {
    const parts = String(combo || '').split('+').map((p) => p.trim()).filter(Boolean);
    if (!parts.length) throw 'empty shortcut';
    let key = null;
    for (const p of parts) {
      const l = p.toLowerCase();
      if (MODS[l]) continue;
      const k = KEY_ALIASES[l] || l;
      if (!KEYS.has(k)) throw `unknown key \`${p}\``;
      if (key) throw `shortcut has two main keys (\`${key}\` and \`${k}\`)`;
      key = k;
    }
  }

  /** Canonical trigger string; forgiving about case and spaces like the Rust parser. */
  function checkTrigger(t) {
    const canon = String(t).split(':').map((p) => p.trim().toLowerCase()).join(':');
    const ok = /^(tap|doubletap|tripletap|hold|drag):[345]$|^swipe:[345]:(up|down|left|right)$|^pinch:[345]:(in|out)$/.test(canon);
    if (!ok) throw `invalid trigger \`${t}\` (expected e.g. tap:3, doubletap:3, hold:4, swipe:3:up, pinch:4:in, drag:3)`;
    return canon;
  }
  const fingersOf = (trigger) => Number(trigger.split(':')[1]);

  function checkAction(a) {
    if (!a || typeof a !== 'object') throw 'invalid type: expected internally tagged enum Action';
    switch (a.type) {
      case 'none': case 'mute': case 'lock': case 'drag': return;
      case 'keys': return checkCombo(a.combo);
      case 'volume': case 'brightness':
        if (!Number.isInteger(a.step)) throw `${a.type} step must be a whole number`;
        return;
      case 'media':
        if (!['play_pause', 'next', 'previous', 'stop', 'volume_up', 'volume_down', 'mute'].includes(a.key)) throw `unknown media key \`${a.key}\``;
        return;
      case 'open_url': if (!a.url) throw 'open_url needs a url'; return;
      case 'launch': case 'focus_or_launch': if (!a.program) throw `${a.type} needs a program`; return;
      case 'run': if (!a.command) throw 'run needs a command'; return;
      default: throw `unknown variant \`${a.type}\``;
    }
  }

  /** Keep bindings passing `keep`, last duplicate of a trigger wins (like CSS). */
  function dedupe(list, keep) {
    const seen = new Set();
    const kept = [];
    for (const b of (list || []).slice().reverse()) {
      if (!b || typeof b !== 'object') throw 'invalid type: expected struct Binding';
      const trigger = checkTrigger(b.trigger);
      checkAction(b.action);
      if (!keep(trigger, b.action) || seen.has(trigger)) continue;
      seen.add(trigger);
      const nb = { trigger, action: clone(b.action) };
      if (b.repeat) nb.repeat = true;
      kept.push(nb);
    }
    return kept.reverse();
  }

  function normalize(cfg) {
    // Global: no `none`, and drag only on drag:N (and drag:N only accepts drag).
    cfg.binding = dedupe(cfg.binding, (t, a) => a.type !== 'none' && t.startsWith('drag:') === (a.type === 'drag'));
    // Profiles keep `none` (= off in that app) but never drag.
    if (cfg.profile != null && !Array.isArray(cfg.profile)) throw 'invalid type: expected a sequence for `profile`';
    const profiles = (cfg.profile || []).map((p) => {
      if (!p || typeof p !== 'object') throw 'invalid type: expected struct Profile';
      if (typeof p.name !== 'string') throw 'missing field `name`';
      if (p.apps != null && !Array.isArray(p.apps)) throw 'invalid type: expected a sequence for `apps`';
      return {
        name: p.name.trim(),
        apps: (p.apps || []).map(String).filter((a) => a.trim()),
        binding: dedupe(p.binding, (t, a) => !t.startsWith('drag:') && a.type !== 'drag'),
      };
    });
    // Like `skip_serializing_if = "Vec::is_empty"`: no profiles, no key.
    if (profiles.length) cfg.profile = profiles; else delete cfg.profile;

    const s = Object.assign(clone(DEFAULT_SETTINGS), cfg.settings || {});
    for (const k of ['tap_max_ms', 'multi_tap_ms', 'hold_ms', 'drag_release_ms']) {
      if (!Number.isInteger(s[k]) || s[k] < 0) throw `invalid value for \`${k}\`: expected u64`;
    }
    for (const k of ['tap_slop_mm', 'swipe_threshold_mm', 'swipe_step_mm', 'pinch_threshold_mm', 'drag_speed', 'drag_acceleration']) {
      if (typeof s[k] !== 'number' || !Number.isFinite(s[k])) throw `invalid value for \`${k}\`: expected f32`;
    }
    for (const k of ['enabled', 'show_overlay', 'pause_in_fullscreen', 'show_copy_notice', 'onboarded', 'check_updates', 'autostart_configured',
      'animations', 'anim_swipe_glow', 'anim_hints', 'anim_pinch', 'anim_notices', 'anim_touches', 'anim_previews', 'anim_mirror', 'anim_celebrate']) {
      if (typeof s[k] !== 'boolean') throw `invalid type for \`${k}\`: expected a boolean`;
    }
    cfg.settings = s;
    return cfg;
  }

  const profiles = () => state.config.profile || [];
  const allBindings = () => state.config.binding.concat(...profiles().map((p) => p.binding));

  function dragFingers() {
    return new Set(state.config.binding.filter((b) => b.action.type === 'drag').map((b) => fingersOf(b.trigger)));
  }

  /** Pinches are only recognised for finger counts with a pinch bound somewhere. */
  function pinchBound(n) {
    return allBindings().some((b) => b.trigger.startsWith(`pinch:${n}:`) && b.action.type !== 'none');
  }

  function recompute() {
    const drag = dragFingers();
    const out = [];
    for (const b of allBindings()) {
      if (b.trigger.startsWith('swipe:') && drag.has(fingersOf(b.trigger)) && !out.includes(b.trigger)) out.push(b.trigger);
    }
    state.shadowed = out;
  }

  const appKey = (app) => {
    const base = String(app || '').trim().replace(/^"+|"+$/g, '').split(/[\\/]/).pop().toLowerCase();
    return base.endsWith('.exe') ? base.slice(0, -4) : base;
  };

  function foregroundApp() {
    return FOREGROUND[Math.floor(Date.now() / 7000) % FOREGROUND.length];
  }

  /**
   * What a trigger does while `app` is in front (pady-core `Config::resolution`):
   * `{ binding, profile }` when bound, `{ off: true, profile }` when a profile
   * switched it off, or null when unbound.
   */
  function resolve(trigger, app) {
    const key = appKey(app);
    const profile = key ? profiles().find((p) => p.apps.some((a) => appKey(a) === key)) : null;
    if (profile) {
      const b = profile.binding.find((x) => x.trigger === trigger);
      if (b) return b.action.type === 'none' ? { off: true, profile } : { binding: b, profile };
    }
    const g = state.config.binding.find((x) => x.trigger === trigger && x.action.type !== 'none');
    return g ? { binding: g, profile: null } : null;
  }

  function actionLabel(a) {
    const sign = (n) => (n >= 0 ? `+${n}` : `${n}`);
    const media = { play_pause: 'Play / pause', next: 'Next track', previous: 'Previous track', stop: 'Stop', volume_up: 'Volume up', volume_down: 'Volume down', mute: 'Mute' };
    switch (a.type) {
      case 'keys': return `Press ${a.combo}`;
      case 'volume': return `Volume ${sign(a.step)}%`;
      case 'mute': return 'Toggle mute';
      case 'brightness': return `Brightness ${sign(a.step)}%`;
      case 'media': return media[a.key];
      case 'open_url': return a.browser ? `Open ${a.url} in ${a.browser}` : `Open ${a.url}`;
      case 'launch': return `Launch ${a.program}`;
      case 'focus_or_launch': return `Switch to ${a.program}`;
      case 'run': return `Run \`${a.command}\``;
      case 'lock': return 'Lock screen';
      case 'drag': return 'Drag (hold left button)';
      default: return 'Nothing';
    }
  }

  /* ---------- Event bus ---------- */

  const listeners = new Map();
  function emit(name, payload) {
    const set = listeners.get(name);
    if (!set) return;
    for (const cb of set) {
      try { cb({ event: name, payload: clone(payload) }); } catch (e) { console.error(e); }
    }
  }
  function listen(name, cb) {
    if (!listeners.has(name)) listeners.set(name, new Set());
    listeners.get(name).add(cb);
    return Promise.resolve(() => listeners.get(name).delete(cb));
  }

  /* ---------- Pretend executor (levels for the overlay) ---------- */

  const levels = { volume: 42, muted: false, brightness: 70 };

  /** Run an action: update fake levels, emit pady://level like the engine does for the overlay. */
  function execute(action) {
    const label = actionLabel(action);
    let level = null;
    const clampPct = (v) => Math.max(0, Math.min(100, v));
    if (action.type === 'volume') { levels.volume = clampPct(levels.volume + action.step); levels.muted = false; level = { kind: 'volume', percent: levels.volume, muted: false }; }
    else if (action.type === 'mute' || (action.type === 'media' && action.key === 'mute')) { levels.muted = !levels.muted; level = { kind: 'volume', percent: levels.volume, muted: levels.muted }; }
    else if (action.type === 'media' && (action.key === 'volume_up' || action.key === 'volume_down')) {
      levels.volume = clampPct(levels.volume + (action.key === 'volume_up' ? 2 : -2)); level = { kind: 'volume', percent: levels.volume, muted: levels.muted };
    } else if (action.type === 'brightness') {
      if (q.has('brightfail')) {
        setTimeout(() => emit('pady://action-failed', { action: label, error: 'This display does not support software brightness control.' }), 60);
        return;
      }
      levels.brightness = clampPct(levels.brightness + action.step); level = { kind: 'brightness', percent: levels.brightness, muted: false };
    }
    if (level && state.config.settings.show_overlay) emit('pady://level', level);
  }

  /* ---------- Commands ---------- */

  let live = false;
  const snapshot = () => clone(state);

  const handlers = {
    get_state: () => snapshot(),
    save_config: ({ config }) => {
      if (!config) throw 'missing field `config`';
      state.config = normalize(clone(config));
      recompute();
      return snapshot();
    },
    set_enabled: ({ enabled }) => { state.config.settings.enabled = !!enabled; return snapshot(); },
    set_autostart: ({ enabled }) => { state.autostart = !!enabled; return snapshot(); },
    reset_config: () => {
      // Like the backend: fresh gestures and settings, but the tutorial stays done.
      const onboarded = state.config.settings.onboarded;
      state.config = { settings: clone(DEFAULT_SETTINGS), binding: clone(DEFAULT_BINDINGS) };
      state.config.settings.onboarded = onboarded;
      recompute();
      return snapshot();
    },
    check_update: () => (q.has('update') ? { version: '9.9.9', notes: null, can_install: !linux } : null),
    install_update: () => new Promise((done) => setTimeout(done, 1500)),
    pick_program: () => (linux ? '/usr/bin/firefox' : 'C:\\Users\\you\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Antigravity.lnk'),
    // Replace the All-apps bindings with a starter set; settings and profiles stay.
    apply_preset: ({ id }) => {
      const preset = PRESETS.find((p) => p.id === id);
      if (!preset) throw `unknown preset \`${id}\``;
      state.config = normalize(Object.assign(clone(state.config), { binding: clone(preset.binding) }));
      recompute();
      return snapshot();
    },
    test_action: ({ action }) => {
      checkAction(action);
      console.info('[mock] test_action →', actionLabel(action));
      if (action.type === 'run' && /\bfail\b/.test(action.command)) throw `command exited with status 1: ${action.command}`;
      setTimeout(() => execute(action), 0);
      return null;
    },
    open_config_folder: () => { console.info('[mock] open_config_folder'); return null; },
    open_touchpad_settings: () => { console.info('[mock] open_touchpad_settings'); return null; },
    fix_permissions: () => new Promise((done) => setTimeout(() => {
      state.conflicts = state.conflicts.filter((c) => !c.can_fix);
      state.input_error = null;
      // The backend retries the touchpad right after the rule is installed.
      if (!state.devices.length) state.devices = [{ name: 'Precision Touchpad', width_mm: 105.0, height_mm: 68.0, max_contacts: 5 }];
      done(snapshot());
    }, 1200)),
    set_live: ({ live: on }) => { live = !!on; return null; },
    // Pretend the user switches to another app after ~2.5 s (or never, with ?detectnull).
    detect_app: ({ timeoutMs }) => new Promise((done) => {
      const timeout = Math.max(1000, Math.min(30000, Number(timeoutMs) || 10000)); // backend clamps too
      const apps = linux ? ['google-chrome', 'code', 'spotify', 'firefox'] : ['chrome.exe', 'code.exe', 'spotify.exe', 'msedge.exe'];
      if (q.has('detectnull') || timeout < 2500) setTimeout(() => done(null), timeout);
      else setTimeout(() => done(apps[Math.floor(Math.random() * apps.length)]), 2500);
    }),
  };

  const errString = (e) => (typeof e === 'string' ? e : String((e && e.message) || e));

  function invoke(cmd, args) {
    const delay = (q.has('slow') ? 600 : 0) + 30 + Math.random() * 70;
    return new Promise((resolvePromise, reject) => {
      setTimeout(() => {
        let out;
        try {
          const h = handlers[cmd];
          if (!h) throw `command ${cmd} not found`;
          out = h(args || {});
        } catch (e) {
          reject(errString(e));
          return;
        }
        // Handlers may be async (detect_app).
        Promise.resolve(out).then((v) => resolvePromise(clone(v)), (e) => reject(errString(e)));
      }, delay);
    });
  }

  /* ---------- Virtual hand ----------
   * A looping script of touch "segments". Each tick computes the contact
   * positions for the current segment and fires gesture events at the moments
   * the real recognizer would (tap on lift, swipe after threshold travel,
   * pinch after the spread changes enough, etc).
   */

  const PAD_W = 105, PAD_H = 68;
  const SCRIPT_ALL = [
    { kind: 'idle', dur: 700 },
    { kind: 'point', n: 1, dur: 1500 },
    { kind: 'idle', dur: 500 },
    { kind: 'tap', n: 3, dur: 140 },
    { kind: 'idle', dur: 1100 },
    { kind: 'multitap', n: 3, count: 2, dur: 0 },
    { kind: 'idle', dur: 900 },
    { kind: 'swipe', n: 4, dir: 'up', dur: 1200 },
    { kind: 'idle', dur: 800 },
    { kind: 'pinch', n: 4, dir: 'out', dur: 1300 },
    { kind: 'idle', dur: 800 },
    { kind: 'swipe', n: 4, dir: 'down', dur: 1200 },
    { kind: 'idle', dur: 900 },
    { kind: 'drag', n: 3, dur: 1800 },
    { kind: 'idle', dur: 900 },
    { kind: 'hold', n: 4, dur: 0 },
    { kind: 'idle', dur: 800 },
    { kind: 'pinch', n: 4, dir: 'in', dur: 1300 },
    { kind: 'idle', dur: 800 },
    { kind: 'swipe', n: 5, dir: 'left', dur: 1000 },
    { kind: 'idle', dur: 900 },
    { kind: 'tap', n: 4, dur: 150 },
    { kind: 'idle', dur: 900 },
    { kind: 'swipe', n: 4, dir: 'right', dur: 1200 },
    { kind: 'idle', dur: 700 },
    { kind: 'tap', n: 5, dur: 150 },
    { kind: 'idle', dur: 900 },
    { kind: 'multitap', n: 3, count: 3, dur: 0 },
    { kind: 'idle', dur: 1300 },
  ];
  // With ?onboarding the hand loops the tutorial's practice gestures quickly,
  // so the "Try it" ticks can be watched (tap, swipe, drag, pinch first).
  const SCRIPT_PRACTICE = [
    { kind: 'idle', dur: 900 },
    { kind: 'tap', n: 3, dur: 140 },
    { kind: 'idle', dur: 900 },
    { kind: 'swipe', n: 4, dir: 'up', dur: 1100 },
    { kind: 'idle', dur: 700 },
    { kind: 'drag', n: 3, dur: 1500 },
    { kind: 'idle', dur: 700 },
    { kind: 'pinch', n: 4, dir: 'out', dur: 1200 },
    { kind: 'idle', dur: 700 },
    { kind: 'multitap', n: 3, count: 2, dur: 0 },
    { kind: 'idle', dur: 800 },
    { kind: 'hold', n: 4, dur: 0 },
    { kind: 'idle', dur: 700 },
    { kind: 'swipe', n: 4, dir: 'down', dur: 1100 },
    { kind: 'idle', dur: 700 },
    { kind: 'tap', n: 4, dur: 150 },
    { kind: 'idle', dur: 1200 },
  ];
  const SCRIPT = q.has('onboarding') ? SCRIPT_PRACTICE : SCRIPT_ALL;
  const still = q.has('still');

  const ease = (t) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);
  const clamp = (v, a, b) => Math.max(a, Math.min(b, v));

  /** Finger positions for an n-finger hand centred at (cx, cy). */
  function hand(n, cx, cy, now, seed) {
    const out = [];
    if (n === 1) return [{ x: cx, y: cy }];
    const fingers = Math.min(n, 4);
    const spread = fingers === 3 ? 16 : 14.5;
    for (let i = 0; i < fingers; i++) {
      const k = i - (fingers - 1) / 2;
      out.push({
        x: cx + k * spread + Math.sin(now / 260 + i * 1.7 + seed) * 0.35,
        y: cy + k * k * 2.6 - (Math.abs(k) < 1 ? 1.5 : 0) + Math.cos(now / 310 + i + seed) * 0.35,
      });
    }
    if (n === 5) out.unshift({ x: cx - spread * 2.1, y: cy + 15 + Math.sin(now / 280) * 0.3 }); // thumb
    return out;
  }

  /** Fingertips on an arc around (cx, cy) at radius r, thumb at the bottom: a pinching hand. */
  const PINCH_ANGLES = { 3: [-140, -40, 95], 4: [-150, -95, -40, 100], 5: [-155, -112, -68, -25, 110] };
  function pinchHand(n, cx, cy, r, now, seed) {
    return (PINCH_ANGLES[n] || PINCH_ANGLES[4]).map((deg, i) => {
      const a = (deg * Math.PI) / 180;
      return { x: cx + Math.cos(a) * r + Math.sin(now / 270 + i + seed) * 0.3, y: cy + Math.sin(a) * r * 0.85 + Math.cos(now / 300 + i) * 0.3 };
    });
  }

  /** Mean distance from the centroid — the recognizer's "spread". */
  function spreadOf(pts) {
    const cx = pts.reduce((t, p) => t + p.x, 0) / pts.length;
    const cy = pts.reduce((t, p) => t + p.y, 0) / pts.length;
    return pts.reduce((t, p) => t + Math.hypot(p.x - cx, p.y - cy), 0) / pts.length;
  }

  let segIndex = 0;
  let segStart = performance.now();
  let seg = null; // runtime copy of the current segment
  let lastContactCount = 0;
  let contactBase = 0;

  /** Is a double/triple tap bound for n fingers anywhere (globally or in a profile)? */
  function tapBound(kind, n) {
    return allBindings().some((b) => b.trigger === `${kind}:${n}` && b.action.type !== 'none');
  }

  /**
   * A run of `count` quick taps ended. Like the recognizer: with a double or
   * triple tap bound for that finger count, it waits `multi_tap_ms` to see
   * whether more taps follow before reporting what it saw.
   */
  function endTaps(n, count) {
    const dbl = tapBound('doubletap', n), tpl = tapBound('tripletap', n);
    const kinds = count >= 3 && tpl ? ['triple_tap']
      : count >= 2 && dbl ? ['double_tap'].concat(count >= 3 ? ['tap'] : [])
        : Array.from({ length: count }, () => 'tap');
    const report = () => kinds.forEach((kind) => fire({ kind, fingers: n }));
    if (dbl || tpl) setTimeout(report, state.config.settings.multi_tap_ms);
    else report();
  }

  function startSegment(now) {
    const s = SCRIPT[segIndex % SCRIPT.length];
    seg = Object.assign({}, s, { fired: false, dragging: false, steps: 0, seed: Math.random() * 10 });
    if (s.kind === 'hold') seg.dur = state.config.settings.hold_ms + 450;
    if (s.kind === 'multitap') seg.dur = s.count * 240 + 40; // 110 ms down, 130 ms up per tap
    // A swipe with a drag-owned finger count is a drag in reality.
    if (s.kind === 'swipe' && dragFingers().has(s.n)) { seg.kind = 'drag'; seg.dir = s.dir; }
    // ...and a drag without a drag binding is recognised as a swipe.
    if (s.kind === 'drag' && !dragFingers().has(s.n)) { seg.kind = 'swipe'; seg.dir = 'right'; }
    segStart = now;
  }

  function triggerOf(g) {
    switch (g.kind) {
      case 'tap': case 'hold': return `${g.kind}:${g.fingers}`;
      case 'double_tap': return `doubletap:${g.fingers}`;
      case 'triple_tap': return `tripletap:${g.fingers}`;
      case 'swipe': case 'pinch': return `${g.kind}:${g.fingers}:${g.direction}`;
      case 'drag_start': return `drag:${g.fingers}`;
      default: return null;
    }
  }

  /** Emit a gesture the way pady-core's engine does (resolution, repeat filtering, profile name). */
  function fire(gesture) {
    if (!state.config.settings.enabled) return; // paused: the recognizer doesn't run at all
    if (gesture.kind === 'drag_end') { emit('pady://gesture', { gesture, trigger: null, action: null, profile: null }); return; }
    const trigger = triggerOf(gesture);
    if (gesture.kind === 'drag_start') {
      emit('pady://gesture', { gesture, trigger, action: actionLabel({ type: 'drag' }), profile: null });
      return;
    }
    const step = gesture.step || 0;
    const app = profiles().length ? foregroundApp() : null;
    const r = resolve(trigger, app);
    const bound = r && r.binding && (step === 0 || r.binding.repeat) ? r : null;
    if (step > 0 && !bound) return; // repeat steps nothing repeats are noise
    // A profile that switched the gesture off is still named, so Live can say "Off in this app".
    const profile = r && r.profile && (bound || r.off) ? r.profile.name : null;
    emit('pady://gesture', { gesture, trigger, action: bound ? actionLabel(bound.binding.action) : null, profile });
    if (bound) execute(bound.binding.action);
  }

  function contactsFor(now) {
    const t = now - segStart;
    const p = clamp(t / seg.dur, 0, 1);
    const S = state.config.settings;
    switch (seg.kind) {
      case 'idle': return [];
      case 'point': {
        const e = ease(p);
        return [{ x: 28 + e * 50 + Math.sin(e * Math.PI) * 4, y: 48 - e * 26 + Math.sin(e * Math.PI * 2) * 5 }];
      }
      case 'tap': return hand(seg.n, 54, 36, now, seg.seed);
      case 'multitap': return t % 240 < 110 ? hand(seg.n, 54, 36, now, seg.seed) : [];
      case 'hold': {
        if (t >= S.hold_ms && !seg.fired) { seg.fired = true; fire({ kind: 'hold', fingers: seg.n }); }
        return p < 0.97 ? hand(seg.n, 54, 37, now, seg.seed) : [];
      }
      case 'pinch': {
        const lift = 0.86;
        if (p >= lift) return [];
        const mp = ease(clamp((p - 0.1) / (lift - 0.14), 0, 1));
        const [r0, r1] = seg.dir === 'out' ? [8, 27] : [27, 8];
        const pts = pinchHand(seg.n, 54, 36, r0 + (r1 - r0) * mp, now, seg.seed);
        if (seg.spread0 == null) seg.spread0 = spreadOf(pts);
        const change = Math.abs(spreadOf(pts) - seg.spread0);
        if (pinchBound(seg.n) && change >= S.pinch_threshold_mm) {
          // Step 0 at the threshold, then one step per `swipe_step_mm` of further spread change.
          const steps = Math.floor((change - S.pinch_threshold_mm) / Math.max(1, S.swipe_step_mm));
          if (!seg.fired) { seg.fired = true; fire({ kind: 'pinch', fingers: seg.n, direction: seg.dir, step: 0 }); }
          while (seg.steps < steps) fire({ kind: 'pinch', fingers: seg.n, direction: seg.dir, step: ++seg.steps });
        }
        return pts;
      }
      case 'swipe': case 'drag': {
        const lift = 0.86;
        if (p >= lift) {
          if (seg.dragging && !seg.ended) { seg.ended = true; fire({ kind: 'drag_end' }); }
          return [];
        }
        const mp = ease(clamp((p - 0.1) / (lift - 0.12), 0, 1)); // move progress
        let dx = 0, dy = 0;
        if (seg.kind === 'drag' && !seg.dir) {
          // free-form drag along a gentle S curve
          dx = -26 + mp * 52; dy = Math.sin(mp * Math.PI * 1.6) * 9 - 2;
        } else {
          const d = seg.dir;
          const span = d === 'up' || d === 'down' ? 34 : 52;
          const off = (mp - 0.5) * span;
          if (d === 'up') dy = -off; else if (d === 'down') dy = off;
          else if (d === 'left') dx = -off; else dx = off;
        }
        const travel = seg.dir ? mp * (seg.dir === 'up' || seg.dir === 'down' ? 34 : 52) : mp * 55;
        if (seg.kind === 'drag') {
          if (!seg.dragging && travel >= S.swipe_threshold_mm) { seg.dragging = true; fire({ kind: 'drag_start', fingers: seg.n }); }
        } else if (travel >= S.swipe_threshold_mm) {
          // The recognizer reports every step; the engine drops the ones nobody repeats.
          const steps = Math.floor((travel - S.swipe_threshold_mm) / Math.max(1, S.swipe_step_mm));
          if (!seg.fired) { seg.fired = true; fire({ kind: 'swipe', fingers: seg.n, direction: seg.dir, step: 0 }); }
          while (seg.steps < steps) fire({ kind: 'swipe', fingers: seg.n, direction: seg.dir, step: ++seg.steps });
        }
        return hand(seg.n, 54 + dx, 36 + dy, now, seg.seed);
      }
      default: return [];
    }
  }

  function tick() {
    if (still || state.devices.length === 0) return; // nothing to touch
    const now = performance.now();
    if (!seg) startSegment(now);
    if (now - segStart >= seg.dur) {
      if (seg.kind === 'tap') endTaps(seg.n, 1);
      if (seg.kind === 'multitap') endTaps(seg.n, seg.count);
      if (seg.dragging && !seg.ended) fire({ kind: 'drag_end' });
      segIndex++;
      startSegment(now);
    }
    const pts = contactsFor(now);
    if (pts.length && !lastContactCount) contactBase = (contactBase + 7) % 40; // fresh ids per touch
    // Only stream while live, and only when something touches (plus one empty frame on lift).
    if (live && (pts.length || lastContactCount)) {
      emit('pady://frame', {
        time_ms: Math.round(now),
        contacts: pts.map((c, i) => ({ id: contactBase + i, x: +clamp(c.x, 0, PAD_W).toFixed(2), y: +clamp(c.y, 0, PAD_H).toFixed(2) })),
        width_mm: PAD_W,
        height_mm: PAD_H,
      });
    }
    lastContactCount = pts.length;
  }

  state.config = normalize(state.config);
  recompute();
  setInterval(tick, 1000 / 90);

  window.PadyMock = { invoke, listen, emit, state, foregroundApp };
  console.info('%cPady%c running with the in-browser mock backend', 'color:#8b7dff;font-weight:700', 'color:inherit');
})();
