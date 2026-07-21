/* ============================================================================
 * site.js — композитор лендинга: связывает шапку (поиск, язык, тема, бургер)
 * с виджетом рынков. Сам ничего не рисует, кроме мелкой ленты в первом экране.
 * ========================================================================== */
(function () {
  const $ = (s) => document.querySelector(s);

  const nav = $('#nav');

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
  I18n.onChange(() => { paintLang(); markets.relabel(); auth.relabel(); refreshMe(); });
  paintLang();

  // ---- Тема ----------------------------------------------------------------
  $('#themeBtn').addEventListener('click', () => Theme.toggle());

  // ---- Окна входа и регистрации --------------------------------------------
  // Сессия живёт в HttpOnly-cookie (ADR-018): JavaScript её не видит и не может
  // потерять при XSS. Отсюда и «кто я» спрашиваем у сервера, а не у localStorage.
  const auth = Auth.mount(document.body, {
    t: I18n.t,
    submit: async (mode, data) => {
      const url = mode === 'register' ? '/auth/register' : '/auth/login';
      let r;
      try {
        r = await fetch(url, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify(data),
        });
      } catch {
        return { ok: false, message: I18n.t('auth.err.net') };
      }
      if (r.ok) return { ok: true, message: I18n.t(mode === 'register' ? 'auth.ok.register' : 'auth.ok.login') };
      // Текст ошибки берём свой, по коду ответа: сервер отвечает по-английски,
      // а сообщение пользователю должно быть на языке страницы.
      const key = r.status === 409 ? 'auth.err.taken'
        : r.status === 429 ? 'auth.err.rate'
        : r.status === 401 ? 'auth.err.creds'
        : r.status === 400 ? 'auth.err.pass'
        : 'auth.err.net';
      return { ok: false, message: I18n.t(key) };
    },
    onSuccess: () => { setTimeout(() => { auth.close(); refreshMe(); }, 700); },
  });

  // ---- Состояние сессии в шапке -------------------------------------------
  const hdrRight = document.querySelector('.hdr__right');
  const btnLogin = document.querySelector('.hdr__login');
  const btnReg = document.querySelector('.hdr__right .btn--accent');
  const who = document.createElement('span');
  who.className = 'hdr__who hidden';
  const btnOut = document.createElement('button');
  btnOut.className = 'btn btn--ghost hidden';
  hdrRight.insertBefore(who, btnLogin);
  hdrRight.insertBefore(btnOut, btnLogin);

  async function refreshMe() {
    let me = null;
    try { const r = await fetch('/auth/me'); if (r.ok) me = await r.json(); } catch { /* не вошли */ }
    const on = !!(me && me.email);
    who.textContent = on ? me.email : '';
    who.classList.toggle('hidden', !on);
    btnOut.textContent = I18n.t('cta.logout');
    btnOut.classList.toggle('hidden', !on);
    btnLogin.classList.toggle('hidden', on);
    btnReg.classList.toggle('hidden', on);
  }
  btnOut.addEventListener('click', async () => {
    try { await fetch('/auth/logout', { method: 'POST' }); } catch { /* всё равно перерисуем */ }
    refreshMe();
  });
  refreshMe();
  // Кнопки шапки, дубль «Входа» в бургер-меню и призывы на странице.
  document.querySelectorAll('[data-i18n="cta.login"]').forEach((b) =>
    b.addEventListener('click', (e) => { e.preventDefault(); nav.classList.remove('open'); auth.open('login'); }));
  document.querySelectorAll('[data-i18n="cta.register"], [data-i18n="cta.start"]').forEach((b) =>
    b.addEventListener('click', (e) => { e.preventDefault(); auth.open('register'); }));

  // ---- Ссылки на терминал --------------------------------------------------
  // Терминал — отдельный продукт на своём порту (ADR-017). Хост берём текущий,
  // чтобы ссылка работала и локально, и с любой другой машины в сети.
  document.querySelectorAll('[data-i18n="cta.terminal"]').forEach((a) => {
    a.href = `${location.protocol}//${location.hostname}:8888/`;
    a.target = '_blank'; a.rel = 'noopener';
  });

  // ---- Мобильное меню ------------------------------------------------------
  $('#burger').addEventListener('click', (e) => { e.stopPropagation(); nav.classList.toggle('open'); });
  nav.addEventListener('click', (e) => { if (e.target.tagName === 'A') nav.classList.remove('open'); });
})();
