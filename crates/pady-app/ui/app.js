/*
 * Pady — front end.
 *
 * Sections:
 *   1. Backend adapter (Tauri or the in-browser mock)
 *   2. DOM helpers
 *   3. Store
 *   4. Toasts, modal, save indicator
 *   5. Shell (navigation, master switch, device pill)
 *   6. Gesture illustrations
 *   7. Gestures page
 *   8. Binding editor (drawer) + shortcut recorder
 *   9. Live page
 *  10. Settings page
 *  11. About page
 *  12. Events & boot
 *
 * Rendering rule: the backend is the source of truth. Every command that
 * changes state returns the canonical AppState, which goes through
 * applyState() and re-renders from scratch where cheap.
 */
(function () {
  'use strict';

  const P = window.Pady;
  const M = P.model;
  const icon = P.icon;

  /* =====================================================================
     1. Backend adapter
     ===================================================================== */

  const api = (function () {
    const T = window.__TAURI__;
    if (T && T.core && T.event) {
      return { invoke: (cmd, args) => T.core.invoke(cmd, args || {}), listen: (name, cb) => T.event.listen(name, cb), mock: false };
    }
    if (window.PadyMock) return { invoke: window.PadyMock.invoke, listen: window.PadyMock.listen, mock: true };
    throw new Error('Pady: no backend available');
  })();

  const errText = (e) => (typeof e === 'string' ? e : (e && (e.message || e.error)) || 'Something went wrong');

  /** invoke() that surfaces failures as an error toast (and still rejects). */
  async function call(cmd, args, opts) {
    try {
      return await api.invoke(cmd, args);
    } catch (e) {
      if (!(opts && opts.quiet)) toast({ kind: 'error', title: (opts && opts.errorTitle) || 'Couldn’t complete that', text: errText(e) });
      throw e;
    }
  }

  /* =====================================================================
     2. DOM helpers
     ===================================================================== */

  const $ = (sel, root) => (root || document).querySelector(sel);
  const $$ = (sel, root) => Array.from((root || document).querySelectorAll(sel));

  /** Tiny element builder: h('div', {class, onClick, ...attrs}, ...children) */
  function h(tag, attrs, ...children) {
    const el = document.createElement(tag);
    if (attrs) {
      for (const [k, v] of Object.entries(attrs)) {
        if (v == null || v === false) continue;
        if (k === 'class') el.className = v;
        else if (k === 'html') el.innerHTML = v;
        else if (k === 'text') el.textContent = v;
        else if (k === 'style') for (const [sk, sv] of Object.entries(v)) el.style.setProperty(sk, sv);
        else if (k.startsWith('on') && typeof v === 'function') el.addEventListener(k.slice(2).toLowerCase(), v);
        else if (k in el && typeof v !== 'string' && k !== 'list') el[k] = v;
        else el.setAttribute(k, v === true ? '' : v);
      }
    }
    append(el, children);
    return el;
  }
  function append(el, children) {
    for (const c of children) {
      if (c == null || c === false) continue;
      if (Array.isArray(c)) append(el, c);
      else el.append(c instanceof Node ? c : document.createTextNode(String(c)));
    }
    return el;
  }
  /** Element from an HTML string (used for inline SVG). */
  function frag(html) {
    const t = document.createElement('template');
    t.innerHTML = html.trim();
    return t.content.firstElementChild;
  }
  const ico = (name, size) => frag(icon(name, size));
  const clone = (v) => JSON.parse(JSON.stringify(v));
  const reducedMotion = () => window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  function keysEl(caps, large) {
    const wrap = h('span', { class: 'keys' });
    caps.forEach((c, i) => {
      if (i) wrap.append(h('span', { class: 'keys-plus', 'aria-hidden': 'true' }, '+'));
      wrap.append(h('kbd', { class: large ? 'kbd kbd--lg' : 'kbd' }, c));
    });
    return wrap;
  }

  function chipEl(action) {
    const s = M.summarize(action, platform());
    const chip = h('span', { class: 'chip' }, ico(s.icon, 15));
    chip.append(s.keys && s.keys.length ? keysEl(s.keys) : h('span', { class: 'chip-text', title: s.text }, s.text));
    return chip;
  }

  /**
   * Arrow-key navigation for a group of radio-like buttons (roving tabindex).
   * `onPick(el)` is called when an item is chosen with the keyboard.
   */
  function roving(container, selector, onPick) {
    const items = () => $$(selector, container).filter((el) => !el.disabled);
    const sync = () => {
      const all = items();
      const current = all.find((el) => el.getAttribute('aria-checked') === 'true') || all[0];
      all.forEach((el) => (el.tabIndex = el === current ? 0 : -1));
    };
    container.addEventListener('keydown', (e) => {
      const keys = ['ArrowRight', 'ArrowDown', 'ArrowLeft', 'ArrowUp', 'Home', 'End'];
      if (!keys.includes(e.key)) return;
      const all = items();
      const i = all.indexOf(document.activeElement);
      if (i < 0) return;
      e.preventDefault();
      let j = i;
      if (e.key === 'Home') j = 0;
      else if (e.key === 'End') j = all.length - 1;
      else j = (i + (e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : -1) + all.length) % all.length;
      all[j].focus();
      onPick(all[j]);
      sync();
    });
    sync();
    return sync;
  }

  /* =====================================================================
     3. Store
     ===================================================================== */

  const store = {
    app: null,          // latest AppState from the backend
    page: 'gestures',
    fingers: 4,         // selected tab on the Gestures page
    recent: [],         // gesture feed (newest first)
    seq: 0,
  };
  const platform = () => (store.app ? store.app.platform : 'windows');
  const osName = () => (platform() === 'linux' ? 'Linux' : 'Windows');

  function applyState(state) {
    if (!state || !state.config) return;
    store.app = state;
    renderShell();
    if (store.page === 'gestures') renderGestures();
    else if (store.page === 'settings') renderSettings();
    else if (store.page === 'live') renderLive();
    else if (store.page === 'about') renderAbout();
  }

  /* =====================================================================
     4. Toasts, modal, save indicator
     ===================================================================== */

  function toast({ kind = 'success', title, text, timeout }) {
    const root = $('#toasts');
    const glyph = kind === 'error' ? 'warning' : kind === 'info' ? 'zap' : 'check';
    const el = h('div', { class: `toast toast--${kind}`, role: kind === 'error' ? 'alert' : 'status' },
      h('span', { class: 'toast-icon' }, ico(glyph, 13)),
      h('div', { class: 'toast-body' },
        title ? h('div', { class: 'toast-title' }, title) : null,
        text ? h('div', { class: 'toast-text' }, text) : null),
      h('button', { class: 'toast-close', type: 'button', 'aria-label': 'Dismiss', onClick: () => dismiss() }, ico('x', 14)));
    root.append(el);
    while (root.children.length > 4) root.firstElementChild.remove();
    let timer = setTimeout(dismiss, timeout || (kind === 'error' ? 7000 : 3200));
    el.addEventListener('mouseenter', () => clearTimeout(timer));
    el.addEventListener('mouseleave', () => { timer = setTimeout(dismiss, 1800); });
    function dismiss() {
      clearTimeout(timer);
      if (!el.isConnected) return;
      el.classList.add('is-leaving');
      setTimeout(() => el.remove(), reducedMotion() ? 0 : 240);
    }
  }

  /** Promise<boolean> confirmation dialog. */
  function confirmDialog({ title, text, confirm = 'Confirm', danger = false }) {
    return new Promise((resolve) => {
      const root = $('#overlay-root');
      const prev = document.activeElement;
      const backdrop = h('div', { class: 'backdrop modal-backdrop', onClick: () => done(false) });
      const ok = h('button', { class: danger ? 'btn btn--primary btn--danger-fill' : 'btn btn--primary', type: 'button', onClick: () => done(true) }, confirm);
      const cancel = h('button', { class: 'btn', type: 'button', onClick: () => done(false) }, 'Cancel');
      const modal = h('div', { class: 'modal', role: 'alertdialog', 'aria-modal': 'true', 'aria-labelledby': 'modal-title' },
        h('h3', { id: 'modal-title' }, title), h('p', null, text), h('div', { class: 'modal-actions' }, cancel, ok));
      modal.addEventListener('keydown', (e) => {
        if (e.key === 'Escape') { e.stopPropagation(); done(false); }
        if (e.key === 'Tab') { e.preventDefault(); (document.activeElement === ok ? cancel : ok).focus(); }
      });
      root.append(backdrop, modal);
      setInert(true);
      cancel.focus();
      function done(v) {
        backdrop.remove(); modal.remove();
        if (!$('.drawer', root)) setInert(false);
        if (prev && prev.focus) prev.focus();
        resolve(v);
      }
    });
  }

  function setInert(on) {
    const app = $('#app');
    app.inert = on;
    if (on) app.setAttribute('aria-hidden', 'true'); else app.removeAttribute('aria-hidden');
  }

  const saveFlag = (function () {
    let timer;
    return function (stateName) {
      const el = $('#save-flag');
      clearTimeout(timer);
      el.className = 'save-flag is-on' + (stateName === 'done' ? ' is-done' : '');
      el.replaceChildren();
      if (stateName === 'saving') el.append(h('span', { class: 'boot-spinner', style: { width: '11px', height: '11px', 'border-width': '1.5px' } }), 'Saving…');
      else if (stateName === 'done') { el.append(ico('check', 13), 'Saved'); timer = setTimeout(() => (el.className = 'save-flag'), 1600); }
      else el.className = 'save-flag';
    };
  })();

  /* =====================================================================
     5. Shell
     ===================================================================== */

  const PAGES = ['gestures', 'live', 'settings', 'about'];

  function initShell() {
    $$('.nav-item').forEach((btn) => {
      btn.prepend(ico(btn.dataset.icon, 18));
      // The text label is hidden in the narrow icon rail, so name the button explicitly.
      const name = $('.nav-label', btn).textContent;
      btn.setAttribute('aria-label', name);
      btn.title = name;
      btn.addEventListener('click', () => route(btn.dataset.page));
    });
    $('#master-switch').addEventListener('change', async (e) => {
      const want = e.target.checked;
      try {
        applyState(await call('set_enabled', { enabled: want }, { errorTitle: want ? 'Couldn’t turn Pady on' : 'Couldn’t pause Pady' }));
      } catch (_) {
        e.target.checked = !want;
      }
    });
    // Ctrl+1..4 jumps between pages.
    document.addEventListener('keydown', (e) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && /^[1-4]$/.test(e.key) && !$('#app').inert) {
        e.preventDefault();
        route(PAGES[Number(e.key) - 1]);
      }
    });
  }

  function route(page) {
    if (!PAGES.includes(page)) page = 'gestures';
    const prev = store.page;
    store.page = page;
    $$('.nav-item').forEach((b) => (b.dataset.page === page ? b.setAttribute('aria-current', 'page') : b.removeAttribute('aria-current')));
    PAGES.forEach((p) => { $('#page-' + p).hidden = p !== page; });
    const section = $('#page-' + page);
    $('#page-title').textContent = section.dataset.title;
    $('#page-sub').textContent = section.dataset.sub || '';
    try { history.replaceState(null, '', '#' + page); } catch (_) { /* file:// in some engines */ }
    $('#content').scrollTop = 0;
    if (prev === 'live' && page !== 'live') liveView.stop();
    if (!store.app) return;
    if (page === 'gestures') renderGestures(true);
    if (page === 'settings') renderSettings();
    if (page === 'about') renderAbout();
    if (page === 'live') { renderLive(); liveView.start(); }
  }

  function renderShell() {
    const s = store.app;
    const enabled = !!s.config.settings.enabled;
    $('#app').classList.toggle('is-paused', !enabled);
    $('#master-switch').checked = enabled;
    $('#master-state').textContent = enabled ? 'On' : 'Paused';
    $('#master-hint').textContent = enabled ? 'Gestures active' : 'No actions run';
    $('#master').title = enabled ? 'Pause Pady' : 'Turn Pady on';
    $('#settings-dot').hidden = !(s.conflicts && s.conflicts.length);

    const pill = $('#device-pill');
    const dev = s.devices && s.devices[0];
    const warn = !!s.input_error || !dev;
    pill.className = 'device-pill' + (warn ? ' is-warn' : '');
    pill.replaceChildren(
      h('span', { class: 'dot' }),
      h('span', { class: 'pill-text' },
        h('strong', null, warn ? (s.input_error ? 'Input problem' : 'No touchpad') : 'Touchpad ready'),
        h('span', { class: 'pill-sub' }, warn ? (s.input_error || 'Connect a precision touchpad') : dev.name)));
    pill.title = warn ? (s.input_error || 'No touchpad detected') : `Touchpad ready · ${dev.name}`;
  }

  /* =====================================================================
     6. Gesture illustrations (inline SVG, animated with CSS)
     ===================================================================== */

  function fingerPoints(n, cx, cy) {
    const row = Math.min(n, 4);
    const spread = row === 3 ? 14 : 12.5;
    const pts = [];
    for (let i = 0; i < row; i++) {
      const k = i - (row - 1) / 2;
      pts.push([cx + k * spread, cy + k * k * 1.7 - (Math.abs(k) < 1 ? 1 : 0)]);
    }
    if (n === 5) pts.unshift([cx - spread * 2.15, cy + 13]); // thumb
    return pts;
  }
  const fingerG = (pts, r) => pts.map(([x, y]) =>
    `<g class="illo-f"><circle class="illo-halo" cx="${x}" cy="${y}" r="${r * 2}"/><circle class="illo-finger" cx="${x}" cy="${y}" r="${r}"/></g>`).join('');

  function illoSVG(kind, n, dir) {
    const r = n === 5 ? 4.1 : 4.6;
    let inner = '';
    if (kind === 'swipe') {
      const dx = dir === 'left' ? -1 : dir === 'right' ? 1 : 0;
      const dy = dir === 'up' ? -1 : dir === 'down' ? 1 : 0;
      const pts = fingerPoints(n, 60 + dx * 3, 39 + dy * 4);
      const trail = pts.map(([x, y]) => `<line x1="${x}" y1="${y}" x2="${x - dx * 15}" y2="${y - dy * 13}"/>`).join('');
      inner = `<g class="illo-move"><g class="illo-trail">${trail}</g><g class="illo-fingers">${fingerG(pts, r)}</g></g>`;
    } else if (kind === 'tap') {
      const pts = fingerPoints(n, 60, 39);
      inner = pts.map(([x, y]) => `<circle class="illo-ripple" cx="${x}" cy="${y}" r="${r}"/>`).join('') + fingerG(pts, r);
    } else if (kind === 'hold') {
      inner = '<circle class="illo-ring-track" cx="60" cy="38" r="26"/><circle class="illo-ring" cx="60" cy="38" r="26" pathLength="100"/>' + fingerG(fingerPoints(n, 60, 39), r);
    } else {
      const pts = fingerPoints(n, 60, 37);
      inner = `<g class="illo-move"><rect class="illo-window" x="36" y="17" width="48" height="34" rx="5"/><rect class="illo-window-bar" x="36.5" y="17.5" width="47" height="8" rx="4.5"/>${fingerG(pts, r - 0.6)}</g>`;
    }
    return `<svg class="illo illo--${kind}${dir ? ' illo--' + dir : ''}" viewBox="0 0 120 76" aria-hidden="true" focusable="false"><rect class="illo-pad" x="2" y="2" width="116" height="72" rx="12"/>${inner}</svg>`;
  }

  /* =====================================================================
     7. Gestures page
     ===================================================================== */

  const gestUI = { built: false };

  function buildGestures() {
    const page = $('#page-gestures');
    gestUI.banners = h('div', { class: 'banners' });
    gestUI.seg = h('div', { class: 'seg', role: 'radiogroup', 'aria-label': 'Number of fingers' }, h('span', { class: 'seg-indicator', 'aria-hidden': 'true' }));
    M.FINGERS.forEach((n) => {
      const btn = h('button', { class: 'seg-btn', type: 'button', role: 'radio', 'data-n': n, onClick: () => pickFingers(n) },
        h('span', { class: 'seg-fingers', 'aria-hidden': 'true' }, Array.from({ length: n }, () => h('i'))),
        `${n} fingers`, h('span', { class: 'seg-count' }, '0'));
      gestUI.seg.append(btn);
    });
    gestUI.syncRoving = roving(gestUI.seg, '.seg-btn', (el) => pickFingers(Number(el.dataset.n)));
    gestUI.summary = h('span', { class: 'summary' });
    gestUI.groups = h('div');
    page.append(gestUI.banners, h('div', { class: 'gest-toolbar' }, gestUI.seg, gestUI.summary), gestUI.groups);
    new ResizeObserver(positionSegIndicator).observe(gestUI.seg);
    gestUI.built = true;
  }

  function pickFingers(n) {
    if (store.fingers === n) return;
    store.fingers = n;
    renderGestures(true);
  }

  function positionSegIndicator() {
    if (!gestUI.built) return;
    const active = $(`.seg-btn[data-n="${store.fingers}"]`, gestUI.seg);
    const ind = $('.seg-indicator', gestUI.seg);
    if (!active || !active.offsetWidth) return;
    ind.style.width = active.offsetWidth + 'px';
    ind.style.transform = `translateX(${active.offsetLeft}px)`;
    // Enable the slide transition only after the first placement.
    if (!ind.classList.contains('is-ready')) requestAnimationFrame(() => ind.classList.add('is-ready'));
  }

  function renderGestures(intro) {
    if (!gestUI.built) buildGestures();
    const s = store.app;
    const cfg = s.config;
    const n = store.fingers;

    // Banners: conflicts + paused
    gestUI.banners.replaceChildren();
    if (!cfg.settings.enabled) {
      gestUI.banners.append(h('div', { class: 'banner banner--info' }, ico('power', 18),
        h('div', { class: 'banner-body' }, h('span', { class: 'banner-title' }, 'Pady is paused'),
          h('span', { class: 'banner-text' }, 'Gestures are still recognised, but no actions run until you turn Pady back on.')),
        h('div', { class: 'banner-actions' }, h('button', { class: 'btn btn--sm btn--primary', type: 'button', onClick: () => { $('#master-switch').click(); } }, 'Turn on'))));
    }
    (s.conflicts || []).forEach((c) => {
      gestUI.banners.append(h('div', { class: 'banner' }, ico('warning', 18),
        h('div', { class: 'banner-body' }, h('span', { class: 'banner-title' }, c.message), h('span', { class: 'banner-text' }, c.fix)),
        h('div', { class: 'banner-actions' },
          c.can_open_settings ? h('button', { class: 'btn btn--sm', type: 'button', onClick: openTouchpadSettings }, ico('external', 14), 'Open settings') : null)));
    });

    // Segmented control
    $$('.seg-btn', gestUI.seg).forEach((btn) => {
      const bn = Number(btn.dataset.n);
      const count = cfg.binding.filter((b) => M.parseTrigger(b.trigger).fingers === bn).length;
      btn.setAttribute('aria-checked', String(bn === n));
      $('.seg-count', btn).textContent = count;
      btn.setAttribute('aria-label', `${bn} fingers, ${count} gesture${count === 1 ? '' : 's'} set`);
    });
    gestUI.syncRoving();
    requestAnimationFrame(positionSegIndicator);

    const bound = M.SLOTS.filter((sl) => M.bindingFor(cfg, M.triggerOf(sl, n))).length;
    gestUI.summary.textContent = `${bound} of ${M.SLOTS.length} gestures set for ${n} fingers`;

    // Cards
    const dragOn = M.dragOn(cfg, n);
    const groups = [
      { title: 'Tap, hold & drag', slots: ['tap', 'hold', 'drag'], cls: 'grid' },
      { title: 'Swipe', slots: ['swipe-up', 'swipe-down', 'swipe-left', 'swipe-right'], cls: 'grid grid--4', note: dragOn },
    ];
    let i = 0;
    gestUI.groups.replaceChildren(...groups.map((g) => {
      const grid = h('div', { class: g.cls });
      g.slots.forEach((id) => grid.append(cardEl(M.SLOTS.find((x) => x.id === id), n, i++, intro, dragOn)));
      const head = h('div', { class: 'group-head' }, h('h2', { class: 'group-title' }, g.title));
      if (g.note) {
        head.append(h('span', { class: 'group-note' }, ico('warning', 14), `${n}-finger drag is on, so ${n}-finger swipes are disabled.`),
          h('button', { class: 'btn btn--sm btn--ghost', type: 'button', onClick: () => turnOffDrag(n) }, 'Turn off drag'));
      }
      return h('section', { class: 'group' }, head, grid);
    }));
    if (intro && !reducedMotion()) {
      const cards = $$('.card', gestUI.groups);
      setTimeout(() => cards.forEach((c) => c.classList.remove('intro')), 2600 + cards.length * 110);
    }
  }

  function cardEl(slot, n, index, intro, dragOn) {
    const s = store.app;
    const trigger = M.triggerOf(slot, n);
    const b = M.bindingFor(s.config, trigger);
    const shadowed = (s.shadowed || []).includes(trigger);
    const inert = slot.kind === 'swipe' && dragOn;
    const label = M.triggerLabel(trigger);

    const badges = h('span', { class: 'card-badges' });
    if (b && b.repeat) badges.append(h('span', { class: 'badge badge--accent', title: 'Keeps firing while you slide' }, ico('repeat', 12), 'Repeats'));
    if (shadowed) badges.append(h('span', { class: 'badge badge--warn', title: `${n}-finger drag is on — swipes with ${n} fingers are disabled` }, ico('warning', 12), 'Inactive'));

    let body;
    if (b) body = chipEl(b.action);
    else if (inert) body = h('span', { class: 'chip chip--empty' }, ico('none', 15), h('span', { class: 'chip-text' }, 'Off while drag is on'));
    else body = h('span', { class: 'chip chip--empty' }, ico('plus', 15), h('span', { class: 'chip-text' }, 'Not set — click to assign'));

    const summary = b ? M.summarize(b.action, platform()).text : 'not set';
    const card = h('button', {
      type: 'button',
      class: 'card' + (b ? ' is-bound' : '') + (shadowed ? ' is-shadowed' : '') + (inert ? ' is-inert' : '') + (intro ? ' intro enter' : ''),
      'data-trigger': trigger,
      'aria-label': `${label}: ${summary}${shadowed ? ' (inactive while drag is on)' : ''}. Edit`,
      style: { '--i': String(index) },
      onClick: () => openEditor(trigger),
    },
    h('div', { class: 'card-illo' }, frag(illoSVG(slot.kind, n, slot.dir)), badges),
    h('div', { class: 'card-body' },
      h('div', { class: 'card-title' }, h('span', { class: 'card-name' }, slot.name), h('span', { class: 'card-edit' }, ico('chevron', 15))),
      body,
      shadowed ? h('div', { class: 'card-note' }, `${n}-finger drag is on — swipes with ${n} fingers are disabled.`) : null));
    return card;
  }

  async function turnOffDrag(n) {
    try {
      applyState(await call('save_config', { config: M.withBinding(store.app.config, `drag:${n}`, { type: 'none' }) }));
      toast({ title: `${n}-finger drag turned off`, text: `${n}-finger swipes work again.` });
    } catch (_) { /* toast shown */ }
  }

  /** Light up the card for a trigger that just fired (Gestures page). */
  function fireCard(trigger) {
    if (!trigger || store.page !== 'gestures') return;
    const p = M.parseTrigger(trigger);
    if (p.fingers !== store.fingers) {
      const btn = $(`.seg-btn[data-n="${p.fingers}"]`);
      if (btn) { btn.classList.remove('is-ping'); void btn.offsetWidth; btn.classList.add('is-ping'); }
      return;
    }
    const card = $(`.card[data-trigger="${trigger}"]`);
    if (!card) return;
    card.classList.remove('is-fired', 'intro');
    void card.offsetWidth; // restart animations
    card.classList.add('is-fired');
    clearTimeout(card._firedTimer);
    card._firedTimer = setTimeout(() => card.classList.remove('is-fired'), 2300);
  }

  async function openTouchpadSettings() {
    try { await call('open_touchpad_settings', null, { errorTitle: 'Couldn’t open touchpad settings' }); } catch (_) { /* toast shown */ }
  }

  /* =====================================================================
     8. Binding editor (drawer)
     ===================================================================== */

  let editor = null;
  let presetsExpanded = false; // remembered across editor openings
  const PRESETS_COLLAPSED = 8;

  function openEditor(trigger) {
    if (editor) return;
    const s = store.app;
    const slot = M.slotOfTrigger(trigger);
    const p = M.parseTrigger(trigger);
    const b = M.bindingFor(s.config, trigger);
    editor = {
      trigger, slot, n: p.fingers,
      original: b,
      draft: b ? clone(b.action) : { type: 'none' },
      repeat: b ? !!b.repeat : false,
      prevFocus: document.activeElement,
      busy: false,
    };
    buildDrawer();
  }

  function buildDrawer() {
    const ed = editor;
    const root = $('#overlay-root');
    const label = M.triggerLabel(ed.trigger);
    ed.backdrop = h('div', { class: 'backdrop', onClick: closeEditor });

    ed.body = h('div', { class: 'drawer-body' });
    ed.errorEl = h('div', { class: 'drawer-error' });
    ed.saveBtn = h('button', { class: 'btn btn--primary', type: 'button', onClick: saveEditor }, ico('check', 15), 'Save', h('span', { class: 'kbd-hint' }, '⏎'));
    ed.testBtn = h('button', { class: 'btn', type: 'button', onClick: testEditor, title: 'Run this action now' }, ico('zap', 15), 'Test');
    ed.removeBtn = h('button', { class: 'btn btn--danger', type: 'button', onClick: removeEditor }, ico('trash', 15), 'Remove');
    ed.removeBtn.hidden = !ed.original;

    ed.el = h('div', { class: 'drawer', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': 'drawer-title' },
      h('header', { class: 'drawer-head' },
        h('div', { class: 'illo-wrap illo-live' }, frag(illoSVG(ed.slot.kind, ed.n, ed.slot.dir))),
        h('div', { class: 'drawer-titles' },
          h('h2', { id: 'drawer-title' }, cap(label)),
          h('div', { class: 'sub' }, h('code', null, ed.trigger), h('span', null, '·'), h('span', null, ed.slot.hint))),
        h('button', { class: 'btn btn--ghost btn--icon', type: 'button', 'aria-label': 'Close editor', onClick: closeEditor }, ico('x', 18))),
      ed.body,
      ed.errorEl,
      h('footer', { class: 'drawer-foot' }, ed.removeBtn, h('span', { class: 'spacer' }), ed.testBtn, ed.saveBtn));

    ed.el.addEventListener('keydown', onDrawerKey);
    root.append(ed.backdrop, ed.el);
    setInert(true);
    renderEditorBody();
    // Focus the first preset (or the selected one) for fast keyboard use.
    requestAnimationFrame(() => {
      const target = $('.preset[aria-pressed="true"]', ed.el) || $('.preset', ed.el) || ed.saveBtn;
      target.focus();
    });
  }

  function onDrawerKey(e) {
    if (!editor) return;
    if (e.key === 'Escape') { e.preventDefault(); closeEditor(); return; }
    if (e.key === 'Enter' && !e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey) {
      const t = e.target;
      if (t.tagName === 'BUTTON' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.closest('.recorder')) return;
      e.preventDefault();
      saveEditor();
      return;
    }
    if (e.key === 'Tab') {
      // keep focus inside the drawer
      const focusables = $$('button, input, select, textarea, [tabindex]', editor.el)
        .filter((el) => !el.disabled && el.tabIndex >= 0 && el.offsetParent !== null);
      if (!focusables.length) return;
      const first = focusables[0], last = focusables[focusables.length - 1];
      if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
    }
  }

  function closeEditor() {
    const ed = editor;
    if (!ed) return;
    editor = null;
    if (ed.recorder) ed.recorder.destroy();
    ed.el.classList.add('is-closing');
    ed.backdrop.classList.add('is-closing');
    setTimeout(() => { ed.el.remove(); ed.backdrop.remove(); }, reducedMotion() ? 0 : 220);
    setInert(false);
    // Return focus to the card (re-rendered cards are found by trigger).
    const target = $(`.card[data-trigger="${ed.trigger}"]`) || ed.prevFocus;
    if (target && target.isConnected && target.focus) target.focus();
  }

  /** Rebuild every section of the drawer body (called when the action type changes). */
  function renderEditorBody() {
    const ed = editor;
    const isDrag = ed.slot.kind === 'drag';
    const isSwipe = ed.slot.kind === 'swipe';
    const scroll = ed.body.scrollTop;
    ed.body.replaceChildren();

    // Shadow warning
    if (isSwipe && M.dragOn(store.app.config, ed.n)) {
      ed.body.append(h('div', { class: 'drawer-section' }, h('div', { class: 'banner' }, ico('warning', 18),
        h('div', { class: 'banner-body' }, h('span', { class: 'banner-title' }, `${ed.n}-finger drag is on`),
          h('span', { class: 'banner-text' }, `Swipes with ${ed.n} fingers are disabled while drag uses them. You can still save this — it starts working once drag is turned off.`)))));
    }

    // Presets
    const presets = M.presetsFor(ed.slot, platform());
    const presetWrap = h('div', { class: 'presets' });
    // Collapse the long list unless the current action is one of the hidden presets.
    const selectedIndex = presets.findIndex((pr) => M.sameAction(pr.action, ed.draft));
    const collapse = !presetsExpanded && presets.length > PRESETS_COLLAPSED + 2 && selectedIndex < PRESETS_COLLAPSED;
    presets.forEach((pr, idx) => {
      if (collapse && idx >= PRESETS_COLLAPSED) return;
      presetWrap.append(h('button', {
        class: 'preset', type: 'button', 'data-preset': pr.id, 'aria-pressed': 'false',
        onClick: () => {
          ed.draft = clone(pr.action);
          if (isSwipe) ed.repeat = !!pr.repeat;
          renderEditorBody();
          const btn = $(`.preset[data-preset="${pr.id}"]`, ed.el);
          if (btn) btn.focus();
        },
      }, ico(pr.icon, 15), pr.label));
    });
    if (collapse) {
      presetWrap.append(h('button', {
        class: 'preset preset--more', type: 'button', 'aria-label': `Show ${presets.length - PRESETS_COLLAPSED} more presets`,
        onClick: () => {
          presetsExpanded = true;
          renderEditorBody();
          const next = $$('.preset', ed.el)[PRESETS_COLLAPSED];
          if (next) next.focus();
        },
      }, ico('plus', 14), `${presets.length - PRESETS_COLLAPSED} more`));
    }
    ed.body.append(h('section', { class: 'drawer-section' }, h('div', { class: 'section-label' }, isDrag ? 'Mode' : 'Quick presets'), presetWrap));

    if (isDrag) {
      ed.body.append(h('section', { class: 'drawer-section' }, h('div', { class: 'about-type' }, ico('drag', 18),
        h('div', null,
          h('div', null, h('strong', null, `${ed.n}-finger drag`), ` holds the left mouse button while your fingers move — drag windows, select text or move files without pressing.`),
          h('div', { class: 'help', style: { 'margin-top': '6px' } }, `While it's on, ${ed.n}-finger swipes are disabled. Speed, acceleration and release delay live in Settings.`)))));
    } else {
      // Action type picker
      const types = h('div', { class: 'types', role: 'radiogroup', 'aria-label': 'Action type' });
      M.GROUPS.filter((g) => g.id !== 'pointer').forEach((g) => {
        const opts = h('div', { class: 'type-opts' });
        Object.entries(M.TYPES).filter(([, t]) => t.group === g.id).forEach(([id, t]) => {
          opts.append(h('button', {
            class: 'type-opt', type: 'button', role: 'radio', 'data-type': id, 'aria-checked': String(ed.draft.type === id),
            onClick: () => { ed.autofocusField = true; pickType(id); },
          }, ico(t.icon, 15), t.label));
        });
        types.append(h('div', { class: 'type-row' }, h('span', { class: 'type-row-label' }, g.label), opts));
      });
      roving(types, '.type-opt', (el) => pickType(el.dataset.type, true));
      ed.body.append(h('section', { class: 'drawer-section' }, h('div', { class: 'section-label' }, 'Action'), types));

      // Type-specific fields
      const fields = fieldsFor(ed.draft);
      ed.autofocusField = false;
      if (fields) ed.body.append(h('section', { class: 'drawer-section fields' }, fields));

      // Repeat toggle (swipes only)
      if (isSwipe) {
        const input = h('input', { type: 'checkbox', role: 'switch', checked: ed.repeat, 'aria-describedby': 'repeat-sub', onChange: (e) => { ed.repeat = e.target.checked; refreshEditor(); } });
        ed.body.append(h('section', { class: 'drawer-section' },
          h('label', { class: 'toggle-row' },
            h('span', { class: 'toggle-row-text' },
              h('span', { class: 'toggle-row-title' }, ico('repeat', 15), 'Keep firing while sliding'),
              h('span', { class: 'toggle-row-sub', id: 'repeat-sub' }, `Fires again every ${fmtNum(store.app.config.settings.swipe_step_mm, 1)} mm of travel, so one slide can scrub volume or brightness.`)),
            h('span', { class: 'switch' }, input, h('span', { class: 'switch-track' }, h('span', { class: 'switch-thumb' }))))));
      }
    }
    ed.body.scrollTop = scroll;
    refreshEditor();
  }

  function pickType(type, keepFocus) {
    const ed = editor;
    if (ed.draft.type === type) return;
    // Carry over sensible values when switching between related types.
    const prev = ed.draft;
    const next = M.TYPES[type].make();
    if ((type === 'launch' || type === 'focus_or_launch') && prev.program) { next.program = prev.program; if (prev.args) next.args = prev.args; }
    ed.draft = next;
    if (ed.slot.kind === 'swipe') ed.repeat = type === 'volume' || type === 'brightness';
    renderEditorBody();
    const btn = $(`.type-opt[data-type="${type}"]`, ed.el);
    if (btn && keepFocus !== false) btn.focus();
  }

  /** Update presets / Save / Test / error without rebuilding inputs (keeps focus & caret). */
  function refreshEditor() {
    const ed = editor;
    if (!ed) return;
    $$('.preset[data-preset]', ed.el).forEach((btn) => {
      const pr = M.presetsFor(ed.slot, platform()).find((x) => x.id === btn.dataset.preset);
      btn.setAttribute('aria-pressed', String(!!pr && M.sameAction(pr.action, ed.draft)));
    });
    const err = M.validate(ed.draft);
    ed.invalid = err;
    const unchanged = ed.original
      ? M.sameAction(ed.original.action, ed.draft) && !!ed.original.repeat === !!(ed.slot.kind === 'swipe' && ed.repeat)
      : ed.draft.type === 'none';
    // Save stays clickable when invalid so a click can explain what's missing.
    ed.saveBtn.disabled = ed.busy || unchanged;
    ed.testBtn.disabled = ed.busy || !!err || ed.draft.type === 'none' || ed.draft.type === 'drag';
    ed.testBtn.hidden = ed.slot.kind === 'drag';
    // Only show validation once the user has typed something, to avoid shouting at an empty form.
    ed.errorEl.replaceChildren();
    if (err && ed.touched) ed.errorEl.append(h('div', { class: 'error-text' }, ico('warning', 14), err));
  }

  function fmtNum(v, d) { return Number(v).toFixed(d).replace(/\.0+$/, ''); }

  /* ---------- Per-type fields ---------- */

  function fieldsFor(a) {
    const ed = editor;
    const set = (patch) => { Object.assign(ed.draft, patch); ed.touched = true; refreshEditor(); };
    switch (a.type) {
      case 'volume':
      case 'brightness': {
        const isVol = a.type === 'volume';
        const max = Math.max(isVol ? 20 : 50, Math.abs(a.step));
        const readout = h('span', { class: 'step-readout', 'aria-live': 'polite' });
        const range = h('input', { class: 'range', type: 'range', min: 1, max, step: 1, value: Math.abs(a.step) || 1, 'aria-label': 'Step size in percent' });
        const dirSeg = h('div', { class: 'seg seg--sm', role: 'radiogroup', 'aria-label': 'Direction' }, h('span', { class: 'seg-indicator' }));
        const dirs = [{ v: 1, label: isVol ? 'Louder' : 'Brighter', icon: 'arrow-up' }, { v: -1, label: isVol ? 'Quieter' : 'Dimmer', icon: 'arrow-down' }];
        dirs.forEach((d) => dirSeg.append(h('button', {
          class: 'seg-btn', type: 'button', role: 'radio', 'data-v': d.v,
          onClick: () => setSign(d.v),
        }, ico(d.icon, 14), d.label)));
        const sync = () => {
          const step = ed.draft.step;
          readout.textContent = `${M.signed(step)}%`;
          range.value = Math.abs(step);
          range.style.setProperty('--fill', ((Math.abs(step) - 1) / (max - 1)) * 100 + '%');
          $$('.seg-btn', dirSeg).forEach((b) => b.setAttribute('aria-checked', String(Math.sign(step) === Number(b.dataset.v))));
          requestAnimationFrame(() => {
            const act = $('.seg-btn[aria-checked="true"]', dirSeg), ind = $('.seg-indicator', dirSeg);
            if (act && act.offsetWidth) {
              ind.style.width = act.offsetWidth + 'px';
              ind.style.transform = `translateX(${act.offsetLeft}px)`;
              requestAnimationFrame(() => ind.classList.add('is-ready'));
            }
          });
          syncRov();
        };
        const setSign = (v) => { set({ step: Math.abs(ed.draft.step || 1) * v }); sync(); };
        const syncRov = roving(dirSeg, '.seg-btn', (b) => setSign(Number(b.dataset.v)));
        range.addEventListener('input', () => { set({ step: Number(range.value) * (Math.sign(ed.draft.step) || 1) }); sync(); });
        const nudge = (d) => { const mag = Math.min(max, Math.max(1, Math.abs(ed.draft.step) + d)); set({ step: mag * (Math.sign(ed.draft.step) || 1) }); sync(); };
        sync();
        return h('div', null,
          h('div', { class: 'field' }, h('span', { class: 'label' }, 'Direction'), dirSeg),
          h('div', { class: 'field' }, h('span', { class: 'label' }, isVol ? 'Volume change per step' : 'Brightness change per step'),
            h('div', { class: 'stepper' }, range,
              h('div', { class: 'step-btns' },
                h('button', { class: 'btn btn--icon btn--sm', type: 'button', 'aria-label': 'Smaller step', onClick: () => nudge(-1) }, '−'),
                h('button', { class: 'btn btn--icon btn--sm', type: 'button', 'aria-label': 'Larger step', onClick: () => nudge(1) }, '+')),
              readout),
            h('span', { class: 'help' }, isVol ? 'Changes the master volume of the default output device.' : 'Changes the built-in display’s brightness.')));
      }

      case 'media': {
        const tiles = h('div', { class: 'tiles', role: 'radiogroup', 'aria-label': 'Media key' });
        M.MEDIA_KEYS.forEach((k) => tiles.append(h('button', {
          class: 'tile', type: 'button', role: 'radio', 'data-key': k.key, 'aria-checked': String(a.key === k.key), title: k.long,
          onClick: () => pick(k.key),
        }, ico(k.icon, 18), k.label)));
        const pick = (key) => {
          set({ key });
          $$('.tile', tiles).forEach((t) => t.setAttribute('aria-checked', String(t.dataset.key === key)));
          syncRov();
        };
        const syncRov = roving(tiles, '.tile', (t) => pick(t.dataset.key));
        return h('div', { class: 'field' }, h('span', { class: 'label' }, 'Key'), tiles,
          h('span', { class: 'help' }, 'Sent as a real media key, so it controls whatever is playing. Volume keys also show the system overlay.'));
      }

      case 'keys': {
        const text = h('input', { class: 'input input--mono', type: 'text', value: a.combo || '', placeholder: 'e.g. ctrl+shift+t', spellcheck: 'false', autocomplete: 'off', 'aria-label': 'Shortcut as text' });
        const status = h('span', { class: 'help' });
        const rec = createRecorder({
          value: () => ed.draft.combo,
          onChange: (combo) => { text.value = combo; set({ combo }); syncText(); },
        });
        ed.recorder = rec;
        const syncText = () => {
          const p = M.parseCombo(text.value);
          text.classList.toggle('is-invalid', !!text.value.trim() && !p.ok);
          status.textContent = text.value.trim() && !p.ok ? p.error : `Modifiers: ctrl, alt, shift, ${platform() === 'linux' ? 'super' : 'win'} · one key max · e.g. ${platform() === 'linux' ? 'super+left' : 'win+shift+s'}`;
          status.className = text.value.trim() && !p.ok ? 'error-text' : 'help';
        };
        text.addEventListener('input', () => { set({ combo: text.value }); syncText(); rec.render(); });
        text.addEventListener('blur', () => {
          const p = M.parseCombo(text.value);
          if (p.ok && p.combo !== text.value) { text.value = p.combo; set({ combo: p.combo }); rec.render(); }
        });
        syncText();
        return h('div', null,
          h('div', { class: 'field' }, h('span', { class: 'label' }, 'Shortcut'), rec.el),
          h('div', { class: 'field' }, h('span', { class: 'label' }, 'Or type it'), text, status));
      }

      case 'open_url': {
        const url = h('input', { class: 'input', type: 'url', value: a.url || '', placeholder: 'https://youtube.com', spellcheck: 'false', autocomplete: 'off' });
        url.addEventListener('input', () => set({ url: url.value }));
        url.addEventListener('blur', () => { const f = M.fixUrl(url.value); if (f !== url.value) { url.value = f; set({ url: f }); } });
        const sel = h('select', { class: 'select', 'aria-label': 'Browser' }, M.BROWSERS.map((b) => h('option', { value: b.id, selected: (a.browser || '') === b.id }, b.label)));
        sel.addEventListener('change', () => { set({ browser: sel.value || undefined }); if (!sel.value) delete ed.draft.browser; refreshEditor(); });
        if (ed.autofocusField && !a.url) setTimeout(() => url.focus(), 30);
        return h('div', null,
          h('div', { class: 'field' }, h('label', { class: 'label' }, 'Link'), url, h('span', { class: 'help' }, 'Any URL or app link (spotify:, ms-settings:, …).')),
          h('div', { class: 'field' }, h('span', { class: 'label' }, 'Open in'), sel));
      }

      case 'launch':
      case 'focus_or_launch': {
        const linux = platform() === 'linux';
        const prog = h('input', { class: 'input input--mono', type: 'text', value: a.program || '', placeholder: linux ? 'firefox' : 'chrome.exe', spellcheck: 'false', autocomplete: 'off' });
        const args = h('input', { class: 'input input--mono', type: 'text', value: M.formatArgs(a.args), placeholder: linux ? '--new-window' : '--new-window "https://example.com"', spellcheck: 'false', autocomplete: 'off' });
        prog.addEventListener('input', () => set({ program: prog.value }));
        args.addEventListener('input', () => { const list = M.parseArgs(args.value); if (list.length) set({ args: list }); else { delete ed.draft.args; set({}); } });
        if (ed.autofocusField && !a.program) setTimeout(() => prog.focus(), 30);
        const help = a.type === 'focus_or_launch'
          ? 'Brings the app to the front. If it’s already in front, minimises it. If it isn’t running, launches it.'
          : (linux ? 'A program on your PATH or a full path.' : 'A program on your PATH, a full path, or a URI such as ms-settings:display.');
        return h('div', null,
          h('div', { class: 'about-type', style: { 'margin-bottom': '12px' } }, ico(a.type === 'focus_or_launch' ? 'switch' : 'apps', 18), h('div', null, help)),
          h('div', { class: 'field' }, h('span', { class: 'label' }, 'Program'), prog),
          h('div', { class: 'field' }, h('span', { class: 'label' }, 'Arguments (optional)'), args, h('span', { class: 'help' }, 'Separate with spaces; use quotes for values with spaces.')));
      }

      case 'run': {
        const cmd = h('input', { class: 'input input--mono', type: 'text', value: a.command || '', placeholder: platform() === 'linux' ? 'notify-send "Hello from Pady"' : 'start ms-settings:bluetooth', spellcheck: 'false', autocomplete: 'off' });
        cmd.addEventListener('input', () => set({ command: cmd.value }));
        if (ed.autofocusField && !a.command) setTimeout(() => cmd.focus(), 30);
        return h('div', { class: 'field' }, h('span', { class: 'label' }, 'Command'), cmd,
          h('span', { class: 'help' }, platform() === 'linux' ? 'Runs with sh -c, without a terminal window.' : 'Runs with cmd /C, without a console window.'));
      }

      case 'mute': return aboutType('mute', 'Toggles mute on the default output device.');
      case 'lock': return aboutType('lock', 'Locks your session immediately, like pressing ' + (platform() === 'linux' ? 'Super+L.' : 'Win+L.'));
      case 'none': return aboutType('none', 'This gesture does nothing. Pick a preset or an action type above.');
      default: return null;
    }
  }

  const aboutType = (name, text) => h('div', { class: 'about-type' }, ico(name, 18), h('div', null, text));

  /* ---------- Shortcut recorder ----------
   * Focus or click the box, press a combination. Modifiers held are shown live;
   * the first non-modifier key completes the shortcut. Releasing modifiers
   * without a key records a modifier-only combo (e.g. "win"). Plain Esc cancels.
   */
  function createRecorder({ value, onChange }) {
    let recording = false;
    let peak = new Set();   // modifiers seen during this recording
    let held = new Set();   // modifiers currently held
    let note = '';

    const el = h('div', { class: 'recorder', tabindex: '0', role: 'button', 'aria-label': 'Record shortcut. Press Enter, then press the key combination.' });

    const modsOf = (e) => {
      const m = new Set();
      if (e.ctrlKey) m.add('ctrl');
      if (e.altKey) m.add('alt');
      if (e.shiftKey) m.add('shift');
      if (e.metaKey) m.add('win');
      // The key being pressed may not yet be reflected in the flags on some engines.
      if (/^Control/.test(e.code) && e.type === 'keydown') m.add('ctrl');
      if (/^Alt/.test(e.code) && e.type === 'keydown') m.add('alt');
      if (/^Shift/.test(e.code) && e.type === 'keydown') m.add('shift');
      if (/^(Meta|OS)/.test(e.code) && e.type === 'keydown') m.add('win');
      return m;
    };
    const ordered = (set) => M.MOD_ORDER.filter((m) => set.has(m));

    function render() {
      el.classList.toggle('is-recording', recording);
      el.setAttribute('aria-pressed', String(recording));
      el.replaceChildren();
      if (recording) {
        el.append(h('span', { class: 'recorder-status' }, h('span', { class: 'rec-dot' }), 'Recording'));
        if (held.size) el.append(keysEl(ordered(held).map((m) => M.modLabel(m, platform())), true));
        else el.append(h('span', { class: 'recorder-placeholder' }, note || 'Press a key combination…', h('span', { class: 'faint' }, 'Esc to cancel')));
        return;
      }
      const p = M.parseCombo(value());
      if (p.ok) {
        el.append(keysEl(M.keycaps(p.combo, platform()), true));
        el.append(h('button', {
          class: 'btn btn--ghost btn--icon btn--sm recorder-clear', type: 'button', 'aria-label': 'Clear shortcut',
          onClick: (e) => { e.stopPropagation(); onChange(''); render(); el.focus(); },
        }, ico('x', 14)));
      } else {
        el.append(h('span', { class: 'recorder-placeholder' }, ico('keyboard', 22), 'Click here, then press the shortcut'));
      }
    }

    function start() { recording = true; peak = new Set(); held = new Set(); note = ''; render(); }
    function stop() { recording = false; held.clear(); render(); }

    el.addEventListener('click', () => { if (!recording) { start(); el.focus(); } });
    el.addEventListener('blur', () => { if (recording) stop(); });
    el.addEventListener('keydown', (e) => {
      if (!recording) {
        if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); e.stopPropagation(); start(); }
        else if ((e.key === 'Backspace' || e.key === 'Delete') && value()) { e.preventDefault(); onChange(''); render(); }
        return;
      }
      e.preventDefault();
      e.stopPropagation();
      if (e.repeat) return;
      const mods = modsOf(e);
      if (e.code === 'Escape' && !mods.size) { stop(); return; }
      if (M.isModifierCode(e.code) || e.key === 'Meta' || e.key === 'OS') {
        mods.forEach((m) => peak.add(m));
        held = mods;
        render();
        return;
      }
      const key = M.keyFromCode(e.code);
      if (!key) { note = `“${e.key}” can’t be used — try another key`; held = mods; render(); return; }
      onChange(ordered(mods).concat([key]).join('+'));
      stop();
    });
    el.addEventListener('keyup', (e) => {
      if (!recording) return;
      e.preventDefault();
      e.stopPropagation();
      held = modsOf(e);
      if (!held.size && peak.size) {
        // All modifiers released without a main key: modifier-only combo.
        onChange(ordered(peak).join('+'));
        stop();
        return;
      }
      render();
    });
    render();
    return { el, render, destroy: () => { recording = false; } };
  }

  /* ---------- Editor actions ---------- */

  async function saveEditor() {
    const ed = editor;
    if (!ed || ed.busy) return;
    const err = M.validate(ed.draft);
    if (err && ed.draft.type !== 'none') {
      ed.touched = true;
      refreshEditor();
      const field = $('.fields .input, .fields .recorder', ed.el);
      if (field) field.focus();
      return;
    }
    if (ed.saveBtn.disabled) { closeEditor(); return; } // nothing changed
    const isSwipe = ed.slot.kind === 'swipe';
    const repeat = isSwipe && ed.repeat && ed.draft.type !== 'none';
    const config = M.withBinding(store.app.config, ed.trigger, ed.draft, repeat);
    setBusy(true);
    try {
      const state = await call('save_config', { config }, { errorTitle: 'Couldn’t save' });
      const label = M.triggerLabel(ed.trigger);
      applyState(state);
      if (editor === ed) closeEditor();
      if (ed.draft.type === 'none') toast({ title: 'Binding removed', text: `${cap(label)} does nothing now.` });
      else toast({ title: 'Saved', text: `${cap(label)} → ${M.summarize(ed.draft, platform()).text}` });
    } catch (_) {
      if (editor === ed) setBusy(false);
    }
  }

  async function removeEditor() {
    const ed = editor;
    if (!ed || ed.busy) return;
    setBusy(true);
    try {
      const state = await call('save_config', { config: M.withBinding(store.app.config, ed.trigger, { type: 'none' }, false) }, { errorTitle: 'Couldn’t remove binding' });
      applyState(state);
      if (editor === ed) closeEditor();
      toast({ title: 'Binding removed', text: `${cap(M.triggerLabel(ed.trigger))} does nothing now.` });
    } catch (_) {
      if (editor === ed) setBusy(false);
    }
  }

  async function testEditor() {
    const ed = editor;
    if (!ed || M.validate(ed.draft)) return;
    const btn = ed.testBtn;
    btn.classList.add('is-busy');
    try {
      await call('test_action', { action: M.clean(ed.draft) }, { errorTitle: 'Test failed' });
      toast({ kind: 'info', title: 'Ran it', text: M.summarize(ed.draft, platform()).text, timeout: 2200 });
    } catch (_) { /* toast shown */ } finally {
      btn.classList.remove('is-busy');
    }
  }

  function setBusy(on) {
    if (!editor) return;
    editor.busy = on;
    editor.saveBtn.classList.toggle('is-busy', on);
    refreshEditor();
  }

  const cap = (s) => s.charAt(0).toUpperCase() + s.slice(1);

  /* =====================================================================
     9. Live page
     ===================================================================== */

  const liveUI = { built: false };

  // Mutable live data, fed by events and drawn by requestAnimationFrame.
  const live = {
    frame: null,            // latest frame payload
    trails: new Map(),      // contact id -> [{x, y, t}]
    frameTimes: [],         // arrival times for the frames/s readout
    flashTimer: 0,
    flashKey: '',
    flashCount: 0,
  };

  function buildLive() {
    const page = $('#page-live');
    liveUI.empty = h('div');
    liveUI.stats = {
      fingers: h('div', { class: 'stat-value' }, '0'),
      rate: h('div', { class: 'stat-value' }, '0', h('small', null, '/s')),
      last: h('div', { class: 'stat-value', style: { 'font-size': '15px', 'line-height': '30px' } }, '—'),
    };
    liveUI.canvas = h('canvas', { 'aria-hidden': 'true' });
    liveUI.flashGesture = h('div', { class: 'flash-gesture' });
    liveUI.flashAction = h('div', { class: 'flash-action' });
    liveUI.flash = h('div', { class: 'flash', 'aria-hidden': 'true' }, liveUI.flashGesture, liveUI.flashAction);
    liveUI.dims = h('div', { class: 'pad-dims' });
    liveUI.hint = h('div', { class: 'pad-hint' }, h('div', null, ico('gestures', 28), 'Place three or more fingers on your touchpad'));
    liveUI.pad = h('div', { class: 'pad', role: 'img', 'aria-label': 'Live touchpad view' }, liveUI.canvas, liveUI.hint, liveUI.flash, liveUI.dims);
    liveUI.list = h('ul', { class: 'feed-list', 'aria-label': 'Recent gestures', 'aria-live': 'polite' });
    liveUI.feedEmpty = h('div', { class: 'feed-empty' }, 'Gestures you perform will appear here.');
    liveUI.items = new Map();
    liveUI.main = h('div', { class: 'live-layout' },
      h('div', { class: 'live-main' },
        h('div', { class: 'live-stats' },
          h('div', { class: 'stat' }, h('div', { class: 'stat-label' }, 'Fingers'), liveUI.stats.fingers),
          h('div', { class: 'stat' }, h('div', { class: 'stat-label' }, 'Frames'), liveUI.stats.rate),
          h('div', { class: 'stat' }, h('div', { class: 'stat-label' }, 'Last gesture'), liveUI.stats.last)),
        h('div', { class: 'pad-stage' }, liveUI.pad)),
      h('aside', { class: 'feed' },
        h('div', { class: 'feed-head' }, h('span', { class: 'feed-title' }, 'Recent gestures'),
          h('button', { class: 'btn btn--ghost btn--sm', type: 'button', onClick: () => { store.recent = []; renderFeed(); } }, 'Clear')),
        liveUI.feedEmpty, liveUI.list));
    page.append(liveUI.empty, liveUI.main);
    new ResizeObserver(() => { liveUI.sizeDirty = true; }).observe(liveUI.pad);
    liveUI.built = true;
  }

  function renderLive() {
    if (!liveUI.built) buildLive();
    const s = store.app;
    const dev = s.devices && s.devices[0];
    const noDevice = !dev && !live.frame;
    liveUI.main.hidden = noDevice;
    liveUI.empty.replaceChildren();
    if (noDevice) {
      liveUI.empty.append(h('div', { class: 'empty' },
        h('div', { class: 'empty-icon' }, ico('touchpad', 28)),
        h('h3', null, 'No touchpad detected'),
        h('p', null, s.input_error || 'Pady couldn’t find a multi-touch touchpad. Make sure it’s enabled and uses a precision driver.'),
        platform() !== 'linux' ? h('button', { class: 'btn', type: 'button', onClick: openTouchpadSettings, style: { 'margin-top': '6px' } }, ico('external', 14), 'Open touchpad settings') : null));
      return;
    }
    const w = (live.frame && live.frame.width_mm) || (dev && dev.width_mm) || 105;
    const hh = (live.frame && live.frame.height_mm) || (dev && dev.height_mm) || 68;
    sizePad(w, hh);
    liveUI.dims.textContent = `${dev ? dev.name + ' · ' : ''}${fmtNum(w, 0)} × ${fmtNum(hh, 0)} mm${dev && dev.max_contacts ? ` · ${dev.max_contacts} contacts` : ''}`;
    renderFeed();
  }

  function sizePad(w, hh) {
    const aspect = w / hh;
    liveUI.pad.style.aspectRatio = `${w} / ${hh}`;
    // Fit by height too, so the pad never pushes the page into scrolling on short windows.
    liveUI.pad.style.width = `min(100%, calc((100vh - 290px) * ${aspect.toFixed(3)}))`;
    liveUI.pad.style.minWidth = '260px';
    liveUI.mm = { w, h: hh };
  }

  /* Frame/gesture ingestion (cheap; drawing happens in the rAF loop). */
  function onFrame(f) {
    const now = performance.now();
    live.frame = f;
    live.frameTimes.push(now);
    for (const c of f.contacts) {
      let t = live.trails.get(c.id);
      if (!t) live.trails.set(c.id, (t = []));
      t.push({ x: c.x, y: c.y, t: now });
      if (t.length > 40) t.shift();
    }
    if (liveUI.mm && (f.width_mm !== liveUI.mm.w || f.height_mm !== liveUI.mm.h) && store.page === 'live') renderLive();
  }

  function onGesture(p) {
    const g = p.gesture || {};
    const now = new Date();
    const top = store.recent[0];
    let entry;
    if (g.kind === 'swipe' && g.step > 0 && top && top.trigger === p.trigger && top.kind === 'swipe') {
      top.count++; top.action = p.action || top.action; top.time = now;
      entry = top;
    } else if (g.kind === 'drag_end') {
      const d = store.recent.find((r) => r.kind === 'drag_start' && !r.ended);
      if (d) { d.ended = true; d.duration = now - d.start; }
      entry = d;
    } else {
      entry = { id: ++store.seq, kind: g.kind, gesture: g, trigger: p.trigger, action: p.action, time: now, start: now, count: 1 };
      store.recent.unshift(entry);
      if (store.recent.length > 25) store.recent.length = 25;
    }
    if (store.page === 'live' && liveUI.built) {
      renderFeed();
      if (g.kind !== 'drag_end') flashGesture(p, entry);
      liveUI.stats.last.textContent = cap(M.gestureLabel(g));
    }
    fireCard(p.trigger);
  }

  const gestureIcon = (g) => (g.kind === 'swipe' ? 'arrow-' + g.direction : g.kind === 'hold' ? 'hold' : g.kind && g.kind.startsWith('drag') ? 'drag' : 'tap');

  function flashGesture(p, entry) {
    const g = p.gesture;
    const key = `${p.trigger}`;
    const same = live.flashKey === key && liveUI.flash.classList.contains('is-on') && g.kind === 'swipe' && g.step > 0;
    live.flashKey = key;
    if (!same) {
      liveUI.flashGesture.replaceChildren(ico(gestureIcon(g), 20), cap(M.gestureLabel(g)));
    }
    liveUI.flashAction.className = 'flash-action' + (p.action ? ' is-bound' : '');
    liveUI.flashAction.replaceChildren(p.action ? '→ ' + p.action : 'No action bound');
    if (entry && entry.count > 1) {
      liveUI.flashAction.append(h('span', { class: 'flash-count' }, '×' + entry.count));
      liveUI.flash.classList.remove('is-bump'); void liveUI.flash.offsetWidth; liveUI.flash.classList.add('is-bump');
    }
    liveUI.flash.classList.add('is-on');
    clearTimeout(live.flashTimer);
    live.flashTimer = setTimeout(() => liveUI.flash.classList.remove('is-on'), 1500);
  }

  function renderFeed() {
    if (!liveUI.built) return;
    liveUI.feedEmpty.hidden = store.recent.length > 0;
    const keep = new Set(store.recent.map((r) => r.id));
    for (const [id, li] of liveUI.items) if (!keep.has(id)) { li.remove(); liveUI.items.delete(id); }
    let prev = null;
    for (const r of store.recent) {
      let li = liveUI.items.get(r.id);
      if (!li) {
        li = h('li', { class: 'feed-item' });
        liveUI.items.set(r.id, li);
        if (prev) prev.after(li); else liveUI.list.prepend(li);
      }
      const pad2 = (x) => String(x).padStart(2, '0');
      const time = `${pad2(r.time.getHours())}:${pad2(r.time.getMinutes())}:${pad2(r.time.getSeconds())}`;
      let actionText = r.action || 'No action bound';
      if (r.kind === 'drag_start' && r.ended) actionText += ` · ${(r.duration / 1000).toFixed(1)}s`;
      li.className = 'feed-item' + (r.action ? '' : ' is-unbound');
      li.replaceChildren(
        h('span', { class: 'feed-ico' }, ico(gestureIcon(r.gesture), 16)),
        h('span', { class: 'feed-main' },
          h('div', { class: 'feed-gesture' }, cap(M.gestureLabel(r.gesture))),
          h('div', { class: 'feed-action', title: actionText }, actionText)),
        h('span', { class: 'feed-meta' },
          h('span', { class: 'feed-time' }, time),
          r.count > 1 ? h('span', { class: 'badge badge--accent' }, '×' + r.count) : null));
      prev = li;
    }
  }

  /* Canvas renderer */
  const liveView = (function () {
    let raf = 0;
    let running = false;
    let colors = null;
    let lastCount = -1;
    let lastRateUpdate = 0;
    const TRAIL_MS = 420;

    function readColors() {
      const cs = getComputedStyle(document.documentElement);
      const v = (n) => cs.getPropertyValue(n).trim();
      colors = { finger: v('--finger'), glow: v('--finger-glow'), text: v('--text'), text3: v('--text-3'), border: v('--border-strong'), surface: v('--surface-solid') };
    }

    function start() {
      if (running) return;
      running = true;
      readColors();
      liveUI.sizeDirty = true;
      call('set_live', { live: true }, { quiet: true }).catch(() => {});
      raf = requestAnimationFrame(draw);
    }
    function stop() {
      if (!running) return;
      running = false;
      cancelAnimationFrame(raf);
      call('set_live', { live: false }, { quiet: true }).catch(() => {});
    }
    function pause() { if (running) { cancelAnimationFrame(raf); call('set_live', { live: false }, { quiet: true }).catch(() => {}); } }
    function resume() { if (running) { call('set_live', { live: true }, { quiet: true }).catch(() => {}); raf = requestAnimationFrame(draw); } }

    function draw() {
      if (!running) return;
      raf = requestAnimationFrame(draw);
      if (!liveUI.built || liveUI.main.hidden) return;
      const cv = liveUI.canvas;
      const dpr = window.devicePixelRatio || 1;
      if (liveUI.sizeDirty) {
        liveUI.sizeDirty = false;
        cv.width = Math.round(cv.clientWidth * dpr);
        cv.height = Math.round(cv.clientHeight * dpr);
        if (liveUI.mm) liveUI.pad.style.setProperty('--grid', (cv.clientWidth / liveUI.mm.w) * 5 + 'px');
      }
      const ctx = cv.getContext('2d');
      const W = cv.clientWidth, H = cv.clientHeight;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, W, H);
      if (!liveUI.mm || !W) return;
      const sx = W / liveUI.mm.w, sy = H / liveUI.mm.h;
      const s = Math.min(sx, sy);
      const now = performance.now();

      // Drop trail points that have faded out.
      for (const [id, pts] of live.trails) {
        while (pts.length && now - pts[0].t > TRAIL_MS) pts.shift();
        if (!pts.length) live.trails.delete(id);
      }

      // Trails
      ctx.lineCap = 'round';
      for (const pts of live.trails.values()) {
        for (let i = 1; i < pts.length; i++) {
          const a = pts[i - 1], b = pts[i];
          const age = 1 - (now - b.t) / TRAIL_MS;
          ctx.strokeStyle = colors.finger;
          ctx.globalAlpha = Math.max(0, age) * 0.55;
          ctx.lineWidth = Math.max(1, s * 3.2 * age);
          ctx.beginPath();
          ctx.moveTo(a.x * sx, a.y * sy);
          ctx.lineTo(b.x * sx, b.y * sy);
          ctx.stroke();
        }
      }
      ctx.globalAlpha = 1;

      const contacts = (live.frame && live.frame.contacts) || [];

      // Centroid web for multi-finger contact
      if (contacts.length >= 2) {
        const cx = contacts.reduce((t, c) => t + c.x, 0) / contacts.length * sx;
        const cy = contacts.reduce((t, c) => t + c.y, 0) / contacts.length * sy;
        ctx.strokeStyle = colors.finger;
        ctx.globalAlpha = 0.22;
        ctx.lineWidth = 1;
        ctx.setLineDash([3, 4]);
        for (const c of contacts) { ctx.beginPath(); ctx.moveTo(cx, cy); ctx.lineTo(c.x * sx, c.y * sy); ctx.stroke(); }
        ctx.setLineDash([]);
        ctx.globalAlpha = 0.6;
        ctx.beginPath(); ctx.arc(cx, cy, 3, 0, Math.PI * 2); ctx.fillStyle = colors.finger; ctx.fill();
        ctx.globalAlpha = 1;
      }

      // Fingers
      const r = Math.max(9, s * 4.6);
      for (const c of contacts) {
        const x = c.x * sx, y = c.y * sy;
        const glow = ctx.createRadialGradient(x, y, r * 0.4, x, y, r * 2.8);
        glow.addColorStop(0, colors.glow);
        glow.addColorStop(1, 'rgba(0,0,0,0)');
        ctx.fillStyle = glow;
        ctx.beginPath(); ctx.arc(x, y, r * 2.8, 0, Math.PI * 2); ctx.fill();

        ctx.fillStyle = colors.finger;
        ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.fill();
        const shine = ctx.createRadialGradient(x - r * 0.35, y - r * 0.4, 0, x, y, r);
        shine.addColorStop(0, 'rgba(255,255,255,0.65)');
        shine.addColorStop(0.6, 'rgba(255,255,255,0.08)');
        shine.addColorStop(1, 'rgba(255,255,255,0)');
        ctx.fillStyle = shine;
        ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.fill();

        ctx.fillStyle = colors.text3;
        ctx.font = '600 10px ' + getComputedStyle(document.body).fontFamily;
        ctx.textAlign = 'left';
        ctx.fillText('#' + c.id, x + r + 5, y - r * 0.6);
      }

      // Readouts (touch the DOM only when values change)
      if (contacts.length !== lastCount) {
        lastCount = contacts.length;
        liveUI.stats.fingers.textContent = String(contacts.length);
        liveUI.stats.fingers.classList.toggle('is-active', contacts.length >= 3);
        liveUI.pad.classList.toggle('has-touch', contacts.length > 0 || live.trails.size > 0 || store.recent.length > 0);
      }
      if (now - lastRateUpdate > 500) {
        lastRateUpdate = now;
        while (live.frameTimes.length && now - live.frameTimes[0] > 1000) live.frameTimes.shift();
        liveUI.stats.rate.firstChild.textContent = String(live.frameTimes.length);
      }
    }

    return { start, stop, pause, resume, readColors };
  })();

  /* =====================================================================
     10. Settings page
     ===================================================================== */

  const SLIDERS = [
    { group: 'recog', key: 'tap_max_ms', name: 'Tap speed', min: 100, max: 400, step: 10, fmt: (v) => [v, 'ms'], help: 'Fingers must lift within this time to count as a tap. Lower feels snappier and avoids accidental taps.' },
    { group: 'recog', key: 'tap_slop_mm', name: 'Tap tolerance', min: 1, max: 10, step: 0.5, fmt: (v) => [Number(v).toFixed(1), 'mm'], help: 'How far fingers may drift during a tap before it stops being one.' },
    { group: 'recog', key: 'swipe_threshold_mm', name: 'Swipe distance', min: 4, max: 30, step: 1, fmt: (v) => [Number(v).toFixed(0), 'mm'], help: 'Travel needed before a swipe — or a drag — is recognised.' },
    { group: 'recog', key: 'swipe_step_mm', name: 'Repeat spacing', min: 3, max: 20, step: 0.5, fmt: (v) => [Number(v).toFixed(1), 'mm'], help: 'Extra travel between repeats for gestures that keep firing, like a volume scrub.' },
    { group: 'recog', key: 'hold_ms', name: 'Hold time', min: 300, max: 1500, step: 50, fmt: (v) => [v, 'ms'], help: 'How long fingers rest without moving before it counts as a hold.' },
    { group: 'drag', key: 'drag_speed', name: 'Drag speed', min: 2, max: 25, step: 0.5, fmt: (v) => [Number(v).toFixed(1), 'px/mm'], help: 'Cursor pixels per millimetre of finger travel while dragging.' },
    { group: 'drag', key: 'drag_acceleration', name: 'Drag acceleration', min: 0, max: 3, step: 0.1, fmt: (v) => (Number(v) === 0 ? ['Linear', ''] : [Number(v).toFixed(1), '×']), help: 'How much faster quick flicks move the cursor. 0 keeps movement linear.' },
    { group: 'drag', key: 'drag_release_ms', name: 'Drag release delay', min: 0, max: 1000, step: 25, fmt: (v) => (Number(v) === 0 ? ['Instant', ''] : [v, 'ms']), help: 'Keeps the button held briefly after lifting, so you can reposition your fingers and continue the drag.' },
  ];
  const INT_KEYS = new Set(['tap_max_ms', 'hold_ms', 'drag_release_ms']);

  const setUI = { built: false, pending: {}, timer: 0, inputs: {} };

  function buildSettings() {
    const page = $('#page-settings');
    setUI.banners = h('div', { class: 'banners' });

    const panel = (iconName, title, sub, ...body) => h('section', { class: 'panel' },
      h('div', { class: 'panel-head' }, h('span', { class: 'panel-icon' }, ico(iconName, 16)),
        h('div', null, h('div', { class: 'panel-title' }, title), h('div', { class: 'panel-sub' }, sub))),
      h('div', { class: 'panel-body' }, body));

    const sliderEl = (d) => {
      const id = 'set-' + d.key;
      const val = h('span', { class: 'setting-value' });
      const input = h('input', { class: 'range', type: 'range', id, min: d.min, max: d.max, step: d.step, 'aria-describedby': id + '-help' });
      input.addEventListener('input', () => {
        const v = INT_KEYS.has(d.key) ? Math.round(Number(input.value)) : Number(input.value);
        setUI.pending[d.key] = v;
        paintSlider(d, v);
        clearTimeout(setUI.timer);
        setUI.timer = setTimeout(flushSettings, 300);
      });
      setUI.inputs[d.key] = { input, val, d };
      return h('div', { class: 'setting' },
        h('div', { class: 'setting-top' }, h('label', { class: 'setting-name', for: id }, d.name), val),
        input,
        h('div', { class: 'setting-help', id: id + '-help' }, d.help));
    };

    setUI.autostart = h('input', { type: 'checkbox', role: 'switch', 'aria-labelledby': 'autostart-title' });
    setUI.autostart.addEventListener('change', async (e) => {
      const want = e.target.checked;
      try {
        applyState(await call('set_autostart', { enabled: want }, { errorTitle: 'Couldn’t change start-up setting' }));
        toast({ title: want ? `Pady will start with ${osName()}` : `Pady won’t start with ${osName()}` });
      } catch (_) { e.target.checked = !want; }
    });
    setUI.autostartTitle = h('div', { class: 'row-title', id: 'autostart-title' });
    setUI.path = h('code', { class: 'path selectable' });
    setUI.devices = h('div');

    page.append(
      setUI.banners,
      h('div', { class: 'settings-grid' },
        panel('gestures', 'Recognition', 'Timing and distances for taps, holds and swipes', SLIDERS.filter((d) => d.group === 'recog').map(sliderEl)),
        h('div', { style: { display: 'flex', 'flex-direction': 'column', gap: '16px' } },
          panel('drag', 'Drag', 'How multi-finger drag moves the pointer', SLIDERS.filter((d) => d.group === 'drag').map(sliderEl)),
          panel('power', 'General', 'Start-up and configuration',
            h('label', { class: 'row', style: { cursor: 'pointer' } },
              h('div', { class: 'row-text' }, setUI.autostartTitle, h('div', { class: 'row-sub' }, 'Runs quietly in the tray after you sign in.')),
              h('span', { class: 'switch' }, setUI.autostart, h('span', { class: 'switch-track' }, h('span', { class: 'switch-thumb' })))),
            h('div', { class: 'row', style: { 'flex-wrap': 'wrap' } },
              h('div', { class: 'row-text' }, h('div', { class: 'row-title' }, 'Configuration file'), h('div', { class: 'row-sub' }, 'Edit by hand if you like — changes apply instantly.'), setUI.path)),
            h('div', { class: 'row', style: { 'border-top': '0', 'padding-top': '0', gap: '8px' } },
              h('button', { class: 'btn btn--sm', type: 'button', onClick: openConfigFolder }, ico('folder', 14), 'Open folder'),
              h('button', { class: 'btn btn--sm', type: 'button', onClick: copyPath }, ico('copy', 14), 'Copy path'),
              h('span', { style: { flex: '1' } }),
              h('button', { class: 'btn btn--sm btn--danger', type: 'button', onClick: resetConfig }, ico('reset', 14), 'Reset to defaults'))),
          panel('touchpad', 'Touchpad', 'Detected input devices', setUI.devices))));
    setUI.built = true;
  }

  function paintSlider(d, v) {
    const { input, val } = setUI.inputs[d.key];
    const [num, unit] = d.fmt(v);
    val.replaceChildren(String(num), unit ? h('small', null, unit) : '');
    input.style.setProperty('--fill', ((v - d.min) / (d.max - d.min)) * 100 + '%');
    input.setAttribute('aria-valuetext', `${num} ${unit}`.trim());
  }

  function renderSettings() {
    if (!setUI.built) buildSettings();
    const s = store.app;
    const cfg = s.config.settings;
    for (const d of SLIDERS) {
      if (d.key in setUI.pending) continue; // user is still adjusting this one
      const v = cfg[d.key];
      setUI.inputs[d.key].input.value = v;
      paintSlider(d, v);
    }
    setUI.autostart.checked = !!s.autostart;
    setUI.autostartTitle.textContent = `Start with ${osName()}`;
    setUI.path.textContent = s.config_path || '';

    setUI.banners.replaceChildren(...(s.conflicts || []).map((c) => h('div', { class: 'banner' }, ico('warning', 18),
      h('div', { class: 'banner-body' }, h('span', { class: 'banner-title' }, c.message), h('span', { class: 'banner-text' }, c.fix)),
      c.can_open_settings ? h('div', { class: 'banner-actions' }, h('button', { class: 'btn btn--sm', type: 'button', onClick: openTouchpadSettings }, ico('external', 14), 'Open touchpad settings')) : null)));

    setUI.devices.replaceChildren();
    if (s.devices && s.devices.length) {
      s.devices.forEach((d) => setUI.devices.append(h('div', { class: 'row' },
        h('span', { class: 'device-pill', style: { padding: '0', border: '0', background: 'none' } }, h('span', { class: 'dot' })),
        h('div', { class: 'row-text' }, h('div', { class: 'row-title' }, d.name),
          h('div', { class: 'row-sub' }, `${fmtNum(d.width_mm, 0)} × ${fmtNum(d.height_mm, 0)} mm · up to ${d.max_contacts} fingers`)))));
    } else {
      setUI.devices.append(h('div', { class: 'row' }, h('div', { class: 'row-text' },
        h('div', { class: 'row-title', style: { color: 'var(--amber-text)' } }, 'No touchpad detected'),
        h('div', { class: 'row-sub' }, s.input_error || 'Pady couldn’t find a multi-touch touchpad.'))));
    }
    if (platform() !== 'linux') {
      setUI.devices.append(h('div', { class: 'row' },
        h('div', { class: 'row-text' }, h('div', { class: 'row-title' }, `${osName()} touchpad gestures`), h('div', { class: 'row-sub' }, 'Set 3- and 4-finger gestures to “Nothing” so they don’t fight with Pady.')),
        h('button', { class: 'btn btn--sm', type: 'button', onClick: openTouchpadSettings }, ico('external', 14), 'Open')));
    }
  }

  async function flushSettings() {
    const patch = setUI.pending;
    if (!Object.keys(patch).length) return;
    const config = clone(store.app.config);
    Object.assign(config.settings, patch);
    saveFlag('saving');
    try {
      const state = await call('save_config', { config }, { errorTitle: 'Couldn’t save settings' });
      // Clear only the keys we sent whose value hasn't changed since.
      for (const k of Object.keys(patch)) if (setUI.pending[k] === patch[k]) delete setUI.pending[k];
      applyState(state);
      saveFlag('done');
    } catch (_) {
      for (const k of Object.keys(patch)) if (setUI.pending[k] === patch[k]) delete setUI.pending[k];
      saveFlag(null);
      renderSettings(); // snap sliders back to the saved values
    }
  }

  async function openConfigFolder() {
    try { await call('open_config_folder', null, { errorTitle: 'Couldn’t open the folder' }); } catch (_) { /* toast shown */ }
  }

  async function copyPath() {
    try {
      await navigator.clipboard.writeText(store.app.config_path);
      toast({ kind: 'info', title: 'Path copied', timeout: 1800 });
    } catch (_) {
      const r = document.createRange(); r.selectNodeContents(setUI.path);
      const sel = getSelection(); sel.removeAllRanges(); sel.addRange(r);
      toast({ kind: 'info', title: 'Path selected', text: 'Press Ctrl+C to copy it.' });
    }
  }

  async function resetConfig() {
    const ok = await confirmDialog({
      title: 'Reset Pady to defaults?',
      text: 'All your gesture bindings and tuning will be replaced with the defaults. This can’t be undone.',
      confirm: 'Reset everything',
      danger: true,
    });
    if (!ok) return;
    try {
      setUI.pending = {};
      applyState(await call('reset_config', null, { errorTitle: 'Couldn’t reset' }));
      toast({ title: 'Defaults restored' });
    } catch (_) { /* toast shown */ }
  }

  /* =====================================================================
     11. About page
     ===================================================================== */

  function renderAbout() {
    const page = $('#page-about');
    const s = store.app;
    const linux = platform() === 'linux';
    const logo = $('.brand-mark svg').cloneNode(true);
    logo.setAttribute('width', '72'); logo.setAttribute('height', '72');
    $$('[id]', logo).forEach((n) => n.removeAttribute('id'));
    // Re-point the cloned gradient to a unique id.
    const grad = logo.querySelector('linearGradient');
    if (grad) { grad.id = 'brand-g-about'; logo.querySelector('rect').setAttribute('fill', 'url(#brand-g-about)'); }

    const step = (iconName, title, text) => h('div', { class: 'flow-step' }, h('span', { class: 'panel-icon' }, ico(iconName, 16)), h('h4', null, title), h('p', null, text));
    const arrow = () => h('div', { class: 'flow-arrow', 'aria-hidden': 'true' }, ico('arrow-right', 18));
    const tip = (strong, rest) => h('li', null, ico('sparkle', 14), h('span', null, h('strong', null, strong), ' ', rest));

    page.replaceChildren(
      h('div', { class: 'about-hero' },
        h('span', { class: 'logo-lg' }, logo),
        h('div', null,
          h('h2', null, 'Pady', h('span', { class: 'badge badge--accent' }, 'v' + s.version)),
          h('p', null, 'Pady turns multi-finger touchpad gestures into actions: scrub the volume with a four-finger slide, play or pause with a three-finger tap, drag windows without pressing, or jump to an app or website with a single touch. It runs quietly in the tray and reads your touchpad directly, so it works in every app.'),
          h('div', { class: 'about-links' },
            h('span', { class: 'link-text selectable' }, ico('github', 14), 'github.com/lumidren/Pady'),
            h('span', { class: 'link-text' }, ico(linux ? 'terminal' : 'desktop', 14), linux ? 'Linux' : 'Windows')))),
      h('section', { class: 'panel', style: { 'margin-bottom': '16px' } },
        h('div', { class: 'panel-head' }, h('div', null, h('div', { class: 'panel-title' }, 'How it works'), h('div', { class: 'panel-sub' }, 'From fingertips to actions in a few milliseconds'))),
        h('div', { class: 'flow' },
          step('touchpad', 'Raw touch input', linux ? 'Contacts are read from the touchpad via evdev, in millimetres.' : 'Contacts are read from the Precision Touchpad via Raw Input, in millimetres.'),
          arrow(),
          step('gestures', 'Gesture engine', 'Finger count, travel and timing become taps, holds, swipes or drags — tuned in Settings.'),
          arrow(),
          step('zap', 'Your actions', 'The bound action runs: keys, volume, media, apps, links or commands.'))),
      h('section', { class: 'panel' },
        h('div', { class: 'panel-head' }, h('div', null, h('div', { class: 'panel-title' }, 'Tips'))),
        h('ul', { class: 'tips' },
          linux
            ? tip('Avoid double actions.', 'Desktop environments like GNOME use 3- and 4-finger swipes; disable them or bind other finger counts in Pady.')
            : tip('Avoid double actions.', 'In Settings → Bluetooth & devices → Touchpad, set three- and four-finger gestures to “Nothing”.'),
          tip('Scrub, don’t repeat.', 'Turn on “Keep firing while sliding” for volume or brightness, then slide further for bigger changes.'),
          tip('Drag claims swipes.', 'When 3-finger drag is on, 3-finger swipes are ignored — use 4 fingers for swipes.'),
          tip('Watch it think.', 'The Live page shows every contact and gesture, which makes tuning Settings much easier.'),
          tip('Plain text config.', 'Everything is stored in config.toml; edits made by hand apply instantly.'))));
  }

  /* =====================================================================
     12. Events & boot
     ===================================================================== */

  function listenAll() {
    const on = (name, cb) => Promise.resolve(api.listen(name, cb)).catch((err) => console.error(`listen ${name} failed:`, err));
    on('pady://state', (e) => applyState(e.payload));
    on('pady://frame', (e) => onFrame(e.payload));
    on('pady://gesture', (e) => onGesture(e.payload));
    on('pady://action-failed', (e) => {
      const p = e.payload || {};
      toast({ kind: 'error', title: p.action ? `“${p.action}” failed` : 'Action failed', text: p.error });
    });
  }

  document.addEventListener('visibilitychange', () => {
    if (store.page !== 'live') return;
    if (document.hidden) liveView.pause(); else liveView.resume();
  });

  window.addEventListener('hashchange', () => {
    const page = location.hash.replace('#', '');
    if (store.app && page !== store.page && PAGES.includes(page) && !editor) route(page);
  });

  // Theme switches change canvas colours.
  window.matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => liveView.readColors());

  async function boot() {
    initShell();
    listenAll();
    const initial = (location.hash || '').replace('#', '');
    try {
      const state = await call('get_state', null, { errorTitle: 'Couldn’t load Pady’s state' });
      store.app = state;
      renderShell();
      $('#boot').remove();
      route(PAGES.includes(initial) ? initial : 'gestures');
    } catch (e) {
      $('#boot').replaceChildren(h('div', { class: 'empty' },
        h('div', { class: 'empty-icon' }, ico('warning', 28)),
        h('h3', null, 'Pady’s engine isn’t responding'),
        h('p', null, errText(e)),
        h('button', { class: 'btn', type: 'button', onClick: () => location.reload() }, ico('reset', 14), 'Try again')));
    }
  }

  boot();
})();
