/* ============================================================================
 * cabinet.js — личный кабинет: обзор счёта, позиции, история, профиль.
 *
 * Доступ определяет сервер: сначала спрашиваем GET /auth/me по сессионной cookie
 * (ADR-018). Пока ответ не пришёл, содержимое скрыто — иначе на секунду мелькнёт
 * кабинет с пустыми полями. Не вошёл — показываем приглашение войти; форма входа
 * живёт на сайте, дублировать её здесь не нужно.
 *
 * Все цифры — настоящие, из шлюза: /account, /deals, /pending, /deals/closed.
 * ========================================================================== */
(function () {
  const $ = (s) => document.querySelector(s);
  const SITE_PORT = 8889, TERMINAL_PORT = 8888;
  const hostUrl = (port) => `${location.protocol}//${location.hostname}:${port}/`;

  // Знак перед валютой: «$-0.06» читается как опечатка, правильно «-$0.06».
  const usd = (cents) => (cents < 0 ? '-' : '') + '$' +
    (Math.abs(cents) / 100).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  const px = (raw, d) => (raw / 10 ** d).toFixed(d);
  const lot = (raw, d) => (raw / 10 ** d).toFixed(d);
  const t = I18n.t;

  const get = async (url) => {
    try { const r = await fetch(url); return r.ok ? await r.json() : null; } catch { return null; }
  };

  // ---- Доступ --------------------------------------------------------------
  let me = null;

  async function boot() {
    me = await get('/auth/me');
    $('#boot').classList.add('hidden');
    if (!me || !me.email) { $('#gate').classList.remove('hidden'); return; }
    $('#app').classList.remove('hidden');
    $('#who').textContent = me.email;
    $('#pfEmail').textContent = me.email;
    $('#pfId').textContent = me.user_id;
    refresh();
    setInterval(refresh, 4000);
  }

  $('#gateLogin').href = hostUrl(SITE_PORT);
  $('#toTerminal').href = hostUrl(TERMINAL_PORT);
  $('#toTerminal').target = '_blank';
  $('#toTerminal').rel = 'noopener';

  // ---- Данные --------------------------------------------------------------
  const STATS = [
    ['st.balance', 'balance'], ['st.equity', 'equity'], ['st.margin', 'used_margin'],
    ['st.free', 'free_margin'], ['st.pnl', 'open_pnl'],
  ];

  function renderStats(a) {
    $('#stats').innerHTML = STATS.map(([key, field]) => {
      const v = a[field] ?? 0;
      const tone = field === 'open_pnl' ? (v > 0 ? ' up' : v < 0 ? ' down' : '') : '';
      return `<div class="stat"><span class="stat__l">${t(key)}</span><span class="stat__v${tone}">${usd(v)}</span></div>`;
    }).join('');
  }

  const head = (cols) => `<div class="row row--head">${cols.map((c) => `<span>${t(c)}</span>`).join('')}</div>`;
  const empty = (key) => `<div class="row row--empty"><span>${t(key)}</span></div>`;

  function renderDeals(el, list) {
    if (!list) return;
    el.innerHTML = head(['col.instr', 'col.side', 'col.qty', 'col.entry', 'col.price', 'col.pnl']) +
      (list.map((d) => {
        const pos = d.pnl >= 0;
        return `<div class="row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span>` +
          `<span>${lot(d.qty, d.qty_decimals)}</span><span>${px(d.entry, d.price_decimals)}</span>` +
          `<span>${px(d.mark, d.price_decimals)}</span><span class="${pos ? 'up' : 'down'}">${usd(d.pnl)}</span></div>`;
      }).join('') || empty('empty.deals'));
  }

  function renderPending(el, list) {
    if (!list) return;
    el.innerHTML = head(['col.instr', 'col.side', 'col.qty', 'col.price', 'col.sl', 'col.tp']) +
      (list.map((d) => {
        const f = (v) => (v == null ? '—' : px(v, d.price_decimals));
        return `<div class="row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span>` +
          `<span>${lot(d.qty, d.qty_decimals)}</span><span>${f(d.price)}</span><span>${f(d.sl)}</span><span>${f(d.tp)}</span></div>`;
      }).join('') || empty('empty.pending'));
  }

  function renderHistory(el, list) {
    if (!list) return;
    el.innerHTML = head(['col.instr', 'col.side', 'col.qty', 'col.entry', 'col.exit', 'col.pnl']) +
      (list.slice().reverse().map((d) => {
        const pos = d.pnl >= 0;
        return `<div class="row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span>` +
          `<span>${lot(d.qty, d.qty_decimals)}</span><span>${px(d.entry, d.price_decimals)}</span>` +
          `<span>${px(d.exit, d.price_decimals)}</span><span class="${pos ? 'up' : 'down'}">${usd(d.pnl)}</span></div>`;
      }).join('') || empty('empty.history'));
  }

  async function refresh() {
    const [account, deals, pending, closed] = await Promise.all([
      get('/account'), get('/deals'), get('/pending'), get('/deals/closed'),
    ]);
    if (account) renderStats(account);
    renderDeals($('#ovDeals'), deals);
    renderDeals($('#posDeals'), deals);
    renderPending($('#posPending'), pending);
    renderHistory($('#histRows'), closed);
  }

  // ---- Вкладки -------------------------------------------------------------
  $('#tabs').addEventListener('click', (e) => {
    const b = e.target.closest('[data-tab]'); if (!b) return;
    document.querySelectorAll('#tabs button').forEach((x) => x.classList.toggle('active', x === b));
    document.querySelectorAll('.pane').forEach((p) => p.classList.toggle('hidden', p.dataset.pane !== b.dataset.tab));
  });

  // ---- Язык, тема, выход ---------------------------------------------------
  const langMenu = $('#langMenu');
  const paintLang = () => {
    $('#langLabel').textContent = I18n.lang().toUpperCase();
    langMenu.querySelectorAll('[data-lang]').forEach((b) => b.classList.toggle('active', b.dataset.lang === I18n.lang()));
  };
  $('#langBtn').addEventListener('click', (e) => { e.stopPropagation(); langMenu.classList.toggle('open'); });
  langMenu.querySelectorAll('[data-lang]').forEach((b) => b.addEventListener('click', () => { I18n.setLang(b.dataset.lang); langMenu.classList.remove('open'); }));
  document.addEventListener('click', () => langMenu.classList.remove('open'));
  I18n.onChange(() => { paintLang(); refresh(); });
  paintLang();

  $('#themeBtn').addEventListener('click', () => Theme.toggle());
  $('#logout').addEventListener('click', async () => {
    try { await fetch('/auth/logout', { method: 'POST' }); } catch { /* всё равно уводим на гейт */ }
    location.reload();
  });

  boot();
})();
