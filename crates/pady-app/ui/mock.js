/*
 * Pady — browser preview backend.
 *
 * Only active when `window.__TAURI__` is absent (the page was opened in a
 * normal browser). It exposes `window.PadyMock = { invoke, listen }` with the
 * same contract as the Rust side, keeps state in memory, and runs a scripted
 * "virtual hand" on a loop that emits `pady://frame` (while live) and
 * `pady://gesture` events so every screen can be previewed.
 *
 * Preview switches (query string):
 *   ?platform=linux   Linux labels / presets
 *   ?nodevice         no touchpad + input_error
 *   ?noconflicts      empty conflicts list
 *   ?theme=light|dark force a theme regardless of the OS setting
 *   ?brightfail       brightness actions emit pady://action-failed
 *   ?slow             add 600ms latency to every command
 */
(function () {
  'use strict';
  if (window.__TAURI__) return;

  const q = new URLSearchParams(location.search);
  const platform = q.get('platform') === 'linux' ? 'linux' : 'windows';
  if (q.get('theme')) document.documentElement.dataset.theme = q.get('theme');

  const clone = (v) => (v === undefined ? null : JSON.parse(JSON.stringify(v)));

  const DEFAULT_SETTINGS = {
    enabled: true, tap_max_ms: 220, tap_slop_mm: 4.0, swipe_threshold_mm: 10.0, swipe_step_mm: 7.0,
    hold_ms: 600, drag_speed: 9.0, drag_acceleration: 1.0, drag_release_ms: 350,
  };
  const DEFAULT_BINDINGS = [
    { trigger: 'drag:3', action: { type: 'drag' } },
    { trigger: 'tap:3', action: { type: 'media', key: 'play_pause' } },
    { trigger: 'swipe:4:up', action: { type: 'volume', step: 4 }, repeat: true },
    { trigger: 'swipe:4:down', action: { type: 'volume', step: -4 }, repeat: true },
    { trigger: 'swipe:4:right', action: { type: 'brightness', step: 10 }, repeat: true },
    { trigger: 'swipe:4:left', action: { type: 'brightness', step: -10 }, repeat: true },
    { trigger: 'tap:4', action: { type: 'keys', combo: 'win+d' } },
    { trigger: 'hold:4', action: { type: 'lock' } },
  ];

  const noDevice = q.has('nodevice');
  const state = {
    config: {
      settings: clone(DEFAULT_SETTINGS),
      // The preview starts with two extra bindings so the "shadowed" badge and
      // an open_url chip are visible out of the box.
      binding: clone(DEFAULT_BINDINGS).concat([
        { trigger: 'swipe:3:up', action: { type: 'keys', combo: 'win+tab' } },
        { trigger: 'tap:5', action: { type: 'open_url', url: 'https://youtube.com', browser: 'chrome' } },
      ]),
    },
    devices: noDevice ? [] : [{ name: 'Precision Touchpad', width_mm: 105.0, height_mm: 68.0, max_contacts: 5 }],
    input_error: noDevice
      ? (platform === 'linux'
        ? 'No touchpad found under /dev/input. Add your user to the "input" group and log in again.'
        : 'No Precision Touchpad was found. Pady needs a touchpad that uses the Windows Precision driver.')
      : null,
    conflicts: q.has('noconflicts') ? [] : [{
      message: platform === 'linux' ? 'GNOME also handles three-finger swipes.' : 'Windows also handles three-finger swipes.',
      fix: platform === 'linux'
        ? 'Disable workspace gestures with a GNOME extension, or bind only 4- and 5-finger swipes.'
        : 'Set three-finger swipes to "Nothing" in Touchpad settings.',
      can_open_settings: platform !== 'linux',
    }],
    shadowed: [],
    autostart: false,
    config_path: platform === 'linux' ? '/home/me/.config/pady/config.toml' : 'C:\\Users\\me\\AppData\\Roaming\\Pady\\config.toml',
    platform,
    version: '0.1.0',
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

  function checkTrigger(t) {
    const m = /^(tap|hold|drag):([345])$|^swipe:([345]):(up|down|left|right)$/.exec(String(t).trim().toLowerCase());
    if (!m) throw `invalid trigger \`${t}\` (expected e.g. tap:3, hold:4, swipe:3:up, drag:3)`;
    return String(t).trim().toLowerCase();
  }

  function checkAction(a) {
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

  function normalize(cfg) {
    const seen = new Set();
    const kept = [];
    for (const b of (cfg.binding || []).slice().reverse()) {
      const trigger = checkTrigger(b.trigger);
      checkAction(b.action);
      const valid = trigger.startsWith('drag:') === (b.action.type === 'drag');
      if (!valid || b.action.type === 'none' || seen.has(trigger)) continue;
      seen.add(trigger);
      const nb = { trigger, action: clone(b.action) };
      if (b.repeat) nb.repeat = true;
      kept.push(nb);
    }
    cfg.binding = kept.reverse();
    const s = Object.assign(clone(DEFAULT_SETTINGS), cfg.settings || {});
    for (const k of ['tap_max_ms', 'hold_ms', 'drag_release_ms']) s[k] = Math.max(0, Math.round(Number(s[k]) || 0));
    cfg.settings = s;
    return cfg;
  }

  function dragFingers() {
    return new Set(state.config.binding.filter((b) => b.action.type === 'drag').map((b) => Number(b.trigger.split(':')[1])));
  }

  function recompute() {
    const drag = dragFingers();
    state.shadowed = state.config.binding
      .filter((b) => b.trigger.startsWith('swipe:') && drag.has(Number(b.trigger.split(':')[1])))
      .map((b) => b.trigger);
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
      state.config = { settings: clone(DEFAULT_SETTINGS), binding: clone(DEFAULT_BINDINGS) };
      recompute();
      return snapshot();
    },
    test_action: ({ action }) => {
      checkAction(action);
      console.info('[mock] test_action →', actionLabel(action));
      if (action.type === 'run' && /\bfail\b/.test(action.command)) throw `command exited with status 1: ${action.command}`;
      if (action.type === 'brightness' && q.has('brightfail')) {
        setTimeout(() => emit('pady://action-failed', { action: actionLabel(action), error: 'This display does not support software brightness control.' }), 120);
      }
      return null;
    },
    open_config_folder: () => { console.info('[mock] open_config_folder'); return null; },
    open_touchpad_settings: () => { console.info('[mock] open_touchpad_settings'); return null; },
    set_live: ({ live: on }) => { live = !!on; return null; },
  };

  function invoke(cmd, args) {
    const delay = (q.has('slow') ? 600 : 0) + 30 + Math.random() * 70;
    return new Promise((resolve, reject) => {
      setTimeout(() => {
        try {
          const h = handlers[cmd];
          if (!h) throw `command ${cmd} not found`;
          resolve(clone(h(args || {})));
        } catch (e) {
          reject(typeof e === 'string' ? e : String(e && e.message || e));
        }
      }, delay);
    });
  }

  /* ---------- Virtual hand ----------
   * A looping script of touch "segments". Each tick computes the contact
   * positions for the current segment and fires gesture events at the moments
   * the real recognizer would (tap on lift, swipe after threshold travel, etc).
   */

  const PAD_W = 105, PAD_H = 68;
  const SCRIPT = [
    { kind: 'idle', dur: 700 },
    { kind: 'point', n: 1, dur: 1500 },
    { kind: 'idle', dur: 500 },
    { kind: 'tap', n: 3, dur: 140 },
    { kind: 'idle', dur: 1100 },
    { kind: 'swipe', n: 4, dir: 'up', dur: 1200 },
    { kind: 'idle', dur: 800 },
    { kind: 'swipe', n: 4, dir: 'down', dur: 1200 },
    { kind: 'idle', dur: 900 },
    { kind: 'drag', n: 3, dur: 1800 },
    { kind: 'idle', dur: 900 },
    { kind: 'hold', n: 4, dur: 0 },
    { kind: 'idle', dur: 800 },
    { kind: 'swipe', n: 5, dir: 'left', dur: 1000 },
    { kind: 'idle', dur: 900 },
    { kind: 'tap', n: 4, dur: 150 },
    { kind: 'idle', dur: 900 },
    { kind: 'swipe', n: 4, dir: 'right', dur: 1200 },
    { kind: 'idle', dur: 700 },
    { kind: 'tap', n: 5, dur: 150 },
    { kind: 'idle', dur: 1300 },
  ];

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

  let segIndex = 0;
  let segStart = performance.now();
  let seg = null; // runtime copy of the current segment
  let lastContactCount = 0;
  let contactBase = 0;

  function startSegment(now) {
    const s = SCRIPT[segIndex % SCRIPT.length];
    seg = Object.assign({}, s, { fired: 0, dragging: false, steps: 0, seed: Math.random() * 10 });
    if (s.kind === 'hold') seg.dur = state.config.settings.hold_ms + 450;
    // A swipe with a drag-owned finger count is a drag in reality.
    if (s.kind === 'swipe' && dragFingers().has(s.n)) { seg.kind = 'drag'; seg.dir = s.dir; }
    // ...and a drag without a drag binding is recognised as a swipe.
    if (s.kind === 'drag' && !dragFingers().has(s.n)) { seg.kind = 'swipe'; seg.dir = 'right'; }
    segStart = now;
  }

  function binding(trigger) {
    return state.config.binding.find((b) => b.trigger === trigger) || null;
  }

  function fire(gesture) {
    let trigger = null;
    if (gesture.kind === 'tap' || gesture.kind === 'hold') trigger = `${gesture.kind}:${gesture.fingers}`;
    else if (gesture.kind === 'swipe') trigger = `swipe:${gesture.fingers}:${gesture.direction}`;
    else if (gesture.kind === 'drag_start') trigger = `drag:${gesture.fingers}`;
    const b = trigger && binding(trigger);
    const action = b && state.config.settings.enabled ? actionLabel(b.action) : null;
    emit('pady://gesture', { gesture, trigger, action });
    if (action && b.action.type === 'brightness' && q.has('brightfail')) {
      emit('pady://action-failed', { action, error: 'This display does not support software brightness control.' });
    }
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
      case 'hold': {
        if (t >= S.hold_ms && !seg.fired) { seg.fired = 1; fire({ kind: 'hold', fingers: seg.n }); }
        return p < 0.97 ? hand(seg.n, 54, 37, now, seg.seed) : [];
      }
      case 'swipe': case 'drag': {
        const lift = 0.86;
        if (p >= lift) {
          if (seg.dragging && !seg.ended) { seg.ended = true; fire({ kind: 'drag_end' }); }
          return [];
        }
        const mp = ease(clamp((p - 0.1) / (lift - 0.12), 0, 1)); // move progress
        let dx = 0, dy = 0, cx = 54, cy = 36;
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
        const travel = seg.dir
          ? mp * (seg.dir === 'up' || seg.dir === 'down' ? 34 : 52)
          : mp * 55;
        if (seg.kind === 'drag') {
          if (!seg.dragging && travel >= S.swipe_threshold_mm) { seg.dragging = true; fire({ kind: 'drag_start', fingers: seg.n }); }
        } else if (travel >= S.swipe_threshold_mm) {
          const b = binding(`swipe:${seg.n}:${seg.dir}`);
          const steps = b && b.repeat ? Math.floor((travel - S.swipe_threshold_mm) / Math.max(1, S.swipe_step_mm)) : 0;
          while (seg.fired === 0 || seg.steps < steps) {
            const step = seg.fired === 0 ? 0 : ++seg.steps;
            seg.fired = 1;
            fire({ kind: 'swipe', fingers: seg.n, direction: seg.dir, step });
          }
        }
        return hand(seg.n, cx + dx, cy + dy, now, seg.seed);
      }
      default: return [];
    }
  }

  function tick() {
    const now = performance.now();
    if (!seg) startSegment(now);
    if (now - segStart >= seg.dur) {
      if (seg.kind === 'tap') fire({ kind: 'tap', fingers: seg.n });
      if (seg.dragging && !seg.ended) fire({ kind: 'drag_end' });
      segIndex++;
      startSegment(now);
    }
    if (state.devices.length === 0) return;
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

  recompute();
  setInterval(tick, 1000 / 90);

  window.PadyMock = { invoke, listen, emit, state };
  console.info('%cPady%c running with the in-browser mock backend', 'color:#8b7dff;font-weight:700', 'color:inherit');
})();
