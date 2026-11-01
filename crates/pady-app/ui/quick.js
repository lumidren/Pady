/* Pady — quick pop-up: the clipboard history and the quick note.
 *
 * The backend sends what to show (`pady://quick`, or `quick_ready` on
 * load); after every render the page reports its size and the backend
 * places the window by the cursor and shows it, like the translation
 * pop-up. A note is never lost: closing it any way saves what's written.
 * Opened in a plain browser, it shows a demo (?mode=clips|note|off|empty). */
(() => {
  'use strict';

  const tauri = window.__TAURI__;
  const invoke = tauri ? tauri.core.invoke : async () => null;
  const $ = (id) => document.getElementById(id);
  const card = $('card'), title = $('title'), body = $('body'), foot = $('foot');

  /** The clipboard list closes on its own after this long untouched. */
  const IDLE_CLOSE_MS = 25000;
  const OUT_MS = 120;
  const WIDTH = 380;

  let current = null;
  let visible = false;
  let shownAt = 0;
  let hovering = false;
  let idleTimer = 0;
  let active = 0; // the highlighted clip
  let shown = []; // clips after the search, with their index in the full list
  let noteBox = null;

  function h(tag, attrs, ...kids) {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (k.startsWith('on')) el.addEventListener(k.slice(2).toLowerCase(), v);
      else if (v !== false && v != null) el.setAttribute(k, v === true ? '' : v);
    }
    el.append(...kids.flat().filter((k) => k != null && k !== false));
    return el;
  }

  function act(kind, extra) {
    if (!current) return;
    const args = { id: current.id, kind, index: null, text: null, ...extra };
    const send = () => invoke('quick_act', args).catch((e) => console.error('quick_act', e));
    if (['close', 'paste', 'copy', 'save', 'notes'].includes(kind)) {
      if (!visible) return;
      hide();
      setTimeout(send, OUT_MS);
    } else {
      send();
    }
  }

  /** Close; a note is saved first (an empty one just closes). */
  function dismiss() {
    if (current && current.mode === 'note') act('save', { text: noteBox ? noteBox.value : '' });
    else act('close');
  }

  function hide() {
    visible = false;
    clearTimeout(idleTimer);
    card.classList.remove('is-in');
    card.classList.add('is-out');
  }

  function armIdleClose() {
    clearTimeout(idleTimer);
    if (!current || current.mode !== 'clips' || hovering) return;
    idleTimer = setTimeout(() => act('close'), IDLE_CLOSE_MS);
  }

  function ago(at) {
    const s = Math.max(0, (Date.now() - at) / 1000);
    if (s < 60) return 'now';
    if (s < 3600) return `${Math.floor(s / 60)}m`;
    if (s < 86400) return `${Math.floor(s / 3600)}h`;
    return `${Math.floor(s / 86400)}d`;
  }

  function btn(label, onClick, cls = '') {
    return h('button', { class: `btn ${cls}`.trim(), type: 'button', onClick }, label);
  }

  // ---- Clipboard history ----

  function renderClips(p, fresh) {
    title.textContent = 'Clipboard';
    foot.replaceChildren();
    if (!p.supported || !p.on) {
      body.replaceChildren(h('div', { class: 'empty' },
        h('strong', null, 'Clipboard history is off'),
        p.supported
          ? 'Turn it on and Pady remembers the last 25 things you copy, in memory only. Passwords from password managers are skipped.'
          : 'Clipboard history works on Windows for now.',
        p.supported ? h('div', null, btn('Turn on', () => act('enable'), 'btn--primary')) : null));
      return;
    }
    if (!p.clips.length) {
      body.replaceChildren(h('div', { class: 'empty' },
        h('strong', null, 'Nothing copied yet'),
        'Copy some text and it shows up here. It’s kept in memory only, and gone when Pady quits.'));
      return;
    }
    const search = h('input', { class: 'search', type: 'search', placeholder: 'Search what you copied', 'aria-label': 'Search', spellcheck: 'false' });
    const list = h('ul', { class: 'clips', role: 'listbox', 'aria-label': 'Copied text' });
    const fill = () => {
      const q = search.value.trim().toLowerCase();
      shown = p.clips.map((c, i) => ({ ...c, i })).filter((c) => !q || c.text.toLowerCase().includes(q));
      active = Math.min(active, Math.max(0, shown.length - 1));
      list.replaceChildren(...(shown.length ? shown.map((c, n) => h('li', null,
        h('button', {
          class: 'clip' + (n === active ? ' is-active' : ''), type: 'button', role: 'option', 'aria-selected': String(n === active),
          title: 'Paste (Enter)', onClick: () => act('paste', { index: c.i }), onMouseMove: () => setActive(n, list),
        },
        h('span', { class: 'clip-key', 'aria-hidden': 'true' }, n < 9 ? String(n + 1) : '·'),
        h('span', { class: 'clip-text' }, c.text.trim().slice(0, 400)),
        h('span', { class: 'clip-age' }, ago(c.at))))) : [h('li', { class: 'empty' }, 'No match')]));
    };
    search.addEventListener('input', () => { active = 0; fill(); layout(); });
    if (fresh) active = 0;
    fill();
    body.replaceChildren(search, list);
    foot.append(
      h('span', { class: 'hint' }, 'Enter pastes · 1–9 · Esc'),
      h('span', { class: 'spacer' }),
      btn('Clear', () => act('clear'), 'btn--ghost'));
    requestAnimationFrame(() => search.focus({ preventScroll: true }));
  }

  function setActive(n, list = body.querySelector('.clips')) {
    if (!list || !shown.length) return;
    active = Math.max(0, Math.min(shown.length - 1, n));
    list.querySelectorAll('.clip').forEach((el, i) => {
      el.classList.toggle('is-active', i === active);
      el.setAttribute('aria-selected', String(i === active));
      if (i === active) el.scrollIntoView({ block: 'nearest' });
    });
  }

  // ---- Quick note ----

  function renderNote(p, fresh) {
    title.textContent = 'Quick note';
    if (!fresh && noteBox) return; // keep what's being typed
    noteBox = h('textarea', { class: 'note-text', placeholder: 'Write something…', 'aria-label': 'Note', spellcheck: 'true' });
    noteBox.value = p.text ? p.text + '\n\n' : '';
    noteBox.addEventListener('input', () => { fit(); layout(); });
    const fit = () => { noteBox.style.height = 'auto'; noteBox.style.height = Math.min(300, Math.max(120, noteBox.scrollHeight + 2)) + 'px'; };
    body.replaceChildren(noteBox, p.source ? h('span', { class: 'source' }, `From ${p.source}`) : null);
    foot.replaceChildren(
      btn('All notes', () => act('notes', { text: noteBox.value }), 'btn--ghost'),
      h('span', { class: 'spacer' }),
      h('span', { class: 'hint' }, 'Ctrl+Enter'),
      btn('Save', () => act('save', { text: noteBox.value }), 'btn--primary'));
    requestAnimationFrame(() => {
      fit();
      noteBox.focus({ preventScroll: true });
      noteBox.setSelectionRange(noteBox.value.length, noteBox.value.length);
      layout();
    });
  }

  function render(p) {
    if (!p) return;
    const fresh = !current || current.id !== p.id;
    current = p;
    if (p.mode === 'note') renderNote(p, fresh);
    else { noteBox = null; renderClips(p, fresh); }
    layout();
    armIdleClose();
  }

  /** Report the size; the backend shows the window once it's placed. */
  function layout() {
    requestAnimationFrame(() => {
      const height = Math.ceil(document.documentElement.getBoundingClientRect().height);
      invoke('quick_layout', { width: WIDTH, height }).catch((e) => console.error('quick_layout', e));
      if (!visible) {
        visible = true;
        shownAt = performance.now();
        card.classList.remove('is-in', 'is-out');
        void card.offsetWidth;
        card.classList.add('is-in');
        if (!card.contains(document.activeElement)) card.focus({ preventScroll: true });
      }
    });
  }

  $('close').addEventListener('click', dismiss);

  for (const handle of [card.querySelector('.head'), foot]) {
    handle.addEventListener('mousedown', (e) => {
      if (e.button !== 0 || e.target.closest('button, input, textarea') || e.detail > 1) return;
      e.preventDefault();
      handle.classList.add('is-dragging');
      act('drag');
    });
    handle.addEventListener('mouseup', () => handle.classList.remove('is-dragging'));
    handle.addEventListener('mouseleave', () => handle.classList.remove('is-dragging'));
  }

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      dismiss();
      return;
    }
    if (!current) return;
    if (current.mode === 'note') {
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        act('save', { text: noteBox ? noteBox.value : '' });
      }
      return;
    }
    if (!shown.length) return;
    const typing = e.target.classList && e.target.classList.contains('search');
    if (e.key === 'ArrowDown') { e.preventDefault(); setActive(active + 1); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); setActive(active - 1); }
    else if (e.key === 'Enter') { e.preventDefault(); act('paste', { index: shown[active].i }); }
    else if (/^[1-9]$/.test(e.key) && !(typing && e.target.value)) {
      const c = shown[Number(e.key) - 1];
      if (c) { e.preventDefault(); act('paste', { index: c.i }); }
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'c' && !String(getSelection())) {
      e.preventDefault();
      act('copy', { index: shown[active].i });
    }
  });

  card.addEventListener('mouseenter', () => { hovering = true; clearTimeout(idleTimer); });
  card.addEventListener('mouseleave', () => { hovering = false; armIdleClose(); });

  // Clicking elsewhere closes it (a note is saved). Dragging drops focus for
  // a blink, so only if focus stays away.
  let blurTimer = 0;
  window.addEventListener('blur', () => {
    if (!visible || performance.now() - shownAt <= 300) return;
    clearTimeout(blurTimer);
    blurTimer = setTimeout(() => { if (!document.hasFocus()) dismiss(); }, 200);
  });
  window.addEventListener('focus', () => clearTimeout(blurTimer));

  if (tauri) {
    tauri.event.listen('pady://quick', (e) => render(e.payload));
    tauri.event.listen('pady://quick-hidden', () => {
      visible = false;
      clearTimeout(idleTimer);
      card.classList.remove('is-in', 'is-out');
    });
    invoke('quick_ready').then(render).catch((e) => console.error('quick_ready', e));
  } else {
    const mode = new URLSearchParams(location.search).get('mode') || 'clips';
    const now = Date.now();
    const clips = [
      { text: 'https://lumidren.github.io/Pady/', at: now - 20e3 },
      { text: 'Meeting moved to 3pm, room 4B', at: now - 5 * 60e3 },
      { text: 'const pinch = fingers >= 3 && spread < threshold;\nreturn pinch;', at: now - 42 * 60e3 },
      { text: 'Thanks so much! I’ll send the files over tonight.', at: now - 3 * 3600e3 },
    ];
    const demo = {
      clips: { id: 1, mode: 'clips', on: true, supported: true, clips },
      empty: { id: 1, mode: 'clips', on: true, supported: true, clips: [] },
      off: { id: 1, mode: 'clips', on: false, supported: true, clips: [] },
      note: { id: 1, mode: 'note', text: 'Pady turns the touchpad into shortcuts.', source: 'chrome.exe' },
    };
    render(demo[mode] || demo.clips);
  }
})();
