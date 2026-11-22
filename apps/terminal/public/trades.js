(function () {
  const CSS = `
.trades { display: flex; flex-direction: column; min-height: 0; background: var(--panel,#10141d); }
.trades__tabs { display: flex; height: 36px; gap: 18px; padding: 0 12px; align-items: stretch; border-bottom: 1px solid var(--border,#1f2735); }
.trades__tabs button { background: transparent; border: none; border-bottom: 2px solid transparent; padding: 0;
  color: var(--muted,#6b7688); font-weight: 700; font-size: 12.5px; cursor: pointer; font-family: inherit; white-space: nowrap; }
.trades__tabs button.active { color: var(--text,#d7dce5); border-bottom-color: var(--brand,#f0b90b); }
.trades__src { margin-left: auto; align-self: center; font-size: 10.5px; font-weight: 600; color: var(--muted,#6b7688); }
.trades__head, .trow { display: grid; grid-template-columns: 1fr 1fr .8fr; gap: 4px; padding: 3px 10px;
  font-size: 11px; font-variant-numeric: tabular-nums; }
.trades__head { color: var(--muted,#6b7688); font-size: 11px; border-bottom: 1px solid var(--border,#1f2735); }
.trades__rows { overflow-y: auto; flex: 1; min-height: 0; }
.trow span:nth-child(n+2), .trades__head span:nth-child(n+2) { text-align: right; }
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

  /** @param {number} ms */
  const hhmmss = (ms) => new Date(ms).toISOString().slice(11, 19);

  /**
   * @param {HTMLElement} root
   * @param {any} ctx
   */
  function mount(root, ctx) {
    const max = ctx.max || 60;
    const fmtUsd = ctx.fmtUsd || ((/** @type {number} */ c) => `${(c / 100).toFixed(2)}`);
    const num = ctx.num || ((/** @type {number} */ v, /** @type {number} */ n) => v.toFixed(n));
    const t = ctx.t || ((/** @type {string} */ _k, /** @type {string} */ en) => en);
    const tx = (/** @type {string} */ k, /** @type {string} */ en) => `<span data-i18n="${k}">${t(k, en)}</span>`;
    root.classList.add('trades');
    const src = ctx.source ? `<span class="trades__src">${ctx.source}</span>` : '';
    root.innerHTML =
      `<div class="trades__tabs"><button data-t="market" class="active">${tx('tr.market', 'MARKET TRADES')}</button><button data-t="mine">${tx('tr.mine', 'MY TRADES')}</button>${src}</div>` +
      `<div class="trades__pane" data-t="market"><div class="trades__head">${tx('tr.price', 'PRICE')}${tx('tr.qty', 'QTY')}${tx('tr.time', 'TIME')}</div><div class="trades__rows" data-rows="market"></div></div>` +
      `<div class="trades__pane hidden" data-t="mine"><div class="trades__head">${tx('tr.sideQty', 'SIDE / QTY')}${tx('tr.entryExit', 'ENTRY → EXIT')}<span>P&L</span></div><div class="trades__rows" data-rows="mine"></div></div>`;

    const rows = (/** @type {string} */ k) => root.querySelector(`[data-rows="${k}"]`);
    root.querySelector('.trades__tabs').addEventListener('click', (/** @type {Event} */ e) => {
      const b = /** @type {HTMLElement} */ (/** @type {Element} */ (e.target).closest('[data-t]'));
      if (b) show(b.dataset.t);
    });
    /** @param {string} tab */
    function show(tab) {
      root.querySelectorAll('.trades__tabs button').forEach((b) => b.classList.toggle('active', /** @type {HTMLElement} */ (b).dataset.t === tab));
      root.querySelectorAll('.trades__pane').forEach((p) => p.classList.toggle('hidden', /** @type {HTMLElement} */ (p).dataset.t !== tab));
    }

    /** @type {Array<{ price: number, qty: number, side: string, at: number }>} */
    let tape = [];
    /** @param {{ instrument: number, price: number, qty: number, side: string }} t */
    function pushMarket(t) {
      const id = ctx.instrument();
      if (id == null || t.instrument !== id) return;
      tape.unshift({ price: t.price, qty: t.qty, side: t.side, at: Date.now() });
      if (tape.length > max) tape.length = max;
      renderMarket();
    }
    function renderMarket() {
      const id = ctx.instrument();
      const { pd, qd } = (ctx.decimals && ctx.decimals(id)) || { pd: 2, qd: 5 };
      if (id != null && ctx.feed && ctx.feed(id) === false) {
        rows('market').innerHTML = `<div class="trades__empty">${t('tr.noFeed', 'No trade feed for this instrument — prices arrive as quotes.')}</div>`;
        return;
      }
      rows('market').innerHTML = tape.map((t) =>
        `<div class="trow"><span class="tp ${t.side === 'sell' ? 'sell' : 'buy'}">${num(t.price / 10 ** pd, pd)}</span><span>${num(t.qty / 10 ** qd, qd)}</span><span class="muted">${hhmmss(t.at)}</span></div>`
      ).join('') || `<div class="trades__empty">${t('tr.waiting', 'Waiting for trades…')}</div>`;
    }
    function clearTape() { tape = []; renderMarket(); }

    async function refreshMine() {
      const id = ctx.instrument(); if (id == null) return;
      if (!ctx.myTrades) return;

      const list = await ctx.myTrades(id);

      if (!Array.isArray(list)) {
        if (!rows('mine').innerHTML) {
          rows('mine').innerHTML = `<div class="trades__empty">${t('tr.unreadable', 'Не удалось прочитать сделки')}</div>`;
        }
        return;
      }
      rows('mine').innerHTML = list.map((d) => {
        const f = (/** @type {number} */ raw) => num(raw / 10 ** d.price_decimals, d.price_decimals);
        const sized = ctx.size ? ctx.size(d) : null;
        const lot = sized != null ? sized : num(d.qty / 10 ** d.qty_decimals, d.qty_decimals);
        const pos = d.pnl >= 0;
        return `<div class="trow"><span class="tp ${d.side === 'sell' ? 'sell' : 'buy'}">${d.side.toUpperCase()} ${lot}</span><span>${f(d.entry)} → ${f(d.exit)}</span><span class="pnl ${pos ? 'pos' : 'neg'}">${fmtUsd(d.pnl)}</span></div>`;
      }).join('') || `<div class="trades__empty">${t('tr.none', 'No trades on this instrument')}</div>`;
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
