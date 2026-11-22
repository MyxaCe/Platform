'use strict';
(function () {
  const THEME_KEY = 'terminal:theme';
  const BRAND_KEY = 'terminal:brand.lastgood';
  const REFRESH_MS = 5 * 60 * 1000;

  const root = document.documentElement;
  function normTheme(t) {
    return t === 'light' || t === 'dark' ? t : null;
  }
  function currentTheme() {
    return root.getAttribute('data-theme') || 'dark';
  }
  function setTheme(t) {
    const v = normTheme(t) || 'dark';
    root.setAttribute('data-theme', v);
    try { localStorage.setItem(THEME_KEY, v); } catch {  }
    document.dispatchEvent(new Event('themechange'));
  }
  function toggleTheme() {
    setTheme(currentTheme() === 'dark' ? 'light' : 'dark');
  }
  (function initTheme() {
    const q = new URLSearchParams(location.search).get('theme');
    let saved = null;
    try { saved = localStorage.getItem(THEME_KEY); } catch {  }
    root.setAttribute('data-theme', normTheme(q) || normTheme(saved) || 'dark');
  })();

  /**
   * @param {string} hex
   * @returns {[number, number, number] | null}
   */
  function hexToChannels(hex) {
    const m = /^#([0-9a-fA-F]{6})$/.exec(hex);
    if (!m) return null;
    const n = parseInt(m[1], 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }
  function luminance([r, g, b]) {
    const f = (c) => { c /= 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
    return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
  }
  function lighten([r, g, b], k) {
    const up = (c) => Math.round(c + (255 - c) * k);
    return [up(r), up(g), up(b)];
  }

  function readImage(v) {
    if (!v || typeof v !== 'object') return null;
    if (typeof v.url !== 'string' || v.url.length === 0) return null;
    return { url: v.url, alt: typeof v.alt === 'string' ? v.alt : '' };
  }

  function readColor(v) {
    return typeof v === 'string' && /^#[0-9a-fA-F]{6}$/.test(v) ? v : null;
  }

  function readName(v) {
    return typeof v === 'string' && v.trim() !== '' ? v : null;
  }

  function normalizeBrand(raw) {
    if (!raw || typeof raw !== 'object') return null;

    const v2 = !!raw.brand && typeof raw.brand === 'object'
      && !!raw.site && typeof raw.site === 'object';
    const src = v2 ? raw.brand : raw;
    const name = readName(v2 ? raw.site.name : raw.name);

    if (!v2 && name === null) return null;

    const brand = {
      shape: v2 ? 'v2' : 'legacy',
      name: name,
      primaryColor: readColor(src.primaryColor),
      logoLight: readImage(v2 ? src.logoLight : src.logo),
      logoDark: v2 ? readImage(src.logoDark) : null,
      favicon: readImage(src.favicon),
    };

    if (!brand.name && !brand.primaryColor && !brand.logoLight && !brand.favicon) {
      return null;
    }

    return brand;
  }

  const LIGHT_LOGO_WORKS_ON_DARK = false;

  function lightLogoUsableOnDark(brand) {
    return brand.shape === 'legacy' ? true : LIGHT_LOGO_WORKS_ON_DARK;
  }

  function pickLogo(brand) {
    if (currentTheme() !== 'dark') return brand.logoLight;
    if (brand.logoDark) return brand.logoDark;
    return lightLogoUsableOnDark(brand) ? brand.logoLight : null;
  }

  let applied = null;

  function renderMark(brand) {
    const mark = document.querySelector('.logo__mark');
    if (!mark) return;

    const logo = pickLogo(brand);
    if (logo) {
      const cur = mark.querySelector('img');
      if (cur && cur.getAttribute('src') === logo.url) return;
      mark.innerHTML = '';
      const img = document.createElement('img');
      img.src = logo.url;
      img.alt = logo.alt || brand.name || '';
      mark.appendChild(img);
      return;
    }

    if (brand.name) mark.textContent = brand.name.trim().charAt(0).toUpperCase();
  }

  function applyBrand(raw) {
    const brand = normalizeBrand(raw);
    if (!brand) return false;
    if (window.I18n) window.I18n.setSite(raw);

    if (brand.primaryColor) {
      const ch = hexToChannels(brand.primaryColor);
      if (ch) {
        root.style.setProperty('--accent', ch.join(' '));
        root.style.setProperty('--accent-hover', lighten(ch, 0.12).join(' '));
        root.style.setProperty('--on-accent', luminance(ch) > 0.4 ? '#08090c' : '#f5f6f8');
      }
    }

    if (brand.name) {
      const brandEl = document.getElementById('brandName');
      if (brandEl) brandEl.textContent = brand.name;
      document.title = brand.name + ' · Terminal';
    }

    applied = brand;
    renderMark(brand);

    const fav = brand.favicon || brand.logoLight;
    if (fav) {
      let link = /** @type {HTMLLinkElement} */ (document.querySelector('link[rel="icon"]'));
      if (!link) { link = document.createElement('link'); link.rel = 'icon'; document.head.appendChild(link); }
      link.href = fav.url;
    }

    document.dispatchEvent(new Event('themechange'));
    return true;
  }

  document.addEventListener('themechange', function () {
    if (applied) renderMark(applied);
  });

  function cacheBrand(b) {
    try { localStorage.setItem(BRAND_KEY, JSON.stringify(b)); } catch {  }
  }
  function loadCached() {
    try {
      const raw = localStorage.getItem(BRAND_KEY);
      if (raw) applyBrand(JSON.parse(raw));
    } catch {  }
  }
  function brandRequestUrl(cfg) {
    const base = String((cfg && cfg.brandUrl) || '').trim();
    if (!base) return null;

    const site = (cfg && cfg.site)
      || (typeof location !== 'undefined' ? new URLSearchParams(location.search).get('site') : '')
      || '';

    if (base.indexOf('{site}') !== -1) {
      if (!site) return null;
      return base.replace(/\{site\}/g, encodeURIComponent(site));
    }

    const qs = new URLSearchParams();
    if (site) qs.set('site', site);
    qs.set('locale', (cfg && cfg.locale) || 'en');
    return `${base.replace(/\/$/, '')}?${qs}`;
  }

  function noBrand() {
    if (!applied && window.I18n) window.I18n.setSite(null);
  }

  async function fetchBrand(cfg) {
    const url = brandRequestUrl(cfg);
    if (!url) { noBrand(); return; }
    try {
      const r = await fetch(url, { credentials: 'omit' });
      if (!r.ok) { noBrand(); return; }
      const brand = await r.json();
      if (applyBrand(brand)) cacheBrand(brand);
      else noBrand();
    } catch { noBrand(); }
  }

  let timer = null;
  function init(overrides) {
    const cfg = Object.assign({ brandUrl: '', site: '', locale: 'en' }, window.TERMINAL_CONFIG || {}, overrides || {});
    loadCached();
    fetchBrand(cfg);
    if (cfg.brandUrl) {
      if (timer) clearInterval(timer);
      timer = setInterval(() => fetchBrand(cfg), REFRESH_MS);
    }
    return { setTheme, toggleTheme, applyBrand, refresh: () => fetchBrand(cfg) };
  }

  window.Theming = {
    init, applyBrand, setTheme, toggleTheme, currentTheme, normalizeBrand, brandRequestUrl,
  };
})();
