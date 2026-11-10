/* ============================================================================
 * positions.js — нижняя панель терминала (ADR-015): вкладки OPEN DEALS /
 * LIMIT ORDERS / CLOSED DEALS / SIGNALS со своим DOM, стилями и опросом.
 *
 *   const panel = Positions.mount(el, {
 *     api,                       // { deals, closedDeals, pending, closeDeal, cancelPending }
 *     fmtUsd: (cents) => '$1.00',
 *     pips: () => false,         // показывать ли пипсы в P&L (тумблер хоста)
 *     onDeals: (list) => {},     // хост получает позиции (рисует SL/TP-линии на графике)
 *     onChange: () => {},        // после закрытия сделки / отмены ордера — обновить метрики
 *     signals: () => [{ symbol, name, tf, dir }],  // сигналы считает хост (из свечей)
 *     balance: () => [[label, value, cls?]] | null, // вкладка «Баланс» (опц.): сводку счёта даёт хост
 *     t: (key, en) => string,    // перевод (опц.): без него — английский
 *     interval: 1500,
 *   });
 *   panel.refresh(); panel.refreshSignals(); panel.show('pending'); panel.destroy();
 *
 * Почему сигналы приходят снаружи: они считаются из свечей графика, которыми владеет
 * хост. Виджет отвечает за их показ, но не за расчёт.
 * ========================================================================== */
(function () {
  const CSS = `
.bottom__tabs { padding: 0 14px; height: 36px; flex: 0 0 36px; display: flex; align-items: stretch; gap: 20px; border-bottom: 1px solid var(--border,#1f2735); overflow-x: auto; scrollbar-width: none; }
.bottom__tabs > span { display: inline-flex; align-items: center; gap: 5px; color: var(--muted,#6b7688); font-weight: 700; font-size: 12.5px; cursor: pointer; border-bottom: 2px solid transparent; white-space: nowrap; }
.bottom__tabs > span.active { color: var(--text,#d7dce5); border-bottom-color: var(--brand,#f0b90b); }
.bottom__tabs .cnt { font-size: 10.5px; font-weight: 700; color: var(--muted,#6b7688); background: var(--panel3,#1a2130); border-radius: 8px; padding: 0 6px; min-width: 18px; text-align: center; }
.bal { display: grid; grid-template-columns: repeat(auto-fill, minmax(190px, 1fr)); gap: 1px; background: var(--border,#1f2735); overflow-y: auto; align-content: start; }
.bal__c { background: var(--panel,#10141d); padding: 9px 14px; display: flex; flex-direction: column; gap: 3px; }
.bal__l { color: var(--muted,#6b7688); font-size: 11px; }
.bal__v { font-size: 13px; font-weight: 600; font-variant-numeric: tabular-nums; }
.bal__v.pos { color: var(--up,#26a69a); } .bal__v.neg { color: var(--down,#ef5350); } .bal__v.warn { color: #f5a524; }
.pane { flex: 1; display: flex; flex-direction: column; min-height: 0; }
.pane.hidden { display: none; }
.rows { overflow-y: auto; flex: 1; }
.signals-note { padding: 6px 12px; font-size: 10px; border-top: 1px solid var(--border,#1f2735); color: var(--muted,#6b7688); }
.deals__head, .deal-row { display: grid; grid-template-columns: var(--cols, 1fr .8fr 1fr 1fr 1fr 1fr .8fr); gap: 6px; padding: 5px 14px; align-items: center; }
.deals__head { color: var(--muted,#6b7688); font-size: 11px; }
.deal-row .lvl { color: var(--muted,#6b7688); }
.deal-row .lvl.sl { color: var(--down,#ef5350); } .deal-row .lvl.tp { color: var(--up,#26a69a); }
.deal-row { font-variant-numeric: tabular-nums; border-bottom: 1px solid var(--border,rgba(31,39,53,.5)); }
.deal-row:hover { background: var(--panel2,#151a25); }
.deal-row .sd.buy { color: var(--up,#26a69a); } .deal-row .sd.sell { color: var(--down,#ef5350); }
.deal-row .pnl.pos { color: var(--up,#26a69a); } .deal-row .pnl.neg { color: var(--down,#ef5350); }
.deal-row .dir.buy { color: var(--up,#26a69a); } .deal-row .dir.sell { color: var(--down,#ef5350); }
.deal-row .closebtn { justify-self: end; background: var(--panel3,#1a2130); border: 1px solid var(--border,#1f2735); color: var(--text,#d7dce5);
  border-radius: 4px; padding: 3px 8px; cursor: pointer; font-size: 11px; }
.deal-row span:nth-child(n+3) { text-align: right; }
.deal-row span:nth-child(2) { text-align: left; }
.ta-r { text-align: right; }
`;
  if (!document.getElementById('positions-css')) {
    const s = document.createElement('style'); s.id = 'positions-css'; s.textContent = CSS;
    document.head.appendChild(s);
  }

  // Заголовки — пары [ключ перевода, английский]. Порядок колонок у позиций
  // начинается так же, как раньше (инструмент…P&L), SL/TP — после P&L: на эти
  // позиции колонок опираются проверки и привычка глаза.
  const TABS = [
    { key: 'deals', title: ['p.positions', 'Positions'], count: true, cols: '1.1fr .7fr .9fr 1fr 1fr 1fr .9fr .9fr .8fr',
      head: [['p.instr', 'Instrument'], ['p.side', 'Side'], ['p.qty', 'Size'], ['p.entry', 'Entry'], ['p.mark', 'Current'], ['p.pnl', 'P&L'], ['f.sl', 'SL'], ['f.tp', 'TP'], null] },
    { key: 'pending', title: ['p.orders', 'Orders'], count: true, cols: '1.1fr .7fr .9fr 1fr 1fr 1fr .8fr',
      head: [['p.instr', 'Instrument'], ['p.side', 'Side'], ['p.qty', 'Size'], ['p.price', 'Price'], ['f.sl', 'SL'], ['f.tp', 'TP'], null] },
    { key: 'closed', title: ['p.history', 'History'], cols: '1.1fr .7fr .9fr 1fr 1fr 1fr .8fr',
      head: [['p.instr', 'Instrument'], ['p.side', 'Side'], ['p.qty', 'Size'], ['p.entry', 'Entry'], ['p.exit', 'Exit'], ['p.pnl', 'P&L'], ['p.comm', 'Comm.']] },
    { key: 'balance', title: ['p.balance', 'Balance'], balance: true },
    { key: 'signals', title: ['p.signals', 'Signals'], cols: '1fr 1.4fr .6fr .8fr',
      head: [['p.instr', 'Instrument'], ['p.signal', 'Signal'], ['p.tf', 'TF'], ['p.dir', 'Direction']] },
  ];
  const defUsd = (c) => `${c < 0 ? '-' : ''}$${(Math.abs(c) / 100).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
  const empty = (text) => `<div class="deal-row muted"><span style="grid-column: 1 / -1">${text}</span></div>`;

  function mount(root, ctx) {
    const fmtUsd = ctx.fmtUsd || defUsd;
    const pips = ctx.pips || (() => false);
    /** Перевод: хост даёт `t(ключ, английский)`; без него — английский (ADR-015). */
    const t = ctx.t || ((_k, en) => en);
    const L = (pair) => (pair ? t(pair[0], pair[1]) : '');
    // Пустой список и отсутствие доступа выглядят одинаково — «строк нет», — но значат
    // противоположное. Без сессии пишем причину, а не «No open positions» (BUG-011).
    const noSession = () => t('p.noSession', 'No cabinet session — trading unavailable');

    // Заголовки помечены data-i18n: хост переводит их при смене языка, не
    // перестраивая виджет (строки и так перерисовываются опросом).
    const headHtml = (cols) => `<div class="deals__head">${cols.map((c, i) => (c ? `<span${i >= 2 ? ' class="ta-r"' : ''} data-i18n="${c[0]}">${L(c)}</span>` : '<span></span>')).join('')}</div>`;
    root.innerHTML =
      `<div class="bottom__tabs">${TABS.map((tb, i) => `<span data-pane="${tb.key}"${i === 0 ? ' class="active"' : ''}><span data-i18n="${tb.title[0]}">${L(tb.title)}</span>${tb.count ? `<span class="cnt" data-cnt="${tb.key}" hidden></span>` : ''}</span>`).join('')}</div>` +
      TABS.map((tb, i) => `<div class="pane${i === 0 ? '' : ' hidden'}" data-pane="${tb.key}"${tb.cols ? ` style="--cols: ${tb.cols}"` : ''}>` +
        (tb.balance ? '<div class="bal" data-rows="balance"></div>' : `${headHtml(tb.head)}<div class="rows" data-rows="${tb.key}"></div>`) +
        (tb.key === 'signals' ? `<div class="signals-note" data-i18n="p.signalsNote">${t('p.signalsNote', 'Our simple indicator-based signals (RSI, MA cross). Not investment advice.')}</div>` : '') + '</div>').join('');
    /** Число строк во вкладке: «Позиции 3» видно, не открывая вкладку. */
    const setCount = (key, n) => {
      const el = root.querySelector(`[data-cnt="${key}"]`);
      if (!el) return;
      el.hidden = !(n > 0);
      el.textContent = String(n);
    };

    const rowsEl = (k) => root.querySelector(`[data-rows="${k}"]`);

    root.querySelector('.bottom__tabs').addEventListener('click', (e) => {
      const s = e.target.closest('[data-pane]'); if (s) show(s.dataset.pane);
    });
    function show(key) {
      root.querySelectorAll('.bottom__tabs [data-pane]').forEach((s) => s.classList.toggle('active', s.dataset.pane === key));
      root.querySelectorAll('.pane').forEach((p) => p.classList.toggle('hidden', p.dataset.pane !== key));
      if (key === 'balance') refreshBalance();
    }

    // Кнопки внутри строк вешаются делегированием: строки перерисовываются каждый тик.
    rowsEl('deals').addEventListener('click', async (e) => {
      const b = e.target.closest('.closebtn'); if (!b) return;
      const { ok } = await ctx.api.closeDeal(+b.dataset.id);
      if (ok) { refresh(); if (ctx.onChange) ctx.onChange(); }
    });
    rowsEl('pending').addEventListener('click', async (e) => {
      const b = e.target.closest('.closebtn'); if (!b) return;
      const { ok } = await ctx.api.cancelPending(+b.dataset.id);
      if (ok) { refresh(); if (ctx.onChange) ctx.onChange(); }
    });

    const lotOf = (d) => (d.qty / 10 ** d.qty_decimals).toFixed(d.qty_decimals);
    const pxOf = (d) => (raw) => (raw == null ? '—' : (raw / 10 ** d.price_decimals).toFixed(d.price_decimals));
    // Комиссия сделки (10.11): вся — открытия и закрытия. Поля нет (старый
    // шлюз) или ноль (тарифа не было) — пусто, а не «$0.00»: нулевая строка
    // читалась бы как «комиссию посчитали и она ноль» у сделки без тарифа.
    const commissionOf = (d) => (Number.isFinite(d.commission) && d.commission > 0 ? fmtUsd(d.commission) : '');
    // Устойчивый отказ (сессии нет) — пишем причину. Сетевой блип не трогает строки:
    // прежние позиции ещё верны, а «нет позиций» при живых позициях — опаснее.
    const denied = () => (ctx.api.tradingStatus ? ctx.api.tradingStatus() === 'no-session' : false);

    async function refreshDeals() {
      const list = await ctx.api.deals();
      if (!list) { if (denied()) { rowsEl('deals').innerHTML = empty(noSession()); setCount('deals', 0); } return; }
      if (ctx.onDeals) ctx.onDeals(list);
      setCount('deals', list.length);
      rowsEl('deals').innerHTML = list.map((d) => {
        const pos = d.pnl >= 0, f = pxOf(d);
        const p = pips() ? ` (${d.side === 'buy' ? d.mark - d.entry : d.entry - d.mark}p)` : '';
        return `<div class="deal-row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span><span>${lotOf(d)}</span><span>${f(d.entry)}</span><span>${f(d.mark)}</span><span class="pnl ${pos ? 'pos' : 'neg'}">${fmtUsd(d.pnl)}${p}</span><span class="lvl${d.sl != null ? ' sl' : ''}">${f(d.sl)}</span><span class="lvl${d.tp != null ? ' tp' : ''}">${f(d.tp)}</span><button class="closebtn" data-id="${d.id}">${t('p.close', 'Close')}</button></div>`;
      }).join('') || empty(t('p.noPositions', 'No open positions'));
    }
    async function refreshPending() {
      const list = await ctx.api.pending();
      if (!list) { if (denied()) { rowsEl('pending').innerHTML = empty(noSession()); setCount('pending', 0); } return; }
      setCount('pending', list.length);
      rowsEl('pending').innerHTML = list.map((d) => {
        const f = pxOf(d);
        return `<div class="deal-row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span><span>${lotOf(d)}</span><span>${f(d.price)}</span><span>${f(d.sl)}</span><span>${f(d.tp)}</span><button class="closebtn" data-id="${d.id}">${t('p.cancel', 'Cancel')}</button></div>`;
      }).join('') || empty(t('p.noOrders', 'No limit orders'));
    }
    async function refreshClosed() {
      const list = await ctx.api.closedDeals();
      if (!list) { if (denied()) rowsEl('closed').innerHTML = empty(noSession()); return; }
      rowsEl('closed').innerHTML = list.map((d) => {
        const pos = d.pnl >= 0, f = pxOf(d);
        return `<div class="deal-row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span><span>${lotOf(d)}</span><span>${f(d.entry)}</span><span>${f(d.exit)}</span><span class="pnl ${pos ? 'pos' : 'neg'}">${fmtUsd(d.pnl)}</span><span>${commissionOf(d)}</span></div>`;
      }).join('') || empty(t('p.noHistory', 'No history'));
    }
    function refreshSignals() {
      const list = (ctx.signals && ctx.signals()) || [];
      rowsEl('signals').innerHTML = list.map((s) =>
        `<div class="deal-row"><span>${s.symbol}</span><span>${s.name}</span><span>${s.tf}</span><span class="dir ${s.dir === 'BUY' ? 'buy' : 'sell'} ta-r">${s.dir}</span></div>`
      ).join('') || empty(t('p.noSignals', 'No signals on this TF'));
    }
    /**
     * Вкладка «Баланс»: сводку счёта собирает хост — у него ответ `/account`
     * и пороги площадки. Данных нет — это говорится словами, а не пустой
     * сеткой (BUG-011).
     */
    function refreshBalance() {
      const el = rowsEl('balance');
      if (!el || !ctx.balance) return;
      const data = ctx.balance();
      el.innerHTML = Array.isArray(data) && data.length
        ? data.map(([l, v, cls]) => `<div class="bal__c"><span class="bal__l">${l}</span><span class="bal__v ${cls || ''}">${v}</span></div>`).join('')
        : `<div class="bal__c"><span class="bal__l">${t('b.noData', 'No account data yet')}</span></div>`;
    }

    async function refresh() { await Promise.all([refreshDeals(), refreshPending(), refreshClosed()]); refreshBalance(); }

    refresh(); refreshSignals();
    const timer = ctx.interval ? setInterval(refresh, ctx.interval) : null;
    return {
      refresh, refreshDeals, refreshSignals, refreshBalance, show,
      destroy() { if (timer) clearInterval(timer); root.innerHTML = ''; },
    };
  }

  window.Positions = { mount };
})();
