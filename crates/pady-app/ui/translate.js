/* Pady — translation pop-up.
 *
 * The backend sends what to show (`pady://translate`, or `translate_ready`
 * on load). After every render the page reports its size; the backend then
 * places the window by the cursor and shows it. Esc, a click elsewhere or
 * a quiet while closes it. Opened in a plain browser, it shows a demo. */
(() => {
  'use strict';

  const tauri = window.__TAURI__;
  const invoke = tauri ? tauri.core.invoke : async () => null;
  const $ = (id) => document.getElementById(id);
  const card = $('card'), langs = $('langs'), body = $('body'), engine = $('engine');
  const copyBtn = $('copy'), replaceBtn = $('replace'), retryBtn = $('retry');

  /** Closes on its own after this long untouched (paused while hovered). */
  const IDLE_CLOSE_MS = 25000;
  let current = null;
  let visible = false;
  let shownAt = 0;
  let hovering = false;
  let idleTimer = 0;

  /** How long the card takes to fade out before the window hides. */
  const OUT_MS = 120;

  function act(kind) {
    if (!current) return;
    const id = current.id;
    const send = () => invoke('translate_act', { id, kind }).catch((e) => console.error('translate_act', e));
    if (kind === 'close' || kind === 'copy' || kind === 'replace') {
      if (!visible) return;
      hide();
      setTimeout(send, OUT_MS);
    } else {
      send();
    }
  }

  function hide() {
    visible = false;
    clearTimeout(idleTimer);
    card.classList.remove('is-in');
    card.classList.add('is-out');
  }

  function armIdleClose() {
    clearTimeout(idleTimer);
    if (!current || current.state === 'loading' || hovering) return;
    idleTimer = setTimeout(() => act('close'), IDLE_CLOSE_MS);
  }

  function shimmer() {
    const s = document.createElement('div');
    s.className = 'shimmer';
    s.setAttribute('aria-label', 'Translating');
    for (let i = 0; i < 3; i++) s.append(document.createElement('i'));
    return s;
  }

  function render(p) {
    if (!p) return;
    const fresh = !current || current.id !== p.id;
    current = p;
    body.className = 'body';
    body.removeAttribute('dir');
    body.replaceChildren();
    copyBtn.hidden = replaceBtn.hidden = false;
    retryBtn.hidden = true;
    copyBtn.classList.remove('is-done');

    if (p.state === 'loading') {
      langs.textContent = p.from_name ? `${p.from_name} → ${p.to_name}` : `Translating into ${p.to_name}…`;
      body.append(shimmer());
      engine.textContent = '';
      copyBtn.disabled = replaceBtn.disabled = true;
    } else if (p.state === 'done') {
      const t = p.translation;
      langs.textContent = t.from_name ? `${t.from_name} → ${t.to_name}` : t.to_name;
      body.textContent = t.text;
      body.setAttribute('dir', p.rtl ? 'rtl' : 'auto');
      if (t.unchanged) {
        const note = document.createElement('span');
        note.className = 'note';
        note.setAttribute('dir', 'ltr');
        note.textContent = `Already in ${t.to_name}.`;
        body.append(note);
      }
      engine.textContent = t.engine;
      copyBtn.disabled = replaceBtn.disabled = false;
    } else {
      langs.textContent = 'Translation';
      body.classList.add('is-error');
      body.textContent = p.message;
      engine.textContent = '';
      copyBtn.hidden = replaceBtn.hidden = true;
      retryBtn.hidden = !p.retry;
    }
    if (fresh) body.scrollTop = 0;
    layout();
    armIdleClose();
  }

  /** Report the size; the backend shows the window once it's placed. */
  function layout() {
    requestAnimationFrame(() => {
      const height = Math.ceil(document.documentElement.getBoundingClientRect().height);
      invoke('translate_layout', { width: 400, height }).catch((e) => console.error('translate_layout', e));
      if (!visible) {
        visible = true;
        shownAt = performance.now();
        // Restart the entrance each time the window appears.
        card.classList.remove('is-in', 'is-out');
        void card.offsetWidth;
        card.classList.add('is-in');
        // Focus a button so Esc and Tab work straight away (not Replace: Enter mustn't paste).
        (copyBtn.hidden || copyBtn.disabled ? $('close') : copyBtn).focus({ preventScroll: true });
      }
    });
  }

  $('close').addEventListener('click', () => act('close'));
  copyBtn.addEventListener('click', () => act('copy'));
  replaceBtn.addEventListener('click', () => act('replace'));
  retryBtn.addEventListener('click', () => act('retry'));

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      act('close');
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'c' && !String(getSelection())) {
      // Ctrl+C with nothing highlighted copies the whole translation.
      if (current && current.state === 'done') {
        e.preventDefault();
        act('copy');
      }
    }
  });

  card.addEventListener('mouseenter', () => { hovering = true; clearTimeout(idleTimer); });
  card.addEventListener('mouseleave', () => { hovering = false; armIdleClose(); });

  // Clicking anywhere else closes it (ignoring the moment it takes focus).
  window.addEventListener('blur', () => {
    if (visible && performance.now() - shownAt > 300) act('close');
  });

  if (tauri) {
    tauri.event.listen('pady://translate', (e) => render(e.payload));
    tauri.event.listen('pady://translate-hidden', () => {
      visible = false;
      clearTimeout(idleTimer);
      card.classList.remove('is-in', 'is-out');
    });
    invoke('translate_ready').then(render).catch((e) => console.error('translate_ready', e));
  } else {
    // Demo for checking the look in a browser: ?state=loading|done|error|rtl
    const state = new URLSearchParams(location.search).get('state') || 'done';
    const demo = {
      loading: { id: 1, state: 'loading', from_name: 'Spanish', to_name: 'English', original: '' },
      done: { id: 1, state: 'done', rtl: false, original: '', translation: { text: 'Hello, how are you? This is a test of the translator, with a second sentence so it wraps onto more lines.', from_name: 'Spanish', to_name: 'English', engine: 'MyMemory', unchanged: false } },
      rtl: { id: 1, state: 'done', rtl: true, original: '', translation: { text: 'مرحبا، كيف حالك؟ هذا اختبار للمترجم.', from_name: 'English', to_name: 'Arabic', engine: 'DeepL', unchanged: false } },
      error: { id: 1, state: 'failed', retry: true, original: '', message: 'Couldn’t reach the translation service. Check your internet connection.' },
    };
    render(demo[state] || demo.done);
  }
})();
