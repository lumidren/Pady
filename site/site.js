// Points the download buttons at the right file of the latest release for
// this visitor's system. Without the GitHub API (offline, rate limited) the
// buttons keep their link to the releases page.
(function () {
  'use strict';
  const REPO = 'lumidren/Pady';
  const ua = navigator.userAgent;
  const os = /Windows/i.test(ua) ? 'windows' : /Linux|X11/i.test(ua) && !/Android/i.test(ua) ? 'linux' : 'other';
  const linuxFlavor = /Fedora|Red Hat|openSUSE|SUSE/i.test(ua) ? 'rpm' : 'deb';

  const mb = (bytes) => (bytes / 1048576).toFixed(1) + ' MB';
  const link = (asset, text) => `<a href="${asset.browser_download_url}">${text}</a>`;

  fetch(`https://api.github.com/repos/${REPO}/releases/latest`)
    .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
    .then((release) => {
      const find = (re) => release.assets.find((a) => re.test(a.name));
      const files = {
        setup: find(/x64-setup\.exe$/), msi: find(/\.msi$/),
        deb: find(/\.deb$/), rpm: find(/\.rpm$/), appimage: find(/\.AppImage$/),
      };
      const version = release.tag_name;
      const main = os === 'windows' ? files.setup : os === 'linux' ? files[linuxFlavor] || files.appimage : null;
      const label = { setup: 'Windows', deb: 'Linux (.deb)', rpm: 'Linux (.rpm)', appimage: 'Linux (AppImage)' };
      const which = Object.keys(files).find((k) => files[k] === main);

      if (main) {
        for (const id of ['dl-main', 'dl-final']) document.getElementById(id).href = main.browser_download_url;
        document.getElementById('dl-label').textContent = `Download for ${label[which]}`;
        document.getElementById('dl-meta').textContent = `${version} · ${mb(main.size)}`;
      } else {
        document.getElementById('dl-meta').textContent = `${version} · for Windows and Linux`;
      }
      const others = [
        files.setup && files.setup !== main && link(files.setup, 'Windows'),
        files.msi && link(files.msi, 'Windows MSI'),
        files.deb && files.deb !== main && link(files.deb, '.deb'),
        files.rpm && files.rpm !== main && link(files.rpm, '.rpm'),
        files.appimage && files.appimage !== main && link(files.appimage, 'AppImage'),
      ].filter(Boolean);
      document.getElementById('dl-others').innerHTML =
        'Also for ' + others.join(' · ') + ` · <a href="${release.html_url}">release notes</a>`;
    })
    .catch(() => {});
})();

// Download section: real file links and sizes, distro tabs, copy buttons.
(function () {
  'use strict';
  const ua = navigator.userAgent;
  const yours = /Windows/i.test(ua) ? 'windows' : /Linux|X11/i.test(ua) && !/Android/i.test(ua) ? 'linux' : null;
  if (yours) document.querySelector(`.dl-card[data-os="${yours}"]`).classList.add('is-yours');

  const tabs = [...document.querySelectorAll('.tabs [data-tab]')];
  const pick = (name) => {
    for (const t of tabs) t.setAttribute('aria-selected', String(t.dataset.tab === name));
    for (const p of document.querySelectorAll('.tab')) p.hidden = p.dataset.panel !== name;
  };
  for (const t of tabs) t.addEventListener('click', () => pick(t.dataset.tab));
  if (/Fedora|Red Hat|openSUSE|SUSE/i.test(ua)) pick('rpm');

  for (const b of document.querySelectorAll('.copy')) {
    b.addEventListener('click', () => {
      navigator.clipboard.writeText(b.previousElementSibling.textContent).then(() => {
        b.textContent = 'Copied';
        setTimeout(() => (b.textContent = 'Copy'), 1500);
      }, () => {});
    });
  }

  const patterns = { setup: /x64-setup\.exe$/, msi: /\.msi$/, deb: /\.deb$/, rpm: /\.rpm$/, appimage: /\.AppImage$/ };
  fetch('https://api.github.com/repos/lumidren/Pady/releases/latest')
    .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
    .then((release) => {
      for (const a of document.querySelectorAll('.dl-file[data-asset]')) {
        const asset = release.assets.find((x) => patterns[a.dataset.asset].test(x.name));
        if (!asset) continue;
        a.href = asset.browser_download_url;
        if (a.classList.contains('btn')) {
          const size = (asset.size / 1048576).toFixed(1) + ' MB';
          const small = a.querySelector('small');
          if (small) small.textContent += ' · ' + size;
          else a.insertAdjacentHTML('beforeend', `<small>${asset.name} · ${size}</small>`);
        }
      }
      document.getElementById('dl-version').textContent =
        `Pady ${release.tag_name}, free for Windows 10/11 and Linux. Every file comes with SHA-256 checksums.`;
    })
    .catch(() => {});
})();
