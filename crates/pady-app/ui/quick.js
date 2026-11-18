/* Pady — the quick note.
 *
 * The window is kept ready while a Quick note gesture is set, so the page
 * is built once and only filled in: the backend shows the window the
 * moment the gesture fires and sends what to show (`pady://quick`, or
 * `quick_ready` on load). The selected text comes a moment later, in a
 * second message with the same id, and then the keyboard focus.
 *
 * A note is never lost: closing it any way saves what's written.
 * Opened in a plain browser, it shows a demo. */
(() => {
  'use strict';

  const tauri = window.__TAURI__;
  const invoke = tauri ? tauri.core.invoke : async () => null;
  const $ = (id) => document.getElementById(id);
  const card = $('card'), box = $('note'), source = $('source'), foot = $('foot');

  const OUT_MS = 100;
  const WIDTH = 380;

  let current = null; // the note on screen: { id, text, source, pending }
  let visible = false;
  let shownAt = 0;
  let filled = 0; // the id whose selected text is already in the box

  function act(kind, extra) {
    if (!current) return;
    const args = { id: current.id, kind, text: null, ...extra };
    const send = () => invoke('quick_act', args).catch((e) => console.error('quick_act', e));
    if (kind === 'drag') return send();
    if (!visible) return;
    hide();
    setTimeout(send, OUT_MS);
  }

  /** Close, saving what's written (an empty note just closes). */
  const dismiss = () => act('save', { text: box.value });

  function hide() {
    visible = false;
    card.classList.remove('is-in', 'is-ready');
    card.classList.add('is-out');
  }

  /** The box grows with the text, up to a point; then it scrolls. */
  function fit() {
    box.style.height = 'auto';
    box.style.height = Math.min(300, Math.max(120, box.scrollHeight + 2)) + 'px';
  }

  /** Tell the backend the size, so it sizes (and, the first time, shows) the window. */
  function layout() {
    if (!current) return;
    const height = Math.ceil(document.documentElement.getBoundingClientRect().height);
    invoke('quick_layout', { id: current.id, width: WIDTH, height }).catch((e) => console.error('quick_layout', e));
  }

  function focusBox() {
    box.focus({ preventScroll: true });
    box.setSelectionRange(box.value.length, box.value.length);
  }

  function render(p) {
    if (!p) return;
    const fresh = !current || current.id !== p.id;
    current = p;
    if (fresh) {
      box.value = '';
      filled = 0;
    }
    // The selected text goes first; whatever was typed meanwhile stays.
    if (p.text && filled !== p.id) {
      filled = p.id;
      box.value = p.text + '\n\n' + box.value;
    }
    source.textContent = p.source ? `· ${p.source.replace(/\.exe$/i, '')}` : '';
    fit();
    layout();
    if (!visible) {
      visible = true;
      shownAt = performance.now();
      // A window that was kept ready is already drawn: no entrance to wait for.
      if (!card.classList.contains('is-ready')) {
        card.classList.remove('is-in', 'is-out');
        void card.offsetWidth;
        card.classList.add('is-in');
      }
    }
    if (document.hasFocus()) focusBox();
  }

  box.addEventListener('input', () => { fit(); layout(); });
  $('close').addEventListener('click', dismiss);
  $('save').addEventListener('click', dismiss);
  $('all').addEventListener('click', () => act('notes', { text: box.value }));

  for (const handle of [card.querySelector('.head'), foot]) {
    handle.addEventListener('mousedown', (e) => {
      if (e.button !== 0 || e.target.closest('button, textarea') || e.detail > 1) return;
      e.preventDefault();
      handle.classList.add('is-dragging');
      act('drag');
    });
    handle.addEventListener('mouseup', () => handle.classList.remove('is-dragging'));
    handle.addEventListener('mouseleave', () => handle.classList.remove('is-dragging'));
  }

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' || (e.key === 'Enter' && (e.ctrlKey || e.metaKey))) {
      e.preventDefault();
      dismiss();
    }
  });

  // Clicking elsewhere saves and closes. Dragging drops the focus for a
  // blink, so only if it stays away.
  let blurTimer = 0;
  window.addEventListener('blur', () => {
    if (!visible || performance.now() - shownAt <= 300) return;
    clearTimeout(blurTimer);
    blurTimer = setTimeout(() => { if (!document.hasFocus()) dismiss(); }, 200);
  });
  window.addEventListener('focus', () => {
    clearTimeout(blurTimer);
    if (visible) focusBox();
  });

  if (tauri) {
    tauri.event.listen('pady://quick', (e) => render(e.payload));
    tauri.event.listen('pady://quick-dismiss', dismiss);
    // Hidden: empty the page, so the next note is there the moment the
    // window shows, before its message arrives.
    tauri.event.listen('pady://quick-hidden', () => {
      visible = false;
      current = null;
      box.value = '';
      source.textContent = '';
      box.style.height = '';
      card.classList.remove('is-in', 'is-out');
      card.classList.add('is-ready');
    });
    invoke('quick_ready').then((p) => {
      // Made ahead of time, with no note waiting: drawn and ready to show.
      if (p) render(p); else card.classList.add('is-ready');
    }).catch((e) => console.error('quick_ready', e));
  } else {
    render({ id: 1, text: 'Pady turns the touchpad into shortcuts.', source: 'chrome.exe', pending: false });
    focusBox();
  }
})();
