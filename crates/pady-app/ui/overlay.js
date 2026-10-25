/*
 * Pady — volume / brightness overlay.
 *
 * Rust shows this transparent, non-focusable window near the bottom of the
 * screen and emits `pady://level` { kind: "volume" | "brightness",
 * percent: 0-100, muted } for every change, then hides the window ~1.3 s
 * after the last one. The pill also fades itself out after ~1.2 s idle, so a
 * hide that races a new event never shows a half-faded or stale pill.
 *
 * Without Tauri (a plain browser), or with ?demo, it loops fake levels.
 * ?static keeps one level on screen (for screenshots); ?static=brightness or
 * ?static=muted pick the variant.
 */
(function () {
  'use strict';

  const T = window.__TAURI__;
  const q = new URLSearchParams(location.search);
  const pill = document.getElementById('pill');
  const iconEl = document.getElementById('pill-icon');
  const fill = document.getElementById('pill-fill');
  const num = document.getElementById('pill-num');
  const label = document.getElementById('pill-label');

  // Fade starts here; the CSS fade-out takes 0.15 s, so the pill is gone by ~1.3 s.
  const IDLE_MS = q.has('static') ? Infinity : 1150;
  const reduced = () => window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  const clamp = (v, a, b) => Math.max(a, Math.min(b, v));

  const SPEAKER = 'M11 5 6.5 9H3.5v6h3L11 19z';
  const ICONS = {
    volume: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="${SPEAKER}" fill="currentColor" fill-opacity=".2"/><path class="wave w1" d="M15.4 9.3a3.9 3.9 0 0 1 0 5.4"/><path class="wave w2" d="M18.3 6.7a7.5 7.5 0 0 1 0 10.6"/></svg>`,
    muted: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="${SPEAKER}" fill="currentColor" fill-opacity=".2"/><path d="m16 9.5 5 5M21 9.5l-5 5"/></svg>`,
    brightness: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="4" fill="currentColor" fill-opacity=".2"/><g class="rays"><path d="M12 2.5v2M12 19.5v2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M2.5 12h2M19.5 12h2M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4"/></g></svg>',
  };

  let visible = false;
  let idleTimer = 0;
  let iconKey = '';

  function setIcon(kind, muted, pct) {
    const key = kind === 'brightness' ? 'brightness' : muted ? 'muted' : 'volume';
    if (key !== iconKey) { iconEl.innerHTML = ICONS[key]; iconKey = key; }
    if (key === 'volume') {
      // 0 = silent, 1 = one wave, 2 = both waves.
      iconEl.dataset.level = pct === 0 ? '0' : pct < 50 ? '1' : '2';
    } else if (key === 'brightness') {
      const rays = iconEl.querySelector('.rays');
      if (rays) {
        rays.style.transform = `scale(${(0.72 + 0.28 * (pct / 100)).toFixed(3)})`;
        rays.style.opacity = (0.45 + 0.55 * (pct / 100)).toFixed(3);
      }
    }
  }

  /* The number counts quickly to its new value (instant with reduced motion
     or when the pill is just appearing, so it never counts up from stale). */
  let shown = 0, from = 0, target = 0, t0 = 0, raf = 0, settle = 0;
  function setNumber(v, jump) {
    target = v;
    cancelAnimationFrame(raf);
    clearTimeout(settle);
    if (jump || reduced()) { shown = v; num.textContent = String(v); return; }
    from = shown;
    t0 = performance.now();
    raf = requestAnimationFrame(function step(t) {
      const k = Math.min(1, (t - t0) / 160);
      shown = Math.round(from + (target - from) * (1 - Math.pow(1 - k, 3)));
      num.textContent = String(shown);
      if (k < 1) raf = requestAnimationFrame(step);
    });
    // Safety net: if frames are throttled, still land on the right number.
    settle = setTimeout(() => { cancelAnimationFrame(raf); shown = target; num.textContent = String(target); }, 220);
  }

  function show(level) {
    const kind = level && level.kind === 'brightness' ? 'brightness' : 'volume';
    const pct = clamp(Math.round(Number(level && level.percent) || 0), 0, 100);
    const muted = kind === 'volume' && !!(level && level.muted);
    const entering = !visible;

    // Appearing: place the bar without sliding from the previous session's level.
    if (entering) pill.classList.add('no-anim');
    pill.dataset.kind = kind;
    pill.classList.toggle('is-muted', muted);
    setIcon(kind, muted, pct);
    fill.style.width = pct + '%';
    setNumber(pct, entering);
    label.textContent = `${kind === 'volume' ? 'Volume' : 'Brightness'} ${pct}%${muted ? ', muted' : ''}`;
    if (entering) { void pill.offsetWidth; pill.classList.remove('no-anim'); }

    pill.classList.remove('is-leaving');
    pill.classList.add('is-on');
    visible = true;
    clearTimeout(idleTimer);
    if (IDLE_MS !== Infinity) idleTimer = setTimeout(hide, IDLE_MS);
  }

  function hide() {
    visible = false;
    pill.classList.remove('is-on');
    pill.classList.add('is-leaving');
  }

  /* ---------- Browser preview ---------- */

  async function demo() {
    document.documentElement.classList.add('demo');
    document.getElementById('demo-caption').hidden = false;
    const wait = (ms) => new Promise((r) => setTimeout(r, ms));
    if (q.has('static')) {
      const v = q.get('static');
      show(v === 'brightness' ? { kind: 'brightness', percent: 70, muted: false } : { kind: 'volume', percent: 64, muted: v === 'muted' });
      return;
    }
    for (;;) {
      let vol = 36, bri = 70;
      await wait(600);
      for (let i = 0; i < 8; i++) { vol += 4; show({ kind: 'volume', percent: vol, muted: false }); await wait(130); }
      await wait(1900);
      for (let i = 0; i < 6; i++) { vol -= 4; show({ kind: 'volume', percent: vol, muted: false }); await wait(130); }
      await wait(1000);
      show({ kind: 'volume', percent: vol, muted: true });
      await wait(1900);
      show({ kind: 'volume', percent: vol, muted: false });
      await wait(1900);
      for (let i = 0; i < 4; i++) { bri -= 10; show({ kind: 'brightness', percent: bri, muted: false }); await wait(180); }
      await wait(700);
      for (let i = 0; i < 6; i++) { bri = Math.min(100, bri + 10); show({ kind: 'brightness', percent: bri, muted: false }); await wait(180); }
      await wait(2200);
    }
  }

  if (T && T.event && !q.has('demo')) {
    Promise.resolve(T.event.listen('pady://level', (e) => show(e.payload || {})))
      .catch((err) => console.error('overlay: listen failed', err));
  } else {
    demo();
  }
})();
