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
 *     interval: 1500,
 *   });
 *   panel.refresh(); panel.refreshSignals(); panel.show('pending'); panel.destroy();
 *
 * Почему сигналы приходят снаружи: они считаются из свечей графика, которыми владеет
 * хост. Виджет отвечает за их показ, но не за расчёт.
 * ========================================================================== */
(function () {
  const CSS = `
.bottom__tabs { padding: 0 12px; height: 32px; display: flex; align-items: center; gap: 18px; border-bottom: 1px solid var(--border,#1f2735); }
.bottom__tabs span { color: var(--muted,#6b7688); font-weight: 700; font-size: 11px; letter-spacing: .5px; padding: 8px 0; cursor: pointer; border-bottom: 2px solid transparent; }
.bottom__tabs span.active { color: var(--accent,#f0b90b); border-bottom-color: var(--accent,#f0b90b); }
.pane { flex: 1; display: flex; flex-direction: column; min-height: 0; }
.pane.hidden { display: none; }
.rows { overflow-y: auto; flex: 1; }
.signals-note { padding: 6px 12px; font-size: 10px; border-top: 1px solid var(--border,#1f2735); color: var(--muted,#6b7688); }
.deals__head, .deal-row { display: grid; grid-template-columns: 1fr .8fr 1fr 1fr 1fr 1fr .8fr; padding: 5px 12px; align-items: center; }
.deals__head { color: var(--muted,#6b7688); font-size: 10px; }
.deal-row { font-variant-numeric: tabular-nums; border-bottom: 1px solid rgba(31,39,53,.5); }
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

  const TABS = [
    { key: 'deals', title: 'OPEN DEALS', head: ['INSTR.', 'SIDE', 'QTY', 'ENTRY', 'PRICE', 'P&L', ''] },
    { key: 'pending', title: 'LIMIT ORDERS', head: ['INSTR.', 'SIDE', 'QTY', 'PRICE', 'SL', 'TP', ''] },
    { key: 'closed', title: 'CLOSED DEALS', head: ['INSTR.', 'SIDE', 'QTY', 'ENTRY', 'EXIT', 'P&L', ''] },
    { key: 'signals', title: 'SIGNALS', head: ['INSTR.', 'SIGNAL', 'TF', 'DIRECTION'] },
  ];
  const defUsd = (c) => `$${(c / 100).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
  const empty = (text) => `<div class="deal-row muted"><span>${text}</span></div>`;

  function mount(root, ctx) {
    const fmtUsd = ctx.fmtUsd || defUsd;
    const pips = ctx.pips || (() => false);

    const headHtml = (cols) => `<div class="deals__head">${cols.map((c, i) => `<span${i >= 2 && c ? ' class="ta-r"' : ''}>${c}</span>`).join('')}</div>`;
    root.innerHTML =
      `<div class="bottom__tabs">${TABS.map((t, i) => `<span data-pane="${t.key}"${i === 0 ? ' class="active"' : ''}>${t.title}</span>`).join('')}</div>` +
      TABS.map((t, i) => `<div class="pane${i === 0 ? '' : ' hidden'}" data-pane="${t.key}">${headHtml(t.head)}<div class="rows" data-rows="${t.key}"></div>` +
        (t.key === 'signals' ? '<div class="signals-note">Our simple indicator-based signals (RSI, MA cross). Not Autochartist.</div>' : '') + '</div>').join('');

    const rowsEl = (k) => root.querySelector(`[data-rows="${k}"]`);

    root.querySelector('.bottom__tabs').addEventListener('click', (e) => {
      const s = e.target.closest('[data-pane]'); if (s) show(s.dataset.pane);
    });
    function show(key) {
      root.querySelectorAll('.bottom__tabs [data-pane]').forEach((s) => s.classList.toggle('active', s.dataset.pane === key));
      root.querySelectorAll('.pane').forEach((p) => p.classList.toggle('hidden', p.dataset.pane !== key));
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

    async function refreshDeals() {
      const list = await ctx.api.deals(); if (!list) return;
      if (ctx.onDeals) ctx.onDeals(list);
      rowsEl('deals').innerHTML = list.map((d) => {
        const pos = d.pnl >= 0, f = pxOf(d);
        const p = pips() ? ` (${d.side === 'buy' ? d.mark - d.entry : d.entry - d.mark}p)` : '';
        return `<div class="deal-row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span><span>${lotOf(d)}</span><span>${f(d.entry)}</span><span>${f(d.mark)}</span><span class="pnl ${pos ? 'pos' : 'neg'}">${fmtUsd(d.pnl)}${p}</span><button class="closebtn" data-id="${d.id}">Close</button></div>`;
      }).join('') || empty('No open positions');
    }
    async function refreshPending() {
      const list = await ctx.api.pending(); if (!list) return;
      rowsEl('pending').innerHTML = list.map((d) => {
        const f = pxOf(d);
        return `<div class="deal-row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span><span>${lotOf(d)}</span><span>${f(d.price)}</span><span>${f(d.sl)}</span><span>${f(d.tp)}</span><button class="closebtn" data-id="${d.id}">Cancel</button></div>`;
      }).join('') || empty('No limit orders');
    }
    async function refreshClosed() {
      const list = await ctx.api.closedDeals(); if (!list) return;
      rowsEl('closed').innerHTML = list.map((d) => {
        const pos = d.pnl >= 0, f = pxOf(d);
        return `<div class="deal-row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span><span>${lotOf(d)}</span><span>${f(d.entry)}</span><span>${f(d.exit)}</span><span class="pnl ${pos ? 'pos' : 'neg'}">${fmtUsd(d.pnl)}</span><span></span></div>`;
      }).join('') || empty('No history');
    }
    function refreshSignals() {
      const list = (ctx.signals && ctx.signals()) || [];
      rowsEl('signals').innerHTML = list.map((s) =>
        `<div class="deal-row"><span>${s.symbol}</span><span>${s.name}</span><span>${s.tf}</span><span class="dir ${s.dir === 'BUY' ? 'buy' : 'sell'} ta-r">${s.dir}</span></div>`
      ).join('') || empty('No signals on this TF');
    }

    async function refresh() { await Promise.all([refreshDeals(), refreshPending(), refreshClosed()]); }

    refresh(); refreshSignals();
    const timer = ctx.interval ? setInterval(refresh, ctx.interval) : null;
    return {
      refresh, refreshDeals, refreshSignals, show,
      destroy() { if (timer) clearInterval(timer); root.innerHTML = ''; },
    };
  }

  window.Positions = { mount };
})();
