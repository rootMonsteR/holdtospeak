// Fill in the latest release (version, asset links, sizes) from GitHub's public API.
// Static fallbacks in the HTML point at the last known release, so the page is complete without JS.
(function () {
  var API = 'https://api.github.com/repos/rootMonsteR/holdtospeak/releases/latest';
  function mb(bytes) { return (bytes / 1048576).toFixed(1) + ' MB'; }
  function setAll(sel, fn) { document.querySelectorAll(sel).forEach(fn); }

  fetch(API, { headers: { Accept: 'application/vnd.github+json' } })
    .then(function (r) { return r.ok ? r.json() : Promise.reject(r.status); })
    .then(function (rel) {
      var tag = rel.tag_name || '';
      var ver = tag.replace(/^v/, '');
      if (!/^\d+\.\d+\.\d+/.test(ver)) return;
      var assets = rel.assets || [];
      var setup = assets.find(function (a) { return /-setup\.exe$/i.test(a.name); });
      var zip = assets.find(function (a) { return /\.zip$/i.test(a.name); });

      setAll('[data-dl-version]', function (el) { el.textContent = 'v' + ver; });
      setAll('[data-dl-ver]', function (el) { el.textContent = ver; });
      if (setup) {
        setAll('[data-dl="setup"]', function (el) { el.href = setup.browser_download_url; });
        setAll('[data-dl-size="setup"]', function (el) { el.textContent = mb(setup.size); });
        setAll('[data-dl-meta]', function (el) { el.textContent = 'v' + ver + ' · ' + mb(setup.size); });
      }
      if (zip) {
        setAll('[data-dl="zip"]', function (el) { el.href = zip.browser_download_url; });
        setAll('[data-dl-size="zip"]', function (el) { el.textContent = mb(zip.size); });
      }
    })
    .catch(function () { /* offline or rate-limited: the static links stay */ });
})();
