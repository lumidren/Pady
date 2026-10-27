/*
 * Pady — inline SVG icon set (24×24 grid, 1.75px strokes, currentColor).
 * `Pady.icon(name, size?)` returns an SVG string.
 */
(function () {
  'use strict';
  const P = (window.Pady = window.Pady || {});

  const paths = {
    gestures: '<rect x="2.5" y="4.5" width="19" height="15" rx="3.5"/><circle cx="8" cy="12.5" r="1.4"/><circle cx="12" cy="11" r="1.4"/><circle cx="16" cy="12.5" r="1.4"/>',
    live: '<path d="M2.5 12h4l2.5-6.5 5 13 2.5-6.5h5"/>',
    download: '<path d="M12 4v11M7 10.5l5 5 5-5M5 19.5h14"/>',
    settings: '<path d="M4 6.5h9M17 6.5h3M4 12h3M11 12h9M4 17.5h11M19 17.5h1"/><circle cx="15" cy="6.5" r="2"/><circle cx="9" cy="12" r="2"/><circle cx="17" cy="17.5" r="2"/>',
    info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v5.5M12 7.8v.01"/>',
    volume: '<path d="M11 5 6.5 9H3.5v6h3L11 19z"/><path d="M15.5 9a4.2 4.2 0 0 1 0 6M18.5 6a8.5 8.5 0 0 1 0 12"/>',
    'volume-down': '<path d="M11 5 6.5 9H3.5v6h3L11 19z"/><path d="M15.5 9a4.2 4.2 0 0 1 0 6"/>',
    mute: '<path d="M11 5 6.5 9H3.5v6h3L11 19z"/><path d="m16 9.5 5 5M21 9.5l-5 5"/>',
    sun: '<circle cx="12" cy="12" r="3.8"/><path d="M12 2.5v2M12 19.5v2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M2.5 12h2M19.5 12h2M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4"/>',
    'sun-dim': '<circle cx="12" cy="12" r="3.8"/><path d="M12 4.5v.01M12 19.5v.01M6.7 6.7v.01M17.3 17.3v.01M4.5 12h.01M19.5 12h.01M6.7 17.3v.01M17.3 6.7v.01"/>',
    'play-pause': '<path d="M4.5 6v12l8-6z"/><path d="M16 6v12M20 6v12"/>',
    play: '<path d="M7.5 5v14l11-7z"/>',
    next: '<path d="M5 6v12l9-6z"/><path d="M18 6v12"/>',
    prev: '<path d="M19 6v12l-9-6z"/><path d="M6 6v12"/>',
    stop: '<rect x="6" y="6" width="12" height="12" rx="2.2"/>',
    keyboard: '<rect x="2.5" y="6" width="19" height="12" rx="2.8"/><path d="M6.5 10h.01M10 10h.01M14 10h.01M17.5 10h.01M7.5 14h9"/>',
    globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>',
    apps: '<rect x="3.5" y="3.5" width="7" height="7" rx="1.8"/><rect x="13.5" y="3.5" width="7" height="7" rx="1.8"/><rect x="3.5" y="13.5" width="7" height="7" rx="1.8"/><path d="M17 13.5v7M13.5 17h7"/>',
    switch: '<rect x="3" y="8" width="13" height="12" rx="2.2"/><path d="M8 8V6a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2h-3"/>',
    terminal: '<rect x="3" y="4" width="18" height="16" rx="2.8"/><path d="m7.5 9.5 3 2.5-3 2.5M13 15h3.5"/>',
    lock: '<rect x="4.5" y="10.5" width="15" height="10" rx="2.6"/><path d="M8 10.5V7.5a4 4 0 0 1 8 0v3"/>',
    drag: '<path d="M9 11V5.5a1.5 1.5 0 0 1 3 0V10M12 10V4.5a1.5 1.5 0 0 1 3 0V10M15 10V6a1.5 1.5 0 0 1 3 0v7.5a6.5 6.5 0 0 1-6.5 6.5h-.6a6 6 0 0 1-4.6-2.2L3.6 14a1.6 1.6 0 0 1 2.4-2.1L9 14.5V11"/>',
    none: '<circle cx="12" cy="12" r="8.5"/><path d="m6 6 12 12"/>',
    repeat: '<path d="m17 2.5 3 3-3 3"/><path d="M4 11.5v-1a5 5 0 0 1 5-5h11"/><path d="m7 21.5-3-3 3-3"/><path d="M20 12.5v1a5 5 0 0 1-5 5H4"/>',
    warning: '<path d="M10.3 4.2 2.4 18a2 2 0 0 0 1.7 3h15.8a2 2 0 0 0 1.7-3L13.7 4.2a2 2 0 0 0-3.4 0z"/><path d="M12 9.5v4M12 17v.01"/>',
    check: '<path d="m5 12.5 4.5 4.5L19 7.5"/>',
    x: '<path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/>',
    plus: '<path d="M12 5v14M5 12h14"/>',
    folder: '<path d="M3 7.5a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
    external: '<path d="M14 4h6v6M20 4l-8.5 8.5M18 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4"/>',
    power: '<path d="M12 3v8"/><path d="M6.3 6.8a8 8 0 1 0 11.4 0"/>',
    reset: '<path d="M3.5 12a8.5 8.5 0 1 0 2.6-6.1L3.5 8.5"/><path d="M3.5 3.5v5h5"/>',
    trash: '<path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4.5h6V7"/>',
    zap: '<path d="M13 2.5 4.5 13.5H11l-1 8 8.5-11H12z"/>',
    chevron: '<path d="m9 6 6 6-6 6"/>',
    desktop: '<rect x="2.5" y="4" width="19" height="13" rx="2.2"/><path d="M8 20.5h8M12 17v3.5"/>',
    taskview: '<rect x="3" y="3.5" width="8" height="8" rx="1.8"/><rect x="13" y="3.5" width="8" height="8" rx="1.8"/><rect x="3" y="13.5" width="8" height="7" rx="1.8"/><rect x="13" y="13.5" width="8" height="7" rx="1.8"/>',
    'desk-next': '<rect x="2.5" y="5" width="13" height="14" rx="2.2"/><path d="m18.5 8.5 3 3.5-3 3.5"/>',
    'desk-prev': '<rect x="8.5" y="5" width="13" height="14" rx="2.2"/><path d="M5.5 8.5 2.5 12l3 3.5"/>',
    screenshot: '<path d="M3.5 8V5.5a2 2 0 0 1 2-2H8M16 3.5h2.5a2 2 0 0 1 2 2V8M20.5 16v2.5a2 2 0 0 1-2 2H16M8 20.5H5.5a2 2 0 0 1-2-2V16"/><circle cx="12" cy="12" r="3"/>',
    'close-window': '<rect x="3.5" y="3.5" width="17" height="17" rx="3"/><path d="m9.5 9.5 5 5M14.5 9.5l-5 5"/>',
    youtube: '<rect x="2.5" y="5.5" width="19" height="13" rx="4"/><path d="m10.5 9.5 4 2.5-4 2.5z"/>',
    browser: '<circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="3.4"/><path d="M12 8.6h8.4M9.05 13.7 4.8 6.4M14.95 13.7 10.7 20.9"/>',
    touchpad: '<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M3 15.5h18M12 15.5V20"/>',
    hold: '<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/>',
    tap: '<circle cx="12" cy="12" r="3"/><circle cx="12" cy="12" r="7.5" opacity=".5"/>',
    sparkle: '<path d="M12 3.5 13.8 10.2 20.5 12 13.8 13.8 12 20.5 10.2 13.8 3.5 12 10.2 10.2z"/>',
    copy: '<rect x="8.5" y="8.5" width="12" height="12" rx="2.2"/><path d="M15.5 8.5V5.5a2 2 0 0 0-2-2h-8a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h3"/>',
    record: '<circle cx="12" cy="12" r="8.5"/><circle cx="12" cy="12" r="3.5"/>',
    'arrow-up': '<path d="M12 19V5M6 11l6-6 6 6"/>',
    'arrow-down': '<path d="M12 5v14M6 13l6 6 6-6"/>',
    'arrow-left': '<path d="M19 12H5M11 6l-6 6 6 6"/>',
    'arrow-right': '<path d="M5 12h14M13 6l6 6-6 6"/>',
    github: '<path d="M9 19c-4.3 1.4-4.3-2.5-6-3M15 21v-3.5a3 3 0 0 0-.9-2.4c2.9-.3 6-1.4 6-6.4a5 5 0 0 0-1.4-3.5 4.6 4.6 0 0 0-.1-3.5s-1.1-.3-3.6 1.4a12.3 12.3 0 0 0-6.4 0C6.1 1.4 5 1.7 5 1.7a4.6 4.6 0 0 0-.1 3.5 5 5 0 0 0-1.4 3.5c0 5 3.1 6.1 6 6.4a3 3 0 0 0-.9 2.4V21"/>',
    'pinch-in': '<path d="m4 4 5.5 5.5M9.5 5v4.5H5M20 4l-5.5 5.5M14.5 5v4.5H19M4 20l5.5-5.5M5 14.5h4.5V19M20 20l-5.5-5.5M19 14.5h-4.5V19"/>',
    'pinch-out': '<path d="M9.5 9.5 4 4M4 8.5V4h4.5M14.5 9.5 20 4M15.5 4H20v4.5M9.5 14.5 4 20M4 15.5V20h4.5M14.5 14.5 20 20M20 15.5V20h-4.5"/>',
    'zoom-in': '<circle cx="10.5" cy="10.5" r="6.5"/><path d="m20 20-4.6-4.6M8 10.5h5M10.5 8v5"/>',
    'zoom-out': '<circle cx="10.5" cy="10.5" r="6.5"/><path d="m20 20-4.6-4.6M8 10.5h5"/>',
    layers: '<path d="m12 3.5 8.5 4.5-8.5 4.5L3.5 8z"/><path d="m3.5 12 8.5 4.5 8.5-4.5"/><path d="m3.5 16 8.5 4.5 8.5-4.5"/>',
    profile: '<rect x="3" y="4.5" width="18" height="15" rx="2.6"/><path d="M3 9h18M6.5 6.8v.01M9 6.8v.01"/><path d="M8 14h8"/>',
    pencil: '<path d="M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16z"/><path d="m13.5 6.5 4 4"/>',
    target: '<circle cx="12" cy="12" r="7.5"/><circle cx="12" cy="12" r="2.5"/><path d="M12 1.8v3M12 19.2v3M1.8 12h3M19.2 12h3"/>',
    tap2: '<circle cx="8.5" cy="12" r="2.6"/><circle cx="15.5" cy="12" r="2.6"/><path d="M4 7.2a7.5 7.5 0 0 0 0 9.6M20 7.2a7.5 7.5 0 0 1 0 9.6"/>',
    tap3: '<circle cx="6.5" cy="12" r="2.2"/><circle cx="12" cy="12" r="2.2"/><circle cx="17.5" cy="12" r="2.2"/><path d="M3 7.5a7.8 7.8 0 0 0 0 9M21 7.5a7.8 7.8 0 0 1 0 9"/>',
    command: '<path d="M9 6.5a2.5 2.5 0 1 0-2.5 2.5h11A2.5 2.5 0 1 0 15 6.5v11a2.5 2.5 0 1 0 2.5-2.5h-11A2.5 2.5 0 1 0 9 17.5z"/>',
    minimal: '<circle cx="12" cy="12" r="8.5"/><path d="M8 12h8"/>',
    graduation: '<path d="m2.5 9 9.5-4.5L21.5 9 12 13.5z"/><path d="M6.5 11v4.5c0 1.6 2.5 3 5.5 3s5.5-1.4 5.5-3V11M21.5 9v5"/>',
    gamepad: '<path d="M7.5 7.5h9a4.5 4.5 0 0 1 4.4 5.4l-.8 4a2.6 2.6 0 0 1-4.5 1.2L14 16h-4l-1.6 2.1a2.6 2.6 0 0 1-4.5-1.2l-.8-4A4.5 4.5 0 0 1 7.5 7.5z"/><path d="M8 10.5v3M6.5 12h3M15.5 11.5v.01M17.5 13v.01"/>',
    tray: '<path d="M3.5 14.5h4.5l1.5 2.5h5l1.5-2.5h4.5"/><path d="M5.5 5h13l2 9.5v4a1.5 1.5 0 0 1-1.5 1.5H5a1.5 1.5 0 0 1-1.5-1.5v-4z"/>',
  };

  P.icon = function icon(name, size) {
    const s = size || 16;
    const body = paths[name] || paths.none;
    return `<svg class="icon" width="${s}" height="${s}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">${body}</svg>`;
  };
  P.hasIcon = (name) => Object.prototype.hasOwnProperty.call(paths, name);
})();
