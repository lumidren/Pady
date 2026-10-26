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
