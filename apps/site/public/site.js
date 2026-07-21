/* ============================================================================
 * site.js — композитор лендинга: связывает шапку (поиск, язык, тема, бургер)
 * с виджетом рынков. Сам ничего не рисует, кроме мелкой ленты в первом экране.
 * ========================================================================== */
(function () {
  const $ = (s) => document.querySelector(s);

  I18n.apply();

  // ---- Рынки: один источник данных на таблицу и ленту в первом экране -------
  const loadInstruments = () => fetch('/instruments').then((r) => (r.ok ? r.json() : [])).catch(() => []);

  const markets = Markets.mount($('#marketsWidget'), {
    fetch: loadInstruments,
    t: I18n.t,
    limit: 8,
    interval: 5000,
  });

  // Лента первого экрана — те же данные, короткий срез.
  const ticker = $('#heroTicker');
  async function renderTicker() {
    const list = await loadInstruments();
    if (!Array.isArray(list) || !list.length) return;
    ticker.innerHTML = list.slice(0, 5).map((i) => {
      const [base, quote] = i.symbol.split('-');
      const pd = i.price_decimals ?? 2;
      const px = (i.last / 10 ** pd).toLocaleString('en-US', { minimumFractionDigits: pd, maximumFractionDigits: pd });
      const up = i.change >= 0;
      return `<div class="ticker__row">
        <span class="ticker__sym">${base}<small>${quote}</small></span>
        <span class="ticker__px">${px}</span>
        <span class="ticker__chg ${up ? 'up' : 'down'}">${up ? '+' : ''}${(i.change ?? 0).toFixed(2)}%</span>
      </div>`;
    }).join('');
  }
  renderTicker();
  setInterval(renderTicker, 5000);

  // ---- Поиск ---------------------------------------------------------------
  const search = $('#search'), input = $('#searchInput');
  const openSearch = (on) => {
    search.classList.toggle('hidden', !on);
    if (on) input.focus(); else { input.value = ''; markets.filter(''); }
  };
  $('#searchBtn').addEventListener('click', () => openSearch(search.classList.contains('hidden')));
  $('#searchClose').addEventListener('click', () => openSearch(false));
  input.addEventListener('input', () => {
    markets.filter(input.value);
    // Найденное должно быть видно сразу, без ручной прокрутки.
    if (input.value.trim()) $('#markets').scrollIntoView({ behavior: 'smooth', block: 'start' });
  });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && !search.classList.contains('hidden')) openSearch(false);
  });

  // ---- Язык ----------------------------------------------------------------
  const langMenu = $('#langMenu'), langLabel = $('#langLabel');
  const paintLang = () => {
    langLabel.textContent = I18n.lang().toUpperCase();
    langMenu.querySelectorAll('[data-lang]').forEach((b) => b.classList.toggle('active', b.dataset.lang === I18n.lang()));
  };
  $('#langBtn').addEventListener('click', (e) => { e.stopPropagation(); langMenu.classList.toggle('open'); });
  langMenu.querySelectorAll('[data-lang]').forEach((b) => b.addEventListener('click', () => {
    I18n.setLang(b.dataset.lang); langMenu.classList.remove('open');
  }));
  document.addEventListener('click', () => langMenu.classList.remove('open'));
  I18n.onChange(() => { paintLang(); markets.relabel(); });
  paintLang();

  // ---- Тема ----------------------------------------------------------------
  $('#themeBtn').addEventListener('click', () => Theme.toggle());

  // ---- Ссылки на терминал --------------------------------------------------
  // Терминал — отдельный продукт на своём порту (ADR-017). Хост берём текущий,
  // чтобы ссылка работала и локально, и с любой другой машины в сети.
  document.querySelectorAll('[data-i18n="cta.terminal"]').forEach((a) => {
    a.href = `${location.protocol}//${location.hostname}:8888/`;
    a.target = '_blank'; a.rel = 'noopener';
  });

  // ---- Мобильное меню ------------------------------------------------------
  const nav = $('#nav');
  $('#burger').addEventListener('click', (e) => { e.stopPropagation(); nav.classList.toggle('open'); });
  nav.addEventListener('click', (e) => { if (e.target.tagName === 'A') nav.classList.remove('open'); });
})();
