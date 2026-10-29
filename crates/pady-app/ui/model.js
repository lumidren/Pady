/*
 * Pady — domain model shared by the UI: gesture slots, action catalogue,
 * key-combo grammar (mirrors pady-core/src/keys.rs), labels, presets and
 * per-app profile helpers (mirrors pady-core/src/config.rs).
 * Pure functions only; no DOM access.
 */
(function () {
  'use strict';
  const P = (window.Pady = window.Pady || {});
  const M = (P.model = {});

  /* ---------------- Gesture slots ---------------- */

  M.FINGERS = [3, 4, 5];

  /*
   * Two-finger shapes: recorded strokes bound as `shape:<id>`. The app keeps
   * the config's list here (`M.setShapes`) so labels can use their names.
   */
  M.SHAPE_FINGERS = 2;
  M.MAX_SHAPES = 32;
  M.shapes = [];
  M.setShapes = (config) => { M.shapes = (config && Array.isArray(config.shape)) ? config.shape : []; };
  M.isShape = (t) => String(t || '').startsWith('shape:');
  M.shapeById = (id) => M.shapes.find((sh) => sh.id === Number(id)) || null;
  M.shapeName = (id) => { const sh = M.shapeById(id); return sh ? sh.name : `Shape ${id}`; };
  /** The lowest free shape id, or null when all are taken. */
  M.nextShapeId = (config) => {
    const used = new Set(((config && config.shape) || []).map((sh) => sh.id));
    for (let id = 1; id <= M.MAX_SHAPES; id++) if (!used.has(id)) return id;
    return null;
  };
  /** A copy of config without a shape and anything bound to it. */
  M.withoutShape = function (config, id) {
    const next = JSON.parse(JSON.stringify(config));
    const t = `shape:${id}`;
    next.shape = (next.shape || []).filter((sh) => sh.id !== id);
    next.binding = (next.binding || []).filter((b) => b.trigger !== t);
    for (const p of next.profile || []) p.binding = (p.binding || []).filter((b) => b.trigger !== t);
    return next;
  };

  M.SLOTS = [
    { id: 'tap', kind: 'tap', name: 'Tap', hint: 'Touch and lift quickly' },
    { id: 'doubletap', kind: 'doubletap', name: 'Double tap', hint: 'Tap twice quickly' },
    { id: 'tripletap', kind: 'tripletap', name: 'Triple tap', hint: 'Tap three times quickly' },
    { id: 'hold', kind: 'hold', name: 'Hold', hint: 'Rest fingers without moving' },
    { id: 'swipe-up', kind: 'swipe', dir: 'up', name: 'Swipe up', hint: 'Slide upward' },
    { id: 'swipe-down', kind: 'swipe', dir: 'down', name: 'Swipe down', hint: 'Slide downward' },
    { id: 'swipe-left', kind: 'swipe', dir: 'left', name: 'Swipe left', hint: 'Slide to the left' },
    { id: 'swipe-right', kind: 'swipe', dir: 'right', name: 'Swipe right', hint: 'Slide to the right' },
    { id: 'pinch-in', kind: 'pinch', dir: 'in', name: 'Pinch in', hint: 'Bring fingers together' },
    { id: 'pinch-out', kind: 'pinch', dir: 'out', name: 'Pinch out', hint: 'Spread fingers apart' },
    { id: 'drag', kind: 'drag', name: 'Drag', hint: 'Move a window or selection' },
  ];

  M.triggerOf = (slot, n) => (slot.dir ? `${slot.kind}:${n}:${slot.dir}` : `${slot.kind}:${n}`);

  /** Swipes and pinches can keep firing while the fingers keep moving (`repeat`). */
  M.isContinuous = (slot) => !!slot && (slot.kind === 'swipe' || slot.kind === 'pinch');

  /** Double and triple taps (binding one makes single taps wait for `multi_tap_ms`). */
  M.isMultiTap = (slot) => !!slot && (slot.kind === 'doubletap' || slot.kind === 'tripletap');

  // Trigger kind → words used in labels ("3-finger double tap").
  const KIND_WORDS = { tap: 'tap', doubletap: 'double tap', tripletap: 'triple tap', hold: 'hold', swipe: 'swipe', pinch: 'pinch', drag: 'drag' };
  M.NUMBER_WORDS = { 1: 'one', 2: 'two', 3: 'three', 4: 'four', 5: 'five' };

  M.parseTrigger = function (t) {
    const [kind, n, dir] = String(t || '').split(':');
    if (kind === 'shape') return { kind, fingers: M.SHAPE_FINGERS, id: Number(n), dir: null };
    return { kind, fingers: Number(n), dir: dir || null };
  };

  M.slotOfTrigger = function (t) {
    const p = M.parseTrigger(t);
    if (p.kind === 'shape') return { id: 'shape', kind: 'shape', shape: p.id, name: M.shapeName(p.id), hint: 'Draw it with two fingers' };
    return M.SLOTS.find((s) => s.kind === p.kind && (s.dir || null) === p.dir) || null;
  };

  /** "4-finger swipe up", "3-finger double tap" */
  M.triggerLabel = function (t) {
    const p = M.parseTrigger(t);
    if (p.kind === 'shape') return `“${M.shapeName(p.id)}” shape`;
    return `${p.fingers}-finger ${KIND_WORDS[p.kind] || p.kind}${p.dir ? ' ' + p.dir : ''}`;
  };

  /** Plain instruction for a trigger: "Swipe up with four fingers", "Double-tap with three fingers". */
  M.triggerInstruction = function (t) {
    const p = M.parseTrigger(t);
    const n = M.NUMBER_WORDS[p.fingers] || p.fingers;
    switch (p.kind) {
      case 'tap': return `Tap with ${n} fingers`;
      case 'doubletap': return `Double-tap with ${n} fingers`;
      case 'tripletap': return `Triple-tap with ${n} fingers`;
      case 'hold': return `Rest ${n} fingers still for a moment`;
      case 'swipe': return `Swipe ${p.dir} with ${n} fingers`;
      case 'pinch': return p.dir === 'in' ? `Pinch ${n} fingers together` : `Spread ${n} fingers apart`;
      case 'drag': return `Drag with ${n} fingers`;
      case 'shape': return `Draw “${M.shapeName(p.id)}” with two fingers`;
      default: return M.triggerLabel(t);
    }
  };

  M.gestureLabel = function (g) {
    if (!g) return 'Gesture';
    const f = g.fingers ? `${g.fingers}-finger ` : '';
    switch (g.kind) {
      case 'tap': return `${f}tap`;
      case 'double_tap': return `${f}double tap`;
      case 'triple_tap': return `${f}triple tap`;
      case 'hold': return `${f}hold`;
      case 'swipe': return `${f}swipe ${g.direction}`;
      case 'pinch': return `${f}pinch ${g.direction}`;
      case 'drag_start': return `${f}drag`;
      case 'drag_end': return 'Drag released';
      case 'shape': return `“${M.shapeName(g.id)}” shape`;
      default: return g.kind;
    }
  };

  /* ---------------- Action catalogue ---------------- */

  M.GROUPS = [
    { id: 'system', label: 'System' },
    { id: 'media', label: 'Media' },
    { id: 'keyboard', label: 'Keyboard' },
    { id: 'web', label: 'Apps & web' },
    { id: 'pointer', label: 'Pointer' },
  ];

  M.TYPES = {
    volume: { label: 'Volume', icon: 'volume', group: 'system', make: () => ({ type: 'volume', step: 4 }) },
    brightness: { label: 'Brightness', icon: 'sun', group: 'system', make: () => ({ type: 'brightness', step: 10 }) },
    mute: { label: 'Mute', icon: 'mute', group: 'system', make: () => ({ type: 'mute' }) },
    lock: { label: 'Lock', icon: 'lock', group: 'system', make: () => ({ type: 'lock' }) },
    media: { label: 'Media key', icon: 'play-pause', group: 'media', make: () => ({ type: 'media', key: 'play_pause' }) },
    keys: { label: 'Shortcut', icon: 'keyboard', group: 'keyboard', make: () => ({ type: 'keys', combo: '' }) },
    open_url: { label: 'Open link', icon: 'globe', group: 'web', make: () => ({ type: 'open_url', url: '' }) },
    focus_or_launch: { label: 'Switch to app', icon: 'switch', group: 'web', make: () => ({ type: 'focus_or_launch', program: '' }) },
    launch: { label: 'Launch app', icon: 'apps', group: 'web', make: () => ({ type: 'launch', program: '' }) },
    run: { label: 'Run command', icon: 'terminal', group: 'web', make: () => ({ type: 'run', command: '' }) },
    drag: { label: 'Drag', icon: 'drag', group: 'pointer', make: () => ({ type: 'drag' }) },
    none: { label: 'Nothing', icon: 'none', group: null, make: () => ({ type: 'none' }) },
  };

  M.MEDIA_KEYS = [
    { key: 'previous', label: 'Previous', long: 'Previous track', icon: 'prev' },
    { key: 'play_pause', label: 'Play/Pause', long: 'Play / pause', icon: 'play-pause' },
    { key: 'next', label: 'Next', long: 'Next track', icon: 'next' },
    { key: 'stop', label: 'Stop', long: 'Stop', icon: 'stop' },
    { key: 'volume_down', label: 'Vol −', long: 'Volume down (OS overlay)', icon: 'volume-down' },
    { key: 'volume_up', label: 'Vol +', long: 'Volume up (OS overlay)', icon: 'volume' },
    { key: 'mute', label: 'Mute', long: 'Mute (OS overlay)', icon: 'mute' },
  ];

  M.BROWSERS = [
    { id: '', label: 'Default browser' },
    { id: 'chrome', label: 'Google Chrome' },
    { id: 'firefox', label: 'Firefox' },
    { id: 'edge', label: 'Microsoft Edge' },
    { id: 'brave', label: 'Brave' },
  ];

  const browserName = (id) => ({ chrome: 'Chrome', firefox: 'Firefox', edge: 'Edge', brave: 'Brave' }[id] || id);

  /* ---------------- Key combos ---------------- */

  const KEY_ALIASES = {
    return: 'enter', escape: 'esc', del: 'delete', ins: 'insert', pgup: 'pageup', pgdn: 'pagedown',
    prtsc: 'printscreen', print: 'printscreen', '-': 'minus', plus: 'equal', '=': 'equal', ',': 'comma',
    '.': 'period', '/': 'slash', '\\': 'backslash', ';': 'semicolon', "'": 'quote', '[': 'bracketleft',
    ']': 'bracketright', backtick: 'grave', '`': 'grave',
  };
  const KEY_NAMES = 'abcdefghijklmnopqrstuvwxyz0123456789'.split('').concat(
    Array.from({ length: 12 }, (_, i) => 'f' + (i + 1)),
    ['enter', 'esc', 'tab', 'space', 'backspace', 'delete', 'insert', 'home', 'end', 'pageup', 'pagedown',
      'up', 'down', 'left', 'right', 'printscreen', 'minus', 'equal', 'comma', 'period', 'slash', 'backslash',
      'semicolon', 'quote', 'bracketleft', 'bracketright', 'grave']);
  const KEY_SET = new Set(KEY_NAMES);
  const MOD_ALIASES = { ctrl: 'ctrl', control: 'ctrl', alt: 'alt', option: 'alt', shift: 'shift', win: 'win', super: 'win', meta: 'win', cmd: 'win', windows: 'win' };
  M.MOD_ORDER = ['ctrl', 'alt', 'shift', 'win'];

  /**
   * Parse a combo string the same way the backend does.
   * Returns { ok, combo (canonical), mods:Set, key, error }.
   */
  M.parseCombo = function (str) {
    const parts = String(str || '').split('+').map((p) => p.trim()).filter(Boolean);
    if (!parts.length) return { ok: false, error: 'Press a shortcut or type one, e.g. ctrl+shift+t' };
    const mods = new Set();
    let key = null;
    for (const part of parts) {
      const l = part.toLowerCase();
      if (MOD_ALIASES[l]) { mods.add(MOD_ALIASES[l]); continue; }
      const k = KEY_ALIASES[l] || l;
      if (!KEY_SET.has(k)) return { ok: false, error: `Unknown key “${part}”` };
      if (key) return { ok: false, error: `Only one main key allowed (“${key}” and “${k}”)` };
      key = k;
    }
    const combo = M.MOD_ORDER.filter((m) => mods.has(m)).concat(key ? [key] : []).join('+');
    return { ok: true, combo, mods, key };
  };

  const KEY_LABELS = {
    enter: 'Enter', esc: 'Esc', tab: 'Tab', space: 'Space', backspace: '⌫ Backspace', delete: 'Del', insert: 'Ins',
    home: 'Home', end: 'End', pageup: 'PgUp', pagedown: 'PgDn', up: '↑', down: '↓', left: '←', right: '→',
    printscreen: 'PrtSc', minus: '-', equal: '=', comma: ',', period: '.', slash: '/', backslash: '\\',
    semicolon: ';', quote: "'", bracketleft: '[', bracketright: ']', grave: '`',
  };

  M.modLabel = (m, platform) => ({ ctrl: 'Ctrl', alt: 'Alt', shift: 'Shift', win: platform === 'linux' ? 'Super' : 'Win' }[m]);
  M.keyLabel = (k) => KEY_LABELS[k] || (k.length === 1 ? k.toUpperCase() : k.toUpperCase());

  /** Keycap labels for display, e.g. ["Win", "Shift", "S"]. Invalid combos fall back to raw parts. */
  M.keycaps = function (combo, platform) {
    const p = M.parseCombo(combo);
    if (!p.ok) return String(combo || '').split('+').filter(Boolean);
    return M.MOD_ORDER.filter((m) => p.mods.has(m)).map((m) => M.modLabel(m, platform)).concat(p.key ? [M.keyLabel(p.key)] : []);
  };

  /** Map a KeyboardEvent.code to a grammar key name (null for modifiers / unsupported). */
  M.keyFromCode = function (code) {
    if (!code) return null;
    let m;
    if ((m = /^Key([A-Z])$/.exec(code))) return m[1].toLowerCase();
    if ((m = /^(?:Digit|Numpad)([0-9])$/.exec(code))) return m[1];
    if ((m = /^F([0-9]{1,2})$/.exec(code)) && +m[1] >= 1 && +m[1] <= 12) return 'f' + m[1];
    const table = {
      Enter: 'enter', NumpadEnter: 'enter', Escape: 'esc', Tab: 'tab', Space: 'space', Backspace: 'backspace',
      Delete: 'delete', Insert: 'insert', Home: 'home', End: 'end', PageUp: 'pageup', PageDown: 'pagedown',
      ArrowUp: 'up', ArrowDown: 'down', ArrowLeft: 'left', ArrowRight: 'right', PrintScreen: 'printscreen',
      Minus: 'minus', NumpadSubtract: 'minus', Equal: 'equal', NumpadAdd: 'equal', Comma: 'comma', Period: 'period',
      NumpadDecimal: 'period', Slash: 'slash', NumpadDivide: 'slash', Backslash: 'backslash', IntlBackslash: 'backslash',
      Semicolon: 'semicolon', Quote: 'quote', BracketLeft: 'bracketleft', BracketRight: 'bracketright', Backquote: 'grave',
    };
    return table[code] || null;
  };

  M.isModifierCode = (code) => /^(Control|Alt|Shift|Meta|OS)(Left|Right)?$/.test(code || '');

  /* ---------------- Args helpers ---------------- */

  /** Split a command-line-ish string into args, honouring "double" and 'single' quotes. */
  M.parseArgs = function (str) {
    const out = [];
    const re = /"([^"]*)"|'([^']*)'|(\S+)/g;
    let m;
    while ((m = re.exec(String(str || '')))) out.push(m[1] ?? m[2] ?? m[3]);
    return out;
  };
  M.formatArgs = (args) => (args || []).map((a) => (/\s/.test(a) || a === '' ? `"${a}"` : a)).join(' ');

  /* ---------------- Labels ---------------- */

  const signed = (n) => (n > 0 ? `+${n}` : n < 0 ? `−${Math.abs(n)}` : '0');
  M.signed = signed;

  const prettyUrl = (u) => String(u || '').replace(/^https?:\/\//i, '').replace(/^www\./i, '').replace(/\/$/, '');
  const programName = (p) => String(p || '').split(/[\\/]/).pop();

  /** Rich summary for chips: { icon, text, keys? } */
  M.summarize = function (action, platform) {
    const a = action || { type: 'none' };
    const t = M.TYPES[a.type] || M.TYPES.none;
    switch (a.type) {
      case 'volume': return { icon: a.step < 0 ? 'volume-down' : 'volume', text: `Volume ${signed(a.step)}%` };
      case 'brightness': return { icon: a.step < 0 ? 'sun-dim' : 'sun', text: `Brightness ${signed(a.step)}%` };
      case 'mute': return { icon: 'mute', text: 'Toggle mute' };
      case 'lock': return { icon: 'lock', text: 'Lock screen' };
      case 'media': {
        const k = M.MEDIA_KEYS.find((x) => x.key === a.key);
        return { icon: k ? k.icon : 'play-pause', text: k ? k.long.replace(' (OS overlay)', '') : a.key };
      }
      case 'keys': { const caps = M.keycaps(a.combo, platform); return { icon: 'keyboard', text: caps.join('+'), keys: caps }; }
      case 'open_url': return { icon: /youtube\./i.test(a.url) ? 'youtube' : 'globe', text: `Open ${prettyUrl(a.url)}${a.browser ? ' in ' + browserName(a.browser) : ''}` };
      case 'launch': return { icon: 'apps', text: `Launch ${programName(a.program)}` };
      case 'focus_or_launch': return { icon: 'switch', text: `Switch to ${programName(a.program)}` };
      case 'run': return { icon: 'terminal', text: a.command };
      case 'drag': return { icon: 'drag', text: 'Click-and-drag' };
      default: return { icon: t.icon, text: 'Nothing' };
    }
  };

  /* ---------------- Validation & cleaning ---------------- */

  /** Returns an error message for an incomplete draft, or null if it can be saved. */
  M.validate = function (a) {
    switch (a.type) {
      case 'keys': { const p = M.parseCombo(a.combo); return p.ok ? null : p.error; }
      case 'volume': case 'brightness':
        if (!Number.isInteger(a.step) || a.step === 0) return 'Pick a step other than 0';
        return null;
      case 'open_url': {
        if (!String(a.url || '').trim()) return 'Enter a link to open';
        try { new URL(M.fixUrl(a.url)); return null; } catch (_) { return 'That doesn’t look like a link (try https://…)'; }
      }
      case 'launch': case 'focus_or_launch': return String(a.program || '').trim() ? null : 'Enter a program name or path';
      case 'run': return String(a.command || '').trim() ? null : 'Enter a command to run';
      default: return null;
    }
  };

  /** Canonical shape for the backend: trims strings, drops empty optional fields. */
  M.clean = function (a) {
    const out = { type: a.type };
    switch (a.type) {
      case 'keys': out.combo = M.parseCombo(a.combo).combo || String(a.combo || '').trim(); break;
      case 'volume': case 'brightness': out.step = Math.round(Number(a.step) || 0); break;
      case 'media': out.key = a.key; break;
      case 'open_url': out.url = M.fixUrl(a.url); if (a.browser) out.browser = a.browser; break;
      case 'launch': case 'focus_or_launch':
        out.program = String(a.program || '').trim();
        if (a.args && a.args.length) out.args = a.args.slice();
        break;
      case 'run': out.command = String(a.command || '').trim(); break;
      default: break;
    }
    return out;
  };

  M.sameAction = (a, b) => JSON.stringify(M.clean(a || { type: 'none' })) === JSON.stringify(M.clean(b || { type: 'none' }));

  /** Prefix https:// when the user typed a bare domain. */
  M.fixUrl = function (u) {
    const s = String(u || '').trim();
    if (!s) return s;
    if (/^[a-z][a-z0-9+.-]*:/i.test(s)) return s;
    return 'https://' + s;
  };

  /* ---------------- Presets ---------------- */

  M.presetsFor = function (slot, platform, fingers) {
    const linux = platform === 'linux';
    if (slot.kind === 'drag') {
      return [
        { id: 'drag', label: fingers ? `${fingers}-finger drag` : 'Drag', icon: 'drag', action: { type: 'drag' } },
        { id: 'off', label: 'Off', icon: 'none', action: { type: 'none' } },
      ];
    }
    const k = (combo) => ({ type: 'keys', combo });
    const zoom = [
      { id: 'zoom-in', label: 'Zoom in', icon: 'zoom-in', action: k('ctrl+equal'), repeat: true },
      { id: 'zoom-out', label: 'Zoom out', icon: 'zoom-out', action: k('ctrl+minus'), repeat: true },
    ];
    const common = [
      { id: 'vol-up', label: 'Volume up', icon: 'volume', action: { type: 'volume', step: 4 }, repeat: true },
      { id: 'vol-down', label: 'Volume down', icon: 'volume-down', action: { type: 'volume', step: -4 }, repeat: true },
      { id: 'bri-up', label: 'Brightness up', icon: 'sun', action: { type: 'brightness', step: 10 }, repeat: true },
      { id: 'bri-down', label: 'Brightness down', icon: 'sun-dim', action: { type: 'brightness', step: -10 }, repeat: true },
      { id: 'play', label: 'Play / Pause', icon: 'play-pause', action: { type: 'media', key: 'play_pause' } },
      { id: 'next', label: 'Next track', icon: 'next', action: { type: 'media', key: 'next' } },
      { id: 'prev', label: 'Previous track', icon: 'prev', action: { type: 'media', key: 'previous' } },
      { id: 'mute', label: 'Mute', icon: 'mute', action: { type: 'mute' } },
      { id: 'desktop', label: 'Show desktop', icon: 'desktop', action: k(linux ? 'win+d' : 'win+d') },
      { id: 'taskview', label: linux ? 'Overview' : 'Task view', icon: 'taskview', action: k(linux ? 'win' : 'win+tab') },
      { id: 'desk-next', label: 'Next desktop', icon: 'desk-next', action: k(linux ? 'win+pagedown' : 'win+ctrl+right') },
      { id: 'desk-prev', label: 'Previous desktop', icon: 'desk-prev', action: k(linux ? 'win+pageup' : 'win+ctrl+left') },
      { id: 'shot', label: 'Screenshot', icon: 'screenshot', action: k(linux ? 'printscreen' : 'win+shift+s') },
      { id: 'close', label: 'Close window', icon: 'close-window', action: k('alt+f4') },
      { id: 'lock', label: 'Lock', icon: 'lock', action: { type: 'lock' } },
      { id: 'chrome', label: 'Open Chrome', icon: 'browser', action: { type: 'focus_or_launch', program: linux ? 'google-chrome' : 'chrome.exe' } },
      { id: 'youtube', label: 'Open YouTube', icon: 'youtube', action: { type: 'open_url', url: 'https://youtube.com', browser: 'chrome' } },
    ];
    // Zoom leads for pinches (the obvious use); elsewhere it sits after the media keys.
    return slot.kind === 'pinch' ? zoom.concat(common) : common.slice(0, 8).concat(zoom, common.slice(8));
  };

  /* ---------------- Config helpers ---------------- */

  M.bindingFor = (config, trigger) => (config.binding || []).find((b) => b.trigger === trigger) || null;

  M.dragOn = (config, n) => (config.binding || []).some((b) => b.trigger === `drag:${n}` && b.action.type === 'drag');

  /**
   * Up to `max` practice tasks for the tutorial, taken from the All-apps
   * bindings with the most beginner-friendly gestures first. Swipes that a
   * drag makes inert are skipped. If too few are bound, unbound basics that
   * are always recognised (tap, swipe, hold) fill in, so there's always
   * something to try. → [{ trigger, binding | null }]
   */
  M.practiceTasks = function (config, max) {
    const limit = max || 4;
    const groups = [
      ['tap:3'],
      ['swipe:4:up', 'swipe:4:down', 'swipe:4:left', 'swipe:4:right'],
      ['drag:3'],
      ['pinch:4:out', 'pinch:4:in'],
      ['doubletap:3'],
      ['hold:4'],
      ['tap:4'],
      ['swipe:3:up', 'swipe:3:down', 'swipe:3:left', 'swipe:3:right'],
      ['tap:5'],
    ];
    const usable = (t) => {
      const b = M.bindingFor(config, t);
      if (!b || b.action.type === 'none') return false;
      return !(t.startsWith('swipe:') && M.dragOn(config, M.parseTrigger(t).fingers));
    };
    const tasks = [];
    for (const group of groups) {
      if (tasks.length >= limit) break;
      const t = group.find(usable);
      if (t) tasks.push({ trigger: t, binding: M.bindingFor(config, t) });
    }
    for (const t of ['tap:3', 'swipe:4:up', 'hold:4']) {
      if (tasks.length >= 3) break;
      if (!tasks.some((x) => x.trigger === t)) tasks.push({ trigger: t, binding: null });
    }
    return tasks;
  };

  /* ---------------- Per-app profiles ---------------- */

  /** The profile list; the `profile` key may be absent. */
  M.profiles = (config) => (config && Array.isArray(config.profile) ? config.profile : []);

  /** A profile's own binding for a trigger (may be `{type: 'none'}` = off in that app). */
  M.overrideFor = (profile, trigger) => ((profile && profile.binding) || []).find((b) => b.trigger === trigger) || null;

  /** Same normalisation as pady-core: `C:\Apps\Code.EXE` / `code.exe` / `Code` → `code`. */
  M.appKey = function (app) {
    const base = String(app || '').trim().replace(/^"+|"+$/g, '').split(/[\\/]/).pop().toLowerCase();
    return base.endsWith('.exe') ? base.slice(0, -4) : base;
  };

  /** Clean a typed app name: trim and drop surrounding quotes, keep what the user wrote otherwise. */
  M.cleanApp = (app) => String(app || '').trim().replace(/^"+|"+$/g, '').trim();

  /** Profiles (other than `skipIndex`) that already list an app, matched like the backend does. */
  M.profilesWithApp = function (config, app, skipIndex) {
    const key = M.appKey(app);
    return M.profiles(config)
      .map((p, index) => ({ p, index }))
      .filter(({ p, index }) => index !== skipIndex && key && (p.apps || []).some((a) => M.appKey(a) === key));
  };

  /** Names of profiles that override a trigger (including "off"). */
  M.overridingProfiles = (config, trigger) => M.profiles(config).filter((p) => M.overrideFor(p, trigger)).map((p) => p.name);

  /**
   * A copy of config with one profile's binding for `trigger` changed:
   *   mode 'inherit' removes the override, 'off' stores `{type: 'none'}`,
   *   'custom' stores `action` (with `repeat` when true).
   */
  M.withProfileBinding = function (config, index, trigger, mode, action, repeat) {
    const next = JSON.parse(JSON.stringify(config));
    const p = M.profiles(next)[index];
    if (!p) return next;
    p.binding = (p.binding || []).filter((b) => b.trigger !== trigger);
    if (mode === 'off') p.binding.push({ trigger, action: { type: 'none' } });
    else if (mode === 'custom' && action && action.type !== 'none') {
      const b = { trigger, action: M.clean(action) };
      if (repeat) b.repeat = true;
      p.binding.push(b);
    }
    return next;
  };

  /** A copy of config with the trigger's binding replaced (or removed for `none`). */
  M.withBinding = function (config, trigger, action, repeat) {
    const next = JSON.parse(JSON.stringify(config));
    next.binding = (next.binding || []).filter((b) => b.trigger !== trigger);
    if (action && action.type !== 'none') {
      const b = { trigger, action: M.clean(action) };
      if (repeat) b.repeat = true;
      next.binding.push(b);
    }
    return next;
  };
})();
