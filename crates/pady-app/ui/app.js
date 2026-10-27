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
 *  8b. Profile editor (drawer)
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
    profile: null,      // { index, name } of the profile shown on the Gestures page; null = All apps
    recent: [],         // gesture feed (newest first)
    seq: 0,
  };
  const platform = () => (store.app ? store.app.platform : 'windows');
  const osName = () => (platform() === 'linux' ? 'Linux' : 'Windows');
  const profileList = () => M.profiles(store.app && store.app.config);

  /**
   * Find a profile again after the config changed: same index and name if it
   * is still there, otherwise by name (profiles can be reordered or removed
   * from the tray or by editing the file). -1 when it's gone.
   */
  function findProfileIndex(ref) {
    const list = profileList();
    if (!ref) return -1;
    if (list[ref.index] && list[ref.index].name === ref.name) return ref.index;
    return list.findIndex((p) => p.name === ref.name);
  }

  /** The profile being viewed on the Gestures page, or null for All apps. */
  function viewProfile() {
    const i = findProfileIndex(store.profile);
    if (i < 0) { store.profile = null; return null; }
    const profile = profileList()[i];
    store.profile = { index: i, name: profile.name };
    return { index: i, profile };
  }

  function applyState(state) {
    if (!state || !state.config) return;
    store.app = state;
    renderShell();
    if (store.page === 'gestures') renderGestures();
    else if (store.page === 'settings') renderSettings();
    else if (store.page === 'live') renderLive();
    else if (store.page === 'about') renderAbout();
    if (tour.open) tourOnState();
  }

  /* =====================================================================
     4. Toasts, modal, save indicator
     ===================================================================== */

  function toast({ kind = 'success', title, text, timeout, action }) {
    const root = $('#toasts');
    const glyph = kind === 'error' ? 'warning' : kind === 'info' ? 'zap' : 'check';
    const el = h('div', { class: `toast toast--${kind}`, role: kind === 'error' ? 'alert' : 'status' },
      h('span', { class: 'toast-icon' }, ico(glyph, 13)),
      h('div', { class: 'toast-body' },
        title ? h('div', { class: 'toast-title' }, title) : null,
        text ? h('div', { class: 'toast-text' }, text) : null,
        action ? h('button', { class: 'btn btn--sm btn--primary toast-action', type: 'button', onClick: () => { dismiss(); action.onClick(); } }, action.label) : null),
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
        if (!$('.drawer', root) && !tour.open) setInert(false);
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

  /** Keep Tab focus inside a dialog. */
  function trapTab(e, root) {
    const focusables = $$('button, input, select, textarea, [tabindex]', root)
      .filter((el) => !el.disabled && el.tabIndex >= 0 && el.offsetParent !== null);
    if (!focusables.length) return;
    const first = focusables[0], last = focusables[focusables.length - 1];
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
  }

  /**
   * Side drawer shell shared by the binding and profile editors: backdrop,
   * header (icon, title, subtitle, close), scrolling body, footer. Makes the
   * app inert while open. Returns { el, close() }.
   */
  function mountDrawer({ headIcon, title, sub, body, after, foot, onClose, onKey }) {
    const root = $('#overlay-root');
    const backdrop = h('div', { class: 'backdrop', onClick: () => onClose() });
    const el = h('div', { class: 'drawer', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': 'drawer-title' },
      h('header', { class: 'drawer-head' },
        headIcon,
        h('div', { class: 'drawer-titles' },
          h('h2', { id: 'drawer-title' }, title),
          sub ? h('div', { class: 'sub' }, sub) : null),
        h('button', { class: 'btn btn--ghost btn--icon', type: 'button', 'aria-label': 'Close', onClick: () => onClose() }, ico('x', 18))),
      body,
      after || null,
      h('footer', { class: 'drawer-foot' }, foot));
    el.addEventListener('keydown', (e) => {
      if (e.key === 'Tab') trapTab(e, el);
      else if (onKey) onKey(e);
    });
    root.append(backdrop, el);
    setInert(true);
    return {
      el,
      close() {
        el.classList.add('is-closing');
        backdrop.classList.add('is-closing');
        setTimeout(() => { el.remove(); backdrop.remove(); }, reducedMotion() ? 0 : 220);
        setInert(false);
      },
    };
  }

  const ping = (el) => { if (el) { el.classList.remove('is-ping'); void el.offsetWidth; el.classList.add('is-ping'); } };
  const profileBadge = (name) => h('span', { class: 'badge badge--profile', title: `Per-app profile: ${name}` }, ico('profile', 12), h('span', { class: 'badge-text' }, name));

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
    } else if (kind === 'tap' || kind === 'doubletap' || kind === 'tripletap') {
      const pts = fingerPoints(n, 60, 39);
      inner = pts.map(([x, y]) => `<circle class="illo-ripple" cx="${x}" cy="${y}" r="${r}"/>`).join('') + fingerG(pts, r);
      // A small ×2 / ×3 tells the multi-taps apart even when they're not animating.
      if (kind !== 'tap') inner += `<text class="illo-count" x="108" y="66" text-anchor="end">×${kind === 'doubletap' ? 2 : 3}</text>`;
    } else if (kind === 'hold') {
      inner = '<circle class="illo-ring-track" cx="60" cy="38" r="26"/><circle class="illo-ring" cx="60" cy="38" r="26" pathLength="100"/>' + fingerG(fingerPoints(n, 60, 39), r);
    } else if (kind === 'pinch') {
      // Fingertips on an arc with the thumb below; they travel to the dashed ring.
      const angles = PINCH_ANGLES[n] || PINCH_ANGLES[4];
      const [r0, r1] = dir === 'in' ? [24, 11] : [11, 24];
      const at = (deg, rad) => [60 + Math.cos((deg * Math.PI) / 180) * rad, 38 + Math.sin((deg * Math.PI) / 180) * rad * 0.82];
      const f = (v) => v.toFixed(2);
      const ringR = dir === 'in' ? r1 + 6 : r1 + 4;
      let guides = '', fingers = '';
      angles.forEach((deg) => {
        const [x0, y0] = at(deg, r0), [x1, y1] = at(deg, r1);
        guides += `<line x1="${f(x0)}" y1="${f(y0)}" x2="${f(x1)}" y2="${f(y1)}"/>`;
        // data-px/py = travel vector, turned into CSS variables by illoEl().
        fingers += `<g class="illo-f" data-px="${f(x1 - x0)}" data-py="${f(y1 - y0)}"><circle class="illo-halo" cx="${f(x0)}" cy="${f(y0)}" r="${r * 1.8}"/><circle class="illo-finger" cx="${f(x0)}" cy="${f(y0)}" r="${r - 0.6}"/></g>`;
      });
      inner = `<ellipse class="illo-pinch-ring" cx="60" cy="38" rx="${ringR}" ry="${f(ringR * 0.82)}"/><g class="illo-guide">${guides}</g><g class="illo-fingers">${fingers}</g>`;
    } else {
      const pts = fingerPoints(n, 60, 37);
      inner = `<g class="illo-move"><rect class="illo-window" x="36" y="17" width="48" height="34" rx="5"/><rect class="illo-window-bar" x="36.5" y="17.5" width="47" height="8" rx="4.5"/>${fingerG(pts, r - 0.6)}</g>`;
    }
    return `<svg class="illo illo--${kind}${dir ? ' illo--' + dir : ''}" viewBox="0 0 120 76" aria-hidden="true" focusable="false"><rect class="illo-pad" x="2" y="2" width="116" height="72" rx="12"/>${inner}</svg>`;
  }

  // Finger angles (degrees, y down) for the pinch illustration: fingertips above, thumb below.
  const PINCH_ANGLES = { 3: [-140, -40, 95], 4: [-150, -95, -40, 100], 5: [-155, -112, -68, -25, 110] };

  /** Turn data-px/data-py travel vectors into CSS variables (through CSSOM, no inline style markup). */
  function applyTravelVars(svg) {
    $$('[data-px]', svg).forEach((g) => {
      g.style.setProperty('--px', g.dataset.px + 'px');
      g.style.setProperty('--py', g.dataset.py + 'px');
    });
    return svg;
  }

  /** Illustration element for a gesture slot. */
  const illoEl = (kind, n, dir) => applyTravelVars(frag(illoSVG(kind, n, dir)));

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
    gestUI.scope = h('div', { class: 'scope-bar' });
    gestUI.strip = h('div', { class: 'profile-strip', hidden: true });
    gestUI.groups = h('div');
    page.append(gestUI.banners, gestUI.scope, gestUI.strip, h('div', { class: 'gest-toolbar' }, gestUI.seg, gestUI.summary), gestUI.groups);
    new ResizeObserver(positionSegIndicator).observe(gestUI.seg);
    gestUI.built = true;
  }

  /* ---------- Profile switcher ("All apps" + one pill per profile) ---------- */

  function renderScope(view) {
    const list = profileList();
    // Rebuilding drops focus, so remember which pill had it.
    const focused = gestUI.scope.contains(document.activeElement) ? document.activeElement.dataset.scope : null;
    const group = h('div', { class: 'scope-group', role: 'radiogroup', 'aria-label': 'Show gestures for' });
    const pill = (key, iconName, label, extra, title) => h('button', {
      class: 'scope-btn', type: 'button', role: 'radio', 'data-scope': key, title,
      'aria-checked': String(key === (view ? String(view.index) : 'all')),
      onClick: () => pickScope(key),
    }, ico(iconName, 15), h('span', { class: 'scope-name' }, label), extra);
    group.append(pill('all', 'layers', 'All apps', null, 'Gestures everywhere, unless a profile changes them'));
    list.forEach((p, i) => {
      const count = (p.binding || []).length;
      group.append(pill(String(i), 'profile', p.name || 'Untitled',
        count ? h('span', { class: 'scope-count', 'aria-label': `${count} override${count === 1 ? '' : 's'}` }, String(count)) : null,
        `${p.name}: ${(p.apps || []).join(', ') || 'no apps yet'}`));
    });
    roving(group, '.scope-btn', (el) => pickScope(el.dataset.scope));
    gestUI.scope.replaceChildren(...[
      group,
      h('button', { class: 'btn btn--sm btn--ghost scope-new', type: 'button', onClick: () => openProfileEditor(null) }, ico('plus', 14), 'New profile'),
      list.length ? null : h('span', { class: 'scope-hint' }, 'Give specific apps their own gestures'),
    ].filter(Boolean)); // replaceChildren() would print "null"
    if (focused) { const el = $(`.scope-btn[data-scope="${focused}"]`, group); if (el) el.focus(); }
  }

  function pickScope(key) {
    const list = profileList();
    const next = key === 'all' || !list[Number(key)] ? null : { index: Number(key), name: list[Number(key)].name };
    const same = next ? !!store.profile && store.profile.index === next.index : !store.profile;
    if (same) return;
    store.profile = next;
    renderGestures(true);
  }

  /** The strip under the switcher that says which apps the viewed profile covers. */
  function renderStrip(view) {
    gestUI.strip.hidden = !view;
    if (!view) { gestUI.strip.replaceChildren(); return; }
    const p = view.profile;
    const n = (p.binding || []).length;
    const apps = p.apps || [];
    gestUI.strip.replaceChildren(
      h('span', { class: 'panel-icon' }, ico('profile', 16)),
      h('div', { class: 'profile-strip-main' },
        h('div', { class: 'profile-strip-title' },
          h('strong', null, p.name || 'Untitled profile'),
          h('span', { class: 'profile-strip-sub' }, `${n} override${n === 1 ? '' : 's'} · everything else follows All apps`)),
        apps.length
          ? h('div', { class: 'app-chips', role: 'list', 'aria-label': 'Apps in this profile' },
            h('span', { class: 'app-chips-label' }, 'While in front:'),
            apps.map((a) => h('span', { class: 'app-chip', role: 'listitem' }, a)))
          : h('div', { class: 'profile-strip-warn' }, ico('warning', 13), 'No apps yet, so this profile never applies.')),
      h('button', { class: 'btn btn--sm', type: 'button', onClick: () => openProfileEditor(view.index) }, ico('pencil', 14), 'Edit profile'));
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
    const view = viewProfile();

    renderScope(view);
    renderStrip(view);
    if (store.page === 'gestures') {
      $('#page-sub').textContent = view
        ? `Changes for ${view.profile.name || 'this profile'}. Everything else follows All apps.`
        : $('#page-gestures').dataset.sub;
    }

    // Banners: conflicts + paused
    gestUI.banners.replaceChildren();
    if (!cfg.settings.enabled) {
      gestUI.banners.append(h('div', { class: 'banner banner--info' }, ico('power', 18),
        h('div', { class: 'banner-body' }, h('span', { class: 'banner-title' }, 'Pady is paused'),
          h('span', { class: 'banner-text' }, 'Gestures do nothing until you turn Pady back on.')),
        h('div', { class: 'banner-actions' }, h('button', { class: 'btn btn--sm btn--primary', type: 'button', onClick: () => { $('#master-switch').click(); } }, 'Turn on'))));
    }
    // One banner here, however many conflicts; Settings lists them all.
    const conflicts = s.conflicts || [];
    if (conflicts.length) {
      const one = conflicts.length === 1;
      const canOpen = conflicts.some((c) => c.can_open_settings);
      const canFix = conflicts.some((c) => c.can_fix);
      gestUI.banners.append(h('div', { class: 'banner' }, ico('warning', 18),
        h('div', { class: 'banner-body' },
          h('span', { class: 'banner-title' }, one ? conflicts[0].message : `${conflicts.length} system settings clash with your gestures`),
          h('span', { class: 'banner-text' }, one ? conflicts[0].fix : conflicts.map((c) => c.message).join(' '))),
        h('div', { class: 'banner-actions' },
          one ? null : h('button', { class: 'btn btn--sm', type: 'button', onClick: () => { location.hash = '#settings'; } }, 'Details'),
          canFix ? fixPermissionsButton() : null,
          canOpen ? h('button', { class: 'btn btn--sm', type: 'button', onClick: openTouchpadSettings }, ico('external', 14), 'Open settings') : null)));
    }

    // Segmented control: counts are bindings (All apps) or overrides (profile view).
    $$('.seg-btn', gestUI.seg).forEach((btn) => {
      const bn = Number(btn.dataset.n);
      const list = view ? (view.profile.binding || []) : cfg.binding;
      const count = list.filter((b) => M.parseTrigger(b.trigger).fingers === bn).length;
      btn.setAttribute('aria-checked', String(bn === n));
      $('.seg-count', btn).textContent = count;
      btn.setAttribute('aria-label', view
        ? `${bn} fingers, ${count} override${count === 1 ? '' : 's'}`
        : `${bn} fingers, ${count} gesture${count === 1 ? '' : 's'} set`);
    });
    gestUI.syncRoving();
    requestAnimationFrame(positionSegIndicator);

    if (view) {
      // Drag is always global, so a profile can change one slot fewer.
      const changed = M.SLOTS.filter((sl) => M.overrideFor(view.profile, M.triggerOf(sl, n))).length;
      gestUI.summary.textContent = `${changed} of ${M.SLOTS.length - 1} gestures changed for ${n} fingers`;
    } else {
      const bound = M.SLOTS.filter((sl) => M.bindingFor(cfg, M.triggerOf(sl, n))).length;
      gestUI.summary.textContent = `${bound} of ${M.SLOTS.length} gestures set for ${n} fingers`;
    }

    // Cards
    const dragOn = M.dragOn(cfg, n);
    const groups = [
      { title: 'Taps & hold', slots: ['tap', 'doubletap', 'tripletap', 'hold'], cls: 'grid grid--4' },
      { title: 'Swipe', slots: ['swipe-up', 'swipe-down', 'swipe-left', 'swipe-right'], cls: 'grid grid--4', note: dragOn },
      { title: 'Pinch & drag', slots: ['pinch-in', 'pinch-out', 'drag'], cls: 'grid grid--4' },
    ];
    let i = 0;
    gestUI.groups.replaceChildren(...groups.map((g) => {
      const grid = h('div', { class: g.cls });
      g.slots.forEach((id) => grid.append(cardEl(M.SLOTS.find((x) => x.id === id), n, i++, intro, dragOn, view)));
      const head = h('div', { class: 'group-head' }, h('h2', { class: 'group-title' }, g.title));
      if (g.note) {
        head.append(h('span', { class: 'group-note' }, ico('warning', 14), `${n}-finger drag is on, so ${n}-finger swipes are disabled.`),
          h('button', { class: 'btn btn--sm btn--ghost', type: 'button', onClick: () => turnOffDrag(n) }, view ? 'Turn off drag (All apps)' : 'Turn off drag'));
      }
      return h('section', { class: 'group' }, head, grid);
    }));
    if (intro && !reducedMotion()) {
      const cards = $$('.card', gestUI.groups);
      setTimeout(() => cards.forEach((c) => c.classList.remove('intro')), 2600 + cards.length * 110);
    }
  }

  /**
   * One gesture slot. In a profile view the card shows the profile's override
   * if there is one ("Off in this app" for a `none` override), otherwise the
   * All-apps binding, muted. Drag is global, so it's read-only in profiles.
   */
  function cardEl(slot, n, index, intro, dragOn, view) {
    const s = store.app;
    const trigger = M.triggerOf(slot, n);
    const label = M.triggerLabel(trigger);
    const global = M.bindingFor(s.config, trigger);
    const override = view ? M.overrideFor(view.profile, trigger) : null;
    const off = !!override && override.action.type === 'none';
    const shown = override ? (off ? null : override) : global;   // the binding this card displays
    const inherited = !!view && !override && !!global;
    const readonly = !!view && slot.kind === 'drag';
    const shadowed = (s.shadowed || []).includes(trigger) && !!shown;
    const inert = slot.kind === 'swipe' && dragOn;

    const badges = h('span', { class: 'card-badges' });
    if (shown && shown.repeat && M.isContinuous(slot)) {
      badges.append(h('span', { class: 'badge' + (inherited ? '' : ' badge--accent'), title: 'Keeps firing while your fingers keep moving' }, ico('repeat', 12), 'Repeats'));
    }
    if (shadowed) badges.append(h('span', { class: 'badge badge--warn', title: `${n}-finger drag is on — swipes with ${n} fingers are disabled` }, ico('warning', 12), 'Inactive'));
    if (!view && slot.kind !== 'drag') {
      const others = M.overridingProfiles(s.config, trigger);
      if (others.length) {
        badges.append(h('span', { class: 'badge badge--profile', title: `Changed in ${others.join(', ')}` },
          ico('profile', 12), others.length === 1 ? others[0] : `${others.length} profiles`));
      }
    }

    let body;
    if (off) body = h('span', { class: 'chip chip--off' }, ico('none', 15), h('span', { class: 'chip-text' }, 'Off in this app'));
    else if (shown) { body = chipEl(shown.action); if (inherited || readonly) body.classList.add('chip--inherited'); }
    else if (inert) body = h('span', { class: 'chip chip--empty' }, ico('none', 15), h('span', { class: 'chip-text' }, 'Off while drag is on'));
    else if (readonly) body = h('span', { class: 'chip chip--empty' }, ico('none', 15), h('span', { class: 'chip-text' }, 'Off'));
    else body = h('span', { class: 'chip chip--empty' }, ico('plus', 15), h('span', { class: 'chip-text' }, 'Not set — click to assign'));

    const summary = off ? 'off in this app'
      : shown ? M.summarize(shown.action, platform()).text + (inherited ? ' (from All apps)' : '')
        : 'not set';
    const kids = [
      h('div', { class: 'card-illo' }, illoEl(slot.kind, n, slot.dir), badges),
      h('div', { class: 'card-body' },
        h('div', { class: 'card-title' }, h('span', { class: 'card-name' }, slot.name), readonly ? null : h('span', { class: 'card-edit' }, ico('chevron', 15))),
        body,
        inherited && !readonly ? h('div', { class: 'card-from' }, ico('layers', 12), 'From All apps') : null,
        readonly ? h('div', { class: 'card-from card-from--readonly' }, ico('layers', 12), 'Drag is set under All apps',
          h('button', { class: 'link-btn', type: 'button', onClick: () => editInAllApps(trigger) }, 'Change')) : null,
        shadowed ? h('div', { class: 'card-note' }, `${n}-finger drag is on — swipes with ${n} fingers are disabled.`) : null),
    ];
    const cls = 'card' + (shown && !inherited && !readonly ? ' is-bound' : '') + (override ? ' is-override' : '') + (off ? ' is-off' : '')
      + (inherited ? ' is-inherited' : '') + (shadowed ? ' is-shadowed' : '') + (inert ? ' is-inert' : '') + (readonly ? ' is-readonly' : '') + (intro ? ' intro enter' : '');
    if (readonly) {
      return h('div', { class: cls, 'data-trigger': trigger, role: 'group', 'aria-label': `${label}: ${summary}. Set under All apps`, style: { '--i': String(index) } }, kids);
    }
    return h('button', {
      type: 'button', class: cls, 'data-trigger': trigger,
      'aria-label': `${label}: ${summary}${shadowed ? ' (inactive while drag is on)' : ''}. Edit`,
      style: { '--i': String(index) },
      onClick: () => openEditor(trigger),
    }, kids);
  }

  /** Switch the Gestures page to All apps and open a slot's editor there. */
  function editInAllApps(trigger) {
    store.profile = null;
    renderGestures(true);
    openEditor(trigger);
  }

  async function turnOffDrag(n) {
    try {
      applyState(await call('save_config', { config: M.withBinding(store.app.config, `drag:${n}`, { type: 'none' }) }));
      toast({ title: `${n}-finger drag turned off`, text: `${n}-finger swipes work again.` });
    } catch (_) { /* toast shown */ }
  }

  /**
   * Light up the card for a trigger that just fired (Gestures page). If the
   * binding that ran belongs to another scope (a profile, or All apps while a
   * profile overriding it is shown), ping that scope's pill instead.
   */
  function fireCard(trigger, profileName) {
    if (!trigger || store.page !== 'gestures' || !gestUI.built) return;
    const view = viewProfile();
    const shownHere = profileName
      ? !!view && view.profile.name === profileName
      : !view || !M.overrideFor(view.profile, trigger);
    if (!shownHere) {
      const key = profileName ? String(profileList().findIndex((pr) => pr.name === profileName)) : 'all';
      ping($(`.scope-btn[data-scope="${key}"]`, gestUI.scope));
      return;
    }
    const p = M.parseTrigger(trigger);
    if (p.fingers !== store.fingers) {
      ping($(`.seg-btn[data-n="${p.fingers}"]`));
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

  /** Linux: installs Pady's udev rule after one password prompt, then re-renders. */
  function fixPermissionsButton() {
    const btn = h('button', { class: 'btn btn--sm btn--primary', type: 'button' }, ico('check', 14), 'Fix permissions');
    btn.addEventListener('click', async () => {
      btn.disabled = true;
      btn.lastChild.textContent = 'Waiting for password…';
      try {
        applyState(await call('fix_permissions', null, { errorTitle: 'Couldn’t fix permissions' }));
        toast({ title: 'Permissions fixed', text: 'Pady can now read your touchpad and send keys.' });
      } catch (_) {
        btn.disabled = false;
        btn.lastChild.textContent = 'Fix permissions';
      }
    });
    return btn;
  }

  /* =====================================================================
     8. Binding editor (drawer)
     ===================================================================== */

  let editor = null;
  let presetsExpanded = false; // remembered across editor openings
  const PRESETS_COLLAPSED = 8;

  /**
   * Open the editor for a slot. On All apps it edits the global binding; in a
   * profile view it edits that profile's override, with three modes:
   *   inherit — no override, the All-apps binding applies
   *   off     — override with `none`: the gesture does nothing in these apps
   *   custom  — override with its own action
   */
  function openEditor(trigger) {
    if (editor || profEd) return;
    const s = store.app;
    const view = viewProfile();
    const slot = M.slotOfTrigger(trigger);
    if (!slot || (view && slot.kind === 'drag')) return; // drag is global-only
    const p = M.parseTrigger(trigger);
    const global = M.bindingFor(s.config, trigger);
    const override = view ? M.overrideFor(view.profile, trigger) : null;
    // What the action fields start from; a profile's "Custom" starts from the All-apps action.
    const from = override && override.action.type !== 'none' ? override : global;
    editor = {
      trigger, slot, n: p.fingers,
      scope: view ? { index: view.index, name: view.profile.name } : null,
      global,
      original: view ? override : global,
      mode: !view ? 'custom' : !override ? 'inherit' : override.action.type === 'none' ? 'off' : 'custom',
      draft: from ? clone(from.action) : { type: 'none' },
      repeat: from ? !!from.repeat : false,
      prevFocus: document.activeElement,
      busy: false,
    };
    buildDrawer();
  }

  function buildDrawer() {
    const ed = editor;
    const label = M.triggerLabel(ed.trigger);
    ed.body = h('div', { class: 'drawer-body' });
    ed.errorEl = h('div', { class: 'drawer-error' });
    ed.saveBtn = h('button', { class: 'btn btn--primary', type: 'button', onClick: saveEditor }, ico('check', 15), 'Save', h('span', { class: 'kbd-hint' }, '⏎'));
    ed.testBtn = h('button', { class: 'btn', type: 'button', onClick: testEditor, title: 'Run this action now' }, ico('zap', 15), 'Test');
    ed.removeBtn = h('button', { class: 'btn btn--danger', type: 'button', onClick: removeEditor }, ico('trash', 15), 'Remove');
    // In a profile, "Use All-apps setting" is the way to remove an override.
    ed.removeBtn.hidden = !ed.original || !!ed.scope;

    ed.drawer = mountDrawer({
      headIcon: h('div', { class: 'illo-wrap illo-live' }, illoEl(ed.slot.kind, ed.n, ed.slot.dir)),
      title: cap(label),
      sub: [h('code', null, ed.trigger), h('span', null, '·'), ed.scope ? profileBadge(ed.scope.name) : h('span', null, ed.slot.hint)],
      body: ed.body,
      after: ed.errorEl,
      foot: [ed.removeBtn, h('span', { class: 'spacer' }), ed.testBtn, ed.saveBtn],
      onClose: closeEditor,
      onKey: onDrawerKey,
    });
    ed.el = ed.drawer.el;
    renderEditorBody();
    // Focus the current choice for fast keyboard use.
    const target = $('.mode[aria-checked="true"]', ed.el) || $('.preset[aria-pressed="true"]', ed.el) || $('.preset', ed.el) || ed.saveBtn;
    target.focus({ preventScroll: true });
  }

  function onDrawerKey(e) {
    if (!editor) return;
    if (e.key === 'Escape') { e.preventDefault(); closeEditor(); return; }
    if (e.key === 'Enter' && !e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey) {
      const t = e.target;
      if (t.tagName === 'BUTTON' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.closest('.recorder')) return;
      e.preventDefault();
      saveEditor();
    }
  }

  function closeEditor() {
    const ed = editor;
    if (!ed) return;
    editor = null;
    if (ed.recorder) ed.recorder.destroy();
    ed.drawer.close();
    // Return focus to the card (re-rendered cards are found by trigger).
    const target = $(`.card[data-trigger="${ed.trigger}"]`) || ed.prevFocus;
    if (target && target.isConnected && target.focus) target.focus();
  }

  /** The three choices for a slot inside a profile. */
  function modeSection() {
    const ed = editor;
    const name = ed.scope.name;
    const g = ed.global;
    const opts = [
      {
        id: 'inherit', icon: 'layers', title: 'Use All-apps setting',
        sub: g ? [chipEl(g.action), g.repeat && M.isContinuous(ed.slot) ? h('span', { class: 'badge' }, ico('repeat', 12), 'Repeats') : null]
          : [h('span', null, 'Not set in All apps, so nothing happens.')],
      },
      { id: 'off', icon: 'none', title: 'Turn off in this app', sub: [h('span', null, `Does nothing while ${name} is in front.`)] },
      { id: 'custom', icon: 'pencil', title: 'Custom action', sub: [h('span', null, `Choose what it does in ${name}.`)] },
    ];
    const group = h('div', { class: 'modes', role: 'radiogroup', 'aria-label': `In ${name}` });
    opts.forEach((o) => group.append(h('button', {
      class: 'mode', type: 'button', role: 'radio', 'data-mode': o.id, 'aria-checked': String(ed.mode === o.id),
      onClick: () => setMode(o.id),
    },
    h('span', { class: 'mode-radio', 'aria-hidden': 'true' }),
    h('span', { class: 'mode-title' }, ico(o.icon, 15), o.title),
    h('span', { class: 'mode-sub' }, o.sub))));
    roving(group, '.mode', (el) => setMode(el.dataset.mode));
    return h('section', { class: 'drawer-section' }, h('div', { class: 'section-label' }, `In ${name}`), group);
  }

  function setMode(mode) {
    const ed = editor;
    if (!ed || ed.mode === mode) return;
    ed.mode = mode;
    renderEditorBody();
    const btn = $(`.mode[data-mode="${mode}"]`, ed.el);
    if (btn) btn.focus();
  }

  /** Rebuild every section of the drawer body (called when the mode or action type changes). */
  function renderEditorBody() {
    const ed = editor;
    const isDrag = ed.slot.kind === 'drag';
    const continuous = M.isContinuous(ed.slot);
    const scroll = ed.body.scrollTop;
    ed.body.replaceChildren();

    // Shadow warning
    if (ed.slot.kind === 'swipe' && M.dragOn(store.app.config, ed.n) && ed.mode !== 'off') {
      ed.body.append(h('div', { class: 'drawer-section' }, h('div', { class: 'banner' }, ico('warning', 18),
        h('div', { class: 'banner-body' }, h('span', { class: 'banner-title' }, `${ed.n}-finger drag is on`),
          h('span', { class: 'banner-text' }, `Swipes with ${ed.n} fingers are disabled while drag uses them. You can still save this — it starts working once drag is turned off.`)))));
    }

    if (ed.scope) {
      ed.body.append(modeSection());
      if (ed.mode !== 'custom') {
        ed.body.scrollTop = scroll;
        refreshEditor();
        return;
      }
    }

    // Presets
    const presets = M.presetsFor(ed.slot, platform(), ed.n);
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
          if (continuous) ed.repeat = !!pr.repeat;
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

      // Repeat toggle (swipes and pinches)
      if (continuous) {
        const pinch = ed.slot.kind === 'pinch';
        const step = fmtNum(store.app.config.settings.swipe_step_mm, 1);
        const input = h('input', { type: 'checkbox', role: 'switch', checked: ed.repeat, 'aria-describedby': 'repeat-sub', onChange: (e) => { ed.repeat = e.target.checked; refreshEditor(); } });
        ed.body.append(h('section', { class: 'drawer-section' },
          h('label', { class: 'toggle-row' },
            h('span', { class: 'toggle-row-text' },
              h('span', { class: 'toggle-row-title' }, ico('repeat', 15), pinch ? 'Keep firing while pinching' : 'Keep firing while sliding'),
              h('span', { class: 'toggle-row-sub', id: 'repeat-sub' }, pinch
                ? `Fires again every ${step} mm your fingers spread or close, so one pinch can zoom several steps.`
                : `Fires again every ${step} mm of travel, so one slide can scrub volume or brightness.`)),
            h('span', { class: 'switch' }, input, h('span', { class: 'switch-track' }, h('span', { class: 'switch-thumb' }))))));
      }
    }

    // Multi-taps delay single taps; say so on the taps of the same finger count.
    const tapsWait = M.isMultiTap(ed.slot)
      || (ed.slot.kind === 'tap' && [`doubletap:${ed.n}`, `tripletap:${ed.n}`].some((t) => M.bindingFor(store.app.config, t)
        || profileList().some((p) => { const o = M.overrideFor(p, t); return o && o.action.type !== 'none'; })));
    if (tapsWait) {
      ed.body.append(h('section', { class: 'drawer-section' }, h('div', { class: 'about-type' }, ico('hold', 18), h('div', null,
        'Binding a double or triple tap makes single taps wait a moment (the double-tap speed in Settings) to tell them apart.',
        h('div', { class: 'help', style: { 'margin-top': '6px' } }, `Currently ${store.app.config.settings.multi_tap_ms ?? 300} ms.`)))));
    }

    // On All apps, say when a profile has its own setting for this slot.
    if (!ed.scope && !isDrag) {
      const names = M.overridingProfiles(store.app.config, ed.trigger);
      if (names.length) {
        const text = names.length === 1
          ? `${names[0]} has its own setting for this gesture, which wins while one of its apps is in front.`
          : `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]} have their own settings for this gesture, which win while their apps are in front.`;
        ed.body.append(h('section', { class: 'drawer-section' }, h('div', { class: 'about-type' }, ico('profile', 18), h('div', null, text))));
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
    // Scrubbing is the usual intent for levels; for pinches, shortcuts (zoom) too.
    if (M.isContinuous(ed.slot)) ed.repeat = type === 'volume' || type === 'brightness' || (ed.slot.kind === 'pinch' && type === 'keys');
    renderEditorBody();
    const btn = $(`.type-opt[data-type="${type}"]`, ed.el);
    if (btn && keepFocus !== false) btn.focus();
  }

  /** What's wrong with the editor's current choice, or null. */
  function editorError(ed) {
    if (ed.scope && ed.mode !== 'custom') return null;
    if (ed.scope && ed.draft.type === 'none') return 'Choose an action, or pick “Turn off in this app”';
    return M.validate(ed.draft);
  }

  function editorUnchanged(ed) {
    const o = ed.original;
    const repeatSame = (b) => !!b.repeat === !!(M.isContinuous(ed.slot) && ed.repeat);
    if (ed.scope) {
      if (!o) return ed.mode === 'inherit';
      if (o.action.type === 'none') return ed.mode === 'off';
      return ed.mode === 'custom' && M.sameAction(o.action, ed.draft) && repeatSame(o);
    }
    return o ? M.sameAction(o.action, ed.draft) && repeatSame(o) : ed.draft.type === 'none';
  }

  /** The action that would run with the current choice (for Test). */
  function editorAction(ed) {
    if (!ed.scope || ed.mode === 'custom') return ed.draft;
    if (ed.mode === 'inherit' && ed.global) return ed.global.action;
    return null;
  }

  /** Update presets / Save / Test / error without rebuilding inputs (keeps focus & caret). */
  function refreshEditor() {
    const ed = editor;
    if (!ed) return;
    $$('.preset[data-preset]', ed.el).forEach((btn) => {
      const pr = M.presetsFor(ed.slot, platform(), ed.n).find((x) => x.id === btn.dataset.preset);
      btn.setAttribute('aria-pressed', String(!!pr && M.sameAction(pr.action, ed.draft)));
    });
    const err = editorError(ed);
    ed.invalid = err;
    // Save stays clickable when invalid so a click can explain what's missing.
    ed.saveBtn.disabled = ed.busy || editorUnchanged(ed);
    const act = editorAction(ed);
    ed.testBtn.disabled = ed.busy || !act || act.type === 'none' || act.type === 'drag' || !!M.validate(act);
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
        const browse = h('button', { class: 'btn btn--sm', type: 'button', onClick: async () => {
          try {
            const picked = await call('pick_program', null, { errorTitle: 'Couldn\u2019t open the file picker' });
            if (picked) { prog.value = picked; set({ program: picked }); }
          } catch (_) { /* toast shown */ }
        } }, ico('folder', 14), 'Browse\u2026');
        const help = a.type === 'focus_or_launch'
          ? 'Brings the app to the front. If it’s already in front, minimises it. If it isn’t running, launches it.'
          : (linux ? 'A program on your PATH or a full path.' : 'An app’s name as it appears in Start, a full path, or a URI such as ms-settings:display.');
        return h('div', null,
          h('div', { class: 'about-type', style: { 'margin-bottom': '12px' } }, ico(a.type === 'focus_or_launch' ? 'switch' : 'apps', 18), h('div', null, help)),
          h('div', { class: 'field' }, h('span', { class: 'label' }, 'Program'), h('div', { class: 'input-row' }, prog, browse),
            linux ? null : h('span', { class: 'help' }, 'Type an app’s name (like Antigravity) or pick it with Browse.')),
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
    const err = editorError(ed);
    // On All apps, type `none` is a valid "unbind"; anything else invalid is explained.
    if (err && (ed.scope || ed.draft.type !== 'none')) {
      ed.touched = true;
      refreshEditor();
      const field = $('.fields .input, .fields .recorder', ed.el) || $('.type-opt', ed.el);
      if (field) field.focus();
      return;
    }
    if (ed.saveBtn.disabled) { closeEditor(); return; } // nothing changed
    const repeat = M.isContinuous(ed.slot) && ed.repeat && ed.draft.type !== 'none';
    const label = cap(M.triggerLabel(ed.trigger));
    let config, done;
    if (ed.scope) {
      const idx = findProfileIndex(ed.scope);
      if (idx < 0) {
        toast({ kind: 'error', title: 'Profile not found', text: `“${ed.scope.name}” was renamed or removed in the meantime.` });
        closeEditor();
        return;
      }
      const name = ed.scope.name;
      config = M.withProfileBinding(store.app.config, idx, ed.trigger, ed.mode, ed.draft, repeat);
      done = ed.mode === 'inherit' ? { title: `Back to All apps in ${name}`, text: `${label} uses the All-apps setting again.` }
        : ed.mode === 'off' ? { title: `Turned off in ${name}`, text: `${label} does nothing while ${name} is in front.` }
          : { title: `Saved for ${name}`, text: `${label} → ${M.summarize(ed.draft, platform()).text}` };
    } else {
      config = M.withBinding(store.app.config, ed.trigger, ed.draft, repeat);
      done = ed.draft.type === 'none'
        ? { title: 'Binding removed', text: `${label} does nothing now.` }
        : { title: 'Saved', text: `${label} → ${M.summarize(ed.draft, platform()).text}` };
    }
    setBusy(true);
    try {
      const state = await call('save_config', { config }, { errorTitle: 'Couldn’t save' });
      applyState(state);
      if (editor === ed) closeEditor();
      toast(done);
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
    const act = ed && editorAction(ed);
    if (!act || act.type === 'none' || M.validate(act)) return;
    const btn = ed.testBtn;
    btn.classList.add('is-busy');
    try {
      await call('test_action', { action: M.clean(act) }, { errorTitle: 'Test failed' });
      toast({ kind: 'info', title: 'Ran it', text: M.summarize(act, platform()).text, timeout: 2200 });
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
     8b. Profile editor (drawer): name, apps, detect, delete
     ===================================================================== */

  let profEd = null;
  const DETECT_MS = 10000;
  const SUGGESTED_APPS = {
    windows: ['chrome.exe', 'msedge.exe', 'firefox.exe', 'Code.exe', 'Spotify.exe', 'explorer.exe'],
    linux: ['firefox', 'google-chrome', 'chromium', 'code', 'spotify', 'nautilus'],
  };

  /** Open the profile editor for profile `index`, or for a new profile when `index` is null. */
  function openProfileEditor(index) {
    if (editor || profEd) return;
    const existing = index != null ? profileList()[index] : null;
    profEd = {
      ref: existing ? { index, name: existing.name } : null,
      index: existing ? index : null,
      name: existing ? existing.name : '',
      apps: existing ? (existing.apps || []).slice() : [],
      overrides: existing ? (existing.binding || []).length : 0,
      prevFocus: document.activeElement,
      busy: false,
      touched: false,
      detect: null,       // { token, endsAt, timer } while waiting for detect_app
      detectHint: '',
    };
    const pe = profEd;
    pe.body = h('div', { class: 'drawer-body' });
    pe.errorEl = h('div', { class: 'drawer-error' });
    pe.saveBtn = h('button', { class: 'btn btn--primary', type: 'button', onClick: saveProfile }, ico('check', 15), existing ? 'Save' : 'Create profile');
    pe.deleteBtn = existing ? h('button', { class: 'btn btn--danger', type: 'button', onClick: deleteProfile }, ico('trash', 15), 'Delete profile') : null;
    pe.drawer = mountDrawer({
      headIcon: h('div', { class: 'illo-wrap drawer-head-icon' }, ico('profile', 30)),
      title: existing ? 'Edit profile' : 'New profile',
      sub: [h('span', null, 'Gestures that change while certain apps are in front')],
      body: pe.body,
      after: pe.errorEl,
      foot: [pe.deleteBtn, h('span', { class: 'spacer' }), h('button', { class: 'btn', type: 'button', onClick: () => closeProfileEditor() }, 'Cancel'), pe.saveBtn],
      onClose: () => closeProfileEditor(),
      onKey: (e) => {
        if (e.key === 'Escape') {
          e.preventDefault();
          if (pe.detect) cancelDetect(); else closeProfileEditor();
        } else if (e.key === 'Enter' && !e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey && e.target.tagName === 'INPUT') {
          e.preventDefault();
          saveProfile();
        }
      },
    });
    renderProfileBody();
    pe.nameInput.focus({ preventScroll: true });
    if (existing) pe.nameInput.select();
  }

  function renderProfileBody() {
    const pe = profEd;
    const linux = platform() === 'linux';

    pe.nameInput = h('input', { class: 'input', type: 'text', value: pe.name, placeholder: 'e.g. Browser', maxlength: '40', autocomplete: 'off', spellcheck: 'false', 'aria-labelledby': 'prof-name-label', 'aria-describedby': 'prof-name-help' });
    pe.nameInput.addEventListener('input', () => { pe.name = pe.nameInput.value; pe.touched = true; refreshProfile(); });

    pe.appInput = h('input', { type: 'text', autocomplete: 'off', spellcheck: 'false', 'aria-label': 'Add an app', 'aria-describedby': 'prof-apps-help' });
    pe.appInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ',' || e.key === ';') {
        if (pe.appInput.value.trim()) { e.preventDefault(); e.stopPropagation(); commitAppInput(); }
        else if (e.key !== 'Enter') e.preventDefault(); // an empty Enter falls through to "save"
      } else if (e.key === 'Backspace' && !pe.appInput.value && pe.apps.length) {
        e.preventDefault();
        pe.apps.pop();
        pe.touched = true;
        renderChips();
      }
    });
    pe.appInput.addEventListener('paste', (e) => {
      const text = (e.clipboardData && e.clipboardData.getData('text')) || '';
      if (/[,;\r\n]/.test(text)) { e.preventDefault(); addApps(text.split(/[,;\r\n]+/)); }
    });
    pe.appInput.addEventListener('blur', () => { if (pe.appInput.value.trim()) commitAppInput(); });

    pe.chipList = h('span', { class: 'chip-list', role: 'list', 'aria-label': 'Apps in this profile' });
    pe.chipsEl = h('div', { class: 'chip-input', onClick: (e) => { if (e.target === pe.chipsEl || e.target === pe.chipList) pe.appInput.focus(); } }, pe.chipList, pe.appInput);
    pe.detectBtn = h('button', { class: 'btn detect-btn', type: 'button', onClick: startDetect, title: 'Switch to the app and Pady picks up its name' }, ico('target', 15), 'Detect app');
    pe.detectEl = h('div', { class: 'detect-area' });
    pe.detectLive = h('span', { class: 'sr-only', 'aria-live': 'polite' });
    pe.suggestEl = h('div', { class: 'suggest' });
    pe.notesEl = h('div', { class: 'prof-notes' });
    pe.emptyNote = h('div', { class: 'warn-text' }, ico('warning', 13), 'Add at least one app — until then this profile never applies.');

    const n = pe.overrides;
    pe.body.replaceChildren(
      h('section', { class: 'drawer-section' },
        h('div', { class: 'field' },
          h('span', { class: 'label', id: 'prof-name-label' }, 'Name'),
          pe.nameInput,
          h('span', { class: 'help', id: 'prof-name-help' }, 'Shown on the Gestures page and in Live when it applies.'))),
      h('section', { class: 'drawer-section' },
        h('div', { class: 'field' },
          h('span', { class: 'label' }, 'Apps'),
          h('div', { class: 'apps-row' }, pe.chipsEl, pe.detectBtn),
          h('span', { class: 'help', id: 'prof-apps-help' }, `Program names, separated by commas — e.g. ${linux ? 'firefox, code, spotify' : 'chrome.exe, code, spotify'}. Case, folders and .exe don’t matter.`),
          pe.detectEl, pe.detectLive, pe.emptyNote, pe.notesEl, pe.suggestEl)),
      h('section', { class: 'drawer-section' },
        h('div', { class: 'about-type' }, ico('layers', 18), h('div', null,
          'While one of these apps is in front, the gestures you change in this profile replace the All-apps ones; everything else works as usual. Drag always comes from All apps.',
          pe.ref ? h('div', { class: 'help', style: { 'margin-top': '6px' } }, n ? `${n} gesture${n === 1 ? '' : 's'} changed in this profile.` : 'No gestures changed yet — pick this profile on the Gestures page, then click a gesture.') : null))));
    renderChips();
    renderDetect();
  }

  /** Chips, suggestions and duplicate-app notes. */
  function renderChips() {
    const pe = profEd;
    if (!pe) return;
    pe.chipList.replaceChildren(...pe.apps.map((a, i) => h('span', { class: 'app-chip app-chip--edit', role: 'listitem', 'data-key': M.appKey(a) },
      h('span', { class: 'app-chip-name' }, a),
      h('button', {
        type: 'button', 'aria-label': `Remove ${a}`, title: 'Remove',
        onClick: (e) => { e.stopPropagation(); pe.apps.splice(i, 1); pe.touched = true; renderChips(); pe.appInput.focus(); },
      }, ico('x', 12)))));
    pe.appInput.placeholder = pe.apps.length ? 'Add another…' : (platform() === 'linux' ? 'firefox, code, spotify' : 'chrome.exe, code, spotify');

    const have = new Set(pe.apps.map(M.appKey));
    const suggestions = (SUGGESTED_APPS[platform()] || SUGGESTED_APPS.windows).filter((a) => !have.has(M.appKey(a)));
    pe.suggestEl.replaceChildren(...(suggestions.length ? [h('span', { class: 'suggest-label' }, 'Suggestions')].concat(suggestions.map((a) =>
      h('button', { class: 'preset', type: 'button', onClick: () => { addApps([a]); pe.appInput.focus(); } }, ico('plus', 12), a))) : []));

    // The backend uses the first profile that lists the app in front.
    const notes = [];
    pe.apps.forEach((a) => M.profilesWithApp(store.app.config, a, pe.index).forEach(({ p, index }) => {
      const otherFirst = pe.index == null || index < pe.index;
      notes.push(h('div', { class: 'warn-text' }, ico('warning', 13), otherFirst
        ? `${a} is also in “${p.name}”, which comes first, so that profile wins for it.`
        : `${a} is also in “${p.name}”; this profile comes first, so it wins.`));
    }));
    pe.notesEl.replaceChildren(...notes);
    refreshProfile();
  }

  /** Add typed/detected/suggested apps, skipping case-insensitive duplicates (flashing the existing chip). */
  function addApps(raw) {
    const pe = profEd;
    if (!pe) return [];
    const added = [];
    const dupes = [];
    for (const r of raw) {
      const a = M.cleanApp(r);
      if (!a) continue;
      const key = M.appKey(a);
      if (pe.apps.some((x) => M.appKey(x) === key)) { dupes.push(key); continue; }
      pe.apps.push(a);
      added.push(a);
    }
    pe.touched = true;
    renderChips();
    dupes.forEach(flashChip); // after re-rendering, or the flash would be thrown away
    return added;
  }

  function commitAppInput() {
    const pe = profEd;
    if (!pe || !pe.appInput.value.trim()) return;
    const text = pe.appInput.value;
    pe.appInput.value = '';
    addApps(text.split(/[,;\r\n]+/));
  }

  function flashChip(key) {
    const chip = profEd && $(`.app-chip[data-key="${CSS.escape(key)}"]`, profEd.chipList);
    if (!chip) return;
    chip.classList.remove('is-flash');
    void chip.offsetWidth;
    chip.classList.add('is-flash');
  }

  /* ---------- Detect app: wait for the next app that comes to the front ---------- */

  function startDetect() {
    const pe = profEd;
    if (!pe || pe.detect) return;
    commitAppInput();
    const token = {};
    pe.detect = { token, endsAt: Date.now() + DETECT_MS, timer: setInterval(tickDetect, 250) };
    pe.detectHint = '';
    pe.detectLive.textContent = 'Waiting. Switch to the app you want to add.';
    renderDetect();
    const cancel = $('.detect-panel .btn', pe.detectEl);
    if (cancel) cancel.focus();
    call('detect_app', { timeoutMs: DETECT_MS }, { errorTitle: 'Couldn’t detect the app' }).then((app) => {
      // Ignore late results after Cancel, a new attempt, or closing the editor.
      if (profEd !== pe || !pe.detect || pe.detect.token !== token) return;
      stopDetect();
      if (app) {
        const key = M.appKey(app);
        if (pe.apps.some((a) => M.appKey(a) === key)) {
          flashChip(key);
          toast({ kind: 'info', title: `${app} is already in this profile`, timeout: 2400 });
          pe.detectLive.textContent = `${app} is already in this profile.`;
        } else {
          addApps([app]);
          toast({ title: `Added ${app}` });
          pe.detectLive.textContent = `Added ${app}.`;
        }
      } else {
        pe.detectHint = 'No other app came to the front.';
        pe.detectLive.textContent = pe.detectHint;
        renderDetect();
      }
      pe.detectBtn.focus();
    }, () => {
      if (profEd === pe && pe.detect && pe.detect.token === token) { stopDetect(); pe.detectBtn.focus(); }
    });
  }

  function tickDetect() {
    const pe = profEd;
    if (!pe || !pe.detect) return;
    const el = $('.detect-count', pe.detectEl);
    if (el) el.textContent = `${Math.max(0, Math.ceil((pe.detect.endsAt - Date.now()) / 1000))}s`;
  }

  function stopDetect() {
    const pe = profEd;
    if (!pe || !pe.detect) return;
    clearInterval(pe.detect.timer);
    pe.detect = null;
    renderDetect();
  }

  function cancelDetect() {
    const pe = profEd;
    if (!pe || !pe.detect) return;
    stopDetect();
    pe.detectLive.textContent = 'Detection cancelled.';
    pe.detectBtn.focus();
  }

  function renderDetect() {
    const pe = profEd;
    if (!pe) return;
    pe.detectBtn.disabled = !!pe.detect;
    pe.detectBtn.classList.toggle('is-waiting', !!pe.detect);
    if (pe.detect) {
      pe.detectEl.replaceChildren(h('div', { class: 'detect-panel' },
        h('span', { class: 'detect-pulse', 'aria-hidden': 'true' }),
        h('span', { class: 'detect-text' }, 'Switch to the app you want…'),
        h('span', { class: 'detect-count', 'aria-hidden': 'true' }, `${Math.ceil(DETECT_MS / 1000)}s`),
        h('button', { class: 'btn btn--sm btn--ghost', type: 'button', onClick: cancelDetect }, 'Cancel')));
      tickDetect();
    } else if (pe.detectHint) {
      pe.detectEl.replaceChildren(h('div', { class: 'detect-hint' }, ico('info', 13), pe.detectHint));
    } else {
      pe.detectEl.replaceChildren();
    }
  }

  /* ---------- Save / delete ---------- */

  function profileError(pe) {
    const name = pe.name.trim();
    if (!name) return 'Give the profile a name';
    const lower = name.toLowerCase();
    if (profileList().some((p, i) => i !== pe.index && String(p.name).trim().toLowerCase() === lower)) return `There’s already a profile called “${name}”`;
    return null;
  }

  function profileUnchanged(pe) {
    if (!pe.ref) return false;
    const p = profileList()[findProfileIndex(pe.ref)];
    return !!p && p.name === pe.name.trim() && JSON.stringify(p.apps || []) === JSON.stringify(pe.apps);
  }

  function refreshProfile() {
    const pe = profEd;
    if (!pe) return;
    const err = profileError(pe);
    pe.nameInput.classList.toggle('is-invalid', !!err && pe.touched);
    pe.errorEl.replaceChildren();
    if (err && pe.touched) pe.errorEl.append(h('div', { class: 'error-text' }, ico('warning', 14), err));
    pe.saveBtn.disabled = pe.busy || profileUnchanged(pe);
    pe.emptyNote.hidden = pe.apps.length > 0;
  }

  async function saveProfile() {
    const pe = profEd;
    if (!pe || pe.busy) return;
    commitAppInput();
    const err = profileError(pe);
    if (err) { pe.touched = true; refreshProfile(); pe.nameInput.focus(); return; }
    if (pe.saveBtn.disabled) { closeProfileEditor(); return; } // nothing changed
    const name = pe.name.trim();
    const config = clone(store.app.config);
    const list = M.profiles(config).slice();
    let idx;
    if (pe.ref) {
      idx = findProfileIndex(pe.ref);
      if (idx < 0) { toast({ kind: 'error', title: 'Profile not found', text: `“${pe.ref.name}” was removed in the meantime.` }); closeProfileEditor(); return; }
      list[idx] = Object.assign({}, list[idx], { name, apps: pe.apps.slice() });
    } else {
      list.push({ name, apps: pe.apps.slice(), binding: [] });
      idx = list.length - 1;
    }
    config.profile = list;
    pe.busy = true;
    pe.saveBtn.classList.add('is-busy');
    refreshProfile();
    try {
      const state = await call('save_config', { config }, { errorTitle: 'Couldn’t save the profile' });
      store.profile = { index: idx, name }; // show the new / renamed profile
      applyState(state);
      closeProfileEditor({ focusScope: true });
      toast(pe.ref
        ? { title: 'Profile saved', text: `${name} · ${pe.apps.length ? pe.apps.join(', ') : 'no apps yet'}` }
        : { title: `Created “${name}”`, text: 'Now click a gesture to give it a different job in these apps.' });
    } catch (_) {
      if (profEd === pe) { pe.busy = false; pe.saveBtn.classList.remove('is-busy'); refreshProfile(); }
    }
  }

  async function deleteProfile() {
    const pe = profEd;
    if (!pe || !pe.ref || pe.busy) return;
    const idx = findProfileIndex(pe.ref);
    const p = profileList()[idx];
    if (!p) { closeProfileEditor(); return; }
    const n = (p.binding || []).length;
    const ok = await confirmDialog({
      title: `Delete “${p.name}”?`,
      text: (n ? `Its ${n} changed gesture${n === 1 ? '' : 's'} will be removed. ` : '') + 'Those apps will use your All-apps gestures again.',
      confirm: 'Delete profile',
      danger: true,
    });
    if (!ok || profEd !== pe) return;
    const config = clone(store.app.config);
    config.profile = M.profiles(config).filter((_, i) => i !== idx);
    pe.busy = true;
    refreshProfile();
    try {
      const state = await call('save_config', { config }, { errorTitle: 'Couldn’t delete the profile' });
      store.profile = null;
      applyState(state);
      closeProfileEditor({ focusScope: true });
      toast({ title: `Deleted “${p.name}”` });
    } catch (_) {
      if (profEd === pe) { pe.busy = false; refreshProfile(); }
    }
  }

  function closeProfileEditor(opts) {
    const pe = profEd;
    if (!pe) return;
    if (pe.detect) clearInterval(pe.detect.timer); // a late detect_app result is ignored
    profEd = null;
    pe.drawer.close();
    const selected = $('.scope-btn[aria-checked="true"]');
    const target = (opts && opts.focusScope && selected) || (pe.prevFocus && pe.prevFocus.isConnected ? pe.prevFocus : selected);
    if (target && target.focus) target.focus();
  }

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
  // Canvases that rest while nothing is on the pad; a new frame wakes them.
  const frameWakers = new Set();

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
    new ResizeObserver(() => { liveUI.sizeDirty = true; liveView.kick(); }).observe(liveUI.pad);
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
    if (tour.open && tour.feel) feelFingers(f.contacts.length); // event-driven, not tied to animation frames
    for (const c of f.contacts) {
      let t = live.trails.get(c.id);
      if (!t) live.trails.set(c.id, (t = []));
      t.push({ x: c.x, y: c.y, t: now });
      if (t.length > 40) t.shift();
    }
    if (liveUI.mm && (f.width_mm !== liveUI.mm.w || f.height_mm !== liveUI.mm.h) && store.page === 'live') renderLive();
    frameWakers.forEach((wake) => wake());
  }

  function onGesture(p) {
    tourGesture(p);
    const g = p.gesture || {};
    const now = new Date();
    const top = store.recent[0];
    const repeating = (g.kind === 'swipe' || g.kind === 'pinch') && g.step > 0;
    let entry;
    if (repeating && top && top.trigger === p.trigger && top.kind === g.kind) {
      // Repeat steps of one swipe/pinch collapse into a single "×N" row.
      top.count++; top.action = p.action || top.action; top.profile = p.profile || top.profile; top.time = now;
      entry = top;
    } else if (g.kind === 'drag_end') {
      const d = store.recent.find((r) => r.kind === 'drag_start' && !r.ended);
      if (d) { d.ended = true; d.duration = now - d.start; }
      entry = d;
    } else {
      entry = { id: ++store.seq, kind: g.kind, gesture: g, trigger: p.trigger, action: p.action, profile: p.profile || null, time: now, start: now, count: 1 };
      store.recent.unshift(entry);
      if (store.recent.length > 25) store.recent.length = 25;
    }
    if (store.page === 'live' && liveUI.built) {
      renderFeed();
      if (g.kind !== 'drag_end') flashGesture(p, entry);
      liveUI.stats.last.textContent = cap(M.gestureLabel(g));
    }
    fireCard(p.trigger, p.profile || null);
  }

  function gestureIcon(g) {
    switch (g.kind) {
      case 'swipe': return 'arrow-' + g.direction;
      case 'pinch': return g.direction === 'in' ? 'pinch-in' : 'pinch-out';
      case 'double_tap': return 'tap2';
      case 'triple_tap': return 'tap3';
      case 'hold': return 'hold';
      case 'drag_start': case 'drag_end': return 'drag';
      default: return 'tap';
    }
  }

  function flashGesture(p, entry) {
    const g = p.gesture;
    const key = `${p.trigger}`;
    const same = live.flashKey === key && liveUI.flash.classList.contains('is-on') && (g.kind === 'swipe' || g.kind === 'pinch') && g.step > 0;
    live.flashKey = key;
    if (!same) {
      liveUI.flashGesture.replaceChildren(ico(gestureIcon(g), 20), cap(M.gestureLabel(g)));
    }
    liveUI.flashAction.className = 'flash-action' + (p.action ? ' is-bound' : '');
    liveUI.flashAction.replaceChildren(p.action ? '→ ' + p.action : p.profile ? 'Off in this app' : 'No action bound');
    if (p.profile) liveUI.flashAction.append(profileBadge(p.profile));
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
      let actionText = r.action || (r.profile ? 'Off in this app' : 'No action bound');
      if (r.kind === 'drag_start' && r.ended) actionText += ` · ${(r.duration / 1000).toFixed(1)}s`;
      li.className = 'feed-item' + (r.action ? '' : ' is-unbound');
      li.replaceChildren(
        h('span', { class: 'feed-ico' }, ico(gestureIcon(r.gesture), 16)),
        h('span', { class: 'feed-main' },
          h('div', { class: 'feed-gesture' }, cap(M.gestureLabel(r.gesture))),
          h('div', { class: 'feed-action', title: r.profile ? `${actionText} (${r.profile})` : actionText },
            h('span', { class: 'feed-action-text' }, actionText), r.profile ? profileBadge(r.profile) : null)),
        h('span', { class: 'feed-meta' },
          h('span', { class: 'feed-time' }, time),
          r.count > 1 ? h('span', { class: 'badge badge--accent' }, '×' + r.count) : null));
      prev = li;
    }
  }

  /* Canvas renderer */

  /** Theme colours for canvas drawing (re-read when the theme changes). */
  function themeColors() {
    const cs = getComputedStyle(document.documentElement);
    const v = (n) => cs.getPropertyValue(n).trim();
    return { finger: v('--finger'), glow: v('--finger-glow'), text: v('--text'), text3: v('--text-3'), border: v('--border-strong'), surface: v('--surface-solid') };
  }

  const TRAIL_MS = 420;

  /**
   * Draw the current contacts — fading trails, a centroid web and glowing
   * fingertips — on a canvas sized to its CSS box. Shared by the Live page
   * and the tutorial's mini touchpad. Returns how many fingers are down.
   */
  function paintTouches(cv, mm, colors, opts) {
    const ctx = cv.getContext('2d');
    const dpr = window.devicePixelRatio || 1;
    const W = cv.clientWidth, H = cv.clientHeight;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, W, H);
    if (!mm || !W || !H || !colors) return 0;
    const sx = W / mm.w, sy = H / mm.h;
    const s = Math.min(sx, sy);
    const now = performance.now();

    // Drop trail points that have faded out.
    for (const [id, pts] of live.trails) {
      while (pts.length && now - pts[0].t > TRAIL_MS) pts.shift();
      if (!pts.length) live.trails.delete(id);
    }

    // Trails
    ctx.lineCap = 'round';
    ctx.strokeStyle = colors.finger;
    for (const pts of live.trails.values()) {
      for (let i = 1; i < pts.length; i++) {
        const a = pts[i - 1], b = pts[i];
        const age = 1 - (now - b.t) / TRAIL_MS;
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
    const r = Math.max(opts && opts.small ? 6 : 9, s * 4.6);
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

      if (opts && opts.labels) {
        ctx.fillStyle = colors.text3;
        ctx.font = '600 10px ' + getComputedStyle(document.body).fontFamily;
        ctx.textAlign = 'left';
        ctx.fillText('#' + c.id, x + r + 5, y - r * 0.6);
      }
    }
    return contacts.length;
  }

  const liveView = (function () {
    let raf = 0;
    let running = false;
    let colors = null;
    let lastCount = -1;
    let lastRateUpdate = 0;

    function readColors() { colors = themeColors(); kick(); }

    // Draw again if the canvas is resting (see the end of draw()).
    function kick() { if (running && !raf) raf = requestAnimationFrame(draw); }

    function start() {
      if (running) return;
      running = true;
      readColors();
      liveUI.sizeDirty = true;
      wantLive('page', true);
      frameWakers.add(kick);
      kick();
    }
    function stop() {
      if (!running) return;
      running = false;
      cancelAnimationFrame(raf);
      raf = 0;
      frameWakers.delete(kick);
      wantLive('page', false);
    }
    // Hidden window: stop drawing (syncLive() pauses the stream itself).
    function pause() { cancelAnimationFrame(raf); raf = 0; }
    function resume() { kick(); }

    function draw() {
      raf = 0;
      if (!running || !liveUI.built || liveUI.main.hidden) return;
      const cv = liveUI.canvas;
      const dpr = window.devicePixelRatio || 1;
      if (liveUI.sizeDirty) {
        liveUI.sizeDirty = false;
        cv.width = Math.round(cv.clientWidth * dpr);
        cv.height = Math.round(cv.clientHeight * dpr);
        if (liveUI.mm) liveUI.pad.style.setProperty('--grid', (cv.clientWidth / liveUI.mm.w) * 5 + 'px');
      }
      const count = paintTouches(cv, liveUI.mm, colors, { labels: true });
      const now = performance.now();

      // Readouts (touch the DOM only when values change)
      if (count !== lastCount) {
        lastCount = count;
        liveUI.stats.fingers.textContent = String(count);
        liveUI.stats.fingers.classList.toggle('is-active', count >= 3);
        liveUI.pad.classList.toggle('has-touch', count > 0 || live.trails.size > 0 || store.recent.length > 0);
      }
      if (now - lastRateUpdate > 500) {
        lastRateUpdate = now;
        while (live.frameTimes.length && now - live.frameTimes[0] > 1000) live.frameTimes.shift();
        liveUI.stats.rate.firstChild.textContent = String(live.frameTimes.length);
      }
      // Keep drawing while fingers are down or trails fade; otherwise rest
      // until the next frame arrives, so an idle Live page costs no CPU.
      if (count > 0 || live.trails.size > 0) raf = requestAnimationFrame(draw);
      else liveUI.stats.rate.firstChild.textContent = '0';
    }

    return { start, stop, pause, resume, readColors, kick };
  })();

  /*
   * set_live is reference-counted: the Live page and the tutorial's mini
   * touchpad both want frames, and a hidden window wants none. The backend
   * also gets a `false` at start-up, in case a previous window (destroyed on
   * close) left the stream on.
   */
  const liveWanted = new Set();
  let liveSent = null;
  function wantLive(key, on) {
    if (on) liveWanted.add(key); else liveWanted.delete(key);
    syncLive();
  }
  function syncLive() {
    const want = liveWanted.size > 0 && !document.hidden;
    if (want === liveSent) return;
    liveSent = want;
    call('set_live', { live: want }, { quiet: true }).catch(() => { liveSent = null; });
  }

  /* =====================================================================
     10. Settings page
     ===================================================================== */

  const SLIDERS = [
    { group: 'recog', key: 'tap_max_ms', name: 'Tap speed', min: 100, max: 400, step: 10, fmt: (v) => [v, 'ms'], help: 'Fingers must lift within this time to count as a tap. Lower feels snappier and avoids accidental taps.' },
    { group: 'recog', key: 'tap_slop_mm', name: 'Tap tolerance', min: 1, max: 10, step: 0.5, fmt: (v) => [Number(v).toFixed(1), 'mm'], help: 'How far fingers may drift during a tap before it stops being one.' },
    { group: 'recog', key: 'multi_tap_ms', name: 'Double-tap speed', min: 150, max: 600, step: 10, fmt: (v) => [v, 'ms'], help: 'The longest gap between taps that still counts as a double or triple tap. Single taps wait this long when a double or triple tap is bound.' },
    { group: 'recog', key: 'swipe_threshold_mm', name: 'Swipe distance', min: 4, max: 30, step: 1, fmt: (v) => [Number(v).toFixed(0), 'mm'], help: 'Travel needed before a swipe — or a drag — is recognised.' },
    { group: 'recog', key: 'swipe_step_mm', name: 'Repeat spacing', min: 3, max: 20, step: 0.5, fmt: (v) => [Number(v).toFixed(1), 'mm'], help: 'Extra travel (or pinch) between repeats for gestures that keep firing, like a volume scrub or zoom.' },
    { group: 'recog', key: 'pinch_threshold_mm', name: 'Pinch distance', min: 3, max: 20, step: 0.5, fmt: (v) => [Number(v).toFixed(1), 'mm'], help: 'How much your fingers must spread apart or close together before a pinch is recognised.' },
    { group: 'recog', key: 'hold_ms', name: 'Hold time', min: 300, max: 1500, step: 50, fmt: (v) => [v, 'ms'], help: 'How long fingers rest without moving before it counts as a hold.' },
    { group: 'drag', key: 'drag_speed', name: 'Drag speed', min: 2, max: 25, step: 0.5, fmt: (v) => [Number(v).toFixed(1), 'px/mm'], help: 'Cursor pixels per millimetre of finger travel while dragging.' },
    { group: 'drag', key: 'drag_acceleration', name: 'Drag acceleration', min: 0, max: 3, step: 0.1, fmt: (v) => (Number(v) === 0 ? ['Linear', ''] : [Number(v).toFixed(1), '×']), help: 'How much faster quick flicks move the cursor. 0 keeps movement linear.' },
    { group: 'drag', key: 'drag_release_ms', name: 'Drag release delay', min: 0, max: 1000, step: 25, fmt: (v) => (Number(v) === 0 ? ['Instant', ''] : [v, 'ms']), help: 'Keeps the button held briefly after lifting, so you can reposition your fingers and continue the drag.' },
  ];
  const INT_KEYS = new Set(['tap_max_ms', 'multi_tap_ms', 'hold_ms', 'drag_release_ms']);
  // Fallbacks if the backend's settings predate a field (same values as pady-core's defaults).
  const SETTING_DEFAULTS = {
    tap_max_ms: 220, tap_slop_mm: 4, multi_tap_ms: 300, swipe_threshold_mm: 10, swipe_step_mm: 7, pinch_threshold_mm: 8,
    hold_ms: 600, drag_speed: 9, drag_acceleration: 1, drag_release_ms: 350, show_overlay: false, pause_in_fullscreen: true, check_updates: true,
  };
  // Starter-set icons by preset id (unknown ids get a generic one).
  const PRESET_ICONS = { recommended: 'sparkle', macos: 'command', media: 'play-pause', productivity: 'taskview', minimal: 'minimal' };
  const presetIcon = (id) => PRESET_ICONS[id] || 'layers';

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

    /* Boolean settings saved right away, together with any slider change
       still waiting for its debounce. */
    setUI.toggles = {};
    const settingToggle = (key, title, sub) => {
      const input = h('input', { type: 'checkbox', role: 'switch', 'aria-labelledby': `${key}-title`, 'aria-describedby': `${key}-sub` });
      input.addEventListener('change', () => {
        setUI.pending[key] = input.checked;
        clearTimeout(setUI.timer);
        flushSettings();
      });
      setUI.toggles[key] = input;
      return h('label', { class: 'row', style: { cursor: 'pointer' }, 'data-setting': key },
        h('div', { class: 'row-text' },
          h('div', { class: 'row-title', id: `${key}-title` }, title),
          h('div', { class: 'row-sub', id: `${key}-sub` }, sub)),
        h('span', { class: 'switch' }, input, h('span', { class: 'switch-track' }, h('span', { class: 'switch-thumb' }))));
    };
    setUI.presets = h('div', { class: 'preset-list' });

    page.append(
      setUI.banners,
      h('div', { class: 'settings-grid' },
        h('div', { class: 'settings-col' },
          panel('gestures', 'Recognition', 'Timing and distances for taps, holds, swipes and pinches', SLIDERS.filter((d) => d.group === 'recog').map(sliderEl)),
          setUI.presetsPanel = panel('sparkle', 'Starter sets', 'Swap all your All-apps gestures for a ready-made set', setUI.presets)),
        h('div', { class: 'settings-col' },
          panel('drag', 'Drag', 'How multi-finger drag moves the pointer', SLIDERS.filter((d) => d.group === 'drag').map(sliderEl)),
          panel('power', 'General', 'Start-up, feedback and configuration',
            h('label', { class: 'row', style: { cursor: 'pointer' } },
              h('div', { class: 'row-text' }, setUI.autostartTitle, h('div', { class: 'row-sub' }, 'Runs quietly in the tray after you sign in.')),
              h('span', { class: 'switch' }, setUI.autostart, h('span', { class: 'switch-track' }, h('span', { class: 'switch-thumb' })))),
            settingToggle('show_overlay', 'Show Pady\'s level pill', 'Pady\'s own volume & brightness pill near the bottom of the screen. On Windows, volume always shows Windows\' own slider.'),
            settingToggle('check_updates', 'Check for updates', 'Looks for a new version on GitHub twice a day and lets you install it with one click.'),
            setUI.pauseRow = settingToggle('pause_in_fullscreen', 'Pause in full-screen games', 'Pady ignores gestures while a game runs in exclusive full screen.'),
            h('div', { class: 'row' },
              h('div', { class: 'row-text' }, h('div', { class: 'row-title' }, 'Tutorial'), h('div', { class: 'row-sub' }, 'A quick, hands-on tour of the basics.')),
              h('button', { class: 'btn btn--sm', type: 'button', onClick: () => openTutorial({ firstRun: false }) }, ico('graduation', 14), 'Show tutorial')),
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
      const v = cfg[d.key] ?? SETTING_DEFAULTS[d.key];
      setUI.inputs[d.key].input.value = v;
      paintSlider(d, v);
    }
    for (const [key, input] of Object.entries(setUI.toggles)) {
      if (!(key in setUI.pending)) input.checked = (cfg[key] ?? SETTING_DEFAULTS[key]) !== false;
    }
    setUI.pauseRow.hidden = platform() !== 'windows'; // exclusive full screen is a Windows thing
    renderPresetList();
    setUI.autostart.checked = !!s.autostart;
    setUI.autostartTitle.textContent = `Start with ${osName()}`;
    setUI.path.textContent = s.config_path || '';

    setUI.banners.replaceChildren(...(s.conflicts || []).map((c) => h('div', { class: 'banner' }, ico('warning', 18),
      h('div', { class: 'banner-body' }, h('span', { class: 'banner-title' }, c.message), h('span', { class: 'banner-text' }, c.fix)),
      c.can_open_settings || c.can_fix
        ? h('div', { class: 'banner-actions' },
          c.can_fix ? fixPermissionsButton() : null,
          c.can_open_settings ? h('button', { class: 'btn btn--sm', type: 'button', onClick: openTouchpadSettings }, ico('external', 14), 'Open touchpad settings') : null)
        : null)));

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

  /** Starter sets from AppState.presets (the panel hides if the backend has none). */
  function renderPresetList() {
    const presets = store.app.presets || [];
    setUI.presetsPanel.hidden = !presets.length;
    setUI.presets.replaceChildren(...presets.map((p) => h('div', { class: 'row preset-row' },
      h('span', { class: 'panel-icon' }, ico(presetIcon(p.id), 16)),
      h('div', { class: 'row-text' }, h('div', { class: 'row-title' }, p.name), h('div', { class: 'row-sub' }, p.description)),
      h('button', { class: 'btn btn--sm', type: 'button', 'aria-label': `Use the ${p.name} starter set`, onClick: (e) => usePreset(p, e.currentTarget) }, 'Use'))));
  }

  async function usePreset(p, btn) {
    const ok = await confirmDialog({
      title: `Use “${p.name}”?`,
      text: 'Your All-apps gestures will be replaced with this set. Settings and per-app profiles stay as they are.',
      confirm: `Use ${p.name}`,
    });
    if (!ok) return;
    btn.disabled = true;
    btn.classList.add('is-busy');
    try {
      applyState(await call('apply_preset', { id: p.id }, { errorTitle: 'Couldn’t switch starter sets' }));
      toast({ title: `Now using “${p.name}”`, text: 'Your gestures are on the Gestures page.' });
    } catch (_) { /* toast shown */ } finally {
      btn.disabled = false;
      btn.classList.remove('is-busy');
    }
  }

  function flushPendingSettings() {
    if (!Object.keys(setUI.pending).length) return;
    clearTimeout(setUI.timer);
    flushSettings();
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

  /** A copy of the sidebar logo at `size` px, with its own gradient id. */
  function brandLogo(size, gradId) {
    const logo = $('.brand-mark svg').cloneNode(true);
    logo.setAttribute('width', String(size));
    logo.setAttribute('height', String(size));
    $$('[id]', logo).forEach((n) => n.removeAttribute('id'));
    const grad = logo.querySelector('linearGradient');
    if (grad) { grad.id = gradId; logo.querySelector('rect').setAttribute('fill', `url(#${gradId})`); }
    return logo;
  }

  function renderAbout() {
    const page = $('#page-about');
    const s = store.app;
    const linux = platform() === 'linux';
    const logo = brandLogo(72, 'brand-g-about');

    const step = (iconName, title, text) => h('div', { class: 'flow-step' }, h('span', { class: 'panel-icon' }, ico(iconName, 16)), h('h4', null, title), h('p', null, text));
    const arrow = () => h('div', { class: 'flow-arrow', 'aria-hidden': 'true' }, ico('arrow-right', 18));
    const tip = (strong, rest) => h('li', null, ico('sparkle', 14), h('span', null, h('strong', null, strong), ' ', rest));

    page.replaceChildren(
      h('div', { class: 'about-hero' },
        h('span', { class: 'logo-lg' }, logo),
        h('div', null,
          h('h2', null, 'Pady', h('span', { class: 'badge badge--accent' }, 'v' + s.version)),
          h('p', null, 'Pady turns multi-finger touchpad gestures into actions: scrub the volume with a four-finger slide, play or pause with a three-finger tap, pinch to zoom, drag windows without pressing, or jump to an app or website with a single touch — and per-app profiles let the same gesture do something different in, say, your browser. It runs quietly in the tray and reads your touchpad directly, so it works in every app.'),
          h('div', { class: 'about-links' },
            h('button', { class: 'btn btn--sm btn--primary', type: 'button', onClick: () => openTutorial({ firstRun: false }) }, ico('graduation', 14), 'Show the tutorial'),
            h('span', { class: 'link-text selectable' }, ico('github', 14), 'github.com/lumidren/Pady'),
            h('span', { class: 'link-text' }, ico(linux ? 'terminal' : 'desktop', 14), linux ? 'Linux' : 'Windows')))),
      updatesPanel(),
      feedbackPanel(),
      h('section', { class: 'panel', style: { 'margin-bottom': '16px' } },
        h('div', { class: 'panel-head' }, h('div', null, h('div', { class: 'panel-title' }, 'How it works'), h('div', { class: 'panel-sub' }, 'From fingertips to actions in a few milliseconds'))),
        h('div', { class: 'flow' },
          step('touchpad', 'Raw touch input', linux ? 'Contacts are read from the touchpad via evdev, in millimetres.' : 'Contacts are read from the Precision Touchpad via Raw Input, in millimetres.'),
          arrow(),
          step('gestures', 'Gesture engine', 'Finger count, travel, spread and timing become taps, holds, swipes, pinches or drags — tuned in Settings.'),
          arrow(),
          step('zap', 'Your actions', 'The binding for the app in front runs: keys, volume, media, apps, links or commands.'))),
      h('section', { class: 'panel' },
        h('div', { class: 'panel-head' }, h('div', null, h('div', { class: 'panel-title' }, 'Tips'))),
        h('ul', { class: 'tips' },
          linux
            ? tip('Avoid double actions.', 'Desktop environments like GNOME use 3- and 4-finger swipes; disable them or bind other finger counts in Pady.')
            : tip('Avoid double actions.', 'In Settings → Bluetooth & devices → Touchpad, set three- and four-finger gestures to “Nothing”.'),
          tip('Scrub, don’t repeat.', 'Turn on “Keep firing while sliding” for volume or brightness, then slide further for bigger changes. Pinches work the same way for zoom.'),
          tip('Tap twice, or three times.', 'Double and triple taps give each finger count more actions. Single taps then wait a beat; set how long with Double-tap speed in Settings.'),
          tip('One gesture, many jobs.', 'Add a profile on the Gestures page — e.g. make four-finger swipes switch tabs only while your browser is in front.'),
          tip('Drag claims swipes.', 'When 3-finger drag is on, 3-finger swipes are ignored — use 4 fingers for swipes.'),
          tip('Watch it think.', 'The Live page shows every contact and gesture, which makes tuning Settings much easier.'),
          tip('Plain text config.', 'Everything is stored in config.toml; edits made by hand apply instantly.'))),
      makerLine());
  }

  /* =====================================================================
     11b. Tutorial — full-window first-run wizard
     ---------------------------------------------------------------------
     Shown at start-up while settings.onboarded is false; finishing or
     skipping saves onboarded = true. "Show the tutorial" (About, Settings)
     reopens it without touching the flag.
     ===================================================================== */

  const tour = { open: false };

  /** The steps; "Starter set" only appears when the backend offers presets. */
  function tourSteps() {
    const steps = [
      { id: 'welcome', name: 'Welcome' },
      { id: 'touchpad', name: 'Your touchpad' },
      { id: 'os', name: platform() === 'linux' ? 'Your desktop' : 'Windows gestures' },
    ];
    if ((store.app.presets || []).length) steps.push({ id: 'preset', name: 'Starter set' });
    steps.push({ id: 'practice', name: 'Try it' }, { id: 'done', name: 'Done' });
    return steps;
  }

  /** OS gesture clashes (permission problems are Fix permissions' job, not this step's). */
  const osConflicts = () => (store.app.conflicts || []).filter((c) => !c.can_fix);

  function openTutorial(opts) {
    if (tour.open || !store.app) return;
    if (editor) closeEditor();
    if (profEd) closeProfileEditor();
    const firstRun = !!(opts && opts.firstRun);
    Object.assign(tour, {
      open: true, firstRun, steps: tourSteps(), index: 0, visited: 0, dir: 1,
      prevFocus: document.activeElement,
      // First run starts from the defaults, so "Recommended" is the honest preselection;
      // a reopened tour starts on "Keep my gestures" so nothing is replaced by accident.
      picked: firstRun ? 'recommended' : 'keep', applied: null, applying: null, confirmed: firstRun,
      tasks: null, pad: null, poll: 0, cur: null, osSig: '',
    });
    tour.live = h('div', { class: 'sr-only', 'aria-live': 'polite' });
    tour.progress = h('ol', { class: 'tour-progress' });
    tour.stage = h('div', { class: 'tour-stage' });
    tour.backBtn = h('button', { class: 'btn btn--ghost', type: 'button', onClick: () => goTour(tour.index - 1) }, ico('arrow-left', 15), 'Back');
    tour.nextBtn = h('button', { class: 'btn btn--primary tour-next', type: 'button', onClick: tourNext });
    tour.count = h('span', { class: 'tour-count' });
    tour.el = h('div', { class: 'tour', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': 'tour-title' },
      h('div', { class: 'tour-glow', 'aria-hidden': 'true' }),
      h('header', { class: 'tour-top' },
        h('div', { class: 'tour-brand' }, brandLogo(26, 'brand-g-tour'), h('span', { class: 'tour-brand-word' }, 'Pady'),
          h('span', { class: 'badge badge--accent' }, firstRun ? 'Setup' : 'Tutorial')),
        h('nav', { class: 'tour-nav', 'aria-label': 'Tutorial steps' }, tour.progress),
        h('button', { class: 'btn btn--ghost btn--sm tour-skip', type: 'button', onClick: () => closeTutorial(false) }, firstRun ? 'Skip setup' : 'Close')),
      tour.stage,
      h('footer', { class: 'tour-foot' }, tour.backBtn, tour.count, tour.nextBtn),
      tour.live);
    tour.el.addEventListener('keydown', (e) => {
      if (e.key === 'Tab') trapTab(e, tour.el);
      else if (e.key === 'Escape' && !$('.modal')) { e.preventDefault(); closeTutorial(false); }
    });
    $('#overlay-root').append(tour.el);
    setInert(true);
    renderTour();
  }

  function goTour(i) {
    if (!tour.open || i < 0 || i >= tour.steps.length || i === tour.index) return;
    tour.dir = i > tour.index ? 1 : -1;
    tour.index = i;
    renderTour();
  }

  function tourNext() {
    if (tour.index === tour.steps.length - 1) closeTutorial(true);
    else goTour(tour.index + 1);
  }

  function renderTour() {
    leaveTourStep();
    const step = tour.steps[tour.index];
    tour.visited = Math.max(tour.visited, tour.index);
    const view = TOUR[step.id];
    tour.cur = view;
    tour.stage.replaceChildren(h('section', { class: 'tour-step' + (tour.dir < 0 ? ' is-back' : ''), 'data-step': step.id }, view.render()));
    tour.stage.scrollTop = 0;
    tour.backBtn.disabled = tour.index === 0;
    tour.backBtn.classList.toggle('is-invisible', tour.index === 0);
    tour.count.textContent = `${tour.index + 1} of ${tour.steps.length}`;
    renderTourProgress();
    if (view.enter) view.enter();
    refreshTourChrome();
    tour.live.textContent = `Step ${tour.index + 1} of ${tour.steps.length}: ${step.name}`;
    ((view.focus && view.focus()) || tour.nextBtn).focus({ preventScroll: true });
  }

  function leaveTourStep() {
    clearInterval(tour.poll);
    tour.poll = 0;
    if (tour.pad) { tour.pad.stop(); tour.pad = null; }
    tour.feel = null;
    tour.cur = null;
  }

  /** Progress dots; visited steps can be revisited by clicking them. */
  function renderTourProgress() {
    tour.progress.replaceChildren(...tour.steps.map((s, i) => h('li', {
      class: 'tour-dot ' + (i === tour.index ? 'is-current' : i < tour.index ? 'is-done' : 'is-todo'), 'data-step': s.id,
    }, h('button', {
      type: 'button', class: 'tour-dot-btn', disabled: i > tour.visited, title: s.name,
      'aria-current': i === tour.index ? 'step' : null,
      'aria-label': `Step ${i + 1}: ${s.name}`,
      onClick: () => goTour(i),
    }, h('span', { class: 'tour-dot-label' }, s.name)))));
  }

  /** Bits of chrome that depend on live state: the OS step's dot and the Next label. */
  function refreshTourChrome() {
    if (!tour.open) return;
    const step = tour.steps[tour.index];
    const osDot = $('.tour-dot[data-step="os"]', tour.progress);
    if (osDot) osDot.classList.toggle('is-ok', !osConflicts().length && tour.visited >= tour.steps.findIndex((s) => s.id === 'os'));
    let label = 'Next', iconName = 'arrow-right';
    if (step.id === 'welcome') label = 'Let’s set it up';
    else if (step.id === 'os' && osConflicts().length) label = 'Skip for now';
    else if (step.id === 'practice' && tour.tasks && tour.tasks.some((t) => !t.done)) label = 'Skip for now';
    else if (step.id === 'done') { label = 'Start using Pady'; iconName = 'check'; }
    if (tour.nextBtn.dataset.label !== label) {
      tour.nextBtn.dataset.label = label;
      tour.nextBtn.replaceChildren(label, ico(iconName, 15));
    }
  }

  /** applyState() hook: let the open step react to new state (devices, conflicts). */
  function tourOnState() {
    if (!tour.open) return;
    if (tour.cur && tour.cur.update) tour.cur.update();
    refreshTourChrome();
  }

  /** Re-read the state every 2 s while a step waits on the OS (quietly; errors just retry). */
  function startTourPoll() {
    clearInterval(tour.poll);
    tour.poll = setInterval(() => {
      if (!document.hidden) api.invoke('get_state').then(applyState, () => {});
    }, 2000);
  }

  async function closeTutorial(finished) {
    if (!tour.open) return;
    leaveTourStep();
    tour.open = false;
    const { el, firstRun, prevFocus } = tour;
    el.classList.add('is-closing');
    setTimeout(() => el.remove(), reducedMotion() ? 0 : 280);
    setInert(false);
    const target = prevFocus && prevFocus.isConnected && prevFocus !== document.body ? prevFocus : $('.nav-item[aria-current="page"]');
    if (target && target.focus) target.focus({ preventScroll: true });
    if (finished) toast({ title: 'You’re all set', text: 'Your gestures work everywhere, even with this window closed.' });
    if (firstRun) {
      try {
        const config = clone(store.app.config);
        config.settings.onboarded = true;
        applyState(await call('save_config', { config }, { errorTitle: 'Couldn’t save that the tutorial is done' }));
      } catch (_) { /* toast shown; the tutorial simply shows again next time */ }
    }
  }

  /* ---------- Step views: render() → content, enter()/update() optional ---------- */

  const TOUR = {
    welcome: {
      render() {
        return [
          heroEl(),
          h('h1', { class: 'tour-title', id: 'tour-title' }, tour.firstRun ? 'Welcome to Pady' : 'Pady in a minute'),
          h('p', { class: 'tour-lead' }, 'Pady turns three-, four- and five-finger touchpad gestures into the things you do all day — media keys, volume, shortcuts, apps and more.'),
          h('p', { class: 'tour-note' }, ico('hold', 14), 'Takes about a minute, and you can change everything later.'),
        ];
      },
    },

    touchpad: {
      render() {
        const s = store.app;
        const dev = s.devices && s.devices[0];
        tour.hadDevice = !!dev;
        if (dev) {
          tour.felt = 0;
          tour.feel = h('div', { class: 'tour-feel', role: 'status' }, ico('gestures', 15), 'Rest three fingers on your touchpad to say hello.');
          tour.pad = createMiniPad({});
          return [
            h('h1', { class: 'tour-title', id: 'tour-title' }, 'Found your touchpad'),
            h('p', { class: 'tour-lead' }, 'Pady reads it directly, so your gestures work in every app.'),
            h('div', { class: 'tour-device' },
              h('span', { class: 'tour-device-icon' }, ico('touchpad', 20)),
              h('div', { class: 'tour-device-text' },
                h('div', { class: 'tour-device-name' }, dev.name),
                h('div', { class: 'tour-device-sub' }, `${fmtNum(dev.width_mm, 0)} × ${fmtNum(dev.height_mm, 0)} mm · up to ${dev.max_contacts} fingers`)),
              h('span', { class: 'badge tour-ok-badge' }, ico('check', 12), 'Ready')),
            tour.pad.el,
            tour.feel,
          ];
        }
        const canFix = (s.conflicts || []).some((c) => c.can_fix);
        return [
          h('h1', { class: 'tour-title', id: 'tour-title' }, 'Let’s find your touchpad'),
          h('p', { class: 'tour-lead' }, 'Pady can’t read a touchpad yet. That’s usually quick to fix.'),
          h('div', { class: 'tour-alert' }, ico('warning', 18), h('div', { class: 'tour-alert-body' },
            h('div', { class: 'tour-alert-title' }, 'No touchpad detected'),
            h('div', { class: 'tour-alert-text selectable' }, s.input_error || 'Make sure your touchpad is switched on and uses a precision driver.'),
            h('div', { class: 'tour-actions' },
              canFix ? fixPermissionsButton() : null,
              platform() !== 'linux' ? h('button', { class: 'btn btn--sm', type: 'button', onClick: openTouchpadSettings }, ico('external', 14), 'Open touchpad settings') : null,
              h('button', { class: 'btn btn--sm btn--ghost', type: 'button', onClick: () => api.invoke('get_state').then(applyState, () => {}) }, ico('reset', 14), 'Check again')))),
          h('p', { class: 'tour-note' }, ico('info', 14), 'You can keep going and fix this later; Pady keeps checking.'),
        ];
      },
      enter() { if (tour.pad) tour.pad.start(); else startTourPoll(); },
      update() { if (!!(store.app.devices || []).length !== tour.hadDevice) renderTour(); },
    },

    os: {
      render() {
        const linux = platform() === 'linux';
        tour.osSig = '';
        tour.osStatus = h('div', { class: 'tour-status', role: 'status' });
        const body = [
          h('h1', { class: 'tour-title', id: 'tour-title' }, linux ? 'Check your desktop’s gestures' : 'Stop Windows reacting too'),
          h('p', { class: 'tour-lead' }, linux
            ? 'Some desktops have touchpad gestures of their own. If both react, one swipe does two things.'
            : 'Windows has its own three- and four-finger gestures. If both react, one swipe does two things.'),
        ];
        if (!linux) {
          body.push(h('ol', { class: 'tour-steps' },
            h('li', null, h('span', null, 'Open ', h('strong', null, 'Touchpad settings'), '.'),
              h('button', { class: 'btn btn--sm', type: 'button', onClick: openTouchpadSettings }, ico('external', 14), 'Open touchpad settings')),
            h('li', null, h('span', null, 'Expand ', h('strong', null, 'Three-finger gestures'), ' and set ', h('strong', null, 'Swipes'), ' and ', h('strong', null, 'Taps'), ' to ', h('strong', null, '“Nothing”'), '.')),
            h('li', null, h('span', null, 'Do the same under ', h('strong', null, 'Four-finger gestures'), '.'))));
        }
        body.push(tour.osStatus);
        return body;
      },
      enter() { this.update(); startTourPoll(); },
      update() {
        const linux = platform() === 'linux';
        const list = osConflicts();
        const sig = JSON.stringify(list);
        if (sig === tour.osSig) return; // unchanged: keep focus and animations where they are
        const wasWaiting = tour.osSig !== '' && tour.osSig !== '[]';
        tour.osSig = sig;
        const ok = !list.length;
        tour.osStatus.className = 'tour-status ' + (ok ? 'is-ok' : 'is-wait') + (ok && wasWaiting ? ' is-fresh' : '');
        if (ok) {
          tour.osStatus.replaceChildren(
            h('span', { class: 'tour-status-icon' }, ico('check', 16)),
            h('div', null,
              h('div', { class: 'tour-status-title' }, 'All clear'),
              h('div', { class: 'tour-status-text' }, linux ? 'Your desktop won’t get in the way of your gestures.' : 'Windows will leave these gestures to Pady.')));
          if (wasWaiting) tour.live.textContent = 'All clear.';
        } else {
          tour.osStatus.replaceChildren(
            h('span', { class: 'tour-status-icon' }, h('span', { class: 'tour-pulse' })),
            h('div', { class: 'tour-status-body' },
              h('div', { class: 'tour-status-title' }, linux ? 'Worth a look' : 'Waiting for the change…'),
              h('ul', { class: 'tour-status-list' }, list.map((c) => h('li', null,
                h('span', null, c.message),
                linux ? h('span', { class: 'tour-status-fix' }, c.fix) : null,
                linux && c.can_open_settings ? h('button', { class: 'btn btn--sm', type: 'button', onClick: openTouchpadSettings }, ico('external', 14), 'Open settings') : null))),
              h('div', { class: 'tour-status-text' }, linux
                ? 'You can skip this and come back to it later.'
                : 'Pady checks again every 2 seconds. You can also skip this and do it later.')));
        }
      },
    },

    preset: {
      render() {
        const options = (tour.firstRun ? [] : [{ id: 'keep', name: 'Keep my gestures', description: 'Change nothing and carry on with what you have now.' }])
          .concat(store.app.presets || []);
        tour.presetGroup = h('div', { class: 'tour-presets', role: 'radiogroup', 'aria-label': 'Starter sets' });
        options.forEach((p) => tour.presetGroup.append(h('button', {
          class: 'tour-preset', type: 'button', role: 'radio', 'data-id': p.id, 'aria-checked': String(tour.picked === p.id),
          onClick: () => pickTourPreset(p.id),
        },
        h('span', { class: 'tour-preset-icon' }, ico(p.id === 'keep' ? 'layers' : presetIcon(p.id), 18)),
        h('span', { class: 'tour-preset-name' }, p.name, p.id === 'recommended' ? h('span', { class: 'badge badge--accent' }, 'Best start') : null),
        h('span', { class: 'tour-preset-desc' }, p.description),
        h('span', { class: 'tour-preset-check', 'aria-hidden': 'true' }, ico('check', 13)))));
        // Arrow keys only move focus here; Enter / Space / click apply (applying replaces bindings).
        roving(tour.presetGroup, '.tour-preset', () => {});
        tour.presetStatus = h('p', { class: 'tour-note', role: 'status' }, ico('info', 14), tour.firstRun
          ? 'Recommended is what you have now. Pick another set to switch — you can fine-tune any gesture later.'
          : 'Picking a set replaces your All-apps gestures. Settings and per-app profiles stay as they are.');
        return [
          h('h1', { class: 'tour-title', id: 'tour-title' }, 'Pick a starter set'),
          h('p', { class: 'tour-lead' }, 'Choose how your gestures start out.'),
          tour.presetGroup,
          tour.presetStatus,
        ];
      },
      focus() { return $('.tour-preset[aria-checked="true"]', tour.presetGroup) || $('.tour-preset', tour.presetGroup); },
    },

    practice: {
      render() {
        tour.tasks = M.practiceTasks(store.app.config).map((t) => Object.assign({ done: false }, t));
        const hasDevice = !!(store.app.devices || []).length;
        tour.pad = hasDevice ? createMiniPad({}) : null;
        tour.taskList = h('ol', { class: 'tour-tasks' }, tour.tasks.map(taskEl));
        tour.taskCount = h('div', { class: 'tour-task-count' });
        tour.celebrate = h('div', { class: 'tour-celebrate', hidden: true },
          h('span', { class: 'tour-sparks', 'aria-hidden': 'true' }, Array.from({ length: 8 }, () => h('i'))),
          ico('sparkle', 16), 'Nice — you’ve got the hang of it!');
        return [
          h('h1', { class: 'tour-title', id: 'tour-title' }, 'Try it out'),
          h('p', { class: 'tour-lead' }, 'Do each gesture on your touchpad. They tick off as Pady recognises them.'),
          h('div', { class: 'tour-try' },
            tour.pad ? tour.pad.el : h('div', { class: 'tour-alert' }, ico('warning', 18), h('div', null, 'No touchpad detected, so there’s nothing to practise on yet. You can skip this step.')),
            h('div', { class: 'tour-try-side' }, tour.taskList, tour.taskCount, tour.celebrate)),
          h('p', { class: 'tour-note tour-note--warn' }, ico('zap', 14), 'These really run: music may start or the volume may change.'),
        ];
      },
      enter() { if (tour.pad) tour.pad.start(); updateTaskCount(); },
    },

    done: {
      render() {
        const linux = platform() === 'linux';
        const tip = (iconName, title, text) => h('div', { class: 'tour-tip' },
          h('span', { class: 'panel-icon' }, ico(iconName, 16)),
          h('div', null, h('div', { class: 'tour-tip-title' }, title), h('div', { class: 'tour-tip-text' }, text)));
        return [
          h('div', { class: 'tour-done-mark', 'aria-hidden': 'true' }, frag(DONE_SVG)),
          h('h1', { class: 'tour-title', id: 'tour-title' }, 'You’re all set'),
          h('p', { class: 'tour-lead' }, 'Pady now works quietly in the background. A few things to try next:'),
          h('div', { class: 'tour-tips' },
            tip('live', 'Watch it think', 'The Live page shows every finger and gesture, which makes tuning timing easy.'),
            tip('profile', 'Make it per-app', 'Profiles give a gesture a different job in specific apps, like switching browser tabs.'),
            tip('tray', 'It lives in the tray', linux
              ? 'Close this window and Pady keeps running from the system tray.'
              : 'Close this window and Pady keeps running from the notification area.')),
        ];
      },
    },
  };

  /* ---------- Step helpers ---------- */

  function feelFingers(n) {
    if (!tour.feel) return;
    tour.felt = Math.max(tour.felt, n);
    if (tour.felt >= 3) {
      if (!tour.feel.classList.contains('is-ok')) {
        tour.feel.className = 'tour-feel is-ok';
        tour.feel.replaceChildren(ico('check', 15), 'Hello! Pady can feel your fingers.');
      }
    } else if (n > 0) {
      tour.feel.replaceChildren(ico('gestures', 15), `${cap(M.NUMBER_WORDS[n] || String(n))} finger${n === 1 ? '' : 's'} — try three.`);
    }
  }

  async function pickTourPreset(id) {
    if (tour.applying) return;
    const prev = tour.picked;
    if (id === 'keep' || id === tour.applied) { tour.picked = id; syncTourPresets(); return; }
    if (!tour.confirmed) {
      const ok = await confirmDialog({
        title: 'Replace your All-apps gestures?',
        text: 'Picking a starter set replaces your current All-apps gestures. Settings and per-app profiles stay as they are.',
        confirm: 'Replace them',
      });
      if (!ok || !tour.open) return;
      tour.confirmed = true;
    }
    tour.applying = id;
    tour.picked = id;
    syncTourPresets();
    try {
      const state = await call('apply_preset', { id }, { errorTitle: 'Couldn’t apply the starter set' });
      applyState(state);
      tour.applied = id;
      if (!tour.open || !tour.presetStatus) return;
      const p = (store.app.presets || []).find((x) => x.id === id);
      const n = (state.config.binding || []).length;
      tour.presetStatus.replaceChildren(ico('check', 14), `${p ? p.name : 'Starter set'} is ready — ${n} gesture${n === 1 ? '' : 's'} set up.`);
      tour.presetStatus.classList.add('is-ok');
      const card = $(`.tour-preset[data-id="${id}"]`, tour.presetGroup);
      if (card) { card.classList.remove('is-applied'); void card.offsetWidth; card.classList.add('is-applied'); }
    } catch (_) {
      tour.picked = prev;
    } finally {
      tour.applying = null;
      if (tour.open && tour.presetGroup) syncTourPresets();
    }
  }

  function syncTourPresets() {
    $$('.tour-preset', tour.presetGroup).forEach((b) => {
      b.setAttribute('aria-checked', String(b.dataset.id === tour.picked));
      b.classList.toggle('is-busy', b.dataset.id === tour.applying);
    });
  }

  function taskEl(t, i) {
    return h('li', { class: 'tour-task', 'data-trigger': t.trigger },
      h('span', { class: 'tour-task-check', 'aria-hidden': 'true' }, h('span', { class: 'tour-task-num' }, String(i + 1)), ico('check', 14)),
      h('span', { class: 'tour-task-main' },
        h('span', { class: 'tour-task-text' }, M.triggerInstruction(t.trigger)),
        t.binding ? chipEl(t.binding.action) : h('span', { class: 'tour-task-unbound' }, 'Nothing bound yet — just try it')),
      h('span', { class: 'sr-only tour-task-state' }, 'not done yet'));
  }

  function updateTaskCount() {
    if (!tour.tasks || !tour.taskCount) return;
    const done = tour.tasks.filter((t) => t.done).length;
    tour.taskCount.textContent = `${done} of ${tour.tasks.length} done`;
  }

  /** onGesture() hook: tick off practice tasks. */
  function tourGesture(p) {
    if (!tour.open || !tour.tasks || !p || !p.trigger || tour.steps[tour.index].id !== 'practice') return;
    const t = tour.tasks.find((x) => !x.done && x.trigger === p.trigger);
    if (!t) return;
    t.done = true;
    const li = $(`.tour-task[data-trigger="${t.trigger}"]`, tour.taskList);
    if (li) { li.classList.add('is-done'); $('.tour-task-state', li).textContent = 'done'; }
    updateTaskCount();
    const all = tour.tasks.every((x) => x.done);
    tour.live.textContent = all ? 'All done. Nice work!' : `Done: ${M.triggerInstruction(t.trigger)}.`;
    if (all) tour.celebrate.hidden = false;
    refreshTourChrome();
  }

  /* ---------- Welcome illustration: tap → swipe up → pinch out, on a loop ---------- */

  function heroSVG() {
    const finger = (x, y, r, px, py) => `<g class="hero-f"${px != null ? ` data-px="${px.toFixed(2)}" data-py="${py.toFixed(2)}"` : ''}>`
      + `<circle class="illo-halo" cx="${x}" cy="${y}" r="${(r * 1.9).toFixed(1)}"/><circle class="illo-finger" cx="${x}" cy="${y}" r="${r}"/></g>`;
    const a = [[148, 116], [180, 106], [212, 116]];
    const actA = `<g class="hero-act hero-act-a">${a.map(([x, y]) => `<circle class="illo-ripple hero-ripple" cx="${x}" cy="${y}" r="11"/>`).join('')}${a.map(([x, y]) => finger(x, y, 11)).join('')}</g>`;
    const b = [[136, 132], [164, 123], [196, 123], [224, 132]];
    const actB = `<g class="hero-act hero-act-b"><g class="hero-swipe">${b.map(([x, y]) => `<line class="hero-trail" x1="${x}" y1="${y}" x2="${x}" y2="${y + 30}"/>`).join('')}${b.map(([x, y]) => finger(x, y, 10)).join('')}</g></g>`;
    const c = [-150, -95, -40, 100].map((deg) => {
      const r = (deg * Math.PI) / 180;
      return { x0: 180 + Math.cos(r) * 19, y0: 108 + Math.sin(r) * 16, x1: 180 + Math.cos(r) * 58, y1: 108 + Math.sin(r) * 46 };
    });
    const actC = `<g class="hero-act hero-act-c"><ellipse class="illo-pinch-ring hero-ring" cx="180" cy="108" rx="66" ry="53"/>${c.map((p) => finger(p.x0.toFixed(1), p.y0.toFixed(1), 10, p.x1 - p.x0, p.y1 - p.y0)).join('')}</g>`;
    return `<svg class="hero-svg" viewBox="0 0 360 210" focusable="false"><rect class="hero-pad" x="14" y="10" width="332" height="190" rx="26"/>${actA}${actB}${actC}</svg>`;
  }

  function heroEl() {
    const el = h('div', { class: 'tour-hero', 'aria-hidden': 'true' },
      applyTravelVars(frag(heroSVG())),
      h('div', { class: 'tour-hero-chips' },
        h('span', { class: 'chip hero-chip hero-chip-a' }, ico('play-pause', 15), 'Play / pause'),
        h('span', { class: 'chip hero-chip hero-chip-b' }, ico('volume', 15), 'Volume +4%'),
        h('span', { class: 'chip hero-chip hero-chip-c' }, ico('zoom-in', 15), 'Zoom in')));
    // Three loops, then rest on the first act so an open tutorial doesn't keep
    // the CPU busy; pointing at it plays the loop again.
    let timer = 0;
    const play = () => {
      el.classList.remove('is-resting');
      clearTimeout(timer);
      timer = setTimeout(() => el.classList.add('is-resting'), 3 * 8000);
    };
    el.addEventListener('pointerenter', play);
    play();
    return el;
  }

  const DONE_SVG = '<svg class="done-svg" viewBox="0 0 96 96" focusable="false">'
    + '<g class="done-sparks"><circle cx="48" cy="6" r="3"/><circle cx="84" cy="22" r="2.5"/><circle cx="90" cy="60" r="3"/><circle cx="64" cy="90" r="2.5"/><circle cx="24" cy="88" r="3"/><circle cx="6" cy="56" r="2.5"/><circle cx="12" cy="20" r="3"/></g>'
    + '<circle class="done-disc" cx="48" cy="48" r="34"/>'
    + '<circle class="done-ring" cx="48" cy="48" r="34" pathLength="100"/>'
    + '<path class="done-check" d="M33 49.5 43.5 60 64 38" pathLength="100"/></svg>';

  /**
   * A small live touchpad (canvas) for the tutorial, drawn with the same
   * painter as the Live page. It asks for the frame stream while running.
   */
  function createMiniPad(opts) {
    const dev = store.app.devices && store.app.devices[0];
    const mm = { w: (dev && dev.width_mm) || 105, h: (dev && dev.height_mm) || 68 };
    const canvas = h('canvas', { 'aria-hidden': 'true' });
    const el = h('div', { class: 'mini-pad', role: 'img', 'aria-label': 'Live view of your touchpad' },
      canvas, h('div', { class: 'mini-pad-hint' }, ico('gestures', 18), 'Your fingers show up here'));
    el.style.aspectRatio = `${mm.w} / ${mm.h}`;
    el.style.setProperty('--aspect', String(mm.w / mm.h));
    let raf = 0, running = false, dirty = true, colors = null, last = -1;
    const ro = new ResizeObserver(() => { dirty = true; kick(); });
    function kick() { if (running && !raf) raf = requestAnimationFrame(frame); }
    function frame() {
      raf = 0;
      if (!running) return;
      if (dirty) {
        dirty = false;
        const dpr = window.devicePixelRatio || 1;
        canvas.width = Math.round(canvas.clientWidth * dpr);
        canvas.height = Math.round(canvas.clientHeight * dpr);
      }
      const n = paintTouches(canvas, mm, colors, { small: true });
      if (n !== last) {
        last = n;
        el.classList.toggle('has-touch', n > 0);
        if (opts.onCount) opts.onCount(n);
      }
      // Rest while nothing is on the pad; the next frame wakes it.
      if (n > 0 || live.trails.size > 0) raf = requestAnimationFrame(frame);
    }
    return {
      el,
      start() {
        if (running) return;
        running = true;
        colors = themeColors();
        ro.observe(el);
        wantLive('tour', true);
        frameWakers.add(kick);
        kick();
      },
      stop() {
        running = false;
        cancelAnimationFrame(raf);
        raf = 0;
        frameWakers.delete(kick);
        ro.disconnect();
        wantLive('tour', false);
      },
    };
  }

  /* ---------- Updates ---------- */

  const updates = { available: null, checking: false, installing: false, note: '' };

  function announceUpdate(u) {
    if (!u || (updates.available && updates.available.version === u.version)) return;
    updates.available = u;
    toast({
      kind: 'info', title: `Pady ${u.version} is available`, timeout: 15000,
      text: u.can_install ? 'Update now, or later from the tray menu.' : 'Update with your package manager, or download it from lumidren.github.io/Pady.',
      action: u.can_install ? { label: 'Update now', onClick: installUpdate } : null,
    });
    if (store.page === 'about') renderAbout();
  }

  async function checkUpdate(manual) {
    updates.checking = true; updates.note = '';
    if (store.page === 'about') renderAbout();
    try {
      const u = await call('check_update', null, { quiet: !manual, errorTitle: 'Couldn’t check for updates' });
      if (u) announceUpdate(u);
      else updates.note = 'Pady is up to date.';
    } catch (_) {
      updates.note = manual ? 'Couldn’t reach GitHub. Check your connection.' : '';
    }
    updates.checking = false;
    if (store.page === 'about') renderAbout();
  }

  async function installUpdate() {
    updates.installing = true;
    if (store.page === 'about') renderAbout();
    toast({ kind: 'info', title: 'Downloading the update…', text: 'Pady restarts by itself when it’s done.', timeout: 20000 });
    try { await call('install_update', null, { errorTitle: 'Couldn’t install the update' }); } catch (_) { /* toast shown */ }
    updates.installing = false;
    if (store.page === 'about') renderAbout();
  }

  /**
   * Feedback straight from the app: GitHub's issue forms, with the version and
   * system already filled in (links open in the default browser).
   */
  function feedbackPanel() {
    const s = store.app;
    const form = (template, fields) => {
      const q = new URLSearchParams({ template, ...fields });
      return `https://github.com/lumidren/Pady/issues/new?${q}`;
    };
    const open = (url) => call('test_action', { action: { type: 'open_url', url } }, { quiet: true }).catch(() => {});
    const button = (iconName, label, url, primary) =>
      h('button', { class: 'btn btn--sm' + (primary ? ' btn--primary' : ''), type: 'button', onClick: () => open(url) }, ico(iconName, 14), label);
    return h('section', { class: 'panel', style: { 'margin-bottom': '16px' } },
      h('div', { class: 'panel-head' },
        h('div', null, h('div', { class: 'panel-title' }, 'Feedback'), h('div', { class: 'panel-sub' }, 'Found a bug or want something new? It opens a short form on GitHub.')),
        h('div', { class: 'about-links', style: { 'margin-top': '0' } },
          button('warning', 'Report a bug', form('bug_report.yml', { title: 'Bug: ', version: s.version }), true),
          button('sparkle', 'Suggest a feature', form('feature_request.yml', { title: 'Idea: ' })),
          button('info', 'Share feedback', form('beta_feedback.yml', { version: s.version })))));
  }

  /** "Made by" at the end of About: three finger dots (links open in the browser). */
  function makerLine() {
    const open = (url) => call('test_action', { action: { type: 'open_url', url } }, { quiet: true }).catch(() => {});
    const link = (label, url) => h('button', { class: 'maker-link', type: 'button', onClick: () => open(url) }, label);
    return h('footer', { class: 'maker-dots' },
      h('div', { class: 'maker-dots-row', 'aria-hidden': 'true' }, h('i'), h('i'), h('i')),
      h('div', { class: 'maker-dots-title' }, 'made by ', h('span', { class: 'maker-dots-name' }, 'lumidren')),
      h('div', { class: 'maker-dots-sub' }, 'Because the Mac trackpad shouldn\u2019t get to have all the fun.', h('span', { class: 'maker-sep' }, '·'), link('LinkedIn', 'https://www.linkedin.com/in/lumidren'), h('span', { class: 'maker-sep' }, '·'), link('GitHub', 'https://github.com/lumidren')));
  }

  function updatesPanel() {
    const u = updates.available;
    const status = u ? `Pady ${u.version} is available.` : updates.checking ? 'Checking…' : updates.note || `You have Pady ${store.app.version}.`;
    const button = u && u.can_install
      ? h('button', { class: 'btn btn--sm btn--primary', type: 'button', disabled: updates.installing, onClick: installUpdate }, ico('download', 14), updates.installing ? 'Updating…' : `Update to ${u.version}`)
      : h('button', { class: 'btn btn--sm', type: 'button', disabled: updates.checking, onClick: () => checkUpdate(true) }, ico('reset', 14), 'Check for updates');
    return h('section', { class: 'panel', style: { 'margin-bottom': '16px' } },
      h('div', { class: 'panel-head' },
        h('div', null, h('div', { class: 'panel-title' }, 'Updates'), h('div', { class: 'panel-sub' }, status)),
        button),
      u && !u.can_install ? h('p', { class: 'panel-sub', style: { padding: '0 20px 16px' } }, 'Update with your package manager, or download it from lumidren.github.io/Pady.') : null);
  }

  /* =====================================================================
     12. Events & boot
     ===================================================================== */

  function listenAll() {
    const on = (name, cb) => Promise.resolve(api.listen(name, cb)).catch((err) => console.error(`listen ${name} failed:`, err));
    on('pady://state', (e) => applyState(e.payload));
    on('pady://frame', (e) => onFrame(e.payload));
    on('pady://gesture', (e) => onGesture(e.payload));
    on('pady://update', (e) => announceUpdate(e.payload));
    on('pady://action-failed', (e) => {
      const p = e.payload || {};
      toast({ kind: 'error', title: p.action ? `“${p.action}” failed` : 'Action failed', text: p.error });
    });
  }

  document.addEventListener('visibilitychange', () => {
    syncLive();
    if (document.hidden) { liveView.pause(); flushPendingSettings(); } else liveView.resume();
  });
  // The backend destroys this window when it's closed (and builds a new one
  // on open), so nothing may wait for later: send a debounced save right away.
  window.addEventListener('pagehide', flushPendingSettings);

  window.addEventListener('hashchange', () => {
    const page = location.hash.replace('#', '');
    if (store.app && page !== store.page && PAGES.includes(page) && !editor && !profEd && !tour.open) route(page);
  });

  // Theme switches change canvas colours.
  window.matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => liveView.readColors());

  async function boot() {
    initShell();
    listenAll();
    const initial = (location.hash || '').replace('#', '');
    try {
      // Everything comes from get_state: the window is rebuilt from scratch each time it opens.
      const state = await call('get_state', null, { errorTitle: 'Couldn’t load Pady’s state' });
      store.app = state;
      renderShell();
      $('#boot').remove();
      syncLive(); // tells the backend "no frames" unless a page below asks for them
      route(PAGES.includes(initial) ? initial : 'gestures');
      // `=== false`: a backend without the field never shows the tutorial.
      if (state.config.settings.onboarded === false) openTutorial({ firstRun: true });
      if (state.config.settings.check_updates !== false) checkUpdate(false);
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
