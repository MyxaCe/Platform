'use strict';
// ============================================================================
// theming.js — адаптер темизации терминала под theming-пакет платформы (v1).
// Фаза Т1 интеграции (ADR-023). Контракт — theming-pack/README.md.
//
// Что делает:
//   1) выставляет тему через атрибут data-theme (dark по умолчанию, light);
//   2) грузит бренд сайта из CMS (GET /v1/cms/brand), валидирует по контракту,
//      применяет accent-цвет, логотип и имя; кеширует last-good;
//   3) автономность: без CMS терминал остаётся на палитре платформы (tokens.v1.css) —
//      «никогда не раскрашиваться мусором».
//
// Цвета платформы приходят RGB-каналами (tokens.v1.css). Бренд переопределяет
// канал --accent на :root — этого достаточно, чтобы перекрасился весь UI и график
// (мост в style.css: --brand: rgb(var(--accent)), график читает канал напрямую).
// ============================================================================
(function () {
  const THEME_KEY = 'terminal:theme';
  const BRAND_KEY = 'terminal:brand.lastgood';
  const REFRESH_MS = 5 * 60 * 1000;

  // ---- Тема ----------------------------------------------------------------
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
    try { localStorage.setItem(THEME_KEY, v); } catch { /* приватный режим */ }
    document.dispatchEvent(new Event('themechange'));
  }
  function toggleTheme() {
    setTheme(currentTheme() === 'dark' ? 'light' : 'dark');
  }
  // Первичная тема — синхронно, ДО построения графика в app.js: приоритет
  // ?theme= (сайт передаёт выбор пользователя) → сохранённая → dark.
  (function initTheme() {
    const q = new URLSearchParams(location.search).get('theme');
    let saved = null;
    try { saved = localStorage.getItem(THEME_KEY); } catch { /* */ }
    root.setAttribute('data-theme', normTheme(q) || normTheme(saved) || 'dark');
  })();

  // ---- Цвет: hex → каналы, яркость, осветление -----------------------------
  function hexToChannels(hex) {
    const m = /^#([0-9a-fA-F]{6})$/.exec(hex);
    if (!m) return null;
    const n = parseInt(m[1], 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }
  // Относительная яркость (sRGB) — чтобы выбрать читаемый текст поверх акцента.
  function luminance([r, g, b]) {
    const f = (c) => { c /= 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
    return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
  }
  function lighten([r, g, b], k) {
    const up = (c) => Math.round(c + (255 - c) * k);
    return [up(r), up(g), up(b)];
  }

  // ---- Валидация ответа CMS ------------------------------------------------
  // Не полноценный JSON-Schema-валидатор: проверяем критичные поля контракта
  // (cms.brand.schema.json). Невалидное — отвергаем целиком, бренд не меняем.
  function validLogo(l) {
    if (l === null) return true;
    return l && typeof l === 'object' && typeof l.url === 'string' && l.url.length > 0
      && Number.isFinite(l.width) && Number.isFinite(l.height) && typeof l.alt === 'string';
  }
  function validBrand(b) {
    return b && typeof b === 'object'
      && typeof b.name === 'string' && b.name.length > 0
      && /^#[0-9a-fA-F]{6}$/.test(b.primaryColor || '')
      && validLogo(b.logo === undefined ? null : b.logo);
  }

  // ---- Применение бренда ---------------------------------------------------
  function applyBrand(brand) {
    if (!validBrand(brand)) return false;

    const ch = hexToChannels(brand.primaryColor);
    if (ch) {
      root.style.setProperty('--accent', ch.join(' '));
      root.style.setProperty('--accent-hover', lighten(ch, 0.12).join(' '));
      // Тёмный акцент → светлый текст на нём, и наоборот (порог по яркости).
      root.style.setProperty('--on-accent', luminance(ch) > 0.4 ? '#08090c' : '#f5f6f8');
    }

    const name = brand.name;
    const brandEl = document.getElementById('brandName');
    if (brandEl) brandEl.textContent = name;
    if (name) document.title = name + ' · Terminal';

    const mark = document.querySelector('.logo__mark');
    if (mark) {
      if (brand.logo && brand.logo.url) {
        mark.innerHTML = '';
        const img = document.createElement('img');
        img.src = brand.logo.url;
        img.alt = brand.logo.alt || name || '';
        mark.appendChild(img);
      } else {
        mark.textContent = (name || 'T').trim().charAt(0).toUpperCase();
      }
    }

    const fav = brand.favicon || brand.logo;
    if (fav && fav.url) {
      let link = document.querySelector('link[rel="icon"]');
      if (!link) { link = document.createElement('link'); link.rel = 'icon'; document.head.appendChild(link); }
      link.href = fav.url;
    }

    document.dispatchEvent(new Event('themechange')); // график перечитает акцент
    return true;
  }

  // ---- Загрузка из CMS -----------------------------------------------------
  function cacheBrand(b) {
    try { localStorage.setItem(BRAND_KEY, JSON.stringify(b)); } catch { /* */ }
  }
  function loadCached() {
    try {
      const raw = localStorage.getItem(BRAND_KEY);
      if (raw) applyBrand(JSON.parse(raw));
    } catch { /* нет валидного кеша — остаёмся на палитре платформы */ }
  }
  async function fetchBrand(cfg) {
    if (!cfg.brandUrl) return; // без прокси работаем на дефолтном бренде платформы
    // Тенант приходит из ?site= (обязателен, ADR-023). CMS-ключ в браузер НЕ кладём:
    // brand запрашивается через gateway-прокси платформы, ключ у него server-side.
    const site = cfg.site || new URLSearchParams(location.search).get('site') || '';
    const qs = new URLSearchParams();
    if (site) qs.set('site', site);
    qs.set('locale', cfg.locale || 'en');
    const url = `${cfg.brandUrl.replace(/\/$/, '')}?${qs}`;
    try {
      const r = await fetch(url, { credentials: 'omit' });
      if (!r.ok) return;
      const brand = await r.json();
      if (applyBrand(brand)) cacheBrand(brand); // кешируем только валидный (last-good)
    } catch { /* прокси/сеть недоступны — держим текущий бренд */ }
  }

  // ---- Инициализация -------------------------------------------------------
  let timer = null;
  function init(overrides) {
    // brandUrl — полный путь к brand-прокси платформы (напр. http://localhost:3002/v1/cms/brand).
    // Ключа CMS здесь нет и быть не должно — он живёт на стороне прокси (server-side).
    const cfg = Object.assign({ brandUrl: '', site: '', locale: 'en' }, window.TERMINAL_CONFIG || {}, overrides || {});
    loadCached();              // мгновенно показать прошлый валидный бренд
    fetchBrand(cfg);           // и обновить из CMS через прокси платформы
    if (cfg.brandUrl) {
      if (timer) clearInterval(timer);
      timer = setInterval(() => fetchBrand(cfg), REFRESH_MS);
    }
    return { setTheme, toggleTheme, applyBrand, refresh: () => fetchBrand(cfg) };
  }

  window.Theming = { init, applyBrand, setTheme, toggleTheme, currentTheme };
})();
