/* ============================================================================
 * markets.js — таблица рынков с живыми котировками (контракт ADR-015: свой DOM,
 * свои стили, зависимости через ctx).
 *
 *   const m = Markets.mount(el, {
 *     fetch: () => Promise<[{ symbol, last, change, high, price_decimals }]>,
 *     t: (key) => 'Pair',      // словарь (опц.)
 *     limit: 8, interval: 5000,
 *   });
 *   m.filter('btc'); m.relabel(); m.destroy();
 *
 * Цены форматирует сам по price_decimals из данных — внешний форматтер не нужен.
 * ========================================================================== */
(function () {
  const CSS = `
.mk { border: 1px solid var(--line); border-radius: 12px; overflow: hidden; background: var(--bg); }
.mk__head, .mk__row {
  display: grid; grid-template-columns: 1.6fr 1fr 1fr 1fr; gap: 12px; align-items: center;
  padding: 13px 20px; font-variant-numeric: tabular-nums;
}
.mk__head { color: var(--muted); font-size: 12px; background: var(--surface); border-bottom: 1px solid var(--line); }
.mk__row { border-bottom: 1px solid var(--line); font-size: 14.5px; }
.mk__row:last-child { border-bottom: none; }
.mk__row:hover { background: var(--surface); }
.mk__head span:nth-child(n+2), .mk__row span:nth-child(n+2) { text-align: right; }
.mk__pair { display: flex; align-items: center; gap: 10px; font-weight: 600; }
.mk__badge {
  width: 26px; height: 26px; flex: 0 0 26px; border-radius: 50%; display: inline-flex;
  align-items: center; justify-content: center; font-size: 11px; font-weight: 700; color: #fff;
}
.mk__quote { color: var(--muted); font-weight: 400; }
.mk__empty { padding: 26px 20px; color: var(--muted); text-align: center; }
`;
  if (!document.getElementById('markets-css')) {
    const s = document.createElement('style'); s.id = 'markets-css'; s.textContent = CSS;
    document.head.appendChild(s);
  }

  /** Стабильный цвет бейджа по тикеру — чтобы не тянуть картинки логотипов. */
  const hue = (s) => { let h = 0; for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) % 360; return h; };

  function mount(root, ctx) {
    const limit = ctx.limit || 8;
    const t = ctx.t || ((k) => k);
    let all = [];
    let query = '';

    root.classList.add('mk');
    root.innerHTML = '<div class="mk__head"></div><div class="mk__body"></div>';
    const head = root.querySelector('.mk__head');
    const body = root.querySelector('.mk__body');

    function relabel() {
      head.innerHTML = `<span>${t('markets.pair')}</span><span>${t('markets.price')}</span>` +
        `<span>${t('markets.change')}</span><span>${t('markets.high')}</span>`;
      render();
    }

    function render() {
      if (!all.length) { body.innerHTML = `<div class="mk__empty">${t('markets.loading')}</div>`; return; }
      const q = query.trim().toLowerCase();
      const list = (q ? all.filter((i) => i.symbol.toLowerCase().includes(q)) : all).slice(0, limit);
      if (!list.length) { body.innerHTML = `<div class="mk__empty">${t('markets.empty')}</div>`; return; }
      body.innerHTML = list.map((i) => {
        const [base, quote] = i.symbol.split('-');
        const pd = i.price_decimals ?? 2;
        const px = (v) => (v / 10 ** pd).toLocaleString('en-US', { minimumFractionDigits: pd, maximumFractionDigits: pd });
        const up = i.change >= 0;
        return `<div class="mk__row">
          <span class="mk__pair"><i class="mk__badge" style="background:hsl(${hue(base)} 52% 42%)">${base[0]}</i>
            ${base}<span class="mk__quote">/${quote}</span></span>
          <span>${px(i.last)}</span>
          <span class="${up ? 'up' : 'down'}">${up ? '+' : ''}${(i.change ?? 0).toFixed(2)}%</span>
          <span class="muted">${px(i.high)}</span>
        </div>`;
      }).join('');
    }

    async function refresh() {
      let list; try { list = await ctx.fetch(); } catch { return; }
      if (!Array.isArray(list) || !list.length) return;
      all = list;
      render();
    }

    relabel();
    refresh();
    const timer = ctx.interval ? setInterval(refresh, ctx.interval) : null;
    return {
      refresh, relabel,
      filter(q) { query = q || ''; render(); },
      destroy() { if (timer) clearInterval(timer); root.innerHTML = ''; },
    };
  }

  window.Markets = { mount };
})();
