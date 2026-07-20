/* ============================================================================
 * orderbook.js — переиспользуемый виджет стакана (ADR-015).
 * Не зависит от страницы: получает точку монтирования + контекст (ctx), сам строит
 * свой DOM, сам опрашивает данные, сам чистится. Ставится на любую страницу так:
 *   const book = OrderBook.mount(el, {
 *     instrument: () => currentId,           // текущий инструмент (или null)
 *     fetch: (id) => Promise<Depth>,         // источник данных (инъекция)
 *     onPick: (priceStr) => {...},           // клик по уровню (опц.)
 *     interval: 700, levels: 10,             // опц.
 *   });
 * Depth = { price_decimals, qty_decimals, bids:[[price,qty]], asks:[[price,qty]] }.
 * Возвращает { refresh(), destroy() }.
 * ========================================================================== */
(function () {
  // Стили компонента едут вместе с модулем (ADR-015). Токены темы — контракт хоста, с фолбэками.
  // Размещение корня в раскладке страницы (.deal__book) остаётся за хостом — компонент не решает,
  // где ему стоять; он отвечает только за свой внутренний вид.
  const CSS = `
.book__head { padding: 7px 12px; font-size: 10px; letter-spacing: .5px; color: var(--muted,#6b7688); text-transform: uppercase; display: flex; justify-content: space-between; }
.book__side { flex: 1; overflow: hidden; display: flex; flex-direction: column; }
.book__side.asks { justify-content: flex-end; }
.brow { position: relative; display: flex; justify-content: space-between; padding: 2px 12px; font-variant-numeric: tabular-nums; font-size: 11px; cursor: pointer; }
.brow .bar { position: absolute; top: 0; bottom: 0; right: 0; }
.brow.ask .bar { background: rgba(239,83,80,.15); } .brow.bid .bar { background: rgba(38,166,154,.15); }
.brow.ask .bp { color: var(--down,#ef5350); } .brow.bid .bp { color: var(--up,#26a69a); }
.brow span { position: relative; z-index: 1; }
.book__mid { text-align: center; padding: 5px; font-weight: 700; font-variant-numeric: tabular-nums; border-top: 1px solid var(--border,#1f2735); border-bottom: 1px solid var(--border,#1f2735); }
`;
  if (!document.getElementById('orderbook-css')) {
    const s = document.createElement('style'); s.id = 'orderbook-css'; s.textContent = CSS;
    document.head.appendChild(s);
  }

  window.OrderBook = {
    mount(root, ctx) {
      const levels = ctx.levels || 10;
      root.innerHTML =
        '<div class="book__head">ORDER BOOK <span class="book__spread muted"></span></div>' +
        '<div class="book__side asks"></div>' +
        '<div class="book__mid">—</div>' +
        '<div class="book__side bids"></div>';
      const elSpread = root.querySelector('.book__spread');
      const elAsks = root.querySelector('.asks');
      const elMid = root.querySelector('.book__mid');
      const elBids = root.querySelector('.bids');

      root.addEventListener('click', (e) => {
        const r = e.target.closest('.brow');
        if (r && r.dataset.p && ctx.onPick) ctx.onPick(r.dataset.p);
      });

      async function refresh() {
        const id = ctx.instrument(); if (id == null) return;
        let d; try { d = await ctx.fetch(id); } catch { return; }
        if (!d || !d.asks) return;
        const s = 10 ** d.price_decimals, v = 10 ** d.qty_decimals;
        const asks = d.asks.slice(0, levels), bids = d.bids.slice(0, levels);
        const maxQ = Math.max(1, ...asks.map((a) => a[1]), ...bids.map((b) => b[1]));
        const row = (lvl, cls) => `<div class="brow ${cls}" data-p="${(lvl[0] / s).toFixed(d.price_decimals)}"><div class="bar" style="width:${lvl[1] / maxQ * 100}%"></div><span class="bp">${(lvl[0] / s).toFixed(d.price_decimals)}</span><span class="bq">${(lvl[1] / v).toFixed(3)}</span></div>`;
        elAsks.innerHTML = asks.slice().reverse().map((a) => row(a, 'ask')).join('');
        elBids.innerHTML = bids.map((b) => row(b, 'bid')).join('');
        const bb = bids[0]?.[0], ba = asks[0]?.[0];
        elMid.textContent = bb && ba ? ((bb + ba) / 2 / s).toFixed(d.price_decimals) : '—';
        elSpread.textContent = bb && ba ? `spread ${((ba - bb) / s).toFixed(d.price_decimals)}` : '';
      }

      refresh(); // первичная отрисовка сразу при монтировании — виджет ставится одной строкой
      const timer = ctx.interval ? setInterval(refresh, ctx.interval) : null;
      return { refresh, destroy() { if (timer) clearInterval(timer); root.innerHTML = ''; } };
    },
  };
})();
