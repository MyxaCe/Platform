/* ============================================================================
 * stats.js — переиспользуемый виджет статистики актива по периодам (ADR-015).
 * Не зависит от страницы: mount(el, ctx) строит ячейки «период → изменение %».
 *   const stats = AssetStats.mount(el, {
 *     instrument: () => currentId,
 *     fetch: (id) => Promise<[{ tf, change }]>,  // change в процентах
 *     labels: { 60: '1m', ... },                 // подписи периодов (опц.)
 *     interval: 5000,                            // опц.
 *   });
 * Возвращает { refresh(), destroy() }.
 * ========================================================================== */
(function () {
  // Стили компонента едут вместе с модулем (ADR-015): страница-хост может не иметь своей CSS.
  // Токены темы (--panel/--muted/--up/--down) — контракт хоста; у каждого задан фолбэк,
  // поэтому виджет отрисуется корректно и на странице без темы.
  const CSS = `
.statbar { display: flex; height: 30px; flex: 0 0 30px; align-items: stretch; border-top: 1px solid var(--border,#1f2735); background: var(--panel,#10141d); }
.stat { flex: 1; display: flex; align-items: center; justify-content: center; gap: 6px; border-right: 1px solid rgba(31,39,53,.5); font-size: 11px; }
.stat:last-child { border-right: none; }
.stat__l { color: var(--muted,#6b7688); text-transform: uppercase; }
.stat__v { font-variant-numeric: tabular-nums; font-weight: 600; }
.stat__v.up { color: var(--up,#26a69a); } .stat__v.down { color: var(--down,#ef5350); }
`;
  if (!document.getElementById('assetstats-css')) {
    const s = document.createElement('style'); s.id = 'assetstats-css'; s.textContent = CSS;
    document.head.appendChild(s);
  }

  window.AssetStats = {
    mount(root, ctx) {
      async function refresh() {
        const id = ctx.instrument(); if (id == null) return;
        let list; try { list = await ctx.fetch(id); } catch { return; }
        if (!Array.isArray(list)) return;
        root.innerHTML = list.map((st) => {
          const up = st.change >= 0;
          const label = (ctx.labels && ctx.labels[st.tf]) || st.tf;
          return `<div class="stat"><span class="stat__l">${label}</span><span class="stat__v ${up ? 'up' : 'down'}">${up ? '+' : ''}${st.change.toFixed(2)}%</span></div>`;
        }).join('');
      }
      refresh(); // первичная отрисовка сразу при монтировании — виджет ставится одной строкой
      const timer = ctx.interval ? setInterval(refresh, ctx.interval) : null;
      return { refresh, destroy() { if (timer) clearInterval(timer); root.innerHTML = ''; } };
    },
  };
})();
