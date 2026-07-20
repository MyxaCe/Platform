/* ============================================================================
 * trades.js — сделки по текущему инструменту (ADR-015): две вкладки —
 * MARKET TRADES (живая лента с биржи) и MY TRADES (мои исполнения по этой паре).
 *
 *   const trades = Trades.mount(el, {
 *     instrument: () => currentId,
 *     decimals: (id) => ({ pd, qd }),        // сколько знаков у цены/объёма
 *     myTrades: (id) => Promise<[deal]>,     // мои сделки по инструменту
 *     fmtUsd: (cents) => '$1.00',
 *     interval: 2000, max: 60,
 *   });
 *   trades.pushMarket({ instrument, price, qty, side });  // хост отдаёт тик из WS
 *   trades.refreshMine(); trades.show('market'); trades.destroy();
 *
 * Лента наполняется хостом (`pushMarket`), а не своим WS: сокет один на всё приложение
 * и им владеет хост. Виджет отвечает за показ, а не за транспорт — та же граница, что
 * у сигналов в positions.js.
 * ========================================================================== */
(function () {
  const CSS = `
.trades { display: flex; flex-direction: column; min-height: 0; background: var(--panel,#10141d); }
.trades__tabs { display: flex; height: 30px; align-items: stretch; border-bottom: 1px solid var(--border,#1f2735); }
.trades__tabs button { flex: 1; background: transparent; border: none; border-bottom: 2px solid transparent;
  color: var(--muted,#6b7688); font-weight: 700; font-size: 10px; letter-spacing: .5px; cursor: pointer; font-family: inherit; }
.trades__tabs button.active { color: var(--accent,#f0b90b); border-bottom-color: var(--accent,#f0b90b); }
.trades__head, .trow { display: grid; grid-template-columns: 1fr 1fr .8fr; gap: 4px; padding: 3px 10px;
  font-size: 11px; font-variant-numeric: tabular-nums; }
.trades__head { color: var(--muted,#6b7688); font-size: 10px; border-bottom: 1px solid var(--border,#1f2735); }
.trades__rows { overflow-y: auto; flex: 1; min-height: 0; }
.trow span:nth-child(n+2) { text-align: right; }
.trow .tp.buy { color: var(--up,#26a69a); } .trow .tp.sell { color: var(--down,#ef5350); }
.trow .pnl.pos { color: var(--up,#26a69a); } .trow .pnl.neg { color: var(--down,#ef5350); }
.trades__pane { display: flex; flex-direction: column; flex: 1; min-height: 0; }
.trades__pane.hidden { display: none; }
.trades__empty { padding: 8px 10px; font-size: 11px; color: var(--muted,#6b7688); }
`;
  if (!document.getElementById('trades-css')) {
    const s = document.createElement('style'); s.id = 'trades-css'; s.textContent = CSS;
    document.head.appendChild(s);
  }

  const hhmmss = (ms) => new Date(ms).toISOString().slice(11, 19);

  function mount(root, ctx) {
    const max = ctx.max || 60;
    const fmtUsd = ctx.fmtUsd || ((c) => `${(c / 100).toFixed(2)}`);
    root.classList.add('trades');
    root.innerHTML =
      '<div class="trades__tabs"><button data-t="market" class="active">MARKET TRADES</button><button data-t="mine">MY TRADES</button></div>' +
      '<div class="trades__pane" data-t="market"><div class="trades__head"><span>PRICE</span><span>QTY</span><span>TIME</span></div><div class="trades__rows" data-rows="market"></div></div>' +
      '<div class="trades__pane hidden" data-t="mine"><div class="trades__head"><span>SIDE / QTY</span><span>ENTRY → EXIT</span><span>P&L</span></div><div class="trades__rows" data-rows="mine"></div></div>';

    const rows = (k) => root.querySelector(`[data-rows="${k}"]`);
    root.querySelector('.trades__tabs').addEventListener('click', (e) => {
      const b = e.target.closest('[data-t]'); if (b) show(b.dataset.t);
    });
    function show(tab) {
      root.querySelectorAll('.trades__tabs button').forEach((b) => b.classList.toggle('active', b.dataset.t === tab));
      root.querySelectorAll('.trades__pane').forEach((p) => p.classList.toggle('hidden', p.dataset.t !== tab));
    }

    // ---- Лента рынка: тики приходят от хоста, храним последние `max` -----------
    let tape = [];
    function pushMarket(t) {
      const id = ctx.instrument();
      if (id == null || t.instrument !== id) return; // лента только по выбранной паре
      tape.unshift({ price: t.price, qty: t.qty, side: t.side, at: Date.now() });
      if (tape.length > max) tape.length = max;
      renderMarket();
    }
    function renderMarket() {
      const id = ctx.instrument();
      const { pd, qd } = (ctx.decimals && ctx.decimals(id)) || { pd: 2, qd: 5 };
      rows('market').innerHTML = tape.map((t) =>
        `<div class="trow"><span class="tp ${t.side === 'sell' ? 'sell' : 'buy'}">${(t.price / 10 ** pd).toFixed(pd)}</span><span>${(t.qty / 10 ** qd).toFixed(3)}</span><span class="muted">${hhmmss(t.at)}</span></div>`
      ).join('') || '<div class="trades__empty">Waiting for trades…</div>';
    }
    /** Смена инструмента: старая лента к новой паре отношения не имеет. */
    function clearTape() { tape = []; renderMarket(); }

    // ---- Мои сделки по инструменту -------------------------------------------
    async function refreshMine() {
      const id = ctx.instrument(); if (id == null) return;
      const list = (ctx.myTrades && (await ctx.myTrades(id))) || [];
      rows('mine').innerHTML = list.map((d) => {
        const f = (raw) => (raw / 10 ** d.price_decimals).toFixed(d.price_decimals);
        const lot = (d.qty / 10 ** d.qty_decimals).toFixed(d.qty_decimals);
        const pos = d.pnl >= 0;
        return `<div class="trow"><span class="tp ${d.side === 'sell' ? 'sell' : 'buy'}">${d.side.toUpperCase()} ${lot}</span><span>${f(d.entry)} → ${f(d.exit)}</span><span class="pnl ${pos ? 'pos' : 'neg'}">${fmtUsd(d.pnl)}</span></div>`;
      }).join('') || '<div class="trades__empty">No trades on this instrument</div>';
    }

    renderMarket(); refreshMine();
    const timer = ctx.interval ? setInterval(refreshMine, ctx.interval) : null;
    return {
      pushMarket, clearTape, refreshMine, show,
      destroy() { if (timer) clearInterval(timer); root.innerHTML = ''; },
    };
  }

  window.Trades = { mount };
})();
